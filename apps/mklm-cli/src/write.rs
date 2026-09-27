//! Write commands (milestone M2; design section F): `set`, `migrate`, `revert`, `undo`, `resolve`,
//! `restore`, `recover`, `keep`, `reboot`, `post-reboot` and the read-only `journal`.
//!
//! A write command is the pipe server: it creates the pipe, launches `mklm-helper.exe` (through
//! UAC when unelevated, with `CreateProcessW` and no prompt when already elevated, Safe Mode
//! included) and relays events, countdown answers and the result. The helper always runs as its
//! own process, so that closing the console disconnects it and a countdown reverts at once
//! (design review C8). Only the hidden `--in-process` fallback of `recover`, `undo` and `restore`
//! runs `mklm_engine::Engine` inside the CLI (elevated only, with a console control handler that
//! turns Ctrl+C / close into "revert now"; DLL hardening failures are fatal there).
//!
//! After every helper session, whatever its end, the CLI reads the journal unelevated and
//! registers the post-reboot RunOnce entry when an entry waits for a restart (design review C17).

// Skeleton (M2): the handlers below are not implemented yet; remove these allows with them.
#![allow(dead_code, unused_variables)]

use std::str::FromStr;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};

/// Process exit codes of the write commands (section F.5). Read-only commands keep 0 / 1 / 2.
pub mod exit_code {
    /// Done: kept, reverted on request, no change needed, or recovered.
    pub const OK: i32 = 0;
    /// Failed; nothing changed or everything was rolled back (the message says which).
    pub const FAILURE: i32 = 1;
    /// Usage error (clap).
    pub const USAGE: i32 = 2;
    /// The user cancelled: declined UAC or answered "no" before anything was written.
    pub const CANCELLED: i32 = 3;
    /// The change was reverted automatically: countdown expired, no answer, verification failed,
    /// or the keyboard did not come back.
    pub const REVERTED: i32 = 4;
    /// A value is in conflict; run `mklm-cli recover` or resolve it in the GUI.
    pub const CONFLICT: i32 = 5;
    /// Another operation holds the lock, an open operation blocks this one, or recovery is needed.
    pub const BLOCKED: i32 = 6;
    /// Written; waiting for a reconnect and `mklm-cli keep <op>` / `revert <op>`.
    pub const AWAITING_CONFIRM: i32 = 10;
    /// Written or reverted; a PC restart is needed (same value as msiexec's
    /// ERROR_SUCCESS_REBOOT_REQUIRED).
    pub const RESTART_REQUIRED: i32 = 3010;
}

/// A keyboard on the command line: an instance ID, or `#n` from `mklm-cli list` (connected
/// keyboards only, in `list` order). The CLI always echoes the resolved name and instance ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardRef {
    InstanceId(String),
    /// 1-based row of `mklm-cli list`.
    ListRow(usize),
}

impl FromStr for KeyboardRef {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(row) = text.strip_prefix('#') {
            return match row.parse::<usize>() {
                Ok(n) if n > 0 => Ok(KeyboardRef::ListRow(n)),
                _ => Err(format!(
                    "{text:?}: expected #1, #2, … as numbered by `mklm-cli list`"
                )),
            };
        }
        if text.contains('\\') && !text.trim().is_empty() {
            Ok(KeyboardRef::InstanceId(text.to_string()))
        } else {
            Err(format!(
                "{text:?}: expected an instance ID (e.g. HID\\VID_…\\…) or #n"
            ))
        }
    }
}

/// `--layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LayoutArg {
    Jis,
    Us,
    /// Follow the PC's standard layout (HID keyboards only).
    Standard,
}

/// `--standard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StandardArg {
    Jis,
    Us,
}

/// `--answer` for non-interactive runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AnswerArg {
    /// Keep, but only when Raw Input reports the intended type; otherwise revert.
    Keep,
    Revert,
}

/// `--on-conflict` of `restore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConflictArg {
    /// Stop and show baseline / last written / current (default).
    Report,
    /// Leave conflicting values alone.
    Skip,
    /// Write the baseline anyway.
    Overwrite,
}

/// A choice of `resolve` (`--all` or `--value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ChoiceArg {
    /// Leave the value as it is now.
    KeepCurrent,
    /// The value before the operation.
    Before,
    /// The value the operation wrote.
    Intended,
    /// The value from before MKLM.
    Baseline,
}

/// `--value <n>=<choice>` of `resolve`: `n` is the record number `resolve` and `journal` print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueChoiceArg {
    pub record: usize,
    pub choice: ChoiceArg,
}

