//! The status line of the main screen (plan 3.2, design m3 B.2): input method (the GUI thread's
//! active HKL), mode, standard layout, the sign-in screen's input method, and one banner for the
//! most important journal attention (design m3 B.12).
//!
//! Also what the journal means for the keyboard rows: why new operations are blocked
//! ([`blocked_reason`]), which keyboards "今すぐ反映…" can put into effect ([`needs_apply`]) and
//! which wait for the PC restart ([`restart_waits`]).

use mklm_client::startup::StartupSummary;
use mklm_core::{
    Assessment, Attention, BootId, GlobalMode, InputMethods, Journal, KeyboardDriver, OpKind,
    OpState, PendingAction, SystemSnapshot, WriteTarget, apply_pending_cleared, can_live_reset,
};

use super::{SnapshotText, Tone};
use crate::i18n::{self, Lang};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusLine {
    pub input_method: String,
    pub input_method_tone: Tone,
    pub mode: String,
    pub standard: String,
    pub sign_in: String,
    pub sign_in_tone: Tone,
    /// Fixed mode: what it means and the way out (review U4); empty otherwise.
    pub mode_note: String,
    pub banner: String,
    pub banner_tone: Tone,
    /// The banner's button, e.g. "回復…"; empty when none.
    pub banner_action: String,
    /// What the banner's button does.
    pub banner_target: Option<BannerTarget>,
}

/// Where the banner's button leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerTarget {
    Recovery,
    Conflict,
    Restart,
    PostReboot,
    /// "今すぐ反映…": `Request::Recover` with the apply method (review U8).
    ApplyNow,
}

/// Attentions in the order the banner picks them.
const PRIORITY: [Attention; 6] = [
    Attention::Recover,
    Attention::Conflict,
    Attention::AwaitingUser,
    Attention::WaitingForReboot,
    Attention::Busy,
    Attention::NeedsApply,
];

/// What the journal still has to put into effect in this boot (design m3 B.2, B.12
/// `NeedsApply`), without what Raw Input already reports (`apply_pending_cleared`: the banner
/// and the rows never ask for something that is done).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NeedsApply {
    /// Keyboards (instance IDs) that `Request::Recover` with a live reset puts into effect now
    /// (design m2 D.7 step 4): listed by an `apply_pending` reset or reconnect of this boot, not
    /// reporting their stored type yet, connected and resettable in place (`can_live_reset`).
    /// "今すぐ反映…" is offered for them, and only for them (review U8).
    pub apply_now: Vec<String>,
    /// Some change takes effect only at the PC restart (`RevertedPendingReboot` of this boot, an
    /// `apply_pending` restart, or a PS/2 keyboard).
    pub restart: bool,
    /// Some keyboard must be unplugged or reconnected by the user (Bluetooth, a keyboard that is
    /// not connected, or another one that cannot be reset in place).
    pub reconnect: bool,
}

impl NeedsApply {
    /// Nothing is left to put into effect: no `NeedsApply` banner.
    pub fn is_empty(&self) -> bool {
        self.apply_now.is_empty() && !self.restart && !self.reconnect
    }
}

