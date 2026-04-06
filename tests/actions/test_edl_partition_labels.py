from pathlib import Path
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


def test_flash_selected_partitions_fails_when_image_missing(mock_env):
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
            edl.flash_selected_partitions(MagicMock())


def test_flash_selected_partitions_writes_selected_entries(mock_env):
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
        edl.flash_selected_partitions(dev, skip_reset=True)

    assert dev.edl.write_partition.call_count == 2
    dev.edl.reset.assert_not_called()


def test_flash_selected_partitions_ab_slot_selection(mock_env):
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
        edl.flash_selected_partitions(dev, skip_reset=True)

    dev.edl.write_partition.assert_called_once_with(
        port="COM1",
        image_path=(img_dir / "boot.img"),
        lun="0",
        start_sector="200",
        partition_name="boot_b",
    )


def test_execute_partition_flash_targets_logs_success_once_per_target():
    dev = MagicMock()
    target = edl.PartitionFlashTarget(
        target_name="init_boot_a",
        image_path=Path("init_boot.img"),
        lun="4",
        start_sector="205962",
    )
    messages = {
        "act_flashing_target": "[*] Flashing {target}",
        "device_flashing_part": "[*] Writing '{filename}' ({lun}, {start})",
        "device_flash_success": "[+] Flashed '{filename}'.",
    }

    with (
        patch("ltbox.actions.edl.get_string", side_effect=messages.__getitem__),
        patch("ltbox.actions.edl.utils.ui") as mock_ui,
    ):
        edl._execute_partition_flash_targets(dev, "COM5", [target])

    success_message = messages["device_flash_success"].format(filename="init_boot.img")
    echoed_messages = [call.args[0] for call in mock_ui.echo.call_args_list]

    assert echoed_messages.count(success_message) == 1
