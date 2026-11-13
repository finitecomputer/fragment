//! `GET /eventlog`: the CCEL boot event log as the firmware left it, for
//! the owner to replay against the quote's RTMRs (fragment_pod's
//! `attest::replay`).

use crate::http::Response;
use std::io::Read;
use std::path::Path;

/// The CCEL data area is 256 KiB on Google's firmware.
const MAX_EVENT_LOG: u64 = 4 * 1024 * 1024;

pub fn serve(ccel: &Path) -> Response {
    let mut log = Vec::new();
    let read = std::fs::File::open(ccel).and_then(|f| f.take(MAX_EVENT_LOG + 1).read_to_end(&mut log));
    match read {
        Ok(n) if n as u64 > MAX_EVENT_LOG => Response::text(500, "event log over 4 MiB"),
        Ok(_) => Response::bytes(log),
        Err(e) => Response::text(500, format!("reading {}: {e}", ccel.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn serves_the_event_log_and_reports_a_missing_one() {
        let dir = Scratch::new();
        let ccel = dir.path().join("CCEL");
        std::fs::write(&ccel, b"LOG").unwrap();
        assert_eq!(serve(&ccel).body, b"LOG");
        assert_eq!(serve(&dir.path().join("missing")).status, 500);
    }
}
