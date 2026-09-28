//! Process identity and liveness (M2): the journal owner check of plan 2.3 and the pipe peer's
//! image check.
//!
//! No process handle is opened. A normal process's DACL grants nothing to Administrators, so
//! `OpenProcess` fails when the helper runs as another user (a standard user who typed an
//! administrator's credentials into UAC; design review S3). Everything here reads the system
//! process table instead, which works across users and integrity levels:
//! - `NtQuerySystemInformation(SystemProcessInformation)`: PID and `CreateTime` of every process;
//! - `NtQuerySystemInformation(SystemProcessIdInformation)`: the image name of one PID, as an NT
//!   path (`\Device\HarddiskVolume3\…`). Paths are compared in that NT form on both sides, never
//!   converted to drive letters.
//!
//! Both structures are defined here with `#[repr(C)]` (ntexapi.h); the `windows` crate lacks the
//! second and only has the first in an opaque form.

use std::mem::offset_of;

use mklm_core::{Liveness, ProcessIdentity};
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemProcessInformation,
};
use windows::Win32::Foundation::{FILETIME, HANDLE, STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING};
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW, OPEN_EXISTING, SYNCHRONIZE,
    VOLUME_NAME_NT,
};
use windows::Win32::System::RemoteDesktop::{
    WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes};
use windows::core::{PCWSTR, PWSTR};

use crate::error::Error;
use crate::security::sid_to_string;
use crate::sys::{last_error, nt_check, own, raw, wide_os, win32};

/// `SystemProcessIdInformation`.
const SYSTEM_PROCESS_ID_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(88);

/// Attempts while the process table or an image name keeps outgrowing the buffer.
const MAX_ATTEMPTS: usize = 8;

/// `SYSTEM_PROCESS_ID_INFORMATION`: the caller provides the name buffer.
#[repr(C)]
struct SystemProcessIdInformation {
    process_id: HANDLE,
    image_name: UNICODE_STRING,
}

/// The fixed head of `SYSTEM_PROCESS_INFORMATION`, up to `SessionId`. Only used for its field
/// offsets; entries are parsed from the byte buffer.
#[repr(C)]
#[allow(dead_code, reason = "the kernel's layout; only some fields are read")]
struct ProcessEntryHead {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UNICODE_STRING,
    base_priority: i32,
    unique_process_id: HANDLE,
    inherited_from_unique_process_id: HANDLE,
    handle_count: u32,
    session_id: u32,
}

// The documented x64 / ARM64 offsets of the fields this module reads.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(offset_of!(ProcessEntryHead, create_time) == 32);
    assert!(offset_of!(ProcessEntryHead, image_name) == 56);
    assert!(offset_of!(ProcessEntryHead, unique_process_id) == 80);
    assert!(offset_of!(ProcessEntryHead, session_id) == 100);
};

/// One process of `SystemProcessInformation`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessEntry {
    pid: u32,
    create_time: u64,
    session_id: u32,
    /// The image's file name (`ImageName`, no directory); `None` for the idle process.
    image_name: Option<String>,
}

/// This process's PID and creation time (`GetProcessTimes` on the pseudo handle; the same
/// `CreateTime` that `SystemProcessInformation` reports).
pub fn current_process_identity() -> Result<ProcessIdentity, Error> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle with full access to this process; all
    // four out pointers are valid.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    }
    .map_err(|error| win32("GetProcessTimes", &error))?;
    Ok(ProcessIdentity {
        // SAFETY: GetCurrentProcessId has no preconditions.
        pid: unsafe { GetCurrentProcessId() },
        creation_time: (u64::from(creation.dwHighDateTime) << 32)
            | u64::from(creation.dwLowDateTime),
    })
}

/// From `SystemProcessInformation`: a process with the same PID and creation time →
/// [`Liveness::Alive`]; none → [`Liveness::Dead`]; the query itself failed →
/// [`Liveness::Unknown`].
pub fn process_liveness(process: &ProcessIdentity) -> Liveness {
    match process_table() {
        Ok(table) if table.contains(&(process.pid, process.creation_time)) => Liveness::Alive,
        Ok(_) => Liveness::Dead,
        Err(_) => Liveness::Unknown,
    }
}

