//! The journal store under `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal` (M2, design section C.1).
//!
//! Reading works unelevated (the GUI and CLI decide from it whether recovery is needed). Writing
//! creates the keys with [`JOURNAL_KEY_SDDL`] and, before every use, checks that each existing key
//! from `SHIN DATA CENTER` down is owned by Administrators or SYSTEM and grants write access to
//! nobody else.
//!
//! Every key is opened in the 64-bit registry view, whatever the bitness of the process.

use mklm_core::{
    JOURNAL_BASELINES_KEY, JOURNAL_KEY, JOURNAL_OPS_KEY, STORE_VERSION, STORE_VERSION_VALUE,
};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_CREATE_SUB_KEY, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, RegCreateKeyExW, RegOpenKeyExW,
};
use windows::core::PCWSTR;
use windows_registry::Key;

use crate::error::Error;
use crate::props::to_wide;
use crate::regraw::{self, RawValue};
use crate::security::{KEY_POLICY, LocalSd, check_descriptor, key_security};

/// Security of `SHIN DATA CENTER`, `MKLM`, `Journal`, `Ops` and `Baselines`: owner Administrators,
/// protected DACL, full control for SYSTEM and Administrators, read for Users.
pub const JOURNAL_KEY_SDDL: &str = "O:BAG:SYD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KR;;;BU)";

/// `SHIN DATA CENTER`, relative to `HKEY_LOCAL_MACHINE`.
const VENDOR_KEY: &str = r"SOFTWARE\SHIN DATA CENTER";
/// `MKLM` (`mklm_core::MKLM_KEY`), relative to `HKEY_LOCAL_MACHINE`.
const PRODUCT_KEY: &str = mklm_core::MKLM_KEY;

/// Which sub-key a record lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JournalSubkey {
    Ops,
    Baselines,
}

/// The raw store, as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawJournal {
    pub store_version: Option<u32>,
    /// `(value name, JSON)` of `Ops`.
    pub ops: Vec<(String, String)>,
    /// `(value name, JSON)` of `Baselines`.
    pub baselines: Vec<(String, String)>,
}

/// `HKLM\…` label of a path relative to `HKEY_LOCAL_MACHINE`.
fn label(path: &str) -> String {
    format!(r"HKLM\{path}")
}

fn hkey(key: &Key) -> HKEY {
    HKEY(key.as_raw())
}

/// Opens an existing key under `HKEY_LOCAL_MACHINE` in the 64-bit view; `Ok(None)` when missing.
fn open_existing(path: &str, access: REG_SAM_FLAGS) -> Result<Option<Key>, Error> {
    let wide = to_wide(path);
    let mut key = HKEY::default();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `key` is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide.as_ptr()),
            None,
            access | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    match status {
        // SAFETY: on success `key` is an open key this process owns; `Key` closes it on drop.
        ERROR_SUCCESS => Ok(Some(unsafe { Key::from_raw(key.0) })),
        ERROR_FILE_NOT_FOUND => Ok(None),
        other => Err(Error::Registry {
            path: label(path),
            code: other.0,
        }),
    }
}

/// Reads the store with `KEY_READ`; an empty [`RawJournal`] when it does not exist. A value that
/// is not `REG_SZ` is an error (the engine then treats the journal as unreadable).
pub fn read_journal_store() -> Result<RawJournal, Error> {
    let Some(journal) = open_existing(JOURNAL_KEY, KEY_READ)? else {
        return Ok(RawJournal::default());
    };
    let store_version = read_store_version(&journal)?;
    Ok(RawJournal {
        store_version,
        ops: read_records(JOURNAL_OPS_KEY)?,
        baselines: read_records(JOURNAL_BASELINES_KEY)?,
    })
}

/// `StoreVersion`, which must be a `REG_DWORD` when present.
fn read_store_version(journal: &Key) -> Result<Option<u32>, Error> {
    let path = label(JOURNAL_KEY);
    match regraw::query_value(hkey(journal), &path, STORE_VERSION_VALUE)? {
        None => Ok(None),
        Some(RawValue { ty, data }) if ty == REG_DWORD.0 && data.len() == 4 => {
            Ok(Some(u32::from_le_bytes([
                data[0], data[1], data[2], data[3],
            ])))
        }
        Some(_) => Err(Error::UnexpectedData {
            path: format!(r"{path}\{STORE_VERSION_VALUE}"),
        }),
    }
}

