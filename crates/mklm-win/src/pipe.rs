//! The helper pipe (M2, plan 2.2, design section E).
//!
//! The unelevated caller is the server: it creates `\\.\pipe\SHINDATACENTER.MKLM.<uuid>` with
//! `PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED`,
//! `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS`, one instance and
//! the DACL [`HELPER_PIPE_SDDL`], then launches the helper. The helper connects with
//! `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`, so the unelevated server can never
//! impersonate it. Both sides check the other's PID. Reads and writes are overlapped with
//! timeouts (`CancelIoEx` on expiry).

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::io::{self, Read, Write};
use std::os::windows::io::OwnedHandle;
use std::time::Duration;

use crate::elevation::ElevatedProcess;
use crate::error::Error;

/// DACL of the helper pipe: SYSTEM and Administrators, and `OWNER RIGHTS` limited to
/// `READ_CONTROL`. The interactive user's SID is left out (design section E.2), and the
/// `OWNER RIGHTS` ACE removes the owner's implicit `WRITE_DAC`, which would otherwise let a
/// process of the same user open the pipe (design review S10). This narrows who can connect; the
/// actual defence is the PID check on both ends ([`PipeServer::accept`], [`PipeConnection::connect`]).
pub const HELPER_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;RC;;;OW)";

/// A pipe created by the caller, waiting for the helper.
#[derive(Debug)]
pub struct PipeServer {
    handle: OwnedHandle,
    path: String,
}

impl PipeServer {
    /// Creates the only instance of `path` (`\\.\pipe\…`) with `sddl`. Fails if the name exists.
    pub fn create(path: &str, sddl: &str) -> Result<Self, Error> {
        todo!("M2")
    }

    /// Waits for a client up to `timeout`, giving up early when `helper` exits (UAC declined or the
    /// helper failed). The client's PID (`GetNamedPipeClientProcessId`) must be
    /// `expected_client_pid`; any other client is dropped (`DisconnectNamedPipe`) and the wait
    /// goes on until `timeout`, so that a process that connects first cannot make the session fail
    /// (design review S10). [`Error::PeerMismatch`] only if the time runs out after a mismatch.
    pub fn accept(
        self,
        expected_client_pid: u32,
        timeout: Duration,
        helper: Option<&ElevatedProcess>,
    ) -> Result<PipeConnection, Error> {
        todo!("M2")
    }
}

/// One connected end of the pipe.
#[derive(Debug)]
pub struct PipeConnection {
    handle: OwnedHandle,
    peer_pid: u32,
    read_timeout: Option<Duration>,
}

impl PipeConnection {
    /// Helper side: connects with SQOS identification and checks the server's PID
    /// (`GetNamedPipeServerProcessId`) against `expected_server_pid`. `ERROR_PIPE_BUSY` (another
    /// client holds the only instance until the server drops it) is retried with `WaitNamedPipeW`
    /// for up to `timeout`.
    pub fn connect(path: &str, expected_server_pid: u32, timeout: Duration) -> Result<Self, Error> {
        todo!("M2")
    }

    /// PID of the other end, as checked at connect time.
    pub fn peer_pid(&self) -> u32 {
        self.peer_pid
    }

    /// `None` waits forever. An expired read fails with `io::ErrorKind::TimedOut`.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.read_timeout = timeout;
    }
}

impl Read for PipeConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        todo!("M2")
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        todo!("M2")
    }

    fn flush(&mut self) -> io::Result<()> {
        todo!("M2: FlushFileBuffers")
    }
}
