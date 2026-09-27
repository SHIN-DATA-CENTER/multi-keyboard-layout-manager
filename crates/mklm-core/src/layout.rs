//! Which layout table a keyboard ends up with (kbdjpn.dll's selection rules).
//!
//! Every rule here assumes the active input method is the Japanese IME (layout DLL kbdjpn.dll);
//! with any other input method every keyboard uses that method's own layout.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{GlobalMode, KeyboardType, Layout};

/// Layer driver of the JIS table.
pub const KBD106: &str = "kbd106.dll";
/// Layer driver of the JIS table with the "n" key assignments; treated as JIS.
pub const KBD106N: &str = "kbd106n.dll";
/// Layer driver of the US table.
pub const KBD101: &str = "kbd101.dll";
/// Layer driver of the NEC PC-98 table.
pub const KBDNEC: &str = "kbdnec.dll";
/// `OverrideKeyboardIdentifier` matching [`KBD106`].
pub const PCAT_106KEY: &str = "PCAT_106KEY";
/// `OverrideKeyboardIdentifier` matching [`KBD101`].
pub const PCAT_101KEY: &str = "PCAT_101KEY";

impl fmt::Display for KeyboardType {
    /// Formats as `0x7/0x2`, matching `tools/m0/Get-KbdState.ps1`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#X}/{:#X}", self.ty, self.subtype)
    }
}

/// Layout table (layer driver) kbdjpn.dll uses for a keyboard, or the PC's standard layout.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutTable {
    /// kbd106.dll
    Jis,
    /// kbd101.dll
    Us,
    /// Any other layer driver, e.g. `kbdnec.dll`.
    Other(String),
}

impl LayoutTable {
    /// Maps a `LayerDriver JPN` value: kbd106/kbd106n → JIS, kbd101 or missing → US, else other.
    pub fn from_layer_driver(layer_driver: Option<&str>) -> Self {
        match layer_driver {
            None => LayoutTable::Us,
            Some(dll) if dll.eq_ignore_ascii_case(KBD106) || dll.eq_ignore_ascii_case(KBD106N) => {
                LayoutTable::Jis
            }
            Some(dll) if dll.eq_ignore_ascii_case(KBD101) => LayoutTable::Us,
            Some(dll) => LayoutTable::Other(dll.to_string()),
        }
    }

    /// The assignable layout this table corresponds to, if any.
    pub fn layout(&self) -> Option<Layout> {
        match self {
            LayoutTable::Jis => Some(Layout::Jis),
            LayoutTable::Us => Some(Layout::Us),
            LayoutTable::Other(_) => None,
        }
    }
}

impl From<Layout> for LayoutTable {
    fn from(layout: Layout) -> Self {
        match layout {
            Layout::Jis => LayoutTable::Jis,
            Layout::Us => LayoutTable::Us,
        }
    }
}

impl Layout {
    /// `LayerDriver JPN` value MKLM writes when this layout is the PC's standard.
    pub const fn layer_driver(self) -> &'static str {
        match self {
            Layout::Jis => KBD106,
            Layout::Us => KBD101,
        }
    }

    /// `OverrideKeyboardIdentifier` value that must accompany [`Layout::layer_driver`].
    pub const fn keyboard_identifier(self) -> &'static str {
        match self {
            Layout::Jis => PCAT_106KEY,
            Layout::Us => PCAT_101KEY,
        }
    }

    /// Global type/subtype the Settings app writes for this standard in fixed mode (7/2 or 7/0).
    pub const fn fixed_mode_type(self) -> KeyboardType {
        match self {
            Layout::Jis => KeyboardType::JIS,
            Layout::Us => KeyboardType::US_ON_JAPANESE,
        }
    }
}

impl KeyboardType {
    /// Table kbdjpn.dll picks for this type in per-keyboard mode, or `None` when the type has no
    /// table of its own and the standard layout applies (0x51/0, 7/0 and anything unknown).
    pub fn per_keyboard_table(self) -> Option<LayoutTable> {
        match self {
            KeyboardType::US => Some(LayoutTable::Us),
            KeyboardType::JIS => Some(LayoutTable::Jis),
            KeyboardType::NEC => Some(LayoutTable::Other(KBDNEC.to_string())),
            _ => None,
        }
    }
}

/// Why a keyboard gets its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutBasis {
    /// Fixed mode: every keyboard uses the standard layout, whatever its type (verified in M0 #2a).
    FixedMode,
    /// Per-keyboard mode and the type selects its own table (4/0, 7/2, NEC).
    KeyboardType,
    /// Per-keyboard mode and the type has no table of its own: the standard layout applies.
    Standard,
}

/// Layout a keyboard types with, assuming the Japanese IME is the active input method.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectiveLayout {
    /// Type/subtype the mapping is based on.
    pub keyboard_type: KeyboardType,
    pub table: LayoutTable,
    pub basis: LayoutBasis,
    /// False when the mapping is not confirmed on real hardware: NEC, a fixed mode whose standard is
    /// neither JIS nor US, and in per-keyboard mode every type that follows the standard (0x51/0,
    /// 7/0 until M0 #5), which M0 has not typed with yet.
    pub verified: bool,
}

