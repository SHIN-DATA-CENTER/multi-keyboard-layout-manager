//! Texts of the pages about journal entries (design m3 B.8 to B.12; WP-U4, WP-U5): the PC
//! restart, the check after it, conflicts, the history and the recovery page with its undo and
//! revert previews. Like the rest of `i18n`, every text has both languages here, typed by what it
//! is about; the view-models in `vm::{restart, post_reboot, conflict, journal, recovery}` only
//! assemble them.
//!
//! Screen words (design m3 D.5, review U5): "確認待ちの変更" for anything that waits for keep or
//! revert, never "確定"; layouts by name ("JIS 配列", "不明な種類（8/2）"), the numbers only in
//! the technical details; "MKLM の管理用プログラム（mklm-helper.exe）", never "helper".

use mklm_core::{KeyboardType, Layout, LayoutChoice, LayoutTable, OpKind, OpState, RestoreScope};

use super::{Lang, pick};

// --- Operations and layout names --------------------------------------------------------------

/// What an operation was for, in one line ("Keychron Receiver を JIS に"); `name_of` names a
/// keyboard by instance ID (design m3 B.11: no instance IDs on screen).
pub fn operation(kind: &OpKind, name_of: &dyn Fn(&str) -> String, lang: Lang) -> String {
    match kind {
        OpKind::SetLayout {
            requested, layout, ..
        } => {
            let name = name_of(requested);
            match (layout, lang) {
                (LayoutChoice::Standard, Lang::Ja) => format!("{name} を標準に従う設定に"),
                (LayoutChoice::Standard, Lang::En) => format!("{name} to follow the standard"),
                (layout, Lang::Ja) => format!("{name} を {} に", layout_choice(*layout, lang)),
                (layout, Lang::En) => format!("{name} to {}", layout_choice(*layout, lang)),
            }
        }
        OpKind::Migrate { standard, .. } => {
            let standard = match standard {
                Layout::Jis => "JIS",
                Layout::Us => "US",
            };
            match lang {
                Lang::Ja => format!("キーボードごとモードへ移行（標準配列 {standard}）"),
                Lang::En => format!("Switch to per-keyboard mode (standard {standard})"),
            }
        }
        OpKind::RestoreBaseline { scope, .. } => restore_operation(scope, name_of, lang),
        OpKind::Cleanup { instance_id, .. } => super::cleanup_text(&name_of(instance_id), lang),
    }
}

/// A restore to before MKLM: "MKLM 導入前に戻す（Keychron Receiver）".
pub fn restore_operation(
    scope: &RestoreScope,
    name_of: &dyn Fn(&str) -> String,
    lang: Lang,
) -> String {
    match (scope, lang) {
        (RestoreScope::All, Lang::Ja) => "MKLM 導入前に戻す（すべて）".into(),
        (RestoreScope::All, Lang::En) => "Back to before MKLM (everything)".into(),
        (RestoreScope::Device { instance_id }, Lang::Ja) => {
            format!("MKLM 導入前に戻す（{}）", name_of(instance_id))
        }
        (RestoreScope::Device { instance_id }, Lang::En) => {
            format!("Back to before MKLM ({})", name_of(instance_id))
        }
    }
}

/// A layout choice: "JIS", "US", "標準に従う".
pub fn layout_choice(choice: LayoutChoice, lang: Lang) -> String {
    match choice {
        LayoutChoice::Jis => "JIS".into(),
        LayoutChoice::Us => "US".into(),
        LayoutChoice::Standard => pick(lang, "標準に従う", "follow the standard"),
    }
}

/// What a pair of type values means, in words (review U5: the numbers go to the details).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairName {
    /// A complete pair.
    Type(KeyboardType),
    /// Neither value exists on a HID keyboard: it follows the PC's standard layout.
    HidAbsent,
    /// Neither value exists on a PS/2 keyboard: the PC-wide values decide.
    Ps2Absent,
    /// Neither PC-wide value exists: per-keyboard mode.
    GlobalAbsent,
    /// The PC-wide pair: fixed mode with this type.
    GlobalFixed(KeyboardType),
    /// Only the standard layout (`LayerDriver JPN`) is involved.
    Standard(LayoutTable),
    /// One value missing, a value of another type, or not read.
    Unknown,
}

