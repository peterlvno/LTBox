import shutil
import zipfile
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from .. import constants as const
from .. import device, downloader, utils
from ..errors import ToolError
from ..i18n import get_string
from ..menu import TerminalMenu
from ..patch.avb import (
    process_boot_image_avb,
    rebuild_vbmeta_with_chained_images,
    vbmeta_has_chain_partition,
)
from ..patch.root import patch_boot_with_root_algo


NIGHTLY_WORKFLOW_FILES: Dict[str, str] = {
    "kernelsu": "build-manager.yml",
    "ksu": "build-manager.yml",
    "kernelsu-next": "build-manager-ci.yml",
    "sukisu": "build-manager.yml",
    "resukisu": "build-manager.yml",
    "apatch": "build.yml",
    "folkpatch": "build-debug.yml",
}


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


def _prompt_kpm_selection(kpm_files: List[Path]) -> List[Path]:
    selected: Set[int] = set()

    while True:
        utils.ui.clear()
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(f"   {get_string('apatch_kpm_select_title')}")
        utils.ui.echo("=" * width + "\n")

        for i, kpm_file in enumerate(kpm_files):
            mark = " [v]" if i in selected else ""
            utils.ui.echo(f"  {i + 1:3d}. {kpm_file.name}{mark}")

        utils.ui.echo("")
        utils.ui.echo(f"   a. {get_string('apatch_kpm_select_all')}")
        utils.ui.echo(f"   d. {get_string('apatch_kpm_deselect_all')}")
        utils.ui.echo(f"   f. {get_string('apatch_kpm_select_done')}")
        utils.ui.echo(f"   c. {get_string('cancel')}")
        utils.ui.echo("\n" + "=" * width + "\n")

        choice = utils.ui.prompt(get_string("prompt_select")).strip().lower()
        if choice == "f":
            return [kpm_files[i] for i in sorted(selected)]
        if choice == "c":
            return []
        if choice == "a":
            selected = set(range(len(kpm_files)))
            continue
        if choice == "d":
            selected.clear()
            continue

        try:
            idx = int(choice)
        except ValueError:
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        if not 1 <= idx <= len(kpm_files):
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        i = idx - 1
        if i in selected:
            selected.remove(i)
        else:
            selected.add(i)


