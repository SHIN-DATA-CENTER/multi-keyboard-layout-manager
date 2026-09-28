//! The keyboard list (plan 3.2, design m3 B.2): one row per physical device (`DeviceGroup`, by
//! container; built-in keyboards and keyboards without a container are rows of their own), with
//! the three states of the glossary — 設定した配列 / 保存済み（反映待ち）/ 現在の動作 — and the
//! badges.
//!
//! "✓" means "as you set it" and nothing else (review U4): only a keyboard with an explicitly
//! assigned layout that is in effect gets it. A keyboard that follows the PC's standard layout or
//! the fixed mode is shown neutrally, and a known physical layout that differs from how it types
//! is a warning with the way out ("US にする…").
//!
//! [`main_screen`] puts the whole page together (status line, rows and the identification
//! announcement), so that `app.rs` only copies it into the window and the snapshot tests see
//! exactly what the page says (design m3 H.3). "今すぐ反映…" opens the change page of WP-U3 with
//! only the apply method to choose ([`apply_now_page`], design m3 B.12, review U8).

use mklm_client::startup::StartupSummary;
use mklm_core::{
    Assessment, BootId, Journal, KeyboardAssessment, KeyboardDevice, KeyboardDriver, LayoutBasis,
    LayoutTable, SystemSnapshot, Transport, assess,
};