fn type_name(ty: KeyboardType, lang: Lang) -> String {
    match ty {
        KeyboardType::JIS => "JIS".into(),
        KeyboardType::US => "US".into(),
        other => match lang {
            Lang::Ja => format!("不明な種類（{}/{}）", other.ty, other.subtype),
            Lang::En => format!("an unknown type ({}/{})", other.ty, other.subtype),
        },
    }
}

fn table_name(table: &LayoutTable, lang: Lang) -> String {
    super::table(table, lang)
}

pub fn pair_name(pair: PairName, lang: Lang) -> String {
    match pair {
        PairName::Type(ty) => type_name(ty, lang),
        PairName::HidAbsent => pick(
            lang,
            "値なし（標準に従う）",
            "no value (follows the standard)",
        ),
        PairName::Ps2Absent => pick(
            lang,
            "値なし（PC 全体の設定に従う）",
            "no value (follows the PC-wide setting)",
        ),
        PairName::GlobalAbsent => pick(lang, "キーボードごとモード", "per-keyboard mode"),
        PairName::GlobalFixed(ty) => match lang {
            Lang::Ja => format!("固定モード（{}）", type_name(ty, lang)),
            Lang::En => format!("fixed mode ({})", type_name(ty, lang)),
        },
        PairName::Standard(table) => match lang {
            Lang::Ja => format!("標準配列 {}", table_name(&table, lang)),
            Lang::En => format!("standard layout {}", table_name(&table, lang)),
        },
        PairName::Unknown => pick(lang, "不明な値", "unknown values"),
    }
}

/// What Windows reports for a keyboard, in words: "JIS 配列", "US 配列", "不明な種類（8/2）";
/// kbdhid's own default (no stored type) follows the PC's standard layout.
pub fn reported(ty: KeyboardType, lang: Lang) -> String {
    match (ty, lang) {
        (KeyboardType::JIS, Lang::Ja) => "JIS 配列".into(),
        (KeyboardType::US, Lang::Ja) => "US 配列".into(),
        (KeyboardType::JIS, Lang::En) => "JIS".into(),
        (KeyboardType::US, Lang::En) => "US".into(),
        (KeyboardType::HID_UNKNOWN, _) => pick(
            lang,
            "種類の指定なし（標準に従う）",
            "no type (follows the standard)",
        ),
        (other, _) => type_name(other, lang),
    }
}

// --- Shared ----------------------------------------------------------------------------------

/// The two ways a revert, an undo or a recovery takes effect on a HID keyboard (design m3 B.5):
/// `(text, detail)`. Unlike a change, putting values back never blocks other changes, so the
/// second choice only delays when the keyboard types the old layout again.
pub fn revert_method(live: bool, lang: Lang) -> (String, String) {
    if live {
        (
            pick(lang, "すぐに元の配列に戻す", "Switch back now"),
            pick(
                lang,
                "キーボードをその場でリセットします（Windows がキーボードを接続し直します。キーボード本体の設定は変わりません）。数秒間このキーボードで入力できません。",
                "The keyboard is reset in place (Windows reconnects it; the keyboard's own settings do not change); it cannot type for a few seconds.",
            ),
        )
    } else {
        (
            pick(
                lang,
                "抜き差しか PC の再起動で戻す",
                "Switch back when it is replugged or the PC restarts",
            ),
            pick(
                lang,
                "設定はすぐに戻りますが、キーボードを抜き差しするか PC を再起動するまで、今の配列のまま動きます。",
                "The settings go back now, but the keyboard keeps its current layout until it is replugged or the PC restarts.",
            ),
        )
    }
}

/// Why the post-reboot RunOnce value could not be registered (design m3 B.17 `run_once_note`):
/// `elevated` is `RunOnceOutcome::TellUser`, otherwise the rule failed.
pub fn run_once_problem(elevated: bool, lang: Lang) -> String {
    if elevated {
        pick(
            lang,
            "管理者として実行している MKLM は、再起動後の確認を自動では開けません。PC を再起動した後に、MKLM を通常どおり開いてください。",
            "MKLM runs as administrator and cannot open the check after the restart by itself. After the restart, open MKLM as usual.",
        )
    } else {
        pick(
            lang,
            "再起動後の確認を自動で開けるように登録できませんでした。PC を再起動した後に、MKLM を開いてください。",
            "The check after the restart could not be registered to open by itself. After the restart, open MKLM.",
        )
    }
}

