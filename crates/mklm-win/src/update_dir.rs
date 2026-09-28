//! `%ProgramData%\SHIN DATA CENTER\MKLM\Updates\<run-id>` (design m5b D.5, D.11): the run folders
//! with an explicit private DACL, validated and pinned, the staged files, the clean-up, the build
//! IDs of the installed executables and the holders of files in use.
//!
//! Every run folder (and its `tmp`) is created with `PRIVATE_DIR_SDDL` (SYSTEM and Administrators
//! only), then opened without following reparse points and validated through that handle (owner,
//! protected DACL, no write-type right for anyone else), and the handle is kept (the pin) while
//! the folder is used, as for the other protected directories (m2 D.9). Users cannot read the
//! folders, so they cannot hold a staged file open to stall the runner or the clean-up.
//!
//! Files are created with `CREATE_NEW` (never replacing anything), written without sharing, and
//! the staged installer is held with `FILE_SHARE_READ` only while the runner verifies and starts
//! it (`RunDir::open_locked`).

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION,
    ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
    FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, OPEN_EXISTING,
    SYNCHRONIZE,
};
use windows::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
    RmStartSession,
};
use windows::core::{PCWSTR, PWSTR};

use crate::elevation::file_build_id;
use crate::error::{Error, win32_code};
use crate::proc_identity::{file_nt_path, processes_with_images};
use crate::protected_dir::{
    DataDir, ProtectedDir, create_private_directory, is_plain_file_name, open, verify_level,
};
use crate::sys::{wide_os, win32};

/// The runner's file name in its folder (`mklm_update::RUNNER_EXE_NAME`; mklm-win cannot depend on
/// mklm-update).
const RUNNER_EXE: &str = "mklm-update-runner.exe";

/// The installed executables, in the order of [`read_build_ids`] (GUI, CLI, helper).
pub const INSTALLED_EXECUTABLES: [&str; 3] = ["mklm.exe", "mklm-cli.exe", "mklm-helper.exe"];

/// Longest file name kept for a holder (design m5b H.1 `FileHolder`).
pub const HOLDER_NAME_MAX: usize = 260;

