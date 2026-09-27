//! Live reset of one keyboard devnode (M2, plan 1.4) and its state while it comes back.
//!
//! The reset is what `pnputil /restart-device` does (verified for USB in M0 #2b):
//! `SetupDiCreateDeviceInfoList(GUID_DEVCLASS_KEYBOARD)` → `SetupDiOpenDeviceInfoW(instance ID)` →
//! `SetupDiSetClassInstallParamsW(SP_PROPCHANGE_PARAMS { DIF_PROPERTYCHANGE, DICS_PROPCHANGE,
//! DICS_FLAG_CONFIGSPECIFIC, HwProfile 0 })` → `SetupDiCallClassInstaller(DIF_PROPERTYCHANGE)` →
//! `SetupDiGetDeviceInstallParamsW`: `DI_NEEDREBOOT` or `DI_NEEDRESTART` in `Flags` means the
//! change needs a PC restart. Callers check `mklm_core::live_reset_bans` first; this module does
//! not repeat the bans.
//!
//! [`restart_device`] has no deadline of its own. [`restart_device_with_deadline`] runs it on a
//! worker thread and hands back a [`PendingRestart`] when the deadline passes, which the process
//! keeps and waits for before it exits (design review C18). [`wait_for_arrival`] and
//! [`reported_type`] follow the devnode and Raw Input afterwards (design B.2).

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mklm_core::KeyboardType;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVNODE_STATUS_FLAGS, CM_Get_DevNode_Status, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW,
    CM_PROB, CR_NO_SUCH_DEVNODE, CR_SUCCESS, DI_NEEDREBOOT, DI_NEEDRESTART,
    DICS_FLAG_CONFIGSPECIFIC, DICS_PROPCHANGE, DIF_PROPERTYCHANGE, DN_STARTED,
    GUID_DEVCLASS_KEYBOARD, HDEVINFO, SP_CLASSINSTALL_HEADER, SP_DEVINFO_DATA,
    SP_DEVINSTALL_PARAMS_W, SP_PROPCHANGE_PARAMS, SetupDiCallClassInstaller,
    SetupDiCreateDeviceInfoList, SetupDiDestroyDeviceInfoList, SetupDiGetDeviceInstallParamsW,
    SetupDiOpenDeviceInfoW, SetupDiSetClassInstallParamsW,
};
use windows::Win32::Foundation::ERROR_CLASS_MISMATCH;
use windows::core::PCWSTR;

use crate::error::{Error, win32_code};
use crate::props::{check, config_ret, to_wide};
use crate::rawinfo::raw_keyboards;
use crate::sys::win32;

/// Result of [`restart_device`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestartResult {
    Restarted,
    /// `DI_NEEDREBOOT` / `DI_NEEDRESTART` was set.
    NeedsReboot,
}

/// A device information set, destroyed on drop.
struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: the set was created by SetupDiCreateDeviceInfoList and is destroyed once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// Restarts one Keyboard-class devnode in place. Synchronous and without a deadline of its own
/// (`SetupDiCallClassInstaller` can block while PnP is busy): `mklm_engine::win::WinDevices` calls
/// it on a worker thread with a deadline and keeps the process alive until it returns
/// (design review C18); [`restart_device_with_deadline`] does exactly that.
///
/// A devnode of another setup class fails with [`Error::NotKeyboard`] before anything happens.
pub fn restart_device(instance_id: &str) -> Result<RestartResult, Error> {
    // SAFETY: the class GUID is a static; no parent window.
    let set = unsafe { SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_KEYBOARD), None) }
        .map_err(|error| win32("SetupDiCreateDeviceInfoList", &error))?;
    let set = DeviceInfoSet(set);
    let id = to_wide(instance_id);
    let mut device = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    // SAFETY: `set` is a valid set of the Keyboard class; `id` is NUL-terminated; `device` has its
    // size set. A devnode of another class fails with ERROR_CLASS_MISMATCH.
    match unsafe { SetupDiOpenDeviceInfoW(set.0, PCWSTR(id.as_ptr()), None, 0, Some(&mut device)) }
    {
        Ok(()) => {}
        Err(error) if win32_code(&error) == ERROR_CLASS_MISMATCH.0 => {
            return Err(Error::NotKeyboard {
                instance_id: instance_id.to_string(),
            });
        }
        Err(error) => return Err(win32("SetupDiOpenDeviceInfoW", &error)),
    }
    let params = SP_PROPCHANGE_PARAMS {
        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
            cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
            InstallFunction: DIF_PROPERTYCHANGE,
        },
        StateChange: DICS_PROPCHANGE,
        Scope: DICS_FLAG_CONFIGSPECIFIC,
        HwProfile: 0,
    };
    // SAFETY: `device` belongs to `set`; `params` is a complete SP_PROPCHANGE_PARAMS whose header
    // starts it, and its size is passed.
    unsafe {
        SetupDiSetClassInstallParamsW(
            set.0,
            Some(&device),
            Some(&params.ClassInstallHeader),
            size_of::<SP_PROPCHANGE_PARAMS>() as u32,
        )
    }
    .map_err(|error| win32("SetupDiSetClassInstallParamsW", &error))?;
    // SAFETY: `device` belongs to `set`, which carries the class install parameters set above.
    unsafe { SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, Some(&device)) }
        .map_err(|error| win32("SetupDiCallClassInstaller", &error))?;
    let mut install = SP_DEVINSTALL_PARAMS_W {
        cbSize: size_of::<SP_DEVINSTALL_PARAMS_W>() as u32,
        ..Default::default()
    };
    // SAFETY: `device` belongs to `set`; `install` has its size set.
    unsafe { SetupDiGetDeviceInstallParamsW(set.0, Some(&device), &mut install) }
        .map_err(|error| win32("SetupDiGetDeviceInstallParamsW", &error))?;
    if install.Flags.0 & (DI_NEEDREBOOT.0 | DI_NEEDRESTART.0) != 0 {
        Ok(RestartResult::NeedsReboot)
    } else {
        Ok(RestartResult::Restarted)
    }
}

