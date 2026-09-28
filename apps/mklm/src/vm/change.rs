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
//!
//! What the page shows is what is sent (design m2 S6): [`plan_change`] makes the plan with the
//! options of the chosen method, with `mklm_core::plan_set_layout` — the function the engine
//! calls — and [`change_request`] sends that plan as the `ExpectedPlan`. In fixed mode the change
//! becomes the migration that carries this assignment (plan 1.3; design m3 B.4). "MKLM 導入前に
//! 戻す…" turns the page into the restore preview of this device ([`plan_restore`],
//! `mklm_client::preview::preview_restore`).

use std::time::{Duration, Instant};

use mklm_client::preview::{RestorePreview, expected, nothing_to_change, preview_restore};
use mklm_client::values::{model_value, op_value};
use mklm_core::{
    ApplyOptions, ConflictPolicy, KeyboardDriver, Layout, LayoutBasis, LayoutChoice, LayoutTable,
    OperationError, OperationPlan, PendingAction, RestoreScope, SystemSnapshot, WriteTarget,
    assess, plan_migration, plan_set_layout, ps2_pin_layout,
};
use mklm_ipc::{Assignment, MigrateRequest, Request, RestoreBaselineRequest, SetLayoutRequest};

use super::{SnapshotText, Tone};
use crate::detect::Verdict;
use crate::i18n::{self, Lang};
use crate::state::{ChangeDraft, PrepareFailure};

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
    /// Leave it to the PC restart: the target counts as the only usable keyboard
    /// (`other_input_available` false), so `mklm_core::apply_method` plans `RestartPc` for a
    /// keyboard that could be reset — never a reset, and never the reconnect path the label does
    /// not promise (`allow_live_reset` false alone would plan `Reconnect` for a USB keyboard).
    Restart,
}

impl ApplyMethod {
    pub fn options(self) -> ApplyOptions {
        match self {
            ApplyMethod::Live => ApplyOptions {
                allow_live_reset: true,
                other_input_available: true,
                ..mklm_core::ApplyOptions::default()
            },
            ApplyMethod::Restart => {
                mklm_client::preview::no_other_input(&ApplyMethod::Live.options())
            }
        }
    }

    /// The index in the page's two choices (0 = switch now, 1 = at the restart).
    pub fn index(self) -> usize {
        match self {
            ApplyMethod::Live => 0,
            ApplyMethod::Restart => 1,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(ApplyMethod::Live),
            1 => Some(ApplyMethod::Restart),
            _ => None,
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

/// The layout choices of a device, in page order: JIS, US and — for HID keyboards only —
/// "標準に従う" (`StandardNotAllowed` for i8042prt, design m3 B.4).
pub fn layout_choices(snapshot: Option<&SystemSnapshot>, members: &[String]) -> Vec<LayoutChoice> {
    let mut choices = vec![LayoutChoice::Jis, LayoutChoice::Us];
    let all_hid = snapshot.is_some_and(|snapshot| {
        let drivers: Vec<&KeyboardDriver> = snapshot
            .keyboards
            .iter()
            .filter(|kb| {
                members
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&kb.instance_id))
            })
            .map(|kb| &kb.driver)
            .collect();
        !drivers.is_empty() && drivers.iter().all(|d| **d == KeyboardDriver::Kbdhid)
    });
    if all_hid {
        choices.push(LayoutChoice::Standard);
    }
    choices
}

/// The keyboard a request names (`plan_set_layout` expands it to its physical device): the first
/// connected member, else the first member the snapshot knows.
pub fn target_instance(snapshot: &SystemSnapshot, members: &[String]) -> Option<String> {
    let known = |present: bool| {
        members.iter().find(|id| {
            snapshot
                .keyboards
                .iter()
                .any(|kb| kb.instance_id.eq_ignore_ascii_case(id) && (kb.present || !present))
        })
    };
    known(true).or_else(|| known(false)).cloned()
}

/// The layout the device is set to now, as a choice (for "（現在）"), and the PC's standard layout.
fn current_choice(
    snapshot: &SystemSnapshot,
    members: &[String],
) -> (Option<LayoutChoice>, LayoutTable) {
    let assessment = assess(snapshot);
    let member = |present: bool| {
        assessment.keyboards.iter().find(|ka| {
            (ka.present || !present)
                && members
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&ka.instance_id))
        })
    };
    let choice = member(true)
        .or_else(|| member(false))
        .and_then(|ka| ka.after_restart.as_ref())
        .and_then(|layout| match (layout.basis, &layout.table) {
            (LayoutBasis::Standard, _) => Some(LayoutChoice::Standard),
            (_, LayoutTable::Jis) => Some(LayoutChoice::Jis),
            (_, LayoutTable::Us) => Some(LayoutChoice::Us),
            (_, LayoutTable::Other(_)) => None,
        });
    (choice, assessment.standard_layout)
}

/// What the page plans with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// "Set layout" in per-keyboard mode.
    Set(OperationPlan),
    /// Fixed mode: the migration that carries this assignment (design m3 B.4).
    Migrate {
        standard: Layout,
        plan: OperationPlan,
    },
    /// "MKLM 導入前に戻す…" of this device.
    Restore(RestorePreview),
    /// The rules refuse the change before any prompt.
    Refused(OperationError),
}

/// The plan for the chosen options (design m3 B.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPlan {
    pub planned: Planned,
    /// The live options would reset a keyboard in place: the two apply methods are offered.
    pub method_offered: bool,
    /// The options the request carries (`ApplyOptions::default()`, which never resets, when the
    /// methods are not offered).
    pub options: ApplyOptions,
    /// Every value is in place already: nothing to send.
    pub nothing_to_change: bool,
}

