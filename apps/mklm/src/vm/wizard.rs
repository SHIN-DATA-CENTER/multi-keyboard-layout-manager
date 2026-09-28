//! The first-run wizard (plan 3.1; design m3 B.1; WP-U2), four steps (review U3):
//!
//! 1. ようこそ — what MKLM does, for every user of the PC, with an administrator's permission.
//! 2. 入力方式 — the user's Preload and the sign-in screen's (`HKU\.DEFAULT`), by language name,
//!    and every `mklm_core::input_warnings` warning as a sentence.
//! 3. キーボードの配列 — every connected keyboard (grouped as on the main screen) with JIS / US
//!    (default: the layout it is set to, which is how it types unless a change waits) and the
//!    in-place layout detection; values with a problem are explained on their row, and values its
//!    driver does not read may be deleted (`Request::CleanupValues`, design m3 A.5).
//! 4. まとめ — what will happen: nothing ("変更は不要です"), one `Migrate` that carries the
//!    assignments (fixed mode; "PC の再起動が 1 回必要です", with the PC's standard layout), or one
//!    ordinary change after another through the change page (per-keyboard mode).
//!
//! The migration carries every keyboard whose layout after it would differ from the choice (plan
//! 1.3: "write the assignment in the same transaction"): a new US keyboard on a fixed-JIS PC is
//! US after the one restart, a keyboard whose stored values fixed mode ignores keeps the layout
//! chosen for it, and a keyboard without values keeps it when another standard layout is chosen
//! ([`wizard_plan`]).
//!
//! The summary is recomputed from every read, so a change that was kept, reverted or blocked is
//! seen as it is ([`next_task`]); the requests go one at a time through the session flow of WP-U3.

use mklm_core::{
    GlobalMode, GlobalSettings, InputMethods, Journal, KeyboardAnomaly, KeyboardAssessment,
    KeyboardDevice, KeyboardDriver, Layout, LayoutChoice, LayoutTable, OpId, OpKind, OpState,
    OperationError, OperationPlan, PendingAction, SystemSnapshot, Transport, assess,
    cleanup_candidates, device_apply_action, effective_layout, input_warnings, ps2_pin_layout,
};

use super::change::{ChoiceRow, DetectPanel};
use super::{SnapshotText, Tone};
use crate::detect::{Detection, Verdict};
use crate::i18n::wizard::{self as text, SummaryButton, SummaryState};
use crate::i18n::{self, BadgeKind, Lang};
use crate::settings::Settings;
use crate::state::PrepareFailure;

/// The wizard's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WizardStep {
    #[default]
    Welcome,
    InputMethods,
    Keyboards,
    Summary,
}

impl WizardStep {
    /// "手順 3 / 4" (review U16 e): the position in words, not by colour.
    pub fn number(self) -> (u32, u32) {
        (self.index() as u32 + 1, 4)
    }

    /// The index the window uses (0 = welcome … 3 = summary).
    pub fn index(self) -> usize {
        match self {
            WizardStep::Welcome => 0,
            WizardStep::InputMethods => 1,
            WizardStep::Keyboards => 2,
            WizardStep::Summary => 3,
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            WizardStep::Welcome => Some(WizardStep::InputMethods),
            WizardStep::InputMethods => Some(WizardStep::Keyboards),
            WizardStep::Keyboards => Some(WizardStep::Summary),
            WizardStep::Summary => None,
        }
    }

    pub fn previous(self) -> Option<Self> {
        match self {
            WizardStep::Welcome => None,
            WizardStep::InputMethods => Some(WizardStep::Welcome),
            WizardStep::Keyboards => Some(WizardStep::InputMethods),
            WizardStep::Summary => Some(WizardStep::Keyboards),
        }
    }
}

/// What to do about a value with a problem (plan 3.1 step 2; design m3 B.1, K.4). Only problem
/// values are listed; a value without one needs no decision (MKLM records the value before its
/// first change as the baseline, design m2 C.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProblemChoice {
    /// Leave it (recommended).
    #[default]
    Keep,
    /// Remove the values its driver ignores: `Request::CleanupValues` (design m3 A.5, WP-E1).
    /// Only offered for such values.
    Delete,
}

/// Override values on a non-keyboard collection of a keyboard's device (read on the I/O worker
/// with `mklm_win::read_non_keyboard_values`; design m3 B.1, J.9). MKLM never writes them (plan
/// 1.5); the wizard only explains how to remove them by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignValues {
    /// The collection's instance ID.
    pub instance_id: String,
    /// The keyboard (instance ID) whose device it belongs to.
    pub keyboard: String,
    /// The value names found.
    pub names: Vec<String>,
}

