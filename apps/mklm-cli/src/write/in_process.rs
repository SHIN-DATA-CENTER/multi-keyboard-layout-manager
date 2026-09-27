//! The hidden `--in-process` fallback of `recover`, `undo` and `restore` (design A.7, F.2 step 6,
//! review C8): the engine runs inside this elevated process, for when the helper cannot start.
//!
//! The helper normally isolates the countdown from this console: closing the console breaks the
//! pipe and the helper reverts at once. Here a console control handler plays that part: Ctrl+C,
//! Ctrl+Break and closing the window count as the caller leaving (`check_cancelled` and
//! `wait_decision` say so), and for a close the handler holds the process until the engine has
//! written the values back and flushed the journal (Windows allows about five seconds).

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use mklm_core::{ApplyOptions, ConflictPolicy, ErrorInfo, Event, OperationResult, RestoreScope};
use mklm_engine::win::{WinDevices, WinHost, WinRegistry};
use mklm_engine::{
    DecisionPoll, Engine, EngineConfig, EngineError, EventSink, RestoreBaselineParams, RestoreMode,
};
use mklm_win::console::{self, ConsoleEvent};

use super::input::Input;
use super::relay::Presenter;

/// Set by the console control handler: the user pressed Ctrl+C / Ctrl+Break or closed the window.
static CANCELLED: AtomicBool = AtomicBool::new(false);
/// Set once the engine has returned (its writes are done and flushed).
static DONE: Mutex<bool> = Mutex::new(false);
static FINISHED: Condvar = Condvar::new();

/// How long a close / logoff / shutdown handler holds the process for the engine (Windows ends it
/// after about five seconds anyway).
const CLOSE_GRACE: Duration = Duration::from_millis(4500);
/// Polling slice while waiting for a decision.
const SLICE: Duration = Duration::from_millis(100);
/// Longest wait for restart workers that outlived their deadline before returning (C18).
const JOIN_PENDING_TIMEOUT: Duration = Duration::from_secs(60);

/// What the fallback runs.
#[derive(Debug, Clone)]
pub enum InProcessRequest {
    Recover,
    Undo,
    Restore {
        scope: RestoreScope,
        on_conflict: ConflictPolicy,
    },
}

fn on_console_event(event: ConsoleEvent) -> bool {
    CANCELLED.store(true, Ordering::SeqCst);
    if event.ends_process() {
        // Returning ends the process: let the engine finish writing back first.
        let done = DONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = FINISHED.wait_timeout_while(done, CLOSE_GRACE, |done| !*done);
    }
    true
}

fn mark_done() {
    let mut done = DONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *done = true;
    FINISHED.notify_all();
}

/// Runs `request` with the engine in this process. The caller has checked that the process is
/// elevated and that DLL loading is restricted.
pub fn run(
    request: &InProcessRequest,
    apply: ApplyOptions,
    presenter: Presenter,
    input: &mut dyn Input,
    out: &mut dyn Write,
) -> Result<Result<OperationResult, ErrorInfo>> {
    if !mklm_win::elevation::is_elevated().context("checking for elevation failed")? {
        bail!("--in-process needs an elevated (administrator) console");
    }
    console::set_ctrl_handler(on_console_event)
        .context("could not install the console control handler")?;
    let config = EngineConfig::default();
    let host = match WinHost::new() {
        Ok(host) => host,
        Err(error) => return Ok(Err(EngineError::Host(error).to_info())),
    };
    let devices = WinDevices::new().with_restart_timeout(config.restart_timeout);
    let mut engine = Engine::new(WinRegistry::new(), devices, host, config);
    let mut sink = ConsoleSink {
        presenter,
        input,
        out,
        decisions: VecDeque::new(),
    };
    let result = match request {
        InProcessRequest::Recover => engine.recover(&apply, &mut sink),
        InProcessRequest::Undo => engine.undo_open(&apply, &mut sink),
        InProcessRequest::Restore { scope, on_conflict } => engine.restore_baseline(
            &RestoreBaselineParams {
                scope: scope.clone(),
                on_conflict: *on_conflict,
                mode: RestoreMode::Interactive,
                apply,
            },
            &mut sink,
        ),
    };
    let _ = sink.presenter.finish(sink.out);
    mark_done();
    let (_, mut devices, _) = engine.into_parts();
    let _ = devices.join_pending(JOIN_PENDING_TIMEOUT);
    Ok(result.map_err(|error| error.to_info()))
}

/// Prints events and reads answers from the console; the control handler is the "caller left".
struct ConsoleSink<'a> {
    presenter: Presenter,
    input: &'a mut dyn Input,
    out: &'a mut dyn Write,
    /// Decisions `--answer`-like rules produced while showing an event.
    decisions: VecDeque<mklm_core::Decision>,
}

impl EventSink for ConsoleSink<'_> {
    fn event(&mut self, event: &Event) {
        let decision = self.presenter.event(event, self.out, Instant::now());
        if self.presenter.take_question_asked() {
            self.input.discard_typed_ahead();
        }
        if let Ok(Some(decision)) = decision {
            self.decisions.push_back(decision);
        }
    }

    fn check_cancelled(&mut self) -> bool {
        CANCELLED.load(Ordering::SeqCst)
    }

    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
        if let Some(decision) = self.decisions.pop_front() {
            return DecisionPoll::Decided(decision);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if CANCELLED.load(Ordering::SeqCst) {
                return DecisionPoll::Disconnected;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return DecisionPoll::NoDecision;
            }
            let slice = remaining.min(SLICE);
            if self.presenter.wants_input() {
                if let Some(line) = self.input.next_line(Some(slice))
                    && let Ok(Some(decision)) = self.presenter.line(line, self.out)
                {
                    return DecisionPoll::Decided(decision);
                }
            } else {
                thread::sleep(slice);
            }
            if let Ok(Some(decision)) = self.presenter.tick(Instant::now(), self.out) {
                return DecisionPoll::Decided(decision);
            }
        }
    }
}
