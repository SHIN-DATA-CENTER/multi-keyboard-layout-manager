//! Vocabulary shared by the engine and everything that drives it: request options, progress events,
//! the user's decisions and the results (design sections B, D and E.6).
//!
//! Pure data. `mklm-engine` produces and consumes these; `mklm-ipc` carries them over the pipe;
//! the GUI and CLI show them. They live here rather than in `mklm-ipc` so that the engine does not
//! depend on the wire protocol, and so that the wire can only expose what `mklm-ipc` chooses to
//! (design review S5: the silent uninstall mode is not reachable through the pipe).

use serde::{Deserialize, Serialize};

use crate::allowlist::{PlanError, PlanStep};
use crate::journal::{FailureReason, OpId, OpState, RegValue};
use crate::layout::{LayoutTable, PendingAction};
use crate::model::KeyboardType;
use crate::safety::InvPs2Violation;

/// Length of the keep-or-revert countdown after a live reset (plan 3.5, design m2 D.2 a): the
/// default, and what the CLI always uses.
pub const DEFAULT_COUNTDOWN_SECONDS: u32 = 20;
/// The longer countdown of the GUI setting "確認の時間を長くする（60 秒）" (design m3 B.14, K.16,
/// WP-E3): time to listen to the screen reader, type and tab to the button.
pub const LONG_COUNTDOWN_SECONDS: u32 = 60;
/// The only countdown lengths the engine accepts; any other is refused (`PlanRejected`).
pub const COUNTDOWN_SECONDS_CHOICES: [u32; 2] = [DEFAULT_COUNTDOWN_SECONDS, LONG_COUNTDOWN_SECONDS];

fn default_countdown_seconds() -> u32 {
    DEFAULT_COUNTDOWN_SECONDS
}

/// Whether a request may restart keyboards in place (plan 1.4), and how long the keep-or-revert
/// countdown after such a reset runs. Every request that can end with a live reset carries it:
/// set, revert, restore, conflict resolution, recover and undo (design review C9). The default
/// (both false) never resets: the change then waits for a reconnect or a PC restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApplyOptions {
    /// False: never reset a keyboard in place (CLI `--no-reset`).
    pub allow_live_reset: bool,
    /// The caller saw recent input from another keyboard, or the user confirmed another way to
    /// type. When false the target counts as the only usable keyboard (plan 1.4) and is not reset.
    pub other_input_available: bool,
    /// Seconds of the countdown after a live reset: [`DEFAULT_COUNTDOWN_SECONDS`] or
    /// [`LONG_COUNTDOWN_SECONDS`], nothing else ([`ApplyOptions::countdown_allowed`]; design m3
    /// WP-E3). Missing on the wire means the default.
    #[serde(default = "default_countdown_seconds")]
    pub countdown_seconds: u32,
}

impl Default for ApplyOptions {
    /// No reset, the default countdown.
    fn default() -> Self {
        Self {
            allow_live_reset: false,
            other_input_available: false,
            countdown_seconds: DEFAULT_COUNTDOWN_SECONDS,
        }
    }
}

impl ApplyOptions {
    /// True when [`ApplyOptions::countdown_seconds`] is one of [`COUNTDOWN_SECONDS_CHOICES`].
    pub fn countdown_allowed(&self) -> bool {
        COUNTDOWN_SECONDS_CHOICES.contains(&self.countdown_seconds)
    }
}

/// What the user approved in the unelevated dry run. When the engine's own plan differs in any
/// step or in how the change takes effect, the request fails with [`ErrorCode::PlanChanged`] and
/// nothing is written (design review S6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedPlan {
    pub steps: Vec<PlanStep>,
    pub apply: Option<PendingAction>,
}

/// What to do with a value whose current content is not what MKLM last wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictPolicy {
    /// Stop at `Conflict` and report the three values (interactive default).
    Report,
    /// Leave the value alone and log it (silent mode, plan 2.3).
    Skip,
    /// Write the baseline anyway (the user already chose "use baseline" for every conflict).
    Overwrite,
}

