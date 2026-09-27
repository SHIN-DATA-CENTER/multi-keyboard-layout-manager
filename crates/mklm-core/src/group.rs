//! Grouping of keyboard devnodes into physical devices.

use serde::{Deserialize, Serialize};

use crate::model::KeyboardDevice;

/// Keyboard devnodes that belong to one physical device (one container).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGroup {
    /// Container shared by the keyboards, as reported by the first one. `None` when unknown.
    pub container_id: Option<String>,
    pub is_internal: bool,
    /// Display name of the first keyboard in the group.
    pub display_name: String,
    /// Instance IDs of the keyboards in this group, in snapshot order.
    pub keyboards: Vec<String>,
}

/// Groups keyboards by container ID (case-insensitive), in order of first appearance.
///
/// Every keyboard in the internal container is its own group (the built-in container holds unrelated
/// devices), and so is every keyboard without a usable container ID (missing or `GUID_NULL`).
pub fn group_keyboards(keyboards: &[KeyboardDevice]) -> Vec<DeviceGroup> {
    let mut groups: Vec<DeviceGroup> = Vec::new();
    for kb in keyboards {
        let internal = kb.in_internal_container();
        let existing = match (kb.known_container_id(), internal) {
            (Some(container), false) => groups.iter_mut().find(|g| {
                !g.is_internal
                    && g.container_id
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(container))
            }),
            _ => None,
        };
        match existing {
            Some(group) => group.keyboards.push(kb.instance_id.clone()),
            None => groups.push(DeviceGroup {
                container_id: kb.container_id.clone(),
                is_internal: internal,
                display_name: kb.display_name.clone(),
                keyboards: vec![kb.instance_id.clone()],
            }),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn dev_machine_groups() {
        let groups = group_keyboards(&fixtures::dev_machine().keyboards);
        assert_eq!(groups.len(), 4);
        assert!(groups[0].is_internal);
        assert_eq!(groups[1].display_name, "Keychron Receiver");
        assert_eq!(groups[1].keyboards.len(), 1);
    }

    #[test]
    fn same_container_is_one_group_but_internal_never_merges() {
        let second_collection = KeyboardDevice {
            instance_id: r"HID\VID_3434&PID_D027&MI_02&COL01\8&2&0&0000".into(),
            container_id: Some("{f0d991ea-a583-5b9c-800d-48846ac6e633}".into()),
            ..fixtures::keychron()
        };
        let second_internal = KeyboardDevice {
            instance_id: r"HID\ELAN0001&Col01\5&1&0&0000".into(),
            ..fixtures::internal_ps2()
        };
        let no_container = KeyboardDevice {
            instance_id: r"HID\VID_1111&PID_2222\1".into(),
            container_id: None,
            ..fixtures::keychron()
        };
        let null_container = KeyboardDevice {
            instance_id: r"HID\VID_1111&PID_2222\3".into(),
            container_id: Some(crate::safety::NULL_CONTAINER_ID.into()),
            ..fixtures::keychron()
        };
        let keyboards = vec![
            fixtures::internal_ps2(),
            fixtures::keychron(),
            second_internal,
            second_collection,
            no_container.clone(),
            KeyboardDevice {
                instance_id: r"HID\VID_1111&PID_2222\2".into(),
                ..no_container
            },
            null_container.clone(),
            KeyboardDevice {
                instance_id: r"HID\VID_1111&PID_2222\4".into(),
                ..null_container
            },
        ];
        let groups = group_keyboards(&keyboards);
        assert_eq!(groups.len(), 7);
        assert_eq!(
            groups[1].keyboards,
            vec![
                keyboards[1].instance_id.clone(),
                keyboards[3].instance_id.clone()
            ]
        );
        assert!(groups[0].is_internal && groups[2].is_internal);
        assert_eq!(groups[3].container_id, None);
        assert_eq!(groups[4].keyboards.len(), 1);
    }
}
