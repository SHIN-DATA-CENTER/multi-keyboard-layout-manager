//! Security descriptors of the places MKLM keeps its state in (design sections C.1, D.9 and G.1):
//! building them from SDDL, and checking the owner and DACL of what is already there.
//!
//! The check is the same for registry keys, directories and files, with a per-kind list of
//! rights that no one but SYSTEM and Administrators may hold ([`AclPolicy`]).

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE,
    HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    INHERIT_ONLY_ACE, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid,
    WinCreatorGroupSid, WinCreatorOwnerSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_READ_DATA, FILE_WRITE_ATTRIBUTES,
    FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
};
use windows::Win32::System::Registry::{
    HKEY, KEY_CREATE_LINK, KEY_CREATE_SUB_KEY, KEY_SET_VALUE, RegGetKeySecurity,
};
use windows::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, ACCESS_DENIED_CALLBACK_ACE_TYPE,
    ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_DENIED_OBJECT_ACE_TYPE,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use crate::error::{Error, win32_code};
use crate::props::to_wide;
use crate::sys::win32;

/// `ACCESS_SYSTEM_SECURITY`: reading and writing the SACL.
const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;

/// Rights that change any securable object: delete it, rewrite its DACL or owner, its SACL, and
/// the generic rights that include writing.
const OBJECT_WRITE_RIGHTS: u32 = DELETE.0
    | WRITE_DAC.0
    | WRITE_OWNER.0
    | ACCESS_SYSTEM_SECURITY
    | GENERIC_ALL.0
    | GENERIC_WRITE.0;

/// Rights that change a file or a directory's entries: write data / add file, append data / add
/// subdirectory, extended attributes, attributes, delete child.
const FILE_WRITE_RIGHTS: u32 = FILE_WRITE_DATA.0
    | FILE_APPEND_DATA.0
    | FILE_WRITE_EA.0
    | FILE_WRITE_ATTRIBUTES.0
    | FILE_DELETE_CHILD.0;

/// Rights that change a registry key: set a value, create a subkey or a link.
const KEY_WRITE_RIGHTS: u32 = KEY_SET_VALUE.0 | KEY_CREATE_SUB_KEY.0 | KEY_CREATE_LINK.0;

/// `NT SERVICE\TrustedInstaller`.
const TRUSTED_INSTALLER_SID: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

/// What a securable object must look like to be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AclPolicy {
    /// Rights that no ACE may grant to a SID other than SYSTEM and Administrators.
    forbidden: u32,
    /// The DACL must not inherit entries from the parent (`SE_DACL_PROTECTED`).
    require_protected: bool,
    /// TrustedInstaller counts as trusted too (as owner and in ACEs): Windows' own folders.
    trusted_installer: bool,
}

/// Directories under `%ProgramData%\SHIN DATA CENTER` (design D.9, G.1).
pub(crate) const DIRECTORY_POLICY: AclPolicy = AclPolicy {
    forbidden: OBJECT_WRITE_RIGHTS | FILE_WRITE_RIGHTS,
    require_protected: true,
    trusted_installer: false,
};

/// `%ProgramData%` itself, the parent of `SHIN DATA CENTER` (design m5b D.9.4). Users may create
/// folders and files in it (Windows grants that), but no one else may rename or delete it or what
/// is in it (`DELETE`, `FILE_DELETE_CHILD`) or change its DACL or owner: a validated
/// `SHIN DATA CENTER` could otherwise be renamed away by its parent's rights.
pub(crate) const PROGRAM_DATA_POLICY: AclPolicy = AclPolicy {
    forbidden: DELETE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | ACCESS_SYSTEM_SECURITY
        | GENERIC_ALL.0
        | FILE_DELETE_CHILD.0,
    require_protected: false,
    trusted_installer: true,
};

/// The lock file: nobody else may even read it, because a handle with read access is enough to
/// hold a byte-range lock (`LockFileEx`), a denial of service on every write (design D.9).
pub(crate) const LOCK_FILE_POLICY: AclPolicy = AclPolicy {
    forbidden: OBJECT_WRITE_RIGHTS | FILE_WRITE_RIGHTS | FILE_READ_DATA.0 | GENERIC_READ.0,
    require_protected: true,
    trusted_installer: false,
};

