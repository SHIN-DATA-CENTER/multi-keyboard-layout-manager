//! The write commands on Windows (design F.2): unelevated checks and preview, the user's
//! confirmation, then one helper session (or the in-process fallback), the result, and the
//! post-reboot RunOnce rule after every session.

use std::borrow::Cow;
use std::io::{self, IsTerminal as _, Write};

use anyhow::{Context as _, Result};
use mklm_core::{
    ApplyOptions, BootId, ConflictPolicy, ErrorInfo, GlobalMode, Journal, JournalEntry,
    JournalError, Layout, LayoutChoice, Liveness, OpKind, OpState, OperationError, OperationResult,
    Outcome, PendingAction, ProcessIdentity, RegValue, ResolutionChoice, RestoreScope,
    STORE_VERSION, STORE_VERSION_VALUE, SystemSnapshot, UnreadableEntry, ValueChoice, ValueOp,
    ValueRecord, WriteTarget, attention, plan_migration, plan_set_layout, ps2_pin_layout,
    value_names,
};
use mklm_ipc::{
    Assignment, MigrateRequest, Request, ResolveConflictRequest, RestoreBaselineRequest,
    SetLayoutRequest,
};
use mklm_win::{ReadIssue, ReadIssueKind, elevation, journal_store, proc_identity, session};

use super::checks::{self, Gate, RunOnce};
use super::in_process::{self, InProcessRequest};
use super::input::{self, Input, Line, StdinInput, YesNo};
use super::launch::{self, LaunchError};
use super::outcome::{
    error_exit_code, helper_exit_text, lost_recovery_exit_code, result_exit_code,
};
use super::preview;
use super::relay::{self, Presenter, RelayConfig, SessionEnd};
use super::render::{
    CurrentValue, entry_line, error_text, model_value, next_steps, records_text, result_text,
};
use super::target::{self, ResolvedKeyboard};
use super::{
    AnswerArg, ApplyArgs, ChoiceArg, ConflictArg, LayoutArg, MigrateArgs, RecoverArgs, ResolveArgs,
    RestoreArgs, RevertArgs, SetArgs, StandardArg, exit_code,
};
use crate::json;

/// Oldest Windows build MKLM supports (Windows 11 24H2).
const MIN_BUILD: u32 = 26100;

/// Standard output for progressive text: the console shows any character; a pipe or file gets the
/// console's code page, as `list` and `status` do.
#[derive(Debug)]
struct Stdout {
    console: bool,
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let bytes: Cow<'_, [u8]> = match (self.console, std::str::from_utf8(buf)) {
            (false, Ok(text)) => Cow::Owned(mklm_win::encode_for_redirected_output(text)),
            _ => Cow::Borrowed(buf),
        };
        let mut stdout = io::stdout().lock();
        match stdout.write_all(&bytes) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
            result => result?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match io::stdout().flush() {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            result => result,
        }
    }
}

/// Where a command talks to the user.
#[derive(Debug)]
struct Ui {
    out: Stdout,
    input: StdinInput,
    console: bool,
}

impl Ui {
    fn new() -> Self {
        let console = io::stdout().is_terminal();
        Self {
            out: Stdout { console },
            input: StdinInput::new(),
            console,
        }
    }

    /// Prints `text`, ending it with a line break if it has none.
    fn say(&mut self, text: &str) -> Result<()> {
        self.out.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            self.out.write_all(b"\n")?;
        }
        self.out.flush()?;
        Ok(())
    }

    fn ask(&mut self, question: &str) -> Result<YesNo> {
        Ok(input::ask_yes_no(&mut self.input, &mut self.out, question)?)
    }
}

fn liveness(process: &ProcessIdentity) -> Liveness {
    proc_identity::process_liveness(process)
}

/// The journal as stored, read unelevated, with a newer store layout counted as unreadable (as
/// the engine does); and the stored `StoreVersion`.
fn read_journal() -> Result<(Journal, Option<u32>)> {
    let raw = journal_store::read_journal_store().context("reading the journal failed")?;
    let mut journal = Journal::parse(&raw.ops, &raw.baselines);
    if let Some(found) = raw.store_version.filter(|version| *version > STORE_VERSION) {
        journal.unreadable.push(UnreadableEntry {
            name: STORE_VERSION_VALUE.to_string(),
            error: JournalError::NewerSchema {
                found,
                supported: STORE_VERSION,
            },
        });
    }
    Ok((journal, raw.store_version))
}

fn boot_id() -> Result<BootId> {
    session::boot_id().context("reading the boot ID failed")
}

fn check_os() -> Result<()> {
    let os =
        mklm_win::read_os_info(&mut Vec::new()).context("reading the Windows version failed")?;
    if os.build < MIN_BUILD {
        anyhow::bail!(
            "MKLM changes keyboard settings only on Windows 11 24H2 (build {MIN_BUILD}) or later; \
             this PC runs build {}",
            os.build
        );
    }
    Ok(())
}

/// The keyboards a command plans with.
#[derive(Debug)]
struct Inventory {
    snapshot: SystemSnapshot,
    /// Some override or global value could not be read as MKLM expects (e.g. a value of another
    /// type): the snapshot shows it as absent, while the helper reads it as it is. Then only the
    /// helper may conclude that nothing needs writing.
    uncertain_values: bool,
}

/// Every keyboard, phantoms included (INV-PS2 needs them). Problems that make the list incomplete
/// stop the command, as they would stop the helper (design review S2); the others are warnings.
fn inventory() -> Result<Inventory> {
    let report = mklm_win::snapshot_report(mklm_win::SnapshotOptions {
        include_non_present: true,
    })
    .context("reading the keyboards failed")?;
    let (blocking, other): (Vec<&ReadIssue>, Vec<&ReadIssue>) = report
        .issues
        .iter()
        .partition(|issue| issue.kind.blocks_writes());
    for issue in &other {
        eprintln!("warning: could not read {issue}");
    }
    if !blocking.is_empty() {
        let list: Vec<String> = blocking.iter().map(|issue| issue.to_string()).collect();
        anyhow::bail!(
            "the keyboards could not be read completely, so nothing may be written: {}",
            list.join("; ")
        );
    }
    Ok(Inventory {
        uncertain_values: other
            .iter()
            .any(|issue| issue.kind == ReadIssueKind::Values),
        snapshot: report.snapshot,
    })
}

