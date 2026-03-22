import shutil
from datetime import datetime
from pathlib import Path
from typing import Callable, List, Optional, Tuple

from .. import constants as const
from .. import device, utils
from ..errors import ToolError
from ..i18n import get_string
from ..menu import TerminalMenu
from ..patch.avb import (
    _apply_avb_integrity_footer,
    _require_info_keys,
    extract_image_avb_info,
    rebuild_vbmeta_with_chained_images,
)
from ..patch.region import detect_country_codes, edit_vendor_boot, patch_country_codes
from . import edl
from .system import get_slot_suffix


def rebuild_vbmeta(
    dev: device.DeviceController,
    on_log: Callable[[str], None] = lambda s: None,
) -> None:
    on_log(get_string("act_wait_vbmeta_rebuild_images"))

    const.IMAGE_DIR.mkdir(exist_ok=True)

    vbmeta_src = const.IMAGE_DIR / const.FN_VBMETA
    candidate_sources = [
        const.IMAGE_DIR / "dtbo.img",
        const.IMAGE_DIR / const.FN_INIT_BOOT,
        const.IMAGE_DIR / const.FN_VENDOR_BOOT,
    ]

    if not vbmeta_src.exists() or not any(img.exists() for img in candidate_sources):
        raise FileNotFoundError(get_string("act_err_vbmeta_rebuild_files_missing"))

    selected_sources = [img for img in candidate_sources if img.exists()]

    if const.OUTPUT_DIR.exists():
        shutil.rmtree(const.OUTPUT_DIR)
    const.OUTPUT_DIR.mkdir(exist_ok=True)

    rebuilt_inputs: List[Path] = []
    for src in selected_sources:
        dst = const.OUTPUT_DIR / src.name
        shutil.copy(src, dst)

        image_info = extract_image_avb_info(src)
        _require_info_keys(
            image_info,
            ["partition_size", "name", "rollback", "salt", "algorithm"],
            src,
        )

        _apply_avb_integrity_footer(dst, image_info, None)
        rebuilt_inputs.append(dst)

    rebuilt_vbmeta = const.OUTPUT_DIR / const.FN_VBMETA
    rebuild_vbmeta_with_chained_images(
        output_path=rebuilt_vbmeta,
        original_vbmeta_path=vbmeta_src,
        chained_images=rebuilt_inputs,
    )

    on_log(
        get_string("act_vbmeta_rebuild_complete").format(
            images=", ".join(img.name for img in rebuilt_inputs)
        )
    )

    if (
        utils.ui.prompt(get_string("prompt_flash_image_folder_confirm")).strip().lower()
        != "y"
    ):
        return

    if not dev.skip_adb:
        dev.adb.wait_for_device()

    try:
        active_slot = get_slot_suffix(dev)
    except (ToolError, OSError):
        active_slot = ""

    on_log(get_string("rescue_reboot_edl"))
    dev.adb.reboot("edl")

    flash_targets = [
        (Path(img_path).stem, img_path)
        for img_path in rebuilt_inputs + [rebuilt_vbmeta]
    ]
    with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
        for base_target, image_path in flash_targets:
            target = f"{base_target}_{active_slot}" if active_slot else base_target
            edl.flash_partition_target(dev, port, target, image_path)


