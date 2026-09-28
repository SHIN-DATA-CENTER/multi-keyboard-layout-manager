//! Protected directories under `%ProgramData%\SHIN DATA CENTER\MKLM` and the write lock file
//! (M2, plan 2.2, design sections A.3 and D.1).
//!
//! `%ProgramData%` comes from `SHGetKnownFolderPath(FOLDERID_ProgramData)`, never from the
//! environment. Each level from `SHIN DATA CENTER` down is opened with
//! `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`, kept open while in use, and
//! validated through that handle: not a reparse point, owner Administrators or SYSTEM, protected
//! DACL, and no ACE that grants any write-type right (`FILE_WRITE_DATA`, `FILE_APPEND_DATA`,
//! `FILE_WRITE_EA`, `FILE_WRITE_ATTRIBUTES`, `DELETE`, `FILE_DELETE_CHILD`, `WRITE_DAC`,
//! `WRITE_OWNER`, `GENERIC_WRITE`, `GENERIC_ALL`) to any SID other than SYSTEM and Administrators.
//! Missing levels are created with the SDDL below. A validated level cannot be renamed or swapped
//! by anyone but SYSTEM and Administrators, because that needs `DELETE` on it or
//! `FILE_DELETE_CHILD` on its parent.
//!
//! The pinning handles ask for no data access (`READ_CONTROL | FILE_READ_ATTRIBUTES |
//! SYNCHRONIZE`), so they take no part in the file system's share-access check: users may list
//! these folders, and a user who holds one open with any share mode cannot make the pin fail
//! (which would stop every write, recovery included). Only the short-lived handle that flushes the
//! `Recovery` directory after a replace asks for `FILE_ADD_FILE`.
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

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE, FILE_ADD_FILE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_CREATION_DISPOSITION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileAttributeTagInfo,
    FileRenameInfo, FileStandardInfo, FlushFileBuffers, GetFileInformationByHandleEx,
    LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE,
    SetFileInformationByHandle, UnlockFileEx,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::IO::OVERLAPPED;
use windows::Win32::UI::Shell::{FOLDERID_ProgramData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::PCWSTR;

use crate::error::{Error, win32_code};
use crate::security::{
    AclPolicy, DIRECTORY_POLICY, LOCK_FILE_POLICY, LocalSd, check_descriptor, file_security,
};
use crate::session::new_uuid;
use crate::sys::{own, raw, wide_os, win32};

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

/// How often a level may be quarantined in one call before giving up (someone keeps recreating
/// it in the moment between the rename and the new folder).
const MAX_QUARANTINES: usize = 3;

/// Access of the handles that pin each level: validation needs `READ_CONTROL` and the attributes.
/// No data access (`FILE_LIST_DIRECTORY`, `FILE_TRAVERSE`, `FILE_ADD_FILE`, `DELETE`): those take
/// part in the share-access check, and Users may open these folders, so any of them would let a
/// standard user block the pin with a handle of their own (module docs).
pub(crate) const PIN_ACCESS: u32 = READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0;
/// Share mode of the pins and of the lock file (whose handle does ask for data access).
pub(crate) const PIN_SHARE: FILE_SHARE_MODE =
    FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
/// Access of the short-lived handle that flushes a directory: `FlushFileBuffers` needs write
/// access (`FILE_ADD_FILE` is `FILE_WRITE_DATA` on a directory).
const FLUSH_ACCESS: u32 = FILE_ADD_FILE.0 | SYNCHRONIZE.0;
/// How long a replace retries while another program (antivirus, the Explorer preview pane, a
/// user copying the files) holds a recovery file or the directory open.
const BUSY_RETRY_FOR: Duration = Duration::from_secs(2);
/// Pause between two of those attempts.
const BUSY_RETRY_EVERY: Duration = Duration::from_millis(100);
/// Open directories, and a reparse point as itself rather than its target.
pub(crate) const NO_FOLLOW: FILE_FLAGS_AND_ATTRIBUTES =
    FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);

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

impl DataDir {
    /// `(name, SDDL)` of every level from `SHIN DATA CENTER` down to this directory.
    fn levels(self) -> Vec<(&'static str, &'static str)> {
        let mut levels = vec![("SHIN DATA CENTER", BASE_DIR_SDDL), ("MKLM", BASE_DIR_SDDL)];
        match self {
            Self::Base => {}
            Self::Recovery => levels.push(("Recovery", RECOVERY_DIR_SDDL)),
            Self::Logs => levels.push(("logs", PRIVATE_DIR_SDDL)),
            Self::Updates => levels.push(("Updates", PRIVATE_DIR_SDDL)),
        }
        levels
    }
}

