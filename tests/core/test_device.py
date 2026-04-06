from unittest.mock import MagicMock, patch

import pytest
from ltbox.device import AdbManager, EdlManager, FastbootManager


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
        patch("ltbox.utils.time.sleep", return_value=None),
    ):
        model = manager.get_model()
        assert model == "Lenovo TB-Test"


def test_fastboot_slot_detection_failure():
    import subprocess

    from ltbox.device import DeviceCommandError, FastbootManager

    manager = FastbootManager()

    with patch(
        "ltbox.device_fastboot.DeviceCommandRunner.run",
        side_effect=subprocess.CalledProcessError(1, "cmd"),
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


def test_edl_flash_rawprogram_sends_pre_erase_and_reset(tmp_path):
    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    patch_xml = tmp_path / "patch0.xml"
    qdlrs = tmp_path / "qdl-rs.exe"

    for path in (loader_path, raw_xml, patch_xml, qdlrs):
        path.write_text("x", encoding="utf-8")

    with (
        patch("ltbox.device_edl.const.QDLRS_EXE", qdlrs),
        patch.object(manager, "load_programmer_safe"),
        patch.object(manager, "_run_command") as mock_run,
        patch.object(manager, "reset") as mock_reset,
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

    # 3 erase calls (frp, metadata, userdata sorted) + 1 flasher call
    assert mock_run.call_count == 4

    erase_cmds = [mock_run.call_args_list[i].args[0] for i in range(3)]
    for cmd in erase_cmds:
        assert "erase" in cmd

    erase_labels = [cmd[-1] for cmd in erase_cmds]
    assert erase_labels == ["frp", "metadata", "userdata"]

    flash_cmd = mock_run.call_args_list[3].args[0]
    assert "flasher" in flash_cmd
    assert "-p" in flash_cmd
    assert "-x" in flash_cmd

    mock_reset.assert_called_once_with("COM1", mode="system")


def test_edl_flash_rawprogram_skips_erase_and_reset_when_disabled(tmp_path):
    manager = EdlManager()
    loader_path = tmp_path / "xbl_s_devprg_ns.melf"
    raw_xml = tmp_path / "rawprogram1.xml"
    patch_xml = tmp_path / "patch0.xml"
    qdlrs = tmp_path / "qdl-rs.exe"

    for path in (loader_path, raw_xml, patch_xml, qdlrs):
        path.write_text("x", encoding="utf-8")

    with (
        patch("ltbox.device_edl.const.QDLRS_EXE", qdlrs),
        patch.object(manager, "load_programmer_safe"),
        patch.object(manager, "_run_command") as mock_run,
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

    # Only the flasher command
    assert mock_run.call_count == 1
    flash_cmd = mock_run.call_args_list[0].args[0]
    assert "flasher" in flash_cmd
    assert "erase" not in flash_cmd


def test_edl_write_partition_leaves_success_logging_to_caller(tmp_path):
    manager = EdlManager()
    image_path = tmp_path / "init_boot.img"
    qdlrs = tmp_path / "qdl-rs.exe"
    image_path.write_text("patched", encoding="utf-8")
    qdlrs.write_text("x", encoding="utf-8")

    with (
        patch("ltbox.device_edl.const.QDLRS_EXE", qdlrs),
        patch("ltbox.device_edl.const.CONF") as mock_conf,
        patch.object(manager, "_run_command"),
        patch("ltbox.device_edl.ui") as mock_ui,
    ):
        mock_conf.edl_loader_file = tmp_path / "loader.melf"
        manager.write_partition(
            port="COM5",
            image_path=image_path,
            lun="4",
            start_sector="205962",
        )

    mock_ui.info.assert_not_called()


def test_fastboot_wait_for_device_uses_transient_status():
    manager = FastbootManager()
    status_cm = MagicMock()
    status_cm.__enter__.return_value = None
    status_cm.__exit__.return_value = False
    strings = {
        "device_wait_mode_title": "WAIT {mode}",
        "device_wait_fastboot_loop": "Waiting for fastboot...",
        "device_fastboot_connected": "[+] Fastboot connected.",
        "device_wait_fastboot_cancel": "[!] Cancelled.",
    }

    with (
        patch.object(manager, "_usb_port_hint"),
        patch.object(manager, "check_device", side_effect=[False, True]),
        patch("ltbox.device_fastboot.get_string", side_effect=strings.__getitem__),
        patch("ltbox.device_fastboot.ui") as mock_ui,
        patch("ltbox.device_fastboot.utils.wait_for_condition") as mock_wait,
    ):
        mock_ui.status.return_value = status_cm
        mock_wait.side_effect = (
            lambda predicate, interval=1.0, timeout=None, on_loop=None: predicate()
        )

        assert manager.wait_for_device() is True

    mock_ui.status.assert_called_once_with(strings["device_wait_fastboot_loop"])
    assert mock_wait.call_args.kwargs.get("on_loop") is None


def test_edl_reset_to_edl_calls_reset_with_edl_mode(tmp_path):
    manager = EdlManager()
    qdlrs = tmp_path / "qdl-rs.exe"
    qdlrs.write_text("x", encoding="utf-8")

    with (
        patch("ltbox.device_edl.const.QDLRS_EXE", qdlrs),
        patch("ltbox.device_edl.const.CONF") as mock_conf,
        patch.object(manager, "_run_command") as mock_run,
    ):
        mock_conf.edl_loader_file = tmp_path / "loader.melf"
        manager.reset_to_edl("COM3")

    cmd = mock_run.call_args.args[0]
    assert cmd[-2:] == ["reset", "edl"]


def test_edl_base_cmd_uses_qdlrs_serial_backend(tmp_path):
    manager = EdlManager()
    loader = tmp_path / "loader.melf"

    cmd = manager._base_cmd("COM12", loader)
    assert "--backend" in cmd
    assert "serial" in cmd
    assert "-d" in cmd
    assert "COM12" in cmd
    assert "-s" in cmd
    assert "ufs" in cmd
    assert "--reset-mode" in cmd
    assert "off" in cmd
