//! Several accounts at one connection (its row's `accounts_max`;
//! docs/pipes-browser-binding.md, Several accounts), through the same
//! browser-bound callback as one: a first account, another one added with
//! its own scopes, an account added again (the same provider account)
//! reauthorized rather than made twice, Pipes' refusals (one account for
//! now, its limit) and the row's own, one account disconnected while the
//! others stay, and its lasting fence holding against an import of it
//! that lands late; a disconnect that did not finish, asked again (an
//! account from before identifiers too); two callbacks finishing together,
//! one deciding after the other; and one that waited its turn past a
//! logout or its time, importing nothing. A row of one account refuses all
//! of it.
use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{back, refused};
use crate::api::{Api, Call, Reply};
use crate::{Suite, SEVERAL, SEVERAL_MAX};

/// A call from the shell, signed in by its session, with a JSON body.
fn shell(api: &Api, session: &str, method: &'static str, path: &str, body: Option<&Value>) -> Result<Reply> {
    api.call(Call {
        method,
        url: format!("{}{path}", api.base),
        body: body.map(|b| b.to_string().into_bytes()),
        content_type: body.map(|_| "application/json"),
        cookie: Some(format!("fragment_session={session}")),
        extra: vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())],
        ..Call::default()
    })
}

/// The row's accounts in the person's `GET /api/connections`, and the row.
fn listed(api: &Api, session: &str) -> Result<(Vec<Value>, Value)> {
    let r = shell(api, session, "GET", "/api/connections", None)?;
    anyhow::ensure!(r.status == 200, "list connections: {r}");
    let row = r.body["providers"].as_array().and_then(|l| l.iter().find(|p| p["provider"] == SEVERAL)).cloned().unwrap_or_default();
    Ok((row["accounts"].as_array().cloned().unwrap_or_default(), row))
}

/// A connect asked with `body`, through the provider's consent as the
/// stand-in `account`, leaving only `grant`'s boxes ticked (or all), and
/// its callback: (the authorize answer, the callback's).
fn connect(api: &Api, session: &str, body: Value, account: &str, grant: Option<&str>) -> Result<(Reply, Option<Reply>)> {
    let (r, back_to) = prepared(api, session, body, account, grant)?;
    let came = match back_to {
        Some((url, cookie)) => Some(back(api, &url, Some(session), Some(&cookie))?),
        None => None,
    };
    Ok((r, came))
}

/// A connect as `connect`, up to its callback: the authorize answer, and
/// where the browser goes back with its cookie (none: refused).
fn prepared(api: &Api, session: &str, body: Value, account: &str, grant: Option<&str>) -> Result<(Reply, Option<(String, String)>)> {
    let r = shell(api, session, "POST", &format!("/api/connections/{SEVERAL}/authorize"), Some(&body))?;
    if r.status != 200 {
        return Ok((r, None));
    }
    let cookie = r.cookies().into_iter().find(|c| c.starts_with("fragment_connection=")).context("connection cookie")?;
    let mut to = reqwest::Url::parse(r.body["url"].as_str().context("authorize URL")?)?;
    to.query_pairs_mut().append_pair("account", account);
    if let Some(grant) = grant {
        to.query_pairs_mut().append_pair("grant", grant);
    }
    let consent = api.external(to.as_str())?;
    anyhow::ensure!(consent.status == 302, "fake consent: {consent}");
    Ok((r, Some((consent.header("location"), cookie))))
}

fn ok(r: &(Reply, Option<Reply>)) -> bool {
    r.0.status == 200 && r.1.as_ref().is_some_and(|c| c.status == 200)
}

fn refused_as(r: &(Reply, Option<Reply>), word: &str) -> bool {
    r.1.as_ref().is_some_and(|c| refused(c, word))
}

fn ids(accounts: &[Value]) -> Vec<String> {
    accounts.iter().map(|a| a["id"].as_str().unwrap_or_default().to_string()).collect()
}

