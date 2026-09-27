//! Every message between the caller (pipe server) and the helper (pipe client).
//!
//! A session: helper → [`Hello`]; caller → [`Welcome`]; then any number of
//! caller → [`Request`] / helper → [`Event`]* → [`OperationResult`] (or [`ErrorInfo`]) rounds,
//! with caller → [`Decision`] allowed while a countdown or a confirmation wait runs; caller →
//! `Bye` or a closed pipe ends it. See section E of docs/design/m2-engine.md.
//!
//! This module defines only what the pipe allows a caller to ask for. Events, decisions, results
//! and request options are `mklm_core::report` types (re-exported here); the helper maps each
//! [`Request`] explicitly onto an engine call (design review S5), so that nothing the engine can do
//! is reachable over the pipe unless it is listed here. In particular the silent
//! restore-to-baseline of the uninstall custom action is not.
//!
//! The types defined here reject unknown fields: caller and helper ship together (same
//! [`crate::PROTOCOL_VERSION`] and build ID), so a field this build does not know is never
//! legitimate, and a request is not silently narrowed to what the helper understood (a
//! `"silent": true` on a restore request is an error, not ignored).

use mklm_core::{Layout, LayoutChoice, OpId, RestoreScope};
use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;
use crate::args::Nonce;

pub use mklm_core::report::{
    ApplyOptions, ConflictInfo, ConflictPolicy, Decision, ErrorCode, ErrorInfo, Event,
    ExpectedKeyboard, ExpectedPlan, OperationResult, Outcome, RecoveredOp, ResolutionChoice,
    ValueChoice,
};

/// Helper → caller, first frame after connecting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol: u32,
    /// The nonce from the helper's command line, hex.
    pub nonce_hex: String,
    pub helper_pid: u32,
    /// `CARGO_PKG_VERSION` of the helper, for logs.
    pub helper_version: String,
    /// Build ID embedded by `build.rs` in both executables: the package version and a hash of the
    /// sources of `mklm-core`, `mklm-ipc`, `mklm-engine` and `mklm-win`. The caller requires an
    /// exact match (design review S11); it also reads the same ID from the helper file's
    /// VERSIONINFO before launching it, so that a stale helper fails before the UAC prompt.
    pub build_id: String,
}

impl Hello {
    /// The helper's greeting for this build's [`PROTOCOL_VERSION`].
    pub fn new(nonce: &Nonce, helper_pid: u32, helper_version: &str, build_id: &str) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            nonce_hex: nonce.to_hex(),
            helper_pid,
            helper_version: helper_version.to_string(),
            build_id: build_id.to_string(),
        }
    }
}

/// Caller → helper, answer to [`Hello`] once the caller checked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    pub protocol: u32,
    pub caller_pid: u32,
    /// The caller's build ID; the helper requires an exact match too.
    pub build_id: String,
}

impl Welcome {
    /// The caller's answer for this build's [`PROTOCOL_VERSION`].
    pub fn new(caller_pid: u32, build_id: &str) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            caller_pid,
            build_id: build_id.to_string(),
        }
    }
}

/// Frames the caller sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum CallerMessage {
    Welcome(Welcome),
    Request(Request),
    Decision(Decision),
    /// Ends the session; the helper exits.
    Bye,
}

/// Frames the helper sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum HelperMessage {
    Hello(Hello),
    Event(Event),
    Result(OperationResult),
    /// The request failed before or without a journal state change worth a result.
    Error(ErrorInfo),
}

/// What the caller asks the helper to do. The helper re-enumerates the devices itself and trusts no
/// data from the caller beyond these fields (plan 2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    SetLayout(SetLayoutRequest),
    Migrate(MigrateRequest),
    Revert {
        op_id: OpId,
        apply: ApplyOptions,
    },
    /// Keep an operation in `AwaitingConfirm` (or in `PendingReboot` seen from a new boot).
    Confirm {
        op_id: OpId,
    },
    RestoreBaseline(RestoreBaselineRequest),
    /// Recover every eligible journal entry (plan 2.3). With `apply` allowing it, keyboards whose
    /// recovered values are not in effect are reset afterwards (design review C1).
    Recover {
        apply: ApplyOptions,
    },
    /// Undo every open entry that is not in flight (`AwaitingConfirm`, `PendingReboot`,
    /// `Conflict`), newest first, with compare-and-swap (design review C7).
    Undo {
        apply: ApplyOptions,
    },
    ResolveConflict(ResolveConflictRequest),
}

/// Assign a layout to one keyboard (and the other collections of its physical device).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetLayoutRequest {
    pub instance_id: String,
    pub layout: LayoutChoice,
    pub apply: ApplyOptions,
    /// What the user approved in the caller's dry run; see [`ExpectedPlan`].
    pub expected: Option<ExpectedPlan>,
}

/// One extra assignment made together with a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub instance_id: String,
    pub layout: LayoutChoice,
}

/// Fixed mode → per-keyboard mode (plan 1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrateRequest {
    /// The PC's standard layout after the migration (`LayerDriver JPN` / identifier).
    pub standard: Layout,
    pub assignments: Vec<Assignment>,
    /// See [`SetLayoutRequest::expected`].
    pub expected: Option<ExpectedPlan>,
}

/// "MKLM 導入前に戻す", interactive. (The silent variant exists only on the helper's fixed
/// `--uninstall-restore` command line; a `silent` field here is rejected as unknown.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBaselineRequest {
    pub scope: RestoreScope,
    pub on_conflict: ConflictPolicy,
    pub apply: ApplyOptions,
}

/// Resolve an operation in `Conflict`. Records without a choice keep their current value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveConflictRequest {
    pub op_id: OpId,
    pub choices: Vec<ValueChoice>,
    pub apply: ApplyOptions,
}
