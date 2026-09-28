//! Behaviour of the engine on the in-memory fakes (design H.1, "mklm-engine", except the
//! exhaustive crash tests, which are in `crash.rs`).

mod common;

use std::time::Duration;

use common::*;
use mklm_core::{
    ApplyOptions, ConflictPolicy, ErrorCode, Event, ExpectedPlan, FailureReason, JournalEntry,
    KeyboardType, Layout, LayoutChoice, OpId, OpKind, OpState, OperationError, Outcome,
    PendingAction, PlanError, RegValue, ResolutionChoice, RestoreError, RestoreScope, SkipReason,
    ValueChoice, WriteTarget, value_names,
};
use mklm_engine::memory::{FakeReset, FaultPlan, ScriptedSink};
use mklm_engine::{
    DecisionPoll, EngineError, Host, RegistryBackend, ResolveParams, RestoreMode, SetLayoutParams,
};

fn op_of(result: &mklm_core::OperationResult) -> OpId {
    result.op_id.clone().expect("the result names an operation")
}

fn us() -> (RegValue, RegValue) {
    (dword(4), dword(0))
}

fn jis() -> (RegValue, RegValue) {
    (dword(7), dword(2))
}

fn absent() -> (RegValue, RegValue) {
    (RegValue::Absent, RegValue::Absent)
}

/// A world with a confirmed JIS on the Keychron.
fn keychron_jis_confirmed() -> (World, OpId) {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
    let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Confirmed);
    (w, op_of(&result))
}

/// A pre-M0 machine migrated (JIS standard, Keychron assigned JIS), in `PendingReboot`.
fn migrated_pending() -> (World, OpId) {
    let mut w = World::pre_m0();
    let result = World::ok(w.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)]));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    (w, op_of(&result))
}

/// The same, restarted and kept.
fn migrated_confirmed() -> (World, OpId) {
    let (mut w, op) = migrated_pending();
    w.reboot();
    let result = World::ok(w.recover());
    assert_eq!(result.recovered.len(), 1);
    assert_eq!(result.recovered[0].to, OpState::AwaitingConfirm);
    let result = World::ok(w.confirm(&op));
    assert_eq!(result.outcome, Outcome::Confirmed);
    (w, op)
}

fn no_in_flight(w: &World) {
    for entry in &w.journal().entries {
        assert!(
            !entry.state.is_in_flight(),
            "{} left in flight: {:?}",
            entry.op_id,
            entry.state
        );
        assert!(
            !(entry.state == OpState::AwaitingConfirm && entry.countdown.is_some()),
            "{} left counting down",
            entry.op_id
        );
    }
    assert!(!w.lock.is_locked(), "the lock was not released");
}

// ------------------------------------------------------------------------------------------
// Normal paths
// ------------------------------------------------------------------------------------------

#[test]
fn usb_set_keep_confirms() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
    let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.failure, None);
    assert_eq!(result.pending_action, None);
    assert_eq!(w.hid(KEYCHRON), jis());
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::JIS));
    assert_eq!(w.devices.restarted(), [KEYCHRON.to_string()]);
    let entry = w.entry(&op_of(&result));
    assert_eq!(entry.state, OpState::Confirmed);
    assert_eq!(entry.apply, Some(PendingAction::ResetKeyboard));
    assert_eq!(entry.apply_pending, None);
    assert_eq!(entry.records.len(), 2);
    assert!(
        entry
            .records
            .iter()
            .all(|r| r.last_written == Some(r.intended.clone()))
    );
    // Events in order: lock, recovery files, plan, steps, states, reset, countdown.
    assert_eq!(sink.events[0], Event::Locked);
    assert!(sink.count(|e| matches!(e, Event::RecoveryAssetsWritten { .. })) == 1);
    let planned = sink
        .events
        .iter()
        .position(|e| matches!(e, Event::Planned { .. }))
        .expect("Planned");
    let first_step = sink
        .events
        .iter()
        .position(|e| matches!(e, Event::StepWritten { .. }))
        .expect("StepWritten");
    assert!(planned < first_step);
    assert!(sink.events.contains(&Event::CountdownStarted {
        op_id: entry.op_id.clone(),
        seconds: 20,
        verified: true,
    }));
    assert!(sink.events.contains(&Event::KeyboardArrived {
        instance_id: KEYCHRON.to_string(),
        reported: Some(KeyboardType::JIS),
        expected: KeyboardType::JIS,
    }));
    if let Some(Event::Planned {
        keyboards, apply, ..
    }) = sink
        .events
        .iter()
        .find(|e| matches!(e, Event::Planned { .. }))
    {
        assert_eq!(*apply, Some(PendingAction::ResetKeyboard));
        assert_eq!(keyboards.len(), 1);
        assert_eq!(keyboards[0].expected_type, Some(KeyboardType::JIS));
        assert!(keyboards[0].changes);
    }
    // The baselines are the values before MKLM.
    let journal = w.journal();
    assert_eq!(journal.baselines.len(), 2);
    assert!(journal.baselines.iter().any(|b| b.value == dword(4)));
    no_in_flight(&w);
}

#[test]
fn usb_set_reverts_and_resets_again_on_revert_now_expiry_and_disconnect() {
    let cases: Vec<(ScriptedSink, Option<FailureReason>)> = vec![
        (ScriptedSink::new([ScriptedSink::revert_now()]), None),
        (
            ScriptedSink::default(),
            Some(FailureReason::CountdownExpired),
        ),
        (
            ScriptedSink::new([DecisionPoll::NoDecision, DecisionPoll::Disconnected]),
            Some(FailureReason::CallerDisconnected),
        ),
    ];
    for (mut sink, failure) in cases {
        let mut w = World::dev_machine();
        let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
        assert_eq!(result.outcome, Outcome::Reverted, "{failure:?}");
        assert_eq!(result.failure, failure);
        assert_eq!(w.hid(KEYCHRON), us());
        assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::US));
        assert_eq!(w.devices.restarted().len(), 2, "reset again to apply US");
        let entry = w.entry(&op_of(&result));
        assert_eq!(entry.state, OpState::Reverted);
        assert_eq!(entry.apply_pending, None);
        assert!(
            entry
                .records
                .iter()
                .all(|r| r.last_written == Some(r.before.clone()))
        );
        no_in_flight(&w);
    }
    // The countdown ran its 20 ticks before expiring.
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default();
    World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(sink.waits, 20);
    assert_eq!(sink.count(|e| matches!(e, Event::CountdownTick { .. })), 20);
}

