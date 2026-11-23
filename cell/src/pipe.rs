//! The pipe (docs/ledger.md, "The gate"; FIN-616): every paid call an
//! agent makes is gated and charged here, the same way, whichever way it
//! came in: its model calls through the model route (models.rs), its
//! tools' calls on an operator key through its computer's swap
//! (computer.rs `egress_swap`).
//!
//! - **Gated beside the call** (`gate`): its payer's standing as this
//!   isolate heard it within the minute, and the fragment's cap when it is
//!   spent in someone else's fragment, never waited on unless last heard
//!   refused (ledger.rs `may_beside`). So a payer out of credit may make
//!   one call more in each isolate that has not heard them.
//! - **Charged after its answer** (`settle`): its usage as one row on its
//!   payer's ledger, under its own reference, on the ledger's queue, never
//!   in the answer's way (decision 62). A call that reported no usage is
//!   charged nothing, said aloud (`ledger.unknown-usage`).
//!
//! A fragment's own paid calls (a job's AI step, an icon's making, a
//! card's shot) are charged through its outbox instead (meter.rs), which
//! keeps each row in the fragment's own storage, beside what it bought,
//! until its ledger has it.

use fragment_core::ledger::{Meter, MeterBatch, MeterRow, Spend};
use fragment_core::price::Usage;
use serde_json::json;
use worker::*;

use crate::error::CellResult;
use crate::js;
use crate::ledger::{self, FragmentOpen, MaySpend};
use crate::meter::QUEUE_BINDING;
use crate::models::{Background, InFragment};

/// One paid call's charge, made after its answer: who pays, under which
/// reference, as which agent, counted in which of the payer's fragments,
/// from which computer's swap.
pub struct Bill {
    pub payer: String,
    pub reference: String,
    pub agent: Option<String>,
    /// The payer's own fragment the call is in: its month's spend counts it.
    pub fragment: Option<String>,
    pub computer: Option<String>,
}

/// Asks the gate whether `payer` may make an agent's paid call, beside it:
/// whose cap applies is the payer's own fragment's, on its ledger, or
/// another owner's, asked of theirs, unless the spender is that owner's
/// (decision 26). The two are asked side by side (`gate::asked_together`),
/// the payer's refusal said first. Its answer is the payer's own fragment
/// the call is in, which the call's bill names.
pub async fn gate(env: &Env, payer: &str, fragment: Option<&InFragment<'_>>, after: &dyn Background) -> CellResult<Option<String>> {
    let own = fragment.filter(|f| f.owner == payer);
    let capped = own.filter(|f| !f.by_owner).map(|f| f.name.to_string());
    let may = MaySpend { spend: Spend::AgentTurn, by_owner: capped.is_none(), fragment: capped };
    let open = fragment.filter(|f| f.owner != payer && !f.by_owner).map(|f| (f.owner, FragmentOpen { fragment: f.name.to_string() }));
    let theirs = open.as_ref().map(|(owner, open)| ledger::may_beside(env, owner, open, after));
    fragment_core::ledger::gate::asked_together(ledger::may_beside(env, payer, &may, after), theirs).await?;
    Ok(own.map(|f| f.name.to_string()))
}

/// Charges each bill its call's usage, after the call answered, never in
/// its way: each a batch of its own under its row's reference, on the
/// ledger's queue, whose consumer applies it (once by its reference: a
/// batch sent again charges nothing twice) and asks again while the
/// ledger does not answer. A bill with no usage is charged nothing, said
/// aloud. A send that fails is said aloud with its references: the charge
/// is lost, and the call stands.
pub async fn settle(env: &Env, bills: Vec<(Bill, Option<Usage>)>) {
    let at_ms = js::now_ms();
    let (mut refs, mut bodies) = (vec![], vec![]);
    for (bill, usage) in bills {
        let Some(usage) = usage else {
            console_error!("{}", json!({ "event": "ledger.unknown-usage", "ref": bill.reference, "why": "the call reported no usage: charged nothing" }));
            continue;
        };
        let row = MeterRow { reference: bill.reference.clone(), usage, fragment: bill.fragment, agent: bill.agent, computer: bill.computer, at_ms };
        let meter = Meter { batch: bill.reference.clone(), rows: vec![row] };
        refs.push(bill.reference);
        bodies.push(serde_json::to_value(MeterBatch { payer: bill.payer, source: String::new(), seq: 0, meter }).expect("a batch serializes"));
    }
    if bodies.is_empty() {
        return;
    }
    if let Err(e) = js::queue_send(env.as_ref(), QUEUE_BINDING, &bodies).await {
        console_error!("{}", json!({ "event": "ledger.charge-lost", "refs": refs, "message": e.message }));
    }
}
