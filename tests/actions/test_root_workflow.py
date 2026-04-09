from unittest.mock import MagicMock, patch

from ltbox.actions.root import workflow as root_workflow
from ltbox.actions.root.strategies import GkiRootStrategy, LkmRootStrategy


def test_root_workflow_session_uses_active_slot_for_partition_map():
    session = root_workflow.RootWorkflowSession(
        strategy=GkiRootStrategy(),
        gki=True,
        lkm_kernel_version=None,
    )

    with (
        patch("ltbox.actions.root.workflow.get_slot_suffix", return_value="_b"),
        patch("ltbox.actions.root.workflow.utils.ui.echo"),
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
        patch("ltbox.actions.root.workflow.get_slot_suffix", return_value=""),
        patch("ltbox.actions.root.workflow.utils.ui.echo"),
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
        patch("ltbox.actions.root.workflow.get_slot_suffix", return_value=""),
        patch("ltbox.actions.root.workflow.utils.ui.echo"),
    ):
        partition_map = session.resolve_partition_map(MagicMock())

    assert partition_map["main"] == "init_boot"
    assert partition_map["vbmeta"] == "vbmeta"


def test_root_device_skips_cleanup_for_preconfigured_gki_strategy():
    strategy = GkiRootStrategy()
    strategy._kernel_zip = MagicMock()
    session = MagicMock()
    session.strategy = strategy
    session.gki = True
    session.lkm_kernel_version = None
    session.resolve_partition_map.return_value = {"main": "boot", "vbmeta": "vbmeta"}
    dev = MagicMock()

    with (
        patch("ltbox.actions.root.workflow.cleanup_manager_apk") as cleanup,
        patch("ltbox.actions.root.workflow._resolve_strategy", return_value=strategy),
        patch("ltbox.actions.root.workflow._prepare_root_env"),
        patch(
            "ltbox.actions.root.workflow._create_root_workflow_session",
            return_value=session,
        ),
        patch.object(strategy, "download_resources", return_value=True),
        patch(
            "ltbox.actions.root.workflow._install_manager_apk",
            return_value=True,
        ),
        patch("ltbox.actions.root.workflow._generate_root_image"),
        patch("ltbox.actions.root.workflow._flash_root_image"),
        patch("ltbox.actions.root.workflow.utils.ui.echo"),
        patch("ltbox.logger.console", MagicMock(width=80)),
    ):
        root_workflow.root_device(dev, gki=True, root_type="gki", strategy=strategy)

    cleanup.assert_not_called()