/// `<major>.<minor>.<patch>-<16 lower-case hex digits>` with 1 to 5 digits per number: the shape of
/// a run ID (`mklm_update::run::RunId` checks the full rule). Nothing else is ever a run folder.
pub fn is_run_dir_name(name: &str) -> bool {
    let Some((version, suffix)) = name.split_once('-') else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
        && suffix.len() == 16
        && suffix
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn insecure(path: &Path, reason: &str) -> Error {
    Error::Insecure {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}

fn io_error(function: &'static str, error: &io::Error) -> Error {
    Error::Win32 {
        function,
        code: error.raw_os_error().map_or(0, |code| code as u32),
    }
}

/// The `Updates` directory, checked to be that one.
fn updates_dir(updates: &ProtectedDir) -> Result<&Path, Error> {
    if updates.which() == DataDir::Updates {
        Ok(updates.path())
    } else {
        Err(insecure(updates.path(), "run folders live in Updates"))
    }
}

/// The path of the run folder `name` in `updates`, the name checked.
fn run_dir_path(updates: &ProtectedDir, name: &str) -> Result<PathBuf, Error> {
    let parent = updates_dir(updates)?;
    if !is_run_dir_name(name) {
        return Err(insecure(&parent.join(name), "not a run folder name"));
    }
    Ok(parent.join(name))
}

/// One `Updates\<run-id>` folder, validated and pinned.
#[derive(Debug)]
pub struct RunDir {
    path: PathBuf,
    /// Keeps the folder from being renamed or replaced while it is used.
    _pin: OwnedHandle,
    /// Pinned subfolders (`tmp`), by name.
    subdirs: Mutex<Vec<(String, OwnedHandle)>>,
}

impl RunDir {
    /// `CreateDirectoryW` with `PRIVATE_DIR_SDDL`; fails if it exists.
    pub fn create(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error> {
        let path = run_dir_path(updates, name)?;
        create_private_directory(&path)?;
        RunDir::pinned(path)
    }

    /// Validates owner, DACL and no reparse point, and pins it.
    pub fn open(updates: &ProtectedDir, name: &str) -> Result<RunDir, Error> {
        RunDir::pinned(run_dir_path(updates, name)?)
    }

    fn pinned(path: PathBuf) -> Result<RunDir, Error> {
        let pin = verify_level(&path)?;
        Ok(RunDir {
            path,
            _pin: pin,
            subdirs: Mutex::new(Vec::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `path\file`, for a plain file name only (no separators, no `..`, no stream names).
    fn file(&self, file: &str) -> Result<PathBuf, Error> {
        if is_plain_file_name(file) {
            Ok(self.path.join(file))
        } else {
            Err(insecure(&self.path.join(file), "not a plain file name"))
        }
    }

    /// A subfolder (`tmp`) with `PRIVATE_DIR_SDDL`, validated and pinned like the run folder.
    pub fn create_private_subdir(&self, name: &str) -> Result<PathBuf, Error> {
        let path = self.file(name)?;
        create_private_directory(&path)?;
        let pin = verify_level(&path)?;
        self.subdirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name.to_string(), pin));
        Ok(path)
    }

    /// Removes a subfolder made by [`create_private_subdir`](Self::create_private_subdir) and
    /// everything in it, never following reparse points (its pin is dropped first). A missing one
    /// is fine.
    pub fn remove_private_subdir(&self, name: &str) -> Result<(), Error> {
        let path = self.file(name)?;
        self.subdirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(pinned, _)| pinned != name);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error("RemoveDirectoryW", &error)),
        }
    }

    /// A new file, opened without sharing and without following a reparse point.
    fn create_new_file(&self, file: &str) -> Result<(File, PathBuf), Error> {
        let path = self.file(file)?;
        let handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)
            .map_err(|error| io_error("CreateFileW", &error))?;
        Ok((handle, path))
    }

    /// `CREATE_NEW`, write, `FlushFileBuffers`.
    pub fn write_new(&self, file: &str, bytes: &[u8]) -> Result<(), Error> {
        let (mut handle, path) = self.create_new_file(file)?;
        let written = handle
            .write_all(bytes)
            .and_then(|()| handle.sync_all())
            .map_err(|error| io_error("WriteFile", &error));
        if written.is_err() {
            drop(handle);
            let _ = std::fs::remove_file(&path);
        }
        written
    }

    /// `CREATE_NEW`, no sharing, for streaming.
    pub fn create_exclusive(&self, file: &str) -> Result<StagedFile, Error> {
        let (handle, path) = self.create_new_file(file)?;
        Ok(StagedFile {
            file: Some(handle),
            path,
        })
    }

    /// A regular file (not a reparse point) of at most `max_len` bytes, opened with
    /// `FILE_SHARE_READ` only.
    pub fn open_locked(&self, file: &str, max_len: u64) -> Result<LockedFile, Error> {
        let path = self.file(file)?;
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)
            .map_err(|error| io_error("CreateFileW", &error))?;
        let metadata = handle
            .metadata()
            .map_err(|error| io_error("GetFileInformationByHandle", &error))?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || !metadata.is_file() {
            return Err(insecure(&path, "not a regular file"));
        }
        let len = metadata.len();
        if len > max_len {
            return Err(insecure(
                &path,
                &format!("{len} bytes, more than {max_len}"),
            ));
        }
        Ok(LockedFile {
            file: handle,
            path,
            len,
        })
    }

    /// Copies `source` in as `file` (`CREATE_NEW`, flushed).
    pub fn copy_in(&self, source: &Path, file: &str) -> Result<(), Error> {
        let mut from = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
            .open(source)
            .map_err(|error| io_error("CreateFileW", &error))?;
        let (mut to, path) = self.create_new_file(file)?;
        let copied = io::copy(&mut from, &mut to)
            .and_then(|_| to.sync_all())
            .map_err(|error| io_error("CopyFile", &error));
        if copied.is_err() {
            drop(to);
            let _ = std::fs::remove_file(&path);
        }
        copied
    }

    /// Deletes a file of the folder (a reparse point is deleted as itself). A missing one is fine.
    pub fn remove_file(&self, file: &str) -> Result<(), Error> {
        let path = self.file(file)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error("DeleteFileW", &error)),
        }
    }
}

