//! The GUI's state and its transitions (design m3 A.6): one [`AppState`] on the UI thread,
//! changed only by [`update`] from an [`AppMsg`]; `update` returns [`Effect`]s that `app.rs`
//! carries out (start a helper session, read the system, save settings …). Pure, so the flows of
//! design m3 B are unit-tested without a window, a helper or a registry.
//!
//! Helper sessions (design m3 A.4, B.18): at most one at a time. [`SessionPhase::Launching`] is
//! set in the same `update` that emits [`Effect::StartSession`], so a second start is refused
//! even while the UAC prompt is up; every message from a session worker carries its
//! [`SessionId`], and messages of an older session are dropped.
//!
//! The change flow (design m3 B.3 to B.7, B.17; WP-U3): "変更…" opens the change page with a
//! [`ChangeDraft`]; choosing a layout reads what the change is planned with on the I/O worker
//! ([`Effect::PrepareChange`], answered by [`AppMsg::ChangePrepared`]); the plan is made here
//! with the options of the apply method (`vm::change::plan_change`, pure), so that what the page
//! shows is what is sent. "変更する" starts the session — the first time through the standalone
//! UAC explanation. Keep and revert answer only the open question of the current session; after
//! an answer the progress dialog says what MKLM waits for, and a late countdown tick does not
//! bring the question back. The result waits for the read that follows the session
//! ([`Effect::ReadForResult`]) before it says what the keyboards do now.
//!
//! The main screen (design m3 B.2, B.12; WP-U1): keyboard arrivals and removals are gathered for
//! [`KEYBOARD_SETTLE`] and read once; hiding a row and "非表示と未接続も表示" are saved at once;
//! the banner's button opens the page of the attention it names. "今すぐ反映…" goes through the
//! same change page and session as a change ([`ChangeDraft::apply_now`]): the two ways of design
//! m3 B.5, the UAC explanation, then `Request::Recover` with a live reset — only after the
//! button, and only while the journal still lists a keyboard a reset puts into effect.

use std::time::{Duration, Instant};

use mklm_client::gate::{BlockReason, Gate};
use mklm_client::orchestrator::RequestReport;
use mklm_client::run_once::{RunOnceError, RunOnceOutcome};
use mklm_client::session::{Notice, Prompt, SessionKind, SessionView};
use mklm_client::startup::StartupSummary;
use mklm_core::{
    ApplyOptions, BootId, Decision, Event, Journal, Layout, LayoutChoice, LayoutTable, OpId,
    SystemSnapshot, assess,
};
use mklm_ipc::Request;

use crate::detect::{Detection, Press, Verdict};
use crate::i18n::Lang;
use crate::settings::{PhysicalKind, PhysicalLayout, PhysicalSource, Settings};
use crate::vm::change::{
    ApplyMethod, DraftPlan, InputActivity, MethodDefault, default_apply_method,
};
use crate::vm::keyboards::{device_members, devices};
use crate::vm::keytest::{self, Expected, KeyContext, KeyTest};
use crate::vm::session::Stage;
use crate::vm::status::{BannerTarget, NeedsApply, banner_target, needs_apply};

/// How long keyboard arrivals and removals are gathered before the list is read again (design
/// m3 A.4, B.2): a dongle or a reset brings several interfaces within a few hundred
/// milliseconds, and one read covers them all.
pub const KEYBOARD_SETTLE: Duration = Duration::from_millis(750);

/// The page in the main area (matches `Screen` in ui/structs.slint). Twelve pages: the change
/// page covers assignment, detection and the apply method (review U14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    Wizard,
    #[default]
    Main,
    /// "配列の変更": choices, detection, apply method, UAC line (design m3 B.4, B.5).
    Change,
    /// The standalone UAC explanation, the first time only (`settings.change.uac_notice_seen`).
    UacNotice,
    Restart,
    PostReboot,
    Conflict,
    Journal,
    Recovery,
    ImeHelp,
    Settings,
    About,
}

/// The overlay (matches `Overlay` in ui/structs.slint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayKind {
    #[default]
    None,
    Progress,
    Countdown,
    Reconnect,
    Result,
    /// The helper was lost; recovering needs another UAC prompt: "今すぐ元に戻す / 後で"
    /// (design m3 A.2.3).
    RecoveryConfirm,
    CloseNotice,
    QuitConfirm,
}

/// What the I/O worker read (design m3 A.4): everything the main screen needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemRead {
    pub snapshot: Option<SystemSnapshot>,
    /// Read problems, as text for the log and the details.
    pub warnings: Vec<String>,
    pub journal: Option<Journal>,
    pub boot: Option<BootId>,
    pub summary: StartupSummary,
}

/// What `PrepareChange` read on the I/O worker (design m3 B.5): the planning inventory, the
/// journal and whether the journal lets the request start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedChange {
    /// Every Keyboard-class devnode, phantoms included (`mklm_client::inventory::read_inventory`).
    pub snapshot: SystemSnapshot,
    /// Some value could not be read as MKLM expects: only the helper may conclude that nothing
    /// needs writing.
    pub uncertain_values: bool,
    /// Read problems that do not stop a write (English, for the technical details).
    pub warnings: Vec<String>,
    pub journal: Journal,
    /// `gate::blocker`: the request would be refused; no UAC prompt is shown.
    pub blocker: Option<BlockReason>,
}

/// Why `PrepareChange` could not read what the change is planned with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareFailure {
    /// Windows did not report every keyboard completely (read problems that stop a write,
    /// design m2 S2); English diagnostics.
    Incomplete(String),
    /// The keyboards, the journal or the boot ID could not be read; English diagnostics.
    Read(String),
}

impl PrepareFailure {
    /// The English diagnostics.
    pub fn diagnostic(&self) -> &str {
        match self {
            PrepareFailure::Incomplete(text) | PrepareFailure::Read(text) => text,
        }
    }
}

/// A change being prepared on the change page (design m3 B.4, B.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDraft {
    /// The row's ID (the container ID, or the instance ID of a keyboard without one).
    pub row: String,
    /// The device's display name.
    pub name: String,
    /// The row's keyboards (instance IDs); the request targets the first present one.
    pub members: Vec<String>,
    /// "MKLM 導入前に戻す…": the page previews the restore of this device.
    pub restore: bool,
    /// "今すぐ反映…" (design m3 B.12; review U8): no layout to choose, only how — a live reset
    /// through `Request::Recover`, or not now. `name` lists the devices, `members` holds every
    /// collection of them (the apply method's default and the result's "今の状態" look at these).
    pub apply_now: bool,
    pub choice: Option<LayoutChoice>,
    /// The apply method the user picked; `None` uses [`Self::method_default`].
    pub method: Option<ApplyMethod>,
    /// `vm::change::default_apply_method` when the preparation arrived.
    pub method_default: Option<MethodDefault>,
    /// The PC's standard layout of a migration from fixed mode; `None` keeps the fixed one.
    pub standard: Option<Layout>,
    /// The token of the `PrepareChange` that runs now: "確認しています…".
    pub preparing: Option<u64>,
    /// What the last preparation read.
    pub prepared: Option<PreparedChange>,
    pub failure: Option<PrepareFailure>,
    /// The plan shown (made with the chosen method's options); sent as `ExpectedPlan`.
    pub plan: Option<DraftPlan>,
    pub detection: Detection,
    /// What the last key press did in the detection.
    pub last_press: Option<Press>,
    /// An answer key came from this other keyboard (instance ID): the page names the one to use.
    pub other_keyboard: Option<String>,
}

impl ChangeDraft {
    pub fn new(row: String, name: String, members: Vec<String>) -> Self {
        Self {
            row,
            name,
            detection: Detection::for_keyboards(members.clone()),
            members,
            restore: false,
            apply_now: false,
            choice: None,
            method: None,
            method_default: None,
            standard: None,
            preparing: None,
            prepared: None,
            failure: None,
            plan: None,
            last_press: None,
            other_keyboard: None,
        }
    }

    /// The method the plan uses: the user's, else the default, else the restart.
    pub fn resolved_method(&self) -> ApplyMethod {
        self.method
            .or(self.method_default.map(|default| default.method))
            .unwrap_or(ApplyMethod::Restart)
    }
}

/// Identifies one helper session (a request and its recovery after a lost helper).
pub type SessionId = u64;

/// The helper session, as the UI thread knows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionPhase {
    #[default]
    Idle,
    /// The worker was started and is launching a helper: the UAC prompt may be up. Quitting now
    /// is immediate — a helper that starts after this process ended finds no pipe and writes
    /// nothing (design m3 F.5).
    Launching { id: SessionId },
    /// The helper is connected (`Notice::Connected`); `view` is the relay's last view.
    Running {
        id: SessionId,
        view: Box<SessionView>,
    },
}

impl SessionPhase {
    pub fn id(&self) -> Option<SessionId> {
        match self {
            SessionPhase::Idle => None,
            SessionPhase::Launching { id } | SessionPhase::Running { id, .. } => Some(*id),
        }
    }
}

/// What the user or the orchestrator did that the progress dialog should say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionNote {
    #[default]
    None,
    /// "このままにする" was sent.
    Keeping,
    /// "元に戻す", Esc, the window's × or quitting during a countdown.
    Reverting,
    /// "後で決める" or "終了して後で決める" on the reconnect path.
    Leaving,
    /// The helper stopped (`Notice::HelperLost`).
    HelperLost,
    /// The recovery after a lost helper runs.
    Recovering,
}

/// A keyboard the result's "今の状態" names (design m3 B.17, review U7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTarget {
    pub name: String,
    /// Its instance IDs (every collection of the device).
    pub members: Vec<String>,
    /// How it typed before the session ("（変更前のまま）").
    pub before: Option<LayoutTable>,
}

/// How a session ended, with the RunOnce rule applied on the worker right after it (design m2
/// C17, m3 A.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutcome {
    pub report: RequestReport,
    pub run_once: Result<RunOnceOutcome, RunOnceError>,
}

/// The answers of the quit confirmation on the reconnect path (design m3 F.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAnswer {
    RevertAndQuit,
    QuitLeavingIt,
    Cancel,
}

/// Everything the UI thread knows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppState {
    pub lang: Option<Lang>,
    pub settings: Settings,
    pub page: Page,
    pub overlay: OverlayKind,
    pub read: Option<SystemRead>,
    /// The GUI thread's active input language (HKL, low 32 bits), polled (design m3 A.4).
    pub active_hkl: u32,
    pub identify: bool,
    /// The keyboard (instance ID) that typed last while identifying.
    pub highlighted: Option<String>,
    /// The last Raw Input key press: keyboard (instance ID) and scan code (for the key test).
    pub last_key: Option<(String, u32)>,
    /// Recent key and pointer input (the apply method's default, design m3 B.5).
    pub activity: InputActivity,
    pub key_test: KeyTest,
    pub draft: Option<ChangeDraft>,
    /// The token the next `PrepareChange` gets.
    pub next_prepare: u64,
    /// The GUI runs elevated: no UAC prompt follows, so none is explained (design m3 B.5).
    pub elevated: bool,
    pub session: SessionPhase,
    /// The id the next session gets.
    pub next_session: SessionId,
    /// The request of the running or last session.
    pub request: Option<Request>,
    /// The keyboards of the running or last request, for "今の状態".
    pub targets: Vec<SessionTarget>,
    /// The last session view that had journaled the operation (the result's reset phase and
    /// operation ID).
    pub last_view: Option<Box<SessionView>>,
    /// The operation whose question the user answered (a late tick does not reopen it).
    pub decided: Option<OpId>,
    pub note: SessionNote,
    /// The last session's outcome, for the result overlay.
    pub outcome: Option<SessionOutcome>,
    /// The preparation found the way blocked: the result overlay says why (no prompt shown).
    pub blocked: Option<BlockReason>,
    /// The result waits for the read that follows the session ("確かめています…").
    pub result_read_pending: bool,
    /// The helper was lost during a countdown (true) or elsewhere, and the worker waits for the
    /// user's answer to the recovery question.
    pub recovery_question: Option<bool>,
    /// The window is shown (false while in the tray).
    pub visible: bool,
    /// Quitting once the running session ends (design m3 F.5).
    pub quit_pending: bool,
    /// A keyboard arrived or left and the read that follows is scheduled
    /// ([`KEYBOARD_SETTLE`]): further notifications join it.
    pub keyboards_settling: bool,
}

