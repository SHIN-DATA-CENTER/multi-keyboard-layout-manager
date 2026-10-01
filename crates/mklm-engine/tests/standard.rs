mod common;
use common::*;
use mklm_core::{
    DeviceOverrides, Event, FailureReason, Layout, LayoutChoice, OpState, Outcome, PendingAction,
    RegValue, SkipReason, WriteTarget, value_names,
};
use mklm_engine::{
    DecisionPoll, EventSink, SetStandardParams,
    memory::{CrashImage, FakeReset, FaultPlan, ScriptedSink},
};

struct OnEvent<F>(F);

impl<F: FnMut(&Event)> EventSink for OnEvent<F> {
    fn event(&mut self, event: &Event) {
        (self.0)(event);
    }

    fn check_cancelled(&mut self) -> bool {
        false
    }

    fn wait_decision(&mut self, _: std::time::Duration) -> DecisionPoll {
        DecisionPoll::NoDecision
    }
}

fn world() -> World {
    let mut snapshot = mklm_core::fixtures::dev_machine();
    snapshot
        .keyboards
        .iter_mut()
        .find(|kb| kb.instance_id == KEYCHRON)
        .unwrap()
        .overrides = DeviceOverrides::default();
    World::new(snapshot.keyboards, snapshot.global)
}

fn params() -> SetStandardParams {
    SetStandardParams {
        standard: Layout::Us,
        follow: Vec::new(),
        apply: NO_RESET,
        expected: None,
    }
}

#[test]
fn pins_before_changing_the_standard_and_never_captures_guard_baselines() {
    let mut w = world();
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    assert_eq!(w.hid(KEYCHRON), (dword(7), dword(2)));
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd101.dll")
    );
    let op = result.op_id.unwrap();
    let guards: Vec<_> = w
        .entry(&op)
        .records
        .into_iter()
        .filter(|r| r.is_check_only())
        .collect();
    assert_eq!(guards.len(), 2);
    for guard in guards {
        assert!(w.journal().baseline(&guard.key()).is_none());
    }
    w.reboot();
    assert_eq!(World::ok(w.confirm(&op)).outcome, Outcome::Confirmed);
    assert_eq!(
        World::ok(w.revert(&op, LIVE)).outcome,
        Outcome::RevertedPendingReboot
    );
    assert!(
        w.devices.restarted().is_empty(),
        "global rollback must wait for restart"
    );
    assert_eq!(w.hid(KEYCHRON), (RegValue::Absent, RegValue::Absent));
}

#[test]
fn a_denied_pin_rolls_back_without_requesting_a_restart() {
    let mut w = world();
    w.registry.set_faults(mklm_engine::memory::FaultPlan {
        deny_target: Some(device(KEYCHRON)),
        ..Default::default()
    });
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd106.dll")
    );
    assert_eq!(w.hid(KEYCHRON), (RegValue::Absent, RegValue::Absent));
}

#[test]
fn remote_confirmation_names_visibility_instead_of_reconnecting() {
    let mut w = world();
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
    w.reboot();
    w.host.set_remote_session(true);
    w.devices.set_raw_input_listed(false);
    let result = World::ok(w.confirm(&result.op_id.unwrap()));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s.contains("not visible in this Remote Desktop session"))
    );
    assert!(
        !result
            .warnings
            .iter()
            .any(|s| s.contains("reconnect the keyboard"))
    );
}

#[test]
fn explicit_follow_does_not_pin_and_same_standard_writes_nothing() {
    let mut w = world();
    let mut p = params();
    p.standard = Layout::Jis;
    assert_eq!(
        World::ok(w.run(|e| e.set_standard(&p, &mut ScriptedSink::default()))).outcome,
        Outcome::NoChange
    );
    assert!(w.journal().entries.is_empty());
    p.standard = Layout::Us;
    p.follow = vec![KEYCHRON.into()];
    World::ok(w.run(|e| e.set_standard(&p, &mut ScriptedSink::default())));
    assert_eq!(w.hid(KEYCHRON), (RegValue::Absent, RegValue::Absent));
}

