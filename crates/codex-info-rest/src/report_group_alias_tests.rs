
// Requirement-derived integration: SQLite -> real DbReader -> publication -> HTTP.
#[test]
fn attempt8_history_alias_cursor_and_local_usage(){
 let s=1803000060_i64;let a=s+10800;let b=a+5;
 let path=temp_db("attempt8-alias");
 Connection::open(&path).unwrap().execute_batch(include_str!("../../../tests/fixtures/report_groups.sql")).unwrap();
 let id=codex_info_db_reader::StoragePartitionIdentity{schema_version:"codex-info-account-db-v1".into(),profile_scope_id:"11".repeat(16),account_scope_id:"22".repeat(32),storage_epoch:13,partition_id:"33".repeat(32)};
 let reader=DbReader::open_partitioned(&path,&id).unwrap();
 let mut server=fixed_time_serialization_fixture(reader,s+10920);
 let fetch=|url:&str,extension:bool|->(String,Value){let ext=if extension{"Codex-Info-History-Extensions: local-usage-v1\r\n"}else{""};let raw=request(server.local_addr(),&format!("GET {url} HTTP/1.1\r\nHost:x\r\n{ext}\r\n"));assert!(raw.starts_with("HTTP/1.1 200"),"{}",raw.lines().next().unwrap());let body:Value=serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();(raw,body)};
 let(_,directory)=fetch("/v5/history/periods?account=account-1",false);
 let periods=directory["history_periods"].as_array().unwrap();assert_eq!(periods.iter().filter(|p|p["reset_at"]==a).count(),1);assert!(!periods.iter().any(|p|p["reset_at"]==b));assert!(periods.iter().any(|p|p["reset_at"]==a+60));
 let(_,selectable)=fetch("/v5/history/periods/selectable?account=account-1",false);assert_eq!(selectable["history_periods"].as_array().unwrap().iter().filter(|p|p["reset_at"]==a).count(),1);assert!(!selectable["history_periods"].as_array().unwrap().iter().any(|p|p["reset_at"]==b));
 let(raw,body)=fetch(&format!("/v3/history?account=account-1&period={b}"),true);
 let points=body["history_samples"].as_array().unwrap();assert!(points.iter().all(|p|p["reset_at"]==b));
 let last=points.iter().find(|p|p["timestamp"]==s+180).unwrap();assert!(last["models"].as_array().unwrap().iter().any(|m|m["model"]=="gpt-6.1-sol"&&m["total_tokens"]==99));
 assert_eq!(body["local_usage"]["start_at"],s);assert_eq!(body["local_usage"]["end_at"],s+182);
 let cursor=body["resume_cursor"].as_str().unwrap();
 let(raw2,resumed)=fetch(&format!("/v3/history?account=account-1&period={b}&cursor={cursor}"),true);
 let pair=|x:&str|x.lines().find(|l|l.to_ascii_lowercase().starts_with("codex-info-published-pair:")).unwrap().to_owned();assert_eq!(pair(&raw),pair(&raw2));assert!(resumed["history_samples"].as_array().unwrap().is_empty());
 server.shutdown();fs::remove_file(path).unwrap();
}

#[test]
fn attempt8_history_extension_preserves_saved_dollar_bits(){
 let s=1803000060_i64;let a=s+10800;
 let path=temp_db("attempt13-saved-usd-bits");
 let db=Connection::open(&path).unwrap();
 db.execute_batch(include_str!("../../../tests/fixtures/report_groups.sql")).unwrap();
 db.execute("UPDATE usage_model_history SET total_dollars=?1 WHERE reset_at=?2 AND timestamp=?3 AND model='gpt-6-sol'",rusqlite::params![f64::from_bits(0x3ff01a35a0cbbd03),a,s+60]).unwrap();drop(db);
 let identity=codex_info_db_reader::StoragePartitionIdentity{schema_version:"codex-info-account-db-v1".into(),profile_scope_id:"11".repeat(16),account_scope_id:"22".repeat(32),storage_epoch:13,partition_id:"33".repeat(32)};
 let reader=DbReader::open_partitioned(&path,&identity).unwrap();
 let mut server=fixed_time_serialization_fixture(reader,s+10920);
 let plain=request(server.local_addr(),&format!("GET /v3/history?account=account-1&period={a} HTTP/1.1\r\nHost:x\r\n\r\n"));
 let extended=request(server.local_addr(),&format!("GET /v3/history?account=account-1&period={a} HTTP/1.1\r\nHost:x\r\nCodex-Info-History-Extensions: local-usage-v1\r\n\r\n"));
 let saved="\"total_dollars\":1.0063988000000001";
 assert!(plain.starts_with("HTTP/1.1 200")&&plain.contains(saved),"plain history must preserve stored binary64");
 assert!(extended.starts_with("HTTP/1.1 200")&&extended.contains(saved),"local_usage extension must preserve stored binary64");
 server.shutdown();fs::remove_file(path).unwrap();
}


