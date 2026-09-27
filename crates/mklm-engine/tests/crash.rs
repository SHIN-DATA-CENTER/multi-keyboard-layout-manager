//! Exhaustive crash tests (design H.1, "クラッシュ時の整合性"): every scenario is run with
//! `crash_after = n` for every `n`; every crash image (process kill, and every power-loss prefix
//! of both hives), with and without a PC restart, is recovered and checked against I1-I6; every
//! recovery is repeated (I4: nothing left to do) and interrupted at every call (I4: the same
//! result). I7 (a target that refuses every write) has its own tests at the end.

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};

use common::*;
use mklm_core::{
    ApplyOptions, Attention, ConflictPolicy, DEVICE_VALUE_NAMES, FailureReason, GLOBAL_VALUE_NAMES,
    Journal, KeyboardType, Layout, LayoutChoice, Liveness, OpId, OpKind, OpState, OperationResult,
    RegValue, ResolutionChoice, RestoreScope, ValueChoice, ValueKey, WriteTarget, attention,
    check_inv_ps2, value_names,
};
use mklm_engine::memory::{FaultPlan, RegistryContents, ScriptedSink};
use mklm_engine::{BackendError, EngineError, ResolveParams, RestoreMode};

type Run = Box<dyn Fn(&mut World) -> Result<OperationResult, EngineError>>;

struct Scenario {
    name: &'static str,
    /// The prepared machine (everything durable, empty log).
    world: World,
    /// Values before MKLM changed anything.
    pristine: RegistryContents,
    run: Run,
    /// Values the user chose to keep in a resolution: never rewritten (C5).
    kept: Vec<(WriteTarget, String, RegValue)>,
    /// A conflict resolution: `Conflict` and `Failed(ConflictKeptCurrent)` are expected ends.
    resolution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    values: RegistryContents,
    ops: Vec<(OpId, OpState, Option<FailureReason>)>,
    baselines: Vec<String>,
}

fn outcome(world: &World) -> Outcome {
    let journal = world.journal();
    Outcome {
        values: values_of(&world.registry.contents()),
        ops: journal
            .entries
            .iter()
            .map(|e| (e.op_id.clone(), e.state, e.failure.clone()))
            .collect(),
        baselines: journal
            .baselines
            .iter()
            .map(|b| b.key.canonical())
            .collect(),
    }
}

#[derive(Debug, Default)]
struct Stats {
    runs: usize,
    images: usize,
    recoveries: usize,
}

type StateKey = (
    RegistryContents,
    BTreeMap<String, Option<KeyboardType>>,
    bool,
);

fn state_key(world: &World, reboot: bool) -> StateKey {
    (
        world.registry.contents(),
        world.devices.running_types(),
        reboot,
    )
}

