import re
import shutil
import tarfile
import zipfile
from pathlib import Path
from typing import Dict, Optional, Set

import requests  # type: ignore[import-untyped]

try:
    from tqdm import tqdm
except ImportError:
    tqdm = None

from . import constants as const
from . import net, utils
from .errors import ToolError
from .i18n import get_string


def _get_owner_repo(repo_url: str) -> str:
    if "github.com/" in repo_url:
        return repo_url.split("github.com/")[-1]
    return repo_url


def _extract_zip_member(
    zip_file: zipfile.ZipFile, member: zipfile.ZipInfo, target_path: Path
) -> None:
    with zip_file.open(member) as source, open(target_path, "wb") as target:
        shutil.copyfileobj(source, target)


def download_resource(
    url: str,
    dest_path: Path,
    show_progress: bool = True,
    timeout: int = 30,
    retries: int = 3,
    backoff: float = 5,
) -> None:
    msg = get_string("dl_downloading").format(filename=dest_path.name)
    utils.ui.echo(msg)
    try:
        with net.request_with_retries(
            "GET",
            url,
            stream=True,
            timeout=timeout,
            retries=retries,
            backoff=backoff,
        ) as response:
            total_size = int(response.headers.get("content-length", 0))
            downloaded = 0

            with open(dest_path, "wb") as f:
                if show_progress and tqdm and total_size > 0:
                    with tqdm(
                        total=total_size,
                        unit="B",
                        unit_scale=True,
                        unit_divisor=1024,
                        leave=False,
                        ncols=80,
                        bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}]",
                    ) as pbar:
                        for chunk in response.iter_content(chunk_size=8192):
                            if chunk:
                                f.write(chunk)
                                downloaded += len(chunk)
                                pbar.update(len(chunk))
                else:
                    for chunk in response.iter_content(chunk_size=8192):
                        if chunk:
                            f.write(chunk)
                            downloaded += len(chunk)

        msg_success = get_string("dl_download_success").format(filename=dest_path.name)
        utils.ui.echo(msg_success)
    except (requests.RequestException, OSError) as e:
        msg_err = get_string("dl_download_failed").format(url=url, error=e)
        utils.ui.error(msg_err)
        if dest_path.exists():
            dest_path.unlink()
        raise ToolError(get_string("dl_err_download_tool").format(name=dest_path.name))


def _resolve_extract_target(
    member_name: str, extract_map: Dict[str, Path]
) -> Optional[Path]:
    normalized = member_name.lstrip("./")
    if normalized in extract_map:
        return extract_map[normalized]

    for relative_path, target_path in extract_map.items():
        if normalized.endswith(f"/{relative_path}"):
            return target_path

    return None


def extract_archive_files(
    archive_path: Path, extract_map: Dict[str, Path]
) -> Set[Path]:
    msg = get_string("dl_extracting").format(filename=archive_path.name)
    utils.ui.echo(msg)
    extracted_paths: Set[Path] = set()

    try:
        is_tar = archive_path.suffix == ".gz" or archive_path.suffix == ".tar"

        if is_tar:
            with tarfile.open(archive_path, "r:*") as tf:
                for member in tf:
                    target_path = _resolve_extract_target(member.name, extract_map)
                    if target_path:
                        f = tf.extractfile(member)
                        if f:
                            with open(target_path, "wb") as target:
                                shutil.copyfileobj(f, target)
                            extracted_paths.add(target_path)
                            utils.ui.echo(
                                get_string("dl_extracted_file").format(
                                    filename=target_path.name
                                )
                            )
        else:
            with zipfile.ZipFile(archive_path, "r") as zf:
                for zip_member in zf.infolist():
                    target_path = _resolve_extract_target(
                        zip_member.filename, extract_map
                    )
                    if target_path:
                        _extract_zip_member(zf, zip_member, target_path)
                        extracted_paths.add(target_path)
                        utils.ui.echo(
                            get_string("dl_extracted_file").format(
                                filename=target_path.name
                            )
                        )

    except (zipfile.BadZipFile, tarfile.TarError, OSError, IOError) as e:
        msg_err = get_string("dl_extract_failed").format(
            filename=archive_path.name, error=e
        )
        utils.ui.error(msg_err)
        raise ToolError(
            get_string("dl_err_extract_tool").format(name=archive_path.name)
        )

    return extracted_paths


