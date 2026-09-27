//! MKLM M0 #8: throwaway Slint 1.18 prototype that de-risks GUI decisions.
//!
//! Read-only with respect to the system: nothing is written to the registry or devices.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod devices;
mod icon;
mod theme;
mod watcher;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::winit_030::winit::event::{
    DeviceEvent, DeviceId, ElementState, RawKeyEvent, WindowEvent,
};
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};
use slint::winit_030::winit::platform::windows::DeviceIdExtWindows;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::{Theme, Window as WinitWindow, WindowId};
use slint::winit_030::{CustomApplicationHandler, EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use windows::UI::ViewManagement::UISettings;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetLocalTime;

use devices::KeyboardInfo;
use theme::{OsTheme, OsThemeWatcher, ThemeMode};
use watcher::{ShellEvent, ShellWatcher};

/// Code generated from `ui/app.slint`.
#[allow(missing_debug_implementations)]
mod generated {
    slint::include_modules!();
}
use generated::{AppWindow, KeyboardRow, TrayIcon};

const LOG_LINES: usize = 16;

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Runs `f` with the UI state if it exists (UI thread only). The borrow is released before `f` runs.
fn with_ui(f: impl FnOnce(&Ui)) {
    let ui = UI.with(|u| u.borrow().clone());
    if let Some(ui) = ui {
        f(&ui);
    }
}

/// Marshals `f` to the UI thread (callable from any thread).
fn on_ui_thread(f: impl FnOnce(&Ui) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_ui(f));
}

fn timestamp() -> String {
    // SAFETY: GetLocalTime has no preconditions.
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

fn hwnd_of(window: &WinitWindow) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut core::ffi::c_void)),
        _ => None,
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}時間{}分{}秒", s / 3600, s / 60 % 60, s % 60)
}

/// Human-readable form of a Slint key text (named keys use private-use code points).
fn describe_key_text(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return Some(text.to_owned());
    }
    match c {
        // Modifiers alone (Shift, Control, Alt, AltGr, CapsLock, ShiftR, ControlR, Meta, MetaR).
        '\u{10}'..='\u{18}' => None,
        ' ' => Some("Space".to_owned()),
        c if c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c) => {
            Some(format!("特殊キー U+{:04X}", c as u32))
        }
        c => Some(c.to_string()),
    }
}

/// UI-thread state of the prototype.
struct Ui {
    app: AppWindow,
    icon: Image,
    tray: RefCell<Option<TrayIcon>>,
    mode: Cell<ThemeMode>,
    os_theme: Cell<Option<OsTheme>>,
    /// Resolved theme, shared with the winit window attributes hook.
    resolved_dark: Rc<Cell<bool>>,
    os_watcher: RefCell<Option<OsThemeWatcher>>,
    shell_watcher: RefCell<Option<ShellWatcher>>,
    log: RefCell<VecDeque<String>>,
    keyboards: Rc<VecModel<KeyboardRow>>,
    last_device: RefCell<Option<KeyboardInfo>>,
    last_physical: Cell<Option<KeyCode>>,
    hidden_since: Cell<Option<Instant>>,
}

impl Ui {
    fn log(&self, line: impl AsRef<str>) {
        let mut log = self.log.borrow_mut();
        log.push_front(format!("{} {}", timestamp(), line.as_ref()));
        log.truncate(LOG_LINES);
        let joined = log
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        drop(log);
        self.app.set_event_log(joined.into());
    }

    // --- Theme ---

    fn resolved(&self) -> bool {
        self.mode
            .get()
            .resolve(self.os_theme.get().is_some_and(|t| t.dark))
    }

    /// Resolves the theme and applies it to the palette and the title bar.
    fn apply_theme(&self, reason: &str) {
        let dark = self.resolved();
        self.resolved_dark.set(dark);
        self.app.invoke_apply_color_scheme(dark);
        self.app.set_resolved_theme_text(
            format!(
                "{}（選択: {}）",
                if dark { "ダーク" } else { "ライト" },
                self.mode.get().label()
            )
            .into(),
        );
        let title_bar = self.set_title_bar(dark);
        self.log(format!(
            "テーマ適用 {} ← {reason}; {title_bar}",
            if dark { "dark" } else { "light" }
        ));
    }

    /// Syncs winit's per-window theme and sets DWMWA_USE_IMMERSIVE_DARK_MODE.
    fn set_title_bar(&self, dark: bool) -> String {
        self.app
            .window()
            .with_winit_window(|w| {
                // set_theme keeps SetWindowTheme/WCA_USEDARKMODECOLORS consistent; it does not persist
                // across WM_SETTINGCHANGE unless the window was created with with_theme(Some(..)).
                w.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
                match hwnd_of(w).map(|hwnd| theme::set_title_bar_dark(hwnd, dark)) {
                    Some(Ok(())) => "DWM OK".to_owned(),
                    Some(Err(e)) => format!("DWM 失敗 {e}"),
                    None => "HWND なし".to_owned(),
                }
            })
            .unwrap_or_else(|| "winit ウィンドウ未作成".to_owned())
    }

