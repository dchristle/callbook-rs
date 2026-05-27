# callbook-rs

Native Linux/macOS/Windows reader for the Buckmaster HamCall callsign
database. Pure Rust and memory-mapped.

> **You must purchase a licensed copy of the database from
> [hamcall.net](https://hamcall.net) to use this crate.** This project does
> not ship, mirror, or redistribute any Buckmaster data. It is an independent
> compatibility reader for locally installed database files.

## Description

Buckmaster ships HamCall as Windows software plus local database files. This
project reads the verified `ham0` data files directly, so amateur radio logging
and other programs can query a local installation programmatically and natively.

## What It Reads

HamCall's installed `ham0` database is a collection of related files, not one
flat callsign table. `callbook-rs` combines those sources into a few higher-level
views:

- **Callsign lookups:** current and historical station snapshots from
  `hamcall.dat`, `hamcall.idx`, `hci.dat`, `hciindex.dat`, and `usa.csv.zip`.
- **Current US records:** FCC-derived current US rows from `usa.csv.zip`.
- **Station profiles:** lookup result plus country metadata, history summary,
  lookup counts, biographies, photos, flags, and country maps when present.
- **Country metadata:** prefix rules, fallback country rules, cleaned labels,
  jurisdiction buckets, and country centroids from the country sidecars.
- **Interest profiles:** four-digit interest codes resolved through the
  `interest` catalog, with search across current and archive records.
- **Lookup counts:** HamCall.net lookup-count records from `counts.dat`.
- **Assets:** callsign biographies/photos and country flag/map files from the
  asset directories and photo manifest.
- **Map data:** station coordinates plus world, county, US county, and state
  vector sidecars for map rendering and diagnostics.
- **Diagnostics:** format verification, lookup tracing, source statistics, and
  low-level inspection APIs for the undocumented `ham0` layout.

The current-US/international distinction mostly reflects how HamCall stores its
data. The public API tries to present callsign, profile, country, asset, and map
workflows rather than requiring callers to know every source file.

## Interfaces

| Interface | Output | Use case |
|---|---|---|
| Rust crate | `callbook` library from the `callbook-rs` package | Native Rust applications and batch tooling |
| CLI | `callbook` / `callbook.exe` | Shell lookups, JSON output, stats, and format verification |
| C ABI | `libcallbook.{so,dylib}`, `callbook.dll`, `callbook.h` | Integration from C, Perl, other FFI hosts, and custom bindings |
| Python package | `callbook-rs` distribution, `callbook_rs` import package | Python scripts/apps; wheels bundle the native library |

## Install

### From source

```sh
git clone https://github.com/dchristle/callbook-rs
cd callbook-rs
cargo build --release --features cli --bin callbook
```

The CLI lands at `target/release/callbook` and the shared library at
`target/release/libcallbook.{so|dylib}` (or `callbook.dll` on Windows).

## Use

### From the shell

```sh
$ CALLBOOK_DB=/path/to/hamcall-db target/release/callbook W1AW
     callsign:  W1AW
         name:  ...
      history:  ...

$ export CALLBOOK_DB=/path/to/hamcall-db
$ callbook W1AW
     callsign:  W1AW
         name:  ...
        class:  C
   city/state:  ...
      country:  United States
         grid:  FN31
 jurisdiction:  UnitedStates
      history:  ...

$ callbook W1AW --json
{
  "status": "Current",
  "current": { "callsign": "W1AW", ... },
  "history": [
    { "callsign": "W1AW", "vintage": 1940, ... },
    { "callsign": "W1AW", "vintage": 2020, ... }
  ],
  ...
}

$ callbook stats
shards: 1  total records: <N>
  ham0 (<N> IDX entries, <N> HCI offsets, us_csv=true)
$ callbook verify          # print on-disk format probe report
```

The DB path is resolved in this order:

1. `--db <path>` flag
2. `CALLBOOK_DB` environment variable
3. Platform default
   (`~/Library/Application Support/callbook` on macOS,
   `~/.local/share/callbook` on Linux,
   `%APPDATA%\callbook` on Windows)
4. `./HAMCALL/` in the current directory

### From Rust

```toml
[dependencies]
callbook = { package = "callbook-rs", version = "0.1" }
```

For unreleased changes from GitHub:

```toml
[dependencies]
callbook = { package = "callbook-rs", git = "https://github.com/dchristle/callbook-rs" }
```

#### Lookup

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
let entry = db.lookup("W1AW")?;

if let Some(current) = entry.current() {
    println!("{}: {:?}", current.callsign, current.country);
}
for snapshot in entry.history() {
    println!("{:?}: {:?}", snapshot.vintage, snapshot.address);
}
```

`CallBook::lookup` returns a `CallsignEntry` handle. The entry owns the lookup
snapshots and can fetch related sidecar data without repeating the callsign.

#### Repeated Lookups

For one-off lookups, use `CallBook::lookup`. For large local scans or validators,
`batch_lookup` reuses temporary lookup buffers across calls.

```rust
use callbook::{CallBook, LookupStatus};

let db = CallBook::open("/path/to/hamcall-db")?;
let mut lookup = db.batch_lookup();

for candidate in ["W1AW", "NOTACALL"] {
    let entry = lookup.lookup(candidate)?;
    if entry.status() == LookupStatus::NotFound {
        println!("{candidate}: not found");
        continue;
    }

    let country = db.country_info(candidate).map(|country| country.cleaned_name);
    let location = entry.map()?.station_location();

    println!("{candidate}: country={country:?} location={location:?}");
}
```

The intended workflow is to keep `CallBook` handle open and reuse it across
many lookups.

#### Station Profiles

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
let profile = db.profile_for_callsign("W1AW")?;

println!("status: {:?}", profile.status);
println!("country: {:?}", profile.country.as_ref().map(|c| &c.cleaned_name));
println!("history snapshots: {}", profile.history.snapshot_count);

if let Some(count) = &profile.lookup_count {
    println!("hamcall.net lookups: {}", count.count);
}

if let Some(photo) = profile.assets.primary_photo() {
    println!("primary photo: {}", photo.path.display());
}
if let Some(bio) = profile.assets.bio_text()? {
    println!("bio chars: {}", bio.len());
}
```

#### Maps

```rust
use callbook::{CallBook, StationMapRenderOptions};

let db = CallBook::open("/path/to/hamcall-db")?;
if let Some(map) = db.map_for_callsign("W1AW")? {
    if let Some(point) = map.station_location() {
        println!("station: {}, {}", point.lat, point.lon);
    }

    let layer_stats = map.layers().statistics()?;
    println!("world boundary points: {}", layer_stats.world_points);

    let svg = map.render_svg_with_options(StationMapRenderOptions::preview())?;
    if let Some(svg) = svg {
        println!("preview SVG bytes: {}", svg.len());
    }
}
```

`StationMap` is the station-oriented API. Use `CallBook::map_layers()` when you
want the underlying map sidecars directly for bulk rendering or diagnostics.

#### Country Catalog

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
let countries = db.country_catalog();

let info = countries.lookup_info("S51DX").unwrap();
println!("raw label: {}", info.raw_name);
println!("group label: {}", info.cleaned_name);
println!("continent: {:?}", info.continent);

let stats = countries.statistics();
println!(
    "country rules: primary={} fallback={} centroids={}",
    stats.primary_rules, stats.fallback_rules, stats.name_centroids
);
```

`CountryCatalog` wraps the primary prefix table, `COUNTRYS.PC` fallback rules,
and `countrys.nam` centroid enrichment. `CountryInfo::raw_name` preserves the
source label; `cleaned_name` is intended for grouping/reporting.

#### Current US Catalog Iteration

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
if let Some(us) = db.current_us_catalog() {
    for record in us.records().take(10) {
        println!("{} {} {}", record.callsign, record.state, record.zip);
    }
}
```

The current-US catalog is loaded from `usa.csv.zip` and iterates in normalized
callsign order, which is useful for local reporting, diagnostics, integration
tests, and local application development.

#### Interest Search

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
let interest = db.search_interest("0010")?;

if let Some(definition) = &interest.definition {
    println!("{}: {}", definition.code, definition.label);
}
println!("current matches: {}", interest.current().count());
println!("archive matches: {}", interest.archive().count());
```

Use `db.interest_catalog()` to list all interest definitions without scanning
the DAT file.

#### Lookup Counts

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
if let Some(counts) = db.lookup_counts()? {
    if let Some(count) = counts.lookup("W1AW") {
        println!("{} lookups as of {:?}", count.count, count.updated_yyyymmdd);
    }

    for record in counts.iter().take(10) {
        println!("{} {}", record.key, record.count);
    }
}
```

`LookupCounts` is the bulk catalog for `counts.dat`; `CallsignEntry` and
`StationProfile` use it for per-callsign lookup-count enrichment.

#### Assets

```rust
use callbook::CallBook;

let db = CallBook::open("/path/to/hamcall-db")?;
let assets = db.asset_catalog();

let profile_assets = assets.profile_assets("W1AW")?;
println!("photos: {}", profile_assets.photos().len());

let diagnostics = assets.diagnostics()?;
println!(
    "photo manifest: {} entries, {} missing files",
    diagnostics.photo_manifest_entries,
    diagnostics.photo_manifest_files_missing
);
```

The builder is available when you want to construct the database handle in a
chain:

```rust
use callbook::CallBook;

let db = CallBook::builder("/path/to/hamcall-db").open()?;
```

Advanced format inspection lives under diagnostics:

```rust
let trace = db.diagnostics().trace_lookup("W1AW")?;
let invariant = db.diagnostics().callsign_hci_posting_start_invariant();
```

### From Python

Python wheels bundle the native shared library for common platforms:

```sh
pip install callbook-rs
```

```python
from callbook_rs import CallBook

with CallBook.open("/path/to/hamcall-db") as db:
    with db.lookup("W1AW") as result:
        print(result.status)
        if result.current is not None:
            print(result.current.fields["grid"])

    with db.profile("W1AW") as profile:
        if profile.current is not None:
            print(profile.current.fields["city"])
        if profile.country is not None:
            print(profile.country.cleaned_name)

    us_records = db.current_us_records()
    first_ten = [record.fields["callsign"] for _, record in zip(range(10), us_records)]
```

Set `CALLBOOK_RS_LIB=/path/to/libcallbook.so` (or `.dylib` / `.dll`) to test a
locally built native library instead of the bundled wheel copy.

### From C / Perl / other FFI hosts

Link against `libcallbook.{so|dylib}` (or `callbook.dll`) and include
`callbook.h`:

```c
#include "callbook.h"
callbook_db *db = NULL;
callbook_open("/path/to/hamcall-db", &db);  // Expensive: open once, reuse.

callbook_lookup_result *result = NULL;
callbook_lookup_modern(db, "W1AW", &result);
const callbook_snapshot *current = callbook_result_current(result);
if (current) {
    puts(callbook_snapshot_field(current, callbook_modern_field_Name));
    puts(callbook_snapshot_field(current, callbook_modern_field_Grid));
}
callbook_result_free(result);
callbook_close(db);
```

For scripting bindings, use `callbook_lookup_json_required_len` plus
`callbook_lookup_json` to get the same structured result as JSON.

A worked smoke-test C program lives at `crates/callbook/tests/c_abi.c`.

## Supported Files

The lookup path supports the main local database and sidecar files
included in a licensed `ham0` installation:

- `<DB>/ham0/usa.csv.zip` contains current US FCC-derived rows.
- `<DB>/ham0/hamcall.idx` is a sorted ASCII index into `hamcall.dat`.
- `<DB>/ham0/hciindex.dat` and `hci.dat` form an inverted index. HCI keys
  route suffix/archive lookups to `hamcall.dat` postings, including calls
  absent from `hamcall.idx`.
- `<DB>/ham0/interest` maps interest-profile codes to labels.
- `<DB>/ham0/counts.dat` provides HamCall.net lookup counts for station profiles.
- `<DB>/ham0/countrys`, `gcmcountrys`, `COUNTRYS.PC`, and `countrys.nam`
  provide country-prefix metadata and fallback country centroids.
- `<DB>/ham0/photos/PHOTOS.TXT` and the `bios/`, `photos/`, `flags/`, and
  `maps/` trees provide station and country assets for profiles.
- `<DB>/ham0/wc.dat`, `cb.dat`, and `USCOUN.DAT` provide map boundary layers
  for station map workflows. `state.dat` provides official state-map vector
  paths without named polygon topology.
- Monthly update overlay files are not supported yet.

Lookups use:

- Memory-mapped I/O (no per-query syscalls)
- An immutable in-memory HCI key index for routing HCI postings
- Owned lookup results for ergonomic concurrent use
- Lock-free concurrent reads after open (`CallBook: Sync`)

## Status

Pre-1.0. The 2025 `ham0` path is the primary API and covers current US,
international, archived snapshot, profile, country, interest, lookup-count,
asset, and map workflows against a local licensed database installation.
Older pre-`ham0` database layouts and monthly update overlays are not supported yet.

## License

Dual-licensed under the MIT or Apache-2.0 license, at your option.

## Acknowledgements

This crate is *unofficial*. It is not affiliated with or endorsed by
Buckmaster International, LLC. The trademark "HamCall" belongs to its
respective owner.

If you find this useful, please buy the database.
