//! Console control events (M2): the handler `mklm-cli --in-process` installs so that Ctrl+C,
//! Ctrl+Break and closing the console window turn into "revert now" and the engine can finish
//! writing back before the process ends (design F.2, review C8).
//!
//! Windows calls the handler on a thread of its own. For `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT`
//! and `CTRL_SHUTDOWN_EVENT` the process is terminated as soon as the handler returns (or after
//! about five seconds), so a handler that must let work finish blocks until it is done.

use std::sync::OnceLock;

use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows::core::BOOL;

use crate::error::Error;
use crate::sys::win32;

/// A console control event (`SetConsoleCtrlHandler`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleEvent {
    CtrlC,
    CtrlBreak,
    /// The console window is being closed; the process ends when the handler returns.
    Close,
    Logoff,
    Shutdown,
}

impl ConsoleEvent {
    fn from_code(code: u32) -> Option<Self> {
        match code {
            c if c == CTRL_C_EVENT => Some(Self::CtrlC),
            c if c == CTRL_BREAK_EVENT => Some(Self::CtrlBreak),
            c if c == CTRL_CLOSE_EVENT => Some(Self::Close),
            c if c == CTRL_LOGOFF_EVENT => Some(Self::Logoff),
            c if c == CTRL_SHUTDOWN_EVENT => Some(Self::Shutdown),
            _ => None,
        }
    }

    /// True for the events after which Windows ends the process once the handler returns.
    pub fn ends_process(self) -> bool {
        matches!(self, Self::Close | Self::Logoff | Self::Shutdown)
    }
}

type Handler = Box<dyn Fn(ConsoleEvent) -> bool + Send + Sync>;

/// The one handler of this process.
static HANDLER: OnceLock<Handler> = OnceLock::new();

/// The `PHANDLER_ROUTINE` Windows calls.
extern "system" fn dispatch(code: u32) -> BOOL {
    match (ConsoleEvent::from_code(code), HANDLER.get()) {
        (Some(event), Some(handler)) if handler(event) => TRUE,
        _ => FALSE,
    }
}

/// Installs `handler` for this process's console control events. It returns true when it handled
/// the event (Ctrl+C then does not end the process); false passes the event on to the default
/// handler. Only one handler can be installed per process; a second call fails with
/// `ERROR_ALREADY_EXISTS` and leaves the first in place.
pub fn set_ctrl_handler(
    handler: impl Fn(ConsoleEvent) -> bool + Send + Sync + 'static,
) -> Result<(), Error> {
    /// `ERROR_ALREADY_EXISTS`.
    const ERROR_ALREADY_EXISTS: u32 = 183;
    if HANDLER.set(Box::new(handler)).is_err() {
        return Err(Error::Win32 {
            function: "SetConsoleCtrlHandler",
            code: ERROR_ALREADY_EXISTS,
        });
    }
    let routine: unsafe extern "system" fn(u32) -> BOOL = dispatch;
    // SAFETY: `routine` is an `extern "system"` function with the PHANDLER_ROUTINE signature that
    // lives for the whole process; it only reads the `OnceLock`, which was set above.
    unsafe { SetConsoleCtrlHandler(Some(routine), true) }
        .map_err(|error| win32("SetConsoleCtrlHandler", &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_codes() {
        assert_eq!(ConsoleEvent::from_code(0), Some(ConsoleEvent::CtrlC));
        assert_eq!(ConsoleEvent::from_code(1), Some(ConsoleEvent::CtrlBreak));
        assert_eq!(ConsoleEvent::from_code(2), Some(ConsoleEvent::Close));
        assert_eq!(ConsoleEvent::from_code(5), Some(ConsoleEvent::Logoff));
        assert_eq!(ConsoleEvent::from_code(6), Some(ConsoleEvent::Shutdown));
        assert_eq!(ConsoleEvent::from_code(3), None);
        assert!(ConsoleEvent::Close.ends_process());
        assert!(!ConsoleEvent::CtrlC.ends_process());
    }

    #[test]
    fn unknown_events_and_a_missing_handler_are_passed_on() {
        // Without a handler installed in this test process, every event goes to the default one.
        if HANDLER.get().is_none() {
            assert_eq!(dispatch(0), FALSE);
        }
        assert_eq!(dispatch(3), FALSE);
    }
}