/// Applies kbdjpn.dll's selection to one keyboard type.
pub fn effective_layout(
    mode: GlobalMode,
    standard: &LayoutTable,
    keyboard_type: KeyboardType,
) -> EffectiveLayout {
    let (table, basis) = match (mode, keyboard_type.per_keyboard_table()) {
        (GlobalMode::Fixed, _) => (standard.clone(), LayoutBasis::FixedMode),
        (GlobalMode::PerKeyboard, Some(table)) => (table, LayoutBasis::KeyboardType),
        (GlobalMode::PerKeyboard, None) => (standard.clone(), LayoutBasis::Standard),
    };
    let verified = match basis {
        // M0 #2a (JIS), and the Settings app's own "English keyboard" (US); not other layer drivers.
        LayoutBasis::FixedMode => table.layout().is_some(),
        // M0 #4 and #2b.
        LayoutBasis::KeyboardType => {
            keyboard_type == KeyboardType::US || keyboard_type == KeyboardType::JIS
        }
        LayoutBasis::Standard => false,
    };
    EffectiveLayout {
        keyboard_type,
        table,
        basis,
        verified,
    }
}

/// What the user has to do before stored values take effect. Ordered from lightest to heaviest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingAction {
    /// Restart the keyboard devnode (DICS_PROPCHANGE). Verified for USB in M0 #2b.
    ResetKeyboard,
    /// Unplug and replug, or reconnect (Bluetooth: power the keyboard off and on).
    Reconnect,
    /// Restart Windows. A shutdown is not enough while Fast Startup is on.
    RestartPc,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_from_layer_driver() {
        assert_eq!(
            LayoutTable::from_layer_driver(Some("kbd106.dll")),
            LayoutTable::Jis
        );
        assert_eq!(
            LayoutTable::from_layer_driver(Some("KBD106N.DLL")),
            LayoutTable::Jis
        );
        assert_eq!(
            LayoutTable::from_layer_driver(Some("kbd101.dll")),
            LayoutTable::Us
        );
        assert_eq!(LayoutTable::from_layer_driver(None), LayoutTable::Us);
        assert_eq!(
            LayoutTable::from_layer_driver(Some("kbd101a.dll")),
            LayoutTable::Other("kbd101a.dll".to_string())
        );
        assert_eq!(
            LayoutTable::from_layer_driver(Some("")),
            LayoutTable::Other(String::new())
        );
    }

    #[test]
    fn fixed_mode_ignores_type() {
        for ty in [
            KeyboardType::US,
            KeyboardType::JIS,
            KeyboardType::HID_UNKNOWN,
            KeyboardType::US_ON_JAPANESE,
        ] {
            let jis = effective_layout(GlobalMode::Fixed, &LayoutTable::Jis, ty);
            assert_eq!(jis.table, LayoutTable::Jis);
            assert_eq!(jis.basis, LayoutBasis::FixedMode);
            assert!(jis.verified);
            assert_eq!(
                effective_layout(GlobalMode::Fixed, &LayoutTable::Us, ty).table,
                LayoutTable::Us
            );
            let nec = LayoutTable::Other(KBDNEC.to_string());
            assert!(!effective_layout(GlobalMode::Fixed, &nec, ty).verified);
        }
    }

    #[test]
    fn per_keyboard_table_selection() {
        let pk = |ty| effective_layout(GlobalMode::PerKeyboard, &LayoutTable::Jis, ty);
        assert_eq!(pk(KeyboardType::US).table, LayoutTable::Us);
        assert_eq!(pk(KeyboardType::US).basis, LayoutBasis::KeyboardType);
        assert_eq!(pk(KeyboardType::JIS).table, LayoutTable::Jis);
        assert_eq!(
            pk(KeyboardType::NEC).table,
            LayoutTable::Other(KBDNEC.to_string())
        );
        assert!(!pk(KeyboardType::NEC).verified);
        assert_eq!(pk(KeyboardType::HID_UNKNOWN).table, LayoutTable::Jis);
        assert_eq!(pk(KeyboardType::HID_UNKNOWN).basis, LayoutBasis::Standard);
        // Following the standard is not typed on real hardware yet.
        assert!(!pk(KeyboardType::HID_UNKNOWN).verified);
        assert!(pk(KeyboardType::US).verified && pk(KeyboardType::JIS).verified);
        assert_eq!(pk(KeyboardType::US_ON_JAPANESE).table, LayoutTable::Jis);
        assert!(!pk(KeyboardType::US_ON_JAPANESE).verified);
        assert_eq!(pk(KeyboardType::new(8, 3)).basis, LayoutBasis::Standard);

        let us_standard = effective_layout(
            GlobalMode::PerKeyboard,
            &LayoutTable::Us,
            KeyboardType::HID_UNKNOWN,
        );
        assert_eq!(us_standard.table, LayoutTable::Us);
        assert_eq!(
            effective_layout(GlobalMode::PerKeyboard, &LayoutTable::Us, KeyboardType::JIS).table,
            LayoutTable::Jis
        );
    }

    #[test]
    fn layout_constants() {
        assert_eq!(Layout::Jis.layer_driver(), KBD106);
        assert_eq!(Layout::Us.keyboard_identifier(), PCAT_101KEY);
        assert_eq!(Layout::Us.fixed_mode_type(), KeyboardType::new(7, 0));
        assert_eq!(LayoutTable::from(Layout::Jis).layout(), Some(Layout::Jis));
        assert_eq!(LayoutTable::Other("x".into()).layout(), None);
    }

    #[test]
    fn display_and_order() {
        assert_eq!(KeyboardType::NEC.to_string(), "0x7/0xD02");
        assert_eq!(KeyboardType::HID_UNKNOWN.to_string(), "0x51/0x0");
        assert!(PendingAction::ResetKeyboard < PendingAction::Reconnect);
        assert!(PendingAction::Reconnect < PendingAction::RestartPc);
    }
}