/// `%ProgramData%` from the known-folder API.
pub fn program_data_dir() -> Result<PathBuf, Error> {
    // SAFETY: FOLDERID_ProgramData is a static GUID; no token (the current user's view, which is
    // the same for this machine-wide folder). The returned string is freed below.
    let text = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let path = PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: the string was allocated by SHGetKnownFolderPath with CoTaskMemAlloc and is freed
    // once, after the copy above.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(Error::Insecure {
            path: path.display().to_string(),
            reason: "%ProgramData% is not an absolute path".to_string(),
        })
    }
}

/// A validated directory; the handles keep every level from `SHIN DATA CENTER` down pinned.
#[derive(Debug)]
pub struct ProtectedDir {
    path: PathBuf,
    /// `%ProgramData%`, then one per level; the last one is this directory.
    handles: Vec<OwnedHandle>,
    /// Levels that failed validation and were renamed out of the way (see the module docs).
    quarantined: Vec<PathBuf>,
    which: DataDir,
}

impl ProtectedDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Which directory this is (`crate::update_dir` requires `Updates`).
    pub(crate) fn which(&self) -> DataDir {
        self.which
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
    /// `FlushFileBuffers` on a short-lived handle of the directory makes the renames durable.
    /// `name` must be a plain file name.
    ///
    /// The current file is *copied* to `.prev` (through its own durable temporary file), not
    /// moved, so that `name` never goes missing, even for a moment: a crash at any point leaves
    /// `name` with either the old or the new content, and `.prev` complete.
    ///
    /// Users may read the recovery files, so another program can hold one open without
    /// `FILE_SHARE_DELETE`, which makes the replace fail with `ERROR_ACCESS_DENIED` or
    /// `ERROR_SHARING_VIOLATION`. Those are retried for a short while (antivirus software and the
    /// Explorer preview pane let go quickly); after that the error is returned and the files keep
    /// their previous, complete content (residual risk, design I.18).
    pub fn replace_file(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        if !is_plain_file_name(name) {
            return Err(Error::Insecure {
                path: self.path.join(name).display().to_string(),
                reason: "not a plain file name".to_string(),
            });
        }
        let target = self.path.join(name);
        if let Some(previous) = retry_while_busy(|| read_existing(&target))? {
            let prev = self.path.join(format!("{name}.prev"));
            self.write_via_temporary(&format!("{name}.prev"), &prev, &previous)?;
        }
        self.write_via_temporary(name, &target, bytes)?;
        self.flush()
    }

    /// Writes `bytes` to a new temporary file next to `target`, flushes it, and renames it over
    /// `target`. The temporary file is removed again on failure.
    fn write_via_temporary(&self, name: &str, target: &Path, bytes: &[u8]) -> Result<(), Error> {
        let temporary = self.path.join(format!("{name}.tmp-{}", new_uuid()?));
        let result = write_durably(&temporary, bytes)
            .and_then(|()| retry_while_busy(|| move_replacing(&temporary, target)));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    /// `FlushFileBuffers` on a short-lived handle of this directory. The pins ask for no data
    /// access (see [`PIN_ACCESS`]), and flushing needs write access; the pinned levels cannot be
    /// renamed by anyone but SYSTEM and Administrators, so the path still names this directory.
    fn flush(&self) -> Result<(), Error> {
        if self.handles.is_empty() {
            return Err(Error::Insecure {
                path: self.path.display().to_string(),
                reason: "the directory is not open".to_string(),
            });
        }
        let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
        let directory = retry_while_busy(|| {
            open(
                &self.path,
                FLUSH_ACCESS,
                share,
                None,
                OPEN_EXISTING,
                NO_FOLLOW,
            )
            .map_err(|error| win32("CreateFileW", &error))
        })?;
        // SAFETY: `directory` is an open directory handle with FILE_ADD_FILE access.
        unsafe { FlushFileBuffers(raw(&directory)) }
            .map_err(|error| win32("FlushFileBuffers", &error))
    }
}

/// Creates (when missing) and validates `which` and every level above it up to
/// `SHIN DATA CENTER`, quarantining levels that fail validation (module docs).
/// `%ProgramData%` itself is only checked for not being a reparse point.
pub fn ensure_protected_dir(which: DataDir) -> Result<ProtectedDir, Error> {
    let mut path = program_data_dir()?;
    let mut handles = vec![open_program_data(&path)?];
    let mut quarantined = Vec::new();
    for (name, sddl) in which.levels() {
        path.push(name);
        handles.push(ensure_level(&path, sddl, &mut quarantined)?);
    }
    Ok(ProtectedDir {
        path,
        handles,
        quarantined,
        which,
    })
}

/// A protected directory as it stands, checked without creating, moving or writing anything: for
/// the unelevated GUI, which shows the `Recovery` folder to the user (design m3 B.11). Every level
/// from `SHIN DATA CENTER` down must exist and pass the validation of [`ensure_protected_dir`]
/// (not a reparse point, owner Administrators or SYSTEM, protected DACL, no write-type right for
/// anyone else), so that a folder another user created and filled before MKLM's first elevated run
/// (squatting, module docs) is never presented as MKLM's. Users may open the levels with the
/// pinning access (`BASE_DIR_SDDL` and `RECOVERY_DIR_SDDL` grant them `0x1200a9`).
///
/// Returns the path and the handles that pin `%ProgramData%` and every level (no
/// `FILE_SHARE_DELETE`: while they are open, no level can be renamed or replaced); keep them until
/// the path has been used. [`Error::Insecure`] names a level that fails; a missing level fails
/// with the error of `CreateFileW`.
pub fn verify_protected_dir(which: DataDir) -> Result<(PathBuf, Vec<OwnedHandle>), Error> {
    let mut path = program_data_dir()?;
    let mut handles = vec![open_program_data(&path)?];
    for (name, _) in which.levels() {
        path.push(name);
        handles.push(verify_level(&path)?);
    }
    Ok((path, handles))
}

/// [`verify_protected_dir`] as a [`ProtectedDir`], for the update runner (design m5b D.7 step 2):
/// every level must exist and pass the validation; nothing is created, moved or quarantined.
pub fn open_protected_dir(which: DataDir) -> Result<ProtectedDir, Error> {
    let (path, handles) = verify_protected_dir(which)?;
    Ok(ProtectedDir {
        path,
        handles,
        quarantined: Vec::new(),
        which,
    })
}

/// A new directory with [`PRIVATE_DIR_SDDL`] (`CreateDirectoryW`); an existing entry of that name
/// is an error (`ERROR_ALREADY_EXISTS`): the update run folders (design m5b D.5).
pub(crate) fn create_private_directory(path: &Path) -> Result<(), Error> {
    let sd = LocalSd::from_sddl(PRIVATE_DIR_SDDL)?;
    let wide = wide_os(path.as_os_str());
    let attributes = sd.attributes();
    // SAFETY: `wide` is NUL-terminated; `attributes` points at `sd`; both outlive the call.
    unsafe { CreateDirectoryW(PCWSTR(wide.as_ptr()), Some(&attributes)) }
        .map_err(|error| win32("CreateDirectoryW", &error))
}

/// One level of [`verify_protected_dir`]: opened pinned and validated, never created or moved.
/// Also each update run folder and its `tmp` (design m5b D.5).
pub(crate) fn verify_level(path: &Path) -> Result<OwnedHandle, Error> {
    let handle = open(path, PIN_ACCESS, PIN_SHARE, None, OPEN_EXISTING, NO_FOLLOW)
        .map_err(|error| win32("CreateFileW", &error))?;
    validate(&handle, Kind::Directory, DIRECTORY_POLICY).map_err(|reason| Error::Insecure {
        path: path.display().to_string(),
        reason,
    })?;
    Ok(handle)
}

/// Opens `%ProgramData%` (following nothing) and checks that it is a plain directory.
fn open_program_data(path: &Path) -> Result<OwnedHandle, Error> {
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
    let handle = open(
        path,
        FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
        share,
        None,
        OPEN_EXISTING,
        NO_FOLLOW,
    )
    .map_err(|error| win32("CreateFileW", &error))?;
    let attributes = attribute_tag(&handle)?.FileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Err(Error::Insecure {
            path: path.display().to_string(),
            reason: "%ProgramData% is a reparse point or not a directory".to_string(),
        });
    }
    Ok(handle)
}