/// Outcome of [`restart_device_with_deadline`].
#[derive(Debug)]
pub enum TimedRestart {
    /// [`restart_device`] returned within the deadline.
    Finished(Result<RestartResult, Error>),
    /// It is still inside the class installer. Treat like a reboot being needed, keep the
    /// [`PendingRestart`] and wait for it before the process exits.
    TimedOut(PendingRestart),
}

/// A [`restart_device`] call that outlived its deadline, still running on its worker thread.
#[derive(Debug)]
pub struct PendingRestart {
    worker: JoinHandle<()>,
}

impl PendingRestart {
    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    /// Waits up to `timeout` for the worker to return; true when it has.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.worker.is_finished() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            thread::sleep(remaining.min(Duration::from_millis(50)));
        }
        true
    }
}

/// [`restart_device`] on a worker thread, waited for up to `deadline` (design review C18).
/// Fails only when the worker thread cannot be started.
pub fn restart_device_with_deadline(
    instance_id: &str,
    deadline: Duration,
) -> Result<TimedRestart, Error> {
    let instance_id = instance_id.to_string();
    run_with_deadline(move || restart_device(&instance_id), deadline).map(|outcome| match outcome {
        Deadline::Finished(result) => TimedRestart::Finished(result),
        Deadline::TimedOut(worker) => TimedRestart::TimedOut(PendingRestart { worker }),
    })
}

/// Outcome of [`run_with_deadline`].
enum Deadline<T> {
    Finished(T),
    TimedOut(JoinHandle<()>),
}

/// Runs `work` on its own thread and waits up to `deadline` for its result. A panic in `work` is
/// passed on to the caller.
fn run_with_deadline<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    deadline: Duration,
) -> Result<Deadline<T>, Error> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("mklm-restart-device".to_string())
        .spawn(move || {
            // The receiver is gone only when the caller stopped waiting; nothing to report then.
            let _ = sender.send(work());
        })
        .map_err(|error| Error::Win32 {
            function: "CreateThread",
            code: error.raw_os_error().map_or(0, |code| code as u32),
        })?;
    match receiver.recv_timeout(deadline) {
        Ok(value) => {
            // The worker has sent its result and is about to return.
            let _ = worker.join();
            Ok(Deadline::Finished(value))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(Deadline::TimedOut(worker)),
        Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            // Unreachable in practice: the worker always sends before it returns.
            Ok(()) => Err(Error::Timeout {
                operation: "restart worker",
            }),
        },
    }
}

/// A devnode's state, for polling its re-arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevNodeState {
    pub present: bool,
    /// `DN_*` flags (`CM_Get_DevNode_Status`), when present.
    pub status: Option<u32>,
    /// `CM_PROB_*`, when present.
    pub problem: Option<u32>,
}

impl DevNodeState {
    /// Present, `DN_STARTED` and no problem code: the driver runs again.
    pub fn is_started(&self) -> bool {
        self.present
            && self.status.is_some_and(|status| status & DN_STARTED.0 != 0)
            && self.problem == Some(0)
    }
}

