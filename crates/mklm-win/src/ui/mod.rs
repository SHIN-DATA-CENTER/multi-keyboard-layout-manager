//! Win32 pieces of the GUI (`apps/mklm`, milestone M3; design m3 A.5). Compiled with the `gui`
//! feature only, which only `apps/mklm` enables. Nothing here writes the machine's keyboard
//! settings; the GUI never does (plan 2.1).
//!
//! - [`theme`]: the OS app theme (`UISettings`), the text size, high contrast and the DWM title
//!   bar.
//! - [`shell_window`]: the hidden window for `TaskbarCreated`, `WM_SETTINGCHANGE`, end of session
//!   and power resume.
//! - Here: the active input language of the GUI thread, the Windows display language, the
//!   per-user settings and log folders, foreground hand-over for the single instance, opening an
//!   allowlisted settings page or the recovery files folder, copying diagnostics to the
//!   clipboard, and the start-up error message box.

pub mod shell_window;
pub mod theme;

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DestroyWindow, HWND_MESSAGE, MB_ICONERROR, MB_OK,
    MB_SETFOREGROUND, MessageBoxW, SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{GUID, PCWSTR, w};

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
    Ok(known_folder(&FOLDERID_RoamingAppData)?
        .join("SHIN DATA CENTER")
        .join("MKLM"))
}

/// `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\logs` (`FOLDERID_LocalAppData`): where the GUI writes its
/// log (design m3 F.8). Local, not roaming: a log belongs to this PC. Only computed; nothing is
/// created here.
pub fn user_log_dir() -> Result<PathBuf, Error> {
    Ok(known_folder(&FOLDERID_LocalAppData)?
        .join("SHIN DATA CENTER")
        .join("MKLM")
        .join("logs"))
}

/// The current user's known folder `id` (`SHGetKnownFolderPath`), never from the environment.
fn known_folder(id: &GUID) -> Result<PathBuf, Error> {
    // SAFETY: `id` is a valid known-folder GUID; no token (the current user). The returned string
    // is freed below.
    let text = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let path = PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: allocated by SHGetKnownFolderPath with CoTaskMemAlloc; freed once, after the copy.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    Ok(path)
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

/// The levels below `%ProgramData%` of the folder with the offline recovery files (design m2
/// G.1: Users may read it).
const RECOVERY_FOLDER: [&str; 3] = ["SHIN DATA CENTER", "MKLM", "Recovery"];

/// Opens `%ProgramData%\SHIN DATA CENTER\MKLM\Recovery` in Explorer (the history's
/// "復旧用ファイルのフォルダーを開く", design m3 B.11). Only that folder: the path is built from the
/// known-folder API and constants, and every level must be a plain directory, not a reparse point,
/// so nothing but that folder is ever handed to `ShellExecuteW` (a folder opens in Explorer; no
/// program runs). Fails when the folder does not exist yet (the helper creates it with its first
/// change). Blocking for a moment: call it off the UI thread.
pub fn open_recovery_folder() -> Result<(), Error> {
    use std::os::windows::fs::MetadataExt;

    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let mut path = crate::protected_dir::program_data_dir()?;
    for level in RECOVERY_FOLDER {
        path.push(level);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| Error::Win32 {
            function: "GetFileAttributesExW",
            code: error.raw_os_error().map_or(0, |code| code as u32),
        })?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || !metadata.is_dir() {
            return Err(Error::Insecure {
                path: path.display().to_string(),
                reason: "not a plain directory".to_string(),
            });
        }
    }
    let _com = ComApartment::enter();
    let folder = wide_os(path.as_os_str());
    if shell_open(PCWSTR(folder.as_ptr()), PCWSTR::null(), PCWSTR::null()) {
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
/// A NUL in `text` ends the copied text there.
///
/// The clipboard is opened for a message-only window made for this call (`EmptyClipboard` after
/// `OpenClipboard(NULL)` leaves the clipboard without an owner, which the documentation says
/// makes `SetClipboardData` fail). Opening is retried for about 100 ms, because another program
/// may hold the clipboard for a moment. The text
/// is a `GlobalAlloc(GMEM_MOVEABLE)` copy that the clipboard owns once `SetClipboardData`
/// succeeds (it is freed here when it fails). Blocks for at most about 100 ms; any thread.
pub fn copy_text_to_clipboard(text: &str) -> Result<(), Error> {
    let units = wide_text(text);
    let owner = MessageWindow::new()?;
    open_clipboard(owner.0)?;
    let _open = ClipboardOpen;
    // SAFETY: this thread opened the clipboard above.
    unsafe { EmptyClipboard() }.map_err(|error| win32("EmptyClipboard", &error))?;
    let memory = GlobalText::new(&units)?;
    // SAFETY: the clipboard is open and owned by `owner`; `memory` is a movable global block
    // holding NUL-terminated UTF-16, unlocked.
    match unsafe { SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0.0))) } {
        Ok(_) => {
            // The clipboard owns the memory now.
            std::mem::forget(memory);
            Ok(())
        }
        Err(error) => Err(win32("SetClipboardData", &error)),
    }
}

