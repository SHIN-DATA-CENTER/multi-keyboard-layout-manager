//! Languages and the Rust side of the translations (design m3 D).
//!
//! Two catalogues with one rule each:
//! - Static labels of the .slint files use `@tr("English")`, translated by the bundled
//!   `translations/ja/LC_MESSAGES/mklm.po` and switched with `slint::select_bundled_translation`.
//! - Everything Rust composes (states, reasons, results, errors, badges) comes from the typed
//!   functions here: an exhaustive `match` per type, so a new variant does not compile until it
//!   has both texts, and tests can snapshot the output without a window.
//!
//! `mklm-core` / `mklm-engine` messages (`ErrorInfo::message`, warnings) are English diagnostics;
//! the GUI shows them only under "technical details". Mapping typed values to text is this
//! module's job alone (design m3 D.4).

use mklm_client::describe::ResetPhase;
use mklm_client::gate::BlockReason;
use mklm_client::outcome::HelperExitKind;
use mklm_client::{HelperExit, LaunchError, LaunchFailure, OutcomeClass};
use mklm_core::{
    Attention, EffectiveLayout, ErrorCode, FailureReason, GlobalMode, KeyboardType, Layout,
    LayoutBasis, LayoutChoice, LayoutTable, OpState, OperationError, PendingAction, RegValue,
    Transport,
};
use serde::{Deserialize, Serialize};

use crate::detect::{Step, Verdict};

/// The restart, post-reboot, conflict, history and recovery pages (WP-U4, WP-U5).
pub mod journal_pages;

/// The first-run wizard (WP-U2).
pub mod wizard;

/// A language the GUI speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    /// The name `slint::select_bundled_translation` takes ("" is the source language, English).
    pub fn bundle(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "",
        }
    }

    /// The UI font (plan 3.8).
    pub fn font_family(self) -> &'static str {
        match self {
            Lang::Ja => "Yu Gothic UI",
            Lang::En => "Segoe UI",
        }
    }
}

/// The language setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LangChoice {
    Ja,
    En,
    /// Follow the Windows display language: Japanese when it is Japanese, else English.
    #[default]
    System,
}

impl LangChoice {
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "ja" => Some(Self::Ja),
            "en" => Some(Self::En),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// The settings ComboBox index (0 = 日本語, 1 = English, 2 = system).
    pub fn from_index(index: i32) -> Self {
        match index {
            0 => Self::Ja,
            1 => Self::En,
            _ => Self::System,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::Ja => 0,
            Self::En => 1,
            Self::System => 2,
        }
    }

    /// `ui_language` is `GetUserDefaultUILanguage` (primary language 0x11 = Japanese).
    pub fn resolve(self, ui_language: u16) -> Lang {
        match self {
            Self::Ja => Lang::Ja,
            Self::En => Lang::En,
            Self::System if ui_language & 0x3FF == 0x11 => Lang::Ja,
            Self::System => Lang::En,
        }
    }
}

fn pick(lang: Lang, ja: &str, en: &str) -> String {
    match lang {
        Lang::Ja => ja.to_string(),
        Lang::En => en.to_string(),
    }
}

/// "キーボードごと" / "固定" (glossary).
pub fn mode(mode: GlobalMode, lang: Lang) -> String {
    match mode {
        GlobalMode::PerKeyboard => pick(lang, "キーボードごと", "Per keyboard"),
        GlobalMode::Fixed => pick(lang, "固定", "Fixed"),
    }
}

/// A layout table: JIS, US, or the layer driver's name.
pub fn table(table: &LayoutTable, lang: Lang) -> String {
    match table {
        LayoutTable::Jis => "JIS".to_string(),
        LayoutTable::Us => "US".to_string(),
        LayoutTable::Other(dll) => match lang {
            Lang::Ja => format!("その他（{dll}）"),
            Lang::En => format!("other ({dll})"),
        },
    }
}

/// What a cleanup did, for the history (design m3 A.5, B.1 "削除する"): the values of
/// `keyboard` (a display name) that its driver does not read were deleted.
pub fn cleanup_text(keyboard: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{keyboard} のドライバーが読まない値を削除"),
        Lang::En => format!("Remove the values {keyboard}'s driver does not read"),
    }
}

/// What a keyboard types and why: "JIS", "標準に従う（JIS）", "固定モード（JIS）".
pub fn effective(layout: &EffectiveLayout, lang: Lang) -> String {
    let name = table(&layout.table, lang);
    match layout.basis {
        LayoutBasis::KeyboardType => name,
        LayoutBasis::Standard => match lang {
            Lang::Ja => format!("標準に従う（{name}）"),
            Lang::En => format!("follows the standard ({name})"),
        },
        LayoutBasis::FixedMode => match lang {
            Lang::Ja => format!("固定モード（{name}）"),
            Lang::En => format!("fixed mode ({name})"),
        },
    }
}

/// "保存済み（反映待ち: …が必要）" (glossary "状態表示").
pub fn pending(action: PendingAction, lang: Lang) -> String {
    match action {
        PendingAction::ResetKeyboard => pick(
            lang,
            "保存済み（反映待ち: キーボードのリセットが必要）",
            "Saved (not in effect yet: the keyboard must be reset)",
        ),
        PendingAction::Reconnect => pick(
            lang,
            "保存済み（反映待ち: 抜き差しか再接続が必要）",
            "Saved (not in effect yet: unplug and replug, or reconnect)",
        ),
        PendingAction::RestartPc => pick(
            lang,
            "保存済み（反映待ち: PC の再起動が必要）",
            "Saved (not in effect yet: restart the PC)",
        ),
    }
}

/// What the user can do about a saved change that is not in effect yet (design m3 B.2, B.12):
/// the second line under "保存済み（反映待ち: …）", which keeps the glossary wording.
pub fn pending_hint(action: PendingAction, lang: Lang) -> String {
    match action {
        PendingAction::ResetKeyboard => pick(
            lang,
            "抜き差しするか、［今すぐ反映…］を押してください",
            "Unplug and replug it, or choose \"Apply now…\"",
        ),
        PendingAction::Reconnect => pick(
            lang,
            "抜き差ししてください（Bluetooth は電源をオフにしてからオンにします）",
            "Unplug and replug it (Bluetooth: turn it off and on)",
        ),
        PendingAction::RestartPc => pick(
            lang,
            "PC を再起動してください（シャットダウンではなく再起動）",
            "Restart the PC (Restart, not Shut down)",
        ),
    }
}

/// The hint under a row's "保存済み（反映待ち: …）" when the row offers no "今すぐ反映…" (the
/// journal holds no reset MKLM could redo for it, or new operations are blocked; review U8): it
/// never points at a button the row does not have.
pub fn pending_hint_without_apply_now(action: PendingAction, lang: Lang) -> String {
    match action {
        PendingAction::ResetKeyboard => pick(
            lang,
            "抜き差しすると反映されます",
            "Unplug and replug it to put it into effect",
        ),
        PendingAction::Reconnect | PendingAction::RestartPc => pending_hint(action, lang),
    }
}

/// The banner of a change that only an unplug or a reconnect puts into effect (design m3
/// B.12 `NeedsApply` without "今すぐ反映…": Bluetooth, or a keyboard that is not connected).
pub fn needs_reconnect(lang: Lang) -> String {
    pick(
        lang,
        "まだ反映されていない変更があります。キーボードを抜き差しするか、接続し直してください（Bluetooth は電源をオフにしてからオンにします）。",
        "A change is not in effect yet. Unplug and replug the keyboard, or reconnect it (Bluetooth: turn it off and on).",
    )
}

/// The polite announcement when "キーを押して特定" marks a row (design m3 B.3, E.3).
pub fn key_pressed_on(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} のキーが押されました"),
        Lang::En => format!("A key was pressed on {name}"),
    }
}

/// Names in a sentence: "Keychron Receiver、VXE R1SE+" / "Keychron Receiver, VXE R1SE+".
pub fn name_list(names: &[String], lang: Lang) -> String {
    names.join(match lang {
        Lang::Ja => "、",
        Lang::En => ", ",
    })
}

/// How a change takes effect, as a sentence for the change page (plan 3.5; design m3 B.5).
/// `seconds` is the keep-or-revert time (20, or 60 with the accessibility setting, design m3
/// B.6).
pub fn takes_effect(action: PendingAction, seconds: u32, lang: Lang) -> String {
    match action {
        PendingAction::ResetKeyboard => match lang {
            Lang::Ja => format!(
                "キーボードをその場でリセットします（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。数秒間このキーボードで入力できません。その後 {seconds} 秒以内に［このままにする］を選ばないと元に戻ります。"
            ),
            Lang::En => format!(
                "The keyboard is reset in place (Windows reconnects it; the keyboard's own settings do not change); it cannot type for a few seconds. Then choose \"Keep\" within {seconds} seconds, or it reverts."
            ),
        },
        PendingAction::Reconnect => pick(
            lang,
            "キーボードを抜き差しするか、再接続すると反映されます（Bluetooth は電源をオフにしてからオンにします）。",
            "It takes effect when the keyboard reconnects: unplug and replug it (Bluetooth: turn it off and on).",
        ),
        PendingAction::RestartPc => pick(
            lang,
            "PC を再起動すると反映されます（シャットダウンではなく再起動）。再起動するまで、MKLM でほかの変更はできません。",
            "It takes effect when the PC restarts (Restart, not Shut down). Until then, MKLM cannot make other changes.",
        ),
    }
}

/// A keyboard layout ID (KLID) for people: "日本語", "英語 (US)", "その他の言語（xxxxxxxx）"
/// (design m3 B.2: no raw KLIDs on decision screens).
pub fn klid_name(klid: &str, lang: Lang) -> String {
    if mklm_core::klid_has_japanese_layout(klid) {
        pick(lang, "日本語", "Japanese")
    } else if klid.eq_ignore_ascii_case("00000409") {
        pick(lang, "英語 (US)", "English (US)")
    } else {
        match lang {
            Lang::Ja => format!("その他の言語（{klid}）"),
            Lang::En => format!("another language ({klid})"),
        }
    }
}

/// Why "変更…" is disabled on a row (design m3 E.2): `None` is an unreadable journal.
pub fn cannot_change_now(attention: Option<Attention>, lang: Lang) -> String {
    let (ja, en) = match attention {
        None => (
            "今は変更できません: 記録（ジャーナル）を読めません。MKLM を更新してください",
            "Cannot change now: the journal cannot be read; update MKLM",
        ),
        Some(Attention::WaitingForReboot) => (
            "今は変更できません: PC の再起動を待っている変更があります",
            "Cannot change now: a change waits for a PC restart",
        ),
        Some(Attention::Recover) => (
            "今は変更できません: 回復が必要な操作があります",
            "Cannot change now: an interrupted operation needs recovery",
        ),
        Some(Attention::AwaitingUser) => (
            "今は変更できません: 確認待ちの変更があります",
            "Cannot change now: a change waits for keep or revert",
        ),
        Some(Attention::Conflict) => (
            "今は変更できません: MKLM 以外による変更の確認が必要です",
            "Cannot change now: values changed outside MKLM need a decision",
        ),
        Some(Attention::Busy) => (
            "今は変更できません: 別の MKLM が処理中です",
            "Cannot change now: another MKLM process is working",
        ),
        Some(Attention::None | Attention::NeedsApply) => ("", ""),
    };
    pick(lang, ja, en)
}

/// Why "変更…" is disabled while the post-reboot check is due (design m3 B.9): a `PendingReboot`
/// seen from a new boot needs the user's keep or revert on that page first.
pub fn cannot_change_before_post_reboot_check(lang: Lang) -> String {
    pick(
        lang,
        "今は変更できません: PC の再起動後の確認が終わっていません",
        "Cannot change now: the check after the PC restart is not done",
    )
}

// --- The main screen (design m3 B.2, B.12; WP-U1) ---