/// One connected keyboard on step 3.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WizardKeyboard {
    /// The row ID (as the main list: the container, or the instance ID of a keyboard without one).
    pub id: String,
    /// The instance IDs of the device's keyboards.
    pub members: Vec<String>,
    /// The keyboard a request names (the first connected member).
    pub target: String,
    pub name: String,
    /// "JIS として動作中".
    pub current: String,
    /// JIS / US can be chosen (not a read-only keyboard).
    pub can_choose: bool,
    /// The layout it is set to: the default choice.
    pub default: Option<Layout>,
    /// What the row shows as chosen: the user's choice, else the default.
    pub choice: Option<Layout>,
    /// "レシーバー: …", "読み取り専用: …".
    pub note: String,
    /// The problems of its values, in words; empty when none.
    pub problem: String,
    /// "削除する" is offered: some member has values its driver does not read (and MKLM may
    /// delete, `mklm_core::cleanup_candidates`).
    pub problem_deletable: bool,
    pub problem_choice: ProblemChoice,
    /// The technical details of the problems (value names, the manual steps).
    pub details: Vec<String>,
}

/// What the keyboards of step 3 are made from.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardsInput<'a> {
    pub snapshot: &'a SystemSnapshot,
    pub settings: &'a Settings,
    /// The user's JIS / US per row ID.
    pub choices: &'a [(String, Layout)],
    /// Rows (IDs) whose unread values the user chose to delete.
    pub deletes: &'a [String],
    /// Values on non-keyboard collections (empty until read).
    pub foreign: &'a [ForeignValues],
    pub lang: Lang,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The connected keyboards, one row per physical device, in snapshot order (as the main list,
/// design m3 B.2): hidden devices and devices without a connected keyboard are left out.
pub fn wizard_keyboards(input: &KeyboardsInput<'_>) -> Vec<WizardKeyboard> {
    let lang = input.lang;
    let snapshot = input.snapshot;
    let assessment = assess(snapshot);
    let mut rows = Vec::new();
    for group in &assessment.groups {
        let members: Vec<(&KeyboardDevice, &KeyboardAssessment)> = group
            .keyboards
            .iter()
            .filter_map(|id| {
                let kb = snapshot.keyboards.iter().find(|kb| &kb.instance_id == id)?;
                let ka = assessment
                    .keyboards
                    .iter()
                    .find(|ka| &ka.instance_id == id)?;
                Some((kb, ka))
            })
            .collect();
        let Some(&(first_kb, _)) = members.first() else {
            continue;
        };
        // The row ID of `vm::keyboards::keyboard_rows`.
        let id = match (&group.container_id, group.is_internal) {
            (Some(container), false) if first_kb.known_container_id().is_some() => {
                container.clone()
            }
            _ => first_kb.instance_id.clone(),
        };
        let Some(&(kb, ka)) = members.iter().find(|(kb, _)| kb.present) else {
            continue;
        };
        if input.settings.is_hidden(&id) {
            continue;
        }
        let read_only =
            kb.transport == Transport::Virtual || matches!(kb.driver, KeyboardDriver::Other(_));
        let default = ka
            .after_restart
            .as_ref()
            .and_then(|layout| layout.table.layout())
            .or_else(|| ka.current.as_ref().and_then(|layout| layout.table.layout()));
        let chosen = input
            .choices
            .iter()
            .find(|(row, _)| same(row, &id))
            .map(|(_, layout)| *layout);
        let mut notes = Vec::new();
        if crate::vm::keyboards::is_receiver(kb) {
            notes.push(i18n::badge(BadgeKind::Receiver, lang).1);
        }
        if read_only {
            notes.push(i18n::badge(BadgeKind::ReadOnly, lang).1);
        }
        let deletable = !read_only
            && members
                .iter()
                .any(|(kb, _)| !cleanup_candidates(kb).is_empty());
        let mut problems: Vec<String> = Vec::new();
        let mut details: Vec<String> = Vec::new();
        for (member, member_ka) in &members {
            for anomaly in &member_ka.anomalies {
                let line = text::problem(anomaly, deletable, lang);
                if !problems.contains(&line) {
                    problems.push(line);
                }
                if let Some(names) = anomaly_names(anomaly) {
                    details.push(text::problem_values(&member.instance_id, &names, lang));
                }
            }
        }
        for foreign in input.foreign.iter().filter(|foreign| {
            members
                .iter()
                .any(|(kb, _)| same(&kb.instance_id, &foreign.keyboard))
        }) {
            let line = text::foreign_values(lang);
            if !problems.contains(&line) {
                problems.push(line);
            }
            details.extend(text::foreign_steps(
                &foreign.instance_id,
                &foreign.names,
                lang,
            ));
        }
        let problem_choice = if deletable && input.deletes.iter().any(|row| same(row, &id)) {
            ProblemChoice::Delete
        } else {
            ProblemChoice::Keep
        };
        rows.push(WizardKeyboard {
            members: members
                .iter()
                .map(|(kb, _)| kb.instance_id.clone())
                .collect(),
            target: kb.instance_id.clone(),
            name: group.display_name.clone(),
            current: text::types_now(ka.current.as_ref().map(|layout| &layout.table), lang),
            can_choose: !read_only,
            default,
            choice: if read_only { None } else { chosen.or(default) },
            note: i18n::summary_join(&notes, lang),
            problem: problems.join("\n"),
            problem_deletable: deletable,
            problem_choice,
            details,
            id,
        });
    }
    rows
}

/// The value names an anomaly is about (for the technical details).
fn anomaly_names(anomaly: &KeyboardAnomaly) -> Option<Vec<String>> {
    match anomaly {
        KeyboardAnomaly::IncompletePair { present, .. } => Some(vec![present.clone()]),
        KeyboardAnomaly::ForeignValueNames { names }
        | KeyboardAnomaly::ValuesOnUnsupportedDriver { names }
        | KeyboardAnomaly::ValuesOnNonKeyboard { names } => Some(names.clone()),
        KeyboardAnomaly::UnverifiedType { keyboard_type }
        | KeyboardAnomaly::UnexpectedType { keyboard_type }
        | KeyboardAnomaly::IgnoredInFixedMode { keyboard_type } => {
            Some(vec![keyboard_type.to_string()])
        }
    }
}

/// One row as [`wizard_plan`] needs it: the keyboard a request names, the device's keyboards and
/// the layout chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub target: String,
    pub members: Vec<String>,
    pub chosen: Layout,
}

