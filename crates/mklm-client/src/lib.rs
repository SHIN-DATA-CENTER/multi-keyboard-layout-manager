//! The caller's side of MKLM's write path, shared by the GUI (`apps/mklm`) and the CLI
//! (`apps/mklm-cli`). See docs/design/m3-gui.md section A.2.
//!
//! Everything here is UI-agnostic: it returns typed data and never text meant for the user (the
//! only strings are English diagnostics for logs and the CLI, e.g. [`LaunchError`]'s `message`).
//! The GUI localizes the typed values; the CLI renders them in English.
//!
//! - [`session`]: one request over a helper link. [`session::relay`] drives any [`session::Link`]
//!   and any [`session::Frontend`] (CLI text and stdin, GUI dialogs and buttons); it tracks what
//!   the helper reported in a [`session::SessionView`] and handles cancellation.
//! - [`orchestrator`]: a whole request as the user sees it: launch, relay, and the immediate
//!   recovery when the helper is lost after journaling (design m2 E.7, review C8).
//! - [`outcome`]: how a result, an error or a lost helper is classified ([`OutcomeClass`]); the
//!   CLI maps the classes onto exit codes, the GUI onto result screens.
//! - [`gate`]: journal rules taken unelevated: what blocks a request, whether the post-reboot
//!   check must be registered, which entries need a restart or the post-reboot check.
//! - [`startup`]: the start-up "attention" summary (recover, busy, waiting for the user ...).
//! - [`preview`]: the unelevated plan of a request (expected plan, restore and undo previews,
//!   the post-reboot check rows), as data.
//! - [`values`]: stored values as the unelevated snapshot reads them.
//! - [`describe`]: facts the front ends need to word a result (e.g. whether a reset happened).
//! - Windows only: [`launch`] (helper start and handshake), [`journal`] (unelevated journal
//!   read), [`inventory`] (the keyboards a request plans with), [`run_once`] (the post-reboot
//!   RunOnce rule, design m2 F.4 / review C17).
//! - [`update`] (M5b, docs/design/m5b-updater.md H.4): check, download and stage updates, the
//!   user's update cache, and the machine's update records as the front ends show them.
//!
//! `mklm-client` never elevates and never writes the machine's keyboard settings: those writes
//! happen in `mklm-helper.exe`. Its only write is the per-user RunOnce value ([`run_once`]),
//! through `mklm_win::session`.

#![forbid(unsafe_code)]

pub mod describe;
pub mod gate;
#[cfg(windows)]
pub mod inventory;
#[cfg(windows)]
pub mod journal;
#[cfg(windows)]
pub mod launch;
pub mod orchestrator;
pub mod outcome;
pub mod preview;
#[cfg(windows)]
pub mod run_once;
pub mod session;
pub mod startup;
pub mod update;
pub mod values;

pub use orchestrator::{LaunchError, LaunchFailure, RecoverySkip};
pub use outcome::{HelperExit, OutcomeClass};