/// `CF_UNICODETEXT` (winuser.h; the `windows` crate keeps it in `Win32_System_Ole`).
const CF_UNICODETEXT: u32 = 13;

/// How many times [`copy_text_to_clipboard`] tries to open the clipboard.
const CLIPBOARD_ATTEMPTS: u32 = 10;

/// Pause between two attempts to open the clipboard.
const CLIPBOARD_RETRY: Duration = Duration::from_millis(10);

/// The NUL-terminated UTF-16 the clipboard gets for `text`: up to the first NUL.
fn wide_text(text: &str) -> Vec<u16> {
    let text = text.split('\0').next().unwrap_or_default();
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `OpenClipboard(owner)`, retried while another program holds the clipboard.
fn open_clipboard(owner: HWND) -> Result<(), Error> {
    let mut attempt = 1;
    loop {
        // SAFETY: `owner` is a window of this thread that lives until the clipboard is closed.
        match unsafe { windows::Win32::System::DataExchange::OpenClipboard(Some(owner)) } {
            Ok(()) => return Ok(()),
            Err(_) if attempt < CLIPBOARD_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(CLIPBOARD_RETRY);
            }
            Err(error) => return Err(win32("OpenClipboard", &error)),
        }
    }
}

/// Closes the clipboard this thread opened, when dropped.
struct ClipboardOpen;

impl Drop for ClipboardOpen {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard before creating this guard.
        let _ = unsafe { CloseClipboard() };
    }
}

/// A message-only window (the predefined `STATIC` class under `HWND_MESSAGE`) that owns the
/// clipboard for one copy; destroyed when dropped, after the clipboard was closed. The text stays
/// on the clipboard: it was rendered at once, not delayed.
struct MessageWindow(HWND);

impl MessageWindow {
    fn new() -> Result<Self, Error> {
        // SAFETY: a predefined class and static strings; no parameters; the window is destroyed
        // by `Drop` on this thread.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|error| win32("CreateWindowExW", &error))?;
        Ok(Self(window))
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created by this thread in `new` and is destroyed once.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// A movable global memory block with a copy of UTF-16 text, freed when dropped (unless handed
/// to the clipboard and forgotten).
struct GlobalText(HGLOBAL);

impl GlobalText {
    fn new(units: &[u16]) -> Result<Self, Error> {
        let bytes = std::mem::size_of_val(units);
        // SAFETY: plain allocation of `bytes` (> 0: `units` ends in a NUL) movable bytes.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
            .map_err(|error| win32("GlobalAlloc", &error))?;
        let block = Self(memory);
        // SAFETY: `memory` is a live movable block; the pointer is valid until GlobalUnlock.
        let pointer = unsafe { GlobalLock(memory) }.cast::<u16>();
        if pointer.is_null() {
            return Err(last_error("GlobalLock"));
        }
        // SAFETY: the locked block holds at least `bytes` bytes, suitably aligned for u16
        // (global memory is 8-byte aligned); `units` does not overlap it.
        unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len()) };
        // SAFETY: balances the GlobalLock above. It reports "failure" with NO_ERROR once the
        // lock count reaches zero, which is the expected outcome.
        let _ = unsafe { GlobalUnlock(memory) };
        Ok(block)
    }
}

impl Drop for GlobalText {
    fn drop(&mut self) {
        // SAFETY: the block was allocated by GlobalAlloc, is unlocked, and was not handed to the
        // clipboard (that path forgets this guard).
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}

/// Shows `text` in a modal error message box titled `title` (`MessageBoxW` with `MB_OK |
/// MB_ICONERROR | MB_SETFOREGROUND`, no owner) and returns when the user closes it: start-up
/// errors before the window exists, since release builds have no console (design m3 F.6). The
/// caller passes the texts in the UI language. A NUL in either text ends it there.
pub fn error_dialog(title: &str, text: &str) {
    let title = wide_text(title);
    let text = wide_text(text);
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clipboard and the message box get NUL-terminated UTF-16 that ends at the first NUL.
    /// (Neither is shown or written in tests: the clipboard belongs to the user.)
    #[test]
    fn texts_are_nul_terminated_utf16() {
        assert_eq!(wide_text("MKLM"), vec![77, 75, 76, 77, 0]);
        assert_eq!(wide_text(""), vec![0]);
        assert_eq!(wide_text("a\0b"), vec![97, 0]);
        let japanese = wide_text("管理用プログラム");
        assert_eq!(japanese.len(), 9);
        assert_eq!(japanese.last(), Some(&0));
        assert_eq!(String::from_utf16_lossy(&japanese[..8]), "管理用プログラム");
    }

    /// The per-user folders come from the known-folder API (read only; nothing is created).
    #[test]
    fn per_user_folders() {
        let settings = user_settings_dir().expect("roaming app data");
        let logs = user_log_dir().expect("local app data");
        assert!(settings.is_absolute() && logs.is_absolute());
        assert!(settings.ends_with(r"SHIN DATA CENTER\MKLM"), "{settings:?}");
        assert!(logs.ends_with(r"SHIN DATA CENTER\MKLM\logs"), "{logs:?}");
    }
}