def convert_region_images(
    dev: device.DeviceController,
    device_model: Optional[str] = None,
    target_region: str = "PRC",
    modify_region_code: bool = True,
    on_log: Callable[[str], None] = lambda s: None,
) -> None:

    on_log(get_string("act_conv_start"))
    on_log(f"Target Region: {target_region}")

    on_log(get_string("act_clean_old"))
    if const.OUTPUT_DIR.exists():
        shutil.rmtree(const.OUTPUT_DIR)
    on_log("")

    on_log(get_string("act_wait_vb_vbmeta"))
    const.IMAGE_DIR.mkdir(exist_ok=True)

    vendor_boot_src = const.IMAGE_DIR / const.FN_VENDOR_BOOT
    vbmeta_src = const.IMAGE_DIR / const.FN_VBMETA

    if not vendor_boot_src.exists() or not vbmeta_src.exists():
        raise FileNotFoundError(
            get_string("act_err_xml_missing").format(dir=const.IMAGE_DIR.name)
        )

    on_log(get_string("act_backup_images").format(name="images"))
    vendor_boot_bak = const.BASE_DIR / const.FN_VENDOR_BOOT_BAK
    vbmeta_bak = const.BASE_DIR / const.FN_VBMETA_BAK

    try:
        shutil.copy(vendor_boot_src, vendor_boot_bak)
        shutil.copy(vbmeta_src, vbmeta_bak)
        on_log(get_string("act_backup_complete"))
    except (IOError, OSError) as e:
        raise IOError(get_string("act_err_copy_input").format(e=e))

    on_log(get_string("act_start_conv"))
    if not modify_region_code:
        on_log(get_string("act_skip_region_modify"))
        on_log(get_string("act_skip_val"))
        on_log(get_string("act_finalize"))
        on_log(get_string("act_rename_final"))

        final_vendor_boot = const.BASE_DIR / const.FN_VENDOR_BOOT
        final_vbmeta = const.BASE_DIR / const.FN_VBMETA
        shutil.copy(vendor_boot_bak, final_vendor_boot)
        shutil.copy(vbmeta_bak, final_vbmeta)

        final_images = [final_vendor_boot, final_vbmeta]
        on_log(get_string("act_move_final").format(dir=const.OUTPUT_DIR.name))
        utils.move_existing_files(final_images, const.OUTPUT_DIR)
        on_log(get_string("act_move_backup").format(dir=const.BACKUP_DIR.name))
        utils.move_existing_files(const.BASE_DIR.glob("*.bak.img"), const.BACKUP_DIR)
        on_log("")

        width = utils.ui.get_term_width()
        on_log("  " + "=" * width)
        on_log(get_string("act_success"))
        on_log(get_string("act_final_saved").format(dir=const.OUTPUT_DIR.name))
        on_log("  " + "=" * width)
        return

    edit_vendor_boot(str(vendor_boot_bak), target_region=target_region)

    vendor_boot_patched = const.BASE_DIR / const.FN_VENDOR_BOOT_PRC
    on_log(get_string("act_verify_conv"))
    if not vendor_boot_patched.exists():
        raise FileNotFoundError(get_string("act_err_vb_prc_not_created"))
    on_log(get_string("act_conv_success"))

    on_log(get_string("act_extract_info"))
    vendor_boot_info = extract_image_avb_info(vendor_boot_bak)
    on_log(get_string("act_info_extracted"))

    if device_model and not dev.skip_adb:
        device_model = device_model.replace(" ", "")
        on_log(get_string("act_val_model").format(model=device_model))
        fingerprint_key = "com.android.build.vendor_boot.fingerprint"
        if fingerprint_key in vendor_boot_info:
            fingerprint = vendor_boot_info[fingerprint_key]
            on_log(get_string("act_found_fp").format(fp=fingerprint))
            if device_model in fingerprint:
                on_log(get_string("act_model_match").format(model=device_model))
            else:
                on_log(get_string("act_model_mismatch").format(model=device_model))
                on_log(get_string("act_rom_mismatch_abort"))
                raise RuntimeError(get_string("act_err_firmware_mismatch"))
        else:
            on_log(get_string("act_warn_fp_missing").format(key=fingerprint_key))
            on_log(get_string("act_skip_val"))

    on_log(get_string("act_add_footer_vb"))

    _require_info_keys(
        vendor_boot_info,
        ["partition_size", "name", "rollback", "salt", "algorithm"],
        vendor_boot_bak,
    )

    _apply_avb_integrity_footer(vendor_boot_patched, vendor_boot_info, None)

    vbmeta_img = const.BASE_DIR / const.FN_VBMETA
    rebuild_vbmeta_with_chained_images(
        output_path=vbmeta_img,
        original_vbmeta_path=vbmeta_bak,
        chained_images=[vendor_boot_patched],
    )
    on_log("")

    on_log(get_string("act_finalize"))
    on_log(get_string("act_rename_final"))
    final_vendor_boot = const.BASE_DIR / const.FN_VENDOR_BOOT
    shutil.move(const.BASE_DIR / const.FN_VENDOR_BOOT_PRC, final_vendor_boot)

    final_images = [final_vendor_boot, const.BASE_DIR / const.FN_VBMETA]

    on_log(get_string("act_move_final").format(dir=const.OUTPUT_DIR.name))
    utils.move_existing_files(final_images, const.OUTPUT_DIR)

    on_log(get_string("act_move_backup").format(dir=const.BACKUP_DIR.name))
    utils.move_existing_files(const.BASE_DIR.glob("*.bak.img"), const.BACKUP_DIR)
    on_log("")

    width = utils.ui.get_term_width()
    on_log("  " + "=" * width)
    on_log(get_string("act_success"))
    on_log(get_string("act_final_saved").format(dir=const.OUTPUT_DIR.name))
    on_log("  " + "=" * width)


