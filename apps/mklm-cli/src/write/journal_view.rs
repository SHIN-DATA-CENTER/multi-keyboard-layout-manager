//! `mklm-cli journal`: the journal as read unelevated (design C.1), as text or JSON.

use std::fmt::Write as _;

use mklm_core::{
    Attention, BaselineRecord, BootId, Journal, JournalEntry, Liveness, ProcessIdentity, Timestamp,
    attention,
};
use serde::Serialize;

use super::render::{CurrentValue, entry_line, failure_text, put, records_text, value_text};
use crate::text::pending_name_long;

/// Where the journal lives, as shown to the user.
const JOURNAL_PATH: &str = r"HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal";

/// `YYYY-MM-DD HH:MM:SS UTC` of a journal timestamp (milliseconds since the Unix epoch).
pub fn timestamp_text(at: Timestamp) -> String {
    let seconds = at.0 / 1000;
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Proleptic Gregorian date of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

fn attention_text(attention: Attention) -> Option<&'static str> {
    match attention {
        Attention::None => None,
        Attention::Busy => Some("another MKLM process is working on it"),
        Attention::Recover => Some("needs recovery: run `mklm-cli recover`"),
        Attention::AwaitingUser => Some("waits for `mklm-cli keep` or `mklm-cli revert`"),
        Attention::WaitingForReboot => Some("waits for a PC restart: `mklm-cli reboot`"),
        Attention::Conflict => Some("in conflict: `mklm-cli resolve`"),
        Attention::NeedsApply => Some("not in effect yet"),
    }
}

/// The text form. `current` gives the value stored now for a record, when a snapshot was read.
pub fn journal_text(
    journal: &Journal,
    boot: Option<BootId>,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    current: Option<CurrentValue<'_>>,
) -> String {
    let mut out = String::new();
    put!(out, "Journal ({JOURNAL_PATH})");
    if journal.entries.is_empty() && journal.baselines.is_empty() && journal.unreadable.is_empty() {
        put!(out, "  Empty: MKLM has not changed anything on this PC.");
        return out;
    }
    put!(out);
    put!(out, "Operations (oldest first)");
    if journal.entries.is_empty() {
        put!(out, "  none");
    }
    for entry in &journal.entries {
        entry_text(&mut out, entry, boot, liveness, current);
    }
    put!(out);
    put!(out, "Values before MKLM (baselines)");
    if journal.baselines.is_empty() {
        put!(out, "  none");
    }
    for baseline in &journal.baselines {
        baseline_text(&mut out, baseline);
    }
    if !journal.unreadable.is_empty() {
        put!(out);
        put!(
            out,
            "Unreadable entries (nothing is written while these exist)"
        );
        for bad in &journal.unreadable {
            put!(out, "  {}: {}", bad.name, bad.error);
        }
    }
    out
}

fn entry_text(
    out: &mut String,
    entry: &JournalEntry,
    boot: Option<BootId>,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    current: Option<CurrentValue<'_>>,
) {
    put!(out, "  {}", entry_line(entry));
    put!(
        out,
        "      {}  #{}  created {}, updated {}",
        entry.op_id,
        entry.seq,
        timestamp_text(entry.created_at),
        timestamp_text(entry.updated_at)
    );
    if let Some(boot) = boot
        && let Some(text) = attention_text(attention(entry, boot, liveness(&entry.owner)))
    {
        put!(out, "      Attention: {text}");
    }
    if let Some(apply) = entry.apply {
        put!(out, "      Takes effect: {}", pending_name_long(apply));
    }
    if let Some(failure) = &entry.failure {
        put!(out, "      Reason: {}", failure_text(failure));
    }
    if let Some(pending) = &entry.apply_pending {
        put!(
            out,
            "      Not in effect yet for {}: {}",
            pending.instance_ids.join(", "),
            pending_name_long(pending.action)
        );
    }
    for line in records_text(entry, current).lines() {
        put!(out, "    {line}");
    }
}

fn baseline_text(out: &mut String, baseline: &BaselineRecord) {
    put!(
        out,
        "  {}\\{} = {}  (recorded by {} on {})",
        baseline.key_path,
        baseline.key.name,
        value_text(&baseline.value),
        baseline.captured_by.short(),
        timestamp_text(baseline.captured_at)
    );
}

/// The JSON form: the stored documents as they are, with what the unelevated check makes of each.
#[derive(Debug, Serialize)]
pub struct JournalDocument<'a> {
    pub store_version: Option<u32>,
    pub boot_id: Option<BootId>,
    pub entries: Vec<EntryDocument<'a>>,
    pub baselines: &'a [BaselineRecord],
    pub unreadable: Vec<UnreadableDocument>,
}

