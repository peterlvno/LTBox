from unittest.mock import MagicMock, patch

from ltbox.actions import root_workflow
from ltbox.actions.root_strategies import GkiRootStrategy, LkmRootStrategy


def test_root_workflow_session_uses_active_slot_for_partition_map():
    session = root_workflow.RootWorkflowSession(
        strategy=GkiRootStrategy(),
        gki=True,
        lkm_kernel_version=None,
    )

    with (
        patch("ltbox.actions.root_workflow.get_slot_suffix", return_value="_b"),
        patch("ltbox.actions.root_workflow.utils.ui.echo"),
    ):
        partition_map = session.resolve_partition_map(MagicMock())

    assert partition_map["main"] == "boot_b"
    assert partition_map["vbmeta"] == "vbmeta_b"


def test_root_workflow_session_falls_back_to_gki_boot_when_slot_missing():
    session = root_workflow.RootWorkflowSession(
        strategy=GkiRootStrategy(),
        gki=True,
        lkm_kernel_version=None,
    )

    with (
        patch("ltbox.actions.root_workflow.get_slot_suffix", return_value=""),
        patch("ltbox.actions.root_workflow.utils.ui.echo"),
    ):
        partition_map = session.resolve_partition_map(MagicMock())

    assert partition_map["main"] == "boot"
    assert partition_map["vbmeta"] == "vbmeta"


def test_root_workflow_session_falls_back_to_lkm_init_boot_when_slot_missing():
    session = root_workflow.RootWorkflowSession(
        strategy=LkmRootStrategy(),
        gki=False,
        lkm_kernel_version=None,
    )

    with (
        patch("ltbox.actions.root_workflow.get_slot_suffix", return_value=""),
        patch("ltbox.actions.root_workflow.utils.ui.echo"),
    ):
        partition_map = session.resolve_partition_map(MagicMock())

    assert partition_map["main"] == "init_boot"
    assert partition_map["vbmeta"] == "vbmeta"
