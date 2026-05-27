from __future__ import annotations

import ctypes
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from ._native import lib


OK = 0
NOT_FOUND = -1
USAGE = -2
BUFFER_TOO_SMALL = -4


class CallBookError(RuntimeError):
    """Raised when the native callbook library returns an error code."""

    def __init__(self, code: int, context: str) -> None:
        self.code = code
        message = _decode(lib.callbook_strerror(code))
        super().__init__(f"{context}: {message}")


def _decode(value: bytes | None) -> str:
    return value.decode("utf-8", errors="replace") if value else ""


def _check(code: int, context: str, allow_not_found: bool = False) -> None:
    if code == OK or (allow_not_found and code == NOT_FOUND):
        return
    raise CallBookError(code, context)


def _call_bytes(value: str) -> bytes:
    return value.encode("utf-8")


def _optional_int(value: int) -> int | None:
    return None if value < 0 else value


def _optional_float_from(getter, pointer: int) -> float | None:
    out = ctypes.c_double()
    code = getter(pointer, ctypes.byref(out))
    if code == NOT_FOUND:
        return None
    _check(code, "read coordinate")
    return float(out.value)


FIELD_NAMES = {
    "callsign": 0,
    "name": 1,
    "first_name": 2,
    "middle_name": 3,
    "last_name": 4,
    "suffix": 5,
    "address": 6,
    "city": 7,
    "state_or_province": 8,
    "postal_code": 9,
    "county": 10,
    "country": 11,
    "license_class": 12,
    "record_code": 13,
    "birth_date": 14,
    "first_issued": 15,
    "expires": 16,
    "last_changed": 17,
    "gmt_offset": 18,
    "latitude": 19,
    "longitude": 20,
    "grid": 21,
    "area_code": 22,
    "previous_call": 23,
    "previous_class": 24,
    "fcc_transaction_type": 25,
    "email": 26,
    "qsl": 27,
    "url": 28,
    "interests": 29,
    "license_id": 30,
    "frn": 31,
    "numeric_id": 32,
    "fax_number": 33,
    "iota": 34,
}

US_FIELD_NAMES = {
    "callsign": 0,
    "class_": 1,
    "name": 2,
    "address": 3,
    "city": 4,
    "state": 5,
    "zip": 6,
    "county": 7,
    "license_issue_date": 8,
    "fcc_transaction_type": 9,
}

LOOKUP_STATUS = {
    0: "current",
    1: "archive_only",
    2: "not_found",
}

JURISDICTION = {
    0: "united_states",
    1: "canada",
    2: "international",
    3: "unknown",
}

ASSET_KIND = {
    0: "biography",
    1: "photo",
    2: "flag",
    3: "map",
    4: "sidecar_data",
}


@dataclass(frozen=True)
class ResolvedInterest:
    code: str
    category: str
    label: str


@dataclass(frozen=True)
class CallSnapshot:
    callsign: str
    vintage: int | None
    source_flags: int
    jurisdiction: str
    fields: dict[str, str]
    interests: tuple[ResolvedInterest, ...]

    @classmethod
    def from_ptr(cls, pointer: int) -> "CallSnapshot | None":
        if not pointer:
            return None
        fields = {
            name: _decode(lib.callbook_snapshot_field(pointer, field_id))
            for name, field_id in FIELD_NAMES.items()
        }
        interests = []
        for index in range(lib.callbook_snapshot_interest_len(pointer)):
            interest = lib.callbook_snapshot_interest_get(pointer, index)
            if interest:
                interests.append(
                    ResolvedInterest(
                        code=_decode(lib.callbook_interest_code(interest)),
                        category=_decode(lib.callbook_interest_category(interest)),
                        label=_decode(lib.callbook_interest_label(interest)),
                    )
                )
        return cls(
            callsign=fields["callsign"],
            vintage=_optional_int(lib.callbook_snapshot_vintage(pointer)),
            source_flags=int(lib.callbook_snapshot_source_flags(pointer)),
            jurisdiction=JURISDICTION.get(lib.callbook_snapshot_jurisdiction(pointer), "unknown"),
            fields=fields,
            interests=tuple(interests),
        )


