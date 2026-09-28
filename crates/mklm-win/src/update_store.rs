//! `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Update` (design m5b D.6): the machine's update records
//! `Trust`, `Run` and `LastResult` (REG_SZ, JSON). Anyone reads them; only the helper creates and
//! writes the key, with the journal's DACL and checks, and only the three values of
//! [`UPDATE_VALUE_NAMES`].
//!
//! WP-0 writes the names; WP-H the registry code.

#![allow(unused_variables, dead_code)] // Skeleton (M5b)

use crate::error::Error;

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

/// Unelevated read; a missing key is the default.
pub fn read_update_store() -> Result<RawUpdateStore, Error> {
    Err(skeleton("read_update_store (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Helper only: the key with `JOURNAL_KEY_SDDL`, owner and DACL of every level verified.
#[derive(Debug)]
pub struct UpdateStore {
    /// Skeleton (M5b): WP-H keeps the key handle here.
    _private: (),
}

impl UpdateStore {
    pub fn open_or_create() -> Result<UpdateStore, Error> {
        Err(skeleton("UpdateStore::open_or_create (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    pub fn read(&self) -> Result<RawUpdateStore, Error> {
        Err(skeleton("UpdateStore::read (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// REG_SZ, then `RegFlushKey`. `name` must be in `UPDATE_VALUE_NAMES` (`ValueNotAllowed`).
    pub fn write(&self, name: &str, json: &str) -> Result<(), Error> {
        Err(skeleton("UpdateStore::write (m5b skeleton)")) // Skeleton (M5b): WP-H
    }

    /// Absent is success; flushed.
    pub fn delete(&self, name: &str) -> Result<(), Error> {
        Err(skeleton("UpdateStore::delete (m5b skeleton)")) // Skeleton (M5b): WP-H
    }
}

/// What the WP-0 skeleton returns (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton(function: &'static str) -> Error {
    Error::Win32 { function, code: 50 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_value_names() {
        assert_eq!(UPDATE_VALUE_NAMES, ["Trust", "Run", "LastResult"]);
        assert_eq!(UPDATE_KEY, r"SOFTWARE\SHIN DATA CENTER\MKLM\Update");
    }
}
