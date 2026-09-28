//! The user's update cache and record: `%LOCALAPPDATA%\SHIN DATA CENTER\MKLM\update\`
//! (design m5b C.4, E.1, D.11, H.5).
//!
//! | File | Content |
//! |---|---|
//! | `state.json` | [`ClientState`]: the user's trust record, the last check, success, failure and ignored rollback |
//! | `latest.json`, `latest.json.minisig` | the last manifest that verified, and the signature that verified it, as received |
//! | `MKLM-Setup-<v>-<arch>.exe` | the verified installer (one is kept) |
//! | `MKLM-Setup-<v>-<arch>.exe.part` | a download under way (removed at start; never resumed) |
//!
//! Every write goes to a temporary file first and is then renamed over the old one (design m3
//! F.4). A missing or unreadable `state.json` reads as nothing recorded: the record is the user's
//! own, and the machine record the helper keeps is the one that matters for installs (C.4).

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use mklm_update::{
    MANIFEST_NAME, MAX_MANIFEST_LEN, MAX_SIGNATURE_LEN, SIGNATURE_NAME, SelectedAsset,
    Sha256Stream, SignatureSlot, TrustState,
};
use serde::{Deserialize, Serialize};

/// The user's record in the cache folder.
pub const STATE_FILE: &str = "state.json";
/// The schema of [`ClientState`].
pub const CLIENT_STATE_SCHEMA: u32 = 1;
/// What a download under way is called: `<installer name>` + this.
pub const PARTIAL_SUFFIX: &str = ".part";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    Transient,
    Structural,
}

/// The last failed check (design m5b E.3; OPS-UX-TEST-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFailure {
    pub class: ErrorClass,
    /// The message ID of design m5b E.6 (e.g. "upd-gh-changed").
    pub message_id: String,
    /// When this run of failures began (Unix seconds).
    pub first_at: u64,
    pub at: u64,
}

/// The last ignored older manifest (design m5b E.3; SECURITY-11, RELIABILITY-6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackNote {
    pub issued_at: u64,
    pub seen: u64,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientState {
    pub schema: u32, // 1
    #[serde(default)]
    pub trust: TrustState,
    #[serde(default)]
    pub last_check: Option<u64>,
    #[serde(default)]
    pub last_success: Option<u64>,
    #[serde(default)]
    pub last_failure: Option<CheckFailure>,
    #[serde(default)]
    pub last_rollback: Option<RollbackNote>,
    /// Which signature the cached `latest.json.minisig` is.
    #[serde(default)]
    pub cached_signature: Option<SignatureSlot>,
}

impl Default for ClientState {
    /// Schema 1, nothing recorded.
    fn default() -> ClientState {
        ClientState {
            schema: CLIENT_STATE_SCHEMA,
            trust: TrustState::default(),
            last_check: None,
            last_success: None,
            last_failure: None,
            last_rollback: None,
            cached_signature: None,
        }
    }
}

