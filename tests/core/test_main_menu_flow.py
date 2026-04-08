import pytest
from unittest.mock import MagicMock

from ltbox import menu_data, menu_router
from ltbox.app_state import AppState


def test_build_root_type_menu_uses_declarative_spec(monkeypatch):
    menu = MagicMock()
    menu.ask.return_value = "b"

    monkeypatch.setattr(menu_router, "TerminalMenu", lambda title, breadcrumbs: menu)
    monkeypatch.setattr(menu_router, "get_string", lambda key: f"T::{key}")

    built = menu_router._build_root_type_menu("main")

    assert built is menu
    assert menu.add_option.call_count == 9
    assert menu.add_separator.call_count == 4


def test_build_root_dispatch_map_covers_all_root_choices():
    breadcrumbs = {
        key: f"main > root > {key}" for key in ["1", "2", "3", "4", "5", "6", "7"]
    }
    dispatch_map = menu_router._build_root_dispatch_map(
        dev=object(), registry=object(), type_breadcrumbs=breadcrumbs
    )

    assert sorted(dispatch_map.keys()) == ["1", "2", "3", "4", "5", "6", "7"]


def test_root_menu_uses_dispatch_map_and_returns_on_route(monkeypatch):
    class FakeMenu:
        def __init__(self, choices):
            self._choices = iter(choices)

        def ask(self, *_args):
            return next(self._choices)

    fake_menu = FakeMenu(["1"])

    monkeypatch.setattr(menu_router, "_build_root_type_menu", lambda *_args: fake_menu)
    monkeypatch.setattr(menu_router, "get_string", lambda key: key)

    called = {"count": 0, "breadcrumbs": None}

    def fake_dispatch(_dev, _registry, breadcrumbs):
        called["count"] += 1
        called["breadcrumbs"] = breadcrumbs
        return {"1": lambda: menu_router.RouteResult.MAIN}

    monkeypatch.setattr(menu_router, "_build_root_dispatch_map", fake_dispatch)

    result = menu_router.root_menu(dev=object(), registry=object())

    assert called["count"] == 1
    assert called["breadcrumbs"]["5"].endswith(" > APatch")
    assert called["breadcrumbs"]["6"].endswith(" > FolkPatch")
    assert called["breadcrumbs"]["7"].endswith(" > menu_root_type_gki")
    assert result is menu_router.RouteResult.MAIN


def test_build_root_dispatch_map_routes_with_selected_type_breadcrumbs(monkeypatch):
    received = []

    def fake_root_action_menu(_dev, _registry, gki, root_type, breadcrumbs):
        received.append((gki, root_type, breadcrumbs))

    monkeypatch.setattr(menu_router, "_root_action_menu", fake_root_action_menu)

    breadcrumbs = {
        "1": "main > root > KernelSU",
        "2": "main > root > KernelSU Next",
        "3": "main > root > SukiSU",
        "4": "main > root > ReSukiSU",
        "5": "main > root > APatch",
        "6": "main > root > FolkPatch",
        "7": "main > root > GKI Mode",
    }
    dispatch_map = menu_router._build_root_dispatch_map(
        dev=object(), registry=object(), type_breadcrumbs=breadcrumbs
    )

    dispatch_map["5"]()
    dispatch_map["6"]()
    dispatch_map["7"]()

    assert received == [
        (True, "apatch", "main > root > APatch"),
        (True, "folkpatch", "main > root > FolkPatch"),
        (True, "gki", "main > root > GKI Mode"),
    ]


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
