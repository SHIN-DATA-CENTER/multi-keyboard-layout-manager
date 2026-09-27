//! A hidden top-level window that receives broadcasts for the GUI (design m3 A.4, F.5):
//! `TaskbarCreated` (explorer restarted: the tray icon must be re-added), `WM_SETTINGCHANGE`
//! (theme and input-language changes), `WM_QUERYENDSESSION` / `WM_ENDSESSION` (sign-out, shutdown,
//! restart) and power resume.
//!
//! Slint's tray uses a message-only window (`HWND_MESSAGE`), and message-only windows never
//! receive broadcasts, so the app listens itself (M0 #8, prototype README 6). Moved from
//! `prototypes/slint-proto/src/watcher.rs`, which passed the manual checks E and F.

use std::cell::RefCell;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, PBT_APMRESUMEAUTOMATIC, RegisterClassW,
    RegisterWindowMessageW, WINDOW_STYLE, WM_CLOSE, WM_ENDSESSION, WM_POWERBROADCAST,
    WM_QUERYENDSESSION, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::error::Error;
use crate::sys::win32;

/// A broadcast observed by the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// Explorer (re)created the taskbar; tray icons must be re-added.
    TaskbarCreated,
    /// `WM_SETTINGCHANGE` with its `lParam` string (e.g. `ImmersiveColorSet`, `intl`,
    /// `Environment`; empty for a null `lParam`).
    SettingChange(String),
    /// `WM_QUERYENDSESSION`: the session may end. The window answers "yes" (MKLM never blocks
    /// the end of a session; the helper's journal makes every write recoverable, design m2 C).
    QueryEndSession,
    /// `WM_ENDSESSION` with `wParam = TRUE`: the session ends now; the process is terminated
    /// soon after this returns.
    EndSession,
    /// The PC resumed from sleep or hibernation (`PBT_APMRESUMEAUTOMATIC`).
    Resumed,
}

type Sink = Box<dyn Fn(ShellEvent)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = RefCell::new(None);
}

const CLASS_NAME: PCWSTR = w!("SHINDATACENTER.MKLM.ShellWatcher");

fn taskbar_created_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: plain Win32 call with a static NUL-terminated string.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Owns the hidden window. Create and drop it on the UI thread, which pumps its messages (the
/// winit event loop does).
#[derive(Debug)]
pub struct ShellWatcher {
    hwnd: HWND,
}

impl ShellWatcher {
    /// Creates the window. `sink` runs on the UI thread inside the window procedure; it must only
    /// record or schedule work (`slint::invoke_from_event_loop`) and must not re-enter the event
    /// loop. Two exceptions (design m3 F.5): on [`ShellEvent::QueryEndSession`] the sink sets the
    /// running session's cancel flag directly (an atomic, outside any `RefCell`) and may register
    /// the post-reboot RunOnce value (one short HKCU write), because a scheduled closure may not
    /// run before the session ends; on [`ShellEvent::EndSession`], where the process ends right
    /// after the call returns, it may wait a bounded time (3 s) for the session worker.
    pub fn new(sink: impl Fn(ShellEvent) + 'static) -> Result<Self, Error> {
        let _ = taskbar_created_message();
        // SAFETY: querying the module handle of the running executable.
        let hinstance =
            unsafe { GetModuleHandleW(None) }.map_err(|error| win32("GetModuleHandleW", &error))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised; a zero atom only means the class already exists.
        let _ = unsafe { RegisterClassW(&class) };
        SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
        // A never-shown, unowned top-level window (not HWND_MESSAGE) so that broadcasts reach it.
        // SAFETY: the class was registered above; all pointers are static or null.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!("MKLM shell watcher"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance.into()),
                None,
            )
        }
        .map_err(|error| {
            SINK.with(|s| s.borrow_mut().take());
            win32("CreateWindowExW", &error)
        })?;
        Ok(Self { hwnd })
    }
}

impl Drop for ShellWatcher {
    fn drop(&mut self) {
        // SAFETY: the window was created by this struct on this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
        SINK.with(|s| s.borrow_mut().take());
    }
}

fn emit(event: ShellEvent) {
    SINK.with(|s| {
        if let Some(sink) = s.borrow().as_ref() {
            sink(event);
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg != 0 && msg == taskbar_created_message() {
        emit(ShellEvent::TaskbarCreated);
        return LRESULT(0);
    }
    match msg {
        // Only Drop destroys this window (e.g. `taskkill` without /F sends WM_CLOSE to every
        // top-level window; the GUI decides about quitting through its own paths).
        WM_CLOSE => return LRESULT(0),
        WM_SETTINGCHANGE => {
            let area = if lparam.0 == 0 {
                String::new()
            } else {
                // SAFETY: for WM_SETTINGCHANGE a non-zero lParam points to a NUL-terminated
                // UTF-16 string that lives for the duration of the message.
                unsafe { PCWSTR(lparam.0 as *const u16).to_string() }.unwrap_or_default()
            };
            emit(ShellEvent::SettingChange(area));
        }
        WM_QUERYENDSESSION => {
            emit(ShellEvent::QueryEndSession);
            return LRESULT(1);
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                emit(ShellEvent::EndSession);
            }
            return LRESULT(0);
        }
        WM_POWERBROADCAST if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize => {
            emit(ShellEvent::Resumed);
        }
        _ => {}
    }
    // SAFETY: forwarding the original message parameters.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
