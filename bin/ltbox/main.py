import json
import os
import platform
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, ClassVar, Dict, List, Optional, Tuple

from . import downloader, i18n, update_service, utils
from .app_state import AppState
from .i18n import get_string
from .logger import logging_context
from .registry import CommandRegistry
from .utils import ui

if TYPE_CHECKING:
    from .menu_router import DeviceControllerFactoryProtocol

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


# --- Settings & Init ---


@dataclass(frozen=True)
class AppSettings:
    language: Optional[str] = None
    target_region: str = "PRC"
    modify_region_code: bool = True
    skip_rollback: bool = False
    preset_code: str = "1"

    _ALLOWED_TARGET_REGIONS: ClassVar[set[str]] = {"PRC", "ROW"}
    _ALLOWED_PRESET_CODES: ClassVar[set[str]] = {"1", "2", "3", "-"}

    @staticmethod
    def validate_language(value: Any) -> Optional[str]:
        return value if isinstance(value, str) else None

    @classmethod
    def validate_target_region(cls, value: Any) -> str:
        return value if value in cls._ALLOWED_TARGET_REGIONS else "PRC"

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "AppSettings":
        target_region = cls.validate_target_region(data.get("target_region", "PRC"))
        modify_region_code = bool(data.get("modify_region_code", True))
        skip_rollback = bool(data.get("skip_rollback", False))
        preset_code = data.get("preset_code", "1")
        if preset_code not in cls._ALLOWED_PRESET_CODES:
            preset_code = "1"
        return cls(
            language=cls.validate_language(data.get("language")),
            target_region=target_region,
            modify_region_code=modify_region_code,
            skip_rollback=skip_rollback,
            preset_code=preset_code,
        )


class SettingsStore:
    _UPDATE_VALIDATORS: ClassVar[Dict[str, Callable[[Any], bool]]] = {
        "language": lambda value: isinstance(value, str),
        "target_region": lambda value: value in AppSettings._ALLOWED_TARGET_REGIONS,
        "modify_region_code": lambda value: isinstance(value, bool),
        "skip_rollback": lambda value: isinstance(value, bool),
        "preset_code": lambda value: value in AppSettings._ALLOWED_PRESET_CODES,
    }

    def __init__(self, path: Path):
        self._path = path

    def load_raw(self) -> Dict[str, Any]:
        if self._path.exists():
            try:
                with open(self._path, "r", encoding="utf-8") as f:
                    data = json.load(f)
                    return data if isinstance(data, dict) else {}
            except (json.JSONDecodeError, OSError):
                return {}
        return {}

    def load(self) -> AppSettings:
        return AppSettings.from_dict(self.load_raw())

    def _filter_valid_updates(self, updates: Dict[str, Any]) -> Dict[str, Any]:
        validated: Dict[str, Any] = {}
        for key, value in updates.items():
            validator = self._UPDATE_VALIDATORS.get(key)
            if validator and validator(value):
                validated[key] = value
        return validated

    def update(self, **updates: Any) -> AppSettings:
        data = self.load_raw()
        validated = self._filter_valid_updates(updates)

        if not validated:
            return AppSettings.from_dict(data)

        data.update(validated)
        try:
            with open(self._path, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2)
        except (OSError, TypeError, ValueError) as e:
            print(get_string("warn_save_settings_failed").format(e=e), file=sys.stderr)
        return AppSettings.from_dict(data)


SETTINGS_STORE = SettingsStore(SETTINGS_FILE)


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

    except (ImportError, OSError, AttributeError) as e:
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


def collect_info_scan_files(paths: List[str]) -> List[Path]:
    files_to_scan: List[Path] = []

    for path_str in paths:
        candidate = Path(path_str)
        if candidate.is_dir():
            files_to_scan.extend(candidate.rglob("*.img"))
        elif candidate.is_file() and candidate.suffix.lower() == ".img":
            files_to_scan.append(candidate)

    return files_to_scan


def build_info_scan_command(image_path: Path, constants: Any) -> List[str]:
    return [
        str(constants.PYTHON_EXE),
        str(constants.AVBTOOL_PY),
        "info_image",
        "--image",
        str(image_path),
    ]


def run_info_scan(paths: List[str], constants: Any, avb_patch: Any) -> None:
    print(get_string("scan_start"))

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_dir = constants.BASE_DIR / "log"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_filename = log_dir / f"image_info_{timestamp}.txt"

    files_to_scan = collect_info_scan_files(paths)

    if not files_to_scan:
        print(get_string("scan_no_files"), file=sys.stderr)
        return

    print(get_string("scan_found_files").format(count=len(files_to_scan)))

    with logging_context(log_filename) as logger:
        for image_path in files_to_scan:
            header = get_string("scan_log_header").format(path=image_path.resolve())
            logger.info(header)
            print(get_string("scan_scanning_file").format(filename=image_path.name))

            try:
                command = build_info_scan_command(image_path, constants)
                result = avb_patch.utils.run_command(command, capture=True, check=False)

                logger.info(result.stdout.strip())

                if result.stderr:
                    logger.info(
                        get_string("scan_log_errors").format(
                            errors=result.stderr.strip()
                        )
                    )

                logger.info("\n" + "=" * ui.get_term_width() + "\n")
            except (OSError, RuntimeError, ValueError, AttributeError) as e:
                error_msg = get_string("scan_failed").format(
                    filename=image_path.name, e=e
                )
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


