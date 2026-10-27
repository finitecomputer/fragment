//! An identity's face: what a page shows it by. An agent's is its agent
//! fragment's, at live: its title (`fragment.json`'s `meta.title`) and
//! its picture (`avatar.png`), which the cell reads at each install of
//! live (docs/api.md, "An agent's name and picture"). A person's picture
//! is the registry's. Both pictures are served at
//! `/api/identities/<npub>/picture` and take the same images.

use fragment_proto::limits;

/// Where an agent fragment keeps its picture: one path, whatever the
/// image's type, which its bytes tell.
pub const AGENT_PICTURE: &str = "avatar.png";

/// A picture's type from its first bytes (a request's content-type and a
/// path's extension are not trusted).
pub fn picture_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// Why a picture is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Its bytes, past `limits::PICTURE_MAX_BYTES`.
    TooLarge(u64),
    NotAnImage,
}

impl Refused {
    /// What its maker is told, `what` naming it (`avatar.png`).
    pub fn message(self, what: &str) -> String {
        match self {
            Refused::TooLarge(size) => format!(
                "{what} is {size} bytes; a picture is at most {} (256 KiB): make it smaller (512×512 or less)",
                limits::PICTURE_MAX_BYTES
            ),
            Refused::NotAnImage => format!("{what} is not a picture: a picture is a PNG, JPEG, WebP or GIF"),
        }
    }
}

/// A picture's type, or why it is refused: at most
/// `limits::PICTURE_MAX_BYTES`, and a PNG, JPEG, WebP or GIF by its bytes.
pub fn check_picture(bytes: &[u8]) -> Result<&'static str, Refused> {
    if bytes.len() > limits::PICTURE_MAX_BYTES {
        return Err(Refused::TooLarge(bytes.len() as u64));
    }
    picture_type(bytes).ok_or(Refused::NotAnImage)
}

/// An agent's name as its face keeps it: its title, trimmed, when it says
/// anything; none when it says nothing (no title, or only spaces), so every
/// reader falls back to the label alike, and none shows a blank.
pub fn title(title: Option<&str>) -> Option<&str> {
    title.map(str::trim).filter(|t| !t.is_empty())
}

/// What an agent is called in the platform's own words (the CLI's, and any
/// fragment's Open Graph title, `site::inject_og`, which its page reads):
/// its name (`title`), else its fragment's label, which `@mentions` it.
/// Never a word it was not given ("Agent").
pub fn agent_name<'a>(title: Option<&'a str>, fragment: &'a str) -> &'a str {
    self::title(title).unwrap_or_else(|| fragment_proto::split_fragment_name(fragment).map_or(fragment, |(label, _)| label))
}

/// An agent's job (`SOUL.md`) a rename rewrites, at most: bigger, or
/// bigger once rewritten, and it is left as it is (a job is a few lines).
pub const JOB_RENAME_MAX_BYTES: usize = 64 * 1024;
/// Where an agent's job lives in its fragment.
pub const JOB: &str = "SOUL.md";

/// The name the shell's first job gave its agent, from 2026-10-03 until
/// 2026-10-16, when a job stopped naming its agent: "You are <name>,
/// <owner>'s default agent: …" on its first line (the owner an email or a
/// username, with no ", "). None for any other first line: a name is read
/// only where that template wrote it, never guessed.
pub fn job_first_name(job: &str) -> Option<&str> {
    let line = job.lines().next()?.strip_prefix("You are ")?;
    let said = &line[..line.find("'s default agent: ")?];
    let name = &said[..said.rfind(", ")?];
    (!name.trim().is_empty() && name.chars().count() <= fragment_proto::limits::TITLE_MAX_CHARS).then_some(name)
}

