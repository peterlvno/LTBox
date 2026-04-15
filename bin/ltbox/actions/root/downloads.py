import shutil
import zipfile
from pathlib import Path
from typing import Any, Dict, Optional

from ... import constants as const, downloader, utils
from ...errors import ToolError
from ...i18n import get_string
from ...root_profiles import RootProviderProfile

KERNEL_VERSION_RELEASE_MAP: Dict[str, str] = {
    "5.10": "android12-5.10",
    "5.15": "android13-5.15",
    "6.1": "android14-6.1",
    "6.6": "android15-6.6",
    "6.12": "android16-6.12",
}


def cleanup_manager_apk(show_message: bool = True) -> None:
    manager_apk = const.TOOLS_DIR / "manager.apk"
    if not manager_apk.exists():
        return

    if show_message:
        utils.ui.echo(get_string("act_cleanup_manager_apk"))

    try:
        manager_apk.unlink()
    except OSError:
        pass


def get_mapped_kernel_name(kernel_version: str) -> Optional[str]:
    if not kernel_version:
        return None

    major_minor = ".".join(kernel_version.split(".")[:2])
    return KERNEL_VERSION_RELEASE_MAP.get(major_minor)


def download_apatch_resources(
    *,
    profile: RootProviderProfile,
    staging_dir: Path,
    repo_config: Dict[str, Any],
    is_nightly: bool,
    workflow_id: Optional[str],
) -> bool:
    cleanup_manager_apk(show_message=False)
    utils.recreate_dir(staging_dir)

    try:
        if is_nightly and workflow_id:
            downloader.download_apatch_nightly(
                workflow_id,
                staging_dir,
                repo=repo_config.get("repo", ""),
                name=profile.display_name,
                workflow_file=profile.workflow_file,
                branch=profile.nightly_branch,
            )
        else:
            downloader.download_apatch_release(
                staging_dir,
                repo=repo_config.get("repo", ""),
                tag=repo_config.get("tag", "latest"),
                name=profile.display_name,
            )
        return True
    except (ToolError, OSError, zipfile.BadZipFile) as error:
        utils.ui.error(
            get_string("apatch_download_failed").format(
                e=error, name=profile.display_name
            )
        )
        return False


def _extract_first_matching_member(
    archive_path: Path,
    *,
    predicate,
    output_path: Path,
    missing_error_key: str,
) -> None:
    with zipfile.ZipFile(archive_path, "r") as archive:
        for member in archive.namelist():
            if not predicate(member):
                continue

            with archive.open(member) as source, open(output_path, "wb") as target:
                shutil.copyfileobj(source, target)
            return

    raise ToolError(get_string(missing_error_key))


def _download_lkm_nightly_artifacts(
    *,
    staging_dir: Path,
    repo_config: Dict[str, Any],
    workflow_id: str,
    workflow_file: str,
    branch: Optional[str],
    kernel_version: Optional[str],
    download_all_ksuinit: bool,
) -> bool:
    mapped_name = get_mapped_kernel_name(kernel_version or "")
    if not mapped_name:
        utils.ui.error(
            get_string("err_sukisu_kernel_map_not_found").format(ver=kernel_version)
        )
        return False

    repo = repo_config.get("repo", "")
    manager_name: str = repo_config.get("manager", "")

    try:
        temp_dl_dir = const.TOOLS_DIR / "dl_temp"
        utils.recreate_dir(temp_dl_dir)

        downloader.download_nightly_artifacts(
            repo=repo,
            workflow_id=workflow_id,
            manager_name=manager_name,
            mapped_name=mapped_name,
            target_dir=temp_dl_dir,
            download_all_ksuinit=download_all_ksuinit,
            manager_fallback_names=repo_config.get("manager_fallbacks"),
            workflow_file=workflow_file,
            branch=branch,
        )

        _extract_first_matching_member(
            temp_dl_dir / str(manager_name),
            predicate=lambda name: name.endswith(".apk"),
            output_path=const.TOOLS_DIR / "manager.apk",
            missing_error_key="act_err_manager_apk_not_found_zip",
        )

        utils.recreate_dir(staging_dir)
        _extract_first_matching_member(
            temp_dl_dir / "lkm.zip",
            predicate=lambda name: name.endswith("kernelsu.ko"),
            output_path=staging_dir / "kernelsu.ko",
            missing_error_key="act_err_kernelsu_ko_not_found_zip",
        )

        ksuinit_path = temp_dl_dir / "ksuinit"
        if ksuinit_path.exists():
            shutil.copy(ksuinit_path, staging_dir / "init")

        shutil.rmtree(temp_dl_dir)
        return True
    except (ToolError, zipfile.BadZipFile, OSError) as error:
        utils.ui.error(f"{error}")
        utils.ui.error(get_string("err_download_workflow"))
        return False


