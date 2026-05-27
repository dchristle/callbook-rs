//! Stable C ABI for the crate's `cdylib` artifact.
//!
//! Linux/macOS/Windows-x64 callers can `dlopen` / `LoadLibrary` the produced
//! shared library and use this flat C interface. [`callbook_open`] performs
//! the expensive database setup once and returns a reusable read-only handle.
//! Modern lookup callers should keep that handle alive, call
//! [`callbook_lookup_modern`] or [`callbook_lookup_json`] many times, then
//! release it with [`callbook_close`]. The header `include/callbook.h` is
//! generated from these declarations by `cbindgen` during the crate's build.
//!
//! # Return-code conventions
//!
//! Functions that produce a count return a non-negative integer on success.
//! Errors are reported as one of these distinct negative codes:
//!
//! | code | constant                       | meaning                                        |
//! |------|--------------------------------|------------------------------------------------|
//! |  -1  | `CALLBOOK_E_NOT_FOUND`          | callsign was not found in any open shard       |
//! |  -2  | `CALLBOOK_E_USAGE`              | bad arguments (null, non-UTF-8, etc.)          |
//! |  -3  | `CALLBOOK_E_OPEN_FAILED`        | could not open the database (only from open)   |
//! |  -4  | `CALLBOOK_E_BUFFER_TOO_SMALL`   | output buffer too small (use the *_required_len query) |
//!
//! Use [`callbook_strerror`] to convert a negative code to a static string.
//! The negative-code space is closed: callers may safely treat unknown
//! negatives as fatal/unknown errors and assume new codes will be added in
//! a way that keeps `(value, meaning)` stable for existing codes.

use std::ffi::{c_char, c_double, c_int, CStr, CString};
use std::path::Path;

use crate::{
    AssetKind, AssetMetadata, CallBook, CallSnapshot, CountryInfo, CountryInfoSource, Error,
    InterestDefinition, InterestSearch, InterestSearchMatch, Jurisdiction, LookupCountRecord,
    LookupResult, LookupStatus, ResolvedInterest, SnapshotSource, StationProfile, UsCsvRecord,
};

/// Return code: callsign was not found.
pub const CALLBOOK_E_NOT_FOUND: c_int = -1;
/// Return code: invalid argument supplied by the caller.
pub const CALLBOOK_E_USAGE: c_int = -2;
/// Return code: failed to open the database.
pub const CALLBOOK_E_OPEN_FAILED: c_int = -3;
/// Return code: output buffer was too small for the result.
pub const CALLBOOK_E_BUFFER_TOO_SMALL: c_int = -4;

/// Source flag: current US FCC catalog contributed to a snapshot.
pub const CALLBOOK_SOURCE_US_CSV: u32 = 1 << 0;
/// Source flag: direct `hamcall.idx` -> `hamcall.dat` lookup contributed.
pub const CALLBOOK_SOURCE_HAMCALL_DAT_IDX: u32 = 1 << 1;
/// Source flag: `hciindex.dat`/`hci.dat` lookup contributed.
pub const CALLBOOK_SOURCE_HAMCALL_HCI: u32 = 1 << 2;

const MODERN_FIELD_COUNT: usize = 35;
const US_FIELD_COUNT: usize = 10;

/// Opaque handle returned by [`callbook_open`].
///
/// Read-only after construction. Pointers handed out by [`callbook_open`]
/// can be shared across threads — the underlying [`CallBook`] supports
/// lock-free concurrent reads. Callers must not call [`callbook_close`] until
/// all concurrent calls using the handle have returned.
#[allow(non_camel_case_types)]
pub struct callbook_db {
    db: CallBook,
}

/// Opaque owned result returned by [`callbook_lookup_modern`].
///
/// All strings returned by result/snapshot accessors are borrowed from this
/// object and remain valid until [`callbook_result_free`] is called.
#[allow(non_camel_case_types)]
pub struct callbook_lookup_result {
    query: CString,
    status: callbook_lookup_status,
    current: Option<callbook_snapshot>,
    history: Vec<callbook_snapshot>,
}

/// Opaque snapshot borrowed from a [`callbook_lookup_result`].
///
/// Pointers to snapshots are invalidated when their owning result is freed.
#[allow(non_camel_case_types)]
pub struct callbook_snapshot {
    fields: Vec<CString>,
    interest_codes: Vec<CString>,
    interests: Vec<callbook_interest>,
    vintage: c_int,
    source_flags: u32,
    jurisdiction: callbook_jurisdiction,
}

/// Opaque interest item borrowed from a [`callbook_snapshot`].
#[allow(non_camel_case_types)]
pub struct callbook_interest {
    code: CString,
    category: CString,
    label: CString,
}

/// Opaque owned station profile returned by [`callbook_profile_for_callsign`].
///
/// Snapshot, country, count, and asset pointers borrowed from this object are
/// valid until [`callbook_profile_free`] is called.
#[allow(non_camel_case_types)]
pub struct callbook_profile {
    callsign: CString,
    status: callbook_lookup_status,
    current: Option<callbook_snapshot>,
    history_snapshot_count: c_int,
    history_vintages: Vec<c_int>,
    country: Option<callbook_country_info>,
    lookup_count: Option<callbook_lookup_count>,
    assets: Vec<callbook_asset>,
}

/// Opaque country metadata.
#[allow(non_camel_case_types)]
pub struct callbook_country_info {
    name: CString,
    raw_name: CString,
    cleaned_name: CString,
    code: CString,
    jurisdiction: callbook_jurisdiction,
    itu_zone: c_int,
    cq_zone: c_int,
    continent: CString,
    latitude: Option<c_double>,
    longitude: Option<c_double>,
    numeric_code: c_int,
    source: callbook_country_source,
}

/// Opaque `counts.dat` lookup-count record.
#[allow(non_camel_case_types)]
pub struct callbook_lookup_count {
    key: CString,
    count: u32,
    updated_yyyymmdd: c_int,
    status: CString,
}

/// Opaque owned country catalog snapshot.
#[allow(non_camel_case_types)]
pub struct callbook_country_catalog {
    records: Vec<callbook_country_info>,
}

/// Opaque owned lookup-count catalog snapshot.
#[allow(non_camel_case_types)]
pub struct callbook_lookup_counts_catalog {
    records: Vec<callbook_lookup_count>,
}

/// Opaque sidecar asset metadata.
#[allow(non_camel_case_types)]
pub struct callbook_asset {
    kind: callbook_asset_kind,
    key: CString,
    media_type: CString,
    path: CString,
}

/// Opaque current-US catalog record.
#[allow(non_camel_case_types)]
pub struct callbook_us_record {
    fields: Vec<CString>,
}

/// Opaque owned interest catalog definition.
#[allow(non_camel_case_types)]
pub struct callbook_interest_definition {
    code: CString,
    category: CString,
    label: CString,
}

/// Opaque owned interest-search result.
#[allow(non_camel_case_types)]
pub struct callbook_interest_search_result {
    code: CString,
    definition: Option<callbook_interest_definition>,
    matches: Vec<callbook_interest_search_match>,
}

/// Opaque interest-search match.
#[allow(non_camel_case_types)]
pub struct callbook_interest_search_match {
    callsign: CString,
    vintage: c_int,
}

/// High-level lookup status.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_lookup_status {
    Current = 0,
    ArchiveOnly = 1,
    NotFound = 2,
}

/// Country/jurisdiction classification.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_jurisdiction {
    UnitedStates = 0,
    Canada = 1,
    International = 2,
    Unknown = 3,
}

/// Source sidecar for country metadata.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_country_source {
    Unknown = 0,
    Countrys = 1,
    GcMcountrys = 2,
    CountrysPc = 3,
}

/// Sidecar asset kind.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_asset_kind {
    Biography = 0,
    Photo = 1,
    Flag = 2,
    Map = 3,
    SidecarData = 4,
}

/// Field selector for [`callbook_snapshot_field`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_modern_field {
    Callsign = 0,
    Name = 1,
    FirstName = 2,
    MiddleName = 3,
    LastName = 4,
    Suffix = 5,
    Address = 6,
    City = 7,
    StateOrProvince = 8,
    PostalCode = 9,
    County = 10,
    Country = 11,
    LicenseClass = 12,
    RecordCode = 13,
    BirthDate = 14,
    FirstIssued = 15,
    Expires = 16,
    LastChanged = 17,
    GmtOffset = 18,
    Latitude = 19,
    Longitude = 20,
    Grid = 21,
    AreaCode = 22,
    PreviousCall = 23,
    PreviousClass = 24,
    FccTransactionType = 25,
    Email = 26,
    Qsl = 27,
    Url = 28,
    Interests = 29,
    LicenseId = 30,
    Frn = 31,
    NumericId = 32,
    FaxNumber = 33,
    Iota = 34,
}

/// Field selector for [`callbook_us_record_field`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, missing_docs)]
pub enum callbook_us_field {
    Callsign = 0,
    Class = 1,
    Name = 2,
    Address = 3,
    City = 4,
    State = 5,
    Zip = 6,
    County = 7,
    LicenseIssueDate = 8,
    FccTransactionType = 9,
}

