//! Human-readable output of `list`, `status` and `global status` (English only for now).

use std::fmt::{self, Write as _};

use mklm_core::{
    Assessment, DN_HAS_PROBLEM, DN_STARTED, DeviceOverrides, EffectiveLayout, GlobalAnomaly,
    GlobalMode, GlobalSettings, InputMethods, InputWarning, InvPs2Status, KeyboardAnomaly,
    KeyboardAssessment, KeyboardDevice, KeyboardDriver, KeyboardType, LayoutBasis, LayoutTable,
    OsInfo, PendingAction, SystemSnapshot, Transport, global_anomalies, hkl_to_string,
    input_warnings, value_names,
};

use crate::table::Table;

/// Registry path of the global values, as shown to the user.
const GLOBAL_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters";
/// Width of the label column in key/value sections.
const LABEL: usize = 28;
/// Widest keyboard name shown in the `list` table.
const NAME_COLUMN: usize = 28;

/// `mklm-cli list`: one table row per keyboard, then the global mode, standard layout and warnings.
pub fn list(
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    include_non_present: bool,
) -> String {
    render(|out| write_list(out, snapshot, assessment, include_non_present))
}

/// `mklm-cli status`: every value read from the system, and its evaluation.
pub fn status(
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    include_non_present: bool,
) -> String {
    render(|out| write_status(out, snapshot, assessment, include_non_present))
}

/// `mklm-cli global status`: the global values, mode, standard layout and input methods.
pub fn global_status(global: &GlobalSettings, input: &InputMethods) -> String {
    render(|out| {
        write_global(
            out,
            global,
            global.mode(),
            &global.standard_layout(),
            &global_anomalies(global),
        )?;
        writeln!(out)?;
        write_input(out, input, &input_warnings(input))
    })
}

fn render(write: impl FnOnce(&mut String) -> fmt::Result) -> String {
    let mut out = String::new();
    write(&mut out).expect("writing to a String cannot fail");
    out
}

/// Pairs every keyboard with its assessment (`assess` keeps the snapshot order).
fn rows<'a>(
    snapshot: &'a SystemSnapshot,
    assessment: &'a Assessment,
) -> impl Iterator<Item = (usize, &'a KeyboardDevice, &'a KeyboardAssessment)> {
    debug_assert!(
        snapshot
            .keyboards
            .iter()
            .map(|kb| &kb.instance_id)
            .eq(assessment.keyboards.iter().map(|ka| &ka.instance_id))
    );
    snapshot
        .keyboards
        .iter()
        .zip(&assessment.keyboards)
        .enumerate()
        .map(|(i, (kb, ka))| (i + 1, kb, ka))
}

fn write_list(
    out: &mut String,
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    include_non_present: bool,
) -> fmt::Result {
    keyboards_heading(out, include_non_present)?;
    if snapshot.keyboards.is_empty() {
        writeln!(out, "  No keyboards found.")?;
    } else {
        let mut table = Table::new(&[
            "#",
            "Name",
            "Driver",
            "Transport",
            "Internal",
            "VID:PID",
            "Stored",
            "Reported",
            "Now",
            "After restart",
            "Pending",
        ])
        .max_width(1, NAME_COLUMN);
        let mut markers = Markers::default();
        for (n, kb, ka) in rows(snapshot, assessment) {
            table.row(vec![
                n.to_string(),
                ka.display_name.clone(),
                driver_name(&ka.driver).to_string(),
                transport_short(ka.transport).to_string(),
                yes_no(ka.is_internal).to_string(),
                vid_pid(kb),
                type_text(ka.stored_type),
                reported_text(ka),
                markers.cell(ka.current.as_ref()),
                markers.cell(ka.after_restart.as_ref()),
                ka.pending_action.map_or("-", pending_name).to_string(),
            ]);
        }
        out.push_str(&table.render("  "));
        if markers.standard {
            writeln!(
                out,
                "  * The PC's standard layout applies (fixed mode, or the type selects no layout of its own)."
            )?;
        }
        if markers.unverified {
            writeln!(
                out,
                "  ? This type's layout is not verified on real hardware."
            )?;
        }
        writeln!(out)?;
        for (n, kb, _) in rows(snapshot, assessment) {
            writeln!(out, "  [{n}] {}", kb.instance_id)?;
        }
    }

    writeln!(out)?;
    field(out, "Mode", mode_text(assessment.mode))?;
    field(
        out,
        "Standard layout",
        standard_text(&assessment.standard_layout),
    )?;
    field(
        out,
        "Pending",
        assessment
            .pending_action
            .map_or("nothing", pending_name_long),
    )?;
    notices(
        out,
        "Input warnings",
        assessment.input_warnings.iter().map(input_warning_text),
    )?;
    let problems: Vec<String> = assessment
        .global_anomalies
        .iter()
        .map(global_anomaly_text)
        .chain(assessment.inv_ps2_violation.iter().map(|v| v.to_string()))
        .chain(rows(snapshot, assessment).flat_map(|(n, _, ka)| {
            ka.anomalies
                .iter()
                .map(move |a| format!("[{n}] {}", keyboard_anomaly_text(a)))
        }))
        .collect();
    notices(out, "Problems", problems)
}

