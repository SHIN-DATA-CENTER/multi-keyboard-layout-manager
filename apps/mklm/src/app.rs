//! The UI thread (design m3 A.4, A.6): start-up, the Slint event loop, and the controller that
//! turns callbacks and worker messages into [`AppMsg`]s, runs [`state::update`] and carries out
//! the [`Effect`]s. Nothing here blocks: reads go to the I/O worker ([`IoWorker`]), helper
//! sessions to a session worker ([`SessionWorker`]).
//!
//! The process (design m3 F): start-up checks with a message box for what stops MKLM (F.6), the
//! single instance (F.1, [`single_instance`]), settings with a `.bad` fallback (F.4), the log
//! (F.8, [`log`]), closing, quitting and the end of the Windows session (F.5), and a bounded wait
//! for the I/O worker's writes before the process ends.

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mklm_client::launch::LaunchConfig;
use mklm_client::orchestrator::{LaunchError, LaunchFailure, RequestEnd, RequestReport};
use mklm_client::run_once::RunOnceError;
use mklm_win::instance::{InstanceCommand, InstanceReply};
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::Theme as WinitTheme;
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::args::{Args, StartMode};
use crate::autostart::AutostartTask;
use crate::i18n::{self, Lang, LangChoice, StartupError};
use crate::input_capture::InputCapture;
use crate::journal_ui::JournalUi;
use crate::log;
use crate::models::KeptModel;
use crate::reader::{self, IoTask, IoWorker};
use crate::settings::{Settings, SettingsStore, load_or_default};
use crate::single_instance::{self, Claim, InstanceService};
use crate::state::{
    self, AppMsg, AppState, Effect, OverlayKind, Page, QuitAnswer, SessionId, SessionOutcome,
    SessionPhase, SettingsMsg, UpdateMsg, WizardState,
};
use crate::theme::{ResolvedTheme, ThemeMode, is_dark_rgb};
use crate::tray;
use crate::ui::{self, AppWindow, TrayIcon};
use crate::update_worker::UpdateWorker;
use crate::vm::keyboards::{KeyboardRow, MainInput, main_screen};
use crate::vm::{self, ListOp, keytest, list_ops};
use crate::watchers::Watchers;
use crate::wizard_ui::WizardUi;
use crate::worker::{self, SessionWorker};
use crate::{BUILD_ID, MIN_BUILD};

/// How long the end of the Windows session waits in all: for a running helper session, then —
/// with what is left — for the I/O worker's queued writes (design m3 A.4 rule 1, F.5: at most
/// 3 s, well inside the 5 s after which Windows calls an application hung).
const END_SESSION_WAIT: Duration = Duration::from_secs(3);

/// Whether this process may register the post-reboot RunOnce value at the end of the Windows
/// session: only when it is not elevated (design m3 F.2; `mklm_client::run_once`): an elevated
/// process may run as another administrator than the signed-in user, so its `HKCU` would be the
/// wrong account's. Set once at start-up, outside any `RefCell` (the shell window reads it inside
/// its window procedure); false until then and when elevation cannot be told.
static MAY_REGISTER_POST_REBOOT: AtomicBool = AtomicBool::new(false);

/// How long quitting waits for the I/O worker's queued writes: settings, the RunOnce rule, the
/// autostart value (design m3 F.5).
const QUIT_IO_WAIT: Duration = Duration::from_secs(1);

thread_local! {
    static APP: RefCell<Option<Rc<Controller>>> = const { RefCell::new(None) };
}

/// Handles `msg` now (UI thread only). The controller is cloned out of the thread-local first,
/// so no borrow is held while it runs.
pub fn dispatch(msg: AppMsg) {
    if let Some(app) = current_app() {
        app.handle(msg);
    }
}

/// Hands `msg` to the UI thread (any thread). Dropped when the event loop has ended.
pub fn post(msg: AppMsg) {
    let _ = slint::invoke_from_event_loop(move || dispatch(msg));
}

/// The controller while the UI runs. Never panics on a borrow: the shell window's procedure may
/// run while the thread-local is being set or cleared.
fn current_app() -> Option<Rc<Controller>> {
    APP.with(|app| app.try_borrow().ok().and_then(|app| app.clone()))
}

/// The end of `session` when no helper was started for it: a setup failure with `message`
/// (English), which takes the state from `Launching` back to `Idle` (review A2).
fn not_launched(session: SessionId, message: String) -> AppMsg {
    AppMsg::SessionEnded {
        session,
        outcome: Box::new(SessionOutcome {
            report: RequestReport {
                first: RequestEnd::NotLaunched(LaunchError::Failed {
                    kind: LaunchFailure::Setup,
                    message,
                }),
                lost_needs_recovery: None,
                recovery: None,
                recovery_skipped: None,
            },
            run_once: Err(RunOnceError::Check("no session ran".into())),
        }),
    }
}

/// A command from a second MKLM process (UI thread; design m3 F.1): `activate` shows the window
/// without changing the page (review A8), `quit` quits as the tray's "終了" does. The reply is
/// `busy` while the quit has to wait for a connected helper session (design m3 F.5, m5b E.4.1).
/// The update runner's `quit-if-idle` quits only when nothing is going on, and otherwise changes
/// nothing (`state::quit_if_idle`, design m5b E.4.1); `received` is when the pipe thread got it.
/// `ping` (a second `--tray` or `--after-update` start) changes nothing: this instance already
/// shows what its own start found (design m5b D.13 step 5).
fn instance_command(command: InstanceCommand, received: Instant) -> InstanceReply {
    let message = match command {
        InstanceCommand::Activate => AppMsg::Activate,
        InstanceCommand::Quit => AppMsg::QuitRequested,
        InstanceCommand::QuitIfIdle => return quit_if_idle(received),
        InstanceCommand::Ping => return single_instance::reply_for(command, false),
    };
    let quit_waits = current_app().is_some_and(|app| {
        app.state
            .try_borrow()
            .is_ok_and(|state| state::quit_reply_waits(&state))
    });
    dispatch(message);
    single_instance::reply_for(command, quit_waits)
}

/// `quit-if-idle` on the UI thread: the decision and what it does in one place (design m5b
/// E.4.1).
fn quit_if_idle(received: Instant) -> InstanceReply {
    let Some(app) = current_app() else {
        return InstanceReply::Busy;
    };
    let decided = app.state.try_borrow_mut().ok().map(|mut state| {
        state.now = Some(Instant::now());
        state.now_unix = Some(unix_now());
        state::quit_if_idle(&mut state, received)
    });
    let Some((effects, reply)) = decided else {
        return InstanceReply::Busy;
    };
    log::info(format!("quit-if-idle from the update runner: {reply:?}"));
    for effect in effects {
        app.run(effect);
    }
    match reply {
        state::InstanceReply::Ok => InstanceReply::Ok,
        state::InstanceReply::Busy => InstanceReply::Busy,
    }
}

/// Now in Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Local time for the update texts (design m5b E.2): the machine's zone, UTC when it cannot be
/// read.
fn local_minute(unix: u64) -> vm::update::LocalMinute {
    match mklm_win::time::local_time(mklm_core::Timestamp(unix.saturating_mul(1000))) {
        Ok(local) => vm::update::LocalMinute {
            year: i64::from(local.year),
            month: u32::from(local.month),
            day: u32::from(local.day),
            hour: u32::from(local.hour),
            minute: u32::from(local.minute),
        },
        Err(_) => vm::update::fixed_offset(0, unix),
    }
}

/// Owns the window, the tray, the workers and the state (UI thread only).
struct Controller {
    window: AppWindow,
    icon: slint::Image,
    tray: RefCell<Option<TrayIcon>>,
    state: RefCell<AppState>,
    io: IoWorker,
    session: RefCell<Option<SessionWorker>>,
    watchers: RefCell<Watchers>,
    /// The single-instance pipe's thread (design m3 F.1); `None` without the pipe.
    instance: RefCell<Option<InstanceService>>,
    theme_mode: Cell<ThemeMode>,
    os_dark: Cell<Option<bool>>,
    /// The keyboard list's model, kept for the window's lifetime and updated row by row, so
    /// that a render does not rebuild the rows and drop the focus (design m3 A.6, review A7).
    keyboards: Rc<VecModel<ui::KeyboardRowVm>>,
    /// The rows the model shows now (to diff against).
    shown_rows: RefCell<Vec<KeyboardRow>>,
    /// The change page's radio groups (layouts, standard layout, apply method), kept like the
    /// list.
    change_choices: KeptModel<ui::ChoiceVm>,
    change_standard_choices: KeptModel<ui::ChoiceVm>,
    change_method_choices: KeptModel<ui::ChoiceVm>,
    /// Polls the input language while the window is visible (design m3 A.4 rule 5).
    layout_timer: slint::Timer,
    /// Gathers keyboard arrivals and removals before the list is read again (design m3 B.2).
    settle_timer: slint::Timer,
    /// The restart, post-reboot, conflict, history and recovery pages (WP-U4, WP-U5).
    journal_ui: JournalUi,
    /// The first-run wizard (WP-U2).
    wizard_ui: WizardUi,
    /// An automated check (`--exit-after`): it only reads and never writes the user's HKCU, so
    /// the RunOnce rule is skipped also when the post-reboot check asks for it.
    automated: bool,
    /// The update worker (design m5b E.1).
    update_worker: Option<UpdateWorker>,
    /// The next automatic update check (design m5b E.1).
    check_timer: slint::Timer,
    /// Closes the hand-off overlay (design m5b E.4).
    handoff_timer: slint::Timer,
}