// --- Restart (B.8) ---------------------------------------------------------------------------

/// "キーボードごとモードへ移行（標準配列 JIS）: PC の再起動待ち".
pub fn restart_reason(what: &str, state: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{what}: {state}"),
        Lang::En => format!("{what}: {state}"),
    }
}

pub fn restart_nothing(lang: Lang) -> String {
    pick(
        lang,
        "PC の再起動を待っている変更はありません。",
        "No change waits for a PC restart.",
    )
}

pub fn restart_failed(lang: Lang) -> String {
    pick(
        lang,
        "PC を再起動できませんでした。スタート メニューの電源ボタンから「再起動」を選んでください（シャットダウンではなく再起動）。",
        "The PC could not be restarted. Choose Restart from the power button in the Start menu (Restart, not Shut down).",
    )
}

pub fn restarting(lang: Lang) -> String {
    pick(lang, "PC を再起動しています…", "Restarting the PC…")
}

// --- The check after a restart (B.9) ---------------------------------------------------------

/// The "setting" column: the layout the change set.
pub fn check_setting(table: Option<&LayoutTable>, standard: bool, lang: Lang) -> String {
    let name = table.map_or_else(|| "—".to_string(), |table| table_name(table, lang));
    match (standard, lang) {
        (true, Lang::Ja) => format!("標準（{name}）"),
        (true, Lang::En) => format!("standard ({name})"),
        (false, _) => name,
    }
}

/// What Windows reports, in words, against what the stored values predict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recognition {
    Matches(KeyboardType),
    Differs(KeyboardType),
    NotConnected,
}

pub fn check_recognition(recognition: Recognition, lang: Lang) -> String {
    match recognition {
        Recognition::Matches(ty) => format!("{} ✓", reported(ty, lang)),
        Recognition::Differs(ty) => match lang {
            Lang::Ja => format!("{} ⚠ 違います", reported(ty, lang)),
            Lang::En => format!("{} ⚠ differs", reported(ty, lang)),
        },
        Recognition::NotConnected => pick(lang, "未接続", "not connected"),
    }
}

/// The "typed" column (review U2): not tried yet, as expected, or not as expected.
pub fn check_typed(typed: Option<bool>, lang: Lang) -> String {
    match typed {
        None => pick(lang, "未", "not yet"),
        Some(true) => "✓".into(),
        Some(false) => "⚠".into(),
    }
}

/// The numbers behind a check row (technical details only).
pub fn check_details(
    expected: Option<KeyboardType>,
    reported: Option<KeyboardType>,
    lang: Lang,
) -> String {
    let text = |ty: Option<KeyboardType>| ty.map_or_else(|| "—".to_string(), |ty| ty.to_string());
    match lang {
        Lang::Ja => format!(
            "保存値からの予想 {}、Windows の報告 {}",
            text(expected),
            text(reported)
        ),
        Lang::En => format!(
            "expected from the stored values {}, reported by Windows {}",
            text(expected),
            text(reported)
        ),
    }
}

pub fn check_migration_note(lang: Lang) -> String {
    pick(
        lang,
        "移行が反映されたかは、Windows の認識では分かりません（内蔵キーボードは移行の前後で同じ種類を報告します）。打鍵テストで確かめてください。",
        "Whether the migration took effect cannot be told from what Windows reports (the built-in keyboard reports the same type before and after it). Check with the key test.",
    )
}

pub fn check_keep_warning(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => {
            format!("⚠ {name} が期待と違う配列で動いています。［元に戻す］をおすすめします。")
        }
        Lang::En => {
            format!("⚠ {name} types a different layout than expected. Revert is recommended.")
        }
    }
}

/// The revert button of the check (review U15 c).
pub fn check_revert(needs_restart: bool, lang: Lang) -> String {
    if needs_restart {
        pick(
            lang,
            "元に戻す（もう一度 PC の再起動が必要）",
            "Revert (needs another PC restart)",
        )
    } else {
        pick(lang, "元に戻す", "Revert")
    }
}

pub fn check_nothing(lang: Lang) -> String {
    pick(
        lang,
        "再起動の後に確認する変更はありません。",
        "No change waits for a check after a restart.",
    )
}

// --- Conflicts (B.10) ------------------------------------------------------------------------

