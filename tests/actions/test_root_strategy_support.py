import zipfile
from unittest.mock import patch

from ltbox.actions.root.prompts import StrategySourceSelection
from ltbox.actions.root.prompts import select_lkm_source
from ltbox.actions.root.strategies import (
    APatchStrategy,
    GkiRootStrategy,
    LkmRootStrategy,
    MagiskRootStrategy,
    _prompt_custom_kernel_zip,
    _prompt_custom_magisk_apk,
)
from ltbox.menus import router as menu_router


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


def test_lkm_strategy_configure_source_returns_main(monkeypatch):
    strategy = LkmRootStrategy("kernelsu")

    monkeypatch.setattr(
        "ltbox.actions.root.strategies.select_lkm_source",
        lambda *_args, **_kwargs: "main",
    )

    assert strategy.configure_source("main > root") is menu_router.RouteResult.MAIN


def test_magisk_strategy_configure_source_applies_prompt_selection():
    selection = StrategySourceSelection(
        repo_config={"repo": "owner/magisk"},
        source_label="Nightly",
        is_nightly=True,
        workflow_id="run-777",
    )
    strategy = MagiskRootStrategy("magisk")

    with patch(
        "ltbox.actions.root.strategies.select_magisk_source",
        return_value=selection,
    ) as select_source:
        strategy.configure_source("main > root")

    select_source.assert_called_once_with("magisk", breadcrumbs="main > root")
    assert strategy.repo_config == {"repo": "owner/magisk"}
    assert strategy.source_label == "Nightly"
    assert strategy.workflow_id == "run-777"
    assert strategy.local_apk_path is None


def test_magisk_strategy_download_resources_uses_local_apk_helper(tmp_path):
    strategy = MagiskRootStrategy("other_forks")
    strategy.local_apk_path = tmp_path / "MagiskAlpha.apk"
    strategy.local_apk_path.write_bytes(b"apk")

    with patch(
        "ltbox.actions.root.strategies.download_magisk_resources",
        return_value=True,
    ) as download_resources:
        assert strategy.download_resources() is True

    download_resources.assert_called_once_with(
        profile=strategy.provider,
        staging_dir=strategy.staging_dir,
        repo_config={},
        is_nightly=False,
        workflow_id=None,
        local_apk_path=strategy.local_apk_path,
    )


def test_magisk_strategy_configure_source_uses_custom_apk_prompt_for_other_forks(
    tmp_path,
):
    strategy = MagiskRootStrategy("other_forks")
    apk_path = tmp_path / "MagiskAlpha.apk"
    apk_path.write_bytes(b"apk")

    with (
        patch(
            "ltbox.actions.root.strategies._prompt_custom_magisk_apk",
            return_value=apk_path,
        ),
        patch("ltbox.actions.root.strategies.cleanup_manager_apk") as cleanup,
    ):
        assert strategy.configure_source("main > root > Other forks") is True

    cleanup.assert_called_once_with()
    assert strategy.local_apk_path == apk_path
    assert strategy.source_label == apk_path.name
    assert strategy.is_nightly is False
    assert strategy.workflow_id is None


