//! Derived local usage. The original source ledger remains numeric authority;
//! provider reset observations never choose or clear a recorded fact.
use crate::{event_admission::NumericEventProof, LosslessSessionEvent, SessionEventSource};
use crate::{
    params, BTreeMap, BTreeSet, Connection, Digest, OptionalExtension, Result, Sha256,
    UsageStoreError,
};
use codex_info_db_reader::recorded_usage::{RecordedTokenPrefix, RecordingCoverage};
use rusqlite::Transaction;

pub(super) const TABLES: [&str; 3] = [
    "recorded_usage_facts",
    "recorded_usage_prefixes",
    "recorded_usage_state",
];
pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recorded_usage_facts (
 fact_id TEXT NOT NULL PRIMARY KEY,
 source_key TEXT NOT NULL UNIQUE,
 timestamp INTEGER NOT NULL CHECK(timestamp>0),
 model TEXT NOT NULL,
 tokens_json TEXT NOT NULL,
 evidence_sha256 TEXT NOT NULL,
 source_identity TEXT NOT NULL,
 ambiguity_key TEXT,
 eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS recorded_usage_facts_time ON recorded_usage_facts(timestamp,model,fact_id);
CREATE INDEX IF NOT EXISTS recorded_usage_facts_model ON recorded_usage_facts(model,timestamp,fact_id);
CREATE INDEX IF NOT EXISTS recorded_usage_facts_ambiguity ON recorded_usage_facts(ambiguity_key,source_identity);
CREATE TABLE IF NOT EXISTS recorded_usage_prefixes (
 model TEXT NOT NULL, timestamp INTEGER NOT NULL CHECK(timestamp>0),
 prefix_json TEXT NOT NULL, prefix_sha256 TEXT NOT NULL,
 PRIMARY KEY(model,timestamp)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS recorded_usage_prefixes_time ON recorded_usage_prefixes(timestamp,model);
CREATE TABLE IF NOT EXISTS recorded_usage_state (
 singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
 format_version INTEGER NOT NULL CHECK(format_version=1),
 initialized INTEGER NOT NULL CHECK(initialized IN (0,1)),
 fact_revision TEXT NOT NULL,
 models_json TEXT NOT NULL,
 coverage TEXT NOT NULL,
 dirty_json TEXT NOT NULL
) WITHOUT ROWID;
INSERT INTO recorded_usage_state VALUES(1,1,0,'0','[]','unknown','{}') ON CONFLICT(singleton) DO NOTHING;
"#;

fn invalid(message: &str) -> UsageStoreError {
    UsageStoreError::InvalidImport(message.into())
}
fn encoded<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid("recorded usage encoding failed"))
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    type ColumnShape<'a> = (&'a str, &'a str, i64);
    type TableShape<'a> = (&'a str, &'a [ColumnShape<'a>]);
    let expected: [TableShape<'_>; 3] = [
        (
            TABLES[0],
            &[
                ("fact_id", "TEXT", 1),
                ("source_key", "TEXT", 0),
                ("timestamp", "INTEGER", 0),
                ("model", "TEXT", 0),
                ("tokens_json", "TEXT", 0),
                ("evidence_sha256", "TEXT", 0),
                ("source_identity", "TEXT", 0),
                ("ambiguity_key", "TEXT", 0),
                ("eligible", "INTEGER", 0),
            ],
        ),
        (
            TABLES[1],
            &[
                ("model", "TEXT", 1),
                ("timestamp", "INTEGER", 2),
                ("prefix_json", "TEXT", 0),
                ("prefix_sha256", "TEXT", 0),
            ],
        ),
        (
            TABLES[2],
            &[
                ("singleton", "INTEGER", 1),
                ("format_version", "INTEGER", 0),
                ("initialized", "INTEGER", 0),
                ("fact_revision", "TEXT", 0),
                ("models_json", "TEXT", 0),
                ("coverage", "TEXT", 0),
                ("dirty_json", "TEXT", 0),
            ],
        ),
    ];
    for (table, columns) in expected {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let actual = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if actual.len() != columns.len()
            || actual
                .iter()
                .zip(columns)
                .any(|((name, kind, required, pk), (en, ek, ep))| {
                    name != en || kind != ek || pk != ep || *required != (name != "ambiguity_key")
                })
        {
            return Err(invalid("invalid recorded usage auxiliary schema"));
        }
        let without: bool = connection.query_row(
            "SELECT wr FROM pragma_table_list WHERE name=?1",
            [table],
            |r| r.get(0),
        )?;
        if !without {
            return Err(invalid("recorded usage auxiliary must be without rowid"));
        }
    }
    Ok(())
}

pub(super) fn source_key(source: &SessionEventSource) -> Result<String> {
    encoded(&[
        source.root_identity.clone(),
        source.relative_path.clone(),
        source.file_device.to_string(),
        source.file_inode.to_string(),
        format!("{:032x}", source.prefix_generation),
        source.range_start.to_string(),
        source.range_end.to_string(),
        source.record_sha256.clone(),
        source.event_index.to_string(),
    ])
}

