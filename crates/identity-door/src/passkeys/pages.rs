//! The passkey door's pages: sign in, enroll from an invite, and manage
//! (add a passkey, recover with a code). Each is one small HTML page and
//! the door's own script (door.js), from the door's own origin, nothing
//! loaded from anywhere else; every value in them is escaped.

pub const SCRIPT: &str = include_str!("door.js");

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;margin:0;background:#fafaf9;color:#1c1917}\
main{max-width:26rem;margin:12vh auto;padding:0 16px}h1{font-size:1.4rem;margin:0 0 1rem}\
form{display:grid;gap:.6rem;margin:0 0 1.6rem}input{font:inherit;padding:.55rem .7rem;border:1px solid #d6d3d1;border-radius:8px}\
button{font:inherit;padding:.6rem;border:0;border-radius:8px;background:#1c1917;color:#fafaf9;cursor:pointer}\
button:disabled{opacity:.5}.bad{color:#b91c1c}pre{background:#f5f5f4;padding:.8rem;border-radius:8px}\
@media (prefers-color-scheme:dark){body{background:#1c1917;color:#f5f5f4}input{background:#292524;color:inherit;border-color:#44403c}\
button{background:#f5f5f4;color:#1c1917}pre{background:#292524}}";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn page(title: &str, data: &[(&str, &str)], body: &str) -> String {
    let attrs: String = data.iter().map(|(k, v)| format!(" data-{k}=\"{}\"", esc(v))).collect();
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{t}</title><style>{STYLE}</style><main{attrs}><h1>{t}</h1>{body}<p id=\"said\" role=\"status\"></p></main><script src=\"{base}/passkeys.js\"></script></html>",
        t = esc(title),
        base = esc(data.iter().find(|(k, _)| *k == "base").map(|(_, v)| *v).unwrap_or("")),
    )
}

/// The sign-in: an email, then that account's passkey.
pub fn signin(base: &str, sealed: &str, hint: Option<&str>) -> String {
    let hint = esc(hint.unwrap_or(""));
    page(
        "Sign in",
        &[("base", base), ("state", sealed)],
        &format!(
            "<form id=\"signin\"><input name=\"email\" type=\"email\" autocomplete=\"username webauthn\" placeholder=\"Email\" value=\"{hint}\" required>\
<button>Sign in with your passkey</button></form><p><a href=\"{}/passkeys\">Add a passkey, or use a recovery code</a></p>",
            esc(base)
        ),
    )
}

/// An invite's page: make the account's first passkey.
pub fn enroll(base: &str, invite: &str, email: &str) -> String {
    page(
        "Make your passkey",
        &[("base", base), ("invite", invite)],
        &format!(
            "<form id=\"enroll\"><p>For {}.</p><button>Make a passkey</button></form>\
<section id=\"codes\" hidden><p>Your recovery codes. Keep them somewhere safe: each one makes a new passkey once if you lose yours. They are not shown again.</p><pre></pre><p><a>Continue</a></p></section>",
            esc(email)
        ),
    )
}

/// Another passkey (approved by one of the account's own), or a new one by
/// a recovery code.
pub fn manage(base: &str) -> String {
    page(
        "Passkeys",
        &[("base", base)],
        "<form id=\"add\"><input name=\"email\" type=\"email\" autocomplete=\"username webauthn\" placeholder=\"Email\" required>\
<button>Add a passkey (use one you have first)</button></form>\
<form id=\"recover\"><input name=\"email\" type=\"email\" autocomplete=\"username\" placeholder=\"Email\" required>\
<input name=\"code\" autocomplete=\"off\" placeholder=\"Recovery code\" required><button>Make a new passkey with a recovery code</button></form>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Goal: every value a page carries is escaped; the script is the
    // door's own, from its base.
    #[test]
    fn pages_escape_what_they_carry() {
        let p = signin("/__door", "a\"b<c", Some("x\"><script>"));
        assert!(p.contains("data-state=\"a&quot;b&lt;c\"") && !p.contains("<script>alert") && !p.contains("x\"><script>"));
        assert!(p.contains("<script src=\"/__door/passkeys.js\"></script>"));
        assert_eq!(p.matches("<script").count(), 1, "one script, the door's own");
        let e = enroll("/__door", "tok", "a<b@example.com");
        assert!(e.contains("a&lt;b@example.com") && e.contains("data-invite=\"tok\""));
        assert!(!manage("/__door").contains("http"), "nothing loaded from elsewhere");
    }
}
