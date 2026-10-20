//! The support directory only. Messages and memberships stay in chat fragments.
use fragment_proto::{ErrorCode, Identity};
use serde::{Deserialize, Serialize};
use worker::*;

use super::calls::{By, Call};
use super::{body, reply, RegistryCell};
use crate::error::{CellError, CellResult};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Thread {
    pub cursor: i64,
    pub created_at: i64,
    pub person: String,
    pub name: String,
    pub owner: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Directory {
    pub by: By,
    pub person: Option<String>,
    pub create: bool,
    pub after: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct DirectoryAnswer {
    pub staff: bool,
    pub threads: Vec<Thread>,
    pub next: Option<String>,
    pub team: Vec<Identity>,
}
impl Call for Directory {
    const PATH: &'static str = "/support/directory";
    type Answer = DirectoryAnswer;
}

/// Authoritative classification, including conversations created before this
/// policy shipped. Names are checked against the directory, never a prefix.
#[derive(Serialize, Deserialize)]
pub(crate) struct Protected {
    pub names: Vec<String>,
}
impl Call for Protected {
    const PATH: &'static str = "/support/protected";
    type Answer = Vec<String>;
}

impl RegistryCell {
    fn support_directory(&self, b: Directory) -> CellResult<DirectoryAnswer> {
        let who = self.person_by(&b.by)?;
        let mut team = Vec::new();
        for email in &self.cfg.support_team {
            if let Some(id) = self.person_by_email(email)? {
                let person = self.by(&By::Identity(id))?;
                team.push(person);
            }
        }
        let staff = team.iter().any(|p| p.id == who.id);
        let person = b.person.as_deref().unwrap_or(&who.id);
        if person != who.id && !staff {
            return Err(CellError::new(
                ErrorCode::Forbidden,
                "only the support team can open another person's support thread",
            ));
        }
        if b.create {
            // A user may start only their own conversation. Staff open existing ones.
            if person != who.id {
                return Err(CellError::new(
                    ErrorCode::Forbidden,
                    "a person starts their own support thread",
                ));
            }
            let email = self.cfg.support_team.first().ok_or_else(|| {
                CellError::invalid("support chat is not configured; email support@finite.computer")
            })?;
            let owner = self.person_by_email(email)?.ok_or_else(|| {
                CellError::invalid("support is not ready yet; email support@finite.computer")
            })?;
            let label = format!("support-{}", crate::js::random_hex::<8>());
            let name = fragment_proto::fragment_name(&label, crate::js::random_bytes::<3>());
            // Synchronous reservation: retries, simultaneous tabs and restarts choose one name.
            self.exec("INSERT OR IGNORE INTO support_threads (person, name, owner, created_at) VALUES (?, ?, ?, ?)",
                vec![person.into(), name.as_str().into(), owner.as_str().into(), SqlStorageValue::Integer(crate::js::now_ms())])?;
        }
        let inbox = staff && b.person.is_none() && !b.create;
        let threads = if inbox {
            let before = b.after.as_deref().map(str::parse::<i64>).transpose().map_err(|_| CellError::invalid("support cursor is an integer"))?.unwrap_or(i64::MAX);
            self.rows::<Thread>("SELECT rowid AS cursor, created_at, person, name, owner FROM support_threads WHERE rowid < ? ORDER BY rowid DESC LIMIT 50",
                vec![SqlStorageValue::Integer(before)])?
        } else {
            self.rows::<Thread>(
                "SELECT rowid AS cursor, created_at, person, name, owner FROM support_threads WHERE person = ?",
                vec![person.into()],
            )?
        };
        let next = (inbox && threads.len() == 50)
            .then(|| threads.last().expect("a full page").cursor.to_string());
        Ok(DirectoryAnswer {
            staff,
            threads,
            next,
            team,
        })
    }

    pub(super) fn support_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        if path == Protected::PATH {
            return Some(reply::<Protected>(body::<Protected>(bytes).and_then(|b| {
                let mut names = Vec::new();
                for name in b.names {
                    if !self.rows::<Thread>("SELECT rowid AS cursor, created_at, person, name, owner FROM support_threads WHERE name = ?", vec![name.as_str().into()])?.is_empty() {
                        names.push(name);
                    }
                }
                Ok(names)
            })));
        }
        (path == Directory::PATH)
            .then(|| reply::<Directory>(body(bytes).and_then(|b| self.support_directory(b))))
    }
}
