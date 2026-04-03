import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import List, Optional, Set

from .. import constants as const
from .. import downloader, ota_super, partition, utils
from ..errors import MissingFileError, ToolError, UserCancelError
from ..i18n import get_string
from ..process_runner import CommandRunner, RunOptions
from ..xml_catalog import PartitionGroup, XmlCatalog


def _payload_dumper_cmd(args: List[str]) -> List[str]:
    """Build command to run payload_dumper.py with its directory on sys.path.

    The bundled Python runs in isolated mode (safe_path=True, ignore_environment=True),
    so PYTHONPATH and automatic sys.path insertion are both disabled. We inject
    the payload_dumper directory into sys.path explicitly via -c.
    """
    script = const.PAYLOAD_DUMPER_PY
    bootstrap = (
        f"import sys; sys.path.insert(0, {str(const.PAYLOAD_DUMPER_DIR)!r}); "
        f"exec(open({str(script)!r}, encoding='utf-8').read())"
    )
    return [str(const.PYTHON_EXE), "-c", bootstrap, *args]


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


def _get_diff_partition_names(payload_bin: Path) -> List[str]:
    runner = CommandRunner()
    result = runner.run(
        _payload_dumper_cmd([str(payload_bin), "--diffpartname"]),
        options=RunOptions(capture=True, check=True),
    )

    partitions = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    if not partitions:
        raise ToolError(get_string("ota_err_no_diff_partitions"))

    return sorted(partitions)


def _prompt_partition_selection(labels: List[str]) -> List[str]:
    utils.ui.echo("")
    utils.ui.echo(get_string("ota_partition_choice_title"))
    utils.ui.echo(f"   1. {get_string('ota_partition_all')}")
    utils.ui.echo(f"   2. {get_string('ota_partition_select')}")
    utils.ui.echo(f"   c. {get_string('cancel')}")
    utils.ui.echo("")

    while True:
        choice = utils.ui.prompt(get_string("prompt_select")).strip().lower()
        if choice == "1":
            return labels[:]
        if choice == "2":
            break
        if choice == "c":
            raise UserCancelError(get_string("act_op_cancel"))
        utils.ui.error(get_string("err_invalid_selection"))

    # Individual toggle-based selection (adapted from edl.py)
    selected: Set[str] = set()

    while True:
        utils.ui.clear()
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(f"   {get_string('ota_select_partitions_title')}")
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
            result = [label for label in labels if label in selected]
            if not result:
                utils.ui.error(get_string("ota_err_none_selected"))
                input(get_string("press_enter_to_continue"))
                continue
            return result
        if choice == "c":
            raise UserCancelError(get_string("act_op_cancel"))

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


def _stage_old_images(
    partitions: List[str], file_map: dict[str, Path], staging_dir: Path
) -> None:
    """Create a staging directory with <partition>.img symlinks/copies.

    payload_dumper expects files named ``<partition>.img`` in the --old directory,
    but the actual firmware files may have other extensions (.elf, .mbn, .bin, etc.).
    """
    staging_dir.mkdir(parents=True, exist_ok=True)
    for name in partitions:
        src = file_map.get(name)
        if src is None:
            continue
        dest = staging_dir / f"{name}.img"
        if dest.exists():
            continue
        if src.suffix == ".img":
            try:
                dest.symlink_to(src.resolve())
            except OSError:
                shutil.copy2(src, dest)
        else:
            shutil.copy2(src, dest)


def _run_differential_patch(
    payload_bin: Path,
    partitions: List[str],
    output_dir: Path,
    old_dir: Path,
) -> None:
    utils.recreate_dir(output_dir)

    images_arg = ",".join(partitions)
    utils.ui.echo(get_string("ota_running_patch").format(images=images_arg))

    runner = CommandRunner()
    try:
        runner.run(
            _payload_dumper_cmd(
                [
                    str(payload_bin),
                    "--diff",
                    "--images",
                    images_arg,
                    "--out",
                    str(output_dir),
                    "--old",
                    str(old_dir),
                ]
            ),
            options=RunOptions(stream=True, check=True),
        )
    except subprocess.CalledProcessError as e:
        if output_dir.exists():
            shutil.rmtree(output_dir)
        raise ToolError(get_string("ota_err_patch_failed").format(e=e)) from e

    utils.ui.echo(get_string("ota_patch_complete").format(dir=output_dir.name))


def _resolve_lpmake_command() -> list[str]:
    bundled_lpmake = const.OTATOOLS_LPMAKE
    if not bundled_lpmake.exists():
        raise ToolError(
            "Required OTA tool missing: "
            f"{bundled_lpmake}. Re-download or re-extract the LTBox release package."
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
    command.append(_windows_to_wsl_path(bundled_lpmake))
    return command


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

    # Ensure payload_dumper is available
    downloader.ensure_payload_dumper()
    rawprogram_paths, catalog = _load_xml_catalog()

    # Extract zip and find payload.bin
    with utils.temporary_workspace(const.OTA_WORKING_DIR):
        payload_bin = _extract_payload_bin(zip_path, const.OTA_WORKING_DIR)

        # Identify differential partitions
        utils.ui.echo(get_string("ota_analyzing_payload"))
        partitions = _get_diff_partition_names(payload_bin)
        utils.ui.echo(
            get_string("ota_found_partitions").format(
                count=len(partitions), names=", ".join(partitions)
            )
        )

        # Partition selection
        selected = _prompt_partition_selection(partitions)
        utils.ui.echo(
            get_string("ota_selected_partitions").format(
                count=len(selected), names=", ".join(selected)
            )
        )

        # Resolve actual filenames from rawprogram XMLs
        file_map = _build_partition_file_map(selected, catalog)
        super_layout, extracted_dynamic_dir = _resolve_dynamic_partition_sources(
            selected,
            file_map,
            _find_super_group(catalog),
            const.OTA_WORKING_DIR,
        )

        # Stage old images as .img files for payload_dumper
        old_staging_dir = const.OTA_DIR / "image_old"
        _stage_old_images(selected, file_map, old_staging_dir)

        # Run differential patch
        try:
            _run_differential_patch(
                payload_bin, selected, const.IMAGE_NEW_DIR, old_staging_dir
            )
            _copy_flash_xmls(const.IMAGE_NEW_DIR, rawprogram_paths)
            if super_layout is not None and extracted_dynamic_dir is not None:
                _rebuild_dynamic_super(super_layout, extracted_dynamic_dir)
        finally:
            if old_staging_dir.exists():
                shutil.rmtree(old_staging_dir)

    utils.ui.echo(get_string("ota_finished"))
