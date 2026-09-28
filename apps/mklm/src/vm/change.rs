//! The change page (plan 3.4, 3.5; design m3 B.4, B.5): one page from "変更…" to the UAC prompt
//! (review U14). The layout choices come first; once one is chosen the page shows how the change
//! takes effect — as a choice between switching now and switching at the restart (review U1) —,
//! that it applies to every user, the one-line UAC explanation and the button "変更する（次に
//! Windows の確認が出ます）". Layout detection runs in place, for this keyboard only.
//!
//! The apply method's default comes from what MKLM saw in the last 10 minutes
//! ([`default_apply_method`], plan 1.4): input from another keyboard, or the pointer (a mouse
//! user can finish with the on-screen keyboard), makes "switch now" the default. Key presses from
//! the target alone make "restart" the default, with the plan's "only keyboard" warning. No input
//! at all gives "restart" without the warning.

use std::time::{Duration, Instant};

use mklm_client::preview::RestorePreview;
use mklm_core::{OperationPlan, SystemSnapshot};

use super::Tone;
use crate::i18n::Lang;

/// "Recent" in plan 1.4.
pub const RECENT_INPUT: Duration = Duration::from_secs(10 * 60);

/// What the user did in MKLM's window lately (Raw Input only reaches a focused window). Instance
/// IDs and times only, never keys (plan 2.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputActivity {
    /// The last key press per keyboard (instance ID).
    keys: Vec<(String, Instant)>,
    /// The last pointer input (mouse button, movement, wheel).
    pub pointer: Option<Instant>,
}

impl InputActivity {
    pub fn key(&mut self, instance_id: &str, at: Instant) {
        match self
            .keys
            .iter_mut()
            .find(|(id, _)| id.eq_ignore_ascii_case(instance_id))
        {
            Some((_, last)) => *last = at,
            None => self.keys.push((instance_id.to_string(), at)),
        }
    }

    pub fn pointer(&mut self, at: Instant) {
        self.pointer = Some(at);
    }
}

/// How the change takes effect (the two choices, `ApplyOptions` in the request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMethod {
    /// Reset the keyboard now and keep or revert within the countdown
    /// (`allow_live_reset` and `other_input_available`).
    Live,
    /// Leave it to the PC restart (`ApplyOptions::default()`).
    Restart,
}

impl ApplyMethod {
    pub fn options(self) -> mklm_core::ApplyOptions {
        match self {
            ApplyMethod::Live => mklm_core::ApplyOptions {
                allow_live_reset: true,
                other_input_available: true,
                ..mklm_core::ApplyOptions::default()
            },
            ApplyMethod::Restart => mklm_core::ApplyOptions::default(),
        }
    }
}

/// The preselected method and whether the "only keyboard" warning shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodDefault {
    pub method: ApplyMethod,
    /// "このキーボードは、最近入力のあった唯一のキーボードです…" (plan 1.4).
    pub only_keyboard_warning: bool,
}

/// The default for a change to the keyboard made of `targets` (instance IDs of one physical
/// device) at `now`.
pub fn default_apply_method(
    activity: &InputActivity,
    targets: &[String],
    now: Instant,
) -> MethodDefault {
    let recent = |at: &Instant| now.saturating_duration_since(*at) <= RECENT_INPUT;
    let is_target = |id: &str| targets.iter().any(|t| t.eq_ignore_ascii_case(id));
    let other_key = activity
        .keys
        .iter()
        .any(|(id, at)| recent(at) && !is_target(id));
    let target_key = activity
        .keys
        .iter()
        .any(|(id, at)| recent(at) && is_target(id));
    let pointer = activity.pointer.as_ref().is_some_and(recent);
    match (other_key || pointer, target_key) {
        (true, _) => MethodDefault {
            method: ApplyMethod::Live,
            only_keyboard_warning: false,
        },
        (false, true) => MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: true,
        },
        (false, false) => MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: false,
        },
    }
}

/// One layout choice (a radio button: ◉ / ○, review U16).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChoiceRow {
    pub text: String,
    /// E.g. "値を消して PC の標準配列に従わせます。打鍵での確認がまだです" for 標準に従う.
    pub detail: String,
    /// The layout it has now ("（現在）").
    pub current: bool,
    pub enabled: bool,
}

/// One value line (technical details, folded by default).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanLine {
    pub key: String,
    pub name: String,
    pub now: String,
    pub after: String,
}

/// The in-place layout detection (design m3 B.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetectPanel {
    /// "わからないときは、このキーボードで Backspace の左のキーを押してください", then the second key.
    pub instruction: String,
    /// "内蔵キーボードのキーです。Keychron Receiver で押してください" and the like.
    pub feedback: String,
    pub feedback_tone: Tone,
    /// "✓ JIS 配列のキーボードです"; selecting the matching choice is one click.
    pub verdict: String,
    /// The choice the verdict points at, if any.
    pub verdict_choice: Option<usize>,
}

