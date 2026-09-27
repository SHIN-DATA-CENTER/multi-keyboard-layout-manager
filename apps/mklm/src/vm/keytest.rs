//! The IME-free key test (plan 1.4, 3.3; design m3 B.6, B.9): what the FocusScope received, and
//! the Shift+2 verdict ("@" = US, "\"" = JIS) judged against what the change should have done
//! (review U2). The keyboard comes from the last Raw Input key press (winit `DeviceEvent::Key`
//! arrives before the window's key event, M0 #8). The typed text is kept in memory only and
//! never logged (plan 2.2).
//!
//! The verdict is only as good as its inputs, so it says when it cannot judge:
//! - the active input method is not the Japanese layout (with English (US) active every keyboard
//!   types US, so "@" proves nothing): a warning to switch with Win+Space;
//! - the key came from a keyboard the change was not about: which keyboard to use instead;
//! - with an expected layout: "✓ 期待どおり JIS です" (success) or "⚠ JIS になるはずが US です"
//!   (danger, "元に戻す" recommended); without one, a neutral statement.

use mklm_core::LayoutTable;

use super::Tone;
use crate::i18n::Lang;

/// The scan code of the "2" key (set 1).
pub const SCANCODE_DIGIT2: u32 = 0x03;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyTest {
    pub prompt: String,
    pub last_text: String,
    pub verdict: String,
    pub verdict_tone: Tone,
    pub device: String,
}

/// What the test is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected<'a> {
    /// The instance IDs of the keyboard(s) the change is about.
    pub targets: &'a [String],
    /// The name to tell the user ("Keychron Receiver で押してください").
    pub name: &'a str,
    /// The layout the change should give.
    pub table: &'a LayoutTable,
}

/// Where a key press came from and what else the verdict depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyContext<'a> {
    /// The last Raw Input key press: instance ID and scan code.
    pub source: Option<(&'a str, u32)>,
    /// The source keyboard's display name.
    pub source_name: Option<&'a str>,
    /// The UI thread's active input language (HKL, low 32 bits).
    pub active_hkl: u32,
    pub expected: Option<Expected<'a>>,
}

/// The prompt before any key.
pub fn prompt(lang: Lang) -> KeyTest {
    KeyTest {
        prompt: match lang {
            Lang::Ja => "キーを押してください（Shift+2 で @ なら US、\" なら JIS）".into(),
            Lang::En => "Press a key (Shift+2: @ means US, \" means JIS)".into(),
        },
        last_text: "—".into(),
        ..KeyTest::default()
    }
}

/// A human-readable form of a Slint key text: `None` for modifiers alone; named keys use private
/// code points (prototype `describe_key_text`).
pub fn describe_key_text(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return Some(text.to_owned());
    }
    match c {
        '\u{10}'..='\u{18}' => None,
        ' ' => Some("Space".to_owned()),
        c if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) => {
            Some(format!("U+{:04X}", c as u32))
        }
        c => Some(c.to_string()),
    }
}

fn table_name(table: &LayoutTable) -> &'static str {
    match table {
        LayoutTable::Jis => "JIS",
        LayoutTable::Us => "US",
        LayoutTable::Other(_) => "?",
    }
}

/// The verdict for one Shift+2 press: `(text, tone)`.
fn verdict(text: &str, context: &KeyContext<'_>, lang: Lang) -> (String, Tone) {
    let pick = |ja: String, en: String| match lang {
        Lang::Ja => ja,
        Lang::En => en,
    };
    if !mklm_core::hkl_has_japanese_layout(context.active_hkl) {
        return (
            pick(
                "入力方式が日本語ではないため判定できません。Win+Space で日本語に切り替えてください".into(),
                "Cannot tell: the input method is not Japanese. Switch to Japanese with Win+Space".into(),
            ),
            Tone::Warning,
        );
    }
    if let (Some(expected), Some((source, _))) = (&context.expected, context.source)
        && !expected
            .targets
            .iter()
            .any(|id| id.eq_ignore_ascii_case(source))
    {
        let from = context.source_name.unwrap_or("?");
        return (
            pick(
                format!(
                    "このキーは {from} から送られました。{} で押してください",
                    expected.name
                ),
                format!("This key came from {from}. Press it on {}", expected.name),
            ),
            Tone::Info,
        );
    }
    let typed = match text {
        "@" => LayoutTable::Us,
        "\"" => LayoutTable::Jis,
        _ => {
            return (
                pick(
                    "Shift+2 → 想定外の文字です".into(),
                    "Shift+2 → an unexpected character".into(),
                ),
                Tone::Warning,
            );
        }
    };
    let got = table_name(&typed);
    match &context.expected {
        Some(expected) if *expected.table == typed => (
            pick(
                format!("Shift+2 → {text} : ✓ 期待どおり {got} です"),
                format!("Shift+2 → {text} : ✓ {got}, as expected"),
            ),
            Tone::Success,
        ),
        Some(expected) => {
            let want = table_name(expected.table);
            (
                pick(
                    format!(
                        "Shift+2 → {text} : ⚠ {want} になるはずが {got} です。『元に戻す』をおすすめします"
                    ),
                    format!(
                        "Shift+2 → {text} : ⚠ it should type {want} but types {got}. Revert is recommended"
                    ),
                ),
                Tone::Danger,
            )
        }
        None => (
            pick(
                format!("Shift+2 → {text} : {got} 配列として動作しています"),
                format!("Shift+2 → {text} : it types {got}"),
            ),
            Tone::Neutral,
        ),
    }
}