/// Something that happened (a user action, a worker's answer, a watcher).
#[derive(Debug, Clone, PartialEq)]
pub enum AppMsg {
    Navigate(Page),
    SystemRead(Box<SystemRead>),
    /// The read asked for by [`Effect::ReadForResult`] arrived (after its `SystemRead`).
    ResultReadArrived,
    ActiveLayout(u32),
    /// A key press seen through Raw Input (winit `DeviceEvent::Key`): the keyboard's instance
    /// ID and the set 1 scan code. Never the character (plan 2.2).
    DeviceKey {
        instance_id: String,
        scancode: u32,
        at: Instant,
    },
    /// Pointer input seen through Raw Input (at most one message every few seconds).
    PointerUsed(Instant),
    /// A key in the IME-free key test (the FocusScope text; kept in memory only, plan 2.2).
    KeyTestPressed {
        text: String,
        shift: bool,
    },
    ToggleIdentify,
    /// "キーボードを特定" from the tray menu (design m3 B.3, B.16): shows the window and starts
    /// identifying on the main screen — unless a flow is under way (a page that is not reached
    /// from the navigation, a session, an overlay), which is only brought to the front.
    StartIdentify,
    /// Read the system again (refresh button, F5, input language changed, resume).
    Refresh,
    /// "変更…" on a keyboard row (its ID): the change page (design m3 B.4).
    OpenChange(String),
    /// A layout choice on the change page (its index).
    ChangeChoose(usize),
    /// An apply method (0 = switch now, 1 = at the restart, review U1).
    ChangeChooseMethod(usize),
    /// The PC's standard layout of a migration (0 = JIS, 1 = US).
    ChangeChooseStandard(usize),
    /// "やり直す" of the in-place detection.
    ChangeRestartDetection,
    /// Select the layout the detection found (one click, design m3 B.3).
    ChangeUseDetected,
    /// "MKLM 導入前に戻す…" on the change page: the restore preview of this device.
    OpenRestore,
    /// What `PrepareChange` read (token `token`, answered at `at`).
    ChangePrepared {
        token: u64,
        at: Instant,
        prepared: Box<Result<PreparedChange, PrepareFailure>>,
    },
    /// "変更する" on the change page.
    ChangeApply,
    /// "確認画面へ進む" on the first-time UAC explanation.
    UacGo,
    /// "キャンセル" on the change page (to the main screen) or on the UAC explanation (back).
    CancelChange,
    /// A keyboard interface arrived or went away (`mklm_win::notify::KeyboardWatcher`): the
    /// list is read again [`KEYBOARD_SETTLE`] later, once for a burst (design m3 B.2).
    KeyboardsChanged,
    /// [`KEYBOARD_SETTLE`] has passed since the first of a burst of `KeyboardsChanged`.
    KeyboardsSettled,
    /// "非表示と未接続も表示" (design m3 B.2, B.14), kept in settings.toml.
    ShowHidden(bool),
    /// The row menu's "非表示にする / 表示する" for the row with group ID `id` (design m3 B.2).
    ToggleHidden(String),
    /// The main screen banner's button (design m3 B.12). `at`: when (the apply method's default
    /// counts the input of the last minutes, design m3 B.5).
    BannerAction {
        at: Instant,
    },
    /// "今すぐ反映…" on a row or the banner: the change page with the choice of how (design m3
    /// B.12, review U8). `at`: when (for the apply method's default, design m3 B.5).
    ApplyNow {
        at: Instant,
    },
    /// Start a helper session for `request` (the change page's button, recovery, undo …).
    StartRequest {
        request: Request,
        apply: ApplyOptions,
    },
    SessionEvent {
        session: SessionId,
        event: Box<Event>,
        view: Box<SessionView>,
    },
    SessionNotice {
        session: SessionId,
        notice: Notice,
    },
    /// The helper was lost and the journal asks for recovery: the worker waits for
    /// [`AppMsg::AnswerRecovery`].
    RecoveryQuestion {
        session: SessionId,
        countdown: bool,
    },
    AnswerRecovery(bool),
    SessionEnded {
        session: SessionId,
        outcome: Box<SessionOutcome>,
    },
    /// A decision for the open question (checked against it).
    Decide(Decision),
    /// "このままにする" in the countdown or the reconnect wait.
    KeepChange,
    /// "元に戻す" (and Esc) in the countdown or the reconnect wait.
    RevertChange,
    /// "後で決める" in the reconnect wait: leave; the change keeps waiting (design m3 B.7).
    DecideLater,
    ResultClosed,
    /// The result's next-step button.
    ResultNextStep,
    /// "詳細をコピー".
    CopyDetails,
    QuitConfirmAnswered(QuitAnswer),
    /// Show the window and bring it to the front (tray click, a second start). Never changes
    /// the page: the post-reboot check and the recovery prompt are decided from the journal on
    /// the next `SystemRead` (design m3 F.1).
    Activate,
    WindowCloseRequested,
    /// The first close notice was answered: quit MKLM, or keep it in the tray.
    CloseNoticeAnswered {
        quit: bool,
    },
    QuitRequested,
}

/// Something `app.rs` must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Read the snapshot, the journal and the boot ID on the I/O worker.
    Read,
    /// The same, then [`AppMsg::ResultReadArrived`]: the result's "今の状態" (design m3 B.17).
    ReadForResult,
    /// Read what a change is planned with on the I/O worker: the planning inventory, the journal
    /// and `gate::blocker(gate)` (design m3 B.5). Answered by [`AppMsg::ChangePrepared`].
    PrepareChange {
        token: u64,
        gate: Gate,
    },
    /// Send [`AppMsg::KeyboardsSettled`] after this delay (a single-shot timer on the UI thread).
    ScheduleSettle(Duration),
    SaveSettings(Box<Settings>),
    /// Start a helper session on a new session worker (only while no other one lives).
    StartSession {
        session: SessionId,
        request: Request,
        apply: ApplyOptions,
    },
    /// Pass a decision to the session worker.
    SendDecision(Decision),
    /// Ask the session worker to leave (`Frontend::cancel_requested`).
    CancelSession,
    /// Answer the worker's recovery question.
    AnswerRecovery(bool),
    /// Apply the post-reboot RunOnce rule on the I/O worker (at start-up, and when the user
    /// chose "decide later" on the post-reboot check). After a session the worker applies it.
    RunOnceRule,
    /// Put text on the clipboard ("詳細をコピー": English diagnostics only, never keys).
    CopyText(String),
    ShowWindow,
    HideWindow,
    Quit,
    /// Re-render every view-model (the language or the read changed).
    Render,
}

/// The display name of a keyboard (its instance ID when the snapshot does not know it).
pub fn display_name(state: &AppState, instance_id: &str) -> String {
    state
        .read
        .as_ref()
        .and_then(|read| read.snapshot.as_ref())
        .and_then(|snapshot| {
            snapshot
                .keyboards
                .iter()
                .find(|kb| kb.instance_id.eq_ignore_ascii_case(instance_id))
        })
        .map_or_else(|| instance_id.to_string(), |kb| kb.display_name.clone())
}

fn current_session(state: &AppState, session: SessionId) -> bool {
    state.session.id() == Some(session)
}

/// The navigation rail is off while a change, a check or a session runs (design m3 B.0).
pub fn navigation_enabled(state: &AppState) -> bool {
    state.session == SessionPhase::Idle
        && state.overlay == OverlayKind::None
        && !matches!(
            state.page,
            Page::Wizard | Page::Change | Page::UacNotice | Page::PostReboot
        )
}

/// What the progress dialog says (design m3 B.5, B.6).
pub fn progress_stage(state: &AppState) -> Stage {
    match (&state.session, state.note) {
        (SessionPhase::Launching { .. }, _) => Stage::Launching,
        (_, SessionNote::Keeping) => Stage::Answered { keep: true },
        (_, SessionNote::Reverting) => Stage::Answered { keep: false },
        (_, SessionNote::Leaving) => Stage::Leaving,
        (_, SessionNote::HelperLost) => Stage::HelperLost,
        (_, SessionNote::Recovering) => Stage::Recovering,
        (_, SessionNote::None) => Stage::Running,
    }
}

/// The running session's view, if the helper is connected.
pub fn session_view(state: &AppState) -> Option<&SessionView> {
    match &state.session {
        SessionPhase::Running { view, .. } => Some(view),
        _ => None,
    }
}

/// What the last read leaves to put into effect (`vm::status::needs_apply`); empty until the
/// snapshot, the journal and the boot ID are all known.
fn read_needs_apply(state: &AppState) -> NeedsApply {
    let Some(read) = &state.read else {
        return NeedsApply::default();
    };
    match (&read.snapshot, &read.journal, read.boot) {
        (Some(snapshot), Some(journal), Some(boot)) => {
            needs_apply(journal, boot, &read.summary, snapshot)
        }
        _ => NeedsApply::default(),
    }
}

/// The last read says new operations must wait (design m2 C.7), or nothing was read yet.
fn writes_blocked(state: &AppState) -> bool {
    state
        .read
        .as_ref()
        .is_none_or(|read| read.summary.blocks_writes())
}

/// The keyboards "今すぐ反映…" resets now, with every collection of their devices (the apply
/// method's default looks at these, design m3 B.5); empty when nothing can be offered: nothing is
/// left to put into effect, or new operations are blocked (a `Recover` would then recover the
/// blocking entries too, which is the recovery page's decision, design m3 B.12).
pub fn apply_now_targets(state: &AppState) -> Vec<String> {
    if writes_blocked(state) {
        return Vec::new();
    }
    let ids = read_needs_apply(state).apply_now;
    match state.read.as_ref().and_then(|read| read.snapshot.as_ref()) {
        Some(snapshot) if !ids.is_empty() => device_members(&mklm_core::assess(snapshot), &ids),
        _ => Vec::new(),
    }
}

/// Pages the navigation reaches; the others belong to a flow that "特定" from the tray must not
/// replace (design m3 B.0).
fn is_navigation_page(page: Page) -> bool {
    matches!(
        page,
        Page::Main | Page::Journal | Page::ImeHelp | Page::Settings | Page::About
    )
}

/// Identifying ends when the main screen goes (the capture field lives there).
fn stop_identifying(state: &mut AppState) {
    state.identify = false;
    state.highlighted = None;
}

