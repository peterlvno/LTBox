from unittest.mock import MagicMock, patch

import pytest
from ltbox.actions import edl


def test_collect_base_partitions_from_xml(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    (img_dir / "rawprogram0.xml").write_text(
        """<?xml version='1.0'?><data>
        <program label='boot_a' filename='boot_a.img' physical_partition_number='0' start_sector='100'/>
        <program label='boot_b' filename='' physical_partition_number='0' start_sector='200'/>
        <program label='super' filename='super.img' physical_partition_number='0' start_sector='300'/>
        <program label='persist' filename='' physical_partition_number='0' start_sector='400'/>
        </data>""",
        encoding="utf-8",
    )

    with patch("ltbox.actions.edl.xml.ensure_xml_files"):
        part_map = edl._collect_base_partitions()

    assert sorted(part_map.keys()) == ["boot", "super"]

    assert part_map["boot"]["is_ab"] is True
    assert len(part_map["boot"]["a"]) == 1
    assert part_map["boot"]["a"][0]["filename"] == "boot_a.img"
    assert len(part_map["boot"]["b"]) == 1
    assert part_map["boot"]["b"][0]["filename"] == ""

    assert part_map["super"]["is_ab"] is False
    assert len(part_map["super"]["none"]) == 1
    assert part_map["super"]["none"][0]["filename"] == "super.img"


def test_flash_partition_labels_fails_when_image_missing(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    (img_dir / "rawprogram0.xml").write_text(
        """<?xml version='1.0'?><data>
        <program label='super' filename='super.img' physical_partition_number='0' start_sector='100'/>
        </data>""",
        encoding="utf-8",
    )

    with (
        patch("ltbox.actions.edl.xml.ensure_xml_files"),
        patch("ltbox.actions.edl._prompt_partition_selection", return_value=["super"]),
    ):
        with pytest.raises(FileNotFoundError):
            edl.flash_partition_labels(MagicMock())


def test_flash_partition_labels_writes_selected_entries(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    (img_dir / "rawprogram0.xml").write_text(
        """<?xml version='1.0'?><data>
        <program label='super' filename='super_1.img' physical_partition_number='0' start_sector='100'/>
        <program label='super' filename='super_2.img' physical_partition_number='0' start_sector='200'/>
        </data>""",
        encoding="utf-8",
    )
    (img_dir / "super_1.img").write_text("a", encoding="utf-8")
    (img_dir / "super_2.img").write_text("b", encoding="utf-8")

    dev = MagicMock()
    dev.edl_session.return_value.__enter__.return_value = "COM1"

    with (
        patch("ltbox.actions.edl.xml.ensure_xml_files"),
        patch("ltbox.actions.edl._prompt_partition_selection", return_value=["super"]),
        patch("ltbox.actions.edl.ensure_edl_requirements"),
    ):
        edl.flash_partition_labels(dev, skip_reset=True)

    assert dev.edl.write_partition.call_count == 2
    dev.edl.reset.assert_not_called()


def test_flash_partition_labels_ab_slot_selection(mock_env):
    img_dir = mock_env["IMAGE_DIR"]
    (img_dir / "rawprogram0.xml").write_text(
        """<?xml version='1.0'?><data>
        <program label='boot_a' filename='boot.img' physical_partition_number='0' start_sector='100'/>
        <program label='boot_b' filename='' physical_partition_number='0' start_sector='200'/>
        </data>""",
        encoding="utf-8",
    )
    (img_dir / "boot.img").write_text("boot_data", encoding="utf-8")

    dev = MagicMock()
    dev.edl_session.return_value.__enter__.return_value = "COM1"

    with (
        patch("ltbox.actions.edl.xml.ensure_xml_files"),
        patch("ltbox.actions.edl._prompt_partition_selection", return_value=["boot"]),
        patch("ltbox.utils.ui.prompt", return_value="2"),
        patch("ltbox.actions.edl.ensure_edl_requirements"),
    ):
        edl.flash_partition_labels(dev, skip_reset=True)

    dev.edl.write_partition.assert_called_once_with(
        port="COM1", image_path=(img_dir / "boot.img"), lun="0", start_sector="200"
    )
