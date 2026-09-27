//! Error type of the Windows layer.

use std::fmt;

use windows::Win32::Foundation::WIN32_ERROR;

/// `ERROR_FILE_NOT_FOUND`: a registry key or value does not exist.
pub(crate) const ERROR_FILE_NOT_FOUND: u32 = 2;

/// A failed Win32 call, with the code Windows returned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A CfgMgr32 function returned something other than `CR_SUCCESS`.
    #[error("{function} failed with CONFIGRET {code:#04x}")]
    ConfigRet { function: &'static str, code: u32 },
    /// A Win32 function failed; `code` is the `WIN32_ERROR` from `GetLastError`.
    #[error("{function} failed with Win32 error {code}")]
    Win32 { function: &'static str, code: u32 },
    /// A native API returned a failing `NTSTATUS`.
    #[error("{function} failed with NTSTATUS {status:#010x}")]
    NtStatus { function: &'static str, status: u32 },
    /// A registry key or value could not be read. `code` is the `WIN32_ERROR`
    /// (or the raw HRESULT when it does not wrap one).
    #[error("reading {path} failed with Win32 error {code}")]
    Registry { path: String, code: u32 },
    /// A registry value exists but has an unexpected type or size.
    #[error("{path} has an unexpected registry type or size")]
    UnexpectedData { path: String },
    /// A device property exists but has an unexpected type or size.
    #[error("device property has type {ty:#x} and {size} bytes, expected type {expected:#x}")]
    UnexpectedProperty { ty: u32, size: usize, expected: u32 },
    /// An instance ID resolved to another devnode.
    #[error("resolved to another devnode, {found}")]
    OtherDevNode { found: String },
    /// A write was requested on a devnode outside the Keyboard setup class (M2).
    #[error("{instance_id} is not a Keyboard-class devnode")]
    NotKeyboard { instance_id: String },
    /// The user declined the UAC prompt (`ERROR_CANCELLED` from `ShellExecuteExW`) (M2).
    #[error("the elevation prompt was declined")]
    Cancelled,
    /// A wait ran out (pipe connect/read, lock, device arrival) (M2).
    #[error("{operation} timed out")]
    Timeout { operation: &'static str },
    /// A protected directory, file, registry key or pipe peer failed validation (M2).
    #[error("{path}: insecure ({reason})")]
    Insecure { path: String, reason: String },
    /// The process at the other end of the pipe is not the expected one (M2).
    #[error("pipe peer is process {found}, expected {expected}")]
    PeerMismatch { expected: u32, found: u32 },
    /// `regwrite` refuses a value name outside `mklm_core::DEVICE_VALUE_NAMES` /
    /// `GLOBAL_VALUE_NAMES` (M2, last line of defence under the engine's checks).
    #[error("{name:?} is not a value MKLM may write on {path}")]
    ValueNotAllowed { path: String, name: String },
}

impl Error {
    /// Wraps an error from `windows-registry` (or any `windows::core::Error`) for `path`.
    pub(crate) fn registry(path: impl Into<String>, error: &windows::core::Error) -> Self {
        Self::Registry {
            path: path.into(),
            code: win32_code(error),
        }
    }

    /// `WIN32_ERROR` of a `Win32` or `Registry` error.
    pub fn win32_code(&self) -> Option<u32> {
        match self {
            Self::Win32 { code, .. } | Self::Registry { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// `CONFIGRET` of a CfgMgr32 error.
    pub fn config_ret(&self) -> Option<u32> {
        match self {
            Self::ConfigRet { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// `WIN32_ERROR` wrapped in an HRESULT, else the HRESULT itself.
pub(crate) fn win32_code(error: &windows::core::Error) -> u32 {
    WIN32_ERROR::from_error(error).map_or(error.code().0 as u32, |code| code.0)
}

/// True when a registry error means "the key or value does not exist".
pub(crate) fn is_not_found(error: &windows::core::Error) -> bool {
    win32_code(error) == ERROR_FILE_NOT_FOUND
}

/// What a [`ReadIssue`] failed to read, which decides whether a writer may go on (M2, design
/// review S2 and C3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadIssueKind {
    /// A Keyboard-class devnode could not be located (other than "no such devnode").
    Locate,
    /// The devnode's own instance ID could not be read or resolved to another devnode.
    Identity,
    /// `DEVPKEY_Device_Service` (the driver decides the value names and INV-PS2).
    Driver,
    /// `DEVPKEY_Device_IsPresent`.
    Presence,
    /// `DEVPKEY_Device_DevNodeStatus` / `ProblemCode`: the field stays `None`, which bans a live
    /// reset of that keyboard (`StatusUnknown`).
    Status,
    /// `DEVPKEY_Device_ContainerId`: `None` bans a live reset (`UnknownContainer`) and keeps the
    /// assignment to the keyboard itself.
    Container,
    /// Ancestors (parent chain, bus service): the transport may end up `Unknown`, which bans a
    /// live reset.
    Topology,
    /// Names and hardware IDs: display only.
    Descriptive,
    /// Override or global values (a value of an unexpected type, an unreadable key). The engine
    /// re-reads every value itself and keeps unexpected types as `RegValue::Other`.
    Values,
    /// Raw Input: the reported type is unknown; confirmation shows "not verified".
    RawInput,
    /// Input methods and OS facts: display only.
    Environment,
}

impl ReadIssueKind {
    /// True when a writer must stop: without these, the set of Keyboard-class devnodes, their
    /// drivers or their presence is unknown, and INV-PS2 or the allowlist could be judged on a
    /// wrong inventory. Every other kind is a warning.
    pub fn blocks_writes(self) -> bool {
        matches!(
            self,
            Self::Locate | Self::Identity | Self::Driver | Self::Presence
        )
    }
}

/// Something the snapshot could not read, which did not stop the snapshot.
///
/// The affected fields are left at `None` / empty. A caller that is about to write stops on the
/// issues whose [`ReadIssueKind::blocks_writes`] is true and reports the others as warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadIssue {
    pub kind: ReadIssueKind,
    /// What was being read: an instance ID, an interface path or a registry path.
    pub subject: String,
    pub error: Error,
}

impl ReadIssue {
    pub(crate) fn new(kind: ReadIssueKind, subject: impl Into<String>, error: Error) -> Self {
        Self {
            kind,
            subject: subject.into(),
            error,
        }
    }
}

impl fmt::Display for ReadIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.error)
    }
}
