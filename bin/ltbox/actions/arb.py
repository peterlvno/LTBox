import os
import shutil
import subprocess
from enum import Enum
from pathlib import Path
from typing import Tuple

from .. import constants as const
from .. import device, utils
from ..i18n import get_string
from ..patch.avb import (
    extract_image_avb_info,
    patch_chained_image_rollback,
    patch_vbmeta_image_rollback,
)
from . import edl
from .system import get_slot_suffix


class ArbStatus(str, Enum):
    MATCH = "MATCH"
    NEEDS_PATCH = "NEEDS_PATCH"
    MISSING_NEW = "MISSING_NEW"
    ERROR = "ERROR"


def read_anti_rollback(
    dumped_boot_path: Path, dumped_vbmeta_path: Path
) -> Tuple[ArbStatus, int, int]:
    utils.ui.echo(get_string("act_start_arb"))
    utils.check_dependencies()

    boot_rollback = 0
    vbmeta_rollback = 0

    utils.ui.echo(get_string("act_arb_step1"))
    try:
        if not dumped_boot_path.exists() or not dumped_vbmeta_path.exists():
            raise FileNotFoundError(get_string("act_err_dumped_missing"))

        utils.ui.echo(
            get_string("act_read_dumped_file").format(name=dumped_boot_path.name)
        )
        boot_info = extract_image_avb_info(dumped_boot_path)
        boot_rollback = int(boot_info.get("rollback", "0"))

        utils.ui.echo(
            get_string("act_read_dumped_file").format(name=dumped_vbmeta_path.name)
        )
        vbmeta_info = extract_image_avb_info(dumped_vbmeta_path)
        vbmeta_rollback = int(vbmeta_info.get("rollback", "0"))

    except (FileNotFoundError, ValueError, subprocess.CalledProcessError) as e:
        width = utils.ui.get_term_width()
        utils.ui.error("\n" + "!" * width)
        utils.ui.error(get_string("act_err_arb_early_fw"))
        utils.ui.error("!" * width + "\n")

        utils.ui.error(get_string("act_err_avb_info").format(e=e))
        utils.ui.echo(get_string("act_arb_error"))
        return ArbStatus.ERROR, 0, 0

    utils.ui.echo(get_string("act_curr_boot_idx").format(idx=boot_rollback))
    utils.ui.echo(get_string("act_curr_vbmeta_idx").format(idx=vbmeta_rollback))

    utils.ui.echo(get_string("act_arb_step2"))
    utils.ui.echo(get_string("act_extract_new_indices"))
    new_boot_img = const.IMAGE_DIR / const.FN_BOOT
    new_vbmeta_img = const.IMAGE_DIR / const.FN_VBMETA_SYSTEM

    if not new_boot_img.exists() or not new_vbmeta_img.exists():
        utils.ui.echo(
            get_string("act_err_new_rom_missing").format(dir=const.IMAGE_DIR.name)
        )
        utils.ui.echo(get_string("act_arb_missing_new"))
        return ArbStatus.MISSING_NEW, 0, 0

    new_boot_rb = 0
    new_vbmeta_rb = 0
    try:
        new_boot_info = extract_image_avb_info(new_boot_img)
        new_boot_rb = int(new_boot_info.get("rollback", "0"))

        new_vbmeta_info = extract_image_avb_info(new_vbmeta_img)
        new_vbmeta_rb = int(new_vbmeta_info.get("rollback", "0"))
    except (ValueError, subprocess.CalledProcessError) as e:
        utils.ui.error(get_string("act_err_read_new_info").format(e=e))
        utils.ui.echo(get_string("act_arb_error"))
        return ArbStatus.ERROR, 0, 0

    utils.ui.echo(get_string("act_new_boot_idx").format(idx=new_boot_rb))
    utils.ui.echo(get_string("act_new_vbmeta_idx").format(idx=new_vbmeta_rb))

    if new_boot_rb < boot_rollback or new_vbmeta_rb < vbmeta_rollback:
        utils.ui.echo(get_string("act_arb_patch_req"))
        status = ArbStatus.NEEDS_PATCH
    else:
        utils.ui.echo(get_string("act_arb_match"))
        status = ArbStatus.MATCH

    utils.ui.echo(get_string("act_arb_complete").format(status=status.value))
    return status, boot_rollback, vbmeta_rollback


