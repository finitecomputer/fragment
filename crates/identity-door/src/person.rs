//! Who a verified id_token signs in, as the door hands them to the
//! platform: a subject of the door's own and an email.
//!
//! **The subject** (the platform's `user.id`) is derived from the
//! provider's issuer and the token's `sub` together: `user_` and 32 hex
//! digits of a hash naming both. The same `sub` from another issuer (the
//! door moved to another provider) is another subject, never this one's.
//! The platform keys a person by its own issuer (`workos:<client id>`) and
//! this subject.
//!
//! **The email** is the person's handle on the platform, so it is never
//! guessed. It must be a mailbox (a local part, `@`, a domain with a dot:
//! a GitHub login's `user@github`, as tsidp gives it, is none), and either
//! the provider says it verified it (`email_verified: true`), or its domain
//! is one the door's operator vouches for (`vouched_domains`): tsidp sends
//! no `email_verified`, and an org's own directory may not either.

use sha2::{Digest, Sha256};

use crate::oidc::Verified;

/// The registry's own ceiling on an email (cell/src/registry/signin.rs
/// `EMAIL_MAX`): a longer one is refused here, not there.
pub const EMAIL_BYTES_MAX: usize = 320;
/// A mailbox's local part at most (RFC 5321 §4.5.3.1.1).
pub const LOCAL_PART_BYTES_MAX: usize = 64;
/// A domain name at most (RFC 1035).
pub const DOMAIN_BYTES_MAX: usize = 253;
/// The most vouched domains.
pub const VOUCHED_DOMAINS_MAX: usize = 64;

/// Someone the door signs in: the platform's `user.id` and their email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub subject: String,
    pub email: String,
}

/// Why a verified id_token signs no one in: its email is no handle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmailRefused {
    #[error("the sign-in provider gave no email in `{claim}` (is `email` among the scopes, and does it release it?)")]
    Absent { claim: String },
    #[error("the sign-in provider's email {0:?} is no mailbox (a GitHub or passkey login, say): this door signs in mailboxes alone")]
    NotMailbox(String),
    #[error("the sign-in provider has not verified the email {0:?}, and this door vouches for no address in its domain")]
    Unverified(String),
}

/// Why the vouched domains are refused at start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("{0:?} is no mail domain: a vouched domain has a dot (a GitHub login is user@github, which is no mailbox)")]
    NotMailDomain(String),
    #[error("at most {VOUCHED_DOMAINS_MAX} vouched domains")]
    TooMany,
}

/// The email domains whose addresses the door takes as verified, lower case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vouched {
    domains: Vec<String>,
}

impl Vouched {
    pub fn new(domains: &[String]) -> Result<Vouched, DomainError> {
        if domains.len() > VOUCHED_DOMAINS_MAX {
            return Err(DomainError::TooMany);
        }
        let mut kept = Vec::with_capacity(domains.len());
        for d in domains {
            let domain = d.trim().to_ascii_lowercase();
            if mail_domain(&domain) {
                kept.push(domain);
            } else {
                return Err(DomainError::NotMailDomain(d.clone()));
            }
        }
        Ok(Vouched { domains: kept })
    }

    pub fn any(&self) -> bool {
        !self.domains.is_empty()
    }

    /// Whether `mailbox`'s domain is vouched for: exactly, never by a parent.
    fn vouches(&self, mailbox: &str) -> bool {
        let (_, domain) = mailbox.rsplit_once('@').expect("a mailbox has an @");
        self.domains.iter().any(|d| d == domain)
    }
}

/// A domain a mailbox may be in: labels of letters, digits and hyphens,
/// at least two of them.
fn mail_domain(domain: &str) -> bool {
    domain.len() <= DOMAIN_BYTES_MAX
        && domain.contains('.')
        && domain.split('.').all(|l| (1..=63).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !l.starts_with('-') && !l.ends_with('-'))
}

/// `raw` as a mailbox the registry keeps (lower case), when it is one: a
/// local part of printable characters, an `@`, and a mail domain.
pub fn mailbox_of(raw: &str) -> Option<String> {
    let email = raw.trim().to_ascii_lowercase();
    let (local, domain) = email.rsplit_once('@')?;
    let local_ok = !local.is_empty() && local.len() <= LOCAL_PART_BYTES_MAX && local.bytes().all(|b| b.is_ascii_graphic() && b != b'@');
    if local_ok && mail_domain(domain) && email.len() <= EMAIL_BYTES_MAX {
        Some(email)
    } else {
        None
    }
}

/// The platform's `user.id` for the provider `issuer`'s subject `sub`.
pub fn subject_of(issuer: &str, sub: &str) -> String {
    assert!(!issuer.is_empty() && !sub.is_empty(), "a subject is an issuer's and a sub's");
    let mut h = Sha256::new();
    h.update(b"fragment-identity-door/oidc-subject\0");
    h.update(issuer.as_bytes());
    h.update(b"\0");
    h.update(sub.as_bytes());
    format!("user_{}", hex::encode(&h.finalize()[..16]))
}

