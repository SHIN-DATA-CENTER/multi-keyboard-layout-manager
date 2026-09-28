//! The result of a request (design m3 B.17): title and tone from `OutcomeClass`, what the
//! keyboards do **now** (from the read that follows every session, review U7), the reason
//! (`i18n::failure` with the reset phase, design m3 G.2), the next step, and the English
//! diagnostics under "技術的な詳細" with a "詳細をコピー" button.
//!
//! "変更は元に戻されています" is said only when the journal's answer says so (`Reverted`,
//! `Failed` with a reason), never guessed from an error code: a `Registry` error may leave a
//! conflict behind (review U7).

use mklm_client::describe::ResetPhase;
use mklm_client::gate::BlockReason;
use mklm_client::orchestrator::{LaunchError, RequestEnd, RequestReport};
use mklm_client::outcome::{classify_error, classify_lost_recovery, classify_result};
use mklm_client::run_once::{RunOnceError, RunOnceOutcome};
use mklm_client::session::{SessionEnd, SessionView};
use mklm_client::{HelperExit, OutcomeClass};
use mklm_core::{ErrorCode, LayoutChoice, OpId, OperationResult, Outcome, PendingAction, assess};
use mklm_ipc::Request;

use super::{SnapshotText, Tone};
use crate::i18n::{self, Lang, RunOnceNote};
use crate::state::{SessionTarget, SystemRead};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResultView {
    pub title: String,
    /// "今の状態": one line per keyboard of the request ("Keychron Receiver: US として動作中
    /// （変更前のまま）"), from the `SystemRead` after the session; "確かめています…" until it
    /// arrives.
    pub current_state: Vec<String>,
    /// What happened, one part per line (design m3 B.12 "行ごとに出す"): the outcome, each
    /// recovered operation, what is still pending and what to do about it.
    pub message: String,
    pub tone: Tone,
    /// US was kept: "US 配列には『半角/全角』キーがありません…［入力方式の案内］" (plan 3.4,
    /// review U19).
    pub ime_note: String,
    /// The RunOnce rule failed or cannot be applied (elevated GUI): the user runs the check.
    pub run_once_note: String,
    /// The button of the next step ("再起動の画面へ", "衝突を解決…"); empty when none.
    pub next_step: String,
    pub next: Option<NextStep>,
    pub details: String,
    /// The text "詳細をコピー" puts on the clipboard: English diagnostics, the short operation
    /// ID, the build ID (`mklm_win::ui::copy_text_to_clipboard`). Never a key the user typed.
    pub copy_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    Restart,
    /// The check after the restart (design m3 B.9).
    PostReboot,
    Conflict,
    Recovery,
    ImeHelp,
}

/// What the result is about besides the report (design m3 B.17).
#[derive(Debug, Clone, Copy, Default)]
pub struct ResultContext<'a> {
    /// The request that ran (the IME note after a kept US assignment).
    pub request: Option<&'a Request>,
    /// The keyboards of the request, with how they typed before it ("今の状態").
    pub targets: &'a [SessionTarget],
    /// The request's last session view (the operation ID, whether the reset was reached).
    pub view: Option<&'a SessionView>,
    /// The request was "今すぐ反映…" (`AppState::applying_now`): a `Recover` that only put
    /// saved values into effect is "反映しました", not "回復しました" (design m3 B.12).
    pub apply_now: bool,
}

/// The tone of a class.
fn class_tone(class: OutcomeClass) -> Tone {
    match class {
        OutcomeClass::Done => Tone::Success,
        OutcomeClass::Cancelled => Tone::Neutral,
        OutcomeClass::RevertedAutomatically
        | OutcomeClass::Blocked
        | OutcomeClass::AwaitingConfirm
        | OutcomeClass::RestartRequired => Tone::Warning,
        OutcomeClass::Failed | OutcomeClass::Conflict => Tone::Danger,
    }
}

/// The next step a class leads to.
fn class_next(class: OutcomeClass) -> Option<NextStep> {
    match class {
        OutcomeClass::RestartRequired => Some(NextStep::Restart),
        OutcomeClass::Conflict => Some(NextStep::Conflict),
        _ => None,
    }
}