/// The sign-in screen's input method on the status line, and whether it types Japanese: a
/// language name, never a raw KLID (review U5). `None`: it could not be read.
pub fn sign_in_status(klid: Option<&str>, lang: Lang) -> (String, bool) {
    match klid {
        Some(klid) if mklm_core::klid_has_japanese_layout(klid) => {
            (pick(lang, "日本語 ✓", "Japanese ✓"), true)
        }
        Some(klid) => {
            let name = klid_name(klid, lang);
            let text = match lang {
                Lang::Ja => {
                    format!("{name} ⚠ サインイン画面では、すべてのキーボードが US 配列になります")
                }
                Lang::En => format!("{name} ⚠ at the sign-in screen every keyboard types US"),
            };
            (text, false)
        }
        None => (pick(lang, "不明", "unknown"), false),
    }
}

/// Under the status line in fixed mode: what it means and the way out (review U4). `standard`
/// is the layout's name ("JIS").
pub fn fixed_mode_note(standard: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "すべてのキーボードが {standard} として動きます。ほかの配列のキーボードは、その行の［変更…］で変えられます。"
        ),
        Lang::En => format!(
            "Every keyboard types {standard}. Change another keyboard with \"Change…\" on its row."
        ),
    }
}

/// The banner of an unreadable journal (design m3 B.2: before every other attention).
pub fn journal_unreadable_banner(lang: Lang) -> String {
    pick(
        lang,
        "記録（ジャーナル）を読めません。MKLM を更新するまで変更できません",
        "The journal cannot be read; nothing can be changed until MKLM is updated",
    )
}

/// The banner while the check after the PC restart is due (design m3 B.9).
pub fn post_reboot_banner(lang: Lang) -> String {
    pick(
        lang,
        "PC の再起動後の確認が必要です",
        "Check the keyboards after the restart",
    )
}

/// The banner's button (design m3 B.12): what it opens, in the words of the attention it is
/// about ("回復…" for an interrupted operation, "確認…" for a change that waits).
pub fn banner_action(
    target: crate::vm::status::BannerTarget,
    attention: Attention,
    lang: Lang,
) -> String {
    use crate::vm::status::BannerTarget;
    let (ja, en) = match target {
        BannerTarget::PostReboot => ("確認…", "Check…"),
        BannerTarget::Recovery if attention == Attention::Recover => ("回復…", "Recover…"),
        BannerTarget::Recovery | BannerTarget::Conflict => ("確認…", "Review…"),
        BannerTarget::Restart => ("再起動…", "Restart…"),
        BannerTarget::ApplyNow => ("今すぐ反映…", "Apply now…"),
    };
    pick(lang, ja, en)
}

/// "設定した配列" of a device whose collections are set differently (design m3 B.2).
pub fn assigned_mixed(lang: Lang) -> String {
    pick(
        lang,
        "混在（コレクションごとに違います）",
        "mixed (the collections differ)",
    )
}

/// The screen-reader part of a row that names the assigned layout: "設定した配列 JIS".
pub fn assigned_summary(assigned: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("設定した配列 {assigned}"),
        Lang::En => format!("assigned {assigned}"),
    }
}

/// The accessible label of a row's "変更…" button (design m3 E.2): "Keychron Receiver の配列を
/// 変更".
pub fn assign_label(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} の配列を変更"),
        Lang::En => format!("Change the layout of {name}"),
    }
}

/// The accessible label of a row's "今すぐ反映…" button (design m3 E.2): "Keychron Receiver の配列を
/// 今すぐ反映".
pub fn apply_now_label(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} の配列を今すぐ反映"),
        Lang::En => format!("Apply the layout of {name} now"),
    }
}

/// Why a keyboard types as it does, for "現在の動作" (glossary; review U4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// Not what the stored values predict: a warning.
    Unexpected,
    /// An explicitly assigned layout in effect: the only case with "✓ 設定どおり".
    AsSet,
    /// Fixed mode, as predicted: neutral.
    FixedMode,
    /// The PC's standard layout, as predicted: neutral.
    Standard,
}

/// "JIS として動作中 ✓ 設定どおり" and the like (design m3 B.2): "✓" only for [`Behavior::AsSet`].
pub fn current_behavior(layout: &LayoutTable, behavior: Behavior, lang: Lang) -> String {
    let name = table(layout, lang);
    match (behavior, lang) {
        (Behavior::Unexpected, Lang::Ja) => format!("{name} として動作中 ⚠"),
        (Behavior::Unexpected, Lang::En) => format!("types {name} ⚠"),
        (Behavior::AsSet, Lang::Ja) => format!("{name} として動作中 ✓ 設定どおり"),
        (Behavior::AsSet, Lang::En) => format!("types {name} ✓ as set"),
        (Behavior::FixedMode, Lang::Ja) => format!("{name} として動作中（固定モード）"),
        (Behavior::FixedMode, Lang::En) => format!("types {name} (fixed mode)"),
        (Behavior::Standard, Lang::Ja) => format!("{name} として動作中（PC の標準配列）"),
        (Behavior::Standard, Lang::En) => format!("types {name} (the PC's standard)"),
    }
}

/// "現在の動作" of a connected keyboard Windows reports no type for.
pub fn behavior_unknown(lang: Lang) -> String {
    pick(lang, "動作を確認できません", "cannot tell how it types")
}

/// "⚠ 実物は US 配列ですが JIS として動いています" (review U4): the physical layout MKLM learned
/// differs from how the keyboard types.
pub fn physical_differs(real: &LayoutTable, types: &LayoutTable, lang: Lang) -> String {
    let real = table(real, lang);
    let types = table(types, lang);
    match lang {
        Lang::Ja => format!("⚠ 実物は {real} 配列ですが {types} として動いています"),
        Lang::En => format!("⚠ It is a {real} keyboard but types {types}"),
    }
}

/// Parts of one screen-reader sentence (a row's `accessible-summary`), in reading order.
pub fn summary_join(parts: &[String], lang: Lang) -> String {
    name_list(parts, lang)
}

/// The "今すぐ反映…" page's title (design m3 B.12; review U8). `names`: the keyboards reset.
pub fn apply_now_title(names: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{names} の配列を今すぐ反映"),
        Lang::En => format!("Put the saved layout of {names} into effect"),
    }
}

/// What the "今すぐ反映…" page is about: the layout is saved, a reset puts it into effect.
pub fn apply_now_summary(names: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "{names} の配列は保存済みですが、まだ反映されていません。キーボードをリセットすると反映されます（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。"
        ),
        Lang::En => format!(
            "The layout of {names} is saved but not in effect yet. Resetting the keyboard puts it into effect (Windows reconnects it; the keyboard's own settings do not change)."
        ),
    }
}

/// The two ways of the "今すぐ反映…" page (design m3 B.5, B.12): `(text, detail)`. Resetting
/// has no countdown — the layout was kept already — and "not now" sends nothing.
pub fn apply_now_method(live: bool, lang: Lang) -> (String, String) {
    if live {
        (
            pick(
                lang,
                "今すぐキーボードをリセットして反映する",
                "Reset the keyboard now",
            ),
            pick(
                lang,
                "このキーボードは数秒間使えません。その間は、ほかのキーボードかマウスで操作します。",
                "This keyboard stops working for a few seconds; use another keyboard or the mouse meanwhile.",
            ),
        )
    } else {
        (
            pick(lang, "今はリセットしない", "Not now"),
            pick(
                lang,
                "MKLM は何もしません。キーボードを抜き差しするか、PC を再起動すると反映されます（シャットダウンではなく再起動）。",
                "MKLM does nothing now. Unplugging and replugging the keyboard, or restarting the PC (Restart, not Shut down), puts the layout into effect.",
            ),
        )
    }
}

/// Plan 1.4 on the "今すぐ反映…" page: only the keyboards being reset typed lately.
pub fn apply_now_only_keyboard(lang: Lang) -> String {
    pick(
        lang,
        "このキーボードは、最近入力のあった唯一のキーボードです。抜き差しするか、PC を再起動して反映することをおすすめします。",
        "This keyboard is the only one that typed recently. Unplugging and replugging it, or restarting the PC, is recommended instead.",
    )
}

/// The "今すぐ反映…" page after the change took effect meanwhile (replugged, or new operations
/// are blocked): nothing is left to send.
pub fn apply_now_nothing_left(lang: Lang) -> String {
    pick(
        lang,
        "反映を待っている変更はありません。",
        "No saved layout is waiting to be put into effect.",
    )
}

/// The "今すぐ反映…" page's main button; without "（次に Windows の確認が出ます）" when no
/// prompt follows (an elevated GUI, design m3 B.5).
pub fn apply_now_button(prompt: bool, lang: Lang) -> String {
    if prompt {
        pick(
            lang,
            "反映する（次に Windows の確認が出ます）",
            "Apply now (Windows asks next)",
        )
    } else {
        pick(lang, "反映する", "Apply now")
    }
}

/// The two ways a change can take effect (design m3 B.5; review U1): the choice replaces the
/// "another way to type" check box and says the consequence of each.
pub fn apply_method(live: bool, seconds: u32, lang: Lang) -> (String, String) {
    if live {
        match lang {
            Lang::Ja => (
                format!("すぐに切り替えて {seconds} 秒間試す"),
                "このキーボードは数秒間使えません。その間は、ほかのキーボードかマウスで操作します。".to_string(),
            ),
            Lang::En => (
                format!("Switch now and try it for {seconds} seconds"),
                "This keyboard stops working for a few seconds; use another keyboard or the mouse meanwhile.".to_string(),
            ),
        }
    } else {
        (
            pick(
                lang,
                "PC の再起動で切り替える",
                "Switch when the PC restarts",
            ),
            pick(
                lang,
                "再起動するまで、ほかのキーボードの配列も変更できません。",
                "Until the restart, no other keyboard can be changed either.",
            ),
        )
    }
}

/// USB / Bluetooth / BLE / PS/2 / I2C …
pub fn transport(transport: Transport, lang: Lang) -> String {
    match transport {
        Transport::Usb => "USB".to_string(),
        Transport::BluetoothClassic => "Bluetooth".to_string(),
        Transport::BluetoothLe => "Bluetooth LE".to_string(),
        Transport::Ps2 => "PS/2".to_string(),
        Transport::I2c => "I2C".to_string(),
        Transport::Spi => "SPI".to_string(),
        Transport::Virtual => pick(lang, "仮想", "virtual"),
        Transport::Unknown => pick(lang, "不明", "unknown"),
    }
}

/// The transport of the Remote Desktop keyboard (`KeyboardDevice::is_remote_desktop`): to the
/// user it is the keys the Remote Desktop client sends, not a "virtual" device.
pub fn remote_transport(lang: Lang) -> String {
    pick(lang, "リモート デスクトップ", "Remote Desktop")
}

/// "現在の動作" of the Remote Desktop keyboard. No layout is named: the session types with the key
/// table it started with, which MKLM cannot read (docs/research/rdp-keyboard.md).
pub fn remote_current(lang: Lang) -> String {
    pick(lang, "接続元の PC からの入力", "Input from the client PC")
}

/// What the Remote Desktop client reported about its keyboard (`OsInfo::client_keyboard_type`),
/// said as the client's report only, because the session may type with another table; `None`
/// for a type this has no name for.
pub fn remote_client_report(reported: KeyboardType, lang: Lang) -> Option<String> {
    let keyboard = match (reported.ty, reported.subtype) {
        (7, 2) => pick(lang, "日本語キーボード (JIS)", "a Japanese keyboard (JIS)"),
        (7, _) => pick(lang, "日本語キーボード", "a Japanese keyboard"),
        (4, _) => pick(
            lang,
            "英語キーボード (101/102 キー)",
            "an English keyboard (101/102 keys)",
        ),
        _ => return None,
    };
    Some(match lang {
        Lang::Ja => {
            format!("接続元の報告: {keyboard}。このセッションのキーの割り当てと同じとは限りません")
        }
        Lang::En => format!("The client reports {keyboard}; this session's key table may differ"),
    })
}

