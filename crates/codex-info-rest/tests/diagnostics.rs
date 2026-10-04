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