use super::change::{ApplyMethod, ChangePage};
use super::status::{
    NeedsApply, RestartWaits, StatusLine, blocked_reason, needs_apply, restart_waits, status_line,
};
use super::{Badge, SnapshotText, Tone};
use crate::i18n::{self, BadgeKind, Behavior, Lang};
use crate::settings::{PhysicalKind, Settings};
use crate::state::ChangeDraft;

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
    /// Hidden by the user (listed only with "非表示と未接続も表示", or while it is the keyboard
    /// that typed): the row menu offers "表示する" instead of "非表示にする".
    pub hidden: bool,
    pub can_assign: bool,
    /// Why "変更…" is disabled (design m3 E.2): shown on the row and read with it. On the Remote
    /// Desktop keyboard's row (never changeable), what its client reported instead
    /// (`i18n::remote_client_report`), when it reported a type MKLM can name.
    pub blocked_note: String,
    /// The accessible label of the row's "変更…" button ("Keychron Receiver の配列を変更").
    pub assign_label: String,
    /// The accessible label of the row's "今すぐ反映…" button ("Keychron Receiver の配列を今すぐ
    /// 反映"; design m3 E.2): several rows may offer it.
    pub apply_now_label: String,
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
    /// Keyboards (instance IDs) "今すぐ反映…" puts into effect ([`NeedsApply::apply_now`]).
    pub apply_now: &'a [String],
    /// Keyboards the journal leaves to the PC restart ([`restart_waits`]).
    pub restart: &'a RestartWaits,
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
        let highlighted = options.highlighted.is_some_and(|typed| {
            members
                .iter()
                .any(|(kb, _)| kb.instance_id.eq_ignore_ascii_case(typed))
        });
        // Hidden and disconnected rows only on request; a hidden keyboard that just typed while
        // identifying is shown (with its "非表示" badge), or identifying could not find it.
        if (!present || (hidden && !highlighted)) && !options.settings.keyboards.show_hidden {
            continue;
        }
        // The member that speaks for the group: the first connected one, else the first.
        let (kb, ka) = members
            .iter()
            .copied()
            .find(|(kb, _)| kb.present)
            .unwrap_or(members[0]);
        // The keys the Remote Desktop client sends (plan 3.2: read-only). Its row names no
        // layout: the session types with the table it started with, which MKLM cannot read.
        let remote = kb.is_remote_desktop();
        let read_only = remote
            || kb.transport == Transport::Virtual
            || matches!(kb.driver, KeyboardDriver::Other(_));
        let tables: Vec<_> = members
            .iter()
            .filter_map(|(_, ka)| ka.after_restart.as_ref().map(|l| &l.table))
            .collect();
        let mixed = tables.windows(2).any(|pair| pair[0] != pair[1]);
        let assigned = match (&ka.after_restart, mixed) {
            (_, true) => i18n::assigned_mixed(lang),
            (Some(layout), false) => i18n::effective(layout, lang),
            (None, false) => "—".to_string(),
        };
        let pending_action = members.iter().filter_map(|(_, ka)| ka.pending_action).max();
        // Not in effect yet, and the journal waits for the restart: that is what puts it into
        // effect, whatever the device would allow (the banner says the same).
        let pending_action = pending_action.map(|action| {
            if members
                .iter()
                .any(|(kb, _)| options.restart.covers(&kb.instance_id))
            {
                mklm_core::PendingAction::RestartPc
            } else {
                action
            }
        });
        // Offered only where the journal holds a reset MKLM can redo now (review U8): a change
        // the journal does not know is not put into effect by `Recover`.
        let apply_now = pending_action == Some(mklm_core::PendingAction::ResetKeyboard)
            && options.blocked_reason.is_none()
            && members.iter().any(|(kb, _)| {
                options
                    .apply_now
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
            });
        let pending = pending_action
            .map(|action| i18n::pending(action, lang))
            .unwrap_or_default();
        let pending_hint = pending_action
            .map(|action| {
                if apply_now {
                    i18n::pending_hint(action, lang)
                } else {
                    i18n::pending_hint_without_apply_now(action, lang)
                }
            })
            .unwrap_or_default();
        let (current, current_tone) = if remote && kb.present {
            (i18n::remote_current(lang), Tone::Neutral)
        } else {
            current_state(ka, kb.present, lang)
        };
        let physical_note = physical_note(options.settings, &members, ka, lang);
        let mut badges = Vec::new();
        let mut badge = |kind, tone| badges.push(super::Badge::new(kind, tone, lang));
        if highlighted {
            badge(BadgeKind::JustPressed, Tone::Info);
        }
        // Never on the PC's own keyboard (review U17), nor on the Remote Desktop keyboard, whose
        // key presses Raw Input does not name.
        if !group.is_internal
            && !remote
            && !members
                .iter()
                .any(|(kb, _)| options.settings.was_seen(&kb.instance_id))
        {
            badge(BadgeKind::NoKeyPress, Tone::Info);
        }
        if is_receiver(kb) {
            badge(BadgeKind::Receiver, Tone::Info);
        }
        // Windows puts the Remote Desktop keyboard in the built-in container; it is not built in.
        if group.is_internal && !remote {
            badge(BadgeKind::Internal, Tone::Neutral);
        }
        if remote {
            badge(BadgeKind::RemoteDesktop, Tone::Neutral);
        } else if read_only {
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
        let transport = if remote {
            i18n::remote_transport(lang)
        } else {
            i18n::transport(kb.transport, lang)
        };
        let can_assign = !read_only && options.blocked_reason.is_none();
        let blocked_note = match options.blocked_reason {
            Some(reason) if !read_only => reason.to_string(),
            // The caption of the Remote Desktop keyboard's row (which "変更…" never is): what the
            // client reported, as its report only.
            _ if remote && kb.present && snapshot.os.remote_session => snapshot
                .os
                .client_keyboard_type
                .and_then(|reported| i18n::remote_client_report(reported, lang))
                .unwrap_or_default(),
            _ => String::new(),
        };
        // One sentence for the screen reader, in the order the row shows it (design m3 E.2).
        let mut summary = vec![
            group.display_name.clone(),
            transport.clone(),
            i18n::assigned_summary(&assigned, lang),
        ];
        summary.extend((!pending.is_empty()).then(|| pending.clone()));
        summary.extend((!pending_hint.is_empty()).then(|| pending_hint.clone()));
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
            hidden,
            can_assign,
            blocked_note,
            assign_label: i18n::assign_label(&group.display_name, lang),
            apply_now_label: i18n::apply_now_label(&group.display_name, lang),
            accessible_summary: i18n::summary_join(&summary, lang),
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
            (i18n::behavior_unknown(lang), Tone::Neutral)
        } else {
            (i18n::badge(BadgeKind::NotConnected, lang).0, Tone::Neutral)
        };
    };
    let (behavior, tone) = match ka.after_restart.as_ref() {
        Some(after) if after.table != layout.table => (Behavior::Unexpected, Tone::Warning),
        Some(after) if after.basis == LayoutBasis::KeyboardType => (Behavior::AsSet, Tone::Success),
        Some(after) if after.basis == LayoutBasis::FixedMode => {
            (Behavior::FixedMode, Tone::Neutral)
        }
        _ => (Behavior::Standard, Tone::Neutral),
    };
    (i18n::current_behavior(&layout.table, behavior, lang), tone)
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
    let real = match physical.layout {
        PhysicalKind::Jis => LayoutTable::Jis,
        PhysicalKind::Us => LayoutTable::Us,
    };
    match ka.current.as_ref().map(|layout| &layout.table) {
        Some(types) if *types != real => i18n::physical_differs(&real, types, lang),
        _ => String::new(),
    }
}

/// "Keychron Receiver のキーが押されました" (polite, design m3 B.3, E.3) for the row that
/// "キーを押して特定" marks; empty when none is marked. The text changes only when another row
/// is marked, so the same keyboard typing on is not announced again.
pub fn identify_announcement(rows: &[KeyboardRow], lang: Lang) -> String {
    rows.iter()
        .find(|row| row.highlighted)
        .map(|row| i18n::key_pressed_on(&row.name, lang))
        .unwrap_or_default()
}

