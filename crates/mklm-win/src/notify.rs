//! Keyboard arrival and removal notifications for the GUI (design m3 A.4): the keyboard list is
//! read again when a keyboard interface appears or goes away (a dongle plugged in, a Bluetooth
//! keyboard reconnecting after a change, a reset by the helper).
//!
//! `CM_Register_Notification(CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, GUID_DEVINTERFACE_KEYBOARD)`
//! needs no window. Its callback runs on a CfgMgr32 thread-pool thread, where
//! `CM_Unregister_Notification` must never be called (it waits for running callbacks, so it would
//! deadlock). The callback therefore only converts the event and posts it to a channel; the
//! caller's `on_change` runs on a dispatcher thread of its own (`mklm-keyboards`), outside any
//! CfgMgr32 callback, so that nothing it does can unregister inside the callback. `on_change` in
//! turn only schedules work (`slint::invoke_from_event_loop`); the GUI debounces and reads on its
//! I/O thread.

use std::ffi::c_void;
use std::mem::offset_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_EVENT_DATA_0_0,
    CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0,
    CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification, CM_Unregister_Notification,
    HCMNOTIFICATION,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::core::GUID;

use crate::error::Error;
use crate::props::{check, from_wide};

/// `GUID_DEVINTERFACE_KEYBOARD` (hidclass.h / ntddkbd.h): the interface every keyboard devnode
/// (kbdhid and i8042prt alike) registers, and the class Raw Input names keyboards by.
const GUID_DEVINTERFACE_KEYBOARD: GUID = GUID::from_u128(0x884b96c3_56ef_11d1_bc8c_00a0c91405dd);

/// Name of the dispatcher thread.
const DISPATCHER_THREAD: &str = "mklm-keyboards";

/// What changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceChange {
    /// A keyboard interface arrived (`CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL`); the interface
    /// path, as Raw Input names it (see [`crate::instance_id_from_interface_path`]).
    Arrived(String),
    /// A keyboard interface was removed (`CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL`).
    Removed(String),
}

/// What the CfgMgr32 callback reaches through its context pointer: only the sending end of the
/// channel. Owned by the [`KeyboardWatcher`], freed after `CM_Unregister_Notification` returned.
struct Context {
    sender: Sender<DeviceChange>,
}

/// A registered notification; dropping it unregisters (`CM_Unregister_Notification`, which waits
/// for running callbacks) and waits for an `on_change` call in progress, after which `on_change`
/// is never called again. Drop it on a thread that `on_change` never waits for.
pub struct KeyboardWatcher {
    registration: HCMNOTIFICATION,
    /// `Box::into_raw` of the callback's [`Context`].
    context: *mut Context,
    /// Set when dropping starts: events still queued are discarded.
    stopped: Arc<AtomicBool>,
    dispatcher: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for KeyboardWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardWatcher")
            .field("registration", &self.registration)
            .finish_non_exhaustive()
    }
}

impl KeyboardWatcher {
    /// Registers for keyboard interface arrivals and removals and calls `on_change` for each, in
    /// order, on the dispatcher thread `mklm-keyboards`. `on_change` must only schedule work
    /// (`slint::invoke_from_event_loop`) and must not wait for the thread that owns the watcher.
    pub fn new(on_change: impl Fn(DeviceChange) + Send + Sync + 'static) -> Result<Self, Error> {
        let (sender, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let dispatcher = spawn_dispatcher(receiver, Arc::clone(&stopped), on_change)?;
        let context = Box::into_raw(Box::new(Context { sender }));
        let filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            Flags: 0,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            Reserved: 0,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                    ClassGuid: GUID_DEVINTERFACE_KEYBOARD,
                },
            },
        };
        let mut registration = HCMNOTIFICATION(std::ptr::null_mut());
        // SAFETY: `filter` is a fully initialized interface filter that outlives the call;
        // `context` stays valid until `Drop` has unregistered (or is freed right below when the
        // registration fails, so no callback can see it); `registration` is a valid out pointer.
        let cr = unsafe {
            CM_Register_Notification(
                &filter,
                Some(context.cast_const().cast()),
                Some(on_notification),
                &mut registration,
            )
        };
        if let Err(error) = check(cr, "CM_Register_Notification") {
            // SAFETY: the registration failed, so no callback holds `context`; it came from
            // `Box::into_raw` above and is freed once, here. Dropping the sender ends the
            // dispatcher.
            drop(unsafe { Box::from_raw(context) });
            let _ = dispatcher.join();
            return Err(error);
        }
        Ok(Self {
            registration,
            context,
            stopped,
            dispatcher: Some(dispatcher),
        })
    }
}