/// The row of the PC-wide values.
pub fn conflict_global_name(lang: Lang) -> String {
    pick(lang, "PC 全体の設定", "PC-wide settings")
}

/// "今の値: 不明な種類（8/2）— MKLM 以外が変更".
pub fn conflict_now(now: Option<&str>, outside: bool, lang: Lang) -> String {
    let unread = pick(lang, "読み取れません", "cannot be read");
    let now = now.unwrap_or(&unread);
    match (outside, lang) {
        (true, Lang::Ja) => format!("今の値: {now} — MKLM 以外が変更"),
        (true, Lang::En) => format!("Now: {now} — changed outside MKLM"),
        (false, Lang::Ja) => format!("今の値: {now}"),
        (false, Lang::En) => format!("Now: {now}"),
    }
}

/// One way to resolve a keyboard, by layout name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictOptionText<'a> {
    Before(&'a str),
    Intended(&'a str),
    KeepCurrent,
    Baseline(&'a str),
}

/// A layout name inside a Japanese sentence: spaced from the Japanese text only where it begins or
/// ends with a Latin letter or digit ("変更前の US に戻す", "変更前の固定モード（JIS）に戻す").
pub fn ja_word(name: &str) -> String {
    let before = if name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        " "
    } else {
        ""
    };
    let after = if name
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        " "
    } else {
        ""
    };
    format!("{before}{name}{after}")
}

pub fn conflict_option(option: ConflictOptionText<'_>, recommended: bool, lang: Lang) -> String {
    let text = match (option, lang) {
        (ConflictOptionText::Before(name), Lang::Ja) => format!("変更前の{}に戻す", ja_word(name)),
        (ConflictOptionText::Before(name), Lang::En) => {
            format!("Back to {name}, as before the change")
        }
        (ConflictOptionText::Intended(name), Lang::Ja) => {
            format!("MKLM が設定した{}にする", ja_word(name))
        }
        (ConflictOptionText::Intended(name), Lang::En) => format!("{name}, as MKLM set it"),
        (ConflictOptionText::KeepCurrent, Lang::Ja) => "今の値のまま".to_string(),
        (ConflictOptionText::KeepCurrent, Lang::En) => "Keep the current values".to_string(),
        (ConflictOptionText::Baseline(name), Lang::Ja) => {
            format!("MKLM 導入前の{}に戻す", ja_word(name))
        }
        (ConflictOptionText::Baseline(name), Lang::En) => {
            format!("Back to {name}, as before MKLM")
        }
    };
    match (recommended, lang) {
        (true, Lang::Ja) => format!("{text}（おすすめ）"),
        (true, Lang::En) => format!("{text} (recommended)"),
        (false, _) => text,
    }
}

/// A value MKLM could not write although it tried again (design m2 C3); the error itself goes
/// to the details.
pub fn conflict_write_error(lang: Lang) -> String {
    pick(
        lang,
        "MKLM が書き込めなかった値があります。PC を再起動してからもう一度試してください。",
        "MKLM could not write some values. Restart the PC and try again.",
    )
}

/// The per-value override in the details: index 0 follows the keyboard's choice.
pub fn conflict_value_options(lang: Lang) -> Vec<String> {
    let (ja, en): (&[&str], &[&str]) = (
        &[
            "キーボードの選択に従う",
            "今の値",
            "操作の前の値",
            "操作で書こうとした値",
            "MKLM 導入前の値",
        ],
        &[
            "As chosen for the keyboard",
            "The current value",
            "The value before the operation",
            "The value the operation meant to write",
            "The value before MKLM",
        ],
    );
    match lang {
        Lang::Ja => ja.iter().map(|text| (*text).to_string()).collect(),
        Lang::En => en.iter().map(|text| (*text).to_string()).collect(),
    }
}

/// The numbers of one value (details): the terms are the same everywhere (design m3 B.10).
#[derive(Debug)]
pub struct ValueNumbers<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub current: &'a str,
    pub last_written: &'a str,
    pub before: &'a str,
    pub intended: &'a str,
    pub baseline: &'a str,
    pub write_error: Option<&'a str>,
}

