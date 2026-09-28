//! Single instance of the GUI (plan 2.2, 3.9; design m3 F.1).
//!
//! Neither name is a secret: the SID and the session ID can be guessed, the pipe namespace is
//! machine-wide, and another account (or a `runas` process in the same session) can create
//! either object first. The names only make a collision unlikely; what keeps a squatter out is
//! the checks below (design m3 F.1, review A6):
//!
//! - A named mutex `Local\SHINDATACENTER.MKLM.Instance.<user SID>` decides which process is the
//!   instance. `ERROR_ALREADY_EXISTS` → secondary. `ERROR_ACCESS_DENIED` (someone else created it
//!   with a restrictive DACL) → log it and run as the instance anyway: a squatted name must not
//!   stop MKLM. (Plan 2.2 forbids `Global\` mutexes only for the write lock, which this is not.)
//! - The instance serves a pipe `\\.\pipe\SHINDATACENTER.MKLM.Instance.<session ID>.<user SID>`
//!   with `FILE_FLAG_FIRST_PIPE_INSTANCE`, `PIPE_REJECT_REMOTE_CLIENTS` and the plan's DACL
//!   ([`instance_pipe_sddl`]). When creating it fails with `ERROR_ACCESS_DENIED` or
//!   `ERROR_PIPE_BUSY` (the name is taken), the instance logs it and keeps running without the
//!   pipe. It accepts exactly three commands, one per connection: `activate`, `quit` and (M5b,
//!   the updater's only command) `quit-if-idle` ([`InstanceCommand`]), each within
//!   [`INSTANCE_READ_TIMEOUT`] (a client that connects and stays silent is disconnected), and
//!   answers `ok` or `busy`.
//! - A second process connects (`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`) and verifies
//!   the server before it trusts it ([`ServerIdentity`], [`is_our_instance`]): the server's
//!   session (`GetNamedPipeServerSessionId`), the user SID of the server process's token
//!   (`GetNamedPipeServerProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` →
//!   `OpenProcessToken`) and its image (`QueryFullProcessImageNameW`) must all be this process's.
//!   Only then does it call `AllowSetForegroundWindow` for that PID and send its command. On a
//!   mismatch it sends nothing and runs as the instance itself. The M5b elevated sender (the
//!   update runner, [`quit_idle_instances`]) computes each GUI's pipe name from the running GUI
//!   processes instead of enumerating pipes, checks the server likewise, and sends
//!   `quit-if-idle` only (design m5b D.8).
//!
//! The instance answers after the client has read the reply: it writes the reply, then waits (at
//! most [`INSTANCE_READ_TIMEOUT`] again) for the client to close its end before it disconnects,
//! because `DisconnectNamedPipe` discards unread data and `FlushFileBuffers` could wait forever on
//! a client that stops reading.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_NOT_CONNECTED,
    FILETIME, GetLastError, HANDLE,
};
use windows::Win32::Security::{TOKEN_QUERY, TOKEN_USER, TokenSessionId, TokenUser};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_NAME_FORMAT, PROCESS_NAME_NATIVE, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::{PCWSTR, PWSTR};

use crate::elevation::{current_user_sid, token_information};
use crate::error::{Error, win32_code};
use crate::pipe::{PipeConnection, PipeServer};
use crate::proc_identity::{
    ImageProcess, ProcessUser, equal_ignoring_case, process_identity, process_image_nt_path,
    process_users, processes_with_images,
};
use crate::props::to_wide;
use crate::security::sid_to_string;
use crate::sys::{own, raw, win32};

/// Longest command or reply on the instance pipe, in bytes. Anything longer is refused.
pub const MAX_INSTANCE_MESSAGE: usize = 16;

/// How long the instance waits for a connected client's command before it disconnects it.
pub const INSTANCE_READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Who serves the instance pipe, as a second process reads it after connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    /// `GetNamedPipeServerSessionId`.
    pub session_id: u32,
    /// The user SID of the server process's token, as a string (`S-1-5-21-…`).
    pub user_sid: String,
    /// `QueryFullProcessImageNameW` of the server process.
    pub image: PathBuf,
}

/// True when the pipe's server is another copy of this program in this user's session: same
/// session, same user SID, same executable path (compared case-insensitively, as NTFS does).
/// `ours` describes the calling process.
pub fn is_our_instance(server: &ServerIdentity, ours: &ServerIdentity) -> bool {
    fn same_path(a: &Path, b: &Path) -> bool {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    server.session_id == ours.session_id
        && server.user_sid.eq_ignore_ascii_case(&ours.user_sid)
        && same_path(&server.image, &ours.image)
}

/// What a second process asks the running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceCommand {
    /// Show the window and bring it to the front (a second start, the RunOnce `--post-reboot`).
    Activate,
    /// Quit cleanly (`mklm.exe --quit`: the installer and the uninstaller).
    Quit,
    /// `quit-if-idle\n` (13 bytes): quit only when nothing is going on, else answer `busy` and
    /// change nothing (design m5b D.8, E.4.1; RELIABILITY-1, OPS-UX-TEST-4). The updater's only
    /// command.
    QuitIfIdle,
}

impl InstanceCommand {
    /// The bytes on the wire.
    pub fn wire(self) -> &'static [u8] {
        match self {
            InstanceCommand::Activate => b"activate\n",
            InstanceCommand::Quit => b"quit\n",
            InstanceCommand::QuitIfIdle => b"quit-if-idle\n",
        }
    }

    /// Parses one received message; anything but the three exact commands is `None`.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"activate\n" => Some(InstanceCommand::Activate),
            b"quit\n" => Some(InstanceCommand::Quit),
            b"quit-if-idle\n" => Some(InstanceCommand::QuitIfIdle),
            _ => None,
        }
    }
}

/// The running instance's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceReply {
    /// Done (activated), or quitting now.
    Ok,
    /// A helper session is running: `quit` happens when it has ended (a countdown is reverted
    /// first, design m3 F.5).
    Busy,
}

