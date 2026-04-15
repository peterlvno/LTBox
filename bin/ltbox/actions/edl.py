import itertools
import shutil
import subprocess
import traceback
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Tuple

from .. import constants as const
from .. import device, utils
from ..i18n import get_string
from ..menus.prompt_helpers import (
    prompt_choice,
    prompt_index_selection,
    prompt_multi_select_indices,
    prompt_yes_no,
)
from ..part.backups import find_dp_source_folders, format_dp_folder_label
from ..part.partition import require_partition_params
from ..part.service import EdlPartitionService
from ..part.xml_catalog import PartitionGroup, PartitionRecord, XmlCatalog
from . import xml


@dataclass(frozen=True)
class PartitionFlashTarget:
    target_name: str
    image_path: Path
    lun: str
    start_sector: str


@dataclass(frozen=True)
class FullFlashPlan:
    raw_xmls: Tuple[Path, ...]
    patch_xmls: Tuple[Path, ...]
    pre_erase: bool
    reset_after: bool


def _partition_service() -> EdlPartitionService:
    return EdlPartitionService(resolve_params=require_partition_params)


def _rawprogram_xml_paths() -> List[Path]:
    xml_files = sorted(const.IMAGE_DIR.glob("rawprogram*.xml"))
    if xml_files:
        return xml_files
    return sorted(const.OUTPUT_XML_DIR.glob("rawprogram*.xml"))


def _collect_partition_groups() -> Dict[str, PartitionGroup]:
    xml.ensure_xml_files()
    return XmlCatalog.from_paths(
        _rawprogram_xml_paths(),
        on_error=utils.ui.error,
    ).group_by_base_label(with_files_only=True)


def _partition_record_to_entry(record: PartitionRecord) -> Dict[str, Optional[str]]:
    return {
        "filename": record.filename,
        "lun": record.lun,
        "start_sector": record.start_sector,
    }


def _collect_base_partitions() -> Dict[str, Any]:
    partition_groups = _collect_partition_groups()
    return {
        base_label: {
            "is_ab": group.is_ab,
            "a": [_partition_record_to_entry(record) for record in group.a],
            "b": [_partition_record_to_entry(record) for record in group.b],
            "none": [_partition_record_to_entry(record) for record in group.none],
            "has_files": group.has_files,
        }
        for base_label, group in partition_groups.items()
    }


def _prompt_slot_selection() -> str:
    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "=" * width)
    utils.ui.echo(f"   {get_string('menu_select_slot')}")
    utils.ui.echo("=" * width + "\n")
    utils.ui.echo(f"   1. {get_string('menu_slot_a')}")
    utils.ui.echo(f"   2. {get_string('menu_slot_b')}\n")

    choice = prompt_index_selection(
        get_string("prompt_select"),
        max_index=2,
        error_message=get_string("err_invalid_selection"),
        input_func=utils.ui.prompt,
        error_func=utils.ui.error,
    )
    return "a" if choice == 1 else "b"


def _resolve_selected_partition_slot(
    selected_bases: List[str],
    partition_groups: Dict[str, PartitionGroup],
) -> str:
    needs_slot = any(partition_groups[base].is_ab for base in selected_bases)
    if not needs_slot:
        return ""
    return _prompt_slot_selection()


