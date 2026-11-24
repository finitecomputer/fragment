//! What a computer's own origin (`<24 hex>--computer.<suffix>`) keeps out
//! of its ports' pages: a service worker. The origin is its owner's for as
//! long as they have the computer (`default_computer_of`), and a worker
//! registered there stays in the browser until something unregisters it,
//! across the computer's restarts, restores and agents, and would see
//! every later navigation on its scope (`/__ticket` among them, with the
//! owner's ticket). The platform serves none there, so the origin has
//! none: a port that wants one gets a refusal.

/// Whether a request to a computer's origin is refused before it reaches
/// `/__ticket` or a port: a service worker's script, which a browser asks
/// for (to register or update it) with `Service-Worker: script`, a header
/// no page's code may set (a forbidden request header). Its value is not
/// read: the header's presence alone says a worker's script.
pub fn refused(service_worker: Option<&str>) -> bool {
    service_worker.is_some()
}

/// The headers of a port's answer dropped before a browser sees it:
/// `Service-Worker-Allowed` would let a worker's scope reach past its
/// script's path, to `/__ticket` and the other ports. No worker registers
/// here (`refused`); this keeps that true of a script reached some other
/// way.
pub const DROPPED_ANSWER_HEADERS: [&str; 1] = ["service-worker-allowed"];

/// Whether `name` (a header of a port's answer, in any case) is dropped.
pub fn dropped_answer_header(name: &str) -> bool {
    DROPPED_ANSWER_HEADERS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_worker_script_is_refused_on_a_computer_origin() {
        assert!(refused(Some("script")));
        // whatever a browser sends as its value: the header alone says a worker's script
        assert!(refused(Some("")));
        assert!(refused(Some("Script")));
    }

    #[test]
    fn a_request_without_the_header_goes_on() {
        assert!(!refused(None));
    }

    #[test]
    fn a_port_answer_widens_no_worker_scope() {
        assert!(dropped_answer_header("service-worker-allowed"));
        assert!(dropped_answer_header("Service-Worker-Allowed"));
        for kept in ["content-type", "content-security-policy", "set-cookie", "cache-control", "service-worker"] {
            assert!(!dropped_answer_header(kept), "{kept} is kept");
        }
    }

    #[test]
    fn dropped_headers_are_lower_case_names() {
        for d in DROPPED_ANSWER_HEADERS {
            assert!(!d.is_empty() && d.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'), "{d}");
        }
    }
}
