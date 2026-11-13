//! The TD report body inside a DCAP quote (v4, or v5 with a TD 1.0 or
//! 1.5 body), read by offset. Layout: Intel TDX DCAP Quoting Library API,
//! appendix A (quote v4 and v5), and the TDX module ABI's TDREPORT.
//! This reads; it does not verify.

use serde::Serialize;
use std::fmt;

pub const HEADER_LEN: usize = 48;
pub const TD10_BODY_LEN: usize = 584;
pub const TEE_TYPE_TDX: u32 = 0x81;

/// Offsets inside the TD 1.0 body (the first 584 bytes of a 1.5 body).
pub mod offset {
    pub const TEE_TCB_SVN: usize = 0;
    pub const MR_SEAM: usize = 16;
    pub const TD_ATTRIBUTES: usize = 120;
    pub const XFAM: usize = 128;
    pub const MR_TD: usize = 136;
    pub const MR_CONFIG_ID: usize = 184;
    pub const MR_OWNER: usize = 232;
    pub const MR_OWNER_CONFIG: usize = 280;
    pub const RTMR0: usize = 328;
    pub const REPORT_DATA: usize = 520;
}

/// TD attribute bits (TDX module ABI, TDATTRIBUTES). Bits 0..7 are the
/// "TD under debug" group; any of them set means the host can inspect
/// or profile the TD.
pub mod attr {
    pub const DEBUG: u64 = 1 << 0;
    pub const TUD_MASK: u64 = 0xff;
    pub const SEPT_VE_DISABLE: u64 = 1 << 28;
    pub const PKS: u64 = 1 << 30;
    pub const KL: u64 = 1 << 31;
    pub const PERFMON: u64 = 1 << 63;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    TooShort { len: usize, need: usize },
    UnsupportedVersion(u16),
    NotTdx(u32),
    UnsupportedBodyType(u16),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len, need } => write!(f, "quote is {len} bytes; need {need}"),
            Self::UnsupportedVersion(v) => write!(f, "quote version {v}; expected 4 or 5"),
            Self::NotTdx(t) => write!(f, "TEE type {t:#x}; expected TDX (0x81)"),
            Self::UnsupportedBodyType(t) => write!(f, "v5 body type {t}; expected 2 or 3"),
        }
    }
}

impl std::error::Error for ReportError {}

pub type Mr = [u8; 48];

/// The fields of a TD report a verifier looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdReport {
    pub quote_version: u16,
    pub tee_tcb_svn: [u8; 16],
    pub mr_seam: Mr,
    /// As in the quote: 8 bytes, little-endian.
    pub td_attributes: [u8; 8],
    pub xfam: [u8; 8],
    pub mr_td: Mr,
    pub mr_config_id: Mr,
    pub mr_owner: Mr,
    pub mr_owner_config: Mr,
    pub rtmr: [Mr; 4],
    pub report_data: [u8; 64],
}

impl TdReport {
    pub fn td_attributes_u64(&self) -> u64 {
        u64::from_le_bytes(self.td_attributes)
    }

    pub fn xfam_u64(&self) -> u64 {
        u64::from_le_bytes(self.xfam)
    }

    /// The named registers a diff compares, in a fixed order.
    pub fn registers(&self) -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("mrtd", self.mr_td.to_vec()),
            ("rtmr0", self.rtmr[0].to_vec()),
            ("rtmr1", self.rtmr[1].to_vec()),
            ("rtmr2", self.rtmr[2].to_vec()),
            ("rtmr3", self.rtmr[3].to_vec()),
            ("td_attributes", self.td_attributes.to_vec()),
            ("xfam", self.xfam.to_vec()),
            ("mrconfigid", self.mr_config_id.to_vec()),
            ("mrowner", self.mr_owner.to_vec()),
            ("mrownerconfig", self.mr_owner_config.to_vec()),
            ("mrseam", self.mr_seam.to_vec()),
            ("tee_tcb_svn", self.tee_tcb_svn.to_vec()),
            ("report_data", self.report_data.to_vec()),
        ]
    }
}

/// Where the TD body starts in a quote, after checking the header.
pub fn body_offset(quote: &[u8]) -> Result<usize, ReportError> {
    need(quote, HEADER_LEN)?;
    let version = u16::from_le_bytes([quote[0], quote[1]]);
    let tee_type = u32::from_le_bytes([quote[4], quote[5], quote[6], quote[7]]);
    if tee_type != TEE_TYPE_TDX {
        return Err(ReportError::NotTdx(tee_type));
    }
    match version {
        4 => Ok(HEADER_LEN),
        5 => {
            need(quote, HEADER_LEN + 6)?;
            let body_type = u16::from_le_bytes([quote[48], quote[49]]);
            if body_type != 2 && body_type != 3 {
                return Err(ReportError::UnsupportedBodyType(body_type));
            }
            Ok(HEADER_LEN + 6)
        }
        v => Err(ReportError::UnsupportedVersion(v)),
    }
}

