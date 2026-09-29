//! `mklm-cli journal`: the journal as read unelevated (design C.1), as text or JSON.
//!
//! The text shows times in local time with their offset from UTC (design m3 G.1; the M2
//! real-machine test R4 found them in UTC) and words `LiveResetUnconfirmed` by whether the reset
//! was reached (m3 G.2, R5). The JSON is for programs and keeps the stored UTC milliseconds.

use std::fmt::Write as _;

use mklm_client::describe::reset_phase;
use mklm_core::{
    Attention, BaselineRecord, BootId, Journal, JournalEntry, Liveness, PendingAction,
    ProcessIdentity, Timestamp, attention,
};
use serde::Serialize;

use super::render::{CurrentValue, entry_line, failure_text, put, records_text, value_text};
use crate::text::pending_name_long;

/// Where the journal lives, as shown to the user.
const JOURNAL_PATH: &str = r"HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal";

/// How the text form writes a journal timestamp ([`timestamp_text`]; tests pass a fixed one).
pub type TimeText<'a> = &'a dyn Fn(Timestamp) -> String;

/// `YYYY-MM-DD HH:MM:SS +HH:MM` in local time, with the time zone rules of that moment
/// (`mklm_win::time::local_time`, design m3 G.1); the UTC form of [`utc_timestamp_text`] when the
/// conversion fails.
pub fn timestamp_text(at: Timestamp) -> String {
    local_timestamp_text(at).unwrap_or_else(|| utc_timestamp_text(at))
}

#[cfg(windows)]
fn local_timestamp_text(at: Timestamp) -> Option<String> {
    mklm_win::time::local_time(at)
        .ok()
        .map(|local| local.to_iso_text())
}

#[cfg(not(windows))]
fn local_timestamp_text(_: Timestamp) -> Option<String> {
    None
}

