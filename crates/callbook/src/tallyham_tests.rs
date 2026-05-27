use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::hci::HciFile;
use crate::idx_text::TextIdxFile;
use crate::us_csv::UsCsvFile;
use crate::{v2_dat, v2_record, CallBook, CountryCatalog, SnapshotSource};
use memmap2::Mmap;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TallyRow {
    last_callsign: String,
    count: usize,
    country: String,
}

#[derive(Debug)]
struct TallyFile {
    by_country: Vec<TallyRow>,
    by_prefix: Vec<TallyRow>,
}

#[derive(Debug, Default)]
struct TallyDiff {
    total: usize,
    samples: Vec<String>,
}

#[derive(Debug, Clone)]
struct ClassifiedRecord {
    callsign: String,
    country: String,
}

#[derive(Debug)]
struct ComputedTally {
    label: &'static str,
    prefix_rows: Vec<TallyRow>,
    country_rows: Vec<TallyRow>,
    total_records: usize,
    unique_callsigns: usize,
    skipped_records: usize,
    callsigns: BTreeSet<String>,
}

#[derive(Debug)]
struct HciSourceStats {
    current_offsets: usize,
    parsed_snapshots: usize,
    multi_snapshot_records: usize,
    parse_failures: usize,
    country_misses: usize,
}

fn real_db_path() -> PathBuf {
    std::env::var_os("CALLBOOK_DB")
        .map(PathBuf::from)
        .expect("CALLBOOK_DB must point at a licensed HamCall database root")
}

fn real_text_idx(root: &Path) -> TextIdxFile {
    let candidates = [
        root.join("ham0").join("hamcall.idx"),
        root.join("hamcall.idx"),
    ];
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| TextIdxFile::open(path).expect("open real DB text IDX"))
        .expect("real DB text IDX not found")
}

fn real_hci(root: &Path) -> HciFile {
    let dir = root.join("ham0");
    HciFile::open(dir.join("hciindex.dat"), dir.join("hci.dat")).expect("open real DB HCI")
}

fn real_dat_mmap(root: &Path) -> Mmap {
    let path = root.join("ham0").join("hamcall.dat");
    let file = File::open(path).expect("open real DB DAT");
    // SAFETY: read-only view of a stable on-disk database.
    unsafe { Mmap::map(&file).expect("mmap real DB DAT") }
}

fn real_us_csv(root: &Path) -> Option<UsCsvFile> {
    let path = root.join("ham0").join("usa.csv.zip");
    path.is_file()
        .then(|| UsCsvFile::open(path).expect("open real usa.csv.zip"))
}

