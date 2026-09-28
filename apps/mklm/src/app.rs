//! The UI thread (design m3 A.4, A.6): start-up, the Slint event loop, and the controller that
//! turns callbacks and worker messages into [`AppMsg`]s, runs [`state::update`] and carries out
//! the [`Effect`]s. Nothing here blocks: reads go to the I/O worker ([`IoWorker`]), helper
//! sessions to a session worker ([`SessionWorker`]).

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mklm_client::launch::LaunchConfig;
use mklm_client::orchestrator::{LaunchError, LaunchFailure, RequestEnd, RequestReport};
use mklm_client::run_once::RunOnceError;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::Theme as WinitTheme;
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::args::{Args, StartMode};
use crate::i18n::{Lang, LangChoice};
use crate::input_capture::InputCapture;
use crate::journal_ui::JournalUi;
use crate::reader::{IoTask, IoWorker};
use crate::settings::Settings;
use crate::state::{
    self, AppMsg, AppState, Effect, OverlayKind, Page, QuitAnswer, SessionId, SessionOutcome,
};
use crate::theme::{ResolvedTheme, ThemeMode, is_dark_rgb};
use crate::tray;
use crate::ui::{self, AppWindow, TrayIcon};
use crate::vm::keyboards::{KeyboardRow, MainInput, main_screen};
use crate::vm::{self, ListOp, keytest, list_ops};
use crate::watchers::Watchers;
use crate::worker::{self, SessionWorker};
use crate::{BUILD_ID, MIN_BUILD};

/// How long the end of the Windows session waits for a running helper session (design m3 F.5).
const END_SESSION_WAIT: Duration = Duration::from_secs(3);

thread_local! {
    static APP: RefCell<Option<Rc<Controller>>> = const { RefCell::new(None) };
}

/// Handles `msg` now (UI thread only). The controller is cloned out of the thread-local first,
/// so no borrow is held while it runs.
pub fn dispatch(msg: AppMsg) {
    let app = APP.with(|app| app.borrow().clone());
    if let Some(app) = app {
        app.handle(msg);
    }
}

/// Hands `msg` to the UI thread (any thread). Dropped when the event loop has ended.
pub fn post(msg: AppMsg) {
    let _ = slint::invoke_from_event_loop(move || dispatch(msg));
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
    settings_dir: Option<std::path::PathBuf>,
    theme_mode: Cell<ThemeMode>,
    os_dark: Cell<Option<bool>>,
    /// The keyboard list's model, kept for the window's lifetime and updated row by row, so
    /// that a render does not rebuild the rows and drop the focus (design m3 A.6, review A7).
    keyboards: Rc<VecModel<ui::KeyboardRowVm>>,
    /// The rows the model shows now (to diff against).
    shown_rows: RefCell<Vec<KeyboardRow>>,
    /// Polls the input language while the window is visible (design m3 A.4 rule 5).
    layout_timer: slint::Timer,
    /// Gathers keyboard arrivals and removals before the list is read again (design m3 B.2).
    settle_timer: slint::Timer,
    /// The restart, post-reboot, conflict, history and recovery pages (WP-U4, WP-U5).
    journal_ui: JournalUi,
}

impl std::fmt::Debug for Controller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller").finish_non_exhaustive()
    }
}

