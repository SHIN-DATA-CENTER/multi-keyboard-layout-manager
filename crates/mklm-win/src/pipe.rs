//! The helper pipe (M2, plan 2.2, design section E).
//!
//! The unelevated caller is the server: it creates `\\.\pipe\SHINDATACENTER.MKLM.<uuid>` with
//! `PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED`,
//! `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS`, one instance and
//! the DACL [`HELPER_PIPE_SDDL`], then launches the helper. The helper connects with
//! `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`, so the unelevated server can never
//! impersonate it. Both sides check the other's PID. Reads and writes are overlapped with
//! timeouts (`CancelIoEx` on expiry, then waiting for the cancelled I/O to finish, so that no
//! buffer is released while the kernel may still write to it).

use std::io::{self, Read, Write};
use std::os::windows::io::OwnedHandle;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND,
    ERROR_INVALID_NAME, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    ERROR_SEM_TIMEOUT, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
    FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{
    CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_WAIT, PeekNamedPipe, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, WaitForMultipleObjects,
};
use windows::core::PCWSTR;

use crate::elevation::ElevatedProcess;
use crate::error::{Error, win32_code};
use crate::props::to_wide;
use crate::security::LocalSd;
use crate::sys::{last_error, own, raw, wait_millis, win32};

/// DACL of the helper pipe: SYSTEM and Administrators, and `OWNER RIGHTS` limited to
/// `READ_CONTROL`. The interactive user's SID is left out (design section E.2), and the
/// `OWNER RIGHTS` ACE removes the owner's implicit `WRITE_DAC`, which would otherwise let a
/// process of the same user open the pipe (design review S10). This narrows who can connect; the
/// actual defence is the PID check on both ends ([`PipeServer::accept`], [`PipeConnection::connect`]).
pub const HELPER_PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;RC;;;OW)";

/// Every pipe path starts with this.
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// In and out buffer size of the pipe (design E.2).
const BUFFER_SIZE: u32 = 64 * 1024;

/// Pause between two connection attempts while the only instance is busy.
const BUSY_RETRY: Duration = Duration::from_millis(20);

/// A pipe created by the caller, waiting for the helper.
#[derive(Debug)]
pub struct PipeServer {
    handle: OwnedHandle,
    path: String,
}

/// `path` must be `\\.\pipe\<name>` with a non-empty name without `\` or NUL.
fn check_pipe_path(path: &str) -> Result<(), Error> {
    let valid = path
        .strip_prefix(PIPE_PREFIX)
        .is_some_and(|name| !name.is_empty() && name.len() <= 200 && !name.contains(['\\', '\0']));
    if valid {
        Ok(())
    } else {
        Err(Error::Win32 {
            function: "pipe path",
            code: ERROR_INVALID_NAME.0,
        })
    }
}

/// A manual-reset event for overlapped I/O.
fn new_event() -> Result<OwnedHandle, Error> {
    // SAFETY: default security, manual reset, initially non-signalled, unnamed.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map_err(|error| win32("CreateEventW", &error))?;
    // SAFETY: CreateEventW succeeded, so `event` is an open handle this process owns.
    Ok(unsafe { own(event) })
}