/// The rows that take part in the plan: every choosable row with a layout.
pub fn plan_rows(rows: &[WizardKeyboard]) -> Vec<PlanRow> {
    rows.iter()
        .filter(|row| row.can_choose)
        .filter_map(|row| {
            Some(PlanRow {
                target: row.target.clone(),
                members: row.members.clone(),
                chosen: row.choice?,
            })
        })
        .collect()
}

/// What step 4 proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardPlan {
    /// Every choice matches what the keyboards are set to.
    NothingToDo,
    /// Fixed mode and some keyboard types differently from its choice: one migration with the
    /// PC's standard layout and the assignments of the keyboards that would differ after it.
    Migrate {
        standard: Layout,
        assignments: Vec<(String, LayoutChoice)>,
    },
    /// Per-keyboard mode: one `SetLayout` after another (instance ID, layout), the ones that can
    /// be tried at once first and the ones that need the PC restart last (a restart blocks every
    /// later change until it happens).
    SetLayouts(Vec<(String, LayoutChoice)>),
}

fn choice(layout: Layout) -> LayoutChoice {
    match layout {
        Layout::Jis => LayoutChoice::Jis,
        Layout::Us => LayoutChoice::Us,
    }
}

/// The layout table a keyboard gets after the migration to `standard` without an assignment
/// (design m2 D.3, `mklm_core::migration_writes`): an i8042prt keyboard is pinned to the layout
/// fixed mode gives it now; a kbdhid keyboard follows its own stored values, or the new standard
/// without them. `None`: not known (other drivers, inconsistent global values).
pub fn after_migration(
    kb: &KeyboardDevice,
    global: &GlobalSettings,
    standard: Layout,
) -> Option<LayoutTable> {
    match kb.driver {
        KeyboardDriver::I8042prt => ps2_pin_layout(global).map(LayoutTable::from),
        KeyboardDriver::Kbdhid => {
            let after = GlobalSettings {
                override_keyboard_type: None,
                override_keyboard_subtype: None,
                layer_driver_jpn: Some(standard.layer_driver().to_string()),
                ..global.clone()
            };
            let standard = LayoutTable::from(standard);
            kb.predicted_type(&after)
                .map(|ty| effective_layout(GlobalMode::PerKeyboard, &standard, ty).table)
        }
        KeyboardDriver::Other(_) => None,
    }
}