impl FromStr for ValueChoiceArg {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (record, choice) = text.split_once('=').ok_or_else(|| {
            format!("{text:?}: expected <n>=<keep-current|before|intended|baseline>")
        })?;
        Ok(ValueChoiceArg {
            record: record
                .parse()
                .map_err(|_| format!("{record:?}: expected a record number"))?,
            choice: ChoiceArg::from_str(choice, true)?,
        })
    }
}

/// `--also <keyboard>=<layout>` of `migrate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlsoArg {
    pub keyboard: KeyboardRef,
    pub layout: LayoutArg,
}

impl FromStr for AlsoArg {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (keyboard, layout) = text
            .rsplit_once('=')
            .ok_or_else(|| format!("{text:?}: expected <instance-id|#n>=<jis|us|standard>"))?;
        Ok(AlsoArg {
            keyboard: keyboard.parse()?,
            layout: LayoutArg::from_str(layout, true)?,
        })
    }
}

/// Whether a command may restart a keyboard in place (plan 1.4). Shared by every command that can
/// end with a live reset (design review C9). Neither flag: the CLI asks; with `--yes` (where
/// allowed) and neither flag, nothing is reset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
#[group(id = "reset_choice", multiple = true)]
pub struct ApplyArgs {
    /// Never reset a keyboard in place; apply on reconnect or PC restart instead.
    #[arg(long)]
    pub no_reset: bool,
    /// I have another way to type (another keyboard, or a mouse and the on-screen keyboard).
    #[arg(long)]
    pub other_input: bool,
}

/// `mklm-cli set`.
#[derive(Debug, Clone, Args)]
pub struct SetArgs {
    /// Instance ID, or #n as numbered by `mklm-cli list` (#n is refused with --yes).
    pub keyboard: KeyboardRef,
    #[arg(long, value_enum)]
    pub layout: LayoutArg,
    #[command(flatten)]
    pub apply: ApplyArgs,
    /// Skip the confirmation of the planned changes (the keep/revert countdown still runs).
    /// Needs --other-input or --no-reset, and an instance ID rather than #n (design review S12).
    #[arg(long, requires = "reset_choice")]
    pub yes: bool,
    /// Answer the keep/revert question without a prompt (for scripted tests).
    #[arg(long, value_enum)]
    pub answer: Option<AnswerArg>,
}

/// `mklm-cli revert`.
#[derive(Debug, Clone, Args)]
pub struct RevertArgs {
    /// Operation ID, or a unique prefix of at least 8 hex digits.
    pub op: String,
    #[command(flatten)]
    pub apply: ApplyArgs,
    #[arg(long)]
    pub yes: bool,
}

/// `mklm-cli resolve` (design D.8): decide what each value of an operation in conflict becomes.
/// Without `--all` or `--value`, the values are listed and asked for one by one.
#[derive(Debug, Clone, Args)]
pub struct ResolveArgs {
    /// Operation ID, or a unique prefix of at least 8 hex digits.
    pub op: String,
    /// The choice for every value without a `--value`.
    #[arg(long, value_enum)]
    pub all: Option<ChoiceArg>,
    /// The choice for one value, e.g. `--value 3=before`. Repeatable; wins over `--all`.
    #[arg(long = "value", value_name = "N=CHOICE")]
    pub values: Vec<ValueChoiceArg>,
    #[command(flatten)]
    pub apply: ApplyArgs,
    #[arg(long)]
    pub yes: bool,
}

/// `mklm-cli recover` and `mklm-cli undo`.
#[derive(Debug, Clone, Args)]
pub struct RecoverArgs {
    #[command(flatten)]
    pub apply: ApplyArgs,
    #[arg(long)]
    pub yes: bool,
    /// Run the engine inside this (elevated) process instead of the helper: a last resort when
    /// the helper cannot start.
    #[arg(long, hide = true)]
    pub in_process: bool,
}

/// `mklm-cli migrate`.
#[derive(Debug, Clone, Args)]
pub struct MigrateArgs {
    /// The PC's standard layout afterwards; defaults to the current fixed layout.
    #[arg(long, value_enum)]
    pub standard: Option<StandardArg>,
    /// Also assign a layout in the same operation, e.g. `--also #2=us`. Repeatable.
    #[arg(long, value_name = "KEYBOARD=LAYOUT")]
    pub also: Vec<AlsoArg>,
    #[arg(long)]
    pub yes: bool,
}