def test_prompt_custom_magisk_apk_uses_root_apk_when_present(tmp_path):
    magisk_dir = tmp_path / "magisk"
    magisk_dir.mkdir()
    apk_path = magisk_dir / "MagiskAlpha.apk"
    apk_path.write_bytes(b"apk")

    with (
        patch("ltbox.actions.root.strategies.const.BASE_DIR", tmp_path),
        patch("ltbox.actions.root.strategies.const.MAGISK_DIR", magisk_dir),
        patch("builtins.input", side_effect=AssertionError("input should not run")),
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        selected = _prompt_custom_magisk_apk()

    assert selected == apk_path


def test_prompt_custom_magisk_apk_allows_selection_when_multiple_exist(tmp_path):
    magisk_dir = tmp_path / "magisk"
    magisk_dir.mkdir()
    first_apk = magisk_dir / "alpha.apk"
    second_apk = magisk_dir / "beta.apk"
    first_apk.write_bytes(b"apk1")
    second_apk.write_bytes(b"apk2")

    with (
        patch("ltbox.menus.terminal.TerminalMenu") as terminal_menu,
        patch("ltbox.actions.root.strategies.const.BASE_DIR", tmp_path),
        patch("ltbox.actions.root.strategies.const.MAGISK_DIR", magisk_dir),
        patch("builtins.input", side_effect=AssertionError("input should not run")),
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        terminal_menu.return_value.ask.return_value = "2"
        selected = _prompt_custom_magisk_apk()

    assert selected == second_apk


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


def test_gki_strategy_configure_source_cleans_stale_manager_apk(tmp_path):
    zip_path = tmp_path / "kernel_bundle.zip"
    tools_dir = tmp_path / "tools"

    with zipfile.ZipFile(zip_path, "w") as archive:
        archive.writestr("Image", b"kernel")
        archive.writestr("Manager-1.0.0-release.APK", b"apk")

    strategy = GkiRootStrategy()

    with (
        patch(
            "ltbox.actions.root.strategies._prompt_custom_kernel_zip",
            return_value=zip_path,
        ),
        patch("ltbox.actions.root.strategies.const.TOOLS_DIR", tools_dir),
        patch("ltbox.actions.root.strategies.cleanup_manager_apk") as cleanup,
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        assert strategy.configure_source("main > root > GKI") is True

    cleanup.assert_called_once_with()
    assert (tools_dir / "manager.apk").read_bytes() == b"apk"


def test_gki_strategy_download_resources_requires_selected_zip():
    strategy = GkiRootStrategy()

    with patch("ltbox.actions.root.strategies.utils.ui.warn") as warn:
        assert strategy.download_resources() is False

    warn.assert_called_once()


def test_prompt_custom_kernel_zip_skips_wait_when_single_zip_exists(tmp_path):
    zip_path = tmp_path / "kernel" / "existing.zip"
    zip_path.parent.mkdir()
    zip_path.write_bytes(b"zip")

    with (
        patch("ltbox.actions.root.strategies.const.KERNEL_DIR", zip_path.parent),
        patch("builtins.input", side_effect=AssertionError("input should not run")),
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        selected = _prompt_custom_kernel_zip()

    assert selected == zip_path


def test_prompt_custom_kernel_zip_skips_wait_when_multiple_zips_exist(tmp_path):
    kernel_dir = tmp_path / "kernel"
    kernel_dir.mkdir()
    first_zip = kernel_dir / "alpha.zip"
    second_zip = kernel_dir / "beta.zip"
    first_zip.write_bytes(b"zip1")
    second_zip.write_bytes(b"zip2")

    with (
        patch("ltbox.menus.terminal.TerminalMenu") as terminal_menu,
        patch("ltbox.actions.root.strategies.const.KERNEL_DIR", kernel_dir),
        patch("builtins.input", side_effect=AssertionError("input should not run")),
        patch("ltbox.actions.root.strategies.utils.ui.echo"),
    ):
        terminal_menu.return_value.ask.return_value = "2"
        selected = _prompt_custom_kernel_zip()

    assert selected == second_zip


def test_select_lkm_source_force_nightly_back_returns_none(monkeypatch):
    monkeypatch.setattr(
        "ltbox.actions.root.prompts._load_provider_repo_config",
        lambda _profile: {"repo": "owner/resukisu", "workflow": "123"},
    )
    monkeypatch.setattr(
        "ltbox.actions.root.prompts.prompt_nightly_workflow",
        lambda *_args, **_kwargs: "back",
    )

    assert select_lkm_source("resukisu", breadcrumbs="main > root") is None


def test_select_lkm_source_force_nightly_main_returns_main(monkeypatch):
    monkeypatch.setattr(
        "ltbox.actions.root.prompts._load_provider_repo_config",
        lambda _profile: {"repo": "owner/resukisu", "workflow": "123"},
    )
    monkeypatch.setattr(
        "ltbox.actions.root.prompts.prompt_nightly_workflow",
        lambda *_args, **_kwargs: "main",
    )

    assert select_lkm_source("resukisu", breadcrumbs="main > root") == "main"