/// The user's choice for one value of an operation in `Conflict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionChoice {
    /// Leave the current value; MKLM records it as what it last saw there.
    KeepCurrent,
    UseBefore,
    /// MKLM's `intended`.
    UseIntended,
    UseBaseline,
}

/// Choice for one record of the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueChoice {
    /// Index into the operation's records (as in [`ConflictInfo::record`]).
    pub record: usize,
    pub choice: ResolutionChoice,
}

/// The user's answer during a countdown or a confirmation wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Decision {
    Keep { op_id: OpId },
    RevertNow { op_id: OpId },
}

/// What one keyboard should report / type once the change is in effect (for the confirmation UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedKeyboard {
    pub instance_id: String,
    pub display_name: String,
    /// Type the driver should report (Raw Input), e.g. 0x4/0x0; `None` for other drivers.
    pub expected_type: Option<KeyboardType>,
    /// Layout it types with afterwards (Japanese IME assumed).
    pub layout_after: Option<LayoutTable>,
    /// True when this keyboard's layout changes (a migration also lists unchanged keyboards).
    pub changes: bool,
}

/// Progress of a running request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Event {
    /// The write lock is held.
    Locked,
    /// The operation is journaled (`Planned`, flushed); nothing is written yet.
    Planned {
        op_id: OpId,
        steps: Vec<PlanStep>,
        apply: Option<PendingAction>,
        keyboards: Vec<ExpectedKeyboard>,
    },
    /// A state transition was flushed.
    StateChanged {
        op_id: OpId,
        state: OpState,
    },
    /// A step's values were written and flushed.
    StepWritten {
        op_id: OpId,
        step: usize,
        of: usize,
    },
    ResettingKeyboard {
        instance_id: String,
    },
    KeyboardArrived {
        instance_id: String,
        reported: Option<KeyboardType>,
        expected: KeyboardType,
    },
    /// Keep-or-revert countdown after a live reset; no answer means revert.
    CountdownStarted {
        op_id: OpId,
        seconds: u32,
        /// Raw Input reports the intended type (plan gate G3).
        verified: bool,
    },
    CountdownTick {
        op_id: OpId,
        remaining: u32,
    },
    /// Reconnect path: waiting for the keyboard to come back; then a [`Decision`] is awaited
    /// without a countdown.
    WaitingForReconnect {
        op_id: OpId,
        instance_ids: Vec<String>,
    },
    RecoveryAssetsWritten {
        directory: String,
        skipped: usize,
    },
    Warning {
        message: String,
    },
    /// Sent by the helper's writer thread every 10 s whatever the engine is doing (design review
    /// S7). The engine itself never emits it.
    Heartbeat,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Every value already had the requested content; nothing was journaled.
    NoChange,
    Confirmed,
    /// Waiting for keep/revert (reconnect path, or a later confirm).
    AwaitingConfirm,
    PendingReboot,
    /// Values are back at `before`. With [`OperationResult::failure`] set, the revert was not the
    /// user's request (rollback after an error, an expired countdown, a disconnect).
    Reverted,
    RevertedPendingReboot,
    /// Closed without its change ever taking effect (see [`OperationResult::failure`]).
    Failed,
    Conflict,
    /// A `Recover` or `Undo` request finished (see [`OperationResult::recovered`]).
    Recovered,
}

/// A value in conflict: the values plan 2.3 shows the user, plus the recorded ends of the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub op_id: OpId,
    /// Index into the operation's records.
    pub record: usize,
    pub key_path: String,
    pub name: String,
    pub baseline: RegValue,
    pub before: RegValue,
    pub intended: RegValue,
    pub last_written: Option<RegValue>,
    pub current: RegValue,
    /// Set when the conflict is a write that kept failing (design review C3), not an outside
    /// change: the error text of the last attempt.
    pub write_error: Option<String>,
}

