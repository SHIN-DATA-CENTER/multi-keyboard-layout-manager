//! The keyboard list (plan 3.2, design m3 B.2): one row per physical device (`DeviceGroup`, by
//! container; built-in keyboards and keyboards without a container are rows of their own), with
//! the three states of the glossary — 設定した配列 / 保存済み（反映待ち）/ 現在の動作 — and the
//! badges.
//!
//! "✓" means "as you set it" and nothing else (review U4): only a keyboard with an explicitly
//! assigned layout that is in effect gets it. A keyboard that follows the PC's standard layout or
//! the fixed mode is shown neutrally, and a known physical layout that differs from how it types
//! is a warning with the way out ("US にする…").

use mklm_core::{
    Assessment, KeyboardAssessment, KeyboardDevice, KeyboardDriver, LayoutBasis, LayoutTable,
    SystemSnapshot, Transport,
};

use super::{Badge, SnapshotText, Tone};
use crate::i18n::{self, BadgeKind, Lang};
use crate::settings::{PhysicalKind, Settings};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyboardRow {
    /// The group key: the container ID, or the instance ID of a keyboard without a usable one.
    pub id: String,
    /// Instance IDs of the group's keyboards (the first present one is what "assign" targets).
    pub members: Vec<String>,
    pub name: String,
    pub transport: String,
    pub vid_pid: String,
    pub assigned: String,
    /// "保存済み（反映待ち: …が必要）" (glossary) and, on its own line, what to do about it.
    pub pending: String,
    pub pending_hint: String,
    /// "今すぐ反映…" is offered: the journal has an `apply_pending` reset for this keyboard
    /// (`Request::Recover` with the apply method clears it, design m2 D.7 step 4; review U8).
    pub apply_now: bool,
    pub current: String,
    pub current_tone: Tone,
    /// "⚠ 実物は US 配列ですが JIS として動いています" (review U4); empty when nothing is known
    /// or it matches.
    pub physical_note: String,
    pub badges: Vec<Badge>,
    pub highlighted: bool,
    pub can_assign: bool,
    /// Why "変更…" is disabled (design m3 E.2): shown on the row and read with it.
    pub blocked_note: String,
    /// The accessible label of the row's "変更…" button ("Keychron Receiver の配列を変更").
    pub assign_label: String,
    pub accessible_summary: String,
}

/// What the list depends on besides the snapshot.
#[derive(Debug, Clone, Copy)]
pub struct ListOptions<'a> {
    pub settings: &'a Settings,
    /// The keyboard (instance ID) that typed last while "identify by key press" is on.
    pub highlighted: Option<&'a str>,
    /// Why new operations are blocked now (`i18n::cannot_change_now`), if they are (design m2
    /// C.7): the rows cannot be changed.
    pub blocked_reason: Option<&'a str>,
    /// Instance IDs whose journal entry keeps an `apply_pending` keyboard reset (WP-U1).
    pub apply_now: &'a [String],
    pub lang: Lang,
}

/// Receivers MKLM recognizes by name or by VID/PID (plan 3.2: "レシーバー" badge). A heuristic for
/// display only.
pub fn is_receiver(kb: &KeyboardDevice) -> bool {
    const NAMES: [&str; 5] = ["receiver", "dongle", "unifying", "bolt", "レシーバー"];
    const KNOWN: [(u16, u16); 4] = [
        (0x046D, 0xC52B), // Logitech Unifying
        (0x046D, 0xC534), // Logitech nano receiver
        (0x046D, 0xC547), // Logitech Bolt
        (0x046D, 0xC548), // Logitech Bolt
    ];
    let name = kb.display_name.to_lowercase();
    NAMES.iter().any(|n| name.contains(n))
        || kb
            .vendor_id
            .zip(kb.product_id)
            .is_some_and(|id| KNOWN.contains(&id))
}