#[test]
fn ble_set_waits_for_a_reconnect_and_keep_leaves_apply_pending_until_reconnected() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default();
    let result = World::ok(w.set(VXE, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(result.pending_action, Some(PendingAction::Reconnect));
    assert!(
        w.devices.restarted().is_empty(),
        "BLE is never reset in place"
    );
    assert!(sink.count(|e| matches!(e, Event::WaitingForReconnect { .. })) > 0);
    let op = op_of(&result);
    let entry = w.entry(&op);
    assert_eq!(entry.state, OpState::AwaitingConfirm);
    assert_eq!(entry.countdown, None);
    let pending = entry.apply_pending.expect("apply_pending");
    assert_eq!(pending.action, PendingAction::Reconnect);
    assert_eq!(pending.instance_ids, [VXE.to_string()]);
    assert!(!w.lock.is_locked());

    // Keep before Raw Input reports the new type: kept, with apply_pending (C1).
    let result = World::ok(w.confirm(&op));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.pending_action, Some(PendingAction::Reconnect));
    assert!(!result.warnings.is_empty());
    assert!(w.entry(&op).apply_pending.is_some());

    // Once reconnected, the next request's housekeeping clears it.
    w.devices.reconnect(VXE);
    World::ok(w.recover());
    assert_eq!(w.entry(&op).apply_pending, None);
}

#[test]
fn ble_set_keep_during_the_reconnect_wait() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::new([DecisionPoll::NoDecision, ScriptedSink::keep()]);
    let result = World::ok(w.set(VXE, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.pending_action, Some(PendingAction::Reconnect));
    no_in_flight(&w);

    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::new([ScriptedSink::revert_now()]);
    let result = World::ok(w.set(VXE, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(w.hid(VXE), absent());
    no_in_flight(&w);
}

#[test]
fn ps2_set_waits_for_a_restart() {
    let mut w = World::dev_machine();
    let result = World::ok(w.set(PS2, LayoutChoice::Us, LIVE, &mut ScriptedSink::default()));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
    assert_eq!(w.ps2(&device(PS2)), us());
    assert_eq!(w.devices.running_type(PS2), Some(KeyboardType::JIS));
    assert!(w.devices.restarted().is_empty());
    // Boot-time values need the recovery files.
    assert!(w.host.recovery_assets().is_some());
    let op = op_of(&result);
    assert!(w.journal().needs_post_reboot_check(w.host.current_boot()));
    // Keep before the restart is refused.
    assert!(matches!(
        w.confirm(&op),
        Err(EngineError::InvalidState {
            state: OpState::PendingReboot,
            ..
        })
    ));
    w.reboot();
    assert_eq!(w.devices.running_type(PS2), Some(KeyboardType::US));
    let result = World::ok(w.confirm(&op));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.pending_action, None);
}

#[test]
fn migrate_restart_recover_confirm() {
    let (mut w, op) = migrated_pending();
    // Pins first, the global pair deleted, the Keychron assigned.
    assert_eq!(w.ps2(&device(PS2)), jis());
    assert_eq!(w.ps2(&WriteTarget::Global), absent());
    assert_eq!(w.hid(KEYCHRON), jis());
    let entry = w.entry(&op);
    assert!(matches!(entry.kind, OpKind::Migrate { .. }));
    assert!(!entry.context.is_empty(), "plan 1.3 step 1 snapshot");
    let order: Vec<&WriteTarget> = entry.records.iter().map(|r| &r.target).collect();
    assert_eq!(order.first(), Some(&&device(PS2)));
    assert_eq!(order.last(), Some(&&WriteTarget::Global));
    assert!(w.host.recovery_assets().is_some());

    w.reboot();
    let result = World::ok(w.recover());
    assert_eq!(result.outcome, Outcome::Recovered);
    assert_eq!(result.recovered[0].decision, "reboot-observed");
    assert_eq!(w.entry(&op).state, OpState::AwaitingConfirm);
    let result = World::ok(w.confirm(&op));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(w.devices.running_type(PS2), Some(KeyboardType::JIS));
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::JIS));
}

#[test]
fn migration_revert_needs_a_restart_then_closes() {
    let (mut w, op) = migrated_pending();
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
    assert_eq!(w.ps2(&WriteTarget::Global), jis());
    assert_eq!(w.ps2(&device(PS2)), absent());
    assert_eq!(w.hid(KEYCHRON), us());
    w.reboot();
    World::ok(w.recover());
    assert_eq!(w.entry(&op).state, OpState::Reverted);
}

#[test]
fn restore_all_from_the_migrated_state() {
    let (mut w, migration) = migrated_confirmed();
    let pristine = World::pre_m0();
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    let op = op_of(&result);
    assert_eq!(
        values_of(&w.registry.contents()),
        values_of(&pristine.registry.contents())
    );
    // Baselines stay until the restore is kept.
    assert_eq!(w.journal().baselines.len(), 6);
    let entry = w.entry(&op);
    // Phase order: the fixed pair first, the pins last.
    assert_eq!(
        entry.records.first().map(|r| &r.target),
        Some(&WriteTarget::Global)
    );
    assert_eq!(entry.records.last().map(|r| &r.target), Some(&device(PS2)));
    w.reboot();
    World::ok(w.recover());
    let result = World::ok(w.confirm(&op));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert!(w.journal().baselines.is_empty(), "C.5 clean-up");
    assert_eq!(w.entry(&migration).state, OpState::Confirmed);
    // A confirmed restore cannot be reverted (C6).
    assert!(matches!(
        w.revert(&op, NO_RESET),
        Err(EngineError::InvalidState { .. })
    ));
}

#[test]
fn silent_restore_confirms_and_cleans_up() {
    let (mut w, _) = migrated_confirmed();
    let pristine = World::pre_m0();
    let result = World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
    assert_eq!(
        values_of(&w.registry.contents()),
        values_of(&pristine.registry.contents())
    );
    assert!(w.journal().baselines.is_empty());
    let entry = w.entry(&op_of(&result));
    assert!(matches!(
        entry.kind,
        OpKind::RestoreBaseline { silent: true, .. }
    ));
}

#[test]
fn silent_restore_skips_outside_values_but_never_breaks_inv_ps2() {
    // The Settings app put a fixed pair back (US fixed, 7/0): skipping the conflicting subtype
    // still leaves a pair, so the pins can go.
    let (mut w, _) = migrated_confirmed();
    w.registry
        .outside_edit(&WriteTarget::Global, value_names::PS2_TYPE, dword(7));
    w.registry
        .outside_edit(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(0));
    let result = World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert!(!result.warnings.is_empty());
    assert_eq!(w.ps2(&WriteTarget::Global), (dword(7), dword(0)));
    assert_eq!(w.ps2(&device(PS2)), absent());
    let entry = w.entry(&op_of(&result));
    assert!(
        entry
            .records
            .iter()
            .any(|r| r.skipped == Some(SkipReason::ConflictSkipped))
    );

    // A global type of the wrong registry type is not a pair: skipping it and removing the pins
    // would break INV-PS2, so the restore is refused before anything is written (C12), whatever
    // the silent mode says.
    let (mut w, _) = migrated_confirmed();
    w.registry.outside_edit(
        &WriteTarget::Global,
        value_names::PS2_TYPE,
        RegValue::Other {
            reg_type: 1,
            data_hex: "370000".into(),
        },
    );
    let before = w.registry.contents();
    let result = w.restore_all(RestoreMode::Silent);
    assert!(
        matches!(result, Err(EngineError::Restore(RestoreError::InvPs2(_)))),
        "{result:?}"
    );
    assert_eq!(w.registry.contents(), before, "nothing written");
    assert_eq!(w.ps2(&device(PS2)), jis(), "the pins stay");
    let keyboards = keyboards_of(&w);
    assert!(mklm_core::check_inv_ps2(&w.registry.global_settings(), &keyboards).is_ok());
    no_in_flight(&w);
}