pub fn parse(quote: &[u8]) -> Result<TdReport, ReportError> {
    let start = body_offset(quote)?;
    need(quote, start + TD10_BODY_LEN)?;
    let body = &quote[start..start + TD10_BODY_LEN];
    let mr = |at: usize| -> Mr { body[at..at + 48].try_into().expect("48 bytes") };
    let rtmr = [0, 1, 2, 3].map(|i| mr(offset::RTMR0 + 48 * i));
    Ok(TdReport {
        quote_version: u16::from_le_bytes([quote[0], quote[1]]),
        tee_tcb_svn: body[offset::TEE_TCB_SVN..16].try_into().expect("16 bytes"),
        mr_seam: mr(offset::MR_SEAM),
        td_attributes: body[offset::TD_ATTRIBUTES..offset::TD_ATTRIBUTES + 8]
            .try_into()
            .expect("8 bytes"),
        xfam: body[offset::XFAM..offset::XFAM + 8].try_into().expect("8 bytes"),
        mr_td: mr(offset::MR_TD),
        mr_config_id: mr(offset::MR_CONFIG_ID),
        mr_owner: mr(offset::MR_OWNER),
        mr_owner_config: mr(offset::MR_OWNER_CONFIG),
        rtmr,
        report_data: body[offset::REPORT_DATA..offset::REPORT_DATA + 64]
            .try_into()
            .expect("64 bytes"),
    })
}

fn need(quote: &[u8], need: usize) -> Result<(), ReportError> {
    if quote.len() < need {
        return Err(ReportError::TooShort { len: quote.len(), need });
    }
    Ok(())
}

/// The registers as hex, for printing and for JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RegistersHex {
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
    /// The quote's 8 bytes in quote order (little-endian), as Intel's
    /// tools print them: SEPT_VE_DISABLE alone is `0000001000000000`.
    pub td_attributes: String,
    pub xfam: String,
    pub mrconfigid: String,
    pub mrowner: String,
    pub mrownerconfig: String,
    pub mrseam: String,
    pub tee_tcb_svn: String,
    pub report_data: String,
}

impl From<&TdReport> for RegistersHex {
    fn from(r: &TdReport) -> Self {
        Self {
            mrtd: hex::encode(r.mr_td),
            rtmr0: hex::encode(r.rtmr[0]),
            rtmr1: hex::encode(r.rtmr[1]),
            rtmr2: hex::encode(r.rtmr[2]),
            rtmr3: hex::encode(r.rtmr[3]),
            td_attributes: hex::encode(r.td_attributes),
            xfam: hex::encode(r.xfam),
            mrconfigid: hex::encode(r.mr_config_id),
            mrowner: hex::encode(r.mr_owner),
            mrownerconfig: hex::encode(r.mr_owner_config),
            mrseam: hex::encode(r.mr_seam),
            tee_tcb_svn: hex::encode(r.tee_tcb_svn),
            report_data: hex::encode(r.report_data),
        }
    }
}

impl RegistersHex {
    pub fn named(&self) -> Vec<(&'static str, &str)> {
        vec![
            ("mrtd", &self.mrtd),
            ("rtmr0", &self.rtmr0),
            ("rtmr1", &self.rtmr1),
            ("rtmr2", &self.rtmr2),
            ("rtmr3", &self.rtmr3),
            ("td_attributes", &self.td_attributes),
            ("xfam", &self.xfam),
            ("mrconfigid", &self.mrconfigid),
            ("mrowner", &self.mrowner),
            ("mrownerconfig", &self.mrownerconfig),
            ("mrseam", &self.mrseam),
            ("tee_tcb_svn", &self.tee_tcb_svn),
            ("report_data", &self.report_data),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_v4() -> Vec<u8> {
        let mut q = vec![0u8; HEADER_LEN + TD10_BODY_LEN];
        q[0] = 4;
        q[4] = 0x81;
        q[HEADER_LEN + offset::MR_TD] = 0xf2;
        q[HEADER_LEN + offset::RTMR0 + 48 * 3] = 0x33;
        q[HEADER_LEN + offset::TD_ATTRIBUTES + 3] = 0x10;
        q
    }

    #[test]
    fn reads_fields_at_their_offsets() {
        let r = parse(&fake_v4()).unwrap();
        assert_eq!(r.mr_td[0], 0xf2);
        assert_eq!(r.rtmr[3][0], 0x33);
        assert_eq!(r.td_attributes_u64(), attr::SEPT_VE_DISABLE);
        assert_eq!(RegistersHex::from(&r).td_attributes, "0000001000000000");
    }

    #[test]
    fn refuses_short_sgx_and_unknown_versions() {
        let q = fake_v4();
        assert!(matches!(parse(&q[..100]), Err(ReportError::TooShort { .. })));
        let mut sgx = q.clone();
        sgx[4] = 0;
        assert_eq!(parse(&sgx), Err(ReportError::NotTdx(0)));
        let mut v3 = q;
        v3[0] = 3;
        assert_eq!(parse(&v3), Err(ReportError::UnsupportedVersion(3)));
    }
}
