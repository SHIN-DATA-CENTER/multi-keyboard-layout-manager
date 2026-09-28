//! Text of the write commands: values, keys, journal entries, results and errors (English, like
//! the rest of the CLI for now).

use std::fmt::Write as _;

use mklm_client::describe::{ResetPhase, reset_phase_from};
use mklm_core::{
    ConflictInfo, ErrorCode, ErrorInfo, FailureReason, JournalEntry, Layout, LayoutChoice, OpKind,
    OpState, OperationResult, Outcome, PendingAction, RecoveredOp, RegValue, RestoreScope,
    ValueKey, ValueRecord, WriteTarget,
};

pub use mklm_client::values::{model_value, op_value};

use crate::text::{dword_text, pending_name_long};

/// The value stored now for a record (`None`: its keyboard no longer exists).
pub type CurrentValue<'a> = &'a dyn Fn(&ValueRecord) -> Option<RegValue>;

/// The registry root every recorded key path is relative to.
const CONTROL_SET: &str = r"HKLM\SYSTEM\CurrentControlSet";

/// Appends to a `String`, which cannot fail.
macro_rules! put {
    ($out:expr) => {
        $out.push('\n')
    };
    ($out:expr, $($arg:tt)*) => {{
        let _ = writeln!($out, $($arg)*);
    }};
}
pub(crate) use put;

/// A registry value as the user reads it.
pub fn value_text(value: &RegValue) -> String {
    match value {
        RegValue::Absent => "(none)".to_string(),
        RegValue::Dword { value } => dword_text(*value),
        RegValue::Sz { value } => format!("\"{value}\""),
        RegValue::Other { reg_type, data_hex } => format!("(type {reg_type}: {data_hex})"),
    }
}

/// `HKLM\SYSTEM\CurrentControlSet\Enum\…\Device Parameters` or `…\Services\i8042prt\Parameters`.
pub fn key_text(target: &WriteTarget) -> String {
    let key = ValueKey {
        target: target.clone(),
        name: String::new(),
    };
    format!(r"{CONTROL_SET}\{}", key.key_path())
}

/// How a change takes effect, in words.
pub fn apply_text(action: PendingAction) -> &'static str {
    match action {
        PendingAction::ResetKeyboard => {
            "the keyboard is reset in place now; then keep or revert within 20 seconds \
             (no answer reverts)"
        }
        PendingAction::Reconnect => {
            "when the keyboard reconnects (unplug and replug it; Bluetooth: turn it off and on)"
        }
        PendingAction::RestartPc => "at the next PC restart (Restart, not Shut down)",
    }
}

pub fn state_text(state: OpState) -> &'static str {
    match state {
        OpState::Planned => "planned (interrupted?)",
        OpState::Written => "written (interrupted?)",
        OpState::Restarting => "resetting the keyboard",
        OpState::AwaitingConfirm => "waiting for keep or revert",
        OpState::PendingReboot => "waiting for a PC restart",
        OpState::Confirmed => "kept",
        OpState::RevertPending => "being reverted",
        OpState::Reverted => "reverted",
        OpState::RevertedPendingReboot => "reverted; the PC must restart",
        OpState::Failed => "failed (nothing kept)",
        OpState::Conflict => "in conflict",
    }
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Jis => "JIS",
        Layout::Us => "US",
    }
}

pub fn choice_text(choice: LayoutChoice) -> &'static str {
    match choice {
        LayoutChoice::Jis => "JIS",
        LayoutChoice::Us => "US",
        LayoutChoice::Standard => "the PC's standard layout",
    }
}

