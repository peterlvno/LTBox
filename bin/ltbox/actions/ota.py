import hashlib
import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import Any, Callable, List, Optional

from .. import constants as const
from .. import ota_super, partition, update_engine_payload, utils
from ..errors import MissingFileError, ToolError
from ..i18n import get_string
from ..patch.avb import (
    extract_image_avb_info,
    rebuild_vbmeta_preserving_descriptors,
    resign_avb_image,
)
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
_OTA_VBMETA_SYSTEM_DESCRIPTOR_PARTITIONS = (
    "pvmfw",
    "product",
    "system",
    "system_ext",
)
_OTA_VBMETA_DESCRIPTOR_PARTITIONS = (
    "boot",
    "dtbo",
    "init_boot",
    "odm",
    "recovery",
    "system_dlkm",
    "vendor",
    "vendor_boot",
    "vendor_dlkm",
)


def _wait_for_prompted_condition(
    predicate: Callable[[], Any],
    prompt_message: str,
    *directories: Path,
) -> Any:
    for directory in directories:
        directory.mkdir(parents=True, exist_ok=True)

    def _prompt_loop() -> None:
        utils.ui.clear()
        utils.ui.echo(get_string("utils_wait_resource"))
        utils.ui.echo(prompt_message)
        utils.ui.echo(get_string("press_enter_to_continue"))
        try:
            utils.ui.prompt()
        except EOFError as exc:
            raise RuntimeError(get_string("act_op_cancel")) from exc

    return utils.wait_for_condition(
        predicate,
        interval=0.1,
        on_loop=_prompt_loop,
    )


def _find_zip_files() -> List[Path]:
    const.OTA_DIR.mkdir(parents=True, exist_ok=True)
    return sorted(const.OTA_DIR.glob("*.zip"))


def _run_cleared_prompt(prompt_func: Callable[[], object]) -> object:
    utils.ui.clear()
    try:
        return prompt_func()
    finally:
        utils.ui.clear()


def _select_zip_file(zip_files: List[Path]) -> Path:
    if len(zip_files) == 1:
        utils.ui.echo(get_string("ota_zip_found").format(name=zip_files[0].name))
        return zip_files[0]

    while True:

        def _prompt_once() -> str:
            utils.ui.echo(get_string("ota_multiple_zips"))
            for i, zf in enumerate(zip_files, 1):
                utils.ui.echo(f"   {i}. {zf.name}")
            utils.ui.echo("")
            return utils.ui.prompt(get_string("prompt_select")).strip()

        choice = str(_run_cleared_prompt(_prompt_once))
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
        # Only extract payload.bin to avoid extracting the entire multi-GB OTA
        # package and to prevent zip-slip (path traversal) attacks.
        payload_members = [
            m for m in zf.namelist() if m == "payload.bin" or m.endswith("/payload.bin")
        ]
        if not payload_members:
            raise MissingFileError(get_string("ota_err_no_payload_bin"))
        zf.extract(payload_members[0], working_dir)

    payload_bin = working_dir / payload_members[0]
    if not payload_bin.exists():
        raise MissingFileError(get_string("ota_err_no_payload_bin"))

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


def _try_load_source_super_layout() -> Optional[
    tuple[list[Path], XmlCatalog, ota_super.SuperLayout]
]:
    try:
        rawprogram_paths, catalog = _load_xml_catalog()
    except MissingFileError:
        return None

    super_group = _find_super_group(catalog)
    if super_group is None:
        return None

    try:
        layout = ota_super.parse_super_layout(super_group.none, const.IMAGE_DIR)
    except MissingFileError:
        return None

    return rawprogram_paths, catalog, layout


def _wait_for_source_super_layout() -> tuple[
    list[Path], XmlCatalog, ota_super.SuperLayout
]:
    prompt = get_string("ota_wait_super_source").format(dir=const.IMAGE_DIR.name)
    result = _wait_for_prompted_condition(
        _try_load_source_super_layout,
        prompt,
        const.IMAGE_DIR,
    )
    if result is None:
        raise RuntimeError(get_string("act_op_cancel"))
    return result


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
    new_hashes: dict[str, bytes],
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

    utils.ui.echo(get_string("ota_verifying_hashes"))
    for name, image_path in zip(partitions, new_images):
        expected_hash = new_hashes.get(name)
        if not expected_hash:
            continue

        sha256_hash = hashlib.sha256()
        try:
            with open(image_path, "rb") as f:
                for byte_block in iter(lambda: f.read(1024 * 1024), b""):
                    sha256_hash.update(byte_block)
            actual_hash = sha256_hash.digest()
            if actual_hash != expected_hash:
                raise ToolError(
                    f"Hash mismatch for {name}: "
                    f"expected {expected_hash.hex()}, got {actual_hash.hex()}"
                )
        except Exception as e:
            if isinstance(e, ToolError):
                raise e
            raise ToolError(f"Failed to verify hash for {name}: {e}") from e

    utils.ui.echo(get_string("ota_patch_complete").format(dir=output_dir.name))