pub fn conflict_value_line(value: &ValueNumbers<'_>, lang: Lang) -> String {
    let error = value.write_error.map(|error| match lang {
        Lang::Ja => format!(" / 書き込みのエラー: {error}"),
        Lang::En => format!(" / write error: {error}"),
    });
    let error = error.unwrap_or_default();
    match lang {
        Lang::Ja => format!(
            "{}\\{}: 今の値 {} / MKLM が最後に書いた値 {} / 操作の前の値 {} / 操作で書こうとした値 {} / MKLM 導入前の値 {}{error}",
            value.key,
            value.name,
            value.current,
            value.last_written,
            value.before,
            value.intended,
            value.baseline
        ),
        Lang::En => format!(
            "{}\\{}: current {} / last written by MKLM {} / before the operation {} / meant to write {} / before MKLM {}{error}",
            value.key,
            value.name,
            value.current,
            value.last_written,
            value.before,
            value.intended,
            value.baseline
        ),
    }
}

/// The helper refused a resolution that leaves a PS/2 keyboard without a fixed layout
/// (INV-PS2, design m2 D.8): which keyboards, and the two ways out.
pub fn conflict_inv_ps2(names: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "この選択では、配列が決まらない PS/2 キーボードが残ります（{names}）。PC 全体の設定を「変更前」に戻す（固定モードに戻す）か、［確認待ちの変更をすべて元に戻す…］を選んでください。"
        ),
        Lang::En => format!(
            "With these choices a PS/2 keyboard would be left without a fixed layout ({names}). Put the PC-wide settings back as before the change (fixed mode), or choose \"Undo every change waiting for you…\"."
        ),
    }
}

/// A restore to before MKLM stopped before writing anything, because some of its values were
/// changed outside MKLM (`ConflictPolicy::Report`, design m3 B.10).
pub fn restore_conflict_note(lang: Lang) -> String {
    pick(
        lang,
        "「MKLM 導入前に戻す」で戻す値のうち、MKLM 以外が変更した値がありました。まだ何も戻していません。その値をどうするかを選んでください。",
        "Some of the values the restore to before MKLM would put back were changed outside MKLM. Nothing has been put back yet; choose what to do with them.",
    )
}

/// What a keyboard had before MKLM, in words: "MKLM 導入前: US".
pub fn restore_conflict_baseline(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("MKLM 導入前: {name}"),
        Lang::En => format!("Before MKLM: {name}"),
    }
}

/// The two ways to send the restore again: leave the values changed outside MKLM
/// (`ConflictPolicy::Skip`) or put them back too (`ConflictPolicy::Overwrite`).
pub fn restore_conflict_choice(overwrite: bool, recommended: bool, lang: Lang) -> String {
    let text = if overwrite {
        pick(
            lang,
            "MKLM 以外が変えた値も MKLM 導入前の値に戻す",
            "Put those values back to before MKLM too",
        )
    } else {
        pick(
            lang,
            "MKLM 以外が変えた値はそのままにする（ほかの値は導入前に戻します）",
            "Leave those values as they are (the others go back to before MKLM)",
        )
    };
    match (recommended, lang) {
        (true, Lang::Ja) => format!("{text}（おすすめ）"),
        (true, Lang::En) => format!("{text} (recommended)"),
        (false, _) => text,
    }
}

pub fn conflict_nothing(lang: Lang) -> String {
    pick(
        lang,
        "MKLM 以外による変更は見つかっていません。",
        "No values changed outside MKLM are waiting for a decision.",
    )
}

// --- History (B.11) --------------------------------------------------------------------------

/// A local date and time for the history: "2026/09/27 22:36" / "2026-09-27 22:36" (design m3
/// G.1: seconds and the offset only in the details). `utc` marks the fallback when the local
/// time could not be computed.
pub fn history_time(
    (year, month, day, hour, minute): (u16, u16, u16, u16, u16),
    utc: bool,
    lang: Lang,
) -> String {
    let suffix = if utc { " UTC" } else { "" };
    match lang {
        Lang::Ja => format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}{suffix}"),
        Lang::En => format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}{suffix}"),
    }
}

/// The accessible label of a row's revert button (design m3 E.2).
pub fn history_revert_label(when: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{when} の変更を元に戻す"),
        Lang::En => format!("Revert the change of {when}"),
    }
}

pub fn history_empty(lang: Lang) -> String {
    pick(
        lang,
        "まだ履歴はありません。",
        "Nothing has been changed yet.",
    )
}

