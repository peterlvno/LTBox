import shutil
import xml.etree.ElementTree as ET
from unittest.mock import MagicMock, patch

import pytest
from ltbox import constants as const
from ltbox.actions import xml as xml_action


pytestmark = pytest.mark.integration


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