/// [`NeedsApply`] of the `NeedsApply` entries of `summary`, judged with the display snapshot.
/// Raw Input "reports the stored type" exactly as the engine's housekeeping says it (connected,
/// reporting, and equal to the type the stored values predict).
pub fn needs_apply(
    journal: &Journal,
    boot: BootId,
    summary: &StartupSummary,
    snapshot: &SystemSnapshot,
) -> NeedsApply {
    let keyboard = |id: &str| {
        snapshot
            .keyboards
            .iter()
            .find(|kb| kb.instance_id.eq_ignore_ascii_case(id))
    };
    let reports_stored_type = |id: &str| {
        keyboard(id).is_some_and(|kb| {
            kb.present
                && kb.reported_type.is_some()
                && kb.reported_type == kb.predicted_type(&snapshot.global)
        })
    };
    let mut result = NeedsApply::default();
    for item in summary.with(Attention::NeedsApply) {
        let Some(entry) = journal
            .entries
            .iter()
            .find(|entry| entry.op_id == item.op.op_id)
        else {
            continue;
        };
        if entry.state == OpState::RevertedPendingReboot && entry.boot_id == boot {
            result.restart = true;
        }
        let Some(pending) = &entry.apply_pending else {
            continue;
        };
        if apply_pending_cleared(pending, boot, &reports_stored_type) {
            continue;
        }
        match pending.action {
            PendingAction::RestartPc => result.restart = true,
            PendingAction::ResetKeyboard | PendingAction::Reconnect => {
                for id in pending
                    .instance_ids
                    .iter()
                    .filter(|id| !reports_stored_type(id))
                {
                    match keyboard(id) {
                        Some(kb) if can_live_reset(kb, false) => {
                            if !result
                                .apply_now
                                .iter()
                                .any(|known| known.eq_ignore_ascii_case(id))
                            {
                                result.apply_now.push(id.clone());
                            }
                        }
                        // A PS/2 keyboard is neither reset nor unplugged: the PC restart
                        // (design m3 B.12 "内蔵は PC の再起動").
                        Some(kb) if kb.driver == KeyboardDriver::I8042prt => result.restart = true,
                        _ => result.reconnect = true,
                    }
                }
            }
        }
    }
    result
}

/// Keyboards whose stored values the journal leaves to the PC restart in this boot: a
/// `PendingReboot` or `RevertedPendingReboot` entry of this boot, or an `apply_pending` restart.
/// A row that is not in effect yet then says "PC の再起動が必要" — not the reset or unplug its
/// device would allow — as the banner does (design m3 B.2, B.12).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestartWaits {
    /// A global value waits (a migration): every keyboard.
    pub all: bool,
    /// Instance IDs.
    pub keyboards: Vec<String>,
}

impl RestartWaits {
    pub fn covers(&self, instance_id: &str) -> bool {
        self.all
            || self
                .keyboards
                .iter()
                .any(|id| id.eq_ignore_ascii_case(instance_id))
    }
}

/// [`RestartWaits`] of `journal` in `boot`.
pub fn restart_waits(journal: &Journal, boot: BootId) -> RestartWaits {
    let mut waits = RestartWaits::default();
    for entry in &journal.entries {
        let restart_state = entry.boot_id == boot
            && matches!(
                entry.state,
                OpState::PendingReboot | OpState::RevertedPendingReboot
            );
        if restart_state {
            for record in &entry.records {
                match &record.target {
                    WriteTarget::Device { instance_id } => {
                        waits.keyboards.push(instance_id.clone());
                    }
                    WriteTarget::Global => waits.all = true,
                }
            }
            match &entry.kind {
                OpKind::SetLayout { instance_ids, .. } => {
                    waits.keyboards.extend(instance_ids.iter().cloned());
                }
                OpKind::Migrate { .. } => waits.all = true,
                OpKind::RestoreBaseline { .. } | OpKind::Cleanup { .. } => {}
            }
        }
        if let Some(pending) = &entry.apply_pending
            && pending.since == boot
            && pending.action == PendingAction::RestartPc
        {
            waits.keyboards.extend(pending.instance_ids.iter().cloned());
        }
    }
    waits
}

/// True when every entry with `attention` waits for the post-reboot check (design m3 B.9): the
/// banner and the rows then speak of that check, not of a recovery or a plain keep-or-revert.
fn post_reboot_only(summary: &StartupSummary, attention: Attention) -> bool {
    matches!(attention, Attention::Recover | Attention::AwaitingUser)
        && summary.post_reboot_due()
        && summary.with(attention).all(|item| {
            summary
                .post_reboot
                .iter()
                .any(|op| op.op_id == item.op.op_id)
        })
}