fn is_mklm_value(target: &WriteTarget, name: &str) -> bool {
    let names: &[&str] = match target {
        WriteTarget::Device { .. } => &DEVICE_VALUE_NAMES,
        WriteTarget::Global => &GLOBAL_VALUE_NAMES,
    };
    names.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn is_boot_time(target: &WriteTarget, name: &str) -> bool {
    match target {
        WriteTarget::Global => true,
        WriteTarget::Device { .. } => {
            name.eq_ignore_ascii_case(value_names::PS2_TYPE)
                || name.eq_ignore_ascii_case(value_names::PS2_SUBTYPE)
        }
    }
}

fn key_of(target: &WriteTarget, name: &str) -> ValueKey {
    ValueKey {
        target: target.clone(),
        name: name.to_string(),
    }
}

fn same_key(a: &ValueKey, b: &ValueKey) -> bool {
    a.canonical().eq_ignore_ascii_case(&b.canonical())
}

/// I2: INV-PS2 on the values as they are (every keyboard, phantoms included).
fn inv_ps2_holds(world: &World) -> bool {
    check_inv_ps2(&world.registry.global_settings(), &world.devices.listed()).is_ok()
}

/// I5: the journal is readable and every value MKLM may write that differs from the pristine
/// machine has a baseline holding the pristine value.
fn check_baselines(sc: &Scenario, world: &World, journal: &Journal, ctx: &str) {
    assert!(
        journal.unreadable.is_empty(),
        "{ctx}: unreadable journal: {:?}",
        journal.unreadable
    );
    for (target, name, pristine, _) in value_diff(&sc.pristine, &world.registry.contents()) {
        if !is_mklm_value(&target, &name) {
            continue;
        }
        let baseline = journal.baseline(&key_of(&target, &name));
        assert!(
            baseline.is_some_and(|b| b.value == pristine),
            "{ctx}: {target:?} {name} changed without its baseline ({baseline:?})"
        );
    }
}

/// Checks on a crash image, before recovery: I2, I5, I6.
fn check_image(sc: &Scenario, base: &World, base_journal: &Journal, world: &World, ctx: &str) {
    if inv_ps2_holds(base) {
        assert!(
            inv_ps2_holds(world),
            "{ctx}: INV-PS2 broken in a crash image"
        );
    }
    let journal = world.journal();
    check_baselines(sc, world, &journal, ctx);
    // I6: a changed value is always described by a journal entry that was written before it.
    for (target, name, _, _) in value_diff(&base.registry.contents(), &world.registry.contents()) {
        if !is_mklm_value(&target, &name) {
            continue;
        }
        let key = key_of(&target, &name);
        let described = journal.entries.iter().any(|entry| {
            entry.records.iter().any(|r| same_key(&r.key(), &key))
                && base_journal.entry(&entry.op_id) != Some(entry)
        });
        assert!(
            described,
            "{ctx}: {target:?} {name} changed without a journaled operation"
        );
        if is_boot_time(&target, &name) {
            let assets = world
                .host
                .recovery_assets()
                .unwrap_or_else(|| panic!("{ctx}: boot-time value changed without recovery files"));
            assert!(
                assets.cmd.contains(&key.key_path()),
                "{ctx}: the recovery files do not cover {}",
                key.key_path()
            );
        }
    }
}

/// Checks after recovery: I1, I3, I5 and the kept values.
fn check_recovered(sc: &Scenario, world: &World, ctx: &str) {
    let journal = world.journal();
    // I3.
    for entry in &journal.entries {
        assert!(
            !entry.state.is_in_flight(),
            "{ctx}: {} still in flight ({:?})",
            entry.op_id,
            entry.state
        );
        assert!(
            !(entry.state == OpState::AwaitingConfirm && entry.countdown.is_some()),
            "{ctx}: {} still counting down",
            entry.op_id
        );
        if !sc.resolution {
            assert_ne!(
                entry.state,
                OpState::Conflict,
                "{ctx}: {} in conflict without outside changes",
                entry.op_id
            );
        }
    }
    assert!(!world.lock.is_locked(), "{ctx}: lock left held");
    check_baselines(sc, world, &journal, ctx);
    for (target, name, value) in &sc.kept {
        assert_eq!(
            world.value(target, name),
            *value,
            "{ctx}: a value the user kept was rewritten"
        );
    }

    // I1: every value is owned by the newest operation that recorded it and did not fail; all
    // the values an operation owns are at `before` or all at `intended`.
    let mut owners: HashMap<String, OpId> = HashMap::new();
    for entry in journal
        .entries
        .iter()
        .filter(|e| e.state != OpState::Failed)
    {
        for record in &entry.records {
            owners.insert(
                record.key().canonical().to_ascii_uppercase(),
                entry.op_id.clone(),
            );
        }
    }
    for entry in &journal.entries {
        if entry.state == OpState::Failed || (sc.resolution && entry.state == OpState::Conflict) {
            continue;
        }
        let owned: Vec<_> = entry
            .records
            .iter()
            .filter(|r| r.skipped.is_none())
            .filter(|r| owners.get(&r.key().canonical().to_ascii_uppercase()) == Some(&entry.op_id))
            .collect();
        if owned.is_empty() {
            continue;
        }
        let now = |r: &mklm_core::ValueRecord| world.value(&r.target, &r.name);
        let at_before = owned
            .iter()
            .all(|r| mklm_core::value_eq(&r.name, &now(r), &r.before));
        let at_intended = owned
            .iter()
            .all(|r| mklm_core::value_eq(&r.name, &now(r), &r.intended));
        let restore = matches!(entry.kind, OpKind::RestoreBaseline { .. });
        let rolled_back = matches!(
            entry.state,
            OpState::Reverted | OpState::RevertedPendingReboot
        );
        if restore && !rolled_back {
            assert!(
                at_intended,
                "{ctx}: restore {} not written through ({:?})",
                entry.op_id, entry.state
            );
        } else {
            assert!(
                at_before || at_intended,
                "{ctx}: {} ({:?}) is half written",
                entry.op_id,
                entry.state
            );
        }
    }

    // I1 (C1): a keyboard runs with its stored values, or the journal says it does not yet.
    for kb in world.devices.listed().iter().filter(|kb| kb.present) {
        let running = world.devices.running_type(&kb.instance_id);
        if running == world.devices.predicted(kb) {
            continue;
        }
        let listed = journal.entries.iter().any(|e| {
            e.apply_pending.as_ref().is_some_and(|p| {
                p.instance_ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&kb.instance_id))
            })
        });
        // A restart state, or a conflict the user is shown, covers its keyboards.
        let restart_state = journal.entries.iter().any(|e| {
            matches!(
                e.state,
                OpState::PendingReboot | OpState::RevertedPendingReboot | OpState::Conflict
            ) && e
                .records
                .iter()
                .any(|r| r.target == WriteTarget::Global || r.target == device(&kb.instance_id))
        });
        assert!(
            listed || restart_state,
            "{ctx}: {} runs {running:?} but its values give {:?}, and nothing says so",
            kb.instance_id,
            world.devices.predicted(kb)
        );
    }
}

