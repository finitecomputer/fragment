//! A TDX quote through configfs-tsm (Documentation/ABI/testing/
//! configfs-tsm-report in the kernel): make a report directory, write
//! the 64-byte REPORTDATA to `inblob`, read `outblob`, and check that
//! `generation` did not move while we read, so the quote is for our
//! write and not another writer's. Then remove the directory.
//!
//! Plain file operations, so it builds everywhere and the tests run it
//! against a temporary directory standing in for configfs. Only a TDX
//! guest's kernel makes `outblob` a quote.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CONFIGFS_REPORT: &str = "/sys/kernel/config/tsm/report";
pub const CCEL_DATA: &str = "/sys/firmware/acpi/tables/data/CCEL";
/// Larger than any DCAP quote (ours is 8,000 bytes).
const MAX_QUOTE: usize = 64 * 1024;

#[derive(Debug)]
pub enum TsmError {
    Io { step: &'static str, path: PathBuf, err: io::Error },
    Generation { step: &'static str, text: String },
    /// Someone else wrote our report directory between our write and our read.
    GenerationMoved { before: u64, after: u64, expected: u64 },
    Provider(String),
    EmptyQuote,
    QuoteTooLarge(usize),
}

impl fmt::Display for TsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { step, path, err } => write!(f, "{step} {}: {err}", path.display()),
            Self::Generation { step, text } => write!(f, "{step}: generation {text:?} is not a number"),
            Self::GenerationMoved { before, after, expected } => write!(
                f,
                "generation moved: {before} then {after}, expected {expected}; another writer used the report"
            ),
            Self::Provider(p) => write!(f, "configfs-tsm provider is {p:?}, not tdx_guest"),
            Self::EmptyQuote => write!(f, "outblob was empty"),
            Self::QuoteTooLarge(n) => write!(f, "outblob is {n} bytes; refusing over {MAX_QUOTE}"),
        }
    }
}

impl std::error::Error for TsmError {}

fn io_err<'a>(step: &'static str, path: &'a Path) -> impl FnOnce(io::Error) -> TsmError + 'a {
    move |err| TsmError::Io { step, path: path.to_owned(), err }
}

/// One report directory, removed on drop if a step failed.
pub struct Report {
    dir: PathBuf,
    removed: bool,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Report {
    pub fn create(root: &Path) -> Result<Self, TsmError> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("pod-init-{}-{n}", std::process::id()));
        fs::create_dir(&dir).map_err(io_err("mkdir", &dir))?;
        Ok(Self { dir, removed: false })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn generation(&self) -> Result<u64, TsmError> {
        let path = self.dir.join("generation");
        let text = fs::read_to_string(&path).map_err(io_err("read", &path))?;
        text.trim()
            .parse()
            .map_err(|_| TsmError::Generation { step: "read", text })
    }

    /// The provider file exists on kernels that name it; if it does, it
    /// must be the TDX guest driver.
    pub fn check_provider(&self) -> Result<(), TsmError> {
        let path = self.dir.join("provider");
        match fs::read_to_string(&path) {
            Ok(p) if p.trim() == "tdx_guest" => Ok(()),
            Ok(p) => Err(TsmError::Provider(p.trim().to_owned())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TsmError::Io { step: "read", path, err: e }),
        }
    }

    pub fn write_inblob(&self, report_data: &[u8; 64]) -> Result<(), TsmError> {
        let path = self.dir.join("inblob");
        fs::write(&path, report_data).map_err(io_err("write", &path))
    }

    pub fn read_outblob(&self) -> Result<Vec<u8>, TsmError> {
        let path = self.dir.join("outblob");
        let quote = fs::read(&path).map_err(io_err("read", &path))?;
        match quote.len() {
            0 => Err(TsmError::EmptyQuote),
            n if n > MAX_QUOTE => Err(TsmError::QuoteTooLarge(n)),
            _ => Ok(quote),
        }
    }

    pub fn remove(mut self) -> Result<(), TsmError> {
        self.removed = true;
        fs::remove_dir(&self.dir).map_err(io_err("rmdir", &self.dir))
    }
}

impl Drop for Report {
    fn drop(&mut self) {
        if !self.removed {
            // Best effort: configfs removes the attribute files with the
            // directory; a failure here leaves one empty report behind.
            let _ = fs::remove_dir(&self.dir);
        }
    }
}

