//! Builders shared by the unit tests of the journal, recovery and restore modules.

use crate::allowlist::WriteTarget;
use crate::journal::{
    BootId, JournalEntry, LayoutChoice, OpId, OpKind, OpState, ProcessIdentity, RegValue,
    RestoreScope, Timestamp, TransitionRecord, ValueKey, ValueRecord,
};
use crate::model::Layout;

/// A well-formed operation ID whose first eight digits are `n` in hex.
pub fn op_id(n: u32) -> OpId {
    OpId::parse(&format!("{n:08x}-0000-4000-8000-000000000000")).unwrap()
}

pub fn boot(n: u128) -> BootId {
    BootId(n)
}

pub const OWNER: ProcessIdentity = ProcessIdentity {
    pid: 4242,
    creation_time: 134_036_790_000_000_000,
};

pub fn dword(value: u32) -> RegValue {
    RegValue::Dword { value }
}

pub fn sz(value: &str) -> RegValue {
    RegValue::Sz {
        value: value.to_string(),
    }
}

pub fn device(instance_id: &str) -> WriteTarget {
    WriteTarget::Device {
        instance_id: instance_id.to_string(),
    }
}

/// A record whose baseline is its `before`, not written yet.
pub fn record(
    target: WriteTarget,
    name: &str,
    before: RegValue,
    intended: RegValue,
) -> ValueRecord {
    let key = ValueKey {
        target: target.clone(),
        name: name.to_string(),
    };
    ValueRecord {
        target,
        key_path: key.key_path(),
        name: name.to_string(),
        baseline: before.clone(),
        before,
        intended,
        last_written: None,
        conflict: None,
        resolve_to: None,
        write_error: None,
        skipped: None,
    }
}

pub fn set_kind(instance_id: &str) -> OpKind {
    OpKind::SetLayout {
        requested: instance_id.to_string(),
        instance_ids: vec![instance_id.to_string()],
        layout: LayoutChoice::Jis,
    }
}

pub fn migrate_kind() -> OpKind {
    OpKind::Migrate {
        standard: Layout::Jis,
        assignments: Vec::new(),
    }
}

/// A standard change JIS → US (design standard-layout B), without keyboards.
pub fn standard_kind() -> OpKind {
    OpKind::SetStandard {
        from: Layout::Jis,
        to: Layout::Us,
        keyboards: Vec::new(),
    }
}

/// A cleanup of the HID pair on `instance_id` (an i8042prt keyboard, design m3 A.5).
pub fn cleanup_kind(instance_id: &str) -> OpKind {
    OpKind::Cleanup {
        instance_id: instance_id.to_string(),
        names: vec![
            crate::model::value_names::HID_TYPE.to_string(),
            crate::model::value_names::HID_SUBTYPE.to_string(),
        ],
    }
}

pub fn restore_kind(silent: bool) -> OpKind {
    OpKind::RestoreBaseline {
        scope: RestoreScope::All,
        silent,
        supersedes: Vec::new(),
    }
}

/// An entry created in boot 1 by [`OWNER`], with `op_id(seq)`.
pub fn entry(seq: u64, kind: OpKind, state: OpState, records: Vec<ValueRecord>) -> JournalEntry {
    let at = Timestamp(1_790_500_000_000 + seq * 1_000);
    JournalEntry {
        schema_version: kind.schema_version(),
        op_id: op_id(u32::try_from(seq).unwrap()),
        seq,
        kind,
        state,
        boot_id: boot(1),
        owner: OWNER,
        created_at: at,
        updated_at: at,
        apply: None,
        countdown: None,
        records,
        context: Vec::new(),
        failure: None,
        revert_mode: None,
        apply_pending: None,
        history: vec![TransitionRecord {
            from: None,
            to: OpState::Planned,
            at,
            boot: boot(1),
            by: OWNER,
            reason: "test".to_string(),
            boot_time_hint: None,
        }],
    }
}