def _build_selected_partition_flash_targets(
    selected_bases: List[str],
    partition_groups: Dict[str, PartitionGroup],
    slot_suffix: str,
) -> List[PartitionFlashTarget]:
    flash_targets: List[PartitionFlashTarget] = []
    missing_files: List[str] = []

    for base in selected_bases:
        partition_group = partition_groups[base]
        if partition_group.is_ab:
            target_slot = slot_suffix
            other_slot = "b" if target_slot == "a" else "a"

            target_records = partition_group.slot_records(target_slot)
            other_records = partition_group.slot_records(other_slot)

            for target_record, other_record in itertools.zip_longest(
                target_records, other_records
            ):
                if target_record is None:
                    utils.ui.error(
                        get_string("act_warn_missing_sector_info").format(
                            partition=f"{base}_{target_slot}"
                        )
                    )
                    continue

                filename = target_record.filename
                if not filename and other_record is not None:
                    filename = other_record.filename

                if not filename:
                    continue

                image_path = const.IMAGE_DIR / filename
                if not image_path.exists():
                    missing_files.append(filename)
                    continue

                flash_targets.append(
                    PartitionFlashTarget(
                        target_name=f"{base}_{target_slot}",
                        image_path=image_path,
                        lun=target_record.lun or "",
                        start_sector=target_record.start_sector or "",
                    )
                )
        else:
            for record in partition_group.none:
                if not record.filename:
                    continue

                image_path = const.IMAGE_DIR / record.filename
                if not image_path.exists():
                    missing_files.append(record.filename)
                    continue

                flash_targets.append(
                    PartitionFlashTarget(
                        target_name=base,
                        image_path=image_path,
                        lun=record.lun or "",
                        start_sector=record.start_sector or "",
                    )
                )

    if missing_files:
        unique_missing = sorted(set(missing_files))
        raise FileNotFoundError(
            get_string("act_err_selected_partitions_missing_images").format(
                files=", ".join(unique_missing)
            )
        )

    return flash_targets


def _execute_partition_flash_targets(
    dev: device.DeviceController,
    port: str,
    flash_targets: List[PartitionFlashTarget],
) -> None:
    for flash_target in flash_targets:
        utils.ui.echo(
            get_string("act_flashing_img_start").format(
                filename=flash_target.image_path.name
            )
        )
        utils.ui.echo(
            get_string("device_flashing_part").format(
                lun=flash_target.lun,
                start_sector=flash_target.start_sector,
                label=flash_target.target_name,
            )
        )

        dev.edl.write_partition(
            port=port,
            image_path=flash_target.image_path,
            lun=flash_target.lun,
            start_sector=flash_target.start_sector,
            partition_name=flash_target.target_name,
        )
        utils.ui.echo(
            get_string("device_flash_success").format(
                filename=flash_target.image_path.name
            )
        )


def _build_full_flash_plan(
    skip_dp: bool,
    wipe_mode: bool,
    skip_reset: bool,
) -> FullFlashPlan:
    _prepare_flash_files(skip_dp)
    raw_xmls, patch_xmls = _select_flash_xmls(skip_dp)
    return FullFlashPlan(
        raw_xmls=tuple(raw_xmls),
        patch_xmls=tuple(patch_xmls),
        pre_erase=wipe_mode,
        reset_after=not skip_reset,
    )


def _prompt_partition_selection(labels: List[str]) -> List[str]:
    def _render(selected_offsets: Set[int]) -> None:
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(f"   {get_string('act_flash_partitions_label_title')}")
        utils.ui.echo("=" * width + "\n")

        count = len(labels)
        for i in range(0, count, 2):
            label1 = labels[i]
            mark1 = " [v]" if i in selected_offsets else ""
            item1 = f" {i + 1:3d}. {label1}{mark1}"

            if i + 1 < count:
                label2 = labels[i + 1]
                mark2 = " [v]" if (i + 1) in selected_offsets else ""
                item2 = f"{i + 2:3d}. {label2}{mark2}"
                utils.ui.echo(f"  {item1:<38} {item2}")
            else:
                utils.ui.echo(f"  {item1}")

        utils.ui.echo(f"   f. {get_string('act_flash_partitions_select_done')}")
        utils.ui.echo(f"   c. {get_string('cancel')}")
        utils.ui.echo("\n" + "=" * width + "\n")

    selected_offsets = prompt_multi_select_indices(
        get_string("prompt_select"),
        item_count=len(labels),
        render_func=_render,
        input_func=utils.ui.prompt,
        error_message=get_string("err_invalid_selection"),
        error_func=utils.ui.error,
        pause_func=lambda: input(get_string("press_enter_to_continue")),
        clear_func=utils.ui.clear,
    )
    if selected_offsets is None:
        return []
    return [labels[index] for index in selected_offsets]