fn write_status(
    out: &mut String,
    snapshot: &SystemSnapshot,
    assessment: &Assessment,
    include_non_present: bool,
) -> fmt::Result {
    writeln!(out, "System")?;
    field(out, "Windows build", os_text(&snapshot.os))?;
    field(
        out,
        "Remote Desktop session",
        yes_no(snapshot.os.remote_session),
    )?;
    writeln!(out)?;

    write_global(
        out,
        &snapshot.global,
        assessment.mode,
        &assessment.standard_layout,
        &assessment.global_anomalies,
    )?;
    writeln!(out)?;
    write_input(out, &snapshot.input, &assessment.input_warnings)?;
    writeln!(out)?;

    writeln!(out, "Safety")?;
    let scope = if include_non_present {
        ""
    } else {
        " (checked on connected keyboards only; --all adds disconnected ones)"
    };
    match &assessment.inv_ps2_violation {
        None if assessment.inv_ps2 == InvPs2Status::Unknown => {
            field(out, "INV-PS2", format!("unknown, holds{scope}"))?;
        }
        None => field(out, "INV-PS2", format!("holds{scope}"))?,
        Some(violation) => field(
            out,
            "INV-PS2",
            format!(
                "VIOLATED{scope}: without OverrideKeyboardType/Subtype these i8042prt keyboards \
                 report 0x4/0x0 (US) after a restart: {}",
                violation.keyboards.join(", ")
            ),
        )?,
    }
    field(
        out,
        "Pending",
        assessment
            .pending_action
            .map_or("nothing", pending_name_long),
    )?;
    writeln!(out)?;

    keyboards_heading(out, include_non_present)?;
    if snapshot.keyboards.is_empty() {
        writeln!(out, "  No keyboards found.")?;
    }
    for (n, kb, ka) in rows(snapshot, assessment) {
        write_keyboard(out, n, kb, ka)?;
    }

    writeln!(out)?;
    writeln!(out, "Device groups")?;
    if assessment.groups.is_empty() {
        writeln!(out, "  (none)")?;
    }
    for group in &assessment.groups {
        let members: Vec<String> = group
            .keyboards
            .iter()
            .map(|id| {
                snapshot
                    .keyboards
                    .iter()
                    .position(|kb| &kb.instance_id == id)
                    .map_or_else(|| id.clone(), |i| format!("[{}]", i + 1))
            })
            .collect();
        let container = match (&group.container_id, group.is_internal) {
            (Some(id), true) => format!("{id}, built-in"),
            (Some(id), false) => id.clone(),
            (None, _) => "no container ID".to_string(),
        };
        writeln!(
            out,
            "  {}  ({container})  {}",
            group.display_name,
            members.join(" ")
        )?;
    }
    Ok(())
}