/// The physical devices (groups) that contain one of `ids`, in list order.
fn groups_of<'a>(
    assessment: &'a Assessment,
    ids: &'a [String],
) -> impl Iterator<Item = &'a mklm_core::DeviceGroup> + 'a {
    assessment.groups.iter().filter(move |group| {
        group
            .keyboards
            .iter()
            .any(|member| ids.iter().any(|id| id.eq_ignore_ascii_case(member)))
    })
}

/// The instance IDs of every physical device (group) that contains one of `ids`: the keyboards a
/// reset of `ids` takes away for a few seconds (the apply method's default, design m3 B.5).
pub fn device_members(assessment: &Assessment, ids: &[String]) -> Vec<String> {
    groups_of(assessment, ids)
        .flat_map(|group| group.keyboards.iter().cloned())
        .collect()
}

/// The devices that contain one of `ids`, one entry each, in list order: the name and the
/// instance IDs (the "今の状態" of the result names them one by one, design m3 B.17).
pub fn devices(assessment: &Assessment, ids: &[String]) -> Vec<(String, Vec<String>)> {
    groups_of(assessment, ids)
        .map(|group| (group.display_name.clone(), group.keyboards.clone()))
        .collect()
}

/// Everything the "今すぐ反映…" page depends on besides the draft (design m3 B.12; review U8).
#[derive(Debug, Clone, Copy)]
pub struct ApplyNowContext<'a> {
    /// `ChangeDraft::apply_now`: the devices by name and the method.
    pub draft: &'a ChangeDraft,
    /// The last read still leaves something that a reset puts into effect, and new operations
    /// are not blocked (`state::apply_now_targets` is not empty).
    pub offered: bool,
    /// Why new operations are blocked now ([`blocked_reason`]), if they are.
    pub blocked: Option<&'a str>,
    /// `settings.change.uac_notice_seen`: the prompt is explained by one line next to the button.
    pub uac_notice_seen: bool,
    /// The GUI runs elevated: no prompt follows (design m3 B.5).
    pub elevated: bool,
    pub lang: Lang,
}

/// The change page of WP-U3 for "今すぐ反映…" (design m3 B.12, B.18: the two ways of design m3
/// B.5, then `Request::Recover`): no layout to choose, only how — reset now, or not now (an
/// unplug or the PC restart puts it into effect, and nothing is sent). Resetting is never the
/// only choice and never starts without the button (design m3 0.2 principle 6).
pub fn apply_now_page(ctx: &ApplyNowContext<'_>) -> ChangePage {
    let lang = ctx.lang;
    let draft = ctx.draft;
    let prompt = !ctx.elevated;
    let mut page = ChangePage {
        title: i18n::apply_now_title(&draft.name, lang),
        apply_text: i18n::apply_now_button(prompt, lang),
        ..ChangePage::default()
    };
    if let Some(reason) = ctx.blocked {
        // Another operation started meanwhile (from the CLI, say): nothing may be sent.
        page.note = reason.to_string();
        page.note_tone = Tone::Warning;
        return page;
    }
    if !ctx.offered {
        // Replugged or put into effect meanwhile: nothing left to decide.
        page.note = i18n::apply_now_nothing_left(lang);
        page.note_tone = Tone::Info;
        return page;
    }
    page.summary = i18n::apply_now_summary(&draft.name, lang);
    let method = draft.resolved_method();
    (page.live_text, page.live_detail) = i18n::apply_now_method(true, lang);
    (page.restart_text, page.restart_detail) = i18n::apply_now_method(false, lang);
    page.method_visible = true;
    page.method = Some(method);
    if draft.method.is_none()
        && draft
            .method_default
            .is_some_and(|default| default.only_keyboard_warning)
    {
        page.warnings.push(i18n::apply_now_only_keyboard(lang));
    }
    page.can_apply = method == ApplyMethod::Live;
    if prompt && ctx.uac_notice_seen && page.can_apply {
        page.uac_line = i18n::uac_line(lang);
    }
    page
}

/// Everything the main screen depends on.
#[derive(Debug, Clone, Copy)]
pub struct MainInput<'a> {
    pub snapshot: &'a SystemSnapshot,
    /// The journal and the boot it was read in; `None` when either could not be read (nothing
    /// is offered to put into effect then).
    pub journal: Option<(&'a Journal, BootId)>,
    pub summary: &'a StartupSummary,
    pub settings: &'a Settings,
    /// The GUI thread's input language (HKL, low 32 bits).
    pub active_hkl: u32,
    /// The keyboard (instance ID) that typed last while identifying.
    pub highlighted: Option<&'a str>,
    pub lang: Lang,
}

/// The main screen (plan 3.2, design m3 B.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainScreen {
    pub status: StatusLine,
    pub rows: Vec<KeyboardRow>,
    /// See [`identify_announcement`].
    pub announcement: String,
}

/// What "今すぐ反映…" would put into effect now: [`needs_apply`] of the journal (empty without
/// a journal).
pub fn main_needs_apply(input: &MainInput<'_>) -> NeedsApply {
    input
        .journal
        .map(|(journal, boot)| needs_apply(journal, boot, input.summary, input.snapshot))
        .unwrap_or_default()
}

