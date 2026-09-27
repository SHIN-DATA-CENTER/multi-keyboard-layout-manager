//! The first-run wizard (plan 3.1; design m3 B.1), four steps (review U3):
//!
//! 1. ようこそ — what MKLM does, for every user of the PC, with an administrator's permission.
//! 2. 入力方式 — the user's Preload and the sign-in screen's (`HKU\.DEFAULT`).
//! 3. キーボードの配列 — every connected keyboard with JIS / US (default: how it types now) and
//!    the in-place layout detection; values with a problem are shown on their row.
//! 4. まとめ — what will happen: nothing ("変更は不要です"), one `Migrate` that carries the
//!    assignments (fixed mode; "PC の再起動が 1 回必要です", with the PC's standard layout), or one
//!    `SetLayout` after another (per-keyboard mode).
//!
//! The migration always carries the keyboards that differ from the standard layout (plan 1.3:
//! "propose the migration when a layout different from the global one is first assigned; write
//! the assignment in the same transaction") — a new US keyboard on a fixed-JIS PC is US after
//! the one restart.

use mklm_core::{Assessment, GlobalMode, InputMethods, LayoutChoice, LayoutTable, SystemSnapshot};

use crate::i18n::Lang;

/// The wizard's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    InputMethods,
    Keyboards,
    Summary,
    Done,
}

impl WizardStep {
    /// "手順 3 / 4: キーボードの配列" (review U16 e): the position in words, not by colour.
    pub fn number(self) -> Option<(u32, u32)> {
        match self {
            WizardStep::Welcome => Some((1, 4)),
            WizardStep::InputMethods => Some((2, 4)),
            WizardStep::Keyboards => Some((3, 4)),
            WizardStep::Summary => Some((4, 4)),
            WizardStep::Done => None,
        }
    }
}

/// What to do about a value with a problem (plan 3.1 step 2; design m3 B.1). Only problem values
/// are listed; a value without one needs no decision (MKLM records the value before its first
/// change as the baseline, design m2 C.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProblemChoice {
    /// Leave it (recommended).
    #[default]
    Keep,
    /// Remove a value its driver ignores: `Request::CleanupValues` (design m3 A.5, WP-E1). Only
    /// offered for such values; when WP-E1 is deferred, the row shows the explanation only.
    Delete,
}

/// One connected keyboard on step 3.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WizardKeyboard {
    pub id: String,
    pub members: Vec<String>,
    pub name: String,
    /// "JIS として動作中".
    pub current: String,
    /// The table it types with now (the default choice).
    pub current_table: Option<LayoutTable>,
    /// The user's choice (JIS / US); `None` = as now.
    pub choice: Option<LayoutTable>,
    /// A problem value on this keyboard, in words, and what may be done about it.
    pub problem: String,
    pub problem_deletable: bool,
    pub problem_choice: ProblemChoice,
}

/// What step 4 proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardPlan {
    /// Every choice matches how the keyboards type now.
    NothingToDo,
    /// Fixed mode and some keyboard differs: one migration carrying the assignments.
    Migrate {
        standard: LayoutChoice,
        assignments: Vec<(String, LayoutChoice)>,
    },
    /// Per-keyboard mode: one `SetLayout` after another.
    SetLayouts(Vec<(String, LayoutChoice)>),
}

/// The step 4 plan from the step 3 choices (per keyboard: the instance ID a request targets and
/// the chosen table). In fixed mode `standard` is the PC's standard layout for the migration
/// (default: the fixed layout of now); in per-keyboard mode it is shown, not changed.
pub fn wizard_plan(
    mode: GlobalMode,
    standard: &LayoutTable,
    keyboards: &[(String, Option<LayoutTable>, LayoutTable)],
) -> WizardPlan {
    let as_choice = |table: &LayoutTable| match table {
        LayoutTable::Jis => Some(LayoutChoice::Jis),
        LayoutTable::Us => Some(LayoutChoice::Us),
        LayoutTable::Other(_) => None,
    };
    let changes: Vec<(String, LayoutChoice)> = keyboards
        .iter()
        .filter(|(_, now, chosen)| now.as_ref() != Some(chosen))
        .filter_map(|(id, _, chosen)| Some((id.clone(), as_choice(chosen)?)))
        .collect();
    if changes.is_empty() {
        return WizardPlan::NothingToDo;
    }
    match (mode, as_choice(standard)) {
        (GlobalMode::Fixed, Some(standard_choice)) => WizardPlan::Migrate {
            standard: standard_choice,
            assignments: changes,
        },
        _ => WizardPlan::SetLayouts(changes),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WizardPage {
    /// "手順 3 / 4: キーボードの配列".
    pub heading: String,
    pub body: String,
    pub warnings: Vec<String>,
    pub keyboards: Vec<WizardKeyboard>,
    pub can_next: bool,
    pub next_text: String,
}

/// The input-methods page: `mklm_core::input_warnings`, worded (WP-U2).
pub fn input_methods_page(input: &InputMethods, lang: Lang) -> WizardPage {
    let _ = (input, lang);
    todo!("WP-U2: the input methods page")
}

/// The keyboards page: every connected keyboard (rows as in `vm::keyboards`), the choice
/// defaulting to how it types now, and the anomalies (`KeyboardAssessment::anomalies`) that are
/// problems, in words (WP-U2).
pub fn keyboards_page(
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    lang: Lang,
) -> WizardPage {
    let _ = (snapshot, assessment, lang);
    todo!("WP-U2: the keyboards page")
}

/// The summary page for `plan` (WP-U2): "変更は不要です", or the migration ("PC の再起動が 1 回
/// 必要です", the standard layout "標準配列（おすすめ: 今の JIS）", the write order, the sign-in
/// help), or the list of changes.
pub fn summary_page(plan: &WizardPlan, lang: Lang) -> WizardPage {
    let _ = (plan, lang);
    todo!("WP-U2: the summary page")
}

#[cfg(test)]
mod tests {
    use super::*;

    const US_KEYBOARD: &str = r"HID\VID_046D&PID_C31C&MI_00\7&1&0&0000";
    const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

    #[test]
    fn a_new_us_keyboard_on_a_fixed_jis_pc_is_migrated_with_its_assignment() {
        // Review U3: the migration carries the US keyboard, so it is US after the restart.
        let plan = wizard_plan(
            GlobalMode::Fixed,
            &LayoutTable::Jis,
            &[
                (BUILT_IN.into(), Some(LayoutTable::Jis), LayoutTable::Jis),
                (US_KEYBOARD.into(), Some(LayoutTable::Jis), LayoutTable::Us),
            ],
        );
        assert_eq!(
            plan,
            WizardPlan::Migrate {
                standard: LayoutChoice::Jis,
                assignments: vec![(US_KEYBOARD.into(), LayoutChoice::Us)],
            }
        );
        // Nothing differs: no restart, nothing written.
        assert_eq!(
            wizard_plan(
                GlobalMode::Fixed,
                &LayoutTable::Jis,
                &[(BUILT_IN.into(), Some(LayoutTable::Jis), LayoutTable::Jis)],
            ),
            WizardPlan::NothingToDo
        );
        // Per-keyboard mode: ordinary changes.
        assert_eq!(
            wizard_plan(
                GlobalMode::PerKeyboard,
                &LayoutTable::Jis,
                &[(US_KEYBOARD.into(), Some(LayoutTable::Jis), LayoutTable::Us)],
            ),
            WizardPlan::SetLayouts(vec![(US_KEYBOARD.into(), LayoutChoice::Us)])
        );
        assert_eq!(WizardStep::Keyboards.number(), Some((3, 4)));
    }
}