/// `(PID, CreateTime)` of every process.
fn process_table() -> Result<Vec<(u32, u64)>, Error> {
    Ok(process_entries()?
        .into_iter()
        .map(|entry| (entry.pid, entry.create_time))
        .collect())
}

/// Every process of `SystemProcessInformation`.
fn process_entries() -> Result<Vec<ProcessEntry>, Error> {
    const FUNCTION: &str = "NtQuerySystemInformation(SystemProcessInformation)";
    let mut size: usize = 512 * 1024;
    for _ in 0..MAX_ATTEMPTS {
        // u64 elements keep the entries 8-byte aligned, as the kernel lays them out.
        let mut buffer = vec![0u64; size.div_ceil(8)];
        let len = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
        let mut returned = 0u32;
        // SAFETY: `buffer` has `len` writable bytes; `returned` is a valid out pointer.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buffer.as_mut_ptr().cast(),
                len,
                &mut returned,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // Processes can start before the next call: leave some room.
            size = (returned as usize).max(size) + 64 * 1024;
            continue;
        }
        nt_check(FUNCTION, status)?;
        // The image names point into `buffer`: their offsets are taken relative to its address.
        let base = buffer.as_ptr() as usize;
        let bytes: Vec<u8> = buffer.iter().flat_map(|word| word.to_ne_bytes()).collect();
        let used = (returned as usize).min(bytes.len());
        return parse_process_entries(&bytes[..used], base).ok_or_else(|| Error::UnexpectedData {
            path: "SystemProcessInformation".to_string(),
        });
    }
    Err(Error::NtStatus {
        function: FUNCTION,
        status: STATUS_INFO_LENGTH_MISMATCH.0 as u32,
    })
}

/// [`parse_process_entries`] without the image names.
#[cfg(test)]
fn parse_process_table(bytes: &[u8]) -> Option<Vec<(u32, u64)>> {
    Some(
        parse_process_entries(bytes, 0)?
            .into_iter()
            .map(|entry| (entry.pid, entry.create_time))
            .collect(),
    )
}

/// Walks the `NextEntryOffset` chain; `None` when an entry does not fit in `bytes`. `base` is the
/// address the kernel wrote to: `ImageName.Buffer` points at `base + offset` of its text, which
/// must lie within `bytes` (else the name is `None`).
fn parse_process_entries(bytes: &[u8], base: usize) -> Option<Vec<ProcessEntry>> {
    let head = size_of::<ProcessEntryHead>();
    let pid_offset = offset_of!(ProcessEntryHead, unique_process_id);
    let time_offset = offset_of!(ProcessEntryHead, create_time);
    let session_offset = offset_of!(ProcessEntryHead, session_id);
    let name_offset = offset_of!(ProcessEntryHead, image_name);
    let name_buffer_offset = name_offset + offset_of!(UNICODE_STRING, Buffer);
    let mut table = Vec::new();
    let mut offset = 0usize;
    loop {
        let entry = bytes.get(offset..offset.checked_add(head)?)?;
        let next = u32::from_ne_bytes(entry.get(..4)?.try_into().ok()?) as usize;
        let create_time =
            i64::from_ne_bytes(entry.get(time_offset..time_offset + 8)?.try_into().ok()?);
        let pid_bytes = entry.get(pid_offset..pid_offset + size_of::<usize>())?;
        let pid = usize::from_ne_bytes(pid_bytes.try_into().ok()?);
        let session_id = u32::from_ne_bytes(
            entry
                .get(session_offset..session_offset + 4)?
                .try_into()
                .ok()?,
        );
        let name_len =
            u16::from_ne_bytes(entry.get(name_offset..name_offset + 2)?.try_into().ok()?);
        let name_pointer = usize::from_ne_bytes(
            entry
                .get(name_buffer_offset..name_buffer_offset + size_of::<usize>())?
                .try_into()
                .ok()?,
        );
        let image_name = image_name(bytes, base, name_pointer, usize::from(name_len));
        // PIDs are multiples of 4 below 2^32; a HANDLE-sized field only carries them.
        table.push(ProcessEntry {
            pid: u32::try_from(pid).ok()?,
            create_time: create_time as u64,
            session_id,
            image_name,
        });
        if next == 0 {
            return Some(table);
        }
        offset = offset.checked_add(next)?;
    }
}

