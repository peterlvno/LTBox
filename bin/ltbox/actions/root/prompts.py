from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from ... import constants as const, downloader, utils
from ...i18n import get_string
from ...menus.prompt_helpers import prompt_multi_select_indices
from ...menus.terminal import TerminalMenu
from ...root_profiles import (
    RootProviderFamily,
    RootProviderProfile,
    get_root_provider_profile,
)


@dataclass(frozen=True)
class StrategySourceSelection:
    repo_config: Dict[str, Any]
    source_label: str
    is_nightly: bool
    workflow_id: Optional[str]
    is_tagged_build: bool = False


def prompt_kpm_selection(kpm_files: List[Path]) -> List[Path]:
    def _render(selected: Set[int]) -> None:
        width = utils.ui.get_term_width()
        utils.ui.echo("\n" + "=" * width)
        utils.ui.echo(f"   {get_string('apatch_kpm_select_title')}")
        utils.ui.echo("=" * width + "\n")

        for index, kpm_file in enumerate(kpm_files):
            mark = " [v]" if index in selected else ""
            utils.ui.echo(f"  {index + 1:3d}. {kpm_file.name}{mark}")

        utils.ui.echo("")
        utils.ui.echo(f"   a. {get_string('apatch_kpm_select_all')}")
        utils.ui.echo(f"   d. {get_string('apatch_kpm_deselect_all')}")
        utils.ui.echo(f"   f. {get_string('apatch_kpm_select_done')}")
        utils.ui.echo(f"   c. {get_string('cancel')}")
        utils.ui.echo("\n" + "=" * width + "\n")

    selected = prompt_multi_select_indices(
        get_string("prompt_select"),
        item_count=len(kpm_files),
        render_func=_render,
        input_func=utils.ui.prompt,
        error_message=get_string("err_invalid_selection"),
        error_func=utils.ui.error,
        pause_func=lambda: input(get_string("press_enter_to_continue")),
        clear_func=utils.ui.clear,
        select_all_choice="a",
        deselect_all_choice="d",
    )
    if selected is None:
        return []
    return [kpm_files[index] for index in selected]


def prompt_nightly_workflow(
    root_name: str,
    repo: str,
    workflow_file: str,
    default_id: str,
    breadcrumbs: Optional[str] = None,
    branch: Optional[str] = None,
) -> Optional[str]:
    menu = TerminalMenu(
        get_string("prompt_workflow_source_title"),
        breadcrumbs=breadcrumbs,
    )
    menu.add_option(
        "1",
        get_string("prompt_workflow_retrieve_latest").format(file=workflow_file),
    )
    menu.add_option("2", get_string("prompt_workflow_manual_input"))
    menu.add_separator()
    menu.add_option("b", get_string("menu_back"))
    menu.add_option("m", get_string("menu_root_m"))
    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))

    if choice == "b" or choice is None:
        return "back"
    if choice == "m":
        return "main"

    if choice == "1" and repo and workflow_file:
        utils.ui.clear()
        utils.ui.echo(get_string("prompt_workflow_retrieving"))
        try:
            run_id = downloader.get_latest_successful_workflow_run(
                repo, workflow_file, branch=branch
            )
            if run_id:
                utils.ui.info(get_string("prompt_workflow_retrieved").format(id=run_id))
                input(get_string("press_enter_to_continue"))
                return run_id
            utils.ui.error(get_string("prompt_workflow_retrieve_failed"))
        except Exception:
            utils.ui.error(get_string("prompt_workflow_retrieve_failed"))

    utils.ui.clear()
    display_id = default_id if default_id else get_string("act_root_auto_detect")
    width = utils.ui.get_term_width()
    utils.ui.echo("-" * width)
    utils.ui.echo(get_string("prompt_workflow_id").format(name=root_name))
    utils.ui.echo(get_string("prompt_workflow_default").format(id=display_id))
    utils.ui.echo("-" * width)
    value = input(get_string("prompt_input_arrow")).strip()
    if not value:
        return default_id
    return value


def _load_provider_repo_config(profile: RootProviderProfile) -> Dict[str, Any]:
    settings = const.load_settings_raw()
    return settings.get(profile.settings_key, {})


def _build_source_selection(
    *,
    profile: RootProviderProfile,
    repo_config: Dict[str, Any],
    is_nightly: bool,
    workflow_id: Optional[str],
) -> StrategySourceSelection:
    source_label_key = (
        "menu_root_subtype_nightly" if is_nightly else "menu_root_subtype_release"
    )
    release_workflow_id = workflow_id
    if not is_nightly and profile.family == RootProviderFamily.LKM:
        release_workflow_id = ""

    return StrategySourceSelection(
        repo_config=repo_config,
        source_label=get_string(source_label_key),
        is_nightly=is_nightly,
        workflow_id=release_workflow_id,
        is_tagged_build=profile.release_uses_tagged_build and not is_nightly,
    )