pub(super) fn several(s: &mut Suite, api: &Api) -> Result<()> {
    let email = "several-a@e2e.test";
    let a = api.sign_in(email)?;
    let add = || json!({ "add": true });

    // a row of one account takes neither
    let one = shell(api, &a, "POST", "/api/connections/e2e-oauth/authorize", Some(&add()))?;
    let one_off = shell(api, &a, "DELETE", "/api/connections/e2e-oauth?account=data_installation_1", None)?;
    s.ok("a row of one account refuses another account, or one named, before any provider is asked", one.status == 400 && one_off.status == 400, format!("{one}; {one_off}"));

    let first = connect(api, &a, json!({}), "work-a", None)?;
    let (accounts, row) = listed(api, &a)?;
    let held = s.workos.accounts(email, SEVERAL);
    s.ok(
        "a first account connects as with one: Pipes' compatibility connection, named by the provider account's own id (its ID token's sub), listed with its label and scopes",
        ok(&first) && row["state"] == "connected" && row["accountsMax"] == SEVERAL_MAX && accounts.len() == 1 && accounts[0]["account"] == "work-a@oauth.test"
            && accounts[0]["missing"] == json!([]) && held.len() == 1 && held[0].compatibility && held[0].identifier.as_deref() == Some("work-a") && row["account"] == "work-a@oauth.test",
        json!({ "row": row, "held": format!("{held:?}") }),
    );
    let work = ids(&accounts)[0].clone();

    let second = connect(api, &a, add(), "home-a", Some("mail.read openid"))?;
    let (accounts, row) = listed(api, &a)?;
    let held = s.workos.accounts(email, SEVERAL);
    s.ok(
        "another account is added beside it, oldest first, each with its own scopes and what it was not allowed; the first stays the default",
        ok(&second) && accounts.len() == 2 && ids(&accounts)[0] == work && accounts[1]["account"] == "home-a@oauth.test"
            && accounts[1]["granted"] == json!(["mail.read", "openid"]) && accounts[1]["missing"] == json!(["files.read"]) && accounts[0]["missing"] == json!([])
            && row["account"] == "work-a@oauth.test" && held.len() == 2 && !held[1].compatibility,
        &row,
    );
    let home = ids(&accounts)[1].clone();

    let imports = s.workos.connection_counts().1;
    let again = connect(api, &a, add(), "home-a", None)?;
    let (accounts, _) = listed(api, &a)?;
    s.ok(
        "an account added again (the same provider account) reauthorizes the one there, never a second: its grant now whole",
        ok(&again) && ids(&accounts) == [work.clone(), home.clone()] && accounts[1]["missing"] == json!([]) && s.workos.accounts(email, SEVERAL).len() == 2 && s.workos.connection_counts().1 == imports + 1,
        json!(accounts),
    );
    let named = connect(api, &a, json!({ "account": home }), "home-a", Some("mail.read openid"))?;
    let unknown = connect(api, &a, json!({ "account": "data_installation_nonesuch" }), "x", None)?;
    let (accounts, _) = listed(api, &a)?;
    s.ok(
        "Reconnect to add names its account, which the grant updates; an account not theirs is refused before the provider's page",
        ok(&named) && accounts[1]["granted"] == json!(["mail.read", "openid"]) && unknown.0.status == 404 && unknown.1.is_none(),
        format!("{}; {}", named.0, unknown.0),
    );

    // Pipes' refusals: one account a person for now, then its limit
    let imports = s.workos.connection_counts().1;
    s.workos.refuse_additional_accounts(true);
    let refused_one = connect(api, &a, add(), "third-a", None)?;
    s.workos.refuse_additional_accounts(false);
    let page = match &refused_one.1 {
        Some(r) => back(api, &r.header("location"), Some(&a), None)?,
        None => anyhow::bail!("no callback: {}", refused_one.0),
    };
    s.ok(
        "while Pipes connects one account a person, another is refused as one-account: a page that says so, nothing imported",
        refused_as(&refused_one, "one-account") && page.status == 400 && page.text.contains("can connect one") && s.workos.connection_counts().1 == imports && s.workos.accounts(email, SEVERAL).len() == 2,
        &page,
    );
    s.workos.limit_accounts(Some(2));
    let full = connect(api, &a, add(), "third-a", None)?;
    s.workos.limit_accounts(None);
    s.ok("past Pipes' limit, another is refused as account-limit, nothing imported", refused_as(&full, "account-limit") && s.workos.accounts(email, SEVERAL).len() == 2, full.1.as_ref().map(|r| r.to_string()).unwrap_or_default());
    let third = connect(api, &a, add(), "third-a", None)?;
    let past = connect(api, &a, add(), "fourth-a", None)?;
    s.ok(
        "at the row's most (accounts_max), another is refused before the provider's page",
        ok(&third) && past.0.status == 400 && past.1.is_none() && s.workos.accounts(email, SEVERAL).len() == usize::from(SEVERAL_MAX),
        &past.0,
    );

    // disconnecting one: named, the others stay
    let path = format!("/api/connections/{SEVERAL}");
    let unnamed = shell(api, &a, "DELETE", &path, None)?;
    s.ok("a disconnect naming no account, with several connected, is refused, naming them; nothing goes", unnamed.status == 400 && unnamed.text.contains(&home) && s.workos.accounts(email, SEVERAL).len() == 3, &unnamed);
    let revoked = s.workos.revoked().len();
    let off = shell(api, &a, "DELETE", &format!("{path}?account={home}"), None)?;
    let (accounts, row) = listed(api, &a)?;
    let minted = s.workos.revoked().last().cloned().unwrap_or_default();
    s.ok(
        "a disconnect of one account revokes that account's grant (a token Pipes mints for it) and deletes it alone: the others stay connected",
        off.status == 200 && off.body["state"] == "connected" && off.body["account"] == home.as_str() && off.body["revoked"] == true
            && s.workos.revoked().len() == revoked + 1 && s.workos.token_account(&minted).as_deref() == Some(home.as_str())
            && !ids(&accounts).contains(&home) && accounts.len() == 2 && row["state"] == "connected" && row["account"] == "work-a@oauth.test",
        json!({ "off": off.body, "accounts": accounts }),
    );

    // an import of an account whose outcome was unknown, landing after
    // that account was disconnected: fenced by its identifier alone
    s.workos.defer_next_import();
    let uncertain = connect(api, &a, add(), "late-a", None)?;
    let held = connect(api, &a, add(), "late-a", None)?;
    s.ok(
        "while an import's outcome is unknown, another connect at that person's accounts is refused as finishing, nothing imported: a list read now could lack it",
        refused_as(&uncertain, "no-key") && refused_as(&held, "finishing") && !s.workos.accounts(email, SEVERAL).iter().any(|x| x.identifier.as_deref() == Some("late-a")),
        held.1.as_ref().map(|r| r.to_string()).unwrap_or_default(),
    );
    // its while run out
    let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": "forgetUncertainImports" })))?;
    anyhow::ensure!(hook.status == 200, "let the unknown import go: {hook}");
    let known = connect(api, &a, add(), "late-a", None)?;
    let (accounts, _) = listed(api, &a)?;
    let late = accounts.iter().find(|x| x["account"] == "late-a@oauth.test").and_then(|x| x["id"].as_str()).unwrap_or_default().to_string();
    let off = shell(api, &a, "DELETE", &format!("{path}?account={late}"), None)?;
    let landed = s.workos.land_deferred_import();
    let (accounts, row) = listed(api, &a)?;
    let theirs = |l: &[Value]| l.iter().filter(|x| x["disconnecting"] != true).count();
    s.ok(
        "an import of an account whose outcome was unknown, landing after that account was disconnected, is not taken for it (listed as a disconnect to finish, used by no one); the person's other accounts stay connected",
        ok(&known) && off.status == 200 && landed
            && s.workos.accounts(email, SEVERAL).iter().any(|x| x.identifier.as_deref() == Some("late-a"))
            && accounts.iter().filter(|x| x["account"] == "late-a@oauth.test").all(|x| x["disconnecting"] == true && x["state"] == "not_connected")
            && theirs(&accounts) == 2 && row["state"] == "connected",
        json!({ "accounts": accounts, "landed": landed }),
    );
    let lifted = connect(api, &a, add(), "late-a", None)?;
    let (accounts, _) = listed(api, &a)?;
    s.ok(
        "connected again, a connect known to import lifts that account's fence: the account Pipes held is reauthorized, not a second",
        ok(&lifted) && accounts.iter().filter(|x| x["account"] == "late-a@oauth.test").count() == 1 && s.workos.accounts(email, SEVERAL).len() == 3,
        json!(accounts),
    );

    retried(s, api)?;
    raced(s, api)?;
    outlasted(s, api)?;

    // one account and no name: that one
    let b_email = "several-b@e2e.test";
    let b = api.sign_in(b_email)?;
    let solo = connect(api, &b, json!({}), "solo-b", None)?;
    let off = shell(api, &b, "DELETE", &path, None)?;
    let (accounts, row) = listed(api, &b)?;
    s.ok(
        "with one account, a disconnect naming none takes that one away",
        ok(&solo) && off.status == 200 && off.body["state"] == "not_connected" && accounts.is_empty() && row["state"] == "not_connected" && !s.workos.holds(b_email, SEVERAL),
        &off,
    );
    Ok(())
}

