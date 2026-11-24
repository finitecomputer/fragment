//! A deployment's connection rows, by reference (FIN-509).
//!
//! A connection's catalog row is a file of the checkout,
//! `deploy/providers/<name>.jsonc`, and a deployment's `providers` names it
//! (`{"row": "<name>", …}`) beside what is the deployment's alone: the store
//! name of its OAuth client's secret (`key`) and the client's id
//! (`client_id`). So a row's hosts, scopes and names for people ship with
//! the release that reads them, a fix to one reaches every deployment, and
//! no deployment keeps a copy to drift.
//!
//! A row written in place (an operator key, a person's own key, or a
//! connection a deployment keeps for itself) is as it was.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

/// Where the rows are, under the checkout.
pub const DIR: &str = "deploy/providers";

/// A `providers` list with each reference (`{"row": "<name>", …}`) replaced
/// by its row, in place; every other entry is kept as it is.
pub fn resolved(providers: &[Value], root: &Path) -> Result<Vec<Value>> {
    providers.iter().map(|p| if p.get("row").is_some() { referenced(p, root) } else { Ok(p.clone()) }).collect()
}

/// One reference's row: its file's, with the reference's own fields beside
/// it.
///
/// - `key` and `client_id` are the deployment's two facts, both or neither.
///   With both, the row connects through the browser (`client_id` goes into
///   its `connection_oauth`). With neither, the row is offered without that
///   connect, its `connection_oauth` dropped: as dev and the e2e offer
///   Google, through the Pipes fake.
/// - Any other field replaces the row's (a deployment's own `note`, its
///   `accounts_max`), never its `name` or `kind`.
fn referenced(reference: &Value, root: &Path) -> Result<Value> {
    let fields = reference.as_object().context("providers: a reference to a row is an object")?;
    let name = reference["row"].as_str().filter(|n| name_ok(n)).ok_or_else(|| anyhow!("providers: a row is named by its file ({DIR}/<name>.jsonc), not {}", reference["row"]))?;
    let path = root.join(DIR).join(format!("{name}.jsonc"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("providers: the row {name} is {}", path.display()))?;
    let mut row: Value = serde_json::from_str(&crate::strip_comments(&text)).with_context(|| format!("parse {}", path.display()))?;
    let Some(o) = row.as_object_mut().filter(|o| o.get("name").and_then(Value::as_str) == Some(name)) else { bail!("{}: a row is an object, named as its file", path.display()) };
    match (fields.get("key"), fields.get("client_id")) {
        (Some(key), Some(client_id)) => {
            let Some(oauth) = o.get_mut("connection_oauth").and_then(Value::as_object_mut) else { bail!("providers: the row {name} has no connection_oauth for a client_id to go in") };
            oauth.insert("client_id".into(), client_id.clone());
            o.insert("key".into(), key.clone());
        }
        (None, None) => {
            o.remove("connection_oauth");
        }
        _ => bail!("providers: the row {name} names its key (its client secret's name in the store) and its client_id, or neither"),
    }
    for (field, value) in fields {
        match field.as_str() {
            "row" | "key" | "client_id" => {}
            "name" | "kind" => bail!("providers: the row {name} is a {DIR} file's: a reference gives it no other {field}"),
            _ => {
                o.insert(field.clone(), value.clone());
            }
        }
    }
    Ok(row)
}

/// A row's name, as a file's: nothing a path would read as more.
fn name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A checkout of this test's own, with one row.
    fn checkout(what: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("devstack-rows-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(DIR)).unwrap();
        let row = r#"// a comment, as a row's file has
{ "name": "linear", "kind": "connection", "title": "Linear", "hosts": ["api.linear.app"],
  "connection_oauth": { "authorize": "https://linear.app/oauth/authorize", "exchange": "https://api.linear.app/oauth/token", "access": { "write": { "scopes": ["write"] } } },
  "scope_names": { "write": "Create and change issues" }, "env": ["LINEAR_OAUTH_ACCESS_TOKEN"] }"#;
        std::fs::write(root.join(DIR).join("linear.jsonc"), row).unwrap();
        std::fs::write(root.join(DIR).join("misnamed.jsonc"), r#"{ "name": "other", "kind": "connection" }"#).unwrap();
        root
    }

    /// A reference is its row with the deployment's facts beside it; a row
    /// written in place is kept; and the list keeps its order.
    #[test]
    fn a_reference_is_its_row_with_the_deployments_own_beside_it() {
        let root = checkout("valid");
        let inline = json!({ "name": "x", "kind": "operator", "hosts": ["api.x.com"], "key": "fragment-x-api-key" });
        let listed = [inline.clone(), json!({ "row": "linear", "key": "fragment-linear-oauth-secret", "client_id": "abc", "note": "Ours.", "accounts_max": 3 })];
        let rows = resolved(&listed, &root).unwrap();
        assert_eq!(rows[0], inline, "a row in place, as it was");
        let linear = &rows[1];
        assert_eq!((&linear["name"], &linear["key"], &linear["connection_oauth"]["client_id"]), (&json!("linear"), &json!("fragment-linear-oauth-secret"), &json!("abc")));
        assert_eq!((&linear["note"], &linear["accounts_max"], &linear["title"]), (&json!("Ours."), &json!(3), &json!("Linear")), "its own fields over the file's, the file's kept");
        assert_eq!(linear["connection_oauth"]["exchange"], "https://api.linear.app/oauth/token");
        assert!(linear.get("row").is_none() && linear.get("client_id").is_none(), "a row, as the catalog reads one: {linear}");
        // neither fact: offered without its browser-bound connect
        let bare = resolved(&[json!({ "row": "linear" })], &root).unwrap();
        let bare = &bare[0];
        assert!(bare.get("connection_oauth").is_none() && bare.get("key").is_none() && bare["scope_names"]["write"] == "Create and change issues", "{bare}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What is refused, each naming the row: one of the two facts alone, a
    /// row that is no file, a name a path would read as more, a file named
    /// for another row, and a reference that renames its row.
    #[test]
    fn a_reference_that_is_not_one_is_refused() {
        let root = checkout("invalid");
        for (bad, why) in [
            (json!({ "row": "linear", "key": "k" }), "or neither"),
            (json!({ "row": "linear", "client_id": "c" }), "or neither"),
            (json!({ "row": "nope" }), "the row nope is"),
            (json!({ "row": "../example" }), "named by its file"),
            (json!({ "row": 7 }), "named by its file"),
            (json!({ "row": "misnamed" }), "named as its file"),
            (json!({ "row": "linear", "name": "other" }), "no other name"),
            (json!({ "row": "linear", "kind": "operator" }), "no other kind"),
        ] {
            let refused = resolved(std::slice::from_ref(&bad), &root).unwrap_err();
            assert!(format!("{refused:#}").contains(why), "{bad}: {refused:#}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