def flash_selected_partitions(
    dev: device.DeviceController, skip_reset: bool = False
) -> None:
    utils.ui.echo(get_string("act_flash_partitions_label_start"))

    partition_groups = _collect_partition_groups()
    if not partition_groups:
        raise FileNotFoundError(get_string("act_err_no_xml_dump"))

    labels = sorted(partition_groups.keys())
    selected_bases = _prompt_partition_selection(labels)

    if not selected_bases:
        utils.ui.echo(get_string("act_op_cancel"))
        return

    utils.ui.clear()

    slot_suffix = _resolve_selected_partition_slot(selected_bases, partition_groups)
    utils.ui.clear()

    flash_targets = _build_selected_partition_flash_targets(
        selected_bases,
        partition_groups,
        slot_suffix,
    )

    if not flash_targets:
        utils.ui.echo(get_string("act_op_cancel"))
        return

    ensure_edl_requirements()
    with dev.edl_session(auto_reset=not skip_reset) as port:
        _execute_partition_flash_targets(dev, port, flash_targets)

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
    _partition_service().flash_partition(dev, port, target_name, image_path)


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
                service = _partition_service()
                params = service.get_params(target)
                service.dump_partition(dev, port, target, out_file, params=params)

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

    if failed_targets:
        failed_targets = sorted(set(failed_targets))
        utils.ui.error(
            get_string("act_dump_failed").format(targets=", ".join(failed_targets))
        )
        raise RuntimeError(
            get_string("act_dump_failed").format(targets=", ".join(failed_targets))
        )

    utils.ui.echo(get_string("act_dump_finish"))
    utils.ui.echo(get_string("act_dump_saved").format(dir=const.BACKUP_DIR.name))


def _find_dp_source_folders() -> List[Path]:
    return find_dp_source_folders(const.BASE_DIR, const.OUTPUT_DP_DIR)


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
        label = format_dp_folder_label(folder)
        utils.ui.echo(f"  {i}. {label}")
    utils.ui.echo("=" * width)
    utils.ui.echo("")

    idx = prompt_index_selection(
        get_string("prompt_select"),
        max_index=len(folders),
        error_message=get_string("err_invalid_selection"),
        input_func=utils.ui.prompt,
        error_func=utils.ui.error,
    )
    chosen = folders[idx - 1]
    utils.ui.clear()
    utils.ui.echo(get_string("act_found_patched_folder").format(dir=chosen.name))
    return chosen


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


def _sync_tree(src_dir: Path, dst_dir: Path) -> None:
    dst_dir.mkdir(parents=True, exist_ok=True)
    for src_file in src_dir.rglob("*"):
        if not src_file.is_file():
            continue
        rel = src_file.relative_to(src_dir)
        dst_file = dst_dir / rel
        if dst_file.exists():
            src_stat = src_file.stat()
            dst_stat = dst_file.stat()
            if (
                src_stat.st_size == dst_stat.st_size
                and src_stat.st_mtime <= dst_stat.st_mtime
            ):
                continue
        dst_file.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src_file, dst_file)


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
                _sync_tree(folder, const.IMAGE_DIR)
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
                _sync_tree(const.OUTPUT_DP_DIR, const.IMAGE_DIR)
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
            xml.create_write_xmls_for_dp()
        else:
            utils.ui.echo(
                get_string("act_skip_dp_copy").format(dir=const.OUTPUT_DP_DIR.name)
            )
    else:
        utils.ui.echo(get_string("act_req_skip_dp"))

    if copied_count == 0:
        utils.ui.echo(get_string("act_no_output_folders"))


