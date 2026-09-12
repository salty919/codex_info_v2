use codex_info::usage_store::{UsageHistorySample, UsageStore};
use rusqlite::{Connection, DatabaseName};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    rows: usize,
    reload: String,
    row_sha256: String,
    file_sha256: String,
}

fn fixture_path() -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "codex-info-db-protection-runtime-{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create isolated fixture directory");
    directory.join("usage_history.sqlite3")
}

fn sample(timestamp: i64, sol_tokens: u64, sol_dollars: f64) -> UsageHistorySample {
    UsageHistorySample {
        timestamp,
        reset_at: 1_700_003_600,
        remaining_percent: Some(75.0),
        sol_dollars,
        terra_dollars: 2.0,
        luna_dollars: 3.0,
        sol_tokens,
        terra_tokens: sol_tokens * 2,
        luna_tokens: sol_tokens * 3,
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha256sum must be available for runtime evidence");
    child
        .stdin
        .take()
        .expect("sha256sum stdin")
        .write_all(bytes)
        .expect("write sha256 input");
    let output = child.wait_with_output().expect("wait for sha256sum");
    assert!(output.status.success(), "sha256sum failed");
    String::from_utf8(output.stdout)
        .expect("sha256sum output is UTF-8")
        .split_whitespace()
        .next()
        .expect("sha256sum digest")
        .to_owned()
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("read SQLite file for SHA-256");
    sha256_bytes(&bytes)
}

fn canonical_rows(connection: &Connection) -> String {
    let mut statement = connection
        .prepare(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
             FROM usage_history
             ORDER BY reset_at, timestamp",
        )
        .expect("prepare canonical history query");
    let rows = statement
        .query_map([], |row| {
            let remaining: Option<f64> = row.get(2)?;
            Ok(format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}\n",
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                remaining.map_or_else(|| "NULL".to_owned(), |value| value.to_string()),
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .expect("query canonical history rows");
    rows.map(|row| row.expect("decode canonical history row"))
        .collect()
}

fn snapshot(label: &str, path: &Path) -> Snapshot {
    let connection = Connection::open(path).expect("open SQLite snapshot");
    let quick_check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .expect("quick_check");
    assert_eq!(quick_check, "ok", "quick_check failed for {label}");
    let rows: usize = connection
        .query_row("SELECT count(*) FROM usage_history", [], |row| row.get(0))
        .expect("count history rows");
    let reload = connection
        .query_row(
            "SELECT count(*) || ':' || COALESCE(SUM(sol_tokens), 0) || ':' ||
                    COALESCE(SUM(terra_tokens), 0) || ':' || COALESCE(SUM(luna_tokens), 0)
             FROM usage_history",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("reload history totals");
    let canonical = canonical_rows(&connection);
    let result = Snapshot {
        rows,
        reload,
        row_sha256: sha256_bytes(canonical.as_bytes()),
        file_sha256: sha256_file(path),
    };
    println!(
        "runtime-sqlite: label={label} quick_check=ok rows={} reload={} row_sha256={} file_sha256={}",
        result.rows, result.reload, result.row_sha256, result.file_sha256
    );
    result
}

fn backup_online(source: &Path, destination: &Path) {
    let connection = Connection::open(source).expect("open source for online restore");
    connection
        .backup(DatabaseName::Main, destination, None)
        .expect("SQLite online backup");
    drop(connection);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o600))
            .expect("make restored database private");
    }
}

fn write_sample(path: &Path, sample: &UsageHistorySample) {
    let store = UsageStore::open(path).expect("open fixture through UsageStore");
    store.upsert_sample(sample).expect("persist fixture sample");
}

fn remove_fixture(path: &Path) -> io::Result<()> {
    let directory = path.parent().expect("fixture parent");
    fs::remove_dir_all(directory)
}

#[test]
fn db_protection_runtime_backup_migration_restore() {
    let path = fixture_path();
    let result = std::panic::catch_unwind(|| {
        let first = sample(1_700_000_060, 11, 1.25);
        let second = sample(1_700_000_120, 44, 1.75);
        let third = sample(1_700_000_180, 77, 2.25);

        write_sample(&path, &first);
        UsageStore::backup_generations(&path, 3).expect("first backup generation");
        write_sample(&path, &second);
        UsageStore::backup_generations(&path, 3).expect("second backup generation");
        write_sample(&path, &third);
        UsageStore::backup_generations(&path, 3).expect("third backup generation");

        let backup_one = snapshot("backup-1", &path.with_extension("sqlite3.bak.1"));
        let backup_two = snapshot("backup-2", &path.with_extension("sqlite3.bak.2"));
        let backup_three = snapshot("backup-3", &path.with_extension("sqlite3.bak.3"));
        assert_eq!(backup_one.rows, 3);
        assert_eq!(backup_two.rows, 2);
        assert_eq!(backup_three.rows, 1);
        println!(
            "backup-generations: PASS gen1_rows={} gen2_rows={} gen3_rows={} gen1_row_sha256={} gen2_row_sha256={} gen3_row_sha256={}",
            backup_one.rows,
            backup_two.rows,
            backup_three.rows,
            backup_one.row_sha256,
            backup_two.row_sha256,
            backup_three.row_sha256
        );

        let source_before_failure = snapshot("source-before-backup-failure", &path);
        let generation_one_before_failure = snapshot(
            "backup-1-before-failure",
            &path.with_extension("sqlite3.bak.1"),
        );
        let blocked_generation = path.with_extension("sqlite3.bak.4");
        fs::create_dir(&blocked_generation).expect("create backup failure blocker");
        assert!(UsageStore::backup_generations(&path, 4).is_err());
        fs::remove_dir(&blocked_generation).expect("remove backup failure blocker");
        let source_after_failure = snapshot("source-after-backup-failure", &path);
        let generation_one_after_failure = snapshot(
            "backup-1-after-failure",
            &path.with_extension("sqlite3.bak.1"),
        );
        assert_eq!(source_before_failure, source_after_failure);
        assert_eq!(generation_one_before_failure, generation_one_after_failure);
        println!(
            "backup-failure-source-preserved: PASS rows={} row_sha256={} file_sha256={}",
            source_after_failure.rows,
            source_after_failure.row_sha256,
            source_after_failure.file_sha256
        );

        let pre_migration = snapshot("source-before-successful-migration", &path);
        let report = UsageStore::migrate_verified(&path, |samples| {
            let mut migrated = samples.to_vec();
            migrated[0].sol_dollars += 0.5;
            Ok(migrated)
        })
        .expect("successful verified migration");
        let preserved = snapshot("migration-preserved-source", &report.preserved_backup);
        assert_eq!(pre_migration.rows, preserved.rows);
        assert_eq!(pre_migration.reload, preserved.reload);
        assert_eq!(pre_migration.row_sha256, preserved.row_sha256);
        assert_eq!(pre_migration.file_sha256, preserved.file_sha256);
        let migrated = snapshot("migration-current-candidate", &path);
        println!(
            "migration-success: PASS source_rows={} candidate_rows={} source_fingerprint={} candidate_fingerprint={} preserved_rows={} preserved_row_sha256={} preserved_file_sha256={} current_row_sha256={}",
            report.source_rows,
            report.candidate_rows,
            report.source_fingerprint,
            report.candidate_fingerprint,
            preserved.rows,
            preserved.row_sha256,
            preserved.file_sha256,
            migrated.row_sha256
        );
        fs::remove_file(&report.preserved_backup).expect("remove checked rollback fixture");

        let before_failed_migration = snapshot("source-before-failed-migration", &path);
        let failure = UsageStore::migrate_verified(&path, |samples| {
            let mut candidate = samples.to_vec();
            candidate[0].remaining_percent = Some(101.0);
            Ok(candidate)
        });
        assert!(failure.is_err(), "invalid candidate must fail closed");
        let after_failed_migration = snapshot("source-after-failed-migration", &path);
        assert_eq!(before_failed_migration, after_failed_migration);
        println!(
            "migration-failure-source-preserved: PASS rows={} reload={} row_sha256={} file_sha256={}",
            after_failed_migration.rows,
            after_failed_migration.reload,
            after_failed_migration.row_sha256,
            after_failed_migration.file_sha256
        );

        let restore_path = path.with_file_name("restored.sqlite3");
        backup_online(&path.with_extension("sqlite3.bak.1"), &restore_path);
        let restored = snapshot("manual-restore-from-backup-1", &restore_path);
        let pre_restore_source = snapshot(
            "backup-1-source-of-restore",
            &path.with_extension("sqlite3.bak.1"),
        );
        assert_eq!(restored.rows, pre_restore_source.rows);
        assert_eq!(restored.reload, pre_restore_source.reload);
        assert_eq!(restored.row_sha256, pre_restore_source.row_sha256);
        println!(
            "manual-restore: PASS rows={} reload={} row_sha256={} quick_check=ok",
            restored.rows, restored.reload, restored.row_sha256
        );

        drop(restored);
        drop(pre_restore_source);
        let before_reopen = snapshot("before-reopen", &path);
        drop(UsageStore::open(&path).expect("reopen migrated database"));
        let after_reopen = snapshot("after-reopen", &path);
        assert_eq!(before_reopen, after_reopen);
        println!(
            "restart-reload-source-preserved: PASS rows={} row_sha256={} file_sha256={}",
            after_reopen.rows, after_reopen.row_sha256, after_reopen.file_sha256
        );
    });

    let cleanup_result = remove_fixture(&path);
    result.expect("DB protection runtime probe failed");
    cleanup_result.expect("remove isolated DB protection fixture");
}