/// One entry a `Recover` or `Undo` request acted on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredOp {
    pub op_id: OpId,
    pub from: OpState,
    pub to: OpState,
    /// The decision taken, e.g. `"roll-back"`, `"roll-forward"`, `"reboot-observed"`, `"undo"`.
    pub decision: String,
}

/// Final answer to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationResult {
    /// The operation created or acted on; `None` for `NoChange`, `Recover` and `Undo`.
    pub op_id: Option<OpId>,
    pub outcome: Outcome,
    /// Why the operation did not end as requested (rollback, supersede, nothing written).
    pub failure: Option<FailureReason>,
    /// What the user still has to do for the stored values to take effect. Also persisted in the
    /// journal ([`crate::JournalEntry::apply_pending`]) so that it survives the process (design
    /// review C1). For [`PendingAction::RestartPc`] the caller registers the post-reboot RunOnce
    /// entry and offers the restart (the helper never restarts the PC).
    pub pending_action: Option<PendingAction>,
    pub conflicts: Vec<ConflictInfo>,
    /// Set when the operation stopped at `Conflict` because the values as they are now break
    /// INV-PS2 (design review C12); lists the i8042prt keyboards without a pin.
    pub inv_ps2_violation: Option<InvPs2Violation>,
    pub recovered: Vec<RecoveredOp>,
    pub warnings: Vec<String>,
}