/// Open a local Buckmaster HamCall database tree at `data_path`.
///
/// On success returns 0 and writes a non-null pointer into `*out`. On
/// failure returns one of the documented negative codes and writes null.
///
/// # Safety
///
/// `data_path` must be a valid NUL-terminated UTF-8 C string. `out` must be
/// a writable pointer to `*mut callbook_db`.
#[no_mangle]
pub unsafe extern "C" fn callbook_open(
    data_path: *const c_char,
    out: *mut *mut callbook_db,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    if data_path.is_null() {
        unsafe { *out = std::ptr::null_mut() };
        return CALLBOOK_E_USAGE;
    }
    let cstr = unsafe { CStr::from_ptr(data_path) };
    let Ok(path) = cstr.to_str() else {
        unsafe { *out = std::ptr::null_mut() };
        return CALLBOOK_E_USAGE;
    };
    match CallBook::open(Path::new(path)) {
        Ok(db) => {
            let boxed = Box::new(callbook_db { db });
            unsafe { *out = Box::into_raw(boxed) };
            0
        }
        Err(_) => {
            unsafe { *out = std::ptr::null_mut() };
            CALLBOOK_E_OPEN_FAILED
        }
    }
}

/// Close a database opened by [`callbook_open`]. Safe to pass null.
///
/// Do not close a handle while another thread is using it.
///
/// # Safety
///
/// `db` must be a pointer returned by [`callbook_open`] and not yet closed.
#[no_mangle]
pub unsafe extern "C" fn callbook_close(db: *mut callbook_db) {
    if db.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(db) });
}

/// Look up `callsign` in the structured lookup path.
///
/// This function uses the database handle opened by [`callbook_open`]; it does
/// not rescan or reopen the database. On success, it writes an owned result to
/// `*out` and returns 0. A not-found callsign is still a successful lookup:
/// inspect [`callbook_result_status`] for [`callbook_lookup_status::NotFound`].
/// The caller owns the result and must release it with
/// [`callbook_result_free`].
///
/// # Safety
///
/// `db` must be a valid pointer returned by [`callbook_open`]. `callsign` must
/// be a valid NUL-terminated UTF-8 C string. `out` must be a writable pointer
/// to `*mut callbook_lookup_result`.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_modern(
    db: *const callbook_db,
    callsign: *const c_char,
    out: *mut *mut callbook_lookup_result,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match handle.db.lookup_report(&call) {
        Ok(result) => {
            let boxed = Box::new(callbook_lookup_result::from(result));
            unsafe { *out = Box::into_raw(boxed) };
            0
        }
        Err(e) => match e {
            Error::Io(_) | Error::Zip { .. } | Error::Csv { .. } => CALLBOOK_E_OPEN_FAILED,
            _ => CALLBOOK_E_USAGE,
        },
    }
}

/// Free a result returned by [`callbook_lookup_modern`]. Safe to pass null.
///
/// # Safety
///
/// `result` must be null or a pointer returned by [`callbook_lookup_modern`]
/// that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_free(result: *mut callbook_lookup_result) {
    if result.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(result) });
}

/// Return the normalized query string for a lookup result.
///
/// The returned pointer is valid until [`callbook_result_free`] is called.
///
/// # Safety
///
/// `result` must be null or a valid pointer returned by
/// [`callbook_lookup_modern`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_query(
    result: *const callbook_lookup_result,
) -> *const c_char {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return empty_cstr();
    };
    result.query.as_ptr()
}

/// Return the status of a lookup result.
///
/// # Safety
///
/// `result` must be null or a valid pointer returned by
/// [`callbook_lookup_modern`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_status(
    result: *const callbook_lookup_result,
) -> callbook_lookup_status {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return callbook_lookup_status::NotFound;
    };
    result.status
}

/// Return the current snapshot, or null when there is no current record.
///
/// The returned pointer is borrowed from `result` and is valid until
/// [`callbook_result_free`] is called.
///
/// # Safety
///
/// `result` must be null or a valid pointer returned by
/// [`callbook_lookup_modern`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_current(
    result: *const callbook_lookup_result,
) -> *const callbook_snapshot {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return std::ptr::null();
    };
    result
        .current
        .as_ref()
        .map_or(std::ptr::null(), |snapshot| snapshot as *const _)
}

/// Return the number of historical snapshots in a lookup result.
///
/// # Safety
///
/// `result` must be a valid pointer returned by [`callbook_lookup_modern`] that
/// has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_history_len(
    result: *const callbook_lookup_result,
) -> c_int {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    clamp_len(result.history.len())
}

/// Return one historical snapshot by zero-based index, or null when out of range.
///
/// The returned pointer is borrowed from `result` and is valid until
/// [`callbook_result_free`] is called.
///
/// # Safety
///
/// `result` must be null or a valid pointer returned by
/// [`callbook_lookup_modern`] that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_result_history_get(
    result: *const callbook_lookup_result,
    index: c_int,
) -> *const callbook_snapshot {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    result
        .history
        .get(index as usize)
        .map_or(std::ptr::null(), |snapshot| snapshot as *const _)
}

/// Return a snapshot string field selected by [`callbook_modern_field`].
///
/// Missing fields return an empty string. The returned pointer is borrowed
/// from the owning result and is valid until [`callbook_result_free`] is called.
///
/// # Safety
///
/// `snapshot` must be null or a pointer returned by
/// [`callbook_result_current`] / [`callbook_result_history_get`] whose owning
/// result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_field(
    snapshot: *const callbook_snapshot,
    field_id: c_int,
) -> *const c_char {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return empty_cstr();
    };
    if field_id < 0 {
        return empty_cstr();
    }
    snapshot
        .fields
        .get(field_id as usize)
        .map_or_else(empty_cstr, |value| value.as_ptr())
}

/// Return the snapshot vintage year, or -1 for current/non-vintage records.
///
/// # Safety
///
/// `snapshot` must be null or a pointer returned by
/// [`callbook_result_current`] / [`callbook_result_history_get`] whose owning
/// result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_vintage(snapshot: *const callbook_snapshot) -> c_int {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return -1;
    };
    snapshot.vintage
}

/// Return ORed `CALLBOOK_SOURCE_*` flags for a snapshot.
///
/// # Safety
///
/// `snapshot` must be null or a pointer returned by
/// [`callbook_result_current`] / [`callbook_result_history_get`] whose owning
/// result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_source_flags(snapshot: *const callbook_snapshot) -> u32 {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return 0;
    };
    snapshot.source_flags
}

/// Return the snapshot jurisdiction classification.
///
/// # Safety
///
/// `snapshot` must be null or a pointer returned by
/// [`callbook_result_current`] / [`callbook_result_history_get`] whose owning
/// result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_jurisdiction(
    snapshot: *const callbook_snapshot,
) -> callbook_jurisdiction {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return callbook_jurisdiction::Unknown;
    };
    snapshot.jurisdiction
}

/// Return the raw concatenated interest-code string.
///
/// # Safety
///
/// `snapshot` must be null or a pointer returned by
/// [`callbook_result_current`] / [`callbook_result_history_get`] whose owning
/// result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_interest_codes_raw(
    snapshot: *const callbook_snapshot,
) -> *const c_char {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return empty_cstr();
    };
    snapshot
        .fields
        .get(callbook_modern_field::Interests as usize)
        .map_or_else(empty_cstr, |value| value.as_ptr())
}

/// Return the number of parsed interest codes.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_snapshot_interest_codes_raw`].
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_interest_code_len(
    snapshot: *const callbook_snapshot,
) -> c_int {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return 0;
    };
    c_int::try_from(snapshot.interest_codes.len()).unwrap_or(c_int::MAX)
}

/// Return one parsed interest code by zero-based index.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_snapshot_interest_codes_raw`].
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_interest_code_get(
    snapshot: *const callbook_snapshot,
    index: c_int,
) -> *const c_char {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return empty_cstr();
    };
    if index < 0 {
        return empty_cstr();
    }
    snapshot
        .interest_codes
        .get(index as usize)
        .map_or_else(empty_cstr, |value| value.as_ptr())
}

/// Return the number of resolved interest labels.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_snapshot_interest_codes_raw`].
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_interest_len(
    snapshot: *const callbook_snapshot,
) -> c_int {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return 0;
    };
    c_int::try_from(snapshot.interests.len()).unwrap_or(c_int::MAX)
}

/// Return one resolved interest by zero-based index.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_snapshot_interest_codes_raw`].
#[no_mangle]
pub unsafe extern "C" fn callbook_snapshot_interest_get(
    snapshot: *const callbook_snapshot,
    index: c_int,
) -> *const callbook_interest {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    snapshot
        .interests
        .get(index as usize)
        .map_or(std::ptr::null(), |interest| interest as *const _)
}

/// Return an interest's four-digit code.
///
/// # Safety
///
/// `interest` must be null or a pointer returned by
/// [`callbook_snapshot_interest_get`] whose owning result has not been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_code(
    interest: *const callbook_interest,
) -> *const c_char {
    let Some(interest) = (unsafe { interest.as_ref() }) else {
        return empty_cstr();
    };
    interest.code.as_ptr()
}

/// Return an interest's category heading.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_interest_code`].
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_category(
    interest: *const callbook_interest,
) -> *const c_char {
    let Some(interest) = (unsafe { interest.as_ref() }) else {
        return empty_cstr();
    };
    interest.category.as_ptr()
}