/// The badges of a keyboard row (plan 3.2; design m3 B.2, E.3): `(label, screen-reader text)`.
/// "Not verified by typing" is no badge: it is said once, on the "標準に従う" choice (review U17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeKind {
    /// No key press seen from this device yet (never on a built-in keyboard).
    NoKeyPress,
    /// A receiver: every keyboard paired with it gets the same layout.
    Receiver,
    /// A virtual keyboard or one of another driver: shown, never changed.
    ReadOnly,
    /// The Remote Desktop keyboard of a session (plan 3.2): read-only like [`BadgeKind::ReadOnly`],
    /// which it replaces on its row, and says where its keys come from.
    RemoteDesktop,
    NotConnected,
    Internal,
    /// Hidden by the user (shown only with "show hidden").
    Hidden,
    /// Stored values MKLM finds odd (design m3 B.1 step 3).
    SettingProblem,
    /// The keyboard that typed last while "identify by key press" is on (design m3 B.3): the
    /// highlight is never shown by colour alone.
    JustPressed,
}

pub fn badge(kind: BadgeKind, lang: Lang) -> (String, String) {
    let (ja, ja_long, en, en_long) = match kind {
        BadgeKind::NoKeyPress => (
            "キー入力なし",
            "キー入力なし: このデバイスからのキー入力をまだ見ていません。マウスなどの付属機能のことがあります。［キーを押して特定］で確かめられます",
            "No key press yet",
            "No key press yet: no key press from this device has been seen. It may be part of a mouse or another device; use \"Identify by key press\" to find out",
        ),
        BadgeKind::Receiver => (
            "レシーバー",
            "レシーバー: このレシーバーにつないだすべてのキーボードに同じ配列が適用されます",
            "Receiver",
            "Receiver: every keyboard connected to it gets the same layout",
        ),
        BadgeKind::ReadOnly => (
            "読み取り専用",
            "読み取り専用: リモート デスクトップや仮想のキーボードは変更できません",
            "Read-only",
            "Read-only: remote desktop and virtual keyboards cannot be changed",
        ),
        BadgeKind::RemoteDesktop => (
            "リモート デスクトップ",
            "リモート デスクトップ: 接続元の PC から届くキー入力です。キーの割り当てはセッションが始まったとき（サインインしたとき）に決まり、MKLM では変更できません",
            "Remote Desktop",
            "Remote Desktop: keys sent by the PC you connect from. Their key table is fixed when the session starts (at sign-in); MKLM cannot change it",
        ),
        BadgeKind::NotConnected => (
            "未接続",
            "未接続: いまは接続されていません",
            "Not connected",
            "Not connected right now",
        ),
        BadgeKind::Internal => ("内蔵", "内蔵キーボード", "Built-in", "Built-in keyboard"),
        BadgeKind::Hidden => (
            "非表示",
            "非表示に設定したキーボード",
            "Hidden",
            "A keyboard you hid",
        ),
        BadgeKind::SettingProblem => (
            "設定に問題",
            "設定に問題: 保存されている値に問題があります。初回セットアップで確かめられます",
            "Setting problem",
            "Setting problem: the stored values look wrong; the setup shows them",
        ),
        BadgeKind::JustPressed => (
            "◀ いま押したキーボード",
            "いま押したキーボード",
            "◀ Just pressed",
            "The keyboard you just pressed",
        ),
    };
    match lang {
        Lang::Ja => (ja.to_string(), ja_long.to_string()),
        Lang::En => (en.to_string(), en_long.to_string()),
    }
}

/// The input method line of the status (plan 3.2): the active HKL of the GUI thread.
/// `(text, ok)`; `ok` is false when per-keyboard layouts do not work with it.
pub fn input_method(hkl_low: u32, lang: Lang) -> (String, bool) {
    // The layout half of the HKL decides the layout DLL: kbdjpn.dll is what per-keyboard layouts
    // need (plan 1.1).
    if mklm_core::hkl_has_japanese_layout(hkl_low) {
        (pick(lang, "日本語 IME ✓", "Japanese IME ✓"), true)
    } else if hkl_low & 0xFFFF == 0x0409 {
        (
            pick(
                lang,
                "英語 (US) ⚠ キーボードごとの配列は無効",
                "English (US) ⚠ per-keyboard layouts do not apply",
            ),
            false,
        )
    } else {
        (
            match lang {
                Lang::Ja => {
                    format!("その他の言語（{hkl_low:08X}）⚠ キーボードごとの配列は無効")
                }
                Lang::En => {
                    format!("another language ({hkl_low:08X}) ⚠ per-keyboard layouts do not apply")
                }
            },
            false,
        )
    }
}

/// An operation's state in the history (design m3 B.11), with what the journal says about it:
/// an interrupted write "stopped halfway" unless its writer still runs; a resolution that kept
/// the values changed outside MKLM is not "nothing kept" (review U5).
pub fn entry_state(
    state: OpState,
    failure: Option<&FailureReason>,
    attention: Attention,
    lang: Lang,
) -> String {
    match (state, failure, attention) {
        (OpState::Planned | OpState::Written, _, Attention::Busy) => {
            pick(lang, "処理中", "In progress")
        }
        (OpState::Planned | OpState::Written, _, _) => pick(
            lang,
            "途中で止まりました（回復が必要）",
            "Stopped halfway (needs recovery)",
        ),
        (OpState::Failed, Some(FailureReason::ConflictKeptCurrent), _) => pick(
            lang,
            "MKLM 以外の値を残しました",
            "Kept the values changed outside MKLM",
        ),
        // "確定" is not a screen word (design m3 D.5): say what the user did.
        (OpState::Confirmed, _, _) => journal_pages::history_kept(lang),
        (state, _, _) => self::state(state, lang),
    }
}

/// An operation's state, plainly (progress lines; the history uses [`entry_state`]).
pub fn state(state: OpState, lang: Lang) -> String {
    let (ja, en) = match state {
        OpState::Planned | OpState::Written => (
            "途中で止まりました（回復が必要）",
            "Stopped halfway (needs recovery)",
        ),
        OpState::Restarting => ("キーボードをリセット中", "Resetting the keyboard"),
        OpState::AwaitingConfirm => ("確認待ち", "Waiting for keep or revert"),
        OpState::PendingReboot => ("PC の再起動待ち", "Waiting for a PC restart"),
        // Never "確定" on the screen (design m3 D.5): what the user did.
        OpState::Confirmed => ("このままにしました", "Kept"),
        OpState::RevertPending => ("元に戻しています", "Being reverted"),
        OpState::Reverted => ("元に戻しました", "Reverted"),
        OpState::RevertedPendingReboot => (
            "元に戻しました（PC の再起動が必要）",
            "Reverted; the PC must restart",
        ),
        OpState::Failed => (
            "完了せず（変更は残っていません）",
            "Not done (nothing kept)",
        ),
        OpState::Conflict => ("衝突", "In conflict"),
    };
    pick(lang, ja, en)
}

/// Why an operation did not end as asked (design m3 G.2: the wording depends on whether the
/// keyboard reset was reached).
pub fn failure(reason: &FailureReason, phase: ResetPhase, lang: Lang) -> String {
    match reason {
        // The value name goes to the technical details (review U5).
        FailureReason::ConcurrentChange { .. } => pick(
            lang,
            "確認画面を出した後に、MKLM 以外がキーボードの設定を変えました。何も変更していません。もう一度やり直してください",
            "Something other than MKLM changed the keyboard settings after the confirmation; nothing was changed. Please try again",
        ),
        FailureReason::WriteError { .. } => pick(
            lang,
            "書き込みに失敗したため、元の値に戻しました",
            "A write failed; the values were put back",
        ),
        FailureReason::CallerDisconnected => pick(
            lang,
            "［このままにする］が選ばれる前に MKLM が終了したため、元に戻しました",
            "MKLM ended before the change was kept; it was put back",
        ),
        FailureReason::Interrupted => pick(
            lang,
            "書き込みの途中で止まったため、回復で元に戻しました",
            "The writer stopped halfway; recovery put the values back",
        ),
        FailureReason::LiveResetUnconfirmed => match phase {
            ResetPhase::NotReached => pick(
                lang,
                "キーボードをリセットする前に止まったため、回復で元に戻しました（キーボードの動作は変わっていません）",
                "The writer stopped before the keyboard reset; recovery put the values back (the keyboard never switched)",
            ),
            ResetPhase::Reached => pick(
                lang,
                "キーボードのリセット後に［このままにする］が選ばれなかったため、回復で元に戻しました",
                "The change was never kept after the keyboard reset; recovery put it back",
            ),
        },
        FailureReason::CountdownExpired => pick(
            lang,
            "時間内に［このままにする］が選ばれなかったため、元に戻しました",
            "No answer before the countdown ran out; it was put back",
        ),
        FailureReason::KeyboardDidNotReturn => pick(
            lang,
            "リセットしたキーボードが戻らなかったか、Windows が再起動を求めたため、元に戻しました",
            "The keyboard did not come back after the reset, or Windows asked for a restart; it was put back",
        ),
        FailureReason::NothingWritten => pick(
            lang,
            "何も書き込まれていませんでした",
            "Nothing had been written",
        ),
        FailureReason::ConflictKeptCurrent => pick(
            lang,
            "MKLM 以外が変更した値をそのまま残しました",
            "The values changed outside MKLM were kept",
        ),
        FailureReason::Superseded { .. } => pick(
            lang,
            "「MKLM 導入前に戻す」に置き換えられました",
            "Replaced by the restore to the values before MKLM",
        ),
    }
}

/// The title of a result (design m3 B.17).
pub fn outcome_title(class: OutcomeClass, lang: Lang) -> String {
    let (ja, en) = match class {
        OutcomeClass::Done => ("完了しました", "Done"),
        OutcomeClass::Failed => ("完了できませんでした", "Not done"),
        OutcomeClass::Cancelled => (
            "取り消しました（何も変更していません）",
            "Cancelled; nothing was changed",
        ),
        OutcomeClass::RevertedAutomatically => ("自動で元に戻しました", "Reverted automatically"),
        OutcomeClass::Conflict => (
            "MKLM 以外による変更が見つかりました",
            "Values changed outside MKLM",
        ),
        OutcomeClass::Blocked => (
            "ほかの操作が終わっていません",
            "Another operation is not finished",
        ),
        OutcomeClass::AwaitingConfirm => {
            ("確認を待っています", "Waiting for you to keep or revert")
        }
        OutcomeClass::RestartRequired => ("PC の再起動が必要です", "The PC must restart"),
    };
    pick(lang, ja, en)
}

/// The next step after an error that says nothing about what to do (design m3 B.17, review U7).
/// The result screen puts the "今の状態" line above it, so these never claim what happened to
/// the values.
const RETRY_AFTER_RESTART: (&str, &str) = (
    "PC を再起動してからもう一度試してください。直らない場合は［詳細をコピー］を押して、その内容を添えて報告してください。",
    "Restart the PC and try again. If it keeps failing, choose \"Copy details\" and include them in a report.",
);

