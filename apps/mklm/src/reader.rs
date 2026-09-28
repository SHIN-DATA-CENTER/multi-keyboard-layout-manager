//! The I/O worker (design m3 A.4): one long-lived thread for every blocking call that is not a
//! helper session — enumerating keyboards (hundreds of milliseconds), reading the journal, saving
//! settings, the RunOnce rule, the autostart value, opening a settings page or the recovery files
//! folder, restarting the PC (design m3 B.8, B.11), and the preparation of a change
//! (`PrepareChange`, design m3 B.5). Tasks run in order; repeated reads that queue up are merged,
//! and of several queued preparations only the last runs (the page dropped the others). Results
//! go back with `slint::invoke_from_event_loop`.
//!
//! Writes (settings, the RunOnce rule, the autostart value) are counted from the moment they are
//! queued until they are done, so that quitting — and the end of the Windows session — can wait a
//! bounded time for them ([`wait_for_writes`], design m3 F.5). The count is a static outside any
//! `RefCell`, like the session's cancel flag: the end-of-session handler reads it from inside the
//! shell window's procedure.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use mklm_client::gate::{Gate, blocker};
use mklm_client::inventory::{InventoryError, read_inventory};
use mklm_client::startup::summarize;
use mklm_win::ui::SettingsPage;

use crate::app::post;
use crate::autostart::{self, AutostartTask};
use crate::log;
use crate::settings::{Settings, SettingsStore};
use crate::state::{AppMsg, JournalMsg, PrepareFailure, PreparedChange, SystemRead, WizardMsg};
use crate::vm::wizard::ForeignValues;

/// A job for the I/O worker.
#[derive(Debug, Clone, PartialEq)]
pub enum IoTask {
    /// Snapshot (display: every problem is a warning), journal, boot ID, attention summary.
    Read,
    /// [`IoTask::Read`], then `AppMsg::ResultReadArrived`: the result's "今の状態" waits for a
    /// read that started after the session ended (design m3 B.17).
    ReadForResult,
    /// What a change is planned with: the planning inventory (read problems that stop a write
    /// stop it here too), the journal and `gate::blocker(gate)` (design m3 B.5).
    PrepareChange {
        token: u64,
        gate: Gate,
    },
    /// Save settings.toml through the worker's [`SettingsStore`] (skipped without a settings
    /// folder; after an unreadable start the old file is kept as settings.toml.bad first).
    SaveSettings(Box<Settings>),
    /// `mklm_client::run_once::apply_run_once_rule(PostRebootCommand::Gui)`.
    RunOnceRule,
    /// Read or change the autostart value (`autostart::run`), answered by
    /// `AppMsg::AutostartRead`.
    Autostart(AutostartTask),
    OpenSettingsPage(SettingsPage),
    /// The RunOnce rule, then `mklm_win::session::restart_pc` (design m3 B.8): the rule first, so
    /// that the check after the restart opens by itself.
    RestartPc,
    /// `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` in Explorer (design m3 B.11).
    OpenRecoveryFolder,
    /// The override values of the non-keyboard collections of these keyboards' devices
    /// (`mklm_win::read_non_keyboard_values`, read only; the wizard's step 3, design m3 B.1, J.9),
    /// answered by `WizardMsg::ForeignRead`.
    ReadNonKeyboardValues(Vec<mklm_core::KeyboardDevice>),
}

impl IoTask {
    /// Tasks a quit waits for (bounded): they write the user's files or registry values.
    fn writes(&self) -> bool {
        match self {
            // The restart writes the RunOnce rule first.
            IoTask::SaveSettings(_) | IoTask::RunOnceRule | IoTask::RestartPc => true,
            IoTask::Autostart(task) => *task != AutostartTask::Read,
            IoTask::Read
            | IoTask::ReadForResult
            | IoTask::PrepareChange { .. }
            | IoTask::OpenSettingsPage(_)
            | IoTask::OpenRecoveryFolder
            | IoTask::ReadNonKeyboardValues(_) => false,
        }
    }
}

/// Writes queued or running.
static PENDING_WRITES: Mutex<usize> = Mutex::new(0);
static WRITES_DONE: Condvar = Condvar::new();

fn add_pending_write(delta: isize) {
    let mut pending = PENDING_WRITES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *pending = pending.saturating_add_signed(delta);
    if *pending == 0 {
        WRITES_DONE.notify_all();
    }
}

/// Waits up to `limit` for the queued writes; true when none is left (design m3 F.5: 1 s before
/// quitting).
pub fn wait_for_writes(limit: Duration) -> bool {
    let pending = PENDING_WRITES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let (pending, _) = WRITES_DONE
        .wait_timeout_while(pending, limit, |pending| *pending > 0)
        .unwrap_or_else(PoisonError::into_inner);
    *pending == 0
}

/// The UI thread's handle on the worker.
#[derive(Debug)]
pub struct IoWorker {
    tasks: Sender<IoTask>,
}

