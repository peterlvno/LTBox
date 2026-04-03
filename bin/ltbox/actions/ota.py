import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import List, Optional

from .. import constants as const
from .. import ota_super, partition, update_engine_payload, utils
from ..errors import MissingFileError, ToolError
from ..i18n import get_string
from ..patch.avb import extract_image_avb_info, resign_avb_image
from ..process_runner import CommandRunner, RunOptions
from ..prompt_helpers import prompt_yes_no
from ..xml_catalog import PartitionGroup, XmlCatalog

_OTA_AVB_RESIGN_TARGETS = (
    "boot",
    "dtbo",
    "init_boot",
    "odm",
    "product",
    "pvmfw",
    "recovery",
    "system_ext",
    "system",
    "system_dlkm",
    "vendor",
    "vendor_boot",
    "vendor_dlkm",
    "vbmeta_system",
)
_OTA_AVB_DEFAULT_RESIGN_KEY_NAME = "testkey_rsa4096.pem"
_OTA_AVB_DEFAULT_RSA_BITS = 4096
_OTA_AVB_SPECIAL_RESIGN_KEYS = {
    "vbmeta_system": ("testkey_rsa2048.pem", 2048),
}


def _find_zip_files() -> List[Path]:
    const.OTA_DIR.mkdir(parents=True, exist_ok=True)
    return sorted(const.OTA_DIR.glob("*.zip"))


def _select_zip_file(zip_files: List[Path]) -> Path:
    if len(zip_files) == 1:
        utils.ui.echo(get_string("ota_zip_found").format(name=zip_files[0].name))
        return zip_files[0]

    utils.ui.echo(get_string("ota_multiple_zips"))
    for i, zf in enumerate(zip_files, 1):
        utils.ui.echo(f"   {i}. {zf.name}")
    utils.ui.echo("")

    while True:
        choice = utils.ui.prompt(get_string("prompt_select")).strip()
        try:
            idx = int(choice)
            if 1 <= idx <= len(zip_files):
                return zip_files[idx - 1]
        except ValueError:
            pass
        utils.ui.error(get_string("err_invalid_selection"))


def _extract_payload_bin(zip_path: Path, working_dir: Path) -> Path:
    utils.ui.echo(get_string("ota_extracting_zip").format(name=zip_path.name))

    with zipfile.ZipFile(zip_path, "r") as zf:
        zf.extractall(working_dir)

    payload_bin = working_dir / "payload.bin"
    if not payload_bin.exists():
        found = list(working_dir.rglob("payload.bin"))
        if not found:
            raise MissingFileError(get_string("ota_err_no_payload_bin"))
        payload_bin = found[0]

    utils.ui.echo(get_string("ota_payload_found").format(path=payload_bin.name))
    return payload_bin


def _get_payload_partition_infos(
    payload_bin: Path,
) -> List[update_engine_payload.PayloadPartitionInfo]:
    partition_infos = update_engine_payload.get_partition_infos(payload_bin)
    if not partition_infos:
        raise ToolError(get_string("ota_err_no_diff_partitions"))
    return partition_infos


def _build_partition_file_map(
    partitions: List[str], catalog: XmlCatalog
) -> dict[str, Path]:
    """Map partition names to actual filenames via rawprogram*.xml lookup.

    The XML ``<program>`` entries use ``label`` for the partition name (with _a/_b
    suffix for A/B slots) and ``filename`` for the real file on disk.
    """
    file_map: dict[str, Path] = {}
    for name in partitions:
        record = catalog.find_partition(name) or catalog.find_partition(f"{name}_a")
        if record and record.filename:
            path = const.IMAGE_DIR / record.filename
            if path.exists():
                file_map[name] = path
    return file_map


def _resolve_output_filenames(
    partitions: List[str], catalog: XmlCatalog
) -> tuple[dict[str, str], dict[str, str]]:
    output_filenames: dict[str, str] = {}
    xml_filename_updates: dict[str, str] = {}

    for name in partitions:
        record = catalog.find_partition(name) or catalog.find_partition(f"{name}_a")
        if record and record.filename:
            output_filename = Path(record.filename).name
            xml_filename_updates[record.filename] = output_filename
        else:
            output_filename = f"{name}.img"

        output_filenames[name] = output_filename

    return output_filenames, xml_filename_updates


def _verify_source_images(partitions: List[str], file_map: dict[str, Path]) -> None:
    missing = [name for name in partitions if name not in file_map]
    if missing:
        raise MissingFileError(
            get_string("ota_err_missing_images").format(
                files=", ".join(missing), dir=const.IMAGE_DIR.name
            )
        )