impl std::fmt::Debug for Controller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller").finish_non_exhaustive()
    }
}

impl Controller {
    fn handle(&self, msg: AppMsg) {
        self.log_message(&msg);
        let effects = {
            let mut state = self.state.borrow_mut();
            state.now = Some(std::time::Instant::now());
            state.now_unix = Some(unix_now());
            state::update(&mut state, msg)
        };
        for effect in effects {
            self.run(effect);
        }
    }

    /// Logs what the log keeps of `msg` (design m3 F.8): read problems, helper sessions,
    /// quitting. Never key presses, the key test or the detection (plan 2.2): those messages are
    /// not matched here, and no message is logged in its `Debug` form.
    fn log_message(&self, msg: &AppMsg) {
        let Ok(state) = self.state.try_borrow() else {
            return;
        };
        let current = |session: SessionId| state.session.id() == Some(session);
        match msg {
            AppMsg::SystemRead(read)
                if state.read.as_ref().map(|last| &last.warnings) != Some(&read.warnings) =>
            {
                for warning in &read.warnings {
                    log::warn(format!("read: {warning}"));
                }
                if read.summary.unreadable > 0 {
                    log::warn(format!(
                        "read: {} journal entries are of a newer MKLM",
                        read.summary.unreadable
                    ));
                }
            }
            AppMsg::SessionNotice { session, notice } if current(*session) => {
                log::info(format!("session {session}: {}", log::notice_text(notice)));
            }
            AppMsg::RecoveryQuestion { session, countdown } if current(*session) => {
                log::info(format!(
                    "session {session}: asking whether to recover after the lost helper (countdown: {countdown})"
                ));
            }
            AppMsg::AnswerRecovery(yes) if state.recovery_question.is_some() => {
                log::info(format!(
                    "the user chose to recover {}",
                    if *yes { "now" } else { "later" }
                ));
            }
            AppMsg::SessionEnded { session, outcome } if current(*session) => {
                log::info(format!(
                    "session {session} ended: {}",
                    log::session_end_text(outcome)
                ));
            }
            AppMsg::Activate => log::info("activated"),
            AppMsg::QuitRequested => log::info("quit requested"),
            AppMsg::QuitConfirmAnswered(answer) => {
                log::info(format!("quit confirmation answered: {answer:?}"));
            }
            AppMsg::AutostartToggled(on) => log::info(format!("sign-in start switched: {on}")),
            AppMsg::Update(UpdateMsg::UpdateNow) => log::info("update now: pressed"),
            AppMsg::Update(UpdateMsg::AutoCheck(on)) => {
                log::info(format!("automatic update checks switched: {on}"));
            }
            AppMsg::Update(UpdateMsg::SessionEnded { session, end }) if current(*session) => {
                log::info(format!("update session {session} ended: {end:?}"));
            }
            AppMsg::Update(UpdateMsg::HandOffClosed) => {
                log::info("the update was handed over; quitting");
            }
            _ => {}
        }
    }

    fn run(&self, effect: Effect) {
        match effect {
            Effect::Read => self.io.send(IoTask::Read),
            Effect::ReadForResult => self.io.send(IoTask::ReadForResult),
            Effect::PrepareChange { token, gate } => {
                self.io.send(IoTask::PrepareChange { token, gate });
            }
            Effect::CopyText(text) => {
                // English diagnostics only, never keys (design m3 B.17).
                if let Err(error) = mklm_win::ui::copy_text_to_clipboard(&text) {
                    log::warn(format!("copying the details failed: {error}"));
                }
            }
            Effect::ScheduleSettle(delay) => {
                self.settle_timer
                    .start(slint::TimerMode::SingleShot, delay, || {
                        dispatch(AppMsg::KeyboardsSettled);
                    });
            }
            Effect::SaveSettings(settings) => self.io.send(IoTask::SaveSettings(settings)),
            Effect::Autostart(task) => self.io.send(IoTask::Autostart(task)),
            Effect::ReadNonKeyboardValues(keyboards) => {
                self.io.send(IoTask::ReadNonKeyboardValues(keyboards));
            }
            Effect::ReadMachineSettings => self.io.send(IoTask::ReadMachineSettings),
            Effect::StartSession {
                session,
                request,
                apply,
            } => self.start_session(session, request, apply),
            Effect::SendDecision(decision) => {
                if let Some(worker) = self.session.borrow().as_ref() {
                    worker.decide(decision);
                }
            }
            Effect::CancelSession => {
                if let Some(worker) = self.session.borrow().as_ref() {
                    worker.cancel();
                }
            }
            Effect::AnswerRecovery(yes) => {
                if let Some(worker) = self.session.borrow().as_ref() {
                    worker.answer_recovery(yes);
                }
            }
            Effect::RunOnceRule if self.automated => {
                log::info("automated check: the RunOnce rule is skipped");
            }
            Effect::RunOnceRule => self.io.send(IoTask::RunOnceRule),
            Effect::ShowWindow => {
                self.show_window();
                self.poll_input_language(true);
            }
            Effect::HideWindow => {
                let _ = self.window.hide();
                self.poll_input_language(false);
            }
            Effect::Quit => {
                log::info("quitting");
                let _ = slint::quit_event_loop();
            }
            Effect::Render => {
                self.update_tray();
                self.render();
            }
            Effect::Journal(effect) => self.journal_ui.run(&self.window, &self.io, effect),
            Effect::Update(task) => {
                if let Some(worker) = &self.update_worker {
                    worker.send(task);
                }
            }
            Effect::CancelUpdateTask => {
                if let Some(worker) = &self.update_worker {
                    worker.cancel();
                }
            }
            Effect::ScheduleUpdateCheck { after, jitter } => {
                let delay = after + random_part(jitter);
                log::info(format!("next update check in {} s", delay.as_secs()));
                self.check_timer
                    .start(slint::TimerMode::SingleShot, delay, || {
                        dispatch(AppMsg::Update(UpdateMsg::CheckDue));
                    });
            }
            Effect::StopUpdateCheck => self.check_timer.stop(),
            Effect::StartUpdateSession {
                session,
                offer,
                installer,
            } => self.start_update_session(session, *offer, installer),
            Effect::AfterUpdateRunOnce(_) if self.automated => {
                log::info("automated check: the after-update RunOnce value is left alone");
            }
            Effect::AfterUpdateRunOnce(register) => {
                self.io.send(IoTask::AfterUpdateRunOnce(register));
            }
            Effect::HandOffTimer(after) => {
                self.handoff_timer
                    .start(slint::TimerMode::SingleShot, after, || {
                        dispatch(AppMsg::Update(UpdateMsg::HandOffClosed));
                    });
            }
        }
    }