/// The UTF-16 text of `len` bytes at address `pointer`, when it lies within `bytes` (which was
/// written at address `base`).
fn image_name(bytes: &[u8], base: usize, pointer: usize, len: usize) -> Option<String> {
    if pointer == 0 || len == 0 || !len.is_multiple_of(2) {
        return None;
    }
    let start = pointer.checked_sub(base)?;
    let text = bytes.get(start..start.checked_add(len)?)?;
    let (units, _) = text.as_chunks::<2>();
    let units: Vec<u16> = units.iter().map(|pair| u16::from_ne_bytes(*pair)).collect();
    String::from_utf16(&units).ok()
}

/// NT image path of a running process (`SystemProcessIdInformation`), e.g.
/// `\Device\HarddiskVolume3\Program Files\MKLM\mklm-cli.exe`.
pub fn process_image_nt_path(pid: u32) -> Result<String, Error> {
    const FUNCTION: &str = "NtQuerySystemInformation(SystemProcessIdInformation)";
    // In bytes; UNICODE_STRING lengths are u16 and even.
    let mut capacity: u16 = 520;
    for _ in 0..MAX_ATTEMPTS {
        let mut name = vec![0u16; usize::from(capacity / 2)];
        let mut info = SystemProcessIdInformation {
            process_id: HANDLE(std::ptr::without_provenance_mut(pid as usize)),
            image_name: UNICODE_STRING {
                Length: 0,
                MaximumLength: capacity,
                Buffer: PWSTR(name.as_mut_ptr()),
            },
        };
        // SAFETY: `info` is the plain-data layout of class 88; its name buffer points at `name`,
        // which has `capacity` writable bytes and outlives the call.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_ID_INFORMATION,
                (&raw mut info).cast(),
                size_of::<SystemProcessIdInformation>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // `MaximumLength` now holds the size the name needs.
            let needed = info.image_name.MaximumLength;
            capacity = if needed > capacity {
                needed.saturating_add(1) & !1
            } else {
                capacity.saturating_mul(2) & !1
            };
            continue;
        }
        nt_check(FUNCTION, status)?;
        let units = name
            .get(..usize::from(info.image_name.Length / 2))
            .unwrap_or_default();
        return String::from_utf16(units)
            .ok()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| Error::UnexpectedData {
                path: format!("image name of process {pid}"),
            });
    }
    Err(Error::NtStatus {
        function: FUNCTION,
        status: STATUS_INFO_LENGTH_MISMATCH.0 as u32,
    })
}

/// True when `pid`'s image is in the same directory as this process's image, both taken from
/// [`process_image_nt_path`] and compared case-insensitively (the helper's check of its caller,
/// design E.2).
pub fn same_image_directory(pid: u32) -> Result<bool, Error> {
    // SAFETY: GetCurrentProcessId has no preconditions.
    let own = process_image_nt_path(unsafe { GetCurrentProcessId() })?;
    let other = process_image_nt_path(pid)?;
    Ok(match (image_directory(&own), image_directory(&other)) {
        (Some(own), Some(other)) => equal_ignoring_case(own, other),
        _ => false,
    })
}

// ---- M5b additions (design m5b H.3, D.7, D.8; WP-H) ----

/// PID → identity with its creation time (SystemProcessInformation); `None` if gone.
pub fn process_identity(pid: u32) -> Result<Option<ProcessIdentity>, Error> {
    Ok(process_table()?
        .into_iter()
        .find(|&(found, _)| found == pid)
        .map(|(pid, creation_time)| ProcessIdentity { pid, creation_time }))
}