/// A disconnect that did not finish, asked again: Pipes failing its delete
/// after the fence (the account still theirs to disconnect, said so in
/// settings), and failing after its delete (telling the computer), each
/// finished by the same disconnect asked again, as often as asked; never
/// one of another person's.
fn retried(s: &mut Suite, api: &Api) -> Result<()> {
    let email = "several-c@e2e.test";
    let c = api.sign_in(email)?;
    let path = format!("/api/connections/{SEVERAL}");
    let work = connect(api, &c, json!({}), "c-work", None)?;
    let home = connect(api, &c, json!({ "add": true }), "c-home", None)?;
    let third = connect(api, &c, json!({ "add": true }), "c-third", None)?;
    anyhow::ensure!(ok(&work) && ok(&home) && ok(&third), "connect c's accounts");
    let id_of = |identifier: &str| s.workos.accounts(email, SEVERAL).into_iter().find(|a| a.identifier.as_deref() == Some(identifier)).map(|a| a.id).unwrap_or_default();
    let (home, third) = (id_of("c-home"), id_of("c-third"));

    s.workos.fail_next_delete();
    let failed = shell(api, &c, "DELETE", &format!("{path}?account={home}"), None)?;
    let (accounts, row) = listed(api, &c)?;
    let shown = accounts.iter().find(|a| a["id"] == home.as_str()).cloned().unwrap_or_default();
    s.ok(
        "a disconnect whose delete at Pipes fails is refused, and settings lists the account as a disconnect to finish, not connected, used by no one; the others stay",
        failed.status >= 500 && shown["disconnecting"] == true && shown["state"] == "not_connected" && row["state"] == "connected" && s.workos.accounts(email, SEVERAL).len() == 3,
        json!({ "failed": failed.to_string(), "accounts": accounts }),
    );
    let again = shell(api, &c, "DELETE", &format!("{path}?account={home}"), None)?;
    let (accounts, _) = listed(api, &c)?;
    s.ok(
        "asked again, the same disconnect finishes: the account deleted at Pipes, and listed no more",
        again.status == 200 && again.body["account"] == home.as_str() && !ids(&accounts).contains(&home) && !s.workos.accounts(email, SEVERAL).iter().any(|a| a.id == home),
        json!({ "again": again.body, "accounts": accounts }),
    );

    // its delete answered, then telling the person's computer fails (the
    // list it tells it from)
    s.workos.fail_list_after(1);
    let failed = shell(api, &c, "DELETE", &format!("{path}?account={third}"), None)?;
    let gone = !s.workos.accounts(email, SEVERAL).iter().any(|a| a.id == third);
    let again = shell(api, &c, "DELETE", &format!("{path}?account={third}"), None)?;
    let more = shell(api, &c, "DELETE", &format!("{path}?account={third}"), None)?;
    let (accounts, row) = listed(api, &c)?;
    s.ok(
        "a disconnect that fails after its delete at Pipes finishes when asked again, and again, as the person's own account: one connected account left",
        failed.status >= 500 && gone && again.status == 200 && more.status == 200 && again.body["account"] == third.as_str() && again.body["revoked"] == false
            && ids(&accounts).len() == 1 && accounts[0]["disconnecting"] != true && row["state"] == "connected",
        json!({ "failed": failed.to_string(), "again": again.body, "more": more.body, "accounts": accounts }),
    );
    // accounts connected before identifiers were (the fake's lever names
    // none): beside another, and as the last, each finished when asked again
    for (email, beside) in [("several-e@e2e.test", true), ("several-f@e2e.test", false)] {
        let e = api.sign_in(email)?;
        s.workos.connect(email, SEVERAL, true);
        s.workos.label(email, SEVERAL, "legacy@oauth.test");
        if beside {
            s.workos.add_account(email, SEVERAL, "e-other", "other@oauth.test");
        }
        let (accounts, _) = listed(api, &e)?;
        let legacy = accounts.iter().find(|a| a["account"] == "legacy@oauth.test").and_then(|a| a["id"].as_str()).unwrap_or_default().to_string();
        s.workos.fail_list_after(1);
        let failed = shell(api, &e, "DELETE", &format!("{path}?account={legacy}"), None)?;
        let gone = !s.workos.accounts(email, SEVERAL).iter().any(|a| a.id == legacy);
        let again = shell(api, &e, "DELETE", &format!("{path}?account={legacy}"), None)?;
        let (accounts, row) = listed(api, &e)?;
        s.ok(
            &format!("an account that names no identifier, disconnected {}, failing after its delete at Pipes, finishes when asked again", if beside { "beside another" } else { "as the last" }),
            !legacy.is_empty() && failed.status >= 500 && gone && again.status == 200 && again.body["account"] == legacy.as_str()
                && accounts.len() == usize::from(beside) && row["state"] == if beside { "connected" } else { "not_connected" },
            json!({ "failed": failed.to_string(), "again": again.body, "accounts": accounts }),
        );
    }
    let other = s.workos.accounts("several-a@e2e.test", SEVERAL).first().map(|a| a.id.clone()).unwrap_or_default();
    let theirs = shell(api, &c, "DELETE", &format!("{path}?account={other}"), None)?;
    let none = shell(api, &c, "DELETE", &format!("{path}?account=data_installation_nonesuch"), None)?;
    s.ok(
        "a disconnect naming another person's account, or none Pipes ever held, is 404, and nothing of theirs goes",
        !other.is_empty() && theirs.status == 404 && none.status == 404 && s.workos.accounts("several-a@e2e.test", SEVERAL).iter().any(|a| a.id == other),
        format!("{theirs}; {none}"),
    );
    Ok(())
}

