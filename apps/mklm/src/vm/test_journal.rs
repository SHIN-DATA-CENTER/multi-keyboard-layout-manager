//! Journal fixtures for the view-model tests of the pages about journal entries (history,
//! restart, post-reboot check, conflicts, recovery). The history is parsed from schema-1 JSON as
//! the M2 builds stored it on the development machine (the R5 entry is verbatim), so these tests
//! also prove that the GUI reads the entries and baselines written before the journal's schema 2
//! (design m3 WP-E1, K.13).

use mklm_core::{
    BootId, Journal, JournalEntry, LayoutChoice, OpId, OpKind, OpState, PendingAction,
    ProcessIdentity, RegValue, Timestamp, TransitionRecord, ValueKey, ValueRecord, WriteTarget,
    value_names,
};

pub const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
pub const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

/// The boot of the R5 entry.
pub const BOOT: BootId = BootId(0x4c70_3377_b861_11f1_a1dd_d9f1_d0b3_ec70);
/// A later boot.
pub const LATER_BOOT: BootId = BootId(0x1111_2222_3333_4444_5555_6666_7777_8888);

/// The M2 real-machine test R5 (helper killed after its first write, before the keyboard reset;
/// recovery rolled it back), exactly as an M2 build stored it.
pub const R5_ENTRY: &str = r#"{"schema_version":1,"op_id":"8e9a9970-f7bf-46c7-b779-f914f17bd40d","seq":7,"kind":{"kind":"set-layout","requested":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","instance_ids":["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],"layout":"jis"},"state":"reverted","boot_id":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","owner":{"pid":4668,"creation_time":134349894905676290},"created_at":1790515887721,"updated_at":1790516010637,"apply":"reset-keyboard","countdown":null,"records":[{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardTypeOverride","baseline":{"kind":"dword","value":4},"before":{"kind":"dword","value":4},"intended":{"kind":"dword","value":7},"last_written":{"kind":"dword","value":4},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null},{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardSubtypeOverride","baseline":{"kind":"dword","value":0},"before":{"kind":"dword","value":0},"intended":{"kind":"dword","value":2},"last_written":{"kind":"dword","value":0},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null}],"context":[],"failure":{"kind":"live-reset-unconfirmed"},"revert_mode":null,"apply_pending":null,"history":[{"from":null,"to":"planned","at":1790515887721,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":20644,"creation_time":134349894876176557},"reason":"set-layout","boot_time_hint":134349552405000000},{"from":"planned","to":"revert-pending","at":1790515890626,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4668,"creation_time":134349894905676290},"reason":"recover:roll-back","boot_time_hint":134349552405000000},{"from":"revert-pending","to":"reverted","at":1790516010637,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4668,"creation_time":134349894905676290},"reason":"recover:roll-back","boot_time_hint":134349552405000000}]}"#;

/// A schema-1 change to JIS whose caller went away during the countdown (T-APPLY-5, R16): the
/// helper put the values back. No `revert_mode`, `apply_pending`, `resolve_to`, `write_error` or
/// `skipped` fields: an older M2 document, read with their defaults.
const DISCONNECTED_ENTRY: &str = r#"{"schema_version":1,"op_id":"1ef48b2f-1111-4aaa-8bbb-000000000004","seq":4,"kind":{"kind":"set-layout","requested":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","instance_ids":["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],"layout":"jis"},"state":"reverted","boot_id":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","owner":{"pid":4000,"creation_time":1},"created_at":1790515200000,"updated_at":1790515230000,"apply":"reset-keyboard","countdown":null,"records":[{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardTypeOverride","baseline":{"kind":"absent"},"before":{"kind":"absent"},"intended":{"kind":"dword","value":7},"last_written":{"kind":"absent"},"conflict":null},{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardSubtypeOverride","baseline":{"kind":"absent"},"before":{"kind":"absent"},"intended":{"kind":"dword","value":2},"last_written":{"kind":"absent"},"conflict":null}],"context":[],"failure":{"kind":"caller-disconnected"},"history":[{"from":null,"to":"planned","at":1790515200000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"set-layout"},{"from":"planned","to":"written","at":1790515201000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"written"},{"from":"written","to":"restarting","at":1790515202000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"live-reset"},{"from":"restarting","to":"awaiting-confirm","at":1790515205000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"countdown"},{"from":"awaiting-confirm","to":"revert-pending","at":1790515210000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"caller-disconnected"},{"from":"revert-pending","to":"reverted","at":1790515230000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4000,"creation_time":1},"reason":"caller-disconnected"}]}"#;

/// A schema-1 change to US that the user kept.
const KEPT_ENTRY: &str = r#"{"schema_version":1,"op_id":"3f2a9c1e-2222-4aaa-8bbb-000000000006","seq":6,"kind":{"kind":"set-layout","requested":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","instance_ids":["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],"layout":"us"},"state":"confirmed","boot_id":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","owner":{"pid":4100,"creation_time":2},"created_at":1790515800000,"updated_at":1790515815000,"apply":"reset-keyboard","countdown":null,"records":[{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardTypeOverride","baseline":{"kind":"absent"},"before":{"kind":"absent"},"intended":{"kind":"dword","value":4},"last_written":{"kind":"dword","value":4},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null},{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardSubtypeOverride","baseline":{"kind":"absent"},"before":{"kind":"absent"},"intended":{"kind":"dword","value":0},"last_written":{"kind":"dword","value":0},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null}],"context":[],"failure":null,"revert_mode":null,"apply_pending":null,"history":[{"from":null,"to":"planned","at":1790515800000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4100,"creation_time":2},"reason":"set-layout","boot_time_hint":134349552405000000},{"from":"awaiting-confirm","to":"confirmed","at":1790515815000,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4100,"creation_time":2},"reason":"keep","boot_time_hint":134349552405000000}]}"#;