/// Journal keys under `HKLM\SOFTWARE\SHIN DATA CENTER` (design C.1: owner and write access only).
pub(crate) const KEY_POLICY: AclPolicy = AclPolicy {
    forbidden: OBJECT_WRITE_RIGHTS | KEY_WRITE_RIGHTS,
    require_protected: false,
    trusted_installer: false,
};

/// A security descriptor that Windows allocated with `LocalAlloc` (from SDDL or
/// `GetSecurityInfo`); freed on drop.
#[derive(Debug)]
pub(crate) struct LocalSd(PSECURITY_DESCRIPTOR);

impl LocalSd {
    /// Builds a self-relative descriptor from SDDL.
    pub(crate) fn from_sddl(sddl: &str) -> Result<Self, Error> {
        let text = to_wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR(ptr::null_mut());
        // SAFETY: `text` is NUL-terminated and outlives the call; `sd` is a valid out pointer. On
        // success it receives a LocalAlloc'd descriptor that `Drop` frees.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|error| {
            win32(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
                &error,
            )
        })?;
        Ok(Self(sd))
    }

    pub(crate) fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }

    /// `SECURITY_ATTRIBUTES` that point at this descriptor. Only valid while `self` lives.
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for LocalSd {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: the descriptor was allocated with LocalAlloc by the API that returned it and
            // is freed exactly once, here.
            unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
        }
    }
}

/// Owner and DACL of an open file or directory (the handle needs `READ_CONTROL`).
pub(crate) fn file_security(handle: HANDLE) -> Result<LocalSd, Error> {
    let mut sd = PSECURITY_DESCRIPTOR(ptr::null_mut());
    // SAFETY: `handle` is an open file handle; only the whole descriptor is requested, through a
    // valid out pointer. It is LocalAlloc'd and freed by `LocalSd`.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut sd),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::Win32 {
            function: "GetSecurityInfo",
            code: status.0,
        });
    }
    Ok(LocalSd(sd))
}

/// Owner and DACL of an open registry key (the key needs `READ_CONTROL`), in an 8-byte aligned
/// buffer.
#[derive(Debug)]
pub(crate) struct KeySd(Vec<u64>);

impl KeySd {
    pub(crate) fn as_ptr(&mut self) -> PSECURITY_DESCRIPTOR {
        PSECURITY_DESCRIPTOR(self.0.as_mut_ptr().cast())
    }
}