/// The step 4 plan from the step 3 choices. In fixed mode `standard` is the PC's standard layout
/// of the migration (`None`: the layout fixed mode gives now, the recommendation); in
/// per-keyboard mode it is not changed (the migration alone sets it, design m3 B.1).
pub fn wizard_plan(
    snapshot: &SystemSnapshot,
    rows: &[PlanRow],
    standard: Option<Layout>,
) -> WizardPlan {
    let assessment = assess(snapshot);
    let assessed = |id: &str| {
        assessment
            .keyboards
            .iter()
            .find(|ka| same(&ka.instance_id, id))
    };
    let device = |id: &str| {
        snapshot
            .keyboards
            .iter()
            .find(|kb| same(&kb.instance_id, id))
    };
    match snapshot.global.mode() {
        GlobalMode::PerKeyboard => {
            // Differs: some keyboard of the device is set to another layout.
            let mut changes: Vec<(PendingAction, String, LayoutChoice)> = rows
                .iter()
                .filter(|row| {
                    let wanted = LayoutTable::from(row.chosen);
                    row.members.iter().filter_map(|id| assessed(id)).any(|ka| {
                        ka.after_restart
                            .as_ref()
                            .is_none_or(|layout| layout.table != wanted)
                    })
                })
                .map(|row| {
                    let action = row
                        .members
                        .iter()
                        .filter_map(|id| device(id))
                        .map(device_apply_action)
                        .max()
                        .unwrap_or(PendingAction::RestartPc);
                    (action, row.target.clone(), choice(row.chosen))
                })
                .collect();
            if changes.is_empty() {
                return WizardPlan::NothingToDo;
            }
            changes.sort_by_key(|(action, _, _)| *action);
            WizardPlan::SetLayouts(
                changes
                    .into_iter()
                    .map(|(_, target, choice)| (target, choice))
                    .collect(),
            )
        }
        GlobalMode::Fixed => {
            // Proposed only when a layout different from the global one is chosen (plan 1.3).
            let differs = rows.iter().any(|row| {
                assessed(&row.target)
                    .and_then(|ka| ka.current.as_ref().or(ka.after_restart.as_ref()))
                    .is_none_or(|layout| layout.table != LayoutTable::from(row.chosen))
            });
            if !differs {
                return WizardPlan::NothingToDo;
            }
            let standard = standard
                .or_else(|| ps2_pin_layout(&snapshot.global))
                .unwrap_or(Layout::Jis);
            let wanted = |row: &PlanRow| Some(LayoutTable::from(row.chosen));
            let assignments =
                rows.iter()
                    .filter(|row| {
                        row.members.iter().filter_map(|id| device(id)).any(|kb| {
                            after_migration(kb, &snapshot.global, standard) != wanted(row)
                        })
                    })
                    .map(|row| (row.target.clone(), choice(row.chosen)))
                    .collect();
            WizardPlan::Migrate {
                standard,
                assignments,
            }
        }
    }
}

/// "削除する" for one keyboard (`Request::CleanupValues`): the values its driver does not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupTask {
    pub row: String,
    pub name: String,
    pub instance_id: String,
    pub names: Vec<String>,
}

/// The cleanups the choices ask for and that are still to be done: per row with "削除する", each
/// member with values its driver does not read.
pub fn cleanup_tasks(
    snapshot: &SystemSnapshot,
    rows: &[WizardKeyboard],
    deletes: &[String],
) -> Vec<CleanupTask> {
    let mut tasks = Vec::new();
    for row in rows.iter().filter(|row| {
        row.problem_deletable
            && row.problem_choice == ProblemChoice::Delete
            && deletes.iter().any(|id| same(id, &row.id))
    }) {
        for kb in snapshot
            .keyboards
            .iter()
            .filter(|kb| row.members.iter().any(|id| same(id, &kb.instance_id)))
        {
            let names = cleanup_candidates(kb);
            if !names.is_empty() {
                tasks.push(CleanupTask {
                    row: row.id.clone(),
                    name: row.name.clone(),
                    instance_id: kb.instance_id.clone(),
                    names: names.into_iter().map(str::to_string).collect(),
                });
            }
        }
    }
    tasks
}

/// A cleanup that waits for keep or revert (`AwaitingConfirm` without a countdown, design m3 A.5):
/// it blocks every new change until the user decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitingCleanup {
    pub op_id: OpId,
    pub instance_id: String,
}

/// The oldest cleanup that waits for the user, if any.
pub fn awaiting_cleanup(journal: &Journal) -> Option<AwaitingCleanup> {
    journal.entries.iter().find_map(|entry| match &entry.kind {
        OpKind::Cleanup { instance_id, .. }
            if entry.state == OpState::AwaitingConfirm && entry.countdown.is_none() =>
        {
            Some(AwaitingCleanup {
                op_id: entry.op_id.clone(),
                instance_id: instance_id.clone(),
            })
        }
        _ => None,
    })
}

