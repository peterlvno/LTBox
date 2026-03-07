import shutil
import subprocess
import zipfile
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

from .. import constants as const
from .. import device, downloader, utils
from ..errors import ToolError
from ..i18n import get_string
from ..menu import TerminalMenu
from ..partition import ensure_params_or_fail
from ..patch.avb import (
    process_boot_image_avb,
    rebuild_vbmeta_with_chained_images,
    vbmeta_has_chain_partition,
)
from ..patch.root import patch_boot_with_root_algo
from . import edl
from .system import detect_active_slot_robust


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
        if const.FN_VBMETA in self.required_files:
            files["vbmeta"] = self.backup_dir / const.FN_VBMETA
        return files

    @property
    def is_unroot_available(self) -> bool:
        return all(p.exists() for p in self.unroot_files.values())

    def get_partition_map(self, suffix: str) -> Dict[str, str]:
        partition_map = {"main": f"{self.spec.main_partition}{suffix}", "vbmeta": ""}
        if const.FN_VBMETA in self.required_files:
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

        kernel_ver_arg = lkm_kernel_version if self.root_type != "magisk" else None

        return patch_boot_with_root_algo(
            work_dir,
            magiskboot_exe,
            dev,
            gki=False,
            lkm_kernel_version=kernel_ver_arg,
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

    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        utils.ui.echo(
            get_string("act_unroot_step4_gki").format(part=partition_map["main"])
        )

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        downloader.download_ksu_manager_release(const.TOOLS_DIR)
        return True

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        magiskboot_exe = const.MAGISKBOOT_EXE

        return patch_boot_with_root_algo(work_dir, magiskboot_exe, dev=None, gki=True)

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


class FolkPatchStrategy(GkiRootStrategy):
    spec = RootStrategySpec(
        image_name=const.FN_BOOT,
        backup_name=const.FN_BOOT_BAK,
        output_dir=const.OUTPUT_ROOT_DIR,
        backup_dir=const.BACKUP_BOOT_DIR,
        required_files=[const.FN_BOOT, const.FN_VBMETA],
        main_partition="boot",
        display_name="FolkPatch",
        unroot_detect_msg_key="act_unroot_gki_detected",
        unroot_menu_msg_key="act_unroot_menu_3_gki",
        menu_shortcut="5",
        patch_image_name="boot.img",
        requires_kernel_version=False,
    )

    def __init__(self):
        super().__init__()
        self.is_nightly = False
        self.workflow_id = None
        self._staging_dir = const.TOOLS_DIR / "folkpatch_staging"

    def configure_source(self) -> None:
        menu = TerminalMenu(
            get_string("folkpatch_menu_version_title"),
            breadcrumbs=get_string("folkpatch_menu_breadcrumbs"),
        )
        menu.add_option("1", get_string("menu_root_subtype_release"))
        menu.add_option("2", get_string("menu_root_subtype_nightly"))
        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "2":
            self.is_nightly = True
            utils.ui.clear()
            width = utils.ui.get_term_width()
            utils.ui.echo("-" * width)
            utils.ui.echo(get_string("folkpatch_prompt_workflow_id"))
            utils.ui.echo("-" * width)
            self.workflow_id = input(get_string("prompt_input_arrow")).strip()
        else:
            self.is_nightly = False

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        _cleanup_manager_apk(show_message=False)
        utils.recreate_dir(self._staging_dir)
        try:
            downloader.download_kptools(const.DOWNLOAD_DIR)
            if self.is_nightly and self.workflow_id:
                downloader.download_folkpatch_nightly(
                    self.workflow_id, self._staging_dir
                )
            else:
                downloader.download_folkpatch_release(self._staging_dir)
            return True
        except Exception as e:
            utils.ui.error(get_string("folkpatch_download_failed").format(e=e))
            return False

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        magiskboot_exe = const.MAGISKBOOT_EXE

        utils.ui.echo("\n" + get_string("folkpatch_superkey_requirement"))
        superkey = ""
        while True:
            superkey = input(get_string("folkpatch_enter_superkey")).strip()
            if 8 <= len(superkey) <= 63 and superkey.isalnum():
                break
            utils.ui.error(get_string("folkpatch_superkey_invalid"))

        kpimg_src = self._staging_dir / "kpimg"
        if kpimg_src.exists():
            import shutil

            shutil.copy(kpimg_src, work_dir / "kpimg")
        else:
            utils.ui.error(get_string("folkpatch_kpimg_missing"))
            return None

        return patch_boot_with_root_algo(
            work_dir,
            magiskboot_exe,
            dev=dev,
            gki=True,
            root_type="folkpatch",
            superkey=superkey,
        )


class MagiskRootStrategy(InitBootRootStrategy):
    spec = RootStrategySpec(
        image_name=const.FN_INIT_BOOT,
        backup_name=const.FN_INIT_BOOT_BAK,
        output_dir=const.OUTPUT_ROOT_MAGISK_DIR,
        backup_dir=const.BACKUP_MAGISK_DIR,
        required_files=[const.FN_INIT_BOOT, const.FN_VBMETA],
        main_partition="init_boot",
        display_name="Magisk",
        unroot_detect_msg_key="act_unroot_magisk_detected",
        unroot_menu_msg_key="act_unroot_menu_1_magisk",
        menu_shortcut="1",
        patch_image_name="init_boot.img",
    )

    def __init__(self) -> None:
        self._staging_dir = const.TOOLS_DIR / "magisk_staging"

    @property
    def staging_dir(self) -> Path:
        return self._staging_dir

    @property
    def payload_files(self) -> List[str]:
        return ["magiskinit", "magisk", "init-ld", "stub.apk"]

    @property
    def root_type(self) -> str:
        return "magisk"

    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        utils.ui.echo(get_string("act_unroot_step4_lkm"))

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        _cleanup_manager_apk(show_message=False)

        utils.recreate_dir(self.staging_dir)

        try:
            apk_path = downloader.download_magisk_apk(self.staging_dir)
            downloader.extract_magisk_libs(apk_path, self.staging_dir)
        except Exception as e:
            utils.ui.error(str(e))
            return False

        manager_path = const.TOOLS_DIR / "manager.apk"
        if manager_path.exists():
            manager_path.unlink()
        shutil.copy(apk_path, manager_path)
        return True


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
        self._root_type = root_type
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
        return self._root_type

    def print_unroot_step(self, partition_map: Dict[str, str]) -> None:
        utils.ui.echo(get_string("act_unroot_step4_lkm"))

    def _get_mapped_kernel_name(self, kernel_version: str) -> Optional[str]:
        if not kernel_version:
            return None
        major_minor = ".".join(kernel_version.split(".")[:2])
        mapping = {
            "5.10": "android12-5.10",
            "5.15": "android13-5.15",
            "6.1": "android14-6.1",
            "6.6": "android15-6.6",
            "6.12": "android16-6.12",
        }
        return mapping.get(major_minor)

    def _prompt_workflow(self, root_name: str, default_id: str) -> str:
        utils.ui.clear()
        msg_enter = get_string("prompt_workflow_id").replace("{name}", root_name)

        display_id = default_id if default_id else get_string("act_root_auto_detect")
        msg_default = get_string("prompt_workflow_default").replace("{id}", display_id)

        width = utils.ui.get_term_width()
        utils.ui.echo("-" * width)
        utils.ui.echo(msg_enter)
        utils.ui.echo(msg_default)
        utils.ui.echo("-" * width)

        val = input(get_string("prompt_input_arrow")).strip()
        if not val:
            return default_id
        return val

    def configure_source(self) -> None:
        settings = const.load_settings_raw()

        if self.root_type == "sukisu":
            self.repo_config = settings.get("sukisu-ultra", {})
            root_name = "SukiSU Ultra"
        elif self.root_type == "resukisu":
            self.repo_config = settings.get("resukisu", {})
            root_name = "ReSukiSU"
        else:
            self.repo_config = settings.get("kernelsu-next", {})
            root_name = "KernelSU Next"

        if self.root_type == "resukisu":
            self.is_nightly = True
            self.is_tagged_build = False
            self.workflow_id = self._prompt_workflow(
                root_name, str(self.repo_config.get("workflow", ""))
            )
        else:
            menu = TerminalMenu(
                get_string("menu_root_subtype_title"),
                breadcrumbs=get_string("menu_root_type_title"),
            )
            menu.add_option("1", get_string("menu_root_subtype_release"))
            menu.add_option("2", get_string("menu_root_subtype_nightly"))

            choice = menu.ask(
                get_string("prompt_select"), get_string("err_invalid_selection")
            )

            if choice == "2":
                self.is_nightly = True
                self.is_tagged_build = False
                self.workflow_id = self._prompt_workflow(
                    root_name, str(self.repo_config.get("workflow", ""))
                )
            else:
                self.is_nightly = False
                self.is_tagged_build = True
                self.workflow_id = ""

    def _perform_nightly_download(
        self,
        repo,
        workflow_id,
        manager_zip,
        kernel_version,
        download_all_ksuinit: bool = False,
    ) -> bool:
        mapped_name = self._get_mapped_kernel_name(kernel_version)
        if not mapped_name:
            utils.ui.error(
                get_string("err_sukisu_kernel_map_not_found").format(ver=kernel_version)
            )
            return False

        try:
            temp_dl_dir = const.TOOLS_DIR / "dl_temp"
            utils.recreate_dir(temp_dl_dir)

            downloader.download_nightly_artifacts(
                repo=repo,
                workflow_id=workflow_id,
                manager_name=manager_zip,
                mapped_name=mapped_name,
                target_dir=temp_dl_dir,
                download_all_ksuinit=download_all_ksuinit,
                manager_fallback_names=self.repo_config.get("manager_fallbacks"),
            )

            mgr_zip_path = temp_dl_dir / manager_zip
            apk_found = False
            if mgr_zip_path.exists():
                with zipfile.ZipFile(mgr_zip_path, "r") as zf:
                    for name in zf.namelist():
                        if name.endswith(".apk"):
                            with (
                                zf.open(name) as src,
                                open(const.TOOLS_DIR / "manager.apk", "wb") as dst,
                            ):
                                shutil.copyfileobj(src, dst)
                            apk_found = True
                            break

            if not apk_found:
                raise ToolError(get_string("act_err_manager_apk_not_found_zip"))

            utils.recreate_dir(self.staging_dir)

            lkm_zip = temp_dl_dir / "lkm.zip"
            ko_found = False
            if lkm_zip.exists():
                with zipfile.ZipFile(lkm_zip, "r") as zf:
                    for name in zf.namelist():
                        if name.endswith("kernelsu.ko"):
                            with (
                                zf.open(name) as src,
                                open(self.staging_dir / "kernelsu.ko", "wb") as dst,
                            ):
                                shutil.copyfileobj(src, dst)
                            ko_found = True
                            break

            if not ko_found:
                raise ToolError(get_string("act_err_kernelsu_ko_not_found_zip"))

            if (temp_dl_dir / "ksuinit").exists():
                shutil.copy(temp_dl_dir / "ksuinit", self.staging_dir / "init")

            shutil.rmtree(temp_dl_dir)
            return True

        except Exception as e:
            utils.ui.error(f"{e}")
            utils.ui.error(get_string("err_download_workflow"))
            return False

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        _cleanup_manager_apk(show_message=False)

        repo = self.repo_config.get("repo")
        manager = self.repo_config.get("manager")

        if self.root_type in ("sukisu", "resukisu") or self.is_nightly:
            if self.is_nightly and self.workflow_id:
                workflow_id = self.workflow_id
            else:
                tag = self.repo_config.get("tag")
                try:
                    if not repo:
                        raise ToolError(get_string("err_download_workflow"))
                    workflow_id, resolved_tag = (
                        downloader.get_latest_tagged_workflow_run(repo, tag)
                    )
                    utils.ui.info(
                        get_string("act_using_tagged_run").format(
                            tag=resolved_tag, id=workflow_id
                        )
                    )
                except Exception as e:
                    utils.ui.error(f"{e}")
                    utils.ui.error(get_string("err_download_workflow"))
                    return False

            return self._perform_nightly_download(
                repo,
                workflow_id,
                manager,
                kernel_version,
                download_all_ksuinit=self.is_tagged_build,
            )
        else:
            utils.recreate_dir(self.staging_dir)

            downloader.download_ksu_manager_release(const.TOOLS_DIR)
            downloader.download_ksuinit_release(self.staging_dir / "init")
            if kernel_version:
                downloader.get_lkm_kernel_release(
                    self.staging_dir / "kernelsu.ko", kernel_version
                )
            return True


def get_root_strategy(gki: bool, root_type: str = "ksu") -> RootStrategy:
    if root_type == "folkpatch":
        return FolkPatchStrategy()
    elif gki:
        return GkiRootStrategy()
    elif root_type == "magisk":
        return MagiskRootStrategy()
    else:
        return LkmRootStrategy(root_type)


def _patch_root_image_from_image_folder(
    strategy: RootStrategy,
    gki: bool,
    dev: Optional[device.DeviceController] = None,
    lkm_kernel_version: Optional[str] = None,
    show_manual_flash_notice: bool = True,
) -> bool:
    utils.check_dependencies()
    wait_image = strategy.image_name
    utils.ui.echo(get_string("act_wait_image").format(image=wait_image))
    const.IMAGE_DIR.mkdir(exist_ok=True)

    requires_vbmeta = const.FN_VBMETA in strategy.required_files

    prompt = get_string("act_prompt_boot").format(name=const.IMAGE_DIR.name)
    if requires_vbmeta:
        prompt = prompt.replace(
            f"'{const.FN_BOOT}'", f"'{strategy.image_name}' and '{const.FN_VBMETA}'"
        )

    utils.wait_for_files(const.IMAGE_DIR, strategy.required_files, prompt)

    for fname in strategy.required_files:
        src = const.IMAGE_DIR / fname
        dst = const.BASE_DIR / fname
        try:
            shutil.copy(src, dst)
            utils.ui.echo(get_string("act_copy_boot").format(name=src.name))
        except (IOError, OSError) as e:
            utils.ui.error(get_string("act_err_copy_boot").format(name=src.name, e=e))
            raise ToolError(get_string("act_err_copy_boot").format(name=src.name, e=e))

    if not (const.BASE_DIR / strategy.image_name).exists():
        msg = get_string("act_err_image_missing").format(image=strategy.image_name)
        utils.ui.echo(msg)
        raise ToolError(msg)

    utils.ui.echo(get_string("act_backup_boot"))
    shutil.copy(
        const.BASE_DIR / strategy.image_name, const.BASE_DIR / strategy.backup_name
    )
    if requires_vbmeta:
        shutil.copy(
            const.BASE_DIR / const.FN_VBMETA, const.BASE_DIR / const.FN_VBMETA_BAK
        )

    patched_boot_path = None
    with utils.temporary_workspace(const.WORK_DIR):
        shutil.copy(
            const.BASE_DIR / strategy.image_name, const.WORK_DIR / strategy.image_name
        )
        (const.BASE_DIR / strategy.image_name).unlink()

        if requires_vbmeta:
            (const.BASE_DIR / const.FN_VBMETA).unlink()

        if isinstance(strategy, LkmRootStrategy) and not lkm_kernel_version:
            utils.ui.echo(get_string("err_req_kernel_ver_lkm"))
            lkm_kernel_version = input(
                get_string("prompt_enter_kernel_version")
            ).strip()
            if not lkm_kernel_version:
                utils.ui.error(get_string("err_kernel_version_req"))
                return False

        if not strategy.download_resources(lkm_kernel_version):
            return False

        patched_boot_path = strategy.patch(
            const.WORK_DIR, dev=dev, lkm_kernel_version=lkm_kernel_version
        )

    if patched_boot_path and patched_boot_path.exists():
        utils.ui.echo(get_string("act_finalize_root"))

        strategy.finalize_patch(patched_boot_path, strategy.output_dir, const.BASE_DIR)
        utils.ui.echo("")

        utils.ui.echo(
            get_string("act_move_root_backup").format(dir=const.BACKUP_DIR.name)
        )
        const.BACKUP_DIR.mkdir(exist_ok=True)
        for bak_file in const.BASE_DIR.glob("*.bak.img"):
            shutil.move(bak_file, const.BACKUP_DIR / bak_file.name)
        utils.ui.echo("")

        width = utils.ui.get_term_width()
        utils.ui.echo("  " + "=" * width)
        utils.ui.echo(get_string("act_success"))

        utils.ui.echo(
            get_string("act_root_saved_file").format(
                name=strategy.image_name, dir=strategy.log_output_dir_name
            )
        )
        if (strategy.output_dir / const.FN_VBMETA).exists():
            utils.ui.echo(
                get_string("act_root_saved_file").format(
                    name=const.FN_VBMETA, dir=strategy.log_output_dir_name
                )
            )

        if show_manual_flash_notice:
            utils.ui.echo("\n" + get_string("act_root_manual_flash_notice"))
        utils.ui.echo("  " + "=" * width)
        return True
    else:
        fail_image = "boot" if gki else "init_boot"
        utils.ui.error(get_string("act_err_root_fail_image").format(image=fail_image))
        return False


def patch_root_image_file(gki: bool = False, root_type: str = "ksu") -> None:
    strategy = get_root_strategy(gki, root_type)

    if hasattr(strategy, "configure_source"):
        strategy.configure_source()

    utils.ui.echo(get_string("act_clean_dir").format(dir=strategy.log_output_dir_name))
    utils.recreate_dir(strategy.output_dir)
    utils.ui.echo("")

    _patch_root_image_from_image_folder(strategy, gki)


def patch_root_image_file_and_flash(
    dev: device.DeviceController, gki: bool = False, root_type: str = "ksu"
) -> None:
    strategy = get_root_strategy(gki, root_type)

    _cleanup_manager_apk()

    if hasattr(strategy, "configure_source"):
        strategy.configure_source()

    utils.ui.echo(get_string("act_clean_dir").format(dir=strategy.log_output_dir_name))
    utils.recreate_dir(strategy.output_dir)
    utils.ui.echo("")

    if not dev.skip_adb:
        dev.adb.wait_for_device()

    lkm_kernel_version = _get_lkm_kernel_version(dev, strategy)

    if not _patch_root_image_from_image_folder(
        strategy,
        gki,
        dev=dev,
        lkm_kernel_version=lkm_kernel_version,
        show_manual_flash_notice=False,
    ):
        return

    confirm = (
        utils.ui.prompt(get_string("prompt_flash_image_folder_confirm")).strip().lower()
    )
    if confirm != "y":
        return

    edl.ensure_edl_requirements()

    active_slot = detect_active_slot_robust(dev)
    suffix = active_slot if active_slot else ""
    partition_map = strategy.get_partition_map(suffix)

    if active_slot:
        utils.ui.echo(get_string("act_active_slot").format(slot=active_slot))
    else:
        utils.ui.echo(get_string("act_warn_root_slot"))
        if gki:
            partition_map["main"] = "boot"
            if const.FN_VBMETA in strategy.required_files:
                partition_map["vbmeta"] = "vbmeta"
        else:
            partition_map["main"] = "init_boot"
            partition_map["vbmeta"] = "vbmeta"

    _flash_root_image(dev, strategy, partition_map, gki)


def _prepare_root_env(strategy: RootStrategy):
    utils.ui.echo(get_string("act_start_root"))

    utils.recreate_dir(strategy.output_dir)
    strategy.backup_dir.mkdir(exist_ok=True)

    utils.check_dependencies()
    edl.ensure_edl_requirements()
    if not const.MAGISKBOOT_EXE.exists():
        raise ToolError(
            get_string("dl_tool_not_found").format(tool_name="magiskboot.exe")
        )


def _get_lkm_kernel_version(
    dev: device.DeviceController, strategy: RootStrategy
) -> Optional[str]:
    if strategy.requires_kernel_version:
        if not dev.skip_adb:
            try:
                return dev.adb.get_kernel_version()
            except Exception as e:
                utils.ui.error(get_string("act_root_warn_lkm_kver_fail").format(e=e))
                utils.ui.error(get_string("act_root_warn_lkm_kver_retry"))
        else:
            utils.ui.error(get_string("act_root_err_lkm_skip_adb"))
            raise ToolError(get_string("act_root_err_lkm_skip_adb_exc"))
    return None


def _dump_partition_to_workspace(
    dev: device.DeviceController, port: str, label: str, output_path: Path
):
    params = ensure_params_or_fail(label)
    utils.ui.echo(
        get_string("act_found_dump_info").format(
            xml=params["source_xml"], lun=params["lun"], start=params["start_sector"]
        )
    )
    dev.edl.read_partition(
        port=port,
        output_filename=str(output_path),
        lun=params["lun"],
        start_sector=params["start_sector"],
        num_sectors=params["num_sectors"],
    )
    if params.get("size_in_kb"):
        expected = int(float(params["size_in_kb"]) * 1024)
        actual = output_path.stat().st_size
        if expected != actual:
            raise RuntimeError(
                get_string("act_err_dump_mismatch").format(
                    part=label, expected=expected, actual=actual
                )
            )


def _dump_and_generate_root_image(
    dev: device.DeviceController,
    strategy: RootStrategy,
    partition_map: Dict[str, str],
    gki: bool,
    lkm_kernel_version: Optional[str],
) -> Path:

    main_partition = partition_map["main"]
    step3_suffix = "" if gki else " (init_boot)"
    utils.ui.echo(
        get_string("act_root_step3_dump").format(
            part=main_partition, suffix=step3_suffix
        )
    )

    with utils.temporary_workspace(const.WORKING_BOOT_DIR):
        dumped_main = const.WORKING_BOOT_DIR / strategy.image_name
        backup_main = strategy.backup_dir / strategy.image_name
        base_main_bak = const.BASE_DIR / strategy.backup_name

        with dev.edl_session(auto_reset=True, reset_msg_key="act_dump_reset") as port:
            try:
                _dump_partition_to_workspace(dev, port, main_partition, dumped_main)

                if const.FN_VBMETA in strategy.required_files:
                    vbmeta_partition = partition_map["vbmeta"]
                    dumped_vbmeta = const.WORKING_BOOT_DIR / const.FN_VBMETA
                    _dump_partition_to_workspace(
                        dev, port, vbmeta_partition, dumped_vbmeta
                    )

                read_ok_suffix = "" if gki else " (init_boot)"
                utils.ui.echo(
                    get_string("act_read_dump_ok").format(
                        part=main_partition, suffix=read_ok_suffix, file=dumped_main
                    )
                )

            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(
                    get_string("act_err_dump").format(part=main_partition, e=e)
                )
                raise

            utils.ui.echo(
                get_string("act_backup_boot_root").format(dir=strategy.backup_dir.name)
            )
            shutil.copy(dumped_main, backup_main)
            utils.ui.echo(get_string("act_temp_backup_avb"))
            shutil.copy(dumped_main, base_main_bak)

            if const.FN_VBMETA in strategy.required_files:
                shutil.copy(
                    const.WORKING_BOOT_DIR / const.FN_VBMETA,
                    strategy.backup_dir / const.FN_VBMETA,
                )
                shutil.copy(
                    const.WORKING_BOOT_DIR / const.FN_VBMETA,
                    const.BASE_DIR / const.FN_VBMETA_BAK,
                )

            utils.ui.echo(get_string("act_backups_done"))

        utils.ui.echo(
            get_string("act_root_step4_patch").format(image=strategy.patch_image_name)
        )

        try:
            patched_boot_path = strategy.patch(
                const.WORKING_BOOT_DIR, dev, lkm_kernel_version
            )
            if not (patched_boot_path and patched_boot_path.exists()):
                fail_image = "boot" if gki else "init_boot"
                raise ToolError(
                    get_string("act_err_root_fail_image").format(image=fail_image)
                )

            utils.ui.echo(get_string("act_root_step5"))
            final_boot = strategy.finalize_patch(
                patched_boot_path, strategy.output_dir, const.BASE_DIR
            )
            utils.ui.echo(
                get_string("act_patched_boot_saved").format(dir=final_boot.parent.name)
            )
        except Exception as e:
            if isinstance(e, ToolError):
                utils.ui.error(str(e))
            else:
                utils.ui.error(get_string("act_err_avb_footer").format(e=e))
            base_main_bak.unlink(missing_ok=True)
            if const.FN_VBMETA in strategy.required_files:
                (const.BASE_DIR / const.FN_VBMETA_BAK).unlink(missing_ok=True)
            raise

        base_main_bak.unlink(missing_ok=True)
        if const.FN_VBMETA in strategy.required_files:
            (const.BASE_DIR / const.FN_VBMETA_BAK).unlink(missing_ok=True)

        return strategy.output_dir / strategy.image_name


def _flash_root_image(
    dev: device.DeviceController,
    strategy: RootStrategy,
    partition_map: Dict[str, str],
    gki: bool,
):
    main_partition = partition_map["main"]
    flash_image = "boot.img" if gki else "init_boot.img"
    utils.ui.echo(
        get_string("act_root_step6_flash").format(
            image=flash_image, part=main_partition
        )
    )

    if not dev.skip_adb:
        utils.ui.echo(get_string("act_wait_sys_adb"))
        dev.adb.wait_for_device()
        utils.ui.echo(get_string("act_reboot_edl_flash"))
    else:
        utils.ui.echo(get_string("act_skip_adb_on"))
        utils.ui.echo(get_string("act_manual_edl_now"))

    with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
        try:
            final_boot_path = strategy.output_dir / strategy.image_name
            edl.flash_partition_target(dev, port, main_partition, final_boot_path)

            utils.ui.echo(
                get_string("act_flash_img").format(
                    filename=strategy.image_name, part=main_partition
                )
            )

            final_vbmeta_path = strategy.output_dir / const.FN_VBMETA
            if final_vbmeta_path.exists() and partition_map.get("vbmeta"):
                vbmeta_part = partition_map["vbmeta"]
                edl.flash_partition_target(dev, port, vbmeta_part, final_vbmeta_path)
                utils.ui.echo(
                    get_string("act_flash_img").format(
                        filename=const.FN_VBMETA, part=vbmeta_part
                    )
                )
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            utils.ui.error(get_string("act_err_edl_write").format(e=e))
            raise


def root_device(
    dev: device.DeviceController, gki: bool = False, root_type: str = "ksu"
) -> None:
    strategy = get_root_strategy(gki, root_type)

    _cleanup_manager_apk()

    if hasattr(strategy, "configure_source"):
        strategy.configure_source()

    _prepare_root_env(strategy)

    utils.ui.echo(get_string("act_root_step1"))
    if not dev.skip_adb:
        dev.adb.wait_for_device()

    lkm_kernel_version = _get_lkm_kernel_version(dev, strategy)

    if not strategy.download_resources(lkm_kernel_version):
        utils.ui.error(get_string("err_download_resources_abort"))
        return

    _install_manager_apk(dev)

    active_slot = detect_active_slot_robust(dev)
    suffix = active_slot if active_slot else ""

    partition_map = strategy.get_partition_map(suffix)

    if active_slot:
        utils.ui.echo(get_string("act_active_slot").format(slot=active_slot))
    else:
        utils.ui.echo(get_string("act_warn_root_slot"))
        if gki:
            partition_map["main"] = "boot"
            if const.FN_VBMETA in strategy.required_files:
                partition_map["vbmeta"] = "vbmeta"
        else:
            partition_map["main"] = "init_boot"
            partition_map["vbmeta"] = "vbmeta"

    utils.ui.echo(get_string("act_root_step2"))

    _dump_and_generate_root_image(dev, strategy, partition_map, gki, lkm_kernel_version)

    _flash_root_image(dev, strategy, partition_map, gki)

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "!" * width)
    utils.ui.error(get_string("act_root_warn_brick"))
    utils.ui.echo("!" * width + "\n")
    utils.ui.echo(get_string("act_root_finish"))