/// The rows, in snapshot order.
pub fn keyboard_rows(
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    options: &ListOptions<'_>,
) -> Vec<KeyboardRow> {
    let lang = options.lang;
    let mut rows = Vec::new();
    for group in &assessment.groups {
        let members: Vec<(&KeyboardDevice, &KeyboardAssessment)> = group
            .keyboards
            .iter()
            .filter_map(|id| {
                let kb = snapshot.keyboards.iter().find(|kb| &kb.instance_id == id)?;
                let ka = assessment
                    .keyboards
                    .iter()
                    .find(|ka| &ka.instance_id == id)?;
                Some((kb, ka))
            })
            .collect();
        let Some(&(first_kb, _)) = members.first() else {
            continue;
        };
        let id = match (&group.container_id, group.is_internal) {
            (Some(container), false) if first_kb.known_container_id().is_some() => {
                container.clone()
            }
            _ => first_kb.instance_id.clone(),
        };
        let present = members.iter().any(|(kb, _)| kb.present);
        let hidden = options.settings.is_hidden(&id);
        if (!present || hidden) && !options.settings.keyboards.show_hidden {
            continue;
        }
        // The member that speaks for the group: the first connected one, else the first.
        let (kb, ka) = members
            .iter()
            .copied()
            .find(|(kb, _)| kb.present)
            .unwrap_or(members[0]);
        let read_only =
            kb.transport == Transport::Virtual || matches!(kb.driver, KeyboardDriver::Other(_));
        let tables: Vec<_> = members
            .iter()
            .filter_map(|(_, ka)| ka.after_restart.as_ref().map(|l| &l.table))
            .collect();
        let mixed = tables.windows(2).any(|pair| pair[0] != pair[1]);
        let assigned = match (&ka.after_restart, mixed) {
            (_, true) => match lang {
                Lang::Ja => "混在（コレクションごとに違います）".to_string(),
                Lang::En => "mixed (the collections differ)".to_string(),
            },
            (Some(layout), false) => i18n::effective(layout, lang),
            (None, false) => "—".to_string(),
        };
        let pending_action = members.iter().filter_map(|(_, ka)| ka.pending_action).max();
        let pending = pending_action
            .map(|action| i18n::pending(action, lang))
            .unwrap_or_default();
        let pending_hint = pending_action
            .map(|action| i18n::pending_hint(action, lang))
            .unwrap_or_default();
        let (current, current_tone) = current_state(ka, kb.present, lang);
        let physical_note = physical_note(options.settings, &members, ka, lang);
        let highlighted = options.highlighted.is_some_and(|typed| {
            members
                .iter()
                .any(|(kb, _)| kb.instance_id.eq_ignore_ascii_case(typed))
        });
        let mut badges = Vec::new();
        let mut badge = |kind, tone| badges.push(super::Badge::new(kind, tone, lang));
        if highlighted {
            badge(BadgeKind::JustPressed, Tone::Info);
        }
        // Never on the PC's own keyboard (review U17).
        if !group.is_internal
            && !members
                .iter()
                .any(|(kb, _)| options.settings.was_seen(&kb.instance_id))
        {
            badge(BadgeKind::NoKeyPress, Tone::Info);
        }
        if is_receiver(kb) {
            badge(BadgeKind::Receiver, Tone::Info);
        }
        if group.is_internal {
            badge(BadgeKind::Internal, Tone::Neutral);
        }
        if read_only {
            badge(BadgeKind::ReadOnly, Tone::Neutral);
        }
        if !present {
            badge(BadgeKind::NotConnected, Tone::Neutral);
        }
        if hidden {
            badge(BadgeKind::Hidden, Tone::Neutral);
        }
        if members.iter().any(|(_, ka)| !ka.anomalies.is_empty()) {
            badge(BadgeKind::SettingProblem, Tone::Warning);
        }
        let vid_pid = kb
            .vendor_id
            .zip(kb.product_id)
            .map(|(v, p)| format!("{v:04X}:{p:04X}"))
            .unwrap_or_default();
        let transport = i18n::transport(kb.transport, lang);
        let can_assign = !read_only && options.blocked_reason.is_none();
        let blocked_note = match options.blocked_reason {
            Some(reason) if !read_only => reason.to_string(),
            _ => String::new(),
        };
        let apply_now = pending_action == Some(mklm_core::PendingAction::ResetKeyboard)
            && options.blocked_reason.is_none()
            && members.iter().any(|(kb, _)| {
                options
                    .apply_now
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
            });
        let mut summary = vec![
            group.display_name.clone(),
            transport.clone(),
            match lang {
                Lang::Ja => format!("設定した配列 {assigned}"),
                Lang::En => format!("assigned {assigned}"),
            },
        ];
        summary.extend((!pending.is_empty()).then(|| pending.clone()));
        summary.push(current.clone());
        summary.extend((!physical_note.is_empty()).then(|| physical_note.clone()));
        summary.extend(badges.iter().map(|b| b.accessible.clone()));
        summary.extend((!blocked_note.is_empty()).then(|| blocked_note.clone()));
        rows.push(KeyboardRow {
            id,
            members: members
                .iter()
                .map(|(kb, _)| kb.instance_id.clone())
                .collect(),
            name: group.display_name.clone(),
            transport,
            vid_pid,
            assigned,
            pending,
            pending_hint,
            apply_now,
            current,
            current_tone,
            physical_note,
            badges,
            highlighted,
            can_assign,
            blocked_note,
            assign_label: match lang {
                Lang::Ja => format!("{} の配列を変更", group.display_name),
                Lang::En => format!("Change the layout of {}", group.display_name),
            },
            accessible_summary: summary.join(match lang {
                Lang::Ja => "、",
                Lang::En => ", ",
            }),
        });
    }
    rows
}