fn write_keyboard(
    out: &mut String,
    n: usize,
    kb: &KeyboardDevice,
    ka: &KeyboardAssessment,
) -> fmt::Result {
    writeln!(out)?;
    writeln!(out, "  [{n}] {}", ka.display_name)?;
    let indent = "    ";
    let item = |out: &mut String, label: &str, value: &str| -> fmt::Result {
        writeln!(out, "{indent}{label:<16} {value}")
    };
    let items = |out: &mut String, label: &str, values: &[String]| -> fmt::Result {
        if values.is_empty() {
            return item(out, label, "(none)");
        }
        for (i, value) in values.iter().enumerate() {
            item(out, if i == 0 { label } else { "" }, value)?;
        }
        Ok(())
    };
    item(out, "Instance ID", &kb.instance_id)?;
    item(
        out,
        "Description",
        or_dash(kb.device_description.as_deref()),
    )?;
    item(out, "Container", or_dash(kb.container_id.as_deref()))?;
    item(out, "Internal", yes_no(ka.is_internal))?;
    item(out, "Connected", yes_no(kb.present))?;
    item(out, "Driver", driver_name(&kb.driver))?;
    item(out, "Transport", transport_name(kb.transport))?;
    item(out, "VID:PID", &vid_pid(kb))?;
    item(out, "USB serial", or_dash(kb.usb_serial.as_deref()))?;
    item(out, "Device values", &overrides_text(&kb.overrides))?;
    item(out, "Stored type", &type_text(ka.stored_type))?;
    item(out, "Reported type", &reported_text(ka))?;
    item(out, "Predicted type", &type_text(ka.predicted_type))?;
    item(out, "Types now", &layout_detail(ka.current.as_ref()))?;
    item(
        out,
        "After restart",
        &layout_detail(ka.after_restart.as_ref()),
    )?;
    item(
        out,
        "Pending",
        ka.pending_action.map_or("nothing", pending_name_long),
    )?;
    item(
        out,
        "Devnode status",
        &devnode_text(kb.dev_node_status, kb.problem_code),
    )?;
    items(out, "Hardware IDs", &kb.hardware_ids)?;
    items(out, "Parents", &kb.parent_chain)?;
    if ka.anomalies.is_empty() {
        return item(out, "Anomalies", "none");
    }
    let anomalies: Vec<String> = ka.anomalies.iter().map(keyboard_anomaly_text).collect();
    items(out, "Anomalies", &anomalies)
}

fn write_global(
    out: &mut String,
    global: &GlobalSettings,
    mode: GlobalMode,
    standard: &LayoutTable,
    anomalies: &[GlobalAnomaly],
) -> fmt::Result {
    writeln!(out, "Global settings ({GLOBAL_KEY})")?;
    let string = |value: &Option<String>| match value {
        Some(s) => format!("\"{s}\""),
        None => "(not set)".to_string(),
    };
    let dword = |value: Option<u32>| value.map_or_else(|| "(not set)".to_string(), dword_text);
    field(
        out,
        value_names::LAYER_DRIVER_JPN,
        string(&global.layer_driver_jpn),
    )?;
    field(
        out,
        value_names::LAYER_DRIVER_KOR,
        string(&global.layer_driver_kor),
    )?;
    field(
        out,
        value_names::KEYBOARD_IDENTIFIER,
        string(&global.override_keyboard_identifier),
    )?;
    field(
        out,
        value_names::PS2_TYPE,
        dword(global.override_keyboard_type),
    )?;
    field(
        out,
        value_names::PS2_SUBTYPE,
        dword(global.override_keyboard_subtype),
    )?;
    field(out, "Mode", mode_text(mode))?;
    field(out, "Standard layout", standard_text(standard))?;
    notices(out, "Anomalies", anomalies.iter().map(global_anomaly_text))
}

fn write_input(out: &mut String, input: &InputMethods, warnings: &[InputWarning]) -> fmt::Result {
    writeln!(out, "Input methods")?;
    field(
        out,
        r"Current user (HKCU Preload)",
        list_text(&input.user_preload),
    )?;
    field(
        out,
        r"Sign-in screen (.DEFAULT)",
        list_text(&input.sign_in_preload),
    )?;
    let loaded: Vec<String> = input
        .loaded_layouts
        .iter()
        .map(|&h| hkl_to_string(h))
        .collect();
    field(out, "Loaded in this session", list_text(&loaded))?;
    notices(out, "Warnings", warnings.iter().map(input_warning_text))
}

fn keyboards_heading(out: &mut String, include_non_present: bool) -> fmt::Result {
    let scope = if include_non_present {
        "connected and disconnected"
    } else {
        "connected only; --all adds disconnected ones"
    };
    writeln!(out, "Keyboards ({scope})")
}

/// One `label  value` line of a key/value section.
fn field(out: &mut String, label: &str, value: impl fmt::Display) -> fmt::Result {
    writeln!(out, "  {label:<LABEL$} {value}")
}

