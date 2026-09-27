//! Hidden top-level window that receives shell broadcasts (`TaskbarCreated`, `WM_SETTINGCHANGE`).
//!
//! Slint's tray uses a message-only window (`HWND_MESSAGE`), and message-only windows never receive
//! broadcasts, so the app has to listen for `TaskbarCreated` itself.

use std::cell::RefCell;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, RegisterWindowMessageW,
    WINDOW_STYLE, WM_CLOSE, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

/// A broadcast observed by the watcher window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// Explorer (re)created the taskbar; tray icons must be re-added.
    TaskbarCreated,
    /// `WM_SETTINGCHANGE` with its `lParam` string (e.g. `ImmersiveColorSet`, `Environment`).
    SettingChange(String),
}

type Sink = Box<dyn Fn(ShellEvent)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = RefCell::new(None);
}

const CLASS_NAME: PCWSTR = w!("MklmSlintProtoShellWatcher");

fn taskbar_created_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: plain Win32 call with a static NUL-terminated string.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Owns the hidden watcher window. Must be created and dropped on the UI thread, which pumps its messages.
#[derive(Debug)]
pub struct ShellWatcher {
    hwnd: HWND,
}

impl ShellWatcher {
    /// Creates the window. `sink` runs on the UI thread inside the window procedure; it should only
    /// schedule work (e.g. `slint::invoke_from_event_loop`) and must not re-enter the event loop.
    pub fn new(sink: impl Fn(ShellEvent) + 'static) -> windows::core::Result<Self> {
        let _ = taskbar_created_message();
        // SAFETY: querying the module handle of the running executable.
        let hinstance = unsafe { GetModuleHandleW(None) }?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: `wc` is fully initialized; a zero atom only means the class already exists.
        let _ = unsafe { RegisterClassW(&wc) };
        SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
        // A never-shown, unowned top-level window (not HWND_MESSAGE) so that broadcasts reach it.
        // SAFETY: the class was registered above; all pointers are static or null.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!("MKLM proto shell watcher"),
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
        }?;
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
    if msg == WM_CLOSE {
        // Only Drop destroys this window (e.g. `taskkill` without /F sends WM_CLOSE to every top-level window).
        return LRESULT(0);
    }
    if msg == WM_SETTINGCHANGE {
        let area = if lparam.0 == 0 {
            String::new()
        } else {
            // SAFETY: for WM_SETTINGCHANGE a non-zero lParam points to a NUL-terminated UTF-16 string.
            unsafe { PCWSTR(lparam.0 as *const u16).to_string() }.unwrap_or_default()
        };
        emit(ShellEvent::SettingChange(area));
    }
    // SAFETY: forwarding the original message parameters.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
