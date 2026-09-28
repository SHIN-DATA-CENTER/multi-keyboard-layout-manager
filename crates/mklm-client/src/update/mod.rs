//! Updates as the GUI and the CLI see them (design m5b H.4, WP-C): the environment
//! ([`env`]), the user's cache and record ([`cache`]), the check ([`check`]) and its failure
//! classes ([`classify`]), the download ([`download`]), the update session with the helper
//! ([`stage`]), the `RecordTrust` report ([`trust_report`]) and the machine's records
//! ([`status`]).

pub mod cache;
pub mod check;
pub mod classify;
pub mod download;
#[cfg(windows)]
pub mod env;
pub mod stage;
#[cfg(windows)]
pub mod status;
pub mod trust_report;