/// Return an interest's human-readable label.
///
/// # Safety
///
/// Same pointer requirements as [`callbook_interest_code`].
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_label(
    interest: *const callbook_interest,
) -> *const c_char {
    let Some(interest) = (unsafe { interest.as_ref() }) else {
        return empty_cstr();
    };
    interest.label.as_ptr()
}

/// Build a station profile for `callsign`.
///
/// The returned profile owns all nested strings and borrowed child pointers.
/// Release it with [`callbook_profile_free`].
///
/// # Safety
///
/// `db` must be a valid pointer returned by [`callbook_open`]. `callsign` must
/// be a valid NUL-terminated UTF-8 string. `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_for_callsign(
    db: *const callbook_db,
    callsign: *const c_char,
    out: *mut *mut callbook_profile,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match handle.db.profile_for_callsign(&call) {
        Ok(profile) => {
            let boxed = Box::new(callbook_profile::from(profile));
            unsafe { *out = Box::into_raw(boxed) };
            0
        }
        Err(e) => match e {
            Error::Io(_) | Error::Zip { .. } | Error::Csv { .. } => CALLBOOK_E_OPEN_FAILED,
            _ => CALLBOOK_E_USAGE,
        },
    }
}

/// Free a profile returned by [`callbook_profile_for_callsign`]. Safe to pass null.
///
/// # Safety
///
/// `profile` must be null or a pointer returned by
/// [`callbook_profile_for_callsign`] that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_free(profile: *mut callbook_profile) {
    if profile.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(profile) });
}

/// Return the normalized callsign for a profile.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_callsign(
    profile: *const callbook_profile,
) -> *const c_char {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return empty_cstr();
    };
    profile.callsign.as_ptr()
}

/// Return the lookup status for a profile.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_status(
    profile: *const callbook_profile,
) -> callbook_lookup_status {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return callbook_lookup_status::NotFound;
    };
    profile.status
}

/// Return the current snapshot in a profile, or null.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_current(
    profile: *const callbook_profile,
) -> *const callbook_snapshot {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return std::ptr::null();
    };
    profile
        .current
        .as_ref()
        .map_or(std::ptr::null(), |snapshot| snapshot as *const _)
}

/// Return the number of historical snapshots summarized by the profile.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_history_snapshot_count(
    profile: *const callbook_profile,
) -> c_int {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return 0;
    };
    profile.history_snapshot_count
}

/// Return the number of unique history vintages in a profile.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_history_vintage_len(
    profile: *const callbook_profile,
) -> c_int {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return 0;
    };
    clamp_len(profile.history_vintages.len())
}

/// Return one unique profile history vintage, or -1 when out of range.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_history_vintage_get(
    profile: *const callbook_profile,
    index: c_int,
) -> c_int {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return -1;
    };
    if index < 0 {
        return -1;
    }
    profile
        .history_vintages
        .get(index as usize)
        .copied()
        .unwrap_or(-1)
}

/// Return country metadata borrowed from a profile, or null.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_country(
    profile: *const callbook_profile,
) -> *const callbook_country_info {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return std::ptr::null();
    };
    profile
        .country
        .as_ref()
        .map_or(std::ptr::null(), |country| country as *const _)
}

/// Return lookup-count metadata borrowed from a profile, or null.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_lookup_count(
    profile: *const callbook_profile,
) -> *const callbook_lookup_count {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return std::ptr::null();
    };
    profile
        .lookup_count
        .as_ref()
        .map_or(std::ptr::null(), |count| count as *const _)
}

/// Return the number of assets attached to a profile.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_asset_len(profile: *const callbook_profile) -> c_int {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return 0;
    };
    clamp_len(profile.assets.len())
}

/// Return one profile asset by zero-based index, or null.
///
/// # Safety
///
/// `profile` must be null or a valid profile pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_profile_asset_get(
    profile: *const callbook_profile,
    index: c_int,
) -> *const callbook_asset {
    let Some(profile) = (unsafe { profile.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    profile
        .assets
        .get(index as usize)
        .map_or(std::ptr::null(), |asset| asset as *const _)
}

/// Look up country metadata for `callsign`.
///
/// Release the returned object with [`callbook_country_info_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer. `callsign` must be a valid
/// NUL-terminated UTF-8 string. `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_info_for_callsign(
    db: *const callbook_db,
    callsign: *const c_char,
    out: *mut *mut callbook_country_info,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(country) = handle.db.country_info(&call) else {
        return CALLBOOK_E_NOT_FOUND;
    };
    unsafe { *out = Box::into_raw(Box::new(callbook_country_info::from(country))) };
    0
}

/// Free country metadata returned by [`callbook_country_info_for_callsign`].
///
/// # Safety
///
/// `country` must be null or an owned country pointer returned by this API.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_info_free(country: *mut callbook_country_info) {
    if country.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(country) });
}

/// Build an owned snapshot of the unified country catalog.
///
/// Release the returned object with [`callbook_country_catalog_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_catalog_open(
    db: *const callbook_db,
    out: *mut *mut callbook_country_catalog,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let records = handle
        .db
        .country_catalog()
        .records()
        .into_iter()
        .map(callbook_country_info::from)
        .collect();
    unsafe { *out = Box::into_raw(Box::new(callbook_country_catalog { records })) };
    0
}

/// Free a country catalog returned by [`callbook_country_catalog_open`].
///
/// # Safety
///
/// `catalog` must be null or an owned country-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_catalog_free(catalog: *mut callbook_country_catalog) {
    if catalog.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(catalog) });
}

/// Return the number of records in a country catalog snapshot.
///
/// # Safety
///
/// `catalog` must be null or a valid country-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_catalog_len(
    catalog: *const callbook_country_catalog,
) -> c_int {
    let Some(catalog) = (unsafe { catalog.as_ref() }) else {
        return 0;
    };
    clamp_len(catalog.records.len())
}

/// Return one country catalog record by zero-based index, or null.
///
/// # Safety
///
/// `catalog` must be null or a valid country-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_catalog_get(
    catalog: *const callbook_country_catalog,
    index: c_int,
) -> *const callbook_country_info {
    let Some(catalog) = (unsafe { catalog.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    catalog
        .records
        .get(index as usize)
        .map_or(std::ptr::null(), |country| country as *const _)
}

/// Return the country display label.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_name(
    country: *const callbook_country_info,
) -> *const c_char {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return empty_cstr();
    };
    country.name.as_ptr()
}

/// Return the raw country label from the source sidecar.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_raw_name(
    country: *const callbook_country_info,
) -> *const c_char {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return empty_cstr();
    };
    country.raw_name.as_ptr()
}

/// Return the cleaned country label.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_cleaned_name(
    country: *const callbook_country_info,
) -> *const c_char {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return empty_cstr();
    };
    country.cleaned_name.as_ptr()
}

/// Return the country code, or an empty string.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_code(
    country: *const callbook_country_info,
) -> *const c_char {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return empty_cstr();
    };
    country.code.as_ptr()
}

/// Return the country jurisdiction classification.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_jurisdiction(
    country: *const callbook_country_info,
) -> callbook_jurisdiction {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return callbook_jurisdiction::Unknown;
    };
    country.jurisdiction
}

/// Return the ITU zone, or -1 when absent.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_itu_zone(country: *const callbook_country_info) -> c_int {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return -1;
    };
    country.itu_zone
}

/// Return the CQ zone, or -1 when absent.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_cq_zone(country: *const callbook_country_info) -> c_int {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return -1;
    };
    country.cq_zone
}

/// Return the continent code, or an empty string.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_continent(
    country: *const callbook_country_info,
) -> *const c_char {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return empty_cstr();
    };
    country.continent.as_ptr()
}

/// Write the country latitude to `out`; returns 0 when present.
///
/// # Safety
///
/// `country` must be a valid country pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_latitude(
    country: *const callbook_country_info,
    out: *mut c_double,
) -> c_int {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(value) = country.latitude else {
        return CALLBOOK_E_NOT_FOUND;
    };
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = value };
    0
}

/// Write the country longitude to `out`; returns 0 when present.
///
/// # Safety
///
/// `country` must be a valid country pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_longitude(
    country: *const callbook_country_info,
    out: *mut c_double,
) -> c_int {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(value) = country.longitude else {
        return CALLBOOK_E_NOT_FOUND;
    };
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = value };
    0
}

/// Return the numeric country code, or -1 when absent.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_numeric_code(
    country: *const callbook_country_info,
) -> c_int {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return -1;
    };
    country.numeric_code
}

/// Return the source sidecar used for country metadata.
///
/// # Safety
///
/// `country` must be null or a valid country pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_country_source_value(
    country: *const callbook_country_info,
) -> callbook_country_source {
    let Some(country) = (unsafe { country.as_ref() }) else {
        return callbook_country_source::Unknown;
    };
    country.source
}

/// Look up web lookup-count metadata for a callsign.
///
/// Release the returned object with [`callbook_lookup_count_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer. `callsign` must be a valid
/// NUL-terminated UTF-8 string. `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_for_callsign(
    db: *const callbook_db,
    callsign: *const c_char,
    out: *mut *mut callbook_lookup_count,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match handle.db.lookup_count(&call) {
        Ok(Some(count)) => {
            unsafe { *out = Box::into_raw(Box::new(callbook_lookup_count::from(count))) };
            0
        }
        Ok(None) => CALLBOOK_E_NOT_FOUND,
        Err(_) => CALLBOOK_E_USAGE,
    }
}

