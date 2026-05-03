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
mod pickers;
mod settings_store;
mod stdout_tap;
mod theme;
mod theme_detect;

use std::collections::HashMap;

use ltbox_core::live;

use iced::widget::{self, Space, button, column, container, row, scrollable, text};
use iced::{Element, Length, Subscription, Task, Theme};
use iced_aw::widget::Spinner;

use theme::{Palette, palette, with_alpha};

/// Palette lookup from `iced` style closures that only have `&Theme`.
fn pal_of(t: &Theme) -> &'static Palette {
    palette(is_dark(t))
}

// Shims for contexts without a `&Theme` (plain `.color(...)` calls).
// Dark-mode-critical surfaces already route through `pal_of` / `self.pal()`.
const ACCENT: iced::Color = theme::LIGHT.primary;
const LABEL: iced::Color = theme::LIGHT.outline;
const GREEN: iced::Color = theme::LIGHT.success;

/// Upper bound on `App.log_lines` — keeps memory flat over long sessions.
const LOG_MAX_LINES: usize = 500;

/// 32×32 RGBA image handle for the title-bar brand icon. Built once,
/// cheap to clone (ref-counted).
static TITLE_BAR_ICON_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        let bytes: &'static [u8] = include_bytes!("../assets/icon_32.bin");
        iced::widget::image::Handle::from_rgba(32, 32, bytes.to_vec())
    });

/// `on_surface_variant` — secondary labels / descriptions.
fn muted_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).on_surface_variant),
    }
}

/// `outline` — captions and sidebar section headers.
fn label_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).outline),
    }
}

/// `on_surface` — primary foreground on surface containers.
fn on_surface_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).on_surface),
    }
}

/// `primary` — accent emphasis (active labels, live-op markers).
fn accent_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).primary),
    }
}

/// `success` — completion markers and "ok" status.
#[allow(dead_code)]
fn success_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).success),
    }
}

/// `warning` — destructive-action callouts (e.g. full-flash confirm
/// step). Kept distinct from `error_style` so it reads as "heads up, not
/// a failure".
fn warning_style(t: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(pal_of(t).warning),
    }
}

/// Reverse-DNS app id. Becomes Wayland `app_id` / X11 `WM_CLASS` via
/// iced `Settings::id`; matches the shipped `.desktop`'s
/// `StartupWMClass=` so the window binds to the launcher entry.
const APP_ID: &str = "io.github.miner7222.LTBox";

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
    let window_settings = iced::window::Settings {
        size: iced::Size::new(920.0, 620.0),
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

fn is_dark(t: &Theme) -> bool {
    t.palette().background.r < 0.5
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
    fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::System => icon::reboot_system(),
            Self::Recovery => icon::reboot_recovery(),
            Self::Bootloader => icon::reboot_bootloader(),
            Self::Edl => icon::reboot_edl(),
        };
        lucide_primary(glyph, 32.0)
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
    fn icon(self) -> Element<'static, Message> {
        // Kept as bundled SVG assets — these are per-brand logos, not
        // monochrome glyphs, so Lucide's icon set doesn't cover them.
        let bytes: &'static [u8] = match self {
            Self::Magisk => include_bytes!("../assets/icons/magisk.svg"),
            Self::KernelSU => include_bytes!("../assets/icons/kernelsu.svg"),
            Self::APatch => include_bytes!("../assets/icons/apatch.svg"),
        };
        svg_icon(bytes, 72.0)
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
    fn icon(self) -> Element<'static, Message> {
        // Provider brand logos — kept as bespoke SVG, not Lucide.
        let bytes: &'static [u8] = match self {
            Self::Magisk => include_bytes!("../assets/icons/magisk.svg"),
            Self::MagiskForks => include_bytes!("../assets/icons/magisk_forks.svg"),
            Self::KernelSU => include_bytes!("../assets/icons/kernelsu.svg"),
            Self::KernelSUNext => include_bytes!("../assets/icons/kernelsu_next.svg"),
            Self::SukiSU => include_bytes!("../assets/icons/sukisu.svg"),
            Self::ReSukiSU => include_bytes!("../assets/icons/resukisu.svg"),
            Self::APatch => include_bytes!("../assets/icons/apatch.svg"),
            Self::FolkPatch => include_bytes!("../assets/icons/folkpatch.svg"),
        };
        svg_icon(bytes, 72.0)
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
    fn icon(self) -> Element<'static, Message> {
        // Lucide chip/layers glyphs in place of the old bespoke SVGs.
        let glyph = match self {
            Self::Lkm => icon::root_lkm(),
            Self::Gki => icon::root_gki(),
        };
        lucide_primary(glyph, 57.6)
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
    fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::Stable => icon::ver_stable(),
            Self::Nightly => icon::ver_nightly(),
        };
        lucide_primary(glyph, 57.6)
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
    fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::AutoDetect => icon::nightly_auto(),
            Self::ManualInput => icon::nightly_manual(),
        };
        lucide_primary(glyph, 57.6)
    }
}

/// Linear-step wizard contract. Wizards whose `next` / `back` simply
/// walk a 0..step_count range share `reset` / `next` / `back` /
/// `is_in_exec` via this trait's default impls; only `step`,
/// `step_mut`, `step_count`, and `can_next` need per-impl bodies.
///
/// Not implemented for `RootWizard` because its non-linear step
/// numbering (steps skip around depending on family/mode) requires
/// custom navigation logic.
trait Wizard: Default {
    fn step(&self) -> usize;
    fn step_mut(&mut self) -> &mut usize;
    fn step_count(&self) -> usize;
    fn can_next(&self) -> bool;

    fn reset(&mut self) {
        *self = Self::default();
    }
    fn next(&mut self) {
        if self.step() < self.step_count() - 1 {
            *self.step_mut() += 1;
        }
    }
    fn back(&mut self) {
        if self.step() > 0 {
            *self.step_mut() -= 1;
        }
    }
    fn is_in_exec(&self) -> bool {
        self.step() == self.step_count() - 1
    }
}

// Internal steps: 0=Family, 1=Mode, 2=Provider, 3=Version,
// 4=NightlySource, 5=Folder, 6=Confirm, 7=Flash, 8=APatch KPM.
// Mode auto-skips for non-KSU. GKI: steps 3/4 collapse into a kernel
// zip picker at 2. MagiskForks: skip Version, APK picker at 3. Nightly
// inserts 4 between Version and Folder.
#[derive(Default)]
struct RootWizard {
    step: usize,
    family: Option<Family>,
    mode: Option<RootMode>,
    provider: Option<Provider>,
    version: Option<VerChoice>,
    nightly_source: Option<NightlySource>,
    file_path: Option<String>, // GKI zip, MagiskForks APK, or manual nightly
    folder_path: Option<String>, // Firmware folder (loader + optional testkey)
    /// APatch: `.kpm` modules to embed. Multi-select + per-entry remove.
    kpm_paths: Vec<String>,
    /// APatch superkey. Secret — never echoed in confirm or any log.
    superkey: Option<String>,
    superkey_popup_open: bool,
    /// Buffer for the currently visible field in the superkey popup;
    /// reset between the first-entry and re-entry stages.
    superkey_buffer: String,
    /// First-entry value held while the popup waits for the user to
    /// re-enter their key on the second stage. `None` → still on the
    /// first-entry stage; `Some(v)` → on the verification stage and
    /// `superkey_buffer` will be compared against `v` on Confirm.
    superkey_first_entry: Option<String>,
    /// Nightly ManualInput: committed workflow run ID (1..=12 digits).
    /// Only meaningful when `nightly_source == Some(ManualInput)`.
    run_id: Option<String>,
    run_id_popup_open: bool,
    run_id_buffer: String,
    /// KernelSU LKM: normalized `major.minor` kernel version from ADB or manual popup.
    kernel_version: Option<String>,
    kernel_version_popup_open: bool,
    kernel_version_buffer: String,
}

const ROOT_STEPS: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_provider",
    "root_step_version",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_NIGHTLY: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_provider",
    "root_step_version",
    "root_step_source",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_GKI: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_kernel",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_NOMODE: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_NOMODE_NIGHTLY: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_source",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_FORKS: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_apk",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_APATCH: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_kpm",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
const ROOT_STEPS_APATCH_NIGHTLY: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_source",
    "root_step_kpm",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];

impl RootWizard {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// True on the final (flash/exec) step. Used to skip wizard reset
    /// when the user sidebar-bounces mid-operation.
    fn is_in_exec(&self) -> bool {
        self.step == 7
    }

    fn is_gki(&self) -> bool {
        self.mode == Some(RootMode::Gki)
    }
    fn is_forks(&self) -> bool {
        self.provider == Some(Provider::MagiskForks)
    }
    fn is_nightly(&self) -> bool {
        self.version == Some(VerChoice::Nightly)
    }
    fn is_apatch(&self) -> bool {
        self.family == Some(Family::APatch)
    }

    fn is_ksu_lkm(&self) -> bool {
        self.family == Some(Family::KernelSU) && self.mode == Some(RootMode::Lkm)
    }

    fn needs_ksu_lkm_kernel_version(&self) -> bool {
        self.is_ksu_lkm() && self.kernel_version.is_none()
    }

    fn active_steps(&self) -> &'static [&'static str] {
        if self.is_gki() {
            return ROOT_STEPS_GKI;
        }
        let has_modes = self.family.map(|f| f.has_modes()).unwrap_or(false);
        if self.is_forks() {
            return ROOT_STEPS_FORKS;
        }
        if self.is_apatch() {
            // APatch route: Version → KPM → Folder. Superkey popup
            // lives on the KPM→Folder edge, not as its own step.
            return if self.is_nightly() {
                ROOT_STEPS_APATCH_NIGHTLY
            } else {
                ROOT_STEPS_APATCH
            };
        }
        match (has_modes, self.is_nightly()) {
            (true, true) => ROOT_STEPS_NIGHTLY,
            (true, false) => ROOT_STEPS,
            (false, true) => ROOT_STEPS_NOMODE_NIGHTLY,
            (false, false) => ROOT_STEPS_NOMODE,
        }
    }

    fn display_step(&self) -> usize {
        // Map internal step index into the position within the active
        // route's label array. Comments at each branch show the mapping.
        let has_modes = self.family.map(|f| f.has_modes()).unwrap_or(false);
        if self.is_gki() {
            // 0,1,2,5,6,7 → 0..5
            return match self.step {
                0 => 0,
                1 => 1,
                2 => 2,
                5 => 3,
                6 => 4,
                7 => 5,
                _ => self.step,
            };
        }
        if self.is_forks() {
            // 0,2,3,5,6,7 → 0..5
            return match self.step {
                0 => 0,
                2 => 1,
                3 => 2,
                5 => 3,
                6 => 4,
                7 => 5,
                _ => self.step,
            };
        }
        if self.is_apatch() {
            // Stable: 0,2,3,8,5,6,7 → 0..6. Nightly: add 4 → 0..7.
            if self.is_nightly() {
                return match self.step {
                    0 => 0,
                    2 => 1,
                    3 => 2,
                    4 => 3,
                    8 => 4,
                    5 => 5,
                    6 => 6,
                    7 => 7,
                    _ => self.step,
                };
            }
            return match self.step {
                0 => 0,
                2 => 1,
                3 => 2,
                8 => 3,
                5 => 4,
                6 => 5,
                7 => 6,
                _ => self.step,
            };
        }
        if !has_modes {
            if self.is_nightly() {
                // 0,2,3,4,5,6,7 → 0..6
                return match self.step {
                    0 => 0,
                    2 => 1,
                    3 => 2,
                    4 => 3,
                    5 => 4,
                    6 => 5,
                    7 => 6,
                    _ => self.step,
                };
            }
            // 0,2,3,5,6,7 → 0..5
            return match self.step {
                0 => 0,
                2 => 1,
                3 => 2,
                5 => 3,
                6 => 4,
                7 => 5,
                _ => self.step,
            };
        }
        if self.is_nightly() {
            self.step
        } else {
            // 0,1,2,3,5,6,7 → 0..6
            match self.step {
                5 => 4,
                6 => 5,
                7 => 6,
                s => s,
            }
        }
    }

    fn next(&mut self) {
        match self.step {
            0 => {
                if let Some(f) = self.family
                    && !f.has_modes()
                {
                    self.mode = None;
                    self.step = 2;
                    return;
                }
                self.step = 1;
            }
            1 => self.step = 2,
            2 => {
                if self.is_gki() {
                    self.step = 5;
                    return;
                }
                self.step = 3;
            }
            3 => {
                if self.is_forks() {
                    self.step = 5;
                    return;
                }
                if self.is_nightly() {
                    self.step = 4;
                    return;
                }
                if self.is_apatch() {
                    self.step = 8;
                    return;
                }
                self.step = 5;
            }
            4 => {
                if self.is_apatch() {
                    self.step = 8;
                    return;
                }
                self.step = 5;
            }
            // Exit gated by superkey popup — caller sets step = 5 on confirm.
            8 => self.step = 5,
            5 => self.step = 6,
            6 => self.step = 7,
            _ => {}
        }
    }

    fn back(&mut self) {
        match self.step {
            1 => self.step = 0,
            2 => {
                if let Some(f) = self.family
                    && !f.has_modes()
                {
                    self.step = 0;
                    return;
                }
                self.step = 1;
            }
            3 => self.step = 2,
            4 => self.step = 3,
            5 => {
                // Folder → whichever sub-step populated the source.
                if self.is_gki() {
                    self.step = 2;
                    return;
                }
                if self.is_forks() {
                    self.step = 3;
                    return;
                }
                if self.is_apatch() {
                    self.step = 8;
                    return;
                }
                if self.is_nightly() {
                    self.step = 4;
                    return;
                }
                self.step = 3;
            }
            6 => self.step = 5,
            7 => self.step = 6,
            8 => {
                self.step = if self.is_nightly() { 4 } else { 3 };
            }
            _ => {}
        }
    }

    fn can_next(&self) -> bool {
        match self.step {
            0 => self.family.is_some(),
            1 => self.mode.is_some(),
            2 => {
                if self.is_gki() {
                    self.file_path.is_some()
                } else {
                    self.provider.is_some()
                }
            }
            3 => {
                if self.is_forks() {
                    self.file_path.is_some()
                } else {
                    self.version.is_some()
                }
            }
            4 => match self.nightly_source {
                // ManualInput also needs the popup's run ID committed.
                Some(NightlySource::AutoDetect) => true,
                Some(NightlySource::ManualInput) => {
                    self.run_id.as_deref().is_some_and(|s| !s.is_empty())
                }
                None => false,
            },
            5 => self.folder_path.is_some(),
            6 => true,
            // KPM embedding is optional — the actual gate is the
            // superkey popup on Next.
            8 => true,
            _ => false,
        }
    }
}

// =========================================================================
// Messages
// =========================================================================

// =========================================================================
// Settings state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    En,
    Ko,
    Zh,
    Ru,
}
impl Language {
    /// Name in its own script — locale-neutral.
    fn label(&self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ko => "한국어",
            Self::Zh => "中文",
            Self::Ru => "Русский",
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ko => "ko",
            Self::Zh => "zh",
            Self::Ru => "ru",
        }
    }
    fn from_code(c: &str) -> Option<Self> {
        match c {
            "en" => Some(Self::En),
            "ko" => Some(Self::Ko),
            "zh" => Some(Self::Zh),
            "ru" => Some(Self::Ru),
            _ => None,
        }
    }
}
const LANGUAGES: &[Language] = &[Language::En, Language::Ko, Language::Zh, Language::Ru];

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
fn phase_marker<S: AsRef<str>>(phase: usize, total: usize, label: S) -> String {
    ltbox_core::i18n::tr("live_phase_marker")
        .replace("{phase}", &phase.to_string())
        .replace("{total}", &total.to_string())
        .replace("{label}", label.as_ref())
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
enum RollbackSetting {
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
enum CountryAction {
    /// Popup hasn't been answered yet.
    #[default]
    Unset,
    /// User picked "Do not change" — devinfo/persist stays put.
    Skip,
    /// User picked a concrete target code; exec runs the patch.
    Set(String),
}

impl CountryAction {
    fn target(&self) -> Option<&str> {
        match self {
            Self::Set(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn is_skipped(&self) -> bool {
        matches!(self, Self::Skip)
    }
}

/// Derived from wizard selections; reset after the op finishes.
#[derive(Debug, Clone, Default)]
struct WorkflowConfig {
    modify_region: bool,
    device_region: Option<DeviceRegion>,
    modify_rollback: RollbackSetting,
    wipe: bool,
    country_action: CountryAction,
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

// =========================================================================
// Unroot wizard state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnrootType {
    MagiskLkm,
    APatchGki,
}
impl UnrootType {
    fn label_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm",
            Self::APatchGki => "unroottype_apatch_gki",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm_desc",
            Self::APatchGki => "unroottype_apatch_gki_desc",
        }
    }
    fn folder_desc_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm_folderdesc",
            Self::APatchGki => "unroottype_apatch_gki_folderdesc",
        }
    }
}

#[derive(Default)]
struct UnrootWizard {
    step: usize,
    unroot_type: Option<UnrootType>,
    folder_path: Option<String>,
    /// Loader file (`xbl_s_devprg_ns.melf`) for the EDL flash. Decoupled
    /// from the backup folder — a typical Unroot workflow points
    /// `folder_path` at a folder that holds only `boot.img` + `vbmeta.img`,
    /// with no loader inside. The wizard pre-fills this from the
    /// Settings-level default loader when one is configured + on disk;
    /// otherwise the folder step exposes a separate loader picker.
    loader_path: Option<String>,
}

const UNROOT_STEPS: &[&str] = &[
    "unroot_step_method",
    "unroot_step_folder",
    "unroot_step_confirm",
    "unroot_step_restore",
];

impl Wizard for UnrootWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        UNROOT_STEPS.len()
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.unroot_type.is_some(),
            1 => self.folder_path.is_some() && self.loader_path.is_some(),
            2 => true,
            _ => false,
        }
    }
}

// =========================================================================
// Flash wizard state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceRegion {
    Prc,
    Row,
}
impl DeviceRegion {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Prc => "deviceregion_prc",
            Self::Row => "deviceregion_row",
        }
    }

    fn to_region_target(self) -> ltbox_patch::region::RegionTarget {
        match self {
            Self::Prc => ltbox_patch::region::RegionTarget::Prc,
            Self::Row => ltbox_patch::region::RegionTarget::Row,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashTarget {
    OtherRegion,
    SameRegion,
}
impl FlashTarget {
    fn label_key(&self) -> &'static str {
        match self {
            Self::OtherRegion => "flashtarget_other",
            Self::SameRegion => "flashtarget_same",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataMode {
    Keep,
    Wipe,
}
impl DataMode {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Keep => "datamode_keep",
            Self::Wipe => "datamode_wipe",
        }
    }
}

#[derive(Default)]
struct FlashWizard {
    step: usize,
    device_region: Option<DeviceRegion>,
    target: Option<FlashTarget>,
    data_mode: Option<DataMode>,
    firmware_folder: Option<String>,
}

const FLASH_STEPS: &[&str] = &[
    "flash_step_region",
    "flash_step_target",
    "flash_step_data",
    "flash_step_folder",
    "flash_step_confirm",
    "flash_step_flash",
];

impl Wizard for FlashWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        FLASH_STEPS.len()
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.device_region.is_some(),
            1 => self.target.is_some(),
            2 => self.data_mode.is_some(),
            3 => self.firmware_folder.is_some(),
            4 => true,
            _ => false,
        }
    }
}

// =========================================================================
// System Update wizard state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SysUpdateAction {
    Disable,
    Enable,
    Rescue,
}
impl SysUpdateAction {
    fn label_key(&self) -> &'static str {
        match self {
            Self::Disable => "sysupdate_disable",
            Self::Enable => "sysupdate_enable",
            Self::Rescue => "sysupdate_rescue",
        }
    }
    fn desc_key(&self) -> &'static str {
        match self {
            Self::Disable => "sysupdate_disable_desc",
            Self::Enable => "sysupdate_enable_desc",
            Self::Rescue => "sysupdate_rescue_desc",
        }
    }
}

/// Region target for Boot Recovery (Rescue). PRC/ROW hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RescueRegion {
    Prc,
    Row,
}

impl RescueRegion {
    fn label_key(self) -> &'static str {
        match self {
            Self::Prc => "rescue_region_prc",
            Self::Row => "rescue_region_row",
        }
    }
    fn to_target(self) -> ltbox_patch::region::RegionTarget {
        match self {
            Self::Prc => ltbox_patch::region::RegionTarget::Prc,
            Self::Row => ltbox_patch::region::RegionTarget::Row,
        }
    }
}

#[derive(Default)]
struct SysUpdateWizard {
    step: usize,
    action: Option<SysUpdateAction>,
    /// Rescue: firmware folder containing loader (`xbl_s_devprg_ns.melf`).
    rescue_folder: Option<String>,
    /// Rescue: selected target region. Set via popup between Folder and
    /// Confirm steps.
    rescue_region: Option<RescueRegion>,
    /// Rescue: region popup overlay flag. Opens on Next press from the
    /// Folder step when no region is committed yet.
    rescue_region_popup_open: bool,
}

const SYSUPDATE_STEPS_COMPACT: &[&str] = &[
    "sysupdate_step_action",
    "sysupdate_step_confirm",
    "sysupdate_step_execute",
];

const SYSUPDATE_STEPS_RESCUE: &[&str] = &[
    "sysupdate_step_action",
    "sysupdate_step_folder",
    "sysupdate_step_confirm",
    "sysupdate_step_execute",
];

impl SysUpdateWizard {
    /// Rescue gets an extra Folder step — distinct step list keeps the
    /// other actions (Disable/Enable) on their short 3-step flow.
    fn steps(&self) -> &'static [&'static str] {
        if matches!(self.action, Some(SysUpdateAction::Rescue)) {
            SYSUPDATE_STEPS_RESCUE
        } else {
            SYSUPDATE_STEPS_COMPACT
        }
    }
    fn is_rescue(&self) -> bool {
        matches!(self.action, Some(SysUpdateAction::Rescue))
    }
}

impl Wizard for SysUpdateWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        self.steps().len()
    }
    fn can_next(&self) -> bool {
        if self.is_rescue() {
            // Rescue flow: Action → Folder → Confirm → Exec.
            match self.step {
                0 => self.action.is_some(),
                1 => self
                    .rescue_folder
                    .as_deref()
                    .map(std::path::Path::new)
                    .and_then(find_edl_loader)
                    .is_some(),
                2 => self.rescue_region.is_some(),
                _ => false,
            }
        } else {
            match self.step {
                0 => self.action.is_some(),
                1 => true,
                _ => false,
            }
        }
    }
}

// =========================================================================
// Flash Partitions wizard state (Advanced → Flash Partitions)
// =========================================================================

/// Tri-state row action — clicking the checkbox cycles through these
/// in order. Flash requires a `file_path`; Erase wipes the sector range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FlashRowState {
    #[default]
    Unchecked,
    Flash,
    Erase,
}

impl FlashRowState {
    fn cycle(self) -> Self {
        match self {
            Self::Unchecked => Self::Flash,
            Self::Flash => Self::Erase,
            Self::Erase => Self::Unchecked,
        }
    }
}

/// One GPT entry surfaced in the wizard table. `file_path` is populated
/// when the user double-clicks the row and picks an image file.
#[derive(Debug, Clone)]
struct FlashPartRow {
    lun: u8,
    label: String,
    start_sector: u64,
    num_sectors: u64,
    size_bytes: u64,
    file_path: Option<String>,
    state: FlashRowState,
}

/// Column the partition table is currently sorted by. Header click
/// fires `*SortBy(col)`; clicking the active column toggles direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PartsSortColumn {
    #[default]
    Lun,
    Label,
    Start,
    Size,
    /// File-path column — only meaningful for FlashParts; DumpParts has
    /// no file-path column so this variant is never produced from its
    /// header buttons.
    File,
}

#[derive(Default)]
struct FlashPartsWizard {
    step: usize, // 0=Loader, 1=Select, 2=Confirm, 3=Exec
    loader_path: Option<String>,
    rows: Vec<FlashPartRow>,
    scanning: bool,
    scan_error: Option<String>,
    sort_col: PartsSortColumn,
    /// `true` → descending. Default `false` (ascending) on first scan
    /// so initial layout matches the device's GPT order well enough
    /// for LUN-then-label browsing.
    sort_desc: bool,
}

const FLASH_PARTS_STEPS: &[&str] = &[
    "flash_parts_step_loader",
    "flash_parts_step_select",
    "flash_step_confirm",
    "flash_step_flash",
];

impl FlashPartsWizard {
    fn active_rows(&self) -> Vec<FlashPartRow> {
        self.rows
            .iter()
            .filter(|r| match r.state {
                FlashRowState::Flash => r.file_path.is_some(),
                FlashRowState::Erase => true,
                FlashRowState::Unchecked => false,
            })
            .cloned()
            .collect()
    }

    /// Stable-sort `rows` by current `sort_col` / `sort_desc`. Tie-break
    /// on (lun, label) so identical primary keys land in a deterministic
    /// order.
    fn apply_sort(&mut self) {
        let col = self.sort_col;
        let desc = self.sort_desc;
        self.rows.sort_by(|a, b| {
            let ord = match col {
                PartsSortColumn::Lun => a.lun.cmp(&b.lun),
                // ASCII byte order — uppercase (A-Z, 0x41-0x5A) sorts
                // before lowercase (a-z, 0x61-0x7A) by user request.
                PartsSortColumn::Label => a.label.cmp(&b.label),
                PartsSortColumn::Start => a.start_sector.cmp(&b.start_sector),
                PartsSortColumn::Size => a.size_bytes.cmp(&b.size_bytes),
                PartsSortColumn::File => a
                    .file_path
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.file_path.as_deref().unwrap_or("")),
            };
            let ord = ord
                .then_with(|| a.lun.cmp(&b.lun))
                .then_with(|| a.label.cmp(&b.label));
            if desc { ord.reverse() } else { ord }
        });
    }

    /// Header click: toggle direction on the active column, otherwise
    /// switch to the new column ascending.
    fn toggle_sort(&mut self, col: PartsSortColumn) {
        if self.sort_col == col {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_col = col;
            self.sort_desc = false;
        }
        self.apply_sort();
    }
}

impl Wizard for FlashPartsWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        FLASH_PARTS_STEPS.len()
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some() && !self.scanning,
            1 => self.rows.iter().any(|r| match r.state {
                FlashRowState::Flash => r.file_path.is_some(),
                FlashRowState::Erase => true,
                FlashRowState::Unchecked => false,
            }),
            2 => true,
            _ => false,
        }
    }
}

/// Scan-phase result carried in a single message. Same shape as the
/// DumpParts variant but with the Flash row type.
#[derive(Debug, Clone, Default)]
struct FlashPartsScanResult {
    logs: Vec<String>,
    rows: Vec<FlashPartRow>,
    error: Option<String>,
}

// =========================================================================
// Dump Partitions wizard state (Advanced → Dump Partitions)
// =========================================================================

#[derive(Debug, Clone)]
struct DumpPartRow {
    lun: u8,
    label: String,
    start_sector: u64,
    num_sectors: u64,
    size_bytes: u64,
    selected: bool,
}

/// Scan-phase result carried in a single message.
#[derive(Debug, Clone, Default)]
struct DumpPartsScanResult {
    logs: Vec<String>,
    rows: Vec<DumpPartRow>,
    error: Option<String>,
}

#[derive(Default)]
struct DumpPartsWizard {
    step: usize, // 0=Loader, 1=Select, 2=Exec
    loader_path: Option<String>,
    rows: Vec<DumpPartRow>,
    output_dir: Option<String>,
    scanning: bool,
    scan_error: Option<String>,
    sort_col: PartsSortColumn,
    sort_desc: bool,
}

const DUMP_PARTS_STEPS: &[&str] = &[
    "dump_parts_step_loader",
    "dump_parts_step_select",
    "dump_parts_step_dump",
];

impl DumpPartsWizard {
    fn reset(&mut self) {
        *self = Self::default();
    }
    fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some() && !self.scanning,
            1 => self.rows.iter().any(|r| r.selected),
            _ => false,
        }
    }
    fn selected_rows(&self) -> Vec<DumpPartRow> {
        self.rows.iter().filter(|r| r.selected).cloned().collect()
    }

    fn apply_sort(&mut self) {
        let col = self.sort_col;
        let desc = self.sort_desc;
        self.rows.sort_by(|a, b| {
            let ord = match col {
                PartsSortColumn::Lun => a.lun.cmp(&b.lun),
                // ASCII byte order — uppercase (A-Z, 0x41-0x5A) sorts
                // before lowercase (a-z, 0x61-0x7A) by user request.
                PartsSortColumn::Label => a.label.cmp(&b.label),
                PartsSortColumn::Start => a.start_sector.cmp(&b.start_sector),
                PartsSortColumn::Size => a.size_bytes.cmp(&b.size_bytes),
                // DumpParts has no file column; behave as Lun fallback.
                PartsSortColumn::File => a.lun.cmp(&b.lun),
            };
            let ord = ord
                .then_with(|| a.lun.cmp(&b.lun))
                .then_with(|| a.label.cmp(&b.label));
            if desc { ord.reverse() } else { ord }
        });
    }

    fn toggle_sort(&mut self, col: PartsSortColumn) {
        if self.sort_col == col {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_col = col;
            self.sort_desc = false;
        }
        self.apply_sort();
    }
}

// =========================================================================
// Physical Storage wizards (Advanced → Dump/Flash Physical)
//
// LUN-level counterparts to the partition wizards. No GPT scan — the
// user picks which of LUN 0..=5 to hit, and the exec pass reads/writes
// the whole LUN. Mirrors qdlrs `Dump` (whole-disk variant) and
// `OverwriteStorage` commands.
// =========================================================================

const PHYS_LUN_COUNT: usize = 6;

#[derive(Default)]
struct DumpPhysWizard {
    step: usize, // 0=Loader, 1=Select, 2=Exec
    loader_path: Option<String>,
    selected: [bool; PHYS_LUN_COUNT],
    output_dir: Option<String>,
    loader_error: Option<String>,
}

const DUMP_PHYS_STEPS: &[&str] = &[
    "dump_parts_step_loader",
    "phys_step_select",
    "dump_parts_step_dump",
];

impl DumpPhysWizard {
    fn reset(&mut self) {
        *self = Self::default();
    }
    fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some(),
            1 => self.selected.iter().any(|&s| s),
            _ => false,
        }
    }
    fn selected_luns(&self) -> Vec<u8> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i as u8) } else { None })
            .collect()
    }
}

#[derive(Default)]
struct FlashPhysWizard {
    step: usize, // 0=Loader, 1=Select, 2=Confirm, 3=Exec
    loader_path: Option<String>,
    selected: [bool; PHYS_LUN_COUNT],
    file_paths: [Option<String>; PHYS_LUN_COUNT],
    loader_error: Option<String>,
}

const FLASH_PHYS_STEPS: &[&str] = &[
    "flash_parts_step_loader",
    "phys_step_select",
    "flash_step_confirm",
    "flash_step_flash",
];

impl FlashPhysWizard {
    /// (LUN, file_path) pairs for every selected, file-bound row.
    fn active_pairs(&self) -> Vec<(u8, String)> {
        (0..PHYS_LUN_COUNT)
            .filter_map(|i| {
                if self.selected[i] {
                    self.file_paths[i].clone().map(|p| (i as u8, p))
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Wizard for FlashPhysWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        FLASH_PHYS_STEPS.len()
    }
    fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some(),
            // At least one row selected AND every selected row has a file.
            1 => {
                let any = self.selected.iter().any(|&s| s);
                let all_have_file = self
                    .selected
                    .iter()
                    .zip(self.file_paths.iter())
                    .all(|(&s, f)| !s || f.is_some());
                any && all_have_file
            }
            2 => true,
            _ => false,
        }
    }
}

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
    i_tb320fc_edl: &str,
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

    // Step 3: TB320FC fallback over EDL (boot_a + vbmeta_system_a).
    if device_model.eq_ignore_ascii_case("TB320FC") {
        let Some(loader) = loader_path else {
            return Err("TB320FC requires an EDL loader for the deeper rollback inspection".into());
        };
        ltbox_core::live!(log, "[ARB] {i_tb320fc_edl}");
        if ensure_edl(ConnectionStatus::Fastboot, "ARB", log).is_err() {
            return Err("Failed to enter EDL".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let loader_pb = std::path::PathBuf::from(&loader);
        let mut session = ltbox_device::edl::EdlSession::open(&loader_pb, true, log)
            .map_err(|e| format!("EDL open: {e}"))?;
        let tmp = std::env::temp_dir();
        let boot_out = tmp.join("ltbox_arb_boot_a.img");
        let vbm_out = tmp.join("ltbox_arb_vbmeta_system_a.img");
        // boot_a → LUN 4, vbmeta_system_a → LUN 0 per the
        // hardcoded LUN map.
        session
            .dump_partition("boot_a", &boot_out, 0, 4, log)
            .map_err(|e| format!("dump boot_a: {e}"))?;
        session
            .dump_partition("vbmeta_system_a", &vbm_out, 0, 0, log)
            .map_err(|e| format!("dump vbmeta_system_a: {e}"))?;
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
            "boot_a = {boot_idx} ({})",
            format_unix_timestamp_utc(boot_idx)
        );
        ltbox_core::live!(
            log,
            "vbmeta_system_a = {vbm_idx} ({})",
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

/// Wizard for every non-FlashPartitions Advanced action. Steps are
/// [source, confirm, exec], plus a country step between source and
/// confirm for `PatchDevinfo`. Country picker routes into the shared
/// country popup and writes onto `self.country`.
#[derive(Default, Debug, Clone)]
struct AdvWizard {
    action: Option<AdvAction>,
    step: usize,
    file_path: Option<String>,
    file_paths: Vec<String>,
    country: Option<String>,
    /// User-picked target region for `RegionConvert`. The previous
    /// behaviour silently flipped to the opposite of whatever the
    /// vendor_boot scan reported, which gave no UI signal of the
    /// destination and refused to expose "no-op flip" cases. The
    /// popup-driven picker writes the explicit target here so the
    /// confirm step can echo it and the exec path can short-circuit
    /// when source and target match.
    region_target: Option<DeviceRegion>,
    /// `{exe_dir}/output_<action>/` — populated on Confirm → Exec.
    /// Read by the Done card's "Open Folder" pill.
    output_dir: Option<std::path::PathBuf>,
    /// PatchArb only: live-typing buffer for the unix-timestamp popup.
    /// Cleared on cancel / commit. The `arb_index_committed` slot is
    /// the canonical post-popup value.
    arb_index_buffer: String,
    /// PatchArb only: committed target rollback index (unix timestamp,
    /// 10 digits). `Some(_)` after the popup OK closes; gates `Next` on
    /// the inspect step and feeds the Confirm summary + exec.
    arb_index_committed: Option<u64>,
    /// PatchArb only: original rollback indices read from the picked
    /// firmware folder's `boot.img` + `vbmeta_system.img`. `Some` after
    /// the Source-step Next succeeds; gates `Next` on the inspect step.
    /// Carries `(boot_rollback, vbmeta_rollback)`.
    arb_inspect: Option<(u64, u64)>,
}

impl AdvWizard {
    fn open(&mut self, a: AdvAction) {
        *self = Self::default();
        self.action = Some(a);
    }
    fn needs_country(&self) -> bool {
        matches!(self.action, Some(AdvAction::PatchDevinfo))
    }
    fn needs_region_target(&self) -> bool {
        matches!(self.action, Some(AdvAction::RegionConvert))
    }
    fn is_image_info(&self) -> bool {
        matches!(self.action, Some(AdvAction::ImageInfo))
    }
    fn steps(&self) -> &'static [&'static str] {
        if self.is_image_info() {
            return &["adv_step_source", "adv_step_info"];
        }
        if self.needs_country() {
            &[
                "adv_step_source",
                "adv_step_country",
                "flash_step_confirm",
                "flash_step_flash",
            ]
        } else if self.needs_region_target() {
            &[
                "adv_step_source",
                "adv_step_region_target",
                "flash_step_confirm",
                "flash_step_flash",
            ]
        } else if matches!(self.action, Some(AdvAction::PatchArb)) {
            &[
                "adv_step_source",
                "adv_step_arb_inspect",
                "flash_step_confirm",
                "flash_step_flash",
            ]
        } else if matches!(self.action, Some(AdvAction::DetectArb)) {
            // DetectArb: source step is either a loader picker (TB320FC
            // path) or a "Start" prompt; no separate confirm — Next on
            // the source step jumps straight to exec.
            &["adv_step_source", "flash_step_flash"]
        } else {
            &["adv_step_source", "flash_step_confirm", "flash_step_flash"]
        }
    }
    fn exec_step(&self) -> usize {
        self.steps().len() - 1
    }
    fn is_confirm_step(&self) -> bool {
        !self.is_image_info() && self.step + 1 == self.exec_step()
    }
}

impl Wizard for AdvWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        self.steps().len()
    }
    fn can_next(&self) -> bool {
        if self.step == 0 {
            if self.is_image_info() {
                return !self.file_paths.is_empty();
            }
            return self.file_path.is_some();
        }
        if self.needs_country() && self.step == 1 {
            return self.country.is_some();
        }
        if self.needs_region_target() && self.step == 1 {
            return self.region_target.is_some();
        }
        // PatchArb inspect step (step 1) requires the inspect read to
        // have completed successfully before the user can advance into
        // the timestamp popup → confirm step.
        if matches!(self.action, Some(AdvAction::PatchArb)) && self.step == 1 {
            return self.arb_inspect.is_some();
        }
        true
    }
}

impl AdvWizard {
    /// Folder-vs-file dispatch for Browse on step 0.
    fn is_folder_op(&self) -> bool {
        matches!(
            self.action,
            // PatchDevinfo: folder holds devinfo.img + persist.img.
            // ConvertXml: folder holds the encrypted `*.x` pack.
            Some(AdvAction::PatchDevinfo) | Some(AdvAction::ConvertXml) | Some(AdvAction::PatchArb)
        )
    }
    /// Extension whitelist for `rfd::AsyncFileDialog::add_filter`.
    /// Empty slice = no constraint.
    fn accepted_exts(&self) -> (&'static str, &'static [&'static str]) {
        match self.action {
            Some(AdvAction::RegionConvert)
            | Some(AdvAction::ImageInfo)
            | Some(AdvAction::RebuildVbmeta) => ("Android partition image (*.img)", &["img"]),
            Some(AdvAction::DetectArb) => ("EDL loader (.melf)", &["melf"]),
            _ => ("", &[]),
        }
    }

    /// Recents bucket for the current action. Folder actions bin into
    /// one of the 4 user-facing folder categories + `OutputFolder` for
    /// dump destinations; file actions share the `File` bucket per the
    /// unified-file-picker design.
    ///
    /// Kept close to [`Self::is_folder_op`] so they don't diverge -
    /// mismatches would either orphan recents (folder op writing to
    /// `File`) or corrupt them (file path shoved into a folder bucket).
    fn picker_kind(&self) -> pickers::PickerKind {
        use pickers::PickerKind;
        match self.action {
            // Source folders (existing payloads).
            Some(AdvAction::ConvertXml) => PickerKind::EncryptedRawprogramFolder,
            Some(AdvAction::PatchDevinfo) | Some(AdvAction::PatchArb) => {
                PickerKind::QfilFirmwareFolder
            }
            // File-picking actions - all share the unified File bucket.
            Some(AdvAction::RegionConvert)
            | Some(AdvAction::ImageInfo)
            | Some(AdvAction::DetectArb)
            | Some(AdvAction::RebuildVbmeta) => PickerKind::File,
            // Remaining actions don't open a Browse dialog on step 0
            // (DumpPartitions/DumpPhysical/Flash* have dedicated wizards);
            // return File defensively so storage_key() is always valid.
            _ => PickerKind::File,
        }
    }

    /// i18n key for the `[X]` slot in the unified Browse description.
    /// Maps each action to a short noun phrase (e.g. "Encrypted
    /// rawprogram folder", "ARB image"). File pickers read this via
    /// `FilePickSpec::target_i18n_key`; folder pickers read it as the
    /// description caption alongside the generic folder-kind label.
    fn picker_target_i18n_key(&self) -> &'static str {
        match self.action {
            Some(AdvAction::ConvertXml) => "picker_target_encrypted_rawprogram",
            Some(AdvAction::PatchDevinfo) => "picker_target_devinfo_persist_folder",
            Some(AdvAction::RegionConvert) => "picker_target_vendor_boot_img",
            Some(AdvAction::ImageInfo) => "picker_target_avb_images",
            Some(AdvAction::DetectArb) => "picker_target_edl_loader",
            Some(AdvAction::PatchArb) => "picker_target_arb_folder",
            Some(AdvAction::RebuildVbmeta) => "picker_target_vbmeta_img",
            _ => "picker_target_file",
        }
    }
}

// =========================================================================
// Messages
// =========================================================================

#[derive(Debug, Clone, Default)]
struct DevicePollResult {
    status: ConnectionStatus,
    model: String,
    slot: String,
    firmware: String,
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
struct LiveLabels {
    op_root_phase: [String; 7],
    op_unroot_phase: [String; 3],
    op_flash_phase: [String; 4],
    closing_dump: String,
    flash_completed: String,
    root_completed: String,
    unroot_completed: String,
    adb_no_kver: String,
    backup_saved_prefix: String,
    root_resolved_prefix: String,
    root_backup_copy_prefix: String,
}

/// Classify a model → ARB bucket i18n key (`arb_yes`/`arb_no`/`arb_unknown`).
fn arb_from_model(model: &str) -> &'static str {
    let m = model.to_uppercase();
    match m.as_str() {
        "TB320FC" | "TB321FU" | "TB520FU" | "TB710FU" => "arb_yes",
        "TB322FC" => "arb_no",
        _ => "arb_unknown",
    }
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

/// Route device into EDL (Qualcomm 9008). Shared by Root/Unroot/Flash.
///
/// Already-EDL: no-op. Fastboot live: continue system boot, wait for ADB,
/// then `adb reboot edl`. ADB live: `adb reboot edl`. If ADB is not
/// usable, ask the user to reboot manually and wait for 9008.
fn transition_to_edl(_ll: &LiveLabels, log: &mut Vec<String>) -> std::result::Result<(), String> {
    let conn = if ltbox_device::edl::check_device() {
        ConnectionStatus::Edl
    } else if ltbox_device::fastboot::FastbootDevice::check_device() {
        ConnectionStatus::Fastboot
    } else {
        let mut adb = ltbox_device::adb::AdbManager::new();
        if adb.check_device().unwrap_or(false) {
            ConnectionStatus::Adb
        } else {
            ConnectionStatus::None
        }
    };
    ensure_edl(conn, "EDL", log).map_err(|()| "Could not transition device to EDL".to_string())
}

/// M3 neutral pill — translucent `on_surface` fill, muted text, 4 dp
/// corners. Small secondary actions (Cancel / Show log / Save log).
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
        ltbox_core::i18n::tr("log_root_installing_manager_apk")
            .replace("{path}", &manager_apk.display().to_string())
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
        "[Root] Waiting up to {}s for ADB to install manager APK",
        timeout.as_secs()
    );
    loop {
        match install_root_manager_apk(manager_apk, log) {
            Ok(()) => return Ok(()),
            Err(last) if std::time::Instant::now() >= deadline => {
                return Err(format!(
                    "{last}. Install the manager APK manually: {}",
                    manager_apk.display()
                ));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

fn neutral_pill_btn_style(t: &Theme, _s: button::Status) -> button::Style {
    let p = pal_of(t);
    button::Style {
        background: Some(with_alpha(p.on_surface, 0.08).into()),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        text_color: p.on_surface_variant,
        ..Default::default()
    }
}

/// M3 dialog shell — centred card on a dim scrim, 28 dp radius,
/// `surface_container` fill, elevation-2 shadow. Inner content owns
/// its own padding + width.
fn m3_dialog(inner: Element<'_, Message>) -> Element<'_, Message> {
    let card = container(inner).style(|t: &Theme| {
        let p = pal_of(t);
        container::Style {
            background: Some(p.surface_container.into()),
            border: iced::Border {
                color: p.outline_variant,
                width: 1.0,
                radius: 28.0.into(),
            },
            shadow: iced::Shadow {
                color: with_alpha(p.shadow, 0.3),
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        }
    });
    let scrim = container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
            ..Default::default()
        });
    let centered = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    iced::widget::stack![scrim, centered].into()
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
    // viable Firehose loader, so the picker only accepts `.melf`.
    // Filename itself is not enforced — a user-renamed copy of the same
    // .melf still works. `.mbn` / `.elf` were dropped since neither has
    // ever produced a successful flash on the supported hardware.
    pickers::FilePickSpec::single(target_i18n_key).with_filter("EDL loader (.melf)", &["melf"])
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

#[derive(Debug, Clone)]
enum Message {
    Navigate(View),
    SetTheme(ThemeChoice),
    ToggleLogPopup(bool),
    SelectCountry(String),
    SkipCountryPatch,
    DismissCountryPopup,
    SelectRegionTarget(DeviceRegion),
    DismissRegionTargetPopup,
    FileSelected(Option<String>),
    FolderSelected(Option<String>),
    RecentFilePicked(PickerTarget, String),
    RecentFolderPicked(PickerTarget, String),
    NoticeRecentMissing(bool),
    OperationError(String),
    DismissError,
    StartOver,
    PollDevice,
    DevicePolled(DevicePollResult),
    /// Click on the dashboard device portrait. Opens the popup; fires
    /// the Lenovo PTSTPD fetch unless the serial is already cached.
    DeviceInfoOpen,
    /// Result of the PTSTPD fetch keyed by the serial it was started for.
    /// Stale results (different serial than the currently open popup)
    /// are still cached for next time but do not flip the popup state.
    DeviceInfoFetched(String, Result<ltbox_core::lenovo_info::MachineInfo, String>),
    /// User dismissed the device-info popup.
    DeviceInfoClose,
    /// Retry fetch for the currently open popup serial.
    DeviceInfoRetry,
    /// Copy `payload` to the OS clipboard. Pairs with `ToastShow` so
    /// the user gets a visual confirmation; clipboard writes return a
    /// `Task<Message>` from iced so the second message is chained.
    CopyToClipboard(String),
    /// Show a transient bottom-of-screen toast message. Auto-clears
    /// via `ToastClear` after a short delay.
    ToastShow(String),
    /// Clear the active toast (timer expiry).
    ToastClear,
    /// Sidebar mouse-area entered — expand to full width.
    SidebarHoverEnter,
    /// Sidebar mouse-area exited — collapse back to icon-only width.
    SidebarHoverExit,
    /// 16 ms tick from the sidebar tween subscription. Steps
    /// `sidebar_anim` toward its target via exponential decay.
    /// Subscription auto-stops once the value has settled.
    SidebarAnimTick,
    DriverCheckDone(ltbox_device::driver::DriverStatus),
    InstallDrivers,
    InstallDriversDone(Result<Vec<String>, String>),
    UpdateCheckDone(Option<ltbox_core::github::StableRelease>),
    OpenUpdateUrl,
    DrainStdoutTap,
    LogEditorAction(iced::widget::text_editor::Action),
    ImageInfoLogEditorAction(iced::widget::text_editor::Action),
    SaveLog,
    SaveLogPath(Option<std::path::PathBuf>),
    Window(WindowMsg),
    Flash(FlashMsg),
    Root(RootMsg),
    Unroot(UnrootMsg),
    Sys(SysMsg),
    Adv(AdvMsg),
    FlashParts(FlashPartsMsg),
    DumpParts(DumpPartsMsg),
    DumpPhys(DumpPhysMsg),
    FlashPhys(FlashPhysMsg),
    Reboot(RebootMsg),
    Settings(SettingsMsg),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum WindowMsg {
    WindowIdReceived(Option<iced::window::Id>),
    WindowDrag,
    WindowMinimize,
    WindowToggleMaximize,
    WindowClose,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum FlashMsg {
    FlashRegion(DeviceRegion),
    FlashTarget(FlashTarget),
    FlashDataMode(DataMode),
    FlashNext,
    FlashBack,
    FlashSelectFolder,
    FlashExecStart,
    FlashExecDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum RootMsg {
    RootFamily(Family),
    RootProvider(Provider),
    RootMode(RootMode),
    RootVersion(VerChoice),
    RootNightlySource(NightlySource),
    RootSelectFile,
    RootSelectFolder,
    RootNext,
    RootBack,
    RootSelectKpm,
    RootKpmSelected(Option<Vec<String>>),
    RootKpmRemove(String),
    RootSuperkeyInput(String),
    RootSuperkeyConfirm,
    RootSuperkeyCancel,
    RootRunIdInput(String),
    RootRunIdConfirm,
    RootRunIdCancel,
    RootKernelVersionInput(String),
    RootKernelVersionConfirm,
    RootKernelVersionCancel,
    /// Result of the off-UI-thread ADB probe started by `RootNext` when
    /// the wizard hits step 6 with `needs_ksu_lkm_kernel_version()`.
    /// `Some(kver)` advances the wizard; `None` opens the manual-input
    /// popup.
    RootKernelVersionProbeDone(Option<String>),
    RootExecStart,
    RootExecDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum UnrootMsg {
    SetUnrootType(UnrootType),
    UnrootSelectFolder,
    UnrootSelectLoader,
    UnrootLoaderChosen(Option<String>),
    UnrootNext,
    UnrootBack,
    UnrootExecStart,
    UnrootExecDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum SysMsg {
    SysAction(SysUpdateAction),
    SysNext,
    SysBack,
    SysExecStart,
    SysExecDone(Vec<String>),
    SysRescueSelectFolder,
    SysRescueFolderChosen(Option<String>),
    SysRescueRegion(RescueRegion),
    SysRescueRegionPopupDismiss,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum AdvMsg {
    AdvConfirm(AdvAction),
    AdvExec(AdvAction),
    AdvExecDone(Vec<String>),
    AdvFileSelected(AdvAction, Option<String>),
    AdvWizOpen(AdvAction),
    AdvWizBack,
    AdvWizNext,
    AdvWizBrowse,
    AdvWizBrowseDone(Option<String>),
    AdvWizBrowseManyDone(Option<Vec<String>>),
    AdvImageInfoExecStart,
    AdvImageInfoExecDone(Result<String, String>),
    /// DetectArb: kicks off the fastboot+EDL anti-rollback probe on
    /// the heavy pool. Triggered by Next on the source step.
    AdvDetectArbExecStart,
    /// DetectArb worker result. `Vec<String>` is the live-log lines
    /// to flush; `Err(_)` carries a banner message.
    AdvDetectArbExecDone(Result<Vec<String>, String>),
    AdvWizOpenCountry,
    AdvWizOpenRegionTarget,
    AdvWizOpenOutputFolder,
    /// PatchArb timestamp popup: live-typing input.
    AdvWizArbIndexInput(String),
    /// PatchArb timestamp popup: OK pressed (only valid when the buffer
    /// is exactly 10 digits — UI gates this).
    AdvWizArbIndexConfirm,
    /// PatchArb timestamp popup: cancel — closes the popup, clears the
    /// buffer, leaves the wizard on the source step.
    AdvWizArbIndexCancel,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum FlashPartsMsg {
    FlashPartsSelectLoader,
    FlashPartsLoaderChosen(Option<String>),
    FlashPartsToggleRow(usize),
    FlashPartsPickRowFile(usize),
    FlashPartsRowFileChosen(usize, Option<String>),
    FlashPartsNext,
    FlashPartsBack,
    FlashPartsClose,
    FlashPartsScanStart,
    FlashPartsScanDone(FlashPartsScanResult),
    FlashPartsExecStart,
    FlashPartsExecDone(Vec<String>),
    /// Header click in the Select-step table.
    FlashPartsSortBy(PartsSortColumn),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum DumpPartsMsg {
    DumpPartsSelectLoader,
    DumpPartsLoaderChosen(Option<String>),
    DumpPartsToggleRow(usize),
    DumpPartsNext,
    DumpPartsBack,
    DumpPartsClose,
    DumpPartsScanStart,
    DumpPartsScanDone(DumpPartsScanResult),
    DumpPartsSelectFolder,
    DumpPartsFolderChosen(Option<String>),
    DumpPartsExecDone(Vec<String>),
    /// Header click in the Select-step table.
    DumpPartsSortBy(PartsSortColumn),
    /// Header checkbox: select-all when any unselected, otherwise clear.
    DumpPartsToggleAll,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum DumpPhysMsg {
    DumpPhysSelectLoader,
    DumpPhysLoaderChosen(Option<String>),
    DumpPhysToggleRow(usize),
    DumpPhysNext,
    DumpPhysBack,
    DumpPhysClose,
    DumpPhysSelectFolder,
    DumpPhysFolderChosen(Option<String>),
    DumpPhysExecDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum FlashPhysMsg {
    FlashPhysSelectLoader,
    FlashPhysLoaderChosen(Option<String>),
    FlashPhysToggleRow(usize),
    FlashPhysPickRowFile(usize),
    FlashPhysRowFileChosen(usize, Option<String>),
    FlashPhysNext,
    FlashPhysBack,
    FlashPhysClose,
    FlashPhysExecStart,
    FlashPhysExecDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum RebootMsg {
    RebootRequest(RebootTarget),
    RebootConfirm,
    RebootDismiss,
    RebootTo(RebootTarget),
    RebootEdlWithLoader(RebootTarget, Option<String>),
    RebootDone(Vec<String>),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum SettingsMsg {
    SetLanguage(Language),
    SettingsPickDefaultLoader,
    SettingsDefaultLoaderChosen(Option<String>),
    SettingsClearDefaultLoader,
}

// =========================================================================
// App
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ConnectionStatus {
    #[default]
    None,
    Adb,
    /// ADB inside a TWRP recovery build (`ro.product.device` starts
    /// with `twrp_`). Same transition rules as `Adb`; different label.
    AdbRecovery,
    /// ADB sees the device but USB-debug auth is unaccepted
    /// (`unauthorized` / `authorizing`). Shell probes fail; dashboard
    /// shows an authorize-debug prompt.
    AdbUnauthorized,
    Fastboot,
    Edl,
}
impl ConnectionStatus {
    fn label_key(&self) -> &'static str {
        match self {
            Self::None => "conn_disconnected",
            Self::Adb => "conn_adb",
            Self::AdbRecovery => "conn_adb_recovery",
            Self::AdbUnauthorized => "conn_adb_unauthorized",
            Self::Fastboot => "conn_fastboot",
            Self::Edl => "conn_edl",
        }
    }
    fn color(&self, pal: &Palette) -> iced::Color {
        match self {
            Self::None => pal.on_surface_variant,
            Self::Adb | Self::AdbRecovery => pal.success,
            Self::AdbUnauthorized => pal.warning,
            Self::Fastboot => pal.warning,
            Self::Edl => pal.tertiary,
        }
    }
    /// True when exec paths should skip the ADB probe. AdbUnauthorized
    /// counts as "no usable ADB" — shell would fail.
    fn skip_adb(self) -> bool {
        matches!(self, Self::Fastboot | Self::Edl | Self::AdbUnauthorized)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdlEntryAction {
    AlreadyEdl,
    AdbReboot,
    FastbootContinueThenAdb,
    ManualWait,
}

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
        ConnectionStatus::Fastboot => EdlEntryAction::FastbootContinueThenAdb,
        ConnectionStatus::AdbUnauthorized | ConnectionStatus::None => EdlEntryAction::ManualWait,
    }
}

struct App {
    window_id: Option<iced::window::Id>,
    current_view: View,
    /// Effective dark-mode flag — cached to keep repaint off the OS
    /// registry. Recomputed on theme-choice change.
    dark_mode: bool,
    theme_choice: ThemeChoice,
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
    /// Device-info popup state. `None` → popup closed. `Some((serial,
    /// state))` → popup open for `serial`; state tracks the in-flight
    /// fetch result so the popup can render a spinner / error / table.
    device_info_popup: Option<(String, DeviceInfoState)>,
    /// PatchArb wizard's unix-timestamp input popup. `true` while the
    /// modal is on screen between picking the firmware folder and the
    /// Confirm step.
    arb_index_popup_open: bool,
    /// Transient toast message shown as a bottom-of-screen pill.
    /// Cleared by a delayed `ToastClear` task; never persisted.
    toast_msg: Option<String>,
    /// Sidebar collapsed-vs-expanded state. Defaults to collapsed
    /// (icons only). Mouse hover sets the tween's target; the
    /// `SidebarAnimTick` subscription drives `sidebar_anim` toward
    /// it on a 16 ms timer (~60 Hz) and stops once the value
    /// settles, so the timer doesn't keep the GPU awake forever.
    sidebar_expanded: bool,
    /// Sidebar tween progress in `[0.0, 1.0]`. `0.0` = collapsed
    /// (64 px icons only), `1.0` = fully expanded (210 px labels).
    /// Width is `lerp(64, 210, sidebar_anim)` at render time.
    sidebar_anim: f32,
    // Device portrait derived at view time via `device_portrait()`.
    platform_supported: Option<bool>,
    busy: bool,
    /// View that owns the current busy op — labels the dashboard
    /// "in progress" card with the sidebar name.
    busy_view: Option<View>,
    /// Persisted recent picks. Rendered as chips under every picker.
    recent_paths: settings_store::RecentPaths,
    /// Single-device convenience: when `Some(path)`, every loader
    /// picker (Flash / Dump partitions, Flash / Dump physical, Root,
    /// Rescue, Reboot-to-EDL) bypasses the picker and seeds the wizard
    /// slot with this path directly. `None` = picker shows as before.
    /// Re-validated at every exec start so a stale path surfaces an
    /// error before the device side starts.
    default_loader_path: Option<String>,
    log_lines: Vec<String>,
    /// `text_editor::Content` mirror of `log_lines` — supports cursor
    /// drag + Ctrl+C unlike `scrollable(text(...))`. Rebuilt on the
    /// drain tick when `log_dirty` (batches cosmic-text reshape away
    /// from per-push so a long pbr flash doesn't crash wgpu).
    log_editor: iced::widget::text_editor::Content,
    log_dirty: bool,
    image_info_log: String,
    image_info_log_editor: iced::widget::text_editor::Content,
    pending_log_save_source: LogSaveSource,
    error_msg: Option<String>,
    picker_target: PickerTarget,
    driver_status: Option<ltbox_device::driver::DriverStatus>,
    installing_drivers: bool,
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
            Self::None | Self::RootFile | Self::RootLoader => PickerKind::File,
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
        let dark_mode = match theme_choice {
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
            ThemeChoice::System => theme_detect::system_prefers_dark(),
        };
        install_core_translator(lang);
        Self {
            window_id: None,
            current_view: View::default(),
            dark_mode,
            theme_choice,
            settings: SettingsState { language: lang },
            translations: Translations::load(lang),
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
            device_arb: String::new(),
            device_ram: String::new(),
            device_storage: String::new(),
            device_market_name: String::new(),
            device_serial: String::new(),
            device_info_cache: std::collections::HashMap::new(),
            device_info_popup: None,
            arb_index_popup_open: false,
            toast_msg: None,
            sidebar_expanded: false,
            sidebar_anim: 0.0,
            platform_supported: None,
            busy: false,
            busy_view: None,
            recent_paths: persisted.recent_paths.clone(),
            default_loader_path: persisted.default_loader_path.clone(),
            log_lines: vec!["Ready.".to_string()],
            log_editor: iced::widget::text_editor::Content::with_text("Ready."),
            log_dirty: false,
            image_info_log: String::new(),
            image_info_log_editor: iced::widget::text_editor::Content::with_text(""),
            pending_log_save_source: LogSaveSource::Main,
            error_msg: None,
            picker_target: PickerTarget::None,
            driver_status: None,
            installing_drivers: false,
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
        (
            Self::default(),
            Task::batch([win, driver_check, update_check]),
        )
    }
    fn theme(&self) -> Theme {
        if self.dark_mode {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// Localized string. Falls back to English, then the key itself.
    fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.translations.t(key)
    }

    fn pal(&self) -> &'static Palette {
        palette(self.dark_mode)
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
        // The *ExecStart handlers populate op_steps right after this
        // call — zero here for a clean slate.
        self.error_msg = None;
        self.op_steps.clear();
        self.current_op_step = 0;
        // START-only banner per user request — no op-name suffix and
        // no matching END separator. Per-op `[Tag] Starting: …` line
        // (emitted by *ExecStart) carries the operation identity, and
        // the final `[Tag] Completed` / done line already marks the
        // tail; a closing rule was just visual noise.
        let _ = v;
        let label = self.t("log_separator_start").to_string();
        self.log_separator(Some(&label));
    }

    /// 7-phase Root flow (Phase 1/7 → 7/7). Reorganised so the
    /// long-running download steps surface as their own phases instead
    /// of hiding under a generic "patch" label, and short tail
    /// transitions (re-sign, post-flash reboot) ride along with the
    /// adjacent step they pair with.
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

    /// Operation-specific replacement for the default busy-dialog body
    /// (`progress_dialog_body`). Used for sub-flows where "<op> is in
    /// progress" reads awkwardly — the four Advanced partition / physical
    /// flows (DumpParts, FlashParts, DumpPhys, FlashPhys) all show their
    /// busy dialog only during the reboot → loader-upload → GPT-scan
    /// preamble, so the unified "Reading partition info…" line is the
    /// honest description regardless of which one of the four the user
    /// kicked off. Returns `None` so the caller falls back to the
    /// templated body for everything else.
    ///
    /// Gated on `busy_view == Advanced` so a stale `advanced_wizard_open`
    /// (the wizards stay mounted under the
    /// scrim while the op runs and only clear on `start_over`) doesn't
    /// hijack an unrelated busy dialog — e.g. the EDL → System reboot
    /// from the Reboot menu was rendering "Reading partition…" because
    /// the user had a DumpParts session still open underneath.
    fn busy_body_override(&self) -> Option<String> {
        if self.busy_view != Some(View::Advanced) {
            return None;
        }
        if self.advanced_wizard_open.is_open() {
            return Some(self.t("busy_partition_scan").to_string());
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

    /// Shared loader-picker helper. Bypasses the file picker if Settings
    /// has a default EDL loader configured, otherwise opens
    /// `loader_file_spec` and routes the result through `on_chosen`.
    /// Used by every `*SelectLoader` handler to dedupe the
    /// `default_loader_path → update | pick_file_for` boilerplate.
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

    /// Map the Lenovo PTSTPD `SaleArea` field of the *currently
    /// connected* device (keyed by `device_serial`) to a `DeviceRegion`
    /// for Flash-wizard preselect:
    ///
    /// * `"CN"` → PRC.
    /// * JSON `null` → ROW.
    /// * Other strings, missing key, or no cached entry for the current
    ///   serial → `None` (caller leaves the field untouched).
    ///
    /// Reads only from the in-memory `device_info_cache`; never issues
    /// a network call. Used by both the post-fetch handler and the
    /// `Navigate(View::Flash)` reset path so navigating into Flash does
    /// not undo a region the user already had inferred.
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

    /// Pre-flight check for the loader path captured by every EDL-using
    /// wizard. Returns `Ok(path_string)` when the path is set and the
    /// file exists on disk; otherwise sets `error_msg` to a localized
    /// string and returns `Err(())` so the caller can early-return out
    /// of the exec start without firing the device-side work.
    ///
    /// Catches the failure mode the new "Default EDL loader" Settings
    /// option introduces: a single-device user sets a default loader
    /// once, the loader file is later moved/deleted, and every wizard
    /// silently inherits the stale path. Without this guard the failure
    /// surfaces as an `EDL session open failed: ...` line buried in the
    /// live log; with it, the user sees a clear "Loader file not found"
    /// banner and the wizard stays on the Confirm step instead of
    /// uselessly rebooting the device into Sahara.
    fn validate_loader_path(&mut self, path: &Option<String>) -> Result<String, ()> {
        let Some(p) = path.as_deref() else {
            self.error_msg = Some(self.t("err_loader_not_selected").to_string());
            return Err(());
        };
        let pb = std::path::Path::new(p);
        if !pb.is_file() {
            let msg = self.t("err_loader_missing").replace("{path}", p);
            self.error_msg = Some(msg);
            return Err(());
        }
        Ok(p.to_string())
    }

    fn persist_settings(&self) {
        settings_store::save(&settings_store::PersistedSettings {
            language: self.settings.language.code().to_string(),
            theme: self.theme_choice.code().to_string(),
            // Legacy field kept readable by older builds.
            dark_mode: self.dark_mode,
            recent_paths: self.recent_paths.clone(),
            default_loader_path: self.default_loader_path.clone(),
        });
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
            if is_loader_file(path) {
                return Ok(selected_path.to_string());
            }
            return Err(format!(
                "Unsupported loader file: {selected_path} (expected .melf, .mbn, or .elf)"
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
        // Sidebar width tween: only emit ticks while the animation
        // hasn't settled at its target so the GPU isn't woken every
        // 16 ms forever.
        let sidebar_settled = (self.sidebar_anim - self.sidebar_anim_target()).abs() < 0.005;
        if !sidebar_settled {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| Message::SidebarAnimTick),
            );
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // Window controls
            Message::Window(WindowMsg::WindowIdReceived(id)) => self.window_id = id,
            Message::Window(WindowMsg::WindowDrag) => {
                if let Some(id) = self.window_id {
                    return iced::window::drag(id);
                }
            }
            Message::Window(WindowMsg::WindowMinimize) => {
                if let Some(id) = self.window_id {
                    return iced::window::minimize(id, true);
                }
            }
            Message::Window(WindowMsg::WindowToggleMaximize) => {
                if let Some(id) = self.window_id {
                    return iced::window::toggle_maximize(id);
                }
            }
            Message::Window(WindowMsg::WindowClose) => {
                if let Some(id) = self.window_id {
                    return iced::window::close(id);
                }
            }
            // Navigation
            Message::Navigate(v) => {
                self.current_view = v;
                // Keep wizard state during a running op or on the
                // exec/Done screen — sidebar bounce mid-flash must
                // not kick back to step 0.
                let busy = self.busy;
                if v == View::Root && !busy && !self.root.is_in_exec() {
                    self.root.reset();
                }
                if v == View::Flash && !busy && !self.flash.is_in_exec() {
                    self.flash.reset();
                    // Re-apply SaleArea-driven preselect: `flash.reset()`
                    // wipes `device_region` back to `None`, but the user's
                    // earlier device-info fetch already picked a region;
                    // mirror it onto the freshly-reset wizard so navigating
                    // into Flash does not undo the inference.
                    if self.flash.device_region.is_none()
                        && let Some(r) = self.inferred_flash_region()
                    {
                        self.flash.device_region = Some(r);
                    }
                }
                if v == View::SystemUpdate && !busy && !self.sysupdate.is_in_exec() {
                    self.sysupdate.reset();
                }
                if v == View::Unroot && !busy && !self.unroot.is_in_exec() {
                    self.unroot.reset();
                    // Pre-fill the new loader slot from Settings so the
                    // user with a default loader configured doesn't see
                    // a "Pick loader" prompt — the folder step's loader
                    // sub-row hides itself once `loader_path` is `Some`.
                    if let Some(path) = self.resolved_default_loader() {
                        self.unroot.loader_path = Some(path);
                    }
                }
            }
            Message::SetTheme(choice) => {
                self.theme_choice = choice;
                self.dark_mode = match choice {
                    ThemeChoice::Light => false,
                    ThemeChoice::Dark => true,
                    ThemeChoice::System => theme_detect::system_prefers_dark(),
                };
                self.persist_settings();
            }
            Message::ToggleLogPopup(open) => {
                self.log_popup_open = open;
            }
            // Settings
            Message::Settings(SettingsMsg::SetLanguage(l)) => {
                self.settings.language = l;
                self.translations = Translations::load(l);
                install_core_translator(l);
                self.persist_settings();
            }
            Message::Settings(SettingsMsg::SettingsPickDefaultLoader) => {
                let spec = loader_file_spec("picker_target_edl_loader");
                return pickers::pick_file_for(spec, &self.recent_paths, |__v| {
                    Message::Settings(SettingsMsg::SettingsDefaultLoaderChosen(__v))
                });
            }
            Message::Settings(SettingsMsg::SettingsDefaultLoaderChosen(path)) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.default_loader_path = Some(p);
                    self.persist_settings();
                }
            }
            Message::Settings(SettingsMsg::SettingsClearDefaultLoader) => {
                self.default_loader_path = None;
                self.persist_settings();
            }
            // Flash wizard
            Message::Flash(FlashMsg::FlashRegion(r)) => self.flash.device_region = Some(r),
            Message::Flash(FlashMsg::FlashTarget(t)) => self.flash.target = Some(t),
            Message::Flash(FlashMsg::FlashDataMode(m)) => self.flash.data_mode = Some(m),
            Message::Flash(FlashMsg::FlashNext) => {
                // Data step → build WorkflowConfig; wipe opens country popup.
                if self.flash.step == 2 {
                    self.wf_config = WorkflowConfig {
                        modify_region: self.flash.target == Some(FlashTarget::OtherRegion),
                        device_region: self.flash.device_region,
                        modify_rollback: if self.flash.target == Some(FlashTarget::OtherRegion) {
                            RollbackSetting::On
                        } else {
                            RollbackSetting::Auto
                        },
                        wipe: self.flash.data_mode == Some(DataMode::Wipe),
                        country_action: CountryAction::Unset,
                    };
                    if self.wf_config.wipe {
                        self.flash.next();
                        self.country_popup_open = true;
                        return Task::none();
                    }
                }
                if self.flash.step == 4 {
                    self.flash.next();
                    return self.update(Message::Flash(FlashMsg::FlashExecStart));
                }
                self.flash.next();
            }
            Message::Flash(FlashMsg::FlashBack) => {
                if self.flash.step == 4 {
                    // Re-arm country patching so the popup's "Do not change"
                    // selection doesn't survive a Back→Next round trip.
                    self.wf_config.country_action = CountryAction::Unset;
                }
                self.flash.back();
            }
            Message::Flash(FlashMsg::FlashSelectFolder) => {
                self.picker_target = PickerTarget::FlashFolder;
                return pick_folder_task(
                    pickers::PickerKind::QfilFirmwareFolder,
                    &self.recent_paths,
                    Message::FolderSelected,
                );
            }
            Message::Flash(FlashMsg::FlashExecStart) => {
                self.begin_op(View::Flash);
                self.op_steps = self.derive_flash_op_steps();
                self.error_msg = None;
                let cfg = self.wf_config.clone();
                let conn = self.connection;
                let fw_folder = self.flash.firmware_folder.clone().unwrap_or_default();
                let rollback_label = self.t(cfg.modify_rollback.label_key()).to_string();
                self.log_push(format!(
                    "[Flash] Starting: modify_region={} rollback={} wipe={}",
                    cfg.modify_region, rollback_label, cfg.wipe
                ));
                let rb_label_for_log = rollback_label.clone();
                // Snapshot rollback index before EDL — `stored_rollback_index`
                // vanishes past Fastboot. Two `None` flavours matter:
                // vars-returned-no-index (no ARB committed, skip) vs
                // vars-unreachable (unsafe for ON mode, caller aborts).
                let (device_rollback_index, fastboot_reachable): (Option<u64>, bool) =
                    match ltbox_device::fastboot::FastbootDevice::open() {
                        Ok(mut dev) => match dev.get_all_vars() {
                            Ok(v) => (
                                ltbox_patch::rollback::compute_device_rollback_index(
                                    &v.rollback_indices,
                                ),
                                true,
                            ),
                            Err(_) => (None, false),
                        },
                        Err(_) => (None, false),
                    };
                let rb_mode = cfg.modify_rollback.to_mode();
                let ll = self.live_labels();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || -> Result<Vec<String>, String> {
                            let mut log = Vec::new();
                            let fw_dir = std::path::Path::new(&fw_folder);

                            // 1. Validate firmware folder
                            live!(log, "[Flash] {}", phase_marker(1, 4, &ll.op_flash_phase[0]));
                            if !fw_dir.exists() {
                                return Err(format!("Firmware folder not found: {fw_folder}"));
                            }
                            live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_flash_firmware_folder")
                                    .replace("{path}", &fw_folder)
                            );

                            // Rollback=ON + no fastboot vars → can't target
                            // a safe index. Bail before risking a brick.
                            if matches!(rb_mode, ltbox_patch::rollback::RollbackMode::On)
                                && !fastboot_reachable
                            {
                                live!(
                                    log,
                                    "[ARB] {}",
                                    ltbox_core::i18n::tr("live_arb_on_fastboot_unreachable")
                                );
                                // Best-effort reboot — any failure stays
                                // in the log; wizard still gets the Err.
                                let mut adb = ltbox_device::adb::AdbManager::new();
                                if adb.check_device().unwrap_or(false) {
                                    if let Err(e) = adb.shell("reboot") {
                                        ltbox_core::live!(
                                            log,
                                            "[ADB] {}",
                                            ltbox_core::i18n::tr("live_adb_reboot_failed")
                                                .replace("{error}", &e.to_string())
                                        );
                                    } else {
                                        ltbox_core::live!(
                                            log,
                                            "[ADB] {}",
                                            ltbox_core::i18n::tr("live_adb_reboot_sent")
                                        );
                                    }
                                } else {
                                    ltbox_core::live!(
                                        log,
                                        "[ADB] {}",
                                        ltbox_core::i18n::tr("live_adb_no_reboot_route")
                                    );
                                }
                                return Err("Rollback=ON requires fastboot var access. Device not in fastboot or getvar failed — aborted without flashing.".to_string());
                            }

                            // 2. Device detection
                            let skip_adb = conn.skip_adb();
                            if skip_adb {
                                ltbox_core::live!(
                                    log,
                                    "[Flash] {}",
                                    ltbox_core::i18n::tr("live_flash_skip_adb")
                                );
                            } else {
                                ltbox_core::live!(
                                    log,
                                    "[ADB] {}",
                                    ltbox_core::i18n::tr("live_adb_checking_device")
                                );
                                let mut adb = ltbox_device::adb::AdbManager::new();
                                if adb.check_device().unwrap_or(false) {
                                    ltbox_core::live!(
                                        log,
                                        "[ADB] {}",
                                        ltbox_core::i18n::tr("live_adb_device_connected")
                                    );
                                    let _slot = adb.get_slot_suffix().ok().flatten().unwrap_or_default();
                                } else {
                                    ltbox_core::live!(
                                        log,
                                        "[ADB] {}",
                                        ltbox_core::i18n::tr("live_adb_no_device_info")
                                    );
                                }
                            }

                            // 3. Scan firmware folder
                            let vendor_boot = fw_dir.join("vendor_boot.img");
                            let vbmeta = fw_dir.join("vbmeta.img");
                            let boot = fw_dir.join("boot.img");
                            let has_vendor_boot = vendor_boot.exists();
                            let has_vbmeta = vbmeta.exists();
                            let has_boot = boot.exists();
                            let found = ltbox_core::i18n::tr("live_status_found");
                            let not_found = ltbox_core::i18n::tr("live_status_not_found");
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_flash_vendor_boot_status").replace(
                                    "{status}",
                                    if has_vendor_boot { &found } else { &not_found }
                                )
                            );
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_flash_vbmeta_status").replace(
                                    "{status}",
                                    if has_vbmeta { &found } else { &not_found }
                                )
                            );
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_flash_boot_status").replace(
                                    "{status}",
                                    if has_boot { &found } else { &not_found }
                                )
                            );

                            // Count .x and .xml files
                            let x_count = std::fs::read_dir(fw_dir).map(|rd| rd.filter(|e| {
                                e.as_ref().ok().map(|e| e.path().extension().map(|ext| ext == "x").unwrap_or(false)).unwrap_or(false)
                            }).count()).unwrap_or(0);
                            let xml_count = std::fs::read_dir(fw_dir).map(|rd| rd.filter(|e| {
                                e.as_ref().ok().map(|e| {
                                    let p = e.path();
                                    p.extension().map(|ext| ext == "xml").unwrap_or(false)
                                        && p.file_name().map(|n| n.to_string_lossy().starts_with("rawprogram")).unwrap_or(false)
                                }).unwrap_or(false)
                            }).count()).unwrap_or(0);
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_flash_files_count")
                                    .replace("{x_count}", &x_count.to_string())
                                    .replace("{xml_count}", &xml_count.to_string())
                            );

                            // 4. Region conversion
                            let mut region_pair: Option<ltbox_patch::region::RegionBootChainOutput> = None;
                            if cfg.modify_region {
                                if has_vendor_boot && has_vbmeta {
                                    ltbox_core::live!(
                                        log,
                                        "[Region] {}",
                                        ltbox_core::i18n::tr("live_region_on")
                                    );
                                    ltbox_core::live!(
                                        log,
                                        "[Region] {}",
                                        ltbox_core::i18n::tr("live_region_ready")
                                    );
                                    let Some(device_region) = cfg.device_region else {
                                        return Err(
                                            "Region conversion requested but no device region was selected"
                                                .to_string(),
                                        );
                                    };
                                    let target = device_region.to_region_target();
                                    let output_dir =
                                        ltbox_core::app_paths::auto_output_dir_for("region_convert");
                                    ltbox_core::live!(
                                        log,
                                        "[Region] Building vendor_boot/vbmeta pair for {:?} hardware",
                                        device_region
                                    );
                                    match ltbox_patch::region::build_region_converted_boot_chain(
                                        fw_dir,
                                        &output_dir,
                                        target,
                                        &ltbox_patch::region::RegionPatternSet::default(),
                                    ) {
                                        Ok(ltbox_patch::region::RegionBootChainBuild::Built(output)) => {
                                            ltbox_core::live!(
                                                log,
                                                "[Region] {}",
                                                ltbox_core::i18n::tr("live_region_source_target")
                                                    .replace("{source}", &format!("{:?}", output.source_region))
                                                    .replace("{target}", &format!("{:?}", output.target))
                                            );
                                            ltbox_core::live!(
                                                log,
                                                "[Region] {}",
                                                ltbox_core::i18n::tr("live_region_patched")
                                                    .replace("{count}", &output.replacement_count.to_string())
                                                    .replace("{path}", &output.vendor_boot.display().to_string())
                                            );
                                            ltbox_core::live!(
                                                log,
                                                "[Region] Repaired vendor_boot footer and rebuilt vbmeta: {}",
                                                output.vbmeta.display()
                                            );
                                            region_pair = Some(output);
                                        }
                                        Ok(ltbox_patch::region::RegionBootChainBuild::Skipped {
                                            source_region,
                                            target,
                                        }) => {
                                            ltbox_core::live!(
                                                log,
                                                "[Region] {}",
                                                ltbox_core::i18n::tr("live_region_source_target")
                                                    .replace("{source}", &format!("{:?}", source_region))
                                                    .replace("{target}", &format!("{:?}", target))
                                            );
                                            ltbox_core::live!(
                                                log,
                                                "[Region] {}",
                                                ltbox_core::i18n::tr("live_region_source_matches_target")
                                            );
                                        }
                                        Err(e) => return Err(format!("Region conversion failed: {e}")),
                                    }
                                } else {
                                    ltbox_core::live!(
                                        log,
                                        "[Region] {}",
                                        ltbox_core::i18n::tr("live_region_missing_skip")
                                    );
                                }
                            } else {
                                ltbox_core::live!(
                                    log,
                                    "[Region] {}",
                                    ltbox_core::i18n::tr("live_region_off")
                                );
                            }

                            // 5. ARB detection
                            ltbox_core::live!(
                                log,
                                "[ARB] {}",
                                ltbox_core::i18n::tr("live_arb_modify").replace("{value}", &rb_label_for_log)
                            );
                            let device_idx_str = device_rollback_index
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| ltbox_core::i18n::tr("live_arb_device_index_none"));
                            ltbox_core::live!(
                                log,
                                "[ARB] {}",
                                ltbox_core::i18n::tr("live_arb_device_index")
                                    .replace("{index}", &device_idx_str)
                            );
                            if has_boot {
                                // Pre-result "Analyzing …" line dropped — analysis is
                                // synchronous and the result line ("boot.img rollback
                                // index: …") fires immediately after.
                                match ltbox_patch::rollback::analyze_rollback_with_mode(
                                    &boot,
                                    device_rollback_index,
                                    rb_mode,
                                ) {
                                    Ok(info) => ltbox_core::live!(
                                        log,
                                        "[ARB] {}",
                                        ltbox_core::i18n::tr("live_arb_boot_index_result")
                                            .replace("{index}", &info.image_index.to_string())
                                            .replace("{needs}", &info.needs_patch.to_string())
                                            .replace("{mode}", &format!("{:?}", rb_mode))
                                    ),
                                    Err(e) => ltbox_core::live!(
                                        log,
                                        "[ARB] {}",
                                        ltbox_core::i18n::tr("live_arb_boot_analysis_failed")
                                            .replace("{error}", &e.to_string())
                                    ),
                                }
                            }
                            // ARB analysis above is diagnostic only — flash plan unchanged.

                            // 6. XML
                            if x_count > 0 {
                                ltbox_core::live!(
                                    log,
                                    "[XML] {}",
                                    ltbox_core::i18n::tr("live_xml_decrypt_pending")
                                        .replace("{count}", &x_count.to_string())
                                );
                            }
                            if !cfg.wipe && xml_count > 0 {
                                ltbox_core::live!(
                                    log,
                                    "[XML] {}",
                                    ltbox_core::i18n::tr("live_xml_keep_excludes")
                                );
                            }

                            // 7. Country code
                            if cfg.wipe {
                                ltbox_core::live!(
                                    log,
                                    "[Flash] {}",
                                    ltbox_core::i18n::tr("live_flash_data_mode_wipe")
                                );
                                if let Some(cc) = cfg.country_action.target() {
                                    ltbox_core::live!(
                                        log,
                                        "[Flash] {}",
                                        ltbox_core::i18n::tr("live_flash_country_devinfo")
                                            .replace("{code}", cc)
                                    );
                                } else if cfg.country_action.is_skipped() {
                                    ltbox_core::live!(
                                        log,
                                        "[Flash] {}",
                                        ltbox_core::i18n::tr("live_flash_country_skip")
                                    );
                                }
                            } else {
                                ltbox_core::live!(
                                    log,
                                    "[Flash] {}",
                                    ltbox_core::i18n::tr("live_flash_data_mode_keep")
                                );
                            }

                            // 8. EDL flash
                            let loader = find_edl_loader(fw_dir)
                                .or_else(|| fw_dir.parent().and_then(find_edl_loader));
                            let loader = match loader {
                                Some(l) => l,
                                None => {
                                    ltbox_core::live!(
                                        log,
                                        "[EDL] {}",
                                        ltbox_core::i18n::tr("live_edl_loader_missing")
                                    );
                                    return Ok(log);
                                }
                            };

                            live!(log, "[Flash] {}", phase_marker(2, 4, &ll.op_flash_phase[1]));
                            transition_to_edl(&ll, &mut log)?;

                            let mut session = ltbox_device::edl::EdlSession::open(&loader, true, &mut log)
                                .map_err(|e| format!("EDL session: {e}"))?;

                            // Full-firmware flash: rawprogram + patch XMLs
                            // drive every program node (no slot guessing).
                            let (raw_xmls, patch_xmls) =
                                ltbox_device::edl::collect_firmware_xmls_for_flash(fw_dir, false)
                                    .map_err(|e| format!("Firmware XML selection failed: {e}"))?;
                            if raw_xmls.is_empty() {
                                return Err(format!(
                                    "No flashable rawprogram*.xml found in {fw_folder}"
                                ));
                            }
                            // ARB-patched copies are flashed *after* rawprogram
                            // so the user's firmware folder stays untouched.
                            // LUN comes from the hardcoded map; start sector
                            // resolves through GPT-by-name in
                            // `flash_partition`. Slot `_a` matches the prior
                            // first-hit `catalog.require(..._a, ..._b, …)`
                            // semantics — overwrites A on top of the
                            // full-firmware flash that already wrote both.
                            let mut arb_patched: Vec<(String, u8, std::path::PathBuf)> =
                                Vec::new();
                            if rb_mode != ltbox_patch::rollback::RollbackMode::Off {
                                let arb_work_dir =
                                    ltbox_core::app_paths::work_dir_for("flash_arb");
                                let _ = std::fs::remove_dir_all(&arb_work_dir);
                                std::fs::create_dir_all(&arb_work_dir)
                                    .map_err(|e| format!("arb work dir: {e}"))?;

                                // (base, on-disk filename, slot label)
                                let label_pairs: &[(&str, &str, &str)] = &[
                                    ("boot", "boot.img", "boot_a"),
                                    (
                                        "vbmeta_system",
                                        "vbmeta_system.img",
                                        "vbmeta_system_a",
                                    ),
                                ];
                                for (log_name, filename, slot_label) in label_pairs {
                                    let Some(lun) =
                                        ltbox_core::partition_lun::lun_for_partition(log_name)
                                    else {
                                        ltbox_core::live!(log,
                                            "[ARB] {log_name}: no hardcoded LUN — skipping"
                                        );
                                        continue;
                                    };
                                    let source = fw_dir.join(filename);
                                    if !source.exists() {
                                            ltbox_core::live!(log,
                                                "[ARB] {log_name}: {} not found — skipping",
                                                source.display()
                                            );
                                            continue;
                                        }

                                        // `Off` is already bypassed; On or Auto here.
                                        let analysis = match ltbox_patch::rollback::analyze_rollback_with_mode(
                                            &source,
                                            device_rollback_index,
                                            rb_mode,
                                        ) {
                                            Ok(a) => a,
                                            Err(e) => {
                                                ltbox_core::live!(log,
                                                    "[ARB] analyze {log_name} failed: {e}"
                                                );
                                                continue;
                                            }
                                        };
                                        ltbox_core::live!(log,
                                            "[ARB] {log_name}: image={}, needs_patch={} (mode={:?})",
                                            analysis.image_index, analysis.needs_patch, rb_mode
                                        );
                                        if !analysis.needs_patch {
                                            continue;
                                        }
                                        let Some(target) = device_rollback_index else {
                                            ltbox_core::live!(log,
                                                "[ARB] {log_name}: needs_patch but device index unknown — skipping"
                                            );
                                            continue;
                                        };

                                        // Signing-key resolution: only the two stock
                                        // testkeys embedded in avbtool-rs. Any image
                                        // signed by an unknown pubkey is skipped.
                                        let key_from_map = ltbox_patch::key_map::key_spec_for_pubkey(
                                            analysis.image_info.public_key_sha1.as_deref(),
                                        );

                                        let patched = arb_work_dir.join(format!("{log_name}.arb.img"));
                                        let is_vbmeta = log_name.starts_with("vbmeta");
                                        let patch_result = if is_vbmeta {
                                            // vbmeta always resigns (no add_hash_footer).
                                            match key_from_map {
                                                Some(spec) => {
                                                    std::fs::copy(&source, &patched)
                                                        .map_err(|e| format!("copy vbmeta: {e}"))?;
                                                    ltbox_patch::avb::resign_image(
                                                        &patched,
                                                        spec,
                                                        &analysis.image_info.algorithm,
                                                        Some(target),
                                                    )
                                                    .map_err(|e| format!("resign {log_name}: {e}"))
                                                }
                                                None => {
                                                    ltbox_core::live!(log,
                                                        "[ARB] {log_name}: pubkey {:?} not in testkey map — skipping",
                                                        analysis.image_info.public_key_sha1
                                                    );
                                                    continue;
                                                }
                                            }
                                        } else if analysis.image_info.algorithm == "NONE" {
                                            // NONE algorithm: add_hash_footer accepts
                                            // an Option<&str> spec; pass map result
                                            // (None is fine).
                                            std::fs::copy(&source, &patched)
                                                .map_err(|e| format!("copy chained: {e}"))?;
                                            ltbox_patch::avb::add_hash_footer(
                                                &patched,
                                                &analysis.image_info,
                                                key_from_map,
                                                Some(target),
                                            )
                                            .map_err(|e| format!("patch {log_name}: {e}"))
                                        } else if let Some(spec) = key_from_map {
                                            std::fs::copy(&source, &patched)
                                                .map_err(|e| format!("copy chained: {e}"))?;
                                            ltbox_patch::avb::resign_image(
                                                &patched,
                                                spec,
                                                &analysis.image_info.algorithm,
                                                Some(target),
                                            )
                                            .map_err(|e| format!("resign {log_name}: {e}"))
                                        } else {
                                            ltbox_core::live!(
                                                log,
                                                "[ARB] {}",
                                                ltbox_core::i18n::tr("live_arb_no_signing_key")
                                                    .replace("{name}", log_name)
                                                    .replace(
                                                        "{key}",
                                                        &format!("{:?}", analysis.image_info.public_key_sha1),
                                                    )
                                            );
                                            continue;
                                        };
                                        if let Err(e) = patch_result {
                                            ltbox_core::live!(
                                                log,
                                                "[ARB] {}",
                                                ltbox_core::i18n::tr("live_arb_patch_failed")
                                                    .replace("{name}", log_name)
                                                    .replace("{error}", &e.to_string())
                                            );
                                            continue;
                                        }

                                        live!(
                                            log,
                                            "[ARB] {}",
                                            ltbox_core::i18n::tr("live_arb_prepared_patch")
                                                .replace("{name}", log_name)
                                                .replace("{path}", &patched.display().to_string())
                                                .replace("{target}", &target.to_string())
                                        );
                                        arb_patched.push((
                                            slot_label.to_string(),
                                            lun,
                                            patched,
                                        ));
                                    }
                            }

                            live!(
                                log,
                                "[Flash] {} ({})",
                                phase_marker(3, 4, &ll.op_flash_phase[2]),
                                ltbox_core::i18n::tr("live_flash_phase3_xml_counts")
                                    .replace("{raw}", &raw_xmls.len().to_string())
                                    .replace("{patch}", &patch_xmls.len().to_string())
                            );
                            session
                                .flash_rawprogram_with_wipe(
                                    &raw_xmls,
                                    &patch_xmls,
                                    cfg.wipe,
                                    &mut log,
                                )
                                .map_err(|e| format!("Firmware flash failed: {e}"))?;

                            // Overwrite rawprogram's stock boot + vbmeta_system
                            // with the ARB-patched copies. GPT-by-name lookup
                            // resolves the slot's start sector from the device,
                            // not the firmware folder's rawprogram XML.
                            for (label, lun, patched) in &arb_patched {
                                live!(
                                    log,
                                    "[ARB] {}",
                                    ltbox_core::i18n::tr("live_arb_flash_patched")
                                        .replace("{label}", label)
                                );
                                if let Err(e) = session.flash_partition(
                                    label,
                                    patched,
                                    0,
                                    *lun,
                                    &mut log,
                                ) {
                                    return Err(format!("ARB flash {label}: {e}"));
                                }
                            }

                            // Overwrite rawprogram's stock vendor_boot/vbmeta
                            // with the final region-converted AVB-valid pair.
                            // This must happen after rawprogram (and after any
                            // ARB overlays) so stock XML entries cannot put the
                            // unconverted ROW pair back on top.
                            if let Some(output) = &region_pair {
                                let overlays: [(&str, &std::path::Path); 2] = [
                                    ("vendor_boot_a", output.vendor_boot.as_path()),
                                    ("vbmeta_a", output.vbmeta.as_path()),
                                ];
                                for (label, image) in overlays {
                                    let Some(lun) =
                                        ltbox_core::partition_lun::lun_for_partition(label)
                                    else {
                                        return Err(format!(
                                            "Region flash {label}: no hardcoded LUN"
                                        ));
                                    };
                                    live!(
                                        log,
                                        "[Region] Flashing final {} ← {}",
                                        label,
                                        image.display()
                                    );
                                    if let Err(e) = session.flash_partition(
                                        label,
                                        image,
                                        0,
                                        lun,
                                        &mut log,
                                    ) {
                                        return Err(format!("Region flash {label}: {e}"));
                                    }
                                }
                            }

                            // Country code patch: dump → patch → flash devinfo
                            // + persist. Skipped when the user picked "Do not
                            // change" (`country_action` is `Skip`) — the
                            // device's existing region images stay put.
                            if cfg.wipe
                                && let Some(target_code) = cfg.country_action.target() {
                                    live!(
                                        log,
                                        "[Flash] {}",
                                        ltbox_core::i18n::tr("live_flash_country_patch_target")
                                            .replace("{target}", target_code)
                                    );
                                    let work_dir =
                                        ltbox_core::app_paths::work_dir_for("flash_country");
                                    let _ = std::fs::remove_dir_all(&work_dir);
                                    if let Err(e) = std::fs::create_dir_all(&work_dir) {
                                        return Err(format!("country work dir: {e}"));
                                    }
                                    // Critical-image backup: original region
                                    // partitions held aside for manual restore.
                                    // `app_paths::backup_dir_for` keeps writes
                                    // off the read-only AppImage mount on
                                    // non-Windows hosts.
                                    let ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    let critical_backup = ltbox_core::app_paths::backup_dir_for(
                                        &format!("backup_critical_{ts}"),
                                    );
                                    std::fs::create_dir_all(&critical_backup)
                                        .map_err(|e| format!("critical backup folder: {e}"))?;
                                    // devinfo + persist resolve through the hardcoded
                                    // LUN map; start/num come from the device GPT via
                                    // `dump_partition_by_name`. Avoids re-decrypting
                                    // `rawprogram*.x` mid-flow when the catalog scratch
                                    // dir has been cleaned.
                                    const KNOWN_CODES: &[&str] = &[
                                        "CN","KR","JP","US","GB","DE","FR","IT","ES","NL",
                                        "AT","BE","BG","HR","CY","CZ","DK","EE","FI","GR",
                                        "HU","IE","LV","LT","LU","MT","PL","PT","RO","SK",
                                        "SI","SE","AU","CA","IN","RU","BR","MX","SA","AE",
                                        "WW",
                                    ];
                                    const EU_CODES: &[&str] = &[
                                        "AT","BE","BG","HR","CY","CZ","DK","EE","FI","FR",
                                        "DE","GR","HU","IE","IT","LV","LT","LU","MT","NL",
                                        "PL","PT","RO","SK","SI","ES","SE",
                                    ];
                                    let mut country_progress = CountryPatchProgress::default();
                                    for label in ["devinfo", "persist"] {
                                        let Some(lun) =
                                            ltbox_core::partition_lun::lun_for_partition(label)
                                        else {
                                            let reason = "no hardcoded LUN for label";
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", reason)
                                            );
                                            country_progress.mark_failed(label, reason);
                                            continue;
                                        };
                                        let dump_path = work_dir.join(format!("{label}.img"));
                                        live!(
                                            log,
                                            "[Country] {}",
                                            ltbox_core::i18n::tr("live_country_dump_partition")
                                                .replace("{label}", label)
                                                .replace("{lun}", &lun.to_string())
                                                .replace("{start}", "?")
                                                .replace("{sectors}", "?")
                                        );
                                        if let Err(e) = session.dump_partition(
                                            label, &dump_path, 0, lun, &mut log,
                                        ) {
                                            let reason = format!("dump failed: {e}");
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", &reason)
                                            );
                                            country_progress.mark_failed(label, reason);
                                            continue;
                                        }
                                        // Preserve the original partition
                                        // *before* any patch touches it.
                                        if let Err(e) = std::fs::copy(
                                            &dump_path,
                                            critical_backup.join(format!("{label}.img")),
                                        ) {
                                            let reason = format!("backup failed: {e}");
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", &reason)
                                            );
                                            country_progress.mark_failed(label, reason);
                                            continue;
                                        }
                                        let detected = match ltbox_patch::region::detect_country_code(
                                            &dump_path,
                                            KNOWN_CODES,
                                        ) {
                                            Ok(c) => c,
                                            Err(e) => {
                                                let reason = format!("detect failed: {e}");
                                                ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", &reason)
                                            );
                                                country_progress.mark_failed(label, reason);
                                                None
                                            }
                                        };
                                        let Some(old_code) = detected else {
                                            let reason = "no known code detected";
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", reason)
                                            );
                                            country_progress.mark_failed(label, reason);
                                            continue;
                                        };
                                        live!(
                                            log,
                                            "[Country] {}",
                                            ltbox_core::i18n::tr("live_country_patch_transition")
                                                .replace("{label}", label)
                                                .replace("{from}", &old_code)
                                                .replace("{to}", target_code)
                                        );
                                        let patched_path =
                                            work_dir.join(format!("{label}.patched.img"));
                                        match ltbox_patch::region::patch_country_code(
                                            &dump_path,
                                            &patched_path,
                                            &old_code,
                                            target_code,
                                            EU_CODES,
                                        ) {
                                            Ok(true) => {
                                                if let Err(e) = session.flash_partition(
                                                    label,
                                                    &patched_path,
                                                    0,
                                                    lun,
                                                    &mut log,
                                                ) {
                                                    ltbox_core::live!(
                                                        log,
                                                        "[Country] {}",
                                                        ltbox_core::i18n::tr("live_country_flash_failed")
                                                            .replace("{label}", label)
                                                            .replace("{error}", &e.to_string())
                                                    );
                                                    country_progress.mark_failed(
                                                        label,
                                                        format!("flash failed: {e}"),
                                                    );
                                                } else {
                                                    live!(
                                                        log,
                                                        "[Country] {}",
                                                        ltbox_core::i18n::tr("live_country_patched_flashed")
                                                            .replace("{label}", label)
                                                    );
                                                    country_progress.mark_flashed(label);
                                                }
                                            }
                                            Ok(false) if old_code == target_code => {
                                                ltbox_core::live!(
                                                    log,
                                                    "[Country] {}",
                                                    ltbox_core::i18n::tr("live_country_partition_already")
                                                        .replace("{label}", label)
                                                        .replace("{target}", target_code)
                                                );
                                                country_progress.mark_flashed(label);
                                            }
                                            Ok(false) => {
                                                let reason = "no replacements";
                                                ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", reason)
                                            );
                                                country_progress.mark_failed(label, reason);
                                            }
                                            Err(e) => {
                                                let reason = format!("patch failed: {e}");
                                                ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_partition_status")
                                                    .replace("{label}", label)
                                                    .replace("{reason}", &reason)
                                            );
                                                country_progress.mark_failed(label, reason);
                                            }
                                        }
                                    }
                                    if let Err(e) = country_progress.finish() {
                                        ltbox_core::live!(
                                            log,
                                            "[Country] {}",
                                            ltbox_core::i18n::tr("live_country_error")
                                                .replace("{error}", &e)
                                        );
                                        return Err(e);
                                    }
                                    // Surface the backup location once
                                    // per run. Empty dir = every label
                                    // was skipped.
                                    if std::fs::read_dir(&critical_backup)
                                        .map(|mut it| it.next().is_some())
                                        .unwrap_or(false)
                                    {
                                        live!(
                                            log,
                                            "[Country] {} {}",
                                            ll.backup_saved_prefix,
                                            critical_backup.display()
                                        );
                                    }
                                }

                            live!(log, "[Flash] {}", phase_marker(4, 4, &ll.op_flash_phase[3]));
                            session.reset_tolerant(&mut log);
                            live!(log, "[Flash] {}", ll.flash_completed);
                            Ok(log)
                            }).and_then(|r| r)
                        }).await.unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Flash(FlashMsg::FlashExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Flash(FlashMsg::FlashExecDone(lines)) => {
                // Extend *before* end_op so the END separator sits
                // below the backend's detail lines, not above them.
                self.flush_exec_done_log(lines);
                self.end_op();
                self.wf_config = WorkflowConfig::default();
            }
            // Country code popup
            Message::SelectCountry(code) => {
                self.country_popup_open = false;
                if self.adv_needs_country {
                    // Advanced wizard stores on `adv_wizard.country`.
                    self.adv_wizard.country = Some(code);
                    self.adv_needs_country = false;
                } else {
                    // Flash wizard: `wf_config` is source of truth.
                    self.wf_config.country_action = CountryAction::Set(code);
                }
            }
            Message::SkipCountryPatch => {
                // Flash wizard only — Advanced PatchDevinfo always needs a
                // target code, so the popup hides this option there.
                // `Skip` makes the exec gate skip the patch and the confirm
                // screen render the choice honestly.
                self.country_popup_open = false;
                if !self.adv_needs_country {
                    self.wf_config.country_action = CountryAction::Skip;
                }
            }
            Message::DismissCountryPopup => {
                self.country_popup_open = false;
                if self.adv_needs_country {
                    self.adv_needs_country = false;
                } else if matches!(self.wf_config.country_action, CountryAction::Unset) {
                    // Flash wizard — back to Data so user can switch wipe off.
                    self.flash.back();
                }
            }
            // Region-convert target picker popup
            Message::SelectRegionTarget(target) => {
                self.region_target_popup_open = false;
                self.adv_wizard.region_target = Some(target);
            }
            Message::DismissRegionTargetPopup => {
                self.region_target_popup_open = false;
            }
            // System Update wizard
            Message::Sys(SysMsg::SysAction(a)) => {
                // Switching action resets Rescue-specific state so a stale
                // folder/region can't leak into a fresh flow.
                self.sysupdate.action = Some(a);
                self.sysupdate.rescue_folder = None;
                self.sysupdate.rescue_region = None;
                self.sysupdate.rescue_region_popup_open = false;
            }
            Message::Sys(SysMsg::SysNext) => {
                // Rescue flow: Action(0) → Folder(1) → Confirm(2) → Exec(3).
                // Gate: popping the region popup between Folder and Confirm.
                if self.sysupdate.is_rescue() {
                    if self.sysupdate.step == 1 && self.sysupdate.rescue_region.is_none() {
                        self.sysupdate.rescue_region_popup_open = true;
                        return Task::none();
                    }
                    if self.sysupdate.step == 2 {
                        self.sysupdate.next();
                        return self.update(Message::Sys(SysMsg::SysExecStart));
                    }
                    self.sysupdate.next();
                } else {
                    // Disable/Enable: Action(0) → Confirm(1) → Exec(2).
                    if self.sysupdate.step == 1 {
                        self.sysupdate.next();
                        return self.update(Message::Sys(SysMsg::SysExecStart));
                    }
                    self.sysupdate.next();
                }
            }
            Message::Sys(SysMsg::SysBack) => self.sysupdate.back(),
            Message::Sys(SysMsg::SysRescueSelectFolder) => {
                // Rescue dump+flash resolves vendor_boot / vbmeta against
                // the device's on-storage GPT (LUN 0), so the wizard only
                // needs the EDL loader binary — `rawprogram*.xml` was
                // never read in this path. File picker with the standard
                // loader extension filter, recents shared with the rest
                // of the loader pickers via the File bucket.
                return self.pick_loader_with_default(|__v| {
                    Message::Sys(SysMsg::SysRescueFolderChosen(__v))
                });
            }
            Message::Sys(SysMsg::SysRescueFolderChosen(path)) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.sysupdate.rescue_folder = Some(p);
                    // Force re-pick of region when loader changes — a stale
                    // region from a prior firmware could target the wrong
                    // hardware.
                    self.sysupdate.rescue_region = None;
                }
            }
            Message::Sys(SysMsg::SysRescueRegion(r)) => {
                self.sysupdate.rescue_region = Some(r);
                self.sysupdate.rescue_region_popup_open = false;
                // Auto-advance out of Folder step into Confirm — picking
                // the region is the implicit "Next" of the popup.
                if self.sysupdate.step == 1 {
                    self.sysupdate.next();
                }
            }
            Message::Sys(SysMsg::SysRescueRegionPopupDismiss) => {
                self.sysupdate.rescue_region_popup_open = false;
            }
            Message::Sys(SysMsg::SysExecStart) => {
                let Some(action) = self.sysupdate.action else {
                    return Task::none();
                };
                // Rescue captures folder + region into the blocking task.
                // Cloning here keeps `self` untouched while the async move
                // takes ownership.
                let rescue_folder = self.sysupdate.rescue_folder.clone();
                let rescue_region = self.sysupdate.rescue_region;
                if action == SysUpdateAction::Rescue
                    && self.validate_loader_path(&rescue_folder).is_err()
                {
                    return Task::none();
                }
                // Capture model for AVB fingerprint validation — mirrors
                // v2 `_validate_device_model`, prevents flashing firmware
                // built for a different TB3xx variant.
                let device_model = self.device_model.clone();
                self.begin_op(View::SystemUpdate);
                self.error_msg = None;
                self.log_push(format!(
                    "[SysUpdate] Starting: {}",
                    self.t(action.label_key())
                ));
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let mut log = Vec::new();
                            let mut adb = ltbox_device::adb::AdbManager::new();
                            ltbox_core::live!(
                                log,
                                "[ADB] {}",
                                ltbox_core::i18n::tr("live_adb_checking_device")
                            );
                            if !adb.check_device().unwrap_or(false) {
                                return Err("No ADB device connected".to_string());
                            }
                            ltbox_core::live!(
                                log,
                                "[ADB] {}",
                                ltbox_core::i18n::tr("live_adb_device_connected")
                            );
                            let packages = [
                                "com.lenovo.ota",
                                "com.tblenovo.lenovowhatsnew",
                                "com.lenovo.tbengine",
                            ];
                            match action {
                                SysUpdateAction::Disable => {
                                    // Command echoes (`$ settings put …` / `$ pm clear …`)
                                    // were noise — the user only needs to see the outcome
                                    // (Uninstalled / Reinstalled / failure). Suppressed.
                                    adb.shell("settings put global ota_disable_automatic_update 1")
                                        .map_err(|e| e.to_string())?;
                                    adb.shell("settings put secure lenovo_ota_new_version_found 0")
                                        .map_err(|e| e.to_string())?;

                                    for pkg in &packages {
                                        let _ = adb.shell(&format!("pm clear {pkg}"));

                                        match adb.shell(&format!("pm uninstall -k --user 0 {pkg}")) {
                                            Ok(out) if out.contains("Success") => ltbox_core::live!(
                                                log,
                                                "[ADB] {}",
                                                ltbox_core::i18n::tr("live_adb_uninstalled")
                                                    .replace("{package}", pkg)
                                            ),
                                            Ok(out) => ltbox_core::live!(log, "[ADB] {pkg}: {out}"),
                                            Err(e) => ltbox_core::live!(log, "[ADB] {pkg}: {e}"),
                                        }
                                    }
                                    ltbox_core::live!(
                                        log,
                                        "[SysUpdate] {}",
                                        ltbox_core::i18n::tr("live_sysupdate_disabled")
                                    );
                                    Ok(log)
                                }
                                SysUpdateAction::Enable => {
                                    // Command echoes suppressed — same rationale as Disable.
                                    adb.shell("settings put global ota_disable_automatic_update 0")
                                        .map_err(|e| e.to_string())?;

                                    for pkg in &packages {
                                        match adb.shell(&format!("cmd package install-existing {pkg}")) {
                                            Ok(out) if out.to_lowercase().contains("installed") => ltbox_core::live!(
                                                log,
                                                "[ADB] {}",
                                                ltbox_core::i18n::tr("live_adb_reinstalled")
                                                    .replace("{package}", pkg)
                                            ),
                                            Ok(out) => ltbox_core::live!(log, "[ADB] {pkg}: {out}"),
                                            Err(e) => ltbox_core::live!(log, "[ADB] {pkg}: {e}"),
                                        }
                                    }
                                    ltbox_core::live!(
                                        log,
                                        "[SysUpdate] {}",
                                        ltbox_core::i18n::tr("live_sysupdate_enabled")
                                    );
                                    Ok(log)
                                }
                                SysUpdateAction::Rescue => {
                                    // Precondition: loader file + region
                                    // picked in the wizard.
                                    let Some(loader_path) = rescue_folder else {
                                        return Err(
                                            "Boot Recovery: EDL loader not selected".into(),
                                        );
                                    };
                                    let Some(region) = rescue_region else {
                                        return Err(
                                            "Boot Recovery: target region (PRC/ROW) not selected".into(),
                                        );
                                    };
                                    let loader = std::path::PathBuf::from(&loader_path);
                                    if !loader.is_file() {
                                        return Err(format!(
                                            "Boot Recovery: loader does not exist: {}",
                                            loader.display()
                                        ));
                                    }
                                    // User spec: extension-only check —
                                    // accept any `.melf` / `.mbn` / `.elf`
                                    // regardless of filename, mirroring the
                                    // root-pipeline rule.
                                    let ext_ok = loader
                                        .extension()
                                        .and_then(|e| e.to_str())
                                        .is_some_and(|e| {
                                            let l = e.to_ascii_lowercase();
                                            l == "melf" || l == "mbn" || l == "elf"
                                        });
                                    if !ext_ok {
                                        return Err(format!(
                                            "Boot Recovery: loader must be .melf / .mbn / .elf, got: {}",
                                            loader.display()
                                        ));
                                    }
                                    let loader_dir = loader
                                        .parent()
                                        .map(std::path::Path::to_path_buf)
                                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                                    ltbox_core::live!(
                                        log,
                                        "[Rescue] {}",
                                        ltbox_core::i18n::tr("live_rescue_loader")
                                            .replace("{path}", &loader.display().to_string())
                                    );
                                    ltbox_core::live!(log,
                                        "[Rescue] Target region: {}",
                                        match region {
                                            RescueRegion::Prc => "PRC",
                                            RescueRegion::Row => "ROW",
                                        }
                                    );

                                    // Stage dumps + patched outputs in a
                                    // timestamped temp dir next to the
                                    // loader so the user's loader directory
                                    // doesn't get cluttered with rescue
                                    // intermediates.
                                    let ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    let work_dir = loader_dir.join(format!("rescue_{ts}"));
                                    if let Err(e) = std::fs::create_dir_all(&work_dir) {
                                        return Err(format!("create work dir: {e}"));
                                    }
                                    ltbox_core::live!(log,
                                        "[Rescue] Work dir: {}",
                                        work_dir.display()
                                    );

                                    ltbox_core::live!(
                                        log,
                                        "[Rescue] {}",
                                        ltbox_core::i18n::tr("live_rescue_transitioning")
                                    );
                                    let _ = adb.reboot("edl");
                                    std::thread::sleep(std::time::Duration::from_secs(5));
                                    ltbox_core::live!(
                                        log,
                                        "[EDL] {}",
                                        ltbox_core::i18n::tr("live_edl_waiting_device")
                                    );
                                    ltbox_device::edl::wait_for_device()
                                        .map_err(|e| format!("EDL not found: {e}"))?;

                                    let mut session = ltbox_device::edl::EdlSession::open(
                                        &loader, true, &mut log,
                                    )
                                    .map_err(|e| format!("EDL open: {e}"))?;

                                    // vendor_boot + vbmeta on Lenovo TB3xx
                                    // Qualcomm UFS land on LUN 0; predefine
                                    // here so the dump + flash loops below
                                    // share one source of truth (root flow
                                    // does the same with `ROOT_PARTITIONS_LUN`
                                    // for boot/init_boot on LUN 4). GPT-by-name
                                    // resolves the actual sector geometry, so
                                    // no rawprogram*.xml is required.
                                    const RESCUE_PARTITIONS_LUN: u8 = 0;
                                    let slots = ["a", "b"];
                                    let mut dumped: Vec<(String, String, std::path::PathBuf)> =
                                        Vec::new();
                                    for slot in &slots {
                                        for base in &["vendor_boot", "vbmeta"] {
                                            let part_name = format!("{base}_{slot}");
                                            let out =
                                                work_dir.join(format!("{part_name}.img"));
                                            ltbox_core::live!(log,
                                                "[Rescue] Dumping {part_name}..."
                                            );
                                            if let Err(e) = session.dump_partition(
                                                &part_name, &out, 0, RESCUE_PARTITIONS_LUN, &mut log,
                                            ) {
                                                ltbox_core::live!(log,
                                                    "[Rescue] Skip {part_name}: {e}"
                                                );
                                                continue;
                                            }
                                            dumped.push((
                                                (*base).to_string(),
                                                (*slot).to_string(),
                                                out,
                                            ));
                                        }
                                    }

                                    if dumped.is_empty() {
                                        return Err(
                                            "Boot Recovery: no partitions dumped — aborting"
                                                .into(),
                                        );
                                    }

                                    // Cross-check firmware against device
                                    // model via AVB vendor_boot fingerprint —
                                    // aborts the whole rescue if the dumped
                                    // image was built for another model. Uses
                                    // the first available vendor_boot dump;
                                    // slot A/B carry the same fingerprint.
                                    if let Some(vb_probe) = dumped
                                        .iter()
                                        .find(|(b, _, _)| b == "vendor_boot")
                                    {
                                        match ltbox_patch::avb::extract_image_avb_info(
                                            &vb_probe.2,
                                        ) {
                                            Ok(info) => {
                                                use ltbox_patch::region::{
                                                    validate_device_model, ModelValidation,
                                                };
                                                match validate_device_model(
                                                    &info,
                                                    &device_model,
                                                ) {
                                                    ModelValidation::Match { fingerprint } => {
                                                        ltbox_core::live!(log,
                                                            "[Rescue] Model check OK (fingerprint={fingerprint})"
                                                        );
                                                    }
                                                    ModelValidation::Missing => {
                                                        ltbox_core::live!(
                                                            log,
                                                            "[Rescue] WARN: vendor_boot has no fingerprint property — skipping model check"
                                                        );
                                                    }
                                                    ModelValidation::Mismatch {
                                                        fingerprint,
                                                        device_model,
                                                    } => {
                                                        ltbox_core::live!(log,
                                                            "[Rescue] ABORT: model mismatch (device={device_model}, firmware fingerprint={fingerprint})"
                                                        );
                                                        session.reset_tolerant(&mut log);
                                                        return Err(
                                                            "Boot Recovery: firmware/device model mismatch".into(),
                                                        );
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                ltbox_core::live!(log,
                                                    "[Rescue] WARN: AVB inspect for model check failed: {e} — skipping"
                                                );
                                            }
                                        }
                                    }

                                    // Patch vendor_boot per region, rebuild
                                    // footer, rebuild vbmeta chain per slot.
                                    let target = region.to_target();
                                    let prc_dot = vec![0x2E, 0x50, 0x52, 0x43]; // ".PRC"
                                    let prc_i = vec![0x49, 0x50, 0x52, 0x43]; // "IPRC"
                                    let row_dot = vec![0x2E, 0x52, 0x4F, 0x57]; // ".ROW"
                                    let row_i = vec![0x49, 0x52, 0x4F, 0x57]; // "IROW"
                                    let prc_patterns: Vec<(Vec<u8>, Vec<u8>)> = vec![
                                        (prc_dot.clone(), row_dot.clone()),
                                        (prc_i.clone(), row_i.clone()),
                                    ];
                                    let row_patterns: Vec<(Vec<u8>, Vec<u8>)> = vec![
                                        (row_dot.clone(), prc_dot.clone()),
                                        (row_i.clone(), prc_i.clone()),
                                    ];

                                    let mut flash_plan: Vec<(String, std::path::PathBuf)> =
                                        Vec::new();
                                    for slot in &slots {
                                        let vb_src = dumped.iter().find(|(b, s, _)| {
                                            b == "vendor_boot" && s == slot
                                        });
                                        let vbm_src = dumped.iter().find(|(b, s, _)| {
                                            b == "vbmeta" && s == slot
                                        });
                                        let (Some(vb_src), Some(vbm_src)) = (vb_src, vbm_src)
                                        else {
                                            ltbox_core::live!(log,
                                                "[Rescue] Slot {slot}: missing dump — skipping"
                                            );
                                            continue;
                                        };

                                        let vb_patched = work_dir
                                            .join(format!("vendor_boot_{slot}.patched.img"));
                                        ltbox_core::live!(log,
                                            "[Rescue] Patching vendor_boot_{slot} → {}",
                                            match region {
                                                RescueRegion::Prc => "PRC",
                                                RescueRegion::Row => "ROW",
                                            }
                                        );
                                        let n = match ltbox_patch::region::patch_vendor_boot(
                                            &vb_src.2,
                                            &vb_patched,
                                            target,
                                            &prc_patterns,
                                            &row_patterns,
                                        ) {
                                            Ok(n) => n,
                                            Err(e) => {
                                                ltbox_core::live!(log,
                                                    "[Rescue] Slot {slot}: region patch failed: {e} — skipping"
                                                );
                                                continue;
                                            }
                                        };
                                        if n == 0 {
                                            ltbox_core::live!(log,
                                                "[Rescue] Slot {slot}: no region bytes changed — already on target region?"
                                            );
                                        } else {
                                            ltbox_core::live!(log,
                                                "[Rescue] Slot {slot}: {n} occurrences patched"
                                            );
                                        }

                                        // Rebuild AVB hash footer on the
                                        // patched vendor_boot using metadata
                                        // from the original.
                                        let vb_info =
                                            match ltbox_patch::avb::extract_image_avb_info(
                                                &vb_src.2,
                                            ) {
                                                Ok(i) => i,
                                                Err(e) => {
                                                    ltbox_core::live!(log,
                                                        "[Rescue] Slot {slot}: vendor_boot AVB inspect failed: {e}"
                                                    );
                                                    continue;
                                                }
                                            };
                                        // Only the two stock testkeys embedded in
                                        // avbtool-rs are supported.
                                        let vb_key_spec = ltbox_patch::key_map::key_spec_for_pubkey(
                                            vb_info.public_key_sha1.as_deref(),
                                        );
                                        if let Err(e) = ltbox_patch::avb::add_hash_footer(
                                            &vb_patched,
                                            &vb_info,
                                            vb_key_spec,
                                            None,
                                        ) {
                                            ltbox_core::live!(log,
                                                "[Rescue] Slot {slot}: add_hash_footer failed: {e} — skipping"
                                            );
                                            continue;
                                        }

                                        // Rebuild vbmeta chained to the
                                        // patched vendor_boot. Key fallback:
                                        // algorithm comes from the original
                                        // vbmeta header.
                                        let vbm_info =
                                            match ltbox_patch::avb::extract_image_avb_info(
                                                &vbm_src.2,
                                            ) {
                                                Ok(i) => i,
                                                Err(e) => {
                                                    ltbox_core::live!(
                                                        log,
                                                        "[Rescue] {}",
                                                        ltbox_core::i18n::tr("live_rescue_vbmeta_inspect_failed")
                                                            .replace("{slot}", slot)
                                                            .replace("{error}", &e.to_string())
                                                    );
                                                    continue;
                                                }
                                            };
                                        let Some(vbm_key) = ltbox_patch::key_map::key_spec_for_pubkey(
                                            vbm_info.public_key_sha1.as_deref(),
                                        )
                                        else {
                                            ltbox_core::live!(
                                                log,
                                                "[Rescue] {}",
                                                ltbox_core::i18n::tr("live_rescue_no_testkey")
                                                    .replace("{slot}", slot)
                                                    .replace("{path}", &loader_dir.display().to_string())
                                            );
                                            continue;
                                        };
                                        let vbm_rebuilt = work_dir
                                            .join(format!("vbmeta_{slot}.rebuilt.img"));
                                        let chained: [&std::path::Path; 1] =
                                            [vb_patched.as_path()];
                                        if let Err(e) =
                                            ltbox_patch::avb::rebuild_vbmeta_with_chained_images(
                                                &vbm_rebuilt,
                                                &vbm_src.2,
                                                &chained,
                                                vbm_key,
                                                Some(vbm_info.algorithm.as_str()),
                                            )
                                        {
                                            ltbox_core::live!(log,
                                                "[Rescue] Slot {slot}: rebuild vbmeta failed: {e} — skipping"
                                            );
                                            continue;
                                        }

                                        flash_plan.push((
                                            format!("vendor_boot_{slot}"),
                                            vb_patched,
                                        ));
                                        flash_plan
                                            .push((format!("vbmeta_{slot}"), vbm_rebuilt));
                                    }

                                    if flash_plan.is_empty() {
                                        return Err(
                                            "Boot Recovery: nothing to flash after patch/resign"
                                                .into(),
                                        );
                                    }

                                    ltbox_core::live!(log,
                                        "[Rescue] Flashing {} target(s)...",
                                        flash_plan.len()
                                    );
                                    for (part_name, image) in &flash_plan {
                                        if let Err(e) = session.flash_partition(
                                            part_name, image, 0, RESCUE_PARTITIONS_LUN, &mut log,
                                        ) {
                                            ltbox_core::live!(log,
                                                "[Rescue] Flash {part_name}: {e}"
                                            );
                                        }
                                    }

                                    ltbox_core::live!(
                                        log,
                                        "[Rescue] {}",
                                        ltbox_core::i18n::tr("live_rescue_resetting")
                                    );
                                    session.reset_tolerant(&mut log);
                                    ltbox_core::live!(
                                        log,
                                        "[Rescue] {}",
                                        ltbox_core::i18n::tr("live_rescue_complete")
                                    );
                                    Ok(log)
                                }
                            }
                        }).await.unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Sys(SysMsg::SysExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Sys(SysMsg::SysExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            // Root wizard
            Message::Root(RootMsg::RootFamily(f)) => {
                self.root.family = Some(f);
                self.root.provider = None;
                self.root.mode = None;
                self.root.file_path = None;
                self.root.kernel_version = None;
            }
            Message::Root(RootMsg::RootProvider(p)) => {
                self.root.provider = Some(p);
                self.root.file_path = None;
                // ReSukiSU has no Stable channel — if the user had Stable
                // picked before switching to ReSukiSU, force Nightly so the
                // hidden-Stable version step lands on the sole valid choice
                // instead of showing an orphan "no selection" state.
                if p == Provider::ReSukiSU && self.root.version == Some(VerChoice::Stable) {
                    self.root.version = Some(VerChoice::Nightly);
                    self.root.nightly_source = None;
                    self.root.run_id = None;
                    self.root.run_id_buffer.clear();
                }
            }
            Message::Root(RootMsg::RootMode(m)) => {
                self.root.mode = Some(m);
                self.root.file_path = None;
                self.root.kernel_version = None;
            }
            Message::Root(RootMsg::RootVersion(v)) => {
                self.root.version = Some(v);
                self.root.nightly_source = None;
                self.root.run_id = None;
                self.root.run_id_buffer.clear();
            }
            Message::Root(RootMsg::RootNightlySource(s)) => {
                self.root.nightly_source = Some(s);
                match s {
                    NightlySource::AutoDetect => {
                        // Leaving ManualInput — drop the committed run ID.
                        self.root.run_id = None;
                        self.root.run_id_buffer.clear();
                    }
                    NightlySource::ManualInput => {
                        // Prefill from any previous commit so re-entry is painless.
                        self.root.run_id_buffer = self.root.run_id.clone().unwrap_or_default();
                        self.root.run_id_popup_open = true;
                    }
                }
            }
            Message::Root(RootMsg::RootSelectFile) => {
                self.picker_target = PickerTarget::RootFile;
                let spec = if self.root.is_gki() {
                    pickers::FilePickSpec::single("picker_target_kernelsu_zip")
                        .with_filter("ZIP", &["zip"])
                } else {
                    pickers::FilePickSpec::single("picker_target_apatch_apk")
                        .with_filter("APK", &["apk"])
                };
                return pickers::pick_file_for(spec, &self.recent_paths, Message::FileSelected);
            }
            Message::Root(RootMsg::RootSelectFolder) => {
                // Name kept for backwards compat with existing view code;
                // the picker is now a single-file dialog for the EDL
                // loader (`.melf`) since root no longer needs a full
                // firmware folder — partitions resolve via the device's
                // GPT, not `rawprogram*.xml`.
                if let Some(path) = self.default_loader_path.clone() {
                    // Settings-level default loader → bypass picker, store
                    // in `folder_path` (historical field name) so the rest
                    // of the wizard reads it as if the user picked it.
                    self.root.folder_path = Some(path);
                    return Task::none();
                }
                self.picker_target = PickerTarget::RootLoader;
                return pickers::pick_file_for(
                    loader_file_spec("picker_target_edl_loader"),
                    &self.recent_paths,
                    Message::FileSelected,
                );
            }
            Message::Root(RootMsg::RootNext) => {
                if self.root.step == 6 {
                    if self.root.needs_ksu_lkm_kernel_version() {
                        // ADB probe is blocking — push to the heavy pool so
                        // the UI doesn't freeze on a slow / unresponsive
                        // device. Continuation lands in
                        // `RootKernelVersionProbeDone`.
                        return task_heavy(
                            || {
                                let mut adb = ltbox_device::adb::AdbManager::new();
                                if adb.check_device().unwrap_or(false) {
                                    adb.get_kernel_version().ok().flatten().and_then(|kv| {
                                        ltbox_patch::root_pipeline::normalize_ksu_kernel_version(
                                            &kv,
                                        )
                                    })
                                } else {
                                    None
                                }
                            },
                            |__v| Message::Root(RootMsg::RootKernelVersionProbeDone(__v)),
                            |_e| None,
                        );
                    }
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
                // APatch KPM step: open superkey popup — advance is
                // gated on a valid commit, not this press. Always start
                // on the first-entry stage; the existing committed key
                // (if any) isn't pre-filled because the user has to
                // re-type it twice anyway, which is the whole point of
                // the verification flow.
                if self.root.step == 8 {
                    self.root.superkey_buffer.clear();
                    self.root.superkey_first_entry = None;
                    self.root.superkey_popup_open = true;
                    return Task::none();
                }
                self.root.next();
                // After advancing, if the wizard landed on the loader
                // (folder) step and a Settings-level default loader is
                // configured + still on disk, pre-fill the folder slot
                // and skip to the Confirm step. The loader-step picker
                // becomes invisible to single-device users while staying
                // available to anyone with `default_loader_path = None`.
                if self.root.step == 5
                    && self.root.folder_path.is_none()
                    && let Some(path) = self.resolved_default_loader()
                {
                    self.root.folder_path = Some(path);
                    self.root.next();
                }
            }
            Message::Root(RootMsg::RootBack) => self.root.back(),
            Message::Root(RootMsg::RootSelectKpm) => {
                // Multi-select; paths merge-dedup into the list so
                // the user can Browse multiple times.
                let spec = pickers::FilePickSpec::multi("picker_target_kpm_modules")
                    .with_filter("KPM modules", &["kpm"]);
                return pickers::pick_files_for(spec, &self.recent_paths, |__v| {
                    Message::Root(RootMsg::RootKpmSelected(__v))
                });
            }
            Message::Root(RootMsg::RootKpmSelected(paths)) => {
                if let Some(paths) = paths {
                    if let Some(first) = paths.first() {
                        self.remember_recent(pickers::PickerKind::File, first);
                    }
                    for p in paths {
                        if !self.root.kpm_paths.iter().any(|existing| existing == &p) {
                            self.root.kpm_paths.push(p);
                        }
                    }
                }
            }
            Message::Root(RootMsg::RootKpmRemove(path)) => {
                self.root.kpm_paths.retain(|p| p != &path);
            }
            Message::Root(RootMsg::RootSuperkeyInput(text)) => {
                self.root.superkey_buffer = text;
            }
            Message::Root(RootMsg::RootSuperkeyConfirm) => {
                let key = self.root.superkey_buffer.trim().to_string();
                match self.root.superkey_first_entry.take() {
                    None => {
                        // Stage 1 — first entry. Validate the format
                        // up-front so the user finds out about a too-short
                        // / non-alnum key on the first round, not after
                        // re-typing it. Upstream rule: 8–63 alphanumeric.
                        let valid = (8..=63).contains(&key.len())
                            && key.chars().all(|c| c.is_ascii_alphanumeric());
                        if !valid {
                            self.error_msg = Some(self.t("apatch_superkey_invalid").to_string());
                            return Task::none();
                        }
                        // Stash the validated first entry, blank the
                        // field, and stay open for the verification
                        // round. View flips to the "re-enter" prompt
                        // because `superkey_first_entry.is_some()`.
                        self.root.superkey_first_entry = Some(key);
                        self.root.superkey_buffer.clear();
                        self.error_msg = None;
                    }
                    Some(first) => {
                        // Stage 2 — verification entry. Mismatch resets
                        // the whole flow so the user types both rounds
                        // again from scratch (no "edit second field"
                        // shortcut, since the typo could be in either).
                        if key != first {
                            self.error_msg = Some(self.t("apatch_superkey_mismatch").to_string());
                            self.root.superkey_buffer.clear();
                            // `superkey_first_entry` already cleared by
                            // the `.take()` above — stage flips back to
                            // first-entry automatically.
                            return Task::none();
                        }
                        self.root.superkey = Some(key);
                        self.root.superkey_buffer.clear();
                        self.root.superkey_popup_open = false;
                        self.error_msg = None;
                        self.root.next();
                    }
                }
            }
            Message::Root(RootMsg::RootSuperkeyCancel) => {
                self.root.superkey_buffer.clear();
                self.root.superkey_first_entry = None;
                self.root.superkey_popup_open = false;
                self.error_msg = None;
            }
            Message::Root(RootMsg::RootRunIdInput(text)) => {
                // GH Actions run IDs are 10 digits; cap at 12 for headroom.
                let filtered: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(12)
                    .collect();
                self.root.run_id_buffer = filtered;
            }
            Message::Root(RootMsg::RootRunIdConfirm) => {
                let id = self.root.run_id_buffer.trim().to_string();
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                    self.error_msg = Some(self.t("nightly_manual_invalid").to_string());
                    return Task::none();
                }
                self.root.run_id = Some(id);
                self.root.run_id_popup_open = false;
                self.error_msg = None;
            }
            Message::Root(RootMsg::RootRunIdCancel) => {
                self.root.run_id_buffer.clear();
                self.root.run_id_popup_open = false;
                // Roll back NightlySource so the step gate forces a re-pick.
                if self.root.run_id.is_none() {
                    self.root.nightly_source = None;
                }
            }
            Message::Root(RootMsg::RootKernelVersionInput(text)) => {
                let filtered: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                    .take(16)
                    .collect();
                self.root.kernel_version_buffer = filtered;
            }
            Message::Root(RootMsg::RootKernelVersionConfirm) => {
                let input = self.root.kernel_version_buffer.trim();
                let Some(kv) = ltbox_patch::root_pipeline::normalize_ksu_kernel_version(input)
                else {
                    self.error_msg = Some(self.t("root_kernel_version_invalid").to_string());
                    return Task::none();
                };
                self.root.kernel_version = Some(kv);
                self.root.kernel_version_buffer.clear();
                self.root.kernel_version_popup_open = false;
                self.error_msg = None;
                if self.root.step == 6 {
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
            }
            Message::Root(RootMsg::RootKernelVersionCancel) => {
                self.root.kernel_version_buffer.clear();
                self.root.kernel_version_popup_open = false;
            }
            Message::Root(RootMsg::RootKernelVersionProbeDone(detected)) => {
                // Wizard may have moved off step 6 by the time the probe
                // returns (user clicked Back); only act if still at the
                // same gating point.
                if self.root.step != 6 || !self.root.needs_ksu_lkm_kernel_version() {
                    return Task::none();
                }
                if let Some(kv) = detected {
                    self.root.kernel_version = Some(kv);
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
                self.root.kernel_version_buffer =
                    self.root.kernel_version.clone().unwrap_or_default();
                self.root.kernel_version_popup_open = true;
            }
            Message::Root(RootMsg::RootExecStart) => {
                if self
                    .validate_loader_path(&self.root.folder_path.clone())
                    .is_err()
                {
                    return Task::none();
                }
                self.begin_op(View::Root);
                self.op_steps = self.derive_root_op_steps();
                self.error_msg = None;
                let family = self.root.family;
                let mode = self.root.mode;
                let provider = self.root.provider;
                let version = self.root.version;
                let file_path = self.root.file_path.clone();
                let gui_kernel_version = self.root.kernel_version.clone();
                let conn = self.connection;
                // Folder must contain `xbl_s_devprg_ns.melf`; optional
                // `keys/testkey_rsa{2048,4096}.pem` as KEY_MAP fallback.
                let fw_folder = self.root.folder_path.clone();
                // APatch-only; empty / default elsewhere.
                let kpm_paths: Vec<std::path::PathBuf> = self
                    .root
                    .kpm_paths
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                let superkey = self.root.superkey.clone().unwrap_or_default();
                let nightly_run_id: Option<u64> =
                    if self.root.nightly_source == Some(NightlySource::ManualInput) {
                        self.root.run_id.as_deref().and_then(|s| s.parse().ok())
                    } else {
                        None
                    };

                let fam_label = family
                    .map(|f| self.t(f.label_key()).to_string())
                    .unwrap_or_else(|| "?".to_string());
                self.log_push(format!(
                    "[Root] {}",
                    self.t("log_op_starting").replace("{what}", &fam_label)
                ));
                // Resolve Magisk preinit device via /proc/self/mountinfo
                // before ADB vanishes past EDL. Gates /data on the device's
                // encryption state — metadata-encrypted devices land preinit
                // on userdata otherwise and boot-loop after first wipe.
                let preinit_device: String = if matches!(family, Some(Family::Magisk))
                    && matches!(
                        self.connection,
                        ConnectionStatus::Adb | ConnectionStatus::AdbRecovery
                    ) {
                    let mut adb = ltbox_device::adb::AdbManager::new();
                    let (mountinfo, encrypt_type) = if adb.check_device().unwrap_or(false) {
                        let mi = adb.shell("cat /proc/self/mountinfo").unwrap_or_default();
                        let cs = adb.shell("getprop ro.crypto.state").unwrap_or_default();
                        let ct = adb.shell("getprop ro.crypto.type").unwrap_or_default();
                        let cme = adb
                            .shell("getprop ro.crypto.metadata.enabled")
                            .unwrap_or_default();
                        (
                            mi,
                            ltbox_patch::magisk::derive_encrypt_type(&cs, &ct, &cme).to_string(),
                        )
                    } else {
                        (String::new(), String::from("file"))
                    };
                    if mountinfo.is_empty() {
                        self.log_push("[Magisk] Preinit device: (ADB unavailable — falling back to runtime detection)".to_string());
                        String::new()
                    } else {
                        self.log_push(format!(
                            "[Magisk] Crypto state: encrypt_type={encrypt_type}"
                        ));
                        match ltbox_patch::magisk::resolve_preinit_device(&mountinfo, &encrypt_type)
                        {
                            Some(name) => {
                                self.log_push(format!("[Magisk] Preinit device: {name} (resolved from /proc/self/mountinfo)"));
                                name
                            }
                            None => {
                                self.log_push("[Magisk] Preinit device: (none detected — Magisk will fall back at runtime)".to_string());
                                String::new()
                            }
                        }
                    }
                } else {
                    String::new()
                };
                let ll = self.live_labels();

                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || -> Result<Vec<String>, String> {
                            let mut log = Vec::new();
                            let skip_adb = conn.skip_adb();

                            // GKI route: AnyKernel3 zip is the full input —
                            // no provider / version / GitHub fetch.
                            let is_gki_route = mode == Some(RootMode::Gki);
                            let family = family.ok_or_else(|| "No root family selected".to_string())?;
                            let (provider, version) = if is_gki_route {
                                // `Magisk` stand-in — picks magiskboot as
                                // the backend for unpack/repack.
                                (Provider::Magisk, VerChoice::Stable)
                            } else {
                                let prov = provider.ok_or_else(|| "No provider selected".to_string())?;
                                let ver = version.ok_or_else(|| "No version selected".to_string())?;
                                (prov, ver)
                            };

                            use ltbox_patch::root_pipeline::{
                                RootFamily, RootPipelineConfig, RootProvider, RootVersion,
                                build_patched_artifacts, ensure_nightly_run_id,
                                stage_root_manager_apk, stage_root_payload,
                            };

                            let pipe_family = match family {
                                Family::Magisk => RootFamily::Magisk,
                                Family::KernelSU => RootFamily::KernelSU,
                                Family::APatch => RootFamily::APatch,
                            };
                            let pipe_provider = match provider {
                                Provider::Magisk => RootProvider::Magisk,
                                Provider::MagiskForks => RootProvider::MagiskFork,
                                Provider::KernelSU => RootProvider::KernelSU,
                                Provider::KernelSUNext => RootProvider::KernelSUNext,
                                Provider::SukiSU => RootProvider::SukiSU,
                                Provider::ReSukiSU => RootProvider::ReSukiSU,
                                Provider::APatch => RootProvider::APatch,
                                Provider::FolkPatch => RootProvider::FolkPatch,
                            };
                            let pipe_version = match version {
                                VerChoice::Stable => RootVersion::Stable,
                                VerChoice::Nightly => RootVersion::Nightly,
                            };
                            let file_path_buf: Option<std::path::PathBuf> =
                                file_path.as_ref().map(std::path::PathBuf::from);

                            let loader_path = fw_folder.ok_or_else(|| {
                                "No EDL loader selected. Pick an xbl_s_devprg_ns.melf \
(or equivalent `.melf`) file on the Loader step and retry."
                                    .to_string()
                            })?;
                            let loader = std::path::PathBuf::from(&loader_path);
                            if !loader.is_file() {
                                return Err(format!(
                                    "Selected loader does not exist: {}",
                                    loader.display()
                                ));
                            }
                            // User spec: match on `.melf` extension only —
                            // filename itself is free-form, so no
                            // `xbl_s_devprg_ns`-equals check.
                            let is_melf = loader
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(|e| e.eq_ignore_ascii_case("melf"));
                            if !is_melf {
                                return Err(format!(
                                    "Selected loader must be a .melf file, got: {}",
                                    loader.display()
                                ));
                            }
                            // Signing key: pipeline resolves via KEY_MAP
                            // + `public_key_sha1`; PEM is `include_str!`'d
                            // in avbtool-rs. No on-disk key consulted here.
                            ltbox_core::live!(
                                log,
                                "[Root] {}",
                                ltbox_core::i18n::tr("log_root_loader")
                                    .replace("{path}", &loader.display().to_string())
                            );

                            let base = ltbox_core::app_paths::work_dir_for("root");
                            let work_dir = base.join("work");
                            let output_dir = base.join("out");
                            let _ = std::fs::remove_dir_all(&work_dir);
                            std::fs::create_dir_all(&work_dir)
                                .map_err(|e| format!("work dir: {e}"))?;
                            std::fs::create_dir_all(&output_dir)
                                .map_err(|e| format!("out dir: {e}"))?;

                            // Phase 1/7 — ADB connect + slot/kver detect.
                            // Front-loaded so the user sees something happen
                            // before the long manager-APK / payload download.
                            live!(log, "[Root] {}", phase_marker(1, 7, &ll.op_root_phase[0]));
                            // Slot detection MUST succeed — root flashes
                            // boot_<slot> + vbmeta_<slot> + init_boot_<slot>,
                            // and silently defaulting to `_a` previously
                            // landed flashes on the wrong slot when the
                            // device was actually running on `_b`. Poll
                            // both ADB + Fastboot up to 30 s; on failure,
                            // the helper returns a diagnostic that names
                            // which transport last failed and what to do
                            // (re-plug into normal/recovery, reboot to
                            // bootloader, fix unauthorized ADB, …).
                            let slot_suffix = ltbox_device::controller::poll_active_slot(
                                std::time::Duration::from_secs(30),
                                &mut log,
                            )?;
                            // Kernel version probe (KSU LKM) needs ADB
                            // shell; runs only when ADB is currently
                            // usable so the slot-resolved-via-Fastboot
                            // path doesn't waste 30 s waiting for a
                            // shell that won't come.
                            let mut kernel_version: Option<String> = gui_kernel_version.clone();
                            let mut adb_ready_at_start = false;
                            if !skip_adb {
                                let mut adb = ltbox_device::adb::AdbManager::new();
                                if adb.check_device().unwrap_or(false) {
                                    adb_ready_at_start = true;
                                    if mode == Some(RootMode::Lkm) {
                                        if let Ok(Some(kv)) = adb.get_kernel_version() {
                                            let normalized =
                                                ltbox_patch::root_pipeline::normalize_ksu_kernel_version(&kv);
                                            live!(
                                                log,
                                                "[ADB] {}",
                                                ltbox_core::i18n::tr("live_adb_kernel_version")
                                                    .replace(
                                                        "{version}",
                                                        normalized.as_deref().unwrap_or(&kv),
                                                    )
                                            );
                                            if let Some(kv) = normalized {
                                                kernel_version = Some(kv);
                                            }
                                        } else {
                                            live!(log, "[ADB] {}", ll.adb_no_kver);
                                        }
                                    }
                                }
                            }
                            if mode == Some(RootMode::Lkm) && kernel_version.is_none() {
                                return Err(
                                    "KernelSU LKM requires kernel version before EDL; enter it manually and retry."
                                        .to_string(),
                                );
                            }

                            let mut manager_cfg = RootPipelineConfig {
                                family: pipe_family,
                                provider: pipe_provider,
                                version: pipe_version,
                                work_dir: work_dir.clone(),
                                output_dir: output_dir.clone(),
                                loader: loader.clone(),
                                slot_suffix: slot_suffix.clone(),
                                preinit_device: preinit_device.clone(),
                                kernel_version: kernel_version.clone(),
                                gki_kernel_zip: if is_gki_route { file_path_buf.clone() } else { None },
                                gki_mode: is_gki_route,
                                kpm_paths: kpm_paths.clone(),
                                superkey: superkey.clone(),
                                magisk_forks_apk: if matches!(pipe_provider, RootProvider::MagiskFork) {
                                    file_path_buf.clone()
                                } else {
                                    None
                                },
                                nightly_run_id,
                            };
                            // Phase 2/7 — Download EVERY root file before
                            // EDL: manager APK + per-family payload
                            // (Magisk APK extract / KSU `.ko`+`init` /
                            // APatch APK→kpimg). Used to split the .ko +
                            // ksuinit fetch into Phase 5; that hid a
                            // multi-second network stall behind a
                            // "patching" label and blocked the user
                            // from copying the device serial. Single
                            // download burst now, then offline patch.
                            live!(log, "[Root] {}", phase_marker(2, 7, &ll.op_root_phase[1]));
                            // Pin the nightly workflow run ID once so
                            // every fetch in this Phase 2 pulls from
                            // the SAME upstream build. Without this,
                            // a new workflow landing between the
                            // ~minute-long manager APK download and
                            // the .ko/ksuinit fetch would split the
                            // installed manager APK across two
                            // different builds.
                            ensure_nightly_run_id(&mut manager_cfg, &mut log)
                                .map_err(|e| format!("Nightly run resolve: {e}"))?;
                            let nightly_run_id = manager_cfg.nightly_run_id;
                            let mut manager_apk = stage_root_manager_apk(&manager_cfg, &mut log)
                                .map_err(|e| format!("Manager APK: {e}"))?;
                            stage_root_payload(&manager_cfg, &mut log)
                                .map_err(|e| format!("Root payload: {e}"))?;
                            let manager_installed_pre_edl = if adb_ready_at_start {
                                if let Some(path) = manager_apk.as_ref() {
                                    install_root_manager_apk(path, &mut log)?;
                                    true
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            // Wrap the device-interaction phase (phase 1/6 onwards)
                            // so any error triggers a best-effort EDL → system reset
                            // before we bubble the error up. Without this, a failure
                            // mid-pipeline (e.g. patch errors out) leaves the device
                            // stuck in 9008 mode and the user has to yank the cable
                            // + battery to recover. `log` / `loader` are captured by
                            // reference so both the success and failure paths still
                            // see the accumulated lines.
                            let device_phase_result: std::result::Result<(), String> = (|| -> std::result::Result<(), String> {
                            // Phase 3/7 — Reboot to EDL (was Phase 1/6).
                            live!(log, "[Root] {}", phase_marker(3, 7, &ll.op_root_phase[2]));
                            transition_to_edl(&ll, &mut log)?;

                            // Partition naming: `boot{_a|_b}` for GKI + APatch
                            // (kernel-blob patching) and `init_boot{_a|_b}` for
                            // Magisk / KSU (ramdisk injection). Slot is derived
                            // from ADB/Fastboot pre-EDL; on devices without an
                            // active-slot getvar we default to `_a`.
                            //
                            // Root pipeline no longer consumes `rawprogram*.xml`
                            // — `EdlSession::{dump,flash}_partition` resolves
                            // geometry via the device's on-storage GPT using
                            // these names, matching the equivalent one-shot
                            // `qdl-rs dump-part <name>` / `write <name> <img>`
                            // invocations a user would run by hand.
                            let is_gki_mode = is_gki_route;
                            let base_name = ltbox_patch::root_pipeline::boot_partition_base(pipe_family, is_gki_mode);
                            // `slot_suffix` was poll-resolved at Phase 1
                            // and propagated through `RootPipelineConfig`;
                            // it is guaranteed to be `_a` or `_b` here.
                            let boot_primary = format!("{base_name}{slot_suffix}");
                            let vbmeta_primary = format!("vbmeta{slot_suffix}");
                            // Lenovo devices on Qualcomm UFS place
                            // boot / init_boot / vbmeta on LUN 4 (userdata
                            // LUN), same index used by the reference
                            // `qdl-rs --phys-part-idx 4` recipe.
                            const ROOT_PARTITIONS_LUN: u8 = 4;
                            live!(
                                log,
                                "[Root] {} {} / {} (LUN {ROOT_PARTITIONS_LUN})",
                                ll.root_resolved_prefix,
                                boot_primary,
                                vbmeta_primary,
                            );

                            // Phase 4/7 — Read stock images (was Phase 2/6).
                            live!(log, "[Root] {}", phase_marker(4, 7, &ll.op_root_phase[3]));
                            // Hoisted so Phase 6 can echo the path.
                            // Routed through `app_paths::backup_dir_for`
                            // so AppImage / distro Linux installs don't
                            // try to write next to the executable.
                            let backup_dir = ltbox_core::app_paths::backup_dir_for(
                                &format!("backup_{base_name}"),
                            );
                            {
                                let mut session = ltbox_device::edl::EdlSession::open(&loader, false, &mut log)
                                    .map_err(|e| format!("EDL session: {e}"))?;
                                // Patch pipeline hardcodes `init_boot.img` /
                                // `vbmeta.img` regardless of device label.
                                let boot_out = if base_name == "boot" { "boot.img" } else { "init_boot.img" };
                                let dumped_boot = work_dir.join(boot_out);
                                let dumped_vbmeta = work_dir.join("vbmeta.img");
                                // `dump_partition` scans the LUN's GPT for the
                                // named partition — matches the shell-level
                                // `qdl-rs --phys-part-idx 4 dump-part <name>`.
                                session.dump_partition(&boot_primary, &dumped_boot, 0, ROOT_PARTITIONS_LUN, &mut log)
                                    .map_err(|e| format!("Dump {boot_primary}: {e}"))?;
                                session.dump_partition(&vbmeta_primary, &dumped_vbmeta, 0, ROOT_PARTITIONS_LUN, &mut log)
                                    .map_err(|e| format!("Dump {vbmeta_primary}: {e}"))?;
                                // Dump backup next to `ltbox.exe` for Unroot.
                                let _ = std::fs::create_dir_all(&backup_dir);
                                let _ = std::fs::copy(&dumped_boot, backup_dir.join(boot_out));
                                let _ = std::fs::copy(&dumped_vbmeta, backup_dir.join("vbmeta.img"));
                                live!(
                                    log,
                                    "[Root] {} {} + vbmeta.img → {}",
                                    ll.root_backup_copy_prefix,
                                    boot_out,
                                    backup_dir.display()
                                );
                                // Bounce to Sahara — otherwise the second
                                // session's sahara_run times out because
                                // the device is still in Firehose.
                                session.reset_to_edl(&mut log)
                                    .map_err(|e| format!("reset_to_edl: {e}"))?;
                                // Terminate any dangling pbr `\r`-only
                                // line so the next message gets a fresh row.
                                println!();
                                live!(log, "[EDL] {}", ll.closing_dump);
                                // Drop session — serial port closes so
                                // the post-patch open gets a fresh handle.
                            }

                            // Phase 5/7 — Offline patch + AVB resign +
                            // vbmeta rebuild. Network downloads moved
                            // up to Phase 2; this step never touches
                            // the network so progress now matches the
                            // "patching" label.
                            live!(log, "[Root] {}", phase_marker(5, 7, &ll.op_root_phase[4]));

                            let cfg = RootPipelineConfig {
                                family: pipe_family,
                                provider: pipe_provider,
                                version: pipe_version,
                                work_dir: work_dir.clone(),
                                output_dir: output_dir.clone(),
                                loader: loader.clone(),
                                slot_suffix: slot_suffix.clone(),
                                preinit_device: preinit_device.clone(),
                                kernel_version: kernel_version.clone(),
                                gki_kernel_zip: if is_gki_route { file_path_buf.clone() } else { None },
                                gki_mode: is_gki_route,
                                kpm_paths: kpm_paths.clone(),
                                superkey: superkey.clone(),
                                magisk_forks_apk: if matches!(pipe_provider, RootProvider::MagiskFork) {
                                    file_path_buf.clone()
                                } else {
                                    None
                                },
                                nightly_run_id,
                            };
                            let artifacts = build_patched_artifacts(&cfg, &mut log)
                                .map_err(|e| format!("Root patch: {e}"))?;
                            if manager_apk.is_none() {
                                manager_apk = artifacts.manager_apk.clone();
                            }
                            // Phase 6/7 — Write patched images (was Phase
                            // 5/6). Old standalone Phase 4 marker dropped
                            // since there was no real work between it and
                            // flash open — collapsed into this one phase.
                            live!(log, "[Root] {}", phase_marker(6, 7, &ll.op_root_phase[5]));
                            let mut session = ltbox_device::edl::EdlSession::open(&loader, true, &mut log)
                                .map_err(|e| format!("EDL session (flash): {e}"))?;
                            // Mirror of the equivalent one-shot `qdl-rs
                            // --phys-part-idx 4 write <name> <img>` — GPT
                            // resolves the start sector, so no rawprogram
                            // sector attrs to thread through.
                            session
                                .flash_partition(&boot_primary, &artifacts.patched_boot, 0, ROOT_PARTITIONS_LUN, &mut log)
                                .map_err(|e| format!("Flash {boot_primary}: {e}"))?;
                            if let Some(vbpath) = &artifacts.patched_vbmeta {
                                session
                                    .flash_partition(&vbmeta_primary, vbpath, 0, ROOT_PARTITIONS_LUN, &mut log)
                                    .map_err(|e| format!("Flash {vbmeta_primary}: {e}"))?;
                            }
                            println!();
                            // Phase 7/7 — Reboot to system (was Phase 6/6).
                            live!(log, "[Root] {}", phase_marker(7, 7, &ll.op_root_phase[6]));
                            // Surface the backup folder before the reset
                            // so the user doesn't have to scroll.
                            if backup_dir.exists() {
                                live!(
                                    log,
                                    "[Root] {} {}",
                                    ll.backup_saved_prefix,
                                    backup_dir.display()
                                );
                            }
                            session.reset_tolerant(&mut log);
                            if !manager_installed_pre_edl
                                && let Some(path) = manager_apk.as_ref()
                            {
                                wait_and_install_root_manager_apk(
                                    path,
                                    std::time::Duration::from_secs(60),
                                    &mut log,
                                )
                                .map_err(|e| format!("Manager APK install after reboot failed: {e}"))?;
                            }
                            live!(log, "[Root] {}", ll.root_completed);
                            Ok(())
                            })();
                            match device_phase_result {
                                Ok(()) => {
                                    // Success path only: drop the
                                    // `AppData\Roaming\ltbox\root`
                                    // staging tree (work_dir + out)
                                    // so a stale ~30 MB Magisk APK +
                                    // dumped boot/vbmeta blobs from a
                                    // previous run don't accumulate
                                    // on disk indefinitely. On error
                                    // we KEEP it — having the dumped
                                    // stock images, downloaded payload
                                    // and intermediate patched files
                                    // around makes post-mortem
                                    // debugging tractable.
                                    let _ = std::fs::remove_dir_all(&base);
                                    Ok(log)
                                }
                                Err(e) => {
                                    // Best-effort: open a fresh session on the same
                                    // loader and ask the device to boot. `reset_tolerant`
                                    // already swallows the post-handoff error some
                                    // devices return, so this never masks the real
                                    // error — failures here are only logged.
                                    let mut reset_log: Vec<String> = Vec::new();
                                    reset_log.push(format!(
                                        "[EDL] attempting device reset after error: {e}"
                                    ));
                                    if let Ok(mut s) = ltbox_device::edl::EdlSession::open(
                                        &loader,
                                        false,
                                        &mut reset_log,
                                    ) {
                                        s.reset_tolerant(&mut reset_log);
                                    } else {
                                        reset_log.push(
                                            "[EDL] reset skipped — could not re-open EDL session".into(),
                                        );
                                    }
                                    for line in reset_log {
                                        println!("{line}");
                                    }
                                    Err(e)
                                }
                            }
                            }).and_then(|r| r)
                        }).await.unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Root(RootMsg::RootExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Root(RootMsg::RootExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            // Unroot wizard
            Message::Unroot(UnrootMsg::SetUnrootType(t)) => self.unroot.unroot_type = Some(t),
            Message::Unroot(UnrootMsg::UnrootSelectFolder) => {
                self.picker_target = PickerTarget::UnrootFolder;
                return pick_folder_task(
                    pickers::PickerKind::QfilFirmwareFolder,
                    &self.recent_paths,
                    Message::FolderSelected,
                );
            }
            Message::Unroot(UnrootMsg::UnrootSelectLoader) => {
                return self.pick_loader_with_default(|__v| {
                    Message::Unroot(UnrootMsg::UnrootLoaderChosen(__v))
                });
            }
            Message::Unroot(UnrootMsg::UnrootLoaderChosen(path)) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.unroot.loader_path = Some(p);
                }
            }
            Message::Unroot(UnrootMsg::UnrootNext) => {
                if self.unroot.step == 2 {
                    self.unroot.next();
                    return self.update(Message::Unroot(UnrootMsg::UnrootExecStart));
                }
                self.unroot.next();
            }
            Message::Unroot(UnrootMsg::UnrootBack) => self.unroot.back(),
            Message::Unroot(UnrootMsg::UnrootExecStart) => {
                let Some(unroot_type) = self.unroot.unroot_type else {
                    return Task::none();
                };
                let Some(folder) = self.unroot.folder_path.clone() else {
                    return Task::none();
                };
                // Loader is decoupled from the backup folder — `folder`
                // holds boot.img + vbmeta.img, the loader can live
                // anywhere (Settings default, or whatever the user
                // pointed the loader picker at). `validate_loader_path`
                // surfaces a missing-file error before the device-side
                // work starts, matching the other wizards' behaviour.
                let loader_override =
                    match self.validate_loader_path(&self.unroot.loader_path.clone()) {
                        Ok(p) => Some(p),
                        Err(()) => return Task::none(),
                    };
                self.begin_op(View::Unroot);
                self.op_steps = self.derive_unroot_op_steps();
                self.error_msg = None;
                self.log_push(format!(
                    "[Unroot] {}",
                    self.t("log_op_starting")
                        .replace("{what}", self.t(unroot_type.label_key()))
                ));
                let ll = self.live_labels();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(
                                move || -> Result<Vec<String>, String> {
                                    let mut log = Vec::new();
                                    let dir = std::path::Path::new(&folder);

                                    let (boot_name, base_part) = match unroot_type {
                                        UnrootType::MagiskLkm => ("init_boot.img", "init_boot"),
                                        UnrootType::APatchGki => ("boot.img", "boot"),
                                    };
                                    let boot_path = dir.join(boot_name);
                                    let vbmeta_path = dir.join("vbmeta.img");
                                    if !boot_path.exists() {
                                        return Err(format!(
                                            "{boot_name} not found in selected folder"
                                        ));
                                    }
                                    if !vbmeta_path.exists() {
                                        return Err(
                                            "vbmeta.img not found in selected folder".to_string()
                                        );
                                    }
                                    live!(
                                        log,
                                        "[Unroot] {}",
                                        ltbox_core::i18n::tr("live_unroot_backup_pair")
                                            .replace("{boot}", boot_name)
                                    );

                                    // Slot resolution must succeed —
                                    // unroot writes init_boot_<slot> +
                                    // vbmeta_<slot> from the user's
                                    // backup folder. Defaulting to `_a`
                                    // when the device was on `_b`
                                    // restored stale stock blobs to the
                                    // wrong slot and left the active
                                    // slot still rooted, with no clear
                                    // signal to the user.
                                    let slot = ltbox_device::controller::poll_active_slot(
                                        std::time::Duration::from_secs(30),
                                        &mut log,
                                    )
                                    .map_err(|e| format!("Unroot slot resolve: {e}"))?;

                                    // Decoupled loader — explicit picker /
                                    // Settings default takes priority. Fall back
                                    // to scanning the backup folder only when no
                                    // override was set, preserving v3-pre-decouple
                                    // behaviour for users who still ship a loader
                                    // alongside the backup images.
                                    let loader = match loader_override.clone() {
                                        Some(p) => std::path::PathBuf::from(p),
                                        None => find_edl_loader(dir)
                                            .or_else(|| dir.parent().and_then(find_edl_loader))
                                            .ok_or_else(|| {
                                                format!(
                                                    "xbl_s_devprg_ns.melf not found under {}",
                                                    dir.display()
                                                )
                                            })?,
                                    };
                                    live!(
                                        log,
                                        "[Unroot] {}",
                                        ltbox_core::i18n::tr("live_unroot_loader_path")
                                            .replace("{path}", &loader.display().to_string())
                                    );

                                    // Boot + vbmeta resolve through the
                                    // hardcoded LUN map; GPT-by-name reads
                                    // the slot's start sector from the
                                    // device. No rawprogram parse needed —
                                    // the loader's parent dir may not even
                                    // contain a firmware XML pair.
                                    let boot_label = format!("{base_part}{slot}");
                                    let vbm_label = format!("vbmeta{slot}");
                                    let boot_lun = ltbox_core::partition_lun::lun_for_partition(
                                        base_part,
                                    )
                                    .ok_or_else(|| {
                                        format!("No hardcoded LUN for {base_part}")
                                    })?;
                                    let vbm_lun = ltbox_core::partition_lun::lun_for_partition(
                                        "vbmeta",
                                    )
                                    .ok_or_else(|| "No hardcoded LUN for vbmeta".to_string())?;
                                    live!(
                                        log,
                                        "[Unroot] {boot_label} (LUN {boot_lun}) / {vbm_label} (LUN {vbm_lun}) via hardcoded map"
                                    );

                                    live!(
                                        log,
                                        "[Unroot] {}",
                                        phase_marker(1, 3, &ll.op_unroot_phase[0])
                                    );
                                    transition_to_edl(&ll, &mut log)?;

                                    live!(
                                        log,
                                        "[Unroot] {} ({})",
                                        phase_marker(2, 3, &ll.op_unroot_phase[1]),
                                        ltbox_core::i18n::tr("live_unroot_backup_pair")
                                            .replace("{boot}", boot_name)
                                    );
                                    let mut session = ltbox_device::edl::EdlSession::open(
                                        &loader, true, &mut log,
                                    )
                                    .map_err(|e| format!("EDL session error: {e}"))?;
                                    session
                                        .flash_partition(
                                            &boot_label,
                                            &boot_path,
                                            0,
                                            boot_lun,
                                            &mut log,
                                        )
                                        .map_err(|e| format!("Flash {boot_label} failed: {e}"))?;
                                    session
                                        .flash_partition(
                                            &vbm_label,
                                            &vbmeta_path,
                                            0,
                                            vbm_lun,
                                            &mut log,
                                        )
                                        .map_err(|e| format!("Flash {vbm_label} failed: {e}"))?;

                                    println!();
                                    live!(
                                        log,
                                        "[Unroot] {}",
                                        phase_marker(3, 3, &ll.op_unroot_phase[2])
                                    );
                                    session
                                        .reset(&mut log)
                                        .map_err(|e| format!("Reset failed: {e}"))?;
                                    live!(log, "[Unroot] {}", ll.unroot_completed);
                                    Ok(log)
                                },
                            )
                            .and_then(|r| r)
                        })
                        .await
                        .unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Unroot(UnrootMsg::UnrootExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Unroot(UnrootMsg::UnrootExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            // Advanced
            Message::Adv(AdvMsg::AdvConfirm(a)) => {
                // Flash/Dump Partitions + Physical Storage preempt the
                // grid with their own dedicated wizards. After the
                // wizard's `_open` flag flips, pull in the Settings
                // default loader if one is configured + still on disk;
                // that pre-fills the wizard's loader slot and advances
                // past the loader step so the user doesn't have to
                // re-pick the same `xbl_s_devprg_ns.melf` for every
                // single-device flow.
                if matches!(a, AdvAction::FlashPartitions) {
                    self.flash_parts.reset();
                    self.advanced_wizard_open = AdvancedWizardOpen::FlashParts;
                    return self.apply_default_loader_to_advanced_wizard();
                } else if matches!(a, AdvAction::DumpPartitions) {
                    self.dump_parts.reset();
                    self.advanced_wizard_open = AdvancedWizardOpen::DumpParts;
                    return self.apply_default_loader_to_advanced_wizard();
                } else if matches!(a, AdvAction::DumpPhysical) {
                    self.dump_phys.reset();
                    self.advanced_wizard_open = AdvancedWizardOpen::DumpPhys;
                    return self.apply_default_loader_to_advanced_wizard();
                } else if matches!(a, AdvAction::FlashPhysical) {
                    self.flash_phys.reset();
                    self.advanced_wizard_open = AdvancedWizardOpen::FlashPhys;
                    return self.apply_default_loader_to_advanced_wizard();
                } else {
                    return self.update(Message::Adv(AdvMsg::AdvWizOpen(a)));
                }
            }
            Message::Adv(AdvMsg::AdvWizOpen(a)) => {
                self.adv_wizard.open(a);
                // Mirror into legacy fields so AdvFileSelected /
                // AdvExecDone keep working unchanged.
                self.adv_confirm = Some(a);
                self.adv_confirm_path = None;
            }
            Message::Adv(AdvMsg::AdvWizBack) => {
                if self.adv_wizard.step == 0 {
                    // Back on step 0 closes the wizard.
                    self.adv_wizard.reset();
                    self.adv_confirm = None;
                    self.adv_confirm_path = None;
                } else {
                    self.adv_wizard.back();
                }
            }
            Message::Adv(AdvMsg::AdvWizNext) => {
                if self.adv_wizard.is_image_info() && self.adv_wizard.step == 0 {
                    self.adv_wizard.next();
                    return self.update(Message::Adv(AdvMsg::AdvImageInfoExecStart));
                }
                // DetectArb: source step Next jumps straight to exec.
                // The source step renders either a loader picker (only
                // when ADB/fastboot already identified the device as
                // TB320FC, since that is the model that needs the EDL
                // fallback) or a plain Start prompt.
                if matches!(self.adv_wizard.action, Some(AdvAction::DetectArb))
                    && self.adv_wizard.step == 0
                {
                    self.adv_wizard.next();
                    return self.update(Message::Adv(AdvMsg::AdvDetectArbExecStart));
                }
                // PatchArb: source step Next reads the AVB rollback
                // indices from the picked folder + advances to the
                // inspect step. Inspect step Next opens the timestamp
                // popup; the popup OK is what advances to the confirm
                // step.
                if matches!(self.adv_wizard.action, Some(AdvAction::PatchArb)) {
                    if self.adv_wizard.step == 0 {
                        let Some(folder) = self.adv_wizard.file_path.clone() else {
                            return Task::none();
                        };
                        let dir = std::path::PathBuf::from(&folder);
                        let boot = dir.join("boot.img");
                        let vbmeta = dir.join("vbmeta_system.img");
                        if !boot.is_file() {
                            self.error_msg = Some(format!("Missing boot.img in {}", dir.display()));
                            return Task::none();
                        }
                        if !vbmeta.is_file() {
                            self.error_msg =
                                Some(format!("Missing vbmeta_system.img in {}", dir.display()));
                            return Task::none();
                        }
                        let boot_info = match ltbox_patch::avb::extract_image_avb_info(&boot) {
                            Ok(i) => i,
                            Err(e) => {
                                self.error_msg = Some(format!("boot.img inspect failed: {e}"));
                                return Task::none();
                            }
                        };
                        let vbmeta_info = match ltbox_patch::avb::extract_image_avb_info(&vbmeta) {
                            Ok(i) => i,
                            Err(e) => {
                                self.error_msg =
                                    Some(format!("vbmeta_system.img inspect failed: {e}"));
                                return Task::none();
                            }
                        };
                        self.adv_wizard.arb_inspect =
                            Some((boot_info.rollback_index, vbmeta_info.rollback_index));
                        self.error_msg = None;
                        self.adv_wizard.next();
                        return Task::none();
                    }
                    if self.adv_wizard.step == 1 {
                        self.adv_wizard.arb_index_buffer = self
                            .adv_wizard
                            .arb_index_committed
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        self.arb_index_popup_open = true;
                        return Task::none();
                    }
                }
                if self.adv_wizard.is_confirm_step() {
                    let Some(action) = self.adv_wizard.action else {
                        return Task::none();
                    };
                    self.adv_confirm_path = self.adv_wizard.file_path.clone();
                    if let Some(code) = self.adv_wizard.country.clone() {
                        self.wf_config.country_action = CountryAction::Set(code);
                    }
                    // Pre-create output folder so the Done card's
                    // "Open Folder" pill always points somewhere real.
                    if action.produces_output() {
                        let dir = adv_output_dir(action);
                        let _ = std::fs::create_dir_all(&dir);
                        self.adv_wizard.output_dir = Some(dir);
                    } else {
                        self.adv_wizard.output_dir = None;
                    }
                    self.adv_wizard.next();
                    return self.update(Message::Adv(AdvMsg::AdvExec(action)));
                }
                self.adv_wizard.next();
            }
            Message::Adv(AdvMsg::AdvWizBrowse) => {
                if self.adv_wizard.is_image_info() {
                    let spec =
                        pickers::FilePickSpec::multi(self.adv_wizard.picker_target_i18n_key())
                            .with_filter("Android image (*.img)", &["img"]);
                    return pickers::pick_files_for(spec, &self.recent_paths, |__v| {
                        Message::Adv(AdvMsg::AdvWizBrowseManyDone(__v))
                    });
                }
                let kind = self.adv_wizard.picker_kind();
                if kind.is_folder() {
                    return pick_folder_task(kind, &self.recent_paths, |__v| {
                        Message::Adv(AdvMsg::AdvWizBrowseDone(__v))
                    });
                }
                let (filter_label, filter_exts) = self.adv_wizard.accepted_exts();
                let target_key = self.adv_wizard.picker_target_i18n_key();
                let mut spec = pickers::FilePickSpec::single(target_key);
                if !filter_exts.is_empty() {
                    spec = spec.with_filter(filter_label, filter_exts);
                }
                return pickers::pick_file_for(spec, &self.recent_paths, |__v| {
                    Message::Adv(AdvMsg::AdvWizBrowseDone(__v))
                });
            }
            Message::Adv(AdvMsg::AdvWizBrowseDone(path)) => {
                if let Some(p) = path {
                    if std::path::Path::new(&p).exists() {
                        // Kind is derived from the action (folder ops →
                        // folder bucket, file ops → File) rather than the
                        // runtime is_dir() check — trusting the action
                        // keeps buckets consistent even if rfd returns an
                        // unexpected path type.
                        self.remember_recent(self.adv_wizard.picker_kind(), &p);
                    }
                    self.adv_wizard.file_path = Some(p);
                }
            }
            Message::Adv(AdvMsg::AdvWizBrowseManyDone(paths)) => {
                if let Some(paths) = paths {
                    let paths: Vec<String> = paths
                        .into_iter()
                        .filter(|p| {
                            std::path::Path::new(p)
                                .extension()
                                .and_then(|s| s.to_str())
                                .map(|s| s.eq_ignore_ascii_case("img"))
                                .unwrap_or(false)
                        })
                        .collect();
                    for p in &paths {
                        if std::path::Path::new(p).exists() {
                            self.remember_recent(pickers::PickerKind::File, p);
                        }
                    }
                    self.adv_wizard.file_paths = paths;
                    self.adv_wizard.file_path = None;
                }
            }
            Message::Adv(AdvMsg::AdvWizOpenCountry) => {
                self.adv_needs_country = true;
                self.country_popup_open = true;
            }
            Message::Adv(AdvMsg::AdvWizOpenRegionTarget) => {
                self.region_target_popup_open = true;
            }
            Message::Adv(AdvMsg::AdvWizOpenOutputFolder) => {
                if let Some(dir) = self.adv_wizard.output_dir.clone()
                    && let Err(err) = open_in_file_manager(&dir)
                {
                    // Surface the failed command + path in the log
                    // so the user can see what was tried — silent
                    // no-op was the old behaviour and made missing
                    // xdg-open invisible on Linux.
                    self.log_push(format!("[GUI] Open Folder failed: {err}"));
                }
            }
            Message::Adv(AdvMsg::AdvWizArbIndexInput(s)) => {
                // Strip non-digits + cap at 10 chars so paste-of-garbage
                // can't smuggle a longer / non-numeric value past the UI.
                let cleaned: String = s.chars().filter(|c| c.is_ascii_digit()).take(10).collect();
                self.adv_wizard.arb_index_buffer = cleaned;
            }
            Message::Adv(AdvMsg::AdvWizArbIndexConfirm) => {
                let buf = self.adv_wizard.arb_index_buffer.clone();
                if buf.len() != 10 {
                    return Task::none();
                }
                let Ok(parsed) = buf.parse::<u64>() else {
                    return Task::none();
                };
                self.adv_wizard.arb_index_committed = Some(parsed);
                self.adv_wizard.arb_index_buffer.clear();
                self.arb_index_popup_open = false;
                // Advance to Confirm.
                self.adv_wizard.next();
            }
            Message::Adv(AdvMsg::AdvWizArbIndexCancel) => {
                self.adv_wizard.arb_index_buffer.clear();
                self.arb_index_popup_open = false;
            }
            Message::Adv(AdvMsg::AdvExec(action)) => {
                // Picker ran in AdvConfirm; replay the saved path.
                let Some(path) = self.adv_confirm_path.clone() else {
                    self.adv_confirm = None;
                    return Task::none();
                };
                return self.update(Message::Adv(AdvMsg::AdvFileSelected(action, Some(path))));
            }
            Message::Adv(AdvMsg::AdvFileSelected(action, path)) => {
                if let Some(input_path) = path {
                    // See AdvWizBrowseDone — trust the action's kind over
                    // the runtime is_dir() probe.
                    self.remember_recent(self.adv_wizard.picker_kind(), &input_path);
                    self.begin_op(View::Advanced);
                    self.error_msg = None;
                    let action_label = self.t(action.label_key()).to_string();
                    self.log_push(format!("[Advanced] {}: {}", action_label, input_path));
                    let _conn = self.connection;
                    // PatchDevinfo only — unused otherwise.
                    let adv_country: Option<String> =
                        self.wf_config.country_action.target().map(str::to_string);
                    // RegionConvert only — user-picked target.
                    let adv_region_target: Option<DeviceRegion> = self.adv_wizard.region_target;
                    // PatchArb only — committed unix-timestamp index.
                    let adv_arb_index: Option<u64> = self.adv_wizard.arb_index_committed;
                    let output_dir: std::path::PathBuf = self
                        .adv_wizard
                        .output_dir
                        .clone()
                        .unwrap_or_else(|| adv_output_dir(action));
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                ltbox_core::runtime::run_heavy(move || -> Result<Vec<String>, String> {
                                let mut log = Vec::new();
                                let input = std::path::Path::new(&input_path);
                                let parent = input.parent().unwrap_or(std::path::Path::new("."));
                                // Created eagerly so a no-op exec still
                                // leaves a folder for the user to find.
                                if action.produces_output() {
                                    let _ = std::fs::create_dir_all(&output_dir);
                                    ltbox_core::live!(log,
                                        "[Advanced] Output folder: {}",
                                        output_dir.display()
                                    );
                                }
                                match action {
                                    AdvAction::ImageInfo => {
                                        return Err(
                                            "Image Info uses a dedicated multi-file flow"
                                                .to_string(),
                                        );
                                    }
                                    AdvAction::ConvertXml => {
                                        // `input` is now the folder holding the encrypted
                                        // `*.x` pack (picker moved from file→folder so
                                        // users don't have to repeat the dialog for each
                                        // file). Iterate every `*.x`, decrypt to `*.xml`
                                        // in `output_dir`.
                                        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(input)
                                            .map_err(|e| format!("read_dir {}: {e}", input.display()))?
                                            .filter_map(|r| r.ok().map(|e| e.path()))
                                            .filter(|p| {
                                                p.is_file()
                                                    && p.extension()
                                                        .and_then(|s| s.to_str())
                                                        .map(|s| s.eq_ignore_ascii_case("x"))
                                                        .unwrap_or(false)
                                            })
                                            .collect();
                                        entries.sort();
                                        if entries.is_empty() {
                                            return Err(format!(
                                                "No *.x files found under {}",
                                                input.display()
                                            ));
                                        }
                                        for src in entries {
                                            let stem = src.file_stem().unwrap_or_default();
                                            let output = output_dir.join(stem).with_extension("xml");
                                            match ltbox_core::crypto::decrypt_file(&src, &output) {
                                                Ok(size) => ltbox_core::live!(
                                                    log,
                                                    "[Crypto] {}",
                                                    ltbox_core::i18n::tr("live_crypto_decrypted")
                                                        .replace("{bytes}", &size.to_string())
                                                ),
                                                Err(e) => return Err(format!("Decryption failed: {e}")),
                                            }
                                        }
                                    }
                                    AdvAction::DetectArb => {
                                        // DetectArb routes through its dedicated
                                        // `AdvDetectArbExecStart` worker, not the
                                        // generic file-selected pipeline. Reaching
                                        // this arm means a stale code path triggered
                                        // it; surface a clear error instead of a
                                        // silent no-op.
                                        return Err(
                                            "DetectArb uses a dedicated worker — file pipeline should not run"
                                                .to_string(),
                                        );
                                    }
                                    AdvAction::FlashPartitions
                                    | AdvAction::DumpPartitions
                                    | AdvAction::FlashPhysical
                                    | AdvAction::DumpPhysical => {
                                        ltbox_core::live!(
                                            log,
                                            "[Advanced] {}",
                                            ltbox_core::i18n::tr("live_advanced_use_dedicated")
                                        );
                                    }
                                    AdvAction::RegionConvert => {
                                        let Some(target_region) = adv_region_target else {
                                            return Err(
                                                "No target region selected — pick PRC or ROW in the popup before starting"
                                                    .into(),
                                            );
                                        };
                                        if input
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .map(|s| !s.eq_ignore_ascii_case("vendor_boot.img"))
                                            .unwrap_or(true)
                                        {
                                            return Err(
                                                "Region Convert expects vendor_boot.img; select the firmware folder's vendor_boot.img"
                                                    .to_string(),
                                            );
                                        }
                                        let firmware_dir = parent;
                                        let sibling_vbmeta = firmware_dir.join("vbmeta.img");
                                        if !sibling_vbmeta.is_file() {
                                            return Err(format!(
                                                "Region Convert requires vbmeta.img beside vendor_boot.img; missing {}",
                                                sibling_vbmeta.display()
                                            ));
                                        }
                                        let target = target_region.to_region_target();
                                        match ltbox_patch::region::build_region_converted_boot_chain(
                                            firmware_dir,
                                            &output_dir,
                                            target,
                                            &ltbox_patch::region::RegionPatternSet::default(),
                                        ) {
                                            Ok(ltbox_patch::region::RegionBootChainBuild::Built(output)) => {
                                                ltbox_core::live!(
                                                    log,
                                                    "[Region] {}",
                                                    ltbox_core::i18n::tr("live_region_source_target")
                                                        .replace("{source}", &format!("{:?}", output.source_region))
                                                        .replace("{target}", &format!("{:?}", output.target))
                                                );
                                                ltbox_core::live!(
                                                    log,
                                                    "[Region] {}",
                                                    ltbox_core::i18n::tr("live_region_patched")
                                                        .replace("{count}", &output.replacement_count.to_string())
                                                        .replace("{path}", &output.vendor_boot.display().to_string())
                                                );
                                                ltbox_core::live!(
                                                    log,
                                                    "[Region] Final vbmeta written: {}",
                                                    output.vbmeta.display()
                                                );
                                            }
                                            Ok(ltbox_patch::region::RegionBootChainBuild::Skipped {
                                                source_region,
                                                target,
                                            }) => {
                                                ltbox_core::live!(
                                                    log,
                                                    "[Region] {}",
                                                    ltbox_core::i18n::tr("live_region_source_target")
                                                        .replace("{source}", &format!("{:?}", source_region))
                                                        .replace("{target}", &format!("{:?}", target))
                                                );
                                                ltbox_core::live!(
                                                    log,
                                                    "[Region] {}",
                                                    ltbox_core::i18n::tr("live_region_source_matches_target")
                                                );
                                            }
                                            Err(e) => return Err(format!("Region conversion failed: {e}")),
                                        }
                                    }
                                    AdvAction::PatchDevinfo => {
                                        // Country code lives in both devinfo.img
                                        // + persist.img — folder picker, at
                                        // least one must exist.
                                        const KNOWN: &[&str] = &[
                                            "CN", "KR", "JP", "US", "GB", "DE", "FR", "IT", "ES", "NL",
                                            "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "GR",
                                            "HU", "IE", "LV", "LT", "LU", "MT", "PL", "PT", "RO", "SK",
                                            "SI", "SE", "AU", "CA", "IN", "RU", "BR", "MX", "SA", "AE",
                                            "WW",
                                        ];
                                        const EU: &[&str] = &[
                                            "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR",
                                            "DE", "GR", "HU", "IE", "IT", "LV", "LT", "LU", "MT", "NL",
                                            "PL", "PT", "RO", "SK", "SI", "ES", "SE",
                                        ];
                                        let Some(new_code) = adv_country.as_deref() else {
                                            return Err(
                                                "No target country code selected — pick one in the popup before starting"
                                                    .into(),
                                            );
                                        };
                                        if !input.is_dir() {
                                            return Err(format!(
                                                "PatchDevinfo expects a folder containing devinfo.img + persist.img, got {}",
                                                input.display()
                                            ));
                                        }
                                        let mut any_written = false;
                                        let mut any_found = false;
                                        for name in ["devinfo.img", "persist.img"] {
                                            let src = input.join(name);
                                            if !src.exists() {
                                                ltbox_core::live!(
                                                    log,
                                                    "[Country] {}",
                                                    ltbox_core::i18n::tr("live_country_name_missing")
                                                        .replace("{name}", name)
                                                );
                                                continue;
                                            }
                                            any_found = true;
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_processing")
                                                    .replace("{path}", &src.display().to_string())
                                            );
                                            let detected = ltbox_patch::region::detect_country_code(&src, KNOWN)
                                                .map_err(|e| format!("Country detect failed on {name}: {e}"))?;
                                            let Some(old_code) = detected else {
                                                ltbox_core::live!(
                                                    log,
                                                    "[Country] {}",
                                                    ltbox_core::i18n::tr("live_country_no_code_detected")
                                                        .replace("{name}", name)
                                                );
                                                continue;
                                            };
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_detected")
                                                    .replace("{name}", name)
                                                    .replace("{old_code}", &old_code)
                                            );
                                            let stem = std::path::Path::new(name)
                                                .file_stem()
                                                .map(|s| s.to_string_lossy().to_string())
                                                .unwrap_or_else(|| name.to_string());
                                            // v2 naming: `<stem>_modified.img`.
                                            let output = output_dir.join(format!("{stem}_modified.img"));
                                            match ltbox_patch::region::patch_country_code(&src, &output, &old_code, new_code, EU) {
                                                Ok(true) => {
                                                    ltbox_core::live!(
                                                        log,
                                                        "[Country] {}",
                                                        ltbox_core::i18n::tr("live_country_written")
                                                            .replace("{name}", name)
                                                            .replace("{old_code}", &old_code)
                                                            .replace("{new_code}", new_code)
                                                            .replace("{path}", &output.display().to_string())
                                                    );
                                                    any_written = true;
                                                }
                                                Ok(false) => ltbox_core::live!(
                                                    log,
                                                    "[Country] {}",
                                                    ltbox_core::i18n::tr("live_country_no_replacements")
                                                        .replace("{name}", name)
                                                ),
                                                Err(e) => return Err(format!(
                                                    "Country patch failed on {name}: {e}"
                                                )),
                                            }
                                        }
                                        if !any_found {
                                            return Err(format!(
                                                "Neither devinfo.img nor persist.img found in {}",
                                                input.display()
                                            ));
                                        }
                                        if !any_written {
                                            ltbox_core::live!(
                                                log,
                                                "[Country] {}",
                                                ltbox_core::i18n::tr("live_country_already_matches")
                                            );
                                        }
                                    }
                                    AdvAction::PatchArb => {
                                        // `input` is the firmware folder; user-picked
                                        // target rollback index lives on the wizard.
                                        let target = adv_arb_index.ok_or_else(|| {
                                            "Patch Rollback Index: missing target index".to_string()
                                        })?;
                                        let boot = input.join("boot.img");
                                        let vbmeta = input.join("vbmeta_system.img");
                                        if !boot.is_file() {
                                            return Err(format!(
                                                "Missing boot.img in {}",
                                                input.display()
                                            ));
                                        }
                                        if !vbmeta.is_file() {
                                            return Err(format!(
                                                "Missing vbmeta_system.img in {}",
                                                input.display()
                                            ));
                                        }
                                        // Read AVB info first so the abort guards (rollback
                                        // == 0 / 1) trip before any signing-key work runs.
                                        let boot_info = ltbox_patch::avb::extract_image_avb_info(&boot)
                                            .map_err(|e| format!("boot.img inspect failed: {e}"))?;
                                        let vbmeta_info = ltbox_patch::avb::extract_image_avb_info(&vbmeta)
                                            .map_err(|e| format!("vbmeta_system.img inspect failed: {e}"))?;
                                        if boot_info.rollback_index <= 1 {
                                            return Err(format!(
                                                "boot.img rollback index is {} — refusing to patch",
                                                boot_info.rollback_index
                                            ));
                                        }
                                        if vbmeta_info.rollback_index <= 1 {
                                            return Err(format!(
                                                "vbmeta_system.img rollback index is {} — refusing to patch",
                                                vbmeta_info.rollback_index
                                            ));
                                        }
                                        // Signing key resolution: only the two stock
                                        // testkeys embedded in avbtool-rs are supported.
                                        // Anything else aborts — user-supplied PEMs are
                                        // intentionally not consulted.
                                        let resolve_key = |info: &ltbox_patch::avb::AvbImageInfo,
                                                           label: &str|
                                         -> std::result::Result<&'static str, String> {
                                            ltbox_patch::key_map::key_spec_for_pubkey(
                                                info.public_key_sha1.as_deref(),
                                            )
                                            .ok_or_else(|| {
                                                format!(
                                                    "{label}: signing key not recognized (pubkey {:?}); only testkey_rsa2048 / testkey_rsa4096 are supported",
                                                    info.public_key_sha1
                                                )
                                            })
                                        };
                                        let boot_key = resolve_key(&boot_info, "boot.img")?;
                                        let vbmeta_key =
                                            resolve_key(&vbmeta_info, "vbmeta_system.img")?;
                                        ltbox_core::live!(log, "[ARB] boot.img signing key: {boot_key}");
                                        ltbox_core::live!(
                                            log,
                                            "[ARB] vbmeta_system.img signing key: {vbmeta_key}"
                                        );
                                        ltbox_core::live!(
                                            log,
                                            "[ARB] boot.img rollback {} → {target}",
                                            boot_info.rollback_index
                                        );
                                        ltbox_core::live!(
                                            log,
                                            "[ARB] vbmeta_system.img rollback {} → {target}",
                                            vbmeta_info.rollback_index
                                        );
                                        let boot_out = output_dir.join("boot.img");
                                        let vbmeta_out = output_dir.join("vbmeta_system.img");
                                        // boot.img: NONE → add_hash_footer; signed → resign.
                                        std::fs::copy(&boot, &boot_out)
                                            .map_err(|e| format!("copy boot.img: {e}"))?;
                                        if boot_info.algorithm == "NONE" {
                                            ltbox_patch::avb::add_hash_footer(
                                                &boot_out,
                                                &boot_info,
                                                Some(boot_key),
                                                Some(target),
                                            )
                                            .map_err(|e| format!("boot ARB add_hash_footer failed: {e}"))?;
                                        } else {
                                            ltbox_patch::avb::resign_image(
                                                &boot_out,
                                                boot_key,
                                                &boot_info.algorithm,
                                                Some(target),
                                            )
                                            .map_err(|e| format!("boot ARB resign failed: {e}"))?;
                                        }
                                        // vbmeta_system.img: always resign (chains require sig).
                                        std::fs::copy(&vbmeta, &vbmeta_out)
                                            .map_err(|e| format!("copy vbmeta_system.img: {e}"))?;
                                        ltbox_patch::avb::resign_image(
                                            &vbmeta_out,
                                            vbmeta_key,
                                            &vbmeta_info.algorithm,
                                            Some(target),
                                        )
                                        .map_err(|e| format!("vbmeta_system ARB resign failed: {e}"))?;
                                        ltbox_core::live!(
                                            log,
                                            "[ARB] Output folder: {}",
                                            output_dir.display()
                                        );
                                    }
                                    AdvAction::RebuildVbmeta => {
                                        // `resign_image` alone won't work — chain
                                        // hashes go stale once dtbo / init_boot /
                                        // vendor_boot move.
                                        let info = ltbox_patch::avb::extract_image_avb_info(input)
                                            .map_err(|e| format!("VBMeta inspect failed: {e}"))?;
                                        // Only the two stock testkeys embedded in
                                        // avbtool-rs are supported.
                                        let key_spec = ltbox_patch::key_map::key_spec_for_pubkey(
                                            info.public_key_sha1.as_deref(),
                                        )
                                        .ok_or_else(|| {
                                            format!(
                                                "Rebuild vbmeta: signing key not recognized (pubkey {:?}); only testkey_rsa2048 / testkey_rsa4096 are supported",
                                                info.public_key_sha1
                                            )
                                        })?;
                                        let alg: Option<&str> = if info.algorithm == "NONE" {
                                            // NONE → infer from the resolved key spec.
                                            Some(if key_spec.contains("2048") {
                                                "SHA256_RSA2048"
                                            } else {
                                                "SHA256_RSA4096"
                                            })
                                        } else {
                                            Some(info.algorithm.as_str())
                                        };

                                        // Advanced is file-only — user supplies
                                        // the chained images (v2 dumps them).
                                        let candidates: &[&str] = &[
                                            "dtbo.img", "dtbo_a.img", "dtbo_b.img",
                                            "init_boot.img", "init_boot_a.img", "init_boot_b.img",
                                            "vendor_boot.img", "vendor_boot_a.img", "vendor_boot_b.img",
                                            "boot.img", "boot_a.img", "boot_b.img",
                                        ];
                                        let mut chained: Vec<std::path::PathBuf> = Vec::new();
                                        for name in candidates {
                                            let p = parent.join(name);
                                            if p.exists() {
                                                chained.push(p);
                                            }
                                        }
                                        if chained.is_empty() {
                                            ltbox_core::live!(
                                                log,
                                                "[AVB] {}",
                                                ltbox_core::i18n::tr("live_avb_no_chained_fallback")
                                            );
                                            if let Err(e) = ltbox_patch::avb::resign_image(
                                                input,
                                                key_spec,
                                                alg.unwrap_or("SHA256_RSA4096"),
                                                Some(info.rollback_index),
                                            ) {
                                                return Err(format!("Rebuild vbmeta fallback resign failed: {e}"));
                                            }
                                        } else {
                                            if chained.iter().any(|p| {
                                                p.file_name()
                                                    .and_then(|s| s.to_str())
                                                    .map(|s| s.starts_with("vendor_boot"))
                                                    .unwrap_or(false)
                                            }) {
                                                ltbox_core::live!(
                                                    log,
                                                    "[AVB] Warning: Rebuild vbmeta does not repair chained image footers; use Region Convert output for modified vendor_boot.img"
                                                );
                                            }
                                            let output = output_dir.join("vbmeta.rebuilt.img");
                                            let chained_refs: Vec<&std::path::Path> =
                                                chained.iter().map(|p| p.as_path()).collect();
                                            ltbox_core::live!(
                                                log,
                                                "[AVB] $ rebuild_vbmeta with {} chained image(s): {}",
                                                chained.len(),
                                                chained
                                                    .iter()
                                                    .map(|p| p.file_name().and_then(|s| s.to_str()).unwrap_or(""))
                                                    .collect::<Vec<_>>()
                                                    .join(", ")
                                            );
                                            ltbox_core::live!(
                                                log,
                                                "[AVB] key={key_spec} algorithm={}",
                                                alg.unwrap_or("(from original vbmeta)"),
                                            );
                                            if let Err(e) = ltbox_patch::avb::rebuild_vbmeta_with_chained_images(
                                                &output,
                                                input,
                                                &chained_refs,
                                                key_spec,
                                                alg,
                                            ) {
                                                return Err(format!("Rebuild vbmeta failed: {e}"));
                                            }
                                            ltbox_core::live!(
                                                log,
                                                "[AVB] {}",
                                                ltbox_core::i18n::tr("live_avb_rebuilt_written")
                                                    .replace("{path}", &output.display().to_string())
                                            );
                                        }
                                    }
                                }
                                ltbox_core::live!(
                                    log,
                                    "[Advanced] {}",
                                    ltbox_core::i18n::tr("live_advanced_completed")
                                        .replace("{action}", &action_label)
                                );
                                Ok(log)
                                }).and_then(|r| r)
                            }).await.unwrap_or(Err("Task failed".to_string()))
                        },
                        |result| match result {
                            Ok(lines) => Message::Adv(AdvMsg::AdvExecDone(lines)),
                            Err(e) => Message::OperationError(e),
                        },
                    );
                }
                self.adv_confirm = None;
            }
            Message::Adv(AdvMsg::AdvExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                // Leave adv_wizard / adv_confirm* intact so the exec
                // screen stays visible with Done/Failed until StartOver.
                self.end_op();
            }
            Message::Adv(AdvMsg::AdvImageInfoExecStart) => {
                let paths: Vec<std::path::PathBuf> = self
                    .adv_wizard
                    .file_paths
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                let scanning = self
                    .t("adv_image_info_scanning")
                    .replace("{count}", &paths.len().to_string());
                self.set_image_info_log(scanning);
                self.begin_silent_op(View::Advanced);
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || {
                                ltbox_patch::avb::image_info_report(&paths)
                                    .map_err(|e| e.to_string())
                            })
                            .and_then(|r| r)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
                    },
                    |__v| Message::Adv(AdvMsg::AdvImageInfoExecDone(__v)),
                );
            }
            Message::Adv(AdvMsg::AdvImageInfoExecDone(result)) => {
                self.end_silent_op();
                match result {
                    Ok(report) => {
                        self.error_msg = None;
                        self.set_image_info_log(report);
                    }
                    Err(e) => {
                        self.error_msg = Some(e.clone());
                        self.set_image_info_log(format!("ERROR: {e}"));
                    }
                }
            }
            Message::Adv(AdvMsg::AdvDetectArbExecStart) => {
                self.begin_op(View::Advanced);
                self.error_msg = None;
                let conn = self.connection;
                let device_model = self.device_model.clone();
                let loader_path = self.adv_wizard.file_path.clone();
                let i_anti = self.t("arb_detect_is_anti_rollback").to_string();
                let i_not = self.t("arb_detect_no_anti_rollback").to_string();
                let i_reboot_fastboot = self.t("live_arb_reboot_to_fastboot").to_string();
                let i_reboot_system = self.t("live_arb_reboot_to_system").to_string();
                let i_tb320fc_edl = self.t("live_arb_tb320fc_edl_dump").to_string();
                return task_heavy(
                    move || {
                        let mut log = Vec::new();
                        match detect_arb_run(
                            conn,
                            device_model,
                            loader_path,
                            &i_anti,
                            &i_not,
                            &i_reboot_fastboot,
                            &i_reboot_system,
                            &i_tb320fc_edl,
                            &mut log,
                        ) {
                            Ok(()) => Ok(log),
                            Err(e) => Err(e),
                        }
                    },
                    |__v| Message::Adv(AdvMsg::AdvDetectArbExecDone(__v)),
                    Err,
                );
            }
            Message::Adv(AdvMsg::AdvDetectArbExecDone(result)) => {
                match result {
                    Ok(lines) => {
                        self.flush_exec_done_log(lines);
                    }
                    Err(e) => {
                        self.error_msg = Some(e.clone());
                        self.log_push(format!("ERROR: {e}"));
                    }
                }
                self.end_op();
            }
            // Async results
            Message::FileSelected(path) => {
                if let Some(p) = path {
                    self.remember_recent(self.picker_target.kind(), &p);
                    match self.picker_target {
                        PickerTarget::RootFile => self.root.file_path = Some(p),
                        // Root loader `.melf` file — stored in
                        // `folder_path` for historical field-name reasons.
                        PickerTarget::RootLoader => self.root.folder_path = Some(p),
                        _ => {}
                    }
                }
                self.picker_target = PickerTarget::None;
            }
            Message::FolderSelected(path) => {
                if let Some(p) = path {
                    self.remember_recent(self.picker_target.kind(), &p);
                    match self.picker_target {
                        PickerTarget::UnrootFolder => self.unroot.folder_path = Some(p),
                        PickerTarget::FlashFolder => self.flash.firmware_folder = Some(p),
                        _ => {}
                    }
                }
                self.picker_target = PickerTarget::None;
            }
            Message::RecentFilePicked(target, path) => {
                // Stale entries self-heal on the next real pick.
                if !std::path::Path::new(&path).is_file() {
                    return Task::none();
                }
                self.remember_recent(target.kind(), &path);
                match target {
                    PickerTarget::RootFile => self.root.file_path = Some(path),
                    PickerTarget::RootLoader => self.root.folder_path = Some(path),
                    _ => {}
                }
            }
            Message::RecentFolderPicked(target, path) => {
                if !std::path::Path::new(&path).is_dir() {
                    return Task::none();
                }
                self.remember_recent(target.kind(), &path);
                match target {
                    PickerTarget::UnrootFolder => self.unroot.folder_path = Some(path),
                    PickerTarget::FlashFolder => self.flash.firmware_folder = Some(path),
                    _ => {}
                }
            }
            Message::NoticeRecentMissing(is_file) => {
                // Surface as the existing error banner — it already
                // overlays every view and has a dismiss button. Keep
                // out of the main log so the user's run history isn't
                // littered with picker UI noise.
                let key = if is_file {
                    "recent_missing_file"
                } else {
                    "recent_missing_folder"
                };
                self.error_msg = Some(self.t(key).to_string());
            }
            Message::OperationError(e) => {
                self.end_op();
                self.error_msg = Some(e.clone());
                self.log_push(format!("ERROR: {e}"));
            }
            Message::DismissError => self.error_msg = None,
            Message::StartOver => {
                match self.current_view {
                    View::Root => self.root.reset(),
                    View::Flash => self.flash.reset(),
                    View::SystemUpdate => self.sysupdate.reset(),
                    View::Unroot => self.unroot.reset(),
                    View::Advanced => {
                        // "Start over" on any Advanced sub-wizard should
                        // return to the Advanced grid, not step 0 of the
                        // currently open sub-flow.
                        self.advanced_wizard_open = AdvancedWizardOpen::None;
                        self.flash_parts.reset();
                        self.dump_parts.reset();
                        self.dump_phys.reset();
                        self.flash_phys.reset();
                        self.adv_wizard.reset();
                        self.adv_confirm = None;
                        self.adv_confirm_path = None;
                        self.set_image_info_log(String::new());
                    }
                    _ => {}
                }
                self.error_msg = None;
            }
            Message::DrainStdoutTap => {
                // Pull from BOTH the Windows stdout pipe (`stdout_tap`,
                // which captures third-party `println!` from qdl /
                // magiskboot / pbr) AND our in-process live sink (every
                // `live!` line we emit). The pipe path can stall on GUI
                // subsystem builds — handle init order, full pipe
                // buffer back-pressure, etc. — so the in-process sink
                // is the safety net that guarantees our own log lines
                // show up regardless of OS plumbing state.
                //
                // Dedup the combined batch with a `HashSet` instead of
                // relying on `log_extend`'s adjacent-only dedup: each
                // of our `live!` lines lands in BOTH sources, so naive
                // chaining produces interleaved doubles
                // (`[A, B, C, A, B, C]`) that the adjacent walker
                // can't collapse. First-occurrence wins, so the tap
                // ordering (which interleaves third-party output with
                // ours in real chronological order) is preserved.
                self.drain_pending_log_streams();
                // Batched rebuild — at most one cosmic-text reshape per tick.
                if self.log_dirty {
                    self.rebuild_log_editor();
                }
            }
            Message::LogEditorAction(action) => {
                // Read-only: swallow `Edit(_)`, forward selection /
                // scroll / caret motion so drag-select + Ctrl+C work.
                // Ctrl+C goes through the widget's key binding directly.
                use iced::widget::text_editor::Action;
                if !matches!(action, Action::Edit(_)) {
                    self.log_editor.perform(action);
                }
            }
            Message::ImageInfoLogEditorAction(action) => {
                use iced::widget::text_editor::Action;
                if !matches!(action, Action::Edit(_)) {
                    self.image_info_log_editor.perform(action);
                }
            }
            Message::SaveLog => {
                let source = self.active_log_save_source();
                self.pending_log_save_source = source;
                let file_name = match source {
                    LogSaveSource::Main => "ltbox.log",
                    LogSaveSource::ImageInfo => "image_info.txt",
                };
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_file_name(file_name)
                            .add_filter("Log", &["log", "txt"])
                            .save_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::SaveLogPath,
                );
            }
            Message::SaveLogPath(path) => {
                if let Some(path) = path {
                    let source = self.pending_log_save_source;
                    let joined = self.log_text_for_save(source);
                    match std::fs::write(&path, joined) {
                        Ok(()) => self.note_log_save_result(
                            source,
                            format!("[Log] Saved to {}", path.display()),
                        ),
                        Err(e) => {
                            self.error_msg = Some(format!("Log save failed: {e}"));
                            self.note_log_save_result(source, format!("[Log] Save failed: {e}"));
                        }
                    }
                }
            }
            // Device polling
            Message::PollDevice => {
                return Task::perform(
                    async {
                        tokio::task::spawn_blocking(|| {
                            let mut r = DevicePollResult::default();
                            // ADB first: distinguish unauthorized /
                            // authorizing from a ready device.
                            let mut adb = ltbox_device::adb::AdbManager::new();
                            match adb.check_device_state() {
                                Ok(Some("unauthorized")) | Ok(Some("authorizing")) => {
                                    r.status = ConnectionStatus::AdbUnauthorized;
                                    return r;
                                }
                                Ok(Some("device")) | Ok(Some("recovery")) => {
                                    let raw_model =
                                        adb.get_model().ok().flatten().unwrap_or_default();
                                    // Empty model = USB-debug OFF or
                                    // auth pending (`adbd: error: closed`).
                                    // Bucket under AdbUnauthorized so
                                    // the dashboard doesn't falsely claim
                                    // the platform is unsupported.
                                    if raw_model.is_empty() {
                                        r.status = ConnectionStatus::AdbUnauthorized;
                                        return r;
                                    }
                                    // TWRP: `twrp_<model>` via `ro.product.device`.
                                    r.status = if is_twrp_product(&raw_model) {
                                        ConnectionStatus::AdbRecovery
                                    } else {
                                        ConnectionStatus::Adb
                                    };
                                    r.model = strip_twrp_prefix(&raw_model);
                                    r.slot =
                                        adb.get_slot_suffix().ok().flatten().unwrap_or_default();
                                    r.firmware = trim_build_display(
                                        &adb.shell("getprop ro.config.lgsi.fp.incremental")
                                            .unwrap_or_default(),
                                    );
                                    r.arb = arb_from_model(&r.model).to_string();
                                    let hwboard =
                                        adb.shell("getprop ro.boot.hwboardid").unwrap_or_default();
                                    if !hwboard.is_empty() {
                                        let (ram, storage) = parse_hwboardid_ram_storage(&hwboard);
                                        r.ram = ram;
                                        r.storage = storage;
                                    }
                                    let name = adb
                                        .shell("getprop ro.vendor.config.lgsi.en.market_name")
                                        .unwrap_or_default();
                                    r.market_name = if !name.is_empty() {
                                        name
                                    } else {
                                        adb.shell("getprop ro.vendor.config.lgsi.kirby_en")
                                            .unwrap_or_default()
                                    };
                                    let hw =
                                        adb.shell("getprop ro.boot.hardware").unwrap_or_default();
                                    r.platform_supported = Some(hw.to_lowercase() == "qcom");
                                    if let Some(sn) = adb.serial() {
                                        r.serial = sn.to_string();
                                    }
                                    return r;
                                }
                                _ => {
                                    // Offline / noperm / detached fall through to Fastboot/EDL.
                                }
                            }
                            if ltbox_device::fastboot::FastbootDevice::check_device() {
                                r.status = ConnectionStatus::Fastboot;
                                if let Ok(mut dev) = ltbox_device::fastboot::FastbootDevice::open()
                                {
                                    let vars = dev.get_all_vars().unwrap_or_default();
                                    r.model = vars.model.unwrap_or_default();
                                    r.slot = vars.current_slot.unwrap_or_default();
                                    r.firmware = trim_build_display(
                                        &vars.build_display_id.unwrap_or_default(),
                                    );
                                    r.ram = vars.ram_gb.unwrap_or_default();
                                    r.storage = vars.storage_gb.unwrap_or_default();
                                    r.market_name = vars.product.unwrap_or_default();
                                    r.serial = vars.serialno.unwrap_or_default();
                                    // Numeric → raw string (dashboard falls through
                                    // when i18n lookup misses).
                                    let arb_val = vars
                                        .rollback_indices
                                        .values()
                                        .filter(|&&v| v > 1)
                                        .max()
                                        .copied();
                                    r.arb = if let Some(v) = arb_val {
                                        v.to_string()
                                    } else {
                                        // TB320FC has ARB but reports no indices.
                                        let m = r.model.to_uppercase();
                                        if m == "TB320FC" {
                                            "arb_yes".to_string()
                                        } else {
                                            arb_from_model(&r.model).to_string()
                                        }
                                    };
                                }
                                return r;
                            }
                            if ltbox_device::edl::check_device() {
                                r.status = ConnectionStatus::Edl;
                            }
                            r
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::DevicePolled,
                );
            }
            Message::DevicePolled(r) => {
                self.connection = r.status;
                if !r.model.is_empty() {
                    self.device_model = r.model;
                }
                if !r.slot.is_empty() {
                    self.device_slot = r.slot;
                }
                if !r.firmware.is_empty() {
                    self.device_firmware = r.firmware;
                }
                if !r.arb.is_empty() {
                    self.device_arb = r.arb;
                }
                if !r.ram.is_empty() {
                    self.device_ram = r.ram;
                }
                if !r.storage.is_empty() {
                    self.device_storage = r.storage;
                }
                if !r.market_name.is_empty() {
                    self.device_market_name = r.market_name;
                }
                if !r.serial.is_empty() {
                    self.device_serial = r.serial;
                }
                self.platform_supported = r.platform_supported;
                if self.connection == ConnectionStatus::None {
                    self.device_model.clear();
                    self.device_slot.clear();
                    self.device_firmware.clear();
                    self.device_arb.clear();
                    self.device_ram.clear();
                    self.device_storage.clear();
                    self.device_market_name.clear();
                    self.device_serial.clear();
                    self.platform_supported = None;
                }
            }
            Message::DeviceInfoOpen => {
                let serial = self.device_serial.trim().to_string();
                if serial.is_empty() {
                    return Task::none();
                }
                if self.device_info_cache.contains_key(&serial) {
                    self.device_info_popup = Some((serial, DeviceInfoState::Ready));
                    return Task::none();
                }
                self.device_info_popup = Some((serial.clone(), DeviceInfoState::Loading));
                let serial_for_task = serial.clone();
                return task_heavy(
                    move || {
                        let result = ltbox_core::lenovo_info::fetch_machine_info(&serial_for_task)
                            .map_err(|e| e.to_string());
                        (serial_for_task, result)
                    },
                    |(s, r)| Message::DeviceInfoFetched(s, r),
                    |e| (String::new(), Err(e)),
                );
            }
            Message::DeviceInfoFetched(serial, result) => {
                if serial.is_empty() {
                    // Worker panic fallback (`task_heavy` fallback case);
                    // surface as error on whichever popup is open.
                    if let Some((s, _)) = self.device_info_popup.clone() {
                        let msg = match result {
                            Err(e) => e,
                            Ok(_) => "task panicked".to_string(),
                        };
                        self.device_info_popup = Some((s, DeviceInfoState::Error(msg)));
                    }
                    return Task::none();
                }
                match result {
                    Ok(info) => {
                        // SaleArea-driven Flash region preselect. CN ⇒ PRC,
                        // explicit JSON null ⇒ ROW. Other strings / missing
                        // key leave the field untouched. Only set when the
                        // user has not already picked one to avoid clobbering
                        // a manual choice.
                        self.device_info_cache.insert(serial.clone(), info);
                        if self.flash.device_region.is_none()
                            && let Some(r) = self.inferred_flash_region()
                        {
                            self.flash.device_region = Some(r);
                        }
                        if matches!(&self.device_info_popup, Some((s, _)) if s == &serial) {
                            self.device_info_popup = Some((serial, DeviceInfoState::Ready));
                        }
                    }
                    Err(e) => {
                        if matches!(&self.device_info_popup, Some((s, _)) if s == &serial) {
                            self.device_info_popup = Some((serial, DeviceInfoState::Error(e)));
                        }
                    }
                }
            }
            Message::DeviceInfoRetry => {
                let Some((serial, _)) = self.device_info_popup.clone() else {
                    return Task::none();
                };
                self.device_info_popup = Some((serial.clone(), DeviceInfoState::Loading));
                let serial_for_task = serial;
                return task_heavy(
                    move || {
                        let result = ltbox_core::lenovo_info::fetch_machine_info(&serial_for_task)
                            .map_err(|e| e.to_string());
                        (serial_for_task, result)
                    },
                    |(s, r)| Message::DeviceInfoFetched(s, r),
                    |e| (String::new(), Err(e)),
                );
            }
            Message::DeviceInfoClose => {
                self.device_info_popup = None;
            }
            Message::CopyToClipboard(payload) => {
                let toast = self.t("toast_copied").to_string();
                return iced::clipboard::write::<Message>(payload)
                    .chain(Task::done(Message::ToastShow(toast)));
            }
            Message::ToastShow(msg) => {
                self.toast_msg = Some(msg);
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                    },
                    |_| Message::ToastClear,
                );
            }
            Message::ToastClear => {
                self.toast_msg = None;
            }
            Message::SidebarHoverEnter => {
                self.sidebar_expanded = true;
            }
            Message::SidebarHoverExit => {
                self.sidebar_expanded = false;
            }
            Message::SidebarAnimTick => {
                let target = self.sidebar_anim_target();
                // Exponential decay toward the target. ~0.30 per
                // 16 ms tick lands at ~99% in ~75 ms — fast enough
                // that label mount (gated below) does not feel
                // delayed but still smooth.
                let factor = 0.30;
                let next = self.sidebar_anim + (target - self.sidebar_anim) * factor;
                if (next - target).abs() < 0.005 {
                    self.sidebar_anim = target;
                } else {
                    self.sidebar_anim = next;
                }
            }
            Message::DriverCheckDone(status) => {
                self.driver_status = Some(status);
            }
            Message::UpdateCheckDone(result) => {
                // `None` means "no banner" — either we're already on the
                // latest stable, the repo has only prereleases, or the
                // probe failed (offline / 5xx / parse). All three should
                // render identically: nothing in the sidebar.
                self.update_available = result;
            }
            Message::OpenUpdateUrl => {
                if let Some(release) = self.update_available.as_ref() {
                    // `open` crate dispatches via `xdg-open` (Linux) /
                    // `start` (Windows) / `open` (macOS). Failure here is
                    // logged but not surfaced — the user can copy the URL
                    // out of the release notes if their default browser
                    // is misconfigured.
                    if let Err(e) = open::that_detached(&release.html_url) {
                        tracing::warn!("failed to open update URL: {e}");
                    }
                }
            }
            Message::InstallDrivers => {
                if self.installing_drivers {
                    return Task::none();
                }
                self.installing_drivers = true;
                self.log_push("[Driver] Starting Qualcomm USB driver install...".to_string());
                return Task::perform(
                    async {
                        tokio::task::spawn_blocking(|| {
                            let mut log = Vec::new();
                            match ltbox_device::driver::download_and_install(&mut log) {
                                Ok(()) => Ok(log),
                                Err(e) => {
                                    ltbox_core::live!(
                                        log,
                                        "[Driver] {}",
                                        ltbox_core::i18n::tr("live_driver_failed")
                                            .replace("{error}", &e.to_string())
                                    );
                                    Err(format!("{e}"))
                                }
                            }
                        })
                        .await
                        .unwrap_or_else(|_| Err("Task panicked".to_string()))
                    },
                    Message::InstallDriversDone,
                );
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsSelectLoader) => {
                return self.pick_loader_with_default(|__v| {
                    Message::FlashParts(FlashPartsMsg::FlashPartsLoaderChosen(__v))
                });
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsLoaderChosen(path)) => {
                if let Some(p) = path {
                    match self.resolve_loader_input(&p) {
                        Ok(loader) => {
                            self.flash_parts.loader_path = Some(loader);
                            self.flash_parts.scan_error = None;
                        }
                        Err(msg) => self.flash_parts.scan_error = Some(msg),
                    }
                }
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx)) => {
                if let Some(row) = self.flash_parts.rows.get_mut(idx) {
                    row.state = row.state.cycle();
                }
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsPickRowFile(idx)) => {
                let spec = pickers::FilePickSpec::single("picker_target_partition_image")
                    .with_filter("Partition image", &["img", "bin", "mbn", "melf", "elf"]);
                return pickers::pick_file_for(spec, &self.recent_paths, move |path| {
                    Message::FlashParts(FlashPartsMsg::FlashPartsRowFileChosen(idx, path))
                });
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsRowFileChosen(idx, path)) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    if let Some(row) = self.flash_parts.rows.get_mut(idx) {
                        row.file_path = Some(p);
                        // Picking a file implicitly flips the row to Flash
                        // so the user doesn't have to also cycle the box.
                        row.state = FlashRowState::Flash;
                    }
                }
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsNext) => match self.flash_parts.step {
                0 => return self.update(Message::FlashParts(FlashPartsMsg::FlashPartsScanStart)),
                1 => self.flash_parts.next(), // → Confirm
                2 => return self.update(Message::FlashParts(FlashPartsMsg::FlashPartsExecStart)),
                _ => {}
            },
            Message::FlashParts(FlashPartsMsg::FlashPartsBack) => self.flash_parts.back(),
            Message::FlashParts(FlashPartsMsg::FlashPartsClose) => {
                self.advanced_wizard_open = AdvancedWizardOpen::None;
                self.flash_parts.reset();
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsScanStart) => {
                let loader = match self.validate_loader_path(&self.flash_parts.loader_path.clone())
                {
                    Ok(p) => p,
                    Err(()) => return Task::none(),
                };
                self.begin_op(View::Flash);
                self.error_msg = None;
                self.flash_parts.scanning = true;
                self.flash_parts.scan_error = None;
                self.flash_parts.rows.clear();
                let conn = self.connection;
                self.log_lines
                    .push("[FlashParts] Scanning partitions...".to_string());
                return task_heavy(
                    move || flash_parts_scan(conn, loader),
                    |__v| Message::FlashParts(FlashPartsMsg::FlashPartsScanDone(__v)),
                    |e| FlashPartsScanResult {
                        logs: vec![format!("[FlashParts] {e}")],
                        rows: Vec::new(),
                        error: Some(e),
                    },
                );
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsScanDone(result)) => {
                self.flush_exec_done_log(result.logs);
                self.flash_parts.scanning = false;
                self.flash_parts.rows = result.rows;
                self.flash_parts.apply_sort();
                self.flash_parts.scan_error = result.error.clone();
                self.end_op();
                if result.error.is_none() && !self.flash_parts.rows.is_empty() {
                    self.flash_parts.next(); // → Select
                }
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsSortBy(col)) => {
                self.flash_parts.toggle_sort(col);
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsExecStart) => {
                self.flash_parts.next(); // advance to Exec screen
                self.begin_op(View::Flash);
                self.error_msg = None;
                let loader = self.flash_parts.loader_path.clone().unwrap_or_default();
                let rows = self.flash_parts.active_rows();
                let flash_cnt = rows
                    .iter()
                    .filter(|r| r.state == FlashRowState::Flash)
                    .count();
                let erase_cnt = rows
                    .iter()
                    .filter(|r| r.state == FlashRowState::Erase)
                    .count();
                self.log_lines.push(format!(
                    "[FlashParts] Flashing {flash_cnt} partition(s), erasing {erase_cnt}"
                ));
                return task_heavy(
                    move || flash_parts_execute(loader, rows),
                    |__v| Message::FlashParts(FlashPartsMsg::FlashPartsExecDone(__v)),
                    |e| vec![format!("[FlashParts] {e}")],
                );
            }
            Message::FlashParts(FlashPartsMsg::FlashPartsExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsSelectLoader) => {
                return self.pick_loader_with_default(|__v| {
                    Message::DumpParts(DumpPartsMsg::DumpPartsLoaderChosen(__v))
                });
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsLoaderChosen(path)) => {
                if let Some(p) = path {
                    match self.resolve_loader_input(&p) {
                        Ok(loader) => {
                            self.dump_parts.loader_path = Some(loader);
                            self.dump_parts.scan_error = None;
                        }
                        Err(msg) => self.dump_parts.scan_error = Some(msg),
                    }
                }
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsToggleRow(idx)) => {
                if let Some(row) = self.dump_parts.rows.get_mut(idx) {
                    row.selected = !row.selected;
                }
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsNext) => match self.dump_parts.step {
                0 => return self.update(Message::DumpParts(DumpPartsMsg::DumpPartsScanStart)),
                1 => return self.update(Message::DumpParts(DumpPartsMsg::DumpPartsSelectFolder)),
                _ => {}
            },
            Message::DumpParts(DumpPartsMsg::DumpPartsBack) => self.dump_parts.back(),
            Message::DumpParts(DumpPartsMsg::DumpPartsClose) => {
                self.advanced_wizard_open = AdvancedWizardOpen::None;
                self.dump_parts.reset();
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsScanStart) => {
                let loader = match self.validate_loader_path(&self.dump_parts.loader_path.clone()) {
                    Ok(p) => p,
                    Err(()) => return Task::none(),
                };
                self.dump_parts.scanning = true;
                self.dump_parts.scan_error = None;
                self.dump_parts.rows.clear();
                self.begin_op(View::Advanced);
                self.error_msg = None;
                let conn = self.connection;
                self.log_lines
                    .push("[DumpParts] Scanning partition tables...".to_string());
                return task_heavy(
                    move || dump_parts_scan(conn, loader),
                    |__v| Message::DumpParts(DumpPartsMsg::DumpPartsScanDone(__v)),
                    |e| DumpPartsScanResult {
                        logs: vec![format!("[DumpParts] {e}")],
                        rows: Vec::new(),
                        error: Some(e),
                    },
                );
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsScanDone(result)) => {
                self.flush_exec_done_log(result.logs);
                self.end_op();
                self.dump_parts.scanning = false;
                self.dump_parts.rows = result.rows;
                self.dump_parts.apply_sort();
                if let Some(err) = result.error {
                    self.dump_parts.scan_error = Some(err);
                } else if self.dump_parts.rows.is_empty() {
                    self.dump_parts.scan_error =
                        Some("No partitions returned from device".to_string());
                } else {
                    self.dump_parts.step = 1;
                }
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsSortBy(col)) => {
                self.dump_parts.toggle_sort(col);
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsToggleAll) => {
                let all_selected = !self.dump_parts.rows.is_empty()
                    && self.dump_parts.rows.iter().all(|r| r.selected);
                let target = !all_selected;
                for r in self.dump_parts.rows.iter_mut() {
                    r.selected = target;
                }
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsSelectFolder) => {
                // Dump destination, not a firmware source — goes to the
                // `OutputFolder` bucket so the MRU list doesn't mix input
                // firmware dirs with output dump dirs.
                return pick_folder_task(
                    pickers::PickerKind::OutputFolder,
                    &self.recent_paths,
                    |__v| Message::DumpParts(DumpPartsMsg::DumpPartsFolderChosen(__v)),
                );
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsFolderChosen(path)) => {
                if let Some(folder) = path {
                    self.remember_recent(pickers::PickerKind::OutputFolder, &folder);
                    self.dump_parts.output_dir = Some(folder.clone());
                    self.dump_parts.step = 2;
                    self.begin_op(View::Advanced);
                    self.error_msg = None;
                    let loader = self.dump_parts.loader_path.clone().unwrap_or_default();
                    let rows = self.dump_parts.selected_rows();
                    self.log_lines.push(format!(
                        "[DumpParts] Dumping {} partition(s) to {}",
                        rows.len(),
                        folder
                    ));
                    return task_heavy(
                        move || dump_parts_execute(loader, folder, rows),
                        |__v| Message::DumpParts(DumpPartsMsg::DumpPartsExecDone(__v)),
                        |e| vec![format!("[DumpParts] {e}")],
                    );
                }
            }
            Message::DumpParts(DumpPartsMsg::DumpPartsExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            // -- Physical Storage: Dump --------------------------------------
            Message::DumpPhys(DumpPhysMsg::DumpPhysSelectLoader) => {
                return self.pick_loader_with_default(|__v| {
                    Message::DumpPhys(DumpPhysMsg::DumpPhysLoaderChosen(__v))
                });
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysLoaderChosen(path)) => {
                if let Some(p) = path {
                    match self.resolve_loader_input(&p) {
                        Ok(loader) => {
                            self.dump_phys.loader_path = Some(loader);
                            self.dump_phys.loader_error = None;
                        }
                        Err(msg) => self.dump_phys.loader_error = Some(msg),
                    }
                }
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysToggleRow(idx)) => {
                if let Some(slot) = self.dump_phys.selected.get_mut(idx) {
                    *slot = !*slot;
                }
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysNext) => match self.dump_phys.step {
                0 => self.dump_phys.step = 1, // loader → select
                1 => return self.update(Message::DumpPhys(DumpPhysMsg::DumpPhysSelectFolder)),
                _ => {}
            },
            Message::DumpPhys(DumpPhysMsg::DumpPhysBack) => self.dump_phys.back(),
            Message::DumpPhys(DumpPhysMsg::DumpPhysClose) => {
                self.advanced_wizard_open = AdvancedWizardOpen::None;
                self.dump_phys.reset();
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysSelectFolder) => {
                // Dump destination — see DumpPartsSelectFolder.
                return pick_folder_task(
                    pickers::PickerKind::OutputFolder,
                    &self.recent_paths,
                    |__v| Message::DumpPhys(DumpPhysMsg::DumpPhysFolderChosen(__v)),
                );
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysFolderChosen(path)) => {
                if let Some(folder) = path {
                    let loader =
                        match self.validate_loader_path(&self.dump_phys.loader_path.clone()) {
                            Ok(p) => p,
                            Err(()) => return Task::none(),
                        };
                    self.remember_recent(pickers::PickerKind::OutputFolder, &folder);
                    self.dump_phys.output_dir = Some(folder.clone());
                    self.dump_phys.step = 2;
                    self.begin_op(View::Advanced);
                    self.error_msg = None;
                    let conn = self.connection;
                    let luns = self.dump_phys.selected_luns();
                    self.log_push(format!(
                        "[DumpPhys] {}",
                        self.t("live_dump_phys_batch_start")
                            .replace("{count}", &luns.len().to_string())
                            .replace("{path}", &folder)
                    ));
                    return task_heavy(
                        move || dump_physical_execute(conn, loader, folder, luns),
                        |__v| Message::DumpPhys(DumpPhysMsg::DumpPhysExecDone(__v)),
                        |e| vec![format!("[DumpPhys] {e}")],
                    );
                }
            }
            Message::DumpPhys(DumpPhysMsg::DumpPhysExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            // -- Physical Storage: Flash -------------------------------------
            Message::FlashPhys(FlashPhysMsg::FlashPhysSelectLoader) => {
                return self.pick_loader_with_default(|__v| {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysLoaderChosen(__v))
                });
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysLoaderChosen(path)) => {
                if let Some(p) = path {
                    match self.resolve_loader_input(&p) {
                        Ok(loader) => {
                            self.flash_phys.loader_path = Some(loader);
                            self.flash_phys.loader_error = None;
                        }
                        Err(msg) => self.flash_phys.loader_error = Some(msg),
                    }
                }
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysToggleRow(idx)) => {
                if let Some(slot) = self.flash_phys.selected.get_mut(idx) {
                    *slot = !*slot;
                }
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysPickRowFile(idx)) => {
                let spec = pickers::FilePickSpec::single("picker_target_storage_image")
                    .with_filter("Storage image", &["img", "bin", "mbn", "melf", "elf"]);
                return pickers::pick_file_for(spec, &self.recent_paths, move |path| {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysRowFileChosen(idx, path))
                });
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysRowFileChosen(idx, path)) => {
                if idx < PHYS_LUN_COUNT
                    && let Some(p) = path
                {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.flash_phys.file_paths[idx] = Some(p);
                    // Picking a file implicitly selects the row.
                    self.flash_phys.selected[idx] = true;
                }
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysNext) => match self.flash_phys.step {
                0 => self.flash_phys.step = 1,
                1 => self.flash_phys.next(), // → Confirm
                2 => return self.update(Message::FlashPhys(FlashPhysMsg::FlashPhysExecStart)),
                _ => {}
            },
            Message::FlashPhys(FlashPhysMsg::FlashPhysBack) => self.flash_phys.back(),
            Message::FlashPhys(FlashPhysMsg::FlashPhysClose) => {
                self.advanced_wizard_open = AdvancedWizardOpen::None;
                self.flash_phys.reset();
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysExecStart) => {
                let loader = match self.validate_loader_path(&self.flash_phys.loader_path.clone()) {
                    Ok(p) => p,
                    Err(()) => return Task::none(),
                };
                self.flash_phys.next(); // advance to Exec screen
                self.begin_op(View::Advanced);
                self.error_msg = None;
                let conn = self.connection;
                let pairs = self.flash_phys.active_pairs();
                self.log_lines
                    .push(format!("[FlashPhys] Flashing {} LUN(s)", pairs.len()));
                return task_heavy(
                    move || flash_physical_execute(conn, loader, pairs),
                    |__v| Message::FlashPhys(FlashPhysMsg::FlashPhysExecDone(__v)),
                    |e| vec![format!("[FlashPhys] {e}")],
                );
            }
            Message::FlashPhys(FlashPhysMsg::FlashPhysExecDone(lines)) => {
                self.flush_exec_done_log(lines);
                self.end_op();
            }
            Message::Reboot(RebootMsg::RebootRequest(target)) => {
                if self.busy {
                    return Task::none();
                }
                if !target.available_from(self.connection) {
                    self.error_msg = Some(format!(
                        "{:?} not reachable from {:?}",
                        target, self.connection
                    ));
                    return Task::none();
                }
                self.reboot_confirm_target = Some(target);
            }
            Message::Reboot(RebootMsg::RebootDismiss) => {
                self.reboot_confirm_target = None;
            }
            Message::Reboot(RebootMsg::RebootConfirm) => {
                if let Some(t) = self.reboot_confirm_target.take() {
                    return self.update(Message::Reboot(RebootMsg::RebootTo(t)));
                }
            }
            Message::Reboot(RebootMsg::RebootTo(target)) => {
                if self.busy {
                    return Task::none();
                }
                let conn = self.connection;
                if !target.available_from(conn) {
                    self.error_msg = Some(format!("{:?} not reachable from {:?}", target, conn));
                    return Task::none();
                }
                // EDL needs a Firehose loader before Power(reset).
                if matches!(conn, ConnectionStatus::Edl) {
                    return self.pick_loader_with_default(move |path| {
                        Message::Reboot(RebootMsg::RebootEdlWithLoader(target, path))
                    });
                }
                self.begin_op(View::Reboot);
                self.error_msg = None;
                self.log_push(format!(
                    "[Reboot] {}",
                    self.t("log_reboot_target_from")
                        .replace("{target}", self.t(target.label_key()))
                        .replace("{source}", &format!("{conn:?}")),
                ));
                let reboot_cmd_sent = self.t("log_reboot_command_sent").to_string();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let mut log = Vec::new();
                            match (conn, target) {
                                (ConnectionStatus::Adb | ConnectionStatus::AdbRecovery, t) => {
                                    let mut adb = ltbox_device::adb::AdbManager::new();
                                    // `AdbManager::reboot` needs the serial
                                    // from a prior `check_device` call.
                                    if !adb.check_device().unwrap_or(false) {
                                        return Err("No ADB device detected — try replugging the cable".into());
                                    }
                                    let arg = match t {
                                        RebootTarget::System => "",
                                        RebootTarget::Recovery => "recovery",
                                        RebootTarget::Bootloader => "bootloader",
                                        RebootTarget::Edl => "edl",
                                    };
                                    if let Err(e) = adb.reboot(arg) {
                                        return Err(format!("ADB reboot failed: {e}"));
                                    }
                                }
                                (ConnectionStatus::Fastboot, t) => {
                                    let mut dev = ltbox_device::fastboot::FastbootDevice::open()
                                        .map_err(|e| format!("Fastboot open: {e}"))?;
                                    match t {
                                        RebootTarget::System => {
                                            dev.reboot().map_err(|e| format!("reboot: {e}"))?;
                                        }
                                        RebootTarget::Bootloader => {
                                            dev.reboot_bootloader().map_err(|e| format!("reboot-bootloader: {e}"))?;
                                        }
                                        RebootTarget::Edl => {
                                            drop(dev);
                                            ensure_edl(ConnectionStatus::Fastboot, "Reboot", &mut log)
                                                .map_err(|()| "Could not transition device to EDL".to_string())?;
                                        }
                                        RebootTarget::Recovery => {
                                            return Err("Fastboot cannot reboot to recovery directly — switch to ADB first".into());
                                        }
                                    }
                                }
                                (ConnectionStatus::Edl, _) => {
                                    // RebootTo routes EDL through
                                    // RebootEdlWithLoader, never here.
                                    unreachable!("EDL reboot goes through RebootEdlWithLoader");
                                }
                                (ConnectionStatus::None, _) => {
                                    return Err("No device connected".into());
                                }
                                (ConnectionStatus::AdbUnauthorized, _) => {
                                    return Err(
                                        "USB debugging is not authorized on the device".into(),
                                    );
                                }
                            }
                            ltbox_core::live!(log, "[Reboot] {}", reboot_cmd_sent);
                            Ok(log)
                        })
                        .await
                        .unwrap_or_else(|_| Err("Task failed".to_string()))
                    },
                    |r| match r {
                        Ok(lines) => Message::Reboot(RebootMsg::RebootDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Reboot(RebootMsg::RebootEdlWithLoader(target, path)) => {
                let Some(loader_input) = path else {
                    self.log_push(format!(
                        "[Reboot] {}",
                        self.t("log_reboot_cancelled_no_loader")
                    ));
                    return Task::none();
                };
                // Accept direct loader files. Legacy folder paths from
                // older recents remain supported via resolve_loader_input.
                let loader = match self.resolve_loader_input(&loader_input) {
                    Ok(p) => std::path::PathBuf::from(p),
                    Err(msg) => {
                        self.error_msg = Some(msg);
                        return Task::none();
                    }
                };
                if !loader.exists() {
                    self.error_msg = Some(format!("Loader not found: {}", loader.display()));
                    return Task::none();
                }
                self.begin_op(View::Reboot);
                self.error_msg = None;
                self.log_push(format!(
                    "[Reboot] {}",
                    self.t("log_reboot_target_from_edl")
                        .replace("{target}", self.t(target.label_key()))
                        .replace("{loader}", &loader.display().to_string()),
                ));
                let reboot_cmd_sent = self.t("log_reboot_command_sent").to_string();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let mut log = Vec::new();
                            // `auto_reset=false` — reset is triggered explicitly below.
                            let mut session =
                                ltbox_device::edl::EdlSession::open(&loader, false, &mut log)
                                    .map_err(|e| format!("EDL session open: {e}"))?;
                            match target {
                                RebootTarget::System => {
                                    session.reset_tolerant(&mut log);
                                }
                                RebootTarget::Edl => {
                                    session
                                        .reset_to_edl(&mut log)
                                        .map_err(|e| format!("reset_to_edl: {e}"))?;
                                }
                                other => {
                                    return Err(format!(
                                        "Reboot to {other:?} is not supported from EDL"
                                    ));
                                }
                            }
                            ltbox_core::live!(log, "[Reboot] {}", reboot_cmd_sent);
                            Ok(log)
                        })
                        .await
                        .unwrap_or_else(|_| Err("Task failed".to_string()))
                    },
                    |r| match r {
                        Ok(lines) => Message::Reboot(RebootMsg::RebootDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
            }
            Message::Reboot(RebootMsg::RebootDone(lines)) => {
                self.end_op();
                self.flush_exec_done_log(lines);
            }
            Message::InstallDriversDone(result) => {
                self.installing_drivers = false;
                match result {
                    Ok(log) => {
                        for line in log {
                            self.log_push(line);
                        }
                        self.log_push(self.t("driver_install_done").to_string());
                        // Re-run the presence check to clear the banner.
                        return Task::perform(
                            async {
                                tokio::task::spawn_blocking(
                                    ltbox_device::driver::check_required_drivers,
                                )
                                .await
                                .unwrap_or(ltbox_device::driver::DriverStatus::NotWindows)
                            },
                            Message::DriverCheckDone,
                        );
                    }
                    Err(e) => {
                        self.log_lines
                            .push(self.t("driver_install_failed").replace("{e}", &e));
                        self.error_msg = Some(self.t("driver_install_failed").replace("{e}", &e));
                    }
                }
            }
        }
        Task::none()
    }

    fn title_bar(&self) -> Element<'_, Message> {
        let title_content = container(
            row![
                iced::widget::image(TITLE_BAR_ICON_HANDLE.clone())
                    .width(16)
                    .height(16),
                text("LTBox").size(12).style(muted_style),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 12])
        .width(Length::Fill);

        let drag_area = iced::widget::mouse_area(title_content)
            .on_press(Message::Window(WindowMsg::WindowDrag))
            .on_double_click(Message::Window(WindowMsg::WindowToggleMaximize));

        let btn_w = 46;
        let btn_h = 32;

        let minimize_btn = button(
            container(lucide_icon(icon::win_minimize(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowMinimize))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.15).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        let maximize_btn = button(
            container(lucide_icon(icon::win_maximize(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowToggleMaximize))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.15).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        let close_btn = button(
            container(lucide_icon(icon::win_close(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowClose))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgb(0.9, 0.2, 0.2).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        container(
            row![drag_area, minimize_btn, maximize_btn, close_btn,]
                .align_y(iced::Alignment::Center)
                .height(btn_h),
        )
        .width(Length::Fill)
        .style(|t: &Theme| container::Style {
            background: Some(pal_of(t).surface_container_low.into()),
            ..Default::default()
        })
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut main = column![];
        main = main.push(self.title_bar());
        // 1-px divider below the chromeless title bar so the title
        // surface doesn't bleed into the content area.
        main = main.push(widget::rule::horizontal(1).style(shell_rule_style));
        main = main.push(row![self.sidebar(), self.content()].height(Length::Fill));
        main = main.push(self.status_bar());

        // 1-px outline around the entire window so the borderless
        // OS-level decoration does not bleed straight into the desktop
        // background — matches the M3 elevation-tint outline that
        // sits under chromeless surfaces.
        // 1-px padding so the inner content does not paint over the
        // border. iced container layouts ignore border width when
        // sizing children, so without the inset the children's own
        // backgrounds were tiling over the top / left / bottom
        // outline pixels and only the right edge (the sidebar's
        // vertical rule) survived.
        let framed = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(1)
            .style(|t: &Theme| container::Style {
                border: iced::Border {
                    color: pal_of(t).outline_variant,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });

        // Error banner below popups so the scrim dims the banner too.
        let mut layers: Vec<Element<'_, Message>> = vec![framed.into()];

        if let Some(err) = &self.error_msg {
            layers.push(self.error_banner(err));
        }
        if self.country_popup_open {
            layers.push(self.country_popup_view());
        }
        if self.region_target_popup_open {
            layers.push(self.region_target_popup_view());
        }
        if let Some(t) = self.reboot_confirm_target {
            layers.push(self.reboot_confirm_popup(t));
        }
        if self.root.run_id_popup_open {
            layers.push(self.root_run_id_popup());
        }
        if self.root.kernel_version_popup_open {
            layers.push(self.root_kernel_version_popup());
        }
        if self.root.superkey_popup_open {
            layers.push(self.root_superkey_popup());
        }
        if self.should_show_busy_progress_dialog() {
            layers.push(self.busy_progress_dialog());
        }
        if self.device_info_popup.is_some() {
            layers.push(self.device_info_popup_view());
        }
        if self.arb_index_popup_open {
            layers.push(self.arb_index_popup_view());
        }
        if self.toast_msg.is_some() {
            layers.push(self.toast_view());
        }

        if layers.len() == 1 {
            layers.into_iter().next().unwrap()
        } else {
            iced::widget::Stack::with_children(layers).into()
        }
    }

    fn busy_progress_dialog(&self) -> Element<'_, Message> {
        let op_name = self.busy_operation_label();
        let body = self.busy_body_override().unwrap_or_else(|| {
            self.t("progress_dialog_body")
                .replace("{operation}", &op_name)
        });

        let spinner: Element<'_, Message> = Spinner::new()
            .width(Length::Fixed(42.0))
            .height(Length::Fixed(42.0))
            .circle_radius(3.0)
            .into();
        let spinner_box = container(spinner)
            .width(56)
            .height(56)
            .center_x(56)
            .center_y(56)
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    text_color: Some(p.primary),
                    ..Default::default()
                }
            });

        let title_col = column![
            text(self.t("progress_dialog_title").to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .style(on_surface_style),
            text(body).size(13).style(muted_style),
        ]
        .spacing(6)
        .width(Length::Fill);

        let content = column![
            row![spinner_box, title_col]
                .spacing(18)
                .align_y(iced::Alignment::Center),
        ]
        .spacing(16)
        .padding(24)
        .width(420);

        m3_dialog(content.into())
    }

    /// Bottom-of-screen transient toast. Renders a low-attention pill
    /// over a transparent passthrough container so the rest of the
    /// view keeps responding to clicks while the toast is on screen.
    fn toast_view(&self) -> Element<'_, Message> {
        let Some(msg) = self.toast_msg.clone() else {
            return container(text("")).into();
        };
        // Background = `on_surface` (near-black in light, near-white
        // in dark); text needs the inverse to stay readable in both
        // modes — `surface` is exactly that role pair.
        let pill = container(
            text(msg)
                .size(12)
                .style(|t: &Theme| iced::widget::text::Style {
                    color: Some(pal_of(t).surface),
                }),
        )
        .padding([8, 16])
        .style(|t: &Theme| -> container::Style {
            let p = pal_of(t);
            container::Style {
                background: Some(p.on_surface.into()),
                border: iced::Border {
                    radius: 18.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
        container(pill)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                // Sits ~100 px above the viewport floor — clears the
                // device-info popup's close-button row without
                // crowding the table content.
                bottom: 100.0,
                left: 0.0,
            })
            .center_x(Length::Fill)
            .align_y(iced::Alignment::End)
            .into()
    }

    /// Device-info popup: render the Lenovo PTSTPD `data` block as a
    /// 2-column key/value table. Branches on `DeviceInfoState` so the
    /// modal stays open through Loading / Error / Ready transitions
    /// without flashing in/out of existence.
    fn device_info_popup_view(&self) -> Element<'_, Message> {
        let Some((serial, state)) = self.device_info_popup.clone() else {
            return container(text("")).into();
        };
        let title = text(self.t("device_info_popup_title").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE);
        // Copy-icon button — only enabled once the upstream payload is
        // cached; clicking copies the unmodified `data` JSON to the
        // clipboard and surfaces a toast.
        let copy_payload: Option<String> = self
            .device_info_cache
            .get(&serial)
            .map(|i| i.data_pretty.clone());
        let copy_glyph = text("⧉").size(16);
        let copy_btn = if let Some(payload) = copy_payload {
            button(container(copy_glyph).padding([2, 6]))
                .on_press(Message::CopyToClipboard(payload))
                .padding(0)
                .style(|t: &Theme, status| {
                    let p = pal_of(t);
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(
                            if hovered {
                                p.surface_container_high
                            } else {
                                p.surface_container
                            }
                            .into(),
                        ),
                        text_color: p.on_surface,
                        border: iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
        } else {
            // Same shape, no on_press — keeps the header layout stable
            // during the loading / error states without leaving an
            // active click target.
            button(container(copy_glyph).padding([2, 6]))
                .padding(0)
                .style(|t: &Theme, _s| {
                    let p = pal_of(t);
                    button::Style {
                        background: Some(p.surface_container.into()),
                        text_color: p.on_surface_variant,
                        border: iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
        };
        let header = iced::widget::row![title, Space::new().width(Length::Fill), copy_btn]
            .align_y(iced::Alignment::Center);
        let serial_line = text(format!("{}: {serial}", self.t("device_info_popup_serial")))
            .size(12)
            .style(muted_style);

        let body: Element<'_, Message> = match &state {
            DeviceInfoState::Loading => container(Spinner::new())
                .width(Length::Fill)
                .height(120)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into(),
            DeviceInfoState::Error(e) => column![
                text(self.t("device_info_popup_error").to_string())
                    .size(13)
                    .color(iced::Color::from_rgb(0.9, 0.2, 0.2)),
                text(e.clone()).size(11).style(muted_style),
                Space::new().height(8),
                button(
                    text(self.t("btn_retry").to_string())
                        .size(12)
                        .color(iced::Color::WHITE),
                )
                .on_press(Message::DeviceInfoRetry)
                .padding([6, 18])
                .style(md_filled_btn_style),
            ]
            .spacing(8)
            .into(),
            DeviceInfoState::Ready => {
                let info = match self.device_info_cache.get(&serial) {
                    Some(i) => i,
                    None => {
                        return container(text("")).into();
                    }
                };
                let mut table = column![].spacing(0);
                for (i, (k, v)) in info.fields.iter().enumerate() {
                    let display_v = v.clone().unwrap_or_default();
                    let key_cell = text(k.clone()).size(12).style(muted_style).width(180);
                    let val_cell = text(display_v).size(12).width(Length::Fill);
                    let row_inner = iced::widget::row![key_cell, val_cell]
                        .spacing(12)
                        .padding([4, 10])
                        .align_y(iced::Alignment::Center);
                    let zebra = i % 2 == 1;
                    let tinted = container(row_inner).width(Length::Fill).style(
                        move |t: &Theme| -> container::Style {
                            let p = pal_of(t);
                            container::Style {
                                background: if zebra {
                                    Some(iced::Background::Color(p.surface_container_low))
                                } else {
                                    None
                                },
                                ..Default::default()
                            }
                        },
                    );
                    table = table.push(tinted);
                }
                scrollable(table)
                    .height(Length::Fixed(420.0))
                    .width(Length::Fill)
                    .into()
            }
        };

        let close_btn = button(
            text(self.t("btn_close").to_string())
                .size(12)
                .color(iced::Color::WHITE),
        )
        .on_press(Message::DeviceInfoClose)
        .padding([6, 18])
        .style(md_filled_btn_style);

        let content = column![
            header,
            serial_line,
            widget::rule::horizontal(1),
            body,
            iced::widget::row![Space::new().width(Length::Fill), close_btn]
                .align_y(iced::Alignment::Center),
        ]
        .spacing(12)
        .padding(20)
        .width(640);

        m3_dialog(content.into())
    }

    /// PatchArb timestamp popup. Reads `adv_wizard.arb_index_buffer`
    /// for the in-flight typing and renders the UTC representation in
    /// real time once the buffer hits exactly 10 digits. OK is enabled
    /// only on a 10-digit buffer that parses to a `u64`.
    fn arb_index_popup_view(&self) -> Element<'_, Message> {
        let buf = self.adv_wizard.arb_index_buffer.clone();
        let valid = buf.len() == 10 && buf.parse::<u64>().is_ok();

        // UTC preview only when the buffer is exactly 10 digits, so
        // shrinking the value (e.g. backspacing while editing) makes
        // the preview disappear instead of jumping to a stale time.
        let utc_preview: Element<'_, Message> = if valid {
            let ts: u64 = buf.parse().unwrap_or(0);
            let formatted = format_unix_timestamp_utc(ts);
            text(formatted)
                .size(13)
                .color(iced::Color::from_rgb(0.4, 0.7, 0.4))
                .into()
        } else {
            // Keep a fixed-height placeholder so the layout doesn't
            // jump when the preview appears / disappears.
            container(text("").size(13)).height(20).into()
        };

        let title = text(self.t("arb_index_popup_title").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE);
        let subtitle = text(self.t("arb_index_popup_subtitle").to_string())
            .size(12)
            .style(muted_style);

        let input = iced::widget::text_input(
            self.t("arb_index_popup_placeholder"),
            &self.adv_wizard.arb_index_buffer,
        )
        .on_input(|s| Message::Adv(AdvMsg::AdvWizArbIndexInput(s)))
        .on_submit(Message::Adv(AdvMsg::AdvWizArbIndexConfirm))
        .padding([8, 12])
        .size(14)
        .width(Length::Fill);

        let cancel_btn = button(text(self.t("btn_cancel").to_string()).size(13))
            .on_press(Message::Adv(AdvMsg::AdvWizArbIndexCancel))
            .padding([8, 18])
            .style(md_text_btn_style);
        let ok_btn_inner = text(self.t("btn_ok").to_string())
            .size(13)
            .color(iced::Color::WHITE);
        let ok_btn = if valid {
            button(ok_btn_inner)
                .on_press(Message::Adv(AdvMsg::AdvWizArbIndexConfirm))
                .padding([8, 18])
                .style(md_filled_btn_style)
        } else {
            button(ok_btn_inner)
                .padding([8, 18])
                .style(md_filled_btn_style)
        };

        let content = column![
            title,
            subtitle,
            utc_preview,
            input,
            iced::widget::row![Space::new().width(Length::Fill), cancel_btn, ok_btn]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        ]
        .spacing(12)
        .padding(20)
        .width(420);

        m3_dialog(content.into())
    }

    fn country_popup_view(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(2);
        let selected_code = self.country_popup_selected_code();
        // Flash wizard only — hide "Do not change" from the Advanced
        // PatchDevinfo flow because that action requires a concrete target
        // code to write into devinfo/persist.
        if !self.adv_needs_country {
            let skipped = self.wf_config.country_action.is_skipped();
            list = list.push(
                button(text(self.t("popup_country_do_not_change").to_string()).size(13))
                    .on_press(Message::SkipCountryPatch)
                    .padding([6, 14])
                    .width(Length::Fill)
                    .style(move |t: &Theme, status| {
                        let p = pal_of(t);
                        let hover = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(if skipped {
                                p.primary.into()
                            } else if hover {
                                p.surface_container_high.into()
                            } else {
                                iced::Color::TRANSPARENT.into()
                            }),
                            text_color: if skipped { p.on_primary } else { p.on_surface },
                            ..Default::default()
                        }
                    }),
            );
            list = list.push(widget::rule::horizontal(1));
        }
        for entry in COUNTRY_CODES {
            let code = entry.code.to_string();
            let selected = selected_code == Some(entry.code);
            let label = format!("{} — {}", entry.code, entry.name);
            list = list.push(
                button(text(label).size(13))
                    .on_press(Message::SelectCountry(code))
                    .padding([6, 14])
                    .width(Length::Fill)
                    .style(move |t: &Theme, status| {
                        let p = pal_of(t);
                        let hover = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(if selected {
                                p.primary.into()
                            } else if hover {
                                p.surface_container_high.into()
                            } else {
                                iced::Color::TRANSPARENT.into()
                            }),
                            text_color: if selected { p.on_primary } else { p.on_surface },
                            ..Default::default()
                        }
                    }),
            );
        }

        let popup_content = container(
            column![
                row![
                    text(self.t("popup_select_country").to_string()).size(16),
                    Space::new().width(Length::Fill),
                    button(
                        text(self.t("btn_cancel").to_string())
                            .size(12)
                            .style(muted_style)
                    )
                    .on_press(Message::DismissCountryPopup)
                    .padding([4, 12])
                    .style(neutral_pill_btn_style),
                ]
                .align_y(iced::Alignment::Center),
                widget::rule::horizontal(1),
                scrollable(list).height(300),
            ]
            .spacing(10)
            .padding(20)
            .width(400),
        )
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::MD.into(),
                },
                shadow: iced::Shadow {
                    color: with_alpha(p.shadow, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 20.0,
                },
                ..Default::default()
            }
        });

        container(
            container(popup_content)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4).into()),
            ..Default::default()
        })
        .into()
    }

    /// PRC / ROW radio popup for the Advanced RegionConvert wizard.
    /// Smaller than the country popup (only two choices) so the
    /// content uses M3 radio rows in a fixed-width card.
    fn region_target_popup_view(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.region_target;
        let mut list = column![].spacing(2);
        for target in [DeviceRegion::Prc, DeviceRegion::Row] {
            let is_selected = selected == Some(target);
            let label = self.t(target.label_key()).to_string();
            let bg_color = if is_selected {
                ACCENT
            } else {
                iced::Color::TRANSPARENT
            };
            let txt_color = if is_selected {
                iced::Color::WHITE
            } else {
                iced::Color::BLACK
            };
            list = list.push(
                button(text(label).size(13).color(txt_color))
                    .on_press(Message::SelectRegionTarget(target))
                    .padding([6, 14])
                    .width(Length::Fill)
                    .style(move |_t: &Theme, status| {
                        let hover = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(if is_selected {
                                bg_color.into()
                            } else if hover {
                                iced::Color::from_rgba(0.357, 0.388, 0.878, 0.08).into()
                            } else {
                                iced::Color::TRANSPARENT.into()
                            }),
                            text_color: txt_color,
                            ..Default::default()
                        }
                    }),
            );
        }

        let popup_content = container(
            column![
                row![
                    text(self.t("popup_select_region_target").to_string()).size(16),
                    Space::new().width(Length::Fill),
                    button(
                        text(self.t("btn_cancel").to_string())
                            .size(12)
                            .style(muted_style)
                    )
                    .on_press(Message::DismissRegionTargetPopup)
                    .padding([4, 12])
                    .style(neutral_pill_btn_style),
                ]
                .align_y(iced::Alignment::Center),
                widget::rule::horizontal(1),
                list,
            ]
            .spacing(10)
            .padding(20)
            .width(320),
        )
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::MD.into(),
                },
                shadow: iced::Shadow {
                    color: with_alpha(p.shadow, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 20.0,
                },
                ..Default::default()
            }
        });

        container(
            container(popup_content)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4).into()),
            ..Default::default()
        })
        .into()
    }

    // -- Sidebar ----------------------------------------------------------

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

    fn sidebar(&self) -> Element<'_, Message> {
        // Only mount labels once the tween has fully settled at the
        // expanded target. While `sidebar_anim` is still climbing the
        // shell width is narrower than the label + icon natural row
        // width, which made each frame re-measure the text and read
        // as a glyph twitch. Closing: first off-target frame drops
        // the label so the contraction stays clean.
        let expanded = self.sidebar_anim >= 0.85;
        let mut col = column![].spacing(1).padding([16, 0]);
        for &v in NAV_MAIN {
            col = col.push(nav_btn(
                v,
                self.t(v.label_key()),
                self.current_view == v,
                self.is_nav_enabled(v),
                expanded,
            ));
        }
        col = col.push(sec_hdr(self.t("nav_section_tools"), expanded));
        for &v in NAV_TOOLS {
            col = col.push(nav_btn(
                v,
                self.t(v.label_key()),
                self.current_view == v,
                self.is_nav_enabled(v),
                expanded,
            ));
        }

        // Bottom-anchored sidebar layout: `[nav | update pill]`.
        // The nav column claims `Length::Fill` so its rows pin to
        // the top and the update-available pill (rendered only when
        // the background probe surfaces a newer stable release) lives
        // below it. The previous `scrollable(col)` wrapper reserved
        // a scrollbar gutter on the right of the column that read as
        // a doubled line next to the sidebar's own vertical divider;
        // the nav set is small enough that scroll is never needed,
        // so the wrapper is dropped.
        let body: Element<'_, Message> = if let Some(release) = self.update_available.as_ref() {
            column![
                container(col).width(Length::Fill).height(Length::Fill),
                self.update_available_pill(release),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            container(col)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        // Width tween: lerp 64 ↔ 210 based on `sidebar_anim`.
        // Inner content swaps to label form at the midpoint so the
        // labels don't pop in over an under-sized shell.
        let width = 64.0 + (210.0 - 64.0) * self.sidebar_anim;
        // Right-edge divider via an explicit `vertical_rule` (1 px)
        // instead of a container border, so the corner where the
        // sidebar meets the status bar reads as one line per
        // direction rather than two stacked outlines (M3 nav-rail
        // guidance: one divider per shared edge).
        let panel = container(body)
            .width(width)
            .height(Length::Fill)
            .style(panel_bg);
        let shell =
            row![panel, widget::rule::vertical(1).style(shell_rule_style)].height(Length::Fill);
        iced::widget::mouse_area(shell)
            .on_enter(Message::SidebarHoverEnter)
            .on_exit(Message::SidebarHoverExit)
            .into()
    }

    /// Material 3 "tonal/filled" success-leaning pill that lives at the
    /// bottom of the sidebar and links to the GitHub release page when
    /// a newer stable build is available. Hidden entirely (caller drops
    /// it from the layout) when `update_available` is `None`.
    ///
    /// Style notes:
    ///   * Background = `tertiary` (M3 success / "go-update" color slot;
    ///     palette-aware so dark mode reads correctly without us hand-rolling
    ///     two greens).
    ///   * Foreground = `on_tertiary` for the same reason.
    ///   * Pill radius (`shape::FULL`) matches M3 button-pill geometry.
    ///   * Hover = subtle alpha lift via `theme::state::HOVER`, parity
    ///     with the rest of the app's tonal buttons.
    fn update_available_pill(
        &self,
        _release: &ltbox_core::github::StableRelease,
    ) -> Element<'_, Message> {
        let label = self.t("sidebar_update_available").to_string();
        // Match the sidebar's settled-only check so the pill swaps
        // in lockstep with the nav-button labels.
        let expanded = self.sidebar_anim >= 0.85;
        let inner: Element<'_, Message> = if expanded {
            row![
                icon::tile_update_on()
                    .size(16)
                    .style(|t: &Theme| iced::widget::text::Style {
                        color: Some(pal_of(t).on_tertiary)
                    }),
                text(label).size(13).line_height(1.2),
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
        let btn_padding = if expanded { [10, 16] } else { [10, 10] };
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

    // -- Content ----------------------------------------------------------

    fn content(&self) -> Element<'_, Message> {
        if self.current_view == View::Root {
            return self.view_root_wizard();
        }
        if self.current_view == View::Flash {
            return self.view_flash_wizard();
        }
        if self.current_view == View::SystemUpdate {
            return self.view_sysupdate_wizard();
        }
        if self.current_view == View::Unroot {
            return self.view_unroot_wizard();
        }
        // Advanced wizards (generic + FlashPartitions) skip the grid's
        // scrollable+padding wrapper so the step bar isn't pinched and
        // the 280 px browse card doesn't stretch.
        if self.current_view == View::Advanced
            && (self.advanced_wizard_open.is_open() || self.adv_wizard.action.is_some())
        {
            return self.view_advanced();
        }

        // Reboot cards need Fill height; scrollable would force Shrink
        // and collapse them.
        if self.current_view == View::Reboot {
            return container(
                container(self.view_reboot())
                    .padding(24)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }
        let inner = match self.current_view {
            View::Dashboard => self.view_dashboard(),
            View::Advanced => self.view_advanced(),
            View::Settings => self.view_settings(),
            _ => self.view_placeholder(),
        };
        // Dashboard wants the log card to fill the leftover vertical space
        // so the inner top + bottom margins stay symmetric. A `scrollable`
        // gives its child unbounded height, which collapses every
        // `Length::Fill` inside the dashboard tree to zero — so the
        // dashboard skips the scrollable wrapper and lets its own
        // `column.height(Fill)` claim the bounded viewport directly.
        // Other views (Advanced, Settings, …) keep the scrollable wrapper
        // because their content can legitimately grow past the viewport.
        let body: Element<'_, Message> = if matches!(self.current_view, View::Dashboard) {
            container(inner)
                .padding(24)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            scrollable(container(inner).padding(24).width(Length::Fill)).into()
        };
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Error banner -----------------------------------------------------

    fn error_banner(&self, msg: &str) -> Element<'_, Message> {
        // Floating overlay via `view()`'s stack so the layout below
        // doesn't shift.
        let err_bg = iced::Color::from_rgba(0.95, 0.2, 0.2, 0.94);
        let card = container(
            row![
                text(format!("  {msg}")).size(12).color(iced::Color::WHITE),
                Space::new().width(Length::Fill),
                button(text(" × ").size(14).color(iced::Color::WHITE))
                    .on_press(Message::DismissError)
                    .padding([2, 10])
                    .style(|_t: &Theme, status| {
                        let a = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: if a {
                                Some(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.18).into())
                            } else {
                                None
                            },
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: 4.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            ]
            .padding([8, 16])
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .style(move |_t: &Theme| container::Style {
            background: Some(err_bg.into()),
            border: iced::Border {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.0),
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25),
                offset: iced::Vector::new(0.0, 2.0),
                blur_radius: 6.0,
            },
            ..Default::default()
        });
        // Pin to y=0 via a Fill-height spacer below.
        column![card, Space::new().width(Length::Fill).height(Length::Fill)]
            .width(Length::Fill)
            .into()
    }

    // -- Status bar -------------------------------------------------------

    fn status_bar(&self) -> Element<'_, Message> {
        let status_color = self.connection.color(self.pal());
        let status_label = self.t(self.connection.label_key());
        let model_text = if self.device_model.is_empty() {
            ""
        } else {
            &self.device_model
        };
        let mut status_row = row![
            text(format!("●  {status_label}"))
                .size(12)
                .color(status_color),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        if !model_text.is_empty() {
            status_row =
                status_row.push(text(format!("— {model_text}")).size(12).style(muted_style));
        }
        status_row = status_row.push(Space::new().width(Length::Fill));
        if self.busy {
            status_row = status_row.push(
                text(self.t("status_working").to_string())
                    .size(12)
                    .style(accent_style),
            );
        }
        status_row = status_row.push(
            text(concat!("v", env!("CARGO_PKG_VERSION")))
                .size(12)
                .style(muted_style),
        );
        // Top divider via an explicit `horizontal_rule` (1 px) so
        // the meeting point with the sidebar's right divider lands
        // as a single line per direction (M3 bottom-app-bar
        // guidance: one divider per shared edge).
        column![
            widget::rule::horizontal(1).style(shell_rule_style),
            container(status_row.padding([8, 20]))
                .width(Length::Fill)
                .style(|t: &Theme| panel_bg(t)),
        ]
        .into()
    }

    // -- Dashboard --------------------------------------------------------

    fn driver_warning_banner(&self) -> Element<'_, Message> {
        let installing = self.installing_drivers;
        let btn_label = if installing {
            self.t("driver_installing_btn").to_string()
        } else {
            self.t("driver_install_btn").to_string()
        };
        let mut btn = button(text(btn_label).size(theme::text_size::LABEL_LARGE))
            .padding([8, 18])
            .style(md_filled_btn_style);
        if !installing {
            btn = btn.on_press(Message::InstallDrivers);
        }

        let body = column![
            text(self.t("driver_missing_title").to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .color(iced::Color::WHITE),
            text(self.t("driver_missing_desc").to_string())
                .size(theme::text_size::BODY_SMALL)
                .color(iced::Color::WHITE),
        ]
        .spacing(4);

        let content = row![body, Space::new().width(Length::Fill), btn,]
            .spacing(12)
            .align_y(iced::Alignment::Center);

        container(content)
            .padding([12, 16])
            .width(Length::Fill)
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    background: Some(p.error.into()),
                    border: iced::Border {
                        color: p.error,
                        width: 1.0,
                        radius: theme::shape::SM.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    fn view_dashboard(&self) -> Element<'_, Message> {
        let model = if self.device_model.is_empty() {
            "—"
        } else {
            &self.device_model
        };
        let slot = if self.device_slot.is_empty() {
            "—"
        } else {
            &self.device_slot
        };
        let firmware = if self.device_firmware.is_empty() {
            "—"
        } else {
            &self.device_firmware
        };
        // i18n key (`arb_*`) or numeric from fastboot vars; translation
        // layer passes numerics through.
        let arb_raw = self.device_arb.clone();
        let arb_display = if arb_raw.is_empty() {
            "—".to_string()
        } else if arb_raw.starts_with("arb_") {
            self.t(&arb_raw).to_string()
        } else {
            arb_raw
        };
        let arb = arb_display.as_str();
        let ram = if self.device_ram.is_empty() {
            "—"
        } else {
            &self.device_ram
        };
        let storage = if self.device_storage.is_empty() {
            "—"
        } else {
            &self.device_storage
        };
        let op_text = if self.busy {
            let base = self.t("dash_operation_in_progress").to_string();
            let label = format!("{base} - {}", self.busy_operation_label());
            text(label).size(13).style(accent_style)
        } else {
            text(self.t("dash_no_operation").to_string())
                .size(13)
                .style(muted_style)
        };
        let _log_preview_len = self.log_lines.len();

        // Title + divider dropped — sidebar already labels the active view,
        // so the duplicate header was eating vertical space without telling
        // the user anything new. `height(Fill)` so the log card (the last
        // child) can claim the remaining vertical space — keeps the top +
        // bottom dashboard margins symmetric.
        let mut content = column![]
            .spacing(14)
            .width(Length::Fill)
            .height(Length::Fill);

        // Unauthorized ADB wins over the platform warning — empty
        // `ro.boot.hardware` otherwise reads as "unsupported platform".
        if self.connection == ConnectionStatus::AdbUnauthorized {
            let warn_bg = iced::Color::from_rgb(0.95, 0.65, 0.0);
            content = content.push(
                container(
                    text(self.t("dash_adb_unauthorized").to_string())
                        .size(12)
                        .color(iced::Color::WHITE),
                )
                .padding([10, 16])
                .width(Length::Fill)
                .style(move |_t: &Theme| container::Style {
                    background: Some(warn_bg.into()),
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        } else if self.platform_supported == Some(false) {
            let warn_bg = iced::Color::from_rgb(0.95, 0.65, 0.0);
            content = content.push(
                container(
                    text(self.t("dash_unsupported_platform").to_string())
                        .size(12)
                        .color(iced::Color::WHITE),
                )
                .padding([10, 16])
                .width(Length::Fill)
                .style(move |_t: &Theme| container::Style {
                    background: Some(warn_bg.into()),
                    border: iced::Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        if let Some(ltbox_device::driver::DriverStatus::Missing(_)) = self.driver_status {
            content = content.push(self.driver_warning_banner());
        }

        let mut device_col = column![].spacing(0).width(Length::Fill);
        device_col = device_col.push(
            text(self.t("dash_device").to_string())
                .size(13)
                .style(label_style)
                .line_height(1.0),
        );
        device_col = device_col.push(Space::new().height(4));
        if !self.device_market_name.is_empty() {
            device_col = device_col.push(
                text(self.device_market_name.clone())
                    .size(16)
                    .line_height(1.0),
            );
        }
        device_col = device_col.push(Space::new().height(12));
        device_col = device_col.push(
            row![
                info_kv(self.t("device_model"), model),
                info_kv(self.t("device_ram"), ram),
                info_kv(self.t("device_storage"), storage),
                info_kv(self.t("device_slot"), slot),
            ]
            .spacing(40),
        );
        device_col = device_col.push(Space::new().height(6));
        device_col = device_col.push(
            row![
                info_kv(self.t("device_arb"), arb),
                info_kv(self.t("device_firmware"), firmware),
            ]
            .spacing(40),
        );

        // Pin the inner row to 160 px regardless of whether the device is
        // populated. Without this the empty-state card collapses to the
        // text column's natural height, then jumps taller once a device
        // connects — same card, two different sizes. The portrait branch
        // already used `height(160)`; the empty branch now matches so the
        // dashboard layout doesn't reflow on connect.
        let device_card_inner: Element<'_, Message> = if self.device_model.is_empty() {
            container(device_col).width(Length::Fill).height(160).into()
        } else {
            let portrait: Element<'_, Message> = match device_portrait(&self.device_model) {
                DevicePortrait::Png(h) => iced::widget::image(h)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .into(),
                DevicePortrait::Svg(h) => iced::widget::svg(h)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .into(),
            };
            // Click on the portrait fires the Lenovo PTSTPD lookup popup.
            // Skip when no serial was captured (e.g. EDL connection) so
            // the click is a clear no-op rather than triggering an empty
            // upstream query.
            let portrait_box = container(portrait)
                .width(220)
                .height(Length::Fill)
                .center_x(220)
                .center_y(Length::Fill);
            let portrait_clickable: Element<'_, Message> = if self.device_serial.is_empty() {
                portrait_box.into()
            } else {
                iced::widget::mouse_area(portrait_box)
                    .on_press(Message::DeviceInfoOpen)
                    .interaction(iced::mouse::Interaction::Pointer)
                    .into()
            };
            row![device_col, portrait_clickable,]
                .spacing(16)
                .align_y(iced::Alignment::Center)
                .height(160)
                .into()
        };
        content = content.push(
            container(
                container(device_card_inner)
                    .padding(iced::Padding {
                        top: 10.0,
                        right: 18.0,
                        bottom: 14.0,
                        left: 18.0,
                    })
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .style(|t: &Theme| {
                theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 0)
            }),
        );
        content = content.push(card(self.t("dash_current_operation"), op_text));
        // Read-only text_editor so drag-select + Ctrl+C work. `Length::Fill`
        // height so the editor expands to fill whatever space the parent log
        // card claims — combined with the log card's own `height(Fill)` and
        // the dashboard column's `height(Fill)`, this is what makes the log
        // grow to balance the dashboard's top and bottom padding.
        let dash_log_editor: Element<'_, Message> = iced::widget::text_editor(&self.log_editor)
            .on_action(Message::LogEditorAction)
            .size(11)
            .height(Length::Fill)
            .into();
        // Bottom-right "Save Log" — same neutral pill the wizard exec
        // step uses, sized so the label doesn't get clipped at any
        // language. Right-aligned via a Fill spacer.
        let dash_save_btn = button(
            text(self.t("btn_save_log").to_string())
                .size(11)
                .style(muted_style)
                .center(),
        )
        .on_press(Message::SaveLog)
        .padding([4, 14])
        .style(neutral_pill_btn_style);
        let dash_log_card = container(
            column![
                text(self.t("dash_log").to_string())
                    .size(13)
                    .style(label_style)
                    .line_height(1.0),
                dash_log_editor,
                row![Space::new().width(Length::Fill), dash_save_btn]
                    .align_y(iced::Alignment::Center),
            ]
            .spacing(8)
            .padding(iced::Padding {
                top: 10.0,
                right: 18.0,
                bottom: 14.0,
                left: 18.0,
            })
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|t: &Theme| {
            theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 1)
        });
        content = content.push(dash_log_card);
        content.into()
    }

    // -- Settings ---------------------------------------------------------

    fn view_settings(&self) -> Element<'_, Message> {
        let s = &self.settings;

        // Single untitled "Preferences" card holds language + theme.
        let lang_row = row![
            text(self.t("settings_language").to_string())
                .size(13)
                .width(Length::Fill),
            widget::pick_list(
                LANGUAGES.iter().map(|l| l.label()).collect::<Vec<_>>(),
                Some(s.language.label()),
                |selected| {
                    let l = LANGUAGES
                        .iter()
                        .find(|l| l.label() == selected)
                        .copied()
                        .unwrap_or(Language::En);
                    Message::Settings(SettingsMsg::SetLanguage(l))
                },
            )
            .width(160),
        ]
        .align_y(iced::Alignment::Center);

        let t_system = self.t(ThemeChoice::System.label_key()).to_string();
        let t_light = self.t(ThemeChoice::Light.label_key()).to_string();
        let t_dark = self.t(ThemeChoice::Dark.label_key()).to_string();
        let current_theme_label = match self.theme_choice {
            ThemeChoice::System => t_system.clone(),
            ThemeChoice::Light => t_light.clone(),
            ThemeChoice::Dark => t_dark.clone(),
        };
        let theme_options: Vec<String> = vec![t_system.clone(), t_light.clone(), t_dark.clone()];
        let theme_row = row![
            text(self.t("settings_theme").to_string())
                .size(13)
                .width(Length::Fill),
            widget::pick_list(theme_options, Some(current_theme_label), move |selected| {
                let choice = if selected == t_system {
                    ThemeChoice::System
                } else if selected == t_dark {
                    ThemeChoice::Dark
                } else {
                    ThemeChoice::Light
                };
                Message::SetTheme(choice)
            },)
            .width(160),
        ]
        .align_y(iced::Alignment::Center);

        // Default EDL loader row — single-device convenience that
        // makes every loader picker auto-fill from this stored path.
        // The (?) icon is rendered as a `widget::tooltip` so the
        // explanation only takes screen real estate when the user is
        // actively pointing at it.
        let default_loader_help = self.t("settings_default_loader_help").to_string();
        let help_icon = widget::tooltip(
            container(text("?").size(11).style(label_style))
                .padding([2, 6])
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(with_alpha(p.on_surface_variant, 0.10).into()),
                        border: iced::Border {
                            radius: theme::shape::FULL.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            container(text(default_loader_help).size(11))
                .padding([6, 10])
                .max_width(280)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(p.surface_container_high.into()),
                        text_color: Some(p.on_surface),
                        border: iced::Border {
                            color: p.outline_variant,
                            width: 1.0,
                            radius: theme::shape::SM.into(),
                        },
                        shadow: theme::elevation(2, is_dark(t)),
                        ..Default::default()
                    }
                }),
            widget::tooltip::Position::Right,
        );

        let mut default_loader_actions = row![
            button(text(self.t("settings_default_loader_browse").to_string()).size(13))
                .on_press(Message::Settings(SettingsMsg::SettingsPickDefaultLoader))
                .padding([6, 14])
                .style(neutral_pill_btn_style),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if self.default_loader_path.is_some() {
            default_loader_actions = default_loader_actions.push(
                button(text(self.t("settings_default_loader_clear").to_string()).size(13))
                    .on_press(Message::Settings(SettingsMsg::SettingsClearDefaultLoader))
                    .padding([6, 14])
                    .style(neutral_pill_btn_style),
            );
        }

        let default_loader_top = row![
            text(self.t("settings_default_loader").to_string())
                .size(13)
                .line_height(1.0),
            help_icon,
            Space::new().width(Length::Fill),
            default_loader_actions,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let default_loader_path_str = self
            .default_loader_path
            .clone()
            .unwrap_or_else(|| self.t("settings_default_loader_unset").to_string());
        let default_loader_row = column![
            default_loader_top,
            text(default_loader_path_str).size(11).style(muted_style),
        ]
        .spacing(6);

        let prefs_card = container(
            column![lang_row, theme_row, default_loader_row,]
                .spacing(14)
                .padding(iced::Padding {
                    top: 14.0,
                    right: 18.0,
                    bottom: 14.0,
                    left: 18.0,
                })
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::MD.into(),
                },
                shadow: theme::elevation(1, is_dark(t)),
                ..Default::default()
            }
        });

        column![
            text(self.t("settings_title").to_string()).size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
            prefs_card,
        ]
        .spacing(14)
        .width(Length::Fill)
        .into()
    }

    // -- Flash Wizard ------------------------------------------------------

    fn view_flash_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash.is_in_exec() {
            return self.log_popup_view();
        }
        let step_labels: Vec<&str> = FLASH_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash.step);
        let body = match self.flash.step {
            0 => self.flash_region_step(),
            1 => self.flash_target_step(),
            2 => self.flash_data_step(),
            3 => self.flash_folder_step(),
            4 => self.flash_confirm_step(),
            _ => self.flash_exec_step(),
        };
        let nav = if self.flash.step < 5 {
            let is_start = self.flash.step == 4;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.flash.can_next() && !(self.busy && is_start);
            wizard_nav_generic(
                self.flash.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Flash(FlashMsg::FlashBack),
                Message::Flash(FlashMsg::FlashNext),
            )
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_region_step(&self) -> Element<'_, Message> {
        let prc_icon = lucide_primary(icon::region_prc(), 57.6);
        let row_icon = lucide_primary(icon::region_row(), 57.6);
        let col = column![
            text(self.t("flash_region_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_region_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    prc_icon,
                    self.t("region_prc"),
                    self.t("region_prc_name"),
                    self.flash.device_region == Some(DeviceRegion::Prc),
                    Message::Flash(FlashMsg::FlashRegion(DeviceRegion::Prc))
                ),
                icon_option_card_sub(
                    row_icon,
                    self.t("region_row"),
                    self.t("region_row_name"),
                    self.flash.device_region == Some(DeviceRegion::Row),
                    Message::Flash(FlashMsg::FlashRegion(DeviceRegion::Row))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_target_step(&self) -> Element<'_, Message> {
        let globe = lucide_primary(icon::tile_globe(), 57.6);
        let device = lucide_primary(icon::tile_device(), 57.6);
        let col = column![
            text(self.t("flash_target_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_target_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    globe,
                    self.t(FlashTarget::OtherRegion.label_key()),
                    self.t("flashtarget_other_desc"),
                    self.flash.target == Some(FlashTarget::OtherRegion),
                    Message::Flash(FlashMsg::FlashTarget(FlashTarget::OtherRegion))
                ),
                icon_option_card_sub(
                    device,
                    self.t(FlashTarget::SameRegion.label_key()),
                    self.t("flashtarget_same_desc"),
                    self.flash.target == Some(FlashTarget::SameRegion),
                    Message::Flash(FlashMsg::FlashTarget(FlashTarget::SameRegion))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_data_step(&self) -> Element<'_, Message> {
        let shield = lucide_primary(icon::tile_shield(), 57.6);
        let wipe = lucide_primary(icon::tile_wipe(), 57.6);
        let col = column![
            text(self.t("flash_data_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_data_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    shield,
                    self.t(DataMode::Keep.label_key()),
                    self.t("datamode_keep_desc"),
                    self.flash.data_mode == Some(DataMode::Keep),
                    Message::Flash(FlashMsg::FlashDataMode(DataMode::Keep))
                ),
                icon_option_card_sub(
                    wipe,
                    self.t(DataMode::Wipe.label_key()),
                    self.t("datamode_wipe_desc"),
                    self.flash.data_mode == Some(DataMode::Wipe),
                    Message::Flash(FlashMsg::FlashDataMode(DataMode::Wipe))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_folder_step(&self) -> Element<'_, Message> {
        let selected = self.flash.firmware_folder.is_some();
        let status = if let Some(p) = &self.flash.firmware_folder {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_folder").to_string())
                        .size(14)
                        .center(),
                    text(self.t("flash_folder_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Flash(FlashMsg::FlashSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_chips(
            self.recent_paths
                .recent(PickerTarget::FlashFolder.kind().storage_key()),
            |p| Message::RecentFolderPicked(PickerTarget::FlashFolder, p),
            "picker_recents",
            false,
        );
        let col = column![
            text(self.t("flash_folder_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let region = self
            .flash
            .device_region
            .map(|r| self.t(r.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let target = self
            .flash
            .target
            .map(|t| self.t(t.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let data = self
            .flash
            .data_mode
            .map(|d| self.t(d.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let modify_region = self
            .t(if self.wf_config.modify_region {
                "rollback_on"
            } else {
                "rollback_off"
            })
            .to_string();
        let rollback = self
            .t(self.wf_config.modify_rollback.label_key())
            .to_string();
        let mut col = column![
            text(self.t("flash_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            info_kv_center(self.t("flash_region_title"), &region),
            info_kv_center(self.t("flash_target_title"), &target),
            info_kv_center(self.t("flash_data_title"), &data),
            info_kv_center(self.t("adv_section_region_patch"), &modify_region),
            info_kv_center(self.t("device_arb"), &rollback),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        if let Some(cc) = self.wf_config.country_action.target() {
            let entry = COUNTRY_CODES.iter().find(|e| e.code == cc);
            let label = entry
                .map(|e| format!("{} — {}", e.code, e.name))
                .unwrap_or_else(|| cc.to_string());
            col = col.push(info_kv_center(self.t("popup_select_country"), &label));
        } else if self.wf_config.wipe && self.wf_config.country_action.is_skipped() {
            col = col.push(info_kv_center(
                self.t("popup_select_country"),
                self.t("flash_confirm_country_skip"),
            ));
        }
        let folder_owned = self
            .flash
            .firmware_folder
            .clone()
            .unwrap_or_else(|| dash.clone());
        col = col.push(info_kv_center(self.t("flash_folder_title"), &folder_owned));

        // Destructive-op callout — parity with v2 `_confirm_full_flash_overwrite`.
        // The wizard's Next button is the trigger, so surface the hazard
        // inline instead of trusting the summary alone. Uses the palette's
        // `warning` colour (amber) so it doesn't read as an error/failure.
        let warning_key = if self.wf_config.wipe {
            "flash_confirm_warning_wipe"
        } else {
            "flash_confirm_warning"
        };
        col = col.push(widget::rule::horizontal(1));
        col = col.push(
            text(self.t(warning_key).to_string())
                .size(13)
                .style(warning_style)
                .center(),
        );

        // Wrap in scrollable so the summary can grow past the viewport
        // (e.g. ARB ON + country patch + region modify all push extra
        // info_kv rows). Nav row stays outside this fn — `view_flash_wizard`
        // composes `[step_bar, body, nav]`, so Back / Start stay sticky at
        // the bottom even when content scrolls.
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }

    // -- System Update Wizard ----------------------------------------------

    fn view_sysupdate_wizard(&self) -> Element<'_, Message> {
        // Exec-step log popup overlay — without this the "Show log" button
        // on the exec card was a no-op for System Update (Flash/Root/Unroot
        // all had it wired; SysUpdate had been missed).
        if self.log_popup_open && self.sysupdate.is_in_exec() {
            return self.log_popup_view();
        }
        let steps = self.sysupdate.steps();
        let step_labels: Vec<&str> = steps.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.sysupdate.step);
        let is_rescue = self.sysupdate.is_rescue();
        let body = if is_rescue {
            match self.sysupdate.step {
                0 => self.sysupdate_action_step(),
                1 => self.sysupdate_rescue_folder_step(),
                2 => self.sysupdate_confirm_step(),
                _ => self.sysupdate_exec_step(),
            }
        } else {
            match self.sysupdate.step {
                0 => self.sysupdate_action_step(),
                1 => self.sysupdate_confirm_step(),
                _ => self.sysupdate_exec_step(),
            }
        };
        let last_nav_step = steps.len() - 2; // Exec step has no nav row.
        let nav = if self.sysupdate.step <= last_nav_step {
            let is_start = self.sysupdate.step == last_nav_step;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.sysupdate.can_next() && !(self.busy && is_start);
            wizard_nav_generic(
                self.sysupdate.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Sys(SysMsg::SysBack),
                Message::Sys(SysMsg::SysNext),
            )
        } else {
            container(text("")).into()
        };
        let core: Element<'_, Message> = column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        if self.sysupdate.rescue_region_popup_open {
            iced::widget::Stack::with_children(vec![core, self.rescue_region_popup_view()]).into()
        } else {
            core
        }
    }

    fn sysupdate_action_step(&self) -> Element<'_, Message> {
        let off_icon = lucide_primary(icon::tile_update_off(), 57.6);
        let on_icon = lucide_primary(icon::tile_update_on(), 57.6);
        let rescue_icon = lucide_primary(icon::tile_rescue(), 57.6);
        let rescue_disabled = self.platform_supported == Some(false);
        let mut cards = row![
            icon_option_card_sub(
                off_icon,
                self.t(SysUpdateAction::Disable.label_key()),
                self.t(SysUpdateAction::Disable.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Disable),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Disable)),
            ),
            icon_option_card_sub(
                on_icon,
                self.t(SysUpdateAction::Enable.label_key()),
                self.t(SysUpdateAction::Enable.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Enable),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Enable)),
            ),
        ]
        .spacing(12);
        if rescue_disabled {
            // Disabled rescue card — no on_press, grayed out; still mirrors
            // the sub-row layout of the other tiles with the Qualcomm-required
            // hint so the label sits at the same height.
            let content = column![
                icon_tile(rescue_icon),
                text(self.t("sysupdate_rescue").to_string())
                    .size(13)
                    .width(Length::Fill)
                    .center()
                    .style(label_style),
                text(self.t("sysupdate_rescue_req").to_string())
                    .size(11)
                    .width(Length::Fill)
                    .center()
                    .style(label_style),
            ]
            .spacing(8)
            .align_x(iced::Alignment::Center);
            cards = cards.push(
                button(
                    container(content)
                        .padding([20, 16])
                        .width(Length::Fill)
                        .height(WIZARD_CARD_HEIGHT)
                        .center_y(WIZARD_CARD_HEIGHT)
                        .style(|t: &Theme| {
                            theme::surface_card_style(
                                t,
                                theme::SurfaceLevel::Lowest,
                                theme::shape::MD,
                                0,
                            )
                        }),
                )
                .padding(0)
                .width(Length::Fill)
                .style(|t: &Theme, _s| button::Style {
                    background: None,
                    text_color: pal_of(t).on_surface,
                    ..Default::default()
                }),
            );
        } else {
            cards = cards.push(icon_option_card_sub(
                rescue_icon,
                self.t(SysUpdateAction::Rescue.label_key()),
                self.t(SysUpdateAction::Rescue.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Rescue),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Rescue)),
            ));
        }
        let col = column![
            text(self.t("sysupdate_action_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("sysupdate_action_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            cards,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn sysupdate_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let action = self
            .sysupdate
            .action
            .map(|a| self.t(a.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let desc = self
            .sysupdate
            .action
            .map(|a| self.t(a.desc_key()).to_string())
            .unwrap_or_default();
        let mut col = column![
            text(self.t("sysupdate_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(desc).size(13).style(muted_style).center(),
            widget::rule::horizontal(1),
            info_kv_center(self.t("sysupdate_step_action"), &action),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        // Rescue: echo the chosen firmware folder + region so the user
        // confirms exactly what's about to flash.
        if self.sysupdate.is_rescue() {
            let folder = self
                .sysupdate
                .rescue_folder
                .clone()
                .unwrap_or_else(|| dash.clone());
            let region = self
                .sysupdate
                .rescue_region
                .map(|r| self.t(r.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            col = col.push(info_kv_center(self.t("rescue_folder_label"), &folder));
            col = col.push(info_kv_center(self.t("rescue_region_label"), &region));
        }
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn sysupdate_rescue_folder_step(&self) -> Element<'_, Message> {
        // Boot Recovery now consumes only the EDL loader file —
        // dump+flash use GPT-by-name on a fixed LUN, no rawprogram*.xml
        // is read. Step layout still matches the flash / root / unroot
        // pickers (title + 280-wide card button + status path + recent
        // chips), just with file-picker semantics.
        let selected = self.sysupdate.rescue_folder.is_some();
        let status = if let Some(p) = &self.sysupdate.rescue_folder {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("rescue_folder_subtitle").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Sys(SysMsg::SysRescueSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        // Loader recents share the File bucket with other loader
        // pickers (root, advanced) — filter to the same ext set the
        // dialog itself accepts.
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::Sys(SysMsg::SysRescueFolderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("rescue_folder_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn rescue_region_popup_view(&self) -> Element<'_, Message> {
        let mk_option = |region: RescueRegion, desc_key: &'static str| {
            let label = self.t(region.label_key()).to_string();
            let desc = self.t(desc_key).to_string();
            let selected = self.sysupdate.rescue_region == Some(region);
            button(
                column![
                    text(label).size(15).style(on_surface_style),
                    text(desc).size(12).style(muted_style),
                ]
                .spacing(4),
            )
            .on_press(Message::Sys(SysMsg::SysRescueRegion(region)))
            .padding([10, 16])
            .width(Length::Fill)
            .style(move |t: &Theme, status| {
                let p = pal_of(t);
                let hover = matches!(status, button::Status::Hovered);
                let bg = if selected {
                    p.primary_container.into()
                } else if hover {
                    with_alpha(p.primary, theme::state::HOVER).into()
                } else {
                    iced::Color::TRANSPARENT.into()
                };
                button::Style {
                    background: Some(bg),
                    text_color: p.on_surface,
                    border: iced::Border {
                        color: if selected {
                            p.primary
                        } else {
                            p.outline_variant
                        },
                        width: 1.0,
                        radius: theme::shape::SM.into(),
                    },
                    ..Default::default()
                }
            })
        };
        let popup_content = container(
            column![
                row![
                    text(self.t("rescue_region_popup_title").to_string()).size(16),
                    Space::new().width(Length::Fill),
                    button(
                        text(self.t("btn_cancel").to_string())
                            .size(12)
                            .style(muted_style)
                    )
                    .on_press(Message::Sys(SysMsg::SysRescueRegionPopupDismiss))
                    .padding([4, 12])
                    .style(neutral_pill_btn_style),
                ]
                .align_y(iced::Alignment::Center),
                widget::rule::horizontal(1),
                text(self.t("rescue_region_popup_subtitle").to_string())
                    .size(12)
                    .style(muted_style),
                mk_option(RescueRegion::Prc, "rescue_region_prc_desc"),
                mk_option(RescueRegion::Row, "rescue_region_row_desc"),
            ]
            .spacing(10)
            .padding(20)
            .width(420),
        )
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::MD.into(),
                },
                shadow: iced::Shadow {
                    color: with_alpha(p.shadow, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 20.0,
                },
                ..Default::default()
            }
        });
        // Dim-scrim under the dialog so the wizard behind it doesn't
        // visually compete with the choice.
        container(popup_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_t: &Theme| container::Style {
                background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4).into()),
                ..Default::default()
            })
            .into()
    }

    fn sysupdate_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }

    /// Reusable exec-step view with collapsible log panel.
    fn exec_step_view(&self) -> Element<'_, Message> {
        let (title, detail) = if self.busy {
            (
                self.t("exec_executing_title").to_string(),
                self.t("exec_executing_subtitle").to_string(),
            )
        } else if self.error_msg.is_some() {
            (
                self.t("exec_failed_title").to_string(),
                self.t("exec_failed_subtitle").to_string(),
            )
        } else {
            (
                self.t("exec_done_title").to_string(),
                self.t("exec_done_subtitle").to_string(),
            )
        };
        let is_error = self.error_msg.is_some();
        let is_busy = self.busy;

        // Current-step card: spinner while running, static glyph
        // on terminal states. Every wizard funnels through this view,
        // so swapping the running glyph for an animated `Spinner`
        // here unifies the in-progress visual across Flash / Root /
        // Unroot / SystemUpdate / Advanced — previously a couple of
        // them showed a static lucide glyph (the firmware-flash one
        // rendered as a sun) and looked frozen.
        let step_icon: Element<'_, Message> = if is_error {
            lucide_icon(icon::op_failed(), 72.0, |t: &Theme| pal_of(t).error)
        } else if is_busy {
            container(
                Spinner::new()
                    .width(Length::Fixed(56.0))
                    .height(Length::Fixed(56.0))
                    .circle_radius(3.5),
            )
            .width(72)
            .height(72)
            .center_x(72)
            .center_y(72)
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    text_color: Some(p.primary),
                    ..Default::default()
                }
            })
            .into()
        } else {
            lucide_icon(icon::op_done(), 72.0, |t: &Theme| pal_of(t).success)
        };

        let (eyebrow_text, label_text) = if self.op_steps.is_empty() {
            (String::new(), detail.clone())
        } else {
            let idx = self.current_op_step.min(self.op_steps.len() - 1);
            let total = self.op_steps.len();
            let step = &self.op_steps[idx];
            let eyebrow_key = if is_error {
                "exec_step_eyebrow_failed"
            } else if is_busy {
                "exec_step_eyebrow_running"
            } else {
                "exec_step_eyebrow_done"
            };
            let eyebrow = self
                .t(eyebrow_key)
                .replace("{n}", &(idx + 1).to_string())
                .replace("{total}", &total.to_string());
            (eyebrow, step.label.clone())
        };

        let eyebrow_node: Element<'_, Message> = if eyebrow_text.is_empty() {
            Space::new().height(0).into()
        } else {
            text(eyebrow_text)
                .size(11)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if is_error {
                        p.error
                    } else if is_busy {
                        p.primary
                    } else {
                        p.success
                    };
                    iced::widget::text::Style { color: Some(color) }
                })
                .into()
        };

        let card_body = column![
            eyebrow_node,
            text(label_text).size(16).style(on_surface_style),
        ]
        .spacing(4)
        .width(Length::Fill);
        let card_row = row![step_icon, card_body]
            .spacing(20)
            .align_y(iced::Alignment::Center);
        let step_card = container(card_row)
            .padding([24, 28])
            .max_width(560)
            .width(Length::Fill)
            .style(move |t: &Theme| {
                let p = pal_of(t);
                let accent = if is_error {
                    p.error
                } else if is_busy {
                    p.primary
                } else {
                    p.success
                };
                container::Style {
                    background: Some(p.surface_container.into()),
                    border: iced::Border {
                        color: accent,
                        width: 1.5,
                        radius: theme::shape::MD.into(),
                    },
                    shadow: theme::elevation(2, is_dark(t)),
                    ..Default::default()
                }
            });

        let pill_style = neutral_pill_btn_style;
        let show_btn = button(
            text(self.t("btn_show_log").to_string())
                .size(11)
                .style(muted_style)
                .center(),
        )
        .on_press(Message::ToggleLogPopup(true))
        .padding([4, 12])
        .style(pill_style);
        let save_btn = button(
            text(self.t("btn_save_log").to_string())
                .size(11)
                .style(muted_style)
                .center(),
        )
        .on_press(Message::SaveLog)
        .padding([4, 12])
        .style(pill_style);

        let mut button_row = row![show_btn, save_btn].spacing(8);

        // "Open Folder" pill for Advanced ops that produce output —
        // guarded on non-busy to avoid racing the file-manager launch.
        if !self.busy
            && self.current_view == View::Advanced
            && self.adv_wizard.output_dir.is_some()
            && self
                .adv_wizard
                .action
                .map(|a| a.produces_output())
                .unwrap_or(false)
        {
            let open_btn = button(
                text(self.t("btn_open_folder").to_string())
                    .size(11)
                    .style(muted_style)
                    .center(),
            )
            .on_press(Message::Adv(AdvMsg::AdvWizOpenOutputFolder))
            .padding([4, 12])
            .style(pill_style);
            button_row = button_row.push(open_btn);
        }

        if !self.busy {
            let start_over = button(
                text(self.t("btn_start_over").to_string())
                    .size(11)
                    .style(muted_style)
                    .center(),
            )
            .on_press(Message::StartOver)
            .padding([4, 12])
            .style(pill_style);
            button_row = button_row.push(start_over);
        }

        let col = column![
            text(title)
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center()
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if is_error {
                        p.error
                    } else if is_busy {
                        p.primary
                    } else {
                        p.success
                    };
                    iced::widget::text::Style { color: Some(color) }
                }),
            text(detail).size(13).style(muted_style).center(),
            Space::new().height(8),
            step_card,
            Space::new().height(6),
            button_row,
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Full-viewport log popup. Replaces the wizard body while open;
    /// dismissed via Close.
    fn log_popup_view(&self) -> Element<'_, Message> {
        let editor = iced::widget::text_editor(&self.log_editor)
            .on_action(Message::LogEditorAction)
            .size(11)
            .height(Length::Fill);
        let body = column![
            row![
                text(self.t("log_popup_title").to_string()).size(theme::text_size::TITLE_LARGE),
                Space::new().width(Length::Fill),
                button(text(self.t("btn_save_log").to_string()).size(12))
                    .on_press(Message::SaveLog)
                    .padding([6, 14])
                    .style(|t: &Theme, _s| {
                        let p = pal_of(t);
                        button::Style {
                            background: Some(with_alpha(p.on_surface, 0.1).into()),
                            text_color: p.on_surface,
                            border: iced::Border {
                                radius: 6.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
                button(text(self.t("btn_close").to_string()).size(12))
                    .on_press(Message::ToggleLogPopup(false))
                    .padding([6, 16])
                    .style(|_t: &Theme, status| {
                        let a = match status {
                            button::Status::Hovered => 1.0,
                            _ => 0.85,
                        };
                        button::Style {
                            background: Some(iced::Color { a, ..ACCENT }.into()),
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: 6.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            widget::rule::horizontal(1),
            container(editor)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(10)
                .style(|t: &Theme| theme::surface_card_style(
                    t,
                    theme::SurfaceLevel::Low,
                    theme::shape::SM,
                    0,
                )),
        ]
        .spacing(12)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill);
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Unroot Wizard ----------------------------------------------------

    fn view_unroot_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.unroot.is_in_exec() {
            return self.log_popup_view();
        }
        let step_labels: Vec<&str> = UNROOT_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.unroot.step);
        let body = match self.unroot.step {
            0 => self.unroot_type_step(),
            1 => self.unroot_folder_step(),
            2 => self.unroot_confirm_step(),
            _ => self.unroot_exec_step(),
        };
        let nav = if self.unroot.step < 3 {
            let is_start = self.unroot.step == 2;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.unroot.can_next() && !(self.busy && is_start);
            wizard_nav_generic(
                self.unroot.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Unroot(UnrootMsg::UnrootBack),
                Message::Unroot(UnrootMsg::UnrootNext),
            )
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn unroot_type_step(&self) -> Element<'_, Message> {
        // Unroot reuses the Lucide puzzle/layers glyphs that the root
        // wizard uses for the LKM/GKI pick — context (title + label)
        // disambiguates.
        let lkm_icon = lucide_primary(icon::root_lkm(), 57.6);
        let gki_icon = lucide_primary(icon::root_gki(), 57.6);
        let col = column![
            text(self.t("unroot_method_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("unroot_method_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    lkm_icon,
                    self.t(UnrootType::MagiskLkm.label_key()),
                    self.t(UnrootType::MagiskLkm.desc_key()),
                    self.unroot.unroot_type == Some(UnrootType::MagiskLkm),
                    Message::Unroot(UnrootMsg::SetUnrootType(UnrootType::MagiskLkm))
                ),
                icon_option_card_sub(
                    gki_icon,
                    self.t(UnrootType::APatchGki.label_key()),
                    self.t(UnrootType::APatchGki.desc_key()),
                    self.unroot.unroot_type == Some(UnrootType::APatchGki),
                    Message::Unroot(UnrootMsg::SetUnrootType(UnrootType::APatchGki))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn unroot_folder_step(&self) -> Element<'_, Message> {
        let selected = self.unroot.folder_path.is_some();
        let desc_owned = self
            .unroot
            .unroot_type
            .map(|t| self.t(t.folder_desc_key()).to_string())
            .unwrap_or_else(|| self.t("unroot_folder_placeholder").to_string());
        let status = if let Some(p) = &self.unroot.folder_path {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_folder").to_string())
                        .size(14)
                        .center(),
                    text(desc_owned).size(11).style(muted_style).center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Unroot(UnrootMsg::UnrootSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_chips(
            self.recent_paths
                .recent(PickerTarget::UnrootFolder.kind().storage_key()),
            |p| Message::RecentFolderPicked(PickerTarget::UnrootFolder, p),
            "picker_recents",
            false,
        );
        let mut col = column![
            text(self.t("unroot_folder_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        // Loader sub-row — separate from the backup folder picker
        // because Unroot's folder is for boot.img + vbmeta.img only.
        // Hidden when the wizard already has a loader (typically
        // pre-filled from the Settings default at view enter time);
        // otherwise the user picks a loader file here.
        if self.unroot.loader_path.is_none() {
            let loader_btn = button(
                container(
                    column![
                        text(self.t("btn_browse_loader").to_string())
                            .size(14)
                            .center(),
                        text(self.t("dump_parts_loader_desc").to_string())
                            .size(11)
                            .style(muted_style)
                            .center(),
                    ]
                    .spacing(6)
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                )
                .padding([16, 24])
                .width(280)
                .style(move |t: &Theme| sel_card_style(t, false)),
            )
            .on_press(Message::Unroot(UnrootMsg::UnrootSelectLoader))
            .padding(0)
            .style(|t: &Theme, status| sel_card_btn_style(t, status, false));
            col = col.push(loader_btn);
        } else if let Some(p) = self.unroot.loader_path.as_deref() {
            col = col.push(
                text(format!("{}: {}", self.t("btn_browse_loader"), p))
                    .size(11)
                    .style(muted_style)
                    .center(),
            );
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn unroot_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let method = self
            .unroot
            .unroot_type
            .map(|t| self.t(t.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let folder = self
            .unroot
            .folder_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        let col = column![
            text(self.t("unroot_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("unroot_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            info_kv_center(self.t("unroot_step_method"), &method),
            info_kv_center(self.t("unroot_folder_title"), &folder),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn unroot_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }

    // -- Root Wizard ------------------------------------------------------

    fn view_root_wizard(&self) -> Element<'_, Message> {
        // Superkey / Run-ID / Kernel-version popups all render as
        // top-level M3 dialog overlays via `view()`'s layer stack —
        // do NOT early-return for any of them here, otherwise the
        // KPM step underneath would unmount and Cancel couldn't
        // restore the curated list.
        if self.log_popup_open && self.root.is_in_exec() {
            return self.log_popup_view();
        }
        let steps = self.root.active_steps();
        let step_labels: Vec<&str> = steps.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.root.display_step());
        let body = match self.root.step {
            0 => self.root_family_step(),
            1 => self.root_mode_step(),
            2 => {
                if self.root.is_gki() {
                    self.root_file_step(self.t("root_kernel_title"), self.t("root_kernel_subtitle"))
                } else {
                    self.root_provider_step()
                }
            }
            3 => {
                if self.root.is_forks() {
                    self.root_file_step(self.t("root_apk_title"), self.t("root_apk_subtitle"))
                } else {
                    self.root_version_step()
                }
            }
            4 => self.root_nightly_source_step(),
            5 => self.root_folder_step(),
            6 => self.root_confirm_step(),
            8 => self.root_kpm_step(),
            _ => self.root_flash_step(),
        };
        // Step 7 is in-progress — no nav. Step 8 (APatch KPM) needs
        // the normal Back/Next bar, so exclude only 7 explicitly.
        let nav = if self.root.step != 7 {
            let is_start = self.root.step == 6;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.root.can_next() && !(self.busy && is_start);
            wizard_nav(self.root.step > 0, &label_owned, can, self.t("btn_back"))
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn root_kpm_step(&self) -> Element<'_, Message> {
        // No recents here — the KPM list already competes for vertical space.
        let kpm_selected = !self.root.kpm_paths.is_empty();
        let pick_btn = button(
            container(
                column![
                    text(self.t("btn_browse_kpm").to_string()).size(14).center(),
                    text(self.t("root_kpm_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, kpm_selected)),
        )
        .on_press(Message::Root(RootMsg::RootSelectKpm))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, kpm_selected));

        let mut list = column![].spacing(4).width(Length::Fill);
        for path in &self.root.kpm_paths {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let p_copy = path.clone();
            let remove = button(text("−").size(14))
                .padding([2, 10])
                .on_press(Message::Root(RootMsg::RootKpmRemove(p_copy)))
                .style(|t: &Theme, _s| {
                    let p = pal_of(t);
                    button::Style {
                        background: Some(with_alpha(p.on_surface, 0.10).into()),
                        text_color: p.on_surface,
                        border: iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });
            list = list.push(
                row![remove, text(name).size(12).style(on_surface_style),]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
            );
        }

        let col = column![
            text(self.t("root_kpm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_kpm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            pick_btn,
            list,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_superkey_popup(&self) -> Element<'_, Message> {
        // M3 text-input dialog — same shape as root_run_id_popup /
        // root_kernel_version_popup so the three APatch-flow popups
        // feel consistent (380 wide, outlined Cancel + filled OK,
        // shared `m3_dialog` scrim + 28-radius card).
        let input = iced::widget::text_input(
            self.t("apatch_superkey_placeholder"),
            &self.root.superkey_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootSuperkeyInput(__v)))
        .on_submit(Message::Root(RootMsg::RootSuperkeyConfirm))
        .secure(true)
        .padding([10, 12])
        .width(Length::Fill)
        .style(|t: &Theme, status| {
            let p = pal_of(t);
            let focused = matches!(status, iced::widget::text_input::Status::Focused { .. });
            iced::widget::text_input::Style {
                background: p.surface.into(),
                border: iced::Border {
                    color: if focused {
                        p.primary
                    } else {
                        p.outline_variant
                    },
                    width: if focused { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                placeholder: with_alpha(p.on_surface, 0.5),
                icon: p.on_surface,
                value: p.on_surface,
                selection: with_alpha(p.primary, 0.3),
            }
        });

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        // Two-stage flow: first-entry vs verification re-entry. The
        // title + subtitle swap so the user knows the first Confirm
        // didn't commit the key yet, plus the password-manager / form
        // autofill heuristics in the OS see "different" prompts.
        let on_verify_stage = self.root.superkey_first_entry.is_some();
        let title_key = if on_verify_stage {
            "apatch_superkey_verify_title"
        } else {
            "apatch_superkey_title"
        };
        let subtitle_key = if on_verify_stage {
            "apatch_superkey_verify_subtitle"
        } else {
            "apatch_superkey_subtitle"
        };

        let content = column![
            text(self.t(title_key).to_string()).size(20),
            text(self.t(subtitle_key).to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootSuperkeyCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootSuperkeyConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    fn root_run_id_popup(&self) -> Element<'_, Message> {
        // M3 text-input dialog — 380 wide, outlined Cancel + filled OK.
        let input = iced::widget::text_input(
            self.t("nightly_manual_placeholder"),
            &self.root.run_id_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootRunIdInput(__v)))
        .on_submit(Message::Root(RootMsg::RootRunIdConfirm))
        .padding([10, 12])
        .width(Length::Fill)
        .style(|t: &Theme, status| {
            let p = pal_of(t);
            let focused = matches!(status, iced::widget::text_input::Status::Focused { .. });
            iced::widget::text_input::Style {
                background: p.surface.into(),
                border: iced::Border {
                    color: if focused {
                        p.primary
                    } else {
                        p.outline_variant
                    },
                    width: if focused { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                placeholder: with_alpha(p.on_surface, 0.5),
                icon: p.on_surface,
                value: p.on_surface,
                selection: with_alpha(p.primary, 0.3),
            }
        });

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        let content = column![
            text(self.t("nightly_manual_title").to_string()).size(20),
            text(self.t("nightly_manual_subtitle").to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootRunIdCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootRunIdConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    fn root_kernel_version_popup(&self) -> Element<'_, Message> {
        let input = iced::widget::text_input(
            self.t("root_kernel_version_placeholder"),
            &self.root.kernel_version_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootKernelVersionInput(__v)))
        .on_submit(Message::Root(RootMsg::RootKernelVersionConfirm))
        .padding([10, 12])
        .width(Length::Fill)
        .style(|t: &Theme, status| {
            let p = pal_of(t);
            let focused = matches!(status, iced::widget::text_input::Status::Focused { .. });
            iced::widget::text_input::Style {
                background: p.surface.into(),
                border: iced::Border {
                    color: if focused {
                        p.primary
                    } else {
                        p.outline_variant
                    },
                    width: if focused { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                placeholder: with_alpha(p.on_surface, 0.5),
                icon: p.on_surface,
                value: p.on_surface,
                selection: with_alpha(p.primary, 0.3),
            }
        });

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        let content = column![
            text(self.t("root_kernel_version_manual_title").to_string()).size(20),
            text(self.t("root_kernel_version_manual_subtitle").to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootKernelVersionCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootKernelVersionConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    fn root_family_step(&self) -> Element<'_, Message> {
        let mk = |f: Family| -> Element<'_, Message> {
            icon_option_card_sub(
                f.icon(),
                self.t(f.label_key()),
                self.t(f.desc_key()),
                self.root.family == Some(f),
                Message::Root(RootMsg::RootFamily(f)),
            )
        };

        let cards = row![mk(Family::Magisk), mk(Family::KernelSU), mk(Family::APatch),].spacing(12);

        let col = column![
            text(self.t("root_type_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_type_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            cards,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_provider_step(&self) -> Element<'_, Message> {
        let family = self.root.family.unwrap_or(Family::KernelSU);
        let providers = family.providers();
        // KernelSU has 4 providers — 2×2 grid clipped at 620 px default
        // window. Switch to a list layout only for that route.
        let is_ksu_lkm_list = family == Family::KernelSU && !self.root.is_gki();

        let grid_card = |p: Provider, selected: bool| -> Element<'_, Message> {
            let sub = p.desc_key().map(|k| self.t(k)).unwrap_or("");
            icon_option_card_sub(
                p.icon(),
                self.t(p.label_key()),
                sub,
                selected,
                Message::Root(RootMsg::RootProvider(p)),
            )
        };

        let list_card = |p: Provider, selected: bool| -> Element<'_, Message> {
            // Icon left + label/desc right; each card Fill height so
            // N cards split the space evenly.
            let icon = p.icon();
            let desc: Element<'_, Message> = match p.desc_key() {
                Some(dk) => text(self.t(dk).to_string())
                    .size(12)
                    .style(muted_style)
                    .into(),
                None => text(" ").size(12).into(),
            };
            let text_block = container(
                column![
                    text(self.t(p.label_key()).to_string())
                        .size(16)
                        .style(on_surface_style),
                    desc,
                ]
                .spacing(4)
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_y(Length::Fill);
            let body = row![icon_tile(icon), text_block]
                .spacing(16)
                .align_y(iced::Alignment::Center);
            button(
                container(body)
                    .padding([16, 20])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_y(Length::Fill)
                    .style(move |t: &Theme| sel_card_style(t, selected)),
            )
            .on_press(Message::Root(RootMsg::RootProvider(p)))
            .padding(0)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected))
            .into()
        };

        let tiles: Element<'_, Message> = if is_ksu_lkm_list {
            let mut list = column![]
                .spacing(10)
                .width(Length::Fill)
                .height(Length::Fill);
            for &p in providers {
                list = list.push(list_card(p, self.root.provider == Some(p)));
            }
            list.into()
        } else {
            // 2-col grid — Magisk / APatch (2 providers each).
            let mut grid = column![].spacing(10).width(Length::Fill);
            for chunk in providers.chunks(2) {
                let mut r = row![].spacing(10);
                for &p in chunk {
                    r = r.push(grid_card(p, self.root.provider == Some(p)));
                }
                if chunk.len() == 1 {
                    r = r.push(Space::new().width(Length::Fill));
                }
                grid = grid.push(r);
            }
            grid.into()
        };

        let title = self
            .t("root_provider_title_tmpl")
            .replace("{family}", self.t(family.label_key()));
        // KSU list claims full height; other grids stay Shrink so the
        // outer container vertical-centres them like other wizard steps.
        let col = column![
            text(title)
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_provider_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            tiles,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        let col = if is_ksu_lkm_list {
            col.height(Length::Fill)
        } else {
            col
        };
        let outer = container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill);
        if is_ksu_lkm_list {
            outer.into()
        } else {
            outer.center_y(Length::Fill).into()
        }
    }

    fn root_file_step(&self, title: &str, subtitle: &str) -> Element<'_, Message> {
        let selected = self.root.file_path.is_some();
        let status_text = if let Some(p) = &self.root.file_path {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };

        let btn_label = if self.root.is_gki() {
            self.t("btn_browse_kernel_zip")
        } else {
            self.t("btn_browse_apk")
        };

        let btn = button(
            container(
                column![
                    text(btn_label.to_string()).size(14).center(),
                    text(subtitle.to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Root(RootMsg::RootSelectFile))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));

        // Root OTA file picker flips between AnyKernel3 zip (GKI route)
        // and provider APK (Magisk fork / APatch manual) — mirror the
        // dialog filter so recents don't surface the wrong family.
        let accepted: &[&str] = if self.root.is_gki() {
            &["zip"]
        } else {
            &["apk"]
        };
        let chips = self.recent_file_chips(
            accepted,
            |p| Message::RecentFilePicked(PickerTarget::RootFile, p),
            "picker_recents",
        );
        let col = column![
            text(title.to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status_text)
                .size(12)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Recents panel — up to three chips, greyed out when stale.
    /// Per-ext-filtered recents strip for file pickers.
    ///
    /// The `File` recents bucket is shared across every file picker
    /// (APK, ZIP, KPM, `.melf`, `.img`, …) because a per-picker bucket
    /// explosion wasn't worth the storage-key churn. The strip itself
    /// would still look noisy though — a user opening the KPM picker
    /// shouldn't see their last Magisk APK. Filter at render time by
    /// the ext list the picker dialog itself accepts; empty list means
    /// "show everything" (legacy behaviour).
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
            let btn = button(text(display).size(11).style(muted_style))
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

    fn root_folder_step(&self) -> Element<'_, Message> {
        // Root pipeline now needs only the EDL loader (`.melf`) — the
        // full firmware folder was dropped when dump/flash stopped
        // depending on `rawprogram*.xml` and started resolving partition
        // names against the device's on-storage GPT. File-pick only.
        let selected = self.root.folder_path.is_some();
        let status = if let Some(p) = &self.root.folder_path {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("root_folder_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Root(RootMsg::RootSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::RecentFilePicked(PickerTarget::RootLoader, p),
            "picker_recents",
        );
        let col = column![
            text(self.t("root_folder_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_folder_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            btn,
            text(status)
                .size(12)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_mode_step(&self) -> Element<'_, Message> {
        let fam_label = self
            .root
            .family
            .map(|f| self.t(f.label_key()))
            .unwrap_or("?");
        let title = self
            .t("root_mode_title_tmpl")
            .replace("{family}", fam_label);
        let col = column![
            text(title)
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_mode_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    RootMode::Lkm.icon(),
                    self.t(RootMode::Lkm.label_key()),
                    self.t(RootMode::Lkm.desc_key()),
                    self.root.mode == Some(RootMode::Lkm),
                    Message::Root(RootMsg::RootMode(RootMode::Lkm)),
                ),
                icon_option_card_sub(
                    RootMode::Gki.icon(),
                    self.t(RootMode::Gki.label_key()),
                    self.t(RootMode::Gki.desc_key()),
                    self.root.mode == Some(RootMode::Gki),
                    Message::Root(RootMsg::RootMode(RootMode::Gki)),
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_version_step(&self) -> Element<'_, Message> {
        let mk = |choice: VerChoice| -> Element<'_, Message> {
            icon_option_card_sub(
                choice.icon(),
                self.t(choice.label_key()),
                self.t(choice.desc_key()),
                self.root.version == Some(choice),
                Message::Root(RootMsg::RootVersion(choice)),
            )
        };

        // ReSukiSU ships nightlies only — hide the Stable card so users
        // can't pick a channel that has no release assets. Other providers
        // keep both.
        let version_row = if self.root.provider == Some(Provider::ReSukiSU) {
            row![mk(VerChoice::Nightly)].spacing(12)
        } else {
            row![mk(VerChoice::Stable), mk(VerChoice::Nightly)].spacing(12)
        };

        let col = column![
            text(self.t("root_version_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_version_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            version_row,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_nightly_source_step(&self) -> Element<'_, Message> {
        let mk = |src: NightlySource| -> Element<'_, Message> {
            icon_option_card_sub(
                src.icon(),
                self.t(src.label_key()),
                self.t(src.desc_key()),
                self.root.nightly_source == Some(src),
                Message::Root(RootMsg::RootNightlySource(src)),
            )
        };

        // Committed ManualInput shows a chip beneath the cards; click re-opens.
        let chip: Element<'_, Message> =
            match (self.root.nightly_source, self.root.run_id.as_deref()) {
                (Some(NightlySource::ManualInput), Some(id)) if !id.is_empty() => {
                    let label = self.t("nightly_manual_committed").replace("{id}", id);
                    button(text(label).size(13).style(on_surface_style))
                        .padding([8, 14])
                        .on_press(Message::Root(RootMsg::RootNightlySource(
                            NightlySource::ManualInput,
                        )))
                        .style(|t: &Theme, status| {
                            let p = pal_of(t);
                            let bg_a = match status {
                                button::Status::Hovered => 0.18,
                                _ => 0.10,
                            };
                            button::Style {
                                background: Some(with_alpha(p.on_surface, bg_a).into()),
                                text_color: p.on_surface,
                                border: iced::Border {
                                    radius: 6.0.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .into()
                }
                _ => Space::new().height(0).into(),
            };

        let col = column![
            text(self.t("root_source_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("root_source_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                mk(NightlySource::AutoDetect),
                mk(NightlySource::ManualInput)
            ]
            .spacing(12),
            chip,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn root_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let fam = self
            .root
            .family
            .map(|f| self.t(f.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let mode = self
            .root
            .mode
            .map(|m| self.t(m.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());

        let mut col = column![
            text(self.t("root_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE),
            text(self.t("root_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style),
            widget::rule::horizontal(1),
            info_kv_center(self.t("root_step_type"), &fam),
            info_kv_center(self.t("root_step_mode"), &mode),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        if self.root.is_gki() {
            let path = self.root.file_path.clone().unwrap_or_else(|| dash.clone());
            col = col.push(info_kv_center(self.t("root_step_kernel"), &path));
        } else if self.root.is_forks() {
            let path = self.root.file_path.clone().unwrap_or_else(|| dash.clone());
            col = col.push(info_kv_center(
                self.t("root_step_provider"),
                self.t("provider_magisk_forks"),
            ));
            col = col.push(info_kv_center(self.t("root_step_apk"), &path));
        } else {
            let prov = self
                .root
                .provider
                .map(|p| self.t(p.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            let ver = self
                .root
                .version
                .map(|v| self.t(v.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            col = col.push(info_kv_center(self.t("root_step_provider"), &prov));
            col = col.push(info_kv_center(self.t("root_step_version"), &ver));
            if self.root.is_nightly() {
                let src = self
                    .root
                    .nightly_source
                    .map(|s| self.t(s.label_key()).to_string())
                    .unwrap_or_else(|| dash.clone());
                col = col.push(info_kv_center(self.t("root_step_source"), &src));
                if self.root.nightly_source == Some(NightlySource::ManualInput) {
                    let id = self.root.run_id.clone().unwrap_or_else(|| dash.clone());
                    col = col.push(info_kv_center(self.t("nightly_run_id_label"), &id));
                }
            }
        }

        if self.root.is_apatch() {
            // Count only — don't echo paths (noisy) or the superkey (secret).
            let kpm_summary = if self.root.kpm_paths.is_empty() {
                self.t("root_kpm_none").to_string()
            } else {
                self.t("root_kpm_count_tmpl")
                    .replace("{n}", &self.root.kpm_paths.len().to_string())
            };
            col = col.push(info_kv_center(self.t("root_step_kpm"), &kpm_summary));
        }

        let folder = self
            .root
            .folder_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        col = col.push(info_kv_center(self.t("root_step_folder"), &folder));

        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn root_flash_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }

    // -- Advanced (grid) --------------------------------------------------

    fn view_advanced(&self) -> Element<'_, Message> {
        // Dedicated wizards preempt the grid.
        if self.advanced_wizard_open.is_flash_parts() {
            return self.view_flash_parts_wizard();
        }
        if self.advanced_wizard_open.is_dump_parts() {
            return self.view_dump_parts_wizard();
        }
        if self.advanced_wizard_open.is_dump_phys() {
            return self.view_dump_phys_wizard();
        }
        if self.advanced_wizard_open.is_flash_phys() {
            return self.view_flash_phys_wizard();
        }
        if self.adv_wizard.action.is_some() {
            return self.view_adv_wizard();
        }

        let mut content = column![
            text(self.t("nav_advanced").to_string()).size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
        ]
        .spacing(14)
        .width(Length::Fill);

        for section in ADV_SECTIONS {
            content = content.push(
                text(self.t(section.title_key).to_string())
                    .size(11)
                    .style(label_style),
            );
            let mut rows = column![].spacing(8);
            for chunk in section.items.chunks(3) {
                let mut r = row![].spacing(8);
                for &item in chunk {
                    r = r.push(adv_grid_btn(item, self.t(item.label_key())));
                }
                for _ in chunk.len()..3 {
                    r = r.push(Space::new().width(Length::Fill));
                }
                rows = rows.push(r);
            }
            content = content.push(rows);
        }

        content.into()
    }

    // -- Advanced wizard (generic) ----------------------------------------

    /// Advanced wizard. PatchDevinfo: source/country/confirm/exec.
    /// Others: source/confirm/exec.
    fn view_adv_wizard(&self) -> Element<'_, Message> {
        let is_exec = self.adv_wizard.step == self.adv_wizard.exec_step();
        if self.log_popup_open && is_exec && !self.adv_wizard.is_image_info() {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = self.adv_wizard.steps().iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.adv_wizard.step);

        let needs_country = self.adv_wizard.needs_country();
        let needs_region_target = self.adv_wizard.needs_region_target();
        let is_confirm = self.adv_wizard.is_confirm_step();

        let detect_arb_step0 = matches!(self.adv_wizard.action, Some(AdvAction::DetectArb))
            && self.adv_wizard.step == 0;
        let body: Element<'_, Message> = if is_exec && self.adv_wizard.is_image_info() {
            self.adv_image_info_exec_step()
        } else if is_exec {
            self.exec_step_view()
        } else if detect_arb_step0 {
            self.adv_wiz_detect_arb_step()
        } else if is_confirm {
            self.adv_wiz_confirm_step()
        } else if needs_country && self.adv_wizard.step == 1 {
            self.adv_wiz_country_step()
        } else if needs_region_target && self.adv_wizard.step == 1 {
            self.adv_wiz_region_target_step()
        } else if matches!(self.adv_wizard.action, Some(AdvAction::PatchArb))
            && self.adv_wizard.step == 1
        {
            self.adv_wiz_arb_inspect_step()
        } else {
            self.adv_wiz_source_step()
        };

        let nav: Element<'_, Message> = if is_exec {
            container(text("")).into()
        } else {
            let label = if is_confirm || detect_arb_step0 {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            // DetectArb gates Start on either a picked loader (TB320FC
            // path) or no requirement at all (other models — Start is
            // always enabled). Other wizards keep the standard
            // `can_next` check.
            let can = if detect_arb_step0 {
                if self.device_model.eq_ignore_ascii_case("TB320FC") {
                    self.adv_wizard.file_path.is_some()
                } else {
                    true
                }
            } else {
                self.adv_wizard.can_next()
            } && !self.busy;
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                Message::Adv(AdvMsg::AdvWizBack),
                Message::Adv(AdvMsg::AdvWizNext),
            )
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Step 0 — Browse tile. Matches Flash/Root folder steps.
    fn adv_wiz_source_step(&self) -> Element<'_, Message> {
        let action = match self.adv_wizard.action {
            Some(a) => a,
            None => return container(text("")).into(),
        };
        let selected = if self.adv_wizard.is_image_info() {
            !self.adv_wizard.file_paths.is_empty()
        } else {
            self.adv_wizard.file_path.is_some()
        };
        let status = if self.adv_wizard.is_image_info() && selected {
            self.t("adv_image_info_selected_count")
                .replace("{count}", &self.adv_wizard.file_paths.len().to_string())
        } else {
            self.adv_wizard
                .file_path
                .clone()
                .unwrap_or_else(|| self.t("adv_source_placeholder").to_string())
        };
        let browse_key = if self.adv_wizard.is_image_info() {
            "btn_browse_files"
        } else if self.adv_wizard.is_folder_op() {
            "btn_browse_folder"
        } else {
            "btn_browse_file"
        };
        let btn = button(
            container(
                column![
                    text(self.t(browse_key).to_string()).size(14).center(),
                    text(self.t(action.source_desc_key()).to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizBrowse))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        // Shrink-wrap the 280 px card so the hit area stays tight.
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_color = if selected { GREEN } else { LABEL };
        let chips: Element<'_, Message> = if self.adv_wizard.is_image_info() {
            self.recent_file_chips(
                &["img"],
                |p| Message::Adv(AdvMsg::AdvWizBrowseManyDone(Some(vec![p]))),
                "picker_recents",
            )
        } else {
            let kind = self.adv_wizard.picker_kind();
            if kind.is_folder() {
                self.recent_chips(
                    self.recent_paths.recent(kind.storage_key()),
                    |p| Message::Adv(AdvMsg::AdvWizBrowseDone(Some(p))),
                    "picker_recents",
                    false,
                )
            } else {
                let (_, exts) = self.adv_wizard.accepted_exts();
                self.recent_file_chips(
                    exts,
                    |p| Message::Adv(AdvMsg::AdvWizBrowseDone(Some(p))),
                    "picker_recents",
                )
            }
        };
        let col = column![
            text(self.t(action.label_key()).to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn_row,
            text(status).size(12).color(status_color).center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Step 1 (PatchDevinfo only) — country picker tile; opens the
    /// shared country popup.
    fn adv_wiz_country_step(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.country.is_some();
        let status = self
            .adv_wizard
            .country
            .clone()
            .unwrap_or_else(|| self.t("adv_country_placeholder").to_string());
        let btn = button(
            container(
                column![
                    text(self.t("btn_pick_country").to_string())
                        .size(14)
                        .center(),
                    text(self.t("adv_country_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizOpenCountry))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_color = if selected { GREEN } else { LABEL };
        let col = column![
            text(self.t("adv_country_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_country_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            btn_row,
            text(status).size(12).color(status_color).center(),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Step 1 for `RegionConvert`: card that opens the target picker
    /// popup. Mirrors `adv_wiz_country_step` shape so the wizard
    /// rendering stays consistent with the other "needs option"
    /// flow (PatchDevinfo).
    fn adv_wiz_region_target_step(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.region_target.is_some();
        let status = match self.adv_wizard.region_target {
            Some(target) => self.t(target.label_key()).to_string(),
            None => self.t("adv_region_target_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_pick_region_target").to_string())
                        .size(14)
                        .center(),
                    text(self.t("adv_region_target_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizOpenRegionTarget))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_color = if selected { GREEN } else { LABEL };
        let col = column![
            text(self.t("adv_region_target_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_region_target_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            btn_row,
            text(status).size(12).color(status_color).center(),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// PatchArb inspect step — render boot.img + vbmeta_system.img
    /// rollback indices (decimal + UTC) read from the picked folder so
    /// the user can sanity-check the source before opening the
    /// timestamp popup. Next on this step opens the popup.
    fn adv_wiz_arb_inspect_step(&self) -> Element<'_, Message> {
        let (boot_idx, vbmeta_idx) = self.adv_wizard.arb_inspect.unwrap_or((0, 0));
        let mk_row = |label_key: &'static str, idx: u64| -> Element<'_, Message> {
            let utc = format_unix_timestamp_utc(idx);
            iced::widget::row![
                text(self.t(label_key).to_string())
                    .size(13)
                    .style(muted_style)
                    .width(220),
                text(idx.to_string()).size(13).width(140),
                text(utc).size(12).style(muted_style),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
            .into()
        };
        let col = column![
            text(self.t("adv_arb_inspect_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_arb_inspect_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            Space::new().height(8),
            mk_row("adv_arb_inspect_boot", boot_idx),
            mk_row("adv_arb_inspect_vbmeta", vbmeta_idx),
        ]
        .spacing(8)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// DetectArb step 0. TB320FC needs an EDL loader (the deeper
    /// path falls back to dumping `boot_a` + `vbmeta_system_a` when
    /// stored_rollback_index is missing, so a Firehose loader is
    /// required); other models just see a Start prompt because the
    /// detection runs entirely over fastboot vars.
    fn adv_wiz_detect_arb_step(&self) -> Element<'_, Message> {
        let needs_loader = self.device_model.eq_ignore_ascii_case("TB320FC");
        let title = text(self.t("adv_detect_arb").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE)
            .center();
        let subtitle_key = if needs_loader {
            "adv_src_detect_arb_loader"
        } else {
            "adv_src_detect_arb_start"
        };
        let subtitle = text(self.t(subtitle_key).to_string())
            .size(13)
            .style(muted_style)
            .center();
        let mut col = column![title, subtitle]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        if needs_loader {
            let selected = self.adv_wizard.file_path.is_some();
            let status = self
                .adv_wizard
                .file_path
                .clone()
                .unwrap_or_else(|| self.t("adv_source_placeholder").to_string());
            let btn = button(
                container(
                    column![
                        text(self.t("btn_browse_loader").to_string())
                            .size(14)
                            .center(),
                        text(self.t("dump_parts_loader_desc").to_string())
                            .size(11)
                            .style(muted_style)
                            .center(),
                    ]
                    .spacing(6)
                    .width(Length::Fixed(280.0))
                    .align_x(iced::Alignment::Center),
                )
                .padding([20, 24])
                .width(Length::Fixed(280.0))
                .style(move |t: &Theme| sel_card_style(t, selected)),
            )
            .width(Length::Shrink)
            .on_press(Message::Adv(AdvMsg::AdvWizBrowse))
            .padding(0)
            .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
            col = col.push(
                row![
                    Space::new().width(Length::Fill),
                    btn,
                    Space::new().width(Length::Fill),
                ]
                .align_y(iced::Alignment::Center),
            );
            let status_color = if selected { GREEN } else { LABEL };
            col = col.push(text(status).size(12).color(status_color).center());
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Confirm step — Next becomes Start.
    fn adv_wiz_confirm_step(&self) -> Element<'_, Message> {
        let action = match self.adv_wizard.action {
            Some(a) => a,
            None => return container(text("")).into(),
        };
        let dash = "—".to_string();
        let path = self
            .adv_wizard
            .file_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        let mut col = column![
            text(self.t(action.label_key()).to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t(action.desc_key()).to_string())
                .size(13)
                .style(muted_style)
                .center(),
            Space::new().height(12),
            info_kv_center(self.t("adv_confirm_source"), &path),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        if self.adv_wizard.needs_country() {
            let code = self.adv_wizard.country.clone().unwrap_or(dash.clone());
            col = col.push(info_kv_center(self.t("adv_confirm_country"), &code));
        }
        if self.adv_wizard.needs_region_target() {
            let label = self
                .adv_wizard
                .region_target
                .map(|r| self.t(r.label_key()).to_string())
                .unwrap_or(dash);
            col = col.push(info_kv_center(self.t("adv_confirm_region_target"), &label));
        }
        if matches!(self.adv_wizard.action, Some(AdvAction::PatchArb))
            && let Some(idx) = self.adv_wizard.arb_index_committed
        {
            let utc = format_unix_timestamp_utc(idx);
            col = col.push(info_kv_center(
                self.t("adv_confirm_arb_index"),
                &format!("{idx}  ({utc})"),
            ));
            if let Some((boot_idx, vbmeta_idx)) = self.adv_wizard.arb_inspect {
                col = col.push(info_kv_center(
                    self.t("adv_arb_inspect_boot"),
                    &format!("{boot_idx} → {idx}"),
                ));
                col = col.push(info_kv_center(
                    self.t("adv_arb_inspect_vbmeta"),
                    &format!("{vbmeta_idx} → {idx}"),
                ));
            }
        }
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn adv_image_info_exec_step(&self) -> Element<'_, Message> {
        let action_label = self
            .adv_wizard
            .action
            .map(|a| self.t(a.label_key()).to_string())
            .unwrap_or_else(|| self.t("adv_image_info").to_string());
        let status = if self.busy {
            self.t("exec_executing_title").to_string()
        } else if self.error_msg.is_some() {
            self.t("exec_failed_title").to_string()
        } else {
            self.t("exec_done_title").to_string()
        };
        let is_error = self.error_msg.is_some();
        let is_busy = self.busy;
        let status_color = move |t: &Theme| {
            let p = pal_of(t);
            let color = if is_error {
                p.error
            } else if is_busy {
                p.primary
            } else {
                p.success
            };
            iced::widget::text::Style { color: Some(color) }
        };

        let editor = iced::widget::text_editor(&self.image_info_log_editor)
            .on_action(Message::ImageInfoLogEditorAction)
            .size(11)
            .height(Length::Fill);

        let pill_style = neutral_pill_btn_style;
        let mut buttons = row![
            button(
                text(self.t("btn_save_log").to_string())
                    .size(11)
                    .style(muted_style)
                    .center(),
            )
            .on_press(Message::SaveLog)
            .padding([4, 12])
            .style(pill_style)
        ]
        .spacing(8);

        if !self.busy {
            buttons = buttons.push(
                button(
                    text(self.t("btn_start_over").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                )
                .on_press(Message::StartOver)
                .padding([4, 12])
                .style(pill_style),
            );
        }

        let header = row![
            column![
                text(action_label).size(theme::text_size::TITLE_LARGE),
                text(status).size(12).style(status_color),
            ]
            .spacing(4),
            Space::new().width(Length::Fill),
            buttons,
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let body = column![
            header,
            widget::rule::horizontal(1),
            container(editor)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(10)
                .style(|t: &Theme| theme::surface_card_style(
                    t,
                    theme::SurfaceLevel::Low,
                    theme::shape::SM,
                    0,
                )),
        ]
        .spacing(12)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill);

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Flash Partitions wizard ------------------------------------------

    fn view_flash_parts_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash_parts.step >= 3 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = FLASH_PARTS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash_parts.step);

        let body: Element<'_, Message> = match self.flash_parts.step {
            0 => self.flash_parts_loader_step(),
            1 => self.flash_parts_select_step(),
            2 => self.flash_parts_confirm_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.flash_parts.step < 3 {
            let label = match self.flash_parts.step {
                0 => self.t("btn_scan").to_string(),
                1 => self.t("btn_next").to_string(),
                2 => self.t("btn_start").to_string(),
                _ => self.t("btn_next").to_string(),
            };
            let is_start = self.flash_parts.step == 2 || self.flash_parts.step == 0;
            let can = self.flash_parts.can_next() && !(self.busy && is_start);
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.flash_parts.step == 0 {
                    Message::FlashParts(FlashPartsMsg::FlashPartsClose)
                } else {
                    Message::FlashParts(FlashPartsMsg::FlashPartsBack)
                },
                Message::FlashParts(FlashPartsMsg::FlashPartsNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_parts_loader_step(&self) -> Element<'_, Message> {
        let selected = self.flash_parts.loader_path.is_some();
        let status = match (&self.flash_parts.loader_path, &self.flash_parts.scan_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("dump_parts_loader_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::FlashParts(FlashPartsMsg::FlashPartsSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.flash_parts.scan_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::FlashParts(FlashPartsMsg::FlashPartsLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status).size(12).color(status_color).center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_parts_select_step(&self) -> Element<'_, Message> {
        let active = self.flash_parts.sort_col;
        let desc = self.flash_parts.sort_desc;
        let mk_msg = |c: PartsSortColumn| Message::FlashParts(FlashPartsMsg::FlashPartsSortBy(c));
        let header = row![
            text(" ").size(11).width(32), // checkbox col
            parts_sort_header(
                self.t("flash_parts_col_lun").to_string(),
                active == PartsSortColumn::Lun,
                desc,
                Length::Fixed(50.0),
                mk_msg(PartsSortColumn::Lun),
            ),
            parts_sort_header(
                self.t("flash_parts_col_label").to_string(),
                active == PartsSortColumn::Label,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::Label),
            ),
            parts_sort_header(
                self.t("flash_parts_col_start").to_string(),
                active == PartsSortColumn::Start,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Start),
            ),
            parts_sort_header(
                self.t("dump_parts_col_size").to_string(),
                active == PartsSortColumn::Size,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Size),
            ),
            parts_sort_header(
                self.t("flash_parts_col_file").to_string(),
                active == PartsSortColumn::File,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::File),
            ),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for (idx, r) in self.flash_parts.rows.iter().enumerate() {
            // Tri-state indicator. Unchecked uses a real iced checkbox so
            // its empty box matches the M3 sizing the user already sees on
            // DumpParts; Flash overlays the checkbox at `is_checked=true`;
            // Erase swaps to a red ⛔ so the destructive state pops.
            // All glyphs render through a fixed 20-px container so the
            // marker column never shifts vertically across state changes.
            let marker: Element<'_, Message> = match r.state {
                FlashRowState::Unchecked => iced::widget::checkbox(false)
                    .on_toggle(move |_| {
                        Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx))
                    })
                    .into(),
                FlashRowState::Flash => iced::widget::checkbox(true)
                    .on_toggle(move |_| {
                        Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx))
                    })
                    .into(),
                FlashRowState::Erase => text("⛔")
                    .size(18)
                    .color(iced::Color::from_rgb(0.9, 0.2, 0.2))
                    .into(),
            };
            let marker_btn = button(
                container(marker)
                    .width(32)
                    .height(20)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .padding(0)
            .on_press(Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx)))
            .style(|_t: &Theme, _s| button::Style {
                background: None,
                ..Default::default()
            });

            // Filename column: short display only.
            let file_disp = r
                .file_path
                .as_ref()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                })
                .unwrap_or_default();

            let data_row = iced::widget::row![
                container(marker_btn).width(32),
                text(r.lun.to_string()).size(12).width(50),
                text(r.label.clone()).size(12).width(Length::FillPortion(3)),
                text(r.start_sector.to_string())
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(format_bytes_auto(r.size_bytes))
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(file_disp).size(12).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);

            // Tint the whole row by its tri-state so flash/erase pop
            // visually; light/dark both pull from the M3 container roles.
            let row_state = r.state;
            let tinted = container(data_row).width(Length::Fill).style(
                move |t: &Theme| -> container::Style {
                    let p = pal_of(t);
                    let bg = match row_state {
                        FlashRowState::Flash => Some(p.primary_container),
                        FlashRowState::Erase => Some(p.error_container),
                        FlashRowState::Unchecked => None,
                    };
                    container::Style {
                        background: bg.map(iced::Background::Color),
                        ..Default::default()
                    }
                },
            );

            // Whole row is a double-click target for the file picker.
            let clickable = iced::widget::mouse_area(tinted).on_double_click(Message::FlashParts(
                FlashPartsMsg::FlashPartsPickRowFile(idx),
            ));
            list = list.push(clickable);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("flash_parts_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_parts_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_parts_confirm_step(&self) -> Element<'_, Message> {
        let rows = self.flash_parts.active_rows();
        let erase_rows: Vec<&FlashPartRow> = rows
            .iter()
            .filter(|r| r.state == FlashRowState::Erase)
            .collect();
        let flash_rows: Vec<&FlashPartRow> = rows
            .iter()
            .filter(|r| r.state == FlashRowState::Flash)
            .collect();

        let mut col = column![
            text(self.t("flash_parts_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_parts_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        // ERASE block first, red and loud.
        if !erase_rows.is_empty() {
            let red = iced::Color::from_rgb(0.9, 0.2, 0.2);
            let mut erase_col = column![
                text(self.t("flash_parts_confirm_erase_warn").to_string())
                    .size(14)
                    .color(red)
            ]
            .spacing(4);
            for r in &erase_rows {
                erase_col = erase_col.push(
                    text(format!(
                        "⛔ {} (LUN {}, {})",
                        r.label,
                        r.lun,
                        format_bytes_auto(r.size_bytes)
                    ))
                    .size(13)
                    .color(red),
                );
            }
            let erase_card =
                container(erase_col)
                    .padding(14)
                    .style(move |t: &Theme| container::Style {
                        background: Some(iced::Background::Color(with_alpha(red, 0.12))),
                        border: iced::Border {
                            color: red,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        text_color: Some(pal_of(t).on_surface),
                        ..Default::default()
                    });
            col = col.push(erase_card);
        }

        // FLASH block.
        if !flash_rows.is_empty() {
            let mut flash_col = column![
                text(self.t("flash_parts_confirm_flash_hdr").to_string())
                    .size(14)
                    .style(on_surface_style)
            ]
            .spacing(4);
            for r in &flash_rows {
                let fname = r
                    .file_path
                    .as_ref()
                    .map(|p| {
                        std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| p.clone())
                    })
                    .unwrap_or_default();
                flash_col = flash_col.push(
                    text(format!("• {} (LUN {}) ← {}", r.label, r.lun, fname))
                        .size(12)
                        .style(muted_style),
                );
            }
            col = col.push(container(flash_col).padding(14).width(Length::Fill));
        }

        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Dump Partitions wizard ------------------------------------------

    fn view_dump_parts_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.dump_parts.step >= 2 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = DUMP_PARTS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.dump_parts.step);

        let body: Element<'_, Message> = match self.dump_parts.step {
            0 => self.dump_parts_loader_step(),
            1 => self.dump_parts_select_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.dump_parts.step < 2 {
            let is_dump_step = self.dump_parts.step == 1;
            let label = if is_dump_step {
                self.t("btn_dump").to_string()
            } else {
                self.t("btn_scan").to_string()
            };
            let can = self.dump_parts.can_next() && !self.busy;
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.dump_parts.step == 0 {
                    Message::DumpParts(DumpPartsMsg::DumpPartsClose)
                } else {
                    Message::DumpParts(DumpPartsMsg::DumpPartsBack)
                },
                Message::DumpParts(DumpPartsMsg::DumpPartsNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn dump_parts_loader_step(&self) -> Element<'_, Message> {
        let selected = self.dump_parts.loader_path.is_some();
        let status = match (&self.dump_parts.loader_path, &self.dump_parts.scan_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("dump_parts_loader_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::DumpParts(DumpPartsMsg::DumpPartsSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.dump_parts.scan_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::DumpParts(DumpPartsMsg::DumpPartsLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status).size(12).color(status_color).center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn dump_parts_select_step(&self) -> Element<'_, Message> {
        let active = self.dump_parts.sort_col;
        let desc = self.dump_parts.sort_desc;
        let mk_msg = |c: PartsSortColumn| Message::DumpParts(DumpPartsMsg::DumpPartsSortBy(c));
        // Header select-all: checked iff every row is selected (and there
        // is at least one row). Click flips toward whichever direction
        // would change state for the majority — full-select if any are
        // unchecked, else clear.
        let all_checked =
            !self.dump_parts.rows.is_empty() && self.dump_parts.rows.iter().all(|r| r.selected);
        let header_cb = iced::widget::checkbox(all_checked)
            .on_toggle(|_| Message::DumpParts(DumpPartsMsg::DumpPartsToggleAll));
        let header = row![
            container(header_cb).width(32),
            parts_sort_header(
                self.t("flash_parts_col_lun").to_string(),
                active == PartsSortColumn::Lun,
                desc,
                Length::Fixed(50.0),
                mk_msg(PartsSortColumn::Lun),
            ),
            parts_sort_header(
                self.t("flash_parts_col_label").to_string(),
                active == PartsSortColumn::Label,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::Label),
            ),
            parts_sort_header(
                self.t("flash_parts_col_start").to_string(),
                active == PartsSortColumn::Start,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Start),
            ),
            parts_sort_header(
                self.t("dump_parts_col_size").to_string(),
                active == PartsSortColumn::Size,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Size),
            ),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for (idx, row) in self.dump_parts.rows.iter().enumerate() {
            let cb = iced::widget::checkbox(row.selected)
                .on_toggle(move |_| Message::DumpParts(DumpPartsMsg::DumpPartsToggleRow(idx)));
            let data_row = iced::widget::row![
                container(cb).width(32),
                text(row.lun.to_string()).size(12).width(50),
                text(row.label.clone())
                    .size(12)
                    .width(Length::FillPortion(3)),
                text(row.start_sector.to_string())
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(format_bytes_auto(row.size_bytes))
                    .size(12)
                    .width(Length::FillPortion(2)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);
            // Tint selected rows so the dump set is visible at a glance.
            let selected = row.selected;
            let tinted = container(data_row).width(Length::Fill).style(
                move |t: &Theme| -> container::Style {
                    let p = pal_of(t);
                    container::Style {
                        background: if selected {
                            Some(iced::Background::Color(p.primary_container))
                        } else {
                            None
                        },
                        ..Default::default()
                    }
                },
            );
            list = list.push(tinted);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("dump_parts_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("dump_parts_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Physical Storage: Dump wizard -----------------------------------

    fn view_dump_phys_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.dump_phys.step >= 2 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = DUMP_PHYS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.dump_phys.step);

        let body: Element<'_, Message> = match self.dump_phys.step {
            0 => self.dump_phys_loader_step(),
            1 => self.dump_phys_select_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.dump_phys.step < 2 {
            let is_dump_step = self.dump_phys.step == 1;
            let label = if is_dump_step {
                self.t("btn_dump").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.dump_phys.can_next() && !self.busy;
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.dump_phys.step == 0 {
                    Message::DumpPhys(DumpPhysMsg::DumpPhysClose)
                } else {
                    Message::DumpPhys(DumpPhysMsg::DumpPhysBack)
                },
                Message::DumpPhys(DumpPhysMsg::DumpPhysNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn dump_phys_loader_step(&self) -> Element<'_, Message> {
        let selected = self.dump_phys.loader_path.is_some();
        let status = match (&self.dump_phys.loader_path, &self.dump_phys.loader_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("dump_parts_loader_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::DumpPhys(DumpPhysMsg::DumpPhysSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.dump_phys.loader_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::DumpPhys(DumpPhysMsg::DumpPhysLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status).size(12).color(status_color).center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn dump_phys_select_step(&self) -> Element<'_, Message> {
        let header = row![
            text(" ").size(11).width(32),
            text(self.t("phys_col_storage").to_string())
                .size(11)
                .width(Length::Fill)
                .style(muted_style),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for idx in 0..PHYS_LUN_COUNT {
            let checked = self.dump_phys.selected[idx];
            let cb = iced::widget::checkbox(checked)
                .on_toggle(move |_| Message::DumpPhys(DumpPhysMsg::DumpPhysToggleRow(idx)));
            let data_row = iced::widget::row![
                container(cb).width(32),
                text(format!("LUN {idx}")).size(12).width(Length::Fill),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);
            list = list.push(data_row);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("phys_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("phys_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Physical Storage: Flash wizard ----------------------------------

    fn view_flash_phys_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash_phys.step >= 3 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = FLASH_PHYS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash_phys.step);

        let body: Element<'_, Message> = match self.flash_phys.step {
            0 => self.flash_phys_loader_step(),
            1 => self.flash_phys_select_step(),
            2 => self.flash_phys_confirm_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.flash_phys.step < 3 {
            let label = match self.flash_phys.step {
                0 => self.t("btn_next").to_string(),
                1 => self.t("btn_next").to_string(),
                2 => self.t("btn_start").to_string(),
                _ => self.t("btn_next").to_string(),
            };
            let is_start = self.flash_phys.step == 2;
            let can = self.flash_phys.can_next() && !(self.busy && is_start);
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.flash_phys.step == 0 {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysClose)
                } else {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysBack)
                },
                Message::FlashPhys(FlashPhysMsg::FlashPhysNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_phys_loader_step(&self) -> Element<'_, Message> {
        let selected = self.flash_phys.loader_path.is_some();
        let status = match (&self.flash_phys.loader_path, &self.flash_phys.loader_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.t("dump_parts_loader_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::FlashPhys(FlashPhysMsg::FlashPhysSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.flash_phys.loader_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf"],
            |p| Message::FlashPhys(FlashPhysMsg::FlashPhysLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status).size(12).color(status_color).center(),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    fn flash_phys_select_step(&self) -> Element<'_, Message> {
        let header = row![
            text(" ").size(11).width(32),
            text(self.t("phys_col_storage").to_string())
                .size(11)
                .width(Length::FillPortion(2))
                .style(muted_style),
            text(self.t("flash_parts_col_file").to_string())
                .size(11)
                .width(Length::FillPortion(3))
                .style(muted_style),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for idx in 0..PHYS_LUN_COUNT {
            let checked = self.flash_phys.selected[idx];
            let cb = iced::widget::checkbox(checked)
                .on_toggle(move |_| Message::FlashPhys(FlashPhysMsg::FlashPhysToggleRow(idx)));

            let file_disp = self.flash_phys.file_paths[idx]
                .as_ref()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                })
                .unwrap_or_default();

            let data_row = iced::widget::row![
                container(cb).width(32),
                text(format!("LUN {idx}"))
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(file_disp).size(12).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);

            let clickable = iced::widget::mouse_area(data_row)
                .on_double_click(Message::FlashPhys(FlashPhysMsg::FlashPhysPickRowFile(idx)));
            list = list.push(clickable);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("phys_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_phys_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn flash_phys_confirm_step(&self) -> Element<'_, Message> {
        let pairs = self.flash_phys.active_pairs();

        let mut col = column![
            text(self.t("flash_parts_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_phys_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        if !pairs.is_empty() {
            let mut list = column![
                text(self.t("flash_parts_confirm_flash_hdr").to_string())
                    .size(14)
                    .style(on_surface_style)
            ]
            .spacing(4);
            for (lun, path) in &pairs {
                let fname = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                list = list.push(
                    text(format!("• LUN {lun} ← {fname}"))
                        .size(12)
                        .style(muted_style),
                );
            }
            col = col.push(container(list).padding(14).width(Length::Fill));
        }

        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    // -- Reboot panel -----------------------------------------------------

    fn view_reboot(&self) -> Element<'_, Message> {
        let conn = self.connection;
        let conn_label = self.t(conn.label_key()).to_string();
        // 1 col × N rows — each target splits the vertical space.
        // Disabled cards: M3 tokens (12% surface alpha, 38% text alpha).
        let mut list = column![]
            .spacing(10)
            .width(Length::Fill)
            .height(Length::Fill);
        for &target in RebootTarget::all().iter() {
            let available = target.available_from(conn);
            let label = self.t(target.label_key()).to_string();
            let desc = self.t(target.desc_key()).to_string();
            let label_style = if available {
                on_surface_style
            } else {
                |t: &Theme| iced::widget::text::Style {
                    color: Some(with_alpha(pal_of(t).on_surface, 0.38)),
                }
            };
            let desc_style = if available {
                muted_style
            } else {
                |t: &Theme| iced::widget::text::Style {
                    color: Some(with_alpha(pal_of(t).on_surface, 0.38)),
                }
            };
            // Empty desc → centred single label; non-empty keeps the stack.
            let label_col: Element<'_, Message> = if desc.is_empty() {
                container(text(label).size(18).style(label_style))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(iced::alignment::Vertical::Center)
                    .into()
            } else {
                column![
                    text(label).size(18).style(label_style),
                    text(desc).size(12).style(desc_style),
                ]
                .spacing(6)
                .width(Length::Fill)
                .into()
            };
            let card_content = row![icon_tile(target.icon()), label_col]
                .spacing(16)
                .align_y(iced::Alignment::Center);
            let card_inner = container(card_content)
                .padding([20, 24])
                .width(Length::Fill)
                .height(Length::Fill)
                .center_y(Length::Fill)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    if available {
                        sel_card_style(t, false)
                    } else {
                        container::Style {
                            background: Some(with_alpha(p.on_surface, 0.12).into()),
                            border: iced::Border {
                                color: iced::Color::TRANSPARENT,
                                width: 0.0,
                                radius: theme::shape::MD.into(),
                            },
                            ..Default::default()
                        }
                    }
                });
            let btn: Element<'_, Message> = if available {
                button(card_inner)
                    .on_press(Message::Reboot(RebootMsg::RebootRequest(target)))
                    .padding(0)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|t: &Theme, status| sel_card_btn_style(t, status, false))
                    .into()
            } else {
                card_inner.into()
            };
            list = list.push(btn);
        }

        let header = text(self.t("reboot_title").to_string()).size(theme::text_size::TITLE_LARGE);
        let subtitle = text(self.t("reboot_subtitle").replace("{conn}", &conn_label))
            .size(13)
            .style(muted_style);
        column![header, subtitle, widget::rule::horizontal(1), list,]
            .spacing(14)
            .padding(0)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// M3 confirm dialog for the Reboot panel.
    fn reboot_confirm_popup(&self, target: RebootTarget) -> Element<'_, Message> {
        let short = self.t(target.short_name_key()).to_string();
        let title = self.t("reboot_confirm_title").replace("{target}", &short);
        let body = self.t("reboot_confirm_body").replace("{target}", &short);
        let content = column![
            text(title).size(20),
            text(body).size(13).style(muted_style),
            widget::rule::horizontal(1),
            row![
                Space::new().width(Length::Fill),
                button(
                    text(self.t("btn_cancel").to_string())
                        .size(13)
                        .style(muted_style)
                )
                .on_press(Message::Reboot(RebootMsg::RebootDismiss))
                .padding([8, 18])
                .style(|t: &Theme, _s| {
                    let p = pal_of(t);
                    button::Style {
                        background: Some(with_alpha(p.on_surface, 0.06).into()),
                        border: iced::Border {
                            radius: 20.0.into(),
                            ..Default::default()
                        },
                        text_color: p.on_surface_variant,
                        ..Default::default()
                    }
                }),
                button(text(self.t("btn_reboot_confirm").to_string()).size(13))
                    .on_press(Message::Reboot(RebootMsg::RebootConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);
        m3_dialog(content.into())
    }

    // -- Placeholder ------------------------------------------------------

    fn view_placeholder(&self) -> Element<'_, Message> {
        column![
            text(self.t(self.current_view.label_key()).to_string())
                .size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
            container(text("").size(14).style(muted_style))
                .padding(48)
                .width(Length::Fill)
                .center_x(Length::Fill),
        ]
        .spacing(14)
        .width(Length::Fill)
        .into()
    }
}

// =========================================================================
// Wizard step bar
// =========================================================================

fn wizard_step_bar<'a>(steps: &[&str], current: usize) -> Element<'a, Message> {
    let mut r = row![]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .padding([14, 24]);

    for (i, &label) in steps.iter().enumerate() {
        if i > 0 {
            let completed = i <= current;
            r = r.push(container(text("")).width(Length::Fill).height(2).style(
                move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if completed {
                        p.success
                    } else {
                        p.outline_variant
                    };
                    container::Style {
                        background: Some(color.into()),
                        ..Default::default()
                    }
                },
            ));
        }

        let done = i < current;
        let active = i == current;
        let dot_text = if done {
            "\u{2713}".to_string()
        } else {
            (i + 1).to_string()
        };

        let dot = container(text(dot_text).size(12).center().style(move |t: &Theme| {
            let p = pal_of(t);
            let fg = if done || active {
                iced::Color::WHITE
            } else {
                p.on_surface_variant
            };
            iced::widget::text::Style { color: Some(fg) }
        }))
        .width(28)
        .height(28)
        .center_x(28)
        .center_y(28)
        .style(move |t: &Theme| {
            let p = pal_of(t);
            let bg = if done {
                p.success
            } else if active {
                p.primary
            } else {
                p.surface_container_high
            };
            let border_color = if done || active {
                bg
            } else {
                p.outline_variant
            };
            container::Style {
                background: Some(bg.into()),
                border: iced::Border {
                    color: border_color,
                    width: 1.0,
                    radius: 14.0.into(),
                },
                ..Default::default()
            }
        });

        let lbl_widget = text(label.to_string()).size(12).style(move |t: &Theme| {
            let p = pal_of(t);
            let color = if done {
                p.success
            } else if active {
                p.primary
            } else {
                p.on_surface_variant
            };
            iced::widget::text::Style { color: Some(color) }
        });
        r = r.push(
            row![dot, lbl_widget]
                .spacing(6)
                .align_y(iced::Alignment::Center),
        );
    }

    // Bottom-edge divider only — top + sides come from the
    // surrounding window outline / sidebar rule, so a 4-side
    // border here would render the bottom + left as a double line.
    column![
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
        widget::rule::horizontal(1).style(shell_rule_style),
    ]
    .into()
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

/// M3 filled button — primary bg + state-layer overlay on hover/press.
fn md_filled_btn_style(t: &Theme, status: button::Status) -> button::Style {
    let p = pal_of(t);
    let state_alpha = match status {
        button::Status::Hovered => theme::state::HOVER,
        button::Status::Pressed => theme::state::PRESSED,
        _ => 0.0,
    };
    let bg = blend(p.primary, p.on_primary, state_alpha);
    button::Style {
        background: Some(bg.into()),
        text_color: p.on_primary,
        border: iced::Border {
            radius: theme::shape::FULL.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// M3 text button — no fill, state layer on hover/press.
fn md_text_btn_style(t: &Theme, status: button::Status) -> button::Style {
    let p = pal_of(t);
    let bg_alpha = match status {
        button::Status::Hovered => theme::state::HOVER,
        button::Status::Pressed => theme::state::PRESSED,
        _ => 0.0,
    };
    button::Style {
        background: if bg_alpha > 0.0 {
            Some(with_alpha(p.primary, bg_alpha).into())
        } else {
            None
        },
        text_color: p.primary,
        border: iced::Border {
            radius: theme::shape::FULL.into(),
            ..Default::default()
        },
        ..Default::default()
    }
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

fn sec_hdr<'a>(label: &str, expanded: bool) -> Element<'a, Message> {
    if !expanded {
        return container(text(""))
            .height(Length::Fixed(SEC_HDR_HEIGHT))
            .into();
    }
    let owned = label.to_string();
    container(
        text(owned)
            .size(theme::text_size::LABEL_SMALL)
            .style(|t: &Theme| iced::widget::text::Style {
                color: Some(pal_of(t).on_surface_variant),
            }),
    )
    .padding([10, 22])
    .height(Length::Fixed(SEC_HDR_HEIGHT))
    .into()
}

/// Pinned nav button height — matches the expanded label form so
/// the sidebar tween's mid-frame swap between icon-only and
/// label content doesn't push every row vertically.
const NAV_BTN_HEIGHT: f32 = 38.0;

fn nav_btn<'a>(
    view: View,
    label: &str,
    active: bool,
    enabled: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let icon = lucide_icon(view.nav_icon(), 18.0, move |t: &Theme| {
        let p = pal_of(t);
        if !enabled {
            with_alpha(p.on_surface, 0.38)
        } else if active {
            p.primary
        } else {
            p.on_surface_variant
        }
    });

    // Single base layout in both modes: icon left-anchored + optional
    // label. Keeping the icon's horizontal position constant across
    // modes means it does not jump from "centered in 64 px shell"
    // to "left-padded next to label" the moment the label mounts.
    // Identical `padding([0, 22])` on the outer button makes the
    // icon's left margin (22 px) and the collapsed shell's natural
    // icon center (≈ 23 px) read as the same x position to the eye.
    let mut inner = iced::widget::row![icon]
        .spacing(12)
        .align_y(iced::Alignment::Center);
    if expanded {
        inner = inner.push(
            text(label.to_string())
                .size(13)
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center),
        );
    }
    let content: Element<'a, Message> = container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(iced::Alignment::Center)
        .into();

    // Horizontal padding stays at 22 in both modes so the icon
    // doesn't slide horizontally as the sidebar tween crosses its
    // midpoint. Vertical padding was already symmetrical.
    let btn = button(content)
        .padding([0, 22])
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
            if active {
                button::Style {
                    background: Some(with_alpha(p.primary, 0.14).into()),
                    text_color: p.primary,
                    ..Default::default()
                }
            } else {
                match status {
                    button::Status::Hovered => button::Style {
                        background: Some(with_alpha(p.on_surface, theme::state::HOVER).into()),
                        text_color: p.on_surface,
                        ..Default::default()
                    },
                    _ => button::Style {
                        background: None,
                        text_color: p.on_surface_variant,
                        ..Default::default()
                    },
                }
            }
        });
    if enabled {
        btn.on_press(Message::Navigate(view)).into()
    } else {
        btn.into()
    }
}

fn card<'a>(title: &str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(
        column![
            text(title.to_string())
                .size(13)
                .style(label_style)
                .line_height(1.0),
            content.into(),
        ]
        .spacing(6)
        .padding(iced::Padding {
            top: 10.0,
            right: 18.0,
            bottom: 14.0,
            left: 18.0,
        })
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .style(|t: &Theme| {
        theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 1)
    })
    .into()
}

fn info_kv<'a>(label: &str, value: &str) -> Element<'a, Message> {
    column![
        text(label.to_string()).size(11).style(label_style),
        text(value.to_string()).size(14),
    ]
    .spacing(3)
    .into()
}

fn info_kv_center<'a>(label: &str, value: &str) -> Element<'a, Message> {
    column![
        text(label.to_string())
            .size(11)
            .style(label_style)
            .width(Length::Fill)
            .center(),
        text(value.to_string())
            .size(14)
            .width(Length::Fill)
            .center(),
    ]
    .spacing(3)
    .width(Length::Fill)
    .align_x(iced::Alignment::Center)
    .into()
}

fn adv_grid_btn<'a>(item: AdvAction, label: &str) -> Element<'a, Message> {
    // Inner container: border-only via `sel_card_style`. Earlier
    // version used `theme::surface_card_style` which paints an opaque
    // bg — that bg sat on top of the button's hover fill, swallowing
    // the highlight and making the grid feel dead on hover.
    let content = container(
        text(label.to_string())
            .size(12)
            .center()
            .width(Length::Fill)
            .style(|t: &Theme| iced::widget::text::Style {
                color: Some(pal_of(t).on_surface),
            }),
    )
    .padding([18, 12])
    .width(Length::Fill)
    .center_x(Length::Fill)
    .style(|t: &Theme| sel_card_style(t, false));

    button(content)
        .on_press(Message::Adv(AdvMsg::AdvConfirm(item)))
        .padding(0)
        .width(Length::Fill)
        .style(|t: &Theme, status| sel_card_btn_style(t, status, false))
        .into()
}

/// Advanced → Flash Partitions end-to-end. Routes to EDL, opens an
/// `EdlSession`, flashes each selected image, resets.
/// Scan phase mirror of `dump_parts_scan`. Transitions to EDL, opens
/// Sahara, reads GPTs on LUN 0..=5, then bounces back to EDL so the
/// exec pass can reopen without a power-cycle.
fn flash_parts_scan(conn: ConnectionStatus, loader_path: String) -> FlashPartsScanResult {
    let mut log = Vec::new();
    if ensure_edl(conn, "FlashParts", &mut log).is_err() {
        return FlashPartsScanResult {
            logs: log,
            rows: Vec::new(),
            error: Some("Could not transition device to EDL".to_string()),
        };
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashParts] {}",
                ltbox_core::i18n::tr("live_flashparts_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return FlashPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(format!("EDL session open failed: {e}")),
            };
        }
    };

    let parts = match session.scan_partitions(0..=5, &mut log) {
        Ok(p) => p,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashParts] {}",
                ltbox_core::i18n::tr("live_flashparts_scan_failed")
                    .replace("{error}", &e.to_string())
            );
            let _ = session.reset_to_edl(&mut log);
            return FlashPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(format!("scan failed: {e}")),
            };
        }
    };

    let rows: Vec<FlashPartRow> = parts
        .into_iter()
        .map(|p| FlashPartRow {
            lun: p.lun,
            label: p.name,
            start_sector: p.start_sector,
            num_sectors: p.num_sectors,
            size_bytes: p.size_bytes,
            file_path: None,
            state: FlashRowState::Unchecked,
        })
        .collect();

    if let Err(e) = session.reset_to_edl(&mut log) {
        ltbox_core::live!(
            log,
            "[FlashParts] {}",
            ltbox_core::i18n::tr("live_flashparts_reset_failed").replace("{error}", &e.to_string())
        );
    }

    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_scan_complete")
            .replace("{count}", &rows.len().to_string())
    );
    FlashPartsScanResult {
        logs: log,
        rows,
        error: None,
    }
}

/// Exec phase. Reopens the EDL session, walks the active rows, flashing
/// or erasing each, then reboots to system.
fn flash_parts_execute(loader_path: String, rows: Vec<FlashPartRow>) -> Vec<String> {
    let mut log = Vec::new();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashParts] {}",
                ltbox_core::i18n::tr("live_flashparts_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return log;
        }
    };

    for row in &rows {
        match row.state {
            FlashRowState::Flash => {
                let Some(path) = row.file_path.as_ref() else {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        ltbox_core::i18n::tr("live_flashparts_skipping")
                            .replace("{label}", &row.label)
                    );
                    continue;
                };
                let img = std::path::Path::new(path);
                if !img.exists() {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        ltbox_core::i18n::tr("live_flashparts_skipping_missing")
                            .replace("{label}", &row.label)
                            .replace("{path}", path)
                    );
                    continue;
                }
                let file_name = img
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                ltbox_core::live!(
                    log,
                    "[FlashParts] {}",
                    ltbox_core::i18n::tr("live_flashparts_flashing")
                        .replace("{label}", &row.label)
                        .replace("{file}", &file_name)
                        .replace("{lun}", &row.lun.to_string())
                );
                if let Err(e) = session.flash_partition_at(
                    &row.label,
                    img,
                    row.lun,
                    &row.start_sector.to_string(),
                    &mut log,
                ) {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        ltbox_core::i18n::tr("live_flashparts_part_failed")
                            .replace("{label}", &row.label)
                            .replace("{error}", &e.to_string())
                    );
                }
            }
            FlashRowState::Erase => {
                ltbox_core::live!(
                    log,
                    "[FlashParts] {}",
                    ltbox_core::i18n::tr("live_flashparts_erasing")
                        .replace("{label}", &row.label)
                        .replace("{lun}", &row.lun.to_string())
                        .replace("{sectors}", &row.num_sectors.to_string())
                );
                if let Err(e) = session.erase_partition_at(
                    &row.label,
                    row.lun,
                    &row.start_sector.to_string(),
                    row.num_sectors as usize,
                    &mut log,
                ) {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        ltbox_core::i18n::tr("live_flashparts_erase_failed")
                            .replace("{label}", &row.label)
                            .replace("{error}", &e.to_string())
                    );
                }
            }
            FlashRowState::Unchecked => {}
        }
    }

    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        ltbox_core::i18n::tr("live_flashparts_resetting")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        ltbox_core::i18n::tr("live_flashparts_done")
    );
    log
}

/// Transition the device to EDL from whatever state it is in. Returns
/// `Ok(())` if the device is already in EDL or was sent there.
/// Shared by `dump_parts_scan`. Mirrors the inline block in
/// `flash_parts_execute`.
fn wait_for_edl_ready(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_wait_edl_port")
    );
    match ltbox_device::edl::wait_for_device() {
        Ok(_) => {
            ltbox_core::live!(log, "[{tag}] {}", ltbox_core::i18n::tr("live_edl_ready"));
            Ok(())
        }
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_edl_not_found").replace("{error}", &e.to_string())
            );
            Err(())
        }
    }
}

fn wait_for_manual_edl(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_manual_reboot_edl_wait")
    );
    wait_for_edl_ready(tag, log)
}

fn reboot_adb_to_edl(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    // Command echo (`adb reboot edl`) suppressed — the user only sees the
    // outcome (waiting / reached EDL / failure).
    let mut mgr = ltbox_device::adb::AdbManager::new();
    // `AdbManager::reboot` requires a preselected serial. Since
    // `check_device` now accepts only `Device` state, use
    // `check_device_state` here so recovery-state ADB can also
    // seed the serial before issuing `reboot edl`.
    let state = match mgr.check_device_state() {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_adb_state_probe_failed")
                    .replace("{error}", &e.to_string())
            );
            return wait_for_manual_edl(tag, log);
        }
    };
    match state {
        Some("device") | Some("recovery") => {}
        Some(other) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_adb_state_cannot_reboot_edl").replace("{state}", other)
            );
            return wait_for_manual_edl(tag, log);
        }
        None => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_no_adb_device_found")
            );
            return wait_for_manual_edl(tag, log);
        }
    }
    match mgr.reboot("edl") {
        Ok(_) => wait_for_edl_ready(tag, log),
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_adb_reboot_edl_failed")
                    .replace("{error}", &e.to_string())
            );
            wait_for_manual_edl(tag, log)
        }
    }
}

fn fastboot_continue_then_adb_edl(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    // Command echo (`fastboot continue`) suppressed — failure path still
    // logs the precise error.
    match ltbox_device::fastboot::FastbootDevice::open() {
        Ok(mut dev) => {
            if let Err(e) = dev.continue_boot() {
                ltbox_core::live!(
                    log,
                    "[{tag}] {}",
                    ltbox_core::i18n::tr("live_fastboot_continue_failed")
                        .replace("{error}", &e.to_string())
                );
                return wait_for_manual_edl(tag, log);
            }
        }
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_fastboot_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return wait_for_manual_edl(tag, log);
        }
    }

    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_adb_wait_after_fastboot")
    );
    let mut mgr = ltbox_device::adb::AdbManager::new();
    if let Err(e) = mgr.wait_for_device() {
        ltbox_core::live!(
            log,
            "[{tag}] {}",
            ltbox_core::i18n::tr("live_adb_wait_after_fastboot_failed")
                .replace("{error}", &e.to_string())
        );
        return wait_for_manual_edl(tag, log);
    }
    reboot_adb_to_edl(tag, log)
}

fn ensure_edl(conn: ConnectionStatus, tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    match edl_entry_action(conn) {
        EdlEntryAction::AlreadyEdl => {
            ltbox_core::live!(log, "[{tag}] {}", ltbox_core::i18n::tr("live_edl_already"));
            Ok(())
        }
        EdlEntryAction::AdbReboot => reboot_adb_to_edl(tag, log),
        EdlEntryAction::FastbootContinueThenAdb => fastboot_continue_then_adb_edl(tag, log),
        EdlEntryAction::ManualWait => wait_for_manual_edl(tag, log),
    }
}

/// Scan GPTs on LUNs 0..=5 using the picked loader. Leaves the device
/// in EDL (bounces through `reset_to_edl`) so the dump pass can re-open
/// Sahara without a power-cycle.
fn dump_parts_scan(conn: ConnectionStatus, loader_path: String) -> DumpPartsScanResult {
    let mut log = Vec::new();
    if ensure_edl(conn, "DumpParts", &mut log).is_err() {
        return DumpPartsScanResult {
            logs: log,
            rows: Vec::new(),
            error: Some("Could not transition device to EDL".to_string()),
        };
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                ltbox_core::i18n::tr("live_dumpparts_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return DumpPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(format!("EDL session open failed: {e}")),
            };
        }
    };

    let parts = match session.scan_partitions(0..=5, &mut log) {
        Ok(p) => p,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                ltbox_core::i18n::tr("live_dumpparts_scan_failed")
                    .replace("{error}", &e.to_string())
            );
            let _ = session.reset_to_edl(&mut log);
            return DumpPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(format!("scan failed: {e}")),
            };
        }
    };

    let rows: Vec<DumpPartRow> = parts
        .into_iter()
        .map(|p| DumpPartRow {
            lun: p.lun,
            label: p.name,
            start_sector: p.start_sector,
            num_sectors: p.num_sectors,
            size_bytes: p.size_bytes,
            selected: false,
        })
        .collect();

    // Bounce back to Sahara so the next `open()` on the dump pass gets
    // a fresh Hello. Without this Sahara times out.
    if let Err(e) = session.reset_to_edl(&mut log) {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            ltbox_core::i18n::tr("live_dumpparts_reset_failed").replace("{error}", &e.to_string())
        );
    }

    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_scan_complete")
            .replace("{count}", &rows.len().to_string())
    );
    DumpPartsScanResult {
        logs: log,
        rows,
        error: None,
    }
}

/// Post-dump stability window before the next EDL op. Large partition
/// reads (e.g. boot_a ~96 MB) leave the USB endpoint in a lingering state;
/// a subsequent reset/open can race a still-draining read and surface as
/// "stale COM port" or Sahara timeout. Mirrors v2 `post_sleep=15` in
/// `bin/ltbox/actions/edl.py::dump_partitions`.
const EDL_POST_DUMP_STABILIZE: std::time::Duration = std::time::Duration::from_secs(15);

/// Partition bases whose dump failure must be surfaced as a critical
/// error, not a per-row log line. These carry region/board state that a
/// subsequent rescue flow cannot reconstruct from scratch. Mirrors v2
/// `critical_targets` set in `bin/ltbox/actions/edl.py::dump_partitions`.
const CRITICAL_DUMP_BASES: &[&str] = &["devinfo", "persist"];

/// Match a partition label (possibly slot-suffixed) against the critical
/// base set. `devinfo`, `devinfo_a`, `DEVINFO_B` all match.
fn is_critical_dump_label(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    CRITICAL_DUMP_BASES
        .iter()
        .any(|base| l == *base || l.starts_with(&format!("{base}_")))
}

#[derive(Debug, Default)]
struct CountryPatchProgress {
    flashed_or_confirmed: Vec<String>,
    failures: Vec<String>,
}

impl CountryPatchProgress {
    fn mark_flashed(&mut self, label: &str) {
        if !self.flashed_or_confirmed.iter().any(|seen| seen == label) {
            self.flashed_or_confirmed.push(label.to_string());
        }
    }

    fn mark_failed(&mut self, label: &str, reason: impl Into<String>) {
        self.failures.push(format!("{label}: {}", reason.into()));
    }

    fn finish(&self) -> std::result::Result<(), String> {
        let missing = CRITICAL_DUMP_BASES
            .iter()
            .filter(|label| !self.flashed_or_confirmed.iter().any(|seen| seen == **label))
            .copied()
            .collect::<Vec<_>>();

        if self.failures.is_empty() && missing.is_empty() {
            return Ok(());
        }

        let mut parts = Vec::new();
        if !self.failures.is_empty() {
            parts.push(self.failures.join("; "));
        }
        if !missing.is_empty() {
            parts.push(format!("missing {}", missing.join(", ")));
        }
        Err(format!(
            "country-code patch incomplete ({})",
            parts.join("; ")
        ))
    }
}

/// Forward buffered worker logs to the stdout tap queue immediately.
///
/// Long-running advanced actions often collect lines in a local `Vec<String>`
/// and only hand that vec back on completion, which makes the exec card look
/// stalled. Emitting lines here lets the UI drain them every 500 ms via
/// `DrainStdoutTap`.
fn flush_worker_logs(log: &mut Vec<String>) {
    for line in log.drain(..) {
        println!("{line}");
    }
}

/// Dump selected partitions to `output_folder` as `<label>.img`. Reopens
/// the EDL session (previous scan left device waiting at Sahara), runs
/// the reads back-to-back, then reboots to system.
fn dump_parts_execute(
    loader_path: String,
    output_folder: String,
    rows: Vec<DumpPartRow>,
) -> Vec<String> {
    let mut log = Vec::new();
    let out_dir = std::path::PathBuf::from(&output_folder);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            ltbox_core::i18n::tr("live_dumpparts_create_output_failed")
                .replace("{error}", &e.to_string())
        );
        return log;
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                ltbox_core::i18n::tr("live_dumpparts_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return log;
        }
    };

    let mut critical_failures: Vec<String> = Vec::new();
    for row in &rows {
        let out_path = out_dir.join(format!("{}.img", row.label));
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            ltbox_core::i18n::tr("live_dumpparts_dumping")
                .replace("{label}", &row.label)
                .replace("{path}", &out_path.display().to_string())
                .replace("{lun}", &row.lun.to_string())
                .replace("{bytes}", &row.size_bytes.to_string())
        );
        if let Err(e) = session.dump_partition_at(
            &row.label,
            &out_path,
            row.lun,
            row.start_sector as u32,
            row.num_sectors as usize,
            &mut log,
        ) {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                ltbox_core::i18n::tr("live_dumpparts_part_failed")
                    .replace("{label}", &row.label)
                    .replace("{error}", &e.to_string())
            );
            if is_critical_dump_label(&row.label) {
                critical_failures.push(row.label.clone());
            }
        }
    }

    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_stabilizing")
            .replace("{seconds}", &EDL_POST_DUMP_STABILIZE.as_secs().to_string())
    );
    std::thread::sleep(EDL_POST_DUMP_STABILIZE);
    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_resetting")
    );
    session.reset_tolerant(&mut log);
    // Surface critical-partition failures prominently — region/board state
    // (devinfo/persist) can't be reconstructed from a partial dump and a
    // silent "Done." would hide the hazard.
    if !critical_failures.is_empty() {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            ltbox_core::i18n::tr("live_dumpparts_critical_failure")
                .replace("{labels}", &critical_failures.join(", "))
        );
    }
    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_done")
    );
    log
}

/// Whole-LUN dump. Walks each selected LUN and writes it as
/// `lun_N.img` into `output_folder`. Unlike `dump_parts_execute` there
/// is no prior scan phase — the LUN set comes straight from the user's
/// checkboxes.
fn dump_physical_execute(
    conn: ConnectionStatus,
    loader_path: String,
    output_folder: String,
    luns: Vec<u8>,
) -> Vec<String> {
    let mut log = Vec::new();
    if ensure_edl(conn, "DumpPhys", &mut log).is_err() {
        flush_worker_logs(&mut log);
        return Vec::new();
    }
    flush_worker_logs(&mut log);
    let out_dir = std::path::PathBuf::from(&output_folder);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        ltbox_core::live!(
            log,
            "[DumpPhys] {}",
            ltbox_core::i18n::tr("live_dump_phys_create_output_failed")
                .replace("{error}", &e.to_string())
        );
        flush_worker_logs(&mut log);
        return Vec::new();
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpPhys] {}",
                ltbox_core::i18n::tr("live_dump_phys_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            flush_worker_logs(&mut log);
            return Vec::new();
        }
    };
    flush_worker_logs(&mut log);

    for lun in &luns {
        let out_path = out_dir.join(format!("lun_{lun}.img"));
        ltbox_core::live!(
            log,
            "[DumpPhys] {}",
            ltbox_core::i18n::tr("live_dump_phys_dumping_lun")
                .replace("{lun}", &lun.to_string())
                .replace("{path}", &out_path.display().to_string())
        );
        flush_worker_logs(&mut log);
        if let Err(e) = session.dump_physical_storage(*lun, &out_path, &mut log) {
            ltbox_core::live!(
                log,
                "[DumpPhys] {}",
                ltbox_core::i18n::tr("live_dump_phys_lun_failed")
                    .replace("{lun}", &lun.to_string())
                    .replace("{error}", &e.to_string())
            );
        }
        flush_worker_logs(&mut log);
    }

    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        ltbox_core::i18n::tr("live_dump_phys_stabilizing_usb")
            .replace("{seconds}", &EDL_POST_DUMP_STABILIZE.as_secs().to_string())
    );
    flush_worker_logs(&mut log);
    std::thread::sleep(EDL_POST_DUMP_STABILIZE);
    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        ltbox_core::i18n::tr("live_dump_phys_resetting_system")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        ltbox_core::i18n::tr("live_dump_phys_done")
    );
    flush_worker_logs(&mut log);
    Vec::new()
}

/// Whole-LUN raw flash. Each `(lun, path)` pair is written verbatim
/// from sector 0. Mirrors qdlrs `OverwriteStorage`.
fn flash_physical_execute(
    conn: ConnectionStatus,
    loader_path: String,
    pairs: Vec<(u8, String)>,
) -> Vec<String> {
    let mut log = Vec::new();
    if ensure_edl(conn, "FlashPhys", &mut log).is_err() {
        return log;
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                ltbox_core::i18n::tr("live_flashphys_edl_open_failed")
                    .replace("{error}", &e.to_string())
            );
            return log;
        }
    };

    for (lun, path) in &pairs {
        let img = std::path::Path::new(path);
        if !img.exists() {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                ltbox_core::i18n::tr("live_flashphys_skipping_missing")
                    .replace("{lun}", &lun.to_string())
                    .replace("{path}", path)
            );
            continue;
        }
        let file_name = img
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        ltbox_core::live!(
            log,
            "[FlashPhys] {}",
            ltbox_core::i18n::tr("live_flashphys_flashing")
                .replace("{lun}", &lun.to_string())
                .replace("{file}", &file_name)
        );
        if let Err(e) = session.flash_physical_storage(*lun, img, &mut log) {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                ltbox_core::i18n::tr("live_flashphys_lun_failed")
                    .replace("{lun}", &lun.to_string())
                    .replace("{error}", &e.to_string())
            );
        }
    }

    ltbox_core::live!(
        log,
        "[FlashPhys] {}",
        ltbox_core::i18n::tr("live_flashphys_resetting")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[FlashPhys] {}",
        ltbox_core::i18n::tr("live_flashphys_done")
    );
    log
}

/// Locate a testkey PEM. Checks the image's folder, then `./keys/`.
fn find_edl_loader(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let candidate = dir.join("xbl_s_devprg_ns.melf");
    if candidate.exists() {
        return Some(candidate);
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name == "xbl_s_devprg_ns.melf" {
                return Some(entry.path());
            }
        }
    }
    None
}

fn is_loader_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "melf" | "mbn" | "elf"))
        .unwrap_or(false)
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
        "TB520FU" => DevicePortrait::Png(TB520FU_HANDLE.clone()),
        "TB710FU" => DevicePortrait::Png(TB710FU_HANDLE.clone()),
        _ => DevicePortrait::Svg(GENERIC_TABLET_SVG_HANDLE.clone()),
    }
}

fn svg_icon(bytes: &'static [u8], size: f32) -> Element<'static, Message> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .into()
}

/// Primary-coloured Lucide icon sized to `size`. Matches the colour
/// role the old per-asset SVG glyphs used for wizard tiles, status
/// markers, and confirm-step eyebrows.
fn lucide_primary(
    icon: iced::widget::Text<'static, Theme, iced::Renderer>,
    size: f32,
) -> Element<'static, Message> {
    icon.size(size)
        .style(|t: &Theme| iced::widget::text::Style {
            color: Some(pal_of(t).primary),
        })
        .into()
}

/// Lucide icon coloured by an arbitrary theme-driven closure. Used
/// where colour depends on widget state (nav active / disabled,
/// op success / failure, title-bar hover).
fn lucide_icon(
    icon: iced::widget::Text<'static, Theme, iced::Renderer>,
    size: f32,
    color: impl Fn(&Theme) -> iced::Color + 'static,
) -> Element<'static, Message> {
    icon.size(size)
        .style(move |t: &Theme| iced::widget::text::Style {
            color: Some(color(t)),
        })
        .into()
}

const WIZARD_CARD_HEIGHT: f32 = 180.0;

/// Fixed sub-row height (~2 lines at size 11) so cards line up across
/// translations.
const SUB_ROW_HEIGHT: f32 = 32.0;

fn icon_option_card_sub(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
    selected: bool,
    msg: Message,
) -> Element<'static, Message> {
    // Sub text centres vertically inside the fixed box — top-aligning
    // left long gaps between short descs and the label above.
    let sub_text: Element<'static, Message> = if sub.is_empty() {
        text(" ").size(11).width(Length::Fill).center().into()
    } else {
        text(sub.to_string())
            .size(11)
            .style(muted_style)
            .width(Length::Fill)
            .center()
            .into()
    };
    let sub_row = container(sub_text)
        .width(Length::Fill)
        .height(Length::Fixed(SUB_ROW_HEIGHT))
        .align_y(iced::alignment::Vertical::Center);
    // Explicit icon→label vs label→desc gaps — a single `spacing` read
    // unbalanced because the centred sub-row adds ~9 px padding.
    let content = column![
        icon_tile(icon),
        Space::new().height(14),
        text(label.to_string())
            .size(13)
            .style(on_surface_style)
            .width(Length::Fill)
            .center(),
        Space::new().height(4),
        sub_row,
    ]
    .spacing(0)
    .align_x(iced::Alignment::Center);

    button(
        container(content)
            .padding([20, 16])
            .width(Length::Fill)
            .height(WIZARD_CARD_HEIGHT)
            .center_x(Length::Fill)
            .center_y(WIZARD_CARD_HEIGHT)
            .style(move |t: &Theme| sel_card_style(t, selected)),
    )
    .on_press(msg)
    .padding(0)
    .width(Length::Fill)
    .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected))
    .into()
}

/// Wrap a wizard icon. Icons already carry their own rounded-rect bg,
/// so no outer border.
fn icon_tile(icon: Element<'static, Message>) -> Element<'static, Message> {
    container(icon).padding(0).into()
}

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

// -- Shared styles --------------------------------------------------------

/// Shared `Rule` styling so every shell-level divider (window
/// outline, title-bar bottom, sidebar-content split, status-bar
/// top) reads as the same hairline. Default rule color is
/// `background.strong` from iced's extended palette which is
/// noticeably darker than the M3 `outline_variant` used elsewhere.
fn shell_rule_style(t: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: pal_of(t).outline_variant,
        radius: 0.0.into(),
        fill_mode: iced::widget::rule::FillMode::Full,
        snap: true,
    }
}

/// Border-less surface fill for the sidebar / status-bar shell
/// panels. Adjacent shells double up `iced::Border` lines when
/// each side has its own 1-px outline; per M3 nav-rail / bottom-app-
/// bar guidance each shared edge should carry exactly one divider,
/// drawn as an explicit `Rule` widget so the corners read as
/// clean T-junctions instead of a 2-px overlap.
fn panel_bg(t: &Theme) -> container::Style {
    let p = pal_of(t);
    container::Style {
        background: Some(p.surface_container_low.into()),
        ..Default::default()
    }
}

/// Inner container style for option / Browse cards. Transparent
/// background — the outer button paints a hover-aware fill via
/// [`sel_card_btn_style`]; this style only renders the rounded border
/// so the visual outline survives without blocking the button's
/// interactive bg.
fn sel_card_style(t: &Theme, selected: bool) -> container::Style {
    let p = pal_of(t);
    container::Style {
        background: None,
        border: iced::Border {
            color: if selected {
                p.primary
            } else {
                p.outline_variant
            },
            width: if selected { 2.0 } else { 1.0 },
            radius: theme::shape::MD.into(),
        },
        ..Default::default()
    }
}

/// Outer button style for option / Browse cards. Drives the per-state
/// background (resting / hover / selected) so wizard cards visibly
/// react to mouse hover. Border carries the same MD radius as
/// [`sel_card_style`] so the bg fill clips to the rounded shape
/// instead of bleeding out as a square.
fn sel_card_btn_style(t: &Theme, status: button::Status, selected: bool) -> button::Style {
    let p = pal_of(t);
    let hovered = matches!(status, button::Status::Hovered);
    let bg = if selected {
        with_alpha(p.primary, 0.12)
    } else if hovered {
        with_alpha(p.primary, theme::state::HOVER)
    } else {
        p.surface_container
    };
    button::Style {
        background: Some(bg.into()),
        text_color: p.on_surface,
        border: iced::Border {
            radius: theme::shape::MD.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lang_jsons_parse_and_share_keys() {
        let en = Translations::load(Language::En);
        for lang in [Language::Ko, Language::Zh, Language::Ru] {
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
    fn language_switch_returns_localized_string() {
        let en = Translations::load(Language::En);
        let ko = Translations::load(Language::Ko);
        assert_eq!(en.t("nav_dashboard"), "Dashboard");
        assert_eq!(ko.t("nav_dashboard"), "대시보드");
    }

    #[test]
    fn unknown_key_falls_back_to_itself() {
        let t = Translations::load(Language::En);
        assert_eq!(t.t("__no_such_key__"), "__no_such_key__");
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
        let needle = format!("tr(\"{key}\")");
        let pos = source.find(&needle).expect("template key must be used");
        let end = (pos + 2_000).min(source.len());
        let window = &source[pos..end];
        let compact_window: String = window.chars().filter(|c| !c.is_whitespace()).collect();
        for placeholder in placeholders {
            let replacement = format!(".replace(\"{{{placeholder}}}\"");
            assert!(
                compact_window.contains(&replacement),
                "{key} must replace {{{placeholder}}} near its log call"
            );
        }
    }

    #[test]
    fn high_risk_log_templates_replace_visible_placeholders() {
        let main_rs = include_str!("main.rs");
        let edl_rs = include_str!("../../ltbox-device/src/edl.rs");

        assert_template_call_replaces(
            edl_rs,
            "log_edl_flash_program_cmd",
            &["label", "image", "lun", "start", "sectors"],
        );
        assert_template_call_replaces(
            main_rs,
            "live_country_dump_partition",
            &["label", "lun", "start", "sectors"],
        );
        assert_template_call_replaces(main_rs, "live_dump_phys_dumping_lun", &["lun", "path"]);
        assert_template_call_replaces(main_rs, "live_dump_phys_lun_failed", &["lun", "error"]);
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
            EdlEntryAction::FastbootContinueThenAdb
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
    fn country_patch_progress_requires_devinfo_and_persist() {
        let mut progress = CountryPatchProgress::default();
        progress.mark_flashed("devinfo");

        let err = progress.finish().expect_err("persist must be required");
        assert!(err.contains("persist"));
    }

    #[test]
    fn country_patch_progress_surfaces_partition_failures() {
        let mut progress = CountryPatchProgress::default();
        progress.mark_flashed("devinfo");
        progress.mark_failed("persist", "no known country code");

        let err = progress
            .finish()
            .expect_err("recorded persist failure must fail workflow");
        assert!(err.contains("persist: no known country code"));
    }
}
