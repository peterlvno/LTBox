import pytest
from unittest.mock import MagicMock

from ltbox.app_state import AppState
from ltbox.menus import data as menu_data
from ltbox.menus import router as menu_router


def test_root_menu_ksu_lkm_flow(monkeypatch):
    """Test flow: Main > Root > KSU variants > LKM Mode > KernelSU"""
    received = []

    def mock_loop_menu(data_fn, title_key, breadcrumbs, handler):
        # 1. Main Root Menu -> select ksu_variants
        if title_key == "menu_main_root":
            return handler("ksu_variants")
        # 2. KSU Variants Menu -> select lkm_mode
        if title_key == "menu_root_variants_ksu":
            return handler("lkm_mode")
        return None

    monkeypatch.setattr(menu_router, "_loop_menu", mock_loop_menu)

    # 3. LKM Mode Menu (manual loop) -> select '1' (KernelSU)
    class FakeTerminalMenu:
        def __init__(self, *args, **kwargs):
            pass

        def add_option(self, *args):
            pass

        def add_separator(self, *args):
            pass

        def ask(self, *args):
            return "1"

    monkeypatch.setattr(menu_router, "TerminalMenu", FakeTerminalMenu)

    # 4. Action Menu
    def mock_root_action_menu(dev, reg, gki, root_type, breadcrumbs):
        received.append((gki, root_type, breadcrumbs))
        return menu_router.RouteResult.MAIN

    monkeypatch.setattr(menu_router, "_root_action_menu", mock_root_action_menu)
    monkeypatch.setattr(menu_router, "get_string", lambda k: k)

    result = menu_router.root_menu(MagicMock(), MagicMock())

    assert result is None  # Loop menu returns MAIN which maps to None in root_menu
    # Breadcrumbs: Main > Root device > KernelSU variants > KernelSU
    assert received[0] == (
        False,
        "kernelsu",
        "menu_main_title > menu_main_root > menu_root_variants_ksu > menu_root_type_ksu",
    )


def test_root_menu_apatch_flow(monkeypatch):
    """Test flow: Main > Root > APatch variants > FolkPatch"""
    received = []

    def mock_loop_menu(data_fn, title_key, breadcrumbs, handler):
        if title_key == "menu_main_root":
            return handler(
                "ksu_variants"
            )  # Test ksu_variants -> apatch_variants transition or direct
        if title_key == "menu_root_variants_apatch":
            return handler("folkpatch")
        return None

    # Actually test direct apatch_variants flow
    def mock_loop_menu_apatch(data_fn, title_key, breadcrumbs, handler):
        if title_key == "menu_main_root":
            return handler("apatch_variants")
        if title_key == "menu_root_variants_apatch":
            return handler("folkpatch")
        return None

    monkeypatch.setattr(menu_router, "_loop_menu", mock_loop_menu_apatch)

    def mock_root_action_menu(dev, reg, gki, root_type, breadcrumbs):
        received.append((gki, root_type, breadcrumbs))
        return menu_router.RouteResult.MAIN

    monkeypatch.setattr(menu_router, "_root_action_menu", mock_root_action_menu)
    monkeypatch.setattr(menu_router, "get_string", lambda k: k)

    menu_router.root_menu(MagicMock(), MagicMock())

    assert received[0] == (
        True,
        "folkpatch",
        "menu_main_title > menu_main_root > menu_root_variants_apatch > FolkPatch",
    )


def test_root_menu_magisk_other_forks_flow(monkeypatch):
    received = []

    def mock_loop_menu(data_fn, title_key, breadcrumbs, handler):
        if title_key == "menu_main_root":
            return handler("magisk_variants")
        if title_key == "menu_root_variants_magisk":
            return handler("other_forks")
        return None

    monkeypatch.setattr(menu_router, "_loop_menu", mock_loop_menu)

    def mock_root_action_menu(dev, reg, gki, root_type, breadcrumbs):
        received.append((gki, root_type, breadcrumbs))
        return menu_router.RouteResult.MAIN

    monkeypatch.setattr(menu_router, "_root_action_menu", mock_root_action_menu)
    monkeypatch.setattr(menu_router, "get_string", lambda k: k)

    menu_router.root_menu(MagicMock(), MagicMock())

    assert received[0] == (
        False,
        "other_forks",
        "menu_main_title > menu_main_root > menu_root_variants_magisk > Other forks",
    )