/// An engine error for the user, with the next step (the English `ErrorInfo::message` goes to
/// the details).
pub fn error_code(code: ErrorCode, lang: Lang) -> String {
    // `retry`: add "restart the PC and try again, else copy the details" (review U7).
    let (ja, en, retry) = match code {
        ErrorCode::Busy => (
            "別の MKLM が処理中です。終わってからもう一度試してください。",
            "Another MKLM process is working; try again when it has finished.",
            false,
        ),
        ErrorCode::OpInProgress => (
            "確認待ちの変更があります。先にそれを［このままにする］か、元に戻してください。",
            "A change is still waiting; keep or undo it first.",
            false,
        ),
        ErrorCode::RecoveryNeeded => (
            "途中で止まった操作があります。先に回復してください。",
            "An interrupted operation needs recovery first.",
            false,
        ),
        ErrorCode::JournalUnreadable => (
            "記録を読めません。MKLM を更新してください。",
            "The journal cannot be read; update MKLM.",
            false,
        ),
        ErrorCode::PlanRejected => (
            "安全規則に反するため実行しませんでした。何も変更していません。",
            "Refused by MKLM's safety rules; nothing was changed.",
            false,
        ),
        ErrorCode::PlanChanged => (
            "確認画面を表示した後にキーボードか設定が変わりました。何も変更していません。もう一度やり直してください。",
            "The keyboards or values changed after the confirmation; nothing was changed. Please try again.",
            false,
        ),
        ErrorCode::MigrationRequired => (
            "この PC は固定モードです。先にキーボードごとモードへ移行する必要があります。",
            "This PC is in fixed mode; switch to per-keyboard mode first.",
            false,
        ),
        ErrorCode::NotFixedMode => (
            "この PC はすでにキーボードごとモードです。",
            "This PC is already in per-keyboard mode.",
            false,
        ),
        ErrorCode::UnknownKeyboard => (
            "キーボードが見つかりません。接続を確かめて、［最新の情報に更新］してからもう一度試してください。",
            "The keyboard was not found. Check that it is connected, refresh, and try again.",
            false,
        ),
        ErrorCode::UnknownOp => (
            "操作が記録に見つかりません。",
            "The operation is not in the journal.",
            false,
        ),
        ErrorCode::NotLatest => (
            "後の操作が同じ値を変えています。「MKLM 導入前に戻す」を使ってください。",
            "A later operation changed the same values; use the restore to before MKLM.",
            false,
        ),
        ErrorCode::InvalidState => (
            "この操作は今の状態ではできません。",
            "Not possible in the operation's current state.",
            false,
        ),
        ErrorCode::InventoryIncomplete => (
            "Windows がキーボードの情報をすべて返さなかったため、何も変更していません。もう一度試してください。",
            "Windows did not report every keyboard completely; nothing was changed. Try again.",
            false,
        ),
        ErrorCode::RecoveryAssetsUnavailable => (
            "復旧用ファイルを書けなかったため、何も変更していません。",
            "The recovery files could not be written; nothing was changed.",
            true,
        ),
        ErrorCode::Cancelled => (
            "取り消しました（何も変更していません）。",
            "Cancelled; nothing was changed.",
            false,
        ),
        ErrorCode::Registry => (
            "キーボードの設定を読み書きできませんでした。",
            "The keyboard settings could not be read or written.",
            true,
        ),
        ErrorCode::Device => (
            "キーボードをリセットできませんでした。",
            "The keyboard could not be reset.",
            true,
        ),
        ErrorCode::Host => (
            "MKLM の作業用の場所を準備できませんでした。",
            "MKLM could not prepare its working area.",
            true,
        ),
        ErrorCode::Protocol => (
            "MKLM の管理用プログラム（mklm-helper.exe）との通信に失敗しました。",
            "Talking to MKLM's administrator program (mklm-helper.exe) failed.",
            true,
        ),
        ErrorCode::Internal => ("MKLM の内部エラーです。", "An internal MKLM error.", true),
    };
    match (lang, retry) {
        (Lang::Ja, true) => format!("{ja}{}", RETRY_AFTER_RESTART.0),
        (Lang::En, true) => format!("{en} {}", RETRY_AFTER_RESTART.1),
        (_, false) => pick(lang, ja, en),
    }
}

/// The antivirus hint for a missing or blocked `mklm-helper.exe` (MKLM is not code-signed, so
/// a quarantine is the likely cause, design m3 B.17; review U7).
const ANTIVIRUS_HINT: (&str, &str) = (
    "ウイルス対策ソフトが mklm-helper.exe を隔離していないか、Windows セキュリティ → ウイルスと脅威の防止 → 保護の履歴 で確かめ、許可してから MKLM を修復（インストールし直し）してください。何も変更していません。",
    "Check whether an antivirus quarantined mklm-helper.exe (Windows Security → Virus & threat protection → Protection history), allow it, then repair (reinstall) MKLM. Nothing was changed.",
);

/// Why no helper session came up (design m3 B.17).
pub fn launch_error(error: &LaunchError, lang: Lang) -> String {
    let helper = pick(
        lang,
        "MKLM の管理用プログラム（mklm-helper.exe）",
        "MKLM's administrator program (mklm-helper.exe)",
    );
    match error {
        LaunchError::Declined => pick(
            lang,
            "管理者の確認で「いいえ」が選ばれたため、何も変更していません。",
            "The administrator prompt was declined; nothing was changed.",
        ),
        LaunchError::Failed { kind, .. } => match kind {
            LaunchFailure::HelperMissing => match lang {
                Lang::Ja => format!("{helper}が見つかりません。{}", ANTIVIRUS_HINT.0),
                Lang::En => format!("{helper} was not found. {}", ANTIVIRUS_HINT.1),
            },
            // 225 ERROR_VIRUS_INFECTED, 226 ERROR_VIRUS_DELETED.
            LaunchFailure::StartFailed(Some(225 | 226)) => match lang {
                Lang::Ja => format!(
                    "{helper}がウイルス対策ソフトに止められました。{}",
                    ANTIVIRUS_HINT.0
                ),
                Lang::En => format!("{helper} was blocked by an antivirus. {}", ANTIVIRUS_HINT.1),
            },
            // 1260 ERROR_ACCESS_DISABLED_BY_POLICY.
            LaunchFailure::StartFailed(Some(1260)) => match lang {
                Lang::Ja => format!(
                    "この PC のポリシーで {helper}の実行が禁止されています。PC の管理者に相談してください。何も変更していません。"
                ),
                Lang::En => format!(
                    "A policy on this PC does not allow {helper} to run; ask the PC's administrator. Nothing was changed."
                ),
            },
            LaunchFailure::OtherBuild => match lang {
                Lang::Ja => format!(
                    "{helper}の版が MKLM と合いません。MKLM を修復（インストールし直し）してください。何も変更していません。"
                ),
                Lang::En => format!(
                    "{helper} does not match this MKLM; repair (reinstall) MKLM. Nothing was changed."
                ),
            },
            LaunchFailure::Setup | LaunchFailure::StartFailed(_) | LaunchFailure::NoConnection => {
                match lang {
                    Lang::Ja => format!(
                        "{helper}を起動できませんでした。何も変更していません。PC を再起動してからもう一度試してください。"
                    ),
                    Lang::En => format!(
                        "{helper} could not be started; nothing was changed. Restart the PC and try again."
                    ),
                }
            }
            LaunchFailure::ExitedEarly(exit) => helper_exit(*exit, lang),
            LaunchFailure::Handshake => match lang {
                Lang::Ja => format!(
                    "{helper}との接続を確認できませんでした。何も変更していません。MKLM を修復（インストールし直し）してください。"
                ),
                Lang::En => format!(
                    "The connection to {helper} could not be verified; nothing was changed. Repair (reinstall) MKLM."
                ),
            },
        },
    }
}

/// A helper that exited before it answered, worded from its exit code (design m2 E.8). The
/// code itself goes to the technical details.
pub fn helper_exit(exit: HelperExit, lang: Lang) -> String {
    let (ja, en) = match exit.kind() {
        HelperExitKind::Ended | HelperExitKind::Failed | HelperExitKind::Unexpected => (
            "MKLM の管理用プログラム（mklm-helper.exe）が途中で終了しました。PC を再起動してからもう一度試してください。",
            "MKLM's administrator program (mklm-helper.exe) stopped early. Restart the PC and try again.",
        ),
        HelperExitKind::BadCommandLine | HelperExitKind::HandshakeFailed => (
            "MKLM の管理用プログラム（mklm-helper.exe）が MKLM と接続できませんでした。MKLM を修復（インストールし直し）してください。",
            "MKLM's administrator program (mklm-helper.exe) could not connect to MKLM. Repair (reinstall) MKLM.",
        ),
        HelperExitKind::NotElevated => (
            "管理者の許可が得られませんでした。もう一度試して、Windows の確認で「はい」を選んでください。",
            "Administrator permission was not granted. Try again and choose Yes in the Windows prompt.",
        ),
        HelperExitKind::UnsupportedOs => (
            "この Windows では MKLM の管理用プログラムが動きません（Windows 11 24H2 以降が必要です）。",
            "MKLM's administrator program does not run on this Windows (Windows 11 24H2 or later is needed).",
        ),
    };
    pick(lang, ja, en)
}

/// The wait for a helper that stopped answering (design m3 B.17, review U7): a standard user
/// cannot end an elevated process, so the way out is a restart, after which MKLM offers the
/// recovery.
pub fn unresponsive(lang: Lang) -> String {
    pick(
        lang,
        "MKLM の管理用プログラム（mklm-helper.exe）が応答しません。1 分待っても変わらなければ PC を再起動してください。再起動後に MKLM が回復を案内します。",
        "MKLM's administrator program (mklm-helper.exe) does not answer. If nothing changes within a minute, restart the PC; MKLM then offers the recovery.",
    )
}

/// An attention for a banner (design m3 B.12).
pub fn attention(attention: Attention, lang: Lang) -> Option<String> {
    let (ja, en) = match attention {
        Attention::None => return None,
        Attention::Busy => ("別の MKLM が処理中です", "Another MKLM process is working"),
        Attention::Recover => (
            "途中で止まった操作があります。回復するまで、キーボードの配列は変更できません",
            "An operation was interrupted and needs recovery",
        ),
        Attention::AwaitingUser => (
            "確認待ちの変更があります",
            "A change waits for keep or revert",
        ),
        Attention::WaitingForReboot => (
            "PC の再起動を待っている変更があります",
            "A change waits for a PC restart",
        ),
        Attention::Conflict => (
            "MKLM 以外による変更が見つかりました",
            "Values were changed outside MKLM",
        ),
        Attention::NeedsApply => (
            "まだ反映されていない変更があります",
            "A change is not in effect yet",
        ),
    };
    Some(pick(lang, ja, en))
}

// --- The change flow (design m3 B.3 to B.7, B.17; WP-U3) ---

/// A layout by name: "JIS" / "US".
pub fn layout_name(layout: Layout) -> String {
    table(&layout.into(), LANG_NEUTRAL)
}

/// Table names are the same in both languages; `table` only needs a language for "other".
const LANG_NEUTRAL: Lang = Lang::En;

/// The change page's title: "Keychron Receiver の配列" (also `@tr("Layout of {}")` in Slint).
pub fn change_title(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} の配列"),
        Lang::En => format!("Layout of {name}"),
    }
}

/// The restore page's title (design m3 B.4 "MKLM 導入前に戻す…").
pub fn restore_title(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} を MKLM 導入前に戻す"),
        Lang::En => format!("Put {name} back to before MKLM"),
    }
}

/// One layout choice of the change page (design m3 B.4): `(text, detail)`. `standard` is the
/// PC's standard layout now; `current` adds "（現在）".
pub fn layout_choice(
    choice: LayoutChoice,
    standard: &LayoutTable,
    current: bool,
    lang: Lang,
) -> (String, String) {
    let (text, detail) = match choice {
        LayoutChoice::Jis => ("JIS".to_string(), String::new()),
        LayoutChoice::Us => ("US".to_string(), String::new()),
        LayoutChoice::Standard => {
            let standard = table(standard, lang);
            match lang {
                Lang::Ja => (
                    format!("標準に従う（今は {standard}）"),
                    "値を消して PC の標準配列に従わせます。打鍵での確認がまだです".to_string(),
                ),
                Lang::En => (
                    format!("Follow the PC's standard layout (now {standard})"),
                    "Removes the values so that the keyboard follows the PC's standard layout. Not yet verified by typing".to_string(),
                ),
            }
        }
    };
    let suffix = match (current, lang) {
        (false, _) => "",
        (true, Lang::Ja) => "（現在）",
        (true, Lang::En) => " (current)",
    };
    (format!("{text}{suffix}"), detail)
}

/// The PC's standard layout for a migration from fixed mode (design m3 B.1, B.4):
/// "JIS（おすすめ: 今の JIS）".
pub fn standard_choice(layout: Layout, recommended: bool, lang: Lang) -> String {
    let name = layout_name(layout);
    match (recommended, lang) {
        (false, _) => name,
        (true, Lang::Ja) => format!("{name}（おすすめ: 今の {name}）"),
        (true, Lang::En) => format!("{name} (recommended: the current {name})"),
    }
}

