import functools
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Tuple

APP_DIR = Path(__file__).parent.resolve()
LANG_DIR = APP_DIR / "lang"

_lang_data: Dict[str, Any] = {}
_fallback_data: Dict[str, Any] = {}
LanguageEntry = Tuple[str, str]
LanguageCacheKey = Tuple[str, Tuple[Tuple[str, int, int], ...]]


def _available_language_cache_key() -> LanguageCacheKey:
    if not LANG_DIR.is_dir():
        raise RuntimeError(f"Language directory not found: {LANG_DIR}")

    lang_files = sorted(LANG_DIR.glob("*.json"))
    if not lang_files:
        raise RuntimeError(f"No language files (*.json) found in: {LANG_DIR}")

    file_signature = []
    for lang_file in lang_files:
        stat_result = lang_file.stat()
        file_signature.append(
            (lang_file.name, stat_result.st_mtime_ns, stat_result.st_size)
        )
    return str(LANG_DIR.resolve()), tuple(file_signature)


@functools.lru_cache(maxsize=4)
def _load_available_languages(cache_key: LanguageCacheKey) -> Tuple[LanguageEntry, ...]:
    lang_dir = Path(cache_key[0])
    languages: List[LanguageEntry] = []

    for file_name, _, _ in cache_key[1]:
        lang_file = lang_dir / file_name
        lang_code = lang_file.stem
        try:
            with open(lang_file, "r", encoding="utf-8") as handle:
                temp_lang = json.load(handle)
                lang_name = temp_lang.get("_lang", lang_code)
                languages.append((lang_code, lang_name))
        except (json.JSONDecodeError, OSError):
            languages.append((lang_code, lang_code))

    languages.sort(key=lambda item: (0 if item[0] == "en" else 1, item[1].lower()))
    return tuple(languages)


def get_available_languages() -> List[LanguageEntry]:
    return list(_load_available_languages(_available_language_cache_key()))


def load_lang(lang_code: str = "en"):
    global _lang_data, _fallback_data

    fallback_file = LANG_DIR / "en.json"
    if not _fallback_data and fallback_file.exists():
        try:
            with open(fallback_file, "r", encoding="utf-8") as f:
                _fallback_data = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            print(f"[!] Failed to load fallback language en.json: {e}", file=sys.stderr)
            _fallback_data = {}

    if lang_code == "en" or not (LANG_DIR / f"{lang_code}.json").exists():
        _lang_data = _fallback_data
    else:
        lang_file = LANG_DIR / f"{lang_code}.json"
        try:
            with open(lang_file, "r", encoding="utf-8") as f:
                _lang_data = json.load(f)
        except (json.JSONDecodeError, OSError) as e:
            print(
                f"[!] Failed to load language {lang_code}, using fallback: {e}",
                file=sys.stderr,
            )
            _lang_data = _fallback_data


def get_string(key: str, default: str = "") -> str:
    if not _fallback_data:
        load_lang("en")
    val = _lang_data.get(key, _fallback_data.get(key, default))
    if val:
        return val

    missing_key_format = _fallback_data.get("err_missing_key", "[{key}]")
    try:
        return missing_key_format.format(key=key)
    except KeyError:
        return f"[{key}]"
