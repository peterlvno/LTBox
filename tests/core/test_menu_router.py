from unittest.mock import MagicMock

from ltbox import menu_router


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
    dispatch_map = menu_router._build_root_dispatch_map(
        dev=object(), registry=object(), type_breadcrumbs="main > root"
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

    called = {"count": 0}

    def fake_dispatch(_dev, _registry, _breadcrumbs):
        called["count"] += 1
        return {"1": lambda: menu_router.RouteResult.MAIN}

    monkeypatch.setattr(menu_router, "_build_root_dispatch_map", fake_dispatch)

    result = menu_router.root_menu(dev=object(), registry=object())

    assert called["count"] == 1
    assert result is None