/// Applies `msg` to `state`. Flows not written yet are no-ops that leave the state unchanged
/// (the skeleton never panics on a click).
pub fn update(state: &mut AppState, msg: AppMsg) -> Vec<Effect> {
    match msg {
        AppMsg::Navigate(page) => {
            if page != Page::Main {
                stop_identifying(state);
            }
            state.page = page;
            if !matches!(page, Page::Change | Page::UacNotice) {
                state.draft = None;
            }
            let mut effects = vec![Effect::Render];
            if page == Page::Main || page == Page::Journal {
                effects.insert(0, Effect::Read);
            }
            effects
        }
        AppMsg::SystemRead(read) => {
            // An open "今すぐ反映…" page follows the read: once nothing is left to put into
            // effect, it says so and sends nothing (`vm::keyboards::apply_now_page`).
            state.read = Some(*read);
            vec![Effect::Render]
        }
        AppMsg::ResultReadArrived => {
            if !state.result_read_pending {
                return Vec::new();
            }
            state.result_read_pending = false;
            vec![Effect::Render]
        }
        AppMsg::ActiveLayout(hkl) if hkl != state.active_hkl => {
            state.active_hkl = hkl;
            vec![Effect::Render]
        }
        AppMsg::ActiveLayout(_) => Vec::new(),
        AppMsg::KeyTestPressed { text, shift } => key_test_pressed(state, &text, shift),
        AppMsg::Refresh => vec![Effect::Read],
        AppMsg::KeyboardsChanged if state.keyboards_settling => Vec::new(),
        AppMsg::KeyboardsChanged => {
            state.keyboards_settling = true;
            vec![Effect::ScheduleSettle(KEYBOARD_SETTLE)]
        }
        AppMsg::KeyboardsSettled => {
            state.keyboards_settling = false;
            vec![Effect::Read]
        }
        AppMsg::ToggleIdentify => {
            // The capture field lives on the main screen; an overlay keeps the keys (design m3
            // B.3, E.1).
            if !state.identify && (state.page != Page::Main || state.overlay != OverlayKind::None) {
                return Vec::new();
            }
            state.identify = !state.identify;
            if !state.identify {
                state.highlighted = None;
            }
            vec![Effect::Render]
        }
        AppMsg::StartIdentify => {
            state.visible = true;
            let mut effects = vec![Effect::ShowWindow];
            let free = state.session == SessionPhase::Idle
                && state.overlay == OverlayKind::None
                && is_navigation_page(state.page);
            if free && state.page != Page::Main {
                // Navigating renders (and reads the list again).
                effects.extend(update(state, AppMsg::Navigate(Page::Main)));
                state.identify = true;
            } else {
                state.identify |= free;
                effects.push(Effect::Render);
            }
            effects
        }
        AppMsg::ShowHidden(on) => {
            if state.settings.keyboards.show_hidden == on {
                return Vec::new();
            }
            state.settings.keyboards.show_hidden = on;
            vec![
                Effect::SaveSettings(Box::new(state.settings.clone())),
                Effect::Render,
            ]
        }
        AppMsg::ToggleHidden(id) => {
            let hidden = &mut state.settings.keyboards.hidden;
            if hidden.iter().any(|known| known.eq_ignore_ascii_case(&id)) {
                hidden.retain(|known| !known.eq_ignore_ascii_case(&id));
            } else {
                hidden.push(id);
            }
            vec![
                Effect::SaveSettings(Box::new(state.settings.clone())),
                Effect::Render,
            ]
        }
        AppMsg::BannerAction { at } => banner_action(state, at),
        AppMsg::ApplyNow { at } => open_apply_now(state, at),
        AppMsg::DeviceKey {
            instance_id,
            scancode,
            at,
        } => device_key(state, instance_id, scancode, at),
        AppMsg::PointerUsed(at) => {
            state.activity.pointer(at);
            Vec::new()
        }
        AppMsg::OpenChange(row) => open_change(state, &row),
        AppMsg::ChangeChoose(index) => choose_layout(state, index),
        AppMsg::ChangeChooseMethod(index) => {
            let Some(method) = ApplyMethod::from_index(index) else {
                return Vec::new();
            };
            let Some(draft) = editable_draft(state) else {
                return Vec::new();
            };
            if draft.method == Some(method) {
                return Vec::new();
            }
            draft.method = Some(method);
            // "今すぐ反映…" has no plan: `Recover` decides under the lock what to reset.
            if !draft.apply_now {
                replan(draft);
            }
            vec![Effect::Render]
        }
        AppMsg::ChangeChooseStandard(index) => {
            let standard = match index {
                0 => Layout::Jis,
                1 => Layout::Us,
                _ => return Vec::new(),
            };
            let Some(draft) = editable_change(state) else {
                return Vec::new();
            };
            draft.standard = Some(standard);
            replan(draft);
            vec![Effect::Render]
        }
        AppMsg::ChangeRestartDetection => {
            let Some(draft) = editable_change(state) else {
                return Vec::new();
            };
            draft.detection.restart();
            draft.last_press = None;
            draft.other_keyboard = None;
            vec![Effect::Render]
        }
        AppMsg::ChangeUseDetected => {
            let Some(draft) = &state.draft else {
                return Vec::new();
            };
            let choice = match draft.detection.verdict() {
                Some(Verdict::Jis) => LayoutChoice::Jis,
                Some(Verdict::Us) => LayoutChoice::Us,
                Some(Verdict::Mixed) | None => return Vec::new(),
            };
            let choices =
                crate::vm::change::layout_choices(draft_snapshot(state, draft), &draft.members);
            match choices.iter().position(|c| *c == choice) {
                Some(index) => choose_layout(state, index),
                None => Vec::new(),
            }
        }
        AppMsg::OpenRestore => {
            let token = state.next_prepare;
            let Some(draft) = editable_change(state) else {
                return Vec::new();
            };
            draft.restore = true;
            draft.choice = None;
            draft.plan = None;
            draft.failure = None;
            draft.preparing = Some(token);
            state.next_prepare += 1;
            vec![
                Effect::PrepareChange {
                    token,
                    gate: Gate::Restore,
                },
                Effect::Render,
            ]
        }
        AppMsg::ChangePrepared {
            token,
            at,
            prepared,
        } => change_prepared(state, token, at, *prepared),
        AppMsg::ChangeApply => {
            if state.session != SessionPhase::Idle || state.page != Page::Change {
                return Vec::new();
            }
            let Some((request, apply)) = ready_request(state) else {
                return Vec::new();
            };
            if !state.elevated && !state.settings.change.uac_notice_seen {
                // Read before the first prompt (M2 R12); later changes explain it in one line.
                state.page = Page::UacNotice;
                return vec![Effect::Render];
            }
            start_change(state, request, apply)
        }
        AppMsg::UacGo => {
            if state.page != Page::UacNotice || state.session != SessionPhase::Idle {
                return Vec::new();
            }
            let Some((request, apply)) = ready_request(state) else {
                state.page = Page::Change;
                return vec![Effect::Render];
            };
            state.settings.change.uac_notice_seen = true;
            state.page = Page::Change;
            let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
            effects.extend(start_change(state, request, apply));
            effects
        }
        AppMsg::CancelChange => {
            if state.session != SessionPhase::Idle {
                return Vec::new();
            }
            match state.page {
                // Back to the choices: nothing is lost, nothing happens.
                Page::UacNotice => {
                    state.page = Page::Change;
                    vec![Effect::Render]
                }
                Page::Change => {
                    state.draft = None;
                    state.page = Page::Main;
                    vec![Effect::Read, Effect::Render]
                }
                _ => Vec::new(),
            }
        }
        AppMsg::StartRequest { request, apply } => start_request(state, request, apply, Vec::new()),
        AppMsg::SessionNotice { session, notice } if current_session(state, session) => {
            match notice {
                // A new helper is being launched (the request's, or the recovery's): a UAC
                // prompt may be up.
                Notice::Starting(kind) => {
                    state.session = SessionPhase::Launching { id: session };
                    if kind == SessionKind::Recovery {
                        state.note = SessionNote::Recovering;
                    }
                }
                Notice::Connected(_) => {
                    state.session = SessionPhase::Running {
                        id: session,
                        view: Box::default(),
                    };
                }
                Notice::HelperLost { .. } => {
                    state.overlay = OverlayKind::Progress;
                    state.note = SessionNote::HelperLost;
                }
                Notice::RecoveringAfterLoss { .. } => {
                    state.overlay = OverlayKind::Progress;
                    state.note = SessionNote::Recovering;
                }
            }
            vec![Effect::Render]
        }
        AppMsg::SessionEvent { session, view, .. } if current_session(state, session) => {
            session_event(state, session, view)
        }
        AppMsg::RecoveryQuestion { session, countdown } if current_session(state, session) => {
            state.recovery_question = Some(countdown);
            state.overlay = OverlayKind::RecoveryConfirm;
            state.visible = true;
            vec![Effect::ShowWindow, Effect::Render]
        }
        AppMsg::AnswerRecovery(yes) => {
            if state.recovery_question.take().is_none() {
                return Vec::new();
            }
            state.overlay = OverlayKind::Progress;
            state.note = if yes {
                SessionNote::Recovering
            } else {
                SessionNote::HelperLost
            };
            vec![Effect::AnswerRecovery(yes), Effect::Render]
        }
        AppMsg::SessionEnded { session, outcome } if current_session(state, session) => {
            session_ended(state, *outcome)
        }
        // Messages of a session that is not the current one.
        AppMsg::SessionNotice { .. }
        | AppMsg::SessionEvent { .. }
        | AppMsg::RecoveryQuestion { .. }
        | AppMsg::SessionEnded { .. } => Vec::new(),
        AppMsg::Decide(decision) => {
            let keep = matches!(decision, Decision::Keep { .. });
            if !session_view(state).is_some_and(|view| view.accepts(&decision)) {
                return Vec::new();
            }
            answer(state, keep)
        }
        AppMsg::KeepChange => answer(state, true),
        AppMsg::RevertChange => answer(state, false),
        AppMsg::DecideLater => {
            let reconnect = session_view(state)
                .is_some_and(|view| matches!(view.prompt, Prompt::Reconnect { .. }));
            if !reconnect || state.decided.is_some() {
                return Vec::new();
            }
            state.note = SessionNote::Leaving;
            state.overlay = OverlayKind::Progress;
            state.page = Page::Main;
            state.draft = None;
            vec![Effect::CancelSession, Effect::Render]
        }
        AppMsg::ResultClosed => {
            if state.overlay != OverlayKind::Result {
                return Vec::new();
            }
            state.overlay = OverlayKind::None;
            state.blocked = None;
            if matches!(state.page, Page::Change | Page::UacNotice) {
                state.page = Page::Main;
                state.draft = None;
            }
            vec![Effect::Read, Effect::Render]
        }
        AppMsg::ResultNextStep => {
            if state.overlay != OverlayKind::Result {
                return Vec::new();
            }
            let lang = state.lang.unwrap_or(Lang::Ja);
            let Some(next) = crate::vm::result::shown_result(state, lang).and_then(|r| r.next)
            else {
                return Vec::new();
            };
            state.overlay = OverlayKind::None;
            state.blocked = None;
            let page = match next {
                crate::vm::result::NextStep::Restart => Page::Restart,
                crate::vm::result::NextStep::PostReboot => Page::PostReboot,
                crate::vm::result::NextStep::Conflict => Page::Conflict,
                crate::vm::result::NextStep::Recovery => Page::Recovery,
                crate::vm::result::NextStep::ImeHelp => Page::ImeHelp,
            };
            let mut effects = update(state, AppMsg::Navigate(page));
            if !effects.contains(&Effect::Read) {
                effects.insert(0, Effect::Read);
            }
            effects
        }
        AppMsg::CopyDetails => {
            let lang = state.lang.unwrap_or(Lang::Ja);
            crate::vm::result::shown_result(state, lang)
                .filter(|result| {
                    state.overlay == OverlayKind::Result && !result.copy_text.is_empty()
                })
                .map(|result| vec![Effect::CopyText(result.copy_text)])
                .unwrap_or_default()
        }
        AppMsg::QuitConfirmAnswered(answer_) => quit_confirm_answered(state, answer_),
        AppMsg::Activate => {
            state.visible = true;
            vec![Effect::ShowWindow, Effect::Read, Effect::Render]
        }
        AppMsg::WindowCloseRequested => close_requested(state),
        AppMsg::CloseNoticeAnswered { quit } => {
            state.settings.tray.close_notice_shown = true;
            state.overlay = OverlayKind::None;
            let mut effects = vec![Effect::SaveSettings(Box::new(state.settings.clone()))];
            if quit {
                effects.extend(update(state, AppMsg::QuitRequested));
            } else {
                state.visible = false;
                effects.push(Effect::HideWindow);
            }
            effects
        }
        AppMsg::QuitRequested => quit_requested(state),
    }
}

/// The snapshot a draft's page describes: the preparation's, else the display read's.
fn draft_snapshot<'a>(state: &'a AppState, draft: &'a ChangeDraft) -> Option<&'a SystemSnapshot> {
    draft
        .prepared
        .as_ref()
        .map(|prepared| &prepared.snapshot)
        .or_else(|| state.read.as_ref().and_then(|read| read.snapshot.as_ref()))
}

/// The draft while the change page can still be edited (no session runs).
fn editable_draft(state: &mut AppState) -> Option<&mut ChangeDraft> {
    if state.session != SessionPhase::Idle || state.page != Page::Change {
        return None;
    }
    state.draft.as_mut()
}

/// [`editable_draft`] of a layout change or restore: "今すぐ反映…" has no layout, detection or
/// restore.
fn editable_change(state: &mut AppState) -> Option<&mut ChangeDraft> {
    editable_draft(state).filter(|draft| !draft.apply_now)
}

/// The request the change page's button sends now, with its options; `None` while nothing may be
/// sent. "今すぐ反映…" sends `Request::Recover` with a live reset, and only while the last read
/// still lists a keyboard a reset puts into effect and "reset now" is chosen (design m3 B.12).
fn ready_request(state: &AppState) -> Option<(Request, ApplyOptions)> {
    let draft = state.draft.as_ref()?;
    if !draft.apply_now {
        return crate::vm::change::draft_request(draft);
    }
    if draft.resolved_method() != ApplyMethod::Live || apply_now_targets(state).is_empty() {
        return None;
    }
    // Resetting to put a kept layout into effect asks no keep-or-revert question.
    let apply = ApplyMethod::Live.options();
    Some((Request::Recover { apply }, apply))
}

/// "今すぐ反映…" on a row or the banner (design m3 B.12; review U8): the change page with the two
/// ways of design m3 B.5 for every device the journal lists. Nothing starts here.
fn open_apply_now(state: &mut AppState, at: Instant) -> Vec<Effect> {
    if state.session != SessionPhase::Idle
        || state.overlay != OverlayKind::None
        || state.page != Page::Main
    {
        return Vec::new();
    }
    let targets = apply_now_targets(state);
    let Some(snapshot) = state.read.as_ref().and_then(|read| read.snapshot.as_ref()) else {
        return Vec::new();
    };
    if targets.is_empty() {
        return Vec::new();
    }
    let lang = state.lang.unwrap_or(Lang::Ja);
    let names: Vec<String> = devices(&assess(snapshot), &targets)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    // Resetting takes these keyboards away for a few seconds: the default of a change (another
    // keyboard or the pointer seen → now; only these → not now, with the warning of plan 1.4).
    let method_default = default_apply_method(&state.activity, &targets, at);
    state.draft = Some(ChangeDraft {
        apply_now: true,
        method_default: Some(method_default),
        ..ChangeDraft::new(String::new(), crate::i18n::name_list(&names, lang), targets)
    });
    stop_identifying(state);
    state.key_test = KeyTest::default();
    state.page = Page::Change;
    vec![Effect::Render]
}

/// The banner's button (design m3 B.12): the page of the attention it names, read again first
/// (what the page decides on comes from the journal), or the "今すぐ反映…" page.
fn banner_action(state: &mut AppState, at: Instant) -> Vec<Effect> {
    if state.session != SessionPhase::Idle
        || state.overlay != OverlayKind::None
        || state.page != Page::Main
    {
        return Vec::new();
    }
    let Some(read) = &state.read else {
        return Vec::new();
    };
    let page = match banner_target(&read.summary, &read_needs_apply(state)) {
        None => return Vec::new(),
        Some(BannerTarget::ApplyNow) => return open_apply_now(state, at),
        Some(BannerTarget::Recovery) => Page::Recovery,
        Some(BannerTarget::Conflict) => Page::Conflict,
        Some(BannerTarget::Restart) => Page::Restart,
        Some(BannerTarget::PostReboot) => Page::PostReboot,
    };
    let mut effects = update(state, AppMsg::Navigate(page));
    if !effects.contains(&Effect::Read) {
        effects.insert(0, Effect::Read);
    }
    effects
}

/// "変更…" on a row: the change page for its device (design m3 B.4).
fn open_change(state: &mut AppState, row: &str) -> Vec<Effect> {
    if state.session != SessionPhase::Idle {
        return Vec::new();
    }
    let Some(snapshot) = state.read.as_ref().and_then(|read| read.snapshot.as_ref()) else {
        return Vec::new();
    };
    let assessment = assess(snapshot);
    // The row ID is the container of an external device, or a keyboard's instance ID
    // (`vm::keyboards::keyboard_rows`).
    let Some(group) = assessment.groups.iter().find(|group| {
        (!group.is_internal
            && group
                .container_id
                .as_deref()
                .is_some_and(|container| container.eq_ignore_ascii_case(row)))
            || group
                .keyboards
                .iter()
                .any(|id| id.eq_ignore_ascii_case(row))
    }) else {
        return Vec::new();
    };
    state.draft = Some(ChangeDraft::new(
        row.to_string(),
        group.display_name.clone(),
        group.keyboards.clone(),
    ));
    stop_identifying(state);
    state.key_test = KeyTest::default();
    state.page = Page::Change;
    vec![Effect::Render]
}

