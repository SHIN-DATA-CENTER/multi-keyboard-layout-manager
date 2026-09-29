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
//! - [`operation`]: the writes of "set layout" and "migrate", and how a change takes effect (M2).
//! - [`journal`], [`recovery`], [`restore`]: journal entries, their state machine, recovery
//!   decisions and restore plans (M2, see docs/design/m2-engine.md).
//! - [`boot`]: the current boot ([`CurrentBoot`]) and how the boot IDs of 0.1.x are judged.
//! - [`recovery_assets`]: the offline recovery files (M2).
//! - [`report`]: request options, events, decisions and results shared by the engine, the pipe
//!   protocol and the UIs (M2).

#![forbid(unsafe_code)]

pub mod allowlist;
pub mod assess;
pub mod boot;
pub mod device;
pub mod global;
pub mod group;
pub mod ids;
pub mod input;
pub mod journal;
pub mod layout;
pub mod model;
pub mod operation;
pub mod recovery;
pub mod recovery_assets;
pub mod report;
pub mod restore;
pub mod safety;
pub mod transport;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
#[cfg(test)]
mod test_support;

pub use allowlist::*;
pub use assess::*;
pub use boot::*;
pub use device::*;
pub use global::*;
pub use group::*;
pub use ids::*;
pub use input::*;
pub use journal::*;
pub use layout::*;
pub use model::*;
pub use operation::*;
pub use recovery::*;
pub use recovery_assets::*;
pub use report::*;
pub use restore::*;
pub use safety::*;
pub use transport::*;