#[test]
fn a_global_conflict_cannot_unpin_followers_through_undo() {
    for (name, value) in [
        (value_names::LAYER_DRIVER_JPN, sz("outside.dll")),
        (value_names::PS2_TYPE, dword(7)),
    ] {
        let mut w = world();
        let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
        let op = result.op_id.unwrap();
        w.registry
            .outside_edit(&WriteTarget::Global, name, value.clone());
        assert_eq!(
            World::ok(w.revert(&op, NO_RESET)).outcome,
            Outcome::Conflict
        );
        World::ok(w.undo());
        assert_eq!(w.entry(&op).state, OpState::Conflict);
        assert_eq!(w.hid(KEYCHRON), (dword(7), dword(2)));
        assert_eq!(w.value(&WriteTarget::Global, name), value);
    }
}

#[test]
fn undo_rechecks_global_values_that_were_already_restored() {
    let mut w = world();
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
    let op = result.op_id.unwrap();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(8));
    assert_eq!(
        World::ok(w.revert(&op, NO_RESET)).outcome,
        Outcome::Conflict
    );
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd106.dll")
    );
    let registry = w.registry.clone();
    let mut sink = OnEvent(move |event: &Event| {
        if matches!(
            event,
            Event::StateChanged {
                state: OpState::RevertPending,
                ..
            }
        ) {
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_TYPE, dword(4));
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(0));
        }
    });
    World::ok(w.run(|e| e.undo_open(&NO_RESET, &mut sink)));
    assert_eq!(w.entry(&op).state, OpState::Conflict);
    assert_eq!(
        w.hid(KEYCHRON),
        (dword(8), dword(2)),
        "a newly appeared fixed-mode pair must block removing even a nonconflicting pin value"
    );
    assert_eq!(w.ps2(&WriteTarget::Global), (dword(4), dword(0)));
}

#[test]
fn resumed_conflict_undo_records_a_new_global_conflict_without_unpinning() {
    let mut w = world();
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut ScriptedSink::default())));
    let op = result.op_id.unwrap();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(8));
    assert_eq!(
        World::ok(w.revert(&op, NO_RESET)).outcome,
        Outcome::Conflict
    );
    let registry = w.registry.clone();
    let mut sink = OnEvent(move |event: &Event| {
        if matches!(
            event,
            Event::StateChanged {
                state: OpState::RevertPending,
                ..
            }
        ) {
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_TYPE, dword(4));
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(0));
            registry.set_faults(FaultPlan {
                crash_after: Some(0),
                ..Default::default()
            });
        }
    });
    assert!(w.run(|e| e.undo_open(&NO_RESET, &mut sink)).is_err());
    let mut recovered = w.after_crash(CrashImage::ProcessKill, false);
    let result = World::ok(recovered.recover());
    assert_eq!(recovered.entry(&op).state, OpState::Conflict);
    assert_eq!(recovered.hid(KEYCHRON), (dword(8), dword(2)));
    assert!(
        result
            .conflicts
            .iter()
            .any(|conflict| conflict.name == value_names::PS2_TYPE),
        "the user must be shown the global value that blocks undo"
    );
}

#[test]
fn a_removed_pin_is_durable_before_the_standard_changes() {
    let mut w = world();
    let registry = w.registry.clone();
    let mut checked_skip = false;
    let mut pin_step = 0;
    let mut sink = OnEvent(|event: &Event| match event {
        Event::Planned { steps, .. } => {
            pin_step = steps
                .iter()
                .position(|step| step.target == device(KEYCHRON))
                .unwrap()
                + 1;
            registry.remove_devnode(KEYCHRON);
        }
        Event::StepWritten { op_id, step, .. } if *step == pin_step => {
            for image in registry.crash_images() {
                let crashed = registry.after_crash(image);
                let journal = journal_of(&crashed);
                let entry = journal.entry(op_id).unwrap();
                assert!(
                    entry
                        .records
                        .iter()
                        .filter(|r| r.target == device(KEYCHRON))
                        .all(|r| r.skipped == Some(SkipReason::DeviceRemoved)),
                    "the skipped pin must survive {image:?}"
                );
                assert_eq!(
                    crashed.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
                    sz("kbd106.dll")
                );
            }
            checked_skip = true;
        }
        _ => {}
    });
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut sink)));
    assert!(checked_skip);
    assert_eq!(result.outcome, Outcome::PendingReboot);
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd101.dll")
    );
}

