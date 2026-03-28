import json
import re
from pathlib import Path
from unittest.mock import patch

import pytest
from ltbox import i18n

BASE = Path(__file__).resolve().parents[2]
SRC = BASE / "bin" / "ltbox"
LANG = SRC / "lang"


def get_src_keys():
    keys = set()
    pat = re.compile(r'get_string\s*\(\s*["\']([^"\']+)["\']\s*\)')
    for f in SRC.rglob("*.py"):
        try:
            keys.update(pat.findall(f.read_text(encoding="utf-8")))
        except Exception:
            pass
    return keys


def load_langs():
    d = {}
    if not LANG.exists():
        return {}
    for f in LANG.glob("*.json"):
        try:
            with open(f, "r", encoding="utf-8") as fp:
                d[f.name] = set(json.load(fp).keys())
        except Exception:
            pytest.fail(f"Bad JSON {f.name}")
    return d


class TestI18n:
    @pytest.fixture(scope="class")
    def src_keys(self):
        return get_src_keys()

    @pytest.fixture(scope="class")
    def lang_map(self):
        return load_langs()

    def test_missing_keys(self, src_keys, lang_map):
        if not lang_map:
            pytest.skip("No lang files")
        for n, k in lang_map.items():
            missing = src_keys - k
            assert not missing, f"Missing in {n}: {missing}"

    def test_parity(self, lang_map):
        if not lang_map:
            pytest.skip("No lang files")
        base = "en.json" if "en.json" in lang_map else list(lang_map.keys())[0]
        base_k = lang_map[base]

        for n, k in lang_map.items():
            if n == base:
                continue
            diff = base_k - k
            assert not diff, f"{n} missing keys from {base}: {diff}"


def test_get_available_languages_reuses_cached_language_metadata(tmp_path):
    lang_dir = tmp_path / "lang"
    lang_dir.mkdir()
    (lang_dir / "en.json").write_text(
        json.dumps({"_lang": "English"}),
        encoding="utf-8",
    )

    i18n._load_available_languages.cache_clear()

    with (
        patch("ltbox.i18n.LANG_DIR", lang_dir),
        patch("ltbox.i18n.json.load", wraps=json.load) as mock_json_load,
    ):
        first = i18n.get_available_languages()
        second = i18n.get_available_languages()

    assert first == [("en", "English")]
    assert second == [("en", "English")]
    assert mock_json_load.call_count == 1


def test_get_available_languages_invalidates_cache_after_language_change(tmp_path):
    lang_dir = tmp_path / "lang"
    lang_dir.mkdir()
    lang_file = lang_dir / "en.json"
    lang_file.write_text(
        json.dumps({"_lang": "English"}),
        encoding="utf-8",
    )

    i18n._load_available_languages.cache_clear()

    with patch("ltbox.i18n.LANG_DIR", lang_dir):
        first = i18n.get_available_languages()
        lang_file.write_text(
            json.dumps({"_lang": "English (Updated)"}),
            encoding="utf-8",
        )
        second = i18n.get_available_languages()

    assert first == [("en", "English")]
    assert second == [("en", "English (Updated)")]
