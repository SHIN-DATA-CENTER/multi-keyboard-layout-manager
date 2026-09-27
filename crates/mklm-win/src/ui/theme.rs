//! App-owned theme resolution (plan 3.7, M0 #8; design m3 C.1): the OS app theme through WinRT
//! `UISettings` (`ColorValuesChanged`), high contrast, and the title bar through DWM.
//!
//! Moved from `prototypes/slint-proto/src/theme.rs`, which passed the manual checks C and D.

use windows::Foundation::TypedEventHandler;
use windows::UI::Color;
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{IInspectable, Ref, w};

use crate::error::Error;
use crate::sys::win32;

/// Microsoft's documented heuristic: a light foreground colour means dark mode.
pub fn is_color_light(r: u8, g: u8, b: u8) -> bool {
    5 * u32::from(g) + 2 * u32::from(r) + u32::from(b) > 8 * 128
}

/// The OS app theme as seen through `UISettings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsTheme {
    pub dark: bool,
    /// `UIColorType::Foreground` as `0xAARRGGBB`.
    pub foreground: u32,
}

fn read_theme(settings: &UISettings) -> windows::core::Result<OsTheme> {
    let Color { A, R, G, B } = settings.GetColorValue(UIColorType::Foreground)?;
    Ok(OsTheme {
        dark: is_color_light(R, G, B),
        foreground: u32::from_be_bytes([A, R, G, B]),
    })
}

/// Reads the OS app theme once (before the first window exists, so that the window is created
/// with the right theme, design m3 C.1). `UISettings` needs no explicit COM initialisation.
pub fn read_os_theme() -> Result<OsTheme, Error> {
    let settings = UISettings::new().map_err(|error| win32("UISettings", &error))?;
    read_theme(&settings).map_err(|error| win32("UISettings.GetColorValue", &error))
}

/// Keeps a `UISettings` instance alive and subscribed to `ColorValuesChanged`.
///
/// The event is raised on a thread-pool thread; `on_change` must marshal to the UI thread
/// (`slint::invoke_from_event_loop`).
#[derive(Debug)]
pub struct OsThemeWatcher {
    settings: UISettings,
    token: i64,
}

impl OsThemeWatcher {
    pub fn new(on_change: impl Fn(OsTheme) + Send + 'static) -> Result<Self, Error> {
        let settings = UISettings::new().map_err(|error| win32("UISettings", &error))?;
        let handler =
            TypedEventHandler::new(move |sender: Ref<UISettings>, _: Ref<IInspectable>| {
                if let Some(sender) = sender.as_ref() {
                    on_change(read_theme(sender)?);
                }
                Ok(())
            });
        let token = settings
            .ColorValuesChanged(&handler)
            .map_err(|error| win32("UISettings.ColorValuesChanged", &error))?;
        Ok(Self { settings, token })
    }

    /// Reads the theme now (e.g. on `WM_SETTINGCHANGE("ImmersiveColorSet")`).
    pub fn read(&self) -> Result<OsTheme, Error> {
        read_theme(&self.settings).map_err(|error| win32("UISettings.GetColorValue", &error))
    }
}

impl Drop for OsThemeWatcher {
    fn drop(&mut self) {
        let _ = self.settings.RemoveColorValuesChanged(self.token);
    }
}

/// Windows' "Text size" (設定 > アクセシビリティ > テキストのサイズ, `UISettings.TextScaleFactor`,
/// 1.0 to 2.25). Win32 DPI scaling does not include it, and Slint does not read it, so the GUI
/// multiplies its font sizes by it (`Theme.font-scale`, design m3 E.4).
pub fn read_text_scale_factor() -> Result<f64, Error> {
    let settings = UISettings::new().map_err(|error| win32("UISettings", &error))?;
    settings
        .TextScaleFactor()
        .map_err(|error| win32("UISettings.TextScaleFactor", &error))
}

/// Keeps a `UISettings` instance subscribed to `TextScaleFactorChanged`. The event is raised on a
/// thread-pool thread; `on_change` must marshal to the UI thread.
#[derive(Debug)]
pub struct TextScaleWatcher {
    settings: UISettings,
    token: i64,
}