fn dirty(tx: &Transaction<'_>, model: &str, at: i64) -> Result<()> {
    let (revision, json): (String, String) = tx.query_row(
        "SELECT fact_revision,dirty_json FROM recorded_usage_state WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let revision = crate::canonical_u64_text(&revision, "recorded fact revision")?
        .checked_add(1)
        .ok_or_else(|| invalid("recorded fact revision overflow"))?;
    let mut changes: BTreeMap<String, i64> =
        serde_json::from_str(&json).map_err(|_| invalid("invalid recorded dirty head"))?;
    changes
        .entry(model.into())
        .and_modify(|v| *v = (*v).min(at))
        .or_insert(at);
    tx.execute(
        "UPDATE recorded_usage_state SET fact_revision=?1,dirty_json=?2 WHERE singleton=1",
        params![revision.to_string(), encoded(&changes)?],
    )?;
    Ok(())
}

fn reconcile_ambiguity(tx: &Transaction<'_>, key: &str) -> Result<()> {
    let mut query=tx.prepare("SELECT fact_id,model,timestamp,eligible,source_identity FROM recorded_usage_facts WHERE ambiguity_key=?1 ORDER BY fact_id")?;
    let values = query
        .query_map([key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let distinct = values.iter().map(|v| &v.4).collect::<BTreeSet<_>>().len();
    for (id, model, at, was, _) in values {
        let eligible = distinct <= 1;
        if eligible != was {
            tx.execute(
                "UPDATE recorded_usage_facts SET eligible=?2 WHERE fact_id=?1",
                params![id, eligible],
            )?;
            dirty(tx, &model, at)?;
        }
    }
    Ok(())
}

pub(super) fn remove(tx: &Transaction<'_>, source: &SessionEventSource) -> Result<()> {
    let saved: Option<(String, i64, Option<String>)> = tx
        .query_row(
            "SELECT model,timestamp,ambiguity_key FROM recorded_usage_facts WHERE source_key=?1",
            [source_key(source)?],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((model, at, key)) = saved {
        tx.execute(
            "DELETE FROM recorded_usage_facts WHERE source_key=?1",
            [source_key(source)?],
        )?;
        dirty(tx, &model, at)?;
        if let Some(key) = key {
            reconcile_ambiguity(tx, &key)?;
        }
    }
    Ok(())
}

pub(super) fn observe(tx: &Transaction<'_>, event: &LosslessSessionEvent) -> Result<()> {
    if crate::source_qualification_repair::is_retired(tx, &event.source)? {
        return remove(tx, &event.source);
    }
    let Some(admission) = &event.admission else {
        return Ok(());
    };
    if !admission.valid_for(event, event.period) {
        return Err(invalid("recorded fact has invalid source qualification"));
    }
    let model = match &admission.numeric {
        NumericEventProof::LegacyFamilyCounter { exact_model, .. } => exact_model.as_str(),
        NumericEventProof::ResponseRecord {
            legacy_family_exact_model: Some(exact_model),
            ..
        } => exact_model.as_str(),
        _ => event.model.as_str(),
    };
    if model == "UNATTRIBUTED" {
        return Ok(());
    }
    crate::validate_storage_partition_identity(tx, &admission.source_binding.partition)?;
    let key = source_key(&event.source)?;
    let identity = encoded(&(
        event.source.root_identity.as_str(),
        event.source.relative_path.as_str(),
        event.source.file_device.to_string(),
        event.source.file_inode.to_string(),
        format!("{:032x}", event.source.prefix_generation),
    ))?;
    let (fact_id, ambiguity) = match &admission.numeric {
        NumericEventProof::ResponseRecord { candidate, .. } => {
            (format!("r:{}", candidate.key), None)
        }
        NumericEventProof::LegacyCounter(proof)
        | NumericEventProof::LegacyFamilyCounter { proof, .. } => {
            // This is only an ambiguity marker. It grants no alias/rebind proof.
            // Both raw records survive until a complete-prefix owner resolves it.
            let marker = encoded(&(
                event.source.root_identity.as_str(),
                &admission.source_binding.metadata,
                model,
                event.timestamp,
                proof.before.pointer.timestamp,
                proof.before.total,
                proof.before.input,
                proof.before.cached_input,
                proof.before.cache_write_input,
                proof.before.output,
                proof.after.total,
                proof.after.input,
                proof.after.cached_input,
                proof.after.cache_write_input,
                proof.after.output,
            ))?;
            (format!("c:{}", hash(&key)), Some(hash(&marker)))
        }
    };
    let tokens = encoded(&event.tokens)?;
    let mut immutable = admission.clone();
    immutable.period = None;
    let evidence = match &admission.numeric {
        NumericEventProof::ResponseRecord { candidate, .. } => hash(&encoded(&(
            model,
            event.timestamp,
            event.tokens,
            &candidate.record.ids,
            candidate.record.usage,
            candidate.session_scope_qualified,
        ))?),
        _ => hash(&encoded(&(
            immutable,
            event.timestamp,
            &event.model,
            event.tokens,
        ))?),
    };
    let existing:Option<(String,String,i64,String,String)>=tx.query_row("SELECT source_key,model,timestamp,tokens_json,evidence_sha256 FROM recorded_usage_facts WHERE fact_id=?1",[&fact_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if let Some((old_key, stored_model, at, old_tokens, old_evidence)) = existing {
        if stored_model != model || at != event.timestamp || old_tokens != tokens {
            return Err(invalid(
                "recorded fact identity has conflicting immutable values",
            ));
        }
        if fact_id.starts_with("r:") && old_evidence != evidence {
            return Err(invalid(
                "recorded response has conflicting six-component identity",
            ));
        }
        if old_key != key {
            if !fact_id.starts_with("r:") {
                return Err(invalid("recorded counter source changed"));
            }
            return Ok(());
        }
        if old_evidence != evidence {
            tx.execute(
                "UPDATE recorded_usage_facts SET evidence_sha256=?2 WHERE fact_id=?1",
                params![fact_id, evidence],
            )?;
        }
        return Ok(());
    }
    tx.execute(
        "INSERT INTO recorded_usage_facts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1)",
        params![
            fact_id,
            key,
            event.timestamp,
            model,
            tokens,
            evidence,
            identity,
            ambiguity
        ],
    )?;
    dirty(tx, model, event.timestamp)?;
    if let Some(key) = ambiguity {
        reconcile_ambiguity(tx, &key)?;
    }
    Ok(())
}

pub(super) fn finish(tx: &Transaction<'_>) -> Result<()> {
    let initialized: bool = tx.query_row(
        "SELECT initialized FROM recorded_usage_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if !initialized {
        // One initial ledger pass. Ordinary commits below visit only their
        // changed keys and affected model suffixes, never all source records.
        for event in
            crate::source_events::read_matching(tx, "WHERE admission_json IS NOT NULL", [])?
        {
            observe(tx, &event)?;
        }
        tx.execute(
            "UPDATE recorded_usage_state SET initialized=1 WHERE singleton=1",
            [],
        )?;
    }
    let json: String = tx.query_row(
        "SELECT dirty_json FROM recorded_usage_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let changes: BTreeMap<String, i64> =
        serde_json::from_str(&json).map_err(|_| invalid("invalid recorded dirty head"))?;
    for (model, since) in changes {
        let previous:Option<String>=tx.query_row("SELECT prefix_json FROM recorded_usage_prefixes WHERE model=?1 AND timestamp<?2 ORDER BY timestamp DESC LIMIT 1",params![model,since],|r|r.get(0)).optional()?;
        let mut prefix: RecordedTokenPrefix = previous
            .map(|v| serde_json::from_str(&v).map_err(|_| invalid("invalid recorded prefix")))
            .transpose()?
            .unwrap_or_default();
        prefix
            .validate()
            .map_err(|_| invalid("invalid recorded prefix"))?;
        let mut q=tx.prepare("SELECT timestamp,tokens_json FROM recorded_usage_facts WHERE model=?1 AND timestamp>=?2 AND eligible=1 ORDER BY timestamp,fact_id")?;
        let rows = q
            .query_map(params![model, since], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tx.execute(
            "DELETE FROM recorded_usage_prefixes WHERE model=?1 AND timestamp>=?2",
            params![model, since],
        )?;
        for (at, tokens) in rows {
            let tokens =
                serde_json::from_str(&tokens).map_err(|_| invalid("invalid recorded tokens"))?;
            prefix
                .add(tokens)
                .map_err(|_| invalid("recorded prefix arithmetic failed"))?;
            let payload = encoded(&prefix)?;
            tx.execute("INSERT INTO recorded_usage_prefixes VALUES(?1,?2,?3,?4) ON CONFLICT(model,timestamp) DO UPDATE SET prefix_json=excluded.prefix_json,prefix_sha256=excluded.prefix_sha256",params![model,at,payload,hash(&payload)])?;
        }
    }
    let mut q = tx.prepare(
        "SELECT DISTINCT model FROM recorded_usage_facts WHERE eligible=1 ORDER BY model",
    )?;
    let models = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // No complete all-device/day coverage is asserted by the source ledger.
    let coverage = if models.is_empty() {
        RecordingCoverage::Unknown
    } else {
        RecordingCoverage::Partial
    };
    let coverage = match coverage {
        RecordingCoverage::Complete => "complete",
        RecordingCoverage::Partial => "partial",
        RecordingCoverage::Unknown => "unknown",
    };
    tx.execute("UPDATE recorded_usage_state SET models_json=?1,coverage=?2,dirty_json='{}' WHERE singleton=1 AND (models_json<>?1 OR coverage<>?2 OR dirty_json<>'{}')",params![encoded(&models)?,coverage])?;
    Ok(())
}
