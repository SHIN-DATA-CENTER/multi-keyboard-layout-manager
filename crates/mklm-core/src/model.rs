//! Data model shared by every MKLM component.
//!
//! `mklm-win` fills these types from the live system (read-only), `mklm-cli` and the GUI render them,
//! and the rule modules of this crate evaluate them. Field names are part of the JSON output of
//! `mklm-cli status --json`, so rename them only together with that output.

use serde::{Deserialize, Serialize};

/// Container ID that Windows gives to every device built into the computer.
pub const INTERNAL_CONTAINER_ID: &str = "{00000000-0000-0000-FFFF-FFFFFFFFFFFF}";

/// Registry value names. kbdhid and i8042prt read differently named values (note the word order).
pub mod value_names {
    /// kbdhid.sys, in the HID keyboard collection's "Device Parameters".
    pub const HID_TYPE: &str = "KeyboardTypeOverride";
    /// kbdhid.sys, in the HID keyboard collection's "Device Parameters".
    pub const HID_SUBTYPE: &str = "KeyboardSubtypeOverride";
    /// kbdhid.sys extras. Never written by MKLM; only preserved.
    pub const HID_TOTAL_KEYS: &str = "KeyboardNumberTotalKeysOverride";
    /// kbdhid.sys extras. Never written by MKLM; only preserved.
    pub const HID_FUNCTION_KEYS: &str = "KeyboardNumberFunctionKeysOverride";
    /// kbdhid.sys extras. Never written by MKLM; only preserved.
    pub const HID_INDICATORS: &str = "KeyboardNumberIndicatorsOverride";
    /// i8042prt.sys, per device ("Device Parameters") and globally (`Services\i8042prt\Parameters`).
    pub const PS2_TYPE: &str = "OverrideKeyboardType";
    /// i8042prt.sys, per device ("Device Parameters") and globally (`Services\i8042prt\Parameters`).
    pub const PS2_SUBTYPE: &str = "OverrideKeyboardSubtype";
    /// Global only. Read by kbdjpn.dll to pick the primary table for untagged keyboards.
    pub const LAYER_DRIVER_JPN: &str = "LayerDriver JPN";
    /// Global only. Read by kbdkor.dll. MKLM never writes it.
    pub const LAYER_DRIVER_KOR: &str = "LayerDriver KOR";
    /// Global only. Read by the Microsoft IME; kept consistent with `LayerDriver JPN`.
    pub const KEYBOARD_IDENTIFIER: &str = "OverrideKeyboardIdentifier";
}

/// Keyboard type/subtype pair as reported by a keyboard driver
/// (`KEYBOARD_ID` / `RID_DEVICE_INFO_KEYBOARD.dwType`/`dwSubType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyboardType {
    #[serde(rename = "type")]
    pub ty: u32,
    pub subtype: u32,
}

impl KeyboardType {
    /// IBM enhanced 101/102-key. Under kbdjpn.dll it selects kbd101.dll (US).
    pub const US: Self = Self { ty: 4, subtype: 0 };
    /// Japanese 106/109-key. Under kbdjpn.dll it selects kbd106.dll (JIS).
    pub const JIS: Self = Self { ty: 7, subtype: 2 };
    /// What the Settings app's global "English keyboard (101/102)" writes. Not used per device in v1.
    pub const US_ON_JAPANESE: Self = Self { ty: 7, subtype: 0 };
    /// NEC PC-98 layout (kbdnec.dll). Recognised, never written.
    pub const NEC: Self = Self {
        ty: 7,
        subtype: 0xD02,
    };
    /// What kbdhid.sys reports when no override is set.
    pub const HID_UNKNOWN: Self = Self {
        ty: 0x51,
        subtype: 0,
    };

    pub const fn new(ty: u32, subtype: u32) -> Self {
        Self { ty, subtype }
    }
}

/// Physical layout a user can assign to a keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Jis,
    Us,
}

impl Layout {
    /// Type/subtype MKLM writes for this layout (both HID and PS/2).
    pub const fn keyboard_type(self) -> KeyboardType {
        match self {
            Layout::Jis => KeyboardType::JIS,
            Layout::Us => KeyboardType::US,
        }
    }
}

/// Driver serving the keyboard collection. Decides which registry value names apply.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "service")]
pub enum KeyboardDriver {
    Kbdhid,
    I8042prt,
    /// Any other service name (e.g. virtual or vendor drivers). Read-only for MKLM.
    Other(String),
}

