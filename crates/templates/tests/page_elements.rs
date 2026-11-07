//! Goal: a page that runs the chat's script (the Telegram connector's
//! links `chat.js`, with a page of its own around it) holds every element
//! the chat's page does, so an element the chat gains is never one its
//! script finds missing there (#1075 added the history's Load earlier
//! messages, its status and Try again, and the Telegram page did not start).
//! Method: every `id` of the chat's page is one of the Telegram page's.

use std::collections::BTreeSet;
use std::path::Path;

fn ids(path: &Path) -> BTreeSet<String> {
    let html = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    html.split("id=\"").skip(1).filter_map(|rest| rest.split('"').next()).map(str::to_string).collect()
}

#[test]
fn the_telegram_page_holds_every_element_of_the_chats() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates");
    let chat = ids(&root.join("chat/site/index.html"));
    let telegram = ids(&root.join("telegram/site/index.html"));
    let missing: Vec<&String> = chat.difference(&telegram).collect();
    assert!(chat.len() > 10, "the chat's page has its elements: {chat:?}");
    assert!(missing.is_empty(), "templates/telegram/site/index.html lacks the chat page's {missing:?}");
}
