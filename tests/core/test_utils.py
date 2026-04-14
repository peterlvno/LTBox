import hashlib
import json
import subprocess
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from ltbox import crypto, downloader, utils
from ltbox.process_runner import CommandResult, CommandRunner, RunOptions


class TestUtils:
    def test_get_tool_env_sets_magiskboot_helper_when_present(self, tmp_path):
        tools_dir = tmp_path / "tools"
        tools_dir.mkdir()
        helper = tools_dir / "magiskboot_xz_helper.exe"
        helper.write_text("stub", encoding="utf-8")

        utils._get_tool_env.cache_clear()
        with (
            patch("ltbox.utils.const.TOOLS_DIR", tools_dir),
            patch.dict("ltbox.utils.os.environ", {"PATH": "C:\\base"}, clear=True),
        ):
            env = utils._get_tool_env()

        assert env["MAGISKBOOT_RUST_XZ_HELPER"] == str(helper)
        assert env["PATH"].split(";")[0] == str(tools_dir)
        utils._get_tool_env.cache_clear()

    def test_get_tool_env_skips_magiskboot_helper_when_missing(self, tmp_path):
        tools_dir = tmp_path / "tools"
        tools_dir.mkdir()

        utils._get_tool_env.cache_clear()
        with (
            patch("ltbox.utils.const.TOOLS_DIR", tools_dir),
            patch.dict("ltbox.utils.os.environ", {"PATH": "C:\\base"}, clear=True),
        ):
            env = utils._get_tool_env()

        assert "MAGISKBOOT_RUST_XZ_HELPER" not in env
        assert env["PATH"].split(";")[0] == str(tools_dir)
        utils._get_tool_env.cache_clear()

    @pytest.mark.parametrize(
        "cur, lat, exp",
        [
            ("v1.0.0", "v1.0.1", True),
            ("v1.0.1", "v1.0.0", False),
            ("1.0", "1.1", True),
        ],
    )
    def test_update_check(self, cur, lat, exp):
        assert utils.is_update_available(cur, lat) == exp

    @patch("ltbox.process_runner.subprocess.run")
    def test_run_cmd_capture(self, mock_run):
        mock_run.return_value = subprocess.CompletedProcess(
            args=["echo"], returncode=0, stdout="ok", stderr=""
        )
        with pytest.deprecated_call():
            res = utils.run_command(["echo"], capture=True)
        assert res.returncode == 0
        assert "ok" in res.stdout
        assert res.combined_output == "ok"

    @patch("ltbox.process_runner.subprocess.Popen")
    def test_run_cmd_stream(self, mock_popen):
        mock_proc = MagicMock()
        mock_proc.stdout = iter(["line1\n", "line2\n"])
        mock_proc.returncode = 0
        mock_popen.return_value = mock_proc

        on_output = MagicMock()
        result = CommandRunner().run(
            ["echo"], options=RunOptions(stream=True), on_output=on_output
        )

        assert result.stdout == "line1\nline2\n"
        assert result.stderr == ""
        assert result.returncode == 0
        assert result.combined_output == "line1\nline2\n"
        assert on_output.call_count == 2

    @patch("ltbox.process_runner.logger.info")
    @patch("ltbox.process_runner.subprocess.Popen")
    def test_run_cmd_stream_strips_embedded_tool_timestamps(
        self, mock_popen, mock_logger
    ):
        mock_proc = MagicMock()
        mock_proc.stdout = iter(
            ["02:27:05: INFO: Hello\n", " 02:27:05: Requested ID 13\n", "\n"]
        )
        mock_proc.returncode = 0
        mock_popen.return_value = mock_proc

        result = CommandRunner().run(["echo"], options=RunOptions(stream=True))

        assert result.stdout == "02:27:05: INFO: Hello\n 02:27:05: Requested ID 13\n\n"
        assert [call.args[0] for call in mock_logger.call_args_list] == [
            "INFO: Hello",
            "Requested ID 13",
            "",
        ]

    @patch("ltbox.process_runner.subprocess.run")
    def test_run_cmd_failure(self, mock_run):
        mock_run.return_value = subprocess.CompletedProcess(
            args=["echo"], returncode=1, stdout="", stderr="boom"
        )

        with pytest.raises(subprocess.CalledProcessError):
            CommandRunner().run(["echo"], options=RunOptions(capture=True, check=True))

    def test_pbkdf1(self):
        salt = b"1234567890123456"
        k1 = crypto.PBKDF1("OSD", salt, 32, hashlib.sha256, 1000)
        k2 = crypto.PBKDF1("OSD", salt, 32, hashlib.sha256, 1000)
        assert len(k1) == 32
        assert k1 == k2

    def test_bad_sig(self, tmp_path):
        f = tmp_path / "bad.enc"
        f.write_bytes(b"\x00" * 32 + b"junk")
        out = tmp_path / "out.bin"

        with patch("ltbox.utils.ui"):
            res = crypto.decrypt_file(str(f), str(out))
        assert res is False

    def test_asset_select(self):
        resp = {
            "assets": [
                {"name": "tool-linux.zip", "browser_download_url": "http://linux"},
                {"name": "tool-windows-x64.zip", "browser_download_url": "http://win"},
            ]
        }

        with (
            patch("ltbox.github_client.net.get_client") as get_session,
            patch("ltbox.downloader.download_resource") as m_dl,
        ):
            get_session.return_value.get.return_value.json.return_value = resp

            downloader._download_github_asset("r", "t", ".*windows.*", Path("."))

            args, _ = m_dl.call_args
            assert args[0] == "http://win"

    @pytest.mark.parametrize(
        "stdout, stderr, expected",
        [
            ("ok", "", "ok"),
            ("", "boom", "boom"),
            ("out", "err", "err\nout"),
        ],
    )
    def test_format_command_output(self, stdout, stderr, expected):
        result = CommandResult(
            stdout=stdout,
            stderr=stderr,
            returncode=0,
            combined_output=f"{stderr}{stdout}" if stderr else stdout,
        )
        assert utils.format_command_output(result) == expected

    def test_wait_for_files_eof_raises(self, tmp_path):
        target = tmp_path / "inputs"
        with patch("ltbox.utils.ui.prompt", side_effect=EOFError):
            with pytest.raises(RuntimeError):
                utils.wait_for_files(target, ["missing.bin"], "need files")

    def test_get_latest_release_versions(self):
        releases = [
            {"tag_name": "v1.0.0", "draft": False, "prerelease": False},
            {"tag_name": "v1.1.0", "draft": False, "prerelease": False},
            {"tag_name": "v2.0.0-beta", "draft": False, "prerelease": True},
            {"tag_name": "v2.0.0-alpha", "draft": False, "prerelease": True},
            {"tag_name": "v9.9.9", "draft": True, "prerelease": False},
        ]
        payload = json.dumps(releases).encode("utf-8")

        mock_response = MagicMock()
        mock_response.status = 200
        mock_response.read.return_value = payload

        mock_context = MagicMock()
        mock_context.__enter__.return_value = mock_response
        mock_context.__exit__.return_value = False

        with patch.object(urllib.request, "urlopen", return_value=mock_context):
            latest_release, latest_prerelease = utils.get_latest_release_versions(
                "owner", "repo"
            )

        assert latest_release == "v1.1.0"
        assert latest_prerelease == "v2.0.0-beta"

    def test_get_latest_release_versions_failure(self):
        with patch.object(
            urllib.request, "urlopen", side_effect=urllib.error.URLError("boom")
        ):
            latest_release, latest_prerelease = utils.get_latest_release_versions(
                "owner", "repo"
            )
        assert latest_release is None
        assert latest_prerelease is None

    def test_check_dependencies_blocks_source_download_without_edl_tools(
        self, tmp_path
    ):
        base_dir = tmp_path / "workspace"
        base_dir.mkdir()

        python_exe = tmp_path / "python.exe"
        adb_exe = tmp_path / "adb.exe"
        fastboot_exe = tmp_path / "fastboot.exe"
        avbtool_py = tmp_path / "avbtool.py"
        qdlrs_exe = tmp_path / "qdl-rs.exe"

        for path in (python_exe, adb_exe, fastboot_exe, avbtool_py):
            path.write_text("ok", encoding="utf-8")

        with (
            patch("ltbox.utils.const.BASE_DIR", base_dir),
            patch("ltbox.utils.const.PYTHON_EXE", python_exe),
            patch("ltbox.utils.const.ADB_EXE", adb_exe),
            patch("ltbox.utils.const.FASTBOOT_EXE", fastboot_exe),
            patch("ltbox.utils.const.AVBTOOL_PY", avbtool_py),
            patch("ltbox.utils.const.QDLRS_EXE", qdlrs_exe),
            patch("ltbox.utils.const.KEY_MAP", {}),
            patch("ltbox.utils._check_required_windows_drivers"),
            patch("ltbox.utils.ui") as mock_ui,
        ):
            with pytest.raises(RuntimeError):
                utils.check_dependencies()

            mock_ui.echo.assert_called_once_with(
                utils.get_string("utils_err_non_release_download")
            )

    @patch("ltbox.utils.subprocess.run")
    def test_driver_present_via_pnputil_matches_inf_name(self, mock_run):
        mock_run.return_value = subprocess.CompletedProcess(
            args=["pnputil", "/enum-drivers"],
            returncode=0,
            stdout=(
                "Published Name : oem12.inf\n"
                "Original Name  : qcser.inf\n"
                "Provider Name  : Qualcomm\n"
            ),
            stderr="",
        )

        assert utils._driver_present_via_pnputil(["qcser.inf"]) is True

    @patch("ltbox.utils.subprocess.run")
    def test_driver_present_via_pnputil_handles_command_failure(self, mock_run):
        mock_run.return_value = subprocess.CompletedProcess(
            args=["pnputil", "/enum-drivers"], returncode=1, stdout="", stderr="err"
        )

        assert utils._driver_present_via_pnputil(["qcser.inf"]) is False

    def test_driver_present_via_driver_store(self, tmp_path):
        driver_store = tmp_path / "System32" / "DriverStore" / "FileRepository"
        driver_store.mkdir(parents=True)
        (driver_store / "android_winusb.inf_amd64_abcd").mkdir()

        with patch.dict("ltbox.utils.os.environ", {"SystemRoot": str(tmp_path)}):
            assert (
                utils._driver_present_via_driver_store(["android_winusb.inf"]) is True
            )
            assert utils._driver_present_via_driver_store(["qcser.inf"]) is False

    def test_check_required_windows_drivers_auto_installs_on_missing(self):
        with (
            patch("ltbox.utils.os.name", "nt"),
            patch("ltbox.utils._is_driver_present", return_value=False),
            patch("ltbox.utils._auto_install_qualcomm_drivers") as mock_install,
            patch("ltbox.utils.ui") as mock_ui,
        ):
            utils._check_required_windows_drivers()

            mock_ui.warn.assert_called_once()
            mock_install.assert_called_once()

    @pytest.mark.integration
    def test_check_dependencies_allows_release_package_edl_tools(
        self, tmp_path, fw_pkg
    ):
        base_dir = tmp_path / "workspace"
        base_dir.mkdir()

        python_exe = tmp_path / "python.exe"
        adb_exe = tmp_path / "adb.exe"
        fastboot_exe = tmp_path / "fastboot.exe"
        avbtool_py = tmp_path / "avbtool.py"
        qdlrs_exe = tmp_path / "qdl-rs.exe"

        for path in (python_exe, adb_exe, fastboot_exe, avbtool_py, qdlrs_exe):
            path.write_text("ok", encoding="utf-8")

        with (
            patch("ltbox.utils.const.BASE_DIR", base_dir),
            patch("ltbox.utils.const.PYTHON_EXE", python_exe),
            patch("ltbox.utils.const.ADB_EXE", adb_exe),
            patch("ltbox.utils.const.FASTBOOT_EXE", fastboot_exe),
            patch("ltbox.utils.const.AVBTOOL_PY", avbtool_py),
            patch("ltbox.utils.const.QDLRS_EXE", qdlrs_exe),
            patch("ltbox.utils.const.KEY_MAP", {}),
            patch("ltbox.utils._check_required_windows_drivers"),
            patch("ltbox.utils.ui") as mock_ui,
        ):
            utils.check_dependencies()

            mock_ui.echo.assert_called_once_with(utils.get_string("utils_deps_found"))

    def test_resolve_extract_target_handles_prefixed_archive_paths(self):
        extract_map = {
            "avbtool.py": Path("avbtool.py"),
            "test/data/testkey_rsa4096.pem": Path("k1.pem"),
        }

        resolved = downloader._resolve_extract_target(
            "android_external_avb-main/test/data/testkey_rsa4096.pem",
            extract_map,
        )

        assert resolved == Path("k1.pem")
