//! The Japanese catalogue covers every static label (design m3 D.2, H.1): each `@tr("…")` of
//! `ui/*.slint` has a non-empty `msgstr` in `translations/ja/LC_MESSAGES/mklm.po`, and the
//! catalogue has no entry the UI no longer uses. Plain-text parsing, no window.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The msgids of every `@tr("…")` in the .slint files under `dir` (escapes resolved).
fn slint_msgids(dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            slint_msgids(&path, out);
        } else if path.extension().is_some_and(|e| e == "slint") {
            let text = fs::read_to_string(&path).unwrap();
            let mut rest = text.as_str();
            while let Some(start) = rest.find("@tr(\"") {
                rest = &rest[start + 5..];
                let mut id = String::new();
                let mut chars = rest.char_indices();
                let end = loop {
                    match chars.next() {
                        Some((_, '\\')) => {
                            if let Some((_, escaped)) = chars.next() {
                                id.push(escaped);
                            }
                        }
                        Some((index, '"')) => break index,
                        Some((_, c)) => id.push(c),
                        None => panic!("unterminated @tr in {}", path.display()),
                    }
                };
                rest = &rest[end + 1..];
                out.push(id);
            }
        }
    }
}

/// A .po string literal's content with escapes resolved.
fn po_string(line: &str) -> String {
    let inner = line
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or_else(|| panic!("not a .po string: {line}"));
    let mut text = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => text.push('\n'),
                Some(other) => text.push(other),
                None => {}
            }
        } else {
            text.push(c);
        }
    }
    text
}

/// msgid → msgstr of a .po file (single-line and continued strings; no plurals or contexts).
fn po_entries(path: &Path) -> BTreeMap<String, String> {
    let text = fs::read_to_string(path).unwrap();
    let mut entries = BTreeMap::new();
    let mut msgid: Option<String> = None;
    let mut msgstr: Option<String> = None;
    let mut flush = |msgid: &mut Option<String>, msgstr: &mut Option<String>| {
        if let (Some(id), Some(value)) = (msgid.take(), msgstr.take())
            && !id.is_empty()
        {
            assert!(
                entries.insert(id.clone(), value).is_none(),
                "duplicate msgid {id:?}"
            );
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            flush(&mut msgid, &mut msgstr);
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut msgid, &mut msgstr);
            msgid = Some(po_string(rest));
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            msgstr = Some(po_string(rest));
        } else if line.starts_with('"') {
            let target = if msgstr.is_some() {
                &mut msgstr
            } else {
                &mut msgid
            };
            if let Some(target) = target {
                target.push_str(&po_string(line));
            }
        }
    }
    flush(&mut msgid, &mut msgstr);
    entries
}

#[test]
fn the_japanese_catalogue_is_complete() {
    let mut ids = Vec::new();
    slint_msgids(&manifest_dir().join("ui"), &mut ids);
    ids.sort();
    ids.dedup();
    assert!(!ids.is_empty());
    let entries = po_entries(&manifest_dir().join("translations/ja/LC_MESSAGES/mklm.po"));
    let missing: Vec<&String> = ids
        .iter()
        .filter(|id| entries.get(*id).is_none_or(String::is_empty))
        .collect();
    assert!(missing.is_empty(), "untranslated: {missing:#?}");
    let unused: Vec<&String> = entries.keys().filter(|id| !ids.contains(id)).collect();
    assert!(unused.is_empty(), "no longer used: {unused:#?}");
    // Placeholders survive the translation.
    for id in &ids {
        let placeholders = id.matches("{}").count();
        assert_eq!(
            entries[id].matches("{}").count(),
            placeholders,
            "placeholders of {id:?}"
        );
    }
}