#[test]
fn undo_a_pending_migration() {
    let (mut w, op) = migrated_pending();
    let result = World::ok(w.undo());
    assert_eq!(result.outcome, Outcome::Recovered);
    assert_eq!(result.recovered.len(), 1);
    assert_eq!(result.recovered[0].decision, "undo");
    assert_eq!(result.recovered[0].to, OpState::RevertedPendingReboot);
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
    assert_eq!(w.ps2(&WriteTarget::Global), jis());
    assert_eq!(w.ps2(&device(PS2)), absent());
    // Undone before the restart: nothing reached a driver.
    assert_eq!(w.entry(&op).apply_pending, None);
}

#[test]
fn undo_a_reconnect_wait() {
    let mut w = World::dev_machine();
    let result = World::ok(w.set(VXE, LayoutChoice::Jis, LIVE, &mut ScriptedSink::default()));
    let op = op_of(&result);
    let result = World::ok(w.undo());
    assert_eq!(result.recovered[0].to, OpState::Reverted);
    assert_eq!(w.hid(VXE), absent());
    assert_eq!(w.entry(&op).state, OpState::Reverted);
}

#[test]
fn undo_a_conflict_writes_only_what_mklm_wrote() {
    let (mut w, op) = keychron_jis_confirmed();
    // Somebody changes the subtype; the type is still what MKLM wrote.
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_SUBTYPE, dword(5));
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Conflict);
    // One step for the key: nothing of it was written.
    assert_eq!(w.hid(KEYCHRON), (dword(7), dword(5)));
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].current, dword(5));

    let result = World::ok(w.undo());
    assert_eq!(w.hid(KEYCHRON), (dword(4), dword(5)));
    assert_eq!(w.entry(&op).state, OpState::Conflict);
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].name, value_names::HID_SUBTYPE);
    assert_eq!(result.conflicts[0].current, dword(5));

    // Once the outside value is gone, undo closes it.
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_SUBTYPE, dword(0));
    World::ok(w.undo());
    assert_eq!(w.entry(&op).state, OpState::Reverted);
}

// ------------------------------------------------------------------------------------------
// Error injection, cancellation, countdown bounds
// ------------------------------------------------------------------------------------------

#[test]
fn injected_failures_end_closed_with_the_values_back() {
    // Calls of an undisturbed run.
    let mut w = World::dev_machine();
    World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    let total = w.registry.mutating_calls();
    let mut failed_runs = 0;
    for n in 1..=total {
        let mut w = World::dev_machine();
        w.registry.set_faults(FaultPlan {
            fail_at: Some(n),
            ..FaultPlan::default()
        });
        let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
        let result = w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink);
        no_in_flight(&w);
        let result = result.unwrap_or_else(|error| panic!("fail_at {n}: {error:?}"));
        match result.outcome {
            // A journal call: retried once, the operation went through.
            Outcome::Confirmed => assert_eq!(w.hid(KEYCHRON), jis(), "fail_at {n}"),
            Outcome::Reverted => {
                failed_runs += 1;
                assert!(
                    matches!(result.failure, Some(FailureReason::WriteError { .. })),
                    "fail_at {n}: {:?}",
                    result.failure
                );
                assert_eq!(w.hid(KEYCHRON), us(), "fail_at {n}");
            }
            other => panic!("fail_at {n}: {other:?}"),
        }
    }
    assert!(
        failed_runs >= 3,
        "every target write and flush fails the operation"
    );
}

#[test]
fn a_concurrent_change_before_the_first_write_fails_the_operation() {
    /// Changes the Keychron's type when the plan is announced.
    struct Meddler {
        inner: ScriptedSink,
        registry: mklm_engine::memory::MemoryRegistry,
    }
    impl mklm_engine::EventSink for Meddler {
        fn event(&mut self, event: &Event) {
            if matches!(event, Event::Planned { .. }) {
                self.registry
                    .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(9));
            }
            self.inner.event(event);
        }
        fn check_cancelled(&mut self) -> bool {
            self.inner.check_cancelled()
        }
        fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
            self.inner.wait_decision(timeout)
        }
    }
    let mut w = World::dev_machine();
    let mut sink = Meddler {
        inner: ScriptedSink::default(),
        registry: w.registry.clone(),
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);
    let result = World::ok(w.run(|e| e.set_layout(&params, &mut sink)));
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(
        result.failure,
        Some(FailureReason::ConcurrentChange {
            name: value_names::HID_TYPE.to_string()
        })
    );
    assert_eq!(w.hid(KEYCHRON), (dword(9), dword(0)));
    no_in_flight(&w);
}

#[test]
fn a_caller_that_left_before_planned_gets_nothing_written() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default().cancel_after(0);
    assert_eq!(
        w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink),
        Err(EngineError::Cancelled)
    );
    assert_eq!(w.registry.mutating_calls(), 0);
    assert!(w.journal().entries.is_empty());

    let mut w = World::pre_m0();
    let params = migrate_params(Layout::Jis, &[]);
    let mut sink = ScriptedSink::default().cancel_after(0);
    assert_eq!(
        w.run(|e| e.migrate(&params, &mut sink)),
        Err(EngineError::Cancelled)
    );
    assert_eq!(w.registry.mutating_calls(), 0);
    assert!(w.journal().entries.is_empty());
    assert_eq!(EngineError::Cancelled.to_info().code, ErrorCode::Cancelled);
}

#[test]
fn a_caller_that_left_after_planned_is_rolled_back_without_a_reset() {
    // Find how many events a run sends up to Planned.
    let mut w = World::dev_machine();
    let mut probe = ScriptedSink::new([ScriptedSink::keep()]);
    World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut probe));
    let planned = probe
        .events
        .iter()
        .position(|e| matches!(e, Event::Planned { .. }))
        .expect("Planned")
        + 1;

    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default().cancel_after(planned);
    let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(result.failure, Some(FailureReason::CallerDisconnected));
    assert!(w.devices.restarted().is_empty());
    assert_eq!(w.hid(KEYCHRON), us());
    let entry = w.entry(&op_of(&result));
    assert_eq!(entry.apply_pending, None);
    assert!(entry.history.iter().any(|h| h.to == OpState::Written));
    no_in_flight(&w);
}

#[test]
fn the_countdown_ends_on_the_monotonic_clock_whatever_the_sink_does() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default().with_clock(w.host.clock(), Duration::from_secs(30));
    let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(result.failure, Some(FailureReason::CountdownExpired));
    assert_eq!(sink.waits, 1, "one slow wait is enough");
}