def _initialize_runtime(
    lang_code: str,
) -> Tuple["DeviceControllerFactoryProtocol", CommandRegistry, Any, Any]:
    downloader.install_base_tools(lang_code)
    utils.check_dependencies()

    from . import constants, device
    from .patch import avb
    from .menu_router import prompt_for_language
    from .registry import REGISTRY
    from .commands import register_all_commands

    @REGISTRY.register("change_language", get_string("lang_changed"), require_dev=False)
    def change_language_task(breadcrumbs: Optional[str] = None):
        new_lang = prompt_for_language(
            force_prompt=True, settings_store=SETTINGS_STORE, breadcrumbs=breadcrumbs
        )
        i18n.load_lang(new_lang)
        return get_string("lang_changed")

    register_all_commands()

    return device.DeviceController, REGISTRY, constants, avb


def _run_entry_mode(
    is_info_mode: bool,
    device_controller_class: "DeviceControllerFactoryProtocol",
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

        settings = settings_store.load()
        final_state = main_loop(
            device_controller_class,
            registry,
            initial_state=AppState(
                target_region=settings.target_region,
                modify_region_code=settings.modify_region_code,
                skip_rollback=settings.skip_rollback,
                preset_code=settings.preset_code,
                language=settings.language,
            ),
        )
        settings_store.update(
            target_region=final_state.target_region,
            modify_region_code=final_state.modify_region_code,
            skip_rollback=final_state.skip_rollback,
            preset_code=final_state.preset_code,
            language=final_state.language,
        )


# --- Singleton Check ---


def _acquire_single_instance_mutex() -> Optional[Any]:
    import ctypes

    if sys.platform != "win32":
        return "Non-Windows-Mutex"

    windll = getattr(ctypes, "windll", None)
    if windll is None:
        return None

    kernel32 = windll.kernel32
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
    current_version, latest_version, _, _ = update_service.get_update_status()
    update_service.prompt_for_update(current_version, latest_version)


def _is_running_as_admin() -> bool:
    if os.name != "nt":
        return True

    try:
        import ctypes

        windll = getattr(ctypes, "windll", None)
        if windll is None:
            return False

        return bool(windll.shell32.IsUserAnAdmin())
    except (ImportError, OSError, AttributeError):
        return False


def _ensure_admin_or_exit() -> None:
    if _is_running_as_admin():
        return

    ui.clear()
    ui.error(get_string("startup_admin_required"))
    input(get_string("press_enter_to_exit"))
    sys.exit(0)


def _force_kill_processes(exe_names: List[str]) -> None:
    for exe_name in exe_names:
        try:
            subprocess.run(
                ["taskkill", "/F", "/IM", exe_name, "/T"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=(
                    getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0
                ),
            )
        except (subprocess.CalledProcessError, OSError):
            pass


def _get_running_processes(exe_names: List[str]) -> List[str]:
    if os.name != "nt":
        return []
    try:
        result = subprocess.run(
            ["tasklist"],
            capture_output=True,
            text=True,
            check=False,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        tasklist_output = result.stdout.lower()
        return [name for name in exe_names if name.lower() in tasklist_output]
    except (subprocess.CalledProcessError, OSError):
        return []


def _resolve_process_conflicts() -> None:
    process_names = [
        "adb.exe",
        "fastboot.exe",
        "Software Fix.exe",
        "fh_loader.exe",
        "QSaharaServer.exe",
    ]
    running = _get_running_processes(process_names)
    if not running:
        return

    ui.clear()
    running_list = ", ".join(running)
    ui.warn(
        get_string("startup_conflict_processes_prompt").format(processes=running_list)
    )
    choice = ui.prompt(get_string("startup_conflict_confirm")).strip().lower()
    if choice == "y":
        _force_kill_processes(running)
        return

    ui.warn(get_string("startup_conflict_exit_message"))
    input(get_string("press_enter_to_exit"))
    sys.exit(0)


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

        if is_info_mode:
            _init_and_run(is_info_mode, lang_code)
            return

        if not singleton_mutex:
            ui.clear()
            ui.error(get_string("err_already_running"))
            input()
            sys.exit(0)

        _ensure_admin_or_exit()
        _resolve_process_conflicts()
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
