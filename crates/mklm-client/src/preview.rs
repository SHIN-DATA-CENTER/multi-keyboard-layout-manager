//! The unelevated preview of a request (design m2 F.2 step 4, review S6; m3 B.5), as data: what
//! the helper will write, how the change takes effect, what a restore or an undo would do, and the
//! rows of the post-reboot check. The CLI renders it as English text, the GUI as its confirmation
//! screens.
//!
//! `set` and `migrate` plan with `mklm_core::plan_set_layout` / `plan_migration`, the functions the
//! engine itself calls; [`expected`] turns such a plan into the `ExpectedPlan` the helper must
//! reproduce before it writes.

use mklm_core::{
    ApplyOptions, ConflictPolicy, DN_STARTED, Expect, ExpectedPlan, InvPs2Violation, Journal,
    JournalEntry, KeyboardDevice, KeyboardDriver, KeyboardType, LayoutTable, OpId, OpKind, OpState,
    OperationError, OperationPlan, PendingAction, RegValue, RestoreError, RestoreScope, RestoreTo,
    SystemSnapshot, Transport, ValueRecord, WriteTarget, apply_method, assess,
    physical_device_members, plan_restore, value_eq,
};

use crate::values::{model_value, op_value};

/// The approved plan the helper must match before it writes (design review S6).
pub fn expected(plan: &OperationPlan) -> ExpectedPlan {
    ExpectedPlan {
        steps: plan.checked.steps.clone(),
        apply: Some(plan.apply),
    }
}

/// True when every planned write finds its value already in place: the engine would journal
/// nothing and answer `NoChange`, so there is no need to ask for elevation.
pub fn nothing_to_change(snapshot: &SystemSnapshot, plan: &OperationPlan) -> bool {
    plan.checked.steps.iter().all(|step| {
        step.writes.iter().all(|write| {
            model_value(
                &snapshot.keyboards,
                &snapshot.global,
                &step.target,
                &write.name,
            )
            .is_some_and(|current| value_eq(&write.name, &current, &op_value(&write.op)))
        })
    })
}

/// The options when the user answers "no other way to type" (design m2 F.1): the plan made with
/// `other_input_available = false`, to tell the user what that answer means.
pub fn no_other_input(options: &ApplyOptions) -> ApplyOptions {
    ApplyOptions {
        allow_live_reset: options.allow_live_reset,
        other_input_available: false,
    }
}

/// A value restore-to-baseline writes or leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRow {
    pub target: WriteTarget,
    pub key_path: String,
    pub name: String,
    pub current: RegValue,
    pub baseline: RegValue,
    /// What MKLM last wrote there (the compare-and-swap expectation, design m2 C.8).
    pub expect: RegValue,
}

/// Restore-to-baseline as the helper would do it now (design m2 D.5 steps 2 to 5), from the
/// unelevated snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePreview {
    pub writes: Vec<RestoreRow>,
    /// Values already at their baseline.
    pub unchanged: usize,
    /// Values of keyboards that no longer exist (left alone, design review C3).
    pub removed: Vec<String>,
    /// Values changed outside MKLM, and what the policy does with them.
    pub conflicts: Vec<RestoreRow>,
    pub policy: ConflictPolicy,
    pub apply: Option<PendingAction>,
    /// Phase order and INV-PS2 (`plan_restore`): `Ok(Some(_))` when the baseline itself broke
    /// INV-PS2 and is put back as found.
    pub order: Result<Option<InvPs2Violation>, RestoreError>,
    /// Open entries the restore closes as superseded (design review C7).
    pub supersedes: Vec<OpId>,
}

