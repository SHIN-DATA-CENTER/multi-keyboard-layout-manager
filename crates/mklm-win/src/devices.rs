//! Keyboard-class devnodes and everything MKLM needs to know about each of them.

use mklm_core::{
    DeviceOverrides, INTERNAL_CONTAINER_ID, KeyboardAnomaly, KeyboardDevice, KeyboardDriver,
    classify_keyboard_transport, non_keyboard_anomaly, usb_serial_from_chain,
    vendor_product_from_ids,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT,
    CR_NO_SUCH_DEVNODE,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_ClassGuid, DEVPKEY_Device_ContainerId,
    DEVPKEY_Device_DevNodeStatus, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName,
    DEVPKEY_Device_HardwareIds, DEVPKEY_Device_InstanceId, DEVPKEY_Device_IsPresent,
    DEVPKEY_Device_Parent, DEVPKEY_Device_ProblemCode, DEVPKEY_Device_Service,
};

use crate::error::{Error, ReadIssue, ReadIssueKind};
use crate::hwkey::read_overrides;
use crate::props::{DevNode, device_id_list};
use crate::rawinfo::RawKeyboard;

/// Setup class of keyboards (`GUID_DEVCLASS_KEYBOARD`).
pub const KEYBOARD_CLASS_GUID: &str = "{4d36e96b-e325-11ce-bfc1-08002be10318}";

/// Safety net against parent loops; real chains are about a dozen levels deep.
const MAX_ANCESTORS: usize = 32;

/// Instance IDs of the Keyboard-class devnodes, optionally including non-present (phantom) ones.
pub fn keyboard_instance_ids(include_non_present: bool) -> Result<Vec<String>, Error> {
    let mut flags = CM_GETIDLIST_FILTER_CLASS;
    if !include_non_present {
        flags |= CM_GETIDLIST_FILTER_PRESENT;
    }
    device_id_list(KEYBOARD_CLASS_GUID, flags)
}

/// Reads every Keyboard-class devnode, sorted by instance ID (case-insensitive).
///
/// `raw` supplies `reported_type` for present devices (see [`crate::raw_keyboards`]). A devnode
/// that cannot be located is skipped with an issue, unless it simply went away meanwhile.
pub fn read_keyboards(
    include_non_present: bool,
    raw: &[RawKeyboard],
    issues: &mut Vec<ReadIssue>,
) -> Result<Vec<KeyboardDevice>, Error> {
    let mut keyboards: Vec<KeyboardDevice> = keyboard_instance_ids(include_non_present)?
        .iter()
        .filter_map(|id| read_keyboard(id, raw, issues))
        .filter(|kb| include_non_present || kb.present)
        .collect();
    keyboards.sort_by_cached_key(|kb| kb.instance_id.to_ascii_uppercase());
    keyboards.dedup_by(|a, b| a.instance_id.eq_ignore_ascii_case(&b.instance_id));
    Ok(keyboards)
}

