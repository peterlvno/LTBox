import re
import subprocess
import sys
import time
from dataclasses import dataclass, replace
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Protocol, Union

from .. import constants as const
from .. import i18n, update_service
from ..app_state import AppState
from ..device.status import DeviceStatusMonitor
from ..device.support import DeviceCommandRunner, find_edl_port, format_serial_port_bare
from ..i18n import get_string
from ..registry import CommandRegistry
from ..root_profiles import (
    RootProviderProfile,
    iter_root_type_menu_profiles,
)
from ..task_runner import run_task
from ..utils import ui
from . import data as menu_data
from .terminal import TerminalMenu, select_menu_action


_PAREN_SUFFIX = re.compile(r"\s*\([^)]*\)\s*$")


def _short_label(text: str) -> str:
    """Strip trailing parenthetical from a label for use in breadcrumbs."""
    return _PAREN_SUFFIX.sub("", text)


class LoopAction(str, Enum):
    BACK = "back"
    RETURN = "return"
    EXIT = "exit"


class MainMenuAction(str, Enum):
    SETTINGS = "menu_settings"
    ROOT = "menu_root"
    ADVANCED = "menu_advanced"
    REBOOT = "menu_reboot"
    PATCH_ALL = "patch_all"
    PATCH_ALL_WIPE = "patch_all_wipe"


class RouteResult(str, Enum):
    MAIN = "main"
    RETURN = "return"


class DeviceControllerProtocol(Protocol):
    skip_adb: bool


class DeviceControllerFactoryProtocol(Protocol):
    def __call__(self, skip_adb: bool) -> DeviceControllerProtocol: ...


MenuReturn = Optional[Union[LoopAction, RouteResult]]


PRESET_1_KEY = "menu_settings_preset_1"
PRESET_2_KEY = "menu_settings_preset_2"
PRESET_3_KEY = "menu_settings_preset_3"
SKIP_ADB_BLOCKED_ACTIONS = {"disable_ota", "reenable_ota"}
PRESET_SELECTION_ORDER: Dict[str, str] = {"1": "2", "2": "3", "3": "1", "-": "1"}
PRESET_UPDATES: Dict[str, Dict[str, Any]] = {
    "1": {
        "target_region": "PRC",
        "modify_region_code": True,
        "modify_rollback_index": "ON",
        "preset_code": "1",
    },
    "2": {
        "target_region": "ROW",
        "modify_region_code": True,
        "modify_rollback_index": "ON",
        "preset_code": "2",
    },
    "3": {
        "modify_region_code": False,
        "modify_rollback_index": "AUTO",
        "preset_code": "3",
    },
}
ROLLBACK_CYCLE = {"ON": "AUTO", "AUTO": "OFF", "OFF": "ON"}


@dataclass(frozen=True)
class SettingsActionSpec:
    state_transform: Optional[Callable[[AppState], AppState]] = None
    effect: Optional[Callable[[], None]] = None
    sync_skip_adb: bool = False


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


def _apply_selected_preset(state: AppState, preset_choice: str) -> AppState:
    updates = PRESET_UPDATES.get(preset_choice)
    if not updates:
        return state
    return replace(state, **updates)


def _select_next_preset(state: AppState) -> AppState:
    next_preset = PRESET_SELECTION_ORDER.get(state.preset_code, "1")
    return _apply_selected_preset(state, next_preset)


def _toggle_region(state: AppState) -> AppState:
    return replace(
        state,
        target_region="ROW" if state.target_region == "PRC" else "PRC",
        preset_code="-",
    )


def _toggle_skip_adb(state: AppState) -> AppState:
    return replace(state, skip_adb=not state.skip_adb)


def _toggle_modify_region_code(state: AppState) -> AppState:
    return replace(
        state,
        modify_region_code=not state.modify_region_code,
        preset_code="-",
    )


def _cycle_rollback_setting(state: AppState) -> AppState:
    new_val = ROLLBACK_CYCLE.get(state.modify_rollback_index, "ON")
    return replace(
        state,
        modify_rollback_index=new_val,
        preset_code="-",
    )