/// The "今の状態" lines: one per keyboard of the request, from the read after the session.
fn current_state(targets: &[SessionTarget], after: Option<&SystemRead>, lang: Lang) -> Vec<String> {
    if targets.is_empty() {
        return Vec::new();
    }
    let Some(snapshot) = after.and_then(|read| read.snapshot.as_ref()) else {
        return vec![i18n::checking_now(lang)];
    };
    let assessment = assess(snapshot);
    targets
        .iter()
        .map(|target| {
            let member = |present: bool| {
                assessment.keyboards.iter().find(|ka| {
                    (ka.present || !present)
                        && target
                            .members
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(&ka.instance_id))
                })
            };
            let member = member(true).or_else(|| member(false));
            let present = member.is_some_and(|ka| ka.present);
            let now = member
                .and_then(|ka| ka.current.as_ref())
                .map(|layout| &layout.table);
            let unchanged = now.is_some() && now == target.before.as_ref();
            i18n::current_state_line(&target.name, now, present, unchanged, lang)
        })
        .collect()
}

/// Whether the reset was reached in the session (the wording of design m3 G.2).
fn reset_phase(view: Option<&SessionView>) -> ResetPhase {
    match view {
        Some(view)
            if view.countdown_seen || !view.arrivals.is_empty() || view.resetting.is_some() =>
        {
            ResetPhase::Reached
        }
        _ => ResetPhase::NotReached,
    }
}

/// The request kept a US assignment: the IME note and the guide button (review U19).
fn kept_us(request: Option<&Request>, result: &OperationResult) -> bool {
    let us = match request {
        Some(Request::SetLayout(set)) => set.layout == LayoutChoice::Us,
        Some(Request::Migrate(migrate)) => migrate
            .assignments
            .iter()
            .any(|assignment| assignment.layout == LayoutChoice::Us),
        _ => false,
    };
    us && result.outcome == Outcome::Confirmed
}

/// The operation's words for a recovered entry (from the journal of the read, if it is there).
fn operation_text(after: Option<&SystemRead>, op: &OpId, lang: Lang) -> Option<String> {
    let read = after?;
    let entry = read
        .journal
        .as_ref()?
        .entries
        .iter()
        .find(|entry| &entry.op_id == op)?;
    let name_of = |id: &str| {
        read.snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .keyboards
                    .iter()
                    .find(|kb| kb.instance_id.eq_ignore_ascii_case(id))
            })
            .map_or_else(|| id.to_string(), |kb| kb.display_name.clone())
    };
    Some(super::journal::kind_text(&entry.kind, &name_of, lang))
}

/// Builds the message of a finished request.
struct Parts {
    class: OutcomeClass,
    message: Vec<String>,
    details: Vec<String>,
    next: Option<NextStep>,
    op: Option<OpId>,
}

fn finished(
    result: &OperationResult,
    class: OutcomeClass,
    context: &ResultContext<'_>,
    after: Option<&SystemRead>,
    lang: Lang,
) -> Parts {
    let mut parts = Parts {
        class,
        message: Vec::new(),
        details: Vec::new(),
        next: class_next(class),
        op: result.op_id.clone(),
    };
    // A machine-wide setting (design m3 B.14) changes no layout: say what is saved.
    let machine_setting = match context.request {
        Some(Request::SetMachineSettings {
            restore_on_uninstall,
        }) => Some(*restore_on_uninstall),
        _ => None,
    };
    match (&result.failure, result.outcome, machine_setting) {
        (Some(failure), _, _) => {
            parts
                .message
                .push(i18n::failure(failure, reset_phase(context.view), lang))
        }
        (None, Outcome::Confirmed, Some(on)) => {
            parts.message.push(i18n::uninstall_restore_saved(on, lang));
        }
        // The recovered rows say what happened.
        (None, Outcome::Recovered, _) if !result.recovered.is_empty() => {}
        (None, Outcome::Recovered, _) if context.apply_now => {
            parts.message.push(i18n::result_applied_now(lang));
        }
        (None, outcome, _) => parts.message.push(i18n::result_outcome(outcome, lang)),
    }
    for recovered in &result.recovered {
        let what = operation_text(after, &recovered.op_id, lang);
        parts.message.push(i18n::recovered_line(
            what.as_deref(),
            recovered.from,
            recovered.to,
            lang,
        ));
    }
    if let Some(action) = result.pending_action
        && result.outcome != Outcome::PendingReboot
        && result.outcome != Outcome::RevertedPendingReboot
    {
        parts.message.push(i18n::pending(action, lang));
        parts.message.push(i18n::pending_hint(action, lang));
        if action == PendingAction::RestartPc && parts.next.is_none() {
            parts.next = Some(NextStep::Restart);
        }
    }
    if result.inv_ps2_violation.is_some() {
        parts.message.push(i18n::result_inv_ps2(lang));
        parts.next = Some(NextStep::Conflict);
    }
    if !result.conflicts.is_empty() && result.outcome != Outcome::Conflict {
        parts
            .message
            .push(i18n::result_outcome(Outcome::Conflict, lang));
        parts.next = Some(NextStep::Conflict);
    }
    if matches!(result.outcome, Outcome::Recovered)
        && after.is_some_and(|read| {
            read.summary
                .with(mklm_core::Attention::AwaitingUser)
                .next()
                .is_some()
        })
    {
        parts.message.push(i18n::result_still_waiting(lang));
    }
    if kept_us(context.request, result) {
        parts.next = Some(NextStep::ImeHelp);
    }
    parts.details.extend(result.warnings.iter().cloned());
    parts
}

