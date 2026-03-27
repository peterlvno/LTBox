from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

from .. import constants as const, downloader, utils
from ..i18n import get_string
from ..menu import TerminalMenu

NIGHTLY_WORKFLOW_FILES: Dict[str, str] = {
    "kernelsu": "build-manager.yml",
    "ksu": "build-manager.yml",
    "kernelsu-next": "build-manager-ci.yml",
    "sukisu": "build-manager.yml",
    "resukisu": "build-manager.yml",
    "apatch": "build.yml",
    "folkpatch": "build-debug.yml",
}


@dataclass(frozen=True)
class StrategySourceSelection:
    repo_config: Dict[str, Any]
    source_label: str
    is_nightly: bool
    workflow_id: Optional[str]
    is_tagged_build: bool = False


def prompt_kpm_selection(kpm_files: List[Path]) -> List[Path]:
    selected: Set[int] = set()

    while True:
        utils.ui.clear()
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

        choice = utils.ui.prompt(get_string("prompt_select")).strip().lower()
        if choice == "f":
            return [kpm_files[index] for index in sorted(selected)]
        if choice == "c":
            return []
        if choice == "a":
            selected = set(range(len(kpm_files)))
            continue
        if choice == "d":
            selected.clear()
            continue

        try:
            index = int(choice)
        except ValueError:
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        if not 1 <= index <= len(kpm_files):
            utils.ui.error(get_string("err_invalid_selection"))
            input(get_string("press_enter_to_continue"))
            continue

        offset = index - 1
        if offset in selected:
            selected.remove(offset)
        else:
            selected.add(offset)


def prompt_nightly_workflow(
    root_name: str,
    repo: str,
    workflow_file: str,
    default_id: str,
    breadcrumbs: Optional[str] = None,
) -> str:
    menu = TerminalMenu(
        get_string("prompt_workflow_source_title"),
        breadcrumbs=breadcrumbs,
    )
    menu.add_option(
        "1",
        get_string("prompt_workflow_retrieve_latest").format(file=workflow_file),
    )
    menu.add_option("2", get_string("prompt_workflow_manual_input"))
    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))

    if choice == "1" and repo and workflow_file:
        utils.ui.clear()
        utils.ui.echo(get_string("prompt_workflow_retrieving"))
        try:
            run_id = downloader.get_latest_successful_workflow_run(repo, workflow_file)
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


def select_apatch_source(
    root_type: str,
    breadcrumbs: Optional[str] = None,
) -> StrategySourceSelection:
    settings = const.load_settings_raw()
    repo_config = settings.get(root_type, {})
    source_name = "FolkPatch" if root_type == "folkpatch" else "APatch"

    menu = TerminalMenu(
        get_string("menu_root_subtype_title").format(name=source_name),
        breadcrumbs=breadcrumbs,
    )
    menu.add_option("1", get_string("menu_root_subtype_release"))
    menu.add_option("2", get_string("menu_root_subtype_nightly"))
    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))

    if choice == "2":
        return StrategySourceSelection(
            repo_config=repo_config,
            source_label=get_string("menu_root_subtype_nightly"),
            is_nightly=True,
            workflow_id=prompt_nightly_workflow(
                source_name,
                repo_config.get("repo", ""),
                NIGHTLY_WORKFLOW_FILES.get(root_type, ""),
                str(repo_config.get("workflow", "")).strip(),
                breadcrumbs,
            ),
        )

    return StrategySourceSelection(
        repo_config=repo_config,
        source_label=get_string("menu_root_subtype_release"),
        is_nightly=False,
        workflow_id=None,
    )


def select_lkm_source(
    root_type: str,
    breadcrumbs: Optional[str] = None,
) -> StrategySourceSelection:
    settings = const.load_settings_raw()

    if root_type == "kernelsu":
        repo_config = settings.get("kernelsu", {})
        root_name = "KernelSU"
    elif root_type == "sukisu":
        repo_config = settings.get("sukisu-ultra", {})
        root_name = "SukiSU Ultra"
    elif root_type == "resukisu":
        repo_config = settings.get("resukisu", {})
        root_name = "ReSukiSU"
    else:
        repo_config = settings.get("kernelsu-next", {})
        root_name = "KernelSU Next"

    resolved_breadcrumbs = breadcrumbs or get_string("menu_root_type_title")
    repo = repo_config.get("repo", "")
    workflow_file = NIGHTLY_WORKFLOW_FILES.get(root_type, "")

    if root_type == "resukisu":
        return StrategySourceSelection(
            repo_config=repo_config,
            source_label=get_string("menu_root_subtype_nightly"),
            is_nightly=True,
            workflow_id=prompt_nightly_workflow(
                root_name,
                repo,
                workflow_file,
                str(repo_config.get("workflow", "")),
                resolved_breadcrumbs,
            ),
            is_tagged_build=False,
        )

    menu = TerminalMenu(
        get_string("menu_root_subtype_title").format(name=root_name),
        breadcrumbs=resolved_breadcrumbs,
    )
    menu.add_option("1", get_string("menu_root_subtype_release"))
    menu.add_option("2", get_string("menu_root_subtype_nightly"))
    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))

    if choice == "2":
        return StrategySourceSelection(
            repo_config=repo_config,
            source_label=get_string("menu_root_subtype_nightly"),
            is_nightly=True,
            workflow_id=prompt_nightly_workflow(
                root_name,
                repo,
                workflow_file,
                str(repo_config.get("workflow", "")),
                resolved_breadcrumbs,
            ),
            is_tagged_build=False,
        )

    return StrategySourceSelection(
        repo_config=repo_config,
        source_label=get_string("menu_root_subtype_release"),
        is_nightly=False,
        workflow_id="",
        is_tagged_build=True,
    )


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
