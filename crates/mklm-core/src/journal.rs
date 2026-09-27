//! Journal of MKLM's registry writes (plan 2.3): entry types, the operation state machine and the
//! per-value records that make every write revertible and every crash recoverable.
//!
//! Pure data and rules only. `mklm-engine` stores one [`JournalEntry`] per operation as JSON in a
//! `REG_SZ` value under [`JOURNAL_OPS_KEY`], and one [`BaselineRecord`] per registry value under
//! [`JOURNAL_BASELINES_KEY`] (docs/design/m2-engine.md, section C).
//!
//! Vocabulary used throughout:
//! - `baseline`: the value before MKLM changed it for the first time ever ("value absent" included).
//! - `before`: the value right before *this* operation wrote it. Reverting an operation restores it.
//! - `intended`: the value this operation writes.
//! - `last_written`: the value this operation last wrote (`None` until its writes are flushed).

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::allowlist::{ValueOp, WriteTarget};
use crate::layout::PendingAction;
use crate::model::Layout;

/// Version of the [`JournalEntry`] JSON schema this build writes. Readers accept this version and
/// migrate older ones; an entry with a newer version makes the journal unreadable (writes stop).
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;

/// MKLM's machine-wide key, relative to `HKEY_LOCAL_MACHINE`. Not an MSI component (plan 2.2).
pub const MKLM_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM";
/// Journal root, relative to `HKEY_LOCAL_MACHINE`. Holds [`STORE_VERSION_VALUE`].
pub const JOURNAL_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Journal";
/// One `REG_SZ` per operation: value name = [`OpId`], data = [`JournalEntry`] JSON.
pub const JOURNAL_OPS_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Journal\Ops";
/// One `REG_SZ` per registry value MKLM ever changed: value name = [`ValueKey::canonical`],
/// data = [`BaselineRecord`] JSON.
pub const JOURNAL_BASELINES_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Journal\Baselines";
/// `REG_DWORD` under [`JOURNAL_KEY`]: layout version of the store (keys and value naming).
pub const STORE_VERSION_VALUE: &str = "StoreVersion";
/// Store layout this build reads and writes.
pub const STORE_VERSION: u32 = 1;

/// Identifier of one operation: a random UUID, lower-case, hyphenated, without braces
/// (`0f8c2d4e-...`). Also the value name of the entry under [`JOURNAL_OPS_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpId(String);

impl OpId {
    /// Accepts exactly `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
    pub fn parse(text: &str) -> Result<Self, JournalError> {
        todo!("M2: validate the UUID shape")
    }

    /// The canonical text form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First eight hex digits, as the CLI prints them. The CLI accepts any unique prefix of 8+ digits.
    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

impl std::fmt::Display for OpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for OpId {
    type Error = JournalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<OpId> for String {
    fn from(id: OpId) -> Self {
        id.0
    }
}

/// Identifies one boot of Windows: the boot identifier GUID the loader generates for every boot
/// (`NtQuerySystemInformation(SystemBootEnvironmentInformation).BootIdentifier`), which no clock
/// adjustment moves (design review C2; the kernel boot *time* shifts with time corrections and is
/// only recorded as a diagnostic, [`TransitionRecord::boot_time_hint`]). A Fast Startup "shutdown"
/// must keep it, which is what MKLM needs: the drivers were not re-initialized either. Both
/// properties are verified on the machine (design H.2, R9 and R10) before M2 is done.
///
/// JSON form: the lower-case hyphenated GUID without braces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BootId(pub u128);

impl BootId {
    /// Accepts exactly `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
    pub fn parse(text: &str) -> Result<Self, JournalError> {
        todo!("M2: parse the GUID text")
    }

