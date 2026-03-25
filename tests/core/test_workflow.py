import contextlib
import pytest

from unittest.mock import patch
from pathlib import Path
from ltbox import workflow
from ltbox.errors import LTBoxError, UserCancelError
from tests.helpers import make_device_mock


def test_patch_all_flow_standard(mock_env):
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        mock_actions.read_anti_rollback.return_value = ("MATCH", 0, 0)

        workflow.patch_all(
            dev=mock_dev, wipe=0, target_region="PRC", modify_region_code=True
        )

        mock_actions.convert_region_images.assert_called_once()

        mock_actions.dump_partitions.assert_called_once()

        mock_actions.read_anti_rollback.assert_called_once()

        mock_actions.flash_full_firmware.assert_called_once()


def test_patch_all_passes_modify_region_code_flag():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        mock_actions.read_anti_rollback.return_value = ("MATCH", 0, 0)

        workflow.patch_all(dev=mock_dev, modify_region_code=False)

        assert (
            mock_actions.convert_region_images.call_args.kwargs["modify_region_code"]
            is False
        )


def test_patch_all_writes_flash_log_under_log_directory(tmp_path):
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow.const.BASE_DIR", tmp_path),
        patch("ltbox.workflow._build_steps", return_value=[]),
        patch("ltbox.workflow._run_steps"),
        patch(
            "ltbox.workflow.logging_context", return_value=contextlib.nullcontext()
        ) as mock_logging_context,
    ):
        workflow.patch_all(dev=mock_dev)

    log_file = Path(mock_logging_context.call_args.args[0])
    assert log_file.parent == tmp_path / "log"
    assert log_file.name.startswith("log_flash_firmware_")
    assert log_file.suffix == ".txt"


def test_patch_all_skip_arb_when_device_has_no_arb():
    mock_dev = make_device_mock()
    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        mock_actions.read_anti_rollback.return_value = ("MATCH", 0, 0)

        workflow.patch_all(dev=mock_dev)

        mock_actions.read_anti_rollback.assert_called_once()
        mock_actions.patch_anti_rollback.assert_not_called()


def test_patch_all_keyboard_interrupt_is_mapped_to_user_cancel():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow.logging_context", return_value=contextlib.nullcontext()),
        patch("ltbox.workflow._build_steps", return_value=[]),
        patch("ltbox.workflow._run_steps", side_effect=KeyboardInterrupt),
        patch("ltbox.workflow._log_workflow_halt") as log_halt,
    ):
        with pytest.raises(UserCancelError):
            workflow.patch_all(dev=mock_dev)

    log_halt.assert_called_once()


def test_patch_all_system_exit_is_mapped_to_ltbox_error():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow.logging_context", return_value=contextlib.nullcontext()),
        patch("ltbox.workflow._build_steps", return_value=[]),
        patch("ltbox.workflow._run_steps", side_effect=SystemExit(7)),
        patch("ltbox.workflow._log_workflow_halt") as log_halt,
    ):
        with pytest.raises(LTBoxError):
            workflow.patch_all(dev=mock_dev)

    log_halt.assert_called_once()


def test_patch_all_domain_errors_are_reraised_and_halt_logged():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow.logging_context", return_value=contextlib.nullcontext()),
        patch("ltbox.workflow._build_steps", return_value=[]),
        patch("ltbox.workflow._run_steps", side_effect=RuntimeError("boom")),
        patch("ltbox.workflow._log_workflow_halt") as log_halt,
    ):
        with pytest.raises(RuntimeError, match="boom"):
            workflow.patch_all(dev=mock_dev)

    log_halt.assert_called_once()