def _find_non_testing_release(owner_repo: str, asset_pattern: str) -> Optional[dict]:
    response = requests.get(
        f"https://api.github.com/repos/{owner_repo}/releases",
        params={"per_page": 10},
    )
    response.raise_for_status()

    releases: list[dict] = []
    try:
        payload = response.json()
        if isinstance(payload, list):
            releases = payload
    except ValueError:
        releases = []

    if not releases:
        return None

    first_non_testing_index = None
    for index, release in enumerate(releases):
        if release.get("draft"):
            continue
        body = release.get("body") or ""
        if "TESTING" not in body:
            first_non_testing_index = index
            break

    if first_non_testing_index is not None:
        for release in releases[first_non_testing_index:]:
            if release.get("draft"):
                continue
            if any(
                re.match(asset_pattern, asset["name"])
                for asset in release.get("assets", [])
            ):
                return release

    return None


def _fetch_release_data(owner_repo: str, tag: str, asset_pattern: str) -> dict:
    if owner_repo.lower() == "wildkernels/gki_kernelsu_susfs" and (
        not tag or tag.lower() == "latest"
    ):
        release_data = _find_non_testing_release(owner_repo, asset_pattern)
        if release_data is not None:
            return release_data

    if not tag or tag.lower() == "latest":
        api_url = f"https://api.github.com/repos/{owner_repo}/releases/latest"
    else:
        api_url = f"https://api.github.com/repos/{owner_repo}/releases/tags/{tag}"

    response = requests.get(api_url)
    response.raise_for_status()
    return response.json()


def _find_asset_by_pattern(release_data: dict, asset_pattern: str) -> dict:
    target_asset = next(
        (
            asset
            for asset in release_data.get("assets", [])
            if re.match(asset_pattern, asset["name"])
        ),
        None,
    )
    if not target_asset:
        raise ToolError(get_string("dl_err_download_tool").format(name=asset_pattern))
    return target_asset


def _download_github_asset(
    repo_url: str, tag: str, asset_pattern: str, dest_dir: Path
) -> Path:
    from requests.exceptions import RequestException  # type: ignore[import-untyped]

    owner_repo = _get_owner_repo(repo_url)

    try:
        release_data = _fetch_release_data(owner_repo, tag, asset_pattern)
        target_asset = _find_asset_by_pattern(release_data, asset_pattern)

        download_url = target_asset["browser_download_url"]
        filename = target_asset["name"]
        dest_path = dest_dir / filename

        download_resource(download_url, dest_path)
        return dest_path

    except (RequestException, ValueError) as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))


def _download_and_move_github_asset(
    repo_url: str, tag: str, asset_pattern: str, target_file: Path
) -> Path:
    downloaded_path = _download_github_asset(
        repo_url, tag, asset_pattern, target_file.parent
    )
    if downloaded_path.resolve() != target_file.resolve():
        if target_file.exists():
            target_file.unlink()
        shutil.move(str(downloaded_path), str(target_file))
    return target_file


def _get_latest_release_tag(owner_repo: str) -> str:
    api_url = f"https://api.github.com/repos/{owner_repo}/releases/latest"
    try:
        response = requests.get(api_url, timeout=15)
        response.raise_for_status()
        release_data = response.json()
    except requests.RequestException as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))

    tag_name = release_data.get("tag_name")
    if not tag_name:
        raise ToolError(get_string("dl_err_latest_release_tag"))
    return tag_name


def _get_latest_tag_name(owner_repo: str) -> str:
    tags_url = f"https://api.github.com/repos/{owner_repo}/tags"
    try:
        response = requests.get(tags_url, params={"per_page": 1}, timeout=15)
        response.raise_for_status()
        tags = response.json()
        if tags:
            tag_name = tags[0].get("name")
            if tag_name:
                return tag_name
    except requests.RequestException as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))

    return _get_latest_release_tag(owner_repo)


def _resolve_release_tag(owner_repo: str, tag: Optional[str]) -> str:
    if not tag or tag.lower() == "latest":
        return _get_latest_release_tag(owner_repo)
    return tag


def _select_workflow_run_for_tag(runs: list[dict], tag: str) -> Optional[dict]:
    for run in runs:
        head_branch = run.get("head_branch") or ""
        if head_branch == tag or head_branch == f"refs/tags/{tag}":
            return run
    for run in runs:
        head_branch = run.get("head_branch") or ""
        if head_branch.endswith(f"/{tag}"):
            return run
    return None