/// The keyboards for display only (current values, Raw Input): commands that plan nothing, such
/// as `undo` (the way out when something went wrong), must not fail because a keyboard property
/// could not be read. Every problem is a warning; the helper checks again before it writes.
fn display_snapshot() -> Option<SystemSnapshot> {
    match mklm_win::snapshot_report(mklm_win::SnapshotOptions {
        include_non_present: true,
    }) {
        Ok(report) => {
            for issue in &report.issues {
                eprintln!("warning: could not read {issue}");
            }
            Some(report.snapshot)
        }
        Err(error) => {
            eprintln!("warning: could not read the keyboards: {error}");
            None
        }
    }
}

/// The value stored now for a record, from the unelevated snapshot (`None` without one, or for a
/// keyboard that no longer exists).
fn current_of(snapshot: Option<&SystemSnapshot>) -> impl Fn(&ValueRecord) -> Option<RegValue> + '_ {
    move |record| {
        snapshot.and_then(|snapshot| {
            model_value(
                &snapshot.keyboards,
                &snapshot.global,
                &record.target,
                &record.name,
            )
        })
    }
}

/// `"<this exe>" post-reboot`.
fn post_reboot_command() -> Result<String> {
    let exe = std::env::current_exe().context("finding mklm-cli.exe failed")?;
    Ok(format!("\"{}\" post-reboot", exe.display()))
}

/// Design F.4 / review C17: registers the post-reboot check when the journal says a restart is
/// pending, whatever the session returned. Problems are warnings: the journal keeps the state.
///
/// Only an unelevated process registers. An elevated one (an elevated console, `--in-process`)
/// may run as another administrator than the signed-in user, so its `HKCU` would be the wrong
/// account's; it tells the user instead. When elevation cannot be checked, it tells the user too.
fn post_reboot_rule(ui: &mut Ui) {
    let elevated = elevated().unwrap_or(true);
    let decision = read_journal()
        .and_then(|(journal, _)| Ok(checks::run_once(&journal, boot_id()?, elevated)));
    match decision {
        Ok(RunOnce::NotNeeded) => {}
        Ok(RunOnce::Register) => {
            let registered = post_reboot_command()
                .and_then(|command| Ok(session::register_post_reboot(&command)?));
            if let Err(error) = registered {
                eprintln!(
                    "warning: could not register the check after the restart ({error:#}); run \
                     `mklm-cli post-reboot` after signing in"
                );
            }
        }
        Ok(RunOnce::TellUser) => {
            let _ = ui.say(
                "After the restart, run `mklm-cli post-reboot` to keep or revert the change \
                 (this elevated process may belong to another account, so it registers nothing).",
            );
        }
        Err(error) => eprintln!("warning: could not check the journal: {error:#}"),
    }
}

/// `--other-input` / `--no-reset` as options; `None` when neither was given (then the CLI asks
/// before a live reset, design F.1).
fn apply_flags(args: &ApplyArgs) -> Option<ApplyOptions> {
    (args.no_reset || args.other_input).then_some(ApplyOptions {
        allow_live_reset: !args.no_reset,
        other_input_available: args.other_input,
    })
}

/// For `revert`, `undo`, `recover`, `resolve`: without a flag nothing is reset in place; the
/// result says what to do instead (design F.1).
fn apply_or_no_reset(args: &ApplyArgs) -> ApplyOptions {
    apply_flags(args).unwrap_or_default()
}

const OTHER_INPUT_QUESTION: &str = "Can you type another way while it resets (another keyboard, or a mouse and the on-screen keyboard)?";

fn layout_choice(layout: LayoutArg) -> LayoutChoice {
    match layout {
        LayoutArg::Jis => LayoutChoice::Jis,
        LayoutArg::Us => LayoutChoice::Us,
        LayoutArg::Standard => LayoutChoice::Standard,
    }
}

fn operation_error(ui: &mut Ui, error: &OperationError) -> Result<i32> {
    let hint = match error {
        OperationError::MigrationRequired { .. } => {
            "\n  Per-keyboard values have no effect in fixed mode. Switch with `mklm-cli migrate` \
             (add `--also <keyboard>=<layout>` to assign this layout in the same step)."
        }
        OperationError::NotFixedMode => {
            "\n  Nothing to migrate. Assign layouts with `mklm-cli set <keyboard> --layout <layout>` \
             instead of `--also`."
        }
        _ => "",
    };
    eprintln!("error: {error}{hint}");
    ui.out.flush()?;
    Ok(exit_code::FAILURE)
}

/// Explains the UAC prompt (plan 3.5) and asks to go on unless `--yes`. `Ok(false)`: cancelled.
fn confirm(ui: &mut Ui, yes: bool, elevated: bool) -> Result<bool> {
    if !elevated {
        ui.say(
            "Windows asks for administrator permission next. The publisher shows as \"Unknown\" \
             because MKLM is not code-signed yet: check that the program is mklm-helper.exe, then \
             choose Yes.",
        )?;
    }
    if yes {
        return Ok(true);
    }
    match ui.ask("Continue?")? {
        YesNo::Yes => Ok(true),
        YesNo::No | YesNo::NoAnswer => {
            ui.say("Cancelled; nothing was changed.")?;
            Ok(false)
        }
    }
}

fn elevated() -> Result<bool> {
    elevation::is_elevated().context("checking for elevation failed")
}

/// What to add after a session's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    Nothing,
    /// `recover`: say so when entries still wait for the user (design review C7).
    Recover,
    /// `migrate`: the note about the Settings app once the migration waits for the restart.
    Migrate,
}

