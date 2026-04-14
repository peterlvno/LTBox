import os
import sys
import pytest
from pathlib import Path
from unittest.mock import patch
from ltbox import i18n
from tests.scripts import cache_fw
from tests.actions.integration.fixtures import firmware_file_getter  # noqa: F401

ROOT = Path(__file__).resolve().parents[1]
BIN_PATH = ROOT / "bin"
INTEGRATION_TOOL_FILES = (
    "magiskboot.exe",
    "magiskboot_xz_helper.exe",
    "openssl.exe",
)

if str(BIN_PATH) not in sys.path:
    sys.path.insert(0, str(BIN_PATH))


def pytest_configure(config):
    i18n.load_lang("en")


def pytest_addoption(parser):
    parser.addoption(
        "--run-integration",
        action="store_true",
        default=False,
        help="Run integration tests that require external tools or downloads.",
    )


def pytest_collection_modifyitems(config, items):
    run_integration = config.getoption("--run-integration")
    skip_integration = pytest.mark.skip(
        reason="integration tests require --run-integration"
    )

    for item in items:
        item.add_marker(pytest.mark.usefixtures("mock_python_executable"))

        if "integration" in item.keywords:
            if not run_integration:
                item.add_marker(skip_integration)


def _integration_tools_ready() -> bool:
    tools_dir = ROOT / "bin" / "tools"
    return all((tools_dir / name).exists() for name in INTEGRATION_TOOL_FILES)


@pytest.fixture(scope="session")
def integration_tools(request):
    if not request.config.getoption("--run-integration"):
        return

    if os.environ.get("LTBOX_FORCE_TOOL_BUILD") != "1" and _integration_tools_ready():
        print("\n[INFO] Using prebuilt tools from bin/tools.", flush=True)
        return

    print("\n[INFO] Setting up external tools for integration tests...", flush=True)
    try:
        from tests.scripts.build_tools import build

        build()

    except Exception as e:
        print(f"\n[WARN] Failed to setup tools: {e}", flush=True)


@pytest.fixture(autouse=True)
def _clear_api_caches():
    from ltbox.github_client import _api_cache
    from ltbox.utils import _release_cache

    _api_cache.clear()
    _release_cache.clear()


@pytest.fixture
def mock_python_executable():
    with patch("ltbox.constants.PYTHON_EXE", sys.executable):
        yield


@pytest.fixture(scope="module")
def fw_pkg(integration_tools):
    if not cache_fw.FW_PW:
        pytest.skip("TEST_FW_PASSWORD not set")

    try:
        cache_fw.ensure_firmware_extracted()
    except Exception as e:
        pytest.fail(f"Firmware preparation failed: {e}")

    cached_map = {}
    missing_targets = False

    for target in cache_fw.TARGETS:
        found = list(cache_fw.EXTRACT_DIR.rglob(target))
        if found:
            cached_map[target] = found[0]
        else:
            missing_targets = True
            break

    if missing_targets:
        pytest.fail("Targets missing despite successful cache preparation")

    print("\n[INFO] Using cached extracted files.", flush=True)
    return cached_map


@pytest.fixture
def mock_env(tmp_path):
    dirs = {
        "IMAGE_DIR": tmp_path / "image",
        "OUTPUT_DP_DIR": tmp_path / "output_dp",
        "OUTPUT_DIR": tmp_path / "output",
        "OUTPUT_ANTI_ROLLBACK_DIR": tmp_path / "output_arb",
        "OUTPUT_XML_DIR": tmp_path / "output_xml",
        "EDL_LOADER_FILE": tmp_path / "loader.elf",
    }
    for directory in dirs.values():
        if directory.suffix:
            directory.parent.mkdir(parents=True, exist_ok=True)
            directory.touch()
        else:
            directory.mkdir(parents=True, exist_ok=True)

    with patch.multiple("ltbox.constants", **dirs):
        yield dirs
