from ltbox import main


def test_settings_store_ignores_unknown_and_invalid_updates(tmp_path):
    store = main.SettingsStore(tmp_path / "settings.json")

    updated = store.update(target_region="INVALID", language=123, unknown_key="x")

    assert updated.target_region == "PRC"
    assert updated.language is None
    assert updated.modify_region_code is True
    assert updated.preset_code == "1"
    assert updated.modify_rollback_index == "ON"
    assert store.load_raw() == {}


def test_settings_store_applies_valid_updates_from_validator_map(tmp_path):
    store = main.SettingsStore(tmp_path / "settings.json")

    store.update(
        language="ko",
        target_region="ROW",
        modify_region_code=False,
        preset_code="3",
        modify_rollback_index="AUTO",
    )

    loaded = store.load()
    assert loaded.language == "ko"
    assert loaded.target_region == "ROW"
    assert loaded.modify_region_code is False
    assert loaded.preset_code == "3"
    assert loaded.modify_rollback_index == "AUTO"


def test_settings_store_write_then_read_persistence(tmp_path):
    path = tmp_path / "settings.json"
    store1 = main.SettingsStore(path)
    store1.update(language="ru", target_region="ROW", preset_code="2")

    store2 = main.SettingsStore(path)
    loaded = store2.load()

    assert loaded.language == "ru"
    assert loaded.target_region == "ROW"
    assert loaded.preset_code == "2"


def test_settings_store_defaults_preset_when_missing(tmp_path):
    path = tmp_path / "settings.json"
    path.write_text(
        '{"target_region":"ROW","modify_region_code":false}',
        encoding="utf-8",
    )
    store = main.SettingsStore(path)

    loaded = store.load()

    assert loaded.preset_code == "1"
