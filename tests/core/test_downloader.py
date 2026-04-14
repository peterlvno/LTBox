"""Tests for ltbox.downloader – archive extraction and helper functions."""

import contextlib
import io
import tarfile
import zipfile
from pathlib import Path
from unittest.mock import patch

import pytest

from ltbox.downloader import (
    _resolve_extract_target,
    download_apatch_nightly,
    download_ksuinit_release,
    download_nightly_artifacts,
    prepare_magisk_apk,
    extract_kernel_from_anykernel3_zip,
    extract_archive_files,
)
from ltbox.errors import ToolError


class TestResolveExtractTarget:
    def test_exact_match(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        assert _resolve_extract_target("boot.img", extract_map) == Path("/out/boot.img")

    def test_prefix_stripped(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        assert _resolve_extract_target("./boot.img", extract_map) == Path(
            "/out/boot.img"
        )

    def test_nested_suffix_match(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        result = _resolve_extract_target("firmware/images/boot.img", extract_map)
        assert result == Path("/out/boot.img")

    def test_no_match_returns_none(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        assert _resolve_extract_target("vbmeta.img", extract_map) is None

    def test_path_traversal_rejected(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        assert _resolve_extract_target("../etc/passwd", extract_map) is None

    def test_double_dot_in_middle_rejected(self):
        extract_map = {"boot.img": Path("/out/boot.img")}
        assert _resolve_extract_target("a/../boot.img", extract_map) is None


class TestExtractArchiveFiles:
    def test_extract_from_zip(self, tmp_path):
        zip_path = tmp_path / "test.zip"
        target = tmp_path / "boot.img"
        content = b"fake boot image content"

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("boot.img", content)

        extract_map = {"boot.img": target}

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(zip_path, extract_map)

        assert target in result
        assert target.read_bytes() == content

    def test_extract_from_tar(self, tmp_path):
        tar_path = tmp_path / "test.tar"
        target = tmp_path / "vbmeta.img"
        content = b"fake vbmeta content"

        with tarfile.open(tar_path, "w") as tf:
            info = tarfile.TarInfo(name="vbmeta.img")
            info.size = len(content)
            tf.addfile(info, io.BytesIO(content))

        extract_map = {"vbmeta.img": target}

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(tar_path, extract_map)

        assert target in result
        assert target.read_bytes() == content

    def test_extract_nested_file_from_zip(self, tmp_path):
        zip_path = tmp_path / "nested.zip"
        target = tmp_path / "init_boot.img"
        content = b"nested init boot"

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("images/init_boot.img", content)

        extract_map = {"init_boot.img": target}

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(zip_path, extract_map)

        assert target in result
        assert target.read_bytes() == content

    def test_extract_skips_unmatched_members(self, tmp_path):
        zip_path = tmp_path / "multi.zip"
        target = tmp_path / "boot.img"

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("boot.img", b"boot")
            zf.writestr("readme.txt", b"ignored")

        extract_map = {"boot.img": target}

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(zip_path, extract_map)

        assert len(result) == 1

    def test_bad_zip_raises_tool_error(self, tmp_path):
        bad_zip = tmp_path / "bad.zip"
        bad_zip.write_bytes(b"not a zip file at all")

        with patch("ltbox.utils.ui"):
            with pytest.raises(ToolError):
                extract_archive_files(bad_zip, {"boot.img": tmp_path / "boot.img"})

    def test_extract_multiple_files_from_zip(self, tmp_path):
        zip_path = tmp_path / "multi.zip"
        boot_target = tmp_path / "boot.img"
        vbmeta_target = tmp_path / "vbmeta.img"

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("boot.img", b"boot content")
            zf.writestr("vbmeta.img", b"vbmeta content")

        extract_map = {
            "boot.img": boot_target,
            "vbmeta.img": vbmeta_target,
        }

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(zip_path, extract_map)

        assert boot_target in result
        assert vbmeta_target in result
        assert boot_target.read_bytes() == b"boot content"
        assert vbmeta_target.read_bytes() == b"vbmeta content"

    def test_extract_from_tar_gz(self, tmp_path):
        tar_path = tmp_path / "test.tar.gz"
        target = tmp_path / "boot.img"
        content = b"compressed boot image"

        with tarfile.open(tar_path, "w:gz") as tf:
            info = tarfile.TarInfo(name="boot.img")
            info.size = len(content)
            tf.addfile(info, io.BytesIO(content))

        extract_map = {"boot.img": target}

        with patch("ltbox.utils.ui"):
            result = extract_archive_files(tar_path, extract_map)

        assert target in result
        assert target.read_bytes() == content


class TestExtractKernelFromZip:
    def test_prefers_image_file(self, tmp_path):
        zip_path = tmp_path / "kernel.zip"
        work_dir = tmp_path / "work"
        work_dir.mkdir()

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("Image", b"image-bytes")
            zf.writestr("kernel", b"kernel-bytes")

        with patch("ltbox.utils.ui"):
            result = extract_kernel_from_anykernel3_zip(zip_path, work_dir)

        assert result.name == "Image"
        assert result.read_bytes() == b"image-bytes"

    def test_falls_back_to_kernel_file(self, tmp_path):
        zip_path = tmp_path / "kernel.zip"
        work_dir = tmp_path / "work"
        work_dir.mkdir()

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("kernel", b"kernel-bytes")

        with patch("ltbox.utils.ui"):
            result = extract_kernel_from_anykernel3_zip(zip_path, work_dir)

        assert result.name == "kernel"
        assert result.read_bytes() == b"kernel-bytes"

    def test_raises_when_image_and_kernel_missing(self, tmp_path):
        zip_path = tmp_path / "kernel.zip"
        work_dir = tmp_path / "work"
        work_dir.mkdir()

        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr("README.txt", b"no kernel here")

        with patch("ltbox.utils.ui"):
            with pytest.raises(ToolError):
                extract_kernel_from_anykernel3_zip(zip_path, work_dir)


def test_download_apatch_nightly_prefers_release_artifact(tmp_path):
    def fake_download(url: str, dest_path: Path, *args, **kwargs):
        assert url.endswith("/APatch-Release.zip")
        with zipfile.ZipFile(dest_path, "w") as archive:
            archive.writestr("nested/APatch.apk", b"apk-bytes")

    with (
        patch(
            "ltbox.downloader._get_matching_workflow_artifacts",
            return_value=["APatch-Debug", "APatch-Release", "mappings"],
        ),
        patch("ltbox.downloader.download_resource", side_effect=fake_download),
        patch("ltbox.downloader._extract_apatch_kpimg") as extract_kpimg,
        patch(
            "ltbox.downloader.utils.ui.status",
            return_value=contextlib.nullcontext(),
        ),
        patch("ltbox.downloader.utils.ui.echo"),
    ):
        download_apatch_nightly(
            "run-123",
            tmp_path,
            repo="bmax121/APatch",
            name="APatch",
            workflow_file="build.yml",
            branch="main",
        )

    extract_kpimg.assert_called_once_with(tmp_path / "FolkPatch.apk", tmp_path)


def test_download_apatch_nightly_raises_when_provider_artifact_missing(tmp_path):
    with patch(
        "ltbox.downloader._get_matching_workflow_artifacts",
        return_value=["mappings"],
    ):
        with pytest.raises(ToolError, match="No APatch artifact found"):
            download_apatch_nightly(
                "run-123",
                tmp_path,
                repo="bmax121/APatch",
                name="APatch",
                workflow_file="build.yml",
                branch="main",
            )


def test_download_ksuinit_release_uses_resolved_artifact_name(tmp_path):
    target_path = tmp_path / "ksuinit"

    def fake_download(url: str, dest_path: Path, *args, **kwargs):
        assert url.endswith("/ksuinit-aarch64-linux-android.zip")
        with zipfile.ZipFile(dest_path, "w") as archive:
            archive.writestr("bin/ksuinit", b"ksuinit-binary")

    with (
        patch("ltbox.downloader._get_workflow_run_id_for_tag", return_value="run-123"),
        patch(
            "ltbox.downloader._get_workflow_run_artifacts",
            return_value=["ksuinit-aarch64-linux-android"],
        ),
        patch("ltbox.downloader.download_resource", side_effect=fake_download),
        patch(
            "ltbox.downloader.utils.ui.status",
            return_value=contextlib.nullcontext(),
        ),
        patch("ltbox.downloader.utils.ui.echo"),
    ):
        download_ksuinit_release(target_path, repo="owner/repo", tag="v1.2.3")

    assert target_path.read_bytes() == b"ksuinit-binary"


def test_download_nightly_artifacts_validates_artifacts_before_download(tmp_path):
    with (
        patch(
            "ltbox.downloader._get_matching_workflow_artifacts",
            return_value=["mappings"],
        ),
        patch(
            "ltbox.downloader.utils.ui.status",
            return_value=contextlib.nullcontext(),
        ),
        patch("ltbox.downloader.utils.ui.echo"),
        patch("ltbox.downloader.download_resource") as download_resource,
    ):
        with pytest.raises(ToolError, match="Failed to download manager artifact"):
            download_nightly_artifacts(
                repo="owner/repo",
                workflow_id="run-123",
                manager_name="manager.zip",
                mapped_name="android14-6.1",
                target_dir=tmp_path,
                workflow_file="build.yml",
                branch="main",
            )

    download_resource.assert_not_called()


def test_prepare_magisk_apk_extracts_payloads_and_copies_manager(tmp_path):
    apk_path = tmp_path / "MagiskAlpha.apk"
    tools_dir = tmp_path / "tools"
    target_dir = tmp_path / "staging"
    target_dir.mkdir()
    tools_dir.mkdir()

    with zipfile.ZipFile(apk_path, "w") as archive:
        archive.writestr("lib/arm64-v8a/libmagiskinit.so", b"magiskinit")
        archive.writestr("lib/arm64-v8a/libmagisk.so", b"magisk64")
        archive.writestr("lib/arm64-v8a/libinit-ld.so", b"init-ld")
        archive.writestr("assets/stub.apk", b"stub")

    with patch("ltbox.downloader.const.TOOLS_DIR", tools_dir):
        prepare_magisk_apk(apk_path, target_dir)

    assert (target_dir / "magiskinit").read_bytes() == b"magiskinit"
    assert (target_dir / "magisk").read_bytes() == b"magisk64"
    assert (target_dir / "init-ld").read_bytes() == b"init-ld"
    assert (target_dir / "stub.apk").read_bytes() == b"stub"
    assert (tools_dir / "manager.apk").read_bytes() == apk_path.read_bytes()