/// Entries of a newer MKLM (design m2 C.10).
pub fn history_unreadable(count: usize, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "この MKLM では読めない記録が {count} 件あります。MKLM を更新してください。それまで、キーボードの配列は変更できません。"
        ),
        Lang::En if count == 1 => "1 journal entry cannot be read by this MKLM. Update MKLM; until then no keyboard's layout can be changed.".to_string(),
        Lang::En => format!(
            "{count} journal entries cannot be read by this MKLM. Update MKLM; until then no keyboard's layout can be changed."
        ),
    }
}

/// Why no "revert" is offered right now.
pub fn history_blocked(busy: bool, lang: Lang) -> String {
    if busy {
        pick(
            lang,
            "別の MKLM が処理中のため、今は元に戻せません。",
            "Another MKLM process is working; nothing can be reverted now.",
        )
    } else {
        pick(
            lang,
            "途中で止まった操作があるため、今は元に戻せません。先に回復してください。",
            "An interrupted operation needs recovery first; nothing can be reverted now.",
        )
    }
}

pub fn history_folder_failed(lang: Lang) -> String {
    pick(
        lang,
        "復旧用ファイルのフォルダーを開けませんでした。まだ作られていない可能性があります（MKLM が初めて設定を変更するときに作られます）。",
        "The recovery files folder could not be opened. It may not exist yet (MKLM creates it with its first change).",
    )
}

/// An operation's state in the history. "確定" is not a screen word (design m3 D.5): a kept
/// change says what the user did.
pub fn history_kept(lang: Lang) -> String {
    pick(lang, "このままにしました", "Kept")
}

// --- Recovery, undo and revert (B.12) --------------------------------------------------------

/// Which page the recovery screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTitle {
    /// What the journal needs (start-up, the banner).
    Attention,
    /// "確認待ちの変更をすべて元に戻す" (`Request::Undo`).
    Undo,
    /// One operation of the history (`Request::Revert`).
    Revert,
}

pub fn recovery_title(title: RecoveryTitle, lang: Lang) -> String {
    match title {
        RecoveryTitle::Attention => pick(
            lang,
            "確認が必要なことがあります",
            "MKLM needs your attention",
        ),
        RecoveryTitle::Undo => pick(
            lang,
            "確認待ちの変更をすべて元に戻す",
            "Undo every change waiting for you",
        ),
        RecoveryTitle::Revert => pick(lang, "変更を元に戻す", "Revert a change"),
    }
}

/// The primary button of each page.
pub fn recovery_primary(title: RecoveryTitle, lang: Lang) -> String {
    match title {
        RecoveryTitle::Attention => pick(lang, "回復する（おすすめ）", "Recover (recommended)"),
        RecoveryTitle::Undo => pick(lang, "すべて元に戻す", "Undo them all"),
        RecoveryTitle::Revert => pick(lang, "元に戻す", "Revert"),
    }
}

/// "後で" on the attention page, "キャンセル" on the previews.
pub fn recovery_dismiss(title: RecoveryTitle, lang: Lang) -> String {
    match title {
        RecoveryTitle::Attention => pick(lang, "後で", "Later"),
        RecoveryTitle::Undo | RecoveryTitle::Revert => pick(lang, "キャンセル", "Cancel"),
    }
}

/// What "後で" costs (review U9, U15 b).
pub fn recovery_later_note(lang: Lang) -> String {
    pick(
        lang,
        "回復するまで、キーボードの配列は変更できません。",
        "Until you recover, no keyboard's layout can be changed.",
    )
}

pub fn recovery_nothing(title: RecoveryTitle, lang: Lang) -> String {
    match title {
        RecoveryTitle::Attention => pick(
            lang,
            "回復や確認が必要なものはありません。",
            "Nothing needs recovery or a decision.",
        ),
        RecoveryTitle::Undo => pick(
            lang,
            "確認待ちの変更はありません。",
            "No change is waiting for you.",
        ),
        RecoveryTitle::Revert => pick(
            lang,
            "この変更は今は元に戻せません（後の操作が同じ値を変えたか、もう元に戻っています）。",
            "This change cannot be reverted now (a later operation changed the same values, or it is back already).",
        ),
    }
}