/// What the summary's button does next (design m3 B.1 step 4, one request at a time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardTask {
    /// Keep ("このままにする", `Request::Confirm`) or undo ("元に戻す", `Request::Revert`) the
    /// cleanup that waits.
    KeepCleanup(AwaitingCleanup),
    /// `Request::CleanupValues` (before the layout changes: a restart would block it).
    Cleanup(CleanupTask),
    /// `Request::Migrate` with the assignments.
    Migrate {
        standard: Layout,
        assignments: Vec<(String, LayoutChoice)>,
    },
    /// The change page of this row with this layout chosen (an ordinary change, WP-U3).
    SetLayout {
        row: String,
        name: String,
        choice: LayoutChoice,
    },
    /// Nothing is left, or nothing can be done now: finish the wizard.
    Finish,
}

/// The next task: an open cleanup's decision first, then — unless the journal blocks new changes
/// — the cleanups, then the layouts.
pub fn next_task(
    awaiting: Option<AwaitingCleanup>,
    blocked: bool,
    cleanups: &[CleanupTask],
    plan: &WizardPlan,
    rows: &[WizardKeyboard],
) -> WizardTask {
    if let Some(awaiting) = awaiting {
        return WizardTask::KeepCleanup(awaiting);
    }
    if blocked {
        return WizardTask::Finish;
    }
    if let Some(cleanup) = cleanups.first() {
        return WizardTask::Cleanup(cleanup.clone());
    }
    match plan {
        WizardPlan::NothingToDo => WizardTask::Finish,
        WizardPlan::Migrate {
            standard,
            assignments,
        } => WizardTask::Migrate {
            standard: *standard,
            assignments: assignments.clone(),
        },
        WizardPlan::SetLayouts(changes) => changes
            .iter()
            .find_map(|(target, choice)| {
                let row = rows.iter().find(|row| same(&row.target, target))?;
                Some(WizardTask::SetLayout {
                    row: row.id.clone(),
                    name: row.name.clone(),
                    choice: *choice,
                })
            })
            .unwrap_or(WizardTask::Finish),
    }
}

/// The page, whatever the step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WizardPage {
    pub step: WizardStep,
    /// "手順 3 / 4: キーボードの配列".
    pub heading: String,
    pub body: String,
    /// Facts and planned changes, one per line.
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
    /// Information banners (the sign-in steps, the one-by-one note, the IME note …).
    pub notes: Vec<String>,
    /// Step 2: the buttons that open the language and keyboard settings.
    pub settings_buttons: bool,
    pub keyboards: Vec<WizardKeyboard>,
    /// Step 3: the in-place detection.
    pub detect: DetectPanel,
    /// Step 4 of a migration: the PC's standard layout ("標準配列（おすすめ: 今の JIS）").
    pub standard_visible: bool,
    pub standard_choices: Vec<ChoiceRow>,
    pub standard_selected: Option<usize>,
    /// Step 4 of a migration: the write order and the ways to sign in after the restart.
    pub migration: bool,
    /// Step 4 when finishing: "サインイン時に MKLM を…で起動する" (default on, design m3 F.3).
    pub autostart_visible: bool,
    pub autostart: bool,
    /// "確認しています…".
    pub busy: bool,
    /// Why the button cannot be used, or what went wrong (with its tone).
    pub note: String,
    pub note_tone: Tone,
    /// "次に Windows の確認画面が出ます…" (from the second prompt on, review U14).
    pub uac_line: String,
    /// Technical details, one per line.
    pub details: Vec<String>,
    pub back_visible: bool,
    /// "元に戻す" next to the primary button (an open cleanup); empty when none.
    pub secondary_text: String,
    /// The primary button; empty is "次へ" (the window's own label).
    pub next_text: String,
    pub can_next: bool,
}

/// Step 1.
pub fn welcome_page(lang: Lang) -> WizardPage {
    WizardPage {
        step: WizardStep::Welcome,
        heading: text::step_heading(WizardStep::Welcome, lang),
        body: text::welcome_body(lang),
        lines: text::welcome_points(lang),
        can_next: true,
        ..WizardPage::default()
    }
}

/// Step 2: the input methods by language name, the warnings, and — when the sign-in screen does
/// not type Japanese — how to copy the user's input methods to it.
pub fn input_methods_page(input: &InputMethods, lang: Lang) -> WizardPage {
    let warnings = input_warnings(input);
    let mut page = WizardPage {
        step: WizardStep::InputMethods,
        heading: text::step_heading(WizardStep::InputMethods, lang),
        body: text::input_body(lang),
        lines: vec![
            text::user_methods(&input.user_preload, lang),
            text::sign_in_methods(&input.sign_in_preload, lang),
        ],
        warnings: warnings
            .iter()
            .map(|warning| text::input_warning(warning, lang))
            .collect(),
        settings_buttons: true,
        back_visible: true,
        can_next: true,
        ..WizardPage::default()
    };
    if warnings.is_empty() {
        page.notes.push(text::input_fine(lang));
    }
    if warnings
        .iter()
        .any(|warning| matches!(warning, mklm_core::InputWarning::SignInNotJapanese { .. }))
    {
        page.notes.push(text::sign_in_copy_steps(lang));
    }
    let hkls: Vec<String> = input
        .loaded_layouts
        .iter()
        .map(|hkl| format!("{hkl:08X}"))
        .collect();
    page.details = vec![
        format!("HKCU Preload: {}", input.user_preload.join(", ")),
        format!(
            "HKU\\.DEFAULT Preload: {}",
            input.sign_in_preload.join(", ")
        ),
        format!("GetKeyboardLayoutList: {}", hkls.join(", ")),
    ];
    page
}