def _default_select_callback(options: List[Tuple[str, str]], prompt_msg: str) -> str:
    breadcrumbs = f"{get_string('menu_main_title')} > {get_string('menu_adv_title')}"
    menu = TerminalMenu(prompt_msg, breadcrumbs=breadcrumbs)
    for idx, (code, name) in enumerate(options):
        menu.add_option(str(idx + 1), f"{name} ({code})")

    choice = menu.ask(get_string("prompt_select"), get_string("act_invalid_selection"))

    idx = int(choice) - 1
    return options[idx][0]


def edit_devinfo_persist(
    on_log: Callable[[str], None] = lambda s: None,
    on_confirm: Callable[[str], bool] = lambda msg: True,
    on_select: Callable[[List[Tuple[str, str]], str], str] = _default_select_callback,
) -> Optional[str]:
    on_log(get_string("act_start_dp_patch"))

    on_log(get_string("act_wait_dp"))
    const.BACKUP_DIR.mkdir(exist_ok=True)

    devinfo_img_src = const.BACKUP_DIR / const.FN_DEVINFO
    persist_img_src = const.BACKUP_DIR / const.FN_PERSIST

    devinfo_img = const.BASE_DIR / const.FN_DEVINFO
    persist_img = const.BASE_DIR / const.FN_PERSIST

    if not devinfo_img_src.exists() and not persist_img_src.exists():
        on_log(get_string("act_err_dp_missing_backup"))
        return None

    if devinfo_img_src.exists():
        shutil.copy(devinfo_img_src, devinfo_img)
    if persist_img_src.exists():
        shutil.copy(persist_img_src, persist_img)

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup_critical_dir = const.BASE_DIR / f"backup_critical_{timestamp}"
    backup_critical_dir.mkdir(exist_ok=True)

    if devinfo_img.exists():
        shutil.copy(devinfo_img, backup_critical_dir)
    if persist_img.exists():
        shutil.copy(persist_img, backup_critical_dir)
    on_log(get_string("act_files_backed_up").format(dir=backup_critical_dir.name))

    on_log(get_string("act_clean_dir").format(dir=const.OUTPUT_DP_DIR.name))
    if const.OUTPUT_DP_DIR.exists():
        shutil.rmtree(const.OUTPUT_DP_DIR)
    const.OUTPUT_DP_DIR.mkdir(exist_ok=True)

    on_log(get_string("act_detect_codes"))
    detected_codes = detect_country_codes()

    status_messages = []
    files_found = 0

    display_order = [const.FN_PERSIST, const.FN_DEVINFO]

    for fname in display_order:
        if fname in detected_codes:
            code = detected_codes[fname]
            display_name = Path(fname).stem

            if code:
                status_messages.append(
                    get_string("act_detect_status_found").format(
                        display_name=display_name, code=code
                    )
                )
                files_found += 1
            else:
                status_messages.append(
                    get_string("act_detect_status_null").format(
                        display_name=display_name
                    )
                )

    on_log(get_string("act_detect_result").format(res=", ".join(status_messages)))

    if files_found == 0:
        on_log(get_string("act_no_codes_skip"))
        devinfo_img.unlink(missing_ok=True)
        persist_img.unlink(missing_ok=True)
        return backup_critical_dir.name

    separator = "=" * utils.ui.get_term_width()
    note_message = get_string("act_note_region_code")
    ask_message = get_string("act_ask_change_code").strip()

    on_log("")
    on_log(f"\033[96m{separator}\033[0m")
    on_log(f"\033[96m{note_message}\033[0m")
    on_log(f"\033[96m{separator}\033[0m")

    should_change = on_confirm(f"\033[93m{ask_message}\033[0m")

    if not should_change:
        on_log(get_string("act_op_cancel"))

        devinfo_img.unlink(missing_ok=True)
        persist_img.unlink(missing_ok=True)

        on_log(get_string("act_safety_remove"))
        (const.IMAGE_DIR / const.FN_DEVINFO).unlink(missing_ok=True)
        (const.IMAGE_DIR / const.FN_PERSIST).unlink(missing_ok=True)
        return backup_critical_dir.name

    target_map = detected_codes.copy()

    if not const.SORTED_COUNTRY_CODES:
        raise ImportError(get_string("act_err_codes_missing_exc"))

    prompt_msg = get_string("act_select_new_code")
    replacement_code = on_select(const.SORTED_COUNTRY_CODES, prompt_msg)

    if not replacement_code:
        on_log(get_string("act_select_cancel"))
        return backup_critical_dir.name

    on_log(
        get_string("act_selected").format(name=replacement_code, code=replacement_code)
    )
    patch_country_codes(replacement_code, target_map)

    modified_devinfo = const.BASE_DIR / "devinfo_modified.img"
    modified_persist = const.BASE_DIR / "persist_modified.img"

    if modified_devinfo.exists():
        shutil.move(modified_devinfo, const.OUTPUT_DP_DIR / const.FN_DEVINFO)
    if modified_persist.exists():
        shutil.move(modified_persist, const.OUTPUT_DP_DIR / const.FN_PERSIST)

    on_log(get_string("act_dp_moved").format(dir=const.OUTPUT_DP_DIR.name))

    devinfo_img.unlink(missing_ok=True)
    persist_img.unlink(missing_ok=True)

    width = utils.ui.get_term_width()
    on_log("\n  " + "=" * width)
    on_log(get_string("act_success"))
    on_log(get_string("act_dp_ready").format(dir=const.OUTPUT_DP_DIR.name))
    on_log("  " + "=" * width)

    return backup_critical_dir.name


