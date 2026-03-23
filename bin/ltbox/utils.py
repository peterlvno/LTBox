import json
import os
import shutil
import subprocess
import time
import urllib.error
import urllib.request
import functools
import warnings
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Generator, Iterable, List, Optional, Union

from . import constants as const
from .i18n import get_string
from .logger import get_logger
from .process_runner import CommandResult, CommandRunner, RunOptions
from .ui import ui

logger = get_logger()


def get_latest_release_versions(
    repo_owner: str, repo_name: str
) -> tuple[Optional[str], Optional[str]]:
    url = f"https://api.github.com/repos/{repo_owner}/{repo_name}/releases?per_page=100"
    latest_release = None
    latest_prerelease = None
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            if response.status == 200:
                data = json.loads(response.read().decode())
                for release in data:
                    if release.get("draft"):
                        continue
                    tag = release.get("tag_name")
                    if not tag:
                        continue
                    if release.get("prerelease"):
                        if latest_prerelease is None or is_update_available(
                            latest_prerelease, tag
                        ):
                            latest_prerelease = tag
                    else:
                        if latest_release is None or is_update_available(
                            latest_release, tag
                        ):
                            latest_release = tag
    except (urllib.error.URLError, json.JSONDecodeError, OSError):
        return None, None
    return latest_release, latest_prerelease


def is_update_available(current: str, latest: str) -> bool:
    def version_to_tuple(v_str):
        try:
            return tuple(map(int, v_str.lstrip("v").split(".")))
        except ValueError:
            return (0, 0, 0)

    return version_to_tuple(latest) > version_to_tuple(current)


@functools.lru_cache(maxsize=1)
def _get_tool_env() -> dict:
    env = os.environ.copy()
    paths = [str(const.TOOLS_DIR)]
    env["PATH"] = os.pathsep.join(paths) + os.pathsep + env["PATH"]
    return env


def wait_for_condition(
    predicate: Callable[[], Any],
    interval: float = 1.0,
    timeout: Optional[float] = None,
    on_loop: Optional[Callable[[], None]] = None,
) -> Any:
    start_time = time.monotonic()
    while True:
        result = predicate()
        if result:
            return result

        if timeout is not None and time.monotonic() - start_time >= timeout:
            return None

        if on_loop:
            on_loop()

        time.sleep(interval)


