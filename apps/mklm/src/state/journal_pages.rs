//! The flows of the pages about journal entries (design m3 B.8 to B.12, B.18; WP-U4, WP-U5): the
//! PC restart, the check after it, conflicts, the history and the recovery page with its undo and
//! revert previews. Pure like the rest of `state`: the page-local state lives in
//! [`JournalPages`], the messages in [`JournalMsg`], the effects only the I/O worker and the
//! window can carry out in [`JournalEffect`]; everything else goes through the common
//! [`Effect`]s — a request always through `AppMsg::StartRequest`, so the one-session rule and the
//! session flow of WP-U3 (progress, result, "今の状態") are the same for every page.
//!
//! The view functions ([`restart_view`], [`post_reboot_view`], [`conflict_view`],
//! [`journal_view`], [`recovery_view`]) build each page from the last `SystemRead`; `app.rs`
//! renders them, and the buttons are decided from the same views, so what is shown is what is
//! sent (the apply method in particular).
//!
//! Never a UAC prompt without a button press (design m3 0.2 principle 6, K.6): pages open by
//! themselves (the post-reboot check, the recovery page once per entry and boot), but every
//! request starts from a button.

use mklm_client::gate::{post_reboot_entries, takes_effect_at_restart};
use mklm_client::orchestrator::{RequestEnd, RequestReport};
use mklm_client::preview::{CheckRow, check_rows, undo_preview};
use mklm_client::run_once::{RunOnceError, RunOnceOutcome};
use mklm_client::session::SessionEnd;
use mklm_client::values::model_value;
use mklm_core::{
    ApplyOptions, Attention, ConflictInfo, ConflictPolicy, ErrorCode, JournalEntry, LayoutTable,
    OpId, OpState, PlanError, RecoveryContext, RegValue, ResolutionChoice, RestoreScope, Timestamp,
    ValueRecord, WriteTarget, assess, check_inv_ps2,
};
use mklm_ipc::{Request, ResolveConflictRequest, RestoreBaselineRequest};

use super::{
    AppMsg, AppState, Effect, OverlayKind, Page, SessionPhase, SessionTarget, display_name, update,
};
use crate::i18n::Lang;
use crate::settings::PromptedEntry;
use crate::vm::change::{ApplyMethod, default_apply_method};
use crate::vm::conflict::{self, ConflictPage, RESTORE_POLICIES, VALUE_CHOICES};
use crate::vm::journal::{self as history, JournalPage};
use crate::vm::keytest::KeyTest;
use crate::vm::post_reboot::{self, PostReboot, Typed};
use crate::vm::recovery::{self, AttentionEntry, RecoveryMode, RecoveryPage};
use crate::vm::restart::{self, Restart};

