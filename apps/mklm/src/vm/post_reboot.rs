//! The check after a restart (plan 3.6; design m2 D.7, m3 B.9): per keyboard the layout its
//! stored values predict, what Windows reports, and what typing showed — in layout names, the
//! numbers under "技術的な詳細" (review U5). The migration note, the Shift+2 test. Never reverts
//! by itself.
//!
//! The "typed" column is the key test on that row's keyboard, judged against the row's expected
//! layout (review U2): a keyboard that types the other layout gets "⚠" and a warning that
//! recommends reverting, but "このままにする" stays possible — the user decides.

use mklm_client::preview::CheckRow;
use mklm_core::{BootId, JournalEntry, LayoutChoice, LayoutTable, OpKind, OpState};

use super::journal::{NameOf, kind_text};
use super::keytest::SCANCODE_DIGIT2;
use super::{SnapshotText, Tone};
use crate::i18n::{Lang, journal_pages as text};

/// What typing on a keyboard showed during this check (review U2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Typed {
    /// Not tried yet: "未".
    #[default]
    NotYet,
    /// Shift+2 gave the expected character: "✓".
    AsExpected,
    /// Shift+2 gave the other layout's character: "⚠" (keep stays possible, with a warning).
    Differs,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckLine {
    /// The keyboard's instance ID (the row's key).
    pub instance_id: String,
    pub name: String,
    /// The layout the change set: "JIS" / "US" / "標準（JIS）".
    pub setting: String,
    /// What Windows reports, in words, with ✓ / "違います" / "未接続".
    pub recognized: String,
    pub tone: Tone,
    /// "未" / "✓" / "⚠".
    pub typed: String,
    pub typed_tone: Tone,
    /// "0x7/0x2 → 0x7/0x2" for the technical details.
    pub details: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PostReboot {
    /// The entry the page asks about (full operation ID); empty when nothing waits.
    pub op_id: String,
    pub operation: String,
    /// `PendingReboot` of this boot: "not in effect yet: Restart, not Shut down".
    pub not_restarted: bool,
    pub rows: Vec<CheckLine>,
    /// For a migration: Windows cannot show it (the built-in keyboard reports 7/2 either way);
    /// the typing test decides (design m2 0.1).
    pub migration_note: String,
    /// A changed keyboard typed the other layout: "このままにする" stays enabled, with this.
    pub keep_warning: String,
    /// "元に戻す", or "元に戻す（もう一度 PC の再起動が必要）" when the revert preview ends in a
    /// restart (a migration or a built-in keyboard, review U15 c).
    pub revert_text: String,
    /// "このままにする" may be pressed (not before the restart).
    pub can_keep: bool,
    /// The line about the Windows prompt that keep and revert bring up (design m3 B.5).
    pub uac_line: String,
    /// Nothing waits for the check: "再起動の後に確認する変更はありません。".
    pub empty_note: String,
}

/// The layout a row's keyboard should type (`None` when it cannot be told: another layer driver,
/// or no stored values).
pub fn expected_table(row: &CheckRow) -> Option<&LayoutTable> {
    row.layout
        .as_ref()
        .filter(|table| matches!(table, LayoutTable::Jis | LayoutTable::Us))
}

/// The verdict of one key press for a row whose keyboard should type `expected` (review U2):
/// only Shift+2 counts ("@" = US, "\"" = JIS), and only with the Japanese layout active (with
/// English (US) active every keyboard types "@"). `None` when the press tells nothing.
pub fn typed_for(
    text: &str,
    shift: bool,
    scancode: u32,
    active_hkl: u32,
    expected: Option<&LayoutTable>,
) -> Option<Typed> {
    if !shift || scancode != SCANCODE_DIGIT2 || !mklm_core::hkl_has_japanese_layout(active_hkl) {
        return None;
    }
    let typed = match text {
        "@" => LayoutTable::Us,
        "\"" => LayoutTable::Jis,
        _ => return None,
    };
    let expected = expected?;
    Some(if *expected == typed {
        Typed::AsExpected
    } else {
        Typed::Differs
    })
}

/// True when the entry assigns "標準に従う" to `instance_id`.
fn follows_standard(entry: &JournalEntry, instance_id: &str) -> bool {
    let same = |id: &String| id.eq_ignore_ascii_case(instance_id);
    match &entry.kind {
        OpKind::SetLayout {
            instance_ids,
            layout: LayoutChoice::Standard,
            ..
        } => instance_ids.iter().any(same),
        OpKind::Migrate { assignments, .. }
        | OpKind::SetStandard {
            keyboards: assignments,
            ..
        } => assignments
            .iter()
            .any(|(id, choice)| same(id) && *choice == LayoutChoice::Standard),
        _ => false,
    }
}

fn line(entry: &JournalEntry, row: &CheckRow, typed: Typed, lang: Lang) -> CheckLine {
    let (recognition, tone) = match row.reported {
        None if !row.present => (text::Recognition::NotConnected, Tone::Neutral),
        None if row.remote => (text::Recognition::RemoteNotVisible, Tone::Neutral),
        None => (text::Recognition::CannotCheck, Tone::Neutral),
        Some(reported) if row.matches() => (text::Recognition::Matches(reported), Tone::Success),
        Some(reported) => (text::Recognition::Differs(reported), Tone::Warning),
    };
    let (typed_text, typed_tone) = match typed {
        Typed::NotYet => (text::check_typed(None, lang), Tone::Neutral),
        Typed::AsExpected => (text::check_typed(Some(true), lang), Tone::Success),
        Typed::Differs => (text::check_typed(Some(false), lang), Tone::Warning),
    };
    CheckLine {
        instance_id: row.instance_id.clone(),
        name: row.name.clone(),
        setting: text::check_setting(
            row.layout.as_ref(),
            follows_standard(entry, &row.instance_id),
            lang,
        ),
        recognized: text::check_recognition(recognition, lang),
        tone,
        typed: typed_text,
        typed_tone,
        details: text::check_details(row.expected, row.reported, lang),
    }
}

/// The check of `entry` (the oldest of `mklm_client::gate::post_reboot_entries`). `rows` from
/// `mklm_client::preview::check_rows`; `typed` holds the key tests of this check per instance ID.
pub fn post_reboot(
    entry: &JournalEntry,
    rows: &[CheckRow],
    typed: &[(String, Typed)],
    boot: BootId,
    name_of: NameOf<'_>,
    lang: Lang,
) -> PostReboot {
    let typed_of = |id: &str| {
        typed
            .iter()
            .find(|(typed_id, _)| typed_id.eq_ignore_ascii_case(id))
            .map_or(Typed::NotYet, |(_, typed)| *typed)
    };
    let lines: Vec<CheckLine> = mklm_client::preview::group_check_rows(rows)
        .into_iter()
        .map(|group| {
            let selected = group
                .rows
                .iter()
                .find(|r| r.reported.is_some() && !r.matches())
                .or_else(|| group.rows.iter().find(|r| r.not_reported()))
                .unwrap_or(&group.rows[0]);
            let typed = if group
                .rows
                .iter()
                .any(|r| typed_of(&r.instance_id) == Typed::Differs)
            {
                Typed::Differs
            } else if group
                .rows
                .iter()
                .any(|r| typed_of(&r.instance_id) == Typed::AsExpected)
            {
                Typed::AsExpected
            } else {
                Typed::NotYet
            };
            let mut line = line(entry, selected, typed, lang);
            line.name = group.name;
            if group.rows.iter().any(|r| r.layout != selected.layout) {
                line.setting = if lang == Lang::Ja {
                    "配列が混在しています"
                } else {
                    "Mixed layouts"
                }
                .into();
            }
            line
        })
        .collect();
    let differs = rows
        .iter()
        .find(|row| typed_of(&row.instance_id) == Typed::Differs);
    let not_restarted = entry.state == OpState::PendingReboot && entry.boot_id == boot;
    PostReboot {
        op_id: entry.op_id.to_string(),
        operation: kind_text(&entry.kind, name_of, lang),
        not_restarted,
        rows: lines,
        migration_note: if matches!(entry.kind, OpKind::SetStandard { .. }) {
            if rows.is_empty() {
                if lang == Lang::Ja { "この一覧で確認するキーボードはありません。今の配列を維持するために割り当てたキーボードは一覧に含めていません。標準配列に従う入力とリモート デスクトップの配列は別途確認してください。" }
                else { "There are no keyboards to check in this list. Assignments that preserve the current layout are omitted. Check input following the standard and the Remote Desktop session layout separately." }
            } else if lang == Lang::Ja { "配列が変わるキーボードを表示しています。接続先の PC の前で Shift+2 を打って確かめてください。" }
            else { "These keyboards change layout. Test Shift+2 at the PC they are attached to." }.into()
        } else if matches!(entry.kind, OpKind::Migrate { .. }) {
            text::check_migration_note(lang)
        } else {
            String::new()
        },
        keep_warning: differs
            .map(|row| text::check_keep_warning(&row.name, lang))
            .unwrap_or_default(),
        revert_text: text::check_revert(entry.touches_boot_time_values(), lang),
        can_keep: !not_restarted,
        uac_line: crate::i18n::uac_line(lang),
        empty_note: String::new(),
    }
}

/// The page when nothing waits for the check (it was decided meanwhile, or from the CLI).
pub fn nothing(lang: Lang) -> PostReboot {
    PostReboot {
        empty_note: text::check_nothing(lang),
        revert_text: text::check_revert(false, lang),
        ..PostReboot::default()
    }
}

impl SnapshotText for PostReboot {
    fn snapshot_text(&self) -> String {
        let mut out = format!("operation: {}\n", self.operation);
        if self.not_restarted {
            out.push_str("not restarted\n");
        }
        for row in &self.rows {
            out.push_str(&format!(
                "row: {} | {} | {} ({:?}) | {} ({:?}) | {}\n",
                row.name,
                row.setting,
                row.recognized,
                row.tone,
                row.typed,
                row.typed_tone,
                row.details
            ));
        }
        for (label, value) in [
            ("note", &self.migration_note),
            ("warning", &self.keep_warning),
            ("empty", &self.empty_note),
        ] {
            if !value.is_empty() {
                out.push_str(&format!("{label}: {value}\n"));
            }
        }
        out.push_str(&format!(
            "buttons: [{}] keep={}\n",
            self.revert_text, self.can_keep
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::preview::check_rows;
    use mklm_core::{KeyboardType, Layout, PendingAction, RegValue, WriteTarget, fixtures};

    use super::*;
    use crate::vm::test_journal::{BOOT, BUILT_IN, KEYCHRON, LATER_BOOT, entry, hid_record};

    const JAPANESE: u32 = 0x0411_0411;

    #[test]
    fn grouped_collections_prioritize_disagreement_and_unobserved_members() {
        let (change, snapshot) = keychron_to_jis();
        let mut rows = check_rows(&snapshot, &change);
        let mut other = rows[0].clone();
        other.instance_id = "another-collection".into();
        other.reported = Some(KeyboardType::US);
        rows.push(other);
        let page = post_reboot(&change, &rows, &[], LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].tone, Tone::Warning);
        rows[1].reported = None;
        rows[1].remote = true;
        let page = post_reboot(&change, &rows, &[], LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(page.rows.len(), 1);
        assert_ne!(page.rows[0].tone, Tone::Success);
        assert!(
            page.rows[0]
                .recognized
                .contains("セッションからは見えません")
        );
        let typed = [
            (rows[0].instance_id.clone(), Typed::AsExpected),
            (rows[1].instance_id.clone(), Typed::Differs),
        ];
        let page = post_reboot(&change, &rows, &typed, LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(page.rows[0].typed_tone, Tone::Warning);
        assert!(!page.keep_warning.is_empty());
    }

    fn name_of(id: &str) -> String {
        if id.eq_ignore_ascii_case(KEYCHRON) {
            "Keychron Receiver".into()
        } else {
            id.into()
        }
    }

    /// Keychron to JIS through the restart, as the snapshot reads it after the restart.
    fn keychron_to_jis() -> (JournalEntry, mklm_core::SystemSnapshot) {
        let mut change = entry(
            "8a8a8a8a-0000-4000-8000-00000000000e",
            14,
            OpState::PendingReboot,
        );
        change.apply = Some(PendingAction::RestartPc);
        let mut snapshot = fixtures::dev_machine();
        let keychron = snapshot
            .keyboards
            .iter_mut()
            .find(|kb| kb.instance_id == KEYCHRON)
            .unwrap();
        keychron.overrides.keyboard_type_override = Some(7);
        keychron.overrides.keyboard_subtype_override = Some(2);
        keychron.reported_type = Some(KeyboardType::JIS);
        (change, snapshot)
    }

    #[test]
    fn a_keyboard_changed_through_the_restart() {
        let (change, snapshot) = keychron_to_jis();
        let rows = check_rows(&snapshot, &change);
        let ja = post_reboot(&change, &rows, &[], LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(
            ja.snapshot_text(),
            "operation: Keychron Receiver を JIS に\n\
             row: Keychron Receiver | JIS | JIS 配列 ✓ (Success) | 未 (Neutral) | \
             保存値からの予想 0x7/0x2、Windows の報告 0x7/0x2\n\
             buttons: [元に戻す] keep=true\n"
        );
        for text in [&ja.operation, &ja.rows[0].setting, &ja.rows[0].recognized] {
            assert!(
                crate::vm::unexpected_latin(text, &["Keychron Receiver"]).is_empty(),
                "{text}"
            );
        }
        // Typing the other layout: ⚠ and the warning, keep still allowed (review U2).
        let typed = [(KEYCHRON.to_string(), Typed::Differs)];
        let wrong = post_reboot(&change, &rows, &typed, LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(wrong.rows[0].typed, "⚠");
        assert_eq!(
            wrong.keep_warning,
            "⚠ Keychron Receiver が期待と違う配列で動いています。［元に戻す］をおすすめします。"
        );
        assert!(wrong.can_keep);
        let en = post_reboot(
            &change,
            &rows,
            &[(KEYCHRON.to_lowercase(), Typed::AsExpected)],
            LATER_BOOT,
            &name_of,
            Lang::En,
        );
        assert_eq!(
            (en.rows[0].recognized.as_str(), en.rows[0].typed.as_str()),
            ("JIS ✓", "✓")
        );
    }

    #[test]
    fn before_the_restart_keep_is_not_offered() {
        let (change, snapshot) = keychron_to_jis();
        let rows = check_rows(&snapshot, &change);
        let same_boot = post_reboot(&change, &rows, &[], BOOT, &name_of, Lang::Ja);
        assert!(same_boot.not_restarted && !same_boot.can_keep);
    }

    /// The reported bug: the migration 0.1.0 wrote under a loader GUID that never changed. Read
    /// as `mklm_client::journal::read_journal` adopts it, the page offers Keep after the restart,
    /// and not in the boot of the writes.
    #[test]
    fn a_legacy_migration_is_judged_by_its_boot_time() {
        let (ops, baselines) = fixtures::legacy_guid_journal();
        let check = |boot_time| {
            let current = fixtures::legacy_pc_boot(boot_time);
            let journal =
                mklm_client::journal::parse_journal(&ops, &baselines, Some(1), Some(&current))
                    .journal;
            let entries = mklm_client::gate::post_reboot_entries(&journal);
            assert_eq!(entries.len(), 1);
            let entry = entries[0];
            assert_eq!(entry.op_id.as_str(), fixtures::LEGACY_PENDING_OP);
            post_reboot(entry, &[], &[], current.id, &name_of, Lang::Ja)
        };
        let after = check(fixtures::LEGACY_PC_BOOT_TIME_AFTER_RESTART);
        assert!(!after.not_restarted && after.can_keep, "{after:?}");
        assert!(!after.snapshot_text().contains("not restarted"));
        let before = check(fixtures::LEGACY_PC_BOOT_TIME_OF_WRITES);
        assert!(before.not_restarted && !before.can_keep, "{before:?}");
    }

    #[test]
    fn a_migration_lists_every_keyboard_and_reverts_with_a_restart() {
        let mut snapshot = fixtures::dev_machine();
        snapshot.keyboards.retain(|kb| kb.present);
        let mut migrate = entry(
            "8b8b8b8b-0000-4000-8000-00000000000f",
            15,
            OpState::PendingReboot,
        );
        migrate.kind = OpKind::Migrate {
            standard: Layout::Jis,
            assignments: vec![(KEYCHRON.into(), LayoutChoice::Us)],
        };
        let mut global = hid_record(mklm_core::value_names::PS2_TYPE, 7, 0, Some(0));
        global.target = WriteTarget::Global;
        global.intended = RegValue::Absent;
        global.last_written = Some(RegValue::Absent);
        migrate.records.push(global);
        let rows = check_rows(&snapshot, &migrate);
        let ja = post_reboot(&migrate, &rows, &[], LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(ja.rows.len(), 3, "{:?}", ja.rows);
        assert_eq!(
            ja.migration_note,
            "移行が反映されたかは、Windows の認識では分かりません（内蔵キーボードは移行の前後で同じ種類を報告します）。打鍵テストで確かめてください。"
        );
        assert_eq!(ja.revert_text, "元に戻す（もう一度 PC の再起動が必要）");
        let built_in = ja
            .rows
            .iter()
            .find(|row| row.instance_id == BUILT_IN)
            .unwrap();
        assert_eq!(built_in.recognized, "JIS 配列 ✓");
        // The VXE mouse's keyboard collection has no stored type: kbdhid's default, in words.
        let vxe = ja.rows.iter().find(|row| row.name == "VXE R1SE+").unwrap();
        assert_eq!(
            (vxe.recognized.as_str(), vxe.tone),
            ("種類の指定なし（標準に従う） ✓", Tone::Success)
        );
        // Another report than the stored values predict is a warning.
        let mut stale = snapshot.clone();
        for kb in &mut stale.keyboards {
            if kb.instance_id == KEYCHRON {
                kb.reported_type = Some(KeyboardType { ty: 8, subtype: 2 });
            }
        }
        let rows = check_rows(&stale, &migrate);
        let differs = post_reboot(&migrate, &rows, &[], LATER_BOOT, &name_of, Lang::Ja);
        assert_eq!(
            (differs.rows[0].recognized.as_str(), differs.rows[0].tone),
            ("不明な種類（8/2） ⚠ 違います", Tone::Warning)
        );
    }

    #[test]
    fn only_shift_2_with_the_japanese_layout_counts() {
        let jis = LayoutTable::Jis;
        let press = |text: &str, shift, code, hkl| typed_for(text, shift, code, hkl, Some(&jis));
        assert_eq!(
            press("\"", true, SCANCODE_DIGIT2, JAPANESE),
            Some(Typed::AsExpected)
        );
        assert_eq!(
            press("@", true, SCANCODE_DIGIT2, JAPANESE),
            Some(Typed::Differs)
        );
        assert_eq!(press("@", true, SCANCODE_DIGIT2, 0x0409_0409), None);
        assert_eq!(press("2", false, SCANCODE_DIGIT2, JAPANESE), None);
        assert_eq!(press("\"", true, 0x28, JAPANESE), None);
        assert_eq!(typed_for("@", true, SCANCODE_DIGIT2, JAPANESE, None), None);
        assert_eq!(
            nothing(Lang::Ja).empty_note,
            "再起動の後に確認する変更はありません。"
        );
    }
}
