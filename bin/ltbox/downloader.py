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
            mininterval=1.0,
            maxinterval=5.0,
            bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}]",
        ) as pbar:
            for chunk in response.iter_bytes(chunk_size=32768):
                if chunk:
                    file.write(chunk)
                    pbar.update(len(chunk))
    else:
        for chunk in response.iter_bytes(chunk_size=32768):
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
        is_tar = tarfile.is_tarfile(archive_path)

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


class _DownloadResult(NamedTuple):
    path: Path
    original_name: str


def _download_and_move_github_asset(
    repo_url: str, tag: str, asset_pattern: str, target_file: Path
) -> _DownloadResult:
    downloaded_path = _download_github_asset(
        repo_url, tag, asset_pattern, target_file.parent
    )
    original_name = downloaded_path.name
    moved = _move_downloaded_file(downloaded_path, target_file)
    return _DownloadResult(moved, original_name)


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


def _normalize_workflow_artifact_name(name: str) -> str:
    normalized = name.strip().lower()
    for suffix in (".zip", ".apk"):
        if normalized.endswith(suffix):
            normalized = normalized[: -len(suffix)]
    return normalized


def _artifact_download_name(name: str) -> str:
    return name if name.lower().endswith(".zip") else f"{name}.zip"


def _resolve_workflow_artifact_name(
    artifact_names: list[str], *candidates: str
) -> Optional[str]:
    normalized_artifacts = {
        _normalize_workflow_artifact_name(name): name for name in artifact_names
    }
    for candidate in candidates:
        if not candidate:
            continue
        resolved = normalized_artifacts.get(
            _normalize_workflow_artifact_name(candidate)
        )
        if resolved:
            return resolved
    return None


def _resolve_ksuinit_artifact_names(
    artifact_names: list[str],
    *,
    ksuinit_variants: Optional[list[str]] = None,
    download_all_ksuinit: bool = False,
) -> list[str]:
    if download_all_ksuinit:
        candidates = [
            name
            for name in artifact_names
            if _normalize_workflow_artifact_name(name).startswith("ksuinit")
        ]
    else:
        resolved_candidates: list[str] = []
        for variant in ksuinit_variants or ["ksuinit"]:
            resolved = _resolve_workflow_artifact_name(artifact_names, variant)
            if resolved and resolved not in resolved_candidates:
                resolved_candidates.append(resolved)
        candidates = resolved_candidates

    preferred = ["ksuinit", "ksuinit-aarch64-linux-android"]
    preferred_keys = [_normalize_workflow_artifact_name(name) for name in preferred]
    return sorted(
        candidates,
        key=lambda name: (
            preferred_keys.index(_normalize_workflow_artifact_name(name))
            if _normalize_workflow_artifact_name(name) in preferred_keys
            else len(preferred_keys)
        ),
    )


def _select_apatch_artifact_name(
    artifact_names: list[str], provider_name: str
) -> Optional[str]:
    normalized_prefix = provider_name.strip().lower()
    matches = [
        artifact_name
        for artifact_name in artifact_names
        if _normalize_workflow_artifact_name(artifact_name).startswith(
            normalized_prefix
        )
    ]
    if not matches:
        return None

    def _artifact_rank(artifact_name: str) -> tuple[int, int, int, str]:
        normalized_name = _normalize_workflow_artifact_name(artifact_name)
        return (
            0 if "release" in normalized_name else 1,
            0 if "signed" in normalized_name else 1,
            1 if "debug" in normalized_name else 2,
            normalized_name,
        )

    return min(matches, key=_artifact_rank)


def _get_matching_workflow_artifacts(
    repo: str,
    workflow_id: str,
    *,
    workflow_file: str,
    branch: Optional[str] = None,
) -> list[str]:
    client = _github_client(repo)
    if not client.workflow_run_matches(workflow_id, workflow_file, branch=branch):
        branch_label = branch or "any branch"
        raise ToolError(
            f"Workflow {workflow_id} does not match {workflow_file} on {branch_label}."
        )
    return client.workflow_run_artifacts(workflow_id)


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


