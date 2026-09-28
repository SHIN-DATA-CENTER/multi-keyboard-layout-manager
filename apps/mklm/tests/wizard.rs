//! Snapshots of the first-run wizard (design m3 B.1, H.3) in Japanese and English: the four steps
//! for the development machine, the input methods of a PC with an English sign-in screen, the
//! problems of stored values, and the summary in each of its states — nothing to do, the ordinary
//! changes of per-keyboard mode, the migration of a fixed-JIS PC with its assignment, a cleanup
//! that waits for keep or undo, a journal that blocks the rest, and done.

mod common;

use mklm_core::{
    DeviceOverrides, GlobalMode, InputMethods, KeyboardType, Layout, LayoutChoice, LayoutTable,
    OpId, OperationError, SystemSnapshot, fixtures, plan_migration, ps2_pin_layout, value_names,
};
use mklm_gui::detect::{Detection, scancode};
use mklm_gui::i18n::Lang;
use mklm_gui::settings::Settings;
use mklm_gui::state::PrepareFailure;
use mklm_gui::vm::SnapshotText;
use mklm_gui::vm::wizard::{
    AwaitingCleanup, ForeignValues, KeyboardsContext, KeyboardsInput, MigrationStatus,
    SummaryContext, WizardKeyboard, WizardPlan, WizardTask, cleanup_tasks, input_methods_page,
    keyboards_page, next_task, plan_rows, summary_page, welcome_page, wizard_keyboards,
    wizard_plan,
};

const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
const KEYCHRON_ROW: &str = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

fn rows(
    snapshot: &SystemSnapshot,
    choices: &[(&str, Layout)],
    deletes: &[&str],
    foreign: &[ForeignValues],
    lang: Lang,
) -> Vec<WizardKeyboard> {
    let choices: Vec<(String, Layout)> = choices
        .iter()
        .map(|(row, layout)| (row.to_string(), *layout))
        .collect();
    let deletes: Vec<String> = deletes.iter().map(|row| row.to_string()).collect();
    wizard_keyboards(&KeyboardsInput {
        snapshot,
        settings: &Settings::default(),
        choices: &choices,
        deletes: &deletes,
        foreign,
        lang,
    })
}

/// The summary of `snapshot` with `choices`, as the state builds it.
struct Summary<'a> {
    snapshot: &'a SystemSnapshot,
    choices: &'a [(&'a str, Layout)],
    deletes: &'a [&'a str],
    awaiting: Option<AwaitingCleanup>,
    blocked: Option<&'a str>,
    ran: bool,
    migration_failed: Option<&'a PrepareFailure>,
}

