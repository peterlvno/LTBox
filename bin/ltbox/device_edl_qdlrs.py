import subprocess
from pathlib import Path
from typing import List, Optional

from . import constants as const
from .device_edl import EdlManager
from .device_support import (
    DeviceCommandRunner,
    format_serial_port_bare,
    prevent_sleep_during_flash,
)
from .errors import DeviceCommandError
from .i18n import get_string
from .ui import ui

_ERASE_LABELS = frozenset({"userdata", "metadata", "frp"})


class QdlrsEdlManager(EdlManager):
    """EdlManager implementation using qdl-rs instead of fh_loader/QSaharaServer."""

    def __init__(
        self,
        usb_port_hint=None,
        command_runner: Optional[DeviceCommandRunner] = None,
    ):
        super().__init__(usb_port_hint=usb_port_hint, command_runner=command_runner)

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

        if partition_name:
            cmd = self._base_cmd(port, loader_path) + [
                "dump-part",
                "-o",
                str(dest_dir),
                partition_name,
            ]
        else:
            cmd = self._base_cmd(port, loader_path) + [
                "-L",
                str(lun),
                "dump-part",
                "-o",
                str(dest_dir),
                dest_file.stem,
            ]

        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd, cwd=dest_dir)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise DeviceCommandError(get_string("device_err_fh_exec").format(e=e), e)

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

        if partition_name:
            cmd = self._base_cmd(port, loader_path) + [
                "write",
                partition_name,
                str(image_file),
            ]
        else:
            cmd = self._base_cmd(port, loader_path) + [
                "-L",
                str(lun),
                "write",
                image_file.stem,
                str(image_file),
            ]

        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
            raise DeviceCommandError(get_string("device_err_flash_exec").format(e=e), e)

    def reset(self, port: str, *, mode: str = "system") -> None:
        if not const.QDLRS_EXE.exists():
            raise FileNotFoundError(
                get_string("device_err_qdlrs_missing").format(path=const.QDLRS_EXE)
            )

        loader_path = const.CONF.edl_loader_file
        cmd = self._base_cmd(port, loader_path) + ["reset", mode]
        try:
            with prevent_sleep_during_flash():
                self._run_command(cmd, timeout=30.0)
        except (subprocess.CalledProcessError, FileNotFoundError) as e:
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
                        self._run_command(
                            self._base_cmd(port, loader_path) + ["erase", label]
                        )

                ui.info(get_string("device_step2_flash"))
                cmd = self._base_cmd(port, loader_path) + ["flasher"]
                for xml_path in raw_xmls:
                    cmd.extend(["-p", str(xml_path)])
                for xml_path in patch_xmls:
                    cmd.extend(["-x", str(xml_path)])
                self._run_command(cmd)

                if reset_after:
                    self.reset(port, mode="system")
        except (subprocess.CalledProcessError, OSError, RuntimeError) as e:
            raise DeviceCommandError(
                get_string("device_err_rawprogram_fail").format(e=e),
                e,
            )