/// `label  none`, or the label followed by one `! message` line per notice.
fn notices(
    out: &mut String,
    label: &str,
    messages: impl IntoIterator<Item = String>,
) -> fmt::Result {
    let messages: Vec<String> = messages.into_iter().collect();
    if messages.is_empty() {
        return field(out, label, "none");
    }
    writeln!(out, "  {label}")?;
    for message in messages {
        writeln!(out, "    ! {message}")?;
    }
    Ok(())
}

/// Marks layout cells in the `list` table and remembers which legend lines are needed.
#[derive(Debug, Default)]
struct Markers {
    standard: bool,
    unverified: bool,
}

impl Markers {
    fn cell(&mut self, layout: Option<&EffectiveLayout>) -> String {
        let Some(layout) = layout else {
            return "-".to_string();
        };
        let mut cell = table_name(&layout.table).to_string();
        if layout.basis != LayoutBasis::KeyboardType {
            self.standard = true;
            cell.push('*');
        }
        if !layout.verified {
            self.unverified = true;
            cell.push('?');
        }
        cell
    }
}

fn layout_detail(layout: Option<&EffectiveLayout>) -> String {
    let Some(layout) = layout else {
        return "-".to_string();
    };
    let table = table_name(&layout.table);
    let ty = layout.keyboard_type;
    let mut text = match layout.basis {
        LayoutBasis::KeyboardType => format!("{table} (type {ty} selects it)"),
        LayoutBasis::Standard => {
            format!("{table} (standard layout; type {ty} selects none of its own)")
        }
        LayoutBasis::FixedMode => format!("{table} (fixed mode: the standard layout, type {ty})"),
    };
    if !layout.verified {
        text.push_str(", not verified on real hardware");
    }
    text
}

fn mode_text(mode: GlobalMode) -> &'static str {
    match mode {
        GlobalMode::Fixed => "fixed (every keyboard uses the standard layout)",
        GlobalMode::PerKeyboard => "per-keyboard (each keyboard's type selects its layout)",
    }
}

fn standard_text(standard: &LayoutTable) -> String {
    match standard {
        LayoutTable::Other(dll) => format!("other ({dll})"),
        table => table_name(table).to_string(),
    }
}

pub(crate) fn table_name(table: &LayoutTable) -> &str {
    match table {
        LayoutTable::Jis => "JIS",
        LayoutTable::Us => "US",
        LayoutTable::Other(dll) => dll,
    }
}

fn driver_name(driver: &KeyboardDriver) -> &str {
    match driver {
        KeyboardDriver::Kbdhid => "kbdhid",
        KeyboardDriver::I8042prt => "i8042prt",
        KeyboardDriver::Other(service) if service.is_empty() => "(no driver)",
        KeyboardDriver::Other(service) => service,
    }
}

/// Short name for the `list` table (the plan's UI terms: USB / BT / BLE / PS/2 / I2C).
fn transport_short(transport: Transport) -> &'static str {
    match transport {
        Transport::BluetoothClassic => "BT",
        Transport::BluetoothLe => "BLE",
        other => transport_name(other),
    }
}

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Usb => "USB",
        Transport::BluetoothClassic => "Bluetooth Classic",
        Transport::BluetoothLe => "Bluetooth LE",
        Transport::Ps2 => "PS/2",
        Transport::I2c => "I2C",
        Transport::Spi => "SPI",
        Transport::Virtual => "virtual",
        Transport::Unknown => "unknown",
    }
}

fn pending_name(action: PendingAction) -> &'static str {
    match action {
        PendingAction::ResetKeyboard => "reset keyboard",
        PendingAction::Reconnect => "reconnect",
        PendingAction::RestartPc => "restart PC",
    }
}

