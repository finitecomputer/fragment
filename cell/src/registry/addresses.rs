use fragment_core::address::{self, Address};
use fragment_proto::ErrorCode;

use crate::error::{CellError, CellResult};
use crate::registry::calls::{AddressOf, ClaimAddress};
use super::RegistryCell;

impl RegistryCell {
    pub(super) fn address_of(&self, query: AddressOf) -> CellResult<Option<Address>> {
        match query {
            AddressOf::Label { label } => self.row("SELECT label, fragment, npub FROM public_addresses WHERE label = ?", vec![label.into()]),
            AddressOf::Fragment { fragment, npub } => self.row("SELECT label, fragment, npub FROM public_addresses WHERE fragment = ? AND npub = ?", vec![fragment.into(), npub.into()]),
        }
    }

    pub(super) fn claim_address(&self, claim: ClaimAddress) -> CellResult<Address> {
        let a = claim.address;
        if !address::valid(&a.label, self.cfg.host_label_suffix()) || !fragment_proto::valid_fragment_name(&a.fragment) || !fragment_core::npub::is_identity(&a.npub) {
            return Err(CellError::invalid("invalid public address binding"));
        }
        // No await between the attempted insert and reading its winner.
        // Both keys are unique: the address and this life of the app.
        self.exec("INSERT OR IGNORE INTO public_addresses (label, fragment, npub) VALUES (?, ?, ?)", vec![a.label.as_str().into(), a.fragment.as_str().into(), a.npub.as_str().into()])?;
        let found = self.address_of(AddressOf::Label { label: a.label.clone() })?;
        match found {
            Some(found) if found == a => Ok(found),
            Some(_) => Err(CellError::new(ErrorCode::AlreadyExists, "that address is already taken")),
            None => Err(CellError::new(ErrorCode::AlreadyExists, "this app already has a public address")),
        }
    }
}
