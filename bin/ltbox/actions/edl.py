import itertools
import shutil
import subprocess
import time
import traceback
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Tuple

from .. import constants as const
from .. import device, utils
from ..i18n import get_string
from ..partition import require_partition_params
from . import xml


def _collect_base_partitions() -> Dict[str, Any]:
    xml.ensure_xml_files()

    xml_files = sorted(const.IMAGE_DIR.glob("rawprogram*.xml"))
    if not xml_files:
        xml_files = sorted(const.OUTPUT_XML_DIR.glob("rawprogram*.xml"))

    partitions: Dict[str, Any] = defaultdict(
        lambda: {
            "is_ab": False,
            "a": [],
            "b": [],
            "none": [],
            "has_files": False,
        }
    )

    for xml_file in xml_files:
        try:
            rp = xml.RawProgramXml(xml_file)
        except (OSError, ET.ParseError) as e:
            utils.ui.error(
                get_string("act_xml_parse_err").format(name=xml_file.name, e=e)
            )
            continue

        for prog in rp.programs:
            label = prog.label.strip()
            if not label:
                continue

            filename = prog.filename.strip()
            lun = prog.lun
            start_sector = prog.start_sector

            is_ab = label.endswith("_a") or label.endswith("_b")
            base_label = label[:-2] if is_ab else label

            if is_ab:
                partitions[base_label]["is_ab"] = True
                slot = label[-1]
                partitions[base_label][slot].append(
                    {"filename": filename, "lun": lun, "start_sector": start_sector}
                )
            else:
                partitions[base_label]["none"].append(
                    {"filename": filename, "lun": lun, "start_sector": start_sector}
                )

            if filename:
                partitions[base_label]["has_files"] = True

    return {k: v for k, v in partitions.items() if v["has_files"]}


def _prompt_partition_selection(labels: List[str]) -> List[str]:
    selected: Set[str] = set()

    while True:
        utils.ui.clear()
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(f"   {get_string('act_flash_partitions_label_title')}")
        utils.ui.echo("=" * width + "\n")

        count = len(labels)
        for i in range(0, count, 2):
            label1 = labels[i]
            mark1 = " [v]" if label1 in selected else ""
            item1 = f" {i + 1:3d}. {label1}{mark1}"

            if i + 1 < count:
                label2 = labels[i + 1]
                mark2 = " [v]" if label2 in selected else ""
                item2 = f"{i + 2:3d}. {label2}{mark2}"
                utils.ui.echo(f"  {item1:<38} {item2}")
            else:
                utils.ui.echo(f"  {item1}")

        utils.ui.echo(f"   f. {get_string('act_flash_partitions_select_done')}")
        utils.ui.echo(f"   c. {get_string('cancel')}")
        utils.ui.echo("\n" + "=" * width + "\n")

        choice = utils.ui.prompt(get_string("prompt_select")).strip().lower()
        if choice == "f":
            return [label for label in labels if label in selected]
        if choice == "c":
            return []

        try:
            idx = int(choice)
        except ValueError:
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        if not 1 <= idx <= len(labels):
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        label = labels[idx - 1]
        if label in selected:
            selected.remove(label)
        else:
            selected.add(label)


