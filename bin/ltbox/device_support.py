import contextlib
import ctypes
import os
import subprocess
from pathlib import Path
from typing import Any, Callable, Iterator, Optional

import serial
import serial.tools.list_ports

from .i18n import get_string
from .process_runner import CommandResult, CommandRunner, RunOptions
from .ui import ui
from .utils import _get_tool_env

QUALCOMM_EDL_HWID = "VID:PID=05C6:9008"


def create_no_window_creationflags() -> int:
    return getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0


def default_usb_port_hint() -> Callable[[], None]:
    return lambda: ui.warn(get_string("device_usb_port_hint"))


def format_serial_port(port: str) -> str:
    return f"\\\\.\\{port}"


def format_serial_port_bare(port: str) -> str:
    """Return bare COM port name (e.g. 'COM12'), stripping '\\\\.\\' prefix if present."""
    prefix = "\\\\.\\"
    if port.startswith(prefix):
        return port[len(prefix) :]
    return port


def is_qualcomm_edl_port(port: Any) -> bool:
    description = port.description or ""
    hwid = (port.hwid or "").upper()
    return (
        "Qualcomm" in description and "9008" in description
    ) or QUALCOMM_EDL_HWID in hwid


def find_edl_port() -> Optional[str]:
    try:
        for port in serial.tools.list_ports.comports():
            if is_qualcomm_edl_port(port):
                return port.device
    except serial.SerialException:
        return None
    return None


@contextlib.contextmanager
def prevent_sleep_during_flash() -> Iterator[None]:
    if os.name != "nt":
        yield
        return

    es_continuous = 0x80000000
    es_system_required = 0x00000001
    es_awaymode_required = 0x00000040

    try:
        ctypes.windll.kernel32.SetThreadExecutionState(  # type: ignore[attr-defined]
            es_continuous | es_system_required | es_awaymode_required
        )
    except (OSError, AttributeError):
        yield
        return

    try:
        yield
    finally:
        try:
            ctypes.windll.kernel32.SetThreadExecutionState(es_continuous)  # type: ignore[attr-defined]
        except (OSError, AttributeError):
            pass


class DeviceCommandRunner:
    def __init__(self, runner: Optional[CommandRunner] = None):
        self._runner = runner or CommandRunner()

    def run(
        self,
        command: list[str],
        *,
        capture: bool = False,
        check: bool = True,
        cwd: Optional[Path] = None,
        timeout: Optional[float] = None,
    ) -> CommandResult:
        return self._runner.run(
            command,
            options=RunOptions(
                capture=capture,
                stream=not capture,
                check=check,
                cwd=cwd,
                env=_get_tool_env(),
                timeout=timeout,
                creationflags=create_no_window_creationflags(),
            ),
        )


class BaseDeviceManager:
    def __init__(
        self,
        usb_port_hint: Optional[Callable[[], None]] = None,
        command_runner: Optional[DeviceCommandRunner] = None,
    ):
        self._usb_port_hint = usb_port_hint or default_usb_port_hint()
        self._command_runner = command_runner or DeviceCommandRunner()

    def _force_kill_process(self, exe_name: str) -> None:
        try:
            subprocess.run(
                ["taskkill", "/F", "/IM", exe_name, "/T"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=create_no_window_creationflags(),
            )
        except (subprocess.CalledProcessError, OSError):
            pass

    def _force_kill_processes(self, exe_names: list[str]) -> None:
        for exe_name in exe_names:
            self._force_kill_process(exe_name)