def _get_workflow_run_id_for_tag(owner_repo: str, tag: str) -> str:
    api_url = f"https://api.github.com/repos/{owner_repo}/actions/runs"
    params: dict[str, str | int] = {
        "per_page": 30,
        "status": "completed",
        "branch": tag,
    }
    try:
        response = requests.get(api_url, params=params, timeout=15)
        response.raise_for_status()
        data = response.json()
        run = _select_workflow_run_for_tag(data.get("workflow_runs", []), tag)
        if run:
            return str(run["id"])

        response = requests.get(api_url, params={"per_page": 50}, timeout=15)
        response.raise_for_status()
        data = response.json()
        run = _select_workflow_run_for_tag(data.get("workflow_runs", []), tag)
        if run:
            return str(run["id"])
    except requests.RequestException as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))

    raise ToolError(get_string("dl_err_workflow_run_for_tag").format(tag=tag))


def _get_workflow_run_artifacts(owner_repo: str, run_id: str) -> list[str]:
    api_url = (
        f"https://api.github.com/repos/{owner_repo}/actions/runs/{run_id}/artifacts"
    )
    try:
        response = requests.get(api_url, timeout=15)
        response.raise_for_status()
        data = response.json()
    except requests.RequestException as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))

    artifacts = data.get("artifacts", [])
    return [artifact.get("name", "") for artifact in artifacts if artifact.get("name")]


def get_latest_tagged_workflow_run(
    repo: str, tag: Optional[str] = None
) -> tuple[str, str]:
    owner_repo = _get_owner_repo(repo)
    resolved_tag = (
        _get_latest_tag_name(owner_repo) if not tag or tag.lower() == "latest" else tag
    )
    run_id = _get_workflow_run_id_for_tag(owner_repo, resolved_tag)
    return run_id, resolved_tag


def get_gki_kernel(kernel_version: str, work_dir: Path) -> Path:
    utils.ui.echo(get_string("dl_gki_downloading"))

    try:
        tag = const.CONF._get_val("wildkernels", "tag", default="latest")
        owner = const.CONF._get_val("wildkernels", "owner")
        repo = const.CONF._get_val("wildkernels", "repo")
    except RuntimeError:
        tag = "latest"
        owner = const.RELEASE_OWNER
        repo = const.RELEASE_REPO

    if not tag:
        tag = "latest"
    repo_ref = f"{owner}/{repo}"

    asset_pattern = f"{re.escape(kernel_version)}.*Normal.*AnyKernel3\\.zip"

    try:
        anykernel_zip = work_dir / const.ANYKERNEL_ZIP_FILENAME
        _download_and_move_github_asset(repo_ref, tag, asset_pattern, anykernel_zip)

        utils.ui.echo(get_string("dl_gki_download_ok"))

        utils.ui.echo(get_string("dl_gki_extracting"))
        extracted_kernel_dir = work_dir / "extracted_kernel"
        if extracted_kernel_dir.exists():
            shutil.rmtree(extracted_kernel_dir)

        with zipfile.ZipFile(anykernel_zip, "r") as zip_ref:
            zip_ref.extractall(extracted_kernel_dir)

        kernel_image = extracted_kernel_dir / "Image"
        if not kernel_image.exists():
            utils.ui.echo(get_string("dl_gki_image_missing"))
            raise ToolError(get_string("dl_gki_image_missing"))

        utils.ui.echo(get_string("dl_gki_extract_ok"))
        return kernel_image

    except (ToolError, zipfile.BadZipFile, OSError) as e:
        utils.ui.echo(get_string("dl_gki_download_fail").format(version=tag))
        raise ToolError(str(e))


def _download_manager_artifact(
    base_url: str,
    target_dir: Path,
    manager_name: str,
    manager_fallback_names: Optional[list[str]] = None,
) -> None:
    manager_zip = target_dir / manager_name

    candidates = [manager_name]
    if manager_fallback_names:
        candidates.extend(
            name for name in manager_fallback_names if name not in candidates
        )

    for candidate in candidates:
        candidate_url = f"{base_url}/{candidate}"
        candidate_path = target_dir / candidate
        try:
            download_resource(candidate_url, candidate_path)
            if candidate_path != manager_zip:
                if manager_zip.exists():
                    manager_zip.unlink()
                shutil.move(candidate_path, manager_zip)
            return
        except (ToolError, requests.RequestException, OSError):
            if candidate_path.exists():
                candidate_path.unlink()
            continue

    raise ToolError(f"Failed to download manager artifact (tried: {candidates})")


