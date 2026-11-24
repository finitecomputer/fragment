//! Files, both ways, by the bridge's rules (`fragment_bridge::simplex::run`:
//! its bounds, its notes, what a file goes as), as its door carries them:
//!
//! - **Its owner's** (a voice message, a photo, a file): the message that
//!   carries one waits at the front of its deliveries while its core
//!   receives it (`/freceive`, at most `run::FILE_WAIT_MS`), then the file
//!   goes whole to its door (`PUT /file/<sha256>`) and its delivery names
//!   it; one past `run::FILE_BYTES_MAX`, or that does not come, is
//!   delivered as a note in its message's text. Deliveries stay in order.
//! - **A reply's** (its connector names each, which its door holds): read
//!   from its door, copied into a folder of its own under `run::OUT_DIR`,
//!   and asked of the core as a voice message, an image or a file
//!   (`run::file_kind`), once the reply's text is sent; the reply is done
//!   once the core took each. Its upload is watched (`Kept::sending`, in
//!   every save) until it ends or `run::SEND_WAIT_MS` passes; one that
//!   fails is said so. One that cannot go is said so in its place.
//! - **The core's way out** (`Relay`): the bridge's SOCKS5 proxy, which
//!   sends what the core opens on 443 by name through its door's XFTP
//!   relay (a container's own TLS hello there is reset), the rest (SMP, on
//!   5223) straight out; at most `RELAYS_MAX` relays at once.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use fragment_bridge::simplex::{run, wire};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::door::{Door, DoorError, File};

/// The relays a door's core holds open at once, at most: a computer's
/// (`fragment_core::relay::OPEN_MAX`; its own workspace, a copy), so the
/// Worker that pipes them holds what one computer's would.
pub const RELAYS_MAX: usize = 4;
/// SimpleX's XFTP servers' port: the one relayed.
const RELAY_PORT: u16 = 443;

/// A file of the message at the front of its deliveries, on its way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The message's item, and its file's id at the core.
    pub item: i64,
    pub file: wire::FileId,
    pub state: Got,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    /// Asked of the core (`/freceive`) at `since` (the runner's clock, ms):
    /// waited for whole.
    Receiving { since: u64 },
    /// Whole at `path`, for its door (again, after a door that did not take it).
    Whole { path: PathBuf },
    /// Being given to its door.
    Giving { path: PathBuf },
    /// Its door holds it: its delivery names it.
    Carried { path: PathBuf, file: File },
    /// It will not come: its delivery says why.
    Noted(String),
}

impl Receipt {
    /// Whether its message may be delivered now.
    pub fn settled(&self) -> bool {
        matches!(self.state, Got::Carried { .. } | Got::Noted(_))
    }
}

/// Why a file given to its door was not taken: asked again later (its door
/// unreachable, or holding as many as it may), or never (`Refused`).
#[derive(Debug)]
pub enum NotGiven {
    Later(String),
    Refused(String),
}

/// A file received whole, given to its door: its bytes read (at most
/// `run::FILE_BYTES_MAX`), named by their SHA-256, typed as its message's
/// kind and name say (`run::media_type`).
pub async fn give(door: &Door, path: &Path, kind: &str) -> Result<File, NotGiven> {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) if !b.is_empty() && b.len() as u64 <= run::FILE_BYTES_MAX => b,
        Ok(b) => return Err(NotGiven::Refused(format!("{} bytes, not a file this carries", b.len()))),
        Err(e) => return Err(NotGiven::Refused(format!("unread: {e}"))),
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file = File { sha256: sha256_hex(&bytes), name: wire::cut(&name, 200).to_string(), media_type: run::media_type(kind, &name).to_string(), size: bytes.len() as u64 };
    match door.put_file(&file.sha256, &file.media_type, Bytes::from(bytes)).await {
        Ok(()) => Ok(file),
        Err(DoorError::Transport(e)) => Err(NotGiven::Later(e)),
        Err(DoorError::Answer { status, body }) if status == 503 || status >= 500 => Err(NotGiven::Later(format!("{status}: {body}"))),
        Err(e) => Err(NotGiven::Refused(e.to_string())),
    }
}

/// A reply's file, read and copied out, or what stays in the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// Copied to `path` (its folder its own, under `run::OUT_DIR`), to go
    /// as `kind`.
    Copied { path: PathBuf, kind: wire::Kind },
    /// It does not go (past SimpleX's bound here, gone from its door, not
    /// what it says, or not written): its words, for its note.
    Kept(&'static str),
}

