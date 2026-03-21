import sys
from dataclasses import replace
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Protocol, Tuple, Union

from . import i18n, menu_data
from .app_state import AppState
from .i18n import get_string
from .menu import TerminalMenu, select_menu_action
from .utils import ui
from . import update_service
from .task_runner import run_task
from .registry import CommandRegistry


class LoopAction(str, Enum):
    BACK = "back"
    RETURN = "return"
    EXIT = "exit"


class MainMenuAction(str, Enum):
    SETTINGS = "menu_settings"
    ROOT = "menu_root"
    ADVANCED = "menu_advanced"
    PATCH_ALL = "patch_all"
    PATCH_ALL_WIPE = "patch_all_wipe"


class RouteResult(str, Enum):
    MAIN = "main"
    RETURN = "return"


ROOT_TYPE_MENU_SPEC: List[Optional[Tuple[str, str]]] = [
    ("1", "menu_root_type_ksu"),
    ("2", "menu_root_type_ksun"),
    None,
    ("3", "menu_root_type_sukisu"),
    ("4", "menu_root_type_resukisu"),
    None,
    ("5", "APatch"),
    ("6", "FolkPatch"),
    None,
    ("b", "menu_back"),
    ("x", "menu_main_exit"),
]

ROOT_TYPE_BREADCRUMB_LABELS: Dict[str, str] = {
    item[0]: item[1] for item in ROOT_TYPE_MENU_SPEC if item is not None
}


class DeviceControllerProtocol(Protocol):
    skip_adb: bool


class DeviceControllerFactoryProtocol(Protocol):
    def __call__(self, skip_adb: bool) -> DeviceControllerProtocol: ...


MenuReturn = Optional[Union[LoopAction, RouteResult]]


PRESET_1_KEY = "menu_settings_preset_1"
PRESET_2_KEY = "menu_settings_preset_2"
PRESET_3_KEY = "menu_settings_preset_3"


def _preset_label_from_code(preset_code: str) -> str:
    if preset_code == "1":
        return get_string(PRESET_1_KEY)
    if preset_code == "2":
        return get_string(PRESET_2_KEY)
    if preset_code == "3":
        return get_string(PRESET_3_KEY)
    return "-"


def _resolve_settings_preset_label(state: AppState) -> str:
    return _preset_label_from_code(state.preset_code)


def _loop_menu(
    menu_items_factory: Callable[[], List[Any]],
    title_key: str,
    breadcrumbs: Optional[str],
    action_handler: Callable[[str], MenuReturn],
) -> MenuReturn:
    while True:
        menu_items = menu_items_factory()
        action = select_menu_action(menu_items, title_key, breadcrumbs=breadcrumbs)

        if action in (LoopAction.BACK, LoopAction.RETURN, LoopAction.EXIT):
            return LoopAction(action)

        if action is not None:
            result = action_handler(action)
            if result in (LoopAction.BACK, LoopAction.RETURN, LoopAction.EXIT):
                return result


def advanced_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    target_region: str,
) -> MenuReturn:
    main_title = get_string("menu_main_title")

    def _handler(action: str) -> None:
        extras: Dict[str, Any] = (
            {"target_region": target_region} if action == "convert" else {}
        )
        run_task(action, dev, registry, extra_kwargs=extras)

    return _loop_menu(
        lambda: menu_data.get_advanced_menu_data(target_region),
        "menu_adv_title",
        main_title,
        _handler,
    )


def _root_action_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    gki: bool,
    root_type: str,
    breadcrumbs: str,
) -> MenuReturn:
    def _handler(action: str) -> None:
        extras: Dict[str, Any] = {"root_type": root_type}
        run_task(action, dev, registry, extra_kwargs=extras)

    res = _loop_menu(
        lambda: menu_data.get_root_menu_data(gki, root_type=root_type),
        "menu_root_title",
        breadcrumbs,
        _handler,
    )
    if res == LoopAction.RETURN:
        return RouteResult.MAIN
    return res


def _handle_ksu_mode(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    type_breadcrumbs: str,
) -> MenuReturn:
    mode_breadcrumbs = f"{type_breadcrumbs} > {get_string('menu_root_mode_title')}"

    def _handler(mode_action: str) -> MenuReturn:
        if mode_action == "lkm":
            return _root_action_menu(
                dev, registry, gki=False, root_type="ksu", breadcrumbs=mode_breadcrumbs
            )
        elif mode_action == "gki":
            return _root_action_menu(
                dev, registry, gki=True, root_type="ksu", breadcrumbs=mode_breadcrumbs
            )
        return None

    res = _loop_menu(
        menu_data.get_root_mode_menu_data,
        "menu_root_mode_title",
        type_breadcrumbs,
        _handler,
    )
    if res == LoopAction.RETURN:
        return RouteResult.RETURN
    return res


