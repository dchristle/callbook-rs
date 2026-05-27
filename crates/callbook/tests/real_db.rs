use std::path::{Path, PathBuf};

use callbook::country::CountryTable;
use callbook::diagnostics::RecordBoundarySource;
use callbook::sidecar::{BoundaryDataset, BoundaryKind};
use callbook::{
    BoundaryBox, CallBook, CallSnapshot, GeoPoint, LookupResult, SnapshotSource,
    StationMapRenderOptions,
};

const GOLDEN_CALLS: [&str; 3] = ["S51DX", "W1AW", "PA0RDT"];

#[derive(Debug, PartialEq, Eq)]
struct SnapshotDigest {
    vintage: Option<u16>,
    callsign: String,
    name: Option<String>,
    address: Option<String>,
    city: Option<String>,
    state: Option<String>,
    postal: Option<String>,
    country: Option<String>,
    class: Option<String>,
    email: Option<String>,
    grid: Option<String>,
    expires: Option<String>,
    last_changed: Option<String>,
}

fn real_db() -> CallBook {
    let path = real_db_path();
    CallBook::open(&path)
        .unwrap_or_else(|err| panic!("failed to open CALLBOOK_DB {:?}: {err}", path))
}

fn real_db_path() -> PathBuf {
    std::env::var_os("CALLBOOK_DB")
        .map(PathBuf::from)
        .expect("set CALLBOOK_DB to a licensed HamCall database path")
}

fn real_country_table(path: &Path) -> CountryTable {
    let candidates = [
        path.join("ham0").join("countrys"),
        path.join("countrys"),
        path.join("ham0").join("gcmcountrys"),
        path.join("gcmcountrys"),
    ];
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| CountryTable::open(path).expect("open real DB country table"))
        .expect("real DB country table not found")
}

fn assert_sane_point(point: GeoPoint) {
    assert!(point_is_sane(point), "out-of-range point: {point:?}");
}

fn assert_sane_bounds(bounds: BoundaryBox) {
    assert!(
        bounds.min_lon.is_finite()
            && bounds.max_lon.is_finite()
            && bounds.min_lat.is_finite()
            && bounds.max_lat.is_finite()
            && bounds.min_lon <= bounds.max_lon
            && bounds.min_lat <= bounds.max_lat
            && (-180.0..=180.0).contains(&bounds.min_lon)
            && (-180.0..=180.0).contains(&bounds.max_lon)
            && (-90.0..=90.0).contains(&bounds.min_lat)
            && (-90.0..=90.0).contains(&bounds.max_lat),
        "invalid geographic bounds: {bounds:?}"
    );
}

fn assert_email_like(value: Option<&String>) {
    let email = value.expect("expected email field");
    let Some((local, domain)) = email.split_once('@') else {
        panic!("email field is missing @: {email:?}");
    };
    assert!(!local.is_empty(), "email local-part is empty: {email:?}");
    assert!(domain.contains('.'), "email domain has no dot: {email:?}");
    assert!(
        email.is_ascii() && !email.chars().any(char::is_whitespace),
        "email field is not plain ASCII email text: {email:?}"
    );
}

fn assert_point_in_bounds(point: GeoPoint, bounds: BoundaryBox, epsilon: f64) {
    assert!(
        point_in_bounds(point, bounds, epsilon),
        "point {point:?} is outside bounds {bounds:?}"
    );
}

fn path_is_under_any_existing_root(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    roots.iter().any(|root| {
        root.canonicalize()
            .map(|root| path.starts_with(root))
            .unwrap_or(false)
    })
}

fn digest(snapshot: &CallSnapshot) -> SnapshotDigest {
    SnapshotDigest {
        vintage: snapshot.vintage,
        callsign: snapshot.callsign.clone(),
        name: snapshot.display_name(),
        address: snapshot.address.clone(),
        city: snapshot.city.clone(),
        state: snapshot.state_or_province.clone(),
        postal: snapshot.postal_code.clone(),
        country: snapshot.country.clone(),
        class: snapshot.license_class.clone(),
        email: snapshot.email.clone(),
        grid: snapshot.grid.clone(),
        expires: snapshot.expires.clone(),
        last_changed: snapshot.last_changed.clone(),
    }
}