/// The attention the banner shows: the first of [`PRIORITY`] that some entry has; `NeedsApply`
/// only while something is left to put into effect.
fn top_attention(summary: &StartupSummary, needs_apply: &NeedsApply) -> Option<Attention> {
    PRIORITY.into_iter().find(|attention| {
        summary.with(*attention).next().is_some()
            && (*attention != Attention::NeedsApply || !needs_apply.is_empty())
    })
}

/// Where the banner's button leads, if it has one (design m3 B.12). `None` also for an
/// unreadable journal: nothing helps but updating MKLM.
pub fn banner_target(summary: &StartupSummary, needs_apply: &NeedsApply) -> Option<BannerTarget> {
    if summary.unreadable > 0 {
        return None;
    }
    let attention = top_attention(summary, needs_apply)?;
    if post_reboot_only(summary, attention) {
        return Some(BannerTarget::PostReboot);
    }
    match attention {
        Attention::Recover | Attention::AwaitingUser => Some(BannerTarget::Recovery),
        Attention::Conflict => Some(BannerTarget::Conflict),
        Attention::WaitingForReboot => Some(BannerTarget::Restart),
        Attention::NeedsApply if !needs_apply.apply_now.is_empty() => Some(BannerTarget::ApplyNow),
        Attention::NeedsApply if needs_apply.restart => Some(BannerTarget::Restart),
        Attention::NeedsApply | Attention::Busy | Attention::None => None,
    }
}

/// Why new operations are blocked now, for the rows (design m3 B.2, E.2: shown on the row and
/// read with it); `None` while they are not (`StartupSummary::blocks_writes`).
pub fn blocked_reason(summary: &StartupSummary, lang: Lang) -> Option<String> {
    if summary.unreadable > 0 {
        return Some(i18n::cannot_change_now(None, lang));
    }
    let attention = PRIORITY
        .into_iter()
        .filter(|attention| attention.blocks_writes())
        .find(|attention| summary.with(*attention).next().is_some())?;
    Some(if post_reboot_only(summary, attention) {
        i18n::cannot_change_before_post_reboot_check(lang)
    } else {
        i18n::cannot_change_now(Some(attention), lang)
    })
}

pub fn status_line(
    assessment: &Assessment,
    input: &InputMethods,
    active_hkl: u32,
    summary: &StartupSummary,
    needs_apply: &NeedsApply,
    lang: Lang,
) -> StatusLine {
    let (input_method, ok) = i18n::input_method(active_hkl, lang);
    // Language names, never raw KLIDs (review U5).
    let (sign_in, sign_in_ok) =
        i18n::sign_in_status(input.sign_in_preload.first().map(String::as_str), lang);
    let standard = i18n::table(&assessment.standard_layout, lang);
    let mode_note = match assessment.mode {
        GlobalMode::Fixed => i18n::fixed_mode_note(&standard, lang),
        GlobalMode::PerKeyboard => String::new(),
    };
    let mut line = StatusLine {
        mode_note,
        input_method,
        input_method_tone: if ok { Tone::Success } else { Tone::Warning },
        mode: i18n::mode(assessment.mode, lang),
        standard,
        sign_in,
        sign_in_tone: if sign_in_ok {
            Tone::Success
        } else {
            Tone::Warning
        },
        ..StatusLine::default()
    };
    if summary.unreadable > 0 {
        line.banner = i18n::journal_unreadable_banner(lang);
        line.banner_tone = Tone::Danger;
        return line;
    }
    let Some(attention) = top_attention(summary, needs_apply) else {
        return line;
    };
    let target = banner_target(summary, needs_apply);
    line.banner = match (target, attention) {
        (Some(BannerTarget::PostReboot), _) => i18n::post_reboot_banner(lang),
        (None, Attention::NeedsApply) => i18n::needs_reconnect(lang),
        _ => i18n::attention(attention, lang).unwrap_or_default(),
    };
    line.banner_tone = match attention {
        Attention::Recover | Attention::Conflict => Tone::Danger,
        Attention::NeedsApply | Attention::Busy => Tone::Info,
        _ => Tone::Warning,
    };
    line.banner_target = target;
    line.banner_action = target
        .map(|target| i18n::banner_action(target, attention, lang))
        .unwrap_or_default();
    line
}