def _resolve_root_type_label(label_key: str) -> str:
    return get_string(label_key) if label_key.startswith("menu_") else label_key


def _build_root_dispatch_map(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    type_breadcrumbs: Dict[str, str],
) -> Dict[str, Callable[[], MenuReturn]]:
    return {
        "1": lambda: _root_action_menu(
            dev,
            registry,
            gki=False,
            root_type="kernelsu",
            breadcrumbs=type_breadcrumbs["1"],
        ),
        "2": lambda: _handle_ksu_mode(dev, registry, type_breadcrumbs["2"]),
        "3": lambda: _root_action_menu(
            dev,
            registry,
            gki=False,
            root_type="sukisu",
            breadcrumbs=type_breadcrumbs["3"],
        ),
        "4": lambda: _root_action_menu(
            dev,
            registry,
            gki=False,
            root_type="resukisu",
            breadcrumbs=type_breadcrumbs["4"],
        ),
        "5": lambda: _root_action_menu(
            dev,
            registry,
            gki=True,
            root_type="apatch",
            breadcrumbs=type_breadcrumbs["5"],
        ),
        "6": lambda: _root_action_menu(
            dev,
            registry,
            gki=True,
            root_type="folkpatch",
            breadcrumbs=type_breadcrumbs["6"],
        ),
    }


def _build_root_type_menu(main_title: str) -> TerminalMenu:
    menu = TerminalMenu(get_string("menu_root_type_title"), breadcrumbs=main_title)

    for item in ROOT_TYPE_MENU_SPEC:
        if item is None:
            menu.add_separator()
            continue

        key, label = item
        menu.add_option(
            key,
            get_string(label) if label.startswith("menu_") else label,
        )

    return menu


def root_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
) -> MenuReturn:
    main_title = get_string("menu_main_title")
    root_type_title = get_string("menu_root_type_title")
    type_breadcrumbs = {
        key: f"{main_title} > {root_type_title} > {_resolve_root_type_label(label)}"
        for key, label in ROOT_TYPE_BREADCRUMB_LABELS.items()
    }
    dispatch_map = _build_root_dispatch_map(dev, registry, type_breadcrumbs)

    while True:
        mode_menu = _build_root_type_menu(main_title)

        choice = mode_menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "b":
            return LoopAction.BACK
        if choice == "x":
            return LoopAction.EXIT

        if choice is not None:
            action_func = dispatch_map.get(choice)
            if action_func is not None:
                res = action_func()
                if res in (RouteResult.MAIN, RouteResult.RETURN):
                    return res
                if res == LoopAction.EXIT:
                    return LoopAction.EXIT


def _handle_update_check():
    ui.clear()
    ui.echo(get_string("act_update_checking"))

    current_version, latest_version, latest_release, latest_prerelease = (
        update_service.get_update_status()
    )

    if latest_version:
        update_service.prompt_for_update(current_version, latest_version)
    else:
        if latest_release or latest_prerelease:
            ui.echo(get_string("act_update_not_found").format(version=current_version))
        else:
            ui.echo(get_string("act_update_error").format(e="Unknown version"))

    ui.echo("")
    input(get_string("press_enter_to_continue"))