/// An agent's job as its owner's rename leaves it (the shell's Rename and
/// `fragment agent --name`, in the commit that renames it; docs/api.md, "An
/// agent's name and picture"): each whole-word mention of one of `olds`
/// (case and all; a mention is whole when no letter, digit or `_` touches
/// it) said as `name` instead, in one pass, the longest old name first.
/// None when there is nothing to rewrite: no old name but `name` itself,
/// none mentioned, or a job past `JOB_RENAME_MAX_BYTES` before or after.
/// So asking again, with the name it has now, changes nothing.
pub fn job_renamed(job: &str, olds: &[&str], name: &str) -> Option<String> {
    let name = name.trim();
    let mut olds: Vec<&str> = olds.iter().map(|o| o.trim()).filter(|o| !o.is_empty() && *o != name).collect();
    if name.is_empty() || olds.is_empty() || job.len() > JOB_RENAME_MAX_BYTES {
        return None;
    }
    olds.sort_by_key(|o| std::cmp::Reverse(o.len()));
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let (mut out, mut at, mut changed) = (String::with_capacity(job.len()), 0, false);
    // bounded by the job: each pass takes at least one char of it
    while at < job.len() {
        let rest = &job[at..];
        let whole = |o: &str| rest.starts_with(o) && !word(job[..at].chars().next_back()) && !word(rest[o.len()..].chars().next());
        match olds.iter().copied().find(|o| whole(o)) {
            Some(o) => {
                out.push_str(name);
                at += o.len();
                changed = true;
            }
            None => {
                let c = rest.chars().next().expect("not at its end");
                out.push(c);
                at += c.len_utf8();
            }
        }
    }
    (changed && out.len() <= JOB_RENAME_MAX_BYTES).then_some(out)
}