impl PipeServer {
    /// Creates the only instance of `path` (`\\.\pipe\…`) with `sddl`. Fails if the name exists.
    pub fn create(path: &str, sddl: &str) -> Result<Self, Error> {
        check_pipe_path(path)?;
        let sd = LocalSd::from_sddl(sddl)?;
        let attributes = sd.attributes();
        let name = to_wide(path);
        // SAFETY: `name` is NUL-terminated; `attributes` points at `sd`; both outlive the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                Some(&attributes),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            return Err(last_error("CreateNamedPipeW"));
        }
        Ok(Self {
            // SAFETY: CreateNamedPipeW succeeded, so `handle` is an open handle this process owns.
            handle: unsafe { own(handle) },
            path: path.to_string(),
        })
    }

    /// The `\\.\pipe\…` path the pipe was created with.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Waits for a client up to `timeout`, giving up early when `helper` exits (UAC declined or the
    /// helper failed). The client's PID (`GetNamedPipeClientProcessId`) must be
    /// `expected_client_pid`; any other client is dropped (`DisconnectNamedPipe`) and the wait
    /// goes on until `timeout`, so that a process that connects first cannot make the session fail
    /// (design review S10). [`Error::PeerMismatch`] only if the time runs out after a mismatch.
    ///
    /// Other outcomes: [`Error::Timeout`] when nobody connected in time, and
    /// [`Error::PeerExited`] when `helper` exited first (its exit code tells why, design E.8).
    pub fn accept(
        self,
        expected_client_pid: u32,
        timeout: Duration,
        helper: Option<&ElevatedProcess>,
    ) -> Result<PipeConnection, Error> {
        let deadline = Instant::now() + timeout;
        let event = new_event()?;
        let handle = raw(&self.handle);
        let mut mismatch: Option<u32> = None;
        loop {
            match self.wait_for_client(&event, deadline, helper)? {
                Connect::Connected => {}
                Connect::Gone if Instant::now() < deadline => continue,
                Connect::Gone | Connect::TimedOut => {
                    return Err(match mismatch {
                        Some(found) => Error::PeerMismatch {
                            expected: expected_client_pid,
                            found,
                        },
                        None => Error::Timeout {
                            operation: "ConnectNamedPipe",
                        },
                    });
                }
                Connect::HelperExited { pid, exit_code } => {
                    return Err(Error::PeerExited { pid, exit_code });
                }
            }
            let mut client = 0u32;
            // SAFETY: `handle` is the connected server end; `client` is a valid out pointer.
            let known = unsafe { GetNamedPipeClientProcessId(handle, &mut client) }.is_ok();
            if known && client == expected_client_pid {
                return PipeConnection::new(self.handle, client);
            }
            // Someone else: drop it and wait again (an unknown PID counts as someone else).
            mismatch = Some(if known { client } else { 0 });
            // SAFETY: `handle` is our server end; disconnecting discards the other client.
            unsafe { DisconnectNamedPipe(handle) }
                .map_err(|error| win32("DisconnectNamedPipe", &error))?;
        }
    }

    /// One `ConnectNamedPipe`, waited for until `deadline` or the helper's exit.
    fn wait_for_client(
        &self,
        event: &OwnedHandle,
        deadline: Instant,
        helper: Option<&ElevatedProcess>,
    ) -> Result<Connect, Error> {
        let handle = raw(&self.handle);
        let mut overlapped = OVERLAPPED {
            hEvent: raw(event),
            ..Default::default()
        };
        // SAFETY: `handle` is an overlapped server end; `overlapped` and its event live until the
        // operation has completed or been cancelled and waited for below.
        match unsafe { ConnectNamedPipe(handle, Some(&mut overlapped)) } {
            Ok(()) => return Ok(Connect::Connected),
            Err(error) => match win32_code(&error) {
                code if code == ERROR_PIPE_CONNECTED.0 => return Ok(Connect::Connected),
                code if code == ERROR_IO_PENDING.0 => {}
                // A client connected and left again before we looked: take the next one.
                code if is_gone(code) => {
                    // SAFETY: `handle` is our server end.
                    unsafe { DisconnectNamedPipe(handle) }
                        .map_err(|error| win32("DisconnectNamedPipe", &error))?;
                    return Ok(Connect::Gone);
                }
                _ => return Err(win32("ConnectNamedPipe", &error)),
            },
        }
        let mut handles = vec![raw(event)];
        if let Some(helper) = helper {
            handles.push(helper.raw_handle());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        // SAFETY: every handle in `handles` is open (the event and the helper process).
        let woke = unsafe { WaitForMultipleObjects(&handles, false, wait_millis(remaining)) };
        // Taken now: the cancellation below overwrites the thread's last error.
        let wait_error = (woke == WAIT_FAILED).then(|| last_error("WaitForMultipleObjects"));
        if woke == WAIT_OBJECT_0 {
            let mut ignored = 0u32;
            // SAFETY: the operation completed (its event is signalled); `overlapped` is the one
            // it used.
            return match unsafe { GetOverlappedResult(handle, &overlapped, &mut ignored, false) } {
                Ok(()) => Ok(Connect::Connected),
                // The client has already left: listen again rather than fail the session.
                Err(error) if is_gone(win32_code(&error)) => {
                    // SAFETY: `handle` is our server end.
                    unsafe { DisconnectNamedPipe(handle) }
                        .map_err(|error| win32("DisconnectNamedPipe", &error))?;
                    Ok(Connect::Gone)
                }
                Err(error) => Err(win32("ConnectNamedPipe", &error)),
            };
        }
        // Time is up or the helper exited: cancel, and wait for the cancellation before
        // `overlapped` goes out of scope.
        // SAFETY: cancels only this operation on our own handle.
        let _ = unsafe { CancelIoEx(handle, Some(&overlapped)) };
        let mut ignored = 0u32;
        // SAFETY: waits for the cancelled (or just completed) operation to finish.
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut ignored, true) }.is_ok() {
            // A client connected in the last moment.
            return Ok(Connect::Connected);
        }
        if woke == WAIT_TIMEOUT {
            return Ok(Connect::TimedOut);
        }
        match helper {
            Some(helper) if woke.0 == WAIT_OBJECT_0.0 + 1 => Ok(Connect::HelperExited {
                pid: helper.pid(),
                exit_code: helper.exit_code()?.unwrap_or(u32::MAX),
            }),
            _ => Err(wait_error.unwrap_or(Error::Win32 {
                function: "WaitForMultipleObjects",
                code: woke.0,
            })),
        }
    }
}

