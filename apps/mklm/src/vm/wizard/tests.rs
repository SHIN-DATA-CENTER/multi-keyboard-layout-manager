use mklm_core::{
    BootId, DeviceOverrides, Journal, JournalEntry, KeyboardType, OpId, OpState, ProcessIdentity,
    Timestamp, fixtures, plan_migration, value_names,
};

use super::*;
use crate::detect::scancode;
use crate::vm::{snapshot_values, unexpected_latin};

const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
const KEYCHRON_ROW: &str = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
const VXE: &str = r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001";
const VXE_ROW: &str = "{BDA55856-0DCA-5B66-8949-3F281E7B4A28}";
const NAMES: [&str; 3] = [
    "Keychron Receiver",
    "VXE R1SE+",
    "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)",
];

fn rows_of(
    snapshot: &SystemSnapshot,
    choices: &[(String, Layout)],
    deletes: &[String],
    foreign: &[ForeignValues],
    lang: Lang,
) -> Vec<WizardKeyboard> {
    wizard_keyboards(&KeyboardsInput {
        snapshot,
        settings: &Settings::default(),
        choices,
        deletes,
        foreign,
        lang,
    })
}

fn plan_of(
    snapshot: &SystemSnapshot,
    choices: &[(&str, Layout)],
    standard: Option<Layout>,
) -> WizardPlan {
    let choices: Vec<(String, Layout)> = choices
        .iter()
        .map(|(row, layout)| (row.to_string(), *layout))
        .collect();
    let rows = rows_of(snapshot, &choices, &[], &[], Lang::Ja);
    wizard_plan(snapshot, &plan_rows(&rows), standard)
}

/// A fixed-JIS PC (before the M0 migration) with a new keyboard that has no values yet.
fn fixed_jis_with_new_keyboard() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    snapshot.global = fixtures::global_fixed_jis();
    snapshot.keyboards[1].overrides = DeviceOverrides::default();
    snapshot.keyboards[1].reported_type = Some(KeyboardType::JIS);
    snapshot
}

fn plain_japanese(text: &str) {
    let found = unexpected_latin(text, &NAMES);
    assert!(found.is_empty(), "{found:?} in {text}");
}

#[test]
fn the_steps_say_where_they_are() {
    assert_eq!(WizardStep::Keyboards.number(), (3, 4));
    assert_eq!(WizardStep::Welcome.next(), Some(WizardStep::InputMethods));
    assert_eq!(WizardStep::Summary.next(), None);
    assert_eq!(WizardStep::Welcome.previous(), None);
    assert_eq!(
        crate::i18n::wizard::step_heading(WizardStep::Keyboards, Lang::Ja),
        "手順 3 / 4: キーボードの配列"
    );
    assert_eq!(
        crate::i18n::wizard::step_heading(WizardStep::Summary, Lang::En),
        "Step 4 of 4: Summary"
    );
}

#[test]
fn the_connected_keyboards_default_to_how_they_are_set() {
    let snapshot = fixtures::dev_machine();
    let rows = rows_of(&snapshot, &[], &[], &[], Lang::Ja);
    // The disconnected BLE mouse is not listed.
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec![BUILT_IN, KEYCHRON_ROW, VXE_ROW]
    );
    assert_eq!(
        rows.iter().map(|row| row.choice).collect::<Vec<_>>(),
        vec![Some(Layout::Jis), Some(Layout::Us), Some(Layout::Jis)]
    );
    assert_eq!(rows[1].target, KEYCHRON);
    assert_eq!(rows[1].current, "US として動作中");
    assert!(rows[1].note.starts_with("レシーバー: "));
    assert!(
        rows.iter()
            .all(|row| row.problem.is_empty() && !row.problem_deletable)
    );
    // A choice is kept per row.
    let rows = rows_of(
        &snapshot,
        &[(KEYCHRON_ROW.to_lowercase(), Layout::Jis)],
        &[],
        &[],
        Lang::Ja,
    );
    assert_eq!(rows[1].choice, Some(Layout::Jis));
    assert_eq!(rows[1].default, Some(Layout::Us));
    // Nothing differs: nothing to do.
    assert_eq!(plan_of(&snapshot, &[], None), WizardPlan::NothingToDo);
}