/// A layout choice: read what the change is planned with (design m3 B.5).
fn choose_layout(state: &mut AppState, index: usize) -> Vec<Effect> {
    let token = state.next_prepare;
    let choices = match &state.draft {
        Some(draft) => {
            crate::vm::change::layout_choices(draft_snapshot(state, draft), &draft.members)
        }
        None => return Vec::new(),
    };
    let Some(choice) = choices.get(index).copied() else {
        return Vec::new();
    };
    let Some(draft) = editable_change(state) else {
        return Vec::new();
    };
    if draft.restore || (draft.choice == Some(choice) && draft.failure.is_none()) {
        return Vec::new();
    }
    draft.choice = Some(choice);
    draft.plan = None;
    draft.failure = None;
    draft.preparing = Some(token);
    state.next_prepare += 1;
    vec![
        Effect::PrepareChange {
            token,
            gate: Gate::NewOp,
        },
        Effect::Render,
    ]
}

/// Makes the draft's plan again from what the preparation read, with the resolved method.
fn replan(draft: &mut ChangeDraft) {
    let Some(prepared) = &draft.prepared else {
        draft.plan = None;
        return;
    };
    let Some(target) = crate::vm::change::target_instance(&prepared.snapshot, &draft.members)
    else {
        draft.plan = None;
        return;
    };
    let method = draft.resolved_method();
    draft.plan = if draft.restore {
        Some(crate::vm::change::plan_restore(
            &prepared.snapshot,
            &prepared.journal,
            &target,
            method,
        ))
    } else {
        draft.choice.map(|choice| {
            crate::vm::change::plan_change(
                &prepared.snapshot,
                prepared.uncertain_values,
                &target,
                choice,
                method,
                draft.standard,
            )
        })
    };
}

fn change_prepared(
    state: &mut AppState,
    token: u64,
    at: Instant,
    prepared: Result<PreparedChange, PrepareFailure>,
) -> Vec<Effect> {
    let activity = state.activity.clone();
    let Some(draft) = state.draft.as_mut() else {
        return Vec::new();
    };
    // A preparation the user has moved on from (another choice since) is dropped.
    if draft.preparing != Some(token) {
        return Vec::new();
    }
    draft.preparing = None;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => {
            draft.failure = Some(failure);
            draft.plan = None;
            return vec![Effect::Render];
        }
    };
    let blocker = prepared.blocker.clone();
    draft.method_default = Some(crate::vm::change::default_apply_method(
        &activity,
        &draft.members,
        at,
    ));
    draft.prepared = Some(prepared);
    replan(draft);
    if let Some(reason) = blocker {
        // Stopped by the journal: say why, and show no UAC prompt (design m3 B.5).
        state.blocked = Some(reason);
        state.outcome = None;
        state.overlay = OverlayKind::Result;
    }
    vec![Effect::Render]
}

/// How the device made of `members` types in `snapshot` (its first connected member, else the
/// first known one).
fn typed_before(snapshot: Option<&SystemSnapshot>, members: &[String]) -> Option<LayoutTable> {
    let assessment = assess(snapshot?);
    let member = |present: bool| {
        assessment.keyboards.iter().find(|ka| {
            (ka.present || !present)
                && members
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&ka.instance_id))
        })
    };
    member(true)
        .or_else(|| member(false))
        .and_then(|ka| ka.current.as_ref())
        .map(|layout| layout.table.clone())
}

/// The keyboards of the draft's request, with how they type now. "今すぐ反映…" names each
/// device it resets (design m3 B.17 "今の状態").
fn draft_targets(state: &AppState) -> Vec<SessionTarget> {
    let Some(draft) = &state.draft else {
        return Vec::new();
    };
    let snapshot = draft_snapshot(state, draft);
    if draft.apply_now {
        let Some(snapshot) = snapshot else {
            return Vec::new();
        };
        return devices(&assess(snapshot), &draft.members)
            .into_iter()
            .map(|(name, members)| SessionTarget {
                before: typed_before(Some(snapshot), &members),
                name,
                members,
            })
            .collect();
    }
    vec![SessionTarget {
        name: draft.name.clone(),
        members: draft.members.clone(),
        before: typed_before(snapshot, &draft.members),
    }]
}

fn start_change(state: &mut AppState, request: Request, apply: ApplyOptions) -> Vec<Effect> {
    let targets = draft_targets(state);
    start_request(state, request, apply, targets)
}

/// Starts a helper session (one at a time, design m3 A.4).
fn start_request(
    state: &mut AppState,
    request: Request,
    apply: ApplyOptions,
    targets: Vec<SessionTarget>,
) -> Vec<Effect> {
    if state.session != SessionPhase::Idle {
        // One session at a time (design m3 A.4): a second click, or undo from the tray while a
        // UAC prompt is up, does nothing.
        return Vec::new();
    }
    let session = state.next_session;
    state.next_session += 1;
    state.session = SessionPhase::Launching { id: session };
    // The overlay takes over: no capture field stays behind it.
    stop_identifying(state);
    state.overlay = OverlayKind::Progress;
    state.outcome = None;
    state.blocked = None;
    state.note = SessionNote::None;
    state.decided = None;
    state.last_view = None;
    state.result_read_pending = false;
    state.targets = targets;
    state.request = Some(request.clone());
    state.key_test = KeyTest::default();
    vec![
        Effect::StartSession {
            session,
            request,
            apply,
        },
        Effect::Render,
    ]
}

fn session_event(state: &mut AppState, session: SessionId, view: Box<SessionView>) -> Vec<Effect> {
    let overlay = match (&view.prompt, state.overlay) {
        // The quit confirmation stays over a reconnect reminder.
        (Prompt::Reconnect { .. }, OverlayKind::QuitConfirm) => OverlayKind::QuitConfirm,
        // Leaving for later: the relay is about to go; a reminder does not reopen the question.
        (Prompt::Reconnect { .. }, _) if state.note == SessionNote::Leaving => {
            OverlayKind::Progress
        }
        // Answered already: a tick sent before the answer reached the helper does not bring the
        // question back.
        (Prompt::Countdown { op_id, .. } | Prompt::Reconnect { op_id, .. }, _)
            if state.decided.as_ref() == Some(op_id) =>
        {
            OverlayKind::Progress
        }
        (Prompt::Countdown { .. }, _) => OverlayKind::Countdown,
        (Prompt::Reconnect { .. }, _) => OverlayKind::Reconnect,
        (Prompt::None | Prompt::Answered { .. }, _) => OverlayKind::Progress,
    };
    let mut effects = Vec::new();
    // A question opened: bring the window to the front even if the user switched away during
    // the UAC prompt — key tests and Raw Input need the focus (review U10).
    if overlay != state.overlay
        && matches!(overlay, OverlayKind::Countdown | OverlayKind::Reconnect)
    {
        if state.overlay == OverlayKind::Progress {
            state.key_test = KeyTest::default();
        }
        state.visible = true;
        effects.push(Effect::ShowWindow);
    }
    state.overlay = overlay;
    if view.planned {
        if state.targets.is_empty() {
            state.targets = view_targets(state, &view);
        }
        state.last_view = Some(view.clone());
    }
    state.session = SessionPhase::Running { id: session, view };
    effects.push(Effect::Render);
    effects
}

/// The keyboards a journaled operation changes, one per name (for requests that did not come
/// from the change page).
fn view_targets(state: &AppState, view: &SessionView) -> Vec<SessionTarget> {
    let snapshot = state.read.as_ref().and_then(|read| read.snapshot.as_ref());
    let assessment = snapshot.map(assess);
    let mut targets: Vec<SessionTarget> = Vec::new();
    for kb in view.keyboards.iter().filter(|kb| kb.changes) {
        if let Some(target) = targets.iter_mut().find(|t| t.name == kb.display_name) {
            target.members.push(kb.instance_id.clone());
            continue;
        }
        let before = assessment.as_ref().and_then(|assessment| {
            assessment
                .keyboards
                .iter()
                .find(|ka| ka.instance_id.eq_ignore_ascii_case(&kb.instance_id))
                .and_then(|ka| ka.current.as_ref())
                .map(|layout| layout.table.clone())
        });
        targets.push(SessionTarget {
            name: kb.display_name.clone(),
            members: vec![kb.instance_id.clone()],
            before,
        });
    }
    targets
}

fn session_ended(state: &mut AppState, outcome: SessionOutcome) -> Vec<Effect> {
    state.session = SessionPhase::Idle;
    state.recovery_question = None;
    state.decided = None;
    state.note = SessionNote::None;
    state.draft = None;
    if matches!(state.page, Page::Change | Page::UacNotice) {
        state.page = Page::Main;
    }
    let abandoned = matches!(
        outcome.report.first,
        mklm_client::orchestrator::RequestEnd::Ended(
            mklm_client::session::SessionEnd::Abandoned { .. }
        )
    );
    state.outcome = Some(outcome);
    // The RunOnce rule already ran on the worker (design m3 A.2.3).
    let mut effects = if abandoned {
        // "Decide later": nothing to report; the banner shows the waiting change.
        state.overlay = OverlayKind::None;
        state.result_read_pending = false;
        vec![Effect::Read, Effect::Render]
    } else {
        state.overlay = OverlayKind::Result;
        state.result_read_pending = true;
        vec![Effect::ReadForResult, Effect::Render]
    };
    if state.quit_pending {
        effects.push(Effect::Quit);
    }
    effects
}

/// Answers the open question of the running session (design m3 B.6, B.7): once per question,
/// and only while one is open (the relay drops anything else, design m3 A.2.2).
fn answer(state: &mut AppState, keep: bool) -> Vec<Effect> {
    let Some(view) = session_view(state) else {
        return Vec::new();
    };
    let op_id = match &view.prompt {
        Prompt::Countdown { op_id, .. } | Prompt::Reconnect { op_id, .. } => op_id.clone(),
        Prompt::None | Prompt::Answered { .. } => return Vec::new(),
    };
    if state.decided.as_ref() == Some(&op_id) {
        return Vec::new();
    }
    let decision = if keep {
        Decision::Keep {
            op_id: op_id.clone(),
        }
    } else {
        Decision::RevertNow {
            op_id: op_id.clone(),
        }
    };
    state.decided = Some(op_id);
    state.note = if keep {
        SessionNote::Keeping
    } else {
        SessionNote::Reverting
    };
    state.overlay = OverlayKind::Progress;
    vec![Effect::SendDecision(decision), Effect::Render]
}

/// A key in the key test, judged against the open question's expectation (review U2).
fn key_test_pressed(state: &mut AppState, text: &str, shift: bool) -> Vec<Effect> {
    let lang = state.lang.unwrap_or(Lang::Ja);
    let source_name = state
        .last_key
        .as_ref()
        .map(|(id, _)| display_name(state, id));
    // WP-U4: the post-reboot check passes its row's expectation the same way.
    let expectation = session_view(state).and_then(keytest::session_expectation);
    let expected = expectation.as_ref().map(|(targets, name, table)| Expected {
        targets,
        name,
        table,
    });
    let context = KeyContext {
        source: state
            .last_key
            .as_ref()
            .map(|(id, code)| (id.as_str(), *code)),
        source_name: source_name.as_deref(),
        active_hkl: state.active_hkl,
        expected,
    };
    match keytest::key_pressed(text, shift, &context, lang) {
        Some(test) => {
            state.key_test = test;
            vec![Effect::Render]
        }
        None => Vec::new(),
    }
}

/// A Raw Input key press: remember it (seen, recent activity, physical hints) and render only
/// when something visible changed — a render on every key would rebuild rows and drop the focus
/// (review A7).
fn device_key(
    state: &mut AppState,
    instance_id: String,
    scancode: u32,
    at: Instant,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    let mut visible_change = false;
    state.activity.key(&instance_id, at);
    let mut save = false;
    if !state.settings.was_seen(&instance_id) {
        state.settings.keyboards.seen.push(instance_id.clone());
        save = true;
        visible_change = true; // the "キー入力なし" badge goes away
    }
    if crate::detect::is_jis_only_key(scancode)
        && state.settings.learn_physical(PhysicalLayout {
            id: instance_id.clone(),
            layout: PhysicalKind::Jis,
            source: PhysicalSource::JisOnlyKey,
        })
    {
        save = true;
        visible_change = true; // the row may now say "実物は JIS 配列です…"
    }
    let detecting = state.session == SessionPhase::Idle && state.page == Page::Change;
    let mut learned = None;
    if detecting
        && let Some(draft) = state.draft.as_mut()
        && !draft.restore
        && !draft.apply_now
    {
        let before = (draft.detection.clone(), draft.other_keyboard.clone());
        let press = draft.detection.press(&instance_id, scancode);
        draft.last_press = Some(press);
        let answer_key = matches!(
            scancode,
            crate::detect::scancode::YEN
                | crate::detect::scancode::EQUAL
                | crate::detect::scancode::RO
                | crate::detect::scancode::SLASH
        );
        match press {
            // Only an answer key on the wrong keyboard is worth a word (not Tab or a letter).
            Press::OtherKeyboard if answer_key => draft.other_keyboard = Some(instance_id.clone()),
            Press::Counted => draft.other_keyboard = None,
            Press::OtherKeyboard | Press::NotAnAnswer | Press::Finished => {}
        }
        if press == Press::Counted
            && let (Some(device), Some(verdict)) =
                (draft.detection.device.clone(), draft.detection.verdict())
        {
            learned = match verdict {
                Verdict::Jis => Some((device, PhysicalKind::Jis)),
                Verdict::Us => Some((device, PhysicalKind::Us)),
                Verdict::Mixed => None,
            };
        }
        if (draft.detection.clone(), draft.other_keyboard.clone()) != before {
            visible_change = true;
        }
    }
    // The detection's verdict is what the keyboard physically is (design m3 B.3).
    if let Some((id, layout)) = learned
        && state.settings.learn_physical(PhysicalLayout {
            id,
            layout,
            source: PhysicalSource::Detection,
        })
    {
        save = true;
    }
    if state.identify
        && state
            .highlighted
            .as_deref()
            .is_none_or(|highlighted| !highlighted.eq_ignore_ascii_case(&instance_id))
    {
        state.highlighted = Some(instance_id.clone());
        visible_change = true;
    }
    state.last_key = Some((instance_id, scancode));
    if save {
        effects.push(Effect::SaveSettings(Box::new(state.settings.clone())));
    }
    if visible_change {
        effects.push(Effect::Render);
    }
    effects
}