def run_command(
    command: Union[List[str], str],
    shell: bool = False,
    check: bool = True,
    env: Optional[dict] = None,
    capture: bool = False,
    cwd: Optional[Union[str, Path]] = None,
    on_output: Optional[Callable[[str], None]] = None,
) -> CommandResult:
    warnings.warn(
        "utils.run_command is deprecated; use process_runner.CommandRunner.run instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    run_env = env if env is not None else _get_tool_env()
    return CommandRunner().run(
        command,
        shell=shell,
        options=RunOptions(
            capture=capture,
            stream=not capture,
            check=check,
            cwd=cwd,
            env=run_env,
        ),
        on_output=on_output,
    )


def format_command_output(result: CommandResult) -> str:
    stdout = (result.stdout or "").strip()
    stderr = (result.stderr or "").strip()
    if stderr and stdout:
        return f"{stderr}\n{stdout}"
    return stderr or stdout


def get_platform_executable(name: str) -> Path:
    return const.TOOLS_DIR / f"{name}.exe"


def _wait_for_resource(
    target_path: Path,
    check_func: Callable[[Path, Optional[List[str]]], bool],
    prompt_msg: str,
    item_list: Optional[List[str]] = None,
) -> bool:
    target_path.mkdir(exist_ok=True, parents=True)

    def _prompt_loop() -> None:
        ui.clear()

        ui.echo(get_string("utils_wait_resource"))
        ui.echo(prompt_msg)
        if item_list:
            ui.echo(get_string("utils_missing_items"))
            for item in item_list:
                if not (target_path / item).exists():
                    ui.echo(get_string("utils_missing_item_format").format(item=item))

        ui.echo(get_string("press_enter_to_continue"))
        try:
            ui.prompt()
        except EOFError:
            raise RuntimeError(get_string("act_op_cancel"))

    return bool(
        wait_for_condition(
            lambda: check_func(target_path, item_list),
            interval=0.1,
            on_loop=_prompt_loop,
        )
    )


def wait_for_files(
    directory: Path, required_files: List[str], prompt_message: str
) -> bool:
    return _wait_for_resource(
        directory,
        lambda p, f: all((p / i).exists() for i in (f or [])),
        prompt_message,
        required_files,
    )


def wait_for_directory(directory: Path, prompt_message: str) -> bool:
    return _wait_for_resource(
        directory, lambda p, _: p.is_dir() and any(p.iterdir()), prompt_message, None
    )


def check_dependencies() -> None:
    is_git_checkout = (const.BASE_DIR / ".git").exists()
    missing_edl_binaries = (
        not const.EDL_EXE.exists() and not const.QSAHARASERVER_EXE.exists()
    )

    if not is_git_checkout and missing_edl_binaries:
        ui.echo(get_string("utils_err_non_release_download"))
        raise RuntimeError(get_string("utils_err_non_release_download"))

    dependencies = {
        "Python Environment": const.PYTHON_EXE,
        "ADB": const.ADB_EXE,
        "Fastboot": const.FASTBOOT_EXE,
        "avbtool": const.AVBTOOL_PY,
    }

    if not is_git_checkout:
        dependencies.update(
            {
                "fh_loader": const.EDL_EXE,
                "Qsaharaserver": const.QSAHARASERVER_EXE,
            }
        )

    for path in const.KEY_MAP.values():
        dependencies[path.name] = path

    missing_deps = [
        name for name, path in dependencies.items() if not Path(path).exists()
    ]

    if missing_deps:
        for name in missing_deps:
            ui.echo(get_string("utils_missing_dep").format(name=name))
        ui.echo(get_string("utils_run_install"))
        raise RuntimeError(get_string("utils_run_install"))

    _check_required_windows_drivers()

    ui.echo(get_string("utils_deps_found"))


def _check_required_windows_drivers() -> None:
    if os.name != "nt":
        return

    if not _is_driver_present(["qcser.inf", "lenovo_ser.inf"]):
        msg = get_string("utils_err_missing_qdloader_driver")
        ui.error(msg)
        raise RuntimeError(msg)

    if not _is_driver_present(["android_winusb.inf"]):
        msg = get_string("utils_err_missing_android_usb_driver")
        ui.error(msg)
        raise RuntimeError(msg)


def _is_driver_present(expected_inf_names: List[str]) -> bool:
    return _driver_present_via_pnputil(
        expected_inf_names
    ) or _driver_present_via_driver_store(expected_inf_names)


def _driver_present_via_pnputil(expected_inf_names: List[str]) -> bool:
    expected_set = {name.lower() for name in expected_inf_names}
    try:
        result = subprocess.run(
            ["pnputil", "/enum-drivers"],
            capture_output=True,
            text=True,
            check=False,
            encoding="utf-8",
            errors="ignore",
        )
        if result.returncode != 0:
            return False

        for line in result.stdout.splitlines():
            if ":" not in line:
                continue
            value = line.split(":", 1)[1].strip().lower()
            if value in expected_set:
                return True
    except OSError:
        return False

    return False


def _driver_present_via_driver_store(expected_inf_names: List[str]) -> bool:
    driver_store = (
        Path(os.environ.get("SystemRoot", r"C:\Windows"))
        / "System32"
        / "DriverStore"
        / "FileRepository"
    )
    if not driver_store.exists():
        return False

    for inf_name in expected_inf_names:
        if any(driver_store.glob(f"{inf_name}*")):
            return True
    return False


def move_existing_files(files: Iterable[Path], dst_dir: Path) -> int:
    dst_dir.mkdir(exist_ok=True, parents=True)
    moved_count = 0
    for f in files:
        if f.exists():
            shutil.move(str(f), str(dst_dir / f.name))
            moved_count += 1
    return moved_count


def recreate_dir(path: Path) -> None:
    """Removes the directory if it exists, then creates a fresh one."""
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


@contextmanager
def temporary_workspace(path: Path) -> Generator[Path, None, None]:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    try:
        yield path
    finally:
        if path.exists():
            try:
                shutil.rmtree(path)
            except OSError as e:
                ui.echo(
                    get_string("warn_failed_cleanup_workspace").format(path=path, e=e),
                    err=True,
                )


def _process_binary_file(
    input_path: Union[str, Path],
    output_path: Union[str, Path],
    patch_func: Any,
    copy_if_unchanged: bool = True,
    **kwargs: Any,
) -> bool:
    input_path = Path(input_path)
    output_path = Path(output_path)

    if not input_path.exists():
        ui.echo(get_string("img_proc_err_not_found").format(path=input_path), err=True)
        return False

    try:
        content = input_path.read_bytes()
        modified_content, stats = patch_func(content, **kwargs)

        if stats.get("changed", False):
            output_path.write_bytes(modified_content)
            ui.echo(
                get_string("img_proc_success").format(
                    msg=stats.get("message", get_string("img_proc_msg_modified"))
                )
            )
            ui.echo(get_string("img_proc_saved").format(name=output_path.name))
            return True
        else:
            ui.echo(
                get_string("img_proc_no_change").format(
                    name=input_path.name,
                    msg=stats.get("message", get_string("img_proc_msg_no_patterns")),
                )
            )
            if copy_if_unchanged:
                ui.echo(get_string("img_proc_copying").format(name=output_path.name))
                if input_path != output_path:
                    shutil.copy(input_path, output_path)
                return True
            return False

    except (OSError, IOError) as e:
        ui.echo(
            get_string("img_proc_error").format(name=input_path.name, e=e), err=True
        )
        return False


class ExternalTool:
    def __init__(self, base_cmd: List[Union[str, Path]]):
        self.base_cmd = [str(c) for c in base_cmd]

    def run(
        self,
        *args: Any,
        capture: bool = False,
        check: bool = True,
        cwd: Optional[Union[str, Path]] = None,
        **kwargs: Any,
    ) -> CommandResult:
        cmd = self.base_cmd + [str(arg) for arg in args]
        return run_command(cmd, capture=capture, check=check, cwd=cwd, **kwargs)


class AvbToolWrapper(ExternalTool):
    def __init__(self):
        super().__init__([const.PYTHON_EXE, const.AVBTOOL_PY])


class MagiskBootWrapper(ExternalTool):
    def __init__(self, exe_path: Union[str, Path]):
        super().__init__([exe_path])