impl<'a> Summary<'a> {
    fn new(snapshot: &'a SystemSnapshot, choices: &'a [(&'a str, Layout)]) -> Self {
        Self {
            snapshot,
            choices,
            deletes: &[],
            awaiting: None,
            blocked: None,
            ran: false,
            migration_failed: None,
        }
    }

    fn page(&self, lang: Lang) -> String {
        let rows = rows(self.snapshot, self.choices, self.deletes, &[], lang);
        let plan = wizard_plan(self.snapshot, &plan_rows(&rows), None);
        let deletes: Vec<String> = self.deletes.iter().map(|row| row.to_string()).collect();
        let cleanups = cleanup_tasks(self.snapshot, &rows, &deletes);
        let task = next_task(
            self.awaiting.clone(),
            self.blocked.is_some(),
            &cleanups,
            &plan,
            &rows,
        );
        let planned = match &task {
            WizardTask::Migrate {
                standard,
                assignments,
            } => Some(plan_migration(
                &self.snapshot.keyboards,
                &self.snapshot.global,
                *standard,
                assignments,
            )),
            _ => None,
        };
        let migration = match (&planned, self.migration_failed) {
            (Some(_), Some(failure)) => MigrationStatus::Failed(failure),
            (Some(plan), None) => MigrationStatus::Planned {
                plan,
                snapshot: self.snapshot,
            },
            (None, _) => MigrationStatus::None,
        };
        let fixed = ps2_pin_layout(&self.snapshot.global);
        let standard = match &plan {
            WizardPlan::Migrate { standard, .. } => *standard,
            _ => fixed.unwrap_or(Layout::Jis),
        };
        summary_page(&SummaryContext {
            rows: Some(&rows),
            plan: &plan,
            cleanups: &cleanups,
            task: &task,
            blocked: self.blocked,
            standard_now: &self.snapshot.global.standard_layout(),
            mode: self.snapshot.global.mode(),
            fixed,
            standard,
            migration,
            ran: self.ran,
            autostart: true,
            uac_notice_seen: true,
            elevated: false,
            lang,
        })
        .snapshot_text()
    }
}

fn fixed_jis_with_new_keyboard() -> SystemSnapshot {
    let mut snapshot = fixtures::dev_machine();
    snapshot.global = fixtures::global_fixed_jis();
    snapshot.keyboards[1].overrides = DeviceOverrides::default();
    snapshot.keyboards[1].reported_type = Some(KeyboardType::JIS);
    snapshot
}

fn with_problems() -> (SystemSnapshot, Vec<ForeignValues>) {
    let mut snapshot = fixtures::dev_machine();
    snapshot.global = fixtures::global_fixed_jis();
    // HID names on the PS/2 keyboard (its driver never reads them); US values that fixed mode
    // ignores on the Keychron; a value on the Keychron's mouse collection.
    snapshot.keyboards[0].overrides.keyboard_type_override = Some(4);
    snapshot.keyboards[0].overrides.keyboard_subtype_override = Some(0);
    let foreign = vec![ForeignValues {
        instance_id: r"HID\VID_3434&PID_D027&MI_01&COL03\8&2A1B&0&0002".into(),
        keyboard: KEYCHRON.into(),
        names: vec![value_names::HID_TYPE.into()],
    }];
    (snapshot, foreign)
}

fn section(out: &mut String, title: &str, text: &str) {
    out.push_str(&format!("=== {title}\n{text}\n"));
}

fn scenes(lang: Lang) -> String {
    let mut out = String::new();
    let dev = fixtures::dev_machine();
    section(&mut out, "welcome", &welcome_page(lang).snapshot_text());
    section(
        &mut out,
        "input methods (development machine)",
        &input_methods_page(&dev.input, lang).snapshot_text(),
    );
    section(
        &mut out,
        "input methods (English default and sign-in screen)",
        &input_methods_page(
            &InputMethods {
                user_preload: vec!["00000409".into(), "00000411".into()],
                sign_in_preload: vec!["00000409".into()],
                loaded_layouts: vec![0x0409_0409, 0x0411_0411],
            },
            lang,
        )
        .snapshot_text(),
    );
    let mut detection = Detection::new();
    detection.press(KEYCHRON, scancode::EQUAL);
    let dev_rows = rows(&dev, &[], &[], &[], lang);
    section(
        &mut out,
        "keyboards (development machine, the Keychron's first answer key)",
        &keyboards_page(&KeyboardsContext {
            rows: Some(&dev_rows),
            detection: &detection,
            detected_name: Some("Keychron Receiver"),
            lang,
        })
        .snapshot_text(),
    );
    detection.press(KEYCHRON, scancode::SLASH);
    let chosen = rows(&dev, &[(KEYCHRON_ROW, Layout::Jis)], &[], &[], lang);
    section(
        &mut out,
        "keyboards (the Keychron found to be US, JIS chosen)",
        &keyboards_page(&KeyboardsContext {
            rows: Some(&chosen),
            detection: &detection,
            detected_name: Some("Keychron Receiver"),
            lang,
        })
        .snapshot_text(),
    );
    let (problems, foreign) = with_problems();
    let problem_rows = rows(&problems, &[], &[BUILT_IN], &foreign, lang);
    section(
        &mut out,
        "keyboards (problem values, fixed mode)",
        &keyboards_page(&KeyboardsContext {
            rows: Some(&problem_rows),
            detection: &Detection::new(),
            detected_name: None,
            lang,
        })
        .snapshot_text(),
    );
    section(
        &mut out,
        "summary (nothing to do)",
        &Summary::new(&dev, &[]).page(lang),
    );
    section(
        &mut out,
        "summary (per-keyboard changes)",
        &Summary::new(&dev, &[(KEYCHRON_ROW, Layout::Jis), (BUILT_IN, Layout::Us)]).page(lang),
    );
    let fixed = fixed_jis_with_new_keyboard();
    section(
        &mut out,
        "summary (fixed JIS, a new US keyboard: the migration)",
        &Summary::new(&fixed, &[(KEYCHRON_ROW, Layout::Us)]).page(lang),
    );
    let incomplete = PrepareFailure::Incomplete("devnode X: no driver".into());
    section(
        &mut out,
        "summary (the migration could not be prepared)",
        &Summary {
            migration_failed: Some(&incomplete),
            ..Summary::new(&fixed, &[(KEYCHRON_ROW, Layout::Us)])
        }
        .page(lang),
    );
    section(
        &mut out,
        "summary (a cleanup first)",
        &Summary {
            deletes: &[BUILT_IN],
            ..Summary::new(&problems, &[(KEYCHRON_ROW, Layout::Us)])
        }
        .page(lang),
    );
    let waiting = match lang {
        Lang::Ja => "今は変更できません: 確認待ちの変更があります",
        Lang::En => "Cannot change now: a change waits for keep or revert",
    };
    section(
        &mut out,
        "summary (the cleanup waits for keep or undo)",
        &Summary {
            awaiting: Some(AwaitingCleanup {
                op_id: OpId::parse("5d6e7f80-1a2b-4c3d-8e9f-0a1b2c3d4e5f").unwrap(),
                instance_id: BUILT_IN.into(),
            }),
            blocked: Some(waiting),
            ran: true,
            ..Summary::new(&dev, &[(KEYCHRON_ROW, Layout::Jis)])
        }
        .page(lang),
    );
    let restart = match lang {
        Lang::Ja => "今は変更できません: PC の再起動を待っている変更があります",
        Lang::En => "Cannot change now: a change waits for a PC restart",
    };
    section(
        &mut out,
        "summary (the rest is blocked by a restart)",
        &Summary {
            blocked: Some(restart),
            ran: true,
            ..Summary::new(&dev, &[(KEYCHRON_ROW, Layout::Jis)])
        }
        .page(lang),
    );
    let mut done = fixtures::dev_machine();
    done.keyboards[1].overrides.keyboard_type_override = Some(7);
    done.keyboards[1].overrides.keyboard_subtype_override = Some(2);
    done.keyboards[1].reported_type = Some(KeyboardType::JIS);
    section(
        &mut out,
        "summary (done)",
        &Summary {
            ran: true,
            ..Summary::new(&done, &[(KEYCHRON_ROW, Layout::Jis)])
        }
        .page(lang),
    );
    out
}

#[test]
fn the_wizard_in_japanese() {
    common::assert_snapshot("wizard.ja.txt", &scenes(Lang::Ja));
}

#[test]
fn the_wizard_in_english() {
    common::assert_snapshot("wizard.en.txt", &scenes(Lang::En));
}

#[test]
fn the_refused_migration_is_not_sent() {
    // A PC whose global values disagree (fixed JIS type, US layer driver: every keyboard types
    // US) cannot be migrated: the page says so.
    let mut snapshot = fixed_jis_with_new_keyboard();
    snapshot.global.layer_driver_jpn = Some("kbd101.dll".into());
    let rows = rows(
        &snapshot,
        &[(KEYCHRON_ROW, Layout::Jis)],
        &[],
        &[],
        Lang::En,
    );
    let plan = wizard_plan(&snapshot, &plan_rows(&rows), None);
    let WizardPlan::Migrate { assignments, .. } = &plan else {
        panic!("{plan:?}")
    };
    // The PS/2 keyboard's pin is unknown: it is assigned what is chosen for it.
    assert!(assignments.contains(&(BUILT_IN.to_string(), LayoutChoice::Us)));
    let refused: Result<_, OperationError> = plan_migration(
        &snapshot.keyboards,
        &snapshot.global,
        Layout::Jis,
        assignments,
    );
    assert!(refused.is_err());
    let task = next_task(None, false, &[], &plan, &rows);
    let page = summary_page(&SummaryContext {
        rows: Some(&rows),
        plan: &plan,
        cleanups: &[],
        task: &task,
        blocked: None,
        standard_now: &LayoutTable::Us,
        mode: GlobalMode::Fixed,
        fixed: None,
        standard: Layout::Jis,
        migration: MigrationStatus::Planned {
            plan: &refused,
            snapshot: &snapshot,
        },
        ran: false,
        autostart: true,
        uac_notice_seen: true,
        elevated: false,
        lang: Lang::En,
    });
    // Nothing can be sent, so no prompt is announced either.
    assert!(!page.can_next);
    assert!(page.uac_line.is_empty());
    assert!(
        page.note.starts_with("The PC-wide keyboard values"),
        "{}",
        page.note
    );
}