class LookupResult:
    """Owned lookup result."""

    def __init__(self, pointer: int) -> None:
        if not pointer:
            raise ValueError("LookupResult received a null pointer")
        self._pointer = pointer

    def __repr__(self) -> str:
        if not self._pointer:
            return "<LookupResult closed>"
        return f"<LookupResult query={self.query!r} status={self.status!r}>"

    def close(self) -> None:
        if self._pointer:
            lib.callbook_result_free(self._pointer)
            self._pointer = 0

    def __enter__(self) -> "LookupResult":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()

    @property
    def query(self) -> str:
        return _decode(lib.callbook_result_query(self._pointer))

    @property
    def status(self) -> str:
        return LOOKUP_STATUS.get(lib.callbook_result_status(self._pointer), "not_found")

    @property
    def current(self) -> CallSnapshot | None:
        return CallSnapshot.from_ptr(lib.callbook_result_current(self._pointer))

    @property
    def history(self) -> tuple[CallSnapshot, ...]:
        out = []
        for index in range(lib.callbook_result_history_len(self._pointer)):
            snapshot = CallSnapshot.from_ptr(lib.callbook_result_history_get(self._pointer, index))
            if snapshot is not None:
                out.append(snapshot)
        return tuple(out)


@dataclass(frozen=True)
class CountryInfo:
    name: str
    cleaned_name: str
    code: str | None
    jurisdiction: str
    itu_zone: int | None
    cq_zone: int | None
    continent: str | None
    latitude: float | None
    longitude: float | None
    numeric_code: int | None
    source: int

    @classmethod
    def from_ptr(cls, pointer: int) -> "CountryInfo | None":
        if not pointer:
            return None
        code = _decode(lib.callbook_country_code(pointer)) or None
        continent = _decode(lib.callbook_country_continent(pointer)) or None
        return cls(
            name=_decode(lib.callbook_country_name(pointer)),
            cleaned_name=_decode(lib.callbook_country_cleaned_name(pointer)),
            code=code,
            jurisdiction=JURISDICTION.get(lib.callbook_country_jurisdiction(pointer), "unknown"),
            itu_zone=_optional_int(lib.callbook_country_itu_zone(pointer)),
            cq_zone=_optional_int(lib.callbook_country_cq_zone(pointer)),
            continent=continent,
            latitude=_optional_float_from(lib.callbook_country_latitude, pointer),
            longitude=_optional_float_from(lib.callbook_country_longitude, pointer),
            numeric_code=_optional_int(lib.callbook_country_numeric_code(pointer)),
            source=int(lib.callbook_country_source_value(pointer)),
        )


@dataclass(frozen=True)
class LookupCount:
    key: str
    count: int
    updated_yyyymmdd: int | None
    status: str | None

    @classmethod
    def from_ptr(cls, pointer: int) -> "LookupCount | None":
        if not pointer:
            return None
        return cls(
            key=_decode(lib.callbook_lookup_count_key(pointer)),
            count=int(lib.callbook_lookup_count_value(pointer)),
            updated_yyyymmdd=_optional_int(lib.callbook_lookup_count_updated_yyyymmdd(pointer)),
            status=_decode(lib.callbook_lookup_count_status(pointer)) or None,
        )


@dataclass(frozen=True)
class Asset:
    kind: str
    key: str
    media_type: str
    path: Path

    @classmethod
    def from_ptr(cls, pointer: int) -> "Asset | None":
        if not pointer:
            return None
        return cls(
            kind=ASSET_KIND.get(lib.callbook_asset_kind_value(pointer), "sidecar_data"),
            key=_decode(lib.callbook_asset_key(pointer)),
            media_type=_decode(lib.callbook_asset_media_type(pointer)),
            path=Path(_decode(lib.callbook_asset_path(pointer))),
        )


