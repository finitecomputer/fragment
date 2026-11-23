//! The bridge a computer image runs (docs/computers.md): an agent runtime on
//! one side (Hermes' Relay, or a scripted stub), the ordinary fragment API
//! on the other. `engine` holds its rules, pure; `driver` its I/O; `store`
//! its state on disk (a snapshot and a log of each step's change); `runtime`
//! the runtimes; `api` every route it calls; `ready` which agents the image
//! has made ready, when it says; `note` what a turn after a cut one is told,
//! from the journal; `screen` each agent's screen, `screens` the image's
//! map of their displays, `lease` who drives one; `simplex` an agent's link
//! to its owner's SimpleX app (groundwork: nothing runs it yet); `transfer`
//! a file's bytes, streamed, through a bounded `scratch` folder.

pub mod answer;
pub mod api;
pub mod cards;
pub mod driver;
pub mod engine;
pub mod history;
pub mod lease;
pub mod limits;
pub mod load;
pub mod log;
pub mod net;
pub mod note;
pub mod ready;
pub mod records;
pub mod runtime;
pub mod scratch;
pub mod screen;
pub mod screens;
pub mod seen;
pub mod simplex;
pub mod store;
pub mod transfer;
