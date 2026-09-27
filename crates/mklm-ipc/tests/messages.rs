//! The messages on the wire (design E.6, H.1 "mklm-ipc"): serde round trips of every message and
//! of every value the messages embed, the S5 guarantees (nothing on the pipe reaches the silent
//! restore), and the JSON-shape snapshot that makes a shape change fail until
//! `PROTOCOL_VERSION` is bumped.

use std::path::PathBuf;

use mklm_core::value_names::{HID_SUBTYPE, HID_TYPE, LAYER_DRIVER_JPN, PS2_SUBTYPE, PS2_TYPE};
use mklm_core::{
    AllowlistError, FailureReason, InvPs2Violation, KeyboardType, Layout, LayoutChoice,
    LayoutTable, OpId, OpState, PendingAction, PlanError, PlanStep, PlannedWrite, RegValue,
    RestoreScope, ValueOp, WriteTarget,
};
use mklm_ipc::{
    ApplyOptions, Assignment, CallerMessage, ConflictInfo, ConflictPolicy, Decision, ErrorCode,
    ErrorInfo, Event, ExpectedKeyboard, ExpectedPlan, Frame, FrameError, FrameReader,
    FrameSequencer, Hello, HelperMessage, MigrateRequest, Nonce, OperationResult, Outcome,
    PROTOCOL_VERSION, RecoveredOp, Request, ResolutionChoice, ResolveConflictRequest,
    RestoreBaselineRequest, SetLayoutRequest, ValueChoice, Welcome, read_frame, write_frame,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

const OP: &str = "0f8c2d4e-5b6a-4c3d-9e8f-a0b1c2d3e4f5";
const OTHER_OP: &str = "7d1e0c3b-2a49-4f58-8e67-9c8b7a6f5e4d";
const KEYCHRON: &str = r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000";
const KEYCHRON_COL02: &str = r"HID\VID_3434&PID_D027&MI_00&COL02\8&148AD7E3&0&0001";
const INTERNAL_PS2: &str = r"ACPI\MSFT0001\4&2A8B8D0D&0";
const BUILD: &str = "0.1.0+0123456789abcdef0123456789abcdef";

// ---- Until WP1 lands ----

/// `OpId::parse` belongs to work package WP1 (mklm-core), developed in parallel with this one;
/// until it is merged it is `todo!()`. The tests that need an `OpId` then return early instead of
/// failing, and run in full once it is implemented.
// TODO(M2 integration): drop the `catch_unwind` once WP1's `OpId::parse` is merged.
fn op_id(text: &str) -> Option<OpId> {
    match std::panic::catch_unwind(|| OpId::parse(text)) {
        Ok(Ok(id)) => Some(id),
        Ok(Err(error)) => panic!("{text:?} must be a valid OpId: {error}"),
        Err(_) => {
            eprintln!("skipped: OpId::parse is not implemented yet (WP1)");
            None
        }
    }
}

// ---- Samples ----

/// Lists one sample per variant of `$ty` (more when a variant is listed twice), and fails to
/// compile when `$ty` gains a variant the list does not name: adding a variant to a message type
/// forces a sample, and the sample changes the snapshot.
macro_rules! every_variant {
    ($ty:ident: $($variant:ident => $value:expr),+ $(,)?) => {{
        // A variant listed twice repeats its arm; the match stays exhaustive either way.
        #[allow(unreachable_patterns)]
        fn variant_name(value: &$ty) -> &'static str {
            match value {
                $($ty::$variant { .. } => stringify!($variant),)+
            }
        }
        let values: Vec<(&'static str, $ty)> = vec![$((stringify!($variant), $value)),+];
        for (listed, value) in &values {
            assert_eq!(
                variant_name(value),
                *listed,
                concat!("a sample of ", stringify!($ty), " is listed under the wrong variant")
            );
        }
        values
    }};
}

fn apply_all() -> ApplyOptions {
    ApplyOptions {
        allow_live_reset: true,
        other_input_available: true,
    }
}

fn hid_writes(layout: Layout) -> PlanStep {
    let keyboard_type = layout.keyboard_type();
    PlanStep {
        target: WriteTarget::Device {
            instance_id: KEYCHRON.to_string(),
        },
        writes: vec![
            PlannedWrite {
                name: HID_TYPE.to_string(),
                op: ValueOp::Set(keyboard_type.ty),
            },
            PlannedWrite {
                name: HID_SUBTYPE.to_string(),
                op: ValueOp::Set(keyboard_type.subtype),
            },
        ],
    }
}

fn migration_steps() -> Vec<PlanStep> {
    vec![
        PlanStep {
            target: WriteTarget::Device {
                instance_id: INTERNAL_PS2.to_string(),
            },
            writes: vec![
                PlannedWrite {
                    name: PS2_TYPE.to_string(),
                    op: ValueOp::Set(7),
                },
                PlannedWrite {
                    name: PS2_SUBTYPE.to_string(),
                    op: ValueOp::Set(2),
                },
            ],
        },
        hid_writes(Layout::Us),
        PlanStep {
            target: WriteTarget::Global,
            writes: vec![
                PlannedWrite {
                    name: LAYER_DRIVER_JPN.to_string(),
                    op: ValueOp::SetString("kbd106.dll".to_string()),
                },
                PlannedWrite {
                    name: PS2_TYPE.to_string(),
                    op: ValueOp::Delete,
                },
                PlannedWrite {
                    name: PS2_SUBTYPE.to_string(),
                    op: ValueOp::Delete,
                },
            ],
        },
    ]
}

fn requests(op: &OpId) -> Vec<(&'static str, Request)> {
    every_variant!(Request:
        SetLayout => Request::SetLayout(SetLayoutRequest {
            instance_id: KEYCHRON.to_string(),
            layout: LayoutChoice::Us,
            apply: apply_all(),
            expected: Some(ExpectedPlan {
                steps: vec![hid_writes(Layout::Us)],
                apply: Some(PendingAction::ResetKeyboard),
            }),
        }),
        SetLayout => Request::SetLayout(SetLayoutRequest {
            instance_id: KEYCHRON.to_string(),
            layout: LayoutChoice::Standard,
            apply: ApplyOptions::default(),
            expected: None,
        }),
        Migrate => Request::Migrate(MigrateRequest {
            standard: Layout::Jis,
            assignments: vec![
                Assignment {
                    instance_id: KEYCHRON.to_string(),
                    layout: LayoutChoice::Us,
                },
                Assignment {
                    instance_id: KEYCHRON_COL02.to_string(),
                    layout: LayoutChoice::Jis,
                },
            ],
            expected: Some(ExpectedPlan {
                steps: migration_steps(),
                apply: Some(PendingAction::RestartPc),
            }),
        }),
        Migrate => Request::Migrate(MigrateRequest {
            standard: Layout::Us,
            assignments: Vec::new(),
            expected: None,
        }),
        Revert => Request::Revert {
            op_id: op.clone(),
            apply: ApplyOptions {
                allow_live_reset: true,
                other_input_available: false,
            },
        },
        Confirm => Request::Confirm { op_id: op.clone() },
        RestoreBaseline => Request::RestoreBaseline(RestoreBaselineRequest {
            scope: RestoreScope::All,
            on_conflict: ConflictPolicy::Report,
            apply: ApplyOptions::default(),
        }),
        RestoreBaseline => Request::RestoreBaseline(RestoreBaselineRequest {
            scope: RestoreScope::Device {
                instance_id: KEYCHRON.to_string(),
            },
            on_conflict: ConflictPolicy::Overwrite,
            apply: apply_all(),
        }),
        Recover => Request::Recover { apply: apply_all() },
        Undo => Request::Undo {
            apply: ApplyOptions::default(),
        },
        ResolveConflict => Request::ResolveConflict(ResolveConflictRequest {
            op_id: op.clone(),
            choices: vec![
                ValueChoice {
                    record: 0,
                    choice: ResolutionChoice::KeepCurrent,
                },
                ValueChoice {
                    record: 1,
                    choice: ResolutionChoice::UseBefore,
                },
                ValueChoice {
                    record: 2,
                    choice: ResolutionChoice::UseIntended,
                },
                ValueChoice {
                    record: 3,
                    choice: ResolutionChoice::UseBaseline,
                },
            ],
            apply: apply_all(),
        }),
    )
}

fn decisions(op: &OpId) -> Vec<(&'static str, Decision)> {
    every_variant!(Decision:
        Keep => Decision::Keep { op_id: op.clone() },
        RevertNow => Decision::RevertNow { op_id: op.clone() },
    )
}

fn caller_messages(op: &OpId) -> Vec<(String, CallerMessage)> {
    let mut samples = Vec::new();
    for (variant, message) in every_variant!(CallerMessage:
        Welcome => CallerMessage::Welcome(Welcome::new(4242, BUILD)),
        Request => CallerMessage::Request(Request::Confirm { op_id: op.clone() }),
        Decision => CallerMessage::Decision(Decision::Keep { op_id: op.clone() }),
        Bye => CallerMessage::Bye,
    ) {
        match message {
            // Every request and every decision, below.
            CallerMessage::Request(_) | CallerMessage::Decision(_) => {}
            message => samples.push((variant.to_string(), message)),
        }
    }
    for (variant, request) in requests(op) {
        samples.push((
            format!("Request.{variant}"),
            CallerMessage::Request(request),
        ));
    }
    for (variant, decision) in decisions(op) {
        samples.push((
            format!("Decision.{variant}"),
            CallerMessage::Decision(decision),
        ));
    }
    samples
}

fn events(op: &OpId) -> Vec<(&'static str, Event)> {
    every_variant!(Event:
        Locked => Event::Locked,
        Planned => Event::Planned {
            op_id: op.clone(),
            steps: vec![hid_writes(Layout::Us)],
            apply: Some(PendingAction::ResetKeyboard),
            keyboards: vec![
                ExpectedKeyboard {
                    instance_id: KEYCHRON.to_string(),
                    display_name: "Keychron K8".to_string(),
                    expected_type: Some(KeyboardType::US),
                    layout_after: Some(LayoutTable::Us),
                    changes: true,
                },
                ExpectedKeyboard {
                    instance_id: INTERNAL_PS2.to_string(),
                    display_name: "標準 PS/2 キーボード".to_string(),
                    expected_type: None,
                    layout_after: None,
                    changes: false,
                },
            ],
        },
        Planned => Event::Planned {
            op_id: op.clone(),
            steps: Vec::new(),
            apply: None,
            keyboards: Vec::new(),
        },
        StateChanged => Event::StateChanged {
            op_id: op.clone(),
            state: OpState::AwaitingConfirm,
        },
        StepWritten => Event::StepWritten {
            op_id: op.clone(),
            step: 1,
            of: 3,
        },
        ResettingKeyboard => Event::ResettingKeyboard {
            instance_id: KEYCHRON.to_string(),
        },
        KeyboardArrived => Event::KeyboardArrived {
            instance_id: KEYCHRON.to_string(),
            reported: Some(KeyboardType::US),
            expected: KeyboardType::US,
        },
        KeyboardArrived => Event::KeyboardArrived {
            instance_id: KEYCHRON.to_string(),
            reported: None,
            expected: KeyboardType::JIS,
        },
        CountdownStarted => Event::CountdownStarted {
            op_id: op.clone(),
            seconds: 15,
            verified: true,
        },
        CountdownTick => Event::CountdownTick {
            op_id: op.clone(),
            remaining: 14,
        },
        WaitingForReconnect => Event::WaitingForReconnect {
            op_id: op.clone(),
            instance_ids: vec![KEYCHRON.to_string(), KEYCHRON_COL02.to_string()],
        },
        RecoveryAssetsWritten => Event::RecoveryAssetsWritten {
            directory: r"C:\ProgramData\SHIN DATA CENTER\MKLM\Recovery".to_string(),
            skipped: 1,
        },
        Warning => Event::Warning {
            message: "Raw Input could not be read; the result is unverified".to_string(),
        },
        Heartbeat => Event::Heartbeat,
    )
}

fn conflict(op: &OpId) -> ConflictInfo {
    ConflictInfo {
        op_id: op.clone(),
        record: 2,
        key_path: r"SYSTEM\CurrentControlSet\Services\i8042prt\Parameters".to_string(),
        name: LAYER_DRIVER_JPN.to_string(),
        baseline: RegValue::Absent,
        before: RegValue::Sz {
            value: "kbd101.dll".to_string(),
        },
        intended: RegValue::Sz {
            value: "kbd106.dll".to_string(),
        },
        last_written: Some(RegValue::Sz {
            value: "kbd106.dll".to_string(),
        }),
        current: RegValue::Other {
            reg_type: 2,
            data_hex: "6b00620064000000".to_string(),
        },
        write_error: Some("Access is denied. (0x80070005)".to_string()),
    }
}

fn helper_messages(op: &OpId, other_op: &OpId) -> Vec<(String, HelperMessage)> {
    let nonce = Nonce::from_bytes(std::array::from_fn(|index| {
        u8::try_from(index * 7 % 256).unwrap_or(0)
    }));
    let full_result = OperationResult {
        op_id: Some(op.clone()),
        outcome: Outcome::Conflict,
        failure: Some(FailureReason::Superseded {
            by: other_op.clone(),
        }),
        pending_action: Some(PendingAction::RestartPc),
        conflicts: vec![
            conflict(op),
            ConflictInfo {
                last_written: None,
                write_error: None,
                current: RegValue::Dword { value: 4 },
                ..conflict(op)
            },
        ],
        inv_ps2_violation: Some(InvPs2Violation {
            keyboards: vec![INTERNAL_PS2.to_string()],
        }),
        recovered: vec![RecoveredOp {
            op_id: other_op.clone(),
            from: OpState::Restarting,
            to: OpState::Reverted,
            decision: "roll-back".to_string(),
        }],
        warnings: vec!["the keyboard did not report a type".to_string()],
    };
    let empty_result = OperationResult {
        op_id: None,
        outcome: Outcome::NoChange,
        failure: None,
        pending_action: None,
        conflicts: Vec::new(),
        inv_ps2_violation: None,
        recovered: Vec::new(),
        warnings: Vec::new(),
    };
    let mut samples = Vec::new();
    for (variant, message) in every_variant!(HelperMessage:
        Hello => HelperMessage::Hello(Hello::new(&nonce, 5150, "0.1.0", BUILD)),
        Event => HelperMessage::Event(Event::Heartbeat),
        Result => HelperMessage::Result(full_result),
        Result => HelperMessage::Result(empty_result),
        Error => HelperMessage::Error(ErrorInfo {
            code: ErrorCode::PlanRejected,
            message: "the plan breaks INV-PS2".to_string(),
            op_id: Some(op.clone()),
            plan_error: Some(PlanError::InvPs2(InvPs2Violation {
                keyboards: vec![INTERNAL_PS2.to_string()],
            })),
        }),
        Error => HelperMessage::Error(ErrorInfo {
            code: ErrorCode::Busy,
            message: "another MKLM process is writing".to_string(),
            op_id: None,
            plan_error: None,
        }),
    ) {
        match message {
            // Every event, below.
            HelperMessage::Event(_) => {}
            message => samples.push((variant.to_string(), message)),
        }
    }
    for (variant, event) in events(op) {
        samples.push((format!("Event.{variant}"), HelperMessage::Event(event)));
    }
    samples
}

/// Every value of the `mklm_core` types the messages embed, one JSON value per line of the
/// snapshot. They are part of the wire format (design A.5: a change to them needs a
/// `PROTOCOL_VERSION` bump too).
fn vocabulary(op: &OpId) -> Vec<(String, String)> {
    fn add<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
        into: &mut Vec<(String, String)>,
        type_name: &str,
        values: Vec<(&'static str, T)>,
    ) {
        for (variant, value) in values {
            let label = format!("{type_name}.{variant}");
            round_trip(&label, &value);
            into.push((label, serde_json::to_string(&value).unwrap()));
        }
    }

    let mut lines = Vec::new();
    add(
        &mut lines,
        "Outcome",
        every_variant!(Outcome:
            NoChange => Outcome::NoChange,
            Confirmed => Outcome::Confirmed,
            AwaitingConfirm => Outcome::AwaitingConfirm,
            PendingReboot => Outcome::PendingReboot,
            Reverted => Outcome::Reverted,
            RevertedPendingReboot => Outcome::RevertedPendingReboot,
            Failed => Outcome::Failed,
            Conflict => Outcome::Conflict,
            Recovered => Outcome::Recovered,
        ),
    );
    add(
        &mut lines,
        "ErrorCode",
        every_variant!(ErrorCode:
            Busy => ErrorCode::Busy,
            OpInProgress => ErrorCode::OpInProgress,
            RecoveryNeeded => ErrorCode::RecoveryNeeded,
            JournalUnreadable => ErrorCode::JournalUnreadable,
            PlanRejected => ErrorCode::PlanRejected,
            PlanChanged => ErrorCode::PlanChanged,
            MigrationRequired => ErrorCode::MigrationRequired,
            NotFixedMode => ErrorCode::NotFixedMode,
            UnknownKeyboard => ErrorCode::UnknownKeyboard,
            UnknownOp => ErrorCode::UnknownOp,
            NotLatest => ErrorCode::NotLatest,
            InvalidState => ErrorCode::InvalidState,
            InventoryIncomplete => ErrorCode::InventoryIncomplete,
            RecoveryAssetsUnavailable => ErrorCode::RecoveryAssetsUnavailable,
            Cancelled => ErrorCode::Cancelled,
            Registry => ErrorCode::Registry,
            Device => ErrorCode::Device,
            Host => ErrorCode::Host,
            Protocol => ErrorCode::Protocol,
            Internal => ErrorCode::Internal,
        ),
    );
    add(
        &mut lines,
        "OpState",
        every_variant!(OpState:
            Planned => OpState::Planned,
            Written => OpState::Written,
            Restarting => OpState::Restarting,
            AwaitingConfirm => OpState::AwaitingConfirm,
            PendingReboot => OpState::PendingReboot,
            Confirmed => OpState::Confirmed,
            RevertPending => OpState::RevertPending,
            Reverted => OpState::Reverted,
            RevertedPendingReboot => OpState::RevertedPendingReboot,
            Failed => OpState::Failed,
            Conflict => OpState::Conflict,
        ),
    );
    add(
        &mut lines,
        "FailureReason",
        every_variant!(FailureReason:
            ConcurrentChange => FailureReason::ConcurrentChange {
                name: HID_TYPE.to_string(),
            },
            WriteError => FailureReason::WriteError {
                message: "Access is denied. (0x80070005)".to_string(),
            },
            CallerDisconnected => FailureReason::CallerDisconnected,
            Interrupted => FailureReason::Interrupted,
            LiveResetUnconfirmed => FailureReason::LiveResetUnconfirmed,
            CountdownExpired => FailureReason::CountdownExpired,
            KeyboardDidNotReturn => FailureReason::KeyboardDidNotReturn,
            NothingWritten => FailureReason::NothingWritten,
            ConflictKeptCurrent => FailureReason::ConflictKeptCurrent,
            Superseded => FailureReason::Superseded { by: op.clone() },
        ),
    );
    add(
        &mut lines,
        "PendingAction",
        every_variant!(PendingAction:
            ResetKeyboard => PendingAction::ResetKeyboard,
            Reconnect => PendingAction::Reconnect,
            RestartPc => PendingAction::RestartPc,
        ),
    );
    add(
        &mut lines,
        "LayoutChoice",
        every_variant!(LayoutChoice:
            Jis => LayoutChoice::Jis,
            Us => LayoutChoice::Us,
            Standard => LayoutChoice::Standard,
        ),
    );
    add(
        &mut lines,
        "Layout",
        every_variant!(Layout:
            Jis => Layout::Jis,
            Us => Layout::Us,
        ),
    );
    add(
        &mut lines,
        "LayoutTable",
        every_variant!(LayoutTable:
            Jis => LayoutTable::Jis,
            Us => LayoutTable::Us,
            Other => LayoutTable::Other("kbdnec.dll".to_string()),
        ),
    );
    add(
        &mut lines,
        "ConflictPolicy",
        every_variant!(ConflictPolicy:
            Report => ConflictPolicy::Report,
            Skip => ConflictPolicy::Skip,
            Overwrite => ConflictPolicy::Overwrite,
        ),
    );
    add(
        &mut lines,
        "ResolutionChoice",
        every_variant!(ResolutionChoice:
            KeepCurrent => ResolutionChoice::KeepCurrent,
            UseBefore => ResolutionChoice::UseBefore,
            UseIntended => ResolutionChoice::UseIntended,
            UseBaseline => ResolutionChoice::UseBaseline,
        ),
    );
    add(
        &mut lines,
        "RestoreScope",
        every_variant!(RestoreScope:
            All => RestoreScope::All,
            Device => RestoreScope::Device {
                instance_id: KEYCHRON.to_string(),
            },
        ),
    );
    add(
        &mut lines,
        "RegValue",
        every_variant!(RegValue:
            Absent => RegValue::Absent,
            Dword => RegValue::Dword { value: 0x51 },
            Sz => RegValue::Sz {
                value: "kbd106.dll".to_string(),
            },
            Other => RegValue::Other {
                reg_type: 1,
                data_hex: "340000".to_string(),
            },
        ),
    );
    add(
        &mut lines,
        "WriteTarget",
        every_variant!(WriteTarget:
            Device => WriteTarget::Device {
                instance_id: KEYCHRON.to_string(),
            },
            Global => WriteTarget::Global,
        ),
    );
    add(
        &mut lines,
        "ValueOp",
        every_variant!(ValueOp:
            Set => ValueOp::Set(7),
            SetString => ValueOp::SetString("PCAT_106KEY".to_string()),
            Delete => ValueOp::Delete,
        ),
    );
    add(
        &mut lines,
        "AllowlistError",
        every_variant!(AllowlistError:
            ReadOnlyDriver => AllowlistError::ReadOnlyDriver {
                service: "HidIr".to_string(),
            },
            VirtualKeyboard => AllowlistError::VirtualKeyboard,
            ValueNotAllowed => AllowlistError::ValueNotAllowed {
                name: "KeyboardNumberTotalKeysOverride".to_string(),
            },
            DuplicateValue => AllowlistError::DuplicateValue {
                name: HID_TYPE.to_string(),
            },
            IncompletePair => AllowlistError::IncompletePair {
                present: HID_TYPE.to_string(),
                missing: HID_SUBTYPE.to_string(),
            },
            MismatchedPair => AllowlistError::MismatchedPair {
                name: PS2_TYPE.to_string(),
                other: PS2_SUBTYPE.to_string(),
            },
            OperationNotAllowed => AllowlistError::OperationNotAllowed {
                name: LAYER_DRIVER_JPN.to_string(),
                op: ValueOp::Set(1),
            },
            TypeNotAllowed => AllowlistError::TypeNotAllowed {
                keyboard_type: KeyboardType::NEC,
            },
            StringNotAllowed => AllowlistError::StringNotAllowed {
                name: LAYER_DRIVER_JPN.to_string(),
                value: "evil.dll".to_string(),
            },
            InconsistentGlobal => AllowlistError::InconsistentGlobal {
                reason: "fixed JIS with the US layer driver".to_string(),
            },
        ),
    );
    add(
        &mut lines,
        "PlanError",
        every_variant!(PlanError:
            Device => PlanError::Device {
                instance_id: KEYCHRON.to_string(),
                error: AllowlistError::TypeNotAllowed {
                    keyboard_type: KeyboardType::US_ON_JAPANESE,
                },
            },
            Global => PlanError::Global {
                error: AllowlistError::VirtualKeyboard,
            },
            UnknownKeyboard => PlanError::UnknownKeyboard {
                instance_id: KEYCHRON.to_string(),
            },
            DuplicateKeyboard => PlanError::DuplicateKeyboard {
                instance_id: KEYCHRON.to_string(),
            },
            InvPs2 => PlanError::InvPs2(InvPs2Violation {
                keyboards: vec![INTERNAL_PS2.to_string()],
            }),
        ),
    );
    lines
}

fn frame_json<T: Serialize>(body: T) -> String {
    serde_json::to_string(&Frame::new(1, body)).unwrap()
}

/// The snapshot text: one line per sample, `<direction> <label> <compact JSON>`.
fn snapshot_text(op: &OpId, other_op: &OpId) -> String {
    let mut text = format!(
        "# mklm-ipc wire format, PROTOCOL_VERSION {PROTOCOL_VERSION}. Generated by \
         tests/messages.rs; never edit by hand.\n"
    );
    for (label, message) in caller_messages(op) {
        text.push_str(&format!("caller {label} {}\n", frame_json(message)));
    }
    for (label, message) in helper_messages(op, other_op) {
        text.push_str(&format!("helper {label} {}\n", frame_json(message)));
    }
    for (label, value) in vocabulary(op) {
        text.push_str(&format!("value {label} {value}\n"));
    }
    text
}

fn round_trip<T>(label: &str, value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{label}: {error}"));
    assert_eq!(&back, value, "{label}: {json}");
}

// ---- Round trips ----

#[test]
fn every_caller_message_round_trips() {
    let Some(op) = op_id(OP) else { return };
    let samples = caller_messages(&op);
    for (label, message) in &samples {
        round_trip(label, message);
    }

    // And through the framing, as one stream.
    let mut sequencer = FrameSequencer::new();
    let mut stream = Vec::new();
    for (_, message) in &samples {
        write_frame(&mut stream, &sequencer.frame(message)).unwrap();
    }
    let mut reader = FrameReader::new();
    let mut input = stream.as_slice();
    for (seq, (label, message)) in (1..).zip(&samples) {
        let frame: Frame<CallerMessage> = reader.read(&mut input).unwrap();
        assert_eq!(frame.seq, seq, "{label}");
        assert_eq!(&frame.body, message, "{label}");
    }
    assert_eq!(
        reader.read::<_, CallerMessage>(&mut input),
        Err(FrameError::Closed)
    );
}

#[test]
fn every_helper_message_round_trips() {
    let (Some(op), Some(other_op)) = (op_id(OP), op_id(OTHER_OP)) else {
        return;
    };
    let samples = helper_messages(&op, &other_op);
    for (label, message) in &samples {
        round_trip(label, message);
    }

    let mut sequencer = FrameSequencer::new();
    let mut stream = Vec::new();
    for (_, message) in &samples {
        write_frame(&mut stream, &sequencer.frame(message)).unwrap();
    }
    let mut reader = FrameReader::new();
    let mut input = stream.as_slice();
    for (label, message) in &samples {
        let frame: Frame<HelperMessage> = reader.read(&mut input).unwrap();
        assert_eq!(&frame.body, message, "{label}");
    }
}

#[test]
fn every_embedded_value_round_trips() {
    let Some(op) = op_id(OP) else { return };
    // `vocabulary` round-trips each value as it builds the list.
    assert!(!vocabulary(&op).is_empty());
}

/// The frame of section E.6, byte for byte.
#[test]
fn the_design_example_frame() {
    let text = r#"{"v":1,"seq":2,"body":{"type":"request","data":{"kind":"set-layout","instance_id":"HID\\VID_3434&PID_D027&MI_00&COL01\\8&148AD7E3&0&0000","layout":"jis","apply":{"allow_live_reset":true,"other_input_available":true},"expected":null}}}"#;
    let frame: Frame<CallerMessage> = serde_json::from_str(text).unwrap();
    assert_eq!(
        frame,
        Frame {
            v: 1,
            seq: 2,
            body: CallerMessage::Request(Request::SetLayout(SetLayoutRequest {
                instance_id: KEYCHRON.to_string(),
                layout: LayoutChoice::Jis,
                apply: apply_all(),
                expected: None,
            })),
        }
    );
    assert_eq!(serde_json::to_string(&frame).unwrap(), text);
    if PROTOCOL_VERSION == 1 {
        let mut bytes = u32::try_from(text.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        let read: Frame<CallerMessage> = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(read, frame);
    }
}

#[test]
fn handshake_frames_have_the_documented_fields() {
    let nonce = Nonce::from_bytes([0xab; 32]);
    let hello = serde_json::to_value(HelperMessage::Hello(Hello::new(
        &nonce, 5150, "0.1.0", BUILD,
    )))
    .unwrap();
    assert_eq!(
        hello,
        serde_json::json!({
            "type": "hello",
            "data": {
                "protocol": PROTOCOL_VERSION,
                "nonce_hex": "ab".repeat(32),
                "helper_pid": 5150,
                "helper_version": "0.1.0",
                "build_id": BUILD,
            }
        })
    );
    let welcome = serde_json::to_value(CallerMessage::Welcome(Welcome::new(4242, BUILD))).unwrap();
    assert_eq!(
        welcome,
        serde_json::json!({
            "type": "welcome",
            "data": { "protocol": PROTOCOL_VERSION, "caller_pid": 4242, "build_id": BUILD }
        })
    );
    assert_eq!(
        serde_json::to_value(CallerMessage::Bye).unwrap(),
        serde_json::json!({ "type": "bye" })
    );
}

// ---- Design review S5: the pipe never reaches the silent restore ----

fn restore_request_json(extra: &str) -> String {
    format!(
        r#"{{"type":"request","data":{{"kind":"restore-baseline","scope":{{"kind":"all"}},"on_conflict":"skip","apply":{{"allow_live_reset":false,"other_input_available":false}}{extra}}}}}"#
    )
}

#[test]
fn restore_baseline_has_no_silent_field() {
    // Without the field: an interactive restore request (the helper maps it to
    // `RestoreMode::Interactive`; nothing else is expressible).
    let message: CallerMessage = serde_json::from_str(&restore_request_json("")).unwrap();
    assert_eq!(
        message,
        CallerMessage::Request(Request::RestoreBaseline(RestoreBaselineRequest {
            scope: RestoreScope::All,
            on_conflict: ConflictPolicy::Skip,
            apply: ApplyOptions::default(),
        }))
    );
    // With it, in any spelling: rejected, not ignored.
    for extra in [
        r#","silent":true"#,
        r#","silent":false"#,
        r#","mode":"silent""#,
        r#","mode":{"kind":"silent"}"#,
        r#","Silent":true"#,
    ] {
        let json = restore_request_json(extra);
        assert!(
            serde_json::from_str::<CallerMessage>(&json).is_err(),
            "accepted {json}"
        );
        // The same through the framing: a malformed message, which ends the session.
        let body = format!(r#"{{"v":{PROTOCOL_VERSION},"seq":1,"body":{json}}}"#);
        let mut bytes = u32::try_from(body.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend_from_slice(body.as_bytes());
        let result = read_frame::<_, CallerMessage>(&mut bytes.as_slice());
        assert!(matches!(result, Err(FrameError::Json(_))), "{result:?}");
    }
}

#[test]
fn no_request_kind_reaches_the_silent_restore() {
    for kind in [
        "uninstall-restore",
        "restore-baseline-silent",
        "silent-restore",
        "restore-silent",
        "execute",
        "RestoreBaseline",
        "restore_baseline",
    ] {
        let json = format!(
            r#"{{"kind":"{kind}","scope":{{"kind":"all"}},"on_conflict":"skip","apply":{{"allow_live_reset":false,"other_input_available":false}}}}"#
        );
        assert!(
            serde_json::from_str::<Request>(&json).is_err(),
            "accepted {json}"
        );
    }
    for message_type in ["uninstall-restore", "execute", "command"] {
        let json = format!(r#"{{"type":"{message_type}","data":{{}}}}"#);
        assert!(serde_json::from_str::<CallerMessage>(&json).is_err());
    }
}

/// The engine's `RestoreMode` cannot even be named here: `mklm-ipc` does not depend on
/// `mklm-engine` (design A.1). The helper's `dispatch` maps `RestoreBaseline` to `Interactive`.
#[test]
fn ipc_does_not_depend_on_the_engine() {
    let manifest = include_str!("../Cargo.toml");
    for line in manifest.lines().map(str::trim) {
        for forbidden in ["mklm-engine", "mklm-win", "windows"] {
            assert!(!line.starts_with(forbidden), "{line}");
        }
    }
}

#[test]
fn ipc_types_reject_unknown_fields() {
    let rejected = [
        r#"{"type":"welcome","data":{"protocol":1,"caller_pid":1,"build_id":"b","extra":1}}"#,
        r#"{"type":"bye","data":null,"extra":1}"#,
        r#"{"type":"request","data":{"kind":"set-layout","instance_id":"x","layout":"jis","apply":{"allow_live_reset":false,"other_input_available":false},"expected":null,"force":true}}"#,
        r#"{"type":"request","data":{"kind":"migrate","standard":"jis","assignments":[{"instance_id":"x","layout":"us","extra":1}],"expected":null}}"#,
        r#"{"type":"request","data":{"kind":"recover","apply":{"allow_live_reset":false,"other_input_available":false},"silent":true}}"#,
        r#"{"type":"request","data":{"kind":"undo","apply":{"allow_live_reset":false,"other_input_available":false},"all":true}}"#,
    ];
    for json in rejected {
        assert!(
            serde_json::from_str::<CallerMessage>(json).is_err(),
            "accepted {json}"
        );
    }
    let hello = r#"{"type":"hello","data":{"protocol":1,"nonce_hex":"","helper_pid":1,"helper_version":"","build_id":"","elevated":true}}"#;
    assert!(serde_json::from_str::<HelperMessage>(hello).is_err());
}

// ---- The shape snapshot ----

fn snapshot_path(version: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("protocol-v{version}.txt"))
}

/// The JSON shape of every message and embedded value, recorded per protocol version.
///
/// A recorded version's snapshot must never change: when this test fails because the shape
/// changed, bump `PROTOCOL_VERSION` (mklm-ipc/src/lib.rs), then run the tests once with
/// `MKLM_BLESS_PROTOCOL_SNAPSHOT=1` to record `tests/snapshots/protocol-v<new>.txt`. Blessing
/// never overwrites an existing snapshot.
#[test]
fn json_shape_matches_the_snapshot_of_this_protocol_version() {
    let (Some(op), Some(other_op)) = (op_id(OP), op_id(OTHER_OP)) else {
        return;
    };
    let actual = snapshot_text(&op, &other_op);
    let path = snapshot_path(PROTOCOL_VERSION);
    let recorded = match std::fs::read_to_string(&path) {
        // Tolerate what a Windows checkout or editor may add: CRLF line ends and a BOM.
        Ok(text) => text.trim_start_matches('\u{feff}').replace("\r\n", "\n"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::env::var_os("MKLM_BLESS_PROTOCOL_SNAPSHOT").is_some() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &actual).unwrap();
                eprintln!("recorded {}", path.display());
                return;
            }
            panic!(
                "no snapshot for protocol version {PROTOCOL_VERSION} ({}). Record it with \
                 MKLM_BLESS_PROTOCOL_SNAPSHOT=1.",
                path.display()
            );
        }
        Err(error) => panic!("{}: {error}", path.display()),
    };
    if recorded != actual {
        let difference = recorded
            .lines()
            .zip(actual.lines())
            .find(|(recorded, actual)| recorded != actual)
            .map(|(recorded, actual)| format!("recorded: {recorded}\nactual:   {actual}"))
            .unwrap_or_else(|| {
                format!(
                    "{} recorded lines, {} actual lines",
                    recorded.lines().count(),
                    actual.lines().count()
                )
            });
        panic!(
            "the wire format changed, but PROTOCOL_VERSION is still {PROTOCOL_VERSION}.\n\
             Bump PROTOCOL_VERSION in crates/mklm-ipc/src/lib.rs, then record the new shape with \
             MKLM_BLESS_PROTOCOL_SNAPSHOT=1 (do not edit {}).\n{difference}",
            path.display()
        );
    }
}
