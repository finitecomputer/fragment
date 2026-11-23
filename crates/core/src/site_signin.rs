//! A fragment's sign-in rows belong to its life, not its name
//! (cell/src/registry/signin.rs). The registry keys a fragment's site
//! sessions (top-level and frame), the redemptions minted for them, and
//! each person's remembered yes to it (`consents`) by the fragment's name,
//! and a name is free the moment its fragment is deleted. So a name's new
//! life begins with none of them: its create asks the registry to forget
//! every row the name keys (`FORGETS`) before that life is written, and a
//! delete asks it too, once its life ended. A person signed in to, or
//! saying yes to, one life of a name is asked again by the next.
//!
//! Each statement is one `DELETE` on an index of its own (registry step
//! 13), bounded by the people the name's lives knew: a person's site
//! sessions on a name are at most `SITE_SESSIONS_PER_FRAGMENT_MAX` of each
//! kind a platform session, their redemptions `REDEMPTIONS_PER_SESSION_MAX`
//! a platform session, and their yeses one a name.

/// What a name's lives leave the next one: nothing. Each statement binds
/// the fragment's name and answers one row per row it forgot: the unspent
/// redemptions for it, its site and frame sessions, and the yeses given to
/// it, in that order (a redemption names a platform session, never a site
/// one, so the order is the reader's only).
pub const FORGETS: [&str; 3] = [
    "DELETE FROM redemptions WHERE fragment = ?1 RETURNING fragment",
    "DELETE FROM sessions WHERE fragment = ?1 RETURNING fragment",
    "DELETE FROM consents WHERE fragment = ?1 RETURNING fragment",
];

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    const NAME: &str = "notes--k3x9";
    const OTHER: &str = "notes--m4p2";

    fn registry() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        for step in crate::migrate::REGISTRY.steps {
            db.execute_batch(step.sql).unwrap();
        }
        db
    }

    fn count(db: &Connection, sql: &str) -> i64 {
        db.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Goal: a fragment's sign-in rows end with its name's life. Method:
    /// a registry at every step holds one person's platform session, and
    /// for two names a site session, a frame session, an unspent
    /// redemption and a yes each (one more yes from another person on the
    /// first). Valid: forgetting the first name takes each of its rows,
    /// and nothing of the other name's, nor the platform session. Replay:
    /// forgetting it again finds nothing. Each statement reads an index,
    /// never the whole table: a create pays one indexed delete per table.
    #[test]
    fn a_names_new_life_keeps_no_session_redemption_or_yes_of_an_earlier_one() {
        let db = registry();
        db.execute("INSERT INTO sessions (hash, identity, created_at, expires_at) VALUES ('platform', 'npub1ann', 1, 100)", []).unwrap();
        for name in [NAME, OTHER] {
            for (kind, embedder) in [("site", None), ("frame", Some("https://platform.example"))] {
                db.execute(
                    "INSERT INTO sessions (hash, identity, fragment, parent, embedder, created_at, expires_at) VALUES (?, 'npub1ann', ?, 'platform', ?, 2, 100)",
                    params![format!("{kind}-{name}"), name, embedder],
                )
                .unwrap();
            }
            db.execute("INSERT INTO redemptions (hash, session, fragment, return_to, expires_at) VALUES (?, 'platform', ?, '/', 100)", params![format!("redeem-{name}"), name]).unwrap();
            db.execute("INSERT INTO consents (identity, fragment, at) VALUES ('npub1ann', ?, 3)", params![name]).unwrap();
        }
        db.execute("INSERT INTO consents (identity, fragment, at) VALUES ('npub1bob', ?, 4)", params![NAME]).unwrap();

        let forget = |name: &str| -> Vec<usize> {
            FORGETS.iter().map(|sql| db.prepare(sql).unwrap().query_map(params![name], |_| Ok(())).unwrap().count()).collect()
        };
        assert_eq!(forget(NAME), [1, 2, 2], "its redemption, its site and frame sessions, and both people's yeses");
        let of = |table: &str, name: &str| count(&db, &format!("SELECT COUNT(*) FROM {table} WHERE fragment = '{name}'"));
        for table in ["sessions", "redemptions", "consents"] {
            assert_eq!(of(table, NAME), 0, "{table} keeps nothing of the name's earlier life");
        }
        assert_eq!((of("sessions", OTHER), of("redemptions", OTHER), of("consents", OTHER)), (2, 1, 1), "another name's rows are its own");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM sessions WHERE fragment IS NULL"), 1, "the platform session stays");
        assert_eq!(forget(NAME), [0, 0, 0], "a second forget finds nothing");

        for sql in FORGETS {
            let plan: Vec<String> = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap().query_map(params![NAME], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect();
            assert!(plan.iter().any(|p| p.contains("USING INDEX") || p.contains("USING COVERING INDEX")), "{sql} reads an index: {plan:?}");
            assert!(!plan.iter().any(|p| p.starts_with("SCAN")), "{sql} reads no whole table: {plan:?}");
        }
    }
}