fn run_scenario(sc: &Scenario) -> Stats {
    let base = sc.world.fork();
    let base_journal = base.journal();
    let mut stats = Stats::default();

    let mut dry = base.fork();
    let dry_result = (sc.run)(&mut dry);
    assert!(
        dry_result.is_ok(),
        "{}: the undisturbed run failed: {dry_result:?}",
        sc.name
    );
    let total = dry.registry.mutating_calls();
    assert!(total > 0, "{}: the scenario writes nothing", sc.name);

    let mut seen: HashSet<StateKey> = HashSet::new();
    let mut nested_seen: HashMap<StateKey, Outcome> = HashMap::new();
    for n in 0..=total {
        let mut run = base.fork();
        run.registry.set_faults(FaultPlan {
            crash_after: Some(n),
            ..FaultPlan::default()
        });
        let result = (sc.run)(&mut run);
        stats.runs += 1;
        if n < total {
            assert!(
                matches!(result, Err(EngineError::Backend(BackendError::Crashed))),
                "{}: crash_after {n}: {result:?}",
                sc.name
            );
        } else {
            assert!(result.is_ok(), "{}: crash_after {n}: {result:?}", sc.name);
        }
        assert!(
            !run.lock.is_locked(),
            "{}: crash_after {n}: lock held",
            sc.name
        );

        for image in run.registry.crash_images() {
            for reboot in [false, true] {
                let mut world = run.after_crash(image, reboot);
                if !seen.insert(state_key(&world, reboot)) {
                    continue;
                }
                stats.images += 1;
                let ctx = format!("{}: crash_after {n}, {image:?}, reboot {reboot}", sc.name);
                check_image(sc, &base, &base_journal, &world, &ctx);

                let pre = world.fork();
                world.registry.set_faults(FaultPlan::default());
                let recovered = world.recover();
                stats.recoveries += 1;
                assert!(recovered.is_ok(), "{ctx}: recovery failed: {recovered:?}");
                let calls = world.registry.mutating_calls();
                check_recovered(sc, &world, &ctx);
                let expected = outcome(&world);

                // I4: a second recovery has nothing left to do.
                world.registry.set_faults(FaultPlan::default());
                let again = world.recover();
                stats.recoveries += 1;
                assert!(again.is_ok(), "{ctx}: second recovery failed: {again:?}");
                assert_eq!(
                    world.registry.mutating_calls(),
                    0,
                    "{ctx}: the second recovery wrote: {:?}",
                    world.registry.mutations()
                );
                assert_eq!(
                    outcome(&world),
                    expected,
                    "{ctx}: second recovery changed things"
                );

                // I4: a recovery interrupted anywhere converges to the same result.
                for m in 0..calls {
                    let mut crashing = pre.fork();
                    crashing.registry.set_faults(FaultPlan {
                        crash_after: Some(m),
                        ..FaultPlan::default()
                    });
                    let crashed = crashing.recover();
                    stats.recoveries += 1;
                    assert!(
                        matches!(crashed, Err(EngineError::Backend(BackendError::Crashed))),
                        "{ctx}: recovery crash_after {m}: {crashed:?}"
                    );
                    for nested in crashing.registry.crash_images() {
                        let mut again = crashing.after_crash(nested, false);
                        let key = state_key(&again, reboot);
                        if let Some(known) = nested_seen.get(&key) {
                            assert_eq!(
                                *known, expected,
                                "{ctx}: recovery crash_after {m}, {nested:?}: another result"
                            );
                            continue;
                        }
                        let nctx = format!("{ctx}; recovery crash_after {m}, {nested:?}");
                        let result = again.recover();
                        stats.recoveries += 1;
                        assert!(result.is_ok(), "{nctx}: {result:?}");
                        check_recovered(sc, &again, &nctx);
                        assert_eq!(outcome(&again), expected, "{nctx}: another result");
                        nested_seen.insert(key, expected.clone());
                    }
                }
            }
        }
    }
    eprintln!(
        "{}: {} calls, {} runs, {} states, {} recoveries",
        sc.name, total, stats.runs, stats.images, stats.recoveries
    );
    assert!(stats.images > total, "{}: too few crash states", sc.name);
    stats
}