    fn on_os_theme(&self, os: OsTheme, source: &str) {
        let changed = self.os_theme.get().is_none_or(|old| old.dark != os.dark);
        self.os_theme.set(Some(os));
        self.app.set_os_theme_text(os.describe().into());
        if changed && self.mode.get() == ThemeMode::System {
            self.apply_theme(source);
        } else {
            self.log(format!(
                "OS テーマ通知（{source}）: 変化なし、または固定モード"
            ));
        }
    }

    fn on_mode_selected(&self, index: i32) {
        self.mode.set(ThemeMode::from_index(index));
        self.apply_theme("テーマの選択");
    }

    // --- Shell broadcasts ---

    fn on_shell_event(&self, event: ShellEvent) {
        match event {
            ShellEvent::TaskbarCreated => {
                if self.app.get_tray_workaround() {
                    self.tray.borrow_mut().take();
                    let result = self.create_tray();
                    self.log(match &result {
                        Ok(_) => "TaskbarCreated 受信 → トレイ アイコンを作り直しました".to_owned(),
                        Err(e) => format!("TaskbarCreated 受信 → 作り直しに失敗: {e}"),
                    });
                    *self.tray.borrow_mut() = result.ok();
                } else {
                    self.log("TaskbarCreated 受信（回避策オフ: Slint の標準動作のまま）");
                }
            }
            ShellEvent::SettingChange(area) => {
                let area = if area.is_empty() {
                    "(null)".to_owned()
                } else {
                    area
                };
                self.log(format!("WM_SETTINGCHANGE \"{area}\""));
                if area == "ImmersiveColorSet" {
                    let os = self.os_watcher.borrow().as_ref().map(OsThemeWatcher::read);
                    if let Some(Ok(os)) = os {
                        self.on_os_theme(os, "WM_SETTINGCHANGE(ImmersiveColorSet)");
                    }
                }
                if self.app.get_reapply_on_settingchange() {
                    let r = self.set_title_bar(self.resolved());
                    self.log(format!("  DWM 再適用: {r}"));
                }
            }
        }
    }

    // --- Tray and visibility ---

    fn create_tray(&self) -> Result<TrayIcon, slint::PlatformError> {
        let tray = TrayIcon::new()?;
        tray.set_tray_image(self.icon.clone());
        tray.on_icon_clicked(|| with_ui(|ui| ui.show_window("トレイ アイコンの左クリック")));
        tray.on_show_window(|| with_ui(|ui| ui.show_window("トレイ メニュー「表示」")));
        tray.on_quit_app(|| {
            let _ = slint::quit_event_loop();
        });
        tray.show()?;
        Ok(tray)
    }

    fn note_hidden(&self, how: &str) {
        self.hidden_since.set(Some(Instant::now()));
        self.log(format!("ウィンドウを非表示（{how}）"));
    }

    fn hide_to_tray(&self) {
        let _ = self.app.hide();
        self.note_hidden("ボタン");
    }

    fn show_window(&self, how: &str) {
        let _ = self.app.show();
        self.app.window().with_winit_window(|w| {
            w.set_minimized(false);
            w.focus_window();
        });
        match self.hidden_since.take() {
            Some(since) => self.log(format!(
                "ウィンドウを表示（{how}、非表示だった時間 {}）",
                format_duration(since.elapsed())
            )),
            None => self.log(format!("ウィンドウを表示（{how}）")),
        }
    }

    // --- Key test and device identification ---

    fn on_device_key(&self, info: &KeyboardInfo, physical: PhysicalKey) {
        let physical_text = match physical {
            PhysicalKey::Code(code) => {
                self.last_physical.set(Some(code));
                format!("{code:?}")
            }
            PhysicalKey::Unidentified(native) => {
                self.last_physical.set(None);
                format!("{native:?}")
            }
        };
        self.app.set_device_path(info.path.clone().into());
        self.app
            .set_device_instance_id(info.instance_id.clone().into());
        self.app.set_device_raw_info(info.raw_info.clone().into());
        self.app.set_device_physical_key(physical_text.into());

        let existing = (0..self.keyboards.row_count()).find(|&i| {
            self.keyboards
                .row_data(i)
                .is_some_and(|r| r.path.as_str() == info.path)
        });
        match existing {
            Some(i) => {
                if let Some(mut row) = self.keyboards.row_data(i) {
                    row.presses += 1;
                    self.keyboards.set_row_data(i, row);
                }
            }
            None => {
                self.keyboards.push(KeyboardRow {
                    path: info.path.clone().into(),
                    instance_id: info.label().into(),
                    raw_info: info.raw_info.clone().into(),
                    last_text: "—".into(),
                    presses: 1,
                });
                self.log(format!("新しいキーボードからの入力: {}", info.label()));
            }
        }
        *self.last_device.borrow_mut() = Some(info.clone());
    }

