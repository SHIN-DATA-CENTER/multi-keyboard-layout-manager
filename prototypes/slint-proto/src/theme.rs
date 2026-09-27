//! App-owned theme resolution: OS theme via WinRT `UISettings`, title bar via DWM.

use windows::Foundation::TypedEventHandler;
use windows::UI::Color;
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::{IInspectable, Ref, w};

/// Theme selected by the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

impl ThemeMode {
    /// Maps the ComboBox index (0 = Light, 1 = Dark, 2 = System).
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "ライト",
            Self::Dark => "ダーク",
            Self::System => "システム",
        }
    }

    /// Parses `light` / `dark` / `system` (command line).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// Returns true when the resolved theme is dark. Never yields "unknown".
    pub fn resolve(self, os_dark: bool) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => os_dark,
        }
    }
}

/// Microsoft's documented heuristic: a light foreground color means dark mode.
pub fn is_color_light(c: Color) -> bool {
    5 * u32::from(c.G) + 2 * u32::from(c.R) + u32::from(c.B) > 8 * 128
}

/// OS app theme as seen through `UISettings`.
#[derive(Clone, Copy, Debug)]
pub struct OsTheme {
    pub dark: bool,
    pub foreground: Color,
}

impl OsTheme {
    pub fn read(settings: &UISettings) -> windows::core::Result<Self> {
        let foreground = settings.GetColorValue(UIColorType::Foreground)?;
        Ok(Self {
            dark: is_color_light(foreground),
            foreground,
        })
    }

    pub fn describe(&self) -> String {
        let c = self.foreground;
        format!(
            "OS: {}（UISettings Foreground=#{:02X}{:02X}{:02X}{:02X}、参考: AppsUseLightTheme={}）",
            if self.dark { "ダーク" } else { "ライト" },
            c.A,
            c.R,
            c.G,
            c.B,
            apps_use_light_theme().map_or_else(|| "なし".to_owned(), |v| v.to_string()),
        )
    }
}

/// Keeps a `UISettings` instance alive and subscribed to `ColorValuesChanged`.
///
/// The event is raised on a thread-pool thread; the callback must marshal to the UI thread.
#[derive(Debug)]
pub struct OsThemeWatcher {
    settings: UISettings,
    token: i64,
}

impl OsThemeWatcher {
    pub fn new(
        settings: UISettings,
        on_change: impl Fn(OsTheme) + Send + 'static,
    ) -> windows::core::Result<Self> {
        let handler =
            TypedEventHandler::new(move |sender: Ref<UISettings>, _: Ref<IInspectable>| {
                if let Some(sender) = sender.as_ref() {
                    on_change(OsTheme::read(sender)?);
                }
                Ok(())
            });
        let token = settings.ColorValuesChanged(&handler)?;
        Ok(Self { settings, token })
    }

    pub fn read(&self) -> windows::core::Result<OsTheme> {
        OsTheme::read(&self.settings)
    }
}

impl Drop for OsThemeWatcher {
    fn drop(&mut self) {
        let _ = self.settings.RemoveColorValuesChanged(self.token);
    }
}

/// Reads `HKCU\...\Themes\Personalize\AppsUseLightTheme` (read-only; for display only).
pub fn apps_use_light_theme() -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are valid for writes and `size` matches the buffer; RegGetValueW
    // opens the key with query access only.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

/// Sets `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) on a top-level window.
pub fn set_title_bar_dark(hwnd: HWND, dark: bool) -> windows::core::Result<()> {
    let value = i32::from(dark);
    // SAFETY: `hwnd` comes from winit for a live window; the attribute is a 4-byte BOOL.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_heuristic() {
        let white = Color {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        };
        let black = Color {
            A: 255,
            R: 0,
            G: 0,
            B: 0,
        };
        assert!(is_color_light(white));
        assert!(!is_color_light(black));
    }

    #[test]
    fn resolve_never_unknown() {
        assert!(!ThemeMode::Light.resolve(true));
        assert!(ThemeMode::Dark.resolve(false));
        assert!(ThemeMode::System.resolve(true));
        assert!(!ThemeMode::System.resolve(false));
        for i in 0..3 {
            assert_eq!(ThemeMode::from_index(i).index(), i);
        }
    }
}
