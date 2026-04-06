"""CI script: download packaged tools into bin/tools/."""

import json
import re
import shutil
import sys
import tarfile
import time
import zipfile
from pathlib import Path

import py7zr
import requests

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_TOOLS_CONFIG = REPO_ROOT / ".github" / "ci-tools.json"
TOOLS_DIR = REPO_ROOT / "bin" / "tools"
OTATOOLS_LINUX_DIR = TOOLS_DIR / "otatools" / "linux"
UPDATE_ENGINE_DIR = TOOLS_DIR / "update_engine"
_CI_ANDROID_JS_VARS = re.compile(r"var JSVariables = (\{.*?\});", re.S)


def _download(
    url: str, dest: Path, description: str, *, max_retries: int = 4
) -> None:
    print(f"[bundle-tools] Downloading {description}...")
    for attempt in range(1, max_retries + 1):
        response = requests.get(url, stream=True, timeout=60)
        if response.status_code == 503 and attempt < max_retries:
            wait = 2**attempt
            print(
                f"[bundle-tools] 503 from server, retrying in {wait}s "
                f"(attempt {attempt}/{max_retries})..."
            )
            time.sleep(wait)
            continue
        response.raise_for_status()
        with open(dest, "wb") as f:
            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    f.write(chunk)
        print(f"[bundle-tools] Downloaded {dest.name}")
        return
    response.raise_for_status()


def _load_ci_android_variables(url: str) -> dict:
    response = requests.get(url, timeout=60)
    response.raise_for_status()
    match = _CI_ANDROID_JS_VARS.search(response.text)
    if not match:
        raise RuntimeError(f"Unable to parse ci.android.com metadata from {url}")
    return json.loads(match.group(1))


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


def _resolve_otatools_metadata(
    branch: str, target: str, artifact_name: str
) -> dict[str, str]:
    grid_url = f"https://ci.android.com/builds/branches/{branch}/grid"
    grid_data = _load_ci_android_variables(grid_url)

    build_id = None
    for build in grid_data.get("builds", []):
        for target_entry in build.get("targets", []):
            target_info = target_entry.get("target", {})
            if target_info.get("name") != target:
                continue
            if target_entry.get("successful") is True:
                build_id = target_entry.get("buildId") or build.get("buildId")
                break
        if build_id:
            break

    if not build_id:
        raise RuntimeError(
            f"No successful ci.android.com build found for {branch}/{target}"
        )

    artifact_page_url = (
        f"https://ci.android.com/builds/submitted/{build_id}/{target}/latest"
    )
    artifact_page = _load_ci_android_variables(artifact_page_url)
    artifact = next(
        (
            item
            for item in artifact_page.get("artifacts", [])
            if item.get("name") == artifact_name
        ),
        None,
    )
    if artifact is None:
        raise RuntimeError(
            f"{artifact_name} not found for ci.android.com build {build_id}/{target}"
        )

    return {
        "branch": branch,
        "target": target,
        "build_id": str(build_id),
        "artifact_name": artifact_name,
        "artifact_size": str(artifact.get("size", "")),
        "artifact_md5": str(artifact.get("md5", "")),
        "download_url": (
            f"https://ci.android.com/builds/submitted/{build_id}/{target}/latest/raw/{artifact_name}"
        ),
    }


def _resolve_otatools_member_target(member_name: str) -> Path | None:
    normalized = member_name.lstrip("./").replace("\\", "/")
    if not normalized:
        return None

    parts = [part for part in normalized.split("/") if part]
    if (
        len(parts) >= 2
        and parts[-2] == "bin"
        and parts[-1]
        in {
            "delta_generator",
            "lpmake",
            "lpdump",
            "lpunpack",
        }
    ):
        return Path("bin") / parts[-1]

    for anchor in ("lib64", "lib"):
        if anchor in parts:
            anchor_index = parts.index(anchor)
            tail = parts[anchor_index + 1 :]
            if tail:
                return Path(anchor).joinpath(*tail)

    return None