    fn on_key_test_pressed(&self, text: &str, shift: bool) {
        let Some(shown) = describe_key_text(text) else {
            return;
        };
        let verdict = if shift && self.last_physical.get() == Some(KeyCode::Digit2) {
            match text {
                "@" => "Shift+2 → @ : US 配列として動作しています",
                "\"" => "Shift+2 → \" : JIS 配列として動作しています",
                _ => "Shift+2 → 想定外の文字です",
            }
        } else {
            ""
        };
        self.app.set_key_text(shown.clone().into());
        self.app.set_key_verdict(verdict.into());

        let device = self.last_device.borrow().clone();
        match device {
            Some(info) => {
                self.app.set_key_device(info.label().into());
                if let Some(i) = (0..self.keyboards.row_count()).find(|&i| {
                    self.keyboards
                        .row_data(i)
                        .is_some_and(|r| r.path.as_str() == info.path)
                }) && let Some(mut row) = self.keyboards.row_data(i)
                {
                    row.last_text = shown.into();
                    self.keyboards.set_row_data(i, row);
                }
            }
            None => self
                .app
                .set_key_device("（DeviceEvent がまだ届いていません）".into()),
        }
    }

    fn on_key_test_released(&self, text: &str) {
        if let Some(shown) = describe_key_text(text) {
            self.app.set_key_release_text(shown.into());
        }
    }
}

/// Receives winit events before Slint does.
#[derive(Debug, Default)]
struct DeviceHandler {
    cache: HashMap<DeviceId, KeyboardInfo>,
}

impl DeviceHandler {
    fn info(&mut self, device_id: DeviceId) -> KeyboardInfo {
        self.cache
            .entry(device_id)
            .or_insert_with(|| match device_id.persistent_identifier() {
                Some(path) => KeyboardInfo::from_path(path),
                None => KeyboardInfo {
                    path: "（persistent_identifier() が None）".to_owned(),
                    ..Default::default()
                },
            })
            .clone()
    }
}

