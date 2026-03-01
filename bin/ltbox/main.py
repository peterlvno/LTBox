import json
import os
import functools
import platform
import subprocess
import sys
import webbrowser
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from . import downloader, i18n, utils
from .i18n import get_string
from .logger import logging_context
from .registry import CommandRegistry
from .utils import ui

APP_DIR = Path(__file__).parent.resolve()
BASE_DIR = APP_DIR.parent
PYTHON_EXE = BASE_DIR / "python3" / "python.exe"
SETTINGS_FILE = APP_DIR / "settings.json"

try:
    from .errors import LTBoxError, ToolError
except ImportError:
    print(get_string("err_import_critical"), file=sys.stderr)
    print(get_string("err_ensure_errors"), file=sys.stderr)
    input(get_string("press_enter_to_exit"))
    sys.exit(1)


# --- UI Helper Class ---


def _format_command_failure_messages(
    error: subprocess.CalledProcessError,
) -> List[str]:
    messages = [
        get_string("err_cmd_failed").format(
            cmd=" ".join(error.cmd) if isinstance(error.cmd, list) else error.cmd
        )
    ]
    if error.stdout:
        messages.append(f"{get_string('err_cmd_stdout_header')}\n{error.stdout}")
    if error.stderr:
        messages.append(f"{get_string('err_cmd_stderr_header')}\n{error.stderr}")
    return messages


# --- Settings & Init ---


@dataclass(frozen=True)
class AppSettings:
    language: Optional[str] = None
    target_region: str = "PRC"

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "AppSettings":
        language = data.get("language")
        if not isinstance(language, str):
            language = None

        target_region = data.get("target_region", "PRC")
        if target_region not in ("PRC", "ROW"):
            target_region = "PRC"

        return cls(language=language, target_region=target_region)


class SettingsStore:
    def __init__(self, path: Path):
        self._path = path

    def load_raw(self) -> Dict[str, Any]:
        if self._path.exists():
            try:
                with open(self._path, "r", encoding="utf-8") as f:
                    data = json.load(f)
                    return data if isinstance(data, dict) else {}
            except Exception:
                return {}
        return {}

    def load(self) -> AppSettings:
        return AppSettings.from_dict(self.load_raw())

    def update(self, **updates: Any) -> AppSettings:
        data = self.load_raw()
        validated = {}

        if "language" in updates:
            language = updates["language"]
            if isinstance(language, str):
                validated["language"] = language

        if "target_region" in updates:
            target_region = updates["target_region"]
            if target_region in ("PRC", "ROW"):
                validated["target_region"] = target_region

        if not validated:
            return AppSettings.from_dict(data)

        data.update(validated)
        try:
            with open(self._path, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2)
        except Exception as e:
            print(f"Warning: Failed to save settings: {e}", file=sys.stderr)
        return AppSettings.from_dict(data)


SETTINGS_STORE = SettingsStore(SETTINGS_FILE)


def _read_current_version() -> str:
    config_file = APP_DIR / "config.json"
    if config_file.exists():
        try:
            with open(config_file, "r", encoding="utf-8") as f:
                config_data = json.load(f)
                return config_data.get("version", "v0.0.0")
        except Exception:
            return "v0.0.0"
    return "v0.0.0"


def _get_latest_version(
    current_version: str,
) -> Tuple[Optional[str], Optional[str], Optional[str]]:
    try:
        latest_release, latest_prerelease = utils.get_latest_release_versions(
            "miner7222", "LTBox"
        )
        latest_version = None

        if latest_release and utils.is_update_available(
            current_version, latest_release
        ):
            latest_version = latest_release
        elif latest_release and utils.is_update_available(
            latest_release, current_version
        ):
            if latest_prerelease and utils.is_update_available(
                current_version, latest_prerelease
            ):
                latest_version = latest_prerelease
        elif latest_release is None and latest_prerelease:
            if utils.is_update_available(current_version, latest_prerelease):
                latest_version = latest_prerelease

        return latest_version, latest_release, latest_prerelease
    except Exception:
        return None, None, None


