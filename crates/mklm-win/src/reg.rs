//! Read-only registry helpers on top of `windows-registry`.

use windows_registry::{Key, Type};

use crate::error::{Error, is_not_found};
use crate::props::from_wide;

/// Opens `root\path` for reading (`KEY_READ`). `Ok(None)` when the key does not exist.
///
/// `root_name` (e.g. `HKLM`) only labels errors.
pub(crate) fn open_read(root: &Key, root_name: &str, path: &str) -> Result<Option<Key>, Error> {
    match root.options().read().open(path) {
        Ok(key) => Ok(Some(key)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(Error::registry(format!(r"{root_name}\{path}"), &error)),
    }
}

/// Reads a `REG_DWORD`. `Ok(None)` when the value does not exist; any other type is an error.
pub(crate) fn read_dword(key: &Key, key_path: &str, name: &str) -> Result<Option<u32>, Error> {
    match key.get_value(name) {
        Ok(value) => match (value.ty(), <[u8; 4]>::try_from(&value[..])) {
            (Type::U32, Ok(bytes)) => Ok(Some(u32::from_le_bytes(bytes))),
            _ => Err(Error::UnexpectedData {
                path: format!(r"{key_path}\{name}"),
            }),
        },
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(Error::registry(format!(r"{key_path}\{name}"), &error)),
    }
}

/// Reads a `REG_SZ` / `REG_EXPAND_SZ` (not expanded). `Ok(None)` when the value does not exist.
pub(crate) fn read_string(key: &Key, key_path: &str, name: &str) -> Result<Option<String>, Error> {
    match key.get_value(name) {
        Ok(value) => string_value(&value)
            .map(Some)
            .ok_or_else(|| Error::UnexpectedData {
                path: format!(r"{key_path}\{name}"),
            }),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(Error::registry(format!(r"{key_path}\{name}"), &error)),
    }
}

/// Text of a string-typed value, up to the first NUL.
pub(crate) fn string_value(value: &windows_registry::Value) -> Option<String> {
    matches!(value.ty(), Type::String | Type::ExpandString).then(|| from_wide(value.as_wide()))
}
