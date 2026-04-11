import sys

from .menus.terminal import TerminalMenu, select_menu_action
from .menus import terminal as _module

__all__ = ["TerminalMenu", "select_menu_action"]

sys.modules[__name__] = _module
