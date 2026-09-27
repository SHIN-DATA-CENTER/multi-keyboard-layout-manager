//! The helper's command line (plan 2.2): only the pipe name, a nonce and the caller's PID, in one
//! fixed format that [`HelperArgs::parse`] validates as strictly as [`HELPER_ARGS_PATTERN`].

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use std::fmt;

/// Prefix of every pipe path.
pub const PIPE_PATH_PREFIX: &str = r"\\.\pipe\";
/// Prefix of MKLM's helper pipe names; a lower-case hyphenated UUID follows.
pub const PIPE_NAME_PREFIX: &str = "SHINDATACENTER.MKLM.";
/// Length of the nonce in bytes (64 hex digits on the command line).
pub const NONCE_LEN: usize = 32;

/// The exact grammar of the helper's arguments (everything after the program name), anchored.
/// [`HelperArgs::parse`] implements it by hand (no regex dependency) and its tests check both.
pub const HELPER_ARGS_PATTERN: &str = r"^--pipe SHINDATACENTER\.MKLM\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12} --nonce [0-9a-f]{64} --caller-pid [1-9][0-9]{0,9}$";

/// A pipe name `SHINDATACENTER.MKLM.<uuid>` (without the `\\.\pipe\` prefix).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipeName(String);

impl PipeName {
    /// Builds the name from a freshly generated UUID (lower-case, hyphenated).
    pub fn from_uuid(uuid: &str) -> Result<Self, ArgsError> {
        todo!("M2")
    }

    /// Validates a full name.
    pub fn parse(name: &str) -> Result<Self, ArgsError> {
        todo!("M2")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `\\.\pipe\SHINDATACENTER.MKLM.<uuid>`.
    pub fn path(&self) -> String {
        format!("{PIPE_PATH_PREFIX}{}", self.0)
    }
}

/// Random bytes that bind one helper launch to one caller. `Debug` never prints them.
#[derive(Clone, PartialEq, Eq)]
pub struct Nonce([u8; NONCE_LEN]);

impl Nonce {
    pub fn from_bytes(bytes: [u8; NONCE_LEN]) -> Self {
        Self(bytes)
    }

    /// Parses exactly 64 lower-case hex digits.
    pub fn parse_hex(text: &str) -> Result<Self, ArgsError> {
        todo!("M2")
    }

    /// 64 lower-case hex digits.
    pub fn to_hex(&self) -> String {
        todo!("M2")
    }

    /// Constant-time comparison.
    pub fn matches(&self, other: &Nonce) -> bool {
        todo!("M2")
    }
}

impl fmt::Debug for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Nonce(..)")
    }
}

/// The helper's parsed arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperArgs {
    pub pipe: PipeName,
    pub nonce: Nonce,
    /// PID of the caller that created the pipe; the helper checks it against
    /// `GetNamedPipeServerProcessId`.
    pub caller_pid: u32,
}

impl HelperArgs {
    /// Parses the arguments part of the raw command line (see [`command_line_tail`]) and accepts
    /// nothing but [`HELPER_ARGS_PATTERN`]: single spaces, this order, no quotes, no extra
    /// arguments, a PID in 1..=u32::MAX.
    pub fn parse(tail: &str) -> Result<Self, ArgsError> {
        todo!("M2")
    }

    /// The `lpParameters` string for `ShellExecuteExW`; `parse` accepts exactly this.
    pub fn to_parameters(&self) -> String {
        todo!("M2")
    }
}

/// Strips the program name from a raw `GetCommandLineW` string (quoted or unquoted, as the CRT
/// parses argv\[0\]) and the single space after it. `None` when nothing follows.
pub fn command_line_tail(command_line: &str) -> Option<&str> {
    todo!("M2")
}

/// The helper's command line is not in the fixed format. Deliberately says nothing about which
/// part failed: the helper logs it and exits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    #[error("the helper's command line is malformed")]
    Malformed,
}
