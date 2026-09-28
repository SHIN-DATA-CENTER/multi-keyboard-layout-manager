//! The watchers (design m3 A.4). Each one reaches the UI thread in its own way and never does
//! work where it is called:
//!
//! | Source | Thread | Path to the UI thread |
//! |---|---|---|
//! | OS theme (`UISettings.ColorValuesChanged`) | WinRT thread pool | `invoke_from_event_loop` |
//! | Text size (`UISettings.TextScaleFactorChanged`) | WinRT thread pool | `invoke_from_event_loop` |
//! | Shell window (`TaskbarCreated`, `WM_SETTINGCHANGE`, end of session, resume) | UI thread (inside the window procedure) | `invoke_from_event_loop` (never re-entered), except the end of session (design m3 F.5) |
//! | Keyboard arrival / removal (`CM_Register_Notification`) | `mklm-keyboards` (fed by the CfgMgr32 thread pool) | `invoke_from_event_loop`, then one read per 750 ms burst (`state::KEYBOARD_SETTLE`) |
//! | Input language (`GetKeyboardLayout(0)`) | UI thread | a 500 ms `slint::Timer` while the window is visible |
//! | Single instance (`activate`, `quit`) | its own thread (`single_instance`) | `invoke_from_event_loop` |

use mklm_win::notify::{DeviceChange, KeyboardWatcher};
use mklm_win::ui::shell_window::{ShellEvent, ShellWatcher};
use mklm_win::ui::theme::{OsTheme, OsThemeWatcher, TextScaleWatcher};

/// The watchers that live as long as the UI. Dropped on the UI thread at the end (the keyboard
/// watcher's drop waits for its dispatcher, which only schedules work and never waits for the UI
/// thread).
#[derive(Debug, Default)]
pub struct Watchers {
    pub theme: Option<OsThemeWatcher>,
    pub text_scale: Option<TextScaleWatcher>,
    pub shell: Option<ShellWatcher>,
    pub keyboards: Option<KeyboardWatcher>,
}

impl Watchers {
    /// Starts the theme, text-size, shell and keyboard watchers; failures are returned as
    /// warnings (the GUI works without them, it only follows the OS less closely — without the
    /// keyboard watcher the list changes on "最新の情報に更新" or F5).
    pub fn start(
        on_theme: impl Fn(OsTheme) + Send + 'static,
        on_shell: impl Fn(ShellEvent) + 'static,
        on_text_scale: impl Fn(f64) + Send + 'static,
        on_keyboards: impl Fn(DeviceChange) + Send + Sync + 'static,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let theme = OsThemeWatcher::new(on_theme)
            .map_err(|error| warnings.push(format!("ColorValuesChanged: {error}")))
            .ok();
        let text_scale = TextScaleWatcher::new(on_text_scale)
            .map_err(|error| warnings.push(format!("TextScaleFactorChanged: {error}")))
            .ok();
        let shell = ShellWatcher::new(on_shell)
            .map_err(|error| warnings.push(format!("shell window: {error}")))
            .ok();
        let keyboards = KeyboardWatcher::new(on_keyboards)
            .map_err(|error| warnings.push(format!("keyboard notifications: {error}")))
            .ok();
        (
            Self {
                theme,
                text_scale,
                shell,
                keyboards,
            },
            warnings,
        )
    }
}
