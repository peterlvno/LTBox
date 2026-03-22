from typing import Optional

from .. import device, utils
from ..errors import DeviceCommandError, DeviceConnectionError, ToolError
from ..i18n import get_string


def detect_slot(dev: device.DeviceController) -> Optional[str]:
    try:
        return dev.detect_active_slot()
    except (DeviceCommandError, DeviceConnectionError) as e:
        raise ToolError(get_string("act_warn_slot_fail")) from e


def get_slot_suffix(dev: device.DeviceController) -> str:
    return detect_slot(dev) or ""


def _safe_shell(
    dev: device.DeviceController, cmd: str, error_msg: Optional[str] = None
) -> str:
    try:
        return dev.adb.shell(cmd)
    except (DeviceCommandError, DeviceConnectionError) as e:
        if error_msg:
            utils.ui.echo(f"{error_msg}: {e}", err=True)
        return ""


def disable_ota(dev: device.DeviceController) -> None:

    utils.ui.echo(get_string("act_start_ota"))

    dev.adb.wait_for_device()

    utils.ui.echo(get_string("act_ota_settings_put"))
    _safe_shell(
        dev,
        "settings put global ota_disable_automatic_update 1",
        get_string("act_ota_warn_failed_settings"),
    )
    _safe_shell(
        dev,
        "settings put secure lenovo_ota_new_version_found 0",
        get_string("act_ota_warn_failed_settings"),
    )

    packages = ["com.lenovo.ota", "com.tblenovo.lenovowhatsnew", "com.lenovo.tbengine"]
    _disable_ota_packages(dev, packages)

    utils.ui.echo(get_string("act_ota_factory_reset_notice"))
    utils.ui.echo(get_string("act_ota_finished"))


def _disable_ota_packages(
    dev: device.DeviceController,
    packages: list[str],
) -> None:
    for pkg in packages:
        _clear_package_data(dev, pkg)
        utils.ui.echo(get_string("act_ota_uninstalling").format(pkg=pkg))
        _uninstall_package(dev, pkg)


def _clear_package_data(dev: device.DeviceController, package: str) -> None:
    _safe_shell(dev, f"pm clear {package}")


def _uninstall_package(dev: device.DeviceController, package: str) -> None:
    output = _safe_shell(dev, f"pm uninstall -k --user 0 {package}")
    if "Success" in output:
        utils.ui.echo(get_string("act_ota_uninstall_success").format(pkg=package))
    else:
        utils.ui.echo(get_string("act_ota_uninstall_fail").format(pkg=package))