/// Where an entry stands, after the operation's name: "（途中で止まりました）".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPhase {
    /// Planned / Written / Restarting whose writer is gone.
    Interrupted,
    /// A countdown whose owner is gone.
    CountdownAbandoned,
    /// RevertPending whose writer is gone.
    RevertInterrupted,
    /// A `PendingReboot` seen from a new boot.
    Restarted,
    /// Waits for keep or revert.
    AwaitingUser,
    WaitingForReboot,
    Conflict,
    /// Another MKLM process works on it.
    Busy,
}

pub fn entry_phase(phase: EntryPhase, lang: Lang) -> String {
    let (ja, en) = match phase {
        EntryPhase::Interrupted => ("途中で止まりました", "stopped halfway"),
        EntryPhase::CountdownAbandoned => (
            "試している途中で MKLM が終了しました",
            "MKLM ended while the new layout was being tried",
        ),
        EntryPhase::RevertInterrupted => ("元に戻す途中で止まりました", "stopped while reverting"),
        EntryPhase::Restarted => ("PC を再起動しました", "the PC has restarted"),
        EntryPhase::AwaitingUser => ("確認待ち", "waiting for you"),
        EntryPhase::WaitingForReboot => ("PC の再起動待ち", "waiting for a PC restart"),
        EntryPhase::Conflict => ("MKLM 以外による変更", "changed outside MKLM"),
        EntryPhase::Busy => ("別の MKLM が処理中", "another MKLM process is working"),
    };
    pick(lang, ja, en)
}

/// "Keychron Receiver を JIS に（途中で止まりました）".
pub fn recovery_operation(what: &str, phase: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{what}（{phase}）"),
        Lang::En => format!("{what} ({phase})"),
    }
}

/// The accessible label of an entry's "このままにする" on the recovery page (design m3 E.2):
/// `operation` is [`recovery_operation`].
pub fn recovery_keep_label(operation: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{operation}をこのままにする"),
        Lang::En => format!("Keep: {operation}"),
    }
}

/// The accessible label of an entry's "元に戻す…" on the recovery page (design m3 E.2).
pub fn recovery_revert_label(operation: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{operation}を元に戻す…"),
        Lang::En => format!("Revert…: {operation}"),
    }
}

/// What recovering will do to an entry (review U9), predicted with `decide_recovery`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome<'a> {
    /// Back to the values before; `switched` is false when the keyboard reset was never reached.
    RollBack {
        before: &'a str,
        switched: bool,
        restart: bool,
    },
    /// The write is complete; keep or revert afterwards (after the restart when `restart`).
    Forward {
        restart: bool,
    },
    /// A restore to before MKLM is written to the end.
    CompleteRestore {
        restart: bool,
    },
    /// Values changed outside MKLM: the conflict page follows.
    Conflict,
    NothingWritten,
    /// A revert that stopped halfway is finished.
    ContinueRevert,
    /// The PC restarted: keep or revert.
    RebootObserved,
    /// A revert that needed the restart is complete.
    RevertApplied,
    /// Nothing for recovery: waits for keep or revert.
    WaitsForUser,
    WaitsForReboot,
    WaitsForConflict,
    /// Another MKLM process works on it: nothing to do now.
    Busy,
    /// No snapshot to predict from: the helper decides when it recovers.
    Unknown,
}

