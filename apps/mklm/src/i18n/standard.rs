//! Texts of the PC's standard layout (design standard-layout B.12, B.13): the page that changes
//! it, its entry points, and what the other pages say about a standard change. Both languages,
//! typed by what they are about, as in the rest of `i18n`.

use super::{Lang, pick};

pub fn entry_label(lang: Lang) -> String {
    pick(
        lang,
        "PC の標準配列を変更…",
        "Change the PC's standard layout…",
    )
}

pub fn title(lang: Lang) -> String {
    pick(lang, "PC の標準配列", "The PC's standard layout")
}

pub fn introduction(os: Option<&mklm_core::OsInfo>, lang: Lang) -> String {
    let pc = match os.filter(|os| os.remote_session) {
        Some(os) => match os.computer_name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => pick(
                lang,
                &format!("接続先の PC（{name}）"),
                &format!("the PC you connect to ({name})"),
            ),
            None => pick(lang, "接続先の PC", "the PC you connect to"),
        },
        None => pick(lang, "この PC", "this PC"),
    };
    pick(
        lang,
        &format!(
            "変更する対象: {pc}\nPC の標準配列は、配列を割り当てていないキーボードが使う配列です。標準に従うキーボードには先に今の配列を割り当てるので、再起動の後も配列は変わりません。切り替えたいキーボードだけ［今の配列のままにする］を外してください。"
        ),
        &format!(
            "Changing: {pc}\nThe PC's standard layout is used by keyboards without a layout of their own. MKLM first assigns those keyboards their current layout so they keep it after the restart. Clear Keep the current layout only for keyboards you want to switch."
        ),
    )
}

pub fn restart_note(lang: Lang) -> String {
    pick(
        lang,
        "変更は、この PC のすべてのユーザーに適用されます。変更後はすぐに PC を再起動してください（シャットダウンではなく再起動）。再起動するまで、MKLM でほかの変更はできません。再起動の前にサインアウト、ユーザーの切り替え、シャットダウンをすると、パスワードを打つ配列が早めに変わることがあります（未確認）。",
        "This affects every user of this PC. Restart the PC immediately after the change (Restart, not Shut down). MKLM cannot make other changes until then. Signing out, switching users or shutting down before the restart may change password typing early (not verified).",
    )
}

pub fn remote_note(lang: Lang) -> String {
    pick(
        lang,
        "リモート デスクトップの入力は、セッションで 1 つのキー配列を使います。この操作で接続元のキーボードごとに配列を分けることはできません。接続先の標準配列と接続元の報告がどう影響し、いつ反映されるかは未確認です。セッションで Shift+2 を打って確かめてください（日本語の入力方式で、@ なら US、\" なら JIS）。接続先の物理キーボードの確認にはなりません。",
        "Remote Desktop uses one key table per session. This operation cannot give each keyboard of the connecting PC its own remote layout. How the destination's standard and the client's report affect it, and when changes apply, are unverified. Check Shift+2 in the session with a Japanese input method (@ means US, \" means JIS). This does not check the destination's physical keyboards.",
    )
}

pub fn choice_label(lang: Lang) -> String {
    pick(
        lang,
        "再起動の後の PC の標準配列",
        "The PC's standard layout after the restart",
    )
}

pub fn choice(layout: mklm_core::Layout, current: bool, lang: Lang) -> String {
    let text = match layout {
        mklm_core::Layout::Jis => pick(
            lang,
            "JIS（日本語 106/109 キー）",
            "JIS (Japanese 106/109 keys)",
        ),
        mklm_core::Layout::Us => pick(lang, "US（英語 101/102 キー）", "US (English 101/102 keys)"),
    };
    format!(
        "{text}{}",
        if current {
            pick(lang, "（現在）", " (current)")
        } else {
            String::new()
        }
    )
}

pub fn fixed_note(lang: Lang) -> String {
    pick(
        lang,
        "固定モードでは、すべてのキーボードが同じ配列を使います。キーボード一覧の［変更…］からキーボードごとモードに移行するときに、標準配列を選べます。",
        "In fixed mode all keyboards use one layout. Choose the standard when moving to per-keyboard mode through a keyboard's Change… button.",
    )
}

/// `UnknownStandard` (design standard-layout B.13 "拒否"): the stored values go to the
/// technical details.
pub fn unknown_standard(lang: Lang) -> String {
    pick(
        lang,
        "標準配列の値が、MKLM の知っている形ではありません。今どの配列で打っているかが分からないため、変更できません。キーボードごとの配列（各行の［変更…］）は今までどおり変えられます。",
        "The standard layout's values are not in a form MKLM knows. MKLM cannot tell what the keyboards type now, so it cannot change it. Layouts per keyboard (each row's \"Change…\") can still be changed.",
    )
}
