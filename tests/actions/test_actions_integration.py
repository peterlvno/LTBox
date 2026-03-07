import re
import shutil
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from ltbox import constants as const
from ltbox.actions import xml as xml_action
from ltbox.actions.root import (
    FolkPatchStrategy,
    GkiRootStrategy,
    LkmRootStrategy,
    MagiskRootStrategy,
)

pytestmark = pytest.mark.integration


def test_xml_wipe(fw_pkg):
    path = fw_pkg.get("rawprogram_unsparse0.xml")
    if not path:
        pytest.skip("XML not found")

    tmp_xml = path.parent / "test_wipe.xml"
    shutil.copy(path, tmp_xml)

    with patch("ltbox.actions.xml.utils.ui"):
        xml_action._patch_xml_for_wipe(tmp_xml, wipe=0)

    root = ET.parse(tmp_xml).getroot()
    progs = [p for p in root.findall("program") if p.get("label") == "userdata"]
    assert len(progs) > 0
    for p in progs:
        assert p.get("filename") == ""


def test_xml_persist_check(fw_pkg):
    path = fw_pkg.get("rawprogram_save_persist_unsparse0.xml")
    if not path:
        pytest.skip("Persist XML not found")

    root = ET.parse(path).getroot()
    p = next((x for x in root.findall("program") if x.get("label") == "persist"), None)
    if p is not None:
        assert p.get("filename", "") == ""


def test_prc_to_row(fw_pkg, mock_env):
    if not fw_pkg:
        pytest.skip("Firmware package not available (Download skipped or failed)")

    img_dir = mock_env["IMAGE_DIR"]
    output_dir = mock_env["OUTPUT_DIR"]

    real_vb = fw_pkg.get("vendor_boot.img")
    real_vbmeta = fw_pkg.get("vbmeta.img")

    if not real_vb or not real_vbmeta:
        pytest.skip(
            "Required images (vendor_boot.img, vbmeta.img) not found in firmware package"
        )

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


def test_root_gki(fw_pkg, tmp_path):
    if not fw_pkg:
        pytest.skip("Firmware package not available")

    boot_img = fw_pkg.get("boot.img")
    if not boot_img:
        pytest.skip("boot.img not found in firmware package")

    mock_dirs = {
        "TOOLS_DIR": tmp_path / "bin" / "tools",
        "DOWNLOAD_DIR": tmp_path / "bin" / "download",
        "OUTPUT_ROOT_DIR": tmp_path / "output" / "root",
        "IMAGE_DIR": tmp_path / "images",
        "BASE_DIR": tmp_path / "base",
    }
    for d in mock_dirs.values():
        d.mkdir(parents=True, exist_ok=True)

    with patch.multiple("ltbox.constants", **mock_dirs):
        print("\n[INFO] [GKI] Checking bundled magiskboot...")
        magiskboot_exe = mock_dirs["TOOLS_DIR"] / "magiskboot.exe"
        real_magiskboot = (
            Path(__file__).resolve().parents[2] / "bin" / "tools" / "magiskboot.exe"
        )

        if real_magiskboot.exists():
            shutil.copy(real_magiskboot, magiskboot_exe)

            for dll_file in real_magiskboot.parent.glob("*.dll"):
                shutil.copy(dll_file, mock_dirs["TOOLS_DIR"] / dll_file.name)
        else:
            pytest.skip("magiskboot.exe not found in bin/tools. Please build it first.")

        strategy = GkiRootStrategy()

        print("[INFO] [GKI] Downloading resources (Manager APK)...")
        if not strategy.download_resources():
            pytest.fail("Failed to download GKI resources")

        work_dir = tmp_path / "work_gki"
        work_dir.mkdir()

        target_boot = work_dir / "boot.img"
        shutil.copy(boot_img, target_boot)

        print("[INFO] [GKI] Running ACTUAL patch process...")
        try:
            patched_path = strategy.patch(work_dir, dev=None)
        except Exception as e:
            pytest.fail(f"GKI Patching failed with real tools: {e}")

        assert patched_path.exists(), "Patched boot image not returned"
        assert patched_path.stat().st_size > 0, "Patched boot image is empty"
        print(f"[INFO] [GKI] Patch success: {patched_path}")

        print("[INFO] [GKI] Finalizing (Signing)...")

        shutil.copy(boot_img, mock_dirs["BASE_DIR"] / "boot.bak.img")

        final_output = strategy.finalize_patch(
            patched_path, mock_dirs["OUTPUT_ROOT_DIR"], mock_dirs["BASE_DIR"]
        )

        assert final_output.exists()
        assert final_output.name == "boot.img"
        print(f"[PASS] GKI Integration Test Complete. Output: {final_output}")


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
        version = match.group(1).decode("utf-8")
        return version

    raise ValueError("Could not find Linux version string in kernel binary")