/// A staged file being written; deleted on drop unless committed.
#[derive(Debug)]
pub struct StagedFile {
    file: Option<File>,
    path: PathBuf,
}

impl io::Write for StagedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.file.as_mut() {
            Some(file) => file.write(buf),
            None => Err(io::Error::other("the staged file is closed")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

impl StagedFile {
    /// `FlushFileBuffers` and close.
    pub fn commit(mut self) -> Result<(), Error> {
        let file = self.file.take().ok_or(Error::Win32 {
            function: "StagedFile::commit",
            code: 6,
        })?;
        let flushed = file
            .sync_all()
            .map_err(|error| io_error("FlushFileBuffers", &error));
        drop(file);
        if flushed.is_err() {
            let _ = std::fs::remove_file(&self.path);
        }
        // Committed or deleted: nothing is left for `Drop`.
        self.path = PathBuf::new();
        flushed
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A staged file held open with `FILE_SHARE_READ` only.
#[derive(Debug)]
pub struct LockedFile {
    file: File,
    path: PathBuf,
    len: u64,
}

impl io::Read for LockedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
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
        self.file
            .seek(SeekFrom::Start(0))
            .map(|_| ())
            .map_err(|error| io_error("SetFilePointerEx", &error))
    }
}

pub fn list_run_dirs(updates: &ProtectedDir) -> Result<Vec<String>, Error> {
    let parent = updates_dir(updates)?;
    let entries = std::fs::read_dir(parent).map_err(|error| io_error("FindFirstFileW", &error))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error("FindNextFileW", &error))?;
        if let Some(name) = entry.file_name().to_str()
            && is_run_dir_name(name)
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Regular files only, never following reparse points: a link inside the folder (or the folder
/// itself, if it were one) is removed as a link, its target is never touched.
pub fn remove_run_dir(updates: &ProtectedDir, name: &str) -> Result<(), Error> {
    let path = run_dir_path(updates, name)?;
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("RemoveDirectoryW", &error)),
    }
}

/// Every run folder except `keep` whose `mklm-update-runner.exe` is not running; the names
/// removed (design m5b D.11; replaces `remove_at_reboot`, RELIABILITY-9).
///
/// A folder whose runner cannot be checked (the process list cannot be read) is kept. A folder
/// that cannot be removed completely (a file another process holds) is left for the next sweep.
pub fn sweep_stale_run_dirs(
    updates: &ProtectedDir,
    keep: Option<&str>,
) -> Result<Vec<String>, Error> {
    let mut removed = Vec::new();
    for name in list_run_dirs(updates)? {
        if keep == Some(name.as_str()) {
            continue;
        }
        let runner = updates.path().join(&name).join(RUNNER_EXE);
        let running = match file_nt_path(&runner) {
            Ok(nt_path) => match processes_with_images(&[nt_path]) {
                Ok(processes) => !processes.is_empty(),
                Err(_) => true,
            },
            // No runner file (never copied, or already deleted): nothing runs from it.
            Err(_) => false,
        };
        if !running && remove_run_dir(updates, &name).is_ok() {
            removed.push(name);
        }
    }
    Ok(removed)
}

/// Build IDs of `mklm.exe`, `mklm-cli.exe`, `mklm-helper.exe` in `install_dir` (in that order;
/// `None` when missing or unreadable). Callers convert with `InstallState::from_build_ids`.
pub fn read_build_ids(install_dir: &Path) -> [Option<String>; 3] {
    INSTALLED_EXECUTABLES.map(|name| file_build_id(&install_dir.join(name)).ok())
}

/// Indices into `names` of the files in `install_dir` that cannot be opened with `DELETE` and
/// full sharing (another handle lacks FILE_SHARE_DELETE). Missing files are not in use.
///
/// A running image can be opened like that (the loader shares delete), which is why the installer
/// can rename it; only a handle opened without `FILE_SHARE_DELETE` (or without read or write
/// sharing) makes it fail with `ERROR_SHARING_VIOLATION` (design m5b D.8 step 4, SECURITY-10).
pub fn files_in_use(install_dir: &Path, names: &[&str]) -> Result<Vec<usize>, Error> {
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
    let mut in_use = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if !is_plain_file_name(name) {
            return Err(insecure(&install_dir.join(name), "not a plain file name"));
        }
        let path = install_dir.join(name);
        match open(
            &path,
            DELETE.0 | SYNCHRONIZE.0,
            share,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
        ) {
            Ok(handle) => drop(handle),
            Err(error) => match WIN32_ERROR(win32_code(&error)) {
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => {}
                ERROR_SHARING_VIOLATION => in_use.push(index),
                _ => return Err(win32("CreateFileW", &error)),
            },
        }
    }
    Ok(in_use)
}

