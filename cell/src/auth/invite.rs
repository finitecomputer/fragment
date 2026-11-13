//! An invite asked first, where a fragment's sign-in begins
//! (`/auth/fragment`, FIN-502). Someone invited to a fragment who opens its
//! address, or the link its invite mailed them, is asked there to accept
//! it, and then is on the page they asked for, signed in as a member: not
//! told they have no access until they find the invite in the platform's
//! list and open the fragment again.
//!
//! The page is the platform's (decision R4: a fragment's page never grants
//! anything), with a sharing page's protections (share.rs): unframed, a
//! form token bound to the session and this fragment, buttons that arm a
//! moment after it shows, and a POST from the platform's origin alone. Its
//! yes and its no are the person's own answer to the invite, as `POST
//! /api/invites/<name>/accept` and `decline` give them (the registry's
//! `answer_invite`: decision 63, an invite waits for its person's yes).

use fragment_core::form;
use fragment_proto::ErrorCode;
use worker::*;

use super::{enc, esc, fragment_asked, redirect, to_signin};
use crate::ask_registry;
use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::calls::{By, Consent, Mint};
use crate::registry::invites::AnswerInvite;
use crate::routed::Signed;
use crate::share;

/// The form purpose of a fragment's invite page: a token for one
/// fragment's answers no other's invite.
fn purpose(name: &str) -> String {
    format!("invite:{name}")
}

/// "You're invited": who shared what, to view or to edit, as whom it will
/// know them, and a yes that opens it beside a no that declines it.
/// `waiting` is the invite as `GET /api/invites` names it (proto's
/// `WaitingInvite`: registry/invites.rs `waiting_on`). Its form redirects
/// on to the fragment's origin after a yes, so it has no forms-here rule.
pub(super) fn page(session: &str, who: &Signed, email: Option<&str>, name: &str, back: &str, address: &str, waiting: &serde_json::Value) -> CellResult<Response> {
    let s = |k: &str| waiting[k].as_str();
    let title = s("title").unwrap_or(share::label(name));
    let may = if s("role") == Some("editor") { "edit" } else { "view" };
    let from = s("from").map_or_else(|| "Someone".to_string(), |f| format!("<b>{}</b>", esc(f)));
    let you = match email {
        Some(e) => esc(e),
        None => format!("<code>{}</code>", esc(&who.id)),
    };
    let body = format!(
        "<p>{from} invited you to {may} <b>{t}</b>.</p>\
         <form method=\"post\" action=\"/auth/fragment/invite?name={qn}&amp;return={qb}{address}\"><input type=\"hidden\" name=\"form\" value=\"{f}\">\
         <footer class=\"answer\"><button class=\"quiet\" name=\"answer\" value=\"decline\" data-arm disabled>Decline</button><button name=\"answer\" value=\"accept\" data-arm disabled>Accept and open</button></footer></form>\
         <p class=\"hint\">Accepting makes you a member: it knows you as {you}, and it is in your fragments.</p>",
        t = esc(title),
        qn = esc(&enc(name)),
        qb = esc(&enc(back)),
        address = esc(address),
        f = esc(&form::issue(session, &purpose(name), crate::js::now_ms())),
    );
    share::sheet_page(200, "You're invited", &body, false)
}

/// `POST /auth/fragment/invite?name=&return=`: the invite page's answer
/// (`answer`: `accept` or `decline`). A yes makes them a member (the
/// fragment claims it), then signs them in there as one, on to the page
/// they asked for; a no declines it, then the platform's own page. An
/// invite no longer waiting (taken back, answered in another tab, run out)
/// says so, with the way on to the fragment as it stands for them now; one
/// the fragment could not take (full, unreachable) keeps the invite and
/// says why, to try again.
pub(super) async fn answered(req: &mut Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    let (name, back) = fragment_asked(url)?;
    let (session, _, fields) = match share::poster(req, env, url, &cfg.platform(), &purpose(&name)).await? {
        Ok(posted) => posted,
        Err(page) => return Ok(page),
    };
    let yes = match fields.get("answer").map(String::as_str) {
        Some("accept") => true,
        Some("decline") => false,
        _ => return Err(CellError::invalid("an invite's answer is accept or decline")),
    };
    let again = format!("/auth/fragment?name={}&return={}{}", enc(&name), enc(&back), super::address_query(cfg, url));
    let answer = AnswerInvite { by: By::Session(session.clone()), acting_for: None, fragment: name.clone(), yes };
    match (ask_registry(env, &answer).await, yes) {
        (Ok(Some(_)), true) => {}
        (Ok(None), false) => return Ok(redirect("/", &[])?.with_status(303)),
        (Ok(_), _) => return Err(CellError::host("the registry's answer to an invite's yes named no role, or one after a no")),
        (Err(e), _) if e.code == ErrorCode::NotFound => return unanswered(404, "This invite is gone", &e.message, &again, share::label(&name)),
        (Err(e), _) if e.code.status() < 500 => return unanswered(e.code.status(), "Not answered", &e.message, &again, share::label(&name)),
        (Err(e), _) => return Err(e),
    }
    // a member now (the fragment answered its claim with their role)
    let member = Mint { token: Some(session), key: None, fragment: name.clone(), return_to: back, consent: Consent::Member, embedder: None, invite_first: false };
    let redeem = ask_registry(env, &member).await?.redeem.ok_or_else(|| CellError::host("a member's mint answered no redemption"))?;
    Ok(to_signin(env, cfg, url, &name, &redeem).await?.with_status(303))
}

/// An answer that did not go through: why, in the platform's words, and
/// the way on to the fragment (`again`: its sign-in, which asks again
/// what it must).
fn unanswered(status: u16, title: &str, why: &str, again: &str, label: &str) -> CellResult<Response> {
    let body = format!("<p>{}</p><p><a href=\"{}\">Continue to {}</a></p>", esc(why), esc(again), esc(label));
    share::sheet_page(status, title, &body, false)
}