class StationProfile:
    """Owned station profile."""

    def __init__(self, pointer: int) -> None:
        if not pointer:
            raise ValueError("StationProfile received a null pointer")
        self._pointer = pointer

    def __repr__(self) -> str:
        if not self._pointer:
            return "<StationProfile closed>"
        return f"<StationProfile callsign={self.callsign!r}>"

    def close(self) -> None:
        if self._pointer:
            lib.callbook_profile_free(self._pointer)
            self._pointer = 0

    def __enter__(self) -> "StationProfile":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()

    @property
    def callsign(self) -> str:
        return _decode(lib.callbook_profile_callsign(self._pointer))

    @property
    def status(self) -> str:
        return LOOKUP_STATUS.get(lib.callbook_profile_status(self._pointer), "not_found")

    @property
    def current(self) -> CallSnapshot | None:
        return CallSnapshot.from_ptr(lib.callbook_profile_current(self._pointer))

    @property
    def history_snapshot_count(self) -> int:
        return int(lib.callbook_profile_history_snapshot_count(self._pointer))

    @property
    def history_vintages(self) -> tuple[int, ...]:
        return tuple(
            lib.callbook_profile_history_vintage_get(self._pointer, index)
            for index in range(lib.callbook_profile_history_vintage_len(self._pointer))
        )

    @property
    def country(self) -> CountryInfo | None:
        return CountryInfo.from_ptr(lib.callbook_profile_country(self._pointer))

    @property
    def lookup_count(self) -> LookupCount | None:
        return LookupCount.from_ptr(lib.callbook_profile_lookup_count(self._pointer))

    @property
    def assets(self) -> tuple[Asset, ...]:
        out = []
        for index in range(lib.callbook_profile_asset_len(self._pointer)):
            asset = Asset.from_ptr(lib.callbook_profile_asset_get(self._pointer, index))
            if asset is not None:
                out.append(asset)
        return tuple(out)


@dataclass(frozen=True)
class UsRecord:
    fields: dict[str, str]

    @classmethod
    def from_ptr(cls, pointer: int) -> "UsRecord | None":
        if not pointer:
            return None
        return cls(
            {
                name: _decode(lib.callbook_us_record_field(pointer, field_id))
                for name, field_id in US_FIELD_NAMES.items()
            }
        )


@dataclass(frozen=True)
class InterestDefinition:
    code: str
    category: str
    label: str

    @classmethod
    def from_ptr(cls, pointer: int) -> "InterestDefinition | None":
        if not pointer:
            return None
        return cls(
            code=_decode(lib.callbook_interest_definition_code(pointer)),
            category=_decode(lib.callbook_interest_definition_category(pointer)),
            label=_decode(lib.callbook_interest_definition_label(pointer)),
        )


@dataclass(frozen=True)
class InterestSearchMatch:
    callsign: str
    vintage: int | None

    @classmethod
    def from_ptr(cls, pointer: int) -> "InterestSearchMatch | None":
        if not pointer:
            return None
        return cls(
            callsign=_decode(lib.callbook_interest_search_match_callsign(pointer)),
            vintage=_optional_int(lib.callbook_interest_search_match_vintage(pointer)),
        )


class InterestSearch:
    """Owned interest search result."""

    def __init__(self, pointer: int) -> None:
        if not pointer:
            raise ValueError("InterestSearch received a null pointer")
        self._pointer = pointer

    def __repr__(self) -> str:
        if not self._pointer:
            return "<InterestSearch closed>"
        return f"<InterestSearch code={self.code!r}>"

    def close(self) -> None:
        if self._pointer:
            lib.callbook_interest_search_free(self._pointer)
            self._pointer = 0

    def __enter__(self) -> "InterestSearch":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()

    @property
    def code(self) -> str:
        return _decode(lib.callbook_interest_search_code(self._pointer))

    @property
    def definition(self) -> InterestDefinition | None:
        return InterestDefinition.from_ptr(lib.callbook_interest_search_definition(self._pointer))

    @property
    def matches(self) -> tuple[InterestSearchMatch, ...]:
        out = []
        for index in range(lib.callbook_interest_search_match_len(self._pointer)):
            entry = InterestSearchMatch.from_ptr(
                lib.callbook_interest_search_match_get(self._pointer, index)
            )
            if entry is not None:
                out.append(entry)
        return tuple(out)