fn tallyham_path(root: &Path) -> PathBuf {
    let candidates = [
        root.join("tallyham.txt"),
        root.join("hamcall").join("tallyham.txt"),
        root.parent()
            .unwrap_or(root)
            .join("hamcall")
            .join("tallyham.txt"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .expect("tallyham.txt not found under CALLBOOK_DB or adjacent hamcall directory")
}

fn parse_tallyham(path: &Path) -> TallyFile {
    enum Section {
        None,
        Country,
        Prefix,
    }

    let text = std::fs::read_to_string(path).expect("read tallyham.txt");
    let mut section = Section::None;
    let mut by_country = Vec::new();
    let mut by_prefix = Vec::new();

    for line in text.lines() {
        if line.contains("Sorted by country name") {
            section = Section::Country;
            continue;
        }
        if line.contains("Sorted by callsign prefix") {
            section = Section::Prefix;
            continue;
        }
        let Some(row) = parse_tally_row(line) else {
            continue;
        };
        match section {
            Section::None => {}
            Section::Country => by_country.push(row),
            Section::Prefix => by_prefix.push(row),
        }
    }

    TallyFile {
        by_country,
        by_prefix,
    }
}

fn parse_tally_row(line: &str) -> Option<TallyRow> {
    let mut fields = line.split_whitespace();
    let last_callsign = fields.next()?;
    let count = fields.next()?.parse().ok()?;
    let country = fields.collect::<Vec<_>>().join(" ");
    if country.is_empty() || last_callsign == "Last" || last_callsign.starts_with('-') {
        return None;
    }
    Some(TallyRow {
        last_callsign: last_callsign.to_owned(),
        count,
        country,
    })
}

fn compute_idx_tally(idx: &TextIdxFile, countries: &CountryCatalog<'_>) -> ComputedTally {
    let mut records = Vec::new();
    let mut skipped_records = 0usize;
    for entry in idx.iter() {
        let call = key_callsign_part(entry.key);
        if call == b"!!!" || call == b"ZZZZZZZZ" {
            continue;
        }
        let callsign = std::str::from_utf8(call).expect("IDX callsign is ASCII");
        let Some(country) = countries.grouping_label(callsign) else {
            skipped_records += 1;
            continue;
        };
        records.push(ClassifiedRecord {
            callsign: callsign.to_owned(),
            country,
        });
    }
    computed_tally("idx", records, skipped_records)
}

fn compute_hci_current_tally(
    hci: &HciFile,
    dat: &[u8],
    countries: &CountryCatalog<'_>,
) -> (ComputedTally, HciSourceStats) {
    let mut offsets = BTreeSet::new();
    hci.visit_callsign_postings(|key, posting| {
        if !key.contains(&b':') {
            offsets.insert(posting.dat_offset);
        }
    });

    let mut records = Vec::new();
    let mut stats = HciSourceStats {
        current_offsets: offsets.len(),
        parsed_snapshots: 0,
        multi_snapshot_records: 0,
        parse_failures: 0,
        country_misses: 0,
    };
    let mut decoded = Vec::new();
    for offset in offsets {
        let Some(record) = decode_dat_record_at(dat, offset, &mut decoded) else {
            stats.parse_failures += 1;
            continue;
        };
        let snapshots =
            v2_record::parse_snapshots(record, None, SnapshotSource::HamCallHci, None, None);
        if snapshots.is_empty() {
            stats.parse_failures += 1;
            continue;
        }
        if snapshots.len() > 1 {
            stats.multi_snapshot_records += 1;
        }
        for snapshot in snapshots {
            if snapshot.vintage.is_some() {
                continue;
            }
            stats.parsed_snapshots += 1;
            let Some(country) = countries.grouping_label(&snapshot.callsign) else {
                stats.country_misses += 1;
                continue;
            };
            records.push(ClassifiedRecord {
                callsign: snapshot.callsign,
                country,
            });
        }
    }

    (
        computed_tally(
            "hci_current",
            records,
            stats.parse_failures + stats.country_misses,
        ),
        stats,
    )
}

fn compute_us_csv_tally(us_csv: &UsCsvFile) -> ComputedTally {
    let records = us_csv
        .callsigns()
        .map(|callsign| ClassifiedRecord {
            callsign: callsign.to_owned(),
            country: "UNITED STATES OF AMERICA".to_owned(),
        })
        .collect();
    computed_tally("us_csv", records, 0)
}

fn computed_tally(
    label: &'static str,
    mut records: Vec<ClassifiedRecord>,
    skipped_records: usize,
) -> ComputedTally {
    records.sort_by(|left, right| left.callsign.cmp(&right.callsign));
    let unique_callsigns = records
        .iter()
        .map(|record| record.callsign.clone())
        .collect::<BTreeSet<_>>();
    let total_records = records.len();
    let prefix_rows = prefix_tally_from_records(&records);
    let country_rows = country_tally_from_prefix_rows(&prefix_rows);
    ComputedTally {
        label,
        prefix_rows,
        country_rows,
        total_records,
        unique_callsigns: unique_callsigns.len(),
        skipped_records,
        callsigns: unique_callsigns,
    }
}

fn prefix_tally_from_records(records: &[ClassifiedRecord]) -> Vec<TallyRow> {
    let mut rows = Vec::new();
    let mut current: Option<TallyRow> = None;
    for record in records {
        match &mut current {
            Some(row) if row.country == record.country => {
                row.last_callsign = record.callsign.clone();
                row.count += 1;
            }
            _ => {
                if let Some(row) = current.take() {
                    rows.push(row);
                }
                current = Some(TallyRow {
                    last_callsign: record.callsign.clone(),
                    count: 1,
                    country: record.country.clone(),
                });
            }
        }
    }

    if let Some(row) = current {
        rows.push(row);
    }
    rows
}

fn decode_dat_record_at<'a>(dat: &[u8], dat_offset: u64, out: &'a mut Vec<u8>) -> Option<&'a [u8]> {
    let start = usize::try_from(dat_offset).ok()?;
    if start >= dat.len() || decode_dat_byte(dat_offset, dat[start]) != 0xb5 {
        return None;
    }
    let mut end = start + 1;
    while end < dat.len() && decode_dat_byte(end as u64, dat[end]) != 0xb5 {
        end += 1;
    }
    let phase = v2_dat::phase_for_dat_offset(dat_offset);
    v2_dat::decode_phase_into(dat_offset, &dat[start..end], phase, out);
    Some(out)
}

