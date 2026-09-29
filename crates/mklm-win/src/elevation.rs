//! Elevation (M2): the token check and launching the helper, through UAC when the caller is not
//! elevated and directly when it is.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_INVALID_HANDLE, ERROR_RESOURCE_TYPE_NOT_FOUND, FILETIME, HANDLE, HWND,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
    TokenElevation, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_VER_GET_NEUTRAL, GetFileVersionInfoExW,
    GetFileVersionInfoSizeExW, VerQueryValueW,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Console::GetConsoleWindow;
use windows::Win32::System::Environment::{GetCommandLineW, SetEnvironmentVariableW};
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetCurrentProcess, GetExitCodeProcess, GetProcessId, GetProcessTimes, OpenProcessToken,
    PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX64, KF_FLAG_DEFAULT,
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHGetKnownFolderPath, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, PWSTR};

use crate::error::{Error, win32_code};
use crate::props::to_wide;
use crate::security::sid_to_string;
use crate::sys::{last_error, own, raw, wait_millis, wide_os, win32};

/// File name of the helper.
pub const HELPER_EXE: &str = "mklm-helper.exe";

/// Name of the VERSIONINFO string that carries the build ID both executables embed
/// (design review S11).
pub const BUILD_ID_VERSION_KEY: &str = "MKLMBuildId";

/// True when this process's token is elevated (`GetTokenInformation(TokenElevation)`).
pub fn is_elevated() -> Result<bool, Error> {
    let token = process_token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: `token` is open with TOKEN_QUERY; `elevation` is a writable TOKEN_ELEVATION of the
    // size passed; `returned` is a valid out pointer.
    unsafe {
        GetTokenInformation(
            raw(&token),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    }
    .map_err(|error| win32("GetTokenInformation(TokenElevation)", &error))?;
    Ok(elevation.TokenIsElevated != 0)
}

/// The SID of the user this process runs as, in `S-1-5-21-…` form (`TokenUser`). For DACLs that
/// name the interactive user, such as the M3 single-instance pipe (design E.2).
pub fn current_user_sid() -> Result<String, Error> {
    let token = process_token()?;
    let buffer = token_information(&token, TokenUser, "GetTokenInformation(TokenUser)")?;
    // SAFETY: GetTokenInformation(TokenUser) filled the 8-byte aligned buffer with a TOKEN_USER
    // whose SID points into the same buffer, which lives until the end of this function.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_to_string(sid)
}

/// This process's token, opened with `TOKEN_QUERY`.
fn process_token() -> Result<OwnedHandle, Error> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle; `token` is a valid out pointer.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|error| win32("OpenProcessToken", &error))?;
    // SAFETY: OpenProcessToken succeeded, so `token` is an open handle this process owns.
    Ok(unsafe { own(token) })
}

/// A variable-size token information class, in an 8-byte aligned buffer.
pub(crate) fn token_information(
    token: &OwnedHandle,
    class: TOKEN_INFORMATION_CLASS,
    function: &'static str,
) -> Result<Vec<u64>, Error> {
    let mut size = 0u32;
    // SAFETY: a size query without a buffer; `size` is a valid out pointer. It fails with
    // ERROR_INSUFFICIENT_BUFFER and sets `size`.
    let query = unsafe { GetTokenInformation(raw(token), class, None, 0, &mut size) };
    if size == 0 {
        return Err(query
            .err()
            .map_or_else(|| last_error(function), |error| win32(function, &error)));
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `buffer` has at least `size` writable bytes; `size` is a valid in/out pointer.
    unsafe {
        GetTokenInformation(
            raw(token),
            class,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )
    }
    .map_err(|error| win32(function, &error))?;
    Ok(buffer)
}

/// A helper process started by [`launch_elevated`] or [`spawn_from_elevated`].
#[derive(Debug)]
pub struct ElevatedProcess {
    handle: OwnedHandle,
    pid: u32,
}

impl ElevatedProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The process handle, for waits inside this crate (`pipe::PipeServer::accept`).
    pub(crate) fn raw_handle(&self) -> HANDLE {
        raw(&self.handle)
    }

    /// Exit code once it has exited; `None` while it runs.
    pub fn exit_code(&self) -> Result<Option<u32>, Error> {
        self.wait(Duration::ZERO)
    }

    /// Waits up to `timeout` for it to exit.
    pub fn wait(&self, timeout: Duration) -> Result<Option<u32>, Error> {
        // SAFETY: the handle is an open process handle owned by `self`.
        let result = unsafe { WaitForSingleObject(raw(&self.handle), wait_millis(timeout)) };
        if result == WAIT_TIMEOUT {
            return Ok(None);
        }
        if result != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject"));
        }
        let mut code = 0u32;
        // SAFETY: the handle is an open process handle; `code` is a valid out pointer.
        unsafe { GetExitCodeProcess(raw(&self.handle), &mut code) }
            .map_err(|error| win32("GetExitCodeProcess", &error))?;
        Ok(Some(code))
    }
}