/// NT path of a file (`GetFinalPathNameByHandleW(VOLUME_NAME_NT)`), for comparing with
/// `process_image_nt_path`.
pub fn file_nt_path(path: &std::path::Path) -> Result<String, Error> {
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
    let wide = wide_os(path.as_os_str());
    // SAFETY: `wide` is NUL-terminated and outlives the call; the handle asks for attributes only
    // (FILE_FLAG_BACKUP_SEMANTICS lets a directory be opened too).
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
            share,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|error| win32("CreateFileW", &error))?;
    // SAFETY: CreateFileW succeeded, so `handle` is an open handle this process owns.
    let handle = unsafe { own(handle) };
    let mut buffer = vec![0u16; 512];
    for _ in 0..4 {
        // SAFETY: `handle` is open; the slice tells the API how many units it may write.
        let len = unsafe {
            GetFinalPathNameByHandleW(
                raw(&handle),
                &mut buffer,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_NT.0),
            )
        } as usize;
        if len == 0 {
            return Err(last_error("GetFinalPathNameByHandleW"));
        }
        if len < buffer.len() {
            buffer.truncate(len);
            return String::from_utf16(&buffer).map_err(|_| Error::UnexpectedData {
                path: path.display().to_string(),
            });
        }
        // Too small: `len` is the size needed, including the NUL.
        buffer = vec![0u16; len + 1];
    }
    Err(last_error("GetFinalPathNameByHandleW"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProcess {
    pub identity: ProcessIdentity,
    /// Index into the `nt_paths` argument.
    pub path_index: usize,
    pub session_id: u32,
}

/// Every process whose image NT path equals one of `nt_paths` (case-insensitive).
///
/// Only processes whose image *file name* matches one of the paths' file names are asked for
/// their full path (`SystemProcessIdInformation`), so a lookup costs one process-table read and a
/// few queries. A process that ends in between is skipped.
pub fn processes_with_images(nt_paths: &[String]) -> Result<Vec<ImageProcess>, Error> {
    let file_names: Vec<&str> = nt_paths
        .iter()
        .map(|path| path.rsplit('\\').next().unwrap_or(path))
        .collect();
    let mut found = Vec::new();
    for entry in process_entries()? {
        let Some(name) = entry.image_name.as_deref() else {
            continue;
        };
        if !file_names
            .iter()
            .any(|wanted| equal_ignoring_case(name, wanted))
        {
            continue;
        }
        let Ok(image) = process_image_nt_path(entry.pid) else {
            continue;
        };
        if let Some(path_index) = nt_paths
            .iter()
            .position(|path| equal_ignoring_case(&image, path))
        {
            found.push(ImageProcess {
                identity: ProcessIdentity {
                    pid: entry.pid,
                    creation_time: entry.create_time,
                },
                path_index,
                session_id: entry.session_id,
            });
        }
    }
    Ok(found)
}

/// One row of `WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessUser {
    pub session_id: u32,
    /// `pUserSid` as `S-1-…` text (`ConvertSidToStringSidW`); `None` when absent.
    pub user_sid: Option<String>,
}

/// Frees the `WTSEnumerateProcessesW` array.
struct WtsMemory(*mut WTS_PROCESS_INFOW);

impl Drop for WtsMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the array was allocated by WTSEnumerateProcessesW and is freed once.
            unsafe { WTSFreeMemory(self.0.cast()) };
        }
    }
}

/// PID → session and user SID of every process (design m5b D.8 step 2; FIX-VERIFICATION-5).
/// Whether `pUserSid` is filled for other users' processes when called from an elevated,
/// non-SYSTEM process is unverified (design m5b I.12).
pub fn process_users() -> Result<std::collections::HashMap<u32, ProcessUser>, Error> {
    let mut rows: *mut WTS_PROCESS_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the local server (None is WTS_CURRENT_SERVER_HANDLE); version 1; both out pointers
    // are valid. The array is freed by `WtsMemory`.
    unsafe { WTSEnumerateProcessesW(None, 0, 1, &mut rows, &mut count) }
        .map_err(|error| win32("WTSEnumerateProcessesW", &error))?;
    let memory = WtsMemory(rows);
    if memory.0.is_null() {
        return Ok(std::collections::HashMap::new());
    }
    // SAFETY: WTSEnumerateProcessesW returned `count` rows at `rows`, alive until `memory` drops
    // at the end of this function.
    let rows = unsafe { std::slice::from_raw_parts(memory.0, count as usize) };
    let mut users = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let user_sid = if row.pUserSid.0.is_null() {
            None
        } else {
            sid_to_string(row.pUserSid).ok()
        };
        users.insert(
            row.ProcessId,
            ProcessUser {
                session_id: row.SessionId,
                user_sid,
            },
        );
    }
    Ok(users)
}