#[test]
fn reset_failures_revert_and_ask_for_a_restart() {
    for behaviour in [
        FakeReset::Hangs,
        FakeReset::NeedsReboot,
        FakeReset::NeverArrives,
        FakeReset::Fails,
    ] {
        let mut w = World::dev_machine();
        w.devices.set_reset(KEYCHRON, behaviour);
        let result = World::ok(w.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::default(),
        ));
        assert_eq!(
            result.outcome,
            Outcome::RevertedPendingReboot,
            "{behaviour:?}"
        );
        assert_eq!(result.failure, Some(FailureReason::KeyboardDidNotReturn));
        assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
        assert_eq!(w.hid(KEYCHRON), us());
        let entry = w.entry(&op_of(&result));
        assert_eq!(
            entry.apply_pending.map(|p| p.action),
            Some(PendingAction::RestartPc)
        );
        no_in_flight(&w);
    }
}

#[test]
fn a_reset_that_does_not_apply_falls_back_to_a_reconnect() {
    let mut w = World::dev_machine();
    w.devices.set_reset(KEYCHRON, FakeReset::ArrivesUnchanged);
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    let entry = w.entry(&op_of(&result));
    assert_eq!(entry.apply, Some(PendingAction::Reconnect));
    assert_eq!(entry.countdown, None);
    assert_eq!(
        entry.apply_pending.map(|p| p.action),
        Some(PendingAction::Reconnect)
    );
    no_in_flight(&w);
}

#[test]
fn a_second_reset_that_fails_after_the_countdown_asks_for_a_restart() {
    let mut w = World::dev_machine();
    w.devices.push_reset(KEYCHRON, FakeReset::Applies);
    w.devices.push_reset(KEYCHRON, FakeReset::NeverArrives);
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);
    assert_eq!(result.failure, Some(FailureReason::CountdownExpired));
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
    assert_eq!(w.hid(KEYCHRON), us());
}

#[test]
fn without_permission_to_reset_a_usb_change_waits_for_a_reconnect_or_a_restart() {
    let mut w = World::dev_machine();
    let no_reset = ApplyOptions {
        allow_live_reset: false,
        other_input_available: true,
        ..ApplyOptions::default()
    };
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        no_reset,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert!(w.devices.restarted().is_empty());

    let mut w = World::dev_machine();
    let only_keyboard = ApplyOptions {
        allow_live_reset: true,
        other_input_available: false,
        ..ApplyOptions::default()
    };
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        only_keyboard,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    assert!(w.devices.restarted().is_empty());
}

// ------------------------------------------------------------------------------------------
// Compare-and-swap and conflicts
// ------------------------------------------------------------------------------------------

#[test]
fn an_outside_change_turns_a_revert_into_a_conflict() {
    let (mut w, op) = keychron_jis_confirmed();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(9));
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Conflict);
    assert_eq!(result.conflicts.len(), 1);
    let info = &result.conflicts[0];
    assert_eq!(info.name, value_names::HID_TYPE);
    assert_eq!(info.current, dword(9));
    assert_eq!(info.before, dword(4));
    assert_eq!(info.intended, dword(7));
    assert_eq!(info.last_written, Some(dword(7)));
    assert_eq!(info.baseline, dword(4));
    assert_eq!(info.write_error, None);
    // Nothing of that key was written.
    assert_eq!(w.hid(KEYCHRON), (dword(9), dword(2)));
    no_in_flight(&w);

    // A value somebody already put back to `before` is simply done (C.8).
    let (mut w, op) = keychron_jis_confirmed();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(4));
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(w.hid(KEYCHRON), us());
}

fn keychron_conflict() -> (World, OpId) {
    let (mut w, op) = keychron_jis_confirmed();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(4));
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_SUBTYPE, dword(5));
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Conflict);
    (w, op)
}

fn resolve(
    w: &mut World,
    op: &OpId,
    choices: &[(usize, ResolutionChoice)],
) -> Result<mklm_core::OperationResult, EngineError> {
    let params = ResolveParams {
        op_id: op.clone(),
        choices: choices
            .iter()
            .map(|(record, choice)| ValueChoice {
                record: *record,
                choice: *choice,
            })
            .collect(),
        apply: NO_RESET,
    };
    w.run(|e| e.resolve_conflict(&params, &mut ScriptedSink::default()))
}

#[test]
fn keep_current_closes_as_failed_and_records_the_current_values() {
    let (mut w, op) = keychron_conflict();
    let result = World::ok(resolve(&mut w, &op, &[]));
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.failure, Some(FailureReason::ConflictKeptCurrent));
    let entry = w.entry(&op);
    assert_eq!(entry.records[0].last_written, Some(dword(4)));
    assert_eq!(entry.records[1].last_written, Some(dword(5)));
    assert_eq!(w.hid(KEYCHRON), (dword(4), dword(5)));
    // A later restore to baseline expects exactly those values: no conflict.
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_ne!(result.outcome, Outcome::Conflict);
    assert!(result.conflicts.is_empty());
    assert_eq!(w.hid(KEYCHRON), us());
}

#[test]
fn use_baseline_and_use_intended_write_the_chosen_values() {
    let (mut w, op) = keychron_conflict();
    let result = World::ok(resolve(
        &mut w,
        &op,
        &[
            (0, ResolutionChoice::UseBaseline),
            (1, ResolutionChoice::UseBaseline),
        ],
    ));
    assert_eq!(w.hid(KEYCHRON), us());
    assert_eq!(result.outcome, Outcome::Reverted);

    let (mut w, op) = keychron_conflict();
    let result = World::ok(resolve(
        &mut w,
        &op,
        &[
            (0, ResolutionChoice::UseIntended),
            (1, ResolutionChoice::UseIntended),
        ],
    ));
    assert_eq!(w.hid(KEYCHRON), jis());
    assert_eq!(result.outcome, Outcome::Confirmed);

    // Resolving something that is not in conflict is refused.
    let (mut w, op) = keychron_jis_confirmed();
    assert!(matches!(
        resolve(&mut w, &op, &[]),
        Err(EngineError::InvalidState { .. })
    ));
}

#[test]
fn a_resolution_interrupted_then_changed_outside_does_not_overwrite_the_user() {
    let (w, op) = keychron_conflict();
    // Crash right after `RevertPending(Resolution)` was flushed (two journal calls).
    let mut run = w.fork();
    run.registry.set_faults(FaultPlan {
        crash_after: Some(2),
        ..FaultPlan::default()
    });
    let result = resolve(
        &mut run,
        &op,
        &[
            (0, ResolutionChoice::UseIntended),
            (1, ResolutionChoice::KeepCurrent),
        ],
    );
    assert!(matches!(result, Err(EngineError::Backend(_))));
    let mut after = run.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
    assert_eq!(after.entry(&op).state, OpState::RevertPending);
    // The user changes the type in the Settings app before recovery runs.
    after
        .registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(0x51));
    World::ok(after.recover());
    assert_eq!(after.entry(&op).state, OpState::Conflict);
    assert_eq!(after.hid(KEYCHRON), (dword(0x51), dword(5)));
}

#[test]
fn case_changes_of_the_layer_driver_are_not_conflicts() {
    let mut w = World::pre_m0();
    let result = World::ok(w.migrate(Layout::Us, &[]));
    let op = op_of(&result);
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd101.dll")
    );
    w.registry.outside_edit(
        &WriteTarget::Global,
        value_names::LAYER_DRIVER_JPN,
        sz("KBD101.DLL"),
    );
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);
    assert_eq!(
        w.value(&WriteTarget::Global, value_names::LAYER_DRIVER_JPN),
        sz("kbd106.dll")
    );
}