/// How long the post-reboot check stays on top unless the user acts first (design m3 B.9).
pub const ON_TOP_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// Page-local state of the journal pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JournalPages {
    pub restart: RestartState,
    pub post_reboot: PostRebootState,
    pub conflict: ConflictState,
    pub recovery: RecoveryState,
    /// The last RunOnce rule of the I/O worker could not register the check after a restart:
    /// `Some(true)` when this process is elevated (`RunOnceOutcome::TellUser`), `Some(false)` when
    /// the rule failed.
    pub run_once_problem: Option<bool>,
    /// "復旧用ファイルのフォルダーを開く" failed.
    pub folder_failed: bool,
    /// The first `SystemRead` of this process was looked at (design m3 F.2: `--tray` still shows
    /// the window when the journal needs the user).
    pub startup_checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestartState {
    /// `restart_pc` was asked for; the button stays off meanwhile.
    pub restarting: bool,
    /// `restart_pc` failed (English diagnostic, for the details).
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostRebootState {
    /// Started with `--post-reboot` (the RunOnce value): the first read opens the check even for
    /// a `PendingReboot` of this boot, to say that the PC was shut down, not restarted (design
    /// m2 D.7 step 2, m3 T-POST-3).
    pub requested: bool,
    /// The entry the page shows (the oldest post-reboot entry unless the user picked one).
    pub op_id: Option<OpId>,
    /// The key tests of this check, per instance ID (review U2): only the verdict, in memory,
    /// never the keys themselves (plan 2.2).
    pub typed: Vec<(String, Typed)>,
    /// Entries this process opened the check for by itself (at most once each).
    pub shown: Vec<OpId>,
    /// The window is kept on top (until the user acts, or [`ON_TOP_FOR`]).
    pub on_top: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConflictState {
    /// The entry the choices below belong to.
    pub op_id: Option<OpId>,
    /// Per keyboard (index on the page) the option index chosen.
    pub selections: Vec<(usize, usize)>,
    /// Per record a choice made in the details (`None`: follow the keyboard).
    pub overrides: Vec<(usize, Option<ResolutionChoice>)>,
    /// A resolution of this entry was sent; its result may carry the INV-PS2 refusal.
    pub resolving: Option<OpId>,
    /// The helper refused the last resolution for INV-PS2: the PS/2 keyboards without a pin.
    pub inv_ps2: Option<Vec<String>>,
    /// The last restore to before MKLM stopped at values changed outside MKLM: the page asks how
    /// to send it again (design m3 B.10). Nothing is journaled for it.
    pub restore: Option<RestoreConflicts>,
}

/// A restore to before MKLM that stopped before writing (`ConflictPolicy::Report`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreConflicts {
    pub scope: RestoreScope,
    /// The apply options of the stopped request (sent again unchanged).
    pub apply: ApplyOptions,
    pub conflicts: Vec<ConflictInfo>,
    /// The user's pick; `None` follows the recommendation.
    pub policy: Option<ConflictPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryState {
    pub mode: RecoveryMode,
    /// The operation the revert preview is about.
    pub revert: Option<OpId>,
    /// Where "キャンセル" of a preview goes back to.
    pub back: Option<Page>,
    /// How a HID keyboard takes the values back: preset when the page opens
    /// (`vm::change::default_apply_method` at that moment), then the user's choice. What the page
    /// shows is what is sent.
    pub method: Option<ApplyMethod>,
}

/// Something that happened on the journal pages.
#[derive(Debug, Clone, PartialEq)]
pub enum JournalMsg {
    /// "今すぐ再起動" with the acknowledgement's state.
    RestartNow {
        acknowledged: bool,
    },
    RestartLater,
    /// The I/O worker could not restart the PC (English diagnostic).
    RestartFailed(String),
    /// The I/O worker read the journal again before the restart and found no reason for it any
    /// more (decided elsewhere meanwhile): nothing was restarted.
    RestartNotNeeded,
    /// "確認待ちの変更をすべて元に戻す…" (restart page, conflict page, recovery page, tray).
    OpenUndo,
    PostRebootKeep,
    PostRebootRevert,
    PostRebootLater,
    /// The on-top period of the post-reboot check ended.
    OnTopExpired,
    /// The conflict page: keyboard `keyboard` gets option `option`.
    ConflictChoice {
        keyboard: usize,
        option: usize,
    },
    /// The conflict details: record `record` gets `VALUE_CHOICES[option]`.
    ConflictValueChoice {
        record: usize,
        option: usize,
    },
    /// A stopped restore: `RESTORE_POLICIES[index]`.
    ConflictRestoreChoice(usize),
    ConflictResolve,
    /// The history's "元に戻す…" of an operation (full ID).
    JournalRevert {
        op_id: String,
    },
    OpenRecoveryFolder,
    RecoveryFolderFailed,
    /// The recovery page's apply method (0 = switch back now, 1 = when replugged or restarted).
    RecoveryChooseMethod(usize),
    /// The recovery page's primary button ("回復する", "すべて元に戻す", "元に戻す").
    RecoveryPrimary,
    /// "このままにする" of an entry that waits for the user.
    RecoveryKeep {
        op_id: String,
    },
    /// "元に戻す…" of an entry that waits for the user: the revert preview.
    RecoveryRevert {
        op_id: String,
    },
    /// "後で" / "キャンセル".
    RecoveryDismiss,
    /// The I/O worker applied the RunOnce rule.
    RunOnceDone(Result<RunOnceOutcome, RunOnceError>),
}

/// What only the I/O worker or the window can do for these pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalEffect {
    /// Read the journal again, apply the RunOnce rule, then `mklm_win::session::restart_pc` (on
    /// the I/O worker).
    RestartPc,
    /// Keep the window on top (the post-reboot check, design m3 B.9, J.6), or stop.
    AlwaysOnTop(bool),
    /// `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` in Explorer (on the I/O worker).
    OpenRecoveryFolder,
}

fn lang(state: &AppState) -> Lang {
    state.lang.unwrap_or(Lang::Ja)
}

/// A request from a button of these pages, through the one-session rule and the session flow of
/// WP-U3 (`state::start_request`); `entries` are the operations it acts on, whose keyboards the
/// result's "今の状態" names (design m3 B.17).
fn start(
    state: &mut AppState,
    request: Request,
    apply: ApplyOptions,
    entries: &[OpId],
) -> Vec<Effect> {
    let targets = entry_targets(state, entries);
    super::start_request(state, request, apply, targets)
}

/// The keyboards the operations `entries` wrote, one per name, with how they type now.
fn entry_targets(state: &AppState, entries: &[OpId]) -> Vec<SessionTarget> {
    let Some(journal) = state.read.as_ref().and_then(|read| read.journal.as_ref()) else {
        return Vec::new();
    };
    let assessment = state
        .read
        .as_ref()
        .and_then(|read| read.snapshot.as_ref())
        .map(assess);
    let mut targets: Vec<SessionTarget> = Vec::new();
    let ids = entries
        .iter()
        .filter_map(|op| journal.entry(op))
        .flat_map(|entry| entry.records.iter())
        .filter_map(|record| match &record.target {
            WriteTarget::Device { instance_id } => Some(instance_id),
            WriteTarget::Global => None,
        });
    for instance_id in ids {
        let name = display_name(state, instance_id);
        if let Some(target) = targets.iter_mut().find(|target| target.name == name) {
            if !target
                .members
                .iter()
                .any(|member| member.eq_ignore_ascii_case(instance_id))
            {
                target.members.push(instance_id.clone());
            }
            continue;
        }
        let before = assessment.as_ref().and_then(|assessment| {
            assessment
                .keyboards
                .iter()
                .find(|ka| ka.instance_id.eq_ignore_ascii_case(instance_id))
                .and_then(|ka| ka.current.as_ref())
                .map(|layout| layout.table.clone())
        });
        targets.push(SessionTarget {
            name,
            members: vec![instance_id.clone()],
            before,
        });
    }
    targets
}

fn stop_on_top(state: &mut AppState) -> Option<Effect> {
    let post = &mut state.journal_pages.post_reboot;
    post.on_top.then(|| {
        post.on_top = false;
        Effect::Journal(JournalEffect::AlwaysOnTop(false))
    })
}

fn find_entry<'a>(state: &'a AppState, op_id: &str) -> Option<&'a JournalEntry> {
    let journal = state.read.as_ref()?.journal.as_ref()?;
    journal
        .entries
        .iter()
        .find(|entry| entry.op_id.as_str().eq_ignore_ascii_case(op_id))
}