/// How often [`wait_for_exit`] looks.
const EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Polls `process_liveness` every 250 ms; true when all are gone within `timeout`. A liveness that
/// cannot be read counts as alive (the wait never ends early on a doubt).
pub fn wait_for_exit(
    processes: &[ProcessIdentity],
    timeout: std::time::Duration,
) -> Result<bool, Error> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let table = process_table();
        let gone = match &table {
            Ok(table) => processes
                .iter()
                .all(|process| !table.contains(&(process.pid, process.creation_time))),
            Err(_) => false,
        };
        if gone {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return match table {
                Ok(_) => Ok(false),
                Err(error) => Err(error),
            };
        }
        std::thread::sleep(remaining.min(EXIT_POLL));
    }
}

/// Directory part of an NT image path (without the last `\`); `None` without one.
fn image_directory(path: &str) -> Option<&str> {
    path.rsplit_once('\\')
        .map(|(directory, _)| directory)
        .filter(|directory| !directory.is_empty())
}

/// Ordinal comparison ignoring case, as the file system compares names.
pub(crate) fn equal_ignoring_case(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    // SAFETY: both slices are valid UTF-16 buffers with their lengths.
    let result = unsafe { CompareStringOrdinal(&a, &b, true) };
    result == CSTR_EQUAL
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};

    use super::*;

    fn creation_time(child: &std::process::Child) -> u64 {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the child's process handle is open (owned by `child`); out pointers are valid.
        unsafe {
            GetProcessTimes(
                HANDLE(child.as_raw_handle()),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
        .expect("GetProcessTimes of the child");
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime)
    }

    #[test]
    fn this_process_is_alive() {
        let me = current_process_identity().expect("own identity");
        assert_eq!(me.pid, std::process::id());
        assert_eq!(process_liveness(&me), Liveness::Alive);
        // Same PID, other creation time: a reused PID is not the same process.
        let impostor = ProcessIdentity {
            creation_time: me.creation_time + 1,
            ..me
        };
        assert_eq!(process_liveness(&impostor), Liveness::Dead);
    }

    #[test]
    fn an_exited_child_is_dead() {
        // The test executable itself, listing its tests: starts and exits on its own.
        let exe = std::env::current_exe().expect("test executable");
        let mut child = Command::new(exe)
            .args(["--list", "--exact", "no such test"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start child");
        let identity = ProcessIdentity {
            pid: child.id(),
            creation_time: creation_time(&child),
        };
        child.wait().expect("child exits");
        assert_eq!(process_liveness(&identity), Liveness::Dead);
    }

    #[test]
    fn own_image_path_is_the_test_executable() {
        let path = process_image_nt_path(std::process::id()).expect("own image path");
        let exe = std::env::current_exe().expect("test executable");
        let file_name = exe
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name");
        assert!(path.starts_with(r"\Device\"), "{path}");
        assert!(
            path.to_ascii_lowercase()
                .ends_with(&format!(r"\{}", file_name.to_ascii_lowercase())),
            "{path} vs {file_name}"
        );
        assert_eq!(same_image_directory(std::process::id()), Ok(true));
    }

    #[test]
    fn other_directories_and_missing_processes() {
        // PID 4 is the System process, whose image is not in the test's directory (and may have
        // no path at all).
        assert_ne!(same_image_directory(4), Ok(true));
        // PIDs are multiples of 4, so this one never exists.
        assert!(process_image_nt_path(0xFFFF_FFF3).is_err());
    }

    #[test]
    fn directory_comparison() {
        assert_eq!(
            image_directory(r"\Device\HarddiskVolume3\MKLM\mklm-cli.exe"),
            Some(r"\Device\HarddiskVolume3\MKLM")
        );
        assert_eq!(image_directory("mklm-cli.exe"), None);
        assert_eq!(image_directory(r"\mklm-cli.exe"), None);
        assert!(equal_ignoring_case(
            r"\Device\HarddiskVolume3\Program Files\MKLM",
            r"\device\harddiskvolume3\PROGRAM FILES\mklm"
        ));
        assert!(!equal_ignoring_case(
            r"\Device\HarddiskVolume3\MKLM",
            r"\Device\HarddiskVolume4\MKLM"
        ));
    }

    // ---- M5b additions ----

    #[test]
    fn identities_and_images_of_this_process() {
        let me = current_process_identity().expect("own identity");
        assert_eq!(process_identity(me.pid), Ok(Some(me)));
        assert_eq!(process_identity(0xFFFF_FFF3), Ok(None));
        let exe = std::env::current_exe().expect("test executable");
        let file = file_nt_path(&exe).expect("NT path of the test executable");
        let image = process_image_nt_path(me.pid).expect("own image path");
        assert!(file.starts_with(r"\Device\"), "{file}");
        assert!(equal_ignoring_case(&file, &image), "{file} / {image}");
        assert!(file_nt_path(&exe.with_file_name("mklm no such file.exe")).is_err());
        // This process runs its own image, in its token's session; nothing runs a missing file.
        let found = processes_with_images(&[
            r"\Device\HarddiskVolume99\no\such\mklm.exe".to_string(),
            file.to_uppercase(),
        ])
        .expect("process lookup");
        let mine: Vec<_> = found.iter().filter(|p| p.identity == me).collect();
        assert_eq!(mine.len(), 1, "{found:?}");
        assert_eq!(mine[0].path_index, 1);
        let users = process_users().expect("WTSEnumerateProcessesW");
        let row = users.get(&me.pid).expect("this process is listed");
        assert_eq!(row.session_id, mine[0].session_id);
        assert_eq!(
            row.user_sid.as_deref(),
            Some(crate::elevation::current_user_sid().expect("SID").as_str())
        );
    }

    #[test]
    fn waiting_for_processes_to_exit() {
        let exe = std::env::current_exe().expect("test executable");
        let mut child = Command::new(exe)
            .args(["--list", "--exact", "no such test"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start child");
        let identity = ProcessIdentity {
            pid: child.id(),
            creation_time: creation_time(&child),
        };
        child.wait().expect("child exits");
        assert_eq!(
            wait_for_exit(&[identity], std::time::Duration::from_secs(5)),
            Ok(true)
        );
        assert_eq!(wait_for_exit(&[], std::time::Duration::ZERO), Ok(true));
        let me = current_process_identity().expect("own identity");
        let started = std::time::Instant::now();
        assert_eq!(
            wait_for_exit(&[identity, me], std::time::Duration::from_millis(300)),
            Ok(false)
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(250));
    }

    #[test]
    fn image_names_are_read_within_the_buffer_only() {
        let text: Vec<u8> = "mklm.exe"
            .encode_utf16()
            .flat_map(u16::to_ne_bytes)
            .collect();
        let mut bytes = vec![0u8; 16];
        bytes.extend_from_slice(&text);
        let base = 0x1000;
        assert_eq!(
            image_name(&bytes, base, base + 16, text.len()).as_deref(),
            Some("mklm.exe")
        );
        assert_eq!(image_name(&bytes, base, 0, text.len()), None);
        assert_eq!(image_name(&bytes, base, base + 16, 0), None);
        assert_eq!(image_name(&bytes, base, base + 16, 3), None);
        assert_eq!(image_name(&bytes, base, base - 2, 4), None);
        assert_eq!(image_name(&bytes, base, base + 16, text.len() + 2), None);
    }

    #[test]
    fn process_table_parsing_stops_at_bad_offsets() {
        let head = size_of::<ProcessEntryHead>();
        let mut bytes = vec![0u8; head * 2];
        // First entry: next = head, pid 8; second: last, pid 12.
        bytes[..4].copy_from_slice(&(head as u32).to_ne_bytes());
        let pid = offset_of!(ProcessEntryHead, unique_process_id);
        bytes[pid..pid + size_of::<usize>()].copy_from_slice(&8usize.to_ne_bytes());
        bytes[head + pid..head + pid + size_of::<usize>()].copy_from_slice(&12usize.to_ne_bytes());
        assert_eq!(parse_process_table(&bytes), Some(vec![(8, 0), (12, 0)]));
        // A next offset past the end.
        bytes[head..head + 4].copy_from_slice(&(head as u32).to_ne_bytes());
        assert_eq!(parse_process_table(&bytes), None);
        assert_eq!(parse_process_table(&[]), None);
    }
}