/// The result of a request that ran (or did not launch). `after` is the read that followed the
/// session, `None` until it arrives ("確かめています…").
pub fn result_view(
    report: &RequestReport,
    run_once: &Result<RunOnceOutcome, RunOnceError>,
    context: &ResultContext<'_>,
    after: Option<&SystemRead>,
    lang: Lang,
) -> ResultView {
    let mut parts = match &report.first {
        RequestEnd::NotLaunched(error) => not_launched(error, lang),
        RequestEnd::Ended(SessionEnd::Finished(result)) => {
            finished(result, classify_result(result), context, after, lang)
        }
        RequestEnd::Ended(SessionEnd::Failed(info)) => {
            let class = classify_error(info.code);
            let mut details = vec![format!("{:?}: {}", info.code, info.message)];
            if let Some(plan_error) = &info.plan_error {
                details.push(plan_error.to_string());
            }
            Parts {
                class,
                message: vec![i18n::error_code(info.code, lang)],
                details,
                next: match info.code {
                    ErrorCode::RecoveryNeeded | ErrorCode::OpInProgress => Some(NextStep::Recovery),
                    _ => None,
                },
                op: info.op_id.clone(),
            }
        }
        RequestEnd::Ended(SessionEnd::Lost {
            exit_code, detail, ..
        }) => lost(report, *exit_code, detail, context, after, lang),
        RequestEnd::Ended(SessionEnd::Unresponsive) => Parts {
            class: OutcomeClass::Failed,
            message: vec![i18n::unresponsive(lang)],
            details: vec![
                "the helper stopped answering (no frame within the silence limit)".into(),
            ],
            next: None,
            op: None,
        },
        // Not shown (the state closes the overlay); a neutral text in case it is.
        RequestEnd::Ended(SessionEnd::Abandoned { .. }) => Parts {
            class: OutcomeClass::Cancelled,
            message: Vec::new(),
            details: vec!["the session was left".into()],
            next: None,
            op: None,
        },
    };
    let op = parts
        .op
        .clone()
        .or_else(|| context.view.and_then(|view| view.op_id.clone()));
    if let Some(view) = context.view {
        parts.details.extend(view.warnings.iter().cloned());
    }
    let launched = !matches!(report.first, RequestEnd::NotLaunched(_));
    let run_once_note = match run_once {
        Ok(RunOnceOutcome::TellUser) => Some(RunOnceNote::Elevated),
        Ok(_) => None,
        Err(RunOnceError::Register(error)) => {
            parts.details.push(format!("RunOnce: {error}"));
            Some(RunOnceNote::NotRegistered)
        }
        Err(RunOnceError::Check(error)) => {
            parts.details.push(format!("RunOnce: {error}"));
            launched.then_some(RunOnceNote::Unchecked)
        }
    };
    let ime = parts.next == Some(NextStep::ImeHelp);
    let details = parts.details.join("\n");
    let mut copy = vec![format!(
        "MKLM {} (build {})",
        env!("CARGO_PKG_VERSION"),
        crate::BUILD_ID
    )];
    copy.push(format!("result: {:?}", parts.class));
    if let Some(op) = &op {
        copy.push(format!("operation: {}", op.short()));
    }
    if !details.is_empty() {
        copy.push(details.clone());
    }
    ResultView {
        title: i18n::outcome_title(parts.class, lang),
        current_state: current_state(context.targets, after, lang),
        // One part per line: the parts are sentences and short status lines ("保存済み（反映待ち:
        // …）") that would run together on one line.
        message: parts.message.join("\n"),
        tone: class_tone(parts.class),
        ime_note: if ime {
            i18n::ime_note_us(lang)
        } else {
            String::new()
        },
        run_once_note: run_once_note
            .map(|note| i18n::run_once_note(note, lang))
            .unwrap_or_default(),
        next_step: parts
            .next
            .map(|next| i18n::next_step(next, lang))
            .unwrap_or_default(),
        next: parts.next,
        details,
        copy_text: copy.join("\n"),
    }
}