impl DraftPlan {
    fn refused(error: OperationError) -> Self {
        Self {
            planned: Planned::Refused(error),
            method_offered: false,
            options: ApplyOptions::default(),
            nothing_to_change: false,
        }
    }

    /// How the change takes effect, when something is written.
    pub fn apply(&self) -> Option<PendingAction> {
        match &self.planned {
            Planned::Set(plan) | Planned::Migrate { plan, .. } => Some(plan.apply),
            Planned::Restore(preview) => preview.apply,
            Planned::Refused(_) => None,
        }
    }
}

/// Plans "set layout" of `target` to `choice` (design m3 B.5): first with the live options — the
/// methods are offered only when that plan resets a keyboard in place —, then with the options
/// of `method`. Fixed mode (`MigrationRequired`) plans the migration with this assignment and the
/// standard layout `standard` (default: the one fixed mode gives now). `uncertain_values`: the
/// inventory could not read some value as MKLM expects, so only the helper may conclude that
/// nothing needs writing (`mklm_client::inventory::Inventory`).
pub fn plan_change(
    snapshot: &SystemSnapshot,
    uncertain_values: bool,
    target: &str,
    choice: LayoutChoice,
    method: ApplyMethod,
    standard: Option<Layout>,
) -> DraftPlan {
    let plan = |options: &ApplyOptions| {
        plan_set_layout(
            &snapshot.keyboards,
            &snapshot.global,
            target,
            choice,
            options,
        )
    };
    let live_options = ApplyMethod::Live.options();
    let live = match plan(&live_options) {
        Ok(live) => live,
        Err(OperationError::MigrationRequired { fixed }) => {
            let standard = standard.or(fixed).unwrap_or(Layout::Jis);
            return match plan_migration(
                &snapshot.keyboards,
                &snapshot.global,
                standard,
                &[(target.to_string(), choice)],
            ) {
                Ok(plan) => DraftPlan {
                    nothing_to_change: !uncertain_values && nothing_to_change(snapshot, &plan),
                    planned: Planned::Migrate { standard, plan },
                    method_offered: false,
                    options: ApplyOptions::default(),
                },
                Err(error) => DraftPlan::refused(error),
            };
        }
        Err(error) => return DraftPlan::refused(error),
    };
    let method_offered = live.apply == PendingAction::ResetKeyboard;
    let options = if method_offered {
        method.options()
    } else {
        ApplyOptions::default()
    };
    let chosen = if options == live_options {
        Ok(live)
    } else {
        plan(&options)
    };
    match chosen {
        Ok(plan) => DraftPlan {
            nothing_to_change: !uncertain_values && nothing_to_change(snapshot, &plan),
            planned: Planned::Set(plan),
            method_offered,
            options,
        },
        Err(error) => DraftPlan::refused(error),
    }
}

/// Plans the restore of `target`'s device to the values before MKLM (design m2 D.5, m3 B.4):
/// conflicts are reported first (`ConflictPolicy::Report`), then decided on the conflict page.
pub fn plan_restore(
    snapshot: &SystemSnapshot,
    journal: &mklm_core::Journal,
    target: &str,
    method: ApplyMethod,
) -> DraftPlan {
    let scope = RestoreScope::Device {
        instance_id: target.to_string(),
    };
    let preview = |options: &ApplyOptions| {
        preview_restore(snapshot, journal, &scope, ConflictPolicy::Report, options)
    };
    let live_options = ApplyMethod::Live.options();
    let live = match preview(&live_options) {
        Ok(live) => live,
        Err(error) => return DraftPlan::refused(error),
    };
    let method_offered = live.apply == Some(PendingAction::ResetKeyboard);
    let options = if method_offered {
        method.options()
    } else {
        ApplyOptions::default()
    };
    let chosen = if options == live_options {
        Ok(live)
    } else {
        preview(&options)
    };
    match chosen {
        Ok(preview) => DraftPlan {
            nothing_to_change: preview.writes.is_empty() && preview.conflicts.is_empty(),
            planned: Planned::Restore(preview),
            method_offered,
            options,
        },
        Err(error) => DraftPlan::refused(error),
    }
}

/// The request for a ready plan, and the options a recovery after a lost helper uses: `None`
/// while nothing may be sent (refused, nothing to change, no choice).
pub fn change_request(
    target: &str,
    choice: Option<LayoutChoice>,
    plan: &DraftPlan,
) -> Option<(Request, ApplyOptions)> {
    if plan.nothing_to_change {
        return None;
    }
    let request = match &plan.planned {
        Planned::Set(set) => Request::SetLayout(SetLayoutRequest {
            instance_id: target.to_string(),
            layout: choice?,
            apply: plan.options,
            expected: Some(expected(set)),
        }),
        Planned::Migrate {
            standard,
            plan: migration,
        } => Request::Migrate(MigrateRequest {
            standard: *standard,
            assignments: vec![Assignment {
                instance_id: target.to_string(),
                layout: choice?,
            }],
            expected: Some(expected(migration)),
        }),
        Planned::Restore(preview) => {
            if preview.order.is_err() {
                return None;
            }
            Request::RestoreBaseline(RestoreBaselineRequest {
                scope: RestoreScope::Device {
                    instance_id: target.to_string(),
                },
                on_conflict: ConflictPolicy::Report,
                apply: plan.options,
            })
        }
        Planned::Refused(_) => return None,
    };
    Some((request, plan.options))
}

/// The request of a draft that is ready to send: prepared, planned with the resolved method,
/// not refused, something to change. `None` otherwise ("変更する" is disabled).
pub fn draft_request(draft: &ChangeDraft) -> Option<(Request, ApplyOptions)> {
    if draft.preparing.is_some() || draft.failure.is_some() {
        return None;
    }
    if !draft.restore && draft.choice.is_none() {
        return None;
    }
    let prepared = draft.prepared.as_ref()?;
    let plan = draft.plan.as_ref()?;
    let target = target_instance(&prepared.snapshot, &draft.members)?;
    change_request(&target, draft.choice, plan)
}