// ------------------------------------------------------------------------------------------
// Baselines
// ------------------------------------------------------------------------------------------

#[test]
fn baselines_keep_the_first_value_across_operations() {
    let w = World::dev_machine();
    // Start without Keychron values.
    let target = device(KEYCHRON);
    let mut writer = w.registry.clone();
    writer
        .write_value(&target, value_names::HID_TYPE, &RegValue::Absent)
        .unwrap();
    writer
        .write_value(&target, value_names::HID_SUBTYPE, &RegValue::Absent)
        .unwrap();
    let mut w = w.fork();
    w.devices.reboot();
    let keep = || ScriptedSink::new([ScriptedSink::keep()]);
    let first = World::ok(w.set(KEYCHRON, LayoutChoice::Us, LIVE, &mut keep()));
    let second = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut keep()));
    let third = World::ok(w.set(KEYCHRON, LayoutChoice::Standard, LIVE, &mut keep()));
    assert!(!third.warnings.is_empty(), "standard is unverified");
    let entry = w.entry(&op_of(&third));
    assert_eq!(entry.records[0].before, dword(7));
    assert_eq!(entry.records[0].baseline, RegValue::Absent);
    let journal = w.journal();
    assert_eq!(journal.baselines.len(), 2);
    assert!(
        journal
            .baselines
            .iter()
            .all(|b| b.value == RegValue::Absent)
    );
    assert!(
        journal
            .baselines
            .iter()
            .all(|b| b.captured_by == op_of(&first))
    );
    // Only the latest operation can be reverted.
    assert!(matches!(
        w.revert(&op_of(&second), NO_RESET),
        Err(EngineError::NotLatest { .. })
    ));
    World::ok(w.revert(&op_of(&third), NO_RESET));
    assert_eq!(w.hid(KEYCHRON), jis());
    World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(
        w.hid(KEYCHRON),
        absent(),
        "an absent baseline deletes the value"
    );
}

#[test]
fn other_baselines_are_restored_byte_for_byte() {
    let w = World::dev_machine();
    let odd = RegValue::Other {
        reg_type: 1,
        data_hex: "340000".to_string(),
    };
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, odd.clone());
    let mut w = w.fork();
    World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(w.hid(KEYCHRON), jis());
    World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(w.hid(KEYCHRON), (odd, dword(0)));
}

#[test]
fn restore_set_restore_returns_to_the_first_baseline() {
    let (mut w, _) = keychron_jis_confirmed();
    let restore = World::ok(w.restore_all(RestoreMode::Silent));
    assert!(w.journal().baselines.is_empty());
    assert!(matches!(
        w.revert(&op_of(&restore), NO_RESET),
        Err(EngineError::InvalidState { .. })
    ));
    World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(
        w.journal().baselines.len(),
        2,
        "new baselines after a confirmed restore"
    );
    World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(w.hid(KEYCHRON), us());
}

#[test]
fn a_removed_devnode_is_skipped_by_restore() {
    let (mut w, _) = migrated_confirmed();
    w.devices.remove(KEYCHRON);
    let result = World::ok(w.restore_all(RestoreMode::Silent));
    assert_eq!(result.outcome, Outcome::Confirmed);
    let entry = w.entry(&op_of(&result));
    let skipped: Vec<_> = entry
        .records
        .iter()
        .filter(|r| r.skipped == Some(SkipReason::DeviceRemoved))
        .collect();
    assert_eq!(skipped.len(), 2);
    assert_eq!(w.ps2(&WriteTarget::Global), jis());
    // Its baselines stay (the value could not be checked).
    assert_eq!(w.journal().baselines.len(), 2);
}

#[test]
fn restore_reports_outside_changes_unless_told_to_skip_or_overwrite() {
    let (mut w, _) = keychron_jis_confirmed();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(9));
    let before = w.registry.mutating_calls();
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_eq!(result.outcome, Outcome::Conflict);
    assert_eq!(result.op_id, None);
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].current, dword(9));
    assert_eq!(w.registry.mutating_calls(), before, "nothing written");

    let mut skip = w.fork();
    let params = restore_params(
        RestoreScope::All,
        ConflictPolicy::Skip,
        RestoreMode::Interactive,
    );
    let result = World::ok(skip.run(|e| e.restore_baseline(&params, &mut ScriptedSink::default())));
    assert!(!result.warnings.is_empty());
    assert_eq!(skip.hid(KEYCHRON), (dword(9), dword(0)));

    let mut overwrite = w.fork();
    let params = restore_params(
        RestoreScope::All,
        ConflictPolicy::Overwrite,
        RestoreMode::Interactive,
    );
    World::ok(overwrite.run(|e| e.restore_baseline(&params, &mut ScriptedSink::default())));
    assert_eq!(overwrite.hid(KEYCHRON), us());
}

#[test]
fn restore_of_one_device() {
    let (mut w, _) = keychron_jis_confirmed();
    World::ok(w.set(
        VXE,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    let params = restore_params(
        RestoreScope::Device {
            instance_id: KEYCHRON.to_string(),
        },
        ConflictPolicy::Report,
        RestoreMode::Interactive,
    );
    let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
    let result = World::ok(w.run(|e| e.restore_baseline(&params, &mut sink)));
    // HID only, no permission to reset: reconnect path, then kept.
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(w.hid(KEYCHRON), us());
    assert_eq!(w.hid(VXE), jis());
}

// ------------------------------------------------------------------------------------------
// Supersede (C7)
// ------------------------------------------------------------------------------------------

#[test]
fn restore_supersedes_a_pending_migration() {
    let (mut w, migration) = migrated_pending();
    let pristine = World::pre_m0();
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_eq!(result.outcome, Outcome::PendingReboot);
    let entry = w.entry(&migration);
    assert_eq!(entry.state, OpState::Failed);
    assert_eq!(
        entry.failure,
        Some(FailureReason::Superseded { by: op_of(&result) })
    );
    assert_eq!(
        values_of(&w.registry.contents()),
        values_of(&pristine.registry.contents())
    );
    let restore = w.entry(&op_of(&result));
    assert!(matches!(
        &restore.kind,
        OpKind::RestoreBaseline { supersedes, .. } if supersedes == &vec![migration.clone()]
    ));
}

#[test]
fn recovery_finishes_a_supersede_interrupted_after_planned() {
    let (w, migration) = migrated_pending();
    let pristine = World::pre_m0();
    // Crash right after `Planned` was flushed: J(Planned), FJ.
    let mut run = w.fork();
    run.registry.set_faults(FaultPlan {
        crash_after: Some(2),
        ..FaultPlan::default()
    });
    assert!(run.restore_all(RestoreMode::Interactive).is_err());
    let mut after = run.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
    let journal = after.journal();
    assert_eq!(
        journal.entry(&migration).map(|e| e.state),
        Some(OpState::PendingReboot)
    );
    let result = World::ok(after.recover());
    assert_eq!(result.recovered.len(), 1);
    assert_eq!(result.recovered[0].decision, "complete-forward");
    assert_eq!(result.recovered[0].to, OpState::PendingReboot);
    assert_eq!(after.entry(&migration).state, OpState::Failed);
    assert_eq!(
        values_of(&after.registry.contents()),
        values_of(&pristine.registry.contents())
    );
}

// ------------------------------------------------------------------------------------------
// Allowlist and INV-PS2
// ------------------------------------------------------------------------------------------

#[test]
fn standard_is_refused_for_ps2() {
    let mut w = World::dev_machine();
    let result = w.set(
        PS2,
        LayoutChoice::Standard,
        LIVE,
        &mut ScriptedSink::default(),
    );
    assert!(matches!(
        result,
        Err(EngineError::Operation(
            OperationError::StandardNotAllowed { .. }
        ))
    ));
    assert_eq!(w.registry.mutating_calls(), 0);
}

#[test]
fn fixed_mode_needs_a_migration_first() {
    let mut w = World::pre_m0();
    let result = w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    );
    assert!(matches!(
        result,
        Err(EngineError::Operation(
            OperationError::MigrationRequired { .. }
        ))
    ));
    let error = result.unwrap_err();
    assert_eq!(error.to_info().code, ErrorCode::MigrationRequired);
}

#[test]
fn migrate_pins_phantom_ps2_keyboards_too() {
    let mut w = World::pre_m0();
    w.devices.add_keyboard(dock_ps2());
    World::ok(w.migrate(Layout::Jis, &[]));
    assert_eq!(w.ps2(&device(DOCK_PS2)), jis());
    assert_eq!(w.ps2(&device(PS2)), jis());
    assert_eq!(w.ps2(&WriteTarget::Global), absent());
}

#[test]
fn an_unpinned_ps2_keyboard_blocks_even_hid_changes() {
    let mut w = World::dev_machine();
    w.devices.add_keyboard(dock_ps2());
    let result = w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    );
    assert!(matches!(
        result,
        Err(EngineError::Operation(OperationError::Plan(
            PlanError::InvPs2(_)
        )))
    ));
    assert_eq!(w.registry.mutating_calls(), 0);
}

