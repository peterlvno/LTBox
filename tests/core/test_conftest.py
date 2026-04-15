from unittest.mock import patch

from tests import conftest


class TestIntegrationToolBootstrap:
    def test_integration_tools_ready_true_when_all_downloaded_tools_exist(
        self, tmp_path
    ):
        tools_dir = tmp_path / "bin" / "tools"
        tools_dir.mkdir(parents=True)
        for name in conftest.INTEGRATION_TOOL_FILES:
            (tools_dir / name).write_text("stub", encoding="utf-8")

        with patch.object(conftest, "ROOT", tmp_path):
            assert conftest._integration_tools_ready() is True

    def test_integration_tools_ready_false_when_no_tools_exist(self, tmp_path):
        tools_dir = tmp_path / "bin" / "tools"
        tools_dir.mkdir(parents=True)

        with patch.object(conftest, "ROOT", tmp_path):
            assert conftest._integration_tools_ready() is False
