//! H2's COM set-up and the unelevated relaunch of the GUI through the desktop shell (design m5b
//! D.7 step 1, D.10; SECURITY-8).
//!
//! The elevated update runner must never start the GUI with its own token (it would run elevated,
//! and as the administrator whose credentials were typed into UAC). It asks the desktop's Explorer
//! instead, the way plan 4.2 step 8 describes: `ShellWindows` (a local server that runs as the
//! session's interactive user) → the desktop's top-level browser → its view's background
//! `IShellFolderViewDual` → `IShellDispatch2::ShellExecute`. Explorer then starts the GUI as the
//! signed-in user, unelevated.
//!
//! Because the runner receives interface pointers from a medium-integrity Explorer, it fixes the
//! process's COM security first, before any other COM call: identify-level impersonation only,
//! no custom marshalling, no activate-as-activator (`EOAC_NO_CUSTOM_MARSHAL |
//! EOAC_DISABLE_AAA`).

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoInitializeSecurity, CoUninitialize, EOAC_DISABLE_AAA, EOAC_NO_CUSTOM_MARSHAL,
    EOLE_AUTHENTICATION_CAPABILITIES, IDispatch, IServiceProvider, RPC_C_AUTHN_LEVEL_DEFAULT,
    RPC_C_IMP_LEVEL_IDENTIFY,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    CSIDL_DESKTOP, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows,
    SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{BSTR, Interface};

use crate::error::Error;
use crate::sys::win32;

/// H2's COM set-up, before any other COM call: `CoInitializeEx(COINIT_MULTITHREADED)` and
/// `CoInitializeSecurity` with `RPC_C_IMP_LEVEL_IDENTIFY` and
/// `EOAC_NO_CUSTOM_MARSHAL | EOAC_DISABLE_AAA` (SECURITY-8). Uninitializes on drop.
#[derive(Debug)]
pub struct RunnerCom {
    _private: (),
}

impl Drop for RunnerCom {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx of `init_com_for_runner` on this thread.
        unsafe { CoUninitialize() };
    }
}

/// See [`RunnerCom`]. Fails (and leaves COM uninitialized) when the security was already set in
/// this process (`RPC_E_TOO_LATE`: some COM call came first), which the runner treats as fatal.
pub fn init_com_for_runner() -> Result<RunnerCom, Error> {
    // SAFETY: no reserved pointer; the flag is a constant. Balanced by `RunnerCom::drop`.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| win32("CoInitializeEx", &error))?;
    let com = RunnerCom { _private: () };
    let capabilities =
        EOLE_AUTHENTICATION_CAPABILITIES(EOAC_NO_CUSTOM_MARSHAL.0 | EOAC_DISABLE_AAA.0);
    // SAFETY: no security descriptor (the default access permissions), -1 authentication
    // services (COM chooses), no reserved pointers, no authentication list.
    unsafe {
        CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_DEFAULT,
            RPC_C_IMP_LEVEL_IDENTIFY,
            None,
            capabilities,
            None,
        )
    }
    .map_err(|error| win32("CoInitializeSecurity", &error))?;
    Ok(com)
}

/// Starts `exe arguments` through the desktop shell of this session (IShellWindows →
/// IShellDispatch2::ShellExecute): as the session's interactive user, unelevated. Gives up after
/// `timeout`. Never falls back to this process's own token.
///
/// The COM calls run on a thread of their own (joined to the process's multithreaded apartment),
/// so that a hung Explorer cannot hold up the caller longer than `timeout`; that thread is left to
/// finish on its own then.
pub fn launch_via_shell(
    exe: &Path,
    arguments: &str,
    directory: &Path,
    timeout: Duration,
) -> Result<(), Error> {
    let exe = exe.to_path_buf();
    let arguments = arguments.to_string();
    let directory = directory.to_path_buf();
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("mklm-relaunch".into())
        .spawn(move || {
            let _ = sender.send(shell_execute(&exe, &arguments, &directory));
        })
        .map_err(|_| Error::Win32 {
            function: "CreateThread",
            code: 0,
        })?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout {
            operation: "launch_via_shell",
        }),
    }
}

