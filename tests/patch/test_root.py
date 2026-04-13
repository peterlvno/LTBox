"""Tests for ltbox.patch.root – kernel version parsing and command build logic."""

import re
from unittest.mock import MagicMock, patch

import pytest

from ltbox.patch.root import (
    _find_magisk_preinit_device_from_mountinfo,
    get_kernel_version,
    patch_boot_with_root_algo,
    patch_magisk_boot,
)


class TestGetKernelVersion:
    def test_parses_linux_version_string(self, tmp_path):
        kernel_file = tmp_path / "kernel"
        # Embed a Linux version string in binary content
        content = b"\x00" * 256
        content += b"Linux version 6.1.75-android14-11-g4c37e535d153 (build@host)"
        content += b"\x00" * 128
        kernel_file.write_bytes(content)

        version = get_kernel_version(kernel_file)
        assert version == "6.1.75"

    def test_returns_none_for_missing_file(self, tmp_path):
        version = get_kernel_version(tmp_path / "nonexistent")
        assert version is None

    def test_returns_none_when_no_version_found(self, tmp_path):
        kernel_file = tmp_path / "kernel"
        kernel_file.write_bytes(b"\x00" * 1024)

        version = get_kernel_version(kernel_file)
        assert version is None

    def test_parses_real_kernel_version_format(self, tmp_path):
        kernel_file = tmp_path / "kernel"
        version_line = b"Linux version 5.15.137-android13-8-00001-gabcdef123456"
        # Pad to make the printable string at least 10 chars
        content = b"\xff" * 100 + version_line + b"\xff" * 100
        kernel_file.write_bytes(content)

        version = get_kernel_version(kernel_file)
        assert version == "5.15.137"


class TestGetKernelVersionIntegration:
    """Integration test using real firmware kernel."""

    @pytest.mark.integration
    def test_real_boot_kernel_version(self, fw_pkg, tmp_path):
        import shutil

        boot_img = fw_pkg.get("boot.img")
        if not boot_img:
            pytest.skip("boot.img not in firmware package")

        from ltbox import constants as const

        magiskboot = const.TOOLS_DIR / "magiskboot.exe"
        if not magiskboot.exists():
            pytest.skip("magiskboot.exe not built")

        work_dir = tmp_path / "work"
        work_dir.mkdir()
        shutil.copy(boot_img, work_dir / "boot.img")

        from ltbox.utils import MagiskBootWrapper

        mb = MagiskBootWrapper(magiskboot)
        mb.run("unpack", "boot.img", cwd=work_dir)

        kernel_file = work_dir / "kernel"
        if not kernel_file.exists():
            pytest.skip("magiskboot unpack did not produce kernel")

        version = get_kernel_version(kernel_file)
        assert version is not None
        assert re.match(r"\d+\.\d+\.\d+", version)