impl Controller {
    fn handle(&self, msg: AppMsg) {
        let effects = {
            let mut state = self.state.borrow_mut();
            state.now = Some(std::time::Instant::now());
            state::update(&mut state, msg)
        };
        for effect in effects {
            self.run(effect);
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
                // WP-W2: `copy_text_to_clipboard` (English diagnostics only, design m3 B.17).
                if let Err(error) = mklm_win::ui::copy_text_to_clipboard(&text) {
                    eprintln!("mklm: warning: copying the details failed: {error}");
                }
            }
            Effect::ScheduleSettle(delay) => {
                self.settle_timer
                    .start(slint::TimerMode::SingleShot, delay, || {
                        dispatch(AppMsg::KeyboardsSettled);
                    });
            }
            Effect::SaveSettings(settings) => {
                if let Some(dir) = &self.settings_dir {
                    self.io.send(IoTask::SaveSettings {
                        settings,
                        dir: dir.clone(),
                    });
                }
            }
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
                let _ = slint::quit_event_loop();
            }
            Effect::Render => self.render(),
            Effect::Journal(effect) => self.journal_ui.run(&self.window, &self.io, effect),
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
            return;
        }
        let started = LaunchConfig::current(BUILD_ID, self.hwnd())
            .map_err(|error| error.to_string())
            .and_then(|launch| {
                SessionWorker::start(session, request, apply, launch)
                    .map_err(|error| format!("the session worker could not start: {error}"))
            });
        match started {
            Ok(worker) => *self.session.borrow_mut() = Some(worker),
            Err(message) => post(AppMsg::SessionEnded {
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
            }),
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
        self.render_change(&state, lang);
        self.render_session(&state, lang);
        // The restart, post-reboot, conflict, history and recovery pages (also without a read:
        // they say what is missing).
        self.journal_ui.render(&self.window, &state);
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
        let choices = |rows: &[vm::change::ChoiceRow]| {
            ModelRc::new(VecModel::from(
                rows.iter()
                    .map(|row| ui::ChoiceVm {
                        text: row.text.as_str().into(),
                        detail: row.detail.as_str().into(),
                        enabled: row.enabled,
                    })
                    .collect::<Vec<_>>(),
            ))
        };
        self.window
            .set_change_keyboard_name(draft.name.as_str().into());
        self.window.set_change_choices(choices(&page.choices));
        self.window.set_change_selected(index(page.selected));
        self.window
            .set_change_method(index(page.method.map(vm::change::ApplyMethod::index)));
        self.window
            .set_change_standard(index(page.standard_selected));
        self.window.set_change(ui::ChangeVm {
            title: page.title.into(),
            preparing: page.preparing,
            method_visible: page.method_visible,
            live_text: page.live_text.into(),
            live_detail: page.live_detail.into(),
            restart_text: page.restart_text.into(),
            restart_detail: page.restart_detail.into(),
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
            standard_choices: choices(&page.standard_choices),
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
            OverlayKind::None | OverlayKind::CloseNotice => {}
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
    }
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
    }
}

/// Shows a start-up problem and returns the exit code. WP-W2 / WP-U6: `mklm_win::ui::error_dialog`
/// (a message box), since the window may not exist yet and release builds have no console
/// (design m3 F.6).
fn fatal(message: &str) -> ExitCode {
    eprintln!("mklm: {message}");
    ExitCode::FAILURE
}