fn decode_dat_byte(offset: u64, encoded: u8) -> u8 {
    let stream_key = ((offset + 4) % 101) as u8;
    (encoded ^ 7) ^ stream_key
}

fn key_callsign_part(key: &[u8]) -> &[u8] {
    key.split(|byte| *byte == b':').next().unwrap_or(key)
}

fn country_tally_from_prefix_rows(prefix_rows: &[TallyRow]) -> Vec<TallyRow> {
    let mut by_country = BTreeMap::<String, TallyRow>::new();
    for row in prefix_rows {
        by_country
            .entry(row.country.clone())
            .and_modify(|country_row| {
                country_row.last_callsign = row.last_callsign.clone();
                country_row.count += row.count;
            })
            .or_insert_with(|| row.clone());
    }
    by_country.into_values().collect()
}

fn diff_rows(expected: &[TallyRow], actual: &[TallyRow], sample_limit: usize) -> TallyDiff {
    let mut diff = TallyDiff::default();
    let max_len = expected.len().max(actual.len());
    for index in 0..max_len {
        match (expected.get(index), actual.get(index)) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some(expected), Some(actual)) => {
                diff.total += 1;
                if diff.samples.len() < sample_limit {
                    diff.samples
                        .push(format!("#{index}: tally={expected:?} computed={actual:?}"));
                }
            }
            (Some(expected), None) => {
                diff.total += 1;
                if diff.samples.len() < sample_limit {
                    diff.samples
                        .push(format!("#{index}: tally={expected:?} computed=<missing>"));
                }
            }
            (None, Some(actual)) => {
                diff.total += 1;
                if diff.samples.len() < sample_limit {
                    diff.samples
                        .push(format!("#{index}: tally=<missing> computed={actual:?}"));
                }
            }
            (None, None) => {}
        }
    }
    diff
}

fn print_source_summary(tally: &TallyFile, source: &ComputedTally) {
    let country_diff = diff_rows(&tally.by_country, &source.country_rows, 5);
    let prefix_diff = diff_rows(&tally.by_prefix, &source.prefix_rows, 5);
    println!(
        "{}: country_rows={} prefix_rows={} records={} unique_callsigns={} skipped={} country_row_diffs={} prefix_row_diffs={}",
        source.label,
        source.country_rows.len(),
        source.prefix_rows.len(),
        source.total_records,
        source.unique_callsigns,
        source.skipped_records,
        country_diff.total,
        prefix_diff.total
    );
    print_largest_country_gaps(source.label, &tally.by_country, &source.country_rows, 12);
}