/// One helper session for `request`, then the post-reboot rule (design F.2 steps 5 to 7).
fn run_request(
    ui: &mut Ui,
    request: Request,
    answer: Option<AnswerArg>,
    apply: ApplyOptions,
    after: After,
) -> Result<i32> {
    let code = match session(ui, request, answer)? {
        Ended::Launch(code) => code,
        Ended::Session(end) => finish(ui, end, apply, after, true)?,
    };
    post_reboot_rule(ui);
    Ok(code)
}

enum Ended {
    /// No session came up; the exit code.
    Launch(i32),
    Session(SessionEnd),
}

fn session(ui: &mut Ui, request: Request, answer: Option<AnswerArg>) -> Result<Ended> {
    let mut session = match launch::start(elevated()?) {
        Ok(session) => session,
        Err(LaunchError::Declined) => {
            ui.say("Cancelled: the administrator permission was declined; nothing was changed.")?;
            return Ok(Ended::Launch(exit_code::CANCELLED));
        }
        Err(LaunchError::Failed(message)) => {
            eprintln!("error: {message}");
            return Ok(Ended::Launch(exit_code::FAILURE));
        }
    };
    let mut presenter = Presenter::new(ui.console, answer);
    let end = relay::relay(
        &mut session,
        request,
        &mut presenter,
        &mut ui.input,
        &mut ui.out,
        RelayConfig::default(),
    );
    session.close();
    Ok(Ended::Session(end?))
}

/// Shows how a session ended and returns the exit code. A helper lost after the operation was
/// journaled is followed at once by a recovery with the same options (design E.7, review C8),
/// unless `recover_lost` is false (that recovery itself).
fn finish(
    ui: &mut Ui,
    end: SessionEnd,
    apply: ApplyOptions,
    after: After,
    recover_lost: bool,
) -> Result<i32> {
    match end {
        SessionEnd::Finished(result) => {
            ui.say(&result_text(&result))?;
            if let Some(next) = next_steps(&result) {
                ui.say(&next)?;
            }
            if after == After::Migrate && result.outcome == Outcome::PendingReboot {
                ui.say(
                    "From now on, choosing \"Use connected keyboard layout\" in Settings is fine \
                     (plan 1.3).",
                )?;
            }
            if after == After::Recover {
                recover_note(ui, &result)?;
            }
            Ok(result_exit_code(&result))
        }
        SessionEnd::Failed(info) => {
            ui.out.flush()?;
            eprint!("{}", error_text(&info));
            Ok(error_exit_code(info.code))
        }
        SessionEnd::Unresponsive => {
            eprintln!(
                "error: the helper does not respond. Wait until it ends, or end mklm-helper.exe in \
                 Task Manager, then run `mklm-cli recover`."
            );
            Ok(exit_code::FAILURE)
        }
        SessionEnd::Lost {
            exit_code: helper_code,
            detail,
            planned,
            countdown,
        } => {
            let why = helper_code.map_or(detail, helper_exit_text);
            eprintln!("error: the helper stopped before it answered: {why}");
            let (journal, _) = read_journal()?;
            let boot = boot_id()?;
            let needs_recovery = journal.entries.iter().any(|entry| {
                attention(entry, boot, liveness(&entry.owner)) == mklm_core::Attention::Recover
            });
            if !(recover_lost && needs_recovery) {
                if planned || needs_recovery {
                    eprintln!(
                        "Run `mklm-cli recover` to finish or undo the interrupted operation \
                         (`mklm-cli journal` shows where it stopped)."
                    );
                }
                return Ok(exit_code::FAILURE);
            }
            // Recovery may finish the operation as well as undo it (C.7): the result says which.
            ui.say(if countdown {
                "The helper stopped during the countdown; recovering now."
            } else {
                "Recovering what the helper left unfinished now."
            })?;
            let request = Request::Recover { apply };
            match session(ui, request, None)? {
                Ended::Launch(code) => {
                    eprintln!("Run `mklm-cli recover` as soon as possible.");
                    Ok(code)
                }
                Ended::Session(SessionEnd::Finished(result)) => {
                    finish(
                        ui,
                        SessionEnd::Finished(result.clone()),
                        apply,
                        After::Recover,
                        false,
                    )?;
                    Ok(lost_recovery_exit_code(&result))
                }
                Ended::Session(end) => finish(ui, end, apply, After::Nothing, false),
            }
        }
    }
}

/// Design review C7: a recovery that left entries waiting for the user says so.
fn recover_note(ui: &mut Ui, result: &OperationResult) -> Result<()> {
    let (journal, _) = read_journal()?;
    let waiting: Vec<&JournalEntry> = journal
        .open_entries()
        .into_iter()
        .filter(|entry| !entry.state.is_in_flight())
        .collect();
    if result.recovered.is_empty() && waiting.is_empty() {
        ui.say("Nothing needed recovery.")?;
    }
    for entry in waiting {
        ui.say(&format!(
            "Still waiting for you: {}. `mklm-cli undo` puts it back.",
            entry_line(entry)
        ))?;
    }
    Ok(())
}

/// The keyboard of a command, echoed with its name and instance ID (design F.1).
fn echo_keyboard(
    ui: &mut Ui,
    snapshot: &SystemSnapshot,
    reference: &super::KeyboardRef,
) -> Result<Option<ResolvedKeyboard>> {
    match target::resolve_keyboard(&snapshot.keyboards, reference) {
        Ok(keyboard) => {
            ui.say(&keyboard.describe())?;
            Ok(Some(keyboard))
        }
        Err(message) => {
            eprintln!("error: {message}");
            Ok(None)
        }
    }
}

/// Stops a command behind `gate`, or in a dry run notes what would stop it.
fn gate(
    ui: &mut Ui,
    journal: &Journal,
    boot: BootId,
    gate: Gate,
    dry_run: bool,
) -> Result<Option<i32>> {
    match checks::blocker(journal, boot, &liveness, gate) {
        None => Ok(None),
        Some(blocker) if dry_run => {
            ui.say(&format!(
                "Note: the helper would refuse this now: {}.",
                blocker.message
            ))?;
            Ok(Some(blocker.exit_code))
        }
        Some(blocker) => {
            ui.out.flush()?;
            eprintln!("error: {}.", blocker.message);
            Err(Blocked(blocker.exit_code).into())
        }
    }
}

