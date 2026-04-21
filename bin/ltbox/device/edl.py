import subprocess
import time
from pathlib import Path
from typing import Callable, List, Optional

import serial

from .. import constants as const
from ..errors import DeviceCommandError
from ..i18n import get_string
from ..ui import ui
from .support import (
    BaseDeviceManager,
    DeviceCommandRunner,
    find_edl_port,
    format_serial_port_bare,
    prevent_sleep_during_flash,
)

_ERASE_LABELS = frozenset({"userdata", "metadata", "frp"})


class EdlManager(BaseDeviceManager):
    """EDL device manager using qdl-rs."""

    def __init__(
        self,
        usb_port_hint: Optional[Callable[[], None]] = None,
        command_runner: Optional[DeviceCommandRunner] = None,
    ):
        super().__init__(usb_port_hint=usb_port_hint, command_runner=command_runner)

    def check_device(self, silent: bool = False) -> Optional[str]:
        if not silent:
            ui.info(get_string("device_check_edl"))

        try:
            port_name = find_edl_port()
            if port_name:
                if not silent:
                    ui.info(get_string("device_found_edl").format(device=port_name))
                return port_name

            if not silent:
                ui.warn(get_string("device_no_edl"))
                ui.warn(get_string("device_connect_edl"))
            return None
        except serial.SerialException as e:
            if not silent:
                ui.error(get_string("device_err_check_edl").format(e=e))
            return None

    def wait_for_device(self) -> str:
        self._usb_port_hint()
        port_name = self.check_device(silent=True)
        if port_name:
            ui.info(get_string("device_edl_connected").format(port=port_name))
            return port_name

        try:
            from .. import utils

            with ui.status(get_string("device_wait_edl_loop")):
                port_name = utils.wait_for_condition(
                    lambda: self.check_device(silent=True),
                    interval=2.0,
                )
            ui.info(get_string("device_edl_connected").format(port=port_name))
            return port_name
        except KeyboardInterrupt:
            ui.warn(get_string("device_wait_edl_cancel"))
            raise

    def _run_command(
        self,
        command: list[str],
        *,
        cwd: Optional[Path] = None,
        check: bool = True,
        timeout: Optional[float] = None,
        capture: bool = False,
    ):
        return self._command_runner.run(
            command,
            capture=capture,
            check=check,
            cwd=cwd,
            timeout=timeout,
        )

    def _ensure_edl_port(self, port: str, timeout: float = 45.0) -> str:
        """Wait for a stable EDL port to appear after a qdl-rs reset.

        qdl-rs resets the device to EDL after each operation. The old COM
        port may linger briefly before USB disconnect, and the post-reset
        port may flap while Windows re-enumerates. Two phases:

        1. If a port is visible right after the grace period, watch up to
           5s for it to disconnect. If it stays, it is not a stale remnant
           and we trust it.
        2. Otherwise, poll until the same port is observed on two
           consecutive checks (1s apart) — that is the new stable port.

        Raises DeviceCommandError on timeout so callers never spawn
        qdl-rs against a vanished COM device.
        """
        deadline = time.monotonic() + timeout
        # Grace period for the reset cycle to begin tearing down the port.
        time.sleep(2.0)

        initial = find_edl_port()
        if initial is not None:
            disconnect_deadline = min(deadline, time.monotonic() + 5.0)
            saw_disconnect = False
            while time.monotonic() < disconnect_deadline:
                time.sleep(1.0)
                if find_edl_port() is None:
                    saw_disconnect = True
                    break
            if not saw_disconnect:
                return initial

        last: Optional[str] = None
        while time.monotonic() < deadline:
            time.sleep(1.0)
            current = find_edl_port()
            if current is not None and current == last:
                return current
            last = current

        raise DeviceCommandError(
            get_string("device_err_edl_port_timeout").format(
                port=port, timeout=int(timeout)
            )
        )

    def _base_cmd(self, port: str, loader_path: Path) -> list[str]:
        return [
            str(const.QDLRS_EXE),
            "--backend",
            "serial",
            "-d",
            format_serial_port_bare(port),
            "-l",
            str(loader_path),
            "-s",
            "ufs",
        ]

    def load_programmer(self, port: str, loader_path: Path) -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        cmd = self._base_cmd(port, loader_path) + ["nop"]
        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd, timeout=30.0)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            msg = get_string("device_fatal_programmer")
            msg += f"\n{get_string('device_fatal_causes')}"
            msg += f"\n{get_string('device_cause_1')}"
            msg += f"\n{get_string('device_cause_2')}"
            msg += f"\n{get_string('device_cause_3')}"
            msg += f"\nError: {e}"
            raise DeviceCommandError(msg, e)

    def load_programmer_safe(self, port: str, loader_path: Path) -> None:
        ui.info(get_string("device_upload_programmer").format(port=port))
        self.load_programmer(port, loader_path)

    def read_partition(
        self,
        port: str,
        output_filename: str,
        lun: str,
        start_sector: str,
        num_sectors: str,
        memory_name: str = "UFS",
        *,
        partition_name: Optional[str] = None,
    ) -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        dest_file = Path(output_filename).resolve()
        dest_dir = dest_file.parent
        dest_dir.mkdir(parents=True, exist_ok=True)

        loader_path = const.CONF.edl_loader_file
        port = self._ensure_edl_port(port)

        name = partition_name or dest_file.stem
        cmd = self._base_cmd(port, loader_path) + [
            "-L",
            str(lun),
            "dump-part",
            "-o",
            str(dest_dir),
            name,
        ]

        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd, cwd=dest_dir)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise DeviceCommandError(get_string("device_err_edl_read").format(e=e), e)

        # qdl-rs saves with the partition name; rename to the expected filename
        if not dest_file.exists():
            for candidate in dest_dir.glob(f"{name}.*"):
                if candidate.is_file() and candidate != dest_file:
                    candidate.rename(dest_file)
                    break
            else:
                # No extension variant — check bare name
                bare = dest_dir / name
                if bare.exists() and bare.is_file() and bare != dest_file:
                    bare.rename(dest_file)

    def write_partition(
        self,
        port: str,
        image_path: Path,
        lun: str,
        start_sector: str,
        memory_name: str = "UFS",
        *,
        partition_name: Optional[str] = None,
    ) -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        image_file = Path(image_path).resolve()
        loader_path = const.CONF.edl_loader_file
        port = self._ensure_edl_port(port)

        name = partition_name or image_file.stem
        cmd = self._base_cmd(port, loader_path) + [
            "-L",
            str(lun),
            "write",
            name,
            str(image_file),
        ]

        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise DeviceCommandError(get_string("device_err_edl_write").format(e=e), e)

    def reset(self, port: str, *, mode: str = "system") -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        loader_path = const.CONF.edl_loader_file
        port = self._ensure_edl_port(port)
        cmd = self._base_cmd(port, loader_path) + [
            "--reset-mode",
            mode,
            "reset",
            mode,
        ]
        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd, timeout=30.0, check=False)
        except FileNotFoundError as e:
            raise DeviceCommandError(get_string("device_err_reset_fail").format(e=e), e)

    def reset_to_edl(self, port: str) -> None:
        """Reset device back to EDL mode (stays in Sahara, no system reboot)."""
        self.reset(port, mode="edl")

    def flash_rawprogram(
        self,
        port: str,
        loader_path: Path,
        memory_type: str,
        raw_xmls: List[Path],
        patch_xmls: List[Path],
        *,
        pre_erase: bool = False,
        reset_after: bool = False,
    ) -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        ui.info(get_string("device_step1_load"))
        self.load_programmer_safe(port, loader_path)

        try:
            with prevent_sleep_during_flash():
                if pre_erase:
                    for label in sorted(_ERASE_LABELS):
                        port = self._ensure_edl_port(port)
                        self._run_command(
                            self._base_cmd(port, loader_path) + ["erase", label]
                        )

                ui.info(get_string("device_step2_flash"))
                port = self._ensure_edl_port(port)

                cmd = self._base_cmd(port, loader_path)
                if reset_after:
                    cmd.extend(["--reset-mode", "system"])
                cmd.append("flasher")
                for xml_path in raw_xmls:
                    cmd.extend(["-p", str(xml_path)])
                for xml_path in patch_xmls:
                    cmd.extend(["-x", str(xml_path)])
                self._run_command(cmd)
        except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
            raise DeviceCommandError(
                get_string("device_err_rawprogram_fail").format(e=e),
                e,
            )
