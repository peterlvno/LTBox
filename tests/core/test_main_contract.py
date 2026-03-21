import json
from pathlib import Path
from unittest.mock import patch

import pytest
from ltbox import main
from ltbox import menu_router
from ltbox.app_state import AppState


def test_imports():
    assert hasattr(main, "CommandRegistry")
    assert hasattr(main, "setup_console")


def test_registry_add_and_get():
    registry = main.CommandRegistry()

    def dummy():
        return "ok"

    registry.add("cmd", dummy, "Title", require_dev=False)
    command_info = registry.get("cmd")
    assert command_info["title"] == "Title"
    assert command_info["func"]() == "ok"


def test_json_validity():
    ltbox_dir = Path(__file__).resolve().parents[2] / "bin" / "ltbox"
    files = list(ltbox_dir.rglob("*.json"))
    if not files:
        pytest.skip("No JSON")

    for path in files:
        with open(path, "r", encoding="utf-8") as fp:
            json.load(fp)


def test_config_keys():
    config_path = Path(__file__).resolve().parents[2] / "bin" / "ltbox" / "config.json"
    if config_path.exists():
        with open(config_path, "r", encoding="utf-8") as file:
            config = json.load(file)
        assert "version" in config


def test_main_loop_exits_only_at_top_level(monkeypatch):
    monkeypatch.setattr(
        menu_router,
        "_loop_menu",
        lambda *_args, **_kwargs: menu_router.LoopAction.EXIT,
    )

    with pytest.raises(SystemExit) as exc:
        menu_router.main_loop(
            device_controller_class=lambda skip_adb: type(
                "Dev", (), {"skip_adb": skip_adb}
            )(),
            registry=main.CommandRegistry(),
            initial_state=AppState(),
        )

    assert exc.value.code == 0


def test_entry_point_runs_conflict_check_after_singleton_check():
    with (
        patch("ltbox.main._prepare_environment", return_value=object()),
        patch("ltbox.main._setup_language", return_value="en"),
        patch("ltbox.main._ensure_admin_or_exit"),
        patch("ltbox.main._handle_conflicting_processes_once") as conflict_check,
        patch("ltbox.main._check_updates"),
        patch("ltbox.main._init_and_run"),
    ):
        main.entry_point()

    conflict_check.assert_called_once_with()


def test_entry_point_skips_conflict_check_when_another_instance_is_running():
    with (
        patch("ltbox.main._prepare_environment", return_value=None),
        patch("ltbox.main._setup_language", return_value="en"),
        patch("ltbox.main._handle_conflicting_processes_once") as conflict_check,
        patch("ltbox.main.ui.clear"),
        patch("ltbox.main.ui.error"),
        patch("builtins.input", return_value=""),
    ):
        with pytest.raises(SystemExit) as exc:
            main.entry_point()

    assert exc.value.code == 0
    conflict_check.assert_not_called()


def test_entry_point_exits_when_admin_required_check_fails():
    with (
        patch("ltbox.main._prepare_environment", return_value=object()),
        patch("ltbox.main._setup_language", return_value="en"),
        patch("ltbox.main._ensure_admin_or_exit", side_effect=SystemExit(0)),
        patch("ltbox.main._handle_conflicting_processes_once") as conflict_check,
        patch("ltbox.main._check_updates"),
        patch("ltbox.main._init_and_run"),
    ):
        with pytest.raises(SystemExit) as exc:
            main.entry_point()

    assert exc.value.code == 0
    conflict_check.assert_not_called()


def test_handle_conflicting_processes_once_exits_on_n():
    with (
        patch(
            "ltbox.main._get_running_processes",
            return_value=["adb.exe", "fastboot.exe"],
        ),
        patch("ltbox.main.ui.clear"),
        patch("ltbox.main.ui.warn"),
        patch("ltbox.main.ui.prompt", return_value="n"),
        patch("builtins.input", return_value=""),
        patch("ltbox.main._force_kill_processes") as kill_processes,
    ):
        with pytest.raises(SystemExit) as exc:
            main._handle_conflicting_processes_once()

    assert exc.value.code == 0
    kill_processes.assert_not_called()


def test_handle_conflicting_processes_once_kills_on_y():
    with (
        patch(
            "ltbox.main._get_running_processes",
            return_value=["adb.exe", "fastboot.exe"],
        ),
        patch("ltbox.main.ui.clear"),
        patch("ltbox.main.ui.warn"),
        patch("ltbox.main.ui.prompt", return_value="y"),
        patch("ltbox.main._force_kill_processes") as kill_processes,
    ):
        main._handle_conflicting_processes_once()

    kill_processes.assert_called_once_with(["adb.exe", "fastboot.exe"])