impl TextScaleWatcher {
    pub fn new(on_change: impl Fn(f64) + Send + 'static) -> Result<Self, Error> {
        let settings = UISettings::new().map_err(|error| win32("UISettings", &error))?;
        let handler =
            TypedEventHandler::new(move |sender: Ref<UISettings>, _: Ref<IInspectable>| {
                if let Some(sender) = sender.as_ref() {
                    on_change(sender.TextScaleFactor()?);
                }
                Ok(())
            });
        let token = settings
            .TextScaleFactorChanged(&handler)
            .map_err(|error| win32("UISettings.TextScaleFactorChanged", &error))?;
        Ok(Self { settings, token })
    }
}

impl Drop for TextScaleWatcher {
    fn drop(&mut self) {
        let _ = self.settings.RemoveTextScaleFactorChanged(self.token);
    }
}

/// `HKCU\...\Themes\Personalize\AppsUseLightTheme` (read-only; diagnostics only).
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

/// Sets `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) on a top-level window. `hwnd` is the raw window
/// handle from winit (`RawWindowHandle::Win32`). Call it together with winit's `set_theme`, so
/// that both keep the same state (M0 #8, prototype README 12).
pub fn set_title_bar_dark(hwnd: isize, dark: bool) -> Result<(), Error> {
    let value = i32::from(dark);
    let hwnd = HWND(std::ptr::without_provenance_mut(hwnd as usize));
    // SAFETY: `hwnd` comes from winit for a live window of this thread; the attribute is a 4-byte
    // BOOL read during the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        )
    }
    .map_err(|error| win32("DwmSetWindowAttribute", &error))
}

/// True while a high-contrast theme is on (`SPI_GETHIGHCONTRAST`, `HCF_HIGHCONTRASTON`).
pub fn high_contrast_on() -> Result<bool, Error> {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a HIGHCONTRASTW with its size set; SPI_GETHIGHCONTRAST writes into it.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some((&raw mut info).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map_err(|error| win32("SystemParametersInfoW", &error))?;
    Ok(info.dwFlags & HCF_HIGHCONTRASTON == HCF_HIGHCONTRASTON)
}

/// The system colours a high-contrast theme defines (`GetSysColor`), as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    pub window: u32,
    pub window_text: u32,
    pub highlight: u32,
    pub highlight_text: u32,
    pub button_face: u32,
    pub button_text: u32,
    pub gray_text: u32,
    pub hot_light: u32,
}

/// Reads the colours of the current (high-contrast) theme.
pub fn system_colors() -> SystemColors {
    let color = |index: SYS_COLOR_INDEX| {
        // SAFETY: GetSysColor has no preconditions; an unknown index returns 0.
        let bgr = unsafe { GetSysColor(index) };
        let [b, g, r] = [(bgr >> 16) & 0xFF, (bgr >> 8) & 0xFF, bgr & 0xFF];
        (r << 16) | (g << 8) | b
    };
    SystemColors {
        window: color(COLOR_WINDOW),
        window_text: color(COLOR_WINDOWTEXT),
        highlight: color(COLOR_HIGHLIGHT),
        highlight_text: color(COLOR_HIGHLIGHTTEXT),
        button_face: color(COLOR_BTNFACE),
        button_text: color(COLOR_BTNTEXT),
        gray_text: color(COLOR_GRAYTEXT),
        hot_light: color(COLOR_HOTLIGHT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_heuristic() {
        assert!(is_color_light(255, 255, 255));
        assert!(!is_color_light(0, 0, 0));
    }

    #[test]
    fn reads_do_not_fail() {
        // Read-only calls; the values depend on the machine.
        let _ = read_os_theme().unwrap();
        let _ = high_contrast_on().unwrap();
        let scale = read_text_scale_factor().unwrap();
        assert!((1.0..=2.25).contains(&scale), "{scale}");
        let colors = system_colors();
        assert!(colors.window <= 0xFF_FFFF);
    }
}
