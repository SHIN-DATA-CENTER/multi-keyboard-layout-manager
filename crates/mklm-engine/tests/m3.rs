//! The engine's M3 additions (design m3 A.5, H.1 "mklm-engine (WP-E1, E2, E3)"): deleting the
//! values a keyboard's driver does not read (`cleanup_values`), the machine-wide settings, the
//! length of the countdown, what a session end does to a running request, and the schema-1
//! journal of the development machine. The exhaustive crash tests of the cleanup are in
//! `crash.rs`.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use mklm_core::value_names::{HID_SUBTYPE, HID_TOTAL_KEYS, HID_TYPE, PS2_SUBTYPE, PS2_TYPE};
use mklm_core::{
    AllowlistError, ApplyOptions, Attention, ConflictPolicy, ErrorCode, Event, FailureReason,
    JournalEntry, KeyboardType, LayoutChoice, Liveness, OpId, OpKind, OpState, OperationError,
    Outcome, PlanError, RESTORE_ON_UNINSTALL_VALUE, RegValue, RestoreScope, attention, fixtures,
};
use mklm_engine::memory::{CrashImage, FaultPlan, Hive, MemoryRegistry, Mutation, ScriptedSink};
use mklm_engine::{
    DecisionPoll, EngineError, EventSink, Host, JournalSlot, MachineSettingsParams,
    RegistryBackend, ResolveParams, RestoreMode, SessionEnd, SessionEndSink,
};

fn op_of(result: &mklm_core::OperationResult) -> OpId {
    result.op_id.clone().expect("the result names an operation")
}

fn jis() -> (RegValue, RegValue) {
    (dword(7), dword(2))
}

fn us() -> (RegValue, RegValue) {
    (dword(4), dword(0))
}

fn absent() -> (RegValue, RegValue) {
    (RegValue::Absent, RegValue::Absent)
}

fn stored_json(w: &World, op: &OpId) -> String {
    w.registry
        .contents()
        .ops
        .get(op.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("{op} is not stored"))
}

fn no_in_flight(w: &World) {
    for entry in &w.journal().entries {
        assert!(!entry.state.is_in_flight(), "{} in flight", entry.op_id);
        assert!(
            !(entry.state == OpState::AwaitingConfirm && entry.countdown.is_some()),
            "{} counting down",
            entry.op_id
        );
    }
    assert!(!w.lock.is_locked(), "the lock was not released");
}

/// The development machine with a guide's advice gone wrong: the HID pair 7/2 written to the
/// built-in PS/2 keyboard, where i8042prt never reads it (design m3 B.1, "HID の名前が ACPI にある").
fn ps2_with_hid_values() -> World {
    let w = World::dev_machine();
    w.registry.seed(&device(PS2), HID_TYPE, dword(7));
    w.registry.seed(&device(PS2), HID_SUBTYPE, dword(2));
    w
}

/// The other way round: the PS/2 pair on the Keychron, which kbdhid never reads.
fn keychron_with_ps2_values() -> World {
    let w = World::dev_machine();
    w.registry.seed(&device(KEYCHRON), PS2_TYPE, dword(7));
    w.registry.seed(&device(KEYCHRON), PS2_SUBTYPE, dword(2));
    w
}

// ------------------------------------------------------------------------------------------
// WP-E1: CleanupValues
// ------------------------------------------------------------------------------------------