/// What the in-place detection asks for next (design m3 B.3); empty once both keys are in.
pub fn detect_instruction(step: Step, lang: Lang) -> String {
    match step {
        Step::LeftOfBackspace => pick(
            lang,
            "わからないときは、このキーボードで Backspace の左のキーを押してください（打鍵テスト）",
            "Not sure? Press the key left of Backspace on this keyboard (key test)",
        ),
        Step::LeftOfRightShift => pick(
            lang,
            "次に、このキーボードで右の Shift の左のキーを押してください",
            "Next, press the key left of the right Shift on this keyboard",
        ),
        Step::Done => String::new(),
    }
}

/// An answer key came from another keyboard (`Press::OtherKeyboard`, design m3 B.3).
pub fn detect_other_keyboard(other: &str, name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{other} のキーです。{name} で押してください"),
        Lang::En => format!("That key is from {other}. Press it on {name}"),
    }
}

/// A key only JIS keyboards have was pressed on the keyboard (design m3 B.3).
pub fn detect_jis_hint(lang: Lang) -> String {
    pick(
        lang,
        "JIS の可能性が高いです（JIS 配列にしかないキーが押されました）",
        "Probably JIS (a key only JIS keyboards have was pressed)",
    )
}

/// The detection's verdict for `name`.
pub fn detect_verdict(verdict: Verdict, name: &str, lang: Lang) -> String {
    match (verdict, lang) {
        (Verdict::Jis, Lang::Ja) => format!("✓ {name} は JIS 配列のキーボードです"),
        (Verdict::Jis, Lang::En) => format!("✓ {name} is a JIS keyboard"),
        (Verdict::Us, Lang::Ja) => format!("✓ {name} は US 配列のキーボードです"),
        (Verdict::Us, Lang::En) => format!("✓ {name} is a US keyboard"),
        (Verdict::Mixed, _) => pick(
            lang,
            "判定できませんでした。［やり直す］を押して、もう一度試してください。",
            "Could not tell. Choose \"Start again\" and try once more.",
        ),
    }
}

/// The button that selects the detected layout (one click, design m3 B.3): "JIS を選ぶ".
pub fn detect_choose(layout: Layout, lang: Lang) -> String {
    let name = layout_name(layout);
    match lang {
        Lang::Ja => format!("{name} を選ぶ"),
        Lang::En => format!("Choose {name}"),
    }
}

/// Every change applies to every user of the PC (plan 3.5).
pub fn all_users_note(lang: Lang) -> String {
    pick(
        lang,
        "ⓘ この設定は、この PC のすべてのユーザーに適用されます。",
        "ⓘ This setting applies to every user of this PC.",
    )
}

/// Fixed mode: the change is a migration that carries this assignment (design m3 B.4).
pub fn migration_note(lang: Lang) -> String {
    pick(
        lang,
        "この PC は固定モードです。キーボードごとモードへ移行し、この割り当ても同時に書きます（PC の再起動が 1 回必要）。",
        "This PC is in fixed mode. MKLM switches it to per-keyboard mode and writes this assignment at the same time (one PC restart is needed).",
    )
}

/// A US keyboard has no 半角/全角 key (plan 3.4; design m3 B.13, review U19).
pub fn ime_note_us(lang: Lang) -> String {
    pick(
        lang,
        "US 配列には「半角/全角」キーがありません。日本語入力のオン/オフは Alt+` です。",
        "A US keyboard has no 半角/全角 key: turn Japanese input on and off with Alt+`.",
    )
}

/// Plan 1.4: the target is the only keyboard that typed recently.
pub fn only_keyboard_warning(lang: Lang) -> String {
    pick(
        lang,
        "このキーボードは、最近入力のあった唯一のキーボードです。PC の再起動で反映することをおすすめします。",
        "This keyboard is the only one that typed recently. Applying the change with a PC restart is recommended.",
    )
}

/// "標準に従う" is not verified by typing yet (design m2 D.2).
pub fn standard_unverified(lang: Lang) -> String {
    pick(
        lang,
        "打鍵での確認がまだです。後で Shift+2 で確かめてください。",
        "Not yet verified by typing. Check it later with Shift+2.",
    )
}

/// Every planned value is in place already (`preview::nothing_to_change`).
pub fn nothing_to_change(lang: Lang) -> String {
    pick(
        lang,
        "変更はありません（すでにこの設定です）。",
        "Nothing to change: it is set this way already.",
    )
}

/// The page a preparation failed on: its next step names buttons that page has (design m3
/// B.17). The change page (also "今すぐ反映…" and the restore previews) has no "最新の情報に更新";
/// the wizard reads and prepares again when its last step opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparePlace {
    ChangePage,
    Wizard,
}

/// Why the preparation could not read what a change is planned with (design m3 B.5), and what
/// to do on `place`.
pub fn prepare_failed(incomplete: bool, place: PreparePlace, lang: Lang) -> String {
    let reason = if incomplete {
        pick(
            lang,
            "Windows がキーボードの情報をすべて返さなかったため、今は変更できません。",
            "Windows did not report every keyboard completely, so no change is possible now.",
        )
    } else {
        pick(
            lang,
            "キーボードか記録（ジャーナル）を読めなかったため、今は変更できません。",
            "The keyboards or the journal could not be read, so no change is possible now.",
        )
    };
    let next = match place {
        PreparePlace::ChangePage => pick(
            lang,
            "［キャンセル］でメイン画面に戻り、［最新の情報に更新］を押してから、もう一度やり直してください。",
            "Choose \"Cancel\", then \"Refresh\" on the main screen, and try again.",
        ),
        PreparePlace::Wizard => pick(
            lang,
            "［戻る］を押してから、もう一度［次へ］を押してください。",
            "Choose \"Back\", then \"Next\" again.",
        ),
    };
    match lang {
        Lang::Ja => format!("{reason}{next}"),
        Lang::En => format!("{reason} {next}"),
    }
}

/// A change the rules refuse before any prompt (the English error goes to the details).
pub fn operation_refused(error: &OperationError, lang: Lang) -> String {
    match error {
        OperationError::UnknownKeyboard { .. } => error_code(ErrorCode::UnknownKeyboard, lang),
        OperationError::MigrationRequired { .. } => error_code(ErrorCode::MigrationRequired, lang),
        OperationError::NotFixedMode => error_code(ErrorCode::NotFixedMode, lang),
        OperationError::InconsistentGlobal => pick(
            lang,
            "PC 全体のキーボードの値が食い違っているため、この変更はできません。",
            "The PC-wide keyboard values are inconsistent, so this change is not possible.",
        ),
        OperationError::StandardNotAllowed { .. } => pick(
            lang,
            "このキーボードは「標準に従う」にできません。",
            "This keyboard cannot follow the standard layout.",
        ),
        OperationError::LayerDriverMissing { .. } => pick(
            lang,
            "配列のファイルが Windows に見つからないため、この変更はできません。",
            "The layout file is missing from Windows, so this change is not possible.",
        ),
        OperationError::Plan(_) => pick(
            lang,
            "MKLM の安全規則に反するため、この変更はできません。理由は技術的な詳細にあります。",
            "MKLM's safety rules do not allow this change; the technical details say why.",
        ),
    }
}

/// The one or two lines about the UAC prompt next to the button (review U14).
pub fn uac_line(lang: Lang) -> String {
    pick(
        lang,
        "次に Windows の確認画面が出ます（発行元は「不明」）。mklm-helper.exe であることを確かめて「はい」を押してください。",
        "Windows asks for permission next (the publisher shows as \"Unknown\"). Check that it names mklm-helper.exe, then choose Yes.",
    )
}

/// The change page's main button: "変更する（次に Windows の確認が出ます）", or "変更する" when
/// no prompt follows (an elevated GUI, design m3 B.5).
pub fn apply_button(restore: bool, prompt: bool, lang: Lang) -> String {
    match (restore, prompt) {
        (false, true) => pick(
            lang,
            "変更する（次に Windows の確認が出ます）",
            "Change (Windows asks next)",
        ),
        (false, false) => pick(lang, "変更する", "Change"),
        (true, true) => pick(
            lang,
            "元に戻す（次に Windows の確認が出ます）",
            "Put back (Windows asks next)",
        ),
        (true, false) => pick(lang, "元に戻す", "Put back"),
    }
}

/// A registry value in the technical details.
pub fn reg_value(value: &RegValue, lang: Lang) -> String {
    match value {
        RegValue::Absent => pick(lang, "（なし）", "(none)"),
        RegValue::Dword { value } => value.to_string(),
        RegValue::Sz { value } => format!("\"{value}\""),
        RegValue::Other { reg_type, data_hex } => format!("({reg_type}: {data_hex})"),
    }
}

/// The PC-wide values in the technical details.
pub fn global_values(lang: Lang) -> String {
    pick(lang, "PC 全体", "PC-wide")
}

/// What a restore to before MKLM does (design m2 D.5, m3 B.4).
pub fn restore_summary(writes: usize, lang: Lang) -> String {
    if writes == 0 {
        pick(
            lang,
            "MKLM 導入前の値のままです。戻すものはありません。",
            "Everything is as it was before MKLM; there is nothing to put back.",
        )
    } else {
        match (lang, writes) {
            (Lang::Ja, _) => format!("{writes} 件の値を MKLM 導入前の値に戻します。"),
            (Lang::En, 1) => "1 value goes back to what it was before MKLM.".to_string(),
            (Lang::En, _) => format!("{writes} values go back to what they were before MKLM."),
        }
    }
}

/// Values changed outside MKLM: the restore stops and asks first (`ConflictPolicy::Report`).
pub fn restore_conflicts(count: usize, lang: Lang) -> String {
    match (lang, count) {
        (Lang::Ja, _) => format!(
            "MKLM 以外が変更した値が {count} 件あります。戻す前に、その値をどうするかを選びます。"
        ),
        (Lang::En, 1) => "1 value was changed outside MKLM; you choose what to do with it before anything is put back.".to_string(),
        (Lang::En, _) => format!(
            "{count} values were changed outside MKLM; you choose what to do with them before anything is put back."
        ),
    }
}

/// Values of keyboards that are gone are left alone (design review C3).
pub fn restore_removed(count: usize, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("接続されていないキーボードの値 {count} 件は、そのままにします。"),
        Lang::En if count == 1 => "1 value of a keyboard that is gone is left alone.".to_string(),
        Lang::En => format!("{count} values of keyboards that are gone are left alone."),
    }
}

/// Open changes the restore closes (design review C7).
pub fn restore_supersedes(count: usize, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("確認待ちの変更 {count} 件は、この操作に置き換えられます。"),
        Lang::En if count == 1 => {
            "1 change waiting for you is replaced by this restore.".to_string()
        }
        Lang::En => format!("{count} changes waiting for you are replaced by this restore."),
    }
}

/// The baseline itself did not pin the built-in keyboard; it is put back as found.
pub fn restore_keeps_inv_ps2_violation(lang: Lang) -> String {
    pick(
        lang,
        "MKLM 導入前の値は、内蔵（PS/2）キーボードの配列を固定していませんでした。そのとおりに戻します。",
        "Before MKLM, the values did not pin the built-in (PS/2) keyboard's layout; they are put back as they were.",
    )
}

/// The restore order could not keep the built-in keyboard usable.
pub fn restore_refused(lang: Lang) -> String {
    pick(
        lang,
        "この戻し方では内蔵（PS/2）キーボードの配列が決まらなくなるため、戻せません。",
        "Putting these values back would leave the built-in (PS/2) keyboard without a layout, so it is not possible.",
    )
}

// --- A running session (design m3 B.6, B.7) ---

/// What the progress dialog says the session is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStep {
    /// The helper is being launched: the UAC prompt may be up.
    WaitingForPrompt,
    /// Connected; nothing reported yet.
    Preparing,
    /// The operation is journaled.
    Journaled,
    /// `(step, of)` written.
    Written(usize, usize),
    /// The user chose "keep"; waiting for the result.
    Keeping,
    /// The user (or the window's ×, Esc) chose "revert"; waiting for the result.
    Reverting,
    /// "Decide later" or quitting on the reconnect path.
    Leaving,
    /// The helper stopped; MKLM checks the journal.
    HelperLost,
    /// The recovery after a lost helper runs.
    Recovering,
}

pub fn progress_title(lang: Lang) -> String {
    pick(
        lang,
        "キーボードの設定を変更しています",
        "Changing the keyboard settings",
    )
}