def unroot_device(dev: device.DeviceController) -> None:
    utils.ui.echo(get_string("act_start_unroot"))

    strategies: List[RootStrategy] = [
        MagiskRootStrategy(),
        LkmRootStrategy(),
        GkiRootStrategy(),
    ]
    available_strategies = [s for s in strategies if s.is_unroot_available]

    selected_strategy: Optional[RootStrategy] = None

    if len(available_strategies) > 1:
        menu = TerminalMenu(
            get_string("act_unroot_menu_title"),
            breadcrumbs=get_string("menu_main_title"),
        )
        for s in available_strategies:
            menu.add_option(s.menu_shortcut, get_string(s.unroot_menu_msg_key))

        menu.add_separator()
        menu.add_option("m", get_string("menu_root_m"))

        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "m":
            utils.ui.echo(get_string("act_op_cancel"))
            return

        for s in available_strategies:
            if choice == s.menu_shortcut:
                selected_strategy = s
                break
        utils.ui.clear()

    elif len(available_strategies) == 1:
        selected_strategy = available_strategies[0]
        utils.ui.echo(get_string(selected_strategy.unroot_detect_msg_key))
    else:
        prompt = get_string("act_unroot_prompt_all").format(
            magisk_dir=MagiskRootStrategy().backup_dir.name,
            lkm_dir=LkmRootStrategy().backup_dir.name,
            gki_dir=GkiRootStrategy().backup_dir.name,
        )

        def check_for_unroot_files(p: Path, f: Optional[list]) -> bool:
            return any(s.is_unroot_available for s in strategies)

        utils._wait_for_resource(const.BASE_DIR, check_for_unroot_files, prompt, None)

        for s in strategies:
            if s.is_unroot_available:
                selected_strategy = s
                utils.ui.echo(get_string(selected_strategy.unroot_detect_msg_key))
                break

    utils.ui.echo(get_string("act_unroot_step1"))
    edl.ensure_edl_requirements()
    utils.ui.echo(get_string("act_unroot_step3"))

    if not dev.skip_adb:
        dev.adb.wait_for_device()

    active_slot = detect_active_slot_robust(dev)
    suffix = active_slot if active_slot else ""

    if selected_strategy:
        with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
            try:
                partition_map = selected_strategy.get_partition_map(suffix)
                selected_strategy.print_unroot_step(partition_map)

                for role, backup_path in selected_strategy.unroot_files.items():
                    target_part = partition_map[role]
                    edl.flash_partition_target(dev, port, target_part, backup_path)
                    utils.ui.echo(
                        get_string("act_flash_img").format(
                            filename=backup_path.name, part=target_part
                        )
                    )
            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(get_string("act_err_edl_write").format(e=e))
                raise

    utils.ui.echo(get_string("act_unroot_finish"))