/// Free a lookup count returned by [`callbook_lookup_count_for_callsign`].
///
/// # Safety
///
/// `count` must be null or an owned lookup-count pointer returned by this API.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_free(count: *mut callbook_lookup_count) {
    if count.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(count) });
}

/// Build an owned snapshot of the searchable `counts.dat` catalog.
///
/// Release the returned object with [`callbook_lookup_counts_catalog_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_counts_catalog_open(
    db: *const callbook_db,
    out: *mut *mut callbook_lookup_counts_catalog,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    match handle.db.lookup_counts() {
        Ok(Some(catalog)) => {
            let records = catalog.iter().map(callbook_lookup_count::from).collect();
            unsafe { *out = Box::into_raw(Box::new(callbook_lookup_counts_catalog { records })) };
            0
        }
        Ok(None) => CALLBOOK_E_NOT_FOUND,
        Err(_) => CALLBOOK_E_USAGE,
    }
}

/// Free a lookup-count catalog returned by [`callbook_lookup_counts_catalog_open`].
///
/// # Safety
///
/// `catalog` must be null or an owned lookup-count-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_counts_catalog_free(
    catalog: *mut callbook_lookup_counts_catalog,
) {
    if catalog.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(catalog) });
}

/// Return the number of records in a lookup-count catalog snapshot.
///
/// # Safety
///
/// `catalog` must be null or a valid lookup-count-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_counts_catalog_len(
    catalog: *const callbook_lookup_counts_catalog,
) -> c_int {
    let Some(catalog) = (unsafe { catalog.as_ref() }) else {
        return 0;
    };
    clamp_len(catalog.records.len())
}

/// Return one lookup-count catalog record by zero-based index, or null.
///
/// # Safety
///
/// `catalog` must be null or a valid lookup-count-catalog pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_counts_catalog_get(
    catalog: *const callbook_lookup_counts_catalog,
    index: c_int,
) -> *const callbook_lookup_count {
    let Some(catalog) = (unsafe { catalog.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    catalog
        .records
        .get(index as usize)
        .map_or(std::ptr::null(), |count| count as *const _)
}

/// Return the lookup-count key.
///
/// # Safety
///
/// `count` must be null or a valid lookup-count pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_key(
    count: *const callbook_lookup_count,
) -> *const c_char {
    let Some(count) = (unsafe { count.as_ref() }) else {
        return empty_cstr();
    };
    count.key.as_ptr()
}

/// Return the HamCall.net lookup count.
///
/// # Safety
///
/// `count` must be null or a valid lookup-count pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_value(count: *const callbook_lookup_count) -> u32 {
    let Some(count) = (unsafe { count.as_ref() }) else {
        return 0;
    };
    count.count
}

/// Return the update date as `YYYYMMDD`, or -1 when absent.
///
/// # Safety
///
/// `count` must be null or a valid lookup-count pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_updated_yyyymmdd(
    count: *const callbook_lookup_count,
) -> c_int {
    let Some(count) = (unsafe { count.as_ref() }) else {
        return -1;
    };
    count.updated_yyyymmdd
}

/// Return the count-record status byte as a string, or empty when absent.
///
/// # Safety
///
/// `count` must be null or a valid lookup-count pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_count_status(
    count: *const callbook_lookup_count,
) -> *const c_char {
    let Some(count) = (unsafe { count.as_ref() }) else {
        return empty_cstr();
    };
    count.status.as_ptr()
}

/// Return an asset's kind.
///
/// # Safety
///
/// `asset` must be null or a valid asset pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_asset_kind_value(
    asset: *const callbook_asset,
) -> callbook_asset_kind {
    let Some(asset) = (unsafe { asset.as_ref() }) else {
        return callbook_asset_kind::SidecarData;
    };
    asset.kind
}

/// Return an asset's callsign or country key.
///
/// # Safety
///
/// `asset` must be null or a valid asset pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_asset_key(asset: *const callbook_asset) -> *const c_char {
    let Some(asset) = (unsafe { asset.as_ref() }) else {
        return empty_cstr();
    };
    asset.key.as_ptr()
}

/// Return an asset's media type.
///
/// # Safety
///
/// `asset` must be null or a valid asset pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_asset_media_type(asset: *const callbook_asset) -> *const c_char {
    let Some(asset) = (unsafe { asset.as_ref() }) else {
        return empty_cstr();
    };
    asset.media_type.as_ptr()
}

/// Return an asset's on-disk path.
///
/// # Safety
///
/// `asset` must be null or a valid asset pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_asset_path(asset: *const callbook_asset) -> *const c_char {
    let Some(asset) = (unsafe { asset.as_ref() }) else {
        return empty_cstr();
    };
    asset.path.as_ptr()
}

/// Return the number of records in `usa.csv.zip`, or 0 when absent.
///
/// # Safety
///
/// `db` must be null or a valid database pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_current_us_record_count(db: *const callbook_db) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return 0;
    };
    handle
        .db
        .current_us_catalog()
        .map_or(0, |catalog| clamp_len(catalog.len()))
}

/// Return one current-US catalog record by sorted index.
///
/// Release the returned object with [`callbook_us_record_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_current_us_record_get(
    db: *const callbook_db,
    index: c_int,
    out: *mut *mut callbook_us_record,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    if index < 0 {
        return CALLBOOK_E_USAGE;
    }
    let Some(record) = handle
        .db
        .current_us_catalog()
        .and_then(|catalog| catalog.records().nth(index as usize))
    else {
        return CALLBOOK_E_NOT_FOUND;
    };
    unsafe { *out = Box::into_raw(Box::new(callbook_us_record::from(record))) };
    0
}

/// Look up one current-US catalog record by callsign.
///
/// Release the returned object with [`callbook_us_record_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer, `callsign` must be a valid
/// NUL-terminated UTF-8 string, and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_current_us_lookup(
    db: *const callbook_db,
    callsign: *const c_char,
    out: *mut *mut callbook_us_record,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(catalog) = handle.db.current_us_catalog() else {
        return CALLBOOK_E_NOT_FOUND;
    };
    match catalog.get(&call) {
        Some(record) => {
            unsafe { *out = Box::into_raw(Box::new(callbook_us_record::from(record))) };
            0
        }
        None => CALLBOOK_E_NOT_FOUND,
    }
}

/// Free a current-US record returned by this API. Safe to pass null.
///
/// # Safety
///
/// `record` must be null or an owned current-US record pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_us_record_free(record: *mut callbook_us_record) {
    if record.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(record) });
}

/// Return one current-US record string field.
///
/// # Safety
///
/// `record` must be null or a valid current-US record pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_us_record_field(
    record: *const callbook_us_record,
    field_id: c_int,
) -> *const c_char {
    let Some(record) = (unsafe { record.as_ref() }) else {
        return empty_cstr();
    };
    if field_id < 0 {
        return empty_cstr();
    }
    record
        .fields
        .get(field_id as usize)
        .map_or_else(empty_cstr, |field| field.as_ptr())
}

/// Return the number of interest-code definitions loaded from `ham0/interest`.
///
/// # Safety
///
/// `db` must be null or a valid database pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_catalog_len(db: *const callbook_db) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return 0;
    };
    handle
        .db
        .interest_catalog()
        .map_or(0, |catalog| clamp_len(catalog.len()))
}

/// Return one interest definition by catalog index.
///
/// Release the returned object with [`callbook_interest_definition_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_catalog_get(
    db: *const callbook_db,
    index: c_int,
    out: *mut *mut callbook_interest_definition,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    if index < 0 {
        return CALLBOOK_E_USAGE;
    }
    let Some(definition) = handle
        .db
        .interest_catalog()
        .and_then(|catalog| catalog.definitions().nth(index as usize))
    else {
        return CALLBOOK_E_NOT_FOUND;
    };
    unsafe {
        *out = Box::into_raw(Box::new(callbook_interest_definition::from(
            definition.clone(),
        )))
    };
    0
}

/// Look up one interest-code definition.
///
/// Release the returned object with [`callbook_interest_definition_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer, `code` must be a valid
/// NUL-terminated UTF-8 string, and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_catalog_lookup(
    db: *const callbook_db,
    code: *const c_char,
    out: *mut *mut callbook_interest_definition,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(code) = read_cstr(code) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(definition) = handle
        .db
        .interest_catalog()
        .and_then(|catalog| catalog.lookup(&code))
    else {
        return CALLBOOK_E_NOT_FOUND;
    };
    unsafe {
        *out = Box::into_raw(Box::new(callbook_interest_definition::from(
            definition.clone(),
        )))
    };
    0
}

/// Free an owned interest definition. Safe to pass null.
///
/// # Safety
///
/// `definition` must be null or an owned definition pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_definition_free(
    definition: *mut callbook_interest_definition,
) {
    if definition.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(definition) });
}

/// Return an interest definition's code.
///
/// # Safety
///
/// `definition` must be null or a valid definition pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_definition_code(
    definition: *const callbook_interest_definition,
) -> *const c_char {
    let Some(definition) = (unsafe { definition.as_ref() }) else {
        return empty_cstr();
    };
    definition.code.as_ptr()
}