def get_latest_successful_workflow_run(
    repo: str, workflow_file: str, branch: Optional[str] = None
) -> Optional[str]:
    return _github_client(repo).latest_successful_workflow_run(
        workflow_file, branch=branch
    )


def extract_kernel_from_anykernel3_zip(zip_path: Path, work_dir: Path) -> Path:
    """Extract a kernel binary from a user-provided zip."""
    utils.ui.echo(get_string("gki_custom_extracting").format(filename=zip_path.name))

    extracted_kernel_dir = work_dir / "extracted_kernel"
    if extracted_kernel_dir.exists():
        shutil.rmtree(extracted_kernel_dir)

    try:
        with zipfile.ZipFile(zip_path, "r") as zip_ref:
            zip_ref.extractall(extracted_kernel_dir)
    except zipfile.BadZipFile as e:
        raise ToolError(
            get_string("gki_custom_bad_zip").format(filename=zip_path.name)
        ) from e

    kernel_image = _find_kernel_binary(extracted_kernel_dir)
    if kernel_image is None:
        utils.ui.echo(get_string("dl_gki_image_missing"))
        raise ToolError(get_string("dl_gki_image_missing"))

    utils.ui.echo(get_string("dl_gki_extract_ok"))
    return kernel_image


def _find_kernel_binary(extracted_kernel_dir: Path) -> Optional[Path]:
    for candidate_name in ("Image", "kernel"):
        exact_match = extracted_kernel_dir / candidate_name
        if exact_match.exists():
            return exact_match

        matches = sorted(
            (
                path
                for path in extracted_kernel_dir.rglob(candidate_name)
                if path.is_file()
            ),
            key=lambda path: (len(path.parts), str(path).lower()),
        )
        if matches:
            return matches[0]

    return None


