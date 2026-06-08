#![windows_subsystem = "windows"]
//! LTBox GUI — iced desktop shell for the v3.0.0 Rust rewrite.
//!
//! Orchestrates `ltbox-core`, `ltbox-device`, `ltbox-patch` through a
//! sidebar + wizard UX. [`main`] handles startup (single-instance lock,
//! AppUserModelID, window + font bundle); [`App`] owns every wizard
//! state machine, the device poll subscription, persisted settings,
//! and the active palette.
//!
//! Wizards: Flash · SystemUpdate · Root · Unroot · Reboot · Advanced.
//! Sub-modules: [`theme`] M3 tokens · [`settings_store`] `settings.json`
//! in the user config dir · [`stdout_tap`] native-crate log capture.

#[rustfmt::skip]
#[allow(dead_code)]
#[path = "icon.rs"]
mod icon;
mod loader;
mod message;
mod model;
mod pickers;
mod settings_store;
mod stdout_tap;
mod theme;
mod theme_detect;
mod update;
mod view;
mod workers;

// Extracted items live in their own modules; re-export so the rest of the
// crate keeps referring to them unqualified.
pub(crate) use loader::*;
pub(crate) use message::*;
pub(crate) use model::device::*;
pub(crate) use model::wizard::*;
pub(crate) use view::components::*;
pub(crate) use view::styles::*;
pub(crate) use workers::advanced::*;
pub(crate) use workers::edl_transition::*;
pub(crate) use workers::flash::*;
pub(crate) use workers::reboot::*;
pub(crate) use workers::root::*;
pub(crate) use workers::sysupdate::*;
pub(crate) use workers::transfer::*;
pub(crate) use workers::unroot::*;

use std::collections::HashMap;

use ltbox_core::{live, tr_args};

use iced::widget::{self, Space, button, column, container, row, text};
use iced::{Element, Length, Subscription, Task, Theme};

use theme::{Palette, ThemeSeed, palette_for, with_alpha};

/// Palette lookup from `iced` style closures that only have `&Theme`.
fn pal_of(t: &Theme) -> Palette {
    theme::active_palette_for(t)
}

/// Upper bound on `App.log_lines` — keeps memory flat over long sessions.
const LOG_MAX_LINES: usize = 500;

/// 32×32 RGBA image handle for the title-bar brand icon. Built once,
/// cheap to clone (ref-counted).
static TITLE_BAR_ICON_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        let bytes: &'static [u8] = include_bytes!("../assets/icon_32.bin");
        iced::widget::image::Handle::from_rgba(32, 32, bytes.to_vec())
    });

/// Reverse-DNS app id. Becomes Wayland `app_id` / X11 `WM_CLASS` via
/// iced `Settings::id`; matches the shipped `.desktop`'s
/// `StartupWMClass=` so the window binds to the launcher entry.
const APP_ID: &str = "io.github.miner7222.LTBox";

/// Initial window dimensions on first run (logical pixels). Used both
/// by `main`'s `window::Settings::size` fallback and by `App::new` when
/// no persisted size exists yet — they must stay in lockstep.
const DEFAULT_WINDOW_WIDTH: f32 = 820.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 620.0;
/// Floor for cursor-drag resize and for the launch-time geometry
/// (`window::Settings::min_size`). Anything below this stops laying
/// out cleanly — wizard cards overlap, sidebar tween jumps.
const MIN_WINDOW_WIDTH: f32 = 820.0;
const MIN_WINDOW_HEIGHT: f32 = 620.0;
/// Minimum interval between window-size persistence writes. Cursor-drag
/// resize fires `Event::Window(Resized)` continuously; throttling to
/// ~250 ms keeps the JSON file from being rewritten 60 times per second
/// while still capturing the final geometry quickly after the drag ends.
const WINDOW_SIZE_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Upstream repo for the sidebar update pill.
const UPDATE_REPO: &str = "miner7222/LTBox";

/// Background probe for the sidebar update pill. Walks
/// `/releases?per_page=100`, returns the latest non-draft /
/// non-prerelease whose semver beats `CARGO_PKG_VERSION`. `None` on
/// network/parse failure or already-current — pill stays hidden.
///
/// Runs synchronously on a `spawn_blocking` worker so the async runtime
/// stays free; the result lands as `Message::UpdateCheckDone`.
fn check_for_update() -> Option<ltbox_core::github::StableRelease> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    let client = ltbox_core::github::GitHubClient::new(UPDATE_REPO).ok()?;
    let stable = client.latest_stable_release().ok().flatten()?;
    let stable_ver = semver::Version::parse(stable.tag.trim_start_matches('v')).ok()?;
    if stable_ver > current {
        Some(stable)
    } else {
        None
    }
}

/// Embedded udev rules — installable from binary without source tree.
#[cfg(target_os = "linux")]
const UDEV_RULES_CONTENT: &str = include_str!("../../../misc/udev/51-ltbox-qcom.rules");

#[cfg(target_os = "linux")]
const UDEV_RULES_PATH: &str = "/etc/udev/rules.d/51-ltbox-qcom.rules";

/// Embedded `.desktop` template — installable via `--install-desktop`.
#[cfg(target_os = "linux")]
const DESKTOP_FILE_TEMPLATE: &str =
    include_str!("../../../misc/desktop/io.github.miner7222.LTBox.desktop");

#[cfg(target_os = "linux")]
const APP_ICON_SVG: &str = include_str!("../assets/icon_source.svg");

/// `ltbox --install-udev` entry point. Writes bundled rules, reloads
/// udev, triggers it, exits. Linux-only — invoke via `sudo` or
/// `pkexec`. Windows / macOS print a one-line refusal.
#[cfg(target_os = "linux")]
fn install_udev_rules() -> ! {
    eprintln!("[ltbox] Installing udev rules → {UDEV_RULES_PATH}");
    if let Err(e) = std::fs::write(UDEV_RULES_PATH, UDEV_RULES_CONTENT) {
        eprintln!("[ltbox] write {UDEV_RULES_PATH}: {e}");
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "ltbox".into());
            eprintln!();
            eprintln!("[ltbox] Permission denied — needs root. Re-run as:");
            eprintln!("  sudo {exe} --install-udev");
            eprintln!("  pkexec {exe} --install-udev");
        }
        std::process::exit(1);
    }
    let reload_ok = std::process::Command::new("udevadm")
        .args(["control", "--reload"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !reload_ok {
        eprintln!(
            "[ltbox] WARNING: `udevadm control --reload` failed (rules still on disk; reboot will pick them up)"
        );
    }
    let trigger_ok = std::process::Command::new("udevadm")
        .arg("trigger")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !trigger_ok {
        eprintln!(
            "[ltbox] WARNING: `udevadm trigger` failed (replug device manually to apply rules)"
        );
    }
    eprintln!();
    eprintln!(
        "[ltbox] Done. Replug a connected Qualcomm 9008 / Lenovo USB device for the new ACL grants to take effect."
    );
    std::process::exit(0);
}

#[cfg(not(target_os = "linux"))]
fn install_udev_rules() -> ! {
    eprintln!("[ltbox] --install-udev is Linux-only — udev does not exist on this host.");
    std::process::exit(1);
}

/// `ltbox --install-desktop` entry point. Linux only. Per-user install
/// under `$XDG_DATA_HOME` (default `~/.local/share`); refreshes desktop
/// + icon caches. The `__LTBOX_EXEC__` placeholder in the bundled
/// `.desktop` is substituted with `current_exe()` at install time so a
/// tarball-extracted binary works without being on PATH.
#[cfg(target_os = "linux")]
fn install_desktop_file() -> ! {
    use std::fs;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        });
    let Some(data_home) = data_home else {
        eprintln!("[ltbox] $HOME and $XDG_DATA_HOME both unset; cannot resolve install dir.");
        std::process::exit(1);
    };

    let apps_dir = data_home.join("applications");
    let icons_dir = data_home.join("icons/hicolor/scalable/apps");
    let desktop_path = apps_dir.join(format!("{APP_ID}.desktop"));
    let icon_path = icons_dir.join(format!("{APP_ID}.svg"));

    if let Err(e) = fs::create_dir_all(&apps_dir) {
        eprintln!("[ltbox] mkdir {}: {e}", apps_dir.display());
        std::process::exit(1);
    }
    if let Err(e) = fs::create_dir_all(&icons_dir) {
        eprintln!("[ltbox] mkdir {}: {e}", icons_dir.display());
        std::process::exit(1);
    }

    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "ltbox".into());
    let desktop = DESKTOP_FILE_TEMPLATE.replace("__LTBOX_EXEC__", &exe);

    eprintln!("[ltbox] Writing desktop entry → {}", desktop_path.display());
    if let Err(e) = fs::write(&desktop_path, desktop) {
        eprintln!("[ltbox] write {}: {e}", desktop_path.display());
        std::process::exit(1);
    }

    eprintln!("[ltbox] Writing icon            → {}", icon_path.display());
    if let Err(e) = fs::write(&icon_path, APP_ICON_SVG) {
        eprintln!("[ltbox] write {}: {e}", icon_path.display());
        std::process::exit(1);
    }

    // Best-effort cache refresh. Both commands are no-ops on
    // sessions that don't have the corresponding cache file (e.g.
    // KDE without `gtk-update-icon-cache`). Failure is logged but
    // does not abort — the menu entry usually still shows up after
    // the next desktop session restart.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg(data_home.join("icons/hicolor"))
        .status();

    eprintln!();
    eprintln!(
        "[ltbox] Done. The entry should appear in your app menu within a few seconds. \
         Re-run with `--install-desktop` after moving the binary."
    );
    std::process::exit(0);
}

#[cfg(not(target_os = "linux"))]
fn install_desktop_file() -> ! {
    eprintln!(
        "[ltbox] --install-desktop is Linux-only — desktop entries follow the freedesktop.org spec."
    );
    std::process::exit(1);
}

fn main() -> iced::Result {
    // Linux/X11 renderer default. On some X11 + Mesa/driver combos wgpu
    // selects a Vulkan adapter whose X11 surface/device creation fails, so
    // the window never appears and `./ltbox` looks dead (issue #69). OpenGL
    // is robust there and more than enough for this UI, so default the wgpu
    // backend to GL on an X11 session when the user hasn't picked one. Wayland
    // keeps the wgpu default (Vulkan), which the Linux roadmap relies on for
    // recent Nvidia. Override anytime, e.g. `WGPU_BACKEND=vulkan ./ltbox`.
    #[cfg(target_os = "linux")]
    {
        // Treat a var as set only when it is non-empty — winit reads these
        // the same way (an empty value means "unset").
        let non_empty = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
        // Only the singular WGPU_BACKEND is read by this iced/wgpu stack, so
        // that alone counts as the user picking a backend. Checking the plural
        // WGPU_BACKENDS would let a value wgpu ignores silently suppress the
        // fallback below.
        let backend_chosen = non_empty("WGPU_BACKEND");
        // winit selects Wayland when WAYLAND_DISPLAY or WAYLAND_SOCKET is set,
        // otherwise X11 via DISPLAY — mirror that to scope the override to
        // pure-X11 sessions only.
        let wayland_session = non_empty("WAYLAND_DISPLAY") || non_empty("WAYLAND_SOCKET");
        let is_x11_session = !wayland_session && non_empty("DISPLAY");
        if !backend_chosen && is_x11_session {
            // SAFETY: first statement in `main`, before the stdout tap, the
            // tracing writer, tokio, or iced spawn any threads — so the
            // process is still single-threaded as `set_var` requires.
            unsafe {
                std::env::set_var("WGPU_BACKEND", "gl");
            }
        }
    }

    // Pre-iced CLI subcommands. Each handler exits the process so
    // the iced setup path runs only when no subcommand fires. Kept
    // tiny + dep-free (no `clap`) — there's exactly one flag and it
    // doesn't need argument parsing beyond presence detection.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--install-udev") {
        install_udev_rules();
    }
    if args.iter().any(|a| a == "--install-desktop") {
        install_desktop_file();
    }

    // Single-instance lock via fs2 advisory lock in the system temp
    // dir. Kernel drops the lock on dirty shutdown. Version-agnostic
    // filename so a running v3.0.0 blocks a v3.0.1 during in-place update.
    let _instance_guard: Option<std::fs::File> = {
        use fs2::FileExt;
        let lock_path = std::env::temp_dir().join("ltbox-gui-singleton.lock");
        match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => match f.try_lock_exclusive() {
                Ok(()) => Some(f),
                // Held by another LTBox — bail quietly.
                Err(_) => return Ok(()),
            },
            // Can't create lockfile (sandboxed FS) — launch without a guard.
            Err(_) => None,
        }
    };

    // Override AppUserModelID so taskbar / jump-list show "LTBox"
    // instead of the Cargo crate name. Must run before window creation.
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let id: Vec<u16> = "LTBox.App\0".encode_utf16().collect();
        unsafe {
            SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
        }
    }

    // Must run before any stdout write — the pipe has to be live
    // before the first `println!` resolves.
    stdout_tap::install();

    // `_log_guard` MUST live for the whole process — dropping it
    // flushes the non-blocking writer; losing it loses the last
    // minute of events on a crash.
    let _log_guard = init_tracing();

    let win_icon =
        iced::window::icon::from_rgba(include_bytes!("../assets/icon_32.bin").to_vec(), 32, 32)
            .ok();
    // Restore the user's previous window geometry if persisted (clamped
    // to ≥ `MIN_WINDOW_*` so corrupted / pre-min-size config files can
    // never launch a sub-floor window). Falls back to the default size
    // on first run.
    let persisted_size = settings_store::load()
        .window_size
        .map(|(w, h)| iced::Size::new(w.max(MIN_WINDOW_WIDTH), h.max(MIN_WINDOW_HEIGHT)))
        .unwrap_or_else(|| iced::Size::new(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT));
    let window_settings = iced::window::Settings {
        size: persisted_size,
        // Cursor-drag resize: `MIN_WINDOW_*` is the floor; anything
        // below is unsupported (sidebar + wizard cards stop laying out
        // cleanly). The borderless decorations strip native resize
        // edges off the window, so the GUI overlays 8 invisible resize
        // handles on the root Stack which emit
        // `WindowMsg::WindowResize(direction)` and call
        // `iced::window::drag_resize` on the host window. The user's
        // resized geometry is persisted to `PersistedSettings::window_size`
        // and restored above on the next launch.
        min_size: Some(iced::Size::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)),
        icon: win_icon,
        decorations: false,
        ..Default::default()
    };
    // Bundle Noto Sans CJK at compile time so cosmic-text can fall
    // back for Hangul / Hanzi glyphs. Noto's Latin + Cyrillic + Greek
    // cover English and Russian UI through the same family.
    let mut app = iced::application(App::new, App::update, App::view)
        .title("LTBox")
        // Application id propagates to winit:
        //   * Wayland → `app_id` on the xdg-shell toplevel
        //   * X11     → `WM_CLASS` (instance + class)
        // Matches `StartupWMClass=` in the shipped `.desktop` file
        // so GNOME / KDE / etc bind the running window to the
        // launcher entry. Without this they fall back to the binary
        // name (`ltbox`) which would only match if the desktop file
        // also said `StartupWMClass=ltbox` — using a reverse-DNS id
        // keeps it future-proof against a renamed binary.
        .settings(iced::Settings {
            id: Some(APP_ID.to_string()),
            default_font: iced::Font::with_name("Noto Sans CJK KR"),
            ..iced::Settings::default()
        })
        .theme(App::theme)
        .subscription(App::subscription)
        .window(window_settings);
    for (_, bytes) in noto_fonts_dl::load_fonts() {
        app = app.font(bytes.clone());
    }
    // Subset Lucide TTF generated at build time from
    // `fonts/lucide.toml`. Registered under the family `"lucide"` so
    // the text-based icon widgets from `mod icon` resolve against it.
    app = app.font(icon::FONT);
    app.run()
}

/// Global tracing subscriber writing daily-rotated files under
/// `%APPDATA%\ltbox\logs\`. Caller must hold the returned `WorkerGuard`
/// for the process lifetime — dropping it flushes queued entries.
/// Filter: `RUST_LOG` env var, falling back to `info`.
fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use camino::Utf8PathBuf;
    use tracing_subscriber::{EnvFilter, fmt};

    // Fall back to `%TEMP%\ltbox-logs` on non-UTF-8 APPDATA paths.
    let log_dir: Utf8PathBuf = dirs::config_dir()
        .and_then(|d| Utf8PathBuf::from_path_buf(d.join("ltbox").join("logs")).ok())
        .unwrap_or_else(|| {
            Utf8PathBuf::from_path_buf(std::env::temp_dir().join("ltbox-logs"))
                .unwrap_or_else(|_| Utf8PathBuf::from("ltbox-logs"))
        });
    if std::fs::create_dir_all(&log_dir).is_err() {
        return None;
    }

    let file_appender = tracing_appender::rolling::daily(log_dir.as_std_path(), "ltbox.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .finish();

    let _ = tracing::subscriber::set_global_default(subscriber);
    Some(guard)
}

// =========================================================================
// Navigation
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Dashboard,
    Flash,
    SystemUpdate,
    Root,
    Unroot,
    Reboot,
    Advanced,
    Settings,
}

impl View {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Dashboard => "nav_dashboard",
            Self::Flash => "nav_flash",
            Self::SystemUpdate => "nav_sysupdate",
            Self::Root => "nav_root",
            Self::Unroot => "nav_unroot",
            Self::Reboot => "nav_reboot",
            Self::Advanced => "nav_advanced",
            Self::Settings => "nav_settings",
        }
    }

    fn sidebar_label_key(&self) -> &'static str {
        match self {
            Self::Flash => "nav_flash_sidebar",
            _ => self.label_key(),
        }
    }

    fn nav_icon(&self) -> iced::widget::Text<'static, Theme, iced::Renderer> {
        match self {
            Self::Dashboard => icon::nav_dashboard(),
            Self::Flash => icon::nav_flash(),
            Self::SystemUpdate => icon::nav_system_update(),
            Self::Root => icon::nav_root(),
            Self::Unroot => icon::nav_unroot(),
            Self::Reboot => icon::nav_reboot(),
            Self::Advanced => icon::nav_advanced(),
            Self::Settings => icon::nav_settings(),
        }
    }
}

const NAV_MAIN: &[View] = &[
    View::Dashboard,
    View::Flash,
    View::SystemUpdate,
    View::Root,
    View::Unroot,
    View::Reboot,
];
const NAV_TOOLS: &[View] = &[View::Advanced, View::Settings];

