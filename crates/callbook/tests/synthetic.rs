//! End-to-end tests using a synthetic on-disk database.

use std::io::Write;
use std::sync::Arc;

use callbook::{CallBook, Error};

#[test]
fn stats_counts_modern_idx_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK1ABC 6 \r\nZZZZZZZZ 11 \r\n")
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let stats = db.diagnostics().stats();

    assert_eq!(stats.shard_count, 1);
    assert_eq!(stats.total_records, 3);
    assert_eq!(stats.modern_idx_records, 3);
    assert_eq!(stats.modern_hci_records, 0);
    assert!(!stats.has_us_csv);
}

#[test]
fn record_statistics_counts_current_archive_and_jurisdictions() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(
            b"!!! 0 \r\n\
              DL1ABC 1 \r\n\
              DL1ABC:2015 2 \r\n\
              K1ABC 3 \r\n\
              K1ABC:1921 4 \r\n\
              Q1ABC 5 \r\n\
              Q1ABC:1940 6 \r\n\
              VE3XYZ 7 \r\n\
              VE3XYZ:1948 8 \r\n\
              ZZZZZZZZ 11 \r\n",
        )
        .unwrap();
    std::fs::File::create(ham0_dir.join("countrys"))
        .unwrap()
        .write_all(
            b"DL    DL    GERMANY [DL] !28@14#EU\n\
              K     N     UNITED STATES [US] !08@05#NA\n\
              VE    VEZZZZCANADA [CA] !09@04#NA\n",
        )
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let stats = db.diagnostics().record_statistics();

    assert_eq!(stats.current.total(), 4);
    assert_eq!(stats.current.united_states, 1);
    assert_eq!(stats.current.canada, 1);
    assert_eq!(stats.current.international, 1);
    assert_eq!(stats.current.unknown, 1);
    assert_eq!(stats.archive.total(), 4);
    assert_eq!(stats.archive.united_states, 1);
    assert_eq!(stats.archive.canada, 1);
    assert_eq!(stats.archive.international, 1);
    assert_eq!(stats.archive.unknown, 1);
    assert_eq!(stats.modern_idx_current_records, 4);
    assert_eq!(stats.modern_idx_archive_records, 4);
    assert_eq!(stats.us_csv_current_records, 0);
    assert_eq!(stats.total_records_including_archive, 8);
    assert_eq!(
        stats
            .archive_years
            .iter()
            .map(|year| (year.year, year.counts.total()))
            .collect::<Vec<_>>(),
        vec![(1921, 1), (1940, 1), (1948, 1), (2015, 1)]
    );
}

