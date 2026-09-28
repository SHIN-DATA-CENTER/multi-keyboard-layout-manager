//! The first-run wizard's flow (design m3 B.1, B.18; WP-U2). Pure like the rest of `state`: the
//! wizard's own state is [`WizardState`] (in `AppState::wizard` while the wizard is open), its
//! messages [`WizardMsg`]; everything else goes through the common [`Effect`]s.
//!
//! The wizard opens at start while `settings.wizard.completed` is false (also with `--tray`) and
//! from Settings. "後でセットアップする" closes it and marks it completed; so does "完了", which
//! also applies the sign-in start switch of the last step (on by default, design m3 F.3).
//!
//! Step 4 runs the plan one request at a time, each through the session flow of WP-U3 (progress,
//! countdown, result, "今の状態"), and is recomputed from every read ([`summary`]): an open
//! cleanup waits for keep or revert first; then — unless the journal blocks new changes — the
//! cleanups, then the migration (prepared on the I/O worker and planned from the planning
//! inventory, so that what is shown is what is sent, design m2 S6) or the ordinary changes, each
//! on the change page with its layout chosen, which returns here afterwards. Every request starts
//! from a button; the first one passes the standalone UAC explanation (design m3 B.5).

use mklm_client::gate::Gate;
use mklm_core::{
    ApplyOptions, Layout, LayoutChoice, OperationError, OperationPlan, SystemSnapshot,
    plan_migration,
};
use mklm_ipc::{Assignment, MigrateRequest, Request};

use super::{
    AppMsg, AppState, Effect, OverlayKind, Page, PrepareFailure, PreparedChange, SessionPhase,
    SessionTarget, display_name, stop_identifying, typed_before, update,
};
use crate::autostart::AutostartTask;
use crate::detect::{Detection, Press, Verdict};
use crate::i18n::Lang;
use crate::settings::PhysicalKind;
use crate::vm::keytest::KeyTest;
use crate::vm::wizard::{
    CleanupTask, ForeignValues, KeyboardsContext, KeyboardsInput, MigrationStatus, SummaryContext,
    WizardKeyboard, WizardPage, WizardPlan, WizardStep, WizardTask, awaiting_cleanup,
    cleanup_tasks, detected_row, input_methods_page, keyboards_page, next_task, plan_rows,
    summary_page, welcome_page, wizard_keyboards, wizard_plan,
};

/// A request from the wizard page that waits behind the first-time UAC explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingRequest {
    pub request: Request,
    pub apply: ApplyOptions,
    pub targets: Vec<SessionTarget>,
}

/// What the wizard keeps while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardState {
    pub step: WizardStep,
    /// The user's JIS / US per row ID; other rows keep the layout they are set to.
    pub choices: Vec<(String, Layout)>,
    /// Rows (IDs) whose values their driver does not read are to be deleted.
    pub deletes: Vec<String>,
    /// Fixed mode: the migration's standard layout; `None` keeps the one fixed mode gives.
    pub standard: Option<Layout>,
    /// The in-place detection of step 3: the first answer key fixes the keyboard (design m3 B.3).
    pub detection: Detection,
    /// Values on non-keyboard collections; `None` until read (once, when step 3 opens).
    pub foreign: Option<Vec<ForeignValues>>,
    pub reading_foreign: bool,
    /// The migration's preparation on the I/O worker (token), and what it read.
    pub preparing: Option<u64>,
    pub prepared: Option<PreparedChange>,
    pub failure: Option<PrepareFailure>,
    /// "サインイン時に MKLM を…で起動する" of the last step.
    pub autostart: bool,
    /// A request of the wizard ran (the summary then says what is left, or that it is done).
    pub ran: bool,
    pub waiting: Option<WaitingRequest>,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            step: WizardStep::Welcome,
            choices: Vec::new(),
            deletes: Vec::new(),
            standard: None,
            detection: Detection::new(),
            foreign: None,
            reading_foreign: false,
            preparing: None,
            prepared: None,
            failure: None,
            autostart: true,
            ran: false,
            waiting: None,
        }
    }
}

