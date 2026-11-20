//! A computer's egress intercepts (cell/intercepts.mjs): the hosts its
//! container's outbound requests are caught at, and their budget.
//!
//! A container takes at most `INTERCEPTS_MAX` entries, and a host caught
//! on one protocol costs 2 of them, on HTTP and HTTPS together 4 (spike S3,
//! measured on Cloudflare: commit c4226594b). The platform's own hosts are caught on HTTP
//! alone; each catalog and Vault host on both. So the three shares are
//! bounded together: the platform's, a deployment's catalog's (checked as
//! it deploys, xtask's `catalog_of`; a test node's may be larger, and its
//! Vaults get the less room), and a person's Vault's (checked as a secret
//! is put, `vault::room`), and what a computer arms fits (`armed`).

/// The most intercept entries one container takes (Cloudflare Containers).
pub const INTERCEPTS_MAX: usize = 128;
/// What one host costs, caught on HTTP and on HTTPS.
pub const HOST_COST: usize = 4;
/// The platform's own: `api`, `model` and `codestorage` (a local fleet's)
/// `.fragment.internal`, each on HTTP alone, and 2 spare (the backups'
/// gateway, which each save and restore registers for itself).
pub const PLATFORM_COST: usize = 4 * 2;
/// The most hosts a person's Vault sends secrets to beyond the catalog's
/// own (a catalog host is caught already, and costs nothing more).
pub const VAULT_HOSTS_MAX: usize = 8;
/// The most hosts a catalog may catch: what is left once the platform's
/// and a full Vault's are counted.
pub const CATALOG_HOSTS_MAX: usize = (INTERCEPTS_MAX - PLATFORM_COST - VAULT_HOSTS_MAX * HOST_COST) / HOST_COST;

/// The most hosts one computer catches, on HTTP and HTTPS each.
pub const HOSTS_MAX: usize = (INTERCEPTS_MAX - PLATFORM_COST) / HOST_COST;

const _: () = assert!(CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX <= HOSTS_MAX);

/// Whether `intercepts` catch `host` already: by name, or by a glob
/// (`*.under`, an account host's: `catalog::AccountHost::covers`, a host
/// of one label or more under it).
pub fn caught(intercepts: &[String], host: &str) -> bool {
    intercepts.iter().any(|c| match c.strip_prefix("*.") {
        Some(under) => host.strip_suffix(under).is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
        None => c == host,
    })
}

