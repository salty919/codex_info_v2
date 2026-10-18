use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct TestProcess(Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
#[test]
fn issue604_catalog_becomes_available_without_restarting_the_listener() {
    use codex_info_account_locator::{
        admit_partition, ensure_partition, mark_partition_initialized, AccountConfirmation,
    };
    use codex_info_db_writer::{StoragePartitionIdentity, UsageStore};
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = root.path().join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    for path in [&home, &data] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut process = TestProcess(
        Command::new(env!("CARGO_BIN_EXE_codex_info_rest"))
            .args(["--port", "0"])
            .env("CODEX_HOME", &home)
            .env("CODEX_INFO_DATA_DIR", &data)
            .env_remove("CODEX_INFO_DB_PATH")
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut errors = BufReader::new(process.0.stderr.take().unwrap());
    let mut startup = String::new();
    errors.read_line(&mut startup).unwrap();
    let address = startup
        .split_whitespace()
        .find_map(|word| word.strip_prefix("listener="))
        .unwrap();
    let mut failure_stage = String::new();
    errors.read_line(&mut failure_stage).unwrap();
    assert_eq!(
        failure_stage.trim(),
        "codex_info_rest: catalog unavailable stage=current-partition"
    );
    let catalog = || -> std::io::Result<serde_json::Value> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(
            stream,
            "GET /v3/accounts HTTP/1.1\r\nHost:{address}\r\n\r\n"
        )?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        Ok(serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap())
    };
    assert!(catalog().unwrap()["default_account_id"].is_null());
    let auth = home.join("auth.json");
    std::fs::write(&auth, br#"{"tokens":{"account_id":"catalog-fixture"}}"#).unwrap();
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let reserved = ensure_partition(&home, &data).unwrap();
    let partition = admit_partition(
        &home,
        &data,
        &reserved,
        AccountConfirmation::InitialRpc { began_at: now },
        || Some(now),
    )
    .unwrap();
    let _writer = UsageStore::create_partitioned(
        &partition.database_path,
        &StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: partition.profile_scope_id.clone(),
            account_scope_id: partition.account_scope_id.clone(),
            storage_epoch: partition.storage_epoch,
            partition_id: partition.partition_id.clone(),
        },
    )
    .unwrap();
    let connection = rusqlite::Connection::open(&partition.database_path).unwrap();
    connection
        .execute(
            "UPDATE collection_generation SET data_generation='1',collector_epoch=?1,cycle_seq='1'",
            ["11".repeat(16)],
        )
        .unwrap();
    mark_partition_initialized(&data, &partition).unwrap();
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let start = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let state = serde_json::json!({"schema":"codex-info-recorder-state-v1","pid":std::process::id(),"process_starttime":start,
        "owner_nonce":"22222222222222222222222222222222","write_state":"ready","partition_id_hash":partition.partition_id,
        "data_generation":1,"collector_epoch":"11111111111111111111111111111111","cycle_seq":1,"last_commit_unix":now,"updated_at_unix":now});
    let state_path = data.join("history/recorder-state.json");
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    std::fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(process.0.try_wait().unwrap().is_none(),"the same listener must reconcile a newly admitted catalog instead of exiting for supervisor restart");
        let observed=catalog().unwrap_or_else(|error| {
            std::thread::sleep(Duration::from_millis(50));
            let status=process.0.try_wait().unwrap();
            panic!("catalog reconciliation must preserve the same listener: status={status:?}, transport={error}");
        });
        if observed["default_account_id"] == partition.public_id() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the admitted account was never catalogued by this listener"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn failure_request_is_logged_without_private_request_data() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex-home");
    let data = root.path().join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let mut process = TestProcess(
        Command::new(env!("CARGO_BIN_EXE_codex_info_rest"))
            .args(["--port", "0"])
            .env("CODEX_HOME", &home)
            .env("CODEX_INFO_DATA_DIR", &data)
            .env_remove("CODEX_INFO_DB_PATH")
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut errors = BufReader::new(process.0.stderr.take().unwrap());
    let mut startup = String::new();
    errors.read_line(&mut startup).unwrap();
    let address = startup
        .split_whitespace()
        .find_map(|word| word.strip_prefix("listener="))
        .unwrap_or_else(|| panic!("REST startup: {startup}"));
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "POST /v1/runtime HTTP/1.1\r\nHost:{address}\r\nAuthorization: Bearer private-token-sentinel\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 405"), "{response}");
    let day = Command::new("date").args(["-u", "+%F"]).output().unwrap();
    let day = String::from_utf8(day.stdout).unwrap();
    let path = data
        .join("logs/rest")
        .join(format!("rest-{}.log", day.trim()));
    let log = std::fs::read_to_string(&path).expect("failure must reach the daily REST log");
    assert!(log.contains("method_not_allowed"), "{log}");
    assert!(log.contains("route_input"), "{log}");
    assert!(log.contains("/v1/runtime"), "{log}");
    assert!(!log.contains("private-token-sentinel"));
    assert!(!log.contains("Authorization"));
}
