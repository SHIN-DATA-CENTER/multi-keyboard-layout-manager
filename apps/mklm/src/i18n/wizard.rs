//! Texts of the first-run wizard (plan 3.1; design m3 B.1; WP-U2): the four steps, the input
//! method warnings, the problems of stored values and the summary with its buttons. Like the rest
//! of `i18n`, both languages are here, typed by what they are about; `vm::wizard` only assembles
//! them.
//!
//! Screen words (design m3 D.5, review U5): input methods by language name, never a KLID; value
//! names and registry paths only in the technical details ([`foreign_steps`]); "MKLM の管理用
//! プログラム" for the helper.

use mklm_core::{InputWarning, KeyboardAnomaly, Layout, LayoutTable};

use super::{Lang, klid_name, pick};
use crate::detect::Step;
use crate::vm::wizard::WizardStep;

/// "手順 3 / 4: キーボードの配列" (review U16 e: the position in words, not by colour).
pub fn step_heading(step: WizardStep, lang: Lang) -> String {
    let (ja, en) = match step {
        WizardStep::Welcome => ("ようこそ", "Welcome"),
        WizardStep::InputMethods => ("入力方式", "Input methods"),
        WizardStep::Keyboards => ("キーボードの配列", "Keyboard layouts"),
        WizardStep::Summary => ("まとめ", "Summary"),
    };
    let (number, of) = step.number();
    match lang {
        Lang::Ja => format!("手順 {number} / {of}: {ja}"),
        Lang::En => format!("Step {number} of {of}: {en}"),
    }
}

// --- 1. Welcome ------------------------------------------------------------------------------

pub fn welcome_body(lang: Lang) -> String {
    pick(
        lang,
        "MKLM は、つないでいるキーボードごとに、JIS 配列か US 配列かを Windows に設定します。このセットアップでは、入力方式を確かめ、キーボードごとに実物の配列を選び、必要な変更をまとめて行います。",
        "MKLM tells Windows, keyboard by keyboard, whether it is a JIS or a US keyboard. This setup checks your input methods, lets you choose each keyboard's physical layout, and then makes the changes that are needed.",
    )
}

/// What the user should know before anything is changed (design m3 B.1 step 1).
pub fn welcome_points(lang: Lang) -> Vec<String> {
    [
        (
            "この設定は、この PC のすべてのユーザーに適用されます。",
            "The settings apply to every user of this PC.",
        ),
        (
            "キーボードの設定を変えるときは、そのたびに Windows が管理者の許可を求めます（MKLM はまだコード署名をしていないため、発行元は「不明」と表示されます）。",
            "Each time a keyboard setting changes, Windows asks for administrator permission (MKLM is not code-signed yet, so the publisher shows as \"Unknown\").",
        ),
        (
            "MKLM は、初めて変える前の値を記録します。後から「MKLM 導入前に戻す」で元に戻せます。",
            "MKLM records the values before its first change; \"Back to before MKLM\" puts them back later.",
        ),
        (
            "何も変えずに終えることもできます。そのときは［後でセットアップする］を押してください。設定画面から、もう一度始められます。",
            "You can also finish without changing anything: choose \"Set up later\". The setup can be started again from Settings.",
        ),
    ]
    .into_iter()
    .map(|(ja, en)| pick(lang, ja, en))
    // The daily update check (design m5b E.2).
    .chain(std::iter::once(super::update::wizard_daily_check(lang)))
    .collect()
}

// --- 2. Input methods ------------------------------------------------------------------------

pub fn input_body(lang: Lang) -> String {
    pick(
        lang,
        "キーボードごとの配列は、日本語の入力方式（Microsoft IME）を使っている間だけ有効です。英語 (US) などほかの入力方式に切り替えると、すべてのキーボードがその入力方式の配列になります。",
        "Per-keyboard layouts apply only while the Japanese input method (Microsoft IME) is in use. With another input method, such as English (US), every keyboard types that method's layout.",
    )
}