fn not_launched(error: &LaunchError, lang: Lang) -> Parts {
    let (class, details) = match error {
        LaunchError::Declined => (OutcomeClass::Cancelled, Vec::new()),
        LaunchError::Failed { kind, message } => {
            (OutcomeClass::Failed, vec![format!("{kind:?}: {message}")])
        }
    };
    Parts {
        class,
        message: vec![i18n::launch_error(error, lang)],
        details,
        next: None,
        op: None,
    }
}

/// The helper was lost (design m2 E.7, m3 A.2.3): the recovery's result, or why it did not run.
fn lost(
    report: &RequestReport,
    exit_code: Option<u32>,
    detail: &str,
    context: &ResultContext<'_>,
    after: Option<&SystemRead>,
    lang: Lang,
) -> Parts {
    let mut details = vec![match exit_code {
        Some(code) => format!("the helper was lost ({detail}; exit code {code})"),
        None => format!("the helper was lost ({detail})"),
    }];
    // Not recovered: failed, as the CLI says it (its exit code 1), with the way out.
    let waiting = || Parts {
        class: OutcomeClass::Failed,
        message: vec![i18n::result_recovery_waits(lang)],
        details: Vec::new(),
        next: Some(NextStep::Recovery),
        op: None,
    };
    let mut parts = match (&report.recovery, &report.lost_needs_recovery) {
        (Some(RequestEnd::Ended(SessionEnd::Finished(result))), _) => {
            let mut parts = finished(result, classify_lost_recovery(result), context, after, lang);
            parts
                .message
                .insert(0, i18n::result_recovered_after_loss(lang));
            parts
        }
        (Some(RequestEnd::NotLaunched(error)), _) => {
            let mut parts = waiting();
            if let LaunchError::Failed { .. } = error {
                parts.message.insert(0, i18n::launch_error(error, lang));
            }
            details.push(format!("recovery not launched: {error}"));
            parts
        }
        (Some(RequestEnd::Ended(SessionEnd::Failed(info))), _) => {
            details.push(format!("recovery: {:?}: {}", info.code, info.message));
            let mut parts = waiting();
            parts.message.insert(0, i18n::error_code(info.code, lang));
            parts
        }
        (Some(RequestEnd::Ended(other)), _) => {
            details.push(format!("recovery ended: {other:?}"));
            waiting()
        }
        (None, _) if report.recovery_skipped.is_some() => waiting(),
        (None, Some(Err(error))) => {
            details.push(error.clone());
            Parts {
                class: OutcomeClass::Failed,
                message: vec![i18n::result_lost_unknown(lang)],
                details: Vec::new(),
                next: None,
                op: None,
            }
        }
        // Nothing to recover: the helper stopped before it journaled, or its entry was closed.
        (None, _) => Parts {
            class: OutcomeClass::Failed,
            message: vec![i18n::helper_exit(HelperExit(exit_code.unwrap_or(1)), lang)],
            details: Vec::new(),
            next: None,
            op: None,
        },
    };
    details.append(&mut parts.details);
    parts.details = details;
    parts
}