/// One key press in the test (`text` and `shift` from the FocusScope). `None` when the press
/// shows nothing (a modifier alone).
pub fn key_pressed(
    text: &str,
    shift: bool,
    context: &KeyContext<'_>,
    lang: Lang,
) -> Option<KeyTest> {
    let shown = describe_key_text(text)?;
    let shift_2 = shift && context.source.map(|(_, code)| code) == Some(SCANCODE_DIGIT2);
    let (verdict, tone) = if shift_2 {
        verdict(text, context, lang)
    } else {
        (String::new(), Tone::Neutral)
    };
    let mut test = prompt(lang);
    test.last_text = shown;
    test.verdict = verdict;
    test.verdict_tone = tone;
    test.device = match (context.source_name, lang) {
        (Some(name), Lang::Ja) => format!("このキーを送ったキーボード: {name}"),
        (Some(name), Lang::En) => format!("Sent by: {name}"),
        (None, _) => String::new(),
    };
    Some(test)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
    const BUILT_IN: &str = r"ACPI\FUJ0309\4&320DB4C2&0";
    const JAPANESE: u32 = 0x0411_0411;

    fn context<'a>(
        source: &'a str,
        name: &'a str,
        hkl: u32,
        expected: Option<Expected<'a>>,
    ) -> KeyContext<'a> {
        KeyContext {
            source: Some((source, SCANCODE_DIGIT2)),
            source_name: Some(name),
            active_hkl: hkl,
            expected,
        }
    }

    #[test]
    fn shift_2_against_the_expected_layout() {
        let targets = vec![KEYCHRON.to_string()];
        let jis = Expected {
            targets: &targets,
            name: "Keychron Receiver",
            table: &LayoutTable::Jis,
        };
        let ok = key_pressed(
            "\"",
            true,
            &context(KEYCHRON, "Keychron Receiver", JAPANESE, Some(jis)),
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(ok.verdict, "Shift+2 → \" : ✓ 期待どおり JIS です");
        assert_eq!(ok.verdict_tone, Tone::Success);
        assert_eq!(ok.device, "このキーを送ったキーボード: Keychron Receiver");
        // The change did not take: danger, not a green "types US" (review U2 a).
        let wrong = key_pressed(
            "@",
            true,
            &context(KEYCHRON, "Keychron Receiver", JAPANESE, Some(jis)),
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(wrong.verdict_tone, Tone::Danger);
        assert!(
            wrong.verdict.contains("JIS になるはずが US"),
            "{}",
            wrong.verdict
        );
        // The built-in keyboard typed (b): no verdict, which keyboard to use.
        let other = key_pressed(
            "\"",
            true,
            &context(BUILT_IN, "内蔵キーボード", JAPANESE, Some(jis)),
            Lang::Ja,
        )
        .unwrap();
        assert_eq!(
            (other.verdict.as_str(), other.verdict_tone),
            (
                "このキーは 内蔵キーボード から送られました。Keychron Receiver で押してください",
                Tone::Info
            )
        );
        // English (US) active (c): cannot tell.
        let english = key_pressed(
            "@",
            true,
            &context(KEYCHRON, "Keychron Receiver", 0x0409_0409, Some(jis)),
            Lang::En,
        )
        .unwrap();
        assert_eq!(english.verdict_tone, Tone::Warning);
        // No expectation: neutral.
        let neutral = key_pressed(
            "@",
            true,
            &context(KEYCHRON, "Keychron Receiver", JAPANESE, None),
            Lang::En,
        )
        .unwrap();
        assert_eq!(
            (neutral.verdict.as_str(), neutral.verdict_tone),
            ("Shift+2 → @ : it types US", Tone::Neutral)
        );
    }

    #[test]
    fn other_keys() {
        let none = KeyContext::default();
        assert_eq!(key_pressed("\u{10}", true, &none, Lang::Ja), None);
        let letter = KeyContext {
            source: Some((KEYCHRON, 0x1E)),
            active_hkl: JAPANESE,
            ..KeyContext::default()
        };
        let other = key_pressed("a", false, &letter, Lang::Ja).unwrap();
        assert_eq!(
            (other.last_text.as_str(), other.verdict.as_str()),
            ("a", "")
        );
    }
}