impl IoWorker {
    /// Starts the worker; `store` saves the settings (`None`: no settings folder, nothing is
    /// saved).
    pub fn start(store: Option<SettingsStore>) -> std::io::Result<Self> {
        let (tasks, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("mklm-io".into())
            .spawn(move || run(receiver, store))?;
        Ok(Self { tasks })
    }

    pub fn send(&self, task: IoTask) {
        let writes = task.writes();
        if writes {
            add_pending_write(1);
        }
        if self.tasks.send(task).is_err() && writes {
            add_pending_write(-1);
        }
    }
}

fn run(tasks: Receiver<IoTask>, mut store: Option<SettingsStore>) {
    while let Ok(first) = tasks.recv() {
        // Merge reads that queued up behind each other. The batch is taken after every task in
        // it was queued, so its one read is newer than any of them.
        let mut batch = vec![first];
        batch.extend(tasks.try_iter());
        let last_prepare = batch
            .iter()
            .rposition(|task| matches!(task, IoTask::PrepareChange { .. }));
        let mut read_done = false;
        let mut result_read = false;
        for (index, task) in batch.into_iter().enumerate() {
            result_read |= matches!(task, IoTask::ReadForResult);
            let writes = task.writes();
            match task {
                IoTask::Read | IoTask::ReadForResult => {
                    if !read_done {
                        read_done = true;
                        post(AppMsg::SystemRead(Box::new(read_system())));
                    }
                }
                // An older preparation: the page has moved on to a newer one.
                IoTask::PrepareChange { .. } if Some(index) != last_prepare => {}
                IoTask::PrepareChange { token, gate } => {
                    let prepared = prepare_change(gate);
                    post(AppMsg::ChangePrepared {
                        token,
                        at: Instant::now(),
                        prepared: Box::new(prepared),
                    });
                }
                IoTask::SaveSettings(settings) => {
                    if let Some(store) = &mut store
                        && let Err(error) = store.save(&settings)
                    {
                        // Kept in memory; the next change saves again.
                        log::warn(format!(
                            "saving the settings in {} failed: {error}",
                            store.dir().display()
                        ));
                    }
                }
                IoTask::RunOnceRule => {
                    // The pages warn when the check after the restart could not be registered.
                    let outcome = mklm_client::run_once::apply_run_once_rule(
                        mklm_client::run_once::PostRebootCommand::Gui,
                    );
                    log_run_once(&outcome);
                    post(AppMsg::Journal(JournalMsg::RunOnceDone(outcome)));
                }
                IoTask::Autostart(task) => {
                    post(AppMsg::AutostartRead(Box::new(autostart::run(task))));
                }
                IoTask::OpenSettingsPage(page) => {
                    if let Err(error) = mklm_win::ui::open_settings_page(page) {
                        log::warn(format!("opening {page:?} failed: {error}"));
                    }
                }
                IoTask::RestartPc => restart_pc(),
                IoTask::OpenRecoveryFolder => {
                    if mklm_win::ui::open_recovery_folder().is_err() {
                        post(AppMsg::Journal(JournalMsg::RecoveryFolderFailed));
                    }
                }
                IoTask::ReadNonKeyboardValues(keyboards) => {
                    let found = read_non_keyboard_values(&keyboards);
                    post(AppMsg::Wizard(WizardMsg::ForeignRead(found)));
                }
            }
            if writes {
                add_pending_write(-1);
            }
        }
        if result_read {
            post(AppMsg::ResultReadArrived);
        }
    }
}

/// "今すぐ再起動" (design m3 B.8): only while the journal, read again now, still has a reason to
/// restart (`PendingReboot` flushed by the helper, plan 2.3); then the RunOnce rule, so that the
/// check after the restart opens by itself; then the restart. A failed registration does not stop
/// the restart (the pages say it, and the journal still asks for the check on the next start of
/// MKLM); a failed restart is reported.
fn restart_pc() {
    let still_needed = mklm_client::journal::read_journal()
        .map_err(|error| format!("reading the journal failed: {error}"))
        .and_then(|read| {
            let boot = mklm_client::journal::boot_id()
                .map_err(|error| format!("reading the boot ID failed: {error}"))?;
            Ok(!mklm_client::gate::restart_reasons(&read.journal, boot).is_empty())
        });
    match still_needed {
        Ok(true) => {}
        Ok(false) => {
            post(AppMsg::Journal(JournalMsg::RestartNotNeeded));
            return;
        }
        Err(error) => {
            post(AppMsg::Journal(JournalMsg::RestartFailed(error)));
            return;
        }
    }
    let outcome =
        mklm_client::run_once::apply_run_once_rule(mklm_client::run_once::PostRebootCommand::Gui);
    log_run_once(&outcome);
    post(AppMsg::Journal(JournalMsg::RunOnceDone(outcome)));
    if let Err(error) = mklm_win::session::restart_pc() {
        log::warn(format!("restarting the PC failed: {error}"));
        post(AppMsg::Journal(JournalMsg::RestartFailed(
            error.to_string(),
        )));
    }
}

/// Logs what the post-reboot RunOnce rule did (design m3 F.8); `TellUser` cannot happen
/// unelevated.
fn log_run_once(
    outcome: &Result<mklm_client::run_once::RunOnceOutcome, mklm_client::run_once::RunOnceError>,
) {
    match outcome {
        Ok(done) => log::info(format!("post-reboot RunOnce rule: {done:?}")),
        Err(error) => log::warn(format!("post-reboot RunOnce rule failed: {error}")),
    }
}

/// Everything the main screen needs, read unelevated.
pub fn read_system() -> SystemRead {
    let mut warnings = Vec::new();
    let snapshot = match mklm_client::inventory::read_display_snapshot() {
        Ok((snapshot, issues)) => {
            warnings.extend(issues.iter().map(ToString::to_string));
            Some(snapshot)
        }
        Err(error) => {
            warnings.push(format!("reading the keyboards failed: {error}"));
            None
        }
    };
    let journal = match mklm_client::journal::read_journal() {
        Ok(read) => Some(read.journal),
        Err(error) => {
            warnings.push(format!("reading the journal failed: {error}"));
            None
        }
    };
    let boot = mklm_client::journal::boot_id()
        .map_err(|error| warnings.push(format!("reading the boot ID failed: {error}")))
        .ok();
    let summary = match (&journal, boot) {
        (Some(journal), Some(boot)) => summarize(journal, boot, &mklm_client::journal::liveness),
        _ => Default::default(),
    };
    SystemRead {
        snapshot,
        warnings,
        journal,
        boot,
        summary,
    }
}

/// The values on non-keyboard collections of the devices of `keyboards` (design m3 B.1, J.9),
/// read unelevated. A hint only: problems are logged, and an error leaves the rows without it.
fn read_non_keyboard_values(
    keyboards: &[mklm_core::KeyboardDevice],
) -> Result<Vec<ForeignValues>, String> {
    let mut issues = Vec::new();
    let found = mklm_win::read_non_keyboard_values(keyboards, &mut issues);
    for issue in &issues {
        log::warn(format!("non-keyboard values: {issue}"));
    }
    let found = found.map_err(|error| {
        log::warn(format!("reading the non-keyboard values failed: {error}"));
        error.to_string()
    })?;
    Ok(found
        .into_iter()
        .filter_map(|values| {
            let names: Vec<String> = values
                .overrides
                .present_value_names()
                .into_iter()
                .map(str::to_string)
                .collect();
            (!names.is_empty()).then_some(ForeignValues {
                instance_id: values.instance_id,
                keyboard: values.keyboard_instance_id,
                names,
            })
        })
        .collect())
}

/// What a change is planned with (design m3 B.5, m2 F.2 step 4), read unelevated: nothing here
/// writes, and the helper checks everything again under its lock.
pub fn prepare_change(gate: Gate) -> Result<PreparedChange, PrepareFailure> {
    let inventory = read_inventory().map_err(|error| match error {
        InventoryError::Read(error) => {
            PrepareFailure::Read(format!("reading the keyboards failed: {error}"))
        }
        InventoryError::Incomplete { blocking, .. } => PrepareFailure::Incomplete(
            blocking
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    })?;
    let journal = mklm_client::journal::read_journal()
        .map_err(|error| PrepareFailure::Read(format!("reading the journal failed: {error}")))?
        .journal;
    let boot = mklm_client::journal::boot_id()
        .map_err(|error| PrepareFailure::Read(format!("reading the boot ID failed: {error}")))?;
    let blocker = blocker(&journal, boot, &mklm_client::journal::liveness, gate);
    Ok(PreparedChange {
        snapshot: inventory.snapshot,
        uncertain_values: inventory.uncertain_values,
        warnings: inventory.warnings.iter().map(ToString::to_string).collect(),
        journal,
        blocker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_writes_are_waited_for() {
        assert!(IoTask::SaveSettings(Box::default()).writes());
        assert!(IoTask::RunOnceRule.writes());
        assert!(IoTask::Autostart(AutostartTask::Repair).writes());
        assert!(IoTask::Autostart(AutostartTask::Set(false)).writes());
        assert!(!IoTask::Autostart(AutostartTask::Read).writes());
        assert!(!IoTask::Read.writes());
        assert!(!IoTask::ReadForResult.writes());
        assert!(
            !IoTask::PrepareChange {
                token: 0,
                gate: Gate::Restore
            }
            .writes()
        );
        assert!(!IoTask::OpenSettingsPage(SettingsPage::Taskbar).writes());
    }

    #[test]
    fn the_wait_for_writes_is_bounded() {
        // (The only test that touches the process-wide count.)
        assert!(wait_for_writes(Duration::ZERO));
        add_pending_write(1);
        let started = Instant::now();
        assert!(!wait_for_writes(Duration::from_millis(100)));
        assert!(started.elapsed() >= Duration::from_millis(90));
        let finisher = thread::spawn(|| {
            thread::sleep(Duration::from_millis(50));
            add_pending_write(-1);
        });
        assert!(wait_for_writes(Duration::from_secs(10)));
        finisher.join().unwrap();
        assert!(wait_for_writes(Duration::ZERO));
    }
}
