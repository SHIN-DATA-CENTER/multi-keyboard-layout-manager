//! Starting a helper session (design m2 E.1 to E.3): check the helper's build ID, create the pipe,
//! launch `mklm-helper.exe` (UAC when unelevated, `CreateProcessW` when elevated), accept only
//! the launched process, and run the handshake. The result is a [`HelperSession`], a
//! [`Link`] for the relay.
//!
//! Blocking: [`start`] waits while the UAC prompt is shown and until the helper connects (up to
//! `CONNECT_TIMEOUT`). The GUI calls it on its session worker thread, never on the UI thread.

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

use crate::HelperExit;
use crate::orchestrator::{HelperLink, LaunchError, LaunchFailure, Launcher};
use crate::session::{Link, Recv};

/// How long the caller waits for the helper to exit after `Bye`.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// How to launch the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    /// This build's ID (`env!("MKLM_BUILD_ID")` of the executable, apps/build_id.rs): the helper
    /// must carry the same (design m2 E.3, review S11).
    pub build_id: String,
    /// This process runs elevated (an administrator console, Safe Mode): `CreateProcessW`, no
    /// UAC prompt, still a separate process (design review C8).
    pub elevated: bool,
    /// The window that owns the UAC prompt: the GUI's main window, the CLI's console window.
    pub owner_window: Option<isize>,
}

impl LaunchConfig {
    /// For the current process: elevation as `mklm_win::elevation::is_elevated` says (`None`
    /// when it cannot be checked).
    pub fn current(build_id: &str, owner_window: Option<isize>) -> Result<Self, mklm_win::Error> {
        Ok(Self {
            build_id: build_id.to_string(),
            elevated: elevation::is_elevated()?,
            owner_window,
        })
    }
}

fn failed(kind: LaunchFailure, message: impl Into<String>) -> LaunchError {
    LaunchError::Failed {
        kind,
        message: message.into(),
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

/// The helper next to this executable, checked to carry `build_id` before anything is launched
/// (design m2 E.3 step 1, review S11).
pub fn checked_helper_path(build_id: &str) -> Result<PathBuf, LaunchError> {
    let helper = elevation::helper_path().map_err(|error| {
        let this = std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "this program".to_string());
        failed(
            LaunchFailure::HelperMissing,
            format!("mklm-helper.exe was not found next to {this} ({error})"),
        )
    })?;
    let found = elevation::file_build_id(&helper).map_err(|error| {
        failed(
            LaunchFailure::OtherBuild,
            format!(
                "could not read the build ID of {} ({error}); reinstall MKLM (during development, \
                 rebuild both executables)",
                helper.display()
            ),
        )
    })?;
    if found != build_id {
        return Err(failed(
            LaunchFailure::OtherBuild,
            format!(
                "the helper is from another build ({found}, this is {build_id}); reinstall MKLM \
                 (during development, rebuild both executables with `cargo build --workspace`)"
            ),
        ));
    }
    Ok(helper)
}

/// Creates the pipe, launches the helper and runs the handshake (design m2 E.1).
pub fn start(config: &LaunchConfig) -> Result<HelperSession, LaunchError> {
    let helper_path = checked_helper_path(&config.build_id)?;
    let uuid = session::new_uuid().map_err(|error| {
        failed(
            LaunchFailure::Setup,
            format!("could not make a pipe name: {error}"),
        )
    })?;
    let pipe_name = PipeName::from_uuid(&uuid).map_err(|error| {
        failed(
            LaunchFailure::Setup,
            format!("could not make a pipe name: {error}"),
        )
    })?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    session::random_bytes(&mut nonce_bytes).map_err(|error| {
        failed(
            LaunchFailure::Setup,
            format!("could not make a nonce: {error}"),
        )
    })?;
    let nonce = Nonce::from_bytes(nonce_bytes);
    let server = PipeServer::create(&pipe_name.path(), HELPER_PIPE_SDDL).map_err(|error| {
        failed(
            LaunchFailure::Setup,
            format!("could not create the pipe: {error}"),
        )
    })?;
    let args = HelperArgs {
        pipe: pipe_name,
        nonce: nonce.clone(),
        caller_pid: std::process::id(),
    };
    let parameters = args.to_parameters();
    let helper = if config.elevated {
        // Already elevated (an administrator console, Safe Mode): no UAC prompt, and still a
        // separate process so that closing this console reverts a countdown (design review C8).
        elevation::spawn_from_elevated(&helper_path, &parameters)
    } else {
        elevation::launch_elevated(&helper_path, &parameters, config.owner_window)
    }
    .map_err(|error| match error {
        mklm_win::Error::Cancelled => LaunchError::Declined,
        error => failed(
            LaunchFailure::StartFailed(error.win32_code()),
            format!("could not start the helper: {error}"),
        ),
    })?;

    let mut pipe = server
        .accept(helper.pid(), CONNECT_TIMEOUT, Some(&helper))
        .map_err(|error| match error {
            mklm_win::Error::PeerExited { exit_code, .. } => failed(
                LaunchFailure::ExitedEarly(HelperExit(exit_code)),
                format!("{} before it connected", HelperExit(exit_code)),
            ),
            error => failed(
                LaunchFailure::NoConnection,
                format!("the helper did not connect: {error}"),
            ),
        })?;
    pipe.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    pipe.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let mut reader = FrameReader::new();
    let hello = match reader.read::<_, HelperMessage>(&mut pipe) {
        Ok(frame) => match frame.body {
            HelperMessage::Hello(hello) => hello,
            other => {
                return Err(failed(
                    LaunchFailure::Handshake,
                    format!("the helper did not greet first: {other:?}"),
                ));
            }
        },
        Err(error) => {
            return Err(failed(
                LaunchFailure::Handshake,
                format!("no greeting from the helper: {error}"),
            ));
        }
    };
    verify_hello(&hello, &nonce, helper.pid(), &config.build_id).map_err(|error| {
        failed(
            LaunchFailure::Handshake,
            format!("the helper failed the check: {error}"),
        )
    })?;
    let mut sequencer = FrameSequencer::new();
    let welcome = CallerMessage::Welcome(Welcome::new(std::process::id(), &config.build_id));
    write_frame(&mut pipe, &sequencer.frame(welcome)).map_err(|error| {
        failed(
            LaunchFailure::Handshake,
            format!("could not answer the helper: {error}"),
        )
    })?;
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

impl HelperLink for HelperSession {
    fn close(self: Box<Self>) {
        HelperSession::close(*self);
    }
}

/// The real [`Launcher`]: [`start`] with a fixed configuration.
#[derive(Debug, Clone)]
pub struct HelperLauncher {
    pub config: LaunchConfig,
}

impl Launcher for HelperLauncher {
    fn launch(&mut self) -> Result<Box<dyn HelperLink>, LaunchError> {
        start(&self.config).map(|session| Box::new(session) as Box<dyn HelperLink>)
    }
}
