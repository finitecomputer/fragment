//! Asking for access, on the platform's page (docs/api.md, Asking for
//! access; FIN-502). Someone signed in who opens a fragment they are not
//! in is told so on its origin (auth.rs `refused`), with a link here, where
//! they ask its owner for it: the fragment sends the request to the
//! registry itself (`POST /signin/ask`, the router's alone: members.rs
//! `ask_access`), naming its owner and its life, and the registry keeps it
//! against them, who see it among their notices and are mailed
//! (registry/asks.rs). The owner's identity never reaches the requester.
//!
//! Someone with an invite to it waiting is sent to the invite instead
//! (`/auth/fragment`, auth/invite.rs), and someone in it already is told
//! so, with the way in. The page is a sharing page's (share.rs): unframed,
//! a form token bound to the session and this fragment, its button armed
//! a moment after it shows, its POST from the platform's origin alone, so
//! no fragment's page (its author's code) can ask in its visitor's name and
//! learn whom they sign in as.

use fragment_core::form;
use fragment_proto::ErrorCode;
use serde::de::IgnoredAny;
use worker::*;

use super::{address_query, enc, esc, fragment_asked, platform_session, redirect, to_login};
use crate::ask_registry;
use crate::config::Config;
use crate::error::CellResult;
use crate::registry::asks::Asked;
use crate::registry::calls::By;
use crate::registry::invites::InvitesFor;
use crate::routed::Signed;
use crate::share;

/// The form purpose of a fragment's request page: a token for one
/// fragment's request asks for no other.
fn purpose(name: &str) -> String {
    format!("ask:{name}")
}

/// The way on to the fragment, through its sign-in (which shows an invite
/// first, and opens it for a member).
fn signin(cfg: &Config, url: &Url, name: &str, back: &str) -> String {
    format!("/auth/fragment?name={}&return={}{}", enc(name), enc(back), address_query(cfg, url))
}

/// `GET /auth/fragment/ask?name=&return=`: "You don't have access to X",
/// and Request access. Signed out: sign-in first, and back. An invite
/// waiting on their emails: its page instead. In it already: said so,
/// with the way in.
pub(super) async fn page(req: &Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    let (name, back) = fragment_asked(url)?;
    let here = format!("/auth/fragment/ask?name={}&return={}{}", enc(&name), enc(&back), address_query(cfg, url));
    let Some((token, live)) = platform_session(req, env, url).await? else { return to_login(&cfg.platform(), &here) };
    let waiting = ask_registry(env, &InvitesFor { by: By::Session(token.clone()), acting_for: None }).await?;
    if waiting["invites"].as_array().is_some_and(|l| l.iter().any(|i| i["fragment"] == name.as_str())) {
        return redirect(&signin(cfg, url, &name, &back), &[]);
    }
    let email = live.email.clone();
    let who = Signed::new(live.identity, None);
    match share::ask::<IgnoredAny>(env, url, &name, &who, Method::Get, "/signin/asking", None).await {
        Ok(_) => {}
        Err(e) if e.code == ErrorCode::Conflict => return in_already(cfg, url, &name, &back, &e.message),
        Err(e) => return Err(e),
    }
    let label = share::label(&name);
    let you = match email.as_deref() {
        Some(e) => esc(e),
        None => format!("<code>{}</code>", esc(&who.id)),
    };
    let body = format!(
        "<p>Ask its owner for <b>{l}</b>. They see your email, {you}, and can share it with you: you'll get an email if they do.</p>\
         <form method=\"post\" action=\"{action}\"><input type=\"hidden\" name=\"form\" value=\"{f}\">\
         <footer class=\"answer\"><button data-arm disabled>Request access</button></footer></form>",
        l = esc(label),
        action = esc(&here),
        f = esc(&form::issue(&token, &purpose(&name), crate::js::now_ms())),
    );
    share::sheet_page(200, &format!("You don't have access to {label}"), &body, true)
}

/// `POST /auth/fragment/ask?name=&return=`: the request, which the
/// fragment sends the registry as its owner's, then said so. Asked again
/// (or dismissed a while ago, which the requester is never told) it is
/// said the same way, as asked already; a request refused (too many today,
/// too many waiting) says why, with its status.
pub(super) async fn asked(req: &mut Request, env: &Env, cfg: &Config, url: &Url) -> CellResult<Response> {
    let (name, back) = fragment_asked(url)?;
    // the form's token bound to the session (`poster`), the fragment asks as the person it names
    let (_, live, _) = match share::poster(req, env, url, &cfg.platform(), &purpose(&name)).await? {
        Ok(posted) => posted,
        Err(page) => return Ok(page),
    };
    let who = Signed::new(live.identity, None);
    let label = esc(share::label(&name));
    match share::ask::<Asked>(env, url, &name, &who, Method::Post, "/signin/ask", None).await {
        Ok(Asked { again, email }) => {
            let (title, said) = match again {
                false => ("Request sent", format!("<p>We told the owner of <b>{label}</b> that you asked for it.</p>")),
                true => ("Asked already", format!("<p>You asked the owner of <b>{label}</b> for it already.</p>")),
            };
            let body = format!("{said}<p class=\"hint\">They see your email, {}. You'll get an email if they share it with you.</p>", esc(&email));
            share::sheet_page(200, title, &body, true)
        }
        Err(e) if e.code == ErrorCode::Conflict => in_already(cfg, url, &name, &back, &e.message),
        Err(e) if e.code.status() < 500 => share::sheet_page(e.code.status(), "Not asked", &format!("<p>{}</p>", esc(&e.message)), true),
        Err(e) => Err(e),
    }
}

/// Someone in it already (`why`, the fragment's words): the way in.
fn in_already(cfg: &Config, url: &Url, name: &str, back: &str, why: &str) -> CellResult<Response> {
    let label = share::label(name);
    let body = format!("<p>{}.</p><p><a href=\"{}\">Open {}</a></p>", esc(&capitalized(why)), esc(&signin(cfg, url, name, back)), esc(label));
    share::sheet_page(200, &format!("You're in {label}"), &body, false)
}

/// `why` with its first letter capitalized, as a sentence starts.
fn capitalized(why: &str) -> String {
    let mut chars = why.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}
