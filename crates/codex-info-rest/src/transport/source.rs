use super::*;
use wire::sync::{hex, Catalog};

#[derive(Clone)]
pub(super) struct Row {
    pub key: Value,
    pub id: u64,
}
#[derive(Clone)]
pub(super) struct View {
    pub account: Option<String>,
    pub image: String,
    pub rows: Vec<Row>,
    pub definitions: Vec<u64>,
    pub tree: wire::manifest::Tree,
    pub row_ids: BTreeMap<Vec<u8>, u64>,
}
pub(super) struct Source {
    pub views: BTreeMap<Vec<u8>, Arc<View>>,
    pub catalog: Catalog,
}
impl Source {
    pub fn empty() -> Self {
        Self {
            views: BTreeMap::new(),
            catalog: Catalog::with_first_id(random_seed()),
        }
    }
    pub fn build(store: &SnapshotStore, previous: &Self) -> Result<Self, wire::WireError> {
        let mut catalog = previous.catalog.clone();
        let mut views = BTreeMap::new();
        let (accounts, _) = store.capture_accounts();
        let mut add = |target: &str,
                       part: Option<Projection>,
                       account: Option<String>,
                       rows: Vec<(Value, &str, Value)>|
         -> Result<(), wire::WireError> {
            let target = Target::parse(target)?.ok_or(wire::WireError("source target"))?;
            let mut prepared = Vec::with_capacity(rows.len());
            let mut roots = Vec::new();
            let mut entries = Vec::new();
            for (key, kind, value) in rows {
                let id = catalog.intern(kind, &value)?;
                entries.push((wire::canonical_json(None, &key)?, catalog.digests[&id]));
                roots.push(id);
                prepared.push(Row { key, id })
            }
            let mut tree = wire::manifest::Tree::default();
            for (key, digest) in &entries {
                tree.insert(key.clone(), *digest)?;
            }
            let image = hex(&tree.root());
            let key = target.view_key(part)?;
            if let Some(old) = previous
                .views
                .get(&key)
                .filter(|v| v.image == image && v.account == account)
            {
                views.insert(key, Arc::clone(old));
                return Ok(());
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut definitions = Vec::new();
            for root in roots {
                for d in catalog.transfer(root, &seen)? {
                    seen.insert(d.id);
                    definitions.push(d.id)
                }
            }
            views.insert(
                key,
                Arc::new(View {
                    account,
                    image,
                    row_ids: prepared
                        .iter()
                        .map(|row| (wire::canonical_json(None, &row.key).unwrap(), row.id))
                        .collect(),
                    rows: prepared,
                    definitions,
                    tree,
                }),
            );
            Ok(())
        };
        for explicit in
            std::iter::once(None).chain(accounts.accounts.iter().map(|a| Some(a.id.as_str())))
        {
            let (snapshot, unavailable, failed, _) =
                store.capture_resource(explicit, Route::CurrentV4);
            let account = explicit
                .or(accounts.default_account_id.as_deref())
                .and_then(|id| accounts.accounts.iter().find(|a| a.id == id));
            let suffix = explicit
                .map(|id| format!("&account={id}"))
                .unwrap_or_default();
            if let Some(snapshot) = snapshot.filter(|_| !unavailable) {
                add(
                    &format!("/state?parts=main{suffix}"),
                    Some(Projection::Main),
                    account.map(|a| a.id.clone()),
                    vec![(
                        json!(["main"]),
                        "main",
                        main_value(&snapshot, account, failed),
                    )],
                )?;
                if let Some(account) = account {
                    add(
                        &format!("/state?parts=account_context{suffix}"),
                        Some(Projection::AccountContext),
                        Some(account.id.clone()),
                        vec![(
                            json!(["account_context", account.id]),
                            "account_context",
                            json!({"id":account.id,"activation_at":account.activation_at,"deactivation_at":account.deactivation_at,"ownership_intervals":account.ownership_intervals}),
                        )],
                    )?;
                }
                if let Some(id) = explicit {
                    if !snapshot.thread_snapshot_known
                        || !snapshot.details.authenticated
                        || failed
                        || snapshot.acquisition_degraded
                    {
                        continue;
                    }
                    let mut rows = Vec::new();
                    {
                        for thread in visible_threads(&snapshot.details.threads) {
                            rows.push((
                                json!(["thread", thread.id]),
                                "thread",
                                thread_value(thread),
                            ));
                        }
                    }

                    add(
                        &format!("/threads?account={id}"),
                        None,
                        Some(id.into()),
                        rows,
                    )?;
                }
            }
        }
        add(
            "/state?parts=account_choices",
            Some(Projection::AccountChoices),
            None,
            vec![(
                json!(["account_choices"]),
                "account_choices",
                json!({"default_account_id":accounts.default_account_id,"accounts":accounts.accounts.iter().map(|a|json!({"id":a.id,"login_id":a.login_id,"is_current":a.is_current})).collect::<Vec<_>>()}),
            )],
        )?;
        let runtime = store.runtime_versions();
        add(
            "/state?parts=runtime",
            Some(Projection::Runtime),
            None,
            vec![(
                json!(["runtime"]),
                "runtime",
                json!({"rest_product_version":runtime.rest_version,"recorder_product_version":runtime.recorder_version,"recorder_status":runtime.recorder_status}),
            )],
        )?;
        for account in &accounts.accounts {
            let (snapshot, unavailable, _, _) =
                store.capture_resource(Some(&account.id), Route::HistoryV3);
            let Some(snapshot) = snapshot.filter(|s| !unavailable && s.history_available) else {
                continue;
            };
            for (index, purpose) in [(0, "window"), (1, "selectable")] {
                let mut rows = Vec::new();
                for &row in &snapshot.directory_indices[index] {
                    let p = &snapshot.retained_history.periods[row];
                    let mut value = serde_json::to_value(p).unwrap();
                    value["origin_at"] = json!(snapshot.retained_history.origins.get(&p.reset_at));
                    rows.push((json!(["period", p.id]), "period", value));
                }
                let target = if purpose == "window" {
                    format!(
                        "/history/periods?account={}&purpose=window&from=1&until=253402300800",
                        account.id
                    )
                } else {
                    format!("/history/periods?account={}&purpose=selectable", account.id)
                };
                add(&target, None, Some(account.id.clone()), rows)?;
            }
            for archive in std::iter::once(&snapshot.retained_history)
                .chain(snapshot.retained_history.raw.as_deref())
            {
                for period in &archive.periods {
                    let Ok((archive, index, _)) =
                        history_selection(&snapshot, Some(&period.id), None)
                    else {
                        continue;
                    };
                    let mut rows = Vec::new();
                    for &row in &index.samples {
                        let sample = &archive.samples[row];
                        let mut value = serde_json::to_value(sample).unwrap();
                        let object = value.as_object_mut().unwrap();
                        object.remove("timestamp");
                        object.remove("reset_at");
                        rows.push((
                            json!(["sample", period.id, sample.reset_at, sample.timestamp]),
                            "sample",
                            value,
                        ));
                    }
                    for &row in &index.gaps {
                        let gap = &archive.gaps[row];
                        let mut value = serde_json::to_value(gap).unwrap();
                        value.as_object_mut().unwrap().remove("gap_id");
                        rows.push((json!(["gap", period.id, gap.gap_id]), "gap", value));
                    }
                    let end = period.end_at.saturating_add(1);
                    let evidence = snapshot
                        .local_recordings
                        .as_ref()
                        .and_then(|r| r.local_usage(period.start_at, end).ok())
                        .unwrap_or_else(|| {
                            codex_info_rest_contract::recorded_usage::LocalUsage::unavailable(
                                period.start_at,
                                end,
                            )
                        });
                    let mut value = serde_json::to_value(evidence).unwrap();
                    value.as_object_mut().unwrap().remove("version");
                    rows.push((json!(["local_usage", period.id]), "local_usage", value));
                    add(
                        &format!("/history?account={}&period={}", account.id, period.id),
                        None,
                        Some(account.id.clone()),
                        rows,
                    )?;
                }
            }
        }
        Ok(Self { views, catalog })
    }
    pub fn selected(&self, target: &Target) -> Option<Vec<(Vec<u8>, Arc<View>)>> {
        if target.resource == Resource::State {
            return target
                .parts
                .iter()
                .map(|part| {
                    let key = target.view_key(Some(*part)).ok()?;
                    Some((key.clone(), Arc::clone(self.views.get(&key)?)))
                })
                .collect();
        }
        let key = target.view_key(None).ok()?;
        if let Some(view) = self.views.get(&key) {
            return Some(vec![(key, Arc::clone(view))]);
        }
        None
    }
    pub fn prepare_window(&mut self, target: &Target) {
        if target.resource == Resource::Periods
            && target.purpose == Some(wire::request::PeriodPurpose::Window)
        {
            let Ok(key) = target.view_key(None) else {
                return;
            };
            let mut all = target.clone();
            all.from = Some(1);
            all.until = Some(253_402_300_800);
            let Ok(all_key) = all.view_key(None) else {
                return;
            };
            let Some(view) = self.views.get(&all_key) else {
                return;
            };
            // The immutable directory index is prepared outside the handler; the
            // requested range is selected here without decoding source records.
            let rows = view
                .rows
                .iter()
                .filter(|row| {
                    let value = &self.catalog.definitions[&row.id];
                    let get = |field: &str| {
                        value
                            .value
                            .get(field)
                            .and_then(|r| r.get("ref"))
                            .and_then(Value::as_u64)
                            .and_then(|id| self.catalog.definitions[&id].value.as_i64())
                    };
                    get("start_at") < target.until && get("end_at") >= target.from
                })
                .cloned()
                .collect::<Vec<_>>();
            let entries = rows
                .iter()
                .map(|row| {
                    Ok((
                        wire::canonical_json(None, &row.key)?,
                        self.catalog.digests[&row.id],
                    ))
                })
                .collect::<Result<Vec<_>, wire::WireError>>();
            let Ok(entries) = entries else { return };
            let mut tree = wire::manifest::Tree::default();
            for (key, digest) in &entries {
                tree.insert(key.clone(), *digest).unwrap();
            }
            let row_ids = rows
                .iter()
                .map(|row| (wire::canonical_json(None, &row.key).unwrap(), row.id))
                .collect();
            let mut filtered = View {
                account: view.account.clone(),
                image: hex(&tree.root()),
                rows,
                definitions: Vec::new(),
                tree,
                row_ids,
            };
            let mut seen = std::collections::BTreeSet::new();
            for row in &filtered.rows {
                for definition in self.catalog.transfer(row.id, &seen).unwrap() {
                    seen.insert(definition.id);
                    filtered.definitions.push(definition.id)
                }
            }
            self.views.insert(key, Arc::new(filtered));
        }
    }
}

fn random_seed() -> u64 {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).expect("wire identity entropy");
    u64::from_le_bytes(bytes) & 0x3fff_ffff_ffff_ffff
}
