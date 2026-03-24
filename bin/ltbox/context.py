from dataclasses import dataclass, field
from typing import Callable, Optional

from .device import DeviceController


@dataclass
class TaskContext:
    dev: DeviceController
    wipe: int = 0
    skip_rollback: bool = False
    modify_rollback_index: str = "ON"
    modify_region_code: bool = True
    target_region: str = "PRC"
    device_model: Optional[str] = None
    active_slot_suffix: Optional[str] = None
    skip_dp_workflow: bool = False
    boot_target: Optional[str] = None
    vbmeta_target: Optional[str] = None
    backup_dir_name: Optional[str] = None
    arb_patched: bool = False

    on_log: Callable[[str], None] = field(default_factory=lambda: lambda s: print(s))
