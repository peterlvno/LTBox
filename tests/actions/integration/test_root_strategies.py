import shutil
from contextlib import nullcontext
from unittest.mock import patch

import pytest
from ltbox import constants as const
from ltbox.actions.root_strategies import (
    APatchStrategy,
    GkiRootStrategy,
    LkmRootStrategy,
)

from .fixtures import (
    _copy_bundled_kptools,
    _copy_bundled_magiskboot,
    _prepare_mock_dirs,
    _run_patch_with_fail_context,
    _setup_apatch_context,
    _setup_gki_context,
    extract_kernel_version_from_img,
)

pytestmark = pytest.mark.integration


@pytest.mark.parametrize(
    "label,strategy_cls,output_key,download_message,download_error,setup_fn,final_name,patch_kwargs,input_value,required_files",
    [
        pytest.param(
            "GKI",
            GkiRootStrategy,
            "OUTPUT_ROOT_DIR",
            "[INFO] [GKI] Downloading resources (Manager APK)...",
            "Failed to download GKI resources",
            _setup_gki_context,
            "boot.img",
            None,
            None,
            ("boot.img",),
            id="gki",
        ),
        pytest.param(
            "FOLKPATCH",
            APatchStrategy,
            "OUTPUT_ROOT_DIR",
            "[INFO] [FOLKPATCH] Downloading resources (kptools, APK)...",
            "Failed to download APatch family resources",
            _setup_apatch_context,
            "boot.img",
            None,
            "SuperKey1234",
            ("boot.img", "vbmeta.img"),
            id="folkpatch",
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
    required_files,
):
    loaded = firmware_file_getter(*required_files)
    boot_img = loaded[0]
    vbmeta_img = loaded[1] if len(loaded) > 1 else None

    mock_dirs = _prepare_mock_dirs(tmp_path, root_output_key=output_key)

    with patch.multiple("ltbox.constants", **mock_dirs):
        _copy_bundled_magiskboot(mock_dirs["TOOLS_DIR"])
        if strategy_cls is APatchStrategy:
            _copy_bundled_kptools(mock_dirs["TOOLS_DIR"])

        strategy = strategy_cls()

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
                "APatch family returned None, likely due to unsupported kernel (missing CONFIG_KALLSYMS). Skipping test."
            )

        assert patched_path.exists(), "Patched boot image not returned"
        assert patched_path.stat().st_size > 0, "Patched boot image is empty"

        final_output = strategy.finalize_patch(
            patched_path,
            mock_dirs[output_key],
            mock_dirs["BASE_DIR"],
        )

        assert final_output.exists()
        assert final_output.name == final_name


def test_root_lkm(firmware_file_getter, tmp_path):
    boot_img, vbmeta_img, init_boot_img = firmware_file_getter(
        "boot.img", "vbmeta.img", "init_boot.img"
    )

    mock_dirs = _prepare_mock_dirs(tmp_path, root_output_key="OUTPUT_ROOT_LKM_DIR")

    with patch.multiple("ltbox.constants", **mock_dirs):
        magiskboot_exe = _copy_bundled_magiskboot(mock_dirs["TOOLS_DIR"])

        try:
            detected_version_full = extract_kernel_version_from_img(
                boot_img, magiskboot_exe, tmp_path
            )
            detected_version_short = ".".join(detected_version_full.split(".")[:2])
        except Exception as e:
            pytest.fail(f"Failed to extract kernel version: {e}")

        strategy = LkmRootStrategy()
        strategy.staging_dir.mkdir(parents=True, exist_ok=True)

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

        shutil.copy(init_boot_img, work_dir / "init_boot.img")
        shutil.copy(init_boot_img, mock_dirs["BASE_DIR"] / "init_boot.bak.img")

        patched_path = _run_patch_with_fail_context(
            strategy,
            work_dir,
            "LKM",
            patch_kwargs={"lkm_kernel_version": detected_version_short},
        )

        assert patched_path.exists()

        final_output = strategy.finalize_patch(
            patched_path,
            mock_dirs["OUTPUT_ROOT_LKM_DIR"],
            mock_dirs["BASE_DIR"],
        )

        assert final_output.exists()
        assert final_output.name == "init_boot.img"
        assert (mock_dirs["OUTPUT_ROOT_LKM_DIR"] / "vbmeta.img").exists()
