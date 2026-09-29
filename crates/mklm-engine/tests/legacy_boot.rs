//! The boot-ID bug of 0.1.0 (docs/research/boot-id.md): the journal it left on a desktop PC,
//! whose loader GUID never changed across full restarts, read by the fixed engine. The boot ID
//! is now the counter form of `KUSER_SHARED_DATA.BootId`; the entries 0.1.0 wrote carry the GUID
//! and are judged by their history's boot times (`mklm_core::boot`).

mod common;

use common::*;
use mklm_core::fixtures::{
    self, LEGACY_PC_BOOT_TIME_AFTER_RESTART, LEGACY_PC_BOOT_TIME_OF_WRITES, LEGACY_PC_GUID,
    LEGACY_PENDING_OP, LEGACY_REVERTED_OP,
};
use mklm_core::{
    BootId, DeviceOverrides, GlobalSettings, KeyboardDevice, KeyboardType, Layout, LayoutChoice,
    OpId, OpState, Outcome, RegValue, WriteTarget,
};
use mklm_engine::EngineError;
use mklm_engine::memory::ScriptedSink;

/// The PS/2 keyboard devnode of the desktop PC.
const DESKTOP_PS2: &str = r"ACPI\PNP0303\0";
/// The USB keyboard of the desktop PC (Holtek, kbdhid).
const DESKTOP_USB: &str = r"HID\VID_04D9&PID_1818&MI_00\7&183DDD3D&0&0000";

fn pending_op() -> OpId {
    OpId::parse(LEGACY_PENDING_OP).unwrap()
}

fn reverted_op() -> OpId {
    OpId::parse(LEGACY_REVERTED_OP).unwrap()
}

/// The desktop PC as `d724c149` left it: both keyboards pinned to US (4/0), the global
/// type/subtype deleted, the global strings put back by the revert of `c10d2d38`.
fn desktop_pc() -> World {
    let mut ps2 = fixtures::internal_ps2();
    ps2.instance_id = DESKTOP_PS2.to_string();
    ps2.display_name = "標準 PS/2 キーボード".to_string();
    ps2.hardware_ids = vec![r"ACPI\PNP0303".to_string(), "*PNP0303".to_string()];
    ps2.overrides = DeviceOverrides {
        override_keyboard_type: Some(4),
        override_keyboard_subtype: Some(0),
        ..DeviceOverrides::default()
    };
    ps2.reported_type = Some(KeyboardType::US);
    let mut usb: KeyboardDevice = fixtures::keychron();
    usb.instance_id = DESKTOP_USB.to_string();
    usb.display_name = "USB Keyboard".to_string();
    usb.container_id = Some("{8A2F1C3E-0000-4000-8000-000000000001}".to_string());
    usb.vendor_id = Some(0x04D9);
    usb.product_id = Some(0x1818);
    usb.usb_serial = None;
    usb.hardware_ids = vec![r"HID\VID_04D9&PID_1818&MI_00".to_string()];
    usb.parent_chain = vec![r"USB\VID_04D9&PID_1818&MI_00\6&1A2B3C4D&0&0000".to_string()];
    usb.overrides = DeviceOverrides {
        keyboard_type_override: Some(4),
        keyboard_subtype_override: Some(0),
        ..DeviceOverrides::default()
    };
    usb.reported_type = Some(KeyboardType::US);
    let global = GlobalSettings {
        layer_driver_jpn: Some("kbd101.dll".to_string()),
        layer_driver_kor: Some("kbd101a.dll".to_string()),
        override_keyboard_identifier: Some("PCAT_101KEY".to_string()),
        override_keyboard_type: None,
        override_keyboard_subtype: None,
    };
    let mut w = World::new(vec![ps2, usb], global);
    let (ops, baselines) = fixtures::legacy_guid_journal();
    w.seed_journal(&ops, &baselines);
    // What the fixed build reads on that PC: the GUID never changes.
    w.host.set_legacy_guid(Some(LEGACY_PC_GUID));
    w
}

