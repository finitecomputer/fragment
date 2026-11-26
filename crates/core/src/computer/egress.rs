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
/// The platform's own, each on HTTP alone: `api`, `model`, `codestorage`
/// (a local fleet's) and `fpac` (a start on a disk) `.fragment.internal`,
/// and the backups' gateway, which each save, restore and disk export
/// registers for itself (the Sandbox SDK's `DirectoryBackup`; cell/entry.mjs
/// `#intercept`).
pub const PLATFORM_COST: usize = 5 * 2;
/// The most hosts a person's Vault sends secrets to, a catalog's host
/// counted like any other: a computer registers each Vault host as its
/// own, so what a catalog changes never moves a Vault over its share.
pub const VAULT_HOSTS_MAX: usize = 8;
/// The most hosts a catalog may catch: what is left once the platform's
/// and a full Vault's are counted.
pub const CATALOG_HOSTS_MAX: usize = (INTERCEPTS_MAX - PLATFORM_COST - VAULT_HOSTS_MAX * HOST_COST) / HOST_COST;

// the three shares fit one container, or this crate doesn't build
const _: () = assert!(PLATFORM_COST + (CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX) * HOST_COST <= INTERCEPTS_MAX);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_has_room_for_21_hosts() {
        assert_eq!(CATALOG_HOSTS_MAX, 21);
    }
}