#[test]
fn a_change_that_waits_is_not_undone_by_the_default() {
    // The Keychron is stored as JIS but still types US: the default is the stored layout, so
    // leaving it as it is sends nothing.
    let mut snapshot = fixtures::dev_machine();
    snapshot.keyboards[1].overrides.keyboard_type_override = Some(7);
    snapshot.keyboards[1].overrides.keyboard_subtype_override = Some(2);
    let rows = rows_of(&snapshot, &[], &[], &[], Lang::Ja);
    assert_eq!(rows[1].choice, Some(Layout::Jis));
    assert_eq!(rows[1].current, "US として動作中");
    assert_eq!(plan_of(&snapshot, &[], None), WizardPlan::NothingToDo);
}

#[test]
fn per_keyboard_changes_come_one_by_one_restart_last() {
    let snapshot = fixtures::dev_machine();
    let plan = plan_of(
        &snapshot,
        &[
            (BUILT_IN, Layout::Us),
            (VXE_ROW, Layout::Us),
            (KEYCHRON_ROW, Layout::Jis),
        ],
        Some(Layout::Us),
    );
    // Reset in place (USB), then reconnect (BLE), then the PC restart (PS/2): a restart would
    // block the others until it happens. The standard layout is not changed here.
    assert_eq!(
        plan,
        WizardPlan::SetLayouts(vec![
            (KEYCHRON.into(), LayoutChoice::Jis),
            (VXE.into(), LayoutChoice::Us),
            (BUILT_IN.into(), LayoutChoice::Us),
        ])
    );
    // A keyboard that follows the PC's standard (JIS) needs nothing to type JIS.
    assert_eq!(
        plan_of(&snapshot, &[(VXE_ROW, Layout::Jis)], None),
        WizardPlan::NothingToDo
    );
}

#[test]
fn a_new_us_keyboard_on_a_fixed_jis_pc_is_migrated_with_its_assignment() {
    // Review U3: the migration carries the US keyboard, so it is US after the one restart.
    let snapshot = fixed_jis_with_new_keyboard();
    let plan = plan_of(&snapshot, &[(KEYCHRON_ROW, Layout::Us)], None);
    assert_eq!(
        plan,
        WizardPlan::Migrate {
            standard: Layout::Jis,
            assignments: vec![(KEYCHRON.into(), LayoutChoice::Us)],
        }
    );
    let WizardPlan::Migrate {
        standard,
        assignments,
    } = &plan
    else {
        unreachable!()
    };
    let planned = plan_migration(
        &snapshot.keyboards,
        &snapshot.global,
        *standard,
        assignments,
    )
    .unwrap();
    assert_eq!(planned.apply, PendingAction::RestartPc);
    // Every keyboard as fixed mode gives it (JIS): nothing is written, no restart.
    assert_eq!(plan_of(&snapshot, &[], None), WizardPlan::NothingToDo);
}

#[test]
fn the_migration_keeps_what_is_chosen_for_keyboards_it_would_change() {
    // The Keychron holds US values that fixed mode ignores: kept as JIS, it needs an assignment,
    // or the migration would make it US.
    let mut snapshot = fixtures::dev_machine();
    snapshot.global = fixtures::global_fixed_jis();
    let rows = rows_of(&snapshot, &[], &[], &[], Lang::Ja);
    assert_eq!(rows[1].choice, Some(Layout::Jis));
    assert!(
        rows[1].problem.contains("固定モードのため"),
        "{}",
        rows[1].problem
    );
    assert_eq!(
        plan_of(&snapshot, &[(VXE_ROW, Layout::Us)], None),
        WizardPlan::Migrate {
            standard: Layout::Jis,
            assignments: vec![
                (KEYCHRON.into(), LayoutChoice::Jis),
                (VXE.into(), LayoutChoice::Us),
            ],
        }
    );
    // Another standard layout: a keyboard without values keeps JIS through an assignment; a new
    // keyboard chosen US follows the new standard without one; the built-in keyboard is pinned.
    let snapshot = fixed_jis_with_new_keyboard();
    let plan = plan_of(&snapshot, &[(KEYCHRON_ROW, Layout::Us)], Some(Layout::Us));
    assert_eq!(
        plan,
        WizardPlan::Migrate {
            standard: Layout::Us,
            assignments: vec![(VXE.into(), LayoutChoice::Jis)],
        }
    );
    assert_eq!(
        after_migration(&snapshot.keyboards[0], &snapshot.global, Layout::Us),
        Some(LayoutTable::Jis)
    );
}

