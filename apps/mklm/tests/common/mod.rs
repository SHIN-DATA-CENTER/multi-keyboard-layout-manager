//! Snapshot files for the screen texts (design m3 H.3): `tests/snapshots/<name>`, compared line
//! by line. `MKLM_BLESS=1 cargo test -p mklm` writes the files instead of comparing (review the
//! diff before committing it). No dependency: a plain comparison with the first difference in
//! the message.

use std::fs;
use std::path::PathBuf;

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name)
}

/// Compares `actual` with `tests/snapshots/<name>` (line endings normalized, so a checkout with
/// CRLF compares equal), or writes it when `MKLM_BLESS=1`.
pub fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    if std::env::var_os("MKLM_BLESS").is_some_and(|value| value == "1") {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| {
            panic!(
                "{}: {error}; run with MKLM_BLESS=1 to create it",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    if expected == actual {
        return;
    }
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(want), Some(got)) if want == got => line += 1,
            (None, None) => panic!("{} differs in the final line break", path.display()),
            (want, got) => panic!(
                "{} differs at line {line}:\n  expected: {}\n  actual:   {}\n\
                 (MKLM_BLESS=1 rewrites the file; review the diff)",
                path.display(),
                want.unwrap_or("<end>"),
                got.unwrap_or("<end>")
            ),
        }
    }
}
