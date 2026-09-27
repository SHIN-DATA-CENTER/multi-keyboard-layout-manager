//! Per-user settings in `%APPDATA%\SHIN DATA CENTER\MKLM\settings.toml` (plan 3.7; design m3 F.4).
//!
//! Only what belongs to the user and the GUI lives here. The autostart state is the HKCU Run value
//! itself (`mklm_win::session::autostart_state`), "restore on uninstall" is machine-wide in HKLM
//! (`mklm_win::machine_settings`), and the journal is the only record of keyboard changes.
//!
//! Reading never fails the start: a missing file gives the defaults; an unreadable one gives the
//! defaults too (WP-U6: keep it as `settings.toml.bad` before the next save). Saving writes a
//! temporary file and renames it over the old one (on the I/O worker, design m3 A.4).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::i18n::LangChoice;
use crate::theme::ThemeMode;

/// The file name inside the settings directory.
pub const SETTINGS_FILE: &str = "settings.toml";

/// The current schema; a newer file is read as far as it goes (unknown keys are ignored) and not
/// overwritten with an older schema number.
pub const SETTINGS_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    pub theme: ThemeMode,
    pub language: LangChoice,
    pub wizard: WizardSettings,
    pub tray: TraySettings,
    pub keyboards: KeyboardSettings,
    pub recovery: RecoverySettings,
    pub change: ChangeSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA,
            theme: ThemeMode::System,
            language: LangChoice::System,
            wizard: WizardSettings::default(),
            tray: TraySettings::default(),
            keyboards: KeyboardSettings::default(),
            recovery: RecoverySettings::default(),
            change: ChangeSettings::default(),
        }
    }
}

/// How changes are made (design m3 B.5, B.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangeSettings {
    /// The standalone UAC explanation was read once; later changes explain the prompt in one
    /// line next to the button (review U14).
    pub uac_notice_seen: bool,
    /// The keep-or-revert time: 20 s, or 60 s ("確認の時間を長くする", review U10). Sent to the
    /// helper in `ApplyOptions`, which allows only these two (WP-E3).
    pub countdown_seconds: u32,
}

impl Default for ChangeSettings {
    fn default() -> Self {
        Self {
            uac_notice_seen: false,
            countdown_seconds: 20,
        }
    }
}