/// What step 3 shows besides the rows.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardsContext<'a> {
    /// `None` until the first read.
    pub rows: Option<&'a [WizardKeyboard]>,
    pub detection: &'a Detection,
    /// The display name of the keyboard the detection is about.
    pub detected_name: Option<&'a str>,
    pub lang: Lang,
}

/// Step 3.
pub fn keyboards_page(ctx: &KeyboardsContext<'_>) -> WizardPage {
    let lang = ctx.lang;
    let mut page = WizardPage {
        step: WizardStep::Keyboards,
        heading: text::step_heading(WizardStep::Keyboards, lang),
        body: text::keyboards_body(lang),
        back_visible: true,
        can_next: true,
        ..WizardPage::default()
    };
    let Some(rows) = ctx.rows else {
        page.busy = true;
        page.note = text::keyboards_reading(lang);
        page.note_tone = Tone::Info;
        return page;
    };
    if rows.is_empty() {
        page.note = text::keyboards_none(lang);
        page.note_tone = Tone::Info;
    }
    page.keyboards = rows.to_vec();
    page.details = rows
        .iter()
        .flat_map(|row| row.details.iter().cloned())
        .collect();
    let detection = ctx.detection;
    let mut detect = DetectPanel {
        instruction: text::detect_instruction(detection.step, lang),
        ..DetectPanel::default()
    };
    if detection.jis_hint && detection.verdict().is_none() {
        detect.feedback = i18n::detect_jis_hint(lang);
        detect.feedback_tone = Tone::Info;
    }
    if let (Some(verdict), Some(name)) = (detection.verdict(), ctx.detected_name) {
        detect.verdict = i18n::detect_verdict(verdict, name, lang);
        let layout = match verdict {
            Verdict::Jis => Some(Layout::Jis),
            Verdict::Us => Some(Layout::Us),
            Verdict::Mixed => None,
        };
        // One click selects it on the keyboard's row (design m3 B.3), unless it is selected.
        let row = detection
            .device
            .as_deref()
            .and_then(|device| detected_row(rows, device));
        if let (Some(layout), Some(row)) = (layout, row)
            && row.can_choose
            && row.choice != Some(layout)
        {
            detect.choose_text = i18n::detect_choose(layout, lang);
        }
    }
    page.detect = detect;
    page
}

/// The row of the keyboard (instance ID) the detection found.
pub fn detected_row<'a>(rows: &'a [WizardKeyboard], device: &str) -> Option<&'a WizardKeyboard> {
    rows.iter()
        .find(|row| row.members.iter().any(|id| same(id, device)))
}

/// Where the migration's preparation stands (design m3 B.5: the planning inventory, the journal
/// and the gate, read on the I/O worker).
#[derive(Debug, Clone, Copy)]
pub enum MigrationStatus<'a> {
    /// No migration is the next task.
    None,
    Preparing,
    Failed(&'a PrepareFailure),
    /// The journal refuses new changes (`gate::blocker`), in words.
    Blocked(&'a str),
    /// Planned from the planning inventory (what is shown is what is sent, design m2 S6).
    Planned {
        plan: &'a Result<OperationPlan, OperationError>,
        snapshot: &'a SystemSnapshot,
    },
}

/// What step 4 shows.
#[derive(Debug, Clone, Copy)]
pub struct SummaryContext<'a> {
    /// `None` until the first read.
    pub rows: Option<&'a [WizardKeyboard]>,
    pub plan: &'a WizardPlan,
    pub cleanups: &'a [CleanupTask],
    pub task: &'a WizardTask,
    /// Why new changes are blocked now (`vm::status::blocked_reason`), if they are.
    pub blocked: Option<&'a str>,
    /// The PC's standard layout now, and the layout fixed mode gives (the recommended standard of
    /// a migration).
    pub standard_now: &'a LayoutTable,
    pub mode: GlobalMode,
    pub fixed: Option<Layout>,
    /// The standard layout the migration uses (the user's, else the recommendation).
    pub standard: Layout,
    pub migration: MigrationStatus<'a>,
    /// The wizard has run a request already.
    pub ran: bool,
    pub autostart: bool,
    /// `settings.change.uac_notice_seen`: the prompt is explained in one line (review U14).
    pub uac_notice_seen: bool,
    /// The GUI runs elevated: no prompt follows (design m3 B.5).
    pub elevated: bool,
    pub lang: Lang,
}