def _abort_platform_check(messages: List[str]) -> None:
    for message in messages:
        print(message, file=sys.stderr)
    print(get_string("err_aborting"), file=sys.stderr)
    input(get_string("press_enter_to_exit"))
    sys.exit(1)


def _check_platform():
    if platform.system() != "Windows":
        _abort_platform_check(
            [
                get_string("err_fatal_windows"),
                get_string("err_current_platform").format(platform=platform.system()),
            ]
        )

    if platform.machine() != "AMD64":
        _abort_platform_check(
            [
                get_string("err_fatal_amd64"),
                get_string("err_current_arch").format(arch=platform.machine()),
                get_string("err_arch_unsupported"),
            ]
        )


def setup_console():
    try:
        import ctypes

        if sys.platform == "win32":
            kernel32 = ctypes.windll.kernel32
            kernel32.SetConsoleTitleW("LTBox")

            STD_INPUT_HANDLE = -10
            ENABLE_QUICK_EDIT_MODE = 0x0040
            ENABLE_EXTENDED_FLAGS = 0x0080

            hStdIn = kernel32.GetStdHandle(STD_INPUT_HANDLE)
            mode = ctypes.c_uint32()
            if kernel32.GetConsoleMode(hStdIn, ctypes.byref(mode)):
                mode.value &= ~ENABLE_QUICK_EDIT_MODE
                mode.value |= ENABLE_EXTENDED_FLAGS
                kernel32.SetConsoleMode(hStdIn, mode)

        sys.stdout.write("\x1b[8;40;80t")
        sys.stdout.flush()

        os.system("mode con: cols=80 lines=40")

    except Exception as e:
        print(get_string("warn_set_console_title").format(e=e), file=sys.stderr)


def check_path_encoding():
    current_path = str(Path(__file__).parent.parent.resolve())
    if not current_path.isascii():
        ui.clear()
        width = ui.get_term_width()
        ui.box_output(
            [
                get_string("critical_error_path_encoding"),
                "-" * width,
                get_string("current_path").format(current_path=current_path),
                "-" * width,
                get_string("path_encoding_details_1"),
                get_string("path_encoding_details_2"),
                "",
                get_string("action_required"),
                get_string("action_required_details"),
                get_string("example_path"),
            ],
            err=True,
        )

        input(get_string("press_enter_to_continue"))
        raise RuntimeError(get_string("critical_error_path_encoding"))


# --- Task Execution ---


@functools.singledispatch
def _handle_task_error(error: BaseException, title: str) -> None:
    pass


@_handle_task_error.register
def _(error: LTBoxError, title: str) -> None:
    ui.box_output([get_string("task_failed").format(title=title), str(error)], err=True)


@_handle_task_error.register
def _(error: subprocess.CalledProcessError, title: str) -> None:
    ui.box_output(_format_command_failure_messages(error), err=True)


@_handle_task_error.register(FileNotFoundError)
@_handle_task_error.register(RuntimeError)
@_handle_task_error.register(KeyError)
def _(error: Exception, title: str) -> None:
    ui.box_output([get_string("unexpected_error").format(e=error)], err=True)


@_handle_task_error.register
def _(error: SystemExit, title: str) -> None:
    ui.error(get_string("process_halted"))


@_handle_task_error.register
def _(error: KeyboardInterrupt, title: str) -> None:
    ui.error(get_string("process_cancelled"))


