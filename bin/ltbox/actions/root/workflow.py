from dataclasses import dataclass
import shutil
import subprocess
import textwrap
from pathlib import Path
from typing import Dict, List, Optional

from ... import constants as const
from ... import device, utils
from ...errors import DeviceCommandError, DeviceConnectionError, ToolError
from ...i18n import get_string
from ...menus.terminal import TerminalMenu
from ...part.partition import require_partition_params
from ...part.service import EdlPartitionService
from .. import edl
from .downloads import cleanup_manager_apk
from .strategies import (
    GkiRootStrategy,
    LkmRootStrategy,
    RootStrategy,
    get_root_strategy,
)
from ..system import get_slot_suffix


def _partition_service() -> EdlPartitionService:
    return EdlPartitionService(resolve_params=require_partition_params)


@dataclass(frozen=True)
class RootWorkflowSession:
    strategy: RootStrategy
    gki: bool
    lkm_kernel_version: Optional[str]

    def resolve_partition_map(self, dev: device.DeviceController) -> Dict[str, str]:
        suffix = get_slot_suffix(dev)
        partition_map = self.strategy.get_partition_map(suffix)

        if suffix:
            utils.ui.echo(get_string("act_active_slot").format(slot=suffix))
            return partition_map

        utils.ui.echo(get_string("act_warn_root_slot"))
        if self.gki:
            partition_map["main"] = "boot"
            if self.strategy.requires_vbmeta:
                partition_map["vbmeta"] = "vbmeta"
        else:
            partition_map["main"] = "init_boot"
            partition_map["vbmeta"] = "vbmeta"

        return partition_map


def _resolve_strategy(
    gki: bool,
    root_type: str,
    strategy: Optional[RootStrategy] = None,
) -> RootStrategy:
    if strategy is not None:
        return strategy

    strategy = get_root_strategy(gki, root_type)
    if hasattr(strategy, "configure_source"):
        configured = strategy.configure_source()
        if configured is not True:
            raise ToolError(get_string("gki_custom_cancelled"))
        utils.ui.clear()
    return strategy


def _should_cleanup_manager_apk(strategy: Optional[RootStrategy]) -> bool:
    return not (
        isinstance(strategy, GkiRootStrategy)
        and getattr(strategy, "_kernel_zip", None) is not None
    )


def _prepare_root_output_dir(strategy: RootStrategy) -> None:
    utils.ui.echo(get_string("act_clean_dir").format(dir=strategy.log_output_dir_name))
    utils.recreate_dir(strategy.output_dir)
    utils.ui.echo("")


def _create_root_workflow_session(
    dev: device.DeviceController,
    *,
    gki: bool,
    strategy: RootStrategy,
    wait_for_adb: bool = False,
) -> RootWorkflowSession:
    if wait_for_adb and not dev.skip_adb:
        dev.adb.wait_for_device()

    return RootWorkflowSession(
        strategy=strategy,
        gki=gki,
        lkm_kernel_version=_get_lkm_kernel_version(dev, strategy),
    )


