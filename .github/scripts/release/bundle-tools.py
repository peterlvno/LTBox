"""CI script: download packaged tools into bin/tools/."""

import json
import re
import shutil
import sys
import zipfile
from pathlib import Path

import py7zr
import requests

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_TOOLS_CONFIG = REPO_ROOT / ".github" / "ci-tools.json"
TOOLS_DIR = REPO_ROOT / "bin" / "tools"


def _download(url: str, dest: Path, description: str, *, max_retries: int = 4) -> None:
    print(f"[bundle-tools] Downloading {description}...")
    response = None
    for _attempt in range(1, max_retries + 1):
        response = requests.get(url, stream=True, timeout=60)
        response.raise_for_status()
        with open(dest, "wb") as f:
            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    f.write(chunk)
        print(f"[bundle-tools] Downloaded {dest.name}")
        return

    if response is None:
        raise RuntimeError(f"Failed to download {description}")
    response.raise_for_status()


def bundle_platform_tools(url: str) -> None:
    if (TOOLS_DIR / "adb.exe").exists() and (TOOLS_DIR / "fastboot.exe").exists():
        print("[bundle-tools] Platform tools already present, skipping.")
        return

    temp_zip = TOOLS_DIR / "platform-tools.zip"
    _download(url, temp_zip, "platform-tools")

    with zipfile.ZipFile(temp_zip, "r") as zf:
        for member in zf.infolist():
            if member.is_dir():
                continue
            if re.match(r"^platform-tools/[^/]+$", member.filename):
                file_name = Path(member.filename).name
                target = TOOLS_DIR / file_name
                with zf.open(member) as src, open(target, "wb") as dst:
                    shutil.copyfileobj(src, dst)
                print(f"[bundle-tools] Extracted {file_name}")

    temp_zip.unlink()


def bundle_avb_tools() -> None:
    """Copy AVB tools from vendor/avb submodule into bin/tools/ for packaging."""
    avb_dir = REPO_ROOT / "vendor" / "avb"
    copy_map = {
        avb_dir / "avbtool.py": TOOLS_DIR / "avbtool.py",
        avb_dir / "test" / "data" / "testkey_rsa4096.pem": TOOLS_DIR
        / "testkey_rsa4096.pem",
        avb_dir / "test" / "data" / "testkey_rsa2048.pem": TOOLS_DIR
        / "testkey_rsa2048.pem",
    }

    if all(dst.exists() for dst in copy_map.values()):
        print("[bundle-tools] AVB tools already present, skipping.")
        return

    for src, dst in copy_map.items():
        if not src.exists():
            raise RuntimeError(
                f"vendor/avb submodule missing {src.relative_to(REPO_ROOT)}. "
                f"Run: git submodule update --init vendor/avb"
            )
        shutil.copy2(src, dst)
        print(f"[bundle-tools] Copied {src.name} -> {dst.relative_to(REPO_ROOT)}")


def bundle_kptools(repo: str, asset_name: str) -> None:
    kptools_exe = TOOLS_DIR / "kptools.exe"
    if kptools_exe.exists():
        print("[bundle-tools] kptools already present, skipping.")
        return

    releases_url = f"https://api.github.com/repos/{repo}/releases"
    response = requests.get(releases_url, timeout=15)
    response.raise_for_status()
    releases = response.json()

    asset_url = None
    for release in releases:
        if release.get("draft"):
            continue
        for asset in release.get("assets", []):
            if asset_name in asset["name"]:
                asset_url = asset["browser_download_url"]
                break
        if asset_url:
            break

    if not asset_url:
        raise RuntimeError(f"kptools asset '{asset_name}' not found in {repo} releases")

    temp_7z = TOOLS_DIR / asset_name
    _download(asset_url, temp_7z, "kptools")

    try:
        with py7zr.SevenZipFile(temp_7z, mode="r") as z:
            z.extractall(path=TOOLS_DIR)
    finally:
        if temp_7z.exists():
            temp_7z.unlink()

    if not kptools_exe.exists():
        extracted_exe = next(TOOLS_DIR.rglob("kptools.exe"), None)
        if extracted_exe:
            exe_dir = extracted_exe.parent
            for item in exe_dir.iterdir():
                dest = TOOLS_DIR / item.name
                if dest.exists():
                    if dest.is_dir():
                        shutil.rmtree(dest)
                    else:
                        dest.unlink()
                shutil.move(str(item), str(TOOLS_DIR))
            try:
                exe_dir.rmdir()
            except OSError:
                pass
        else:
            raise RuntimeError("kptools.exe not found after extraction")

    print("[bundle-tools] kptools ready.")


def main() -> None:
    with open(CI_TOOLS_CONFIG, "r", encoding="utf-8") as f:
        config = json.load(f)

    TOOLS_DIR.mkdir(parents=True, exist_ok=True)

    tools = config["tools"]
    bundle_platform_tools(tools["platform_tools_url"])
    bundle_avb_tools()

    kp = config["kptools"]
    bundle_kptools(kp["repo"], kp["asset_name"])

    print("[bundle-tools] All tools bundled successfully.")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"[bundle-tools] FATAL: {e}", file=sys.stderr)
        sys.exit(1)