#[test]
fn a_cleanup_deletes_what_the_driver_does_not_read_and_waits_for_the_user() {
    let mut w = ps2_with_hid_values();
    let mut sink = ScriptedSink::default();
    let params = mklm_engine::CleanupParams {
        instance_id: PS2.to_string(),
        names: vec![HID_TYPE.to_string(), HID_SUBTYPE.to_string()],
    };
    let result = World::ok(w.run(|e| e.cleanup_values(&params, &mut sink)));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(result.failure, None);
    assert_eq!(result.pending_action, None, "nothing has to take effect");
    assert_eq!(w.hid(PS2), absent());
    // The pin i8042prt reads is untouched; so is what the keyboard types with.
    assert_eq!(w.ps2(&device(PS2)), jis());
    assert_eq!(w.devices.running_type(PS2), Some(KeyboardType::JIS));
    assert!(w.devices.restarted().is_empty(), "no reset of anything");

    let op = op_of(&result);
    let entry = w.entry(&op);
    assert_eq!(
        entry.kind,
        OpKind::Cleanup {
            instance_id: PS2.to_string(),
            names: vec![HID_TYPE.to_string(), HID_SUBTYPE.to_string()],
        }
    );
    assert_eq!(entry.state, OpState::AwaitingConfirm);
    assert_eq!((entry.apply, entry.countdown), (None, None));
    assert_eq!(entry.apply_pending, None);
    assert_eq!(entry.schema_version, 2);
    assert!(stored_json(&w, &op).starts_with("{\"schema_version\":2,"));
    // Baselines as for every change: "restore to baseline" puts the values back.
    let journal = w.journal();
    let baseline = |name: &str| {
        journal
            .baseline(&mklm_core::ValueKey {
                target: device(PS2),
                name: name.to_string(),
            })
            .map(|b| b.value.clone())
    };
    assert_eq!(baseline(HID_TYPE), Some(dword(7)));
    assert_eq!(baseline(HID_SUBTYPE), Some(dword(2)));
    // C.5 order and the events: plan, one step, `Written`, `AwaitingConfirm`; no countdown.
    assert_eq!(sink.events[0], Event::Locked);
    let planned = sink
        .events
        .iter()
        .find_map(|e| match e {
            Event::Planned {
                apply, keyboards, ..
            } => Some((*apply, keyboards.clone())),
            _ => None,
        })
        .expect("Planned");
    assert_eq!(planned.0, None);
    assert_eq!(planned.1.len(), 1);
    assert!(!planned.1[0].changes, "the keyboard types as before");
    let states: Vec<OpState> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            Event::StateChanged { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(states, [OpState::Written, OpState::AwaitingConfirm]);
    assert_eq!(
        sink.count(|e| matches!(
            e,
            Event::CountdownStarted { .. } | Event::ResettingKeyboard { .. }
        )),
        0
    );
    assert_eq!(
        attention(&entry, w.host.current_boot(), Liveness::Dead),
        Attention::AwaitingUser
    );
    no_in_flight(&w);

    // The user keeps it: nothing to check on Raw Input, nothing pending.
    let kept = World::ok(w.confirm(&op));
    assert_eq!(kept.outcome, Outcome::Confirmed);
    assert_eq!(kept.pending_action, None);
    assert!(kept.warnings.is_empty(), "{:?}", kept.warnings);
    let entry = w.entry(&op);
    assert_eq!(entry.state, OpState::Confirmed);
    assert_eq!(entry.apply_pending, None);
    assert_eq!(
        attention(&entry, w.host.current_boot(), Liveness::Dead),
        Attention::None
    );
}

#[test]
fn a_cleanup_is_reverted_undone_and_restored_like_any_change() {
    // Revert: back without a restart (nothing any driver reads) and without a reset.
    let mut w = ps2_with_hid_values();
    let op = op_of(&World::ok(w.cleanup(PS2, &[HID_TYPE, HID_SUBTYPE])));
    let result = World::ok(w.revert(&op, LIVE));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!((result.failure, result.pending_action), (None, None));
    assert_eq!(w.hid(PS2), jis());
    assert!(
        w.devices.restarted().is_empty(),
        "the PS/2 keyboard is never reset"
    );
    assert_eq!(w.entry(&op).apply_pending, None);
    no_in_flight(&w);

    // Undo (the way out of every open entry).
    let mut w = ps2_with_hid_values();
    let op = op_of(&World::ok(w.cleanup(PS2, &[HID_TYPE, HID_SUBTYPE])));
    World::ok(w.undo());
    assert_eq!(w.entry(&op).state, OpState::Reverted);
    assert_eq!(w.hid(PS2), jis());

    // Kept, then "MKLM 導入前に戻す": the values come back, again waiting for the user only.
    let mut w = ps2_with_hid_values();
    let op = op_of(&World::ok(w.cleanup(PS2, &[HID_TYPE, HID_SUBTYPE])));
    World::ok(w.confirm(&op));
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(result.pending_action, None);
    assert_eq!(w.hid(PS2), jis());
    assert!(w.devices.restarted().is_empty());
    let restore = op_of(&result);
    let entry = w.entry(&restore);
    assert_eq!((entry.apply, entry.countdown), (None, None));
    assert_eq!(entry.schema_version, 1, "a restore stays in schema 1");
    let kept = World::ok(w.confirm(&restore));
    assert_eq!(kept.outcome, Outcome::Confirmed);
    assert!(w.journal().baselines.is_empty(), "back at the baselines");
    no_in_flight(&w);
}

/// A restore to baseline that only puts back values no driver reads leaves no keyboard waiting
/// (m2 D.11), also for a keyboard that is not present (no Raw Input report can clear it) and
/// when a crash interrupts the restore and recovery closes it.
#[test]
fn a_restore_of_values_no_driver_reads_leaves_nothing_to_apply() {
    let phantom = fixtures::ms_ble_phantom();
    let base = {
        let mut w = World::dev_machine();
        w.registry
            .seed(&device(&phantom.instance_id), PS2_TYPE, dword(7));
        let op = op_of(&World::ok(w.cleanup(&phantom.instance_id, &[PS2_TYPE])));
        World::ok(w.confirm(&op));
        w.fork()
    };

    let mut w = base.fork();
    let result = World::ok(w.restore_all(RestoreMode::Interactive));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(result.pending_action, None);
    assert_eq!(w.value(&device(&phantom.instance_id), PS2_TYPE), dword(7));
    let restore = op_of(&result);
    let entry = w.entry(&restore);
    assert_eq!((entry.apply, entry.apply_pending), (None, None));
    let kept = World::ok(w.confirm(&restore));
    assert_eq!(
        (kept.outcome, kept.pending_action),
        (Outcome::Confirmed, None)
    );
    assert_eq!(w.entry(&restore).apply_pending, None);

    // Reverted instead of kept: the same.
    let mut w = base.fork();
    let restore = op_of(&World::ok(w.restore_all(RestoreMode::Interactive)));
    let back = World::ok(w.revert(&restore, NO_RESET));
    assert_eq!(
        (back.outcome, back.pending_action),
        (Outcome::Reverted, None)
    );
    assert_eq!(w.entry(&restore).apply_pending, None);

    // A crash at every point of the restore, then recovery.
    let total = {
        let mut dry = base.fork();
        World::ok(dry.restore_all(RestoreMode::Interactive));
        dry.registry.mutating_calls()
    };
    for n in 0..total {
        let mut run = base.fork();
        run.registry.set_faults(FaultPlan {
            crash_after: Some(n),
            ..Default::default()
        });
        let _ = run.restore_all(RestoreMode::Interactive);
        let mut after = run.after_crash(CrashImage::ProcessKill, false);
        let recovered = World::ok(after.recover());
        assert_eq!(recovered.pending_action, None, "crash after {n}");
        for entry in &after.journal().entries {
            assert_eq!(
                entry.apply_pending, None,
                "crash after {n}: {} {:?}",
                entry.op_id, entry.state
            );
        }
    }
}

#[test]
fn a_cleanup_of_the_ps2_pair_on_a_hid_keyboard_needs_no_restart() {
    let mut w = keychron_with_ps2_values();
    // No boot-time value changes (kbdhid reads neither name): the recovery files are not
    // required, a failure to write them is a warning.
    w.host.set_fail_assets(true);
    let result = World::ok(w.cleanup(KEYCHRON, &[PS2_TYPE, PS2_SUBTYPE]));
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(result.pending_action, None);
    assert!(
        result
            .warnings
            .iter()
            .any(|m| m.contains("recovery files could not be written")),
        "{result:?}"
    );
    assert_eq!(w.ps2(&device(KEYCHRON)), absent());
    assert_eq!(w.hid(KEYCHRON), us());
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::US));
    let op = op_of(&result);
    assert!(!w.entry(&op).touches_boot_time_values());

    let result = World::ok(w.revert(&op, LIVE));
    assert_eq!(result.outcome, Outcome::Reverted, "no restart is needed");
    assert_eq!(result.pending_action, None);
    assert_eq!(w.ps2(&device(KEYCHRON)), jis());
    assert!(w.devices.restarted().is_empty(), "nothing to re-apply");
}