/// The name of the keyboard (instance ID) among the rows, else the ID.
fn row_name(rows: &[WizardKeyboard], id: &str) -> String {
    rows.iter()
        .find(|row| row.members.iter().any(|member| same(member, id)))
        .map_or_else(|| id.to_string(), |row| row.name.clone())
}

/// Step 4.
pub fn summary_page(ctx: &SummaryContext<'_>) -> WizardPage {
    let lang = ctx.lang;
    let prompt = !ctx.elevated;
    let mut page = WizardPage {
        step: WizardStep::Summary,
        heading: text::step_heading(WizardStep::Summary, lang),
        back_visible: true,
        ..WizardPage::default()
    };
    let Some(rows) = ctx.rows else {
        page.busy = true;
        page.note = text::keyboards_reading(lang);
        page.note_tone = Tone::Info;
        return page;
    };
    // What is left, in the order it is done.
    let mut left: Vec<String> = ctx
        .cleanups
        .iter()
        .map(|cleanup| text::cleanup_line(&cleanup.name, lang))
        .collect();
    match ctx.plan {
        WizardPlan::NothingToDo => {}
        WizardPlan::Migrate { assignments, .. } => {
            left.push(text::migration_line(ctx.standard, lang));
            for (target, choice) in assignments {
                if let Some(layout) = choice.layout() {
                    left.push(text::assignment_line(&row_name(rows, target), layout, lang));
                }
            }
        }
        WizardPlan::SetLayouts(changes) => {
            for (target, choice) in changes {
                if let Some(layout) = choice.layout() {
                    left.push(text::change_line(&row_name(rows, target), layout, lang));
                }
            }
        }
    }
    let state = match (left.is_empty(), ctx.blocked.is_some(), ctx.ran) {
        (false, true, _) => SummaryState::Blocked,
        (false, false, _) => SummaryState::Changes,
        (true, _, true) => SummaryState::Done,
        (true, _, false) => SummaryState::NothingToDo,
    };
    page.body = text::summary_body(state, lang);
    page.lines = left;
    if ctx.mode == GlobalMode::PerKeyboard {
        page.notes
            .push(text::standard_unchanged(ctx.standard_now, lang));
    }
    if matches!(ctx.plan, WizardPlan::SetLayouts(_)) {
        page.notes.push(text::one_by_one_note(lang));
    }
    if rows.iter().any(|row| row.choice == Some(Layout::Us)) {
        page.notes.push(i18n::ime_note_us(lang));
    }
    // The primary button.
    page.can_next = true;
    let starts_request = match ctx.task {
        WizardTask::Finish => {
            page.next_text = text::summary_button(SummaryButton::Finish, prompt, lang);
            page.autostart_visible = true;
            page.autostart = ctx.autostart;
            if let Some(reason) = ctx.blocked {
                page.notes.push(if page.lines.is_empty() {
                    reason.to_string()
                } else {
                    text::blocked_rest(reason, lang)
                });
            }
            false
        }
        WizardTask::KeepCleanup(awaiting) => {
            page.body = text::awaiting_cleanup(&row_name(rows, &awaiting.instance_id), lang);
            page.next_text = text::summary_button(SummaryButton::KeepCleanup, prompt, lang);
            page.secondary_text = text::revert_cleanup(lang);
            true
        }
        WizardTask::Cleanup(_) => {
            page.next_text = text::summary_button(SummaryButton::Cleanup, prompt, lang);
            true
        }
        WizardTask::SetLayout { name, .. } => {
            page.next_text = text::summary_button(SummaryButton::OpenChange(name), prompt, lang);
            // The change page explains its own prompt.
            false
        }
        WizardTask::Migrate { .. } => {
            page.next_text = text::summary_button(SummaryButton::Migrate, prompt, lang);
            migration_parts(ctx, &mut page);
            true
        }
    };
    if starts_request {
        page.notes.push(i18n::all_users_note(lang));
        if prompt && ctx.uac_notice_seen && page.can_next {
            page.uac_line = i18n::uac_line(lang);
        }
    }
    page
}

