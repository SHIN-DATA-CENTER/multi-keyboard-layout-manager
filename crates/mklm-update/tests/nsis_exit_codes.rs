//! The installer's exit codes (design m5b D.9.1): every `!define MKLM_EXIT_*` in
//! installer/nsis/mklm.nsi has the value of the matching `mklm_update::run::nsis_exit` constant,
//! and every constant is defined there. `MKLM_EXIT_BAD_INSTALL_DIR` (25) is the uninstaller's
//! only: the script marks it so, and it is not a constant (`InstallerExit::Other(25)`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use mklm_update::run::{InstallerExit, classify_installer_exit, nsis_exit};

fn script() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("installer")
        .join("nsis")
        .join("mklm.nsi");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `MKLM_EXIT_<NAME>` → (value, line number, whether the comment block right above marks it as
/// the uninstaller's only).
fn defines(text: &str) -> BTreeMap<String, (u32, usize, bool)> {
    let mut found = BTreeMap::new();
    let mut comment_block: Vec<String> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix(';') {
            comment_block.push(comment.trim().to_string());
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() >= 3
            && words[0] == "!define"
            && let Some(name) = words[1].strip_prefix("MKLM_EXIT_")
        {
            let value: u32 = words[2]
                .parse()
                .unwrap_or_else(|_| panic!("line {}: {line}", index + 1));
            let uninstaller_only = comment_block
                .iter()
                .any(|comment| comment.starts_with("uninstaller-only"));
            let earlier = found.insert(name.to_string(), (value, index + 1, uninstaller_only));
            assert!(earlier.is_none(), "MKLM_EXIT_{name} is defined twice");
        }
        comment_block.clear();
    }
    found
}

#[test]
fn the_script_and_the_constants_agree() {
    let expected = [
        ("OS_TOO_OLD", nsis_exit::OS_TOO_OLD),
        ("WRONG_ARCH", nsis_exit::WRONG_ARCH),
        ("HELPER_RUNNING", nsis_exit::HELPER_RUNNING),
        ("CLI_RUNNING", nsis_exit::CLI_RUNNING),
        ("GUI_RUNNING", nsis_exit::GUI_RUNNING),
        ("FILES_IN_USE", nsis_exit::FILES_IN_USE),
        ("FILE_WRITE", nsis_exit::FILE_WRITE),
    ];
    let mut defined = defines(&script());
    for (name, value) in expected {
        let Some((found, line, uninstaller_only)) = defined.remove(name) else {
            panic!("mklm.nsi does not define MKLM_EXIT_{name}");
        };
        assert_eq!(found, value, "MKLM_EXIT_{name} (line {line})");
        assert!(!uninstaller_only, "MKLM_EXIT_{name} is the installer's");
        // Every installer code means "nothing was replaced" (D.13).
        assert!(classify_installer_exit(value).leaves_old_files(), "{name}");
    }
    // What is left must be the uninstaller's 25, marked as such.
    let Some((value, line, uninstaller_only)) = defined.remove("BAD_INSTALL_DIR") else {
        panic!("mklm.nsi does not define MKLM_EXIT_BAD_INSTALL_DIR");
    };
    assert_eq!(value, 25, "line {line}");
    assert!(
        uninstaller_only,
        "line {line}: MKLM_EXIT_BAD_INSTALL_DIR needs its `; uninstaller-only` comment"
    );
    assert_eq!(classify_installer_exit(25), InstallerExit::Other(25));
    assert!(
        defined.is_empty(),
        "mklm.nsi defines exit codes the updater does not know: {defined:?}"
    );
}

/// Every code the script uses is one of its defines (no bare numbers), except the uninstaller's
/// 3010.
#[test]
fn the_script_uses_its_defines() {
    for (index, line) in script().lines().enumerate() {
        let code = line.split(';').next().unwrap_or_default().trim();
        if let Some(value) = code.strip_prefix("SetErrorLevel ") {
            let value = value.trim();
            assert!(
                value == "3010" || (value.starts_with("${MKLM_EXIT_") && value.ends_with('}')),
                "line {}: {code}",
                index + 1
            );
        }
    }
}