/// The keep-or-revert time the page names: 20 s, or 60 s with "確認の時間を長くする" (the only
/// two the helper accepts, WP-E3); anything else in the settings file counts as 20.
pub fn countdown_seconds(settings: &crate::settings::Settings) -> u32 {
    match settings.change.countdown_seconds {
        60 => 60,
        _ => 20,
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
    /// The button that selects it ("JIS を選ぶ"); empty when it is selected already.
    pub choose_text: String,
}

/// The page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangePage {
    /// "Keychron Receiver の配列" / "Keychron Receiver を MKLM 導入前に戻す".
    pub title: String,
    /// The restore preview of this device (no choices, no detection).
    pub restore: bool,
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
    /// The PC's standard layout of the migration: JIS / US.
    pub standard_visible: bool,
    pub standard_choices: Vec<ChoiceRow>,
    pub standard_selected: Option<usize>,
    /// The restore: what it puts back.
    pub summary: String,
    /// Why nothing can be sent now (refused, unreadable, nothing to change), with its tone.
    pub note: String,
    pub note_tone: Tone,
    /// US chosen: the IME switch (Alt+` because 半角/全角 is missing); shown again after the
    /// change (design m3 B.13).
    pub ime_note: String,
    /// "only keyboard", "標準に従う is not verified by typing", conflicts of a restore …
    pub warnings: Vec<String>,
    pub lines: Vec<PlanLine>,
    /// English diagnostics under "技術的な詳細" (read problems, the rule that refused).
    pub details: String,
    /// "次に Windows の確認画面が出ます。発行元は『不明』と表示されます…" (one or two lines).
    pub uac_line: String,
    /// "変更する（次に Windows の確認が出ます）"; "変更する" when elevated (no prompt).
    pub apply_text: String,
    pub can_apply: bool,
}

/// Everything the page depends on besides the draft.
#[derive(Debug, Clone, Copy)]
pub struct ChangeContext<'a> {
    pub draft: &'a ChangeDraft,
    /// The display read's snapshot, until the preparation brings its own.
    pub display: Option<&'a SystemSnapshot>,
    /// `settings.change.uac_notice_seen`: from the second change on, the UAC prompt is explained
    /// by one line next to the button (review U14).
    pub uac_notice_seen: bool,
    /// The GUI runs elevated: no prompt follows (design m3 B.5).
    pub elevated: bool,
    /// The keep-or-revert time (20, or 60 with the setting).
    pub seconds: u32,
    /// The display name of the keyboard an answer key came from (`draft.other_keyboard`).
    pub other_name: Option<&'a str>,
    pub lang: Lang,
}

/// The value lines of `plan` (technical details): key, name, the value now and after. Also the
/// wizard's migration (design m3 B.1 step 4).
pub(crate) fn plan_lines(
    snapshot: &SystemSnapshot,
    plan: &OperationPlan,
    lang: Lang,
) -> Vec<PlanLine> {
    let mut lines = Vec::new();
    for step in &plan.checked.steps {
        let key = match &step.target {
            WriteTarget::Device { instance_id } => instance_id.clone(),
            WriteTarget::Global => i18n::global_values(lang),
        };
        for write in &step.writes {
            let now = model_value(
                &snapshot.keyboards,
                &snapshot.global,
                &step.target,
                &write.name,
            )
            .map_or_else(|| "?".to_string(), |value| i18n::reg_value(&value, lang));
            lines.push(PlanLine {
                key: key.clone(),
                name: write.name.clone(),
                now,
                after: i18n::reg_value(&op_value(&write.op), lang),
            });
        }
    }
    lines
}

/// The restore preview's parts of the page (design m2 D.5, m3 B.4).
pub fn restore_page(preview: &RestorePreview, lang: Lang) -> ChangePage {
    let mut page = ChangePage {
        restore: true,
        summary: i18n::restore_summary(preview.writes.len(), lang),
        lines: preview
            .writes
            .iter()
            .map(|row| PlanLine {
                key: row.key_path.clone(),
                name: row.name.clone(),
                now: i18n::reg_value(&row.current, lang),
                after: i18n::reg_value(&row.baseline, lang),
            })
            .collect(),
        ..ChangePage::default()
    };
    if !preview.conflicts.is_empty() {
        page.warnings
            .push(i18n::restore_conflicts(preview.conflicts.len(), lang));
    }
    if !preview.removed.is_empty() {
        page.warnings
            .push(i18n::restore_removed(preview.removed.len(), lang));
    }
    if !preview.supersedes.is_empty() {
        page.warnings
            .push(i18n::restore_supersedes(preview.supersedes.len(), lang));
    }
    match &preview.order {
        Ok(None) => {}
        Ok(Some(_)) => page
            .warnings
            .push(i18n::restore_keeps_inv_ps2_violation(lang)),
        Err(error) => {
            page.note = i18n::restore_refused(lang);
            page.note_tone = Tone::Warning;
            page.details = error.to_string();
        }
    }
    page
}

