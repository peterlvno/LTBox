from dataclasses import dataclass
from pathlib import Path
from typing import Protocol, Sequence

from ..backup_sources import format_dp_folder_label
from ..i18n import get_string
from ..ui import ui
from .prompt_helpers import prompt_index_selection, prompt_yes_no


@dataclass(frozen=True)
class BackupChoice:
    force_dump: bool = False
    selected_dir: Path | None = None
    skip_all: bool = False


class WorkflowPrompts(Protocol):
    def choose_backup_source(self, backup_dirs: Sequence[Path]) -> BackupChoice: ...

    def confirm(self, message: str) -> bool: ...


class UiWorkflowPrompts:
    def choose_backup_source(self, backup_dirs: Sequence[Path]) -> BackupChoice:
        if not backup_dirs:
            return BackupChoice(force_dump=False, selected_dir=None)

        ui.clear()
        ui.echo(get_string("wf_backup_critical_found"))
        ui.echo("")

        width = ui.get_term_width()
        ui.echo("=" * width)
        for index, folder in enumerate(backup_dirs, 1):
            ui.echo(f"  {index}. {format_dp_folder_label(folder)}")
        dump_option = len(backup_dirs) + 1
        skip_option = dump_option + 1
        ui.echo(f"  {dump_option}. {get_string('wf_backup_critical_dump')}")
        ui.echo(f"  {skip_option}. {get_string('wf_backup_critical_skip')}")
        ui.echo("=" * width)
        ui.echo("")

        selected_index = prompt_index_selection(
            get_string("prompt_select"),
            max_index=skip_option,
            error_message=get_string("err_invalid_selection"),
            input_func=ui.prompt,
            error_func=ui.error,
        )

        ui.clear()
        if selected_index == dump_option:
            return BackupChoice(force_dump=True)
        if selected_index == skip_option:
            return BackupChoice(skip_all=True)
        return BackupChoice(selected_dir=backup_dirs[selected_index - 1])

    def confirm(self, message: str) -> bool:
        result = prompt_yes_no(
            message + " (y/n) ",
            input_func=ui.prompt,
            error_message=get_string("err_invalid_selection"),
            error_func=ui.error,
        )
        return bool(result)