fn all_digests(result: &LookupResult) -> Vec<SnapshotDigest> {
    let mut out = Vec::new();
    if let Some(current) = &result.current {
        out.push(digest(current));
    }
    out.extend(result.history.iter().map(digest));
    out
}

fn fingerprint(result: &LookupResult) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    push_lookup_result(&mut hash, result);
    hash
}

fn push_lookup_result(hash: &mut u64, result: &LookupResult) {
    push(hash, &result.query);
    push(hash, &format!("{:?}", result.status));
    if let Some(current) = &result.current {
        push_snapshot(hash, current);
    }
    for snapshot in &result.history {
        push_snapshot(hash, snapshot);
    }
}

fn push_snapshot(hash: &mut u64, snapshot: &CallSnapshot) {
    push(hash, &snapshot.callsign);
    push(hash, &format!("{:?}", snapshot.vintage));
    push(hash, &format!("{:?}", snapshot.sources));
    push(hash, &format!("{:?}", snapshot.jurisdiction));
    push(hash, &format!("{:?}", snapshot.license_class));
    push(hash, &format!("{:?}", snapshot.record_code));
    push(hash, &format!("{:?}", snapshot.first_name));
    push(hash, &format!("{:?}", snapshot.middle_name));
    push(hash, &format!("{:?}", snapshot.last_name));
    push(hash, &format!("{:?}", snapshot.suffix));
    push(hash, &format!("{:?}", snapshot.address));
    push(hash, &format!("{:?}", snapshot.city));
    push(hash, &format!("{:?}", snapshot.state_or_province));
    push(hash, &format!("{:?}", snapshot.postal_code));
    push(hash, &format!("{:?}", snapshot.county));
    push(hash, &format!("{:?}", snapshot.country));
    push(hash, &format!("{:?}", snapshot.birth_date));
    push(hash, &format!("{:?}", snapshot.first_issued));
    push(hash, &format!("{:?}", snapshot.expires));
    push(hash, &format!("{:?}", snapshot.last_changed));
    push(hash, &format!("{:?}", snapshot.gmt_offset));
    push(hash, &format!("{:?}", snapshot.latitude));
    push(hash, &format!("{:?}", snapshot.longitude));
    push(hash, &format!("{:?}", snapshot.grid));
    push(hash, &format!("{:?}", snapshot.area_code));
    push(hash, &format!("{:?}", snapshot.previous_call));
    push(hash, &format!("{:?}", snapshot.previous_class));
    push(hash, &format!("{:?}", snapshot.fcc_transaction_type));
    push(hash, &format!("{:?}", snapshot.email));
    push(hash, &format!("{:?}", snapshot.qsl));
    push(hash, &format!("{:?}", snapshot.url));
    push(hash, &format!("{:?}", snapshot.fax_number));
    push(hash, &format!("{:?}", snapshot.iota));
    push(hash, &format!("{:?}", snapshot.interest_codes_raw));
    push(hash, &format!("{:?}", snapshot.interest_codes));
    push(hash, &format!("{:?}", snapshot.interests));
    push(hash, &format!("{:?}", snapshot.license_id));
    push(hash, &format!("{:?}", snapshot.frn));
    push(hash, &format!("{:?}", snapshot.numeric_id));
    push(hash, &format!("{:?}", snapshot.raw_tags));
}