impl InstanceReply {
    pub fn wire(self) -> &'static [u8] {
        match self {
            InstanceReply::Ok => b"ok\n",
            InstanceReply::Busy => b"busy\n",
        }
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"ok\n" => Some(InstanceReply::Ok),
            b"busy\n" => Some(InstanceReply::Busy),
            _ => None,
        }
    }
}

/// The DACL of the instance pipe (plan 2.2): SYSTEM, Administrators and the user.
pub fn instance_pipe_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})")
}

/// The mutex name for `user_sid`.
pub fn instance_mutex_name(user_sid: &str) -> String {
    format!(r"Local\SHINDATACENTER.MKLM.Instance.{user_sid}")
}

/// The pipe path for `session_id` and `user_sid` (pipe names are machine-wide).
pub fn instance_pipe_path(session_id: u32, user_sid: &str) -> String {
    format!(r"\\.\pipe\SHINDATACENTER.MKLM.Instance.{session_id}.{user_sid}")
}

// ---- M5b: the update runner's side (design m5b D.8; WP-H) ----

/// How one GUI answered the update runner's `quit-if-idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAnswer {
    Ok,
    Busy,
    /// The pipe's server is not `gui_nt_path` in the pipe's session: nothing was sent.
    NotOurs,
    NoAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceQuit {
    pub pipe: String,
    /// The session in the pipe's name.
    pub session_id: u32,
    pub server_pid: Option<u32>,
    pub answer: QuitAnswer,
}

/// The fixed part of an instance pipe's name, before `<session>.<SID>`.
const INSTANCE_PIPE_NAME_PREFIX: &str = "SHINDATACENTER.MKLM.Instance.";

/// `SHINDATACENTER.MKLM.Instance.<session>.<SID>` → (session, SID) when the whole name matches
/// `^SHINDATACENTER\.MKLM\.Instance\.([0-9]{1,10})\.(S-1-5-21(-[0-9]{1,10}){4}|S-1-12-1(-[0-9]{1,10}){4})$`
/// (SECURITY-7). Anything else: `None`, and the pipe is never opened.
///
/// The name is without the `\\.\pipe\` prefix. A session number that does not fit a `u32` is
/// `None` too (no session has it).
pub fn parse_instance_pipe_name(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix(INSTANCE_PIPE_NAME_PREFIX)?;
    let (session, sid) = rest.split_once('.')?;
    if !is_decimal(session) {
        return None;
    }
    let session = session.parse::<u32>().ok()?;
    let sub_authorities = sid
        .strip_prefix("S-1-5-21-")
        .or_else(|| sid.strip_prefix("S-1-12-1-"))?;
    let mut count = 0;
    for part in sub_authorities.split('-') {
        count += 1;
        if !is_decimal(part) {
            return None;
        }
    }
    (count == 4).then(|| (session, sid.to_string()))
}

