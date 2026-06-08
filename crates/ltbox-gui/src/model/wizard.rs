//! Wizard model — the per-flow wizard state structs and their
//! navigation logic, extracted from `main.rs`.

use crate::pickers;
use crate::{
    AdvAction, Family, LOADER_PICKER_EXTS, NightlySource, Provider, RootMode, VerChoice,
    is_loader_file,
};

// Internal steps: 0=Family, 1=Mode, 2=Provider, 3=Version,
// 4=NightlySource, 5=Folder, 6=Confirm, 7=Flash, 8=APatch KPM.
// Mode auto-skips for non-KSU. GKI: steps 3/4 collapse into a kernel
// zip picker at 2. MagiskForks: skip Version, APK picker at 3. Nightly
// inserts 4 between Version and Folder.
#[derive(Default)]
pub(crate) struct RootWizard {
    pub(crate) step: usize,
    pub(crate) family: Option<Family>,
    pub(crate) mode: Option<RootMode>,
    pub(crate) provider: Option<Provider>,
    pub(crate) version: Option<VerChoice>,
    pub(crate) nightly_source: Option<NightlySource>,
    pub(crate) file_path: Option<String>, // GKI zip, MagiskForks APK, or manual nightly
    pub(crate) folder_path: Option<String>, // Firmware folder (loader + optional testkey)
    /// APatch: `.kpm` modules to embed. Multi-select + per-entry remove.
    pub(crate) kpm_paths: Vec<String>,
    /// APatch superkey. Secret — never echoed in confirm or any log.
    pub(crate) superkey: Option<String>,
    pub(crate) superkey_popup_open: bool,
    /// Buffer for the currently visible field in the superkey popup;
    /// reset between the first-entry and re-entry stages.
    pub(crate) superkey_buffer: String,
    /// First-entry value held while the popup waits for the user to
    /// re-enter their key on the second stage. `None` → still on the
    /// first-entry stage; `Some(v)` → on the verification stage and
    /// `superkey_buffer` will be compared against `v` on Confirm.
    pub(crate) superkey_first_entry: Option<String>,
    /// Nightly ManualInput: committed workflow run ID (1..=12 digits).
    /// Only meaningful when `nightly_source == Some(ManualInput)`.
    pub(crate) run_id: Option<String>,
    pub(crate) run_id_popup_open: bool,
    pub(crate) run_id_buffer: String,
    /// KernelSU LKM: normalized `major.minor` kernel version from ADB or manual popup.
    pub(crate) kernel_version: Option<String>,
    pub(crate) kernel_version_popup_open: bool,
    pub(crate) kernel_version_buffer: String,
}