def _download_manager_artifact(
    base_url: str,
    target_dir: Path,
    manager_name: str,
    manager_fallback_names: Optional[list[str]] = None,
    artifact_names: Optional[list[str]] = None,
) -> None:
    manager_zip = target_dir / manager_name

    candidates = [manager_name]
    if manager_fallback_names:
        candidates.extend(
            name for name in manager_fallback_names if name not in candidates
        )

    resolved_candidates = candidates
    if artifact_names is not None:
        resolved_candidates = []
        for candidate in candidates:
            resolved = _resolve_workflow_artifact_name(artifact_names, candidate)
            if resolved and resolved not in resolved_candidates:
                resolved_candidates.append(resolved)

    for candidate in resolved_candidates:
        download_name = _artifact_download_name(candidate)
        candidate_url = f"{base_url}/{download_name}"
        candidate_path = target_dir / download_name
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
    target_dir: Path,
    ksuinit_variants: Optional[list[str]] = None,
    download_all_ksuinit: bool = False,
    artifact_names: Optional[list[str]] = None,
) -> None:
    ksuinit_dest = target_dir / "ksuinit"

    candidates = (
        _resolve_ksuinit_artifact_names(
            artifact_names,
            ksuinit_variants=ksuinit_variants,
            download_all_ksuinit=download_all_ksuinit,
        )
        if artifact_names is not None
        else (ksuinit_variants or ["ksuinit"])
    )

    preferred = ["ksuinit", "ksuinit-aarch64-linux-android"]
    preferred_keys = [_normalize_workflow_artifact_name(name) for name in preferred]
    candidates.sort(
        key=lambda name: (
            preferred_keys.index(_normalize_workflow_artifact_name(name))
            if _normalize_workflow_artifact_name(name) in preferred_keys
            else len(preferred_keys)
        )
    )

    downloaded = False
    for variant in candidates:
        download_name = _artifact_download_name(variant)
        ksuinit_url = f"{base_url}/{download_name}"
        temp_zip = target_dir / f"temp_{download_name}"
        try:
            download_resource(ksuinit_url, temp_zip)

            with zipfile.ZipFile(temp_zip, "r") as zf:
                for member in zf.namelist():
                    if member.endswith("ksuinit"):
                        with zf.open(member) as src, open(ksuinit_dest, "wb") as dst:
                            shutil.copyfileobj(src, dst)
                        downloaded = True
                        if download_all_ksuinit:
                            variant_name = _normalize_workflow_artifact_name(
                                variant
                            ).replace("/", "_")
                            variant_dest = target_dir / f"{variant_name}.ksuinit"
                            shutil.copy2(ksuinit_dest, variant_dest)
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
    workflow_file: str = "",
    branch: Optional[str] = None,
):
    base_url = f"https://nightly.link/{repo}/actions/runs/{workflow_id}"

    manager_zip = target_dir / manager_name
    ksuinit_dest = target_dir / "ksuinit"
    lkm_dest = target_dir / "lkm.zip"

    with utils.ui.status(
        get_string("dl_fetching_workflow_artifacts").format(workflow_id=workflow_id)
    ):
        try:
            artifact_names = _get_matching_workflow_artifacts(
                repo,
                workflow_id,
                workflow_file=workflow_file,
                branch=branch,
            )
            _download_manager_artifact(
                base_url,
                target_dir,
                manager_name,
                manager_fallback_names,
                artifact_names=artifact_names,
            )
            _download_ksuinit_artifact(
                base_url,
                target_dir,
                ksuinit_variants,
                download_all_ksuinit,
                artifact_names=artifact_names,
            )
            lkm_artifact = _resolve_workflow_artifact_name(
                artifact_names, f"{mapped_name}-lkm"
            )
            if not lkm_artifact:
                raise ToolError(f"Missing workflow artifact: {mapped_name}-lkm")

            download_resource(
                f"{base_url}/{_artifact_download_name(lkm_artifact)}", lkm_dest
            )

        except (ToolError, httpx.HTTPError, zipfile.BadZipFile, OSError) as e:
            _cleanup_files(manager_zip, ksuinit_dest, lkm_dest)
            raise e

    utils.ui.echo(
        get_string("dl_download_success").format(filename=f"Workflow {workflow_id}")
    )


def download_ksu_manager_release(
    target_dir: Path, repo: str = "", tag: str = ""
) -> None:
    target_file = target_dir / "manager.apk"

    repo_url = f"https://github.com/{repo or const.KSU_APK_REPO}"
    resolved_tag = tag or const.KSU_APK_TAG

    with utils.ui.status(get_string("dl_ksu_downloading")):
        try:
            result = _download_and_move_github_asset(
                repo_url, resolved_tag, ".*spoofed.*\\.apk", target_file
            )
        except ToolError:
            try:
                result = _download_and_move_github_asset(
                    repo_url, resolved_tag, ".*\\.apk", target_file
                )
            except ToolError as e:
                utils.ui.error(get_string("dl_err_ksu_download").format(e=e))
                raise

    utils.ui.echo(
        get_string("dl_download_success").format(filename=result.original_name)
    )


def download_ksuinit_release(target_path: Path, repo: str = "", tag: str = "") -> None:
    if target_path.exists():
        target_path.unlink()

    owner_repo = _get_owner_repo(f"https://github.com/{repo or const.KSU_APK_REPO}")
    resolved_tag = _resolve_release_tag(owner_repo, tag or const.KSU_APK_TAG)
    workflow_id = _get_workflow_run_id_for_tag(owner_repo, resolved_tag)
    base_url = f"https://nightly.link/{owner_repo}/actions/runs/{workflow_id}"
    artifact_names = _get_workflow_run_artifacts(owner_repo, workflow_id)
    ksuinit_artifacts = _resolve_ksuinit_artifact_names(
        artifact_names,
        ksuinit_variants=["ksuinit", "ksuinit-aarch64-linux-android"],
    )
    if not ksuinit_artifacts:
        raise ToolError(get_string("dl_err_ksuinit_not_found"))

    download_name = _artifact_download_name(ksuinit_artifacts[0])
    temp_zip = target_path.parent / download_name

    with utils.ui.status(get_string("dl_downloading").format(filename="ksuinit.zip")):
        try:
            download_resource(f"{base_url}/{download_name}", temp_zip)
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

    utils.ui.echo(get_string("dl_download_success").format(filename="ksuinit.zip"))