def _download_ksuinit_artifact(
    base_url: str,
    repo: str,
    workflow_id: str,
    target_dir: Path,
    ksuinit_variants: Optional[list[str]] = None,
    download_all_ksuinit: bool = False,
) -> None:
    ksuinit_dest = target_dir / "ksuinit"

    artifact_names: list[str] = []
    if download_all_ksuinit:
        try:
            owner_repo = _get_owner_repo(repo)
            artifact_names = _get_workflow_run_artifacts(owner_repo, workflow_id)
        except ToolError:
            artifact_names = []

    candidates = (
        [name for name in artifact_names if name.startswith("ksuinit")]
        if artifact_names
        else (ksuinit_variants or ["ksuinit"])
    )

    preferred = ["ksuinit", "ksuinit-aarch64-linux-android"]
    candidates.sort(
        key=lambda name: preferred.index(name) if name in preferred else len(preferred)
    )

    downloaded = False
    for variant in candidates:
        ksuinit_url = f"{base_url}/{variant}.zip"
        temp_zip = target_dir / f"temp_{variant}.zip"
        try:
            download_resource(ksuinit_url, temp_zip)

            with zipfile.ZipFile(temp_zip, "r") as zf:
                for member in zf.namelist():
                    if member.endswith("ksuinit"):
                        with zf.open(member) as src, open(ksuinit_dest, "wb") as dst:
                            shutil.copyfileobj(src, dst)
                        downloaded = True
                        if download_all_ksuinit:
                            variant_name = variant.replace("/", "_")
                            variant_dest = target_dir / f"{variant_name}.ksuinit"
                            with (
                                zf.open(member) as src,
                                open(variant_dest, "wb") as dst,
                            ):
                                shutil.copyfileobj(src, dst)
                        break
            temp_zip.unlink()

            if downloaded and not download_all_ksuinit:
                break
        except (ToolError, requests.RequestException, zipfile.BadZipFile, OSError):
            if temp_zip.exists():
                temp_zip.unlink()
            continue

    if not downloaded:
        raise ToolError(get_string("dl_err_ksuinit_download_variants"))


def download_nightly_artifacts(
    repo: str,
    workflow_id: str,
    manager_name: str,
    mapped_name: str,
    target_dir: Path,
    ksuinit_variants: Optional[list[str]] = None,
    download_all_ksuinit: bool = False,
    manager_fallback_names: Optional[list[str]] = None,
):
    base_url = f"https://nightly.link/{repo}/actions/runs/{workflow_id}"

    manager_zip = target_dir / manager_name
    ksuinit_dest = target_dir / "ksuinit"
    lkm_dest = target_dir / "lkm.zip"

    utils.ui.info(
        get_string("dl_fetching_workflow_artifacts").format(workflow_id=workflow_id)
    )

    try:
        _download_manager_artifact(
            base_url, target_dir, manager_name, manager_fallback_names
        )
        _download_ksuinit_artifact(
            base_url,
            repo,
            workflow_id,
            target_dir,
            ksuinit_variants,
            download_all_ksuinit,
        )
        download_resource(f"{base_url}/{mapped_name}-lkm.zip", lkm_dest)

        utils.ui.echo(
            get_string("dl_download_success").format(filename="All Artifacts")
        )

    except (ToolError, requests.RequestException, zipfile.BadZipFile, OSError) as e:
        if manager_zip.exists():
            manager_zip.unlink()
        if ksuinit_dest.exists():
            ksuinit_dest.unlink()
        if lkm_dest.exists():
            lkm_dest.unlink()
        raise e


def download_ksu_manager_release(
    target_dir: Path, repo: str = "", tag: str = ""
) -> None:
    utils.ui.echo(get_string("dl_ksu_downloading"))
    target_file = target_dir / "manager.apk"

    repo_url = f"https://github.com/{repo or const.KSU_APK_REPO}"
    resolved_tag = tag or const.KSU_APK_TAG

    try:
        _download_and_move_github_asset(
            repo_url, resolved_tag, ".*spoofed.*\\.apk", target_file
        )
    except ToolError:
        try:
            _download_and_move_github_asset(
                repo_url, resolved_tag, ".*\\.apk", target_file
            )
        except ToolError as e:
            utils.ui.error(get_string("dl_err_ksu_download").format(e=e))
            return

    utils.ui.echo(get_string("dl_ksu_success"))


