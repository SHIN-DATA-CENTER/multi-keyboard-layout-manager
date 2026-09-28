//! The I/O worker (design m3 A.4): one long-lived thread for every blocking call that is not a
//! helper session — enumerating keyboards (hundreds of milliseconds), reading the journal, saving
//! settings, the RunOnce rule, opening a settings page or the recovery files folder, restarting
//! the PC (design m3 B.8, B.11), and the preparation of a change (`PrepareChange`, design m3
//! B.5). Tasks run in order; repeated reads that queue up are merged,
//! and of several queued preparations only the last runs (the page dropped the others). Results
//! go back with `slint::invoke_from_event_loop`.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use mklm_client::gate::{Gate, blocker};
use mklm_client::inventory::{InventoryError, read_inventory};
use mklm_client::startup::summarize;
use mklm_win::ui::SettingsPage;

use crate::app::post;
use crate::settings::Settings;
use crate::state::{AppMsg, JournalMsg, PrepareFailure, PreparedChange, SystemRead};

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
    SaveSettings {
        settings: Box<Settings>,
        dir: PathBuf,
    },
    /// `mklm_client::run_once::apply_run_once_rule(PostRebootCommand::Gui)`.
    RunOnceRule,
    OpenSettingsPage(SettingsPage),
    /// The RunOnce rule, then `mklm_win::session::restart_pc` (design m3 B.8): the rule first, so
    /// that the check after the restart opens by itself.
    RestartPc,
    /// `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` in Explorer (design m3 B.11).
    OpenRecoveryFolder,
}

/// The UI thread's handle on the worker.
#[derive(Debug)]
pub struct IoWorker {
    tasks: Sender<IoTask>,
}

impl IoWorker {
    pub fn start() -> std::io::Result<Self> {
        let (tasks, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("mklm-io".into())
            .spawn(move || run(receiver))?;
        Ok(Self { tasks })
    }

    pub fn send(&self, task: IoTask) {
        let _ = self.tasks.send(task);
    }
}

fn run(tasks: Receiver<IoTask>) {
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
                IoTask::SaveSettings { settings, dir } => {
                    // WP-U6: report a failure to the UI (a banner) instead of dropping it.
                    let _ = settings.save(&dir);
                }
                IoTask::RunOnceRule => {
                    // The pages warn when the check after the restart could not be registered.
                    let outcome = mklm_client::run_once::apply_run_once_rule(
                        mklm_client::run_once::PostRebootCommand::Gui,
                    );
                    post(AppMsg::Journal(JournalMsg::RunOnceDone(outcome)));
                }
                IoTask::OpenSettingsPage(page) => {
                    let _ = mklm_win::ui::open_settings_page(page);
                }
                IoTask::RestartPc => restart_pc(),
                IoTask::OpenRecoveryFolder => {
                    if mklm_win::ui::open_recovery_folder().is_err() {
                        post(AppMsg::Journal(JournalMsg::RecoveryFolderFailed));
                    }
                }
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
    post(AppMsg::Journal(JournalMsg::RunOnceDone(outcome)));
    if let Err(error) = mklm_win::session::restart_pc() {
        post(AppMsg::Journal(JournalMsg::RestartFailed(
            error.to_string(),
        )));
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