/// Machine-readable error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// Another MKLM process holds the write lock.
    Busy,
    /// An open operation blocks new ones (plan 2.2).
    OpInProgress,
    /// An entry needs `Recover` first.
    RecoveryNeeded,
    /// The journal has entries this build cannot read; writes stop.
    JournalUnreadable,
    /// Allowlist or INV-PS2 (see [`ErrorInfo::plan_error`]).
    PlanRejected,
    /// The engine's plan differs from the approved [`ExpectedPlan`].
    PlanChanged,
    MigrationRequired,
    NotFixedMode,
    UnknownKeyboard,
    UnknownOp,
    /// `Revert` of an operation a later operation overwrote; use restore-to-baseline.
    NotLatest,
    /// The operation is not in a state that allows the request.
    InvalidState,
    /// The device enumeration could not establish every Keyboard-class devnode, its driver and
    /// its presence; nothing may be written.
    InventoryIncomplete,
    /// The offline recovery files could not be written durably before a change of boot-time
    /// values (design review C10); nothing was written.
    RecoveryAssetsUnavailable,
    /// The caller went away before anything was journaled; nothing was written.
    Cancelled,
    Registry,
    Device,
    /// Lock, protected directory, process or clock failures.
    Host,
    Protocol,
    Internal,
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    /// English, for logs and the CLI.
    pub message: String,
    pub op_id: Option<OpId>,
    pub plan_error: Option<PlanError>,
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use serde::de::DeserializeOwned;

    use super::*;
    use crate::allowlist::{AllowlistError, PlannedWrite, WriteTarget};
    use crate::model::value_names;

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) -> String {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "{json}");
        json
    }

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    fn keychron() -> String {
        r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".to_string()
    }

    fn step() -> PlanStep {
        PlanStep {
            target: WriteTarget::Device {
                instance_id: keychron(),
            },
            writes: vec![
                PlannedWrite::set(value_names::HID_TYPE, 7),
                PlannedWrite::set(value_names::HID_SUBTYPE, 2),
            ],
        }
    }

    fn violation() -> InvPs2Violation {
        InvPs2Violation {
            keyboards: vec![r"ACPI\PNP0303\4&1&0".to_string()],
        }
    }

    #[test]
    fn options_plans_and_choices() {
        for allow_live_reset in [false, true] {
            for other_input_available in [false, true] {
                for countdown_seconds in [20, 60, 7] {
                    round_trip(&ApplyOptions {
                        allow_live_reset,
                        other_input_available,
                        countdown_seconds,
                    });
                }
            }
        }
        assert_eq!(
            round_trip(&ApplyOptions::default()),
            r#"{"allow_live_reset":false,"other_input_available":false,"countdown_seconds":20}"#
        );
        // Without the field (a caller of protocol 1's shape): the default countdown.
        let old: ApplyOptions =
            serde_json::from_str(r#"{"allow_live_reset":true,"other_input_available":true}"#)
                .unwrap();
        assert_eq!(old.countdown_seconds, DEFAULT_COUNTDOWN_SECONDS);
        // Only 20 and 60 (design m3 WP-E3).
        for (seconds, allowed) in [(20, true), (60, true), (0, false), (15, false), (61, false)] {
            let options = ApplyOptions {
                countdown_seconds: seconds,
                ..ApplyOptions::default()
            };
            assert_eq!(options.countdown_allowed(), allowed, "{seconds}");
        }
        assert!(ApplyOptions::default().countdown_allowed());
        round_trip(&ExpectedPlan {
            steps: vec![step()],
            apply: Some(PendingAction::ResetKeyboard),
        });
        round_trip(&ExpectedPlan {
            steps: vec![],
            apply: None,
        });
        for policy in [
            ConflictPolicy::Report,
            ConflictPolicy::Skip,
            ConflictPolicy::Overwrite,
        ] {
            round_trip(&policy);
        }
        for choice in [
            ResolutionChoice::KeepCurrent,
            ResolutionChoice::UseBefore,
            ResolutionChoice::UseIntended,
            ResolutionChoice::UseBaseline,
        ] {
            round_trip(&ValueChoice { record: 3, choice });
        }
        assert_eq!(
            round_trip(&ResolutionChoice::KeepCurrent),
            "\"keep-current\""
        );
        assert_eq!(
            round_trip(&Decision::Keep { op_id: op() }),
            r#"{"kind":"keep","op_id":"3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"}"#
        );
        round_trip(&Decision::RevertNow { op_id: op() });
    }

    #[test]
    fn events() {
        let events = vec![
            Event::Locked,
            Event::Planned {
                op_id: op(),
                steps: vec![step()],
                apply: Some(PendingAction::RestartPc),
                keyboards: vec![
                    ExpectedKeyboard {
                        instance_id: keychron(),
                        display_name: "Keychron Receiver".into(),
                        expected_type: Some(KeyboardType::JIS),
                        layout_after: Some(LayoutTable::Jis),
                        changes: true,
                    },
                    // Synthetic ID (not what Windows uses); only the JSON shape matters here.
                    ExpectedKeyboard {
                        instance_id: r"TS_INPT\TS_KBD\1".into(),
                        display_name: "RDP".into(),
                        expected_type: None,
                        layout_after: Some(LayoutTable::Other("kbdnec.dll".into())),
                        changes: false,
                    },
                ],
            },
            Event::StateChanged {
                op_id: op(),
                state: OpState::RevertedPendingReboot,
            },
            Event::StepWritten {
                op_id: op(),
                step: 1,
                of: 3,
            },
            Event::ResettingKeyboard {
                instance_id: keychron(),
            },
            Event::KeyboardArrived {
                instance_id: keychron(),
                reported: None,
                expected: KeyboardType::US,
            },
            Event::CountdownStarted {
                op_id: op(),
                seconds: 20,
                verified: true,
            },
            Event::CountdownTick {
                op_id: op(),
                remaining: 7,
            },
            Event::WaitingForReconnect {
                op_id: op(),
                instance_ids: vec![keychron()],
            },
            Event::RecoveryAssetsWritten {
                directory: r"C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery".into(),
                skipped: 1,
            },
            Event::Warning {
                message: "標準に従う設定は未確認です".into(),
            },
            Event::Heartbeat,
        ];
        for event in &events {
            round_trip(event);
        }
        assert_eq!(round_trip(&Event::Heartbeat), r#"{"kind":"heartbeat"}"#);
    }

    #[test]
    fn results_and_errors() {
        for outcome in [
            Outcome::NoChange,
            Outcome::Confirmed,
            Outcome::AwaitingConfirm,
            Outcome::PendingReboot,
            Outcome::Reverted,
            Outcome::RevertedPendingReboot,
            Outcome::Failed,
            Outcome::Conflict,
            Outcome::Recovered,
        ] {
            round_trip(&outcome);
        }
        let conflict = ConflictInfo {
            op_id: op(),
            record: 0,
            key_path: format!(r"Enum\{}\Device Parameters", keychron()),
            name: value_names::HID_TYPE.into(),
            baseline: RegValue::Absent,
            before: RegValue::Dword { value: 4 },
            intended: RegValue::Dword { value: 7 },
            last_written: Some(RegValue::Dword { value: 7 }),
            current: RegValue::Other {
                reg_type: 1,
                data_hex: "340000".into(),
            },
            write_error: Some("access denied".into()),
        };
        round_trip(&conflict);
        round_trip(&RecoveredOp {
            op_id: op(),
            from: OpState::Planned,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        let failures = [
            FailureReason::ConcurrentChange {
                name: value_names::HID_TYPE.into(),
            },
            FailureReason::WriteError {
                message: "x".into(),
            },
            FailureReason::CallerDisconnected,
            FailureReason::Interrupted,
            FailureReason::LiveResetUnconfirmed,
            FailureReason::CountdownExpired,
            FailureReason::KeyboardDidNotReturn,
            FailureReason::NothingWritten,
            FailureReason::ConflictKeptCurrent,
            FailureReason::Superseded { by: op() },
        ];
        for failure in failures {
            round_trip(&OperationResult {
                op_id: Some(op()),
                outcome: Outcome::Reverted,
                failure: Some(failure),
                pending_action: Some(PendingAction::Reconnect),
                conflicts: vec![conflict.clone()],
                inv_ps2_violation: Some(violation()),
                recovered: vec![],
                warnings: vec!["w".into()],
            });
        }
        round_trip(&OperationResult {
            op_id: None,
            outcome: Outcome::Recovered,
            failure: None,
            pending_action: None,
            conflicts: vec![],
            inv_ps2_violation: None,
            recovered: vec![RecoveredOp {
                op_id: op(),
                from: OpState::PendingReboot,
                to: OpState::AwaitingConfirm,
                decision: "reboot-observed".into(),
            }],
            warnings: vec![],
        });
        let codes = [
            ErrorCode::Busy,
            ErrorCode::OpInProgress,
            ErrorCode::RecoveryNeeded,
            ErrorCode::JournalUnreadable,
            ErrorCode::PlanRejected,
            ErrorCode::PlanChanged,
            ErrorCode::MigrationRequired,
            ErrorCode::NotFixedMode,
            ErrorCode::UnknownKeyboard,
            ErrorCode::UnknownOp,
            ErrorCode::NotLatest,
            ErrorCode::InvalidState,
            ErrorCode::InventoryIncomplete,
            ErrorCode::RecoveryAssetsUnavailable,
            ErrorCode::Cancelled,
            ErrorCode::Registry,
            ErrorCode::Device,
            ErrorCode::Host,
            ErrorCode::Protocol,
            ErrorCode::Internal,
        ];
        for code in codes {
            round_trip(&code);
        }
        assert_eq!(
            round_trip(&ErrorCode::RecoveryAssetsUnavailable),
            "\"recovery-assets-unavailable\""
        );
        let plan_errors = [
            None,
            Some(PlanError::InvPs2(violation())),
            Some(PlanError::Device {
                instance_id: keychron(),
                error: AllowlistError::TypeNotAllowed {
                    keyboard_type: KeyboardType::new(7, 0),
                },
            }),
            Some(PlanError::Global {
                error: AllowlistError::ValueNotAllowed {
                    name: "Start".into(),
                },
            }),
            Some(PlanError::UnknownKeyboard {
                instance_id: "x".into(),
            }),
            Some(PlanError::DuplicateKeyboard {
                instance_id: "x".into(),
            }),
        ];
        for plan_error in plan_errors {
            round_trip(&ErrorInfo {
                code: ErrorCode::PlanRejected,
                message: "refused".into(),
                op_id: Some(op()),
                plan_error,
            });
        }
    }
}