fn push(hash: &mut u64, value: &str) {
    for byte in value.bytes().chain([0xff]) {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn golden_lookup_fingerprints_from_real_db() {
    let db = real_db();

    let expected = [
        ("S51DX", 0x557b_d389_cb11_3383),
        ("W1AW", 0xf788_b981_c0ff_1710),
        ("PA0RDT", 0x46d5_e34b_9f67_899a),
    ];

    for (callsign, expected_hash) in expected {
        let result = db.lookup(callsign).unwrap().into_report();
        assert_eq!(
            fingerprint(&result),
            expected_hash,
            "golden lookup changed for {callsign}: {:#?}",
            all_digests(&result)
        );
    }
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn golden_lookup_key_fields_from_real_db() {
    let db = real_db();

    let s51dx = db.lookup("S51DX").unwrap().into_report();
    assert_eq!(
        s51dx.current.as_ref().unwrap().country.as_deref(),
        Some("SLOVENIA")
    );
    assert_email_like(s51dx.current.as_ref().unwrap().email.as_ref());
    assert_eq!(
        s51dx
            .history
            .iter()
            .map(|snapshot| snapshot.vintage)
            .collect::<Vec<_>>(),
        vec![Some(2000), Some(2005), Some(2010), Some(2015), Some(2020)]
    );

    let w1aw = db.lookup("W1AW").unwrap().into_report();
    assert!(w1aw.current.as_ref().unwrap().city.is_some());
    assert!(w1aw.current.as_ref().unwrap().postal_code.is_some());
    assert!(w1aw
        .history
        .iter()
        .any(|snapshot| snapshot.vintage == Some(1940)));
    assert!(w1aw
        .history
        .iter()
        .any(|snapshot| snapshot.vintage == Some(2020)));

    let pa0rdt = db.lookup("PA0RDT").unwrap().into_report();
    assert_eq!(
        pa0rdt.current.as_ref().unwrap().country.as_deref(),
        Some("NETHERLANDS")
    );
    assert_email_like(pa0rdt.current.as_ref().unwrap().email.as_ref());
    assert_eq!(
        pa0rdt
            .history
            .iter()
            .map(|snapshot| snapshot.vintage)
            .collect::<Vec<_>>(),
        vec![
            Some(1995),
            Some(2000),
            Some(2005),
            Some(2010),
            Some(2015),
            Some(2020)
        ]
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn trace_lookup_w1aw_from_real_db_reports_hci_bounds() {
    let db = real_db();

    let trace = db
        .diagnostics()
        .trace_lookup_with_limit("W1AW", 80)
        .unwrap();
    assert_eq!(trace.normalized_callsign, "W1AW");
    assert!(trace.hci_keys.iter().any(|key| {
        key.hci_entries.iter().any(|entry| {
            entry.postings.iter().any(|posting| {
                posting.record_start.is_some()
                    && posting.record_end.is_some()
                    && posting.record_boundary_source == Some(RecordBoundarySource::HciPostingStart)
                    && posting.parsed_snapshots.count > 0
            })
        })
    }));
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn callsign_hci_postings_are_dat_record_starts_from_real_db() {
    let db = real_db();
    let report = db
        .diagnostics()
        .callsign_hci_posting_start_invariant()
        .expect("real DB should include modern HCI sources");

    println!("callsign_postings_total={}", report.total_postings);
    println!(
        "posting_offsets_at_record_start={}",
        report.record_start_postings
    );
    println!(
        "posting_offsets_not_at_record_start={}",
        report.non_record_start_postings
    );
    println!(
        "posting_offsets_out_of_bounds={}",
        report.out_of_bounds_postings
    );
    println!("samples={:?}", report.samples);

    assert!(report.total_postings > 0);
    assert_eq!(report.record_start_postings, report.total_postings);
    assert_eq!(report.non_record_start_postings, 0);
    assert_eq!(report.out_of_bounds_postings, 0);
    assert!(report.samples.is_empty());
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn interest_catalog_parses_real_definitions() {
    let db = real_db();
    let stats = db.diagnostics().interest_statistics();

    assert!(
        stats.catalog_entries >= 100,
        "real interest catalog parsed too few entries: {}",
        stats.catalog_entries
    );
    let definition = db
        .interest_catalog()
        .and_then(|catalog| catalog.lookup("0010"))
        .expect("real interest catalog should define code 0010");
    assert!(!definition.category.is_empty());
    assert!(!definition.label.is_empty());
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates usa.csv.zip current-record workflow"]
fn us_csv_current_records_are_integrated_into_real_lookup_workflow() {
    let db = real_db();
    let stats = db.diagnostics().record_statistics();
    let callsign = db
        .diagnostics()
        .sample_us_current_callsign()
        .expect("real usa.csv.zip should expose at least one current callsign");
    let trace = db.diagnostics().trace_lookup(&callsign).unwrap();

    assert!(
        stats.us_csv_current_records >= 100_000,
        "real usa.csv.zip parsed too few current records: {}",
        stats.us_csv_current_records
    );
    assert!(
        trace.us_csv_hit,
        "{callsign} should exercise usa.csv.zip current-record lookup"
    );
    println!(
        "usa.csv.zip current_records={} sampled_callsign={}",
        stats.us_csv_current_records, callsign
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn no_neighbor_records_leak_from_real_db() {
    let db = real_db();

    for callsign in GOLDEN_CALLS {
        let result = db.lookup(callsign).unwrap().into_report();
        for snapshot in result.current.iter().chain(result.history.iter()) {
            assert_eq!(snapshot.callsign, callsign);
        }
    }

    let kd0b = db.lookup("K0AB").unwrap().into_report();
    let w1aw = db.lookup("W1AW").unwrap().into_report();
    assert_ne!(
        kd0b.current
            .as_ref()
            .map(|snapshot| snapshot.callsign.as_str()),
        w1aw.current
            .as_ref()
            .map(|snapshot| snapshot.callsign.as_str())
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn hci_postings_return_only_verified_callsigns_from_real_db() {
    let db = real_db();

    for callsign in GOLDEN_CALLS {
        let result = db.lookup(callsign).unwrap().into_report();
        let hci_snapshots: Vec<_> = result
            .current
            .iter()
            .chain(result.history.iter())
            .filter(|snapshot| snapshot.sources.contains(&SnapshotSource::HamCallHci))
            .collect();
        assert!(
            !hci_snapshots.is_empty(),
            "{callsign} should exercise HCI postings"
        );
        for snapshot in hci_snapshots {
            assert_eq!(
                snapshot.callsign, callsign,
                "HCI posting for {callsign} decoded a neighboring record"
            );
        }
    }
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates USCOUN.DAT parser semantics against real data"]
fn us_county_boundaries_parse_with_real_db_invariants() {
    let db = real_db();
    let dataset = db
        .us_county_boundaries()
        .expect("open real DB US county boundaries")
        .expect("real DB USCOUN.DAT");

    let county_count = dataset.counties.len();
    let point_count: usize = dataset
        .counties
        .iter()
        .map(|county| county.points.len())
        .sum();
    let bounds_count = dataset
        .counties
        .iter()
        .filter(|county| county.bounds.is_some())
        .count();
    let next_offset_count = dataset
        .counties
        .iter()
        .filter(|county| county.next_offset.is_some())
        .count();

    println!("USCOUN.DAT path={}", dataset.path.display());
    println!("county_records={county_count}");
    println!("total_points={point_count}");
    println!("records_with_bounds={bounds_count}");
    println!("records_with_next_offset={next_offset_count}");

    assert!(
        county_count >= 1_000,
        "USCOUN.DAT parsed too few county records: {county_count}"
    );
    assert!(
        point_count >= 10_000,
        "USCOUN.DAT parsed too few boundary points: {point_count}"
    );
    assert_eq!(
        bounds_count, county_count,
        "every real USCOUN.DAT record should have parseable bounds"
    );
    assert_eq!(
        next_offset_count, county_count,
        "every real USCOUN.DAT record should have a next-offset field"
    );

    let mut previous_next_offset = 0u64;
    for (index, county) in dataset.counties.iter().enumerate() {
        let bounds = county
            .bounds
            .unwrap_or_else(|| panic!("county record {index} has no parsed bounds"));
        let next_offset = county
            .next_offset
            .unwrap_or_else(|| panic!("county record {index} has no next offset"));
        assert!(
            next_offset > previous_next_offset,
            "county record {index} next_offset={next_offset} is not greater than previous {previous_next_offset}"
        );
        previous_next_offset = next_offset;

        assert!(
            bounds.min_lon <= bounds.max_lon && bounds.min_lat <= bounds.max_lat,
            "county record {index} has inverted bounds: {bounds:?}"
        );
        assert!(
            (-180.0..=180.0).contains(&bounds.min_lon)
                && (-180.0..=180.0).contains(&bounds.max_lon)
                && (-90.0..=90.0).contains(&bounds.min_lat)
                && (-90.0..=90.0).contains(&bounds.max_lat),
            "county record {index} has out-of-range bounds: {bounds:?}"
        );
        assert!(
            !county.points.is_empty(),
            "county record {index} has no boundary points"
        );
        for point in &county.points {
            assert!(
                point_is_sane(*point),
                "county record {index} has out-of-range point: {point:?}"
            );
            assert!(
                point_in_bounds(*point, bounds, 1.0e-6),
                "county record {index} point {point:?} is outside bounds {bounds:?}"
            );
        }
    }

    let generic = dataset.boundary_dataset();
    assert_eq!(generic.kind, BoundaryKind::UsCounty);
    assert_eq!(generic.segments.len(), county_count);
    assert_eq!(generic.point_count(), point_count);
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates wc.dat and cb.dat boundary sidecar invariants"]
fn world_and_county_boundaries_parse_with_real_db_invariants() {
    fn assert_dataset(
        dataset: &BoundaryDataset,
        min_segments: usize,
        min_points: usize,
        label: &str,
    ) {
        let segment_count = dataset.segments.len();
        let point_count: usize = dataset
            .segments
            .iter()
            .map(|segment| segment.points.len())
            .sum();

        println!(
            "{label} path={} segments={segment_count} points={point_count}",
            dataset.path.display()
        );

        assert!(
            segment_count >= min_segments,
            "{label} parsed too few segments: {segment_count}"
        );
        assert!(
            point_count >= min_points,
            "{label} parsed too few points: {point_count}"
        );

        for (index, segment) in dataset.segments.iter().enumerate() {
            assert_sane_bounds(segment.bounds);
            assert!(
                segment.next_offset > 0,
                "{label} segment {index} has non-positive next offset"
            );
            assert!(
                segment.points.len() >= 2,
                "{label} segment {index} has fewer than two points"
            );
            for point in &segment.points {
                assert_sane_point(*point);
                assert_point_in_bounds(*point, segment.bounds, 1.0e-6);
            }
        }

        assert_eq!(
            dataset.point_count(),
            point_count,
            "{label} point_count disagrees with manual sum"
        );
    }

    let db = real_db();
    let world = db
        .world_boundary_dataset()
        .expect("open real wc.dat")
        .expect("real DB wc.dat");
    let county = db
        .county_boundary_dataset()
        .expect("open real cb.dat")
        .expect("real DB cb.dat");

    assert_dataset(&world, 100, 1_000, "wc.dat");
    assert_dataset(&county, 1_000, 10_000, "cb.dat");
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates counts.dat lookup-count workflow invariants"]
fn lookup_counts_parse_with_real_db_invariants() {
    let db = real_db();
    let counts = db
        .lookup_counts()
        .expect("open real counts.dat")
        .expect("real DB counts.dat");
    let len = counts.len();

    assert!(len >= 1_000_000, "counts.dat record count too small: {len}");

    let sorted_m0ggo = counts
        .lookup("M0GGO")
        .expect("M0GGO should resolve from sorted counts.dat table");

    let mut sorted_m0ggo_count = 0usize;
    let mut previous_sorted_key: Option<String> = None;
    let mut sampled_records = 0usize;
    for (index, record) in counts.iter().enumerate() {
        if let Some(previous_key) = &previous_sorted_key {
            assert!(
                previous_key <= &record.key,
                "counts.dat sorted table is not nondecreasing at {index}: {previous_key:?} then {:?}",
                record.key
            );
            assert!(
                previous_key != &record.key || record.key.contains(':'),
                "counts.dat sorted table contains duplicate bare key {:?} at {index}",
                record.key
            );
        }
        if record.key == "M0GGO" {
            sorted_m0ggo_count += 1;
            assert_eq!(
                record, sorted_m0ggo,
                "counts.dat sorted M0GGO slot differs from lookup result"
            );
        }
        assert!(
            !record.key.is_empty(),
            "counts.dat record {index} has empty key"
        );
        assert!(
            record
                .key
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_lowercase()),
            "counts.dat record {index} has non-uppercase-ish key {:?}",
            record.key
        );
        if let Some(date) = record.updated_yyyymmdd {
            assert!(
                (19900101..=21000101).contains(&date),
                "counts.dat record {index} has implausible date {date}"
            );
        }
        if let Some(status) = record.status {
            assert!(
                status.is_ascii_graphic(),
                "counts.dat record {index} has non-graphic status {status:?}"
            );
        }

        if !record.key.contains(':') {
            assert_eq!(
                counts.lookup(&record.key),
                Some(record.clone()),
                "counts.dat lookup disagrees for sorted-table sampled key {:?}",
                record.key
            );
        }
        previous_sorted_key = Some(record.key);
        sampled_records += 1;
    }

    assert!(
        sampled_records >= 1_000,
        "counts.dat parsed too few searchable records: {sampled_records}"
    );
    assert_eq!(
        sorted_m0ggo_count, 1,
        "counts.dat sorted table should contain exactly one M0GGO record"
    );

    println!(
        "counts.dat path={} records={len} searchable_records={sampled_records} sorted_m0ggo={sorted_m0ggo:?}",
        counts.path().display(),
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates PHOTOS.TXT manifest invariants"]
fn photo_manifest_parse_with_real_db_invariants() {
    let db = real_db();
    let diagnostics = db
        .asset_catalog()
        .diagnostics()
        .expect("open real PHOTOS.TXT");

    assert!(
        diagnostics.photo_manifest_entries >= 1_000,
        "PHOTOS.TXT parsed too few entries: {}",
        diagnostics.photo_manifest_entries
    );
    assert!(
        diagnostics.photo_manifest_files_found > 0,
        "no PHOTOS.TXT file references resolved"
    );
    println!(
        "PHOTOS.TXT entries={} entries_with_files={} files_found={} files_missing={}",
        diagnostics.photo_manifest_entries,
        diagnostics.photo_manifest_entries_with_file,
        diagnostics.photo_manifest_files_found,
        diagnostics.photo_manifest_files_missing
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates state.dat vector workflow invariants"]
fn state_vector_dataset_parse_with_real_db_invariants() {
    let db = real_db();
    let vectors = db
        .state_vector_dataset()
        .expect("open real state.dat")
        .expect("real DB state.dat");

    assert!(
        vectors.segments.len() >= 100,
        "state.dat vector conversion yielded too few segments: {}",
        vectors.segments.len()
    );
    let mut lat_range = (f64::INFINITY, f64::NEG_INFINITY);
    let mut lon_range = (f64::INFINITY, f64::NEG_INFINITY);
    for point in vectors.segments.iter().flat_map(|segment| &segment.points) {
        lat_range.0 = lat_range.0.min(point.lat);
        lat_range.1 = lat_range.1.max(point.lat);
        lon_range.0 = lon_range.0.min(point.lon);
        lon_range.1 = lon_range.1.max(point.lon);
    }
    assert!(
        (25.0..=55.0).contains(&lat_range.0) && (25.0..=55.0).contains(&lat_range.1),
        "state.dat latitude range is not plausible for US state vectors: {lat_range:?}"
    );
    assert!(
        (-130.0..=-65.0).contains(&lon_range.0) && (-130.0..=-65.0).contains(&lon_range.1),
        "state.dat longitude range is not plausible for US state vectors: {lon_range:?}"
    );

    println!(
        "state.dat path={} vector_segments={} vector_points={} lat_range={lat_range:?} lon_range={lon_range:?}",
        vectors.path.display(),
        vectors.segments.len(),
        vectors.point_count()
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates rich country metadata against a real IDX sample"]
fn country_catalog_rich_metadata_matches_real_idx_sample() {
    let path = real_db_path();
    let db = real_db();
    let table = real_country_table(&path);
    let pc_table = db
        .pc_country_catalog()
        .expect("real DB COUNTRYS.PC catalog");
    let country_stats = db.country_catalog().statistics();

    let mut checked = 0usize;
    let mut rich = 0usize;
    let mut previous_leading = None;
    for (entry_index, entry) in db.entries().include_history(false).into_iter().enumerate() {
        let entry = entry.expect("real DB entry should be readable");
        let callsign = entry.callsign();
        let leading_changed = previous_leading != callsign.as_bytes().first().copied();
        previous_leading = callsign.as_bytes().first().copied();
        if checked < 512 || entry_index % 997 == 0 || leading_changed {
            let info = table.lookup_info(callsign);
            assert_eq!(
                table.lookup(callsign),
                info.as_ref().map(callbook::CountryInfo::to_match),
                "country lookup and lookup_info disagree for {callsign}"
            );

            if let Some(info) = info {
                rich += 1;
                assert!(
                    !info.name.is_empty(),
                    "country info for {callsign} has empty name"
                );
                if let Some(code) = &info.code {
                    assert!(
                        code.len() <= 4
                            && code
                                .bytes()
                                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
                        "country info for {callsign} has invalid code {code:?}"
                    );
                }
                if let Some(continent) = &info.continent {
                    assert!(
                        continent.split(',').all(|part| part.len() == 2
                            && part.bytes().all(|byte| byte.is_ascii_uppercase())),
                        "country info for {callsign} has invalid continent {continent:?}"
                    );
                }
                if let (Some(latitude), Some(longitude)) = (info.latitude, info.longitude) {
                    assert_sane_point(GeoPoint {
                        lon: longitude,
                        lat: latitude,
                    });
                }
                if let Some(zone) = info.itu_zone {
                    assert!(
                        (1..=90).contains(&zone),
                        "country info for {callsign} has invalid ITU zone {zone}"
                    );
                }
                if let Some(zone) = info.cq_zone {
                    assert!(
                        (1..=90).contains(&zone),
                        "country info for {callsign} has invalid CQ zone {zone}"
                    );
                }
            }
            checked += 1;
        }
    }

    assert!(
        checked >= 1_000,
        "real IDX country metadata sample was unexpectedly small: {checked}"
    );
    assert!(
        pc_table.len() >= 100,
        "COUNTRYS.PC parsed too few fallback rules: {}",
        pc_table.len()
    );
    assert!(
        country_stats.name_centroids >= 100,
        "countrys.nam parsed too few country centroids: {}",
        country_stats.name_centroids
    );
    println!(
        "country_catalog_checked={checked} rich_info={rich} pc_rules={} name_centroids={}",
        pc_table.len(),
        country_stats.name_centroids
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; validates station profile sidecar integration best-effort"]
fn station_profiles_use_real_sidecars_best_effort() {
    let root = real_db_path();
    let db = real_db();
    let photo_roots = [root.join("ham0/photos"), root.join("photos")];
    let bio_roots = [root.join("ham0/bios"), root.join("bios")];
    let flag_roots = [root.join("ham0/flags"), root.join("flags")];
    let map_roots = [root.join("ham0/maps"), root.join("maps")];
    let mut exercised_sidecar = false;

    for callsign in GOLDEN_CALLS {
        let entry = db.lookup(callsign).expect("lookup representative callsign");
        let profile = entry.profile().expect("build representative profile");
        let map = entry.map().expect("build representative map");
        let expected_country = db.country_info(callsign);
        let expected_lookup_count = db.lookup_count(callsign).expect("lookup count");

        assert_eq!(
            profile.country, expected_country,
            "profile country disagrees for {callsign}"
        );
        assert_eq!(
            profile.lookup_count, expected_lookup_count,
            "profile lookup count disagrees for {callsign}"
        );

        for photo in profile.assets.photos() {
            assert!(
                photo.path.is_file(),
                "profile photo for {callsign} does not exist: {}",
                photo.path.display()
            );
            assert!(
                path_is_under_any_existing_root(&photo.path, &photo_roots),
                "profile photo for {callsign} is outside photos roots: {}",
                photo.path.display()
            );
        }
        if let Some(bio) = profile.assets.bio() {
            assert!(
                bio.path.is_file(),
                "profile bio for {callsign} does not exist: {}",
                bio.path.display()
            );
            assert!(
                path_is_under_any_existing_root(&bio.path, &bio_roots),
                "profile bio for {callsign} is outside bios roots: {}",
                bio.path.display()
            );
        }
        if let Some(flag) = profile.assets.country_flag() {
            assert!(
                flag.path.is_file(),
                "profile flag for {callsign} does not exist: {}",
                flag.path.display()
            );
            assert!(
                path_is_under_any_existing_root(&flag.path, &flag_roots),
                "profile flag for {callsign} is outside flags roots: {}",
                flag.path.display()
            );
        }
        if let Some(country_map) = profile.assets.country_map() {
            assert!(
                country_map.path.is_file(),
                "profile country map for {callsign} does not exist: {}",
                country_map.path.display()
            );
            assert!(
                path_is_under_any_existing_root(&country_map.path, &map_roots),
                "profile country map for {callsign} is outside maps roots: {}",
                country_map.path.display()
            );
        }

        let has_map_data =
            map.station_location().is_some() || map.world_boundaries().unwrap().is_some();
        if has_map_data {
            map.render_svg_with_options(StationMapRenderOptions::preview())
                .expect("render representative preview map");
            let bounded_all_layers = StationMapRenderOptions {
                max_boundary_segments: Some(512),
                ..StationMapRenderOptions::all_layers()
            };
            map.render_svg_with_options(bounded_all_layers)
                .expect("render representative all-layer map");
        }
        assert!(
            map.us_county_boundaries()
                .expect("open real US county boundaries")
                .is_some(),
            "real station map should expose USCOUN.DAT"
        );
        assert!(
            map.state_vectors()
                .expect("open real state vectors")
                .is_some(),
            "real station map should expose state.dat vector paths"
        );

        let has_sidecar_asset = !profile.assets.photos().is_empty()
            || profile.assets.bio().is_some()
            || profile.assets.country_flag().is_some()
            || profile.assets.country_map().is_some();
        let has_lookup_count = profile.lookup_count.is_some();
        exercised_sidecar |= has_sidecar_asset || has_lookup_count;

        println!(
            "profile callsign={callsign} country={:?} lookup_count={} photos={} map_marker={}",
            profile
                .country
                .as_ref()
                .map(|country| country.name.as_str()),
            has_lookup_count,
            profile.assets.photos().len(),
            map.station_location().is_some()
        );
    }

    assert!(
        exercised_sidecar,
        "representative station profiles did not exercise any sidecar assets or lookup counts"
    );
}

fn point_is_sane(point: GeoPoint) -> bool {
    point.lon.is_finite()
        && point.lat.is_finite()
        && (-180.0..=180.0).contains(&point.lon)
        && (-90.0..=90.0).contains(&point.lat)
}

fn point_in_bounds(point: GeoPoint, bounds: callbook::BoundaryBox, epsilon: f64) -> bool {
    point.lon >= bounds.min_lon - epsilon
        && point.lon <= bounds.max_lon + epsilon
        && point.lat >= bounds.min_lat - epsilon
        && point.lat <= bounds.max_lat + epsilon
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB"]
fn indexed_country_lookup_matches_linear_lookup_for_real_idx_sample() {
    let db = real_db();
    let path = real_db_path();
    let countries = real_country_table(&path);

    let mut checked = 0usize;
    let mut previous_leading = None;
    for (entry_index, entry) in db.entries().include_history(false).into_iter().enumerate() {
        let entry = entry.expect("real DB entry should be readable");
        let callsign = entry.callsign();
        let leading_changed = previous_leading != callsign.as_bytes().first().copied();
        previous_leading = callsign.as_bytes().first().copied();
        if checked < 512 || entry_index % 997 == 0 || leading_changed {
            assert_eq!(
                countries.lookup(callsign),
                countries.lookup_linear(callsign),
                "indexed country lookup differs for real IDX callsign {callsign}"
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 1_000,
        "real IDX country equivalence sample was unexpectedly small: {checked}"
    );
}

#[test]
#[ignore = "requires licensed CALLBOOK_DB; exhaustive proof, run manually after country-index changes"]
fn indexed_country_lookup_matches_linear_lookup_for_all_real_idx_callsigns() {
    let db = real_db();
    let path = real_db_path();
    let countries = real_country_table(&path);

    let mut checked = 0usize;
    for entry in db.entries().include_history(false) {
        let entry = entry.expect("real DB entry should be readable");
        let callsign = entry.callsign();
        assert_eq!(
            countries.lookup(callsign),
            countries.lookup_linear(callsign),
            "indexed country lookup differs for real IDX callsign {callsign}"
        );
        checked += 1;
    }

    assert!(checked >= 1_000_000, "checked only {checked} IDX callsigns");
}