/// What an operation was for, on one line.
pub fn kind_text(kind: &OpKind) -> String {
    match kind {
        OpKind::SetLayout {
            requested, layout, ..
        } => format!("set {requested} to {}", choice_text(*layout)),
        OpKind::Migrate {
            standard,
            assignments,
        } => {
            let mut text = format!(
                "migrate to per-keyboard mode (standard layout {})",
                layout_name(*standard)
            );
            for (id, choice) in assignments {
                let _ = write!(text, ", {id} = {}", choice_text(*choice));
            }
            text
        }
        OpKind::RestoreBaseline { scope, silent, .. } => {
            let scope = match scope {
                RestoreScope::All => "every value".to_string(),
                RestoreScope::Device { instance_id } => instance_id.clone(),
            };
            let silent = if *silent { " (uninstall)" } else { "" };
            format!("restore the values from before MKLM: {scope}{silent}")
        }
        OpKind::Cleanup { instance_id, names } => format!(
            "delete values the driver does not read from {instance_id}: {}",
            names.join(", ")
        ),
    }
}

/// Why an operation did not end as requested. `phase` tells whether it got as far as the
/// keyboard reset (`mklm_client::describe`): `LiveResetUnconfirmed` is recorded both before and
/// after the reset, and only the words differ (design m3 G.2; M2 real-machine test R5).
pub fn failure_text(failure: &FailureReason, phase: ResetPhase) -> String {
    match failure {
        FailureReason::ConcurrentChange { name } => {
            format!("{name} changed between planning and writing; nothing was written")
        }
        FailureReason::WriteError { message } => {
            format!("a write failed ({message}); the values were put back")
        }
        FailureReason::CallerDisconnected => "the command ended before the change was kept".into(),
        FailureReason::Interrupted => {
            "the writer stopped halfway; recovery put the values back".into()
        }
        FailureReason::LiveResetUnconfirmed => match phase {
            ResetPhase::NotReached => "the writer stopped before the keyboard reset; recovery put \
                                       the values back (the keyboard never switched)"
                .into(),
            ResetPhase::Reached => {
                "the change was never kept after the keyboard reset; recovery put it back".into()
            }
        },
        FailureReason::CountdownExpired => "no answer before the countdown ran out".into(),
        FailureReason::KeyboardDidNotReturn => {
            "the keyboard did not come back after the reset, or Windows asked for a restart".into()
        }
        FailureReason::NothingWritten => "nothing had been written".into(),
        FailureReason::ConflictKeptCurrent => "the values changed outside MKLM were kept".into(),
        FailureReason::Superseded { by } => {
            format!(
                "replaced by the restore to the values before MKLM ({})",
                by.short()
            )
        }
    }
}

/// `3f2a9c1e  set … to JIS  (waiting for keep or revert)`.
pub fn entry_line(entry: &JournalEntry) -> String {
    format!(
        "{}  {}  ({})",
        entry.op_id.short(),
        kind_text(&entry.kind),
        state_text(entry.state)
    )
}

/// The records of `entry`, numbered from 1 as `resolve --value <n>=…` takes them. With `current`
/// (the values read now; `None` for a keyboard that no longer exists), also the value now.
pub fn records_text(entry: &JournalEntry, current: Option<CurrentValue<'_>>) -> String {
    let mut out = String::new();
    for (index, record) in entry.records.iter().enumerate() {
        put!(
            out,
            "  [{}] {}\\{}",
            index + 1,
            record.key_path,
            record.name
        );
        put!(
            out,
            "        before {}, intended {}, baseline {}",
            value_text(&record.before),
            value_text(&record.intended),
            value_text(&record.baseline)
        );
        let last = record
            .last_written
            .as_ref()
            .map_or_else(|| "nothing yet".to_string(), value_text);
        match current {
            Some(current) => {
                let now = current(record)
                    .map_or_else(|| "(device removed)".to_string(), |v| value_text(&v));
                put!(out, "        last written {last}, now {now}");
            }
            None => put!(out, "        last written {last}"),
        }
        if let Some(conflict) = &record.conflict {
            put!(out, "        conflict: found {}", value_text(conflict));
        }
        if let Some(error) = &record.write_error {
            put!(out, "        write error: {error}");
        }
        if let Some(skipped) = record.skipped {
            put!(out, "        skipped: {skipped:?}");
        }
    }
    out
}