/// How one wait for a client ended.
enum Connect {
    Connected,
    /// A client came and went before it could be checked; the instance listens again.
    Gone,
    TimedOut,
    HelperExited {
        pid: u32,
        exit_code: u32,
    },
}

/// One connected end of the pipe.
#[derive(Debug)]
pub struct PipeConnection {
    handle: OwnedHandle,
    peer_pid: u32,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    /// Events of the overlapped reads and writes (one each, so that a clone can read while
    /// another writes).
    read_event: OwnedHandle,
    write_event: OwnedHandle,
}

impl PipeConnection {
    fn new(handle: OwnedHandle, peer_pid: u32) -> Result<Self, Error> {
        Ok(Self {
            handle,
            peer_pid,
            read_timeout: None,
            write_timeout: None,
            read_event: new_event()?,
            write_event: new_event()?,
        })
    }

    /// Helper side: connects with SQOS identification and checks the server's PID
    /// (`GetNamedPipeServerProcessId`) against `expected_server_pid`. `ERROR_PIPE_BUSY` (another
    /// client holds the only instance until the server drops it) is retried with `WaitNamedPipeW`
    /// for up to `timeout`.
    pub fn connect(path: &str, expected_server_pid: u32, timeout: Duration) -> Result<Self, Error> {
        check_pipe_path(path)?;
        let name = to_wide(path);
        let deadline = Instant::now() + timeout;
        let handle = loop {
            // SAFETY: `name` is NUL-terminated and outlives the call. SQOS identification keeps
            // the server from impersonating this process.
            let result = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            };
            match result {
                // SAFETY: CreateFileW succeeded, so the handle is open and owned by us.
                Ok(handle) => break unsafe { own(handle) },
                Err(error) if win32_code(&error) == ERROR_PIPE_BUSY.0 => {
                    wait_for_instance(&name, deadline)?;
                }
                Err(error) => return Err(win32("CreateFileW", &error)),
            }
        };
        let mut server = 0u32;
        // SAFETY: `handle` is a connected client end; `server` is a valid out pointer.
        unsafe { GetNamedPipeServerProcessId(raw(&handle), &mut server) }
            .map_err(|error| win32("GetNamedPipeServerProcessId", &error))?;
        if server != expected_server_pid {
            return Err(Error::PeerMismatch {
                expected: expected_server_pid,
                found: server,
            });
        }
        Self::new(handle, server)
    }

    /// PID of the other end, as checked at connect time.
    pub fn peer_pid(&self) -> u32 {
        self.peer_pid
    }

    /// `None` waits forever. An expired read fails with `io::ErrorKind::TimedOut`.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.read_timeout = timeout;
    }

    /// `None` (the default) waits forever. A write only waits when the pipe's buffer is full,
    /// that is, when the other end stopped reading. An expired write fails with
    /// `io::ErrorKind::TimedOut`; part of the data may have been sent.
    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) {
        self.write_timeout = timeout;
    }

    /// Bytes waiting to be read, without reading or waiting (`PeekNamedPipe`). Fails with
    /// `io::ErrorKind::BrokenPipe` once the other end has closed and everything was read, which
    /// is how the helper notices a caller that went away (design review S4).
    pub fn bytes_available(&self) -> io::Result<u32> {
        let mut available = 0u32;
        // SAFETY: `handle` is a connected pipe end; no data is copied; `available` is valid.
        unsafe { PeekNamedPipe(raw(&self.handle), None, 0, None, Some(&mut available), None) }
            .map_err(|error| io_error(&error))?;
        Ok(available)
    }

    /// A second handle to the same connection (`DuplicateHandle`), with its own events and the
    /// same timeouts, so that one thread can write while another reads.
    pub fn try_clone(&self) -> Result<Self, Error> {
        let mut duplicate = HANDLE::default();
        // SAFETY: both process handles are this process's pseudo handle; `self.handle` is open;
        // `duplicate` is a valid out pointer.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                raw(&self.handle),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .map_err(|error| win32("DuplicateHandle", &error))?;
        // SAFETY: DuplicateHandle succeeded, so `duplicate` is an open handle this process owns.
        let mut clone = Self::new(unsafe { own(duplicate) }, self.peer_pid)?;
        clone.read_timeout = self.read_timeout;
        clone.write_timeout = self.write_timeout;
        Ok(clone)
    }

    /// Runs one overlapped read or write started by `start` and waits for it up to `timeout`.
    /// The operation always completes or is cancelled and waited for before this returns, so
    /// the buffer and `OVERLAPPED` it uses are never released while the kernel may touch them.
    fn complete(
        &self,
        event: &OwnedHandle,
        timeout: Option<Duration>,
        operation: &'static str,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> io::Result<usize> {
        let handle = raw(&self.handle);
        let mut overlapped = OVERLAPPED {
            hEvent: raw(event),
            ..Default::default()
        };
        match start(handle, &mut overlapped) {
            Ok(()) => {}
            Err(error) if win32_code(&error) == ERROR_IO_PENDING.0 => {}
            // Failed synchronously: nothing is in flight.
            Err(error) => return Err(io_error(&error)),
        }
        let mut transferred = 0u32;
        let millis = timeout.map_or(INFINITE, overlapped_wait_millis);
        // SAFETY: `overlapped` is the structure the operation was started with.
        let waited =
            unsafe { GetOverlappedResultEx(handle, &overlapped, &mut transferred, millis, false) };
        let Err(wait_error) = waited else {
            return Ok(transferred as usize);
        };
        // The wait ran out (`WAIT_TIMEOUT`, or `ERROR_IO_INCOMPLETE` for a zero wait), or failed
        // for another reason. In every case the operation may still be in flight, so cancel it
        // and wait for it before `overlapped` and the caller's buffer go out of scope. If it had
        // already completed, the cancellation finds nothing and the wait returns its result.
        let timed_out = [WAIT_TIMEOUT.0, ERROR_IO_INCOMPLETE.0].contains(&win32_code(&wait_error));
        // SAFETY: cancels only this operation on our own handle.
        let _ = unsafe { CancelIoEx(handle, Some(&overlapped)) };
        // SAFETY: waits until the cancelled (or completed) operation has finished.
        match unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) } {
            Ok(()) => Ok(transferred as usize),
            Err(error) if win32_code(&error) == ERROR_OPERATION_ABORTED.0 => {
                if timed_out {
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("pipe {operation} timed out"),
                    ))
                } else {
                    Err(io_error(&wait_error))
                }
            }
            Err(error) => Err(io_error(&error)),
        }
    }
}