def sign_and_flash_twrp(dev: device.DeviceController) -> None:
    utils.ui.echo(get_string("act_start_rec_flash"))

    twrp_name = const.FN_TWRP
    out_dir = const.OUTPUT_TWRP_DIR

    utils.recreate_dir(out_dir)

    utils.check_dependencies()
    edl.ensure_edl_requirements()

    utils.ui.echo(get_string("act_wait_image"))
    prompt = get_string("act_prompt_twrp").format(dir=const.IMAGE_DIR.name)
    utils.wait_for_files(const.IMAGE_DIR, [twrp_name], prompt)

    twrp_src = const.IMAGE_DIR / twrp_name

    utils.ui.echo(get_string("act_root_step1"))
    if not dev.skip_adb:
        dev.adb.wait_for_device()

    active_slot = detect_active_slot_robust(dev)
    suffix = active_slot if active_slot else ""
    target_partition = f"recovery{suffix}"

    utils.ui.echo(get_string("act_root_step2"))

    with utils.temporary_workspace(const.WORK_DIR):
        dumped_recovery = const.WORK_DIR / f"recovery{suffix}.img"

        with dev.edl_session(auto_reset=True, reset_msg_key="act_dump_reset") as port:
            utils.ui.echo(get_string("act_dump_recovery").format(part=target_partition))
            try:
                params = ensure_params_or_fail(target_partition)
                dev.edl.read_partition(
                    port=port,
                    output_filename=str(dumped_recovery),
                    lun=params["lun"],
                    start_sector=params["start_sector"],
                    num_sectors=params["num_sectors"],
                )
            except Exception as e:
                utils.ui.error(
                    get_string("act_err_dump").format(part=target_partition, e=e)
                )
                raise

            backup_recovery = const.BACKUP_DIR / f"recovery{suffix}.img"
            const.BACKUP_DIR.mkdir(exist_ok=True)
            shutil.copy(dumped_recovery, backup_recovery)
            utils.ui.echo(get_string("act_backup_recovery_ok"))

        utils.ui.echo(get_string("act_sign_twrp_start"))

        from ..patch.avb import _apply_hash_footer, extract_image_avb_info

        rec_info = extract_image_avb_info(dumped_recovery)

        pubkey = rec_info.get("pubkey_sha1")
        key_file = const.KEY_MAP.get(str(pubkey))

        if not key_file:
            utils.ui.error(get_string("img_err_boot_key_mismatch").format(key=pubkey))
            raise KeyError(f"Unknown key: {pubkey}")

        final_twrp = out_dir / twrp_name
        shutil.copy(twrp_src, final_twrp)

        subprocess.run(
            [
                str(const.PYTHON_EXE),
                str(const.AVBTOOL_PY),
                "erase_footer",
                "--image",
                str(final_twrp),
            ],
            capture_output=True,
        )

        _apply_hash_footer(
            image_path=final_twrp, image_info=rec_info, key_file=key_file
        )
        utils.ui.echo(get_string("act_sign_twrp_ok"))

        utils.ui.echo(get_string("act_reboot_edl_flash"))
        if not dev.skip_adb:
            dev.adb.wait_for_device()
        else:
            utils.ui.echo(get_string("act_manual_edl_now"))

        with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
            edl.flash_partition_target(dev, port, target_partition, final_twrp)

            utils.ui.echo(
                get_string("act_flash_img").format(
                    filename=twrp_name, part=target_partition
                )
            )

    utils.ui.echo(get_string("act_success"))


def _cleanup_manager_apk(show_message: bool = True):
    manager_apk = const.TOOLS_DIR / "manager.apk"
    if manager_apk.exists():
        if show_message:
            utils.ui.echo(get_string("act_cleanup_manager_apk"))
        try:
            manager_apk.unlink()
        except OSError:
            pass


def _install_manager_apk(dev: device.DeviceController):
    manager_apk = const.TOOLS_DIR / "manager.apk"

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "-" * width)
    utils.ui.echo(get_string("act_install_ksu").format(name="Manager App"))

    if not manager_apk.exists():
        utils.ui.error(get_string("act_manager_apk_not_found"))
        return

    if dev.skip_adb:
        utils.ui.echo(get_string("act_adb_skipped_manual_install"))
        utils.ui.echo(get_string("act_file_location").format(path=manager_apk))
        return

    utils.ui.echo(get_string("act_wait_sys_adb"))
    try:
        dev.adb.wait_for_device()
        dev.adb.install(manager_apk)
        utils.ui.echo(get_string("act_ksu_ok"))
    except Exception as e:
        utils.ui.error(get_string("act_err_ksu").format(e=e))
    utils.ui.echo("-" * width + "\n")
