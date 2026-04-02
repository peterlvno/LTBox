"""CI script: download platform-tools, AVB tools, and kptools into bin/tools/."""

import json
import re
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

import py7zr
import requests

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_TOOLS_CONFIG = REPO_ROOT / ".github" / "ci-tools.json"
TOOLS_DIR = REPO_ROOT / "bin" / "tools"


def _download(url: str, dest: Path, description: str) -> None:
    print(f"[bundle-tools] Downloading {description}...")
    response = requests.get(url, stream=True, timeout=60)
    response.raise_for_status()
    with open(dest, "wb") as f:
        for chunk in response.iter_content(chunk_size=8192):
            if chunk:
                f.write(chunk)
    print(f"[bundle-tools] Downloaded {dest.name}")


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


def bundle_avb_tools(url: str) -> None:
    avbtool = TOOLS_DIR / "avbtool.py"
    key1 = TOOLS_DIR / "testkey_rsa4096.pem"
    key2 = TOOLS_DIR / "testkey_rsa2048.pem"

    if avbtool.exists() and key1.exists() and key2.exists():
        print("[bundle-tools] AVB tools already present, skipping.")
        return

    extract_map = {
        "avbtool.py": avbtool,
        "test/data/testkey_rsa4096.pem": key1,
        "test/data/testkey_rsa2048.pem": key2,
    }

    temp_tar = TOOLS_DIR / "avb.tar.gz"
    try:
        _download(url, temp_tar, "AVB archive")
        _extract_archive(temp_tar, extract_map)
    finally:
        if temp_tar.exists():
            temp_tar.unlink()

    missing = [p.name for p in extract_map.values() if not p.exists()]
    if missing:
        raise RuntimeError(f"AVB extraction incomplete, missing: {missing}")


def _extract_archive(archive_path: Path, extract_map: dict[str, Path]) -> None:
    is_tar = archive_path.suffix == ".gz" or archive_path.suffix == ".tar"

    if is_tar:
        with tarfile.open(archive_path, "r:*") as tf:
            for member in tf:
                target = _resolve_target(member.name, extract_map)
                if target:
                    f = tf.extractfile(member)
                    if f:
                        with open(target, "wb") as dst:
                            shutil.copyfileobj(f, dst)
                        print(f"[bundle-tools] Extracted {target.name}")
    else:
        with zipfile.ZipFile(archive_path, "r") as zf:
            for zip_member in zf.infolist():
                target = _resolve_target(zip_member.filename, extract_map)
                if target:
                    with zf.open(zip_member) as src, open(target, "wb") as dst:
                        shutil.copyfileobj(src, dst)
                    print(f"[bundle-tools] Extracted {target.name}")


def _resolve_target(member_name: str, extract_map: dict[str, Path]) -> Path | None:
    normalized = member_name.lstrip("./")
    if normalized in extract_map:
        return extract_map[normalized]
    for rel_path, target_path in extract_map.items():
        if normalized.endswith(f"/{rel_path}"):
            return target_path
    return None


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
    bundle_avb_tools(tools["avb_archive_url"])

    kp = config["kptools"]
    bundle_kptools(kp["repo"], kp["asset_name"])

    print("[bundle-tools] All tools bundled successfully.")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"[bundle-tools] FATAL: {e}", file=sys.stderr)
        sys.exit(1)
