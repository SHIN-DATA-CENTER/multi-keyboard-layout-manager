//! Checks both sides make before the first request, and the session timeouts.
//!
//! Order (section E.3 of the design doc):
//! 1. Caller: pipe created with `FILE_FLAG_FIRST_PIPE_INSTANCE`; helper launched; the connecting
//!    client's PID (`GetNamedPipeClientProcessId`) must be the launched helper's PID.
//! 2. Helper: the server's PID (`GetNamedPipeServerProcessId`) must be `--caller-pid`, and that
//!    process's image must sit in the helper's own directory; then it sends [`Hello`].
//! 3. Caller: [`verify_hello`]; sends [`Welcome`].
//! 4. Helper: [`verify_welcome`].

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use std::time::Duration;

use crate::args::Nonce;
use crate::message::{Hello, Welcome};

/// Caller: how long to wait for the helper to connect (covers the UAC prompt).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
/// Both: how long each handshake frame may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Helper: how long to wait for the next request (or `Bye`) before exiting.
pub const REQUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Caller: longest silence while a request runs. The helper's writer thread sends
/// [`crate::Event::Heartbeat`] every [`HEARTBEAT_INTERVAL`] whatever the engine is doing, so only a
/// wedged helper process stays silent this long (design review S7); a helper process that exits
/// ends the wait at once (the caller holds its process handle).
pub const MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Helper: interval of the writer thread's heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// Helper: longest wait for a [`crate::Decision`] outside a countdown (reconnect path).
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(600);

/// A handshake check failed; the session ends without any request being read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    #[error("protocol version {found}, expected {expected}")]
    Version { found: u32, expected: u32 },
    #[error("the helper did not present the expected nonce")]
    Nonce,
    #[error("peer PID {found}, expected {expected}")]
    Pid { expected: u32, found: u32 },
    #[error("peer build {found:?}, expected {expected:?}; reinstall or rebuild both executables")]
    Build { expected: String, found: String },
}

/// Caller side: version, build ID (exact), nonce (constant time) and `helper_pid` = the PID the
/// caller launched and saw connect.
pub fn verify_hello(
    hello: &Hello,
    nonce: &Nonce,
    launched_pid: u32,
    build_id: &str,
) -> Result<(), HandshakeError> {
    todo!("M2")
}

/// Helper side: version, build ID (exact) and `caller_pid` = `--caller-pid`.
pub fn verify_welcome(
    welcome: &Welcome,
    caller_pid: u32,
    build_id: &str,
) -> Result<(), HandshakeError> {
    todo!("M2")
}