    /// The canonical text form (see the type docs).
    pub fn to_text(self) -> String {
        todo!("M2: format the GUID text")
    }
}

impl TryFrom<String> for BootId {
    type Error = JournalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<BootId> for String {
    fn from(id: BootId) -> Self {
        id.to_text()
    }
}

/// Wall-clock time in milliseconds since the Unix epoch (UTC). For display and pruning only; no
/// decision depends on the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

/// A process, identified robustly against PID reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Creation time from `GetProcessTimes` (FILETIME units).
    pub creation_time: u64,
}

/// Whether the process that owns an entry still runs. Only the unelevated start-up check
/// ([`crate::attention`]) uses it; the engine, which holds the write lock, never needs it
/// (design review C3: every in-flight entry found under the lock is abandoned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Liveness {
    /// A process with the same PID and creation time exists.
    Alive,
    /// No such process (or the boot changed).
    Dead,
    /// The process table could not be read (the Windows implementation reads it with
    /// `NtQuerySystemInformation(SystemProcessInformation)`, which needs no access to the process,
    /// so this is rare). Treated like [`Liveness::Dead`] by `attention`: the helper then takes the
    /// lock, and a live owner answers with `Busy`.
    Unknown,
}

/// A registry value as MKLM records it. [`RegValue::Absent`] is a value, not an error: a baseline of
/// "no value" is restored by deleting the value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RegValue {
    /// The value does not exist.
    Absent,
    /// `REG_DWORD`.
    Dword { value: u32 },
    /// `REG_SZ`.
    Sz { value: String },
    /// Any other type or size (e.g. a `REG_SZ` "4" someone put into `KeyboardTypeOverride`), kept
    /// byte for byte so that a baseline can be restored exactly as found. MKLM never writes this
    /// except when restoring such a baseline.
    Other {
        /// `REG_*` type code.
        reg_type: u32,
        /// Raw data, lower-case hex without separators.
        data_hex: String,
    },
}

/// Equality MKLM uses for the value named `name`, everywhere it compares values (recovery's
/// observation, compare-and-swap, "before equals intended, nothing to record"): exact, except that
/// the `REG_SZ` values `LayerDriver JPN` and `OverrideKeyboardIdentifier` compare ASCII
/// case-insensitively, as the M1 rules already read them (design review C16: Settings or Windows
/// may rewrite `kbd106.dll` as `KBD106.DLL`; that is not a conflict).
pub fn value_eq(name: &str, a: &RegValue, b: &RegValue) -> bool {
    todo!("M2: exact, or case-insensitive Sz for the two global string names")
}

impl From<&ValueOp> for RegValue {
    fn from(op: &ValueOp) -> Self {
        match op {
            ValueOp::Set(value) => RegValue::Dword { value: *value },
            ValueOp::SetString(value) => RegValue::Sz {
                value: value.clone(),
            },
            ValueOp::Delete => RegValue::Absent,
        }
    }
}

/// One registry value: the key it lives in and its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ValueKey {
    pub target: WriteTarget,
    pub name: String,
}

impl ValueKey {
    /// Stable text form, used as the value name under [`JOURNAL_BASELINES_KEY`]:
    /// `device|<INSTANCE ID IN UPPER CASE>|<value name>` or `global|<value name>`.
    /// Instance IDs compare case-insensitively, hence the upper case.
    pub fn canonical(&self) -> String {
        todo!("M2: canonical key text")
    }

    /// Key path relative to a control set (`CurrentControlSet` or `ControlSet00N`):
    /// `Enum\<instance id>\Device Parameters` or `Services\i8042prt\Parameters`. Recorded for the
    /// offline recovery files; the engine itself never opens `Enum` paths (plan 1.2).
    pub fn key_path(&self) -> String {
        todo!("M2: control-set relative key path")
    }
}

/// What one operation does to one registry value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRecord {
    pub target: WriteTarget,
    /// See [`ValueKey::key_path`].
    pub key_path: String,
    pub name: String,
    /// Value before MKLM's first change ever (copied from the [`BaselineRecord`]).
    pub baseline: RegValue,
    /// Value right before this operation; reverting the operation restores it.
    pub before: RegValue,
    /// Value this operation writes.
    pub intended: RegValue,
    /// Value this operation last wrote and flushed; `None` until then. After a revert it equals
    /// `before`. Compare-and-swap expects the current value to equal it.
    pub last_written: Option<RegValue>,
    /// Current value observed when a compare-and-swap failed (the op is then in
    /// [`OpState::Conflict`]); `None` otherwise. A resolution expects the value to still be this
    /// (the value the user saw), else it stops at `Conflict` again.
    pub conflict: Option<RegValue>,
    /// Set on **every** record while a conflict resolution is written
    /// ([`RevertMode::Resolution`]): the value the user chose, `KeepCurrent` included (then it is
    /// the current value the user saw). A recovery that finds it writes only the records whose
    /// current value differs from it, expecting [`ValueRecord::conflict`] (design review C5).
    /// Cleared when the resolution is done.
    #[serde(default)]
    pub resolve_to: Option<RegValue>,
    /// Error of the last attempt when writing this record kept failing without a crash
    /// (`AccessDenied`, `Os`, …). The operation then stops at [`OpState::Conflict`] instead of
    /// staying in flight, so that recovery does not retry forever (design review C3).
    #[serde(default)]
    pub write_error: Option<String>,
    /// Set when a restore left this record alone on purpose.
    #[serde(default)]
    pub skipped: Option<SkipReason>,
}