/// Runs the GUI until it quits (design m3 F).
pub fn run(args: Args) -> ExitCode {
    if let Err(error) = mklm_win::restrict_dll_search() {
        eprintln!("mklm: warning: DLL search could not be restricted: {error}");
    }
    for unknown in &args.unknown {
        eprintln!("mklm: ignoring the argument {unknown:?}");
    }
    match mklm_win::read_os_info(&mut Vec::new()) {
        Ok(os) if os.build < MIN_BUILD => {
            return fatal(&format!(
                "MKLM needs Windows 11 24H2 (build {MIN_BUILD}) or later; this PC runs build {}",
                os.build
            ));
        }
        Ok(_) => {}
        Err(error) => return fatal(&format!("reading the Windows version failed: {error}")),
    }
    if args.start == StartMode::Quit {
        // WP-W1: `mklm_win::instance::send_to_instance(Quit)`.
        eprintln!("mklm: --quit is not implemented yet (WP-W1)");
        return ExitCode::SUCCESS;
    }
    // WP-W1 / WP-U6: the single instance (`single_instance`) comes here.

    let settings_dir = mklm_win::ui::user_settings_dir().ok();
    let settings = settings_dir
        .as_deref()
        .and_then(|dir| match Settings::load(dir) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("mklm: warning: settings.toml is unreadable ({error}); using defaults");
                None
            }
        })
        .unwrap_or_default();
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
        return fatal(&format!("the window system could not start: {error}"));
    }
    let window = match AppWindow::new() {
        Ok(window) => window,
        Err(error) => return fatal(&format!("the window could not be created: {error}")),
    };
    if let Err(error) = slint::select_bundled_translation(lang.bundle()) {
        eprintln!("mklm: warning: language {:?}: {error}", lang.bundle());
    }
    window
        .global::<ui::Theme>()
        .set_font_family(lang.font_family().into());
    let icon = crate::icon::app_icon();
    window.set_app_icon(icon.clone());
    window.set_about_version(format!("{} (build {BUILD_ID})", env!("CARGO_PKG_VERSION")).into());
    window.set_settings_theme_index(theme_mode.index());
    window.set_settings_language_index(args.lang.unwrap_or(settings.language).index());

    let io = match IoWorker::start() {
        Ok(io) => io,
        Err(error) => return fatal(&format!("the I/O worker could not start: {error}")),
    };
    // `--post-reboot` with nothing to check is `--tray` (design m3 F.2): the first read opens the
    // check, in front, when there is one.
    let start_hidden =
        matches!(args.start, StartMode::Tray | StartMode::PostReboot) && settings.wizard.completed;
    let mut journal_pages = crate::state::JournalPages::default();
    journal_pages.post_reboot.requested = args.start == StartMode::PostReboot;
    let controller = Rc::new(Controller {
        window: window.clone_strong(),
        icon,
        tray: RefCell::new(None),
        state: RefCell::new(AppState {
            lang: Some(lang),
            settings,
            visible: !start_hidden,
            // An elevated GUI shows no UAC prompt, so none is explained (design m3 B.5); when it
            // cannot be told, the explanation is shown (harmless).
            elevated: mklm_win::elevation::is_elevated().unwrap_or(false),
            journal_pages,
            ..AppState::default()
        }),
        io,
        session: RefCell::new(None),
        watchers: RefCell::new(Watchers::default()),
        settings_dir,
        theme_mode: Cell::new(theme_mode),
        os_dark: Cell::new(os_dark),
        keyboards: Rc::new(VecModel::default()),
        shown_rows: RefCell::new(Vec::new()),
        layout_timer: slint::Timer::default(),
        settle_timer: slint::Timer::default(),
        journal_ui: JournalUi::new(&window),
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
        // (design m3 B.16). The session rules of design m3 F.5 are in `state::update`.
        let has_tray = APP
            .with(|app| app.borrow().clone())
            .is_some_and(|app| app.tray.borrow().is_some());
        dispatch(if has_tray {
            AppMsg::WindowCloseRequested
        } else {
            AppMsg::QuitRequested
        });
        CloseRequestResponse::HideWindow
    });
    match tray::create_tray(&controller.icon) {
        Ok(tray) => *controller.tray.borrow_mut() = Some(tray),
        Err(error) => eprintln!("mklm: warning: the tray icon could not be created: {error}"),
    }
    let (watchers, warnings) = Watchers::start(
        |os| {
            let dark = os.dark;
            let _ = slint::invoke_from_event_loop(move || {
                let app = APP.with(|app| app.borrow().clone());
                if let Some(app) = app {
                    app.os_dark.set(Some(dark));
                    app.apply_theme();
                }
            });
        },
        |event| {
            use mklm_win::ui::shell_window::ShellEvent;
            match event {
                ShellEvent::TaskbarCreated => {
                    let _ = slint::invoke_from_event_loop(|| {
                        let app = APP.with(|app| app.borrow().clone());
                        if let Some(app) = app {
                            app.tray.borrow_mut().take();
                            if let Ok(tray) = tray::create_tray(&app.icon) {
                                *app.tray.borrow_mut() = Some(tray);
                            }
                        }
                    });
                }
                ShellEvent::SettingChange(area) if area == "ImmersiveColorSet" => {
                    let _ = slint::invoke_from_event_loop(|| {
                        let app = APP.with(|app| app.borrow().clone());
                        if let Some(app) = app {
                            app.apply_theme();
                        }
                    });
                }
                ShellEvent::SettingChange(area) if area == "intl" => post(AppMsg::Refresh),
                ShellEvent::Resumed => post(AppMsg::Refresh),
                // Directly, not scheduled: a scheduled closure may not run before the session
                // ends (design m3 F.5, review A5). The flag is an atomic outside any RefCell.
                ShellEvent::QueryEndSession => {
                    if worker::cancel_current_session() {
                        // A journaled change is open: make sure the next sign-in shows it,
                        // whatever the journal says at this instant (one short HKCU write).
                        let _ = mklm_client::run_once::PostRebootCommand::Gui
                            .command_line()
                            .map(|line| mklm_win::session::register_post_reboot(&line));
                    }
                }
                ShellEvent::EndSession => worker::wait_for_current_session(END_SESSION_WAIT),
                ShellEvent::SettingChange(_) => {}
            }
        },
        |scale| {
            let _ = slint::invoke_from_event_loop(move || {
                let app = APP.with(|app| app.borrow().clone());
                if let Some(app) = app {
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
        eprintln!("mklm: warning: {warning}");
    }
    *controller.watchers.borrow_mut() = watchers;

    // The title bar once the winit window exists (it is created by the event loop).
    let weak = window.as_weak();
    let _ = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else { return };
        let created = window.window().winit_window().await.is_ok();
        drop(window);
        if created {
            let app = APP.with(|app| app.borrow().clone());
            if let Some(app) = app {
                app.apply_theme();
            }
        }
    });

    let exit_timer = slint::Timer::default();
    if let Some(after) = args.exit_after {
        exit_timer.start(slint::TimerMode::SingleShot, after, || {
            let _ = slint::quit_event_loop();
        });
    }

    controller.run(Effect::Read);
    // The RunOnce rule at start-up (design m3 A.4): a restart may be pending from a change that
    // did not register the check (another front end, an elevated run).
    controller.run(Effect::RunOnceRule);
    if start_hidden {
        controller.run(Effect::Render);
    } else {
        // Shows the window, starts the input-language poll (design m3 A.4 rule 5), renders.
        controller.run(Effect::ShowWindow);
        controller.run(Effect::Render);
    }
    let result = slint::run_event_loop_until_quit();

    // Tear down on the UI thread in a defined order (tray first, so it leaves the notification
    // area; then the watchers). A running session worker is left to finish by itself.
    // WP-U6: wait up to 1 s for the I/O worker's queued saves (design m3 F.5).
    controller.tray.borrow_mut().take();
    *controller.watchers.borrow_mut() = Watchers::default();
    APP.with(|app| app.borrow_mut().take());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fatal(&format!("the event loop failed: {error}")),
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
        let app = APP.with(|app| app.borrow().clone());
        if let Some(app) = app {
            app.io.send(IoTask::OpenSettingsPage(page));
        }
    });
    window.on_theme_selected(|index| {
        let app = APP.with(|app| app.borrow().clone());
        if let Some(app) = app {
            app.theme_mode.set(ThemeMode::from_index(index));
            app.apply_theme();
            // WP-U6: save the choice in settings.toml.
        }
    });
    window.on_language_selected(|index| {
        let choice = LangChoice::from_index(index);
        let lang = choice.resolve(mklm_win::ui::user_default_ui_language());
        let app = APP.with(|app| app.borrow().clone());
        if let Some(app) = app {
            let _ = slint::select_bundled_translation(lang.bundle());
            app.window
                .global::<ui::Theme>()
                .set_font_family(lang.font_family().into());
            app.state.borrow_mut().lang = Some(lang);
            app.render();
            // WP-U6: save the choice in settings.toml.
        }
    });
    wire_change_flow(window);
    // The restart, post-reboot, conflict, history and recovery pages.
    crate::journal_ui::wire(window);
    // WP-U1, WP-U2, WP-U6 and WP-U7 wire the remaining callbacks (wizard, settings) through
    // `state::update`.
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