/// Something that happened in the wizard.
#[derive(Debug, Clone, PartialEq)]
pub enum WizardMsg {
    /// Settings' "初回セットアップをもう一度行う".
    Open,
    /// "次へ", and the summary's primary button.
    Next,
    Back,
    /// "後でセットアップする".
    Skip,
    /// "元に戻す" of a cleanup that waits.
    Secondary,
    /// A row's layout (0 = JIS, 1 = US).
    Choose {
        row: String,
        index: usize,
    },
    /// A row's problem values: keep, or delete.
    Problem {
        row: String,
        delete: bool,
    },
    /// The migration's standard layout (0 = JIS, 1 = US).
    Standard(usize),
    /// The sign-in start switch of the last step.
    Autostart(bool),
    /// "やり直す" of the detection.
    RestartDetection,
    /// Select the detected layout on its keyboard's row.
    UseDetected,
    /// The I/O worker read the values of the non-keyboard collections (English error).
    ForeignRead(Result<Vec<ForeignValues>, String>),
}

fn lang(state: &AppState) -> Lang {
    state.lang.unwrap_or(Lang::Ja)
}

/// No session and no overlay: the wizard's buttons may act.
fn idle(state: &AppState) -> bool {
    state.session == SessionPhase::Idle && state.overlay == OverlayKind::None
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The rows of step 3 from the last read; `None` without a snapshot.
pub fn rows(state: &AppState) -> Option<Vec<WizardKeyboard>> {
    let wizard = state.wizard.as_ref()?;
    let snapshot = state.read.as_ref()?.snapshot.as_ref()?;
    Some(wizard_keyboards(&KeyboardsInput {
        snapshot,
        settings: &state.settings,
        choices: &wizard.choices,
        deletes: &wizard.deletes,
        foreign: wizard.foreign.as_deref().unwrap_or(&[]),
        lang: lang(state),
    }))
}

/// Everything step 4 decides on, from the last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub rows: Vec<WizardKeyboard>,
    pub plan: WizardPlan,
    pub cleanups: Vec<CleanupTask>,
    pub task: WizardTask,
    /// Why new changes are blocked now (an unreadable journal counts), if they are.
    pub blocked: Option<String>,
}

pub fn summary(state: &AppState) -> Option<Summary> {
    let wizard = state.wizard.as_ref()?;
    let read = state.read.as_ref()?;
    let snapshot = read.snapshot.as_ref()?;
    let rows = rows(state)?;
    let plan = wizard_plan(snapshot, &plan_rows(&rows), wizard.standard);
    let cleanups = cleanup_tasks(snapshot, &rows, &wizard.deletes);
    let blocked = match &read.journal {
        Some(_) => crate::vm::status::blocked_reason(&read.summary, lang(state)),
        // Nothing is started on a journal that could not be read.
        None => Some(crate::i18n::cannot_change_now(None, lang(state))),
    };
    let awaiting = read.journal.as_ref().and_then(awaiting_cleanup);
    let task = next_task(awaiting, blocked.is_some(), &cleanups, &plan, &rows);
    Some(Summary {
        rows,
        plan,
        cleanups,
        task,
        blocked,
    })
}

/// The migration's plan from the planning inventory, when prepared.
fn migration_plan<'a>(
    wizard: &'a WizardState,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Option<(Result<OperationPlan, OperationError>, &'a SystemSnapshot)> {
    let prepared = wizard.prepared.as_ref()?;
    let plan = plan_migration(
        &prepared.snapshot.keyboards,
        &prepared.snapshot.global,
        standard,
        assignments,
    );
    Some((plan, &prepared.snapshot))
}

/// The page for the current step.
pub fn page(state: &AppState) -> WizardPage {
    let lang = lang(state);
    let Some(wizard) = state.wizard.as_ref() else {
        return welcome_page(lang);
    };
    match wizard.step {
        WizardStep::Welcome => welcome_page(lang),
        WizardStep::InputMethods => {
            match state.read.as_ref().and_then(|read| read.snapshot.as_ref()) {
                Some(snapshot) => input_methods_page(&snapshot.input, lang),
                None => input_methods_page(&mklm_core::InputMethods::default(), lang),
            }
        }
        WizardStep::Keyboards => {
            let rows = rows(state);
            let detected_name = wizard.detection.device.as_deref().map(|device| {
                rows.as_deref()
                    .and_then(|rows| detected_row(rows, device))
                    .map_or_else(|| display_name(state, device), |row| row.name.clone())
            });
            keyboards_page(&KeyboardsContext {
                rows: rows.as_deref(),
                detection: &wizard.detection,
                detected_name: detected_name.as_deref(),
                lang,
            })
        }
        WizardStep::Summary => summary_view(state, wizard),
    }
}