/// A command stopped by [`gate`]; carries its exit code up to the command.
#[derive(Debug)]
struct Blocked(i32);

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "blocked")
    }
}

impl std::error::Error for Blocked {}

/// Runs a command body; a [`Blocked`] error becomes its exit code.
fn guarded(body: impl FnOnce() -> Result<i32>) -> Result<i32> {
    match body() {
        Err(error) => match error.downcast_ref::<Blocked>() {
            Some(Blocked(code)) => Ok(*code),
            None => Err(error),
        },
        ok => ok,
    }
}

/// Ends a dry run: the helper is only looked at (its build ID, design E.3), never started.
fn dry_run_done(ui: &mut Ui) -> Result<()> {
    match launch::checked_helper_path() {
        Ok(helper) => ui.say(&format!(
            "Helper: {} carries this build's ID ({}).",
            helper.display(),
            super::BUILD_ID
        ))?,
        Err(LaunchError::Failed(message)) => ui.say(&format!("Note: {message}."))?,
        Err(LaunchError::Declined) => {}
    }
    ui.say("Dry run: nothing was written and the helper was not started.")
}

/// Common start of every write command (design F.2 steps 1 and 2).
fn start(ui: &mut Ui, dry_run: bool) -> Result<(Journal, BootId)> {
    check_os()?;
    if !dry_run {
        post_reboot_rule(ui);
    }
    let (journal, _) = read_journal()?;
    Ok((journal, boot_id()?))
}

/// `mklm-cli set` (design D.2, F.1).
pub fn set(args: &SetArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, args.dry_run)?;
        let Inventory {
            snapshot,
            uncertain_values,
        } = inventory()?;
        let Some(keyboard) = echo_keyboard(&mut ui, &snapshot, &args.keyboard)? else {
            return Ok(exit_code::FAILURE);
        };
        let blocked = gate(&mut ui, &journal, boot, Gate::NewOp, args.dry_run)?;
        let choice = layout_choice(args.layout);
        let flags = apply_flags(&args.apply);
        let mut options = flags.unwrap_or(ApplyOptions {
            allow_live_reset: true,
            other_input_available: true,
        });
        let plan_for = |options: &ApplyOptions| {
            plan_set_layout(
                &snapshot.keyboards,
                &snapshot.global,
                &keyboard.instance_id,
                choice,
                options,
            )
        };
        let mut plan = match plan_for(&options) {
            Ok(plan) => plan,
            Err(error) => return operation_error(&mut ui, &error),
        };
        if !uncertain_values && preview::nothing_to_change(&snapshot, &plan) {
            ui.say(&format!(
                "Nothing to change: {} already has this layout.",
                keyboard.display_name
            ))?;
            if args.dry_run {
                dry_run_done(&mut ui)?;
            }
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if flags.is_none() && plan.apply != PendingAction::ResetKeyboard {
            // The question was not asked, so the user declared nothing: the request must not
            // claim another way to type (C9). The plan is made again with the options that are
            // sent, so that what is shown, sent as the expected plan and written agree (usually
            // the same apply method: no live reset was planned anyway).
            options = preview::no_other_input(&options);
            plan = match plan_for(&options) {
                Ok(plan) => plan,
                Err(error) => return operation_error(&mut ui, &error),
            };
        } else if flags.is_none() {
            let alone = plan_for(&preview::no_other_input(&options));
            if args.dry_run {
                if let Ok(alone) = alone {
                    ui.say(&format!(
                        "Without --other-input or --no-reset, you will be asked whether you can \
                         type another way. Answering no makes the change take effect {}; with \
                         --no-reset it takes effect when the keyboard reconnects instead.",
                        super::render::apply_text(alone.apply)
                    ))?;
                }
            } else {
                ui.say(
                    "The keyboard can be reset in place so that the new layout applies at once. \
                     While it resets, it cannot type for a few seconds.",
                )?;
                if ui.ask(OTHER_INPUT_QUESTION)? != YesNo::Yes {
                    options = preview::no_other_input(&options);
                    plan = match alone {
                        Ok(plan) => plan,
                        Err(error) => return operation_error(&mut ui, &error),
                    };
                }
            }
        }
        ui.say(&preview::plan_text(
            &snapshot,
            &plan,
            Some(&plan.instance_ids),
        ))?;
        if choice == LayoutChoice::Standard {
            ui.say(
                "Note: \"follow the standard\" has not been verified by typing yet; check the \
                 layout with Shift+2 afterwards.",
            )?;
        }
        if args.dry_run {
            dry_run_done(&mut ui)?;
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        let request = Request::SetLayout(SetLayoutRequest {
            instance_id: keyboard.instance_id.clone(),
            layout: choice,
            apply: options,
            expected: Some(preview::expected(&plan)),
        });
        run_request(&mut ui, request, args.answer, options, After::Nothing)
    })
}

