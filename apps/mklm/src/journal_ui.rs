//! The restart, post-reboot, conflict, history and recovery pages in the window (design m3 B.8 to
//! B.12; WP-U4, WP-U5): the view-models `state::journal_pages` builds, copied into the generated
//! Slint properties; the lists kept as models and updated row by row, so that a render does not
//! rebuild the rows and drop the focus (design m3 A.6, review A7); the callbacks turned into
//! `AppMsg::Journal` messages; and the effects only the window or the I/O worker can carry out.

use std::cell::RefCell;
use std::rc::Rc;

use mklm_core::Timestamp;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::app::dispatch;
use crate::i18n::{Lang, journal_pages as text};
use crate::reader::{IoTask, IoWorker};
use crate::state::journal_pages::{self as pages, JournalEffect, JournalMsg, ON_TOP_FOR};
use crate::state::{AppMsg, AppState, Page};
use crate::ui::{self, AppWindow};
use crate::vm::conflict::{ConflictKeyboard, value_choice_index};
use crate::vm::journal::{JournalRow, utc_time_text};
use crate::vm::post_reboot::CheckLine;
use crate::vm::recovery::RecoveryItem;
use crate::vm::{self, ListOp, list_ops};

/// The kept list models of the journal pages and the on-top timer (UI thread only).
pub struct JournalUi {
    checks: Rc<VecModel<ui::CheckRowVm>>,
    shown_checks: RefCell<Vec<CheckLine>>,
    conflicts: Rc<VecModel<ui::ConflictKeyboardVm>>,
    shown_conflicts: RefCell<Vec<ConflictKeyboard>>,
    history: Rc<VecModel<ui::JournalRowVm>>,
    shown_history: RefCell<Vec<JournalRow>>,
    items: Rc<VecModel<ui::RecoveryItemVm>>,
    shown_items: RefCell<Vec<RecoveryItem>>,
    /// Ends the post-reboot check's on-top period (design m3 B.9).
    on_top_timer: slint::Timer,
}

impl std::fmt::Debug for JournalUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalUi").finish_non_exhaustive()
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

fn strings(values: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Applies the difference between the shown rows and `rows` to a kept model.
fn update_list<T: Clone + PartialEq, V: Clone + 'static>(
    model: &VecModel<V>,
    shown: &RefCell<Vec<T>>,
    rows: Vec<T>,
    key: fn(&T) -> &str,
    convert: fn(T) -> V,
) {
    let ops = list_ops(&shown.borrow(), &rows, key);
    for op in ops {
        match op {
            ListOp::Remove(index) => {
                model.remove(index);
            }
            ListOp::Insert(index, row) => model.insert(index, convert(row)),
            ListOp::Set(index, row) => model.set_row_data(index, convert(row)),
        }
    }
    *shown.borrow_mut() = rows;
}

fn check_key(row: &CheckLine) -> &str {
    &row.instance_id
}

fn check_vm(row: CheckLine) -> ui::CheckRowVm {
    ui::CheckRowVm {
        name: row.name.into(),
        setting: row.setting.into(),
        recognized: row.recognized.into(),
        tone: tone(row.tone),
        typed: row.typed.into(),
        typed_tone: tone(row.typed_tone),
        details: row.details.into(),
    }
}

fn conflict_key(keyboard: &ConflictKeyboard) -> &str {
    &keyboard.name
}

fn conflict_vm(keyboard: ConflictKeyboard) -> ui::ConflictKeyboardVm {
    ui::ConflictKeyboardVm {
        name: keyboard.name.into(),
        now: keyboard.now.into(),
        options: strings(
            keyboard
                .options
                .into_iter()
                .map(|option| option.label)
                .collect(),
        ),
        selected: i32::try_from(keyboard.selected).unwrap_or(0),
        write_error: keyboard.write_error.into(),
        baseline: keyboard.baseline.into(),
        values: ModelRc::new(VecModel::from(
            keyboard
                .values
                .into_iter()
                .map(|value| ui::ConflictValueVm {
                    record: i32::try_from(value.record).unwrap_or(-1),
                    line: value.line.into(),
                    selected: i32::try_from(value_choice_index(value.choice)).unwrap_or(0),
                })
                .collect::<Vec<_>>(),
        )),
    }
}

fn history_key(row: &JournalRow) -> &str {
    &row.op_id
}

fn history_vm(row: JournalRow) -> ui::JournalRowVm {
    ui::JournalRowVm {
        op: row.op.into(),
        op_id: row.op_id.into(),
        when: row.when.into(),
        what: row.what.into(),
        state: row.state.into(),
        reason: row.reason.into(),
        tone: tone(row.tone),
        can_revert: row.can_revert,
        revert_label: row.revert_label.into(),
    }
}

fn item_key(item: &RecoveryItem) -> &str {
    &item.op_id
}

