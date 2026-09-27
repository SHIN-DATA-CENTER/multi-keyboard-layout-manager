//! OS-independent model and rules for Multi Keyboard Layout Manager (MKLM).
//!
//! This crate contains no `unsafe` code and no Windows API calls, so every rule that decides
//! which layout a keyboard ends up with can be unit-tested on any machine.
//!
//! - [`model`]: the data read from the system (the shared contract).
//! - [`ids`], [`transport`]: parsing of PnP IDs, KLIDs and HKLs.
//! - [`global`], [`device`], [`layout`], [`input`]: which layout each keyboard types with, and why.
//! - [`safety`], [`allowlist`]: rules every write must pass (live-reset bans, INV-PS2, allowlist).
//! - [`group`], [`mod@assess`]: grouping into physical devices and the one-call [`assess()`] API.

#![forbid(unsafe_code)]

pub mod allowlist;
pub mod assess;
pub mod device;
pub mod global;
pub mod group;
pub mod ids;
pub mod input;
pub mod layout;
pub mod model;
pub mod safety;
pub mod transport;

#[cfg(test)]
mod fixtures;

pub use allowlist::*;
pub use assess::*;
pub use device::*;
pub use global::*;
pub use group::*;
pub use ids::*;
pub use input::*;
pub use layout::*;
pub use model::*;
pub use safety::*;
pub use transport::*;