/// `mklm-cli migrate` (design D.3).
pub fn migrate(args: &MigrateArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, args.dry_run)?;
        let snapshot = inventory()?.snapshot;
        let mut assignments = Vec::new();
        for also in &args.also {
            let Some(keyboard) = echo_keyboard(&mut ui, &snapshot, &also.keyboard)? else {
                return Ok(exit_code::FAILURE);
            };
            assignments.push((keyboard.instance_id, layout_choice(also.layout)));
        }
        let blocked = gate(&mut ui, &journal, boot, Gate::NewOp, args.dry_run)?;
        let global = &snapshot.global;
        if global.mode() != GlobalMode::Fixed {
            // Design F.5: already in per-keyboard mode is "no change" (0), like `set` with
            // nothing to change, unless `--also` asked for assignments migrate cannot make (1).
            if !assignments.is_empty() {
                return operation_error(&mut ui, &OperationError::NotFixedMode);
            }
            ui.say("Nothing to migrate: this PC is already in per-keyboard mode.")?;
            if args.dry_run {
                dry_run_done(&mut ui)?;
            }
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        let standard = match args.standard {
            Some(StandardArg::Jis) => Layout::Jis,
            Some(StandardArg::Us) => Layout::Us,
            None => match ps2_pin_layout(global) {
                Some(layout) => layout,
                None => {
                    eprintln!(
                        "error: the global values do not tell which layout the fixed mode uses; \
                         pass --standard jis or --standard us"
                    );
                    return Ok(exit_code::FAILURE);
                }
            },
        };
        let plan = match plan_migration(&snapshot.keyboards, global, standard, &assignments) {
            Ok(plan) => plan,
            Err(error) => return operation_error(&mut ui, &error),
        };
        // Plan 1.5 / design review S8: the layout DLL the migration names must exist.
        for step in plan
            .checked
            .steps
            .iter()
            .filter(|s| s.target == WriteTarget::Global)
        {
            for write in &step.writes {
                if let (true, ValueOp::SetString(dll)) = (
                    write
                        .name
                        .eq_ignore_ascii_case(value_names::LAYER_DRIVER_JPN),
                    &write.op,
                ) {
                    if !elevation::system32_file_exists(dll).unwrap_or(false) {
                        let error = OperationError::LayerDriverMissing { dll: dll.clone() };
                        return operation_error(&mut ui, &error);
                    }
                    ui.say(&format!("Layout driver {dll}: present in System32."))?;
                }
            }
        }
        ui.say(&preview::plan_text(&snapshot, &plan, None))?;
        ui.say(
            "After the restart, `mklm-cli post-reboot` shows what each keyboard reports and asks \
             whether to keep the change.",
        )?;
        if args.dry_run {
            dry_run_done(&mut ui)?;
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        let request = Request::Migrate(MigrateRequest {
            standard,
            assignments: assignments
                .into_iter()
                .map(|(instance_id, layout)| Assignment {
                    instance_id,
                    layout,
                })
                .collect(),
            expected: Some(preview::expected(&plan)),
        });
        run_request(
            &mut ui,
            request,
            None,
            ApplyOptions::default(),
            After::Migrate,
        )
    })
}

/// Finds `op` in the journal (a full ID or a unique prefix of 8+ hex digits).
fn find_entry<'a>(journal: &'a Journal, op: &str) -> Option<&'a JournalEntry> {
    match journal.resolve_prefix(op) {
        Ok(entry) => Some(entry),
        Err(error) => {
            eprintln!("error: {error}; `mklm-cli journal` lists the operations");
            None
        }
    }
}

/// `mklm-cli revert <op>` (design D.4).
pub fn revert(args: &RevertArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, false)?;
        let Some(entry) = find_entry(&journal, &args.op) else {
            return Ok(exit_code::FAILURE);
        };
        gate(&mut ui, &journal, boot, Gate::Existing, false)?;
        let revertible = match entry.state {
            OpState::AwaitingConfirm | OpState::PendingReboot => true,
            OpState::Confirmed => !matches!(entry.kind, OpKind::RestoreBaseline { .. }),
            _ => false,
        };
        if !revertible {
            let hint = match entry.state {
                OpState::Confirmed => {
                    " (a kept restore to the values before MKLM cannot be reverted; set the layout \
                     again instead)"
                }
                OpState::Conflict => " (use `mklm-cli resolve` or `mklm-cli undo`)",
                _ => "",
            };
            eprintln!(
                "error: operation {} is {}; it cannot be reverted{hint}",
                entry.op_id.short(),
                super::render::state_text(entry.state)
            );
            return Ok(exit_code::FAILURE);
        }
        let snapshot = display_snapshot();
        ui.say(&preview::revert_text(entry, &current_of(snapshot.as_ref())))?;
        let apply = apply_or_no_reset(&args.apply);
        if !apply.allow_live_reset || !apply.other_input_available {
            ui.say(
                "Keyboards are not reset in place (add --other-input to allow it); the result \
                 says how the old layout comes back.",
            )?;
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        let request = Request::Revert {
            op_id: entry.op_id.clone(),
            apply,
        };
        run_request(&mut ui, request, None, apply, After::Nothing)
    })
}

/// Runs the in-process fallback and shows the result (design F.2 step 6).
fn run_in_process(ui: &mut Ui, request: &InProcessRequest, apply: ApplyOptions) -> Result<i32> {
    if !crate::dll_search_restricted() {
        anyhow::bail!(
            "--in-process refuses to run: DLL loading could not be restricted to System32"
        );
    }
    let presenter = Presenter::new(ui.console, None);
    let result: Result<OperationResult, ErrorInfo> =
        in_process::run(request, apply, presenter, &mut ui.input, &mut ui.out)?;
    // `recover` says what still waits for the user, as through the helper (design review C7).
    let after = match request {
        InProcessRequest::Recover => After::Recover,
        InProcessRequest::Undo | InProcessRequest::Restore { .. } => After::Nothing,
    };
    let code = match result {
        Ok(result) => finish(ui, SessionEnd::Finished(result), apply, after, false)?,
        Err(info) => finish(ui, SessionEnd::Failed(info), apply, After::Nothing, false)?,
    };
    post_reboot_rule(ui);
    Ok(code)
}

/// `mklm-cli undo` (design D.10).
pub fn undo(args: &RecoverArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, args.dry_run)?;
        let blocked = gate(&mut ui, &journal, boot, Gate::Recover, args.dry_run)?;
        let snapshot = display_snapshot();
        ui.say(&preview::undo_text(
            &journal,
            &current_of(snapshot.as_ref()),
        ))?;
        if journal.open_entries().is_empty() {
            if args.dry_run {
                dry_run_done(&mut ui)?;
            }
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        let apply = apply_or_no_reset(&args.apply);
        if args.dry_run {
            dry_run_done(&mut ui)?;
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if args.in_process {
            // No UAC here: the fallback runs only in an elevated console.
            if !confirm(&mut ui, args.yes, true)? {
                return Ok(exit_code::CANCELLED);
            }
            return run_in_process(&mut ui, &InProcessRequest::Undo, apply);
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        run_request(
            &mut ui,
            Request::Undo { apply },
            None,
            apply,
            After::Nothing,
        )
    })
}

