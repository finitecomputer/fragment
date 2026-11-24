//! A fragment's sign-in rows belong to its life, not its name
//! (cell/src/registry/signin.rs). The registry keys a fragment's site
//! sessions (top-level and frame), the redemptions minted for them, and
//! each person's remembered yes to it (`consents`) by the fragment's name,
//! and a name is free the moment its fragment is deleted. So a name's new
//! life begins with none of them, and a person signed in to, or saying yes
//! to, one life of a name is asked again by the next:
//!
//! - **A create forgets the name's rows** (`FORGETS`) before its life is
//!   written, and counts the name's next life (`NEXT_LIFE`, `site_lives`)
//!   in the same turn of the registry's. It keeps that count as its own.
//! - **A member's mint holds only for the life it was asked about**. A
//!   mint for someone the fragment knows is asked in two steps: whether
//!   the fragment knows them (the router asks it; or an invite's answer,
//!   which the fragment claims), then the mint. Each first reads the
//!   name's life (`LIFE_OF`) before the fragment is asked, and the mint
//!   goes on only while that is still the name's (`still_its_life`): a
//!   create in between, which forgot every row, refuses it.
//! - **A delete forgets its own life's rows, never a newer one's**: it
//!   names the life it ended, and the registry forgets only while that is
//!   still the name's (`still_its_life`). A create begun since forgot the
//!   ended life's rows itself, and every row since is the newer life's,
//!   as every other part of an ended life's cleanup leaves the newer life
//!   its own (cell/src/ended.rs). A life made before this rule recorded no
//!   count: its delete forgets nothing, and the rows wait for the name's
//!   next create, or their own end (a session's 30 days, a redemption's
//!   minute); no life answers on the name meanwhile, so no one reads them.
//!
//! Each forget is one `DELETE` on an index of its own (registry step
//! 13), bounded by the people the name's lives knew: a person's site
//! sessions on a name are at most `SITE_SESSIONS_PER_FRAGMENT_MAX` of each
//! kind a platform session, their redemptions `REDEMPTIONS_PER_SESSION_MAX`
//! a platform session, and their yeses one a name. `site_lives` keeps one
//! row a name a create has begun (step 14), as `public_addresses` keeps
//! one an app.

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

/// The name's life as the registry counts it (binds the name): 0 for a
/// name no create has counted.
pub const LIFE_OF: &str = "SELECT COALESCE((SELECT life FROM site_lives WHERE fragment = ?1), 0) AS life";

/// A create's: the name's next life, counted in the turn that forgets its
/// rows (binds the name, answers the new count).
pub const NEXT_LIFE: &str = "INSERT INTO site_lives (fragment, life) VALUES (?1, 1) ON CONFLICT (fragment) DO UPDATE SET life = life + 1 RETURNING life";

