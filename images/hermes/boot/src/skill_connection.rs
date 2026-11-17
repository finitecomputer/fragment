//! The connection a skill is for, as its frontmatter names it (`metadata:
//! fragment: connection: <provider>`, templates/skills/README.md): the
//! catalog row an agent must hold to use it (FIN-509). Shared by the image,
//! which shows a connection's skill to an agent that holds the connection,
//! and by the host tests that hold each connection's row and skills
//! together (`fragment-templates`' and `xtask`'s), so a skill is never read
//! two ways.

/// The provider a `SKILL.md` names its connection as; none for a skill of
/// no connection.
pub fn of(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let front = text.strip_prefix("---\n")?.split_once("\n---")?.0;
    let mut metadata = front.lines().skip_while(|l| l.trim_end() != "metadata:").skip(1).take_while(|l| l.starts_with(' '));
    metadata.by_ref().find(|l| l.trim_end() == "  fragment:")?;
    metadata.take_while(|l| l.starts_with("    ")).find_map(|l| l.trim().strip_prefix("connection:")).map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_skill_names_its_connection_under_metadata_fragment_and_nowhere_else() {
        let named = "---\nname: a\nmetadata:\n  hermes:\n    tags: [A]\n  fragment:\n    connection: linear\n---\n\n# a\n";
        assert_eq!(super::of(named), Some("linear".into()));
        assert_eq!(super::of(&format!("\u{feff}{}", named.replace('\n', "\r\n"))), Some("linear".into()), "a byte order mark and CRLF, as an editor may leave them");
        for none in ["---\nname: a\n---\n", "---\nname: a\nmetadata:\n  hermes:\n    connection: x\n---\n", "---\nname: a\nconnection: x\n---\n", "---\nname: a\nmetadata:\n  fragment:\n    connection:\n---\n", "no frontmatter"] {
            assert_eq!(super::of(none), None, "{none:?}");
        }
    }
}