#[test]
fn a_cleanup_refuses_what_a_driver_reads_and_anything_outside_the_keyboard_class() {
    let base = ps2_with_hid_values();
    let refused = |instance_id: &str, names: &[&str]| {
        let mut w = base.fork();
        let error = w
            .cleanup(instance_id, names)
            .expect_err("refused before anything is written");
        assert_eq!(w.registry.mutating_calls(), 0, "{names:?}");
        assert!(w.journal().entries.is_empty(), "{names:?}");
        assert!(!w.lock.is_locked());
        error
    };
    let device_error = |error: EngineError| match error {
        EngineError::Operation(OperationError::Plan(PlanError::Device { error, .. })) => error,
        other => panic!("not an allowlist refusal: {other:?}"),
    };
    // The pair each driver reads: INV-PS2 and the layouts depend on it.
    for (keyboard, name) in [
        (PS2, PS2_TYPE),
        (PS2, PS2_SUBTYPE),
        (KEYCHRON, HID_TYPE),
        (KEYCHRON, HID_SUBTYPE),
    ] {
        let error = refused(keyboard, &[name]);
        assert_eq!(error.to_info().code, ErrorCode::PlanRejected);
        assert!(
            matches!(
                device_error(error),
                AllowlistError::OperationNotAllowed { .. }
            ),
            "{keyboard} {name}"
        );
    }
    // Names outside the pair, whatever the driver.
    for name in [
        HID_TOTAL_KEYS,
        "Start",
        "UpperFilters",
        "keyboardtypeoverride",
    ] {
        assert_eq!(
            device_error(refused(PS2, &[name])),
            AllowlistError::ValueNotAllowed { name: name.into() }
        );
    }
    // A devnode outside the Keyboard class (the receiver's mouse collection) is not in the
    // inventory, so it is no keyboard to clean.
    let mouse = r"HID\VID_3434&PID_D027&MI_01&COL03\8&2&0&0002";
    let error = refused(mouse, &[PS2_TYPE]);
    assert!(matches!(
        error,
        EngineError::Operation(OperationError::UnknownKeyboard { .. })
    ));
    assert_eq!(error.to_info().code, ErrorCode::UnknownKeyboard);

    // INV-PS2 already broken (a dock's PS/2 keyboard without a pin): refused like any plan.
    let mut broken = base.fork();
    broken.devices.add_keyboard(dock_ps2());
    assert!(matches!(
        broken.cleanup(PS2, &[HID_TYPE]),
        Err(EngineError::Operation(OperationError::Plan(
            PlanError::InvPs2(_)
        )))
    ));
    assert_eq!(broken.registry.mutating_calls(), 0);

    // An open entry blocks it, and it blocks the others until kept or reverted.
    let mut open = base.fork();
    let op = op_of(&World::ok(open.cleanup(PS2, &[HID_TYPE])));
    assert!(matches!(
        open.cleanup(PS2, &[HID_SUBTYPE]),
        Err(EngineError::OpInProgress { op_id, .. }) if op_id == op
    ));
    assert!(matches!(
        open.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::default()
        ),
        Err(EngineError::OpInProgress { .. })
    ));

    // Nothing to delete: no change, nothing journaled.
    let mut clean = World::dev_machine();
    let result = World::ok(clean.cleanup(PS2, &[HID_TYPE, HID_SUBTYPE]));
    assert_eq!(result.outcome, Outcome::NoChange);
    assert!(clean.journal().entries.is_empty());
    assert_eq!(clean.registry.mutating_calls(), 0);
}