fn summary_view(state: &AppState, wizard: &WizardState) -> WizardPage {
    let lang = lang(state);
    let Some(summary) = summary(state) else {
        return summary_page(&SummaryContext {
            rows: None,
            plan: &WizardPlan::NothingToDo,
            cleanups: &[],
            task: &WizardTask::Finish,
            blocked: None,
            standard_now: &mklm_core::LayoutTable::Jis,
            mode: mklm_core::GlobalMode::PerKeyboard,
            fixed: None,
            standard: Layout::Jis,
            migration: MigrationStatus::None,
            ran: wizard.ran,
            autostart: wizard.autostart,
            uac_notice_seen: state.settings.change.uac_notice_seen,
            elevated: state.elevated,
            lang,
        });
    };
    let global = state
        .read
        .as_ref()
        .and_then(|read| read.snapshot.as_ref())
        .map(|snapshot| snapshot.global.clone())
        .unwrap_or_default();
    let fixed = mklm_core::ps2_pin_layout(&global);
    let standard = match &summary.plan {
        WizardPlan::Migrate { standard, .. } => *standard,
        _ => wizard.standard.or(fixed).unwrap_or(Layout::Jis),
    };
    let planned = match &summary.task {
        WizardTask::Migrate {
            standard,
            assignments,
        } => migration_plan(wizard, *standard, assignments),
        _ => None,
    };
    let blocker = wizard
        .prepared
        .as_ref()
        .and_then(|prepared| prepared.blocker.as_ref())
        .map(|reason| {
            let what = reason
                .op()
                .map(|op| {
                    crate::vm::journal::kind_text(
                        &op.kind,
                        &|id: &str| display_name(state, id),
                        lang,
                    )
                })
                .unwrap_or_default();
            crate::i18n::block_reason(reason, &what, lang)
        });
    let migration = match (&summary.task, &wizard.failure, &blocker, &planned) {
        (WizardTask::Migrate { .. }, Some(failure), _, _) => MigrationStatus::Failed(failure),
        (WizardTask::Migrate { .. }, None, Some(reason), _) => MigrationStatus::Blocked(reason),
        (WizardTask::Migrate { .. }, None, None, Some((plan, snapshot))) => {
            MigrationStatus::Planned { plan, snapshot }
        }
        (WizardTask::Migrate { .. }, None, None, None) => MigrationStatus::Preparing,
        _ => MigrationStatus::None,
    };
    summary_page(&SummaryContext {
        rows: Some(&summary.rows),
        plan: &summary.plan,
        cleanups: &summary.cleanups,
        task: &summary.task,
        blocked: summary.blocked.as_deref(),
        standard_now: &global.standard_layout(),
        mode: global.mode(),
        fixed,
        standard,
        migration,
        ran: wizard.ran,
        autostart: wizard.autostart,
        uac_notice_seen: state.settings.change.uac_notice_seen,
        elevated: state.elevated,
        lang,
    })
}

/// Opens the wizard at its first step (Settings, or `Navigate(Page::Wizard)`).
pub fn open(state: &mut AppState) -> Vec<Effect> {
    if !idle(state) {
        return Vec::new();
    }
    // The sign-in start switch shows what it will do: on for a first setup (design m3 F.3), the
    // value itself when the setup runs again.
    let autostart = !state.settings.wizard.completed
        || state
            .autostart
            .as_ref()
            .is_none_or(|report| crate::autostart::view(Some(report)).on);
    stop_identifying(state);
    state.draft = None;
    state.key_test = KeyTest::default();
    state.wizard = Some(WizardState {
        autostart,
        ..WizardState::default()
    });
    state.page = Page::Wizard;
    vec![Effect::Read, Effect::Render]
}