    /// Starts an update session on a session worker (design m5b E.4), like `start_session`.
    fn start_update_session(
        &self,
        session: SessionId,
        offer: mklm_client::update::check::Offer,
        installer: std::path::PathBuf,
    ) {
        use crate::state::update::UpdateSessionEnd;
        if self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|worker| !worker.finished())
        {
            log::warn(format!(
                "update session {session}: not started, the previous session worker still runs"
            ));
            post(AppMsg::Update(UpdateMsg::SessionEnded {
                session,
                end: Box::new(UpdateSessionEnd::NotLaunched(LaunchError::Failed {
                    kind: LaunchFailure::Setup,
                    message: "the previous session worker still runs".into(),
                })),
            }));
            return;
        }
        log::info(format!(
            "update session {session}: starting ({})",
            offer.verified.version
        ));
        let started = LaunchConfig::current(BUILD_ID, self.hwnd())
            .map_err(|error| error.to_string())
            .and_then(|launch| {
                SessionWorker::start_update(session, offer, installer, launch)
                    .map_err(|error| format!("the session worker could not start: {error}"))
            });
        match started {
            Ok(worker) => *self.session.borrow_mut() = Some(worker),
            Err(message) => post(AppMsg::Update(UpdateMsg::SessionEnded {
                session,
                end: Box::new(UpdateSessionEnd::NotLaunched(LaunchError::Failed {
                    kind: LaunchFailure::Setup,
                    message,
                })),
            })),
        }
    }

    /// The tray's tooltip, its "確認待ちの変更をすべて元に戻す…" item (design m3 B.16) and its
    /// "MKLM を更新…" item (design m5b E.3), also while the window is hidden.
    fn update_tray(&self) {
        let Ok(state) = self.state.try_borrow() else {
            return;
        };
        let lang = state.lang.unwrap_or(Lang::Ja);
        let summary = state.read.as_ref().map(|read| &read.summary);
        let (tooltip, can_update) = vm::update::tray(&state, lang);
        let tray_state = tray::with_update(
            tray::tray_state(summary, state::navigation_enabled(&state), lang),
            tooltip,
            can_update,
        );
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray::update_tray(tray, &tray_state);
        }
    }

    /// Starts the session worker. The state has already refused a second session; a worker that
    /// still lives (its end not yet processed) refuses too. A start that fails is reported like
    /// a launch failure, so the state leaves `Launching` (review A2).
    fn start_session(
        &self,
        session: SessionId,
        request: mklm_ipc::Request,
        apply: mklm_core::ApplyOptions,
    ) {
        if self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|worker| !worker.finished())
        {
            // Refused (review A2), and reported like a launch failure: the state set `Launching`
            // with the start, and only `SessionEnded` takes it back to `Idle`.
            let message = "the previous session worker still runs".to_string();
            log::warn(format!("session {session}: not started, {message}"));
            post(not_launched(session, message));
            return;
        }
        log::info(format!(
            "session {session}: starting {}",
            log::request_kind(&request)
        ));
        let started = LaunchConfig::current(BUILD_ID, self.hwnd())
            .map_err(|error| error.to_string())
            .and_then(|launch| {
                SessionWorker::start(session, request, apply, launch)
                    .map_err(|error| format!("the session worker could not start: {error}"))
            });
        match started {
            Ok(worker) => *self.session.borrow_mut() = Some(worker),
            Err(message) => post(not_launched(session, message)),
        }
    }

    /// Starts or stops the 500 ms input-language poll (only while the window is visible, so
    /// that MKLM in the tray does not wake up twice a second; review A10).
    fn poll_input_language(&self, on: bool) {
        if on {
            dispatch(AppMsg::ActiveLayout(mklm_win::ui::active_keyboard_layout()));
            self.layout_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(500),
                || dispatch(AppMsg::ActiveLayout(mklm_win::ui::active_keyboard_layout())),
            );
        } else {
            self.layout_timer.stop();
        }
    }

    /// The main window's HWND (the owner of the UAC prompt).
    fn hwnd(&self) -> Option<isize> {
        self.window
            .window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
                _ => None,
            })
            .flatten()
    }

    /// Shows the window and brings it to the front; when Windows refuses the foreground (another
    /// application has it), the taskbar button flashes (design m3 B.6, B.9, J.6).
    fn show_window(&self) {
        let _ = self.window.show();
        self.window.window().with_winit_window(|w| {
            w.set_minimized(false);
            w.focus_window();
            if !w.has_focus() {
                w.request_user_attention(Some(
                    slint::winit_030::winit::window::UserAttentionType::Critical,
                ));
            }
        });
    }

    // --- Theme (design m3 C.1) ---

    fn resolved_theme(&self) -> ResolvedTheme {
        let high_contrast = mklm_win::ui::theme::high_contrast_on().unwrap_or(false);
        let colors = mklm_win::ui::theme::system_colors();
        ResolvedTheme::new(
            self.theme_mode.get(),
            self.os_dark.get(),
            high_contrast,
            is_dark_rgb(colors.window),
        )
    }

    fn apply_theme(&self) {
        let theme = self.resolved_theme();
        let global = self.window.global::<ui::Theme>();
        global.set_high_contrast(theme.high_contrast);
        if theme.high_contrast {
            let colors = mklm_win::ui::theme::system_colors();
            let rgb = |value: u32| {
                let [_, r, g, b] = value.to_be_bytes();
                slint::Color::from_rgb_u8(r, g, b)
            };
            global.set_hc_window(rgb(colors.window));
            global.set_hc_window_text(rgb(colors.window_text));
            global.set_hc_highlight(rgb(colors.highlight));
            global.set_hc_highlight_text(rgb(colors.highlight_text));
            global.set_hc_button_face(rgb(colors.button_face));
            global.set_hc_button_text(rgb(colors.button_text));
            global.set_hc_gray_text(rgb(colors.gray_text));
            global.set_hc_hot_light(rgb(colors.hot_light));
        }
        self.window.invoke_apply_color_scheme(theme.dark);
        self.set_title_bar(theme.dark);
    }

    /// winit's per-window theme and `DWMWA_USE_IMMERSIVE_DARK_MODE` together (M0 #8 C, D).
    fn set_title_bar(&self, dark: bool) {
        self.window.window().with_winit_window(|w| {
            w.set_theme(Some(if dark {
                WinitTheme::Dark
            } else {
                WinitTheme::Light
            }));
            if let Ok(handle) = w.window_handle()
                && let RawWindowHandle::Win32(handle) = handle.as_raw()
            {
                let _ = mklm_win::ui::theme::set_title_bar_dark(handle.hwnd.get(), dark);
            }
        });
    }

    // --- Rendering: view-models into the window (design m3 A.6) ---

    fn render(&self) {
        let state = self.state.borrow();
        // Nothing to draw in the tray; showing the window renders (review A10).
        if !state.visible {
            return;
        }
        let lang = state.lang.unwrap_or(Lang::Ja);
        self.window.set_screen(screen(state.page));
        self.window.set_identify_active(state.identify);
        self.window
            .set_show_hidden(state.settings.keyboards.show_hidden);
        let test = if state.key_test == vm::keytest::KeyTest::default() {
            keytest::prompt(lang)
        } else {
            state.key_test.clone()
        };
        self.window.set_key_test(ui::KeyTestVm {
            prompt: test.prompt.into(),
            last_text: test.last_text.into(),
            verdict: test.verdict.into(),
            verdict_tone: tone(test.verdict_tone),
            device: test.device.into(),
        });
        self.window
            .set_navigation_enabled(state::navigation_enabled(&state));
        self.window.set_overlay(overlay(state.overlay));
        // The page's controls are off while a dialog is open (design m3 E.1): set together with
        // the overlay, before the next frame.
        self.window
            .global::<ui::Modal>()
            .set_open(state.overlay != OverlayKind::None);
        self.render_settings(&state, lang);
        self.render_change(&state, lang);
        self.render_session(&state, lang);
        self.render_update(&state, lang);
        // The restart, post-reboot, conflict, history and recovery pages (also without a read:
        // they say what is missing).
        self.journal_ui.render(&self.window, &state);
        // The first-run wizard (its first step needs no read).
        self.wizard_ui.render(&self.window, &state);
        let Some(read) = &state.read else {
            return;
        };
        let Some(snapshot) = &read.snapshot else {
            return;
        };
        let main = main_screen(&MainInput {
            snapshot,
            journal: read.journal.as_ref().zip(read.boot),
            summary: &read.summary,
            settings: &state.settings,
            active_hkl: state.active_hkl,
            highlighted: state.highlighted.as_deref(),
            lang,
        });
        let status = main.status;
        self.window.set_status(ui::StatusVm {
            input_method: status.input_method.into(),
            input_method_tone: tone(status.input_method_tone),
            mode: status.mode.into(),
            mode_note: status.mode_note.into(),
            standard: status.standard.into(),
            sign_in: status.sign_in.into(),
            sign_in_tone: tone(status.sign_in_tone),
            banner: status.banner.into(),
            banner_tone: tone(status.banner_tone),
            banner_action: status.banner_action.into(),
        });
        self.window
            .set_identify_announcement(main.announcement.into());
        self.update_keyboards(main.rows);
    }

    /// The settings page (design m3 B.14; WP-U7): the sign-in start shows the Run value itself
    /// (design m3 F.3), "restore on uninstall" the machine-wide value as last read.
    fn render_settings(&self, state: &AppState, lang: Lang) {
        let page = vm::settings::settings_page(&vm::settings::SettingsInput {
            settings: &state.settings,
            autostart: state.autostart.as_ref(),
            restore_on_uninstall: state.settings_page.restore_on_uninstall.as_ref(),
            confirming: state.settings_page.confirming,
            idle: state.session == SessionPhase::Idle,
            elevated: state.elevated,
            lang,
        });
        self.window.set_settings_autostart(page.autostart_on);
        self.window
            .set_settings_autostart_available(page.autostart_available);
        self.window
            .set_settings_autostart_note(page.autostart_note.into());
        self.window.set_settings_long_countdown(page.long_countdown);
        let confirm = page.uninstall.confirm.unwrap_or_default();
        self.window.set_settings_uninstall(ui::UninstallRestoreVm {
            status: page.uninstall.status.into(),
            note: page.uninstall.note.into(),
            can_change: page.uninstall.can_change,
            confirm_open: !confirm.title.is_empty(),
            confirm_title: confirm.title.into(),
            confirm_text: confirm.text.into(),
            uac_explanation: confirm.uac_explanation,
            uac_line: confirm.uac_line.into(),
            confirm_button: confirm.button.into(),
        });
        if state.page == Page::About {
            let about = vm::settings::about_page(env!("CARGO_PKG_VERSION"), BUILD_ID, lang);
            self.window.set_about_version(about.version.into());
            // A new model only when the lines changed (the language): the list is not rebuilt
            // on every render.
            let shown: Vec<SharedString> = self.window.get_about_licences().iter().collect();
            if shown
                .iter()
                .map(SharedString::as_str)
                .ne(about.licences.iter().map(String::as_str))
            {
                self.window.set_about_licences(ModelRc::new(VecModel::from(
                    about
                        .licences
                        .into_iter()
                        .map(SharedString::from)
                        .collect::<Vec<_>>(),
                )));
            }
        }
    }

    /// The change page (design m3 B.4, B.5; WP-U3), also for "今すぐ反映…" (design m3 B.12).
    fn render_change(&self, state: &AppState, lang: Lang) {
        let Some(draft) = &state.draft else {
            return;
        };
        let page = if draft.apply_now {
            let blocked = state
                .read
                .as_ref()
                .and_then(|read| vm::status::blocked_reason(&read.summary, lang));
            vm::keyboards::apply_now_page(&vm::keyboards::ApplyNowContext {
                draft,
                offered: !state::apply_now_targets(state).is_empty(),
                blocked: blocked.as_deref(),
                uac_notice_seen: state.settings.change.uac_notice_seen,
                elevated: state.elevated,
                lang,
            })
        } else {
            let display = state.read.as_ref().and_then(|read| read.snapshot.as_ref());
            let other_name = draft
                .other_keyboard
                .as_deref()
                .map(|id| state::display_name(state, id));
            vm::change::change_page(&vm::change::ChangeContext {
                draft,
                display,
                uac_notice_seen: state.settings.change.uac_notice_seen,
                elevated: state.elevated,
                seconds: vm::change::countdown_seconds(&state.settings),
                other_name: other_name.as_deref(),
                lang,
            })
        };
        let index = |index: Option<usize>| index.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
        // The radio groups' models are kept (updated in place), so that a render — a detection
        // key press, an input-language change, a read — does not rebuild the radio buttons.
        let choices = |kept: &KeptModel<ui::ChoiceVm>, rows: &[vm::change::ChoiceRow]| {
            kept.show(rows.iter().map(|row| ui::ChoiceVm {
                text: row.text.as_str().into(),
                detail: row.detail.as_str().into(),
                enabled: row.enabled,
            }))
        };
        self.window
            .set_change_keyboard_name(draft.name.as_str().into());
        self.window
            .set_change_choices(choices(&self.change_choices, &page.choices));
        self.window.set_change_method_choices(
            self.change_method_choices.show(
                [
                    (&page.live_text, &page.live_detail),
                    (&page.restart_text, &page.restart_detail),
                ]
                .map(|(text, detail)| ui::ChoiceVm {
                    text: text.as_str().into(),
                    detail: detail.as_str().into(),
                    enabled: true,
                }),
            ),
        );
        self.window.set_change_selected(index(page.selected));
        self.window
            .set_change_method(index(page.method.map(vm::change::ApplyMethod::index)));
        self.window
            .set_change_standard(index(page.standard_selected));
        self.window.set_change(ui::ChangeVm {
            title: page.title.into(),
            preparing: page.preparing,
            method_visible: page.method_visible,
            takes_effect: page.takes_effect.into(),
            all_users_note: page.all_users_note.into(),
            migration_note: page.migration_note.into(),
            ime_note: page.ime_note.into(),
            warnings: ModelRc::new(VecModel::from(
                page.warnings
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )),
            lines: ModelRc::new(VecModel::from(
                page.lines
                    .into_iter()
                    .map(|line| ui::PlanLineVm {
                        key: line.key.into(),
                        name: line.name.into(),
                        now: line.now.into(),
                        after: line.after.into(),
                    })
                    .collect::<Vec<_>>(),
            )),
            uac_line: page.uac_line.into(),
            apply_text: page.apply_text.into(),
            can_apply: page.can_apply,
            detect_instruction: page.detect.instruction.into(),
            detect_feedback: page.detect.feedback.into(),
            detect_feedback_tone: tone(page.detect.feedback_tone),
            detect_verdict: page.detect.verdict.into(),
            detect_choose_text: page.detect.choose_text.into(),
            // No layout choices, detection or "MKLM 導入前に戻す…" on the restore preview and on
            // "今すぐ反映…".
            restore_mode: page.restore || draft.apply_now,
            summary: page.summary.into(),
            standard_visible: page.standard_visible,
            standard_choices: choices(&self.change_standard_choices, &page.standard_choices),
            note: page.note.into(),
            note_tone: tone(page.note_tone),
            details: page.details.into(),
        });
    }

    /// The session overlays and the result (design m3 B.6, B.7, B.17; WP-U3).
    fn render_session(&self, state: &AppState, lang: Lang) {
        let empty = mklm_client::session::SessionView::default();
        let view = state::session_view(state).unwrap_or(&empty);
        match state.overlay {
            OverlayKind::Progress => {
                let progress = vm::session::progress(state::progress_stage(state), view, lang);
                self.window.set_progress(ui::ProgressVm {
                    title: progress.title.into(),
                    step: progress.step.into(),
                    detail: progress.detail.into(),
                });
            }
            OverlayKind::Countdown => {
                if let Some(countdown) = vm::session::countdown(view, lang) {
                    self.window.set_countdown(ui::CountdownVm {
                        title: countdown.title.into(),
                        message: countdown.message.into(),
                        remaining: i32::try_from(countdown.remaining).unwrap_or(i32::MAX),
                        total: i32::try_from(countdown.total).unwrap_or(i32::MAX),
                        recognition: countdown.recognition.into(),
                        recognition_tone: tone(countdown.recognition_tone),
                        reminder: countdown.reminder.into(),
                    });
                }
            }
            OverlayKind::Reconnect => {
                if let Some(reconnect) = vm::session::reconnect(view, lang) {
                    self.window.set_reconnect(ui::ReconnectVm {
                        title: reconnect.title.into(),
                        instructions: reconnect.instructions.into(),
                        status: reconnect.status.into(),
                        status_tone: tone(reconnect.status_tone),
                        can_keep: reconnect.can_keep,
                    });
                }
            }
            OverlayKind::Result => {
                if let Some(result) = vm::result::shown_result(state, lang) {
                    self.window.set_result(ui::ResultVm {
                        title: result.title.into(),
                        current_state: ModelRc::new(VecModel::from(
                            result
                                .current_state
                                .into_iter()
                                .map(SharedString::from)
                                .collect::<Vec<_>>(),
                        )),
                        message: result.message.into(),
                        tone: tone(result.tone),
                        ime_note: result.ime_note.into(),
                        run_once_note: result.run_once_note.into(),
                        next_step: result.next_step.into(),
                        details: result.details.into(),
                    });
                }
            }
            OverlayKind::RecoveryConfirm => {
                self.window
                    .set_recovery_after_countdown(state.recovery_question.unwrap_or(false));
            }
            OverlayKind::QuitConfirm => {
                let names = vm::session::reconnect_names(view);
                self.window
                    .set_quit_message(crate::i18n::quit_confirm(&names, lang).into());
            }
            // The update overlays: `render_update`.
            OverlayKind::None
            | OverlayKind::CloseNotice
            | OverlayKind::UpdateHandOff
            | OverlayKind::UpdateResult => {}
        }
    }

    /// Updates (design m5b E): the page, the banner, the settings section, the hand-off and the
    /// result, and the UAC explanation's line before an update.
    fn render_update(&self, state: &AppState, lang: Lang) {
        let clock = |unix: u64| local_minute(unix);
        let actions = |actions: Vec<vm::update::ActionView>| {
            ModelRc::new(VecModel::from(
                actions
                    .into_iter()
                    .map(|action| ui::UpdateActionVm {
                        action: action.action.index(),
                        text: action.text.into(),
                        accessible_label: action.label.into(),
                        primary: action.primary,
                        enabled: action.enabled,
                    })
                    .collect::<Vec<_>>(),
            ))
        };
        let strings = |lines: Vec<String>| {
            ModelRc::new(VecModel::from(
                lines
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            ))
        };
        let banner = vm::update::banner(state, lang).unwrap_or_default();
        self.window.set_update_banner(ui::UpdateBannerVm {
            text: banner.text.into(),
            tone: tone(banner.tone),
            actions: actions(banner.actions),
        });
        let section = vm::update::settings_section(state, &clock, lang);
        self.window.set_update_auto_check(section.auto_check);
        self.window.set_update_settings(ui::UpdateSettingsVm {
            last_check: section.last_check.into(),
            last_success: section.last_success.into(),
            stale: section.stale.into(),
            expired: section.expired.into(),
            can_check: section.can_check,
        });
        self.window
            .set_uac_location_line(vm::update::uac_location_line(state, lang).into());
        if state.page == Page::Update {
            let page = vm::update::update_page(state, &clock, lang);
            let percent = page.progress.unwrap_or(0);
            self.window.set_update_page(ui::UpdatePageVm {
                title: page.title.into(),
                status: page.status.into(),
                tone: tone(page.tone),
                version_line: page.version_line.into(),
                published: page.published.into(),
                links: actions(page.links),
                progress_text: page.progress_text.into(),
                progress_visible: page.progress.is_some(),
                progress: percent as f32 / 100.0,
                progress_label: page.progress_label.into(),
                progress_announcement: page.progress_announcement.into(),
                explanation: page.explanation.into(),
                info: strings(page.info),
                details: page.details.into(),
                buttons: actions(page.buttons),
            });
        }
        match state.overlay {
            OverlayKind::UpdateHandOff => {
                let (title, message) = vm::update::handoff(lang);
                self.window.set_handoff_title(title.into());
                self.window.set_handoff_message(message.into());
            }
            OverlayKind::UpdateResult => {
                if let Some(result) = vm::update::result_overlay(state, lang) {
                    self.window
                        .set_update_close_action(vm::update::UpdateAction::Close.index());
                    self.window.set_update_result(ui::UpdateResultVm {
                        title: result.title.into(),
                        message: result.message.into(),
                        tone: tone(result.tone),
                        details: result.details.into(),
                        actions: actions(result.actions),
                    });
                }
            }
            _ => {}
        }
    }

    /// Applies the difference between the shown rows and `rows` to the kept model.
    fn update_keyboards(&self, rows: Vec<KeyboardRow>) {
        let ops = list_ops(&self.shown_rows.borrow(), &rows, |row| row.id.as_str());
        for op in ops {
            match op {
                ListOp::Remove(index) => {
                    self.keyboards.remove(index);
                }
                ListOp::Insert(index, row) => self.keyboards.insert(index, keyboard_row_vm(row)),
                ListOp::Set(index, row) => self.keyboards.set_row_data(index, keyboard_row_vm(row)),
            }
        }
        debug_assert_eq!(self.keyboards.row_count(), rows.len());
        *self.shown_rows.borrow_mut() = rows;
    }
}