/// One-shot reboot target for the Reboot panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RebootTarget {
    System,
    Recovery,
    Bootloader,
    Edl,
}
impl RebootTarget {
    fn label_key(&self) -> &'static str {
        match self {
            Self::System => "reboot_system",
            Self::Recovery => "reboot_recovery",
            Self::Bootloader => "reboot_bootloader",
            Self::Edl => "reboot_edl",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::System => "reboot_system_desc",
            Self::Recovery => "reboot_recovery_desc",
            Self::Bootloader => "reboot_bootloader_desc",
            Self::Edl => "reboot_edl_desc",
        }
    }
    /// Short-name key used inside the confirm popup so "Reboot to
    /// {Reboot to System}?" doesn't double-phrase.
    fn short_name_key(&self) -> &'static str {
        match self {
            Self::System => "reboot_target_system",
            Self::Recovery => "reboot_target_recovery",
            Self::Bootloader => "reboot_target_bootloader",
            Self::Edl => "reboot_target_edl",
        }
    }
    /// Reachable from `conn`. Impossible combos (Fastboot → Recovery,
    /// EDL → Recovery/Bootloader — Firehose only resets system/edl)
    /// stay disabled.
    fn available_from(&self, conn: ConnectionStatus) -> bool {
        match (conn, self) {
            (ConnectionStatus::None, _) => false,
            (ConnectionStatus::AdbUnauthorized, _) => false,
            (ConnectionStatus::AdbServerBlocking, _) => false,
            (ConnectionStatus::Adb, _) => true,
            (ConnectionStatus::AdbRecovery, _) => true,
            (ConnectionStatus::Fastboot, Self::Recovery) => false,
            (ConnectionStatus::Fastboot, _) => true,
            (ConnectionStatus::Edl, Self::System | Self::Edl) => true,
            (ConnectionStatus::Edl, _) => false,
        }
    }
    fn all() -> &'static [RebootTarget] {
        &[Self::System, Self::Recovery, Self::Bootloader, Self::Edl]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvAction {
    RegionConvert,
    ImageInfo,
    PatchDevinfo,
    DetectArb,
    PatchArb,
    ConvertXml,
    DumpPartitions,
    DumpPhysical,
    FlashPartitions,
    FlashPhysical,
    RebuildVbmeta,
}
impl AdvAction {
    fn label_key(&self) -> &'static str {
        match self {
            Self::RegionConvert => "adv_region_convert",
            Self::ImageInfo => "adv_image_info",
            Self::PatchDevinfo => "adv_patch_devinfo",
            Self::DetectArb => "adv_detect_arb",
            Self::PatchArb => "adv_patch_arb",
            Self::ConvertXml => "adv_convert_xml",
            Self::DumpPartitions => "adv_dump_partitions",
            Self::DumpPhysical => "adv_dump_physical",
            Self::FlashPartitions => "adv_flash_partitions",
            Self::FlashPhysical => "adv_flash_physical",
            Self::RebuildVbmeta => "adv_rebuild_vbmeta",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::RegionConvert => "adv_region_convert_desc",
            Self::ImageInfo => "adv_image_info_desc",
            Self::PatchDevinfo => "adv_patch_devinfo_desc",
            Self::DetectArb => "adv_detect_arb_desc",
            Self::PatchArb => "adv_patch_arb_desc",
            Self::ConvertXml => "adv_convert_xml_desc",
            Self::DumpPartitions => "adv_dump_partitions_desc",
            Self::DumpPhysical => "adv_dump_physical_desc",
            Self::FlashPartitions => "adv_flash_partitions_desc",
            Self::FlashPhysical => "adv_flash_physical_desc",
            Self::RebuildVbmeta => "adv_rebuild_vbmeta_desc",
        }
    }
    /// Browse-tile sub-description: *what* to pick, not the action's
    /// high-level description.
    fn source_desc_key(&self) -> &'static str {
        match self {
            Self::RegionConvert => "adv_src_region_convert",
            Self::ImageInfo => "adv_src_image_info",
            Self::PatchDevinfo => "adv_src_patch_devinfo",
            Self::DetectArb => "adv_src_detect_arb",
            Self::PatchArb => "adv_src_patch_arb_folder",
            Self::ConvertXml => "adv_src_convert_xml",
            Self::DumpPartitions => "adv_src_dump_partitions",
            Self::DumpPhysical => "adv_src_dump_physical",
            Self::FlashPartitions => "adv_src_flash_partitions",
            Self::FlashPhysical => "adv_src_flash_physical",
            Self::RebuildVbmeta => "adv_src_rebuild_vbmeta",
        }
    }
    /// snake_case slug for `{exe_dir}/output_{slug}/` — Advanced ops
    /// drop artefacts here instead of asking the user for a location.
    fn output_slug(&self) -> &'static str {
        match self {
            Self::RegionConvert => "region_convert",
            Self::ImageInfo => "image_info",
            Self::PatchDevinfo => "patch_devinfo",
            Self::DetectArb => "detect_arb",
            Self::PatchArb => "rb",
            Self::ConvertXml => "convert_xml",
            Self::DumpPartitions => "dump_partitions",
            Self::DumpPhysical => "dump_physical",
            Self::FlashPartitions => "flash_partitions",
            Self::FlashPhysical => "flash_physical",
            Self::RebuildVbmeta => "rebuild_vbmeta",
        }
    }
    /// True iff the action writes into the output folder — gates the
    /// "Open Folder" pill on the Done card.
    fn produces_output(&self) -> bool {
        matches!(
            self,
            Self::RegionConvert
                | Self::PatchDevinfo
                | Self::PatchArb
                | Self::ConvertXml
                | Self::RebuildVbmeta
        )
    }
}

/// Auto-output directory for an Advanced wizard action. Caller
/// `create_dir_all`s before writing. Routes through
/// [`ltbox_core::app_paths::auto_output_dir_for`] so AppImage /
/// distro-installed Linux copies don't try to write next to a
/// read-only or root-owned executable. Windows path stays
/// exe-adjacent (`<exe-dir>/output_<slug>`) for v3 continuity.
fn adv_output_dir(action: AdvAction) -> std::path::PathBuf {
    ltbox_core::app_paths::auto_output_dir_for(action.output_slug())
}

/// Launch the platform file manager on `path`.
///
/// Returns `Ok(())` only when a launcher actually accepted the spawn
/// — previously every error path was a `let _ = …` swallow, which on
/// Linux meant a missing `xdg-open` (or a desktop session without a
/// MIME handler for `inode/directory`) silently no-op'd. Caller is
/// expected to surface the returned error string in the GUI log /
/// error popup so users know why the "Open Folder" button did
/// nothing.
fn open_in_file_manager(path: &std::path::Path) -> std::result::Result<(), String> {
    #[cfg(windows)]
    {
        // `CREATE_NO_WINDOW` hides the transient cmd flash.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("explorer")
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("explorer {}: {e}", path.display()))
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open {}: {e}", path.display()))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Try xdg-open first (every desktop ships one); fall back to
        // GNOME's `gio open` which behaves correctly on
        // xdg-portal-only sessions where `xdg-open` itself errors out
        // mapping `inode/directory`. Capture the xdg error before
        // touching `gio` so the match below is exhaustive (compiler
        // can't see that the early return makes `xdg` provably Err
        // by this point).
        let xdg = std::process::Command::new("xdg-open").arg(path).spawn();
        if xdg.is_ok() {
            return Ok(());
        }
        let xdg_err = xdg.expect_err("checked Ok above");
        let gio = std::process::Command::new("gio")
            .arg("open")
            .arg(path)
            .spawn();
        match gio {
            Ok(_) => Ok(()),
            Err(gio_err) => Err(format!(
                "xdg-open {}: {xdg_err}; gio open {}: {gio_err}",
                path.display(),
                path.display(),
            )),
        }
    }
}
struct AdvSection {
    title_key: &'static str,
    items: &'static [AdvAction],
}

const ADV_SECTIONS: &[AdvSection] = &[
    AdvSection {
        title_key: "adv_section_region_patch",
        items: &[AdvAction::RegionConvert, AdvAction::PatchDevinfo],
    },
    AdvSection {
        title_key: "adv_section_rollback",
        items: &[
            AdvAction::ImageInfo,
            AdvAction::DetectArb,
            AdvAction::PatchArb,
            AdvAction::RebuildVbmeta,
        ],
    },
    AdvSection {
        title_key: "adv_section_edl_ops",
        items: &[
            AdvAction::ConvertXml,
            // Per-partition Read / Write paired together (read above
            // write so users can dump first, then re-flash if needed).
            AdvAction::DumpPartitions,
            AdvAction::FlashPartitions,
            // Whole-LUN dump / flash paired the same way.
            AdvAction::DumpPhysical,
            AdvAction::FlashPhysical,
        ],
    },
];

// =========================================================================
// Root wizard types
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Magisk,
    KernelSU,
    APatch,
}
impl Family {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Magisk => "family_magisk",
            Self::KernelSU => "family_ksu",
            Self::APatch => "family_apatch",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::Magisk => "family_magisk_desc",
            Self::KernelSU => "family_ksu_desc",
            Self::APatch => "family_apatch_desc",
        }
    }
    fn icon_disabled(self) -> Element<'static, Message> {
        let bytes: &'static [u8] = match self {
            Self::Magisk => include_bytes!("../assets/icons/magisk.svg"),
            Self::KernelSU => include_bytes!("../assets/icons/kernelsu.svg"),
            Self::APatch => include_bytes!("../assets/icons/apatch.svg"),
        };
        svg_icon_disabled(bytes, 72.0)
    }
    fn has_modes(&self) -> bool {
        matches!(self, Self::KernelSU)
    }
    fn providers(&self) -> &'static [Provider] {
        match self {
            Self::Magisk => &[Provider::Magisk, Provider::MagiskForks],
            Self::KernelSU => &[
                Provider::KernelSU,
                Provider::KernelSUNext,
                Provider::SukiSU,
                Provider::ReSukiSU,
            ],
            Self::APatch => &[Provider::APatch, Provider::FolkPatch],
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Magisk,
    MagiskForks,
    KernelSU,
    KernelSUNext,
    SukiSU,
    ReSukiSU,
    APatch,
    FolkPatch,
}
impl Provider {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Magisk => "provider_magisk",
            Self::MagiskForks => "provider_magisk_forks",
            Self::KernelSU => "provider_ksu",
            Self::KernelSUNext => "provider_ksu_next",
            Self::SukiSU => "provider_sukisu",
            Self::ReSukiSU => "provider_resukisu",
            Self::APatch => "provider_apatch",
            Self::FolkPatch => "provider_folkpatch",
        }
    }
    fn desc_key(&self) -> Option<&'static str> {
        match self {
            Self::Magisk => Some("provider_magisk_desc"),
            Self::MagiskForks => Some("provider_magisk_forks_desc"),
            Self::KernelSU => Some("provider_ksu_desc"),
            Self::KernelSUNext => Some("provider_ksu_next_desc"),
            Self::SukiSU => Some("provider_sukisu_desc"),
            Self::ReSukiSU => Some("provider_resukisu_desc"),
            Self::APatch => Some("provider_apatch_desc"),
            Self::FolkPatch => Some("provider_folkpatch_desc"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootMode {
    Lkm,
    Gki,
}
impl RootMode {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Lkm => "rootmode_lkm",
            Self::Gki => "rootmode_gki",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::Lkm => "rootmode_lkm_desc",
            Self::Gki => "rootmode_gki_desc",
        }
    }
    fn icon_disabled(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::Lkm => icon::root_lkm(),
            Self::Gki => icon::root_gki(),
        };
        lucide_disabled(glyph, 57.6)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerChoice {
    Stable,
    Nightly,
}
impl VerChoice {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Stable => "verchoice_stable",
            Self::Nightly => "verchoice_nightly",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::Stable => "verchoice_stable_desc",
            Self::Nightly => "verchoice_nightly_desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NightlySource {
    AutoDetect,
    ManualInput,
}
impl NightlySource {
    fn label_key(&self) -> &'static str {
        match self {
            Self::AutoDetect => "nightly_auto",
            Self::ManualInput => "nightly_manual",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::AutoDetect => "nightly_auto_desc",
            Self::ManualInput => "nightly_manual_desc",
        }
    }
}

// =========================================================================
// Settings state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    En,
    Ko,
    Zh,
    Ru,
    Ja,
}
impl Language {
    /// Name in its own script — locale-neutral.
    fn label(&self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ko => "한국어",
            Self::Zh => "中文",
            Self::Ru => "Русский",
            Self::Ja => "日本語",
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ko => "ko",
            Self::Zh => "zh",
            Self::Ru => "ru",
            Self::Ja => "ja",
        }
    }
    fn from_code(c: &str) -> Option<Self> {
        match c {
            "en" => Some(Self::En),
            "ko" => Some(Self::Ko),
            "zh" => Some(Self::Zh),
            "ru" => Some(Self::Ru),
            "ja" => Some(Self::Ja),
            _ => None,
        }
    }
}
const LANGUAGES: &[Language] = &[
    Language::En,
    Language::Ko,
    Language::Zh,
    Language::Ru,
    Language::Ja,
];

