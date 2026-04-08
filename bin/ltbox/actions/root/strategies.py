import shutil
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

from ... import constants as const
from ... import device, utils
from ...i18n import get_string
from ...patch.avb import (
    process_boot_image_avb,
    rebuild_vbmeta_with_chained_images,
    vbmeta_has_chain_partition,
)
from ...patch.root import patch_boot_with_root_algo
from ...root_profiles import (
    RootProviderFamily,
    get_root_provider_profile,
)
from .downloads import (
    download_apatch_resources,
    download_lkm_resources,
)
from .prompts import (
    StrategySourceSelection,
    prompt_apatch_superkey,
    prompt_embed_kpm,
    prompt_kpm_selection,
    select_apatch_source,
    select_lkm_source,
    wait_for_kpm_files,
)


class RootStrategy(ABC):
    @property
    @abstractmethod
    def image_name(self) -> str:
        pass

    @property
    @abstractmethod
    def backup_name(self) -> str:
        pass

    @property
    @abstractmethod
    def output_dir(self) -> Path:
        pass

    @property
    @abstractmethod
    def backup_dir(self) -> Path:
        pass

    @property
    @abstractmethod
    def required_files(self) -> List[str]:
        pass

    @property
    def requires_vbmeta(self) -> bool:
        return const.FN_VBMETA in self.required_files

    @property
    @abstractmethod
    def log_output_dir_name(self) -> str:
        pass

    @property
    @abstractmethod
    def display_name(self) -> str:
        pass

    @property
    @abstractmethod
    def unroot_detect_msg_key(self) -> str:
        pass

    @property
    @abstractmethod
    def unroot_menu_msg_key(self) -> str:
        pass

    @property
    @abstractmethod
    def menu_shortcut(self) -> str:
        pass

    @property
    @abstractmethod
    def unroot_files(self) -> Dict[str, Path]:
        pass

    @property
    @abstractmethod
    def is_unroot_available(self) -> bool:
        pass

    @property
    @abstractmethod
    def patch_image_name(self) -> str:
        pass

    @property
    def requires_kernel_version(self) -> bool:
        return False

    @property
    def manager_apk_required(self) -> bool:
        return True

    @abstractmethod
    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        pass

    @abstractmethod
    def get_partition_map(self, suffix: str) -> Dict[str, str]:
        pass

    @abstractmethod
    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        pass

    @abstractmethod
    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        pass

    @abstractmethod
    def finalize_patch(
        self, patched_boot: Path, output_dir: Path, backup_source_dir: Path
    ) -> Path:
        pass


@dataclass(frozen=True)
class RootStrategySpec:
    image_name: str
    backup_name: str
    output_dir: Path
    backup_dir: Path
    required_files: List[str]
    main_partition: str
    display_name: str
    unroot_detect_msg_key: str
    unroot_menu_msg_key: str
    menu_shortcut: str
    patch_image_name: str
    requires_kernel_version: bool = False


class ConfigurableRootStrategy(RootStrategy):
    spec: RootStrategySpec

    @property
    def image_name(self) -> str:
        return self.spec.image_name

    @property
    def backup_name(self) -> str:
        return self.spec.backup_name

    @property
    def output_dir(self) -> Path:
        return self.spec.output_dir

    @property
    def backup_dir(self) -> Path:
        return self.spec.backup_dir

    @property
    def required_files(self) -> List[str]:
        return self.spec.required_files

    @property
    def display_name(self) -> str:
        return self.spec.display_name

    @property
    def unroot_detect_msg_key(self) -> str:
        return self.spec.unroot_detect_msg_key

    @property
    def unroot_menu_msg_key(self) -> str:
        return self.spec.unroot_menu_msg_key

    @property
    def menu_shortcut(self) -> str:
        return self.spec.menu_shortcut

    @property
    def patch_image_name(self) -> str:
        return self.spec.patch_image_name

    @property
    def requires_kernel_version(self) -> bool:
        return self.spec.requires_kernel_version

    @property
    def log_output_dir_name(self) -> str:
        return self.output_dir.name

    @property
    def unroot_files(self) -> Dict[str, Path]:
        files = {"main": self.backup_dir / self.image_name}
        if self.requires_vbmeta:
            files["vbmeta"] = self.backup_dir / const.FN_VBMETA
        return files

    @property
    def is_unroot_available(self) -> bool:
        return all(p.exists() for p in self.unroot_files.values())

    def get_partition_map(self, suffix: str) -> Dict[str, str]:
        partition_map = {"main": f"{self.spec.main_partition}{suffix}", "vbmeta": ""}
        if self.requires_vbmeta:
            partition_map["vbmeta"] = f"vbmeta{suffix}"
        return partition_map


