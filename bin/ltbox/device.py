import contextlib
import time
from typing import Optional

from adbutils import AdbError

from . import constants as const
from . import utils
from .device_adb import AdbManager
from .device_edl import EdlManager
from .device_fastboot import FastbootManager
from .device_support import DeviceCommandRunner
from .errors import DeviceCommandError
from .i18n import get_string
from .ui import ui


class DeviceController:
    def __init__(
        self,
        skip_adb: bool = False,
        adb_manager: Optional[AdbManager] = None,
        fastboot_manager: Optional[FastbootManager] = None,
        edl_manager: Optional[EdlManager] = None,
    ):
        self._usb_port_hint_shown = False
        self._command_runner = DeviceCommandRunner()

        self.adb = (
            adb_manager
            if adb_manager is not None
            else AdbManager(
                skip_adb,
                self._maybe_warn_usb_port_hint,
                command_runner=self._command_runner,
            )
        )
        self.fastboot = (
            fastboot_manager
            if fastboot_manager is not None
            else FastbootManager(
                self._maybe_warn_usb_port_hint,
                command_runner=self._command_runner,
            )
        )
        self.edl = (
            edl_manager
            if edl_manager is not None
            else EdlManager(
                self._maybe_warn_usb_port_hint,
                command_runner=self._command_runner,
            )
        )

        self.adb._usb_port_hint = self._maybe_warn_usb_port_hint
        self.fastboot._usb_port_hint = self._maybe_warn_usb_port_hint
        self.edl._usb_port_hint = self._maybe_warn_usb_port_hint

    def reset_task_state(self) -> None:
        self._usb_port_hint_shown = False

    def _maybe_warn_usb_port_hint(self) -> None:
        if self._usb_port_hint_shown:
            return
        ui.warn(get_string("device_usb_port_hint"))
        self._usb_port_hint_shown = True

    @property
    def skip_adb(self) -> bool:
        return self.adb.skip_adb

    @skip_adb.setter
    def skip_adb(self, value: bool) -> None:
        self.adb.skip_adb = value

    def detect_active_slot(self) -> Optional[str]:
        self.ensure_fastboot_mode()
        return self.fastboot.get_slot_suffix()

    def ensure_fastboot_mode(self) -> None:
        if self.fastboot.check_device(silent=True):
            return

        if not self.skip_adb:
            try:
                self.adb.reboot("bootloader")
            except (DeviceCommandError, AdbError) as e:
                ui.warn(get_string("act_err_reboot_bl").format(e=e))

        self.fastboot.wait_for_device()

    def ensure_edl_mode(self) -> None:
        if self.edl.check_device(silent=True):
            ui.info(get_string("device_already_edl"))
            return

        in_fastboot = self.fastboot.check_device(silent=True)

        if in_fastboot:
            ui.info(get_string("device_edl_via_fastboot"))
            if self.fastboot.oem_edl():
                ui.info(get_string("device_wait_10s_edl"))
                time.sleep(10)
                return

            ui.warn(get_string("device_oem_edl_failed"))

            if not self.skip_adb:
                ui.info(get_string("device_edl_via_fastboot_fallback"))
                self.fastboot.continue_boot()
                time.sleep(10)
                self.adb.wait_for_device()
                ui.info(get_string("device_edl_setup_title"))
                self.adb.reboot("edl")
                ui.info(get_string("device_wait_10s_edl"))
                time.sleep(10)
            else:
                width = ui.get_term_width()
                ui.echo("\n" + "=" * width)
                ui.echo(get_string("act_manual_edl"))
                ui.echo("=" * width + "\n")
        elif not self.skip_adb:
            self.adb.wait_for_device()
            ui.info(get_string("device_edl_setup_title"))
            self.adb.reboot("edl")
            ui.info(get_string("device_wait_10s_edl"))
            time.sleep(10)
        else:
            width = ui.get_term_width()
            ui.echo("\n" + "=" * width)
            ui.echo(get_string("act_manual_edl"))
            ui.echo("=" * width + "\n")

    def setup_edl_connection(self) -> str:
        self.ensure_edl_mode()

        required_files = [const.EDL_LOADER_FILENAME]
        const.IMAGE_DIR.mkdir(exist_ok=True)

        if not const.EDL_LOADER_FILE.exists():
            prompt = get_string("device_loader_prompt").format(
                loader=const.EDL_LOADER_FILENAME,
                folder=const.IMAGE_DIR.name,
            )
            utils.wait_for_files(const.IMAGE_DIR, required_files, prompt)

        ui.info(
            get_string("device_loader_found").format(
                file=const.EDL_LOADER_FILE.name,
                dir=const.IMAGE_DIR.name,
            )
        )

        port = self.edl.wait_for_device()
        return port

    @contextlib.contextmanager
    def edl_session(
        self,
        load_programmer: bool = True,
        auto_reset: bool = True,
        reset_msg_key: str = "act_reboot_device",
        skip_msg_key: str = "act_skip_reboot",
        pre_sleep: int = 0,
        post_sleep: int = 0,
    ):
        port = self.setup_edl_connection()

        if load_programmer:
            try:
                self.edl.load_programmer_safe(port, const.EDL_LOADER_FILE)
            except (DeviceCommandError, FileNotFoundError) as e:
                ui.warn(get_string("act_warn_prog_load").format(e=e))

        try:
            yield port
        finally:
            if not auto_reset:
                if skip_msg_key:
                    ui.info(get_string(skip_msg_key))
            else:
                if pre_sleep > 0:
                    ui.info(get_string("act_wait_stability"))
                    time.sleep(pre_sleep)
                if reset_msg_key:
                    ui.info(get_string(reset_msg_key))
                try:
                    ui.info(get_string("device_resetting"))
                    self.edl.reset(port)
                    ui.info(get_string("act_reset_sent"))
                except (DeviceCommandError, OSError) as e:
                    ui.error(get_string("act_err_reset").format(e=e))
                if post_sleep > 0:
                    ui.info(get_string("act_wait_stability_long"))
                    time.sleep(post_sleep)
