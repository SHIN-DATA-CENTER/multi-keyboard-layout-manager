//! One helper session (design section E): parse the fixed command line, connect to the caller's
//! pipe, handshake, then serve requests with the engine until `Bye`, a closed pipe or the idle
//! timeout.

use mklm_core::OperationResult;
use mklm_engine::{
    DeviceController, Engine, EngineError, EventSink, Host, MigrateParams, RegistryBackend,
    ResolveParams, RestoreBaselineParams, RestoreMode, SetLayoutParams,
};
use mklm_ipc::{Request, command_line_tail};

#[cfg(windows)]
pub use windows_session::run;

/// Maps one wire request onto one engine call, explicitly (design review S5). Only what
/// `mklm_ipc::Request` lists is reachable; the silent restore is not (it has its own fixed command
/// line, M5): a pipe restore is always [`RestoreMode::Interactive`].
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
    match request {
        Request::SetLayout(request) => engine.set_layout(
            &SetLayoutParams {
                instance_id: request.instance_id.clone(),
                layout: request.layout,
                apply: request.apply,
                expected: request.expected.clone(),
            },
            sink,
        ),
        Request::Migrate(request) => engine.migrate(
            &MigrateParams {
                standard: request.standard,
                assignments: request
                    .assignments
                    .iter()
                    .map(|a| (a.instance_id.clone(), a.layout))
                    .collect(),
                expected: request.expected.clone(),
            },
            sink,
        ),
        Request::Revert { op_id, apply } => engine.revert(op_id, apply, sink),
        Request::Confirm { op_id } => engine.confirm(op_id, sink),
        Request::RestoreBaseline(request) => engine.restore_baseline(
            &RestoreBaselineParams {
                scope: request.scope.clone(),
                on_conflict: request.on_conflict,
                mode: RestoreMode::Interactive,
                apply: request.apply,
            },
            sink,
        ),
        Request::Recover { apply } => engine.recover(apply, sink),
        Request::Undo { apply } => engine.undo_open(apply, sink),
        Request::ResolveConflict(request) => engine.resolve_conflict(
            &ResolveParams {
                op_id: request.op_id.clone(),
                choices: request.choices.clone(),
                apply: request.apply,
            },
            sink,
        ),
    }
}

/// Parses the helper's own raw command line (design E.4): the program name and one space are
/// dropped by [`command_line_tail`], and the rest must match `HELPER_ARGS_PATTERN` exactly.
fn parse_args(command_line: &str) -> Option<mklm_ipc::HelperArgs> {
    command_line_tail(command_line).and_then(|tail| mklm_ipc::HelperArgs::parse(tail).ok())
}

#[cfg(windows)]
mod windows_session {
    use std::collections::VecDeque;
    use std::process::ExitCode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::thread;
    use std::time::{Duration, Instant};

    use mklm_core::{Decision, ErrorCode, ErrorInfo, Event};
    use mklm_engine::win::{WinDevices, WinHost, WinRegistry};
    use mklm_engine::{DecisionPoll, Engine, EngineConfig, EngineError, EventSink};
    use mklm_ipc::{
        CallerMessage, FrameError, FrameReader, FrameSequencer, HANDSHAKE_TIMEOUT,
        HEARTBEAT_INTERVAL, Hello, HelperArgs, HelperMessage, MESSAGE_TIMEOUT,
        REQUEST_IDLE_TIMEOUT, verify_welcome, write_frame,
    };
    use mklm_win::elevation;
    use mklm_win::pipe::PipeConnection;
    use mklm_win::proc_identity;

    use super::{dispatch, parse_args};
    use crate::exit;

    /// Oldest Windows build MKLM supports (Windows 11 24H2).
    const MIN_BUILD: u32 = 26100;
    /// This build's ID (apps/build_id.rs); the caller requires an exact match (design E.3).
    const BUILD_ID: &str = env!("MKLM_BUILD_ID");
    /// How long [`EventSink::check_cancelled`] waits for the rest of a frame that has started to
    /// arrive.
    const PEEK_READ: Duration = Duration::from_millis(50);
    /// Longest wait for restart workers that outlived their deadline before the process exits
    /// (design review C18): a class-installer call is never cut short by the exit, within reason.
    const JOIN_PENDING_TIMEOUT: Duration = Duration::from_secs(10 * 60);

    type WinEngine = Engine<WinRegistry, WinDevices, WinHost>;

