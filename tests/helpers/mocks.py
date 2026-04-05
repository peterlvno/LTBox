from types import MappingProxyType
from typing import Dict, Optional
from unittest.mock import MagicMock

from ltbox.device import DeviceController
from ltbox.device_adb import AdbManager
from ltbox.device_fastboot import FastbootManager, FastbootVars


def make_device_mock(
    *,
    skip_adb: bool = False,
    active_slot: str = "_a",
    model: str = "TestModel",
    serialno: str = "QX947M3L",
    stored_rollback_indices: Optional[Dict[int, int]] = None,
) -> MagicMock:
    dev = MagicMock(spec=DeviceController)
    dev.skip_adb = skip_adb
    dev.detect_active_slot.return_value = active_slot
    dev.adb = MagicMock(spec=AdbManager)
    dev.fastboot = MagicMock(spec=FastbootManager)
    dev.adb.get_model.return_value = model
    dev.fastboot.get_all_vars.return_value = FastbootVars(
        model=model,
        slot_suffix=active_slot,
        serialno=serialno,
        stored_rollback_indices=MappingProxyType(stored_rollback_indices or {}),
    )
    return dev
