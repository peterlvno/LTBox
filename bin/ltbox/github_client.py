import re
from dataclasses import dataclass
from typing import Any, Optional

import httpx
from cachetools import TTLCache

from . import net, utils
from .errors import ToolError
from .i18n import get_string

GitHubPayload = dict[str, Any]

_api_cache: TTLCache[
    tuple[str, str, Optional[tuple[tuple[str, str | int], ...]]], Any
] = TTLCache(maxsize=64, ttl=300)


def _select_workflow_run_for_tag(
    runs: list[GitHubPayload],
    tag: str,
) -> Optional[GitHubPayload]:
    for run in runs:
        head_branch = run.get("head_branch") or ""
        if head_branch == tag or head_branch == f"refs/tags/{tag}":
            return run
    for run in runs:
        head_branch = run.get("head_branch") or ""
        if head_branch.endswith(f"/{tag}"):
            return run
    return None


@dataclass(frozen=True)
class GitHubClient:
    owner_repo: str

    def _request_json(
        self,
        path: str,
        *,
        params: Optional[dict[str, str | int]] = None,
        timeout: int = 15,
    ) -> Any:
        frozen_params = tuple(sorted(params.items())) if params else None
        cache_key = (self.owner_repo, path, frozen_params)
        cached = _api_cache.get(cache_key)
        if cached is not None:
            return cached

        api_url = f"https://api.github.com/repos/{self.owner_repo}/{path}"
        try:
            response = net.get_client().get(api_url, params=params, timeout=timeout)
            response.raise_for_status()
            data = response.json()
        except httpx.HTTPError as error:
            utils.ui.error(get_string("dl_err_check_network"))
            raise ToolError(get_string("dl_github_failed").format(e=error))

        _api_cache[cache_key] = data
        return data

    def _request_list(
        self,
        path: str,
        *,
        params: Optional[dict[str, str | int]] = None,
        timeout: int = 15,
    ) -> list[GitHubPayload]:
        try:
            payload = self._request_json(path, params=params, timeout=timeout)
        except ValueError:
            return []
        if isinstance(payload, list):
            return [item for item in payload if isinstance(item, dict)]
        return []

    def _request_object(
        self,
        path: str,
        *,
        params: Optional[dict[str, str | int]] = None,
        timeout: int = 15,
    ) -> GitHubPayload:
        payload = self._request_json(path, params=params, timeout=timeout)
        return payload if isinstance(payload, dict) else {}

    def find_non_testing_release_with_asset(
        self,
        asset_pattern: str,
    ) -> Optional[GitHubPayload]:
        releases = self._request_list("releases", params={"per_page": 10})
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

        if first_non_testing_index is None:
            return None

        for release in releases[first_non_testing_index:]:
            if release.get("draft"):
                continue
            if any(
                re.match(asset_pattern, asset["name"])
                for asset in release.get("assets", [])
            ):
                return release
        return None

    def fetch_release_data(
        self,
        tag: str,
        asset_pattern: str,
    ) -> GitHubPayload:
        if self.owner_repo.lower() == "wildkernels/gki_kernelsu_susfs" and (
            not tag or tag.lower() == "latest"
        ):
            release_data = self.find_non_testing_release_with_asset(asset_pattern)
            if release_data is not None:
                return release_data

        if not tag or tag.lower() == "latest":
            return self._request_object("releases/latest")
        return self._request_object(f"releases/tags/{tag}")

    @staticmethod
    def find_asset_by_pattern(
        release_data: GitHubPayload,
        asset_pattern: str,
    ) -> GitHubPayload:
        target_asset = next(
            (
                asset
                for asset in release_data.get("assets", [])
                if re.match(asset_pattern, asset["name"])
            ),
            None,
        )
        if not target_asset:
            raise ToolError(
                get_string("dl_err_download_tool").format(name=asset_pattern)
            )
        return target_asset

    def latest_release_tag(self) -> str:
        tag_name = self._request_object("releases/latest").get("tag_name")
        if not tag_name:
            raise ToolError(get_string("dl_err_latest_release_tag"))
        return str(tag_name)

    def latest_tag_name(self) -> str:
        tags = self._request_list("tags", params={"per_page": 1})
        if tags:
            tag_name = tags[0].get("name")
            if tag_name:
                return str(tag_name)
        return self.latest_release_tag()

    def workflow_run_id_for_tag(self, tag: str) -> str:
        runs = self._request_object(
            "actions/runs",
            params={"per_page": 30, "status": "completed", "branch": tag},
        ).get("workflow_runs", [])
        if isinstance(runs, list):
            run = _select_workflow_run_for_tag(runs, tag)
            if run:
                return str(run["id"])

        runs = self._request_object(
            "actions/runs",
            params={"per_page": 50},
        ).get("workflow_runs", [])
        if isinstance(runs, list):
            run = _select_workflow_run_for_tag(runs, tag)
            if run:
                return str(run["id"])

        raise ToolError(get_string("dl_err_workflow_run_for_tag").format(tag=tag))

    def workflow_run_artifacts(self, run_id: str) -> list[str]:
        artifacts = self._request_object(f"actions/runs/{run_id}/artifacts").get(
            "artifacts", []
        )
        if not isinstance(artifacts, list):
            return []
        return [
            artifact.get("name", "")
            for artifact in artifacts
            if isinstance(artifact, dict) and artifact.get("name")
        ]

    def latest_successful_workflow_run(self, workflow_file: str) -> Optional[str]:
        runs = self._request_object(
            f"actions/workflows/{workflow_file}/runs",
            params={"status": "success", "per_page": 1},
        ).get("workflow_runs", [])
        if isinstance(runs, list) and runs:
            return str(runs[0]["id"])
        return None