// ------------------------------------------------------------------------------------------
// WP-E2: SetMachineSettings
// ------------------------------------------------------------------------------------------

fn set_machine(
    w: &mut World,
    restore: bool,
    sink: &mut ScriptedSink,
) -> Result<mklm_core::OperationResult, EngineError> {
    let params = MachineSettingsParams {
        restore_on_uninstall: restore,
    };
    w.run(|e| e.set_machine_settings(&params, sink))
}

#[test]
fn machine_settings_are_written_under_the_lock_and_flushed() {
    let mut w = World::dev_machine();
    let mut sink = ScriptedSink::default();
    let result = World::ok(set_machine(&mut w, false, &mut sink));
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(result.op_id, None);
    assert_eq!(sink.events, vec![Event::Locked]);
    assert_eq!(
        w.registry.machine_setting(RESTORE_ON_UNINSTALL_VALUE),
        Some(0)
    );
    assert_eq!(
        w.registry.mutations(),
        vec![
            Mutation::WriteSetting {
                name: RESTORE_ON_UNINSTALL_VALUE.to_string(),
                value: 0
            },
            Mutation::Flush {
                hive: Hive::Software
            },
        ]
    );
    assert!(w.journal().entries.is_empty(), "not journaled");
    assert!(!w.lock.is_locked());
    World::ok(set_machine(&mut w, true, &mut ScriptedSink::default()));
    assert_eq!(
        w.registry.machine_setting(RESTORE_ON_UNINSTALL_VALUE),
        Some(1)
    );

    // Another MKLM process holds the lock: busy, nothing written.
    let mut busy = w.fork();
    let mut other = busy.host.clone();
    let _held = other.acquire_lock(Duration::from_secs(1)).unwrap();
    assert_eq!(
        set_machine(&mut busy, false, &mut ScriptedSink::default()),
        Err(EngineError::Busy)
    );
    assert_eq!(busy.registry.mutating_calls(), 0);

    // Open entries and an unreadable journal do not stop it (no keyboard value is touched).
    let mut open = ps2_with_hid_values();
    World::ok(open.cleanup(PS2, &[HID_TYPE]));
    World::ok(set_machine(&mut open, false, &mut ScriptedSink::default()));
    assert_eq!(
        open.registry.machine_setting(RESTORE_ON_UNINSTALL_VALUE),
        Some(0)
    );
    let mut unreadable = World::dev_machine();
    let newer = OpId::parse("00000009-0000-4000-8000-000000000000").unwrap();
    unreadable
        .registry
        .write_journal(&JournalSlot::Op(newer), "{\"schema_version\": 99}")
        .unwrap();
    World::ok(set_machine(
        &mut unreadable,
        true,
        &mut ScriptedSink::default(),
    ));
    assert_eq!(
        unreadable
            .registry
            .machine_setting(RESTORE_ON_UNINSTALL_VALUE),
        Some(1)
    );

    // Only the allowlisted names reach the store (both backends refuse others).
    let mut registry = MemoryRegistry::new();
    for name in ["Start", "InstallDir", ""] {
        assert_eq!(
            registry.write_machine_setting(name, 1),
            Err(mklm_engine::BackendError::NameNotAllowed {
                name: name.to_string()
            })
        );
    }
    assert_eq!(
        registry.write_machine_setting("restoreonuninstall", 0),
        Ok(())
    );
    assert_eq!(
        registry.machine_setting(RESTORE_ON_UNINSTALL_VALUE),
        Some(0)
    );
}