fn item_vm(item: RecoveryItem) -> ui::RecoveryItemVm {
    ui::RecoveryItemVm {
        op_id: item.op_id.into(),
        operation: item.operation.into(),
        outcome: item.outcome.into(),
        tone: tone(item.tone),
        can_keep: item.can_keep,
        can_revert: item.can_revert,
    }
}

/// A journal time in local time for the history (design m3 G.1): "2026/09/27 22:36"; UTC with
/// its mark when the conversion fails.
pub fn local_time_text(at: Timestamp, lang: Lang) -> String {
    match mklm_win::time::local_time(at) {
        Ok(local) => text::history_time(
            (local.year, local.month, local.day, local.hour, local.minute),
            false,
            lang,
        ),
        Err(_) => utc_time_text(at, lang),
    }
}

impl JournalUi {
    /// Creates the kept models and hands them to the window.
    pub fn new(window: &AppWindow) -> Self {
        let ui = Self {
            checks: Rc::new(VecModel::default()),
            shown_checks: RefCell::new(Vec::new()),
            conflicts: Rc::new(VecModel::default()),
            shown_conflicts: RefCell::new(Vec::new()),
            history: Rc::new(VecModel::default()),
            shown_history: RefCell::new(Vec::new()),
            items: Rc::new(VecModel::default()),
            shown_items: RefCell::new(Vec::new()),
            on_top_timer: slint::Timer::default(),
        };
        window.set_post_reboot_rows(ModelRc::from(ui.checks.clone()));
        window.set_conflict_keyboards(ModelRc::from(ui.conflicts.clone()));
        window.set_journal_rows(ModelRc::from(ui.history.clone()));
        window.set_recovery_items(ModelRc::from(ui.items.clone()));
        ui
    }

    /// Renders the page shown now (the others are rendered when they open).
    pub fn render(&self, window: &AppWindow, state: &AppState) {
        let lang = state.lang.unwrap_or(Lang::Ja);
        let run_once_note: SharedString = state
            .journal_pages
            .run_once_problem
            .map(|elevated| text::run_once_problem(elevated, lang))
            .unwrap_or_default()
            .into();
        match state.page {
            Page::Restart => {
                let view = pages::restart_view(state);
                let restart = &state.journal_pages.restart;
                window.set_restart_reasons(strings(view.reasons));
                window.set_restart_ready(view.ready);
                window.set_restart_layout_changes(view.layout_changes);
                window.set_restart_can_undo(view.can_undo);
                window.set_restart_empty(view.empty_note.into());
                window.set_restart_restarting(restart.restarting);
                window.set_restart_restarting_text(text::restarting(lang).into());
                window.set_restart_error(
                    restart
                        .error
                        .as_ref()
                        .map(|_| text::restart_failed(lang))
                        .unwrap_or_default()
                        .into(),
                );
                window.set_restart_error_details(restart.error.clone().unwrap_or_default().into());
                window.set_restart_run_once_note(run_once_note);
            }
            Page::PostReboot => {
                let view = pages::post_reboot_view(state);
                window.set_post_reboot_operation(view.operation.into());
                window.set_post_reboot_not_restarted(view.not_restarted);
                window.set_post_reboot_migration_note(view.migration_note.into());
                window.set_post_reboot_keep_warning(view.keep_warning.into());
                window.set_post_reboot_revert_text(view.revert_text.into());
                window.set_post_reboot_can_keep(view.can_keep);
                window.set_post_reboot_uac_line(view.uac_line.into());
                window.set_post_reboot_empty(view.empty_note.into());
                window.set_post_reboot_run_once_note(run_once_note);
                update_list(
                    &self.checks,
                    &self.shown_checks,
                    view.rows,
                    check_key,
                    check_vm,
                );
            }
            Page::Conflict => {
                let view = pages::conflict_view(state);
                let restore = !view.restore_choices.is_empty();
                window.set_conflict_can_resolve(view.can_resolve());
                // Undo concerns operations in conflict; a stopped restore wrote nothing.
                window.set_conflict_can_undo(!view.op_id.is_empty());
                window.set_conflict_operation(view.operation.into());
                window.set_conflict_error(view.error.into());
                window.set_conflict_uac_line(view.uac_line.into());
                window.set_conflict_empty(view.empty_note.into());
                window.set_conflict_restore_note(view.restore_note.into());
                window.set_conflict_restore_selected(if restore {
                    i32::try_from(view.restore_selected).unwrap_or(-1)
                } else {
                    -1
                });
                window.set_conflict_restore_choices(ModelRc::new(VecModel::from(
                    view.restore_choices
                        .into_iter()
                        .map(|text| ui::ChoiceVm {
                            text: text.into(),
                            detail: SharedString::new(),
                            enabled: true,
                        })
                        .collect::<Vec<_>>(),
                )));
                // A stopped restore has one choice for everything: no per-value override.
                window.set_conflict_value_options(strings(if restore {
                    Vec::new()
                } else {
                    text::conflict_value_options(lang)
                }));
                update_list(
                    &self.conflicts,
                    &self.shown_conflicts,
                    view.keyboards,
                    conflict_key,
                    conflict_vm,
                );
            }
            Page::Journal => {
                let view = pages::journal_view(state, &|at| local_time_text(at, lang));
                window.set_journal_empty(view.empty_text.into());
                window.set_journal_notice(view.notice.into());
                window.set_journal_blocked(view.blocked_note.into());
                window.set_journal_folder_error(
                    if state.journal_pages.folder_failed {
                        text::history_folder_failed(lang)
                    } else {
                        String::new()
                    }
                    .into(),
                );
                update_list(
                    &self.history,
                    &self.shown_history,
                    view.rows,
                    history_key,
                    history_vm,
                );
            }
            Page::Recovery => {
                let view = pages::recovery_view(state, &|at| local_time_text(at, lang));
                window.set_recovery_method(i32::try_from(view.method.selected).unwrap_or(1));
                window.set_recovery(ui::RecoveryPageVm {
                    title: view.title.into(),
                    primary_text: view.primary_text.into(),
                    can_recover: view.can_recover,
                    can_undo: view.can_undo,
                    dismiss_text: view.dismiss_text.into(),
                    later_note: view.later_note.into(),
                    uac_line: view.uac_line.into(),
                    empty_note: view.empty_note.into(),
                    method_visible: view.method.visible,
                    live_text: view.method.live_text.into(),
                    live_detail: view.method.live_detail.into(),
                    later_text: view.method.later_text.into(),
                    later_detail: view.method.later_detail.into(),
                });
                update_list(
                    &self.items,
                    &self.shown_items,
                    view.items,
                    item_key,
                    item_vm,
                );
            }
            _ => {}
        }
    }

