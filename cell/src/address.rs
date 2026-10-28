//! A public address is a second origin of an existing fragment, not a
//! new fragment, repo, or membership. Claims are the owner's; the
//! registry's constraints choose the first claimant atomically.

use fragment_core::address::{self, Address};
use fragment_proto::{ErrorCode, FragmentKind, Role};
use serde_json::{json, Value};
use worker::Response;

use crate::error::{CellError, CellResult};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::registry::calls::{AddressOf, ClaimAddress};

/// Only the router sets this, after resolving the address. It is never
/// copied from a browser's headers. Every request checks it before acting.
pub(crate) const IDENTITY_HEADER: &str = "x-fragment-address-identity";

impl FragmentCell {
    pub(crate) async fn address(&self, caller: &Caller) -> CellResult<Response> {
        self.require(caller, false, Role::Viewer)?;
        let address = crate::ask_registry(&self.env, &AddressOf::Fragment { fragment: self.name()?, npub: self.must(MetaKey::Npub)? }).await?;
        self.address_answer(caller, address)
    }

    pub(crate) async fn claim_address(&self, caller: &Caller, body: Value) -> CellResult<Response> {
        self.require(caller, false, Role::Owner)?;
        self.writable().await?;
        if matches!(self.kind()?, FragmentKind::Chat | FragmentKind::Agent | FragmentKind::Skills) {
            return Err(CellError::invalid("a public address is for an app or a brain"));
        }
        let label = body["label"].as_str().ok_or_else(|| CellError::invalid("name the public address"))?;
        if !address::valid(label, self.cfg.host_label_suffix()) {
            return Err(CellError::invalid("use lowercase letters, numbers and single dashes; the address must fit one DNS label"));
        }
        let origin = self.cfg.address_origin(&caller.url, label);
        let host = url::Url::parse(&origin).map_err(|e| CellError::host(e.to_string()))?;
        if host.host_str().is_some_and(|host| self.cfg.is_platform_host(host)) {
            return Err(CellError::new(ErrorCode::AlreadyExists, "that address is reserved for the platform"));
        }
        let name = self.name()?;
        let npub = self.must(MetaKey::Npub)?;
        let address = crate::ask_registry(&self.env, &ClaimAddress { address: Address { label: label.to_string(), fragment: name, npub: npub.clone() } }).await?;
        // A delete during the registry call cannot name a new life.
        if self.meta(MetaKey::Npub)?.as_deref() != Some(npub.as_str()) {
            return Err(CellError::new(ErrorCode::NotFound, "this app ended while its address was being claimed"));
        }
        self.address_answer(caller, Some(address))
    }

    fn address_answer(&self, caller: &Caller, address: Option<Address>) -> CellResult<Response> {
        let public = address.map(|a| json!({ "label": a.label, "url": format!("{}/", self.cfg.address_origin(&caller.url, &a.label)) }));
        json_response(&json!({ "address": public, "suffix": self.cfg.address_suffix(&caller.url), "maxLength": 63 - self.cfg.host_label_suffix().len() }))
    }
}
