import json
import sys
import webbrowser
from pathlib import Path
from typing import Optional, Tuple

from . import utils
from .i18n import get_string
from .utils import ui

APP_DIR = Path(__file__).parent.resolve()


def read_current_version() -> str:
    config_file = APP_DIR / "config.json"
    if config_file.exists():
        try:
            with open(config_file, "r", encoding="utf-8") as f:
                config_data = json.load(f)
                return config_data.get("version", "v0.0.0")
        except (OSError, json.JSONDecodeError, TypeError, ValueError):
            return "v0.0.0"
    return "v0.0.0"


def get_latest_version(
    current_version: str,
) -> Tuple[Optional[str], Optional[str], Optional[str]]:
    latest_release, latest_prerelease = utils.get_latest_release_versions(
        "miner7222", "LTBox"
    )
    latest_version = None

    if latest_release and utils.is_update_available(current_version, latest_release):
        latest_version = latest_release
    elif latest_release and utils.is_update_available(latest_release, current_version):
        if latest_prerelease and utils.is_update_available(
            current_version, latest_prerelease
        ):
            latest_version = latest_prerelease
    elif latest_release is None and latest_prerelease:
        if utils.is_update_available(current_version, latest_prerelease):
            latest_version = latest_prerelease

    return latest_version, latest_release, latest_prerelease


def get_update_status() -> Tuple[str, Optional[str], Optional[str], Optional[str]]:
    current_version = read_current_version()
    latest_version, latest_release, latest_prerelease = get_latest_version(
        current_version
    )
    return current_version, latest_version, latest_release, latest_prerelease


def prompt_for_update(current_version: str, latest_version: Optional[str]) -> bool:
    if not latest_version:
        return False

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
    return False