def flash_selected_partitions(
    dev: device.DeviceController, skip_reset: bool = False
) -> None:
    utils.ui.echo(get_string("act_flash_partitions_label_start"))

    partition_map = _collect_base_partitions()
    if not partition_map:
        raise FileNotFoundError(get_string("act_err_no_xml_dump"))

    labels = sorted(partition_map.keys())
    selected_bases = _prompt_partition_selection(labels)

    if not selected_bases:
        utils.ui.echo(get_string("act_op_cancel"))
        return

    utils.ui.clear()

    needs_slot = any(partition_map[base]["is_ab"] for base in selected_bases)
    slot_suffix = ""
    if needs_slot:
        while True:
            width = utils.ui.get_term_width()
            utils.ui.echo("\n" + "=" * width)
            utils.ui.echo(f"   {get_string('menu_select_slot')}")
            utils.ui.echo("=" * width + "\n")
            utils.ui.echo(f"   1. {get_string('menu_slot_a')}")
            utils.ui.echo(f"   2. {get_string('menu_slot_b')}\n")

            choice = utils.ui.prompt(get_string("prompt_select")).strip()
            if choice == "1":
                slot_suffix = "a"
                break
            elif choice == "2":
                slot_suffix = "b"
                break
            else:
                utils.ui.error(get_string("err_invalid_selection"))

    flash_plan = []
    missing_files = []

    for base in selected_bases:
        p_info = partition_map[base]
        if p_info["is_ab"]:
            target_slot = slot_suffix
            other_slot = "b" if target_slot == "a" else "a"

            target_entries = p_info[target_slot]
            other_entries = p_info[other_slot]

            for t_entry, o_entry in itertools.zip_longest(
                target_entries, other_entries
            ):
                if not t_entry:
                    utils.ui.error(
                        get_string("act_warn_missing_sector_info").format(
                            partition=f"{base}_{target_slot}"
                        )
                    )
                    continue

                filename = t_entry["filename"]
                if not filename and o_entry:
                    filename = o_entry["filename"]

                if not filename:
                    continue

                if not (const.IMAGE_DIR / filename).exists():
                    missing_files.append(filename)
                else:
                    flash_plan.append(
                        (
                            f"{base}_{target_slot}",
                            filename,
                            t_entry["lun"],
                            t_entry["start_sector"],
                        )
                    )
        else:
            for entry in p_info["none"]:
                filename = entry["filename"]
                if not filename:
                    continue
                if not (const.IMAGE_DIR / filename).exists():
                    missing_files.append(filename)
                else:
                    flash_plan.append(
                        (base, filename, entry["lun"], entry["start_sector"])
                    )

    if missing_files:
        unique_missing = sorted(set(missing_files))
        raise FileNotFoundError(
            get_string("act_err_selected_partitions_missing_images").format(
                files=", ".join(unique_missing)
            )
        )

    if not flash_plan:
        utils.ui.echo(get_string("act_op_cancel"))
        return

    ensure_edl_requirements()
    with dev.edl_session(auto_reset=not skip_reset) as port:
        for target_label, filename, lun, start_sector in flash_plan:
            image_path = const.IMAGE_DIR / filename

            utils.ui.echo(get_string("act_flashing_target").format(target=target_label))
            utils.ui.echo(
                get_string("device_flashing_part").format(
                    filename=image_path.name,
                    lun=lun,
                    start=start_sector,
                )
            )

            dev.edl.write_partition(
                port=port,
                image_path=image_path,
                lun=lun,
                start_sector=start_sector,
            )

    utils.ui.echo(get_string("act_write_finish"))


def ensure_loader_file() -> None:
    if not const.EDL_LOADER_FILE.exists():
        utils.ui.echo(
            get_string("act_err_loader_missing").format(
                name=const.EDL_LOADER_FILE.name, dir=const.IMAGE_DIR.name
            )
        )
        prompt = get_string("device_loader_prompt").format(
            loader=const.EDL_LOADER_FILENAME, folder=const.IMAGE_DIR.name
        )
        utils.wait_for_files(const.IMAGE_DIR, [const.EDL_LOADER_FILENAME], prompt)


def ensure_edl_requirements() -> None:
    ensure_loader_file()
    xml.ensure_xml_files()


def flash_partition_target(
    dev: device.DeviceController, port: str, target_name: str, image_path: Path
) -> None:
    utils.ui.echo(get_string("act_flashing_target").format(target=target_name))

    params = require_partition_params(target_name)
    utils.ui.echo(
        get_string("act_found_dump_info").format(
            xml=params["source_xml"], lun=params["lun"], start=params["start_sector"]
        )
    )

    utils.ui.echo(
        get_string("device_flashing_part").format(
            filename=image_path.name, lun=params["lun"], start=params["start_sector"]
        )
    )
    dev.edl.write_partition(
        port=port,
        image_path=image_path,
        lun=params["lun"],
        start_sector=params["start_sector"],
    )
    utils.ui.echo(get_string("device_flash_success").format(filename=image_path.name))


