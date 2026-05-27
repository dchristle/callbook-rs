//! Reader for the shipped `usa.csv.zip` current-US catalog.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use rustc_hash::FxHashMap;

/// Current US callsign record from `usa.csv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsCsvRecord {
    /// Callsign.
    pub callsign: String,
    /// FCC license class.
    pub class: String,
    /// Licensee name.
    pub name: String,
    /// Mailing address.
    pub address: String,
    /// Mailing city.
    pub city: String,
    /// Mailing state.
    pub state: String,
    /// ZIP code.
    pub zip: String,
    /// County.
    pub county: String,
    /// License issue date in `YYYYMMDD` form.
    pub license_issue_date: String,
    /// FCC transaction type.
    pub fcc_transaction_type: String,
}

/// Opened `usa.csv.zip` catalog.
#[derive(Debug, Clone)]
pub struct UsCsvFile {
    path: PathBuf,
    records: FxHashMap<String, UsCsvRecord>,
    callsigns: Vec<String>,
}

impl UsCsvFile {
    /// Open the ZIP path for future scans.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let records = load_records(path)?;
        let mut callsigns = records.keys().cloned().collect::<Vec<_>>();
        callsigns.sort();
        Ok(Self {
            path: path.to_owned(),
            records,
            callsigns,
        })
    }

    /// Path to `usa.csv.zip`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow a current US callsign record.
    #[must_use]
    pub fn get(&self, callsign: &str) -> Option<&UsCsvRecord> {
        let target = normalized_lookup_key(callsign)?;
        self.records.get(&target)
    }

    /// Look up a current US callsign.
    pub fn lookup(&self, callsign: &str) -> Result<Option<UsCsvRecord>> {
        Ok(self.get(callsign).cloned())
    }

    /// Return whether the current-US catalog contains `callsign`.
    #[must_use]
    pub fn contains_callsign(&self, callsign: &str) -> bool {
        self.get(callsign).is_some()
    }

    /// Number of rows in the current-US catalog.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the current-US catalog has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Iterate normalized callsigns from the current-US catalog.
    pub fn callsigns(&self) -> impl Iterator<Item = &str> {
        self.callsigns.iter().map(String::as_str)
    }

    /// Iterate current-US records sorted by callsign.
    pub fn records(&self) -> impl Iterator<Item = &UsCsvRecord> {
        self.callsigns
            .iter()
            .filter_map(|callsign| self.records.get(callsign))
    }
}

fn normalized_lookup_key(callsign: &str) -> Option<String> {
    let target = callsign.trim().to_ascii_uppercase();
    (!target.is_empty() && !target.contains(':')).then_some(target)
}

fn load_records(path: &Path) -> Result<FxHashMap<String, UsCsvRecord>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| Error::Zip {
        path: path.to_owned(),
        source: Box::new(e),
    })?;
    let csv = zip.by_name("usa.csv").map_err(|e| Error::Zip {
        path: path.to_owned(),
        source: Box::new(e),
    })?;
    read_csv(csv, path)
}

fn read_csv<R: Read>(reader: R, path: &Path) -> Result<FxHashMap<String, UsCsvRecord>> {
    let mut csv = csv::Reader::from_reader(reader);
    let mut records = FxHashMap::default();
    for row in csv.byte_records() {
        let row = row.map_err(|e| Error::Csv {
            path: path.to_owned(),
            source: Box::new(e),
        })?;
        let callsign = field(&row, 0);
        if !callsign.is_empty() {
            records.insert(
                callsign.to_ascii_uppercase(),
                UsCsvRecord {
                    callsign,
                    class: field(&row, 1),
                    name: field(&row, 2),
                    address: field(&row, 3),
                    city: field(&row, 4),
                    state: field(&row, 5),
                    zip: field(&row, 6),
                    county: field(&row, 7),
                    license_issue_date: field(&row, 8),
                    fcc_transaction_type: field(&row, 9),
                },
            );
        }
    }
    Ok(records)
}

fn field(row: &csv::ByteRecord, index: usize) -> String {
    row.get(index)
        .map(trim_ascii)
        .map(String::from_utf8_lossy)
        .unwrap_or_default()
        .into_owned()
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn parses_and_matches_current_us_record() {
        let data = concat!(
            "\"Callsign\",\"Class\",\"Name\",\"Address\",\"City\",\"State\",\"ZIP\",\"County\",\"License Issue Date\",\"FCC Transaction Type\"\n",
            "\"K0ABC\",\"E\",\"Jane Q Example\",\"123 Test St\",\"Springfield\",\"IL\",\"62704\",\"Sangamon\",\"20170815\",\"LIAUA\"\n",
        );
        let records = read_csv(Cursor::new(data), Path::new("usa.csv")).unwrap();
        let found = records.get("K0ABC").cloned().unwrap();
        assert_eq!(found.callsign, "K0ABC");
        assert_eq!(found.class, "E");
        assert_eq!(found.zip, "62704");
    }

    #[test]
    fn contains_callsign_normalizes_input() {
        let data = concat!(
            "\"Callsign\",\"Class\",\"Name\",\"Address\",\"City\",\"State\",\"ZIP\",\"County\",\"License Issue Date\",\"FCC Transaction Type\"\n",
            "\"K0ABC\",\"E\",\"Jane Q Example\",\"123 Test St\",\"Springfield\",\"IL\",\"62704\",\"Sangamon\",\"20170815\",\"LIAUA\"\n",
        );
        let records = read_csv(Cursor::new(data), Path::new("usa.csv")).unwrap();
        let mut callsigns = records.keys().cloned().collect::<Vec<_>>();
        callsigns.sort();
        let file = UsCsvFile {
            path: PathBuf::from("usa.csv.zip"),
            records,
            callsigns,
        };

        assert!(file.contains_callsign(" k0abc "));
        assert_eq!(file.get(" k0abc ").unwrap().callsign, "K0ABC");
        assert_eq!(file.lookup(" k0abc ").unwrap().unwrap().callsign, "K0ABC");
        assert!(!file.contains_callsign("K0ABC:2020"));
        assert!(file.get("K0ABC:2020").is_none());
        assert!(!file.contains_callsign(""));
        assert!(file.get("").is_none());
    }
}