/// Unelevated caller: `ShellExecuteExW` with verb `runas`, the absolute `exe`, `parameters` (the
/// fixed helper arguments), `lpDirectory` = System32, `SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC |
/// SEE_MASK_FLAG_NO_UI` and `SW_HIDE`. `owner_window` parents the UAC prompt (the GUI window, or
/// the console window for the CLI). Fails with [`Error::Cancelled`] when the user declines.
///
/// `exe` must be an absolute path to a regular file (not a reparse point), else
/// [`Error::Insecure`].
pub fn launch_elevated(
    exe: &Path,
    parameters: &str,
    owner_window: Option<isize>,
) -> Result<ElevatedProcess, Error> {
    check_executable(exe)?;
    let file = wide_os(exe.as_os_str());
    let parameters = to_wide(parameters);
    let directory = system_directory_wide()?;
    let verb = to_wide("runas");
    let _com = ComApartment::enter();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: HWND(std::ptr::without_provenance_mut(
            owner_window.unwrap_or(0) as usize
        )),
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised with its size; every string it points at is
    // NUL-terminated and outlives the call.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => {}
        Err(error) if win32_code(&error) == ERROR_CANCELLED.0 => return Err(Error::Cancelled),
        Err(error) => return Err(win32("ShellExecuteExW", &error)),
    }
    if info.hProcess.is_invalid() {
        return Err(Error::Win32 {
            function: "ShellExecuteExW",
            code: ERROR_INVALID_HANDLE.0,
        });
    }
    // SAFETY: with SEE_MASK_NOCLOSEPROCESS the caller owns the returned process handle.
    let handle = unsafe { own(info.hProcess) };
    process_from_handle(handle)
}

/// Elevated caller (an administrator console, Safe Mode): `CreateProcessW` of the absolute `exe`
/// with `parameters`, current directory System32, `CREATE_NO_WINDOW`; no UAC prompt and no
/// dependency on the Appinfo service. The helper still runs as a separate process, so that closing
/// the console or pressing Ctrl+C disconnects the pipe and a running countdown reverts at once
/// (design review C8).
///
/// `exe` must be an absolute path to a regular file (not a reparse point), else
/// [`Error::Insecure`]. No handles are inherited.
pub fn spawn_from_elevated(exe: &Path, parameters: &str) -> Result<ElevatedProcess, Error> {
    check_executable(exe)?;
    let application = wide_os(exe.as_os_str());
    let mut command_line = command_line(exe.as_os_str(), parameters);
    let directory = system_directory_wide()?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: the application name and directory are NUL-terminated; `command_line` is a
    // writable NUL-terminated buffer, as CreateProcessW requires; `startup` has its size set and
    // `info` is a valid out pointer. Nothing is inherited.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            PCWSTR(directory.as_ptr()),
            &startup,
            &mut info,
        )
    }
    .map_err(|error| win32("CreateProcessW", &error))?;
    // SAFETY: on success both handles are open and owned by the caller.
    let (process, thread) = unsafe { (own(info.hProcess), own(info.hThread)) };
    drop(thread);
    Ok(ElevatedProcess {
        handle: process,
        pid: info.dwProcessId,
    })
}

/// `"<exe>" <parameters>` as a NUL-terminated, writable UTF-16 buffer.
fn command_line(exe: &OsStr, parameters: &str) -> Vec<u16> {
    let mut line: Vec<u16> = Vec::new();
    line.push(u16::from(b'"'));
    line.extend(exe.encode_wide());
    line.push(u16::from(b'"'));
    if !parameters.is_empty() {
        line.push(u16::from(b' '));
        line.extend(parameters.encode_utf16());
    }
    line.push(0);
    line
}

fn process_from_handle(handle: OwnedHandle) -> Result<ElevatedProcess, Error> {
    // SAFETY: `handle` is an open process handle.
    let pid = unsafe { GetProcessId(raw(&handle)) };
    if pid == 0 {
        return Err(last_error("GetProcessId"));
    }
    Ok(ElevatedProcess { handle, pid })
}

/// `exe` must be absolute, exist as a regular file, and not be a reparse point.
fn check_executable(exe: &Path) -> Result<(), Error> {
    let insecure = |reason: &str| Error::Insecure {
        path: exe.display().to_string(),
        reason: reason.to_string(),
    };
    if !exe.is_absolute() {
        return Err(insecure("not an absolute path"));
    }
    let metadata = std::fs::symlink_metadata(exe).map_err(|error| Error::Win32 {
        function: "GetFileAttributesExW",
        code: error.raw_os_error().map_or(0, |code| code as u32),
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(insecure("is a reparse point"));
    }
    if !metadata.is_file() {
        return Err(insecure("is not a regular file"));
    }
    Ok(())
}

// ---- M5b: the update runner's processes (design m5b D.4 step 18, D.7 step 16, D.9.4;
// SECURITY-6; WP-H) ----

/// `ERROR_INVALID_PARAMETER`.
const INVALID_PARAMETER: u32 = 87;

/// Name/value pairs of an explicit environment block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanEnvironment {
    pub vars: Vec<(String, String)>,
}

/// The variables of [`runner_environment`], and nothing else.
pub const RUNNER_ENVIRONMENT_NAMES: [&str; 10] = [
    "ComSpec",
    "PATH",
    "ProgramData",
    "ProgramFiles",
    "ProgramW6432",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "windir",
];

/// Sets `SystemRoot` and `windir` to the Windows folder (`GetSystemWindowsDirectoryW`) and
/// `SystemDrive` to its drive (`X:`) in this process's own environment (design m5b D.9.4). The
/// elevated entry points call it first thing in `main`: the helper in every mode, and the CLI
/// (its `--in-process` fallback runs the engine elevated).
///
/// `SHGetKnownFolderPath(FOLDERID_ProgramData)` expands the `%SystemDrive%` of
/// `HKLM\…\ProfileList\ProgramData` from the calling process's environment, and a UAC-elevated
/// process's environment carries the values the unelevated user sets in `HKCU\Environment`: with
/// a forged `SystemDrive`, the helper would put its lock file, logs and update folders under a
/// folder of the user's choosing. The shell caches a known folder's path after the first lookup in
/// a process, so this must run before any known-folder call (a later pin changes nothing).
/// Registry values of type `REG_EXPAND_SZ` that name `%SystemRoot%` (COM servers, for one) are
/// expanded with the same environment.
pub fn pin_system_environment() -> Result<(), Error> {
    let (windows, drive) = system_roots()?;
    for (name, value) in [
        ("SystemDrive", drive.as_str()),
        ("SystemRoot", windows.as_str()),
        ("windir", windows.as_str()),
    ] {
        let name = to_wide(name);
        let value = to_wide(value);
        // SAFETY: both strings are NUL-terminated and outlive the call. The call changes this
        // process's environment block only, under the process's own lock (so, as
        // `std::env::set_var` on Windows, it is safe with other threads).
        unsafe { SetEnvironmentVariableW(PCWSTR(name.as_ptr()), PCWSTR(value.as_ptr())) }
            .map_err(|error| win32("SetEnvironmentVariableW", &error))?;
    }
    Ok(())
}