/// The chain of plan 4.2 step 8, on the calling thread.
fn shell_execute(exe: &Path, arguments: &str, directory: &Path) -> Result<(), Error> {
    // SAFETY: no reserved pointer; the flag is a constant. Balanced below.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| win32("CoInitializeEx", &error))?;
    let result = shell_execute_in_mta(exe, arguments, directory);
    // SAFETY: balances the successful CoInitializeEx above, after every interface is released
    // (they are dropped inside `shell_execute_in_mta`).
    unsafe { CoUninitialize() };
    result
}

fn shell_execute_in_mta(exe: &Path, arguments: &str, directory: &Path) -> Result<(), Error> {
    let text = |path: &Path| {
        path.to_str()
            .map(BSTR::from)
            .ok_or_else(|| Error::UnexpectedData {
                path: path.display().to_string(),
            })
    };
    // SAFETY: a registered class and interface; out-of-process only (the shell's local server).
    let windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER) }
            .map_err(|error| win32("CoCreateInstance(ShellWindows)", &error))?;
    let location = VARIANT::from(CSIDL_DESKTOP as i32);
    let root = VARIANT::default();
    let mut hwnd = 0i32;
    // SAFETY: both VARIANTs and `hwnd` are valid for the call.
    let desktop: IDispatch = unsafe {
        windows.FindWindowSW(&location, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
    }
    .map_err(|error| win32("IShellWindows::FindWindowSW", &error))?;
    let provider: IServiceProvider = desktop
        .cast()
        .map_err(|error| win32("QueryInterface(IServiceProvider)", &error))?;
    // SAFETY: a static service GUID.
    let browser: IShellBrowser = unsafe { provider.QueryService(&SID_STopLevelBrowser) }
        .map_err(|error| win32("IServiceProvider::QueryService", &error))?;
    // SAFETY: plain COM call on a live interface.
    let view: IShellView = unsafe { browser.QueryActiveShellView() }
        .map_err(|error| win32("IShellBrowser::QueryActiveShellView", &error))?;
    // SAFETY: plain COM call on a live interface.
    let background: IDispatch = unsafe { view.GetItemObject(SVGIO_BACKGROUND) }
        .map_err(|error| win32("IShellView::GetItemObject", &error))?;
    let folder_view: IShellFolderViewDual = background
        .cast()
        .map_err(|error| win32("QueryInterface(IShellFolderViewDual)", &error))?;
    // SAFETY: plain COM call on a live interface.
    let application: IDispatch = unsafe { folder_view.Application() }
        .map_err(|error| win32("IShellFolderViewDual::Application", &error))?;
    let shell: IShellDispatch2 = application
        .cast()
        .map_err(|error| win32("QueryInterface(IShellDispatch2)", &error))?;
    let file = text(exe)?;
    let arguments = VARIANT::from(BSTR::from(arguments));
    let directory = VARIANT::from(text(directory)?);
    let operation = VARIANT::from(BSTR::from("open"));
    let show = VARIANT::from(SW_SHOWNORMAL.0);
    // SAFETY: every argument is a valid BSTR or VARIANT that outlives the call.
    unsafe { shell.ShellExecute(&file, &arguments, &directory, &operation, &show) }
        .map_err(|error| win32("IShellDispatch2::ShellExecute", &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Neither the relaunch (it would make the desktop's Explorer start a program) nor the
    /// runner's COM set-up (it fixes a process's COM security once, for every later test) runs in
    /// unit tests: both belong to the rehearsal (design m5b F.6). Only the flags are checked.
    #[test]
    fn the_runner_security_flags() {
        let capabilities = EOAC_NO_CUSTOM_MARSHAL.0 | EOAC_DISABLE_AAA.0;
        assert_eq!(capabilities, 0x2000 | 0x1000);
        assert_eq!(RPC_C_IMP_LEVEL_IDENTIFY.0, 2);
    }
}