impl ClientState {
    /// `state.json`'s text; `None` for another schema or anything unreadable.
    pub fn parse(text: &str) -> Option<ClientState> {
        serde_json::from_str::<ClientState>(text)
            .ok()
            .filter(|state| state.schema == CLIENT_STATE_SCHEMA)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateCache {
    dir: PathBuf,
}

impl UpdateCache {
    pub fn new(dir: PathBuf) -> UpdateCache {
        UpdateCache { dir }
    }

    /// The cache folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Missing or unreadable → default.
    pub fn load_state(&self) -> ClientState {
        fs::read_to_string(self.dir.join(STATE_FILE))
            .ok()
            .and_then(|text| ClientState::parse(&text))
            .unwrap_or_default()
    }

    /// Temporary file, then rename.
    pub fn save_state(&self, state: &ClientState) -> io::Result<()> {
        self.write_file(STATE_FILE, state.to_json().as_bytes())
    }

    /// The manifest and the signature that verified it.
    pub fn store_manifest(&self, manifest: &[u8], signature: &[u8]) -> io::Result<()> {
        self.write_file(MANIFEST_NAME, manifest)?;
        self.write_file(SIGNATURE_NAME, signature)
    }

    /// Both files, each within its size limit; `None` when one is missing, too large or
    /// unreadable.
    pub fn load_manifest(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let manifest = read_limited(&self.dir.join(MANIFEST_NAME), MAX_MANIFEST_LEN)?;
        let signature = read_limited(&self.dir.join(SIGNATURE_NAME), MAX_SIGNATURE_LEN)?;
        Some((manifest, signature))
    }

    /// `<dir>\<name>`. `name` is an installer's name from a verified manifest; anything with a
    /// path in it is reduced to its last part, so that nothing outside the folder is ever named.
    pub fn installer_path(&self, name: &str) -> PathBuf {
        self.dir.join(file_part(name))
    }

    /// `<dir>\<name>.part`.
    pub fn partial_path(&self, name: &str) -> PathBuf {
        self.dir
            .join(format!("{}{PARTIAL_SUFFIX}", file_part(name)))
    }

    /// Deletes every installer and `.part` except `keep_installer`. The GUI calls it only under
    /// the conditions of design m5b D.11.
    pub fn prune(&self, keep_installer: Option<&str>) -> io::Result<()> {
        let keep = keep_installer.map(file_part);
        self.remove_matching(|name| {
            if let Some(installer) = name.strip_suffix(PARTIAL_SUFFIX) {
                return is_installer_name(installer);
            }
            is_installer_name(name) && Some(name) != keep
        })
    }

    /// Deletes every `.part` (at start: a download is never resumed, design m5b E.1).
    pub fn remove_partials(&self) -> io::Result<()> {
        self.remove_matching(|name| {
            name.strip_suffix(PARTIAL_SUFFIX)
                .is_some_and(is_installer_name)
        })
    }

    /// The cached installer of `asset` when its size and SHA-256 match (read in full).
    pub fn matching_installer(&self, asset: &SelectedAsset) -> Option<PathBuf> {
        let path = self.installer_path(&asset.name);
        let metadata = fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() != asset.size {
            return None;
        }
        let mut file = fs::File::open(&path).ok()?;
        let mut hash = Sha256Stream::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            total += read as u64;
            hash.update(&buffer[..read]);
        }
        (total == asset.size && hash.finish() == asset.sha256).then_some(path)
    }

    /// Writes `<dir>\<name>` through `<name>.tmp` and a rename (creates the folder).
    fn write_file(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let temporary = self.dir.join(format!("{name}.tmp"));
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, self.dir.join(name))
    }

    /// Deletes the regular files of the folder whose name `matches` (a missing folder is fine).
    fn remove_matching(&self, matches: impl Fn(&str) -> bool) -> io::Result<()> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut first_error = None;
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let regular = entry.file_type().is_ok_and(|kind| kind.is_file());
            if regular
                && matches(&name)
                && let Err(error) = fs::remove_file(entry.path())
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

/// The last part of `name` (an installer's name has no separators; anything else is reduced).
fn file_part(name: &str) -> &str {
    name.rsplit(['\\', '/', ':']).next().unwrap_or(name)
}

/// `MKLM-Setup-<X.Y.Z>-<x64|arm64>.exe`.
pub fn is_installer_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("MKLM-Setup-") else {
        return false;
    };
    ["-x64.exe", "-arm64.exe"].iter().any(|suffix| {
        rest.strip_suffix(suffix).is_some_and(|version| {
            let parts: Vec<&str> = version.split('.').collect();
            parts.len() == 3
                && parts.iter().all(|part| {
                    !part.is_empty()
                        && part.len() <= 5
                        && part.bytes().all(|b| b.is_ascii_digit())
                        && (*part == "0" || !part.starts_with('0'))
                })
        })
    })
}