fn conflict_lines(out: &mut String, conflicts: &[ConflictInfo]) {
    for conflict in conflicts {
        put!(
            out,
            "  [{}] {} of operation {}: {}\\{}",
            conflict.record + 1,
            if conflict.write_error.is_some() {
                "cannot be written"
            } else {
                "changed outside MKLM"
            },
            conflict.op_id.short(),
            conflict.key_path,
            conflict.name
        );
        let last = conflict
            .last_written
            .as_ref()
            .map_or_else(|| "nothing yet".to_string(), value_text);
        put!(
            out,
            "        now {}; MKLM last wrote {last}; before {}, intended {}, baseline {}",
            value_text(&conflict.current),
            value_text(&conflict.before),
            value_text(&conflict.intended),
            value_text(&conflict.baseline)
        );
        if let Some(error) = &conflict.write_error {
            put!(out, "        write error: {error}");
        }
    }
}

/// How far the operation of a result got, for its reason (design m3 G.2): a recovered row for the
/// same operation tells (`reset_phase_from`, its state when recovery found it). Without one the
/// reset counts as reached, the wording M2 used: a result's own `LiveResetUnconfirmed` comes
/// only from recovery, and saying the keyboard never switched is only right when it is known.
fn result_reset_phase(result: &OperationResult) -> ResetPhase {
    result
        .recovered
        .iter()
        .find(|recovered| result.op_id.as_ref() == Some(&recovered.op_id))
        .map_or(ResetPhase::Reached, |recovered| {
            reset_phase_from(recovered.from)
        })
}

/// The end of a recovered row (design m3 G.2, B.12): an operation that recovery put back
/// (`Reverted`) from a state before its keyboard reset (`Planned`, `Written`) never switched the
/// keyboard, which the user should know (M2 real-machine test R5). Put back into a state that
/// needs a restart, or from a later state, nothing is added.
fn recovered_note(recovered: &RecoveredOp) -> &'static str {
    match (recovered.to, reset_phase_from(recovered.from)) {
        (OpState::Reverted, ResetPhase::NotReached) => {
            "; stopped before the keyboard reset, so the keyboard never switched"
        }
        _ => "",
    }
}

/// The final answer of a request.
pub fn result_text(result: &OperationResult) -> String {
    let mut out = String::new();
    let op = result
        .op_id
        .as_ref()
        .map(|id| format!("Operation {}: ", id.short()))
        .unwrap_or_default();
    let headline = match result.outcome {
        Outcome::NoChange => "Nothing to change: every value already is as requested.",
        Outcome::Confirmed => "Done: the change is kept.",
        Outcome::AwaitingConfirm => "Written; waiting for you to keep or revert it.",
        Outcome::PendingReboot => "Written; it takes effect when the PC restarts.",
        Outcome::Reverted if result.failure.is_some() => "Reverted automatically.",
        Outcome::Reverted => "Reverted.",
        Outcome::RevertedPendingReboot if result.failure.is_some() => {
            "Reverted automatically; the PC must restart to finish."
        }
        Outcome::RevertedPendingReboot => "Reverted; the PC must restart to finish.",
        Outcome::Failed => "Not done.",
        Outcome::Conflict => "Stopped: some values are not what MKLM expected.",
        Outcome::Recovered => "Recovery finished.",
    };
    put!(out, "{op}{headline}");
    if let Some(failure) = &result.failure {
        put!(
            out,
            "  Reason: {}",
            failure_text(failure, result_reset_phase(result))
        );
    }
    for recovered in &result.recovered {
        put!(
            out,
            "  {}: {} -> {} ({}{})",
            recovered.op_id.short(),
            state_text(recovered.from),
            state_text(recovered.to),
            recovered.decision,
            recovered_note(recovered)
        );
    }
    if !result.conflicts.is_empty() {
        put!(out, "  Values in conflict:");
        conflict_lines(&mut out, &result.conflicts);
    }
    if let Some(violation) = &result.inv_ps2_violation {
        put!(out, "  INV-PS2 is broken: {violation}");
    }
    if let Some(action) = result.pending_action {
        put!(out, "  Still to do: {}.", pending_name_long(action));
    }
    for warning in &result.warnings {
        put!(out, "warning: {warning}");
    }
    out
}

