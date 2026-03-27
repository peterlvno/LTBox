import shutil
import subprocess
from pathlib import Path
from typing import Dict, List, Optional

from .. import constants as const
from .. import device, utils
from ..errors import DeviceCommandError, DeviceConnectionError, ToolError
from ..i18n import get_string
from ..menu import TerminalMenu
from ..partition import require_partition_params
from . import edl
from .root_strategy_downloads import cleanup_manager_apk
from .root_strategies import (
    GkiRootStrategy,
    LkmRootStrategy,
    RootStrategy,
    get_root_strategy,
)
from .system import get_slot_suffix


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
        (const.BASE_DIR / strategy.image_name).unlink()

        if requires_vbmeta:
            (const.BASE_DIR / const.FN_VBMETA).unlink()

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


def patch_root_image_file(
    gki: bool = False, root_type: str = "ksu", strategy=None
) -> None:
    if strategy is None:
        strategy = get_root_strategy(gki, root_type)
        if hasattr(strategy, "configure_source"):
            strategy.configure_source()
            utils.ui.clear()

    utils.ui.echo(get_string("act_clean_dir").format(dir=strategy.log_output_dir_name))
    utils.recreate_dir(strategy.output_dir)
    utils.ui.echo("")

    _patch_root_from_folder(strategy, gki)


def patch_and_flash_root(
    dev: device.DeviceController,
    gki: bool = False,
    root_type: str = "ksu",
    strategy=None,
) -> None:
    if strategy is None:
        strategy = get_root_strategy(gki, root_type)
        if hasattr(strategy, "configure_source"):
            strategy.configure_source()
            utils.ui.clear()

    cleanup_manager_apk()

    utils.ui.echo(get_string("act_clean_dir").format(dir=strategy.log_output_dir_name))
    utils.recreate_dir(strategy.output_dir)
    utils.ui.echo("")

    if not dev.skip_adb:
        dev.adb.wait_for_device()

    lkm_kernel_version = _get_lkm_kernel_version(dev, strategy)

    if not _patch_root_from_folder(
        strategy,
        gki,
        dev=dev,
        lkm_kernel_version=lkm_kernel_version,
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

    suffix = get_slot_suffix(dev)
    partition_map = strategy.get_partition_map(suffix)

    if suffix:
        utils.ui.echo(get_string("act_active_slot").format(slot=suffix))
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
    params = require_partition_params(label)
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
                get_string("act_err_dump_size_mismatch").format(
                    target=label, expected=expected, actual=actual
                )
            )


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

        with dev.edl_session(auto_reset=True, reset_msg_key="act_dump_reset") as port:
            try:
                _dump_partition(dev, port, main_partition, dumped_main)

                if const.FN_VBMETA in strategy.required_files:
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
    dev: device.DeviceController,
    gki: bool = False,
    root_type: str = "ksu",
    strategy=None,
) -> None:
    if strategy is None:
        strategy = get_root_strategy(gki, root_type)
        if hasattr(strategy, "configure_source"):
            strategy.configure_source()
            utils.ui.clear()

    cleanup_manager_apk()

    _prepare_root_env(strategy)

    utils.ui.echo(get_string("act_root_step1"))
    if not dev.skip_adb:
        dev.adb.wait_for_device()

    lkm_kernel_version = _get_lkm_kernel_version(dev, strategy)

    if not strategy.download_resources(lkm_kernel_version):
        utils.ui.error(get_string("err_download_resources_abort"))
        return

    apk_installed = _install_manager_apk(dev)

    suffix = get_slot_suffix(dev)

    partition_map = strategy.get_partition_map(suffix)

    if suffix:
        utils.ui.echo(get_string("act_active_slot").format(slot=suffix))
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

    _generate_root_image(dev, strategy, partition_map, gki, lkm_kernel_version)

    _flash_root_image(dev, strategy, partition_map, gki)

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "!" * width)
    utils.ui.error(get_string("act_root_warn_brick"))
    utils.ui.echo("!" * width + "\n")
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
                    utils.ui.echo(
                        get_string("act_flash_img").format(
                            filename=backup_path.name, part=target_part
                        )
                    )
            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(get_string("act_err_edl_write").format(e=e))
                raise

    utils.ui.echo(get_string("act_unroot_finish"))


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

        with dev.edl_session(auto_reset=True, reset_msg_key="act_dump_reset") as port:
            utils.ui.echo(get_string("act_dump_recovery").format(part=target_partition))
            try:
                params = require_partition_params(target_partition)
                dev.edl.read_partition(
                    port=port,
                    output_filename=str(dumped_recovery),
                    lun=params["lun"],
                    start_sector=params["start_sector"],
                    num_sectors=params["num_sectors"],
                )
            except (subprocess.CalledProcessError, OSError, ValueError) as e:
                utils.ui.error(
                    get_string("act_err_dump").format(part=target_partition, e=e)
                )
                raise

            backup_recovery = const.BACKUP_DIR / f"recovery{suffix}.img"
            const.BACKUP_DIR.mkdir(exist_ok=True)
            shutil.copy(dumped_recovery, backup_recovery)
            utils.ui.echo(get_string("act_backup_recovery_ok"))

        utils.ui.echo(get_string("act_sign_twrp_start"))

        from ..patch.avb import _apply_avb_integrity_footer, extract_image_avb_info

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

        _apply_avb_integrity_footer(
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


def _install_manager_apk(dev: device.DeviceController) -> bool:
    manager_apk = const.TOOLS_DIR / "manager.apk"

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "-" * width)
    utils.ui.echo(get_string("act_install_ksu").format(name="Manager App"))

    if not manager_apk.exists():
        utils.ui.error(get_string("act_manager_apk_not_found"))
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