def run_task(
    command: str,
    dev: Any,
    registry: CommandRegistry,
    extra_kwargs: Optional[Dict[str, Any]] = None,
):
    ui.clear()

    cmd_info = registry.get(command)
    if not cmd_info:
        raise ToolError(get_string("unknown_command").format(command=command))

    title = cmd_info.title
    func = cmd_info.func
    base_kwargs = cmd_info.default_kwargs
    require_dev = cmd_info.require_dev
    result_handler = cmd_info.result_handler

    is_workflow = command in ("patch_all", "patch_all_wipe")
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")

    log_filename = None
    if not is_workflow:
        log_dir = BASE_DIR.parent / "log"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_filename = str(log_dir / f"log_{command}_{timestamp}.txt")

    try:
        with logging_context(log_filename):
            if dev and hasattr(dev, "reset_task_state"):
                dev.reset_task_state()

            if not is_workflow:
                ui.info(get_string("logging_enabled").format(log_file=log_filename))
                ui.info(get_string("logging_command").format(command=command))

            final_kwargs = base_kwargs.copy()

            if extra_kwargs:
                final_kwargs.update(extra_kwargs)

            if require_dev:
                final_kwargs["dev"] = dev

            result = func(**final_kwargs)

            if result_handler:
                result_handler(result)
            elif isinstance(result, str) and result:
                ui.echo(result)
            elif result:
                ui.echo(get_string("act_unhandled_success_result").format(res=result))

    except Exception as e:
        _handle_task_error(e, title)
    finally:
        if dev and hasattr(dev, "adb"):
            dev.adb.force_kill_server()
        if dev and hasattr(dev, "fastboot"):
            dev.fastboot.force_kill_server()

        if not is_workflow:
            ui.info(get_string("logging_finished").format(log_file=log_filename))

        ui.echo("")
        input(get_string("press_enter_to_continue"))


def run_info_scan(paths, constants, avb_patch):
    print(get_string("scan_start"))

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_dir = constants.BASE_DIR / "log"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_filename = log_dir / f"image_info_{timestamp}.txt"

    files_to_scan = []
    for path_str in paths:
        p = Path(path_str)
        if p.is_dir():
            files_to_scan.extend(p.rglob("*.img"))
        elif p.is_file() and p.suffix.lower() == ".img":
            files_to_scan.append(p)

    if not files_to_scan:
        print(get_string("scan_no_files"), file=sys.stderr)
        return

    print(get_string("scan_found_files").format(count=len(files_to_scan)))

    with logging_context(log_filename) as logger:
        for f in files_to_scan:
            header = get_string("scan_log_header").format(path=f.resolve())
            logger.info(header)
            print(get_string("scan_scanning_file").format(filename=f.name))

            try:
                cmd = [
                    str(constants.PYTHON_EXE),
                    str(constants.AVBTOOL_PY),
                    "info_image",
                    "--image",
                    str(f),
                ]

                result = avb_patch.utils.run_command(cmd, capture=True, check=False)

                logger.info(result.stdout.strip())

                if result.stderr:
                    logger.info(
                        get_string("scan_log_errors").format(
                            errors=result.stderr.strip()
                        )
                    )

                logger.info("\n" + "=" * ui.get_term_width() + "\n")
            except Exception as e:
                error_msg = get_string("scan_failed").format(filename=f.name, e=e)
                print(error_msg, file=sys.stderr)
                logger.info(error_msg)

    print(get_string("scan_complete"))
    print(get_string("scan_saved_to").format(filename=log_filename.name))


# --- Menus ---


def _resolve_language_code(
    is_info_mode: bool, settings_store: SettingsStore = SETTINGS_STORE
) -> str:
    from .menu_router import prompt_for_language

    return "en" if is_info_mode else prompt_for_language(settings_store=settings_store)


def _prompt_for_update(current_version: str, latest_version: Optional[str]) -> None:
    if not latest_version:
        return

    ui.echo(get_string("update_avail_title"))

    prompt_msg = get_string("update_avail_prompt").format(
        curr=current_version, new=latest_version
    )
    choice = input(prompt_msg).strip().lower()

    if choice == "y":
        ui.echo(get_string("update_open_web"))
        webbrowser.open("https://github.com/miner7222/LTBox/releases")
        sys.exit(0)

    ui.clear()