/// Return an interest definition's category.
///
/// # Safety
///
/// `definition` must be null or a valid definition pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_definition_category(
    definition: *const callbook_interest_definition,
) -> *const c_char {
    let Some(definition) = (unsafe { definition.as_ref() }) else {
        return empty_cstr();
    };
    definition.category.as_ptr()
}

/// Return an interest definition's label.
///
/// # Safety
///
/// `definition` must be null or a valid definition pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_definition_label(
    definition: *const callbook_interest_definition,
) -> *const c_char {
    let Some(definition) = (unsafe { definition.as_ref() }) else {
        return empty_cstr();
    };
    definition.label.as_ptr()
}

/// Search decoded records for a four-digit interest code.
///
/// Release the returned object with [`callbook_interest_search_free`].
///
/// # Safety
///
/// `db` must be a valid database pointer, `code` must be a valid
/// NUL-terminated UTF-8 string, and `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_for_code(
    db: *const callbook_db,
    code: *const c_char,
    out: *mut *mut callbook_interest_search_result,
) -> c_int {
    if out.is_null() {
        return CALLBOOK_E_USAGE;
    }
    unsafe { *out = std::ptr::null_mut() };
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(code) = read_cstr(code) else {
        return CALLBOOK_E_USAGE;
    };
    match handle.db.search_interest(&code) {
        Ok(search) => {
            unsafe {
                *out = Box::into_raw(Box::new(callbook_interest_search_result::from(search)))
            };
            0
        }
        Err(_) => CALLBOOK_E_USAGE,
    }
}

/// Free an interest search returned by [`callbook_interest_search_for_code`].
///
/// # Safety
///
/// `search` must be null or an owned interest-search pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_free(
    search: *mut callbook_interest_search_result,
) {
    if search.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(search) });
}

/// Return the normalized interest-search code.
///
/// # Safety
///
/// `search` must be null or a valid interest-search pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_code(
    search: *const callbook_interest_search_result,
) -> *const c_char {
    let Some(search) = (unsafe { search.as_ref() }) else {
        return empty_cstr();
    };
    search.code.as_ptr()
}

/// Return the definition for this interest search, or null.
///
/// # Safety
///
/// `search` must be null or a valid interest-search pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_definition(
    search: *const callbook_interest_search_result,
) -> *const callbook_interest_definition {
    let Some(search) = (unsafe { search.as_ref() }) else {
        return std::ptr::null();
    };
    search
        .definition
        .as_ref()
        .map_or(std::ptr::null(), |definition| definition as *const _)
}

/// Return the number of interest-search matches.
///
/// # Safety
///
/// `search` must be null or a valid interest-search pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_match_len(
    search: *const callbook_interest_search_result,
) -> c_int {
    let Some(search) = (unsafe { search.as_ref() }) else {
        return 0;
    };
    clamp_len(search.matches.len())
}

/// Return one interest-search match by zero-based index, or null.
///
/// # Safety
///
/// `search` must be null or a valid interest-search pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_match_get(
    search: *const callbook_interest_search_result,
    index: c_int,
) -> *const callbook_interest_search_match {
    let Some(search) = (unsafe { search.as_ref() }) else {
        return std::ptr::null();
    };
    if index < 0 {
        return std::ptr::null();
    }
    search
        .matches
        .get(index as usize)
        .map_or(std::ptr::null(), |entry| entry as *const _)
}

/// Return the callsign for an interest-search match.
///
/// # Safety
///
/// `entry` must be null or a valid match pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_match_callsign(
    entry: *const callbook_interest_search_match,
) -> *const c_char {
    let Some(entry) = (unsafe { entry.as_ref() }) else {
        return empty_cstr();
    };
    entry.callsign.as_ptr()
}

/// Return the archive vintage for an interest-search match, or -1 for current.
///
/// # Safety
///
/// `entry` must be null or a valid match pointer.
#[no_mangle]
pub unsafe extern "C" fn callbook_interest_search_match_vintage(
    entry: *const callbook_interest_search_match,
) -> c_int {
    let Some(entry) = (unsafe { entry.as_ref() }) else {
        return -1;
    };
    entry.vintage
}

/// Render the default station map SVG into `out_buf`.
///
/// Returns the number of bytes written, excluding NUL. Use
/// [`callbook_map_svg_required_len`] to size the output buffer.
///
/// # Safety
///
/// `db` must be a valid database pointer, `callsign` must be a valid
/// NUL-terminated UTF-8 string, and `out_buf` must be writable.
#[no_mangle]
pub unsafe extern "C" fn callbook_map_svg(
    db: *const callbook_db,
    callsign: *const c_char,
    out_buf: *mut c_char,
    out_buf_len: c_int,
) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match map_svg(&handle.db, &call) {
        Ok(svg) => write_buf(out_buf, out_buf_len, svg.as_bytes()),
        Err(code) => code,
    }
}

/// Return the required buffer length for [`callbook_map_svg`], including NUL.
///
/// # Safety
///
/// `db` must be a valid database pointer and `callsign` must be a valid
/// NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn callbook_map_svg_required_len(
    db: *const callbook_db,
    callsign: *const c_char,
) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match map_svg(&handle.db, &call) {
        Ok(svg) => clamp_len(svg.len() + 1),
        Err(code) => code,
    }
}

/// Look up `callsign` and write the structured result as JSON.
///
/// Returns the number of bytes written, excluding the trailing NUL. A
/// not-found callsign writes a JSON object with `"status": "NotFound"` and
/// still returns success. Use [`callbook_lookup_json_required_len`] to size
/// the output buffer.
///
/// # Safety
///
/// `db` must be null or a pointer returned by [`callbook_open`]. `callsign`
/// must be a valid NUL-terminated UTF-8 string. `out_buf` must be writable
/// for `out_buf_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_json(
    db: *const callbook_db,
    callsign: *const c_char,
    out_buf: *mut c_char,
    out_buf_len: c_int,
) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    if out_buf.is_null() || out_buf_len <= 0 {
        return CALLBOOK_E_USAGE;
    }
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    let Ok(json) = lookup_json(&handle.db, &call) else {
        return CALLBOOK_E_USAGE;
    };
    write_buf(out_buf, out_buf_len, json.as_bytes())
}

/// Return the buffer size required for [`callbook_lookup_json`], including NUL.
///
/// # Safety
///
/// `db` must be null or a pointer returned by [`callbook_open`]. `callsign`
/// must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn callbook_lookup_json_required_len(
    db: *const callbook_db,
    callsign: *const c_char,
) -> c_int {
    let Some(handle) = (unsafe { db.as_ref() }) else {
        return CALLBOOK_E_USAGE;
    };
    let Some(call) = read_cstr(callsign) else {
        return CALLBOOK_E_USAGE;
    };
    match lookup_json(&handle.db, &call) {
        Ok(json) => clamp_len(json.len() + 1),
        Err(_) => CALLBOOK_E_USAGE,
    }
}

/// Crate version string (NUL-terminated, static).
#[no_mangle]
pub extern "C" fn callbook_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

/// Map a negative return code from any of the `callbook_*` functions to a
/// short, human-readable, NUL-terminated, statically-allocated description.
///
/// Returns a pointer to `"unknown error"` for codes that are not recognised,
/// and `"ok"` for non-negative inputs (which are not errors).
#[no_mangle]
pub extern "C" fn callbook_strerror(code: c_int) -> *const c_char {
    static OK: &[u8] = b"ok\0";
    static MISS: &[u8] = b"callsign not found\0";
    static USAGE: &[u8] = b"invalid argument (null pointer, bad UTF-8, or zero buffer length)\0";
    static OPEN_FAIL: &[u8] = b"failed to open database (path missing, malformed, or unreadable)\0";
    static TOO_SMALL: &[u8] =
        b"output buffer too small (use the *_required_len query to size it)\0";
    static UNKNOWN: &[u8] = b"unknown error\0";
    let bytes: &[u8] = match code {
        c if c >= 0 => OK,
        CALLBOOK_E_NOT_FOUND => MISS,
        CALLBOOK_E_USAGE => USAGE,
        CALLBOOK_E_OPEN_FAILED => OPEN_FAIL,
        CALLBOOK_E_BUFFER_TOO_SMALL => TOO_SMALL,
        _ => UNKNOWN,
    };
    bytes.as_ptr().cast()
}

impl From<LookupResult> for callbook_lookup_result {
    fn from(result: LookupResult) -> Self {
        Self {
            query: to_cstring(&result.query),
            status: ffi_status(result.status),
            current: result.current.map(callbook_snapshot::from),
            history: result
                .history
                .into_iter()
                .map(callbook_snapshot::from)
                .collect(),
        }
    }
}

