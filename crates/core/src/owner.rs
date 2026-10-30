//! Who owns a fragment (docs/cloudflare-v1.md, decisions 66 to 71): a
//! person, by their identity, or an org, by its id. The fragment's own
//! `ownership` row is the one record of it. An owner is named as its
//! ledger and its list are (`name`): a person's identity, or `org:<id>`,
//! so the fragment's hosting and AI steps bill `payer()`, and its owner's
//! row goes to the list of that name (an org's list is its inventory).
//! An org is never a signing identity: no key, no session, no agent.

use crate::ledger::org::{Payer, ORG_LEDGER_PREFIX};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Person(String),
    Org(String),
}

impl Owner {
    /// As the `ownership` row keeps it: `(kind, id)`.
    pub fn from_row(kind: &str, id: &str) -> Option<Owner> {
        match kind {
            "person" if crate::npub::is_identity(id) => Some(Owner::Person(id.to_string())),
            "org" if crate::org::valid_org_id(id) => Some(Owner::Org(id.to_string())),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Owner::Person(_) => "person",
            Owner::Org(_) => "org",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Owner::Person(id) | Owner::Org(id) => id,
        }
    }

    /// Its ledger's and its list's name: the person's identity, or
    /// `org:<id>`. A fragment's legacy `owner` field shows it.
    pub fn name(&self) -> String {
        self.payer().name()
    }

    /// The owner a name names (`name`'s inverse).
    pub fn parse(name: &str) -> Option<Owner> {
        match Payer::parse(name)? {
            Payer::Person(id) => Some(Owner::Person(id)),
            Payer::Org(id) => Some(Owner::Org(id)),
        }
    }

    /// Who pays for the fragment: its owner.
    pub fn payer(&self) -> Payer {
        match self {
            Owner::Person(id) => Payer::Person(id.clone()),
            Owner::Org(id) => Payer::Org(id.clone()),
        }
    }

    /// The person, when a person owns it.
    pub fn person(&self) -> Option<&str> {
        match self {
            Owner::Person(id) => Some(id),
            Owner::Org(_) => None,
        }
    }

    /// The org, when an org owns it.
    pub fn org(&self) -> Option<&str> {
        match self {
            Owner::Org(id) => Some(id),
            Owner::Person(_) => None,
        }
    }

    /// Whether `principal` is this owner: a person owns as themselves; an
    /// org is never a principal.
    pub fn is(&self, principal: &str) -> bool {
        self.person() == Some(principal)
    }
}

const _: () = assert!(ORG_LEDGER_PREFIX.len() == 4, "an org's name is org:<id>");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_is_named_as_its_ledger_and_list() {
        let person = crate::npub::encode(&"cd".repeat(32));
        let org = "org-0123456789abcdef";
        for owner in [Owner::Person(person.clone()), Owner::Org(org.into())] {
            assert_eq!(Owner::parse(&owner.name()), Some(owner.clone()));
            assert_eq!(Owner::from_row(owner.kind(), owner.id()), Some(owner.clone()));
            assert_eq!(owner.payer().name(), owner.name());
        }
        assert_eq!(Owner::Org(org.into()).name(), format!("org:{org}"));
        assert!(Owner::Person(person.clone()).is(&person));
        assert!(!Owner::Org(org.into()).is(org), "an org is never a principal");
        assert_eq!(Owner::from_row("org", &person), None);
        assert_eq!(Owner::from_row("person", org), None);
        assert_eq!(Owner::from_row("team", org), None);
    }
}