def test_loop_menu_propagates_main_result(monkeypatch):
    monkeypatch.setattr(
        menu_router,
        "select_menu_action",
        lambda *_args, **_kwargs: "go",
    )

    result = menu_router._loop_menu(
        lambda: [],
        "menu_main_root",
        "main",
        lambda _action: menu_router.RouteResult.MAIN,
    )

    assert result is menu_router.RouteResult.MAIN


def test_root_action_menu_returns_main_when_source_selection_requests_main(monkeypatch):
    class FakeStrategy:
        def configure_source(self, breadcrumbs=None):
            return menu_router.RouteResult.MAIN

    monkeypatch.setattr(
        "ltbox.actions.root.strategies.get_root_strategy",
        lambda *_args, **_kwargs: FakeStrategy(),
    )

    result = menu_router._root_action_menu(
        dev=MagicMock(),
        registry=MagicMock(),
        gki=False,
        root_type="kernelsu",
        breadcrumbs="main > root > KernelSU",
    )

    assert result is menu_router.RouteResult.MAIN


def test_build_task_kwargs_uses_app_state_for_patch_actions():
    state = AppState(modify_region_code=False, target_region="ROW")

    extras = menu_router.build_task_kwargs(menu_router.MainMenuAction.PATCH_ALL, state)

    assert extras == {
        "modify_region_code": False,
        "target_region": "ROW",
        "modify_rollback_index": "ON",
    }
    assert menu_router.build_task_kwargs("menu_root", state) == {}


def test_settings_menu_returns_updated_state(monkeypatch):
    actions = iter(
        [
            "toggle_region",
            "toggle_adb",
            "toggle_modify_region_code",
            "back",
        ]
    )
    monkeypatch.setattr(
        menu_router, "select_menu_action", lambda *_args, **_kwargs: next(actions)
    )

    class DummyDev:
        def __init__(self):
            self.skip_adb = False

    state = AppState()
    dev = DummyDev()

    result = menu_router.settings_menu(dev, registry=MagicMock(), state=state)

    next_state, action = result

    assert next_state == AppState(
        skip_adb=True,
        modify_region_code=False,
        target_region="ROW",
        preset_code="-",
        language=None,
    )
    assert action == menu_router.LoopAction.BACK
    assert dev.skip_adb is True


def test_resolve_settings_preset_label():
    assert (
        menu_router._resolve_settings_preset_label(AppState(preset_code="1"))
        == "Install Global Firmware on Chinese Device"
    )
    assert (
        menu_router._resolve_settings_preset_label(AppState(preset_code="2"))
        == "Install Chinese Firmware on Global Device"
    )
    assert (
        menu_router._resolve_settings_preset_label(AppState(preset_code="3"))
        == "Stock Firmware Install/Restore"
    )
    assert menu_router._resolve_settings_preset_label(AppState(preset_code="-")) == "-"


@pytest.mark.parametrize(
    "initial_state, expected",
    [
        pytest.param(
            AppState(),
            {
                "preset_code": "2",
                "target_region": "ROW",
                "modify_region_code": True,
                "modify_rollback_index": "ON",
            },
            id="default_to_preset_2",
        ),
        pytest.param(
            AppState(
                target_region="ROW",
                modify_region_code=True,
                preset_code="2",
            ),
            {
                "preset_code": "3",
                "target_region": "ROW",
                "modify_region_code": False,
                "modify_rollback_index": "AUTO",
            },
            id="preset_2_to_3",
        ),
        pytest.param(
            AppState(
                target_region="PRC",
                modify_region_code=True,
                preset_code="2",
            ),
            {
                "preset_code": "3",
                "target_region": "PRC",
                "modify_region_code": False,
                "modify_rollback_index": "AUTO",
            },
            id="preset_3_keeps_region",
        ),
        pytest.param(
            AppState(
                target_region="ROW",
                modify_region_code=False,
                preset_code="-",
            ),
            {
                "preset_code": "1",
                "target_region": "PRC",
                "modify_region_code": True,
                "modify_rollback_index": "ON",
            },
            id="dash_to_preset_1",
        ),
    ],
)
def test_settings_menu_preset_selection_cycle(monkeypatch, initial_state, expected):
    actions = iter(["select_preset", "back"])
    monkeypatch.setattr(
        menu_router, "select_menu_action", lambda *_args, **_kwargs: next(actions)
    )

    class DummyDev:
        def __init__(self):
            self.skip_adb = False

    next_state, action = menu_router.settings_menu(
        DummyDev(), registry=MagicMock(), state=initial_state
    )

    assert next_state.preset_code == expected["preset_code"]
    assert next_state.target_region == expected["target_region"]
    assert next_state.modify_region_code is expected["modify_region_code"]
    assert next_state.modify_rollback_index == expected["modify_rollback_index"]
    assert action == menu_router.LoopAction.BACK


