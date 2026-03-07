from unittest.mock import MagicMock


def make_device_mock(
    *,
    skip_adb: bool = False,
    active_slot: str = "_a",
    model: str = "TestModel",
) -> MagicMock:
    dev = MagicMock()
    dev.skip_adb = skip_adb
    dev.detect_active_slot.return_value = active_slot
    dev.adb = MagicMock()
    dev.fastboot = MagicMock()
    dev.adb.get_model.return_value = model
    return dev
