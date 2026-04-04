import re
from typing import Any, Callable, Optional

import adbutils
from adbutils import AdbError

from . import constants as const
from . import utils
from .device_support import BaseDeviceManager, DeviceCommandRunner
from .errors import DeviceCommandError, DeviceConnectionError
from .i18n import get_string
from .ui import ui


class AdbManager(BaseDeviceManager):
    def __init__(
        self,
        skip_adb: bool,
        usb_port_hint: Optional[Callable[[], None]] = None,
        command_runner: Optional[DeviceCommandRunner] = None,
    ):
        super().__init__(usb_port_hint=usb_port_hint, command_runner=command_runner)
        self.skip_adb = skip_adb
        self.connected_once = False
        if const.ADB_EXE.exists():
            adbutils.adb_path = str(const.ADB_EXE)

    def _get_device(self) -> Optional[adbutils.AdbDevice]:
        try:
            return adbutils.adb.device()
        except AdbError:
            return None

    def _with_device(
        self,
        action: Callable[[adbutils.AdbDevice], Any],
    ) -> Optional[Any]:
        if not self.wait_for_device():
            return None
        try:
            device = self._get_device()
        except AdbError as e:
            raise DeviceConnectionError(
                get_string("device_err_wait_adb").format(e=e),
                e,
            )
        if not device:
            raise DeviceConnectionError(
                get_string("device_err_wait_adb").format(e="No device found"),
            )
        return action(device)

    def wait_for_device(self) -> bool:
        if self.skip_adb:
            ui.warn(get_string("device_skip_adb"))
            return False

        self._usb_port_hint()
        if not self.connected_once:
            ui.box_output(
                [
                    get_string("device_wait_mode_title").format(mode="ADB"),
                    get_string("device_enable_usb_debug"),
                    get_string("device_usb_prompt_appear"),
                    get_string("device_check_always_allow"),
                    get_string("device_wait_cancel_hint"),
                ]
            )
        else:
            print(get_string("device_wait_adb_loop") + "...", end="\r")

        def _check_adb() -> bool:
            try:
                return any(
                    device.get_state() == "device"
                    for device in adbutils.adb.device_list()
                )
            except (AdbError, OSError):
                return False

        try:
            utils.wait_for_condition(_check_adb, interval=1.0)
            if not self.connected_once:
                ui.info(get_string("device_adb_connected"))
            self.connected_once = True
            print(" " * 40, end="\r")
            return True
        except KeyboardInterrupt:
            ui.warn("\n" + get_string("device_wait_cancelled"))
            self.skip_adb = True
            ui.warn(get_string("act_skip_adb_active"))
            return False

    def get_model(self) -> Optional[str]:
        try:
            return self._with_device(lambda device: device.prop.model)
        except (DeviceConnectionError, AdbError) as e:
            raise DeviceConnectionError(
                get_string("device_err_get_model").format(e=e),
                e,
            )

    def get_slot_suffix(self) -> Optional[str]:
        try:

            def _read_suffix(device: adbutils.AdbDevice) -> Optional[str]:
                suffix = device.getprop("ro.boot.slot_suffix")
                return suffix if suffix in ["_a", "_b"] else None

            return self._with_device(_read_suffix)
        except (DeviceConnectionError, AdbError) as e:
            raise DeviceConnectionError(
                get_string("device_err_get_slot").format(e=e),
                e,
            )

    def get_kernel_version(self) -> str:
        try:
            if not self.wait_for_device():
                raise DeviceConnectionError(
                    get_string("dl_lkm_kver_fail").format(ver="SKIP_ADB")
                )

            print(get_string("dl_lkm_get_kver"))

            def _read_version(device: adbutils.AdbDevice) -> str:
                version_string = device.shell("cat /proc/version")
                match = re.search(r"Linux version (\d+\.\d+)", version_string)
                if not match:
                    raise DeviceCommandError(
                        get_string("dl_lkm_kver_fail").format(ver=version_string)
                    )
                return match.group(1)

            version = self._with_device(_read_version)
            if not version:
                raise DeviceConnectionError(
                    get_string("device_err_wait_adb").format(e="No device")
                )
            print(get_string("dl_lkm_kver_found").format(ver=version))
            return version
        except (DeviceConnectionError, DeviceCommandError, AdbError) as e:
            raise DeviceCommandError(
                get_string("dl_lkm_kver_fail").format(ver=str(e)),
                e,
            )

    def reboot(self, target: str) -> None:
        try:
            if not self.wait_for_device():
                if target == "edl":
                    ui.warn(get_string("device_manual_edl_req"))
                return

            def _reboot(device: adbutils.AdbDevice) -> None:
                if target == "edl":
                    try:
                        with device.open_transport() as connection:
                            connection.send_command("reboot:edl")
                            connection.check_okay()
                    except (AdbError, OSError):
                        device.shell("reboot edl")
                elif target == "bootloader":
                    try:
                        with device.open_transport() as connection:
                            connection.send_command("reboot:bootloader")
                            connection.check_okay()
                    except (AdbError, OSError):
                        device.shell("reboot bootloader")
                else:
                    device.shell(f"reboot {target}")

            self._with_device(_reboot)
        except (DeviceConnectionError, DeviceCommandError, AdbError) as e:
            raise DeviceCommandError(get_string("device_err_reboot").format(e=e), e)

    def install(self, apk_path: str) -> None:
        self._with_device(lambda device: device.install(apk_path))

    def push(self, local: str, remote: str) -> None:
        self._with_device(lambda device: device.sync.push(local, remote))

    def pull(self, remote: str, local: str) -> None:
        self._with_device(lambda device: device.sync.pull(remote, local))

    def shell(self, cmd: str) -> str:
        return self._with_device(lambda device: device.shell(cmd)) or ""

    def force_kill_server(self) -> None:
        self._force_kill_process("adb.exe")