/// The page (WP-U3 fills it from the draft, the plan and the settings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangePage {
    /// "Keychron Receiver の配列".
    pub title: String,
    pub choices: Vec<ChoiceRow>,
    pub selected: Option<usize>,
    pub detect: DetectPanel,
    /// `PrepareChange` runs: "確認しています…".
    pub preparing: bool,
    /// The plan can reset in place: the two apply methods are offered.
    pub method_visible: bool,
    pub method: Option<ApplyMethod>,
    /// `i18n::apply_method(true / false, seconds)`.
    pub live_text: String,
    pub live_detail: String,
    pub restart_text: String,
    pub restart_detail: String,
    /// `i18n::takes_effect(plan.apply, seconds)` for the chosen method.
    pub takes_effect: String,
    pub all_users_note: String,
    /// Fixed mode (`MigrationRequired`): the migration with this assignment, one restart, and
    /// the PC's standard layout ("標準配列（おすすめ: 今の JIS）"), design m3 B.1 / B.4.
    pub migration_note: String,
    /// US chosen: the IME switch (Alt+` because 半角/全角 is missing); shown again after the
    /// change (design m3 B.13).
    pub ime_note: String,
    /// "only keyboard", "標準に従う is not verified by typing", INV-PS2 …
    pub warnings: Vec<String>,
    pub lines: Vec<PlanLine>,
    /// "次に Windows の確認画面が出ます。発行元は『不明』と表示されます…" (one or two lines).
    pub uac_line: String,
    /// "変更する（次に Windows の確認が出ます）"; "変更する" when elevated (no prompt).
    pub apply_text: String,
    pub can_apply: bool,
}

/// Rules (WP-U3): the title names the keyboard; choices JIS / US / 標準に従う (kbdhid only) with
/// "（現在）"; `method` defaults per [`default_apply_method`], and the plan is made again with
/// the chosen method's options (what is shown is what is sent as `ExpectedPlan`, design m2 S6);
/// `takes_effect` from `i18n::takes_effect`; a warning for `LayoutChoice::Standard` (not verified
/// by typing, design m2 D.2) and for the only keyboard (plan 1.4); `can_apply` unless
/// `check_inv_ps2` fails or `PrepareChange` runs.
pub fn change_page(
    snapshot: &SystemSnapshot,
    plan: Option<&OperationPlan>,
    method: Option<ApplyMethod>,
    lang: Lang,
) -> ChangePage {
    let _ = (snapshot, plan, method, lang);
    todo!("WP-U3: the change page view-model")
}

/// For a restore to before MKLM (design m2 D.5): the same page from a `RestorePreview` (the
/// conflicts per keyboard, design m3 B.10).
pub fn restore_page(preview: &RestorePreview, lang: Lang) -> ChangePage {
    let _ = (preview, lang);
    todo!("WP-U3: the restore view-model")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const KEYCHRON_2: &str = r"HID\VID_3434&PID_D027&MI_01&COL01\8&2A1B&0&0000";
    const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";

    #[test]
    fn the_default_apply_method() {
        let start = Instant::now();
        let now = start + Duration::from_secs(60);
        let targets = vec![KEYCHRON.to_string(), KEYCHRON_2.to_string()];
        // A mouse user who pressed no key: switch now (review U1).
        let mut mouse = InputActivity::default();
        mouse.pointer(start);
        assert_eq!(
            default_apply_method(&mouse, &targets, now),
            MethodDefault {
                method: ApplyMethod::Live,
                only_keyboard_warning: false
            }
        );
        // Another keyboard typed: switch now.
        let mut other = InputActivity::default();
        other.key(BUILT_IN, start);
        other.key(KEYCHRON, start);
        assert_eq!(
            default_apply_method(&other, &targets, now).method,
            ApplyMethod::Live
        );
        // Only the target typed (another collection of it counts as the target): restart,
        // with the plan's warning.
        let mut only = InputActivity::default();
        only.key(KEYCHRON_2, start);
        assert_eq!(
            default_apply_method(&only, &targets, now),
            MethodDefault {
                method: ApplyMethod::Restart,
                only_keyboard_warning: true
            }
        );
        // Nothing seen, or only long ago: restart, no warning.
        let later = start + RECENT_INPUT + Duration::from_secs(1);
        assert_eq!(
            default_apply_method(&mouse, &targets, later),
            MethodDefault {
                method: ApplyMethod::Restart,
                only_keyboard_warning: false
            }
        );
        assert!(ApplyMethod::Live.options().allow_live_reset);
        assert!(!ApplyMethod::Restart.options().other_input_available);
    }
}