pub fn progress_step(step: ProgressStep, lang: Lang) -> String {
    match step {
        ProgressStep::WaitingForPrompt => pick(
            lang,
            "管理者の確認を待っています…",
            "Waiting for the administrator prompt…",
        ),
        ProgressStep::Preparing => pick(lang, "準備しています…", "Preparing…"),
        ProgressStep::Journaled => pick(lang, "操作を記録しました", "The operation is journaled"),
        ProgressStep::Written(step, of) => match lang {
            Lang::Ja => format!("書き込み {step} / {of}"),
            Lang::En => format!("Written {step} of {of}"),
        },
        ProgressStep::Keeping => pick(lang, "このままにします…", "Keeping the new layout…"),
        ProgressStep::Reverting => pick(lang, "元に戻しています…", "Reverting…"),
        ProgressStep::Leaving => pick(
            lang,
            "後で決めます。変更は確認待ちのまま残ります…",
            "Leaving it for later; the change keeps waiting for you…",
        ),
        ProgressStep::HelperLost => pick(
            lang,
            "MKLM の管理用プログラム（mklm-helper.exe）が止まりました。確かめています…",
            "MKLM's administrator program (mklm-helper.exe) stopped. Checking…",
        ),
        ProgressStep::Recovering => pick(
            lang,
            "キーボードを元に戻しています…",
            "Putting the keyboard back…",
        ),
    }
}

/// "Keychron Receiver をリセットしています…".
pub fn progress_resetting(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} をリセットしています…"),
        Lang::En => format!("Resetting {name}…"),
    }
}

/// A reported keyboard type in words (review U5: no hex): "JIS 配列" / "US 配列" /
/// "不明な種類（8/2）".
pub fn recognized(reported: Option<KeyboardType>, lang: Lang) -> String {
    match (reported, lang) {
        (Some(KeyboardType::JIS), Lang::Ja) => "JIS 配列".into(),
        (Some(KeyboardType::JIS), Lang::En) => "JIS".into(),
        (Some(KeyboardType::US), Lang::Ja) => "US 配列".into(),
        (Some(KeyboardType::US), Lang::En) => "US".into(),
        (Some(other), Lang::Ja) => format!("不明な種類（{}/{}）", other.ty, other.subtype),
        (Some(other), Lang::En) => {
            format!("an unknown type ({}/{})", other.ty, other.subtype)
        }
        (None, Lang::Ja) => "不明".into(),
        (None, Lang::En) => "unknown".into(),
    }
}

pub fn countdown_title(lang: Lang) -> String {
    pick(lang, "新しい配列を試してください", "Try the new layout")
}

/// "Keychron Receiver を JIS に切り替えました。"
pub fn countdown_switched(name: &str, layout: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} を {layout} に切り替えました。"),
        Lang::En => format!("{name} switched to {layout}."),
    }
}

/// The time limit and how to keep the change (review U10): part of the one announcement.
pub fn countdown_how(seconds: u32, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "あと {seconds} 秒で自動的に元に戻ります。そのキーボードで Shift+2 を押して確かめてから（\" なら JIS、@ なら US）、Tab で［このままにする］へ移って押してください。"
        ),
        Lang::En => format!(
            "It reverts automatically in {seconds} seconds. Press Shift+2 on that keyboard to check (\" means JIS, @ means US), then Tab to \"Keep this layout\" and press it."
        ),
    }
}

/// "Keychron Receiver は JIS 配列です".
pub fn arrival(name: &str, kind: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{name} は {kind}です"),
        Lang::En => format!("{name}: {kind}"),
    }
}

/// "Windows の認識: … ✓", or that it cannot be confirmed yet (review U5).
pub fn recognition(verified: bool, arrivals: &[String], lang: Lang) -> String {
    match (verified, lang) {
        (true, Lang::Ja) => format!("Windows の認識: {} ✓", arrivals.join("、")),
        (true, Lang::En) => format!("Windows reports: {} ✓", arrivals.join(", ")),
        (false, _) => pick(
            lang,
            "Windows の認識をまだ確かめられません。打鍵テストで確かめてください。",
            "Windows has not confirmed it yet; check with the key test.",
        ),
    }
}

/// The polite reminder at 10 and 5 seconds left (design m3 E.3).
pub fn countdown_reminder(remaining: u32, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("あと {remaining} 秒で元に戻ります"),
        Lang::En => format!("Reverting in {remaining} seconds"),
    }
}

pub fn reconnect_title(lang: Lang) -> String {
    pick(
        lang,
        "キーボードを接続し直してください",
        "Reconnect the keyboard",
    )
}

pub fn reconnect_instructions(names: &[String], lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "{} を抜いて差し直してください（Bluetooth は電源をオフにしてからオンにします）。その後 Shift+2 で確かめてください。",
            names.join("、")
        ),
        Lang::En => format!(
            "Unplug and replug {} (Bluetooth: turn it off and on), then try Shift+2.",
            names.join(", ")
        ),
    }
}

pub fn reconnect_status(arrived: bool, lang: Lang) -> String {
    if arrived {
        pick(
            lang,
            "接続し直したキーボードが新しい種類を報告しています ✓",
            "The keyboard is back and reports the new type ✓",
        )
    } else {
        pick(
            lang,
            "接続し直すのを待っています…",
            "Waiting for the keyboard to reconnect…",
        )
    }
}

/// Quitting while a reconnect waits for keep or revert (design m3 F.5 `quit-confirm`).
pub fn quit_confirm(names: &[String], lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "{} の変更は、このままにするか元に戻すかをまだ決めていません。終了して後で決める場合、変更は確認待ちのまま残り、次に MKLM を開いたときに決められます。",
            names.join("、")
        ),
        Lang::En => format!(
            "You have not decided yet whether to keep the change of {}. If you quit and decide later, the change keeps waiting and MKLM asks again when you open it.",
            names.join(", ")
        ),
    }
}

// --- The key test (design m3 B.6; review U2) ---

pub fn key_test_prompt(lang: Lang) -> String {
    pick(
        lang,
        "キーを押してください（Shift+2 で @ なら US、\" なら JIS）",
        "Press a key (Shift+2: @ means US, \" means JIS)",
    )
}

pub fn key_test_not_japanese(lang: Lang) -> String {
    pick(
        lang,
        "入力方式が日本語ではないため判定できません。Win+Space で日本語に切り替えてください。",
        "Cannot tell: the input method is not Japanese. Switch to Japanese with Win+Space.",
    )
}

pub fn key_test_other_keyboard(from: &str, name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("このキーは {from} から送られました。{name} で押してください"),
        Lang::En => format!("This key came from {from}. Press it on {name}"),
    }
}

pub fn key_test_unexpected(lang: Lang) -> String {
    pick(
        lang,
        "Shift+2 → 想定外の文字です",
        "Shift+2 → an unexpected character",
    )
}

pub fn key_test_as_expected(text: &str, got: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("Shift+2 → {text} : ✓ 期待どおり {got} です"),
        Lang::En => format!("Shift+2 → {text} : ✓ {got}, as expected"),
    }
}

pub fn key_test_wrong(text: &str, want: &str, got: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "Shift+2 → {text} : ⚠ {want} になるはずが {got} です。［元に戻す］をおすすめします。"
        ),
        Lang::En => format!(
            "Shift+2 → {text} : ⚠ it should type {want} but types {got}. Revert is recommended."
        ),
    }
}

pub fn key_test_neutral(text: &str, got: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("Shift+2 → {text} : {got} 配列として動作しています"),
        Lang::En => format!("Shift+2 → {text} : it types {got}"),
    }
}

pub fn key_test_device(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("このキーを送ったキーボード: {name}"),
        Lang::En => format!("Sent by: {name}"),
    }
}

// --- The result (design m3 B.17) ---

/// Until the read after the session arrives (review U7).
pub fn checking_now(lang: Lang) -> String {
    pick(lang, "確かめています…", "Checking…")
}

/// One "今の状態" line: "Keychron Receiver: US として動作中（変更前のまま）".
pub fn current_state_line(
    name: &str,
    typing: Option<&LayoutTable>,
    present: bool,
    unchanged: bool,
    lang: Lang,
) -> String {
    match (present, typing) {
        (false, _) => match lang {
            Lang::Ja => format!("{name}: 未接続"),
            Lang::En => format!("{name}: not connected"),
        },
        (true, None) => match lang {
            Lang::Ja => format!("{name}: 動作を確かめられません"),
            Lang::En => format!("{name}: cannot tell how it types"),
        },
        (true, Some(layout)) => {
            let layout = table(layout, lang);
            match (lang, unchanged) {
                (Lang::Ja, false) => format!("{name}: {layout} として動作中"),
                (Lang::Ja, true) => format!("{name}: {layout} として動作中（変更前のまま）"),
                (Lang::En, false) => format!("{name}: types {layout}"),
                (Lang::En, true) => format!("{name}: types {layout} (as before)"),
            }
        }
    }
}

/// What a finished request did, when the reason does not say it (design m3 B.17).
/// The result of "今すぐ反映…" (design m3 B.12) when nothing had to be recovered: the saved
/// layout is in effect now.
pub fn result_applied_now(lang: Lang) -> String {
    pick(lang, "反映しました。", "Put into effect.")
}

pub fn result_outcome(outcome: mklm_core::Outcome, lang: Lang) -> String {
    use mklm_core::Outcome;
    match outcome {
        Outcome::NoChange => pick(
            lang,
            "すでにその設定だったため、何も変更していません。",
            "It was set this way already; nothing was changed.",
        ),
        Outcome::Confirmed => pick(
            lang,
            "新しい配列のままにしました。",
            "The new layout is kept.",
        ),
        Outcome::AwaitingConfirm => pick(
            lang,
            "変更は確認待ちです。キーボードを接続し直してから、メイン画面の［確認…］で、このままにするか元に戻すかを選んでください。",
            "The change waits for you: reconnect the keyboard, then choose keep or revert with \"Review…\" on the main screen.",
        ),
        Outcome::PendingReboot => takes_effect(PendingAction::RestartPc, 0, lang),
        Outcome::Reverted => pick(lang, "元に戻しました。", "Put back as it was."),
        Outcome::RevertedPendingReboot => pick(
            lang,
            "元に戻しました。PC を再起動すると反映されます（シャットダウンではなく再起動）。",
            "Put back; it takes effect when the PC restarts (Restart, not Shut down).",
        ),
        Outcome::Failed => pick(lang, "完了できませんでした。", "It was not done."),
        Outcome::Conflict => pick(
            lang,
            "MKLM 以外がキーボードの設定を変更していました。どうするかを選んでください。",
            "Something other than MKLM changed the keyboard settings; choose what to do.",
        ),
        Outcome::Recovered => pick(lang, "回復しました。", "Recovered."),
    }
}

/// INV-PS2 would break with the values as they are (design review C12).
pub fn result_inv_ps2(lang: Lang) -> String {
    pick(
        lang,
        "今の値のままでは内蔵（PS/2）キーボードの配列が決まらないため、MKLM は止めました。どうするかを選んでください。",
        "With the values as they are now the built-in (PS/2) keyboard would have no fixed layout, so MKLM stopped. Choose what to do.",
    )
}

/// One operation a recovery or an undo acted on: "途中で止まりました → 元に戻しました（…）".
pub fn recovered_line(what: Option<&str>, from: OpState, to: OpState, lang: Lang) -> String {
    // It has been recovered, so no "（回復が必要）" on the state it was found in.
    let from_text = match from {
        OpState::Planned | OpState::Written => pick(lang, "途中で止まりました", "Stopped halfway"),
        other => state(other, lang),
    };
    let to_text = state(to, lang);
    let phase = match (to, mklm_client::describe::reset_phase_from(from)) {
        (OpState::Reverted | OpState::RevertedPendingReboot, ResetPhase::NotReached) => pick(
            lang,
            "（リセットの前に止まっていたため、キーボードの動作は変わっていません）",
            " (it stopped before the keyboard reset, so the keyboard never switched)",
        ),
        _ => String::new(),
    };
    match what {
        Some(what) => format!("{what}: {from_text} → {to_text}{phase}"),
        None => format!("{from_text} → {to_text}{phase}"),
    }
}