pub(crate) fn pending_name_long(action: PendingAction) -> &'static str {
    match action {
        PendingAction::ResetKeyboard => "reset the keyboard (restart the device)",
        PendingAction::Reconnect => {
            "reconnect the keyboard (unplug and replug; Bluetooth: turn it off and on)"
        }
        PendingAction::RestartPc => "restart the PC (Restart, not Shut down)",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn or_dash(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn vid_pid(kb: &KeyboardDevice) -> String {
    match (kb.vendor_id, kb.product_id) {
        (Some(vid), Some(pid)) => format!("{vid:04X}:{pid:04X}"),
        (Some(vid), None) => format!("{vid:04X}:-"),
        _ => "-".to_string(),
    }
}

pub(crate) fn type_text(ty: Option<KeyboardType>) -> String {
    ty.map_or_else(|| "-".to_string(), |ty| ty.to_string())
}

fn reported_text(ka: &KeyboardAssessment) -> String {
    match ka.reported_type {
        Some(ty) => ty.to_string(),
        None if !ka.present => "offline".to_string(),
        None => "unknown".to_string(),
    }
}

pub(crate) fn dword_text(value: u32) -> String {
    if value < 10 {
        value.to_string()
    } else {
        format!("{value} ({value:#X})")
    }
}

fn overrides_text(overrides: &DeviceOverrides) -> String {
    let values = [
        (value_names::HID_TYPE, overrides.keyboard_type_override),
        (
            value_names::HID_SUBTYPE,
            overrides.keyboard_subtype_override,
        ),
        (value_names::PS2_TYPE, overrides.override_keyboard_type),
        (
            value_names::PS2_SUBTYPE,
            overrides.override_keyboard_subtype,
        ),
        (
            value_names::HID_TOTAL_KEYS,
            overrides.number_total_keys_override,
        ),
        (
            value_names::HID_FUNCTION_KEYS,
            overrides.number_function_keys_override,
        ),
        (
            value_names::HID_INDICATORS,
            overrides.number_indicators_override,
        ),
    ];
    let present: Vec<String> = values
        .iter()
        .filter_map(|(name, value)| value.map(|v| format!("{name}={}", dword_text(v))))
        .collect();
    if present.is_empty() {
        "(none)".to_string()
    } else {
        present.join(", ")
    }
}

fn devnode_text(status: Option<u32>, problem: Option<u32>) -> String {
    let Some(status) = status else {
        return "unknown".to_string();
    };
    let mut text = format!("{status:#010X}");
    text.push_str(if status & DN_STARTED != 0 {
        " (started"
    } else {
        " (not started"
    });
    if status & DN_HAS_PROBLEM != 0 || problem.is_some_and(|p| p != 0) {
        write!(text, ", problem code {}", problem.unwrap_or(0)).expect("String write");
    }
    text.push(')');
    text
}

fn os_text(os: &OsInfo) -> String {
    match os.ubr {
        Some(ubr) => format!("{}.{ubr} ({})", os.build, os.native_arch),
        None => format!("{} ({})", os.build, os.native_arch),
    }
}

fn list_text(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

fn input_warning_text(warning: &InputWarning) -> String {
    match warning {
        InputWarning::NonJapaneseLayouts { preload, loaded } => {
            let mut found = preload.clone();
            found.extend(loaded.iter().map(|hkl| format!("{hkl} (loaded)")));
            format!(
                "Non-Japanese input methods are installed: {}. While one of them is active, \
                 every keyboard types with that input method's layout; per-keyboard layouts \
                 work only with the Japanese IME.",
                found.join(", ")
            )
        }
        InputWarning::UserDefaultNotJapanese { klid } => {
            format!("The default input method ({klid}) is not Japanese.")
        }
        InputWarning::SignInNotJapanese { klid } => format!(
            "The sign-in screen's input method is {}, not Japanese: every keyboard types US there.",
            klid.as_deref().unwrap_or("unknown")
        ),
        InputWarning::NoJapaneseLayout => {
            "No Japanese input method is installed: per-keyboard layouts have no effect."
                .to_string()
        }
    }
}

fn global_anomaly_text(anomaly: &GlobalAnomaly) -> String {
    match anomaly {
        GlobalAnomaly::IncompleteFixedType { present, missing } => format!(
            "Only {present} exists; {missing} is missing. Windows still treats this as fixed mode."
        ),
        GlobalAnomaly::FixedTypeMismatch {
            keyboard_type,
            expected,
        } => format!(
            "The global type {keyboard_type} differs from {expected}, which Windows Settings \
             writes for this standard layout."
        ),
        GlobalAnomaly::UnknownLayerDriver { layer_driver } => format!(
            "{} is \"{layer_driver}\", neither kbd106.dll (JIS) nor kbd101.dll (US).",
            value_names::LAYER_DRIVER_JPN
        ),
        GlobalAnomaly::IdentifierMismatch {
            identifier,
            expected,
        } => format!(
            "{} is {}, but {} needs {expected}.",
            value_names::KEYBOARD_IDENTIFIER,
            identifier
                .as_deref()
                .map_or_else(|| "not set".to_string(), |id| format!("\"{id}\"")),
            value_names::LAYER_DRIVER_JPN
        ),
    }
}

fn keyboard_anomaly_text(anomaly: &KeyboardAnomaly) -> String {
    match anomaly {
        KeyboardAnomaly::IncompletePair { present, missing } => {
            format!("{present} exists without {missing}.")
        }
        KeyboardAnomaly::ForeignValueNames { names } => format!(
            "Values for the other driver are ignored by this keyboard's driver: {}.",
            names.join(", ")
        ),
        KeyboardAnomaly::ValuesOnUnsupportedDriver { names } => format!(
            "Override values on a keyboard MKLM does not manage: {}.",
            names.join(", ")
        ),
        KeyboardAnomaly::ValuesOnNonKeyboard { names } => format!(
            "Override values on a device that is not a keyboard: {}.",
            names.join(", ")
        ),
        KeyboardAnomaly::UnverifiedType { keyboard_type } => format!(
            "Stored type {keyboard_type} is not verified per keyboard on real hardware; MKLM \
             writes only 0x4/0x0 (US) and 0x7/0x2 (JIS)."
        ),
        KeyboardAnomaly::UnexpectedType { keyboard_type } => format!(
            "Stored type {keyboard_type} is not one MKLM writes and selects no layout of its own."
        ),
        KeyboardAnomaly::IgnoredInFixedMode { keyboard_type } => format!(
            "Stored type {keyboard_type} is ignored while the PC is in fixed mode \
             (migration to per-keyboard mode needed)."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mklm_core::{INTERNAL_CONTAINER_ID, assess, assess_with};

    fn internal_ps2() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"ACPI\FUJ0309\4&320DB4C2&0".into(),
            display_name: "日本語 PS/2 キーボード (106/109 キー Ctrl+英数)".into(),
            device_description: Some("日本語 PS/2 キーボード (106/109 キー Ctrl+英数)".into()),
            container_id: Some(INTERNAL_CONTAINER_ID.into()),
            is_internal: true,
            present: true,
            driver: KeyboardDriver::I8042prt,
            transport: Transport::Ps2,
            vendor_id: None,
            product_id: None,
            usb_serial: None,
            hardware_ids: vec![r"ACPI\VEN_FUJ&DEV_0309".into(), r"ACPI\FUJ0309".into()],
            parent_chain: vec![r"ACPI\PNP0A08\1".into()],
            overrides: DeviceOverrides {
                override_keyboard_type: Some(7),
                override_keyboard_subtype: Some(2),
                ..Default::default()
            },
            reported_type: Some(KeyboardType::JIS),
            dev_node_status: Some(0x0180_000A),
            problem_code: Some(0),
        }
    }

    fn keychron() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".into(),
            display_name: "Keychron Receiver".into(),
            device_description: Some("HID キーボード デバイス".into()),
            container_id: Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}".into()),
            is_internal: false,
            present: true,
            driver: KeyboardDriver::Kbdhid,
            transport: Transport::Usb,
            vendor_id: Some(0x3434),
            product_id: Some(0xD027),
            usb_serial: Some("B76E483E3F08D96E".into()),
            hardware_ids: vec![r"HID\VID_3434&PID_D027&MI_00&Col01".into()],
            parent_chain: vec![
                r"USB\VID_3434&PID_D027&MI_00\7&295e03ca&0&0000".into(),
                r"USB\VID_3434&PID_D027\B76E483E3F08D96E".into(),
            ],
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                keyboard_subtype_override: Some(0),
                ..Default::default()
            },
            reported_type: Some(KeyboardType::US),
            dev_node_status: Some(0x0180_000A),
            problem_code: Some(0),
        }
    }

    fn vxe_ble() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&0225A7_PID&FA6C_REV&0300_F977E93BA63F&COL02\B&B4852A&0&0001".into(),
            display_name: "VXE R1SE+".into(),
            container_id: Some("{BDA55856-0DCA-5B66-8949-3F281E7B4A28}".into()),
            transport: Transport::BluetoothLe,
            vendor_id: Some(0x25A7),
            product_id: Some(0xFA6C),
            usb_serial: None,
            overrides: DeviceOverrides::default(),
            reported_type: Some(KeyboardType::HID_UNKNOWN),
            ..keychron()
        }
    }

    fn phantom() -> KeyboardDevice {
        KeyboardDevice {
            instance_id: r"HID\{00001812-0000-1000-8000-00805F9B34FB}_DEV_VID&02045E_PID&0040_REV&0300_788712B99B19&COL01\B&1455DB79&0&0000".into(),
            display_name: "X3-5.4 Mouse".into(),
            present: false,
            reported_type: None,
            dev_node_status: None,
            problem_code: None,
            ..vxe_ble()
        }
    }

    fn dev_machine() -> SystemSnapshot {
        SystemSnapshot {
            keyboards: vec![internal_ps2(), keychron(), phantom(), vxe_ble()],
            global: GlobalSettings {
                layer_driver_jpn: Some("kbd106.dll".into()),
                layer_driver_kor: Some("kbd101a.dll".into()),
                override_keyboard_identifier: Some("PCAT_106KEY".into()),
                override_keyboard_type: None,
                override_keyboard_subtype: None,
            },
            input: InputMethods {
                user_preload: vec!["00000411".into(), "00000409".into()],
                sign_in_preload: vec!["00000411".into()],
                loaded_layouts: vec![0x0411_0411, 0x0409_0409],
            },
            os: OsInfo {
                build: 26200,
                ubr: Some(9550),
                native_arch: "x64".into(),
                remote_session: false,
                client_keyboard_type: None,
            },
        }
    }

    fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
        text.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
    }

    #[test]
    fn list_rows() {
        let snapshot = dev_machine();
        let text = list(&snapshot, &assess(&snapshot), true);
        let header = line_with(&text, "After restart");
        assert!(header.trim_start().starts_with("#  Name"));
        // Cells joined by single spaces, so the expectations do not depend on column widths.
        let row = |needle: &str| {
            line_with(&text, needle)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            row("Keychron Receiver"),
            "2 Keychron Receiver kbdhid USB no 3434:D027 0x4/0x0 0x4/0x0 US US -"
        );
        assert_eq!(
            row("日本語"),
            "1 日本語 PS/2 キーボード... i8042prt PS/2 yes - 0x7/0x2 0x7/0x2 JIS JIS -"
        );
        assert_eq!(
            row("VXE"),
            "4 VXE R1SE+ kbdhid BLE no 25A7:FA6C - 0x51/0x0 JIS*? JIS*? -"
        );
        assert!(row("X3-5.4").ends_with(" offline - JIS*? -"));
        assert!(text.contains("  * The PC's standard layout applies"));
        // Following the standard is not typed on real hardware yet.
        assert!(text.contains("  ? This type's layout is not verified on real hardware."));
        assert!(text.contains(r"  [2] HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000"));
        assert!(line_with(&text, "Mode").contains("per-keyboard"));
        assert!(line_with(&text, "Standard layout").ends_with("JIS"));
        assert!(line_with(&text, "Pending ").ends_with("nothing"));
        assert!(
            text.contains(
                "! Non-Japanese input methods are installed: 00000409, 04090409 (loaded)."
            )
        );
        assert!(line_with(&text, "Problems").ends_with("none"));
    }

    #[test]
    fn list_columns_align_with_japanese_names() {
        let snapshot = dev_machine();
        let text = list(&snapshot, &assess(&snapshot), true);
        let lines: Vec<&str> = text.lines().skip(1).take(5).collect();
        let driver_column = |line: &str| {
            let at = line
                .find("kbdhid")
                .or_else(|| line.find("i8042prt"))
                .or_else(|| line.find("Driver"))
                .unwrap();
            crate::table::display_width(&line[..at])
        };
        let first = driver_column(lines[0]);
        assert!(lines.iter().all(|l| driver_column(l) == first), "{text}");
    }

    #[test]
    fn list_flags_problems() {
        let bare_ps2 = KeyboardDevice {
            overrides: DeviceOverrides::default(),
            ..internal_ps2()
        };
        let half_written = KeyboardDevice {
            overrides: DeviceOverrides {
                keyboard_type_override: Some(4),
                ..Default::default()
            },
            ..keychron()
        };
        let snapshot = SystemSnapshot {
            keyboards: vec![bare_ps2, half_written],
            ..dev_machine()
        };
        let text = list(&snapshot, &assess(&snapshot), false);
        assert!(text.starts_with("Keyboards (connected only; --all adds disconnected ones)"));
        assert!(line_with(&text, "日本語").contains("restart PC"));
        assert!(line_with(&text, "Pending ").contains("restart the PC"));
        assert!(text.contains("! INV-PS2 violated"));
        assert!(
            text.contains("! [2] KeyboardTypeOverride exists without KeyboardSubtypeOverride.")
        );
    }

    #[test]
    fn fixed_mode_marks_every_layout() {
        let snapshot = SystemSnapshot {
            global: GlobalSettings {
                override_keyboard_type: Some(7),
                override_keyboard_subtype: Some(2),
                ..dev_machine().global
            },
            ..dev_machine()
        };
        let text = list(&snapshot, &assess(&snapshot), true);
        assert!(line_with(&text, "Keychron").contains("JIS*"));
        assert!(line_with(&text, "Mode").contains("fixed"));
        assert!(text.contains("[2] Stored type 0x4/0x0 is ignored while the PC is in fixed mode"));
    }

    #[test]
    fn status_details() {
        let snapshot = dev_machine();
        let text = status(&snapshot, &assess(&snapshot), true);
        assert!(line_with(&text, "Windows build").ends_with("26200.9550 (x64)"));
        assert!(line_with(&text, "LayerDriver JPN").ends_with("\"kbd106.dll\""));
        assert!(line_with(&text, "OverrideKeyboardType").ends_with("(not set)"));
        assert!(line_with(&text, "Current user").ends_with("00000411, 00000409"));
        assert!(line_with(&text, "Sign-in screen").ends_with("00000411"));
        assert!(line_with(&text, "Loaded in this session").ends_with("04110411, 04090409"));
        assert!(line_with(&text, "INV-PS2").ends_with("holds"));
        assert!(text.contains("  [1] 日本語 PS/2 キーボード (106/109 キー Ctrl+英数)\n"));
        assert!(text.contains("OverrideKeyboardType=7, OverrideKeyboardSubtype=2"));
        assert!(text.contains("KeyboardTypeOverride=4, KeyboardSubtypeOverride=0"));
        assert!(text.contains("US (type 0x4/0x0 selects it)"));
        assert!(text.contains("JIS (standard layout; type 0x51/0x0 selects none of its own)"));
        assert!(text.contains("0x0180000A (started)"));
        assert!(text.contains(r"USB\VID_3434&PID_D027\B76E483E3F08D96E"));
        assert!(line_with(&text, "B76E483E3F08D96E").contains("USB serial"));
        assert!(
            text.contains("  Keychron Receiver  ({F0D991EA-A583-5B9C-800D-48846AC6E633})  [2]")
        );
        assert!(text.contains("({00000000-0000-0000-FFFF-FFFFFFFFFFFF}, built-in)  [1]"));

        let present_only = status(&snapshot, &assess_with(&snapshot, false), false);
        assert!(
            line_with(&present_only, "INV-PS2")
                .contains("unknown, holds (checked on connected keyboards only")
        );
    }

    #[test]
    fn global_status_text() {
        let global = GlobalSettings {
            layer_driver_jpn: Some("kbdnec.dll".into()),
            override_keyboard_identifier: Some("PCAT_106KEY".into()),
            override_keyboard_type: Some(7),
            override_keyboard_subtype: Some(0xD02),
            ..Default::default()
        };
        let input = InputMethods {
            user_preload: vec!["00000409".into()],
            sign_in_preload: vec![],
            loaded_layouts: vec![0x0409_0409],
        };
        let text = global_status(&global, &input);
        assert!(line_with(&text, "OverrideKeyboardSubtype").ends_with("3330 (0xD02)"));
        assert!(line_with(&text, "LayerDriver KOR").ends_with("(not set)"));
        assert!(line_with(&text, "Mode").contains("fixed"));
        assert!(line_with(&text, "Standard layout").ends_with("other (kbdnec.dll)"));
        assert!(text.contains("! LayerDriver JPN is \"kbdnec.dll\""));
        assert!(text.contains("! No Japanese input method is installed"));
        assert!(text.contains("! The sign-in screen's input method is unknown"));
        assert!(line_with(&text, "Sign-in screen").ends_with("(none)"));
    }

    #[test]
    fn empty_system() {
        let snapshot = SystemSnapshot {
            keyboards: vec![],
            global: GlobalSettings::default(),
            input: InputMethods::default(),
            ..dev_machine()
        };
        let assessment = assess(&snapshot);
        assert!(list(&snapshot, &assessment, false).contains("  No keyboards found."));
        let text = status(&snapshot, &assessment, false);
        assert!(text.contains("  No keyboards found."));
        assert!(text.contains("Device groups\n  (none)"));
    }
}