#[test]
fn interest_statistics_counts_current_and_archive_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    let current_plain = b"\xb5K1ABC\xd5001000209999";
    let archive_plain = b"\xb5K1ABC:2015\xd50010";
    let current_offset = 1u64;
    let current_encoded = encode_dat_record(current_offset, current_plain);
    let archive_offset = current_offset + current_encoded.len() as u64;
    let archive_encoded = encode_dat_record(archive_offset, archive_plain);
    let end_offset = archive_offset + archive_encoded.len() as u64;

    let mut dat = vec![0u8];
    dat.extend_from_slice(&current_encoded);
    dat.extend_from_slice(&archive_encoded);
    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(&dat)
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(
            format!(
                "!!! 0 \r\n\
                 K1ABC {current_offset} \r\n\
                 K1ABC:2015 {archive_offset} \r\n\
                 ZZZZZZZZ {} \r\n",
                end_offset + 5
            )
            .as_bytes(),
        )
        .unwrap();
    std::fs::File::create(ham0_dir.join("interest"))
        .unwrap()
        .write_all(
            b"---Bands\n\
              0010*160 meters\n\
              0020*80 meters\n",
        )
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let stats = db.diagnostics().interest_statistics();

    assert_eq!(stats.catalog_entries, 2);
    assert_eq!(stats.current_callsigns_with_interests, 1);
    assert_eq!(stats.current_callsigns_with_resolved_interests, 1);
    assert_eq!(stats.current_snapshots_with_interests, 1);
    assert_eq!(stats.archive_snapshots_with_interests, 1);
    assert_eq!(
        stats
            .unknown_codes
            .iter()
            .map(|code| (code.code.as_str(), code.occurrences))
            .collect::<Vec<_>>(),
        vec![("9999", 1)]
    );
    assert_eq!(stats.unknown_codes[0].current_occurrences, 1);
    assert_eq!(stats.unknown_codes[0].archive_occurrences, 0);
    assert_eq!(stats.unknown_codes[0].examples[0].callsign, "K1ABC");

    let tags = db.diagnostics().modern_tag_statistics();
    let interest_tag = tags.tags.iter().find(|tag| tag.tag == 0xd5).unwrap();
    assert_eq!(interest_tag.field_name, Some("interest_codes"));
    assert_eq!(interest_tag.current_occurrences, 1);
    assert_eq!(interest_tag.archive_occurrences, 1);

    let catalog = db.interest_catalog().unwrap();
    assert_eq!(
        catalog
            .definitions()
            .map(|definition| (definition.code.as_str(), definition.label.as_str()))
            .collect::<Vec<_>>(),
        vec![("0010", "160 meters"), ("0020", "80 meters")]
    );

    let matches = db.search_interest("0010").unwrap();
    assert_eq!(matches.definition.as_ref().unwrap().label, "160 meters");
    assert_eq!(matches.len(), 2);
    assert_eq!(
        matches
            .current()
            .map(|entry| entry.callsign.as_str())
            .collect::<Vec<_>>(),
        vec!["K1ABC"]
    );
    assert_eq!(
        matches
            .archive()
            .map(|entry| (entry.callsign.as_str(), entry.vintage))
            .collect::<Vec<_>>(),
        vec![("K1ABC", Some(2015))]
    );

    let unknown = db.search_interest("9999").unwrap();
    assert!(unknown.definition.is_none());
    assert_eq!(unknown.len(), 1);
    assert!(matches!(
        db.search_interest("10"),
        Err(Error::InvalidInterestCode(_))
    ));
}

#[test]
fn sidecar_assets_are_returned_as_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("bios/K")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("photos/X")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("flags")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("maps")).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n")
        .unwrap();
    std::fs::File::create(ham0_dir.join("countrys"))
        .unwrap()
        .write_all(b"K     N     UNITED STATES [US] !08@05#NA\n")
        .unwrap();
    std::fs::File::create(ham0_dir.join("bios/K/K0AB.txt"))
        .unwrap()
        .write_all(b"bio")
        .unwrap();
    std::fs::File::create(ham0_dir.join("photos/X/K0ABX.JPG"))
        .unwrap()
        .write_all(b"jpg")
        .unwrap();
    std::fs::File::create(ham0_dir.join("flags/US.GIF"))
        .unwrap()
        .write_all(b"gif")
        .unwrap();
    std::fs::File::create(ham0_dir.join("maps/US.gif"))
        .unwrap()
        .write_all(b"gif")
        .unwrap();
    std::fs::File::create(ham0_dir.join("counts.dat"))
        .unwrap()
        .write_all(b"counts")
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let callsign_assets = db.callsign_assets("K0AB");
    let country_assets = db.country_assets_for_callsign("K0AB");
    let asset_catalog = db.asset_catalog();
    let entry = db.lookup("K0AB").unwrap();
    let entry_assets = entry.assets();
    let sidecars = db.sidecar_files();
    let diagnostics = asset_catalog.diagnostics().unwrap();

    assert_eq!(
        callsign_assets
            .iter()
            .map(|asset| format!("{:?}:{}", asset.kind, asset.media_type))
            .collect::<Vec<_>>(),
        vec!["Biography:text/plain", "Photo:image/jpeg"]
    );
    assert_eq!(
        country_assets
            .iter()
            .map(|asset| format!("{:?}:{}", asset.kind, asset.media_type))
            .collect::<Vec<_>>(),
        vec!["Flag:image/gif", "Map:image/gif"]
    );
    assert_eq!(entry.callsign(), "K0AB");
    assert_eq!(entry.country().unwrap().name, "UNITED STATES");
    assert_eq!(
        entry_assets.len(),
        callsign_assets.len() + country_assets.len()
    );
    assert!(sidecars
        .iter()
        .any(|asset| asset.path.ends_with("counts.dat")));
    assert_eq!(asset_catalog.callsign_assets("K0AB"), callsign_assets);
    assert_eq!(
        asset_catalog.country_assets_for_callsign("K0AB"),
        country_assets
    );
    assert!(diagnostics.bios_dir_present);
    assert!(diagnostics.photos_dir_present);
    assert!(diagnostics.flags_dir_present);
    assert!(diagnostics.maps_dir_present);
    assert!(diagnostics.sidecar_files >= 1);
}