impl SnapshotText for StatusLine {
    fn snapshot_text(&self) -> String {
        let mut text = format!(
            "input: {} ({:?})\nmode: {}\nstandard: {}\nsign-in: {} ({:?})\nbanner: {} ({:?}) [{}]\n",
            self.input_method,
            self.input_method_tone,
            self.mode,
            self.standard,
            self.sign_in,
            self.sign_in_tone,
            self.banner,
            self.banner_tone,
            self.banner_action
        );
        if !self.mode_note.is_empty() {
            text.push_str(&format!("mode note: {}\n", self.mode_note));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use mklm_client::gate::OpRef;
    use mklm_client::startup::AttentionItem;
    use mklm_core::{OpId, OpKind, assess, fixtures};

    use super::*;

    fn line(summary: &StartupSummary, needs_apply: &NeedsApply, lang: Lang) -> StatusLine {
        let snapshot = fixtures::dev_machine();
        status_line(
            &assess(&snapshot),
            &snapshot.input,
            0x0411_0411,
            summary,
            needs_apply,
            lang,
        )
    }

    fn item(n: u32, state: OpState, attention: Attention) -> AttentionItem {
        AttentionItem {
            op: OpRef {
                op_id: OpId::parse(&format!("{n:08x}-0000-4000-8000-000000000000")).unwrap(),
                kind: OpKind::SetLayout {
                    requested: fixtures::keychron().instance_id,
                    instance_ids: vec![fixtures::keychron().instance_id],
                    layout: mklm_core::LayoutChoice::Jis,
                },
                state,
            },
            attention,
        }
    }

    #[test]
    fn the_dev_machine() {
        let snapshot = fixtures::dev_machine();
        let assessment = assess(&snapshot);
        let line = status_line(
            &assessment,
            &snapshot.input,
            0x0411_0411,
            &StartupSummary::default(),
            &NeedsApply::default(),
            Lang::Ja,
        );
        assert_eq!(
            line.snapshot_text(),
            "input: 日本語 IME ✓ (Success)\nmode: キーボードごと\nstandard: JIS\n\
             sign-in: 日本語 ✓ (Success)\nbanner:  (Neutral) []\n"
        );
        let english = status_line(
            &assessment,
            &snapshot.input,
            0x0409_0409,
            &StartupSummary::default(),
            &NeedsApply::default(),
            Lang::En,
        );
        assert_eq!(english.input_method_tone, Tone::Warning);
        assert_eq!(english.mode, "Per keyboard");
        assert_eq!(line.mode_note, "");
    }

    #[test]
    fn a_fixed_mode_pc_with_an_english_sign_in_screen() {
        let mut snapshot = fixtures::dev_machine();
        snapshot.global = fixtures::global_fixed_jis();
        snapshot.input.sign_in_preload = vec!["00000409".into()];
        let assessment = assess(&snapshot);
        let line = status_line(
            &assessment,
            &snapshot.input,
            0x0411_0411,
            &StartupSummary::default(),
            &NeedsApply::default(),
            Lang::Ja,
        );
        // Language names instead of KLIDs (review U5); the fixed mode says what it means
        // (review U4).
        assert_eq!(
            line.sign_in,
            "英語 (US) ⚠ サインイン画面では、すべてのキーボードが US 配列になります"
        );
        assert_eq!(
            line.mode_note,
            "すべてのキーボードが JIS として動きます。ほかの配列のキーボードは、その行の［変更…］で変えられます"
        );
        for text in [&line.sign_in, &line.mode_note, &line.input_method] {
            assert!(crate::vm::unexpected_latin(text, &[]).is_empty(), "{text}");
        }
    }

    #[test]
    fn banner_buttons_follow_the_attention() {
        let none = NeedsApply::default();
        let summary = |items: Vec<AttentionItem>| StartupSummary {
            items,
            ..StartupSummary::default()
        };
        // Recovery before everything else; the button's name says what it does.
        let recover = summary(vec![
            item(2, OpState::AwaitingConfirm, Attention::AwaitingUser),
            item(1, OpState::Written, Attention::Recover),
        ]);
        let ja = line(&recover, &none, Lang::Ja);
        assert_eq!(ja.banner_target, Some(BannerTarget::Recovery));
        assert_eq!(ja.banner_action, "回復…");
        assert_eq!(ja.banner_tone, Tone::Danger);
        let waiting = summary(vec![item(
            1,
            OpState::AwaitingConfirm,
            Attention::AwaitingUser,
        )]);
        assert_eq!(line(&waiting, &none, Lang::En).banner_action, "Review…");
        let reboot = summary(vec![item(
            1,
            OpState::PendingReboot,
            Attention::WaitingForReboot,
        )]);
        let ja = line(&reboot, &none, Lang::Ja);
        assert_eq!(
            (ja.banner.as_str(), ja.banner_action.as_str()),
            ("PC の再起動を待っている変更があります", "再起動…")
        );
        let busy = summary(vec![item(1, OpState::Written, Attention::Busy)]);
        let ja = line(&busy, &none, Lang::Ja);
        assert_eq!(ja.banner_target, None);
        assert_eq!(ja.banner_action, "");
        // A PendingReboot seen from a new boot is the post-reboot check, not a recovery.
        let mut post_reboot = summary(vec![item(1, OpState::PendingReboot, Attention::Recover)]);
        post_reboot.post_reboot = vec![post_reboot.items[0].op.clone()];
        let ja = line(&post_reboot, &none, Lang::Ja);
        assert_eq!(ja.banner_target, Some(BannerTarget::PostReboot));
        assert_eq!(ja.banner, "PC の再起動後の確認が必要です");
        assert_eq!(
            blocked_reason(&post_reboot, Lang::Ja).as_deref(),
            Some("今は変更できません: PC の再起動後の確認が終わっていません")
        );
    }

    #[test]
    fn needs_apply_shows_only_what_is_left() {
        let summary = StartupSummary {
            items: vec![item(1, OpState::Confirmed, Attention::NeedsApply)],
            ..StartupSummary::default()
        };
        // Everything already in effect: no banner at all (apply_pending_cleared).
        let line_empty = line(&summary, &NeedsApply::default(), Lang::Ja);
        assert_eq!(line_empty.banner, "");
        let apply_now = NeedsApply {
            apply_now: vec![fixtures::keychron().instance_id],
            ..NeedsApply::default()
        };
        let ja = line(&summary, &apply_now, Lang::Ja);
        assert_eq!(ja.banner_target, Some(BannerTarget::ApplyNow));
        assert_eq!(
            (ja.banner.as_str(), ja.banner_action.as_str()),
            ("まだ反映されていない変更があります", "今すぐ反映…")
        );
        assert_eq!(ja.banner_tone, Tone::Info);
        // NeedsApply never blocks writes.
        assert_eq!(blocked_reason(&summary, Lang::Ja), None);
        let restart = NeedsApply {
            restart: true,
            ..NeedsApply::default()
        };
        assert_eq!(
            line(&summary, &restart, Lang::En).banner_target,
            Some(BannerTarget::Restart)
        );
        let reconnect = NeedsApply {
            reconnect: true,
            ..NeedsApply::default()
        };
        let en = line(&summary, &reconnect, Lang::En);
        assert_eq!(en.banner_target, None);
        assert!(
            en.banner.starts_with("A change is not in effect yet."),
            "{en:?}"
        );
        let ja = line(&summary, &reconnect, Lang::Ja);
        assert!(crate::vm::unexpected_latin(&ja.banner, &[]).is_empty());
    }

    #[test]
    fn needs_apply_sorts_the_keyboards_by_what_puts_them_into_effect() {
        const BOOT: &str = "9b1c0d6e-2f4a-4c8b-a1d3-5e6f7a8b9c0d";
        let boot = BootId::parse(BOOT).unwrap();
        // None of the three connected keyboards types what its stored values predict yet.
        let mut snapshot = fixtures::dev_machine();
        snapshot.keyboards[0].reported_type = Some(mklm_core::KeyboardType::US);
        snapshot.keyboards[1].overrides.keyboard_type_override = Some(7);
        snapshot.keyboards[1].overrides.keyboard_subtype_override = Some(2);
        snapshot.keyboards[2].reported_type = Some(mklm_core::KeyboardType::US);
        let ids: Vec<String> = snapshot.keyboards[..3]
            .iter()
            .map(|kb| kb.instance_id.clone())
            .collect();
        // A schema-1 entry of this boot, closed, whose `apply_pending` lists all three.
        let keychron = ids[1].replace('\\', "\\\\");
        let json = format!(
            r#"{{ "schema_version": 1, "op_id": "0000000a-0000-4000-8000-000000000000",
              "seq": 1, "kind": {{ "kind": "set-layout", "requested": "{keychron}",
                "instance_ids": ["{keychron}"], "layout": "jis" }},
              "state": "confirmed", "boot_id": "{BOOT}",
              "owner": {{ "pid": 1, "creation_time": 1 }}, "created_at": 1, "updated_at": 2,
              "apply": null, "countdown": null, "records": [], "context": [], "failure": null,
              "apply_pending": null, "history": [] }}"#
        );
        let mut entry = mklm_core::JournalEntry::from_json(&json).unwrap();
        entry.apply_pending = Some(mklm_core::ApplyPending {
            action: PendingAction::Reconnect,
            instance_ids: ids.clone(),
            since: boot,
        });
        let journal = Journal {
            entries: vec![entry.clone()],
            ..Journal::default()
        };
        let summary =
            mklm_client::startup::summarize(&journal, boot, &|_| mklm_core::Liveness::Dead);
        // USB: reset now; Bluetooth LE: reconnect; the built-in PS/2 keyboard: the PC restart.
        assert_eq!(
            needs_apply(&journal, boot, &summary, &snapshot),
            NeedsApply {
                apply_now: vec![ids[1].clone()],
                restart: true,
                reconnect: true,
            }
        );
        // Recorded in an earlier boot: every driver has read its values since.
        let mut earlier = entry;
        if let Some(pending) = &mut earlier.apply_pending {
            pending.since = BootId::parse("11112222-3333-4444-5555-666677778888").unwrap();
        }
        let journal = Journal {
            entries: vec![earlier],
            ..Journal::default()
        };
        let summary =
            mklm_client::startup::summarize(&journal, boot, &|_| mklm_core::Liveness::Dead);
        assert!(needs_apply(&journal, boot, &summary, &snapshot).is_empty());
    }

    #[test]
    fn blocked_reasons_name_the_blocking_attention() {
        let summary = |attention| StartupSummary {
            items: vec![item(1, OpState::Written, attention)],
            ..StartupSummary::default()
        };
        assert_eq!(
            blocked_reason(&summary(Attention::Busy), Lang::Ja).as_deref(),
            Some("今は変更できません: 別の MKLM が処理中です")
        );
        assert_eq!(
            blocked_reason(&summary(Attention::Recover), Lang::En).as_deref(),
            Some("Cannot change now: an interrupted operation needs recovery")
        );
        assert_eq!(blocked_reason(&StartupSummary::default(), Lang::Ja), None);
        let unreadable = StartupSummary {
            unreadable: 1,
            ..StartupSummary::default()
        };
        assert_eq!(
            blocked_reason(&unreadable, Lang::Ja).as_deref(),
            Some("今は変更できません: 記録（ジャーナル）を読めません。MKLM を更新してください")
        );
    }
}
