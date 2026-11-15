//! The release check as the key service holds it: one of the owner's
//! approvals has the quote's registers, is inside its window
//! (`not_before <= now < expires`), and is not below `min_seq`.

use crate::error::Refusal;
use fragment_pod::attest::report::TdReport;
use fragment_pod::checks::release::{matches, Matched};
use fragment_pod::keyd::approve::Approval;

fn window(a: &Approval, now: i64) -> Result<(), Refusal> {
    let (name, seq) = (a.release.name.clone(), a.release.seq);
    // A clock before 1970 is before every window.
    let at = u64::try_from(now).unwrap_or(0);
    if let Some(not_before) = a.not_before.filter(|t| at < *t) {
        return Err(Refusal::NotYetValid { name, seq, not_before, now });
    }
    if let Some(expires) = a.expires.filter(|t| at >= *t) {
        return Err(Refusal::Expired { name, seq, expires, now });
    }
    Ok(())
}

/// The newest approval that matches and is open now. If some match but
/// none is open, the refusal is the newest match's window.
pub fn choose(approvals: &[Approval], report: &TdReport, min_seq: u64, now: i64) -> Result<Matched, Refusal> {
    let matching: Vec<&Approval> = approvals.iter().filter(|a| matches(&a.release, report)).collect();
    let newest = matching
        .iter()
        .max_by_key(|a| a.release.seq)
        .ok_or_else(|| Refusal::Release(format!("no approved release has these registers ({} approved)", approvals.len())))?;
    let best = matching.iter().filter(|a| window(a, now).is_ok()).max_by_key(|a| a.release.seq);
    let Some(best) = best else {
        return Err(window(newest, now).err().unwrap_or_else(|| Refusal::Internal("no window refused".into())));
    };
    if best.release.seq < min_seq {
        return Err(Refusal::Rollback { name: best.release.name.clone(), seq: best.release.seq, min_seq });
    }
    Ok(Matched::of(&best.release))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_pod::unlock::release::Release;

    fn report(rtmr1: u8) -> TdReport {
        TdReport {
            quote_version: 4,
            tee_tcb_svn: [0; 16],
            mr_seam: [0; 48],
            td_attributes: [0; 8],
            xfam: [0; 8],
            mr_td: [0; 48],
            mr_config_id: [0; 48],
            mr_owner: [0; 48],
            mr_owner_config: [0; 48],
            rtmr: [[0; 48], [rtmr1; 48], [0; 48], [0; 48]],
            report_data: [0; 64],
        }
    }

    fn approval(seq: u64, rtmr1: u8, not_before: Option<u64>, expires: Option<u64>) -> Approval {
        let mut registers = [None; 5];
        registers[2] = Some([rtmr1; 48]);
        Approval { release: Release { seq, name: format!("r{seq}"), registers }, not_before, expires }
    }

    #[test]
    fn the_newest_open_match_wins() {
        let a = [approval(1, 7, None, None), approval(2, 7, Some(50), None), approval(3, 8, None, None)];
        assert_eq!(choose(&a, &report(7), 0, 100).unwrap().seq, 2);
        assert_eq!(choose(&a, &report(7), 0, 10).unwrap().seq, 1, "seq 2 is not open yet");
        assert_eq!(choose(&a, &report(8), 0, 10).unwrap().seq, 3);
    }

    #[test]
    fn each_failure_is_named() {
        let at = |a: Approval, min: u64, now: i64| choose(&[a], &report(7), min, now).unwrap_err().code();
        assert_eq!(choose(&[approval(1, 8, None, None)], &report(7), 0, 0).unwrap_err().code(), "release");
        assert_eq!(at(approval(1, 7, Some(100), None), 0, 99), "not_yet_valid");
        assert_eq!(at(approval(1, 7, None, Some(100)), 0, 100), "expired");
        assert_eq!(at(approval(1, 7, None, None), 2, 0), "rollback");
        assert_eq!(at(approval(1, 7, Some(100), None), 0, -5), "not_yet_valid", "a clock before 1970");
    }

    #[test]
    fn windows_include_not_before_and_exclude_expires() {
        let a = [approval(1, 7, Some(100), Some(200))];
        assert!(choose(&a, &report(7), 0, 100).is_ok());
        assert!(choose(&a, &report(7), 0, 199).is_ok());
        assert!(choose(&a, &report(7), 0, 200).is_err());
        assert!(choose(&a, &report(7), 1, 150).is_ok(), "seq equal to min_seq is allowed");
    }
}