/// The value stored now for `record`, from the display snapshot (`None`: not known).
fn current_of(state: &AppState, record: &ValueRecord) -> Option<RegValue> {
    let snapshot = state.read.as_ref()?.snapshot.as_ref()?;
    model_value(
        &snapshot.keyboards,
        &snapshot.global,
        &record.target,
        &record.name,
    )
}

// --- Views ---------------------------------------------------------------------------------

/// The restart page from the last read.
pub fn restart_view(state: &AppState) -> Restart {
    let lang = lang(state);
    let name_of = |id: &str| display_name(state, id);
    match state
        .read
        .as_ref()
        .and_then(|read| Some((read.journal.as_ref()?, read.boot?)))
    {
        Some((journal, boot)) => restart::restart(journal, boot, &name_of, lang),
        None => restart::nothing(lang),
    }
}

/// The entry the post-reboot check shows: the one picked, while it still waits, else the oldest.
pub fn post_reboot_entry(state: &AppState) -> Option<&JournalEntry> {
    let journal = state.read.as_ref()?.journal.as_ref()?;
    let entries = post_reboot_entries(journal);
    state
        .journal_pages
        .post_reboot
        .op_id
        .as_ref()
        .and_then(|picked| entries.iter().find(|entry| entry.op_id == *picked))
        .or_else(|| entries.first())
        .copied()
}

fn check_rows_of(state: &AppState, entry: &JournalEntry) -> Vec<CheckRow> {
    state
        .read
        .as_ref()
        .and_then(|read| read.snapshot.as_ref())
        .map_or_else(Vec::new, |snapshot| check_rows(snapshot, entry))
}

/// The post-reboot check from the last read and the key tests so far.
pub fn post_reboot_view(state: &AppState) -> PostReboot {
    let lang = lang(state);
    let (Some(entry), Some(boot)) = (
        post_reboot_entry(state),
        state.read.as_ref().and_then(|read| read.boot),
    ) else {
        return post_reboot::nothing(lang);
    };
    let rows = check_rows_of(state, entry);
    let mut view = post_reboot::post_reboot(
        entry,
        &rows,
        &state.journal_pages.post_reboot.typed,
        boot,
        &|id| display_name(state, id),
        lang,
    );
    no_prompt_when_elevated(state, &mut view.uac_line);
    view
}

/// An elevated GUI launches the helper without a Windows prompt: the line about it goes (design
/// m3 B.5).
fn no_prompt_when_elevated(state: &AppState, uac_line: &mut String) {
    if state.elevated {
        uac_line.clear();
    }
}

/// The row of the post-reboot check a key from `source` belongs to: the same devnode, else
/// another collection of the same (external) physical device.
fn check_row_for<'a>(state: &AppState, rows: &'a [CheckRow], source: &str) -> Option<&'a CheckRow> {
    rows.iter()
        .find(|row| row.instance_id.eq_ignore_ascii_case(source))
        .or_else(|| {
            let snapshot = state.read.as_ref()?.snapshot.as_ref()?;
            let container = |id: &str| {
                snapshot
                    .keyboards
                    .iter()
                    .find(|kb| kb.instance_id.eq_ignore_ascii_case(id))
                    .filter(|kb| !kb.in_internal_container())
                    .and_then(|kb| kb.known_container_id())
                    .map(str::to_ascii_lowercase)
            };
            let source = container(source)?;
            rows.iter()
                .find(|row| container(&row.instance_id).as_deref() == Some(source.as_str()))
        })
}

/// What the key test on the post-reboot check is judged against (review U2): the row of the
/// keyboard that sent the key, `(targets, name, table)`; `None` elsewhere.
pub fn key_expectation(state: &AppState) -> Option<(Vec<String>, String, LayoutTable)> {
    if state.page != Page::PostReboot || state.overlay != OverlayKind::None {
        return None;
    }
    let entry = post_reboot_entry(state)?;
    let (source, _) = state.last_key.as_ref()?;
    let rows = check_rows_of(state, entry);
    let row = check_row_for(state, &rows, source)?;
    let table = post_reboot::expected_table(row)?.clone();
    Some((
        vec![row.instance_id.clone(), source.clone()],
        row.name.clone(),
        table,
    ))
}