pub fn preview_restore(
    snapshot: &SystemSnapshot,
    journal: &Journal,
    scope: &RestoreScope,
    policy: ConflictPolicy,
    options: &ApplyOptions,
) -> Result<RestorePreview, OperationError> {
    let members: Option<Vec<String>> = match scope {
        RestoreScope::All => None,
        RestoreScope::Device { instance_id } => Some(
            physical_device_members(&snapshot.keyboards, instance_id)?
                .iter()
                .map(|kb| kb.instance_id.clone())
                .collect(),
        ),
    };
    let mut preview = RestorePreview {
        writes: Vec::new(),
        unchanged: 0,
        removed: Vec::new(),
        conflicts: Vec::new(),
        policy,
        apply: None,
        order: Ok(None),
        supersedes: Vec::new(),
    };
    for baseline in &journal.baselines {
        let key = &baseline.key;
        if let Some(members) = &members {
            let in_scope = match &key.target {
                WriteTarget::Device { instance_id } => {
                    members.iter().any(|m| m.eq_ignore_ascii_case(instance_id))
                }
                WriteTarget::Global => false,
            };
            if !in_scope {
                continue;
            }
        }
        let Some(current) = model_value(
            &snapshot.keyboards,
            &snapshot.global,
            &key.target,
            &key.name,
        ) else {
            preview
                .removed
                .push(format!("{}\\{}", baseline.key_path, key.name));
            continue;
        };
        let expect = journal
            .latest_record(key)
            .and_then(|(_, record)| record.last_written.clone())
            .unwrap_or_else(|| baseline.value.clone());
        let row = RestoreRow {
            target: key.target.clone(),
            key_path: baseline.key_path.clone(),
            name: key.name.clone(),
            current: current.clone(),
            baseline: baseline.value.clone(),
            expect: expect.clone(),
        };
        if value_eq(&key.name, &current, &baseline.value) {
            preview.unchanged += 1;
        } else if value_eq(&key.name, &current, &expect) {
            preview.writes.push(row);
        } else {
            if policy == ConflictPolicy::Overwrite {
                preview.writes.push(row.clone());
            }
            preview.conflicts.push(row);
        }
    }
    let records: Vec<ValueRecord> = preview
        .writes
        .iter()
        .map(|row| ValueRecord {
            target: row.target.clone(),
            key_path: row.key_path.clone(),
            name: row.name.clone(),
            baseline: row.baseline.clone(),
            before: row.current.clone(),
            intended: row.baseline.clone(),
            last_written: None,
            conflict: None,
            resolve_to: None,
            write_error: None,
            skipped: None,
        })
        .collect();
    if records.is_empty() {
        return Ok(preview);
    }
    preview.order = plan_restore(
        &records,
        RestoreTo::Baseline,
        &|_| Expect::Any,
        &snapshot.keyboards,
        &snapshot.global,
        &journal.baselines,
    )
    .map(|plan| plan.restores_inv_ps2_violation);
    preview.apply = Some(if records.iter().any(ValueRecord::is_boot_time) {
        PendingAction::RestartPc
    } else {
        let mut targets: Vec<&KeyboardDevice> = Vec::new();
        for record in &records {
            if let WriteTarget::Device { instance_id } = &record.target
                && let Some(kb) = snapshot
                    .keyboards
                    .iter()
                    .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
                && !targets.iter().any(|t| t.instance_id == kb.instance_id)
            {
                targets.push(kb);
            }
        }
        let only_usable =
            !options.other_input_available || !other_keyboard_usable(&snapshot.keyboards, &targets);
        apply_method(&targets, false, only_usable, options.allow_live_reset)
    });
    let keys: Vec<String> = records
        .iter()
        .map(|r| r.key().canonical().to_ascii_uppercase())
        .collect();
    preview.supersedes = journal
        .entries
        .iter()
        .filter(|e| e.state.is_open() && !e.state.is_in_flight())
        .filter(|e| {
            e.records
                .iter()
                .any(|r| keys.contains(&r.key().canonical().to_ascii_uppercase()))
        })
        .map(|e| e.op_id.clone())
        .collect();
    Ok(preview)
}

/// True when some keyboard outside `targets` (and their external containers) is connected,
/// started and not virtual: the rule `plan_set_layout` and the engine's restore apply
/// (plan 1.4, "the only usable keyboard").
pub fn other_keyboard_usable(keyboards: &[KeyboardDevice], targets: &[&KeyboardDevice]) -> bool {
    let containers: Vec<&str> = targets
        .iter()
        .filter(|kb| !kb.in_internal_container())
        .filter_map(|kb| kb.known_container_id())
        .collect();
    keyboards.iter().any(|kb| {
        let is_target = targets
            .iter()
            .any(|t| t.instance_id.eq_ignore_ascii_case(&kb.instance_id));
        let same_container = !kb.in_internal_container()
            && kb
                .known_container_id()
                .is_some_and(|c| containers.iter().any(|t| t.eq_ignore_ascii_case(c)));
        !is_target
            && !same_container
            && kb.present
            && kb.dev_node_status.is_some_and(|s| s & DN_STARTED != 0)
            && kb.transport != Transport::Virtual
    })
}