def dump_partitions(
    dev: device.DeviceController,
    skip_reset: bool = False,
    additional_targets: Optional[List[str]] = None,
    default_targets: bool = True,
) -> None:
    utils.ui.echo(get_string("act_start_dump"))

    const.BACKUP_DIR.mkdir(exist_ok=True)

    targets = []
    if default_targets:
        targets.extend(["devinfo", "persist"])

    if additional_targets:
        targets.extend(additional_targets)
        utils.ui.echo(
            get_string("act_ext_dump_targets").format(targets=", ".join(targets))
        )

    critical_targets = {"devinfo", "persist"} & set(targets)
    failed_targets: list[str] = []
    ensure_edl_requirements()
    with dev.edl_session(
        auto_reset=not skip_reset,
        reset_msg_key="act_reset_sys",
        skip_msg_key="act_skip_reset",
        post_sleep=15,
    ) as port:
        for target in targets:
            out_file = const.BACKUP_DIR / f"{target}.img"
            utils.ui.echo(get_string("act_prep_dump").format(target=target))

            try:
                params = require_partition_params(target)
                utils.ui.echo(
                    get_string("act_found_dump_info").format(
                        xml=params["source_xml"],
                        lun=params["lun"],
                        start=params["start_sector"],
                    )
                )

                utils.ui.echo(
                    get_string("device_dumping_part").format(
                        lun=params["lun"],
                        start=params["start_sector"],
                        num=params["num_sectors"],
                    )
                )

                dev.edl.read_partition(
                    port=port,
                    output_filename=str(out_file),
                    lun=params["lun"],
                    start_sector=params["start_sector"],
                    num_sectors=params["num_sectors"],
                )

                if params.get("size_in_kb"):
                    try:
                        expected_size = int(float(params["size_in_kb"]) * 1024)
                        actual_size = out_file.stat().st_size

                        if expected_size != actual_size:
                            raise RuntimeError(
                                get_string("act_err_dump_size_mismatch").format(
                                    target=target,
                                    expected=expected_size,
                                    actual=actual_size,
                                )
                            )
                    except (ValueError, OSError) as e:
                        utils.ui.echo(
                            get_string("act_skip_dump").format(
                                target=target, e=f"Size validation error: {e}"
                            )
                        )

                utils.ui.echo(
                    get_string("act_dump_success").format(
                        target=target, file=out_file.name
                    )
                )

            except (ValueError, FileNotFoundError) as e:
                utils.ui.echo(get_string("act_skip_dump").format(target=target, e=e))
                if target in critical_targets:
                    failed_targets.append(target)
            except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
                utils.ui.error(get_string("act_err_dump").format(target=target, e=e))
                if target in critical_targets:
                    failed_targets.append(target)

            utils.ui.echo(get_string("act_wait_stability"))
            time.sleep(5)

    if failed_targets:
        failed_targets = sorted(set(failed_targets))
        utils.ui.error(
            get_string("act_dump_failed").format(targets=", ".join(failed_targets))
        )
        raise RuntimeError(
            get_string("act_dump_failed").format(targets=", ".join(failed_targets))
        )

    utils.ui.echo(get_string("act_dump_ignore_warn"))
    utils.ui.echo(get_string("act_dump_finish"))
    utils.ui.echo(get_string("act_dump_saved").format(dir=const.BACKUP_DIR.name))


def _format_dp_folder_label(folder: Path) -> str:
    from ..patch.region import detect_country_codes

    codes = detect_country_codes(source_dir=folder)
    parts = []
    for fname in ["devinfo.img", "persist.img"]:
        code = codes.get(fname)
        label = Path(fname).stem
        parts.append(f"{label}: {code.upper() if code else '?'}")
    return f"{folder.name} [{', '.join(parts)}]"


def _find_dp_source_folders() -> List[Path]:
    backup_dirs = sorted(
        [
            d
            for d in const.BASE_DIR.iterdir()
            if d.is_dir()
            and d.name.startswith("backup_critical")
            and any(d.glob("*.img"))
        ],
        key=lambda d: d.name,
    )
    folders = list(backup_dirs)
    if const.OUTPUT_DP_DIR.exists() and any(const.OUTPUT_DP_DIR.glob("*.img")):
        folders.append(const.OUTPUT_DP_DIR)
    return folders