/// A key in the post-reboot check's key test: fills the row's "typed" column. The first key the
/// user types there also ends the on-top period (the user is acting on the page).
pub fn key_typed(state: &mut AppState, text: &str, shift: bool) -> Vec<Effect> {
    if state.page != Page::PostReboot || state.overlay != OverlayKind::None {
        return Vec::new();
    }
    let mut effects: Vec<Effect> = stop_on_top(state).into_iter().collect();
    let Some(entry) = post_reboot_entry(state) else {
        return effects;
    };
    let Some((source, scancode)) = state.last_key.clone() else {
        return effects;
    };
    let rows = check_rows_of(state, entry);
    let Some(row) = check_row_for(state, &rows, &source) else {
        return effects;
    };
    let Some(typed) = post_reboot::typed_for(
        text,
        shift,
        scancode,
        state.active_hkl,
        post_reboot::expected_table(row),
    ) else {
        return effects;
    };
    let id = row.instance_id.clone();
    let post = &mut state.journal_pages.post_reboot;
    match post
        .typed
        .iter_mut()
        .find(|(known, _)| known.eq_ignore_ascii_case(&id))
    {
        Some((_, known)) => *known = typed,
        None => post.typed.push((id, typed)),
    }
    effects.push(Effect::Render);
    effects
}

/// The entry in conflict the page shows (the oldest).
pub fn conflict_entry(state: &AppState) -> Option<&JournalEntry> {
    let journal = state.read.as_ref()?.journal.as_ref()?;
    journal
        .open_entries()
        .into_iter()
        .find(|entry| entry.state == OpState::Conflict)
}

/// The conflict page with the choices made so far: a restore that stopped (just now, from this
/// window) comes first, else the oldest operation in `Conflict`.
pub fn conflict_view(state: &AppState) -> ConflictPage {
    let lang = lang(state);
    let name_of = |id: &str| display_name(state, id);
    let draft = &state.journal_pages.conflict;
    if let Some(restore) = &draft.restore {
        let mut view = conflict::restore_conflict_page(
            &restore.scope,
            &restore.conflicts,
            restore.policy,
            &|record| current_of(state, record),
            &name_of,
            lang,
        );
        no_prompt_when_elevated(state, &mut view.uac_line);
        return view;
    }
    let entry = conflict_entry(state);
    let mut keyboards = entry.map_or_else(Vec::new, |entry| {
        conflict::conflict_keyboards(entry, &|record| current_of(state, record), &name_of, lang)
    });
    let same = entry.is_some_and(|entry| draft.op_id.as_ref() == Some(&entry.op_id));
    if same {
        conflict::apply_choices(&mut keyboards, &draft.selections, &draft.overrides);
    }
    let error = draft
        .inv_ps2
        .as_ref()
        .filter(|_| same)
        .map(|keyboards| conflict::inv_ps2_error(keyboards, &name_of, lang));
    let mut view = conflict::conflict_page(entry, keyboards, error, &name_of, lang);
    no_prompt_when_elevated(state, &mut view.uac_line);
    view
}

/// The history page; `time_text` formats a journal time (local time in the GUI).
pub fn journal_view(state: &AppState, time_text: &dyn Fn(Timestamp) -> String) -> JournalPage {
    let lang = lang(state);
    let read = state.read.as_ref();
    let summary = read.map(|read| read.summary.clone()).unwrap_or_default();
    history::journal_page(
        read.and_then(|read| read.journal.as_ref()),
        &summary,
        time_text,
        &|id| display_name(state, id),
        lang,
    )
}

/// True while keep or revert of an existing operation must wait: an interrupted or busy entry
/// (the helper's gate for existing operations refuses them first), or an unreadable journal. A
/// `PendingReboot` seen from a new boot does not count: its own check decides it.
fn reverts_blocked(state: &AppState) -> bool {
    state.read.as_ref().is_none_or(|read| {
        read.summary.unreadable > 0
            || read.summary.items.iter().any(|item| match item.attention {
                Attention::Busy => true,
                Attention::Recover => item.op.state != OpState::PendingReboot,
                _ => false,
            })
    })
}

/// The entries of the start-up summary with what the recovery page needs to predict them.
fn attention_entries(state: &AppState) -> Vec<AttentionEntry<'_>> {
    let Some(read) = state.read.as_ref() else {
        return Vec::new();
    };
    let Some(journal) = read.journal.as_ref() else {
        return Vec::new();
    };
    let blocked = reverts_blocked(state);
    read.summary
        .items
        .iter()
        .filter_map(|item| {
            let entry = journal.entry(&item.op.op_id)?;
            Some(AttentionEntry {
                entry,
                attention: item.attention,
                current: read.snapshot.as_ref().map(|_| {
                    entry
                        .records
                        .iter()
                        .map(|record| current_of(state, record))
                        .collect()
                }),
                revertible: !blocked && history::revertible(journal, entry),
                decidable: !blocked,
            })
        })
        .collect()
}

/// The HID keyboards the recovery page's request may reset: the targets of the apply method's
/// default (`vm::change::default_apply_method`).
fn recovery_targets(state: &AppState) -> Vec<String> {
    let recovery = &state.journal_pages.recovery;
    match recovery.mode {
        RecoveryMode::Attention => attention_entries(state)
            .iter()
            .filter(|item| item.attention == Attention::Recover)
            .flat_map(|item| recovery::hid_targets(item.entry))
            .collect(),
        RecoveryMode::Undo => {
            let Some(journal) = state.read.as_ref().and_then(|read| read.journal.as_ref()) else {
                return Vec::new();
            };
            let current = |record: &ValueRecord| current_of(state, record);
            undo_preview(journal, &current)
                .undone
                .iter()
                .flat_map(|undo| recovery::hid_targets(undo.entry))
                .collect()
        }
        RecoveryMode::Revert => recovery
            .revert
            .as_ref()
            .and_then(|op| find_entry(state, op.as_str()))
            .map_or_else(Vec::new, recovery::hid_targets),
    }
}