/// 現在の動作 (glossary): "✓ 設定どおり" only for an explicitly assigned layout in effect; a
/// keyboard that follows the standard layout or the fixed mode is neutral; a difference from
/// what the stored values predict is a warning (review U4).
fn current_state(ka: &KeyboardAssessment, present: bool, lang: Lang) -> (String, Tone) {
    let Some(layout) = ka.current.as_ref().filter(|_| present) else {
        return if present {
            match lang {
                Lang::Ja => ("動作を確認できません".to_string(), Tone::Neutral),
                Lang::En => ("cannot tell how it types".to_string(), Tone::Neutral),
            }
        } else {
            (i18n::badge(BadgeKind::NotConnected, lang).0, Tone::Neutral)
        };
    };
    let name = i18n::table(&layout.table, lang);
    let after = ka.after_restart.as_ref();
    match (after, lang) {
        (Some(after), _) if after.table != layout.table => match lang {
            Lang::Ja => (format!("{name} として動作中 ⚠"), Tone::Warning),
            Lang::En => (format!("types {name} ⚠"), Tone::Warning),
        },
        (Some(after), Lang::Ja) if after.basis == LayoutBasis::KeyboardType => {
            (format!("{name} として動作中 ✓ 設定どおり"), Tone::Success)
        }
        (Some(after), Lang::En) if after.basis == LayoutBasis::KeyboardType => {
            (format!("types {name} ✓ as set"), Tone::Success)
        }
        (Some(after), Lang::Ja) if after.basis == LayoutBasis::FixedMode => {
            (format!("{name} として動作中（固定モード）"), Tone::Neutral)
        }
        (Some(after), Lang::En) if after.basis == LayoutBasis::FixedMode => {
            (format!("types {name} (fixed mode)"), Tone::Neutral)
        }
        (_, Lang::Ja) => (
            format!("{name} として動作中（PC の標準配列）"),
            Tone::Neutral,
        ),
        (_, Lang::En) => (format!("types {name} (the PC's standard)"), Tone::Neutral),
    }
}

/// "⚠ 実物は US 配列ですが JIS として動いています" when a member's physical layout is known and
/// differs from how the keyboard types now (review U4).
fn physical_note(
    settings: &Settings,
    members: &[(&KeyboardDevice, &KeyboardAssessment)],
    ka: &KeyboardAssessment,
    lang: Lang,
) -> String {
    let Some(physical) = members
        .iter()
        .find_map(|(kb, _)| settings.physical(&kb.instance_id))
    else {
        return String::new();
    };
    let (real, real_table) = match physical.layout {
        PhysicalKind::Jis => ("JIS", LayoutTable::Jis),
        PhysicalKind::Us => ("US", LayoutTable::Us),
    };
    match ka.current.as_ref().map(|layout| &layout.table) {
        Some(types) if *types != real_table => {
            let types = i18n::table(types, lang);
            match lang {
                Lang::Ja => format!("⚠ 実物は {real} 配列ですが {types} として動いています"),
                Lang::En => format!("⚠ It is a {real} keyboard but types {types}"),
            }
        }
        _ => String::new(),
    }
}