/// The input methods of a Preload list by language name, in order, without repeats.
fn method_names(klids: &[String], lang: Lang) -> String {
    let mut names: Vec<String> = Vec::new();
    for klid in klids {
        let name = klid_name(klid, lang);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    super::name_list(&names, lang)
}

/// "あなたの入力方式: 日本語、英語 (US)".
pub fn user_methods(klids: &[String], lang: Lang) -> String {
    let names = if klids.is_empty() {
        pick(lang, "読み取れませんでした", "could not be read")
    } else {
        method_names(klids, lang)
    };
    match lang {
        Lang::Ja => format!("あなたの入力方式: {names}"),
        Lang::En => format!("Your input methods: {names}"),
    }
}

/// "サインイン画面の入力方式: 日本語" (`HKU\.DEFAULT`).
pub fn sign_in_methods(klids: &[String], lang: Lang) -> String {
    let names = if klids.is_empty() {
        pick(lang, "読み取れませんでした", "could not be read")
    } else {
        method_names(klids, lang)
    };
    match lang {
        Lang::Ja => format!("サインイン画面の入力方式: {names}"),
        Lang::En => format!("Input methods of the sign-in screen: {names}"),
    }
}

/// One `mklm_core::input_warnings` warning as a sentence (design m3 B.1 step 2, plan 3.1).
pub fn input_warning(warning: &InputWarning, lang: Lang) -> String {
    match warning {
        InputWarning::NonJapaneseLayouts { preload, .. } => {
            let names = method_names(preload, lang);
            match (lang, names.is_empty()) {
                (Lang::Ja, false) => format!(
                    "日本語以外の入力方式があります（{names}）。英語を打つときも英語 (US) には切り替えず、日本語 IME をオフにしてください（Alt+` か「半角/全角」）。英語 (US) に切り替えると、すべてのキーボードが US 配列になります。"
                ),
                (Lang::Ja, true) => "日本語以外の入力方式があります。英語を打つときも英語 (US) には切り替えず、日本語 IME をオフにしてください（Alt+` か「半角/全角」）。英語 (US) に切り替えると、すべてのキーボードが US 配列になります。".to_string(),
                (Lang::En, false) => format!(
                    "There are input methods other than Japanese ({names}). To type English, turn the Japanese IME off (Alt+` or 半角/全角) instead of switching to English (US): with English (US) every keyboard types US."
                ),
                (Lang::En, true) => "There are input methods other than Japanese. To type English, turn the Japanese IME off (Alt+` or 半角/全角) instead of switching to English (US): with English (US) every keyboard types US.".to_string(),
            }
        }
        InputWarning::UserDefaultNotJapanese { klid } => {
            let name = klid_name(klid, lang);
            match lang {
                Lang::Ja => format!(
                    "既定の入力方式が {name} です。サインインのたびに {name} で始まるため、日本語に切り替えるまでキーボードごとの配列が効きません。［言語の設定］で日本語を一番上にしてください。"
                ),
                Lang::En => format!(
                    "Your default input method is {name}: every sign-in starts with it, and per-keyboard layouts do not apply until you switch to Japanese. Move Japanese to the top in Language settings."
                ),
            }
        }
        InputWarning::SignInNotJapanese { klid } => {
            let name = klid.as_deref().map_or_else(
                || pick(lang, "不明", "unknown"),
                |klid| klid_name(klid, lang),
            );
            match lang {
                Lang::Ja => format!(
                    "サインイン画面の入力方式が {name} です。サインイン画面では、すべてのキーボードが US 配列になります（パスワードの記号の位置に注意してください）。"
                ),
                Lang::En => format!(
                    "The sign-in screen's input method is {name}: there every keyboard types US (mind the symbols in your password)."
                ),
            }
        }
        InputWarning::NoJapaneseLayout => pick(
            lang,
            "日本語の入力方式（Microsoft IME）がありません。キーボードごとの配列は、日本語 IME を使っている間だけ有効です。［言語の設定］で日本語を追加してください。",
            "There is no Japanese input method (Microsoft IME). Per-keyboard layouts apply only with the Japanese IME: add Japanese in Language settings.",
        ),
    }
}

/// How to give the sign-in screen the user's input methods (plan 3.1: intl.cpl → 管理 → 設定の
/// コピー).
pub fn sign_in_copy_steps(lang: Lang) -> String {
    pick(
        lang,
        "サインイン画面の入力方式を合わせるには: ［地域（設定のコピー）］を開き、［管理］タブの［設定のコピー］で「ようこそ画面とシステム アカウント」にチェックを付けて［OK］を押します。",
        "To give the sign-in screen your input methods: open \"Region (copy settings)\", and on the Administrative tab choose \"Copy settings\", check \"Welcome screen and system accounts\" and confirm.",
    )
}

pub fn input_fine(lang: Lang) -> String {
    pick(
        lang,
        "入力方式に問題はありません。",
        "The input methods are fine.",
    )
}

// --- 3. Keyboards ----------------------------------------------------------------------------

pub fn keyboards_body(lang: Lang) -> String {
    pick(
        lang,
        "つないでいるキーボードごとに、実物の配列を選んでください（はじめは、いま設定されている配列が選ばれています）。",
        "Choose each connected keyboard's physical layout (at first, the layout it is set to now is chosen).",
    )
}

pub fn keyboards_reading(lang: Lang) -> String {
    pick(
        lang,
        "キーボードを確かめています…",
        "Looking for the keyboards…",
    )
}

pub fn keyboards_none(lang: Lang) -> String {
    pick(
        lang,
        "つないでいるキーボードが見つかりません。キーボードをつないでから［次へ］を押すか、後でセットアップしてください。",
        "No connected keyboard was found. Connect one and choose Next, or set up later.",
    )
}

/// "JIS として動作中": how a row types now (neutral; the main screen says why, design m3 B.2).
pub fn types_now(table: Option<&LayoutTable>, lang: Lang) -> String {
    match table {
        Some(table) => {
            let name = super::table(table, lang);
            match lang {
                Lang::Ja => format!("{name} として動作中"),
                Lang::En => format!("types {name}"),
            }
        }
        None => super::behavior_unknown(lang),
    }
}

/// The in-place detection of step 3 (design m3 B.3): the first answer key fixes the keyboard.
pub fn detect_instruction(step: Step, lang: Lang) -> String {
    match step {
        Step::LeftOfBackspace => pick(
            lang,
            "わからないときは、下の欄をクリックしてから、調べたいキーボードで Backspace の左のキーを押してください。",
            "Not sure? Click the field below, then press the key left of Backspace on the keyboard in question.",
        ),
        Step::LeftOfRightShift => pick(
            lang,
            "次に、同じキーボードで右の Shift の左のキーを押してください。",
            "Next, press the key left of the right Shift on the same keyboard.",
        ),
        Step::Done => String::new(),
    }
}

/// A problem of a keyboard's stored values, in words (design m3 B.1 step 3; the value names and
/// numbers are in the technical details). `deletable`: the row offers "削除する".
pub fn problem(anomaly: &KeyboardAnomaly, deletable: bool, lang: Lang) -> String {
    match anomaly {
        KeyboardAnomaly::IncompletePair { .. } => pick(
            lang,
            "⚠ 設定に問題: 2 つで 1 組の値のうち、片方だけが保存されています。配列を選び直すと、両方を書き直します。",
            "⚠ Setting problem: only one value of a pair is stored. Choosing a layout for it writes both again.",
        ),
        KeyboardAnomaly::ForeignValueNames { .. } if deletable => pick(
            lang,
            "⚠ 設定に問題: このキーボードのドライバーが読まない値（別の種類のキーボード用の値）があります。害はありませんが、削除することもできます。",
            "⚠ Setting problem: there are values this keyboard's driver does not read (values for another kind of keyboard). They do no harm; you can also delete them.",
        ),
        KeyboardAnomaly::ForeignValueNames { .. } => pick(
            lang,
            "⚠ 設定に問題: このキーボードのドライバーが読まない値があります。害はありません。MKLM は変更しません。",
            "⚠ Setting problem: there are values this keyboard's driver does not read. They do no harm; MKLM leaves them.",
        ),
        KeyboardAnomaly::ValuesOnUnsupportedDriver { .. } => pick(
            lang,
            "⚠ 設定に問題: MKLM が扱わないドライバーのキーボードに値があります。MKLM は変更しません。",
            "⚠ Setting problem: a keyboard with a driver MKLM does not handle has values. MKLM leaves them.",
        ),
        KeyboardAnomaly::ValuesOnNonKeyboard { .. } => foreign_values(lang),
        KeyboardAnomaly::UnverifiedType { .. } | KeyboardAnomaly::UnexpectedType { .. } => pick(
            lang,
            "⚠ 設定に問題: MKLM が動作を確かめていない種類の値が保存されています。JIS か US を選び直すと、確かめられた値に書き直します。",
            "⚠ Setting problem: a kind of value MKLM has not verified is stored. Choosing JIS or US for it writes verified values instead.",
        ),
        KeyboardAnomaly::IgnoredInFixedMode { keyboard_type } => {
            let table = keyboard_type.per_keyboard_table();
            let name = table
                .as_ref()
                .map(|table| super::table(table, lang))
                .unwrap_or_default();
            match lang {
                Lang::Ja => format!(
                    "⚠ 設定に問題: 固定モードのため、このキーボードに保存されている {name} 配列の値は今は使われていません。キーボードごとモードへ移行すると使われます（下の選択が優先されます）。"
                ),
                Lang::En => format!(
                    "⚠ Setting problem: in fixed mode the {name} values stored for this keyboard are not used. After the switch to per-keyboard mode they are (the choice below takes precedence)."
                ),
            }
        }
    }
}

/// Values on a non-keyboard collection of the device (design m3 B.1, J.9): MKLM never writes
/// them (plan 1.5); the steps to remove them by hand are in the technical details.
pub fn foreign_values(lang: Lang) -> String {
    pick(
        lang,
        "⚠ 設定に問題: このデバイスのキーボード以外の部分（マウスなど）に、どのドライバーも読まない値があります。害はありません。MKLM は変更しないので、消したいときは「技術的な詳細」の手順で手動で削除してください。",
        "⚠ Setting problem: a part of this device that is not a keyboard (a mouse, say) has values no driver reads. They do no harm. MKLM does not change them; to remove them, follow the steps in the technical details.",
    )
}

/// The manual steps for values on a non-keyboard collection (the technical details; design m3 B.1,
/// A.5, J.9): check the collection in Device Manager, delete each value from an administrator's
/// command prompt, then replug the device. One line each.
pub fn foreign_steps(instance_id: &str, names: &[String], lang: Lang) -> Vec<String> {
    let mut lines = vec![match lang {
        Lang::Ja => format!(
            "キーボード以外の部分: {instance_id}。デバイス マネージャーで［表示］→［デバイス (接続別)］を選ぶと、このキーボードと同じデバイスの下にあることを確かめられます。"
        ),
        Lang::En => format!(
            "Non-keyboard part: {instance_id}. In Device Manager, View → Devices by connection shows it under the same device as this keyboard."
        ),
    }];
    lines.push(pick(
        lang,
        "削除するには、管理者として実行したコマンド プロンプトで次を実行します（値ごとに 1 行）:",
        "To delete them, run in a command prompt started as administrator (one line per value):",
    ));
    for name in names {
        lines.push(format!(
            "reg delete \"HKLM\\SYSTEM\\CurrentControlSet\\Enum\\{instance_id}\\Device Parameters\" /v {name} /f"
        ));
    }
    lines.push(pick(
        lang,
        "その後、デバイスを抜き差しするか、PC を再起動してください。",
        "Then unplug and replug the device, or restart the PC.",
    ));
    lines
}

/// The technical details of a keyboard's own problem values: "…\Device Parameters:
/// OverrideKeyboardType, OverrideKeyboardSubtype".
pub fn problem_values(instance_id: &str, names: &[String], lang: Lang) -> String {
    let names = names.join(", ");
    match lang {
        Lang::Ja => format!("{instance_id} の値: {names}"),
        Lang::En => format!("Values of {instance_id}: {names}"),
    }
}

// --- 4. Summary ------------------------------------------------------------------------------

/// Where the summary stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryState {
    /// Every choice is what the keyboards are set to already.
    NothingToDo,
    /// The wizard made its changes; nothing is left.
    Done,
    /// Changes follow.
    Changes,
    /// Changes remain, but none can be made now (the journal blocks new ones).
    Blocked,
}