def _select_dp_source_folder() -> Path:
    folders = _find_dp_source_folders()

    if not folders:
        utils.ui.error(
            get_string("act_err_dp_folder").format(dir=const.OUTPUT_DP_DIR.name)
        )
        utils.ui.error(get_string("act_err_run_patch_first"))
        raise FileNotFoundError(
            get_string("act_err_dp_folder_nf").format(dir=const.OUTPUT_DP_DIR.name)
        )

    if len(folders) == 1:
        chosen = folders[0]
        utils.ui.echo(get_string("act_found_patched_folder").format(dir=chosen.name))
        return chosen

    utils.ui.clear()
    has_backup = any(f.name.startswith("backup_critical") for f in folders)
    if has_backup:
        utils.ui.echo(get_string("act_dp_backup_exists"))
    utils.ui.echo("")

    width = utils.ui.get_term_width()
    utils.ui.echo("=" * width)
    for i, folder in enumerate(folders, 1):
        label = _format_dp_folder_label(folder)
        utils.ui.echo(f"  {i}. {label}")
    utils.ui.echo("=" * width)
    utils.ui.echo("")

    while True:
        choice = utils.ui.prompt(get_string("prompt_select")).strip()
        try:
            idx = int(choice)
            if 1 <= idx <= len(folders):
                chosen = folders[idx - 1]
                utils.ui.clear()
                utils.ui.echo(
                    get_string("act_found_patched_folder").format(dir=chosen.name)
                )
                return chosen
        except ValueError:
            pass
        utils.ui.error(get_string("err_invalid_selection"))


def flash_partitions(
    dev: device.DeviceController, skip_reset: bool = False, skip_reset_edl: bool = False
) -> None:
    utils.ui.echo(get_string("act_start_write"))

    source_dir = _select_dp_source_folder()

    ensure_edl_requirements()
    with dev.edl_session(auto_reset=not skip_reset) as port:
        targets = ["devinfo", "persist"]

        for target in targets:
            image_path = source_dir / f"{target}.img"

            if not image_path.exists():
                utils.ui.echo(get_string(f"act_skip_{target}"))
                continue

            try:
                flash_partition_target(dev, port, target, image_path)

            except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
                utils.ui.error(get_string("act_err_edl_write").format(e=e))
                raise

    utils.ui.echo(get_string("act_write_finish"))


def write_anti_rollback(dev: device.DeviceController, skip_reset: bool = False) -> None:
    utils.ui.echo(get_string("act_start_arb_write"))

    boot_img = const.OUTPUT_ANTI_ROLLBACK_DIR / "boot.img"
    vbmeta_img = const.OUTPUT_ANTI_ROLLBACK_DIR / "vbmeta_system.img"

    if not boot_img.exists() or not vbmeta_img.exists():
        utils.ui.error(
            get_string("act_err_patched_missing").format(
                dir=const.OUTPUT_ANTI_ROLLBACK_DIR.name
            )
        )
        utils.ui.error(get_string("act_err_run_patch_arb"))
        raise FileNotFoundError(
            get_string("act_err_patched_missing_exc").format(
                dir=const.OUTPUT_ANTI_ROLLBACK_DIR.name
            )
        )
    utils.ui.echo(
        get_string("act_found_patched_folder").format(
            dir=const.OUTPUT_ANTI_ROLLBACK_DIR.name
        )
    )

    ensure_edl_requirements()

    utils.ui.echo(get_string("act_arb_write_step1"))
    utils.ui.echo(get_string("act_boot_fastboot"))
    dev.fastboot.wait_for_device()

    utils.ui.echo(get_string("device_get_slot_fastboot"))
    active_slot = dev.fastboot.get_slot_suffix()
    if active_slot:
        utils.ui.echo(get_string("act_slot_confirmed").format(slot=active_slot))
    else:
        utils.ui.echo(get_string("act_warn_slot_fail"))
        active_slot = ""

    target_boot = f"boot{active_slot}"
    target_vbmeta = f"vbmeta_system{active_slot}"

    utils.ui.echo(get_string("act_arb_write_step2"))
    if not dev.skip_adb:
        utils.ui.echo(get_string("act_manual_edl_now"))
        utils.ui.echo(get_string("act_manual_edl_hint"))

    with dev.edl_session(
        auto_reset=not skip_reset,
        reset_msg_key="act_arb_reset",
        skip_msg_key="act_arb_skip_reset",
    ) as port:
        try:
            utils.ui.echo(get_string("act_arb_write_step3").format(slot=active_slot))
            flash_partition_target(dev, port, target_boot, boot_img)
            flash_partition_target(dev, port, target_vbmeta, vbmeta_img)
        except (subprocess.CalledProcessError, FileNotFoundError, ValueError) as e:
            utils.ui.error(get_string("act_err_edl_write").format(e=e))
            raise

    utils.ui.echo(get_string("act_arb_write_finish"))


