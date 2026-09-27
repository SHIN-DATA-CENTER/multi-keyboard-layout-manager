//! The notification-area icon (plan 3.9; design m3 B.16): Slint's `SystemTrayIcon` as proven by
//! the prototype (M0 #8 E, F), re-created when explorer restarts (`TaskbarCreated` from the shell
//! window, because Slint 1.18.1 does not re-add it).
//!
//! The one-time close notice is not a balloon: Windows 11 swallows balloons under Do Not Disturb
//! or with notifications off, and hides new icons behind "^", so the first × always shows the
//! in-window notice (`Overlay::CloseNotice`, `state::update` on `WindowCloseRequested`) and hides
//! the window only on its OK (design m3 B.16, review U13).

use slint::{Image, PlatformError};

use crate::app::dispatch;
use crate::state::AppMsg;
use crate::ui::TrayIcon;

/// Creates the icon with its menu wired to the app.
pub fn create_tray(icon: &Image) -> Result<TrayIcon, PlatformError> {
    let tray = TrayIcon::new()?;
    tray.set_tray_image(icon.clone());
    tray.on_icon_clicked(|| dispatch(AppMsg::Activate));
    tray.on_show_window(|| dispatch(AppMsg::Activate));
    tray.on_identify(|| dispatch(AppMsg::ToggleIdentify));
    // WP-U5: the undo preview (design m3 B.12) through the recovery page.
    tray.on_undo_open(|| dispatch(AppMsg::Navigate(crate::state::Page::Recovery)));
    tray.on_quit_app(|| dispatch(AppMsg::QuitRequested));
    tray.show()?;
    Ok(tray)
}