impl From<CallSnapshot> for callbook_snapshot {
    fn from(snapshot: CallSnapshot) -> Self {
        let mut fields = vec![to_cstring(""); MODERN_FIELD_COUNT];
        let name = snapshot.display_name();
        set_field(
            &mut fields,
            callbook_modern_field::Callsign,
            Some(snapshot.callsign),
        );
        set_field(&mut fields, callbook_modern_field::Name, name);
        set_field(
            &mut fields,
            callbook_modern_field::FirstName,
            snapshot.first_name,
        );
        set_field(
            &mut fields,
            callbook_modern_field::MiddleName,
            snapshot.middle_name,
        );
        set_field(
            &mut fields,
            callbook_modern_field::LastName,
            snapshot.last_name,
        );
        set_field(&mut fields, callbook_modern_field::Suffix, snapshot.suffix);
        set_field(
            &mut fields,
            callbook_modern_field::Address,
            snapshot.address,
        );
        set_field(&mut fields, callbook_modern_field::City, snapshot.city);
        set_field(
            &mut fields,
            callbook_modern_field::StateOrProvince,
            snapshot.state_or_province,
        );
        set_field(
            &mut fields,
            callbook_modern_field::PostalCode,
            snapshot.postal_code,
        );
        set_field(&mut fields, callbook_modern_field::County, snapshot.county);
        set_field(
            &mut fields,
            callbook_modern_field::Country,
            snapshot.country,
        );
        set_field(
            &mut fields,
            callbook_modern_field::LicenseClass,
            snapshot.license_class,
        );
        set_field(
            &mut fields,
            callbook_modern_field::RecordCode,
            snapshot.record_code,
        );
        set_field(
            &mut fields,
            callbook_modern_field::BirthDate,
            snapshot.birth_date,
        );
        set_field(
            &mut fields,
            callbook_modern_field::FirstIssued,
            snapshot.first_issued,
        );
        set_field(
            &mut fields,
            callbook_modern_field::Expires,
            snapshot.expires,
        );
        set_field(
            &mut fields,
            callbook_modern_field::LastChanged,
            snapshot.last_changed,
        );
        set_field(
            &mut fields,
            callbook_modern_field::GmtOffset,
            snapshot.gmt_offset,
        );
        set_field(
            &mut fields,
            callbook_modern_field::Latitude,
            snapshot.latitude,
        );
        set_field(
            &mut fields,
            callbook_modern_field::Longitude,
            snapshot.longitude,
        );
        set_field(&mut fields, callbook_modern_field::Grid, snapshot.grid);
        set_field(
            &mut fields,
            callbook_modern_field::AreaCode,
            snapshot.area_code,
        );
        set_field(
            &mut fields,
            callbook_modern_field::PreviousCall,
            snapshot.previous_call,
        );
        set_field(
            &mut fields,
            callbook_modern_field::PreviousClass,
            snapshot.previous_class,
        );
        set_field(
            &mut fields,
            callbook_modern_field::FccTransactionType,
            snapshot.fcc_transaction_type,
        );
        set_field(&mut fields, callbook_modern_field::Email, snapshot.email);
        set_field(&mut fields, callbook_modern_field::Qsl, snapshot.qsl);
        set_field(&mut fields, callbook_modern_field::Url, snapshot.url);
        set_field(
            &mut fields,
            callbook_modern_field::FaxNumber,
            snapshot.fax_number,
        );
        set_field(&mut fields, callbook_modern_field::Iota, snapshot.iota);
        set_field(
            &mut fields,
            callbook_modern_field::Interests,
            snapshot.interest_codes_raw,
        );
        set_field(
            &mut fields,
            callbook_modern_field::LicenseId,
            snapshot.license_id,
        );
        set_field(&mut fields, callbook_modern_field::Frn, snapshot.frn);
        set_field(
            &mut fields,
            callbook_modern_field::NumericId,
            snapshot.numeric_id,
        );

        let interest_codes = snapshot
            .interest_codes
            .into_iter()
            .map(|code| to_cstring(&code))
            .collect();
        let interests = snapshot
            .interests
            .into_iter()
            .map(callbook_interest::from)
            .collect();

        Self {
            fields,
            interest_codes,
            interests,
            vintage: snapshot.vintage.map_or(-1, c_int::from),
            source_flags: source_flags(&snapshot.sources),
            jurisdiction: ffi_jurisdiction(snapshot.jurisdiction),
        }
    }
}

impl From<ResolvedInterest> for callbook_interest {
    fn from(interest: ResolvedInterest) -> Self {
        Self {
            code: to_cstring(&interest.code),
            category: to_cstring(&interest.category),
            label: to_cstring(&interest.label),
        }
    }
}

impl From<StationProfile> for callbook_profile {
    fn from(profile: StationProfile) -> Self {
        let mut assets = Vec::new();
        assets.extend(profile.assets.photos().iter().map(callbook_asset::from));
        if let Some(bio) = profile.assets.bio() {
            assets.push(callbook_asset::from(bio));
        }
        if let Some(flag) = profile.assets.country_flag() {
            assets.push(callbook_asset::from(flag));
        }
        if let Some(map) = profile.assets.country_map() {
            assets.push(callbook_asset::from(map));
        }

        Self {
            callsign: to_cstring(&profile.callsign),
            status: ffi_status(profile.status),
            current: profile.current.map(callbook_snapshot::from),
            history_snapshot_count: clamp_len(profile.history.snapshot_count),
            history_vintages: profile
                .history
                .vintages
                .into_iter()
                .map(c_int::from)
                .collect(),
            country: profile.country.map(callbook_country_info::from),
            lookup_count: profile.lookup_count.map(callbook_lookup_count::from),
            assets,
        }
    }
}

impl From<CountryInfo> for callbook_country_info {
    fn from(country: CountryInfo) -> Self {
        Self {
            name: to_cstring(&country.name),
            raw_name: to_cstring(&country.raw_name),
            cleaned_name: to_cstring(&country.cleaned_name),
            code: to_cstring(country.code.unwrap_or_default()),
            jurisdiction: ffi_jurisdiction(country.jurisdiction),
            itu_zone: country.itu_zone.map_or(-1, c_int::from),
            cq_zone: country.cq_zone.map_or(-1, c_int::from),
            continent: to_cstring(country.continent.unwrap_or_default()),
            latitude: country.latitude,
            longitude: country.longitude,
            numeric_code: country.numeric_code.map_or(-1, c_int::from),
            source: ffi_country_source(country.source),
        }
    }
}

impl From<LookupCountRecord> for callbook_lookup_count {
    fn from(count: LookupCountRecord) -> Self {
        Self {
            key: to_cstring(count.key),
            count: count.count,
            updated_yyyymmdd: count
                .updated_yyyymmdd
                .map_or(-1, |date| c_int::try_from(date).unwrap_or(c_int::MAX)),
            status: to_cstring(
                count
                    .status
                    .map_or_else(String::new, |status| status.to_string()),
            ),
        }
    }
}

impl From<&crate::AssetRef> for callbook_asset {
    fn from(asset: &crate::AssetRef) -> Self {
        Self {
            kind: ffi_asset_kind(asset.kind),
            key: to_cstring(&asset.key),
            media_type: to_cstring(&asset.media_type),
            path: to_cstring(asset.path.to_string_lossy()),
        }
    }
}

impl From<&crate::TextAssetRef> for callbook_asset {
    fn from(asset: &crate::TextAssetRef) -> Self {
        Self {
            kind: ffi_asset_kind(asset.kind),
            key: to_cstring(&asset.key),
            media_type: to_cstring(&asset.media_type),
            path: to_cstring(asset.path.to_string_lossy()),
        }
    }
}

impl From<&AssetMetadata> for callbook_asset {
    fn from(asset: &AssetMetadata) -> Self {
        Self {
            kind: ffi_asset_kind(asset.kind),
            key: to_cstring(&asset.key),
            media_type: to_cstring(&asset.media_type),
            path: to_cstring(asset.path.to_string_lossy()),
        }
    }
}

impl From<UsCsvRecord> for callbook_us_record {
    fn from(record: UsCsvRecord) -> Self {
        Self::from(&record)
    }
}

impl From<&UsCsvRecord> for callbook_us_record {
    fn from(record: &UsCsvRecord) -> Self {
        let mut fields = vec![to_cstring(""); US_FIELD_COUNT];
        fields[callbook_us_field::Callsign as usize] = to_cstring(&record.callsign);
        fields[callbook_us_field::Class as usize] = to_cstring(&record.class);
        fields[callbook_us_field::Name as usize] = to_cstring(&record.name);
        fields[callbook_us_field::Address as usize] = to_cstring(&record.address);
        fields[callbook_us_field::City as usize] = to_cstring(&record.city);
        fields[callbook_us_field::State as usize] = to_cstring(&record.state);
        fields[callbook_us_field::Zip as usize] = to_cstring(&record.zip);
        fields[callbook_us_field::County as usize] = to_cstring(&record.county);
        fields[callbook_us_field::LicenseIssueDate as usize] =
            to_cstring(&record.license_issue_date);
        fields[callbook_us_field::FccTransactionType as usize] =
            to_cstring(&record.fcc_transaction_type);
        Self { fields }
    }
}

impl From<InterestDefinition> for callbook_interest_definition {
    fn from(definition: InterestDefinition) -> Self {
        Self {
            code: to_cstring(definition.code),
            category: to_cstring(definition.category),
            label: to_cstring(definition.label),
        }
    }
}

impl From<InterestSearch> for callbook_interest_search_result {
    fn from(search: InterestSearch) -> Self {
        Self {
            code: to_cstring(search.code),
            definition: search.definition.map(callbook_interest_definition::from),
            matches: search
                .matches
                .into_iter()
                .map(callbook_interest_search_match::from)
                .collect(),
        }
    }
}