impl CustomApplicationHandler for DeviceHandler {
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
                let info = self.info(device_id);
                with_ui(|ui| ui.on_device_key(&info, physical_key));
            }
            // Raw Input handles can be reused after a device leaves.
            DeviceEvent::Added | DeviceEvent::Removed => {
                self.cache.remove(&device_id);
            }
            _ => {}
        }
        EventResult::Propagate
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _winit_window: Option<&WinitWindow>,
        _slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        if let WindowEvent::ThemeChanged(theme) = event {
            let theme = *theme;
            with_ui(|ui| ui.log(format!("winit ThemeChanged({theme:?}) を受信（想定外）")));
        }
        EventResult::Propagate
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--list-keyboards") {
        // Console check of the device lookups without the GUI (debug builds have a console).
        for kbd in devices::raw_keyboards() {
            println!("{}\n  {}\n  {}", kbd.path, kbd.instance_id, kbd.raw_info);
        }
        return Ok(());
    }

    let initial_mode = std::env::args()
        .find_map(|a| a.strip_prefix("--theme=").and_then(ThemeMode::parse))
        .unwrap_or(ThemeMode::System);

    // Resolve the OS theme before any window exists so the window is created with the right theme.
    let settings = UISettings::new()?;
    let os = OsTheme::read(&settings).ok();
    let resolved_dark = Rc::new(Cell::new(initial_mode.resolve(os.is_some_and(|t| t.dark))));

    // `--no-with-theme` reproduces the winit 0.30 issue: without a preferred theme at creation,
    // a later set_theme() is undone by the next WM_SETTINGCHANGE.
    let with_theme = !std::env::args().any(|a| a == "--no-with-theme");
    let hook_theme = resolved_dark.clone();
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(move |attrs| {
            if !with_theme {
                return attrs;
            }
            attrs.with_theme(Some(if hook_theme.get() {
                Theme::Dark
            } else {
                Theme::Light
            }))
        })
        .with_winit_custom_application_handler(DeviceHandler::default())
        .select()?;

    let app = AppWindow::new()?;
    let icon = icon::app_icon();
    app.set_app_icon(icon.clone());
    app.set_theme_mode(initial_mode.index());
    let keyboards = Rc::new(VecModel::default());
    app.set_keyboards(ModelRc::from(keyboards.clone()));
    app.set_env_info(
        format!(
            "slint 1.18 / winit 0.30 / SLINT_BACKEND={} / PID {}",
            std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "（未設定）".to_owned()),
            std::process::id()
        )
        .into(),
    );

    let ui = Rc::new(Ui {
        app: app.clone_strong(),
        icon,
        tray: RefCell::new(None),
        mode: Cell::new(initial_mode),
        os_theme: Cell::new(os),
        resolved_dark,
        os_watcher: RefCell::new(None),
        shell_watcher: RefCell::new(None),
        log: RefCell::new(VecDeque::new()),
        keyboards,
        last_device: RefCell::new(None),
        last_physical: Cell::new(None),
        hidden_since: Cell::new(None),
    });
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));

    if let Some(os) = os {
        app.set_os_theme_text(os.describe().into());
    }
    app.invoke_apply_color_scheme(ui.resolved());
    app.set_resolved_theme_text(
        format!(
            "{}（選択: {}）",
            if ui.resolved() {
                "ダーク"
            } else {
                "ライト"
            },
            initial_mode.label()
        )
        .into(),
    );

    match OsThemeWatcher::new(settings, |os| {
        on_ui_thread(move |ui| ui.on_os_theme(os, "ColorValuesChanged"))
    }) {
        Ok(w) => *ui.os_watcher.borrow_mut() = Some(w),
        Err(e) => ui.log(format!("ColorValuesChanged の購読に失敗: {e}")),
    }
    match ShellWatcher::new(|event| on_ui_thread(move |ui| ui.on_shell_event(event))) {
        Ok(w) => *ui.shell_watcher.borrow_mut() = Some(w),
        Err(e) => ui.log(format!("ブロードキャスト監視ウィンドウの作成に失敗: {e}")),
    }

    app.on_theme_mode_selected(|index| with_ui(|ui| ui.on_mode_selected(index)));
    app.on_reapply_title_bar(|| {
        with_ui(|ui| {
            let r = ui.set_title_bar(ui.resolved());
            ui.log(format!("タイトルバーを手動で再適用: {r}"));
        })
    });
    app.on_key_test_pressed(|text: SharedString, shift| {
        with_ui(|ui| ui.on_key_test_pressed(&text, shift))
    });
    app.on_key_test_released(|text: SharedString| with_ui(|ui| ui.on_key_test_released(&text)));
    app.on_hide_to_tray(|| with_ui(Ui::hide_to_tray));
    app.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });
    app.window().on_close_requested(|| {
        let mut has_tray = false;
        with_ui(|ui| {
            has_tray = ui.tray.borrow().is_some();
            ui.note_hidden("閉じるボタン");
        });
        if !has_tray {
            // Without a tray there would be no way back; quit instead.
            let _ = slint::quit_event_loop();
        }
        CloseRequestResponse::HideWindow
    });

    match ui.create_tray() {
        Ok(tray) => *ui.tray.borrow_mut() = Some(tray),
        Err(e) => ui.log(format!("トレイ アイコンの作成に失敗: {e}")),
    }

    // Apply the title bar once the winit window exists (it is created lazily by the event loop).
    let weak = app.as_weak();
    slint::spawn_local(async move {
        let Some(app) = weak.upgrade() else { return };
        let created = app.window().winit_window().await.is_ok();
        drop(app);
        with_ui(|ui| {
            if created {
                ui.apply_theme(&format!(
                    "起動（{}、{}）",
                    if with_theme {
                        "with_theme あり"
                    } else {
                        "with_theme なし"
                    },
                    initial_mode.label()
                ));
            }
        });
    })?;

    // Smoke test: `--exit-after=SECONDS` quits through the normal path and prints the event log.
    let exit_after = std::env::args().find_map(|a| {
        a.strip_prefix("--exit-after=")
            .and_then(|s| s.parse::<u64>().ok())
    });
    if let Some(secs) = exit_after {
        slint::Timer::single_shot(Duration::from_secs(secs), || {
            let _ = slint::quit_event_loop();
        });
    }

    app.show()?;
    ui.log("起動しました");
    slint::run_event_loop_until_quit()?;

    if exit_after.is_some() {
        println!("{}", ui.app.get_event_log());
    }

    // Tear down on the UI thread in a defined order (tray icon first so it leaves the notification area).
    ui.tray.borrow_mut().take();
    ui.os_watcher.borrow_mut().take();
    ui.shell_watcher.borrow_mut().take();
    UI.with(|u| u.borrow_mut().take());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_text_description() {
        assert_eq!(describe_key_text("@").as_deref(), Some("@"));
        assert_eq!(describe_key_text("\"").as_deref(), Some("\""));
        assert_eq!(describe_key_text("\u{10}"), None);
        assert_eq!(
            describe_key_text("\u{F704}").as_deref(),
            Some("特殊キー U+F704")
        );
        assert_eq!(describe_key_text(""), None);
    }

    #[test]
    fn duration_format() {
        assert_eq!(
            format_duration(Duration::from_secs(3 * 3600 + 62)),
            "3時間1分2秒"
        );
    }
}