def _prompt_nightly_workflow(
    root_name: str,
    repo: str,
    workflow_file: str,
    default_id: str,
    breadcrumbs: Optional[str] = None,
) -> str:
    menu = TerminalMenu(
        get_string("prompt_workflow_source_title"),
        breadcrumbs=breadcrumbs,
    )
    menu.add_option(
        "1",
        get_string("prompt_workflow_retrieve_latest").format(file=workflow_file),
    )
    menu.add_option("2", get_string("prompt_workflow_manual_input"))
    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))

    if choice == "1" and repo and workflow_file:
        utils.ui.clear()
        utils.ui.echo(get_string("prompt_workflow_retrieving"))
        try:
            run_id = downloader.get_latest_successful_workflow_run(repo, workflow_file)
            if run_id:
                utils.ui.info(get_string("prompt_workflow_retrieved").format(id=run_id))
                input(get_string("press_enter_to_continue"))
                return run_id
            else:
                utils.ui.error(get_string("prompt_workflow_retrieve_failed"))
        except Exception:
            utils.ui.error(get_string("prompt_workflow_retrieve_failed"))

    utils.ui.clear()
    display_id = default_id if default_id else get_string("act_root_auto_detect")
    width = utils.ui.get_term_width()
    utils.ui.echo("-" * width)
    utils.ui.echo(get_string("prompt_workflow_id").format(name=root_name))
    utils.ui.echo(get_string("prompt_workflow_default").format(id=display_id))
    utils.ui.echo("-" * width)
    val = input(get_string("prompt_input_arrow")).strip()
    if not val:
        return default_id
    return val


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
        self.root_type = root_type
        self.is_nightly = False
        self.workflow_id: Optional[str] = None
        self.repo_config: Dict[str, Any] = {}
        self._staging_dir = const.TOOLS_DIR / f"{self.root_type}_staging"

    @property
    def source_name(self) -> str:
        return "FolkPatch" if self.root_type == "folkpatch" else "APatch"

    def configure_source(self, breadcrumbs: Optional[str] = None) -> None:
        settings = const.load_settings_raw()
        self.repo_config = settings.get(self.root_type, {})

        bc = breadcrumbs

        menu = TerminalMenu(
            get_string("menu_root_subtype_title").format(name=self.source_name),
            breadcrumbs=bc,
        )
        menu.add_option("1", get_string("menu_root_subtype_release"))
        menu.add_option("2", get_string("menu_root_subtype_nightly"))
        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "2":
            self.is_nightly = True
            self.source_label = get_string("menu_root_subtype_nightly")
            self.workflow_id = _prompt_nightly_workflow(
                self.source_name,
                self.repo_config.get("repo", ""),
                NIGHTLY_WORKFLOW_FILES.get(self.root_type, ""),
                str(self.repo_config.get("workflow", "")).strip(),
                bc,
            )
        else:
            self.is_nightly = False
            self.source_label = get_string("menu_root_subtype_release")

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        _cleanup_manager_apk(show_message=False)
        utils.recreate_dir(self._staging_dir)
        try:
            if self.is_nightly and self.workflow_id:
                downloader.download_apatch_nightly(
                    self.workflow_id,
                    self._staging_dir,
                    repo=self.repo_config.get("repo", ""),
                    name=self.source_name,
                )
            else:
                downloader.download_apatch_release(
                    self._staging_dir,
                    repo=self.repo_config.get("repo", ""),
                    tag=self.repo_config.get("tag", "latest"),
                    name=self.source_name,
                )
            return True
        except (ToolError, OSError, zipfile.BadZipFile) as e:
            utils.ui.error(
                get_string("apatch_download_failed").format(e=e, name=self.source_name)
            )
            return False

    def patch(
        self,
        work_dir: Path,
        dev: Optional[device.DeviceController] = None,
        lkm_kernel_version: Optional[str] = None,
    ) -> Optional[Path]:
        magiskboot_exe = const.MAGISKBOOT_EXE

        utils.ui.clear()
        utils.ui.echo(
            "\n"
            + get_string("apatch_superkey_requirement").format(name=self.source_name)
        )
        superkey = ""
        while True:
            superkey = input(get_string("apatch_enter_superkey")).strip()
            if 8 <= len(superkey) <= 63 and superkey.isalnum():
                break
            utils.ui.error(get_string("apatch_superkey_invalid"))

        utils.ui.clear()
        kpm_paths: List[Path] = []

        choice = ""
        while choice not in ("y", "n"):
            choice = input(get_string("apatch_kpm_ask_embed")).strip().lower()

        if choice == "y":
            kpm_dir = work_dir / "kpm"
            kpm_dir.mkdir(exist_ok=True)

            utils.ui.clear()
            utils.ui.echo(
                get_string("apatch_kpm_folder_instruction").format(path=kpm_dir)
            )

            kpm_files: List[Path] = []
            try:
                while True:
                    input(get_string("press_enter_to_continue"))
                    kpm_files = sorted(kpm_dir.glob("*.kpm"))
                    if kpm_files:
                        break
                    utils.ui.error(
                        get_string("apatch_kpm_no_files_found").format(path=kpm_dir)
                    )
            except KeyboardInterrupt:
                utils.ui.echo("")
                kpm_files = []

            if kpm_files:
                selected = _prompt_kpm_selection(kpm_files)
                if selected:
                    kpm_paths = selected
                    utils.ui.clear()
                    utils.ui.echo(
                        get_string("apatch_kpm_embedding").format(count=len(kpm_paths))
                    )
                else:
                    utils.ui.echo(get_string("apatch_kpm_skipped"))
            else:
                utils.ui.echo(get_string("apatch_kpm_skipped"))
        else:
            utils.ui.echo(get_string("apatch_kpm_skipped"))

        utils.ui.clear()
        kpimg_src = self._staging_dir / "kpimg"
        if kpimg_src.exists():
            import shutil

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

    def configure_source(self, breadcrumbs: Optional[str] = None) -> None:
        settings = const.load_settings_raw()

        if self.root_type == "kernelsu":
            self.repo_config = settings.get("kernelsu", {})
            root_name = "KernelSU"
        elif self.root_type == "sukisu":
            self.repo_config = settings.get("sukisu-ultra", {})
            root_name = "SukiSU Ultra"
        elif self.root_type == "resukisu":
            self.repo_config = settings.get("resukisu", {})
            root_name = "ReSukiSU"
        else:
            self.repo_config = settings.get("kernelsu-next", {})
            root_name = "KernelSU Next"

        bc = breadcrumbs or get_string("menu_root_type_title")
        repo = self.repo_config.get("repo", "")
        workflow_file = NIGHTLY_WORKFLOW_FILES.get(self.root_type, "")

        if self.root_type == "resukisu":
            self.is_nightly = True
            self.is_tagged_build = False
            self.source_label = get_string("menu_root_subtype_nightly")
            self.workflow_id = _prompt_nightly_workflow(
                root_name,
                repo,
                workflow_file,
                str(self.repo_config.get("workflow", "")),
                bc,
            )
        else:
            menu = TerminalMenu(
                get_string("menu_root_subtype_title").format(name=root_name),
                breadcrumbs=bc,
            )
            menu.add_option("1", get_string("menu_root_subtype_release"))
            menu.add_option("2", get_string("menu_root_subtype_nightly"))

            choice = menu.ask(
                get_string("prompt_select"), get_string("err_invalid_selection")
            )

            if choice == "2":
                self.is_nightly = True
                self.is_tagged_build = False
                self.source_label = get_string("menu_root_subtype_nightly")
                self.workflow_id = _prompt_nightly_workflow(
                    root_name,
                    repo,
                    workflow_file,
                    str(self.repo_config.get("workflow", "")),
                    bc,
                )
            else:
                self.is_nightly = False
                self.is_tagged_build = True
                self.workflow_id = ""
                self.source_label = get_string("menu_root_subtype_release")

    def _download_nightly(
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

            manager_zip_path = temp_dl_dir / manager_zip
            apk_found = False
            if manager_zip_path.exists():
                with zipfile.ZipFile(manager_zip_path, "r") as zf:
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

        except (ToolError, zipfile.BadZipFile, OSError) as e:
            utils.ui.error(f"{e}")
            utils.ui.error(get_string("err_download_workflow"))
            return False

    def download_resources(self, kernel_version: Optional[str] = None) -> bool:
        _cleanup_manager_apk(show_message=False)

        repo = self.repo_config.get("repo", "")
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
                except (ToolError, ValueError) as e:
                    utils.ui.error(f"{e}")
                    utils.ui.error(get_string("err_download_workflow"))
                    return False

            return self._download_nightly(
                repo,
                workflow_id,
                manager,
                kernel_version,
                download_all_ksuinit=self.is_tagged_build,
            )
        else:
            utils.recreate_dir(self.staging_dir)

            tag = self.repo_config.get("tag", "latest")

            downloader.download_ksu_manager_release(const.TOOLS_DIR, repo=repo, tag=tag)
            downloader.download_ksuinit_release(
                self.staging_dir / "init", repo=repo, tag=tag
            )
            if kernel_version:
                downloader.get_lkm_kernel_release(
                    self.staging_dir / "kernelsu.ko", kernel_version, repo=repo, tag=tag
                )
            return True


def get_root_strategy(gki: bool, root_type: str = "ksu") -> RootStrategy:
    if root_type in ("folkpatch", "apatch"):
        return APatchStrategy(root_type)
    elif gki:
        return GkiRootStrategy()
    else:
        return LkmRootStrategy(root_type)


def _cleanup_manager_apk(show_message: bool = True):
    manager_apk = const.TOOLS_DIR / "manager.apk"
    if manager_apk.exists():
        if show_message:
            utils.ui.echo(get_string("act_cleanup_manager_apk"))
        try:
            manager_apk.unlink()
        except OSError:
            pass
