//! Keyboard arrival and removal notifications for the GUI (design m3 A.4): the keyboard list is
//! read again when a keyboard interface appears or goes away (a dongle plugged in, a Bluetooth
//! keyboard reconnecting after a change, a reset by the helper).
//!
//! `CM_Register_Notification(CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, GUID_DEVINTERFACE_KEYBOARD)`
//! needs no window. The callback runs on a CfgMgr32 thread-pool thread: it must only schedule
//! work (`slint::invoke_from_event_loop`); the GUI debounces and reads on its I/O thread.

use crate::error::Error;

/// What changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChange {
    /// A keyboard interface arrived (`CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL`); the interface
    /// path, as Raw Input names it.
    Arrived(String),
    /// A keyboard interface was removed (`CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL`).
    Removed(String),
}

/// A registered notification; dropping it unregisters (`CM_Unregister_Notification`, which waits
/// for running callbacks).
#[derive(Debug)]
pub struct KeyboardWatcher {
    _private: (),
}

impl KeyboardWatcher {
    /// Registers `on_change`. Implementation (WP-W1): box the callback, pass it as the context,
    /// convert `CM_NOTIFY_EVENT_DATA.u.DeviceInterface.SymbolicLink` to a `String`, return
    /// `ERROR_SUCCESS` from the callback.
    pub fn new(on_change: impl Fn(DeviceChange) + Send + Sync + 'static) -> Result<Self, Error> {
        let _ = on_change;
        todo!("WP-W1: CM_Register_Notification for GUID_DEVINTERFACE_KEYBOARD")
    }
}