/// Reads one keyboard devnode. `None` when it cannot be located (with an issue, unless it simply
/// does not exist).
///
/// `instance_id` is matched case-insensitively; the result carries the canonical form
/// (`DEVPKEY_Device_InstanceId`, upper case as `Get-PnpDevice` prints it). Missing properties
/// are `None`; a devnode without a service gets `KeyboardDriver::Other("")` (read-only for MKLM).
/// A property that exists but cannot be read is also `None`, with an issue. If that property is
/// the service of an `ACPI\` keyboard, the driver is taken to be i8042prt, so that INV-PS2 and the
/// PS/2 reset ban still cover it (HID keyboards are always `HID\` collections).
pub fn read_keyboard(
    instance_id: &str,
    raw: &[RawKeyboard],
    issues: &mut Vec<ReadIssue>,
) -> Option<KeyboardDevice> {
    let node = match DevNode::locate(instance_id) {
        Ok(node) => node,
        Err(error) => {
            if error.config_ret() != Some(CR_NO_SUCH_DEVNODE.0) {
                issues.push(ReadIssue::new(ReadIssueKind::Locate, instance_id, error));
            }
            return None;
        }
    };
    let canonical = or_issue(
        ReadIssueKind::Identity,
        node.string(&DEVPKEY_Device_InstanceId),
        instance_id,
        "DEVPKEY_Device_InstanceId",
        issues,
    );
    if let Some(found) = canonical
        .as_deref()
        .filter(|found| !found.eq_ignore_ascii_case(instance_id))
    {
        issues.push(ReadIssue::new(
            ReadIssueKind::Identity,
            instance_id,
            Error::OtherDevNode {
                found: found.to_string(),
            },
        ));
        return None;
    }
    let instance_id = canonical.as_deref().unwrap_or(instance_id);

    let dev_node_status = or_issue(
        ReadIssueKind::Status,
        node.u32(&DEVPKEY_Device_DevNodeStatus),
        instance_id,
        "DEVPKEY_Device_DevNodeStatus",
        issues,
    );
    let problem_code = or_issue(
        ReadIssueKind::Status,
        node.u32(&DEVPKEY_Device_ProblemCode),
        instance_id,
        "DEVPKEY_Device_ProblemCode",
        issues,
    );
    let present = or_issue(
        ReadIssueKind::Presence,
        node.bool(&DEVPKEY_Device_IsPresent),
        instance_id,
        "DEVPKEY_Device_IsPresent",
        issues,
    )
    .unwrap_or(dev_node_status.is_some());
    let container_id = or_issue(
        ReadIssueKind::Container,
        node.guid(&DEVPKEY_Device_ContainerId),
        instance_id,
        "DEVPKEY_Device_ContainerId",
        issues,
    );
    let is_internal = container_id
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case(INTERNAL_CONTAINER_ID));
    let driver = match node.string(&DEVPKEY_Device_Service) {
        Ok(Some(service)) => KeyboardDriver::from_service(&service),
        Ok(None) => KeyboardDriver::Other(String::new()),
        Err(error) => {
            issues.push(property_issue(
                ReadIssueKind::Driver,
                instance_id,
                "DEVPKEY_Device_Service",
                error,
            ));
            driver_without_service(instance_id)
        }
    };
    let hardware_ids = or_issue(
        ReadIssueKind::Descriptive,
        node.strings(&DEVPKEY_Device_HardwareIds),
        instance_id,
        "DEVPKEY_Device_HardwareIds",
        issues,
    );
    let own_name = read_name(node, instance_id, NameSource::FriendlyName, issues)
        .or_else(|| read_name(node, instance_id, NameSource::DeviceDesc, issues));

    let ancestors = walk_ancestors(
        node,
        instance_id,
        container_id.as_deref(),
        !is_internal,
        issues,
    );
    let display_name = (!is_internal)
        .then(|| ancestors.names.iter().rev().flatten().next().cloned())
        .flatten()
        .or_else(|| own_name.clone())
        .or_else(|| read_name(node, instance_id, NameSource::BusReported, issues))
        .unwrap_or_else(|| instance_id.to_string());

    let vendor_product = vendor_product_from_ids(instance_id, &hardware_ids);
    // The Remote Desktop keyboard is virtual whatever enumerates or serves it (plan 3.2).
    let transport = classify_keyboard_transport(
        instance_id,
        &hardware_ids,
        &ancestors.chain,
        &driver,
        ancestors.bus_service.as_deref(),
    );
    let reported_type = present
        .then(|| {
            raw.iter()
                .find(|r| {
                    r.instance_id
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(instance_id))
                })
                .map(|r| r.keyboard_type)
        })
        .flatten();

    Some(KeyboardDevice {
        instance_id: instance_id.to_string(),
        display_name,
        device_description: own_name,
        container_id,
        is_internal,
        present,
        overrides: read_overrides(node, instance_id, issues),
        driver,
        transport,
        vendor_id: vendor_product.map(|vp| vp.vendor_id),
        product_id: vendor_product.map(|vp| vp.product_id),
        usb_serial: usb_serial_from_chain(&ancestors.chain),
        hardware_ids,
        parent_chain: ancestors.chain,
        reported_type,
        dev_node_status,
        problem_code,
    })
}

/// Enumerator of the HID collections (`HID\…`): each function of a HID device (keyboard, mouse,
/// consumer control, vendor pages) is one of them.
const HID_ENUMERATOR: &str = "HID";