/// Presets the recovery page's apply method as the page opens: "switch back now" when recent
/// input came from another keyboard or the pointer (design m3 B.5), else the safe "when replugged
/// or restarted". Without a time (`AppState::now`), nothing counts as recent.
fn preset_method(state: &mut AppState) {
    let targets = recovery_targets(state);
    let live = state.now.is_some_and(|now| {
        default_apply_method(&state.activity, &targets, now).method == ApplyMethod::Live
    });
    state.journal_pages.recovery.method = Some(if live {
        ApplyMethod::Live
    } else {
        ApplyMethod::Restart
    });
}

/// The recovery page in its current mode; `time_text` formats the time of the operation a revert
/// is about (local time in the GUI).
pub fn recovery_view(state: &AppState, time_text: &dyn Fn(Timestamp) -> String) -> RecoveryPage {
    let mut view = recovery_page_of(state, time_text);
    no_prompt_when_elevated(state, &mut view.uac_line);
    view
}

fn recovery_page_of(state: &AppState, time_text: &dyn Fn(Timestamp) -> String) -> RecoveryPage {
    let lang = lang(state);
    let name_of = |id: &str| display_name(state, id);
    let live = state.journal_pages.recovery.method == Some(ApplyMethod::Live);
    let read = state.read.as_ref();
    let journal = read.and_then(|read| read.journal.as_ref());
    match state.journal_pages.recovery.mode {
        RecoveryMode::Attention => {
            let entries = attention_entries(state);
            let context = read.and_then(|read| {
                Some(RecoveryContext {
                    current_boot: read.boot?,
                    inv_ps2: read.snapshot.as_ref().and_then(|snapshot| {
                        check_inv_ps2(&snapshot.global, &snapshot.keyboards).err()
                    }),
                })
            });
            recovery::recovery_page(&entries, context.as_ref(), &name_of, live, lang)
        }
        RecoveryMode::Undo => {
            let empty = mklm_core::Journal::default();
            let journal = journal.unwrap_or(&empty);
            let current = |record: &ValueRecord| current_of(state, record);
            let preview = undo_preview(journal, &current);
            recovery::undo_page(&preview, &name_of, live, lang)
        }
        RecoveryMode::Revert => {
            let entry = state
                .journal_pages
                .recovery
                .revert
                .as_ref()
                .and_then(|op| find_entry(state, op.as_str()));
            let revertible = entry.zip(journal).is_some_and(|(entry, journal)| {
                !reverts_blocked(state) && history::revertible(journal, entry)
            });
            let when = entry.map_or_else(String::new, |entry| time_text(entry.created_at));
            recovery::revert_page(entry, revertible, &when, &name_of, live, lang)
        }
    }
}

// --- Transitions ---------------------------------------------------------------------------

/// Resets what a page keeps when it is opened through the navigation, a banner or the result's
/// next step.
pub fn entered(state: &mut AppState, page: Page) {
    if page != Page::Conflict {
        // A stopped restore is asked about right after it (the result's next step) only.
        state.journal_pages.conflict.restore = None;
    }
    match page {
        Page::Recovery => {
            state.journal_pages.recovery = RecoveryState::default();
            preset_method(state);
        }
        Page::Restart => state.journal_pages.restart.error = None,
        Page::Journal => state.journal_pages.folder_failed = false,
        Page::PostReboot => state.key_test = KeyTest::default(),
        _ => {}
    }
}

fn open_recovery(state: &mut AppState, mode: RecoveryMode, revert: Option<OpId>) -> Vec<Effect> {
    let back = state.page;
    state.journal_pages.recovery = RecoveryState {
        mode,
        revert,
        back: Some(back),
        method: None,
    };
    preset_method(state);
    state.page = Page::Recovery;
    vec![Effect::Read, Effect::Render]
}

fn open_post_reboot(state: &mut AppState, op_id: Option<OpId>) -> Vec<Effect> {
    let post = &mut state.journal_pages.post_reboot;
    if post.op_id != op_id {
        post.typed.clear();
    }
    post.op_id = op_id;
    state.page = Page::PostReboot;
    state.draft = None;
    state.key_test = KeyTest::default();
    vec![Effect::Render]
}

/// True while a button of these pages may start something: no session, no overlay.
fn idle(state: &AppState) -> bool {
    state.session == SessionPhase::Idle && state.overlay == OverlayKind::None
}