    /// Carries out an effect of these pages.
    pub fn run(&self, window: &AppWindow, io: &IoWorker, effect: JournalEffect) {
        match effect {
            JournalEffect::RestartPc => io.send(IoTask::RestartPc),
            JournalEffect::OpenRecoveryFolder => io.send(IoTask::OpenRecoveryFolder),
            JournalEffect::AlwaysOnTop(on) => {
                window.window().with_winit_window(|w| {
                    w.set_window_level(if on {
                        WindowLevel::AlwaysOnTop
                    } else {
                        WindowLevel::Normal
                    });
                });
                if on {
                    self.on_top_timer
                        .start(slint::TimerMode::SingleShot, ON_TOP_FOR, || {
                            send(JournalMsg::OnTopExpired);
                        });
                } else {
                    self.on_top_timer.stop();
                }
            }
        }
    }
}

fn send(msg: JournalMsg) {
    dispatch(AppMsg::Journal(msg));
}

/// A list index from Slint (`-1` for none).
fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

/// Connects the callbacks of these pages.
pub fn wire(window: &AppWindow) {
    window.on_restart_now(|acknowledged| send(JournalMsg::RestartNow { acknowledged }));
    window.on_restart_later(|| send(JournalMsg::RestartLater));
    window.on_undo_open(|| send(JournalMsg::OpenUndo));
    window.on_post_reboot_keep(|| send(JournalMsg::PostRebootKeep));
    window.on_post_reboot_revert(|| send(JournalMsg::PostRebootRevert));
    window.on_post_reboot_later(|| send(JournalMsg::PostRebootLater));
    window.on_conflict_choice(|keyboard, option| {
        if let (Some(keyboard), Some(option)) = (index(keyboard), index(option)) {
            send(JournalMsg::ConflictChoice { keyboard, option });
        }
    });
    window.on_conflict_value_choice(|record, option| {
        if let (Some(record), Some(option)) = (index(record), index(option)) {
            send(JournalMsg::ConflictValueChoice { record, option });
        }
    });
    window.on_conflict_restore_choice(|index_| {
        if let Some(choice) = index(index_) {
            send(JournalMsg::ConflictRestoreChoice(choice));
        }
    });
    window.on_conflict_resolve(|| send(JournalMsg::ConflictResolve));
    window.on_journal_revert(|op_id: SharedString| {
        send(JournalMsg::JournalRevert {
            op_id: op_id.to_string(),
        });
    });
    window.on_open_recovery_folder(|| send(JournalMsg::OpenRecoveryFolder));
    window.on_recovery_choose_method(|method| {
        if let Some(method) = index(method) {
            send(JournalMsg::RecoveryChooseMethod(method));
        }
    });
    window.on_recover(|| send(JournalMsg::RecoveryPrimary));
    window.on_recovery_keep(|op_id: SharedString| {
        send(JournalMsg::RecoveryKeep {
            op_id: op_id.to_string(),
        });
    });
    window.on_recovery_revert(|op_id: SharedString| {
        send(JournalMsg::RecoveryRevert {
            op_id: op_id.to_string(),
        });
    });
    window.on_recovery_later(|| send(JournalMsg::RecoveryDismiss));
}