fn period_rest_prep_insert_raw_legacy_row(
    db: &Connection,
    reset_at: i64,
    timestamp: i64,
    dollars: f64,
) {
    db.execute(
        "INSERT INTO usage_model_history (
             reset_at, timestamp, model, total_tokens, input_tokens,
             cached_input_tokens, output_tokens, cache_write_input_tokens,
             model_set_complete, total_dollars, price_version
         ) VALUES (?1, ?2, 'legacy-raw', '777', '777', '0', '0', '0', 0, ?3,
                   '1791430564-183968cc652f6276')",
        rusqlite::params![reset_at, timestamp, dollars],
    )
    .unwrap();
}

#[test]
fn report_group_opaque_and_raw_history_cursor_local_usage() {
    const ORIGIN_MINUTE: i64 = 1_803_000_060;
    const RAW_REPRESENTATIVE: i64 = 1_803_010_860;
    const RAW_MEMBER: i64 = 1_803_010_865;
    const GROUP: &str = "rg1-1803000060-10800";
    let raw_representative_id = RAW_REPRESENTATIVE.to_string();
    let raw_member_id = RAW_MEMBER.to_string();

    let path = temp_db("period-rest-prep-opaque");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../../../tests/fixtures/report_groups.sql"))
        .unwrap();
    let raw_price = f64::from_bits(0x3ff123456789abcd);
    period_rest_prep_insert_raw_legacy_row(
        &db,
        RAW_MEMBER,
        ORIGIN_MINUTE + 120,
        raw_price,
    );
    drop(db);

    let identity = codex_info_db_reader::StoragePartitionIdentity {
        schema_version: "codex-info-account-db-v1".into(),
        profile_scope_id: "11".repeat(16),
        account_scope_id: "22".repeat(32),
        storage_epoch: 13,
        partition_id: "33".repeat(32),
    };
    let mut server = fixed_time_serialization_fixture(
        DbReader::open_partitioned(&path, &identity).unwrap(),
        ORIGIN_MINUTE + 10_920,
    );
    let fetch =
        |target: &str, local_usage: bool, expected_status: &str| -> (String, Value) {
        let extension = if local_usage {
            "Codex-Info-History-Extensions: local-usage-v1\r\n"
        } else {
            ""
        };
        let response = request(
            server.local_addr(),
            &format!(
                "GET {target} HTTP/1.1\r\nHost:x\r\n{extension}\r\n"
            ),
        );
        assert!(
            response.starts_with(expected_status),
            "{target}: {}",
            response.lines().next().unwrap_or_default()
        );
        (
            response.clone(),
            serde_json::from_str(body(&response)).unwrap(),
        )
        };

    let (directory_wire, directory) =
        fetch("/v5/history/periods", false, "HTTP/1.1 200");
    let pair = published_pair(&directory_wire).to_owned();
    let periods = directory["history_periods"].as_array().unwrap();
    let canonical: Vec<_> = periods
        .iter()
        .filter(|period| period["id"].as_str() == Some(GROUP))
        .collect();
    assert_eq!(canonical.len(), 1, "canonical directory has one opaque group");
    assert_eq!(canonical[0]["reset_at"], RAW_MEMBER);
    assert_eq!(canonical[0]["start_at"], ORIGIN_MINUTE);
    assert!(canonical[0]["end_at"].as_i64().unwrap() >= ORIGIN_MINUTE + 180);
    assert_eq!(canonical[0]["origin_at"], ORIGIN_MINUTE);
    assert!(!periods.iter().any(|period| {
        period["id"].as_str() == Some(raw_representative_id.as_str())
            || period["id"].as_str() == Some(raw_member_id.as_str())
    }));

    let (selectable_wire, selectable) =
        fetch("/v5/history/periods/selectable", false, "HTTP/1.1 200");
    assert_eq!(published_pair(&selectable_wire), pair);
    let selected = selectable["history_periods"].as_array().unwrap();
    assert_eq!(
        selected
            .iter()
            .filter(|period| period["id"].as_str() == Some(GROUP))
            .count(),
        1,
        "a raw legacy-positive member keeps the canonical group selectable"
    );
    assert!(!selected.iter().any(|period| {
        period["id"].as_str() == Some(raw_representative_id.as_str())
            || period["id"].as_str() == Some(raw_member_id.as_str())
    }));

    let (canonical_wire, canonical_body) =
        fetch(&format!("/v3/history?period={GROUP}"), false, "HTTP/1.1 200");
    assert_eq!(published_pair(&canonical_wire), pair);
    let canonical_points = canonical_body["history_samples"].as_array().unwrap();
    assert!(!canonical_points.is_empty());
    assert!(canonical_points
        .iter()
        .all(|point| point["reset_at"] == RAW_MEMBER));
    for point in canonical_points {
        if let Some(models) = point["models"].as_array() {
            assert!(!models.iter().any(|model| model["model"] == "legacy-raw"));
        }
    }

    let (raw_wire, raw_body) =
        fetch(&format!("/v3/history?period={RAW_MEMBER}"), false, "HTTP/1.1 200");
    assert_eq!(published_pair(&raw_wire), pair);
    let raw_points = raw_body["history_samples"].as_array().unwrap();
    assert!(!raw_points.is_empty());
    assert!(raw_points.iter().all(|point| point["reset_at"] == RAW_MEMBER));
    let raw_model = raw_points
        .iter()
        .filter_map(|point| point["models"].as_array())
        .flat_map(|models| models.iter())
        .find(|model| model["model"] == "legacy-raw")
        .expect("raw history keeps the positive legacy model");
    assert_eq!(raw_model["total_tokens"], 777);
    assert_eq!(
        raw_model["total_dollars"].as_f64().unwrap().to_bits(),
        raw_price.to_bits()
    );
    assert_ne!(canonical_points, raw_points);

    let (_, canonical_extended) =
        fetch(&format!("/v3/history?period={GROUP}"), true, "HTTP/1.1 200");
    let (_, raw_extended) =
        fetch(&format!("/v3/history?period={RAW_MEMBER}"), true, "HTTP/1.1 200");
    assert!(canonical_extended["local_usage"].is_object());
    assert!(raw_extended["local_usage"].is_object());
    assert!(raw_extended["local_usage"]["end_at"]
        .as_i64()
        .unwrap()
        <= canonical_extended["local_usage"]["end_at"].as_i64().unwrap());

    let canonical_cursor = canonical_body["resume_cursor"]
        .as_str()
        .expect("canonical cursor")
        .to_owned();
    let raw_cursor = raw_body["resume_cursor"]
        .as_str()
        .expect("raw cursor")
        .to_owned();
    let (canonical_resume_wire, canonical_resume) = fetch(
        &format!("/v3/history?period={GROUP}&cursor={canonical_cursor}"),
        false,
        "HTTP/1.1 200",
    );
    assert!(canonical_resume_wire.starts_with("HTTP/1.1 200"));
    assert!(canonical_resume["history_samples"].as_array().unwrap().is_empty());
    let (raw_resume_wire, raw_resume) = fetch(
        &format!("/v3/history?period={RAW_MEMBER}&cursor={raw_cursor}"),
        false,
        "HTTP/1.1 200",
    );
    assert!(raw_resume_wire.starts_with("HTTP/1.1 200"));
    assert!(raw_resume["history_samples"].as_array().unwrap().is_empty());

    let (cross_raw_wire, _) = fetch(
        &format!("/v3/history?period={GROUP}&cursor={raw_cursor}"),
        false,
        "HTTP/1.1 400",
    );
    assert!(cross_raw_wire.starts_with("HTTP/1.1 400"));
    let (cross_canonical_wire, _) = fetch(
        &format!("/v3/history?period={RAW_MEMBER}&cursor={canonical_cursor}"),
        false,
        "HTTP/1.1 400",
    );
    assert!(cross_canonical_wire.starts_with("HTTP/1.1 400"));

    server.shutdown();
    fs::remove_file(path).unwrap();

    // Separate subcase: every recorded fact is explicitly ineligible while a
    // positive raw legacy model remains.  The group must still be published
    // once/selectable and the old raw value must remain directly readable.
    let eligible_zero_path = temp_db("period-rest-prep-eligible0");
    let db = Connection::open(&eligible_zero_path).unwrap();
    db.execute_batch(include_str!("../../../tests/fixtures/report_groups.sql"))
        .unwrap();
    db.execute("UPDATE recorded_usage_facts SET eligible=0", [])
        .unwrap();
    let raw_price = f64::from_bits(0x3ff323456789abcd);
    period_rest_prep_insert_raw_legacy_row(
        &db,
        RAW_MEMBER,
        ORIGIN_MINUTE + 120,
        raw_price,
    );
    let ineligible: i64 = db
        .query_row(
            "SELECT count(*) FROM recorded_usage_facts WHERE eligible=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(ineligible > 0, "eligible=0 subcase must be explicit");
    drop(db);

    let mut eligible_zero_server = fixed_time_serialization_fixture(
        DbReader::open_partitioned(&eligible_zero_path, &identity).unwrap(),
        ORIGIN_MINUTE + 10_920,
    );
    let fetch_eligible_zero = |target: &str, expected_status: &str| -> (String, Value) {
        let response = request(
            eligible_zero_server.local_addr(),
            &format!("GET {target} HTTP/1.1\r\nHost:x\r\n\r\n"),
        );
        assert!(response.starts_with(expected_status));
        (
            response.clone(),
            serde_json::from_str(body(&response)).unwrap(),
        )
    };
    let (directory_wire, directory) =
        fetch_eligible_zero("/v5/history/periods", "HTTP/1.1 200");
    let periods = directory["history_periods"].as_array().unwrap();
    assert_eq!(
        periods
            .iter()
            .filter(|period| period["id"].as_str() == Some(GROUP))
            .count(),
        1,
        "eligible=0 plus raw-positive still yields one canonical group"
    );
    assert!(!periods.iter().any(|period| {
        period["id"].as_str() == Some(raw_representative_id.as_str())
            || period["id"].as_str() == Some(raw_member_id.as_str())
    }));
    let (_, selectable) =
        fetch_eligible_zero("/v5/history/periods/selectable", "HTTP/1.1 200");
    assert_eq!(
        selectable["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|period| period["id"].as_str() == Some(GROUP))
            .count(),
        1
    );
    let (raw_wire, raw_body) = fetch_eligible_zero(
        &format!("/v3/history?period={RAW_MEMBER}"),
        "HTTP/1.1 200",
    );
    assert_eq!(published_pair(&raw_wire), published_pair(&directory_wire));
    let raw_model = raw_body["history_samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|point| point["models"].as_array())
        .flat_map(|models| models.iter())
        .find(|model| model["model"] == "legacy-raw")
        .expect("eligible=0 subcase retains raw legacy-positive model");
    assert_eq!(raw_model["total_tokens"], 777);
    assert_eq!(
        raw_model["total_dollars"].as_f64().unwrap().to_bits(),
        raw_price.to_bits()
    );
    eligible_zero_server.shutdown();
    fs::remove_file(eligible_zero_path).unwrap();
}

// period-rest-prep-final-amend-v1
// period-rest-prep-late-receipt-amend-v2
#[test]
fn report_group_raw_only_change_advances_publication() {
    const ORIGIN_MINUTE: i64 = 1_803_000_060;
    const RAW_MEMBER: i64 = 1_803_010_865;
    const GROUP: &str = "rg1-1803000060-10800";
    const RAW_TIMESTAMP: i64 = ORIGIN_MINUTE + 120;

    let path = temp_db("period-rest-prep-raw-update");
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../../../tests/fixtures/report_groups.sql"))
        .unwrap();
    let first_price = f64::from_bits(0x3ff123456789abcd);
    period_rest_prep_insert_raw_legacy_row(&db, RAW_MEMBER, RAW_TIMESTAMP, first_price);
    const LATE_CLOCK: i64 = ORIGIN_MINUTE + 32 * 86_400;
    const WINDOW_SECONDS: i64 = 10_800;
    const NEW_RESET: i64 = LATE_CLOCK + WINDOW_SECONDS;
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO quota_receipts (
             receipt_id, sequence, observed_at, reset_at, window_seconds,
             remaining_percent, applied_generation
         ) VALUES ('period-rest-prep-late-current', 83, ?1, ?2, ?3, 74.0, '83')",
        rusqlite::params![LATE_CLOCK, NEW_RESET, WINDOW_SECONDS],
    )
    .unwrap();
    let generation_rows = tx
        .execute(
            "UPDATE collection_generation
             SET data_generation='83', collector_epoch='11111111111111111111111111111111', cycle_seq='1',
                 reset_at=?1, latest_quota_reset_at=?1
             WHERE singleton=1 AND data_generation='82'
               AND collector_epoch IS NULL AND cycle_seq='0'",
            rusqlite::params![NEW_RESET],
        )
        .unwrap();
    assert_eq!(generation_rows, 1, "late receipt must advance the commit marker");
    tx.commit().unwrap();
    let initial_commit: (String, Option<String>, String, i64, i64) = db
        .query_row(
            "SELECT data_generation, collector_epoch, cycle_seq, reset_at,
                    latest_quota_reset_at
             FROM collection_generation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        initial_commit,
        (
            "83".to_owned(),
            Some("11111111111111111111111111111111".to_owned()),
            "1".to_owned(),
            NEW_RESET,
            NEW_RESET,
        )
    );
    let late_receipt: (i64, i64, i64, i64, f64, String) = db
        .query_row(
            "SELECT sequence, observed_at, reset_at, window_seconds,
                    remaining_percent, applied_generation
             FROM quota_receipts
             WHERE receipt_id='period-rest-prep-late-current'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(
        late_receipt,
        (83, LATE_CLOCK, NEW_RESET, WINDOW_SECONDS, 74.0, "83".to_owned())
    );
    drop(db);

    let identity = codex_info_db_reader::StoragePartitionIdentity {
        schema_version: "codex-info-account-db-v1".into(),
        profile_scope_id: "11".repeat(16),
        account_scope_id: "22".repeat(32),
        storage_epoch: 13,
        partition_id: "33".repeat(32),
    };
    let reader = DbReader::open_partitioned(&path, &identity).unwrap();
    let store = SnapshotStore::new(reader);
    assert!(matches!(store.refresh(), RefreshStatus::Updated { .. }));
    let mut server = serialization_listener(store);
    let fetch = |target: &str| -> (String, Value) {
        let response = request(
            server.local_addr(),
            &format!("GET {target} HTTP/1.1\r\nHost:x\r\n\r\n"),
        );
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "{target}: {}",
            response.lines().next().unwrap_or_default()
        );
        (
            response.clone(),
            serde_json::from_str(body(&response)).unwrap(),
        )
    };

    let (canonical_first_wire, canonical_first) =
        fetch(&format!("/v3/history?period={GROUP}"));
    let (raw_first_wire, raw_first) =
        fetch(&format!("/v3/history?period={RAW_MEMBER}"));
    let first_pair = published_pair(&raw_first_wire).to_owned();
    assert_eq!(published_pair(&canonical_first_wire), first_pair);
    let old_raw_cursor = raw_first["resume_cursor"]
        .as_str()
        .expect("first raw cursor")
        .to_owned();
    let old_canonical_samples = canonical_first["history_samples"].clone();
    let old_canonical_gaps = canonical_first["history_gaps"].clone();
    let old_raw_price = raw_first["history_samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|point| point["models"].as_array())
        .flat_map(|models| models.iter())
        .find(|model| model["model"] == "legacy-raw")
        .expect("first raw model")["total_dollars"]
        .as_f64()
        .unwrap()
        .to_bits();
    assert_eq!(old_raw_price, first_price.to_bits());

    let public_numeric_state = |snapshot: &PublishedSnapshot| {
        (
            snapshot.current,
            snapshot.details.quota.clone(),
            snapshot.details.models.clone(),
            snapshot.models_v3.clone(),
            snapshot.details.history_samples.clone(),
            snapshot.details.history_gaps.clone(),
            snapshot.history_samples_v2.clone(),
            snapshot.history_samples_v3.clone(),
        )
    };
    let old_snapshot = server.store().snapshot().unwrap();
    assert_eq!(
        old_snapshot.details.observed_at,
        Some(LATE_CLOCK),
        "the official late receipt is the reader's publication observation"
    );
    assert_eq!(
        old_snapshot.details.quota.as_ref().map(|quota| quota.reset_at),
        Some(NEW_RESET),
        "the late receipt is the current quota authority"
    );
    let recent_cutoff = LATE_CLOCK - 31 * 86_400;
    assert!(
        RAW_TIMESTAMP <= recent_cutoff,
        "the legacy raw row is outside the 31-day recent window"
    );
    assert!(
        old_snapshot
            .history_samples_v3
            .iter()
            .all(|point| point.timestamp > recent_cutoff),
        "recent snapshot fields exclude the retained raw-only row"
    );
    let old_data_hash = old_snapshot.data_hash.clone();
    let old_recent_current = public_numeric_state(old_snapshot.as_ref());

    let second_price = f64::from_bits(0x3ff223456789abcd);
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "UPDATE usage_model_history SET total_dollars=?1
         WHERE reset_at=?2 AND timestamp=?3 AND model='legacy-raw'",
        rusqlite::params![second_price, RAW_MEMBER, RAW_TIMESTAMP],
    )
    .unwrap();
    let generation_rows = tx
        .execute(
            "UPDATE collection_generation
             SET data_generation='84', cycle_seq='2'
             WHERE singleton=1 AND data_generation='83'
               AND collector_epoch='11111111111111111111111111111111'
               AND cycle_seq='1'",
            [],
        )
        .unwrap();
    assert_eq!(generation_rows, 1, "raw-only update must advance the committed generation");
    tx.commit().unwrap();
    let committed: (String, Option<String>, String) = db
        .query_row(
            "SELECT data_generation, collector_epoch, cycle_seq
             FROM collection_generation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        committed,
                ("84".to_owned(), Some("11111111111111111111111111111111".to_owned()), "2".to_owned())
    );
    drop(db);
    assert!(matches!(
        server.store().refresh(),
        RefreshStatus::Updated { .. }
    ));
    let new_snapshot = server.store().snapshot().unwrap();
    let new_recent_current = public_numeric_state(new_snapshot.as_ref());
    assert_ne!(
        old_data_hash,
        new_snapshot.data_hash,
        "publication data_hash is recomputed from the changed raw view"
    );
    assert_eq!(
        new_recent_current,
        old_recent_current,
        "retained raw-only price change leaves recent/current numeric public values unchanged"
    );

    let (canonical_second_wire, canonical_second) =
        fetch(&format!("/v3/history?period={GROUP}"));
    let (raw_second_wire, raw_second) =
        fetch(&format!("/v3/history?period={RAW_MEMBER}"));
    let second_pair = published_pair(&raw_second_wire).to_owned();
    assert_eq!(published_pair(&canonical_second_wire), second_pair);
    assert_ne!(first_pair, second_pair, "raw-only change gets a new pair");
    assert_eq!(canonical_second["history_samples"], old_canonical_samples);
    assert_eq!(canonical_second["history_gaps"], old_canonical_gaps);
    assert_ne!(raw_second["history_samples"], raw_first["history_samples"]);
    let new_raw_price = raw_second["history_samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|point| point["models"].as_array())
        .flat_map(|models| models.iter())
        .find(|model| model["model"] == "legacy-raw")
        .expect("updated raw model")["total_dollars"]
        .as_f64()
        .unwrap()
        .to_bits();
    assert_eq!(new_raw_price, second_price.to_bits());
    assert_ne!(old_raw_cursor, raw_second["resume_cursor"].as_str().unwrap());

    let stale = request(
        server.local_addr(),
        &format!(
            "GET /v3/history?period={RAW_MEMBER}&cursor={old_raw_cursor} HTTP/1.1\r\nHost:x\r\n\r\n"
        ),
    );
    assert_eq!(
        stale.lines().next().unwrap(),
        "HTTP/1.1 400 Bad Request",
        "the old raw cursor is bound to the new publication pair/fingerprint"
    );

    server.shutdown();
    fs::remove_file(path).unwrap();
}
