//! What an import brings of an agent's files, and what it never does
//! (FIN-463 §4, "The files cap"), by name alone: the same rules for any
//! agent's home, wherever it ran. A layout adds the paths that hold
//! secrets on its platform (`layout.rs`); nothing here knows one.

/// Files that hold, or may hold, a credential, by name. A source holding
/// one is refused (`survey`): the import never reads a secret, and what
/// staged it should have left it out.
const SECRET_NAMES: &[&str] = &[
    ".env", ".envrc", "auth.json", "google_token.json", "google_client_secret.json", "google_oauth_pending.json", "git-credentials", ".git-credentials", ".netrc",
    ".npmrc", ".pypirc", ".pgpass", "id_rsa", "id_dsa", "id_ed25519", "id_ecdsa", "credentials.json", "client_secret.json", "token.json", "secrets.yaml", "secrets.yml",
    "secrets.json",
];
const SECRET_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".env"];

/// Whether a file's name says it may hold a secret.
pub fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let json_shape = lower.ends_with(".json")
        && (lower.starts_with("client_secret") || lower.strip_prefix("service").is_some_and(|r| r.trim_start_matches(['-', '_']).starts_with("account")));
    SECRET_NAMES.contains(&name) || name.starts_with(".env.") || SECRET_SUFFIXES.iter().any(|s| lower.ends_with(s)) || json_shape
}

/// Folders of dependencies and caches, anywhere: made again by the tools
/// that made them, never a person's own.
const DEPENDENCY_DIRS: &[&str] = &[
    ".git", "node_modules", ".venv", "venv", "__pycache__", ".cache", ".next", ".mypy_cache", ".pytest_cache", ".tox", ".gradle", "bower_components", "site-packages", "Pods",
    "DerivedData", "elm-stuff",
];
/// What a tool builds beside a project file, made again from the source
/// around it: left out only beside such a file, since a folder of notes
/// may be called `build` too.
const BUILD_DIRS: &[&str] = &["dist", "build", "out", "target", "_build", "obj", "deps", "vendor", "coverage"];
const PROJECT_FILES: &[&str] = &[
    "package.json", "deno.json", "Cargo.toml", "go.mod", "pyproject.toml", "setup.py", "pom.xml", "build.gradle", "build.gradle.kts", "Makefile", "CMakeLists.txt",
    "mix.exs", "Gemfile", "composer.json", "pubspec.yaml", "Package.swift", "build.zig", "stack.yaml",
];
const PROJECT_SUFFIXES: &[&str] = &[".csproj", ".fsproj", ".sln", ".cabal"];

pub fn dependency_dir(name: &str) -> bool {
    DEPENDENCY_DIRS.contains(&name)
}

pub fn build_dir(name: &str) -> bool {
    BUILD_DIRS.contains(&name)
}

pub fn project_file(name: &str) -> bool {
    PROJECT_FILES.contains(&name) || PROJECT_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// What a file is, by its extension: what the cap brings first (media
/// and documents), what it brings after (everything else: source and
/// data), and what it never brings (`Kind::never`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Photo,
    Video,
    Audio,
    Document,
    /// Source and data: anything else.
    Other,
    Archive,
    Installer,
    ModelWeights,
    Compiled,
}

const PHOTO: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "tif", "tiff", "bmp", "svg", "dng", "raw", "cr2", "cr3", "nef", "arw", "orf", "rw2", "psd"];
const VIDEO: &[&str] = &["mp4", "mov", "m4v", "avi", "mkv", "webm", "wmv", "flv", "mpg", "mpeg", "3gp"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "aif", "aiff", "wma"];
const DOCUMENT: &[&str] = &[
    "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "markdown", "pages", "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key", "epub",
];
const ARCHIVE: &[&str] = &["zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar"];
const INSTALLER: &[&str] = &["dmg", "iso", "img", "pkg", "deb", "rpm", "apk", "ipa", "msi", "exe", "jar", "war", "whl"];
const MODEL_WEIGHTS: &[&str] = &["safetensors", "ckpt", "pt", "pth", "onnx", "gguf", "ggml", "bin", "h5", "npy", "npz", "pkl", "tflite"];
const COMPILED: &[&str] = &["o", "so", "dylib", "dll", "a", "lib", "class", "pyc", "wasm", "map"];

impl Kind {
    pub fn of(name: &str) -> Kind {
        let Some((_, ext)) = name.rsplit_once('.') else { return Kind::Other };
        let ext = ext.to_ascii_lowercase();
        let is = |list: &[&str]| list.contains(&ext.as_str());
        // `.key` is a Keynote deck and a private key's name: a secret's
        // name is refused before any kind is asked (`secret_name`)
        if is(PHOTO) {
            Kind::Photo
        } else if is(VIDEO) {
            Kind::Video
        } else if is(AUDIO) {
            Kind::Audio
        } else if is(DOCUMENT) {
            Kind::Document
        } else if is(ARCHIVE) {
            Kind::Archive
        } else if is(INSTALLER) {
            Kind::Installer
        } else if is(MODEL_WEIGHTS) {
            Kind::ModelWeights
        } else if is(COMPILED) {
            Kind::Compiled
        } else {
            Kind::Other
        }
    }

    /// Never brought, at any size: made again, or not a person's to read.
    pub fn never(self) -> bool {
        matches!(self, Kind::Archive | Kind::Installer | Kind::ModelWeights | Kind::Compiled)
    }

    /// Brought before the rest: what a person keeps.
    pub fn first(self) -> bool {
        matches!(self, Kind::Photo | Kind::Video | Kind::Audio | Kind::Document)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_by_name() {
        for name in [".env", ".env.local", "prod.env", "id_ed25519", "server.PEM", "auth.json", "client_secret_123.json", "service-account-x.json", "service_account.json", ".netrc"] {
            assert!(secret_name(name), "{name}");
        }
        for name in ["env.md", "notes.json", "id_ed25519.pub", "keys.txt", "service.json", "README.md", "environment.yaml"] {
            assert!(!secret_name(name), "{name}");
        }
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(Kind::of("IMG_0001.HEIC"), Kind::Photo);
        assert_eq!(Kind::of("talk.mp4"), Kind::Video);
        assert_eq!(Kind::of("memo.m4a"), Kind::Audio);
        assert_eq!(Kind::of("plan.pdf"), Kind::Document);
        assert_eq!(Kind::of("main.py"), Kind::Other);
        assert_eq!(Kind::of("Makefile"), Kind::Other);
        assert_eq!(Kind::of("backup.tar.gz"), Kind::Archive);
        assert_eq!(Kind::of("model.gguf"), Kind::ModelWeights);
        assert!(Kind::of("lib.so").never() && Kind::of("setup.exe").never());
        assert!(Kind::of("a.png").first() && !Kind::of("a.csv").first() && !Kind::of("a.csv").never());
    }
}