pub fn main_screen(input: &MainInput<'_>) -> MainScreen {
    let lang = input.lang;
    let assessment = assess(input.snapshot);
    let needs_apply = main_needs_apply(input);
    let blocked = blocked_reason(input.summary, lang);
    let status = status_line(
        &assessment,
        &input.snapshot.input,
        input.active_hkl,
        input.summary,
        &needs_apply,
        lang,
    );
    let restart = input
        .journal
        .map(|(journal, boot)| restart_waits(journal, boot))
        .unwrap_or_default();
    let rows = keyboard_rows(
        input.snapshot,
        &assessment,
        &ListOptions {
            settings: input.settings,
            highlighted: input.highlighted,
            blocked_reason: blocked.as_deref(),
            apply_now: &needs_apply.apply_now,
            restart: &restart,
            lang,
        },
    );
    let announcement = identify_announcement(&rows, lang);
    MainScreen {
        status,
        rows,
        announcement,
    }
}

impl SnapshotText for MainScreen {
    fn snapshot_text(&self) -> String {
        let mut text = self.status.snapshot_text();
        if !self.announcement.is_empty() {
            text.push_str(&format!("announce: {}\n", self.announcement));
        }
        for row in &self.rows {
            text.push_str(&row.snapshot_text());
            text.push_str(&format!(
                "  reader: {}\n  button: {}\n",
                row.accessible_summary, row.assign_label
            ));
            if row.apply_now {
                text.push_str(&format!("  apply-now button: {}\n", row.apply_now_label));
            }
        }
        text
    }
}