/// What to run next after a result, if anything.
pub fn next_steps(result: &OperationResult) -> Option<String> {
    let op = result.op_id.as_ref().map(|id| id.short().to_string());
    match (result.outcome, op) {
        (Outcome::AwaitingConfirm, Some(op)) => Some(format!(
            "When the keyboard types as intended, run `mklm-cli keep {op}`; to undo it, \
             `mklm-cli revert {op}` (or `mklm-cli undo`)."
        )),
        (Outcome::PendingReboot, _) => Some(
            "Restart the PC with `mklm-cli reboot` (Restart, not Shut down). After signing in, \
             `mklm-cli post-reboot` asks whether to keep the change. To undo it before \
             restarting, run `mklm-cli undo`."
                .to_string(),
        ),
        (Outcome::RevertedPendingReboot, _) => {
            Some("Restart the PC with `mklm-cli reboot` (Restart, not Shut down).".to_string())
        }
        (Outcome::Conflict, Some(op)) => Some(format!(
            "Decide with `mklm-cli resolve {op}`, or put everything back with `mklm-cli undo`."
        )),
        (Outcome::Conflict, None) => Some(
            "Choose `--on-conflict skip` or `--on-conflict overwrite` after checking the values."
                .to_string(),
        ),
        _ => None,
    }
}

/// A failed request.
pub fn error_text(info: &ErrorInfo) -> String {
    let mut out = String::new();
    put!(out, "error: {}", info.message);
    if let Some(plan) = &info.plan_error {
        put!(out, "  {plan}");
    }
    let hint = match info.code {
        ErrorCode::Busy => Some("Another MKLM process is writing; try again when it has finished."),
        ErrorCode::OpInProgress => Some(
            "Finish the open operation first: `mklm-cli keep`, `mklm-cli revert` or `mklm-cli undo`.",
        ),
        ErrorCode::RecoveryNeeded => Some("Run `mklm-cli recover` first."),
        ErrorCode::JournalUnreadable => {
            Some("The journal was written by a newer MKLM or is damaged; update MKLM.")
        }
        ErrorCode::MigrationRequired => Some(
            "The PC is in fixed mode, where per-keyboard values have no effect. Switch with \
             `mklm-cli migrate` (add `--also <keyboard>=<layout>` to assign this layout in the \
             same step).",
        ),
        ErrorCode::PlanChanged => {
            Some("The keyboards or values changed after the plan was shown; run the command again.")
        }
        ErrorCode::NotLatest => Some(
            "A later operation changed the same values; use `mklm-cli restore --baseline` instead.",
        ),
        ErrorCode::InventoryIncomplete => Some(
            "Windows did not report every keyboard completely; nothing was written. Try again.",
        ),
        _ => None,
    };
    if let Some(hint) = hint {
        put!(out, "  {hint}");
    }
    out
}

#[cfg(test)]
mod tests {
    use mklm_core::{OpId, ValueOp, fixtures, value_names};

    use super::*;