/// The window's ×: to the tray, the first time through the in-window notice (review U13);
/// during a countdown it reverts (design m3 F.5); during other session phases the window stays.
fn close_requested(state: &mut AppState) -> Vec<Effect> {
    match &state.session {
        SessionPhase::Running { view, .. } => match &view.prompt {
            Prompt::Countdown { .. } => answer(state, false),
            _ => Vec::new(),
        },
        SessionPhase::Launching { .. } => Vec::new(),
        SessionPhase::Idle if !state.settings.tray.close_notice_shown => {
            state.overlay = OverlayKind::CloseNotice;
            vec![Effect::Render]
        }
        SessionPhase::Idle => {
            state.visible = false;
            vec![Effect::HideWindow]
        }
    }
}

/// Quitting (tray, `quit`): at once without a connected helper; a reconnect wait asks first;
/// otherwise after the result (a countdown is reverted by the relay, design m3 F.5).
fn quit_requested(state: &mut AppState) -> Vec<Effect> {
    match &state.session {
        SessionPhase::Idle => vec![Effect::Quit],
        // Nothing is connected yet: leave now (design m3 F.5 "UAC 待ち").
        SessionPhase::Launching { .. } => vec![Effect::CancelSession, Effect::Quit],
        SessionPhase::Running { view, .. } => {
            if matches!(view.prompt, Prompt::Reconnect { .. })
                && state.decided.is_none()
                && !state.quit_pending
            {
                state.overlay = OverlayKind::QuitConfirm;
                state.visible = true;
                return vec![Effect::ShowWindow, Effect::Render];
            }
            let countdown = matches!(view.prompt, Prompt::Countdown { .. });
            state.quit_pending = true;
            if countdown && state.decided.is_none() {
                state.note = SessionNote::Reverting;
                state.overlay = OverlayKind::Progress;
                return vec![Effect::CancelSession, Effect::Render];
            }
            vec![Effect::CancelSession]
        }
    }
}