/// Handles a message of the wizard.
pub fn handle(state: &mut AppState, msg: WizardMsg) -> Vec<Effect> {
    if let WizardMsg::Open = msg {
        return open(state);
    }
    if let WizardMsg::ForeignRead(found) = msg {
        let Some(wizard) = state.wizard.as_mut() else {
            return Vec::new();
        };
        wizard.reading_foreign = false;
        // A hint only: without it the rows just do not mention those values.
        wizard.foreign = Some(found.unwrap_or_default());
        return vec![Effect::Render];
    }
    if state.page != Page::Wizard || !idle(state) || state.wizard.is_none() {
        return Vec::new();
    }
    match msg {
        WizardMsg::Next => next(state),
        WizardMsg::Back => back(state),
        WizardMsg::Skip => skip(state),
        WizardMsg::Secondary => secondary(state),
        WizardMsg::Choose { row, index } => {
            let layout = match index {
                0 => Layout::Jis,
                1 => Layout::Us,
                _ => return Vec::new(),
            };
            set_choice(state, &row, layout)
        }
        WizardMsg::Problem { row, delete } => {
            let Some(wizard) = state.wizard.as_mut() else {
                return Vec::new();
            };
            if wizard.step != WizardStep::Keyboards {
                return Vec::new();
            }
            wizard.deletes.retain(|known| !same(known, &row));
            if delete {
                wizard.deletes.push(row);
            }
            vec![Effect::Render]
        }
        WizardMsg::Standard(index) => {
            let standard = match index {
                0 => Layout::Jis,
                1 => Layout::Us,
                _ => return Vec::new(),
            };
            let Some(wizard) = state.wizard.as_mut() else {
                return Vec::new();
            };
            if wizard.step != WizardStep::Summary || wizard.standard == Some(standard) {
                return Vec::new();
            }
            wizard.standard = Some(standard);
            vec![Effect::Render]
        }
        WizardMsg::Autostart(on) => {
            if let Some(wizard) = state.wizard.as_mut() {
                wizard.autostart = on;
            }
            Vec::new()
        }
        WizardMsg::RestartDetection => {
            let Some(wizard) = state.wizard.as_mut() else {
                return Vec::new();
            };
            wizard.detection.restart();
            vec![Effect::Render]
        }
        WizardMsg::UseDetected => use_detected(state),
        WizardMsg::Open | WizardMsg::ForeignRead(_) => Vec::new(),
    }
}

fn set_choice(state: &mut AppState, row: &str, layout: Layout) -> Vec<Effect> {
    let choosable = rows(state).is_some_and(|rows| {
        rows.iter()
            .any(|known| same(&known.id, row) && known.can_choose)
    });
    let Some(wizard) = state.wizard.as_mut() else {
        return Vec::new();
    };
    if wizard.step != WizardStep::Keyboards || !choosable {
        return Vec::new();
    }
    wizard.choices.retain(|(known, _)| !same(known, row));
    wizard.choices.push((row.to_string(), layout));
    vec![Effect::Render]
}

/// Selects the layout the detection found on its keyboard's row (one click, design m3 B.3).
fn use_detected(state: &mut AppState) -> Vec<Effect> {
    let Some(wizard) = state.wizard.as_ref() else {
        return Vec::new();
    };
    let layout = match wizard.detection.verdict() {
        Some(Verdict::Jis) => Layout::Jis,
        Some(Verdict::Us) => Layout::Us,
        Some(Verdict::Mixed) | None => return Vec::new(),
    };
    let Some(device) = wizard.detection.device.clone() else {
        return Vec::new();
    };
    let Some(row) = rows(state)
        .as_deref()
        .and_then(|rows| detected_row(rows, &device).map(|row| row.id.clone()))
    else {
        return Vec::new();
    };
    set_choice(state, &row, layout)
}

fn next(state: &mut AppState) -> Vec<Effect> {
    let Some(wizard) = state.wizard.as_mut() else {
        return Vec::new();
    };
    match wizard.step {
        WizardStep::Welcome => {
            wizard.step = WizardStep::InputMethods;
            // The input methods as they are now.
            vec![Effect::Read, Effect::Render]
        }
        WizardStep::InputMethods => {
            wizard.step = WizardStep::Keyboards;
            wizard.detection = Detection::new();
            state.key_test = KeyTest::default();
            let mut effects = read_foreign(state);
            effects.extend([Effect::Read, Effect::Render]);
            effects
        }
        WizardStep::Keyboards => {
            wizard.step = WizardStep::Summary;
            forget_preparation(wizard);
            // What step 4 decides on is read again first (the journal in particular).
            vec![Effect::Read, Effect::Render]
        }
        WizardStep::Summary => run_task(state),
    }
}

