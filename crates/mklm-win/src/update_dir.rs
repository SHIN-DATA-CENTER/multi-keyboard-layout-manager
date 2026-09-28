//! `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>` (design m5b D.5, D.11): the run folders
//! with an explicit private DACL, validated and pinned, the staged files, the clean-up, the build
//! IDs of the installed executables and the holders of files in use.
//!
//! WP-H implements it.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use std::io;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::protected_dir::ProtectedDir;

/// One `Updates\<run-id>` folder, validated and pinned.
#[derive(Debug)]
pub struct RunDir {
    /// Skeleton (M5b): WP-H adds the pinning handles.
    path: PathBuf,
}

impl RunDir {
    /// `CreateDirectoryW` with `PRIVATE_DIR_SDDL`; fails if it exists.
    pub fn create(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error> {
        Err(skeleton("RunDir::create (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// Validates owner, DACL and no reparse point, and pins it.
    pub fn open(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error> {
        Err(skeleton("RunDir::open (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A subfolder (`tmp`) with `PRIVATE_DIR_SDDL`, validated and pinned like the run folder.
    pub fn create_private_subdir(&self, name: &str) -> Result<PathBuf, Error> {
        Err(skeleton("RunDir::create_private_subdir (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// `CREATE_NEW`, write, `FlushFileBuffers`.
    pub fn write_new(&self, file: &str, bytes: &[u8]) -> Result<(), Error> {
        Err(skeleton("RunDir::write_new (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// `CREATE_NEW`, no sharing, for streaming.
    pub fn create_exclusive(&self, file: &str) -> Result<StagedFile, Error> {
        Err(skeleton("RunDir::create_exclusive (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// A regular file (not a reparse point) of at most `max_len` bytes, opened with
    /// `FILE_SHARE_READ` only.
    pub fn open_locked(&self, file: &str, max_len: u64) -> Result<LockedFile, Error> {
        Err(skeleton("RunDir::open_locked (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// Copies `source` in as `file` (`CREATE_NEW`, flushed).
    pub fn copy_in(&self, source: &Path, file: &str) -> Result<(), Error> {
        Err(skeleton("RunDir::copy_in (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    pub fn remove_file(&self, file: &str) -> Result<(), Error> {
        Err(skeleton("RunDir::remove_file (m5b skeleton)")) // Skeleton (M5b): WP-H
    }
}

/// A staged file being written; deleted on drop unless committed.
#[derive(Debug)]
pub struct StagedFile {
    /// Skeleton (M5b): WP-H keeps the file handle here.
    _private: (),
}

impl io::Write for StagedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Err(io_skeleton()) // Skeleton (M5b): WP-H
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io_skeleton()) // Skeleton (M5b): WP-H
    }
}

impl StagedFile {
    /// `FlushFileBuffers` and close.
    pub fn commit(self) -> Result<(), Error> {
        Err(skeleton("StagedFile::commit (m5b skeleton)")) // Skeleton (M5b): WP-H
    }
}

/// A staged file held open with `FILE_SHARE_READ` only.
#[derive(Debug)]
pub struct LockedFile {
    /// Skeleton (M5b): WP-H keeps the file handle here.
    path: PathBuf,
    len: u64,
}

impl io::Read for LockedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Err(io_skeleton()) // Skeleton (M5b): WP-H
    }
}

impl LockedFile {
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Not in design m5b H.3; `len` without it fails clippy (`len_without_is_empty`).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Back to offset 0 (to hash, then keep the handle).
    pub fn rewind(&mut self) -> Result<(), Error> {
        Err(skeleton("LockedFile::rewind (m5b skeleton)")) // Skeleton (M5b): WP-H
    }
}

pub fn list_run_dirs(updates: &ProtectedDir) -> Result<Vec<String>, Error> {
    Err(skeleton("list_run_dirs (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Regular files only, never following reparse points.
pub fn remove_run_dir(updates: &ProtectedDir, name: &str) -> Result<(), Error> {
    Err(skeleton("remove_run_dir (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Every run folder except `keep` whose `mklm-update-runner.exe` is not running; the names
/// removed (design m5b D.11; replaces `remove_at_reboot`, RELIABILITY-9).
pub fn sweep_stale_run_dirs(
    updates: &ProtectedDir,
    keep: Option<&str>,
) -> Result<Vec<String>, Error> {
    Err(skeleton("sweep_stale_run_dirs (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Build IDs of `mklm.exe`, `mklm-cli.exe`, `mklm-helper.exe` in `install_dir` (in that order;
/// `None` when missing or unreadable). Callers convert with `InstallState::from_build_ids`.
pub fn read_build_ids(install_dir: &Path) -> [Option<String>; 3] {
    [None, None, None] // Skeleton (M5b): WP-H
}

/// Indices into `names` of the files in `install_dir` that cannot be opened with `DELETE` and
/// full sharing (another handle lacks FILE_SHARE_DELETE). Missing files are not in use.
pub fn files_in_use(install_dir: &Path, names: &[&str]) -> Result<Vec<usize>, Error> {
    Err(skeleton("files_in_use (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Who holds `paths` open (Restart Manager: `RmStartSession`, `RmRegisterResources`, `RmGetList`,
/// `RmEndSession`): (PID, session ID, application name) each (RED-TEAM-3). Best effort; the
/// callers treat an error as "unknown". Whether `RmGetList` reports plain open handles (not only
/// loaded images) is unverified (design m5b I.16).
pub fn file_holders(paths: &[PathBuf]) -> Result<Vec<(u32, u32, String)>, Error> {
    Err(skeleton("file_holders (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// `GetDiskFreeSpaceExW` for the caller: free bytes on the volume of `path`.
pub fn free_space(path: &Path) -> Result<u64, Error> {
    Err(skeleton("free_space (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// What the WP-0 skeleton returns (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton(function: &'static str) -> Error {
    Error::Win32 { function, code: 50 }
}

fn io_skeleton() -> io::Error {
    io::Error::other("not implemented (m5b skeleton)")
}