/// A Restart Manager session, ended on drop.
struct RmSession(u32);

impl Drop for RmSession {
    fn drop(&mut self) {
        // SAFETY: ends the session this value started; nothing uses the handle afterwards.
        let _ = unsafe { RmEndSession(self.0) };
    }
}

fn rm_error(function: &'static str, status: WIN32_ERROR) -> Error {
    Error::Win32 {
        function,
        code: status.0,
    }
}

/// Who holds `paths` open (Restart Manager: `RmStartSession`, `RmRegisterResources`, `RmGetList`,
/// `RmEndSession`): (PID, session ID, application name) each (RED-TEAM-3). Best effort; the
/// callers treat an error as "unknown". Whether `RmGetList` reports plain open handles (not only
/// loaded images) is unverified (design m5b I.16).
pub fn file_holders(paths: &[PathBuf]) -> Result<Vec<(u32, u32, String)>, Error> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut handle = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: `handle` is a valid out pointer and `key` has room for CCH_RM_SESSION_KEY + 1
    // characters, as the API requires.
    let status = unsafe { RmStartSession(&mut handle, None, PWSTR(key.as_mut_ptr())) };
    if status != ERROR_SUCCESS {
        return Err(rm_error("RmStartSession", status));
    }
    let session = RmSession(handle);
    let wide: Vec<Vec<u16>> = paths.iter().map(|path| wide_os(path.as_os_str())).collect();
    let names: Vec<PCWSTR> = wide.iter().map(|path| PCWSTR(path.as_ptr())).collect();
    // SAFETY: every name is NUL-terminated and lives in `wide` until after the call.
    let status = unsafe { RmRegisterResources(session.0, Some(&names), None, None) };
    if status != ERROR_SUCCESS {
        return Err(rm_error("RmRegisterResources", status));
    }
    let mut capacity = 8usize;
    for _ in 0..8 {
        let mut infos = vec![RM_PROCESS_INFO::default(); capacity];
        let mut needed = 0u32;
        let mut count = u32::try_from(capacity).unwrap_or(u32::MAX);
        let mut reasons = 0u32;
        // SAFETY: `infos` has room for `count` entries; the other pointers are valid out
        // pointers.
        let status = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                Some(infos.as_mut_ptr()),
                &mut reasons,
            )
        };
        if status == ERROR_MORE_DATA {
            capacity = (needed as usize).max(capacity * 2);
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(rm_error("RmGetList", status));
        }
        infos.truncate(count as usize);
        return Ok(infos
            .iter()
            .map(|info| {
                (
                    info.Process.dwProcessId,
                    info.TSSessionId,
                    holder_name(&info.strAppName),
                )
            })
            .collect());
    }
    Err(rm_error("RmGetList", ERROR_MORE_DATA))
}

/// A holder's name for `LastResult` (Users can read it): the UTF-16 up to the first NUL, control
/// characters removed, at most [`HOLDER_NAME_MAX`] characters.
pub fn holder_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
        .chars()
        .filter(|c| !c.is_control())
        .take(HOLDER_NAME_MAX)
        .collect()
}