def test_settings_menu_data_orders_modify_region_before_skip_adb():
    items = menu_data.get_settings_menu_data(
        preset_label="x",
        skip_adb_state="OFF",
        modify_region_code_enabled=True,
        target_region="PRC",
    )
    option_actions = [i.action for i in items if i.item_type == "option"]

    assert option_actions[:5] == [
        "select_preset",
        "toggle_modify_region_code",
        "toggle_region",
        "cycle_rollback",
        "toggle_adb",
    ]


def test_settings_menu_hides_region_toggle_when_modify_region_off():
    items = menu_data.get_settings_menu_data(
        preset_label="x",
        skip_adb_state="OFF",
        modify_region_code_enabled=False,
        target_region="PRC",
    )
    option_items = [i for i in items if i.item_type == "option"]
    option_actions = [i.action for i in option_items]
    option_keys = [i.key for i in option_items if i.key.isdigit()]

    assert "toggle_region" not in option_actions
    assert option_keys == ["1", "2", "4", "5", "6", "7"]


def test_settings_menu_data_uses_short_skip_adb_label():
    items = menu_data.get_settings_menu_data(
        preset_label="x",
        skip_adb_state="OFF",
        modify_region_code_enabled=True,
        target_region="PRC",
    )
    skip_adb_item = next(i for i in items if i.action == "toggle_adb")

    assert skip_adb_item.text == "Skip ADB: [OFF]"


def test_main_menu_hides_region_name_when_modify_region_off():
    items = menu_data.get_main_menu_data(
        target_region="PRC",
        modify_region_code_enabled=False,
    )
    option_items = [i for i in items if i.item_type == "option"]

    assert option_items[0].text == "Install Firmware on Device (Wipe Data)"
    assert option_items[1].text == "Install Firmware on Device (Keep Data)"


def test_advanced_menu_hides_convert_option_when_modify_region_off():
    items = menu_data.get_advanced_menu_data(
        target_region="PRC",
        modify_region_code_enabled=False,
    )
    option_actions = [i.action for i in items if i.item_type == "option"]

    assert "convert" not in option_actions


def test_only_main_and_advanced_menus_include_exit_option():
    main_actions = {
        item.action
        for item in menu_data.get_main_menu_data("ROW")
        if item.item_type == "option"
    }
    advanced_actions = {
        item.action
        for item in menu_data.get_advanced_menu_data("ROW")
        if item.item_type == "option"
    }

    submenu_builders = (
        lambda: menu_data.get_root_variants_menu_data(),
        lambda: menu_data.get_root_ksu_modes_menu_data(),
        lambda: menu_data.get_root_apatch_variants_menu_data(),
        lambda: menu_data.get_root_menu_data(gki=False, root_type="kernelsu"),
        lambda: menu_data.get_settings_menu_data(
            preset_label="1",
            skip_adb_state="OFF",
            modify_region_code_enabled=True,
            target_region="ROW",
        ),
        lambda: menu_data.get_reboot_menu_data("device_status_adb"),
    )

    assert "exit" in main_actions
    assert "exit" in advanced_actions

    for build_menu in submenu_builders:
        submenu_actions = {
            item.action for item in build_menu() if item.item_type == "option"
        }
        assert "exit" not in submenu_actions