impl SnapshotText for KeyboardRow {
    fn snapshot_text(&self) -> String {
        let badges: Vec<&str> = self.badges.iter().map(|b| b.text.as_str()).collect();
        let mut text = format!(
            "{} [{} {}] assigned={} pending={} current={} ({:?}) badges={} assign={}{}{}{}\n",
            self.name,
            self.transport,
            self.vid_pid,
            self.assigned,
            self.pending,
            self.current,
            self.current_tone,
            badges.join("|"),
            self.can_assign,
            if self.apply_now { " apply-now" } else { "" },
            if self.hidden { " hidden" } else { "" },
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
    use crate::vm::change::MethodDefault;

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
                restart: &RestartWaits::default(),
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
    fn the_dev_machine_in_english() {
        let text = rows(&Settings::default(), None, None, Lang::En);
        assert!(
            text.contains("current=types US ✓ as set (Success)"),
            "{text}"
        );
        assert!(
            text.contains("assigned=follows the standard (JIS)"),
            "{text}"
        );
        assert!(
            text.contains("current=types JIS (the PC's standard) (Neutral)"),
            "{text}"
        );
    }

    #[test]
    fn a_known_physical_layout_and_a_block() {
        let mut settings = Settings::default();
        // The VXE was detected as a US keyboard, but it types JIS (the standard layout).
        let vxe = fixtures::vxe_ble().instance_id;
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
        let english = rows(&settings, None, None, Lang::En);
        assert!(
            english.contains("physical: ⚠ It is a US keyboard but types JIS"),
            "{english}"
        );
    }

    /// The development machine seen from a Remote Desktop session whose client reported `client`.
    fn remote_session(client: Option<mklm_core::KeyboardType>) -> SystemSnapshot {
        let mut snapshot = fixtures::dev_machine();
        snapshot.keyboards.push(fixtures::rdp_keyboard());
        snapshot.os.remote_session = true;
        snapshot.os.client_keyboard_type = client;
        snapshot
    }

    fn rdp_row(snapshot: &SystemSnapshot, blocked_reason: Option<&str>, lang: Lang) -> KeyboardRow {
        let settings = Settings::default();
        keyboard_rows(
            snapshot,
            &assess(snapshot),
            &ListOptions {
                settings: &settings,
                highlighted: None,
                blocked_reason,
                apply_now: &[],
                restart: &RestartWaits::default(),
                lang,
            },
        )
        .into_iter()
        .find(|row| row.members == vec![fixtures::rdp_keyboard().instance_id])
        .expect("the Remote Desktop keyboard's row")
    }

    const RDP_NAME: &str = "リモート デスクトップ キーボード デバイス";

    #[test]
    fn the_remote_desktop_keyboard_in_a_remote_session() {
        let snapshot = remote_session(Some(mklm_core::KeyboardType::JIS));
        let row = rdp_row(&snapshot, None, Lang::Ja);
        // No layout is named (the client's 7/2 is not what the session types with), no "内蔵"
        // or "キー入力なし", and "リモート デスクトップ" replaces "読み取り専用".
        assert_eq!(
            row.snapshot_text(),
            "リモート デスクトップ キーボード デバイス [リモート デスクトップ ] assigned=— pending= \
             current=接続元の PC からの入力 (Neutral) badges=リモート デスクトップ assign=false\n  \
             blocked: 接続元の報告: 日本語キーボード (JIS)。このセッションのキーの割り当てと同じとは限りません\n"
        );
        assert!(!row.can_assign && !row.apply_now);
        assert_eq!(row.id, fixtures::rdp_keyboard().instance_id);
        assert!(
            row.accessible_summary
                .contains("リモート デスクトップ: 接続元の PC から届くキー入力です。"),
            "{row:?}"
        );
        for text in [row.snapshot_text(), row.accessible_summary.clone()] {
            assert_eq!(
                crate::vm::unexpected_latin(
                    &text,
                    &[
                        RDP_NAME, "assigned", "pending", "current", "Neutral", "badges", "assign",
                        "false", "blocked"
                    ]
                ),
                Vec::<String>::new(),
                "{text}"
            );
        }

        let english = rdp_row(&snapshot, None, Lang::En);
        assert_eq!(english.transport, "Remote Desktop");
        assert_eq!(english.current, "Input from the client PC");
        assert_eq!(
            english.blocked_note,
            "The client reports a Japanese keyboard (JIS); this session's key table may differ"
        );
        assert_eq!(
            english
                .badges
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Remote Desktop"]
        );

        // A block does not replace the report: the row is never changeable anyway.
        let blocked =
            i18n::cannot_change_now(Some(mklm_core::Attention::WaitingForReboot), Lang::Ja);
        let row = rdp_row(&snapshot, Some(&blocked), Lang::Ja);
        assert!(row.blocked_note.starts_with("接続元の報告: "), "{row:?}");
        assert!(!row.can_assign);
    }

    #[test]
    fn the_remote_desktop_keyboard_without_a_report() {
        // Nothing reported, or a type without a name here: no caption.
        for client in [None, Some(mklm_core::KeyboardType::HID_UNKNOWN)] {
            let row = rdp_row(&remote_session(client), None, Lang::Ja);
            assert_eq!(row.blocked_note, "", "{client:?}");
            assert_eq!(row.current, "接続元の PC からの入力");
        }
        // Outside a remote session nothing the client said is shown.
        let mut console = remote_session(Some(mklm_core::KeyboardType::JIS));
        console.os.remote_session = false;
        assert_eq!(rdp_row(&console, None, Lang::Ja).blocked_note, "");
        // The keyboard of a disconnected session is listed only on request, as not connected.
        let mut gone = remote_session(Some(mklm_core::KeyboardType::JIS));
        let rdp = gone.keyboards.last_mut().unwrap();
        rdp.present = false;
        rdp.dev_node_status = None;
        let mut settings = Settings::default();
        let assessment = assess(&gone);
        let list = |settings: &Settings| {
            keyboard_rows(
                &gone,
                &assessment,
                &ListOptions {
                    settings,
                    highlighted: None,
                    blocked_reason: None,
                    apply_now: &[],
                    restart: &RestartWaits::default(),
                    lang: Lang::Ja,
                },
            )
        };
        assert!(list(&settings).iter().all(|row| row.name != RDP_NAME));
        settings.keyboards.show_hidden = true;
        let rows = list(&settings);
        let row = rows.iter().find(|row| row.name == RDP_NAME).unwrap();
        assert_eq!(row.current, "未接続");
        assert_eq!(
            row.badges
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>(),
            vec!["リモート デスクトップ", "未接続"]
        );
        assert_eq!(row.blocked_note, "");
        // Other rows are as before.
        let text: String = rows.iter().map(SnapshotText::snapshot_text).collect();
        assert!(text.contains("badges=内蔵 assign=true"), "{text}");
        assert_eq!(text.matches("リモート デスクトップ").count(), 3, "{text}");
    }

    #[test]
    fn hidden_and_disconnected_keyboards() {
        // By default neither the hidden row nor the one without a connected member is listed.
        let mut settings = Settings::default();
        settings
            .keyboards
            .hidden
            .push(fixtures::vxe_ble().container_id.unwrap());
        let text = rows(&settings, None, None, Lang::En);
        assert!(!text.contains("X3-5.4 Mouse"), "{text}");
        assert!(!text.contains("VXE"), "{text}");
        // "非表示と未接続も表示": both, marked.
        settings.keyboards.show_hidden = true;
        let text = rows(&settings, None, None, Lang::En);
        assert!(text.contains("X3-5.4 Mouse"), "{text}");
        assert!(text.contains("Not connected"), "{text}");
        assert!(text.contains("badges=No key press yet|Hidden"), "{text}");
        assert!(text.contains(" hidden\n"), "{text}");
    }

    const BOOT: &str = "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d";

    /// A schema-1 journal entry (as M2 wrote them) that set the Keychron to JIS and closed in
    /// `state` with `apply_pending` (JSON or `null`).
    fn keychron_entry(state: &str, apply_pending: &str) -> mklm_core::JournalEntry {
        let json = format!(
            r#"{{
              "schema_version": 1,
              "op_id": "0000000a-0000-4000-8000-000000000000",
              "seq": 1,
              "kind": {{ "kind": "set-layout",
                "requested": "HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000",
                "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
                "layout": "jis" }},
              "state": "{state}",
              "boot_id": "{BOOT}",
              "owner": {{ "pid": 12345, "creation_time": 134036790000000000 }},
              "created_at": 1790500000000,
              "updated_at": 1790500004000,
              "apply": "reset-keyboard",
              "countdown": null,
              "records": [],
              "context": [],
              "failure": null,
              "apply_pending": {apply_pending},
              "history": []
            }}"#
        );
        mklm_core::JournalEntry::from_json(&json).unwrap()
    }

