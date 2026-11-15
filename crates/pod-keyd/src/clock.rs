//! Wall-clock time, behind a trait so tests can move it. In a TD the
//! clock is the host's to set: it decides approval windows and nonce
//! lifetimes here, and collateral freshness in pod-verify. A host that
//! moves it can open an approval's window early or keep an expired one
//! open; it cannot get past min_seq, which never goes down.

use std::sync::Arc;

pub trait Clock: Send + Sync {
    /// Unix seconds.
    fn now(&self) -> i64;
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> i64 {
        (**self).now()
    }
}

pub struct System;

impl Clock for System {
    fn now(&self) -> i64 {
        fragment_pod::verify::time::now()
    }
}