/// Two callbacks of one person at one provider finishing together, from
/// two browsers: the first holds its import (the fake's gate) while the
/// second waits for its turn, never at the import, then reads the list
/// the first left. The same provider account twice is one account; two
/// into the last place, one, the other refused as account-limit.
fn raced(s: &mut Suite, api: &Api) -> Result<()> {
    use fragment_fakes::workos::Gate;
    let email = "several-d@e2e.test";
    let (one, two) = (api.sign_in(email)?, api.sign_in(email)?);
    let first = connect(api, &one, json!({}), "d-one", None)?;
    anyhow::ensure!(ok(&first), "connect d's first account");
    let wait = std::time::Duration::from_secs(20);
    // the two callbacks, the first held at its import until the second has
    // exchanged its code and had time to reach its own
    let together = |s: &mut Suite, first: (String, String), second: (String, String)| -> Result<(Reply, Reply, bool)> {
        let exchanged = s.workos.connection_counts().0;
        s.workos.pause(Gate::Import);
        let (a, b, waited) = std::thread::scope(|scope| {
            let a = scope.spawn(|| back(api, &first.0, Some(&one), Some(&first.1)));
            let importing = s.eventually(wait, || s.workos.paused(Gate::Import) == 1);
            let b = scope.spawn(|| back(api, &second.0, Some(&two), Some(&second.1)));
            let second_exchanged = s.eventually(wait, || s.workos.connection_counts().0 == exchanged + 2);
            std::thread::sleep(std::time::Duration::from_secs(1));
            let waited = importing && second_exchanged && s.workos.paused(Gate::Import) == 1;
            s.workos.resume(Gate::Import);
            (a.join().expect("first callback"), b.join().expect("second callback"), waited)
        });
        Ok((a?, b?, waited))
    };
    let add = || json!({ "add": true });
    let (_, x) = prepared(api, &one, add(), "d-two", None)?;
    let (_, y) = prepared(api, &two, add(), "d-two", None)?;
    let (a, b, waited) = together(s, x.context("first")?, y.context("second")?)?;
    let held = s.workos.accounts(email, SEVERAL);
    s.ok(
        "two callbacks adding the same provider account together: the second waits for the first's import, then reauthorizes it; one account, never two",
        waited && a.status == 200 && b.status == 200 && held.len() == 2 && held.iter().filter(|x| x.identifier.as_deref() == Some("d-two")).count() == 1,
        json!({ "waited": waited, "first": a.status, "second": b.status, "held": held.len() }),
    );
    let (_, x) = prepared(api, &one, add(), "d-three", None)?;
    let (_, y) = prepared(api, &two, add(), "d-four", None)?;
    let (a, b, waited) = together(s, x.context("first")?, y.context("second")?)?;
    let held = s.workos.accounts(email, SEVERAL);
    s.ok(
        "two callbacks adding two accounts into the last place together: the first connects, the second, reading the list after it, is refused as account-limit",
        waited && a.status == 200 && refused(&b, "account-limit") && held.len() == usize::from(crate::SEVERAL_MAX) && held.iter().any(|x| x.identifier.as_deref() == Some("d-three")),
        json!({ "waited": waited, "first": a.status, "second": b.to_string(), "held": held.len() }),
    );
    Ok(())
}

