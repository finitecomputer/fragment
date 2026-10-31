//! A corpus as the brain's search sees it: its markdown pages, cut into
//! sections by the rules of templates/brain/applib/sections.mjs (ported
//! here line for line), so every engine indexes the very same sections.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::LazyLock;

pub const SECTION_MAX_CHARS: usize = 8000;
pub const SECTIONS_MAX: usize = 500;
pub const TITLE_MAX_CHARS: usize = 300;
/// A page this large is a blob to the brain, never read (app.mjs).
pub const PAGE_MAX_BYTES: usize = 1024 * 1024;
/// Pages read from one corpus, at most.
pub const PAGES_MAX: usize = 100_000;

#[derive(Debug, Clone)]
pub struct Page {
    pub path: String,
    pub text: String,
}

/// Pages, each with its sections.
pub type Sectioned = Vec<(Page, Vec<Section>)>;

#[derive(Debug, Clone)]
pub struct Section {
    /// `<path>#<n>`: the page's path and the section's place in it.
    pub id: String,
    pub path: String,
    pub title: String,
    pub ancestry: Vec<String>,
    pub heading: Option<String>,
    pub body: String,
}

impl Section {
    /// What an embedding model reads for this section: its title, its
    /// headings, then its text (the heading or title when it has none),
    /// as Finite Brain's semantic index gives them to its provider.
    pub fn embedding_input(&self) -> String {
        let mut trail = self.ancestry.clone();
        trail.extend(self.heading.clone());
        let text = if !self.body.trim().is_empty() {
            self.body.as_str()
        } else {
            self.heading.as_deref().unwrap_or(&self.title)
        };
        if trail.is_empty() {
            format!("{}\n\n{text}", self.title)
        } else {
            format!("{}\n{}\n\n{text}", self.title, trail.join(" > "))
        }
    }
}

/// The fragment's machinery and a wiki's instructions, which the brain
/// does not search (app.mjs's `hidden` and `indexable`).
pub fn indexable(path: &str) -> bool {
    let lower = path.to_lowercase();
    let md = lower.ends_with(".md") || lower.ends_with(".markdown");
    let hidden = path == "fragment.json"
        || path == "app.mjs"
        || path.starts_with("site/")
        || path.starts_with("applib/")
        || path.starts_with('.')
        || path.contains("/.")
        || path.contains(".conflict-");
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    md && !hidden && name != "agents.md" && name != "_index.md"
}

/// Every indexable page under `root`, by path. Pages of PAGE_MAX_BYTES or
/// more are left out, as the brain leaves them; `skipped` counts them.
pub fn load(root: &Path) -> Result<(Vec<Page>, usize), String> {
    let mut pages = Vec::new();
    let mut skipped = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let kind = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(root).expect("under the root").to_string_lossy().replace('\\', "/");
            if !indexable(&rel) {
                continue;
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| format!("{rel}: {e}"))?;
            if bytes.len() >= PAGE_MAX_BYTES {
                skipped += 1;
                continue;
            }
            pages.push(Page { path: rel, text: String::from_utf8_lossy(&bytes).into_owned() });
            if pages.len() > PAGES_MAX {
                return Err(format!("more than {PAGES_MAX} pages under {}", root.display()));
            }
        }
    }
    pages.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((pages, skipped))
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").unwrap());
static ATX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$").unwrap());
static CLOSING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|[ \t])#+$").unwrap());
static FRONTMATTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^---[ \t]*\r?\n([\s\S]*?)\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)").unwrap());
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+-\s+(.*)$").unwrap());
static KV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z_][\w-]*):\s*(.*)$").unwrap());
static INLINE_LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[(.*)\]$").unwrap());