fn back(state: &mut AppState) -> Vec<Effect> {
    let Some(wizard) = state.wizard.as_mut() else {
        return Vec::new();
    };
    let Some(previous) = wizard.step.previous() else {
        return Vec::new();
    };
    if wizard.step == WizardStep::Summary {
        forget_preparation(wizard);
    }
    wizard.step = previous;
    vec![Effect::Render]
}

fn forget_preparation(wizard: &mut WizardState) {
    wizard.preparing = None;
    wizard.prepared = None;
    wizard.failure = None;
}

/// "後でセットアップする": closed and completed; Settings opens it again (design m3 B.1). Nothing
/// is written for the user but settings.toml.
fn skip(state: &mut AppState) -> Vec<Effect> {
    state.settings.wizard.completed = true;
    state.wizard = None;
    let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
    effects.extend(update(state, AppMsg::Navigate(Page::Main)));
    effects
}

/// Marks the wizard completed and applies the sign-in start switch (design m3 B.1 "完了", F.3).
fn complete(state: &mut AppState) -> Vec<Effect> {
    let autostart = state.wizard.as_ref().is_none_or(|wizard| wizard.autostart);
    state.settings.wizard.completed = true;
    state.wizard = None;
    vec![
        Effect::SaveSettings(Box::new(state.settings.clone())),
        Effect::Autostart(AutostartTask::Set(autostart)),
    ]
}

/// "完了".
fn finish(state: &mut AppState) -> Vec<Effect> {
    let mut effects = complete(state);
    effects.extend(update(state, AppMsg::Navigate(Page::Main)));
    effects
}

/// The result's next step leads away from the wizard after one of its requests (the restart
/// page after the migration, say): the wizard has done its part and is completed.
pub(super) fn leaving_for_next_step(state: &mut AppState) -> Vec<Effect> {
    if state.page != Page::Wizard || !state.wizard.as_ref().is_some_and(|wizard| wizard.ran) {
        return Vec::new();
    }
    complete(state)
}

/// The summary's primary button: the next task.
fn run_task(state: &mut AppState) -> Vec<Effect> {
    let Some(summary) = summary(state) else {
        return Vec::new();
    };
    match summary.task {
        WizardTask::Finish => finish(state),
        WizardTask::KeepCleanup(awaiting) => {
            let targets = targets(state, &summary.rows, Some(&awaiting.instance_id));
            start(
                state,
                Request::Confirm {
                    op_id: awaiting.op_id,
                },
                ApplyOptions::default(),
                targets,
            )
        }
        WizardTask::Cleanup(cleanup) => {
            let targets = targets(state, &summary.rows, Some(&cleanup.instance_id));
            start(
                state,
                Request::CleanupValues {
                    instance_id: cleanup.instance_id,
                    names: cleanup.names,
                },
                ApplyOptions::default(),
                targets,
            )
        }
        WizardTask::Migrate {
            standard,
            assignments,
        } => {
            let Some(request) = migration_request(state, standard, &assignments) else {
                return Vec::new();
            };
            let targets = targets(state, &summary.rows, None);
            start(state, request, ApplyOptions::default(), targets)
        }
        WizardTask::SetLayout { row, choice, .. } => open_change(state, &row, choice),
    }
}

/// "元に戻す" of the cleanup that waits: `Request::Revert` (nothing to reset: the values were never
/// read by the driver).
fn secondary(state: &mut AppState) -> Vec<Effect> {
    let Some(summary) = summary(state) else {
        return Vec::new();
    };
    let WizardTask::KeepCleanup(awaiting) = summary.task else {
        return Vec::new();
    };
    let targets = targets(state, &summary.rows, Some(&awaiting.instance_id));
    let apply = ApplyOptions::default();
    start(
        state,
        Request::Revert {
            op_id: awaiting.op_id,
            apply,
        },
        apply,
        targets,
    )
}