def test_root_manual_submenus_do_not_offer_exit(monkeypatch):
    monkeypatch.setattr(menu_router, "get_string", lambda key: key)

    class FakeTerminalMenu:
        instances = []

        def __init__(self, *_args, **_kwargs):
            self.options = []
            type(self).instances.append(self)

        def add_option(self, key, *_args):
            self.options.append(key)

        def add_separator(self, *_args):
            pass

        def ask(self, *_args):
            return "b"

    monkeypatch.setattr(menu_router, "TerminalMenu", FakeTerminalMenu)

    root_type_menu = menu_router._build_root_type_menu("main")
    assert "x" not in root_type_menu.options

    result = menu_router._root_lkm_variants_menu(
        MagicMock(),
        MagicMock(),
        "main > root > kernelsu",
    )

    assert result is menu_router.LoopAction.BACK
    assert "x" not in FakeTerminalMenu.instances[-1].options


@pytest.mark.parametrize("action", ["disable_ota", "reenable_ota"])
def test_handle_skip_adb_menu_block_blocks_ota_actions(monkeypatch, action):
    mock_ui = MagicMock()
    monkeypatch.setattr(menu_router, "ui", mock_ui)
    monkeypatch.setattr("builtins.input", lambda *_args: "")

    blocked = menu_router._handle_skip_adb_menu_block(
        action,
        AppState(skip_adb=True),
    )

    assert blocked is True
    mock_ui.clear.assert_called_once()
    mock_ui.warn.assert_called_once_with(
        "This option is only available when 'Skip ADB' is disabled."
    )


def test_handle_skip_adb_menu_block_ignores_other_actions(monkeypatch):
    mock_ui = MagicMock()
    monkeypatch.setattr(menu_router, "ui", mock_ui)
    monkeypatch.setattr("builtins.input", lambda *_args: "")

    blocked = menu_router._handle_skip_adb_menu_block(
        "menu_root",
        AppState(skip_adb=True),
    )

    assert blocked is False
    mock_ui.warn.assert_not_called()


def test_settings_menu_direct_toggle_recomputes_preset_code(monkeypatch):
    actions = iter(["toggle_modify_region_code", "back"])
    monkeypatch.setattr(
        menu_router, "select_menu_action", lambda *_args, **_kwargs: next(actions)
    )

    class DummyDev:
        def __init__(self):
            self.skip_adb = False

    state = AppState(
        target_region="PRC",
        modify_region_code=True,
        preset_code="1",
    )
    dev = DummyDev()

    next_state, _ = menu_router.settings_menu(dev, registry=MagicMock(), state=state)

    assert next_state.modify_region_code is False
    assert next_state.preset_code == "-"


def test_reboot_from_edl_uses_tolerant_reset_path(monkeypatch, tmp_path):
    loader = tmp_path / "xbl_s_devprg_ns.melf"
    loader.write_text("loader", encoding="utf-8")

    events = []

    class FakeEdlManager:
        def load_programmer(self, port, loader_path):
            events.append(("load", port, loader_path))

        def reset(self, port, mode="system"):
            events.append(("reset", port, mode))

    strings = {
        "reboot_edl_start": "start",
        "reboot_edl_found_port": "found {port}",
        "reboot_edl_uploading": "uploading",
        "reboot_edl_resetting": "resetting",
        "reboot_sent_success": "sent",
        "press_enter_to_continue": "enter",
    }

    mock_ui = MagicMock()
    monkeypatch.setattr(menu_router, "ui", mock_ui)
    monkeypatch.setattr(menu_router, "find_edl_port", lambda: "COM5")
    monkeypatch.setattr(menu_router.time, "sleep", lambda *_args: None)
    monkeypatch.setattr(menu_router.device, "EdlManager", FakeEdlManager)
    monkeypatch.setattr(menu_router.const, "EDL_LOADER_FILE", loader)
    monkeypatch.setattr(menu_router.const, "EDL_LOADER_FILENAME", loader.name)
    monkeypatch.setattr(menu_router.const, "IMAGE_DIR", tmp_path)
    monkeypatch.setattr(menu_router, "get_string", lambda key: strings[key])
    monkeypatch.setattr("builtins.input", lambda *_args: "")

    menu_router._reboot_from_edl()

    assert events == [
        ("load", "COM5", loader),
        ("reset", "COM5", "system"),
    ]
    mock_ui.error.assert_not_called()
    mock_ui.warn.assert_not_called()
    mock_ui.info.assert_any_call(strings["reboot_sent_success"])
