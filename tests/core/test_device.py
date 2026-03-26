import xml.etree.ElementTree as ET
from unittest.mock import MagicMock, patch

import pytest
from ltbox.device import AdbManager, EdlManager


def test_adb_get_model_retry_success():
    manager = AdbManager(skip_adb=False)

    mock_device = MagicMock()
    mock_device.get_state.return_value = "device"
    mock_device.prop.model = "Lenovo TB-Test"

    with (
        patch(
            "adbutils.adb.device_list",
            side_effect=[[], [], [mock_device], [mock_device]],
        ),
        patch("ltbox.device.time.sleep", return_value=None),
    ):
        model = manager.get_model()
        assert model == "Lenovo TB-Test"


def test_fastboot_slot_detection_failure():
    import subprocess

    from ltbox.device import DeviceCommandError, FastbootManager

    manager = FastbootManager()

    with patch(
        "ltbox.utils.run_command", side_effect=subprocess.CalledProcessError(1, "cmd")
    ):
        with pytest.raises(DeviceCommandError):
            manager.get_slot_suffix()


def test_adb_reboot_edl_does_not_force_kill_processes():
    manager = AdbManager(skip_adb=False)

    with (
        patch.object(manager, "wait_for_device", return_value=True),
        patch.object(manager, "_with_device", return_value=None),
        patch.object(manager, "_force_kill_processes") as kill_processes,
    ):
        manager.reboot("edl")

    kill_processes.assert_not_called()


def test_adb_reboot_non_edl_does_not_kill_edl_related_processes():
    manager = AdbManager(skip_adb=False)

    with (
        patch.object(manager, "wait_for_device", return_value=True),
        patch.object(manager, "_with_device", return_value=None),
        patch.object(manager, "_force_kill_processes") as kill_processes,
    ):
        manager.reboot("bootloader")

    kill_processes.assert_not_called()


def test_edl_flash_rawprogram_sends_pre_erase_and_inline_reset(tmp_path):
    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    patch_xml = tmp_path / "patch0.xml"
    fh_loader = tmp_path / "fh_loader.exe"
    qsahara = tmp_path / "QSaharaServer.exe"

    for path in (loader_path, patch_xml, fh_loader, qsahara):
        path.write_text("x", encoding="utf-8")

    raw_xml.write_text(
        """<?xml version="1.0"?>
<data>
  <program
    label="metadata"
    physical_partition_number="0"
    partofsingleimage="false"
    filename="metadata_1.img"
    start_sector="100"
    num_partition_sectors="2"
    readbackverify="false"
    SECTOR_SIZE_IN_BYTES="4096"
  />
  <program
    label="frp"
    physical_partition_number="0"
    partofsingleimage="false"
    filename=""
    sparse="false"
    start_sector="200"
    num_partition_sectors="128"
    start_byte_hex="0x16108000"
    SECTOR_SIZE_IN_BYTES="4096"
  />
  <program
    label="userdata"
    physical_partition_number="6"
    partofsingleimage="false"
    filename="userdata_1.img"
    start_sector="4096"
    num_partition_sectors="8192"
    readbackverify="false"
    SECTOR_SIZE_IN_BYTES="4096"
  />
  <program
    label="super"
    physical_partition_number="0"
    start_sector="9999"
    num_partition_sectors="32"
    filename="super.img"
    SECTOR_SIZE_IN_BYTES="4096"
  />
</data>
""",
        encoding="utf-8",
    )

    with (
        patch("ltbox.device.const.EDL_EXE", fh_loader),
        patch("ltbox.device.const.QSAHARASERVER_EXE", qsahara),
        patch.object(manager, "load_programmer_safe"),
        patch("ltbox.device.utils.run_command") as mock_run_command,
    ):
        manager.flash_rawprogram(
            "COM1",
            loader_path,
            "UFS",
            [raw_xml],
            [patch_xml],
            pre_erase=True,
            reset_after=True,
        )

    erase_cmd = mock_run_command.call_args_list[0].args[0]
    flash_cmd = mock_run_command.call_args_list[1].args[0]
    erase_xml = tmp_path / "FHLoaderErase.xml"
    erase_root = ET.parse(erase_xml).getroot()
    erase_entries = erase_root.findall("erase")

    assert "--sendxml=FHLoaderErase.xml" in erase_cmd
    assert "--reset" in flash_cmd
    assert erase_xml.exists()
    assert [erase.get("label") for erase in erase_entries] == [
        "metadata",
        "frp",
        "userdata",
    ]
    assert all(erase.get("filename") is None for erase in erase_entries)
    assert erase_entries[0].get("partofsingleimage") == "false"
    assert erase_entries[1].get("start_byte_hex") == "0x16108000"
    assert erase_entries[2].get("physical_partition_number") == "6"
    assert erase_entries[2].get("start_sector") == "4096"
    assert erase_entries[2].get("num_partition_sectors") == "8192"
    assert erase_entries[2].get("SECTOR_SIZE_IN_BYTES") == "4096"


