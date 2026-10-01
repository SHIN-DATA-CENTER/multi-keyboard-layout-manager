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

mod standard;
pub use standard::{StandardRole, StandardRow, standard_rows};

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
        other_input_available: false,
        ..*options
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
    /// Physical keyboard identity, shared by its HID collections.
    pub group_id: String,
    pub present: bool,
    pub remote: bool,
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

    /// Raw Input does not report the keyboard; presence is tracked separately.
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
    if let OpKind::SetStandard { keyboards, .. } = &entry.kind {
        ids.clear();
        for (id, choice) in keyboards {
            if *choice == mklm_core::LayoutChoice::Standard {
                ids.push(id);
            }
        }
        for kb in &snapshot.keyboards {
            if kb.present
                && !kb.is_remote_desktop()
                && kb.driver == KeyboardDriver::Kbdhid
                && kb.transport == Transport::Virtual
                && kb
                    .predicted_type(&snapshot.global)
                    .is_some_and(|ty| ty.per_keyboard_table().is_none())
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
            group_id: assessment
                .groups
                .iter()
                .find(|g| g.keyboards.contains(&ka.instance_id))
                .and_then(|g| {
                    if g.is_internal {
                        None
                    } else {
                        snapshot
                            .keyboards
                            .iter()
                            .find(|kb| kb.instance_id == ka.instance_id)
                            .and_then(|kb| kb.known_container_id())
                            .map(str::to_owned)
                    }
                })
                .unwrap_or_else(|| ka.instance_id.clone()),
            present: ka.present,
            remote: snapshot.os.remote_session,
            instance_id: ka.instance_id.clone(),
            name: ka.display_name.clone(),
            expected: ka.predicted_type,
            reported: ka.reported_type,
            layout: ka.after_restart.as_ref().map(|l| l.table.clone()),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckGroup {
    pub id: String,
    pub name: String,
    pub rows: Vec<CheckRow>,
}

impl CheckGroup {
    pub fn matches(&self) -> bool {
        self.rows.iter().all(CheckRow::matches)
    }
    pub fn differs(&self) -> bool {
        self.rows
            .iter()
            .any(|r| matches!((r.expected, r.reported), (Some(e), Some(a)) if e != a))
    }
}

pub fn group_check_rows(rows: &[CheckRow]) -> Vec<CheckGroup> {
    let mut groups: Vec<CheckGroup> = Vec::new();
    for row in rows {
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g.id.eq_ignore_ascii_case(&row.group_id))
        {
            group.rows.push(row.clone());
        } else {
            groups.push(CheckGroup {
                id: row.group_id.clone(),
                name: row.name.clone(),
                rows: vec![row.clone()],
            });
        }
    }
    groups
}

pub fn check_groups(snapshot: &SystemSnapshot, entry: &JournalEntry) -> Vec<CheckGroup> {
    group_check_rows(&check_rows(snapshot, entry))
}

#[cfg(test)]
mod check_tests {
    use super::*;
    use mklm_core::{Layout, LayoutChoice, fixtures};

    #[test]
    fn an_unknown_container_does_not_merge_unrelated_keyboards() {
        let (ops, baselines) = fixtures::schema_1_journal();
        let mut entry = Journal::parse(&ops, &baselines).entries.remove(0);
        let mut snapshot = fixtures::dev_machine();
        let mut one = fixtures::keychron();
        one.instance_id = "one".into();
        one.container_id = Some("{00000000-0000-0000-0000-000000000000}".into());
        let mut two = one.clone();
        two.instance_id = "two".into();
        snapshot.keyboards = vec![one, two];
        entry.kind = OpKind::SetLayout {
            requested: "one".into(),
            instance_ids: vec!["one".into(), "two".into()],
            layout: LayoutChoice::Us,
        };
        entry.records[0].target = WriteTarget::Device {
            instance_id: "one".into(),
        };
        let mut second = entry.records[0].clone();
        second.target = WriteTarget::Device {
            instance_id: "two".into(),
        };
        entry.records.push(second);
        assert_eq!(check_groups(&snapshot, &entry).len(), 2);
    }

    #[test]
    fn standard_checks_followers_and_virtual_collections_but_excludes_pins_and_rdp() {
        let (ops, baselines) = fixtures::schema_1_journal();
        let mut entry = Journal::parse(&ops, &baselines).entries.remove(0);
        let mut snapshot = fixtures::dev_machine();
        let mut follower = fixtures::keychron();
        follower.instance_id = "follower-1".into();
        follower.container_id = Some("external-group".into());
        follower.overrides = Default::default();
        let mut second = follower.clone();
        second.instance_id = "follower-2".into();
        let mut virtual_kb = follower.clone();
        virtual_kb.instance_id = "virtual".into();
        virtual_kb.container_id = None;
        virtual_kb.transport = Transport::Virtual;
        snapshot.keyboards.extend([follower, second, virtual_kb]);
        entry.kind = OpKind::SetStandard {
            from: Layout::Jis,
            to: Layout::Us,
            keyboards: vec![
                ("follower-1".into(), LayoutChoice::Standard),
                ("follower-2".into(), LayoutChoice::Standard),
                (fixtures::keychron().instance_id, LayoutChoice::Jis),
            ],
        };
        let groups = check_groups(&snapshot, &entry);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].rows.len(), 2);
        assert_eq!(groups[1].rows[0].instance_id, "virtual");
        assert!(
            groups
                .iter()
                .flat_map(|g| &g.rows)
                .all(|r| r.instance_id.starts_with("follower") || r.instance_id == "virtual")
        );
        let mut rows = groups[0].rows.clone();
        for row in &mut rows {
            row.reported = row.expected;
        }
        assert!(group_check_rows(&rows)[0].matches());
        rows[1].reported = Some(KeyboardType::US);
        let mixed = group_check_rows(&rows);
        assert!(mixed[0].differs());
        assert!(!mixed[0].matches());
        rows[1].reported = None;
        assert!(!group_check_rows(&rows)[0].matches());
    }
}