/// Markdown's inline marks, gone: what a heading or a title reads as.
pub fn plain(text: &str) -> String {
    static WIKI_ALIAS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]|]+)\|([^\]]+)\]\]").unwrap());
    static WIKI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap());
    static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_`~]+").unwrap());
    static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let t = WIKI_ALIAS.replace_all(text, "$2");
    let t = WIKI.replace_all(&t, "$1");
    let t = LINK.replace_all(&t, "$1");
    let t = MARKS.replace_all(&t, "");
    SPACE.replace_all(&t, " ").trim().to_owned()
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].trim().to_owned();
        }
    }
    v.to_owned()
}

#[derive(Default)]
struct Meta {
    title: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
}

fn frontmatter(text: &str) -> (Meta, &str) {
    let mut meta = Meta::default();
    let Some(m) = FRONTMATTER.captures(text) else { return (meta, text) };
    let mut in_tags = false;
    for line in m[1].split('\n').map(|l| l.trim_end_matches('\r')) {
        if let Some(item) = ITEM.captures(line) {
            if in_tags {
                meta.tags.push(unquote(&item[1]));
                continue;
            }
        }
        in_tags = false;
        let Some(kv) = KV.captures(line) else { continue };
        let value = kv[2].to_owned();
        match &kv[1] {
            "tags" => {
                let v = value.trim();
                meta.tags = match INLINE_LIST.captures(v) {
                    Some(inline) => inline[1].split(',').map(unquote).filter(|s| !s.is_empty()).collect(),
                    None if !v.is_empty() => v.split(',').map(unquote).filter(|s| !s.is_empty()).collect(),
                    None => Vec::new(),
                };
                in_tags = v.is_empty();
            }
            "title" => meta.title = Some(unquote(&value)),
            "summary" => meta.summary = Some(unquote(&value)),
            "description" => meta.description = Some(unquote(&value)),
            _ => {}
        }
    }
    (meta, &text[m.get(0).unwrap().end()..])
}

fn heading(line: &str) -> Option<(usize, String)> {
    let m = ATX.captures(line)?;
    let raw = m.get(2).map_or("", |g| g.as_str());
    let text = plain(&CLOSING.replace(raw, ""));
    (!text.is_empty()).then(|| (m[1].len(), text))
}

/// A body too long for one section, cut at paragraphs (or hard, when a
/// paragraph alone is longer). Lengths are in UTF-16 units, as in JS.
fn split(body: &str) -> Vec<String> {
    let text = body.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= SECTION_MAX_CHARS {
        return vec![text.to_owned()];
    }
    let mut parts = Vec::new();
    let mut rest = units;
    while rest.len() > SECTION_MAX_CHARS {
        // JS's rest.lastIndexOf("\n\n", SECTION_MAX_CHARS)
        let nl = b'\n' as u16;
        let mut cut = (0..=SECTION_MAX_CHARS.min(rest.len() - 2)).rev().find(|&i| rest[i] == nl && rest[i + 1] == nl).unwrap_or(0);
        if cut < SECTION_MAX_CHARS / 2 {
            cut = SECTION_MAX_CHARS;
        }
        parts.push(String::from_utf16_lossy(&rest[..cut]).trim().to_owned());
        rest = String::from_utf16_lossy(&rest[cut..]).trim().encode_utf16().collect();
    }
    if !rest.is_empty() {
        parts.push(String::from_utf16_lossy(&rest));
    }
    parts
}

/// The file name a path reads as, when its page has no title.
pub fn stem(path: &str) -> String {
    static EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(md|markdown)$").unwrap());
    static DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[-_]+").unwrap());
    let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(path);
    let s = DASH.replace_all(&EXT.replace(name, ""), " ").trim().to_owned();
    if s.is_empty() { path.to_owned() } else { s }
}

/// `page`'s sections, and whether it had more than SECTIONS_MAX.
pub fn sections(page: &Page) -> (Vec<Section>, bool) {
    let text = page.text.strip_prefix('\u{feff}').unwrap_or(&page.text);
    let (meta, body) = frontmatter(text);
    let lines: Vec<&str> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut heads: Vec<(usize, usize, String)> = Vec::new();
    let mut fence: Option<char> = None;
    for (at, line) in lines.iter().enumerate() {
        if let Some(f) = FENCE.captures(line) {
            let c = f[1].chars().next().unwrap();
            match fence {
                None => fence = Some(c),
                Some(open) if open == c => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some((level, text)) = heading(line) {
            heads.push((at, level, text));
        }
    }
    let h1 = heads.iter().find(|h| h.1 == 1).map(|h| h.2.clone());
    let fm_title = plain(meta.title.as_deref().unwrap_or(""));
    let title: String = if !fm_title.is_empty() { fm_title } else { h1.unwrap_or_else(|| stem(&page.path)) };
    let title: String = String::from_utf16_lossy(&title.encode_utf16().take(TITLE_MAX_CHARS).collect::<Vec<_>>());
    let mut out: Vec<(Option<String>, Vec<String>, String)> = Vec::new();
    let mut push = |head: Option<String>, ancestry: Vec<String>, text: String| {
        let mut parts = split(&text);
        if parts.is_empty() && head.is_some() {
            parts.push(String::new());
        }
        for part in parts {
            out.push((head.clone(), ancestry.clone(), part));
        }
    };
    let mut lead: Vec<String> = [meta.summary.clone(), meta.description.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    if !meta.tags.is_empty() {
        lead.push(meta.tags.join(", "));
    }
    let first = heads.first().map_or(lines.len(), |h| h.0);
    lead.push(lines[..first].join("\n"));
    push(None, Vec::new(), lead.join("\n\n"));
    let mut above: Vec<Option<String>> = Vec::new();
    for (i, (at, level, text)) in heads.iter().enumerate() {
        above.resize(level - 1, None);
        let ancestry: Vec<String> = above.iter().flatten().cloned().collect();
        above.push(Some(text.clone()));
        let end = heads.get(i + 1).map_or(lines.len(), |h| h.0);
        push(Some(text.clone()), ancestry, lines[at + 1..end].join("\n"));
    }
    let truncated = out.len() > SECTIONS_MAX;
    let sections = out
        .into_iter()
        .take(SECTIONS_MAX)
        .enumerate()
        .map(|(n, (heading, ancestry, body))| Section {
            id: format!("{}#{n}", page.path),
            path: page.path.clone(),
            title: title.clone(),
            ancestry,
            heading,
            body,
        })
        .collect();
    (sections, truncated)
}

/// A query as words: runs of letters, digits and marks, lowercased
/// (sections.mjs's `words`, but for its NFKC step, which ASCII questions
/// do not need).
pub fn words(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || is_mark(c)))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str, text: &str) -> Page {
        Page { path: path.into(), text: text.into() }
    }

    #[test]
    fn sections_follow_headings_and_frontmatter() {
        let p = page(
            "eng/release.md",
            "---\ntitle: \"Release *notes*\"\nsummary: How we ship\ntags: [ops, deploy]\n---\nLead text.\n# Top\nIntro\n## Rollback\nUndo it.\n```\n# not a heading\n```\n### Deep ###\nDeeper.\n",
        );
        let (s, truncated) = sections(&p);
        assert!(!truncated);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].title, "Release notes");
        assert_eq!(s[0].heading, None);
        assert_eq!(s[0].body, "How we ship\n\nops, deploy\n\nLead text.");
        assert_eq!(s[1].heading.as_deref(), Some("Top"));
        assert_eq!(s[2].heading.as_deref(), Some("Rollback"));
        assert_eq!(s[2].ancestry, vec!["Top"]);
        assert!(s[2].body.contains("# not a heading"));
        assert_eq!(s[3].heading.as_deref(), Some("Deep"));
        assert_eq!(s[3].ancestry, vec!["Top", "Rollback"]);
        assert_eq!(s[3].id, "eng/release.md#3");
    }

    #[test]
    fn long_bodies_split_at_paragraphs_and_titles_fall_back() {
        let para = "word ".repeat(1000);
        let p = page("notes/long-page.md", &format!("{para}\n\n{para}\n\n{para}"));
        let (s, _) = sections(&p);
        assert_eq!(s[0].title, "long page");
        assert!(s.len() >= 2);
        assert!(s.iter().all(|x| x.body.encode_utf16().count() <= SECTION_MAX_CHARS));
    }

    #[test]
    fn indexable_skips_machinery_and_instructions() {
        assert!(indexable("wiki/page.md"));
        assert!(!indexable("wiki/AGENTS.md"));
        assert!(!indexable("_index.md"));
        assert!(!indexable(".finitebrain/x.md"));
        assert!(!indexable("a/.hidden/x.md"));
        assert!(!indexable("site/index.md"));
        assert!(!indexable("page.conflict-1.md"));
        assert!(!indexable("data.json"));
    }

    #[test]
    fn words_split_like_the_brain() {
        assert_eq!(words("What's the \"release\" plan, v2?"), vec!["what", "s", "the", "release", "plan", "v2"]);
    }
}