/// The page (design m3 B.4, B.5).
pub fn change_page(ctx: &ChangeContext<'_>) -> ChangePage {
    let draft = ctx.draft;
    let lang = ctx.lang;
    let snapshot = draft
        .prepared
        .as_ref()
        .map(|prepared| &prepared.snapshot)
        .or(ctx.display);
    let mut page = match draft.plan.as_ref().map(|plan| &plan.planned) {
        Some(Planned::Restore(preview)) if draft.restore => restore_page(preview, lang),
        _ => ChangePage {
            restore: draft.restore,
            ..ChangePage::default()
        },
    };
    page.title = if draft.restore {
        i18n::restore_title(&draft.name, lang)
    } else {
        i18n::change_title(&draft.name, lang)
    };
    let choices = layout_choices(snapshot, &draft.members);
    if !draft.restore {
        let (current, standard) = snapshot.map_or((None, LayoutTable::Jis), |snapshot| {
            current_choice(snapshot, &draft.members)
        });
        page.choices = choices
            .iter()
            .map(|choice| {
                let is_current = current == Some(*choice);
                let (text, detail) = i18n::layout_choice(*choice, &standard, is_current, lang);
                ChoiceRow {
                    text,
                    detail,
                    current: is_current,
                    enabled: true,
                }
            })
            .collect();
        page.selected = draft
            .choice
            .and_then(|choice| choices.iter().position(|c| *c == choice));
        page.detect = detect_panel(ctx, &choices, page.selected);
    }
    page.preparing = draft.preparing.is_some();
    page.all_users_note = i18n::all_users_note(lang);
    let (live_text, live_detail) = i18n::apply_method(true, ctx.seconds, lang);
    let (restart_text, restart_detail) = i18n::apply_method(false, ctx.seconds, lang);
    page.live_text = live_text;
    page.live_detail = live_detail;
    page.restart_text = restart_text;
    page.restart_detail = restart_detail;
    let prompt = !ctx.elevated;
    page.apply_text = i18n::apply_button(draft.restore, prompt, lang);
    if prompt && ctx.uac_notice_seen {
        page.uac_line = i18n::uac_line(lang);
    }
    let mut details: Vec<String> = Vec::new();
    if !page.details.is_empty() {
        details.push(std::mem::take(&mut page.details));
    }
    if let Some(prepared) = &draft.prepared {
        details.extend(prepared.warnings.iter().cloned());
    }
    if draft.choice == Some(LayoutChoice::Us) && !draft.restore {
        page.ime_note = i18n::ime_note_us(lang);
    }
    if let Some(failure) = &draft.failure {
        page.note = i18n::prepare_failed(matches!(failure, PrepareFailure::Incomplete(_)), lang);
        page.note_tone = Tone::Warning;
        details.push(failure.diagnostic().to_string());
    } else if let (Some(plan), false) = (&draft.plan, page.preparing) {
        let method = draft
            .method
            .or(draft.method_default.map(|default| default.method));
        if plan.method_offered {
            page.method_visible = true;
            page.method = method;
            if draft.method.is_none()
                && draft
                    .method_default
                    .is_some_and(|default| default.only_keyboard_warning)
            {
                page.warnings.push(i18n::only_keyboard_warning(lang));
            }
        }
        if let Some(apply) = plan.apply() {
            page.takes_effect = i18n::takes_effect(apply, ctx.seconds, lang);
        }
        match &plan.planned {
            Planned::Set(set) => {
                if let Some(snapshot) = snapshot {
                    page.lines = plan_lines(snapshot, set, lang);
                }
            }
            Planned::Migrate {
                standard,
                plan: migration,
            } => {
                page.migration_note = i18n::migration_note(lang);
                let fixed = snapshot.and_then(|snapshot| ps2_pin_layout(&snapshot.global));
                page.standard_visible = true;
                page.standard_choices = [Layout::Jis, Layout::Us]
                    .into_iter()
                    .map(|layout| ChoiceRow {
                        text: i18n::standard_choice(layout, fixed == Some(layout), lang),
                        current: fixed == Some(layout),
                        enabled: true,
                        ..ChoiceRow::default()
                    })
                    .collect();
                page.standard_selected = Some(match standard {
                    Layout::Jis => 0,
                    Layout::Us => 1,
                });
                if let Some(snapshot) = snapshot {
                    page.lines = plan_lines(snapshot, migration, lang);
                }
            }
            Planned::Restore(_) => {}
            Planned::Refused(error) => {
                page.note = i18n::operation_refused(error, lang);
                page.note_tone = Tone::Warning;
                details.push(error.to_string());
            }
        }
        if plan.nothing_to_change {
            page.takes_effect.clear();
            if !draft.restore {
                page.note = i18n::nothing_to_change(lang);
                page.note_tone = Tone::Info;
            }
        }
        if draft.choice == Some(LayoutChoice::Standard) && !draft.restore {
            page.warnings.push(i18n::standard_unverified(lang));
        }
        page.can_apply =
            draft_request(draft).is_some() && (!plan.method_offered || method.is_some());
    }
    page.details = details.join("\n");
    page
}

/// The detection panel for the draft's keyboard.
fn detect_panel(
    ctx: &ChangeContext<'_>,
    choices: &[LayoutChoice],
    selected: Option<usize>,
) -> DetectPanel {
    let draft = ctx.draft;
    let lang = ctx.lang;
    let detection = &draft.detection;
    let mut panel = DetectPanel {
        instruction: i18n::detect_instruction(detection.step, lang),
        ..DetectPanel::default()
    };
    if let Some(other) = ctx.other_name {
        panel.feedback = i18n::detect_other_keyboard(other, &draft.name, lang);
        panel.feedback_tone = Tone::Info;
    } else if detection.jis_hint && detection.verdict().is_none() {
        panel.feedback = i18n::detect_jis_hint(lang);
        panel.feedback_tone = Tone::Info;
    }
    if let Some(verdict) = detection.verdict() {
        panel.verdict = i18n::detect_verdict(verdict, &draft.name, lang);
        let layout = match verdict {
            Verdict::Jis => Some((LayoutChoice::Jis, Layout::Jis)),
            Verdict::Us => Some((LayoutChoice::Us, Layout::Us)),
            Verdict::Mixed => None,
        };
        if let Some((choice, layout)) = layout {
            panel.verdict_choice = choices.iter().position(|c| *c == choice);
            if panel.verdict_choice.is_some() && panel.verdict_choice != selected {
                panel.choose_text = i18n::detect_choose(layout, lang);
            }
        }
    }
    panel
}

