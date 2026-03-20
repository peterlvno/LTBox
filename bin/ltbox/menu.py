from typing import List, Optional, Tuple

from .i18n import get_string
from .menu_data import MenuItem
from .utils import ui

SEPARATOR_TEXT = "-" * 12

try:
    import questionary
    from questionary import Choice, Separator
except ImportError:
    questionary = None  # type: ignore


class TerminalMenu:
    def __init__(self, title: str, breadcrumbs: Optional[str] = None):
        self.title = title
        self.breadcrumbs = breadcrumbs
        self.options: List[Tuple[Optional[str], str, bool]] = []
        self.valid_keys: List[str] = []

    def add_option(self, key: str, text: str) -> None:
        self.options.append((key, text, True))
        self.valid_keys.append(key.lower())

    def add_label(self, text: str) -> None:
        self.options.append((None, text, False))

    def add_separator(self) -> None:
        self.options.append((None, "", False))

    def populate(self, items: List[MenuItem]) -> None:
        for item in items:
            if item.item_type == "label":
                self.add_label(item.text)
            elif item.item_type == "separator":
                self.add_separator()
            elif item.item_type == "option" and item.key is not None:
                self.add_option(str(item.key), item.text)

    def _display_title(self) -> str:
        return f"{self.breadcrumbs} > {self.title}" if self.breadcrumbs else self.title

    def _render_header(self) -> None:
        width = ui.get_term_width()
        ui.clear()
        ui.echo("\n" + "=" * width)
        ui.echo(f"   {self._display_title()}")
        ui.echo("=" * width + "\n")

    def show(self) -> None:
        self._render_header()

        for key, text, is_selectable in self.options:
            if is_selectable:
                ui.echo(f"   {key}. {text}")
            else:
                if text:
                    ui.echo(f"  {text}")
                else:
                    ui.echo(f"   {SEPARATOR_TEXT}")

        width = ui.get_term_width()
        ui.echo("\n" + "=" * width + "\n")

    def ask(self, prompt_msg: str, error_msg: str) -> str:
        if questionary:
            self._render_header()

            choices = [Separator(" ")]
            for key, text, is_selectable in self.options:
                if is_selectable and key is not None:
                    choices.append(Choice(f"{key}. {text}", value=key.lower()))
                else:
                    display_text = f"  {text}" if text else f"   {SEPARATOR_TEXT}"
                    choices.append(Separator(display_text))

            answer = questionary.select(
                prompt_msg.strip(),
                choices=choices,
                qmark=">",
                pointer="->",
                instruction=get_string("prompt_use_arrow_keys"),
            ).ask()

            if answer is not None:
                return answer
            raise KeyboardInterrupt()

        while True:
            self.show()
            choice = input(prompt_msg).strip().lower()
            if choice in self.valid_keys:
                return choice

            ui.echo(error_msg)
            input(get_string("press_enter_to_continue"))


def select_menu_action(
    menu_items: List[MenuItem], title_key: str, breadcrumbs: Optional[str] = None
) -> Optional[str]:
    menu = TerminalMenu(get_string(title_key), breadcrumbs)
    menu.populate(menu_items)

    action_map = {
        item.key: item.action for item in menu_items if item.item_type == "option"
    }

    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))
    return action_map.get(choice)