/// At most `limit` bytes of `path`; `None` when it is missing, unreadable or longer.
fn read_limited(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch cache folder under the temporary directory, removed on drop.
    #[derive(Debug)]
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            Scratch(std::env::temp_dir().join(format!(
                "mklm-update-cache-test-{tag}-{}-{nanos}",
                std::process::id()
            )))
        }

        pub(crate) fn cache(&self) -> UpdateCache {
            UpdateCache::new(self.0.clone())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_state_round_trips_and_a_bad_file_reads_as_nothing() {
        let scratch = Scratch::new("state");
        let cache = scratch.cache();
        assert_eq!(cache.load_state(), ClientState::default());
        let state = ClientState {
            last_check: Some(1_792_022_400),
            last_success: Some(1_792_022_400),
            last_failure: Some(CheckFailure {
                class: ErrorClass::Structural,
                message_id: "upd-gh-changed".into(),
                first_at: 1_792_000_000,
                at: 1_792_022_000,
            }),
            last_rollback: Some(RollbackNote {
                issued_at: 1,
                seen: 2,
                at: 3,
            }),
            cached_signature: Some(SignatureSlot::Alt),
            ..ClientState::default()
        };
        cache.save_state(&state).unwrap();
        assert_eq!(cache.load_state(), state);
        assert!(!scratch.0.join("state.json.tmp").exists());
        fs::write(scratch.0.join(STATE_FILE), "{not json").unwrap();
        assert_eq!(cache.load_state(), ClientState::default());
        // Another schema is not read as this one.
        fs::write(scratch.0.join(STATE_FILE), r#"{"schema":2}"#).unwrap();
        assert_eq!(cache.load_state(), ClientState::default());
        // Unknown fields and missing ones are fine.
        fs::write(
            scratch.0.join(STATE_FILE),
            r#"{"schema":1,"last_check":5,"future":true}"#,
        )
        .unwrap();
        assert_eq!(cache.load_state().last_check, Some(5));
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["last_failure"]["class"], "structural");
        assert_eq!(json["cached_signature"], "alt");
    }

    #[test]
    fn the_manifest_and_its_signature() {
        let scratch = Scratch::new("manifest");
        let cache = scratch.cache();
        assert_eq!(cache.load_manifest(), None);
        cache.store_manifest(b"{}", b"sig").unwrap();
        assert_eq!(
            cache.load_manifest(),
            Some((b"{}".to_vec(), b"sig".to_vec()))
        );
        // Over the limits: not read.
        fs::write(
            scratch.0.join(SIGNATURE_NAME),
            vec![b'a'; MAX_SIGNATURE_LEN + 1],
        )
        .unwrap();
        assert_eq!(cache.load_manifest(), None);
    }

    #[test]
    fn installers_and_downloads_under_way() {
        let scratch = Scratch::new("prune");
        let cache = scratch.cache();
        assert!(cache.prune(None).is_ok());
        fs::create_dir_all(&scratch.0).unwrap();
        for name in [
            "MKLM-Setup-0.2.1-x64.exe",
            "MKLM-Setup-0.2.2-x64.exe",
            "MKLM-Setup-0.2.2-x64.exe.part",
            "state.json",
            "latest.json",
            "notes.txt",
        ] {
            fs::write(scratch.0.join(name), b"x").unwrap();
        }
        cache.remove_partials().unwrap();
        assert!(!scratch.0.join("MKLM-Setup-0.2.2-x64.exe.part").exists());
        fs::write(scratch.0.join("MKLM-Setup-0.2.2-x64.exe.part"), b"x").unwrap();
        cache.prune(Some("MKLM-Setup-0.2.2-x64.exe")).unwrap();
        let mut left: Vec<String> = fs::read_dir(&scratch.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "MKLM-Setup-0.2.2-x64.exe",
                "latest.json",
                "notes.txt",
                "state.json"
            ]
        );
        cache.prune(None).unwrap();
        assert!(!scratch.0.join("MKLM-Setup-0.2.2-x64.exe").exists());
        // Names never leave the folder.
        assert_eq!(
            cache.installer_path(r"..\..\evil.exe"),
            scratch.0.join("evil.exe")
        );
        assert_eq!(
            cache.partial_path("MKLM-Setup-0.2.1-x64.exe"),
            scratch.0.join("MKLM-Setup-0.2.1-x64.exe.part")
        );
        assert!(is_installer_name("MKLM-Setup-0.2.1-arm64.exe"));
        assert!(!is_installer_name("MKLM-Setup-0.2.01-x64.exe"));
        assert!(!is_installer_name("MKLM-Setup-0.2.1-x64.exe.part"));
    }
}