def _load_xml_catalog() -> tuple[list[Path], XmlCatalog]:
    xml_paths = partition.scan_and_decrypt_xmls()
    if not xml_paths:
        raise MissingFileError(get_string("act_err_no_xml_dump"))
    return xml_paths, XmlCatalog.from_paths(xml_paths, on_error=utils.ui.error)


def _find_super_group(catalog: XmlCatalog) -> Optional[PartitionGroup]:
    return catalog.group_by_base_label(with_files_only=True).get("super")


def _resolve_dynamic_partition_sources(
    selected: List[str],
    file_map: dict[str, Path],
    super_group: Optional[PartitionGroup],
    working_dir: Path,
) -> tuple[Optional[ota_super.SuperLayout], Optional[Path]]:
    missing = [name for name in selected if name not in file_map]
    if not missing or super_group is None:
        _verify_source_images(selected, file_map)
        return None, None

    layout = ota_super.parse_super_layout(super_group.none, const.IMAGE_DIR)
    dynamic_targets = [
        name for name in missing if name in layout.dynamic_partition_names
    ]

    dynamic_dir: Optional[Path] = None
    if dynamic_targets:
        dynamic_dir = working_dir / "dynamic_old"
        ota_super.extract_partition_images(layout, dynamic_dir)
        for name in dynamic_targets:
            image_path = dynamic_dir / f"{name}.img"
            if image_path.exists():
                file_map[name] = image_path

    _verify_source_images(selected, file_map)
    return layout, dynamic_dir


def _run_differential_patch(
    payload_bin: Path,
    partitions: List[str],
    output_dir: Path,
    file_map: dict[str, Path],
    new_sizes: dict[str, int],
    output_filenames: dict[str, str],
) -> None:
    utils.recreate_dir(output_dir)

    images_arg = ",".join(partitions)
    utils.ui.echo(get_string("ota_running_patch").format(images=images_arg))

    new_images = []
    for name in partitions:
        image_path = output_dir / output_filenames.get(name, f"{name}.img")
        image_path.parent.mkdir(parents=True, exist_ok=True)
        with open(image_path, "wb") as f:
            f.truncate(new_sizes[name])
        new_images.append(image_path)

    runner = CommandRunner()
    try:
        delta_generator_command = [
            *_resolve_delta_generator_command(),
            f"-in_file={_windows_to_wsl_path(payload_bin)}",
            f"-partition_names={':'.join(partitions)}",
            "-new_partitions="
            + ":".join(_windows_to_wsl_path(path) for path in new_images),
            "-old_partitions="
            + ":".join(_windows_to_wsl_path(file_map[name]) for name in partitions),
        ]
        runner.run(
            delta_generator_command,
            options=RunOptions(stream=True, check=True),
        )
    except subprocess.CalledProcessError as e:
        if output_dir.exists():
            shutil.rmtree(output_dir)
        raise ToolError(get_string("ota_err_patch_failed").format(e=e)) from e

    utils.ui.echo(get_string("ota_patch_complete").format(dir=output_dir.name))


def _resolve_otatools_linux_command(tool_path: Path) -> list[str]:
    if not tool_path.exists():
        raise ToolError(
            "Required OTA tool missing: "
            f"{tool_path}. Re-download or re-extract the LTBox release package."
        )

    wsl_exe = shutil.which("wsl.exe")
    if not wsl_exe:
        raise ToolError(
            "WSL is required for OTA super rebuilding. Install WSL and re-run LTBox."
        )

    ld_library_paths = []
    for lib_dir in (const.OTATOOLS_LINUX_LIB64_DIR, const.OTATOOLS_LINUX_LIB_DIR):
        if lib_dir.exists():
            ld_library_paths.append(_windows_to_wsl_path(lib_dir))

    command = [wsl_exe, "--exec", "/usr/bin/env"]
    if ld_library_paths:
        command.append(f"LD_LIBRARY_PATH={':'.join(ld_library_paths)}")
    command.append(_windows_to_wsl_path(tool_path))
    return command


def _resolve_lpmake_command() -> list[str]:
    return _resolve_otatools_linux_command(const.OTATOOLS_LPMAKE)


def _resolve_delta_generator_command() -> list[str]:
    return _resolve_otatools_linux_command(const.OTATOOLS_DELTA_GENERATOR)


def _windows_to_wsl_path(path: Path) -> str:
    resolved = str(Path(path).resolve())
    if len(resolved) < 2 or resolved[1] != ":":
        raise ToolError(f"Unable to translate non-Windows path for WSL: {resolved}")

    drive = resolved[0].lower()
    tail = resolved[2:].replace("\\", "/").lstrip("/")
    return f"/mnt/{drive}/{tail}"


