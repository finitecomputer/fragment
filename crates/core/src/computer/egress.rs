//! A computer's egress intercepts (cell/intercepts.mjs): their budget.
//!
//! A container takes at most `INTERCEPTS_MAX` entries, and a host caught
//! on one protocol costs 2 of them, on HTTP and HTTPS together 4 (spike S3,
//! measured on Cloudflare: commit c4226594b). The platform's own hosts are
//! caught on HTTP alone; each catalog and Vault host on both. So the three
//! shares are bounded together, each where it is set: the platform's
//! (fixed), a deployment's catalog's (as it deploys, xtask's `catalog_of`;
//! a test node's may be larger) and a person's Vault's (as a secret is
//! put, `vault::room`). A start registers them all, so one within them
//! always fits.

/// The most intercept entries one container takes (Cloudflare Containers).
pub const INTERCEPTS_MAX: usize = 128;
/// What one host costs, caught on HTTP and on HTTPS.
pub const HOST_COST: usize = 4;
/// The platform's own, each on HTTP alone: `api`, `model` and
/// `codestorage` (a local fleet's) `.fragment.internal`, and the backups'
/// gateway, which each save and restore registers for itself (the Sandbox
/// SDK's `DirectoryBackup`; cell/entry.mjs `#intercept`).
pub const PLATFORM_COST: usize = 4 * 2;
/// The most hosts a person's Vault sends secrets to beyond the catalog's
/// own (a catalog host is caught already, and costs nothing more).
pub const VAULT_HOSTS_MAX: usize = 8;
/// The most hosts a catalog may catch: what is left once the platform's
/// and a full Vault's are counted.
pub const CATALOG_HOSTS_MAX: usize = (INTERCEPTS_MAX - PLATFORM_COST - VAULT_HOSTS_MAX * HOST_COST) / HOST_COST;

/// Whether `intercepts` catch `host` already: by name, or by a glob
/// (`*.under`, an account host's: `catalog::AccountHost::covers`, a host
/// of one label or more under it).
pub fn caught(intercepts: &[String], host: &str) -> bool {
    intercepts.iter().any(|c| match c.strip_prefix("*.") {
        Some(under) => host.strip_suffix(under).is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
        None => c == host,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shares_fit_the_containers_entries() {
        assert_eq!(CATALOG_HOSTS_MAX, 22);
        assert_eq!(PLATFORM_COST + (CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX) * HOST_COST, INTERCEPTS_MAX);
    }

    #[test]
    fn a_host_under_a_catalog_glob_is_caught_by_it() {
        let catalog = vec!["*.my.salesforce.com".to_string(), "api.example.com".to_string()];
        assert!(caught(&catalog, "acme.my.salesforce.com") && caught(&catalog, "a.b.my.salesforce.com") && caught(&catalog, "api.example.com"));
        assert!(!caught(&catalog, "my.salesforce.com") && !caught(&catalog, "evilmy.salesforce.com") && !caught(&catalog, "x.example.com"));
    }
}