#[test]
fn a_tampered_record_name_is_refused() {
    let (mut w, op) = keychron_jis_confirmed();
    let mut entry: JournalEntry = w.entry(&op);
    entry.records[0].name = "Start".to_string();
    let json = entry.to_json().unwrap();
    w.registry
        .write_journal(&mklm_engine::JournalSlot::Op(op.clone()), &json)
        .unwrap();
    let result = w.revert(&op, NO_RESET);
    assert!(matches!(
        result,
        Err(EngineError::Restore(RestoreError::NameNotAllowed { .. }))
    ));
    assert_eq!(w.hid(KEYCHRON), jis());
}

#[test]
fn an_unpinned_phantom_after_written_rolls_the_migration_back() {
    let w = World::pre_m0();
    // Run the migration up to (and including) the `Written` flush, then stop.
    let mut probe = w.fork();
    World::ok(probe.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)]));
    let calls = probe.registry.mutating_calls();
    // The last two calls are J(PendingReboot) and its flush.
    let mut run = w.fork();
    run.registry.set_faults(FaultPlan {
        crash_after: Some(calls - 2),
        ..FaultPlan::default()
    });
    assert!(
        run.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)])
            .is_err()
    );
    let mut after = run.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
    let op = after.journal().entries[0].op_id.clone();
    assert_eq!(after.entry(&op).state, OpState::Written);
    after.devices.add_keyboard(dock_ps2());
    let result = World::ok(after.recover());
    assert_eq!(result.recovered[0].decision, "roll-back");
    let entry = after.entry(&op);
    assert_eq!(entry.state, OpState::RevertedPendingReboot);
    assert_eq!(entry.failure, Some(FailureReason::Interrupted));
    assert_eq!(after.ps2(&WriteTarget::Global), jis());
}

#[test]
fn an_unpinned_phantom_after_the_restart_is_a_conflict() {
    let (mut w, op) = migrated_pending();
    w.devices.add_keyboard(dock_ps2());
    w.reboot();
    let result = World::ok(w.recover());
    assert_eq!(result.recovered[0].decision, "conflict");
    assert_eq!(w.entry(&op).state, OpState::Conflict);
    let violation = result.inv_ps2_violation.expect("INV-PS2 violation");
    assert_eq!(violation.keyboards, [DOCK_PS2.to_string()]);

    // Keeping the current values would leave the violation: refused (C12).
    let result = resolve(&mut w, &op, &[]);
    assert!(matches!(
        result,
        Err(EngineError::Restore(RestoreError::InvPs2(_)))
    ));
    assert_eq!(result.unwrap_err().to_info().code, ErrorCode::PlanRejected);
    // Going back to fixed mode is fine.
    let choices: Vec<(usize, ResolutionChoice)> = (0..w.entry(&op).records.len())
        .map(|i| (i, ResolutionChoice::UseBefore))
        .collect();
    let result = World::ok(resolve(&mut w, &op, &choices));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);
    assert_eq!(w.ps2(&WriteTarget::Global), jis());
}

#[test]
fn a_missing_layer_driver_stops_the_migration() {
    let mut w = World::pre_m0();
    w.host.remove_system32_file("kbd101.dll");
    let result = w.migrate(Layout::Us, &[]);
    assert!(matches!(
        result,
        Err(EngineError::Operation(
            OperationError::LayerDriverMissing { .. }
        ))
    ));
    assert_eq!(w.registry.mutating_calls(), 0);
    assert!(w.host.recovery_assets().is_none());
}

// ------------------------------------------------------------------------------------------
// Lock and pre-flight
// ------------------------------------------------------------------------------------------

#[test]
fn another_process_holding_the_lock_means_busy() {
    let mut w = World::dev_machine();
    let mut other = w.host.clone();
    let _held = other.acquire_lock(Duration::from_secs(1)).unwrap();
    let result = w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    );
    assert_eq!(result, Err(EngineError::Busy));
    assert_eq!(w.registry.mutating_calls(), 0);
    assert_eq!(EngineError::Busy.to_info().code, ErrorCode::Busy);
}

#[test]
fn an_open_operation_blocks_new_ones() {
    let (mut w, op) = migrated_pending();
    let mut w2 = w.fork();
    let result = w2.set(PS2, LayoutChoice::Us, LIVE, &mut ScriptedSink::default());
    assert!(matches!(result, Err(EngineError::OpInProgress { op_id, .. }) if op_id == op));
    assert!(matches!(
        w.migrate(Layout::Jis, &[]),
        Err(EngineError::OpInProgress { .. })
    ));
}

#[test]
fn an_unreadable_journal_stops_writes() {
    let mut w = World::dev_machine();
    let mut writer = w.registry.clone();
    let slot =
        mklm_engine::JournalSlot::Op(OpId::parse("0000000a-0000-4000-8000-000000000000").unwrap());
    writer
        .write_journal(&slot, "{\"schema_version\": 99}")
        .unwrap();
    let result = w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    );
    assert!(matches!(result, Err(EngineError::JournalUnreadable(_))));
    assert!(matches!(
        w.recover(),
        Err(EngineError::JournalUnreadable(_))
    ));
    assert_eq!(w.hid(KEYCHRON), us());
}