    /// Runs the session and maps its end to an exit code (`crate::exit`).
    ///
    /// 1. `SetCurrentDirectoryW(System32)` (the launcher also passes it as the start directory),
    ///    then `command_line_tail(GetCommandLineW())` → `HelperArgs::parse` (else
    ///    `BAD_ARGUMENTS`).
    /// 2. `is_elevated()` and the OS build (else `NOT_ELEVATED` / `UNSUPPORTED_OS`).
    /// 3. `PipeConnection::connect` (checks the server PID against `--caller-pid`); the caller's
    ///    image must be in this executable's directory (`same_image_directory`, which works when
    ///    the caller runs as another user); send `Hello` (with the build ID), read and verify
    ///    `Welcome` (else `HANDSHAKE`).
    /// 4. Start the writer thread; serve requests: `Request` → [`dispatch`] → `Result` or `Error`;
    ///    `Bye` / closed pipe / `REQUEST_IDLE_TIMEOUT` → `OK`.
    /// 5. Before exiting, `WinDevices::join_pending` (design review C18).
    pub fn run() -> ExitCode {
        ExitCode::from(session())
    }

    fn session() -> u8 {
        // `std::env::set_current_dir` is `SetCurrentDirectoryW`: nothing is resolved against the
        // directory the caller happened to be in (design A.6).
        let Ok(system32) = elevation::system_directory() else {
            return exit::FAILURE;
        };
        if std::env::set_current_dir(&system32).is_err() {
            return exit::FAILURE;
        }
        let Some(args) = parse_args(&elevation::process_command_line()) else {
            return exit::BAD_ARGUMENTS;
        };
        match elevation::is_elevated() {
            Ok(true) => {}
            Ok(false) => return exit::NOT_ELEVATED,
            Err(_) => return exit::FAILURE,
        }
        match mklm_win::read_os_info(&mut Vec::new()) {
            Ok(os) if os.build >= MIN_BUILD => {}
            Ok(_) => return exit::UNSUPPORTED_OS,
            Err(_) => return exit::FAILURE,
        }
        let Some((reader, writer, sequencer, frames_in)) = handshake(&args) else {
            return exit::HANDSHAKE;
        };
        serve(reader, writer, sequencer, frames_in)
    }

    /// Connects and runs the handshake. Returns both ends of the connection, the sequencer of the
    /// frames sent so far and the reader of the frames received so far.
    fn handshake(
        args: &HelperArgs,
    ) -> Option<(PipeConnection, PipeConnection, FrameSequencer, FrameReader)> {
        let mut pipe =
            PipeConnection::connect(&args.pipe.path(), args.caller_pid, HANDSHAKE_TIMEOUT).ok()?;
        // The server PID matched; its image must be ours (same directory; M5: the install
        // location), even when the caller runs as another user (design E.2, review S3).
        if !proc_identity::same_image_directory(args.caller_pid).unwrap_or(false) {
            return None;
        }
        pipe.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        pipe.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
        let mut sequencer = FrameSequencer::new();
        let hello = Hello::new(
            &args.nonce,
            std::process::id(),
            env!("CARGO_PKG_VERSION"),
            BUILD_ID,
        );
        write_frame(&mut pipe, &sequencer.frame(HelperMessage::Hello(hello))).ok()?;
        let mut frames_in = FrameReader::new();
        let welcome = match frames_in.read::<_, CallerMessage>(&mut pipe).ok()?.body {
            CallerMessage::Welcome(welcome) => welcome,
            _ => return None,
        };
        verify_welcome(&welcome, args.caller_pid, BUILD_ID).ok()?;
        let mut writer = pipe.try_clone().ok()?;
        // A caller that stops reading must not block the writer forever; a timed-out write ends
        // the session's output (a partly written frame cannot be continued).
        writer.set_write_timeout(Some(MESSAGE_TIMEOUT));
        Some((pipe, writer, sequencer, frames_in))
    }

