//! Which release a verified quote is: a release matches when every
//! register it names equals the quote's.

use crate::attest::report::TdReport;
use crate::unlock::release::{Release, REGISTERS};

fn register(report: &TdReport, index: usize) -> &[u8; 48] {
    match index {
        0 => &report.mr_td,
        i => &report.rtmr[i - 1],
    }
}

/// A release that names no register matches nothing: Release::from_wire
/// refuses one, and this holds for one built some other way too.
pub fn matches(release: &Release, report: &TdReport) -> bool {
    let named = release.registers.iter().any(Option::is_some);
    let equal = release.registers.iter().enumerate().all(|(i, r)| r.as_ref().is_none_or(|v| v == register(report, i)));
    named && equal
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub seq: u64,
    pub name: String,
    pub not_compared: Vec<&'static str>,
}

impl Matched {
    pub fn of(release: &Release) -> Self {
        let not_compared = REGISTERS.into_iter().zip(&release.registers).filter(|(_, r)| r.is_none()).map(|(n, _)| n).collect();
        Self { seq: release.seq, name: release.name.clone(), not_compared }
    }

    pub fn detail(&self) -> String {
        let compared = if self.not_compared.is_empty() {
            "all five registers compared".to_owned()
        } else {
            format!("not compared: {}", self.not_compared.join(", "))
        };
        format!("{} (seq {}); {compared}", self.name, self.seq)
    }
}

/// The newest release whose named registers all equal the quote's, if
/// it is not older than the last one this owner unlocked.
pub fn match_release(releases: &[Release], report: &TdReport, last_unlocked_seq: Option<u64>) -> Result<Matched, String> {
    let best = releases
        .iter()
        .filter(|r| matches(r, report))
        .max_by_key(|r| r.seq)
        .ok_or_else(|| format!("no release in policy.json has these registers ({} listed)", releases.len()))?;
    if let Some(last) = last_unlocked_seq.filter(|last| best.seq < *last) {
        return Err(format!("release {} (seq {}) is older than the last unlocked (seq {last})", best.name, best.seq));
    }
    Ok(Matched::of(best))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn report() -> TdReport {
        let mut rtmr = [[0u8; 48]; 4];
        for (i, r) in rtmr.iter_mut().enumerate() {
            *r = [i as u8 + 1; 48];
        }
        TdReport {
            quote_version: 4,
            tee_tcb_svn: [0; 16],
            mr_seam: [0; 48],
            td_attributes: [0; 8],
            xfam: [0; 8],
            mr_td: [0xaa; 48],
            mr_config_id: [0; 48],
            mr_owner: [0; 48],
            mr_owner_config: [0; 48],
            rtmr,
            report_data: [0; 64],
        }
    }

    pub fn release(seq: u64, name: &str, regs: &[(&'static str, u8)]) -> Release {
        let mut registers = [None; 5];
        for (n, b) in regs {
            let i = REGISTERS.iter().position(|r| r == n).unwrap();
            registers[i] = Some([*b; 48]);
        }
        Release { seq, name: name.into(), registers }
    }

    #[test]
    fn matches_on_named_registers_and_says_which_were_not_compared() {
        let rs = [release(1, "a", &[("mrtd", 0xaa), ("rtmr1", 2), ("rtmr2", 3)])];
        let m = match_release(&rs, &report(), None).unwrap();
        assert_eq!((m.seq, m.not_compared.as_slice()), (1, ["rtmr0", "rtmr3"].as_slice()));
        assert_eq!(m.detail(), "a (seq 1); not compared: rtmr0, rtmr3");
        let all = [release(2, "b", &[("mrtd", 0xaa), ("rtmr0", 1), ("rtmr1", 2), ("rtmr2", 3), ("rtmr3", 4)])];
        assert_eq!(match_release(&all, &report(), None).unwrap().detail(), "b (seq 2); all five registers compared");
    }

    #[test]
    fn one_differing_register_is_no_match_and_naming_none_matches_nothing() {
        let rs = [release(1, "a", &[("mrtd", 0xaa), ("rtmr2", 9)])];
        let err = match_release(&rs, &report(), None).unwrap_err();
        assert!(err.contains("no release"), "{err}");
        assert!(match_release(&[], &report(), None).is_err());
        assert!(!matches(&release(1, "empty", &[]), &report()));
    }

    #[test]
    fn refuses_a_release_older_than_the_last_unlocked() {
        let rs = [release(1, "old", &[("rtmr1", 2)]), release(3, "other", &[("rtmr1", 9)])];
        assert_eq!(match_release(&rs, &report(), Some(1)).unwrap().seq, 1);
        let err = match_release(&rs, &report(), Some(2)).unwrap_err();
        assert!(err.contains("older"), "{err}");
    }

    #[test]
    fn the_newest_of_several_matches_wins() {
        let rs = [release(1, "a", &[("rtmr1", 2)]), release(4, "b", &[("rtmr2", 3)])];
        assert_eq!(match_release(&rs, &report(), Some(4)).unwrap().name, "b");
    }
}
