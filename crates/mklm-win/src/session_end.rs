//! The end of the Windows session, seen from the elevated helper (design m3 A.5 WP-E3, F.5,
//! J.18): a sign-out, shutdown or restart may terminate the helper while a keep-or-revert
//! countdown runs, before its pipe closes. The helper therefore
//! - asks to be shut down after its callers ([`shut_down_after_callers`]:
//!   `SetProcessShutdownParameters(0x100, SHUTDOWN_NORETRY)`; the GUI keeps the default 0x280, and
//!   higher levels are notified first), and
//! - listens itself ([`SessionEndWindow`]): a hidden top-level window on its own thread receives
//!   `WM_QUERYENDSESSION` / `WM_ENDSESSION`, so that the helper reverts a running countdown before
//!   it is terminated (the policy is `mklm_engine::SessionEnd`; this module only delivers the
//!   messages).
//!
//! The window never blocks the end of the session: it answers `TRUE` to `WM_QUERYENDSESSION`
//! after the handler returns. The handler runs on the window's thread and may wait a bounded time
//! (the engine's revert on the request thread goes on meanwhile).
//!
//! Not part of the GUI's `ui` module: the helper links no UI toolkit and does not enable the `gui`
//! feature. Only `Win32_UI_WindowsAndMessaging` is used, like the GUI's shell window.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::SetProcessShutdownParameters;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::error::Error;
use crate::sys::win32;

/// Shutdown level of the helper: the lowest of the range applications may use (0x100-0x3FF).
/// Processes with a higher level are shut down first; the GUI and the CLI keep the default 0x280,
/// so the helper is asked (and terminated) after them.
pub const HELPER_SHUTDOWN_LEVEL: u32 = 0x100;
/// `SHUTDOWN_NORETRY` (`Win32_System_WindowsProgramming`, not enabled for one constant): no retry
/// dialog if the process is slow to end; it is simply ended.
const SHUTDOWN_NORETRY: u32 = 0x1;

/// `SetProcessShutdownParameters(HELPER_SHUTDOWN_LEVEL, SHUTDOWN_NORETRY)` for this process.
pub fn shut_down_after_callers() -> Result<(), Error> {
    // SAFETY: a plain Win32 call about the current process, without pointers.
    unsafe { SetProcessShutdownParameters(HELPER_SHUTDOWN_LEVEL, SHUTDOWN_NORETRY) }
        .map_err(|error| win32("SetProcessShutdownParameters", &error))
}

/// A session-end message, as the window received it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEndEvent {
    /// `WM_QUERYENDSESSION`: the session is about to end (sign-out, shutdown, restart). The
    /// window answers `TRUE` once the handler returns.
    QueryEndSession,
    /// `WM_ENDSESSION`: `ending` is its `wParam`. True: the session ends now, and the process may
    /// be terminated as soon as the handler returns. False: another application refused, the
    /// session goes on.
    EndSession { ending: bool },
}

type Handler = Box<dyn Fn(SessionEndEvent)>;

thread_local! {
    /// The handler of the window on this thread (one window per thread).
    static HANDLER: RefCell<Option<Handler>> = RefCell::new(None);
}

const CLASS_NAME: PCWSTR = w!("SHINDATACENTER.MKLM.SessionEnd");
/// Private message: destroy the window and end its thread. `WM_CLOSE` is ignored (e.g. `taskkill`
/// without /F sends it to every top-level window), so only the owner ends it.
const WM_APP_QUIT: u32 = WM_APP + 1;

/// A hidden top-level window on its own message thread that reports session-end messages to a
/// handler. Dropping it destroys the window and joins the thread.
#[derive(Debug)]
pub struct SessionEndWindow {
    /// The window handle as an integer (`HWND` is not `Send`); only posted to.
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl SessionEndWindow {
    /// Starts the thread and creates the window. `handler` runs on that thread, inside the
    /// window procedure; it may block for a bounded time (the session waits for it).
    pub fn spawn(handler: impl Fn(SessionEndEvent) + Send + 'static) -> Result<Self, Error> {
        let (ready, created) = mpsc::channel::<Result<isize, Error>>();
        let thread = thread::Builder::new()
            .name("mklm-session-end".into())
            .spawn(move || run(Box::new(handler), &ready))
            .map_err(|_| Error::Win32 {
                function: "CreateThread",
                code: 0,
            })?;
        match created.recv() {
            Ok(Ok(hwnd)) => Ok(Self {
                hwnd,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Win32 {
                    function: "CreateWindowExW",
                    code: 0,
                })
            }
        }
    }

    /// The window, for tests that send it messages.
    #[cfg(test)]
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }
}

// ---- M5b: the update runner (design m5b D.7; RELIABILITY-3; WP-H) ----

/// H2: the highest application level, so that it hears of the session end first.
pub const RUNNER_SHUTDOWN_LEVEL: u32 = 0x3FF;

/// `SetProcessShutdownParameters(RUNNER_SHUTDOWN_LEVEL, SHUTDOWN_NORETRY)`.
pub fn shut_down_first() -> Result<(), Error> {
    Err(Error::Win32 {
        function: "shut_down_first (m5b skeleton)",
        code: 50,
    }) // Skeleton (M5b): WP-H
}

/// The update runner's answer to `WM_QUERYENDSESSION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryAnswer {
    Allow,
    Block,
}

