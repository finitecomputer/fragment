//! The passkey door's state (passkeys.rs): its accounts, each with its
//! passkeys and its recovery codes' hashes, and its open invites' hashes.
//! One JSON file, owner-only, in its own directory (a box's encrypted
//! disk). Every change is a read, a change and an atomic replace under an
//! exclusive lock, so the running door and its `invite` command, two
//! processes, never lose each other's writes; a read takes the lock shared.
//! Nothing secret is in it: a passkey is a public key, a code or an invite
//! is kept as its SHA-256 alone.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::{Passkey, Uuid};

/// Accounts one door holds, at most (a home's people).
pub const ACCOUNTS_MAX: usize = 1000;
/// Passkeys one account holds, at most.
pub const PASSKEYS_MAX: usize = 16;
/// Recovery codes an account is given, each usable once.
pub const RECOVERY_CODES: usize = 10;
/// Open invites, at most.
pub const INVITES_MAX: usize = 100;
/// The state file, at most.
pub const FILE_BYTES_MAX: u64 = 8 << 20;
/// An account's email, at most.
pub const EMAIL_BYTES_MAX: usize = 254;

const FILE: &str = "accounts.json";
const LOCK: &str = "accounts.lock";
/// The asks an inviter's signatures answered (passkeys.rs `asked_invite`),
/// each event's id and when: a file of its own beside the accounts, so a
/// door of an older release reads its accounts as before, and a replay is
/// refused across this door's restarts too.
const ASKS: &str = "asks.json";
/// The asks' file at most (`passkeys::ASKS_MAX` of them is some 30 KB).
const ASKS_BYTES_MAX: u64 = 256 << 10;

/// Whether an ask is answered here for the first time.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    First,
    Again,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: readable or writable by others (mode {mode:o}): the door's state is its own")]
    Exposed { path: PathBuf, mode: u32 },
    #[error("{path}: past {FILE_BYTES_MAX} bytes")]
    TooLarge { path: PathBuf },
    #[error("{path}: not the door's state: {why}")]
    Malformed { path: PathBuf, why: String },
    #[error("{0}")]
    Full(&'static str),
}

/// One person: their passkeys, and their unused recovery codes' hashes.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub created_at_s: i64,
    pub passkeys: Vec<Passkey>,
    /// SHA-256 (hex) of each recovery code not yet used.
    pub recovery: Vec<String>,
}

/// An invite to make one new account, for one email, until it expires.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    /// SHA-256 (hex) of the invite's token (the link holds the token).
    pub token_sha256: String,
    pub email: String,
    pub expires_at_s: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub accounts: Vec<Account>,
    pub invites: Vec<Invite>,
}

impl State {
    /// The account of `email` (case aside).
    pub fn by_email(&self, email: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.email.eq_ignore_ascii_case(email))
    }

    pub fn by_email_mut(&mut self, email: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.email.eq_ignore_ascii_case(email))
    }

    pub fn by_id_mut(&mut self, id: Uuid) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// Whether any account holds a passkey of this credential id: a
    /// credential is one account's alone.
    pub fn credential_taken(&self, passkey: &Passkey) -> bool {
        self.accounts.iter().any(|a| a.passkeys.iter().any(|p| p.cred_id() == passkey.cred_id()))
    }

    /// Drops expired invites.
    pub fn prune(&mut self, now_s: i64) {
        self.invites.retain(|i| i.expires_at_s > now_s);
    }

    /// The state's own bounds, checked after every change.
    fn check(&self) -> Result<(), StoreError> {
        if self.accounts.len() > ACCOUNTS_MAX {
            return Err(StoreError::Full("the door holds ACCOUNTS_MAX accounts"));
        }
        if self.invites.len() > INVITES_MAX {
            return Err(StoreError::Full("the door holds INVITES_MAX open invites"));
        }
        for a in &self.accounts {
            if a.passkeys.len() > PASSKEYS_MAX {
                return Err(StoreError::Full("an account holds PASSKEYS_MAX passkeys"));
            }
            assert!(a.recovery.len() <= RECOVERY_CODES, "an account holds at most its recovery codes");
            assert!(!a.email.is_empty() && a.email.len() <= EMAIL_BYTES_MAX, "an account's email is checked as it is made");
        }
        for (i, a) in self.accounts.iter().enumerate() {
            assert!(!self.accounts[..i].iter().any(|b| b.id == a.id || b.email.eq_ignore_ascii_case(&a.email)), "an account's id and email are its own");
        }
        Ok(())
    }
}