/// The person `v` (an id_token of `issuer`'s, verified) signs in as, its
/// email read from `claim`.
pub fn person_of(issuer: &str, v: &Verified, claim: &str, vouched: &Vouched) -> Result<Person, EmailRefused> {
    let raw = v.email.as_deref().map(str::trim).filter(|e| !e.is_empty());
    let Some(raw) = raw else {
        return Err(EmailRefused::Absent { claim: claim.into() });
    };
    let Some(email) = mailbox_of(raw) else {
        return Err(EmailRefused::NotMailbox(raw.chars().take(EMAIL_BYTES_MAX).collect()));
    };
    // `email_verified` speaks for the `email` claim alone: an address read
    // from another claim signs in only in a vouched domain
    if !((v.email_verified && claim == "email") || vouched.vouches(&email)) {
        return Err(EmailRefused::Unverified(email));
    }
    let person = Person { subject: subject_of(issuer, &v.subject), email };
    assert!(person.subject.starts_with("user_") && person.subject.len() == 37, "a subject is user_ and 32 hex digits");
    Ok(person)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUER: &str = "https://idp.example.ts.net";

    fn token(email: Option<&str>, verified: bool) -> Verified {
        Verified { subject: "userid:7".into(), email: email.map(str::to_string), email_verified: verified }
    }

    fn vouched() -> Vouched {
        Vouched::new(&["example.com".into()]).unwrap()
    }

    /// Goal: an email the provider verified, or one in a vouched domain,
    /// signs in, lower case, keyed by the issuer and sub together.
    #[test]
    fn a_verified_or_vouched_mailbox_signs_in() {
        let p = person_of(ISSUER, &token(Some(" Grace@Example.com "), false), "email", &vouched()).unwrap();
        assert_eq!(p.email, "grace@example.com", "vouched for (tsidp sends no email_verified)");
        assert_eq!(p.subject, subject_of(ISSUER, "userid:7"));
        assert_eq!(person_of(ISSUER, &token(Some("jo@other.org"), true), "email", &vouched()).unwrap().email, "jo@other.org", "verified by the provider");
        assert_ne!(subject_of(ISSUER, "userid:7"), subject_of("https://dex.corp.example", "userid:7"), "another provider's sub is another subject");
    }

    /// Goal: no email, no mailbox, or an address neither verified nor
    /// vouched for, signs no one in; nothing is guessed.
    #[test]
    fn other_emails_are_refused() {
        let v = vouched();
        assert_eq!(person_of(ISSUER, &token(None, true), "email", &v), Err(EmailRefused::Absent { claim: "email".into() }));
        assert_eq!(person_of(ISSUER, &token(Some("  "), true), "email", &v), Err(EmailRefused::Absent { claim: "email".into() }));
        assert_eq!(person_of(ISSUER, &token(Some("alice@github"), true), "email", &v), Err(EmailRefused::NotMailbox("alice@github".into())), "verified or not, no mailbox");
        assert_eq!(person_of(ISSUER, &token(Some("bob@passkey"), false), "email", &v), Err(EmailRefused::NotMailbox("bob@passkey".into())));
        assert_eq!(person_of(ISSUER, &token(Some("bob@other.org"), false), "email", &v), Err(EmailRefused::Unverified("bob@other.org".into())));
        assert_eq!(person_of(ISSUER, &token(Some("bob@sub.example.com"), false), "email", &v), Err(EmailRefused::Unverified("bob@sub.example.com".into())), "a parent domain vouches for no subdomain");
        assert_eq!(person_of(ISSUER, &token(Some(&format!("{}@example.com", "a".repeat(LOCAL_PART_BYTES_MAX + 1))), true), "email", &v).map(|p| p.email).ok(), None);
    }

    /// Goal: `email_verified` speaks for the `email` claim alone. An address
    /// read from another claim signs in only in a vouched domain, whatever
    /// the token says it verified.
    #[test]
    fn email_verified_speaks_for_email_alone() {
        let v = vouched();
        assert_eq!(person_of(ISSUER, &token(Some("jo@other.org"), true), "preferred_username", &v), Err(EmailRefused::Unverified("jo@other.org".into())));
        assert_eq!(person_of(ISSUER, &token(Some("grace@example.com"), true), "preferred_username", &v).unwrap().email, "grace@example.com", "vouched for");
    }

    /// Goal: a vouched domain that is no mail domain is refused at start.
    #[test]
    fn vouched_domains_are_mail_domains() {
        assert_eq!(Vouched::new(&["github".into()]), Err(DomainError::NotMailDomain("github".into())));
        assert_eq!(Vouched::new(&["@example.com".into()]), Err(DomainError::NotMailDomain("@example.com".into())));
        assert_eq!(Vouched::new(&vec!["example.com".to_string(); VOUCHED_DOMAINS_MAX + 1]), Err(DomainError::TooMany));
        assert!(Vouched::new(&[" Example.COM ".into()]).unwrap().vouches("a@example.com"));
    }
}