// ------------------------------------------------------------------------------------------
// WP-E3: the countdown's length
// ------------------------------------------------------------------------------------------

#[test]
fn the_countdown_lasts_20_or_60_seconds_as_requested() {
    for seconds in [20, 60] {
        let mut w = World::dev_machine();
        // A sink that takes one real second per wait: the monotonic bound (seconds + slack)
        // must not cut a 60 s countdown at 25 s.
        let mut sink = ScriptedSink::default().with_clock(w.host.clock(), Duration::from_secs(1));
        let apply = ApplyOptions {
            countdown_seconds: seconds,
            ..LIVE
        };
        let result = World::ok(w.set(KEYCHRON, LayoutChoice::Jis, apply, &mut sink));
        assert_eq!(result.failure, Some(FailureReason::CountdownExpired));
        let op = op_of(&result);
        assert!(sink.events.contains(&Event::CountdownStarted {
            op_id: op.clone(),
            seconds,
            verified: true,
        }));
        assert_eq!(
            sink.count(|e| matches!(e, Event::CountdownTick { .. })),
            usize::try_from(seconds).unwrap()
        );
        assert_eq!(w.hid(KEYCHRON), us());
    }
    // The journal records the length while it counts (what recovery and the GUI see).
    let mut w = World::dev_machine();
    let registry = w.registry.clone();
    let mut sink = JournalPeek {
        registry,
        seen: None,
    };
    let apply = ApplyOptions {
        countdown_seconds: 60,
        ..LIVE
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, apply);
    World::ok(w.run(|e| e.set_layout(&params, &mut sink)));
    assert_eq!(sink.seen, Some(60));
}

/// Answers nothing; on its first wait it reads the counting-down entry from the store.
struct JournalPeek {
    registry: MemoryRegistry,
    seen: Option<u32>,
}

impl EventSink for JournalPeek {
    fn event(&mut self, _event: &Event) {}

    fn check_cancelled(&mut self) -> bool {
        false
    }

    fn wait_decision(&mut self, _timeout: Duration) -> DecisionPoll {
        if self.seen.is_none() {
            self.seen = journal_of(&self.registry)
                .entries
                .iter()
                .find_map(|e| e.countdown.map(|c| c.seconds));
        }
        DecisionPoll::NoDecision
    }
}

#[test]
fn any_other_countdown_length_is_refused_before_anything_happens() {
    for seconds in [0, 15, 30, 59, 61, u32::MAX] {
        let apply = ApplyOptions {
            countdown_seconds: seconds,
            ..LIVE
        };
        let mut w = World::dev_machine();
        let expected = Err(EngineError::CountdownNotAllowed { seconds });
        let mut sink = ScriptedSink::default();
        assert_eq!(
            w.set(KEYCHRON, LayoutChoice::Jis, apply, &mut sink),
            expected
        );
        assert!(sink.events.is_empty(), "not even locked");
        assert_eq!(
            EngineError::CountdownNotAllowed { seconds }.to_info().code,
            ErrorCode::PlanRejected
        );
        let any_op = OpId::parse("00000001-0000-4000-8000-000000000000").unwrap();
        assert_eq!(w.revert(&any_op, apply), expected);
        assert_eq!(
            w.run(|e| e.recover(&apply, &mut ScriptedSink::default())),
            expected
        );
        assert_eq!(
            w.run(|e| e.undo_open(&apply, &mut ScriptedSink::default())),
            expected
        );
        let restore = mklm_engine::RestoreBaselineParams {
            apply,
            ..restore_params(
                RestoreScope::All,
                ConflictPolicy::Report,
                RestoreMode::Interactive,
            )
        };
        assert_eq!(
            w.run(|e| e.restore_baseline(&restore, &mut ScriptedSink::default())),
            expected
        );
        let resolve = ResolveParams {
            op_id: any_op,
            choices: Vec::new(),
            apply,
        };
        assert_eq!(
            w.run(|e| e.resolve_conflict(&resolve, &mut ScriptedSink::default())),
            expected
        );
        assert_eq!(w.registry.mutating_calls(), 0);
        assert!(!w.lock.is_locked());
    }
}

// ------------------------------------------------------------------------------------------
// WP-E3: the end of the Windows session
// ------------------------------------------------------------------------------------------