fn print_largest_country_gaps(
    label: &str,
    expected: &[TallyRow],
    actual: &[TallyRow],
    limit: usize,
) {
    let expected = country_counts(expected);
    let actual = country_counts(actual);
    let countries = expected
        .keys()
        .chain(actual.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut gaps = countries
        .into_iter()
        .filter_map(|country| {
            let tally = *expected.get(&country).unwrap_or(&0);
            let computed = *actual.get(&country).unwrap_or(&0);
            (tally != computed).then_some((country, tally, computed))
        })
        .collect::<Vec<_>>();
    gaps.sort_by(|left, right| {
        let left_abs = left.1.abs_diff(left.2);
        let right_abs = right.1.abs_diff(right.2);
        right_abs.cmp(&left_abs).then_with(|| left.0.cmp(&right.0))
    });
    println!("{label}: largest country count gaps:");
    for (country, tally, computed) in gaps.into_iter().take(limit) {
        let delta = computed as isize - tally as isize;
        println!("  {country}: tally={tally} computed={computed} delta={delta:+}");
    }
}

fn country_counts(rows: &[TallyRow]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row.country.clone()).or_default() += row.count;
    }
    counts
}

fn row_count(rows: &[TallyRow]) -> usize {
    rows.iter().map(|row| row.count).sum()
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn tallyham_matches_or_reports_bounded_differences() {
    let root = real_db_path();
    let tally_path = tallyham_path(&root);
    let tally = parse_tallyham(&tally_path);
    let db = CallBook::open(&root).expect("open real HamCall DB");
    let idx = real_text_idx(&root);
    let countries = db.country_catalog();
    let hci = real_hci(&root);
    let dat = real_dat_mmap(&root);
    let idx_tally = compute_idx_tally(&idx, &countries);
    let (hci_tally, hci_stats) = compute_hci_current_tally(&hci, &dat, &countries);
    let us_csv_tally = real_us_csv(&root).map(|us_csv| compute_us_csv_tally(&us_csv));

    println!("tallyham path: {}", tally_path.display());
    println!(
        "tallyham: country_rows={} prefix_rows={} total_count={}",
        tally.by_country.len(),
        tally.by_prefix.len(),
        row_count(&tally.by_country)
    );
    print_source_summary(&tally, &idx_tally);
    println!(
        "hci_current decode: current_offsets={} parsed_snapshots={} multi_snapshot_records={} parse_failures={} country_misses={}",
        hci_stats.current_offsets,
        hci_stats.parsed_snapshots,
        hci_stats.multi_snapshot_records,
        hci_stats.parse_failures,
        hci_stats.country_misses
    );
    print_source_summary(&tally, &hci_tally);
    if let Some(us_csv_tally) = &us_csv_tally {
        let us_csv_not_in_hci = us_csv_tally
            .callsigns
            .difference(&hci_tally.callsigns)
            .count();
        println!(
            "us_csv overlap: records={} callsigns_not_in_hci_current={}",
            us_csv_tally.total_records, us_csv_not_in_hci
        );
        print_source_summary(&tally, us_csv_tally);
    }

    assert!(
        tally.by_country.len() > 200,
        "parsed too few country rows from tallyham.txt: {}",
        tally.by_country.len()
    );
    assert!(
        tally.by_prefix.len() > tally.by_country.len(),
        "prefix section should have more rows than country section"
    );
    assert!(
        idx_tally.country_rows.len() > 200,
        "IDX computed too few countries: {}",
        idx_tally.country_rows.len()
    );
    assert!(
        hci_tally.country_rows.len() > 200,
        "HCI computed too few countries: {}",
        hci_tally.country_rows.len()
    );
    assert!(
        hci_tally.total_records > idx_tally.total_records,
        "HCI current source should cover more records than IDX"
    );

    if std::env::var_os("HAMCALL_TALLYHAM_REQUIRE_EXACT").is_some() {
        assert_eq!(
            tally.by_country, hci_tally.country_rows,
            "country tally mismatch"
        );
        assert_eq!(
            tally.by_prefix, hci_tally.prefix_rows,
            "prefix tally mismatch"
        );
    }
}
