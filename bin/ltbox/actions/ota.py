import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import List, Optional

from .. import constants as const
from .. import ota_super, partition, update_engine_payload, utils
from ..errors import MissingFileError, ToolError
from ..i18n import get_string
from ..process_runner import CommandRunner, RunOptions
from ..prompt_helpers import prompt_yes_no
from ..xml_catalog import PartitionGroup, XmlCatalog


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
) -> None:
    utils.recreate_dir(output_dir)

    images_arg = ",".join(partitions)
    utils.ui.echo(get_string("ota_running_patch").format(images=images_arg))

    new_images = []
    for name in partitions:
        image_path = output_dir / f"{name}.img"
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


def _copy_flash_xmls(output_dir: Path, rawprogram_paths: List[Path]) -> None:
    patch_paths: list[Path] = []
    seen_patch_names: set[str] = set()
    candidate_dirs = {const.IMAGE_DIR, *[path.parent for path in rawprogram_paths]}
    for candidate_dir in candidate_dirs:
        for patch_path in sorted(candidate_dir.glob("patch*.xml")):
            if patch_path.name in seen_patch_names:
                continue
            seen_patch_names.add(patch_path.name)
            patch_paths.append(patch_path)

    ota_super.copy_flash_xmls(rawprogram_paths, patch_paths, output_dir)
    ota_super.create_keep_data_ota_xml(output_dir)


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
    ota_super.split_rebuilt_super(layout, rebuilt_super, const.IMAGE_NEW_DIR)


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
        super_layout, extracted_dynamic_dir = _resolve_dynamic_partition_sources(
            partitions,
            file_map,
            _find_super_group(catalog),
            const.OTA_WORKING_DIR,
        )

        # Run differential patch
        _run_differential_patch(
            payload_bin, partitions, const.IMAGE_NEW_DIR, file_map, partition_sizes
        )
        _copy_flash_xmls(const.IMAGE_NEW_DIR, rawprogram_paths)
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
