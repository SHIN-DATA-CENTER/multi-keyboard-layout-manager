//! The unelevated preview of a write (design F.2 step 4, review S6), also printed on its own by
//! `--dry-run`: what the helper will write, in which order, how the change takes effect, and
//! whether INV-PS2 holds. The data comes from `mklm_client::preview` (shared with the GUI, design
//! m3 A.2); this module writes it as the CLI's English text.

use std::fmt::Write as _;

use mklm_core::{
    ConflictPolicy, GlobalSettings, Journal, JournalEntry, KeyboardDevice, KeyboardDriver, OpId,
    OperationPlan, PendingAction, RegValue, SystemSnapshot, ValueRecord, assess, check_inv_ps2,
};

pub use mklm_client::preview::{
    CheckRow, RestorePreview, check_rows, expected, no_other_input, nothing_to_change,
    preview_restore, undo_preview,
};

use super::render::{apply_text, entry_line, key_text, model_value, op_value, put, value_text};
use crate::text::{table_name, type_text};

/// The plan of a `set` or `migrate`, as the user approves it. `only`: the keyboards to list
/// afterwards (`None`: every kbdhid and i8042prt keyboard, for a migration).
pub fn plan_text(
    snapshot: &SystemSnapshot,
    plan: &OperationPlan,
    only: Option<&[String]>,
) -> String {
    let mut out = String::new();
    put!(
        out,
        "Planned changes (they apply to every user of this PC):"
    );
    for (index, step) in plan.checked.steps.iter().enumerate() {
        put!(out, "  {}. {}", index + 1, key_text(&step.target));
        let width = step.writes.iter().map(|w| w.name.len()).max().unwrap_or(0);
        for write in &step.writes {
            let current = model_value(
                &snapshot.keyboards,
                &snapshot.global,
                &step.target,
                &write.name,
            )
            .map_or_else(|| "?".to_string(), |value| value_text(&value));
            let new = op_value(&write.op);
            let new = if new == RegValue::Absent {
                "(delete)".to_string()
            } else {
                value_text(&new)
            };
            put!(out, "       {:<width$}  {current} -> {new}", write.name);
        }
    }
    put!(out, "Takes effect: {}", apply_text(plan.apply));
    if plan.only_usable_keyboard && plan.apply == PendingAction::RestartPc {
        put!(
            out,
            "  (the keyboard counts as the only one you can type with, so it is not reset in place)"
        );
    }
    put!(
        out,
        "INV-PS2: {}",
        match check_inv_ps2(&plan.checked.global, &plan.checked.keyboards) {
            Ok(()) => "holds after every step".to_string(),
            Err(violation) => format!("broken: {violation}"),
        }
    );
    out.push_str(&keyboards_after(
        snapshot,
        &plan.checked.keyboards,
        &plan.checked.global,
        only,
    ));
    out
}

/// What each keyboard reports and types before and after (the engine builds its
/// `ExpectedKeyboard` list the same way).
fn keyboards_after(
    snapshot: &SystemSnapshot,
    after_keyboards: &[KeyboardDevice],
    after_global: &GlobalSettings,
    only: Option<&[String]>,
) -> String {
    let before = assess(snapshot);
    let after = assess(&SystemSnapshot {
        keyboards: after_keyboards.to_vec(),
        global: after_global.clone(),
        ..snapshot.clone()
    });
    let mut table =
        crate::table::Table::new(&["Keyboard", "Type now", "Type after", "Layout after"])
            .max_width(0, 32);
    let mut rows = 0;
    for (assessed, kb) in after.keyboards.iter().zip(after_keyboards) {
        let listed = match only {
            Some(ids) => ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&kb.instance_id)),
            None => matches!(kb.driver, KeyboardDriver::Kbdhid | KeyboardDriver::I8042prt),
        };
        if !listed {
            continue;
        }
        let was = before
            .keyboards
            .iter()
            .find(|b| b.instance_id.eq_ignore_ascii_case(&kb.instance_id));
        let layout_before = was.and_then(|b| b.after_restart.as_ref().map(|l| l.table.clone()));
        let layout_after = assessed.after_restart.as_ref().map(|l| l.table.clone());
        let mut layout = layout_after
            .as_ref()
            .map_or_else(|| "-".to_string(), |t| table_name(t).to_string());
        if layout_after != layout_before {
            layout.push_str(" (changes)");
        }
        let state = if kb.present { "" } else { " (not connected)" };
        table.row(vec![
            format!("{}{state}", kb.display_name),
            type_text(was.and_then(|b| b.predicted_type)),
            type_text(assessed.predicted_type),
            layout,
        ]);
        rows += 1;
    }
    if rows == 0 {
        return String::new();
    }
    let mut out = String::from("Keyboards afterwards:\n");
    out.push_str(&table.render("  "));
    out
}

