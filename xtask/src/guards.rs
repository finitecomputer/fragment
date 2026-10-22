//! Guards over the cell's source that its types alone cannot state.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    /// Every `.rs` file under `cell/src`, by its path there, with its text.
    fn cell_sources() -> BTreeMap<String, String> {
        let root = crate::devstack::repo_root().join("cell/src");
        let mut files = BTreeMap::new();
        let mut dirs = vec![root.clone()];
        // bounded: the cell's own tree
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("cell/src reads") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path.strip_prefix(&root).expect("under cell/src").to_string_lossy().into_owned();
                    files.insert(rel, std::fs::read_to_string(&path).expect("a source file"));
                }
            }
        }
        assert!(files.len() > 20 && files.contains_key("auth.rs"), "the cell's sources were found");
        files
    }

    /// Where `needle` occurs in the cell, by file, ignoring comments.
    fn uses(files: &BTreeMap<String, String>, needle: &str) -> BTreeMap<String, usize> {
        let mut found = BTreeMap::new();
        for (path, text) in files {
            let n = text.lines().filter(|l| !l.trim_start().starts_with("//")).filter(|l| l.contains(needle)).count();
            if n > 0 {
                found.insert(path.clone(), n);
            }
        }
        found
    }

    fn only(file: &str, n: usize) -> BTreeMap<String, usize> {
        BTreeMap::from([(file.to_string(), n)])
    }

    /// Goal: a browser round trip finishes only in the browser that began
    /// it (docs/api.md, Sign-in; audit H3), and the cell's types say so:
    /// `auth::Bound` has private fields, so no other module can make one,
    /// and what finishes a round trip takes one. This holds what the types
    /// cannot: that `auth.rs` makes a `Bound` in `trip_bound` alone, from
    /// the request's own cookie; that its fields stay private; that no
    /// other file reads a round trip's cookie or compares states; and that
    /// the finished round trips (`Exchange`, `OidcExchange`, `computer::SignedIn`) are made
    /// only from a `Bound` and are never `Deserialize`. Method: the cell's
    /// source, read.
    #[test]
    fn a_round_trip_is_bound_only_by_its_request() {
        let files = cell_sources();
        let auth = &files["auth.rs"];
        assert!(
            auth.contains("pub(crate) struct Bound {\n    trip: Trip,\n    state: String,\n}"),
            "Bound's fields are private to auth.rs, and it derives nothing (no Clone, no Deserialize)"
        );
        assert!(!auth.contains("#[derive(Clone)]\npub(crate) struct Bound") && !auth.contains("Deserialize)]\npub(crate) struct Bound"), "Bound derives nothing");
        assert_eq!(uses(&files, "Bound { trip"), only("auth.rs", 1), "a Bound is made once, in trip_bound");
        let made_at = auth.find("Bound { trip").expect("made");
        let trip_bound = auth.find("pub(crate) fn trip_bound(req: &Request").expect("trip_bound reads the request");
        let next_fn = auth[trip_bound + 1..].find("\nfn ").or_else(|| auth[trip_bound + 1..].find("\npub")).map(|i| i + trip_bound + 1).unwrap_or(auth.len());
        assert!((trip_bound..next_fn).contains(&made_at), "inside trip_bound");
        assert_eq!(uses(&files, "same_state("), only("auth.rs", 1), "states are compared in trip_bound alone");
        assert_eq!(uses(&files, ".cookie()"), only("auth.rs", 3), "a round trip's cookie is set, spent and read in auth.rs alone");
        for cookie in ["\"fragment_login\"", "\"fragment_switch\"", "\"fragment_own_key\"", "\"fragment_connection\""] {
            assert!(uses(&files, cookie).is_empty(), "{cookie} is named by fragment_core::roundtrip alone");
        }
        // the finished round trips: made from a Bound, never decoded
        let calls = &files["registry/calls.rs"];
        assert!(calls.contains("#[derive(Serialize)]\n#[serde(rename_all = \"camelCase\")]\npub(crate) struct Exchange {\n    state: String,"), "Exchange is Serialize only, its fields private");
        // and an OpenID Connect provider's (docs/self-host.md, seam 4), alike
        assert!(calls.contains("#[derive(Serialize)]\npub(crate) struct OidcExchange {\n    state: String,"), "OidcExchange is Serialize only, its fields private");
        assert_eq!(uses(&files, "Exchange { state"), only("registry/calls.rs", 2), "an Exchange and an OidcExchange are each made in their `of` alone");
        assert_eq!(uses(&files, "OidcExchange::of("), only("oidc.rs", 1), "an OidcExchange is made from the callback's Bound");
        assert!(calls.contains("pub(crate) fn of(bound: crate::auth::Bound,"), "from a Bound");
        assert!(calls.contains("#[derive(Serialize)]\npub(crate) struct FinishConnection {\n    state: String,"), "connection completion is Serialize only with private fields");
        assert_eq!(uses(&files, "FinishConnection::of("), only("connections.rs", 1));
        assert_eq!(uses(&files, "keys::connection_exchange("), only("registry/connection_signin.rs", 1));
        assert_eq!(uses(&files, "keys::pipes_import("), only("registry/connection_signin.rs", 1));
        assert!(uses(&files, "pipes_authorize(").is_empty(), "no legacy authorization bypass");
        let computer = &files["computer.rs"];
        assert!(computer.contains("#[derive(serde::Serialize)]\npub(crate) struct SignedIn {"), "SignedIn is Serialize only");
        assert_eq!(uses(&files, "SignedIn { computer"), only("computer.rs", 1), "a SignedIn is made in SignedIn::of alone");
        assert!(computer.contains("pub(crate) fn of(bound: crate::auth::Bound,"), "from a Bound");
        assert_eq!(uses(&files, "\"computer/signed-in\""), only("computer.rs", 2), "the computer's signed-in is asked through signed_in alone (and answered by the computer)");
        assert!(Path::new(&crate::devstack::repo_root().join("crates/core/src/roundtrip.rs")).exists());
    }

    /// Goal: a platform object's schema changes only through its class's
    /// steps (crates/core/migrations, run by fragment_core::migrate), which
    /// the schema lock holds: no statement of the cell's makes, alters or
    /// drops a table, an index or a trigger, but the test levers that plant
    /// the shapes the runner must tell apart (schema.rs, its
    /// `schema-unversion` dropping what later steps made, and a list's
    /// `before-searched`). A `CREATE TABLE IF NOT EXISTS` in a module, as
    /// every object's schema was until 2026-10-19, never reaches an object
    /// made before it. Method: the cell's source, read.
    #[test]
    fn the_cell_changes_no_schema_outside_its_steps() {
        let files = cell_sources();
        let mut found: BTreeMap<String, usize> = BTreeMap::new();
        for needle in ["CREATE TABLE", "CREATE INDEX", "CREATE UNIQUE INDEX", "CREATE VIRTUAL TABLE", "CREATE TRIGGER", "ALTER TABLE", "DROP TABLE", "DROP INDEX", "DROP TRIGGER"] {
            for (file, n) in uses(&files, needle) {
                *found.entry(file).or_default() += n;
            }
        }
        let levers = BTreeMap::from([("principal.rs".to_string(), 2), ("schema.rs".to_string(), 7)]);
        assert_eq!(found, levers, "a schema changes by a step of its class's (crates/core/src/migrate.rs says how), not in the cell");
    }

    /// Goal: a change to a call's shape between the Worker and a platform
    /// object, or between two objects, ships safely across a deploy
    /// (fragment_core::release; docs/MODEL.md, "Calls between releases"):
    /// every call into an object goes out through routed.rs, which stamps
    /// it with its caller's release, and every object's `fetch` checks
    /// that first, so another release's call is refused before it acts
    /// (2026-10-20: an old Registry exchanged a sign-in's WorkOS code for a
    /// new Worker that could not read its answer). The JavaScript calls
    /// the supervisor only with requests `InternalRoute.request` makes from
    /// its env, which stamps them too. Method: the cell's source, read.
    #[test]
    fn every_call_into_an_object_carries_its_release() {
        let files = cell_sources();
        assert_eq!(uses(&files, "durable_object("), only("routed.rs", 1), "an object is reached through routed::send alone");
        assert_eq!(uses(&files, "fetch_with_request("), only("routed.rs", 1), "and called there alone, stamped");
        let objects = [("fragment.rs", "FragmentCell"), ("registry.rs", "RegistryCell"), ("principal.rs", "PrincipalCell"), ("ledger.rs", "LedgerCell"), ("computer.rs", "ComputerCell")];
        for (file, class) in objects {
            let text = &files[file];
            let at = text.find(&format!("impl DurableObject for {class} {{")).unwrap_or_else(|| panic!("{class} is a Durable Object"));
            let fetch = text[at..].find("    async fn fetch(&self, req: Request) -> Result<Response> {\n").unwrap_or_else(|| panic!("{class} answers fetch")) + at;
            let first = text[fetch..].lines().nth(1).unwrap_or_default();
            assert!(first.trim_start().starts_with("if let Some(refused) = crate::routed::refused(&req, "), "{class}'s fetch checks its caller's release before anything else, not: {first}");
        }
        let checked: BTreeMap<String, usize> = objects.iter().map(|(file, _)| (file.to_string(), 1)).collect();
        assert_eq!(uses(&files, "routed::refused(&req, "), checked, "each object checks once, in its fetch");
        let entry = std::fs::read_to_string(crate::devstack::repo_root().join("cell/entry.mjs")).expect("entry.mjs reads");
        let code: Vec<&str> = entry.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
        // and `Node`, a sandcastle node's (docs/self-host.md, seam 2): it runs
        // no Rust, and its calls are the node's protocol, no shape of the cell's
        assert_eq!(code.iter().filter(|l| l.contains(" extends DurableObject {")).count(), objects.len() + 1, "the platform's objects are the five checked above, and a node's");
        assert_eq!(code.iter().filter(|l| l.contains("export class Node extends DurableObject {")).count(), 1, "the one object beyond them is a node's");
        let made: Vec<&&str> = code.iter().filter(|l| l.contains("InternalRoute.request(")).collect();
        assert!(made.len() >= 3 && made.iter().all(|l| l.contains("InternalRoute.request(this.env, ") || l.contains("InternalRoute.request(env, ")), "the JavaScript's internal calls are stamped from its env: {made:?}");
        let called: Vec<&&str> = code.iter().filter(|l| l.contains("getByName(")).collect();
        assert!(!called.is_empty() && called.iter().all(|l| l.contains(".fetch(rs.InternalRoute.request(this.env, ")), "the JavaScript calls an object with a stamped request alone: {called:?}");
    }

    /// The value of a constant whose expression is integer literals
    /// multiplied (`15 * 60_000`, `60 * 60 * 1000`): `name` declared once in
    /// `text`, as `<head> <name><sep>` up to its `;`.
    fn constant(text: &str, head: &str, name: &str, sep: &str) -> i64 {
        let at = format!("{head} {name}{sep}");
        let lines: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with(&at)).collect();
        assert_eq!(lines.len(), 1, "{name} is declared once");
        let expr = lines[0].trim_start()[at.len()..].trim_end().trim_end_matches(';');
        expr.split('*').map(|n| n.trim().replace('_', "").parse::<i64>().unwrap_or_else(|_| panic!("{name}'s `{expr}` is integer literals multiplied"))).product()
    }

    /// Goal: the limits the shell copies (it reads no config of the cell's
    /// that carries them) say what their sources say, so the sidebar never
    /// drifts from them silently: proto's `LISTED_FRESH_MS` and
    /// `LISTED_UNREAD_MAX`, and the bridge's `TURN_IDLE_MS_MAX` and
    /// `PROMPT_TTL_MS_DEFAULT` (images/bridge/src/limits.rs: images/ is a
    /// workspace of its own, so its file is read). Method: each copy and
    /// each Rust source, read and evaluated.
    #[test]
    fn the_shell_copies_its_limits() {
        let root = crate::devstack::repo_root();
        let shell = std::fs::read_to_string(root.join("cell/shell/shell.js")).expect("the shell reads");
        let bridge = std::fs::read_to_string(root.join("images/bridge/src/limits.rs")).expect("the bridge's limits read");
        assert_eq!(constant(&shell, "const", "LISTED_FRESH_MS", " = "), fragment_proto::limits::LISTED_FRESH_MS);
        assert_eq!(constant(&shell, "const", "UNREAD_MAX", " = "), i64::from(fragment_proto::limits::LISTED_UNREAD_MAX));
        for name in ["TURN_IDLE_MS_MAX", "PROMPT_TTL_MS_DEFAULT"] {
            assert_eq!(constant(&shell, "const", name, " = "), constant(&bridge, "pub const", name, ": u64 = "), "the shell's {name} is the bridge's");
        }
    }
}