/// `(value name, text)` of every value of `Ops` or `Baselines`; empty when the key is missing.
fn read_records(path: &str) -> Result<Vec<(String, String)>, Error> {
    let Some(key) = open_existing(path, KEY_READ)? else {
        return Ok(Vec::new());
    };
    let path = label(path);
    regraw::enum_values(hkey(&key), &path)?
        .into_iter()
        .map(|(name, raw)| {
            let text = (raw.ty == REG_SZ.0)
                .then(|| regraw::decode_sz(&raw.data))
                .flatten();
            text.map(|text| (name.clone(), text))
                .ok_or_else(|| Error::UnexpectedData {
                    path: format!(r"{path}\{name}"),
                })
        })
        .collect()
}

/// The store opened for writing.
#[derive(Debug)]
pub struct JournalStore {
    /// `SHIN DATA CENTER` and `MKLM`: not written, kept open for the security checks.
    vendor: Key,
    product: Key,
    journal: Key,
    ops: Key,
    baselines: Key,
}

impl JournalStore {
    /// Creates missing keys with [`JOURNAL_KEY_SDDL`], validates existing ones, and sets
    /// `StoreVersion` when missing. Fails with [`Error::Insecure`] on a bad owner or DACL.
    ///
    /// An existing `StoreVersion` other than `mklm_core::STORE_VERSION` fails with
    /// [`Error::UnexpectedData`]: this build never writes into a store layout it does not know.
    pub fn open_or_create() -> Result<Self, Error> {
        let sd = LocalSd::from_sddl(JOURNAL_KEY_SDDL)?;
        let parent_access = KEY_READ | KEY_CREATE_SUB_KEY;
        // Each level is checked before anything is created under it.
        let vendor = create_key(
            HKEY_LOCAL_MACHINE,
            VENDOR_KEY,
            VENDOR_KEY,
            parent_access,
            &sd,
        )?;
        check_key(&vendor, VENDOR_KEY)?;
        let product = create_key(hkey(&vendor), "MKLM", PRODUCT_KEY, parent_access, &sd)?;
        check_key(&product, PRODUCT_KEY)?;
        let journal = create_key(
            hkey(&product),
            "Journal",
            JOURNAL_KEY,
            parent_access | KEY_SET_VALUE,
            &sd,
        )?;
        check_key(&journal, JOURNAL_KEY)?;
        let ops = create_key(
            hkey(&journal),
            "Ops",
            JOURNAL_OPS_KEY,
            KEY_READ | KEY_SET_VALUE,
            &sd,
        )?;
        let baselines = create_key(
            hkey(&journal),
            "Baselines",
            JOURNAL_BASELINES_KEY,
            KEY_READ | KEY_SET_VALUE,
            &sd,
        )?;
        let store = Self {
            vendor,
            product,
            journal,
            ops,
            baselines,
        };
        store.validate()?;
        store.ensure_store_version()?;
        Ok(store)
    }

    /// Owner and DACL of every key from `SHIN DATA CENTER` down (design C.1).
    fn validate(&self) -> Result<(), Error> {
        check_key(&self.vendor, VENDOR_KEY)?;
        check_key(&self.product, PRODUCT_KEY)?;
        check_key(&self.journal, JOURNAL_KEY)?;
        check_key(&self.ops, JOURNAL_OPS_KEY)?;
        check_key(&self.baselines, JOURNAL_BASELINES_KEY)
    }

    fn ensure_store_version(&self) -> Result<(), Error> {
        match read_store_version(&self.journal)? {
            None => regraw::set_value(
                hkey(&self.journal),
                &label(JOURNAL_KEY),
                STORE_VERSION_VALUE,
                REG_DWORD.0,
                &STORE_VERSION.to_le_bytes(),
            ),
            Some(STORE_VERSION) => Ok(()),
            Some(_) => Err(Error::UnexpectedData {
                path: format!(r"{}\{STORE_VERSION_VALUE}", label(JOURNAL_KEY)),
            }),
        }
    }

    fn subkey(&self, subkey: JournalSubkey) -> (&Key, &'static str) {
        match subkey {
            JournalSubkey::Ops => (&self.ops, JOURNAL_OPS_KEY),
            JournalSubkey::Baselines => (&self.baselines, JOURNAL_BASELINES_KEY),
        }
    }

