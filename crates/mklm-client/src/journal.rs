//! The journal as stored, read unelevated (design m2 C.1: Users may read it), with the boot IDs of
//! 0.1.x judged against this boot ([`read_journal`]), and the live [`JournalSource`] of the
//! orchestrator.

use mklm_core::{
    Attention, BootId, Journal, JournalError, Liveness, ProcessIdentity, STORE_VERSION,
    STORE_VERSION_VALUE, UnreadableEntry, attention,
};
use mklm_win::{journal_store, proc_identity, session};

use crate::orchestrator::JournalSource;

/// The parsed journal and the stored `StoreVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRead {
    pub journal: Journal,
    pub store_version: Option<u32>,
}

/// Reads the journal unelevated, counting a newer store layout as unreadable (as the engine does).
///
/// The boot IDs that 0.1.x recorded (the loader GUID) are judged here, in memory, as the engine
/// judges them when it opens a session (`mklm_core::Journal::adopt_legacy_boots`): those of this
/// boot become this boot's [`boot_id`], every other one reads as an earlier boot. So every
/// caller's `entry.boot_id == boot` means "written in this boot" for either form, and
/// `mklm-cli journal --json` shows the adopted id (the history lines keep the stored one). When
/// the boot cannot be read nothing is adopted; the callers then fail on [`boot_id`] anyway.
pub fn read_journal() -> Result<JournalRead, mklm_win::Error> {
    let raw = journal_store::read_journal_store()?;
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
    if let Ok(current) = session::current_boot() {
        journal.adopt_legacy_boots(&current);
    }
    Ok(JournalRead {
        journal,
        store_version: raw.store_version,
    })
}

/// Whether an entry's owner still runs (`NtQuerySystemInformation`, no `OpenProcess`, design m2
/// S3).
pub fn liveness(process: &ProcessIdentity) -> Liveness {
    proc_identity::process_liveness(process)
}

/// The boot ID of this boot (the counter form of `KUSER_SHARED_DATA.BootId`).
pub fn boot_id() -> Result<BootId, mklm_win::Error> {
    session::boot_id()
}

/// The live journal: [`JournalSource::needs_recovery`] reads it now.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveJournal;

impl JournalSource for LiveJournal {
    fn needs_recovery(&mut self) -> Result<bool, String> {
        let read =
            read_journal().map_err(|error| format!("reading the journal failed: {error}"))?;
        let boot = boot_id().map_err(|error| format!("reading the boot ID failed: {error}"))?;
        Ok(read
            .journal
            .entries
            .iter()
            .any(|entry| attention(entry, boot, liveness(&entry.owner)) == Attention::Recover))
    }
}
