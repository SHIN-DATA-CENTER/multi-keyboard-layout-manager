//! The GUI's state and its transitions (design m3 A.6): one [`AppState`] on the UI thread,
//! changed only by [`update`] from an [`AppMsg`]; `update` returns [`Effect`]s that `app.rs`
//! carries out (start a helper session, read the system, save settings …). Pure, so the flows of
//! design m3 B are unit-tested without a window, a helper or a registry.
//!
//! Helper sessions (design m3 A.4, B.18): at most one at a time. [`SessionPhase::Launching`] is
//! set in the same `update` that emits [`Effect::StartSession`], so a second start is refused
//! even while the UAC prompt is up; every message from a session worker carries its
//! [`SessionId`], and messages of an older session are dropped.

use std::time::Instant;

use mklm_client::orchestrator::RequestReport;
use mklm_client::run_once::{RunOnceError, RunOnceOutcome};
use mklm_client::session::{Notice, Prompt, SessionView};
use mklm_client::startup::StartupSummary;
use mklm_core::{
    ApplyOptions, BootId, Decision, Event, Journal, LayoutChoice, OperationPlan, SystemSnapshot,
};
use mklm_ipc::Request;

use crate::detect::{Detection, Press};
use crate::i18n::Lang;
use crate::settings::Settings;
use crate::vm::change::{ApplyMethod, InputActivity};
use crate::vm::keytest::{self, KeyContext, KeyTest};

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

/// A change being prepared on the change page (design m3 B.4, B.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDraft {
    /// The row's keyboards (instance IDs); the request targets the first present one.
    pub members: Vec<String>,
    pub choice: Option<LayoutChoice>,
    /// The apply method (default from `vm::change::default_apply_method`).
    pub method: Option<ApplyMethod>,
    /// The plan shown (made with `method`'s options); sent as `ExpectedPlan`.
    pub plan: Option<OperationPlan>,
    pub detection: Detection,
    /// What the last key press did in the detection.
    pub last_press: Option<Press>,
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

/// How a session ended, with the RunOnce rule applied on the worker right after it (design m2
/// C17, m3 A.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutcome {
    pub report: RequestReport,
    pub run_once: Result<RunOnceOutcome, RunOnceError>,
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
    pub session: SessionPhase,
    /// The id the next session gets.
    pub next_session: SessionId,
    /// The last session's outcome, for the result overlay.
    pub outcome: Option<SessionOutcome>,
    /// The helper was lost during a countdown (true) or elsewhere, and the worker waits for the
    /// user's answer to the recovery question.
    pub recovery_question: Option<bool>,
    /// The window is shown (false while in the tray).
    pub visible: bool,
    /// Quitting once the running session ends (design m3 F.5).
    pub quit_pending: bool,
}