impl From<InterestSearchMatch> for callbook_interest_search_match {
    fn from(entry: InterestSearchMatch) -> Self {
        Self {
            callsign: to_cstring(entry.callsign),
            vintage: entry.vintage.map_or(-1, c_int::from),
        }
    }
}

fn set_field(fields: &mut [CString], field: callbook_modern_field, value: Option<String>) {
    fields[field as usize] = to_cstring(value.unwrap_or_default());
}

fn ffi_status(status: LookupStatus) -> callbook_lookup_status {
    match status {
        LookupStatus::Current => callbook_lookup_status::Current,
        LookupStatus::ArchiveOnly => callbook_lookup_status::ArchiveOnly,
        LookupStatus::NotFound => callbook_lookup_status::NotFound,
    }
}

fn ffi_jurisdiction(jurisdiction: Jurisdiction) -> callbook_jurisdiction {
    match jurisdiction {
        Jurisdiction::UnitedStates => callbook_jurisdiction::UnitedStates,
        Jurisdiction::Canada => callbook_jurisdiction::Canada,
        Jurisdiction::International => callbook_jurisdiction::International,
        Jurisdiction::Unknown => callbook_jurisdiction::Unknown,
    }
}

fn ffi_country_source(source: CountryInfoSource) -> callbook_country_source {
    match source {
        CountryInfoSource::Unknown => callbook_country_source::Unknown,
        CountryInfoSource::Countrys => callbook_country_source::Countrys,
        CountryInfoSource::GcMcountrys => callbook_country_source::GcMcountrys,
        CountryInfoSource::CountrysPc => callbook_country_source::CountrysPc,
    }
}

fn ffi_asset_kind(kind: AssetKind) -> callbook_asset_kind {
    match kind {
        AssetKind::Biography => callbook_asset_kind::Biography,
        AssetKind::Photo => callbook_asset_kind::Photo,
        AssetKind::Flag => callbook_asset_kind::Flag,
        AssetKind::Map => callbook_asset_kind::Map,
        AssetKind::SidecarData => callbook_asset_kind::SidecarData,
    }
}

fn source_flags(sources: &[SnapshotSource]) -> u32 {
    sources.iter().fold(0, |flags, source| {
        flags
            | match source {
                SnapshotSource::UsCsv => CALLBOOK_SOURCE_US_CSV,
                SnapshotSource::HamCallDatIdx => CALLBOOK_SOURCE_HAMCALL_DAT_IDX,
                SnapshotSource::HamCallHci => CALLBOOK_SOURCE_HAMCALL_HCI,
            }
    })
}

fn lookup_json(db: &CallBook, callsign: &str) -> crate::Result<String> {
    let result = db.lookup_report(callsign)?;
    Ok(serde_json::to_string_pretty(&lookup_json_value(&result)).expect("JSON value serializes"))
}

fn map_svg(db: &CallBook, callsign: &str) -> std::result::Result<String, c_int> {
    let map = db
        .map_for_callsign(callsign)
        .map_err(|_| CALLBOOK_E_USAGE)?
        .ok_or(CALLBOOK_E_NOT_FOUND)?;
    map.render_svg()
        .map_err(|_| CALLBOOK_E_USAGE)?
        .ok_or(CALLBOOK_E_NOT_FOUND)
}

fn lookup_json_value(result: &LookupResult) -> serde_json::Value {
    serde_json::json!({
        "query": result.query,
        "status": format!("{:?}", result.status),
        "current": result.current.as_ref().map(snapshot_json_value),
        "history": result.history.iter().map(snapshot_json_value).collect::<Vec<_>>(),
    })
}

fn snapshot_json_value(snapshot: &CallSnapshot) -> serde_json::Value {
    serde_json::json!({
        "callsign": snapshot.callsign,
        "vintage": snapshot.vintage,
        "sources": snapshot.sources.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>(),
        "jurisdiction": format!("{:?}", snapshot.jurisdiction),
        "license_class": snapshot.license_class,
        "record_code": snapshot.record_code,
        "name": snapshot.display_name(),
        "first_name": snapshot.first_name,
        "middle_name": snapshot.middle_name,
        "last_name": snapshot.last_name,
        "suffix": snapshot.suffix,
        "address": snapshot.address,
        "city": snapshot.city,
        "state_or_province": snapshot.state_or_province,
        "postal_code": snapshot.postal_code,
        "county": snapshot.county,
        "country": snapshot.country,
        "birth_date": snapshot.birth_date,
        "first_issued": snapshot.first_issued,
        "expires": snapshot.expires,
        "last_changed": snapshot.last_changed,
        "gmt_offset": snapshot.gmt_offset,
        "latitude": snapshot.latitude,
        "longitude": snapshot.longitude,
        "grid": snapshot.grid,
        "area_code": snapshot.area_code,
        "previous_call": snapshot.previous_call,
        "previous_class": snapshot.previous_class,
        "fcc_transaction_type": snapshot.fcc_transaction_type,
        "email": snapshot.email,
        "qsl": snapshot.qsl,
        "url": snapshot.url,
        "fax_number": snapshot.fax_number,
        "iota": snapshot.iota,
        "interest_codes_raw": snapshot.interest_codes_raw,
        "interest_codes": snapshot.interest_codes,
        "interests": snapshot.interests.iter().map(interest_json_value).collect::<Vec<_>>(),
        "license_id": snapshot.license_id,
        "frn": snapshot.frn,
        "numeric_id": snapshot.numeric_id,
        "raw_tags": snapshot.raw_tags.iter().map(|(k, v)| (format!("{k:02x}"), serde_json::Value::String(v.clone()))).collect::<serde_json::Map<_, _>>(),
    })
}

fn interest_json_value(interest: &ResolvedInterest) -> serde_json::Value {
    serde_json::json!({
        "code": interest.code,
        "category": interest.category,
        "label": interest.label,
    })
}

fn read_cstr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().ok().map(str::to_owned)
}

fn to_cstring(value: impl AsRef<str>) -> CString {
    let mut bytes = value.as_ref().as_bytes().to_vec();
    for byte in &mut bytes {
        if *byte == 0 {
            *byte = b' ';
        }
    }
    CString::new(bytes).expect("NUL bytes are sanitized")
}

fn empty_cstr() -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    EMPTY.as_ptr().cast()
}

/// Copy `bytes` into the C buffer (NUL-terminated) if it fits; otherwise
/// return [`CALLBOOK_E_BUFFER_TOO_SMALL`].
fn write_buf(out_buf: *mut c_char, out_buf_len: c_int, bytes: &[u8]) -> c_int {
    if out_buf.is_null() || out_buf_len <= 0 {
        return CALLBOOK_E_USAGE;
    }
    let need = bytes.len();
    let cap = out_buf_len as usize;
    let Some(required) = need.checked_add(1) else {
        return CALLBOOK_E_BUFFER_TOO_SMALL;
    };
    if required > cap {
        return CALLBOOK_E_BUFFER_TOO_SMALL;
    }
    // SAFETY: caller guarantees out_buf points to at least cap bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf.cast::<u8>(), need);
        *out_buf.add(need) = 0;
    }
    clamp_len(need)
}