/// What the kernel does by itself in a configfs report directory. The
/// real kernel needs no help; the tests stand in for it with files.
pub trait Kernel {
    /// mkdir made the attribute files (`generation` at 0, `provider`).
    fn after_mkdir(&self, _dir: &Path) {}
    /// Writing `inblob` bumped `generation` and made `outblob`.
    fn after_inblob(&self, _dir: &Path) {}
    /// rmdir on configfs takes the attribute files with it.
    fn before_rmdir(&self, _dir: &Path) {}
}

pub struct Configfs;
impl Kernel for Configfs {}

/// The checked sequence: mkdir, read generation, write inblob, read
/// generation, read outblob, read generation, rmdir. Both reads after
/// the write must see exactly one more than before it.
pub fn quote_with(root: &Path, report_data: &[u8; 64], kernel: &dyn Kernel) -> Result<Vec<u8>, TsmError> {
    let report = Report::create(root)?;
    kernel.after_mkdir(report.dir());
    report.check_provider()?;
    let start = report.generation()?;
    report.write_inblob(report_data)?;
    kernel.after_inblob(report.dir());
    let before = report.generation()?;
    let quote = report.read_outblob()?;
    let after = report.generation()?;
    let expected = start + 1;
    kernel.before_rmdir(report.dir());
    if before != expected || after != expected {
        return Err(TsmError::GenerationMoved { before, after, expected });
    }
    report.remove()?;
    Ok(quote)
}

pub fn quote(root: &Path, report_data: &[u8; 64]) -> Result<Vec<u8>, TsmError> {
    quote_with(root, report_data, &Configfs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Stands in for the TDX guest driver behind configfs-tsm.
    pub struct FakeKernel {
        pub provider: &'static str,
        /// What a write to inblob does to generation (1 is honest).
        pub bump: u64,
        /// The quote is "QUOTE" followed by the REPORTDATA, or this.
        pub fixed: Option<&'static [u8]>,
    }

    impl FakeKernel {
        pub fn honest() -> Self {
            Self { provider: "tdx_guest", bump: 1, fixed: None }
        }
    }

    impl Kernel for FakeKernel {
        fn after_mkdir(&self, dir: &Path) {
            fs::write(dir.join("generation"), "0\n").unwrap();
            fs::write(dir.join("provider"), format!("{}\n", self.provider)).unwrap();
        }
        fn after_inblob(&self, dir: &Path) {
            let inblob = fs::read(dir.join("inblob")).unwrap();
            fs::write(dir.join("generation"), format!("{}\n", self.bump)).unwrap();
            let quote = match self.fixed {
                Some(q) => q.to_vec(),
                None => [b"QUOTE".as_slice(), &inblob].concat(),
            };
            fs::write(dir.join("outblob"), quote).unwrap();
        }
        fn before_rmdir(&self, dir: &Path) {
            for entry in fs::read_dir(dir).unwrap() {
                fs::remove_file(entry.unwrap().path()).unwrap();
            }
        }
    }

    #[test]
    fn reads_the_quote_for_our_report_data_and_removes_the_directory() {
        let root = crate::scratch::Scratch::new();
        let quote = quote_with(root.path(), &[7; 64], &FakeKernel::honest()).unwrap();
        assert_eq!(quote, [b"QUOTE".as_slice(), &[7; 64]].concat());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn refuses_when_generation_moved_and_cleans_up() {
        let root = crate::scratch::Scratch::new();
        let kernel = FakeKernel { bump: 2, ..FakeKernel::honest() };
        let err = quote_with(root.path(), &[7; 64], &kernel).unwrap_err();
        assert!(matches!(err, TsmError::GenerationMoved { before: 2, after: 2, expected: 1 }), "{err}");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn refuses_another_provider_and_an_empty_quote() {
        let root = crate::scratch::Scratch::new();
        let sev = FakeKernel { provider: "sev_guest", ..FakeKernel::honest() };
        let err = quote_with(root.path(), &[7; 64], &sev).unwrap_err();
        assert!(matches!(err, TsmError::Provider(ref p) if p == "sev_guest"), "{err}");
        let empty = FakeKernel { fixed: Some(b""), ..FakeKernel::honest() };
        let err = quote_with(root.path(), &[7; 64], &empty).unwrap_err();
        assert!(matches!(err, TsmError::EmptyQuote), "{err}");
    }

    #[test]
    fn says_which_step_failed_without_configfs() {
        let err = quote(Path::new("/nonexistent/tsm/report"), &[0; 64]).unwrap_err();
        assert!(matches!(err, TsmError::Io { step: "mkdir", .. }), "{err}");
    }
}
