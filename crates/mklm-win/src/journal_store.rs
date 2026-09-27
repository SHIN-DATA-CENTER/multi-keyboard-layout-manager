//! The journal store under `HKLM\SOFTWARE\SHIN DATA CENTER\MKLM\Journal` (M2, design section C.1).
//!
//! Reading works unelevated (the GUI and CLI decide from it whether recovery is needed). Writing
//! creates the keys with [`JOURNAL_KEY_SDDL`] and, before every use, checks that each existing key
//! from `SHIN DATA CENTER` down is owned by Administrators or SYSTEM and grants write access to
//! nobody else.

// Skeleton (M2): the bodies below are `todo!()`; remove these allows once they are implemented.
#![allow(unused_variables, dead_code)]

use windows_registry::Key;

use crate::error::Error;

/// Security of `SHIN DATA CENTER`, `MKLM`, `Journal`, `Ops` and `Baselines`: owner Administrators,
/// protected DACL, full control for SYSTEM and Administrators, read for Users.
pub const JOURNAL_KEY_SDDL: &str = "O:BAG:SYD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KR;;;BU)";

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

/// Reads the store with `KEY_READ`; an empty [`RawJournal`] when it does not exist. A value that
/// is not `REG_SZ` is an error (the engine then treats the journal as unreadable).
pub fn read_journal_store() -> Result<RawJournal, Error> {
    todo!("M2")
}

/// The store opened for writing.
#[derive(Debug)]
pub struct JournalStore {
    journal: Key,
    ops: Key,
    baselines: Key,
}

impl JournalStore {
    /// Creates missing keys with [`JOURNAL_KEY_SDDL`], validates existing ones, and sets
    /// `StoreVersion` when missing. Fails with [`Error::Insecure`] on a bad owner or DACL.
    pub fn open_or_create() -> Result<Self, Error> {
        todo!("M2")
    }

    /// One `RegSetValueExW` (`REG_SZ`).
    pub fn write(&self, subkey: JournalSubkey, name: &str, json: &str) -> Result<(), Error> {
        todo!("M2")
    }

    /// `RegDeleteValueW`; a missing value is fine.
    pub fn delete(&self, subkey: JournalSubkey, name: &str) -> Result<(), Error> {
        todo!("M2")
    }

    /// `RegFlushKey` (SOFTWARE hive).
    pub fn flush(&self) -> Result<(), Error> {
        todo!("M2")
    }
}