class InitBootRootStrategy(ConfigurableRootStrategy):
    @property
    @abstractmethod
    def payload_files(self) -> List[str]:
        pass

    @property
    @abstractmethod
    def root_type(self) -> str:
        pass

    @property
    @abstractmethod
    def staging_dir(self) -> Path:
        pass

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        magiskboot_exe = const.MAGISKBOOT_EXE

        init_boot_source = work_dir / self.image_name
        init_boot_backup = const.BASE_DIR / self.backup_name
        if init_boot_source.exists() and not init_boot_backup.exists():
            shutil.copy(init_boot_source, init_boot_backup)

        if not all((self.staging_dir / name).exists() for name in self.payload_files):
            if not self.download_resources(lkm_kernel_version):
                return None

        for name in self.payload_files:
            shutil.copy(self.staging_dir / name, work_dir / name)

        return patch_boot_with_root_algo(
            work_dir,
            magiskboot_exe,
            dev,
            gki=False,
            lkm_kernel_version=lkm_kernel_version,
            root_type=self.root_type,
            skip_lkm_download=True,
        )

    def finalize_patch(
        self, patched_boot: Path, output_dir: Path, backup_source_dir: Path
    ) -> Path:
        process_boot_image_avb(patched_boot, gki=False, backup_dir=backup_source_dir)

        vbmeta_bak = backup_source_dir / const.FN_VBMETA_BAK
        patched_vbmeta_path = const.BASE_DIR / const.FN_VBMETA_ROOT

        rebuild_vbmeta_with_chained_images(
            output_path=patched_vbmeta_path,
            original_vbmeta_path=vbmeta_bak,
            chained_images=[patched_boot],
        )

        final_boot = output_dir / self.image_name
        shutil.move(patched_boot, final_boot)

        if patched_vbmeta_path.exists():
            shutil.move(patched_vbmeta_path, output_dir / const.FN_VBMETA)

        return final_boot


class GkiRootStrategy(ConfigurableRootStrategy):
    spec = RootStrategySpec(
        image_name=const.FN_BOOT,
        backup_name=const.FN_BOOT_BAK,
        output_dir=const.OUTPUT_ROOT_DIR,
        backup_dir=const.BACKUP_BOOT_DIR,
        required_files=[const.FN_BOOT, const.FN_VBMETA],
        main_partition="boot",
        display_name="GKI",
        unroot_detect_msg_key="act_unroot_gki_detected",
        unroot_menu_msg_key="act_unroot_menu_3_gki",
        menu_shortcut="3",
        patch_image_name="boot.img",
    )

    def __init__(self) -> None:
        super().__init__()
        self._kernel_zip: Optional[Path] = None

    @property
    def manager_apk_required(self) -> bool:
        return False

    def configure_source(self, breadcrumbs: Optional[str] = None) -> bool:
        self._kernel_zip = _prompt_custom_kernel_zip()
        if self._kernel_zip is None:
            return False

        self.source_label = self._kernel_zip.name
        _extract_manager_apk_from_zip(self._kernel_zip)
        return True

    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        utils.ui.echo(
            get_string("act_unroot_step4_gki").format(part=partition_map["main"])
        )

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        if self._kernel_zip is None:
            utils.ui.warn(get_string("gki_custom_cancelled"))
            return False
        return True

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        if self._kernel_zip is None:
            utils.ui.error(get_string("gki_custom_cancelled"))
            return None

        magiskboot_exe = const.MAGISKBOOT_EXE
        return patch_boot_with_root_algo(
            work_dir,
            magiskboot_exe,
            dev=None,
            gki=True,
            custom_kernel_zip=self._kernel_zip,
        )

    def finalize_patch(
        self, patched_boot: Path, output_dir: Path, backup_source_dir: Path
    ) -> Path:
        vbmeta_bak = backup_source_dir / const.FN_VBMETA_BAK

        if vbmeta_bak.exists() and not vbmeta_has_chain_partition(vbmeta_bak, "boot"):
            process_boot_image_avb(patched_boot, gki=True, backup_dir=backup_source_dir)

            patched_vbmeta_path = const.BASE_DIR / const.FN_VBMETA_ROOT
            rebuild_vbmeta_with_chained_images(
                output_path=patched_vbmeta_path,
                original_vbmeta_path=vbmeta_bak,
                chained_images=[patched_boot],
            )
        else:
            process_boot_image_avb(patched_boot, gki=True, backup_dir=backup_source_dir)
            patched_vbmeta_path = None

        final_boot = output_dir / self.image_name
        shutil.move(patched_boot, final_boot)

        if patched_vbmeta_path and patched_vbmeta_path.exists():
            shutil.move(patched_vbmeta_path, output_dir / const.FN_VBMETA)

        return final_boot