/// `YYYY-MM-DD HH:MM:SS UTC` of a journal timestamp (milliseconds since the Unix epoch).
pub fn utc_timestamp_text(at: Timestamp) -> String {
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

/// The text form. `current` gives the value stored now for a record, when a snapshot was read;
/// `time` writes the timestamps ([`timestamp_text`]).
pub fn journal_text(
    journal: &Journal,
    boot: Option<BootId>,
    liveness: &dyn Fn(&ProcessIdentity) -> Liveness,
    current: Option<CurrentValue<'_>>,
    time: TimeText<'_>,
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
        entry_text(&mut out, entry, boot, liveness, current, time);
    }
    put!(out);
    put!(out, "Values before MKLM (baselines)");
    if journal.baselines.is_empty() {
        put!(out, "  none");
    }
    for baseline in &journal.baselines {
        baseline_text(&mut out, baseline, time);
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
    time: TimeText<'_>,
) {
    put!(out, "  {}", entry_line(entry));
    put!(
        out,
        "      {}  #{}  created {}, updated {}",
        entry.op_id,
        entry.seq,
        time(entry.created_at),
        time(entry.updated_at)
    );
    if let Some(boot) = boot
        && let Some(text) = attention_text(attention(entry, boot, liveness(&entry.owner)))
    {
        put!(out, "      Attention: {text}");
    }
    // What to do only while the entry is open; a closed one says how it applied, which asks for
    // nothing (what is still not in effect has its own lines: `Attention`, `Not in effect yet`).
    if let Some(apply) = entry.apply {
        if entry.state.is_open() {
            put!(out, "      Takes effect: {}", pending_name_long(apply));
        } else {
            put!(out, "      Applied by: {}", applied_by(apply));
        }
    }
    if let Some(failure) = &entry.failure {
        put!(
            out,
            "      Reason: {}",
            failure_text(failure, reset_phase(entry))
        );
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

/// How a closed entry's values applied, in words that ask for nothing.
fn applied_by(action: PendingAction) -> &'static str {
    match action {
        PendingAction::ResetKeyboard => "a keyboard reset",
        PendingAction::Reconnect => "reconnecting the keyboard",
        PendingAction::RestartPc => "a PC restart",
    }
}

fn baseline_text(out: &mut String, baseline: &BaselineRecord, time: TimeText<'_>) {
    put!(
        out,
        "  {}\\{} = {}  (recorded by {} on {})",
        baseline.key_path,
        baseline.key.name,
        value_text(&baseline.value),
        baseline.captured_by.short(),
        time(baseline.captured_at)
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
    fn utc_timestamps() {
        assert_eq!(utc_timestamp_text(Timestamp(0)), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            utc_timestamp_text(Timestamp(1_790_500_004_000)),
            "2026-09-27 09:06:44 UTC"
        );
        assert_eq!(
            utc_timestamp_text(Timestamp(951_782_400_000)),
            "2000-02-29 00:00:00 UTC"
        );
    }

    /// Day count since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's
    /// `days_from_civil`), to check [`civil_from_days`] and the local times independently.
    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let yoe = year - era * 400;
        let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Seconds since the Unix epoch of `YYYY-MM-DD HH:MM:SS +HH:MM`.
    fn epoch_seconds_of_local_text(text: &str) -> i64 {
        let number = |range: std::ops::Range<usize>| -> i64 { text[range].parse().unwrap() };
        let days = days_from_civil(number(0..4), number(5..7), number(8..10));
        let local = days * 86_400 + number(11..13) * 3600 + number(14..16) * 60 + number(17..19);
        let offset = number(21..23) * 3600 + number(24..26) * 60;
        match &text[20..21] {
            "+" => local - offset,
            "-" => local + offset,
            sign => panic!("unexpected offset sign {sign:?} in {text:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn journal_times_are_local_with_their_offset() {
        // Design m3 G.1 (R4): the shape `2026-09-27 22:22:05 +09:00`, whatever this PC's zone,
        // and the same instant as the stored UTC milliseconds.
        for at in [1_790_515_325_000, 1_790_515_887_721, 951_782_400_000] {
            let text = timestamp_text(Timestamp(at));
            assert_eq!(text.len(), "2026-09-27 22:22:05 +09:00".len(), "{text}");
            for (index, separator) in [
                (4, '-'),
                (7, '-'),
                (10, ' '),
                (13, ':'),
                (16, ':'),
                (19, ' '),
                (23, ':'),
            ] {
                assert_eq!(text.chars().nth(index), Some(separator), "{text}");
            }
            assert!(!text.ends_with("UTC"), "{text}");
            assert_eq!(
                epoch_seconds_of_local_text(&text),
                i64::try_from(at / 1000).unwrap(),
                "{text}"
            );
        }
        // The same days as the UTC form (checks `civil_from_days` both ways).
        for days in [0, 11_016, 20_723, 20_724, 47_540] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(
                days_from_civil(year as i64, month as i64, day as i64),
                days as i64
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_fixed_local_time_reads_as_the_journal_shows_it() {
        let local = mklm_win::time::LocalTime {
            year: 2026,
            month: 9,
            day: 27,
            hour: 22,
            minute: 22,
            second: 5,
            utc_offset_minutes: 540,
        };
        assert_eq!(local.to_iso_text(), "2026-09-27 22:22:05 +09:00");
        assert_eq!(
            epoch_seconds_of_local_text(&local.to_iso_text()),
            1_790_515_325
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
        let empty = journal_text(
            &Journal::default(),
            Some(BootId(1)),
            &dead,
            None,
            &utc_timestamp_text,
        );
        assert!(empty.contains("Empty: MKLM has not changed anything"));

        let journal = Journal {
            entries: vec![entry()],
            ..Journal::default()
        };
        let text = journal_text(&journal, Some(BootId(1)), &dead, None, &utc_timestamp_text);
        assert!(text.contains("3f2a9c1e  set HID"), "{text}");
        assert!(text.contains("(waiting for keep or revert)"), "{text}");
        assert!(
            text.contains("Attention: waits for `mklm-cli keep`"),
            "{text}"
        );
        assert!(
            text.contains("created 2026-09-27 09:06:40 UTC, updated 2026-09-27 09:06:44 UTC"),
            "{text}"
        );
        assert!(text.contains("[1] Enum\\HID\\"), "{text}");
        assert!(text.contains("last written 7"), "{text}");
        assert!(!text.contains("device removed"), "{text}");
        let current = |_: &ValueRecord| Some(RegValue::Dword { value: 7 });
        let text = journal_text(
            &journal,
            Some(BootId(1)),
            &dead,
            Some(&current),
            &|at: Timestamp| format!("<{}>", at.0),
        );
        assert!(text.contains("now 7"), "{text}");
        assert!(
            text.contains("created <1790500000000>, updated <1790500004000>"),
            "{text}"
        );

        let document = journal_document(&journal, Some(1), Some(BootId(1)), &dead);
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(
            json["entries"][0]["op_id"],
            "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"
        );
        assert_eq!(json["entries"][0]["state"], "awaiting-confirm");
        assert_eq!(json["entries"][0]["attention"], "awaiting-user");
        assert_eq!(json["store_version"], 1);
        // Machine-readable: the stored UTC milliseconds, not the local text (design m3 G.1).
        assert_eq!(json["entries"][0]["created_at"], 1_790_500_000_000_u64);
    }

    /// The schema-1 entry of the M2 real-machine test R5, as an M2 build stored it (helper killed
    /// after its first write, before the keyboard reset; recovery rolled it back).
    const R5_ENTRY: &str = r#"{"schema_version":1,"op_id":"8e9a9970-f7bf-46c7-b779-f914f17bd40d","seq":7,"kind":{"kind":"set-layout","requested":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","instance_ids":["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],"layout":"jis"},"state":"reverted","boot_id":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","owner":{"pid":4668,"creation_time":134349894905676290},"created_at":1790515887721,"updated_at":1790516010637,"apply":"reset-keyboard","countdown":null,"records":[{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardTypeOverride","baseline":{"kind":"dword","value":4},"before":{"kind":"dword","value":4},"intended":{"kind":"dword","value":7},"last_written":{"kind":"dword","value":4},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null},{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","name":"KeyboardSubtypeOverride","baseline":{"kind":"dword","value":0},"before":{"kind":"dword","value":0},"intended":{"kind":"dword","value":2},"last_written":{"kind":"dword","value":0},"conflict":null,"resolve_to":null,"write_error":null,"skipped":null}],"context":[],"failure":{"kind":"live-reset-unconfirmed"},"revert_mode":null,"apply_pending":null,"history":[{"from":null,"to":"planned","at":1790515887721,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":20644,"creation_time":134349894876176557},"reason":"set-layout","boot_time_hint":134349552405000000},{"from":"planned","to":"revert-pending","at":1790515890626,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4668,"creation_time":134349894905676290},"reason":"recover:roll-back","boot_time_hint":134349552405000000},{"from":"revert-pending","to":"reverted","at":1790516010637,"boot":"4c703377-b861-11f1-a1dd-d9f1d0b3ec70","by":{"pid":4668,"creation_time":134349894905676290},"reason":"recover:roll-back","boot_time_hint":134349552405000000}]}"#;

    /// A schema-1 baseline stored by the same M2 build.
    const M2_BASELINE: (&str, &str) = (
        r"device|HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000|KeyboardTypeOverride",
        r#"{"schema_version":1,"key":{"target":{"kind":"device","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"},"name":"KeyboardTypeOverride"},"key_path":"Enum\\HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000\\Device Parameters","value":{"kind":"dword","value":4},"captured_at":1790515162813,"captured_by":"bfaca7cd-fdef-4dd0-8d75-6f311d32bc37"}"#,
    );

    fn m2_journal() -> Journal {
        Journal::parse(
            &[(
                "8e9a9970-f7bf-46c7-b779-f914f17bd40d".to_string(),
                R5_ENTRY.to_string(),
            )],
            &[(M2_BASELINE.0.to_string(), M2_BASELINE.1.to_string())],
        )
    }

    #[test]
    fn schema_1_entries_and_baselines_from_m2_are_shown() {
        let journal = m2_journal();
        assert!(journal.unreadable.is_empty(), "{:?}", journal.unreadable);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.baselines.len(), 1);
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let text = journal_text(&journal, Some(BootId(1)), &dead, None, &utc_timestamp_text);
        assert!(text.contains("8e9a9970  set HID"), "{text}");
        assert!(text.contains("(reverted)"), "{text}");
        assert!(
            text.contains("created 2026-09-27 13:31:27 UTC, updated 2026-09-27 13:33:30 UTC"),
            "{text}"
        );
        // Design m3 G.2 (R5): the helper stopped before the reset.
        assert!(
            text.contains(
                "Reason: the writer stopped before the keyboard reset; recovery put the values \
                 back (the keyboard never switched)"
            ),
            "{text}"
        );
        assert!(!text.contains("after the keyboard reset"), "{text}");
        assert!(
            text.contains(
                "KeyboardTypeOverride = 4  (recorded by bfaca7cd on 2026-09-27 13:19:22 UTC)"
            ),
            "{text}"
        );

        // The same entry had it reached the reset: the M2 wording.
        let mut reached = journal.entries[0].clone();
        let mut restarting = reached.history[0].clone();
        restarting.from = Some(OpState::Written);
        restarting.to = OpState::Restarting;
        reached.history.insert(1, restarting);
        let journal = Journal {
            entries: vec![reached],
            ..Journal::default()
        };
        let text = journal_text(&journal, None, &dead, None, &utc_timestamp_text);
        assert!(
            text.contains(
                "Reason: the change was never kept after the keyboard reset; recovery put it back"
            ),
            "{text}"
        );

        // The JSON keeps the stored documents as they are.
        let journal = m2_journal();
        let document = journal_document(&journal, Some(1), None, &dead);
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["entries"][0]["schema_version"], 1);
        assert_eq!(
            json["entries"][0]["failure"]["kind"],
            "live-reset-unconfirmed"
        );
        assert_eq!(json["entries"][0]["updated_at"], 1_790_516_010_637_u64);
        assert_eq!(json["baselines"][0]["captured_at"], 1_790_515_162_813_u64);
    }

    /// CLEAN-RUN-4 (after MT-1 on the desktop PC): a closed entry that applied at a restart no
    /// longer reads as an instruction to restart; an open one still does.
    #[test]
    fn only_an_open_entry_says_what_to_do_for_it_to_take_effect() {
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let text_of = |state: OpState| {
            let mut entry = entry();
            entry.state = state;
            entry.apply = Some(PendingAction::RestartPc);
            let journal = Journal {
                entries: vec![entry],
                ..Journal::default()
            };
            journal_text(&journal, Some(BootId(2)), &dead, None, &utc_timestamp_text)
        };
        for closed in [
            OpState::Reverted,
            OpState::Confirmed,
            OpState::Failed,
            OpState::RevertedPendingReboot,
        ] {
            let text = text_of(closed);
            assert!(text.contains("      Applied by: a PC restart\n"), "{text}");
            assert!(!text.contains("Takes effect"), "{text}");
            assert!(!text.contains("restart the PC"), "{text}");
        }
        let text = text_of(OpState::Reverted);
        assert!(text.contains("(reverted)"), "{text}");
        assert!(!text.contains("Attention"), "{text}");
        let text = text_of(OpState::PendingReboot);
        assert!(
            text.contains("      Takes effect: restart the PC (Restart, not Shut down)\n"),
            "{text}"
        );
        assert!(!text.contains("Applied by"), "{text}");
        assert_eq!(applied_by(PendingAction::ResetKeyboard), "a keyboard reset");
        assert_eq!(
            applied_by(PendingAction::Reconnect),
            "reconnecting the keyboard"
        );
    }

    /// The journal 0.1.0 left on the desktop PC of the boot-ID bug, adopted as `read_journal`
    /// does. The JSON's `boot_id` is this boot's counter form; an entry's `boot_id` is the
    /// adopted one when it is of this boot, and the stored GUID otherwise (history lines keep it).
    #[test]
    fn the_legacy_journal_shows_the_counter_form_boot() {
        use mklm_core::fixtures;

        let (ops, baselines) = fixtures::legacy_guid_journal();
        let dead = |_: &ProcessIdentity| Liveness::Dead;
        let document_of = |boot_time| {
            let current = fixtures::legacy_pc_boot(boot_time);
            let journal =
                mklm_client::journal::parse_journal(&ops, &baselines, Some(1), Some(&current))
                    .journal;
            let text = journal_text(&journal, Some(current.id), &dead, None, &utc_timestamp_text);
            let document = journal_document(&journal, Some(1), Some(current.id), &dead);
            (serde_json::to_value(&document).unwrap(), text)
        };
        let counter = "00000007-0000-8000-8000-000000000000";
        let guid = "9845bda6-baa7-11f1-adca-ca988d513a4f";

        let (json, text) = document_of(fixtures::LEGACY_PC_BOOT_TIME_AFTER_RESTART);
        assert_eq!(json["boot_id"], counter);
        assert_eq!(json["entries"][0]["op_id"], fixtures::LEGACY_REVERTED_OP);
        assert_eq!(json["entries"][0]["attention"], "none");
        assert_eq!(json["entries"][0]["boot_id"], guid);
        assert_eq!(json["entries"][1]["op_id"], fixtures::LEGACY_PENDING_OP);
        assert_eq!(json["entries"][1]["attention"], "recover");
        assert_eq!(json["entries"][1]["boot_id"], guid);
        assert_eq!(json["entries"][1]["apply_pending"]["since"], guid);
        assert!(!text.contains("waits for a PC restart"), "{text}");

        let (json, text) = document_of(fixtures::LEGACY_PC_BOOT_TIME_OF_WRITES);
        assert_eq!(json["boot_id"], counter);
        assert_eq!(json["entries"][0]["attention"], "needs-apply");
        assert_eq!(json["entries"][0]["boot_id"], counter);
        assert_eq!(json["entries"][1]["attention"], "waiting-for-reboot");
        assert_eq!(json["entries"][1]["boot_id"], counter);
        assert_eq!(json["entries"][1]["apply_pending"]["since"], counter);
        assert_eq!(json["entries"][1]["history"][0]["boot"], guid);
        assert!(text.contains("Attention: waits for a PC restart"), "{text}");
    }
}