    fn reset_pending() -> String {
        format!(
            r#"{{ "action": "reset-keyboard",
                "instance_ids": ["HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000"],
                "since": "{BOOT}" }}"#
        )
    }

    /// The dev machine with the Keychron stored as JIS while it still types US (the reset that
    /// would put it into effect has not happened).
    fn keychron_saved_as_jis() -> SystemSnapshot {
        let mut snapshot = fixtures::dev_machine();
        let keychron = snapshot
            .keyboards
            .iter_mut()
            .find(|kb| kb.display_name == "Keychron Receiver")
            .unwrap();
        keychron.overrides.keyboard_type_override = Some(7);
        keychron.overrides.keyboard_subtype_override = Some(2);
        snapshot
    }

    struct Scene {
        snapshot: SystemSnapshot,
        journal: Journal,
        boot: BootId,
        summary: StartupSummary,
        settings: Settings,
    }

    impl Scene {
        fn new(snapshot: SystemSnapshot, entries: Vec<mklm_core::JournalEntry>) -> Self {
            let journal = Journal {
                entries,
                ..Journal::default()
            };
            let boot = BootId::parse(BOOT).unwrap();
            let summary =
                mklm_client::startup::summarize(&journal, boot, &|_| mklm_core::Liveness::Dead);
            Self {
                snapshot,
                journal,
                boot,
                summary,
                settings: Settings::default(),
            }
        }

        fn screen(&self, lang: Lang) -> MainScreen {
            main_screen(&MainInput {
                snapshot: &self.snapshot,
                journal: Some((&self.journal, self.boot)),
                summary: &self.summary,
                settings: &self.settings,
                active_hkl: 0x0411_0411,
                highlighted: None,
                lang,
            })
        }
    }

    const LATIN_NAMES: [&str; 4] = [
        "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)",
        "Keychron Receiver",
        "VXE R1SE+",
        "X3-5.4 Mouse",
    ];

    #[test]
    fn a_saved_reset_is_offered_on_its_row_and_the_banner() {
        let scene = Scene::new(
            keychron_saved_as_jis(),
            vec![keychron_entry("confirmed", &reset_pending())],
        );
        let screen = scene.screen(Lang::Ja);
        assert_eq!(
            screen.status.banner_target,
            Some(super::super::status::BannerTarget::ApplyNow)
        );
        assert_eq!(screen.status.banner_action, "今すぐ反映…");
        let keychron = &screen.rows[1];
        assert!(keychron.apply_now, "{keychron:?}");
        assert_eq!(
            keychron.pending,
            "保存済み（反映待ち: キーボードのリセットが必要）"
        );
        assert_eq!(
            keychron.pending_hint,
            "抜き差しするか、［今すぐ反映…］を押してください"
        );
        assert!(
            keychron.accessible_summary.contains("［今すぐ反映…］"),
            "{keychron:?}"
        );
        // The same NeedsApply entry blocks nothing.
        assert!(screen.rows.iter().all(|row| row.blocked_note.is_empty()));
        assert!(keychron.can_assign);
        for row in &screen.rows {
            for text in [&row.pending_hint, &row.accessible_summary] {
                assert_eq!(
                    crate::vm::unexpected_latin(text, &LATIN_NAMES),
                    Vec::<String>::new()
                );
            }
        }
    }

    #[test]
    fn nothing_is_offered_once_raw_input_reports_the_stored_type() {
        // The keyboard was replugged: it types JIS as stored, the journal still lists it.
        let mut snapshot = keychron_saved_as_jis();
        snapshot.keyboards[1].reported_type = Some(mklm_core::KeyboardType::JIS);
        let scene = Scene::new(
            snapshot,
            vec![keychron_entry("confirmed", &reset_pending())],
        );
        let screen = scene.screen(Lang::Ja);
        assert_eq!(screen.status.banner, "");
        assert!(!screen.rows[1].apply_now);
        assert_eq!(screen.rows[1].pending, "");
    }

    #[test]
    fn a_pending_change_the_journal_does_not_know_has_no_button() {
        // Stored JIS, typing US, no journal entry: `Recover` would not reset it (review U8).
        let scene = Scene::new(keychron_saved_as_jis(), Vec::new());
        let screen = scene.screen(Lang::Ja);
        let keychron = &screen.rows[1];
        assert!(!keychron.apply_now);
        assert_eq!(keychron.pending_hint, "抜き差しすると反映されます");
        assert_eq!(screen.status.banner, "");
    }

    #[test]
    fn a_block_disables_the_rows_and_the_apply_now_button() {
        let mut scene = Scene::new(
            keychron_saved_as_jis(),
            vec![keychron_entry("pending-reboot", "null")],
        );
        scene.settings.keyboards.show_hidden = true;
        let screen = scene.screen(Lang::Ja);
        assert_eq!(screen.status.banner_action, "再起動…");
        // The journal waits for the restart: the row says so, not "reset" or "unplug".
        let keychron = &screen.rows[1];
        assert_eq!(
            (keychron.pending.as_str(), keychron.pending_hint.as_str()),
            (
                "保存済み（反映待ち: PC の再起動が必要）",
                "PC を再起動してください（シャットダウンではなく再起動）"
            )
        );
        for row in screen.rows.iter().filter(|row| row.name != "X3-5.4 Mouse") {
            assert!(!row.can_assign && !row.apply_now);
            assert_eq!(
                row.blocked_note,
                "今は変更できません: PC の再起動を待っている変更があります"
            );
            assert!(
                row.accessible_summary.ends_with(&row.blocked_note),
                "{row:?}"
            );
        }
        let english = scene.screen(Lang::En);
        assert_eq!(
            english.rows[0].blocked_note,
            "Cannot change now: a change waits for a PC restart"
        );
    }

    #[test]
    fn identifying_marks_announces_and_reveals_a_hidden_keyboard() {
        let keychron = fixtures::keychron();
        let mut settings = Settings::default();
        settings
            .keyboards
            .hidden
            .push(keychron.container_id.clone().unwrap());
        let snapshot = fixtures::dev_machine();
        let assessment = assess(&snapshot);
        let list = |highlighted: Option<&str>, lang| {
            keyboard_rows(
                &snapshot,
                &assessment,
                &ListOptions {
                    settings: &settings,
                    highlighted,
                    blocked_reason: None,
                    apply_now: &[],
                    restart: &RestartWaits::default(),
                    lang,
                },
            )
        };
        // Hidden: not listed.
        assert!(list(None, Lang::Ja).iter().all(|row| !row.hidden));
        assert_eq!(identify_announcement(&list(None, Lang::Ja), Lang::Ja), "");
        // It typed while identifying: listed, marked, announced, still badged as hidden.
        let rows = list(Some(&keychron.instance_id), Lang::Ja);
        let row = rows.iter().find(|row| row.highlighted).unwrap();
        assert!(row.hidden);
        assert!(row.badges.iter().any(|badge| badge.text == "非表示"));
        assert!(
            row.badges
                .iter()
                .any(|badge| badge.text == "◀ いま押したキーボード")
        );
        assert_eq!(
            identify_announcement(&rows, Lang::Ja),
            "Keychron Receiver のキーが押されました"
        );
        assert_eq!(
            identify_announcement(&list(Some(&keychron.instance_id), Lang::En), Lang::En),
            "A key was pressed on Keychron Receiver"
        );
    }

    #[test]
    fn device_members_cover_the_whole_device() {
        let snapshot = fixtures::dev_machine();
        let assessment = assess(&snapshot);
        let keychron = fixtures::keychron().instance_id;
        assert_eq!(
            device_members(&assessment, &[keychron.to_lowercase()]),
            vec![keychron.clone()]
        );
        assert_eq!(
            devices(&assessment, std::slice::from_ref(&keychron)),
            vec![("Keychron Receiver".to_string(), vec![keychron])]
        );
        assert!(device_members(&assessment, &[]).is_empty());
    }

    fn apply_now_draft(method: Option<ApplyMethod>, default: MethodDefault) -> ChangeDraft {
        ChangeDraft {
            apply_now: true,
            method,
            method_default: Some(default),
            ..ChangeDraft::new(
                String::new(),
                "Keychron Receiver".into(),
                vec![fixtures::keychron().instance_id],
            )
        }
    }

    const LIVE: MethodDefault = MethodDefault {
        method: ApplyMethod::Live,
        only_keyboard_warning: false,
    };

    #[test]
    fn the_apply_now_page() {
        let draft = apply_now_draft(None, LIVE);
        let context = |uac_notice_seen, elevated, lang| ApplyNowContext {
            draft: &draft,
            offered: true,
            blocked: None,
            uac_notice_seen,
            elevated,
            lang,
        };
        // The first time the standalone UAC explanation follows the button: no line here.
        let ja = apply_now_page(&context(false, false, Lang::Ja));
        assert_eq!(
            ja.snapshot_text(),
            "title: Keychron Receiver の配列を今すぐ反映\n\
             method: ◉ 今すぐキーボードをリセットして反映する — このキーボードは数秒間使えません。その間は、ほかのキーボードかマウスで操作します。\n\
             method: ○ 今はリセットしない — MKLM は何もしません。キーボードを抜き差しするか、PC を再起動すると反映されます（シャットダウンではなく再起動）。\n\
             summary: Keychron Receiver の配列は保存済みですが、まだ反映されていません。キーボードをリセットすると反映されます（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。\n\
             all users: \n\
             button: 反映する（次に Windows の確認が出ます）\n\
             can apply: true\n"
        );
        assert_eq!(
            crate::vm::unexpected_latin(
                &crate::vm::snapshot_values(&ja.snapshot_text()),
                &LATIN_NAMES
            ),
            Vec::<String>::new()
        );
        // Later with one line; never when no prompt follows.
        let seen = apply_now_page(&context(true, false, Lang::En));
        assert!(
            seen.uac_line
                .starts_with("Windows asks for permission next")
        );
        assert_eq!(seen.apply_text, "Apply now (Windows asks next)");
        let elevated = apply_now_page(&context(true, true, Lang::Ja));
        assert_eq!(
            (elevated.uac_line.as_str(), elevated.apply_text.as_str()),
            ("", "反映する")
        );
    }

    #[test]
    fn the_apply_now_page_does_not_reset_the_only_keyboard_by_default() {
        let only = MethodDefault {
            method: ApplyMethod::Restart,
            only_keyboard_warning: true,
        };
        let draft = apply_now_draft(None, only);
        let page = apply_now_page(&ApplyNowContext {
            draft: &draft,
            offered: true,
            blocked: None,
            uac_notice_seen: true,
            elevated: false,
            lang: Lang::Ja,
        });
        // "Not now" is preselected and sends nothing; the warning of plan 1.4 says why.
        assert_eq!(page.method, Some(ApplyMethod::Restart));
        assert!(!page.can_apply && page.uac_line.is_empty());
        assert_eq!(
            page.warnings,
            vec![
                "このキーボードは、最近入力のあった唯一のキーボードです。抜き差しするか、PC を再起動して反映することをおすすめします。"
                    .to_string()
            ]
        );
        // The user's own choice: reset now, and the warning has done its job.
        let chosen = apply_now_draft(Some(ApplyMethod::Live), only);
        let page = apply_now_page(&ApplyNowContext {
            draft: &chosen,
            offered: true,
            blocked: None,
            uac_notice_seen: true,
            elevated: false,
            lang: Lang::Ja,
        });
        assert!(page.can_apply && page.warnings.is_empty());
        // Nothing left (replugged meanwhile): nothing to choose or send.
        let page = apply_now_page(&ApplyNowContext {
            draft: &chosen,
            offered: false,
            blocked: None,
            uac_notice_seen: true,
            elevated: false,
            lang: Lang::En,
        });
        assert!(!page.method_visible && !page.can_apply && page.summary.is_empty());
        assert_eq!(
            page.note,
            "No saved layout is waiting to be put into effect."
        );
        // Another operation started meanwhile: why nothing may be sent.
        let busy = i18n::cannot_change_now(Some(mklm_core::Attention::Busy), Lang::Ja);
        let page = apply_now_page(&ApplyNowContext {
            draft: &chosen,
            offered: false,
            blocked: Some(&busy),
            uac_notice_seen: true,
            elevated: false,
            lang: Lang::Ja,
        });
        assert!(!page.method_visible && !page.can_apply);
        assert_eq!(
            (page.note.as_str(), page.note_tone),
            ("今は変更できません: 別の MKLM が処理中です", Tone::Warning)
        );
    }

    #[test]
    fn the_dev_machine_journal_of_m2_shows_nothing_to_do() {
        // The schema-1 journal the development machine keeps from the M2 tests (a resolved
        // conflict and a kept US assignment, both closed in an earlier boot).
        let (ops, baselines) = fixtures::schema_1_journal();
        let journal = Journal::parse(&ops, &baselines);
        assert!(journal.unreadable.is_empty());
        let boot = BootId::parse(BOOT).unwrap();
        let summary =
            mklm_client::startup::summarize(&journal, boot, &|_| mklm_core::Liveness::Dead);
        let settings = Settings::default();
        let screen = main_screen(&MainInput {
            snapshot: &fixtures::dev_machine(),
            journal: Some((&journal, boot)),
            summary: &summary,
            settings: &settings,
            active_hkl: 0x0411_0411,
            highlighted: None,
            lang: Lang::Ja,
        });
        assert_eq!(screen.status.banner, "");
        assert!(
            screen
                .rows
                .iter()
                .all(|row| row.can_assign && !row.apply_now && row.pending.is_empty())
        );
    }
}