/// The result when the preparation found the way blocked (design m3 B.5): nothing was
/// launched, no prompt was shown. `name_of` names a keyboard by instance ID.
pub fn blocked_view(
    reason: &BlockReason,
    name_of: &dyn Fn(&str) -> String,
    lang: Lang,
) -> ResultView {
    let what = reason
        .op()
        .map(|op| super::journal::kind_text(&op.kind, name_of, lang))
        .unwrap_or_default();
    let next = match reason {
        BlockReason::JournalUnreadable { .. } | BlockReason::Busy(_) => None,
        BlockReason::PostRebootCheck(_) => Some(NextStep::PostReboot),
        BlockReason::NeedsRecovery(_) | BlockReason::AwaitingUser(_) => Some(NextStep::Recovery),
        BlockReason::WaitingForReboot(_) => Some(NextStep::Restart),
        BlockReason::Conflict(_) => Some(NextStep::Conflict),
    };
    let class = reason.class();
    let details = match reason {
        BlockReason::JournalUnreadable { count, first } => {
            format!(
                "{count} journal entries cannot be read; first: {}: {}",
                first.name, first.error
            )
        }
        other => other
            .op()
            .map(|op| format!("blocked by {} ({:?})", op.op_id.short(), op.state))
            .unwrap_or_default(),
    };
    ResultView {
        title: i18n::outcome_title(class, lang),
        message: i18n::block_reason(reason, &what, lang),
        tone: class_tone(class),
        next_step: next
            .map(|next| i18n::next_step(next, lang))
            .unwrap_or_default(),
        next,
        copy_text: format!(
            "MKLM {} (build {})\nresult: {class:?}\n{details}",
            env!("CARGO_PKG_VERSION"),
            crate::BUILD_ID
        ),
        details,
        ..ResultView::default()
    }
}

/// The result the overlay shows now: why the preparation stopped, or the last session's end.
pub fn shown_result(state: &crate::state::AppState, lang: Lang) -> Option<ResultView> {
    let name_of = |id: &str| crate::state::display_name(state, id);
    if let Some(reason) = &state.blocked {
        return Some(blocked_view(reason, &name_of, lang));
    }
    let outcome = state.outcome.as_ref()?;
    let context = ResultContext {
        request: state.request.as_ref(),
        targets: &state.targets,
        view: state.last_view.as_deref(),
        apply_now: state.applying_now,
    };
    let after = if state.result_read_pending {
        None
    } else {
        state.read.as_ref()
    };
    let mut view = result_view(&outcome.report, &outcome.run_once, &context, after, lang);
    // Over the wizard, the input method guide would end it with keyboards still to set: the note
    // stays, and the wizard's last step repeats it (design m3 B.1, B.13).
    if state.page == crate::state::Page::Wizard && view.next == Some(NextStep::ImeHelp) {
        view.next = None;
        view.next_step.clear();
    }
    Some(view)
}