/// `mklm-cli restore`.
#[derive(Debug, Clone, Args)]
pub struct RestoreArgs {
    /// Restore the values from before MKLM (the only kind of restore in M2).
    #[arg(long, required = true)]
    pub baseline: bool,
    /// Every value MKLM ever changed, global values included.
    #[arg(
        long,
        conflicts_with = "keyboard",
        required_unless_present = "keyboard"
    )]
    pub all: bool,
    /// Only this keyboard's physical device.
    pub keyboard: Option<KeyboardRef>,
    #[arg(long, value_enum, default_value = "report")]
    pub on_conflict: ConflictArg,
    #[command(flatten)]
    pub apply: ApplyArgs,
    /// Needs --other-input or --no-reset (design review S12).
    #[arg(long, requires = "reset_choice")]
    pub yes: bool,
    /// See [`RecoverArgs::in_process`].
    #[arg(long, hide = true)]
    pub in_process: bool,
}

/// Runs `set`.
pub fn set(args: &SetArgs) -> Result<i32> {
    bail!("`set` is not implemented yet (milestone M2)")
}

/// Runs `migrate`.
pub fn migrate(args: &MigrateArgs) -> Result<i32> {
    bail!("`migrate` is not implemented yet (milestone M2)")
}

/// Runs `revert <op>`.
pub fn revert(args: &RevertArgs) -> Result<i32> {
    bail!("`revert` is not implemented yet (milestone M2)")
}

/// Runs `undo`: undoes every open entry that is not in flight (waiting for keep/revert, for a
/// restart, or for a conflict decision), newest first (design D.10). The first thing
/// docs/recovery.md tells a user to run.
pub fn undo(args: &RecoverArgs) -> Result<i32> {
    bail!("`undo` is not implemented yet (milestone M2)")
}

/// Runs `resolve <op>`.
pub fn resolve(args: &ResolveArgs) -> Result<i32> {
    bail!("`resolve` is not implemented yet (milestone M2)")
}

/// Runs `restore --baseline`.
pub fn restore(args: &RestoreArgs) -> Result<i32> {
    bail!("`restore` is not implemented yet (milestone M2)")
}

/// Runs `recover`. When nothing was rolled back but an entry still waits for the user, says so
/// and points to `undo` (design review C7).
pub fn recover(args: &RecoverArgs) -> Result<i32> {
    bail!("`recover` is not implemented yet (milestone M2)")
}

/// Runs `keep <op>`. For an operation that took effect through a PC restart (a migration, a PS/2
/// assignment) it shows the same Raw Input table and Shift+2 test as `post-reboot` and asks
/// before sending `Confirm` (design review C2).
pub fn keep(op: &str) -> Result<i32> {
    bail!("`keep` is not implemented yet (milestone M2)")
}

/// Runs `reboot`.
pub fn reboot(yes: bool) -> Result<i32> {
    bail!("`reboot` is not implemented yet (milestone M2)")
}

/// Runs `post-reboot` (started by the RunOnce entry).
pub fn post_reboot() -> Result<i32> {
    bail!("`post-reboot` is not implemented yet (milestone M2)")
}

/// Runs `journal` (read-only).
pub fn journal(json: bool) -> Result<String> {
    bail!("`journal` is not implemented yet (milestone M2)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_refs() {
        assert_eq!("#2".parse(), Ok(KeyboardRef::ListRow(2)));
        assert!("#0".parse::<KeyboardRef>().is_err());
        assert!("#x".parse::<KeyboardRef>().is_err());
        assert_eq!(
            r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".parse(),
            Ok(KeyboardRef::InstanceId(
                r"HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000".into()
            ))
        );
        assert!("keychron".parse::<KeyboardRef>().is_err());
    }

    #[test]
    fn also_args() {
        assert_eq!(
            "#2=us".parse(),
            Ok(AlsoArg {
                keyboard: KeyboardRef::ListRow(2),
                layout: LayoutArg::Us
            })
        );
        assert!("#2".parse::<AlsoArg>().is_err());
        assert!("#2=dvorak".parse::<AlsoArg>().is_err());
    }

    #[test]
    fn value_choices() {
        assert_eq!(
            "3=keep-current".parse(),
            Ok(ValueChoiceArg {
                record: 3,
                choice: ChoiceArg::KeepCurrent
            })
        );
        assert!("3".parse::<ValueChoiceArg>().is_err());
        assert!("x=before".parse::<ValueChoiceArg>().is_err());
        assert!("3=later".parse::<ValueChoiceArg>().is_err());
    }
}