impl Drop for KeyboardWatcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // SAFETY: `registration` came from a successful CM_Register_Notification and is
        // unregistered once, here. This never runs inside the CfgMgr32 callback: the callback only
        // posts to the channel, and `on_change` runs on the dispatcher thread.
        let _ = unsafe { CM_Unregister_Notification(self.registration) };
        // SAFETY: CM_Unregister_Notification has returned, which waits for running callbacks, so
        // no callback can use `context` any more. It came from `Box::into_raw` in `new` and is
        // freed once, here; dropping the sender ends the dispatcher's loop.
        drop(unsafe { Box::from_raw(self.context) });
        if let Some(dispatcher) = self.dispatcher.take()
            && dispatcher.thread().id() != thread::current().id()
        {
            let _ = dispatcher.join();
        }
    }
}

/// Starts the thread that calls `on_change` for every event until the channel closes.
fn spawn_dispatcher(
    receiver: Receiver<DeviceChange>,
    stopped: Arc<AtomicBool>,
    on_change: impl Fn(DeviceChange) + Send + 'static,
) -> Result<JoinHandle<()>, Error> {
    thread::Builder::new()
        .name(DISPATCHER_THREAD.to_string())
        .spawn(move || {
            for change in receiver {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                on_change(change);
            }
        })
        .map_err(|error| Error::Win32 {
            function: "CreateThread",
            code: error.raw_os_error().map_or(0, |code| code as u32),
        })
}

/// The CfgMgr32 callback (`PCM_NOTIFY_CALLBACK`). Runs on a thread-pool thread: converts the
/// event, posts it, returns `ERROR_SUCCESS`. Never unregisters, never blocks, never panics.
unsafe extern "system" fn on_notification(
    _registration: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    event: *const CM_NOTIFY_EVENT_DATA,
    event_size: u32,
) -> u32 {
    if context.is_null() {
        return ERROR_SUCCESS.0;
    }
    // SAFETY: `context` is the `Context` the watcher registered; it stays valid until
    // CM_Unregister_Notification has returned, which waits for this callback.
    let context = unsafe { &*context.cast::<Context>() };
    // SAFETY: CfgMgr32 passes an event of `event_size` readable bytes (or null).
    if let Some(change) = unsafe { device_change(action, event, event_size) } {
        // The receiver is gone only while the watcher is being dropped: nothing to deliver then.
        let _ = context.sender.send(change);
    }
    ERROR_SUCCESS.0
}