class CallBook:
    """Handle to a local HamCall database."""

    def __init__(self, pointer: int) -> None:
        if not pointer:
            raise ValueError("CallBook received a null pointer")
        self._pointer = pointer

    def __repr__(self) -> str:
        if not self._pointer:
            return "<CallBook closed>"
        return "<CallBook open>"

    @classmethod
    def open(cls, path: str | Path) -> "CallBook":
        out = ctypes.c_void_p()
        code = lib.callbook_open(os_fspath_bytes(path), ctypes.byref(out))
        _check(code, "open database")
        return cls(int(out.value))

    def close(self) -> None:
        if self._pointer:
            lib.callbook_close(self._pointer)
            self._pointer = 0

    def __enter__(self) -> "CallBook":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()

    def lookup(self, callsign: str) -> LookupResult:
        out = ctypes.c_void_p()
        code = lib.callbook_lookup_modern(self._pointer, _call_bytes(callsign), ctypes.byref(out))
        _check(code, "lookup callsign")
        return LookupResult(int(out.value))

    def profile(self, callsign: str) -> StationProfile:
        out = ctypes.c_void_p()
        code = lib.callbook_profile_for_callsign(self._pointer, _call_bytes(callsign), ctypes.byref(out))
        _check(code, "build station profile")
        return StationProfile(int(out.value))

    def country_info(self, callsign: str) -> CountryInfo | None:
        out = ctypes.c_void_p()
        code = lib.callbook_country_info_for_callsign(
            self._pointer, _call_bytes(callsign), ctypes.byref(out)
        )
        if code == NOT_FOUND:
            return None
        _check(code, "lookup country info")
        try:
            return CountryInfo.from_ptr(int(out.value))
        finally:
            if out.value:
                lib.callbook_country_info_free(out.value)

    def lookup_count(self, callsign: str) -> LookupCount | None:
        out = ctypes.c_void_p()
        code = lib.callbook_lookup_count_for_callsign(
            self._pointer, _call_bytes(callsign), ctypes.byref(out)
        )
        if code == NOT_FOUND:
            return None
        _check(code, "lookup count")
        try:
            return LookupCount.from_ptr(int(out.value))
        finally:
            if out.value:
                lib.callbook_lookup_count_free(out.value)

    def current_us_lookup(self, callsign: str) -> UsRecord | None:
        out = ctypes.c_void_p()
        code = lib.callbook_current_us_lookup(self._pointer, _call_bytes(callsign), ctypes.byref(out))
        if code == NOT_FOUND:
            return None
        _check(code, "lookup current US record")
        try:
            return UsRecord.from_ptr(int(out.value))
        finally:
            if out.value:
                lib.callbook_us_record_free(out.value)

    def current_us_records(self) -> Iterable[UsRecord]:
        for index in range(lib.callbook_current_us_record_count(self._pointer)):
            out = ctypes.c_void_p()
            code = lib.callbook_current_us_record_get(self._pointer, index, ctypes.byref(out))
            _check(code, "read current US record")
            try:
                record = UsRecord.from_ptr(int(out.value))
                if record is not None:
                    yield record
            finally:
                if out.value:
                    lib.callbook_us_record_free(out.value)

    def interest_definition(self, code: str) -> InterestDefinition | None:
        out = ctypes.c_void_p()
        rc = lib.callbook_interest_catalog_lookup(self._pointer, _call_bytes(code), ctypes.byref(out))
        if rc == NOT_FOUND:
            return None
        _check(rc, "lookup interest definition")
        try:
            return InterestDefinition.from_ptr(int(out.value))
        finally:
            if out.value:
                lib.callbook_interest_definition_free(out.value)

    def interest_definitions(self) -> Iterable[InterestDefinition]:
        for index in range(lib.callbook_interest_catalog_len(self._pointer)):
            out = ctypes.c_void_p()
            code = lib.callbook_interest_catalog_get(self._pointer, index, ctypes.byref(out))
            _check(code, "read interest definition")
            try:
                definition = InterestDefinition.from_ptr(int(out.value))
                if definition is not None:
                    yield definition
            finally:
                if out.value:
                    lib.callbook_interest_definition_free(out.value)

    def search_interest(self, code: str) -> InterestSearch:
        out = ctypes.c_void_p()
        rc = lib.callbook_interest_search_for_code(self._pointer, _call_bytes(code), ctypes.byref(out))
        _check(rc, "search interest")
        return InterestSearch(int(out.value))

    def map_svg(self, callsign: str) -> str | None:
        call = _call_bytes(callsign)
        required = lib.callbook_map_svg_required_len(self._pointer, call)
        if required == NOT_FOUND:
            return None
        _check(required if required < 0 else OK, "render map svg")
        buf = ctypes.create_string_buffer(required)
        written = lib.callbook_map_svg(self._pointer, call, buf, required)
        _check(written if written < 0 else OK, "render map svg")
        return buf.value.decode("utf-8", errors="replace")


def os_fspath_bytes(path: str | Path) -> bytes:
    return str(Path(path)).encode("utf-8")