/// `[0-9]{1,10}`: ASCII digits only.
fn is_decimal(text: &str) -> bool {
    (1..=10).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// Pure (FIX-VERIFICATION-5): for each GUI process (from `processes_with_images`) whose
/// `process_users` row has a SID and the same session, `instance_pipe_path(session, sid)` —
/// kept only if its name also passes `parse_instance_pipe_name`; at most `max_pipes`, in input
/// order. Returns (pipe path, the GUI process).
pub fn instance_pipe_candidates(
    guis: &[ImageProcess],
    users: &HashMap<u32, ProcessUser>,
    max_pipes: usize,
) -> Vec<(String, ImageProcess)> {
    let mut candidates = Vec::new();
    for gui in guis {
        if candidates.len() >= max_pipes {
            break;
        }
        let Some(user) = users.get(&gui.identity.pid) else {
            continue;
        };
        let Some(sid) = user.user_sid.as_deref() else {
            continue;
        };
        if user.session_id != gui.session_id {
            continue;
        }
        let path = instance_pipe_path(gui.session_id, sid);
        let accepted = path
            .strip_prefix(r"\\.\pipe\")
            .and_then(parse_instance_pipe_name)
            .is_some_and(|(session, name_sid)| session == gui.session_id && name_sid == sid);
        if accepted {
            candidates.push((path, gui.clone()));
        }
    }
    candidates
}

/// Design m5b D.8 step 2: finds the processes running `gui_nt_path`, computes their pipe names
/// with `instance_pipe_candidates` (never enumerates `\\.\pipe\`), opens each with
/// `pipe::open_client` (SQOS identification, overlapped I/O), `per_pipe` each and `total` for all;
/// the server must be that GUI's PID, pinned by a handle (or compared by identity before and
/// after), running `gui_nt_path` in the pipe's session; sends `quit-if-idle` only.
///
/// GUIs past `max_pipes` get no message and no entry. Once `total` is used up, the remaining
/// candidates are reported as `NoAnswer` without being opened.
pub fn quit_idle_instances(
    gui_nt_path: &str,
    per_pipe: Duration,
    total: Duration,
    max_pipes: usize,
) -> Result<Vec<InstanceQuit>, Error> {
    let guis = processes_with_images(&[gui_nt_path.to_string()])?;
    if guis.is_empty() {
        return Ok(Vec::new());
    }
    let users = process_users()?;
    let started = Instant::now();
    let mut answers = Vec::new();
    for (pipe, gui) in instance_pipe_candidates(&guis, &users, max_pipes) {
        let remaining = total.saturating_sub(started.elapsed());
        let (server_pid, answer) = if remaining.is_zero() {
            (None, QuitAnswer::NoAnswer)
        } else {
            quit_one(&pipe, &gui, gui_nt_path, per_pipe.min(remaining))
        };
        answers.push(InstanceQuit {
            pipe,
            session_id: gui.session_id,
            server_pid,
            answer,
        });
    }
    Ok(answers)
}

/// One GUI: connect, verify the server, send `quit-if-idle`, read the answer, all within `wait`.
fn quit_one(
    pipe: &str,
    gui: &ImageProcess,
    gui_nt_path: &str,
    wait: Duration,
) -> (Option<u32>, QuitAnswer) {
    let deadline = Instant::now() + wait;
    let Ok(mut connection) = PipeConnection::connect_any(pipe, wait) else {
        return (None, QuitAnswer::NoAnswer);
    };
    let server = connection.peer_pid();
    if server != gui.identity.pid {
        return (Some(server), QuitAnswer::NotOurs);
    }
    // Pinned by a handle when this process may open it (then the PID cannot be reused until the
    // answer is read); otherwise its identity is compared before sending and after the answer
    // (design m5b I.12).
    let pinned = open_process(server).ok();
    let verified = match &pinned {
        Some(process) => {
            process_creation_time(raw(process)).ok() == Some(gui.identity.creation_time)
                && process_image(raw(process), PROCESS_NAME_NATIVE)
                    .is_ok_and(|image| same_nt_path(&image.to_string_lossy(), gui_nt_path))
        }
        None => {
            process_identity(server).ok().flatten() == Some(gui.identity)
                && process_image_nt_path(server)
                    .is_ok_and(|image| same_nt_path(&image, gui_nt_path))
        }
    };
    let same_session = connection
        .server_session_id()
        .is_ok_and(|session| session == gui.session_id);
    if !verified || !same_session {
        return (Some(server), QuitAnswer::NotOurs);
    }
    connection.set_write_timeout(Some(deadline.saturating_duration_since(Instant::now())));
    if connection
        .write_all(InstanceCommand::QuitIfIdle.wire())
        .is_err()
    {
        return (Some(server), QuitAnswer::NoAnswer);
    }
    let reply = read_message(&mut connection, deadline)
        .ok()
        .and_then(|bytes| InstanceReply::parse(&bytes));
    if pinned.is_none()
        && let Ok(Some(now)) = process_identity(server)
        && now != gui.identity
    {
        return (Some(server), QuitAnswer::NotOurs);
    }
    let answer = match reply {
        Some(InstanceReply::Ok) => QuitAnswer::Ok,
        Some(InstanceReply::Busy) => QuitAnswer::Busy,
        None => QuitAnswer::NoAnswer,
    };
    (Some(server), answer)
}

/// NT paths compare like file names: ordinal, ignoring case.
fn same_nt_path(a: &str, b: &str) -> bool {
    equal_ignoring_case(a, b)
}

/// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`.
fn open_process(pid: u32) -> Result<OwnedHandle, Error> {
    // SAFETY: plain Win32 call with a process ID; the handle is owned below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| win32("OpenProcess", &error))?;
    // SAFETY: OpenProcess succeeded, so `process` is an open handle this process owns.
    Ok(unsafe { own(process) })
}

/// Creation time of an open process (FILETIME units, as `ProcessIdentity` keeps it).
fn process_creation_time(process: HANDLE) -> Result<u64, Error> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `process` is open with PROCESS_QUERY_LIMITED_INFORMATION; out pointers are valid.
    unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
        .map_err(|error| win32("GetProcessTimes", &error))?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

/// Which role this process has.
#[derive(Debug)]
pub enum InstanceRole {
    /// This process is the instance; keep the guard for the process lifetime.
    Primary(InstanceGuard),
    /// Another process is the instance.
    Secondary,
}

/// Holds the instance mutex for as long as it lives.
#[derive(Debug)]
pub struct InstanceGuard {
    /// `None` when someone else created the name with a DACL that shuts this user out
    /// (`ERROR_ACCESS_DENIED`): this process runs as the instance without it.
    mutex: Option<OwnedHandle>,
}

impl InstanceGuard {
    /// True when the mutex name was taken by another account (`ERROR_ACCESS_DENIED`) and this
    /// process runs as the instance without holding it. The caller logs it (design m3 F.1).
    pub fn name_taken(&self) -> bool {
        self.mutex.is_none()
    }
}

/// Creates or opens the instance mutex [`instance_mutex_name`] for this process's user
/// (`CreateMutexW`): `ERROR_ALREADY_EXISTS` → [`InstanceRole::Secondary`];
/// `ERROR_ACCESS_DENIED` → [`InstanceRole::Primary`] without the mutex
/// ([`InstanceGuard::name_taken`], logged by the caller), so that a squatted name never stops
/// MKLM.
pub fn acquire_instance() -> Result<InstanceRole, Error> {
    acquire_named(&instance_mutex_name(&current_user_sid()?))
}

/// [`acquire_instance`] for the mutex `name`.
fn acquire_named(name: &str) -> Result<InstanceRole, Error> {
    let wide = to_wide(name);
    // SAFETY: default security, not initially owned; `wide` is NUL-terminated and outlives the
    // call.
    let created = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) };
    // Taken at once: on success, the thread's last error tells whether the mutex existed.
    // SAFETY: GetLastError has no preconditions.
    let last = unsafe { GetLastError() };
    match created {
        Ok(handle) => {
            // SAFETY: CreateMutexW succeeded, so `handle` is an open handle this process owns.
            let handle = unsafe { own(handle) };
            if last == ERROR_ALREADY_EXISTS {
                // Our handle closes here; the instance's keeps the mutex alive.
                Ok(InstanceRole::Secondary)
            } else {
                Ok(InstanceRole::Primary(InstanceGuard {
                    mutex: Some(handle),
                }))
            }
        }
        Err(error) if win32_code(&error) == ERROR_ACCESS_DENIED.0 => {
            Ok(InstanceRole::Primary(InstanceGuard { mutex: None }))
        }
        Err(error) => Err(win32("CreateMutexW", &error)),
    }
}

/// The running instance's pipe server. `Send`: it is served on its own thread
/// (`mklm-instance`, design m3 A.4).
#[derive(Debug)]
pub struct InstanceServer {
    pipe: PipeServer,
    /// The client whose command was returned last and still waits for its [`reply`](Self::reply).
    pending: Option<PipeConnection>,
}

impl InstanceServer {
    /// Creates the pipe [`instance_pipe_path`] of this process's session and user with the DACL
    /// [`instance_pipe_sddl`] (`FILE_FLAG_FIRST_PIPE_INSTANCE`, `PIPE_REJECT_REMOTE_CLIENTS`, one
    /// instance). When the name is taken, this fails with the Win32 error of `CreateNamedPipeW`
    /// (`ERROR_ACCESS_DENIED` or `ERROR_PIPE_BUSY`); the caller logs it and keeps running without
    /// the pipe (design m3 F.1).
    pub fn create() -> Result<Self, Error> {
        let token = open_token(current_process())?;
        let user_sid = token_user_sid(&token)?;
        let session_id = token_session_id(&token)?;
        Self::create_at(
            &instance_pipe_path(session_id, &user_sid),
            &instance_pipe_sddl(&user_sid),
        )
    }

