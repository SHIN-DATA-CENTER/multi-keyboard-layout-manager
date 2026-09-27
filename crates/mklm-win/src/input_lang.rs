//! Input methods: `Preload` lists of the current user and the sign-in screen, and loaded layouts.

use mklm_core::InputMethods;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutList, HKL};
use windows_registry::{CURRENT_USER, Key, USERS};

use crate::error::{Error, ReadIssue, ReadIssueKind};
use crate::reg::{open_read, string_value};

/// Current user's list, relative to `HKEY_CURRENT_USER`.
pub const USER_PRELOAD: &str = r"Keyboard Layout\Preload";
/// Sign-in screen's list, relative to `HKEY_USERS`.
pub const SIGN_IN_PRELOAD: &str = r".DEFAULT\Keyboard Layout\Preload";
/// Current user's Preload substitutes (e.g. `d0010411` → `00000409`), relative to `HKEY_CURRENT_USER`.
pub const USER_SUBSTITUTES: &str = r"Keyboard Layout\Substitutes";
/// Sign-in screen's Preload substitutes, relative to `HKEY_USERS`.
pub const SIGN_IN_SUBSTITUTES: &str = r".DEFAULT\Keyboard Layout\Substitutes";

/// Reads both `Preload` lists, with substitutes resolved to the KLIDs they stand for, and the
/// layouts loaded in the calling session. Anything unreadable is left empty, with an issue.
pub fn read_input_methods(issues: &mut Vec<ReadIssue>) -> InputMethods {
    InputMethods {
        user_preload: read_preload(CURRENT_USER, "HKCU", USER_PRELOAD, USER_SUBSTITUTES, issues),
        sign_in_preload: read_preload(USERS, "HKU", SIGN_IN_PRELOAD, SIGN_IN_SUBSTITUTES, issues),
        loaded_layouts: loaded_layouts(),
    }
}

/// KLIDs of a `Preload` key in list order (value names `1`, `2`, ... sorted numerically), each
/// replaced by its entry in the `substitutes` key when it has one.
fn read_preload(
    root: &Key,
    root_name: &str,
    path: &str,
    substitutes: &str,
    issues: &mut Vec<ReadIssue>,
) -> Vec<String> {
    let preload = order_preload(read_string_values(root, root_name, path, issues));
    if preload.is_empty() {
        return preload;
    }
    resolve_substitutes(
        preload,
        &read_string_values(root, root_name, substitutes, issues),
    )
}

/// Every string value of `root\path` as (name, data). Empty when the key does not exist; an
/// unreadable key is empty with an issue.
fn read_string_values(
    root: &Key,
    root_name: &str,
    path: &str,
    issues: &mut Vec<ReadIssue>,
) -> Vec<(String, String)> {
    let key = match open_read(root, root_name, path) {
        Ok(Some(key)) => key,
        Ok(None) => return Vec::new(),
        Err(error) => {
            issues.push(ReadIssue::new(
                ReadIssueKind::Environment,
                format!(r"{root_name}\{path}"),
                error,
            ));
            return Vec::new();
        }
    };
    match key.values() {
        Ok(values) => values
            .filter_map(|(name, value)| string_value(&value).map(|data| (name, data)))
            .collect(),
        Err(error) => {
            let full = format!(r"{root_name}\{path}");
            issues.push(ReadIssue::new(
                ReadIssueKind::Environment,
                full.clone(),
                Error::registry(full, &error),
            ));
            Vec::new()
        }
    }
}

/// Replaces every Preload entry that names a substitute (compared case-insensitively) with the KLID
/// it stands for. Entries without a substitute stay as they are.
pub(crate) fn resolve_substitutes(
    preload: Vec<String>,
    substitutes: &[(String, String)],
) -> Vec<String> {
    preload
        .into_iter()
        .map(|klid| {
            substitutes
                .iter()
                .find(|(name, _)| name.trim().eq_ignore_ascii_case(&klid))
                .map_or(klid, |(_, target)| target.trim().to_string())
        })
        .collect()
}

/// Keeps the numerically named entries and orders them by number.
pub(crate) fn order_preload(entries: Vec<(String, String)>) -> Vec<String> {
    let mut numbered: Vec<(u32, String)> = entries
        .into_iter()
        .filter_map(|(name, klid)| {
            let name = name.trim();
            let index = (!name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
                .then(|| name.parse::<u32>().ok())
                .flatten()?;
            Some((index, klid.trim().to_string()))
        })
        .collect();
    numbered.sort_by_key(|(index, _)| *index);
    numbered.into_iter().map(|(_, klid)| klid).collect()
}

/// Layouts loaded in the calling session (`GetKeyboardLayoutList`), as the low 32 bits of each HKL.
pub fn loaded_layouts() -> Vec<u32> {
    // SAFETY: without a buffer the call only returns the number of layouts.
    let count = unsafe { GetKeyboardLayoutList(None) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    if count == 0 {
        return Vec::new();
    }
    let mut list = vec![HKL(std::ptr::null_mut()); count];
    // SAFETY: the slice length tells the API how many HKLs it may write.
    let written = unsafe { GetKeyboardLayoutList(Some(&mut list)) };
    list.truncate(usize::try_from(written).unwrap_or(0));
    // An HKL is a handle-sized value; the layout identity lives in its low 32 bits.
    list.iter().map(|hkl| hkl.0 as usize as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, klid: &str) -> (String, String) {
        (name.to_string(), klid.to_string())
    }

    #[test]
    fn preload_sorted_numerically() {
        let entries = vec![
            entry("10", "00000407"),
            entry("2", "00000409"),
            entry("1", "00000411"),
            entry("", "E0200411"),
            entry("Default", "00000411"),
            entry("x1", "00000804"),
        ];
        assert_eq!(
            order_preload(entries),
            vec!["00000411", "00000409", "00000407"]
        );
        assert!(order_preload(Vec::new()).is_empty());
    }

    #[test]
    fn substitutes_are_resolved() {
        let substitutes = vec![
            entry("d0010411", "00000409"),
            entry("D0010409", " 00000411 "),
        ];
        assert_eq!(
            resolve_substitutes(
                vec![
                    "00000411".into(),
                    "D0010411".into(),
                    "d0010409".into(),
                    "d0020411".into()
                ],
                &substitutes
            ),
            vec!["00000411", "00000409", "00000411", "d0020411"]
        );
        assert!(resolve_substitutes(Vec::new(), &substitutes).is_empty());
    }
}