def bundle_otatools(branch: str, target: str, artifact_name: str) -> None:
    bundled_lpmake = OTATOOLS_LINUX_DIR / "bin" / "lpmake"
    bundled_delta_generator = OTATOOLS_LINUX_DIR / "bin" / "delta_generator"
    metadata_path = OTATOOLS_LINUX_DIR / "otatools-metadata.json"
    if bundled_lpmake.exists() and bundled_delta_generator.exists() and metadata_path.exists():
        print("[bundle-tools] otatools already present, skipping.")
        return

    metadata = _resolve_otatools_metadata(branch, target, artifact_name)
    temp_zip = TOOLS_DIR / artifact_name
    extracted_files: list[str] = []

    try:
        _download(
            metadata["download_url"],
            temp_zip,
            f"otatools ({metadata['build_id']}/{target})",
        )

        if OTATOOLS_LINUX_DIR.exists():
            shutil.rmtree(OTATOOLS_LINUX_DIR)
        OTATOOLS_LINUX_DIR.mkdir(parents=True, exist_ok=True)

        with zipfile.ZipFile(temp_zip, "r") as zf:
            for member in zf.infolist():
                if member.is_dir():
                    continue
                relative_target = _resolve_otatools_member_target(member.filename)
                if relative_target is None:
                    continue
                destination = OTATOOLS_LINUX_DIR / relative_target
                destination.parent.mkdir(parents=True, exist_ok=True)
                with zf.open(member) as src, open(destination, "wb") as dst:
                    shutil.copyfileobj(src, dst)
                extracted_files.append(relative_target.as_posix())
                print(f"[bundle-tools] Extracted {relative_target.as_posix()}")
    finally:
        if temp_zip.exists():
            temp_zip.unlink()

    missing_bins = []
    for required_bin in (bundled_lpmake, bundled_delta_generator):
        if not required_bin.exists():
            missing_bins.append(required_bin.relative_to(OTATOOLS_LINUX_DIR).as_posix())
    if missing_bins:
        raise RuntimeError(
            f"otatools extraction incomplete, missing: {', '.join(missing_bins)}"
        )

    metadata["extracted_files"] = extracted_files
    metadata_path.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    print("[bundle-tools] otatools ready.")


def bundle_update_engine_scripts(url: str) -> None:
    package_init = UPDATE_ENGINE_DIR / "scripts" / "update_payload" / "__init__.py"
    update_metadata_pb2 = UPDATE_ENGINE_DIR / "scripts" / "update_metadata_pb2.py"
    if package_init.exists() and update_metadata_pb2.exists():
        print("[bundle-tools] update_engine scripts already present, skipping.")
        return

    extract_map = {
        "scripts/update_metadata_pb2.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_metadata_pb2.py",
        "scripts/update_payload/__init__.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "__init__.py",
        "scripts/update_payload/checker.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "checker.py",
        "scripts/update_payload/common.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "common.py",
        "scripts/update_payload/error.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "error.py",
        "scripts/update_payload/format_utils.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "format_utils.py",
        "scripts/update_payload/histogram.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "histogram.py",
        "scripts/update_payload/payload.py": UPDATE_ENGINE_DIR
        / "scripts"
        / "update_payload"
        / "payload.py",
    }

    temp_tar = TOOLS_DIR / "update_engine.tar.gz"
    try:
        _download(url, temp_tar, "update_engine scripts archive")
        _extract_archive(temp_tar, extract_map)
    finally:
        if temp_tar.exists():
            temp_tar.unlink()

    missing = [
        path.relative_to(UPDATE_ENGINE_DIR).as_posix()
        for path in extract_map.values()
        if not path.exists()
    ]
    if missing:
        raise RuntimeError(f"update_engine extraction incomplete, missing: {missing}")

    metadata = {
        "archive_url": url,
        "extracted_files": [
            path.relative_to(UPDATE_ENGINE_DIR).as_posix()
            for path in extract_map.values()
        ],
    }
    metadata_path = UPDATE_ENGINE_DIR / "update-engine-metadata.json"
    metadata_path.parent.mkdir(parents=True, exist_ok=True)
    metadata_path.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    print("[bundle-tools] update_engine scripts ready.")


def _extract_archive(archive_path: Path, extract_map: dict[str, Path]) -> None:
    is_tar = archive_path.suffix == ".gz" or archive_path.suffix == ".tar"

    if is_tar:
        with tarfile.open(archive_path, "r:*") as tf:
            for member in tf:
                target = _resolve_target(member.name, extract_map)
                if target:
                    target.parent.mkdir(parents=True, exist_ok=True)
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
                        target.parent.mkdir(parents=True, exist_ok=True)
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
    bundle_update_engine_scripts(config["update_engine"]["archive_url"])
    otatools = config["otatools"]
    bundle_otatools(
        otatools["branch"],
        otatools["target"],
        otatools["artifact_name"],
    )

    kp = config["kptools"]
    bundle_kptools(kp["repo"], kp["asset_name"])

    print("[bundle-tools] All tools bundled successfully.")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"[bundle-tools] FATAL: {e}", file=sys.stderr)
        sys.exit(1)