#[test]
fn an_incomplete_inventory_stops_writes() {
    let mut w = World::dev_machine();
    w.devices.set_incomplete(true);
    let result = w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::default(),
    );
    let error = result.unwrap_err();
    assert_eq!(error.to_info().code, ErrorCode::InventoryIncomplete);
    assert_eq!(w.registry.mutating_calls(), 0);
}

#[test]
fn read_warnings_do_not_stop_writes() {
    let (mut w, op) = keychron_jis_confirmed();
    w.devices
        .set_warnings(vec!["display name unreadable".to_string()]);
    w.host.push_warning("quarantined a squatted directory");
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert!(result.warnings.iter().any(|m| m.contains("display name")));
    assert!(result.warnings.iter().any(|m| m.contains("quarantined")));
    let result = World::ok(w.recover());
    assert!(result.warnings.iter().any(|m| m.contains("display name")));
    let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
    let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    assert_eq!(result.outcome, Outcome::Confirmed);
    // A REG_SZ override reads as `Other` and does not stop the engine either.
    w.registry.outside_edit(
        &device(VXE),
        value_names::HID_TYPE,
        RegValue::Other {
            reg_type: 1,
            data_hex: "340000".into(),
        },
    );
    let result = World::ok(w.set(
        VXE,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(result.outcome, Outcome::Confirmed);
}

#[test]
fn a_changed_plan_writes_nothing() {
    let mut w = World::dev_machine();
    // What the dry run showed: live reset.
    let keyboards = keyboards_of(&w);
    let global = w.registry.global_settings();
    let dry = mklm_core::plan_set_layout(&keyboards, &global, KEYCHRON, LayoutChoice::Jis, &LIVE)
        .unwrap();
    let approved = ExpectedPlan {
        steps: dry.checked.steps.clone(),
        apply: Some(dry.apply),
    };
    assert_eq!(dry.apply, PendingAction::ResetKeyboard);

    // Same plan: accepted.
    let mut ok = w.fork();
    let params = SetLayoutParams {
        expected: Some(approved.clone()),
        ..set_params(KEYCHRON, LayoutChoice::Jis, LIVE)
    };
    let result = World::ok(
        ok.run(|e| e.set_layout(&params, &mut ScriptedSink::new([ScriptedSink::keep()]))),
    );
    assert_eq!(result.outcome, Outcome::Confirmed);

    // The apply method changed (no reset allowed any more): refused.
    let params = SetLayoutParams {
        expected: Some(approved.clone()),
        ..set_params(KEYCHRON, LayoutChoice::Jis, NO_RESET)
    };
    let result = w.run(|e| e.set_layout(&params, &mut ScriptedSink::default()));
    assert!(matches!(result, Err(EngineError::PlanChanged { .. })));
    // The steps changed: refused.
    let params = SetLayoutParams {
        expected: Some(approved),
        ..set_params(KEYCHRON, LayoutChoice::Us, LIVE)
    };
    let result = w.run(|e| e.set_layout(&params, &mut ScriptedSink::default()));
    assert!(matches!(result, Err(EngineError::PlanChanged { .. })));
    assert_eq!(w.registry.mutating_calls(), 0);
}

#[test]
fn an_in_flight_entry_left_by_this_process_is_recovered() {
    let w = World::dev_machine();
    let mut run = w.fork();
    run.registry.set_faults(FaultPlan {
        crash_after: Some(6),
        ..FaultPlan::default()
    });
    assert!(
        run.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::default()
        )
        .is_err()
    );
    // Same process, same boot: the entry found under the lock is still abandoned (C3).
    let mut same = run.fork();
    let op = same.journal().entries[0].op_id.clone();
    assert!(same.entry(&op).state.is_in_flight());
    let mut other = same.fork();
    assert!(matches!(
        other.set(
            KEYCHRON,
            LayoutChoice::Us,
            LIVE,
            &mut ScriptedSink::default()
        ),
        Err(EngineError::RecoveryNeeded { .. })
    ));
    World::ok(same.recover());
    assert!(!same.entry(&op).state.is_open());
    assert_eq!(same.hid(KEYCHRON), us());
}

#[test]
fn nothing_to_change_is_no_change() {
    let mut w = World::dev_machine();
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Us,
        LIVE,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(result.outcome, Outcome::NoChange);
    assert_eq!(result.op_id, None);
    assert_eq!(w.registry.mutating_calls(), 0);
    let result = World::ok(w.recover());
    assert_eq!(result.outcome, Outcome::Recovered);
    assert!(result.recovered.is_empty());
    assert!(matches!(
        w.set(
            "HID\\NOPE",
            LayoutChoice::Us,
            LIVE,
            &mut ScriptedSink::default()
        ),
        Err(EngineError::Operation(
            OperationError::UnknownKeyboard { .. }
        ))
    ));
    let unknown = OpId::parse("0000000b-0000-4000-8000-000000000000").unwrap();
    assert!(matches!(
        w.revert(&unknown, NO_RESET),
        Err(EngineError::UnknownOp { .. })
    ));
}

// ------------------------------------------------------------------------------------------
// Recovery files (C10)
// ------------------------------------------------------------------------------------------

#[test]
fn recovery_files_are_required_for_boot_time_values_only() {
    let mut w = World::pre_m0();
    w.host.set_fail_assets(true);
    let result = w.migrate(Layout::Jis, &[]);
    assert!(matches!(
        result,
        Err(EngineError::RecoveryAssetsUnavailable(_))
    ));
    assert_eq!(w.registry.mutating_calls(), 0);
    assert!(w.journal().entries.is_empty());

    let mut w = World::dev_machine();
    w.host.set_fail_assets(true);
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert!(result.warnings.iter().any(|m| m.contains("recovery files")));
}

#[test]
fn recovery_files_list_the_baselines() {
    let (w, _) = migrated_pending();
    let assets = w.host.recovery_assets().expect("recovery files");
    assert!(assets.cmd.contains(r"Services\i8042prt\Parameters"));
    assert!(
        assets
            .cmd
            .contains(r"Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters")
    );
    assert!(assets.cmd.contains("KeyboardTypeOverride"));
}

// ------------------------------------------------------------------------------------------
// Pruning (C.9)
// ------------------------------------------------------------------------------------------

#[test]
fn pruning_keeps_the_newest_the_latest_and_the_pending() {
    let mut w = World::dev_machine();
    let keep = || ScriptedSink::new([ScriptedSink::keep()]);
    // Two BLE changes kept before a reconnect: both keep `apply_pending`.
    let ble_old = op_of(&World::ok(w.set(VXE, LayoutChoice::Jis, LIVE, &mut keep())));
    let ble_new = op_of(&World::ok(w.set(VXE, LayoutChoice::Us, LIVE, &mut keep())));
    assert!(w.entry(&ble_old).apply_pending.is_some());
    // 40 Keychron changes.
    let mut last = None;
    for i in 0..40 {
        let layout = if i % 2 == 0 {
            LayoutChoice::Jis
        } else {
            LayoutChoice::Us
        };
        last = Some(op_of(&World::ok(w.set(
            KEYCHRON,
            layout,
            LIVE,
            &mut keep(),
        ))));
    }
    let journal = w.journal();
    assert_eq!(journal.entries.len(), 32 + 2);
    assert!(
        journal.entry(&ble_old).is_some(),
        "apply_pending still applies"
    );
    assert!(
        journal.entry(&ble_new).is_some(),
        "latest record of the VXE values"
    );
    assert!(journal.entry(&last.unwrap()).is_some());
    // Reconnected: the old one has nothing left to keep it.
    w.devices.reconnect(VXE);
    World::ok(w.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut keep()));
    let journal = w.journal();
    assert!(journal.entry(&ble_old).is_none());
    assert!(journal.entry(&ble_new).is_some());
}