pub fn summary_body(state: SummaryState, lang: Lang) -> String {
    match state {
        SummaryState::NothingToDo => pick(
            lang,
            "変更は不要です。どのキーボードも、選んだ配列のとおりに設定されています。",
            "No change is needed: every keyboard is set to the layout you chose.",
        ),
        SummaryState::Done => pick(
            lang,
            "設定が終わりました。",
            "The setup has made its changes.",
        ),
        SummaryState::Changes => pick(
            lang,
            "次の変更を、上から順に行います。変更のたびに、Windows が管理者の許可を求めます。",
            "These changes are made one after the other. Windows asks for administrator permission for each.",
        ),
        SummaryState::Blocked => pick(
            lang,
            "次の変更が残っていますが、今は行えません。",
            "These changes are left, but they cannot be made now.",
        ),
    }
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Jis => "JIS",
        Layout::Us => "US",
    }
}

/// The migration of fixed mode (design m3 B.1 step 4): one restart.
pub fn migration_line(standard: Layout, lang: Lang) -> String {
    let standard = layout_name(standard);
    match lang {
        Lang::Ja => format!(
            "キーボードごとモードへ移行します（PC の標準配列 {standard}）。PC の再起動が 1 回必要です。"
        ),
        Lang::En => format!(
            "Switch to per-keyboard mode (the PC's standard layout {standard}). One PC restart is needed."
        ),
    }
}