#[test]
fn current_us_catalog_iterates_records_for_local_workflows() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK0ABC 6 \r\nW1AW 11 \r\nZZZZZZZZ 16 \r\n")
        .unwrap();
    write_usa_csv_zip(
        &ham0_dir.join("usa.csv.zip"),
        concat!(
            "\"Callsign\",\"Class\",\"Name\",\"Address\",\"City\",\"State\",\"ZIP\",\"County\",\"License Issue Date\",\"FCC Transaction Type\"\n",
            "\"W1AW\",\"C\",\"Example Club\",\"1 Test Way\",\"Example City\",\"CT\",\"00000\",\"Example\",\"20200101\",\"LIAUA\"\n",
            "\"K0ABC\",\"E\",\"Jane Example\",\"123 Test St\",\"Springfield\",\"IL\",\"62704\",\"Sangamon\",\"20170815\",\"LIAUA\"\n",
        ),
    );

    let db = CallBook::open(dir.path()).unwrap();
    let catalog = db.current_us_catalog().unwrap();

    assert_eq!(catalog.len(), 2);
    assert_eq!(
        catalog.callsigns().collect::<Vec<_>>(),
        vec!["K0ABC", "W1AW"]
    );
    assert_eq!(
        catalog
            .records()
            .map(|record| (record.callsign.as_str(), record.state.as_str()))
            .collect::<Vec<_>>(),
        vec![("K0ABC", "IL"), ("W1AW", "CT")]
    );
}

#[test]
fn lookup_counts_accessor_caches_catalog_and_lookup_count_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK0AB 6 \r\nW1AW 11 \r\nZZZZZZZZ 16 \r\n")
        .unwrap();
    write_encoded_count_slots(
        &ham0_dir.join("counts.dat"),
        &[
            *b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            *b"K0AB           1      20240101 \r\n",
            *b"!!!!!!!!!!!!!!!1      99999999N\r\n",
            *b"K0AB           7      20260509N\r\n",
            *b"W1AW           9      20260510N\r\n",
            *b"zzzzzzzzzzzzzzz1      99999999N\r\n",
        ],
    );

    let db = CallBook::open(dir.path()).unwrap();
    let first = db.lookup_counts().unwrap().unwrap();
    let second = db.lookup_counts().unwrap().unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert!(first.path().ends_with("counts.dat"));
    assert_eq!(first.len(), 2);
    assert_eq!(
        first.iter().map(|record| record.key).collect::<Vec<_>>(),
        vec!["K0AB", "W1AW"]
    );
    assert_eq!(db.lookup_count("w1aw").unwrap().unwrap().count, 9);
}

