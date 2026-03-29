import contextlib
import pytest

from unittest.mock import patch
from pathlib import Path
from ltbox.execution import TaskResult
from ltbox import workflow
from ltbox.actions.arb import ArbStatus
from ltbox.context import TaskContext
from ltbox.errors import LTBoxError, UserCancelError
from ltbox.workflow_prompts import BackupChoice
from tests.helpers import make_device_mock


def test_patch_all_flow_with_stored_rollback_indices(mock_env):
    mock_dev = make_device_mock(
        stored_rollback_indices={2: 0x41B7A200, 3: 0x41B7A200, 1: 1, 0: 0},
    )

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
        patch(
            "ltbox.workflow.check_image_folder_arb",
            return_value=(ArbStatus.MATCH, 0x41B7A200, 0x41B7A200),
        ) as mock_check,
    ):
        workflow.patch_all(
            dev=mock_dev, wipe=0, target_region="PRC", modify_region_code=True
        )

        mock_actions.convert_region_images.assert_called_once()
        mock_check.assert_called_once_with(0x41B7A200, "ON")
        mock_actions.flash_full_firmware.assert_called_once()


def test_patch_all_flow_no_stored_indices_skips_arb(mock_env):
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
        patch("ltbox.workflow.check_image_folder_arb") as mock_check,
    ):
        workflow.patch_all(
            dev=mock_dev, wipe=0, target_region="PRC", modify_region_code=True
        )

        mock_check.assert_not_called()
        mock_actions.read_anti_rollback.assert_not_called()
        mock_actions.patch_anti_rollback.assert_not_called()
        mock_actions.flash_full_firmware.assert_called_once()


def test_patch_all_passes_modify_region_code_flag():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        workflow.patch_all(dev=mock_dev, modify_region_code=False)

        assert (
            mock_actions.convert_region_images.call_args.kwargs["modify_region_code"]
            is False
        )


def test_patch_all_wipe_passes_wipe_flag_to_flash():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        workflow.patch_all(dev=mock_dev, wipe=1)

        assert mock_actions.flash_full_firmware.call_args.kwargs["wipe"] is True


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


def test_patch_all_skip_arb_when_no_stored_indices():
    mock_dev = make_device_mock()
    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        workflow.patch_all(dev=mock_dev, modify_rollback_index="AUTO")

        mock_actions.read_anti_rollback.assert_not_called()
        mock_actions.patch_anti_rollback.assert_not_called()


def test_patch_all_tb320fc_uses_edl_fallback():
    mock_dev = make_device_mock(model="TB320FC")

    with (
        patch("ltbox.workflow.actions") as mock_actions,
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow._wait_for_input_images"),
        patch("ltbox.workflow._cleanup_previous_outputs"),
    ):
        mock_actions.read_anti_rollback.return_value = (ArbStatus.MATCH, 0, 0)

        workflow.patch_all(dev=mock_dev, modify_rollback_index="AUTO")

        mock_actions.dump_partitions.assert_called_once()
        call_kwargs = mock_actions.dump_partitions.call_args.kwargs
        assert "boot_a" in call_kwargs["additional_targets"]
        assert "vbmeta_system_a" in call_kwargs["additional_targets"]

        mock_actions.read_anti_rollback.assert_called_once()


def test_check_backup_critical_uses_injected_prompt_service(tmp_path):
    mock_dev = make_device_mock()
    backup_dir = tmp_path / "backup_critical_20260101"
    backup_dir.mkdir()
    (backup_dir / "devinfo.img").write_bytes(b"devinfo")
    output_dp_dir = tmp_path / "output_dp"

    class PromptStub:
        def choose_backup_source(self, backup_dirs):
            assert list(backup_dirs) == [backup_dir]
            return BackupChoice(selected_dir=backup_dir)

        def confirm(self, message: str) -> bool:
            raise AssertionError(f"confirm should not be called: {message}")

    ctx = TaskContext(
        dev=mock_dev,
        modify_region_code=False,
        on_log=lambda _message: None,
        prompts=PromptStub(),
    )

    with patch.multiple(
        "ltbox.workflow.const",
        BASE_DIR=tmp_path,
        OUTPUT_DP_DIR=output_dp_dir,
    ):
        workflow._check_backup_critical(ctx)

    assert ctx.use_backup_dp is True
    assert ctx.backup_dir_name == backup_dir.name
    assert (output_dp_dir / "devinfo.img").read_bytes() == b"devinfo"


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


def test_patch_all_can_run_under_outer_task_executor():
    mock_dev = make_device_mock()

    with (
        patch("ltbox.workflow.utils.ui"),
        patch("ltbox.workflow.logging_context") as logging_context,
        patch("ltbox.workflow._build_steps", return_value=[]),
        patch("ltbox.workflow._run_steps"),
    ):
        result = workflow.patch_all(dev=mock_dev, manage_execution=False)

    logging_context.assert_not_called()
    assert isinstance(result, TaskResult)
    assert result.messages


def test_populate_device_info_sets_context_fields():
    mock_dev = make_device_mock(
        model="TB350XU",
        active_slot="_b",
        serialno="KW583P4R",
        stored_rollback_indices={2: 0x41B7A200, 3: 0x41B7A200, 1: 1, 0: 0},
    )

    ctx = TaskContext(dev=mock_dev, on_log=lambda _: None)
    workflow._populate_device_info(ctx)

    assert ctx.device_model == "TB350XU"
    assert ctx.active_slot_suffix == "_b"
    assert ctx.serialno == "KW583P4R"
    assert ctx.device_rollback_index == 0x41B7A200