/// Schema-1 baselines stored by the same M2 build (the R5 test's, and the subtype).
const BASELINES: [(&str, &str); 2] = [
    (
        r"device|HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000|KeyboardTypeOverride",
        r#"{"schema_version":1,"key":{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"name":"KeyboardTypeOverride"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","value":{"kind":"dword","value":4},"captured_at":1790515162813,"captured_by":"bfaca7cd-fdef-4dd0-8d75-6f311d32bc37"}"#,
    ),
    (
        r"device|HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000|KeyboardSubtypeOverride",
        r#"{"schema_version":1,"key":{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"name":"KeyboardSubtypeOverride"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","value":{"kind":"dword","value":0},"captured_at":1790515162813,"captured_by":"bfaca7cd-fdef-4dd0-8d75-6f311d32bc37"}"#,
    ),
];

/// The three schema-1 entries and two baselines above, parsed as the GUI reads the store.
pub fn history() -> Journal {
    let ops: Vec<(String, String)> = [
        ("1ef48b2f-1111-4aaa-8bbb-000000000004", DISCONNECTED_ENTRY),
        ("3f2a9c1e-2222-4aaa-8bbb-000000000006", KEPT_ENTRY),
        ("8e9a9970-f7bf-46c7-b779-f914f17bd40d", R5_ENTRY),
    ]
    .iter()
    .map(|(name, json)| ((*name).to_string(), (*json).to_string()))
    .collect();
    let baselines: Vec<(String, String)> = BASELINES
        .iter()
        .map(|(name, json)| ((*name).to_string(), (*json).to_string()))
        .collect();
    Journal::parse(&ops, &baselines)
}

/// One HID value record of the Keychron.
pub fn hid_record(name: &str, before: u32, intended: u32, written: Option<u32>) -> ValueRecord {
    let target = WriteTarget::Device {
        instance_id: KEYCHRON.into(),
    };
    ValueRecord {
        key_path: ValueKey {
            target: target.clone(),
            name: name.into(),
        }
        .key_path(),
        target,
        name: name.into(),
        baseline: RegValue::Dword { value: before },
        before: RegValue::Dword { value: before },
        intended: RegValue::Dword { value: intended },
        last_written: written.map(|value| RegValue::Dword { value }),
        conflict: None,
        resolve_to: None,
        write_error: None,
        skipped: None,
    }
}

/// A change of the Keychron from US (4/0) to JIS (7/2) in `state`, written except while
/// `Planned`, in this boot with a writer that has gone.
pub fn entry(op_id: &str, seq: u64, state: OpState) -> JournalEntry {
    let written = (state != OpState::Planned).then_some(());
    JournalEntry {
        schema_version: 1,
        op_id: OpId::parse(op_id).unwrap(),
        seq,
        kind: OpKind::SetLayout {
            requested: KEYCHRON.into(),
            instance_ids: vec![KEYCHRON.into()],
            layout: LayoutChoice::Jis,
        },
        state,
        boot_id: BOOT,
        owner: ProcessIdentity {
            pid: 4242,
            creation_time: 7,
        },
        created_at: Timestamp(1_790_520_000_000),
        updated_at: Timestamp(1_790_520_000_000),
        apply: Some(PendingAction::ResetKeyboard),
        countdown: None,
        records: vec![
            hid_record(value_names::HID_TYPE, 4, 7, written.map(|()| 7)),
            hid_record(value_names::HID_SUBTYPE, 0, 2, written.map(|()| 2)),
        ],
        context: Vec::new(),
        failure: None,
        revert_mode: None,
        apply_pending: None,
        history: vec![TransitionRecord {
            from: None,
            to: OpState::Planned,
            at: Timestamp(1_790_520_000_000),
            boot: BOOT,
            by: ProcessIdentity {
                pid: 4242,
                creation_time: 7,
            },
            reason: "set-layout".into(),
            boot_time_hint: None,
        }],
    }
}

/// Appends a history line (a transition this entry went through).
pub fn went_through(entry: &mut JournalEntry, to: OpState) {
    entry.history.push(TransitionRecord {
        from: entry.history.last().map(|line| line.to),
        to,
        at: entry.updated_at,
        boot: entry.boot_id,
        by: entry.owner,
        reason: "test".into(),
        boot_time_hint: None,
    });
}

#[test]
fn schema_1_entries_and_baselines_are_read() {
    let journal = history();
    assert!(journal.unreadable.is_empty(), "{:?}", journal.unreadable);
    assert_eq!(journal.entries.len(), 3);
    assert_eq!(journal.baselines.len(), 2);
    assert!(
        journal
            .entries
            .iter()
            .all(|entry| entry.schema_version == 1)
    );
    // The fields added during M2 read as absent from the oldest documents.
    let disconnected = &journal.entries[0];
    assert_eq!(disconnected.seq, 4);
    assert_eq!(disconnected.revert_mode, None);
    assert_eq!(disconnected.records[0].write_error, None);
}