fn quit_confirm_answered(state: &mut AppState, answer_: QuitAnswer) -> Vec<Effect> {
    if state.overlay != OverlayKind::QuitConfirm {
        return Vec::new();
    }
    let reconnect =
        session_view(state).is_some_and(|view| matches!(view.prompt, Prompt::Reconnect { .. }));
    match answer_ {
        QuitAnswer::Cancel => {
            state.overlay = if reconnect {
                OverlayKind::Reconnect
            } else {
                OverlayKind::Progress
            };
            vec![Effect::Render]
        }
        QuitAnswer::RevertAndQuit => {
            state.quit_pending = true;
            let effects = answer(state, false);
            if effects.is_empty() {
                // The question closed meanwhile: leave as soon as the result is in.
                state.overlay = OverlayKind::Progress;
                return vec![Effect::CancelSession, Effect::Render];
            }
            effects
        }
        QuitAnswer::QuitLeavingIt => {
            state.quit_pending = true;
            state.note = SessionNote::Leaving;
            state.overlay = OverlayKind::Progress;
            vec![Effect::CancelSession, Effect::Render]
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mklm_client::gate::OpRef;
    use mklm_client::orchestrator::{LaunchError, RecoverySkip, RequestEnd};
    use mklm_client::session::SessionEnd;
    use mklm_core::{
        ExpectedKeyboard, KeyboardType, OpKind, OpState, OperationResult, Outcome, PendingAction,
        fixtures,
    };

    use super::*;
    use crate::vm::SnapshotText;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const KEYCHRON_ROW: &str = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
    const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
    const JAPANESE: u32 = 0x0411_0411;

    fn key(state: &mut AppState, scancode: u32) -> Vec<Effect> {
        key_from(state, KEYCHRON, scancode)
    }

    fn key_from(state: &mut AppState, instance_id: &str, scancode: u32) -> Vec<Effect> {
        update(
            state,
            AppMsg::DeviceKey {
                instance_id: instance_id.into(),
                scancode,
                at: Instant::now(),
            },
        )
    }

    fn undo() -> AppMsg {
        AppMsg::StartRequest {
            request: Request::Undo {
                apply: ApplyOptions::default(),
            },
            apply: ApplyOptions::default(),
        }
    }

    fn ended(session: SessionId, end: SessionEnd) -> AppMsg {
        ended_with(
            session,
            RequestReport {
                first: RequestEnd::Ended(end),
                lost_needs_recovery: None,
                recovery: None,
                recovery_skipped: None,
            },
        )
    }

    fn ended_with(session: SessionId, report: RequestReport) -> AppMsg {
        AppMsg::SessionEnded {
            session,
            outcome: Box::new(SessionOutcome {
                report,
                run_once: Ok(RunOnceOutcome::NotNeeded),
            }),
        }
    }

    #[test]
    fn identify_highlights_the_keyboard_that_typed_and_marks_it_seen() {
        let mut state = AppState::default();
        update(&mut state, AppMsg::ToggleIdentify);
        let effects = key(&mut state, 0x1E);
        assert_eq!(state.highlighted.as_deref(), Some(KEYCHRON));
        assert!(matches!(effects[0], Effect::SaveSettings(_)));
        assert_eq!(effects.last(), Some(&Effect::Render));
        // The same keyboard again: nothing visible changed, so no render — the focus stays
        // where it is (review A7).
        assert!(key(&mut state, 0x1E).is_empty());
        update(&mut state, AppMsg::ToggleIdentify);
        assert_eq!(state.highlighted, None);
        assert!(key(&mut state, 0x1F).is_empty());
    }

    #[test]
    fn a_jis_only_key_teaches_the_physical_layout() {
        let mut state = AppState::default();
        key(&mut state, 0x1E);
        let effects = key(&mut state, crate::detect::scancode::RO);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveSettings(_), Effect::Render]
        ));
        // Known already: nothing to save or show.
        assert!(key(&mut state, crate::detect::scancode::RO).is_empty());
        assert_eq!(
            state.settings.physical(KEYCHRON).map(|p| p.layout),
            Some(crate::settings::PhysicalKind::Jis)
        );
    }

    #[test]
    fn one_session_at_a_time_and_stale_messages_are_dropped() {
        let mut state = AppState::default();
        let effects = update(&mut state, undo());
        assert!(matches!(
            effects[0],
            Effect::StartSession { session: 0, .. }
        ));
        assert_eq!(state.session, SessionPhase::Launching { id: 0 });
        // A second start while the UAC prompt is up is refused (review A2 b).
        assert!(update(&mut state, undo()).is_empty());
        // A message of another session changes nothing.
        assert!(update(&mut state, ended(7, SessionEnd::Unresponsive)).is_empty());
        assert_eq!(state.session, SessionPhase::Launching { id: 0 });
        update(
            &mut state,
            AppMsg::SessionNotice {
                session: 0,
                notice: Notice::Connected(SessionKind::Request),
            },
        );
        assert!(matches!(state.session, SessionPhase::Running { id: 0, .. }));
        update(&mut state, ended(0, SessionEnd::Unresponsive));
        assert_eq!(state.session, SessionPhase::Idle);
        assert_eq!(state.overlay, OverlayKind::Result);
        // The next session gets a new id.
        assert!(matches!(
            update(&mut state, undo())[0],
            Effect::StartSession { session: 1, .. }
        ));
    }

    #[test]
    fn quitting_during_the_uac_prompt_is_immediate() {
        // Review A2 a: nothing is connected, so there is nothing to wait for.
        let mut state = AppState::default();
        update(&mut state, undo());
        assert_eq!(
            update(&mut state, AppMsg::QuitRequested),
            vec![Effect::CancelSession, Effect::Quit]
        );
    }

    #[test]
    fn quitting_waits_for_a_running_session() {
        let mut state = AppState {
            session: SessionPhase::Running {
                id: 3,
                view: Box::default(),
            },
            ..AppState::default()
        };
        assert_eq!(
            update(&mut state, AppMsg::QuitRequested),
            vec![Effect::CancelSession]
        );
        let effects = update(
            &mut state,
            ended(3, SessionEnd::Abandoned { planned: false }),
        );
        assert_eq!(effects.last(), Some(&Effect::Quit));
        assert_eq!(state.overlay, OverlayKind::None);
    }

    #[test]
    fn the_first_close_shows_the_notice_and_hides_only_on_ok() {
        // Review U13: no balloon; the notice is in the window, the window hides on OK.
        let mut state = AppState {
            visible: true,
            ..AppState::default()
        };
        assert_eq!(
            update(&mut state, AppMsg::WindowCloseRequested),
            vec![Effect::Render]
        );
        assert_eq!(state.overlay, OverlayKind::CloseNotice);
        assert!(state.visible);
        let effects = update(&mut state, AppMsg::CloseNoticeAnswered { quit: false });
        assert!(matches!(effects[0], Effect::SaveSettings(_)));
        assert_eq!(effects[1], Effect::HideWindow);
        assert!(state.settings.tray.close_notice_shown && !state.visible);
        // Later closes go straight to the tray.
        state.visible = true;
        assert_eq!(
            update(&mut state, AppMsg::WindowCloseRequested),
            vec![Effect::HideWindow]
        );
    }

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    #[test]
    fn closing_the_window_during_a_countdown_reverts() {
        let op_id = op();
        let mut view = SessionView::default();
        view.observe(&Event::CountdownStarted {
            op_id: op_id.clone(),
            seconds: 20,
            verified: true,
        });
        let mut state = AppState {
            session: SessionPhase::Running {
                id: 0,
                view: Box::new(view),
            },
            overlay: OverlayKind::Countdown,
            ..AppState::default()
        };
        assert_eq!(
            update(&mut state, AppMsg::WindowCloseRequested),
            vec![
                Effect::SendDecision(Decision::RevertNow { op_id }),
                Effect::Render
            ]
        );
        // The dialog now waits for the result (design m3 B.18).
        assert_eq!(state.overlay, OverlayKind::Progress);
        assert_eq!(progress_stage(&state), Stage::Answered { keep: false });
        // A second × does not send it again.
        assert!(update(&mut state, AppMsg::WindowCloseRequested).is_empty());
    }

    #[test]
    fn the_recovery_question_is_asked_and_answered_once() {
        let mut state = AppState::default();
        update(&mut state, undo());
        update(
            &mut state,
            AppMsg::RecoveryQuestion {
                session: 0,
                countdown: true,
            },
        );
        assert_eq!(state.overlay, OverlayKind::RecoveryConfirm);
        assert_eq!(
            update(&mut state, AppMsg::AnswerRecovery(false)),
            vec![Effect::AnswerRecovery(false), Effect::Render]
        );
        assert!(update(&mut state, AppMsg::AnswerRecovery(true)).is_empty());
    }

    // --- The change flow with a fake helper (WP-U3) ---

    /// The development machine as the display read and the planning inventory see it.
    fn read() -> SystemRead {
        SystemRead {
            snapshot: Some(fixtures::dev_machine()),
            warnings: Vec::new(),
            journal: Some(Journal::default()),
            boot: None,
            summary: StartupSummary::default(),
        }
    }

    fn prepared() -> PreparedChange {
        PreparedChange {
            snapshot: fixtures::dev_machine(),
            uncertain_values: false,
            warnings: Vec::new(),
            journal: Journal::default(),
            blocker: None,
        }
    }

    fn ready_state() -> AppState {
        AppState {
            lang: Some(Lang::Ja),
            read: Some(read()),
            active_hkl: JAPANESE,
            visible: true,
            ..AppState::default()
        }
    }

    // --- The main screen (WP-U1) ---

    const BOOT: &str = "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d";

    /// A read of the dev machine with the Keychron stored as JIS, still typing US, and a
    /// schema-1 journal entry in `state` whose `apply_pending` asks for its reset.
    fn read_with(state: &str) -> SystemRead {
        let mut snapshot = mklm_core::fixtures::dev_machine();
        snapshot.keyboards[1].overrides.keyboard_type_override = Some(7);
        snapshot.keyboards[1].overrides.keyboard_subtype_override = Some(2);
        let json = format!(
            r#"{{ "schema_version": 1, "op_id": "0000000a-0000-4000-8000-000000000000",
              "seq": 1,
              "kind": {{ "kind": "set-layout",
                "requested": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000",
                "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
                "layout": "jis" }},
              "state": "{state}", "boot_id": "{BOOT}",
              "owner": {{ "pid": 12345, "creation_time": 134036790000000000 }},
              "created_at": 1, "updated_at": 2, "apply": "reset-keyboard", "countdown": null,
              "records": [], "context": [], "failure": null,
              "apply_pending": {{ "action": "reset-keyboard",
                "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
                "since": "{BOOT}" }},
              "history": [] }}"#
        );
        let journal = Journal {
            entries: vec![mklm_core::JournalEntry::from_json(&json).unwrap()],
            ..Journal::default()
        };
        let boot = BootId::parse(BOOT).unwrap();
        let summary =
            mklm_client::startup::summarize(&journal, boot, &|_| mklm_core::Liveness::Dead);
        SystemRead {
            snapshot: Some(snapshot),
            warnings: Vec::new(),
            journal: Some(journal),
            boot: Some(boot),
            summary,
        }
    }

    fn with_read(read: SystemRead) -> AppState {
        AppState {
            read: Some(read),
            visible: true,
            ..AppState::default()
        }
    }

    /// Plays the helper's part: every event goes through `SessionView::observe` first, as the
    /// relay does, then reaches the UI thread as `AppMsg::SessionEvent`.
    struct FakeHelper {
        session: SessionId,
        view: SessionView,
    }

    impl FakeHelper {
        fn connect(state: &mut AppState, session: SessionId) -> Self {
            for notice in [
                Notice::Starting(SessionKind::Request),
                Notice::Connected(SessionKind::Request),
            ] {
                update(state, AppMsg::SessionNotice { session, notice });
            }
            Self {
                session,
                view: SessionView::default(),
            }
        }

        fn send(&mut self, state: &mut AppState, event: Event) -> Vec<Effect> {
            self.view.observe(&event);
            update(
                state,
                AppMsg::SessionEvent {
                    session: self.session,
                    event: Box::new(event),
                    view: Box::new(self.view.clone()),
                },
            )
        }

        /// A USB set up to its countdown (design m2 D.2 a).
        fn live_reset(&mut self, state: &mut AppState, layout: LayoutTable) -> Vec<Effect> {
            let expected_type = if layout == LayoutTable::Jis {
                KeyboardType::JIS
            } else {
                KeyboardType::US
            };
            let mut effects = Vec::new();
            for event in [
                Event::Locked,
                Event::Planned {
                    op_id: op(),
                    steps: Vec::new(),
                    apply: Some(PendingAction::ResetKeyboard),
                    keyboards: vec![ExpectedKeyboard {
                        instance_id: KEYCHRON.into(),
                        display_name: "Keychron Receiver".into(),
                        expected_type: Some(expected_type),
                        layout_after: Some(layout),
                        changes: true,
                    }],
                },
                Event::StateChanged {
                    op_id: op(),
                    state: OpState::Written,
                },
                Event::StepWritten {
                    op_id: op(),
                    step: 1,
                    of: 1,
                },
                Event::ResettingKeyboard {
                    instance_id: KEYCHRON.into(),
                },
                Event::KeyboardArrived {
                    instance_id: KEYCHRON.into(),
                    reported: Some(expected_type),
                    expected: expected_type,
                },
                Event::CountdownStarted {
                    op_id: op(),
                    seconds: 20,
                    verified: true,
                },
            ] {
                effects = self.send(state, event);
            }
            effects
        }

        fn tick(&mut self, state: &mut AppState, remaining: u32) -> Vec<Effect> {
            self.send(
                state,
                Event::CountdownTick {
                    op_id: op(),
                    remaining,
                },
            )
        }
    }

    fn result(outcome: Outcome, failure: Option<mklm_core::FailureReason>) -> OperationResult {
        OperationResult {
            op_id: Some(op()),
            outcome,
            failure,
            pending_action: None,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// "変更…" on the Keychron, JIS chosen and prepared; returns the effects of the answer.
    fn choose_jis(state: &mut AppState, at: Instant) -> Vec<Effect> {
        assert_eq!(
            update(state, AppMsg::OpenChange(KEYCHRON_ROW.into())),
            vec![Effect::Render]
        );
        assert_eq!(state.page, Page::Change);
        let effects = update(state, AppMsg::ChangeChoose(0));
        let token = match effects.as_slice() {
            [
                Effect::PrepareChange {
                    token,
                    gate: Gate::NewOp,
                },
                Effect::Render,
            ] => *token,
            other => panic!("{other:?}"),
        };
        update(
            state,
            AppMsg::ChangePrepared {
                token,
                at,
                prepared: Box::new(Ok(prepared())),
            },
        )
    }

    fn page(state: &AppState) -> crate::vm::change::ChangePage {
        let draft = state.draft.as_ref().unwrap();
        crate::vm::change::change_page(&crate::vm::change::ChangeContext {
            draft,
            display: state.read.as_ref().and_then(|read| read.snapshot.as_ref()),
            uac_notice_seen: state.settings.change.uac_notice_seen,
            elevated: state.elevated,
            seconds: 20,
            other_name: None,
            lang: Lang::Ja,
        })
    }

    #[test]
    fn a_change_from_the_page_to_the_result() {
        // T-APPLY-1 without the hardware: Keychron to JIS, a mouse user (switch now), the
        // first-time UAC explanation, the countdown, "keep", the result and the state now.
        let mut state = ready_state();
        let now = Instant::now();
        update(&mut state, AppMsg::PointerUsed(now));
        choose_jis(&mut state, now + Duration::from_secs(5));
        let shown = page(&state);
        assert!(shown.can_apply && shown.method_visible);
        assert_eq!(shown.method, Some(ApplyMethod::Live));
        assert!(!navigation_enabled(&state));
        // The first change: the explanation page comes before any prompt (M2 R12).
        assert_eq!(
            update(&mut state, AppMsg::ChangeApply),
            vec![Effect::Render]
        );
        assert_eq!(state.page, Page::UacNotice);
        let effects = update(&mut state, AppMsg::UacGo);
        assert!(matches!(effects[0], Effect::SaveSettings(ref s) if s.change.uac_notice_seen));
        let Effect::StartSession {
            session,
            request: Request::SetLayout(set),
            apply,
        } = &effects[1]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(set.instance_id, KEYCHRON);
        assert_eq!(set.layout, LayoutChoice::Jis);
        assert_eq!(*apply, ApplyMethod::Live.options());
        assert_eq!(
            set.expected.as_ref().unwrap().apply,
            Some(PendingAction::ResetKeyboard)
        );
        let session = *session;
        assert_eq!(state.overlay, OverlayKind::Progress);
        assert_eq!(progress_stage(&state), Stage::Launching);
        // Nothing else starts while the prompt is up.
        assert!(update(&mut state, AppMsg::ChangeApply).is_empty());

        let mut helper = FakeHelper::connect(&mut state, session);
        assert_eq!(progress_stage(&state), Stage::Running);
        // The countdown opens: the window comes to the front (review U10).
        state.visible = false;
        let effects = helper.live_reset(&mut state, LayoutTable::Jis);
        assert_eq!(state.overlay, OverlayKind::Countdown);
        assert_eq!(effects, vec![Effect::ShowWindow, Effect::Render]);
        assert!(state.visible);
        // Shift+2 on the Keychron: judged against JIS (review U2).
        key(&mut state, keytest::SCANCODE_DIGIT2);
        update(
            &mut state,
            AppMsg::KeyTestPressed {
                text: "\"".into(),
                shift: true,
            },
        );
        assert_eq!(
            state.key_test.verdict,
            "Shift+2 → \" : ✓ 期待どおり JIS です"
        );
        // The built-in keyboard: which keyboard to use instead.
        key_from(&mut state, BUILT_IN, keytest::SCANCODE_DIGIT2);
        update(
            &mut state,
            AppMsg::KeyTestPressed {
                text: "\"".into(),
                shift: true,
            },
        );
        assert!(
            state
                .key_test
                .verdict
                .ends_with("Keychron Receiver で押してください"),
            "{}",
            state.key_test.verdict
        );
        helper.tick(&mut state, 10);
        let countdown =
            crate::vm::session::countdown(session_view(&state).unwrap(), Lang::Ja).unwrap();
        assert_eq!(countdown.reminder, "あと 10 秒で元に戻ります");
        // Keep: sent once; the dialog waits for the result.
        assert_eq!(
            update(&mut state, AppMsg::KeepChange),
            vec![
                Effect::SendDecision(Decision::Keep { op_id: op() }),
                Effect::Render
            ]
        );
        assert_eq!(state.overlay, OverlayKind::Progress);
        assert!(update(&mut state, AppMsg::KeepChange).is_empty());
        // A tick sent before the answer reached the helper does not reopen the question.
        helper.tick(&mut state, 9);
        assert_eq!(state.overlay, OverlayKind::Progress);
        assert_eq!(progress_stage(&state), Stage::Answered { keep: true });

        let effects = update(
            &mut state,
            ended_with(
                session,
                RequestReport {
                    first: RequestEnd::Ended(SessionEnd::Finished(result(
                        Outcome::Confirmed,
                        None,
                    ))),
                    lost_needs_recovery: None,
                    recovery: None,
                    recovery_skipped: None,
                },
            ),
        );
        assert_eq!(effects, vec![Effect::ReadForResult, Effect::Render]);
        assert_eq!(state.overlay, OverlayKind::Result);
        assert_eq!(state.page, Page::Main);
        assert!(state.draft.is_none());
        // Until the read after the session arrives, "今の状態" says so (review U7).
        let waiting = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(waiting.current_state, vec!["確かめています…".to_string()]);
        let mut after = read();
        after.snapshot.as_mut().unwrap().keyboards[1].reported_type = Some(KeyboardType::JIS);
        update(&mut state, AppMsg::SystemRead(Box::new(after)));
        assert_eq!(
            update(&mut state, AppMsg::ResultReadArrived),
            vec![Effect::Render]
        );
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(
            shown.snapshot_text(),
            "title: 完了しました\n\
             tone: Success\n\
             now: Keychron Receiver: JIS として動作中\n\
             message: 新しい配列のままにしました。\n"
        );
        // "詳細をコピー": English diagnostics only.
        let copy = update(&mut state, AppMsg::CopyDetails);
        assert!(matches!(&copy[..], [Effect::CopyText(text)] if text.contains("3f2a9c1e")));
        assert_eq!(
            update(&mut state, AppMsg::ResultClosed),
            vec![Effect::Read, Effect::Render]
        );
        assert_eq!((state.page, state.overlay), (Page::Main, OverlayKind::None));
        assert!(navigation_enabled(&state));

        // The second change: no explanation page; the one line and the button say it.
        choose_jis(&mut state, Instant::now());
        assert!(!page(&state).uac_line.is_empty());
        let effects = update(&mut state, AppMsg::ChangeApply);
        assert!(matches!(
            effects[0],
            Effect::StartSession { session: 1, .. }
        ));
    }

    #[test]
    fn a_keyboard_only_user_of_the_target_switches_at_the_restart() {
        // T-APPLY-9: only the Keychron typed; the default is the restart, with the warning,
        // and the request carries the restart's options.
        let mut state = ready_state();
        state.settings.change.uac_notice_seen = true;
        let now = Instant::now();
        key(&mut state, 0x1E);
        choose_jis(&mut state, now);
        let shown = page(&state);
        assert_eq!(shown.method, Some(ApplyMethod::Restart));
        assert_eq!(shown.warnings.len(), 1);
        // The user picks "switch now" after all: the plan is made again (design m2 S6).
        update(&mut state, AppMsg::ChangeChooseMethod(0));
        let shown = page(&state);
        assert_eq!(shown.method, Some(ApplyMethod::Live));
        assert!(shown.warnings.is_empty());
        let effects = update(&mut state, AppMsg::ChangeApply);
        let Effect::StartSession { apply, .. } = &effects[0] else {
            panic!("{effects:?}")
        };
        assert!(apply.allow_live_reset && apply.other_input_available);
    }

    #[test]
    fn a_stale_preparation_is_dropped_and_the_detection_selects() {
        let mut state = ready_state();
        update(&mut state, AppMsg::OpenChange(KEYCHRON.into()));
        let first = update(&mut state, AppMsg::ChangeChoose(0));
        let second = update(&mut state, AppMsg::ChangeChoose(1));
        let token = |effects: &[Effect]| match effects[0] {
            Effect::PrepareChange { token, .. } => token,
            _ => panic!(),
        };
        // The answer to the first choice arrives late: ignored.
        assert!(
            update(
                &mut state,
                AppMsg::ChangePrepared {
                    token: token(&first),
                    at: Instant::now(),
                    prepared: Box::new(Ok(prepared())),
                },
            )
            .is_empty()
        );
        assert!(page(&state).preparing);
        update(
            &mut state,
            AppMsg::ChangePrepared {
                token: token(&second),
                at: Instant::now(),
                prepared: Box::new(Ok(prepared())),
            },
        );
        // US is what it is set to: nothing to change, no prompt.
        assert!(!page(&state).can_apply);
        // The detection on the page: the built-in keyboard's answer key is not counted.
        let effects = key_from(&mut state, BUILT_IN, crate::detect::scancode::YEN);
        assert!(effects.contains(&Effect::Render));
        assert_eq!(
            state.draft.as_ref().unwrap().other_keyboard.as_deref(),
            Some(BUILT_IN)
        );
        // A Tab on the Keychron changes nothing visible (no render: the focus stays).
        key(&mut state, 0x1E);
        assert!(key(&mut state, 0x0F).is_empty());
        key(&mut state, crate::detect::scancode::YEN);
        let effects = key(&mut state, crate::detect::scancode::RO);
        // JIS found: learned for the keyboard, saved.
        assert!(matches!(effects[0], Effect::SaveSettings(_)));
        assert_eq!(
            state
                .settings
                .physical(KEYCHRON)
                .map(|p| (p.layout, p.source)),
            Some((PhysicalKind::Jis, PhysicalSource::Detection))
        );
        let effects = update(&mut state, AppMsg::ChangeUseDetected);
        assert!(matches!(effects[0], Effect::PrepareChange { .. }));
        assert_eq!(
            state.draft.as_ref().unwrap().choice,
            Some(LayoutChoice::Jis)
        );
    }

    #[test]
    fn a_blocked_change_shows_why_and_starts_nothing() {
        let mut state = ready_state();
        update(&mut state, AppMsg::OpenChange(KEYCHRON_ROW.into()));
        let effects = update(&mut state, AppMsg::ChangeChoose(0));
        let Effect::PrepareChange { token, .. } = effects[0] else {
            panic!()
        };
        let reason = BlockReason::WaitingForReboot(OpRef {
            op_id: op(),
            kind: OpKind::SetLayout {
                requested: KEYCHRON.into(),
                instance_ids: vec![KEYCHRON.into()],
                layout: LayoutChoice::Us,
            },
            state: OpState::PendingReboot,
        });
        update(
            &mut state,
            AppMsg::ChangePrepared {
                token,
                at: Instant::now(),
                prepared: Box::new(Ok(PreparedChange {
                    blocker: Some(reason),
                    ..prepared()
                })),
            },
        );
        assert_eq!(state.overlay, OverlayKind::Result);
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(shown.next, Some(crate::vm::result::NextStep::Restart));
        assert!(
            shown
                .message
                .starts_with("PC の再起動を待っている変更があります（Keychron Receiver を US）")
        );
        // The next step leaves the change: the restart page.
        let effects = update(&mut state, AppMsg::ResultNextStep);
        assert!(effects.contains(&Effect::Read));
        assert_eq!(
            (state.page, state.overlay),
            (Page::Restart, OverlayKind::None)
        );
        assert!(state.draft.is_none() && state.session == SessionPhase::Idle);
    }

    #[test]
    fn an_unreadable_inventory_is_said_on_the_page() {
        let mut state = ready_state();
        update(&mut state, AppMsg::OpenChange(KEYCHRON_ROW.into()));
        let Effect::PrepareChange { token, .. } = update(&mut state, AppMsg::ChangeChoose(0))[0]
        else {
            panic!()
        };
        update(
            &mut state,
            AppMsg::ChangePrepared {
                token,
                at: Instant::now(),
                prepared: Box::new(Err(PrepareFailure::Incomplete("no driver".into()))),
            },
        );
        let shown = page(&state);
        assert!(!shown.can_apply);
        assert!(
            shown
                .note
                .starts_with("Windows がキーボードの情報をすべて返さなかった")
        );
        assert!(update(&mut state, AppMsg::ChangeApply).is_empty());
        // Choosing again reads again.
        assert!(matches!(
            update(&mut state, AppMsg::ChangeChoose(0))[0],
            Effect::PrepareChange { .. }
        ));
        // Cancel: back to the main screen, nothing started.
        assert_eq!(
            update(&mut state, AppMsg::CancelChange),
            vec![Effect::Read, Effect::Render]
        );
        assert_eq!(state.page, Page::Main);
        assert!(state.draft.is_none());
    }

    #[test]
    fn the_uac_explanation_can_be_left_back_to_the_page() {
        let mut state = ready_state();
        choose_jis(&mut state, Instant::now());
        update(&mut state, AppMsg::ChangeApply);
        assert_eq!(state.page, Page::UacNotice);
        assert_eq!(
            update(&mut state, AppMsg::CancelChange),
            vec![Effect::Render]
        );
        assert_eq!(state.page, Page::Change);
        assert!(state.draft.is_some() && !state.settings.change.uac_notice_seen);
        // Elevated: no explanation, no prompt.
        state.elevated = true;
        assert!(matches!(
            update(&mut state, AppMsg::ChangeApply)[0],
            Effect::StartSession { .. }
        ));
    }

    fn reconnecting(state: &mut AppState) -> FakeHelper {
        update(state, undo());
        let mut helper = FakeHelper::connect(state, 0);
        helper.send(
            state,
            Event::Planned {
                op_id: op(),
                steps: Vec::new(),
                apply: Some(PendingAction::Reconnect),
                keyboards: vec![ExpectedKeyboard {
                    instance_id: KEYCHRON.into(),
                    display_name: "Keychron Receiver".into(),
                    expected_type: Some(KeyboardType::JIS),
                    layout_after: Some(LayoutTable::Jis),
                    changes: true,
                }],
            },
        );
        helper.send(
            state,
            Event::WaitingForReconnect {
                op_id: op(),
                instance_ids: vec![KEYCHRON.into()],
            },
        );
        assert_eq!(state.overlay, OverlayKind::Reconnect);
        helper
    }

    #[test]
    fn deciding_later_leaves_the_change_waiting() {
        // Design m3 B.7: no automatic revert; the banner offers the decision later.
        let mut state = ready_state();
        let mut helper = reconnecting(&mut state);
        assert_eq!(
            update(&mut state, AppMsg::DecideLater),
            vec![Effect::CancelSession, Effect::Render]
        );
        assert_eq!(progress_stage(&state), Stage::Leaving);
        // A reminder before the relay leaves keeps the progress up.
        helper.send(
            &mut state,
            Event::WaitingForReconnect {
                op_id: op(),
                instance_ids: vec![KEYCHRON.into()],
            },
        );
        assert_eq!(state.overlay, OverlayKind::Progress);
        let effects = update(
            &mut state,
            ended(0, SessionEnd::Abandoned { planned: true }),
        );
        assert_eq!(effects, vec![Effect::Read, Effect::Render]);
        assert_eq!((state.page, state.overlay), (Page::Main, OverlayKind::None));
    }

    #[test]
    fn quitting_during_a_reconnect_asks_first() {
        let mut state = ready_state();
        reconnecting(&mut state);
        assert_eq!(
            update(&mut state, AppMsg::QuitRequested),
            vec![Effect::ShowWindow, Effect::Render]
        );
        assert_eq!(state.overlay, OverlayKind::QuitConfirm);
        // Cancel: back to the question.
        update(&mut state, AppMsg::QuitConfirmAnswered(QuitAnswer::Cancel));
        assert_eq!(state.overlay, OverlayKind::Reconnect);
        update(&mut state, AppMsg::QuitRequested);
        assert_eq!(
            update(
                &mut state,
                AppMsg::QuitConfirmAnswered(QuitAnswer::RevertAndQuit)
            ),
            vec![
                Effect::SendDecision(Decision::RevertNow { op_id: op() }),
                Effect::Render
            ]
        );
        assert!(state.quit_pending);
        let effects = update(
            &mut state,
            ended(0, SessionEnd::Finished(result(Outcome::Reverted, None))),
        );
        assert_eq!(effects.last(), Some(&Effect::Quit));
        // "Quit and decide later" leaves the change waiting and quits after the relay left.
        let mut state = ready_state();
        reconnecting(&mut state);
        update(&mut state, AppMsg::QuitRequested);
        assert_eq!(
            update(
                &mut state,
                AppMsg::QuitConfirmAnswered(QuitAnswer::QuitLeavingIt)
            ),
            vec![Effect::CancelSession, Effect::Render]
        );
        let effects = update(
            &mut state,
            ended(0, SessionEnd::Abandoned { planned: true }),
        );
        assert_eq!(effects.last(), Some(&Effect::Quit));
    }

    #[test]
    fn quitting_during_a_countdown_reverts_then_quits() {
        let mut state = ready_state();
        update(&mut state, undo());
        let mut helper = FakeHelper::connect(&mut state, 0);
        helper.live_reset(&mut state, LayoutTable::Jis);
        assert_eq!(
            update(&mut state, AppMsg::QuitRequested),
            vec![Effect::CancelSession, Effect::Render]
        );
        assert_eq!(progress_stage(&state), Stage::Answered { keep: false });
        let effects = update(
            &mut state,
            ended(
                0,
                SessionEnd::Finished(result(
                    Outcome::Reverted,
                    Some(mklm_core::FailureReason::CallerDisconnected),
                )),
            ),
        );
        assert_eq!(effects.last(), Some(&Effect::Quit));
    }

    #[test]
    fn a_lost_helper_asks_before_the_recovery_prompt() {
        // T-APPLY-8: the helper dies during the countdown; "put it back now" launches the
        // recovery session (with its own prompt), "later" launches nothing.
        let mut state = ready_state();
        update(&mut state, undo());
        let mut helper = FakeHelper::connect(&mut state, 0);
        helper.live_reset(&mut state, LayoutTable::Jis);
        update(
            &mut state,
            AppMsg::SessionNotice {
                session: 0,
                notice: Notice::HelperLost {
                    exit: None,
                    detail: "the helper closed the pipe".into(),
                },
            },
        );
        assert_eq!(progress_stage(&state), Stage::HelperLost);
        state.visible = false;
        assert_eq!(
            update(
                &mut state,
                AppMsg::RecoveryQuestion {
                    session: 0,
                    countdown: true
                }
            ),
            vec![Effect::ShowWindow, Effect::Render]
        );
        update(&mut state, AppMsg::AnswerRecovery(true));
        for notice in [
            Notice::RecoveringAfterLoss { countdown: true },
            Notice::Starting(SessionKind::Recovery),
        ] {
            update(&mut state, AppMsg::SessionNotice { session: 0, notice });
        }
        // The recovery's prompt: said as such.
        assert_eq!(progress_stage(&state), Stage::Launching);
        update(
            &mut state,
            AppMsg::SessionNotice {
                session: 0,
                notice: Notice::Connected(SessionKind::Recovery),
            },
        );
        assert_eq!(progress_stage(&state), Stage::Recovering);
        let mut recovered = result(Outcome::Recovered, None);
        recovered.op_id = None;
        recovered.recovered.push(mklm_core::RecoveredOp {
            op_id: op(),
            from: OpState::AwaitingConfirm,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        update(
            &mut state,
            ended_with(
                0,
                RequestReport {
                    first: RequestEnd::Ended(SessionEnd::Lost {
                        exit_code: Some(1),
                        detail: "the helper closed the pipe".into(),
                        planned: true,
                        countdown: true,
                    }),
                    lost_needs_recovery: Some(Ok(true)),
                    recovery: Some(RequestEnd::Ended(SessionEnd::Finished(recovered))),
                    recovery_skipped: None,
                },
            ),
        );
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(shown.title, "自動で元に戻しました");
        assert!(
            shown
                .message
                .starts_with("MKLM の管理用プログラムが止まったため、すぐに回復しました。")
        );
        // "Later": nothing launched; the result points to the recovery page.
        let mut state = ready_state();
        update(&mut state, undo());
        FakeHelper::connect(&mut state, 0).live_reset(&mut state, LayoutTable::Jis);
        update(
            &mut state,
            AppMsg::RecoveryQuestion {
                session: 0,
                countdown: true,
            },
        );
        update(&mut state, AppMsg::AnswerRecovery(false));
        update(
            &mut state,
            ended_with(
                0,
                RequestReport {
                    first: RequestEnd::Ended(SessionEnd::Lost {
                        exit_code: None,
                        detail: "the helper exited".into(),
                        planned: true,
                        countdown: true,
                    }),
                    lost_needs_recovery: Some(Ok(true)),
                    recovery: None,
                    recovery_skipped: Some(RecoverySkip::Declined),
                },
            ),
        );
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(shown.next, Some(crate::vm::result::NextStep::Recovery));
        assert!(update(&mut state, AppMsg::ResultNextStep).contains(&Effect::Read));
        assert_eq!(state.page, Page::Recovery);
    }

    #[test]
    fn a_declined_prompt_is_cancelled_and_nothing_else_happens() {
        // T-APPLY-3.
        let mut state = ready_state();
        state.settings.change.uac_notice_seen = true;
        choose_jis(&mut state, Instant::now());
        let effects = update(&mut state, AppMsg::ChangeApply);
        let Effect::StartSession { session, .. } = effects[0] else {
            panic!()
        };
        update(
            &mut state,
            AppMsg::SessionNotice {
                session,
                notice: Notice::Starting(SessionKind::Request),
            },
        );
        update(
            &mut state,
            ended_with(
                session,
                RequestReport {
                    first: RequestEnd::NotLaunched(LaunchError::Declined),
                    lost_needs_recovery: None,
                    recovery: None,
                    recovery_skipped: None,
                },
            ),
        );
        let shown = crate::vm::result::shown_result(&state, Lang::Ja).unwrap();
        assert_eq!(shown.title, "取り消しました（何も変更していません）");
        assert_eq!(state.page, Page::Main);
    }

    #[test]
    fn decisions_answer_only_the_open_question() {
        let mut state = ready_state();
        update(&mut state, undo());
        let mut helper = FakeHelper::connect(&mut state, 0);
        // No question yet: keep and revert do nothing.
        assert!(update(&mut state, AppMsg::KeepChange).is_empty());
        assert!(update(&mut state, AppMsg::RevertChange).is_empty());
        helper.live_reset(&mut state, LayoutTable::Us);
        // A decision about another operation is dropped.
        let other = OpId::parse("11111111-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        assert!(update(&mut state, AppMsg::Decide(Decision::Keep { op_id: other })).is_empty());
        assert_eq!(
            update(&mut state, AppMsg::RevertChange),
            vec![
                Effect::SendDecision(Decision::RevertNow { op_id: op() }),
                Effect::Render
            ]
        );
        // "Decide later" is for the reconnect path only.
        assert!(update(&mut state, AppMsg::DecideLater).is_empty());
    }

    #[test]
    fn the_fixed_mode_standard_layout_is_chosen_on_the_page() {
        let mut state = ready_state();
        state.settings.change.uac_notice_seen = true;
        update(&mut state, AppMsg::OpenChange(KEYCHRON_ROW.into()));
        let Effect::PrepareChange { token, .. } = update(&mut state, AppMsg::ChangeChoose(1))[0]
        else {
            panic!()
        };
        let mut fixed = prepared();
        fixed.snapshot.global = fixtures::global_fixed_jis();
        update(
            &mut state,
            AppMsg::ChangePrepared {
                token,
                at: Instant::now(),
                prepared: Box::new(Ok(fixed)),
            },
        );
        assert!(page(&state).standard_visible);
        update(&mut state, AppMsg::ChangeChooseStandard(1));
        assert_eq!(page(&state).standard_selected, Some(1));
        let effects = update(&mut state, AppMsg::ChangeApply);
        let Effect::StartSession {
            request: Request::Migrate(migrate),
            ..
        } = &effects[0]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(migrate.standard, Layout::Us);
    }

    #[test]
    fn the_restore_preview_of_a_device() {
        let mut state = ready_state();
        state.settings.change.uac_notice_seen = true;
        update(&mut state, AppMsg::OpenChange(KEYCHRON_ROW.into()));
        let effects = update(&mut state, AppMsg::OpenRestore);
        let Effect::PrepareChange {
            token,
            gate: Gate::Restore,
        } = effects[0]
        else {
            panic!("{effects:?}")
        };
        update(
            &mut state,
            AppMsg::ChangePrepared {
                token,
                at: Instant::now(),
                prepared: Box::new(Ok(prepared())),
            },
        );
        // Nothing recorded: nothing to put back, nothing to send.
        let shown = page(&state);
        assert!(shown.restore && !shown.can_apply);
        assert_eq!(
            shown.summary,
            "MKLM 導入前の値のままです。戻すものはありません。"
        );
        assert!(update(&mut state, AppMsg::ChangeApply).is_empty());
        // Layout choices do nothing in the restore preview.
        assert!(update(&mut state, AppMsg::ChangeChoose(0)).is_empty());
    }

    #[test]
    fn navigation_is_off_during_a_change_and_a_session() {
        let mut state = ready_state();
        assert!(navigation_enabled(&state));
        update(&mut state, undo());
        assert!(!navigation_enabled(&state));
        // Leaving the change page by navigation drops the draft.
        let mut state = ready_state();
        update(&mut state, AppMsg::OpenChange(KEYCHRON_ROW.into()));
        update(&mut state, AppMsg::Navigate(Page::Settings));
        assert!(state.draft.is_none());
    }

    #[test]
    fn keyboard_notifications_are_read_once_per_burst() {
        let mut state = AppState::default();
        assert_eq!(
            update(&mut state, AppMsg::KeyboardsChanged),
            vec![Effect::ScheduleSettle(KEYBOARD_SETTLE)]
        );
        // A dongle brings several interfaces: they join the scheduled read.
        assert!(update(&mut state, AppMsg::KeyboardsChanged).is_empty());
        assert!(update(&mut state, AppMsg::KeyboardsChanged).is_empty());
        assert_eq!(
            update(&mut state, AppMsg::KeyboardsSettled),
            vec![Effect::Read]
        );
        // The next change schedules a new read.
        assert_eq!(
            update(&mut state, AppMsg::KeyboardsChanged),
            vec![Effect::ScheduleSettle(KEYBOARD_SETTLE)]
        );
        assert_eq!(KEYBOARD_SETTLE, Duration::from_millis(750));
    }

    #[test]
    fn hiding_and_showing_are_saved() {
        let mut state = AppState::default();
        let container = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
        let effects = update(&mut state, AppMsg::ToggleHidden(container.into()));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveSettings(saved), Effect::Render] if saved.is_hidden(container)
        ));
        update(&mut state, AppMsg::ToggleHidden(container.to_lowercase()));
        assert!(state.settings.keyboards.hidden.is_empty());
        let effects = update(&mut state, AppMsg::ShowHidden(true));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SaveSettings(saved), Effect::Render] if saved.keyboards.show_hidden
        ));
        // Unchanged: nothing to save.
        assert!(update(&mut state, AppMsg::ShowHidden(true)).is_empty());
    }

    #[test]
    fn apply_now_goes_through_the_change_page_and_resets_only_after_the_button() {
        let mut state = with_read(read_with("confirmed"));
        let now = Instant::now();
        // A mouse user: "reset now" is preselected (design m3 B.5, review U1).
        update(&mut state, AppMsg::PointerUsed(now));
        assert_eq!(
            update(&mut state, AppMsg::ApplyNow { at: now }),
            vec![Effect::Render]
        );
        assert_eq!(state.page, Page::Change);
        assert!(!navigation_enabled(&state));
        let draft = state.draft.clone().unwrap();
        assert!(draft.apply_now);
        assert_eq!(draft.name, "Keychron Receiver");
        assert_eq!(draft.members, vec![KEYCHRON.to_string()]);
        assert_eq!(draft.resolved_method(), ApplyMethod::Live);
        // Opening the page starts nothing; it has no layouts, detection or restore preview.
        assert_eq!(state.session, SessionPhase::Idle);
        assert!(update(&mut state, AppMsg::ChangeChoose(0)).is_empty());
        assert!(update(&mut state, AppMsg::OpenRestore).is_empty());
        assert!(update(&mut state, AppMsg::ChangeRestartDetection).is_empty());
        key(&mut state, crate::detect::scancode::YEN);
        assert_eq!(state.draft.as_ref().unwrap().detection, draft.detection);
        // The first time through the UAC explanation (M2 R12), as a change.
        assert_eq!(
            update(&mut state, AppMsg::ChangeApply),
            vec![Effect::Render]
        );
        assert_eq!(state.page, Page::UacNotice);
        let effects = update(&mut state, AppMsg::UacGo);
        let apply = ApplyMethod::Live.options();
        assert!(
            matches!(effects.as_slice(), [Effect::SaveSettings(saved), Effect::StartSession {
                session: 0,
                request: Request::Recover { apply: sent },
                apply: options,
            }, Effect::Render] if saved.change.uac_notice_seen && *sent == apply && *options == apply),
            "{effects:?}"
        );
        assert_eq!(state.overlay, OverlayKind::Progress);
        // The result says how the reset keyboard types afterwards (design m3 B.17).
        assert_eq!(
            state.targets,
            vec![SessionTarget {
                name: "Keychron Receiver".into(),
                members: vec![KEYCHRON.to_string()],
                before: Some(LayoutTable::Us),
            }]
        );
        // The session ends: back to the main screen, the result over it.
        update(
            &mut state,
            ended(
                0,
                SessionEnd::Finished(OperationResult {
                    op_id: None,
                    outcome: Outcome::Recovered,
                    failure: None,
                    pending_action: None,
                    conflicts: Vec::new(),
                    inv_ps2_violation: None,
                    recovered: Vec::new(),
                    warnings: Vec::new(),
                }),
            ),
        );
        assert_eq!(
            (state.page, state.overlay),
            (Page::Main, OverlayKind::Result)
        );
        assert!(state.draft.is_none());
    }

    #[test]
    fn apply_now_does_not_reset_the_only_keyboard_by_default() {
        let mut state = with_read(read_with("confirmed"));
        state.settings.change.uac_notice_seen = true;
        let now = Instant::now();
        // Only the Keychron typed lately: not now, with the warning of plan 1.4.
        key(&mut state, 0x1E);
        update(&mut state, AppMsg::ApplyNow { at: now });
        let draft = state.draft.clone().unwrap();
        assert_eq!(draft.resolved_method(), ApplyMethod::Restart);
        assert!(draft.method_default.unwrap().only_keyboard_warning);
        // "Not now" sends nothing.
        assert!(update(&mut state, AppMsg::ChangeApply).is_empty());
        assert_eq!(state.session, SessionPhase::Idle);
        // The user's own choice: reset now; the explanation was read before, so the prompt
        // follows the button directly.
        assert_eq!(
            update(&mut state, AppMsg::ChangeChooseMethod(0)),
            vec![Effect::Render]
        );
        let effects = update(&mut state, AppMsg::ChangeApply);
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::StartSession {
                        request: Request::Recover { .. },
                        ..
                    },
                    Effect::Render
                ]
            ),
            "{effects:?}"
        );
        // An elevated GUI shows no UAC explanation either (design m3 B.5).
        let mut elevated = with_read(read_with("confirmed"));
        elevated.elevated = true;
        elevated.activity.pointer(now);
        update(&mut elevated, AppMsg::ApplyNow { at: now });
        assert!(matches!(
            update(&mut elevated, AppMsg::ChangeApply)[0],
            Effect::StartSession { .. }
        ));
    }

    #[test]
    fn apply_now_is_offered_only_while_something_is_left() {
        let now = Instant::now();
        // The same keyboard waits for a restart: new operations are blocked.
        let mut blocked = with_read(read_with("pending-reboot"));
        assert!(update(&mut blocked, AppMsg::ApplyNow { at: now }).is_empty());
        assert!(blocked.draft.is_none());
        // Nothing read yet: nothing to offer.
        assert!(update(&mut AppState::default(), AppMsg::ApplyNow { at: now }).is_empty());
        // Not from another page, and not twice.
        let mut elsewhere = with_read(read_with("confirmed"));
        elsewhere.page = Page::Settings;
        assert!(update(&mut elsewhere, AppMsg::ApplyNow { at: now }).is_empty());
        let mut state = with_read(read_with("confirmed"));
        state.settings.change.uac_notice_seen = true;
        state.activity.pointer(now);
        update(&mut state, AppMsg::ApplyNow { at: now });
        assert!(update(&mut state, AppMsg::ApplyNow { at: now }).is_empty());
        // The keyboard is replugged and types JIS meanwhile: the page sends nothing.
        let mut replugged = read_with("confirmed");
        if let Some(snapshot) = &mut replugged.snapshot {
            snapshot.keyboards[1].reported_type = Some(KeyboardType::JIS);
        }
        update(&mut state, AppMsg::SystemRead(Box::new(replugged)));
        assert!(apply_now_targets(&state).is_empty());
        assert!(update(&mut state, AppMsg::ChangeApply).is_empty());
        // Cancel: back to the main screen.
        assert_eq!(
            update(&mut state, AppMsg::CancelChange),
            vec![Effect::Read, Effect::Render]
        );
        assert_eq!(state.page, Page::Main);
        assert!(state.draft.is_none());
    }

    #[test]
    fn the_banner_button_leads_to_its_page() {
        let mut state = with_read(read_with("pending-reboot"));
        let effects = update(&mut state, AppMsg::BannerAction { at: Instant::now() });
        // The decision page is read again first.
        assert_eq!(effects, vec![Effect::Read, Effect::Render]);
        assert_eq!(state.page, Page::Restart);
        // "今すぐ反映…": the change page for it.
        let mut state = with_read(read_with("confirmed"));
        update(&mut state, AppMsg::PointerUsed(Instant::now()));
        update(&mut state, AppMsg::BannerAction { at: Instant::now() });
        assert_eq!(state.page, Page::Change);
        assert!(state.draft.as_ref().is_some_and(|draft| draft.apply_now));
        // Not while a session runs.
        let mut busy = with_read(read_with("pending-reboot"));
        update(&mut busy, undo());
        assert!(update(&mut busy, AppMsg::BannerAction { at: Instant::now() }).is_empty());
        // No attention: the banner has no button.
        let mut quiet = AppState {
            read: Some(SystemRead {
                snapshot: None,
                warnings: Vec::new(),
                journal: None,
                boot: None,
                summary: StartupSummary::default(),
            }),
            ..AppState::default()
        };
        assert!(update(&mut quiet, AppMsg::BannerAction { at: Instant::now() }).is_empty());
    }

    #[test]
    fn identify_is_toggled_only_on_the_main_screen() {
        let mut state = AppState {
            page: Page::Journal,
            ..AppState::default()
        };
        assert!(update(&mut state, AppMsg::ToggleIdentify).is_empty());
        assert!(!state.identify);
        state.page = Page::Main;
        update(&mut state, AppMsg::ToggleIdentify);
        assert!(state.identify);
        // A session's overlay takes over and ends it.
        update(&mut state, undo());
        assert!(!state.identify);
    }

    #[test]
    fn identify_from_the_tray_does_not_replace_a_flow() {
        // In the tray on the settings page: shown, on the main screen, identifying.
        let mut state = AppState {
            page: Page::Settings,
            ..AppState::default()
        };
        let effects = update(&mut state, AppMsg::StartIdentify);
        assert_eq!(effects.first(), Some(&Effect::ShowWindow));
        assert!(effects.contains(&Effect::Read));
        assert!(state.visible && state.identify);
        assert_eq!(state.page, Page::Main);
        // Leaving the main screen ends identifying.
        state.highlighted = Some(KEYCHRON.into());
        update(&mut state, AppMsg::Navigate(Page::Journal));
        assert!(!state.identify && state.highlighted.is_none());
        // On the change page: only brought to the front.
        let mut changing = AppState {
            page: Page::Change,
            ..AppState::default()
        };
        assert_eq!(
            update(&mut changing, AppMsg::StartIdentify),
            vec![Effect::ShowWindow, Effect::Render]
        );
        assert_eq!(changing.page, Page::Change);
        assert!(!changing.identify);
    }
}