/// A caller that never answers (the user is signing out): each wait really lasts until the
/// session ends or its timeout passes.
struct Silent {
    guard: Arc<SessionEnd>,
    events: Vec<Event>,
}

impl EventSink for Silent {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn check_cancelled(&mut self) -> bool {
        false
    }

    fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
        let start = Instant::now();
        while start.elapsed() < timeout && !self.guard.is_ending() {
            thread::sleep(Duration::from_millis(1));
        }
        DecisionPoll::NoDecision
    }
}

/// `WM_QUERYENDSESSION` then `WM_ENDSESSION(TRUE)` on the window thread, sent while the
/// countdown runs (the helper's window handler makes exactly these calls). Returns what the two
/// waits reported, `None` when no countdown started.
fn end_the_session_during_the_countdown(
    guard: &Arc<SessionEnd>,
) -> thread::JoinHandle<Option<(bool, bool)>> {
    let guard = Arc::clone(guard);
    thread::spawn(move || {
        let start = Instant::now();
        while !guard.is_counting_down() {
            if start.elapsed() > Duration::from_secs(30) {
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let restored = guard.query_end_session(Duration::from_secs(30));
        let finished = guard.end_session(true, Duration::from_secs(30));
        Some((restored, finished))
    })
}

#[test]
fn a_session_end_reverts_a_running_countdown_like_revert_now() {
    let mut w = World::dev_machine();
    let guard = Arc::new(SessionEnd::new());
    let window = end_the_session_during_the_countdown(&guard);
    let mut silent = Silent {
        guard: Arc::clone(&guard),
        events: Vec::new(),
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);
    let result = {
        let mut sink = SessionEndSink::new(&mut silent, &guard);
        w.run(|e| e.set_layout(&params, &mut sink))
    };
    let result = World::ok(result);
    assert_eq!(
        window.join().expect("the window thread"),
        Some((true, true)),
        "the revert finished within the waits"
    );
    // As on the user's RevertNow: put back, reset again to re-apply US, no failure.
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(result.failure, None);
    assert_eq!(w.hid(KEYCHRON), us());
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::US));
    assert_eq!(w.devices.restarted().len(), 2);
    let ticks = silent
        .events
        .iter()
        .filter(|e| matches!(e, Event::CountdownTick { .. }))
        .count();
    assert!(
        ticks < 20,
        "reverted before the countdown ran out ({ticks} ticks)"
    );
    no_in_flight(&w);
}

