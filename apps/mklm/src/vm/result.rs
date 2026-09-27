//! The result of a request (design m3 B.17): title and tone from `OutcomeClass`, what the
//! keyboards do **now** (from the read that follows every session, review U7), the reason
//! (`i18n::failure` with the reset phase, design m3 G.2), the next step, and the English
//! diagnostics under "技術的な詳細" with a "詳細をコピー" button.

use mklm_client::orchestrator::RequestReport;
use mklm_client::run_once::{RunOnceError, RunOnceOutcome};

use super::Tone;
use crate::i18n::Lang;
use crate::state::SystemRead;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResultView {
    pub title: String,
    /// "今の状態": one line per keyboard of the request ("Keychron Receiver: US として動作中
    /// （変更前のまま）"), from the `SystemRead` after the session; "確かめています…" until it
    /// arrives.
    pub current_state: Vec<String>,
    pub message: String,
    pub tone: Tone,
    /// US was kept: "US 配列には『半角/全角』キーがありません…［入力方式の案内］" (plan 3.4,
    /// review U19).
    pub ime_note: String,
    /// The RunOnce rule failed or cannot be applied (elevated GUI): the user runs the check.
    pub run_once_note: String,
    /// The button of the next step ("再起動の画面へ", "衝突を解決…"); empty when none.
    pub next_step: String,
    pub next: Option<NextStep>,
    pub details: String,
    /// The text "詳細をコピー" puts on the clipboard: English diagnostics, the short operation
    /// ID, the build ID (`mklm_win::ui::copy_text_to_clipboard`).
    pub copy_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    Restart,
    Conflict,
    Recovery,
    ImeHelp,
}

/// Rules (WP-U3): `OutcomeClass` from `classify_result` / `classify_error` /
/// `classify_lost_recovery` (the recovery session's result when the helper was lost);
/// `NotLaunched(Declined)` is "cancelled, nothing changed"; `Abandoned` shows nothing; a skipped
/// recovery (`RequestReport::recovery_skipped`) says the change is waiting and how to recover.
/// "変更は元に戻されています" is said only when the journal entry says so (`Reverted`,
/// `Failed`), never assumed from an error code.
pub fn result_view(
    report: &RequestReport,
    run_once: &Result<RunOnceOutcome, RunOnceError>,
    after: Option<&SystemRead>,
    lang: Lang,
) -> ResultView {
    let _ = (report, run_once, after, lang);
    todo!("WP-U3: the result view-model")
}
