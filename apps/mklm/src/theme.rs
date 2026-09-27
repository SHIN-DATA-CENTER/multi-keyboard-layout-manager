//! The theme the user chose and how it resolves (plan 3.7, design m3 C.1). Pure; the OS side is
//! `mklm_win::ui::theme` and the application to the window is in `app.rs`.

use serde::{Deserialize, Serialize};

/// Light, dark, or follow Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemeMode {
    /// The settings ComboBox index (0 = light, 1 = dark, 2 = system).
    pub fn from_index(index: i32) -> Self {
        match index {
            0 => Self::Light,
            1 => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::Light => 0,
            Self::Dark => 1,
            Self::System => 2,
        }
    }

    /// `light` / `dark` / `system` (command line).
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// True when the resolved theme is dark. Never "unknown" (plan 3.7: `Palette.color-scheme`
    /// is always set explicitly). `os_dark` is `None` when the OS theme could not be read: then
    /// light.
    pub fn resolve(self, os_dark: Option<bool>) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => os_dark.unwrap_or(false),
        }
    }
}

/// What the window shows (design m3 C.2): high contrast wins over light and dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub dark: bool,
    pub high_contrast: bool,
}

impl ResolvedTheme {
    /// `hc_background_dark`: whether the high-contrast window colour is dark (then the std-widgets
    /// use their dark scheme, which is closer; design m3 C.2).
    pub fn new(
        mode: ThemeMode,
        os_dark: Option<bool>,
        high_contrast: bool,
        hc_background_dark: bool,
    ) -> Self {
        Self {
            dark: if high_contrast {
                hc_background_dark
            } else {
                mode.resolve(os_dark)
            },
            high_contrast,
        }
    }
}

/// True when an `0xRRGGBB` colour is dark (relative luminance below one half).
pub fn is_dark_rgb(rgb: u32) -> bool {
    let [_, r, g, b] = rgb.to_be_bytes();
    // Rec. 709 luma on gamma-encoded values is close enough to decide dark or light.
    2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) < 10_000 * 128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_never_unknown() {
        assert!(!ThemeMode::Light.resolve(Some(true)));
        assert!(ThemeMode::Dark.resolve(Some(false)));
        assert!(ThemeMode::System.resolve(Some(true)));
        assert!(!ThemeMode::System.resolve(None));
        for i in 0..3 {
            assert_eq!(ThemeMode::from_index(i).index(), i);
        }
        assert_eq!(ThemeMode::parse("DARK"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::parse("blue"), None);
    }

    #[test]
    fn high_contrast_follows_its_background() {
        let hc = ResolvedTheme::new(ThemeMode::Light, Some(false), true, true);
        assert!(hc.dark && hc.high_contrast);
        assert!(is_dark_rgb(0x000000));
        assert!(!is_dark_rgb(0xFFFFFF));
    }
}
