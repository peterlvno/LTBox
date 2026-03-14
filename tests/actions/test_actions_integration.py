import re
import shutil
import subprocess
from contextlib import nullcontext
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

import pytest
from ltbox import constants as const
from ltbox.actions import xml as xml_action
from ltbox.actions.root import FolkPatchStrategy, GkiRootStrategy, LkmRootStrategy

pytestmark = pytest.mark.integration

MAGISKBOOT_PATH = (
    Path(__file__).resolve().parents[2] / "bin" / "tools" / "magiskboot.exe"
)


@pytest.fixture
def firmware_file_getter(fw_pkg):
    def _get(*names: str) -> tuple[Path, ...]:
        if not fw_pkg:
            pytest.skip("Firmware package not available")

        files: list[Path] = []
        missing: list[str] = []

        for name in names:
            path = fw_pkg.get(name)
            if path is None:
                missing.append(name)
            else:
                files.append(path)

        if missing:
            joined = ", ".join(missing)
            pytest.skip(f"Required files missing in firmware package: {joined}")

        return tuple(files)

    return _get


def _prepare_mock_dirs(tmp_path: Path, root_output_key: str) -> dict[str, Path]:
    mock_dirs = {
        "TOOLS_DIR": tmp_path / "bin" / "tools",
        "DOWNLOAD_DIR": tmp_path / "bin" / "download",
        root_output_key: tmp_path / "output" / root_output_key.lower(),
        "IMAGE_DIR": tmp_path / "images",
        "BASE_DIR": tmp_path / "base",
    }
    for directory in mock_dirs.values():
        directory.mkdir(parents=True, exist_ok=True)
    return mock_dirs


def _copy_bundled_magiskboot(tools_dir: Path) -> Path:
    if not MAGISKBOOT_PATH.exists():
        pytest.skip("magiskboot.exe not found in bin/tools. Please build it first.")

    magiskboot_exe = tools_dir / "magiskboot.exe"
    shutil.copy(MAGISKBOOT_PATH, magiskboot_exe)
    for dll_file in MAGISKBOOT_PATH.parent.glob("*.dll"):
        shutil.copy(dll_file, tools_dir / dll_file.name)
    return magiskboot_exe


def _run_patch_with_fail_context(
    strategy: Any,
    work_dir: Path,
    fail_label: str,
    patch_kwargs: dict[str, Any] | None = None,
) -> Path:
    try:
        return strategy.patch(work_dir, dev=None, **(patch_kwargs or {}))
    except Exception as e:
        pytest.fail(f"{fail_label} patching failed with real tools: {e}")


def _setup_gki_context(ctx: dict[str, Path | None]) -> None:
    shutil.copy(ctx["boot_img"], ctx["work_dir"] / "boot.img")
    shutil.copy(ctx["boot_img"], ctx["base_dir"] / "boot.bak.img")


def _setup_folkpatch_context(ctx: dict[str, Path | None]) -> None:
    shutil.copy(ctx["boot_img"], ctx["work_dir"] / "boot.img")
    if ctx["vbmeta_img"] is None:
        pytest.fail("FolkPatch requires vbmeta image in setup context")
    shutil.copy(ctx["vbmeta_img"], ctx["base_dir"] / "vbmeta.bak.img")
    shutil.copy(ctx["boot_img"], ctx["base_dir"] / "boot.bak.img")


def test_xml_wipe(firmware_file_getter):
    (path,) = firmware_file_getter("rawprogram_unsparse0.xml")

    tmp_xml = path.parent / "test_wipe.xml"
    shutil.copy(path, tmp_xml)

    with patch("ltbox.actions.xml.utils.ui"):
        xml_action._patch_xml_for_wipe(tmp_xml, wipe=0)

    root = ET.parse(tmp_xml).getroot()
    progs = [p for p in root.findall("program") if p.get("label") == "userdata"]
    assert len(progs) > 0
    for p in progs:
        assert p.get("filename") == ""


def test_xml_persist_check(firmware_file_getter):
    (path,) = firmware_file_getter("rawprogram_save_persist_unsparse0.xml")

    root = ET.parse(path).getroot()
    persist_program = next(
        (item for item in root.findall("program") if item.get("label") == "persist"),
        None,
    )
    if persist_program is not None:
        assert persist_program.get("filename", "") == ""


