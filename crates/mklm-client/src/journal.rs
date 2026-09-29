//! The journal as the front ends read it: [`parse_journal`] turns the stored documents into a
//! [`JournalRead`] whose boot IDs are judged against the current boot, and (Windows only)
//! [`read_journal`] reads them unelevated (design m2 C.1: Users may read it); also the live
//! [`JournalSource`] of the orchestrator.

#[cfg(windows)]
use mklm_core::{Attention, BootId, Liveness, ProcessIdentity, attention};
use mklm_core::{
    CurrentBoot, Journal, JournalError, STORE_VERSION, STORE_VERSION_VALUE, UnreadableEntry,
};
#[cfg(windows)]
use mklm_win::{journal_store, proc_identity, session};

#[cfg(windows)]
use crate::orchestrator::JournalSource;

/// The parsed journal and the stored `StoreVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRead {
    pub journal: Journal,
    pub store_version: Option<u32>,
}

/// The stored documents (`(value name, JSON)` of `Ops` and `Baselines`, and `StoreVersion`) as
/// every front end must see them: parsed, with a newer store layout counted as unreadable (as the
/// engine does), and, when `current` is known, with the recorded boot IDs judged against it in
/// memory as the engine judges them when it opens a session
/// (`mklm_core::Journal::adopt_current_boot`): those judged to be this boot (0.1.x's loader GUIDs
/// by the boot time of their history, counter ids by the safety net) become `current.id`, every
/// other one reads as an earlier boot. So every caller's `entry.boot_id == boot` means "written
/// in this boot" for either form, and `mklm-cli journal --json` shows the adopted id (the history
/// lines keep the stored one). Without `current` nothing is adopted.
pub fn parse_journal(
    ops: &[(String, String)],
    baselines: &[(String, String)],
    store_version: Option<u32>,
    current: Option<&CurrentBoot>,
) -> JournalRead {
    let mut journal = Journal::parse(ops, baselines);
    if let Some(found) = store_version.filter(|version| *version > STORE_VERSION) {
        journal.unreadable.push(UnreadableEntry {
            name: STORE_VERSION_VALUE.to_string(),
            error: JournalError::NewerSchema {
                found,
                supported: STORE_VERSION,
            },
        });
    }
    if let Some(current) = current {
        journal.adopt_current_boot(current);
    }
    JournalRead {
        journal,
        store_version,
    }
}

/// Reads the journal unelevated and judges it against this boot ([`parse_journal`] with
/// `mklm_win::session::current_boot`). When the boot cannot be read nothing is adopted; the
/// callers then fail on [`boot_id`] anyway.
#[cfg(windows)]
pub fn read_journal() -> Result<JournalRead, mklm_win::Error> {
    let raw = journal_store::read_journal_store()?;
    let current = session::current_boot().ok();
    Ok(parse_journal(
        &raw.ops,
        &raw.baselines,
        raw.store_version,
        current.as_ref(),
    ))
}

/// Whether an entry's owner still runs (`NtQuerySystemInformation`, no `OpenProcess`, design m2
/// S3).
#[cfg(windows)]
pub fn liveness(process: &ProcessIdentity) -> Liveness {
    proc_identity::process_liveness(process)
}

/// The boot ID of this boot (the counter form of `KUSER_SHARED_DATA.BootId`).
#[cfg(windows)]
pub fn boot_id() -> Result<BootId, mklm_win::Error> {
    session::boot_id()
}

/// The live journal: [`JournalSource::needs_recovery`] reads it now.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveJournal;

#[cfg(windows)]
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

#[cfg(test)]
mod tests {
    use mklm_core::fixtures::{
        LEGACY_PC_BOOT_COUNTER, LEGACY_PC_BOOT_TIME_AFTER_RESTART, LEGACY_PC_BOOT_TIME_OF_WRITES,
        LEGACY_PC_GUID, LEGACY_PENDING_OP, LEGACY_REVERTED_OP, legacy_guid_journal, legacy_pc_boot,
    };
    use mklm_core::{Attention, BootId, JournalEntry, Liveness, OpId, attention};

    use super::*;

    fn read(boot_time: u64) -> JournalRead {
        let (ops, baselines) = legacy_guid_journal();
        parse_journal(
            &ops,
            &baselines,
            Some(STORE_VERSION),
            Some(&legacy_pc_boot(boot_time)),
        )
    }

