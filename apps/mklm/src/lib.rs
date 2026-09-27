//! The GUI of Multi Keyboard Layout Manager (milestone M3; docs/design/m3-gui.md), as a library
//! so that its pure parts are tested without a window; `src/main.rs` only calls [`app::run`].
//!
//! Runs as the user (`asInvoker`) and never writes the machine's keyboard settings: every write
//! goes through `mklm-helper.exe`, launched with UAC by the shared client crate (`mklm-client`),
//! exactly as the CLI does (design m3 A.2). The GUI writes only per-user things: its
//! `settings.toml`, the HKCU autostart value and the post-reboot RunOnce value.
//!
//! Threads (design m3 A.4): the Slint UI thread; one session worker per helper session
//! ([`worker`]); one I/O worker for every other blocking call ([`reader`]); watchers that marshal
//! onto the UI thread with `slint::invoke_from_event_loop` ([`watchers`]).
//!
//! Pure modules (tested without a window): [`args`], [`theme`], [`i18n`], [`settings`],
//! [`state`], [`detect`] and the view-models in [`vm`].

#![deny(unsafe_code)]

pub mod args;
pub mod detect;
pub mod i18n;
pub mod icon;
pub mod settings;
pub mod state;
pub mod theme;
pub mod vm;

#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub mod autostart;
#[cfg(windows)]
pub mod input_capture;
#[cfg(windows)]
pub mod reader;
#[cfg(windows)]
pub mod single_instance;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod watchers;
#[cfg(windows)]
pub mod worker;

/// Code generated from `ui/app.slint` (design m3 B).
#[allow(
    missing_debug_implementations,
    unsafe_code,
    clippy::all,
    clippy::pedantic
)]
pub mod ui {
    slint::include_modules!();
}

/// This build's ID (apps/build_id.rs): the helper must carry the same (design m2 E.3).
pub const BUILD_ID: &str = env!("MKLM_BUILD_ID");

/// Oldest Windows build MKLM supports (Windows 11 24H2, plan 4.1).
pub const MIN_BUILD: u32 = 26100;
