import sys

from .menus.data import (
    MenuItem,
    MenuSpec,
    get_advanced_menu_data,
    get_main_menu_data,
    get_reboot_menu_data,
    get_root_apatch_variants_menu_data,
    get_root_ksu_modes_menu_data,
    get_root_menu_data,
    get_root_variants_menu_data,
    get_settings_menu_data,
)
from .menus import data as _module

__all__ = [
    "MenuItem",
    "MenuSpec",
    "get_advanced_menu_data",
    "get_main_menu_data",
    "get_reboot_menu_data",
    "get_root_apatch_variants_menu_data",
    "get_root_ksu_modes_menu_data",
    "get_root_menu_data",
    "get_root_variants_menu_data",
    "get_settings_menu_data",
]

sys.modules[__name__] = _module