/// Something that happened (a user action, a worker's answer, a watcher).
#[derive(Debug, Clone, PartialEq)]
pub enum AppMsg {
    Navigate(Page),
    SystemRead(Box<SystemRead>),
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
    /// Read the system again (refresh button, input language changed, resume).
    Refresh,
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
    Decide(Decision),
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

/// Applies `msg` to `state`. Flows not written yet are no-ops that leave the state unchanged
/// (the skeleton never panics on a click).
pub fn update(state: &mut AppState, msg: AppMsg) -> Vec<Effect> {
    match msg {
        AppMsg::Navigate(page) => {
            state.page = page;
            let mut effects = vec![Effect::Render];
            if page == Page::Main || page == Page::Journal {
                effects.insert(0, Effect::Read);
            }
            effects
        }
        AppMsg::SystemRead(read) => {
            state.read = Some(*read);
            vec![Effect::Render]
        }
        AppMsg::ActiveLayout(hkl) if hkl != state.active_hkl => {
            state.active_hkl = hkl;
            vec![Effect::Render]
        }
        AppMsg::ActiveLayout(_) => Vec::new(),
        AppMsg::KeyTestPressed { text, shift } => {
            let lang = state.lang.unwrap_or(Lang::Ja);
            let source_name = state
                .last_key
                .as_ref()
                .map(|(id, _)| display_name(state, id));
            // WP-U3 / WP-U4: `expected` from the session view (countdown, reconnect) or the
            // post-reboot entry; `None` shows a neutral verdict.
            let context = KeyContext {
                source: state
                    .last_key
                    .as_ref()
                    .map(|(id, code)| (id.as_str(), *code)),
                source_name: source_name.as_deref(),
                active_hkl: state.active_hkl,
                expected: None,
            };
            match keytest::key_pressed(&text, shift, &context, lang) {
                Some(test) => {
                    state.key_test = test;
                    vec![Effect::Render]
                }
                None => Vec::new(),
            }
        }
        AppMsg::Refresh => vec![Effect::Read],
        AppMsg::ToggleIdentify => {
            state.identify = !state.identify;
            if !state.identify {
                state.highlighted = None;
            }
            vec![Effect::Render]
        }
        AppMsg::DeviceKey {
            instance_id,
            scancode,
            at,
        } => device_key(state, instance_id, scancode, at),
        AppMsg::PointerUsed(at) => {
            state.activity.pointer(at);
            Vec::new()
        }
        AppMsg::StartRequest { request, apply } => {
            if state.session != SessionPhase::Idle {
                // One session at a time (design m3 A.4): a second click, or undo from the tray
                // while a UAC prompt is up, does nothing.
                return Vec::new();
            }
            let session = state.next_session;
            state.next_session += 1;
            state.session = SessionPhase::Launching { id: session };
            state.overlay = OverlayKind::Progress;
            state.outcome = None;
            vec![
                Effect::StartSession {
                    session,
                    request,
                    apply,
                },
                Effect::Render,
            ]
        }
        AppMsg::SessionNotice { session, notice } if current_session(state, session) => {
            match notice {
                // A new helper is being launched (the request's, or the recovery's): a UAC
                // prompt may be up.
                Notice::Starting(_) => state.session = SessionPhase::Launching { id: session },
                Notice::Connected(_) => {
                    state.session = SessionPhase::Running {
                        id: session,
                        view: Box::default(),
                    };
                }
                Notice::HelperLost { .. } | Notice::RecoveringAfterLoss { .. } => {
                    state.overlay = OverlayKind::Progress;
                }
            }
            vec![Effect::Render]
        }
        AppMsg::SessionEvent { session, view, .. } if current_session(state, session) => {
            let overlay = match &view.prompt {
                Prompt::Countdown { .. } => OverlayKind::Countdown,
                Prompt::Reconnect { .. } => OverlayKind::Reconnect,
                Prompt::None | Prompt::Answered { .. } => OverlayKind::Progress,
            };
            let mut effects = Vec::new();
            // A question opened: bring the window to the front even if the user switched away
            // during the UAC prompt — key tests and Raw Input need the focus (review U10).
            if overlay != state.overlay
                && matches!(overlay, OverlayKind::Countdown | OverlayKind::Reconnect)
            {
                state.visible = true;
                effects.push(Effect::ShowWindow);
            }
            state.overlay = overlay;
            state.session = SessionPhase::Running { id: session, view };
            effects.push(Effect::Render);
            effects
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
            vec![Effect::AnswerRecovery(yes), Effect::Render]
        }
        AppMsg::SessionEnded { session, outcome } if current_session(state, session) => {
            state.session = SessionPhase::Idle;
            state.recovery_question = None;
            let abandoned = matches!(
                outcome.report.first,
                mklm_client::orchestrator::RequestEnd::Ended(
                    mklm_client::session::SessionEnd::Abandoned { .. }
                )
            );
            state.overlay = if abandoned {
                OverlayKind::None
            } else {
                OverlayKind::Result
            };
            state.outcome = Some(*outcome);
            // The RunOnce rule already ran on the worker (design m3 A.2.3).
            let mut effects = vec![Effect::Read, Effect::Render];
            if state.quit_pending {
                effects.push(Effect::Quit);
            }
            effects
        }
        // Messages of a session that is not the current one.
        AppMsg::SessionNotice { .. }
        | AppMsg::SessionEvent { .. }
        | AppMsg::RecoveryQuestion { .. }
        | AppMsg::SessionEnded { .. } => Vec::new(),
        AppMsg::Decide(decision) => vec![Effect::SendDecision(decision)],
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
        AppMsg::QuitRequested => match state.session {
            SessionPhase::Idle => vec![Effect::Quit],
            // Nothing is connected yet: leave now (design m3 F.5 "UAC 待ち").
            SessionPhase::Launching { .. } => vec![Effect::CancelSession, Effect::Quit],
            SessionPhase::Running { .. } => {
                state.quit_pending = true;
                vec![Effect::CancelSession]
            }
        },
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
        && state
            .settings
            .learn_physical(crate::settings::PhysicalLayout {
                id: instance_id.clone(),
                layout: crate::settings::PhysicalKind::Jis,
                source: crate::settings::PhysicalSource::JisOnlyKey,
            })
    {
        save = true;
        visible_change = true; // the row may now say "実物は JIS 配列です…"
    }
    if let Some(draft) = &mut state.draft {
        draft.last_press = Some(draft.detection.press(&instance_id, scancode));
        visible_change = true;
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
            Prompt::Countdown { op_id, .. } => vec![Effect::SendDecision(Decision::RevertNow {
                op_id: op_id.clone(),
            })],
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

#[cfg(test)]
mod tests {
    use mklm_client::orchestrator::RequestEnd;
    use mklm_client::session::SessionEnd;
    use mklm_core::OpId;

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn key(state: &mut AppState, scancode: u32) -> Vec<Effect> {
        update(
            state,
            AppMsg::DeviceKey {
                instance_id: KEYCHRON.into(),
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
        AppMsg::SessionEnded {
            session,
            outcome: Box::new(SessionOutcome {
                report: RequestReport {
                    first: RequestEnd::Ended(end),
                    lost_needs_recovery: None,
                    recovery: None,
                    recovery_skipped: None,
                },
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
                notice: Notice::Connected(mklm_client::session::SessionKind::Request),
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

    #[test]
    fn closing_the_window_during_a_countdown_reverts() {
        let op_id = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
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
            ..AppState::default()
        };
        assert_eq!(
            update(&mut state, AppMsg::WindowCloseRequested),
            vec![Effect::SendDecision(Decision::RevertNow { op_id })]
        );
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
}