impl SnapshotText for ChangePage {
    fn snapshot_text(&self) -> String {
        let mut out = format!("title: {}\n", self.title);
        for (index, choice) in self.choices.iter().enumerate() {
            let mark = if Some(index) == self.selected {
                "◉"
            } else {
                "○"
            };
            out.push_str(&format!("choice: {mark} {}", choice.text));
            if !choice.detail.is_empty() {
                out.push_str(&format!(" — {}", choice.detail));
            }
            out.push('\n');
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
        if self.preparing {
            out.push_str("preparing\n");
        }
        if self.method_visible {
            for (method, text, detail) in [
                (ApplyMethod::Live, &self.live_text, &self.live_detail),
                (
                    ApplyMethod::Restart,
                    &self.restart_text,
                    &self.restart_detail,
                ),
            ] {
                let mark = if self.method == Some(method) {
                    "◉"
                } else {
                    "○"
                };
                out.push_str(&format!("method: {mark} {text} — {detail}\n"));
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
        for (label, text) in [
            ("summary", &self.summary),
            ("takes effect", &self.takes_effect),
            ("migration", &self.migration_note),
            ("note", &self.note),
            ("ime", &self.ime_note),
        ] {
            if !text.is_empty() {
                out.push_str(&format!("{label}: {text}\n"));
            }
        }
        for warning in &self.warnings {
            out.push_str(&format!("warning: {warning}\n"));
        }
        out.push_str(&format!("all users: {}\n", self.all_users_note));
        for line in &self.lines {
            out.push_str(&format!(
                "line: {} {}: {} → {}\n",
                line.key, line.name, line.now, line.after
            ));
        }
        if !self.uac_line.is_empty() {
            out.push_str(&format!("uac: {}\n", self.uac_line));
        }
        out.push_str(&format!(
            "button: {}\ncan apply: {}\n",
            self.apply_text, self.can_apply
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{
        BaselineRecord, BootId, Journal, JournalEntry, OpId, OpKind, OpState, ProcessIdentity,
        RegValue, Timestamp, ValueKey, ValueRecord, fixtures, value_names,
    };

    use super::*;
    use crate::detect::{Detection, scancode};
    use crate::state::PreparedChange;

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

    fn prepared(snapshot: SystemSnapshot) -> PreparedChange {
        PreparedChange {
            snapshot,
            uncertain_values: false,
            warnings: Vec::new(),
            journal: Journal::default(),
            blocker: None,
        }
    }

    /// A draft for the Keychron row of the development machine, prepared for `choice`.
    fn draft(snapshot: SystemSnapshot, choice: LayoutChoice, method: ApplyMethod) -> ChangeDraft {
        let members = vec![KEYCHRON.to_string()];
        let target = target_instance(&snapshot, &members).unwrap();
        let plan = plan_change(&snapshot, false, &target, choice, method, None);
        ChangeDraft {
            choice: Some(choice),
            method_default: Some(MethodDefault {
                method,
                only_keyboard_warning: false,
            }),
            plan: Some(plan),
            prepared: Some(prepared(snapshot)),
            ..ChangeDraft::new("{F0D991EA}".into(), "Keychron Receiver".into(), members)
        }
    }

    fn page(draft: &ChangeDraft, lang: Lang) -> ChangePage {
        change_page(&ChangeContext {
            draft,
            display: None,
            uac_notice_seen: true,
            elevated: false,
            seconds: 20,
            other_name: None,
            lang,
        })
    }

    fn plain_japanese(text: &str) {
        let names = [
            "Keychron Receiver",
            "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)",
        ];
        assert!(
            crate::vm::unexpected_latin(text, &names).is_empty(),
            "{:?} in {text}",
            crate::vm::unexpected_latin(text, &names)
        );
    }

    #[test]
    fn switching_the_keychron_to_jis_now() {
        let draft = draft(
            fixtures::dev_machine(),
            LayoutChoice::Jis,
            ApplyMethod::Live,
        );
        let plan = draft.plan.as_ref().unwrap();
        assert!(plan.method_offered);
        assert_eq!(plan.apply(), Some(PendingAction::ResetKeyboard));
        let ja = page(&draft, Lang::Ja);
        assert!(ja.can_apply);
        assert_eq!(
            ja.snapshot_text(),
            "title: Keychron Receiver の配列\n\
             choice: ◉ JIS\n\
             choice: ○ US（現在）\n\
             choice: ○ 標準に従う（今は JIS） — 値を消して PC の標準配列に従わせます。打鍵での確認がまだです\n\
             detect: わからないときは、このキーボードで Backspace の左のキーを押してください（打鍵テスト）\n\
             method: ◉ すぐに切り替えて 20 秒間試す — このキーボードは数秒間使えません。その間は、ほかのキーボードかマウスで操作します。\n\
             method: ○ PC の再起動で切り替える — 再起動するまで、ほかのキーボードの配列も変更できません。\n\
             takes effect: キーボードをその場でリセットします（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。数秒間このキーボードで入力できません。その後 20 秒以内に「このままにする」を選ばないと元に戻ります。\n\
             all users: ⓘ この設定は、この PC のすべてのユーザーに適用されます。\n\
             line: HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000 KeyboardTypeOverride: 4 → 7\n\
             line: HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000 KeyboardSubtypeOverride: 0 → 2\n\
             uac: 次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。\n\
             button: 変更する（次に Windows の確認が出ます）\n\
             can apply: true\n"
        );
        plain_japanese(&crate::vm::snapshot_values(&ja.snapshot_text()));
        let en = page(&draft, Lang::En);
        assert_eq!(
            en.snapshot_text(),
            "title: Layout of Keychron Receiver\n\
             choice: ◉ JIS\n\
             choice: ○ US (current)\n\
             choice: ○ Follow the PC's standard layout (now JIS) — Removes the values so that the keyboard follows the PC's standard layout. Not yet verified by typing\n\
             detect: Not sure? Press the key left of Backspace on this keyboard (key test)\n\
             method: ◉ Switch now and try it for 20 seconds — This keyboard stops working for a few seconds; use another keyboard or the mouse meanwhile.\n\
             method: ○ Switch when the PC restarts — Until the restart, no other keyboard can be changed either.\n\
             takes effect: The keyboard is reset in place (Windows reconnects it; the keyboard's own settings do not change); it cannot type for a few seconds. Then choose \"Keep\" within 20 seconds, or it reverts.\n\
             all users: ⓘ This setting applies to every user of this PC.\n\
             line: HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000 KeyboardTypeOverride: 4 → 7\n\
             line: HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000 KeyboardSubtypeOverride: 0 → 2\n\
             uac: Windows asks for permission next (the publisher shows as \"Unknown\"). Check that it names mklm-helper.exe, then choose Yes.\n\
             button: Change (Windows asks next)\n\
             can apply: true\n"
        );
        // What is shown is what is sent (design m2 S6).
        let (request, apply) = change_request(KEYCHRON, draft.choice, plan).unwrap();
        let Planned::Set(set) = &plan.planned else {
            panic!("{plan:?}")
        };
        assert_eq!(
            request,
            Request::SetLayout(SetLayoutRequest {
                instance_id: KEYCHRON.into(),
                layout: LayoutChoice::Jis,
                apply: ApplyMethod::Live.options(),
                expected: Some(expected(set)),
            })
        );
        assert_eq!(apply, ApplyMethod::Live.options());
    }

    #[test]
    fn switching_at_the_restart_plans_a_restart_and_says_so() {
        // T-POST-1: "PC の再起動で切り替える" plans `RestartPc` for the USB receiver, and the page
        // says that nothing else can change until then (review U1).
        let mut draft = draft(
            fixtures::dev_machine(),
            LayoutChoice::Jis,
            ApplyMethod::Restart,
        );
        draft.method_default = Some(MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: true,
        });
        let plan = draft.plan.as_ref().unwrap();
        assert_eq!(plan.apply(), Some(PendingAction::RestartPc));
        assert!(plan.method_offered);
        let ja = page(&draft, Lang::Ja);
        assert_eq!(ja.method, Some(ApplyMethod::Restart));
        assert_eq!(
            ja.takes_effect,
            "PC を再起動すると反映されます（シャットダウンではなく再起動）。再起動するまで、MKLM でほかの変更はできません。"
        );
        assert_eq!(
            ja.warnings,
            vec!["このキーボードは、最近入力のあった唯一のキーボードです。PC の再起動で反映することをおすすめします。".to_string()]
        );
        let (request, _) = change_request(KEYCHRON, draft.choice, plan).unwrap();
        let Request::SetLayout(set) = request else {
            panic!()
        };
        assert!(!set.apply.other_input_available);
        assert_eq!(set.expected.unwrap().apply, Some(PendingAction::RestartPc));
    }

    #[test]
    fn the_current_layout_needs_no_prompt() {
        let draft = draft(fixtures::dev_machine(), LayoutChoice::Us, ApplyMethod::Live);
        assert!(draft.plan.as_ref().unwrap().nothing_to_change);
        let ja = page(&draft, Lang::Ja);
        assert!(!ja.can_apply);
        assert_eq!(ja.note, "変更はありません（すでにこの設定です）。");
        assert_eq!(ja.takes_effect, "");
        // US chosen: the IME note is on the page (plan 3.4).
        assert_eq!(
            ja.ime_note,
            "US 配列には「半角/全角」キーがありません。日本語入力のオン/オフは Alt+` です。"
        );
        plain_japanese(&ja.ime_note);
        assert!(change_request(KEYCHRON, draft.choice, draft.plan.as_ref().unwrap()).is_none());
    }

    #[test]
    fn the_standard_choice_is_for_hid_keyboards_only() {
        let snapshot = fixtures::dev_machine();
        assert_eq!(
            layout_choices(Some(&snapshot), &[BUILT_IN.to_string()]),
            vec![LayoutChoice::Jis, LayoutChoice::Us]
        );
        assert_eq!(
            layout_choices(Some(&snapshot), &[KEYCHRON.to_string()]).len(),
            3
        );
        let draft = draft(snapshot, LayoutChoice::Standard, ApplyMethod::Live);
        let ja = page(&draft, Lang::Ja);
        assert_eq!(ja.selected, Some(2));
        assert!(
            ja.warnings
                .contains(&"打鍵での確認がまだです。後で Shift+2 で確かめてください。".to_string()),
            "{:?}",
            ja.warnings
        );
    }

    #[test]
    fn the_built_in_keyboard_changes_at_the_restart_without_a_choice() {
        let snapshot = fixtures::dev_machine();
        let members = vec![BUILT_IN.to_string()];
        let plan = plan_change(
            &snapshot,
            false,
            BUILT_IN,
            LayoutChoice::Us,
            ApplyMethod::Live,
            None,
        );
        // PS/2 changes take effect at the restart: no method to choose, nothing claimed.
        assert!(!plan.method_offered);
        assert_eq!(plan.options, ApplyOptions::default());
        assert_eq!(plan.apply(), Some(PendingAction::RestartPc));
        let draft = ChangeDraft {
            choice: Some(LayoutChoice::Us),
            plan: Some(plan),
            prepared: Some(prepared(snapshot)),
            ..ChangeDraft::new(BUILT_IN.into(), "内蔵".into(), members)
        };
        let en = page(&draft, Lang::En);
        assert!(!en.method_visible && en.can_apply);
        assert!(
            en.takes_effect
                .starts_with("It takes effect when the PC restarts")
        );
    }

    #[test]
    fn fixed_mode_migrates_with_the_assignment() {
        let mut snapshot = fixtures::dev_machine();
        snapshot.global = fixtures::global_fixed_jis();
        let draft = draft(snapshot, LayoutChoice::Us, ApplyMethod::Live);
        let plan = draft.plan.as_ref().unwrap();
        let Planned::Migrate { standard, .. } = &plan.planned else {
            panic!("{plan:?}")
        };
        assert_eq!(*standard, Layout::Jis);
        let ja = page(&draft, Lang::Ja);
        assert_eq!(
            ja.migration_note,
            "この PC は固定モードです。キーボードごとモードへ移行し、この割り当ても同時に書きます（PC の再起動が 1 回必要）。"
        );
        assert!(ja.standard_visible && !ja.method_visible && ja.can_apply);
        assert_eq!(
            ja.standard_choices
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["JIS（おすすめ: 今の JIS）", "US"]
        );
        assert_eq!(ja.standard_selected, Some(0));
        plain_japanese(&ja.migration_note);
        let (request, _) = change_request(KEYCHRON, draft.choice, plan).unwrap();
        let Request::Migrate(migrate) = request else {
            panic!()
        };
        assert_eq!(migrate.standard, Layout::Jis);
        assert_eq!(
            migrate.assignments,
            vec![Assignment {
                instance_id: KEYCHRON.into(),
                layout: LayoutChoice::Us
            }]
        );
        assert_eq!(
            migrate.expected.unwrap().apply,
            Some(PendingAction::RestartPc)
        );
    }

    #[test]
    fn refused_and_unreadable() {
        let snapshot = fixtures::dev_machine();
        let mut draft = draft(snapshot, LayoutChoice::Jis, ApplyMethod::Live);
        draft.plan = Some(DraftPlan::refused(OperationError::InconsistentGlobal));
        let ja = page(&draft, Lang::Ja);
        assert!(!ja.can_apply);
        assert_eq!(
            ja.note,
            "PC 全体のキーボードの値が食い違っているため、この変更はできません。"
        );
        assert!(ja.details.contains("inconsistent"), "{}", ja.details);
        draft.failure = Some(PrepareFailure::Incomplete("devnode X: no driver".into()));
        let en = page(&draft, Lang::En);
        assert!(!en.can_apply);
        assert!(en.note.starts_with("Windows did not report every keyboard"));
        assert!(en.details.contains("devnode X"));
        // While preparing: "確認しています…", nothing can be sent.
        draft.failure = None;
        draft.preparing = Some(3);
        let preparing = page(&draft, Lang::Ja);
        assert!(preparing.preparing && !preparing.can_apply && !preparing.method_visible);
    }

    #[test]
    fn the_uac_explanation_and_the_button() {
        let draft = draft(
            fixtures::dev_machine(),
            LayoutChoice::Jis,
            ApplyMethod::Live,
        );
        let context = |seen, elevated| ChangeContext {
            draft: &draft,
            display: None,
            uac_notice_seen: seen,
            elevated,
            seconds: 60,
            other_name: None,
            lang: Lang::Ja,
        };
        // The first time, the standalone explanation follows the button (review U14).
        let first = change_page(&context(false, false));
        assert_eq!(first.uac_line, "");
        assert_eq!(first.apply_text, "変更する（次に Windows の確認が出ます）");
        assert_eq!(first.live_text, "すぐに切り替えて 60 秒間試す");
        // Elevated: no prompt, nothing to explain.
        let elevated = change_page(&context(true, true));
        assert_eq!(
            (elevated.uac_line.as_str(), elevated.apply_text.as_str()),
            ("", "変更する")
        );
    }

    #[test]
    fn the_detection_on_the_page() {
        let mut draft = draft(
            fixtures::dev_machine(),
            LayoutChoice::Jis,
            ApplyMethod::Live,
        );
        draft.detection = Detection::for_keyboards(draft.members.clone());
        let with = |draft: &ChangeDraft, other: Option<&str>| {
            change_page(&ChangeContext {
                draft,
                display: None,
                uac_notice_seen: true,
                elevated: false,
                seconds: 20,
                other_name: other,
                lang: Lang::Ja,
            })
            .detect
        };
        let other = with(&draft, Some("日本語 PS/2 キーボード"));
        assert_eq!(
            other.feedback,
            "日本語 PS/2 キーボード のキーです。Keychron Receiver で押してください"
        );
        draft.detection.press(KEYCHRON, scancode::EQUAL);
        assert_eq!(
            with(&draft, None).instruction,
            "次に、このキーボードで右の Shift の左のキーを押してください"
        );
        draft.detection.press(KEYCHRON, scancode::SLASH);
        let done = with(&draft, None);
        assert_eq!(done.instruction, "");
        assert_eq!(
            done.verdict,
            "✓ Keychron Receiver は US 配列のキーボードです"
        );
        assert_eq!(done.verdict_choice, Some(1));
        assert_eq!(done.choose_text, "US を選ぶ");
        plain_japanese(&format!(
            "{} {} {}",
            done.instruction, done.verdict, done.choose_text
        ));
        let mut mixed = Detection::for_keyboards(vec![KEYCHRON.into()]);
        mixed.press(KEYCHRON, scancode::YEN);
        mixed.press(KEYCHRON, scancode::SLASH);
        draft.detection = mixed;
        let mixed = with(&draft, None);
        assert_eq!(mixed.verdict_choice, None);
        assert!(mixed.verdict.starts_with("判定できませんでした"));
    }

    /// The Keychron was JIS (7/2) before MKLM; a kept change made it US (4/0, as the fixture
    /// stores it now).
    fn keychron_baselines() -> Journal {
        let op = OpId::parse("1ef48b2f-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let key = |name: &str| ValueKey {
            target: WriteTarget::Device {
                instance_id: KEYCHRON.into(),
            },
            name: name.into(),
        };
        let baseline = |name: &str, value: u32| BaselineRecord {
            schema_version: 1,
            key_path: key(name).key_path(),
            key: key(name),
            value: RegValue::Dword { value },
            captured_at: Timestamp(1_790_000_000_000),
            captured_by: op.clone(),
        };
        let record = |name: &str, before: u32, written: u32| ValueRecord {
            target: key(name).target,
            key_path: key(name).key_path(),
            name: name.into(),
            baseline: RegValue::Dword { value: before },
            before: RegValue::Dword { value: before },
            intended: RegValue::Dword { value: written },
            last_written: Some(RegValue::Dword { value: written }),
            conflict: None,
            resolve_to: None,
            write_error: None,
            skipped: None,
        };
        let entry = JournalEntry {
            schema_version: 1,
            op_id: op.clone(),
            seq: 1,
            kind: OpKind::SetLayout {
                requested: KEYCHRON.into(),
                instance_ids: vec![KEYCHRON.into()],
                layout: LayoutChoice::Us,
            },
            state: OpState::Confirmed,
            boot_id: BootId(1),
            owner: ProcessIdentity {
                pid: 1,
                creation_time: 1,
            },
            created_at: Timestamp(1_790_000_000_000),
            updated_at: Timestamp(1_790_000_000_000),
            apply: Some(PendingAction::ResetKeyboard),
            countdown: None,
            records: vec![
                record(value_names::HID_TYPE, 7, 4),
                record(value_names::HID_SUBTYPE, 2, 0),
            ],
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        };
        Journal {
            entries: vec![entry],
            baselines: vec![
                baseline(value_names::HID_TYPE, 7),
                baseline(value_names::HID_SUBTYPE, 2),
            ],
            ..Journal::default()
        }
    }

    #[test]
    fn the_restore_preview() {
        let snapshot = fixtures::dev_machine();
        let journal = keychron_baselines();
        let plan = plan_restore(&snapshot, &journal, KEYCHRON, ApplyMethod::Live);
        let Planned::Restore(preview) = &plan.planned else {
            panic!("{plan:?}")
        };
        assert_eq!(preview.writes.len(), 2);
        assert!(plan.method_offered && !plan.nothing_to_change);
        let draft = ChangeDraft {
            restore: true,
            plan: Some(plan.clone()),
            method_default: Some(MethodDefault {
                method: ApplyMethod::Live,
                only_keyboard_warning: false,
            }),
            prepared: Some(PreparedChange {
                journal: journal.clone(),
                ..prepared(snapshot.clone())
            }),
            ..ChangeDraft::new(
                "{F0D991EA}".into(),
                "Keychron Receiver".into(),
                vec![KEYCHRON.into()],
            )
        };
        let ja = page(&draft, Lang::Ja);
        assert!(ja.restore && ja.choices.is_empty() && ja.can_apply);
        assert_eq!(ja.title, "Keychron Receiver を MKLM 導入前に戻す");
        assert_eq!(ja.summary, "2 件の値を MKLM 導入前の値に戻します。");
        assert_eq!(ja.apply_text, "元に戻す（次に Windows の確認が出ます）");
        assert_eq!(ja.lines[0].after, "7");
        plain_japanese(&format!("{} {} {}", ja.title, ja.summary, ja.apply_text));
        let (request, _) = change_request(KEYCHRON, None, &plan).unwrap();
        assert_eq!(
            request,
            Request::RestoreBaseline(RestoreBaselineRequest {
                scope: RestoreScope::Device {
                    instance_id: KEYCHRON.into()
                },
                on_conflict: ConflictPolicy::Report,
                apply: ApplyMethod::Live.options(),
            })
        );
        // Nothing recorded: nothing to put back.
        let nothing = plan_restore(&snapshot, &Journal::default(), KEYCHRON, ApplyMethod::Live);
        assert!(nothing.nothing_to_change);
        assert!(change_request(KEYCHRON, None, &nothing).is_none());
        // A value changed outside MKLM is reported, not overwritten.
        let mut changed = snapshot;
        changed.keyboards[1].overrides.keyboard_type_override = Some(8);
        let conflict = plan_restore(&changed, &journal, KEYCHRON, ApplyMethod::Live);
        let Planned::Restore(preview) = &conflict.planned else {
            panic!()
        };
        assert_eq!(preview.conflicts.len(), 1);
        let page = restore_page(preview, Lang::Ja);
        assert!(page.warnings[0].starts_with("MKLM 以外が変更した値が 1 件あります"));
    }

    #[test]
    fn a_global_value_line() {
        let mut snapshot = fixtures::dev_machine();
        snapshot.global = fixtures::global_fixed_jis();
        let plan = plan_change(
            &snapshot,
            false,
            KEYCHRON,
            LayoutChoice::Us,
            ApplyMethod::Live,
            Some(Layout::Us),
        );
        let Planned::Migrate { plan, .. } = &plan.planned else {
            panic!()
        };
        let lines = plan_lines(&snapshot, plan, Lang::Ja);
        assert!(lines.iter().any(|line| line.key == "PC 全体"), "{lines:?}");
    }
}