/// Saturate a `usize` length to a positive `c_int`, never returning a
/// value that would collide with the negative error space.
fn clamp_len(n: usize) -> c_int {
    let max = c_int::MAX as usize;
    n.min(max) as c_int
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_abi_exposes_raw_and_resolved_interests() {
        let mut snapshot =
            CallSnapshot::new("K0AB".to_owned(), None, SnapshotSource::HamCallDatIdx);
        snapshot.interest_codes_raw = Some("00100020".to_owned());
        snapshot.interest_codes = vec!["0010".to_owned(), "0020".to_owned()];
        snapshot.interests = vec![ResolvedInterest {
            code: "0010".to_owned(),
            category: "Bands".to_owned(),
            label: "160 meters".to_owned(),
        }];
        let snapshot = callbook_snapshot::from(snapshot);
        let snapshot = &snapshot as *const callbook_snapshot;

        let raw = unsafe { CStr::from_ptr(callbook_snapshot_interest_codes_raw(snapshot)) };
        assert_eq!(raw.to_str().unwrap(), "00100020");
        assert_eq!(unsafe { callbook_snapshot_interest_code_len(snapshot) }, 2);

        let code = unsafe { CStr::from_ptr(callbook_snapshot_interest_code_get(snapshot, 1)) };
        assert_eq!(code.to_str().unwrap(), "0020");
        assert_eq!(unsafe { callbook_snapshot_interest_len(snapshot) }, 1);

        let interest = unsafe { callbook_snapshot_interest_get(snapshot, 0) };
        assert!(!interest.is_null());
        let category = unsafe { CStr::from_ptr(callbook_interest_category(interest)) };
        let label = unsafe { CStr::from_ptr(callbook_interest_label(interest)) };
        assert_eq!(category.to_str().unwrap(), "Bands");
        assert_eq!(label.to_str().unwrap(), "160 meters");
        assert!(unsafe { callbook_snapshot_interest_get(snapshot, 1) }.is_null());
    }

    #[test]
    fn modern_structured_abi_uses_reusable_database_handle() {
        let Some(path) = std::env::var_os("CALLBOOK_DB") else {
            return;
        };
        let path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let call = CString::new("W1AW").unwrap();

        let mut db = std::ptr::null_mut();
        assert_eq!(unsafe { callbook_open(path.as_ptr(), &mut db) }, 0);
        assert!(!db.is_null());

        let mut result = std::ptr::null_mut();
        assert_eq!(
            unsafe { callbook_lookup_modern(db, call.as_ptr(), &mut result) },
            0
        );
        assert!(!result.is_null());
        assert_eq!(
            unsafe { callbook_result_status(result) },
            callbook_lookup_status::Current
        );

        let current = unsafe { callbook_result_current(result) };
        assert!(!current.is_null());
        let name = unsafe {
            CStr::from_ptr(callbook_snapshot_field(
                current,
                callbook_modern_field::Name as c_int,
            ))
        };
        assert!(!name.to_str().unwrap().is_empty());
        assert!(unsafe { callbook_result_history_len(result) } >= 1);

        unsafe {
            callbook_result_free(result);
            callbook_close(db);
        }
    }

    #[test]
    fn modern_json_abi_reports_required_length_and_writes_json() {
        let Some(path) = std::env::var_os("CALLBOOK_DB") else {
            return;
        };
        let path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let call = CString::new("W1AW").unwrap();

        let mut db = std::ptr::null_mut();
        assert_eq!(unsafe { callbook_open(path.as_ptr(), &mut db) }, 0);
        assert!(!db.is_null());

        let required = unsafe { callbook_lookup_json_required_len(db, call.as_ptr()) };
        assert!(required > 1);

        let mut too_small = [0i8; 2];
        assert_eq!(
            unsafe {
                callbook_lookup_json(
                    db,
                    call.as_ptr(),
                    too_small.as_mut_ptr(),
                    too_small.len() as c_int,
                )
            },
            CALLBOOK_E_BUFFER_TOO_SMALL
        );

        let mut buf = vec![0i8; required as usize];
        let written = unsafe {
            callbook_lookup_json(db, call.as_ptr(), buf.as_mut_ptr(), buf.len() as c_int)
        };
        assert_eq!(written + 1, required);
        let json = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap();
        assert!(json.contains("\"status\": \"Current\""));
        assert!(json.contains("\"callsign\": \"W1AW\""));

        unsafe { callbook_close(db) };
    }

    #[test]
    fn write_buf_rejects_invalid_buffers() {
        let mut out = [0i8; 8];

        assert_eq!(
            write_buf(std::ptr::null_mut(), out.len() as c_int, b"x"),
            CALLBOOK_E_USAGE
        );
        assert_eq!(write_buf(out.as_mut_ptr(), 0, b"x"), CALLBOOK_E_USAGE);
        assert_eq!(write_buf(out.as_mut_ptr(), -1, b"x"), CALLBOOK_E_USAGE);
        assert_eq!(
            write_buf(out.as_mut_ptr(), 1, b"x"),
            CALLBOOK_E_BUFFER_TOO_SMALL
        );
        assert_eq!(write_buf(out.as_mut_ptr(), out.len() as c_int, b"x"), 1);
    }

    #[test]
    fn profile_abi_exposes_workflow_summary() {
        let dir = tempfile::tempdir().unwrap();
        let ham0_dir = dir.path().join("ham0");
        std::fs::create_dir_all(ham0_dir.join("photos")).unwrap();
        std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
        std::fs::write(
            ham0_dir.join("hamcall.idx"),
            b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
        )
        .unwrap();
        std::fs::write(ham0_dir.join("photos/K0AB.JPG"), "jpg").unwrap();

        let path = CString::new(dir.path().to_string_lossy().as_bytes()).unwrap();
        let call = CString::new("k0ab").unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(unsafe { callbook_open(path.as_ptr(), &mut db) }, 0);

        let mut profile = std::ptr::null_mut();
        assert_eq!(
            unsafe { callbook_profile_for_callsign(db, call.as_ptr(), &mut profile) },
            0
        );
        assert!(!profile.is_null());
        let callsign = unsafe { CStr::from_ptr(callbook_profile_callsign(profile)) };
        assert_eq!(callsign.to_str().unwrap(), "K0AB");
        assert_eq!(
            unsafe { callbook_profile_status(profile) },
            callbook_lookup_status::NotFound
        );
        assert_eq!(unsafe { callbook_profile_asset_len(profile) }, 1);
        let asset = unsafe { callbook_profile_asset_get(profile, 0) };
        assert!(!asset.is_null());
        assert_eq!(
            unsafe { callbook_asset_kind_value(asset) },
            callbook_asset_kind::Photo
        );
        let media_type = unsafe { CStr::from_ptr(callbook_asset_media_type(asset)) };
        assert_eq!(media_type.to_str().unwrap(), "image/jpeg");

        unsafe {
            callbook_profile_free(profile);
            callbook_close(db);
        }
    }

    #[test]
    fn catalog_abi_exposes_current_us_and_interest_workflows() {
        use std::io::Write;

        fn write_usa_csv_zip(path: &std::path::Path, csv: &str) {
            let file = std::fs::File::create(path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("usa.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(csv.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let ham0_dir = dir.path().join("ham0");
        std::fs::create_dir_all(&ham0_dir).unwrap();
        std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
        std::fs::write(
            ham0_dir.join("hamcall.idx"),
            b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
        )
        .unwrap();
        write_usa_csv_zip(
            &ham0_dir.join("usa.csv.zip"),
            concat!(
                "\"Callsign\",\"Class\",\"Name\",\"Address\",\"City\",\"State\",\"ZIP\",\"County\",\"License Issue Date\",\"FCC Transaction Type\"\n",
                "\"K0AB\",\"E\",\"Example Operator\",\"1 Test Way\",\"Example City\",\"NJ\",\"00000\",\"Example\",\"20200101\",\"LIAUA\"\n",
            ),
        );
        std::fs::write(ham0_dir.join("interest"), "--- Bands\n0010 * 160 meters\n").unwrap();

        let path = CString::new(dir.path().to_string_lossy().as_bytes()).unwrap();
        let call = CString::new("k0ab").unwrap();
        let mut db = std::ptr::null_mut();
        assert_eq!(unsafe { callbook_open(path.as_ptr(), &mut db) }, 0);

        assert_eq!(unsafe { callbook_current_us_record_count(db) }, 1);
        let mut us = std::ptr::null_mut();
        assert_eq!(
            unsafe { callbook_current_us_lookup(db, call.as_ptr(), &mut us) },
            0
        );
        let state = unsafe {
            CStr::from_ptr(callbook_us_record_field(
                us,
                callbook_us_field::State as c_int,
            ))
        };
        assert_eq!(state.to_str().unwrap(), "NJ");

        assert_eq!(unsafe { callbook_interest_catalog_len(db) }, 1);
        let code = CString::new("0010").unwrap();
        let mut definition = std::ptr::null_mut();
        assert_eq!(
            unsafe { callbook_interest_catalog_lookup(db, code.as_ptr(), &mut definition) },
            0
        );
        let label = unsafe { CStr::from_ptr(callbook_interest_definition_label(definition)) };
        assert_eq!(label.to_str().unwrap(), "160 meters");

        let missing = unsafe { callbook_map_svg_required_len(db, call.as_ptr()) };
        assert_eq!(missing, CALLBOOK_E_NOT_FOUND);

        unsafe {
            callbook_interest_definition_free(definition);
            callbook_us_record_free(us);
            callbook_close(db);
        }
    }

    #[test]
    fn country_and_count_abi_accessors_preserve_optional_fields() {
        let country = callbook_country_info::from(CountryInfo {
            name: "United States".to_owned(),
            raw_name: "UNITED STATES".to_owned(),
            cleaned_name: "United States".to_owned(),
            code: Some("K".to_owned()),
            jurisdiction: Jurisdiction::UnitedStates,
            itu_zone: Some(8),
            cq_zone: None,
            continent: Some("NA".to_owned()),
            latitude: Some(38.0),
            longitude: Some(-97.0),
            numeric_code: Some(291),
            source: CountryInfoSource::Countrys,
        });
        let country = &country as *const callbook_country_info;
        assert_eq!(
            unsafe { callbook_country_jurisdiction(country) },
            callbook_jurisdiction::UnitedStates
        );
        assert_eq!(unsafe { callbook_country_itu_zone(country) }, 8);
        assert_eq!(unsafe { callbook_country_cq_zone(country) }, -1);
        let mut lat = 0.0;
        assert_eq!(unsafe { callbook_country_latitude(country, &mut lat) }, 0);
        assert_eq!(lat, 38.0);
        assert_eq!(
            unsafe { callbook_country_source_value(country) },
            callbook_country_source::Countrys
        );

        let count = callbook_lookup_count::from(LookupCountRecord {
            key: "K0AB".to_owned(),
            count: 7,
            updated_yyyymmdd: Some(20251101),
            status: Some('A'),
        });
        let count = &count as *const callbook_lookup_count;
        assert_eq!(unsafe { callbook_lookup_count_value(count) }, 7);
        assert_eq!(
            unsafe { callbook_lookup_count_updated_yyyymmdd(count) },
            20251101
        );
        let status = unsafe { CStr::from_ptr(callbook_lookup_count_status(count)) };
        assert_eq!(status.to_str().unwrap(), "A");
    }
}