def _patch_root_from_folder(
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

    requires_vbmeta = strategy.requires_vbmeta

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

    utils.ui.echo(get_string("act_backup_images").format(name="image"))
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
        # Move originals aside instead of deleting so they can be restored on
        # failure.  The backup copies created above (strategy.backup_name and
        # FN_VBMETA_BAK) serve as the restore source.
        base_image = const.BASE_DIR / strategy.image_name
        base_vbmeta = const.BASE_DIR / const.FN_VBMETA if requires_vbmeta else None
        base_image.unlink()
        if base_vbmeta is not None:
            base_vbmeta.unlink()

        try:
            if isinstance(strategy, LkmRootStrategy) and not lkm_kernel_version:
                utils.ui.clear()
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
        except BaseException:
            # Restore originals from the backups created earlier.
            backup_image = const.BASE_DIR / strategy.backup_name
            if backup_image.exists() and not base_image.exists():
                shutil.copy(backup_image, base_image)
            if base_vbmeta is not None and not base_vbmeta.exists():
                vbmeta_bak = const.BASE_DIR / const.FN_VBMETA_BAK
                if vbmeta_bak.exists():
                    shutil.copy(vbmeta_bak, base_vbmeta)
            raise

    if patched_boot_path and patched_boot_path.exists():
        utils.ui.echo("\n" + get_string("act_finalize"))

        strategy.finalize_patch(patched_boot_path, strategy.output_dir, const.BASE_DIR)
        utils.ui.echo("")

        utils.ui.echo(get_string("act_move_backup").format(dir=const.BACKUP_DIR.name))
        const.BACKUP_DIR.mkdir(exist_ok=True)
        for bak_file in const.BASE_DIR.glob("*.bak.img"):
            shutil.move(bak_file, const.BACKUP_DIR / bak_file.name)
        utils.ui.echo("")

        width = utils.ui.get_term_width()
        utils.ui.echo("=" * width)
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
        utils.ui.echo("=" * width)
        return True
    else:
        fail_image = "boot" if gki else "init_boot"
        utils.ui.error(get_string("act_err_root_fail_image").format(image=fail_image))
        return False


def patch_root_image_file(
    gki: bool = False, root_type: str = "ksu", strategy=None
) -> None:
    strategy = _resolve_strategy(gki, root_type, strategy)
    _prepare_root_output_dir(strategy)

    _patch_root_from_folder(strategy, gki)


def patch_and_flash_root(
    dev: device.DeviceController,
    gki: bool = False,
    root_type: str = "ksu",
    strategy=None,
) -> None:
    if _should_cleanup_manager_apk(strategy):
        cleanup_manager_apk()
    strategy = _resolve_strategy(gki, root_type, strategy)
    _prepare_root_output_dir(strategy)
    session = _create_root_workflow_session(
        dev,
        gki=gki,
        strategy=strategy,
        wait_for_adb=True,
    )

    # Resolve PREINITDEVICE while ADB is still available
    if hasattr(session.strategy, "resolve_preinit_device"):
        if dev.skip_adb:
            utils.ui.error(get_string("magisk_err_skip_adb_required"))
            return
        session.strategy.resolve_preinit_device(dev)

    if not _patch_root_from_folder(
        session.strategy,
        session.gki,
        dev=dev,
        lkm_kernel_version=session.lkm_kernel_version,
        show_manual_flash_notice=False,
    ):
        return

    utils.ui.clear()
    confirm = (
        utils.ui.prompt(get_string("prompt_flash_image_folder_confirm")).strip().lower()
    )
    if confirm != "y":
        return

    edl.ensure_edl_requirements()
    _flash_root_image(
        dev,
        session.strategy,
        session.resolve_partition_map(dev),
        session.gki,
    )


def _prepare_root_env(strategy: RootStrategy):
    utils.ui.echo(get_string("act_start_root"))

    _prepare_root_output_dir(strategy)
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
            except (DeviceCommandError, DeviceConnectionError) as e:
                utils.ui.error(get_string("act_root_warn_lkm_kver_fail").format(e=e))
                utils.ui.error(get_string("act_root_warn_lkm_kver_retry"))
        else:
            utils.ui.clear()
            utils.ui.warn(get_string("act_root_warn_lkm_skip_adb"))
            manual_ver = input(get_string("prompt_enter_kernel_version")).strip()
            if not manual_ver:
                utils.ui.error(get_string("err_kernel_version_req"))
                raise ToolError(get_string("act_root_err_lkm_skip_adb_exc"))
            return manual_ver
    return None


def _dump_partition(
    dev: device.DeviceController, port: str, label: str, output_path: Path
):
    return _partition_service().dump_partition(dev, port, label, output_path)


def _generate_root_image(
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

        with dev.edl_session(auto_reset=False) as port:
            try:
                _dump_partition(dev, port, main_partition, dumped_main)

                if strategy.requires_vbmeta:
                    vbmeta_partition = partition_map["vbmeta"]
                    dumped_vbmeta = const.WORKING_BOOT_DIR / const.FN_VBMETA
                    _dump_partition(dev, port, vbmeta_partition, dumped_vbmeta)

                read_ok_suffix = "" if gki else " (init_boot)"
                utils.ui.echo(
                    get_string("act_read_dump_ok").format(
                        part=main_partition, suffix=read_ok_suffix, file=dumped_main
                    )
                )

            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(
                    get_string("act_err_dump").format(target=main_partition, e=e)
                )
                raise

            utils.ui.echo(
                get_string("act_backup_boot_root").format(dir=strategy.backup_dir.name)
            )
            shutil.copy(dumped_main, backup_main)
            utils.ui.echo(get_string("act_temp_backup_avb"))
            shutil.copy(dumped_main, base_main_bak)

            if strategy.requires_vbmeta:
                shutil.copy(
                    const.WORKING_BOOT_DIR / const.FN_VBMETA,
                    strategy.backup_dir / const.FN_VBMETA,
                )
                shutil.copy(
                    const.WORKING_BOOT_DIR / const.FN_VBMETA,
                    const.BASE_DIR / const.FN_VBMETA_BAK,
                )

            utils.ui.echo(get_string("act_backup_complete"))

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
        except (ToolError, subprocess.CalledProcessError, OSError, KeyError) as e:
            if isinstance(e, ToolError):
                utils.ui.error(str(e))
            else:
                utils.ui.error(get_string("act_err_avb_footer").format(e=e))
            base_main_bak.unlink(missing_ok=True)
            if strategy.requires_vbmeta:
                (const.BASE_DIR / const.FN_VBMETA_BAK).unlink(missing_ok=True)
            raise

        base_main_bak.unlink(missing_ok=True)
        if strategy.requires_vbmeta:
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

    if not dev.edl.check_device(silent=True):
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

            final_vbmeta_path = strategy.output_dir / const.FN_VBMETA
            if final_vbmeta_path.exists() and partition_map.get("vbmeta"):
                vbmeta_part = partition_map["vbmeta"]
                edl.flash_partition_target(dev, port, vbmeta_part, final_vbmeta_path)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            utils.ui.error(get_string("act_err_edl_write").format(e=e))
            raise


def root_device(
    dev: device.DeviceController,
    gki: bool = False,
    root_type: str = "ksu",
    strategy=None,
) -> None:
    if _should_cleanup_manager_apk(strategy):
        cleanup_manager_apk()
    strategy = _resolve_strategy(gki, root_type, strategy)

    _prepare_root_env(strategy)
    utils.ui.echo(get_string("act_root_step1"))
    session = _create_root_workflow_session(
        dev,
        gki=gki,
        strategy=strategy,
        wait_for_adb=True,
    )

    if not session.strategy.download_resources(session.lkm_kernel_version):
        utils.ui.error(get_string("err_download_resources_abort"))
        return

    apk_installed = _install_manager_apk(
        dev, required=session.strategy.manager_apk_required
    )

    # Resolve PREINITDEVICE while ADB is still available (before EDL dump)
    if hasattr(session.strategy, "resolve_preinit_device"):
        if dev.skip_adb:
            utils.ui.error(get_string("magisk_err_skip_adb_required"))
            return
        session.strategy.resolve_preinit_device(dev)

    utils.ui.echo(get_string("act_root_step2"))
    partition_map = session.resolve_partition_map(dev)

    _generate_root_image(
        dev,
        session.strategy,
        partition_map,
        session.gki,
        session.lkm_kernel_version,
    )

    _flash_root_image(dev, session.strategy, partition_map, session.gki)

    from ...logger import console as _console

    width = min(_console.width, 78)
    banner = "!" * width
    msg = get_string("act_root_warn_brick")
    _console.print()
    _console.print(banner, style="red", highlight=False, soft_wrap=True)
    for line in textwrap.wrap(msg, width):
        _console.print(line, style="red", highlight=False, soft_wrap=True)
    _console.print(banner, style="red", highlight=False, soft_wrap=True)
    _console.print()
    utils.ui.echo(get_string("act_root_finish"))

    if not apk_installed:
        _move_manager_apk_to_base()


def unroot_device(dev: device.DeviceController) -> None:
    utils.ui.echo(get_string("act_start_unroot"))

    strategies: List[RootStrategy] = [
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
        strategy_choices = {
            str(index): strategy
            for index, strategy in enumerate(available_strategies, start=1)
        }
        for key, strategy in strategy_choices.items():
            menu.add_option(key, get_string(strategy.unroot_menu_msg_key))

        menu.add_separator()
        menu.add_option("m", get_string("menu_root_m"))

        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "m":
            utils.ui.echo(get_string("act_op_cancel"))
            return

        selected_strategy = strategy_choices.get(choice)
        utils.ui.clear()

    elif len(available_strategies) == 1:
        selected_strategy = available_strategies[0]
        utils.ui.echo(get_string(selected_strategy.unroot_detect_msg_key))
    else:
        prompt = get_string("act_unroot_prompt_all").format(
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

    suffix = get_slot_suffix(dev)

    if selected_strategy:
        with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
            try:
                partition_map = selected_strategy.get_partition_map(suffix)
                selected_strategy.print_unroot_step(partition_map)

                for role, backup_path in selected_strategy.unroot_files.items():
                    target_part = partition_map[role]
                    edl.flash_partition_target(dev, port, target_part, backup_path)
            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(get_string("act_err_edl_write").format(e=e))
                raise

    utils.ui.echo(get_string("act_unroot_finish"))


def _sign_recovery_image(
    dumped_recovery: Path, twrp_src: Path, out_dir: Path, twrp_name: str
) -> Path:
    from ...patch.avb import (
        apply_avb_integrity_footer,
        _resolve_signing_key,
        extract_image_avb_info,
    )

    rec_info = extract_image_avb_info(dumped_recovery)
    key_file = _resolve_signing_key(rec_info.get("pubkey_sha1"), twrp_name)

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

    apply_avb_integrity_footer(
        image_path=final_twrp, image_info=rec_info, key_file=key_file
    )
    utils.ui.echo(get_string("act_sign_twrp_ok"))
    return final_twrp


def sign_and_flash_recovery(dev: device.DeviceController) -> None:
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

    suffix = get_slot_suffix(dev)
    target_partition = f"recovery{suffix}"

    utils.ui.echo(get_string("act_root_step2"))

    with utils.temporary_workspace(const.WORK_DIR):
        dumped_recovery = const.WORK_DIR / f"recovery{suffix}.img"

        with dev.edl_session(auto_reset=False) as port:
            utils.ui.echo(get_string("act_dump_recovery").format(part=target_partition))
            try:
                _partition_service().dump_partition(
                    dev, port, target_partition, dumped_recovery
                )
            except (subprocess.CalledProcessError, OSError, ValueError) as e:
                utils.ui.error(
                    get_string("act_err_dump").format(target=target_partition, e=e)
                )
                raise

            backup_recovery = const.BACKUP_DIR / f"recovery{suffix}.img"
            const.BACKUP_DIR.mkdir(exist_ok=True)
            shutil.copy(dumped_recovery, backup_recovery)
            utils.ui.echo(get_string("act_backup_recovery_ok"))

        utils.ui.echo(get_string("act_sign_twrp_start"))
        final_twrp = _sign_recovery_image(dumped_recovery, twrp_src, out_dir, twrp_name)

        if not dev.edl.check_device(silent=True):
            utils.ui.echo(get_string("act_reboot_edl_flash"))
            if not dev.skip_adb:
                dev.adb.wait_for_device()
            else:
                utils.ui.echo(get_string("act_manual_edl_now"))

        with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
            edl.flash_partition_target(dev, port, target_partition, final_twrp)

    utils.ui.echo(get_string("act_success"))


def _install_manager_apk(
    dev: device.DeviceController, *, required: bool = True
) -> bool:
    manager_apk = const.TOOLS_DIR / "manager.apk"

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "-" * width)
    utils.ui.echo(get_string("act_install_ksu").format(name="Manager App"))

    if not manager_apk.exists():
        printer = utils.ui.error if required else utils.ui.echo
        printer(get_string("act_manager_apk_not_found"))
        utils.ui.echo("-" * width + "\n")
        return False

    if dev.skip_adb:
        utils.ui.echo(get_string("act_adb_skipped_manual_install"))
        utils.ui.echo(get_string("act_file_location").format(path=manager_apk))
        utils.ui.echo("-" * width + "\n")
        return True

    utils.ui.echo(get_string("act_wait_sys_adb"))
    try:
        dev.adb.wait_for_device()
        dev.adb.install(manager_apk)
        utils.ui.echo(get_string("act_ksu_ok"))
        utils.ui.echo("-" * width + "\n")
        return True
    except Exception as e:
        utils.ui.error(get_string("act_err_ksu").format(e=e))
        utils.ui.echo("-" * width + "\n")
        return False


def _move_manager_apk_to_base():
    manager_apk = const.TOOLS_DIR / "manager.apk"
    if manager_apk.exists():
        dest = const.BASE_DIR / "manager.apk"
        shutil.move(str(manager_apk), str(dest))
        utils.ui.echo(get_string("act_manager_apk_manual_install").format(path=dest))