impl SnapshotText for KeyboardRow {
    fn snapshot_text(&self) -> String {
        let badges: Vec<&str> = self.badges.iter().map(|b| b.text.as_str()).collect();
        let mut text = format!(
            "{} [{} {}] assigned={} pending={} current={} ({:?}) badges={} assign={}{}\n",
            self.name,
            self.transport,
            self.vid_pid,
            self.assigned,
            self.pending,
            self.current,
            self.current_tone,
            badges.join("|"),
            self.can_assign,
            if self.highlighted { " *" } else { "" }
        );
        for (label, value) in [
            ("hint", &self.pending_hint),
            ("physical", &self.physical_note),
            ("blocked", &self.blocked_note),
        ] {
            if !value.is_empty() {
                text.push_str(&format!("  {label}: {value}\n"));
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{assess, fixtures};

    use super::*;
    use crate::settings::{PhysicalLayout, PhysicalSource};

    fn rows(
        settings: &Settings,
        highlighted: Option<&str>,
        blocked_reason: Option<&str>,
        lang: Lang,
    ) -> String {
        let snapshot = fixtures::dev_machine();
        let assessment = assess(&snapshot);
        keyboard_rows(
            &snapshot,
            &assessment,
            &ListOptions {
                settings,
                highlighted,
                blocked_reason,
                apply_now: &[],
                lang,
            },
        )
        .iter()
        .map(SnapshotText::snapshot_text)
        .collect()
    }

    #[test]
    fn the_dev_machine_in_japanese() {
        let mut settings = Settings::default();
        settings
            .keyboards
            .seen
            .push(fixtures::keychron().instance_id);
        let keychron = fixtures::keychron().instance_id;
        let text = rows(&settings, Some(&keychron), None, Lang::Ja);
        // ✓ only where a layout was assigned and is in effect; the VXE follows the standard
        // layout (neutral); no "no key press" badge on the built-in keyboard (review U4, U17).
        assert_eq!(
            text,
            "日本語 PS/2 キーボード (106/109 キー Ctrl+英数) [PS/2 ] assigned=JIS pending= \
             current=JIS として動作中 ✓ 設定どおり (Success) badges=内蔵 assign=true\n\
             Keychron Receiver [USB 3434:D027] assigned=US pending= current=US として動作中 ✓ \
             設定どおり (Success) badges=◀ いま押したキーボード|レシーバー assign=true *\n\
             VXE R1SE+ [Bluetooth LE 25A7:FA6C] assigned=標準に従う（JIS） pending= \
             current=JIS として動作中（PC の標準配列） (Neutral) badges=キー入力なし assign=true\n"
        );
        let names = [
            "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)",
            "Keychron Receiver",
            "VXE R1SE+",
            "assigned",
            "pending",
            "current",
            "Success",
            "Neutral",
            "badges",
            "assign",
            "true",
        ];
        assert_eq!(
            crate::vm::unexpected_latin(&text, &names),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_known_physical_layout_and_a_block() {
        let mut settings = Settings::default();
        // The VXE was detected as a US keyboard, but it types JIS (the standard layout).
        let vxe = fixtures::dev_machine()
            .keyboards
            .iter()
            .find(|kb| kb.display_name.contains("VXE") || kb.instance_id.contains("25A7"))
            .map(|kb| kb.instance_id.clone())
            .unwrap();
        settings.learn_physical(PhysicalLayout {
            id: vxe,
            layout: crate::settings::PhysicalKind::Us,
            source: PhysicalSource::Detection,
        });
        let blocked =
            i18n::cannot_change_now(Some(mklm_core::Attention::WaitingForReboot), Lang::Ja);
        let text = rows(&settings, None, Some(&blocked), Lang::Ja);
        assert!(
            text.contains("physical: ⚠ 実物は US 配列ですが JIS として動いています"),
            "{text}"
        );
        assert!(
            text.contains("blocked: 今は変更できません: PC の再起動を待っている変更があります"),
            "{text}"
        );
        assert!(text.contains("assign=false"), "{text}");
    }

    #[test]
    fn hidden_and_disconnected_keyboards() {
        let mut settings = Settings::default();
        settings.keyboards.show_hidden = true;
        let text = rows(&settings, None, None, Lang::En);
        assert!(text.contains("X3-5.4 Mouse"), "{text}");
        assert!(text.contains("Not connected"), "{text}");
    }
}
