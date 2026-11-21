//! The deployment's private home (docs/api.md, "A private agent"): a
//! Fragment in a confidential sandbox of its own, whose identity door
//! starts a new account there when this deployment's admin asks, for the
//! operator signed in here. What the ask is, and how the door's answer is
//! held to the door, so a door that answers anything else sends no one
//! anywhere.

use fragment_proto::PrivateAgent;
use serde::{Deserialize, Serialize};
use url::Url;

/// Where the door takes an ask, under its public URL (the identity door's
/// `passkeys::INVITES_PATH`).
pub const INVITES_PATH: &str = "/passkeys/invites";
/// The door's answer, read at most: a link and a word or two.
pub const ANSWER_BYTES_MAX: usize = 4 * 1024;
/// A door's URL at most.
pub const DOOR_BYTES_MAX: usize = 2048;
/// The door's own words in a refusal, passed on at most.
const WHY_CHARS_MAX: usize = 200;
/// An invite's token: 64 hex digits.
const TOKEN_HEX: usize = 64;

/// Whether `door` names a home's door: https (http on loopback alone, for
/// dev and the e2e), a host, no credentials, query or fragment, no
/// trailing slash (its paths are put after it), and written as a URL
/// writes itself (a lower-case host, no default port), since the door's
/// own links are compared with it as text.
pub fn door_ok(door: &str) -> bool {
    let Ok(u) = Url::parse(door) else { return false };
    let loopback = matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let scheme = u.scheme() == "https" || (u.scheme() == "http" && loopback);
    let canonical = u.as_str() == door || (u.path() == "/" && u.as_str() == format!("{door}/"));
    scheme && canonical && u.host_str().is_some() && u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none() && !door.ends_with('/') && door.len() <= DOOR_BYTES_MAX
}

/// The ask's URL, as it is signed (NIP-98's `u`).
pub fn ask_url(door: &str) -> String {
    assert!(door_ok(door), "a door is checked before it is asked");
    format!("{door}{INVITES_PATH}")
}

/// The ask's body: the operator's verified email, and a nonce of its own
/// (16 random bytes, hex). NIP-98 signs with no randomness and to the
/// second, so two asks in one second would otherwise be one event, and the
/// door answers an event once: the nonce keeps each ask its own, and an
/// event seen twice always a replay.
pub fn ask_body(email: &str, nonce: &str) -> String {
    assert!(nonce.len() == 32 && nonce.bytes().all(|b| b.is_ascii_hexdigit()), "a nonce is 16 bytes, hex");
    #[derive(Serialize)]
    struct Ask<'a> {
        email: &'a str,
        nonce: &'a str,
    }
    serde_json::to_string(&Ask { email, nonce }).expect("strings serialize")
}

