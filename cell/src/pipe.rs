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
//! - **Nothing it adds comes back** (`answer`): each credential the swap
//!   put on a request, an operator key or a Vault's fill, is named as its
//!   secret wherever the answer echoes it.
//!
//! A fragment's own paid calls (a job's AI step, an icon's making, a
//! card's shot) are charged through its outbox instead (meter.rs), which
//! keeps each row in the fragment's own storage, beside what it bought,
//! until its ledger has it.

use fragment_core::ledger::{Meter, MeterBatch, MeterRow, Spend};
use fragment_core::price::Usage;
use futures_util::StreamExt;
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

/// An answer to a request read whole to be named, every spelling of each
/// value it echoes, up to this long (text: `secrets::read_whole`); past it,
/// or one that streams as it is made, each value's own bytes are named as
/// it streams.
const ANSWER_HIDDEN_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The pipe's one rule for an answer (FIN-616): nothing it put on the
/// request comes back. Each credential the swap placed (`passes`: the
/// operator's keys first, then a Vault's fills) is named as its secret
/// (`{{name}}`), as a job's fetch names its own (2026-11-09: echoed values
/// hidden), in the answer's headers and its body. The operator's keys go
/// in a pass of their own, first, so no value a person chose cuts into one
/// (`secrets::redact_passes`). A body read whole (up to
/// `ANSWER_HIDDEN_MAX_BYTES`, text) has every spelling of a value named
/// (`secrets::redact`), or its bytes, not text; past that, or one that
/// streams as it is made (an event stream, a streamed speech's audio),
/// each value's own bytes are named as it streams
/// (`secrets::StreamedPasses`), a value split across pieces too.
pub async fn answer(mut resp: Response, passes: &[Vec<(String, String)>]) -> CellResult<Response> {
    let redact = |text: String| fragment_core::secrets::redact_passes(text, passes);
    // appended, not set: each of several fields of one name (`set-cookie`)
    // stays its own
    let headers = Headers::new();
    for (name, value) in resp.headers().entries() {
        headers.append(&name, &redact(value))?;
    }
    let status = resp.status_code();
    // no body (a HEAD's, a 204's): its headers were all it could echo
    if !matches!(resp.body(), ResponseBody::Stream(_)) || matches!(status, 101 | 204 | 304) {
        return Ok(resp.with_headers(headers));
    }
    let kind = resp.headers().get("content-type")?.unwrap_or_default();
    let mut stream = resp.stream()?;
    let mut streamed = fragment_core::secrets::StreamedPasses::new(passes);
    let mut read = vec![];
    if fragment_core::secrets::read_whole(&kind) {
        // bounded: each piece read adds to `read`, which stops at its cap
        while read.len() <= ANSWER_HIDDEN_MAX_BYTES {
            match stream.next().await {
                Some(piece) => read.extend(piece?),
                None => {
                    let body = match String::from_utf8(read) {
                        Ok(text) => redact(text).into_bytes(),
                        Err(e) => [streamed.push(&e.into_bytes()), streamed.finish()].concat(),
                    };
                    headers.set("content-length", &body.len().to_string())?;
                    return Ok(Response::from_bytes(body)?.with_status(status).with_headers(headers));
                }
            }
        }
    }
    // a stream: what was read so far, then the rest as it comes, each named
    let _ = headers.delete("content-length");
    let first = streamed.push(&read);
    let rest = futures_util::stream::unfold(Some((stream, streamed)), |state| async move {
        let (mut stream, mut streamed) = state?;
        match stream.next().await {
            Some(Ok(piece)) => Some((Ok::<Vec<u8>, worker::Error>(streamed.push(&piece)), Some((stream, streamed)))),
            Some(Err(e)) => Some((Err(e), None)),
            None => Some((Ok(streamed.finish()), None)),
        }
    });
    let body = futures_util::stream::once(async move { Ok::<Vec<u8>, worker::Error>(first) }).chain(rest);
    Ok(Response::from_stream(body)?.with_status(status).with_headers(headers))
}