/// An assignment the migration writes: "Keychron Receiver を US に".
pub fn assignment_line(name: &str, layout: Layout, lang: Lang) -> String {
    let layout = layout_name(layout);
    match lang {
        Lang::Ja => format!("{name} を {layout} に（移行と同時に書きます）"),
        Lang::En => format!("{name} to {layout} (written with the switch)"),
    }
}

/// One ordinary change of per-keyboard mode: "Keychron Receiver を US に".
pub fn change_line(name: &str, layout: Layout, lang: Lang) -> String {
    let layout = layout_name(layout);
    match lang {
        Lang::Ja => format!("{name} を {layout} に"),
        Lang::En => format!("{name} to {layout}"),
    }
}

/// "削除する" of step 3 (`Request::CleanupValues`).
pub fn cleanup_line(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "{name}: ドライバーが読まない値を削除（削除の後、このままにするか元に戻すかを選びます）"
        ),
        Lang::En => format!(
            "{name}: delete the values its driver does not read (then choose to keep or undo it)"
        ),
    }
}

/// Per-keyboard mode: the changes go one by one through the change page (design m3 B.1).
pub fn one_by_one_note(lang: Lang) -> String {
    pick(
        lang,
        "キーボードごとに順に変更します（同時に行える変更は 1 つです）。それぞれの画面で切り替え方を確かめて［変更する］を押してください。",
        "The keyboards are changed one at a time (one change can be made at a time). On each page, check how the change takes effect and choose Change.",
    )
}

