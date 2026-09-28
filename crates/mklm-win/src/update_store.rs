//! `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` (design m5b D.6): the machine's update records
//! `Trust`, `Run` and `LastResult` (REG_SZ, JSON). Anyone reads them; only the helper creates and
//! writes the key, with the journal's DACL and checks, and only the three values of
//! [`UPDATE_VALUE_NAMES`].
//!
//! Like the journal: the key and every level above it from `SHIN DATA CENTER` down are created
//! with `JOURNAL_KEY_SDDL` and their owner and DACL are checked before anything is written; every
//! write is one `RegSetValueExW` (atomic, and no reader can hold it up, unlike a file) followed by
//! `RegFlushKey`. Standard users cannot create keys under `HKLM\SOFTWARE`, so the key cannot be
//! squatted.

use windows::Win32::System::Registry::{KEY_READ, KEY_SET_VALUE, REG_SZ};
use windows_registry::Key;

use crate::error::Error;
use crate::journal_store::{check_key, open_existing, open_or_create_product_subkey, raw_key};
use crate::regraw;

pub const UPDATE_KEY: &str = r"SOFTWARE\SHIN DATA CENTER\MKLM\Update";
pub const TRUST_VALUE: &str = "Trust";
pub const RUN_VALUE: &str = "Run";
pub const LAST_RESULT_VALUE: &str = "LastResult";
pub const UPDATE_VALUE_NAMES: [&str; 3] = [TRUST_VALUE, RUN_VALUE, LAST_RESULT_VALUE];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawUpdateStore {
    pub trust: Option<String>,
    pub run: Option<String>,
    pub last_result: Option<String>,
}

/// `HKLM\…` label of the key.
fn label() -> String {
    format!(r"HKLM\{UPDATE_KEY}")
}

/// Unelevated read; a missing key is the default.
pub fn read_update_store() -> Result<RawUpdateStore, Error> {
    match open_existing(UPDATE_KEY, KEY_READ)? {
        Some(key) => read_values(&key),
        None => Ok(RawUpdateStore::default()),
    }
}

/// The three values of an open key. A value that is not a clean `REG_SZ` is an error (only the
/// helper writes them, always as `REG_SZ`).
fn read_values(key: &Key) -> Result<RawUpdateStore, Error> {
    let path = label();
    let read =
        |name: &str| -> Result<Option<String>, Error> {
            match regraw::query_value(raw_key(key), &path, name)? {
                None => Ok(None),
                Some(raw) if raw.ty == REG_SZ.0 => regraw::decode_sz(&raw.data)
                    .map(Some)
                    .ok_or_else(|| Error::UnexpectedData {
                        path: format!(r"{path}\{name}"),
                    }),
                Some(_) => Err(Error::UnexpectedData {
                    path: format!(r"{path}\{name}"),
                }),
            }
        };
    Ok(RawUpdateStore {
        trust: read(TRUST_VALUE)?,
        run: read(RUN_VALUE)?,
        last_result: read(LAST_RESULT_VALUE)?,
    })
}

/// Only the names of [`UPDATE_VALUE_NAMES`] may be written or deleted, in their exact spelling
/// (the same kind of gate as `regwrite`'s, design m2 S9). Checked before any key is opened.
fn check_value_name(name: &str) -> Result<(), Error> {
    if UPDATE_VALUE_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(Error::ValueNotAllowed {
            path: label(),
            name: name.to_string(),
        })
    }
}

/// Helper only: the key with `JOURNAL_KEY_SDDL`, owner and DACL of every level verified.
#[derive(Debug)]
pub struct UpdateStore {
    key: Key,
}

impl UpdateStore {
    /// Creates `SHIN DATA CENTER`, `MKLM` and `Update` when missing (with `JOURNAL_KEY_SDDL`) and
    /// checks the owner and DACL of each level (`Error::Insecure`).
    pub fn open_or_create() -> Result<UpdateStore, Error> {
        let key = open_or_create_product_subkey("Update", UPDATE_KEY, KEY_READ | KEY_SET_VALUE)?;
        Ok(UpdateStore { key })
    }

    pub fn read(&self) -> Result<RawUpdateStore, Error> {
        read_values(&self.key)
    }

    /// REG_SZ, then `RegFlushKey`. `name` must be in `UPDATE_VALUE_NAMES` (`ValueNotAllowed`).
    /// The key's owner and DACL are checked again first.
    pub fn write(&self, name: &str, json: &str) -> Result<(), Error> {
        check_value_name(name)?;
        let path = label();
        let data = regraw::encode_sz(json).ok_or_else(|| Error::UnexpectedData {
            path: format!(r"{path}\{name}"),
        })?;
        check_key(&self.key, UPDATE_KEY)?;
        regraw::set_value(raw_key(&self.key), &path, name, REG_SZ.0, &data)?;
        regraw::flush(raw_key(&self.key), &path)
    }

    /// Absent is success; flushed.
    pub fn delete(&self, name: &str) -> Result<(), Error> {
        check_value_name(name)?;
        let path = label();
        check_key(&self.key, UPDATE_KEY)?;
        regraw::delete_value(raw_key(&self.key), &path, name)?;
        regraw::flush(raw_key(&self.key), &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_value_names() {
        assert_eq!(UPDATE_VALUE_NAMES, ["Trust", "Run", "LastResult"]);
        assert_eq!(UPDATE_KEY, r"SOFTWARE\SHIN DATA CENTER\MKLM\Update");
    }

    /// Design m5b F.2: the gate refuses before any key is opened (writing needs elevation and
    /// happens on the machine only).
    #[test]
    fn only_the_three_values_pass_the_gate() {
        for name in UPDATE_VALUE_NAMES {
            assert_eq!(check_value_name(name), Ok(()), "{name}");
        }
        for name in [
            "",
            "trust",
            "RUN",
            "lastresult",
            "Last Result",
            "Trust\0",
            "Start",
            "RestoreOnUninstall",
            r"..\Journal",
            "Run ",
        ] {
            assert_eq!(
                check_value_name(name),
                Err(Error::ValueNotAllowed {
                    path: r"HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update".to_string(),
                    name: name.to_string()
                }),
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_store_reads_unelevated() {
        // Read-only. On a machine without MKLM's key this is simply empty.
        let store = read_update_store();
        assert!(store.is_ok(), "{store:?}");
    }
}
