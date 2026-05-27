//! Native reader for the Buckmaster HamCall callsign database.
//!
//! HamCall is a commercial product distributed by Buckmaster International on
//! DVD/USB/download. This crate is an independent compatibility reader for
//! locally installed database files. It does not ship, mirror, or redistribute
//! any Buckmaster data — you must purchase a licensed copy of the database from
//! <https://hamcall.net> to use this crate.
//!
//! # On-disk format
//!
//! The supported layout is the `ham0` database: `<root>/ham0/hamcall.{dat,idx}`
//! plus `hci.dat`/`hciindex.dat`, `usa.csv.zip`, and related sidecars. IDX is
//! plain ASCII text; DAT and HCI use a verified position-dependent XOR transform.
//!
//! # Quick start
//!
//! ```no_run
//! use callbook::CallBook;
//!
//! let db = CallBook::open("/path/to/HAMCALL")?;
//! let entry = db.lookup("W1AW")?;
//! if let Some(record) = entry.current() {
//!     println!("{}: {:?}", record.callsign, record.country);
//! }
//! # Ok::<(), callbook::Error>(())
//! ```

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![allow(clippy::module_name_repetitions)]

pub mod callsign;
pub mod country;
mod db;
pub mod error;
mod ffi;
mod format;
mod hci;
mod idx_text;
pub mod interest;
pub mod modern;
#[path = "sidecar.rs"]
mod sidecar_impl;
/// Semantic sidecar data returned by workflow APIs.
pub mod sidecar {
    pub use crate::sidecar_impl::{
        BoundaryBox, BoundaryDataset, BoundaryKind, BoundarySegment, GeoPoint, LookupCountIter,
        LookupCountRecord, LookupCounts, StateVectorDataset, StateVectorSegment, UsCountyBoundary,
        UsCountyBoundaryDataset,
    };
}
pub mod us_csv;
mod v2_dat;
mod v2_record;

#[cfg(test)]
mod tallyham_tests;

/// Advanced inspection APIs and diagnostic data structures.
pub mod diagnostics {
    pub use crate::db::{
        ArchiveYearStatistics, AssetCatalogDiagnostics, Diagnostics, HciEntryTrace, HciKeyTrace,
        HciPostingStartInvariant, HciPostingStartInvariantSample, HciPostingTrace, IdxTrace,
        InterestCodeExample, InterestStatistics, JurisdictionCounts, LookupTrace, ModernTagCount,
        ModernTagStatistics, ParsedSnapshotTrace, RawV2Record, RecordBoundarySource,
        RecordStatistics, Stats, UnknownInterestCode, DEFAULT_TRACE_PREVIEW_LIMIT,
    };
    pub use crate::hci::{
        ArchivePostingYearStatistics, CallsignPostingStatistics, DecodedHciRecord, RawHciRecord,
    };
    pub use crate::v2_dat::DecodedV2Candidate;
}

pub use callsign::Callsign;
pub use country::{CountryInfo, CountryInfoSource, CountryMatch};
pub use db::{
    AssetCatalog, AssetKind, AssetMetadata, AssetRef, BatchLookup, CallBook, CallBookBuilder,
    CallsignEntry, CountryCatalog, CountryCatalogStatistics, Diagnostics, Entries, EntryIter,
    HistorySummary, InterestSearch, InterestSearchMatch, MapLayerStatistics, MapLayers,
    ProfileAssets, StationMap, StationMapRenderOptions, StationProfile, TextAssetRef,
};
pub use error::{Error, Result};
pub use interest::{InterestDefinition, InterestTable, ResolvedInterest};
pub use modern::{CallSnapshot, Jurisdiction, LookupResult, LookupStatus, SnapshotSource};
pub use sidecar::{BoundaryBox, GeoPoint, LookupCountRecord, LookupCounts};
pub use us_csv::{UsCsvFile, UsCsvRecord};