/// The change an interface event reports; `None` for other actions, other filter types and
/// events without a symbolic link.
///
/// # Safety
///
/// `event` must be null or point to `event_size` readable bytes, aligned for
/// `CM_NOTIFY_EVENT_DATA`.
unsafe fn device_change(
    action: CM_NOTIFY_ACTION,
    event: *const CM_NOTIFY_EVENT_DATA,
    event_size: u32,
) -> Option<DeviceChange> {
    let make: fn(String) -> DeviceChange = match action {
        CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL => DeviceChange::Arrived,
        CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL => DeviceChange::Removed,
        _ => return None,
    };
    let link_offset =
        offset_of!(CM_NOTIFY_EVENT_DATA, u) + offset_of!(CM_NOTIFY_EVENT_DATA_0_0, SymbolicLink);
    let size = event_size as usize;
    if event.is_null() || size <= link_offset {
        return None;
    }
    // SAFETY: `event` points to at least `size` > `link_offset` readable bytes (caller), which
    // cover the leading filter type.
    if unsafe { (*event).FilterType } != CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE {
        return None;
    }
    // SAFETY: the symbolic link starts `link_offset` bytes in (2-byte aligned inside an aligned
    // event) and the event holds `size` bytes, so `(size - link_offset) / 2` UTF-16 units are
    // readable. `from_wide` stops at the first NUL.
    let units = unsafe {
        std::slice::from_raw_parts(
            event.cast::<u8>().add(link_offset).cast::<u16>(),
            (size - link_offset) / 2,
        )
    };
    Some(from_wide(units))
        .filter(|link| !link.is_empty())
        .map(make)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    const KEYCHRON: &str = r"\\?\HID#VID_3434&PID_D027&MI_00&Col01#8&148ad7e3&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";

    /// A CfgMgr32 interface event for `link`, in an 8-byte aligned buffer, with its size in bytes.
    fn interface_event(filter: i32, link: &str) -> (Vec<u64>, u32) {
        let link_offset = offset_of!(CM_NOTIFY_EVENT_DATA, u)
            + offset_of!(CM_NOTIFY_EVENT_DATA_0_0, SymbolicLink);
        let mut bytes = vec![0u8; link_offset];
        bytes[..4].copy_from_slice(&filter.to_ne_bytes());
        bytes.extend(
            link.encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_ne_bytes),
        );
        let size = bytes.len() as u32;
        let mut words = vec![0u64; bytes.len().div_ceil(8)];
        for (index, byte) in bytes.iter().enumerate() {
            let word = &mut words[index / 8];
            let mut raw = word.to_ne_bytes();
            raw[index % 8] = *byte;
            *word = u64::from_ne_bytes(raw);
        }
        (words, size)
    }

    fn change(action: CM_NOTIFY_ACTION, event: &(Vec<u64>, u32)) -> Option<DeviceChange> {
        // SAFETY: the buffer holds `event.1` bytes and is 8-byte aligned.
        unsafe { device_change(action, event.0.as_ptr().cast(), event.1) }
    }

    #[test]
    fn interface_events_become_changes() {
        let event = interface_event(CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE.0, KEYCHRON);
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, &event),
            Some(DeviceChange::Arrived(KEYCHRON.to_string()))
        );
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, &event),
            Some(DeviceChange::Removed(KEYCHRON.to_string()))
        );
        // Other actions, other filter types, empty links and short or missing events: nothing.
        assert_eq!(change(CM_NOTIFY_ACTION(5), &event), None);
        let handle_event = interface_event(1, KEYCHRON);
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, &handle_event),
            None
        );
        let empty = interface_event(CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE.0, "");
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, &empty),
            None
        );
        let short = (event.0.clone(), 24);
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, &short),
            None
        );
        // SAFETY: a null event is allowed.
        let none = unsafe {
            device_change(
                CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
                std::ptr::null(),
                100,
            )
        };
        assert_eq!(none, None);
        // A link without its NUL within the size is cut at the size.
        let cut = (event.0.clone(), event.1 - 4);
        assert_eq!(
            change(CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL, &cut),
            Some(DeviceChange::Arrived(
                KEYCHRON[..KEYCHRON.len() - 1].to_string()
            ))
        );
    }

    /// The callback posts to the channel and the dispatcher calls `on_change` on its own thread;
    /// after `stopped`, queued events are discarded.
    #[test]
    fn the_callback_posts_and_the_dispatcher_delivers() {
        let (sender, receiver) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let record = Arc::clone(&seen);
        let dispatcher = spawn_dispatcher(receiver, Arc::clone(&stopped), move |change| {
            let name = thread::current().name().map(str::to_string);
            record.lock().expect("lock").push((change, name));
        })
        .expect("dispatcher");
        let context = Context { sender };
        let event = interface_event(CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE.0, KEYCHRON);
        // SAFETY: `context` is a live `Context`; the event buffer holds `event.1` aligned bytes.
        let result = unsafe {
            on_notification(
                HCMNOTIFICATION(std::ptr::null_mut()),
                (&raw const context).cast(),
                CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
                event.0.as_ptr().cast(),
                event.1,
            )
        };
        assert_eq!(result, ERROR_SUCCESS.0);
        drop(context);
        dispatcher.join().expect("dispatcher thread");
        assert_eq!(
            *seen.lock().expect("lock"),
            vec![(
                DeviceChange::Arrived(KEYCHRON.to_string()),
                Some(DISPATCHER_THREAD.to_string())
            )]
        );
    }

    /// Registering and unregistering the real notification (read only: nothing is changed on the
    /// machine) neither fails nor hangs.
    #[test]
    fn registers_and_unregisters() {
        let watcher = KeyboardWatcher::new(|_| {}).expect("CM_Register_Notification");
        thread::sleep(Duration::from_millis(50));
        drop(watcher);
    }
}
