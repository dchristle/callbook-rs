from __future__ import annotations

import ctypes
import os
import sys
from importlib import resources
from pathlib import Path


def _library_name() -> str:
    if sys.platform == "win32":
        return "callbook.dll"
    if sys.platform == "darwin":
        return "libcallbook.dylib"
    return "libcallbook.so"


def _candidate_paths() -> list[Path]:
    paths: list[Path] = []
    override = os.environ.get("CALLBOOK_RS_LIB")
    if override:
        paths.append(Path(override))
    try:
        lib_root = resources.files("callbook_rs").joinpath("lib")
        paths.append(Path(str(lib_root.joinpath(_library_name()))))
    except (FileNotFoundError, ModuleNotFoundError):
        pass
    return paths


def load_library() -> ctypes.CDLL:
    attempted: list[str] = []
    for path in _candidate_paths():
        attempted.append(str(path))
        if path.exists():
            return ctypes.CDLL(str(path))
    name = _library_name()
    attempted.append(name)
    try:
        return ctypes.CDLL(name)
    except OSError as exc:
        joined = ", ".join(attempted)
        raise RuntimeError(f"could not load callbook native library; tried: {joined}") from exc


lib = load_library()

c_char_p = ctypes.c_char_p
c_double_p = ctypes.POINTER(ctypes.c_double)
c_int_p = ctypes.POINTER(ctypes.c_int)
c_uint32 = ctypes.c_uint32
c_void_p_p = ctypes.POINTER(ctypes.c_void_p)


def _set(name: str, restype: object, argtypes: list[object]) -> None:
    fn = getattr(lib, name)
    fn.restype = restype
    fn.argtypes = argtypes


_set("callbook_open", ctypes.c_int, [c_char_p, c_void_p_p])
_set("callbook_close", None, [ctypes.c_void_p])
_set("callbook_strerror", c_char_p, [ctypes.c_int])

_set("callbook_lookup_modern", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_result_free", None, [ctypes.c_void_p])
_set("callbook_result_query", c_char_p, [ctypes.c_void_p])
_set("callbook_result_status", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_result_current", ctypes.c_void_p, [ctypes.c_void_p])
_set("callbook_result_history_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_result_history_get", ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_int])
_set("callbook_snapshot_field", c_char_p, [ctypes.c_void_p, ctypes.c_int])
_set("callbook_snapshot_vintage", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_snapshot_source_flags", c_uint32, [ctypes.c_void_p])
_set("callbook_snapshot_jurisdiction", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_snapshot_interest_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_snapshot_interest_get", ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_int])
_set("callbook_interest_code", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_category", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_label", c_char_p, [ctypes.c_void_p])

_set("callbook_profile_for_callsign", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_profile_free", None, [ctypes.c_void_p])
_set("callbook_profile_callsign", c_char_p, [ctypes.c_void_p])
_set("callbook_profile_status", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_profile_current", ctypes.c_void_p, [ctypes.c_void_p])
_set("callbook_profile_history_snapshot_count", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_profile_history_vintage_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_profile_history_vintage_get", ctypes.c_int, [ctypes.c_void_p, ctypes.c_int])
_set("callbook_profile_country", ctypes.c_void_p, [ctypes.c_void_p])
_set("callbook_profile_lookup_count", ctypes.c_void_p, [ctypes.c_void_p])
_set("callbook_profile_asset_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_profile_asset_get", ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_int])

_set("callbook_country_info_for_callsign", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_country_info_free", None, [ctypes.c_void_p])
_set("callbook_country_name", c_char_p, [ctypes.c_void_p])
_set("callbook_country_cleaned_name", c_char_p, [ctypes.c_void_p])
_set("callbook_country_code", c_char_p, [ctypes.c_void_p])
_set("callbook_country_jurisdiction", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_country_itu_zone", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_country_cq_zone", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_country_continent", c_char_p, [ctypes.c_void_p])
_set("callbook_country_latitude", ctypes.c_int, [ctypes.c_void_p, c_double_p])
_set("callbook_country_longitude", ctypes.c_int, [ctypes.c_void_p, c_double_p])
_set("callbook_country_numeric_code", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_country_source_value", ctypes.c_int, [ctypes.c_void_p])

_set("callbook_lookup_count_for_callsign", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_lookup_count_free", None, [ctypes.c_void_p])
_set("callbook_lookup_count_key", c_char_p, [ctypes.c_void_p])
_set("callbook_lookup_count_value", c_uint32, [ctypes.c_void_p])
_set("callbook_lookup_count_updated_yyyymmdd", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_lookup_count_status", c_char_p, [ctypes.c_void_p])

_set("callbook_asset_kind_value", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_asset_key", c_char_p, [ctypes.c_void_p])
_set("callbook_asset_media_type", c_char_p, [ctypes.c_void_p])
_set("callbook_asset_path", c_char_p, [ctypes.c_void_p])

_set("callbook_current_us_record_count", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_current_us_record_get", ctypes.c_int, [ctypes.c_void_p, ctypes.c_int, c_void_p_p])
_set("callbook_current_us_lookup", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_us_record_free", None, [ctypes.c_void_p])
_set("callbook_us_record_field", c_char_p, [ctypes.c_void_p, ctypes.c_int])

_set("callbook_interest_catalog_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_interest_catalog_get", ctypes.c_int, [ctypes.c_void_p, ctypes.c_int, c_void_p_p])
_set("callbook_interest_catalog_lookup", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_interest_definition_free", None, [ctypes.c_void_p])
_set("callbook_interest_definition_code", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_definition_category", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_definition_label", c_char_p, [ctypes.c_void_p])

_set("callbook_interest_search_for_code", ctypes.c_int, [ctypes.c_void_p, c_char_p, c_void_p_p])
_set("callbook_interest_search_free", None, [ctypes.c_void_p])
_set("callbook_interest_search_code", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_search_definition", ctypes.c_void_p, [ctypes.c_void_p])
_set("callbook_interest_search_match_len", ctypes.c_int, [ctypes.c_void_p])
_set("callbook_interest_search_match_get", ctypes.c_void_p, [ctypes.c_void_p, ctypes.c_int])
_set("callbook_interest_search_match_callsign", c_char_p, [ctypes.c_void_p])
_set("callbook_interest_search_match_vintage", ctypes.c_int, [ctypes.c_void_p])

_set("callbook_map_svg_required_len", ctypes.c_int, [ctypes.c_void_p, c_char_p])
_set("callbook_map_svg", ctypes.c_int, [ctypes.c_void_p, c_char_p, ctypes.c_void_p, ctypes.c_int])