/// Theme preference. `System` reads the OS setting via
/// `theme_detect::system_prefers_dark`; Light/Dark override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}
impl ThemeChoice {
    fn label_key(&self) -> &'static str {
        match self {
            Self::System => "theme_system",
            Self::Light => "theme_light",
            Self::Dark => "theme_dark",
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    fn from_code(c: &str) -> Option<Self> {
        match c {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

// =========================================================================
// Operation progress steps
// =========================================================================

/// One phase of a long-running op. The GUI advances through
/// `Vec<OpStep>` by matching phase markers (`N/M`) in the log stream —
/// no separate event channel.
#[derive(Debug, Clone)]
struct OpStep {
    /// Pre-translated label for the single-step card. Derived at op
    /// start so language changes only re-run `derive_*_op_steps`.
    label: String,
}

/// Localized phase marker text that still includes a stable `N/M` token
/// for progress parsing.
pub(crate) fn phase_marker<S: AsRef<str>>(phase: usize, total: usize, label: S) -> String {
    tr_args!(
        "live_phase_marker",
        phase = phase.to_string(),
        total = total.to_string(),
        label = label.as_ref()
    )
}

/// Match a SKU token (e.g. `"TB323FU"`) inside an arbitrary string with
/// alphanumeric word boundaries so a future variant like `TB323FUX` does
/// not collide with the bare match. Used by the flash worker to gate
/// SKU-specific behaviour off either a vendor_boot fingerprint or the
/// probe-reported device model string.
pub(crate) fn fingerprint_token_match(haystack: &str, model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(model) {
        let abs = start + pos;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let end = abs + model.len();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Parse `N/M` out of a log line. Returns `N` (1-indexed).
/// Shape stays stable across locales as long as a `digit/digit` token
/// is present in the line — but rejects fractional pairs like
/// `12.3/45.6 MB` from downloader progress ticks. Without that gate,
/// every `5%` progress emit looked like a phase marker and yanked
/// `current_op_step` to whatever digit landed next to the slash,
/// making the wizard race through every phase mid-download and snap
/// back when the next real `Phase N/M` line arrived.
fn parse_phase_marker(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for slash in 0..bytes.len() {
        if bytes[slash] != b'/' {
            continue;
        }
        let mut lhs = slash;
        while lhs > 0 && bytes[lhs - 1].is_ascii_digit() {
            lhs -= 1;
        }
        if lhs == slash {
            continue;
        }
        let mut rhs = slash + 1;
        while rhs < bytes.len() && bytes[rhs].is_ascii_digit() {
            rhs += 1;
        }
        if rhs == slash + 1 {
            continue;
        }
        // Decimal-point guard: `1.2/3.4 MB` digits-adjacent-to-slash
        // are fragments of floats, not phase counters. Reject when
        // either side touches a `.` instead of a separator.
        if lhs > 0 && bytes[lhs - 1] == b'.' {
            continue;
        }
        if rhs < bytes.len() && bytes[rhs] == b'.' {
            continue;
        }
        return line[lhs..slash].parse::<usize>().ok();
    }
    None
}

// Icon glyphs for the current-step card (running / done / failed).
// Colour is applied at the call site so running/done/failed each paint
// with the palette role appropriate to the outcome (primary / success
// / error).

// =========================================================================
// Translations
// =========================================================================

const EN_JSON: &str = include_str!("../lang/en.json");
const KO_JSON: &str = include_str!("../lang/ko.json");
const ZH_JSON: &str = include_str!("../lang/zh.json");
const RU_JSON: &str = include_str!("../lang/ru.json");
const JA_JSON: &str = include_str!("../lang/ja.json");

// Parsed once on first access; `Translations::load` then swaps two
// `&'static` refs — no reparse on language switch.
static EN_TABLE: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(|| serde_json::from_str(EN_JSON).expect("en.json must parse"));
static KO_TABLE: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(|| serde_json::from_str(KO_JSON).expect("ko.json must parse"));
static ZH_TABLE: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(|| serde_json::from_str(ZH_JSON).expect("zh.json must parse"));
static RU_TABLE: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(|| serde_json::from_str(RU_JSON).expect("ru.json must parse"));
static JA_TABLE: std::sync::LazyLock<HashMap<String, String>> =
    std::sync::LazyLock::new(|| serde_json::from_str(JA_JSON).expect("ja.json must parse"));

/// Active translation table + English fallback. Two `&'static` refs
/// into the process-wide `LazyLock` tables, so reload is free.
#[derive(Debug, Clone, Copy)]
struct Translations {
    primary: &'static HashMap<String, String>,
    fallback: &'static HashMap<String, String>,
}

impl Translations {
    fn load(lang: Language) -> Self {
        let fallback: &'static HashMap<String, String> = &EN_TABLE;
        let primary: &'static HashMap<String, String> = match lang {
            Language::En => &EN_TABLE,
            Language::Ko => &KO_TABLE,
            Language::Zh => &ZH_TABLE,
            Language::Ru => &RU_TABLE,
            Language::Ja => &JA_TABLE,
        };
        Self { primary, fallback }
    }

    fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.primary
            .get(key)
            .or_else(|| self.fallback.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }
}

impl Default for Translations {
    fn default() -> Self {
        Self::load(Language::En)
    }
}

/// Wire the language tables into `ltbox_core::i18n` so backend crates
/// still produce localized log output.
fn install_core_translator(lang: Language) {
    let tr = Translations::load(lang);
    ltbox_core::i18n::set_translator(move |key| tr.t(key).to_string());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RollbackSetting {
    On,
    Auto,
    #[default]
    Off,
}
impl RollbackSetting {
    fn label_key(&self) -> &'static str {
        match self {
            Self::On => "rollback_on",
            Self::Auto => "rollback_auto",
            Self::Off => "rollback_off",
        }
    }
    /// Map the wizard tri-state to `rollback::RollbackMode`.
    fn to_mode(self) -> ltbox_patch::rollback::RollbackMode {
        match self {
            Self::On => ltbox_patch::rollback::RollbackMode::On,
            Self::Auto => ltbox_patch::rollback::RollbackMode::Auto,
            Self::Off => ltbox_patch::rollback::RollbackMode::Off,
        }
    }
}

#[derive(Debug, Clone)]
struct SettingsState {
    language: Language,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            language: Language::En,
        }
    }
}

/// Country-code state for the Flash wizard's wipe path. Sum type so
/// the three valid states (popup not yet reached / explicitly
/// skipped / target picked) stay un-collapsible — the previous
/// `Option<String>` + `bool` pair encoded the same with two fields
/// and a doc-comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum CountryAction {
    /// Popup hasn't been answered yet.
    #[default]
    Unset,
    /// User picked "Do not change" — devinfo/persist stays put.
    Skip,
    /// User picked a concrete target code; exec runs the patch.
    Set(String),
}

impl CountryAction {
    pub(crate) fn target(&self) -> Option<&str> {
        match self {
            Self::Set(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub(crate) fn is_skipped(&self) -> bool {
        matches!(self, Self::Skip)
    }
}

/// Derived from wizard selections; reset after the op finishes.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkflowConfig {
    pub(crate) modify_region: bool,
    pub(crate) device_region: Option<DeviceRegion>,
    pub(crate) modify_rollback: RollbackSetting,
    pub(crate) wipe: bool,
    pub(crate) country_action: CountryAction,
}

struct CountryEntry {
    code: &'static str,
    name: &'static str,
}

const COUNTRY_CODES: &[CountryEntry] = &[
    CountryEntry {
        code: "AE",
        name: "United Arab Emirates",
    },
    CountryEntry {
        code: "AM",
        name: "Armenia",
    },
    CountryEntry {
        code: "AR",
        name: "Argentina",
    },
    CountryEntry {
        code: "AT",
        name: "Austria",
    },
    CountryEntry {
        code: "AU",
        name: "Australia",
    },
    CountryEntry {
        code: "AZ",
        name: "Azerbaijan",
    },
    CountryEntry {
        code: "BE",
        name: "Belgium",
    },
    CountryEntry {
        code: "BG",
        name: "Bulgaria",
    },
    CountryEntry {
        code: "BH",
        name: "Bahrain",
    },
    CountryEntry {
        code: "BR",
        name: "Brazil",
    },
    CountryEntry {
        code: "CA",
        name: "Canada",
    },
    CountryEntry {
        code: "CH",
        name: "Switzerland",
    },
    CountryEntry {
        code: "CL",
        name: "Chile",
    },
    CountryEntry {
        code: "CN",
        name: "China",
    },
    CountryEntry {
        code: "CO",
        name: "Colombia",
    },
    CountryEntry {
        code: "CR",
        name: "Costa Rica",
    },
    CountryEntry {
        code: "CY",
        name: "Cyprus",
    },
    CountryEntry {
        code: "CZ",
        name: "Czech Republic",
    },
    CountryEntry {
        code: "DE",
        name: "Germany",
    },
    CountryEntry {
        code: "DK",
        name: "Denmark",
    },
    CountryEntry {
        code: "EC",
        name: "Ecuador",
    },
    CountryEntry {
        code: "EE",
        name: "Estonia",
    },
    CountryEntry {
        code: "EG",
        name: "Egypt",
    },
    CountryEntry {
        code: "ES",
        name: "Spain",
    },
    CountryEntry {
        code: "FI",
        name: "Finland",
    },
    CountryEntry {
        code: "FR",
        name: "France",
    },
    CountryEntry {
        code: "GB",
        name: "United Kingdom",
    },
    CountryEntry {
        code: "GE",
        name: "Georgia",
    },
    CountryEntry {
        code: "GH",
        name: "Ghana",
    },
    CountryEntry {
        code: "GR",
        name: "Greece",
    },
    CountryEntry {
        code: "GT",
        name: "Guatemala",
    },
    CountryEntry {
        code: "HK",
        name: "Hong Kong",
    },
    CountryEntry {
        code: "HR",
        name: "Croatia",
    },
    CountryEntry {
        code: "HU",
        name: "Hungary",
    },
    CountryEntry {
        code: "ID",
        name: "Indonesia",
    },
    CountryEntry {
        code: "IL",
        name: "Israel",
    },
    CountryEntry {
        code: "IN",
        name: "India",
    },
    CountryEntry {
        code: "IS",
        name: "Iceland",
    },
    CountryEntry {
        code: "IT",
        name: "Italy",
    },
    CountryEntry {
        code: "JO",
        name: "Jordan",
    },
    CountryEntry {
        code: "JP",
        name: "Japan",
    },
    CountryEntry {
        code: "KE",
        name: "Kenya",
    },
    CountryEntry {
        code: "KG",
        name: "Kyrgyzstan",
    },
    CountryEntry {
        code: "KR",
        name: "Korea",
    },
    CountryEntry {
        code: "KW",
        name: "Kuwait",
    },
    CountryEntry {
        code: "KZ",
        name: "Kazakhstan",
    },
    CountryEntry {
        code: "LB",
        name: "Lebanon",
    },
    CountryEntry {
        code: "LT",
        name: "Lithuania",
    },
    CountryEntry {
        code: "LV",
        name: "Latvia",
    },
    CountryEntry {
        code: "MA",
        name: "Morocco",
    },
    CountryEntry {
        code: "MD",
        name: "Moldova",
    },
    CountryEntry {
        code: "MX",
        name: "Mexico",
    },
    CountryEntry {
        code: "MY",
        name: "Malaysia",
    },
    CountryEntry {
        code: "MZ",
        name: "Mozambique",
    },
    CountryEntry {
        code: "NG",
        name: "Nigeria",
    },
    CountryEntry {
        code: "NL",
        name: "Netherlands",
    },
    CountryEntry {
        code: "NO",
        name: "Norway",
    },
    CountryEntry {
        code: "NZ",
        name: "New Zealand",
    },
    CountryEntry {
        code: "OM",
        name: "Oman",
    },
    CountryEntry {
        code: "PA",
        name: "Panama",
    },
    CountryEntry {
        code: "PE",
        name: "Peru",
    },
    CountryEntry {
        code: "PH",
        name: "Philippines",
    },
    CountryEntry {
        code: "PK",
        name: "Pakistan",
    },
    CountryEntry {
        code: "PL",
        name: "Poland",
    },
    CountryEntry {
        code: "PT",
        name: "Portugal",
    },
    CountryEntry {
        code: "QA",
        name: "Qatar",
    },
    CountryEntry {
        code: "RO",
        name: "Romania",
    },
    CountryEntry {
        code: "RS",
        name: "Serbia",
    },
    CountryEntry {
        code: "RU",
        name: "Russia",
    },
    CountryEntry {
        code: "SA",
        name: "Saudi Arabia",
    },
    CountryEntry {
        code: "SE",
        name: "Sweden",
    },
    CountryEntry {
        code: "SG",
        name: "Singapore",
    },
    CountryEntry {
        code: "SI",
        name: "Slovenia",
    },
    CountryEntry {
        code: "SK",
        name: "Slovakia",
    },
    CountryEntry {
        code: "SV",
        name: "El Salvador",
    },
    CountryEntry {
        code: "TH",
        name: "Thailand",
    },
    CountryEntry {
        code: "TJ",
        name: "Tajikistan",
    },
    CountryEntry {
        code: "TN",
        name: "Tunisia",
    },
    CountryEntry {
        code: "TR",
        name: "Turkey",
    },
    CountryEntry {
        code: "TW",
        name: "Taiwan",
    },
    CountryEntry {
        code: "TZ",
        name: "Tanzania",
    },
    CountryEntry {
        code: "UA",
        name: "Ukraine",
    },
    CountryEntry {
        code: "UG",
        name: "Uganda",
    },
    CountryEntry {
        code: "US",
        name: "United States",
    },
    CountryEntry {
        code: "UY",
        name: "Uruguay",
    },
    CountryEntry {
        code: "UZ",
        name: "Uzbekistan",
    },
    CountryEntry {
        code: "VE",
        name: "Venezuela",
    },
    CountryEntry {
        code: "VN",
        name: "Vietnam",
    },
    CountryEntry {
        code: "ZA",
        name: "South Africa",
    },
];

/// Sortable header cell for the FlashParts / DumpParts partition table.
/// Renders `label` followed by either ▲/▼ (active sort, direction
/// reflects `desc`) or ⇅ (sortable but inactive). Click fires `msg`.
/// Transparent button so the cell reads as text first.
fn parts_sort_header(
    label: String,
    is_active: bool,
    desc: bool,
    width: Length,
    msg: Message,
) -> Element<'static, Message> {
    let arrow = if is_active {
        if desc { " ▼" } else { " ▲" }
    } else {
        " ⇅"
    };
    let lbl = format!("{label}{arrow}");
    button(text(lbl).size(11).style(muted_style))
        .padding(0)
        .width(width)
        .style(|_t: &Theme, _s| button::Style {
            background: None,
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// Format a unix timestamp (seconds) as `YYYY-MM-DD HH:MM:SS UTC`.
/// Pure stdlib — chrono is intentionally not pulled into the GUI just
/// for one popup label. Uses Howard Hinnant's civil-from-days
/// algorithm so the proleptic Gregorian conversion stays correct
/// across leap years and century boundaries without a calendar table.
fn format_unix_timestamp_utc(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let rem = (ts % 86_400) as u32;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
}

/// Howard Hinnant `civil_from_days`: (days since 1970-01-01) →
/// `(year, month, day)` in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Worker for the Advanced → Detect Anti-Rollback flow. Mirrors the
/// flash wizard's ARB probe but in a manual, report-only shape:
///
/// 1. Reach fastboot (reboot from ADB if needed).
/// 2. Read `stored_rollback_index:N` vars. Any entry whose value is
///    not 0 / 1 makes the device anti-rollback; the report lists each
///    surviving entry as `stored_rollback_index:N = TS (UTC)`.
/// 3. If no `stored_rollback_index` was reported AND the model is
///    `TB320FC`, fall back to dumping `boot_a` + `vbmeta_system_a`
///    over EDL using the user-picked Firehose loader and report
///    their AVB rollback indices the same way.
/// 4. Otherwise (no stored_rollback_index, not TB320FC) the device
///    is not anti-rollback.
/// 5. Always reboot to system at the end so the user can keep using
///    the device.
#[allow(clippy::too_many_arguments)]
fn detect_arb_run(
    conn: ConnectionStatus,
    device_model: String,
    loader_path: Option<String>,
    i_anti: &str,
    i_not: &str,
    i_reboot_fastboot: &str,
    i_reboot_system: &str,
    i_edl_dump: &str,
    log: &mut Vec<String>,
) -> std::result::Result<(), String> {
    use ltbox_device::adb::AdbManager;
    use ltbox_device::fastboot::FastbootDevice;

    // Step 1: ensure we are in fastboot.
    if !matches!(conn, ConnectionStatus::Fastboot) {
        match conn {
            ConnectionStatus::Adb | ConnectionStatus::AdbRecovery => {
                ltbox_core::live!(log, "[ARB] {i_reboot_fastboot}");
                let mut adb = AdbManager::new();
                if !adb.check_device().unwrap_or(false) {
                    return Err("ADB device not reachable".into());
                }
                let _ = adb.shell("reboot bootloader");
                std::thread::sleep(std::time::Duration::from_secs(5));
                if FastbootDevice::wait_for_device().is_err() {
                    return Err("Failed to enter fastboot".into());
                }
            }
            _ => {
                return Err(
                    "Device must be in ADB or fastboot to run anti-rollback detection".into(),
                );
            }
        }
    }

    // Step 2: read fastboot vars (rollback_indices map is the source
    // of truth — its emptiness drives the model-specific fallback).
    let vars = FastbootDevice::open()
        .and_then(|mut d| d.get_all_vars())
        .map_err(|e| format!("fastboot vars: {e}"))?;

    let stored_present = !vars.rollback_indices.is_empty();
    if stored_present {
        let mut filtered: Vec<(u32, u64)> = vars
            .rollback_indices
            .iter()
            .filter(|&(_, &v)| v != 0 && v != 1)
            .map(|(k, v)| (*k, *v))
            .collect();
        filtered.sort_by_key(|(k, _)| *k);
        ltbox_core::live!(log, "");
        ltbox_core::live!(log, "{i_anti}");
        ltbox_core::live!(log, "");
        for (idx, ts) in &filtered {
            let utc = format_unix_timestamp_utc(*ts);
            ltbox_core::live!(log, "stored_rollback_index:{idx} = {ts} ({utc})");
        }
        ltbox_core::live!(log, "");
        ltbox_core::live!(log, "[ARB] {i_reboot_system}");
        if let Ok(mut dev) = FastbootDevice::open() {
            let _ = dev.reboot();
        }
        return Ok(());
    }

    // Step 3: every model except the no-ARB TB322FC enforces rollback
    // protection but may not expose `stored_rollback_index` over fastboot —
    // read the ACTIVE-slot boot + vbmeta_system indices over EDL. (TB322FC
    // falls through to step 4 / "no anti-rollback".)
    if is_rollback_protected_model(&device_model) {
        let Some(loader) = loader_path else {
            return Err("An EDL loader is required for the deeper rollback inspection".into());
        };
        ltbox_core::live!(log, "[ARB] {i_edl_dump}");
        if ensure_edl(ConnectionStatus::Fastboot, "ARB", log).is_err() {
            return Err("Failed to enter EDL".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let loader_pb = std::path::PathBuf::from(&loader);
        let mut session = ltbox_device::edl::EdlSession::open(&loader_pb, true, log)
            .map_err(|e| format!("EDL open: {e}"))?;
        // Read the active slot (a first-time user may be on `_b`).
        let slot = active_slot_suffix(vars.current_slot.as_deref());
        let boot_part = format!("boot{slot}");
        let vbm_part = format!("vbmeta_system{slot}");
        let tmp = std::env::temp_dir();
        let boot_out = tmp.join(format!("ltbox_arb_{boot_part}.img"));
        let vbm_out = tmp.join(format!("ltbox_arb_{vbm_part}.img"));
        // boot → LUN 4, vbmeta_system → LUN 0 per the hardcoded LUN map.
        session
            .dump_partition(&boot_part, &boot_out, 0, 4, log)
            .map_err(|e| format!("dump {boot_part}: {e}"))?;
        session
            .dump_partition(&vbm_part, &vbm_out, 0, 0, log)
            .map_err(|e| format!("dump {vbm_part}: {e}"))?;
        let boot_idx = ltbox_patch::avb::extract_image_avb_info(&boot_out)
            .map_err(|e| format!("boot AVB: {e}"))?
            .rollback_index;
        let vbm_idx = ltbox_patch::avb::extract_image_avb_info(&vbm_out)
            .map_err(|e| format!("vbmeta_system AVB: {e}"))?
            .rollback_index;
        let _ = std::fs::remove_file(&boot_out);
        let _ = std::fs::remove_file(&vbm_out);
        ltbox_core::live!(log, "");
        ltbox_core::live!(log, "{i_anti}");
        ltbox_core::live!(log, "");
        ltbox_core::live!(
            log,
            "{boot_part} = {boot_idx} ({})",
            format_unix_timestamp_utc(boot_idx)
        );
        ltbox_core::live!(
            log,
            "{vbm_part} = {vbm_idx} ({})",
            format_unix_timestamp_utc(vbm_idx)
        );
        ltbox_core::live!(log, "");
        ltbox_core::live!(log, "[ARB] {i_reboot_system}");
        session.reset_tolerant(log);
        return Ok(());
    }

    // Step 4: no stored_rollback_index, no TB320FC override.
    ltbox_core::live!(log, "");
    ltbox_core::live!(log, "{i_not}");
    ltbox_core::live!(log, "");
    ltbox_core::live!(log, "[ARB] {i_reboot_system}");
    if let Ok(mut dev) = FastbootDevice::open() {
        let _ = dev.reboot();
    }
    Ok(())
}

/// Human-readable auto-unit byte formatter (B/KB/MB/GB).
fn format_bytes_auto(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

// =========================================================================
// Async poll results + popup UI state
// =========================================================================

#[derive(Debug, Clone, Default)]
struct DevicePollResult {
    status: ConnectionStatus,
    model: String,
    slot: String,
    /// Trimmed `ro.build.display.id` — leading device-model prefix
    /// stripped so the dashboard cell stays readable.
    firmware: String,
    /// Untrimmed `ro.build.display.id` exactly as the device reports
    /// it. Required by Lenovo's OTA `querynewfirmware` endpoint —
    /// passing the trimmed form returns an empty `<firmwareupdate/>`
    /// because the upstream key matches the full string.
    firmware_full: String,
    arb: String,
    ram: String,
    storage: String,
    market_name: String,
    /// Device serial captured from ADB or fastboot. Empty when no
    /// connected device produced a serial (EDL/Sahara never reports
    /// one). Used by the device-info popup to query the Lenovo PTSTPD
    /// API. Reset to empty whenever the device disconnects so a stale
    /// serial does not bleed across hardware swaps mid-session.
    serial: String,
    platform_supported: Option<bool>, // None = unknown, Some(true) = qcom, Some(false) = unsupported
}

/// Loading state for the device-info popup. The popup view branches on
/// this to render a spinner / table / error banner while keeping the
/// modal open so the user has a clear target to dismiss.
#[derive(Debug, Clone)]
enum DeviceInfoState {
    /// Fetch is in flight; render a spinner and disable retry.
    Loading,
    /// `device_info_cache[serial]` is populated; render the table.
    Ready,
    /// Fetch failed; render the message + a retry pill.
    Error(String),
}

/// Loading state for the firmware-OTA popup. Mirrors `DeviceInfoState`
/// but adds a `NoUpdate` arm — the upstream `<firmwareupdate/>` empty
/// payload means "no OTA staged for this firmware id" and renders as a
/// single placeholder line, not as an error banner.
#[derive(Debug, Clone)]
enum OtaPopupState {
    Loading,
    NoUpdate,
    Ready(ltbox_core::lenovo_ota::OtaUpdate),
    Error(String),
}

/// Parse hwboardid: `"SM8750P_16+512_13"` → `("16 GB", "512 GB")`.
fn parse_hwboardid_ram_storage(hwboardid: &str) -> (String, String) {
    let parts: Vec<&str> = hwboardid.split('_').collect();
    for part in &parts {
        if let Some((ram, storage)) = part.split_once('+')
            && ram.chars().all(|c| c.is_ascii_digit())
            && storage.chars().all(|c| c.is_ascii_digit())
        {
            return (format!("{ram} GB"), format!("{storage} GB"));
        }
    }
    (String::new(), String::new())
}

/// Pre-translated live-log strings for spawn_blocking closures that
/// can't carry `self` across thread boundaries.
#[derive(Debug, Clone)]
pub(crate) struct LiveLabels {
    pub(crate) op_root_phase: [String; 7],
    pub(crate) op_unroot_phase: [String; 3],
    pub(crate) op_flash_phase: [String; 4],
    pub(crate) closing_dump: String,
    pub(crate) flash_completed: String,
    pub(crate) root_completed: String,
    pub(crate) unroot_completed: String,
    pub(crate) adb_no_kver: String,
    pub(crate) backup_saved_prefix: String,
    pub(crate) root_resolved_prefix: String,
    pub(crate) root_backup_copy_prefix: String,
}

/// Classify a model → rollback-protection i18n key. Every supported model
/// enforces AVB rollback protection except the PRC-only TB322FC, and an
/// unknown model is assumed protected, so this is a TB322FC check.
fn arb_from_model(model: &str) -> &'static str {
    if is_rollback_protected_model(model) {
        "arb_yes"
    } else {
        "arb_no"
    }
}

/// Normalize an optional fastboot `current-slot` to a partition suffix
/// (`_a`/`_b`), defaulting to `_a` when unknown (e.g. EDL-start with no
/// fastboot probe).
pub(crate) fn active_slot_suffix(slot: Option<&str>) -> &'static str {
    match slot {
        Some(s) if s.eq_ignore_ascii_case("_b") || s.eq_ignore_ascii_case("b") => "_b",
        _ => "_a",
    }
}

/// Read the device's committed AVB rollback index by dumping the active-slot
/// `boot` + `vbmeta_system` over EDL and taking the higher index. Used when
/// fastboot can't report `stored_rollback_index` (every model but the no-ARB
/// TB322FC). `slot` is the active-slot suffix; falls back to `_a` when
/// unknown. The max is the device's rollback floor — bumping a partition
/// above its own claim is safe, so the generic key-map overlay path needs
/// only this single value (vs the per-partition split the TB323FU testkey
/// path keeps for its re-sign targets).
fn read_device_rollback_index_via_edl(
    session: &mut ltbox_device::edl::EdlSession,
    slot: Option<&str>,
    work_dir: &std::path::Path,
    log: &mut Vec<String>,
) -> std::result::Result<u64, String> {
    let s = active_slot_suffix(slot);
    let boot = format!("boot{s}");
    let vbs = format!("vbmeta_system{s}");
    let boot_lun = ltbox_core::partition_lun::lun_for_partition(&boot)
        .ok_or_else(|| format!("no LUN for {boot}"))?;
    let vbs_lun = ltbox_core::partition_lun::lun_for_partition(&vbs)
        .ok_or_else(|| format!("no LUN for {vbs}"))?;
    let boot_img = work_dir.join(format!("dev_{boot}.img"));
    let vbs_img = work_dir.join(format!("dev_{vbs}.img"));
    session
        .dump_partition(&boot, &boot_img, 0, boot_lun, log)
        .map_err(|e| format!("dump device {boot}: {e}"))?;
    session
        .dump_partition(&vbs, &vbs_img, 0, vbs_lun, log)
        .map_err(|e| format!("dump device {vbs}: {e}"))?;
    let boot_idx = ltbox_patch::avb::extract_image_avb_info(&boot_img)
        .map_err(|e| format!("AVB {boot}: {e}"))?
        .rollback_index;
    let vbs_idx = ltbox_patch::avb::extract_image_avb_info(&vbs_img)
        .map_err(|e| format!("AVB {vbs}: {e}"))?
        .rollback_index;
    let _ = std::fs::remove_file(&boot_img);
    let _ = std::fs::remove_file(&vbs_img);
    Ok(boot_idx.max(vbs_idx))
}

/// Trim Lenovo build-display to the ROM + version tail. Example:
/// `TB322FC_..._ZUXOS_1.5.10.183_ST_...` → `ZUXOS_1.5.10.183_ST_...`.
/// ROW firmware uses `_ZUI_`. No marker → passthrough.
fn trim_build_display(s: &str) -> String {
    if let Some(i) = s.find("_ZUXOS_") {
        return s[i + 1..].to_string();
    }
    if let Some(i) = s.find("_ZUI_") {
        return s[i + 1..].to_string();
    }
    s.to_string()
}

/// True if the ADB product name is a TWRP recovery build. Lenovo stock
/// never uses this prefix, so it's reliable without `ro.bootmode`.
fn is_twrp_product(product: &str) -> bool {
    product.to_ascii_lowercase().starts_with("twrp_")
}

/// Strip a leading `twrp_` (any case) from a product name.
fn strip_twrp_prefix(product: &str) -> String {
    if is_twrp_product(product) {
        product[5..].to_string()
    } else {
        product.to_string()
    }
}

/// Normalize a raw `getprop` value for display: trim surrounding whitespace
/// and treat an empty or whitespace-only result (what `getprop` prints for an
/// absent property) as `None`.
fn non_empty_prop(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Pick the dashboard device name from the LGSI market-name properties in
/// priority order, falling back to the legacy `kirby_en` property. `getprop`
/// is invoked lazily, so the probe stops at the first populated property.
///
/// `getprop(name)` returns the raw `getprop <name>` output (empty/whitespace
/// when the property is absent).
fn select_device_name<F: FnMut(&str) -> String>(mut getprop: F) -> String {
    [
        "ro.vendor.config.lgsi.en.market_name",
        "ro.vendor.config.lgsi.market_name",
        "ro.config.lgsi.market_name",
        "ro.vendor.config.lgsi.kirby_en",
    ]
    .into_iter()
    .find_map(|prop| non_empty_prop(&getprop(prop)))
    .unwrap_or_default()
}

/// GBL EFI asset suffix for a TB323FU target firmware, by region (`is_prc`) and
/// whether the anti-rollback build is needed (`arb`). Picks the
/// `*_prc.efi` / `*_row.efi` asset (or `*_prc_arb.efi` / `*_row_arb.efi`) from
/// the gbl_root_canoe release. The `_arb` GBL roots trust at the testkey so it
/// accepts the testkey-re-signed boot chain LTBox stages on a downgrade. The
/// region comes from the vendor_boot `product_region` DTB marker — TB323FU's AVB
/// fingerprint carries no `_PRC`/`_ROW` token.
pub(crate) fn efisp_asset_suffix(is_prc: bool, arb: bool) -> &'static str {
    match (is_prc, arb) {
        (true, false) => "_prc.efi",
        (true, true) => "_prc_arb.efi",
        (false, false) => "_row.efi",
        (false, true) => "_row_arb.efi",
    }
}

/// A dumped `efisp` partition counts as empty (un-provisioned) when every byte
/// is zero — the stock/erased state. A GBL-provisioned `efisp` carries the EFI
/// payload, so it has non-zero bytes. The TB323FU root gate refuses to proceed
/// on an empty `efisp`.
fn efisp_is_empty(data: &[u8]) -> bool {
    data.iter().all(|&b| b == 0)
}

/// One staged ARB overlay: (GPT label, UFS LUN, patched image path).
pub(crate) type ArbOverlay = (String, u8, std::path::PathBuf);

/// TB323FU firmware-flash anti-rollback. Unlike the generic fastboot path,
/// TB323FU never exposes `stored_rollback_index` so the device-committed
/// indices are read by dumping `boot_a` + `vbmeta_system_a` over the open
/// EDL session. When the install images sit BELOW those indices (a downgrade
/// the bootloader would reject) the four AVB-signed partitions are re-signed
/// with `testkey_rsa4096` and staged as overlays — boot / vbmeta_system get
/// their rollback index bumped to the device value, recovery keeps its stock
/// index (re-signed only), and vbmeta is rebuilt with its boot / recovery /
/// vbmeta_system chain descriptors repointed at the testkey so the re-signed
/// trio verifies. recovery + vbmeta are install-image-only (no device dump).
///
/// Returns `(overlays, need)`. `need` is true when a downgrade was patched —
/// the caller swaps the efisp GBL to its `_arb` (testkey-root) variant. When
/// `need` is false the install images are flashed stock (empty overlays).
pub(crate) fn build_tb323fu_arb_overlays(
    session: &mut ltbox_device::edl::EdlSession,
    fw_dir: &std::path::Path,
    work_dir: &std::path::Path,
    slot: Option<&str>,
    device_floors: Option<(u64, u64)>,
    force_resign: bool,
    log: &mut Vec<String>,
) -> std::result::Result<(Vec<ArbOverlay>, bool), String> {
    const KEY: &str = "testkey_rsa4096";
    const ALGO: &str = "SHA256_RSA4096";

    let lun_of = |label: &str| -> std::result::Result<u8, String> {
        ltbox_core::partition_lun::lun_for_partition(label)
            .ok_or_else(|| format!("no hardcoded LUN for {label}"))
    };
    let idx_of = |path: &std::path::Path| -> std::result::Result<u64, String> {
        Ok(ltbox_patch::avb::extract_image_avb_info(path)
            .map_err(|e| format!("AVB inspect {}: {e}", path.display()))?
            .rollback_index)
    };

    // 1. Device-committed per-location indices (boot LUN 4, vbmeta_system
    //    LUN 0). On an EDL-start flash the caller passes component-wise maxima
    //    already read across BOTH slots (the active slot is unknown there, and
    //    AVB indices are per-location, so a single slot can underestimate one
    //    location). Otherwise read the ACTIVE slot here — a first-time user may
    //    still be on `_b`, so don't assume `_a`.
    let (dev_boot_idx, dev_vbs_idx) = match device_floors {
        Some(floors) => floors,
        None => {
            let dev_boot = format!("boot{}", active_slot_suffix(slot));
            let dev_vbs = format!("vbmeta_system{}", active_slot_suffix(slot));
            let dev_boot_img = work_dir.join(format!("dev_{dev_boot}.img"));
            let dev_vbs_img = work_dir.join(format!("dev_{dev_vbs}.img"));
            session
                .dump_partition(&dev_boot, &dev_boot_img, 0, lun_of(&dev_boot)?, log)
                .map_err(|e| format!("dump device {dev_boot}: {e}"))?;
            session
                .dump_partition(&dev_vbs, &dev_vbs_img, 0, lun_of(&dev_vbs)?, log)
                .map_err(|e| format!("dump device {dev_vbs}: {e}"))?;
            let b = idx_of(&dev_boot_img)?;
            let v = idx_of(&dev_vbs_img)?;
            let _ = std::fs::remove_file(&dev_boot_img);
            let _ = std::fs::remove_file(&dev_vbs_img);
            (b, v)
        }
    };

    // 2. Install-image indices.
    let inst_boot = fw_dir.join("boot.img");
    let inst_vbs = fw_dir.join("vbmeta_system.img");
    let inst_rec = fw_dir.join("recovery.img");
    let inst_vbmeta = fw_dir.join("vbmeta.img");
    for p in [&inst_boot, &inst_vbs, &inst_rec, &inst_vbmeta] {
        if !p.exists() {
            return Err(format!("install image missing: {}", p.display()));
        }
    }
    let inst_boot_idx = idx_of(&inst_boot)?;
    let inst_vbs_idx = idx_of(&inst_vbs)?;
    ltbox_core::live!(
        log,
        "[ARB] {}",
        tr_args!(
            "live_arb_tb323_indices",
            boot_i = inst_boot_idx.to_string(),
            boot_d = dev_boot_idx.to_string(),
            vbs_i = inst_vbs_idx.to_string(),
            vbs_d = dev_vbs_idx.to_string()
        )
    );

    // 3. need = any dumped partition is behind the device-committed index.
    let need = inst_boot_idx < dev_boot_idx || inst_vbs_idx < dev_vbs_idx;
    // `force_resign` re-signs to testkey even without a downgrade: a non-TB323FU
    // device on a testkey root must accept a testkey-fixed ("key2") firmware,
    // which means re-signing the install images to the testkey regardless of
    // rollback index.
    if !need && !force_resign {
        ltbox_core::live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_arb_tb323_skip_uptodate")
        );
        return Ok((Vec::new(), false));
    }

    // 4. Re-sign overlays (all testkey). boot / vbmeta_system bump to the
    // device value; never lower the image's own claim, hence the max().
    let boot_target = inst_boot_idx.max(dev_boot_idx);
    let vbs_target = inst_vbs_idx.max(dev_vbs_idx);
    let out_boot = work_dir.join("boot.arb.img");
    let out_vbs = work_dir.join("vbmeta_system.arb.img");
    let out_rec = work_dir.join("recovery.arb.img");
    let out_vbmeta = work_dir.join("vbmeta.arb.img");
    std::fs::copy(&inst_boot, &out_boot).map_err(|e| format!("copy boot: {e}"))?;
    std::fs::copy(&inst_vbs, &out_vbs).map_err(|e| format!("copy vbmeta_system: {e}"))?;
    std::fs::copy(&inst_rec, &out_rec).map_err(|e| format!("copy recovery: {e}"))?;

    let boot_algo = ltbox_patch::avb::extract_image_avb_info(&out_boot)
        .map_err(|e| format!("boot AVB: {e}"))?
        .algorithm;
    ltbox_patch::avb::resign_image(&out_boot, KEY, &boot_algo, Some(boot_target))
        .map_err(|e| format!("resign boot: {e}"))?;
    let vbs_algo = ltbox_patch::avb::extract_image_avb_info(&out_vbs)
        .map_err(|e| format!("vbmeta_system AVB: {e}"))?
        .algorithm;
    ltbox_patch::avb::resign_image(&out_vbs, KEY, &vbs_algo, Some(vbs_target))
        .map_err(|e| format!("resign vbmeta_system: {e}"))?;
    let rec_algo = ltbox_patch::avb::extract_image_avb_info(&out_rec)
        .map_err(|e| format!("recovery AVB: {e}"))?
        .algorithm;
    ltbox_patch::avb::resign_image(&out_rec, KEY, &rec_algo, None)
        .map_err(|e| format!("resign recovery: {e}"))?;
    ltbox_patch::avb::rebuild_vbmeta_rechained(
        &out_vbmeta,
        &inst_vbmeta,
        &["boot", "recovery", "vbmeta_system"],
        KEY,
        KEY,
        ALGO,
    )
    .map_err(|e| format!("rebuild vbmeta: {e}"))?;
    ltbox_core::live!(
        log,
        "[ARB] {}",
        tr_args!(
            "live_arb_tb323_resigned",
            boot = boot_target.to_string(),
            vbs = vbs_target.to_string()
        )
    );

    // 5. Overlays: (GPT label, LUN, patched path). Flashed after rawprogram.
    // Ordered to shrink the partial-write brick window: leaf images first,
    // the boot source next, and the root `vbmeta_a` (which ties the whole
    // chain together) LAST — and the caller flashes the `_arb` GBL only
    // after every overlay lands.
    let overlays = vec![
        ("recovery_a".to_string(), lun_of("recovery_a")?, out_rec),
        (
            "vbmeta_system_a".to_string(),
            lun_of("vbmeta_system_a")?,
            out_vbs,
        ),
        ("boot_a".to_string(), lun_of("boot_a")?, out_boot),
        ("vbmeta_a".to_string(), lun_of("vbmeta_a")?, out_vbmeta),
    ];
    Ok((overlays, true))
}

/// Route device into EDL (Qualcomm 9008). Shared by Root/Unroot/Flash.
///
/// Already-EDL: no-op. Fastboot live: continue system boot, wait for ADB,
/// then `adb reboot edl`. ADB live: `adb reboot edl`. If ADB is not
/// usable, ask the user to reboot manually and wait for 9008.
///
/// `conn` is the caller's captured `App.connection`, used only as a
/// fallback. The body re-probes EDL → Fastboot → ADB live because flows
/// (e.g. Flash) may reboot the device themselves between worker spawn
/// and the EDL transition (ADB → bootloader for variable query), making
/// the captured `conn` stale.
pub(crate) fn transition_to_edl(
    conn: ConnectionStatus,
    _ll: &LiveLabels,
    log: &mut Vec<String>,
) -> std::result::Result<(), String> {
    let live = probe_connection_for_edl().unwrap_or(conn);
    ensure_edl(live, "EDL", log).map_err(|()| "Could not transition device to EDL".to_string())
}

/// Quick EDL/Fastboot/ADB probe in that order. Returns `None` only when
/// every transport is silent (caller falls back to its captured conn).
fn probe_connection_for_edl() -> Option<ConnectionStatus> {
    if ltbox_device::edl::check_device() {
        return Some(ConnectionStatus::Edl);
    }
    if ltbox_device::fastboot::FastbootDevice::check_device() {
        return Some(ConnectionStatus::Fastboot);
    }
    let mut adb = ltbox_device::adb::AdbManager::new();
    match adb.check_device_state().ok().flatten() {
        Some("device" | "recovery") => Some(ConnectionStatus::Adb),
        Some("adb_server_blocking") => Some(ConnectionStatus::AdbServerBlocking),
        Some("unauthorized" | "authorizing") => Some(ConnectionStatus::AdbUnauthorized),
        _ => None,
    }
}

fn install_root_manager_apk(
    manager_apk: &std::path::Path,
    log: &mut Vec<String>,
) -> std::result::Result<(), String> {
    let mut adb = ltbox_device::adb::AdbManager::new();
    if !adb.check_device().unwrap_or(false) {
        return Err("ADB device is not connected".to_string());
    }
    let path = manager_apk.to_string_lossy().to_string();
    live!(
        log,
        "[Root] {}",
        tr_args!(
            "log_root_installing_manager_apk",
            path = manager_apk.display().to_string()
        )
    );
    adb.install(&path)
        .map_err(|e| format!("Manager APK install failed: {e}"))?;
    live!(
        log,
        "[Root] {}",
        ltbox_core::i18n::tr("log_root_manager_apk_installed")
    );
    Ok(())
}

fn wait_and_install_root_manager_apk(
    manager_apk: &std::path::Path,
    timeout: std::time::Duration,
    log: &mut Vec<String>,
) -> std::result::Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    live!(
        log,
        "[Root] {}",
        tr_args!(
            "live_root_wait_adb_for_apk",
            seconds = timeout.as_secs().to_string()
        )
    );
    loop {
        match install_root_manager_apk(manager_apk, log) {
            Ok(()) => return Ok(()),
            // Return the raw install error only — the caller wraps it
            // with the manual-install reminder template (avoids the
            // "Install manually: {path}" hint showing up twice in the
            // same log line).
            Err(last) if std::time::Instant::now() >= deadline => return Err(last),
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

/// After the manager APK fails to auto-install, copy it onto the device at
/// `/sdcard/manager.apk` so the user can install it there by hand.
///
/// Returns the path to surface in the manual-install reminder plus whether
/// the local staging copy must be kept:
/// - `(/sdcard/manager.apk, false)` — the push succeeded, so the on-device
///   copy is enough and the staging dir can be cleaned up.
/// - `(local apk path, true)` — the push also failed, so the user needs the
///   local file and the caller must keep the staging dir.
///
/// A fresh [`AdbManager`] is used so a transport dropped by the failed
/// install is re-claimed cleanly.
fn stage_manager_apk_for_manual_install(
    apk: &std::path::Path,
    log: &mut Vec<String>,
) -> (std::path::PathBuf, bool) {
    const REMOTE: &str = "/sdcard/manager.apk";
    let mut adb = ltbox_device::adb::AdbManager::new();
    if adb.check_device().unwrap_or(false) && adb.push_file(apk, REMOTE).is_ok() {
        live!(
            log,
            "[Root] {}",
            tr_args!("log_root_manager_apk_pushed", path = REMOTE)
        );
        (std::path::PathBuf::from(REMOTE), false)
    } else {
        live!(
            log,
            "[Root] {}",
            ltbox_core::i18n::tr("log_root_manager_apk_push_failed")
        );
        (apk.to_path_buf(), true)
    }
}

/// `Task<Message>` wrapping `rfd::AsyncFileDialog::pick_folder` for
/// direct `return` from an update handler.
///
/// `kind` selects the recents bucket — the dialog seeds its starting
/// directory from the kind's most-recent path so users land where they
/// last worked. Call-sites that don't fit one of the 4 folder categories
/// should use [`PickerKind::OutputFolder`] rather than introducing a new
/// kind silently.
fn pick_folder_task(
    kind: pickers::PickerKind,
    recents: &settings_store::RecentPaths,
    on_pick: fn(Option<String>) -> Message,
) -> Task<Message> {
    pickers::pick_folder_for(kind, recents, on_pick)
}

fn loader_file_spec(target_i18n_key: &'static str) -> pickers::FilePickSpec {
    // LTBox-supported devices ship `xbl_s_devprg_ns.melf` as the only
    // viable Firehose loader, so the picker accepts `.melf`. TB323FU
    // (kaanapali chipset) uses a multi-image manifest instead — a
    // `qsahara_device_programmer.xml` enumerating the per-id ELF /
    // MBN payloads — so the picker also accepts `.xml`. Filename
    // itself is not enforced for the .melf case; the model-aware
    // resolver upgrades a TB323FU `.melf` selection to the manifest
    // sitting next to it.
    pickers::FilePickSpec::single(target_i18n_key)
        .with_filter("EDL loader (.melf / .xml / .x)", LOADER_PICKER_EXTS)
}

/// Wrap a heavy blocking flow as a `Task<Message>`. Runs `f` on the
/// 64 MiB heavy-task pool via `spawn_blocking + run_heavy`, then sends
/// the result through `done`. Both `run_heavy` panics and the
/// `spawn_blocking` JoinError collapse to a single error string passed
/// to `fallback`, so callers no longer hand-write the two-level
/// `unwrap_or_else` chain.
fn task_heavy<T, F, G>(f: F, done: fn(T) -> Message, fallback: G) -> Task<Message>
where
    F: FnOnce() -> T + Send + 'static,
    G: FnOnce(String) -> T + Send + 'static,
    T: Send + 'static,
{
    Task::perform(
        async move {
            match tokio::task::spawn_blocking(move || ltbox_core::runtime::run_heavy(f)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => fallback(e),
                Err(_) => fallback("task panicked".to_string()),
            }
        },
        done,
    )
}

// =========================================================================
// App
// =========================================================================

/// Which Advanced sub-wizard (if any) currently owns the screen. Sum
/// type so the four sub-wizards stay mutually exclusive at the type
/// level — adding a fifth wizard turns existing read sites into
/// non-exhaustive `match` errors instead of silent precedence bugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AdvancedWizardOpen {
    #[default]
    None,
    FlashParts,
    DumpParts,
    DumpPhys,
    FlashPhys,
}

impl AdvancedWizardOpen {
    fn is_open(self) -> bool {
        !matches!(self, Self::None)
    }
    fn is_flash_parts(self) -> bool {
        matches!(self, Self::FlashParts)
    }
    fn is_dump_parts(self) -> bool {
        matches!(self, Self::DumpParts)
    }
    fn is_dump_phys(self) -> bool {
        matches!(self, Self::DumpPhys)
    }
    fn is_flash_phys(self) -> bool {
        matches!(self, Self::FlashPhys)
    }
}

fn edl_entry_action(conn: ConnectionStatus) -> EdlEntryAction {
    match conn {
        ConnectionStatus::Edl => EdlEntryAction::AlreadyEdl,
        ConnectionStatus::Adb | ConnectionStatus::AdbRecovery => EdlEntryAction::AdbReboot,
        ConnectionStatus::Fastboot => EdlEntryAction::FastbootRebootThenAdb,
        ConnectionStatus::AdbUnauthorized
        | ConnectionStatus::AdbServerBlocking
        | ConnectionStatus::None => EdlEntryAction::ManualWait,
    }
}

struct App {
    window_id: Option<iced::window::Id>,
    current_view: View,
    /// Effective dark-mode flag — cached to keep repaint off the OS
    /// registry. Recomputed on theme-choice change.
    dark_mode: bool,
    theme_choice: ThemeChoice,
    theme_seed: ThemeSeed,
    settings: SettingsState,
    translations: Translations,
    root: RootWizard,
    flash: FlashWizard,
    sysupdate: SysUpdateWizard,
    unroot: UnrootWizard,
    adv_confirm: Option<AdvAction>,
    /// Staged path for the pending advanced action — replayed into the
    /// exec path on Start so no second dialog fires.
    adv_confirm_path: Option<String>,
    /// Advanced wizard state. Mirrors into `adv_confirm*` on exec so
    /// the legacy handlers stay oblivious.
    adv_wizard: AdvWizard,
    wf_config: WorkflowConfig,
    country_popup_open: bool,
    /// Routes `SelectCountry` back to the Advanced wizard instead of
    /// the Flash flow when PatchDevinfo opened the popup.
    adv_needs_country: bool,
    /// Region-convert target picker overlay. Shown when the
    /// `RegionConvert` wizard reaches step 1 so the user can pick
    /// PRC or ROW as the destination explicitly instead of relying
    /// on the prior auto-flip behaviour.
    region_target_popup_open: bool,
    /// Staging slot for the Reboot confirm popup.
    reboot_confirm_target: Option<RebootTarget>,
    // Device & operation state
    connection: ConnectionStatus,
    device_model: String,
    device_slot: String,
    device_firmware: String,
    /// Untrimmed `ro.build.display.id`. Mirrors `device_firmware` but
    /// keeps the leading device-model prefix so the OTA popup can
    /// pass the full string to Lenovo's `querynewfirmware` endpoint
    /// (the trimmed dashboard form would silently miss every match).
    device_firmware_full: String,
    device_arb: String,
    device_ram: String,
    device_storage: String,
    device_market_name: String,
    /// Last-seen device serial captured by `DevicePolled` (ADB or
    /// fastboot). Empty when nothing reachable produces a serial. Drives
    /// the device-info popup query — reset to empty on disconnect so a
    /// stale serial cannot trigger an unrelated upstream lookup after a
    /// hardware swap mid-session.
    device_serial: String,
    /// Session-scoped cache for the Lenovo PTSTPD device-info popup,
    /// keyed by serial. Lives only as long as the App — process exit
    /// drops the map, no persistence — so the user is not asked to
    /// "remember" anything across runs and the same serial is queried
    /// at most once per session.
    device_info_cache: std::collections::HashMap<String, ltbox_core::lenovo_info::MachineInfo>,
    /// Device-info popup state. `Some((serial, state))` while open.
    device_info_popup: Option<(String, DeviceInfoState)>,
    /// Firmware-OTA popup state. `Some((serial, firmware_id, state))` while open.
    ota_popup: Option<(String, String, OtaPopupState)>,
    /// Session OTA cache. `None` value = NoUpdate (still cached); errors not cached.
    ota_cache:
        std::collections::HashMap<(String, String), Option<ltbox_core::lenovo_ota::OtaUpdate>>,
    /// Selectable mirror of OTA changelog — `text` widget can't be selected.
    ota_changelog_editor: iced::widget::text_editor::Content,
    /// PatchArb wizard's unix-timestamp input popup.
    arb_index_popup_open: bool,
    /// Transient toast message; auto-cleared by a delayed task.
    toast_msg: Option<String>,
    /// Sidebar hover state — true when mouse is over the rail.
    sidebar_expanded: bool,
    /// Tween progress in [0.0, 1.0]. Width = lerp(64, 210, anim).
    /// Driven by an M3 Expressive Spatial spring (see `SidebarAnimTick`).
    sidebar_anim: f32,
    /// Spring velocity for `sidebar_anim`. Settle requires both the
    /// displacement to target AND the velocity to be near zero so we
    /// don't stop the subscription mid-overshoot.
    sidebar_velocity: f32,
    /// Current logical window size. Tracks `Event::Window(Resized)`
    /// so the user's preferred geometry survives restarts via
    /// `PersistedSettings::window_size`. A simple `Instant` debounce
    /// throttles persistence writes during cursor-drag resize since
    /// resize events fire on every frame.
    window_size: (f32, f32),
    /// Last instant a window-size save hit disk. Cursor-drag resize
    /// fires `Resized` continuously; persistence is throttled to once
    /// per `WINDOW_SIZE_SAVE_INTERVAL`.
    window_size_last_save: std::time::Instant,
    /// `true` while a pending window-size update hasn't been flushed
    /// to disk. Cleared by `persist_window_size_if_due`.
    window_size_dirty: bool,
    // Device portrait derived at view time via `device_portrait()`.
    platform_supported: Option<bool>,
    busy: bool,
    /// View that owns the current busy op — labels the dashboard
    /// "in progress" card with the sidebar name.
    busy_view: Option<View>,
    /// Persisted recent picks. Rendered as chips under every picker.
    recent_paths: settings_store::RecentPaths,
    /// When set, every loader picker bypasses to this path. Re-validated at exec.
    default_loader_path: Option<String>,
    log_lines: Vec<String>,
    /// Selectable mirror of `log_lines`. Rebuilt on drain tick when `log_dirty`
    /// — batched to keep a long pbr flash from crashing wgpu.
    log_editor: iced::widget::text_editor::Content,
    log_dirty: bool,
    image_info_log: String,
    image_info_log_editor: iced::widget::text_editor::Content,
    pending_log_save_source: LogSaveSource,
    error_msg: Option<String>,
    picker_target: PickerTarget,
    driver_status: Option<ltbox_device::driver::DriverStatus>,
    installing_drivers: bool,
    /// `Some` when the installed Qualcomm driver is older than the latest
    /// release — drives the optional amber "update available" banner. Held
    /// `None` when up to date, not installed, offline, or the user chose
    /// "don't show again".
    driver_update: Option<ltbox_device::driver::DriverUpdate>,
    /// Result of the startup GitHub-reachability probe. `None` until the
    /// probe lands; `Some(false)` disables the driver install/update
    /// buttons with an "internet required" tooltip.
    online: Option<bool>,
    /// Persisted "don't show again" for the driver-update prompt. Skips the
    /// update check + banner; never affects the missing-driver banner.
    qcom_driver_update_dismissed: bool,
    /// Models whose dual-USB-C port advisory the user permanently dismissed
    /// ("don't show again"); loaded from + saved to settings.
    dual_usb_advisory_dismissed: Vec<String>,
    /// Models whose advisory was closed this session only ("close"). Not
    /// persisted, so the advisory returns on the next launch.
    dual_usb_advisory_closed: Vec<String>,
    /// Newest stable (`prerelease == false && draft == false`) release on
    /// `miner7222/LTBox` whose semver is strictly greater than the
    /// running build's. `None` either before the background probe lands
    /// or when the running build is already at-or-ahead of the latest
    /// stable. Populates the green sidebar "Update available" pill.
    update_available: Option<ltbox_core::github::StableRelease>,
    flash_parts: FlashPartsWizard,
    dump_parts: DumpPartsWizard,
    dump_phys: DumpPhysWizard,
    flash_phys: FlashPhysWizard,
    /// Single sum-typed flag for the four mutually-exclusive Advanced
    /// sub-wizards. Replaces 4 parallel booleans whose `if/else if`
    /// read sites would silently pick a precedence if two ever got
    /// set. `match`-driven dispatch makes that bug class unreachable.
    advanced_wizard_open: AdvancedWizardOpen,
    /// Phases of the running op. Populated at exec start, cleared on
    /// `end_op`.
    op_steps: Vec<OpStep>,
    /// Index advanced by parsing `Phase N/M` markers in `log_push`.
    current_op_step: usize,
    log_popup_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PickerTarget {
    #[default]
    None,
    RootFile,
    /// Root pipeline EDL loader (.melf file). Stored in
    /// `self.root.folder_path` despite the name — the field was repurposed
    /// from "firmware folder" to "loader file" when the root flow stopped
    /// needing `rawprogram*.xml` and just uses `qdl-rs dump-part` /
    /// `qdl-rs write` against a GPT-resolved partition name on LUN 4.
    RootLoader,
    UnrootFolder,
    /// Unroot EDL loader (.melf / .xml file) — routes a recent pick into
    /// `unroot.loader_path`. Shares the `File` recents bucket like the other
    /// loader pickers (Root loader, dump/flash loaders).
    UnrootLoader,
    FlashFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LogSaveSource {
    #[default]
    Main,
    ImageInfo,
}

impl PickerTarget {
    /// Map this routing target to the recents bucket it should store into.
    /// `None` returns `File` defensively so callers get a valid bucket even
    /// if they forgot to set the target — the recents entry is harmless;
    /// the field-routing `match` in `FolderSelected` / `FileSelected` is
    /// what actually prevents wrong writes.
    fn kind(self) -> pickers::PickerKind {
        use pickers::PickerKind;
        match self {
            // Root OTA file is a unified file pick (zip or apk).
            // Root loader is also a file pick (.melf) — shares the File
            // bucket so the user sees recent .melf picks in the recents
            // strip regardless of which wizard they came from.
            Self::None | Self::RootFile | Self::RootLoader | Self::UnrootLoader => PickerKind::File,
            // Firmware folders all share the "full QFIL" bucket — Unroot
            // and Flash typically point the user at the same dump/archive
            // they extracted from `ltbox dump full`.
            Self::UnrootFolder | Self::FlashFolder => PickerKind::QfilFirmwareFolder,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        let persisted = settings_store::load();
        let lang = Language::from_code(&persisted.language).unwrap_or(Language::En);
        // Upgrade path: prefer `theme`, fall back to legacy `dark_mode`.
        let theme_choice = ThemeChoice::from_code(&persisted.theme).unwrap_or({
            if persisted.theme.is_empty() && persisted.dark_mode {
                ThemeChoice::Dark
            } else {
                ThemeChoice::System
            }
        });
        let theme_seed = ThemeSeed::from_code(&persisted.theme_seed).unwrap_or_default();
        let dark_mode = match theme_choice {
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
            ThemeChoice::System => theme_detect::system_prefers_dark(),
        };
        theme::set_runtime_theme(theme_seed, dark_mode);
        install_core_translator(lang);
        let translations = Translations::load(lang);
        let ready_log = translations.t("log_ready").to_string();
        Self {
            window_id: None,
            current_view: View::default(),
            dark_mode,
            theme_choice,
            theme_seed,
            settings: SettingsState { language: lang },
            translations,
            root: RootWizard::default(),
            flash: FlashWizard::default(),
            sysupdate: SysUpdateWizard::default(),
            unroot: UnrootWizard::default(),
            adv_confirm: None,
            adv_confirm_path: None,
            adv_wizard: AdvWizard::default(),
            wf_config: WorkflowConfig::default(),
            country_popup_open: false,
            adv_needs_country: false,
            region_target_popup_open: false,
            reboot_confirm_target: None,
            connection: ConnectionStatus::default(),
            device_model: String::new(),
            device_slot: String::new(),
            device_firmware: String::new(),
            device_firmware_full: String::new(),
            device_arb: String::new(),
            device_ram: String::new(),
            device_storage: String::new(),
            device_market_name: String::new(),
            device_serial: String::new(),
            device_info_cache: std::collections::HashMap::new(),
            device_info_popup: None,
            ota_popup: None,
            ota_cache: std::collections::HashMap::new(),
            ota_changelog_editor: iced::widget::text_editor::Content::with_text(""),
            arb_index_popup_open: false,
            toast_msg: None,
            sidebar_expanded: false,
            sidebar_anim: 0.0,
            sidebar_velocity: 0.0,
            // Use the persisted size if present, otherwise the default
            // initial window dimensions (kept in lockstep with the
            // values passed to `iced::window::Settings::size` in `main`).
            window_size: persisted
                .window_size
                .unwrap_or((DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT)),
            window_size_last_save: std::time::Instant::now(),
            window_size_dirty: false,
            platform_supported: None,
            busy: false,
            busy_view: None,
            recent_paths: persisted.recent_paths.clone(),
            default_loader_path: persisted.default_loader_path.clone(),
            log_lines: vec![ready_log.clone()],
            log_editor: iced::widget::text_editor::Content::with_text(&ready_log),
            log_dirty: false,
            image_info_log: String::new(),
            image_info_log_editor: iced::widget::text_editor::Content::with_text(""),
            pending_log_save_source: LogSaveSource::Main,
            error_msg: None,
            picker_target: PickerTarget::None,
            driver_status: None,
            installing_drivers: false,
            driver_update: None,
            online: None,
            qcom_driver_update_dismissed: persisted.qcom_driver_update_dismissed,
            dual_usb_advisory_dismissed: persisted.dual_usb_advisory_dismissed_models.clone(),
            dual_usb_advisory_closed: Vec::new(),
            update_available: None,
            flash_parts: FlashPartsWizard::default(),
            dump_parts: DumpPartsWizard::default(),
            dump_phys: DumpPhysWizard::default(),
            flash_phys: FlashPhysWizard::default(),
            advanced_wizard_open: AdvancedWizardOpen::default(),
            op_steps: Vec::new(),
            current_op_step: 0,
            log_popup_open: false,
        }
    }
}

impl App {
    fn new() -> (Self, Task<Message>) {
        // Window-id + driver check + update check all fire in parallel.
        let app = Self::default();
        let win =
            iced::window::latest().map(|__v| Message::Window(WindowMsg::WindowIdReceived(__v)));
        let driver_check = Task::perform(
            async {
                tokio::task::spawn_blocking(ltbox_device::driver::check_required_drivers)
                    .await
                    .unwrap_or(ltbox_device::driver::DriverStatus::NotWindows)
            },
            Message::DriverCheckDone,
        );
        // GitHub releases probe — runs once at startup. `latest_stable_release`
        // walks `/releases?per_page=100` (not `/releases/latest`) so the
        // result is well-defined even when the repo has only prereleases
        // published. Network failure / parse failure → `None`, no banner.
        let update_check = Task::perform(
            async {
                tokio::task::spawn_blocking(check_for_update)
                    .await
                    .unwrap_or(None)
            },
            Message::UpdateCheckDone,
        );
        // GitHub-reachability probe — gates the driver install/update
        // buttons so the user can't click into a guaranteed-to-fail
        // download while offline.
        let connectivity = Task::perform(
            async {
                tokio::task::spawn_blocking(ltbox_device::driver::probe_connectivity)
                    .await
                    .unwrap_or(false)
            },
            Message::ConnectivityChecked,
        );
        // Qualcomm driver version check. Skipped entirely (no network call)
        // when the user chose "don't show again" for driver updates. A
        // silent failure (offline / GitHub down / parse) yields `None`, so
        // no banner — distinct from the missing-driver banner, which the
        // separate `driver_check` above always drives.
        let driver_update_check = if app.qcom_driver_update_dismissed {
            Task::none()
        } else {
            Task::perform(
                async {
                    tokio::task::spawn_blocking(ltbox_device::driver::check_driver_update)
                        .await
                        .unwrap_or(None)
                },
                Message::DriverUpdateCheckDone,
            )
        };
        (
            app,
            Task::batch([
                win,
                driver_check,
                update_check,
                connectivity,
                driver_update_check,
            ]),
        )
    }
    fn theme(&self) -> Theme {
        self.sync_runtime_theme();
        Theme::custom(
            format!(
                "LTBox {} {}",
                self.theme_seed.code(),
                if self.dark_mode { "dark" } else { "light" }
            ),
            theme::iced_palette(self.theme_seed, self.dark_mode),
        )
    }

    fn sync_runtime_theme(&self) {
        theme::set_runtime_theme(self.theme_seed, self.dark_mode);
    }

    /// Localized string. Falls back to English, then the key itself.
    fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.translations.t(key)
    }

    fn pal(&self) -> Palette {
        palette_for(self.theme_seed, self.dark_mode)
    }

    /// Push one line, trim to `LOG_MAX_LINES`. Editor rebuild is
    /// deferred to the drain tick — per-push reshape was driving
    /// wgpu into TDR during long pbr flashes.
    fn log_push<S: Into<String>>(&mut self, line: S) {
        let s = line.into();
        self.maybe_advance_op_step(&s);
        self.log_lines.push(s);
        self.trim_log();
        self.log_dirty = true;
    }

    /// Tap + sink drain shared by `Message::DrainStdoutTap` and
    /// every `*ExecDone` handler. Pulls third-party `println!` from
    /// the Windows stdout pipe AND our own `live!` lines from the
    /// in-process sink, dedupes against the recent log tail (catches
    /// the tap-late race where a `live!` line lands in the sink at
    /// tick T and only surfaces in the tap at tick T+1) and
    /// in-batch (catches the same line landing in BOTH streams at
    /// the same tick). Returns count of new lines added so callers
    /// can decide whether to rebuild the editor.
    fn drain_pending_log_streams(&mut self) -> usize {
        let tap_lines = stdout_tap::drain();
        let sink_lines = ltbox_core::live_sink::drain();
        let total = tap_lines.len() + sink_lines.len();
        if total == 0 {
            return 0;
        }
        let mut seen: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(total + 32);
        let tail_window = self.log_lines.len().saturating_sub(32);
        seen.extend(self.log_lines[tail_window..].iter().cloned());
        let mut combined: Vec<String> = Vec::with_capacity(total);
        for line in tap_lines.into_iter().chain(sink_lines) {
            if seen.insert(line.clone()) {
                combined.push(line);
            }
        }
        let added = combined.len();
        if added > 0 {
            self.log_extend(combined);
        }
        added
    }

    /// Final flush at `*ExecDone` time. The closure's local Vec is
    /// dropped — `live!` already pushed every line through the sink
    /// path (bulk-streamed across the run) and the macro's Vec copy
    /// is pure dead weight at completion time. Re-appending it via
    /// `log_extend` doubled the entire transcript on screen; the
    /// adjacent-tail dedup only collapses the boundary line, not the
    /// 100+ interior lines.
    fn flush_exec_done_log(&mut self, _vec_from_closure: Vec<String>) {
        // `_vec_from_closure` intentionally ignored — see above.
        // Drain whatever the 500 ms tick missed between the last
        // `Message::DrainStdoutTap` and the closure's return so the
        // user sees the closing lines without a tick of latency.
        self.drain_pending_log_streams();
    }

    /// Bulk append; one truncation pass.
    fn log_extend<I: IntoIterator<Item = String>>(&mut self, lines: I) {
        // Adjacent dedup against the existing tail. The `live!` macro
        // now both prints (for the stdout tap) and pushes to the
        // closure's local Vec (for *ExecDone resilience), so the same
        // line can arrive twice — once via tap drain in real time, then
        // again when the closure returns and the Vec is `log_extend`ed.
        // Skipping over a matching prefix collapses the dup back to one
        // entry without losing lines that the tap actually missed.
        let mut prev_tail = self.log_lines.last().cloned();
        let mut accepted: Vec<String> = Vec::new();
        for line in lines {
            if prev_tail.as_deref() == Some(line.as_str()) {
                continue;
            }
            self.maybe_advance_op_step(&line);
            prev_tail = Some(line.clone());
            accepted.push(line);
        }
        if !accepted.is_empty() {
            self.log_lines.extend(accepted);
            self.trim_log();
            self.log_dirty = true;
        }
    }

    /// Advance `current_op_step` on a `Phase N/M` match. Silent no-op
    /// when no op is in flight or the line has no marker.
    fn maybe_advance_op_step(&mut self, line: &str) {
        if self.op_steps.is_empty() {
            return;
        }
        if let Some(n) = parse_phase_marker(line)
            && n > 0
        {
            let cap = self.op_steps.len();
            self.current_op_step = (n - 1).min(cap.saturating_sub(1));
        }
    }

    /// Start a new long-running op. Sets `busy` + `busy_view`; drops
    /// an `=`-bar into the log so consecutive runs are distinguishable.
    fn begin_op(&mut self, v: View) {
        self.busy = true;
        self.busy_view = Some(v);
        self.error_msg = None;
        self.op_steps.clear();
        self.current_op_step = 0;
        // Single START banner; no closing rule.
        let _ = v;
        let label = self.t("log_separator_start").to_string();
        self.log_separator(Some(&label));
    }

    /// 7-phase Root flow (Phase 1/7 → 7/7).
    fn derive_root_op_steps(&self) -> Vec<OpStep> {
        [
            "op_root_phase_1",
            "op_root_phase_2",
            "op_root_phase_3",
            "op_root_phase_4",
            "op_root_phase_5",
            "op_root_phase_6",
            "op_root_phase_7",
        ]
        .iter()
        .map(|k| OpStep {
            label: self.t(k).to_string(),
        })
        .collect()
    }

    /// 3-phase Unroot flow.
    fn derive_unroot_op_steps(&self) -> Vec<OpStep> {
        [
            "op_unroot_phase_1",
            "op_unroot_phase_2",
            "op_unroot_phase_3",
        ]
        .iter()
        .map(|k| OpStep {
            label: self.t(k).to_string(),
        })
        .collect()
    }

    /// Snapshot localized log strings for use across thread boundaries.
    fn live_labels(&self) -> LiveLabels {
        let t = |k: &str| self.t(k).to_string();
        LiveLabels {
            op_root_phase: [
                t("op_root_phase_1"),
                t("op_root_phase_2"),
                t("op_root_phase_3"),
                t("op_root_phase_4"),
                t("op_root_phase_5"),
                t("op_root_phase_6"),
                t("op_root_phase_7"),
            ],
            op_unroot_phase: [
                t("op_unroot_phase_1"),
                t("op_unroot_phase_2"),
                t("op_unroot_phase_3"),
            ],
            op_flash_phase: [
                t("op_flash_phase_1"),
                t("op_flash_phase_2"),
                t("op_flash_phase_3"),
                t("op_flash_phase_4"),
            ],
            closing_dump: t("live_closing_dump_session"),
            flash_completed: t("live_flash_completed"),
            root_completed: t("live_root_completed"),
            unroot_completed: t("live_unroot_completed"),
            adb_no_kver: t("live_adb_no_kver"),
            backup_saved_prefix: t("live_backup_saved_prefix"),
            root_resolved_prefix: t("live_root_resolved_prefix"),
            root_backup_copy_prefix: t("live_root_backup_copy_prefix"),
        }
    }

    /// 4-phase Flash flow (validate, EDL, partitions, reboot). Grow
    /// in lockstep if the backend adds a phase.
    fn derive_flash_op_steps(&self) -> Vec<OpStep> {
        [
            "op_flash_phase_1",
            "op_flash_phase_2",
            "op_flash_phase_3",
            "op_flash_phase_4",
        ]
        .iter()
        .map(|k| OpStep {
            label: self.t(k).to_string(),
        })
        .collect()
    }

    /// Pairs with `begin_op`. END separator dropped per user request —
    /// `begin_op` already prints a START banner and the per-op tail
    /// (`Completed` / error popup) is sufficient to mark closure, so
    /// the trailing rule was just visual noise.
    fn end_op(&mut self) {
        if !self.op_steps.is_empty() {
            self.current_op_step = self.op_steps.len() - 1;
        }
        self.busy = false;
        self.busy_view = None;
    }

    fn begin_silent_op(&mut self, v: View) {
        self.busy = true;
        self.busy_view = Some(v);
        self.error_msg = None;
        self.op_steps.clear();
        self.current_op_step = 0;
    }

    fn end_silent_op(&mut self) {
        self.busy = false;
        self.busy_view = None;
    }

    fn set_image_info_log(&mut self, text: String) {
        self.image_info_log = text;
        self.image_info_log_editor =
            iced::widget::text_editor::Content::with_text(&self.image_info_log);
        use iced::widget::text_editor::{Action, Motion};
        self.image_info_log_editor
            .perform(Action::Move(Motion::DocumentEnd));
    }

    fn image_info_exec_active(&self) -> bool {
        self.current_view == View::Advanced
            && self.adv_wizard.is_image_info()
            && self.adv_wizard.step == self.adv_wizard.exec_step()
    }

    fn active_log_save_source(&self) -> LogSaveSource {
        if self.image_info_exec_active() {
            LogSaveSource::ImageInfo
        } else {
            LogSaveSource::Main
        }
    }

    fn country_popup_selected_code(&self) -> Option<&str> {
        if self.adv_needs_country {
            self.adv_wizard.country.as_deref()
        } else {
            self.wf_config.country_action.target()
        }
    }

    fn log_text_for_save(&self, source: LogSaveSource) -> String {
        match source {
            LogSaveSource::Main => self.log_lines.join("\n"),
            LogSaveSource::ImageInfo => self.image_info_log.clone(),
        }
    }

    fn note_log_save_result(&mut self, source: LogSaveSource, line: String) {
        match source {
            LogSaveSource::Main => self.log_push(line),
            LogSaveSource::ImageInfo => {
                let mut text = self.image_info_log.trim_end().to_string();
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&line);
                self.set_image_info_log(text);
            }
        }
    }

    /// 80-wide `=` separator with an optional centred label.
    fn log_separator(&mut self, label: Option<&str>) {
        const BAR: &str =
            "================================================================================";
        let line = match label {
            Some(s) if !s.is_empty() => {
                let inner = format!(" {s} ");
                let bar_len = BAR.len();
                let inner_len = inner.chars().count();
                if inner_len >= bar_len {
                    inner
                } else {
                    let side = (bar_len - inner_len) / 2;
                    let left = &BAR[..side];
                    let right = &BAR[..bar_len - side - inner_len];
                    format!("{left}{inner}{right}")
                }
            }
            _ => BAR.to_string(),
        };
        self.log_push(line);
    }

    fn trim_log(&mut self) {
        if self.log_lines.len() > LOG_MAX_LINES {
            let drop = self.log_lines.len() - LOG_MAX_LINES;
            self.log_lines.drain(..drop);
        }
    }

    fn advanced_inline_exec_surface_active(&self) -> bool {
        if self.advanced_wizard_open.is_flash_parts() {
            return self.flash_parts.step >= 3;
        }
        if self.advanced_wizard_open.is_dump_parts() {
            return self.dump_parts.step >= 2;
        }
        if self.advanced_wizard_open.is_dump_phys() {
            return self.dump_phys.step >= 2;
        }
        if self.advanced_wizard_open.is_flash_phys() {
            return self.flash_phys.step >= 3;
        }
        self.adv_wizard.action.is_some() && self.adv_wizard.step == self.adv_wizard.exec_step()
    }

    fn current_view_has_inline_exec_surface(&self) -> bool {
        match self.current_view {
            View::Flash => self.flash.is_in_exec(),
            View::SystemUpdate => self.sysupdate.is_in_exec(),
            View::Root => self.root.is_in_exec(),
            View::Unroot => self.unroot.is_in_exec(),
            View::Advanced => self.advanced_inline_exec_surface_active(),
            View::Dashboard | View::Reboot | View::Settings => false,
        }
    }

    fn blocking_popup_open(&self) -> bool {
        self.country_popup_open
            || self.reboot_confirm_target.is_some()
            || self.sysupdate.rescue_region_popup_open
            || self.root.superkey_popup_open
            || self.root.run_id_popup_open
            || self.root.kernel_version_popup_open
    }

    /// True when the Advanced view holds wizard state the user would lose to a
    /// sidebar bounce: a generic op sitting on its confirm step, or a partition
    /// read/write whose GPT table is still valid (device still in EDL). The
    /// `Navigate` handler consults this to skip the entry-time reset so
    /// navigating away and back keeps the user's place.
    fn advanced_in_progress(&self) -> bool {
        use AdvancedWizardOpen as W;
        match self.advanced_wizard_open {
            // Generic advanced op (PatchArb / PatchDevinfo / DetectArb / ...):
            // preserve only on the confirm step (waiting to start).
            W::None => self.adv_wizard.action.is_some() && self.adv_wizard.is_confirm_step(),
            // Read/Write Partitions: a rendered GPT table — or the confirm
            // screen after it — survives as long as the device stays in EDL,
            // since the table reflects the live partition layout.
            W::FlashParts => {
                self.connection == ConnectionStatus::Edl && !self.flash_parts.rows.is_empty()
            }
            W::DumpParts => {
                self.connection == ConnectionStatus::Edl && !self.dump_parts.rows.is_empty()
            }
            // Physical storage: preserve the confirm screen (FlashPhys);
            // DumpPhys runs Select → Exec with no confirm screen to preserve.
            W::FlashPhys => self.flash_phys.step + 2 == FLASH_PHYS_STEPS.len(),
            W::DumpPhys => false,
        }
    }

    fn should_show_busy_progress_dialog(&self) -> bool {
        self.busy
            && self.current_view != View::Dashboard
            && !self.blocking_popup_open()
            && !self.current_view_has_inline_exec_surface()
    }

    fn advanced_operation_label(&self) -> Option<String> {
        if self.advanced_wizard_open.is_flash_parts() {
            return Some(self.t(AdvAction::FlashPartitions.label_key()).to_string());
        }
        if self.advanced_wizard_open.is_dump_parts() {
            return Some(self.t(AdvAction::DumpPartitions.label_key()).to_string());
        }
        if self.advanced_wizard_open.is_dump_phys() {
            return Some(self.t(AdvAction::DumpPhysical.label_key()).to_string());
        }
        if self.advanced_wizard_open.is_flash_phys() {
            return Some(self.t(AdvAction::FlashPhysical.label_key()).to_string());
        }
        self.adv_wizard
            .action
            .map(|action| self.t(action.label_key()).to_string())
    }

    fn busy_operation_label(&self) -> String {
        if self.busy_view == Some(View::Advanced)
            && let Some(label) = self.advanced_operation_label()
        {
            return label;
        }
        self.busy_view
            .map(|view| self.t(view.label_key()).to_string())
            .unwrap_or_else(|| self.t("status_working").to_string())
    }

    /// Override busy-dialog body for the four Advanced partition/physical
    /// flows during their reboot → loader → GPT-scan preamble. Gated on
    /// `busy_view == Advanced` so a stale wizard doesn't hijack unrelated ops.
    ///
    /// When the Advanced view is busy but no specific sub-action labels
    /// itself, the default template "{operation} 중입니다." substitutes
    /// in `nav_advanced` ("고급") and reads awkwardly across all four
    /// locales — "고급 중입니다." / "Advanced is in progress." /
    /// "高级 正在进行中。" / "Дополнительно выполняется." — because
    /// the operation token is a section noun, not a verb phrase. The
    /// `busy_advanced_generic` key carries a per-locale full sentence
    /// for this fallback.
    fn busy_body_override(&self) -> Option<String> {
        if self.busy_view != Some(View::Advanced) {
            return None;
        }
        if self.advanced_wizard_open.is_open() {
            // Write Partitions' exec phase is a partition *write*; the loader-
            // upload + GPT scan preamble (and the other advanced flows) keep
            // the scan label.
            let key = if self.advanced_wizard_open.is_flash_parts() && self.flash_parts.is_in_exec()
            {
                "busy_partition_write"
            } else {
                "busy_partition_scan"
            };
            return Some(self.t(key).to_string());
        }
        if self.advanced_operation_label().is_none() {
            return Some(self.t("busy_advanced_generic").to_string());
        }
        None
    }

    /// Rebuild the editor from `log_lines` and auto-scroll to the
    /// bottom via `Motion::DocumentEnd`. Selection state resets.
    fn rebuild_log_editor(&mut self) {
        let joined = self.log_lines.join("\n");
        self.log_editor = iced::widget::text_editor::Content::with_text(&joined);
        use iced::widget::text_editor::{Action, Motion};
        self.log_editor.perform(Action::Move(Motion::DocumentEnd));
        self.log_dirty = false;
    }

    /// Picker shortcut: routes through `default_loader_path` if set,
    /// else opens `loader_file_spec`. Dedupe across `*SelectLoader` handlers.
    fn pick_loader_with_default<F>(&mut self, on_chosen: F) -> Task<Message>
    where
        F: 'static + Send + Fn(Option<String>) -> Message,
    {
        if let Some(path) = self.default_loader_path.clone() {
            return self.update(on_chosen(Some(path)));
        }
        pickers::pick_file_for(
            loader_file_spec("picker_target_edl_loader"),
            &self.recent_paths,
            on_chosen,
        )
    }

    /// Map cached PTSTPD `SaleArea` for the connected device → `DeviceRegion`.
    /// `"CN"` → PRC, JSON null → ROW, anything else → `None`. Cache-only.
    fn inferred_flash_region(&self) -> Option<DeviceRegion> {
        if self.device_serial.is_empty() {
            return None;
        }
        let info = self.device_info_cache.get(&self.device_serial)?;
        match info.field("SaleArea") {
            ltbox_core::lenovo_info::FieldValue::Value(s) if s.eq_ignore_ascii_case("CN") => {
                Some(DeviceRegion::Prc)
            }
            ltbox_core::lenovo_info::FieldValue::Null => Some(DeviceRegion::Row),
            _ => None,
        }
    }

    /// Returns the Settings-level default EDL loader path when it is set
    /// **and** the file currently exists on disk. Used by every wizard
    /// open / reset path to decide whether to pre-fill its loader slot
    /// and skip past the loader step. Returns `None` when the default is
    /// unset or the file has been moved/deleted since it was saved (in
    /// which case the wizard falls back to the picker step as if no
    /// default had been configured — better than auto-advancing past a
    /// step with a missing file and surfacing the error later).
    fn resolved_default_loader(&self) -> Option<String> {
        let p = self.default_loader_path.as_deref()?;
        if std::path::Path::new(p).is_file() {
            Some(p.to_string())
        } else {
            None
        }
    }

    /// Apply the resolved default loader to whichever advanced-wizard
    /// loader-step is currently open. Pre-fills the wizard's `loader_path`
    /// and either advances directly to the Select step (DumpPhys /
    /// FlashPhys — no scan needed) or fires the GPT scan (FlashParts /
    /// DumpParts — Select step requires populated rows). Called from
    /// `AdvConfirm` after a wizard's `_open` flag flips.
    ///
    /// Returns `Task::none()` when the default loader is unset or the
    /// file is missing — the caller's existing flow then surfaces the
    /// loader step as before.
    fn apply_default_loader_to_advanced_wizard(&mut self) -> Task<Message> {
        let Some(path) = self.resolved_default_loader() else {
            return Task::none();
        };
        if self.advanced_wizard_open.is_flash_parts() {
            // Leave step at 0 (Loader); FlashPartsScanDone advances to
            // Select on success, so jumping past step 0 here would
            // double-advance past Select.
            self.flash_parts.loader_path = Some(path);
            return self.update(Message::FlashParts(FlashPartsMsg::FlashPartsScanStart));
        } else if self.advanced_wizard_open.is_dump_parts() {
            self.dump_parts.loader_path = Some(path);
            return self.update(Message::DumpParts(DumpPartsMsg::DumpPartsScanStart));
        } else if self.advanced_wizard_open.is_dump_phys() {
            // Whole-LUN — no scan. Skip to Select directly.
            self.dump_phys.loader_path = Some(path);
            self.dump_phys.step = 1;
        } else if self.advanced_wizard_open.is_flash_phys() {
            self.flash_phys.loader_path = Some(path);
            self.flash_phys.step = 1;
        }
        Task::none()
    }

    /// Validate a picked/default EDL loader before device work starts.
    fn validate_loader_path(&mut self, path: &Option<String>) -> Result<String, ()> {
        let Some(p) = path.as_deref() else {
            self.error_msg = Some(self.t("err_loader_not_selected").to_string());
            return Err(());
        };
        let pb = std::path::Path::new(p);
        if !pb.is_file() {
            let msg = tr_args!("err_loader_missing", path = p);
            self.error_msg = Some(msg);
            return Err(());
        }
        // A `.x` loader (encrypted manifest) passes through as-is here;
        // `EdlSession::open` decrypts it to the sibling `.xml` at load time.
        Ok(p.to_string())
    }

    fn persist_settings(&self) {
        settings_store::save(&settings_store::PersistedSettings {
            language: self.settings.language.code().to_string(),
            theme: self.theme_choice.code().to_string(),
            theme_seed: self.theme_seed.code().to_string(),
            // Legacy field kept readable by older builds.
            dark_mode: self.dark_mode,
            recent_paths: self.recent_paths.clone(),
            default_loader_path: self.default_loader_path.clone(),
            window_size: Some(self.window_size),
            qcom_driver_update_dismissed: self.qcom_driver_update_dismissed,
            dual_usb_advisory_dismissed_models: self.dual_usb_advisory_dismissed.clone(),
        });
    }

    /// The connected dual-USB-C model whose port advisory should currently
    /// show, or `None`. Shows when the model is one of [`DUAL_USBC_MODELS`]
    /// and the user has neither permanently dismissed ("don't show again")
    /// nor session-closed ("close") it.
    fn dual_usb_advisory_model(&self) -> Option<&str> {
        let m = self.device_model.as_str();
        let hidden = |list: &[String]| list.iter().any(|x| x.eq_ignore_ascii_case(m));
        if !m.is_empty()
            && is_dual_usbc_model(m)
            && !hidden(&self.dual_usb_advisory_dismissed)
            && !hidden(&self.dual_usb_advisory_closed)
        {
            Some(m)
        } else {
            None
        }
    }

    /// Record `path` in the MRU list for `kind`. Persists on change so
    /// the list survives restarts (write is cheap — small JSON, and only
    /// triggers when the list actually moves).
    fn remember_recent(&mut self, kind: pickers::PickerKind, path: &str) {
        if self.recent_paths.push(kind.storage_key(), path) {
            self.persist_settings();
        }
    }

    /// Resolve loader input from the unified picker path.
    ///
    /// Preferred path is a file (`*.melf`, `*.mbn`, `*.elf`) and accepts
    /// any filename with one of those extensions. A directory is still
    /// accepted for backwards compatibility with older recents entries
    /// and is resolved via [`find_edl_loader`].
    fn resolve_loader_input(&mut self, selected_path: &str) -> std::result::Result<String, String> {
        let path = std::path::Path::new(selected_path);
        if path.is_file() {
            self.remember_recent(pickers::PickerKind::File, selected_path);
            // TB323FU model gate: if the user picked a `.melf` but the
            // device is a multi-image kaanapali (TB323FU), upgrade to
            // the `qsahara_device_programmer.xml` manifest sitting in
            // the same folder. If the manifest is missing the .melf
            // alone is wrong and would fail mid-Sahara — abort up
            // front. Performed during resolve so the wizard's Confirm
            // step shows the correct path.
            if self.is_tb323fu()
                && is_melf_loader(path)
                && let Some(parent) = path.parent()
            {
                if let Some(manifest) = resolve_sahara_manifest(parent) {
                    return Ok(manifest.to_string_lossy().to_string());
                }
                return Err(format!(
                    "TB323FU requires a multi-image loader manifest. Pick \
                     `qsahara_device_programmer.xml` (or its encrypted `.x`) \
                     directly, or place it next to the chosen .melf file ({}).",
                    path.display()
                ));
            }
            // Encrypted multi-image manifest picked directly
            // (`qsahara_device_programmer.x`) passes through as-is;
            // `EdlSession::open` decrypts it to the sibling `.xml`.
            if ltbox_core::sahara_xml::is_encrypted_manifest_filename(path) {
                return Ok(selected_path.to_string());
            }
            if is_loader_file(path) {
                return Ok(selected_path.to_string());
            }
            return Err(format!(
                "Unsupported loader file: {selected_path} (expected .melf, .mbn, .elf, .xml, or .x manifest)"
            ));
        }

        if path.is_dir() {
            self.remember_recent(pickers::PickerKind::LoaderFolder, selected_path);
            return find_edl_loader(path)
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| format!("xbl_s_devprg_ns.melf not found in {selected_path}"));
        }

        Err(format!("Path does not exist: {selected_path}"))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::time::every(std::time::Duration::from_secs(3)).map(|_| Message::PollDevice),
            // 500 ms drain — 4 Hz drove some GPU drivers into TDR
            // during long qdl flashes.
            iced::time::every(std::time::Duration::from_millis(500))
                .map(|_| Message::DrainStdoutTap),
        ];
        // Sidebar width tween: only emit ticks while the spring
        // hasn't settled at its target so the GPU isn't woken every
        // 16 ms forever. Velocity check catches the overshoot tail.
        let sidebar_settled = (self.sidebar_anim - self.sidebar_anim_target()).abs() < 0.001
            && self.sidebar_velocity.abs() < 0.05;
        if !sidebar_settled {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| Message::SidebarAnimTick),
            );
        }
        // Listen for window resize events so the user's preferred
        // geometry survives a restart. `event::listen_with` filters at
        // the source so non-window events don't bubble back as
        // `Message::Noop`.
        subs.push(iced::event::listen_with(|event, _, _| {
            if let iced::Event::Window(iced::window::Event::Resized(size)) = event {
                Some(Message::WindowResized(size.width, size.height))
            } else {
                None
            }
        }));
        // Debounced window-size persistence tick: only fires while a
        // pending size update hasn't been flushed yet.
        if self.window_size_dirty {
            subs.push(
                iced::time::every(WINDOW_SIZE_SAVE_INTERVAL).map(|_| Message::PersistWindowSize),
            );
        }
        if self.theme_choice == ThemeChoice::System {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(2))
                    .map(|_| Message::RefreshSystemTheme),
            );
        }
        Subscription::batch(subs)
    }

    /// Shared error-state body. Renders the localized header
    /// (`error_key`), the raw upstream error text, and a Retry pill
    /// that fires `retry_msg`. Same shape as the loading view —
    /// pulled out of the device-info / OTA popups which had two
    /// near-identical copies.
    fn popup_error_view(
        &self,
        error_key: &str,
        e: &str,
        retry_msg: Message,
    ) -> Element<'_, Message> {
        column![
            text(self.t(error_key).to_string())
                .size(13)
                .style(|t: &Theme| iced::widget::text::Style {
                    color: Some(pal_of(t).error),
                }),
            text(e.to_string()).size(11).style(muted_style),
            Space::new().height(8),
            button(text(self.t("btn_retry").to_string()).size(12))
                .on_press(retry_msg)
                .padding([6, 18])
                .style(md_filled_btn_style),
        ]
        .spacing(8)
        .into()
    }

    /// Sidebar tween target — `1.0` while hovered, `0.0` otherwise.
    /// `SidebarAnimTick` lerps `sidebar_anim` toward this each frame
    /// and the subscription stops once the two match.
    fn sidebar_anim_target(&self) -> f32 {
        if self.sidebar_expanded { 1.0 } else { 0.0 }
    }

    fn is_nav_enabled(&self, view: View) -> bool {
        if self.platform_supported == Some(false) {
            return matches!(view, View::Dashboard | View::SystemUpdate | View::Settings);
        }
        true
    }

    /// Classification of the polled device — the wizard's gating
    /// branches (Root family availability, EDL loader manifest path,
    /// region-flash availability) ask this enum directly instead of
    /// pattern-matching the raw `device_model` string at every call
    /// site. New SKUs add a variant here once; the existing `is_tbXXX`
    /// methods are thin shims that delegate to this classifier.
    fn device_class(&self) -> DeviceClass {
        DeviceClass::from_model(&self.device_model)
    }

    /// Whether the polled device is a TB320FC. Drives the Root wizard
    /// gating (Magisk family disabled, KernelSU LKM mode disabled —
    /// only KernelSU GKI + APatch family work cleanly on this kernel).
    fn is_tb320fc(&self) -> bool {
        self.device_class() == DeviceClass::TB320FC
    }

    /// Whether the polled device is a TB323FU. Drives the multi-image
    /// EDL loader path: TB323FU's kaanapali chipset doesn't accept a
    /// single `xbl_s_devprg_ns.melf`; it needs the full
    /// `qsahara_device_programmer.xml` manifest + the per-id ELF /
    /// MBN payloads it references. The loader resolver upgrades a
    /// stray `.melf` selection to the manifest when one exists in
    /// the same folder; if not, it aborts up front rather than
    /// failing mid-Sahara.
    fn is_tb323fu(&self) -> bool {
        self.device_class() == DeviceClass::TB323FU
    }

    /// Loader-picker description, resolved live against the connected
    /// device. TB323FU (Y700 Gen 5) needs the `qsahara_device_programmer.xml`
    /// manifest, not the `.melf`; with no recognised model the picker hints
    /// both. Every loader picker routes its subtitle through this.
    fn loader_picker_desc(&self) -> String {
        if self.is_tb323fu() {
            self.t("loader_desc_tb323fu").to_string()
        } else if self.device_model.is_empty() {
            self.t("loader_desc_unknown").to_string()
        } else {
            self.t("dump_parts_loader_desc").to_string()
        }
    }

    /// Whether the polled device is a TB322FC. PRC-only SKU — the Flash
    /// wizard hides ROW + OtherRegion as disabled cards so the user
    /// cannot pick a region or cross-region flash target that the
    /// hardware doesn't ship with.
    fn is_tb322fc(&self) -> bool {
        self.device_class() == DeviceClass::TB322FC
    }

    /// True when the dashboard poll has placed the device in a mode
    /// any wizard can transition out of (`ensure_*` helpers + the
    /// flash/sysupdate bridges). Used to gate every wizard's final
    /// "Start" button — `None` and `AdbUnauthorized` mean we can't
    /// even start the operation, so spawning a worker that would
    /// immediately bail with "no device" is just noise.
    fn device_reachable(&self) -> bool {
        matches!(
            self.connection,
            ConnectionStatus::Adb
                | ConnectionStatus::AdbRecovery
                | ConnectionStatus::Fastboot
                | ConnectionStatus::Edl
        )
    }

    /// Re-populate `ota_changelog_editor` from the current popup
    /// state. Picks `desc_cn` for the Chinese GUI locale (with
    /// `desc_en` fallback when `desc_cn` is empty), `desc_en`
    /// otherwise. Called from both `OtaOpen` (cache restore) and
    /// `OtaFetched` (fresh fetch) so the editor's contents stay in
    /// lockstep with whatever the popup is about to render.
    fn seed_ota_changelog_editor(&mut self, state: &OtaPopupState) {
        let editor_text = if let OtaPopupState::Ready(u) = state {
            let prefer_cn = matches!(self.settings.language, Language::Zh);
            let raw = if prefer_cn && !u.desc_cn.trim().is_empty() {
                &u.desc_cn
            } else if !u.desc_en.trim().is_empty() {
                &u.desc_en
            } else {
                &u.desc_cn
            };
            ltbox_core::lenovo_ota::format_changelog(raw)
        } else {
            String::new()
        };
        self.ota_changelog_editor = iced::widget::text_editor::Content::with_text(&editor_text);
    }

    /// Bottom-of-sidebar pill linking to the GitHub release when a
    /// newer stable build is available.
    fn update_available_pill(
        &self,
        _release: &ltbox_core::github::StableRelease,
    ) -> Element<'_, Message> {
        let label = self.t("sidebar_update_available").to_string();
        // Pill label rides the same opacity tween as nav-button labels
        // for visual coherence. Mount text at any non-zero alpha so it
        // fades in alongside the sidebar width spring rather than
        // popping in at a threshold.
        let label_t = ((self.sidebar_anim - 0.4) / 0.5).clamp(0.0, 1.0);
        let label_alpha = ease_out_cubic(label_t);
        let show_label = label_alpha > 0.0;
        let inner: Element<'_, Message> = if show_label {
            row![
                icon::tile_update_on()
                    .size(16)
                    .style(|t: &Theme| iced::widget::text::Style {
                        color: Some(pal_of(t).on_tertiary)
                    }),
                text(label)
                    .size(13)
                    .line_height(1.2)
                    // No-wrap during sidebar spring: pill label
                    // ("업데이트 가능" / "Доступно обновление") must
                    // not wrap into 2 lines while the panel is narrow.
                    .wrapping(iced::widget::text::Wrapping::None)
                    .style(move |t: &Theme| iced::widget::text::Style {
                        color: Some(with_alpha(pal_of(t).on_tertiary, label_alpha)),
                    }),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            // Force the lucide glyph itself to center inside its
            // measured text box. Wrapping in a center container
            // alone left the glyph anchored to the text widget's
            // top-left, so the bell still rode the left edge.
            // `align_x = Center` on the text widget pulls the glyph
            // onto the box's geometric midpoint.
            icon::tile_update_on()
                .size(16)
                .width(Length::Fixed(20.0))
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center)
                .style(|t: &Theme| iced::widget::text::Style {
                    color: Some(pal_of(t).on_tertiary),
                })
                .into()
        };
        // Horizontal padding tweens with label_alpha so the pill grows
        // smoothly from icon-only (10) to label-bearing (16) rather
        // than jumping in a single frame.
        let pad_x = 10.0 + 6.0 * label_alpha;
        let btn_padding = [10.0, pad_x];
        container(
            button(inner)
                .on_press(Message::OpenUpdateUrl)
                .padding(btn_padding)
                .style(|t: &Theme, status| {
                    let p = pal_of(t);
                    let hover = matches!(status, button::Status::Hovered);
                    let bg = if hover {
                        with_alpha(p.tertiary, 1.0 - theme::state::HOVER * 0.5)
                    } else {
                        p.tertiary
                    };
                    button::Style {
                        background: Some(bg.into()),
                        text_color: p.on_tertiary,
                        border: iced::Border {
                            radius: theme::shape::FULL.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
        )
        // The button widget itself sizes to its content (label + icon),
        // so its width is locale-dependent — Korean "업데이트 가능"
        // renders narrower than Russian "Доступно обновление".
        // `center_x(Length::Fill)` centers the pill in the sidebar
        // column regardless of which language is active. Bottom padding
        // is intentionally larger than the top so the pill clears the
        // sidebar's bottom edge with breathing room rather than hugging
        // the connection-status bar that sits below the sidebar frame.
        .padding(iced::Padding {
            top: 12.0,
            right: 16.0,
            bottom: 24.0,
            left: 16.0,
        })
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
    }

    /// Per-extension recents strip for file pickers.
    fn recent_file_chips<F>(
        &self,
        accepted_exts: &[&str],
        on_pick: F,
        label_key: &str,
    ) -> Element<'_, Message>
    where
        F: Fn(String) -> Message,
    {
        let all = self
            .recent_paths
            .recent(pickers::PickerKind::File.storage_key());
        let filtered: Vec<String> = if accepted_exts.is_empty() {
            all.to_vec()
        } else {
            all.iter()
                .filter(|p| {
                    std::path::Path::new(p)
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| accepted_exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
                })
                .cloned()
                .collect()
        };
        self.recent_chips(&filtered, on_pick, label_key, true)
    }

    /// Empty column when the list is empty so call sites can splice
    /// it in unconditionally.
    fn recent_chips<F>(
        &self,
        items: &[String],
        on_pick: F,
        label_key: &str,
        is_file_picker: bool,
    ) -> Element<'_, Message>
    where
        F: Fn(String) -> Message,
    {
        if items.is_empty() {
            return iced::widget::column![].into();
        }
        let label_row = row![
            lucide_icon(icon::history(), 12.0, |t: &Theme| pal_of(t)
                .on_surface_variant),
            text(self.t(label_key).to_string())
                .size(11)
                .style(muted_style),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let mut col = column![label_row]
            .spacing(4)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        for path in items.iter().take(settings_store::RECENT_MAX) {
            let exists = std::path::Path::new(path).exists();
            let display = path.clone();
            let path_for_msg = path.clone();
            // Missing entries used to be `on_press`-less (silent no-op),
            // which was confusing — the chip looked clickable but didn't
            // do anything. Route clicks on a stale chip to a banner so
            // the user actually learns *why* nothing happened. The
            // file/folder split decides which i18n key fires; we pick it
            // up at click time, not now, so the kind enum stays out of
            // the chip closure.
            let on_press = if exists {
                on_pick(path_for_msg)
            } else {
                Message::NoticeRecentMissing(is_file_picker)
            };
            let btn = button(
                text(display)
                    .size(11)
                    .style(muted_style)
                    .width(Length::Fill)
                    .center()
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .width(Length::Fill)
            .padding([4, 10])
            .style(|_t: &Theme, _s| button::Style {
                background: None,
                ..Default::default()
            })
            .on_press(on_press);
            col = col.push(btn);
        }
        col.into()
    }
}

/// True for the localized "Start" / "Dump" labels — the primary-action button
/// shown only on a wizard's confirm/start screen (intermediate steps use
/// "Next" / "Scan"). Drives the red Cancel button in the footer helpers.
fn is_start_label(label: &str) -> bool {
    label == ltbox_core::i18n::tr("btn_start").as_str()
        || label == ltbox_core::i18n::tr("btn_dump").as_str()
}

fn wizard_nav<'a>(
    can_back: bool,
    next_label: &str,
    can_next: bool,
    back_label: &str,
) -> Element<'a, Message> {
    let mut r = row![].spacing(8).padding([12, 24]);

    if can_back {
        r = r.push(
            button(text(back_label.to_string()).size(13))
                .on_press(Message::Root(RootMsg::RootBack))
                .padding([10, 20])
                .style(md_text_btn_style),
        );
    }

    r = r.push(Space::new().width(Length::Fill));

    // Red "Cancel" on the confirm/start step → StartOver (see
    // `wizard_nav_generic` for the M3 placement rationale).
    if is_start_label(next_label) {
        r = r.push(
            button(text(ltbox_core::i18n::tr("btn_cancel")).size(13))
                .on_press(Message::StartOver)
                .padding([10, 20])
                .style(md_error_text_btn_style),
        );
    }

    let next_btn = button(text(next_label.to_string()).size(13))
        .padding([10, 24])
        .style(md_filled_btn_style);

    r = r.push(if can_next {
        next_btn.on_press(Message::Root(RootMsg::RootNext))
    } else {
        next_btn
    });

    // Top-edge divider only — bottom + sides come from the window
    // outline / sidebar rule, so a 4-side border here would render
    // the top + left as a double line.
    column![
        widget::rule::horizontal(1).style(shell_rule_style),
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
    ]
    .into()
}

/// Linear mix of two colors by `t` ∈ [0, 1].
fn blend(base: iced::Color, overlay: iced::Color, t: f32) -> iced::Color {
    let t = t.clamp(0.0, 1.0);
    iced::Color {
        r: base.r * (1.0 - t) + overlay.r * t,
        g: base.g * (1.0 - t) + overlay.g * t,
        b: base.b * (1.0 - t) + overlay.b * t,
        a: base.a,
    }
}

// =========================================================================
// Reusable widgets
// =========================================================================

/// Section header. Renders the label when `expanded` is `true`,
/// otherwise an invisible spacer at the same fixed height — keeps
/// the nav column from re-flowing vertically as the sidebar tween
/// crosses its midpoint.
const SEC_HDR_HEIGHT: f32 = 36.0;

/// Cubic ease-out curve `f(t) = 1 - (1 - t)^3`, mapped to `[0, 1]`.
/// Used by the sidebar tween so labels fade in faster early and
/// settle smoothly near the spring's resting point.
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Pinned nav button height — matches the expanded label form so
/// the sidebar tween's mid-frame swap between icon-only and
/// label content doesn't push every row vertically.
const NAV_BTN_HEIGHT: f32 = 38.0;

/// Collapsed sidebar rail width (icon-only). The main row reserves
/// exactly this much space so the content area never reflows when the
/// sidebar tweens — the expanded form floats over content via a
/// `Stack` overlay.
const SIDEBAR_RAIL_WIDTH: f32 = 64.0;
const SIDEBAR_EXPANDED_WIDTH: f32 = 210.0;

fn nav_btn<'a>(
    view: View,
    label: &str,
    active: bool,
    enabled: bool,
    label_alpha: f32,
) -> Element<'a, Message> {
    // M3 active indicator pill: a 32x28 `secondary_container` chip
    // wraps the icon when the item is selected. Replaces the older
    // "tint the whole button" treatment so the active marker stays
    // anchored to the icon and the label stays readable in its
    // standard `on_surface` color.
    let icon = lucide_icon(view.nav_icon(), 18.0, move |t: &Theme| {
        let p = pal_of(t);
        if !enabled {
            with_alpha(p.on_surface, 0.38)
        } else if active {
            p.on_secondary_container
        } else {
            p.on_surface_variant
        }
    });
    let icon_pill: Element<'a, Message> = container(icon)
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(28.0))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(move |t: &Theme| {
            if active && enabled {
                iced::widget::container::Style {
                    background: Some(pal_of(t).secondary_container.into()),
                    border: iced::Border {
                        radius: theme::shape::FULL.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            } else {
                iced::widget::container::Style::default()
            }
        })
        .into();

    // Single base layout in both modes: icon left-anchored + optional
    // label. Keeping the icon's horizontal position constant across
    // modes means it does not jump from "centered in 64 px shell"
    // to "left-padded next to label" the moment the label mounts.
    // Outer padding shrinks from 22 → 15 (= 22 - (32-18)/2) so the
    // pill's geometric center sits at the same x as the bare icon
    // did before, avoiding a horizontal shift the moment a row
    // becomes active.
    let mut inner = iced::widget::row![icon_pill]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if label_alpha > 0.0 {
        // Resolve the base text color (hover / disabled apply via the
        // button style below; here we just fade the label in along
        // the spring), then re-apply alpha so the glyph fades in step
        // with the sidebar width tween. M3 nav rail uses `on_surface`
        // for both active and inactive labels in the expanded form —
        // the pill carries the emphasis, the label stays uniform.
        let alpha = label_alpha;
        let base_label_color = move |t: &Theme| -> iced::Color {
            let p = pal_of(t);
            if !enabled {
                with_alpha(p.on_surface, 0.38)
            } else {
                p.on_surface
            }
        };
        inner = inner.push(
            text(label.to_string())
                .size(13)
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                // Forbid wrapping: during the sidebar spring there is
                // a brief window where the panel is wide enough to
                // mount the label but too narrow for long glyphs to
                // fit on one line. Wrapping into 2 rows mid-tween then
                // collapsing back to 1 row reads as a jank flicker.
                // No-wrap lets the text overflow under the panel's
                // clip rect instead — invisible until width settles.
                .wrapping(iced::widget::text::Wrapping::None)
                .style(move |t: &Theme| iced::widget::text::Style {
                    color: Some(with_alpha(base_label_color(t), alpha)),
                }),
        );
    }
    let content: Element<'a, Message> = container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(iced::Alignment::Center)
        .into();

    // Outer padding 15px on each side: the 32-wide pill centered in
    // a 62-wide content box places the pill (and the 18px icon inside)
    // at the same on-screen x as the 18px icon at padding 22 used to.
    // Vertical padding stays 0; height is fixed by NAV_BTN_HEIGHT.
    let btn = button(content)
        .padding([0, 15])
        .width(Length::Fill)
        .height(Length::Fixed(NAV_BTN_HEIGHT))
        .style(move |t: &Theme, status| {
            let p = pal_of(t);
            if !enabled {
                return button::Style {
                    background: None,
                    text_color: with_alpha(p.on_surface, 0.38),
                    ..Default::default()
                };
            }
            // State layers per M3: hover 8%, pressed 12%. The spec
            // does NOT add a persistent row tint to active items —
            // the indicator pill (secondary_container chip around the
            // icon) is the sole "selected" marker, and state layers
            // only stack on top during interaction. Idle active items
            // therefore get no row background.
            let bg = theme::state_layer_bg(status, p.on_surface).map(|c| c.into());
            button::Style {
                background: bg,
                text_color: if active {
                    p.on_surface
                } else {
                    p.on_surface_variant
                },
                ..Default::default()
            }
        });
    if enabled {
        btn.on_press(Message::Navigate(view)).into()
    } else {
        btn.into()
    }
}

// Device portrait handles — built once, cloned each render.
// Unknown models fall through to `GENERIC_TABLET_SVG_HANDLE`.
static TB320FC_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb320fc.png").as_slice(),
        )
    });