/// Handles a message of these pages.
pub fn handle(state: &mut AppState, msg: JournalMsg) -> Vec<Effect> {
    match msg {
        JournalMsg::RestartNow { acknowledged } => {
            // Only with the acknowledgement, while nothing else runs, and while the journal
            // (flushed by the helper, plan 2.3) still has a reason to restart; the I/O worker
            // reads it once more right before the restart.
            if !acknowledged
                || !idle(state)
                || state.page != Page::Restart
                || state.journal_pages.restart.restarting
                || !restart_view(state).ready
            {
                return Vec::new();
            }
            state.journal_pages.restart = RestartState {
                restarting: true,
                error: None,
            };
            vec![Effect::Journal(JournalEffect::RestartPc), Effect::Render]
        }
        JournalMsg::RestartLater => {
            if state.journal_pages.restart.restarting {
                return Vec::new();
            }
            update(state, AppMsg::Navigate(Page::Main))
        }
        JournalMsg::RestartFailed(error) => {
            state.journal_pages.restart = RestartState {
                restarting: false,
                error: Some(error),
            };
            vec![Effect::Render]
        }
        JournalMsg::RestartNotNeeded => {
            state.journal_pages.restart = RestartState::default();
            vec![Effect::Read, Effect::Render]
        }
        JournalMsg::OpenUndo => {
            let mut effects = Vec::new();
            if !state.visible {
                state.visible = true;
                effects.push(Effect::ShowWindow);
            }
            // Not over a running session or its result, and not away from a change being made:
            // the window only comes to the front.
            if !idle(state)
                || matches!(state.page, Page::Change | Page::UacNotice | Page::Wizard)
                || state.journal_pages.restart.restarting
            {
                effects.push(Effect::Render);
                return effects;
            }
            effects.extend(open_recovery(state, RecoveryMode::Undo, None));
            effects
        }
        JournalMsg::PostRebootKeep => {
            let mut effects: Vec<Effect> = stop_on_top(state).into_iter().collect();
            let view = post_reboot_view(state);
            if view.can_keep
                && let Ok(op_id) = OpId::parse(&view.op_id)
            {
                let entries = [op_id.clone()];
                effects.extend(start(
                    state,
                    Request::Confirm { op_id },
                    ApplyOptions::default(),
                    &entries,
                ));
            }
            effects
        }
        JournalMsg::PostRebootRevert => {
            let mut effects: Vec<Effect> = stop_on_top(state).into_iter().collect();
            let view = post_reboot_view(state);
            if let Ok(op_id) = OpId::parse(&view.op_id) {
                // No reset in place: this page does not offer the choice (design m3 B.9); a HID
                // keyboard then shows "保存済み（反映待ち）" and "今すぐ反映…".
                let apply = ApplyOptions::default();
                let entries = [op_id.clone()];
                effects.extend(start(
                    state,
                    Request::Revert { op_id, apply },
                    apply,
                    &entries,
                ));
            }
            effects
        }
        JournalMsg::PostRebootLater => {
            // Nothing changes; the check comes back after the next sign-in (design m2 D.7 step
            // 4), so the RunOnce value is registered again.
            let mut effects: Vec<Effect> = stop_on_top(state).into_iter().collect();
            if post_reboot_entry(state).is_some() {
                effects.push(Effect::RunOnceRule);
            }
            effects.extend(update(state, AppMsg::Navigate(Page::Main)));
            effects
        }
        JournalMsg::OnTopExpired => stop_on_top(state).into_iter().collect(),
        JournalMsg::ConflictChoice { keyboard, option } => {
            track_conflict(state);
            let draft = &mut state.journal_pages.conflict;
            draft.selections.retain(|(known, _)| *known != keyboard);
            draft.selections.push((keyboard, option));
            draft.inv_ps2 = None;
            vec![Effect::Render]
        }
        JournalMsg::ConflictValueChoice { record, option } => {
            track_conflict(state);
            let Some(choice) = VALUE_CHOICES.get(option).copied() else {
                return Vec::new();
            };
            let draft = &mut state.journal_pages.conflict;
            draft.overrides.retain(|(known, _)| *known != record);
            draft.overrides.push((record, choice));
            draft.inv_ps2 = None;
            vec![Effect::Render]
        }
        JournalMsg::ConflictRestoreChoice(index) => {
            let (Some(restore), Some(policy)) = (
                state.journal_pages.conflict.restore.as_mut(),
                RESTORE_POLICIES.get(index),
            ) else {
                return Vec::new();
            };
            restore.policy = Some(*policy);
            vec![Effect::Render]
        }
        JournalMsg::ConflictResolve => {
            if !idle(state) || state.page != Page::Conflict {
                return Vec::new();
            }
            let view = conflict_view(state);
            if !view.can_resolve() {
                return Vec::new();
            }
            if let Some(restore) = state.journal_pages.conflict.restore.clone() {
                let Some(policy) = RESTORE_POLICIES.get(view.restore_selected) else {
                    return Vec::new();
                };
                let request = Request::RestoreBaseline(RestoreBaselineRequest {
                    scope: restore.scope,
                    on_conflict: *policy,
                    apply: restore.apply,
                });
                // The operations that last wrote the values in question name the keyboards.
                let entries: Vec<OpId> = restore
                    .conflicts
                    .iter()
                    .map(|conflict| conflict.op_id.clone())
                    .collect();
                return start(state, request, restore.apply, &entries);
            }
            let Ok(op_id) = OpId::parse(&view.op_id) else {
                return Vec::new();
            };
            let choices = conflict::choices(&view.keyboards);
            // No reset in place (the page does not offer it): a HID keyboard that goes back
            // shows "保存済み（反映待ち）" and "今すぐ反映…" (design m2 D.8, D.4 step 9).
            let apply = ApplyOptions::default();
            let entries = [op_id.clone()];
            let effects = start(
                state,
                Request::ResolveConflict(ResolveConflictRequest {
                    op_id: op_id.clone(),
                    choices,
                    apply,
                }),
                apply,
                &entries,
            );
            if !effects.is_empty() {
                state.journal_pages.conflict.resolving = Some(op_id);
            }
            effects
        }
        JournalMsg::JournalRevert { op_id } => match OpId::parse(&op_id) {
            Ok(op_id) if idle(state) => open_recovery(state, RecoveryMode::Revert, Some(op_id)),
            _ => Vec::new(),
        },
        JournalMsg::OpenRecoveryFolder => {
            state.journal_pages.folder_failed = false;
            vec![
                Effect::Journal(JournalEffect::OpenRecoveryFolder),
                Effect::Render,
            ]
        }
        JournalMsg::RecoveryFolderFailed => {
            state.journal_pages.folder_failed = true;
            vec![Effect::Render]
        }
        JournalMsg::RecoveryChooseMethod(index) => {
            let Some(method) = ApplyMethod::from_index(index) else {
                return Vec::new();
            };
            if state.page != Page::Recovery || state.journal_pages.recovery.method == Some(method) {
                return Vec::new();
            }
            state.journal_pages.recovery.method = Some(method);
            vec![Effect::Render]
        }
        JournalMsg::RecoveryPrimary => {
            if !idle(state) || state.page != Page::Recovery {
                return Vec::new();
            }
            let lang = lang(state);
            let view = recovery_view(state, &|at| history::utc_time_text(at, lang));
            if !view.can_recover {
                return Vec::new();
            }
            // Only a reset in place when the page offered the choice and it is chosen.
            let apply = if view.method.visible && view.method.selected == ApplyMethod::Live.index()
            {
                ApplyMethod::Live.options()
            } else {
                ApplyOptions::default()
            };
            let request = match view.mode {
                RecoveryMode::Attention => Request::Recover { apply },
                RecoveryMode::Undo => Request::Undo { apply },
                RecoveryMode::Revert => {
                    let Some(op_id) = state.journal_pages.recovery.revert.clone() else {
                        return Vec::new();
                    };
                    Request::Revert { op_id, apply }
                }
            };
            // The entries the page lists are the ones the request acts on.
            let entries: Vec<OpId> = view
                .items
                .iter()
                .filter_map(|item| OpId::parse(&item.op_id).ok())
                .collect();
            start(state, request, apply, &entries)
        }
        JournalMsg::RecoveryKeep { op_id } => {
            // The helper would refuse it behind an interrupted or busy entry: no prompt for it.
            if !idle(state) || reverts_blocked(state) {
                return Vec::new();
            }
            let Some(entry) = find_entry(state, &op_id) else {
                return Vec::new();
            };
            if entry.state != OpState::AwaitingConfirm || entry.countdown.is_some() {
                return Vec::new();
            }
            let op = entry.op_id.clone();
            if takes_effect_at_restart(entry) {
                // Kept only after the check a restart needs (design m2 D.6, review C2).
                open_post_reboot(state, Some(op))
            } else {
                let entries = [op.clone()];
                start(
                    state,
                    Request::Confirm { op_id: op },
                    ApplyOptions::default(),
                    &entries,
                )
            }
        }
        JournalMsg::RecoveryRevert { op_id } => match OpId::parse(&op_id) {
            Ok(op_id) if idle(state) => open_recovery(state, RecoveryMode::Revert, Some(op_id)),
            _ => Vec::new(),
        },
        JournalMsg::RecoveryDismiss => {
            let recovery = &state.journal_pages.recovery;
            let back = match recovery.mode {
                RecoveryMode::Attention => Page::Main,
                RecoveryMode::Undo => recovery.back.unwrap_or(Page::Main),
                RecoveryMode::Revert => recovery.back.unwrap_or(Page::Journal),
            };
            // Back on the recovery page, `entered` shows what the journal needs again.
            update(state, AppMsg::Navigate(back))
        }
        JournalMsg::RunOnceDone(result) => {
            state.journal_pages.run_once_problem = match result {
                Ok(RunOnceOutcome::TellUser) => Some(true),
                Ok(RunOnceOutcome::NotNeeded | RunOnceOutcome::Registered(_)) => None,
                Err(_) => Some(false),
            };
            vec![Effect::Render]
        }
    }
}

