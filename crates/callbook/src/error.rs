//! Error type for the crate.

use std::path::PathBuf;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors returned by the crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The supplied data path does not exist or is not a directory.
    #[error("data path not found or not a directory: {0}")]
    DataPathNotFound(PathBuf),

    /// No supported database files were discovered under the supplied data path.
    #[error("no HamCall data files found under {0}")]
    NoDataFiles(PathBuf),

    /// A required companion file (`.idx`/`.ast`) was not found.
    #[error("missing companion file `{0}` for {1}")]
    MissingCompanion(&'static str, PathBuf),

    /// The file's on-disk header is shorter or malformed.
    #[error("malformed header in {path}: {reason}")]
    MalformedHeader {
        /// File whose header is malformed.
        path: PathBuf,
        /// Human-readable reason.
        reason: &'static str,
    },

    /// The supplied callsign was empty or contained invalid characters.
    #[error("invalid callsign: {0}")]
    InvalidCallsign(String),

    /// The supplied interest-profile code was not four decimal digits.
    #[error("invalid interest code: {0}")]
    InvalidInterestCode(String),

    /// I/O error from the underlying filesystem.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// ZIP container error.
    #[error("zip error in {path}")]
    Zip {
        /// ZIP file path.
        path: PathBuf,
        /// Underlying ZIP error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// CSV parse error.
    #[error("csv error in {path}")]
    Csv {
        /// CSV file path.
        path: PathBuf,
        /// Underlying CSV error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The on-disk file size is inconsistent with the declared record count.
    #[error("file size mismatch in {path}: declared {declared} records, file size {size} bytes, entry size {entry} bytes")]
    SizeMismatch {
        /// File whose size doesn't line up.
        path: PathBuf,
        /// Number of records declared in the header.
        declared: usize,
        /// File size in bytes.
        size: usize,
        /// Inferred per-entry byte size.
        entry: usize,
    },
}
