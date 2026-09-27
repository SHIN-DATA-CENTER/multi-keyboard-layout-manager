//! Win32 pieces of the GUI (`apps/mklm`, milestone M3; design m3 A.5). Compiled with the `gui`
//! feature only, which only `apps/mklm` enables. Nothing here writes the machine's keyboard
//! settings; the GUI never does (plan 2.1).
//!
//! - [`theme`]: the OS app theme (`UISettings`), the text size, high contrast and the DWM title
//!   bar.
//! - [`shell_window`]: the hidden window for `TaskbarCreated`, `WM_SETTINGCHANGE`, end of session
//!   and power resume.
//! - Here: the active input language of the GUI thread, the Windows display language, the
//!   per-user settings folder, foreground hand-over for the single instance, opening an
//!   allowlisted settings page, and copying diagnostics to the clipboard.

pub mod shell_window;
pub mod theme;

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::Shell::{
    FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, SW_SHOWNORMAL};
use windows::core::{PCWSTR, w};

use crate::elevation::ComApartment;
use crate::error::Error;
use crate::sys::{last_error, wide_os, win32};

/// The user's Windows display language (`GetUserDefaultUILanguage`), e.g. `0x0411` for Japanese.
/// The GUI follows it when its language setting is "system" (design m3 D.3).
pub fn user_default_ui_language() -> u16 {
    // SAFETY: GetUserDefaultUILanguage has no preconditions.
    unsafe { GetUserDefaultUILanguage() }
}

/// `%APPDATA%\SHIN DATA CENTER\MKLM` (`FOLDERID_RoamingAppData`): where the GUI keeps its per-user
/// `settings.toml` (plan 3.7). Only computed; nothing is created here.
pub fn user_settings_dir() -> Result<PathBuf, Error> {
    // SAFETY: FOLDERID_RoamingAppData is a static GUID; no token (the current user). The returned
    // string is freed below.
    let text = unsafe { SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let base = PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: allocated by SHGetKnownFolderPath with CoTaskMemAlloc; freed once, after the copy.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    Ok(base.join("SHIN DATA CENTER").join("MKLM"))
}

/// The input language (HKL) active on the calling thread, low 32 bits (e.g. `0x04110411` for the
/// Japanese IME, `0x04090409` for English (US)). The GUI polls it on its UI thread while the
/// window is visible (design m3 A.4): `WM_INPUTLANGCHANGE` only reaches the focused window, which
/// winit owns.
pub fn active_keyboard_layout() -> u32 {
    // SAFETY: GetKeyboardLayout has no preconditions; 0 means the calling thread.
    let hkl = unsafe { GetKeyboardLayout(0) };
    (hkl.0 as usize & 0xFFFF_FFFF) as u32
}

/// Lets process `pid` (the running MKLM instance) take the foreground (`AllowSetForegroundWindow`),
/// before a second instance asks it to activate (plan 3.9).
pub fn allow_set_foreground_window(pid: u32) -> Result<(), Error> {
    // SAFETY: plain Win32 call with a process ID.
    unsafe { AllowSetForegroundWindow(pid) }
        .map_err(|error| win32("AllowSetForegroundWindow", &error))
}

/// The pages MKLM opens (plan 3.1, 3.10; design m3 B.8, B.16). Only these: the GUI never passes a
/// string of its own to `ShellExecuteW`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    /// 時刻と言語 > 言語と地域 (`ms-settings:regionlanguage`).
    RegionLanguage,
    /// 入力 > キーボードの詳細設定 (`ms-settings:keyboard-advanced`).
    KeyboardAdvanced,
    /// Microsoft IME の設定 (`ms-settings:regionlanguage-jpnime`).
    JapaneseIme,
    /// 地域の設定 (`intl.cpl`): 管理 > 設定のコピー (the sign-in screen's input method).
    IntlControlPanel,
    /// 個人用設定 > タスク バー (`ms-settings:taskbar`): where the tray icon can be kept visible
    /// (the first close notice, design m3 B.16).
    Taskbar,
    /// アカウント > サインイン オプション (`ms-settings:signinoptions`): set up a PIN before a
    /// restart that changes a layout (design m3 B.8).
    SignInOptions,
}

/// What `ShellExecuteW` opens for a page.
enum Target {
    /// A settings URI.
    Uri(PCWSTR),
    /// A program in System32, by absolute path and with System32 as its working directory, so
    /// that a file of the same name in the current directory is never run (design m3 A.5).
    System32 {
        file: &'static str,
        parameters: PCWSTR,
    },
}

impl SettingsPage {
    fn target(self) -> Target {
        match self {
            SettingsPage::RegionLanguage => Target::Uri(w!("ms-settings:regionlanguage")),
            SettingsPage::KeyboardAdvanced => Target::Uri(w!("ms-settings:keyboard-advanced")),
            SettingsPage::JapaneseIme => Target::Uri(w!("ms-settings:regionlanguage-jpnime")),
            SettingsPage::Taskbar => Target::Uri(w!("ms-settings:taskbar")),
            SettingsPage::SignInOptions => Target::Uri(w!("ms-settings:signinoptions")),
            SettingsPage::IntlControlPanel => Target::System32 {
                file: "control.exe",
                parameters: w!("intl.cpl"),
            },
        }
    }
}

/// Opens `page` (`ShellExecuteW`, verb `open`). Blocking for a moment: call it off the UI thread.
pub fn open_settings_page(page: SettingsPage) -> Result<(), Error> {
    let _com = ComApartment::enter();
    let opened = match page.target() {
        Target::Uri(uri) => shell_open(uri, PCWSTR::null(), PCWSTR::null()),
        Target::System32 { file, parameters } => {
            let dir = crate::elevation::system_directory()?;
            let program = wide_os(dir.join(file).as_os_str());
            let dir = wide_os(dir.as_os_str());
            shell_open(PCWSTR(program.as_ptr()), parameters, PCWSTR(dir.as_ptr()))
        }
    };
    if opened {
        Ok(())
    } else {
        Err(last_error("ShellExecuteW"))
    }
}

/// `ShellExecuteW(open)`; true on success.
fn shell_open(file: PCWSTR, parameters: PCWSTR, directory: PCWSTR) -> bool {
    // SAFETY: the callers pass static NUL-terminated literals, null, or NUL-terminated buffers
    // that live until this call returns.
    let result =
        unsafe { ShellExecuteW(None, w!("open"), file, parameters, directory, SW_SHOWNORMAL) };
    // ShellExecuteW returns a value greater than 32 on success.
    result.0 as usize > 32
}

/// Puts `text` on the clipboard as `CF_UNICODETEXT` (the result screen's "copy details", design
/// m3 B.17): English diagnostics, the short operation ID and the build ID — never key contents.
///
/// Implementation (WP-W2): `OpenClipboard(None)` (retry a few times for 100 ms: another process
/// may hold it), `EmptyClipboard`, a `GlobalAlloc(GMEM_MOVEABLE)` copy of the NUL-terminated
/// UTF-16 text, `SetClipboardData(CF_UNICODETEXT)` (the clipboard owns the memory on success, it
/// is freed on failure), `CloseClipboard`.
pub fn copy_text_to_clipboard(text: &str) -> Result<(), Error> {
    let _ = text;
    todo!("WP-W2: CF_UNICODETEXT on the clipboard")
}