/// `GetDiskFreeSpaceExW` for the caller: free bytes on the volume of `path`.
pub fn free_space(path: &Path) -> Result<u64, Error> {
    let wide = wide_os(path.as_os_str());
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; only the caller's free bytes are
    // requested, through a valid out pointer.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }
        .map_err(|error| win32("GetDiskFreeSpaceExW", &error))?;
    Ok(available)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::session::new_uuid;

    /// A scratch directory under the user's temp folder, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let path =
                std::env::temp_dir().join(format!("mklm-update-dir-{}", new_uuid().expect("uuid")));
            std::fs::create_dir(&path).expect("scratch directory");
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A run folder object over a scratch directory (the real ones need elevation: their DACL
    /// names Administrators as the owner).
    fn scratch_run_dir(scratch: &Scratch) -> RunDir {
        let pin = open(
            &scratch.0,
            crate::protected_dir::PIN_ACCESS,
            crate::protected_dir::PIN_SHARE,
            None,
            OPEN_EXISTING,
            crate::protected_dir::NO_FOLLOW,
        )
        .expect("pin");
        RunDir {
            path: scratch.0.clone(),
            _pin: pin,
            subdirs: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn run_folder_names() {
        for name in [
            "0.2.1-3f9a0c2b7d1e4a65",
            "65535.0.10-ffffffffffffffff",
            "1.0.0-0000000000000000",
        ] {
            assert!(is_run_dir_name(name), "{name}");
        }
        for name in [
            "",
            "0.2.1",
            "0.2-3f9a0c2b7d1e4a65",
            "0.2.1.4-3f9a0c2b7d1e4a65",
            "0.2.1-3F9A0C2B7D1E4A65",
            "0.2.1-3f9a0c2b7d1e4a6",
            "..",
            ".",
            r"..\0.2.1-3f9a0c2b7d1e4a65",
            "0.2.1-3f9a0c2b7d1e4a65\\x",
            "0.2.1-3f9a0c2b7d1e4a65 ",
            "123456.0.0-3f9a0c2b7d1e4a65",
            "0.2.1-3f9a0c2b7d1e4a65:stream",
            "logs",
            "0.2.1-3f9a0c2b7d1e4a65.untrusted-0f8c2d4e",
        ] {
            assert!(!is_run_dir_name(name), "{name:?}");
        }
    }

    #[test]
    fn holder_names_are_cleaned() {
        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().collect() };
        let mut raw = wide("powershell.exe");
        raw.extend([0, u16::from(b'x')]);
        assert_eq!(holder_name(&raw), "powershell.exe");
        assert_eq!(holder_name(&wide("evil\r\nname\u{7}.exe")), "evilname.exe");
        assert_eq!(holder_name(&wide(&"a".repeat(400))).len(), HOLDER_NAME_MAX);
        assert_eq!(holder_name(&[]), "");
        assert_eq!(holder_name(&[0xd800, u16::from(b'a')]), "\u{fffd}a");
        assert_eq!(holder_name(&wide("日本語.exe")), "日本語.exe");
    }

    #[test]
    fn staged_files() {
        let scratch = Scratch::new();
        let run = scratch_run_dir(&scratch);
        run.write_new("latest.json", b"{}").expect("write");
        assert_eq!(
            std::fs::read(scratch.0.join("latest.json")).ok(),
            Some(b"{}".to_vec())
        );
        // Never replaces anything.
        assert!(run.write_new("latest.json", b"x").is_err());
        assert!(run.create_exclusive("latest.json").is_err());
        // Plain names only.
        for name in ["", "..", r"..\x", "a/b", "a:b", "x.", ".x"] {
            assert!(run.write_new(name, b"x").is_err(), "{name:?}");
        }
        // A committed file stays; an uncommitted one is deleted.
        let mut staged = run
            .create_exclusive("MKLM-Setup-0.2.1-x64.exe")
            .expect("create");
        staged.write_all(&[1, 2, 3]).expect("write");
        // Not shared while written.
        assert!(std::fs::read(scratch.0.join("MKLM-Setup-0.2.1-x64.exe")).is_err());
        staged.commit().expect("commit");
        assert_eq!(
            std::fs::read(scratch.0.join("MKLM-Setup-0.2.1-x64.exe")).ok(),
            Some(vec![1, 2, 3])
        );
        let mut dropped = run.create_exclusive("partial.exe").expect("create");
        dropped.write_all(&[9; 10]).expect("write");
        drop(dropped);
        assert!(!scratch.0.join("partial.exe").exists());
    }

    #[test]
    fn locked_files_allow_reading_only() {
        let scratch = Scratch::new();
        let run = scratch_run_dir(&scratch);
        run.write_new("installer.exe", &[5u8; 1000]).expect("write");
        assert!(run.open_locked("installer.exe", 999).is_err(), "too large");
        let mut locked = run.open_locked("installer.exe", 1000).expect("open");
        assert_eq!(locked.len(), 1000);
        assert!(!locked.is_empty());
        let mut first = Vec::new();
        locked.read_to_end(&mut first).expect("read");
        locked.rewind().expect("rewind");
        let mut second = Vec::new();
        locked.read_to_end(&mut second).expect("read again");
        assert_eq!(first, second);
        // While it is held: others may read, but not write, rename or delete it.
        let path = scratch.0.join("installer.exe");
        assert!(std::fs::read(&path).is_ok());
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        assert!(std::fs::rename(&path, scratch.0.join("moved.exe")).is_err());
        assert!(run.remove_file("installer.exe").is_err());
        // It is in use in the sense of design m5b D.8 step 4.
        assert_eq!(
            files_in_use(&scratch.0, &["installer.exe", "missing.exe"]),
            Ok(vec![0])
        );
        drop(locked);
        assert_eq!(files_in_use(&scratch.0, &["installer.exe"]), Ok(vec![]));
        run.remove_file("installer.exe").expect("remove");
        run.remove_file("installer.exe").expect("missing is fine");
        // A directory is not a file to lock.
        std::fs::create_dir(scratch.0.join("dir")).expect("dir");
        assert!(run.open_locked("dir", 10).is_err());
    }

    #[test]
    fn copies_in_and_private_subfolders() {
        let scratch = Scratch::new();
        let run = scratch_run_dir(&scratch);
        let source = scratch.0.join("source.bin");
        std::fs::write(&source, vec![42u8; 70_000]).expect("source");
        run.copy_in(&source, "mklm-update-runner.exe")
            .expect("copy");
        assert_eq!(
            std::fs::read(scratch.0.join("mklm-update-runner.exe")).ok(),
            Some(vec![42u8; 70_000])
        );
        assert!(run.copy_in(&source, "mklm-update-runner.exe").is_err());
        // The subfolder is made with the private DACL; a scratch folder owned by the test user
        // cannot hold one that validates (owner), so only the refusal paths run here.
        assert!(run.create_private_subdir("..").is_err());
        run.remove_private_subdir("tmp").expect("missing is fine");
        std::fs::create_dir(scratch.0.join("tmp")).expect("tmp");
        std::fs::write(scratch.0.join("tmp").join("nsA1B2.tmp"), b"x").expect("file");
        run.remove_private_subdir("tmp").expect("remove");
        assert!(!scratch.0.join("tmp").exists());
    }

    #[test]
    fn build_ids_of_missing_files_are_none() {
        let scratch = Scratch::new();
        assert_eq!(read_build_ids(&scratch.0), [None, None, None]);
        // A file without VERSIONINFO has no build ID either.
        std::fs::write(scratch.0.join("mklm.exe"), b"MZ").expect("file");
        assert_eq!(read_build_ids(&scratch.0), [None, None, None]);
    }

    #[test]
    fn free_space_of_the_temp_volume() {
        let free = free_space(&std::env::temp_dir()).expect("free space");
        assert!(free > 0);
        assert!(free_space(Path::new(r"Q:\no\such\volume")).is_err() || cfg!(miri));
    }

    /// Restart Manager on a file this test holds open (design m5b I.16: whether it reports plain
    /// handles is recorded, not required).
    #[test]
    fn holders_of_a_file() {
        let scratch = Scratch::new();
        let path = scratch.0.join("held.bin");
        std::fs::write(&path, b"x").expect("file");
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&path)
            .expect("hold");
        let holders = file_holders(std::slice::from_ref(&path)).expect("Restart Manager");
        for (pid, _, name) in &holders {
            assert!(*pid != 0 && name.len() <= HOLDER_NAME_MAX);
        }
        eprintln!("holders of an open handle: {holders:?}");
        drop(held);
        assert_eq!(file_holders(&[]), Ok(Vec::new()));
    }
}