def _run_change_language(registry: CommandRegistry) -> None:
    cmd_info = registry.get("change_language")
    if cmd_info:
        cmd_info.func(
            breadcrumbs=f"{get_string('menu_main_title')} > {get_string('menu_settings_title')}"
        )


def _build_settings_action_specs(
    registry: CommandRegistry,
) -> Dict[str, SettingsActionSpec]:
    return {
        "select_preset": SettingsActionSpec(state_transform=_select_next_preset),
        "toggle_region": SettingsActionSpec(state_transform=_toggle_region),
        "toggle_adb": SettingsActionSpec(
            state_transform=_toggle_skip_adb,
            sync_skip_adb=True,
        ),
        "toggle_modify_region_code": SettingsActionSpec(
            state_transform=_toggle_modify_region_code
        ),
        "cycle_rollback": SettingsActionSpec(state_transform=_cycle_rollback_setting),
        "change_lang": SettingsActionSpec(
            effect=lambda: _run_change_language(registry)
        ),
        "check_update": SettingsActionSpec(effect=_handle_update_check),
    }


def _apply_settings_action(
    action: str,
    *,
    state: AppState,
    dev: DeviceControllerProtocol,
    action_specs: Dict[str, SettingsActionSpec],
) -> AppState:
    spec = action_specs.get(action)
    if spec is None:
        return state

    next_state = state
    if spec.state_transform is not None:
        next_state = spec.state_transform(state)
    if spec.sync_skip_adb:
        dev.skip_adb = next_state.skip_adb
    if spec.effect is not None:
        spec.effect()
    return next_state


def _loop_menu(
    menu_items_factory: Callable[[], List[Any]],
    title_key: str,
    breadcrumbs: Union[None, str, Callable[[], Optional[str]]],
    action_handler: Callable[[str], MenuReturn],
    status_fn: Optional[Callable[[], str]] = None,
    status_key_fn: Optional[Callable[[], str]] = None,
    title_literal: Optional[str] = None,
) -> MenuReturn:
    while True:
        resolved_bc = breadcrumbs() if callable(breadcrumbs) else breadcrumbs
        menu_items = menu_items_factory()

        action = select_menu_action(
            menu_items,
            title_key,
            breadcrumbs=resolved_bc,
            status_fn=status_fn,
            status_key_fn=status_key_fn,
            title_literal=title_literal,
        )

        if action in (LoopAction.BACK, LoopAction.RETURN, LoopAction.EXIT):
            return LoopAction(action)

        if action is not None:
            result = action_handler(action)
            if result in (RouteResult.MAIN, LoopAction.RETURN, LoopAction.EXIT):
                return result
            # result BACK is swallowed to stay in current menu loop


def advanced_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    target_region: str,
    modify_region_code_enabled: bool,
) -> MenuReturn:
    def _handler(action: str) -> None:
        extras: Dict[str, Any] = (
            {"target_region": target_region} if action == "convert" else {}
        )
        run_task(action, dev, registry, extra_kwargs=extras)

    return _loop_menu(
        lambda: menu_data.get_advanced_menu_data(
            target_region, modify_region_code_enabled
        ),
        "menu_adv_title",
        lambda: get_string("menu_main_title"),
        _handler,
    )


def _root_action_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    gki: bool,
    root_type: str,
    breadcrumbs: str,
) -> MenuReturn:
    from ..actions.root.strategies import get_root_strategy

    strategy = get_root_strategy(gki, root_type)

    if hasattr(strategy, "configure_source"):
        configured = strategy.configure_source(breadcrumbs=breadcrumbs)
        if configured is False:
            return None
        if configured in (RouteResult.MAIN, LoopAction.RETURN):
            return RouteResult.MAIN
        ui.clear()

    source_label = getattr(strategy, "source_label", "")

    title_key = "menu_root_title"
    title_literal = None
    if source_label:
        title_literal = _short_label(source_label)

    def _handler(action: str) -> None:
        extras: Dict[str, Any] = {"root_type": root_type, "strategy": strategy}
        run_task(action, dev, registry, extra_kwargs=extras)

    res = _loop_menu(
        lambda: menu_data.get_root_menu_data(gki, root_type=root_type),
        title_key,
        lambda: breadcrumbs,
        _handler,
        title_literal=title_literal,
    )
    if res == LoopAction.RETURN:
        return RouteResult.MAIN
    return res