/// One value `undo` (design m2 D.10) would write back, or leave alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoValue<'a> {
    pub record: &'a ValueRecord,
    /// The value stored now (`None`: unknown, or the keyboard no longer exists).
    pub now: Option<RegValue>,
    /// False for a value of an entry in conflict that changed outside MKLM: undo leaves it
    /// alone (resolve it instead).
    pub restorable: bool,
}

/// One open entry `undo` would undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry<'a> {
    pub entry: &'a JournalEntry,
    pub values: Vec<UndoValue<'a>>,
    /// The entry involves boot-time values: the PC must restart afterwards.
    pub restart_needed: bool,
}

/// What `undo` would do now: interrupted entries are recovered first, then every open entry that
/// waits for the user is undone, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UndoPreview<'a> {
    pub recovered_first: Vec<&'a JournalEntry>,
    pub undone: Vec<UndoEntry<'a>>,
}

impl UndoPreview<'_> {
    /// Nothing waits for keep, revert, a restart or a conflict decision.
    pub fn is_empty(&self) -> bool {
        self.recovered_first.is_empty() && self.undone.is_empty()
    }
}

pub fn undo_preview<'a>(
    journal: &'a Journal,
    current: &dyn Fn(&ValueRecord) -> Option<RegValue>,
) -> UndoPreview<'a> {
    let open = journal.open_entries();
    let recovered_first = open
        .iter()
        .copied()
        .filter(|e| e.state.is_in_flight())
        .collect();
    let mut waiting: Vec<&JournalEntry> = open
        .iter()
        .copied()
        .filter(|e| !e.state.is_in_flight())
        .collect();
    waiting.sort_by(|a, b| (b.seq, &b.op_id).cmp(&(a.seq, &a.op_id)));
    let undone = waiting
        .into_iter()
        .map(|entry| UndoEntry {
            entry,
            values: entry
                .records
                .iter()
                .map(|record| {
                    let now = current(record);
                    let restorable = match entry.state {
                        OpState::Conflict => {
                            let expect = record.last_written.as_ref().unwrap_or(&record.intended);
                            now.as_ref()
                                .is_some_and(|now| value_eq(&record.name, now, expect))
                        }
                        _ => true,
                    };
                    UndoValue {
                        record,
                        now,
                        restorable,
                    }
                })
                .collect(),
            restart_needed: entry.touches_boot_time_values(),
        })
        .collect();
    UndoPreview {
        recovered_first,
        undone,
    }
}

/// One row of the post-reboot check (design m2 D.7): what a keyboard should report now and what
/// Raw Input says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    pub instance_id: String,
    pub name: String,
    pub expected: Option<KeyboardType>,
    pub reported: Option<KeyboardType>,
    pub layout: Option<LayoutTable>,
}

impl CheckRow {
    /// Raw Input reports what the stored values predict.
    pub fn matches(&self) -> bool {
        matches!((self.expected, self.reported), (Some(e), Some(r)) if e == r)
    }

    /// Raw Input does not report the keyboard (not connected).
    pub fn not_reported(&self) -> bool {
        self.reported.is_none()
    }
}

/// The keyboards an entry wrote (for a migration: every connected kbdhid and i8042prt keyboard),
/// with the type their stored values predict and the type Raw Input reports now.
pub fn check_rows(snapshot: &SystemSnapshot, entry: &JournalEntry) -> Vec<CheckRow> {
    let assessment = assess(snapshot);
    let mut ids: Vec<&str> = Vec::new();
    for record in &entry.records {
        if let WriteTarget::Device { instance_id } = &record.target
            && !ids.iter().any(|id| id.eq_ignore_ascii_case(instance_id))
        {
            ids.push(instance_id);
        }
    }
    if matches!(entry.kind, OpKind::Migrate { .. }) {
        // A migration changes what every keyboard types; list them all.
        for kb in &snapshot.keyboards {
            if matches!(kb.driver, KeyboardDriver::Kbdhid | KeyboardDriver::I8042prt)
                && kb.present
                && !ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
            {
                ids.push(&kb.instance_id);
            }
        }
    }
    ids.iter()
        .filter_map(|id| {
            assessment
                .keyboards
                .iter()
                .find(|ka| ka.instance_id.eq_ignore_ascii_case(id))
        })
        .map(|ka| CheckRow {
            instance_id: ka.instance_id.clone(),
            name: ka.display_name.clone(),
            expected: ka.predicted_type,
            reported: ka.reported_type,
            layout: ka.after_restart.as_ref().map(|l| l.table.clone()),
        })
        .collect()
}