fn keyboard_row_vm(row: KeyboardRow) -> ui::KeyboardRowVm {
    ui::KeyboardRowVm {
        id: row.id.into(),
        name: row.name.into(),
        transport: row.transport.into(),
        vid_pid: row.vid_pid.into(),
        assigned: row.assigned.into(),
        pending: row.pending.into(),
        pending_hint: row.pending_hint.into(),
        apply_now: row.apply_now,
        current: row.current.into(),
        current_tone: tone(row.current_tone),
        physical_note: row.physical_note.into(),
        badges: ModelRc::new(VecModel::from(
            row.badges
                .into_iter()
                .map(|badge| ui::BadgeVm {
                    text: badge.text.into(),
                    tone: tone(badge.tone),
                    accessible_text: badge.accessible.into(),
                })
                .collect::<Vec<_>>(),
        )),
        highlighted: row.highlighted,
        hidden: row.hidden,
        can_assign: row.can_assign,
        blocked_note: row.blocked_note.into(),
        assign_label: row.assign_label.into(),
        apply_now_label: row.apply_now_label.into(),
        accessible_summary: row.accessible_summary.into(),
    }
}

fn tone(tone: vm::Tone) -> ui::Tone {
    match tone {
        vm::Tone::Neutral => ui::Tone::Neutral,
        vm::Tone::Info => ui::Tone::Info,
        vm::Tone::Success => ui::Tone::Success,
        vm::Tone::Warning => ui::Tone::Warning,
        vm::Tone::Danger => ui::Tone::Danger,
    }
}