def _resolve_root_type_label(profile: RootProviderProfile) -> str:
    if profile.menu_label_key:
        return get_string(profile.menu_label_key)
    return profile.menu_label_literal or profile.display_name


def _build_root_dispatch_map(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    type_breadcrumbs: Dict[str, str],
) -> Dict[str, Callable[[], MenuReturn]]:
    dispatch_map: Dict[str, Callable[[], MenuReturn]] = {}
    for profile in (p for p in iter_root_type_menu_profiles() if p is not None):
        breadcrumbs = type_breadcrumbs[profile.menu_key]
        dispatch_map[profile.menu_key] = (
            lambda profile=profile, breadcrumbs=breadcrumbs: _root_action_menu(  # type: ignore[misc]
                dev,
                registry,
                gki=bool(profile.direct_gki),
                root_type=profile.strategy_root_type,
                breadcrumbs=breadcrumbs,
            )
        )
    return dispatch_map


def _build_root_type_menu(main_title: str) -> TerminalMenu:
    menu = TerminalMenu(get_string("menu_root_type_title"), breadcrumbs=main_title)

    for profile in iter_root_type_menu_profiles():
        if profile is None:
            menu.add_separator()
            continue

        menu.add_option(profile.menu_key, _resolve_root_type_label(profile))

    menu.add_option("b", get_string("menu_back"))

    return menu


def _root_lkm_variants_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    breadcrumbs: str,
) -> MenuReturn:
    from ..root_profiles import get_root_provider_profile

    lkm_profiles = [
        get_root_provider_profile("kernelsu"),
        get_root_provider_profile("kernelsu-next"),
        get_root_provider_profile("sukisu"),
        get_root_provider_profile("resukisu"),
    ]

    while True:
        menu = TerminalMenu(get_string("menu_root_mode_lkm"), breadcrumbs=breadcrumbs)
        for profile in lkm_profiles:
            menu.add_option(profile.menu_key, _resolve_root_type_label(profile))
        menu.add_separator()
        menu.add_option("b", get_string("menu_back"))
        menu.add_option("m", get_string("menu_root_m"))

        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )
        if choice == "b":
            return LoopAction.BACK
        if choice == "m":
            return LoopAction.RETURN

        selected_profile = next((p for p in lkm_profiles if p.menu_key == choice), None)
        if selected_profile:
            res = _root_action_menu(
                dev,
                registry,
                gki=False,
                root_type=selected_profile.strategy_root_type,
                breadcrumbs=f"{breadcrumbs} > {_resolve_root_type_label(selected_profile)}",
            )
            if res in (RouteResult.MAIN, RouteResult.RETURN, LoopAction.EXIT):
                return res


def _root_ksu_variants_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    breadcrumbs: str,
) -> MenuReturn:
    from ..root_profiles import get_root_provider_profile

    # We append the current menu's title for children breadcrumbs
    current_breadcrumbs = f"{breadcrumbs} > {get_string('menu_root_variants_ksu')}"

    def _handler(action: str) -> MenuReturn:
        if action == "lkm_mode":
            return _root_lkm_variants_menu(
                dev,
                registry,
                current_breadcrumbs,
            )
        if action == "gki_mode":
            profile = get_root_provider_profile("gki")
            return _root_action_menu(
                dev,
                registry,
                gki=True,
                root_type="gki",
                breadcrumbs=f"{current_breadcrumbs} > {_resolve_root_type_label(profile)}",
            )
        return None

    return _loop_menu(
        menu_data.get_root_ksu_modes_menu_data,
        "menu_root_variants_ksu",
        breadcrumbs,
        _handler,
    )