/// Waits (`WaitNamedPipeW`) until the only instance can take a connection again, or `deadline`.
fn wait_for_instance(name: &[u16], deadline: Instant) -> Result<(), Error> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(Error::Timeout {
            operation: "WaitNamedPipeW",
        });
    }
    // SAFETY: `name` is NUL-terminated.
    let ready = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), wait_millis(remaining).max(1)) };
    if !ready.as_bool() {
        // SAFETY: GetLastError has no preconditions.
        let code = unsafe { GetLastError() };
        if code == ERROR_SEM_TIMEOUT {
            return Err(Error::Timeout {
                operation: "WaitNamedPipeW",
            });
        }
        if code == ERROR_FILE_NOT_FOUND {
            return Err(Error::Win32 {
                function: "WaitNamedPipeW",
                code: code.0,
            });
        }
    }
    // Another client may take the instance first; do not spin while it does.
    thread::sleep(BUSY_RETRY.min(deadline.saturating_duration_since(Instant::now())));
    Ok(())
}

/// Milliseconds to wait for an overlapped read or write: a positive timeout under 1 ms waits
/// 1 ms rather than being truncated to a zero-length poll.
fn overlapped_wait_millis(timeout: Duration) -> u32 {
    if timeout.is_zero() {
        0
    } else {
        wait_millis(timeout).max(1)
    }
}

