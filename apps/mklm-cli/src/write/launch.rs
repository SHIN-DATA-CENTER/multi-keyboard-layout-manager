//! Starting a helper session (design E.1 to E.3): check the helper's build ID, create the pipe,
//! launch `mklm-helper.exe` (UAC when unelevated, `CreateProcessW` when elevated), accept only the
//! launched process, and run the handshake. The result is a [`Link`] for the relay.

use std::path::PathBuf;
use std::time::Duration;

use mklm_ipc::{
    CONNECT_TIMEOUT, CallerMessage, FrameError, FrameReader, FrameSequencer, HANDSHAKE_TIMEOUT,
    HelperArgs, HelperMessage, MESSAGE_TIMEOUT, NONCE_LEN, Nonce, PipeName, Welcome, verify_hello,
    write_frame,
};
use mklm_win::elevation::{self, ElevatedProcess};
use mklm_win::pipe::{HELPER_PIPE_SDDL, PipeConnection, PipeServer};
use mklm_win::session;

use super::BUILD_ID;
use super::outcome::helper_exit_text;
use super::relay::{Link, Recv};

/// How long the CLI waits for the helper to exit after `Bye`.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// Why no session came up.
#[derive(Debug)]
pub enum LaunchError {
    /// The user declined the UAC prompt (exit code 3).
    Declined,
    /// Anything else, with the message to show (exit code 1).
    Failed(String),
}

impl LaunchError {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// A connected, verified helper.
#[derive(Debug)]
pub struct HelperSession {
    pipe: PipeConnection,
    helper: ElevatedProcess,
    sequencer: FrameSequencer,
    reader: FrameReader,
}

/// The helper next to this executable, checked to carry this build's ID before anything is
/// launched (design E.3 step 1, review S11).
pub fn checked_helper_path() -> Result<PathBuf, LaunchError> {
    let helper = elevation::helper_path().map_err(|error| {
        LaunchError::failed(format!(
            "mklm-helper.exe was not found next to mklm-cli.exe ({error})"
        ))
    })?;
    let found = elevation::file_build_id(&helper).map_err(|error| {
        LaunchError::failed(format!(
            "could not read the build ID of {} ({error}); reinstall MKLM (during development, \
             rebuild both executables)",
            helper.display()
        ))
    })?;
    if found != BUILD_ID {
        return Err(LaunchError::failed(format!(
            "the helper is from another build ({found}, this is {BUILD_ID}); reinstall MKLM \
             (during development, rebuild both executables with `cargo build --workspace`)"
        )));
    }
    Ok(helper)
}

/// Creates the pipe, launches the helper and runs the handshake (design E.1).
pub fn start(elevated: bool) -> Result<HelperSession, LaunchError> {
    let helper_path = checked_helper_path()?;
    let uuid = session::new_uuid()
        .map_err(|error| LaunchError::failed(format!("could not make a pipe name: {error}")))?;
    let pipe_name = PipeName::from_uuid(&uuid)
        .map_err(|error| LaunchError::failed(format!("could not make a pipe name: {error}")))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    session::random_bytes(&mut nonce_bytes)
        .map_err(|error| LaunchError::failed(format!("could not make a nonce: {error}")))?;
    let nonce = Nonce::from_bytes(nonce_bytes);
    let server = PipeServer::create(&pipe_name.path(), HELPER_PIPE_SDDL)
        .map_err(|error| LaunchError::failed(format!("could not create the pipe: {error}")))?;
    let args = HelperArgs {
        pipe: pipe_name,
        nonce: nonce.clone(),
        caller_pid: std::process::id(),
    };
    let parameters = args.to_parameters();
    let helper = if elevated {
        // Already elevated (an administrator console, Safe Mode): no UAC prompt, and still a
        // separate process so that closing this console reverts a countdown (design review C8).
        elevation::spawn_from_elevated(&helper_path, &parameters)
    } else {
        elevation::launch_elevated(&helper_path, &parameters, elevation::console_window())
    }
    .map_err(|error| match error {
        mklm_win::Error::Cancelled => LaunchError::Declined,
        error => LaunchError::failed(format!("could not start the helper: {error}")),
    })?;

    let mut pipe = server
        .accept(helper.pid(), CONNECT_TIMEOUT, Some(&helper))
        .map_err(|error| match error {
            mklm_win::Error::PeerExited { exit_code, .. } => LaunchError::failed(format!(
                "{} before it connected",
                helper_exit_text(exit_code)
            )),
            error => LaunchError::failed(format!("the helper did not connect: {error}")),
        })?;
    pipe.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    pipe.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let mut reader = FrameReader::new();
    let hello = match reader.read::<_, HelperMessage>(&mut pipe) {
        Ok(frame) => match frame.body {
            HelperMessage::Hello(hello) => hello,
            other => {
                return Err(LaunchError::failed(format!(
                    "the helper did not greet first: {other:?}"
                )));
            }
        },
        Err(error) => {
            return Err(LaunchError::failed(format!(
                "no greeting from the helper: {error}"
            )));
        }
    };
    verify_hello(&hello, &nonce, helper.pid(), BUILD_ID)
        .map_err(|error| LaunchError::failed(format!("the helper failed the check: {error}")))?;
    let mut sequencer = FrameSequencer::new();
    let welcome = CallerMessage::Welcome(Welcome::new(std::process::id(), BUILD_ID));
    write_frame(&mut pipe, &sequencer.frame(welcome))
        .map_err(|error| LaunchError::failed(format!("could not answer the helper: {error}")))?;
    // Frames are small and the helper reads all the time; a write that blocks this long means the
    // helper is wedged.
    pipe.set_write_timeout(Some(MESSAGE_TIMEOUT));
    Ok(HelperSession {
        pipe,
        helper,
        sequencer,
        reader,
    })
}

impl HelperSession {
    /// Says `Bye` and gives the helper a moment to exit (it may still wait for a keyboard reset
    /// that outlived its deadline, design review C18; it does not need this process for that).
    pub fn close(mut self) {
        let _ = self.send(&CallerMessage::Bye);
        let _ = self.helper.wait(EXIT_WAIT);
    }
}

impl Link for HelperSession {
    fn send(&mut self, message: &CallerMessage) -> Result<(), String> {
        write_frame(&mut self.pipe, &self.sequencer.frame(message.clone()))
            .map_err(|error| error.to_string())
    }

    fn recv(&mut self, timeout: Duration) -> Recv {
        self.pipe
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))));
        match self.reader.read::<_, HelperMessage>(&mut self.pipe) {
            Ok(frame) => Recv::Message(frame.body),
            Err(FrameError::Timeout) => Recv::Timeout,
            Err(FrameError::Closed) => Recv::Closed("the helper closed the connection".into()),
            Err(error) => Recv::Closed(error.to_string()),
        }
    }

    fn helper_exit_code(&mut self) -> Option<u32> {
        self.helper.exit_code().ok().flatten()
    }
}