/// Reads [`DevNodeState`] (`CM_Locate_DevNodeW` without the phantom flag, then
/// `CM_Get_DevNode_Status`). A devnode that is not present (or does not exist) is
/// `present: false`.
pub fn devnode_state(instance_id: &str) -> Result<DevNodeState, Error> {
    const ABSENT: DevNodeState = DevNodeState {
        present: false,
        status: None,
        problem: None,
    };
    if instance_id.trim().is_empty() || instance_id.contains('\0') {
        return Ok(ABSENT);
    }
    let id = to_wide(instance_id);
    let mut devinst = 0u32;
    // SAFETY: `id` is NUL-terminated and outlives the call; `devinst` is a valid out pointer.
    let cr =
        unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
    if cr == CR_NO_SUCH_DEVNODE {
        return Ok(ABSENT);
    }
    check(cr, "CM_Locate_DevNodeW")?;
    let mut status = CM_DEVNODE_STATUS_FLAGS(0);
    let mut problem = CM_PROB(0);
    // SAFETY: `devinst` was just located; both out pointers are valid.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    match cr {
        CR_SUCCESS => Ok(DevNodeState {
            present: true,
            status: Some(status.0),
            problem: Some(problem.0),
        }),
        CR_NO_SUCH_DEVNODE => Ok(ABSENT),
        other => Err(config_ret("CM_Get_DevNode_Status", other)),
    }
}

/// Type/subtype Raw Input reports for `instance_id` now; `None` when it does not list it.
pub fn reported_type(instance_id: &str) -> Result<Option<KeyboardType>, Error> {
    let mut issues = Vec::new();
    Ok(raw_keyboards(&mut issues)?
        .into_iter()
        .find(|keyboard| {
            keyboard
                .instance_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(instance_id))
        })
        .map(|keyboard| keyboard.keyboard_type))
}

/// Result of [`wait_for_arrival`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceArrival {
    /// Present, `DN_STARTED`, no problem code. `reported` is what Raw Input reports, or `None`
    /// when it had not listed the keyboard by the deadline.
    Started { reported: Option<KeyboardType> },
    /// Not started by the deadline.
    TimedOut,
}

/// Polls every `poll` until the devnode is started and Raw Input lists it, or `timeout` (design
/// B.2). A devnode that started but never appeared in Raw Input is `Started { reported: None }`
/// at the deadline. A Raw Input failure counts as "not listed yet".
pub fn wait_for_arrival(
    instance_id: &str,
    timeout: Duration,
    poll: Duration,
) -> Result<DeviceArrival, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        let started = devnode_state(instance_id)?.is_started();
        if started && let Some(reported) = reported_type(instance_id).ok().flatten() {
            return Ok(DeviceArrival::Started {
                reported: Some(reported),
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(if started {
                DeviceArrival::Started { reported: None }
            } else {
                DeviceArrival::TimedOut
            });
        }
        thread::sleep(poll.min(remaining));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard_instance_ids;

    #[test]
    fn unknown_devnodes_are_absent() {
        let absent = DevNodeState {
            present: false,
            status: None,
            problem: None,
        };
        assert_eq!(devnode_state(r"HID\MKLM_NO_SUCH_DEVICE\0"), Ok(absent));
        assert_eq!(devnode_state(""), Ok(absent));
        assert!(!absent.is_started());
        assert_eq!(reported_type(r"HID\MKLM_NO_SUCH_DEVICE\0"), Ok(None));
    }

    #[test]
    fn present_keyboards_have_a_state() {
        // Read-only. Machines without a keyboard (some CI runners) skip the check.
        let Ok(ids) = keyboard_instance_ids(false) else {
            return;
        };
        for id in ids {
            let state = devnode_state(&id).expect("state of a present keyboard");
            assert!(state.present, "{id}");
            assert!(state.status.is_some());
        }
    }

    #[test]
    fn started_needs_the_flag_and_no_problem() {
        let started = DevNodeState {
            present: true,
            status: Some(0x0180_000A),
            problem: Some(0),
        };
        assert!(started.is_started());
        assert!(
            !DevNodeState {
                problem: Some(10),
                ..started
            }
            .is_started()
        );
        assert!(
            !DevNodeState {
                status: Some(0x0180_0002),
                ..started
            }
            .is_started()
        );
    }

    #[test]
    fn waiting_for_a_missing_keyboard_times_out() {
        let started = Instant::now();
        assert_eq!(
            wait_for_arrival(
                r"HID\MKLM_NO_SUCH_DEVICE\0",
                Duration::from_millis(60),
                Duration::from_millis(20)
            ),
            Ok(DeviceArrival::TimedOut)
        );
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn deadlines_hand_back_slow_work() {
        // Stand-ins for the class installer call; no device is touched.
        match run_with_deadline(|| 7, Duration::from_secs(5)) {
            Ok(Deadline::Finished(7)) => {}
            _ => panic!("fast work should finish"),
        }
        let slow = run_with_deadline(
            || thread::sleep(Duration::from_millis(300)),
            Duration::from_millis(20),
        );
        let Ok(Deadline::TimedOut(worker)) = slow else {
            panic!("slow work should time out");
        };
        let pending = PendingRestart { worker };
        assert!(!pending.is_finished());
        assert!(!pending.wait(Duration::from_millis(10)));
        assert!(pending.wait(Duration::from_secs(10)));
        assert!(pending.is_finished());
    }
}