/// The helper was lost and MKLM recovered at once (design m3 B.17).
pub fn result_recovered_after_loss(lang: Lang) -> String {
    pick(
        lang,
        "MKLM の管理用プログラムが止まったため、すぐに回復しました。",
        "MKLM's administrator program stopped, so MKLM recovered at once.",
    )
}

/// The helper was lost and the recovery did not run (declined, skipped, or its prompt refused).
pub fn result_recovery_waits(lang: Lang) -> String {
    pick(
        lang,
        "変更は確認待ちのまま残っています。［回復…］で回復できます。それまで、キーボードの配列は変更できません。",
        "The change is still waiting. \"Recover…\" recovers it; until then no keyboard's layout can be changed.",
    )
}

/// The helper was lost and whether recovery is needed could not be read.
pub fn result_lost_unknown(lang: Lang) -> String {
    pick(
        lang,
        "MKLM の管理用プログラムが止まりました。回復が必要かどうかを確かめられなかったため、メイン画面の表示を確かめてください。",
        "MKLM's administrator program stopped, and whether recovery is needed could not be read; check the main screen.",
    )
}

/// After a recovery, entries still wait for keep or revert (design m2 C7).
pub fn result_still_waiting(lang: Lang) -> String {
    pick(
        lang,
        "確認待ちの変更があります。［確認待ちの変更をすべて元に戻す…］で戻せます。",
        "A change still waits for you; \"Undo every change waiting for you\" puts it back.",
    )
}

/// The post-reboot RunOnce rule could not be applied (design m3 B.17 `run_once_note`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOnceNote {
    /// An elevated GUI does not register (`RunOnceOutcome::TellUser`).
    Elevated,
    /// Registering failed.
    NotRegistered,
    /// The journal could not be read to decide.
    Unchecked,
}

pub fn run_once_note(note: RunOnceNote, lang: Lang) -> String {
    match note {
        RunOnceNote::Elevated => pick(
            lang,
            "管理者として実行している MKLM は、再起動後の確認を登録できません。PC を再起動してサインインしたら、MKLM を開いてください。",
            "An MKLM run as administrator cannot register the check after the restart. After restarting the PC and signing in, open MKLM.",
        ),
        RunOnceNote::NotRegistered => pick(
            lang,
            "再起動後の確認を登録できませんでした。PC を再起動してサインインしたら、MKLM を開いてください。",
            "The check after the restart could not be registered. After restarting the PC and signing in, open MKLM.",
        ),
        RunOnceNote::Unchecked => pick(
            lang,
            "記録を読めなかったため、再起動後の確認が必要かを確かめられませんでした。PC を再起動した後は、MKLM を開いて確かめてください。",
            "The journal could not be read to see whether a check after the restart is needed. After restarting the PC, open MKLM to check.",
        ),
    }
}

/// The result's next-step button (design m3 B.17).
pub fn next_step(step: crate::vm::result::NextStep, lang: Lang) -> String {
    use crate::vm::result::NextStep;
    match step {
        NextStep::Restart => pick(lang, "再起動の画面へ", "Go to the restart"),
        NextStep::PostReboot => pick(lang, "再起動後の確認へ", "Check after the restart"),
        NextStep::Conflict => pick(lang, "衝突を解決…", "Resolve…"),
        NextStep::Recovery => pick(lang, "回復…", "Recover…"),
        NextStep::ImeHelp => pick(lang, "入力方式の案内", "Input method guide"),
    }
}

/// Why a new change cannot start (the gate before any prompt, design m3 B.5): `what` is the
/// blocking operation in words (`vm::journal::kind_text`).
pub fn block_reason(reason: &BlockReason, what: &str, lang: Lang) -> String {
    match reason {
        BlockReason::JournalUnreadable { .. } => error_code(ErrorCode::JournalUnreadable, lang),
        BlockReason::Busy(_) => match lang {
            Lang::Ja => {
                format!("別の MKLM が処理中です（{what}）。終わってからもう一度試してください。")
            }
            Lang::En => {
                format!("Another MKLM process is working ({what}); try again when it has finished.")
            }
        },
        BlockReason::PostRebootCheck(_) => match lang {
            Lang::Ja => format!("PC の再起動後の確認がまだです（{what}）。先に確認してください。"),
            Lang::En => format!("The check after the restart is still open ({what}); do it first."),
        },
        BlockReason::NeedsRecovery(_) => match lang {
            Lang::Ja => format!("途中で止まった操作があります（{what}）。先に回復してください。"),
            Lang::En => format!("An operation was interrupted ({what}); recover it first."),
        },
        BlockReason::AwaitingUser(_) => match lang {
            Lang::Ja => format!(
                "確認待ちの変更があります（{what}）。先にそれをこのままにするか元に戻してください。"
            ),
            Lang::En => format!("A change is still waiting ({what}); keep or undo it first."),
        },
        BlockReason::WaitingForReboot(_) => match lang {
            Lang::Ja => format!(
                "PC の再起動を待っている変更があります（{what}）。再起動するまで、ほかの変更はできません。"
            ),
            Lang::En => format!(
                "A change waits for a PC restart ({what}); until then no other change is possible."
            ),
        },
        BlockReason::Conflict(_) => match lang {
            Lang::Ja => {
                format!("MKLM 以外による変更の確認が必要です（{what}）。先に解決してください。")
            }
            Lang::En => {
                format!("Values changed outside MKLM need a decision ({what}); resolve them first.")
            }
        },
    }
}

/// The tray icon's tooltip (design m3 B.16): "MKLM", with the most important journal attention
/// (the banner's, `vm::status::PRIORITY`) when there is one. `post_reboot`: the attention is the
/// post-reboot check; `unreadable`: the journal has entries this build cannot read.
pub fn tray_tooltip(
    attention: Option<Attention>,
    post_reboot: bool,
    unreadable: bool,
    lang: Lang,
) -> String {
    let detail = if unreadable {
        Some(pick(
            lang,
            "記録（ジャーナル）を読めません。MKLM を更新してください",
            "The journal cannot be read; update MKLM",
        ))
    } else if post_reboot && attention == Some(Attention::Recover) {
        Some(pick(
            lang,
            "PC の再起動後の確認が必要です",
            "Check the keyboards after the restart",
        ))
    } else {
        attention.and_then(|attention| self::attention(attention, lang))
    };
    match detail {
        Some(detail) => format!("MKLM — {detail}"),
        None => "MKLM".to_string(),
    }
}

/// Why the sign-in start switch has a note (design m3 B.14, F.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartNote {
    /// Task Manager (or Settings > Apps > Startup) turned it off; MKLM never turns it back on.
    DisabledByUser,
    /// The Run value could not be read.
    Unreadable,
    /// The last change could not be written.
    WriteFailed,
    /// This MKLM runs elevated: its HKCU may be another administrator's, so it does not change
    /// the value (the same rule as the post-reboot RunOnce value, design m3 F.2, F.3).
    Elevated,
}

/// The note next to the sign-in start switch.
pub fn autostart_note(note: AutostartNote, lang: Lang) -> String {
    match note {
        AutostartNote::DisabledByUser => pick(
            lang,
            "Windows のスタートアップ設定で無効になっています。タスク マネージャーの「スタートアップ アプリ」で有効にできます。",
            "Turned off in Windows' startup settings. You can turn it on under Startup apps in Task Manager.",
        ),
        AutostartNote::Unreadable => pick(
            lang,
            "サインイン時の起動の設定を読み取れませんでした。MKLM を開き直すと、もう一度読み取ります。",
            "The sign-in start setting could not be read. MKLM reads it again when you open it again.",
        ),
        AutostartNote::WriteFailed => pick(
            lang,
            "サインイン時の起動を変更できませんでした。もう一度切り替えるか、MKLM を開き直してから試してください。",
            "The sign-in start could not be changed. Switch it again, or open MKLM again and retry.",
        ),
        AutostartNote::Elevated => pick(
            lang,
            "管理者として実行している MKLM では、サインイン時の起動を変更できません。MKLM を通常の方法で開き直してから変更してください。",
            "The sign-in start cannot be changed while MKLM runs as administrator. Open MKLM the usual way and change it there.",
        ),
    }
}

// --- Settings and About (design m3 B.14, B.15; WP-U7) ---

/// What the settings page knows of "restore the keyboards when MKLM is uninstalled" (plan 3.13):
/// machine-wide, read unelevated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallRestore {
    On,
    Off,
    /// Not read yet.
    Reading,
    /// It could not be read: "変更…" is off (the new value would be a guess).
    Unreadable,
}

/// The setting's line on the settings page: "アンインストール時にキーボードの設定を元に戻す: オン".
pub fn uninstall_restore_status(value: UninstallRestore, lang: Lang) -> String {
    let state = match (value, lang) {
        (UninstallRestore::On, Lang::Ja) => "オン",
        (UninstallRestore::On, Lang::En) => "on",
        (UninstallRestore::Off, Lang::Ja) => "オフ",
        (UninstallRestore::Off, Lang::En) => "off",
        (UninstallRestore::Reading | UninstallRestore::Unreadable, Lang::Ja) => "不明",
        (UninstallRestore::Reading | UninstallRestore::Unreadable, Lang::En) => "unknown",
    };
    match lang {
        Lang::Ja => format!("アンインストール時にキーボードの設定を元に戻す: {state}"),
        Lang::En => format!("Put the keyboards back when MKLM is uninstalled: {state}"),
    }
}

/// What the setting means now, under its line.
pub fn uninstall_restore_note(value: UninstallRestore, lang: Lang) -> String {
    match value {
        UninstallRestore::On => pick(
            lang,
            "MKLM をアンインストールするとき、すべてのキーボードの設定を MKLM 導入前に戻します。この設定は、この PC のすべてのユーザーに適用されます。",
            "Uninstalling MKLM puts every keyboard's settings back to what they were before MKLM. This setting applies to every user of this PC.",
        ),
        UninstallRestore::Off => pick(
            lang,
            "MKLM をアンインストールしても、キーボードの設定は今のまま残ります。この設定は、この PC のすべてのユーザーに適用されます。",
            "Uninstalling MKLM leaves the keyboards' settings as they are. This setting applies to every user of this PC.",
        ),
        UninstallRestore::Reading => checking_now(lang),
        UninstallRestore::Unreadable => pick(
            lang,
            "この設定を読み取れなかったため、今は変更できません。設定のページを開き直すと、もう一度読み取ります。",
            "This setting could not be read, so it cannot be changed now. Open the settings page again to read it again.",
        ),
    }
}

/// The confirmation's title for the new value `on`.
pub fn uninstall_confirm_title(on: bool, lang: Lang) -> String {
    if on {
        pick(
            lang,
            "アンインストール時にキーボードの設定を元に戻すようにします",
            "Put the keyboards back when MKLM is uninstalled",
        )
    } else {
        pick(
            lang,
            "アンインストール時にキーボードの設定を元に戻さないようにします",
            "Leave the keyboards as they are when MKLM is uninstalled",
        )
    }
}

/// What the new value `on` means, and why Windows asks.
pub fn uninstall_confirm_text(on: bool, lang: Lang) -> String {
    let what = if on {
        pick(
            lang,
            "MKLM をアンインストールするとき、すべてのキーボードの設定を MKLM 導入前に戻します。",
            "Uninstalling MKLM will put every keyboard's settings back to what they were before MKLM.",
        )
    } else {
        pick(
            lang,
            "MKLM をアンインストールしても、キーボードの設定は MKLM で最後に設定したまま残り、MKLM 導入前には戻りません。",
            "Uninstalling MKLM will leave the keyboards as MKLM last set them; they will not go back to what they were before MKLM.",
        )
    };
    let why = pick(
        lang,
        "この設定は PC のすべてのユーザーに適用されるため、保存には管理者の許可が必要です。キーボードの配列は今は変わりません。",
        "This setting applies to every user of this PC, so saving it needs an administrator's permission. No keyboard's layout changes now.",
    );
    format!("{what}{}{why}", if lang == Lang::Ja { "" } else { " " })
}

