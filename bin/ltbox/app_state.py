from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class AppState:
    skip_adb: bool = False
    modify_region_code: bool = True
    target_region: str = "PRC"
    preset_code: str = "1"
    modify_rollback_index: str = "ON"
    language: Optional[str] = None