    /// The server on `path` with `sddl`.
    fn create_at(path: &str, sddl: &str) -> Result<Self, Error> {
        Ok(Self {
            pipe: PipeServer::create(path, sddl)?,
            pending: None,
        })
    }

    /// The `\\.\pipe\…` path of the pipe.
    pub fn path(&self) -> &str {
        self.pipe.path()
    }

    /// Waits for the next command (blocking, without a limit; run it on its own thread). A client
    /// that sends anything but one exact command ([`InstanceCommand::parse`], at most
    /// [`MAX_INSTANCE_MESSAGE`] bytes), or nothing within [`INSTANCE_READ_TIMEOUT`] of
    /// connecting, is disconnected and the wait goes on. Answer the command with
    /// [`reply`](Self::reply); a command left unanswered is dropped by the next wait.
    pub fn next_command(&mut self) -> Result<InstanceCommand, Error> {
        loop {
            if let Some(command) = self.wait_command(None)? {
                return Ok(command);
            }
        }
    }

    /// [`next_command`](Self::next_command) for up to `timeout`; `Ok(None)` when no command came.
    /// Lets the serving thread look at a stop flag between waits.
    pub fn next_command_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<InstanceCommand>, Error> {
        self.wait_command(Instant::now().checked_add(timeout))
    }

    fn wait_command(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Option<InstanceCommand>, Error> {
        self.close_pending()?;
        loop {
            let Some(mut connection) = self.pipe.wait_any(deadline)? else {
                return Ok(None);
            };
            let message = read_message(&mut connection, Instant::now() + INSTANCE_READ_TIMEOUT);
            if let Some(command) = message
                .ok()
                .and_then(|bytes| InstanceCommand::parse(&bytes))
            {
                self.pending = Some(connection);
                return Ok(Some(command));
            }
            // Silent, malformed or gone: drop it without an answer and take the next client.
            drop(connection);
            self.pipe.disconnect()?;
        }
    }

    /// Answers the command just received, waits (at most [`INSTANCE_READ_TIMEOUT`]) for the
    /// client to read it and close its end, and disconnects it. Fails with
    /// `ERROR_PIPE_NOT_CONNECTED` when there is no command to answer.
    pub fn reply(&mut self, reply: InstanceReply) -> Result<(), Error> {
        let Some(mut connection) = self.pending.take() else {
            return Err(Error::Win32 {
                function: "InstanceServer::reply",
                code: ERROR_PIPE_NOT_CONNECTED.0,
            });
        };
        connection.set_write_timeout(Some(INSTANCE_READ_TIMEOUT));
        let written = connection.write_all(reply.wire());
        if written.is_ok() {
            wait_for_close(&mut connection, Instant::now() + INSTANCE_READ_TIMEOUT);
        }
        drop(connection);
        self.pipe.disconnect()?;
        written.map_err(|error| io_failure("WriteFile", &error))
    }

    /// Disconnects a client whose command was never answered.
    fn close_pending(&mut self) -> Result<(), Error> {
        if self.pending.take().is_some() {
            self.pipe.disconnect()?;
        }
        Ok(())
    }
}

/// Sends `command` to the running instance (a second process) and returns its reply. Retries the
/// connection for up to `timeout`, because at sign-in the instance may hold the mutex a moment
/// before its pipe exists (Run and RunOnce start together, design m3 F.2), then waits up to
/// `timeout` again for the reply.
///
/// Before sending, it reads the server's [`ServerIdentity`] (`GetNamedPipeServerSessionId`, the
/// user SID of the server process's token, `QueryFullProcessImageNameW`) and fails with
/// [`Error::Insecure`] when it cannot be read or is not [`is_our_instance`]: nothing is sent then
/// and the caller runs as the instance. Only for our own instance does it call
/// `AllowSetForegroundWindow`, so that the instance may bring its window to the front.
/// [`Error::Timeout`] when no pipe could be reached or no reply came in time.
pub fn send_to_instance(
    command: InstanceCommand,
    timeout: Duration,
) -> Result<InstanceReply, Error> {
    let ours = own_identity()?;
    let path = instance_pipe_path(ours.session_id, &ours.user_sid);
    send_at(&path, command, timeout, &ours)
}

/// [`send_to_instance`] on `path`, trusting only a server that matches `ours`.
fn send_at(
    path: &str,
    command: InstanceCommand,
    timeout: Duration,
    ours: &ServerIdentity,
) -> Result<InstanceReply, Error> {
    let mut connection = PipeConnection::connect_any(path, timeout)?;
    let server = server_identity(&connection).map_err(|error| Error::Insecure {
        path: path.to_string(),
        reason: format!("the server could not be verified: {error}"),
    })?;
    if !is_our_instance(&server, ours) {
        return Err(Error::Insecure {
            path: path.to_string(),
            reason: format!(
                "served by {} in session {} as {}, not by this program in this session",
                server.image.display(),
                server.session_id,
                server.user_sid
            ),
        });
    }
    // Lets the instance take the foreground; when this fails the instance can only flash its
    // taskbar button, which is no reason not to send.
    // SAFETY: plain Win32 call with a process ID.
    let _ = unsafe { AllowSetForegroundWindow(connection.peer_pid()) };
    connection.set_write_timeout(Some(timeout));
    connection
        .write_all(command.wire())
        .map_err(|error| io_failure("WriteFile", &error))?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(|| Instant::now() + INSTANCE_READ_TIMEOUT);
    let reply = match read_message(&mut connection, deadline) {
        Ok(bytes) => InstanceReply::parse(&bytes),
        Err(MessageError::TimedOut) => {
            return Err(Error::Timeout {
                operation: "instance pipe reply",
            });
        }
        Err(MessageError::Io(error)) => return Err(io_failure("ReadFile", &error)),
        Err(MessageError::Closed | MessageError::TooLong) => None,
    };
    reply.ok_or_else(|| Error::UnexpectedData {
        path: format!("{path} (reply)"),
    })
}

/// Why [`read_message`] returned no message.
#[derive(Debug)]
enum MessageError {
    /// The deadline passed first.
    TimedOut,
    /// The other end closed first.
    Closed,
    /// More than [`MAX_INSTANCE_MESSAGE`] bytes without a newline.
    TooLong,
    Io(io::Error),
}

/// Reads one message before `deadline`: every byte read until one of them is a newline (bytes
/// after it in the same read stay in the message, so that [`InstanceCommand::parse`] refuses
/// them), at most [`MAX_INSTANCE_MESSAGE`] bytes.
fn read_message(
    connection: &mut PipeConnection,
    deadline: Instant,
) -> Result<Vec<u8>, MessageError> {
    let mut buffer = [0u8; MAX_INSTANCE_MESSAGE + 1];
    let mut len = 0usize;
    while !buffer[..len].contains(&b'\n') {
        if len > MAX_INSTANCE_MESSAGE {
            return Err(MessageError::TooLong);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(MessageError::TimedOut);
        }
        connection.set_read_timeout(Some(remaining));
        match connection.read(&mut buffer[len..]) {
            Ok(0) => return Err(MessageError::Closed),
            Ok(read) => len += read,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Err(MessageError::TimedOut);
            }
            Err(error) => return Err(MessageError::Io(error)),
        }
    }
    Ok(buffer[..len].to_vec())
}

