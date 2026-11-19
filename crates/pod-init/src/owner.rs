//! The owner as files: inside the volume (`owner_file`, the truth once
//! open) and beside it (`owner_hint`, what the host's disk says while
//! locked). Both are the x-only key as 64 lowercase hex and a newline.

use fragment_pod::unlock::wire;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// None if the file does not exist.
pub fn read(path: &Path) -> Result<Option<[u8; 32]>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    wire::hex32("owner", text.trim_end_matches('\n')).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write through a temporary file and a rename, so a crash leaves the
/// old file or the new one, never half of one.
pub fn write(path: &Path, owner: &[u8; 32]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(format!("{}\n", hex::encode(owner)).as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_refuses_garbage() {
        let dir = crate::scratch::Scratch::new();
        let p = dir.path().join("pod/owner");
        assert_eq!(read(&p), Ok(None));
        write(&p, &[0xab; 32]).unwrap();
        assert_eq!(read(&p), Ok(Some([0xab; 32])));
        fs::write(&p, "nope\n").unwrap();
        assert!(read(&p).is_err());
    }
}