pub(crate) const ROOT_STEPS: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_provider",
    "root_step_version",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_NIGHTLY: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_provider",
    "root_step_version",
    "root_step_source",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_GKI: &[&str] = &[
    "root_step_type",
    "root_step_mode",
    "root_step_kernel",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_NOMODE: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_NOMODE_NIGHTLY: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_source",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_FORKS: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_apk",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_APATCH: &[&str] = &[
    "root_step_type",
    "root_step_provider",
    "root_step_version",
    "root_step_kpm",
    "root_step_folder",
    "root_step_confirm",
    "root_step_flash",
];
pub(crate) const ROOT_STEPS_APATCH_NIGHTLY: &[&str] = &[
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
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// True on the final (flash/exec) step. Used to skip wizard reset
    /// when the user sidebar-bounces mid-operation.
    pub(crate) fn is_in_exec(&self) -> bool {
        self.step == 7
    }
    /// True on the confirm screen (step 6, before Flash). A sidebar
    /// bounce here preserves the wizard instead of resetting to step 0.
    pub(crate) fn is_on_confirm_step(&self) -> bool {
        self.step == 6
    }

    pub(crate) fn is_gki(&self) -> bool {
        self.mode == Some(RootMode::Gki)
    }
    pub(crate) fn is_forks(&self) -> bool {
        self.provider == Some(Provider::MagiskForks)
    }
    pub(crate) fn is_nightly(&self) -> bool {
        self.version == Some(VerChoice::Nightly)
    }
    pub(crate) fn is_apatch(&self) -> bool {
        self.family == Some(Family::APatch)
    }

    pub(crate) fn is_ksu_lkm(&self) -> bool {
        self.family == Some(Family::KernelSU) && self.mode == Some(RootMode::Lkm)
    }

    pub(crate) fn needs_ksu_lkm_kernel_version(&self) -> bool {
        self.is_ksu_lkm() && self.kernel_version.is_none()
    }

    pub(crate) fn active_steps(&self) -> &'static [&'static str] {
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

    pub(crate) fn display_step(&self) -> usize {
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

    pub(crate) fn next(&mut self) {
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

    pub(crate) fn back(&mut self) {
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

    pub(crate) fn can_next(&self) -> bool {
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

/// Linear-step wizard contract. Wizards whose `next` / `back` simply
/// walk a 0..step_count range share `reset` / `next` / `back` /
/// `is_in_exec` via this trait's default impls; only `step`,
/// `step_mut`, `step_count`, and `can_next` need per-impl bodies.
///
/// Not implemented for `RootWizard` because its non-linear step
/// numbering (steps skip around depending on family/mode) requires
/// custom navigation logic.
pub(crate) trait Wizard: Default {
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
    /// True on the confirm/start screen — the step immediately before
    /// exec. A sidebar bounce here preserves the wizard (the user returns
    /// to the confirm screen) instead of resetting to step 0.
    fn is_on_confirm_step(&self) -> bool {
        let n = self.step_count();
        n >= 2 && self.step() == n - 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnrootType {
    MagiskLkm,
    APatchGki,
}
impl UnrootType {
    pub(crate) fn label_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm",
            Self::APatchGki => "unroottype_apatch_gki",
        }
    }
    pub(crate) fn desc_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm_desc",
            Self::APatchGki => "unroottype_apatch_gki_desc",
        }
    }
    pub(crate) fn folder_desc_key(&self) -> &'static str {
        match self {
            Self::MagiskLkm => "unroottype_magisk_lkm_folderdesc",
            Self::APatchGki => "unroottype_apatch_gki_folderdesc",
        }
    }
}

#[derive(Default)]
pub(crate) struct UnrootWizard {
    pub(crate) step: usize,
    pub(crate) unroot_type: Option<UnrootType>,
    pub(crate) folder_path: Option<String>,
    /// Loader file (`xbl_s_devprg_ns.melf`) for the EDL flash. Has
    /// its own wizard step. The Settings-level default loader
    /// auto-fills + auto-advances the loader step on Next from the
    /// method step (mirrors the Root wizard's step-5 fold-through);
    /// anyone without a default sees the explicit loader picker.
    pub(crate) loader_path: Option<String>,
}

pub(crate) const UNROOT_STEPS: &[&str] = &[
    "unroot_step_method",
    "unroot_step_loader",
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
        // Step indexes match `UNROOT_STEPS` — loader is its own step
        // (#1) so the folder step (#2) only gates on the backup folder
        // pick and doesn't have to bundle a loader sub-row.
        match self.step {
            0 => self.unroot_type.is_some(),
            1 => self.loader_path.is_some(),
            2 => self.folder_path.is_some(),
            3 => true,
            _ => false,
        }
    }
}

// =========================================================================
// Flash wizard state
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceRegion {
    Prc,
    Row,
}
impl DeviceRegion {
    pub(crate) fn label_key(&self) -> &'static str {
        match self {
            Self::Prc => "deviceregion_prc",
            Self::Row => "deviceregion_row",
        }
    }

    pub(crate) fn to_region_target(self) -> ltbox_patch::region::RegionTarget {
        match self {
            Self::Prc => ltbox_patch::region::RegionTarget::Prc,
            Self::Row => ltbox_patch::region::RegionTarget::Row,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlashTarget {
    OtherRegion,
    SameRegion,
}
impl FlashTarget {
    pub(crate) fn label_key(&self) -> &'static str {
        match self {
            Self::OtherRegion => "flashtarget_other",
            Self::SameRegion => "flashtarget_same",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataMode {
    Keep,
    Wipe,
}
impl DataMode {
    pub(crate) fn label_key(&self) -> &'static str {
        match self {
            Self::Keep => "datamode_keep",
            Self::Wipe => "datamode_wipe",
        }
    }
}

#[derive(Default)]
pub(crate) struct FlashWizard {
    pub(crate) step: usize,
    pub(crate) device_region: Option<DeviceRegion>,
    pub(crate) target: Option<FlashTarget>,
    pub(crate) data_mode: Option<DataMode>,
    pub(crate) firmware_folder: Option<String>,
}

pub(crate) const FLASH_STEPS: &[&str] = &[
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
pub(crate) enum SysUpdateAction {
    Disable,
    Enable,
    Rescue,
}
impl SysUpdateAction {
    pub(crate) fn label_key(&self) -> &'static str {
        match self {
            Self::Disable => "sysupdate_disable",
            Self::Enable => "sysupdate_enable",
            Self::Rescue => "sysupdate_rescue",
        }
    }
    pub(crate) fn desc_key(&self) -> &'static str {
        match self {
            Self::Disable => "sysupdate_disable_desc",
            Self::Enable => "sysupdate_enable_desc",
            Self::Rescue => "sysupdate_rescue_desc",
        }
    }
}

/// Region target for Boot Recovery (Rescue). PRC/ROW hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RescueRegion {
    Prc,
    Row,
}

impl RescueRegion {
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Prc => "rescue_region_prc",
            Self::Row => "rescue_region_row",
        }
    }
    pub(crate) fn to_target(self) -> ltbox_patch::region::RegionTarget {
        match self {
            Self::Prc => ltbox_patch::region::RegionTarget::Prc,
            Self::Row => ltbox_patch::region::RegionTarget::Row,
        }
    }
}

#[derive(Default)]
pub(crate) struct SysUpdateWizard {
    pub(crate) step: usize,
    pub(crate) action: Option<SysUpdateAction>,
    /// Rescue: firmware folder containing loader (`xbl_s_devprg_ns.melf`).
    pub(crate) rescue_folder: Option<String>,
    /// Rescue: selected target region. Set via popup between Folder and
    /// Confirm steps. May be pre-seeded from `inferred_flash_region`
    /// (PTSTPD `SaleArea`) before the popup opens — `rescue_region_confirmed`
    /// tracks whether the user explicitly clicked through.
    pub(crate) rescue_region: Option<RescueRegion>,
    /// Rescue: region popup overlay flag. Opens on Next press from the
    /// Folder step when the user hasn't yet confirmed a region pick.
    pub(crate) rescue_region_popup_open: bool,
    /// Rescue: true once the user has clicked a region radio in the
    /// popup. Distinguishes a pre-seeded `rescue_region` (initial
    /// preselect from `inferred_flash_region`) from a user-confirmed
    /// pick — preselect alone shouldn't skip the popup.
    pub(crate) rescue_region_confirmed: bool,
}

pub(crate) const SYSUPDATE_STEPS_COMPACT: &[&str] = &[
    "sysupdate_step_action",
    "sysupdate_step_confirm",
    "sysupdate_step_execute",
];

pub(crate) const SYSUPDATE_STEPS_RESCUE: &[&str] = &[
    "sysupdate_step_action",
    "sysupdate_step_folder",
    "sysupdate_step_confirm",
    "sysupdate_step_execute",
];

impl SysUpdateWizard {
    /// Rescue gets an extra Folder step — distinct step list keeps the
    /// other actions (Disable/Enable) on their short 3-step flow.
    pub(crate) fn steps(&self) -> &'static [&'static str] {
        if matches!(self.action, Some(SysUpdateAction::Rescue)) {
            SYSUPDATE_STEPS_RESCUE
        } else {
            SYSUPDATE_STEPS_COMPACT
        }
    }
    pub(crate) fn is_rescue(&self) -> bool {
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
                    .is_some_and(|p| {
                        is_loader_file(p)
                            || ltbox_core::sahara_xml::is_encrypted_manifest_filename(p)
                    }),
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

/// Tri-state row action — clicking the checkbox cycles through these
/// in order. Flash requires a `file_path`; Erase wipes the sector range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FlashRowState {
    #[default]
    Unchecked,
    Flash,
    Erase,
}

impl FlashRowState {
    pub(crate) fn cycle(self) -> Self {
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
pub(crate) struct FlashPartRow {
    pub(crate) lun: u8,
    pub(crate) label: String,
    pub(crate) start_sector: u64,
    pub(crate) num_sectors: u64,
    pub(crate) size_bytes: u64,
    pub(crate) file_path: Option<String>,
    pub(crate) state: FlashRowState,
}

/// Column the partition table is currently sorted by. Header click
/// fires `*SortBy(col)`; clicking the active column toggles direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PartsSortColumn {
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
pub(crate) struct FlashPartsWizard {
    pub(crate) step: usize, // 0=Loader, 1=Select, 2=Confirm, 3=Exec
    pub(crate) loader_path: Option<String>,
    pub(crate) rows: Vec<FlashPartRow>,
    pub(crate) scanning: bool,
    pub(crate) scan_error: Option<String>,
    pub(crate) sort_col: PartsSortColumn,
    /// `true` → descending. Default `false` (ascending) on first scan
    /// so initial layout matches the device's GPT order well enough
    /// for LUN-then-label browsing.
    pub(crate) sort_desc: bool,
}

pub(crate) const FLASH_PARTS_STEPS: &[&str] = &[
    "flash_parts_step_loader",
    "flash_parts_step_select",
    "flash_step_confirm",
    "flash_step_flash",
];

impl FlashPartsWizard {
    pub(crate) fn active_rows(&self) -> Vec<FlashPartRow> {
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
    pub(crate) fn apply_sort(&mut self) {
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
    pub(crate) fn toggle_sort(&mut self, col: PartsSortColumn) {
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
pub(crate) struct FlashPartsScanResult {
    pub(crate) logs: Vec<String>,
    pub(crate) rows: Vec<FlashPartRow>,
    pub(crate) error: Option<String>,
}

// =========================================================================
// Dump Partitions wizard state (Advanced → Dump Partitions)
// =========================================================================

#[derive(Debug, Clone)]
pub(crate) struct DumpPartRow {
    pub(crate) lun: u8,
    pub(crate) label: String,
    pub(crate) start_sector: u64,
    pub(crate) num_sectors: u64,
    pub(crate) size_bytes: u64,
    pub(crate) selected: bool,
}

/// Scan-phase result carried in a single message.
#[derive(Debug, Clone, Default)]
pub(crate) struct DumpPartsScanResult {
    pub(crate) logs: Vec<String>,
    pub(crate) rows: Vec<DumpPartRow>,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
pub(crate) struct DumpPartsWizard {
    pub(crate) step: usize, // 0=Loader, 1=Select, 2=Exec
    pub(crate) loader_path: Option<String>,
    pub(crate) rows: Vec<DumpPartRow>,
    pub(crate) output_dir: Option<String>,
    pub(crate) scanning: bool,
    pub(crate) scan_error: Option<String>,
    pub(crate) sort_col: PartsSortColumn,
    pub(crate) sort_desc: bool,
}

pub(crate) const DUMP_PARTS_STEPS: &[&str] = &[
    "dump_parts_step_loader",
    "dump_parts_step_select",
    "dump_parts_step_dump",
];

impl DumpPartsWizard {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }
    pub(crate) fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some() && !self.scanning,
            1 => self.rows.iter().any(|r| r.selected),
            _ => false,
        }
    }
    pub(crate) fn selected_rows(&self) -> Vec<DumpPartRow> {
        self.rows.iter().filter(|r| r.selected).cloned().collect()
    }

    pub(crate) fn apply_sort(&mut self) {
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

    pub(crate) fn toggle_sort(&mut self, col: PartsSortColumn) {
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

pub(crate) const PHYS_LUN_COUNT: usize = 6;

#[derive(Default)]
pub(crate) struct DumpPhysWizard {
    pub(crate) step: usize, // 0=Loader, 1=Select, 2=Exec
    pub(crate) loader_path: Option<String>,
    pub(crate) selected: [bool; PHYS_LUN_COUNT],
    pub(crate) output_dir: Option<String>,
    pub(crate) loader_error: Option<String>,
}

pub(crate) const DUMP_PHYS_STEPS: &[&str] = &[
    "dump_parts_step_loader",
    "phys_step_select",
    "dump_parts_step_dump",
];

impl DumpPhysWizard {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }
    pub(crate) fn can_next(&self) -> bool {
        match self.step {
            0 => self.loader_path.is_some(),
            1 => self.selected.iter().any(|&s| s),
            _ => false,
        }
    }
    pub(crate) fn selected_luns(&self) -> Vec<u8> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i as u8) } else { None })
            .collect()
    }
}

#[derive(Default)]
pub(crate) struct FlashPhysWizard {
    pub(crate) step: usize, // 0=Loader, 1=Select, 2=Confirm, 3=Exec
    pub(crate) loader_path: Option<String>,
    pub(crate) selected: [bool; PHYS_LUN_COUNT],
    pub(crate) file_paths: [Option<String>; PHYS_LUN_COUNT],
    pub(crate) loader_error: Option<String>,
}

pub(crate) const FLASH_PHYS_STEPS: &[&str] = &[
    "flash_parts_step_loader",
    "phys_step_select",
    "flash_step_confirm",
    "flash_step_flash",
];

impl FlashPhysWizard {
    /// (LUN, file_path) pairs for every selected, file-bound row.
    pub(crate) fn active_pairs(&self) -> Vec<(u8, String)> {
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

// =========================================================================
// Simple Firmware Flash wizard state (Advanced → EDL ops)
// =========================================================================

/// Minimal flash wizard for the "Simple Firmware Flash" advanced op: pick a
/// firmware folder, review a read-only confirm screen, flash. Step 0 (intro)
/// opens the folder picker on Next; the picker callback advances to the
/// confirm step. No region / rollback / data choices — the flash runs the
/// firmware's own rawprogram verbatim.
#[derive(Default)]
pub(crate) struct SimpleFlashWizard {
    pub(crate) step: usize, // 0=Intro, 1=Confirm, 2=Exec
    pub(crate) firmware_folder: Option<String>,
}

pub(crate) const SIMPLE_FLASH_STEPS: &[&str] = &[
    "simple_flash_step_intro",
    "flash_step_confirm",
    "flash_step_flash",
];

impl Wizard for SimpleFlashWizard {
    fn step(&self) -> usize {
        self.step
    }
    fn step_mut(&mut self) -> &mut usize {
        &mut self.step
    }
    fn step_count(&self) -> usize {
        SIMPLE_FLASH_STEPS.len()
    }
    fn can_next(&self) -> bool {
        // Intro (0): Next opens the firmware-folder picker. Confirm (1):
        // Start. Exec (2) has no Next.
        matches!(self.step, 0 | 1)
    }
}

/// Wizard for every non-FlashPartitions Advanced action. Steps are
/// [source, confirm, exec], plus country step between for `PatchDevinfo`.
#[derive(Default, Debug, Clone)]
pub(crate) struct AdvWizard {
    pub(crate) action: Option<AdvAction>,
    pub(crate) step: usize,
    pub(crate) file_path: Option<String>,
    pub(crate) file_paths: Vec<String>,
    pub(crate) country: Option<String>,
    /// User-picked target region for `RegionConvert`. Explicit target so
    /// confirm can echo it and exec can short-circuit on no-op.
    pub(crate) region_target: Option<DeviceRegion>,
    /// `{exe_dir}/output_<action>/` — set on Confirm → Exec.
    pub(crate) output_dir: Option<std::path::PathBuf>,
    /// PatchArb: live-typing buffer for the unix-timestamp popup.
    pub(crate) arb_index_buffer: String,
    /// PatchArb: committed target rollback index. Gates inspect-step Next.
    pub(crate) arb_index_committed: Option<u64>,
    /// PatchArb: `(boot_rollback, vbmeta_rollback)` from picked firmware.
    pub(crate) arb_inspect: Option<(u64, u64)>,
}

impl AdvWizard {
    pub(crate) fn open(&mut self, a: AdvAction) {
        *self = Self::default();
        self.action = Some(a);
    }
    pub(crate) fn needs_country(&self) -> bool {
        matches!(self.action, Some(AdvAction::PatchDevinfo))
    }
    pub(crate) fn needs_region_target(&self) -> bool {
        matches!(self.action, Some(AdvAction::RegionConvert))
    }
    pub(crate) fn is_image_info(&self) -> bool {
        matches!(self.action, Some(AdvAction::ImageInfo))
    }
    pub(crate) fn steps(&self) -> &'static [&'static str] {
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
    pub(crate) fn exec_step(&self) -> usize {
        self.steps().len() - 1
    }
    pub(crate) fn is_confirm_step(&self) -> bool {
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
    pub(crate) fn is_folder_op(&self) -> bool {
        matches!(
            self.action,
            // PatchDevinfo: folder holds devinfo.img + persist.img.
            // ConvertXml: folder holds the encrypted `*.x` pack.
            Some(AdvAction::PatchDevinfo) | Some(AdvAction::ConvertXml) | Some(AdvAction::PatchArb)
        )
    }
    /// Extension whitelist for `rfd::AsyncFileDialog::add_filter`.
    /// Empty slice = no constraint.
    pub(crate) fn accepted_exts(&self) -> (&'static str, &'static [&'static str]) {
        match self.action {
            Some(AdvAction::RegionConvert)
            | Some(AdvAction::ImageInfo)
            | Some(AdvAction::RebuildVbmeta) => ("Android partition image (*.img)", &["img"]),
            Some(AdvAction::DetectArb) => ("EDL loader (.melf / .xml / .x)", LOADER_PICKER_EXTS),
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
    pub(crate) fn picker_kind(&self) -> pickers::PickerKind {
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
    pub(crate) fn picker_target_i18n_key(&self) -> &'static str {
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