    /// Serves requests until the caller leaves; returns the exit code.
    fn serve(
        reader: PipeConnection,
        writer: PipeConnection,
        sequencer: FrameSequencer,
        frames_in: FrameReader,
    ) -> u8 {
        let busy = Arc::new(AtomicBool::new(false));
        let (frames, outbox) = mpsc::channel::<HelperMessage>();
        let writer_thread = {
            let busy = Arc::clone(&busy);
            thread::spawn(move || write_loop(writer, sequencer, &outbox, &busy))
        };
        let mut inbox = Inbox {
            pipe: reader,
            reader: frames_in,
            decisions: VecDeque::new(),
            gone: false,
        };
        let mut engine: Option<WinEngine> = None;
        let mut code = exit::OK;
        loop {
            let message = match inbox.read(REQUEST_IDLE_TIMEOUT) {
                Ok(message) => message,
                // Closed by the caller, or idle for too long: a normal end.
                Err(FrameError::Closed | FrameError::Timeout) => break,
                Err(_) => {
                    code = exit::FAILURE;
                    break;
                }
            };
            match message {
                CallerMessage::Request(request) => {
                    busy.store(true, Ordering::SeqCst);
                    let reply = match engine_for(&mut engine) {
                        Ok(engine) => {
                            let mut sink = PipeSink::new(&frames, &mut inbox);
                            match dispatch(engine, &request, &mut sink) {
                                Ok(result) => HelperMessage::Result(result),
                                Err(error) => HelperMessage::Error(error.to_info()),
                            }
                        }
                        Err(info) => HelperMessage::Error(*info),
                    };
                    busy.store(false, Ordering::SeqCst);
                    // A writer that failed has dropped its receiver; nothing more can be said.
                    let _ = frames.send(reply);
                    if inbox.gone {
                        break;
                    }
                }
                CallerMessage::Bye => break,
                // A late answer to a countdown that already ended.
                CallerMessage::Decision(_) => {}
                CallerMessage::Welcome(_) => {
                    let _ = frames.send(HelperMessage::Error(protocol_error(
                        "a second welcome frame",
                    )));
                    code = exit::FAILURE;
                    break;
                }
            }
        }
        drop(frames);
        let _ = writer_thread.join();
        if let Some(engine) = engine {
            let (_, mut devices, _) = engine.into_parts();
            // Design review C18: a restart worker that outlived its deadline must finish before
            // the process exits under it.
            let _ = devices.join_pending(JOIN_PENDING_TIMEOUT);
        }
        code
    }

    /// The engine, created on the first request. A host that cannot be set up (e.g. the
    /// protected directory cannot be validated) fails each request instead of the session.
    fn engine_for(engine: &mut Option<WinEngine>) -> Result<&mut WinEngine, Box<ErrorInfo>> {
        if engine.is_none() {
            let config = EngineConfig::default();
            let host =
                WinHost::new().map_err(|error| Box::new(EngineError::Host(error).to_info()))?;
            let devices = WinDevices::new().with_restart_timeout(config.restart_timeout);
            *engine = Some(Engine::new(WinRegistry::new(), devices, host, config));
        }
        engine
            .as_mut()
            .ok_or_else(|| Box::new(protocol_error("no engine")))
    }

    fn protocol_error(message: &str) -> ErrorInfo {
        ErrorInfo {
            code: ErrorCode::Protocol,
            message: message.to_string(),
            op_id: None,
            plan_error: None,
        }
    }