static TB321FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb321fu.png").as_slice(),
        )
    });
static TB322FC_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb322fc.png").as_slice(),
        )
    });
static TB323FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb323fu.png").as_slice(),
        )
    });
static TB520FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb520fu.png").as_slice(),
        )
    });
static TB710FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb710fu.png").as_slice(),
        )
    });
static GENERIC_TABLET_SVG_HANDLE: std::sync::LazyLock<iced::widget::svg::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::svg::Handle::from_memory(
            include_bytes!("../assets/devices/generic_tablet.svg").as_slice(),
        )
    });

/// Asset for the Dashboard portrait slot.
enum DevicePortrait {
    Png(iced::widget::image::Handle),
    Svg(iced::widget::svg::Handle),
}

fn device_portrait(model: &str) -> DevicePortrait {
    match model.to_uppercase().as_str() {
        "TB320FC" => DevicePortrait::Png(TB320FC_HANDLE.clone()),
        "TB321FU" => DevicePortrait::Png(TB321FU_HANDLE.clone()),
        "TB322FC" => DevicePortrait::Png(TB322FC_HANDLE.clone()),
        "TB323FU" => DevicePortrait::Png(TB323FU_HANDLE.clone()),
        "TB520FU" => DevicePortrait::Png(TB520FU_HANDLE.clone()),
        "TB710FU" => DevicePortrait::Png(TB710FU_HANDLE.clone()),
        _ => DevicePortrait::Svg(GENERIC_TABLET_SVG_HANDLE.clone()),
    }
}