impl SnapshotText for ResultView {
    fn snapshot_text(&self) -> String {
        let mut out = format!("title: {}\ntone: {:?}\n", self.title, self.tone);
        for line in &self.current_state {
            out.push_str(&format!("now: {line}\n"));
        }
        for line in self.message.lines() {
            out.push_str(&format!("message: {line}\n"));
        }
        for (label, text) in [
            ("ime", &self.ime_note),
            ("run once", &self.run_once_note),
            ("next", &self.next_step),
        ] {
            if !text.is_empty() {
                out.push_str(&format!("{label}: {text}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::gate::OpRef;
    use mklm_client::orchestrator::{LaunchFailure, RecoverySkip};
    use mklm_client::startup::StartupSummary;
    use mklm_core::{
        ErrorInfo, FailureReason, KeyboardType, LayoutTable, OpKind, OpState, RecoveredOp, fixtures,
    };
    use mklm_ipc::SetLayoutRequest;

    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";

    fn op() -> OpId {
        OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap()
    }

    fn report(first: RequestEnd) -> RequestReport {
        RequestReport {
            first,
            lost_needs_recovery: None,
            recovery: None,
            recovery_skipped: None,
        }
    }

    fn result(outcome: Outcome, failure: Option<FailureReason>) -> OperationResult {
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

    fn finished_report(result: OperationResult) -> RequestReport {
        report(RequestEnd::Ended(SessionEnd::Finished(result)))
    }

    fn targets(before: LayoutTable) -> Vec<SessionTarget> {
        vec![SessionTarget {
            name: "Keychron Receiver".into(),
            members: vec![KEYCHRON.into()],
            before: Some(before),
        }]
    }

    /// The read after the session, with the Keychron typing `reported`.
    fn read_after(reported: KeyboardType) -> SystemRead {
        let mut snapshot = fixtures::dev_machine();
        snapshot.keyboards[1].reported_type = Some(reported);
        SystemRead {
            snapshot: Some(snapshot),
            warnings: Vec::new(),
            journal: None,
            boot: None,
            summary: StartupSummary::default(),
        }
    }

    fn set_us() -> Request {
        Request::SetLayout(SetLayoutRequest {
            instance_id: KEYCHRON.into(),
            layout: LayoutChoice::Us,
            apply: Default::default(),
            expected: None,
        })
    }

    fn view(
        report: &RequestReport,
        context: &ResultContext<'_>,
        after: Option<&SystemRead>,
        lang: Lang,
    ) -> ResultView {
        result_view(report, &Ok(RunOnceOutcome::NotNeeded), context, after, lang)
    }

    /// Plain Japanese only (review U5): device names and allowed words. Takes a snapshot or a
    /// single text.
    fn plain_japanese(text: &str) {
        let values = if text.contains(": ") && text.contains('\n') {
            crate::vm::snapshot_values(text)
        } else {
            text.to_string()
        };
        let unexpected = crate::vm::unexpected_latin(&values, &["Keychron Receiver"]);
        assert!(unexpected.is_empty(), "{unexpected:?} in {values}");
    }

    #[test]
    fn a_kept_change_and_the_state_now() {
        // T-APPLY-1: kept JIS; "今の状態" waits for the read, then says how it types.
        let targets = targets(LayoutTable::Us);
        let context = ResultContext {
            request: None,
            targets: &targets,
            view: None,
            apply_now: false,
        };
        let done = finished_report(result(Outcome::Confirmed, None));
        let waiting = view(&done, &context, None, Lang::Ja);
        assert_eq!(waiting.current_state, vec!["確かめています…".to_string()]);
        let after = read_after(KeyboardType::JIS);
        let ja = view(&done, &context, Some(&after), Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "title: 完了しました\n\
             tone: Success\n\
             now: Keychron Receiver: JIS として動作中\n\
             message: 新しい配列のままにしました。\n"
        );
        plain_japanese(&ja.snapshot_text());
        let en = view(&done, &context, Some(&after), Lang::En);
        assert_eq!(
            en.snapshot_text(),
            "title: Done\n\
             tone: Success\n\
             now: Keychron Receiver: types JIS\n\
             message: The new layout is kept.\n"
        );
        assert!(
            ja.copy_text.contains("operation: 3f2a9c1e"),
            "{}",
            ja.copy_text
        );
        assert!(ja.copy_text.contains(crate::BUILD_ID));
    }

    #[test]
    fn a_kept_us_assignment_points_to_the_ime_guide() {
        // Review U19: the IME note and its button after "keep" on US.
        let request = set_us();
        let targets = targets(LayoutTable::Jis);
        let context = ResultContext {
            request: Some(&request),
            targets: &targets,
            view: None,
            apply_now: false,
        };
        let after = read_after(KeyboardType::US);
        let ja = view(
            &finished_report(result(Outcome::Confirmed, None)),
            &context,
            Some(&after),
            Lang::Ja,
        );
        assert_eq!(ja.next, Some(NextStep::ImeHelp));
        assert_eq!(
            ja.snapshot_text(),
            "title: 完了しました\n\
             tone: Success\n\
             now: Keychron Receiver: US として動作中\n\
             message: 新しい配列のままにしました。\n\
             ime: US 配列には「半角/全角」キーがありません。日本語入力のオン/オフは Alt+` です。\n\
             next: 入力方式の案内\n"
        );
        plain_japanese(&ja.snapshot_text());
        // Reverted automatically: no IME note, and the keyboard types as before.
        let expired = view(
            &finished_report(result(
                Outcome::Reverted,
                Some(FailureReason::CountdownExpired),
            )),
            &context,
            Some(&read_after(KeyboardType::JIS)),
            Lang::Ja,
        );
        assert_eq!(
            expired.snapshot_text(),
            "title: 自動で元に戻しました\n\
             tone: Warning\n\
             now: Keychron Receiver: JIS として動作中（変更前のまま）\n\
             message: 時間内に［このままにする］が選ばれなかったため、元に戻しました\n"
        );
        assert_eq!(expired.next, None);
    }

    #[test]
    fn the_rows_of_the_b17_table() {
        let context = ResultContext::default();
        let text = |report: &RequestReport, lang| view(report, &context, None, lang);
        // Declined UAC: cancelled, nothing changed (T-APPLY-3).
        let declined = text(
            &report(RequestEnd::NotLaunched(LaunchError::Declined)),
            Lang::Ja,
        );
        assert_eq!(
            declined.snapshot_text(),
            "title: 取り消しました（何も変更していません）\n\
             tone: Neutral\n\
             message: 管理者の確認で「いいえ」が選ばれたため、何も変更していません。\n"
        );
        // A quarantined helper: the antivirus hint; the English message goes to the details.
        let missing = text(
            &report(RequestEnd::NotLaunched(LaunchError::Failed {
                kind: LaunchFailure::HelperMissing,
                message: "mklm-helper.exe is not next to mklm.exe".into(),
            })),
            Lang::Ja,
        );
        assert_eq!(missing.title, "完了できませんでした");
        assert!(
            missing.message.contains("保護の履歴"),
            "{}",
            missing.message
        );
        assert!(missing.details.contains("is not next to"));
        plain_japanese(&missing.message);
        // An engine error: the class decides the title, the code the next step.
        let busy = text(
            &report(RequestEnd::Ended(SessionEnd::Failed(ErrorInfo {
                code: ErrorCode::RecoveryNeeded,
                message: "an entry needs recovery".into(),
                op_id: None,
                plan_error: None,
            }))),
            Lang::En,
        );
        assert_eq!(
            busy.snapshot_text(),
            "title: Another operation is not finished\n\
             tone: Warning\n\
             message: An interrupted operation needs recovery first.\n\
             next: Recover…\n"
        );
        // No answer from the helper (review U7).
        let unresponsive = text(
            &report(RequestEnd::Ended(SessionEnd::Unresponsive)),
            Lang::Ja,
        );
        assert_eq!(unresponsive.title, "完了できませんでした");
        assert!(
            unresponsive
                .message
                .starts_with("MKLM の管理用プログラム（mklm-helper.exe）が応答しません")
        );
        plain_japanese(&unresponsive.message);
        // A restart is needed: the restart page is the next step.
        let restart = text(
            &finished_report(result(Outcome::PendingReboot, None)),
            Lang::Ja,
        );
        assert_eq!(
            restart.snapshot_text(),
            "title: PC の再起動が必要です\n\
             tone: Warning\n\
             message: PC を再起動すると反映されます（シャットダウンではなく再起動）。再起動するまで、MKLM でほかの変更はできません。\n\
             next: 再起動の画面へ\n"
        );
        // A conflict: the conflict page.
        let conflict = text(&finished_report(result(Outcome::Conflict, None)), Lang::En);
        assert_eq!(conflict.next, Some(NextStep::Conflict));
        assert_eq!(conflict.next_step, "Resolve…");
    }

    fn lost_report(recovery: Option<RequestEnd>, skipped: Option<RecoverySkip>) -> RequestReport {
        RequestReport {
            first: RequestEnd::Ended(SessionEnd::Lost {
                exit_code: Some(1),
                detail: "the helper closed the pipe".into(),
                planned: true,
                countdown: true,
            }),
            lost_needs_recovery: Some(Ok(true)),
            recovery,
            recovery_skipped: skipped,
        }
    }

    #[test]
    fn a_lost_helper() {
        let context = ResultContext::default();
        // Recovered at once: the recovery's rows (design m3 B.12, G.2).
        let mut recovered = result(Outcome::Recovered, None);
        recovered.op_id = None;
        recovered.recovered.push(RecoveredOp {
            op_id: op(),
            from: OpState::Written,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        let ja = view(
            &lost_report(
                Some(RequestEnd::Ended(SessionEnd::Finished(recovered))),
                None,
            ),
            &context,
            None,
            Lang::Ja,
        );
        assert_eq!(
            ja.snapshot_text(),
            "title: 自動で元に戻しました\n\
             tone: Warning\n\
             message: MKLM の管理用プログラムが止まったため、すぐに回復しました。\n\
             message: 途中で止まりました → 元に戻しました（リセットの前に止まっていたため、キーボードの動作は変わっていません）\n"
        );
        plain_japanese(&ja.snapshot_text());
        assert!(ja.details.contains("exit code 1"), "{}", ja.details);
        // "Later": the change waits; the recovery page is the next step (design m3 B.17).
        let later = view(
            &lost_report(None, Some(RecoverySkip::Declined)),
            &context,
            None,
            Lang::Ja,
        );
        assert_eq!(
            later.snapshot_text(),
            "title: 完了できませんでした\n\
             tone: Danger\n\
             message: 変更は確認待ちのまま残っています。［回復…］で回復できます。それまで、キーボードの配列は変更できません。\n\
             next: 回復…\n"
        );
        // Nothing to recover: the helper's exit, worded (design m2 E.8).
        let mut nothing = lost_report(None, None);
        nothing.lost_needs_recovery = Some(Ok(false));
        let en = view(&nothing, &context, None, Lang::En);
        assert_eq!(en.title, "Not done");
        assert!(en.message.contains("stopped early"), "{}", en.message);
    }

    #[test]
    fn run_once_notes() {
        let context = ResultContext::default();
        let done = finished_report(result(Outcome::PendingReboot, None));
        let elevated = result_view(
            &done,
            &Ok(RunOnceOutcome::TellUser),
            &context,
            None,
            Lang::Ja,
        );
        assert!(
            elevated
                .run_once_note
                .starts_with("管理者として実行している MKLM は")
        );
        plain_japanese(&elevated.run_once_note);
        let failed = result_view(
            &done,
            &Err(RunOnceError::Register("access denied".into())),
            &context,
            None,
            Lang::En,
        );
        assert!(
            failed
                .run_once_note
                .starts_with("The check after the restart could not be registered")
        );
        assert!(failed.details.contains("access denied"));
        // A prompt that was declined wrote nothing: an unread journal is not worth a note.
        let declined = result_view(
            &report(RequestEnd::NotLaunched(LaunchError::Declined)),
            &Err(RunOnceError::Check("no session ran".into())),
            &context,
            None,
            Lang::Ja,
        );
        assert_eq!(declined.run_once_note, "");
    }

    #[test]
    fn a_blocked_change() {
        let reason = BlockReason::WaitingForReboot(OpRef {
            op_id: op(),
            kind: OpKind::SetLayout {
                requested: KEYCHRON.into(),
                instance_ids: vec![KEYCHRON.into()],
                layout: LayoutChoice::Jis,
            },
            state: OpState::PendingReboot,
        });
        let name_of = |_: &str| "Keychron Receiver".to_string();
        let ja = blocked_view(&reason, &name_of, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "title: ほかの操作が終わっていません\n\
             tone: Warning\n\
             message: PC の再起動を待っている変更があります（Keychron Receiver を JIS に）。再起動するまで、ほかの変更はできません。\n\
             next: 再起動の画面へ\n"
        );
        plain_japanese(&ja.snapshot_text());
        let en = blocked_view(&reason, &name_of, Lang::En);
        assert_eq!(
            en.message,
            "A change waits for a PC restart (Keychron Receiver to JIS); until then no other change is possible."
        );
        assert!(en.details.contains("3f2a9c1e"));
    }

    /// Every part of a result is a line of its own (design m3 B.12): undoing two changes that
    /// waited for the user, and a revert that leaves a reset pending, in both languages.
    #[test]
    fn each_part_of_a_result_is_a_line() {
        let undone = |id: &str| RecoveredOp {
            op_id: OpId::parse(id).unwrap(),
            from: OpState::AwaitingConfirm,
            to: OpState::Reverted,
            decision: "undo".into(),
        };
        let mut undo = result(Outcome::Recovered, None);
        undo.op_id = None;
        undo.recovered = vec![
            undone("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"),
            undone("1ef48b2f-8fca-4c9b-80f5-dfc0965c17a6"),
        ];
        undo.pending_action = Some(PendingAction::ResetKeyboard);
        let context = ResultContext::default();
        let ja = view(&finished_report(undo.clone()), &context, None, Lang::Ja);
        assert_eq!(
            ja.message,
            "確認待ち → 元に戻しました\n\
             確認待ち → 元に戻しました\n\
             保存済み（反映待ち: キーボードのリセットが必要）\n\
             抜き差しするか、［今すぐ反映…］を押してください"
        );
        let en = view(&finished_report(undo), &context, None, Lang::En);
        assert_eq!(en.message.lines().count(), 4, "{}", en.message);
        assert!(
            en.snapshot_text()
                .lines()
                .filter(|l| l.starts_with("message: "))
                .count()
                == 4
        );

        let mut revert = result(Outcome::Reverted, None);
        revert.pending_action = Some(PendingAction::ResetKeyboard);
        let ja = view(&finished_report(revert.clone()), &context, None, Lang::Ja);
        assert_eq!(
            ja.message,
            "元に戻しました。\n\
             保存済み（反映待ち: キーボードのリセットが必要）\n\
             抜き差しするか、［今すぐ反映…］を押してください"
        );
        let en = view(&finished_report(revert), &context, None, Lang::En);
        assert_eq!(
            en.message.lines().next(),
            Some("Put back as it was."),
            "{}",
            en.message
        );
        assert_eq!(en.message.lines().count(), 3);
    }
}
