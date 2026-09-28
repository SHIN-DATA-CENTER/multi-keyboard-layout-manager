//! Journaled write transactions of Multi Keyboard Layout Manager (MKLM): set a layout, migrate
//! from fixed to per-keyboard mode, revert, undo, restore to baseline, recover and confirm
//! (plan 2.3); and, since M3, delete the values a keyboard's driver does not read and save the
//! machine-wide settings (design m3 A.5). [`session_end`] reverts a running countdown when the
//! Windows session ends.
//!
//! The engine is shared by `mklm-helper` (elevated, driven over the pipe; every `mklm-cli` and GUI
//! write goes through it) and, as a maintenance fallback, `mklm-cli --in-process`; later the
//! service. It talks to the system only through three traits, so that every step can be tested
//! with fault injection:
//! - [`RegistryBackend`]: device hardware keys, the global i8042prt key and the journal store;
//! - [`DeviceController`]: keyboard inventory, live reset, re-arrival and reported type;
//! - [`Host`]: write lock, clocks, IDs, boot ID, process liveness, System32 checks and recovery
//!   files.
//!
//! Its inputs are [`params`] types and its outputs `mklm_core::report` types; it does not depend on
//! the pipe protocol (`mklm-ipc`). [`memory`] has in-memory implementations with crash injection;
//! `win` (Windows only) has the real ones on top of `mklm-win`. Design: docs/design/m2-engine.md.

#![forbid(unsafe_code)]

pub mod backend;
pub mod device;
pub mod engine;
pub mod error;
pub mod host;
pub mod memory;
pub mod params;
pub mod session_end;
pub mod sink;
pub mod uninstall;
#[cfg(windows)]
pub mod win;

pub use backend::{BackendError, JournalDump, JournalSlot, RegistryBackend};
pub use device::{Arrival, DeviceController, DeviceError, Inventory, RestartOutcome};
pub use engine::{Engine, EngineConfig};
pub use error::EngineError;
pub use host::{Host, HostError};
pub use params::{
    CleanupParams, MachineSettingsParams, MigrateParams, ResolveParams, RestoreBaselineParams,
    RestoreMode, SetLayoutParams,
};
pub use session_end::{SessionEnd, SessionEndSink};
pub use sink::{DecisionPoll, EventSink, NullSink};