fn screen(page: Page) -> ui::Screen {
    match page {
        Page::Wizard => ui::Screen::Wizard,
        Page::Main => ui::Screen::Main,
        Page::Change => ui::Screen::Change,
        Page::UacNotice => ui::Screen::UacNotice,
        Page::Restart => ui::Screen::Restart,
        Page::PostReboot => ui::Screen::PostReboot,
        Page::Conflict => ui::Screen::Conflict,
        Page::Journal => ui::Screen::Journal,
        Page::Recovery => ui::Screen::Recovery,
        Page::ImeHelp => ui::Screen::ImeHelp,
        Page::Settings => ui::Screen::Settings,
        Page::About => ui::Screen::About,
        Page::Update => ui::Screen::Update,
    }
}

/// A random part of at most `jitter` (the schedule of the update checks, design m5b E.1:
/// `mklm_win::session::random_bytes`; none when it fails).
fn random_part(jitter: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    if jitter.is_zero() || mklm_win::session::random_bytes(&mut bytes).is_err() {
        return Duration::ZERO;
    }
    Duration::from_secs(u64::from_le_bytes(bytes) % (jitter.as_secs() + 1))
}

fn overlay(overlay: OverlayKind) -> ui::Overlay {
    match overlay {
        OverlayKind::None => ui::Overlay::None,
        OverlayKind::Progress => ui::Overlay::Progress,
        OverlayKind::Countdown => ui::Overlay::Countdown,
        OverlayKind::Reconnect => ui::Overlay::Reconnect,
        OverlayKind::Result => ui::Overlay::Result,
        OverlayKind::RecoveryConfirm => ui::Overlay::RecoveryConfirm,
        OverlayKind::CloseNotice => ui::Overlay::CloseNotice,
        OverlayKind::QuitConfirm => ui::Overlay::QuitConfirm,
        OverlayKind::UpdateHandOff => ui::Overlay::UpdateHandOff,
        OverlayKind::UpdateResult => ui::Overlay::UpdateResult,
    }
}

fn page(screen: ui::Screen) -> Page {
    match screen {
        ui::Screen::Wizard => Page::Wizard,
        ui::Screen::Main => Page::Main,
        ui::Screen::Change => Page::Change,
        ui::Screen::UacNotice => Page::UacNotice,
        ui::Screen::Restart => Page::Restart,
        ui::Screen::PostReboot => Page::PostReboot,
        ui::Screen::Conflict => Page::Conflict,
        ui::Screen::Journal => Page::Journal,
        ui::Screen::Recovery => Page::Recovery,
        ui::Screen::ImeHelp => Page::ImeHelp,
        ui::Screen::Settings => Page::Settings,
        ui::Screen::About => Page::About,
        ui::Screen::Update => Page::Update,
    }
}

/// Shows a problem that stops MKLM (design m3 F.6) and returns the exit code: logged, and shown
/// in a message box in `lang` (`mklm_win::ui::error_dialog`), since the window may not exist and
/// release builds have no console. `detail` is the English diagnostic. No message box for
/// `--quit` (the installer may run unattended) or `--exit-after` (an automated check): nobody
/// would close it.
fn fatal(error: StartupError, detail: &str, lang: Lang) -> ExitCode {
    log::error(detail);
    if ERROR_DIALOGS.load(Ordering::Relaxed) {
        let (title, text) = i18n::startup_error(error, detail, lang);
        mklm_win::ui::error_dialog(&title, &text);
    }
    ExitCode::FAILURE
}

/// Whether [`fatal`] shows its message box (set once at start-up).
static ERROR_DIALOGS: AtomicBool = AtomicBool::new(true);