impl SessionEndWindow {
    /// Like `spawn`, but the handler's answer to `QueryEndSession` is returned to Windows.
    #[allow(unused_variables)] // Skeleton (M5b)
    pub fn spawn_with_answer(
        handler: impl Fn(SessionEndEvent) -> QueryAnswer + Send + 'static,
    ) -> Result<SessionEndWindow, Error> {
        Err(Error::Win32 {
            function: "SessionEndWindow::spawn_with_answer (m5b skeleton)",
            code: 50,
        }) // Skeleton (M5b): WP-H
    }

    /// `ShutdownBlockReasonCreate` / `Destroy` on the window's own thread (posted to it).
    #[allow(unused_variables)] // Skeleton (M5b)
    pub fn set_block_reason(&self, reason: Option<&str>) -> Result<(), Error> {
        Err(Error::Win32 {
            function: "SessionEndWindow::set_block_reason (m5b skeleton)",
            code: 50,
        }) // Skeleton (M5b): WP-H
    }
}

impl Drop for SessionEndWindow {
    fn drop(&mut self) {
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        // SAFETY: posting a message to a window handle is safe even if the window is gone (the
        // call then fails); no pointers are passed.
        let posted = unsafe { PostMessageW(Some(hwnd), WM_APP_QUIT, WPARAM(0), LPARAM(0)) };
        if posted.is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

/// The window thread: registers the handler, creates the window, reports it, pumps messages until
/// the window is destroyed.
fn run(handler: Handler, ready: &mpsc::Sender<Result<isize, Error>>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
    match create_window() {
        Ok(hwnd) => {
            let _ = ready.send(Ok(hwnd.0 as isize));
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            HANDLER.with(|h| h.borrow_mut().take());
            return;
        }
    }
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid out pointer; the loop ends on WM_QUIT (0) or an error (-1).
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: `msg` was filled by GetMessageW on this thread.
        unsafe { DispatchMessageW(&msg) };
    }
    HANDLER.with(|h| h.borrow_mut().take());
}

fn create_window() -> Result<HWND, Error> {
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
    // A never-shown, unowned top-level window (not HWND_MESSAGE): only top-level windows receive
    // the session-end messages.
    // SAFETY: the class was registered above; all pointers are static or null.
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!("MKLM session end"),
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
    .map_err(|error| win32("CreateWindowExW", &error))
}

fn emit(event: SessionEndEvent) {
    HANDLER.with(|h| {
        if let Some(handler) = h.borrow().as_ref() {
            handler(event);
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => {
            emit(SessionEndEvent::QueryEndSession);
            // Never block the end of the session (design m3 F.5).
            return LRESULT(1);
        }
        WM_ENDSESSION => {
            emit(SessionEndEvent::EndSession {
                ending: wparam.0 != 0,
            });
            return LRESULT(0);
        }
        WM_CLOSE => return LRESULT(0),
        WM_APP_QUIT => {
            // SAFETY: the window belongs to this thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwarding the original message parameters.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use windows::Win32::UI::WindowsAndMessaging::{ENDSESSION_LOGOFF, SendMessageW};

    use super::*;

    /// Fake session-end messages sent to the helper's window (design m3 H.1, WP-E3): the handler
    /// sees each, `WM_QUERYENDSESSION` is answered `TRUE`, and `WM_CLOSE` does not end the window.
    /// Nothing here ends a session: the messages only reach this process's own window.
    #[test]
    fn session_end_messages_reach_the_handler() {
        let seen: Arc<Mutex<Vec<SessionEndEvent>>> = Arc::default();
        let log = Arc::clone(&seen);
        let window = SessionEndWindow::spawn(move |event| {
            if let Ok(mut log) = log.lock() {
                log.push(event);
            }
        })
        .expect("the hidden window");
        let hwnd = window.hwnd();
        // SAFETY: a window of this process; SendMessageW waits for its thread to handle them.
        let answers = unsafe {
            [
                SendMessageW(
                    hwnd,
                    WM_QUERYENDSESSION,
                    Some(WPARAM(0)),
                    Some(LPARAM(ENDSESSION_LOGOFF as isize)),
                ),
                SendMessageW(hwnd, WM_CLOSE, None, None),
                SendMessageW(
                    hwnd,
                    WM_ENDSESSION,
                    Some(WPARAM(0)),
                    Some(LPARAM(ENDSESSION_LOGOFF as isize)),
                ),
                SendMessageW(
                    hwnd,
                    WM_ENDSESSION,
                    Some(WPARAM(1)),
                    Some(LPARAM(ENDSESSION_LOGOFF as isize)),
                ),
            ]
        };
        assert_eq!(
            answers[0],
            LRESULT(1),
            "WM_QUERYENDSESSION is never refused"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                SessionEndEvent::QueryEndSession,
                SessionEndEvent::EndSession { ending: false },
                SessionEndEvent::EndSession { ending: true },
            ]
        );
        drop(window);
    }

    /// The GUI keeps the default level 0x280; a lower level is shut down later, and 0x100 is the
    /// lowest one applications may use.
    const _: () = assert!(HELPER_SHUTDOWN_LEVEL < 0x280 && HELPER_SHUTDOWN_LEVEL >= 0x100);
    const _: () = assert!(SHUTDOWN_NORETRY == 1);
}
