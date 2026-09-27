//! Which keyboard typed (plan 2.2 "Raw Input", 3.3; M0 #8): winit's `DeviceEvent::Key` through
//! Slint's custom application handler. MKLM registers no Raw Input of its own; winit delivers
//! device events only while the window has the focus (`DeviceEvents::WhenFocused`), which is what
//! plan 2.2 asks for.
//!
//! `persistent_identifier()` is the interface path (`\?\HID#VID_…`); it is resolved to the
//! devnode instance ID once per `DeviceId` and cached until the device is added or removed again
//! (handles are reused). Only the instance ID, the scan code and the time leave this module —
//! never a character (plan 2.2: key contents are not logged).
//!
//! Pointer input (buttons, movement, wheel) is reported too, as a time only and at most once per
//! [`POINTER_REPORT_INTERVAL`]: a mouse user has another way to type (the on-screen keyboard),
//! which makes "switch now" the default apply method (design m3 B.5, review U1).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use slint::winit_030::winit::event::{DeviceEvent, DeviceId, ElementState, RawKeyEvent};
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::winit::platform::scancode::PhysicalKeyExtScancode;
use slint::winit_030::winit::platform::windows::DeviceIdExtWindows;
use slint::winit_030::{CustomApplicationHandler, EventResult};

use crate::app::dispatch;
use crate::state::AppMsg;

/// How often pointer input reaches the state at most.
pub const POINTER_REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct InputCapture {
    /// `None`: the device's instance ID could not be resolved (ignored).
    cache: HashMap<DeviceId, Option<String>>,
    last_pointer_report: Option<Instant>,
}

impl InputCapture {
    fn instance_id(&mut self, device_id: DeviceId) -> Option<String> {
        self.cache
            .entry(device_id)
            .or_insert_with(|| {
                device_id
                    .persistent_identifier()
                    .and_then(|path| mklm_win::instance_id_from_interface_path(&path))
            })
            .clone()
    }

    fn pointer(&mut self) {
        let now = Instant::now();
        if self
            .last_pointer_report
            .is_none_or(|last| now.duration_since(last) >= POINTER_REPORT_INTERVAL)
        {
            self.last_pointer_report = Some(now);
            dispatch(AppMsg::PointerUsed(now));
        }
    }
}

impl CustomApplicationHandler for InputCapture {
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        device_id: DeviceId,
        event: DeviceEvent,
    ) -> EventResult {
        match event {
            DeviceEvent::Key(RawKeyEvent {
                physical_key,
                state: ElementState::Pressed,
            }) => {
                if let (Some(instance_id), Some(scancode)) =
                    (self.instance_id(device_id), physical_key.to_scancode())
                {
                    // The UI thread already: handled at once, so that the key test sees the
                    // device before the window's key event (M0 #8, prototype README 3).
                    dispatch(AppMsg::DeviceKey {
                        instance_id,
                        scancode,
                        at: Instant::now(),
                    });
                }
            }
            DeviceEvent::Button { .. }
            | DeviceEvent::MouseMotion { .. }
            | DeviceEvent::MouseWheel { .. } => self.pointer(),
            DeviceEvent::Added | DeviceEvent::Removed => {
                self.cache.remove(&device_id);
            }
            _ => {}
        }
        EventResult::Propagate
    }
}
