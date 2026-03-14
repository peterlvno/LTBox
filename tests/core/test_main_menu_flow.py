from unittest.mock import MagicMock

from ltbox import menu_router
from ltbox.app_state import AppState


def test_build_root_type_menu_uses_declarative_spec(monkeypatch):
    menu = MagicMock()
    menu.ask.return_value = "b"

    monkeypatch.setattr(menu_router, "TerminalMenu", lambda title, breadcrumbs: menu)
    monkeypatch.setattr(menu_router, "get_string", lambda key: f"T::{key}")

    built = menu_router._build_root_type_menu("main")

    assert built is menu
    assert menu.add_option.call_count == 8
    assert menu.add_separator.call_count == 3


def test_build_root_dispatch_map_covers_all_root_choices():
    breadcrumbs = {
        key: f"main > root > {key}" for key in ["1", "2", "3", "4", "5", "6"]
    }
    dispatch_map = menu_router._build_root_dispatch_map(
        dev=object(), registry=object(), type_breadcrumbs=breadcrumbs
    )

    assert sorted(dispatch_map.keys()) == ["1", "2", "3", "4", "5", "6"]


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
    }
    dispatch_map = menu_router._build_root_dispatch_map(
        dev=object(), registry=object(), type_breadcrumbs=breadcrumbs
    )

    dispatch_map["5"]()
    dispatch_map["6"]()

    assert received == [
        (True, "apatch", "main > root > APatch"),
        (True, "folkpatch", "main > root > FolkPatch"),
    ]


def test_build_task_kwargs_uses_app_state_for_patch_actions():
    state = AppState(skip_rollback=True, target_region="ROW")

    extras = menu_router.build_task_kwargs(menu_router.MainMenuAction.PATCH_ALL, state)

    assert extras == {"skip_rollback": True, "target_region": "ROW"}
    assert menu_router.build_task_kwargs("menu_root", state) == {}


def test_settings_menu_returns_updated_state(monkeypatch):
    actions = iter(["toggle_region", "toggle_adb", "toggle_rollback", "back"])
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
        skip_adb=True, skip_rollback=True, target_region="ROW", language=None
    )
    assert action == menu_router.LoopAction.BACK
    assert dev.skip_adb is True