/// A callback that waited its turn while another imported, past its
/// session's logout or its attempt's time: checked again once it has its
/// turn and the list, it imports nothing, and one import results.
fn outlasted(s: &mut Suite, api: &Api) -> Result<()> {
    use fragment_fakes::workos::Gate;
    let email = "several-g@e2e.test";
    let one = api.sign_in(email)?;
    let first = connect(api, &one, json!({}), "g-one", None)?;
    anyhow::ensure!(ok(&first), "connect g's first account");
    let wait = std::time::Duration::from_secs(20);
    let add = || json!({ "add": true });
    let state_of = |r: &Reply| reqwest::Url::parse(r.body["url"].as_str().unwrap_or_default()).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())).unwrap_or_default();
    // the first held at its import, the second past its exchange and its
    // checks, waiting its turn; then `meanwhile`, then the first let go
    let waited = |s: &mut Suite, first: (String, String), second: (&str, String, String), meanwhile: &dyn Fn() -> Result<()>| -> Result<(Reply, Reply, bool)> {
        let exchanged = s.workos.connection_counts().0;
        s.workos.pause(Gate::Import);
        let (a, b, held) = std::thread::scope(|scope| {
            let a = scope.spawn(|| back(api, &first.0, Some(&one), Some(&first.1)));
            let importing = s.eventually(wait, || s.workos.paused(Gate::Import) == 1);
            let b = scope.spawn(|| back(api, &second.1, Some(second.0), Some(&second.2)));
            let past = s.eventually(wait, || s.workos.connection_counts().0 == exchanged + 2);
            std::thread::sleep(std::time::Duration::from_secs(1));
            let done = meanwhile();
            let held = importing && past && done.is_ok() && s.workos.paused(Gate::Import) == 1;
            s.workos.resume(Gate::Import);
            (a.join().expect("first callback"), b.join().expect("second callback"), held)
        });
        Ok((a?, b?, held))
    };

    // logged out while it waits
    let two = api.sign_in(email)?;
    let (_, x) = prepared(api, &one, add(), "g-two", None)?;
    let (_, y) = prepared(api, &two, add(), "g-three", None)?;
    let (x, y) = (x.context("first")?, y.context("second")?);
    let imports = s.workos.connection_counts().1;
    let end = || -> Result<()> {
        let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "expireSession": two } })))?;
        anyhow::ensure!(hook.status == 200, "end the second session: {hook}");
        Ok(())
    };
    let (a, b, held) = waited(s, x, (&two, y.0, y.1), &end)?;
    let has = |s: &Suite, identifier: &str| s.workos.accounts(email, SEVERAL).iter().any(|x| x.identifier.as_deref() == Some(identifier));
    s.ok(
        "a callback whose session logs out while it waits its turn imports nothing once it has it (signed-out); the first's import is the one",
        held && a.status == 200 && refused(&b, "signed-out") && has(s, "g-two") && !has(s, "g-three") && s.workos.connection_counts().1 == imports + 1,
        json!({ "held": held, "first": a.status, "second": b.to_string() }),
    );

    // past its time while it waits
    let three = api.sign_in(email)?;
    let (_, x) = prepared(api, &one, add(), "g-four", None)?;
    let (r, y) = prepared(api, &three, add(), "g-five", None)?;
    let (x, y, nonce) = (x.context("first")?, y.context("second")?, state_of(&r));
    let imports = s.workos.connection_counts().1;
    let lever = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "expireConnectionIn": { "state": nonce, "ms": 3000 } } })))?;
    anyhow::ensure!(lever.status == 200, "the second attempt's time: {lever}");
    let set = std::time::Instant::now();
    let lapse = || -> Result<()> {
        std::thread::sleep(std::time::Duration::from_millis(3500).saturating_sub(set.elapsed()));
        Ok(())
    };
    let (a, b, held) = waited(s, x, (&three, y.0, y.1), &lapse)?;
    s.ok(
        "a callback whose attempt's ten minutes run out while it waits its turn imports nothing once it has it (not-under-way); the first's import is the one",
        held && a.status == 200 && refused(&b, "not-under-way") && has(s, "g-four") && !has(s, "g-five") && s.workos.connection_counts().1 == imports + 1,
        json!({ "held": held, "first": a.status, "second": b.to_string() }),
    );
    Ok(())
}