/// `mklm-cli recover` (design D.7). Always runs the helper, even with nothing to recover: it also
/// clears stale "not in effect yet" records, and it is the first real-machine check (H.2 R1).
pub fn recover(args: &RecoverArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, args.dry_run)?;
        let blocked = gate(&mut ui, &journal, boot, Gate::Recover, args.dry_run)?;
        let mut listed = false;
        for entry in &journal.entries {
            let text = match attention(entry, boot, liveness(&entry.owner)) {
                mklm_core::Attention::Recover => "interrupted; recovery finishes or undoes it",
                mklm_core::Attention::AwaitingUser
                | mklm_core::Attention::WaitingForReboot
                | mklm_core::Attention::Conflict => {
                    "waits for you; recovery leaves it (see `mklm-cli undo`)"
                }
                mklm_core::Attention::NeedsApply => "not in effect yet",
                mklm_core::Attention::Busy | mklm_core::Attention::None => continue,
            };
            ui.say(&format!("{}: {text}", entry_line(entry)))?;
            listed = true;
        }
        if !listed {
            ui.say("No interrupted operation is recorded; the helper will only check.")?;
        }
        let apply = apply_or_no_reset(&args.apply);
        if args.dry_run {
            dry_run_done(&mut ui)?;
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if args.in_process {
            // No UAC here: the fallback runs only in an elevated console.
            if !confirm(&mut ui, args.yes, true)? {
                return Ok(exit_code::CANCELLED);
            }
            return run_in_process(&mut ui, &InProcessRequest::Recover, apply);
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        run_request(
            &mut ui,
            Request::Recover { apply },
            None,
            apply,
            After::Recover,
        )
    })
}

fn resolution(choice: ChoiceArg) -> ResolutionChoice {
    match choice {
        ChoiceArg::KeepCurrent => ResolutionChoice::KeepCurrent,
        ChoiceArg::Before => ResolutionChoice::UseBefore,
        ChoiceArg::Intended => ResolutionChoice::UseIntended,
        ChoiceArg::Baseline => ResolutionChoice::UseBaseline,
    }
}

/// The choice for every record: `--value` (1-based) wins over `--all`; without either, asked one
/// by one. `Ok(None)`: the user gave up (end of input).
fn resolution_choices(
    ui: &mut Ui,
    args: &ResolveArgs,
    entry: &JournalEntry,
) -> Result<Option<Vec<ValueChoice>>> {
    let count = entry.records.len();
    let mut choices: Vec<Option<ResolutionChoice>> = vec![args.all.map(resolution); count];
    for value in &args.values {
        if value.record == 0 || value.record > count {
            anyhow::bail!(
                "--value {}: operation {} has records 1 to {count}",
                value.record,
                entry.op_id.short()
            );
        }
        choices[value.record - 1] = Some(resolution(value.choice));
    }
    if args.all.is_none() && args.values.is_empty() {
        for (index, record) in entry.records.iter().enumerate() {
            let question = format!(
                "[{}] {}\\{}: (k)eep current, (b)efore, (i)ntended or base(l)ine? [k]",
                index + 1,
                record.key_path,
                record.name
            );
            loop {
                match input::ask_line(&mut ui.input, &mut ui.out, &question)? {
                    None => return Ok(None),
                    Some(answer) => {
                        let choice = match answer.trim().to_ascii_lowercase().as_str() {
                            "" | "k" | "keep" | "keep-current" => ResolutionChoice::KeepCurrent,
                            "b" | "before" => ResolutionChoice::UseBefore,
                            "i" | "intended" => ResolutionChoice::UseIntended,
                            "l" | "baseline" => ResolutionChoice::UseBaseline,
                            _ => {
                                ui.say("Please answer k, b, i or l.")?;
                                continue;
                            }
                        };
                        choices[index] = Some(choice);
                        break;
                    }
                }
            }
        }
    }
    Ok(Some(
        choices
            .into_iter()
            .enumerate()
            .filter_map(|(record, choice)| choice.map(|choice| ValueChoice { record, choice }))
            .collect(),
    ))
}

/// `mklm-cli resolve <op>` (design D.8).
pub fn resolve(args: &ResolveArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, false)?;
        let Some(entry) = find_entry(&journal, &args.op) else {
            return Ok(exit_code::FAILURE);
        };
        gate(&mut ui, &journal, boot, Gate::Existing, false)?;
        if entry.state != OpState::Conflict {
            eprintln!(
                "error: operation {} is {}, not in conflict",
                entry.op_id.short(),
                super::render::state_text(entry.state)
            );
            return Ok(exit_code::FAILURE);
        }
        let snapshot = display_snapshot();
        let current = current_of(snapshot.as_ref());
        ui.say(&entry_line(entry))?;
        let current: Option<CurrentValue<'_>> =
            snapshot.as_ref().map(|_| &current as CurrentValue<'_>);
        ui.say(&records_text(entry, current))?;
        let Some(choices) = resolution_choices(&mut ui, args, entry)? else {
            ui.say("Cancelled; nothing was changed.")?;
            return Ok(exit_code::CANCELLED);
        };
        ui.say("Resolution:")?;
        for (index, record) in entry.records.iter().enumerate() {
            let choice = choices
                .iter()
                .find(|c| c.record == index)
                .map_or(ResolutionChoice::KeepCurrent, |c| c.choice);
            let choice = match choice {
                ResolutionChoice::KeepCurrent => "keep the current value",
                ResolutionChoice::UseBefore => "the value before the operation",
                ResolutionChoice::UseIntended => "the value the operation wrote",
                ResolutionChoice::UseBaseline => "the value before MKLM",
            };
            ui.say(&format!(
                "  [{}] {}\\{}: {choice}",
                index + 1,
                record.key_path,
                record.name
            ))?;
        }
        let apply = apply_or_no_reset(&args.apply);
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        let request = Request::ResolveConflict(ResolveConflictRequest {
            op_id: entry.op_id.clone(),
            choices,
            apply,
        });
        run_request(&mut ui, request, None, apply, After::Nothing)
    })
}