#[test]
fn a_session_end_leaves_a_change_waiting_for_a_reconnect() {
    /// Ends the session as soon as the reconnect wait starts.
    struct EndsOnReconnect {
        guard: Arc<SessionEnd>,
        inner: ScriptedSink,
        answered: Option<bool>,
    }
    impl EventSink for EndsOnReconnect {
        fn event(&mut self, event: &Event) {
            if matches!(event, Event::WaitingForReconnect { .. }) && self.answered.is_none() {
                // Nothing counts down: the window's wait returns at once.
                self.answered = Some(self.guard.query_end_session(Duration::from_secs(5)));
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
    let guard = Arc::new(SessionEnd::new());
    let mut inner = EndsOnReconnect {
        guard: Arc::clone(&guard),
        inner: ScriptedSink::default(),
        answered: None,
    };
    let params = set_params(VXE, LayoutChoice::Jis, LIVE);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    assert_eq!(inner.answered, Some(true));
    assert!(guard.is_ending());
    // Never reverted without a countdown (design m2 D.2 b): the user decides later.
    assert_eq!(result.outcome, Outcome::AwaitingConfirm);
    assert_eq!(w.hid(VXE), jis());
    let entry = w.entry(&op_of(&result));
    assert_eq!(entry.state, OpState::AwaitingConfirm);
    assert!(
        !entry
            .history
            .iter()
            .any(|line| line.to == OpState::RevertPending)
    );
}

#[test]
fn the_callers_answer_comes_first() {
    /// Ends the session right after the caller's answer took effect.
    struct EndsAfter {
        guard: Arc<SessionEnd>,
        inner: ScriptedSink,
        when: OpState,
    }
    impl EventSink for EndsAfter {
        fn event(&mut self, event: &Event) {
            self.inner.event(event);
            if matches!(event, Event::StateChanged { state, .. } if *state == self.when) {
                let _ = self.guard.query_end_session(Duration::ZERO);
            }
        }
        fn check_cancelled(&mut self) -> bool {
            self.inner.check_cancelled()
        }
        fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
            self.inner.wait_decision(timeout)
        }
    }
    // RevertNow first: reverted once, as the user asked.
    let mut w = World::dev_machine();
    let guard = Arc::new(SessionEnd::new());
    let mut inner = EndsAfter {
        guard: Arc::clone(&guard),
        inner: ScriptedSink::new([ScriptedSink::revert_now()]),
        when: OpState::RevertPending,
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    assert_eq!((result.outcome, result.failure), (Outcome::Reverted, None));
    assert_eq!(
        inner.inner.count(|e| matches!(
            e,
            Event::StateChanged {
                state: OpState::RevertPending,
                ..
            }
        )),
        1
    );
    assert_eq!(w.devices.restarted().len(), 2);

    // Keep first: kept; the session end afterwards changes nothing.
    let mut w = World::dev_machine();
    let guard = Arc::new(SessionEnd::new());
    let mut inner = EndsAfter {
        guard: Arc::clone(&guard),
        inner: ScriptedSink::new([ScriptedSink::keep()]),
        when: OpState::Confirmed,
    };
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert!(guard.is_ending());
    assert!(
        guard.query_end_session(Duration::ZERO),
        "nothing left to wait for"
    );
    assert_eq!(w.hid(KEYCHRON), jis());
}

#[test]
fn a_decision_that_already_arrived_comes_before_the_session_end() {
    /// Ends the session as the countdown starts, while the caller's Keep is already queued.
    struct EndsAtCountdown {
        guard: Arc<SessionEnd>,
        inner: ScriptedSink,
    }
    impl EventSink for EndsAtCountdown {
        fn event(&mut self, event: &Event) {
            self.inner.event(event);
            if matches!(event, Event::CountdownStarted { .. }) {
                let _ = self.guard.query_end_session(Duration::ZERO);
            }
        }
        fn check_cancelled(&mut self) -> bool {
            self.inner.check_cancelled()
        }
        fn wait_decision(&mut self, timeout: Duration) -> DecisionPoll {
            self.inner.wait_decision(timeout)
        }
    }
    let mut w = World::dev_machine();
    let guard = Arc::new(SessionEnd::new());
    let mut inner = EndsAtCountdown {
        guard: Arc::clone(&guard),
        inner: ScriptedSink::new([ScriptedSink::keep()]),
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    assert!(guard.is_ending());
    assert_eq!(
        result.outcome,
        Outcome::Confirmed,
        "the caller's Keep stands"
    );
    assert_eq!(w.hid(KEYCHRON), jis());
    assert_eq!(w.devices.restarted().len(), 1, "no second reset");
    assert!(guard.query_end_session(Duration::ZERO));
    no_in_flight(&w);
}

#[test]
fn a_session_end_while_the_keyboard_is_reset_waits_for_the_revert() {
    /// Sends `WM_QUERYENDSESSION` and `WM_ENDSESSION(TRUE)` from a window thread as the keyboard
    /// is reset, before the countdown starts, and gives that thread the time it would have.
    struct EndsDuringReset {
        guard: Arc<SessionEnd>,
        log: Arc<Mutex<Vec<Event>>>,
        window: Option<thread::JoinHandle<(bool, bool, bool)>>,
        inner: ScriptedSink,
    }
    impl EventSink for EndsDuringReset {
        fn event(&mut self, event: &Event) {
            self.log.lock().expect("the event log").push(event.clone());
            if matches!(event, Event::ResettingKeyboard { .. }) && self.window.is_none() {
                let guard = Arc::clone(&self.guard);
                let log = Arc::clone(&self.log);
                let answered = Arc::new(AtomicBool::new(false));
                let flag = Arc::clone(&answered);
                self.window = Some(thread::spawn(move || {
                    let restored = guard.query_end_session(Duration::from_secs(10));
                    let counted = log
                        .lock()
                        .expect("the event log")
                        .iter()
                        .any(|e| matches!(e, Event::CountdownStarted { .. }));
                    flag.store(true, Ordering::SeqCst);
                    let finished = guard.end_session(true, Duration::from_secs(10));
                    (restored, counted, finished)
                }));
                let start = Instant::now();
                while !answered.load(Ordering::SeqCst)
                    && start.elapsed() < Duration::from_millis(300)
                {
                    thread::sleep(Duration::from_millis(1));
                }
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
    let guard = Arc::new(SessionEnd::new());
    let mut inner = EndsDuringReset {
        guard: Arc::clone(&guard),
        log: Arc::new(Mutex::new(Vec::new())),
        window: None,
        inner: ScriptedSink::default(),
    };
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    let window = inner.window.take().expect("the keyboard was reset");
    let (restored, counted, finished) = window.join().expect("the window thread");
    assert!(
        restored && counted,
        "WM_QUERYENDSESSION waited for the countdown to start and revert"
    );
    assert!(finished, "WM_ENDSESSION waited for the revert to finish");
    assert_eq!((result.outcome, result.failure), (Outcome::Reverted, None));
    assert_eq!(w.hid(KEYCHRON), us());
    assert_eq!(w.devices.running_type(KEYCHRON), Some(KeyboardType::US));
    assert_eq!(w.devices.restarted().len(), 2);
    no_in_flight(&w);
}

#[test]
fn while_the_session_ends_nothing_new_is_written() {
    let base = World::dev_machine();
    let guard = SessionEnd::new();
    assert!(guard.query_end_session(Duration::ZERO));
    let params = set_params(KEYCHRON, LayoutChoice::Jis, LIVE);

    let mut w = base.fork();
    let mut inner = ScriptedSink::new([ScriptedSink::keep()]);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        w.run(|e| e.set_layout(&params, &mut sink))
    };
    assert_eq!(result, Err(EngineError::Cancelled));
    assert!(w.journal().entries.is_empty());

    // The end was cancelled (another application refused): writes go on.
    assert!(guard.end_session(false, Duration::ZERO));
    let mut w = base.fork();
    let mut inner = ScriptedSink::new([ScriptedSink::keep()]);
    let result = {
        let mut sink = SessionEndSink::new(&mut inner, &guard);
        World::ok(w.run(|e| e.set_layout(&params, &mut sink)))
    };
    assert_eq!(result.outcome, Outcome::Confirmed);
}

// ------------------------------------------------------------------------------------------
// The schema-1 journal of the development machine (M2 real tests)
// ------------------------------------------------------------------------------------------

/// Design m3 WP-E1: journal schema 2 reads the schema-1 entries and baselines the development
/// machine already holds, operates on them, and leaves them in schema 1; only a cleanup is
/// written in schema 2, which the M2 build would refuse as newer.
#[test]
fn the_schema_1_journal_of_the_development_machine_keeps_working() {
    let (ops, baselines) = fixtures::schema_1_journal();
    let mut w = ps2_with_hid_values();
    w.seed_journal(&ops, &baselines);
    let journal = w.journal();
    assert!(journal.unreadable.is_empty());
    assert_eq!(journal.entries.len(), 9);

    // A new set: journaled next to them in schema 1; the M2 baselines of the Keychron stay.
    let set = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    assert_eq!(set.outcome, Outcome::Confirmed);
    let set_op = op_of(&set);
    assert!(stored_json(&w, &set_op).starts_with("{\"schema_version\":1,"));
    let entry = w.entry(&set_op);
    assert_eq!(entry.seq, 10, "after the stored seq 9");
    let journal = w.journal();
    assert_eq!(
        journal.baselines.len(),
        2,
        "no new baseline for the Keychron"
    );
    assert!(
        journal
            .baselines
            .iter()
            .all(|b| b.captured_by.as_str().starts_with("bfaca7cd"))
    );
    let contents = w.registry.contents();
    for (name, json) in &ops {
        assert_eq!(contents.ops.get(name), Some(json), "{name} was rewritten");
    }
    for (name, json) in &baselines {
        assert_eq!(
            contents.baselines.get(name),
            Some(json),
            "{name} was rewritten"
        );
    }

    // A cleanup: schema 2, the only one an M2 build could not read.
    let cleanup = op_of(&World::ok(w.cleanup(PS2, &[HID_TYPE, HID_SUBTYPE])));
    World::ok(w.confirm(&cleanup));
    let contents = w.registry.contents();
    for (name, json) in &contents.ops {
        let entry = JournalEntry::from_json(json).unwrap_or_else(|e| panic!("{name}: {e}"));
        let expected = if entry.op_id == cleanup { 2 } else { 1 };
        assert!(
            json.starts_with(&format!("{{\"schema_version\":{expected},")),
            "{name}: {json}"
        );
    }
    assert!(w.journal().unreadable.is_empty());

    // Restoring the Keychron puts back the value M2 recorded before its first change (4/0).
    let restore = mklm_engine::RestoreBaselineParams {
        apply: LIVE,
        ..restore_params(
            RestoreScope::Device {
                instance_id: KEYCHRON.to_string(),
            },
            ConflictPolicy::Report,
            RestoreMode::Interactive,
        )
    };
    let result = World::ok(
        w.run(|e| e.restore_baseline(&restore, &mut ScriptedSink::new([ScriptedSink::keep()]))),
    );
    assert_eq!(result.outcome, Outcome::Confirmed);
    assert_eq!(w.hid(KEYCHRON), us());
    no_in_flight(&w);

    // An operation read from schema 1 is reverted and written back in schema 1.
    let mut w = World::dev_machine();
    w.seed_journal(&ops, &baselines);
    let kept = OpId::parse("31f7f7bc-3939-403d-9290-ef1c17065d08").unwrap();
    let result = World::ok(w.revert(&kept, NO_RESET));
    assert_eq!(result.outcome, Outcome::Reverted);
    assert_eq!(w.hid(KEYCHRON), (dword(8), dword(2)), "its `before`");
    assert!(stored_json(&w, &kept).starts_with("{\"schema_version\":1,"));
}