def get_lkm_kernel_release(
    target_path: Path, kernel_version: str, repo: str = "", tag: str = ""
) -> None:
    if not kernel_version:
        raise ToolError(get_string("err_req_kernel_ver_lkm"))

    utils.ui.echo(get_string("dl_lkm_kver_found").format(ver=kernel_version))

    asset_pattern_regex = f"android.*-{kernel_version}_kernelsu.ko"

    with utils.ui.status(get_string("dl_lkm_downloading")):
        try:
            result = _download_and_move_github_asset(
                f"https://github.com/{repo or const.KSU_APK_REPO}",
                tag or const.KSU_APK_TAG,
                asset_pattern_regex,
                target_path,
            )
        except (ToolError, OSError) as e:
            utils.ui.error(
                get_string("dl_lkm_download_fail").format(asset=asset_pattern_regex)
            )
            raise ToolError(str(e))

    utils.ui.echo(
        get_string("dl_download_success").format(filename=result.original_name)
    )


def download_apatch_release(
    target_dir: Path, repo: str = "", tag: str = "latest", name: str = "APatch"
):
    apk_path = target_dir / "FolkPatch.apk"

    with utils.ui.status(get_string("dl_apatch_stable_downloading").format(name=name)):
        result = _download_and_move_github_asset(
            repo or const.FOLKPATCH_REPO,
            tag or const.FOLKPATCH_TAG,
            r".*\.apk$",
            apk_path,
        )

    utils.ui.echo(
        get_string("dl_download_success").format(filename=result.original_name)
    )
    _extract_apatch_kpimg(apk_path, target_dir)


def download_apatch_nightly(
    workflow_id: str,
    target_dir: Path,
    repo: str = "",
    name: str = "APatch",
    workflow_file: str = "",
    branch: Optional[str] = None,
):
    repo = repo or const.FOLKPATCH_REPO
    artifact_names = _get_matching_workflow_artifacts(
        repo,
        workflow_id,
        workflow_file=workflow_file,
        branch=branch,
    )

    target_artifact = _select_apatch_artifact_name(artifact_names, name)
    if not target_artifact:
        raise ToolError(
            get_string("dl_err_apatch_artifact_missing").format(
                name=name, workflow_id=workflow_id, artifacts=artifact_names
            )
        )

    download_name = _artifact_download_name(target_artifact)
    base_url = f"https://nightly.link/{repo}/actions/runs/{workflow_id}/{download_name}"
    temp_zip = target_dir / download_name

    with utils.ui.status(
        get_string("dl_apatch_nightly_downloading").format(
            name=name, workflow_id=workflow_id
        )
    ):
        try:
            download_resource(base_url, temp_zip)
            apk_path = target_dir / "FolkPatch.apk"
            with zipfile.ZipFile(temp_zip, "r") as zf:
                apk_member = next(
                    (m for m in zf.namelist() if m.endswith(".apk")), None
                )
                if not apk_member:
                    raise ToolError(get_string("dl_err_apatch_apk_missing_in_nightly"))
                with zf.open(apk_member) as src, open(apk_path, "wb") as dst:
                    shutil.copyfileobj(src, dst)
        finally:
            _cleanup_files(temp_zip)

    utils.ui.echo(
        get_string("dl_download_success").format(filename=f"{target_artifact}.apk")
    )
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


# -- Magisk APK download & binary extraction --

_MAGISK_APK_BINARIES = {
    "lib/arm64-v8a/libmagiskinit.so": "magiskinit",
    "lib/arm64-v8a/libmagisk.so": "magisk",
    "lib/arm64-v8a/libinit-ld.so": "init-ld",
}

