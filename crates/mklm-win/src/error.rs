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

/// Something the snapshot could not read, which did not stop the snapshot.
///
/// The affected fields are left at `None` / empty. A caller that is about to write must treat
/// any issue as a reason to stop, because a missing value may only look absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadIssue {
    /// What was being read: an instance ID, an interface path or a registry path.
    pub subject: String,
    pub error: Error,
}

impl ReadIssue {
    pub(crate) fn new(subject: impl Into<String>, error: Error) -> Self {
        Self {
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
