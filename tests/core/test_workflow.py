import contextlib

import pytest
from unittest.mock import MagicMock, patch

from ltbox import workflow
from ltbox.errors import LTBoxError, UserCancelError


def test_patch_all_flow_standard(mock_env):
    mock_dev = MagicMock()
    mock_dev.skip_adb = False
    mock_dev.detect_active_slot.return_value = "_a"
    mock_dev.adb.get_model.return_value = "TestModel"

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        mock_actions.read_anti_rollback.return_value = ("OK", "PASS")

        workflow.patch_all(dev=mock_dev, wipe=0, target_region="PRC")

        mock_actions.convert_region_images.assert_called_once()

        mock_actions.dump_partitions.assert_called_once()

        mock_actions.read_anti_rollback.assert_called_once()

        mock_actions.flash_full_firmware.assert_called_once()


def test_patch_all_skip_arb():
    mock_dev = MagicMock()
    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        workflow.patch_all(dev=mock_dev, skip_rollback=True)

        mock_actions.read_anti_rollback.assert_not_called()


def test_patch_all_keyboard_interrupt_is_mapped_to_user_cancel():
    mock_dev = MagicMock()

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
    mock_dev = MagicMock()

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
    mock_dev = MagicMock()

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