def test_prc_to_row(firmware_file_getter, mock_env):
    real_vb, real_vbmeta = firmware_file_getter("vendor_boot.img", "vbmeta.img")

    img_dir = mock_env["IMAGE_DIR"]
    output_dir = mock_env["OUTPUT_DIR"]

    shutil.copy(real_vb, img_dir / "vendor_boot.img")
    shutil.copy(real_vbmeta, img_dir / "vbmeta.img")

    mock_dev = MagicMock()
    mock_dev.skip_adb = True

    from ltbox.actions import region
    from ltbox.patch.avb import extract_image_avb_info

    vendor_boot_info = extract_image_avb_info(img_dir / "vendor_boot.img")
    assert vendor_boot_info, "Failed to extract AVB info from vendor_boot.img"

    region.convert_region_images(dev=mock_dev, target_region="ROW", on_log=print)

    out_vb = output_dir / "vendor_boot.img"
    out_vbmeta = output_dir / "vbmeta.img"

    assert out_vb.exists(), "Output vendor_boot.img was not created"
    assert out_vbmeta.exists(), "Output vbmeta.img was not created"
    assert out_vb.stat().st_size > 0
    assert (const.BACKUP_DIR / "vendor_boot.bak.img").exists()

    print(
        f"\n[PASS] Successfully converted Real Firmware to ROW. Output size: {out_vb.stat().st_size} bytes"
    )


@pytest.mark.parametrize(
    "label,strategy_cls,output_key,download_message,download_error,setup_fn,final_name,patch_kwargs,input_value",
    [
        (
            "GKI",
            GkiRootStrategy,
            "OUTPUT_ROOT_DIR",
            "[INFO] [GKI] Downloading resources (Manager APK)...",
            "Failed to download GKI resources",
            _setup_gki_context,
            "boot.img",
            None,
            None,
        ),
        (
            "FOLKPATCH",
            FolkPatchStrategy,
            "OUTPUT_ROOT_DIR",
            "[INFO] [FOLKPATCH] Downloading resources (kptools, APK)...",
            "Failed to download FolkPatch resources",
            _setup_folkpatch_context,
            "boot.img",
            None,
            "SuperKey1234",
        ),
    ],
)
def test_root_patch_strategies(
    firmware_file_getter,
    tmp_path,
    label,
    strategy_cls,
    output_key,
    download_message,
    download_error,
    setup_fn,
    final_name,
    patch_kwargs,
    input_value,
):
    required = ["boot.img"]
    if label == "FOLKPATCH":
        required.append("vbmeta.img")

    loaded = firmware_file_getter(*required)
    boot_img = loaded[0]
    vbmeta_img = loaded[1] if len(loaded) > 1 else None

    mock_dirs = _prepare_mock_dirs(tmp_path, root_output_key=output_key)

    with patch.multiple("ltbox.constants", **mock_dirs):
        print(f"\n[INFO] [{label}] Checking bundled magiskboot...")
        _copy_bundled_magiskboot(mock_dirs["TOOLS_DIR"])

        strategy = strategy_cls()

        print(download_message)
        if not strategy.download_resources():
            pytest.fail(download_error)

        work_dir = tmp_path / f"work_{label.lower()}"
        work_dir.mkdir()

        setup_context = {
            "boot_img": boot_img,
            "vbmeta_img": vbmeta_img,
            "work_dir": work_dir,
            "base_dir": mock_dirs["BASE_DIR"],
        }
        setup_fn(setup_context)

        print(f"[INFO] [{label}] Running ACTUAL patch process...")

        input_context = (
            patch("builtins.input", return_value=input_value)
            if input_value is not None
            else nullcontext()
        )
        with input_context:
            patched_path = _run_patch_with_fail_context(
                strategy,
                work_dir,
                label,
                patch_kwargs=patch_kwargs,
            )

        if patched_path is None:
            pytest.skip(
                "FolkPatch returned None, likely due to unsupported kernel (missing CONFIG_KALLSYMS). Skipping test."
            )

        assert patched_path.exists(), "Patched boot image not returned"
        assert patched_path.stat().st_size > 0, "Patched boot image is empty"
        print(f"[INFO] [{label}] Patch success: {patched_path}")

        print(f"[INFO] [{label}] Finalizing...")
        final_output = strategy.finalize_patch(
            patched_path,
            mock_dirs[output_key],
            mock_dirs["BASE_DIR"],
        )

        assert final_output.exists()
        assert final_output.name == final_name
        print(f"[PASS] {label} Integration Test Complete. Output: {final_output}")


