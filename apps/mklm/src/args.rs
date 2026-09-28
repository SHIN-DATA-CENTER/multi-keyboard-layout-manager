//! The command line (design m3 F.2). Every switch is optional; unknown arguments are reported in
//! the log and ignored (a wrong autostart value must not keep MKLM from starting).
//!
//! | Switch | Meaning |
//! |---|---|
//! | (none) | Start with the window shown (or activate the running instance). |
//! | `--tray` | Start in the notification area (the HKCU Run value). The window still opens when the first-run wizard is not done or the journal needs the user. |
//! | `--post-reboot` | Started by the RunOnce value after a restart: open in front; the journal decides what to show (design m2 C17). Wins over `--tray`. |
//! | `--after-update` | Started by the update runner or the after-update RunOnce value (design m5b D.10, E.5): open in front when there is an update result to show, else as `--tray`. Ranks like `--post-reboot`. |
//! | `--quit` | Ask the running instance to quit (the M5 installer) and exit. |
//! | `--update-endpoint=http://127.0.0.1:<port>` | Development builds with `--cfg mklm_update_dev` only: check a local rehearsal server instead of GitHub (design m5b A.10, E.8, F.6). Release builds do not know it (logged and ignored like any unknown argument). |
//! | `--theme=light\|dark\|system` | Override the theme for this run (development). |
//! | `--lang=ja\|en\|system` | Override the language for this run (development). |
//! | `--renderer=software\|femtovg` | Override the renderer (development; design m3 C.3). |
//! | `--exit-after=SECONDS` | Quit through the normal path after that many seconds (smoke tests). |

use std::time::Duration;

use crate::i18n::LangChoice;
use crate::theme::ThemeMode;

/// How the process starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartMode {
    #[default]
    Window,
    Tray,
    PostReboot,
    /// `--after-update` (design m5b E.5).
    AfterUpdate,
    Quit,
}

/// The renderer (design m3 C.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Renderer {
    /// Slint's software renderer: about 28 MB instead of 127 MB (M0 #8), no GL context to lose
    /// after sleep. The default.
    #[default]
    Software,
    Femtovg,
}

impl Renderer {
    /// The name `slint::BackendSelector::renderer_name` takes.
    pub fn slint_name(self) -> &'static str {
        match self {
            Renderer::Software => "software",
            Renderer::Femtovg => "femtovg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    pub start: StartMode,
    pub theme: Option<ThemeMode>,
    pub lang: Option<LangChoice>,
    pub renderer: Renderer,
    pub exit_after: Option<Duration>,
    /// `--update-endpoint` (development builds only, design m5b A.10).
    #[cfg(all(debug_assertions, mklm_update_dev))]
    pub update_endpoint: Option<String>,
    /// Arguments that were not understood (logged, ignored).
    pub unknown: Vec<String>,
}

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Args {
    let mut parsed = Args::default();
    for arg in args {
        match arg.as_str() {
            "--tray" if parsed.start == StartMode::Window => parsed.start = StartMode::Tray,
            "--tray" => {}
            // `--post-reboot` and `--after-update` rank alike: over `--tray`, under `--quit`; the
            // first of the two wins.
            "--post-reboot" | "--after-update"
                if matches!(parsed.start, StartMode::Window | StartMode::Tray) =>
            {
                parsed.start = if arg == "--post-reboot" {
                    StartMode::PostReboot
                } else {
                    StartMode::AfterUpdate
                };
            }
            "--post-reboot" | "--after-update" => {}
            "--quit" => parsed.start = StartMode::Quit,
            _ => {
                if !parse_value(&mut parsed, &arg) {
                    parsed.unknown.push(arg);
                }
            }
        }
    }
    parsed
}

/// `--name=value` switches; false when `arg` is none of them or its value is not understood.
fn parse_value(parsed: &mut Args, arg: &str) -> bool {
    let Some((name, value)) = arg.split_once('=') else {
        return false;
    };
    match name {
        "--theme" => ThemeMode::parse(value).map(|theme| parsed.theme = Some(theme)),
        "--lang" => LangChoice::parse(value).map(|lang| parsed.lang = Some(lang)),
        "--renderer" => match value {
            "software" => Some(Renderer::Software),
            "femtovg" => Some(Renderer::Femtovg),
            _ => None,
        }
        .map(|renderer| parsed.renderer = renderer),
        "--exit-after" => value
            .parse::<u64>()
            .ok()
            .map(|seconds| parsed.exit_after = Some(Duration::from_secs(seconds))),
        #[cfg(all(debug_assertions, mklm_update_dev))]
        "--update-endpoint" if value.starts_with("http://127.0.0.1:") => {
            parsed.update_endpoint = Some(value.to_string());
            Some(())
        }
        _ => None,
    }
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        parse(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn start_modes() {
        assert_eq!(args(&[]).start, StartMode::Window);
        assert_eq!(args(&["--tray"]).start, StartMode::Tray);
        assert_eq!(
            args(&["--tray", "--post-reboot"]).start,
            StartMode::PostReboot
        );
        assert_eq!(
            args(&["--post-reboot", "--tray"]).start,
            StartMode::PostReboot
        );
        assert_eq!(args(&["--quit", "--post-reboot"]).start, StartMode::Quit);
    }

    /// `--after-update` ranks like `--post-reboot` (design m5b H.5).
    #[test]
    fn after_update() {
        assert_eq!(args(&["--after-update"]).start, StartMode::AfterUpdate);
        assert_eq!(
            args(&["--tray", "--after-update"]).start,
            StartMode::AfterUpdate
        );
        assert_eq!(
            args(&["--after-update", "--tray"]).start,
            StartMode::AfterUpdate
        );
        assert_eq!(
            args(&["--after-update", "--post-reboot"]).start,
            StartMode::AfterUpdate
        );
        assert_eq!(
            args(&["--post-reboot", "--after-update"]).start,
            StartMode::PostReboot
        );
        assert_eq!(args(&["--after-update", "--quit"]).start, StartMode::Quit);
        assert_eq!(args(&["--quit", "--after-update"]).start, StartMode::Quit);
        // A release build does not know the development endpoint: logged and ignored.
        #[cfg(not(all(debug_assertions, mklm_update_dev)))]
        assert_eq!(
            args(&["--update-endpoint=http://127.0.0.1:8080"]).unknown,
            vec!["--update-endpoint=http://127.0.0.1:8080"]
        );
    }

    #[test]
    fn values_and_unknown_arguments() {
        let parsed = args(&[
            "--theme=dark",
            "--lang=en",
            "--renderer=femtovg",
            "--exit-after=5",
            "--theme=blue",
            "/S",
        ]);
        assert_eq!(parsed.theme, Some(ThemeMode::Dark));
        assert_eq!(parsed.lang, Some(LangChoice::En));
        assert_eq!(parsed.renderer, Renderer::Femtovg);
        assert_eq!(parsed.exit_after, Some(Duration::from_secs(5)));
        assert_eq!(parsed.unknown, vec!["--theme=blue", "/S"]);
        assert_eq!(args(&[]).renderer.slint_name(), "software");
    }
}