#[test]
fn values_the_driver_does_not_read_may_be_deleted() {
    let mut snapshot = fixtures::dev_machine();
    // HID names on the PS/2 keyboard: i8042prt never reads them.
    snapshot.keyboards[0].overrides.keyboard_type_override = Some(4);
    snapshot.keyboards[0].overrides.keyboard_subtype_override = Some(0);
    let foreign = [ForeignValues {
        instance_id: r"HID\VID_3434&PID_D027&MI_01&COL03\8&2A1B&0&0002".into(),
        keyboard: KEYCHRON.into(),
        names: vec![value_names::HID_TYPE.into()],
    }];
    let deletes = [BUILT_IN.to_string()];
    let rows = rows_of(&snapshot, &[], &deletes, &foreign, Lang::Ja);
    assert!(rows[0].problem_deletable);
    assert_eq!(rows[0].problem_choice, ProblemChoice::Delete);
    assert!(rows[0].problem.contains("ドライバーが読まない値"));
    // The mouse collection's value is explained, never offered for deletion (plan 1.5).
    assert!(!rows[1].problem_deletable);
    assert!(rows[1].problem.contains("キーボード以外の部分"));
    assert!(
        rows[1]
            .details
            .iter()
            .any(|line| line.starts_with("reg delete \"HKLM\\SYSTEM\\CurrentControlSet\\Enum\\HID\\VID_3434&PID_D027&MI_01&COL03\\8&2A1B&0&0002\\Device Parameters\" /v KeyboardTypeOverride /f")),
        "{:?}",
        rows[1].details
    );
    for row in &rows {
        plain_japanese(&row.problem);
    }
    let tasks = cleanup_tasks(&snapshot, &rows, &deletes);
    assert_eq!(
        tasks,
        vec![CleanupTask {
            row: BUILT_IN.into(),
            name: NAMES[2].into(),
            instance_id: BUILT_IN.into(),
            names: vec![
                value_names::HID_TYPE.into(),
                value_names::HID_SUBTYPE.into()
            ],
        }]
    );
    // Kept: nothing to delete.
    let kept = rows_of(&snapshot, &[], &[], &foreign, Lang::Ja);
    assert!(cleanup_tasks(&snapshot, &kept, &[]).is_empty());
}

fn cleanup_entry(state: OpState) -> JournalEntry {
    JournalEntry {
        schema_version: 2,
        op_id: OpId::parse("5d6e7f80-1a2b-4c3d-8e9f-0a1b2c3d4e5f").unwrap(),
        seq: 1,
        kind: OpKind::Cleanup {
            instance_id: BUILT_IN.into(),
            names: vec![
                value_names::HID_TYPE.into(),
                value_names::HID_SUBTYPE.into(),
            ],
        },
        state,
        boot_id: BootId(1),
        owner: ProcessIdentity {
            pid: 1,
            creation_time: 1,
        },
        created_at: Timestamp(1_790_000_000_000),
        updated_at: Timestamp(1_790_000_000_000),
        apply: None,
        countdown: None,
        records: Vec::new(),
        context: Vec::new(),
        failure: None,
        revert_mode: None,
        apply_pending: None,
        history: Vec::new(),
    }
}