/// Why a restore left a record alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    /// The devnode no longer exists (removed in Device Manager); there is nothing to write to.
    /// Distinct from [`RegValue::Absent`] (design review C3).
    DeviceRemoved,
    /// Silent mode ([`crate::ConflictPolicy::Skip`]) left a conflicting value alone and logged it.
    ConflictSkipped,
}

/// What an operation in [`OpState::RevertPending`] is doing. Persisted with the `RevertPending`
/// transition so that recovery continues exactly that (design review C5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevertMode {
    /// The user's revert (or undo) of a written operation: restore `before`, expect
    /// `last_written`.
    Revert,
    /// Undo of an operation that did not finish or was not kept (error, disconnect, expired
    /// countdown, recovery): restore `before`, expect `intended` (or `last_written` when set);
    /// records already at `before` are done.
    Rollback,
    /// A conflict resolution: write `resolve_to` where the current value differs from it, expect
    /// [`ValueRecord::conflict`] (or `last_written` for records that were not in conflict).
    Resolution,
}

/// Stored values that the drivers may not be using yet (design review C1). Persisted on the entry
/// when it closes (or keeps a reconnect-path `AwaitingConfirm`) with values that no reset applied,
/// so that the GUI and CLI can say "not in effect yet" long after the process that wrote them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyPending {
    pub action: PendingAction,
    /// Keyboards whose running type may differ from their stored values.
    pub instance_ids: Vec<String>,
    /// Boot in which it was recorded. A later boot clears it (every driver re-reads its values).
    pub since: BootId,
}

impl ValueRecord {
    pub fn key(&self) -> ValueKey {
        ValueKey {
            target: self.target.clone(),
            name: self.name.clone(),
        }
    }
}

/// Layout choice for one keyboard, as the user makes it (plan 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutChoice {
    Jis,
    Us,
    /// "標準に従う": delete the HID values so that the PC's standard layout applies (kbdhid only).
    Standard,
}

impl LayoutChoice {
    /// The layout to write, or `None` for [`LayoutChoice::Standard`].
    pub const fn layout(self) -> Option<Layout> {
        match self {
            LayoutChoice::Jis => Some(Layout::Jis),
            LayoutChoice::Us => Some(Layout::Us),
            LayoutChoice::Standard => None,
        }
    }
}

/// What "restore to baseline" covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RestoreScope {
    /// Every value MKLM ever changed (global values included).
    All,
    /// The values of one keyboard's physical device (every collection in its container).
    Device { instance_id: String },
}

/// What an operation was started for. Reverting is not an operation of its own: it is a state
/// transition of the operation it reverts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OpKind {
    /// Assign a layout to one physical keyboard (all its kbdhid collections, plan 3.4).
    SetLayout {
        /// The instance ID the user picked.
        requested: String,
        /// Every devnode written (the requested one and the other collections in its container).
        instance_ids: Vec<String>,
        layout: LayoutChoice,
    },
    /// Fixed mode → per-keyboard mode (plan 1.3), with optional assignments.
    Migrate {
        standard: Layout,
        assignments: Vec<(String, LayoutChoice)>,
    },
    /// "MKLM 導入前に戻す" (plan 2.3, 3.13). Recovery completes it forward rather than undoing it
    /// (design review C14), and once `Confirmed` it cannot be reverted (its baselines are gone;
    /// design review C6): the user sets the layout again instead.
    RestoreBaseline {
        scope: RestoreScope,
        /// True for the uninstall custom action: no confirmation, conflicts skipped and logged,
        /// never rolled back. Reachable only from the helper's fixed `--uninstall-restore`
        /// command line, never from the pipe (design review S5).
        silent: bool,
        /// Open, not in-flight operations this restore closes as `Failed(Superseded)` right after
        /// its `Planned` is flushed (design D.5, review C7). Recorded here so that recovery can
        /// finish superseding them if the writer stops in between.
        #[serde(default)]
        supersedes: Vec<OpId>,
    },
}