def test_edl_flash_rawprogram_deduplicates_erase_spans_across_xmls(tmp_path):
    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    raw_xml_dup = tmp_path / "rawprogram2.xml"
    patch_xml = tmp_path / "patch0.xml"
    fh_loader = tmp_path / "fh_loader.exe"
    qsahara = tmp_path / "QSaharaServer.exe"

    for path in (loader_path, patch_xml, fh_loader, qsahara):
        path.write_text("x", encoding="utf-8")

    raw_xml.write_text(
        """<?xml version="1.0"?>
<data>
  <program
    label="userdata"
    physical_partition_number="0"
    filename="userdata_1.img"
    start_sector="1024"
    num_partition_sectors="2048"
    SECTOR_SIZE_IN_BYTES="4096"
  />
</data>
""",
        encoding="utf-8",
    )
    raw_xml_dup.write_text(
        """<?xml version="1.0"?>
<data>
  <program
    label="userdata"
    physical_partition_number="0"
    filename="userdata_1.img"
    start_sector="1024"
    num_partition_sectors="2048"
    SECTOR_SIZE_IN_BYTES="4096"
  />
</data>
""",
        encoding="utf-8",
    )

    with (
        patch("ltbox.device.const.EDL_EXE", fh_loader),
        patch("ltbox.device.const.QSAHARASERVER_EXE", qsahara),
        patch.object(manager, "load_programmer_safe"),
        patch("ltbox.device.utils.run_command"),
    ):
        manager.flash_rawprogram(
            "COM1",
            loader_path,
            "UFS",
            [raw_xml, raw_xml_dup],
            [patch_xml],
            pre_erase=True,
            reset_after=False,
        )

    erase_entries = ET.parse(tmp_path / "FHLoaderErase.xml").getroot().findall("erase")

    assert len(erase_entries) == 1
    assert erase_entries[0].get("filename") is None


def test_edl_flash_rawprogram_skips_pre_erase_and_inline_reset_when_disabled(
    tmp_path,
):
    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    patch_xml = tmp_path / "patch0.xml"
    fh_loader = tmp_path / "fh_loader.exe"
    qsahara = tmp_path / "QSaharaServer.exe"

    for path in (loader_path, raw_xml, patch_xml, fh_loader, qsahara):
        path.write_text("x", encoding="utf-8")

    with (
        patch("ltbox.device.const.EDL_EXE", fh_loader),
        patch("ltbox.device.const.QSAHARASERVER_EXE", qsahara),
        patch.object(manager, "load_programmer_safe"),
        patch("ltbox.device.utils.run_command") as mock_run_command,
    ):
        manager.flash_rawprogram(
            "COM1",
            loader_path,
            "UFS",
            [raw_xml],
            [patch_xml],
            pre_erase=False,
            reset_after=False,
        )

    flash_cmd = mock_run_command.call_args_list[0].args[0]

    assert mock_run_command.call_count == 1
    assert "--sendxml=FHLoaderErase.xml" not in flash_cmd
    assert "--reset" not in flash_cmd


def test_edl_flash_rawprogram_requires_erase_spans_for_pre_erase(tmp_path):
    from ltbox.device import DeviceCommandError

    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    patch_xml = tmp_path / "patch0.xml"
    fh_loader = tmp_path / "fh_loader.exe"
    qsahara = tmp_path / "QSaharaServer.exe"

    for path in (loader_path, patch_xml, fh_loader, qsahara):
        path.write_text("x", encoding="utf-8")

    raw_xml.write_text(
        """<?xml version="1.0"?>
<data>
  <program
    label="super"
    physical_partition_number="0"
    start_sector="1"
    num_partition_sectors="2"
  />
</data>
""",
        encoding="utf-8",
    )

    with (
        patch("ltbox.device.const.EDL_EXE", fh_loader),
        patch("ltbox.device.const.QSAHARASERVER_EXE", qsahara),
        patch.object(manager, "load_programmer_safe"),
    ):
        with pytest.raises(DeviceCommandError, match="erase spans"):
            manager.flash_rawprogram(
                "COM1",
                loader_path,
                "UFS",
                [raw_xml],
                [patch_xml],
                pre_erase=True,
                reset_after=False,
            )