/// Reads and drops whatever the client still sends until it closes its end or `deadline`.
fn wait_for_close(connection: &mut PipeConnection, deadline: Instant) {
    let mut sink = [0u8; MAX_INSTANCE_MESSAGE];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        connection.set_read_timeout(Some(remaining));
        match connection.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// An I/O error of the pipe as an [`Error`]: a timeout is [`Error::Timeout`].
fn io_failure(function: &'static str, error: &io::Error) -> Error {
    if error.kind() == io::ErrorKind::TimedOut {
        Error::Timeout {
            operation: function,
        }
    } else {
        Error::Win32 {
            function,
            code: error.raw_os_error().map_or(0, |code| code as u32),
        }
    }
}

/// This process's pseudo handle (needs no closing).
fn current_process() -> HANDLE {
    // SAFETY: GetCurrentProcess has no preconditions.
    unsafe { GetCurrentProcess() }
}

/// This process as a server would be seen: its session, user and image.
fn own_identity() -> Result<ServerIdentity, Error> {
    let process = current_process();
    let token = open_token(process)?;
    Ok(ServerIdentity {
        session_id: token_session_id(&token)?,
        user_sid: token_user_sid(&token)?,
        image: process_image(process, PROCESS_NAME_WIN32)?,
    })
}

/// The identity of the server at the other end of a client `connection`.
fn server_identity(connection: &PipeConnection) -> Result<ServerIdentity, Error> {
    let session_id = connection.server_session_id()?;
    // SAFETY: plain Win32 call with a process ID; the handle is owned below.
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            connection.peer_pid(),
        )
    }
    .map_err(|error| win32("OpenProcess", &error))?;
    // SAFETY: OpenProcess succeeded, so `process` is an open handle this process owns.
    let process = unsafe { own(process) };
    let token = open_token(raw(&process))?;
    Ok(ServerIdentity {
        session_id,
        user_sid: token_user_sid(&token)?,
        image: process_image(raw(&process), PROCESS_NAME_WIN32)?,
    })
}

/// The token of `process`, opened with `TOKEN_QUERY`.
fn open_token(process: HANDLE) -> Result<OwnedHandle, Error> {
    let mut token = HANDLE::default();
    // SAFETY: `process` is an open process handle (or this process's pseudo handle); `token` is
    // a valid out pointer.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
        .map_err(|error| win32("OpenProcessToken", &error))?;
    // SAFETY: OpenProcessToken succeeded, so `token` is an open handle this process owns.
    Ok(unsafe { own(token) })
}

/// The user SID of `token`, as `S-1-5-21-…`.
fn token_user_sid(token: &OwnedHandle) -> Result<String, Error> {
    let buffer = token_information(token, TokenUser, "GetTokenInformation(TokenUser)")?;
    if buffer.len() * 8 < size_of::<TOKEN_USER>() {
        return Err(Error::UnexpectedData {
            path: "TokenUser".to_string(),
        });
    }
    // SAFETY: GetTokenInformation(TokenUser) filled the 8-byte aligned buffer, which is large
    // enough for a TOKEN_USER (checked above), with a TOKEN_USER whose SID points into the same
    // buffer, which lives until the end of this function.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_to_string(sid)
}

/// The Terminal Services session of `token` (`TokenSessionId`, a DWORD).
fn token_session_id(token: &OwnedHandle) -> Result<u32, Error> {
    let buffer = token_information(token, TokenSessionId, "GetTokenInformation(TokenSessionId)")?;
    let first = buffer.first().map(|word| word.to_ne_bytes());
    first
        .and_then(|bytes| bytes.first_chunk::<4>().copied())
        .map(u32::from_ne_bytes)
        .ok_or_else(|| Error::UnexpectedData {
            path: "TokenSessionId".to_string(),
        })
}