#[test]
fn station_profile_and_map_workflows_use_semantic_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("bios/K")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("photos/K")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("flags")).unwrap();
    std::fs::create_dir_all(ham0_dir.join("maps")).unwrap();

    let mut plain = Vec::new();
    plain.extend_from_slice(b"\xb5K0AB\xc8");
    plain.extend_from_slice(b"41.7146\xc9");
    plain.extend_from_slice(b"-72.6553\xd1");
    plain.extend_from_slice(b"United States");
    let offset = 1u64;
    let encoded = encode_dat_record(offset, &plain);
    let mut dat = vec![0u8];
    dat.extend_from_slice(&encoded);
    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(&dat)
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK0AB 1 \r\nZZZZZZZZ 60 \r\n")
        .unwrap();
    std::fs::File::create(ham0_dir.join("countrys"))
        .unwrap()
        .write_all(b"K     N     UNITED STATES [US] !08@05#NA$38%-97^291\n")
        .unwrap();
    std::fs::write(ham0_dir.join("bios/K/K0AB.txt"), "profile bio").unwrap();
    std::fs::write(ham0_dir.join("photos/K/K0AB-2.JPG"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/K/K0AB-1.JPG"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB K0AB-2.JPG\n").unwrap();
    std::fs::write(ham0_dir.join("flags/US.GIF"), "gif").unwrap();
    std::fs::write(ham0_dir.join("maps/US.gif"), "gif").unwrap();
    std::fs::write(
        ham0_dir.join("wc.dat"),
        "# -b -73 -72 41 42 24\n-73 41\n-72 42\n",
    )
    .unwrap();
    std::fs::write(
        ham0_dir.join("USCOUN.DAT"),
        "001*Test County*-73*-72*41*42*24\n-73 41\n-72 42\n",
    )
    .unwrap();
    std::fs::write(
        ham0_dir.join("state.dat"),
        packed_state_bytes(&[(4000, 2460, -4380), (1, 2520, -4320)]),
    )
    .unwrap();
    let count_record = *b"K0AB           7      20260509N\r\n";
    let counts = count_record
        .iter()
        .enumerate()
        .map(|(offset, byte)| {
            let stream_key = ((offset as u64 + 4) % 101) as u8;
            (byte ^ 7) ^ stream_key
        })
        .collect::<Vec<_>>();
    std::fs::write(ham0_dir.join("counts.dat"), counts).unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let entry = db.lookup("k0ab").unwrap();
    let profile = entry.profile().unwrap();

    assert_eq!(profile.callsign, "K0AB");
    assert_eq!(
        profile.country.as_ref().unwrap().code.as_deref(),
        Some("US")
    );
    assert_eq!(profile.country.as_ref().unwrap().itu_zone, Some(8));
    assert_eq!(profile.lookup_count.as_ref().unwrap().count, 7);
    assert_eq!(
        profile.assets.bio_text().unwrap().as_deref(),
        Some("profile bio")
    );
    assert!(profile.assets.country_flag().is_some());
    assert!(profile.assets.country_map().is_some());
    assert!(profile
        .assets
        .primary_photo()
        .unwrap()
        .path
        .ends_with("K0AB-2.JPG"));

    let map = entry.map().unwrap();
    assert_eq!(
        map.station_location().unwrap(),
        callbook::GeoPoint {
            lon: -72.6553,
            lat: 41.7146
        }
    );
    assert_eq!(map.world_boundaries().unwrap().unwrap().point_count(), 2);
    assert_eq!(
        map.us_county_boundaries().unwrap().unwrap().counties.len(),
        1
    );
    assert_eq!(map.state_vectors().unwrap().unwrap().point_count(), 2);
    let layer_stats = map.layers().statistics().unwrap();
    assert_eq!(layer_stats.world_points, 2);
    assert_eq!(layer_stats.us_counties, 1);
    assert_eq!(layer_stats.state_segments, 1);
    assert!(map.render_svg().unwrap().unwrap().contains("<circle"));
    assert!(map
        .render_svg_with_options(callbook::StationMapRenderOptions::all_layers())
        .unwrap()
        .unwrap()
        .contains("stroke=\"#567\""));
    assert!(db.map_for_callsign("missing").unwrap().is_none());
}

#[test]
fn pc_country_and_name_centroid_enrich_country_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\n1A0AA 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(
        ham0_dir.join("COUNTRYS.PC"),
        b"#Don't sort this file\n1A     1A     SOV. MIL. ORDER OF MALTA\n",
    )
    .unwrap();
    std::fs::write(
        ham0_dir.join("countrys.nam"),
        b"SOV. MIL. ORDER OF MALTA (41.9, 12.4)\n",
    )
    .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let country = db.country_info("1A0AA").unwrap();

    assert_eq!(country.name, "SOV. MIL. ORDER OF MALTA");
    assert_eq!(country.raw_name, "SOV. MIL. ORDER OF MALTA");
    assert_eq!(country.cleaned_name, "SOV. MIL. ORDER OF MALTA");
    assert_eq!(country.source, callbook::CountryInfoSource::CountrysPc);
    assert_eq!(country.latitude, Some(41.9));
    assert_eq!(country.longitude, Some(12.4));
    assert!(db.pc_country_catalog().is_some());
    let catalog = db.country_catalog();
    let stats = catalog.statistics();
    assert_eq!(stats.fallback_rules, 1);
    assert_eq!(stats.name_centroids, 1);
    assert_eq!(
        catalog.grouping_label("1A0AA").as_deref(),
        Some("SOV. MIL. ORDER OF MALTA")
    );
}