/// A path as UTF-8 text; [`Error::UnexpectedData`] when it is not valid Unicode.
fn path_text(path: &Path) -> Result<String, Error> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::UnexpectedData {
            path: path.display().to_string(),
        })
}

/// The Windows folder (`GetSystemWindowsDirectoryW`) and its drive, `X:`.
fn system_roots() -> Result<(String, String), Error> {
    let windows = path_text(&windows_directory()?)?;
    let drive = windows
        .get(..2)
        .filter(|drive| drive.as_bytes()[0].is_ascii_alphabetic() && drive.ends_with(':'))
        .ok_or_else(|| Error::UnexpectedData {
            path: windows.clone(),
        })?
        .to_string();
    Ok((windows, drive))
}

/// SystemRoot, windir, SystemDrive, ComSpec, PATH (System32; Windows; System32\Wbem),
/// ProgramData, ProgramFiles, ProgramW6432 from the system, and TEMP = TMP = `temp_dir`
/// (design m5b D.9.4). Nothing from this process's environment.
///
/// The Windows folder comes from `GetSystemWindowsDirectoryW`, System32 from
/// `GetSystemDirectoryW`, the others from `SHGetKnownFolderPath` (whose `ProgramData` depends on
/// this process's `SystemDrive`: the helper has pinned it, [`pin_system_environment`]);
/// `temp_dir` must be absolute.
pub fn runner_environment(temp_dir: &Path) -> Result<CleanEnvironment, Error> {
    if !temp_dir.is_absolute() {
        return Err(Error::Insecure {
            path: temp_dir.display().to_string(),
            reason: "TEMP must be an absolute path".to_string(),
        });
    }
    let (windows, drive) = system_roots()?;
    let system32 = path_text(&system_directory()?)?;
    let temp = path_text(temp_dir)?;
    let vars = vec![
        ("ComSpec".to_string(), format!(r"{system32}\cmd.exe")),
        (
            "PATH".to_string(),
            format!(r"{system32};{windows};{system32}\Wbem"),
        ),
        (
            "ProgramData".to_string(),
            path_text(&known_folder(&FOLDERID_ProgramData)?)?,
        ),
        (
            "ProgramFiles".to_string(),
            path_text(&known_folder(&FOLDERID_ProgramFiles)?)?,
        ),
        (
            "ProgramW6432".to_string(),
            path_text(&known_folder(&FOLDERID_ProgramFilesX64)?)?,
        ),
        ("SystemDrive".to_string(), drive),
        ("SystemRoot".to_string(), windows.clone()),
        ("TEMP".to_string(), temp.clone()),
        ("TMP".to_string(), temp),
        ("windir".to_string(), windows),
    ];
    Ok(CleanEnvironment { vars })
}

