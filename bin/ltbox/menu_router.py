import sys
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Tuple

from . import i18n, menu_data
from .i18n import get_string
from .menu import TerminalMenu, select_menu_action
from .utils import ui
from . import update_service
from .task_runner import run_task


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
    ("2", "menu_root_type_ksu_next"),
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


def _loop_menu(
    menu_items_factory: Callable[[], List[Any]],
    title_key: str,
    breadcrumbs: Optional[str],
    action_handler: Callable[[str], Any],
) -> Optional[str]:
    while True:
        menu_items = menu_items_factory()
        action = select_menu_action(menu_items, title_key, breadcrumbs=breadcrumbs)

        if action in (LoopAction.BACK, LoopAction.RETURN, LoopAction.EXIT):
            return action

        if action is not None:
            result = action_handler(action)
            if result in (LoopAction.BACK, LoopAction.RETURN, LoopAction.EXIT):
                return result


def advanced_menu(dev: Any, registry: Any, target_region: str):
    main_title = get_string("menu_main_title")

    def _handler(action: str):
        extras: Dict[str, Any] = (
            {"target_region": target_region} if action == "convert" else {}
        )
        run_task(action, dev, registry, extra_kwargs=extras)

    action = _loop_menu(
        lambda: menu_data.get_advanced_menu_data(target_region),
        "menu_adv_title",
        main_title,
        _handler,
    )
    if action == LoopAction.EXIT:
        sys.exit()


def _root_action_menu(
    dev: Any, registry: Any, gki: bool, root_type: str, breadcrumbs: str
):
    def _handler(action: str):
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
    if res == LoopAction.EXIT:
        sys.exit()
    return res


def _handle_ksu_mode(dev: Any, registry: Any, type_breadcrumbs: str) -> Optional[str]:
    mode_breadcrumbs = f"{type_breadcrumbs} > {get_string('menu_root_mode_title')}"

    def _handler(mode_action: str):
        if mode_action == "lkm":
            return _root_action_menu(
                dev, registry, gki=False, root_type="ksu", breadcrumbs=mode_breadcrumbs
            )
        elif mode_action == "gki":
            return _root_action_menu(
                dev, registry, gki=True, root_type="ksu", breadcrumbs=mode_breadcrumbs
            )

    res = _loop_menu(
        menu_data.get_root_mode_menu_data,
        "menu_root_mode_title",
        type_breadcrumbs,
        _handler,
    )
    if res == LoopAction.RETURN:
        return RouteResult.RETURN
    if res == LoopAction.EXIT:
        sys.exit()
    return None


def _build_root_dispatch_map(
    dev: Any, registry: Any, type_breadcrumbs: str
) -> Dict[str, Callable[[], Optional[str]]]:
    return {
        "1": lambda: _root_action_menu(
            dev, registry, gki=False, root_type="kernelsu", breadcrumbs=type_breadcrumbs
        ),
        "2": lambda: _handle_ksu_mode(dev, registry, type_breadcrumbs),
        "3": lambda: _root_action_menu(
            dev, registry, gki=False, root_type="sukisu", breadcrumbs=type_breadcrumbs
        ),
        "4": lambda: _root_action_menu(
            dev, registry, gki=False, root_type="resukisu", breadcrumbs=type_breadcrumbs
        ),
        "5": lambda: _root_action_menu(
            dev, registry, gki=True, root_type="apatch", breadcrumbs=type_breadcrumbs
        ),
        "6": lambda: _root_action_menu(
            dev, registry, gki=True, root_type="folkpatch", breadcrumbs=type_breadcrumbs
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


def root_menu(dev: Any, registry: Any):
    main_title = get_string("menu_main_title")
    type_breadcrumbs = f"{main_title} > {get_string('menu_root_type_title')}"
    dispatch_map = _build_root_dispatch_map(dev, registry, type_breadcrumbs)

    while True:
        mode_menu = _build_root_type_menu(main_title)

        choice = mode_menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "b":
            return
        if choice == "x":
            sys.exit()

        if choice is not None:
            action_func = dispatch_map.get(choice)
            if action_func is not None:
                res = action_func()
                if res in (RouteResult.MAIN, RouteResult.RETURN):
                    return


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
    dev: Any,
    registry: Any,
    skip_adb: bool,
    skip_rollback: bool,
    target_region: str,
    settings_store: Any,
) -> Tuple[bool, bool, str]:
    main_title = get_string("menu_main_title")

    def _toggle_region():
        nonlocal target_region
        target_region = "ROW" if target_region == "PRC" else "PRC"
        settings_store.update(target_region=target_region)

    def _toggle_adb():
        nonlocal skip_adb
        skip_adb = not skip_adb
        dev.skip_adb = skip_adb

    def _toggle_rollback():
        nonlocal skip_rollback
        skip_rollback = not skip_rollback

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
        "change_lang": _change_lang,
        "check_update": _handle_update_check,
    }

    def _handler(act: str):
        func = action_handlers.get(act)
        if func:
            func()

    action = _loop_menu(
        lambda: menu_data.get_settings_menu_data(
            "ON" if skip_adb else "OFF", "ON" if skip_rollback else "OFF", target_region
        ),
        "menu_settings_title",
        main_title,
        _handler,
    )

    if action == LoopAction.EXIT:
        sys.exit()

    return skip_adb, skip_rollback, target_region


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
    device_controller_class: Any,
    registry: Any,
    settings_store: Any,
):
    settings = settings_store.load()

    state = {
        "skip_adb": False,
        "skip_rollback": False,
        "target_region": settings.target_region,
    }
    dev = device_controller_class(skip_adb=state["skip_adb"])

    def _run_settings():
        state["skip_adb"], state["skip_rollback"], state["target_region"] = (
            settings_menu(
                dev,
                registry,
                state["skip_adb"],
                state["skip_rollback"],
                state["target_region"],
                settings_store,
            )
        )

    menu_handlers: Dict[str, Callable[[], Any]] = {
        MainMenuAction.SETTINGS: _run_settings,
        MainMenuAction.ROOT: lambda: root_menu(dev, registry),
        MainMenuAction.ADVANCED: lambda: advanced_menu(
            dev, registry, state["target_region"]
        ),
    }

    def _handler(action: str):
        action_func = menu_handlers.get(action)
        if action_func:
            action_func()
        else:
            extras: Dict[str, Any] = {}
            if action in [MainMenuAction.PATCH_ALL, MainMenuAction.PATCH_ALL_WIPE]:
                extras["skip_rollback"] = state["skip_rollback"]
                extras["target_region"] = state["target_region"]
            run_task(action, dev, registry, extra_kwargs=extras)

    _loop_menu(
        lambda: menu_data.get_main_menu_data(state["target_region"]),
        "menu_main_title",
        None,
        _handler,
    )