/// The seeded documents, as stored.
fn seeded_ops() -> Vec<(String, String)> {
    fixtures::legacy_guid_journal().0
}

/// The reported case: the fix is installed after the restart of 11:37 (no restart needed for
/// the update). The next session closes the reverted migration, and Keep is accepted.
#[test]
fn after_the_restart_keep_works_and_the_reverted_migration_closes() {
    let mut w = desktop_pc();
    w.host
        .set_boot_time(Some(LEGACY_PC_BOOT_TIME_AFTER_RESTART));
    let values_before = values_of(&w.registry.contents());

    let result = World::ok(w.confirm(&pending_op()));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.pending_action, None);

    let pending = w.entry(&pending_op());
    assert_eq!(pending.state, OpState::Confirmed);
    assert_eq!(pending.apply_pending, None);
    let reasons: Vec<(OpState, &str)> = pending
        .history
        .iter()
        .map(|h| (h.to, h.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        vec![
            (OpState::Planned, "migrate"),
            (OpState::Written, "written"),
            (OpState::PendingReboot, "restart-pc"),
            (OpState::AwaitingConfirm, "recover:reboot-observed"),
            (OpState::Confirmed, "keep"),
        ]
    );
    // The old lines stay verbatim; the new ones carry this boot.
    assert!(
        pending.history[..3]
            .iter()
            .all(|h| h.boot == LEGACY_PC_GUID)
    );
    assert!(
        pending.history[3..]
            .iter()
            .all(|h| h.boot == w.host.current_boot())
    );
    // Its last write phase was an earlier boot: the GUID stays (nothing adopted it).
    assert_eq!(pending.boot_id, LEGACY_PC_GUID);

    let reverted = w.entry(&reverted_op());
    assert_eq!(reverted.state, OpState::Reverted);
    assert_eq!(reverted.apply_pending, None);
    assert_eq!(
        reverted.history.last().map(|h| h.reason.as_str()),
        Some("reboot-observed")
    );
    assert_eq!(reverted.boot_id, LEGACY_PC_GUID);

    // Nothing but the journal changed, and new changes are accepted again.
    assert_eq!(values_of(&w.registry.contents()), values_before);
    assert!(w.journal().open_entries().is_empty());
    let set = World::ok(w.set(
        DESKTOP_USB,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(set.outcome, Outcome::Confirmed);
    let new = w.entry(set.op_id.as_ref().unwrap());
    assert_eq!(new.boot_id, w.host.current_boot());
    assert!(new.boot_id.boot_counter().is_some());
}

/// A session that only reads (here a refused request) already closes the reverted migration:
/// housekeeping of D.1 step 7 under the lock.
#[test]
fn after_the_restart_any_session_closes_the_reverted_migration() {
    let mut w = desktop_pc();
    w.host
        .set_boot_time(Some(LEGACY_PC_BOOT_TIME_AFTER_RESTART));
    let refused = w.migrate(Layout::Us, &[(DESKTOP_USB, LayoutChoice::Us)]);
    assert!(
        matches!(&refused, Err(EngineError::OpInProgress { op_id, state: OpState::PendingReboot }) if *op_id == pending_op()),
        "{refused:?}"
    );
    // The gate refused before the session; the recovery path runs housekeeping.
    World::ok(w.recover());
    let reverted = w.entry(&reverted_op());
    assert_eq!(reverted.state, OpState::Reverted);
    assert_eq!(reverted.apply_pending, None);
    let pending = w.entry(&pending_op());
    assert_eq!(pending.state, OpState::AwaitingConfirm, "reboot observed");
}

/// The same journal if the fix had been installed before the restart of 11:37: both entries
/// were written in this boot, so Keep is refused and nothing is written.
#[test]
fn before_the_restart_keep_is_refused_and_nothing_changes() {
    let mut w = desktop_pc();
    w.host.set_boot_time(Some(LEGACY_PC_BOOT_TIME_OF_WRITES));
    let before = w.registry.contents();

    let refused = w.confirm(&pending_op());
    assert_eq!(
        refused,
        Err(EngineError::InvalidState {
            op_id: pending_op(),
            state: OpState::PendingReboot,
            action: "confirm before the PC restarts (restart, not shut down)",
        })
    );
    World::ok(w.recover());
    let after = w.registry.contents();
    assert_eq!(after, before, "nothing was written");
    assert_eq!(
        w.entry(&reverted_op()).state,
        OpState::RevertedPendingReboot
    );
    assert_eq!(w.entry(&pending_op()).state, OpState::PendingReboot);

    // `read_journal` shows the store as it is: the GUIDs, not the adopted ids.
    let stored = w.run(|e| e.read_journal()).expect("journal");
    for entry in &stored.entries {
        assert_eq!(entry.boot_id, LEGACY_PC_GUID, "{}", entry.op_id);
    }
    assert_eq!(
        stored
            .entry(&pending_op())
            .and_then(|e| e.apply_pending.as_ref())
            .map(|p| p.since),
        Some(LEGACY_PC_GUID)
    );

    // After a restart (the counter goes up; the GUID and, on that PC, nothing else tells) the
    // boot time changes too, and Keep works.
    w.reboot();
    w.host
        .set_boot_time(Some(LEGACY_PC_BOOT_TIME_AFTER_RESTART));
    let result = World::ok(w.confirm(&pending_op()));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(w.entry(&reverted_op()).state, OpState::Reverted);
}

/// A legacy entry of this boot still waits for the restart, whatever the GUID says, as long as
/// the boot time is this boot's. Sleep, hibernation and a Fast Startup shutdown are expected to
/// keep the boot time and the counter (expected; design H.2 MT-2..MT-4, MT-8 and MT-9 check it
/// before a release). The 4 s offset here stays within the tolerance.
#[test]
fn a_legacy_entry_of_this_boot_waits_whatever_the_guid_says() {
    for guid in [Some(LEGACY_PC_GUID), Some(BootId(0x1234)), None] {
        let mut w = desktop_pc();
        w.host.set_legacy_guid(guid);
        w.host
            .set_boot_time(Some(LEGACY_PC_BOOT_TIME_OF_WRITES + 40_000_000));
        assert!(
            matches!(
                w.confirm(&pending_op()),
                Err(EngineError::InvalidState {
                    state: OpState::PendingReboot,
                    ..
                })
            ),
            "{guid:?}"
        );
    }
}

/// A legacy entry judged to be of this boot and then reverted is stored in the new form: its
/// `boot_id` is this boot's counter id, and the lines 0.1.0 wrote stay verbatim. The other entry
/// is not written at all.
#[test]
fn a_legacy_entry_of_this_boot_is_rewritten_only_when_the_engine_writes_it() {
    let mut w = desktop_pc();
    w.host.set_boot_time(Some(LEGACY_PC_BOOT_TIME_OF_WRITES));
    let result = World::ok(w.revert(&pending_op(), NO_RESET));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);

    let entry = w.entry(&pending_op());
    let counter = w.host.current_boot();
    assert!(!counter.is_legacy());
    assert_eq!(entry.state, OpState::RevertedPendingReboot);
    assert_eq!(entry.boot_id, counter);
    let original = mklm_core::JournalEntry::from_json(&seeded_ops()[1].1).unwrap();
    assert_eq!(entry.history[..3], original.history[..3]);
    assert!(entry.history[3..].iter().all(|h| h.boot == counter));
    // The values are back at `before`.
    assert_eq!(w.hid(DESKTOP_USB), (RegValue::Absent, RegValue::Absent));
    assert_eq!(
        w.ps2(&device(DESKTOP_PS2)),
        (RegValue::Absent, RegValue::Absent)
    );
    assert_eq!(w.ps2(&WriteTarget::Global), (dword(7), dword(0)));

    // The reverted migration of the same boot was left alone, stored verbatim.
    let contents = w.registry.contents();
    let (name, json) = &seeded_ops()[0];
    assert_eq!(contents.ops.get(name), Some(json));
}