/// One level: create it if missing, open it pinned, validate it; quarantine and recreate it when
/// it fails.
fn ensure_level(
    path: &Path,
    sddl: &str,
    quarantined: &mut Vec<PathBuf>,
) -> Result<OwnedHandle, Error> {
    let sd = LocalSd::from_sddl(sddl)?;
    for attempt in 0..=MAX_QUARANTINES {
        create_directory(path, &sd)?;
        let reason = match open(path, PIN_ACCESS, PIN_SHARE, None, OPEN_EXISTING, NO_FOLLOW) {
            Ok(handle) => match validate(&handle, Kind::Directory, DIRECTORY_POLICY) {
                Ok(()) => return Ok(handle),
                Err(reason) => reason,
            },
            // A DACL that locks Administrators out is itself a sign of squatting.
            Err(error) if win32_code(&error) == ERROR_ACCESS_DENIED.0 => {
                "Administrators cannot open it".to_string()
            }
            Err(error) => return Err(win32("CreateFileW", &error)),
        };
        if attempt == MAX_QUARANTINES {
            return Err(Error::Insecure {
                path: path.display().to_string(),
                reason: format!(
                    "{reason}, and it was created again each time it was moved aside. Sign out \
                     other users or restart Windows, then try again"
                ),
            });
        }
        quarantined.push(quarantine(path, &reason)?);
    }
    Err(Error::Insecure {
        path: path.display().to_string(),
        reason: "it could not be prepared".to_string(),
    })
}