/// Override values on a HID collection outside the Keyboard class that belongs to the same
/// physical device as a keyboard (plan 3.1 step 2, "同じデバイスのキーボード以外のコレクション";
/// design m3 B.1, J.9): for example `KeyboardTypeOverride` written by hand on the mouse
/// collection (`…&Col03`) of the Keychron receiver. No driver reads them there. MKLM never writes
/// or removes them (plan 1.5); the wizard only explains how to remove them by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonKeyboardValues {
    /// Canonical instance ID of the collection.
    pub instance_id: String,
    /// The keyboard whose device it belongs to (the first match in the order of `keyboards`).
    pub keyboard_instance_id: String,
    /// `DEVPKEY_Device_Service` (e.g. `mouhid`), for the explanation; `None` without one.
    pub service: Option<String>,
    /// The collection is connected.
    pub present: bool,
    /// The values found; at least one is set.
    pub overrides: DeviceOverrides,
}

impl NonKeyboardValues {
    /// The anomaly to show: [`KeyboardAnomaly::ValuesOnNonKeyboard`] with the value names.
    pub fn anomaly(&self) -> Option<KeyboardAnomaly> {
        non_keyboard_anomaly(&self.overrides)
    }
}

/// Finds the [`NonKeyboardValues`] of `keyboards` (read only, for the wizard): every `HID\`
/// devnode, connected or not, outside the Keyboard setup class, that has at least one of the
/// seven override values and belongs to the device of one of the keyboards: it shares the
/// keyboard's container (unless that is the internal container, which holds every built-in
/// device), or its parent (the other collections of the same HID device; this is what groups a
/// built-in HID keyboard's collections). Sorted by instance ID.
///
/// Fails only when the HID devnodes cannot be listed. A devnode or value that cannot be read is
/// skipped with an issue; none of them blocks a write (this decides nothing but a hint).
pub fn read_non_keyboard_values(
    keyboards: &[KeyboardDevice],
    issues: &mut Vec<ReadIssue>,
) -> Result<Vec<NonKeyboardValues>, Error> {
    let owners: Vec<Owner> = keyboards
        .iter()
        .map(|keyboard| Owner {
            instance_id: keyboard.instance_id.clone(),
            container: keyboard
                .container_id
                .clone()
                .filter(|container| !container.eq_ignore_ascii_case(INTERNAL_CONTAINER_ID)),
            parent: direct_parent(&keyboard.instance_id, issues),
        })
        .collect();
    if owners.is_empty() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for id in device_id_list(HID_ENUMERATOR, CM_GETIDLIST_FILTER_ENUMERATOR)? {
        if owners
            .iter()
            .any(|owner| owner.instance_id.eq_ignore_ascii_case(&id))
        {
            continue;
        }
        if let Some(values) = non_keyboard_values(&id, &owners, issues) {
            found.push(values);
        }
    }
    found.sort_by_cached_key(|values| values.instance_id.to_ascii_uppercase());
    found.dedup_by(|a, b| a.instance_id.eq_ignore_ascii_case(&b.instance_id));
    Ok(found)
}

/// A keyboard as [`read_non_keyboard_values`] matches collections against it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    instance_id: String,
    /// The keyboard's container, `None` for the internal container or none.
    container: Option<String>,
    /// `DEVPKEY_Device_Parent` of the keyboard.
    parent: Option<String>,
}