/// State of an operation (plan 2.3).
///
/// ```text
/// Planned → Written → Restarting → AwaitingConfirm → Confirmed
/// Planned → Written → PendingReboot → AwaitingConfirm → Confirmed
/// Planned → Written → AwaitingConfirm (reconnect) → Confirmed
/// revert: … → RevertPending → Reverted | RevertedPendingReboot (→ Reverted after a boot)
/// other:  Failed (nothing kept), Conflict (the user decides; also a write that keeps failing)
/// ```
/// The full transition table is in docs/design/m2-engine.md (section C.4) and in
/// [`OpState::can_transition_to`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpState {
    /// Entry and baselines are flushed; target writes may have started.
    Planned,
    /// Every target value is written and flushed.
    Written,
    /// A live reset (DIF_PROPERTYCHANGE) of the keyboard is in progress.
    Restarting,
    /// Values are in effect or waiting for a reconnect; the user decides keep or revert.
    /// With [`JournalEntry::countdown`] set, no answer means revert.
    AwaitingConfirm,
    /// Values are written; they take effect at the next PC restart.
    PendingReboot,
    /// The user kept the change (or a silent restore finished). Closed, but still revertible.
    Confirmed,
    /// Restoring `before` or writing a resolution is in progress (see
    /// [`JournalEntry::revert_mode`]).
    RevertPending,
    /// Values are back at `before`. HID keyboards that a reset did not re-apply are listed in
    /// [`JournalEntry::apply_pending`]. [`JournalEntry::failure`] says why when the revert was not
    /// the user's request (a rollback).
    Reverted,
    /// Values are back, but i8042prt or global values need a PC restart to be read again. Also the
    /// end of a rollback that touched boot-time values (design review C1).
    RevertedPendingReboot,
    /// Closed without its change ever being kept: nothing was written
    /// (`NothingWritten`, `ConcurrentChange`), the user kept outside values
    /// (`ConflictKeptCurrent`), or a restore-to-baseline replaced it (`Superseded`).
    /// See [`JournalEntry::failure`]; [`JournalEntry::apply_pending`] may still be set.
    Failed,
    /// A value was neither what MKLM expected nor what it intended; the user must decide.
    Conflict,
}

impl OpState {
    /// True while the operation blocks every other write, update and re-apply (plan 2.2):
    /// every state except [`Confirmed`](OpState::Confirmed), [`Reverted`](OpState::Reverted),
    /// [`RevertedPendingReboot`](OpState::RevertedPendingReboot) and [`Failed`](OpState::Failed).
    pub fn is_open(self) -> bool {
        todo!("M2: state classification")
    }

    /// True while a process is in the middle of writing or resetting: `Planned`, `Written`,
    /// `Restarting`, `RevertPending`. Such an entry whose owner is gone must be recovered.
    pub fn is_in_flight(self) -> bool {
        todo!("M2: state classification")
    }

    /// The transition table of section C.4 of the design doc.
    pub fn can_transition_to(self, next: OpState) -> bool {
        todo!("M2: transition table")
    }
}

/// The keep-or-revert countdown after a live reset (plan 1.4, 3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Countdown {
    pub seconds: u32,
    /// When the countdown expires. Informational; recovery treats an owner-less countdown as expired.
    pub deadline: Timestamp,
}

