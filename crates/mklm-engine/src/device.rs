//! Keyboards as devices: inventory, live reset, re-arrival and the type the driver reports
//! (section B.2 of the design doc).

use std::time::Duration;

use mklm_core::{KeyboardDevice, KeyboardType};

/// Result of a live reset request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestartOutcome {
    /// `DIF_PROPERTYCHANGE` / `DICS_PROPCHANGE` returned and asked for nothing more.
    Restarted,
    /// Windows set `DI_NEEDREBOOT` or `DI_NEEDRESTART`: the change needs a PC restart.
    NeedsReboot,
    /// The class installer call did not return within the implementation's deadline (it runs on
    /// its own thread; design review C18). Treated like `NeedsReboot`. The implementation keeps
    /// the thread and the process waits for it before exiting, so the devnode is never left
    /// half-restarted by a process exit.
    TimedOut,
}

/// Result of waiting for a keyboard to come back after a reset or a reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arrival {
    /// Present, `DN_STARTED`, no problem code. `reported` is what Raw Input reports, if it lists
    /// the keyboard already.
    Started {
        reported: Option<KeyboardType>,
    },
    TimedOut,
}

/// The keyboards, and what could not be read without making a write unsafe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub keyboards: Vec<KeyboardDevice>,
    /// Read issues that do not block writes (design review S2, C3): names, topology, Raw Input,
    /// values (the engine re-reads every value through the `RegistryBackend`), and status or
    /// container (the affected field is `None`, which bans a live reset of that keyboard).
    pub warnings: Vec<String>,
}

/// A device call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    /// The set of Keyboard-class devnodes, or the driver or presence of one of them, could not be
    /// established. INV-PS2 and the allowlist depend on these, so nothing may be written.
    #[error("device enumeration incomplete: {issues:?}")]
    Incomplete { issues: Vec<String> },
    #[error("{instance_id}: no such keyboard")]
    NotFound { instance_id: String },
    #[error("{what}: error {code}")]
    Os { what: String, code: u32 },
    /// Fault injection.
    #[error("injected device failure")]
    Injected,
}

/// Engine-facing device access. Implementations: `memory::FakeDevices` and `win::WinDevices`.
pub trait DeviceController {
    /// Every Keyboard-class devnode, phantoms included (INV-PS2 needs them), with topology, driver,
    /// transport, status and reported type. `overrides` may be stale: the engine re-reads every
    /// value through [`crate::RegistryBackend`], the single source of truth for values.
    /// Fails with [`DeviceError::Incomplete`] only for write-blocking read issues
    /// (`mklm_win::ReadIssueKind::blocks_writes`); the others become [`Inventory::warnings`].
    fn keyboards(&mut self) -> Result<Inventory, DeviceError>;

    /// Restarts one devnode in place (`DIF_PROPERTYCHANGE` with `DICS_PROPCHANGE`, plan 1.4). The
    /// engine calls it only for keyboards that passed `mklm_core::live_reset_bans`, one at a time.
    /// Bounded by a deadline ([`RestartOutcome::TimedOut`]).
    fn restart(&mut self, instance_id: &str) -> Result<RestartOutcome, DeviceError>;

    /// Polls until the devnode is started again (and listed by Raw Input), or `timeout`.
    fn wait_for_arrival(
        &mut self,
        instance_id: &str,
        timeout: Duration,
    ) -> Result<Arrival, DeviceError>;

    /// Type/subtype Raw Input reports now; `None` when the keyboard is not listed.
    fn reported_type(&mut self, instance_id: &str) -> Result<Option<KeyboardType>, DeviceError>;
}