/// Where an identity's picture is served, on the platform's origin: its
/// address names the picture's SHA-256 (its first 12 digits), so a new
/// picture is a new address.
pub fn picture_path(identity: &str, sha256: &str) -> String {
    assert!(sha256.len() >= 12 && sha256.is_char_boundary(12), "a picture is named by its SHA-256");
    format!("/api/identities/{identity}/picture?v={}", &sha256[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid: each of the four types, by its bytes alone, up to the limit
    /// itself. Invalid: one byte past it, text, an empty file, and a
    /// RIFF that is no WebP, each saying why in words its maker acts on.
    #[test]
    fn a_picture_is_one_of_four_images_of_at_most_256_kib() {
        let png = [&[0x89, b'P', b'N', b'G', 0x0D, 0x0A][..], &[0; 10]].concat();
        assert_eq!(check_picture(&png), Ok("image/png"));
        assert_eq!(check_picture(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Ok("image/jpeg"));
        assert_eq!(check_picture(b"GIF89a......"), Ok("image/gif"));
        assert_eq!(check_picture(b"RIFF\0\0\0\0WEBPVP8 "), Ok("image/webp"));
        let mut full = vec![0u8; limits::PICTURE_MAX_BYTES];
        full[..4].copy_from_slice(&[0x89, b'P', b'N', b'G']);
        assert_eq!(check_picture(&full), Ok("image/png"), "the limit itself is a picture");
        full.push(0);
        let over = check_picture(&full);
        assert_eq!(over, Err(Refused::TooLarge(limits::PICTURE_MAX_BYTES as u64 + 1)));
        let said = over.unwrap_err().message(AGENT_PICTURE);
        assert!(said.starts_with("avatar.png is 262145 bytes") && said.contains("262144"), "{said}");
        for bytes in [&b"hello, I am text"[..], b"", b"RIFF\0\0\0\0WAVEfmt "] {
            assert_eq!(check_picture(bytes), Err(Refused::NotAnImage), "{bytes:?}");
        }
        assert!(Refused::NotAnImage.message(AGENT_PICTURE).contains("PNG, JPEG, WebP or GIF"));
    }

    /// Valid: a title is its name, trimmed. Invalid: none, or one of only
    /// spaces, is no name: the label stands in, never a blank or a word it
    /// was not given.
    #[test]
    fn an_agent_is_called_by_its_title_else_its_label() {
        assert_eq!(title(Some("  Juniper ")), Some("Juniper"));
        assert_eq!(title(Some(" \t ")), None);
        assert_eq!(title(None), None);
        assert_eq!(agent_name(Some("Waffle"), "pulsar-360--k3x9"), "Waffle");
        assert_eq!(agent_name(Some("  "), "pulsar-360--k3x9"), "pulsar-360");
        assert_eq!(agent_name(None, "pulsar-360--k3x9"), "pulsar-360");
        assert_eq!(agent_name(None, "not a name"), "not a name", "what is no fragment's name is said as it is");
    }

    /// Goal (Paul, 2026-10-27: Skyler's XP, once Bliss, still said Bliss
    /// in "Its job"): a rename says the new name wherever the job said the
    /// old one as a word. Valid: each whole mention, the legacy first
    /// line's name too, longest first, in one pass. Invalid: a part of a
    /// word, another case, no old name, a job past its bound. Replay: the
    /// same rename again, its old name now the new one, changes nothing.
    #[test]
    fn a_rename_says_the_new_name_in_its_job() {
        let job = "You are Bliss, skyler@example.com's default agent: the first one they talk to.\nBliss keeps notes. Blissful, bliss_x and Bliss2 are not it; (Bliss) is.\n";
        assert_eq!(job_first_name(job), Some("Bliss"));
        let renamed = job_renamed(job, &["XP", "Bliss"], "XP").expect("rewritten");
        assert_eq!(renamed, "You are XP, skyler@example.com's default agent: the first one they talk to.\nXP keeps notes. Blissful, bliss_x and Bliss2 are not it; (XP) is.\n");
        // replay: the old name is the new one now, and the first line says it
        assert_eq!(job_first_name(&renamed), Some("XP"));
        assert_eq!(job_renamed(&renamed, &["XP", "XP"], "XP"), None, "asked again, nothing to rewrite");
        // a name with spaces, a longer old one first, the new one holding the old
        let two = "Waffle Bot and Waffle are friends.";
        assert_eq!(job_renamed(two, &["Waffle", "Waffle Bot"], "Waffle Bot II").as_deref(), Some("Waffle Bot II and Waffle Bot II are friends."));
        assert_eq!(job_renamed("Waffle Bots", &["Waffle Bot", "Waffle"], "X").as_deref(), Some("X Bots"), "a longer one not whole: the shorter");
        assert_eq!(job_renamed("$& and a.b (x)", &["a.b", "(x)"], "$1").as_deref(), Some("$& and $1 $1"), "said literally");
        assert_eq!(job_renamed("Ünï here", &["Ünï"], "Zoë").as_deref(), Some("Zoë here"));
        assert_eq!(job_renamed("aÜnï", &["Ünï"], "Zoë"), None, "a part of a word");
        // invalid: nothing to do
        assert_eq!(job_renamed(job, &[], "XP"), None);
        assert_eq!(job_renamed(job, &["  ", "Nova"], "Nova"), None, "no old name but the new one");
        assert_eq!(job_renamed("You keep the garden.\n", &["Bliss"], "XP"), None, "a job that names no one is left as it is");
        assert_eq!(job_renamed("BLISS and bliss", &["Bliss"], "XP"), None, "another case is another word");
        assert_eq!(job_renamed(&"Bliss ".repeat(JOB_RENAME_MAX_BYTES / 6 + 1), &["Bliss"], "XP"), None, "past its bound");
        assert_eq!(job_renamed(&"B ".repeat(JOB_RENAME_MAX_BYTES / 2 - 1), &["B"], &"N".repeat(100)), None, "past its bound once rewritten");
        // the legacy line only where the template wrote it
        for other in ["You keep the garden.", "Hello Bliss, skyler's default agent: x", "You are , x's default agent: y", "You are Bliss's default agent: y", ""] {
            assert_eq!(job_first_name(other), None, "{other:?}");
        }
        assert_eq!(job_first_name("You are Bliss, the Great, paul's default agent: x"), Some("Bliss, the Great"), "the owner holds no \", \"");
    }

    /// The address names the picture's hash, so a new one is a new address.
    #[test]
    fn a_pictures_address_names_its_hash() {
        let sha = "0123456789abcdef".repeat(4);
        assert_eq!(picture_path("npub1x", &sha), "/api/identities/npub1x/picture?v=0123456789ab");
        assert_ne!(picture_path("npub1x", &sha), picture_path("npub1x", &"f".repeat(64)));
    }
}