fn conflict_policy(arg: ConflictArg) -> ConflictPolicy {
    match arg {
        ConflictArg::Report => ConflictPolicy::Report,
        ConflictArg::Skip => ConflictPolicy::Skip,
        ConflictArg::Overwrite => ConflictPolicy::Overwrite,
    }
}

/// `mklm-cli restore --baseline` (design D.5).
pub fn restore(args: &RestoreArgs) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, args.dry_run)?;
        let Inventory {
            snapshot,
            uncertain_values,
        } = inventory()?;
        let scope = match &args.keyboard {
            None => RestoreScope::All,
            Some(reference) => {
                let Some(keyboard) = echo_keyboard(&mut ui, &snapshot, reference)? else {
                    return Ok(exit_code::FAILURE);
                };
                RestoreScope::Device {
                    instance_id: keyboard.instance_id,
                }
            }
        };
        let blocked = gate(&mut ui, &journal, boot, Gate::Restore, args.dry_run)?;
        let policy = conflict_policy(args.on_conflict);
        let flags = apply_flags(&args.apply);
        let mut options = flags.unwrap_or(ApplyOptions {
            allow_live_reset: true,
            other_input_available: true,
        });
        let preview_for = |options: &ApplyOptions| {
            preview::preview_restore(&snapshot, &journal, &scope, policy, options)
        };
        let mut restore = match preview_for(&options) {
            Ok(restore) => restore,
            Err(error) => return operation_error(&mut ui, &error),
        };
        let asks = flags.is_none() && restore.apply == Some(PendingAction::ResetKeyboard);
        if asks && !args.dry_run {
            // The preview once, before the question; after it only what changed.
            ui.say(&preview::restore_text(&restore))?;
            if ui.ask(OTHER_INPUT_QUESTION)? != YesNo::Yes {
                options = preview::no_other_input(&options);
                restore = match preview_for(&options) {
                    Ok(restore) => restore,
                    Err(error) => return operation_error(&mut ui, &error),
                };
                if let Some(apply) = restore.apply {
                    ui.say(&format!(
                        "Takes effect: {}",
                        super::render::apply_text(apply)
                    ))?;
                }
            }
        } else {
            if flags.is_none() && !asks {
                // Nothing was asked, so nothing was declared: the request must not claim another
                // way to type (C9). The helper plans the apply method again by itself (a restore
                // carries no expected plan), so with a keyboard plugged in meanwhile it must not
                // take a reset the user never agreed to.
                options = preview::no_other_input(&options);
                restore = match preview_for(&options) {
                    Ok(restore) => restore,
                    Err(error) => return operation_error(&mut ui, &error),
                };
            }
            ui.say(&preview::restore_text(&restore))?;
        }
        if let Err(error) = &restore.order {
            eprintln!("error: {error}");
            return Ok(exit_code::FAILURE);
        }
        if uncertain_values {
            // The helper reads the values itself and decides; nothing is concluded from here.
            ui.say(
                "Some values could not be read here as MKLM expects; the helper reads them itself \
                 and may find more to restore or a conflict.",
            )?;
        } else if !restore.conflicts.is_empty() && policy == ConflictPolicy::Report {
            ui.say(
                "Nothing was written. Check the values above, then run the command again with \
                 `--on-conflict skip` (leave them) or `--on-conflict overwrite` (put back the \
                 values from before MKLM).",
            )?;
            if args.dry_run {
                dry_run_done(&mut ui)?;
            }
            return Ok(blocked.unwrap_or(exit_code::CONFLICT));
        } else if restore.writes.is_empty() {
            if args.dry_run {
                dry_run_done(&mut ui)?;
            }
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if args.dry_run {
            dry_run_done(&mut ui)?;
            return Ok(blocked.unwrap_or(exit_code::OK));
        }
        if args.in_process {
            // No UAC here: the fallback runs only in an elevated console.
            if !confirm(&mut ui, args.yes, true)? {
                return Ok(exit_code::CANCELLED);
            }
            let request = InProcessRequest::Restore {
                scope,
                on_conflict: policy,
            };
            return run_in_process(&mut ui, &request, options);
        }
        if !confirm(&mut ui, args.yes, elevated()?)? {
            return Ok(exit_code::CANCELLED);
        }
        let request = Request::RestoreBaseline(RestoreBaselineRequest {
            scope,
            on_conflict: policy,
            apply: options,
        });
        run_request(&mut ui, request, None, options, After::Nothing)
    })
}

/// The post-reboot check of one entry (design D.7, D.6): the Raw Input table and the Shift+2
/// test.
fn show_check(ui: &mut Ui, snapshot: Option<&SystemSnapshot>, entry: &JournalEntry) -> Result<()> {
    let migration = matches!(entry.kind, OpKind::Migrate { .. });
    let rows = match snapshot {
        Some(snapshot) => preview::check_rows(snapshot, entry),
        None => {
            ui.say("What the keyboards report could not be read; the typing test decides.")?;
            Vec::new()
        }
    };
    ui.say(&preview::check_text(&rows, migration))
}

