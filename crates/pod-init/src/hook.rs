//! The open hook: an executable run once the volume is mounted (to start
//! what lives on it). It runs while the service waits, one request at a
//! time, so it is bounded: past the timeout it is killed and reported.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(50);

/// Run `hook` with no arguments; its output goes to pod-init's stderr.
pub fn run(hook: &Path, timeout: Duration) -> Result<(), String> {
    let mut child = Command::new(hook)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("{}: {e}", hook.display()))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{} {status}", hook.display())),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} still running after {}s; killed", hook.display(), timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(format!("{}: {e}", hook.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_exit_status_missing_hooks_and_timeouts() {
        assert_eq!(run(Path::new("/usr/bin/true"), TIMEOUT), Ok(()));
        assert!(run(Path::new("/usr/bin/false"), TIMEOUT).unwrap_err().contains("exit"));
        assert!(run(Path::new("/nonexistent/hook"), TIMEOUT).is_err());
        let dir = crate::scratch::Scratch::new();
        let slow = dir.path().join("slow");
        std::fs::write(&slow, "#!/bin/sh\nsleep 5\n").unwrap();
        std::fs::set_permissions(&slow, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(run(&slow, Duration::from_millis(200)).unwrap_err().contains("killed"));
    }
}
