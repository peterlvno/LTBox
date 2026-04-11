from typing import Any, Callable, Iterable, Optional, Set

InputFunc = Callable[[str], str]
ErrorFunc = Callable[[str], None]
PauseFunc = Callable[[], Any]
ClearFunc = Callable[[], None]
RenderMultiSelectFunc = Callable[[Set[int]], None]


def prompt_choice(
    prompt: str,
    valid_choices: Iterable[str],
    *,
    input_func: InputFunc,
    error_message: str,
    error_func: ErrorFunc,
    normalize: Optional[Callable[[str], str]] = None,
    pause_func: Optional[PauseFunc] = None,
) -> str:
    choices = set(valid_choices)
    normalize_choice = normalize or (lambda value: value)

    while True:
        choice = normalize_choice(input_func(prompt))
        if choice in choices:
            return choice

        error_func(error_message)
        if pause_func is not None:
            pause_func()


def prompt_index_selection(
    prompt: str,
    *,
    max_index: int,
    error_message: str,
    input_func: InputFunc,
    error_func: ErrorFunc,
    pause_func: Optional[PauseFunc] = None,
    min_index: int = 1,
) -> int:
    while True:
        choice = input_func(prompt).strip()
        try:
            index = int(choice)
        except ValueError:
            error_func(error_message)
            if pause_func is not None:
                pause_func()
            continue

        if min_index <= index <= max_index:
            return index

        error_func(error_message)
        if pause_func is not None:
            pause_func()


def prompt_yes_no(
    prompt: str,
    *,
    input_func: InputFunc,
    error_message: str,
    error_func: ErrorFunc,
    allow_cancel: bool = False,
) -> Optional[bool]:
    valid_choices = {"y", "n"}
    if allow_cancel:
        valid_choices.add("c")

    choice = prompt_choice(
        prompt,
        valid_choices,
        input_func=input_func,
        error_message=error_message,
        error_func=error_func,
        normalize=lambda value: value.strip().lower(),
    )
    if choice == "c":
        return None
    return choice == "y"


def prompt_multi_select_indices(
    prompt: str,
    *,
    item_count: int,
    render_func: RenderMultiSelectFunc,
    input_func: InputFunc,
    error_message: str,
    error_func: ErrorFunc,
    pause_func: Optional[PauseFunc] = None,
    clear_func: Optional[ClearFunc] = None,
    finish_choice: str = "f",
    cancel_choice: str = "c",
    select_all_choice: Optional[str] = None,
    deselect_all_choice: Optional[str] = None,
) -> Optional[list[int]]:
    selected: Set[int] = set()

    while True:
        if clear_func is not None:
            clear_func()
        render_func(selected)

        choice = input_func(prompt).strip().lower()
        if choice == finish_choice:
            return sorted(selected)
        if choice == cancel_choice:
            return None
        if select_all_choice is not None and choice == select_all_choice:
            selected = set(range(item_count))
            continue
        if deselect_all_choice is not None and choice == deselect_all_choice:
            selected.clear()
            continue

        try:
            index = int(choice)
        except ValueError:
            error_func(error_message)
            if pause_func is not None:
                pause_func()
            continue

        if not 1 <= index <= item_count:
            error_func(error_message)
            if pause_func is not None:
                pause_func()
            continue

        offset = index - 1
        if offset in selected:
            selected.remove(offset)
        else:
            selected.add(offset)
