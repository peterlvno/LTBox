import xml.etree.ElementTree as ET
from unittest.mock import MagicMock, patch

from ltbox import constants as const
from ltbox.actions import edl
from ltbox.actions import xml as xml_action
from ltbox.actions.root import GkiRootStrategy
from ltbox.patch.avb import vbmeta_has_chain_partition


def create_xmls(img_dir, names):
    for n in names:
        (img_dir / n).touch()


def test_xml_select(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = [
        "rawprogram0.xml",
        "rawprogram1.xml",
        "rawprogram_unsparse0.xml",
        "rawprogram_save_persist_unsparse0.xml",
        "rawprogram_WIPE_PARTITIONS.xml",
        "patch0.xml",
    ]
    create_xmls(img_dir, files)

    with patch("ltbox.actions.edl.utils.ui"):
        raw, patch_files = edl._select_flash_xmls(skip_dp=False)

    r_names = [p.name for p in raw]
    p_names = [p.name for p in patch_files]

    assert "rawprogram_WIPE_PARTITIONS.xml" not in r_names
    assert "rawprogram0.xml" not in r_names
    assert "rawprogram1.xml" in r_names
    assert "rawprogram_save_persist_unsparse0.xml" in r_names
    assert "rawprogram_unsparse0.xml" not in r_names
    assert "patch0.xml" in p_names


def test_flash_args(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    files = ["rawprogram1.xml", "rawprogram_unsparse0.xml", "patch0.xml"]
    create_xmls(img_dir, files)

    mock_dev = MagicMock()

    with (
        patch("ltbox.actions.edl.utils.ui") as mock_ui,
        patch("ltbox.actions.edl.ensure_loader_file"),
        patch("ltbox.actions.edl._prepare_flash_files"),
        patch("builtins.input", return_value="y"),
    ):
        mock_ui.prompt.return_value = "y"

        edl.flash_full_firmware(mock_dev, skip_reset=True, skip_reset_edl=False)

        args, _ = mock_dev.edl.flash_rawprogram.call_args
        passed = [p.name for p in args[3]]

        assert "rawprogram_unsparse0.xml" in passed
        assert len(passed) == 2


def test_xml_fallback(mock_env):
    out_dir = mock_env["OUTPUT_XML_DIR"]
    target = out_dir / "rawprogram_save_persist_unsparse0.xml"

    cases = [
        (
            ["rawprogram_unsparse0.xml", "rawprogram0.xml"],
            "rawprogram_unsparse0.xml",
            "A",
        ),
        (["rawprogram0.xml"], "rawprogram0.xml", "B"),
    ]
    tmpl = """<?xml version=\"1.0\" ?><data><program label=\"{m}\" filename=\"\"/></data>"""

    for fnames, expected, marker in cases:
        if target.exists():
            target.unlink()
        for f in out_dir.glob("*.xml"):
            f.unlink()

        for fn in fnames:
            m = marker if fn == expected else "X"
            (out_dir / fn).write_text(tmpl.format(m=m))

        with patch("ltbox.actions.xml.utils.ui"):
            xml_action._ensure_rawprogram_save_persist(out_dir)

        assert target.exists()
        root = ET.parse(target).getroot()
        assert root.find("program").get("label") == marker


def test_gki_strategy_requires_vbmeta_file():
    strategy = GkiRootStrategy()

    assert const.FN_BOOT in strategy.required_files
    assert const.FN_VBMETA in strategy.required_files


def test_vbmeta_has_chain_partition_parses_descriptor(tmp_path):
    vbmeta_img = tmp_path / "vbmeta.img"
    vbmeta_img.write_bytes(b"dummy")

    mock_proc = MagicMock()
    mock_proc.stdout = """Descriptors:
    Chain Partition descriptor:
      Partition Name:          recovery
    Chain Partition descriptor:
      Partition Name:          boot
"""

    with patch("ltbox.patch.avb.utils.AvbToolWrapper") as mock_avbtool:
        mock_avbtool.return_value.run.return_value = mock_proc

        assert vbmeta_has_chain_partition(vbmeta_img, "boot") is True
        assert vbmeta_has_chain_partition(vbmeta_img, "init_boot") is False


def test_gki_finalize_patch_rebuilds_vbmeta_when_boot_chain_missing(tmp_path):
    strategy = GkiRootStrategy()
    patched_boot = tmp_path / "boot_patched.img"
    patched_boot.write_bytes(b"patched")
    output_dir = tmp_path / "output"
    output_dir.mkdir()

    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()
    (backup_dir / const.FN_VBMETA_BAK).write_bytes(b"vbmeta")

    with (
        patch("ltbox.actions.root.process_boot_image_avb") as process_avb,
        patch("ltbox.actions.root.vbmeta_has_chain_partition", return_value=False),
        patch(
            "ltbox.actions.root.rebuild_vbmeta_with_chained_images"
        ) as rebuild_vbmeta,
        patch("ltbox.actions.root.const.BASE_DIR", tmp_path),
    ):
        (tmp_path / const.FN_VBMETA_ROOT).write_bytes(b"new-vbmeta")

        final_boot = strategy.finalize_patch(patched_boot, output_dir, backup_dir)

    assert final_boot == output_dir / const.FN_BOOT
    assert final_boot.exists()
    process_avb.assert_called_once()
    rebuild_vbmeta.assert_called_once()
    assert (output_dir / const.FN_VBMETA).exists()


def test_gki_finalize_patch_skips_vbmeta_rebuild_when_boot_chain_exists(tmp_path):
    strategy = GkiRootStrategy()
    patched_boot = tmp_path / "boot_patched.img"
    patched_boot.write_bytes(b"patched")
    output_dir = tmp_path / "output"
    output_dir.mkdir()

    backup_dir = tmp_path / "backup"
    backup_dir.mkdir()
    (backup_dir / const.FN_VBMETA_BAK).write_bytes(b"vbmeta")

    with (
        patch("ltbox.actions.root.process_boot_image_avb") as process_avb,
        patch("ltbox.actions.root.vbmeta_has_chain_partition", return_value=True),
        patch(
            "ltbox.actions.root.rebuild_vbmeta_with_chained_images"
        ) as rebuild_vbmeta,
    ):
        final_boot = strategy.finalize_patch(patched_boot, output_dir, backup_dir)

    assert final_boot == output_dir / const.FN_BOOT
    assert final_boot.exists()
    process_avb.assert_called_once()
    rebuild_vbmeta.assert_not_called()
    assert not (output_dir / const.FN_VBMETA).exists()
