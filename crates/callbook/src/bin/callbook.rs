//! `callbook` — command-line front-end for the [`callbook`] crate.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use callbook::diagnostics::{
    HciEntryTrace, HciKeyTrace, HciPostingStartInvariant, HciPostingTrace, InterestStatistics,
    JurisdictionCounts, LookupTrace, ModernTagStatistics, ParsedSnapshotTrace,
    RecordBoundarySource, RecordStatistics, DEFAULT_TRACE_PREVIEW_LIMIT,
};
use callbook::sidecar::BoundaryDataset;
use callbook::{AssetMetadata, CallBook, CallSnapshot, LookupStatus};
use clap::{Parser, Subcommand};

const MARKETED_ARCHIVE_YEARS: &[u16] = &[
    1921, 1940, 1948, 1954, 1957, 1960, 1965, 1969, 1972, 1977, 1983, 1990, 1995, 2000, 2005, 2010,
    2015,
];

#[derive(Parser, Debug)]
#[command(
    name = "callbook",
    version,
    about = "Look up callsigns in a Buckmaster HamCall database.",
    long_about = "callbook — query a locally installed Buckmaster HamCall\n\
                  database (DVD/USB/download).\n\
                  HamCall is sold separately at https://hamcall.net."
)]
struct Cli {
    /// Path to the HamCall data directory (or its parent).
    /// Falls back to $CALLBOOK_DB and platform defaults if omitted.
    #[arg(long, value_name = "PATH", global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Convenience form: `callbook W1AW`.
    callsign: Option<String>,

    /// Print raw record bytes instead of pretty output.
    #[arg(long, global = true)]
    raw: bool,