/// Why an operation did not end as the user asked. Set on [`OpState::Failed`], and on
/// [`OpState::Reverted`] / [`OpState::RevertedPendingReboot`] when the revert was a rollback
/// rather than the user's request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FailureReason {
    /// A value changed between planning and writing (compare-and-swap before the first write).
    ConcurrentChange { name: String },
    /// A registry or device call failed; written values were rolled back.
    WriteError { message: String },
    /// The caller disconnected before the change could be confirmed.
    CallerDisconnected,
    /// The writer stopped (killed, crashed, power loss) with some values written; recovery rolled
    /// them back.
    Interrupted,
    /// A live-reset change that never reached the user's keep was found by recovery and rolled
    /// back (the countdown's "no answer means revert" rule).
    LiveResetUnconfirmed,
    /// The countdown ran out (or its owner died during it).
    CountdownExpired,
    /// The reset keyboard did not come back, or Windows asked for a restart
    /// (plan 1.4: revert, then recommend a PC restart).
    KeyboardDidNotReturn,
    /// Recovery found that nothing had been written.
    NothingWritten,
    /// The user resolved a conflict by keeping values that are neither `before` nor `intended`.
    ConflictKeptCurrent,
    /// A restore-to-baseline closed this open, not in-flight operation and wrote over its values
    /// (design review C7).
    Superseded { by: OpId },
}

/// One line of an entry's audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRecord {
    pub from: Option<OpState>,
    pub to: OpState,
    pub at: Timestamp,
    pub boot: BootId,
    pub by: ProcessIdentity,
    /// Short machine-readable reason, e.g. `"countdown-expired"`, `"recover:roll-forward"`.
    pub reason: String,
    /// Kernel boot time minus its bias (`SYSTEM_TIMEOFDAY_INFORMATION`, FILETIME units), for
    /// support only: it moves with clock corrections, so no decision reads it (design review C2).
    #[serde(default)]
    pub boot_time_hint: Option<u64>,
}

/// A value read for the plan 1.3 step 1 snapshot. Informational (support and manual recovery);
/// the engine never restores from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextValue {
    pub target: WriteTarget,
    pub key_path: String,
    pub name: String,
    pub value: RegValue,
}

/// One operation in the journal. Stored whole as one JSON `REG_SZ`, so every transition is one
/// atomic registry write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub schema_version: u32,
    pub op_id: OpId,
    /// Order of creation (max existing + 1). Decides which operation is "latest" for a value.
    pub seq: u64,
    pub kind: OpKind,
    pub state: OpState,
    /// Boot of the most recent write phase (set at `Planned` and again at `RevertPending`).
    pub boot_id: BootId,
    /// Process that performs the current write phase (set together with `boot_id`).
    pub owner: ProcessIdentity,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// How the change takes effect; `None` when it needs nothing (e.g. only journal changes).
    pub apply: Option<PendingAction>,
    /// Set while in `AwaitingConfirm` after a live reset.
    pub countdown: Option<Countdown>,
    /// Every value the operation writes, in forward write order (`CheckedPlan::steps`).
    pub records: Vec<ValueRecord>,
    /// Plan 1.3 step 1 snapshot (migrations only).
    pub context: Vec<ContextValue>,
    pub failure: Option<FailureReason>,
    /// Set together with every transition to [`OpState::RevertPending`]; `None` otherwise.
    #[serde(default)]
    pub revert_mode: Option<RevertMode>,
    /// Stored values the drivers may not be using yet (design review C1). Cleared (one journal
    /// write, no state change) once a later boot is seen or Raw Input reports the stored types.
    #[serde(default)]
    pub apply_pending: Option<ApplyPending>,
    /// Audit trail, oldest first; capped at [`MAX_HISTORY`] lines.
    pub history: Vec<TransitionRecord>,
}

/// Most transitions kept in [`JournalEntry::history`].
pub const MAX_HISTORY: usize = 64;

impl JournalEntry {
    /// Moves to `to`, checking [`OpState::can_transition_to`], and appends a history line.
    /// Does not touch the registry; the caller persists the entry and flushes.
    pub fn transition(
        &mut self,
        to: OpState,
        at: Timestamp,
        boot: BootId,
        by: ProcessIdentity,
        reason: &str,
    ) -> Result<(), JournalError> {
        todo!("M2: checked transition + history")
    }

    /// Records that another process continues this in-flight entry (recovery completing or
    /// rolling back an abandoned one): updates `boot_id` and `owner` and appends a history line,
    /// without a state change. Informational only: the write lock protects in-flight entries, not
    /// the owner field (design review C3).
    pub fn take_over(&mut self, at: Timestamp, boot: BootId, by: ProcessIdentity, reason: &str) {
        todo!("M2: owner/boot update + history")
    }

    /// True when any record is an i8042prt device value or a global value (their changes need a
    /// PC restart, so a revert ends in `RevertedPendingReboot`).
    pub fn touches_boot_time_values(&self) -> bool {
        todo!("M2: classify records")
    }