/// The confirmation's button; "（次に Windows の確認が出ます）" unless the GUI runs elevated.
pub fn uninstall_confirm_button(on: bool, prompt: bool, lang: Lang) -> String {
    match (on, prompt) {
        (true, true) => pick(
            lang,
            "オンにする（次に Windows の確認が出ます）",
            "Turn on (Windows asks next)",
        ),
        (true, false) => pick(lang, "オンにする", "Turn on"),
        (false, true) => pick(
            lang,
            "オフにする（次に Windows の確認が出ます）",
            "Turn off (Windows asks next)",
        ),
        (false, false) => pick(lang, "オフにする", "Turn off"),
    }
}

/// The result of `Request::SetMachineSettings` (design m3 B.17): what is saved now.
pub fn uninstall_restore_saved(on: bool, lang: Lang) -> String {
    if on {
        pick(
            lang,
            "保存しました。MKLM をアンインストールするとき、キーボードの設定を MKLM 導入前に戻します。",
            "Saved. Uninstalling MKLM will put the keyboards back to what they were before MKLM.",
        )
    } else {
        pick(
            lang,
            "保存しました。MKLM をアンインストールしても、キーボードの設定は今のまま残ります。",
            "Saved. Uninstalling MKLM will leave the keyboards as they are.",
        )
    }
}

/// "すべてのキーボードを MKLM 導入前に戻す" (design m3 B.14): the change page's title for
/// `RestoreScope::All`.
pub fn restore_all_title(lang: Lang) -> String {
    pick(
        lang,
        "すべてのキーボードを MKLM 導入前に戻す",
        "Put every keyboard back to before MKLM",
    )
}

/// The keyboards of a restore of everything, as one name.
pub fn every_keyboard(lang: Lang) -> String {
    pick(lang, "すべてのキーボード", "Every keyboard")
}

/// About's version line: "バージョン 0.1.0（ビルド 0123abcd…）" (design m3 B.15).
pub fn about_version(version: &str, build_id: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("バージョン {version}（ビルド {build_id}）"),
        Lang::En => format!("Version {version} (build {build_id})"),
    }
}

/// The main third-party components and their licences (design m3 B.15), until M5 embeds the full
/// list that cargo-about generates. Names stay as their authors write them.
pub fn third_party_components(lang: Lang) -> Vec<String> {
    let either = pick(
        lang,
        "MIT ライセンスまたは Apache License 2.0",
        "MIT License or Apache License 2.0",
    );
    vec![
        "Slint: Slint Royalty-free License 2.0".to_string(),
        format!("windows-rs (windows, windows-registry): {either}"),
        format!("serde, serde_json: {either}"),
        format!("toml: {either}"),
    ]
}

/// A problem that stops MKLM before or instead of its window (design m3 F.6). Shown in a message
/// box: release builds have no console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupError {
    /// Older than Windows 11 24H2 (plan 4.1).
    WindowsTooOld { build: u32 },
    /// The Windows version could not be read.
    WindowsUnknown,
    /// The window system, the window or a worker thread could not start.
    CannotStart,
    /// The event loop failed while MKLM ran.
    Stopped,
}

/// The message box for `error`: `(title, text)`. `detail` is the English diagnostic, shown under
/// "technical details".
pub fn startup_error(error: StartupError, detail: &str, lang: Lang) -> (String, String) {
    let title = pick(lang, "MKLM を起動できません", "MKLM cannot start");
    let text = match (error, lang) {
        (StartupError::WindowsTooOld { build }, Lang::Ja) => format!(
            "MKLM には Windows 11 24H2 以降が必要です。この PC の Windows はビルド {build} です。Windows を更新してから、もう一度起動してください。"
        ),
        (StartupError::WindowsTooOld { build }, Lang::En) => format!(
            "MKLM needs Windows 11 24H2 or later. This PC runs Windows build {build}. Update Windows with Windows Update, then start MKLM again."
        ),
        (StartupError::WindowsUnknown, _) => pick(
            lang,
            "Windows の版を確認できませんでした。PC を再起動してから、もう一度起動してください。",
            "The Windows version could not be read. Restart the PC, then start MKLM again.",
        ),
        (StartupError::CannotStart, _) => pick(
            lang,
            "MKLM の画面を表示できませんでした。PC を再起動してから、もう一度起動してください。",
            "MKLM could not open its window. Restart the PC, then start MKLM again.",
        ),
        (StartupError::Stopped, _) => pick(
            lang,
            "MKLM が予期せず終了しました。キーボードの設定を変更している途中だった場合は、次に MKLM を起動したときに回復を案内します。",
            "MKLM stopped unexpectedly. If a keyboard change was in progress, MKLM offers the recovery the next time it starts.",
        ),
    };
    let details = pick(lang, "技術的な詳細", "Technical details");
    let title = match error {
        StartupError::Stopped => pick(lang, "MKLM が終了しました", "MKLM stopped"),
        _ => title,
    };
    (title, format!("{text}\n\n{details}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages() {
        assert_eq!(LangChoice::System.resolve(0x0411), Lang::Ja);
        assert_eq!(LangChoice::System.resolve(0x0409), Lang::En);
        assert_eq!(LangChoice::Ja.resolve(0x0409), Lang::Ja);
        assert_eq!(Lang::En.bundle(), "");
        assert_eq!(Lang::Ja.font_family(), "Yu Gothic UI");
        for i in 0..3 {
            assert_eq!(LangChoice::from_index(i).index(), i);
        }
    }

    #[test]
    fn the_r5_reason_depends_on_the_reset() {
        let before = failure(
            &FailureReason::LiveResetUnconfirmed,
            ResetPhase::NotReached,
            Lang::En,
        );
        assert!(before.contains("before the keyboard reset"), "{before}");
        let after = failure(
            &FailureReason::LiveResetUnconfirmed,
            ResetPhase::Reached,
            Lang::Ja,
        );
        assert!(after.contains("リセット後"), "{after}");
    }

    #[test]
    fn process_texts() {
        // Japanese output: only device names and allowlisted Latin words (design m3 H.1).
        let keychron = "Keychron Receiver";
        let mut japanese = vec![
            tray_tooltip(Some(Attention::WaitingForReboot), false, false, Lang::Ja),
            tray_tooltip(Some(Attention::Recover), true, false, Lang::Ja),
            tray_tooltip(None, false, true, Lang::Ja),
        ];
        for note in [
            AutostartNote::DisabledByUser,
            AutostartNote::Unreadable,
            AutostartNote::WriteFailed,
            AutostartNote::Elevated,
        ] {
            japanese.push(autostart_note(note, Lang::Ja));
            assert!(!autostart_note(note, Lang::En).is_empty());
        }
        for error in [
            StartupError::WindowsTooOld { build: 22631 },
            StartupError::WindowsUnknown,
            StartupError::CannotStart,
            StartupError::Stopped,
        ] {
            let (title, text) = startup_error(error, "", Lang::Ja);
            japanese.push(title);
            japanese.push(text);
        }
        // "24H2" names a Windows release, like a device name.
        let names = [keychron, "24H2"];
        for text in &japanese {
            assert!(
                crate::vm::unexpected_latin(text, &names).is_empty(),
                "{text}: {:?}",
                crate::vm::unexpected_latin(text, &names)
            );
        }
        // The tooltip is the banner's attention.
        assert_eq!(tray_tooltip(None, false, false, Lang::Ja), "MKLM");
        assert_eq!(
            tray_tooltip(Some(Attention::WaitingForReboot), false, false, Lang::Ja),
            "MKLM — PC の再起動を待っている変更があります"
        );
        assert_eq!(
            tray_tooltip(Some(Attention::None), false, false, Lang::En),
            "MKLM"
        );
        // The English diagnostic is kept under "technical details".
        let (title, text) = startup_error(
            StartupError::WindowsTooOld { build: 22631 },
            "build 22631 < 26100",
            Lang::En,
        );
        assert_eq!(title, "MKLM cannot start");
        assert!(text.contains("22631") && text.ends_with("Technical details: build 22631 < 26100"));
        assert_eq!(
            autostart_note(AutostartNote::DisabledByUser, Lang::Ja)
                .split('。')
                .next(),
            Some("Windows のスタートアップ設定で無効になっています")
        );
    }

    /// The notation of the Japanese screens (design m3 D.5): MKLM's buttons in ［］, other
    /// quotes in 「」, never 『』; and never "確定" (say what the user did: "このままにする").
    /// Checked over every text source of the screens — the Rust texts and the catalogue of the
    /// static labels — outside comments.
    #[test]
    fn japanese_notation() {
        let sources = [
            ("src/i18n.rs", include_str!("i18n.rs")),
            (
                "src/i18n/journal_pages.rs",
                include_str!("i18n/journal_pages.rs"),
            ),
            ("src/i18n/wizard.rs", include_str!("i18n/wizard.rs")),
            (
                "translations/ja/LC_MESSAGES/mklm.po",
                include_str!("../translations/ja/LC_MESSAGES/mklm.po"),
            ),
        ];
        // Spelled with escapes, so that this test's own source passes.
        let forbidden = ["\u{300e}", "\u{300f}", "\u{78ba}\u{5b9a}"];
        for (name, source) in sources {
            for (number, line) in source.lines().enumerate() {
                let text = line.trim_start();
                if text.starts_with("//") || text.starts_with('#') {
                    continue;
                }
                for word in forbidden {
                    assert!(
                        !text.contains(word),
                        "{name}:{}: {word:?} in {text}",
                        number + 1
                    );
                }
            }
        }
        // The texts that used them before.
        assert_eq!(state(OpState::Confirmed, Lang::Ja), "このままにしました");
        assert_eq!(
            failure(
                &FailureReason::CallerDisconnected,
                ResetPhase::Reached,
                Lang::Ja
            ),
            "［このままにする］が選ばれる前に MKLM が終了したため、元に戻しました"
        );
        assert!(countdown_how(20, Lang::Ja).contains("Tab で［このままにする］へ"));
    }

    /// English counts of one are singular (review of WP-U5).
    #[test]
    fn english_counts() {
        assert_eq!(
            restore_summary(1, Lang::En),
            "1 value goes back to what it was before MKLM."
        );
        assert_eq!(
            restore_summary(2, Lang::En),
            "2 values go back to what they were before MKLM."
        );
        assert!(restore_conflicts(1, Lang::En).starts_with("1 value was changed"));
        assert!(restore_removed(1, Lang::En).starts_with("1 value of a keyboard"));
        assert!(restore_supersedes(1, Lang::En).starts_with("1 change waiting for you is"));
        assert!(journal_pages::history_unreadable(1, Lang::En).starts_with("1 journal entry "));
        assert_eq!(
            recognition(
                true,
                &[arrival("Keychron Receiver", "JIS", Lang::En)],
                Lang::En
            ),
            "Windows reports: Keychron Receiver: JIS ✓"
        );
        assert_eq!(countdown_reminder(10, Lang::En), "Reverting in 10 seconds");
    }

    /// A preparation that failed names buttons of the page it is shown on (review of WP-U3).
    #[test]
    fn preparation_failures_name_the_pages_buttons() {
        let change = prepare_failed(true, PreparePlace::ChangePage, Lang::Ja);
        assert!(change.contains("［キャンセル］") && change.contains("［最新の情報に更新］"));
        let wizard = prepare_failed(false, PreparePlace::Wizard, Lang::Ja);
        assert!(wizard.contains("［戻る］") && wizard.contains("［次へ］"));
        assert!(!wizard.contains("最新の情報に更新"));
    }

    #[test]
    fn input_methods() {
        assert!(input_method(0x0411_0411, Lang::Ja).1);
        assert!(input_method(0xE001_0411, Lang::Ja).1);
        // Japanese language with the US layout: every keyboard types US.
        assert!(!input_method(0x0409_0411, Lang::Ja).1);
        assert!(!input_method(0x0409_0409, Lang::Ja).1);
        assert!(
            input_method(0x0409_0409, Lang::En)
                .0
                .starts_with("English (US)")
        );
    }
}