/// The parts of step 4 that belong to the migration (design m3 B.1, B.4): the standard layout,
/// the write order, the Settings app's option, the ways to sign in after the restart (the window's
/// own text), the value lines under the details, and whether it can be sent.
fn migration_parts(ctx: &SummaryContext<'_>, page: &mut WizardPage) {
    let lang = ctx.lang;
    page.migration = true;
    page.standard_visible = true;
    page.standard_choices = [Layout::Jis, Layout::Us]
        .into_iter()
        .map(|layout| ChoiceRow {
            text: i18n::standard_choice(layout, ctx.fixed == Some(layout), lang),
            current: ctx.fixed == Some(layout),
            enabled: true,
            ..ChoiceRow::default()
        })
        .collect();
    page.standard_selected = Some(match ctx.standard {
        Layout::Jis => 0,
        Layout::Us => 1,
    });
    page.notes.push(text::write_order(lang));
    page.warnings.push(text::connected_layout_warning(lang));
    match ctx.migration {
        MigrationStatus::None | MigrationStatus::Preparing => {
            page.busy = true;
            page.can_next = false;
        }
        MigrationStatus::Failed(failure) => {
            page.note = i18n::prepare_failed(
                matches!(failure, PrepareFailure::Incomplete(_)),
                i18n::PreparePlace::Wizard,
                lang,
            );
            page.note_tone = Tone::Warning;
            page.details.push(failure.diagnostic().to_string());
            page.can_next = false;
        }
        MigrationStatus::Blocked(reason) => {
            page.note = reason.to_string();
            page.note_tone = Tone::Warning;
            page.can_next = false;
        }
        MigrationStatus::Planned { plan, snapshot } => match plan {
            Ok(plan) => {
                page.details.extend(
                    crate::vm::change::plan_lines(snapshot, plan, lang)
                        .into_iter()
                        .map(|line| {
                            format!("{} {}: {} → {}", line.key, line.name, line.now, line.after)
                        }),
                );
            }
            Err(error) => {
                page.note = i18n::operation_refused(error, lang);
                page.note_tone = Tone::Warning;
                page.details.push(error.to_string());
                page.can_next = false;
            }
        },
    }
}

impl SnapshotText for WizardPage {
    fn snapshot_text(&self) -> String {
        let mut out = format!("heading: {}\n", self.heading);
        if !self.body.is_empty() {
            out.push_str(&format!("body: {}\n", self.body));
        }
        for line in &self.lines {
            out.push_str(&format!("item: {line}\n"));
        }
        for warning in &self.warnings {
            out.push_str(&format!("warning: {warning}\n"));
        }
        for note in &self.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        for row in &self.keyboards {
            let choice = match (row.can_choose, row.choice) {
                (false, _) => "—",
                (true, Some(Layout::Jis)) => "JIS",
                (true, Some(Layout::Us)) => "US",
                (true, None) => "?",
            };
            out.push_str(&format!(
                "keyboard: {} [{choice}] {}\n",
                row.name, row.current
            ));
            if !row.note.is_empty() {
                out.push_str(&format!("keyboard note: {}\n", row.note));
            }
            for problem in row.problem.lines() {
                out.push_str(&format!("problem: {problem}\n"));
            }
            if row.problem_deletable {
                out.push_str(&format!(
                    "problem choice: {}\n",
                    match row.problem_choice {
                        ProblemChoice::Keep => "keep",
                        ProblemChoice::Delete => "delete",
                    }
                ));
            }
        }
        for (label, text) in [
            ("detect", &self.detect.instruction),
            ("detect feedback", &self.detect.feedback),
            ("detect verdict", &self.detect.verdict),
            ("detect choose", &self.detect.choose_text),
        ] {
            if !text.is_empty() {
                out.push_str(&format!("{label}: {text}\n"));
            }
        }
        for (index, choice) in self.standard_choices.iter().enumerate() {
            let mark = if Some(index) == self.standard_selected {
                "◉"
            } else {
                "○"
            };
            out.push_str(&format!("standard: {mark} {}\n", choice.text));
        }
        if self.migration {
            out.push_str("migration: sign-in help\n");
        }
        if self.busy {
            out.push_str("busy\n");
        }
        if !self.note.is_empty() {
            out.push_str(&format!("note ({:?}): {}\n", self.note_tone, self.note));
        }
        if self.autostart_visible {
            out.push_str(&format!("autostart: {}\n", self.autostart));
        }
        if !self.uac_line.is_empty() {
            out.push_str(&format!("uac: {}\n", self.uac_line));
        }
        for line in &self.details {
            out.push_str(&format!("details: {line}\n"));
        }
        if !self.secondary_text.is_empty() {
            out.push_str(&format!("secondary: {}\n", self.secondary_text));
        }
        out.push_str(&format!(
            "button: {}\ncan next: {}\n",
            if self.next_text.is_empty() {
                "(Next)"
            } else {
                &self.next_text
            },
            self.can_next
        ));
        out
    }
}

#[cfg(test)]
mod tests;