#[test]
fn an_open_cleanup_is_decided_first_then_cleanups_then_layouts() {
    let snapshot = fixtures::dev_machine();
    let rows = rows_of(&snapshot, &[], &[], &[], Lang::Ja);
    let plan = WizardPlan::SetLayouts(vec![(KEYCHRON.into(), LayoutChoice::Jis)]);
    let cleanup = CleanupTask {
        row: BUILT_IN.into(),
        name: NAMES[2].into(),
        instance_id: BUILT_IN.into(),
        names: vec![value_names::HID_TYPE.into()],
    };
    let journal = Journal {
        entries: vec![cleanup_entry(OpState::AwaitingConfirm)],
        ..Journal::default()
    };
    let awaiting = awaiting_cleanup(&journal).unwrap();
    assert_eq!(awaiting.instance_id, BUILT_IN);
    assert!(matches!(
        next_task(
            Some(awaiting),
            true,
            std::slice::from_ref(&cleanup),
            &plan,
            &rows
        ),
        WizardTask::KeepCleanup(_)
    ));
    // Blocked by something else: nothing more now.
    assert_eq!(
        next_task(None, true, std::slice::from_ref(&cleanup), &plan, &rows),
        WizardTask::Finish
    );
    assert_eq!(
        next_task(None, false, std::slice::from_ref(&cleanup), &plan, &rows),
        WizardTask::Cleanup(cleanup)
    );
    assert_eq!(
        next_task(None, false, &[], &plan, &rows),
        WizardTask::SetLayout {
            row: KEYCHRON_ROW.into(),
            name: "Keychron Receiver".into(),
            choice: LayoutChoice::Jis,
        }
    );
    assert_eq!(
        next_task(None, false, &[], &WizardPlan::NothingToDo, &rows),
        WizardTask::Finish
    );
    // A kept cleanup no longer waits.
    let kept = Journal {
        entries: vec![cleanup_entry(OpState::Confirmed)],
        ..Journal::default()
    };
    assert_eq!(awaiting_cleanup(&kept), None);
}

#[test]
fn the_input_methods_page() {
    let page = input_methods_page(&fixtures::input_methods(), Lang::Ja);
    assert_eq!(
        page.lines,
        vec![
            "あなたの入力方式: 日本語、英語 (US)".to_string(),
            "サインイン画面の入力方式: 日本語、英語 (US)".to_string(),
        ]
    );
    assert_eq!(page.warnings.len(), 1);
    assert!(page.warnings[0].starts_with("日本語以外の入力方式があります（英語 (US)）"));
    assert!(page.settings_buttons);
    // KLIDs only in the details (review U5).
    assert!(page.details[0].contains("00000411, 00000409"));
    plain_japanese(&snapshot_values(&page.snapshot_text()));
    // An English sign-in screen: the warning and the way to copy the settings.
    let english = mklm_core::InputMethods {
        user_preload: vec!["00000409".into(), "00000411".into()],
        sign_in_preload: vec!["00000409".into()],
        loaded_layouts: vec![0x0409_0409, 0x0411_0411],
    };
    let page = input_methods_page(&english, Lang::Ja);
    assert!(
        page.warnings
            .iter()
            .any(|w| w.starts_with("サインイン画面の入力方式が 英語 (US) です")),
        "{:?}",
        page.warnings
    );
    assert!(
        page.warnings
            .iter()
            .any(|w| w.starts_with("既定の入力方式が 英語 (US) です"))
    );
    assert!(page.notes.iter().any(|n| n.contains("設定のコピー")));
    plain_japanese(&snapshot_values(&page.snapshot_text()));
    let page = input_methods_page(&english, Lang::En);
    assert!(page.notes.iter().any(|n| n.contains("Copy settings")));
}