#[test]
fn station_map_uses_latest_historical_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    let mut first = Vec::new();
    first.extend_from_slice(b"\xb5K0AB:2015\xc8");
    first.extend_from_slice(b"40.0\xc9");
    first.extend_from_slice(b"-70.0");
    let mut second = Vec::new();
    second.extend_from_slice(b"\xb5K0AB:2020\xc8");
    second.extend_from_slice(b"41.0\xc9");
    second.extend_from_slice(b"-71.0");
    let first_offset = 1u64;
    let first_encoded = encode_dat_record(first_offset, &first);
    let second_offset = first_offset + first_encoded.len() as u64;
    let second_encoded = encode_dat_record(second_offset, &second);
    let mut dat = vec![0u8];
    dat.extend_from_slice(&first_encoded);
    dat.extend_from_slice(&second_encoded);
    std::fs::write(ham0_dir.join("hamcall.dat"), dat).unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        format!(
            "!!! 0 \r\nK0AB:2015 {first_offset} \r\nK0AB:2020 {second_offset} \r\nZZZZZZZZ 80 \r\n"
        ),
    )
    .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let map = db.map_for_callsign("K0AB").unwrap().unwrap();
    assert_eq!(
        map.station_location().unwrap(),
        callbook::GeoPoint {
            lon: -71.0,
            lat: 41.0
        }
    );
}

#[test]
fn profile_assets_tolerate_bad_photo_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("photos/K")).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/K/K0AB.JPG"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), [0xff, 0xfe]).unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert_eq!(profile.assets.photos().len(), 1);
    assert!(db.asset_catalog().diagnostics().is_err());
}

#[test]
fn photo_manifest_can_locate_non_prefixed_photo() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("photos/cards")).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/cards/qsl-one.jpg"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB qsl-one.jpg\n").unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert_eq!(profile.assets.photos().len(), 1);
    assert!(profile
        .assets
        .primary_photo()
        .unwrap()
        .path
        .ends_with("qsl-one.jpg"));
}

#[test]
fn photo_manifest_rejects_wrong_basename_for_relative_path() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("photos/other")).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/other/qsl.jpg"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB cards/qsl.jpg\n").unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert!(profile.assets.photos().is_empty());
}

#[test]
fn photo_manifest_can_locate_relative_suffix_photo() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("photos/K/cards")).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/K/cards/qsl.jpg"), "jpg").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB cards/qsl.jpg\n").unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert_eq!(profile.assets.photos().len(), 1);
    assert!(profile
        .assets
        .primary_photo()
        .unwrap()
        .path
        .ends_with("cards/qsl.jpg"));
}

#[test]
fn photo_manifest_rejects_paths_outside_photos_tree() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(ham0_dir.join("photos")).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/hamcall.dat"), "not a photo").unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB ../hamcall.dat\n").unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert!(profile.assets.photos().is_empty());
}

fn encode_dat_record(dat_offset: u64, plain: &[u8]) -> Vec<u8> {
    let phase = ((dat_offset % 101) * 2 + 3).rem_euclid(101) as u8;
    let mut out = Vec::with_capacity(plain.len());
    let mut key = (1 - dat_offset as i64 + i64::from(phase)).rem_euclid(101) as usize;
    let mut remaining = plain;
    while !remaining.is_empty() {
        let n = (101 - key).min(remaining.len());
        out.extend(remaining[..n].iter().enumerate().map(|(i, &byte)| {
            let key = (key + i) as u8;
            (byte ^ 7) ^ key
        }));
        remaining = &remaining[n..];
        key = 0;
    }
    out
}

#[cfg(unix)]
#[test]
fn profile_assets_ignore_symlinked_photo_targets() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    let outside_dir = dir.path().join("outside");
    std::fs::create_dir_all(ham0_dir.join("photos")).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();

    std::fs::write(ham0_dir.join("hamcall.dat"), b"headerrecord").unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 6 \r\nZZZZZZZZ 11 \r\n",
    )
    .unwrap();
    std::fs::write(outside_dir.join("K0AB.JPG"), "outside").unwrap();
    symlink(
        outside_dir.join("K0AB.JPG"),
        ham0_dir.join("photos/K0AB.JPG"),
    )
    .unwrap();
    std::fs::write(ham0_dir.join("photos/PHOTOS.TXT"), "K0AB K0AB.JPG\n").unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let profile = db.lookup("K0AB").unwrap().profile().unwrap();

    assert!(profile.assets.photos().is_empty());
}