def test_root_lkm(fw_pkg, tmp_path):
    if not fw_pkg:
        pytest.skip("Firmware package not available")

    boot_img = fw_pkg.get("boot.img")
    vbmeta_img = fw_pkg.get("vbmeta.img")
    init_boot_img = fw_pkg.get("init_boot.img")

    if not boot_img or not vbmeta_img or not init_boot_img:
        pytest.skip("Required images (boot, init_boot, vbmeta) missing")

    mock_dirs = {
        "TOOLS_DIR": tmp_path / "bin" / "tools",
        "DOWNLOAD_DIR": tmp_path / "bin" / "download",
        "OUTPUT_ROOT_LKM_DIR": tmp_path / "output" / "root_lkm",
        "IMAGE_DIR": tmp_path / "images",
        "BASE_DIR": tmp_path / "base",
    }
    for d in mock_dirs.values():
        d.mkdir(parents=True, exist_ok=True)

    with patch.multiple("ltbox.constants", **mock_dirs):
        print("\n[INFO] [LKM] Checking bundled magiskboot...")
        magiskboot_exe = mock_dirs["TOOLS_DIR"] / "magiskboot.exe"
        real_magiskboot = (
            Path(__file__).resolve().parents[2] / "bin" / "tools" / "magiskboot.exe"
        )

        if real_magiskboot.exists():
            shutil.copy(real_magiskboot, magiskboot_exe)

            for dll_file in real_magiskboot.parent.glob("*.dll"):
                shutil.copy(dll_file, mock_dirs["TOOLS_DIR"] / dll_file.name)
        else:
            pytest.skip("magiskboot.exe not found in bin/tools. Please build it first.")

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

        vbmeta_bak = mock_dirs["BASE_DIR"] / const.FN_VBMETA_BAK
        shutil.copy(vbmeta_img, vbmeta_bak)

        shutil.copy(boot_img, mock_dirs["BASE_DIR"] / "init_boot.bak.img")

        print(f"[INFO] [LKM] Using REAL {init_boot_img.name} from firmware...")
        target_init_boot = work_dir / "init_boot.img"
        shutil.copy(init_boot_img, target_init_boot)
        shutil.copy(init_boot_img, mock_dirs["BASE_DIR"] / "init_boot.bak.img")

        print("[INFO] [LKM] Running ACTUAL patch process...")
        try:
            patched_path = strategy.patch(
                work_dir, dev=None, lkm_kernel_version=detected_version_short
            )
        except Exception as e:
            pytest.fail(f"LKM Patching failed with real tools: {e}")

        assert patched_path.exists()
        print(f"[INFO] [LKM] Patch success: {patched_path}")

        print("[INFO] [LKM] Finalizing (AVB Chaining)...")
        final_output = strategy.finalize_patch(
            patched_path, mock_dirs["OUTPUT_ROOT_LKM_DIR"], mock_dirs["BASE_DIR"]
        )

        assert final_output.exists()
        assert final_output.name == "init_boot.img"
        assert (mock_dirs["OUTPUT_ROOT_LKM_DIR"] / "vbmeta.img").exists()

        print(f"[PASS] LKM Integration Test Complete. Output: {final_output}")


def test_root_magisk(fw_pkg, tmp_path):
    if not fw_pkg:
        pytest.skip("Firmware package not available")

    init_boot_img = fw_pkg.get("init_boot.img")
    vbmeta_img = fw_pkg.get("vbmeta.img")

    if not init_boot_img or not vbmeta_img:
        pytest.skip("Required images (init_boot, vbmeta) missing")

    mock_dirs = {
        "TOOLS_DIR": tmp_path / "bin" / "tools",
        "DOWNLOAD_DIR": tmp_path / "bin" / "download",
        "OUTPUT_ROOT_MAGISK_DIR": tmp_path / "output" / "root_magisk",
        "IMAGE_DIR": tmp_path / "images",
        "BASE_DIR": tmp_path / "base",
    }
    for d in mock_dirs.values():
        d.mkdir(parents=True, exist_ok=True)

    with patch.multiple("ltbox.constants", **mock_dirs):
        print("\n[INFO] [MAGISK] Checking bundled magiskboot...")
        magiskboot_exe = mock_dirs["TOOLS_DIR"] / "magiskboot.exe"
        real_magiskboot = (
            Path(__file__).resolve().parents[2] / "bin" / "tools" / "magiskboot.exe"
        )

        if real_magiskboot.exists():
            shutil.copy(real_magiskboot, magiskboot_exe)

            for dll_file in real_magiskboot.parent.glob("*.dll"):
                shutil.copy(dll_file, mock_dirs["TOOLS_DIR"] / dll_file.name)
        else:
            pytest.skip("magiskboot.exe not found in bin/tools. Please build it first.")

        strategy = MagiskRootStrategy()

        print("[INFO] [MAGISK] Downloading resources (APK)...")
        if not strategy.download_resources():
            pytest.fail("Failed to download Magisk resources")

        work_dir = tmp_path / "work_magisk"
        work_dir.mkdir()

        target_init_boot = work_dir / "init_boot.img"
        shutil.copy(init_boot_img, target_init_boot)

        shutil.copy(vbmeta_img, mock_dirs["BASE_DIR"] / const.FN_VBMETA_BAK)

        print("[INFO] [MAGISK] Running ACTUAL patch process...")
        try:
            patched_path = strategy.patch(work_dir, dev=None)
        except Exception as e:
            pytest.fail(f"Magisk patching failed with real tools: {e}")

        assert patched_path.exists(), "Patched init_boot image not returned"
        assert patched_path.stat().st_size > 0, "Patched init_boot image is empty"
        print(f"[INFO] [MAGISK] Patch success: {patched_path}")

        print("[INFO] [MAGISK] Finalizing (AVB Chaining)...")
        final_output = strategy.finalize_patch(
            patched_path, mock_dirs["OUTPUT_ROOT_MAGISK_DIR"], mock_dirs["BASE_DIR"]
        )

        assert final_output.exists()
        assert final_output.name == "init_boot.img"
        assert (mock_dirs["OUTPUT_ROOT_MAGISK_DIR"] / "vbmeta.img").exists()

        print(f"[PASS] Magisk Integration Test Complete. Output: {final_output}")