/// The conflict choices belong to the entry shown now; another entry starts afresh.
fn track_conflict(state: &mut AppState) {
    let current = conflict_entry(state).map(|entry| entry.op_id.clone());
    let draft = &mut state.journal_pages.conflict;
    if draft.op_id != current {
        *draft = ConflictState {
            op_id: current,
            restore: draft.restore.take(),
            ..ConflictState::default()
        };
    }
}

/// After a session (before its result is shown): a restore to before MKLM that stopped at values
/// changed outside MKLM is kept for the conflict page, and a resolution the helper refused for
/// INV-PS2 is explained on that page (design m2 D.8, m3 B.10).
pub fn session_ended(state: &mut AppState, report: &RequestReport) {
    state.journal_pages.conflict.restore = match (&state.request, &report.first) {
        (
            Some(Request::RestoreBaseline(request)),
            RequestEnd::Ended(SessionEnd::Finished(result)),
        ) if request.on_conflict == ConflictPolicy::Report
            && result.op_id.is_none()
            && !result.conflicts.is_empty() =>
        {
            Some(RestoreConflicts {
                scope: request.scope.clone(),
                apply: request.apply,
                conflicts: result.conflicts.clone(),
                policy: None,
            })
        }
        _ => None,
    };
    let Some(op_id) = state.journal_pages.conflict.resolving.take() else {
        return;
    };
    if let RequestEnd::Ended(SessionEnd::Failed(info)) = &report.first
        && info.code == ErrorCode::PlanRejected
        && let Some(PlanError::InvPs2(violation)) = &info.plan_error
    {
        let draft = &mut state.journal_pages.conflict;
        if draft.op_id.as_ref() != Some(&op_id) {
            *draft = ConflictState {
                op_id: Some(op_id),
                ..ConflictState::default()
            };
        }
        draft.inv_ps2 = Some(violation.keyboards.clone());
    }
}