    /// The writer thread (design review S7): the only code that writes to the pipe. Sends the
    /// frames it receives in order, and `Event::Heartbeat` every `HEARTBEAT_INTERVAL` while a
    /// request runs, whatever the engine is blocked in (`restart`, `keyboards`, the lock).
    fn write_loop(
        mut pipe: PipeConnection,
        mut sequencer: FrameSequencer,
        outbox: &Receiver<HelperMessage>,
        busy: &AtomicBool,
    ) {
        let mut next_heartbeat = Instant::now() + HEARTBEAT_INTERVAL;
        loop {
            let wait = next_heartbeat.saturating_duration_since(Instant::now());
            let message = match outbox.recv_timeout(wait) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => {
                    next_heartbeat = Instant::now() + HEARTBEAT_INTERVAL;
                    if !busy.load(Ordering::SeqCst) {
                        continue;
                    }
                    HelperMessage::Event(Event::Heartbeat)
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            if write_frame(&mut pipe, &sequencer.frame(message)).is_err() {
                // The caller is gone or stopped reading. The reading side notices on its own;
                // keep draining the channel so that senders never block.
                while outbox.recv().is_ok() {}
                return;
            }
        }
    }

    /// The receiving side of the pipe: frames arrive here both between requests and while the
    /// engine runs (decisions, `Bye`).
    #[derive(Debug)]
    struct Inbox {
        pipe: PipeConnection,
        reader: FrameReader,
        /// Decisions that arrived while the engine was not waiting for one.
        decisions: VecDeque<Decision>,
        /// The caller is gone: the pipe closed, `Bye` arrived, or it broke the protocol.
        gone: bool,
    }

    impl Inbox {
        fn read(&mut self, timeout: Duration) -> Result<CallerMessage, FrameError> {
            self.pipe.set_read_timeout(Some(timeout));
            self.reader
                .read::<_, CallerMessage>(&mut self.pipe)
                .map(|frame| frame.body)
        }

        /// Reads at most one frame within `timeout` while a request runs. Returns a decision;
        /// records `Bye`, a closed pipe and protocol violations (a request or welcome in the
        /// middle of a request) in `gone`.
        fn poll(&mut self, timeout: Duration) -> Option<Decision> {
            if self.gone {
                return None;
            }
            match self.read(timeout) {
                Ok(CallerMessage::Decision(decision)) => Some(decision),
                Ok(CallerMessage::Bye | CallerMessage::Request(_) | CallerMessage::Welcome(_)) => {
                    self.gone = true;
                    None
                }
                Err(FrameError::Timeout) => None,
                Err(_) => {
                    self.gone = true;
                    None
                }
            }
        }
    }

    /// Forwards engine events to the writer thread and reads decisions from the pipe.
    ///
    /// [`EventSink::check_cancelled`] peeks the pipe without blocking (`PeekNamedPipe`): a closed
    /// pipe, or a `Bye` frame waiting, means the caller is gone (design review S4). While the
    /// engine waits for a decision, the pipe is read with the given timeout; a closed pipe is
    /// [`DecisionPoll::Disconnected`] (the engine then reverts a running countdown, section E.7).
    /// A `Request` arriving mid-operation is a protocol error and counts as the caller leaving.
    #[derive(Debug)]
    struct PipeSink<'a> {
        frames: &'a Sender<HelperMessage>,
        inbox: &'a mut Inbox,
        /// Debug builds: whether the R5 pause already happened.
        #[cfg(debug_assertions)]
        paused: bool,
    }

    impl<'a> PipeSink<'a> {
        fn new(frames: &'a Sender<HelperMessage>, inbox: &'a mut Inbox) -> Self {
            Self {
                frames,
                inbox,
                #[cfg(debug_assertions)]
                paused: false,
            }
        }

        /// Debug builds only (design H.2 R5): with `MKLM_DEBUG_PAUSE=after-first-write` in the
        /// helper's environment, stop for a while after the first step is written and flushed,
        /// so that a tester can kill the process there. The writer thread keeps sending
        /// heartbeats meanwhile. (The environment reaches the helper when an elevated console
        /// starts it with `CreateProcessW`; a UAC launch starts it with the user's default one.)
        #[cfg(debug_assertions)]
        fn debug_pause(&mut self, event: &Event) {
            const PAUSE: Duration = Duration::from_secs(120);
            if self.paused || !matches!(event, Event::StepWritten { .. }) {
                return;
            }
            self.paused = true;
            if std::env::var("MKLM_DEBUG_PAUSE").as_deref() == Ok("after-first-write") {
                thread::sleep(PAUSE);
            }
        }
    }

    impl EventSink for PipeSink<'_> {
        fn event(&mut self, event: &Event) {
            let _ = self.frames.send(HelperMessage::Event(event.clone()));
            #[cfg(debug_assertions)]
            self.debug_pause(event);
        }

        fn check_cancelled(&mut self) -> bool {
            if self.inbox.gone {
                return true;
            }
            match self.inbox.pipe.bytes_available() {
                Ok(0) => {}
                Ok(_) => {
                    // A frame is (at least partly) waiting: a decision is kept for later, a
                    // `Bye` means the caller left.
                    if let Some(decision) = self.inbox.poll(PEEK_READ) {
                        self.inbox.decisions.push_back(decision);
                    }
                }
                Err(_) => self.inbox.gone = true,
            }
            self.inbox.gone
        }

        fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
            if let Some(decision) = self.inbox.decisions.pop_front() {
                return DecisionPoll::Decided(decision);
            }
            let deadline = Instant::now() + timeout;
            loop {
                if self.inbox.gone {
                    return DecisionPoll::Disconnected;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return DecisionPoll::NoDecision;
                }
                // At least 1 ms, so that a sub-millisecond rest is a real wait, not a poll.
                if let Some(decision) = self.inbox.poll(remaining.max(Duration::from_millis(1))) {
                    return DecisionPoll::Decided(decision);
                }
            }
        }
    }
}