def _resolve_persist_xml(raw_xmls: List[Path], skip_dp: bool) -> List[Path]:
    persist_write_xml = const.IMAGE_DIR / "rawprogram_write_persist_unsparse0.xml"
    persist_save_xml = const.IMAGE_DIR / "rawprogram_save_persist_unsparse0.xml"
    persist_save_ota_xml = const.IMAGE_DIR / "rawprogram_save_persist_ota_unsparse0.xml"
    raw_unsparse0 = const.IMAGE_DIR / "rawprogram_unsparse0.xml"
    raw_unsparse0_half = const.IMAGE_DIR / "rawprogram_unsparse0-half.xml"

    excluded_names = {
        persist_write_xml.name,
        persist_save_xml.name,
        persist_save_ota_xml.name,
        raw_unsparse0.name,
        raw_unsparse0_half.name,
    }
    raw_xmls = [x for x in raw_xmls if x.name not in excluded_names]

    has_patched_persist = (const.OUTPUT_DP_DIR / "persist.img").exists()
    if persist_write_xml.exists() and has_patched_persist and not skip_dp:
        utils.ui.echo(get_string("act_use_patched_persist"))
        raw_xmls.append(persist_write_xml)
    elif persist_save_ota_xml.exists():
        utils.ui.echo(get_string("act_skip_persist_flash"))
        raw_xmls.append(persist_save_ota_xml)
    elif persist_save_xml.exists():
        utils.ui.echo(get_string("act_skip_persist_flash"))
        raw_xmls.append(persist_save_xml)
    elif raw_unsparse0_half.exists():
        utils.ui.echo(get_string("act_using_xml_persist_fallback"))
        raw_xmls.append(raw_unsparse0_half)
    elif raw_unsparse0.exists():
        utils.ui.echo(get_string("act_using_xml_full_wipe"))
        raw_xmls.append(raw_unsparse0)

    return raw_xmls


def _resolve_devinfo_xml(raw_xmls: List[Path], skip_dp: bool) -> List[Path]:
    devinfo_write_xml = const.IMAGE_DIR / "rawprogram4_write_devinfo.xml"
    devinfo_original_xml = const.IMAGE_DIR / "rawprogram4.xml"
    has_patched_devinfo = (const.OUTPUT_DP_DIR / "devinfo.img").exists()

    if devinfo_write_xml.exists() and has_patched_devinfo and not skip_dp:
        utils.ui.echo(get_string("act_use_patched_devinfo"))
        raw_xmls = [x for x in raw_xmls if x.name != devinfo_original_xml.name]
        raw_xmls.append(devinfo_write_xml)
    else:
        if devinfo_write_xml.exists():
            utils.ui.echo(get_string("act_skip_devinfo_flash"))
            raw_xmls = [x for x in raw_xmls if x.name != devinfo_write_xml.name]

    return raw_xmls


_DP_FILENAMES = {"persist.img", "devinfo.img"}


def _verify_no_dp_filenames(raw_xmls: List[Path]) -> None:
    """Warn if any selected rawprogram XML references persist.img or devinfo.img.

    Called when skip_dp is True to catch accidental inclusion of DP images
    in the flash plan.
    """
    for xml_path in raw_xmls:
        try:
            tree = ET.parse(xml_path)
            for prog in tree.getroot().findall("program"):
                fname = prog.get("filename", "").strip()
                if fname.lower() in _DP_FILENAMES:
                    utils.ui.warn(
                        get_string("act_warn_dp_in_xml").format(
                            filename=fname, xml=xml_path.name
                        )
                    )
        except ET.ParseError:
            pass


def _select_flash_xmls(skip_dp: bool = False) -> Tuple[List[Path], List[Path]]:
    all_raw_xmls = sorted(list(const.IMAGE_DIR.glob("rawprogram*.xml")))
    patch_xmls = sorted(list(const.IMAGE_DIR.glob("patch*.xml")))

    raw_xmls = [
        x
        for x in all_raw_xmls
        if "WIPE_PARTITIONS" not in x.name
        and "BLANK_GPT" not in x.name
        and x.name != "rawprogram0.xml"
    ]

    raw_xmls = _resolve_persist_xml(raw_xmls, skip_dp)
    raw_xmls = _resolve_devinfo_xml(raw_xmls, skip_dp)
    raw_xmls.sort(key=lambda x: x.name)

    if skip_dp:
        _verify_no_dp_filenames(raw_xmls)

    if not raw_xmls or not patch_xmls:
        utils.ui.echo(
            get_string("act_err_xml_missing").format(dir=const.IMAGE_DIR.name)
        )
        utils.ui.echo(get_string("act_err_flash_aborted"))
        raise FileNotFoundError(
            get_string("act_err_xml_missing_exc").format(dir=const.IMAGE_DIR.name)
        )

    return raw_xmls, patch_xmls


