//! A private home's identity door, faked over HTTP (docs/api.md, "A private
//! agent"): the one route a deployment's admin asks it, `POST
//! /passkeys/invites` (`fragment_core::private_home`), answered as the
//! passkey door answers it (crates/identity-door on the Nitro line,
//! `passkeys::asked_invite`). Its URL is its door's public URL.
//!
//! An ask is let in only when signed by its inviter (NIP-98, for
//! `<url>/passkeys/invites` and its body), fresh, and not seen before:
//! else 401. Its body is `{"email": …}` alone (else 400). A new email
//! gets an invite, `<url>/invite/<64 hex>`, good for an hour; an email the
//! door holds an account for (`account`) gets `signIn`, `<url>/`, and no
//! invite. Lever: the next ask is refused (`fail_next`). What it was asked,
//! and what it made: `asked`, `invited`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

/// How far an ask's signing may be from the clock, as the door's.
pub const WINDOW_S: i64 = 60;

#[derive(Default)]
struct State {
    /// Emails with an account here.
    accounts: Vec<String>,
    /// Each ask let in, its email.
    asked: Vec<String>,
    /// Each invite made, its email.
    invited: Vec<String>,
    /// The asks answered, their headers: a replay is refused.
    seen: Vec<String>,
    fail_next: Option<(u16, String)>,
}

pub struct PrivateDoor {
    pub url: String,
    state: Arc<Mutex<State>>,
    _server: Server,
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("after the epoch").as_secs() as i64
}

fn refused(status: u16, why: &str) -> Response {
    Response::json(status, &json!({ "error": why }))
}

impl PrivateDoor {
    /// A door that trusts `inviter` (64 hex) alone.
    pub fn start(inviter: &str) -> std::io::Result<PrivateDoor> {
        let state = Arc::new(Mutex::new(State::default()));
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let (st, inviter, door) = (Arc::clone(&state), inviter.to_string(), url.clone());
        let handler: Handler = Arc::new(move |req: &Request| answer(&st, &inviter, &door, req, now_s()));
        let server = Server::serve(listener, handler)?;
        Ok(PrivateDoor { url, state, _server: server })
    }

    /// `email` has an account here: an ask for it gets "sign in".
    pub fn account(&self, email: &str) {
        self.state.lock().expect("private door state").accounts.push(email.to_ascii_lowercase());
    }

    /// The next ask is refused with `status` and `why`.
    pub fn fail_next(&self, status: u16, why: &str) {
        self.state.lock().expect("private door state").fail_next = Some((status, why.to_string()));
    }

    /// The emails of the asks let in, oldest first.
    pub fn asked(&self) -> Vec<String> {
        self.state.lock().expect("private door state").asked.clone()
    }

    /// The emails invites were made for, oldest first.
    pub fn invited(&self) -> Vec<String> {
        self.state.lock().expect("private door state").invited.clone()
    }
}

fn answer(state: &Mutex<State>, inviter: &str, door: &str, req: &Request, now_s: i64) -> Response {
    if (req.method.as_str(), req.path.as_str()) != ("POST", "/passkeys/invites") {
        return refused(404, "not_found");
    }
    let mut s = state.lock().expect("private door state");
    if let Some((status, why)) = s.fail_next.take() {
        return refused(status, &why);
    }
    let header = req.header("authorization");
    match fragment_nip98::verify(header, "POST", &format!("{door}/passkeys/invites"), &req.body, now_s, WINDOW_S) {
        Ok(signer) if signer == inviter => {}
        Ok(_) => return refused(401, "not signed by this door's inviter"),
        Err(e) => return refused(401, &e.to_string()),
    }
    let header = header.expect("a verified ask is signed").to_string();
    if s.seen.contains(&header) {
        return refused(401, "this ask was answered already");
    }
    s.seen.push(header);
    let email = match serde_json::from_slice::<Value>(&req.body) {
        Ok(Value::Object(o)) if o.len() == 1 => o.get("email").and_then(Value::as_str).map(str::to_ascii_lowercase),
        _ => None,
    };
    let Some(email) = email.filter(|e| e.contains('@')) else { return refused(400, "an ask is {\"email\": …}") };
    s.asked.push(email.clone());
    if s.accounts.contains(&email) {
        return Response::json(200, &json!({ "signIn": format!("{door}/") }));
    }
    s.invited.push(email);
    // a token of its own each time (the real one's are random)
    let token = format!("{:064x}", s.invited.len());
    Response::json(200, &json!({ "invite": format!("{door}/invite/{token}") }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_nip98::Keys;

    const DOOR: &str = "http://127.0.0.1:1";

    fn ask(by: &Keys, body: &str, at: i64) -> Request {
        let h = by.header("POST", &format!("{DOOR}/passkeys/invites"), body.as_bytes(), at);
        Request::new("POST", "/passkeys/invites", &[("authorization", h.as_str()), ("content-type", "application/json")], body.as_bytes())
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    // Goal: the inviter's fresh, first ask gets a new email's invite under
    // the door, or "sign in" for an email with an account (no invite made);
    // another key's, a stale one, a replay and a body that is not an email
    // alone are refused; the lever refuses the next.
    #[test]
    fn it_answers_as_the_door() {
        let (inviter, stranger) = (Keys::generate(), Keys::generate());
        let state = Mutex::new(State::default());
        let now = 1_800_000_000;
        let new = ask(&inviter, r#"{"email":"Ada@Example.com"}"#, now);
        let r = answer(&state, inviter.pubkey_hex(), DOOR, &new, now);
        assert_eq!(r.status, 200);
        let link = json_of(&r)["invite"].as_str().unwrap().to_string();
        assert!(link.starts_with("http://127.0.0.1:1/invite/") && link.len() == DOOR.len() + 8 + 64, "{link}");
        assert_eq!(answer(&state, inviter.pubkey_hex(), DOOR, &new, now).status, 401, "a replay");
        state.lock().unwrap().accounts.push("bo@example.com".into());
        let r = answer(&state, inviter.pubkey_hex(), DOOR, &ask(&inviter, r#"{"email":"bo@example.com"}"#, now), now);
        assert_eq!(json_of(&r), json!({ "signIn": "http://127.0.0.1:1/" }));
        assert_eq!(answer(&state, inviter.pubkey_hex(), DOOR, &ask(&stranger, r#"{"email":"cy@example.com"}"#, now), now).status, 401);
        assert_eq!(answer(&state, inviter.pubkey_hex(), DOOR, &ask(&inviter, r#"{"email":"cy@example.com"}"#, now - 2 * WINDOW_S), now).status, 401, "stale");
        assert_eq!(answer(&state, inviter.pubkey_hex(), DOOR, &ask(&inviter, r#"{"email":"cy@example.com","more":1}"#, now), now).status, 400);
        state.lock().unwrap().fail_next = Some((503, "down".into()));
        assert_eq!(answer(&state, inviter.pubkey_hex(), DOOR, &ask(&inviter, r#"{"email":"cy@example.com"}"#, now), now).status, 503);
        let s = state.lock().unwrap();
        assert_eq!((s.asked.clone(), s.invited.clone()), (vec!["ada@example.com".to_string(), "bo@example.com".into()], vec!["ada@example.com".to_string()]));
    }
}