def rescue_after_ota(
    dev: device.DeviceController, on_log: Callable[[str], None] = lambda s: None
) -> None:
    on_log(get_string("rescue_prompt_files"))

    edl.ensure_edl_requirements()

    on_log(get_string("rescue_wait_adb"))
    dev.adb.wait_for_device()

    on_log(get_string("rescue_reboot_edl"))
    dev.adb.reboot("edl")

    slots = ["a", "b"]
    targets = [f"vendor_boot_{s}" for s in slots] + [f"vbmeta_{s}" for s in slots]

    edl.dump_partitions(
        dev, skip_reset=False, additional_targets=targets, default_targets=False
    )

    const.OUTPUT_DIR.mkdir(exist_ok=True)
    patched_map = {}

    for slot in slots:
        vb_target = f"vendor_boot_{slot}"
        vbmeta_target = f"vbmeta_{slot}"

        vb_path = const.BACKUP_DIR / f"{vb_target}.img"
        vbmeta_path = const.BACKUP_DIR / f"{vbmeta_target}.img"

        if not vb_path.exists() or not vbmeta_path.exists():
            continue

        prc_temp = vb_path.parent / const.FN_VENDOR_BOOT_PRC
        prc_temp.unlink(missing_ok=True)

        try:
            on_log(get_string("rescue_patching_slot").format(slot=slot))
            if not edit_vendor_boot(str(vb_path), copy_if_unchanged=False):
                on_log(get_string("rescue_skip_no_change").format(slot=slot))
                continue
        except (OSError, ValueError, RuntimeError) as e:
            on_log(get_string("rescue_skip_error").format(slot=slot, e=e))
            continue

        if not prc_temp.exists():
            on_log(get_string("rescue_skip_no_output").format(slot=slot))
            continue

        dest_vb = const.OUTPUT_DIR / f"{vb_target}.img"
        shutil.move(prc_temp, dest_vb)
        patched_map[vb_target] = dest_vb

        on_log(get_string("rescue_remaking_vbmeta").format(slot=slot))

        vb_info = extract_image_avb_info(vb_path)
        _require_info_keys(
            vb_info,
            ["partition_size", "name", "rollback", "salt", "algorithm"],
            vb_path,
            defaults={
                "name": "vendor_boot",
                "rollback": "0",
                "salt": "",
                "algorithm": "SHA256_RSA4096",
            },
        )

        _apply_avb_integrity_footer(dest_vb, vb_info, None)

        dest_vbmeta = const.OUTPUT_DIR / f"{vbmeta_target}.img"

        rebuild_vbmeta_with_chained_images(
            output_path=dest_vbmeta,
            original_vbmeta_path=vbmeta_path,
            chained_images=[dest_vb],
        )

        patched_map[vbmeta_target] = dest_vbmeta

    if not patched_map:
        on_log(get_string("rescue_nothing_to_flash"))
        return

    on_log(get_string("rescue_wait_adb_flash"))

    edl.ensure_edl_requirements()
    with dev.edl_session(auto_reset=True, reset_msg_key="act_reset_sys") as port:
        for target, path in patched_map.items():
            edl.flash_partition_target(dev, port, target, path)