/// The state's directory: made owner-only, refused when others may read it.
pub struct Store {
    dir: PathBuf,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }
}

impl Store {
    pub fn open(dir: &Path) -> Result<Store, StoreError> {
        if !dir.exists() {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io(dir))?;
        }
        let meta = fs::metadata(dir).map_err(io(dir))?;
        if meta.mode() & 0o077 != 0 {
            return Err(StoreError::Exposed { path: dir.to_path_buf(), mode: meta.mode() & 0o777 });
        }
        let s = Store { dir: dir.to_path_buf() };
        // a state that does not read stops the door at its start
        s.read()?;
        Ok(s)
    }

    fn lock(&self, exclusive: bool) -> Result<File, StoreError> {
        let path = self.dir.join(LOCK);
        let f = OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(&path).map_err(io(&path))?;
        match exclusive {
            true => f.lock().map_err(io(&path))?,
            false => f.lock_shared().map_err(io(&path))?,
        }
        Ok(f)
    }

    fn load(&self) -> Result<State, StoreError> {
        let path = self.dir.join(FILE);
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(e) => return Err(StoreError::Io { path, source: e }),
        };
        if meta.mode() & 0o077 != 0 {
            return Err(StoreError::Exposed { path, mode: meta.mode() & 0o777 });
        }
        if meta.len() > FILE_BYTES_MAX {
            return Err(StoreError::TooLarge { path });
        }
        let bytes = fs::read(&path).map_err(io(&path))?;
        let state: State = serde_json::from_slice(&bytes).map_err(|e| StoreError::Malformed { path: path.clone(), why: e.to_string() })?;
        state.check().map_err(|e| StoreError::Malformed { path, why: e.to_string() })?;
        Ok(state)
    }

    /// The state as it is now.
    pub fn read(&self) -> Result<State, StoreError> {
        let _lock = self.lock(false)?;
        self.load()
    }

    /// Reads the state, applies `change`, and writes it back whole, all
    /// under the exclusive lock. When `change` refuses, nothing is written.
    pub fn change<T, E: From<StoreError>>(&self, change: impl FnOnce(&mut State) -> Result<T, E>) -> Result<T, E> {
        let _lock = self.lock(true)?;
        let mut state = self.load()?;
        let out = change(&mut state)?;
        state.check()?;
        self.write(&state)?;
        Ok(out)
    }

    /// The ask `id` (an event's id) at `now_s`: its first time, or again
    /// while it is remembered (`keep_s` from its first). At most `max` are
    /// remembered at once: past that, `Full`. Under the exclusive lock,
    /// written before it answers.
    pub fn ask(&self, id: &str, now_s: i64, keep_s: i64, max: usize) -> Result<Ask, StoreError> {
        let _lock = self.lock(true)?;
        let path = self.dir.join(ASKS);
        let mut asks: Vec<(String, i64)> = match fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(StoreError::Io { path, source: e }),
            Ok(meta) if meta.mode() & 0o077 != 0 => return Err(StoreError::Exposed { path, mode: meta.mode() & 0o777 }),
            Ok(meta) if meta.len() > ASKS_BYTES_MAX => return Err(StoreError::TooLarge { path }),
            Ok(_) => serde_json::from_slice(&fs::read(&path).map_err(io(&path))?).map_err(|e| StoreError::Malformed { path: path.clone(), why: e.to_string() })?,
        };
        asks.retain(|(_, at)| now_s.saturating_sub(*at) <= keep_s);
        if asks.iter().any(|(a, _)| a == id) {
            return Ok(Ask::Again);
        }
        if asks.len() >= max {
            return Err(StoreError::Full("the door remembers ASKS_MAX asks at once"));
        }
        asks.push((id.to_string(), now_s));
        self.write_file(ASKS, &serde_json::to_vec(&asks).expect("asks serialize"), ASKS_BYTES_MAX)?;
        Ok(Ask::First)
    }

    fn write(&self, state: &State) -> Result<(), StoreError> {
        self.write_file(FILE, &serde_json::to_vec(state).expect("the state serializes"), FILE_BYTES_MAX)
    }

    /// `bytes` as the file `name`, whole or not at all (a new file, synced,
    /// renamed over the old one).
    fn write_file(&self, name: &str, bytes: &[u8], max: u64) -> Result<(), StoreError> {
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!("{name}.new"));
        let _ = fs::remove_file(&tmp);
        if bytes.len() as u64 > max {
            return Err(StoreError::TooLarge { path });
        }
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(io(&tmp))?;
        f.write_all(bytes).and_then(|_| f.sync_all()).map_err(io(&tmp))?;
        fs::rename(&tmp, &path).map_err(io(&path))?;
        File::open(&self.dir).and_then(|d| d.sync_all()).map_err(io(&self.dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("door-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn account(email: &str) -> Account {
        Account { id: Uuid::new_v4(), email: email.into(), created_at_s: 1, passkeys: vec![], recovery: vec!["a".repeat(64)] }
    }

    // Goal: a change is written whole and read back the same, by another
    // store on the same directory too (the `invite` command's); a refused
    // change writes nothing. Restart: a store opened again reads it all.
    #[test]
    fn changes_are_kept_whole() {
        let d = dir("kept");
        let s = Store::open(&d).unwrap();
        s.change(|st| -> Result<(), StoreError> {
            st.accounts.push(account("ada@example.com"));
            st.invites.push(Invite { token_sha256: "b".repeat(64), email: "bo@example.com".into(), expires_at_s: 100 });
            Ok(())
        })
        .unwrap();
        let other = Store::open(&d).unwrap();
        let st = other.read().unwrap();
        assert_eq!(st.accounts.len(), 1);
        assert!(st.by_email("ADA@example.com").is_some(), "an email's case is its own");
        assert_eq!(st.invites.len(), 1);
        let refused: Result<(), StoreError> = s.change(|st| {
            st.accounts.clear();
            Err(StoreError::Full("refused"))
        });
        assert!(refused.is_err());
        assert_eq!(Store::open(&d).unwrap().read().unwrap().accounts.len(), 1, "a refused change wrote nothing");
        assert_eq!(fs::metadata(d.join(FILE)).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(&d).unwrap();
    }

    // Goal: the bounds hold: past an account's passkeys or the invites'
    // number, a change is refused and nothing is written; a state others
    // may read, or one malformed, stops the store at its open.
    #[test]
    fn bounds_and_bad_states_are_refused() {
        let d = dir("bounds");
        let s = Store::open(&d).unwrap();
        let many: Result<(), StoreError> = s.change(|st| {
            for i in 0..=INVITES_MAX {
                st.invites.push(Invite { token_sha256: format!("{i:064}"), email: "x@example.com".into(), expires_at_s: 100 });
            }
            Ok(())
        });
        assert!(matches!(many, Err(StoreError::Full(_))));
        assert!(s.read().unwrap().invites.is_empty());
        fs::write(d.join(FILE), b"{\"accounts\":[],\"invites\":[],\"surprise\":1}").unwrap();
        fs::set_permissions(d.join(FILE), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        assert!(matches!(Store::open(&d), Err(StoreError::Malformed { .. })), "unknown fields refused");
        fs::write(d.join(FILE), b"{\"accounts\":[],\"invites\":[]}").unwrap();
        fs::set_permissions(d.join(FILE), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert!(matches!(Store::open(&d), Err(StoreError::Exposed { .. })));
        fs::remove_dir_all(&d).unwrap();
    }

    // Goal: expired invites go at a prune; open ones stay.
    #[test]
    fn expired_invites_are_pruned() {
        let mut st = State::default();
        st.invites.push(Invite { token_sha256: "a".repeat(64), email: "a@example.com".into(), expires_at_s: 10 });
        st.invites.push(Invite { token_sha256: "b".repeat(64), email: "b@example.com".into(), expires_at_s: 20 });
        st.prune(10);
        assert_eq!(st.invites.len(), 1);
        assert_eq!(st.invites[0].email, "b@example.com");
    }
}