def patch_anti_rollback(comparison_result: Tuple[ArbStatus, int, int]) -> None:
    utils.ui.echo(get_string("act_start_arb_patch"))
    utils.check_dependencies()

    utils.recreate_dir(const.OUTPUT_ANTI_ROLLBACK_DIR)

    try:
        if comparison_result:
            utils.ui.echo(get_string("act_use_pre_arb"))
            status, boot_rollback, vbmeta_rollback = comparison_result
        else:
            utils.ui.echo(get_string("act_err_no_cmp"))
            return

        if status != ArbStatus.NEEDS_PATCH:
            utils.ui.echo(get_string("act_arb_no_patch"))
            return

        utils.ui.echo(get_string("act_arb_step3"))

        patch_chained_image_rollback(
            image_name=const.FN_BOOT,
            current_rb_index=boot_rollback,
            new_image_path=(const.IMAGE_DIR / const.FN_BOOT),
            patched_image_path=(const.OUTPUT_ANTI_ROLLBACK_DIR / const.FN_BOOT),
        )

        utils.ui.echo("-" * 20)

        patch_vbmeta_image_rollback(
            image_name=const.FN_VBMETA_SYSTEM,
            current_rb_index=vbmeta_rollback,
            new_image_path=(const.IMAGE_DIR / const.FN_VBMETA_SYSTEM),
            patched_image_path=(
                const.OUTPUT_ANTI_ROLLBACK_DIR / const.FN_VBMETA_SYSTEM
            ),
        )

        width = utils.ui.get_term_width()
        utils.ui.echo("\n  " + "=" * width)
        utils.ui.echo(get_string("act_success"))
        utils.ui.echo(
            get_string("act_arb_patched_ready").format(
                dir=const.OUTPUT_ANTI_ROLLBACK_DIR.name
            )
        )
        utils.ui.echo("  " + "=" * width)

    except (KeyError, subprocess.CalledProcessError, FileNotFoundError, OSError) as e:
        utils.ui.error(get_string("act_err_arb_patch").format(e=e))
        shutil.rmtree(const.OUTPUT_ANTI_ROLLBACK_DIR)


def read_device_anti_rollback(dev: device.DeviceController) -> None:
    utils.ui.echo(get_string("act_start_arb"))

    suffix = get_slot_suffix(dev)
    boot_target = f"boot{suffix}"
    vbmeta_target = f"vbmeta_system{suffix}"

    edl.dump_partitions(
        dev=dev,
        skip_reset=False,
        additional_targets=[boot_target, vbmeta_target],
        default_targets=False,
    )

    dumped_boot = const.BACKUP_DIR / f"{boot_target}.img"
    dumped_vbmeta = const.BACKUP_DIR / f"{vbmeta_target}.img"

    if not dumped_boot.exists() or not dumped_vbmeta.exists():
        utils.ui.error(get_string("act_err_dumped_missing"))
        raise FileNotFoundError(get_string("act_err_dumped_missing"))

    read_anti_rollback(dumped_boot_path=dumped_boot, dumped_vbmeta_path=dumped_vbmeta)


def patch_rom_anti_rollback() -> None:
    utils.ui.echo(get_string("act_start_arb_patch"))

    backup_dir = const.BACKUP_DIR

    boot_files = sorted(
        backup_dir.glob("boot*.img"), key=os.path.getmtime, reverse=True
    )
    vbmeta_files = sorted(
        backup_dir.glob("vbmeta_system*.img"), key=os.path.getmtime, reverse=True
    )

    if not boot_files or not vbmeta_files:
        utils.ui.error(get_string("act_err_dumped_missing"))
        utils.ui.error(get_string("act_arb_run_detect_first"))
        raise FileNotFoundError(get_string("act_err_dumped_missing"))

    dumped_boot = boot_files[0]
    dumped_vbmeta = vbmeta_files[0]

    utils.ui.echo(
        get_string("act_arb_using_dumped_files").format(
            boot=dumped_boot.name, vbmeta=dumped_vbmeta.name
        )
    )

    comparison_result = read_anti_rollback(
        dumped_boot_path=dumped_boot, dumped_vbmeta_path=dumped_vbmeta
    )

    patch_anti_rollback(comparison_result=comparison_result)