/// Runs the GUI until it quits (design m3 F).
pub fn run(args: Args) -> ExitCode {
    // First, before anything loads a DLL (design m3 F.7). The GUI runs unelevated: a failure is
    // only a warning (like the CLI, design m3 F.6), logged once the log is open.
    let dll_search = mklm_win::restrict_dll_search();
    ERROR_DIALOGS.store(
        args.start != StartMode::Quit && args.exit_after.is_none(),
        Ordering::Relaxed,
    );
    let log_dir = mklm_win::ui::user_log_dir();
    log::init(log_dir.as_ref().ok().cloned());
    log::info(format!(
        "MKLM {} (build {BUILD_ID}) starting ({:?})",
        env!("CARGO_PKG_VERSION"),
        args.start
    ));
    if let Err(error) = &log_dir {
        log::warn(format!("no log folder: {error}"));
    }
    // Until the settings are read, a problem is shown in the language of the command line or of
    // Windows.
    let early_lang = args
        .lang
        .unwrap_or(LangChoice::System)
        .resolve(mklm_win::ui::user_default_ui_language());
    if let Err(error) = dll_search {
        log::warn(format!("the DLL search could not be restricted: {error}"));
    }
    for unknown in &args.unknown {
        log::warn(format!("ignoring the argument {unknown:?}"));
    }
    match mklm_win::read_os_info(&mut Vec::new()) {
        Ok(os) if os.build < MIN_BUILD => {
            return fatal(
                StartupError::WindowsTooOld { build: os.build },
                &format!(
                    "MKLM needs Windows 11 24H2 (build {MIN_BUILD}) or later; this PC runs build {}",
                    os.build
                ),
                early_lang,
            );
        }
        Ok(_) => {}
        Err(error) => {
            return fatal(
                StartupError::WindowsUnknown,
                &format!("reading the Windows version failed: {error}"),
                early_lang,
            );
        }
    }
    // An update past `ready` waits for MKLM's programs to end (design m5b D.13 step 1): this start
    // ends at once, before it becomes the instance or shows a window that would lock mklm.exe.
    if args.start != StartMode::Quit
        && let Some(code) = update_start_gate(&args)
    {
        return code;
    }
    // One GUI per session (design m3 F.1): a second start hands over and ends here, and so does
    // `--quit`. The guard holds the instance mutex until the process ends.
    let _instance_guard = match single_instance::claim(args.start) {
        Claim::Run(guard) => guard,
        Claim::Exit(code) => {
            log::info("this process ends here (one MKLM per session)");
            return code;
        }
    };

    // Settings (design m3 F.4): read once, before the window; an unreadable file is kept as
    // settings.toml.bad at the next save.
    let settings_dir = match mklm_win::ui::user_settings_dir() {
        Ok(dir) => Some(dir),
        Err(error) => {
            log::warn(format!(
                "no settings folder ({error}); the settings are not saved"
            ));
            None
        }
    };
    let (settings, loaded) = match settings_dir.as_deref() {
        Some(dir) => load_or_default(dir),
        None => (Settings::default(), Ok(())),
    };
    if let Err(error) = &loaded {
        log::warn(format!(
            "settings.toml is unreadable ({error}); using the defaults, and keeping it as settings.toml.bad at the next save"
        ));
    }
    let store = settings_dir.map(|dir| SettingsStore::new(dir, loaded.is_err()));
    let lang = args
        .lang
        .unwrap_or(settings.language)
        .resolve(mklm_win::ui::user_default_ui_language());
    let theme_mode = args.theme.unwrap_or(settings.theme);
    let os_dark = mklm_win::ui::theme::read_os_theme()
        .ok()
        .map(|theme| theme.dark);
    let initial_dark = theme_mode.resolve(os_dark);

    // The window is created with its theme (with_theme(Some)), so that winit keeps it through
    // unrelated WM_SETTINGCHANGE broadcasts (M0 #8 D, prototype README 11 and 13).
    let selected = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(args.renderer.slint_name().into())
        .with_winit_window_attributes_hook(move |attributes| {
            attributes.with_theme(Some(if initial_dark {
                WinitTheme::Dark
            } else {
                WinitTheme::Light
            }))
        })
        .with_winit_custom_application_handler(InputCapture::default())
        .select();
    if let Err(error) = selected {
        return fatal(
            StartupError::CannotStart,
            &format!("the window system could not start: {error}"),
            lang,
        );
    }
    // The single-instance pipe as early as commands can be handed to the UI thread (once the
    // backend is selected): a second start at sign-in (Run and RunOnce together) retries for
    // only 5 s (design m3 F.1). Its commands wait in Slint's queue until the event loop runs;
    // the pipe thread waits long enough for the window to be built (`UI_REPLY_WAIT`).
    let instance = InstanceService::start(instance_command);
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => {
            return fatal(
                StartupError::CannotStart,
                &format!("the window could not be created: {error}"),
                lang,
            );
        }
    };
    if let Err(error) = slint::select_bundled_translation(lang.bundle()) {
        log::warn(format!("language {:?}: {error}", lang.bundle()));
    }
    window
        .global::<ui::Theme>()
        .set_font_family(lang.font_family().into());
    let icon = crate::icon::app_icon();
    window.set_app_icon(icon.clone());
    window.set_settings_theme_index(theme_mode.index());
    window.set_settings_language_index(args.lang.unwrap_or(settings.language).index());

    // An elevated GUI runs (design m3 F.2) but never registers the post-reboot check itself.
    let elevation = mklm_win::elevation::is_elevated();
    MAY_REGISTER_POST_REBOOT.store(matches!(elevation, Ok(false)), Ordering::SeqCst);
    let io = match IoWorker::start(store) {
        Ok(io) => io,
        Err(error) => {
            return fatal(
                StartupError::CannotStart,
                &format!("the I/O worker could not start: {error}"),
                lang,
            );
        }
    };
    // `--tray` (sign-in) and `--post-reboot` (RunOnce) start in the taskbar corner once the
    // wizard is done; the first read of the journal shows the window when it needs the user
    // (design m3 F.2). `--post-reboot` with nothing to check is `--tray`: the first read opens
    // the check, in front, when there is one.
    // `--after-update` too: the update worker's first answer shows the window when there is a
    // result to show (design m5b E.5).
    let start_hidden = matches!(
        args.start,
        StartMode::Tray | StartMode::PostReboot | StartMode::AfterUpdate
    ) && settings.wizard.completed;
    // Every helper session first reports the user's newest verified update information when the
    // machine's record is behind (design m5b C.4); the answer is only logged (E.1).
    let _ = mklm_client::session::set_trust_reporter(Box::new(
        mklm_client::update::trust_report::CurrentUser {
            on_answer: Some(log_trust_answer),
        },
    ));
    let update_worker = match UpdateWorker::start(update_endpoints(&args)) {
        Ok(worker) => Some(worker),
        Err(error) => {
            log::warn(format!("the update worker could not start: {error}"));
            None
        }
    };
    let mut journal_pages = crate::state::JournalPages::default();
    journal_pages.post_reboot.requested = args.start == StartMode::PostReboot;
    // The first-run wizard opens until it is finished or skipped, `--tray` too (design m3 B.1).
    let wizard = (!settings.wizard.completed).then(WizardState::default);
    let controller = Rc::new(Controller {
        window: window.clone_strong(),
        icon,
        tray: RefCell::new(None),
        state: RefCell::new(AppState {
            lang: Some(lang),
            page: if wizard.is_some() {
                Page::Wizard
            } else {
                Page::Main
            },
            wizard,
            settings,
            visible: !start_hidden,
            reveal_on_attention: start_hidden,
            // An elevated GUI shows no UAC prompt, so none is explained (design m3 B.5); when it
            // cannot be told, the explanation is shown (harmless).
            elevated: elevation.unwrap_or(false),
            journal_pages,
            update: crate::state::UpdateState {
                after_update: args.start == StartMode::AfterUpdate,
                ..crate::state::UpdateState::default()
            },
            ..AppState::default()
        }),
        io,
        session: RefCell::new(None),
        watchers: RefCell::new(Watchers::default()),
        instance: RefCell::new(instance),
        theme_mode: Cell::new(theme_mode),
        os_dark: Cell::new(os_dark),
        keyboards: Rc::new(VecModel::default()),
        shown_rows: RefCell::new(Vec::new()),
        change_choices: KeptModel::default(),
        change_standard_choices: KeptModel::default(),
        change_method_choices: KeptModel::default(),
        layout_timer: slint::Timer::default(),
        settle_timer: slint::Timer::default(),
        journal_ui: JournalUi::new(&window),
        wizard_ui: WizardUi::new(&window),
        automated: args.exit_after.is_some(),
        update_worker,
        check_timer: slint::Timer::default(),
        handoff_timer: slint::Timer::default(),
    });
    APP.with(|app| *app.borrow_mut() = Some(controller.clone()));
    window.set_keyboards(ModelRc::from(controller.keyboards.clone()));
    // Windows' text size (design m3 E.4): Slint does not apply it by itself.
    if let Ok(scale) = mklm_win::ui::theme::read_text_scale_factor() {
        window.global::<ui::Theme>().set_font_scale(scale as f32);
    }

    wire_callbacks(&window);
    window.window().on_close_requested(|| {
        // Without a tray icon there would be no way back to a hidden window: quit instead
        // (design m3 B.16). The rules of design m3 F.5 are in `state::update`, which hides the
        // window itself when it goes to the tray: the first × keeps it for the in-window
        // notice, and a × during a session never closes it.
        let has_tray = current_app().is_some_and(|app| app.tray.borrow().is_some());
        dispatch(if has_tray {
            AppMsg::WindowCloseRequested
        } else {
            AppMsg::QuitRequested
        });
        CloseRequestResponse::KeepWindowShown
    });
    match tray::create_tray(&controller.icon) {
        Ok(tray) => *controller.tray.borrow_mut() = Some(tray),
        Err(error) => log::warn(format!("the tray icon could not be created: {error}")),
    }
    let (watchers, warnings) = Watchers::start(
        |os| {
            let dark = os.dark;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = current_app() {
                    app.os_dark.set(Some(dark));
                    app.apply_theme();
                }
            });
        },
        shell_event,
        |scale| {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = current_app() {
                    app.window
                        .global::<ui::Theme>()
                        .set_font_scale(scale as f32);
                }
            });
        },
        // Called on the watcher's dispatcher thread: only schedules; the state gathers a burst
        // for 750 ms and then reads on the I/O worker (design m3 A.4).
        |_change| post(AppMsg::KeyboardsChanged),
    );
    for warning in warnings {
        log::warn(format!("watcher: {warning}"));
    }
    *controller.watchers.borrow_mut() = watchers;

    // The title bar once the winit window exists (it is created by the event loop).
    let weak = window.as_weak();
    let _ = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else { return };
        let created = window.window().winit_window().await.is_ok();
        drop(window);
        if created && let Some(app) = current_app() {
            app.apply_theme();
        }
    });

    // Automated checks end through the normal quit path (design m3 F.2, F.5).
    let exit_timer = slint::Timer::default();
    if let Some(after) = args.exit_after {
        exit_timer.start(slint::TimerMode::SingleShot, after, || {
            dispatch(AppMsg::QuitRequested);
        });
    }

    controller.run(Effect::Read);
    // Updates: the environment, the records and what a start shows (design m5b D.13, E.1).
    controller.run(Effect::Update(crate::state::UpdateTask::Start));
    if args.exit_after.is_none() {
        // The per-user writes of a start: the post-reboot RunOnce rule (design m2 C17, m3 A.4;
        // a restart may be pending from a change that did not register the check, from another
        // front end or an elevated run) and the autostart repair (F.3).
        controller.run(Effect::RunOnceRule);
        controller.run(Effect::Autostart(AutostartTask::Repair));
    } else {
        // An automated check (`--exit-after`) only reads: it must not change the user's HKCU.
        log::info("automated check: the start-up RunOnce rule and autostart repair are skipped");
        controller.run(Effect::Autostart(AutostartTask::Read));
    }
    if start_hidden {
        controller.run(Effect::Render);
    } else {
        // Shows the window, starts the input-language poll (design m3 A.4 rule 5), renders.
        controller.run(Effect::ShowWindow);
        controller.run(Effect::Render);
    }
    let result = slint::run_event_loop_until_quit();

    // Tear down on the UI thread in a defined order (design m3 F.5): the tray first, so that it
    // leaves the taskbar corner; the watchers; the single-instance pipe. A running session
    // worker is left to finish by itself (its RunOnce rule runs before its end is posted). The
    // I/O worker's queued writes get up to 1 s. A running check or download stops.
    controller.tray.borrow_mut().take();
    if let Some(worker) = &controller.update_worker {
        worker.cancel();
    }
    *controller.watchers.borrow_mut() = Watchers::default();
    controller.instance.borrow_mut().take();
    if !reader::wait_for_writes(QUIT_IO_WAIT) {
        log::warn("quitting before every queued settings or registry write was done");
    }
    APP.with(|app| app.borrow_mut().take());
    match result {
        Ok(()) => {
            log::info("MKLM ended");
            ExitCode::SUCCESS
        }
        Err(error) => fatal(
            StartupError::Stopped,
            &format!("the event loop failed: {error}"),
            lang,
        ),
    }
}