fn packed_state_bytes(points: &[(i16, i16, i16)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (command, lat_minutes, lon_minutes) in points {
        out.extend(command.to_le_bytes());
        out.extend(lat_minutes.to_le_bytes());
        out.extend(lon_minutes.to_le_bytes());
    }
    out
}

fn write_encoded_count_slots(path: &std::path::Path, slots: &[[u8; 33]]) {
    let encoded = slots
        .iter()
        .flatten()
        .copied()
        .enumerate()
        .map(|(offset, byte)| {
            let stream_key = ((offset as u64 + 4) % 101) as u8;
            (byte ^ 7) ^ stream_key
        })
        .collect::<Vec<_>>();
    std::fs::write(path, encoded).unwrap();
}

fn write_usa_csv_zip(path: &std::path::Path, csv: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("usa.csv", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(csv.as_bytes()).unwrap();
    zip.finish().unwrap();
}

#[test]
fn station_map_svg_render_options_control_preview_limits() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    let mut plain = Vec::new();
    plain.extend_from_slice(b"\xb5K0AB\xc8");
    plain.extend_from_slice(b"41.0\xc9");
    plain.extend_from_slice(b"-71.0");
    let encoded = encode_dat_record(1, &plain);
    let mut dat = vec![0u8];
    dat.extend_from_slice(&encoded);
    std::fs::write(ham0_dir.join("hamcall.dat"), dat).unwrap();
    std::fs::write(
        ham0_dir.join("hamcall.idx"),
        b"!!! 0 \r\nK0AB 1 \r\nZZZZZZZZ 40 \r\n",
    )
    .unwrap();
    let mut wc = String::new();
    for index in 0..513 {
        wc.push_str(&format!("# -b -73 -72 41 42 {index}\n-73 41\n-72 42\n"));
    }
    std::fs::write(ham0_dir.join("wc.dat"), wc).unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let map = db.lookup("K0AB").unwrap().map().unwrap();
    let complete = map.render_svg().unwrap().unwrap();
    let preview = map
        .render_svg_with_options(callbook::StationMapRenderOptions::preview())
        .unwrap()
        .unwrap();

    assert_eq!(complete.matches("<polyline").count(), 513);
    assert_eq!(preview.matches("<polyline").count(), 512);
}

#[test]
fn lookup_entry_and_batch_report_not_found_status() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(b"headerrecord")
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK1ABC 6 \r\nZZZZZZZZ 11 \r\n")
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let entry = db.lookup("NOPE1").unwrap();
    assert_eq!(entry.callsign(), "NOPE1");
    assert_eq!(entry.status(), callbook::LookupStatus::NotFound);
    assert!(entry.current().is_none());
    assert!(entry.history().is_empty());

    let mut batch = db.batch_lookup();
    let batch_entry = batch.lookup("nope1").unwrap();
    assert_eq!(batch_entry.callsign(), "NOPE1");
    assert_eq!(batch_entry.status(), entry.status());
}

#[test]
fn entries_iterates_domain_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ham0_dir = dir.path().join("ham0");
    std::fs::create_dir_all(&ham0_dir).unwrap();

    let offset = 1u64;
    let encoded = encode_dat_record(offset, b"\xb5K1ABC");
    let mut dat = vec![0u8];
    dat.extend_from_slice(&encoded);
    std::fs::File::create(ham0_dir.join("hamcall.dat"))
        .unwrap()
        .write_all(&dat)
        .unwrap();
    std::fs::File::create(ham0_dir.join("hamcall.idx"))
        .unwrap()
        .write_all(b"!!! 0 \r\nK1ABC 1 \r\nK1ABC:2020 1 \r\nZZZZZZZZ 11 \r\n")
        .unwrap();

    let db = CallBook::open(dir.path()).unwrap();
    let callsigns = db
        .entries()
        .into_iter()
        .map(|entry| entry.unwrap().callsign().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(callsigns, vec!["K1ABC"]);

    let current_only = db
        .entries()
        .include_history(false)
        .into_iter()
        .map(|entry| entry.unwrap().callsign().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(current_only, vec!["K1ABC"]);
}
