import hashlib
import json
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
from ltbox import crypto, downloader, utils

sys.path.append(str(Path(__file__).resolve().parents[2] / "bin"))


class TestUtils:
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

    @patch("ltbox.utils.subprocess.run")
    def test_run_cmd(self, mock_run):
        mock_run.return_value = subprocess.CompletedProcess(
            args=["echo"], returncode=0, stdout="ok", stderr=""
        )
        res = utils.run_command(["echo"], capture=True)
        assert res.returncode == 0
        assert "ok" in res.stdout

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
            patch("requests.get") as m_get,
            patch("ltbox.downloader.download_resource") as m_dl,
        ):
            m_get.return_value.json.return_value = resp
            m_get.return_value.status_code = 200

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
        result = subprocess.CompletedProcess(
            args=["cmd"], returncode=0, stdout=stdout, stderr=stderr
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

    def test_wildkernels_skip_testing_and_fallback_previous_release(self):
        releases = [
            {
                "tag_name": "v3",
                "draft": False,
                "body": "Contains TESTING marker",
                "assets": [
                    {
                        "name": "5.10-Normal-AnyKernel3.zip",
                        "browser_download_url": "http://testing",
                    }
                ],
            },
            {
                "tag_name": "v2",
                "draft": False,
                "body": "Stable release",
                "assets": [],
            },
            {
                "tag_name": "v1",
                "draft": False,
                "body": "Older stable release",
                "assets": [
                    {
                        "name": "5.10-Normal-AnyKernel3.zip",
                        "browser_download_url": "http://stable-old",
                    }
                ],
            },
        ]

        with (
            patch("requests.get") as m_get,
            patch("ltbox.downloader.download_resource") as m_dl,
        ):
            m_get.return_value.json.return_value = releases
            m_get.return_value.raise_for_status.return_value = None

            downloader._download_github_asset(
                "WildKernels/GKI_KernelSU_SUSFS",
                "latest",
                ".*Normal.*AnyKernel3\\.zip",
                Path("."),
            )

            args, _ = m_dl.call_args
            assert args[0] == "http://stable-old"

    def test_check_dependencies_blocks_source_download_without_edl_tools(
        self, tmp_path
    ):
        base_dir = tmp_path / "workspace"
        base_dir.mkdir()

        python_exe = tmp_path / "python.exe"
        adb_exe = tmp_path / "adb.exe"
        fastboot_exe = tmp_path / "fastboot.exe"
        avbtool_py = tmp_path / "avbtool.py"
        edl_exe = tmp_path / "fh_loader.exe"
        qs_exe = tmp_path / "Qsaharaserver.exe"

        for path in (python_exe, adb_exe, fastboot_exe, avbtool_py):
            path.write_text("ok", encoding="utf-8")

        with (
            patch("ltbox.utils.const.BASE_DIR", base_dir),
            patch("ltbox.utils.const.PYTHON_EXE", python_exe),
            patch("ltbox.utils.const.ADB_EXE", adb_exe),
            patch("ltbox.utils.const.FASTBOOT_EXE", fastboot_exe),
            patch("ltbox.utils.const.AVBTOOL_PY", avbtool_py),
            patch("ltbox.utils.const.EDL_EXE", edl_exe),
            patch("ltbox.utils.const.QSAHARASERVER_EXE", qs_exe),
            patch("ltbox.utils.const.KEY_MAP", {}),
            patch("ltbox.utils.ui") as mock_ui,
        ):
            with pytest.raises(RuntimeError):
                utils.check_dependencies()

            mock_ui.echo.assert_called_once_with(
                utils.get_string("utils_err_non_release_download")
            )

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
        edl_exe = tmp_path / "fh_loader.exe"
        qs_exe = tmp_path / "Qsaharaserver.exe"

        for path in (python_exe, adb_exe, fastboot_exe, avbtool_py):
            path.write_text("ok", encoding="utf-8")

        src_fh_loader = fw_pkg.get("fh_loader.exe")
        src_qsahara = fw_pkg.get("QSaharaServer.exe")
        assert src_fh_loader is not None
        assert src_qsahara is not None

        edl_exe.write_bytes(Path(src_fh_loader).read_bytes())
        qs_exe.write_bytes(Path(src_qsahara).read_bytes())

        with (
            patch("ltbox.utils.const.BASE_DIR", base_dir),
            patch("ltbox.utils.const.PYTHON_EXE", python_exe),
            patch("ltbox.utils.const.ADB_EXE", adb_exe),
            patch("ltbox.utils.const.FASTBOOT_EXE", fastboot_exe),
            patch("ltbox.utils.const.AVBTOOL_PY", avbtool_py),
            patch("ltbox.utils.const.EDL_EXE", edl_exe),
            patch("ltbox.utils.const.QSAHARASERVER_EXE", qs_exe),
            patch("ltbox.utils.const.KEY_MAP", {}),
            patch("ltbox.utils.ui") as mock_ui,
        ):
            utils.check_dependencies()

            mock_ui.echo.assert_called_once_with(utils.get_string("utils_deps_found"))

    def test_wildkernels_fallback_when_releases_json_invalid(self):
        releases_response = MagicMock()
        releases_response.raise_for_status.return_value = None
        releases_response.json.side_effect = ValueError("invalid json")

        latest_response = MagicMock()
        latest_response.raise_for_status.return_value = None
        latest_response.json.return_value = {
            "assets": [
                {
                    "name": "6.6.89-Normal-AnyKernel3.zip",
                    "browser_download_url": "http://latest-ok",
                }
            ]
        }

        with (
            patch("requests.get", side_effect=[releases_response, latest_response]),
            patch("ltbox.downloader.download_resource") as m_dl,
        ):
            downloader._download_github_asset(
                "WildKernels/GKI_KernelSU_SUSFS",
                "latest",
                "6\\.6\\.89.*Normal.*AnyKernel3\\.zip",
                Path("."),
            )

            args, _ = m_dl.call_args
            assert args[0] == "http://latest-ok"