/// `CreateDirectoryW` with `sd`; an existing entry of that name (directory or not) is fine and
/// is validated by the caller.
fn create_directory(path: &Path, sd: &LocalSd) -> Result<(), Error> {
    let wide = wide_os(path.as_os_str());
    let attributes = sd.attributes();
    // SAFETY: `wide` is NUL-terminated; `attributes` points at `sd`; both outlive the call.
    match unsafe { CreateDirectoryW(PCWSTR(wide.as_ptr()), Some(&attributes)) } {
        Ok(()) => Ok(()),
        Err(error) if win32_code(&error) == ERROR_ALREADY_EXISTS.0 => Ok(()),
        Err(error) => Err(win32("CreateDirectoryW", &error)),
    }
}

/// What a level must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory,
    /// A regular file with a single link.
    File,
}

/// Checks an opened level through its handle; the reason in words on failure.
fn validate(handle: &OwnedHandle, kind: Kind, policy: AclPolicy) -> Result<(), String> {
    let attributes = attribute_tag(handle)
        .map_err(|error| format!("its attributes cannot be read ({error})"))?
        .FileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err("it is a reparse point".to_string());
    }
    let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    match kind {
        Kind::Directory if !is_directory => return Err("it is not a directory".to_string()),
        Kind::File if is_directory => return Err("it is a directory".to_string()),
        Kind::File => {
            let links = standard_info(handle)
                .map_err(|error| format!("its links cannot be counted ({error})"))?
                .NumberOfLinks;
            if links != 1 {
                return Err(format!("it has {links} hard links"));
            }
        }
        Kind::Directory => {}
    }
    let sd = file_security(raw(handle))
        .map_err(|error| format!("its security cannot be read ({error})"))?;
    check_descriptor(sd.as_ptr(), policy)
}

/// Renames `path` (the entry itself, never a link target) to `<name>.untrusted-<uuid>` next to
/// it, without reading it. [`Error::Insecure`] with advice when that is impossible.
fn quarantine(path: &Path, reason: &str) -> Result<PathBuf, Error> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_name = format!("{name}.untrusted-{}", new_uuid()?);
    let fail = |error: Error| Error::Insecure {
        path: path.display().to_string(),
        reason: format!(
            "{reason}, and it could not be moved aside ({error}). Sign out other users or \
             restart Windows, then try again"
        ),
    };
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
    let handle = open(
        path,
        DELETE.0 | SYNCHRONIZE.0,
        share,
        None,
        OPEN_EXISTING,
        NO_FOLLOW,
    )
    .map_err(|error| fail(win32("CreateFileW", &error)))?;
    let target = path.with_file_name(new_name);
    rename_by_handle(&handle, &target).map_err(fail)?;
    Ok(target)
}