/// The Windows folder (`GetSystemWindowsDirectoryW`: the shared one, also on a terminal server).
fn windows_directory() -> Result<PathBuf, Error> {
    let mut buffer = vec![0u16; 260];
    for _ in 0..4 {
        // SAFETY: the slice tells the API how much it may write.
        let len = unsafe { GetSystemWindowsDirectoryW(Some(&mut buffer)) } as usize;
        if len == 0 {
            return Err(last_error("GetSystemWindowsDirectoryW"));
        }
        if len < buffer.len() {
            buffer.truncate(len);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer = vec![0u16; len + 1];
    }
    Err(last_error("GetSystemWindowsDirectoryW"))
}

/// A known folder of the machine (`SHGetKnownFolderPath`, no token), absolute.
fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, Error> {
    // SAFETY: `id` is a static known-folder GUID; no token. The returned string is freed below.
    let text = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
        .map_err(|error| win32("SHGetKnownFolderPath", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let path = PathBuf::from(OsString::from_wide(unsafe { text.as_wide() }));
    // SAFETY: allocated by SHGetKnownFolderPath with CoTaskMemAlloc; freed once, after the copy.
    unsafe { CoTaskMemFree(Some(text.0.cast_const().cast())) };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(Error::Insecure {
            path: path.display().to_string(),
            reason: "a known folder is not an absolute path".to_string(),
        })
    }
}

/// `CREATE_UNICODE_ENVIRONMENT` block: sorted case-insensitively by name, `name=value\0` each,
/// then `\0`. Names containing `=` or NUL, and values containing NUL, are refused. Pure.
///
/// Also refused: an empty name, and two names that differ only in case (one variable to
/// Windows). The order is Windows': names compared upper-cased, as UTF-16 units. An empty list is
/// the empty block `\0\0`.
pub fn environment_block(vars: &[(String, String)]) -> Result<Vec<u16>, Error> {
    let refused = || Error::Win32 {
        function: "environment_block",
        code: INVALID_PARAMETER,
    };
    let mut entries: Vec<(Vec<u16>, &str, &str)> = Vec::with_capacity(vars.len());
    for (name, value) in vars {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(refused());
        }
        entries.push((name.to_uppercase().encode_utf16().collect(), name, value));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(refused());
    }
    let mut block = Vec::new();
    for (_, name, value) in entries {
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// A process started by `spawn_clean`.
#[derive(Debug)]
pub struct SpawnedProcess {
    pub process: ElevatedProcess,
    /// The primary thread, kept only when started suspended.
    pub thread: Option<OwnedHandle>,
}

impl SpawnedProcess {
    /// PID and creation time (`GetProcessTimes` on the handle this value holds, so the PID
    /// cannot have been reused).
    pub fn identity(&self) -> Result<mklm_core::ProcessIdentity, Error> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the process handle is open (owned by `self.process`); out pointers are valid.
        unsafe {
            GetProcessTimes(
                self.process.raw_handle(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
        .map_err(|error| win32("GetProcessTimes", &error))?;
        Ok(mklm_core::ProcessIdentity {
            pid: self.process.pid(),
            creation_time: (u64::from(creation.dwHighDateTime) << 32)
                | u64::from(creation.dwLowDateTime),
        })
    }

    /// `ResumeThread` of the primary thread of a process started suspended; the thread handle is
    /// closed afterwards. A process that was not suspended has nothing to resume
    /// (`ERROR_INVALID_HANDLE`).
    pub fn resume(&mut self) -> Result<(), Error> {
        let thread = self.thread.take().ok_or(Error::Win32 {
            function: "ResumeThread",
            code: ERROR_INVALID_HANDLE.0,
        })?;
        // SAFETY: `thread` is the open primary-thread handle CreateProcessW returned.
        let previous = unsafe { ResumeThread(raw(&thread)) };
        if previous == u32::MAX {
            let error = last_error("ResumeThread");
            self.thread = Some(thread);
            return Err(error);
        }
        Ok(())
    }

    /// `TerminateProcess` with `exit_code`.
    pub fn terminate(&self, exit_code: u32) -> Result<(), Error> {
        // SAFETY: the process handle is open and has PROCESS_TERMINATE (CreateProcessW returns a
        // handle with all access).
        unsafe { TerminateProcess(self.process.raw_handle(), exit_code) }
            .map_err(|error| win32("TerminateProcess", &error))
    }
}

/// `CreateProcessW` of the absolute `exe` (regular file, not a reparse point) with `parameters`,
/// `env` as the whole environment, current directory System32, `CREATE_NO_WINDOW`, optionally
/// `CREATE_SUSPENDED`; nothing inherited. No UAC (the caller is elevated).
///
/// `CREATE_UNICODE_ENVIRONMENT` with the block of [`environment_block`]: the child sees `env` and
/// nothing of this process's environment (design m5b D.9.4, SECURITY-6).
pub fn spawn_clean(
    exe: &Path,
    parameters: &str,
    env: &CleanEnvironment,
    suspended: bool,
) -> Result<SpawnedProcess, Error> {
    check_executable(exe)?;
    let block = environment_block(&env.vars)?;
    let application = wide_os(exe.as_os_str());
    let mut command_line = command_line(exe.as_os_str(), parameters);
    let directory = system_directory_wide()?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
    if suspended {
        flags |= CREATE_SUSPENDED;
    }
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: the application name and directory are NUL-terminated; `command_line` is a
    // writable NUL-terminated buffer, as CreateProcessW requires; `block` is a complete UTF-16
    // environment block (double NUL) that outlives the call; `startup` has its size set and
    // `info` is a valid out pointer. Nothing is inherited.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            Some(block.as_ptr().cast()),
            PCWSTR(directory.as_ptr()),
            &startup,
            &mut info,
        )
    }
    .map_err(|error| win32("CreateProcessW", &error))?;
    // SAFETY: on success both handles are open and owned by the caller.
    let (process, thread) = unsafe { (own(info.hProcess), own(info.hThread)) };
    Ok(SpawnedProcess {
        process: ElevatedProcess {
            handle: process,
            pid: info.dwProcessId,
        },
        thread: suspended.then_some(thread),
    })
}

/// System32 (`GetSystemDirectoryW`): the launch directory of the helper (design A.6, E.1), and
/// where the layout DLLs must be (plan 1.5).
pub fn system_directory() -> Result<PathBuf, Error> {
    let mut buffer = vec![0u16; 260];
    for _ in 0..4 {
        // SAFETY: the slice tells the API how much it may write.
        let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if len == 0 {
            return Err(last_error("GetSystemDirectoryW"));
        }
        if len < buffer.len() {
            buffer.truncate(len);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        // Too small: `len` is the size needed, including the NUL.
        buffer = vec![0u16; len + 1];
    }
    Err(last_error("GetSystemDirectoryW"))
}

/// System32 as a NUL-terminated UTF-16 string.
fn system_directory_wide() -> Result<Vec<u16>, Error> {
    Ok(wide_os(system_directory()?.as_os_str()))
}

/// Plan 1.5 / design review S8: true when `file_name` (e.g. `kbd106.dll`) exists in System32 as a
/// regular file (not a directory, not a reparse point). `file_name` must be a plain file name;
/// anything with a path separator, a drive or `..` is refused with [`Error::Insecure`].
pub fn system32_file_exists(file_name: &str) -> Result<bool, Error> {
    let plain = !file_name.is_empty()
        && file_name != "."
        && file_name != ".."
        && !file_name.contains(['\\', '/', ':', '\0']);
    if !plain {
        return Err(Error::Insecure {
            path: file_name.to_string(),
            reason: "not a plain file name".to_string(),
        });
    }
    let path = system_directory()?.join(file_name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            Ok(metadata.is_file()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::Win32 {
            function: "GetFileAttributesExW",
            code: error.raw_os_error().map_or(0, |code| code as u32),
        }),
    }
}

/// This process's command line exactly as Windows passed it (`GetCommandLineW`), for the helper's
/// strict check of its fixed arguments (design E.4: no CRT argv splitting). Unpaired surrogates
/// become U+FFFD, which that check then refuses.
pub fn process_command_line() -> String {
    // SAFETY: GetCommandLineW has no preconditions and returns a NUL-terminated string that
    // lives as long as the process.
    let line = unsafe { GetCommandLineW() };
    // SAFETY: `line` is a valid NUL-terminated string (see above).
    String::from_utf16_lossy(unsafe { line.as_wide() })
}

/// The console window of this process (`GetConsoleWindow`), to parent the UAC prompt the CLI
/// raises ([`launch_elevated`]'s `owner_window`); `None` without a console.
pub fn console_window() -> Option<isize> {
    // SAFETY: GetConsoleWindow has no preconditions.
    let window = unsafe { GetConsoleWindow() };
    (!window.is_invalid()).then_some(window.0 as isize)
}

/// COM for `ShellExecuteExW`, which may delegate to shell extensions. Leaves an apartment the
/// thread already had alone.
pub(crate) struct ComApartment {
    entered: bool,
}

impl ComApartment {
    pub(crate) fn enter() -> Self {
        // SAFETY: no reserved pointer; the flags are constants. Balanced by `Drop` when it
        // succeeded (S_OK or S_FALSE).
        let result =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Self {
            entered: result.is_ok(),
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.entered {
            // SAFETY: balances the successful CoInitializeEx of `enter` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// M2 helper discovery: `mklm-helper.exe` in the directory of the running executable, as an
/// absolute path, checked to be a regular file (not a reparse point). M5 replaces this with the
/// install location recorded in HKLM.
pub fn helper_path() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe().map_err(|error| Error::Win32 {
        function: "GetModuleFileNameW",
        code: error.raw_os_error().map_or(0, |code| code as u32),
    })?;
    let directory = exe.parent().ok_or_else(|| Error::Insecure {
        path: exe.display().to_string(),
        reason: "has no parent directory".to_string(),
    })?;
    let helper = directory.join(HELPER_EXE);
    check_executable(&helper)?;
    Ok(helper)
}

/// The build ID in `exe`'s VERSIONINFO ([`BUILD_ID_VERSION_KEY`], `GetFileVersionInfoW` +
/// `VerQueryValueW`). The caller compares it with its own before launching the helper, so that a
/// stale helper fails before the UAC prompt (design review S11).
///
/// Uses `FILE_VER_GET_NEUTRAL` and looks for the string in every translation the file lists, then
/// in `040904B0`. A file without it fails with `ERROR_RESOURCE_TYPE_NOT_FOUND`. Note that the
/// version APIs take the *strings* from a matching `<language>\<file>.mui` when one exists (only
/// `VS_FIXEDFILEINFO` comes from the file itself), so this is a pre-UAC staleness check, not a
/// guarantee: the authoritative check is the build ID compiled into the helper, compared in the
/// handshake (design E.3). Whoever can place such a `.mui` next to the helper can replace the
/// helper itself.
pub fn file_build_id(exe: &Path) -> Result<String, Error> {
    let path = wide_os(exe.as_os_str());
    let mut ignored = 0u32;
    // SAFETY: `path` is NUL-terminated; `ignored` is a valid out pointer.
    let size = unsafe {
        GetFileVersionInfoSizeExW(FILE_VER_GET_NEUTRAL, PCWSTR(path.as_ptr()), &mut ignored)
    };
    if size == 0 {
        return Err(last_error("GetFileVersionInfoSizeExW"));
    }
    let mut block = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: `block` has at least `size` writable bytes, aligned for the version structures.
    unsafe {
        GetFileVersionInfoExW(
            FILE_VER_GET_NEUTRAL,
            PCWSTR(path.as_ptr()),
            None,
            size,
            block.as_mut_ptr().cast(),
        )
    }
    .map_err(|error| win32("GetFileVersionInfoExW", &error))?;

    let mut translations = version_translations(&block);
    translations.push((0x0409, 1200));
    for (language, code_page) in translations {
        let sub_block =
            format!(r"\StringFileInfo\{language:04x}{code_page:04x}\{BUILD_ID_VERSION_KEY}");
        if let Some(value) = version_string(&block, &sub_block) {
            return Ok(value);
        }
    }
    Err(Error::Win32 {
        function: "VerQueryValueW",
        code: ERROR_RESOURCE_TYPE_NOT_FOUND.0,
    })
}

/// Pointer into `block` and length (as VerQueryValueW counts it) of a VERSIONINFO sub-block.
fn version_value(block: &[u64], sub_block: &str) -> Option<(*const u16, usize)> {
    let sub_block = to_wide(sub_block);
    let mut pointer: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `block` holds a version-information block filled by GetFileVersionInfoExW;
    // `sub_block` is NUL-terminated; the out pointers are valid. The returned pointer points into
    // `block`.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(sub_block.as_ptr()),
            &mut pointer,
            &mut len,
        )
    };
    (found.as_bool() && !pointer.is_null()).then_some((pointer.cast_const().cast(), len as usize))
}

/// `(language, code page)` pairs of `\VarFileInfo\Translation`.
fn version_translations(block: &[u64]) -> Vec<(u16, u16)> {
    let Some((pointer, bytes)) = version_value(block, r"\VarFileInfo\Translation") else {
        return Vec::new();
    };
    let count = bytes / 4;
    // SAFETY: VerQueryValueW returned `bytes` readable bytes of DWORD pairs inside `block`, which
    // is borrowed for this whole function.
    let words = unsafe { std::slice::from_raw_parts(pointer, count * 2) };
    let (pairs, _) = words.as_chunks::<2>();
    pairs
        .iter()
        .map(|&[language, code_page]| (language, code_page))
        .collect()
}

/// A string value; its length from VerQueryValueW is in characters, including the NUL.
fn version_string(block: &[u64], sub_block: &str) -> Option<String> {
    let (pointer, chars) = version_value(block, sub_block)?;
    // SAFETY: VerQueryValueW returned `chars` UTF-16 units inside `block`, which is borrowed for
    // this whole function.
    let units = unsafe { std::slice::from_raw_parts(pointer, chars) };
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::{TOKEN_MANDATORY_LABEL, TokenIntegrityLevel};

    /// Mandatory label of this process's token: `S-1-16-8192` medium, `S-1-16-12288` high.
    fn integrity_level() -> String {
        let token = process_token().expect("token");
        let buffer = token_information(&token, TokenIntegrityLevel, "TokenIntegrityLevel")
            .expect("integrity level");
        // SAFETY: the buffer holds a TOKEN_MANDATORY_LABEL whose SID points into it.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
        sid_to_string(sid).expect("label SID")
    }

    #[test]
    fn cargo_test_runs_unelevated() {
        // A normal `cargo test` runs at medium integrity and is not elevated. A console that is
        // elevated (or a CI runner with UAC off) runs at high integrity: then the answer flips
        // with it, which is what the check must follow.
        let level = integrity_level();
        let elevated = level == "S-1-16-12288" || level == "S-1-16-16384";
        assert_eq!(is_elevated(), Ok(elevated), "integrity level {level}");
    }

    #[test]
    fn user_sid_looks_like_a_sid() {
        let sid = current_user_sid().expect("user SID");
        assert!(sid.starts_with("S-1-5-"), "{sid}");
    }

    #[test]
    fn system32_and_its_files() {
        let system32 = system_directory().expect("System32");
        assert!(
            system32.is_absolute() && system32.is_dir(),
            "{}",
            system32.display()
        );
        assert!(system32.ends_with("System32") || system32.ends_with("system32"));
        assert_eq!(system32_file_exists("kbd106.dll"), Ok(true));
        assert_eq!(system32_file_exists("KBD101.DLL"), Ok(true));
        assert_eq!(system32_file_exists("mklm-no-such-layout.dll"), Ok(false));
        // Directories are not layout files.
        assert_eq!(system32_file_exists("drivers"), Ok(false));
        for name in ["", "..", r"..\kbd106.dll", "drivers/x.sys", "C:kbd106.dll"] {
            assert!(
                matches!(system32_file_exists(name), Err(Error::Insecure { .. })),
                "{name}"
            );
        }
    }

    #[test]
    fn command_line_and_console() {
        let line = process_command_line();
        let exe = std::env::current_exe().expect("test executable");
        let stem = exe
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("file stem");
        assert!(line.contains(stem), "{line}");
        // `cargo test` may run with or without a console; either answer is well-formed.
        assert_ne!(console_window(), Some(0));
    }

    #[test]
    fn spawned_processes_report_their_exit_code() {
        // An ordinary child (this works unelevated too): cmd exits with 7.
        let cmd = system_directory().expect("System32").join("cmd.exe");
        let child = spawn_from_elevated(&cmd, "/d /c exit 7").expect("spawn cmd");
        assert_ne!(child.pid(), 0);
        assert_eq!(child.wait(Duration::from_secs(30)), Ok(Some(7)));
        assert_eq!(child.exit_code(), Ok(Some(7)));
    }

    #[test]
    fn launch_targets_must_be_absolute_regular_files() {
        assert!(matches!(
            spawn_from_elevated(Path::new("cmd.exe"), ""),
            Err(Error::Insecure { .. })
        ));
        let directory = std::env::temp_dir();
        assert!(matches!(
            check_executable(&directory),
            Err(Error::Insecure { .. })
        ));
        assert!(check_executable(&directory.join("mklm no such file.exe")).is_err());
        let exe = std::env::current_exe().expect("test executable");
        assert_eq!(check_executable(&exe), Ok(()));
    }

    #[test]
    fn command_lines_quote_the_executable() {
        let line = command_line(
            OsStr::new(r"C:\Program Files\MKLM\mklm-helper.exe"),
            "--pipe x",
        );
        assert_eq!(
            String::from_utf16_lossy(&line),
            "\"C:\\Program Files\\MKLM\\mklm-helper.exe\" --pipe x\0"
        );
        let line = command_line(OsStr::new(r"C:\a.exe"), "");
        assert_eq!(String::from_utf16_lossy(&line), "\"C:\\a.exe\"\0");
    }

    #[test]
    fn helper_is_found_next_to_the_executable_only_if_present() {
        // The test executable's directory has no mklm-helper.exe unless it was built there.
        match helper_path() {
            Ok(path) => assert!(path.ends_with(HELPER_EXE)),
            Err(error) => assert!(
                matches!(error, Error::Win32 { .. } | Error::Insecure { .. }),
                "{error:?}"
            ),
        }
    }

    #[test]
    fn build_id_is_missing_from_files_without_it() {
        let system32 = system_directory().expect("System32");
        // kernel32.dll has a VERSIONINFO, but no MKLM build ID.
        assert_eq!(
            file_build_id(&system32.join("kernel32.dll")),
            Err(Error::Win32 {
                function: "VerQueryValueW",
                code: ERROR_RESOURCE_TYPE_NOT_FOUND.0
            })
        );
        let block_translations = {
            let path = wide_os(system32.join("kernel32.dll").as_os_str());
            let mut ignored = 0u32;
            // SAFETY: as in `file_build_id`.
            let size = unsafe {
                GetFileVersionInfoSizeExW(FILE_VER_GET_NEUTRAL, PCWSTR(path.as_ptr()), &mut ignored)
            };
            let mut block = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: as in `file_build_id`.
            unsafe {
                GetFileVersionInfoExW(
                    FILE_VER_GET_NEUTRAL,
                    PCWSTR(path.as_ptr()),
                    None,
                    size,
                    block.as_mut_ptr().cast(),
                )
            }
            .expect("kernel32 version info");
            let translations = version_translations(&block);
            let company = translations.iter().find_map(|(language, code_page)| {
                version_string(
                    &block,
                    &format!(r"\StringFileInfo\{language:04x}{code_page:04x}\CompanyName"),
                )
            });
            (translations, company)
        };
        assert!(!block_translations.0.is_empty());
        assert_eq!(
            block_translations.1.as_deref(),
            Some("Microsoft Corporation")
        );
        // The test executable has no VERSIONINFO at all.
        let exe = std::env::current_exe().expect("test executable");
        assert!(file_build_id(&exe).is_err());
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// Design m5b D.9.4, F.2 (SECURITY-6).
    #[test]
    fn environment_blocks() {
        let block = environment_block(&vars(&[
            ("windir", r"C:\Windows"),
            ("TMP", r"C:\t"),
            ("Path", r"C:\Windows\System32"),
            ("ComSpec", ""),
            ("ProgramW6432", "P"),
            ("_X", "u"),
            ("Temp", r"C:\t"),
        ]))
        .expect("block");
        assert_eq!(
            String::from_utf16(&block).unwrap(),
            "ComSpec=\0Path=C:\\Windows\\System32\0ProgramW6432=P\0Temp=C:\\t\0TMP=C:\\t\0\
             windir=C:\\Windows\0_X=u\0\0"
        );
        // Case-insensitive order: `Temp` before `TMP` (E < M), `windir` before `_X` (W < _ in
        // upper case, as Windows compares).
        assert_eq!(environment_block(&[]).unwrap(), vec![0, 0]);
        assert_eq!(
            String::from_utf16(&environment_block(&vars(&[("A", "ü😀")])).unwrap()).unwrap(),
            "A=ü😀\0\0"
        );
    }

    /// Design m5b D.9.4, F.2 (SECURITY-6): the runner's environment is the fixed list, from the
    /// system, with nothing of this process's (no user PATH, no __COMPAT_LAYER).
    #[test]
    fn the_runner_environment_is_the_fixed_list() {
        let temp = std::env::temp_dir().join("mklm-run").join("tmp");
        let env = runner_environment(&temp).expect("environment");
        let names: Vec<&str> = env.vars.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, RUNNER_ENVIRONMENT_NAMES);
        let value = |name: &str| {
            env.vars
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        let system32 = system_directory().expect("System32");
        let windows = system32
            .parent()
            .expect("Windows")
            .to_string_lossy()
            .into_owned();
        assert!(value("SystemRoot").eq_ignore_ascii_case(&windows));
        assert_eq!(value("windir"), value("SystemRoot"));
        assert_eq!(value("SystemDrive"), value("SystemRoot")[..2]);
        assert_eq!(value("TEMP"), temp.to_string_lossy());
        assert_eq!(value("TMP"), value("TEMP"));
        assert_eq!(
            value("ComSpec"),
            format!(r"{}\cmd.exe", system32.to_string_lossy())
        );
        let path = value("PATH");
        assert_eq!(path.split(';').count(), 3, "{path}");
        assert!(path.ends_with(r"\Wbem"), "{path}");
        if let Ok(own) = std::env::var("PATH") {
            assert_ne!(path, own);
        }
        assert!(value("ProgramData").len() > 3);
        assert!(!value("ProgramFiles").contains("(x86)"));
        assert!(!names.contains(&"__COMPAT_LAYER"));
        assert!(
            !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("USERPROFILE"))
        );
        // The block Windows gets from it.
        let block = environment_block(&env.vars).expect("block");
        assert_eq!(block.last(), Some(&0));
        assert!(matches!(
            runner_environment(Path::new(r"relative\tmp")),
            Err(Error::Insecure { .. })
        ));
    }

    /// The child sees the clean block only, starts in System32, and a suspended child runs only
    /// once resumed (design m5b D.7 step 16).
    #[test]
    fn clean_children() {
        let cmd = system_directory().expect("System32").join("cmd.exe");
        let env = runner_environment(&std::env::temp_dir()).expect("environment");
        // USERPROFILE is in this process's environment but not in the child's.
        assert!(std::env::var_os("USERPROFILE").is_some());
        let child = spawn_clean(
            &cmd,
            "/d /c if defined USERPROFILE (exit 9) else (if /i \"%CD%\"==\"%SystemRoot%\\System32\" (exit 4) else (exit 5))",
            &env,
            false,
        )
        .expect("spawn");
        assert!(child.thread.is_none());
        assert_eq!(child.process.wait(Duration::from_secs(30)), Ok(Some(4)));

        let mut suspended = spawn_clean(&cmd, "/d /c exit 6", &env, true).expect("spawn");
        let identity = suspended.identity().expect("identity");
        assert_eq!(identity.pid, suspended.process.pid());
        assert!(identity.creation_time > 0);
        assert_eq!(
            suspended.process.wait(Duration::from_millis(300)),
            Ok(None),
            "not running while suspended"
        );
        suspended.resume().expect("resume");
        assert!(suspended.resume().is_err(), "resumed once");
        assert_eq!(suspended.process.wait(Duration::from_secs(30)), Ok(Some(6)));

        // A suspended child that never ran can be terminated.
        let never = spawn_clean(&cmd, "/d /c exit 0", &env, true).expect("spawn");
        never.terminate(1).expect("terminate");
        assert_eq!(never.process.wait(Duration::from_secs(30)), Ok(Some(1)));

        assert!(matches!(
            spawn_clean(Path::new("cmd.exe"), "", &env, false),
            Err(Error::Insecure { .. })
        ));
        let bad = CleanEnvironment {
            vars: vec![("A=B".to_string(), "c".to_string())],
        };
        assert!(spawn_clean(&cmd, "/d /c exit 0", &bad, false).is_err());
    }

    /// Environment variable that turns [`forged_system_drive_child`] into the child of
    /// [`a_forged_system_drive_does_not_move_program_data`]: `<mode>|<expected>|<fake drive>`.
    const FORGED_DRIVE_CHILD: &str = "MKLM_WIN_TEST_FORGED_DRIVE_CHILD";

    /// Runs [`forged_system_drive_child`] in a new process of this test executable whose
    /// `SystemDrive` (and, with `forge_roots`, `SystemRoot` and `windir`) is `fake`, as the user can
    /// set them for a UAC-elevated process in `HKCU\Environment`. Its success and its output.
    fn run_forged_child(
        mode: &str,
        expected: &Path,
        fake: &Path,
        forge_roots: bool,
    ) -> (bool, String) {
        let exe = std::env::current_exe().expect("test executable");
        let mut command = std::process::Command::new(exe);
        command
            .args([
                "--exact",
                "elevation::tests::forged_system_drive_child",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(
                FORGED_DRIVE_CHILD,
                format!("{mode}|{}|{}", expected.display(), fake.display()),
            )
            .env("SystemDrive", fake)
            .stdin(std::process::Stdio::null());
        if forge_roots {
            command.env("SystemRoot", fake).env("windir", fake);
        }
        let output = command.output().expect("run the child");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), text)
    }

    /// M5b security review, finding 1 (design m5b D.9.4): a forged `SystemDrive` moves
    /// `%ProgramData%` to a folder the user chose, which the protected directories then refuse
    /// (its owner and DACL); pinned first thing, `%ProgramData%` and the runner's environment are
    /// the machine's own again.
    #[test]
    fn a_forged_system_drive_does_not_move_program_data() {
        let expected = crate::protected_dir::program_data_dir().expect("ProgramData");
        let fake = std::env::temp_dir().join(format!(
            "mklm-forged-drive-{}",
            crate::session::new_uuid().expect("uuid")
        ));
        std::fs::create_dir_all(fake.join("ProgramData")).expect("the fake ProgramData");
        let unpinned = run_forged_child("unpinned", &expected, &fake, false);
        let pinned = run_forged_child("pinned", &expected, &fake, true);
        let _ = std::fs::remove_dir_all(&fake);
        assert!(unpinned.0, "the unpinned child failed:\n{}", unpinned.1);
        assert!(pinned.0, "the pinned child failed:\n{}", pinned.1);
    }

    /// The child side of [`a_forged_system_drive_does_not_move_program_data`]; does nothing in a
    /// normal test run.
    #[test]
    fn forged_system_drive_child() {
        use crate::protected_dir::{DataDir, program_data_dir, verify_protected_dir};

        let Ok(spec) = std::env::var(FORGED_DRIVE_CHILD) else {
            return;
        };
        let mut parts = spec.splitn(3, '|');
        let mode = parts.next().expect("mode").to_string();
        let expected = PathBuf::from(parts.next().expect("expected"));
        let fake = PathBuf::from(parts.next().expect("fake drive"));
        if mode == "pinned" {
            // Before any known-folder call: the shell caches the first answer.
            pin_system_environment().expect("pin");
            let (windows, drive) = system_roots().expect("the system's roots");
            for (name, value) in [
                ("SystemDrive", &drive),
                ("SystemRoot", &windows),
                ("windir", &windows),
            ] {
                assert_eq!(std::env::var(name).ok().as_ref(), Some(value), "{name}");
            }
            assert_eq!(program_data_dir().expect("ProgramData"), expected);
            let env = runner_environment(&expected.join("tmp")).expect("environment");
            let value = |name: &str| {
                env.vars
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, value)| value.clone())
            };
            assert_eq!(
                value("ProgramData").map(PathBuf::from),
                Some(expected.clone())
            );
            assert_eq!(value("SystemDrive"), Some(drive));
        } else {
            // What the pin prevents: the known folder follows this process's SystemDrive ...
            assert_eq!(
                program_data_dir().expect("ProgramData"),
                fake.join("ProgramData"),
                "SHGetKnownFolderPath no longer expands %SystemDrive% from the environment"
            );
            // ... and, as a second line, the protected directories refuse a %ProgramData% that the
            // user owns or may rename things in.
            match verify_protected_dir(DataDir::Base) {
                Err(Error::Insecure { path, .. }) => {
                    assert_eq!(PathBuf::from(path), fake.join("ProgramData"));
                }
                other => panic!("a user's %ProgramData% was accepted: {other:?}"),
            }
        }
    }

    #[test]
    fn environment_blocks_refuse_what_windows_would_misread() {
        for pairs in [
            vec![("A=B", "c")],
            vec![("=C:", r"C:\")],
            vec![("", "x")],
            vec![("A\0B", "c")],
            vec![("A", "b\0c")],
            vec![("Path", "x"), ("PATH", "y")],
            vec![("temp", "x"), ("TMP", "y"), ("TEMP", "z")],
        ] {
            assert_eq!(
                environment_block(&vars(&pairs)),
                Err(Error::Win32 {
                    function: "environment_block",
                    code: 87
                }),
                "{pairs:?}"
            );
        }
    }
}
