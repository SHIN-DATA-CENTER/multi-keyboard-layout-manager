//! The first-run wizard in the window (design m3 B.1; WP-U2): the view-model `state::wizard`
//! builds, copied into the generated Slint properties; the keyboard rows kept as one model and
//! updated row by row, so that a render does not rebuild the rows and drop the focus of a combo
//! box (design m3 A.6, review A7); the callbacks turned into `AppMsg::Wizard` messages.

use std::cell::RefCell;
use std::rc::Rc;

use mklm_core::Layout;
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::app::dispatch;
use crate::models::KeptModel;
use crate::state::{AppMsg, AppState, Page, WizardMsg};
use crate::ui::{self, AppWindow};
use crate::vm::wizard::{ProblemChoice, WizardKeyboard};
use crate::vm::{self, ListOp, list_ops};

/// The kept rows of step 3 and the kept options of the standard layout (UI thread only).
pub struct WizardUi {
    rows: Rc<VecModel<ui::WizardKeyboardVm>>,
    shown: RefCell<Vec<WizardKeyboard>>,
    standard_choices: KeptModel<ui::ChoiceVm>,
}

impl std::fmt::Debug for WizardUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WizardUi").finish_non_exhaustive()
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

fn index(value: Option<usize>) -> i32 {
    value.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1)
}

fn row_vm(row: WizardKeyboard) -> ui::WizardKeyboardVm {
    ui::WizardKeyboardVm {
        id: row.id.into(),
        name: row.name.into(),
        current: row.current.into(),
        note: row.note.into(),
        can_choose: row.can_choose,
        choice: match row.choice {
            Some(Layout::Jis) => 0,
            Some(Layout::Us) => 1,
            None => -1,
        },
        problem: row.problem.into(),
        problem_deletable: row.problem_deletable,
        problem_choice: match row.problem_choice {
            ProblemChoice::Keep => 0,
            ProblemChoice::Delete => 1,
        },
    }
}

impl WizardUi {
    pub fn new(window: &AppWindow) -> Self {
        let rows = Rc::new(VecModel::default());
        window.set_wizard_keyboards(ModelRc::from(rows.clone()));
        Self {
            rows,
            shown: RefCell::new(Vec::new()),
            standard_choices: KeptModel::default(),
        }
    }

    /// Renders the wizard page (only while it is shown).
    pub fn render(&self, window: &AppWindow, state: &AppState) {
        if state.page != Page::Wizard {
            return;
        }
        let page = crate::state::wizard::page(state);
        window.set_wizard_standard(index(page.standard_selected));
        window.set_wizard_autostart(page.autostart);
        window.set_wizard(ui::WizardVm {
            step: i32::try_from(page.step.index()).unwrap_or(0),
            heading: page.heading.into(),
            body: page.body.into(),
            lines: strings(page.lines),
            warnings: strings(page.warnings),
            notes: strings(page.notes),
            settings_buttons: page.settings_buttons,
            detect_instruction: page.detect.instruction.into(),
            detect_feedback: page.detect.feedback.into(),
            detect_feedback_tone: tone(page.detect.feedback_tone),
            detect_verdict: page.detect.verdict.into(),
            detect_choose_text: page.detect.choose_text.into(),
            standard_visible: page.standard_visible,
            // Kept and updated in place: a render does not rebuild the radio buttons.
            standard_choices: self
                .standard_choices
                .show(
                    page.standard_choices
                        .into_iter()
                        .map(|choice| ui::ChoiceVm {
                            text: choice.text.into(),
                            detail: choice.detail.into(),
                            enabled: choice.enabled,
                        }),
                ),
            migration: page.migration,
            autostart_visible: page.autostart_visible,
            busy: page.busy,
            note: page.note.into(),
            note_tone: tone(page.note_tone),
            uac_line: page.uac_line.into(),
            details: page.details.join("\n").into(),
            back_visible: page.back_visible,
            secondary_text: page.secondary_text.into(),
            next_text: page.next_text.into(),
            can_next: page.can_next,
        });
        self.update_rows(page.keyboards);
    }

    /// Applies the difference between the shown rows and `rows` to the kept model.
    fn update_rows(&self, rows: Vec<WizardKeyboard>) {
        let ops = list_ops(&self.shown.borrow(), &rows, |row| row.id.as_str());
        for op in ops {
            match op {
                ListOp::Remove(index) => {
                    self.rows.remove(index);
                }
                ListOp::Insert(index, row) => self.rows.insert(index, row_vm(row)),
                ListOp::Set(index, row) => self.rows.set_row_data(index, row_vm(row)),
            }
        }
        debug_assert_eq!(self.rows.row_count(), rows.len());
        *self.shown.borrow_mut() = rows;
    }
}

/// Connects the wizard's callbacks, and Settings' "初回セットアップをもう一度行う".
pub fn wire(window: &AppWindow) {
    let send = |msg: WizardMsg| dispatch(AppMsg::Wizard(msg));
    window.on_wizard_back(move || send(WizardMsg::Back));
    window.on_wizard_next(move || send(WizardMsg::Next));
    window.on_wizard_skip(move || send(WizardMsg::Skip));
    window.on_wizard_secondary(move || send(WizardMsg::Secondary));
    window.on_wizard_keyboard_choice(move |row: SharedString, choice| {
        if let Ok(index) = usize::try_from(choice) {
            send(WizardMsg::Choose {
                row: row.to_string(),
                index,
            });
        }
    });
    window.on_wizard_problem_choice(move |row: SharedString, choice| {
        send(WizardMsg::Problem {
            row: row.to_string(),
            delete: choice == 1,
        });
    });
    window.on_wizard_choose_standard(move |choice| {
        if let Ok(index) = usize::try_from(choice) {
            send(WizardMsg::Standard(index));
        }
    });
    window.on_wizard_autostart_toggled(move |on| send(WizardMsg::Autostart(on)));
    window.on_wizard_restart_detection(move || send(WizardMsg::RestartDetection));
    window.on_wizard_use_detected(move || send(WizardMsg::UseDetected));
    window.on_run_wizard(move || send(WizardMsg::Open));
}