#[derive(Debug, Serialize)]
pub struct EntryDocument<'a> {
    #[serde(flatten)]
    pub entry: &'a JournalEntry,
    /// `mklm_core::attention` for this boot; `null` when the boot ID could not be read.
    pub attention: Option<Attention>,
}

#[derive(Debug, Serialize)]
pub struct UnreadableDocument {
    pub name: String,
    pub error: String,
}

pub fn journal_document<'a>(
    journal: &'a Journal,
    store_version: Option<u32>,
    boot: Option<BootId>,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
) -> JournalDocument<'a> {
    JournalDocument {
        store_version,
        boot_id: boot,
        entries: journal
            .entries
            .iter()
            .map(|entry| EntryDocument {
                entry,
                attention: boot.map(|boot| attention(entry, boot, liveness(&entry.owner))),
            })
            .collect(),
        baselines: &journal.baselines,
        unreadable: journal
            .unreadable
            .iter()
            .map(|bad| UnreadableDocument {
                name: bad.name.clone(),
                error: bad.error.to_string(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{
        LayoutChoice, OpId, OpKind, OpState, PendingAction, RegValue, ValueKey, ValueRecord,
        WriteTarget, value_names,
    };

    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp_text(Timestamp(0)), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            timestamp_text(Timestamp(1_790_500_004_000)),
            "2026-09-27 09:06:44 UTC"
        );
        assert_eq!(
            timestamp_text(Timestamp(951_782_400_000)),
            "2000-02-29 00:00:00 UTC"
        );
    }

    fn entry() -> JournalEntry {
        let id = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".to_string();
        let target = WriteTarget::Device {
            instance_id: id.clone(),
        };
        JournalEntry {
            schema_version: 1,
            op_id: OpId::parse("3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f").unwrap(),
            seq: 7,
            kind: OpKind::SetLayout {
                requested: id.clone(),
                instance_ids: vec![id],
                layout: LayoutChoice::Jis,
            },
            state: OpState::AwaitingConfirm,
            boot_id: BootId(1),
            owner: ProcessIdentity {
                pid: 1,
                creation_time: 1,
            },
            created_at: Timestamp(1_790_500_000_000),
            updated_at: Timestamp(1_790_500_004_000),
            apply: Some(PendingAction::Reconnect),
            countdown: None,
            records: vec![ValueRecord {
                key_path: ValueKey {
                    target: target.clone(),
                    name: value_names::HID_TYPE.into(),
                }
                .key_path(),
                target,
                name: value_names::HID_TYPE.into(),
                baseline: RegValue::Dword { value: 4 },
                before: RegValue::Dword { value: 4 },
                intended: RegValue::Dword { value: 7 },
                last_written: Some(RegValue::Dword { value: 7 }),
                conflict: None,
                resolve_to: None,
                write_error: None,
                skipped: None,
            }],
            context: Vec::new(),
            failure: None,
            revert_mode: None,
            apply_pending: None,
            history: Vec::new(),
        }
    }

    #[test]
    fn text_and_json() {
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let empty = journal_text(&Journal::default(), Some(BootId(1)), &dead, None);
        assert!(empty.contains("Empty: MKLM has not changed anything"));

        let journal = Journal {
            entries: vec![entry()],
            ..Journal::default()
        };
        let text = journal_text(&journal, Some(BootId(1)), &dead, None);
        assert!(text.contains("3f2a9c1e  set HID"), "{text}");
        assert!(text.contains("(waiting for keep or revert)"), "{text}");
        assert!(
            text.contains("Attention: waits for `mklm-cli keep`"),
            "{text}"
        );
        assert!(text.contains("[1] Enum\\HID\\"), "{text}");
        assert!(text.contains("last written 7"), "{text}");
        assert!(!text.contains("device removed"), "{text}");
        let current = |_: &ValueRecord| Some(RegValue::Dword { value: 7 });
        let text = journal_text(&journal, Some(BootId(1)), &dead, Some(&current));
        assert!(text.contains("now 7"), "{text}");

        let document = journal_document(&journal, Some(1), Some(BootId(1)), &dead);
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(
            json["entries"][0]["op_id"],
            "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"
        );
        assert_eq!(json["entries"][0]["state"], "awaiting-confirm");
        assert_eq!(json["entries"][0]["attention"], "awaiting-user");
        assert_eq!(json["store_version"], 1);
    }
}