_MAGISK_APK_ASSETS = {
    "assets/stub.apk": "stub.apk",
}


def prepare_magisk_apk(apk_path: Path, target_dir: Path) -> None:
    _extract_magisk_binaries(apk_path, target_dir)

    manager_apk = const.TOOLS_DIR / "manager.apk"
    shutil.copy(apk_path, manager_apk)


def download_magisk_release(
    target_dir: Path,
    repo: str = "",
    tag: str = "latest",
    name: str = "Magisk",
) -> None:
    apk_path = target_dir / f"{name.replace(' ', '_')}.apk"
    repo = repo or "topjohnwu/Magisk"

    with utils.ui.status(get_string("dl_apatch_stable_downloading").format(name=name)):
        _download_and_move_github_asset(
            f"https://github.com/{repo}",
            tag,
            r"^(?!.*app-debug).*\.apk$",
            apk_path,
        )

    utils.ui.echo(get_string("dl_download_success").format(filename=apk_path.name))
    prepare_magisk_apk(apk_path, target_dir)


def download_magisk_nightly(
    workflow_id: str,
    target_dir: Path,
    repo: str = "",
    workflow_file: str = "",
    branch: Optional[str] = None,
    name: str = "Magisk",
) -> None:
    repo = repo or "topjohnwu/Magisk"
    artifact_names = _get_matching_workflow_artifacts(
        repo,
        workflow_id,
        workflow_file=workflow_file,
        branch=branch,
    )

    # Look for an APK-producing artifact
    target_artifact = None
    for name in artifact_names:
        lower = name.lower()
        if "app-release" in lower or "magisk" in lower or "apk" in lower:
            target_artifact = name
            break
    if not target_artifact and artifact_names:
        target_artifact = artifact_names[0]
    if not target_artifact:
        raise ToolError(
            get_string("dl_err_apatch_artifact_missing").format(
                name=name, workflow_id=workflow_id, artifacts=artifact_names
            )
        )

    download_name = _artifact_download_name(target_artifact)
    base_url = f"https://nightly.link/{repo}/actions/runs/{workflow_id}/{download_name}"
    temp_zip = target_dir / download_name

    with utils.ui.status(
        get_string("dl_apatch_nightly_downloading").format(
            name=name, workflow_id=workflow_id
        )
    ):
        try:
            download_resource(base_url, temp_zip)
            apk_path = target_dir / f"{name.replace(' ', '_')}.apk"
            with zipfile.ZipFile(temp_zip, "r") as zf:
                namelist = zf.namelist()
                apk_member = None
                for candidate in ["apk-ng-release.apk", "app-release.apk"]:
                    if candidate in namelist:
                        apk_member = candidate
                        break
                if not apk_member:
                    apk_member = next((m for m in namelist if m.endswith(".apk")), None)
                if not apk_member:
                    raise ToolError(get_string("dl_err_apatch_apk_missing_in_nightly"))
                with zf.open(apk_member) as src, open(apk_path, "wb") as dst:
                    shutil.copyfileobj(src, dst)
        finally:
            _cleanup_files(temp_zip)

    utils.ui.echo(
        get_string("dl_download_success").format(filename=f"{target_artifact}.apk")
    )
    prepare_magisk_apk(apk_path, target_dir)


def _extract_magisk_binaries(apk_path: Path, target_dir: Path) -> None:
    utils.ui.echo(get_string("magisk_extracting_apk"))
    with zipfile.ZipFile(apk_path, "r") as zf:
        for zip_path, out_name in {
            **_MAGISK_APK_BINARIES,
            **_MAGISK_APK_ASSETS,
        }.items():
            out_file = target_dir / out_name
            try:
                with zf.open(zip_path) as src, open(out_file, "wb") as dst:
                    shutil.copyfileobj(src, dst)
            except KeyError:
                raise ToolError(
                    get_string("magisk_extract_missing").format(name=zip_path)
                )