class TestPatchBootCommandBuild:
    """Test that patch_boot_with_root_algo builds correct commands without executing them."""

    def test_returns_none_when_image_missing(self, tmp_path):
        with patch("ltbox.patch.root.const.BASE_DIR", tmp_path):
            result = patch_boot_with_root_algo(
                work_dir=tmp_path,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                gki=False,
            )
        assert result is None

    def test_apatch_requires_superkey(self, tmp_path):
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "boot.img").write_bytes(b"\x00" * 64)

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_BOOT", "boot.img"),
            patch("ltbox.patch.root.const.FN_BOOT_ROOT", "boot_root.img"),
            patch(
                "ltbox.patch.root.get_root_provider_profile",
                return_value=MagicMock(
                    family=MagicMock(__eq__=lambda self, other: True),
                    display_name="APatch",
                ),
            ),
            patch("ltbox.utils.ui"),
        ):
            result = patch_boot_with_root_algo(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                gki=True,
                root_type="apatch",
                superkey=None,
            )
        assert result is None

    def test_apatch_builds_kpm_flags(self, tmp_path):
        """Verify kpm_paths are correctly appended to the patch command."""
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "boot.img").write_bytes(b"\x00" * 64)
        (work_dir / "kpimg").write_bytes(b"\x00" * 64)

        from ltbox.root_profiles import RootProviderFamily

        kpm1 = tmp_path / "mod1.kpm"
        kpm2 = tmp_path / "mod2.kpm"
        kpm1.touch()
        kpm2.touch()

        captured_cmds = []

        def fake_run(cmd, **kwargs):
            captured_cmds.append(cmd)
            # After unpack, create kernel file
            if "unpack" in cmd:
                (work_dir / "kernel").write_bytes(b"\x00" * 64)
            # After repack, create new-boot.img
            if "repack" in cmd:
                (work_dir / "new-boot.img").write_bytes(b"\x00" * 64)
            r = MagicMock()
            r.returncode = 0
            r.stdout = "CONFIG_KALLSYMS=y\nCONFIG_KALLSYMS_ALL=y"
            r.stderr = ""
            return r

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_BOOT", "boot.img"),
            patch("ltbox.patch.root.const.FN_BOOT_ROOT", "boot_root.img"),
            patch("ltbox.patch.root.const.TOOLS_DIR", tmp_path),
            patch(
                "ltbox.patch.root.get_root_provider_profile",
                return_value=MagicMock(
                    family=RootProviderFamily.APATCH,
                    display_name="FolkPatch",
                ),
            ),
            patch("ltbox.patch.root.subprocess.run", side_effect=fake_run),
            patch("ltbox.utils.ui"),
        ):
            result = patch_boot_with_root_algo(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                gki=True,
                root_type="apatch",
                superkey="mysuperkey",
                kpm_paths=[kpm1, kpm2],
            )

        assert result is not None

        # Find the patch command (the one with "-p" flag)
        patch_cmds = [c for c in captured_cmds if "-p" in c]
        assert len(patch_cmds) == 1
        patch_cmd = patch_cmds[0]

        # Verify -M and -T flags for both KPM modules
        assert "-M" in patch_cmd
        m_indices = [i for i, x in enumerate(patch_cmd) if x == "-M"]
        assert len(m_indices) == 2
        for idx in m_indices:
            assert patch_cmd[idx + 2] == "-T"
            assert patch_cmd[idx + 3] == "kpm"