/// How the keyboard is attached, derived from the devnode's ancestor chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Usb,
    BluetoothClassic,
    BluetoothLe,
    Ps2,
    I2c,
    Spi,
    /// Remote Desktop, VM or software keyboards. Always read-only.
    Virtual,
    Unknown,
}

/// Override values found in the device's hardware key ("Device Parameters").
/// `None` means the value does not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceOverrides {
    /// `KeyboardTypeOverride` (kbdhid).
    pub keyboard_type_override: Option<u32>,
    /// `KeyboardSubtypeOverride` (kbdhid).
    pub keyboard_subtype_override: Option<u32>,
    /// `OverrideKeyboardType` (i8042prt).
    pub override_keyboard_type: Option<u32>,
    /// `OverrideKeyboardSubtype` (i8042prt).
    pub override_keyboard_subtype: Option<u32>,
    /// `KeyboardNumberTotalKeysOverride` (kbdhid, preserved only).
    pub number_total_keys_override: Option<u32>,
    /// `KeyboardNumberFunctionKeysOverride` (kbdhid, preserved only).
    pub number_function_keys_override: Option<u32>,
    /// `KeyboardNumberIndicatorsOverride` (kbdhid, preserved only).
    pub number_indicators_override: Option<u32>,
}

/// One Keyboard-class devnode (a HID keyboard collection or a PS/2 keyboard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardDevice {
    /// Device instance ID, e.g. `HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000`.
    pub instance_id: String,
    /// Best human-readable name: the top-most ancestor in the same container
    /// (bus-reported description or friendly name), else the devnode's own description.
    pub display_name: String,
    /// The devnode's own friendly name / device description (e.g. "HID キーボード デバイス").
    pub device_description: Option<String>,
    /// `DEVPKEY_Device_ContainerId` in upper-case braces form.
    pub container_id: Option<String>,
    /// True when the container is [`INTERNAL_CONTAINER_ID`].
    pub is_internal: bool,
    pub present: bool,
    pub driver: KeyboardDriver,
    pub transport: Transport,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    /// Serial number taken from the USB parent's instance ID, when the device reports one.
    pub usb_serial: Option<String>,
    pub hardware_ids: Vec<String>,
    /// Ancestor instance IDs, nearest parent first, stopping at the first ancestor outside the device's container.
    pub parent_chain: Vec<String>,
    pub overrides: DeviceOverrides,
    /// Type/subtype the driver reports right now (Raw Input). `None` for non-present devices.
    pub reported_type: Option<KeyboardType>,
    /// `DEVPKEY_Device_DevNodeStatus` (DN_* flags), when available.
    pub dev_node_status: Option<u32>,
    /// `DEVPKEY_Device_ProblemCode` (CM_PROB_*), when available.
    pub problem_code: Option<u32>,
}

/// Values under `HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters` that decide the global behaviour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSettings {
    pub layer_driver_jpn: Option<String>,
    pub layer_driver_kor: Option<String>,
    pub override_keyboard_identifier: Option<String>,
    pub override_keyboard_type: Option<u32>,
    pub override_keyboard_subtype: Option<u32>,
}

/// Whether Windows forces one layout on every keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlobalMode {
    /// Global `OverrideKeyboardType/Subtype` exist: per-device values are ignored for mapping.
    Fixed,
    /// Neither global value exists: each keyboard's own type decides its table.
    PerKeyboard,
}

/// Input methods known to the system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputMethods {
    /// Current user's `HKCU\Keyboard Layout\Preload`, in order (KLIDs such as `"00000411"`).
    pub user_preload: Vec<String>,
    /// Sign-in screen's `HKU\.DEFAULT\Keyboard Layout\Preload`, in order.
    pub sign_in_preload: Vec<String>,
    /// Layouts loaded in the calling session (`GetKeyboardLayoutList`), as the low 32 bits of each HKL.
    pub loaded_layouts: Vec<u32>,
}

/// Operating system facts relevant to support decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsInfo {
    /// Build number from `RtlGetVersion` (e.g. 26200).
    pub build: u32,
    /// Update build revision (UBR) from the registry, when available.
    pub ubr: Option<u32>,
    /// Native machine architecture, e.g. `"x64"` or `"arm64"`.
    pub native_arch: String,
    /// True when the current process runs in a Remote Desktop session.
    pub remote_session: bool,
}

/// Everything MKLM reads from the system in one pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub keyboards: Vec<KeyboardDevice>,
    pub global: GlobalSettings,
    pub input: InputMethods,
    pub os: OsInfo,
}
