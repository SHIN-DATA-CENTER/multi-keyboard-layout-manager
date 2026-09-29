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

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::allowlist::{ValueOp, WriteTarget};
use crate::layout::PendingAction;
use crate::model::{Layout, value_names};

/// Newest [`JournalEntry`] JSON schema this build reads and writes. Readers accept every version up
/// to this one and migrate older ones; an entry with a newer version makes the journal unreadable
/// (writes stop).
///
/// Version 2 (design m3 A.5, K.13) adds [`OpKind::Cleanup`] and nothing else, so only a cleanup
/// entry is written as 2 ([`OpKind::schema_version`]): a build from before M3 then reports it as
/// [`JournalError::NewerSchema`] ("update MKLM") instead of as malformed, and keeps reading every
/// other entry, which stays at [`JOURNAL_SCHEMA_V1`].
pub const JOURNAL_SCHEMA_VERSION: u32 = 2;
/// The first [`JournalEntry`] schema (M2): every kind but [`OpKind::Cleanup`] is still written in it.
pub const JOURNAL_SCHEMA_V1: u32 = 1;
/// Version of the [`BaselineRecord`] JSON schema this build writes (same rules as
/// [`JOURNAL_SCHEMA_VERSION`]; design C.10 counts it as one of the three store versions).
pub const BASELINE_SCHEMA_VERSION: u32 = 1;

/// MKLM's machine-wide key, relative to `HKEY_LOCAL_MACHINE`. Not an MSI component (plan 2.2).
pub const MKLM_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM";
/// Machine-wide settings (plan 3.13; design m3 B.14, WP-E2), relative to `HKEY_LOCAL_MACHINE`.
/// Readable by everyone, written only by the helper under the write lock
/// (`Request::SetMachineSettings`). Its value names are [`crate::MACHINE_SETTING_NAMES`].
pub const MACHINE_SETTINGS_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Settings";
/// `REG_DWORD` under [`MACHINE_SETTINGS_KEY`]: 1 restores the keyboards when MKLM is uninstalled
/// (the MSI's custom action reads it, M5), 0 leaves them. Absent means 1 (the plan's default).
pub const RESTORE_ON_UNINSTALL_VALUE: &str = "RestoreOnUninstall";
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

/// Key path of the global values, relative to a control set.
const GLOBAL_KEY_PATH: &str = r"Services\i8042prt\Parameters";

/// True when `text` has the exact shape `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
fn is_uuid_text(text: &str) -> bool {
    text.len() == 36 && is_uuid_prefix(text)
}