// ------------------------------------------------------------------------------------------
// Scenarios
// ------------------------------------------------------------------------------------------

fn scenario(
    name: &'static str,
    world: World,
    pristine: &World,
    run: impl Fn(&mut World) -> Result<OperationResult, EngineError> + 'static,
) -> Scenario {
    Scenario {
        name,
        world: world.fork(),
        pristine: pristine.registry.contents(),
        run: Box::new(run),
        kept: Vec::new(),
        resolution: false,
    }
}

fn op_of(result: &OperationResult) -> OpId {
    result.op_id.clone().expect("operation ID")
}

/// A pre-M0 machine with the migration done (`PendingReboot`).
fn migrated_pending() -> (World, OpId) {
    let mut w = World::pre_m0();
    let result = World::ok(w.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)]));
    assert_eq!(result.outcome, mklm_core::Outcome::PendingReboot);
    (w, op_of(&result))
}

fn migrated_confirmed() -> World {
    let (mut w, op) = migrated_pending();
    w.reboot();
    World::ok(w.recover());
    World::ok(w.confirm(&op));
    w
}

#[test]
fn crash_usb_set_keep() {
    let w = World::dev_machine();
    run_scenario(&scenario("usb set, keep", w.fork(), &w, |w| {
        w.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::new([ScriptedSink::keep()]),
        )
    }));
}

#[test]
fn crash_usb_set_countdown_expires() {
    let w = World::dev_machine();
    run_scenario(&scenario("usb set, countdown expires", w.fork(), &w, |w| {
        w.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::default(),
        )
    }));
}

#[test]
fn crash_ble_set() {
    let w = World::dev_machine();
    run_scenario(&scenario("ble set", w.fork(), &w, |w| {
        w.set(VXE, LayoutChoice::Jis, LIVE, &mut ScriptedSink::default())
    }));
}

#[test]
fn crash_ps2_set() {
    let w = World::dev_machine();
    run_scenario(&scenario("ps/2 set", w.fork(), &w, |w| {
        w.set(PS2, LayoutChoice::Us, LIVE, &mut ScriptedSink::default())
    }));
}

#[test]
fn crash_migrate() {
    let w = World::pre_m0();
    run_scenario(&scenario("migrate", w.fork(), &w, |w| {
        w.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)])
    }));
}

#[test]
fn crash_migrate_with_a_phantom_ps2_keyboard() {
    let mut w = World::pre_m0();
    w.devices.add_keyboard(dock_ps2());
    let pristine = w.fork();
    run_scenario(&scenario(
        "migrate, phantom PS/2",
        w.fork(),
        &pristine,
        |w| w.migrate(Layout::Us, &[]),
    ));
}

#[test]
fn crash_migration_revert() {
    let pristine = World::pre_m0();
    let (w, op) = migrated_pending();
    run_scenario(&scenario("migration revert", w, &pristine, move |w| {
        w.revert(&op, NO_RESET)
    }));
}

#[test]
fn crash_restore_all_from_the_migrated_state() {
    let pristine = World::pre_m0();
    let w = migrated_confirmed();
    run_scenario(&scenario("restore --baseline --all", w, &pristine, |w| {
        w.restore_all(RestoreMode::Interactive)
    }));
}

#[test]
fn crash_silent_restore_from_the_migrated_state() {
    let pristine = World::pre_m0();
    let w = migrated_confirmed();
    run_scenario(&scenario("silent restore", w, &pristine, |w| {
        w.restore_all(RestoreMode::Silent)
    }));
}

#[test]
fn crash_restore_superseding_a_pending_migration() {
    let pristine = World::pre_m0();
    let (w, _) = migrated_pending();
    run_scenario(&scenario("restore superseding", w, &pristine, |w| {
        w.restore_all(RestoreMode::Interactive)
    }));
}