pub fn restore_text(preview: &RestorePreview) -> String {
    let mut out = String::new();
    if preview.writes.is_empty() && preview.conflicts.is_empty() {
        if preview.unchanged == 0 && preview.removed.is_empty() {
            put!(
                out,
                "Nothing to restore: MKLM has not changed any of these values (no value from \
                 before MKLM is recorded)."
            );
        } else {
            put!(
                out,
                "Nothing to restore: {} value(s) already have their value from before MKLM.",
                preview.unchanged
            );
        }
    } else {
        put!(
            out,
            "Values to put back (they apply to every user of this PC):"
        );
        for row in &preview.writes {
            put!(
                out,
                "  {}\\{}: {} -> {}",
                row.key_path,
                row.name,
                value_text(&row.current),
                if row.baseline == RegValue::Absent {
                    "(delete)".to_string()
                } else {
                    value_text(&row.baseline)
                }
            );
        }
        if preview.unchanged > 0 {
            put!(
                out,
                "  ({} value(s) already as before MKLM)",
                preview.unchanged
            );
        }
    }
    for removed in &preview.removed {
        put!(
            out,
            "  {removed}: the keyboard no longer exists; left alone"
        );
    }
    if !preview.conflicts.is_empty() {
        let action = match preview.policy {
            ConflictPolicy::Report => "the restore stops and shows them (use --on-conflict)",
            ConflictPolicy::Skip => "left as they are (--on-conflict skip)",
            ConflictPolicy::Overwrite => {
                "overwritten with the value before MKLM (--on-conflict overwrite)"
            }
        };
        put!(out, "Changed outside MKLM ({action}):");
        for row in &preview.conflicts {
            put!(
                out,
                "  {}\\{}: now {}, MKLM last wrote {}, before MKLM {}",
                row.key_path,
                row.name,
                value_text(&row.current),
                value_text(&row.expect),
                value_text(&row.baseline)
            );
        }
    }
    if let Some(apply) = preview.apply {
        put!(out, "Takes effect: {}", apply_text(apply));
    }
    match &preview.order {
        Ok(None) if !preview.writes.is_empty() => put!(out, "INV-PS2: holds after every step"),
        Ok(None) => {}
        Ok(Some(violation)) => put!(
            out,
            "INV-PS2: the values before MKLM already broke it and are put back as found: {violation}"
        ),
        Err(error) => put!(out, "Refused: {error}"),
    }
    if !preview.supersedes.is_empty() {
        let ids: Vec<&str> = preview.supersedes.iter().map(OpId::short).collect();
        put!(
            out,
            "Open operations it replaces (closed as superseded): {}",
            ids.join(", ")
        );
    }
    out
}

