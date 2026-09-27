//! The journal as stored, read unelevated (design m2 C.1: Users may read it), and the live
//! [`JournalSource`] of the orchestrator.

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

/// The boot ID of this boot.
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