/// The result overlay was closed without its next step: a stopped restore is not asked about
/// later (the values may change meanwhile; the user starts the restore again).
pub fn result_closed(state: &mut AppState) {
    state.journal_pages.conflict.restore = None;
}

/// Pages that open by themselves may replace these (never a change, a UAC explanation or another
/// journal page, and never while a session or an overlay is up).
fn may_open_by_itself(state: &AppState) -> bool {
    idle(state)
        && matches!(
            state.page,
            Page::Main
                | Page::Journal
                | Page::ImeHelp
                | Page::Settings
                | Page::About
                | Page::Wizard
        )
}

fn show(state: &mut AppState, effects: &mut Vec<Effect>) {
    state.visible = true;
    effects.push(Effect::ShowWindow);
}

/// After every `SystemRead` (design m3 B.18): forgets choices of entries that are gone, opens the
/// post-reboot check (in front, on top) or the recovery page once per entry and boot, and shows a
/// hidden window when the first read after start finds something the user must decide (design m3
/// F.2: `--tray` with a check, a recovery or a conflict).
pub fn after_read(state: &mut AppState) -> Vec<Effect> {
    let first = !state.journal_pages.startup_checked;
    state.journal_pages.startup_checked = true;
    let mut effects = Vec::new();
    let Some((boot, summary)) = state
        .read
        .as_ref()
        .map(|read| (read.boot, read.summary.clone()))
    else {
        return effects;
    };
    // Choices of an entry that is no longer in conflict, or of a check that was decided.
    let conflict = conflict_entry(state).map(|entry| entry.op_id.clone());
    let pages = &mut state.journal_pages;
    if pages.conflict.op_id.is_some() && pages.conflict.op_id != conflict {
        pages.conflict = ConflictState {
            restore: pages.conflict.restore.take(),
            ..ConflictState::default()
        };
    }
    let post_entry = post_reboot_entry(state).map(|entry| entry.op_id.clone());
    let post = &mut state.journal_pages.post_reboot;
    if post.op_id.is_some() && post.op_id != post_entry {
        post.op_id = None;
        post.typed.clear();
    }
    if !may_open_by_itself(state) {
        return effects;
    }
    let requested = first && state.journal_pages.post_reboot.requested;
    if let Some(op_id) = post_entry.filter(|op_id| {
        (summary.post_reboot_due() || requested)
            && !state.journal_pages.post_reboot.shown.contains(op_id)
    }) {
        state.journal_pages.post_reboot.shown.push(op_id.clone());
        effects.extend(open_post_reboot(state, Some(op_id)));
        show(state, &mut effects);
        state.journal_pages.post_reboot.on_top = true;
        effects.push(Effect::Journal(JournalEffect::AlwaysOnTop(true)));
        // Shut down instead of restarted: ask again after the next sign-in (design m2 D.7
        // step 2, m3 B.9).
        if post_reboot_view(state).not_restarted {
            effects.push(Effect::RunOnceRule);
        }
        return effects;
    }
    // The recovery page, once per entry and boot (design m2 D.7, m3 B.12: the prompt is
    // remembered in `settings.recovery.prompted`). A `PendingReboot` seen from a new boot is the
    // post-reboot check's.
    if let Some(boot) = boot {
        let boot_text = boot.to_text();
        let unprompted: Vec<String> = summary
            .with(Attention::Recover)
            .filter(|item| item.op.state != OpState::PendingReboot)
            .map(|item| item.op.op_id.to_string())
            .filter(|op| {
                !state
                    .settings
                    .recovery
                    .prompted
                    .iter()
                    .any(|prompted| prompted.op == *op && prompted.boot == boot_text)
            })
            .collect();
        if !unprompted.is_empty() {
            let prompted = &mut state.settings.recovery.prompted;
            // Only this boot's records matter; older ones are dropped.
            prompted.retain(|entry| entry.boot == boot_text);
            prompted.extend(unprompted.into_iter().map(|op| PromptedEntry {
                op,
                boot: boot_text.clone(),
            }));
            effects.push(Effect::SaveSettings(Box::new(state.settings.clone())));
            state.page = Page::Recovery;
            state.journal_pages.recovery = RecoveryState::default();
            preset_method(state);
            show(state, &mut effects);
            effects.push(Effect::Render);
            return effects;
        }
    }
    if first && !state.visible && (conflict.is_some() || summary.needs_recovery()) {
        show(state, &mut effects);
        effects.push(Effect::Render);
    }
    effects
}

#[cfg(test)]
mod tests;
