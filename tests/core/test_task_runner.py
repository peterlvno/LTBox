import subprocess
from contextlib import nullcontext
from unittest.mock import patch

import pytest

from ltbox.registry import CommandRegistry
from ltbox.errors import ToolError
from ltbox.task_runner import run_task
from tests.helpers import make_device_mock


def test_run_task_raises_for_unknown_command():
    with pytest.raises(ToolError):
        run_task("unknown", None, CommandRegistry())


def test_run_task_handles_called_process_error_and_cleans_up():
    registry = CommandRegistry()

    def failing_cmd(dev):
        raise subprocess.CalledProcessError(
            returncode=1,
            cmd=["fake", "cmd"],
            output="stdout-data",
            stderr="stderr-data",
        )

    registry.add("fail", failing_cmd, "Fail Task")
    dev = make_device_mock()

    with (
        patch("ltbox.task_runner.logging_context", return_value=nullcontext()),
        patch("ltbox.task_runner.ui.box_output") as mock_box,
        patch("builtins.input", return_value=""),
    ):
        run_task("fail", dev, registry)

    assert mock_box.called
    dev.adb.force_kill_server.assert_called_once()
    dev.fastboot.force_kill_server.assert_called_once()


def test_run_task_unhandled_exception_bubbles_up():
    registry = CommandRegistry()

    def crash_cmd(dev):
        raise ZeroDivisionError("boom")

    registry.add("crash", crash_cmd, "Crash Task")
    dev = make_device_mock()

    with (
        patch("ltbox.task_runner.logging_context", return_value=nullcontext()),
        patch("builtins.input", return_value=""),
    ):
        with pytest.raises(ZeroDivisionError):
            run_task("crash", dev, registry)

    dev.adb.force_kill_server.assert_called_once()
    dev.fastboot.force_kill_server.assert_called_once()