impl Owner {
    /// True when a devnode with `container` and `parent` belongs to this keyboard's device.
    fn owns(&self, container: Option<&str>, parent: Option<&str>) -> bool {
        let same = |a: Option<&str>, b: Option<&str>| matches!((a, b), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
        same(self.container.as_deref(), container) || same(self.parent.as_deref(), parent)
    }
}

/// The parent of `instance_id`'s devnode; `None` when it has none or cannot be read (with an
/// issue).
fn direct_parent(instance_id: &str, issues: &mut Vec<ReadIssue>) -> Option<String> {
    let node = match DevNode::locate(instance_id) {
        Ok(node) => node,
        Err(error) => {
            if error.config_ret() != Some(CR_NO_SUCH_DEVNODE.0) {
                issues.push(ReadIssue::new(
                    ReadIssueKind::Descriptive,
                    instance_id,
                    error,
                ));
            }
            return None;
        }
    };
    or_issue(
        ReadIssueKind::Descriptive,
        node.string(&DEVPKEY_Device_Parent),
        instance_id,
        "DEVPKEY_Device_Parent",
        issues,
    )
}

/// The values of HID devnode `id` when it is outside the Keyboard class, belongs to one of
/// `owners` and has any.
fn non_keyboard_values(
    id: &str,
    owners: &[Owner],
    issues: &mut Vec<ReadIssue>,
) -> Option<NonKeyboardValues> {
    let node = match DevNode::locate(id) {
        Ok(node) => node,
        Err(error) => {
            if error.config_ret() != Some(CR_NO_SUCH_DEVNODE.0) {
                issues.push(ReadIssue::new(ReadIssueKind::Descriptive, id, error));
            }
            return None;
        }
    };
    // A class that cannot be read might be the Keyboard class: skip rather than guess. A
    // collection without a class (no driver installed) is not a keyboard.
    let class = match node.guid(&DEVPKEY_Device_ClassGuid) {
        Ok(class) => class,
        Err(error) => {
            issues.push(property_issue(
                ReadIssueKind::Descriptive,
                id,
                "DEVPKEY_Device_ClassGuid",
                error,
            ));
            return None;
        }
    };
    if class
        .as_deref()
        .is_some_and(|class| class.eq_ignore_ascii_case(KEYBOARD_CLASS_GUID))
    {
        return None;
    }
    let container = or_issue(
        ReadIssueKind::Descriptive,
        node.guid(&DEVPKEY_Device_ContainerId),
        id,
        "DEVPKEY_Device_ContainerId",
        issues,
    );
    let parent = or_issue(
        ReadIssueKind::Descriptive,
        node.string(&DEVPKEY_Device_Parent),
        id,
        "DEVPKEY_Device_Parent",
        issues,
    );
    let owner = owners
        .iter()
        .find(|owner| owner.owns(container.as_deref(), parent.as_deref()))?;
    let instance_id = or_issue(
        ReadIssueKind::Descriptive,
        node.string(&DEVPKEY_Device_InstanceId),
        id,
        "DEVPKEY_Device_InstanceId",
        issues,
    )
    .unwrap_or_else(|| id.to_string());
    let overrides = read_overrides(node, &instance_id, issues);
    if overrides.present_value_names().is_empty() {
        return None;
    }
    let present = or_issue(
        ReadIssueKind::Descriptive,
        node.bool(&DEVPKEY_Device_IsPresent),
        &instance_id,
        "DEVPKEY_Device_IsPresent",
        issues,
    )
    .unwrap_or(false);
    let service = or_issue(
        ReadIssueKind::Descriptive,
        node.string(&DEVPKEY_Device_Service),
        &instance_id,
        "DEVPKEY_Device_Service",
        issues,
    );
    Some(NonKeyboardValues {
        instance_id,
        keyboard_instance_id: owner.instance_id.clone(),
        service,
        present,
        overrides,
    })
}

/// Driver assumed for a Keyboard-class devnode whose service cannot be read: i8042prt for an
/// `ACPI\` devnode, which can only be a PS/2 keyboard (HID keyboards are `HID\` collections), so
/// that INV-PS2 and the PS/2 reset ban still cover it; otherwise an unknown driver.
fn driver_without_service(instance_id: &str) -> KeyboardDriver {
    if enumerator(instance_id).eq_ignore_ascii_case("ACPI") {
        KeyboardDriver::I8042prt
    } else {
        KeyboardDriver::Other(String::new())
    }
}

/// Issue for a property of `instance_id` that exists but could not be read.
fn property_issue(
    kind: ReadIssueKind,
    instance_id: &str,
    property: &str,
    error: Error,
) -> ReadIssue {
    ReadIssue::new(kind, format!("{instance_id} ({property})"), error)
}

/// The value of a property read, or its default (missing) after recording the failure as an issue.
fn or_issue<T: Default>(
    kind: ReadIssueKind,
    result: Result<T, Error>,
    instance_id: &str,
    property: &str,
    issues: &mut Vec<ReadIssue>,
) -> T {
    result.unwrap_or_else(|error| {
        issues.push(property_issue(kind, instance_id, property, error));
        T::default()
    })
}

/// Name properties a display name can come from.
#[derive(Debug, Clone, Copy)]
enum NameSource {
    FriendlyName,
    DeviceDesc,
    BusReported,
}

fn read_name(
    node: DevNode,
    instance_id: &str,
    source: NameSource,
    issues: &mut Vec<ReadIssue>,
) -> Option<String> {
    let (key, property) = match source {
        NameSource::FriendlyName => (&DEVPKEY_Device_FriendlyName, "DEVPKEY_Device_FriendlyName"),
        NameSource::DeviceDesc => (&DEVPKEY_Device_DeviceDesc, "DEVPKEY_Device_DeviceDesc"),
        NameSource::BusReported => (
            &DEVPKEY_Device_BusReportedDeviceDesc,
            "DEVPKEY_Device_BusReportedDeviceDesc",
        ),
    };
    or_issue(
        ReadIssueKind::Descriptive,
        node.string(key),
        instance_id,
        property,
        issues,
    )
}

/// Same-container ancestors of a devnode.
#[derive(Debug, Default)]
struct Ancestors {
    /// Canonical instance IDs, nearest parent first.
    chain: Vec<String>,
    /// Display-name candidate of each `chain` entry (only collected when asked for).
    names: Vec<Option<String>>,
    /// Service of the nearest non-`HID\` ancestor (the HID transport minidriver), even when that
    /// ancestor is outside the container.
    bus_service: Option<String>,
}

/// Walks `DEVPKEY_Device_Parent` upwards (non-present parents included) while the container
/// stays the same. A parent that cannot be read or located ends the walk with an issue.
fn walk_ancestors(
    node: DevNode,
    instance_id: &str,
    container: Option<&str>,
    with_names: bool,
    issues: &mut Vec<ReadIssue>,
) -> Ancestors {
    let mut out = Ancestors::default();
    let mut bus_checked = false;
    let mut current = node;
    let mut current_id = instance_id.to_string();
    for _ in 0..MAX_ANCESTORS {
        let parent_id = match current.string(&DEVPKEY_Device_Parent) {
            Ok(Some(parent_id)) => parent_id,
            // Only the root of the device tree has no parent.
            Ok(None) => break,
            Err(error) => {
                issues.push(property_issue(
                    ReadIssueKind::Topology,
                    &current_id,
                    "DEVPKEY_Device_Parent",
                    error,
                ));
                break;
            }
        };
        let parent = match DevNode::locate(&parent_id) {
            Ok(parent) => parent,
            Err(error) => {
                issues.push(ReadIssue::new(ReadIssueKind::Topology, parent_id, error));
                break;
            }
        };
        // `DEVPKEY_Device_Parent` keeps whatever case the bus driver used; store the canonical ID.
        let parent_id = or_issue(
            ReadIssueKind::Topology,
            parent.string(&DEVPKEY_Device_InstanceId),
            &parent_id,
            "DEVPKEY_Device_InstanceId",
            issues,
        )
        .unwrap_or(parent_id);
        if !bus_checked && !is_hid_enumerated(&parent_id) {
            bus_checked = true;
            out.bus_service = or_issue(
                ReadIssueKind::Topology,
                parent.string(&DEVPKEY_Device_Service),
                &parent_id,
                "DEVPKEY_Device_Service",
                issues,
            );
        }
        let parent_container = or_issue(
            ReadIssueKind::Topology,
            parent.guid(&DEVPKEY_Device_ContainerId),
            &parent_id,
            "DEVPKEY_Device_ContainerId",
            issues,
        );
        if !same_container(container, parent_container.as_deref())
            || out
                .chain
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&parent_id))
        {
            break;
        }
        if with_names {
            out.names.push(ancestor_name(parent, &parent_id, issues));
        }
        out.chain.push(parent_id.clone());
        current = parent;
        current_id = parent_id;
    }
    out
}

