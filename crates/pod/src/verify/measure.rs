//! The RTMR1 and RTMR2 a correct boot of the pod image must produce,
//! computed from the image's files alone (`attest::expect`), and
//! optionally checked against a quote and lined up against a boot's event
//! log, event by event. Serialisable, for a command to print or save.

use anyhow::{Context, Result};
use crate::attest::expect::{self, Expected};
use crate::attest::{describe, eventlog, report};
use serde::Serialize;

#[derive(Serialize)]
pub struct EventLine {
    pub what: String,
    pub origin: &'static str,
    pub digest: String,
    /// With a log: whether the log's event at this position has this
    /// digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_log: Option<bool>,
}

#[derive(Serialize)]
pub struct Register {
    pub rtmr: usize,
    pub expected: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches_quote: Option<bool>,
    pub events: Vec<EventLine>,
    /// With a log: its events for this register that the prediction does
    /// not have, described.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unpredicted: Vec<String>,
}

pub struct Inputs<'a> {
    pub uki: &'a [u8],
    pub disk_head: &'a [u8],
    pub log: Option<&'a [u8]>,
    pub quote: Option<&'a [u8]>,
}

pub fn build(i: &Inputs<'_>) -> Result<Vec<Register>> {
    let mut events = expect::rtmr1(i.uki, i.disk_head).context("predicting RTMR1")?;
    events.extend(expect::rtmr2(i.uki).context("predicting RTMR2")?);
    let log = i.log.map(eventlog::parse).transpose().context("parsing the event log")?;
    let quote = i.quote.map(report::parse).transpose().context("reading the quote")?;
    Ok([1, 2]
        .into_iter()
        .map(|rtmr| {
            let mine: Vec<&Expected> = events.iter().filter(|e| e.rtmr == rtmr).collect();
            let logged: Vec<&eventlog::Event> = log
                .as_ref()
                .map(|l| l.events.iter().filter(|e| e.rtmr() == Some(rtmr)).collect())
                .unwrap_or_default();
            let expected = expect::fold(&events, rtmr);
            let lines = mine
                .iter()
                .enumerate()
                .map(|(n, e)| EventLine {
                    what: e.what.clone(),
                    origin: e.origin.label(),
                    digest: hex::encode(e.digest),
                    in_log: log.as_ref().map(|_| logged.get(n).and_then(|l| l.sha384) == Some(e.digest)),
                })
                .collect();
            let unpredicted = logged
                .iter()
                .filter(|l| !mine.iter().any(|e| Some(e.digest) == l.sha384))
                .map(|l| format!("#{} {} {}", l.index, describe::type_name(l.event_type), describe::describe(l)))
                .collect();
            Register {
                rtmr,
                expected: hex::encode(expected),
                quote: quote.as_ref().map(|q| hex::encode(q.rtmr[rtmr])),
                matches_quote: quote.as_ref().map(|q| q.rtmr[rtmr] == expected),
                events: lines,
                unpredicted,
            }
        })
        .collect())
}
