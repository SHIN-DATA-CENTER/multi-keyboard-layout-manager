//! One helper session (design section E): parse the fixed command line, connect to the caller's
//! pipe, handshake, then serve requests with the engine until `Bye`, a closed pipe or the idle
//! timeout.

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(dead_code, unused_variables)]

use std::process::ExitCode;
use std::sync::mpsc::Sender;
use std::time::Duration;

use mklm_core::{Event, OperationResult};
use mklm_engine::{
    DecisionPoll, DeviceController, Engine, EngineError, EventSink, Host, RegistryBackend,
};
use mklm_ipc::{HelperArgs, HelperMessage, Request};

/// Runs the session and maps its end to an exit code (`crate::exit`).
///
/// 1. `SetCurrentDirectoryW(System32)` (the launcher also passes it as `lpDirectory`), then
///    `mklm_ipc::command_line_tail(GetCommandLineW())` → `HelperArgs::parse`
///    (else `BAD_ARGUMENTS`).
/// 2. `mklm_win::elevation::is_elevated()` and the OS build (else `NOT_ELEVATED` /
///    `UNSUPPORTED_OS`).
/// 3. `PipeConnection::connect(args.pipe.path(), args.caller_pid, HANDSHAKE_TIMEOUT)`; the
///    caller's image must be in this executable's directory
///    (`proc_identity::same_image_directory`, which works when the caller runs as another user);
///    send `Hello` (with the build ID), read and verify `Welcome` (else `HANDSHAKE`).
/// 4. Start the writer thread (below). Build `Engine<WinRegistry, WinDevices, WinHost>`; loop:
///    read a `CallerMessage` with `REQUEST_IDLE_TIMEOUT`; `Request` → [`dispatch`] → `Result` or
///    `Error` frame; `Bye` / EOF / timeout → `OK`.
/// 5. Before exiting, `WinDevices::join_pending` (a restart worker that outlived its deadline must
///    finish; design review C18).
///
/// Writer thread (design review S7): the only code that writes to the pipe. It takes frames from a
/// channel ([`PipeSink`] sends into it) and sends `Event::Heartbeat` every `HEARTBEAT_INTERVAL`
/// while a request runs, whatever the engine is blocked in (`restart`, `keyboards`, the lock).
#[cfg(windows)]
pub fn run() -> ExitCode {
    todo!("M2")
}

/// Maps one wire request onto one engine call, explicitly (design review S5). Only what
/// `mklm_ipc::Request` lists is reachable; the silent restore is not (it has its own fixed command
/// line, M5).
pub fn dispatch<R, D, H>(
    engine: &mut Engine<R, D, H>,
    request: &Request,
    sink: &mut dyn EventSink,
) -> Result<OperationResult, EngineError>
where
    R: RegistryBackend,
    D: DeviceController,
    H: Host,
{
    todo!("M2: Request::SetLayout → engine.set_layout(&SetLayoutParams {{ .. }}), …")
}

/// Forwards engine events to the writer thread and reads decisions from the pipe.
///
/// [`EventSink::check_cancelled`] peeks the pipe without blocking (`PeekNamedPipe`:
/// `ERROR_BROKEN_PIPE`, or a `Bye` frame waiting) (design review S4). While the engine waits for a
/// decision, the pipe is read with the given timeout; a closed pipe is
/// [`DecisionPoll::Disconnected`] (the engine then reverts a running countdown, section E.7). A
/// `Request` arriving mid-operation is a protocol error.
#[derive(Debug)]
struct PipeSink {
    frames: Sender<HelperMessage>,
}

impl EventSink for PipeSink {
    fn event(&mut self, event: &Event) {
        todo!("M2")
    }

    fn check_cancelled(&mut self) -> bool {
        todo!("M2")
    }

    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
        todo!("M2")
    }
}

/// Parses the helper's own command line.
fn parse_args(command_line: &str) -> Option<HelperArgs> {
    todo!("M2")
}