/// What `undo` (design D.10) would do now: interrupted entries are recovered first, then every
/// open entry that waits for the user is undone, newest first.
pub fn undo_text(journal: &Journal, current: &dyn Fn(&ValueRecord) -> Option<RegValue>) -> String {
    let mut out = String::new();
    let preview = undo_preview(journal, current);
    if preview.is_empty() {
        put!(
            out,
            "Nothing to undo: no operation waits for keep, revert, a restart or a conflict decision."
        );
        return out;
    }
    for entry in &preview.recovered_first {
        put!(out, "Recovered first: {}", entry_line(entry));
    }
    for undone in &preview.undone {
        put!(out, "Undo {}", entry_line(undone.entry));
        for value in &undone.values {
            let record = value.record;
            let now_text = value
                .now
                .as_ref()
                .map_or_else(|| "?".to_string(), value_text);
            if value.restorable {
                put!(
                    out,
                    "  {}\\{}: {now_text} -> {}",
                    record.key_path,
                    record.name,
                    value_text(&record.before)
                );
            } else {
                put!(
                    out,
                    "  {}\\{}: {now_text} (changed outside MKLM; left alone, resolve it with \
                     `mklm-cli resolve {}`)",
                    record.key_path,
                    record.name,
                    undone.entry.op_id.short()
                );
            }
        }
        if undone.restart_needed {
            put!(
                out,
                "  The PC must restart afterwards (Restart, not Shut down)."
            );
        }
    }
    out
}

/// What `revert <op>` would write (design D.4).
pub fn revert_text(
    entry: &JournalEntry,
    current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
) -> String {
    let mut out = String::new();
    put!(out, "Revert {}", entry_line(entry));
    for record in &entry.records {
        let now = current(record).map_or_else(|| "?".to_string(), |v| value_text(&v));
        put!(
            out,
            "  {}\\{}: {now} -> {}",
            record.key_path,
            record.name,
            value_text(&record.before)
        );
    }
    if entry.touches_boot_time_values() {
        put!(
            out,
            "The PC must restart afterwards (Restart, not Shut down)."
        );
    }
    out
}

pub fn check_text(rows: &[CheckRow], migration: bool) -> String {
    let mut out = String::new();
    let mut table = crate::table::Table::new(&["Keyboard", "Expected", "Raw Input", "", "Layout"])
        .max_width(0, 32);
    for row in rows {
        let mark = match (row.expected, row.reported) {
            (Some(expected), Some(reported)) if expected == reported => "ok",
            (_, None) => "not connected",
            _ => "DIFFERENT",
        };
        table.row(vec![
            row.name.clone(),
            type_text(row.expected),
            type_text(row.reported),
            mark.to_string(),
            row.layout
                .as_ref()
                .map_or_else(|| "-".to_string(), |t| table_name(t).to_string()),
        ]);
    }
    out.push_str(&table.render("  "));
    if migration {
        put!(
            out,
            "Raw Input cannot show whether the switch to per-keyboard mode took effect (the built-in \
             keyboard reports 0x7/0x2 either way). The typing test decides."
        );
    }
    put!(
        out,
        "Type Shift+2 in any text box with each keyboard: \" means JIS, @ means US."
    );
    out
}

#[cfg(test)]
mod tests {
    use mklm_core::{
        ApplyOptions, BaselineRecord, KeyboardType, LayoutChoice, OpKind, OpState, ProcessIdentity,
        RestoreScope, Timestamp, ValueKey, WriteTarget, fixtures, plan_migration, plan_set_layout,
        value_names,
    };

    use super::*;

    const RESET: ApplyOptions = ApplyOptions {
        allow_live_reset: true,
        other_input_available: true,
    };