def _prepare_flash_files(skip_dp: bool = False) -> None:
    utils.ui.echo(get_string("act_copy_patched"))
    output_folders_to_copy = [
        const.OUTPUT_DIR,
        const.OUTPUT_ANTI_ROLLBACK_DIR,
        const.OUTPUT_XML_DIR,
    ]

    copied_count = 0
    for folder in output_folders_to_copy:
        if folder.exists():
            try:
                shutil.copytree(folder, const.IMAGE_DIR, dirs_exist_ok=True)
                utils.ui.echo(
                    get_string("act_copied_content").format(
                        src=folder.name, dst=const.IMAGE_DIR.name
                    )
                )
                copied_count += 1
            except (OSError, shutil.Error) as e:
                utils.ui.error(get_string("act_err_copy").format(name=folder.name, e=e))

    if not skip_dp:
        if const.OUTPUT_DP_DIR.exists():
            try:
                shutil.copytree(
                    const.OUTPUT_DP_DIR, const.IMAGE_DIR, dirs_exist_ok=True
                )
                utils.ui.echo(
                    get_string("act_copied_content").format(
                        src=const.OUTPUT_DP_DIR.name, dst=const.IMAGE_DIR.name
                    )
                )
                copied_count += 1
            except (OSError, shutil.Error) as e:
                utils.ui.error(
                    get_string("act_err_copy").format(
                        name=const.OUTPUT_DP_DIR.name, e=e
                    )
                )
        else:
            utils.ui.echo(
                get_string("act_skip_dp_copy").format(dir=const.OUTPUT_DP_DIR.name)
            )
    else:
        utils.ui.echo(get_string("act_req_skip_dp"))

    if copied_count == 0:
        utils.ui.echo(get_string("act_no_output_folders"))


def _select_flash_xmls(skip_dp: bool = False) -> Tuple[List[Path], List[Path]]:
    all_raw_xmls = sorted(list(const.IMAGE_DIR.glob("rawprogram*.xml")))
    patch_xmls = sorted(list(const.IMAGE_DIR.glob("patch*.xml")))

    raw_xmls = []
    for xml_file in all_raw_xmls:
        name = xml_file.name
        if "WIPE_PARTITIONS" in name or "BLANK_GPT" in name:
            continue
        if name == "rawprogram0.xml":
            continue
        raw_xmls.append(xml_file)

    persist_write_xml = const.IMAGE_DIR / "rawprogram_write_persist_unsparse0.xml"
    persist_save_xml = const.IMAGE_DIR / "rawprogram_save_persist_unsparse0.xml"
    raw_unsparse0 = const.IMAGE_DIR / "rawprogram_unsparse0.xml"
    raw_unsparse0_half = const.IMAGE_DIR / "rawprogram_unsparse0-half.xml"

    devinfo_write_xml = const.IMAGE_DIR / "rawprogram4_write_devinfo.xml"
    devinfo_original_xml = const.IMAGE_DIR / "rawprogram4.xml"

    has_patched_persist = (const.OUTPUT_DP_DIR / "persist.img").exists()

    raw_xmls = [
        x
        for x in raw_xmls
        if x.name
        not in [
            persist_write_xml.name,
            persist_save_xml.name,
            raw_unsparse0.name,
            raw_unsparse0_half.name,
        ]
    ]

    if persist_write_xml.exists() and has_patched_persist and not skip_dp:
        utils.ui.echo(get_string("act_use_patched_persist"))
        raw_xmls.append(persist_write_xml)
    elif persist_save_xml.exists():
        utils.ui.echo(get_string("act_skip_persist_flash"))
        raw_xmls.append(persist_save_xml)
    elif raw_unsparse0_half.exists():
        utils.ui.echo(get_string("act_using_xml_persist_fallback"))
        raw_xmls.append(raw_unsparse0_half)
    elif raw_unsparse0.exists():
        utils.ui.echo(get_string("act_using_xml_full_wipe"))
        raw_xmls.append(raw_unsparse0)

    has_patched_devinfo = (const.OUTPUT_DP_DIR / "devinfo.img").exists()

    if devinfo_write_xml.exists() and has_patched_devinfo and not skip_dp:
        utils.ui.echo(get_string("act_use_patched_devinfo"))
        raw_xmls = [x for x in raw_xmls if x.name != devinfo_original_xml.name]
        raw_xmls.append(devinfo_write_xml)
    else:
        if devinfo_write_xml.exists():
            utils.ui.echo(get_string("act_skip_devinfo_flash"))
            raw_xmls = [x for x in raw_xmls if x.name != devinfo_write_xml.name]

    raw_xmls.sort(key=lambda x: x.name)

    if not raw_xmls or not patch_xmls:
        utils.ui.echo(
            get_string("act_err_xml_missing").format(dir=const.IMAGE_DIR.name)
        )
        utils.ui.echo(get_string("act_err_flash_aborted"))
        raise FileNotFoundError(
            get_string("act_err_xml_missing_exc").format(dir=const.IMAGE_DIR.name)
        )

    return raw_xmls, patch_xmls