/// The release endpoints: GitHub; in development builds with `--cfg mklm_update_dev` maybe a
/// loopback rehearsal server (`--update-endpoint`, design m5b A.10, E.8).
fn update_endpoints(args: &Args) -> mklm_update::url::Endpoints {
    #[cfg(all(debug_assertions, mklm_update_dev))]
    if let Some(base) = &args.update_endpoint {
        match mklm_update::url::Endpoints::loopback(base) {
            Ok(endpoints) => return endpoints,
            Err(error) => log::warn(format!("--update-endpoint: {error}")),
        }
    }
    let _ = args;
    mklm_update::url::Endpoints::production()
}

/// The helper's answer to `RecordTrust` (design m5b C.4, E.1: logged only).
fn log_trust_answer(reply: &mklm_ipc::UpdateMessage) {
    log::info(format!("the helper answered the trust report: {reply:?}"));
}

/// The start while an update runs (design m5b D.13 step 1): `Some(exit code)` to end at once.
/// Another session's update: this user's after-update value (unelevated) and `closed_by_update`
/// first, so that the next start says why MKLM was away (FIX-VERIFICATION-9). What to do is
/// `state::update::start_gate_effects`; only its I/O is here.
fn update_start_gate(args: &Args) -> Option<ExitCode> {
    use crate::state::update::{start_gate, start_gate_effects};

    let (record, view) = mklm_client::update::status::current_run()?;
    let my_session = mklm_win::proc_identity::process_users()
        .ok()
        .and_then(|users| users.get(&std::process::id()).map(|user| user.session_id));
    let gate = start_gate(&view, record.caller_session, my_session)?;
    log::info(format!(
        "an update to {} is running ({view:?}); MKLM does not start now ({gate:?})",
        record.to_version
    ));
    if args.exit_after.is_some() {
        return Some(ExitCode::SUCCESS);
    }
    // An elevated GUI registers no RunOnce value (design m5b D.10, m3 F.2); nor one that cannot
    // tell.
    let elevated = !matches!(mklm_win::elevation::is_elevated(), Ok(false));
    let mut store = None;
    let effects =
        start_gate_effects(
            gate,
            elevated,
            unix_now(),
            || match mklm_win::ui::user_settings_dir() {
                Ok(dir) => {
                    let (settings, loaded) = load_or_default(&dir);
                    store = Some(SettingsStore::new(dir, loaded.is_err()));
                    Some(settings)
                }
                Err(error) => {
                    log::warn(format!("no settings folder: {error}"));
                    None
                }
            },
        );
    for effect in effects {
        match effect {
            Effect::AfterUpdateRunOnce(register) => crate::reader::after_update_run_once(register),
            Effect::SaveSettings(settings) => {
                if let Some(store) = store.as_mut()
                    && let Err(error) = store.save(&settings)
                {
                    log::warn(format!("saving the settings failed: {error}"));
                }
            }
            other => log::warn(format!("start gate: {other:?} is not carried out here")),
        }
    }
    Some(ExitCode::SUCCESS)
}

/// The shell window's broadcasts (UI thread, inside its window procedure; design m3 A.4 rule 3).
/// Work is scheduled with `invoke_from_event_loop`, except at the end of the Windows session
/// (design m3 F.5, review A5): a scheduled closure may not run before the session ends.
fn shell_event(event: mklm_win::ui::shell_window::ShellEvent) {
    use mklm_win::ui::shell_window::ShellEvent;
    match event {
        ShellEvent::TaskbarCreated => {
            let _ = slint::invoke_from_event_loop(|| {
                if let Some(app) = current_app() {
                    app.tray.borrow_mut().take();
                    match tray::create_tray(&app.icon) {
                        Ok(tray) => *app.tray.borrow_mut() = Some(tray),
                        Err(error) => {
                            log::warn(format!("the tray icon could not be re-created: {error}"));
                        }
                    }
                    app.update_tray();
                }
            });
        }
        ShellEvent::SettingChange(area) if area == "intl" => post(AppMsg::Refresh),
        // "ImmersiveColorSet" (the app mode), and whatever else changed: high contrast is read
        // again at every WM_SETTINGCHANGE (design m3 C.2) — turning it on or off, or another
        // contrast theme, arrives as a broadcast whose area names no colour set. Cheap: two
        // system calls, and properties that did not change redraw nothing.
        ShellEvent::SettingChange(_) => {
            let _ = slint::invoke_from_event_loop(|| {
                if let Some(app) = current_app() {
                    app.apply_theme();
                }
            });
        }
        ShellEvent::Resumed => {
            post(AppMsg::Refresh);
            // A check whose time passed during sleep comes 30 s later (design m5b E.1).
            post(AppMsg::Update(UpdateMsg::Resumed));
        }
        // Directly: the cancel flag is an atomic outside any RefCell. A countdown is reverted at
        // the relay's next tick; a reconnect wait is left open (it stays waiting for the user).
        ShellEvent::QueryEndSession => {
            log::info("the Windows session may end");
            if worker::cancel_current_session() {
                // An elevated GUI does not register (design m3 F.2, `RunOnceOutcome::TellUser`):
                // its HKCU may be another administrator's. The journal still asks for the check
                // at the next start of MKLM.
                if !MAY_REGISTER_POST_REBOOT.load(Ordering::SeqCst) {
                    log::info(
                        "an open change, but elevated: the post-reboot check is not registered",
                    );
                    return;
                }
                // A journaled change is open: make sure the next sign-in shows it, whatever the
                // journal says at this instant (one short HKCU write).
                let registered = mklm_client::run_once::PostRebootCommand::Gui
                    .command_line()
                    .map_err(|error| error.to_string())
                    .and_then(|line| {
                        mklm_win::session::register_post_reboot(&line)
                            .map_err(|error| error.to_string())
                    });
                match registered {
                    Ok(()) => log::info("an open change: the post-reboot check is registered"),
                    Err(error) => {
                        log::warn(format!("registering the post-reboot check failed: {error}"))
                    }
                }
            }
        }
        // The process may be ended as soon as this returns: one bounded wait of 3 s in all (the
        // session worker, then the queued writes with what is left), then quit.
        ShellEvent::EndSession => {
            log::info("the Windows session ends");
            let started = Instant::now();
            if !worker::wait_for_current_session(END_SESSION_WAIT) {
                log::warn("the Windows session ends while a helper session is still open");
            }
            let left = END_SESSION_WAIT
                .saturating_sub(started.elapsed())
                .min(QUIT_IO_WAIT);
            if !reader::wait_for_writes(left) {
                log::warn("the Windows session ends before every queued write was done");
            }
            let _ = slint::quit_event_loop();
        }
    }
}

