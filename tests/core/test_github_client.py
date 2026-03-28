from unittest.mock import MagicMock, patch

from ltbox.github_client import GitHubClient


def test_github_client_fetch_release_data_skips_testing_release_with_matching_asset():
    releases_response = MagicMock()
    releases_response.raise_for_status.return_value = None
    releases_response.json.return_value = [
        {
            "tag_name": "v3",
            "draft": False,
            "body": "Contains TESTING marker",
            "assets": [
                {
                    "name": "5.10-Normal-AnyKernel3.zip",
                    "browser_download_url": "http://testing",
                }
            ],
        },
        {
            "tag_name": "v1",
            "draft": False,
            "body": "Older stable release",
            "assets": [
                {
                    "name": "5.10-Normal-AnyKernel3.zip",
                    "browser_download_url": "http://stable-old",
                }
            ],
        },
    ]

    session = MagicMock()
    session.get.return_value = releases_response

    with patch("ltbox.github_client.net.get_session", return_value=session):
        release_data = GitHubClient(
            "WildKernels/GKI_KernelSU_SUSFS"
        ).fetch_release_data("latest", ".*Normal.*AnyKernel3\\.zip")

    assert release_data["tag_name"] == "v1"


def test_github_client_workflow_run_id_for_tag_falls_back_to_unfiltered_runs():
    filtered_runs_response = MagicMock()
    filtered_runs_response.raise_for_status.return_value = None
    filtered_runs_response.json.return_value = {"workflow_runs": []}

    all_runs_response = MagicMock()
    all_runs_response.raise_for_status.return_value = None
    all_runs_response.json.return_value = {
        "workflow_runs": [
            {"id": 42, "head_branch": "refs/tags/v1.2.3"},
        ]
    }

    session = MagicMock()
    session.get.side_effect = [filtered_runs_response, all_runs_response]

    with patch(
        "ltbox.github_client.net.get_session",
        return_value=session,
    ):
        run_id = GitHubClient("owner/repo").workflow_run_id_for_tag("v1.2.3")

    assert run_id == "42"