def _wsl_is_available() -> bool:
    wsl_exe = shutil.which("wsl.exe")
    if not wsl_exe:
        return False

    try:
        result = subprocess.run(
            [wsl_exe, "--exec", "/usr/bin/env", "true"],
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except (OSError, subprocess.SubprocessError):
        return False

    return result.returncode == 0


def _ensure_wsl_available() -> None:
    if _wsl_is_available():
        return
    raise ToolError(get_string("ota_err_wsl_required"))


def _resolve_otatools_linux_command(tool_path: Path) -> list[str]:
    if not tool_path.exists():
        raise ToolError(
            "Required OTA tool missing: "
            f"{tool_path}. Re-download or re-extract the LTBox release package."
        )

    wsl_exe = shutil.which("wsl.exe")
    if not wsl_exe:
        raise ToolError(get_string("ota_err_wsl_required"))

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
    patch_paths = _collect_patch_xml_paths(rawprogram_paths)
    copied_xmls = ota_super.copy_flash_xmls(rawprogram_paths, patch_paths, output_dir)
    ota_super.rewrite_xml_filenames(copied_xmls, xml_filename_updates)
    ota_super.create_keep_data_ota_xml(output_dir)


def _collect_patch_xml_paths(rawprogram_paths: List[Path]) -> list[Path]:
    patch_paths: list[Path] = []
    seen_patch_names: set[str] = set()
    candidate_dirs = {const.IMAGE_DIR, *[path.parent for path in rawprogram_paths]}
    for candidate_dir in candidate_dirs:
        for patch_path in sorted(candidate_dir.glob("patch*.xml")):
            if patch_path.name in seen_patch_names:
                continue
            seen_patch_names.add(patch_path.name)
            patch_paths.append(patch_path)
    return patch_paths


def _promote_incremental_ota_outputs(
    source_dir: Path,
    target_dir: Path,
    preserve_abl: bool = False,
) -> None:
    if not source_dir.exists():
        raise MissingFileError(
            get_string("ota_err_missing_images").format(
                files=source_dir.name,
                dir=source_dir.parent.name,
            )
        )

    target_dir.mkdir(parents=True, exist_ok=True)
    skipped_names = {"abl.elf"} if preserve_abl else set()

    for source_path in sorted(source_dir.iterdir()):
        if source_path.name in skipped_names:
            continue

        target_path = target_dir / source_path.name
        if target_path.exists():
            if target_path.is_dir():
                shutil.rmtree(target_path)
            else:
                target_path.unlink()

        shutil.move(str(source_path), str(target_path))

    shutil.rmtree(source_dir)
    utils.ui.echo(get_string("ota_outputs_promoted").format(dir=target_dir.name))


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

    def _prompt_once() -> bool:
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

    return bool(_run_cleared_prompt(_prompt_once))


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
    return (
        _resolve_ota_testkey_path(_OTA_AVB_DEFAULT_RESIGN_KEY_NAME),
        _resolve_testkey_resign_algorithm(
            original_algorithm, _OTA_AVB_DEFAULT_RSA_BITS
        ),
    )


def _resolve_ota_vbmeta_inputs(
    partition_names: tuple[str, ...], candidate_paths: dict[str, Path]
) -> list[Path]:
    return [
        candidate_paths[name] for name in partition_names if name in candidate_paths
    ]


def _prepare_ota_vbmeta_rebuild_base(
    output_path: Path,
) -> tuple[Path, Optional[Path]]:
    if output_path.exists():
        temp_dir = const.OTA_WORKING_DIR / "vbmeta_rebuild"
        temp_dir.mkdir(parents=True, exist_ok=True)
        temp_copy = temp_dir / output_path.name
        shutil.copy2(output_path, temp_copy)
        return temp_copy, temp_copy

    source_path = const.IMAGE_DIR / output_path.name
    if not source_path.exists():
        raise MissingFileError(
            get_string("ota_err_missing_images").format(
                files=output_path.name,
                dir=const.IMAGE_DIR.name,
            )
        )
    return source_path, None


def _rebuild_ota_vbmeta_image(
    partition_name: str,
    chained_images: list[Path],
) -> Optional[Path]:
    if not chained_images:
        return None

    output_path = const.IMAGE_NEW_DIR / f"{partition_name}.img"
    base_path, temp_copy = _prepare_ota_vbmeta_rebuild_base(output_path)
    base_vbmeta_info = extract_image_avb_info(base_path)
    key_path, rebuild_algorithm = _resolve_ota_resign_policy(
        partition_name,
        str(base_vbmeta_info.get("algorithm", "NONE")).upper(),
    )
    try:
        rebuild_vbmeta_preserving_descriptors(
            output_path=output_path,
            original_vbmeta_path=base_path,
            chained_images=chained_images,
            key_file=key_path,
            algorithm=rebuild_algorithm,
        )
    finally:
        if temp_copy is not None and temp_copy.exists():
            temp_copy.unlink()
    return output_path


def _rebuild_incremental_ota_vbmeta(candidate_paths: dict[str, Path]) -> None:
    rebuilt_vbmeta_system = _rebuild_ota_vbmeta_image(
        "vbmeta_system",
        _resolve_ota_vbmeta_inputs(
            _OTA_VBMETA_SYSTEM_DESCRIPTOR_PARTITIONS,
            candidate_paths,
        ),
    )

    vbmeta_inputs = _resolve_ota_vbmeta_inputs(
        _OTA_VBMETA_DESCRIPTOR_PARTITIONS,
        candidate_paths,
    )
    if rebuilt_vbmeta_system is not None:
        vbmeta_inputs.append(rebuilt_vbmeta_system)
    elif "vbmeta_system" in candidate_paths:
        vbmeta_inputs.append(candidate_paths["vbmeta_system"])

    _rebuild_ota_vbmeta_image("vbmeta", vbmeta_inputs)


def _ensure_ota_resign_key_available() -> Path:
    prompt = get_string("ota_wait_resign_key").format(
        key=_OTA_AVB_DEFAULT_RESIGN_KEY_NAME,
        dir=const.TOOLS_DIR.name,
    )
    utils.wait_for_files(
        const.TOOLS_DIR,
        [_OTA_AVB_DEFAULT_RESIGN_KEY_NAME],
        prompt,
    )
    return _resolve_ota_testkey_path(_OTA_AVB_DEFAULT_RESIGN_KEY_NAME)


def _collect_standalone_resign_targets() -> Optional[dict[str, Path]]:
    image_targets = _resolve_ota_resign_targets(const.IMAGE_DIR, {})
    output_targets = _resolve_ota_resign_targets(const.IMAGE_NEW_DIR, {})
    combined_targets = {**image_targets, **output_targets}
    if not combined_targets:
        return None

    vbmeta_base_path = const.IMAGE_NEW_DIR / "vbmeta.img"
    if not vbmeta_base_path.exists():
        vbmeta_base_path = const.IMAGE_DIR / "vbmeta.img"
    if not vbmeta_base_path.exists():
        return None

    needs_vbmeta_system = "vbmeta_system" in combined_targets or any(
        partition_name in combined_targets
        for partition_name in _OTA_VBMETA_SYSTEM_DESCRIPTOR_PARTITIONS
    )
    if needs_vbmeta_system:
        vbmeta_system_base_path = const.IMAGE_NEW_DIR / "vbmeta_system.img"
        if not vbmeta_system_base_path.exists():
            vbmeta_system_base_path = const.IMAGE_DIR / "vbmeta_system.img"
        if not vbmeta_system_base_path.exists():
            return None

    return combined_targets


def _wait_for_standalone_resign_targets() -> dict[str, Path]:
    prompt = get_string("ota_wait_resign_source").format(
        dir=const.IMAGE_DIR.name,
        out_dir=const.IMAGE_NEW_DIR.name,
    )
    result = _wait_for_prompted_condition(
        _collect_standalone_resign_targets,
        prompt,
        const.IMAGE_DIR,
        const.IMAGE_NEW_DIR,
    )
    if result is None:
        raise RuntimeError(get_string("act_op_cancel"))
    return result


def _prepare_standalone_resign_targets(
    source_targets: dict[str, Path],
) -> dict[str, Path]:
    const.IMAGE_NEW_DIR.mkdir(parents=True, exist_ok=True)
    prepared_targets: dict[str, Path] = {}

    for partition_name, source_path in source_targets.items():
        if source_path.parent == const.IMAGE_NEW_DIR:
            prepared_targets[partition_name] = source_path
            continue

        destination_path = const.IMAGE_NEW_DIR / source_path.name
        if not destination_path.exists():
            shutil.copy2(source_path, destination_path)
        prepared_targets[partition_name] = destination_path

    return prepared_targets


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

    _rebuild_incremental_ota_vbmeta(candidate_paths)

    utils.ui.echo(
        get_string("ota_resign_summary").format(
            resigned=resigned_count,
            skipped_none=skipped_none_count,
            skipped_scan=skipped_unreadable_count,
        )
    )


def _confirm_dynamic_super_rebuild() -> bool:
    def _prompt_once() -> bool:
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

    return bool(_run_cleared_prompt(_prompt_once))


def _rebuild_dynamic_super(
    layout: ota_super.SuperLayout,
    extracted_dynamic_dir: Path,
) -> None:
    dynamic_build_dir = const.OTA_WORKING_DIR / "dynamic_build"
    if dynamic_build_dir.exists():
        shutil.rmtree(dynamic_build_dir)
    shutil.copytree(extracted_dynamic_dir, dynamic_build_dir)

    rebuilt_dynamic_outputs: list[Path] = []
    for partition_name in sorted(layout.dynamic_partition_names):
        patched_image = const.IMAGE_NEW_DIR / f"{partition_name}.img"
        if patched_image.exists():
            shutil.copy2(patched_image, dynamic_build_dir / patched_image.name)
            rebuilt_dynamic_outputs.append(patched_image)

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
    for patched_image in rebuilt_dynamic_outputs:
        patched_image.unlink(missing_ok=True)


def unpack_super_images() -> None:
    utils.ui.echo(get_string("ota_super_unpack_start"))
    _rawprogram_paths, _catalog, layout = _wait_for_source_super_layout()

    const.IMAGE_NEW_DIR.mkdir(parents=True, exist_ok=True)
    for partition_name in sorted(layout.dynamic_partition_names):
        (const.IMAGE_NEW_DIR / f"{partition_name}.img").unlink(missing_ok=True)

    extracted = ota_super.extract_partition_images(layout, const.IMAGE_NEW_DIR)
    utils.ui.echo(
        get_string("ota_super_unpack_complete").format(
            dir=const.IMAGE_NEW_DIR.name,
            count=len(extracted),
            names=", ".join(sorted(extracted)),
        )
    )


def _copy_flash_xmls_for_super_repack(
    output_dir: Path,
    rawprogram_paths: List[Path],
) -> None:
    patch_paths = _collect_patch_xml_paths(rawprogram_paths)
    ota_super.copy_flash_xmls(rawprogram_paths, patch_paths, output_dir)


def repack_super_images() -> None:
    _ensure_wsl_available()
    utils.ui.echo(get_string("ota_super_repack_start"))
    rawprogram_paths, _catalog, layout = _wait_for_source_super_layout()

    const.IMAGE_NEW_DIR.mkdir(parents=True, exist_ok=True)
    _copy_flash_xmls_for_super_repack(const.IMAGE_NEW_DIR, rawprogram_paths)

    with utils.temporary_workspace(const.OTA_WORKING_DIR):
        extracted_dynamic_dir = const.OTA_WORKING_DIR / "dynamic_old"
        ota_super.extract_partition_images(layout, extracted_dynamic_dir)
        _rebuild_dynamic_super(layout, extracted_dynamic_dir)

    utils.ui.echo(
        get_string("ota_super_repack_complete").format(dir=const.IMAGE_NEW_DIR.name)
    )


def resign_firmware_with_testkeys() -> None:
    utils.ui.echo(get_string("ota_resign_standalone_start"))
    _ensure_ota_resign_key_available()
    source_targets = _wait_for_standalone_resign_targets()
    prepared_targets = _prepare_standalone_resign_targets(source_targets)
    _resign_incremental_ota_outputs(prepared_targets)
    utils.ui.echo(
        get_string("ota_resign_standalone_complete").format(
            dir=const.IMAGE_NEW_DIR.name
        )
    )


def apply_incremental_ota() -> None:
    _ensure_wsl_available()
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

        partition_hashes = update_engine_payload.get_partition_hashes(payload_bin)

        # Run differential patch
        _run_differential_patch(
            payload_bin,
            partitions,
            const.IMAGE_NEW_DIR,
            file_map,
            partition_sizes,
            partition_hashes,
            output_filenames,
        )
        _copy_flash_xmls(const.IMAGE_NEW_DIR, rawprogram_paths, xml_filename_updates)
        ota_resign_targets = _resolve_ota_resign_targets(
            const.IMAGE_NEW_DIR,
            output_filenames,
        )
        did_resign_outputs = False
        if _confirm_ota_output_resign(ota_resign_targets):
            _resign_incremental_ota_outputs(ota_resign_targets)
            did_resign_outputs = True
        if super_layout is not None and extracted_dynamic_dir is not None:
            if not _confirm_dynamic_super_rebuild():
                utils.ui.echo(
                    get_string("ota_super_rebuild_skipped").format(
                        dir=const.IMAGE_NEW_DIR.name
                    )
                )
                return
            _rebuild_dynamic_super(super_layout, extracted_dynamic_dir)
        _promote_incremental_ota_outputs(
            const.IMAGE_NEW_DIR,
            const.IMAGE_DIR,
            preserve_abl=did_resign_outputs,
        )

    utils.ui.echo(get_string("ota_finished"))