// ------------------------------------------------------------------------------------------
// Recovery with resets (C1, C9)
// ------------------------------------------------------------------------------------------

#[test]
fn recovery_resets_keyboards_only_when_allowed() {
    // A countdown interrupted by a crash: the Keychron runs JIS, the values go back to US.
    let w = World::dev_machine();
    let mut probe = w.fork();
    let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
    World::ok(probe.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink));
    // Crash before the Confirmed journal write (J, FJ, then pruning makes no calls).
    let calls = probe.registry.mutating_calls();
    let crashed = |apply: ApplyOptions| {
        let mut run = w.fork();
        run.registry.set_faults(FaultPlan {
            crash_after: Some(calls - 2),
            ..FaultPlan::default()
        });
        let mut sink = ScriptedSink::new([ScriptedSink::keep()]);
        assert!(
            run.set(KEYCHRON, LayoutChoice::Jis, LIVE, &mut sink)
                .is_err()
        );
        let mut after = run.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
        assert_eq!(
            after.devices.running_type(KEYCHRON),
            Some(KeyboardType::JIS)
        );
        let result = World::ok(after.run(|e| e.recover(&apply, &mut ScriptedSink::default())));
        (after, result)
    };
    let (after, result) = crashed(NO_RESET);
    assert_eq!(result.recovered[0].decision, "roll-back");
    assert_eq!(after.hid(KEYCHRON), us());
    let op = result.recovered[0].op_id.clone();
    let entry = after.entry(&op);
    assert_eq!(entry.failure, Some(FailureReason::CountdownExpired));
    assert_eq!(
        entry.apply_pending.map(|p| p.action),
        Some(PendingAction::Reconnect)
    );
    assert_eq!(result.pending_action, Some(PendingAction::Reconnect));

    let (after, result) = crashed(LIVE);
    assert_eq!(after.devices.running_type(KEYCHRON), Some(KeyboardType::US));
    assert_eq!(after.entry(&op).apply_pending, None);
    assert_eq!(result.pending_action, None);
}

#[test]
fn revert_with_permission_resets_the_keyboard() {
    let (mut w, op) = keychron_jis_confirmed();
    let result = World::ok(w.revert(&op, LIVE));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(result.pending_action, None);
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::US));

    let (mut w, op) = keychron_jis_confirmed();
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(result.pending_action, Some(PendingAction::ResetKeyboard));
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::JIS));

    let (mut w, op) = keychron_jis_confirmed();
    w.devices.set_reset(KEYCHRON, FakeReset::NeverArrives);
    let result = World::ok(w.revert(&op, LIVE));
    assert_eq!(result.outcome, Outcome::RevertedPendingReboot);
    assert_eq!(result.pending_action, Some(PendingAction::RestartPc));
}

// ------------------------------------------------------------------------------------------
// Deny target (C3, I7)
// ------------------------------------------------------------------------------------------

#[test]
fn a_write_that_keeps_failing_ends_in_conflict_once() {
    let (w, op) = keychron_jis_confirmed();
    let mut w = w.fork();
    w.registry.set_faults(FaultPlan {
        deny_target: Some(device(KEYCHRON)),
        ..FaultPlan::default()
    });
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, Outcome::Conflict);
    assert!(result.conflicts[0].write_error.is_some());
    let entry = w.entry(&op);
    assert!(entry.records.iter().any(|r| r.write_error.is_some()));
    assert_eq!(
        mklm_core::attention(&entry, w.host.current_boot(), mklm_core::Liveness::Dead),
        mklm_core::Attention::Conflict
    );
    w.registry.set_faults(FaultPlan {
        deny_target: Some(device(KEYCHRON)),
        ..FaultPlan::default()
    });
    World::ok(w.recover());
    assert_eq!(
        w.registry.mutating_calls(),
        0,
        "recovery does not retry a conflict"
    );
}

#[test]
fn journal_reads_without_the_lock() {
    let (w, op) = keychron_jis_confirmed();
    let mut other = w.host.clone();
    let _held = other.acquire_lock(Duration::from_secs(1)).unwrap();
    let journal = w.engine().read_journal().unwrap();
    assert!(journal.entry(&op).is_some());
}

#[test]
fn a_change_put_back_later_does_not_block_reverting_the_one_before() {
    // C.8: only the operation that last changed a value may revert it. A later `set` whose
    // countdown ran out put the value back where the first one left it: no net change.
    let mut w = World::dev_machine();
    let original = w.hid(KEYCHRON);
    let first = op_of(&World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    )));
    let second = op_of(&World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Us,
        LIVE,
        &mut ScriptedSink::default(),
    )));
    assert_eq!(w.entry(&second).state, OpState::Reverted);
    assert_eq!(w.hid(KEYCHRON), jis());
    World::ok(w.revert(&first, NO_RESET));
    assert_eq!(w.hid(KEYCHRON), original);

    // A later change that is still in effect keeps blocking it.
    let mut w = World::dev_machine();
    let first = op_of(&World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    )));
    World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Us,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert!(matches!(
        w.revert(&first, NO_RESET),
        Err(EngineError::NotLatest { .. })
    ));
}

#[test]
fn recovery_files_moved_by_a_quarantine_are_written_again() {
    // Design review S1 / G.1: a quarantine of the base directory takes `Recovery` with it; the
    // next request puts the recovery files back while baselines exist.
    let mut w = World::pre_m0();
    World::ok(w.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)]));
    let written = w.host.recovery_asset_writes();
    assert!(w.host.recovery_assets().is_some());
    w.host.quarantine_base();
    assert!(w.host.recovery_assets().is_none());
    let result = World::ok(w.recover());
    assert!(
        result.warnings.iter().any(|m| m.contains("quarantined")),
        "{result:?}"
    );
    assert_eq!(w.host.recovery_asset_writes(), written + 1);
    let assets = w.host.recovery_assets().expect("written again");
    assert!(assets.cmd.contains("i8042prt"), "{}", assets.cmd);
    // Once only.
    World::ok(w.recover());
    assert_eq!(w.host.recovery_asset_writes(), written + 1);

    // Nothing to put back without baselines.
    let mut clean = World::dev_machine();
    clean.host.quarantine_base();
    World::ok(clean.recover());
    assert_eq!(clean.host.recovery_asset_writes(), 0);
}