def flash_full_firmware(
    dev: device.DeviceController,
    skip_reset: bool = False,
    skip_reset_edl: bool = False,
    skip_dp: bool = False,
) -> None:
    utils.ui.echo(get_string("act_start_flash"))

    if not const.IMAGE_DIR.is_dir() or not any(const.IMAGE_DIR.iterdir()):
        utils.ui.echo(
            get_string("act_err_image_empty").format(dir=const.IMAGE_DIR.name)
        )
        utils.ui.echo(get_string("act_err_run_xml_mod"))
        raise FileNotFoundError(
            get_string("act_err_image_empty_exc").format(dir=const.IMAGE_DIR.name)
        )

    ensure_loader_file()

    if not skip_reset_edl:
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(get_string("act_warn_overwrite_1"))
        utils.ui.echo(get_string("act_warn_overwrite_2"))
        utils.ui.echo(get_string("act_warn_overwrite_3"))
        utils.ui.echo("=" * width + "\n")

        choice = ""
        while choice not in ["y", "n"]:
            choice = utils.ui.prompt(get_string("act_ask_continue")).lower().strip()

        if choice == "n":
            utils.ui.echo(get_string("act_op_cancel"))
            return

    _prepare_flash_files(skip_dp)

    raw_xmls, patch_xmls = _select_flash_xmls(skip_dp)

    utils.ui.echo(get_string("act_flash_step1"))

    with dev.edl_session(
        load_programmer=False,
        auto_reset=not skip_reset,
        reset_msg_key="act_reset_sys",
        skip_msg_key="act_skip_final_reset",
        pre_sleep=5,
    ) as port:
        try:
            dev.edl.flash_rawprogram(
                port, const.EDL_LOADER_FILE, "UFS", raw_xmls, patch_xmls
            )
        except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
            utils.ui.error(get_string("act_err_main_flash").format(e=e))
            utils.ui.error(
                get_string("err_detailed_traceback") + traceback.format_exc()
            )
            utils.ui.echo(get_string("act_warn_unstable"))
            raise

        utils.ui.echo(get_string("act_flash_step2"))
        if not skip_dp:
            try:
                (const.IMAGE_DIR / "devinfo.img").unlink(missing_ok=True)
                (const.IMAGE_DIR / "persist.img").unlink(missing_ok=True)
                utils.ui.echo(get_string("act_removed_temp_imgs"))
            except OSError as e:
                utils.ui.error(get_string("act_err_clean_imgs").format(e=e))

        if not skip_reset:
            utils.ui.echo(get_string("act_flash_step3"))

    if not skip_reset:
        utils.ui.echo(get_string("act_flash_finish"))