class APatchStrategy(GkiRootStrategy):
    spec = RootStrategySpec(
        image_name=const.FN_BOOT,
        backup_name=const.FN_BOOT_BAK,
        output_dir=const.OUTPUT_ROOT_DIR,
        backup_dir=const.BACKUP_BOOT_DIR,
        required_files=[const.FN_BOOT, const.FN_VBMETA],
        main_partition="boot",
        display_name="APatch",
        unroot_detect_msg_key="act_unroot_gki_detected",
        unroot_menu_msg_key="act_unroot_menu_3_gki",
        menu_shortcut="5",
        patch_image_name="boot.img",
        requires_kernel_version=False,
    )

    def __init__(self, root_type: str = "folkpatch"):
        super().__init__()
        self.provider = get_root_provider_profile(root_type)
        if self.provider.family != RootProviderFamily.APATCH:
            raise ValueError(f"Expected an APatch-family provider, got: {root_type}")
        self.root_type = self.provider.provider_id
        self.is_nightly = False
        self.workflow_id: Optional[str] = None
        self.repo_config: Dict[str, Any] = {}
        self._staging_dir = const.TOOLS_DIR / f"{self.root_type}_staging"

    @property
    def manager_apk_required(self) -> bool:
        return True

    @property
    def source_name(self) -> str:
        return self.provider.display_name

    def _apply_source_selection(self, selection: StrategySourceSelection) -> None:
        self.repo_config = selection.repo_config
        self.source_label = selection.source_label
        self.is_nightly = selection.is_nightly
        self.workflow_id = selection.workflow_id

    def configure_source(self, breadcrumbs: Optional[str] = None) -> bool:
        self._apply_source_selection(
            select_apatch_source(self.provider.provider_id, breadcrumbs=breadcrumbs)
        )
        return True

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        return download_apatch_resources(
            profile=self.provider,
            staging_dir=self._staging_dir,
            repo_config=self.repo_config,
            is_nightly=self.is_nightly,
            workflow_id=self.workflow_id,
        )

    def _select_kpm_paths(self, work_dir: Path) -> List[Path]:
        if not prompt_embed_kpm():
            utils.ui.echo(get_string("apatch_kpm_skipped"))
            return []

        kpm_dir = work_dir / "kpm"
        kpm_dir.mkdir(exist_ok=True)

        utils.ui.clear()
        utils.ui.echo(get_string("apatch_kpm_folder_instruction").format(path=kpm_dir))

        kpm_files = wait_for_kpm_files(kpm_dir)
        if not kpm_files:
            utils.ui.echo(get_string("apatch_kpm_skipped"))
            return []

        selected = prompt_kpm_selection(kpm_files)
        if not selected:
            utils.ui.echo(get_string("apatch_kpm_skipped"))
            return []

        utils.ui.clear()
        utils.ui.echo(get_string("apatch_kpm_embedding").format(count=len(selected)))
        return selected

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        magiskboot_exe = const.MAGISKBOOT_EXE

        superkey = prompt_apatch_superkey(self.source_name)
        utils.ui.clear()
        kpm_paths = self._select_kpm_paths(work_dir)

        utils.ui.clear()
        kpimg_src = self._staging_dir / "kpimg"
        if kpimg_src.exists():
            shutil.copy(kpimg_src, work_dir / "kpimg")
        else:
            utils.ui.error(get_string("apatch_kpimg_missing"))
            return None

        return patch_boot_with_root_algo(
            work_dir,
            magiskboot_exe,
            dev=dev,
            gki=True,
            root_type=self.root_type,
            superkey=superkey,
            kpm_paths=kpm_paths,
        )