/// Name an ancestor contributes to `display_name`. Bluetooth devnodes carry the user-visible
/// name (e.g. "VXE R1SE+") as FriendlyName and a generic bus-reported description; USB devices
/// carry the product string (e.g. "Keychron Receiver") as the bus-reported description.
fn ancestor_name(node: DevNode, instance_id: &str, issues: &mut Vec<ReadIssue>) -> Option<String> {
    let (first, second) = if is_bluetooth_enumerated(instance_id) {
        (NameSource::FriendlyName, NameSource::BusReported)
    } else {
        (NameSource::BusReported, NameSource::FriendlyName)
    };
    read_name(node, instance_id, first, issues)
        .or_else(|| read_name(node, instance_id, second, issues))
}

fn enumerator(instance_id: &str) -> &str {
    instance_id.split('\\').next().unwrap_or("")
}

fn is_hid_enumerated(instance_id: &str) -> bool {
    enumerator(instance_id).eq_ignore_ascii_case("HID")
}

fn is_bluetooth_enumerated(instance_id: &str) -> bool {
    let enumerator = enumerator(instance_id).to_ascii_uppercase();
    enumerator.starts_with("BTH")
}

/// Container IDs compare case-insensitively; two devnodes without one count as the same.
fn same_container(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_compare_case_insensitively() {
        assert!(same_container(
            Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}"),
            Some("{f0d991ea-a583-5b9c-800d-48846ac6e633}")
        ));
        assert!(!same_container(
            Some("{F0D991EA-A583-5B9C-800D-48846AC6E633}"),
            Some(INTERNAL_CONTAINER_ID)
        ));
        assert!(!same_container(Some(INTERNAL_CONTAINER_ID), None));
        assert!(same_container(None, None));
    }

    #[test]
    fn enumerators() {
        assert!(is_hid_enumerated(
            r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000"
        ));
        assert!(!is_hid_enumerated(
            r"USB\VID_3434&PID_D027\B76E483E3F08D96E"
        ));
        assert!(is_bluetooth_enumerated(
            r"BTHLE\Dev_f977e93ba63f\9&20c32106&0&f977e93ba63f"
        ));
        assert!(is_bluetooth_enumerated(
            r"BTHLEDevice\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&0225a7\a&1"
        ));
        assert!(is_bluetooth_enumerated(
            r"BTHENUM\{00001124-0000-1000-8000-00805f9b34fb}\8&1"
        ));
        assert!(!is_bluetooth_enumerated(
            r"USB\VID_8087&PID_0029\7&19311e50&0&1"
        ));
        assert_eq!(enumerator(""), "");
    }

    #[test]
    fn collections_belong_by_container_or_parent() {
        let receiver = "{F0D991EA-A583-5B9C-800D-48846AC6E633}";
        let keychron = Owner {
            instance_id: r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".to_string(),
            container: Some(receiver.to_string()),
            parent: Some(r"USB\VID_3434&PID_D027&MI_00\7&1B9C5C0B&0&0000".to_string()),
        };
        // The receiver's mouse collection (Col03) and its vendor interface (MI_01).
        assert!(keychron.owns(
            Some(&receiver.to_ascii_lowercase()),
            Some(r"usb\vid_3434&pid_d027&mi_00\7&1b9c5c0b&0&0000")
        ));
        assert!(keychron.owns(
            Some(receiver),
            Some(r"USB\VID_3434&PID_D027&MI_01\7&1B9C5C0B&0&0001")
        ));
        // Another device, or nothing known.
        assert!(!keychron.owns(
            Some("{0B7E5F8A-1C2D-4E3F-8A9B-0C1D2E3F4A5B}"),
            Some(r"USB\VID_046D&PID_C52B\6&1")
        ));
        assert!(!keychron.owns(None, None));
        // A built-in HID keyboard: the internal container is not one device, but the other
        // collections of the same HID device share its parent.
        let builtin = Owner {
            instance_id: r"HID\VEN_ELAN&DEV_0001&COL01\5&1".to_string(),
            container: None,
            parent: Some(r"ACPI\ELAN0001\1".to_string()),
        };
        assert!(builtin.owns(Some(INTERNAL_CONTAINER_ID), Some(r"acpi\elan0001\1")));
        assert!(!builtin.owns(Some(INTERNAL_CONTAINER_ID), Some(r"ACPI\SYNA0002\1")));
    }

    /// Read only, on this machine: the scan runs, and whatever it finds is a `HID\` devnode that
    /// is not one of the keyboards and has values.
    #[test]
    fn non_keyboard_values_on_this_machine() {
        let mut issues = Vec::new();
        let keyboards = read_keyboards(true, &[], &mut issues).expect("keyboards");
        let found = read_non_keyboard_values(&keyboards, &mut issues).expect("scan");
        for values in &found {
            assert!(values.anomaly().is_some(), "{values:?}");
            assert!(
                !keyboards.iter().any(|keyboard| keyboard
                    .instance_id
                    .eq_ignore_ascii_case(&values.instance_id)),
                "{values:?}"
            );
            assert!(is_hid_enumerated(&values.instance_id), "{values:?}");
        }
        assert_eq!(read_non_keyboard_values(&[], &mut issues), Ok(Vec::new()));
    }

    #[test]
    fn unreadable_service_keeps_ps2_under_inv_ps2() {
        assert_eq!(
            driver_without_service(r"ACPI\FUJ0309\4&320DB4C2&0"),
            KeyboardDriver::I8042prt
        );
        assert_eq!(
            driver_without_service(r"acpi\PNP0303\4&1&0"),
            KeyboardDriver::I8042prt
        );
        assert_eq!(
            driver_without_service(r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000"),
            KeyboardDriver::Other(String::new())
        );
    }
}