#[test]
fn the_detection_names_the_keyboard_and_selects_its_row() {
    let snapshot = fixtures::dev_machine();
    let rows = rows_of(&snapshot, &[], &[], &[], Lang::Ja);
    let mut detection = Detection::new();
    detection.press(KEYCHRON, scancode::YEN);
    detection.press(KEYCHRON, scancode::RO);
    let page = keyboards_page(&KeyboardsContext {
        rows: Some(&rows),
        detection: &detection,
        detected_name: Some("Keychron Receiver"),
        lang: Lang::Ja,
    });
    assert_eq!(
        page.detect.verdict,
        "✓ Keychron Receiver は JIS 配列のキーボードです"
    );
    // The row is US: one click makes it JIS.
    assert_eq!(page.detect.choose_text, "JIS を選ぶ");
    assert_eq!(
        detected_row(&rows, KEYCHRON).map(|row| row.id.as_str()),
        Some(KEYCHRON_ROW)
    );
    plain_japanese(&snapshot_values(&page.snapshot_text()));
    // Before a read: nothing to choose yet.
    let waiting = keyboards_page(&KeyboardsContext {
        rows: None,
        detection: &Detection::new(),
        detected_name: None,
        lang: Lang::En,
    });
    assert!(waiting.busy && waiting.keyboards.is_empty());
}

fn summary_of(
    rows: &[WizardKeyboard],
    plan: &WizardPlan,
    task: &WizardTask,
    migration: MigrationStatus<'_>,
    lang: Lang,
) -> WizardPage {
    summary_page(&SummaryContext {
        rows: Some(rows),
        plan,
        cleanups: &[],
        task,
        blocked: None,
        standard_now: &LayoutTable::Jis,
        mode: GlobalMode::Fixed,
        fixed: Some(Layout::Jis),
        standard: Layout::Jis,
        migration,
        ran: false,
        autostart: true,
        uac_notice_seen: true,
        elevated: false,
        lang,
    })
}

#[test]
fn the_migration_summary_is_what_is_sent() {
    let snapshot = fixed_jis_with_new_keyboard();
    let rows = rows_of(
        &snapshot,
        &[(KEYCHRON_ROW.into(), Layout::Us)],
        &[],
        &[],
        Lang::Ja,
    );
    let plan = wizard_plan(&snapshot, &plan_rows(&rows), None);
    let WizardPlan::Migrate {
        standard,
        assignments,
    } = &plan
    else {
        panic!("{plan:?}")
    };
    let task = WizardTask::Migrate {
        standard: *standard,
        assignments: assignments.clone(),
    };
    let planned = plan_migration(
        &snapshot.keyboards,
        &snapshot.global,
        *standard,
        assignments,
    );
    let page = summary_of(
        &rows,
        &plan,
        &task,
        MigrationStatus::Planned {
            plan: &planned,
            snapshot: &snapshot,
        },
        Lang::Ja,
    );
    assert!(page.can_next && page.migration && page.standard_visible);
    assert_eq!(page.standard_selected, Some(0));
    assert_eq!(page.standard_choices[0].text, "JIS（おすすめ: 今の JIS）");
    assert_eq!(page.next_text, "変更する（次に Windows の確認が出ます）");
    assert!(!page.uac_line.is_empty());
    assert!(
        page.details
            .iter()
            .any(|line| line.contains("LayerDriver JPN") || line.contains("OverrideKeyboardType"))
    );
    plain_japanese(&snapshot_values(&page.snapshot_text()));
    // While it is prepared: nothing can be sent.
    let preparing = summary_of(&rows, &plan, &task, MigrationStatus::Preparing, Lang::Ja);
    assert!(preparing.busy && !preparing.can_next);
    // Refused by the rules: said, not sent.
    let refused: Result<OperationPlan, OperationError> = Err(OperationError::InconsistentGlobal);
    let page = summary_of(
        &rows,
        &plan,
        &task,
        MigrationStatus::Planned {
            plan: &refused,
            snapshot: &snapshot,
        },
        Lang::Ja,
    );
    assert!(!page.can_next);
    assert_eq!(
        page.note,
        "PC 全体のキーボードの値が食い違っているため、この変更はできません。"
    );
}
