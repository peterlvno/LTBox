import zipfile
from unittest.mock import patch

from ltbox.actions.root.prompts import StrategySourceSelection
from ltbox.actions.root.strategies import (
    APatchStrategy,
    GkiRootStrategy,
    LkmRootStrategy,
)


def test_apatch_strategy_configure_source_applies_prompt_selection():
    selection = StrategySourceSelection(
        repo_config={"repo": "owner/apatch"},
        source_label="Nightly",
        is_nightly=True,
        workflow_id="12345",
    )
    strategy = APatchStrategy("apatch")

    with patch(
        "ltbox.actions.root.strategies.select_apatch_source",
        return_value=selection,
    ) as select_source:
        strategy.configure_source("main > root")

    select_source.assert_called_once_with("apatch", breadcrumbs="main > root")
    assert strategy.repo_config == {"repo": "owner/apatch"}
    assert strategy.source_label == "Nightly"
    assert strategy.is_nightly is True
    assert strategy.workflow_id == "12345"


def test_apatch_strategy_download_resources_uses_download_helper():
    strategy = APatchStrategy("folkpatch")
    strategy.repo_config = {"repo": "owner/folkpatch"}
    strategy.is_nightly = True
    strategy.workflow_id = "run-123"

    with patch(
        "ltbox.actions.root.strategies.download_apatch_resources",
        return_value=True,
    ) as download_resources:
        assert strategy.download_resources() is True

    download_resources.assert_called_once_with(
        profile=strategy.provider,
        staging_dir=strategy._staging_dir,
        repo_config={"repo": "owner/folkpatch"},
        is_nightly=True,
        workflow_id="run-123",
    )


def test_lkm_strategy_configure_source_applies_prompt_selection():
    selection = StrategySourceSelection(
        repo_config={"repo": "owner/ksu"},
        source_label="Release",
        is_nightly=False,
        workflow_id="",
        is_tagged_build=True,
    )
    strategy = LkmRootStrategy("kernelsu-next")

    with patch(
        "ltbox.actions.root.strategies.select_lkm_source",
        return_value=selection,
    ) as select_source:
        strategy.configure_source("main > root")

    select_source.assert_called_once_with("kernelsu-next", breadcrumbs="main > root")
    assert strategy.repo_config == {"repo": "owner/ksu"}
    assert strategy.source_label == "Release"
    assert strategy.is_nightly is False
    assert strategy.workflow_id == ""
    assert strategy.is_tagged_build is True


def test_lkm_strategy_download_resources_uses_download_helper():
    strategy = LkmRootStrategy("kernelsu-next")
    strategy.repo_config = {"repo": "owner/ksu-next"}
    strategy.is_nightly = False
    strategy.workflow_id = ""
    strategy.is_tagged_build = True

    with patch(
        "ltbox.actions.root.strategies.download_lkm_resources",
        return_value=True,
    ) as download_resources:
        assert strategy.download_resources("6.6.0") is True

    download_resources.assert_called_once_with(
        profile=strategy.provider,
        staging_dir=strategy.staging_dir,
        repo_config={"repo": "owner/ksu-next"},
        kernel_version="6.6.0",
        is_nightly=False,
        workflow_id="",
        is_tagged_build=True,
    )


def test_gki_strategy_configure_source_extracts_manager_apk(tmp_path):
    zip_path = tmp_path / "AnyKernel3.zip"
    tools_dir = tmp_path / "tools"
    tools_dir.mkdir()

    with zipfile.ZipFile(zip_path, "w") as archive:
        archive.writestr("Image", b"kernel")
        archive.writestr("nested/manager.apk", b"apk")

    strategy = GkiRootStrategy()

    with (
        patch(
            "ltbox.actions.root.strategies._prompt_custom_kernel_zip",
            return_value=zip_path,
        ),
        patch("ltbox.actions.root.strategies.const.TOOLS_DIR", tools_dir),
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        assert strategy.configure_source("main > root > GKI") is True

    assert strategy._kernel_zip == zip_path
    assert strategy.source_label == zip_path.name
    assert (tools_dir / "manager.apk").read_bytes() == b"apk"


def test_gki_strategy_download_resources_requires_selected_zip():
    strategy = GkiRootStrategy()

    with patch("ltbox.actions.root.strategies.utils.ui.warn") as warn:
        assert strategy.download_resources() is False

    warn.assert_called_once()