def _select_profile_source(
    profile: RootProviderProfile,
    breadcrumbs: Optional[str] = None,
) -> Optional[StrategySourceSelection]:
    repo_config = _load_provider_repo_config(profile)
    resolved_breadcrumbs = breadcrumbs or get_string("menu_root_type_title")
    repo = repo_config.get("repo", "")
    default_workflow = str(repo_config.get("workflow", "")).strip()

    while True:
        if profile.force_nightly:
            workflow_id = prompt_nightly_workflow(
                profile.display_name,
                repo,
                profile.workflow_file,
                default_workflow,
                resolved_breadcrumbs,
                branch=profile.nightly_branch,
            )
            if workflow_id in (None, "back"):
                return None
            if workflow_id == "main":
                return "main"  # type: ignore[return-value]

            return _build_source_selection(
                profile=profile,
                repo_config=repo_config,
                is_nightly=True,
                workflow_id=workflow_id,
            )

        menu = TerminalMenu(
            get_string("menu_root_subtype_title").format(name=profile.display_name),
            breadcrumbs=resolved_breadcrumbs,
        )
        menu.add_option("1", get_string("menu_root_subtype_release"))
        menu.add_option("2", get_string("menu_root_subtype_nightly"))
        menu.add_separator()
        menu.add_option("b", get_string("menu_back"))
        menu.add_option("m", get_string("menu_root_m"))
        choice = menu.ask(
            get_string("prompt_select"), get_string("err_invalid_selection")
        )

        if choice == "b" or choice is None:
            return None
        if choice == "m":
            return "main"  # type: ignore

        if choice == "2":
            workflow_id = prompt_nightly_workflow(
                profile.display_name,
                repo,
                profile.workflow_file,
                default_workflow,
                resolved_breadcrumbs,
                branch=profile.nightly_branch,
            )
            if workflow_id == "back" or workflow_id is None:
                continue
            if workflow_id == "main":
                return "main"  # type: ignore

            return _build_source_selection(
                profile=profile,
                repo_config=repo_config,
                is_nightly=True,
                workflow_id=workflow_id,
            )

        return _build_source_selection(
            profile=profile,
            repo_config=repo_config,
            is_nightly=False,
            workflow_id=None,
        )


def select_apatch_source(
    root_type: str,
    breadcrumbs: Optional[str] = None,
) -> Optional[StrategySourceSelection]:
    profile = get_root_provider_profile(root_type)
    if profile.family != RootProviderFamily.APATCH:
        raise ValueError(f"Expected an APatch-family provider, got: {root_type}")
    return _select_profile_source(profile, breadcrumbs)


def select_magisk_source(
    root_type: str,
    breadcrumbs: Optional[str] = None,
) -> Optional[StrategySourceSelection]:
    profile = get_root_provider_profile(root_type)
    if profile.family != RootProviderFamily.MAGISK:
        raise ValueError(f"Expected a Magisk-family provider, got: {root_type}")
    return _select_profile_source(profile, breadcrumbs)


def select_lkm_source(
    root_type: str,
    breadcrumbs: Optional[str] = None,
) -> Optional[StrategySourceSelection]:
    profile = get_root_provider_profile(root_type)
    if profile.family != RootProviderFamily.LKM:
        raise ValueError(f"Expected an LKM provider, got: {root_type}")
    return _select_profile_source(profile, breadcrumbs)


def prompt_apatch_superkey(source_name: str) -> str:
    utils.ui.clear()
    utils.ui.echo(
        "\n" + get_string("apatch_superkey_requirement").format(name=source_name)
    )

    while True:
        superkey = input(get_string("apatch_enter_superkey")).strip()
        if 8 <= len(superkey) <= 63 and superkey.isalnum():
            return superkey
        utils.ui.error(get_string("apatch_superkey_invalid"))


def prompt_embed_kpm() -> bool:
    choice = ""
    while choice not in ("y", "n"):
        choice = input(get_string("apatch_kpm_ask_embed")).strip().lower()
    return choice == "y"


def wait_for_kpm_files(kpm_dir: Path) -> List[Path]:
    try:
        while True:
            input(get_string("press_enter_to_continue"))
            kpm_files = sorted(kpm_dir.glob("*.kpm"))
            if kpm_files:
                return kpm_files
            utils.ui.error(get_string("apatch_kpm_no_files_found").format(path=kpm_dir))
    except KeyboardInterrupt:
        utils.ui.echo("")
        return []