/// The migration request, planned from the planning inventory with the expected plan (design m2
/// S6); `None` while it is not prepared, refused, or the journal blocks it.
fn migration_request(
    state: &AppState,
    standard: Layout,
    assignments: &[(String, LayoutChoice)],
) -> Option<Request> {
    let wizard = state.wizard.as_ref()?;
    if wizard.preparing.is_some()
        || wizard
            .prepared
            .as_ref()
            .is_none_or(|prepared| prepared.blocker.is_some())
    {
        return None;
    }
    let (plan, _) = migration_plan(wizard, standard, assignments)?;
    let plan = plan.ok()?;
    Some(Request::Migrate(MigrateRequest {
        standard,
        assignments: assignments
            .iter()
            .map(|(instance_id, layout)| Assignment {
                instance_id: instance_id.clone(),
                layout: *layout,
            })
            .collect(),
        expected: Some(mklm_client::preview::expected(&plan)),
    }))
}

/// The keyboards a request is about, for the result's "今の状態" (design m3 B.17): the row of
/// `instance_id`, or every choosable row.
fn targets(
    state: &AppState,
    rows: &[WizardKeyboard],
    instance_id: Option<&str>,
) -> Vec<SessionTarget> {
    let snapshot = state.read.as_ref().and_then(|read| read.snapshot.as_ref());
    rows.iter()
        .filter(|row| match instance_id {
            Some(id) => row.members.iter().any(|member| same(member, id)),
            None => row.can_choose,
        })
        .map(|row| SessionTarget {
            name: row.name.clone(),
            members: row.members.clone(),
            before: typed_before(snapshot, &row.members),
        })
        .collect()
}

/// Starts a request of the wizard page: the first one passes the standalone UAC explanation
/// (design m3 B.5, M2 R12).
fn start(
    state: &mut AppState,
    request: Request,
    apply: ApplyOptions,
    targets: Vec<SessionTarget>,
) -> Vec<Effect> {
    if !state.elevated && !state.settings.change.uac_notice_seen {
        if let Some(wizard) = state.wizard.as_mut() {
            wizard.waiting = Some(WaitingRequest {
                request,
                apply,
                targets,
            });
        }
        state.page = Page::UacNotice;
        return vec![Effect::Render];
    }
    super::start_request(state, request, apply, targets)
}

/// "確認画面へ進む" for a request of the wizard page; `None` when the explanation came from the
/// change page.
pub(super) fn uac_go(state: &mut AppState) -> Option<Vec<Effect>> {
    let waiting = state.wizard.as_mut()?.waiting.take()?;
    state.settings.change.uac_notice_seen = true;
    state.page = Page::Wizard;
    let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
    effects.extend(super::start_request(
        state,
        waiting.request,
        waiting.apply,
        waiting.targets,
    ));
    Some(effects)
}

/// "キャンセル" on the UAC explanation of a wizard request: back to the wizard, nothing sent.
pub(super) fn uac_cancel(state: &mut AppState) -> bool {
    let waited = state
        .wizard
        .as_mut()
        .and_then(|wizard| wizard.waiting.take())
        .is_some();
    if waited {
        state.page = Page::Wizard;
    }
    waited
}

/// An ordinary change of per-keyboard mode: the change page of the row, its layout chosen (the
/// preparation starts), and back to the wizard afterwards.
fn open_change(state: &mut AppState, row: &str, choice: LayoutChoice) -> Vec<Effect> {
    let mut effects = super::open_change(state, row);
    let Some(draft) = state.draft.as_ref() else {
        return effects;
    };
    let index =
        crate::vm::change::layout_choices(super::draft_snapshot(state, draft), &draft.members)
            .iter()
            .position(|known| *known == choice);
    if let Some(draft) = state.draft.as_mut() {
        draft.wizard = true;
    }
    if let Some(index) = index {
        for effect in super::choose_layout(state, index) {
            if !effects.contains(&effect) {
                effects.push(effect);
            }
        }
    }
    effects
}