/// What a keyboard physically is, as MKLM found out (design m3 B.2; review U4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalLayout {
    /// The keyboard's instance ID.
    pub id: String,
    pub layout: PhysicalKind,
    pub source: PhysicalSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhysicalKind {
    Jis,
    Us,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhysicalSource {
    /// The two-key layout detection (design m3 B.3).
    Detection,
    /// A key only JIS keyboards have (0x70, 0x79, 0x7B, 0x7D, 0x73) was pressed on it. Only
    /// JIS can be learned this way: a US keyboard is never inferred from keys it lacks.
    JisOnlyKey,
}

/// The first-run wizard (plan 3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WizardSettings {
    /// Finished or skipped once; the wizard then opens only from Settings.
    pub completed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraySettings {
    /// The first close-to-tray notice was shown (plan 3.9: once).
    pub close_notice_shown: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    /// Devices the user hid (group IDs: the container ID, or the instance ID of a keyboard
    /// without a usable container), compared case-insensitively.
    pub hidden: Vec<String>,
    /// Keyboards a key press was seen from (instance IDs): they lose the "キー入力なし" badge
    /// (plan 3.2). Only IDs are kept, never keys (plan 2.2).
    pub seen: Vec<String>,
    pub show_hidden: bool,
    /// Physical layouts learned from the detection or from JIS-only keys (a derived fact per
    /// instance ID, never the keys themselves, plan 2.2).
    pub physical: Vec<PhysicalLayout>,
}

/// Design m2 D.7: the automatic recovery prompt appears at most once per boot for an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoverySettings {
    pub prompted: Vec<PromptedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptedEntry {
    pub op: String,
    pub boot: String,
}

impl Settings {
    /// Parses a settings document; `Err` for a document that is not valid TOML of this shape.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Reads `dir\settings.toml`. `Ok(None)` when there is none; `Err` with the reason when it is
    /// unreadable (the caller warns and uses the defaults).
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        match fs::read_to_string(dir.join(SETTINGS_FILE)) {
            Ok(text) => Self::from_toml(&text).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Writes `dir\settings.toml` through a temporary file and a rename (creates `dir`).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let target = dir.join(SETTINGS_FILE);
        let temporary: PathBuf = dir.join(format!("{SETTINGS_FILE}.tmp"));
        fs::write(&temporary, self.to_toml())?;
        fs::rename(&temporary, &target)
    }

    /// True when `id` is hidden.
    pub fn is_hidden(&self, id: &str) -> bool {
        self.keyboards
            .hidden
            .iter()
            .any(|hidden| hidden.eq_ignore_ascii_case(id))
    }

    /// True when a key press was seen from `instance_id`.
    pub fn was_seen(&self, instance_id: &str) -> bool {
        self.keyboards
            .seen
            .iter()
            .any(|seen| seen.eq_ignore_ascii_case(instance_id))
    }

    /// What `instance_id` physically is, if known.
    pub fn physical(&self, instance_id: &str) -> Option<&PhysicalLayout> {
        self.keyboards
            .physical
            .iter()
            .find(|known| known.id.eq_ignore_ascii_case(instance_id))
    }

    /// Records a physical layout; true when that changed what is known. A detection result
    /// replaces anything; a JIS-only key never overrides a detection.
    pub fn learn_physical(&mut self, record: PhysicalLayout) -> bool {
        match self
            .keyboards
            .physical
            .iter_mut()
            .find(|known| known.id.eq_ignore_ascii_case(&record.id))
        {
            Some(known) if *known == record => false,
            Some(known)
                if known.source == PhysicalSource::Detection
                    && record.source == PhysicalSource::JisOnlyKey =>
            {
                false
            }
            Some(known) => {
                *known = record;
                true
            }
            None => {
                self.keyboards.physical.push(record);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let settings = Settings {
            theme: ThemeMode::Dark,
            keyboards: KeyboardSettings {
                seen: vec![r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".into()],
                ..KeyboardSettings::default()
            },
            ..Settings::default()
        };
        let text = settings.to_toml();
        assert_eq!(Settings::from_toml(&text).unwrap(), settings);
        assert!(
            Settings::from_toml(&text)
                .unwrap()
                .was_seen(r"hid\vid_3434&pid_d027&mi_00&col01\8&148ad7e3&0&0000")
        );
        // Missing keys take their defaults; unknown keys are ignored.
        let partial = Settings::from_toml("theme = \"light\"\nfuture = 1\n").unwrap();
        assert_eq!(partial.theme, ThemeMode::Light);
        assert_eq!(partial.language, LangChoice::System);
        assert!(Settings::from_toml("theme = 3").is_err());
    }

    #[test]
    fn physical_layouts() {
        let mut settings = Settings::default();
        let id = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
        let detected = PhysicalLayout {
            id: id.into(),
            layout: PhysicalKind::Us,
            source: PhysicalSource::Detection,
        };
        assert!(settings.learn_physical(detected.clone()));
        assert!(!settings.learn_physical(detected.clone()));
        // A JIS-only key does not override a detection.
        assert!(!settings.learn_physical(PhysicalLayout {
            layout: PhysicalKind::Jis,
            source: PhysicalSource::JisOnlyKey,
            ..detected.clone()
        }));
        assert_eq!(settings.physical(&id.to_lowercase()), Some(&detected));
        let text = settings.to_toml();
        assert_eq!(Settings::from_toml(&text).unwrap(), settings);
        assert_eq!(settings.change.countdown_seconds, 20);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("mklm-settings-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(Settings::load(&dir), Ok(None));
        let mut settings = Settings::default();
        settings.wizard.completed = true;
        settings.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), Ok(Some(settings)));
        let _ = fs::remove_dir_all(&dir);
    }
}