    /// Emit a JSON document instead of pretty output.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug, Clone)]
enum Cmd {
    /// Look up a single callsign.
    Lookup {
        /// Callsign, e.g. W1AW.
        callsign: String,
    },
    /// Print database statistics.
    Stats {
        /// Include interest-profile coverage statistics.
        #[arg(long)]
        interests: bool,
        /// Include DAT tag inventory statistics.
        #[arg(long)]
        tags: bool,
        /// Print sample records for this many unresolved interest codes.
        #[arg(long, default_value_t = 0)]
        unknown_samples: usize,
    },
    /// List sidecar assets associated with a callsign.
    Assets {
        /// Callsign, e.g. W1AW.
        callsign: String,
    },
    /// List discovered database-level sidecar files.
    Sidecars,
    /// Print decoded sidecar data summaries.
    SidecarStats,
    /// Look up HamCall.net web lookup-count metadata.
    Count {
        /// Callsign, e.g. W1AW.
        callsign: String,
    },
    /// Run on-disk format probes and print a verification report.
    Verify,
    /// Decode or dump one HCI ordinal for inspection.
    HciDump {
        /// Zero-based ordinal in hciindex.dat.
        index: usize,
        /// Print encoded bytes instead of decoded bytes.
        #[arg(long)]
        encoded: bool,
        /// Maximum bytes to print.
        #[arg(long, default_value_t = 160)]
        limit: usize,
    },
    /// Check whether callsign-looking HCI postings target DAT record starts.
    HciPostingInvariant,
    /// Trace one ham0 lookup through IDX, HCI, and DAT records.
    TraceLookup {
        /// Callsign, e.g. W1AW.
        callsign: String,
        /// Maximum decoded bytes to include in hex/text previews.
        #[arg(long, default_value_t = DEFAULT_TRACE_PREVIEW_LIMIT)]
        limit: usize,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let cmd = cli
        .cmd
        .clone()
        .or_else(|| cli.callsign.clone().map(|c| Cmd::Lookup { callsign: c }));

    let Some(cmd) = cmd else {
        eprintln!("usage: callbook <CALLSIGN>  (or `callbook help`)");
        return ExitCode::from(2);
    };

    let db_path = match resolve_db_path(cli.db.as_deref()) {
        Some(p) => p,
        None => {
            eprintln!(
                "callbook: could not locate the HamCall database. Pass --db <path> \
                 or set CALLBOOK_DB."
            );
            return ExitCode::from(2);
        }
    };

    let db = match CallBook::builder(&db_path).open() {
        Ok(db) => db,
        Err(e) => {
            eprintln!(
                "callbook: failed to open database at {}: {e}",
                db_path.display()
            );
            return ExitCode::from(1);
        }
    };

    match cmd {
        Cmd::Lookup { callsign } => lookup(&db, &callsign, cli.raw, cli.json),
        Cmd::Stats {
            interests,
            tags,
            unknown_samples,
        } => stats(&db, cli.json, interests, tags, unknown_samples),
        Cmd::Assets { callsign } => assets(&db, &callsign, cli.json),
        Cmd::Sidecars => sidecars(&db, cli.json),
        Cmd::SidecarStats => sidecar_stats(&db, cli.json),
        Cmd::Count { callsign } => count(&db, &callsign, cli.json),
        Cmd::Verify => {
            print!("{}", db.diagnostics().verify());
            ExitCode::SUCCESS
        }
        Cmd::HciDump {
            index,
            encoded,
            limit,
        } => hci_dump(&db, index, encoded, limit, cli.json),
        Cmd::HciPostingInvariant => hci_posting_invariant(&db, cli.json),
        Cmd::TraceLookup { callsign, limit } => trace_lookup(&db, &callsign, limit, cli.json),
    }
}

fn resolve_db_path(flag: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(p) = flag {
        return Some(p.to_owned());
    }
    if let Ok(p) = std::env::var("CALLBOOK_DB") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    for default in platform_defaults() {
        if default.is_dir() {
            return Some(default);
        }
    }
    let here = std::env::current_dir().ok()?;
    let local = here.join("HAMCALL");
    if local.is_dir() {
        return Some(local);
    }
    None
}

fn platform_defaults() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs_home() {
        if cfg!(target_os = "macos") {
            out.push(home.join("Library/Application Support/callbook"));
        }
        out.push(home.join(".local/share/callbook"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        out.push(PathBuf::from(appdata).join("callbook"));
    }
    out
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn lookup(db: &CallBook, callsign: &str, raw: bool, json: bool) -> ExitCode {
    if raw {
        if let Some(rec) = db.diagnostics().lookup_v2_raw(callsign) {
            let _ = std::io::stdout().write_all(rec.raw_bytes);
            let _ = writeln!(std::io::stdout());
            return ExitCode::SUCCESS;
        }
        eprintln!("not found: {callsign}");
        return ExitCode::from(1);
    }

    let result = match db.lookup(callsign) {
        Ok(entry) => entry.into_report(),
        Err(e) => {
            eprintln!("callbook: lookup failed: {e}");
            return ExitCode::from(1);
        }
    };

    if !matches!(result.status, LookupStatus::NotFound) {
        if raw {
            unreachable!("raw handled before structured lookup");
        }
        if json {
            let v = serde_json::json!({
                "query": result.query,
                "status": format!("{:?}", result.status),
                "current": result.current.as_ref().map(snapshot_json),
                "history": result.history.iter().map(snapshot_json).collect::<Vec<_>>(),
            });
            println!("{v:#}");
            return ExitCode::SUCCESS;
        }
        print_lookup_result(&result);
        return ExitCode::SUCCESS;
    }

    eprintln!("not found: {callsign}");
    ExitCode::from(1)
}

fn print_lookup_result(result: &callbook::LookupResult) {
    if let Some(current) = &result.current {
        println!("{:>14}  {}", "callsign:", current.callsign);
        if let Some(name) = current.display_name() {
            println!("{:>14}  {name}", "name:");
        }
        if let Some(class) = &current.license_class {
            println!("{:>14}  {class}", "class:");
        }
        if let Some(address) = &current.address {
            println!("{:>14}  {address}", "address:");
        }
        let city = current.city.as_deref().unwrap_or("");
        let state = current.state_or_province.as_deref().unwrap_or("");
        let zip = current.postal_code.as_deref().unwrap_or("");
        if !city.is_empty() || !state.is_empty() || !zip.is_empty() {
            println!("{:>14}  {} {} {}", "city/state:", city, state, zip);
        }
        if let Some(country) = &current.country {
            println!("{:>14}  {country}", "country:");
        }
        if let Some(grid) = &current.grid {
            println!("{:>14}  {grid}", "grid:");
        }
        if let Some(expires) = &current.expires {
            println!("{:>14}  {expires}", "expires:");
        }
        println!("{:>14}  {:?}", "jurisdiction:", current.jurisdiction);
    }
    if !result.history.is_empty() {
        println!("{:>14}  {}", "history:", result.history.len());
        for item in &result.history {
            let vintage = item
                .vintage
                .map_or_else(|| "current".to_owned(), |y| y.to_string());
            let city = item.city.as_deref().unwrap_or("");
            let state = item.state_or_province.as_deref().unwrap_or("");
            let address = item.address.as_deref().unwrap_or("");
            println!("  {vintage:>6}  {address} {city} {state}");
        }
    }
}

fn snapshot_json(snapshot: &CallSnapshot) -> serde_json::Value {
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
        "grid": snapshot.grid,
        "latitude": snapshot.latitude,
        "longitude": snapshot.longitude,
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
        "interests": snapshot.interests.iter().map(interest_json).collect::<Vec<_>>(),
        "license_id": snapshot.license_id,
        "frn": snapshot.frn,
        "numeric_id": snapshot.numeric_id,
        "raw_tags": snapshot.raw_tags.iter().map(|(k, v)| (format!("{k:02x}"), serde_json::Value::String(v.clone()))).collect::<serde_json::Map<_, _>>(),
    })
}

fn interest_json(interest: &callbook::ResolvedInterest) -> serde_json::Value {
    serde_json::json!({
        "code": interest.code,
        "category": interest.category,
        "label": interest.label,
    })
}

fn stats(
    db: &CallBook,
    json: bool,
    interests: bool,
    tags: bool,
    unknown_samples: usize,
) -> ExitCode {
    let diag = db.diagnostics();
    let s = diag.stats();
    let record_stats = diag.record_statistics();
    let interest_stats = interests.then(|| diag.interest_statistics());
    let tag_stats = tags.then(|| diag.modern_tag_statistics());
    if json {
        let v = serde_json::json!({
            "shard_count": s.shard_count,
            "total_records": s.total_records,
            "modern": {
                "idx_records": s.modern_idx_records,
                "hci_records": s.modern_hci_records,
                "has_us_csv": s.has_us_csv,
            },
            "records": record_statistics_json(&record_stats),
            "interest_profiles": interest_stats.as_ref().map(interest_statistics_json),
            "modern_tags": tag_stats.as_ref().map(tag_statistics_json),
        });
        println!("{v:#}");
        return ExitCode::SUCCESS;
    }
    println!(
        "shards: {}  total records: {}",
        s.shard_count, s.total_records
    );
    if s.modern_idx_records > 0 {
        println!(
            "  ham0 ({} IDX entries, {} HCI offsets, us_csv={})",
            s.modern_idx_records, s.modern_hci_records, s.has_us_csv
        );
        print_record_statistics(&record_stats);
        if let Some(interest_stats) = &interest_stats {
            print_interest_statistics(interest_stats, unknown_samples);
        }
        if let Some(tag_stats) = &tag_stats {
            print_tag_statistics(tag_stats);
        }
    }
    ExitCode::SUCCESS
}

fn print_record_statistics(stats: &RecordStatistics) {
    println!(
        "  current records: {} (US {}, Canada {}, other {}, unknown {})",
        stats.current.total(),
        stats.current.united_states,
        stats.current.canada,
        stats.current.international,
        stats.current.unknown,
    );
    println!(
        "  archive records: {} (US {}, Canada {}, other {}, unknown {})",
        stats.archive.total(),
        stats.archive.united_states,
        stats.archive.canada,
        stats.archive.international,
        stats.archive.unknown,
    );
    println!(
        "  total incl. archive: {}",
        stats.total_records_including_archive
    );
    println!(
        "  source counts: idx_current={} idx_archive={} us_csv_current={}",
        stats.modern_idx_current_records,
        stats.modern_idx_archive_records,
        stats.us_csv_current_records,
    );
    if !stats.archive_years.is_empty() {
        let years = stats
            .archive_years
            .iter()
            .map(|year| format!("{}:{}", year.year, year.counts.total()))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  archive years: {years}");
    }
    if let Some(hci) = &stats.hci_callsigns {
        println!(
            "  hci current estimate: {} unique DAT offsets from {} postings",
            hci.current_unique_dat_offsets, hci.current_postings
        );
        println!(
            "  hci archive estimate: {} unique DAT offsets from {} postings",
            hci.archive_unique_dat_offsets, hci.archive_postings
        );
        println!(
            "  hci marketed archive years: {} unique DAT offsets",
            marketed_archive_year_offsets(stats)
        );
    }
}

fn record_statistics_json(stats: &RecordStatistics) -> serde_json::Value {
    serde_json::json!({
        "current": jurisdiction_counts_json(&stats.current),
        "archive": jurisdiction_counts_json(&stats.archive),
        "total_records_including_archive": stats.total_records_including_archive,
        "modern_idx_current_records": stats.modern_idx_current_records,
        "modern_idx_archive_records": stats.modern_idx_archive_records,
        "us_csv_current_records": stats.us_csv_current_records,
        "archive_years": stats.archive_years.iter().map(|year| {
            serde_json::json!({
                "year": year.year,
                "counts": jurisdiction_counts_json(&year.counts),
            })
        }).collect::<Vec<_>>(),
        "hci_callsigns": stats.hci_callsigns.as_ref().map(|hci| {
            serde_json::json!({
                "current_postings": hci.current_postings,
                "current_unique_dat_offsets": hci.current_unique_dat_offsets,
                "archive_postings": hci.archive_postings,
                "archive_unique_dat_offsets": hci.archive_unique_dat_offsets,
                "archive_years": hci.archive_years.iter().map(|year| {
                    serde_json::json!({
                        "year": year.year,
                        "postings": year.postings,
                        "unique_dat_offsets": year.unique_dat_offsets,
                    })
                }).collect::<Vec<_>>(),
                "marketed_archive_year_unique_dat_offsets": marketed_archive_year_offsets(stats),
                "marketed_archive_years": MARKETED_ARCHIVE_YEARS,
            })
        }),
    })
}

fn print_interest_statistics(stats: &InterestStatistics, unknown_samples: usize) {
    println!("  interest catalog entries: {}", stats.catalog_entries);
    println!(
        "  current callsigns with interests: {}",
        stats.current_callsigns_with_interests
    );
    println!(
        "  current callsigns with resolved interests: {}",
        stats.current_callsigns_with_resolved_interests
    );
    println!(
        "  snapshots with interests: current={} archive={}",
        stats.current_snapshots_with_interests, stats.archive_snapshots_with_interests
    );
    if !stats.unknown_codes.is_empty() {
        println!("  unknown interest codes: {}", stats.unknown_codes.len());
        for code in stats.unknown_codes.iter().take(unknown_samples) {
            let examples = code
                .examples
                .iter()
                .map(|example| match example.vintage {
                    Some(year) => format!("{}:{year}", example.callsign),
                    None => example.callsign.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "    {}: {} occurrences (current {}, archive {}){}",
                code.code,
                code.occurrences,
                code.current_occurrences,
                code.archive_occurrences,
                if examples.is_empty() {
                    String::new()
                } else {
                    format!(" examples: {examples}")
                }
            );
        }
    }
}

fn interest_statistics_json(stats: &InterestStatistics) -> serde_json::Value {
    serde_json::json!({
        "catalog_entries": stats.catalog_entries,
        "current_callsigns_with_interests": stats.current_callsigns_with_interests,
        "current_snapshots_with_interests": stats.current_snapshots_with_interests,
        "archive_snapshots_with_interests": stats.archive_snapshots_with_interests,
        "current_callsigns_with_resolved_interests": stats.current_callsigns_with_resolved_interests,
        "unknown_codes": stats.unknown_codes.iter().map(|code| {
            serde_json::json!({
                "code": code.code,
                "occurrences": code.occurrences,
                "current_occurrences": code.current_occurrences,
                "archive_occurrences": code.archive_occurrences,
                "examples": code.examples.iter().map(|example| {
                    serde_json::json!({
                        "callsign": example.callsign,
                        "vintage": example.vintage,
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn print_tag_statistics(stats: &ModernTagStatistics) {
    println!("  DAT tags: {}", stats.tags.len());
    for tag in &stats.tags {
        println!(
            "    {:02x} {} total={} current={} archive={}",
            tag.tag,
            tag.field_name.unwrap_or("unmapped"),
            tag.occurrences,
            tag.current_occurrences,
            tag.archive_occurrences,
        );
    }
}

fn tag_statistics_json(stats: &ModernTagStatistics) -> serde_json::Value {
    serde_json::json!({
        "tags": stats.tags.iter().map(|tag| {
            serde_json::json!({
                "tag": format!("{:02x}", tag.tag),
                "field_name": tag.field_name,
                "occurrences": tag.occurrences,
                "current_occurrences": tag.current_occurrences,
                "archive_occurrences": tag.archive_occurrences,
                "sample_values": tag.sample_values,
            })
        }).collect::<Vec<_>>(),
    })
}

fn assets(db: &CallBook, callsign: &str, json: bool) -> ExitCode {
    let mut assets = db.callsign_assets(callsign);
    assets.extend(db.country_assets_for_callsign(callsign));
    if json {
        let value = serde_json::json!({
            "callsign": callsign,
            "assets": assets.iter().map(asset_json).collect::<Vec<_>>(),
        });
        println!("{value:#}");
        return ExitCode::SUCCESS;
    }
    if assets.is_empty() {
        println!("no assets found for {callsign}");
        return ExitCode::SUCCESS;
    }
    for asset in &assets {
        println!(
            "{:?}\t{}\t{}\t{}",
            asset.kind,
            asset.key,
            asset.media_type,
            asset.path.display()
        );
    }
    ExitCode::SUCCESS
}

fn sidecars(db: &CallBook, json: bool) -> ExitCode {
    let assets = db.sidecar_files();
    if json {
        let value = serde_json::json!({
            "sidecars": assets.iter().map(asset_json).collect::<Vec<_>>(),
        });
        println!("{value:#}");
        return ExitCode::SUCCESS;
    }
    for asset in &assets {
        println!(
            "{:?}\t{}\t{}",
            asset.kind,
            asset.media_type,
            asset.path.display()
        );
    }
    ExitCode::SUCCESS
}

fn sidecar_stats(db: &CallBook, json: bool) -> ExitCode {
    let world = match db.world_boundary_dataset() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("callbook: failed to read wc.dat: {e}");
            return ExitCode::from(1);
        }
    };
    let counties = match db.county_boundary_dataset() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("callbook: failed to read cb.dat: {e}");
            return ExitCode::from(1);
        }
    };
    let us_counties = match db.us_county_boundaries() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("callbook: failed to read USCOUN.DAT: {e}");
            return ExitCode::from(1);
        }
    };
    let state_vectors = match db.state_vector_dataset() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("callbook: failed to read state.dat: {e}");
            return ExitCode::from(1);
        }
    };
    let asset_diagnostics = match db.asset_catalog().diagnostics() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("callbook: failed to read PHOTOS.TXT: {e}");
            return ExitCode::from(1);
        }
    };
    let record_stats = db.diagnostics().record_statistics();
    let interest_stats = db.diagnostics().interest_statistics();
    let country_stats = db.country_catalog().statistics();

    if json {
        let value = serde_json::json!({
            "world_boundaries": world.as_ref().map(|dataset| boundary_summary_json(dataset.as_ref())),
            "county_boundaries": counties.as_ref().map(|dataset| boundary_summary_json(dataset.as_ref())),
            "us_county_boundaries": us_counties.as_ref().map(|dataset| serde_json::json!({
                "path": dataset.path.display().to_string(),
                "counties": dataset.counties.len(),
                "points": dataset.counties.iter().map(|county| county.points.len()).sum::<usize>(),
            })),
            "state_vectors": state_vectors.as_ref().map(|vectors| serde_json::json!({
                "path": vectors.path.display().to_string(),
                "segments": vectors.segments.len(),
                "points": vectors.point_count(),
            })),
            "photo_manifest": serde_json::json!({
                "entries": asset_diagnostics.photo_manifest_entries,
                "entries_with_files": asset_diagnostics.photo_manifest_entries_with_file,
                "files_found": asset_diagnostics.photo_manifest_files_found,
                "files_missing": asset_diagnostics.photo_manifest_files_missing,
            }),
            "country_catalogs": {
                "primary": country_stats.primary_rules,
                "pc": db.pc_country_catalog().map(|table| table.len()),
                "name_centroids": country_stats.name_centroids,
            },
            "interest_catalog_entries": interest_stats.catalog_entries,
            "us_csv_current_records": record_stats.us_csv_current_records,
        });
        println!("{value:#}");
        return ExitCode::SUCCESS;
    }

    if let Some(world) = &world {
        println!(
            "wc.dat: {} segments, {} points",
            world.segments.len(),
            world.point_count()
        );
    }
    if let Some(counties) = &counties {
        println!(
            "cb.dat: {} segments, {} points",
            counties.segments.len(),
            counties.point_count()
        );
    }
    if let Some(vectors) = &state_vectors {
        println!(
            "state.dat: {} vector segments, {} vector points",
            vectors.segments.len(),
            vectors.point_count()
        );
    }
    if let Some(us_counties) = &us_counties {
        let points = us_counties
            .counties
            .iter()
            .map(|county| county.points.len())
            .sum::<usize>();
        println!(
            "USCOUN.DAT: {} counties, {} points",
            us_counties.counties.len(),
            points
        );
    }
    println!(
        "PHOTOS.TXT: {} entries, {} explicit files, {} found, {} missing",
        asset_diagnostics.photo_manifest_entries,
        asset_diagnostics.photo_manifest_entries_with_file,
        asset_diagnostics.photo_manifest_files_found,
        asset_diagnostics.photo_manifest_files_missing
    );
    println!(
        "country catalogs: primary={}, pc={}, name_centroids={}",
        country_stats.primary_rules,
        db.pc_country_catalog().map_or(0, |table| table.len()),
        country_stats.name_centroids
    );
    println!("interest: {} entries", interest_stats.catalog_entries);
    println!(
        "usa.csv.zip: {} current records",
        record_stats.us_csv_current_records
    );
    ExitCode::SUCCESS
}

fn boundary_summary_json(file: &BoundaryDataset) -> serde_json::Value {
    serde_json::json!({
        "path": file.path.display().to_string(),
        "segments": file.segments.len(),
        "points": file.point_count(),
        "sample": file.segments.iter().take(3).map(|segment| {
            serde_json::json!({
                "bounds": {
                    "min_lon": segment.bounds.min_lon,
                    "max_lon": segment.bounds.max_lon,
                    "min_lat": segment.bounds.min_lat,
                    "max_lat": segment.bounds.max_lat,
                },
                "next_offset": segment.next_offset,
                "points": segment.points.len(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn count(db: &CallBook, callsign: &str, json: bool) -> ExitCode {
    let record = match db.lookup_count(callsign) {
        Ok(record) => record,
        Err(e) => {
            eprintln!("callbook: count lookup failed: {e}");
            return ExitCode::from(1);
        }
    };
    let Some(record) = record else {
        eprintln!("count not found: {callsign}");
        return ExitCode::from(1);
    };
    if json {
        let value = serde_json::json!({
            "key": record.key,
            "count": record.count,
            "updated_yyyymmdd": record.updated_yyyymmdd,
            "status": record.status.map(|status| status.to_string()),
        });
        println!("{value:#}");
        return ExitCode::SUCCESS;
    }
    println!("{}: {} lookups", record.key, record.count);
    if let Some(date) = record.updated_yyyymmdd {
        println!("updated: {date}");
    }
    if let Some(status) = record.status {
        println!("status: {status}");
    }
    ExitCode::SUCCESS
}

fn asset_json(asset: &AssetMetadata) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", asset.kind),
        "key": asset.key,
        "media_type": asset.media_type,
        "path": asset.path.display().to_string(),
    })
}

fn marketed_archive_year_offsets(stats: &RecordStatistics) -> usize {
    stats
        .hci_callsigns
        .as_ref()
        .map(|hci| {
            hci.archive_years
                .iter()
                .filter(|year| MARKETED_ARCHIVE_YEARS.contains(&year.year))
                .map(|year| year.unique_dat_offsets)
                .sum()
        })
        .unwrap_or(0)
}

fn jurisdiction_counts_json(counts: &JurisdictionCounts) -> serde_json::Value {
    serde_json::json!({
        "total": counts.total(),
        "united_states": counts.united_states,
        "canada": counts.canada,
        "international": counts.international,
        "unknown": counts.unknown,
    })
}

fn hci_posting_invariant(db: &CallBook, json: bool) -> ExitCode {
    let Some(report) = db.diagnostics().callsign_hci_posting_start_invariant() else {
        eprintln!("callbook: no ham0 HCI/DAT sources opened");
        return ExitCode::from(1);
    };
    if json {
        println!("{:#}", hci_posting_invariant_json(&report));
        return ExitCode::SUCCESS;
    }

    println!("callsign_postings_total: {}", report.total_postings);
    println!(
        "posting_offsets_at_record_start: {}",
        report.record_start_postings
    );
    println!(
        "posting_offsets_not_at_record_start: {}",
        report.non_record_start_postings
    );
    println!(
        "posting_offsets_out_of_bounds: {}",
        report.out_of_bounds_postings
    );
    if !report.samples.is_empty() {
        println!("samples:");
        for sample in &report.samples {
            println!(
                "  key={} dat_offset={} position={} decoded_byte={}",
                sample.hci_key,
                sample.dat_offset,
                sample.position,
                sample.decoded_byte.map_or_else(
                    || "out-of-bounds".to_owned(),
                    |byte| { format!("0x{byte:02x}") }
                )
            );
        }
    }
    ExitCode::SUCCESS
}

fn trace_lookup(db: &CallBook, callsign: &str, limit: usize, json: bool) -> ExitCode {
    let trace = match db.diagnostics().trace_lookup_with_limit(callsign, limit) {
        Ok(trace) => trace,
        Err(e) => {
            eprintln!("callbook: trace lookup failed: {e}");
            return ExitCode::from(1);
        }
    };
    if json {
        println!("{:#}", lookup_trace_json(&trace));
        return ExitCode::SUCCESS;
    }
    print_lookup_trace(&trace);
    ExitCode::SUCCESS
}

fn hci_posting_invariant_json(report: &HciPostingStartInvariant) -> serde_json::Value {
    serde_json::json!({
        "total_postings": report.total_postings,
        "record_start_postings": report.record_start_postings,
        "non_record_start_postings": report.non_record_start_postings,
        "out_of_bounds_postings": report.out_of_bounds_postings,
        "samples": report.samples.iter().map(|sample| {
            serde_json::json!({
                "hci_key": sample.hci_key,
                "dat_offset": sample.dat_offset,
                "position": sample.position,
                "decoded_byte": sample.decoded_byte,
            })
        }).collect::<Vec<_>>(),
    })
}

fn print_lookup_trace(trace: &LookupTrace) {
    println!("query: {}", trace.query);
    println!("normalized: {}", trace.normalized_callsign);
    println!("status: {:?}", trace.final_status);
    println!("us_csv_hit: {}", trace.us_csv_hit);
    println!("idx_hits: {}", trace.idx_hits.len());
    for hit in &trace.idx_hits {
        println!(
            "  {} dat_offset={} next={} raw_len={} parsed={}",
            hit.key,
            hit.dat_offset,
            hit.next_dat_offset
                .map_or_else(|| "-".to_owned(), |offset| offset.to_string()),
            hit.raw_len,
            hit.parsed_snapshots.count
        );
        println!("    decoded_hex: {}", hit.decoded_hex_prefix);
        println!("    decoded_text: {}", hit.decoded_text_prefix);
    }
    println!("hci_keys: {}", trace.hci_keys.len());
    for key in &trace.hci_keys {
        print_hci_key_trace(key);
    }
}

fn print_hci_key_trace(key: &HciKeyTrace) {
    println!(
        "  key {} entries={}",
        key.searched_key,
        key.hci_entries.len()
    );
    for entry in &key.hci_entries {
        print_hci_entry_trace(entry);
    }
}

fn print_hci_entry_trace(entry: &HciEntryTrace) {
    println!(
        "    ordinal={} hci_dat_offset={} raw_len={} header_len={} header={}",
        entry.ordinal, entry.hci_dat_offset, entry.raw_len, entry.header_len, entry.decoded_header
    );
    println!("      encoded_hex: {}", entry.encoded_hex_prefix);
    println!("      decoded_hex: {}", entry.decoded_hex_prefix);
    for posting in &entry.postings {
        print_hci_posting_trace(posting);
    }
}

fn print_hci_posting_trace(posting: &HciPostingTrace) {
    println!(
        "      posting dat_offset={} position={} source={} record_start={} record_end={} record_len={} distance_start={} distance_end={} match_start={} parsed={}",
        posting.dat_offset,
        posting.position,
        option_boundary_source(posting.record_boundary_source),
        option_u64(posting.record_start),
        option_u64(posting.record_end),
        option_usize(posting.record_len),
        option_usize(posting.distance_to_record_start),
        option_usize(posting.distance_to_record_end),
        posting
            .posting_matches_record_start
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        posting.parsed_snapshots.count
    );
    if posting.backward_scan_bytes.is_some() || posting.forward_scan_bytes.is_some() {
        println!(
            "        scan backward={} forward={}",
            option_usize(posting.backward_scan_bytes),
            option_usize(posting.forward_scan_bytes)
        );
    }
    println!(
        "        decoded_record_hex: {}",
        posting.decoded_record_hex_prefix
    );
    println!(
        "        decoded_record_text: {}",
        posting.decoded_record_text_prefix
    );
}

fn option_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn option_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn option_boundary_source(value: Option<RecordBoundarySource>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn lookup_trace_json(trace: &LookupTrace) -> serde_json::Value {
    serde_json::json!({
        "query": trace.query,
        "normalized_callsign": trace.normalized_callsign,
        "us_csv_hit": trace.us_csv_hit,
        "final_status": format!("{:?}", trace.final_status),
        "idx_hits": trace.idx_hits.iter().map(|hit| {
            serde_json::json!({
                "key": hit.key,
                "dat_offset": hit.dat_offset,
                "next_dat_offset": hit.next_dat_offset,
                "raw_len": hit.raw_len,
                "decoded_hex_prefix": hit.decoded_hex_prefix,
                "decoded_text_prefix": hit.decoded_text_prefix,
                "parsed_snapshots": parsed_trace_json(&hit.parsed_snapshots),
            })
        }).collect::<Vec<_>>(),
        "hci_keys": trace.hci_keys.iter().map(hci_key_trace_json).collect::<Vec<_>>(),
    })
}

fn hci_key_trace_json(key: &HciKeyTrace) -> serde_json::Value {
    serde_json::json!({
        "searched_key": key.searched_key,
        "hci_entries": key.hci_entries.iter().map(|entry| {
            serde_json::json!({
                "ordinal": entry.ordinal,
                "hci_dat_offset": entry.hci_dat_offset,
                "raw_len": entry.raw_len,
                "header_len": entry.header_len,
                "decoded_header": entry.decoded_header,
                "encoded_hex_prefix": entry.encoded_hex_prefix,
                "decoded_hex_prefix": entry.decoded_hex_prefix,
                "postings": entry.postings.iter().map(|posting| {
                    serde_json::json!({
                        "dat_offset": posting.dat_offset,
                        "position": posting.position,
                        "record_boundary_source": posting.record_boundary_source.map(|source| source.to_string()),
                        "record_start": posting.record_start,
                        "record_end": posting.record_end,
                        "record_len": posting.record_len,
                        "distance_to_record_start": posting.distance_to_record_start,
                        "distance_to_record_end": posting.distance_to_record_end,
                        "backward_scan_bytes": posting.backward_scan_bytes,
                        "forward_scan_bytes": posting.forward_scan_bytes,
                        "posting_matches_record_start": posting.posting_matches_record_start,
                        "decoded_record_hex_prefix": posting.decoded_record_hex_prefix,
                        "decoded_record_text_prefix": posting.decoded_record_text_prefix,
                        "parsed_snapshots": parsed_trace_json(&posting.parsed_snapshots),
                    })
                }).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn parsed_trace_json(trace: &ParsedSnapshotTrace) -> serde_json::Value {
    serde_json::json!({
        "count": trace.count,
        "callsigns": trace.callsigns,
        "vintages": trace.vintages,
    })
}

fn hci_dump(db: &CallBook, index: usize, encoded: bool, limit: usize, json: bool) -> ExitCode {
    let diag = db.diagnostics();
    let Some(raw) = diag.hci_raw_record(index) else {
        eprintln!("hci ordinal not found: {index}");
        return ExitCode::from(1);
    };
    let decoded = diag.hci_decoded_record(index);
    if json {
        let decoded_ref = decoded.as_ref();
        let bytes = if encoded {
            raw.raw_bytes
        } else {
            decoded_ref.map_or(&[][..], |d| d.bytes.as_slice())
        };
        let v = serde_json::json!({
            "index": index,
            "dat_offset": raw.dat_offset,
            "raw_len": raw.raw_len,
            "mode": if encoded { "encoded" } else { "decoded" },
            "terminated": decoded_ref.map(|d| d.terminated),
            "hex": hex_prefix(bytes, limit),
            "text": text_prefix(bytes, limit),
        });
        println!("{v:#}");
        return ExitCode::SUCCESS;
    }

    println!("index: {index}");
    println!("dat_offset: {}", raw.dat_offset);
    println!("raw_len: {}", raw.raw_len);
    if encoded {
        println!("encoded_hex: {}", hex_prefix(raw.raw_bytes, limit));
        println!("encoded_text: {}", text_prefix(raw.raw_bytes, limit));
    } else if let Some(decoded) = decoded {
        println!("terminated: {}", decoded.terminated);
        println!("decoded_len: {}", decoded.bytes.len());
        println!("decoded_hex: {}", hex_prefix(&decoded.bytes, limit));
        println!("decoded_text: {}", text_prefix(&decoded.bytes, limit));
    }
    ExitCode::SUCCESS
}

fn hex_prefix(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn text_prefix(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .take(limit)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}
