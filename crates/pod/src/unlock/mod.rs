//! The lock-and-unlock protocol between a pod and its owner, as pure
//! code both sides link: the binding a quote's REPORTDATA carries, the
//! disk key sealed to one boot's key, and the owner's signature over
//! the request (docs/pod.md, "Lock and unlock").

pub mod binding;
pub mod keys;
pub mod release;
pub mod request;
pub mod seal;
pub mod sign;
pub mod tls;
pub mod wire;
