"""CI script: download Python embeddable zip and install pip + requirements into bin/python3/."""

import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

import requests

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_TOOLS_CONFIG = REPO_ROOT / ".github" / "ci-tools.json"
PYTHON_DIR = REPO_ROOT / "bin" / "python3"
REQUIREMENTS = REPO_ROOT / "bin" / "requirements.txt"


def _download(url: str, dest: Path, description: str) -> None:
    print(f"[bundle-python] Downloading {description}...")
    response = requests.get(url, stream=True, timeout=60)
    response.raise_for_status()
    with open(dest, "wb") as f:
        for chunk in response.iter_content(chunk_size=8192):
            if chunk:
                f.write(chunk)
    print(f"[bundle-python] Downloaded {dest.name}")


def main() -> None:
    with open(CI_TOOLS_CONFIG, "r", encoding="utf-8") as f:
        config = json.load(f)

    py_config = config["python"]
    embed_url = py_config["embed_url"]
    get_pip_url = py_config["get_pip_url"]
    pth_source = REPO_ROOT / py_config["pth_source"]

    python_exe = PYTHON_DIR / "python.exe"
    if python_exe.exists():
        print("[bundle-python] Python already present, skipping.")
        return

    PYTHON_DIR.mkdir(parents=True, exist_ok=True)

    # 1. Download and extract Python embeddable zip
    temp_zip = REPO_ROOT / "bin" / "python_embed.zip"
    _download(embed_url, temp_zip, f"Python {py_config['version']} embeddable")

    with zipfile.ZipFile(temp_zip, "r") as zf:
        zf.extractall(PYTHON_DIR)
    temp_zip.unlink()
    print("[bundle-python] Python extracted.")

    # 2. Copy ._pth file to enable site-packages
    if pth_source.exists():
        pth_dest = PYTHON_DIR / pth_source.name
        shutil.copy(pth_source, pth_dest)
        print(f"[bundle-python] Copied {pth_source.name}")

    # 3. Install pip
    get_pip_path = PYTHON_DIR / "get-pip.py"
    _download(get_pip_url, get_pip_path, "get-pip.py")

    subprocess.run(
        [str(python_exe), str(get_pip_path)],
        check=True,
    )
    get_pip_path.unlink()
    print("[bundle-python] pip installed.")

    # 4. Install requirements
    if REQUIREMENTS.exists():
        subprocess.run(
            [str(python_exe), "-m", "pip", "install", "--upgrade", "-r", str(REQUIREMENTS)],
            check=True,
        )
        print("[bundle-python] Requirements installed.")
    else:
        print(f"[bundle-python] WARNING: {REQUIREMENTS} not found, skipping dependency install.")

    print("[bundle-python] Python environment bundled successfully.")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"[bundle-python] FATAL: {e}", file=sys.stderr)
        sys.exit(1)