    #[test]
    fn set_keychron_to_jis_on_the_dev_machine() {
        let snapshot = fixtures::dev_machine();
        let keychron = fixtures::keychron().instance_id;
        let plan = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &keychron,
            LayoutChoice::Jis,
            &RESET,
        )
        .unwrap();
        assert_eq!(plan.apply, PendingAction::ResetKeyboard);
        assert!(!nothing_to_change(&snapshot, &plan));
        let text = plan_text(&snapshot, &plan, Some(&plan.instance_ids));
        assert!(
            text.contains(
                r"Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters"
            ),
            "{text}"
        );
        assert!(text.contains("KeyboardTypeOverride     4 -> 7"), "{text}");
        assert!(text.contains("KeyboardSubtypeOverride  0 -> 2"), "{text}");
        assert!(
            text.contains("Takes effect: the keyboard is reset in place now"),
            "{text}"
        );
        assert!(text.contains("INV-PS2: holds after every step"), "{text}");
        assert!(text.contains("Keychron Receiver"), "{text}");
        assert!(text.contains("0x4/0x0"), "{text}");
        assert!(text.contains("JIS (changes)"), "{text}");
        let expected = expected(&plan);
        assert_eq!(expected.apply, Some(PendingAction::ResetKeyboard));
        assert_eq!(expected.steps, plan.checked.steps);