    fn entry<'a>(read: &'a JournalRead, op: &str) -> &'a JournalEntry {
        read.journal
            .entry(&OpId::parse(op).unwrap())
            .unwrap_or_else(|| panic!("no {op}"))
    }

    fn since(entry: &JournalEntry) -> Option<BootId> {
        entry.apply_pending.as_ref().map(|pending| pending.since)
    }

    /// The reported case (docs/research/boot-id.md): read after the restart, both entries of
    /// 0.1.0 are of an earlier boot. Their GUIDs stay, and the pending migration needs the
    /// post-reboot check (Keep), not a restart.
    #[test]
    fn the_legacy_journal_after_the_restart() {
        let read = read(LEGACY_PC_BOOT_TIME_AFTER_RESTART);
        let boot = BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER);
        assert!(read.journal.unreadable.is_empty());
        assert_eq!(read.store_version, Some(STORE_VERSION));
        let (ops, baselines) = legacy_guid_journal();
        assert_eq!(
            read.journal,
            Journal::parse(&ops, &baselines),
            "nothing adopted"
        );
        let pending = entry(&read, LEGACY_PENDING_OP);
        let reverted = entry(&read, LEGACY_REVERTED_OP);
        assert_eq!(pending.boot_id, LEGACY_PC_GUID);
        assert_eq!(since(pending), Some(LEGACY_PC_GUID));
        assert_eq!(reverted.boot_id, LEGACY_PC_GUID);
        assert_eq!(attention(pending, boot, Liveness::Dead), Attention::Recover);
        assert_eq!(attention(reverted, boot, Liveness::Dead), Attention::None);
    }

    /// The same journal in the boot of its writes (the fix installed before the restart): both
    /// entries are adopted to this boot's counter id and wait for the restart. Without this
    /// adoption the front ends would offer Keep before the restart.
    #[test]
    fn the_legacy_journal_in_the_boot_of_its_writes() {
        let read = read(LEGACY_PC_BOOT_TIME_OF_WRITES);
        let boot = BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER);
        let pending = entry(&read, LEGACY_PENDING_OP);
        let reverted = entry(&read, LEGACY_REVERTED_OP);
        assert_eq!(pending.boot_id, boot);
        assert_eq!(since(pending), Some(boot));
        assert_eq!(reverted.boot_id, boot);
        assert!(
            pending
                .history
                .iter()
                .chain(&reverted.history)
                .all(|line| line.boot == LEGACY_PC_GUID),
            "history lines keep the stored id"
        );
        assert_eq!(
            attention(pending, boot, Liveness::Dead),
            Attention::WaitingForReboot
        );
        assert_eq!(
            attention(reverted, boot, Liveness::Dead),
            Attention::NeedsApply
        );
    }

    /// The safety net of the counter reaches the front ends too: the same journal written by
    /// 0.1.1 under counter 6, read in a boot whose counter is 7. With the boot time of the writes
    /// (the counter moved without a new boot) the migration still waits for the restart; with
    /// another boot time it was restarted.
    #[test]
    fn a_counter_id_with_this_boot_time_waits_for_the_restart() {
        let (ops, baselines) = legacy_guid_journal();
        let written = BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER - 1);
        let ops: Vec<(String, String)> = ops
            .into_iter()
            .map(|(name, json)| {
                let json = json.replace(&LEGACY_PC_GUID.to_text(), &written.to_text());
                (name, json)
            })
            .collect();
        let boot = BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER);
        let read = |boot_time| {
            parse_journal(
                &ops,
                &baselines,
                Some(STORE_VERSION),
                Some(&legacy_pc_boot(boot_time)),
            )
        };

        let same = read(LEGACY_PC_BOOT_TIME_OF_WRITES);
        let pending = entry(&same, LEGACY_PENDING_OP);
        assert!(pending.history.iter().all(|line| line.boot == written));
        assert_eq!(pending.boot_id, boot);
        assert_eq!(since(pending), Some(boot));
        assert_eq!(
            attention(pending, boot, Liveness::Dead),
            Attention::WaitingForReboot
        );

        let restarted = read(LEGACY_PC_BOOT_TIME_AFTER_RESTART);
        let pending = entry(&restarted, LEGACY_PENDING_OP);
        assert_eq!(pending.boot_id, written);
        assert_eq!(attention(pending, boot, Liveness::Dead), Attention::Recover);
    }

    /// Without the current boot nothing is adopted.
    #[test]
    fn without_the_current_boot_nothing_is_adopted() {
        let (ops, baselines) = legacy_guid_journal();
        let read = parse_journal(&ops, &baselines, Some(STORE_VERSION), None);
        assert_eq!(read.journal, Journal::parse(&ops, &baselines));
        assert_eq!(read.store_version, Some(STORE_VERSION));
    }

    /// A newer `StoreVersion` makes the journal unreadable (writes stop, "update MKLM"), and is
    /// reported as it was stored; an older or missing one does not.
    #[test]
    fn a_newer_store_version_is_unreadable() {
        let (ops, baselines) = legacy_guid_journal();
        let current = legacy_pc_boot(LEGACY_PC_BOOT_TIME_OF_WRITES);
        let newer = parse_journal(&ops, &baselines, Some(STORE_VERSION + 1), Some(&current));
        assert_eq!(newer.store_version, Some(STORE_VERSION + 1));
        assert_eq!(
            newer.journal.unreadable,
            vec![UnreadableEntry {
                name: STORE_VERSION_VALUE.to_string(),
                error: JournalError::NewerSchema {
                    found: STORE_VERSION + 1,
                    supported: STORE_VERSION,
                },
            }]
        );
        // The entries are still judged (the front ends show them).
        assert_eq!(
            entry(&newer, LEGACY_PENDING_OP).boot_id,
            BootId::from_boot_counter(LEGACY_PC_BOOT_COUNTER)
        );
        for version in [None, Some(STORE_VERSION)] {
            let read = parse_journal(&ops, &baselines, version, Some(&current));
            assert!(read.journal.unreadable.is_empty(), "{version:?}");
            assert_eq!(read.store_version, version);
        }
    }
}