/// The path of `process`'s image (`QueryFullProcessImageNameW`): `PROCESS_NAME_WIN32` (a drive
/// path) or `PROCESS_NAME_NATIVE` (an NT path, as the update runner compares it).
fn process_image(process: HANDLE, format: PROCESS_NAME_FORMAT) -> Result<PathBuf, Error> {
    let mut buffer = vec![0u16; 1024];
    for _ in 0..4 {
        let mut size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: `process` is open with PROCESS_QUERY_LIMITED_INFORMATION (or is this process's
        // pseudo handle); `buffer` has `size` writable UTF-16 units; `size` is a valid in/out
        // pointer.
        let result = unsafe {
            QueryFullProcessImageNameW(process, format, PWSTR(buffer.as_mut_ptr()), &mut size)
        };
        match result {
            Ok(()) => {
                let units = buffer.get(..size as usize).unwrap_or_default();
                return Ok(PathBuf::from(OsString::from_wide(units)));
            }
            Err(error) if win32_code(&error) == ERROR_INSUFFICIENT_BUFFER.0 => {
                buffer = vec![0u16; buffer.len() * 4];
            }
            Err(error) => return Err(win32("QueryFullProcessImageNameW", &error)),
        }
    }
    Err(Error::Win32 {
        function: "QueryFullProcessImageNameW",
        code: ERROR_INSUFFICIENT_BUFFER.0,
    })
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::session::new_uuid;

    /// A pipe path of its own for each test: the real one may be served by a running MKLM.
    fn test_pipe() -> String {
        format!(
            r"\\.\pipe\SHINDATACENTER.MKLM.test-instance-{}",
            new_uuid().expect("uuid")
        )
    }

    /// An instance server on `path` with the real DACL for this user.
    fn test_server(path: &str) -> InstanceServer {
        let sid = current_user_sid().expect("user SID");
        InstanceServer::create_at(path, &instance_pipe_sddl(&sid)).expect("create the pipe")
    }

    #[test]
    fn own_identity_is_this_process() {
        let ours = own_identity().expect("identity");
        assert_eq!(ours.user_sid, current_user_sid().expect("user SID"));
        assert!(ours.user_sid.starts_with("S-1-5-"), "{ours:?}");
        assert!(ours.image.is_absolute(), "{ours:?}");
        let exe = std::env::current_exe().expect("test executable");
        assert_eq!(
            ours.image
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase()),
            exe.file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
        );
        assert!(is_our_instance(&ours, &ours));
    }

    #[test]
    fn commands_round_trip_in_process() {
        let path = test_pipe();
        let mut server = test_server(&path);
        assert_eq!(server.path(), path);
        let ours = own_identity().expect("identity");
        let client_path = path.clone();
        let client = thread::spawn(move || {
            let timeout = Duration::from_secs(10);
            // What a client sees of the server: this process, in this session. The connection
            // closes without a command, which the server skips.
            let identity = PipeConnection::connect_any(&client_path, timeout)
                .and_then(|connection| server_identity(&connection));
            let first = send_at(&client_path, InstanceCommand::Activate, timeout, &ours);
            let second = send_at(&client_path, InstanceCommand::Quit, timeout, &ours);
            (identity, first, second)
        });
        assert_eq!(server.next_command(), Ok(InstanceCommand::Activate));
        assert_eq!(server.reply(InstanceReply::Ok), Ok(()));
        assert_eq!(server.next_command(), Ok(InstanceCommand::Quit));
        assert_eq!(server.reply(InstanceReply::Busy), Ok(()));
        let (identity, first, second) = client.join().expect("client thread");
        assert_eq!(identity, own_identity());
        assert_eq!(first, Ok(InstanceReply::Ok));
        assert_eq!(second, Ok(InstanceReply::Busy));
        // Nothing is left to answer.
        assert!(server.reply(InstanceReply::Ok).is_err());
    }

    /// A client that expects another executable (here: the installed `mklm.exe`) refuses this
    /// test process as its server, sends nothing, and the server never sees a command.
    #[test]
    fn a_server_of_another_program_gets_nothing() {
        let path = test_pipe();
        let mut server = test_server(&path);
        let mut ours = own_identity().expect("identity");
        ours.image = PathBuf::from(r"C:\Program Files\MKLM\mklm.exe");
        let client_path = path.clone();
        let result = thread::spawn(move || {
            send_at(
                &client_path,
                InstanceCommand::Quit,
                Duration::from_secs(10),
                &ours,
            )
        })
        .join()
        .expect("client thread");
        assert!(matches!(result, Err(Error::Insecure { .. })), "{result:?}");
        assert_eq!(
            server.next_command_timeout(Duration::from_millis(1500)),
            Ok(None)
        );
    }

    /// Silent, malformed and over-long clients are disconnected without an answer, and the
    /// instance goes on to serve the next client.
    #[test]
    fn bad_clients_are_dropped_and_the_next_one_is_served() {
        let path = test_pipe();
        let mut server = test_server(&path);
        let ours = own_identity().expect("identity");
        let client_path = path.clone();
        let clients = thread::spawn(move || {
            let connect = || {
                let mut client = PipeConnection::connect_any(&client_path, Duration::from_secs(10))
                    .expect("connect");
                client.set_read_timeout(Some(Duration::from_secs(10)));
                client
            };
            let mut byte = [0u8; 1];
            let mut silent = connect();
            let started = Instant::now();
            assert_eq!(silent.read(&mut byte).expect("disconnected"), 0);
            let silent_for = started.elapsed();
            for message in [&b"show\n"[..], b"activate\nquit\n", &[b'a'; 40]] {
                let mut client = connect();
                client.write_all(message).expect("write");
                assert_eq!(
                    client.read(&mut byte).expect("disconnected"),
                    0,
                    "{message:?}"
                );
            }
            let reply = send_at(
                &client_path,
                InstanceCommand::Activate,
                Duration::from_secs(10),
                &ours,
            );
            (reply, silent_for)
        });
        assert_eq!(server.next_command(), Ok(InstanceCommand::Activate));
        assert_eq!(server.reply(InstanceReply::Ok), Ok(()));
        let (reply, silent_for) = clients.join().expect("client thread");
        assert_eq!(reply, Ok(InstanceReply::Ok));
        assert!(
            silent_for >= Duration::from_millis(800) && silent_for < Duration::from_secs(5),
            "{silent_for:?}"
        );
    }

    #[test]
    fn no_instance_pipe_times_out() {
        let ours = own_identity().expect("identity");
        let started = Instant::now();
        let result = send_at(
            &test_pipe(),
            InstanceCommand::Activate,
            Duration::from_millis(200),
            &ours,
        );
        assert_eq!(
            result,
            Err(Error::Timeout {
                operation: "CreateFileW"
            })
        );
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[test]
    fn the_first_holder_of_the_mutex_is_the_instance() {
        let name = format!(
            r"Local\SHINDATACENTER.MKLM.test-instance-{}",
            new_uuid().expect("uuid")
        );
        let Ok(InstanceRole::Primary(guard)) = acquire_named(&name) else {
            panic!("the first process to create the mutex is the instance");
        };
        assert!(!guard.name_taken());
        assert!(matches!(acquire_named(&name), Ok(InstanceRole::Secondary)));
        drop(guard);
        assert!(matches!(acquire_named(&name), Ok(InstanceRole::Primary(_))));
    }

    #[test]
    fn only_the_three_commands_are_accepted() {
        for command in [
            InstanceCommand::Activate,
            InstanceCommand::Quit,
            InstanceCommand::QuitIfIdle,
        ] {
            assert_eq!(InstanceCommand::parse(command.wire()), Some(command));
            assert!(command.wire().len() <= MAX_INSTANCE_MESSAGE);
        }
        // The updater's command (design m5b D.8): 13 bytes.
        assert_eq!(InstanceCommand::QuitIfIdle.wire(), b"quit-if-idle\n");
        assert_eq!(InstanceCommand::QuitIfIdle.wire().len(), 13);
        for bad in [
            &b"activate"[..],
            b"ACTIVATE\n",
            b"quit\n\n",
            b"",
            b"show\n",
            b"quit-if-idle",
            b"quit-if-idle\n\n",
            b"QUIT-IF-IDLE\n",
            b"quit_if_idle\n",
            b"quit-if-idle \n",
            b"quit-if-idle\r\n",
        ] {
            assert_eq!(InstanceCommand::parse(bad), None, "{bad:?}");
        }
        for reply in [InstanceReply::Ok, InstanceReply::Busy] {
            assert_eq!(InstanceReply::parse(reply.wire()), Some(reply));
        }
    }

    #[test]
    fn instance_pipe_names_of_the_update_runner() {
        let local = "S-1-5-21-1004336348-1177238915-682003330-1001";
        let entra = "S-1-12-1-3915915452-1316434290-2451406996-2913412866";
        for (name, expected) in [
            (
                format!("SHINDATACENTER.MKLM.Instance.1.{local}"),
                Some((1, local)),
            ),
            (
                format!("SHINDATACENTER.MKLM.Instance.4294967295.{entra}"),
                Some((u32::MAX, entra)),
            ),
            (
                "SHINDATACENTER.MKLM.Instance.0.S-1-5-21-0-0-0-0".to_string(),
                Some((0, "S-1-5-21-0-0-0-0")),
            ),
            (
                "SHINDATACENTER.MKLM.Instance.2.S-1-5-21-9999999999-1-2-3".to_string(),
                Some((2, "S-1-5-21-9999999999-1-2-3")),
            ),
        ] {
            assert_eq!(
                parse_instance_pipe_name(&name),
                expected.map(|(session, sid)| (session, sid.to_string())),
                "{name}"
            );
        }
        // What `instance_pipe_path` makes is what the runner accepts (without `\\.\pipe\`).
        let path = instance_pipe_path(3, local);
        assert_eq!(
            parse_instance_pipe_name(path.strip_prefix(r"\\.\pipe\").unwrap()),
            Some((3, local.to_string()))
        );
        for name in [
            String::new(),
            path.clone(),
            format!("SHINDATACENTER.MKLM.Instance.1.{local}.x"),
            format!("SHINDATACENTER.MKLM.Instance.1.{local}-5"),
            format!("SHINDATACENTER.MKLM.Instance.1.{local}/x"),
            format!("SHINDATACENTER.MKLM.Instance.1.{local}\\x"),
            format!("SHINDATACENTER.MKLM.Instance.1.{local}\u{0}"),
            format!("SHINDATACENTER.MKLM.Instance.1.{local} "),
            format!("SHINDATACENTER.MKLM.Instance.1/../{local}"),
            format!("SHINDATACENTER.MKLM.Instance...{local}"),
            format!("SHINDATACENTER.MKLM.Instance..{local}"),
            format!("SHINDATACENTER.MKLM.Instance.12345678901.{local}"),
            format!("SHINDATACENTER.MKLM.Instance.4294967296.{local}"),
            format!("SHINDATACENTER.MKLM.Instance.-1.{local}"),
            format!("SHINDATACENTER.MKLM.Instance.+1.{local}"),
            format!("SHINDATACENTER.MKLM.Instance.１.{local}"),
            format!("shindatacenter.mklm.instance.1.{local}"),
            format!("SHINDATACENTER.MKLM.Instance.1.{}", local.to_lowercase()),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-18".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-21-1-2-3".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-21-1-2-3-4-5".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-21-1-2-3-12345678901".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-21-1--3-4".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-21-1-2-3-4é".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-12-2-1-2-3-4".to_string(),
            "SHINDATACENTER.MKLM.Instance.1.S-1-5-32-544-1-2-3".to_string(),
            "SHINDATACENTER.MKLM.1.S-1-5-21-1-2-3-4".to_string(),
            "..".to_string(),
            ".".to_string(),
        ] {
            assert_eq!(parse_instance_pipe_name(&name), None, "{name:?}");
        }
    }

    // ---- The update runner's side (design m5b D.8, F.2) ----

    fn gui(pid: u32, session_id: u32) -> ImageProcess {
        ImageProcess {
            identity: mklm_core::ProcessIdentity {
                pid,
                creation_time: 1000 + u64::from(pid),
            },
            path_index: 0,
            session_id,
        }
    }

    fn user(session_id: u32, sid: Option<&str>) -> ProcessUser {
        ProcessUser {
            session_id,
            user_sid: sid.map(str::to_string),
        }
    }

    /// FIX-VERIFICATION-5: names computed from the GUI processes, never enumerated.
    #[test]
    fn pipe_candidates_of_the_update_runner() {
        let local = "S-1-5-21-1004336348-1177238915-682003330-1001";
        let entra = "S-1-12-1-3915915452-1316434290-2451406996-2913412866";
        let guis = [
            gui(100, 1),
            gui(200, 2),
            gui(300, 3),
            gui(400, 4),
            gui(500, 5),
            gui(600, 6),
        ];
        let users: HashMap<u32, ProcessUser> = [
            (100, user(1, Some(local))),
            (200, user(2, Some(entra))),
            // No SID.
            (300, user(3, None)),
            // Another session than the process table says.
            (400, user(7, Some(local))),
            // A service account: never a GUI's pipe name.
            (500, user(5, Some("S-1-5-18"))),
            // 600: no row at all.
        ]
        .into_iter()
        .collect();
        let candidates = instance_pipe_candidates(&guis, &users, 16);
        assert_eq!(
            candidates,
            vec![
                (instance_pipe_path(1, local), guis[0].clone()),
                (instance_pipe_path(2, entra), guis[1].clone()),
            ]
        );
        // The same input, the same order; at most `max_pipes`.
        assert_eq!(instance_pipe_candidates(&guis, &users, 16), candidates);
        assert_eq!(
            instance_pipe_candidates(&guis, &users, 1),
            vec![(instance_pipe_path(1, local), guis[0].clone())]
        );
        assert!(instance_pipe_candidates(&guis, &users, 0).is_empty());
        let many: Vec<ImageProcess> = (0..20).map(|i| gui(1000 + i * 4, 1)).collect();
        let many_users: HashMap<u32, ProcessUser> = many
            .iter()
            .map(|g| (g.identity.pid, user(1, Some(local))))
            .collect();
        assert_eq!(instance_pipe_candidates(&many, &many_users, 16).len(), 16);
    }

    /// Nothing runs a file that does not exist: no GUI, nothing opened, nothing sent.
    #[test]
    fn no_gui_no_message() {
        assert_eq!(
            quit_idle_instances(
                r"\Device\HarddiskVolume99\no\such\folder\mklm.exe",
                Duration::from_secs(1),
                Duration::from_secs(2),
                16,
            ),
            Ok(Vec::new())
        );
    }

    /// `quit-if-idle` against a test instance served by this process: the server is verified
    /// (PID, handle, image, session) and answers `busy`; a server of another image gets nothing.
    #[test]
    fn quit_if_idle_reaches_a_verified_server_only() {
        let path = test_pipe();
        let mut server = test_server(&path);
        let me = crate::proc_identity::current_process_identity().expect("identity");
        let session = own_identity().expect("identity").session_id;
        let own_image = crate::proc_identity::process_image_nt_path(me.pid).expect("image");
        let this_gui = ImageProcess {
            identity: me,
            path_index: 0,
            session_id: session,
        };
        let client_path = path.clone();
        let client_gui = this_gui.clone();
        let client_image = own_image.clone();
        let client = thread::spawn(move || {
            quit_one(
                &client_path,
                &client_gui,
                &client_image,
                Duration::from_secs(10),
            )
        });
        assert_eq!(server.next_command(), Ok(InstanceCommand::QuitIfIdle));
        assert_eq!(server.reply(InstanceReply::Busy), Ok(()));
        assert_eq!(
            client.join().expect("client"),
            (Some(me.pid), QuitAnswer::Busy)
        );
        // Another image, another creation time, another session, another PID: nothing is sent.
        let wrong: [(ImageProcess, String); 4] = [
            (
                this_gui.clone(),
                r"\Device\HarddiskVolume3\Program Files\SHIN DATA CENTER\MKLM\mklm.exe".to_string(),
            ),
            (
                ImageProcess {
                    identity: mklm_core::ProcessIdentity {
                        creation_time: me.creation_time + 1,
                        ..me
                    },
                    ..this_gui.clone()
                },
                own_image.clone(),
            ),
            (
                ImageProcess {
                    session_id: session + 1,
                    ..this_gui.clone()
                },
                own_image.clone(),
            ),
            (
                ImageProcess {
                    identity: mklm_core::ProcessIdentity { pid: 4, ..me },
                    ..this_gui.clone()
                },
                own_image.clone(),
            ),
        ];
        for (other, image) in wrong {
            let client_path = path.clone();
            // The server listens meanwhile (a disconnected pipe takes no client until it does).
            let client = thread::spawn(move || {
                quit_one(&client_path, &other, &image, Duration::from_secs(10))
            });
            assert_eq!(
                server.next_command_timeout(Duration::from_millis(2500)),
                Ok(None),
                "nothing was sent"
            );
            let answer = client.join().expect("client");
            assert_eq!(answer.0, Some(me.pid));
            assert_eq!(answer.1, QuitAnswer::NotOurs);
        }
        // No server at all: no answer within the wait.
        let started = Instant::now();
        assert_eq!(
            quit_one(
                &test_pipe(),
                &this_gui,
                &own_image,
                Duration::from_millis(300)
            ),
            (None, QuitAnswer::NoAnswer)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn only_our_own_instance_is_trusted() {
        let ours = ServerIdentity {
            session_id: 2,
            user_sid: "S-1-5-21-1-2-3-1001".into(),
            image: PathBuf::from(r"C:\Program Files\MKLM\mklm.exe"),
        };
        let same = ServerIdentity {
            user_sid: "s-1-5-21-1-2-3-1001".into(),
            image: PathBuf::from(r"c:\program files\mklm\MKLM.EXE"),
            ..ours.clone()
        };
        assert!(is_our_instance(&same, &ours));
        let other_user = ServerIdentity {
            user_sid: "S-1-5-21-1-2-3-1002".into(),
            ..ours.clone()
        };
        let other_session = ServerIdentity {
            session_id: 3,
            ..ours.clone()
        };
        let other_program = ServerIdentity {
            image: PathBuf::from(r"C:\Users\Public\mklm.exe"),
            ..ours.clone()
        };
        for squatter in [other_user, other_session, other_program] {
            assert!(!is_our_instance(&squatter, &ours), "{squatter:?}");
        }
    }

    #[test]
    fn names() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert_eq!(
            instance_pipe_sddl(sid),
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
        assert_eq!(
            instance_mutex_name(sid),
            r"Local\SHINDATACENTER.MKLM.Instance.S-1-5-21-1-2-3-1001"
        );
        assert_eq!(
            instance_pipe_path(2, sid),
            r"\\.\pipe\SHINDATACENTER.MKLM.Instance.2.S-1-5-21-1-2-3-1001"
        );
    }
}