        // Answering "no other input" makes it the only usable keyboard: no reset in place.
        let alone = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &keychron,
            LayoutChoice::Jis,
            &no_other_input(&RESET),
        )
        .unwrap();
        assert_eq!(alone.apply, PendingAction::RestartPc);
        assert!(plan_text(&snapshot, &alone, None).contains("only one you can type with"));

        // US is what it has already.
        let same = plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            &keychron,
            LayoutChoice::Us,
            &RESET,
        )
        .unwrap();
        assert!(nothing_to_change(&snapshot, &same));
    }

    #[test]
    fn migrate_from_fixed_jis() {
        let mut snapshot = fixtures::dev_machine();
        snapshot.global = fixtures::global_fixed_jis();
        let keychron = fixtures::keychron().instance_id;
        let plan = plan_migration(
            &snapshot.keyboards,
            &snapshot.global,
            mklm_core::Layout::Jis,
            &[(keychron, LayoutChoice::Us)],
        )
        .unwrap();
        let text = plan_text(&snapshot, &plan, None);
        assert!(text.contains(r"Services\i8042prt\Parameters"), "{text}");
        assert!(
            text.contains("OverrideKeyboardType     7 -> (delete)"),
            "{text}"
        );
        assert!(
            text.contains("Takes effect: at the next PC restart"),
            "{text}"
        );
        assert!(text.contains("INV-PS2: holds after every step"), "{text}");
    }

    fn baseline(target: WriteTarget, name: &str, value: RegValue) -> BaselineRecord {
        let key = ValueKey {
            target,
            name: name.into(),
        };
        BaselineRecord {
            schema_version: 1,
            key_path: key.key_path(),
            key,
            value,
            captured_at: Timestamp(1),
            captured_by: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
        }
    }

    #[test]
    fn restore_previews() {
        let mut snapshot = fixtures::dev_machine();
        let keychron = WriteTarget::Device {
            instance_id: fixtures::keychron().instance_id,
        };
        // MKLM set the Keychron to JIS (7/2) from 4/0.
        for kb in &mut snapshot.keyboards {
            if kb.instance_id == fixtures::keychron().instance_id {
                kb.overrides.keyboard_type_override = Some(7);
                kb.overrides.keyboard_subtype_override = Some(2);
            }
        }
        let journal = Journal {
            baselines: vec![
                baseline(
                    keychron.clone(),
                    value_names::HID_TYPE,
                    RegValue::Dword { value: 4 },
                ),
                baseline(
                    keychron.clone(),
                    value_names::HID_SUBTYPE,
                    RegValue::Dword { value: 0 },
                ),
            ],
            ..Journal::default()
        };
        let preview = preview_restore(
            &snapshot,
            &journal,
            &RestoreScope::All,
            ConflictPolicy::Report,
            &RESET,
        )
        .unwrap();
        // No journal entry has written them: the expectation is the baseline, so 7/2 counts as a
        // change outside MKLM.
        assert_eq!(preview.writes.len(), 0);
        assert_eq!(preview.conflicts.len(), 2);
        let text = restore_text(&preview);
        assert!(text.contains("the restore stops and shows them"), "{text}");

        let preview = preview_restore(
            &snapshot,
            &journal,
            &RestoreScope::Device {
                instance_id: fixtures::keychron().instance_id,
            },
            ConflictPolicy::Overwrite,
            &RESET,
        )
        .unwrap();
        assert_eq!(preview.writes.len(), 2);
        assert_eq!(preview.apply, Some(PendingAction::ResetKeyboard));
        assert_eq!(preview.order, Ok(None));
        let text = restore_text(&preview);
        assert!(text.contains("KeyboardTypeOverride: 7 -> 4"), "{text}");
        assert!(text.contains("INV-PS2: holds after every step"), "{text}");

        // The built-in keyboard is not in the Keychron's scope.
        let preview = preview_restore(
            &snapshot,
            &Journal::default(),
            &RestoreScope::Device {
                instance_id: fixtures::internal_ps2().instance_id,
            },
            ConflictPolicy::Report,
            &RESET,
        )
        .unwrap();
        assert!(restore_text(&preview).starts_with("Nothing to restore"));
    }

    fn entry(state: OpState) -> JournalEntry {
        let keychron = fixtures::keychron().instance_id;
        let target = WriteTarget::Device {
            instance_id: keychron.clone(),
        };
        JournalEntry {
            schema_version: 1,
            op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
            seq: 1,
            kind: OpKind::SetLayout {
                requested: keychron.clone(),
                instance_ids: vec![keychron],
                layout: LayoutChoice::Jis,
            },
            state,
            boot_id: mklm_core::BootId(1),
            owner: ProcessIdentity {
                pid: 1,
                creation_time: 1,
            },
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            apply: Some(PendingAction::Reconnect),
            countdown: None,
            records: vec![ValueRecord {
                key_path: ValueKey {
                    target: target.clone(),
                    name: value_names::HID_TYPE.into(),
                }
                .key_path(),
                target,
                name: value_names::HID_TYPE.into(),
                baseline: RegValue::Dword { value: 4 },
                before: RegValue::Dword { value: 4 },
                intended: RegValue::Dword { value: 7 },
                last_written: Some(RegValue::Dword { value: 7 }),
                conflict: None,
                resolve_to: None,
                write_error: None,
                skipped: None,
            }],
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn undo_and_revert_previews() {
        assert!(undo_text(&Journal::default(), &|_| None).starts_with("Nothing to undo"));
        let at = |value| move |_: &ValueRecord| Some(RegValue::Dword { value });
        let waiting = Journal {
            entries: vec![entry(OpState::AwaitingConfirm)],
            ..Journal::default()
        };
        let text = undo_text(&waiting, &at(7));
        assert!(text.contains("Undo 3f2a9c1e"), "{text}");
        assert!(text.contains("KeyboardTypeOverride: 7 -> 4"), "{text}");
        let conflict = Journal {
            entries: vec![entry(OpState::Conflict)],
            ..Journal::default()
        };
        assert!(undo_text(&conflict, &at(5)).contains("left alone"));
        assert!(!undo_text(&conflict, &at(7)).contains("left alone"));
        let text = revert_text(&entry(OpState::Confirmed), &at(7));
        assert!(text.contains("KeyboardTypeOverride: 7 -> 4"), "{text}");
    }

    #[test]
    fn post_reboot_rows() {
        let snapshot = fixtures::dev_machine();
        let rows = check_rows(&snapshot, &entry(OpState::PendingReboot));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Keychron Receiver");
        assert_eq!(rows[0].expected, Some(KeyboardType::US));
        assert_eq!(rows[0].reported, Some(KeyboardType::US));
        let text = check_text(&rows, false);
        assert!(text.contains("ok"), "{text}");
        assert!(text.contains("Shift+2"));
        assert!(check_text(&rows, true).contains("Raw Input cannot show"));
    }
}