def _copy_flash_xmls(
    output_dir: Path,
    rawprogram_paths: List[Path],
    xml_filename_updates: dict[str, str],
) -> None:
    patch_paths: list[Path] = []
    seen_patch_names: set[str] = set()
    candidate_dirs = {const.IMAGE_DIR, *[path.parent for path in rawprogram_paths]}
    for candidate_dir in candidate_dirs:
        for patch_path in sorted(candidate_dir.glob("patch*.xml")):
            if patch_path.name in seen_patch_names:
                continue
            seen_patch_names.add(patch_path.name)
            patch_paths.append(patch_path)

    copied_xmls = ota_super.copy_flash_xmls(rawprogram_paths, patch_paths, output_dir)
    ota_super.rewrite_xml_filenames(copied_xmls, xml_filename_updates)
    ota_super.create_keep_data_ota_xml(output_dir)


def _resolve_ota_resign_targets(
    output_dir: Path,
    output_filenames: dict[str, str],
) -> dict[str, Path]:
    targets: dict[str, Path] = {}
    for partition_name in _OTA_AVB_RESIGN_TARGETS:
        image_name = output_filenames.get(partition_name, f"{partition_name}.img")
        image_path = output_dir / image_name
        if image_path.exists():
            targets[partition_name] = image_path
    return targets


def _confirm_ota_output_resign(candidate_paths: dict[str, Path]) -> bool:
    if not candidate_paths:
        return False

    utils.ui.echo("")
    utils.ui.echo(
        get_string("ota_resign_ready").format(
            images=", ".join(path.name for path in candidate_paths.values())
        )
    )
    return bool(
        prompt_yes_no(
            get_string("ota_resign_prompt"),
            input_func=utils.ui.prompt,
            error_message=get_string("act_invalid_selection"),
            error_func=utils.ui.error,
        )
    )


def _resolve_ota_testkey_path(key_name: str) -> Path:
    key_path = const.TOOLS_DIR / key_name
    if not key_path.exists():
        raise MissingFileError(
            get_string("ota_err_missing_resign_key").format(path=key_path)
        )
    return key_path


def _resolve_testkey_resign_algorithm(
    original_algorithm: str,
    rsa_bits: int,
) -> str:
    normalized = original_algorithm.upper()
    if normalized == "NONE":
        return normalized

    parts = normalized.split("_")
    if (
        len(parts) != 2
        or not parts[0].startswith("SHA")
        or not parts[1].startswith("RSA")
    ):
        raise ToolError(
            get_string("ota_err_unsupported_resign_algorithm").format(
                algorithm=original_algorithm
            )
        )
    return f"{parts[0]}_RSA{rsa_bits}"


def _resolve_ota_resign_policy(
    partition_name: str, original_algorithm: str
) -> tuple[Path, str]:
    key_name, rsa_bits = _OTA_AVB_SPECIAL_RESIGN_KEYS.get(
        partition_name,
        (_OTA_AVB_DEFAULT_RESIGN_KEY_NAME, _OTA_AVB_DEFAULT_RSA_BITS),
    )
    return (
        _resolve_ota_testkey_path(key_name),
        _resolve_testkey_resign_algorithm(original_algorithm, rsa_bits),
    )


def _resign_incremental_ota_outputs(candidate_paths: dict[str, Path]) -> None:
    if not candidate_paths:
        return

    resigned_count = 0
    skipped_none_count = 0
    skipped_unreadable_count = 0

    utils.ui.echo(get_string("ota_resign_scanning"))

    for partition_name, image_path in candidate_paths.items():
        utils.ui.echo(get_string("ota_resign_scan_image").format(name=image_path.name))
        try:
            image_info = extract_image_avb_info(image_path)
        except subprocess.CalledProcessError as e:
            skipped_unreadable_count += 1
            utils.ui.echo(
                get_string("ota_resign_skip_unreadable").format(
                    name=image_path.name, e=e
                )
            )
            continue

        algorithm = str(image_info.get("algorithm", "NONE")).upper()
        if algorithm == "NONE":
            skipped_none_count += 1
            utils.ui.echo(
                get_string("ota_resign_skip_none").format(name=image_path.name)
            )
            continue

        key_path, resign_algorithm = _resolve_ota_resign_policy(
            partition_name,
            algorithm,
        )
        resign_avb_image(
            image_path=image_path,
            key_file=key_path,
            algorithm=resign_algorithm,
        )
        resigned_count += 1
        utils.ui.echo(
            get_string("ota_resign_done_image").format(
                name=image_path.name,
                key=key_path.name,
                algorithm=resign_algorithm,
            )
        )

    utils.ui.echo(
        get_string("ota_resign_summary").format(
            resigned=resigned_count,
            skipped_none=skipped_none_count,
            skipped_scan=skipped_unreadable_count,
        )
    )