const WIZARD_CARD_HEIGHT: f32 = 180.0;

/// Fixed sub-row height (~2 lines at size 11) so cards line up across
/// translations.
const SUB_ROW_HEIGHT: f32 = 32.0;

fn wizard_nav_generic<'a>(
    can_back: bool,
    next_label: &str,
    can_next: bool,
    back_label: &str,
    back_msg: Message,
    next_msg: Message,
) -> Element<'a, Message> {
    let mut r = row![].spacing(8).padding([12, 24]);
    if can_back {
        r = r.push(
            button(text(back_label.to_string()).size(13))
                .on_press(back_msg)
                .padding([10, 20])
                .style(md_text_btn_style),
        );
    }
    r = r.push(Space::new().width(Length::Fill));
    // Red "Cancel" on the confirm/start step → StartOver, returning the menu
    // to its beginning. M3: the cancel/start decision pair sits at the
    // trailing edge, navigation (Back) at the leading edge; one filled button
    // (Start), destructive action in the error color.
    if is_start_label(next_label) {
        r = r.push(
            button(text(ltbox_core::i18n::tr("btn_cancel")).size(13))
                .on_press(Message::StartOver)
                .padding([10, 20])
                .style(md_error_text_btn_style),
        );
    }
    let next_btn = button(text(next_label.to_string()).size(13))
        .padding([10, 24])
        .style(md_filled_btn_style);
    r = r.push(if can_next {
        next_btn.on_press(next_msg)
    } else {
        next_btn
    });
    // Top-edge divider only — bottom + sides come from the window
    // outline / sidebar rule.
    column![
        widget::rule::horizontal(1).style(shell_rule_style),
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lang_jsons_parse_and_share_keys() {
        let en = Translations::load(Language::En);
        for lang in [Language::Ko, Language::Zh, Language::Ru, Language::Ja] {
            let tr = Translations::load(lang);
            for key in en.fallback.keys() {
                assert!(
                    tr.primary.contains_key(key),
                    "lang {:?} missing key {}",
                    lang,
                    key
                );
            }
        }
    }

    #[test]
    fn flash_sidebar_uses_sidebar_specific_label_key() {
        assert_eq!(View::Flash.sidebar_label_key(), "nav_flash_sidebar");
        assert_eq!(View::Flash.label_key(), "nav_flash");
        assert_eq!(
            View::Dashboard.sidebar_label_key(),
            View::Dashboard.label_key()
        );
    }

    #[test]
    fn unknown_key_falls_back_to_itself() {
        let t = Translations::load(Language::En);
        assert_eq!(t.t("__no_such_key__"), "__no_such_key__");
    }

    #[test]
    fn non_empty_prop_treats_blank_as_absent() {
        assert_eq!(non_empty_prop(""), None);
        assert_eq!(non_empty_prop("   \n\t"), None);
        assert_eq!(
            non_empty_prop("  Tab Plus 14  \n"),
            Some("Tab Plus 14".to_string())
        );
    }

    #[test]
    fn select_device_name_falls_back_through_lgsi_props() {
        use std::collections::HashMap;
        let pick = |map: HashMap<&'static str, &'static str>| {
            select_device_name(|p| map.get(p).copied().unwrap_or("").to_string())
        };

        // Primary populated wins.
        assert_eq!(
            pick(HashMap::from([(
                "ro.vendor.config.lgsi.en.market_name",
                "Tab Plus"
            )])),
            "Tab Plus"
        );
        // Primary whitespace-only -> vendor LGSI market name.
        assert_eq!(
            pick(HashMap::from([
                ("ro.vendor.config.lgsi.en.market_name", "   "),
                ("ro.vendor.config.lgsi.market_name", "Tab Vendor"),
            ])),
            "Tab Vendor"
        );
        // -> system LGSI market name.
        assert_eq!(
            pick(HashMap::from([(
                "ro.config.lgsi.market_name",
                "Tab System"
            )])),
            "Tab System"
        );
        // -> legacy kirby_en final fallback (preserved).
        assert_eq!(
            pick(HashMap::from([("ro.vendor.config.lgsi.kirby_en", "Kirby")])),
            "Kirby"
        );
        // Nothing populated -> empty string.
        assert_eq!(pick(HashMap::new()), "");
    }

    #[test]
    fn efisp_asset_suffix_picks_prc_or_row() {
        assert_eq!(efisp_asset_suffix(true, false), "_prc.efi");
        assert_eq!(efisp_asset_suffix(false, false), "_row.efi");
        // Anti-rollback downgrade requests the `_arb` GBL (testkey root).
        assert_eq!(efisp_asset_suffix(true, true), "_prc_arb.efi");
        assert_eq!(efisp_asset_suffix(false, true), "_row_arb.efi");
    }

    #[test]
    fn efisp_is_empty_only_for_all_zero() {
        assert!(efisp_is_empty(&[]));
        assert!(efisp_is_empty(&[0u8; 4096]));
        assert!(!efisp_is_empty(&[0, 0, 1, 0]));
        let mut buf = vec![0u8; 1024];
        buf[1000] = 0xEF;
        assert!(!efisp_is_empty(&buf));
    }

    #[test]
    fn advanced_in_progress_gates_partition_table_on_edl() {
        let row = || FlashPartRow {
            lun: 4,
            label: "boot_a".into(),
            start_sector: 0,
            num_sectors: 0,
            size_bytes: 0,
            file_path: None,
            state: FlashRowState::Unchecked,
        };
        let mut app = App {
            advanced_wizard_open: AdvancedWizardOpen::FlashParts,
            connection: ConnectionStatus::Edl,
            ..App::default()
        };
        // No scanned rows yet → not preserve-worthy.
        assert!(!app.advanced_in_progress());
        // GPT table loaded + still in EDL → preserve.
        app.flash_parts.rows = vec![row()];
        assert!(app.advanced_in_progress());
        // Device left EDL → table is stale → reset.
        app.connection = ConnectionStatus::None;
        assert!(!app.advanced_in_progress());

        // Physical confirm screen preserves; DumpPhys (no confirm) + the grid
        // do not.
        let mut app = App {
            advanced_wizard_open: AdvancedWizardOpen::FlashPhys,
            ..App::default()
        };
        app.flash_phys.step = FLASH_PHYS_STEPS.len() - 2; // Confirm
        assert!(app.advanced_in_progress());
        app.flash_phys.step = 1; // Select
        assert!(!app.advanced_in_progress());
        app.advanced_wizard_open = AdvancedWizardOpen::DumpPhys;
        assert!(!app.advanced_in_progress());
        app.advanced_wizard_open = AdvancedWizardOpen::None;
        assert!(!app.advanced_in_progress());
    }

    // ---- parse_phase_marker decimal-point guard ----------------------
    //
    // Regression: downloader progress emits e.g.
    // `[dl] kernelsu.ko [████····]  45% (1.2/2.7 MB, 0.5 MB/s)`.
    // Old `parse_phase_marker` saw the `2/2` digits adjacent to the
    // slash and yanked the wizard's `current_op_step` to phase 2 (or
    // worse for `12.3/45.6 MB` which yields `3/4`). On every 5%
    // bucket the wizard raced through phases mid-download then
    // snapped back when the next real `Phase N/M` line arrived.
    // These tests pin the new decimal-point sidestep.

    #[test]
    fn phase_marker_real_phase_line_parses() {
        assert_eq!(parse_phase_marker("[Root] Phase 3/7 — Reboot"), Some(3));
        assert_eq!(parse_phase_marker("[Root] 단계 5/7 — 부트 패치"), Some(5),);
    }

    #[test]
    fn phase_marker_decimal_progress_rejected() {
        // Both sides surrounded by dots — clear float pair.
        assert_eq!(
            parse_phase_marker("[dl] kernelsu.ko 45% (12.3/45.6 MB, 0.5 MB/s)"),
            None,
        );
        // Left side decimal only (`.2` before slash).
        assert_eq!(
            parse_phase_marker("[dl] manager.apk 45% (1.2/2.7 MB)"),
            None,
        );
        // Right side decimal only (`5.` after slash digit).
        assert_eq!(parse_phase_marker("[dl] file 12/5.6 MB"), None,);
    }

    #[test]
    fn phase_marker_no_slash_returns_none() {
        assert_eq!(parse_phase_marker("[Root] Manager APK installed"), None);
        assert_eq!(parse_phase_marker("[dl] file 45%"), None);
    }

    // Wizard state-machine tests ------------------------------------------

    #[test]
    fn flash_wizard_next_back_round_trip() {
        let mut w = FlashWizard::default();
        assert_eq!(w.step, 0);
        // Can't advance without a region selected.
        assert!(!w.can_next());
        w.device_region = Some(DeviceRegion::Prc);
        assert!(w.can_next());
        w.next();
        assert_eq!(w.step, 1);
        w.back();
        assert_eq!(w.step, 0);
        // Reset wipes every field.
        w.next();
        w.reset();
        assert_eq!(w.step, 0);
        assert!(w.device_region.is_none());
    }

    #[test]
    fn confirm_step_is_the_step_before_exec() {
        // Linear (trait default): confirm = step_count - 2, exec = -1.
        let mut f = FlashWizard::default();
        let confirm = f.step_count() - 2;
        f.step = 0;
        assert!(!f.is_on_confirm_step());
        f.step = confirm;
        assert!(f.is_on_confirm_step());
        assert!(!f.is_in_exec());
        f.step = f.step_count() - 1;
        assert!(!f.is_on_confirm_step());
        assert!(f.is_in_exec());

        // SysUpdate step count flexes with rescue mode; confirm still tracks
        // step_count - 2 on both the compact and the longer rescue flow.
        let mut s = SysUpdateWizard::default();
        s.step = s.step_count() - 2; // compact: confirm = step 1
        assert!(s.is_on_confirm_step());
        s.action = Some(SysUpdateAction::Rescue);
        s.step = s.step_count() - 2; // rescue: confirm = step 2
        assert!(s.is_on_confirm_step());
        s.step = 1; // rescue folder step — not confirm
        assert!(!s.is_on_confirm_step());

        // Root is non-linear: confirm = step 6, exec = step 7.
        let mut r = RootWizard {
            step: 6,
            ..Default::default()
        };
        assert!(r.is_on_confirm_step());
        r.step = 7;
        assert!(!r.is_on_confirm_step());
        assert!(r.is_in_exec());
        r.step = 0;
        assert!(!r.is_on_confirm_step());
    }

    #[test]
    fn root_wizard_kernelsu_lkm_path() {
        let mut w = RootWizard {
            family: Some(Family::KernelSU),
            ..RootWizard::default()
        };
        w.next(); // 0 → 1 (Mode)
        assert_eq!(w.step, 1);
        w.mode = Some(RootMode::Lkm);
        w.next(); // 1 → 2 (Provider)
        assert_eq!(w.step, 2);
        w.provider = Some(Provider::KernelSU);
        w.next(); // 2 → 3 (Version)
        assert_eq!(w.step, 3);
        w.version = Some(VerChoice::Stable);
        w.next(); // Stable skips NightlySource, jumps to Confirm (5)
        assert_eq!(w.step, 5);
    }

    #[test]
    fn root_wizard_kernelsu_lkm_requires_kernel_version_before_exec() {
        let mut w = RootWizard {
            family: Some(Family::KernelSU),
            mode: Some(RootMode::Lkm),
            provider: Some(Provider::KernelSU),
            version: Some(VerChoice::Stable),
            folder_path: Some("firmware".to_string()),
            step: 6,
            ..RootWizard::default()
        };

        assert!(w.needs_ksu_lkm_kernel_version());
        w.kernel_version = Some("6.1".to_string());
        assert!(!w.needs_ksu_lkm_kernel_version());
    }

    #[test]
    fn root_wizard_magisk_skips_mode() {
        let mut w = RootWizard {
            family: Some(Family::Magisk),
            ..RootWizard::default()
        };
        w.next(); // 0 → 2 directly (Magisk has no modes)
        assert_eq!(w.step, 2);
    }

    #[test]
    fn image_info_wizard_runs_after_multi_image_selection() {
        let mut w = AdvWizard::default();
        w.open(AdvAction::ImageInfo);

        assert_eq!(w.steps(), &["adv_step_source", "adv_step_info"]);
        assert!(!w.is_confirm_step());
        assert!(!w.can_next());

        w.file_paths = vec!["boot.img".into(), "vbmeta.img".into()];
        assert!(w.can_next());
        w.next();
        assert_eq!(w.step, w.exec_step());
    }

    #[test]
    fn advanced_menu_taxonomy_matches_avb_image_reclass() {
        let section = |key: &str| {
            ADV_SECTIONS
                .iter()
                .find(|section| section.title_key == key)
                .expect("section exists")
                .items
        };

        assert_eq!(
            section("adv_section_region_patch"),
            &[AdvAction::RegionConvert, AdvAction::PatchDevinfo]
        );
        assert!(
            ADV_SECTIONS
                .iter()
                .all(|section| section.title_key != "adv_section_country_code")
        );
        assert_eq!(
            section("adv_section_rollback"),
            &[
                AdvAction::ImageInfo,
                AdvAction::DetectArb,
                AdvAction::PatchArb,
                AdvAction::RebuildVbmeta,
            ]
        );
        assert_eq!(
            section("adv_section_edl_ops"),
            &[
                AdvAction::ConvertXml,
                AdvAction::DumpPartitions,
                AdvAction::FlashPartitions,
                AdvAction::DumpPhysical,
                AdvAction::FlashPhysical,
            ]
        );
    }

    fn assert_template_call_replaces(source: &str, key: &str, placeholders: &[&str]) {
        // Whitespace-strip the whole source so rustfmt line-wrapping (which can
        // split a tr_args! call across lines) doesn't hide it. Accept either
        // substitution form: the manual tr(key) followed by a replace chain, or
        // the tr_args! macro (which expands to the same chain). Both guarantee
        // the placeholder is filled rather than shipped literally.
        let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        let tr_args_needle = format!("tr_args!(\"{key}\"");
        if let Some(pos) = compact.find(&tr_args_needle) {
            let window = &compact[pos..(pos + 2_000).min(compact.len())];
            for placeholder in placeholders {
                assert!(
                    window.contains(&format!("{placeholder}=")),
                    "{key} (tr_args!) must pass {placeholder}"
                );
            }
            return;
        }
        let needle = format!("tr(\"{key}\")");
        let pos = compact.find(&needle).expect("template key must be used");
        let window = &compact[pos..(pos + 2_000).min(compact.len())];
        for placeholder in placeholders {
            assert!(
                window.contains(&format!(".replace(\"{{{placeholder}}}\"")),
                "{key} must replace {{{placeholder}}} near its log call"
            );
        }
    }

    #[test]
    fn high_risk_log_templates_replace_visible_placeholders() {
        // Concatenate the GUI sources that carry high-risk log templates;
        // some live in main.rs, others in the extracted worker modules.
        let gui_src = concat!(
            include_str!("main.rs"),
            include_str!("workers/transfer.rs"),
            include_str!("workers/flash.rs"),
        );
        let edl_rs = include_str!("../../ltbox-device/src/edl.rs");

        assert_template_call_replaces(
            edl_rs,
            "log_edl_flash_program_cmd",
            &["label", "image", "lun", "start", "sectors"],
        );
        assert_template_call_replaces(
            gui_src,
            "live_country_dump_partition",
            &["label", "lun", "start", "sectors"],
        );
        assert_template_call_replaces(gui_src, "live_dump_phys_dumping_lun", &["lun", "path"]);
        assert_template_call_replaces(gui_src, "live_dump_phys_lun_failed", &["lun", "error"]);
    }

    #[test]
    fn country_popup_selection_uses_opening_flow_context() {
        let app = App {
            adv_needs_country: true,
            adv_wizard: AdvWizard {
                country: Some("KR".to_string()),
                ..AdvWizard::default()
            },
            wf_config: WorkflowConfig {
                country_action: CountryAction::Set("CN".to_string()),
                ..WorkflowConfig::default()
            },
            ..App::default()
        };
        assert_eq!(app.country_popup_selected_code(), Some("KR"));

        let app = App {
            adv_needs_country: false,
            adv_wizard: AdvWizard {
                country: Some("KR".to_string()),
                ..AdvWizard::default()
            },
            wf_config: WorkflowConfig {
                country_action: CountryAction::Set("CN".to_string()),
                ..WorkflowConfig::default()
            },
            ..App::default()
        };
        assert_eq!(app.country_popup_selected_code(), Some("CN"));
    }

    #[test]
    fn sysupdate_wizard_gate_requires_action() {
        let mut w = SysUpdateWizard::default();
        assert!(!w.can_next());
        w.action = Some(SysUpdateAction::Disable);
        assert!(w.can_next());
        w.next();
        assert_eq!(w.step, 1);
        w.next();
        w.next();
        // Caps at len - 1.
        assert_eq!(w.step, SYSUPDATE_STEPS_COMPACT.len() - 1);
    }

    #[test]
    fn flash_parts_wizard_requires_selection() {
        let mut w = FlashPartsWizard::default();
        assert!(!w.can_next());
        w.loader_path = Some("/tmp/xbl.melf".to_string());
        // Step 0 only needs a loader picked.
        assert!(w.can_next());
        w.next();
        assert_eq!(w.step, 1);
        // Step 1: need at least one row with a resolvable action.
        w.rows.push(FlashPartRow {
            lun: 0,
            label: "boot_a".into(),
            start_sector: 0,
            num_sectors: 8192,
            size_bytes: 4 * 1024 * 1024,
            file_path: None,
            state: FlashRowState::Unchecked,
        });
        assert!(!w.can_next()); // Unchecked doesn't count
        w.rows[0].state = FlashRowState::Flash;
        assert!(!w.can_next()); // Flash w/o file still invalid
        w.rows[0].file_path = Some("/tmp/boot.img".into());
        assert!(w.can_next());
        // Erase alone is enough — no file required.
        w.rows[0].state = FlashRowState::Erase;
        w.rows[0].file_path = None;
        assert!(w.can_next());
    }

    #[test]
    fn busy_progress_dialog_shows_only_without_inline_log_surface() {
        let mut app = App {
            busy: true,
            busy_view: Some(View::Reboot),
            current_view: View::Reboot,
            ..App::default()
        };

        assert!(app.should_show_busy_progress_dialog());

        app.current_view = View::Dashboard;
        assert!(!app.should_show_busy_progress_dialog());

        app.current_view = View::Advanced;
        app.advanced_wizard_open = AdvancedWizardOpen::FlashParts;
        app.flash_parts.step = 0;
        assert!(app.should_show_busy_progress_dialog());

        app.flash_parts.step = 3;
        assert!(!app.should_show_busy_progress_dialog());

        app.advanced_wizard_open = AdvancedWizardOpen::DumpParts;
        app.dump_parts.step = 0;
        assert!(app.should_show_busy_progress_dialog());

        app.dump_parts.step = 2;
        assert!(!app.should_show_busy_progress_dialog());

        app.advanced_wizard_open = AdvancedWizardOpen::None;
        app.current_view = View::Flash;
        app.flash.step = FLASH_STEPS.len() - 1;
        assert!(!app.should_show_busy_progress_dialog());
    }

    #[test]
    fn busy_operation_label_names_advanced_subtask() {
        let mut app = App {
            busy: true,
            busy_view: Some(View::Advanced),
            current_view: View::Advanced,
            ..App::default()
        };

        app.adv_wizard.action = Some(AdvAction::PatchDevinfo);
        assert_eq!(
            app.busy_operation_label(),
            app.t(AdvAction::PatchDevinfo.label_key()).to_string()
        );

        app.advanced_wizard_open = AdvancedWizardOpen::FlashParts;
        assert_eq!(
            app.busy_operation_label(),
            app.t(AdvAction::FlashPartitions.label_key()).to_string()
        );

        app.busy_view = Some(View::Reboot);
        assert_eq!(app.busy_operation_label(), app.t("nav_reboot").to_string());
    }

    #[test]
    fn loader_file_check_is_extension_based() {
        assert!(is_loader_file(std::path::Path::new("xbl_anything.melf")));
        assert!(is_loader_file(std::path::Path::new("firehose_loader.MBN")));
        assert!(is_loader_file(std::path::Path::new("prog.elf")));
        assert!(!is_loader_file(std::path::Path::new("xbl_s_devprg_ns.bin")));
    }

    #[test]
    fn edl_entry_action_uses_adb_from_fastboot() {
        assert_eq!(
            edl_entry_action(ConnectionStatus::Fastboot),
            EdlEntryAction::FastbootRebootThenAdb
        );
    }

    #[test]
    fn edl_entry_action_waits_manual_without_usable_adb() {
        assert_eq!(
            edl_entry_action(ConnectionStatus::AdbUnauthorized),
            EdlEntryAction::ManualWait
        );
    }

    #[test]
    fn country_patch_progress_requires_all_expected() {
        let mut progress = CountryPatchProgress::new(&["devinfo", "persist"]);
        progress.mark_flashed("devinfo");

        let err = progress.finish().expect_err("persist must be required");
        assert!(err.contains("persist"));
    }

    #[test]
    fn country_patch_progress_oemowninfo_expected() {
        // TB320FC / TB323FU patch oemowninfo instead of devinfo.
        let mut progress = CountryPatchProgress::new(&["oemowninfo", "persist"]);
        progress.mark_flashed("oemowninfo");
        progress.mark_flashed("persist");
        assert!(progress.finish().is_ok());
    }

    #[test]
    fn country_patch_progress_surfaces_partition_failures() {
        let mut progress = CountryPatchProgress::new(&["devinfo", "persist"]);
        progress.mark_flashed("devinfo");
        progress.mark_failed("persist", "no known country code");

        let err = progress
            .finish()
            .expect_err("recorded persist failure must fail workflow");
        assert!(err.contains("persist: no known country code"));
    }
}