/// The hosts a computer arms: every one of `kept` (those its container
/// counts already, which an intercept never gives back: none at a fresh
/// start), then `catalog`'s, then `vault`'s (its agents' Vault hosts,
/// sorted), each once and none another catches, as many as `HOSTS_MAX`
/// leaves room for beside `kept`, in that order. The second answer is those
/// left out: a person's Vault hosts from before the cap, or, at a takeover
/// whose container keeps hosts no longer asked (a catalog's or a Vault's
/// changed since its start), what no longer fits beside them. Their
/// placeholders go out uncaught, and fill nothing, while the computer
/// still runs; nothing kept is ever over the budget's room, so the
/// backups' gateway's entries stay free.
pub fn armed(kept: Vec<String>, catalog: Vec<String>, vault: impl IntoIterator<Item = String>) -> (Vec<String>, Vec<String>) {
    let mut vault: Vec<String> = vault.into_iter().collect();
    vault.sort_unstable();
    vault.dedup();
    // a test node's catalog may be over its share: its Vaults get less room
    let room = HOSTS_MAX.saturating_sub(kept.len());
    let mut more: Vec<String> = Vec::new();
    for host in catalog.into_iter().chain(vault) {
        // a Vault host under a catalog glob is caught by it
        if !caught(&kept, &host) && !caught(&more, &host) {
            more.push(host);
        }
    }
    let left_out = more.split_off(room.min(more.len()));
    let kepts = kept.len();
    let mut hosts = kept;
    hosts.extend(more);
    assert!(hosts.len() <= kepts.max(HOSTS_MAX));
    (hosts, left_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i:02}.example.com")).collect()
    }

    #[test]
    fn the_shares_fit_the_containers_entries() {
        assert_eq!((HOSTS_MAX, CATALOG_HOSTS_MAX), (30, 22));
        assert_eq!(PLATFORM_COST + (CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX) * HOST_COST, INTERCEPTS_MAX);
    }

    #[test]
    fn a_vault_within_its_cap_is_armed_whole_and_a_catalog_host_costs_nothing_more() {
        let catalog = hosts("c", CATALOG_HOSTS_MAX);
        let mut vault = hosts("v", VAULT_HOSTS_MAX);
        vault.push(catalog[0].clone());
        vault.push(vault[0].clone());
        let (on, out) = armed(vec![], catalog.clone(), vault);
        assert!(out.is_empty());
        assert_eq!(on.len(), CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX);
        assert!(catalog.iter().all(|h| on.contains(h)));
    }

    #[test]
    fn a_vault_over_the_budget_from_before_the_cap_leaves_some_out_and_never_the_catalogs() {
        let catalog = hosts("c", CATALOG_HOSTS_MAX);
        let (on, out) = armed(vec![], catalog.clone(), hosts("v", 64 * 8));
        assert_eq!(on.len(), CATALOG_HOSTS_MAX + VAULT_HOSTS_MAX);
        assert_eq!(out.len(), 64 * 8 - VAULT_HOSTS_MAX);
        assert!(catalog.iter().all(|h| on.contains(h)));
        // a smaller catalog leaves the Vault more room
        let (on, out) = armed(vec![], hosts("c", 2), hosts("v", 64 * 8));
        assert_eq!(PLATFORM_COST + on.len() * HOST_COST, INTERCEPTS_MAX);
        assert_eq!(on.len() + out.len(), 2 + 64 * 8);
    }

    #[test]
    fn a_host_under_a_catalog_glob_is_caught_by_it() {
        let catalog = vec!["*.my.salesforce.com".to_string(), "api.example.com".to_string()];
        assert!(caught(&catalog, "acme.my.salesforce.com") && caught(&catalog, "a.b.my.salesforce.com") && caught(&catalog, "api.example.com"));
        assert!(!caught(&catalog, "my.salesforce.com") && !caught(&catalog, "evilmy.salesforce.com") && !caught(&catalog, "x.example.com"));
        let (on, out) = armed(vec![], catalog.clone(), ["acme.my.salesforce.com".to_string()]);
        assert_eq!((on, out.len()), (catalog.clone(), 0));
        // kept already, as at a takeover
        let (on, out) = armed(catalog.clone(), catalog.clone(), ["acme.my.salesforce.com".to_string()]);
        assert_eq!((on, out.len()), (catalog, 0));
    }

    #[test]
    fn a_catalog_over_its_share_leaves_a_vault_no_room_and_never_panics() {
        let (on, out) = armed(vec![], hosts("c", HOSTS_MAX + 2), hosts("v", 3));
        assert_eq!((on.len(), out.len()), (HOSTS_MAX, 5));
        // what a container keeps is never taken back, even over the budget
        let (on, out) = armed(hosts("k", HOSTS_MAX + 2), hosts("c", 1), hosts("v", 1));
        assert_eq!((on.len(), out.len()), (HOSTS_MAX + 2, 2));
    }

    #[test]
    fn armed_again_it_keeps_what_it_caught_and_adds_only_what_fits() {
        let (on, _) = armed(vec![], hosts("c", CATALOG_HOSTS_MAX), hosts("v", 4));
        // a host that sorts first never takes the place of one caught already
        let (again, out) = armed(on.clone(), hosts("c", CATALOG_HOSTS_MAX), ["a.example.com".to_string()].into_iter().chain(hosts("w", 8)));
        assert!(on.iter().all(|h| again.contains(h)));
        assert_eq!(again.len(), HOSTS_MAX);
        assert_eq!(out.len(), 9 - (HOSTS_MAX - on.len()));
        assert!(again.contains(&"a.example.com".to_string()));
    }

    #[test]
    fn a_takeover_whose_catalog_changed_keeps_the_budget_the_backups_gateway_needs() {
        // a full container: 22 catalog hosts and 8 Vault hosts kept
        let (kept, _) = armed(vec![], hosts("c", CATALOG_HOSTS_MAX), hosts("v", VAULT_HOSTS_MAX));
        assert_eq!(kept.len(), HOSTS_MAX);
        // the new release's catalog renames a host: it waits for room
        let mut catalog = hosts("c", CATALOG_HOSTS_MAX - 1);
        catalog.push("renamed.example.com".to_string());
        let (on, out) = armed(kept.clone(), catalog, hosts("v", VAULT_HOSTS_MAX));
        assert_eq!((on, out), (kept, vec!["renamed.example.com".to_string()]));
        assert!(PLATFORM_COST + HOSTS_MAX * HOST_COST <= INTERCEPTS_MAX);
    }
}
