import sys

from .menus.router import (
    DeviceControllerFactoryProtocol,
    LoopAction,
    MainMenuAction,
    RouteResult,
    main_loop,
    prompt_for_language,
)
from .menus import router as _module

__all__ = [
    "DeviceControllerFactoryProtocol",
    "LoopAction",
    "MainMenuAction",
    "RouteResult",
    "main_loop",
    "prompt_for_language",
]

sys.modules[__name__] = _module
