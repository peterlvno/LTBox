import re
import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import Any, Optional, TypedDict

import pytest

_REAL_TOOLS_DIR = Path(__file__).resolve().parents[3] / "bin" / "tools"
MAGISKBOOT_PATH = _REAL_TOOLS_DIR / "magiskboot.exe"
MAGISKBOOT_XZ_HELPER_PATH = _REAL_TOOLS_DIR / "magiskboot_xz_helper.exe"
KPTOOLS_PATH = _REAL_TOOLS_DIR / "kptools.exe"


class RootSetupContext(TypedDict):
    boot_img: Path
    vbmeta_img: Optional[Path]
    work_dir: Path
    base_dir: Path


@pytest.fixture
def firmware_file_getter(request, fw_pkg):
    if not request.config.getoption("--run-integration"):
        pytest.skip("integration tests require --run-integration")

    if not fw_pkg:
        pytest.skip("Firmware package not available")

    def _get(*names: str) -> tuple[Path, ...]:
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
    if MAGISKBOOT_XZ_HELPER_PATH.exists():
        shutil.copy(MAGISKBOOT_XZ_HELPER_PATH, tools_dir / "magiskboot_xz_helper.exe")
    for dll_file in MAGISKBOOT_PATH.parent.glob("*.dll"):
        shutil.copy(dll_file, tools_dir / dll_file.name)
    return magiskboot_exe


def _copy_bundled_kptools(tools_dir: Path) -> Path:
    if not KPTOOLS_PATH.exists():
        pytest.skip("kptools.exe not found in bin/tools. Bundle it via CI first.")

    kptools_exe = tools_dir / "kptools.exe"
    shutil.copy(KPTOOLS_PATH, kptools_exe)
    return kptools_exe


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
        raise AssertionError("unreachable")


def _setup_gki_context(ctx: RootSetupContext) -> None:
    shutil.copy(ctx["boot_img"], ctx["work_dir"] / "boot.img")
    shutil.copy(ctx["boot_img"], ctx["base_dir"] / "boot.bak.img")

    unpack_dir = ctx["work_dir"] / "gki_zip_src"
    unpack_dir.mkdir(exist_ok=True)
    shutil.copy(ctx["boot_img"], unpack_dir / "boot.img")

    try:
        subprocess.run(
            [str(MAGISKBOOT_PATH), "unpack", "boot.img"],
            cwd=str(unpack_dir),
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError as exc:
        pytest.fail(f"Failed to unpack boot image for GKI zip setup: {exc}")

    kernel_file = unpack_dir / "kernel"
    if not kernel_file.exists():
        pytest.fail("Kernel file not found after unpacking boot.img for GKI zip setup")

    with zipfile.ZipFile(ctx["work_dir"] / "AnyKernel3.zip", "w") as archive:
        archive.write(kernel_file, "Image")
        archive.writestr("manager.apk", b"fake-apk")


def _setup_apatch_context(ctx: RootSetupContext) -> None:
    shutil.copy(ctx["boot_img"], ctx["work_dir"] / "boot.img")
    vbmeta_img = ctx["vbmeta_img"]
    if vbmeta_img is None:
        raise AssertionError("APatch requires vbmeta image in setup context")
    shutil.copy(vbmeta_img, ctx["base_dir"] / "vbmeta.bak.img")
    shutil.copy(ctx["boot_img"], ctx["base_dir"] / "boot.bak.img")


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
