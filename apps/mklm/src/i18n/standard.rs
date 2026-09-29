//! Texts of the PC's standard layout (design standard-layout B.12, B.13): the page that changes
//! it, its entry points, and what the other pages say about a standard change. Both languages,
//! typed by what they are about, as in the rest of `i18n`.

use super::{Lang, pick};

/// `UnknownStandard` (design standard-layout B.13 "拒否"): the stored values go to the
/// technical details.
pub fn unknown_standard(lang: Lang) -> String {
    pick(
        lang,
        "標準配列の値が、MKLM の知っている形ではありません。今どの配列で打っているかが分からないため、変更できません。キーボードごとの配列（各行の［変更…］）は今までどおり変えられます。",
        "The standard layout's values are not in a form MKLM knows. MKLM cannot tell what the keyboards type now, so it cannot change it. Layouts per keyboard (each row's \"Change…\") can still be changed.",
    )
}