/// True when `text` is a prefix of the UUID shape of [`is_uuid_text`] (lower-case hex digits,
/// hyphens at positions 8, 13, 18 and 23).
fn is_uuid_prefix(text: &str) -> bool {
    text.len() <= 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// Identifier of one operation: a random UUID, lower-case, hyphenated, without braces
/// (`0f8c2d4e-...`). Also the value name of the entry under [`JOURNAL_OPS_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpId(String);

impl OpId {
    /// Accepts exactly `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
    pub fn parse(text: &str) -> Result<Self, JournalError> {
        if is_uuid_text(text) {
            Ok(Self(text.to_string()))
        } else {
            Err(JournalError::BadOpId {
                text: text.to_string(),
            })
        }
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

/// Identifies one boot of Windows. "Same boot" is decided by equality everywhere, and "restarted"
/// must mean that a full kernel boot happened since the write phase (design review C2): a false
/// "different boot" would let the user keep a change that is not in effect yet.
///
/// Since 0.1.1 it is the **counter form** of `KUSER_SHARED_DATA.BootId`
/// ([`BootId::from_boot_counter`]): the boot sequence number that the OS loader increases on
/// every boot attempt (`\Windows\bootstat.dat`). Sleep, resume from hibernation and a Fast
/// Startup "shutdown" are expected to keep it (design H.2 MT-2..MT-4), which is what MKLM needs:
/// the drivers did not re-read their values either. No clock adjustment moves it (the kernel boot
/// *time* shifts with time corrections; [`TransitionRecord::boot_time_hint`] records it without
/// them, and backs the counter up: [`crate::JournalEntry::counter_boot_is_current`]).
///
/// 0.1.0 recorded the loader's boot identifier GUID
/// (`NtQuerySystemInformation(SystemBootEnvironmentInformation).BootIdentifier`) instead, which is
/// not per boot on every machine (a desktop PC kept the same GUID across full restarts, so its
/// `PendingReboot` never ended). Such ids are **legacy** ([`BootId::is_legacy`]): they never equal
/// a counter id, so every comparison reads them as an earlier boot, unless
/// [`crate::JournalEntry::adopt_current_boot`] judged them to be the current boot (see
/// [`crate::boot`]).
///
/// JSON form: the lower-case hyphenated UUID text without braces, for both forms (0.1.0's parser
/// reads the counter form). The `u128` is the 32 hex digits of that text read as one big-endian
/// number (see [`BootId::from_guid`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BootId(pub u128);

impl BootId {
    /// Every bit of a counter-form id except the counter: an RFC 9562 version-8 UUID with variant
    /// `10` and every other bit zero. Loader GUIDs are version 1 or 4, so they never have it.
    pub const COUNTER_FORM: u128 = 0x0000_0000_0000_8000_8000_0000_0000_0000;

    /// Bits of the counter in a counter-form id (the first group of the text).
    const COUNTER_BITS: u128 = 0xffff_ffff << 96;

    /// The id of the boot whose `KUSER_SHARED_DATA.BootId` is `counter`: the text is
    /// `cccccccc-0000-8000-8000-000000000000` with the counter in hex (for example
    /// `00000007-0000-8000-8000-000000000000`).
    pub const fn from_boot_counter(counter: u32) -> BootId {
        BootId(((counter as u128) << 96) | Self::COUNTER_FORM)
    }

    /// The boot counter of a counter-form id; `None` for any other id (a legacy one).
    pub const fn boot_counter(self) -> Option<u32> {
        if self.0 & !Self::COUNTER_BITS == Self::COUNTER_FORM {
            Some((self.0 >> 96) as u32)
        } else {
            None
        }
    }

    /// True for an id that is not in the counter form: the loader GUID 0.1.0 recorded (or a
    /// test's arbitrary number). See [`crate::boot`] for how such ids are judged.
    pub const fn is_legacy(self) -> bool {
        self.boot_counter().is_none()
    }

    /// Accepts exactly `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`.
    pub fn parse(text: &str) -> Result<Self, JournalError> {
        let bad = || JournalError::Malformed {
            message: format!("{text:?} is not a boot ID"),
        };
        if !is_uuid_text(text) {
            return Err(bad());
        }
        let digits: String = text.chars().filter(|c| *c != '-').collect();
        u128::from_str_radix(&digits, 16)
            .map(Self)
            .map_err(|_| bad())
    }

    /// The canonical text form (see the type docs).
    pub fn to_text(self) -> String {
        let hex = format!("{:032x}", self.0);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The boot ID of a Windows `GUID` (`Data1`, `Data2`, `Data3`, `Data4`), so that
    /// [`BootId::to_text`] prints the GUID's usual text form without braces.
    pub fn from_guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self(
            (u128::from(data1) << 96)
                | (u128::from(data2) << 80)
                | (u128::from(data3) << 64)
                | u128::from(u64::from_be_bytes(data4)),
        )
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

/// The two `REG_SZ` names whose contents compare ASCII case-insensitively (see [`value_eq`]).
const CASE_INSENSITIVE_NAMES: [&str; 2] = [
    value_names::LAYER_DRIVER_JPN,
    value_names::KEYBOARD_IDENTIFIER,
];

/// Equality MKLM uses for the value named `name`, everywhere it compares values (recovery's
/// observation, compare-and-swap, "before equals intended, nothing to record"): exact, except that
/// the `REG_SZ` values `LayerDriver JPN` and `OverrideKeyboardIdentifier` compare ASCII
/// case-insensitively, as the M1 rules already read them (design review C16: Settings or Windows
/// may rewrite `kbd106.dll` as `KBD106.DLL`; that is not a conflict).
pub fn value_eq(name: &str, a: &RegValue, b: &RegValue) -> bool {
    match (a, b) {
        (RegValue::Sz { value: x }, RegValue::Sz { value: y })
            if CASE_INSENSITIVE_NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name)) =>
        {
            x.eq_ignore_ascii_case(y)
        }
        _ => a == b,
    }
}

/// True for the i8042prt value names (`OverrideKeyboardType` / `OverrideKeyboardSubtype`), which
/// are read at boot both per device and globally.
pub(crate) fn is_ps2_value_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(value_names::PS2_TYPE)
        || name.eq_ignore_ascii_case(value_names::PS2_SUBTYPE)
}

/// True for the instance ID of a HID collection (`HID\...`). hidclass enumerates those, and
/// i8042prt never serves one: it drives the 8042 controller's devnodes (`ACPI\...`).
pub(crate) fn is_hid_instance_id(instance_id: &str) -> bool {
    instance_id
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"HID\"))
}

/// True when a change of this value takes effect only at a PC restart: every global value, and the
/// i8042prt values of a device (HID values use other names, see [`crate::DEVICE_VALUE_NAMES`]).
///
/// i8042prt names on a HID collection are not: no driver reads them there (the values
/// `CleanupValues` deletes and a restore may put back, design m3 A.5), so they never wait for a
/// restart. Any other devnode is counted with its i8042prt names, on the safe side.
pub(crate) fn is_boot_time_value(target: &WriteTarget, name: &str) -> bool {
    match target {
        WriteTarget::Global => true,
        WriteTarget::Device { instance_id } => {
            is_ps2_value_name(name) && !is_hid_instance_id(instance_id)
        }
    }
}

/// True when both address the same registry key (instance IDs compare case-insensitively).
pub(crate) fn same_target(a: &WriteTarget, b: &WriteTarget) -> bool {
    match (a, b) {
        (WriteTarget::Global, WriteTarget::Global) => true,
        (WriteTarget::Device { instance_id: x }, WriteTarget::Device { instance_id: y }) => {
            x.eq_ignore_ascii_case(y)
        }
        _ => false,
    }
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
        match &self.target {
            WriteTarget::Device { instance_id } => {
                format!("device|{}|{}", instance_id.to_ascii_uppercase(), self.name)
            }
            WriteTarget::Global => format!("global|{}", self.name),
        }
    }

    /// Key path relative to a control set (`CurrentControlSet` or `ControlSet00N`):
    /// `Enum\<instance id>\Device Parameters` or `Services\i8042prt\Parameters`. Recorded for the
    /// offline recovery files; the engine itself never opens `Enum` paths (plan 1.2).
    pub fn key_path(&self) -> String {
        match &self.target {
            WriteTarget::Device { instance_id } => format!(r"Enum\{instance_id}\Device Parameters"),
            WriteTarget::Global => GLOBAL_KEY_PATH.to_string(),
        }
    }

    /// True when both name the same registry value: same key (instance IDs compare
    /// case-insensitively) and same value name (registry value names are case-insensitive).
    pub(crate) fn same_value(&self, other: &ValueKey) -> bool {
        same_target(&self.target, &other.target) && self.name.eq_ignore_ascii_case(&other.name)
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
    /// Adopted like [`JournalEntry::boot_id`] when it is a legacy id of the current boot.
    pub since: BootId,
}

impl ValueRecord {
    pub fn key(&self) -> ValueKey {
        ValueKey {
            target: self.target.clone(),
            name: self.name.clone(),
        }
    }

    /// True when a change of this value takes effect only at a PC restart (a global value, or an
    /// i8042prt device value; not one on a HID collection, which no driver reads).
    pub fn is_boot_time(&self) -> bool {
        is_boot_time_value(&self.target, &self.name)
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
    /// "削除する" of the first-run wizard (plan 3.1 step 2; design m3 A.5, WP-E1): delete values
    /// that the keyboard's driver does not read (the other driver stack's type/subtype pair,
    /// [`crate::cleanup_candidates`]) from one Keyboard-class devnode. Nothing changes for the
    /// driver, so nothing has to take effect: after `Written` it waits in `AwaitingConfirm`
    /// without a countdown for the user's keep or revert. Baselines are recorded as for every
    /// change, so "restore to baseline" puts the values back. Written with journal schema 2
    /// ([`OpKind::schema_version`]).
    Cleanup {
        instance_id: String,
        /// The value names deleted, in the order written.
        names: Vec<String>,
    },
}

impl OpKind {
    /// The [`JournalEntry`] schema an entry of this kind is written in: 2 for
    /// [`OpKind::Cleanup`] (which older builds must refuse as newer), else 1 (so that older
    /// builds keep reading them). See [`JOURNAL_SCHEMA_VERSION`].
    pub fn schema_version(&self) -> u32 {
        match self {
            OpKind::Cleanup { .. } => JOURNAL_SCHEMA_VERSION,
            OpKind::SetLayout { .. } | OpKind::Migrate { .. } | OpKind::RestoreBaseline { .. } => {
                JOURNAL_SCHEMA_V1
            }
        }
    }
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
    /// Every state, in declaration order.
    pub const ALL: [OpState; 11] = [
        OpState::Planned,
        OpState::Written,
        OpState::Restarting,
        OpState::AwaitingConfirm,
        OpState::PendingReboot,
        OpState::Confirmed,
        OpState::RevertPending,
        OpState::Reverted,
        OpState::RevertedPendingReboot,
        OpState::Failed,
        OpState::Conflict,
    ];

    /// True while the operation blocks every other write, update and re-apply (plan 2.2):
    /// every state except [`Confirmed`](OpState::Confirmed), [`Reverted`](OpState::Reverted),
    /// [`RevertedPendingReboot`](OpState::RevertedPendingReboot) and [`Failed`](OpState::Failed).
    pub fn is_open(self) -> bool {
        !matches!(
            self,
            OpState::Confirmed
                | OpState::Reverted
                | OpState::RevertedPendingReboot
                | OpState::Failed
        )
    }

    /// True while a process is in the middle of writing or resetting: `Planned`, `Written`,
    /// `Restarting`, `RevertPending`. Such an entry whose owner is gone must be recovered.
    pub fn is_in_flight(self) -> bool {
        matches!(
            self,
            OpState::Planned | OpState::Written | OpState::Restarting | OpState::RevertPending
        )
    }

    /// The transition table of section C.4 of the design doc.
    ///
    /// One addition to the printed table: `Restarting → Conflict`. The recovery table (C.7) sends
    /// an abandoned `Restarting` entry whose values are no longer where it left them to
    /// `Conflict`, touching nothing; without this edge that decision could not be carried out.
    /// Staying in the same state is never a transition (a rewrite without a state change, such as
    /// [`JournalEntry::take_over`], does not go through this table).
    pub fn can_transition_to(self, next: OpState) -> bool {
        use OpState::*;
        match self {
            Planned => matches!(
                next,
                Written | Failed | RevertPending | PendingReboot | AwaitingConfirm | Conflict
            ),
            Written => matches!(
                next,
                Restarting | AwaitingConfirm | PendingReboot | Confirmed | RevertPending | Conflict
            ),
            Restarting => matches!(next, AwaitingConfirm | RevertPending | Conflict),
            AwaitingConfirm => matches!(next, Confirmed | RevertPending | Conflict | Failed),
            PendingReboot => matches!(next, AwaitingConfirm | RevertPending | Conflict | Failed),
            Confirmed => next == RevertPending,
            RevertPending => matches!(
                next,
                Reverted | RevertedPendingReboot | Confirmed | Failed | Conflict
            ),
            RevertedPendingReboot => next == Reverted,
            Conflict => matches!(
                next,
                RevertPending | Confirmed | Reverted | RevertedPendingReboot | Failed
            ),
            Reverted | Failed => false,
        }
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
    /// Transitions made by recovery start with [`crate::RECOVERY_REASON_PREFIX`].
    pub reason: String,
    /// Kernel boot time minus its bias (`SYSTEM_TIMEOFDAY_INFORMATION`, FILETIME units) of the
    /// boot this line was written in. It tells whether a legacy (0.1.x) `boot` is the current boot
    /// ([`JournalEntry::legacy_boot_is_current`]), and it can make a counter `boot` count as the
    /// current boot, never as an earlier one ([`JournalEntry::counter_boot_is_current`]). The boot
    /// ID itself never depends on it (design review C2).
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
    /// Boot of the most recent write phase (set at `Planned` and again at `RevertPending`). An
    /// id judged to be the current boot (a legacy 0.1.x one, or a counter one by the safety net)
    /// is replaced in memory when the journal is read ([`JournalEntry::adopt_current_boot`]).
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

/// The minimal shape every stored JSON document has; read first so that a document of a newer
/// schema is reported as such rather than as malformed.
#[derive(Deserialize)]
struct SchemaPeek {
    schema_version: Option<u32>,
}

fn malformed(error: impl std::fmt::Display) -> JournalError {
    JournalError::Malformed {
        message: error.to_string(),
    }
}

/// Checks the `schema_version` of a stored document against the version this build writes.
fn check_schema(json: &str, supported: u32) -> Result<u32, JournalError> {
    let peek: SchemaPeek = serde_json::from_str(json).map_err(malformed)?;
    match peek.schema_version {
        None => Err(malformed("schema_version is missing")),
        Some(0) => Err(malformed("schema_version 0 does not exist")),
        Some(found) if found > supported => Err(JournalError::NewerSchema { found, supported }),
        Some(found) => Ok(found),
    }
}

impl JournalEntry {
    /// Moves to `to`, checking [`OpState::can_transition_to`], and appends a history line.
    /// Does not touch the registry; the caller persists the entry and flushes.
    ///
    /// Also keeps the per-state fields consistent: a transition to `RevertPending` makes `boot` and
    /// `by` the entry's `boot_id` and `owner` (a new write phase starts; the caller sets
    /// `revert_mode`), `revert_mode` is cleared on every transition to another state, and
    /// `countdown` on every transition to a state other than `AwaitingConfirm` (the caller sets it
    /// when a live reset starts one).
    pub fn transition(
        &mut self,
        to: OpState,
        at: Timestamp,
        boot: BootId,
        by: ProcessIdentity,
        reason: &str,
    ) -> Result<(), JournalError> {
        if !self.state.can_transition_to(to) {
            return Err(JournalError::InvalidTransition {
                op_id: self.op_id.to_string(),
                from: self.state,
                to,
            });
        }
        let from = self.state;
        self.state = to;
        self.updated_at = at;
        if to == OpState::RevertPending {
            self.boot_id = boot;
            self.owner = by;
        } else {
            self.revert_mode = None;
        }
        if to != OpState::AwaitingConfirm {
            self.countdown = None;
        }
        self.push_history(TransitionRecord {
            from: Some(from),
            to,
            at,
            boot,
            by,
            reason: reason.to_string(),
            boot_time_hint: None,
        });
        Ok(())
    }

    /// Records that another process continues this in-flight entry (recovery completing or
    /// rolling back an abandoned one): updates `boot_id` and `owner` and appends a history line,
    /// without a state change. Informational only: the write lock protects in-flight entries, not
    /// the owner field (design review C3).
    pub fn take_over(&mut self, at: Timestamp, boot: BootId, by: ProcessIdentity, reason: &str) {
        self.boot_id = boot;
        self.owner = by;
        self.updated_at = at;
        self.push_history(TransitionRecord {
            from: Some(self.state),
            to: self.state,
            at,
            boot,
            by,
            reason: reason.to_string(),
            boot_time_hint: None,
        });
    }

    /// Appends a history line, dropping the oldest lines after the first (creation) one once the
    /// trail exceeds [`MAX_HISTORY`].
    fn push_history(&mut self, line: TransitionRecord) {
        self.history.push(line);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(1..1 + excess);
        }
    }

    /// True when any record is an i8042prt device value or a global value (their changes need a
    /// PC restart, so a revert ends in `RevertedPendingReboot`).
    pub fn touches_boot_time_values(&self) -> bool {
        self.records.iter().any(ValueRecord::is_boot_time)
    }

    /// Serializes with the schema of its kind ([`OpKind::schema_version`]: 2 for a cleanup, else 1),
    /// whatever `schema_version` it was read with (an entry read from an older schema is written
    /// back in the current one, C.10). Fields keep their declaration order, as in design C.3.
    pub fn to_json(&self) -> Result<String, JournalError> {
        let version = self.kind.schema_version();
        if self.schema_version == version {
            return serde_json::to_string(self).map_err(malformed);
        }
        let current = JournalEntry {
            schema_version: version,
            ..self.clone()
        };
        serde_json::to_string(&current).map_err(malformed)
    }

    /// Parses an entry of any schema up to [`JOURNAL_SCHEMA_VERSION`]; a newer version is an
    /// error ([`JournalError::NewerSchema`]).
    ///
    /// Nothing needs migrating: version 2 only adds [`OpKind::Cleanup`], so a version 1 document
    /// (every entry of M2, as the development machine holds them) parses as it is, and fields added
    /// since version 1 was introduced are `#[serde(default)]` and read as absent from older
    /// documents (C.10).
    pub fn from_json(json: &str) -> Result<Self, JournalError> {
        check_schema(json, JOURNAL_SCHEMA_VERSION)?;
        serde_json::from_str(json).map_err(malformed)
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

impl BaselineRecord {
    /// Serializes with [`BASELINE_SCHEMA_VERSION`].
    pub fn to_json(&self) -> Result<String, JournalError> {
        if self.schema_version == BASELINE_SCHEMA_VERSION {
            return serde_json::to_string(self).map_err(malformed);
        }
        let current = BaselineRecord {
            schema_version: BASELINE_SCHEMA_VERSION,
            ..self.clone()
        };
        serde_json::to_string(&current).map_err(malformed)
    }

    /// Parses a record; a newer schema version is an error ([`JournalError::NewerSchema`]).
    pub fn from_json(json: &str) -> Result<Self, JournalError> {
        check_schema(json, BASELINE_SCHEMA_VERSION)?;
        serde_json::from_str(json).map_err(malformed)
    }
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
    ///
    /// Besides JSON and schema errors, a document whose value name does not match its content (an
    /// entry not stored under its own `op_id`, a baseline not under its key's
    /// [`ValueKey::canonical`]) is unreadable: the engine would write the next transition under
    /// another name and end up with two versions of it. Entries are sorted by `seq`.
    pub fn parse(ops: &[(String, String)], baselines: &[(String, String)]) -> Self {
        let mut journal = Journal::default();
        for (name, json) in ops {
            match JournalEntry::from_json(json) {
                Ok(entry) if entry.op_id.as_str() == name => journal.entries.push(entry),
                Ok(entry) => journal.unreadable.push(UnreadableEntry {
                    name: name.clone(),
                    error: malformed(format!(
                        "the entry of operation {} is stored under another name",
                        entry.op_id
                    )),
                }),
                Err(error) => journal.unreadable.push(UnreadableEntry {
                    name: name.clone(),
                    error,
                }),
            }
        }
        for (name, json) in baselines {
            match BaselineRecord::from_json(json) {
                Ok(record) if record.key.canonical().eq_ignore_ascii_case(name) => {
                    journal.baselines.push(record);
                }
                Ok(record) => journal.unreadable.push(UnreadableEntry {
                    name: name.clone(),
                    error: malformed(format!(
                        "the baseline of {} is stored under another name",
                        record.key.canonical()
                    )),
                }),
                Err(error) => journal.unreadable.push(UnreadableEntry {
                    name: name.clone(),
                    error,
                }),
            }
        }
        journal
            .entries
            .sort_by(|a, b| (a.seq, &a.op_id).cmp(&(b.seq, &b.op_id)));
        journal
    }

    /// Operations in an open state ([`OpState::is_open`]), oldest first. At most one in normal use.
    pub fn open_entries(&self) -> Vec<&JournalEntry> {
        let mut open: Vec<&JournalEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.state.is_open())
            .collect();
        open.sort_by(|a, b| (a.seq, &a.op_id).cmp(&(b.seq, &b.op_id)));
        open
    }

    pub fn entry(&self, op_id: &OpId) -> Option<&JournalEntry> {
        self.entries.iter().find(|entry| entry.op_id == *op_id)
    }

    /// Resolves a full ID or a unique prefix of at least 8 hex digits.
    ///
    /// Upper-case hex digits are accepted (the stored IDs are lower-case). The names of unreadable
    /// entries count too: a prefix that also matches one of them is ambiguous.
    pub fn resolve_prefix(&self, prefix: &str) -> Result<&JournalEntry, JournalError> {
        let needle = prefix.to_ascii_lowercase();
        if needle.len() < 8 || !is_uuid_prefix(&needle) {
            return Err(JournalError::BadOpId {
                text: prefix.to_string(),
            });
        }
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.op_id.as_str().starts_with(&needle));
        let unreadable = self
            .unreadable
            .iter()
            .filter(|bad| bad.name.to_ascii_lowercase().starts_with(&needle))
            .count();
        match (matches.next(), matches.next(), unreadable) {
            (Some(entry), None, 0) => Ok(entry),
            (None, _, 0) => Err(JournalError::UnknownOp {
                prefix: prefix.to_string(),
            }),
            _ => Err(JournalError::AmbiguousOp {
                prefix: prefix.to_string(),
            }),
        }
    }

    /// The newest record (highest `seq`) for `key` whose `last_written` is set: what MKLM last put
    /// there. Restore-to-baseline compares the current value with it.
    pub fn latest_record(&self, key: &ValueKey) -> Option<(&JournalEntry, &ValueRecord)> {
        self.entries
            .iter()
            .flat_map(|entry| entry.records.iter().map(move |record| (entry, record)))
            .filter(|(_, record)| record.last_written.is_some() && record.key().same_value(key))
            .max_by(|(a, _), (b, _)| (a.seq, &a.op_id).cmp(&(b.seq, &b.op_id)))
    }

    pub fn baseline(&self, key: &ValueKey) -> Option<&BaselineRecord> {
        self.baselines
            .iter()
            .find(|record| record.key.same_value(key))
    }

    /// `seq` for a new operation.
    pub fn next_seq(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| entry.seq)
            .max()
            .map_or(1, |max| max.saturating_add(1))
    }

    /// True when the post-reboot check must be (re)registered: an operation in `PendingReboot`,
    /// or one that takes effect through a restart (`apply == RestartPc`) waiting in
    /// `AwaitingConfirm`. The unelevated caller evaluates it after every helper session, after
    /// `recover` and when a write command starts, rather than relying on having received a
    /// `PendingReboot` result (design review C17).
    ///
    /// `PendingReboot` counts in any boot: in the boot that wrote it the restart is still to come,
    /// and in a later one the post-reboot check has not confirmed it yet (the user may have skipped
    /// the question, which asks again at the next sign-in). `current_boot` therefore does not change
    /// the answer; registering once too often only shows "nothing to check".
    pub fn needs_post_reboot_check(&self, current_boot: BootId) -> bool {
        let _ = current_boot;
        self.entries.iter().any(|entry| match entry.state {
            OpState::PendingReboot => true,
            OpState::AwaitingConfirm => entry.apply == Some(PendingAction::RestartPc),
            _ => false,
        })
    }

    /// Closed operations that may be deleted: all but the newest `keep_closed` closed ones, never an
    /// open one, never one that holds [`Journal::latest_record`] of any value, and never one whose
    /// [`JournalEntry::apply_pending`] is still set (the engine clears expired ones first).
    /// Oldest first.
    pub fn prunable(&self, keep_closed: usize) -> Vec<OpId> {
        let holds_latest: HashSet<&OpId> = self
            .entries
            .iter()
            .flat_map(|entry| entry.records.iter())
            .filter_map(|record| self.latest_record(&record.key()))
            .map(|(entry, _)| &entry.op_id)
            .collect();
        let mut closed: Vec<&JournalEntry> = self
            .entries
            .iter()
            .filter(|entry| !entry.state.is_open())
            .collect();
        // Newest first, so that `skip` keeps the newest ones.
        closed.sort_by(|a, b| (b.seq, &b.op_id).cmp(&(a.seq, &a.op_id)));
        let mut prunable: Vec<&JournalEntry> = closed
            .into_iter()
            .skip(keep_closed)
            .filter(|entry| !holds_latest.contains(&entry.op_id) && entry.apply_pending.is_none())
            .collect();
        prunable.sort_by(|a, b| (a.seq, &a.op_id).cmp(&(b.seq, &b.op_id)));
        prunable
            .into_iter()
            .map(|entry| entry.op_id.clone())
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::value_names::*;
    use crate::test_support::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const PS2: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

    #[test]
    fn op_id_accepts_only_the_canonical_uuid_form() {
        let id = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        assert_eq!(id.as_str(), "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f");
        assert_eq!(id.short(), "3f2a9c1e");
        assert_eq!(id.to_string(), id.as_str());
        for bad in [
            "3F2A9C1E-5B7D-4E8A-9C0F-1A2B3C4D5E6F",
            "{3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f}",
            "3f2a9c1e5b7d4e8a9c0f1a2b3c4d5e6f",
            "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6",
            "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f0",
            "3f2a9c1e_5b7d-4e8a-9c0f-1a2b3c4d5e6f",
            "3f2a9c1g-5b7d-4e8a-9c0f-1a2b3c4d5e6f",
            " 3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6",
            "",
        ] {
            assert_eq!(
                OpId::parse(bad),
                Err(JournalError::BadOpId { text: bad.into() }),
                "{bad}"
            );
        }
        // Serde goes through the same check.
        assert!(serde_json::from_str::<OpId>("\"3F2A9C1E-5B7D-4E8A-9C0F-1A2B3C4D5E6F\"").is_err());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f\"");
        assert_eq!(serde_json::from_str::<OpId>(&json).unwrap(), id);
    }

    #[test]
    fn boot_id_text_form() {
        let text = "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d";
        let id = BootId::parse(text).unwrap();
        assert_eq!(id.0, 0x9b1c0d6e_2f4a_4c8b_a1d3_5e6f7a8b9c0d);
        assert_eq!(id.to_text(), text);
        assert_eq!(
            BootId::from_guid(
                0x9b1c_0d6e,
                0x2f4a,
                0x4c8b,
                [0xa1, 0xd3, 0x5e, 0x6f, 0x7a, 0x8b, 0x9c, 0x0d]
            ),
            id
        );
        assert_eq!(BootId(1).to_text(), "00000000-0000-0000-0000-000000000001");
        assert_eq!(
            BootId(u128::MAX).to_text(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff"
        );
        for bad in [
            "9B1C0D6E-2F4A-4C8B-A1D3-5E6F7A8B9C0D",
            "{9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d}",
            "9b1c0d6e2f4a4c8ba1d35e6f7a8b9c0d",
            "134036748000000000",
        ] {
            assert!(
                matches!(BootId::parse(bad), Err(JournalError::Malformed { .. })),
                "{bad}"
            );
        }
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<BootId>(&json).unwrap(), id);
        // The earlier numeric form is not accepted.
        assert!(serde_json::from_str::<BootId>("134036748000000000").is_err());
    }

    /// The counter form of `KUSER_SHARED_DATA.BootId` (0.1.1): a version-8 UUID whose first
    /// group is the counter, which 0.1.0's parser (the same `is_uuid_text`) reads.
    #[test]
    fn boot_id_counter_form() {
        let seven = BootId::from_boot_counter(7);
        assert_eq!(seven.to_text(), "00000007-0000-8000-8000-000000000000");
        assert_eq!(
            BootId::from_boot_counter(0xffff_ffff).to_text(),
            "ffffffff-0000-8000-8000-000000000000"
        );
        for n in [1, 7, 0x0001_0000, 0xffff_ffff] {
            let id = BootId::from_boot_counter(n);
            assert_eq!(id.boot_counter(), Some(n));
            assert!(!id.is_legacy());
            let text = id.to_text();
            assert!(is_uuid_text(&text), "{text}");
            assert_eq!(BootId::parse(&text), Ok(id));
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<BootId>(&json).unwrap(), id);
        }
        assert_eq!(
            BootId::parse("00000007-0000-8000-8000-000000000000"),
            Ok(seven)
        );
        assert_eq!(
            BootId::COUNTER_FORM,
            BootId::from_boot_counter(0).0,
            "the form is the id of counter 0"
        );
    }

    /// Loader GUIDs (version 1 or 4) and arbitrary test numbers are legacy; so is a counter id
    /// with any other bit set.
    #[test]
    fn legacy_boot_ids() {
        for text in [
            "9845bda6-baa7-11f1-adca-ca988d513a4f",
            "4c703377-b861-11f1-a1dd-d9f1d0b3ec70",
            "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d",
        ] {
            let id = BootId::parse(text).unwrap();
            assert!(id.is_legacy(), "{text}");
            assert_eq!(id.boot_counter(), None, "{text}");
        }
        assert!(BootId(0).is_legacy());
        assert!(BootId(1).is_legacy());
        assert!(BootId(u128::MAX).is_legacy());
        let seven = BootId::from_boot_counter(7);
        for bit in 0..96 {
            let flipped = BootId(seven.0 ^ (1u128 << bit));
            assert!(flipped.is_legacy(), "bit {bit}: {}", flipped.to_text());
        }
        // The counter bits are not part of the form.
        for bit in 96..128 {
            let flipped = BootId(seven.0 ^ (1u128 << bit));
            assert!(!flipped.is_legacy(), "bit {bit}");
        }
    }

    #[test]
    fn value_eq_is_case_insensitive_only_for_the_two_global_strings() {
        for name in [LAYER_DRIVER_JPN, KEYBOARD_IDENTIFIER, "layerdriver jpn"] {
            assert!(
                value_eq(name, &sz("kbd106.dll"), &sz("KBD106.DLL")),
                "{name}"
            );
            assert!(
                !value_eq(name, &sz("kbd106.dll"), &sz("kbd101.dll")),
                "{name}"
            );
            assert!(
                !value_eq(name, &sz("kbd106.dll"), &RegValue::Absent),
                "{name}"
            );
        }
        assert!(value_eq(
            KEYBOARD_IDENTIFIER,
            &sz("pcat_106key"),
            &sz("PCAT_106KEY")
        ));
        // Every other name, and every other type, compares exactly.
        assert!(!value_eq(
            LAYER_DRIVER_KOR,
            &sz("kbd101a.dll"),
            &sz("KBD101A.DLL")
        ));
        assert!(!value_eq(HID_TYPE, &sz("a"), &sz("A")));
        assert!(value_eq(HID_TYPE, &dword(4), &dword(4)));
        assert!(!value_eq(HID_TYPE, &dword(4), &dword(7)));
        assert!(!value_eq(HID_TYPE, &dword(4), &sz("4")));
        assert!(!value_eq(HID_TYPE, &dword(0), &RegValue::Absent));
        assert!(value_eq(HID_TYPE, &RegValue::Absent, &RegValue::Absent));
        let other = |hex: &str| RegValue::Other {
            reg_type: 1,
            data_hex: hex.into(),
        };
        assert!(value_eq(LAYER_DRIVER_JPN, &other("3400"), &other("3400")));
        assert!(!value_eq(LAYER_DRIVER_JPN, &other("3400"), &sz("4")));
    }

    #[test]
    fn value_key_forms() {
        let key = ValueKey {
            target: device(r"hid\vid_3434&pid_d027&mi_00&col01\8&148ad7e3&0&0000"),
            name: HID_TYPE.into(),
        };
        assert_eq!(
            key.canonical(),
            r"device|HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000|KeyboardTypeOverride"
        );
        assert_eq!(
            key.key_path(),
            r"Enum\hid\vid_3434&pid_d027&mi_00&col01\8&148ad7e3&0&0000\Device Parameters"
        );
        let global = ValueKey {
            target: WriteTarget::Global,
            name: PS2_TYPE.into(),
        };
        assert_eq!(global.canonical(), "global|OverrideKeyboardType");
        assert_eq!(global.key_path(), r"Services\i8042prt\Parameters");
        assert!(key.same_value(&ValueKey {
            target: device(KEYCHRON),
            name: "keyboardtypeoverride".into(),
        }));
        assert!(!key.same_value(&global));
    }

    #[test]
    fn state_classification() {
        use OpState::*;
        let table = [
            (Planned, true, true),
            (Written, true, true),
            (Restarting, true, true),
            (AwaitingConfirm, true, false),
            (PendingReboot, true, false),
            (Confirmed, false, false),
            (RevertPending, true, true),
            (Reverted, false, false),
            (RevertedPendingReboot, false, false),
            (Failed, false, false),
            (Conflict, true, false),
        ];
        assert_eq!(table.len(), OpState::ALL.len());
        for (state, open, in_flight) in table {
            assert_eq!(state.is_open(), open, "{state:?}");
            assert_eq!(state.is_in_flight(), in_flight, "{state:?}");
            // Every in-flight state is open.
            assert!(!state.is_in_flight() || state.is_open());
        }
    }

    /// Section C.4 of the design doc, row by row ("元 → 先"), plus `Restarting → Conflict`, which
    /// the recovery table (C.7) needs (see `OpState::can_transition_to`).
    #[test]
    fn transitions_match_the_design_table_exhaustively() {
        use OpState::*;
        let table: &[(OpState, &[OpState])] = &[
            (
                Planned,
                &[
                    Written,
                    Failed,
                    RevertPending,
                    PendingReboot,
                    AwaitingConfirm,
                    Conflict,
                ],
            ),
            (
                Written,
                &[
                    Restarting,
                    AwaitingConfirm,
                    PendingReboot,
                    Confirmed,
                    RevertPending,
                    Conflict,
                ],
            ),
            (Restarting, &[AwaitingConfirm, RevertPending, Conflict]),
            (
                AwaitingConfirm,
                &[Confirmed, RevertPending, Conflict, Failed],
            ),
            (
                PendingReboot,
                &[AwaitingConfirm, RevertPending, Conflict, Failed],
            ),
            (Confirmed, &[RevertPending]),
            (
                RevertPending,
                &[Reverted, RevertedPendingReboot, Confirmed, Failed, Conflict],
            ),
            (Reverted, &[]),
            (RevertedPendingReboot, &[Reverted]),
            (Failed, &[]),
            (
                Conflict,
                &[
                    RevertPending,
                    Confirmed,
                    Reverted,
                    RevertedPendingReboot,
                    Failed,
                ],
            ),
        ];
        assert_eq!(table.len(), 11);
        let mut checked = 0;
        for from in OpState::ALL {
            let allowed = table
                .iter()
                .find(|(state, _)| *state == from)
                .map(|(_, to)| *to)
                .unwrap();
            for to in OpState::ALL {
                assert_eq!(
                    from.can_transition_to(to),
                    allowed.contains(&to),
                    "{from:?} → {to:?}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 121);
        // No self-transitions: a rewrite without a state change is not a transition.
        for state in OpState::ALL {
            assert!(!state.can_transition_to(state), "{state:?}");
        }
    }

    #[test]
    fn transition_checks_and_records_history() {
        let mut e = entry(
            1,
            set_kind(KEYCHRON),
            OpState::Planned,
            vec![record(device(KEYCHRON), HID_TYPE, dword(4), dword(7))],
        );
        let other = ProcessIdentity {
            pid: 7,
            creation_time: 9,
        };
        e.transition(OpState::Written, Timestamp(5), boot(1), OWNER, "written")
            .unwrap();
        assert_eq!(e.state, OpState::Written);
        assert_eq!(e.updated_at, Timestamp(5));
        let line = e.history.last().unwrap();
        assert_eq!(line.from, Some(OpState::Planned));
        assert_eq!(line.to, OpState::Written);
        assert_eq!(line.reason, "written");

        let before = e.clone();
        assert_eq!(
            e.transition(OpState::Reverted, Timestamp(6), boot(1), OWNER, "nope"),
            Err(JournalError::InvalidTransition {
                op_id: e.op_id.to_string(),
                from: OpState::Written,
                to: OpState::Reverted
            })
        );
        assert_eq!(e, before);

        // A countdown lives only in AwaitingConfirm.
        e.transition(OpState::Restarting, Timestamp(7), boot(1), OWNER, "reset")
            .unwrap();
        e.transition(
            OpState::AwaitingConfirm,
            Timestamp(8),
            boot(1),
            OWNER,
            "arrived",
        )
        .unwrap();
        e.countdown = Some(Countdown {
            seconds: 20,
            deadline: Timestamp(28),
        });
        // RevertPending starts a new write phase: boot and owner follow.
        e.transition(
            OpState::RevertPending,
            Timestamp(9),
            boot(2),
            other,
            "expired",
        )
        .unwrap();
        e.revert_mode = Some(RevertMode::Rollback);
        assert_eq!(e.countdown, None);
        assert_eq!((e.boot_id, e.owner), (boot(2), other));
        // Other transitions keep them, and clear the revert mode.
        e.transition(OpState::Reverted, Timestamp(10), boot(3), OWNER, "done")
            .unwrap();
        assert_eq!(e.revert_mode, None);
        assert_eq!((e.boot_id, e.owner), (boot(2), other));
        assert_eq!(e.history.len(), 6);
    }

    #[test]
    fn take_over_keeps_the_state() {
        let mut e = entry(3, migrate_kind(), OpState::Written, Vec::new());
        let other = ProcessIdentity {
            pid: 8,
            creation_time: 1,
        };
        e.take_over(Timestamp(99), boot(5), other, "recover:complete");
        assert_eq!(e.state, OpState::Written);
        assert_eq!((e.boot_id, e.owner), (boot(5), other));
        let line = e.history.last().unwrap();
        assert_eq!(
            (line.from, line.to),
            (Some(OpState::Written), OpState::Written)
        );
        assert_eq!(line.by, other);
        assert_eq!(e.updated_at, Timestamp(99));
    }

    #[test]
    fn history_is_capped_but_keeps_its_first_line() {
        let mut e = entry(1, migrate_kind(), OpState::Conflict, Vec::new());
        for i in 0..100 {
            e.take_over(Timestamp(i), boot(1), OWNER, &format!("line {i}"));
        }
        assert_eq!(e.history.len(), MAX_HISTORY);
        assert_eq!(e.history[0].reason, "test");
        assert_eq!(e.history[1].reason, "line 37");
        assert_eq!(e.history.last().unwrap().reason, "line 99");
    }

    #[test]
    fn boot_time_values() {
        let hid = record(device(KEYCHRON), HID_TYPE, dword(4), dword(7));
        let ps2 = record(device(PS2), PS2_TYPE, RegValue::Absent, dword(7));
        let global = record(
            WriteTarget::Global,
            LAYER_DRIVER_JPN,
            sz("kbd106.dll"),
            sz("kbd101.dll"),
        );
        assert!(!hid.is_boot_time());
        assert!(ps2.is_boot_time() && global.is_boot_time());
        let e = |records| entry(1, migrate_kind(), OpState::Written, records);
        assert!(!e(vec![hid.clone()]).touches_boot_time_values());
        assert!(e(vec![hid.clone(), ps2]).touches_boot_time_values());
        assert!(e(vec![hid, global]).touches_boot_time_values());
        assert!(!e(Vec::new()).touches_boot_time_values());
    }

    /// The example of design section C.3, with the subtype record added.
    const DESIGN_EXAMPLE: &str = r#"{
      "schema_version": 1,
      "op_id": "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f",
      "seq": 7,
      "kind": {
        "kind": "set-layout",
        "requested": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000",
        "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
        "layout": "jis"
      },
      "state": "awaiting-confirm",
      "boot_id": "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d",
      "owner": { "pid": 12345, "creation_time": 134036790000000000 },
      "created_at": 1790500000000,
      "updated_at": 1790500004000,
      "apply": "reset-keyboard",
      "countdown": { "seconds": 20, "deadline": 1790500024000 },
      "records": [
        {
          "target": { "kind": "device", "instance_id": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000" },
          "key_path": "Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters",
          "name": "KeyboardTypeOverride",
          "baseline": { "kind": "dword", "value": 4 },
          "before": { "kind": "dword", "value": 4 },
          "intended": { "kind": "dword", "value": 7 },
          "last_written": { "kind": "dword", "value": 7 },
          "conflict": null,
          "resolve_to": null,
          "write_error": null,
          "skipped": null
        },
        {
          "target": { "kind": "device", "instance_id": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000" },
          "key_path": "Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters",
          "name": "KeyboardSubtypeOverride",
          "baseline": { "kind": "dword", "value": 0 },
          "before": { "kind": "dword", "value": 0 },
          "intended": { "kind": "dword", "value": 2 },
          "last_written": { "kind": "dword", "value": 2 },
          "conflict": null,
          "resolve_to": null,
          "write_error": null,
          "skipped": null
        }
      ],
      "context": [],
      "failure": null,
      "revert_mode": null,
      "apply_pending": null,
      "history": [
        { "from": null, "to": "planned", "at": 1790500000000, "boot": "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d",
          "by": { "pid": 12345, "creation_time": 134036790000000000 }, "reason": "set-layout",
          "boot_time_hint": 134036748000000000 }
      ]
    }"#;

    #[test]
    fn design_example_parses_and_round_trips() {
        let e = JournalEntry::from_json(DESIGN_EXAMPLE).unwrap();
        assert_eq!(e.state, OpState::AwaitingConfirm);
        assert_eq!(e.apply, Some(PendingAction::ResetKeyboard));
        assert_eq!(e.records.len(), 2);
        assert_eq!(e.records[1].last_written, Some(dword(2)));
        assert_eq!(e.history[0].boot_time_hint, Some(134_036_748_000_000_000));
        let json = e.to_json().unwrap();
        assert!(
            json.starts_with("{\"schema_version\":1,\"op_id\":"),
            "{json}"
        );
        assert_eq!(JournalEntry::from_json(&json).unwrap(), e);
    }

    #[test]
    fn older_json_without_optional_fields_is_read() {
        // Written before the review added revert_mode, apply_pending, resolve_to, write_error,
        // skipped, boot_time_hint and supersedes.
        let old = r#"{
          "schema_version": 1,
          "op_id": "00000001-0000-4000-8000-000000000000",
          "seq": 1,
          "kind": { "kind": "restore-baseline", "scope": { "kind": "all" }, "silent": false },
          "state": "revert-pending",
          "boot_id": "00000000-0000-0000-0000-000000000001",
          "owner": { "pid": 1, "creation_time": 2 },
          "created_at": 3,
          "updated_at": 4,
          "apply": null,
          "countdown": null,
          "records": [{
            "target": { "kind": "global" },
            "key_path": "Services\\i8042prt\\Parameters",
            "name": "OverrideKeyboardType",
            "baseline": { "kind": "absent" },
            "before": { "kind": "dword", "value": 7 },
            "intended": { "kind": "absent" },
            "last_written": null,
            "conflict": null
          }],
          "context": [],
          "failure": null,
          "history": [{ "from": null, "to": "planned", "at": 3,
            "boot": "00000000-0000-0000-0000-000000000001",
            "by": { "pid": 1, "creation_time": 2 }, "reason": "restore" }]
        }"#;
        let e = JournalEntry::from_json(old).unwrap();
        assert_eq!(e.revert_mode, None);
        assert_eq!(e.apply_pending, None);
        assert_eq!(e.records[0].resolve_to, None);
        assert_eq!(e.records[0].write_error, None);
        assert_eq!(e.records[0].skipped, None);
        assert_eq!(e.history[0].boot_time_hint, None);
        assert!(matches!(
            &e.kind,
            OpKind::RestoreBaseline { supersedes, .. } if supersedes.is_empty()
        ));
        // Unknown fields (a later, compatible addition) are ignored.
        let newer_field = old.replacen("\"seq\": 1,", "\"seq\": 1, \"added_later\": true,", 1);
        assert_eq!(JournalEntry::from_json(&newer_field).unwrap(), e);
    }

    #[test]
    fn full_entry_round_trips() {
        let mut e = entry(
            9,
            OpKind::RestoreBaseline {
                scope: RestoreScope::Device {
                    instance_id: KEYCHRON.into(),
                },
                silent: true,
                supersedes: vec![op_id(3)],
            },
            OpState::RevertPending,
            vec![record(
                device(KEYCHRON),
                HID_TYPE,
                RegValue::Other {
                    reg_type: 1,
                    data_hex: "340000".into(),
                },
                RegValue::Absent,
            )],
        );
        e.records[0].conflict = Some(dword(5));
        e.records[0].resolve_to = Some(dword(5));
        e.records[0].write_error = Some("access denied".into());
        e.records[0].skipped = Some(SkipReason::ConflictSkipped);
        e.revert_mode = Some(RevertMode::Resolution);
        e.apply_pending = Some(ApplyPending {
            action: PendingAction::Reconnect,
            instance_ids: vec![KEYCHRON.into()],
            since: boot(77),
        });
        e.failure = Some(FailureReason::Superseded { by: op_id(10) });
        e.countdown = Some(Countdown {
            seconds: 20,
            deadline: Timestamp(1),
        });
        e.context.push(ContextValue {
            target: WriteTarget::Global,
            key_path: GLOBAL_KEY_PATH.into(),
            name: "Start".into(),
            value: dword(1),
        });
        let json = e.to_json().unwrap();
        assert_eq!(JournalEntry::from_json(&json).unwrap(), e);

        // An entry is written back in its kind's schema, whatever it was read with.
        let mut older = e.clone();
        older.schema_version = 0;
        let rewritten = JournalEntry::from_json(&older.to_json().unwrap()).unwrap();
        assert_eq!(rewritten.schema_version, JOURNAL_SCHEMA_V1);
    }

    #[test]
    fn newer_schema_is_refused() {
        let e = entry(1, migrate_kind(), OpState::Planned, Vec::new());
        let json = e
            .to_json()
            .unwrap()
            .replacen("\"schema_version\":1", "\"schema_version\":3", 1);
        assert_eq!(
            JournalEntry::from_json(&json),
            Err(JournalError::NewerSchema {
                found: 3,
                supported: 2
            })
        );
        // Even when the rest no longer parses as version 2.
        assert_eq!(
            JournalEntry::from_json(r#"{"schema_version": 4, "totally": "different"}"#),
            Err(JournalError::NewerSchema {
                found: 4,
                supported: 2
            })
        );
        for bad in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"schema_version": 0}"#,
            r#"{"schema_version": 1}"#,
        ] {
            assert!(
                matches!(
                    JournalEntry::from_json(bad),
                    Err(JournalError::Malformed { .. })
                ),
                "{bad}"
            );
        }
    }

    /// The schema-1 journal of the development machine (M2 real tests) reads as it is: nothing
    /// unreadable, the entries and baselines as stored, and an entry written back stays in
    /// schema 1 (design m3 WP-E1: only cleanup entries are 2).
    #[test]
    fn the_schema_1_journal_of_the_development_machine_reads() {
        let (ops, baselines) = crate::fixtures::schema_1_journal();
        assert_eq!((ops.len(), baselines.len()), (9, 2));
        let journal = Journal::parse(&ops, &baselines);
        assert!(journal.unreadable.is_empty(), "{:?}", journal.unreadable);
        assert_eq!(journal.entries.len(), 9);
        assert_eq!(journal.baselines.len(), 2);
        let states: Vec<(u64, OpState, Option<FailureReason>)> = journal
            .entries
            .iter()
            .map(|e| (e.seq, e.state, e.failure.clone()))
            .collect();
        assert_eq!(
            states,
            vec![
                (1, OpState::Reverted, Some(FailureReason::CountdownExpired)),
                (2, OpState::Reverted, None),
                (3, OpState::Reverted, None),
                (4, OpState::Failed, Some(FailureReason::ConflictKeptCurrent)),
                (5, OpState::Confirmed, None),
                (6, OpState::Reverted, None),
                (
                    7,
                    OpState::Reverted,
                    Some(FailureReason::LiveResetUnconfirmed)
                ),
                (
                    8,
                    OpState::Reverted,
                    Some(FailureReason::CallerDisconnected)
                ),
                (
                    9,
                    OpState::Reverted,
                    Some(FailureReason::CallerDisconnected)
                ),
            ]
        );
        // The entry recovery closed: its last lines carry the recovery reason.
        let recovered = &journal.entries[6];
        assert_eq!(
            recovered
                .history
                .iter()
                .map(|h| (h.to, h.reason.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (OpState::Planned, "set-layout"),
                (OpState::RevertPending, "recover:roll-back"),
                (OpState::Reverted, "recover:roll-back"),
            ]
        );
        assert!(
            recovered
                .history
                .last()
                .is_some_and(|h| h.reason.starts_with(crate::RECOVERY_REASON_PREFIX))
        );
        let left = &journal.entries[8];
        assert_eq!(
            left.history.last().map(|h| h.reason.as_str()),
            Some("caller-disconnected")
        );
        let [failed, kept] = [&journal.entries[3], &journal.entries[4]];
        assert_eq!((failed.seq, failed.state), (4, OpState::Failed));
        assert_eq!((kept.seq, kept.state), (5, OpState::Confirmed));
        assert!(journal.entries.iter().all(|e| e.schema_version == 1));
        assert!(journal.baselines.iter().all(|b| b.schema_version == 1));
        let key = ValueKey {
            target: device(KEYCHRON),
            name: HID_TYPE.into(),
        };
        assert_eq!(journal.baseline(&key).unwrap().value, dword(4));
        // The latest record is the last change, which the caller left (#9): put back to US.
        let (latest, latest_record) = journal.latest_record(&key).unwrap();
        assert_eq!(latest.op_id, left.op_id);
        assert_eq!(latest_record.last_written, Some(dword(4)));
        // Written back: still schema 1, the same document, byte for byte, for every entry.
        assert_eq!(journal.entries.len(), ops.len());
        for (entry, (name, stored)) in journal.entries.iter().zip(&ops) {
            assert_eq!(entry.op_id.as_str(), name);
            let json = entry.to_json().unwrap();
            assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
            assert_eq!(json, *stored, "{name}");
        }
        for (baseline, (name, stored)) in journal.baselines.iter().zip(&baselines) {
            assert_eq!(baseline.key.canonical(), *name);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&baseline.to_json().unwrap()).unwrap(),
                serde_json::from_str::<serde_json::Value>(stored).unwrap()
            );
        }
        // A cleanup added next to them is schema 2; the old entries are untouched.
        let mut cleanup = entry(
            10,
            cleanup_kind(PS2),
            OpState::AwaitingConfirm,
            vec![record(device(PS2), HID_TYPE, dword(7), RegValue::Absent)],
        );
        cleanup.schema_version = JOURNAL_SCHEMA_V1;
        let mut ops = ops.clone();
        ops.push((cleanup.op_id.to_string(), cleanup.to_json().unwrap()));
        let journal = Journal::parse(&ops, &baselines);
        assert!(journal.unreadable.is_empty());
        assert_eq!(
            journal.entry(&cleanup.op_id).map(|e| e.schema_version),
            Some(JOURNAL_SCHEMA_VERSION)
        );
        assert!(
            journal
                .entries
                .iter()
                .filter(|e| e.op_id != cleanup.op_id)
                .all(|e| e.schema_version == 1)
        );
    }

    /// A cleanup entry is written as schema 2, which a build of M2 (schema 1) reports as newer
    /// rather than as malformed (design m3 K.13, J.12): "update MKLM", and every write stops
    /// there. Every other kind stays readable for it.
    #[test]
    fn cleanup_entries_are_schema_2_and_newer_for_an_m2_build() {
        assert_eq!(JOURNAL_SCHEMA_VERSION, 2);
        let kinds = [
            set_kind(KEYCHRON),
            migrate_kind(),
            restore_kind(false),
            cleanup_kind(PS2),
        ];
        for kind in kinds {
            let cleanup = matches!(kind, OpKind::Cleanup { .. });
            let e = entry(1, kind, OpState::Confirmed, Vec::new());
            let json = e.to_json().unwrap();
            let expected = if cleanup { 2 } else { 1 };
            assert_eq!(e.kind.schema_version(), expected);
            assert!(
                json.starts_with(&format!("{{\"schema_version\":{expected},")),
                "{json}"
            );
            assert_eq!(JournalEntry::from_json(&json).unwrap(), e);
            // What the M2 build's reader (schema 1) makes of it.
            let m2 = check_schema(&json, JOURNAL_SCHEMA_V1);
            if cleanup {
                assert_eq!(
                    m2,
                    Err(JournalError::NewerSchema {
                        found: 2,
                        supported: 1
                    })
                );
            } else {
                assert_eq!(m2, Ok(1));
            }
        }
        // The kind and its fields on the wire of the journal.
        let e = entry(
            1,
            cleanup_kind(PS2),
            OpState::AwaitingConfirm,
            vec![record(device(PS2), HID_TYPE, dword(7), RegValue::Absent)],
        );
        let value: serde_json::Value = serde_json::from_str(&e.to_json().unwrap()).unwrap();
        assert_eq!(
            value["kind"],
            serde_json::json!({
                "kind": "cleanup",
                "instance_id": PS2,
                "names": ["KeyboardTypeOverride", "KeyboardSubtypeOverride"],
            })
        );
        // A cleanup stored as schema 1 (never written so) still reads, and is written back as 2.
        let old = e
            .to_json()
            .unwrap()
            .replacen("\"schema_version\":2", "\"schema_version\":1", 1);
        let read = JournalEntry::from_json(&old).unwrap();
        assert!(
            read.to_json()
                .unwrap()
                .starts_with("{\"schema_version\":2,")
        );
    }

    #[test]
    fn values_no_driver_reads_are_not_boot_time_values() {
        // The PS/2 names on a HID collection (a cleanup of the Keychron) and the HID names on the
        // PS/2 keyboard are read by no driver; the PS/2 keyboard's own pin and the global values
        // are read at boot.
        assert!(!record(device(KEYCHRON), PS2_TYPE, dword(7), RegValue::Absent).is_boot_time());
        assert!(
            !record(
                device(&KEYCHRON.to_ascii_lowercase()),
                PS2_SUBTYPE,
                dword(2),
                RegValue::Absent
            )
            .is_boot_time()
        );
        assert!(!record(device(PS2), HID_TYPE, dword(7), RegValue::Absent).is_boot_time());
        assert!(record(device(PS2), PS2_TYPE, dword(7), RegValue::Absent).is_boot_time());
        assert!(record(WriteTarget::Global, LAYER_DRIVER_JPN, sz("a"), sz("b")).is_boot_time());
        // Any other devnode keeps its PS/2 names counted, on the safe side.
        assert!(record(device(r"ROOT\X\0000"), PS2_TYPE, dword(7), dword(4)).is_boot_time());
        assert!(is_hid_instance_id(r"hid\x"));
        assert!(!is_hid_instance_id("HID"));
        assert!(!is_hid_instance_id(r"HIDX\1"));
    }

    fn baseline_record(target: WriteTarget, name: &str, value: RegValue) -> BaselineRecord {
        let key = ValueKey {
            target,
            name: name.into(),
        };
        BaselineRecord {
            schema_version: BASELINE_SCHEMA_VERSION,
            key_path: key.key_path(),
            key,
            value,
            captured_at: Timestamp(1),
            captured_by: op_id(1),
        }
    }

    #[test]
    fn baseline_record_json() {
        let record = baseline_record(device(KEYCHRON), HID_TYPE, dword(4));
        let json = record.to_json().unwrap();
        assert_eq!(BaselineRecord::from_json(&json).unwrap(), record);
        let newer = json.replacen("\"schema_version\":1", "\"schema_version\":5", 1);
        assert_eq!(
            BaselineRecord::from_json(&newer),
            Err(JournalError::NewerSchema {
                found: 5,
                supported: 1
            })
        );
    }

    #[test]
    fn parse_collects_unreadable_entries() {
        let good = entry(2, migrate_kind(), OpState::Confirmed, Vec::new());
        let first = entry(1, migrate_kind(), OpState::Reverted, Vec::new());
        let newer = entry(3, migrate_kind(), OpState::Planned, Vec::new())
            .to_json()
            .unwrap()
            .replacen("\"schema_version\":1", "\"schema_version\":3", 1);
        let misplaced = entry(4, migrate_kind(), OpState::Planned, Vec::new());
        let ops = vec![
            (good.op_id.to_string(), good.to_json().unwrap()),
            (op_id(3).to_string(), newer),
            ("junk".to_string(), "{not json".to_string()),
            (op_id(99).to_string(), misplaced.to_json().unwrap()),
            (first.op_id.to_string(), first.to_json().unwrap()),
        ];
        let baseline = baseline_record(device(KEYCHRON), HID_TYPE, dword(4));
        let baselines = vec![
            (
                // Instance IDs compare case-insensitively.
                baseline.key.canonical().to_ascii_lowercase(),
                baseline.to_json().unwrap(),
            ),
            (
                "global|OverrideKeyboardType".to_string(),
                baseline.to_json().unwrap(),
            ),
            ("global|x".to_string(), "42".to_string()),
        ];
        let journal = Journal::parse(&ops, &baselines);
        assert_eq!(journal.entries, vec![first, good]);
        assert_eq!(journal.baselines, vec![baseline]);
        let names: Vec<&str> = journal.unreadable.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                op_id(3).as_str(),
                "junk",
                op_id(99).as_str(),
                "global|OverrideKeyboardType",
                "global|x"
            ]
        );
        assert_eq!(
            journal.unreadable[0].error,
            JournalError::NewerSchema {
                found: 3,
                supported: 2
            }
        );
        assert!(
            journal.unreadable[1..]
                .iter()
                .all(|u| matches!(u.error, JournalError::Malformed { .. }))
        );
        assert_eq!(Journal::parse(&[], &[]), Journal::default());
    }

    fn written(mut r: ValueRecord) -> ValueRecord {
        r.last_written = Some(r.intended.clone());
        r
    }

    #[test]
    fn latest_record_baseline_and_next_seq() {
        let key = ValueKey {
            target: device(KEYCHRON),
            name: HID_TYPE.into(),
        };
        let lower = KEYCHRON.to_ascii_lowercase();
        let journal = Journal {
            entries: vec![
                entry(
                    1,
                    set_kind(KEYCHRON),
                    OpState::Confirmed,
                    vec![written(record(
                        device(KEYCHRON),
                        HID_TYPE,
                        dword(4),
                        dword(7),
                    ))],
                ),
                // Lower-case instance ID: the same value.
                entry(
                    2,
                    set_kind(KEYCHRON),
                    OpState::Reverted,
                    vec![written(record(
                        device(&lower),
                        HID_TYPE,
                        dword(7),
                        dword(4),
                    ))],
                ),
                // Never written: not what MKLM last put there.
                entry(
                    3,
                    set_kind(KEYCHRON),
                    OpState::Failed,
                    vec![record(device(KEYCHRON), HID_TYPE, dword(4), dword(7))],
                ),
                entry(
                    4,
                    migrate_kind(),
                    OpState::Confirmed,
                    vec![written(record(
                        WriteTarget::Global,
                        PS2_TYPE,
                        dword(7),
                        RegValue::Absent,
                    ))],
                ),
            ],
            baselines: vec![baseline_record(device(KEYCHRON), HID_TYPE, dword(4))],
            unreadable: Vec::new(),
        };
        let (latest, record) = journal.latest_record(&key).unwrap();
        assert_eq!(latest.seq, 2);
        assert_eq!(record.intended, dword(4));
        let subtype = ValueKey {
            target: device(KEYCHRON),
            name: HID_SUBTYPE.into(),
        };
        assert!(journal.latest_record(&subtype).is_none());
        assert_eq!(journal.baseline(&key).unwrap().value, dword(4));
        assert!(journal.baseline(&subtype).is_none());
        assert_eq!(journal.next_seq(), 5);
        assert_eq!(Journal::default().next_seq(), 1);
        assert_eq!(journal.entry(&op_id(3)).unwrap().state, OpState::Failed);
        assert!(journal.entry(&op_id(30)).is_none());
        let open = Journal {
            entries: vec![
                entry(5, migrate_kind(), OpState::PendingReboot, Vec::new()),
                entry(2, migrate_kind(), OpState::Conflict, Vec::new()),
                entry(3, migrate_kind(), OpState::Confirmed, Vec::new()),
            ],
            ..Journal::default()
        };
        let seqs: Vec<u64> = open.open_entries().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![2, 5]);
    }

    #[test]
    fn resolve_prefix() {
        let a = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let b = OpId::parse("3f2a9c1e-aaaa-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let c = OpId::parse("0badcafe-0000-4000-8000-000000000000").unwrap();
        let mut journal = Journal::default();
        for (seq, id) in [(1, &a), (2, &b), (3, &c)] {
            let mut e = entry(seq, migrate_kind(), OpState::Confirmed, Vec::new());
            e.op_id = id.clone();
            journal.entries.push(e);
        }
        assert_eq!(journal.resolve_prefix(a.as_str()).unwrap().op_id, a);
        assert_eq!(journal.resolve_prefix("3f2a9c1e-5").unwrap().op_id, a);
        assert_eq!(journal.resolve_prefix("0badcafe").unwrap().op_id, c);
        assert_eq!(journal.resolve_prefix("0BADCAFE").unwrap().op_id, c);
        assert_eq!(
            journal.resolve_prefix("3f2a9c1e"),
            Err(JournalError::AmbiguousOp {
                prefix: "3f2a9c1e".into()
            })
        );
        assert_eq!(
            journal.resolve_prefix("12345678"),
            Err(JournalError::UnknownOp {
                prefix: "12345678".into()
            })
        );
        for bad in ["0badcaf", "", "0badcafez", "{0badcafe}", "0badcafe0000"] {
            assert_eq!(
                journal.resolve_prefix(bad),
                Err(JournalError::BadOpId { text: bad.into() }),
                "{bad}"
            );
        }
        // An unreadable entry with the same prefix makes it ambiguous.
        journal.unreadable.push(UnreadableEntry {
            name: "0badcafe-1111-4000-8000-000000000000".into(),
            error: malformed("x"),
        });
        assert!(matches!(
            journal.resolve_prefix("0badcafe"),
            Err(JournalError::AmbiguousOp { .. })
        ));
        assert_eq!(journal.resolve_prefix(c.as_str()).unwrap().op_id, c);
    }

    #[test]
    fn needs_post_reboot_check() {
        let with = |state, apply| {
            let mut e = entry(1, migrate_kind(), state, Vec::new());
            e.apply = apply;
            Journal {
                entries: vec![e],
                ..Journal::default()
            }
        };
        let restart = Some(PendingAction::RestartPc);
        for current in [boot(1), boot(2)] {
            assert!(with(OpState::PendingReboot, restart).needs_post_reboot_check(current));
            assert!(with(OpState::AwaitingConfirm, restart).needs_post_reboot_check(current));
            assert!(
                !with(OpState::AwaitingConfirm, Some(PendingAction::Reconnect))
                    .needs_post_reboot_check(current)
            );
            assert!(!with(OpState::AwaitingConfirm, None).needs_post_reboot_check(current));
            for state in [
                OpState::Confirmed,
                OpState::RevertedPendingReboot,
                OpState::Conflict,
                OpState::Written,
            ] {
                assert!(!with(state, restart).needs_post_reboot_check(current));
            }
            assert!(!Journal::default().needs_post_reboot_check(current));
        }
    }

    #[test]
    fn prunable_keeps_the_newest_open_latest_and_pending() {
        let mut journal = Journal::default();
        for seq in 1..=40 {
            // Never written: no operation holds a latest record by default.
            journal.entries.push(entry(
                seq,
                set_kind(KEYCHRON),
                OpState::Reverted,
                vec![record(device(KEYCHRON), HID_TYPE, dword(4), dword(7))],
            ));
        }
        // #3 holds the latest record of the Keychron's subtype.
        journal.entries[2].records.push(written(record(
            device(KEYCHRON),
            HID_SUBTYPE,
            dword(0),
            dword(2),
        )));
        // #5 still has values that are not in effect.
        journal.entries[4].apply_pending = Some(ApplyPending {
            action: PendingAction::Reconnect,
            instance_ids: vec![KEYCHRON.into()],
            since: boot(1),
        });
        // #6 is open.
        journal.entries[5].state = OpState::Conflict;
        // 39 closed: the newest 32 (#9 to #40) stay, #3 and #5 stay for their records.
        let expected: Vec<OpId> = [1, 2, 4, 7, 8].into_iter().map(op_id).collect();
        assert_eq!(journal.prunable(32), expected);
        assert!(journal.prunable(100).is_empty());
        assert_eq!(journal.prunable(0).len(), 37);
    }
}