/// `mklm-cli keep <op>` (design D.6).
pub fn keep(op: &str) -> Result<i32> {
    guarded(|| {
        let mut ui = Ui::new();
        let (journal, boot) = start(&mut ui, false)?;
        let Some(entry) = find_entry(&journal, op) else {
            return Ok(exit_code::FAILURE);
        };
        gate(&mut ui, &journal, boot, Gate::Existing, false)?;
        if !matches!(
            entry.state,
            OpState::AwaitingConfirm | OpState::PendingReboot
        ) {
            eprintln!(
                "error: operation {} is {}; only a change that waits for keep or revert can be \
                 kept",
                entry.op_id.short(),
                super::render::state_text(entry.state)
            );
            return Ok(exit_code::FAILURE);
        }
        ui.say(&entry_line(entry))?;
        if checks::takes_effect_at_restart(entry) {
            if entry.state == OpState::PendingReboot && entry.boot_id == boot {
                eprintln!(
                    "error: the PC has not restarted since this change was written. Restart it \
                     first (Restart, not Shut down): `mklm-cli reboot`."
                );
                return Ok(exit_code::RESTART_REQUIRED);
            }
            // Design review C2: a change that took effect at a restart is kept only after the
            // user saw what the keyboards report and typed the test.
            let snapshot = display_snapshot();
            show_check(&mut ui, snapshot.as_ref(), entry)?;
            if ui.ask("Keep these changes?")? != YesNo::Yes {
                ui.say(&format!(
                    "Not kept; nothing was changed. `mklm-cli revert {}` undoes it.",
                    entry.op_id.short()
                ))?;
                return Ok(exit_code::CANCELLED);
            }
        }
        confirm(&mut ui, true, elevated()?)?;
        let request = Request::Confirm {
            op_id: entry.op_id.clone(),
        };
        run_request(
            &mut ui,
            request,
            None,
            ApplyOptions::default(),
            After::Nothing,
        )
    })
}

/// `mklm-cli reboot` (design F.4).
pub fn reboot(yes: bool) -> Result<i32> {
    let mut ui = Ui::new();
    let (journal, _) = read_journal()?;
    let boot = boot_id()?;
    let reasons = checks::restart_reasons(&journal, boot);
    if reasons.is_empty() {
        ui.say("No change needs a restart.")?;
        return Ok(exit_code::OK);
    }
    ui.say("These changes need a PC restart:")?;
    for entry in &reasons {
        ui.say(&format!("  {}", entry_line(entry)))?;
    }
    ui.say(
        "If you cannot type at the sign-in screen: use the on-screen keyboard (the Accessibility \
         button at the bottom right), sign in with your PIN, or plug in another keyboard.",
    )?;
    if !yes {
        let answer = input::ask_line(
            &mut ui.input,
            &mut ui.out,
            "Type restart to restart the PC now:",
        )?;
        if answer.as_deref().map(str::trim) != Some("restart") {
            ui.say("Not restarted.")?;
            return Ok(exit_code::CANCELLED);
        }
    }
    post_reboot_rule(&mut ui);
    session::restart_pc().context("restarting the PC failed")?;
    ui.say("Restarting...")?;
    Ok(exit_code::OK)
}

/// How the user answered the post-reboot question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Keep,
    Revert,
    /// Enter alone or end of input: nothing now, ask again next time (design D.7 step 4).
    Later,
}

fn ask_verdict(ui: &mut Ui) -> Result<Verdict> {
    loop {
        write!(ui.out, "Keep these changes? [y/n, Enter = decide later] ")?;
        ui.out.flush()?;
        match ui.input.next_line(None) {
            Some(Line::Text(text)) => match text.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(Verdict::Keep),
                "n" | "no" => return Ok(Verdict::Revert),
                "" => return Ok(Verdict::Later),
                _ => ui.say("Please answer y or n.")?,
            },
            Some(Line::Eof) | None => {
                writeln!(ui.out)?;
                return Ok(Verdict::Later);
            }
        }
    }
}

/// `mklm-cli post-reboot`, started by the RunOnce entry (design D.7, F.4).
pub fn post_reboot() -> Result<i32> {
    let mut ui = Ui::new();
    let (journal, _) = read_journal()?;
    let boot = boot_id()?;
    let entries = checks::post_reboot_entries(&journal);
    if entries.is_empty() {
        ui.say("Nothing waits for a check after a restart.")?;
        return Ok(exit_code::OK);
    }
    let snapshot = display_snapshot();
    let mut code = exit_code::OK;
    for entry in entries {
        ui.say(&entry_line(entry))?;
        if entry.state == OpState::PendingReboot && entry.boot_id == boot {
            ui.say(
                "Not in effect yet: the PC has not restarted since the change was written. \
                 Restart it with Restart, not Shut down (with Fast Startup a shutdown keeps the \
                 drivers as they were).",
            )?;
            post_reboot_rule(&mut ui);
            code = exit_code::RESTART_REQUIRED;
            continue;
        }
        show_check(&mut ui, snapshot.as_ref(), entry)?;
        let op = entry.op_id.short().to_string();
        let request = match ask_verdict(&mut ui)? {
            Verdict::Keep => Request::Confirm {
                op_id: entry.op_id.clone(),
            },
            Verdict::Revert => Request::Revert {
                op_id: entry.op_id.clone(),
                apply: ApplyOptions::default(),
            },
            Verdict::Later => {
                ui.say(&format!(
                    "Nothing changed; you will be asked again after the next sign-in (or run \
                     `mklm-cli keep {op}` / `mklm-cli revert {op}`)."
                ))?;
                post_reboot_rule(&mut ui);
                code = exit_code::AWAITING_CONFIRM;
                continue;
            }
        };
        confirm(&mut ui, true, elevated()?)?;
        code = run_request(
            &mut ui,
            request,
            None,
            ApplyOptions::default(),
            After::Nothing,
        )?;
    }
    Ok(code)
}

/// `mklm-cli journal` (read-only).
pub fn journal(json_output: bool) -> Result<String> {
    let (journal, store_version) = read_journal()?;
    let boot = session::boot_id().ok();
    if json_output {
        let document =
            super::journal_view::journal_document(&journal, store_version, boot, &liveness);
        return Ok(json::to_json(&document, !io::stdout().is_terminal())?);
    }
    // The values stored now, when the keyboards can be read (display only).
    let snapshot = mklm_win::snapshot_report(mklm_win::SnapshotOptions {
        include_non_present: true,
    })
    .ok()
    .map(|report| report.snapshot);
    let current = current_of(snapshot.as_ref());
    let current: Option<CurrentValue<'_>> = snapshot.as_ref().map(|_| &current as CurrentValue<'_>);
    Ok(super::journal_view::journal_text(
        &journal, boot, &liveness, current,
    ))
}
