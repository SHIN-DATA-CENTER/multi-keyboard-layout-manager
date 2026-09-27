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
use mklm_client::outcome::HelperExitKind;
use mklm_client::{HelperExit, LaunchError, LaunchFailure, OutcomeClass};
use mklm_core::{
    Attention, EffectiveLayout, ErrorCode, FailureReason, GlobalMode, LayoutBasis, LayoutTable,
    OpState, PendingAction, Transport,
};
use serde::{Deserialize, Serialize};

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

/// How a change takes effect, as a sentence for the change page (plan 3.5; design m3 B.5).
/// `seconds` is the keep-or-revert time (20, or 60 with the accessibility setting, design m3
/// B.6).
pub fn takes_effect(action: PendingAction, seconds: u32, lang: Lang) -> String {
    match action {
        PendingAction::ResetKeyboard => match lang {
            Lang::Ja => format!(
                "キーボードをその場でリセットします（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。数秒間このキーボードで入力できません。その後 {seconds} 秒以内に「このままにする」を選ばないと元に戻ります。"
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

/// The badges of a keyboard row (plan 3.2; design m3 B.2, E.3): `(label, screen-reader text)`.
/// "Not verified by typing" is no badge: it is said once, on the "標準に従う" choice (review U17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadgeKind {
    /// No key press seen from this device yet (never on a built-in keyboard).
    NoKeyPress,
    /// A receiver: every keyboard paired with it gets the same layout.
    Receiver,
    /// RDP or a virtual keyboard: shown, never changed.
    ReadOnly,
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
            "キー入力なし: このデバイスからのキー入力をまだ見ていません。マウスなどの付属機能のことがあります。『キーを押して特定』で確かめられます",
            "No key press yet",
            "No key press yet: no key press from this device has been seen. It may be part of a mouse or another device; \"Identify by key press\" tells",
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
        OpState::Confirmed => ("確定", "Kept"),
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
            "確定する前に MKLM が終了したため、元に戻しました",
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
                "キーボードのリセット後に確定されなかったため、回復で元に戻しました",
                "The change was never kept after the keyboard reset; recovery put it back",
            ),
        },
        FailureReason::CountdownExpired => pick(
            lang,
            "時間内に「このままにする」が選ばれなかったため、元に戻しました",
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
            "確認待ちの変更があります。先にそれを「このままにする」か元に戻してください。",
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