def extract_kernel_version_from_img(boot_img_path, magiskboot_exe, work_dir):
    unpack_dir = work_dir / "unpack_for_ver"
    if unpack_dir.exists():
        shutil.rmtree(unpack_dir)
    unpack_dir.mkdir()

    shutil.copy(boot_img_path, unpack_dir / "boot.img")

    try:
        subprocess.run(
            [str(magiskboot_exe), "unpack", "boot.img"],
            cwd=str(unpack_dir),
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"Failed to unpack boot image for version check: {e}")

    kernel_file = unpack_dir / "kernel"
    if not kernel_file.exists():
        raise FileNotFoundError("Kernel file not found after unpacking boot.img")

    content = kernel_file.read_bytes()
    match = re.search(rb"Linux version ([0-9]+\.[0-9]+\.[0-9]+)", content)
    if match:
        return match.group(1).decode("utf-8")

    raise ValueError("Could not find Linux version string in kernel binary")


def test_root_lkm(firmware_file_getter, tmp_path):
    boot_img, vbmeta_img, init_boot_img = firmware_file_getter(
        "boot.img", "vbmeta.img", "init_boot.img"
    )

    mock_dirs = _prepare_mock_dirs(tmp_path, root_output_key="OUTPUT_ROOT_LKM_DIR")

    with patch.multiple("ltbox.constants", **mock_dirs):
        print("\n[INFO] [LKM] Checking bundled magiskboot...")
        magiskboot_exe = _copy_bundled_magiskboot(mock_dirs["TOOLS_DIR"])

        print(f"[INFO] [LKM] Extracting kernel version from {boot_img.name}...")
        try:
            detected_version_full = extract_kernel_version_from_img(
                boot_img, magiskboot_exe, tmp_path
            )
            print(
                f"[INFO] [LKM] Detected Kernel Version (Full): {detected_version_full}"
            )
            detected_version_short = ".".join(detected_version_full.split(".")[:2])
            print(
                f"[INFO] [LKM] Using Kernel Version (Short): {detected_version_short}"
            )
        except Exception as e:
            pytest.fail(f"Failed to extract kernel version: {e}")

        strategy = LkmRootStrategy()
        strategy.staging_dir.mkdir(parents=True, exist_ok=True)

        print(
            f"[INFO] [LKM] Downloading resources for kernel {detected_version_short}..."
        )
        if not strategy.download_resources(detected_version_short):
            pytest.fail(
                f"Failed to download LKM resources for {detected_version_short}"
            )

        assert (strategy.staging_dir / "init").exists()
        assert (strategy.staging_dir / "kernelsu.ko").exists()

        work_dir = tmp_path / "work_lkm"
        work_dir.mkdir()

        shutil.copy(vbmeta_img, mock_dirs["BASE_DIR"] / const.FN_VBMETA_BAK)
        shutil.copy(boot_img, mock_dirs["BASE_DIR"] / "init_boot.bak.img")

        print(f"[INFO] [LKM] Using REAL {init_boot_img.name} from firmware...")
        shutil.copy(init_boot_img, work_dir / "init_boot.img")
        shutil.copy(init_boot_img, mock_dirs["BASE_DIR"] / "init_boot.bak.img")

        print("[INFO] [LKM] Running ACTUAL patch process...")
        patched_path = _run_patch_with_fail_context(
            strategy,
            work_dir,
            "LKM",
            patch_kwargs={"lkm_kernel_version": detected_version_short},
        )

        assert patched_path.exists()
        print(f"[INFO] [LKM] Patch success: {patched_path}")

        print("[INFO] [LKM] Finalizing (AVB Chaining)...")
        final_output = strategy.finalize_patch(
            patched_path,
            mock_dirs["OUTPUT_ROOT_LKM_DIR"],
            mock_dirs["BASE_DIR"],
        )

        assert final_output.exists()
        assert final_output.name == "init_boot.img"
        assert (mock_dirs["OUTPUT_ROOT_LKM_DIR"] / "vbmeta.img").exists()
        print(f"[PASS] LKM Integration Test Complete. Output: {final_output}")