class TestPatchMagiskBoot:
    def test_find_magisk_preinit_device_matches_official_metadata_selection(self):
        mountinfo = "\n".join(
            [
                "23 1 8:1 / /vendor ro,seclabel - ext4 /dev/block/sda1 ro,seclabel",
                "24 1 8:13 / /metadata rw,seclabel - ext4 /dev/block/sda13 rw,seclabel",
                "25 1 254:0 / /data rw,seclabel - ext4 /dev/block/dm-0 rw,seclabel",
            ]
        )

        result = _find_magisk_preinit_device_from_mountinfo(
            mountinfo=mountinfo,
            crypto_state="encrypted",
            crypto_type="file",
            crypto_metadata_enabled="true",
        )

        assert result == "sda13"

    def test_includes_preinitdevice_in_magisk_config_when_detected(self, tmp_path):
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "init_boot.img").write_bytes(b"\x00" * 64)
        (work_dir / "ramdisk.cpio").write_bytes(b"\x00" * 64)
        (work_dir / "magisk").write_bytes(b"\x00" * 64)
        (work_dir / "stub.apk").write_bytes(b"\x00" * 64)
        (work_dir / "init-ld").write_bytes(b"\x00" * 64)

        captured_config = None

        class FakeWrapper:
            def __init__(self, exe_path):
                self.exe_path = exe_path

            def run(self, *args, **kwargs):
                nonlocal captured_config
                result = MagicMock()
                result.returncode = 0
                result.stdout = ""
                result.stderr = ""

                if args[:3] == ("cpio", "ramdisk.cpio", "test"):
                    return result
                if args[:2] == ("sha1", "init_boot.img"):
                    result.stdout = "deadbeef\n"
                    return result
                if args[:2] == ("cpio", "ramdisk.cpio"):
                    captured_config = (work_dir / "config").read_text(encoding="utf-8")
                    return result
                if args[:2] == ("repack", "init_boot.img"):
                    (work_dir / "new-boot.img").write_bytes(b"\x00" * 64)
                    return result
                return result

        dev = MagicMock()
        dev.skip_adb = False

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_INIT_BOOT", "init_boot.img"),
            patch("ltbox.patch.root.const.FN_INIT_BOOT_ROOT", "init_boot_patched.img"),
            patch("ltbox.patch.root.utils.MagiskBootWrapper", FakeWrapper),
            patch(
                "ltbox.patch.root._resolve_magisk_preinit_device", return_value="sda13"
            ),
        ):
            result = patch_magisk_boot(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                dev=dev,
            )

        assert result == tmp_path / "init_boot_patched.img"
        assert captured_config is not None
        assert captured_config == (
            "KEEPVERITY=true\n"
            "KEEPFORCEENCRYPT=true\n"
            "RECOVERYMODE=false\n"
            "VENDORBOOT=false\n"
            "PREINITDEVICE=sda13\n"
            "SHA1=deadbeef\n"
        )

    def test_already_patched_image_aborts_without_restore_and_reboots_system(
        self, tmp_path
    ):
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "init_boot.img").write_bytes(b"\x00" * 64)
        (work_dir / "ramdisk.cpio").write_bytes(b"\x00" * 64)

        run_calls = []

        class FakeWrapper:
            def __init__(self, exe_path):
                self.exe_path = exe_path

            def run(self, *args, **kwargs):
                run_calls.append(args)
                result = MagicMock()
                result.returncode = (
                    1 if args[:3] == ("cpio", "ramdisk.cpio", "test") else 0
                )
                result.stdout = ""
                result.stderr = ""
                return result

        dev = MagicMock()
        dev.skip_adb = False

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_INIT_BOOT", "init_boot.img"),
            patch("ltbox.patch.root.const.FN_INIT_BOOT_ROOT", "init_boot_patched.img"),
            patch("ltbox.patch.root.utils.MagiskBootWrapper", FakeWrapper),
        ):
            result = patch_magisk_boot(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                dev=dev,
            )

        assert result is None
        assert ("cpio", "ramdisk.cpio", "restore") not in run_calls
        dev.adb.reboot.assert_called_once_with("system")

    def test_magisk_init_boot_patch_does_not_run_dtb_patch_steps(self, tmp_path):
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "init_boot.img").write_bytes(b"\x00" * 64)
        (work_dir / "ramdisk.cpio").write_bytes(b"\x00" * 64)
        (work_dir / "magisk").write_bytes(b"\x00" * 64)
        (work_dir / "stub.apk").write_bytes(b"\x00" * 64)
        (work_dir / "init-ld").write_bytes(b"\x00" * 64)
        (work_dir / "dtb").write_bytes(b"\x00" * 64)
        (work_dir / "kernel_dtb").write_bytes(b"\x00" * 64)
        (work_dir / "extra").write_bytes(b"\x00" * 64)

        run_calls = []

        class FakeWrapper:
            def __init__(self, exe_path):
                self.exe_path = exe_path

            def run(self, *args, **kwargs):
                run_calls.append(args)
                result = MagicMock()
                result.returncode = 0
                result.stdout = ""
                result.stderr = ""
                if args[:2] == ("sha1", "init_boot.img"):
                    result.stdout = "deadbeef\n"
                if args[:2] == ("repack", "init_boot.img"):
                    (work_dir / "new-boot.img").write_bytes(b"\x00" * 64)
                return result

        dev = MagicMock()
        dev.skip_adb = False

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_INIT_BOOT", "init_boot.img"),
            patch("ltbox.patch.root.const.FN_INIT_BOOT_ROOT", "init_boot_patched.img"),
            patch("ltbox.patch.root.utils.MagiskBootWrapper", FakeWrapper),
            patch(
                "ltbox.patch.root._resolve_magisk_preinit_device", return_value="sda13"
            ),
        ):
            result = patch_magisk_boot(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
                dev=dev,
            )

        assert result == tmp_path / "init_boot_patched.img"
        assert not any(call and call[0] == "dtb" for call in run_calls)

    def test_vendor_ramdisk_fallback_is_not_used_for_magisk(self, tmp_path):
        work_dir = tmp_path / "work"
        work_dir.mkdir()
        (work_dir / "init_boot.img").write_bytes(b"\x00" * 64)
        vendor_dir = work_dir / "vendor_ramdisk"
        vendor_dir.mkdir()
        (vendor_dir / "init_boot.cpio").write_bytes(b"\x00" * 64)

        class FakeWrapper:
            def __init__(self, exe_path):
                self.exe_path = exe_path

            def run(self, *args, **kwargs):
                result = MagicMock()
                result.returncode = 0
                result.stdout = ""
                result.stderr = ""
                return result

        with (
            patch("ltbox.patch.root.const.BASE_DIR", tmp_path),
            patch("ltbox.patch.root.const.FN_INIT_BOOT", "init_boot.img"),
            patch("ltbox.patch.root.const.FN_INIT_BOOT_ROOT", "init_boot_patched.img"),
            patch("ltbox.patch.root.utils.MagiskBootWrapper", FakeWrapper),
        ):
            result = patch_magisk_boot(
                work_dir=work_dir,
                magiskboot_exe=tmp_path / "magiskboot.exe",
            )

        assert result is None
