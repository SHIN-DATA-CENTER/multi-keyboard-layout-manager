//! The status line of the main screen (plan 3.2, design m3 B.2): input method (the GUI thread's
//! active HKL), mode, standard layout, the sign-in screen's input method, and one banner for the
//! most important journal attention (design m3 B.12).

use mklm_client::startup::StartupSummary;
use mklm_core::{Assessment, Attention, InputMethods, klid_has_japanese_layout};

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

pub fn status_line(
    assessment: &Assessment,
    input: &InputMethods,
    active_hkl: u32,
    summary: &StartupSummary,
    lang: Lang,
) -> StatusLine {
    let (input_method, ok) = i18n::input_method(active_hkl, lang);
    let sign_in_klid = input.sign_in_preload.first();
    let sign_in_ok = sign_in_klid.is_some_and(|klid| klid_has_japanese_layout(klid));
    // Language names, never raw KLIDs (review U5).
    let sign_in = match (sign_in_klid, sign_in_ok, lang) {
        (Some(_), true, Lang::Ja) => "日本語 ✓".to_string(),
        (Some(_), true, Lang::En) => "Japanese ✓".to_string(),
        (Some(klid), false, Lang::Ja) => format!(
            "{} ⚠ サインイン画面では、すべてのキーボードが US 配列になります",
            i18n::klid_name(klid, lang)
        ),
        (Some(klid), false, Lang::En) => format!(
            "{} ⚠ at the sign-in screen every keyboard types US",
            i18n::klid_name(klid, lang)
        ),
        (None, _, Lang::Ja) => "不明".to_string(),
        (None, _, Lang::En) => "unknown".to_string(),
    };
    let standard = i18n::table(&assessment.standard_layout, lang);
    let mode_note = match (assessment.mode, lang) {
        (mklm_core::GlobalMode::Fixed, Lang::Ja) => format!(
            "すべてのキーボードが {standard} として動きます。ほかの配列のキーボードは、その行の［変更…］で変えられます"
        ),
        (mklm_core::GlobalMode::Fixed, Lang::En) => format!(
            "Every keyboard types {standard}. Change another keyboard with \"Change…\" on its row"
        ),
        (mklm_core::GlobalMode::PerKeyboard, _) => String::new(),
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
        line.banner = match lang {
            Lang::Ja => "記録（ジャーナル）を読めません。MKLM を更新するまで変更できません".into(),
            Lang::En => {
                "The journal cannot be read; nothing can be changed until MKLM is updated".into()
            }
        };
        line.banner_tone = Tone::Danger;
        return line;
    }
    let Some(attention) = PRIORITY
        .into_iter()
        .find(|attention| summary.with(*attention).next().is_some())
    else {
        return line;
    };
    let post_reboot = attention == Attention::Recover && summary.post_reboot_due();
    line.banner = if post_reboot {
        match lang {
            Lang::Ja => "PC の再起動後の確認が必要です".into(),
            Lang::En => "Check the keyboards after the restart".into(),
        }
    } else {
        i18n::attention(attention, lang).unwrap_or_default()
    };
    line.banner_tone = match attention {
        Attention::Recover | Attention::Conflict => Tone::Danger,
        Attention::NeedsApply | Attention::Busy => Tone::Info,
        _ => Tone::Warning,
    };
    let (target, ja, en) = match attention {
        _ if post_reboot => (Some(BannerTarget::PostReboot), "確認…", "Check…"),
        Attention::Recover => (Some(BannerTarget::Recovery), "回復…", "Recover…"),
        Attention::Conflict => (Some(BannerTarget::Conflict), "確認…", "Review…"),
        Attention::AwaitingUser => (Some(BannerTarget::Recovery), "確認…", "Review…"),
        Attention::WaitingForReboot => (Some(BannerTarget::Restart), "再起動…", "Restart…"),
        Attention::NeedsApply => (Some(BannerTarget::ApplyNow), "今すぐ反映…", "Apply now…"),
        _ => (None, "", ""),
    };
    line.banner_target = target;
    line.banner_action = match lang {
        Lang::Ja => ja.into(),
        Lang::En => en.into(),
    };
    line
}

impl SnapshotText for StatusLine {
    fn snapshot_text(&self) -> String {
        format!(
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
        )
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::{assess, fixtures};

    use super::*;

    #[test]
    fn the_dev_machine() {
        let snapshot = fixtures::dev_machine();
        let assessment = assess(&snapshot);
        let line = status_line(
            &assessment,
            &snapshot.input,
            0x0411_0411,
            &StartupSummary::default(),
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
}