/// The values of the non-keyboard collections are read once, when step 3 has a snapshot.
fn read_foreign(state: &mut AppState) -> Vec<Effect> {
    let Some(keyboards) = state
        .read
        .as_ref()
        .and_then(|read| read.snapshot.as_ref())
        .map(|snapshot| snapshot.keyboards.clone())
    else {
        return Vec::new();
    };
    let Some(wizard) = state.wizard.as_mut() else {
        return Vec::new();
    };
    if wizard.step != WizardStep::Keyboards || wizard.foreign.is_some() || wizard.reading_foreign {
        return Vec::new();
    }
    wizard.reading_foreign = true;
    vec![Effect::ReadNonKeyboardValues(keyboards)]
}

/// Step 4 with the migration next: read what it is planned with on the I/O worker (design m3 B.5),
/// once per visit of the step and per request.
fn prepare(state: &mut AppState) -> Vec<Effect> {
    if state.page != Page::Wizard || !idle(state) {
        return Vec::new();
    }
    let ready = state.wizard.as_ref().is_some_and(|wizard| {
        wizard.step == WizardStep::Summary
            && wizard.preparing.is_none()
            && wizard.prepared.is_none()
            && wizard.failure.is_none()
    });
    if !ready
        || !summary(state).is_some_and(|summary| matches!(summary.task, WizardTask::Migrate { .. }))
    {
        return Vec::new();
    }
    let token = state.next_prepare;
    state.next_prepare += 1;
    if let Some(wizard) = state.wizard.as_mut() {
        wizard.preparing = Some(token);
    }
    vec![Effect::PrepareChange {
        token,
        gate: Gate::NewOp,
    }]
}

/// The wizard's preparation `token` arrived (`AppMsg::ChangePrepared`); `None` when it is not the
/// wizard's.
pub(super) fn prepared(
    state: &mut AppState,
    token: u64,
    prepared: Result<PreparedChange, PrepareFailure>,
) -> Option<Vec<Effect>> {
    let wizard = state.wizard.as_mut()?;
    if wizard.preparing != Some(token) {
        return None;
    }
    wizard.preparing = None;
    Some(match prepared {
        Ok(prepared) => {
            // Blocked meanwhile: the page says why, and the rest of it follows the next read.
            let blocked = prepared.blocker.is_some();
            wizard.prepared = Some(prepared);
            wizard.failure = None;
            if blocked {
                vec![Effect::Read, Effect::Render]
            } else {
                vec![Effect::Render]
            }
        }
        Err(failure) => {
            wizard.failure = Some(failure);
            vec![Effect::Render]
        }
    })
}

/// After every `SystemRead`: step 3 reads the non-keyboard values once; step 4 prepares the
/// migration when it is next.
pub(super) fn after_read(state: &mut AppState) -> Vec<Effect> {
    if state.page != Page::Wizard {
        return Vec::new();
    }
    let mut effects = read_foreign(state);
    effects.extend(prepare(state));
    effects
}

/// A session ended: `from_change_page` is a change page the wizard opened. The wizard's own
/// requests and its change pages return to the wizard, which plans again from the next read.
pub(super) fn session_ended(state: &mut AppState, from_change_page: bool) -> bool {
    let ours = from_change_page || state.page == Page::Wizard;
    let Some(wizard) = state.wizard.as_mut() else {
        return false;
    };
    if !ours {
        return false;
    }
    wizard.ran = true;
    forget_preparation(wizard);
    true
}

/// A Raw Input key press on step 3: the detection (design m3 B.3). Returns whether the page
/// changed and the physical layout it found (instance ID and layout, remembered in settings).
pub(super) fn device_key(
    state: &mut AppState,
    instance_id: &str,
    scancode: u32,
) -> (bool, Option<(String, PhysicalKind)>) {
    if state.page != Page::Wizard || !idle(state) {
        return (false, None);
    }
    let Some(wizard) = state.wizard.as_mut() else {
        return (false, None);
    };
    if wizard.step != WizardStep::Keyboards {
        return (false, None);
    }
    let before = wizard.detection.clone();
    let press = wizard.detection.press(instance_id, scancode);
    let learned = match (
        press,
        wizard.detection.device.clone(),
        wizard.detection.verdict(),
    ) {
        (Press::Counted, Some(device), Some(Verdict::Jis)) => Some((device, PhysicalKind::Jis)),
        (Press::Counted, Some(device), Some(Verdict::Us)) => Some((device, PhysicalKind::Us)),
        _ => None,
    };
    (wizard.detection != before, learned)
}

#[cfg(test)]
mod tests;
