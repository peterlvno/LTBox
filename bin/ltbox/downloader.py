import re
import shutil
import tarfile
import zipfile
from pathlib import Path
from typing import IO, Dict, NamedTuple, Optional, Set

import httpx

try:
    from tqdm import tqdm
except ImportError:
    tqdm = None

from . import constants as const
from . import net, utils
from .errors import ToolError
from .github_client import GitHubClient
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


def _cleanup_files(*paths: Path) -> None:
    for path in paths:
        if path.exists():
            path.unlink()


def _move_downloaded_file(downloaded_path: Path, target_file: Path) -> Path:
    if downloaded_path.resolve() == target_file.resolve():
        return target_file
    if target_file.exists():
        target_file.unlink()
    shutil.move(str(downloaded_path), str(target_file))
    return target_file


def _github_client(repo_url: str) -> GitHubClient:
    return GitHubClient(_get_owner_repo(repo_url))


def _write_stream(
    response: "httpx.Response",
    file: "IO[bytes]",
    total_size: int,
    show_progress: bool,
) -> None:
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
            for chunk in response.iter_bytes(chunk_size=8192):
                if chunk:
                    file.write(chunk)
                    pbar.update(len(chunk))
    else:
        for chunk in response.iter_bytes(chunk_size=8192):
            if chunk:
                file.write(chunk)


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

            with open(dest_path, "wb") as f:
                _write_stream(response, f, total_size, show_progress)

        msg_success = get_string("dl_download_success").format(filename=dest_path.name)
        utils.ui.echo(msg_success)
    except (httpx.HTTPError, OSError) as e:
        msg_err = get_string("dl_download_failed").format(url=url, error=e)
        utils.ui.error(msg_err)
        if dest_path.exists():
            dest_path.unlink()
        raise ToolError(get_string("dl_err_download_tool").format(name=dest_path.name))


def _resolve_extract_target(
    member_name: str, extract_map: Dict[str, Path]
) -> Optional[Path]:
    normalized = member_name.lstrip("./")
    if ".." in normalized.split("/"):
        return None

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
                            with f, open(target_path, "wb") as target:
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


def _download_github_asset(
    repo_url: str, tag: str, asset_pattern: str, dest_dir: Path
) -> Path:
    try:
        client = _github_client(repo_url)
        release_data = client.fetch_release_data(tag, asset_pattern)
        target_asset = client.find_asset_by_pattern(release_data, asset_pattern)

        download_url = target_asset["browser_download_url"]
        filename = target_asset["name"]
        dest_path = dest_dir / filename

        download_resource(download_url, dest_path)
        return dest_path

    except ValueError as e:
        utils.ui.error(get_string("dl_err_check_network"))
        raise ToolError(get_string("dl_github_failed").format(e=e))


def _download_and_move_github_asset(
    repo_url: str, tag: str, asset_pattern: str, target_file: Path
) -> Path:
    downloaded_path = _download_github_asset(
        repo_url, tag, asset_pattern, target_file.parent
    )
    return _move_downloaded_file(downloaded_path, target_file)


def _get_latest_release_tag(owner_repo: str) -> str:
    return _github_client(owner_repo).latest_release_tag()


def _get_latest_tag_name(owner_repo: str) -> str:
    return _github_client(owner_repo).latest_tag_name()


def _resolve_release_tag(owner_repo: str, tag: Optional[str]) -> str:
    if not tag or tag.lower() == "latest":
        return _get_latest_release_tag(owner_repo)
    return tag


def _get_workflow_run_id_for_tag(owner_repo: str, tag: str) -> str:
    return _github_client(owner_repo).workflow_run_id_for_tag(tag)


def _get_workflow_run_artifacts(owner_repo: str, run_id: str) -> list[str]:
    return _github_client(owner_repo).workflow_run_artifacts(run_id)


class WorkflowRunInfo(NamedTuple):
    run_id: str
    resolved_tag: str


def get_latest_tagged_workflow_run(
    repo: str, tag: Optional[str] = None
) -> WorkflowRunInfo:
    owner_repo = _get_owner_repo(repo)
    resolved_tag = (
        _get_latest_tag_name(owner_repo) if not tag or tag.lower() == "latest" else tag
    )
    run_id = _get_workflow_run_id_for_tag(owner_repo, resolved_tag)
    return WorkflowRunInfo(run_id, resolved_tag)


def get_latest_successful_workflow_run(repo: str, workflow_file: str) -> Optional[str]:
    return _github_client(repo).latest_successful_workflow_run(workflow_file)


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
                _move_downloaded_file(candidate_path, manager_zip)
            return
        except (ToolError, httpx.HTTPError, OSError):
            _cleanup_files(candidate_path)
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
            _cleanup_files(temp_zip)

            if downloaded and not download_all_ksuinit:
                break
        except (ToolError, httpx.HTTPError, zipfile.BadZipFile, OSError):
            _cleanup_files(temp_zip)
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

    except (ToolError, httpx.HTTPError, zipfile.BadZipFile, OSError) as e:
        _cleanup_files(manager_zip, ksuinit_dest, lkm_dest)
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
        _cleanup_files(temp_zip)


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


def download_apatch_release(
    target_dir: Path, repo: str = "", tag: str = "latest", name: str = "APatch"
):
    utils.ui.echo(get_string("dl_apatch_stable_downloading").format(name=name))
    apk_path = target_dir / "FolkPatch.apk"
    _download_and_move_github_asset(
        repo or const.FOLKPATCH_REPO,
        tag or const.FOLKPATCH_TAG,
        r".*\.apk$",
        apk_path,
    )
    _extract_apatch_kpimg(apk_path, target_dir)


def download_apatch_nightly(
    workflow_id: str, target_dir: Path, repo: str = "", name: str = "APatch"
):
    utils.ui.echo(
        get_string("dl_apatch_nightly_downloading").format(
            name=name, workflow_id=workflow_id
        )
    )
    repo = repo or const.FOLKPATCH_REPO
    artifact_names = _get_workflow_run_artifacts(repo, workflow_id)

    target_artifact = next(
        (name for name in artifact_names if name.lower().startswith("folkpatch")), None
    )
    if not target_artifact:
        raise ToolError(
            get_string("dl_err_apatch_artifact_missing").format(
                name=name, workflow_id=workflow_id, artifacts=artifact_names
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
        _cleanup_files(temp_zip)

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