def _root_apatch_variants_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    breadcrumbs: str,
) -> MenuReturn:
    from ..root_profiles import get_root_provider_profile

    # We append the current menu's title for children breadcrumbs
    current_breadcrumbs = f"{breadcrumbs} > {get_string('menu_root_variants_apatch')}"

    def _handler(action: str) -> MenuReturn:
        profile = get_root_provider_profile(action)
        return _root_action_menu(
            dev,
            registry,
            gki=True,
            root_type=profile.strategy_root_type,
            breadcrumbs=f"{current_breadcrumbs} > {_resolve_root_type_label(profile)}",
        )

    return _loop_menu(
        menu_data.get_root_apatch_variants_menu_data,
        "menu_root_variants_apatch",
        breadcrumbs,
        _handler,
    )


def root_menu(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
) -> MenuReturn:
    main_title = get_string("menu_main_title")
    breadcrumbs = f"{main_title} > {get_string('menu_main_root')}"

    def _handler(action: str) -> MenuReturn:
        if action == "ksu_variants":
            return _root_ksu_variants_menu(
                dev,
                registry,
                breadcrumbs,
            )
        if action == "apatch_variants":
            return _root_apatch_variants_menu(
                dev,
                registry,
                breadcrumbs,
            )
        return None

    res = _loop_menu(
        menu_data.get_root_variants_menu_data,
        "menu_main_root",
        main_title,
        _handler,
    )
    if res == RouteResult.MAIN or res == LoopAction.RETURN:
        return None
    return res


def _execute_reboot_command(action: str) -> None:
    adb_cmds = {
        "reboot_adb_system": [str(const.ADB_EXE), "reboot"],
        "reboot_adb_bootloader": [str(const.ADB_EXE), "reboot", "bootloader"],
        "reboot_adb_fastboot": [str(const.ADB_EXE), "reboot", "fastboot"],
        "reboot_adb_edl": [str(const.ADB_EXE), "reboot", "edl"],
    }

    fb_cmds = {
        "reboot_fb_system": [str(const.FASTBOOT_EXE), "reboot"],
        "reboot_fb_bootloader": [str(const.FASTBOOT_EXE), "reboot", "bootloader"],
        "reboot_fb_fastboot": [str(const.FASTBOOT_EXE), "reboot", "fastboot"],
        "reboot_fb_edl": [str(const.FASTBOOT_EXE), "oem", "edl"],
    }

    cmd = adb_cmds.get(action) or fb_cmds.get(action)
    if cmd:
        ui.clear()
        ui.info(get_string("reboot_sending"))
        runner = DeviceCommandRunner()
        try:
            runner.run(
                cmd,
                capture=True,
                timeout=15,
            )
            ui.info(get_string("reboot_sent_success"))
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
            ui.error(get_string("reboot_failed").format(e=e))
        input(get_string("press_enter_to_continue"))
        return

    if action == "reboot_edl_system":
        _reboot_from_edl()
        return


def _reboot_from_edl() -> None:
    ui.clear()
    ui.info(get_string("reboot_edl_start"))

    edl_port = find_edl_port()
    if not edl_port:
        ui.error(get_string("reboot_edl_port_not_found"))
        input(get_string("press_enter_to_continue"))
        return

    ui.info(get_string("reboot_edl_found_port").format(port=edl_port))

    if not const.EDL_LOADER_FILE.exists():
        ui.error(
            get_string("reboot_edl_loader_missing").format(
                file=const.EDL_LOADER_FILENAME, dir=const.IMAGE_DIR.name
            )
        )
        input(get_string("press_enter_to_continue"))
        return

    try:
        ui.info(get_string("reboot_edl_uploading"))
        base_cmd = [
            str(const.QDLRS_EXE),
            "--backend",
            "serial",
            "-d",
            format_serial_port_bare(edl_port),
            "-l",
            str(const.EDL_LOADER_FILE),
            "-s",
            "ufs",
        ]

        subprocess.run(base_cmd + ["nop"], check=True, timeout=30)
        time.sleep(2)

        ui.info(get_string("reboot_edl_resetting"))
        subprocess.run(
            base_cmd + ["--reset-mode", "system", "reset", "system"],
            check=True,
            timeout=30,
        )

        ui.info(get_string("reboot_sent_success"))
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
        ui.error(get_string("reboot_edl_failed").format(e=e))
        ui.warn(get_string("reboot_edl_manual_hint"))

    input(get_string("press_enter_to_continue"))


