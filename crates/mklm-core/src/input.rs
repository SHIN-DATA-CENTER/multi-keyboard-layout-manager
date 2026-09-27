//! Input-method checks. Per-keyboard layouts only work while the Japanese IME (kbdjpn.dll) is active.

use serde::{Deserialize, Serialize};

use crate::ids::{hkl_has_japanese_layout, hkl_to_string, klid_has_japanese_layout};
use crate::model::InputMethods;

/// Problems with the configured input methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InputWarning {
    /// Input methods without the Japanese layout (kbdjpn.dll) are installed. Switching to one (e.g.
    /// 00000409, or Japanese with the US layout 04090411) makes every keyboard use that method's
    /// layout, whatever its per-keyboard setting.
    NonJapaneseLayouts {
        /// KLIDs from the user's Preload.
        preload: Vec<String>,
        /// Loaded HKLs (low 32 bits) formatted as 8 hex digits.
        loaded: Vec<String>,
    },
    /// The user's default input method (first Preload entry) is not Japanese.
    UserDefaultNotJapanese { klid: String },
    /// The sign-in screen's default input method (`HKU\.DEFAULT`) is not Japanese or unknown:
    /// every keyboard is US there.
    SignInNotJapanese { klid: Option<String> },
    /// No input method uses the Japanese layout (kbdjpn.dll): per-keyboard layouts have no effect.
    NoJapaneseLayout,
}

/// Lists every input-method warning. Nothing is reported for the user side when both the Preload
/// list and the loaded layouts are empty (not read).
pub fn input_warnings(input: &InputMethods) -> Vec<InputWarning> {
    let mut warnings = Vec::new();
    let user_known = !input.user_preload.is_empty() || !input.loaded_layouts.is_empty();
    let any_japanese = input
        .user_preload
        .iter()
        .any(|k| klid_has_japanese_layout(k))
        || input
            .loaded_layouts
            .iter()
            .any(|&h| hkl_has_japanese_layout(h));

    if user_known && !any_japanese {
        warnings.push(InputWarning::NoJapaneseLayout);
    } else if user_known {
        let preload: Vec<String> = input
            .user_preload
            .iter()
            .filter(|k| !klid_has_japanese_layout(k))
            .cloned()
            .collect();
        let loaded: Vec<String> = input
            .loaded_layouts
            .iter()
            .filter(|&&h| !hkl_has_japanese_layout(h))
            .map(|&h| hkl_to_string(h))
            .collect();
        if !preload.is_empty() || !loaded.is_empty() {
            warnings.push(InputWarning::NonJapaneseLayouts { preload, loaded });
        }
        if let Some(first) = input.user_preload.first()
            && !klid_has_japanese_layout(first)
        {
            warnings.push(InputWarning::UserDefaultNotJapanese {
                klid: first.clone(),
            });
        }
    }

    match input.sign_in_preload.first() {
        Some(first) if klid_has_japanese_layout(first) => {}
        first => warnings.push(InputWarning::SignInNotJapanese {
            klid: first.cloned(),
        }),
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn methods(user: &[&str], sign_in: &[&str], loaded: &[u32]) -> InputMethods {
        InputMethods {
            user_preload: user.iter().map(|s| s.to_string()).collect(),
            sign_in_preload: sign_in.iter().map(|s| s.to_string()).collect(),
            loaded_layouts: loaded.to_vec(),
        }
    }

    #[test]
    fn dev_machine_warns_about_us_layout() {
        assert_eq!(
            input_warnings(&fixtures::input_methods()),
            vec![InputWarning::NonJapaneseLayouts {
                preload: vec!["00000409".into()],
                loaded: vec!["04090409".into()],
            }]
        );
    }

    #[test]
    fn japanese_only_is_clean() {
        let input = methods(&["00000411"], &["00000411"], &[0x0411_0411]);
        assert_eq!(input_warnings(&input), vec![]);
        let ime = methods(&["E0200411"], &["00000411", "00000409"], &[0xE020_0411]);
        assert_eq!(input_warnings(&ime), vec![]);
    }

    #[test]
    fn us_default_and_sign_in() {
        let input = methods(&["00000409", "00000411"], &["00000409"], &[]);
        assert_eq!(
            input_warnings(&input),
            vec![
                InputWarning::NonJapaneseLayouts {
                    preload: vec!["00000409".into()],
                    loaded: vec![],
                },
                InputWarning::UserDefaultNotJapanese {
                    klid: "00000409".into()
                },
                InputWarning::SignInNotJapanese {
                    klid: Some("00000409".into())
                },
            ]
        );
    }

    #[test]
    fn no_japanese_at_all() {
        let input = methods(&["00000409"], &["00000411"], &[0x0409_0409]);
        assert_eq!(input_warnings(&input), vec![InputWarning::NoJapaneseLayout]);
    }

    #[test]
    fn layout_part_of_hkl_decides() {
        // Japanese language with the US layout: every keyboard types US while it is active.
        let japanese_us = methods(&["00000411"], &["00000411"], &[0x0411_0411, 0x0409_0411]);
        assert_eq!(
            input_warnings(&japanese_us),
            vec![InputWarning::NonJapaneseLayouts {
                preload: vec![],
                loaded: vec!["04090411".into()],
            }]
        );
        // English language with the Japanese layout still uses kbdjpn.dll.
        let english_jis = methods(&[], &["00000411"], &[0x0411_0409]);
        assert_eq!(input_warnings(&english_jis), vec![]);
    }

    #[test]
    fn unread_lists() {
        assert_eq!(
            input_warnings(&InputMethods::default()),
            vec![InputWarning::SignInNotJapanese { klid: None }]
        );
    }
}
