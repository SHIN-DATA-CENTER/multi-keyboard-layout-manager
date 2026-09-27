//! The I/O worker (design m3 A.4): one long-lived thread for every blocking call that is not a
//! helper session — enumerating keyboards (hundreds of milliseconds), reading the journal, saving
//! settings, the RunOnce rule, opening a settings page. Tasks run in order; repeated reads that
//! queue up are merged. Results go back with `slint::invoke_from_event_loop`.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use mklm_client::startup::summarize;
use mklm_win::ui::SettingsPage;

use crate::app::post;
use crate::settings::Settings;
use crate::state::{AppMsg, SystemRead};

/// A job for the I/O worker.
#[derive(Debug, Clone, PartialEq)]
pub enum IoTask {
    /// Snapshot (display: every problem is a warning), journal, boot ID, attention summary.
    Read,
    SaveSettings {
        settings: Box<Settings>,
        dir: PathBuf,
    },
    /// `mklm_client::run_once::apply_run_once_rule(PostRebootCommand::Gui)`.
    RunOnceRule,
    OpenSettingsPage(SettingsPage),
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
        // Merge reads that queued up behind each other.
        let mut batch = vec![first];
        batch.extend(tasks.try_iter());
        let mut read_done = false;
        for task in batch {
            match task {
                IoTask::Read if read_done => {}
                IoTask::Read => {
                    read_done = true;
                    post(AppMsg::SystemRead(Box::new(read_system())));
                }
                IoTask::SaveSettings { settings, dir } => {
                    // WP-U6: report a failure to the UI (a banner) instead of dropping it.
                    let _ = settings.save(&dir);
                }
                IoTask::RunOnceRule => {
                    // WP-U4: warn in the UI on an error; `TellUser` cannot happen unelevated.
                    let _ = mklm_client::run_once::apply_run_once_rule(
                        mklm_client::run_once::PostRebootCommand::Gui,
                    );
                }
                IoTask::OpenSettingsPage(page) => {
                    let _ = mklm_win::ui::open_settings_page(page);
                }
            }
        }
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
