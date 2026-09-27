//! Protected directories under `%ProgramData%\SHIN DATA CENTER\MKLM` and the write lock file
//! (M2, plan 2.2, design sections A.3 and D.1).
//!
//! `%ProgramData%` comes from `SHGetKnownFolderPath(FOLDERID_ProgramData)`, never from the
//! environment. Each level from `SHIN DATA CENTER` down is opened with
//! `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` and without `FILE_SHARE_DELETE`, and
//! kept open while in use (so it cannot be renamed or swapped), then validated through the handle:
//! not a reparse point, owner Administrators or SYSTEM, protected DACL, and no ACE that grants any
//! write-type right (`FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `FILE_WRITE_EA`, `FILE_WRITE_ATTRIBUTES`,
//! `DELETE`, `FILE_DELETE_CHILD`, `WRITE_DAC`, `WRITE_OWNER`, `GENERIC_WRITE`, `GENERIC_ALL`) to any
//! SID other than SYSTEM and Administrators. Missing levels are created with the SDDL below.
//!
//! **Squatting** (design review S1). `%ProgramData%` lets every user create folders, so before
//! MKLM's first elevated run anyone can create `SHIN DATA CENTER` (or `MKLM`, or `mklm.lock`)
//! and own it. Refusing to use it would stop every write, recovery included. Instead, a level that
//! fails validation is quarantined, never trusted and never read: with its parent pinned, it is
//! opened with `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` and `DELETE` (a
//! reparse point is renamed as the link itself), renamed with
//! `SetFileInformationByHandle(FileRenameInfo)` to `<name>.untrusted-<uuid>` next to it, and
//! recreated with the protected SDDL; the rename is reported as a warning. If the rename fails
//! (another process holds the folder open without `FILE_SHARE_DELETE`), the operation stops with
//! [`Error::Insecure`] and a message that says to sign out other users or restart and retry
//! (residual risk, design I.18).

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Error;

/// `SHIN DATA CENTER` and `MKLM`: full control for SYSTEM and Administrators (inherited), list and
/// read of the folder itself for Users (not inherited), so that Explorer can reach `Recovery`.
pub const BASE_DIR_SDDL: &str = "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x1200a9;;;BU)";
/// `Recovery`: like the base, plus read for Users on the files, so that they can copy the recovery
/// files to a USB stick (deviation from plan 2.2, see design section G).
pub const RECOVERY_DIR_SDDL: &str =
    "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
/// `logs` and (M5) `Updates`: SYSTEM and Administrators only.
pub const PRIVATE_DIR_SDDL: &str = "O:BAG:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// The lock file: SYSTEM and Administrators only, so that no standard user can open it and hold
/// a byte-range lock (a denial of service on every write).
pub const LOCK_FILE_SDDL: &str = "O:BAG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)";
/// Name of the lock file in the `MKLM` directory.
pub const LOCK_FILE_NAME: &str = "mklm.lock";

/// The directories MKLM uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataDir {
    /// `%ProgramData%\SHIN DATA CENTER\MKLM`
    Base,
    /// `…\MKLM\Recovery`
    Recovery,
    /// `…\MKLM\logs`
    Logs,
    /// `…\MKLM\Updates` (M5)
    Updates,
}

/// `%ProgramData%` from the known-folder API.
pub fn program_data_dir() -> Result<PathBuf, Error> {
    todo!("M2")
}

/// A validated directory; the handles keep every level from `SHIN DATA CENTER` down pinned.
#[derive(Debug)]
pub struct ProtectedDir {
    path: PathBuf,
    handles: Vec<OwnedHandle>,
    /// Levels that failed validation and were renamed out of the way (see the module docs).
    quarantined: Vec<PathBuf>,
}

impl ProtectedDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where squatted levels were moved to while this directory was prepared; the caller reports
    /// each as a warning.
    pub fn quarantined(&self) -> &[PathBuf] {
        &self.quarantined
    }

    /// Writes `name` durably and atomically (design review C10): `WriteFile` into a temporary
    /// file with the directory's inherited DACL, `FlushFileBuffers` on it, then the current file
    /// (if any) is kept as `<name>.prev` (`MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` over the
    /// previous `.prev`), the temporary file is renamed to `name` the same way, and
    /// `FlushFileBuffers` on the pinned directory handle makes the renames durable. `name` must be
    /// a plain file name.
    pub fn replace_file(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        todo!("M2")
    }
}

/// Creates (when missing) and validates `which` and every level above it up to
/// `SHIN DATA CENTER`, quarantining levels that fail validation (module docs).
/// `%ProgramData%` itself is only checked for not being a reparse point.
pub fn ensure_protected_dir(which: DataDir) -> Result<ProtectedDir, Error> {
    todo!("M2")
}

/// The machine-wide write lock (plan 2.2: `LockFileEx`, not a `Global\` mutex).
#[derive(Debug)]
pub struct FileLock {
    file: std::fs::File,
}

impl FileLock {
    /// Opens or creates [`LOCK_FILE_NAME`] in `base` (the [`DataDir::Base`] directory) with
    /// [`LOCK_FILE_SDDL`], checks that it is a regular file with one link and that DACL (a lock
    /// file that fails is quarantined like a directory level), then
    /// retries `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` on byte 0 every
    /// 100 ms until `timeout` ([`Error::Timeout`]). Dropping the lock (or the process dying)
    /// releases it.
    pub fn acquire(base: &ProtectedDir, timeout: Duration) -> Result<Self, Error> {
        todo!("M2")
    }
}
