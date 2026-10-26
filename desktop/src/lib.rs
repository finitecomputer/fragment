use nsh_core::prelude::*;

#[native_module("finite-computer/native")]
fn native(registry: &mut Registry) {
    registry.module::<Shell>();
}

/// Rules shared by the desktop shell's views.
#[module]
pub struct Shell;

#[module]
impl Shell {
    /// The Fragment deployment to use, defaulting to production.
    pub fn host() -> String {
        std::env::var("FRAGMENT_HOST").unwrap_or_else(|_| "https://finite.chat".to_owned())
    }

    /// The human-readable part of a fragment's address.
    pub fn label(name: String) -> String {
        label(&name)
    }

    /// An agent's stable color, shared with the web shell.
    pub fn agent_color(identity: Option<String>) -> String {
        const COLORS: [&str; 6] = ["#a88bea", "#62c8af", "#eda978", "#80afe9", "#dc91b6", "#b7c878"];
        match identity {
            None => COLORS[0].to_owned(),
            Some(identity) => COLORS[(fnv(&identity) % COLORS.len() as u32) as usize].to_owned(),
        }
    }

    /// An application's stable color, shared with the web shell.
    pub fn app_color(label: String) -> String {
        const COLORS: [&str; 6] = ["#b18a30", "#5d7bc3", "#c9625e", "#4f9a7a", "#8f88bc", "#c4743d"];
        COLORS[(fnv(&label) % COLORS.len() as u32) as usize].to_owned()
    }

    /// A chat's title from its first message, at most 48 characters.
    pub fn first_title(message: String) -> String {
        let words = message
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if words.chars().count() <= 48 {
            words
        } else {
            format!("{}…", words.chars().take(47).collect::<String>().trim_end())
        }
    }

    /// The bounded search hint; the complete prompt is still sent to a new chat.
    pub fn search_hint(prompt: String) -> String {
        let hint = prompt.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
        let mut end = hint.len();
        while end > 0 && !hint.is_char_boundary(end) {
            end -= 1;
        }
        while end > 256 {
            end -= 1;
            while !hint.is_char_boundary(end) {
                end -= 1;
            }
        }
        hint[..end].to_owned()
    }

    /// A short, unique label for an agent or its chat.
    pub fn free_label(base: String, taken: Vec<String>) -> String {
        let base = slug(&base);
        let taken = taken.into_iter().collect::<std::collections::HashSet<_>>();
        for number in 1.. {
            let candidate = if number == 1 { base.clone() } else { format!("{base}-{number}") };
            if !taken.contains(&candidate) && !taken.contains(&format!("{candidate}-chat")) {
                return candidate;
            }
        }
        unreachable!()
    }
}

fn fnv(value: &str) -> u32 {
    value.encode_utf16().fold(0x811c9dc5, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x01000193)
    })
}

fn label(name: &str) -> String {
    let Some((prefix, suffix)) = name.rsplit_once("--") else {
        return name.to_owned();
    };
    if suffix.len() == 4
        && suffix
            .chars()
            .all(|c| matches!(c, 'a'..='k' | 'm'..='n' | 'p'..='z' | '2'..='9'))
    {
        prefix.to_owned()
    } else {
        name.to_owned()
    }
}

fn slug(value: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for c in value.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(c);
            dash = false;
        } else {
            dash = true;
        }
        if slug.len() == 31 {
            break;
        }
    }
    slug.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_only_remove_a_real_suffix() {
        assert_eq!(label("maple-chat--k3x9"), "maple-chat");
        assert_eq!(label("maple-chat"), "maple-chat");
        assert_eq!(label("odd--0000"), "odd--0000");
    }

    #[test]
    fn colors_match_the_shells_fnv_rule() {
        assert_eq!(Shell::agent_color(None), "#a88bea");
        assert_eq!(Shell::agent_color(Some("person:abc".to_owned())), Shell::agent_color(Some("person:abc".to_owned())));
    }

    #[test]
    fn first_titles_are_one_bounded_line() {
        assert_eq!(Shell::first_title("\n  Plan\tour trip\nnext".to_owned()), "Plan our trip");
        assert_eq!(Shell::first_title("x".repeat(60)).chars().count(), 48);
    }

    #[test]
    fn search_hints_keep_whole_utf8() {
        assert_eq!(Shell::search_hint("one two three four five six seven eight nine".to_owned()), "one two three four five six seven eight");
        assert!(Shell::search_hint("🦀".repeat(200)).len() <= 256);
    }

    #[test]
    fn labels_avoid_the_agent_and_its_chat() {
        assert_eq!(Shell::free_label("My Agent!".to_owned(), vec!["my-agent".to_owned()]), "my-agent-2");
    }
}