def _confirm_dynamic_super_rebuild() -> bool:
    utils.ui.echo("")
    utils.ui.echo(
        get_string("ota_super_rebuild_ready").format(dir=const.IMAGE_NEW_DIR.name)
    )
    result = prompt_yes_no(
        get_string("ota_super_rebuild_prompt"),
        input_func=utils.ui.prompt,
        error_message=get_string("act_invalid_selection"),
        error_func=utils.ui.error,
    )
    return bool(result)


def _rebuild_dynamic_super(
    layout: ota_super.SuperLayout,
    extracted_dynamic_dir: Path,
) -> None:
    dynamic_build_dir = const.OTA_WORKING_DIR / "dynamic_build"
    if dynamic_build_dir.exists():
        shutil.rmtree(dynamic_build_dir)
    shutil.copytree(extracted_dynamic_dir, dynamic_build_dir)

    for partition_name in sorted(layout.dynamic_partition_names):
        patched_image = const.IMAGE_NEW_DIR / f"{partition_name}.img"
        if patched_image.exists():
            shutil.copy2(patched_image, dynamic_build_dir / patched_image.name)

    rebuilt_super = const.OTA_WORKING_DIR / "super_rebuilt.img"
    lpmake_parts = _resolve_lpmake_command()
    lpmake_command = [
        *lpmake_parts[:-1],
        *ota_super.build_lpmake_command(
            layout,
            dynamic_build_dir,
            rebuilt_super,
            lpmake_parts[-1],
            _windows_to_wsl_path,
        ),
    ]

    CommandRunner().run(
        lpmake_command,
        options=RunOptions(stream=True, check=True),
    )
    rebuilt_layout = ota_super.parse_full_super_image(
        rebuilt_super,
        start_sector=layout.chunks[0].start_sector,
        sector_size_bytes=layout.chunks[0].sector_size_bytes,
    )
    rebuilt_chunks = ota_super.plan_rebuilt_super_chunks(layout, rebuilt_layout)
    ota_super.write_rebuilt_super_chunks(
        rebuilt_super,
        rebuilt_chunks,
        const.IMAGE_NEW_DIR,
    )
    ota_super.rewrite_super_xml_entries(
        sorted(const.IMAGE_NEW_DIR.glob("rawprogram*.xml")),
        rebuilt_chunks,
    )


def apply_incremental_ota() -> None:
    utils.ui.echo(get_string("ota_start"))

    # Find zip files in ota folder
    zip_files = _find_zip_files()
    if not zip_files:
        raise MissingFileError(
            get_string("ota_err_no_zips").format(dir=const.OTA_DIR.name)
        )

    # Select zip file
    zip_path = _select_zip_file(zip_files)

    rawprogram_paths, catalog = _load_xml_catalog()

    # Extract zip and find payload.bin
    with utils.temporary_workspace(const.OTA_WORKING_DIR):
        payload_bin = _extract_payload_bin(zip_path, const.OTA_WORKING_DIR)

        # Identify payload partitions and apply them all
        utils.ui.echo(get_string("ota_analyzing_payload"))
        partition_infos = _get_payload_partition_infos(payload_bin)
        partitions = update_engine_payload.partition_names_from_infos(partition_infos)
        partition_sizes = {info.name: info.new_size for info in partition_infos}
        utils.ui.echo(
            get_string("ota_found_partitions").format(
                count=len(partitions), names=", ".join(partitions)
            )
        )
        utils.ui.echo(
            get_string("ota_selected_partitions").format(
                count=len(partitions), names=", ".join(partitions)
            )
        )

        # Resolve actual filenames from rawprogram XMLs
        file_map = _build_partition_file_map(partitions, catalog)
        output_filenames, xml_filename_updates = _resolve_output_filenames(
            partitions, catalog
        )
        super_layout, extracted_dynamic_dir = _resolve_dynamic_partition_sources(
            partitions,
            file_map,
            _find_super_group(catalog),
            const.OTA_WORKING_DIR,
        )

        # Run differential patch
        _run_differential_patch(
            payload_bin,
            partitions,
            const.IMAGE_NEW_DIR,
            file_map,
            partition_sizes,
            output_filenames,
        )
        _copy_flash_xmls(const.IMAGE_NEW_DIR, rawprogram_paths, xml_filename_updates)
        ota_resign_targets = _resolve_ota_resign_targets(
            const.IMAGE_NEW_DIR,
            output_filenames,
        )
        if _confirm_ota_output_resign(ota_resign_targets):
            _resign_incremental_ota_outputs(ota_resign_targets)
        if super_layout is not None and extracted_dynamic_dir is not None:
            if not _confirm_dynamic_super_rebuild():
                utils.ui.echo(
                    get_string("ota_super_rebuild_skipped").format(
                        dir=const.IMAGE_NEW_DIR.name
                    )
                )
                return
            _rebuild_dynamic_super(super_layout, extracted_dynamic_dir)

    utils.ui.echo(get_string("ota_finished"))
