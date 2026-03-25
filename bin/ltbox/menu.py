from typing import Any, Callable, Dict, List, Optional, Tuple, Union

from .i18n import get_string
from .menu_data import MenuItem
from .utils import ui

SEPARATOR_TEXT = "-" * 12
_REFRESH_SENTINEL = "__auto_refresh__"

_LOGO_LINES = [
    "██╗  ████████╗██████╗  ██████╗ ██╗  ██╗",
    "██║  ╚══██╔══╝██╔══██╗██╔═══██╗╚██╗██╔╝",
    "██║     ██║   ██████╔╝██║   ██║ ╚███╔╝ ",
    "██║     ██║   ██╔══██╗██║   ██║ ██╔██╗ ",
    "███████╗██║   ██████╔╝╚██████╔╝██╔╝ ██╗",
    "╚══════╝╚═╝   ╚═════╝  ╚═════╝ ╚═╝  ╚═╝",
]

try:
    import questionary
    from questionary import Choice, Separator
except ImportError:
    questionary = None  # type: ignore


class _LiveStatusText:
    """Text object whose __format__ returns fresh status on each render."""

    def __init__(
        self,
        status_fn: Callable[[], str],
        status_key_fn: Optional[Callable[[], str]] = None,
        initial_key: Optional[str] = None,
    ):
        self._status_fn = status_fn
        self._status_key_fn = status_key_fn
        self._initial_key = initial_key

    def __format__(self, spec: str) -> str:
        if self._status_key_fn is not None:
            if self._status_key_fn() != self._initial_key:
                try:
                    from prompt_toolkit.application import get_app

                    get_app().exit(result=_REFRESH_SENTINEL)
                except Exception:
                    pass
        return f"■ {self._status_fn()}"

    def __str__(self) -> str:
        return f"■ {self._status_fn()}"


class TerminalMenu:
    def __init__(
        self,
        title: str,
        breadcrumbs: Optional[str] = None,
        status_fn: Optional[Callable[[], str]] = None,
        status_key_fn: Optional[Callable[[], str]] = None,
    ):
        self.title = title
        self.breadcrumbs = breadcrumbs
        self.options: List[Tuple[Optional[str], str, bool]] = []
        self.valid_keys: List[str] = []
        self._status_fn = status_fn
        self._status_key_fn = status_key_fn

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
        if self.breadcrumbs is None:
            ui.echo("\n" + "=" * width)
            ui.echo("")
            for line in _LOGO_LINES:
                ui.echo(line.center(width))
            ui.echo("")
            ui.echo("=" * width + "\n")
        else:
            ui.echo("\n" + "=" * width)
            ui.echo(f"   {self._display_title()}")
            ui.echo("=" * width + "\n")

    def show(self) -> None:
        self._render_header()

        if self._status_fn:
            ui.echo(f"   ■ {self._status_fn()}")
            ui.echo("")

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

            choices: List[Union[Choice, Separator]] = []

            extra_kwargs: Dict[str, Any] = {}

            if self._status_fn:
                initial_key = self._status_key_fn() if self._status_key_fn else None
                choices.append(Separator(" "))
                choices.append(
                    Separator(
                        _LiveStatusText(  # type: ignore[arg-type]
                            self._status_fn, self._status_key_fn, initial_key
                        )
                    )
                )
                choices.append(Separator(" "))
                extra_kwargs["refresh_interval"] = 3.0
            else:
                choices.append(Separator(" "))

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
                **extra_kwargs,
            ).ask()

            if answer == _REFRESH_SENTINEL:
                return _REFRESH_SENTINEL
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
    menu_items: List[MenuItem],
    title_key: str,
    breadcrumbs: Optional[str] = None,
    status_fn: Optional[Callable[[], str]] = None,
    status_key_fn: Optional[Callable[[], str]] = None,
) -> Optional[str]:
    menu = TerminalMenu(
        get_string(title_key),
        breadcrumbs,
        status_fn=status_fn,
        status_key_fn=status_key_fn,
    )
    menu.populate(menu_items)

    action_map = {
        item.key: item.action for item in menu_items if item.item_type == "option"
    }

    choice = menu.ask(get_string("prompt_select"), get_string("err_invalid_selection"))
    if choice == _REFRESH_SENTINEL:
        return None
    return action_map.get(choice)