#[test]
fn crash_restore_of_a_usb_keyboard_with_a_countdown() {
    let pristine = World::dev_machine();
    let mut w = pristine.fork();
    World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    run_scenario(&scenario("restore of the Keychron", w, &pristine, |w| {
        let params = mklm_engine::RestoreBaselineParams {
            apply: LIVE,
            ..restore_params(
                RestoreScope::Device {
                    instance_id: KEYCHRON.to_string(),
                },
                ConflictPolicy::Report,
                RestoreMode::Interactive,
            )
        };
        w.run(|e| e.restore_baseline(&params, &mut ScriptedSink::new([ScriptedSink::keep()])))
    }));
}

#[test]
fn crash_undo() {
    let pristine = World::pre_m0();
    let (w, _) = migrated_pending();
    run_scenario(&scenario("undo", w, &pristine, |w| w.undo()));
}

#[test]
fn crash_undo_after_the_restart() {
    let pristine = World::pre_m0();
    let (mut w, _) = migrated_pending();
    w.reboot();
    run_scenario(&scenario("undo after the restart", w, &pristine, |w| {
        w.undo()
    }));
}

#[test]
fn crash_conflict_resolution() {
    let pristine = World::dev_machine();
    let mut w = pristine.fork();
    let result = World::ok(w.set(
        KEYCHRON,
        LayoutChoice::Jis,
        LIVE,
        &mut ScriptedSink::new([ScriptedSink::keep()]),
    ));
    let op = op_of(&result);
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(4));
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_SUBTYPE, dword(5));
    let result = World::ok(w.revert(&op, NO_RESET));
    assert_eq!(result.outcome, mklm_core::Outcome::Conflict);
    let mut sc = scenario("conflict resolution", w, &pristine, move |w| {
        let params = ResolveParams {
            op_id: op.clone(),
            choices: vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::UseIntended,
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::KeepCurrent,
                },
            ],
            apply: NO_RESET,
        };
        w.run(|e| e.resolve_conflict(&params, &mut ScriptedSink::default()))
    });
    sc.kept = vec![(
        device(KEYCHRON),
        value_names::HID_SUBTYPE.to_string(),
        dword(5),
    )];
    sc.resolution = true;
    run_scenario(&sc);
}

#[test]
fn crash_resolution_that_keeps_a_migration() {
    // The migration is in conflict after the restart: somebody put the Keychron back to US and
    // the Settings app restored the fixed pair. The user re-applies the migration's global values
    // and keeps the Keychron as it is (C5: the kept values must not be undone by a recovery).
    let pristine = World::pre_m0();
    let (mut w, op) = migrated_pending();
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_TYPE, dword(4));
    w.registry
        .outside_edit(&device(KEYCHRON), value_names::HID_SUBTYPE, dword(0));
    w.registry
        .outside_edit(&WriteTarget::Global, value_names::PS2_TYPE, dword(7));
    w.registry
        .outside_edit(&WriteTarget::Global, value_names::PS2_SUBTYPE, dword(2));
    w.reboot();
    let result = World::ok(w.recover());
    assert_eq!(result.recovered[0].to, OpState::Conflict);
    let records = w.entry(&op).records;
    let choices: Vec<ValueChoice> = records
        .iter()
        .enumerate()
        .map(|(record, r)| ValueChoice {
            record,
            choice: if r.target == device(KEYCHRON) {
                ResolutionChoice::KeepCurrent
            } else {
                ResolutionChoice::UseIntended
            },
        })
        .collect();
    let mut sc = scenario("resolution keeping a migration", w, &pristine, move |w| {
        let params = ResolveParams {
            op_id: op.clone(),
            choices: choices.clone(),
            apply: NO_RESET,
        };
        w.run(|e| e.resolve_conflict(&params, &mut ScriptedSink::default()))
    });
    sc.kept = vec![
        (
            device(KEYCHRON),
            value_names::HID_TYPE.to_string(),
            dword(4),
        ),
        (
            device(KEYCHRON),
            value_names::HID_SUBTYPE.to_string(),
            dword(0),
        ),
        (device(PS2), value_names::PS2_TYPE.to_string(), dword(7)),
        (device(PS2), value_names::PS2_SUBTYPE.to_string(), dword(2)),
    ];
    sc.resolution = true;
    run_scenario(&sc);
}

// ------------------------------------------------------------------------------------------
// I7: a target that refuses every write (design review C3)
// ------------------------------------------------------------------------------------------