/// A reply's files read from its door and each copied into a folder of its
/// own (`<life>-<n>` under `out`, `n` from `next`), under its own name
/// made safe (`run::file_name`). Its door unreachable: none, the reply's
/// files asked again later (the folders made so far let go).
pub async fn fetch(door: &Door, out: &Path, life: u64, next: u64, files: &[File]) -> Result<Vec<Fetched>, String> {
    let mut got = Vec::with_capacity(files.len());
    for (n, f) in files.iter().enumerate() {
        let words = run::words_of(&f.media_type);
        if f.size == 0 || f.size > run::FILE_BYTES_MAX {
            got.push(Fetched::Kept(words));
            continue;
        }
        let bytes = match door.file(&f.sha256).await {
            Ok(b) => b,
            Err(DoorError::Transport(e)) => {
                let _ = let_go_all(&got).await;
                return Err(e);
            }
            Err(e) => {
                fragment_bridge::ev!("door.file_kept", { "sha256": f.sha256, "why": e.to_string() });
                got.push(Fetched::Kept(words));
                continue;
            }
        };
        if bytes.len() as u64 > run::FILE_BYTES_MAX || sha256_hex(&bytes) != f.sha256 {
            fragment_bridge::ev!("door.file_kept", { "sha256": f.sha256, "why": "not the file it names" });
            got.push(Fetched::Kept(words));
            continue;
        }
        let kind = run::file_kind(&f.media_type, &bytes);
        let folder = out.join(format!("{life}-{}", next + n as u64));
        let path = folder.join(run::file_name(&f.name));
        let written = async {
            tokio::fs::create_dir_all(&folder).await?;
            tokio::fs::write(&path, &bytes).await
        };
        match written.await {
            Ok(()) => got.push(Fetched::Copied { path, kind }),
            Err(e) => {
                fragment_bridge::ev!("door.file_kept", { "sha256": f.sha256, "why": e.to_string() });
                let _ = tokio::fs::remove_dir_all(&folder).await;
                got.push(Fetched::Kept(kind.words()));
            }
        }
    }
    Ok(got)
}

async fn let_go_all(got: &[Fetched]) {
    for g in got {
        if let Fetched::Copied { path, .. } = g {
            let_go(path);
        }
    }
}

/// A copy's folder let go, whole (its own, under `run::OUT_DIR`).
pub fn let_go(path: &Path) {
    if let Some(folder) = path.parent().filter(|p| p.parent().is_some_and(|o| o.ends_with(run::OUT_DIR))) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The core's XFTP traffic through its door (the bridge's socks.rs): what
/// it opens on 443 by name, at most `RELAYS_MAX` at once.
#[derive(Clone)]
pub struct Relay {
    pub door: Door,
    pub open: Arc<Semaphore>,
}

impl fragment_bridge::simplex::socks::Relay for Relay {
    type Held = OwnedSemaphorePermit;

    fn carried(&self, host: &str, port: u16) -> Option<String> {
        (port == RELAY_PORT).then(|| host.to_ascii_lowercase())
    }

    async fn open(&self, host: &str, port: u16) -> Result<(fragment_bridge::net::ClientWs, OwnedSemaphorePermit), String> {
        let held = self.open.clone().try_acquire_owned().map_err(|_| format!("{RELAYS_MAX} relays are open already"))?;
        Ok((self.door.relay(host, port).await?, held))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_bridge::simplex::socks::Relay as _;

    /// Goal: what the core opens on 443 by name goes through its door, the
    /// rest straight out; and no more than `RELAYS_MAX` at once. Method: a
    /// relay over a door that is nowhere (its opens fail after their place
    /// is taken), and its places taken by hand.
    #[tokio::test]
    async fn only_443_by_name_is_relayed_and_at_most_four_at_once() {
        let relay = Relay { door: Door::new("http://127.0.0.1:9", 1).unwrap(), open: Arc::new(Semaphore::new(RELAYS_MAX)) };
        assert_eq!(relay.carried("XFTP3.simplex.im", 443).as_deref(), Some("xftp3.simplex.im"));
        assert_eq!(relay.carried("smp4.simplex.im", 5223), None, "SMP goes straight out");
        let held: Vec<_> = (0..RELAYS_MAX).map(|_| relay.open.clone().try_acquire_owned().unwrap()).collect();
        let refused = relay.open("xftp3.simplex.im", 443).await.err().unwrap();
        assert!(refused.contains("open already"), "{refused}");
        drop(held);
        let unreachable = relay.open("xftp3.simplex.im", 443).await.err().unwrap();
        assert!(!unreachable.contains("open already"), "a place freed is taken again: {unreachable}");
        assert!(relay.open("bad host", 443).await.is_err(), "a name of letters, digits, dots and dashes alone");
    }
}