    /// Serializes with [`JOURNAL_SCHEMA_VERSION`].
    pub fn to_json(&self) -> Result<String, JournalError> {
        todo!("M2: serde_json")
    }

    /// Parses an entry, migrating older schema versions; a newer version is an error.
    pub fn from_json(json: &str) -> Result<Self, JournalError> {
        todo!("M2: serde_json + schema check")
    }
}

/// The first-ever value of one registry value MKLM changed. Written once, before MKLM's first write
/// to that value; removed only when a "restore to baseline" of it is confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineRecord {
    pub schema_version: u32,
    pub key: ValueKey,
    pub key_path: String,
    pub value: RegValue,
    pub captured_at: Timestamp,
    /// Operation that captured it.
    pub captured_by: OpId,
}

/// An entry that could not be parsed. Its presence stops every write until a human looks at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableEntry {
    /// Registry value name.
    pub name: String,
    pub error: JournalError,
}

/// The whole journal as read from the store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    pub entries: Vec<JournalEntry>,
    pub baselines: Vec<BaselineRecord>,
    pub unreadable: Vec<UnreadableEntry>,
}

impl Journal {
    /// Parses the raw store: `(value name, JSON)` pairs of [`JOURNAL_OPS_KEY`] and
    /// [`JOURNAL_BASELINES_KEY`]. Never fails; bad entries go to `unreadable`.
    pub fn parse(ops: &[(String, String)], baselines: &[(String, String)]) -> Self {
        todo!("M2: parse store")
    }

    /// Operations in an open state ([`OpState::is_open`]), oldest first. At most one in normal use.
    pub fn open_entries(&self) -> Vec<&JournalEntry> {
        todo!("M2")
    }

    pub fn entry(&self, op_id: &OpId) -> Option<&JournalEntry> {
        todo!("M2")
    }

    /// Resolves a full ID or a unique prefix of at least 8 hex digits.
    pub fn resolve_prefix(&self, prefix: &str) -> Result<&JournalEntry, JournalError> {
        todo!("M2")
    }

    /// The newest record (highest `seq`) for `key` whose `last_written` is set: what MKLM last put
    /// there. Restore-to-baseline compares the current value with it.
    pub fn latest_record(&self, key: &ValueKey) -> Option<(&JournalEntry, &ValueRecord)> {
        todo!("M2")
    }

    pub fn baseline(&self, key: &ValueKey) -> Option<&BaselineRecord> {
        todo!("M2")
    }

    /// `seq` for a new operation.
    pub fn next_seq(&self) -> u64 {
        todo!("M2")
    }

    /// True when the post-reboot check must be (re)registered: an operation in `PendingReboot`,
    /// or one that takes effect through a restart (`apply == RestartPc`) waiting in
    /// `AwaitingConfirm`. The unelevated caller evaluates it after every helper session, after
    /// `recover` and when a write command starts, rather than relying on having received a
    /// `PendingReboot` result (design review C17).
    pub fn needs_post_reboot_check(&self, current_boot: BootId) -> bool {
        todo!("M2")
    }

    /// Closed operations that may be deleted: all but the newest `keep_closed` closed ones, never an
    /// open one, never one that holds [`Journal::latest_record`] of any value, and never one whose
    /// [`JournalEntry::apply_pending`] is still set (the engine clears expired ones first).
    pub fn prunable(&self, keep_closed: usize) -> Vec<OpId> {
        todo!("M2")
    }
}

/// A journal rule was broken or the store could not be understood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum JournalError {
    #[error("{text:?} is not an operation ID")]
    BadOpId { text: String },
    #[error("no operation matches {prefix:?}")]
    UnknownOp { prefix: String },
    #[error("{prefix:?} matches more than one operation")]
    AmbiguousOp { prefix: String },
    #[error("operation {op_id}: {from:?} → {to:?} is not a valid transition")]
    InvalidTransition {
        op_id: String,
        from: OpState,
        to: OpState,
    },
    #[error("schema version {found} is newer than {supported}; update MKLM")]
    NewerSchema { found: u32, supported: u32 },
    #[error("malformed journal data: {message}")]
    Malformed { message: String },
}