/// Connects the window's callbacks to messages. Flows not built yet dispatch nothing.
fn wire_callbacks(window: &AppWindow) {
    window.on_navigate(|screen| dispatch(AppMsg::Navigate(page(screen))));
    window.on_refresh(|| dispatch(AppMsg::Refresh));
    window.on_toggle_identify(|| dispatch(AppMsg::ToggleIdentify));
    // The main screen (design m3 B.2, B.12; WP-U1).
    window.on_banner_action(|| dispatch(AppMsg::BannerAction { at: Instant::now() }));
    // Every row offers the same page: `Request::Recover` resets every keyboard the journal lists
    // at once.
    window.on_apply_now(|_row| dispatch(AppMsg::ApplyNow { at: Instant::now() }));
    window.on_toggle_hidden(|row: SharedString| dispatch(AppMsg::ToggleHidden(row.to_string())));
    window.on_show_hidden_toggled(|on| dispatch(AppMsg::ShowHidden(on)));
    window.on_key_pressed(|text: SharedString, shift| {
        dispatch(AppMsg::KeyTestPressed {
            text: text.to_string(),
            shift,
        });
    });
    window.on_quit_requested(|| dispatch(AppMsg::QuitRequested));
    window.on_close_notice_ok(|| dispatch(AppMsg::CloseNoticeAnswered { quit: false }));
    window.on_close_notice_quit(|| dispatch(AppMsg::CloseNoticeAnswered { quit: true }));
    window.on_recover_after_loss(|yes| dispatch(AppMsg::AnswerRecovery(yes)));
    window.on_open_settings_page(|index| {
        use mklm_win::ui::SettingsPage;
        let page = match index {
            0 => SettingsPage::RegionLanguage,
            1 => SettingsPage::KeyboardAdvanced,
            2 => SettingsPage::JapaneseIme,
            3 => SettingsPage::IntlControlPanel,
            4 => SettingsPage::Taskbar,
            _ => SettingsPage::SignInOptions,
        };
        if let Some(app) = current_app() {
            app.io.send(IoTask::OpenSettingsPage(page));
        }
    });
    // Applied at once (design m3 C.1, D.3) and saved in settings.toml (F.4).
    window.on_theme_selected(|index| {
        let theme = ThemeMode::from_index(index);
        if let Some(app) = current_app() {
            app.theme_mode.set(theme);
            app.apply_theme();
        }
        dispatch(AppMsg::ThemeChosen(theme));
    });
    window.on_language_selected(|index| {
        let choice = LangChoice::from_index(index);
        let lang = choice.resolve(mklm_win::ui::user_default_ui_language());
        if let Err(error) = slint::select_bundled_translation(lang.bundle()) {
            log::warn(format!("language {:?}: {error}", lang.bundle()));
        }
        if let Some(app) = current_app() {
            app.window
                .global::<ui::Theme>()
                .set_font_family(lang.font_family().into());
        }
        dispatch(AppMsg::LanguageChosen { choice, lang });
    });
    window.on_autostart_toggled(|on| dispatch(AppMsg::AutostartToggled(on)));
    // The rest of the settings page (design m3 B.14; WP-U7). Only "uninstall-confirm" can lead to
    // a UAC prompt, and only through `state::update` (one session at a time).
    window.on_long_countdown_toggled(|on| {
        dispatch(AppMsg::Settings(SettingsMsg::LongCountdown(on)));
    });
    window.on_uninstall_change(|| dispatch(AppMsg::Settings(SettingsMsg::UninstallChange)));
    window.on_uninstall_cancel(|| dispatch(AppMsg::Settings(SettingsMsg::UninstallCancel)));
    window.on_uninstall_confirm(|| dispatch(AppMsg::Settings(SettingsMsg::UninstallConfirm)));
    window.on_restore_all(|| dispatch(AppMsg::Settings(SettingsMsg::RestoreAll)));
    wire_updates(window);
    wire_change_flow(window);
    // The restart, post-reboot, conflict, history and recovery pages.
    crate::journal_ui::wire(window);
    // The first-run wizard, and Settings' "初回セットアップをもう一度行う" (WP-U2).
    crate::wizard_ui::wire(window);
}

/// The update callbacks (design m5b E): the page's, the banner's and the result's buttons, the
/// settings section, the hand-off's "OK".
fn wire_updates(window: &AppWindow) {
    use vm::update::UpdateAction;
    window.on_update_action(|index| {
        let Some(action) = UpdateAction::from_index(index) else {
            return;
        };
        let from_result = current_app().is_some_and(|app| {
            app.state
                .try_borrow()
                .is_ok_and(|state| state.overlay == OverlayKind::UpdateResult)
        });
        match action {
            // Out of the result, to the page's button (which decides whether it can).
            UpdateAction::UpdateNow if from_result => {
                dispatch(AppMsg::Update(UpdateMsg::ResultClosed));
                dispatch(AppMsg::Update(UpdateMsg::OpenPage));
                dispatch(AppMsg::Update(UpdateMsg::UpdateNow));
            }
            // The result's other buttons act on what it shows, then close it ("詳細をコピー"
            // keeps it open).
            UpdateAction::Close | UpdateAction::CopyDetails => {
                dispatch(AppMsg::Update(action.message()));
            }
            _ if from_result => {
                dispatch(AppMsg::Update(action.message()));
                dispatch(AppMsg::Update(UpdateMsg::ResultClosed));
            }
            _ => dispatch(AppMsg::Update(action.message())),
        }
    });
    window.on_update_auto_check_toggled(|on| dispatch(AppMsg::Update(UpdateMsg::AutoCheck(on))));
    window.on_update_check_now(|| dispatch(AppMsg::Update(UpdateMsg::CheckNow)));
    window.on_open_update_page(|| dispatch(AppMsg::Update(UpdateMsg::OpenPage)));
    window.on_handoff_ok(|| dispatch(AppMsg::Update(UpdateMsg::HandOffClosed)));
}

/// The change flow's callbacks (design m3 B.3 to B.7, B.17; WP-U3).
fn wire_change_flow(window: &AppWindow) {
    let index = |index: i32| usize::try_from(index).ok();
    window.on_assign(|row: SharedString| dispatch(AppMsg::OpenChange(row.to_string())));
    window.on_change_choose(move |i| {
        if let Some(i) = index(i) {
            dispatch(AppMsg::ChangeChoose(i));
        }
    });
    window.on_change_choose_method(move |i| {
        if let Some(i) = index(i) {
            dispatch(AppMsg::ChangeChooseMethod(i));
        }
    });
    window.on_change_choose_standard(move |i| {
        if let Some(i) = index(i) {
            dispatch(AppMsg::ChangeChooseStandard(i));
        }
    });
    window.on_change_restart_detection(|| dispatch(AppMsg::ChangeRestartDetection));
    window.on_change_use_detected(|| dispatch(AppMsg::ChangeUseDetected));
    window.on_restore_baseline(|| dispatch(AppMsg::OpenRestore));
    window.on_change_apply(|| dispatch(AppMsg::ChangeApply));
    window.on_uac_go(|| dispatch(AppMsg::UacGo));
    window.on_cancel_change(|| dispatch(AppMsg::CancelChange));
    window.on_keep(|| dispatch(AppMsg::KeepChange));
    window.on_revert(|| dispatch(AppMsg::RevertChange));
    window.on_decide_later(|| dispatch(AppMsg::DecideLater));
    window.on_result_closed(|| dispatch(AppMsg::ResultClosed));
    window.on_result_next_step(|| dispatch(AppMsg::ResultNextStep));
    window.on_copy_details(|| dispatch(AppMsg::CopyDetails));
    window.on_quit_revert(|| dispatch(AppMsg::QuitConfirmAnswered(QuitAnswer::RevertAndQuit)));
    window.on_quit_leave(|| dispatch(AppMsg::QuitConfirmAnswered(QuitAnswer::QuitLeavingIt)));
    window.on_quit_cancel(|| dispatch(AppMsg::QuitConfirmAnswered(QuitAnswer::Cancel)));
}