/// Whether what was asked about one life of a name (`asked`: the count
/// read before the fragment was asked, or the count a delete's life kept)
/// may act on the name now (`now`): only while that is still its life.
/// 0 is a life too, one made before the count (no create counted it), and
/// holds until a create counts the next; none (nothing read) never holds.
pub fn still_its_life(asked: Option<u64>, now: u64) -> bool {
    match asked {
        Some(asked) => asked == now,
        None => false,
    }
}

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

        for sql in FORGETS.iter().chain([&LIFE_OF]) {
            let plan: Vec<String> = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap().query_map(params![NAME], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect();
            assert!(plan.iter().any(|p| p.contains("USING INDEX") || p.contains("USING COVERING INDEX")), "{sql} reads an index: {plan:?}");
            assert!(!plan.iter().any(|p| p.starts_with("SCAN") && p != "SCAN CONSTANT ROW"), "{sql} reads no whole table: {plan:?}");
        }
    }

    fn life(db: &Connection, name: &str) -> u64 {
        db.query_row(LIFE_OF, params![name], |r| r.get::<_, i64>(0)).unwrap() as u64
    }

    /// A create's turn in the registry: the name's rows forgotten, its next
    /// life counted (cell/src/registry/signin.rs `forget_site`).
    fn create(db: &Connection, name: &str) -> u64 {
        for sql in FORGETS {
            db.prepare(sql).unwrap().query_map(params![name], |_| Ok(())).unwrap().for_each(drop);
        }
        db.query_row(NEXT_LIFE, params![name], |r| r.get::<_, i64>(0)).unwrap() as u64
    }

    fn signed_in(db: &Connection, name: &str, hash: &str) {
        db.execute("INSERT INTO sessions (hash, identity, fragment, parent, created_at, expires_at) VALUES (?, 'npub1ann', ?, 'platform', 2, 100)", params![hash, name]).unwrap();
        db.execute("INSERT OR REPLACE INTO consents (identity, fragment, at) VALUES ('npub1ann', ?, 3)", params![name]).unwrap();
    }

    /// Goal: a member's mint asked about one life of a name is refused once
    /// the name was made again. Method: the router's two steps across a
    /// create, against the registry's SQL: the life read with the first
    /// mint, the fragment asked (it knew them), then the name deleted and
    /// made again before the member's mint. Valid: no create between, and
    /// one asked after the create. Invalid: the one asked before. Replay:
    /// asked again about the new life, it holds, until a third life.
    #[test]
    fn a_members_mint_asked_about_an_earlier_life_is_refused_after_the_name_is_made_again() {
        let db = registry();
        assert_eq!(life(&db, NAME), 0, "a name no create counted");
        assert!(still_its_life(Some(life(&db, NAME)), life(&db, NAME)), "a life made before the count signs its members in as before");
        let before = life(&db, NAME);
        let first = create(&db, NAME);
        assert!(!still_its_life(Some(before), life(&db, NAME)), "and is refused once a create counted the next");
        assert_eq!(first, 1, "its first life counted");
        let asked = life(&db, NAME);
        assert!(still_its_life(Some(asked), life(&db, NAME)), "no create between: the member is minted");
        let second = create(&db, NAME);
        assert!(second > first, "the name made again counts a newer life");
        assert!(!still_its_life(Some(asked), life(&db, NAME)), "asked about the earlier life, the member's mint is refused");
        let again = life(&db, NAME);
        assert!(still_its_life(Some(again), life(&db, NAME)), "asked about the life it has now, it holds");
        create(&db, NAME);
        assert!(!still_its_life(Some(again), life(&db, NAME)), "and is refused after the next");
        assert!(!still_its_life(None, life(&db, NAME)), "a mint that read no life holds for none");
        assert_eq!(life(&db, OTHER), 0, "another name's count is its own");
    }

    /// Goal: an invite accepted in one life of a name signs no one in to
    /// the next. Method: the invite's answer reads the life before the
    /// fragment claims it (the claim made them a member of that life), the
    /// name is deleted and made again, then the member's mint. Valid: the
    /// claim and the mint in one life. Invalid: a create between them.
    #[test]
    fn an_invite_accepted_in_an_earlier_life_mints_no_member_after_the_name_is_made_again() {
        let db = registry();
        create(&db, NAME);
        let answered = life(&db, NAME);
        assert!(still_its_life(Some(answered), life(&db, NAME)), "claimed and minted in one life");
        create(&db, NAME);
        assert!(!still_its_life(Some(answered), life(&db, NAME)), "claimed in the earlier life, minted after the name was made again: refused");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM redemptions"), 0, "and nothing minted for it is kept from before");
    }

    /// Goal: a delete forgets its own life's sign-ins and never a newer
    /// life's. Method: a life signs someone in, is deleted, and its name is
    /// made again and signs them in before the old delete's forget runs;
    /// that forget names the life it ended. Invalid: the old forget there
    /// leaves the new life's rows. Valid: the new life's own delete forgets
    /// them. A life no create counted (`None`, from before) forgets none.
    #[test]
    fn an_old_lifes_late_forget_keeps_the_sign_ins_of_the_life_made_since() {
        let db = registry();
        let old = create(&db, NAME);
        signed_in(&db, NAME, "old-life");
        let new = create(&db, NAME);
        signed_in(&db, NAME, "new-life");
        let late = |ended: Option<u64>| {
            if still_its_life(ended, life(&db, NAME)) {
                for sql in FORGETS {
                    db.prepare(sql).unwrap().query_map(params![NAME], |_| Ok(())).unwrap().for_each(drop);
                }
            }
        };
        late(Some(old));
        late(None);
        let kept: Vec<String> = db.prepare("SELECT hash FROM sessions WHERE fragment = ?1").unwrap().query_map(params![NAME], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(kept, ["new-life"], "the old life's delete keeps the new life's session; the create forgot the old one's");
        assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM consents WHERE fragment = '{NAME}'")), 1, "and the yes given to the new life");
        late(Some(new));
        assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM sessions WHERE fragment = '{NAME}'")), 0, "the new life's own delete forgets them");
        assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM consents WHERE fragment = '{NAME}'")), 0);
    }
}