def download_ksuinit_release(target_path: Path, repo: str = "", tag: str = "") -> None:
    if target_path.exists():
        target_path.unlink()

    owner_repo = _get_owner_repo(f"https://github.com/{repo or const.KSU_APK_REPO}")
    resolved_tag = _resolve_release_tag(owner_repo, tag or const.KSU_APK_TAG)
    workflow_id = _get_workflow_run_id_for_tag(owner_repo, resolved_tag)
    base_url = f"https://nightly.link/{owner_repo}/actions/runs/{workflow_id}"
    temp_zip = target_path.parent / "ksuinit.zip"

    try:
        download_resource(f"{base_url}/ksuinit.zip", temp_zip)
        with zipfile.ZipFile(temp_zip, "r") as zf:
            ksuinit_member = None
            for member in zf.namelist():
                if member.endswith("ksuinit"):
                    ksuinit_member = member
                    break
            if not ksuinit_member:
                raise ToolError(get_string("dl_err_ksuinit_not_found"))

            with zf.open(ksuinit_member) as src, open(target_path, "wb") as dst:
                shutil.copyfileobj(src, dst)
    finally:
        if temp_zip.exists():
            temp_zip.unlink()


def get_lkm_kernel_release(
    target_path: Path, kernel_version: str, repo: str = "", tag: str = ""
) -> None:
    if not kernel_version:
        raise ToolError(get_string("err_req_kernel_ver_lkm"))

    utils.ui.echo(get_string("dl_lkm_kver_found").format(ver=kernel_version))

    asset_pattern_regex = f"android.*-{kernel_version}_kernelsu.ko"
    utils.ui.echo(get_string("dl_lkm_downloading").format(asset=asset_pattern_regex))

    try:
        _download_and_move_github_asset(
            f"https://github.com/{repo or const.KSU_APK_REPO}",
            tag or const.KSU_APK_TAG,
            asset_pattern_regex,
            target_path,
        )
        utils.ui.echo(get_string("dl_lkm_download_ok"))
    except (ToolError, OSError) as e:
        utils.ui.error(
            get_string("dl_lkm_download_fail").format(asset=asset_pattern_regex)
        )
        raise ToolError(str(e))


def download_apatch_release(target_dir: Path, repo: str = "", tag: str = "latest"):
    utils.ui.echo(get_string("dl_apatch_stable_downloading"))
    apk_path = target_dir / "FolkPatch.apk"
    _download_and_move_github_asset(
        repo or const.FOLKPATCH_REPO,
        tag or const.FOLKPATCH_TAG,
        r".*\.apk$",
        apk_path,
    )
    _extract_apatch_kpimg(apk_path, target_dir)


def download_apatch_nightly(workflow_id: str, target_dir: Path, repo: str = ""):
    utils.ui.echo(
        get_string("dl_apatch_nightly_downloading").format(workflow_id=workflow_id)
    )
    repo = repo or const.FOLKPATCH_REPO
    artifact_names = _get_workflow_run_artifacts(repo, workflow_id)

    target_artifact = next(
        (name for name in artifact_names if name.lower().startswith("folkpatch")), None
    )
    if not target_artifact:
        raise ToolError(
            get_string("dl_err_apatch_artifact_missing").format(
                workflow_id=workflow_id, artifacts=artifact_names
            )
        )

    base_url = (
        f"https://nightly.link/{repo}/actions/runs/{workflow_id}/{target_artifact}.zip"
    )
    temp_zip = target_dir / f"{target_artifact}.zip"

    try:
        download_resource(base_url, temp_zip)
        apk_path = target_dir / "FolkPatch.apk"
        with zipfile.ZipFile(temp_zip, "r") as zf:
            apk_member = next((m for m in zf.namelist() if m.endswith(".apk")), None)
            if not apk_member:
                raise ToolError(get_string("dl_err_apatch_apk_missing_in_nightly"))
            with zf.open(apk_member) as src, open(apk_path, "wb") as dst:
                shutil.copyfileobj(src, dst)
    finally:
        if temp_zip.exists():
            temp_zip.unlink()

    _extract_apatch_kpimg(apk_path, target_dir)


def _extract_apatch_kpimg(apk_path: Path, target_dir: Path):
    kpimg_path = target_dir / "kpimg"
    with zipfile.ZipFile(apk_path, "r") as zf:
        try:
            with zf.open("assets/kpimg") as src, open(kpimg_path, "wb") as dst:
                shutil.copyfileobj(src, dst)
            utils.ui.echo(get_string("dl_apatch_kpimg_extracted"))
        except KeyError:
            raise ToolError(get_string("dl_err_apatch_kpimg_missing"))

    manager_apk = const.TOOLS_DIR / "manager.apk"
    shutil.copy(apk_path, manager_apk)