    #[test]
    fn reasons_follow_the_reset_phase() {
        // Design m3 G.2 (R5): `LiveResetUnconfirmed` before and after the reset.
        let unconfirmed = FailureReason::LiveResetUnconfirmed;
        assert_eq!(
            failure_text(&unconfirmed, ResetPhase::NotReached),
            "the writer stopped before the keyboard reset; recovery put the values back (the \
             keyboard never switched)"
        );
        assert_eq!(
            failure_text(&unconfirmed, ResetPhase::Reached),
            "the change was never kept after the keyboard reset; recovery put it back"
        );
        for phase in [ResetPhase::NotReached, ResetPhase::Reached] {
            assert_eq!(
                failure_text(&FailureReason::CountdownExpired, phase),
                "no answer before the countdown ran out"
            );
        }

        // A result's own reason: the recovered row of the same operation tells the phase;
        // without one, the reset counts as reached (the M2 wording).
        let op = OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap();
        let mut result = OperationResult {
            op_id: Some(op.clone()),
            outcome: Outcome::Reverted,
            failure: Some(unconfirmed),
            pending_action: None,
            conflicts: Vec::new(),
            inv_ps2_violation: None,
            recovered: Vec::new(),
            warnings: Vec::new(),
        };
        let text = result_text(&result);
        assert!(
            text.contains(
                "  Reason: the change was never kept after the keyboard reset; recovery put it back"
            ),
            "{text}"
        );
        result.recovered.push(RecoveredOp {
            op_id: op,
            from: OpState::Written,
            to: OpState::Reverted,
            decision: "roll-back".into(),
        });
        let text = result_text(&result);
        assert!(
            text.contains("  Reason: the writer stopped before the keyboard reset;"),
            "{text}"
        );
        assert!(
            text.contains(
                "  3f2a9c1e: written (interrupted?) -> reverted (roll-back; stopped before the \
                 keyboard reset, so the keyboard never switched)\n"
            ),
            "{text}"
        );

        // Put back into a state that needs a restart, or after the reset: no claim about the
        // keyboard.
        result.recovered[0].to = OpState::RevertedPendingReboot;
        assert!(result_text(&result).contains(
            "  3f2a9c1e: written (interrupted?) -> reverted; the PC must restart (roll-back)\n"
        ));
        result.recovered[0].from = OpState::Restarting;
        result.recovered[0].to = OpState::Reverted;
        let text = result_text(&result);
        assert!(
            text.contains("  3f2a9c1e: resetting the keyboard -> reverted (roll-back)\n"),
            "{text}"
        );
        assert!(
            text.contains("  Reason: the change was never kept after the keyboard reset"),
            "{text}"
        );
    }

    #[test]
    fn values_and_keys() {
        assert_eq!(value_text(&RegValue::Absent), "(none)");
        assert_eq!(value_text(&RegValue::Dword { value: 7 }), "7");
        assert_eq!(value_text(&RegValue::Dword { value: 0x51 }), "81 (0x51)");
        assert_eq!(
            value_text(&RegValue::Sz {
                value: "kbd106.dll".into()
            }),
            "\"kbd106.dll\""
        );
        assert_eq!(op_value(&ValueOp::Delete), RegValue::Absent);
        assert_eq!(
            key_text(&WriteTarget::Global),
            r"HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters"
        );
        assert_eq!(
            key_text(&WriteTarget::Device {
                instance_id: r"ACPI\FUJ0309\4&320DB4C2&0".into()
            }),
            r"HKLM\SYSTEM\CurrentControlSet\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters"
        );
    }

    #[test]
    fn model_values_follow_the_snapshot() {
        let snapshot = fixtures::dev_machine();
        let (keyboards, global) = (&snapshot.keyboards, &snapshot.global);
        let keychron = WriteTarget::Device {
            instance_id: fixtures::keychron().instance_id.to_ascii_lowercase(),
        };
        assert_eq!(
            model_value(keyboards, global, &keychron, value_names::HID_TYPE),
            Some(RegValue::Dword { value: 4 })
        );
        assert_eq!(
            model_value(keyboards, global, &keychron, value_names::PS2_TYPE),
            Some(RegValue::Absent)
        );
        assert_eq!(
            model_value(keyboards, global, &WriteTarget::Global, "layerdriver jpn"),
            Some(RegValue::Sz {
                value: "kbd106.dll".into()
            })
        );
        assert_eq!(
            model_value(
                keyboards,
                global,
                &WriteTarget::Global,
                value_names::PS2_TYPE
            ),
            Some(RegValue::Absent)
        );
        let removed = WriteTarget::Device {
            instance_id: r"HID\VID_0000&PID_0000\1".into(),
        };
        assert_eq!(
            model_value(keyboards, global, &removed, value_names::HID_TYPE),
            None
        );
    }
}