#[test]
fn failures_before_and_after_the_first_global_write_request_the_right_action() {
    for (fail_at, outcome, pending) in [
        (1, Outcome::Reverted, None),
        (
            2,
            Outcome::RevertedPendingReboot,
            Some(PendingAction::RestartPc),
        ),
    ] {
        let mut w = world();
        let registry = w.registry.clone();
        let mut injected = false;
        let mut sink = OnEvent(move |event: &Event| {
            if !injected && matches!(event, Event::StepWritten { step, of, .. } if *step + 1 == *of)
            {
                registry.set_faults(FaultPlan {
                    fail_at: Some(fail_at),
                    ..Default::default()
                });
                injected = true;
            }
        });
        let result = World::ok(w.run(|e| e.set_standard(&params(), &mut sink)));
        assert_eq!(result.outcome, outcome, "failure at global write {fail_at}");
        assert_eq!(result.pending_action, pending);
        assert!(matches!(
            result.failure,
            Some(FailureReason::WriteError {
                target: Some(WriteTarget::Global),
                ..
            })
        ));
        assert_eq!(
            w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
            sz("kbd106.dll")
        );
        assert_eq!(w.hid(KEYCHRON), (RegValue::Absent, RegValue::Absent));
    }
}

#[test]
fn a_fixed_mode_pair_appearing_after_pins_blocks_the_global_step() {
    let mut w = world();
    let registry = w.registry.clone();
    let mut sink = OnEvent(move |event: &Event| {
        if matches!(event, Event::StepWritten { step: 1, .. }) {
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_TYPE, dword(4));
            registry.outside_edit(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(0));
        }
    });
    let result = World::ok(w.run(|e| e.set_standard(&params(), &mut sink)));
    assert_eq!(result.outcome, Outcome::Conflict);
    assert_eq!(w.hid(KEYCHRON), (dword(7), dword(2)));
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd106.dll")
    );
    assert_eq!(w.ps2(&WriteTarget::Global), (dword(4), dword(0)));
}

#[test]
fn live_pin_resets_do_not_count_down_or_roll_back_a_standard_change() {
    for reset in [FakeReset::Applies, FakeReset::NeverArrives] {
        let mut w = world();
        w.devices.set_reset(KEYCHRON, reset);
        let mut p = params();
        p.apply = LIVE;
        let mut sink = ScriptedSink::default();
        let result = World::ok(w.run(|e| e.set_standard(&p, &mut sink)));
        assert_eq!(result.outcome, Outcome::PendingReboot);
        assert_eq!(sink.waits, 0);
        let entry = w.entry(&result.op_id.unwrap());
        assert!(!entry.history.iter().any(|h| h.to == OpState::Restarting));
        assert_eq!(w.hid(KEYCHRON), (dword(7), dword(2)));
        assert_eq!(w.devices.restarted(), [KEYCHRON.to_string()]);
        let waiting = entry.apply_pending.unwrap().instance_ids;
        assert!(
            waiting.iter().any(|id| id == VXE),
            "BLE pins wait for a restart"
        );
        if reset == FakeReset::Applies {
            assert!(!waiting.iter().any(|id| id == KEYCHRON));
        } else {
            assert!(waiting.iter().any(|id| id == KEYCHRON));
        }
    }
}

#[test]
fn invisible_keyboards_distinguish_restarted_values_from_pending_reconnects() {
    for remote in [false, true] {
        for reboot in [false, true] {
            let mut w = world();
            let result = World::ok(w.set(
                KEYCHRON,
                LayoutChoice::Us,
                NO_RESET,
                &mut ScriptedSink::default(),
            ));
            let op = result.op_id.unwrap();
            if reboot {
                w.reboot();
            }
            w.host.set_remote_session(remote);
            w.devices.set_raw_input_listed(false);
            let result = World::ok(w.confirm(&op));
            let warning = result
                .warnings
                .iter()
                .find(|warning| warning.contains(KEYCHRON))
                .unwrap();
            assert_eq!(warning.contains("Remote Desktop"), remote);
            assert_eq!(warning.contains("in effect since the restart"), reboot);
            assert_eq!(warning.contains("reconnect it"), !reboot);
            assert_eq!(
                result.pending_action,
                if reboot {
                    None
                } else {
                    Some(PendingAction::Reconnect)
                }
            );
        }
    }
}