fn io_error(error: &windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(win32_code(error) as i32)
}

/// Win32 errors that mean the other end has closed (or was disconnected).
fn is_gone(code: u32) -> bool {
    [
        ERROR_BROKEN_PIPE.0,
        ERROR_NO_DATA.0,
        ERROR_PIPE_NOT_CONNECTED.0,
    ]
    .contains(&code)
}

/// The other end has closed: end of the stream for a reader.
fn is_closed(error: &io::Error) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| is_gone(code as u32))
}

impl Read for PipeConnection {
    /// Returns `Ok(0)` once the other end has closed the pipe.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let buffer = &mut buf[..len];
        let result = self.complete(
            &self.read_event,
            self.read_timeout,
            "read",
            |handle, overlapped| {
                // SAFETY: `buffer` stays borrowed until `complete` returns, which is after the read
                // has completed or been cancelled and waited for; `overlapped` likewise.
                unsafe { ReadFile(handle, Some(buffer), None, Some(overlapped)) }
            },
        );
        match result {
            Err(error) if is_closed(&error) => Ok(0),
            other => other,
        }
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let buffer = &buf[..len];
        self.complete(
            &self.write_event,
            self.write_timeout,
            "write",
            |handle, overlapped| {
                // SAFETY: `buffer` stays borrowed until `complete` returns, which is after the write
                // has completed or been cancelled and waited for; `overlapped` likewise.
                unsafe { WriteFile(handle, Some(buffer), None, Some(overlapped)) }
            },
        )
    }

    /// `FlushFileBuffers`: returns once the other end has read everything written so far (it
    /// blocks without a timeout while the other end is alive but not reading).
    fn flush(&mut self) -> io::Result<()> {
        // SAFETY: `handle` is a connected pipe end.
        unsafe { FlushFileBuffers(raw(&self.handle)) }.map_err(|error| io_error(&error))
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;
    use crate::elevation::current_user_sid;
    use crate::session::new_uuid;

    /// Environment variable that turns [`child_client`] into the client of the named pipe.
    const CHILD_PIPE: &str = "MKLM_WIN_TEST_PIPE_CHILD";

    fn pipe_path() -> String {
        format!(
            r"\\.\pipe\SHINDATACENTER.MKLM.test-{}",
            new_uuid().expect("uuid")
        )
    }

    /// The helper DACL plus the current user, so that an unelevated test can connect.
    fn test_sddl() -> String {
        let user = current_user_sid().expect("user SID");
        format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user})(A;;RC;;;OW)")
    }

    fn read_exact_string(connection: &mut PipeConnection, len: usize) -> String {
        let mut buffer = vec![0u8; len];
        connection.read_exact(&mut buffer).expect("read");
        String::from_utf8(buffer).expect("UTF-8")
    }

    #[test]
    fn round_trip_in_process_with_pid_checks() {
        let path = pipe_path();
        let server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        // Only one instance: a second server with the same name is refused.
        assert!(PipeServer::create(&path, &test_sddl()).is_err());
        let me = std::process::id();
        let client_path = path.clone();
        let client = thread::spawn(move || {
            let mut client = PipeConnection::connect(&client_path, me, Duration::from_secs(10))
                .expect("connect");
            assert_eq!(client.peer_pid(), me);
            client.write_all(b"hello helper").expect("write");
            client.set_read_timeout(Some(Duration::from_secs(10)));
            let reply = read_exact_string(&mut client, 12);
            assert_eq!(reply, "hello caller");
            // The server closes after this: end of stream.
            let mut rest = Vec::new();
            client.read_to_end(&mut rest).expect("read to end");
            assert!(rest.is_empty());
        });
        let mut server = server
            .accept(me, Duration::from_secs(10), None)
            .expect("accept");
        assert_eq!(server.peer_pid(), me);
        server.set_read_timeout(Some(Duration::from_secs(10)));
        assert_eq!(read_exact_string(&mut server, 12), "hello helper");
        server.write_all(b"hello caller").expect("write");
        server.flush().expect("flush");
        drop(server);
        client.join().expect("client thread");
    }

    #[test]
    fn reads_time_out_and_peeks_see_the_data() {
        let path = pipe_path();
        let server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        let me = std::process::id();
        let client_path = path.clone();
        let client = thread::spawn(move || {
            PipeConnection::connect(&client_path, me, Duration::from_secs(10)).expect("connect")
        });
        let mut server = server
            .accept(me, Duration::from_secs(10), None)
            .expect("accept");
        let mut client = client.join().expect("client thread");

        server.set_read_timeout(Some(Duration::from_millis(50)));
        let started = Instant::now();
        let error = server.read(&mut [0u8; 4]).expect_err("nothing to read");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(40));

        assert_eq!(server.bytes_available().expect("peek"), 0);
        client.write_all(b"abc").expect("write");
        assert_eq!(server.bytes_available().expect("peek"), 3);
        // A clone reads from the same connection.
        let mut reader = server.try_clone().expect("clone");
        assert_eq!(read_exact_string(&mut reader, 3), "abc");

        drop(client);
        let error = server.bytes_available().expect_err("closed");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(server.read(&mut [0u8; 4]).expect("end of stream"), 0);
    }

    /// A read with a timeout under 1 ms (or zero) times out cleanly: nothing stays in flight, so
    /// bytes written afterwards reach the next read instead of the abandoned buffer.
    #[test]
    fn sub_millisecond_reads_time_out_without_losing_data() {
        let path = pipe_path();
        let server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        let me = std::process::id();
        let client_path = path.clone();
        let client = thread::spawn(move || {
            PipeConnection::connect(&client_path, me, Duration::from_secs(10)).expect("connect")
        });
        let mut server = server
            .accept(me, Duration::from_secs(10), None)
            .expect("accept");
        let mut client = client.join().expect("client thread");

        for timeout in [Duration::from_micros(500), Duration::ZERO] {
            server.set_read_timeout(Some(timeout));
            let mut abandoned = [0u8; 6];
            let error = server.read(&mut abandoned).expect_err("nothing to read");
            assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{timeout:?}");
            client.write_all(b"abcdef").expect("write");
            // Give a still-pending read the chance to take the bytes (it must not exist).
            thread::sleep(Duration::from_millis(20));
            assert_eq!(abandoned, [0u8; 6], "{timeout:?}");
            server.set_read_timeout(Some(Duration::from_secs(5)));
            assert_eq!(read_exact_string(&mut server, 6), "abcdef", "{timeout:?}");
        }
    }

    #[test]
    fn overlapped_waits_round_up_to_one_millisecond() {
        assert_eq!(overlapped_wait_millis(Duration::ZERO), 0);
        assert_eq!(overlapped_wait_millis(Duration::from_micros(1)), 1);
        assert_eq!(overlapped_wait_millis(Duration::from_micros(999)), 1);
        assert_eq!(overlapped_wait_millis(Duration::from_millis(7)), 7);
    }

    #[test]
    fn nobody_connecting_times_out() {
        let path = pipe_path();
        let server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        let error = server
            .accept(std::process::id(), Duration::from_millis(50), None)
            .expect_err("nobody connects");
        assert_eq!(
            error,
            Error::Timeout {
                operation: "ConnectNamedPipe"
            }
        );
    }

    #[test]
    fn a_client_expecting_another_server_refuses() {
        let path = pipe_path();
        let _server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        let me = std::process::id();
        let error = PipeConnection::connect(&path, me + 4, Duration::from_secs(5))
            .expect_err("wrong server");
        assert_eq!(
            error,
            Error::PeerMismatch {
                expected: me + 4,
                found: me
            }
        );
    }

    #[test]
    fn bad_paths_are_refused() {
        for path in [
            r"\\.\pipe\",
            r"\\server\pipe\x",
            r"C:\x",
            "\\\\.\\pipe\\a\\b",
            "\\\\.\\pipe\\a\0",
        ] {
            assert!(
                PipeServer::create(path, HELPER_PIPE_SDDL).is_err(),
                "{path}"
            );
        }
    }

    /// A wrong client that connects first is dropped, and the server keeps waiting for the
    /// expected one (design review S10). The expected client is this test executable started
    /// again as a child process that runs only [`child_client`].
    #[test]
    fn a_wrong_client_is_dropped_and_the_wait_goes_on() {
        let path = pipe_path();
        let server = PipeServer::create(&path, &test_sddl()).expect("create pipe");
        let me = std::process::id();
        // The impostor (this process) takes the only instance first.
        let mut impostor =
            PipeConnection::connect(&path, me, Duration::from_secs(10)).expect("impostor connects");
        let exe = std::env::current_exe().expect("test executable");
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "pipe::tests::child_client",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD_PIPE, format!("{path}|{me}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the child client");
        let result = server.accept(child.id(), Duration::from_secs(30), None);
        let mut connection = match result {
            Ok(connection) => connection,
            Err(error) => {
                let _ = child.kill();
                panic!("accept: {error:?}");
            }
        };
        assert_eq!(connection.peer_pid(), child.id());
        connection.set_read_timeout(Some(Duration::from_secs(30)));
        assert_eq!(read_exact_string(&mut connection, 10), "from child");
        connection.write_all(b"ok").expect("write");
        let status = child.wait().expect("child exits");
        assert!(status.success(), "{status:?}");
        // The impostor was disconnected.
        impostor.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buffer = [0u8; 1];
        assert!(!matches!(impostor.read(&mut buffer), Ok(1)));
    }

    /// The child side of [`a_wrong_client_is_dropped_and_the_wait_goes_on`]; does nothing in a
    /// normal test run.
    #[test]
    fn child_client() {
        let Ok(spec) = std::env::var(CHILD_PIPE) else {
            return;
        };
        let (path, server) = spec.split_once('|').expect("pipe|pid");
        let server: u32 = server.parse().expect("server PID");
        let mut connection =
            PipeConnection::connect(path, server, Duration::from_secs(20)).expect("child connects");
        connection.write_all(b"from child").expect("write");
        connection.set_read_timeout(Some(Duration::from_secs(20)));
        assert_eq!(read_exact_string(&mut connection, 2), "ok");
    }
}