pub(crate) fn key_security(key: HKEY) -> Result<KeySd, Error> {
    let mut buffer: Vec<u64> = Vec::new();
    let mut size = 0u32;
    for _ in 0..4 {
        let pointer =
            (!buffer.is_empty()).then(|| PSECURITY_DESCRIPTOR(buffer.as_mut_ptr().cast()));
        // SAFETY: `key` is an open key; `pointer` is either absent (size query) or a buffer of at
        // least `size` writable bytes; `size` is a valid in/out pointer.
        let status = unsafe {
            RegGetKeySecurity(
                key,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                pointer,
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS if pointer.is_some() => return Ok(KeySd(buffer)),
            ERROR_SUCCESS | ERROR_INSUFFICIENT_BUFFER if size > 0 => {
                buffer = vec![0u64; (size as usize).div_ceil(8)];
            }
            other => {
                return Err(Error::Win32 {
                    function: "RegGetKeySecurity",
                    code: other.0,
                });
            }
        }
    }
    Err(Error::Win32 {
        function: "RegGetKeySecurity",
        code: ERROR_INSUFFICIENT_BUFFER.0,
    })
}

/// Checks that `sd` is owned by SYSTEM or Administrators (or TrustedInstaller, when `policy` says
/// so), has a DACL (protected when `policy` says so), and that no ACE grants any right of
/// `policy.forbidden` to another SID. Denying ACEs never hurt; allowing ACEs of a type this check
/// does not understand fail it. Inherit-only ACEs for CREATOR OWNER / CREATOR GROUP are templates
/// for children, which only the trusted SIDs can create, so they are accepted.
///
/// Returns the reason in words on failure.
pub(crate) fn check_descriptor(sd: PSECURITY_DESCRIPTOR, policy: AclPolicy) -> Result<(), String> {
    let trusted = |sid: PSID| {
        is_trusted(sid)
            || (policy.trusted_installer
                && sid_to_string(sid).is_ok_and(|text| text == TRUSTED_INSTALLER_SID))
    };
    let mut owner = PSID(ptr::null_mut());
    let mut defaulted = BOOL(0);
    // SAFETY: `sd` is a valid descriptor for the duration of this function; both out pointers are
    // valid. The owner SID points into the descriptor.
    unsafe { GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted) }
        .map_err(|error| format!("its owner cannot be read (error {})", win32_code(&error)))?;
    if owner.is_invalid() {
        return Err("it has no owner".to_string());
    }
    if !trusted(owner) {
        return Err(format!("it is owned by {}", sid_text(owner)));
    }

    if policy.require_protected {
        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: `sd` is valid; both out pointers are valid.
        unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) }.map_err(
            |error| {
                format!(
                    "its control flags cannot be read (error {})",
                    win32_code(&error)
                )
            },
        )?;
        if control & SE_DACL_PROTECTED.0 == 0 {
            return Err("its DACL inherits entries from the parent folder".to_string());
        }
    }

    let mut present = BOOL(0);
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: `sd` is valid; the out pointers are valid. The DACL points into the descriptor.
    unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) }
        .map_err(|error| format!("its DACL cannot be read (error {})", win32_code(&error)))?;
    if !present.as_bool() || dacl.is_null() {
        return Err("it has no DACL, which grants everyone full access".to_string());
    }
    // SAFETY: `dacl` is non-null and points to the ACL header inside the descriptor.
    let count = unsafe { ptr::read_unaligned(&raw const (*dacl).AceCount) };
    for index in 0..u32::from(count) {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: `dacl` is a valid ACL and `index` is below its ACE count; `ace` is a valid out
        // pointer that receives a pointer into the ACL.
        unsafe { GetAce(dacl, index, &mut ace) }
            .map_err(|error| format!("its DACL cannot be read (error {})", win32_code(&error)))?;
        // SAFETY: GetAce returned a pointer to a whole ACE inside the ACL, and every ACE starts
        // with an ACE_HEADER.
        let header = unsafe { ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        match u32::from(header.AceType) {
            ACCESS_DENIED_ACE_TYPE
            | ACCESS_DENIED_OBJECT_ACE_TYPE
            | ACCESS_DENIED_CALLBACK_ACE_TYPE
            | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE => continue,
            ACCESS_ALLOWED_ACE_TYPE => {}
            other => return Err(format!("its DACL has an entry of unexpected type {other}")),
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: the ACE is an ACCESS_ALLOWED_ACE (checked above): the access mask follows the
        // header and the SID starts at `SidStart`, both inside the ACE.
        let (mask, sid) = unsafe {
            (
                ptr::read_unaligned(&raw const (*allowed).Mask),
                PSID((&raw mut (*allowed).SidStart).cast()),
            )
        };
        if mask & policy.forbidden == 0 || trusted(sid) {
            continue;
        }
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE.0 != 0 && is_creator(sid) {
            continue;
        }
        return Err(format!(
            "its DACL grants {} the access mask {mask:#x}",
            sid_text(sid)
        ));
    }
    Ok(())
}

fn is_sid(sid: PSID, kind: WELL_KNOWN_SID_TYPE) -> bool {
    // SAFETY: `sid` points to a valid SID inside a security descriptor the caller holds.
    unsafe { IsWellKnownSid(sid, kind) }.as_bool()
}

/// SYSTEM or Administrators.
fn is_trusted(sid: PSID) -> bool {
    is_sid(sid, WinLocalSystemSid) || is_sid(sid, WinBuiltinAdministratorsSid)
}

/// CREATOR OWNER or CREATOR GROUP.
fn is_creator(sid: PSID) -> bool {
    is_sid(sid, WinCreatorOwnerSid) || is_sid(sid, WinCreatorGroupSid)
}

/// `S-1-5-…` text of a SID, for messages.
fn sid_text(sid: PSID) -> String {
    sid_to_string(sid).unwrap_or_else(|_| "an unreadable SID".to_string())
}

/// `S-1-5-…` text of a valid SID.
pub(crate) fn sid_to_string(sid: PSID) -> Result<String, Error> {
    let mut text = PWSTR(ptr::null_mut());
    // SAFETY: `sid` is a valid SID; `text` receives a LocalAlloc'd string, freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }
        .map_err(|error| win32("ConvertSidToStringSidW", &error))?;
    // SAFETY: on success `text` is a NUL-terminated string.
    let result = unsafe { text.to_string() };
    // SAFETY: `text` was allocated by ConvertSidToStringSidW with LocalAlloc and is freed once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    result.map_err(|_| Error::UnexpectedData {
        path: "SID text".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal_store::JOURNAL_KEY_SDDL;
    use crate::pipe::HELPER_PIPE_SDDL;
    use crate::protected_dir::{
        BASE_DIR_SDDL, LOCK_FILE_SDDL, PRIVATE_DIR_SDDL, RECOVERY_DIR_SDDL,
    };

    fn check(sddl: &str, policy: AclPolicy) -> Result<(), String> {
        let sd = LocalSd::from_sddl(sddl).map_err(|error| error.to_string())?;
        check_descriptor(sd.as_ptr(), policy)
    }

    #[test]
    fn mklm_descriptors_pass_their_own_checks() {
        assert_eq!(check(BASE_DIR_SDDL, DIRECTORY_POLICY), Ok(()));
        assert_eq!(check(RECOVERY_DIR_SDDL, DIRECTORY_POLICY), Ok(()));
        assert_eq!(check(PRIVATE_DIR_SDDL, DIRECTORY_POLICY), Ok(()));
        assert_eq!(check(LOCK_FILE_SDDL, LOCK_FILE_POLICY), Ok(()));
        assert_eq!(check(JOURNAL_KEY_SDDL, KEY_POLICY), Ok(()));
    }

    #[test]
    fn users_may_read_the_directories_but_not_the_lock_file() {
        // RECOVERY_DIR_SDDL grants Users read; as a lock file that would allow a DoS.
        assert!(check(RECOVERY_DIR_SDDL, LOCK_FILE_POLICY).is_err());
        assert!(check("O:BAD:P(A;;FA;;;BA)(A;;FR;;;BU)", LOCK_FILE_POLICY).is_err());
    }

    #[test]
    fn owner_must_be_system_or_administrators() {
        assert_eq!(check("O:SYD:P(A;;FA;;;SY)", DIRECTORY_POLICY), Ok(()));
        let error = check("O:BUD:P(A;;FA;;;BA)", DIRECTORY_POLICY).unwrap_err();
        assert!(error.contains("S-1-5-32-545"), "{error}");
        assert!(check("O:WDD:P(A;;FA;;;BA)", DIRECTORY_POLICY).is_err());
        assert!(check("D:P(A;;FA;;;BA)", DIRECTORY_POLICY).is_err());
    }

    #[test]
    fn directories_need_a_protected_dacl_keys_do_not() {
        assert!(check("O:BAD:(A;;FA;;;BA)", DIRECTORY_POLICY).is_err());
        assert_eq!(check("O:BAD:(A;;KA;;;BA)(A;;KR;;;BU)", KEY_POLICY), Ok(()));
    }

    #[test]
    fn a_null_dacl_fails() {
        assert!(check("O:BAD:NO_ACCESS_CONTROL", DIRECTORY_POLICY).is_err());
        assert!(check("O:BA", KEY_POLICY).is_err());
    }

    #[test]
    fn write_rights_for_anyone_else_fail() {
        for sddl in [
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;BU)",
            "O:BAD:P(A;;FA;;;BA)(A;;GW;;;WD)",
            "O:BAD:P(A;;FA;;;BA)(A;;GA;;;AU)",
            // FILE_ADD_SUBDIRECTORY for Users, as %ProgramData% itself grants.
            "O:BAD:P(A;;FA;;;BA)(A;CI;0x4;;;BU)",
            // WRITE_DAC alone.
            "O:BAD:P(A;;FA;;;BA)(A;;WD;;;BU)",
            // An inherit-only ACE still reaches every file created later.
            "O:BAD:P(A;;FA;;;BA)(A;OICIIO;FA;;;BU)",
        ] {
            assert!(check(sddl, DIRECTORY_POLICY).is_err(), "{sddl}");
        }
        assert!(check("O:BAD:(A;;KA;;;BA)(A;;KW;;;BU)", KEY_POLICY).is_err());
        assert!(check("O:BAD:(A;;KA;;;BA)(A;;0x2;;;IU)", KEY_POLICY).is_err());
    }

    #[test]
    fn deny_entries_and_creator_templates_are_fine() {
        assert_eq!(
            check("O:BAD:P(D;;FA;;;BU)(A;;FA;;;BA)", DIRECTORY_POLICY),
            Ok(())
        );
        assert_eq!(
            check("O:BAD:P(A;;FA;;;BA)(A;OICIIO;GA;;;CO)", DIRECTORY_POLICY),
            Ok(())
        );
        // CREATOR OWNER that applies to the object itself is not a template.
        assert!(check("O:BAD:P(A;;FA;;;BA)(A;;FA;;;CO)", DIRECTORY_POLICY).is_err());
    }

    /// `%ProgramData%` as Windows sets it up passes; whatever would let someone else rename a
    /// validated `SHIN DATA CENTER` (or `%ProgramData%` itself) does not (design m5b D.9.4; M5b
    /// security review, finding 1).
    #[test]
    fn program_data_as_windows_sets_it_up() {
        // C:\ProgramData on Windows 11 (Get-Acl): Users may add files and folders (0x116).
        const WINDOWS: &str = "O:SYG:SYD:PAI(A;OICIIO;GA;;;CO)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)\
                               (A;OICI;0x1200a9;;;BU)(A;CI;DCLCRPCR;;;BU)";
        assert_eq!(check(WINDOWS, PROGRAM_DATA_POLICY), Ok(()));
        // The MKLM folders are stricter, on purpose.
        assert!(check(WINDOWS, DIRECTORY_POLICY).is_err());
        // An unprotected DACL is fine for it; TrustedInstaller may own it, but not MKLM's folders.
        assert_eq!(
            check("O:BAD:(A;;FA;;;BA)(A;CI;0x116;;;BU)", PROGRAM_DATA_POLICY),
            Ok(())
        );
        let installer = format!("O:{TRUSTED_INSTALLER_SID}D:(A;;FA;;;{TRUSTED_INSTALLER_SID})");
        assert_eq!(check(&installer, PROGRAM_DATA_POLICY), Ok(()));
        assert!(check(&format!("{installer}(A;;FA;;;SY)"), KEY_POLICY).is_err());
        for sddl in [
            // Owned by a user, as a folder the user made for a forged %SystemDrive% is.
            "O:BUD:(A;;FA;;;BA)",
            "O:SYD:(A;;FA;;;SY)(A;;FA;;;BU)",
            // FILE_DELETE_CHILD: renames SHIN DATA CENTER away.
            "O:SYD:(A;;FA;;;SY)(A;CI;0x40;;;BU)",
            // DELETE: renames %ProgramData% itself.
            "O:SYD:(A;;FA;;;SY)(A;;SD;;;AU)",
            "O:SYD:(A;;FA;;;SY)(A;;WD;;;WD)",
            "O:SYD:(A;;FA;;;SY)(A;;WO;;;BU)",
            "O:SYD:(A;;FA;;;SY)(A;;GA;;;BU)",
            "O:SYD:NO_ACCESS_CONTROL",
        ] {
            assert!(check(sddl, PROGRAM_DATA_POLICY).is_err(), "{sddl}");
        }
    }

    #[test]
    fn unknown_allow_entries_fail() {
        // An object ACE (type 5).
        assert!(
            check(
                "O:BAD:P(A;;FA;;;BA)(OA;;RP;bf967aba-0de6-11d0-a285-00aa003049e2;;BU)",
                DIRECTORY_POLICY
            )
            .is_err()
        );
    }

    #[test]
    fn helper_pipe_sddl_parses() {
        assert!(LocalSd::from_sddl(HELPER_PIPE_SDDL).is_ok());
        assert!(LocalSd::from_sddl("not sddl").is_err());
    }
}
