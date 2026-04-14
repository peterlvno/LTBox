from ltbox.root_profiles import get_root_provider_profile


def test_root_provider_profiles_pin_nightly_workflow_file_and_branch():
    expected = {
        "kernelsu": ("build-manager.yml", "main"),
        "kernelsu-next": ("build-manager-ci.yml", "dev"),
        "sukisu": ("build-manager.yml", "main"),
        "resukisu": ("build-manager.yml", "main"),
        "apatch": ("build.yml", "main"),
        "folkpatch": ("build.yml", "main"),
        "magisk": ("ci.yml", "master"),
    }

    for provider_id, (workflow_file, branch) in expected.items():
        profile = get_root_provider_profile(provider_id)
        assert profile.workflow_file == workflow_file
        assert profile.nightly_branch == branch
