//! On-disk format constants.
//!
//! Constants for the supported `ham0` database layout.

/// Length, in bytes, of a callsign key as stored in the IDX file.
pub(crate) const KEY_LEN: usize = 26;

/// Constants for the 2025 DVD layout.
///
/// These constants describe the `ham0` layout, which is independent
/// of the 2013 DLL layout.
pub(crate) mod v2 {
    /// Subdirectory under the user-supplied data path that holds the
    /// monolithic 2025 database.
    pub(crate) const DATA_DIR: &str = "ham0";

    /// Single canonical DAT filename in the 2025 layout. There are **no**
    /// `20*.DAT` shards — the entire database is one file.
    pub(crate) const DAT_NAME: &str = "hamcall.dat";

    /// Single canonical IDX filename in the 2025 layout.
    pub(crate) const IDX_NAME: &str = "hamcall.idx";

    /// Compact HCI record stream.
    pub(crate) const HCI_DAT_NAME: &str = "hci.dat";

    /// Big-endian u32 offset table for [`HCI_DAT_NAME`].
    pub(crate) const HCI_INDEX_NAME: &str = "hciindex.dat";

    /// Current FCC-derived US callsign catalog.
    pub(crate) const US_CSV_ZIP_NAME: &str = "usa.csv.zip";

    /// IDX field separator within a key: `<callsign>:<year>`. Optional —
    /// many entries are bare callsigns with no year suffix (e.g. `W1AWK`).
    pub(crate) const IDX_KEY_YEAR_SEP: u8 = b':';
}
