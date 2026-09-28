//! Start in the taskbar corner at sign-in (plan 3.9; design m3 F.3): the GUI's own HKCU Run
//! value `SHINDATACENTER.MKLM` = `"<dir>\mklm.exe" --tray`, written through `mklm_win::session`
//! (the only HKCU writer). On by default: the wizard's last step turns it on.
//!
//! Rules:
//! - The setting shown is the value itself (`autostart_state`), not a copy in settings.toml.
//! - At start, when the value exists but names another path because MKLM moved — the program it
//!   names is gone — rewrite it ([`AutostartTask::Repair`]). A value naming another copy that
//!   still exists is left alone: running a second copy (a development build, a copy on a USB
//!   stick) must not take the sign-in start over unasked. When Task Manager disabled it
//!   (`disabled_by_user`), never turn it back on — the switch is off and unavailable, with
//!   "Windows のスタートアップ設定で無効になっています" next to it ([`view`]). Registering never
//!   touches Task Manager's state.
//! - Turning it off removes the value; M5's uninstaller removes it too.
//!
//! Every read and write runs on the I/O worker ([`run`], Windows only); the decisions
//! ([`write_for`], [`view`]) are pure and work on a copy of the value ([`RunValue`]).

use std::path::Path;

pub use crate::i18n::AutostartNote;

/// The Run value as read (`mklm_win::session::AutostartState`, copied so that the decisions here
/// and the state that keeps them stay pure).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunValue {
    /// The registered command line, if any.
    pub command_line: Option<String>,
    /// Task Manager (or Settings > Apps > Startup) turned it off; MKLM never turns it back on.
    pub disabled_by_user: bool,
}

/// The command line to register for `exe`.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// Whether the registered value names another command line than `expected` (and Task Manager did
/// not turn it off). [`write_for`] rewrites it only when that program is gone.
pub fn needs_repair(state: &RunValue, expected: &str) -> bool {
    state
        .command_line
        .as_deref()
        .is_some_and(|registered| !registered.eq_ignore_ascii_case(expected))
        && !state.disabled_by_user
}

/// The program a registered command line starts: the quoted path at its start.
pub fn registered_program(command_line: &str) -> Option<&Path> {
    command_line
        .strip_prefix('"')
        .and_then(|tail| tail.split_once('"'))
        .map(|(program, _)| Path::new(program))
        .filter(|program| !program.as_os_str().is_empty())
}

/// A job for the I/O worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartTask {
    /// Read the value (the settings page is opened).
    Read,
    /// At start: rewrite a value that names another path.
    Repair,
    /// The settings switch (or the wizard's last step): register (true) or remove (false).
    Set(bool),
}

/// What the I/O worker found after a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartReport {
    /// The value after the task; `Err` with the English reason when it could not be read.
    pub state: Result<RunValue, String>,
    /// The write the task needed failed (English reason).
    pub write_error: Option<String>,
}

/// A write to the Run value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartWrite {
    Register(String),
    Unregister,
}

/// The write `task` needs, given the value as read, the command line for this executable (`None`
/// when its path is unknown) and whether a program exists (for the repair). `Err` when the task
/// cannot be done at all.
pub fn write_for(
    task: AutostartTask,
    state: &RunValue,
    expected: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Result<Option<AutostartWrite>, String> {
    match task {
        AutostartTask::Read => Ok(None),
        AutostartTask::Repair => {
            let moved = state.command_line.as_deref().is_some_and(|line| {
                registered_program(line).is_none_or(|program| !exists(program))
            });
            Ok(expected
                .filter(|expected| moved && needs_repair(state, expected))
                .map(|expected| AutostartWrite::Register(expected.to_string())))
        }
        AutostartTask::Set(true) => match expected {
            Some(expected)
                if state
                    .command_line
                    .as_deref()
                    .is_some_and(|registered| registered.eq_ignore_ascii_case(expected)) =>
            {
                Ok(None)
            }
            Some(expected) => Ok(Some(AutostartWrite::Register(expected.to_string()))),
            None => Err("the path of this program is unknown".to_string()),
        },
        AutostartTask::Set(false) => Ok(state
            .command_line
            .is_some()
            .then_some(AutostartWrite::Unregister)),
    }
}

/// What the settings page shows for the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartView {
    /// Checked: MKLM starts at sign-in.
    pub on: bool,
    /// The switch can be used.
    pub available: bool,
    pub note: Option<AutostartNote>,
}