/// `SetFileInformationByHandle(FileRenameInfo)` to the full path `target` (no replacing). The
/// handle needs `DELETE`; the caller keeps the target's directory pinned, so the path names the
/// directory it was checked to be.
fn rename_by_handle(handle: &OwnedHandle, target: &Path) -> Result<(), Error> {
    let name: Vec<u16> = target.as_os_str().encode_wide().collect();
    let name_bytes = name.len() * 2;
    let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    // The name, plus a NUL that FileNameLength does not count.
    let total = header + name_bytes + 2;
    // u64 elements align the structure (it holds a HANDLE); zeroed: no replacing, no root.
    let mut buffer = vec![0u64; total.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buffer` is zeroed, 8-byte aligned and has room for the header, the name and a
    // NUL; the name is copied into the trailing `FileName` array, which extends into that room.
    unsafe {
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    // SAFETY: `handle` is open with DELETE; `info` points at `total` initialised bytes.
    unsafe { SetFileInformationByHandle(raw(handle), FileRenameInfo, info.cast(), total as u32) }
        .map_err(|error| win32("SetFileInformationByHandle(FileRenameInfo)", &error))
}

/// `CreateFileW` into an owned handle, keeping the `windows` error for inspection.
pub(crate) fn open(
    path: &Path,
    access: u32,
    share: FILE_SHARE_MODE,
    attributes: Option<&SECURITY_ATTRIBUTES>,
    disposition: FILE_CREATION_DISPOSITION,
    flags: FILE_FLAGS_AND_ATTRIBUTES,
) -> Result<OwnedHandle, windows::core::Error> {
    let wide = wide_os(path.as_os_str());
    // SAFETY: `wide` is NUL-terminated; `attributes`, when given, points at a descriptor the
    // caller keeps alive; both outlive the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            share,
            attributes.map(std::ptr::from_ref),
            disposition,
            flags,
            None,
        )
    }?;
    // SAFETY: CreateFileW succeeded, so `handle` is an open handle this process owns.
    Ok(unsafe { own(handle) })
}

fn attribute_tag(handle: &OwnedHandle) -> Result<FILE_ATTRIBUTE_TAG_INFO, Error> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is a writable FILE_ATTRIBUTE_TAG_INFO of the size passed.
    unsafe {
        GetFileInformationByHandleEx(
            raw(handle),
            FileAttributeTagInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    }
    .map_err(|error| win32("GetFileInformationByHandleEx(FileAttributeTagInfo)", &error))?;
    Ok(info)
}

fn standard_info(handle: &OwnedHandle) -> Result<FILE_STANDARD_INFO, Error> {
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: `info` is a writable FILE_STANDARD_INFO of the size passed.
    unsafe {
        GetFileInformationByHandleEx(
            raw(handle),
            FileStandardInfo,
            (&raw mut info).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }
    .map_err(|error| win32("GetFileInformationByHandleEx(FileStandardInfo)", &error))?;
    Ok(info)
}

/// ASCII letters, digits, `.`, `_` and `-`; not starting or ending with a dot.
pub(crate) fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn io_error(function: &'static str, error: &std::io::Error) -> Error {
    Error::Win32 {
        function,
        code: error.raw_os_error().map_or(0, |code| code as u32),
    }
}

/// The current content of `path`, or `None` when it does not exist. The file must be a regular
/// file; it is opened without following reparse points.
fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let mut file = match OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("CreateFileW", &error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error("GetFileInformationByHandle", &error))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || !metadata.is_file() {
        return Err(Error::Insecure {
            path: path.display().to_string(),
            reason: "it is not a regular file".to_string(),
        });
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error("ReadFile", &error))?;
    Ok(Some(bytes))
}

/// Creates `path` (it must not exist), writes `bytes` and flushes them to disk.
fn write_durably(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|error| io_error("CreateFileW", &error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("WriteFile", &error))?;
    file.sync_all()
        .map_err(|error| io_error("FlushFileBuffers", &error))
}

/// `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`: returns once the rename is on disk.
fn move_replacing(from: &Path, to: &Path) -> Result<(), Error> {
    let from = wide_os(from.as_os_str());
    let to = wide_os(to.as_os_str());
    // SAFETY: both paths are NUL-terminated and outlive the call.
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| win32("MoveFileExW", &error))
}

/// Runs `attempt` again every [`BUSY_RETRY_EVERY`] for up to [`BUSY_RETRY_FOR`] while it fails
/// with `ERROR_SHARING_VIOLATION` or `ERROR_ACCESS_DENIED` (what `MoveFileExW` returns when the
/// file it replaces is open without `FILE_SHARE_DELETE`); the last error otherwise.
fn retry_while_busy<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let deadline = Instant::now() + BUSY_RETRY_FOR;
    loop {
        match attempt() {
            Err(error)
                if error.win32_code().is_some_and(|code| {
                    code == ERROR_SHARING_VIOLATION.0 || code == ERROR_ACCESS_DENIED.0
                }) && Instant::now() < deadline =>
            {
                thread::sleep(BUSY_RETRY_EVERY);
            }
            other => return other,
        }
    }
}

/// The machine-wide write lock (plan 2.2: `LockFileEx`, not a `Global\` mutex).
#[derive(Debug)]
pub struct FileLock {
    file: File,
    /// Where a squatted lock file was moved to, if one was.
    quarantined: Option<PathBuf>,
}

impl FileLock {
    /// Opens or creates [`LOCK_FILE_NAME`] in `base` (the [`DataDir::Base`] directory) with
    /// [`LOCK_FILE_SDDL`], checks that it is a regular file with one link and that DACL (a lock
    /// file that fails is quarantined like a directory level), then
    /// retries `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` on byte 0 every
    /// 100 ms until `timeout` ([`Error::Timeout`]). Dropping the lock (or the process dying)
    /// releases it.
    pub fn acquire(base: &ProtectedDir, timeout: Duration) -> Result<Self, Error> {
        if base.which != DataDir::Base {
            return Err(Error::Insecure {
                path: base.path.display().to_string(),
                reason: "the lock file lives in the MKLM base directory".to_string(),
            });
        }
        let path = base.path.join(LOCK_FILE_NAME);
        let mut quarantined = None;
        let handle = open_lock_file(&path, &mut quarantined)?;
        lock_exclusive(&handle, timeout)?;
        Ok(Self {
            file: File::from(handle),
            quarantined,
        })
    }

    /// Where a squatted lock file was moved to while the lock was taken; the caller reports it as
    /// a warning, like [`ProtectedDir::quarantined`].
    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: the file handle is open and holds the lock on byte 0 taken in `acquire`;
        // closing it right after would release it too, this only makes it immediate.
        let _ = unsafe { UnlockFileEx(raw(&self.file), None, 1, 0, &mut overlapped) };
    }
}

/// Opens (or creates with [`LOCK_FILE_SDDL`]) the lock file for reading and writing and validates
/// it; a lock file that fails is quarantined and created again.
fn open_lock_file(path: &Path, quarantined: &mut Option<PathBuf>) -> Result<OwnedHandle, Error> {
    let sd = LocalSd::from_sddl(LOCK_FILE_SDDL)?;
    let attributes = sd.attributes();
    let access = GENERIC_READ.0 | GENERIC_WRITE.0 | READ_CONTROL.0;
    let flags = FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);
    for attempt in 0..=MAX_QUARANTINES {
        // New: with our descriptor. Existing: as it is, validated below.
        let opened = match open(
            path,
            access,
            PIN_SHARE,
            Some(&attributes),
            CREATE_NEW,
            flags,
        ) {
            Err(error) if win32_code(&error) == ERROR_FILE_EXISTS.0 => {
                open(path, access, PIN_SHARE, None, OPEN_EXISTING, flags)
            }
            other => other,
        };
        let reason = match opened {
            Ok(handle) => match validate(&handle, Kind::File, LOCK_FILE_POLICY) {
                Ok(()) => return Ok(handle),
                Err(reason) => reason,
            },
            Err(error) if win32_code(&error) == ERROR_ACCESS_DENIED.0 => {
                "Administrators cannot open it".to_string()
            }
            Err(error) => return Err(win32("CreateFileW", &error)),
        };
        if attempt == MAX_QUARANTINES {
            break;
        }
        *quarantined = Some(quarantine(path, &reason)?);
    }
    Err(Error::Insecure {
        path: path.display().to_string(),
        reason: "it was created again each time it was moved aside. Sign out other users or \
                 restart Windows, then try again"
            .to_string(),
    })
}

/// `LockFileEx` on byte 0, retried every 100 ms until `timeout`.
fn lock_exclusive(handle: &OwnedHandle, timeout: Duration) -> Result<(), Error> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: `handle` is an open, synchronous file handle with read and write access;
        // LOCKFILE_FAIL_IMMEDIATELY makes the call return at once, so `overlapped` (offset 0)
        // is not used after it.
        let result = unsafe {
            LockFileEx(
                raw(handle),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                None,
                1,
                0,
                &mut overlapped,
            )
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if win32_code(&error) == ERROR_LOCK_VIOLATION.0 => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::Timeout {
                        operation: "LockFileEx",
                    });
                }
                thread::sleep(remaining.min(Duration::from_millis(100)));
            }
            Err(error) => return Err(win32("LockFileEx", &error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Storage::FileSystem::{FILE_LIST_DIRECTORY, FILE_TRAVERSE};

    use super::*;

    /// A scratch directory under the user's temp folder, removed on drop. Only for the parts that
    /// work the same anywhere (renames, durable writes, locks); the protected directories
    /// themselves need elevation and are tested on the machine (design H.2 R7, R15).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("mklm-win-test-{}", new_uuid().expect("uuid")));
            std::fs::create_dir(&path).expect("create scratch directory");
            Self(path)
        }

        /// A `ProtectedDir` over the scratch directory, pinned like a real one.
        fn as_protected(&self) -> ProtectedDir {
            let handle = open(
                &self.0,
                PIN_ACCESS,
                PIN_SHARE,
                None,
                OPEN_EXISTING,
                NO_FOLLOW,
            )
            .expect("pin the scratch directory");
            ProtectedDir {
                path: self.0.clone(),
                handles: vec![handle],
                quarantined: Vec::new(),
                which: DataDir::Recovery,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn program_data_is_an_absolute_directory() {
        let path = program_data_dir().expect("ProgramData");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.is_dir(), "{}", path.display());
        assert!(open_program_data(&path).is_ok());
    }

    /// The `Recovery` folder the helper made on this machine verifies unelevated, read only (the
    /// GUI's "復旧用ファイルのフォルダーを開く"). Needs a machine where MKLM already wrote a change.
    #[test]
    #[ignore = "needs %ProgramData%\\SHIN DATA CENTER\\MKLM\\Recovery made by the helper"]
    fn the_recovery_folder_on_the_machine_verifies_unelevated() {
        let (path, pins) = verify_protected_dir(DataDir::Recovery).expect("verified");
        assert!(
            path.ends_with(r"SHIN DATA CENTER\MKLM\Recovery"),
            "{}",
            path.display()
        );
        assert_eq!(pins.len(), 4, "%ProgramData% and three levels");
    }

    /// A folder any user could have created (owned by the user who runs the test, with the DACL
    /// the temp folder hands down) is refused, and so is a missing one; nothing is created.
    #[test]
    fn a_folder_another_user_could_own_is_not_verified() {
        let scratch = Scratch::new();
        match verify_level(&scratch.0) {
            Err(Error::Insecure { path, .. }) => assert_eq!(path, scratch.0.display().to_string()),
            other => panic!("a user-owned folder was accepted: {other:?}"),
        }
        let missing = scratch.0.join("Recovery");
        assert!(verify_level(&missing).is_err());
        assert!(!missing.exists(), "the check creates nothing");
    }

    #[test]
    fn replace_file_keeps_the_previous_version() {
        let scratch = Scratch::new();
        let dir = scratch.as_protected();
        dir.replace_file("README.txt", b"first")
            .expect("first write");
        assert_eq!(
            std::fs::read(scratch.0.join("README.txt")).ok(),
            Some(b"first".to_vec())
        );
        assert!(!scratch.0.join("README.txt.prev").exists());

        dir.replace_file("README.txt", b"second")
            .expect("second write");
        dir.replace_file("README.txt", b"third")
            .expect("third write");
        assert_eq!(
            std::fs::read(scratch.0.join("README.txt")).ok(),
            Some(b"third".to_vec())
        );
        assert_eq!(
            std::fs::read(scratch.0.join("README.txt.prev")).ok(),
            Some(b"second".to_vec())
        );
        // No temporary files are left behind.
        let names: Vec<String> = std::fs::read_dir(&scratch.0)
            .expect("list")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn replace_file_takes_plain_names_only() {
        let scratch = Scratch::new();
        let dir = scratch.as_protected();
        for name in [
            "",
            "..",
            ".hidden",
            r"..\x",
            "a/b",
            "a:b",
            "name.",
            "日本語.txt",
        ] {
            assert!(
                matches!(dir.replace_file(name, b"x"), Err(Error::Insecure { .. })),
                "{name}"
            );
        }
        assert!(is_plain_file_name("restore-offline.cmd"));
        assert!(is_plain_file_name("mklm-baseline.reg"));
    }

    /// Users may list the protected folders. Whatever share mode a user's handle uses, it must not
    /// make pinning fail, because every write (recovery included) pins the base levels first.
    #[test]
    fn a_folder_held_open_by_a_user_can_still_be_pinned() {
        let scratch = Scratch::new();
        for share in [FILE_SHARE_READ, FILE_SHARE_MODE(0)] {
            let holder = open(
                &scratch.0,
                FILE_LIST_DIRECTORY.0 | FILE_TRAVERSE.0 | SYNCHRONIZE.0,
                share,
                None,
                OPEN_EXISTING,
                NO_FOLLOW,
            )
            .expect("hold");
            let pinned = open(
                &scratch.0,
                PIN_ACCESS,
                PIN_SHARE,
                None,
                OPEN_EXISTING,
                NO_FOLLOW,
            );
            assert!(pinned.is_ok(), "share {share:?}: {pinned:?}");
            drop(holder);
        }
    }

    /// A reader that holds a recovery file open without `FILE_SHARE_DELETE` for a moment (the
    /// preview pane, antivirus) only delays the replace.
    #[test]
    fn a_briefly_held_file_is_replaced_after_a_retry() {
        let scratch = Scratch::new();
        let dir = scratch.as_protected();
        dir.replace_file("README.txt", b"first")
            .expect("first write");
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(scratch.0.join("README.txt"))
            .expect("reader");
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            drop(reader);
        });
        dir.replace_file("README.txt", b"second")
            .expect("replaced after the reader let go");
        releaser.join().expect("releaser");
        assert_eq!(
            std::fs::read(scratch.0.join("README.txt")).ok(),
            Some(b"second".to_vec())
        );
    }

    /// A reader that keeps holding the file makes the replace fail after the retries, and leaves
    /// the previous content complete and no temporary file behind.
    #[test]
    fn a_file_held_for_long_keeps_its_content() {
        let scratch = Scratch::new();
        let dir = scratch.as_protected();
        dir.replace_file("README.txt", b"first")
            .expect("first write");
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(scratch.0.join("README.txt"))
            .expect("reader");
        let started = Instant::now();
        let error = dir
            .replace_file("README.txt", b"second")
            .expect_err("held open");
        assert!(started.elapsed() >= BUSY_RETRY_FOR, "{error:?}");
        assert!(
            matches!(
                error.win32_code(),
                Some(code) if code == ERROR_ACCESS_DENIED.0 || code == ERROR_SHARING_VIOLATION.0
            ),
            "{error:?}"
        );
        drop(reader);
        assert_eq!(
            std::fs::read(scratch.0.join("README.txt")).ok(),
            Some(b"first".to_vec())
        );
        let names: Vec<String> = std::fs::read_dir(&scratch.0)
            .expect("list")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn a_user_owned_directory_fails_validation_and_is_moved_aside() {
        let scratch = Scratch::new();
        let squatted = scratch.0.join("SHIN DATA CENTER");
        std::fs::create_dir(&squatted).expect("create");
        std::fs::write(squatted.join("planted.txt"), b"x").expect("plant");
        let handle = open(
            &squatted,
            PIN_ACCESS,
            PIN_SHARE,
            None,
            OPEN_EXISTING,
            NO_FOLLOW,
        )
        .expect("open");
        // Owned by the test user, DACL inherited from the temp folder.
        assert!(validate(&handle, Kind::Directory, DIRECTORY_POLICY).is_err());
        assert!(validate(&handle, Kind::File, LOCK_FILE_POLICY).is_err());
        // The squatter holds it open without FILE_SHARE_DELETE: it cannot be moved aside while
        // that handle is open (residual risk, design I.18).
        let holder = open(
            &squatted,
            FILE_LIST_DIRECTORY.0 | SYNCHRONIZE.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            NO_FOLLOW,
        )
        .expect("hold");
        assert!(matches!(
            quarantine(&squatted, "test"),
            Err(Error::Insecure { .. })
        ));
        drop(holder);
        drop(handle);

        let moved = quarantine(&squatted, "test").expect("quarantine");
        assert!(!squatted.exists());
        let moved_name = moved
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        assert!(
            moved_name.starts_with("SHIN DATA CENTER.untrusted-"),
            "{moved_name}"
        );
        assert_eq!(moved.parent(), Some(scratch.0.as_path()));
        // Moved as a whole; its content was never needed.
        assert!(moved.join("planted.txt").exists());
    }

    #[test]
    fn plain_files_are_not_directories() {
        let scratch = Scratch::new();
        let file = scratch.0.join("file");
        std::fs::write(&file, b"x").expect("write");
        let handle =
            open(&file, PIN_ACCESS, PIN_SHARE, None, OPEN_EXISTING, NO_FOLLOW).expect("open");
        assert_eq!(
            validate(&handle, Kind::Directory, DIRECTORY_POLICY),
            Err("it is not a directory".to_string())
        );
        std::fs::hard_link(&file, scratch.0.join("link")).expect("hard link");
        assert_eq!(
            validate(&handle, Kind::File, LOCK_FILE_POLICY),
            Err("it has 2 hard links".to_string())
        );
    }

    #[test]
    fn a_held_lock_times_out_and_is_released_on_drop() {
        let scratch = Scratch::new();
        let path = scratch.0.join(LOCK_FILE_NAME);
        let flags = FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0);
        let access = GENERIC_READ.0 | GENERIC_WRITE.0;
        let first = open(&path, access, PIN_SHARE, None, CREATE_NEW, flags).expect("create");
        let second = open(&path, access, PIN_SHARE, None, OPEN_EXISTING, flags).expect("open");
        assert_eq!(lock_exclusive(&first, Duration::from_secs(1)), Ok(()));
        let started = Instant::now();
        assert_eq!(
            lock_exclusive(&second, Duration::from_millis(250)),
            Err(Error::Timeout {
                operation: "LockFileEx"
            })
        );
        assert!(started.elapsed() >= Duration::from_millis(200));
        let held = FileLock {
            file: File::from(first),
            quarantined: None,
        };
        drop(held);
        assert_eq!(lock_exclusive(&second, Duration::from_secs(1)), Ok(()));
    }

    /// Design H.2 R7 and R15: creates and validates `%ProgramData%\SHIN DATA CENTER\MKLM` and takes
    /// the lock. Needs elevation and changes the machine (creates the protected folders), so it
    /// only runs by hand as part of those tests.
    #[test]
    #[ignore = "H.2 R7/R15: needs elevation and creates %ProgramData%\\SHIN DATA CENTER\\MKLM"]
    fn protected_directories_on_the_machine() {
        let base = ensure_protected_dir(DataDir::Base).expect("base directory");
        println!(
            "{} (quarantined: {:?})",
            base.path().display(),
            base.quarantined()
        );
        let lock = FileLock::acquire(&base, Duration::from_secs(10)).expect("lock");
        println!("lock taken (quarantined: {:?})", lock.quarantined());
        let recovery = ensure_protected_dir(DataDir::Recovery).expect("recovery directory");
        println!(
            "{} (quarantined: {:?})",
            recovery.path().display(),
            recovery.quarantined()
        );
    }
}
