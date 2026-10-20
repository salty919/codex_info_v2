//! Bounded maintainer entrypoint for an independently qualified Issue #604
//! repair packet. It does not discover accounts or rotate backups. Migration
//! requires the explicit --migrate-v16 action and never also repairs rows.
//! The caller supplies a private plan and a separate preserved DB.

use codex_info_db_writer::{
    QuotaReceipt, SessionLifecycleInterval, StoragePartitionIdentity, UsageStore,
};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    start_at: Option<i64>,
    end_at: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    identity: StoragePartitionIdentity,
    intervals: Vec<Interval>,
    receipts: Vec<QuotaReceipt>,
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let migrate = arguments
        .first()
        .is_some_and(|argument| argument == std::path::Path::new("--migrate-v16"));
    if migrate {
        arguments.remove(0);
    }
    if arguments.len() != 3 {
        return Err("usage: issue604_repair_quota_history [--migrate-v16] DATABASE PRESERVED_DATABASE PRIVATE_PLAN_JSON".into());
    }
    let plan_file = std::fs::File::open(&arguments[2])?;
    if plan_file.metadata()?.len() > 4 * 1024 * 1024 {
        return Err("quota repair plan exceeds its bounded size".into());
    }
    let plan: Plan = serde_json::from_reader(plan_file)?;
    if migrate {
        let proof = UsageStore::verify_preserved_partition_backup(
            &arguments[0],
            &arguments[1],
            &plan.identity,
        )?;
        let changed = UsageStore::migrate_partition_history_after_verified_backup(
            &arguments[0],
            &plan.identity,
            &proof,
        )?;
        println!("{{\"migrated_schema\":{changed}}}");
        return Ok(());
    }
    let intervals = plan
        .intervals
        .into_iter()
        .map(|interval| SessionLifecycleInterval::new(interval.start_at, interval.end_at))
        .collect::<Vec<_>>();
    let changed = UsageStore::repair_quota_history_after_backup(
        &arguments[0],
        &arguments[1],
        &plan.identity,
        &intervals,
        &plan.receipts,
    )?;
    println!("{{\"repaired_minutes\":{changed}}}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("quota history repair rejected: {error}");
        std::process::exit(1);
    }
}
