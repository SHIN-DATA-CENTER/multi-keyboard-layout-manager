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
//!   pipe. It accepts exactly two commands, one per connection: `activate` and `quit`
//!   ([`InstanceCommand`]), each within [`INSTANCE_READ_TIMEOUT`] (a client that connects and
//!   stays silent is disconnected), and answers `ok` or `busy`.
//! - A second process connects (`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`) and verifies
//!   the server before it trusts it ([`ServerIdentity`], [`is_our_instance`]): the server's
//!   session (`GetNamedPipeServerSessionId`), the user SID of the server process's token
//!   (`GetNamedPipeServerProcessId` → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` →
//!   `OpenProcessToken`) and its image (`QueryFullProcessImageNameW`) must all be this process's.
//!   Only then does it call `AllowSetForegroundWindow` for that PID and send its command. On a
//!   mismatch it sends nothing and runs as the instance itself. The M5 elevated sender (the
//!   updater) must make the same checks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Error;

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
    /// Quit cleanly (`mklm.exe --quit`: the M5 installer and updater).
    Quit,
}

impl InstanceCommand {
    /// The bytes on the wire.
    pub fn wire(self) -> &'static [u8] {
        match self {
            InstanceCommand::Activate => b"activate\n",
            InstanceCommand::Quit => b"quit\n",
        }
    }

    /// Parses one received message; anything but the two exact commands is `None`.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"activate\n" => Some(InstanceCommand::Activate),
            b"quit\n" => Some(InstanceCommand::Quit),
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

/// Which role this process has.
#[derive(Debug)]
pub enum InstanceRole {
    /// This process is the instance; keep the guard for the process lifetime.
    Primary(InstanceGuard),
    /// Another process is the instance.
    Secondary,
}

/// Holds the instance mutex.
#[derive(Debug)]
pub struct InstanceGuard {
    _private: (),
}

/// Creates or opens the instance mutex (`CreateMutexW`; `ERROR_ALREADY_EXISTS` → secondary;
/// `ERROR_ACCESS_DENIED` → primary without a guard, logged by the caller).
///
/// Implementation (WP-W1): user SID from `elevation::current_user_sid`, session ID from
/// `ProcessIdToSessionId(GetCurrentProcessId())`.
pub fn acquire_instance() -> Result<InstanceRole, Error> {
    todo!("WP-W1: CreateMutexW(Local\\SHINDATACENTER.MKLM.Instance.<SID>)")
}

/// The running instance's pipe server.
#[derive(Debug)]
pub struct InstanceServer {
    _private: (),
}

impl InstanceServer {
    /// Creates the pipe (first instance only). Implementation (WP-W1): `pipe::PipeServer::create`
    /// with [`instance_pipe_sddl`], plus an `accept_any` that accepts any client (the helper
    /// pipe's `accept` checks one expected PID; here the DACL and the fixed commands are the
    /// defence).
    pub fn create() -> Result<Self, Error> {
        todo!("WP-W1: instance pipe server")
    }

    /// Waits for the next command (blocking; run on its own thread). A malformed message closes
    /// that connection and waits for the next one.
    pub fn next_command(&mut self) -> Result<InstanceCommand, Error> {
        todo!("WP-W1: accept, read at most MAX_INSTANCE_MESSAGE bytes, parse")
    }

    /// Answers the command just received and closes the connection.
    pub fn reply(&mut self, reply: InstanceReply) -> Result<(), Error> {
        let _ = reply;
        todo!("WP-W1: write the reply, DisconnectNamedPipe")
    }
}

/// Sends `command` to the running instance (a second process). Retries the connection for up to
/// `timeout`, because at sign-in the instance may hold the mutex a moment before its pipe exists
/// (Run and RunOnce start together, design m3 F.2). Fails with [`Error::Insecure`] when the
/// server is not [`is_our_instance`]; nothing is sent then and the caller runs as the instance.
pub fn send_to_instance(
    command: InstanceCommand,
    timeout: Duration,
) -> Result<InstanceReply, Error> {
    let _ = (command, timeout);
    todo!(
        "WP-W1: connect, verify ServerIdentity, AllowSetForegroundWindow(server PID), write, \
         read the reply"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_commands_are_accepted() {
        for command in [InstanceCommand::Activate, InstanceCommand::Quit] {
            assert_eq!(InstanceCommand::parse(command.wire()), Some(command));
            assert!(command.wire().len() <= MAX_INSTANCE_MESSAGE);
        }
        for bad in [&b"activate"[..], b"ACTIVATE\n", b"quit\n\n", b"", b"show\n"] {
            assert_eq!(InstanceCommand::parse(bad), None);
        }
        for reply in [InstanceReply::Ok, InstanceReply::Busy] {
            assert_eq!(InstanceReply::parse(reply.wire()), Some(reply));
        }
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