pub fn recovery_outcome(outcome: &RecoveryOutcome<'_>, lang: Lang) -> String {
    match outcome {
        RecoveryOutcome::RollBack {
            before,
            switched,
            restart,
        } => {
            let mut text = match lang {
                Lang::Ja => format!(
                    "→ 回復すると{}（変更前）に戻ります",
                    ja_word(before).trim_end()
                ),
                Lang::En => format!("→ Recovering puts it back to {before} (as before)"),
            };
            if *restart {
                text.push_str(&pick(
                    lang,
                    "（PC の再起動で反映されます）",
                    " (in effect after a PC restart)",
                ));
            }
            if !switched {
                text.push_str(&pick(
                    lang,
                    "。キーボードの動作は変わっていません",
                    "; the keyboard never switched",
                ));
            }
            text
        }
        RecoveryOutcome::Forward { restart: true } => pick(
            lang,
            "→ 書き込みは終わっています。回復の後、PC を再起動すると反映されます。その後、このままにするか元に戻すかを選べます",
            "→ The values are written; they take effect when the PC restarts. Then you choose to keep or revert them",
        ),
        RecoveryOutcome::Forward { restart: false } => pick(
            lang,
            "→ 書き込みは終わっています。回復の後に、このままにするか元に戻すかを選べます",
            "→ The values are written; after the recovery you choose to keep or revert them",
        ),
        RecoveryOutcome::CompleteRestore { restart } => {
            let mut text = pick(
                lang,
                "→ 「MKLM 導入前に戻す」を最後まで書き込みます。その後、このままにするか元に戻すかを選べます",
                "→ The restore to before MKLM is written to the end; then you choose to keep or revert it",
            );
            if *restart {
                text.push_str(&pick(
                    lang,
                    "（PC の再起動で反映されます）",
                    " (in effect after a PC restart)",
                ));
            }
            text
        }
        RecoveryOutcome::Conflict => pick(
            lang,
            "→ MKLM 以外の変更が見つかりました。回復の後で、どうするかを選びます",
            "→ Values were changed outside MKLM; after the recovery you decide what to do",
        ),
        RecoveryOutcome::NothingWritten => pick(
            lang,
            "→ 何も書き込まれていませんでした。記録を閉じるだけです",
            "→ Nothing had been written; the record is only closed",
        ),
        RecoveryOutcome::ContinueRevert => pick(
            lang,
            "→ 元に戻す処理を最後まで行います",
            "→ The revert is finished",
        ),
        RecoveryOutcome::RebootObserved => pick(
            lang,
            "→ PC の再起動が済んでいます。回復の後に、このままにするか元に戻すかを選べます",
            "→ The PC has restarted; after the recovery you choose to keep or revert it",
        ),
        RecoveryOutcome::RevertApplied => pick(
            lang,
            "→ 元に戻した値が反映されました。記録を閉じます",
            "→ The reverted values are in effect; the record is closed",
        ),
        RecoveryOutcome::WaitsForUser => pick(
            lang,
            "このままにするか、元に戻すかを選んでください",
            "Choose to keep or revert it",
        ),
        RecoveryOutcome::WaitsForReboot => pick(
            lang,
            "PC を再起動すると反映されます（シャットダウンではなく再起動）",
            "It takes effect when the PC restarts (Restart, not Shut down)",
        ),
        RecoveryOutcome::WaitsForConflict => pick(
            lang,
            "MKLM 以外による変更をどうするか選んでください",
            "Decide what to do about the values changed outside MKLM",
        ),
        RecoveryOutcome::Busy => pick(
            lang,
            "別の MKLM が処理中です。終わるまで待ってください",
            "Another MKLM process is working on it; wait until it has finished",
        ),
        RecoveryOutcome::Unknown => pick(
            lang,
            "→ 回復のときに、MKLM の管理用プログラム（mklm-helper.exe）が今の値を確かめて決めます",
            "→ MKLM's administrator program (mklm-helper.exe) checks the current values when it recovers",
        ),
    }
}

/// What undoing or reverting an entry does (the undo and revert previews, design m2 D.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoOutcome<'a> {
    /// Recovered first (an interrupted entry).
    RecoveredFirst,
    /// Back to `before`; `restart` when boot-time values are involved; `kept_outside` values
    /// changed outside MKLM are left alone (an entry in conflict).
    Back {
        before: &'a str,
        restart: bool,
        kept_outside: bool,
    },
}

pub fn undo_outcome(outcome: &UndoOutcome<'_>, lang: Lang) -> String {
    match outcome {
        UndoOutcome::RecoveredFirst => pick(
            lang,
            "→ 途中で止まっているため、先に回復します",
            "→ It stopped halfway, so it is recovered first",
        ),
        UndoOutcome::Back {
            before,
            restart,
            kept_outside,
        } => {
            let mut text = match lang {
                Lang::Ja => format!("→ {before}（変更前）に戻します"),
                Lang::En => format!("→ Back to {before} (as before)"),
            };
            if *restart {
                text.push_str(&pick(
                    lang,
                    "（PC の再起動が必要）",
                    " (the PC must restart)",
                ));
            }
            if *kept_outside {
                text.push_str(&pick(
                    lang,
                    "。MKLM 以外が変えた値はそのまま残します",
                    "; values changed outside MKLM are left as they are",
                ));
            }
            text
        }
    }
}

/// A state of an operation in the undo preview (the history's words).
pub fn undo_state(state: OpState, lang: Lang) -> String {
    match state {
        OpState::Confirmed => history_kept(lang),
        other => super::state(other, lang),
    }
}