def _prompt_flash_wipe_mode() -> Optional[bool]:
    utils.ui.clear()
    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "=" * width)
    utils.ui.echo(f"   {get_string('task_title_flash_full_firmware')}")
    utils.ui.echo("=" * width + "\n")
    utils.ui.echo(f"   1. {get_string('menu_main_install_wipe')}")
    utils.ui.echo(f"   2. {get_string('menu_main_install_keep')}")
    utils.ui.echo(f"   c. {get_string('cancel')}\n")

    choice = prompt_choice(
        get_string("prompt_select"),
        {"1", "2", "c"},
        input_func=utils.ui.prompt,
        error_message=get_string("err_invalid_selection"),
        error_func=utils.ui.error,
        normalize=lambda value: value.strip().lower(),
    )
    if choice == "1":
        return True
    if choice == "2":
        return False
    return None


def _resolve_flash_wipe_mode(wipe: Optional[bool]) -> Optional[bool]:
    if wipe is not None:
        return bool(wipe)
    return _prompt_flash_wipe_mode()


def _validate_image_dir_for_flash() -> None:
    if const.IMAGE_DIR.is_dir() and any(const.IMAGE_DIR.iterdir()):
        return

    utils.ui.echo(get_string("act_err_image_empty").format(dir=const.IMAGE_DIR.name))
    utils.ui.echo(get_string("act_err_run_xml_mod"))
    raise FileNotFoundError(
        get_string("act_err_image_empty_exc").format(dir=const.IMAGE_DIR.name)
    )


def _confirm_full_flash_overwrite(
    skip_reset_edl: bool, skip_confirm: bool = False
) -> bool:
    if skip_reset_edl or skip_confirm:
        return True

    width = utils.ui.get_term_width()
    utils.ui.echo("\n" + "=" * width)
    utils.ui.warn(get_string("act_warn_overwrite_1"))
    utils.ui.warn(get_string("act_warn_overwrite_2"))
    utils.ui.warn(get_string("act_warn_overwrite_3"))
    utils.ui.echo("=" * width + "\n")

    result = prompt_yes_no(
        get_string("act_ask_continue"),
        input_func=utils.ui.prompt,
        error_message=get_string("err_invalid_selection"),
        error_func=utils.ui.error,
    )
    return bool(result)


def _execute_full_flash_plan(
    dev: device.DeviceController,
    flash_plan: FullFlashPlan,
    skip_reset: bool,
    skip_dp: bool,
) -> None:
    utils.ui.echo(get_string("act_flash_step1"))

    with dev.edl_session(
        load_programmer=False,
        auto_reset=False,
        skip_msg_key="act_skip_final_reset" if skip_reset else "",
    ) as port:
        try:
            dev.edl.flash_rawprogram(
                port,
                const.EDL_LOADER_FILE,
                "UFS",
                list(flash_plan.raw_xmls),
                list(flash_plan.patch_xmls),
                pre_erase=flash_plan.pre_erase,
                reset_after=flash_plan.reset_after,
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

        # qdl-rs already prints "Resetting to system" when reset_after is set.


def flash_full_firmware(
    dev: device.DeviceController,
    skip_reset: bool = False,
    skip_reset_edl: bool = False,
    skip_dp: bool = False,
    wipe: Optional[bool] = None,
    skip_confirm: bool = False,
) -> None:
    utils.ui.echo(get_string("act_start_flash"))

    _validate_image_dir_for_flash()
    ensure_loader_file()

    wipe_mode = _resolve_flash_wipe_mode(wipe)
    if wipe_mode is None:
        utils.ui.echo(get_string("act_op_cancel"))
        return

    if not _confirm_full_flash_overwrite(skip_reset_edl, skip_confirm):
        utils.ui.echo(get_string("act_op_cancel"))
        return

    flash_plan = _build_full_flash_plan(
        skip_dp=skip_dp,
        wipe_mode=wipe_mode,
        skip_reset=skip_reset,
    )
    _execute_full_flash_plan(
        dev=dev,
        flash_plan=flash_plan,
        skip_reset=skip_reset,
        skip_dp=skip_dp,
    )

    if not skip_reset:
        utils.ui.echo(get_string("act_flash_finish"))