def reboot_menu(
    monitor: Any,
) -> MenuReturn:
    def _handler(action: str) -> None:
        _execute_reboot_command(action)

    return _loop_menu(
        lambda: menu_data.get_reboot_menu_data(monitor.get_status_key()),
        "menu_reboot_title",
        lambda: get_string("menu_main_title"),
        _handler,
        status_fn=monitor.get_status_text,
        status_key_fn=monitor.get_status_key,
    )


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
    next_state = state
    action_specs = _build_settings_action_specs(registry)

    def _handler(act: str) -> None:
        nonlocal next_state
        next_state = _apply_settings_action(
            act,
            state=next_state,
            dev=dev,
            action_specs=action_specs,
        )

    action = _loop_menu(
        lambda: menu_data.get_settings_menu_data(
            _preset_label_from_code(next_state.preset_code),
            "ON" if next_state.skip_adb else "OFF",
            next_state.modify_region_code,
            next_state.target_region,
            next_state.modify_rollback_index,
        ),
        "menu_settings_title",
        lambda: get_string("menu_main_title"),
        _handler,
    )

    return next_state, action


def build_task_kwargs(action: str, state: AppState) -> Dict[str, Any]:
    if action in [MainMenuAction.PATCH_ALL, MainMenuAction.PATCH_ALL_WIPE]:
        return {
            "target_region": state.target_region,
            "modify_region_code": state.modify_region_code,
            "modify_rollback_index": state.modify_rollback_index,
        }
    return {}


def _build_main_menu_handlers(
    dev: DeviceControllerProtocol,
    registry: CommandRegistry,
    monitor: Any,
    *,
    run_settings: Callable[[], MenuReturn],
    get_state: Callable[[], AppState],
) -> Dict[str, Callable[[], MenuReturn]]:
    return {
        MainMenuAction.SETTINGS: run_settings,
        MainMenuAction.ROOT: lambda: root_menu(dev, registry),
        MainMenuAction.ADVANCED: lambda: advanced_menu(
            dev,
            registry,
            get_state().target_region,
            get_state().modify_region_code,
        ),
        MainMenuAction.REBOOT: lambda: reboot_menu(monitor),
    }


def _handle_skip_adb_menu_block(action: str, state: AppState) -> bool:
    if not state.skip_adb or action not in SKIP_ADB_BLOCKED_ACTIONS:
        return False

    ui.clear()
    ui.warn(get_string("menu_main_skip_adb_disabled_required"))
    input(get_string("press_enter_to_continue"))
    return True


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

    monitor = DeviceStatusMonitor()
    monitor.start()

    def _run_settings() -> MenuReturn:
        nonlocal state
        state, action = settings_menu(dev, registry, state)
        return action

    menu_handlers = _build_main_menu_handlers(
        dev,
        registry,
        monitor,
        run_settings=_run_settings,
        get_state=lambda: state,
    )

    def _handler(action: str) -> MenuReturn:
        action_func = menu_handlers.get(action)
        if action_func:
            result = action_func()
            if result == LoopAction.BACK:
                return None
            return result

        if _handle_skip_adb_menu_block(action, state):
            return None

        extras = build_task_kwargs(action, state)
        run_task(action, dev, registry, extra_kwargs=extras)
        return None

    try:
        action = _loop_menu(
            lambda: menu_data.get_main_menu_data(
                state.target_region, state.modify_region_code
            ),
            "menu_main_title",
            None,
            _handler,
            status_fn=monitor.get_status_text,
        )
    finally:
        monitor.stop()

    if action == LoopAction.EXIT:
        sys.exit(0)

    # RETURN or BACK from main loop just returns current state to caller (which saves settings)
    return state