def settings_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    state: AppState,
) -> tuple[AppState, MenuReturn]:
    main_title = get_string("menu_main_title")
    next_state = state

    def _apply_selected_preset(preset_choice: str) -> None:
        nonlocal next_state
        if preset_choice == "1":
            next_state = replace(
                next_state,
                target_region="PRC",
                modify_region_code=True,
                skip_rollback=False,
                preset_code="1",
            )
        elif preset_choice == "2":
            next_state = replace(
                next_state,
                target_region="ROW",
                modify_region_code=True,
                skip_rollback=False,
                preset_code="2",
            )
        elif preset_choice == "3":
            next_state = replace(
                next_state,
                modify_region_code=False,
                skip_rollback=True,
                preset_code="3",
            )

    def _select_preset():
        current_preset_code = next_state.preset_code
        if current_preset_code == "1":
            _apply_selected_preset("2")
        elif current_preset_code == "2":
            _apply_selected_preset("3")
        elif current_preset_code == "3":
            _apply_selected_preset("1")
        else:
            _apply_selected_preset("1")

    def _toggle_region():
        nonlocal next_state
        next_state = replace(
            next_state,
            target_region="ROW" if next_state.target_region == "PRC" else "PRC",
            preset_code="-",
        )

    def _toggle_adb():
        nonlocal next_state
        next_state = replace(next_state, skip_adb=not next_state.skip_adb)
        dev.skip_adb = next_state.skip_adb

    def _toggle_rollback():
        nonlocal next_state
        next_state = replace(
            next_state, skip_rollback=not next_state.skip_rollback, preset_code="-"
        )

    def _toggle_modify_region_code():
        nonlocal next_state
        next_state = replace(
            next_state,
            modify_region_code=not next_state.modify_region_code,
            preset_code="-",
        )

    def _change_lang():
        cmd_info = registry.get("change_language")
        if cmd_info:
            cmd_info.func(
                breadcrumbs=f"{main_title} > {get_string('menu_settings_title')}"
            )

    action_handlers = {
        "toggle_region": _toggle_region,
        "toggle_adb": _toggle_adb,
        "toggle_rollback": _toggle_rollback,
        "toggle_modify_region_code": _toggle_modify_region_code,
        "change_lang": _change_lang,
        "check_update": _handle_update_check,
    }

    def _handler(act: str) -> None:
        func = action_handlers.get(act)
        if func:
            func()

    action = _loop_menu(
        lambda: menu_data.get_settings_menu_data(
            _preset_label_from_code(next_state.preset_code),
            "ON" if next_state.skip_adb else "OFF",
            "ON" if next_state.skip_rollback else "OFF",
            "ON" if next_state.modify_region_code else "OFF",
            next_state.target_region,
        ),
        "menu_settings_title",
        main_title,
        _handler,
    )

    return next_state, action


def build_task_kwargs(action: str, state: AppState) -> Dict[str, Any]:
    extras: Dict[str, Any] = {}
    if action in [MainMenuAction.PATCH_ALL, MainMenuAction.PATCH_ALL_WIPE]:
        extras["skip_rollback"] = state.skip_rollback
        extras["target_region"] = state.target_region
        extras["modify_region_code"] = state.modify_region_code
    return extras


def prompt_for_language(
    force_prompt: bool = False,
    settings_store: Any = None,
    breadcrumbs: Optional[str] = None,
) -> str:
    if settings_store is None:
        raise ValueError("settings_store is required")

    if not force_prompt:
        settings = settings_store.load()
        saved_lang = settings.language

        if saved_lang:
            try:
                available_languages = i18n.get_available_languages()
                avail_codes = [code for code, _ in available_languages]

                if saved_lang in avail_codes:
                    return saved_lang
            except RuntimeError:
                pass

    i18n.load_lang("en")

    try:
        available_languages = i18n.get_available_languages()
    except RuntimeError as e:
        print(get_string("err_lang_generic").format(e=e), file=sys.stderr)
        input(get_string("press_enter_to_continue"))
        raise e

    menu = TerminalMenu(get_string("menu_lang_title"), breadcrumbs=breadcrumbs)
    lang_map = {}

    for i, (lang_code, lang_name) in enumerate(available_languages, 1):
        key = str(i)
        lang_map[key] = lang_code
        menu.add_option(key, lang_name)

    prompt = get_string("prompt_select").format(len=len(lang_map))
    error_msg = get_string("err_invalid_selection").format(len=len(lang_map))

    choice = menu.ask(prompt, error_msg)
    selected_lang = lang_map[choice]

    settings_store.update(language=selected_lang)

    return selected_lang


def main_loop(
    device_controller_class: DeviceControllerFactoryProtocol,
    registry: CommandRegistry,
    initial_state: AppState,
) -> AppState:
    state = initial_state
    dev = device_controller_class(state.skip_adb)

    def _run_settings() -> MenuReturn:
        nonlocal state
        state, action = settings_menu(dev, registry, state)
        return action

    menu_handlers: Dict[str, Callable[[], MenuReturn]] = {
        MainMenuAction.SETTINGS: _run_settings,
        MainMenuAction.ROOT: lambda: root_menu(dev, registry),
        MainMenuAction.ADVANCED: lambda: advanced_menu(
            dev, registry, state.target_region
        ),
    }

    def _handler(action: str) -> MenuReturn:
        action_func = menu_handlers.get(action)
        if action_func:
            result = action_func()
            if result == LoopAction.BACK:
                return None
            return result

        extras = build_task_kwargs(action, state)
        run_task(action, dev, registry, extra_kwargs=extras)
        return None

    action = _loop_menu(
        lambda: menu_data.get_main_menu_data(state.target_region),
        "menu_main_title",
        None,
        _handler,
    )

    if action == LoopAction.EXIT:
        sys.exit(0)

    return state