/// What the door answers, each field its own case.
#[derive(Deserialize)]
struct DoorAnswer {
    #[serde(default)]
    invite: Option<String>,
    #[serde(default, rename = "signIn")]
    sign_in: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// What the door answered (`status`, its body: JSON, read whole), held to `door`: a
/// new account's one-time invite under the door's own URL (`invite`), or,
/// for an email with an account there, the home on the door's own origin
/// to sign in at (`signIn`). Anything else is the door's failure, said with
/// its own words where it gave them.
pub fn answered(door: &str, status: u16, body: &[u8]) -> Result<PrivateAgent, String> {
    let at = Url::parse(door).map_err(|_| "the private home's door is not a URL".to_string())?;
    let said: Option<DoorAnswer> = serde_json::from_slice(body).ok();
    if status != 200 {
        let why: String = said.and_then(|s| s.error).as_deref().unwrap_or("no reason given").chars().take(WHY_CHARS_MAX).collect();
        return Err(format!("the private home refused ({status}): {why}"));
    }
    let said = said.ok_or("the private home's answer is not its JSON")?;
    if let Some(invite) = said.invite.as_deref() {
        let token = invite.strip_prefix(&format!("{door}/invite/")).ok_or("the private home's invite is not at its door")?;
        if token.len() != TOKEN_HEX || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("the private home's invite is not a link of its door's".into());
        }
        return Ok(PrivateAgent { url: invite.to_string(), invited: true });
    }
    if let Some(home) = said.sign_in.as_deref() {
        let to = Url::parse(home).map_err(|_| "the private home's sign-in is not a URL".to_string())?;
        if to.scheme() != at.scheme() || to.origin() != at.origin() || to.query().is_some() || to.fragment().is_some() {
            return Err("the private home's sign-in is not on its door's origin".into());
        }
        return Ok(PrivateAgent { url: to.to_string(), invited: false });
    }
    Err("the private home answered neither an invite nor a sign-in".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// `answered` of a JSON value's bytes.
    fn of(door: &str, status: u16, v: &Value) -> Result<PrivateAgent, String> {
        answered(door, status, v.to_string().as_bytes())
    }

    const DOOR: &str = "https://home.example.com/__door";

    // Goal: a door is https (http on loopback alone), with no credentials,
    // query, fragment or trailing slash.
    #[test]
    fn doors_are_checked() {
        for ok in [DOOR, "https://home.example.com", "http://127.0.0.1:8803", "http://localhost:9000/__door"] {
            assert!(door_ok(ok), "{ok}");
        }
        for bad in ["", "home.example.com", "http://home.example.com/__door", "https://home.example.com/__door/", "https://u:p@home.example.com", "https://home.example.com/?x=1", "https://home.example.com/#f", "ftp://home.example.com", "https://Home.Example.com/__door", "https://home.example.com:443/__door"] {
            assert!(!door_ok(bad), "{bad}");
        }
        assert_eq!(ask_url(DOOR), "https://home.example.com/__door/passkeys/invites");
        assert_eq!(ask_body("ada@example.com", &"0f".repeat(16)), format!(r#"{{"email":"ada@example.com","nonce":"{}"}}"#, "0f".repeat(16)));
    }

    // Goal: an invite under the door's own URL, or a sign-in on its origin,
    // is where the browser goes; anything else is refused, the door's words
    // passed on (and cut) when it refused.
    #[test]
    fn answers_are_held_to_the_door() {
        let token = "ab".repeat(32);
        let invite = format!("{DOOR}/invite/{token}");
        assert_eq!(of(DOOR, 200, &json!({ "invite": invite })), Ok(PrivateAgent { url: invite.clone(), invited: true }));
        assert_eq!(of(DOOR, 200, &json!({ "signIn": "https://home.example.com/" })), Ok(PrivateAgent { url: "https://home.example.com/".into(), invited: false }));
        for bad in [
            json!({ "invite": format!("https://evil.example/__door/invite/{token}") }),
            json!({ "invite": format!("{DOOR}/invite/{token}?next=https://evil.example") }),
            json!({ "invite": format!("{DOOR}/invite/xyz") }),
            json!({ "signIn": "https://evil.example/" }),
            json!({ "signIn": "https://home.example.com/?next=x" }),
            json!({ "signIn": "javascript:alert(1)" }),
            json!({ "signIn": "blob:https://home.example.com/0f0e" }),
            json!({}),
            Value::Null,
        ] {
            assert!(of(DOOR, 200, &bad).is_err(), "{bad}");
        }
        let refused = of(DOOR, 401, &json!({ "error": "not signed by this door's inviter" })).unwrap_err();
        assert!(refused.contains("401") && refused.contains("inviter"), "{refused}");
        let long = of(DOOR, 400, &json!({ "error": "x".repeat(5000) })).unwrap_err();
        assert!(long.len() < 300, "cut");
        assert!(of(DOOR, 502, &Value::Null).unwrap_err().contains("no reason given"));
        assert!(answered(DOOR, 200, b"<html>").is_err() && answered(DOOR, 502, b"").unwrap_err().contains("no reason given"), "not JSON");
    }
}