def test_root_folkpatch(fw_pkg, tmp_path):
    if not fw_pkg:
        pytest.skip("Firmware package not available")

    boot_img = fw_pkg.get("boot.img")
    vbmeta_img = fw_pkg.get("vbmeta.img")

    if not boot_img or not vbmeta_img:
        pytest.skip("Required images (boot, vbmeta) missing")

    mock_dirs = {
        "TOOLS_DIR": tmp_path / "bin" / "tools",
        "DOWNLOAD_DIR": tmp_path / "bin" / "download",
        "OUTPUT_ROOT_DIR": tmp_path / "output" / "root",
        "IMAGE_DIR": tmp_path / "images",
        "BASE_DIR": tmp_path / "base",
    }
    for d in mock_dirs.values():
        d.mkdir(parents=True, exist_ok=True)

    with patch.multiple("ltbox.constants", **mock_dirs):
        print("\n[INFO] [FOLKPATCH] Checking bundled magiskboot...")
        magiskboot_exe = mock_dirs["TOOLS_DIR"] / "magiskboot.exe"
        real_magiskboot = (
            Path(__file__).resolve().parents[2] / "bin" / "tools" / "magiskboot.exe"
        )

        if real_magiskboot.exists():
            shutil.copy(real_magiskboot, magiskboot_exe)

            for dll_file in real_magiskboot.parent.glob("*.dll"):
                shutil.copy(dll_file, mock_dirs["TOOLS_DIR"] / dll_file.name)
        else:
            pytest.skip("magiskboot.exe not found in bin/tools. Please build it first.")

        strategy = FolkPatchStrategy()

        print("[INFO] [FOLKPATCH] Downloading resources (kptools, APK)...")
        if not strategy.download_resources():
            pytest.fail("Failed to download FolkPatch resources")

        work_dir = tmp_path / "work_fp"
        work_dir.mkdir()

        target_boot = work_dir / "boot.img"
        shutil.copy(boot_img, target_boot)
        shutil.copy(vbmeta_img, mock_dirs["BASE_DIR"] / "vbmeta.bak.img")
        shutil.copy(boot_img, mock_dirs["BASE_DIR"] / "boot.bak.img")

        print("[INFO] [FOLKPATCH] Running ACTUAL patch process...")

        with patch("builtins.input", return_value="SuperKey1234"):
            try:
                patched_path = strategy.patch(work_dir, dev=None)
            except Exception as e:
                pytest.fail(f"FolkPatch patching failed with real tools: {e}")

        if patched_path is None:
            pytest.skip(
                "FolkPatch returned None, likely due to unsupported kernel (missing CONFIG_KALLSYMS). Skipping test."
            )

        assert patched_path.exists(), "Patched boot image not returned"
        assert patched_path.stat().st_size > 0, "Patched boot image is empty"
        print(f"[INFO] [FOLKPATCH] Patch success: {patched_path}")

        print("[INFO] [FOLKPATCH] Finalizing (AVB Chaining)...")
        final_output = strategy.finalize_patch(
            patched_path, mock_dirs["OUTPUT_ROOT_DIR"], mock_dirs["BASE_DIR"]
        )

        assert final_output.exists()
        assert final_output.name == "boot.img"
        print(f"[PASS] FolkPatch Integration Test Complete. Output: {final_output}")