class LkmRootStrategy(InitBootRootStrategy):
    spec = RootStrategySpec(
        image_name=const.FN_INIT_BOOT,
        backup_name=const.FN_INIT_BOOT_BAK,
        output_dir=const.OUTPUT_ROOT_LKM_DIR,
        backup_dir=const.BACKUP_INIT_BOOT_DIR,
        required_files=[const.FN_INIT_BOOT, const.FN_VBMETA],
        main_partition="init_boot",
        display_name="LKM",
        unroot_detect_msg_key="act_unroot_lkm_detected",
        unroot_menu_msg_key="act_unroot_menu_2_lkm",
        menu_shortcut="2",
        patch_image_name="init_boot.img (LKM)",
        requires_kernel_version=True,
    )

    def __init__(self, root_type: str = "ksu"):
        self.provider = get_root_provider_profile(root_type)
        if self.provider.family != RootProviderFamily.LKM:
            raise ValueError(f"Expected an LKM provider, got: {root_type}")
        self.is_nightly = False
        self.is_tagged_build = False
        self.workflow_id: Optional[str] = None
        self.repo_config: Dict[str, Any] = {}
        self._staging_dir = const.TOOLS_DIR / "lkm_staging"

    @property
    def staging_dir(self) -> Path:
        return self._staging_dir

    @property
    def payload_files(self) -> List[str]:
        return ["init", "kernelsu.ko"]

    @property
    def root_type(self) -> str:
        return self.provider.provider_id

    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        utils.ui.echo(get_string("act_unroot_step4_lkm"))

    def _apply_source_selection(self, selection: StrategySourceSelection) -> None:
        self.repo_config = selection.repo_config
        self.source_label = selection.source_label
        self.is_nightly = selection.is_nightly
        self.workflow_id = selection.workflow_id
        self.is_tagged_build = selection.is_tagged_build

    def configure_source(self, breadcrumbs: Optional[str] = None) -> bool:
        self._apply_source_selection(
            select_lkm_source(self.provider.provider_id, breadcrumbs=breadcrumbs)
        )
        return True

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        return download_lkm_resources(
            profile=self.provider,
            staging_dir=self.staging_dir,
            repo_config=self.repo_config,
            kernel_version=kernel_version,
            is_nightly=self.is_nightly,
            workflow_id=self.workflow_id,
            is_tagged_build=self.is_tagged_build,
        )


def _prompt_custom_kernel_zip() -> Optional[Path]:
    """Prompt the user to place an AnyKernel3 zip in the kernel folder and select one."""
    kernel_dir = const.KERNEL_DIR
    kernel_dir.mkdir(parents=True, exist_ok=True)

    utils.ui.echo("")
    utils.ui.echo(get_string("gki_custom_place_zip").format(path=kernel_dir))

    while True:
        try:
            input(get_string("gki_custom_press_enter"))
        except (KeyboardInterrupt, EOFError):
            utils.ui.warn(get_string("gki_custom_cancelled"))
            return None

        zips = sorted(kernel_dir.glob("*.zip"))
        if not zips:
            utils.ui.warn(get_string("gki_custom_no_zip").format(path=kernel_dir))
            continue

        if len(zips) == 1:
            utils.ui.echo(
                get_string("gki_custom_selected").format(filename=zips[0].name)
            )
            return zips[0]

        # Multiple zips — let the user choose
        from ...menu import TerminalMenu

        menu = TerminalMenu(get_string("gki_custom_select_title"))
        zip_map: Dict[str, Path] = {}
        for i, zf in enumerate(zips, 1):
            key = str(i)
            zip_map[key] = zf
            menu.add_option(key, zf.name)
        menu.add_option("c", get_string("cancel"))

        choice = menu.ask(
            get_string("prompt_select"),
            get_string("err_invalid_selection"),
        )

        if choice == "c" or choice is None:
            utils.ui.warn(get_string("gki_custom_cancelled"))
            return None

        selected = zip_map.get(choice)
        if selected:
            utils.ui.echo(
                get_string("gki_custom_selected").format(filename=selected.name)
            )
            return selected


def _extract_manager_apk_from_zip(zip_path: Path) -> None:
    """Extract .apk from AnyKernel3 zip to TOOLS_DIR/manager.apk if present."""
    import zipfile

    try:
        with zipfile.ZipFile(zip_path, "r") as zf:
            apk_names = [n for n in zf.namelist() if n.lower().endswith(".apk")]
            if not apk_names:
                return
            apk_name = apk_names[0]
            dest = const.TOOLS_DIR / "manager.apk"
            with zf.open(apk_name) as src, open(dest, "wb") as dst:
                shutil.copyfileobj(src, dst)
            utils.ui.echo(get_string("gki_apk_found").format(filename=apk_name))
    except (zipfile.BadZipFile, OSError):
        pass


def get_root_strategy(gki: bool, root_type: str = "ksu") -> RootStrategy:
    provider = get_root_provider_profile(root_type)
    if provider.family == RootProviderFamily.APATCH:
        return APatchStrategy(provider.provider_id)
    if gki:
        return GkiRootStrategy()
    return LkmRootStrategy(provider.provider_id)