    /// One `RegSetValueExW` (`REG_SZ`).
    pub fn write(&self, subkey: JournalSubkey, name: &str, json: &str) -> Result<(), Error> {
        let (key, path) = self.subkey(subkey);
        let path = label(path);
        check_record_name(&path, name)?;
        let data = regraw::encode_sz(json).ok_or_else(|| Error::UnexpectedData {
            path: format!(r"{path}\{name}"),
        })?;
        self.validate()?;
        regraw::set_value(hkey(key), &path, name, REG_SZ.0, &data)
    }

    /// `RegDeleteValueW`; a missing value is fine.
    pub fn delete(&self, subkey: JournalSubkey, name: &str) -> Result<(), Error> {
        let (key, path) = self.subkey(subkey);
        let path = label(path);
        check_record_name(&path, name)?;
        self.validate()?;
        regraw::delete_value(hkey(key), &path, name)
    }

    /// `RegFlushKey` (SOFTWARE hive).
    pub fn flush(&self) -> Result<(), Error> {
        regraw::flush(hkey(&self.journal), &label(JOURNAL_KEY))
    }
}

/// Owner and DACL of one journal key; `path` is relative to `HKEY_LOCAL_MACHINE`.
fn check_key(key: &Key, path: &str) -> Result<(), Error> {
    let mut sd = key_security(hkey(key))?;
    check_descriptor(sd.as_ptr(), KEY_POLICY).map_err(|reason| Error::Insecure {
        path: label(path),
        reason,
    })
}

/// Record names are operation IDs and canonical value keys: never empty (that would be the key's
/// default value) and never with a NUL.
fn check_record_name(path: &str, name: &str) -> Result<(), Error> {
    if name.is_empty() || name.contains('\0') {
        Err(Error::UnexpectedData {
            path: format!(r"{path}\{name}"),
        })
    } else {
        Ok(())
    }
}

/// `RegCreateKeyExW(parent, name)` with `sd` for a key that does not exist yet (an existing key
/// keeps its security; [`JournalStore::validate`] checks it). `path` labels errors.
fn create_key(
    parent: HKEY,
    name: &str,
    path: &str,
    access: REG_SAM_FLAGS,
    sd: &LocalSd,
) -> Result<Key, Error> {
    let wide = to_wide(name);
    let attributes = sd.attributes();
    let mut key = HKEY::default();
    // SAFETY: `wide` is NUL-terminated; `attributes` points at `sd`, which outlives the call;
    // `key` is a valid out pointer.
    let status = unsafe {
        RegCreateKeyExW(
            parent,
            PCWSTR(wide.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            access | KEY_WOW64_64KEY,
            Some(&attributes),
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(Error::Registry {
            path: label(path),
            code: status.0,
        });
    }
    // SAFETY: on success `key` is an open key this process owns; `Key` closes it on drop.
    Ok(unsafe { Key::from_raw(key.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_reads_unelevated() {
        // Read-only. On a machine without MKLM the store is simply empty.
        let store = read_journal_store();
        assert!(store.is_ok(), "{store:?}");
    }

    #[test]
    fn key_security_is_readable_and_checked() {
        // Read-only: HKLM\SOFTWARE is readable (with READ_CONTROL) by every user. Whatever its
        // owner and DACL, the check answers without failing to read them.
        let key = open_existing("SOFTWARE", KEY_READ)
            .expect("open HKLM\\SOFTWARE")
            .expect("HKLM\\SOFTWARE exists");
        let result = check_key(&key, "SOFTWARE");
        assert!(
            matches!(&result, Ok(()) | Err(Error::Insecure { .. })),
            "{result:?}"
        );
        // It grants CREATOR OWNER and others more than the journal allows, or is owned by
        // TrustedInstaller: never a valid journal key.
        assert!(result.is_err());
    }

    #[test]
    fn record_names_are_checked() {
        assert!(check_record_name("p", "").is_err());
        assert!(check_record_name("p", "a\0b").is_err());
        assert_eq!(
            check_record_name("p", "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"),
            Ok(())
        );
        assert_eq!(check_record_name("p", r"device|HID\VID_1|Name"), Ok(()));
    }
}
