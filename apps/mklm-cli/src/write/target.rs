//! Which keyboard a command means (design F.1): `#n` from `mklm-cli list`, or an instance ID.

use mklm_core::KeyboardDevice;

use super::KeyboardRef;

/// A keyboard reference resolved against one enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeyboard {
    /// The canonical instance ID, as enumerated.
    pub instance_id: String,
    pub display_name: String,
    /// The `#n` it was given as, if any.
    pub row: Option<usize>,
    pub present: bool,
}

impl ResolvedKeyboard {
    /// `Keyboard #2: Keychron Receiver` / `Keyboard: …`, then the instance ID on its own line (the
    /// CLI always echoes both before it goes on, design F.1).
    pub fn describe(&self) -> String {
        let row = self.row.map(|n| format!(" #{n}")).unwrap_or_default();
        let state = if self.present { "" } else { " (not connected)" };
        format!(
            "Keyboard{row}: {}{state}\n  {}\n",
            self.display_name, self.instance_id
        )
    }
}

/// Resolves `reference` against `keyboards`, which must be the complete enumeration (phantoms
/// included) in `mklm_win::read_keyboards` order: `#n` is the n-th **connected** keyboard, exactly
/// as `mklm-cli list` numbers them; an instance ID matches case-insensitively and may name a
/// keyboard that is not connected.
pub fn resolve_keyboard(
    keyboards: &[KeyboardDevice],
    reference: &KeyboardRef,
) -> Result<ResolvedKeyboard, String> {
    let (kb, row) = match reference {
        KeyboardRef::ListRow(n) => {
            let connected: Vec<&KeyboardDevice> =
                keyboards.iter().filter(|kb| kb.present).collect();
            let kb = n
                .checked_sub(1)
                .and_then(|index| connected.get(index))
                .ok_or_else(|| {
                    format!(
                        "there is no keyboard #{n}; `mklm-cli list` shows {} connected keyboard(s)",
                        connected.len()
                    )
                })?;
            (*kb, Some(*n))
        }
        KeyboardRef::InstanceId(id) => {
            let kb = keyboards
                .iter()
                .find(|kb| kb.instance_id.eq_ignore_ascii_case(id.trim()))
                .ok_or_else(|| {
                    format!("no keyboard has the instance ID {id}; see `mklm-cli list --all`")
                })?;
            (kb, None)
        }
    };
    Ok(ResolvedKeyboard {
        instance_id: kb.instance_id.clone(),
        display_name: kb.display_name.clone(),
        row,
        present: kb.present,
    })
}

/// Design F.1 / review S12: `--yes` together with `#n` is refused, because the numbering can change
/// between `list` and the command (a keyboard plugged in or out), and nobody would see which
/// keyboard `#n` became. A usage error (exit code 2).
pub fn check_yes_with_rows<'a>(
    yes: bool,
    references: impl IntoIterator<Item = &'a KeyboardRef>,
) -> Result<(), String> {
    if !yes {
        return Ok(());
    }
    let rows: Vec<String> = references
        .into_iter()
        .filter_map(|reference| match reference {
            KeyboardRef::ListRow(n) => Some(format!("#{n}")),
            KeyboardRef::InstanceId(_) => None,
        })
        .collect();
    if rows.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "--yes needs instance IDs, not {}: the numbers of `mklm-cli list` can change when a \
             keyboard is plugged in or out. Use the instance ID `mklm-cli list` prints.",
            rows.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use mklm_core::fixtures;

    use super::*;

    fn keyboards() -> Vec<KeyboardDevice> {
        // `read_keyboards` order: sorted by upper-case instance ID, phantoms included.
        let mut keyboards = fixtures::dev_machine().keyboards;
        keyboards.sort_by_cached_key(|kb| kb.instance_id.to_ascii_uppercase());
        keyboards
    }

    #[test]
    fn rows_count_connected_keyboards_in_list_order() {
        let keyboards = keyboards();
        let connected: Vec<&str> = keyboards
            .iter()
            .filter(|kb| kb.present)
            .map(|kb| kb.instance_id.as_str())
            .collect();
        assert_eq!(connected.len(), 3);
        for (index, id) in connected.iter().enumerate() {
            let resolved = resolve_keyboard(&keyboards, &KeyboardRef::ListRow(index + 1)).unwrap();
            assert_eq!(resolved.instance_id, *id);
            assert_eq!(resolved.row, Some(index + 1));
            assert!(resolved.present);
        }
        // #2 on the development machine is the Keychron receiver, as `mklm-cli list` shows.
        let keychron = resolve_keyboard(&keyboards, &KeyboardRef::ListRow(2)).unwrap();
        assert_eq!(keychron.instance_id, fixtures::keychron().instance_id);
        assert_eq!(keychron.display_name, "Keychron Receiver");
        assert!(
            keychron
                .describe()
                .starts_with("Keyboard #2: Keychron Receiver\n  HID\\")
        );

        let error = resolve_keyboard(&keyboards, &KeyboardRef::ListRow(4)).unwrap_err();
        assert!(error.contains("no keyboard #4"), "{error}");
        assert!(error.contains("3 connected"), "{error}");
        assert!(resolve_keyboard(&keyboards, &KeyboardRef::ListRow(0)).is_err());
    }

    #[test]
    fn instance_ids_match_any_case_and_phantoms() {
        let keyboards = keyboards();
        let lower = fixtures::keychron().instance_id.to_ascii_lowercase();
        let resolved = resolve_keyboard(&keyboards, &KeyboardRef::InstanceId(lower)).unwrap();
        assert_eq!(resolved.instance_id, fixtures::keychron().instance_id);
        assert_eq!(resolved.row, None);

        let phantom = fixtures::ms_ble_phantom();
        let resolved = resolve_keyboard(
            &keyboards,
            &KeyboardRef::InstanceId(phantom.instance_id.clone()),
        )
        .unwrap();
        assert!(!resolved.present);
        assert!(resolved.describe().contains("(not connected)"));

        let unknown = KeyboardRef::InstanceId(r"HID\VID_0000&PID_0000\1".into());
        assert!(resolve_keyboard(&keyboards, &unknown).is_err());
    }

    #[test]
    fn yes_refuses_list_rows() {
        let id = KeyboardRef::InstanceId(fixtures::keychron().instance_id);
        let row = KeyboardRef::ListRow(2);
        assert_eq!(check_yes_with_rows(false, [&row]), Ok(()));
        assert_eq!(check_yes_with_rows(true, [&id]), Ok(()));
        assert_eq!(check_yes_with_rows(true, []), Ok(()));
        let error = check_yes_with_rows(true, [&id, &row]).unwrap_err();
        assert!(error.contains("#2"), "{error}");
    }
}