def _initialize_runtime(lang_code: str) -> Tuple[type, CommandRegistry, Any, Any]:
    downloader.install_base_tools(lang_code)
    utils.check_dependencies()

    from . import constants, device
    from .patch import avb
    from .menu_router import prompt_for_language
    from .registry import REGISTRY
    from .commands import register_all_commands

    @REGISTRY.register("change_language", get_string("lang_changed"), require_dev=False)
    def change_language_task(breadcrumbs: Optional[str] = None):
        new_lang = prompt_for_language(force_prompt=True, breadcrumbs=breadcrumbs)
        i18n.load_lang(new_lang)
        return get_string("lang_changed")

    register_all_commands()

    return device.DeviceController, REGISTRY, constants, avb


def _run_entry_mode(
    is_info_mode: bool,
    device_controller_class: type,
    registry: CommandRegistry,
    constants_module: Any,
    avb_patch_module: Any,
    settings_store: Optional[SettingsStore] = None,
) -> None:
    check_path_encoding()

    if is_info_mode:
        if len(sys.argv) > 2:
            run_info_scan(sys.argv[2:], constants_module, avb_patch_module)
        else:
            ui.error(get_string("info_no_files_dragged"))
            ui.error(get_string("info_drag_files_prompt"))

        input(get_string("press_enter_to_exit"))
    else:
        if settings_store is None:
            settings_store = SETTINGS_STORE

        from .menu_router import main_loop

        main_loop(device_controller_class, registry, settings_store=settings_store)


# --- Singleton Check ---


def _acquire_single_instance_mutex() -> Optional[Any]:
    import ctypes

    if sys.platform != "win32":
        return "Non-Windows-Mutex"

    kernel32 = ctypes.windll.kernel32
    mutex_name = "Global\\LTBox_Singleton_Mutex"

    mutex = kernel32.CreateMutexW(None, False, mutex_name)

    if kernel32.GetLastError() == 183:
        return None

    return mutex


# --- Entry Point ---


def _prepare_environment() -> Any:
    _check_platform()
    setup_console()
    return _acquire_single_instance_mutex()


def _setup_language(is_info_mode: bool) -> str:
    lang_code = _resolve_language_code(is_info_mode, settings_store=SETTINGS_STORE)
    i18n.load_lang(lang_code)
    return lang_code


def _check_updates() -> None:
    ui.clear()
    current_version = _read_current_version()
    latest_version, _, _ = _get_latest_version(current_version)
    _prompt_for_update(current_version, latest_version)


def _init_and_run(is_info_mode: bool, lang_code: str) -> None:
    try:
        (
            device_controller_class,
            registry,
            constants_module,
            avb_patch_module,
        ) = _initialize_runtime(lang_code)

        _run_entry_mode(
            is_info_mode,
            device_controller_class,
            registry,
            constants_module,
            avb_patch_module,
            settings_store=SETTINGS_STORE,
        )
    except (subprocess.CalledProcessError, FileNotFoundError, ToolError) as e:
        ui.error(get_string("critical_err_base_tools").format(e=e))
        ui.error(get_string("err_run_install_manually"))
        input(get_string("press_enter_to_exit"))
        sys.exit(1)
    except ImportError as e:
        ui.error(get_string("err_import_ltbox"))
        ui.error(get_string("err_details").format(e=e))
        ui.error(get_string("err_ensure_ltbox_present"))
        input(get_string("press_enter_to_exit"))
        sys.exit(1)


def entry_point() -> None:
    try:
        is_info_mode = len(sys.argv) > 1 and sys.argv[1].lower() == "info"
        singleton_mutex = _prepare_environment()
        lang_code = _setup_language(is_info_mode)
        if not singleton_mutex:
            ui.clear()
            ui.error(get_string("err_already_running"))
            input()
            sys.exit(0)

        _check_updates()
        _init_and_run(is_info_mode, lang_code)

    except (LTBoxError, RuntimeError) as e:
        ui.error(get_string("err_fatal_abort"))
        ui.error(get_string("err_details").format(e=e))
        input(get_string("press_enter_to_exit"))
        sys.exit(1)
    except KeyboardInterrupt:
        ui.error(get_string("err_fatal_user_cancel"))
        sys.exit(0)


if __name__ == "__main__":
    entry_point()