/// The switch for the last report (`None`: not read yet).
pub fn view(report: Option<&AutostartReport>) -> AutostartView {
    let Some(report) = report else {
        return AutostartView {
            on: false,
            available: false,
            note: None,
        };
    };
    match &report.state {
        Err(_) => AutostartView {
            on: false,
            available: false,
            note: Some(AutostartNote::Unreadable),
        },
        Ok(state) if state.disabled_by_user => AutostartView {
            on: false,
            available: false,
            note: Some(AutostartNote::DisabledByUser),
        },
        Ok(state) => AutostartView {
            on: state.command_line.is_some(),
            available: true,
            note: report
                .write_error
                .as_ref()
                .map(|_| AutostartNote::WriteFailed),
        },
    }
}

/// Reads the Run value and its Task Manager state (registry reads only).
#[cfg(windows)]
fn read_value() -> Result<RunValue, mklm_win::Error> {
    mklm_win::session::autostart_state().map(|state| RunValue {
        command_line: state.command_line,
        disabled_by_user: state.disabled_by_user,
    })
}

/// Runs `task` (I/O worker only: registry reads and writes): reads the value, makes the write the
/// task needs, and reads it again for the settings page.
#[cfg(windows)]
pub fn run(task: AutostartTask) -> AutostartReport {
    let expected = std::env::current_exe().ok().map(|exe| command_line(&exe));
    let before = match read_value() {
        Ok(state) => state,
        Err(error) => {
            crate::log::warn(format!("autostart: reading the Run value failed: {error}"));
            return AutostartReport {
                state: Err(error.to_string()),
                write_error: None,
            };
        }
    };
    let write_error = match write_for(task, &before, expected.as_deref(), Path::is_file) {
        Ok(None) => None,
        Ok(Some(write)) => {
            let (what, result) = match &write {
                AutostartWrite::Register(line) => {
                    ("registering", mklm_win::session::register_autostart(line))
                }
                AutostartWrite::Unregister => {
                    ("removing", mklm_win::session::unregister_autostart())
                }
            };
            match result {
                Ok(()) => {
                    crate::log::info(format!("autostart: {what} the Run value ({task:?})"));
                    None
                }
                Err(error) => {
                    crate::log::warn(format!("autostart: {what} the Run value failed: {error}"));
                    Some(error.to_string())
                }
            }
        }
        Err(reason) => {
            crate::log::warn(format!("autostart: {task:?} failed: {reason}"));
            Some(reason)
        }
    };
    AutostartReport {
        state: read_value().map_err(|error| error.to_string()),
        write_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Program Files\MKLM\mklm.exe";

    fn expected() -> String {
        command_line(Path::new(EXE))
    }

    fn registered(line: &str) -> RunValue {
        RunValue {
            command_line: Some(line.into()),
            disabled_by_user: false,
        }
    }

    #[test]
    fn repair_rules() {
        let expected = expected();
        assert_eq!(expected, r#""C:\Program Files\MKLM\mklm.exe" --tray"#);
        let moved = registered(r#""D:\old\mklm.exe" --tray"#);
        assert!(needs_repair(&moved, &expected));
        let disabled = RunValue {
            disabled_by_user: true,
            ..moved.clone()
        };
        assert!(!needs_repair(&disabled, &expected));
        assert!(!needs_repair(&RunValue::default(), &expected));
        // Case differences are the same path.
        assert!(!needs_repair(
            &registered(&expected.to_uppercase()),
            &expected
        ));
    }

    fn gone(_: &Path) -> bool {
        false
    }

    fn there(_: &Path) -> bool {
        true
    }

    #[test]
    fn the_program_of_a_command_line() {
        assert_eq!(registered_program(&expected()), Some(Path::new(EXE)));
        assert_eq!(
            registered_program(r#""D:\old\mklm.exe""#),
            Some(Path::new(r"D:\old\mklm.exe"))
        );
        for bad in [
            r"C:\MKLM\mklm.exe --tray",
            r#""" --tray"#,
            r#""C:\MKLM\mklm.exe"#,
            "",
        ] {
            assert_eq!(registered_program(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_start_rewrites_only_a_moved_value() {
        let expected = expected();
        let moved = registered(r#""D:\old\mklm.exe" --tray"#);
        assert_eq!(
            write_for(AutostartTask::Repair, &moved, Some(&expected), gone),
            Ok(Some(AutostartWrite::Register(expected.clone())))
        );
        // Another copy that still exists (a development build, a second copy) keeps the value.
        assert_eq!(
            write_for(AutostartTask::Repair, &moved, Some(&expected), there),
            Ok(None)
        );
        // A value that is not a quoted program at all is repaired.
        assert_eq!(
            write_for(
                AutostartTask::Repair,
                &registered(r"D:\old\mklm.exe --tray"),
                Some(&expected),
                there
            ),
            Ok(Some(AutostartWrite::Register(expected.clone())))
        );
        // Nothing registered (the user turned it off, or never on): the start writes nothing.
        assert_eq!(
            write_for(
                AutostartTask::Repair,
                &RunValue::default(),
                Some(&expected),
                gone
            ),
            Ok(None)
        );
        // Turned off in Task Manager: never touched.
        let disabled = RunValue {
            disabled_by_user: true,
            ..moved.clone()
        };
        assert_eq!(
            write_for(AutostartTask::Repair, &disabled, Some(&expected), gone),
            Ok(None)
        );
        assert_eq!(
            write_for(
                AutostartTask::Repair,
                &registered(&expected),
                Some(&expected),
                gone
            ),
            Ok(None)
        );
        // This executable's path is unknown: nothing to compare with, nothing written.
        assert_eq!(
            write_for(AutostartTask::Repair, &moved, None, gone),
            Ok(None)
        );
        assert_eq!(
            write_for(AutostartTask::Read, &moved, Some(&expected), gone),
            Ok(None)
        );
    }

    #[test]
    fn the_switch_writes_only_what_changes() {
        let expected = expected();
        let none = RunValue::default();
        assert_eq!(
            write_for(AutostartTask::Set(true), &none, Some(&expected), there),
            Ok(Some(AutostartWrite::Register(expected.clone())))
        );
        assert_eq!(
            write_for(
                AutostartTask::Set(true),
                &registered(&expected),
                Some(&expected),
                there
            ),
            Ok(None)
        );
        // Turning it on where another copy is registered points it at this one.
        assert_eq!(
            write_for(
                AutostartTask::Set(true),
                &registered(r#""D:\old\mklm.exe" --tray"#),
                Some(&expected),
                there
            ),
            Ok(Some(AutostartWrite::Register(expected.clone())))
        );
        assert!(write_for(AutostartTask::Set(true), &none, None, there).is_err());
        assert_eq!(
            write_for(
                AutostartTask::Set(false),
                &registered(&expected),
                None,
                there
            ),
            Ok(Some(AutostartWrite::Unregister))
        );
        assert_eq!(
            write_for(AutostartTask::Set(false), &none, Some(&expected), there),
            Ok(None)
        );
    }

    #[test]
    fn the_switch_shows_the_value_itself() {
        assert_eq!(
            view(None),
            AutostartView {
                on: false,
                available: false,
                note: None
            }
        );
        let on = AutostartReport {
            state: Ok(registered(&expected())),
            write_error: None,
        };
        assert_eq!(
            view(Some(&on)),
            AutostartView {
                on: true,
                available: true,
                note: None
            }
        );
        // Disabled in Task Manager: off, unavailable, and said so.
        let disabled = AutostartReport {
            state: Ok(RunValue {
                disabled_by_user: true,
                ..registered(&expected())
            }),
            write_error: None,
        };
        assert_eq!(
            view(Some(&disabled)),
            AutostartView {
                on: false,
                available: false,
                note: Some(AutostartNote::DisabledByUser)
            }
        );
        let failed = AutostartReport {
            state: Ok(RunValue::default()),
            write_error: Some("access denied".into()),
        };
        assert_eq!(
            view(Some(&failed)),
            AutostartView {
                on: false,
                available: true,
                note: Some(AutostartNote::WriteFailed)
            }
        );
        let unreadable = AutostartReport {
            state: Err("access denied".into()),
            write_error: None,
        };
        assert_eq!(
            view(Some(&unreadable)).note,
            Some(AutostartNote::Unreadable)
        );
        assert!(!view(Some(&unreadable)).available);
    }
}