def download_magisk_resources(
    *,
    profile: RootProviderProfile,
    staging_dir: Path,
    repo_config: Dict[str, Any],
    is_nightly: bool,
    workflow_id: Optional[str],
    local_apk_path: Optional[Path] = None,
) -> bool:
    cleanup_manager_apk(show_message=False)
    utils.recreate_dir(staging_dir)

    repo = repo_config.get("repo", "")

    try:
        if local_apk_path is not None:
            downloader.prepare_magisk_apk(local_apk_path, staging_dir)
        elif is_nightly and workflow_id:
            downloader.download_magisk_nightly(
                workflow_id,
                staging_dir,
                repo=repo,
                workflow_file=profile.workflow_file,
                branch=profile.nightly_branch,
                name=profile.display_name,
            )
        else:
            tag = repo_config.get("tag", "latest")
            downloader.download_magisk_release(
                staging_dir,
                repo=repo,
                tag=tag,
                name=profile.display_name,
            )
        return True
    except (ToolError, OSError, zipfile.BadZipFile) as error:
        utils.ui.error(
            get_string("magisk_download_failed").format(
                e=error, name=profile.display_name
            )
        )
        return False


def download_lkm_resources(
    *,
    profile: RootProviderProfile,
    staging_dir: Path,
    repo_config: Dict[str, Any],
    kernel_version: Optional[str],
    is_nightly: bool,
    workflow_id: Optional[str],
    is_tagged_build: bool,
) -> bool:
    cleanup_manager_apk(show_message=False)

    repo = repo_config.get("repo", "")

    if profile.release_uses_tagged_build or profile.force_nightly or is_nightly:
        if is_nightly and workflow_id:
            resolved_workflow_id = workflow_id
        else:
            tag = repo_config.get("tag")
            try:
                if not repo:
                    raise ToolError(get_string("err_download_workflow"))
                resolved_workflow_id, resolved_tag = (
                    downloader.get_latest_tagged_workflow_run(repo, tag)
                )
                utils.ui.info(
                    get_string("act_using_tagged_run").format(
                        tag=resolved_tag, id=resolved_workflow_id
                    )
                )
            except (ToolError, ValueError) as error:
                utils.ui.error(f"{error}")
                utils.ui.error(get_string("err_download_workflow"))
                return False

        return _download_lkm_nightly_artifacts(
            staging_dir=staging_dir,
            repo_config=repo_config,
            workflow_id=resolved_workflow_id,
            workflow_file=profile.workflow_file if is_nightly else "",
            branch=profile.nightly_branch if is_nightly else None,
            kernel_version=kernel_version,
            download_all_ksuinit=is_tagged_build,
        )

    utils.recreate_dir(staging_dir)

    tag = repo_config.get("tag", "latest")
    downloader.download_ksu_manager_release(const.TOOLS_DIR, repo=repo, tag=tag)
    downloader.download_ksuinit_release(staging_dir / "init", repo=repo, tag=tag)
    if kernel_version:
        downloader.get_lkm_kernel_release(
            staging_dir / "kernelsu.ko",
            kernel_version,
            repo=repo,
            tag=tag,
        )
    return True