/// Per-keyboard mode: the standard layout is not chosen here (design m3 B.1: `Migrate` would
/// refuse it with `NotFixedMode`).
pub fn standard_unchanged(table: &LayoutTable, lang: Lang) -> String {
    let name = super::table(table, lang);
    match lang {
        Lang::Ja => format!(
            "PC の標準配列: {name}（キーボードごとモードでは、ここでは変えません。値のないキーボードがこの配列になります）"
        ),
        Lang::En => format!(
            "The PC's standard layout: {name} (not changed here in per-keyboard mode; keyboards without values type it)"
        ),
    }
}

/// The write order of the migration (plan 1.3; design m3 B.1 step 4).
pub fn write_order(lang: Lang) -> String {
    pick(
        lang,
        "書き込みの順序: まず内蔵（PS/2）キーボードに今の配列を記録し、次に各キーボードの割り当て、最後に PC 全体の値を書きます。途中で止まっても、内蔵キーボードの配列は変わりません。MKLM が次に起動したときに回復を案内します。",
        "The write order: first the built-in (PS/2) keyboards keep their layout in their own values, then the assignments, then the PC-wide values. If it stops halfway, the built-in keyboard's layout does not change; MKLM offers the recovery at its next start.",
    )
}

/// Plan 1.3: the Settings app's own option makes the built-in keyboard US when chosen first.
pub fn connected_layout_warning(lang: Lang) -> String {
    pick(
        lang,
        "Windows の設定アプリの「接続済みキーボード レイアウトを使用する」は、この移行の前には選ばないでください。先に選ぶと、内蔵キーボードが US 配列になります。",
        "Do not choose \"Use connected keyboard layout\" in the Windows Settings app before this switch: chosen first, it makes the built-in keyboard US.",
    )
}