fn denied_recovery(
    name: &str,
    base: World,
    run: &dyn Fn(&mut World) -> Result<OperationResult, EngineError>,
    denied: WriteTarget,
) {
    let mut dry = base.fork();
    assert!(run(&mut dry).is_ok());
    let total = dry.registry.mutating_calls();
    let mut conflicts = 0;
    for n in 0..total {
        let mut crashed = base.fork();
        crashed.registry.set_faults(FaultPlan {
            crash_after: Some(n),
            ..FaultPlan::default()
        });
        assert!(run(&mut crashed).is_err());
        let mut world = crashed.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
        let ctx = format!("{name}: crash_after {n}");
        world.registry.set_faults(FaultPlan {
            deny_target: Some(denied.clone()),
            ..FaultPlan::default()
        });
        let result = world.recover();
        assert!(result.is_ok(), "{ctx}: {result:?}");
        assert!(inv_ps2_holds(&world), "{ctx}: INV-PS2 broken");
        let journal = world.journal();
        for entry in &journal.entries {
            assert!(
                !entry.state.is_in_flight(),
                "{ctx}: {:?} in flight",
                entry.state
            );
            if entry.state == OpState::Conflict {
                conflicts += 1;
                assert!(
                    entry.records.iter().any(|r| r.write_error.is_some()),
                    "{ctx}: a conflict without the write error"
                );
                assert_eq!(
                    attention(entry, world.host.current_boot(), Liveness::Dead),
                    Attention::Conflict,
                    "{ctx}"
                );
            }
        }
        // The second recovery does not try again.
        world.registry.set_faults(FaultPlan {
            deny_target: Some(denied.clone()),
            ..FaultPlan::default()
        });
        assert!(world.recover().is_ok());
        assert_eq!(
            world.registry.mutating_calls(),
            0,
            "{ctx}: the second recovery wrote {:?}",
            world.registry.mutations()
        );
    }
    assert!(
        conflicts > 0,
        "{name}: no crash point needed the denied target"
    );
}

#[test]
fn a_denied_keyboard_ends_recovery_in_conflict_once() {
    denied_recovery(
        "usb set",
        World::dev_machine(),
        &|w: &mut World| {
            w.set(
                KEYCHRON,
                LayoutChoice::Jis,
                LIVE,
                &mut ScriptedSink::new([ScriptedSink::keep()]),
            )
        },
        device(KEYCHRON),
    );
}

#[test]
fn a_denied_global_key_ends_recovery_in_conflict_once() {
    // The migration rolled back by recovery (a PS/2 keyboard appears unpinned is not needed:
    // a crash before `Written` with some values written is enough).
    denied_recovery(
        "migrate",
        World::pre_m0(),
        &|w: &mut World| w.migrate(Layout::Jis, &[(KEYCHRON, LayoutChoice::Jis)]),
        WriteTarget::Global,
    );
}

#[test]
fn recovery_with_permission_to_reset_leaves_nothing_pending() {
    // Every crash of a USB set, recovered with the caller's permission to reset (C1): the
    // Keychron always ends up running with its stored values.
    let base = World::dev_machine();
    let run = |w: &mut World| {
        w.set(
            KEYCHRON,
            LayoutChoice::Jis,
            LIVE,
            &mut ScriptedSink::new([ScriptedSink::keep()]),
        )
    };
    let mut dry = base.fork();
    assert!(run(&mut dry).is_ok());
    for n in 0..dry.registry.mutating_calls() {
        let mut crashed = base.fork();
        crashed.registry.set_faults(FaultPlan {
            crash_after: Some(n),
            ..FaultPlan::default()
        });
        assert!(run(&mut crashed).is_err());
        let mut world = crashed.after_crash(mklm_engine::memory::CrashImage::ProcessKill, false);
        let result = World::ok(world.run(|e| {
            e.recover(
                &ApplyOptions {
                    allow_live_reset: true,
                    other_input_available: true,
                },
                &mut ScriptedSink::default(),
            )
        }));
        let kb = world
            .devices
            .listed()
            .into_iter()
            .find(|kb| kb.instance_id == KEYCHRON)
            .expect("Keychron");
        assert_eq!(
            world.devices.running_type(KEYCHRON),
            world.devices.predicted(&kb),
            "crash_after {n}: {result:?}"
        );
        assert!(
            world
                .journal()
                .entries
                .iter()
                .all(|e| e.apply_pending.is_none()),
            "crash_after {n}: apply_pending left"
        );
    }
}