/// The wizard's cleanup waits for keep or revert (`AwaitingConfirm`, design m3 A.5).
pub fn awaiting_cleanup(name: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "{name} の値を削除しました。キーボードの動作は変わりません。このままにするか、元に戻すかを選んでください。決めるまで、ほかのキーボードの配列も変更できません。"
        ),
        Lang::En => format!(
            "The values of {name} were deleted; the keyboard types as before. Keep this or undo it. Until you decide, no other keyboard can be changed."
        ),
    }
}

/// What is left when new changes are blocked (design m3 B.1: one change at a time).
pub fn blocked_rest(reason: &str, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!("{reason}。残りの変更は、後でメイン画面の［変更…］から行えます。"),
        Lang::En => {
            format!("{reason}. The rest can be changed later with \"Change…\" on the main screen.")
        }
    }
}

/// The primary button of the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryButton<'a> {
    Finish,
    /// Delete the unread values of a keyboard.
    Cleanup,
    /// Keep the wizard's cleanup.
    KeepCleanup,
    /// The migration (`vm::change`'s button text).
    Migrate,
    /// Open the change page of this keyboard.
    OpenChange(&'a str),
}

/// The primary button's text; `prompt`: a UAC prompt follows (not elevated).
pub fn summary_button(button: SummaryButton<'_>, prompt: bool, lang: Lang) -> String {
    let asks = |ja: &str, en: &str| match (lang, prompt) {
        (Lang::Ja, true) => format!("{ja}（次に Windows の確認が出ます）"),
        (Lang::Ja, false) => ja.to_string(),
        (Lang::En, true) => format!("{en} (Windows asks next)"),
        (Lang::En, false) => en.to_string(),
    };
    match button {
        SummaryButton::Finish => pick(lang, "完了", "Finish"),
        SummaryButton::Cleanup => asks("値を削除する", "Delete the values"),
        SummaryButton::KeepCleanup => asks("このままにする", "Keep"),
        SummaryButton::Migrate => super::apply_button(false, prompt, lang),
        SummaryButton::OpenChange(name) => match lang {
            Lang::Ja => format!("{name} の変更へ進む…"),
            Lang::En => format!("Continue with {name}…"),
        },
    }
}

/// The second button next to "このままにする" of the wizard's cleanup.
pub fn revert_cleanup(lang: Lang) -> String {
    pick(lang, "元に戻す", "Undo")
}
