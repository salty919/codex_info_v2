// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
//! UI-owned projections over the single shared change-driven receiver.
use super::*;
use codex_info::wire_client::{Client, Failure, Page};
use std::sync::Arc;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Desired {
    account: Option<String>,
    follow_default: bool,
    graph: bool,
    threads: bool,
    runtime: bool,
    period: Option<String>,
    refresh: u64,
}
pub(super) enum Update {
    Preparing,
    Page(String, Box<Page>),
    Failure(Failure),
}
pub(super) struct Worker {
    desired: tokio::sync::watch::Sender<Desired>,
    updates: std::sync::mpsc::Receiver<Update>,
}
impl Worker {
    pub fn start(address: SocketAddr) -> Self {
        let (desired, mut input) = tokio::sync::watch::channel(Desired::default());
        let (output, updates) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move{
    let parent=std::env::var_os("CODEX_INFO_WIRE_CACHE_PATH").map(PathBuf::from).or_else(||std::env::var_os("XDG_STATE_HOME").map(|p|PathBuf::from(p).join("codex-info/wire-receipts"))).or_else(||std::env::var_os("HOME").map(|p|PathBuf::from(p).join(".local/state/codex-info/wire-receipts")));
    let Some(path)=parent else{let _=output.send(Update::Failure(Failure::Cache("cache_path_missing".into())));return};
    if let Some(parent)=path.parent(){if let Err(error)=fs::create_dir_all(parent){let _=output.send(Update::Failure(Failure::Cache(error.to_string())));return}}
    let mut client:Option<Arc<Client>>=None;let mut reconnect=false;let mut refreshed=0;let _=output.send(Update::Preparing);
    loop{
     if client.is_none(){match Client::connect(address,&path).await{Ok(connected)=>{client=Some(connected)},Err(error)=>{debug_runtime(format!("wire connect failure: {error:?}"));if output.send(Update::Failure(error)).is_err(){return}
if input.changed().await.is_err(){return}continue}}}
     let active=client.as_ref().unwrap().clone();let mut changed=active.changes();let desired=input.borrow_and_update().clone();
     let account=desired.account.as_ref().filter(|_|!desired.follow_default).map(|id|format!("&account={id}")).unwrap_or_default();let mut targets=vec![format!("/state?parts=main{}{account}",if desired.runtime{",account_choices"}else{""})];
     if desired.runtime{targets.push("/state?parts=runtime".into())}
     if let Some(id)=desired.account.as_ref(){
      if desired.graph{targets.push(format!("/state?parts=account_context&account={id}"));targets.push(format!("/history/periods?account={id}&purpose=selectable"));if let Some(period)=desired.period.as_ref(){targets.push(format!("/history?account={id}&period={period}"));}}
      if desired.threads{targets.push(format!("/threads?account={id}"));}
     }
     if let Err(error)=active.set_targets(&targets).await{if output.send(Update::Failure(error)).is_err(){return}}
     if desired.refresh!=refreshed{refreshed=desired.refresh;if let Err(error)=active.check().await{let _=output.send(Update::Failure(error));}}
     let mut requests=tokio::task::JoinSet::new();for target in targets{let active=active.clone();requests.spawn(async move{let mut page=active.get(&target).await?;let mut seen=BTreeSet::new();while let Some(cursor)=page.data.as_ref().and_then(|data|data.next_cursor.as_ref()).cloned(){if !seen.insert(cursor.clone()){return Err(Failure::Invalid)}page=active.get(&format!("{target}&cursor={cursor}")).await?;}Ok::<_,Failure>((target,page))});}
     let mut transport_failed=false;let mut selection_changed=false;
     loop{tokio::select!{
      update=input.changed()=>{if update.is_err(){return}requests.abort_all();selection_changed=true;break},
      result=requests.join_next()=>{let Some(result)=result else{break};match result{Ok(Ok((target,page)))=>{if output.send(Update::Page(target,Box::new(page))).is_err(){return}},Ok(Err(error))=>{debug_runtime(format!("wire resource failure: {error:?}"));transport_failed|=matches!(error,Failure::Transport);if output.send(Update::Failure(error)).is_err(){return}},Err(_)=>{let _=output.send(Update::Failure(Failure::Invalid));}}}
     }}
     if selection_changed{reconnect=false;continue}
     if transport_failed&&!reconnect{client=None;reconnect=true;continue}
     if transport_failed{if input.changed().await.is_err(){return}reconnect=false;continue}
     reconnect=false;
     tokio::select!{result=input.changed()=>{if result.is_err(){return}},result=changed.changed()=>{if result.is_err(){return}}}
    }
   });
        });
        Self { desired, updates }
    }
    pub fn tick(&self, state: &mut CodexInfoState, graph: bool, threads: bool, runtime: bool) {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Update::Preparing => {
                    if !state.wire_active {
                        state.checking = true;
                        state.service_endpoint_error = None;
                        state.error = None;
                        state.status = "受領済みデータを復元しています…".into();
                    }
                }
                Update::Page(target, page) => {
                    if let Err(error) = apply(state, &target, &page) {
                        state.account_error = Some(error)
                    }
                }
                Update::Failure(Failure::Pending | Failure::Sync) => {}
                Update::Failure(Failure::Transport) => {
                    state.hold_service_endpoint_error("常駐サービスへ接続できません。".into())
                }
                Update::Failure(Failure::Cache(_)) => {
                    state.account_error = Some("受信キャッシュの復旧が必要です。".into());
                    state.service_endpoint_error = None
                }
                Update::Failure(_) => {
                    state.account_error = Some("常駐サービスの応答を確認できません。".into());
                    state.service_endpoint_error = None
                }
            }
        }
        let account = state.service_selected_account_id.clone();
        let period = requested_period(
            &state.service_history_periods,
            state
                .service_history_pending_reset_at
                .or(state.selected_reset_at),
        )
        .map(|p| p.id.clone());
        let refresh = self
            .desired
            .borrow()
            .refresh
            .wrapping_add(u64::from(state.service_current_force_poll));
        state.service_current_force_poll = false;
        let next = Desired {
            account,
            follow_default: state
                .selected_service_account()
                .is_none_or(|account| account.is_current),
            graph,
            threads,
            runtime,
            period,
            refresh,
        };
        if *self.desired.borrow() != next {
            self.desired.send_replace(next);
        }
    }
}
// The graph's loading decision and its request must select the same period.
// A retained reset may have disappeared after account/image resynchronization.
fn requested_period(
    periods: &[PublicHistoryPeriod],
    selected: Option<i64>,
) -> Option<&PublicHistoryPeriod> {
    selected
        .and_then(|reset| select_public_history_period(periods, reset))
        .or_else(|| periods.iter().find(|p| p.current))
        .or_else(|| periods.first())
}

fn rows(page: &Page) -> impl Iterator<Item = (&Value, &Value)> {
    page.views
        .values()
        .flat_map(|(_, rows)| rows.iter().map(|(k, v)| (k, v)))
}
fn image(page: &Page, kind: &str) -> Option<String> {
    page.views
        .values()
        .find(|(_, rows)| rows.iter().any(|(key, _)| key[0] == kind))
        .map(|(image, _)| image.clone())
}
fn convert<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn summary(value: &Value, field: &str) -> ActiveThreadSummary {
    let mut result = ActiveThreadSummary::default();
    if let Some(models) = value["by_model"].as_array() {
        for model in models {
            let count = model[field].as_u64().unwrap_or(0).min(i32::MAX as u64) as i32;
            let bucket = match classify_active_thread_model(model["model"].as_str().unwrap_or("")) {
                "SOL" => &mut result.sol,
                "TERRA" => &mut result.terra,
                "LUNA" => &mut result.luna,
                "ASTRA" => &mut result.astra,
                _ => &mut result.other,
            };
            *bucket = bucket.saturating_add(count);
            result.total = result.total.saturating_add(count)
        }
    }
    result
}
fn apply(state: &mut CodexInfoState, target: &str, page: &Page) -> Result<(), String> {
    if target.starts_with("/history") || target.starts_with("/threads") {
        let requested = target.split('&').find_map(|part| {
            part.strip_prefix("account=")
                .or_else(|| part.split_once("?account=").map(|(_, id)| id))
        });
        if requested != state.service_selected_account_id.as_deref() {
            return Ok(());
        }
    }
    if let Some((_, choices)) = rows(page).find(|(key, _)| key[0] == "account_choices") {
        let default = choices["default_account_id"].as_str().map(str::to_owned);
        let old = state.service_selected_account_id.clone();
        let followed = old == state.service_default_account_id;
        let accounts = choices["accounts"]
            .as_array()
            .ok_or("invalid account choices")?
            .iter()
            .map(|value| {
                let id = value["id"].as_str().ok_or("invalid account id")?;
                let old = state.service_accounts.iter().find(|a| a.id == id);
                Ok(ServiceAccountV3 {
                    id: id.into(),
                    login_id: value["login_id"].as_str().map(str::to_owned),
                    is_current: value["is_current"]
                        .as_bool()
                        .ok_or("invalid current flag")?,
                    activation_at: old.and_then(|a| a.activation_at),
                    deactivation_at: old.and_then(|a| a.deactivation_at),
                    ownership_intervals: old
                        .map(|a| a.ownership_intervals.clone())
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let selected = if followed {
            default.clone()
        } else {
            old.filter(|id| accounts.iter().any(|a| a.id == *id))
                .or_else(|| default.clone())
        };
        let changed = selected != state.service_selected_account_id;
        state.service_accounts = accounts;
        state.service_default_account_id = default;
        state.service_accounts_supported = true;
        state.service_accounts_known = true;
        state.service_accounts_error = None;
        if changed {
            state.service_selected_account_id = selected;
            state.clear_selected_account_state();
        }
    }
    if !rows(page).any(|(key, _)| key[0] == "account_choices") {
        if let Some((_, main)) = rows(page).find(|(key, _)| key[0] == "main") {
            let selection = &main["selection"];
            if selection.is_null()
                && state.service_selected_account_id == state.service_default_account_id
            {
                state.service_default_account_id = None;
                state.service_selected_account_id = None;
                for account in &mut state.service_accounts {
                    account.is_current = false;
                }
                state.clear_selected_account_state();
            }
            if selection["is_current"].as_bool() == Some(true) {
                if let Some(id) = selection["account_id"].as_str() {
                    let followed = state.service_selected_account_id.is_none()
                        || state.service_selected_account_id == state.service_default_account_id;
                    for account in &mut state.service_accounts {
                        account.is_current = account.id == id;
                    }
                    if !state
                        .service_accounts
                        .iter()
                        .any(|account| account.id == id)
                    {
                        state.service_accounts.push(ServiceAccountV3 {
                            id: id.into(),
                            is_current: true,
                            activation_at: None,
                            deactivation_at: None,
                            ownership_intervals: vec![],
                            login_id: selection["login_id"].as_str().map(str::to_owned),
                        });
                    }
                    state.service_default_account_id = Some(id.into());
                    state.service_accounts_supported = true;
                    state.service_accounts_known = true;
                    if followed && state.service_selected_account_id.as_deref() != Some(id) {
                        state.service_selected_account_id = Some(id.into());
                        state.clear_selected_account_state();
                    }
                }
            }
        }
    }
    if let Some((_, main)) = rows(page).find(|(key, _)| key[0] == "main") {
        let selected = main["selection"]["account_id"].as_str();
        if state
            .service_selected_account_id
            .as_deref()
            .is_some_and(|id| Some(id) != selected)
        {
            return Ok(());
        }
        let main_image = image(page, "main").ok_or("missing Main image")?;
        // Successful metadata reconciliation can confirm this exact image
        // without retransferring its body. It also recovers transport state.
        state.service_endpoint_error = None;
        if state.service_current_pair.as_deref() != Some(&main_image) {
            let mut models = main["models"].clone();
            if let Some(models) = models.as_array_mut() {
                for model in models {
                    if let Some(cost) = model["estimated_cost"].as_object_mut() {
                        if let Some(price) = cost.remove("price_revision") {
                            cost.insert("price_version".into(), price);
                        }
                    }
                }
            }
            let models: Vec<PublicModelUsageV3> = convert(models)?;
            let public_state: PublicState = convert(main["acquisition_state"].clone())?;
            let quota: Option<PublicQuota> = convert(main["quota"].clone())?;
            state.wire_active = true;
            state.wire_summary = (!main["thread_summary"].is_null()).then(|| {
                (
                    summary(&main["thread_summary"], "open_count"),
                    summary(&main["thread_summary"], "running_count"),
                )
            });
            state.authenticated = main["authenticated"]
                .as_bool()
                .ok_or("invalid authentication state")?;
            state.plan_label = main["plan_label"].as_str().unwrap_or("").into();
            state.remaining_percent = quota.as_ref().map(|q| q.remaining_percent);
            state.has_quota_percent = quota.is_some();
            state.reset_at = quota.as_ref().map(|q| q.reset_at);
            state.window_seconds = quota.as_ref().map_or(WEEK_SECONDS, |q| q.window_seconds);
            state.monthly = quota.as_ref().is_some_and(|q| q.monthly);
            state.limit_name = "Codex".into();
            state.quota_title = if state.monthly {
                "月間利用枠"
            } else {
                "残り利用枠"
            }
            .into();
            state.last_success_at = main["observed_at"].as_i64();
            state.has_usage = state.authenticated
                && state.last_success_at.is_some()
                && matches!(public_state, PublicState::Ready | PublicState::Error);
            state.usage_snapshot_committed = state.has_usage;
            state.local_usage_pending = false;
            state.model_usage = models
                .iter()
                .map(|model| ModelUsageRow {
                    name: model.model.clone(),
                    tokens: model.total_tokens,
                    input_tokens: model.input_tokens,
                    cached_input_tokens: model.cached_input_tokens,
                    output_tokens: model.output_tokens,
                    cache_write_input_tokens: model.cache_write_input_tokens,
                    server_estimated_cost: model.estimated_cost.clone(),
                    server_pricing_authoritative: true,
                })
                .collect();
            state.estimated_cost_label = estimated_cost_label_from_v3(&models);
            state.checking = public_state == PublicState::Initializing;
            state.service_current_models_status = Some(convert(main["models_status"].clone())?);
            state.service_current_active_thread_count =
                main["thread_summary"]["open_count"].as_u64();
            state.account_error = (public_state == PublicState::Error)
                .then(|| "常駐サービスが取得状態を報告しました。".into());
            state.service_endpoint_error = None;
            state.service_current_pair = Some(main_image.clone());
            state.service_published_pair = Some(main_image.clone());
            state.service_v3_published_pair = Some(main_image.clone());
            state.service_split_capable = true;
            state.service_current_snapshot = Some(PublicDetailsV3 {
                state: public_state,
                observed_at: state.last_success_at,
                authenticated: state.authenticated,
                plan_label: Some(state.plan_label.clone()),
                quota,
                models,
                active_thread_count: main["thread_summary"]["running_count"]
                    .as_u64()
                    .unwrap_or(0),
                open_session_thread_count: main["thread_summary"]["open_count"]
                    .as_u64()
                    .unwrap_or(0),
                history_periods: vec![],
                history_samples: vec![],
                history_gaps: vec![],
                threads: vec![],
            });
            if state.selected_reset_at.is_none() {
                state.selected_reset_at = state.reset_at;
            }
            state.status = match public_state {
                PublicState::Initializing => "常駐サービスが利用状況を取得しています…",
                PublicState::Ready => "利用状況を更新しました。",
                PublicState::AuthRequired => "未認証です。認証を開始してください。",
                PublicState::Error => "常駐サービスが利用状況を取得できませんでした。",
            }
            .into();
        }
    }
    for (_, context) in rows(page).filter(|(key, _)| key[0] == "account_context") {
        if let Some(account) = state
            .service_accounts
            .iter_mut()
            .find(|a| Some(a.id.as_str()) == context["id"].as_str())
        {
            account.activation_at = context["activation_at"].as_i64();
            account.deactivation_at = context["deactivation_at"].as_i64();
            account.ownership_intervals = convert(context["ownership_intervals"].clone())?;
        }
    }
    if let Some((_, runtime)) = rows(page).find(|(key, _)| key[0] == "runtime") {
        state.wire_runtime = Some(codex_info_rest_contract::PublicRuntimeVersions {
            api_version: String::new(),
            rest_version: runtime["rest_product_version"]
                .as_str()
                .ok_or("invalid runtime")?
                .into(),
            recorder_version: runtime["recorder_product_version"]
                .as_str()
                .map(str::to_owned),
            recorder_status: runtime["recorder_status"]
                .as_str()
                .ok_or("invalid runtime state")?
                .into(),
        });
    }
    if target.starts_with("/history/periods?") {
        let mut periods = rows(page)
            .filter(|(key, _)| key[0] == "period")
            .map(|(key, value)| {
                let mut value = value.clone();
                value
                    .as_object_mut()
                    .ok_or("invalid period")?
                    .remove("origin_at");
                value["id"] = key[1].clone();
                convert(value)
            })
            .collect::<Result<Vec<PublicHistoryPeriod>, String>>()?;
        periods.sort_by(|a, b| a.reset_at.cmp(&b.reset_at).then_with(|| a.id.cmp(&b.id)));
        state.service_history_periods = periods;
        state.service_history_periods_pair =
            page.views.values().next().map(|(image, _)| image.clone());
        state.service_history_periods_selectable = true;
    }
    if target.starts_with("/history?") {
        let mut samples = Vec::new();
        let mut gaps = Vec::new();
        let mut local = Vec::new();
        let mut period = target
            .split('&')
            .find_map(|part| part.strip_prefix("period="))
            .map(str::to_owned);
        for (key, value) in rows(page) {
            match key[0].as_str() {
                Some("sample") => {
                    let mut value = value.clone();
                    value["reset_at"] = key[2].clone();
                    value["timestamp"] = key[3].clone();
                    samples.push(convert::<PublicHistoryObservationV3>(value)?);
                    period = key[1].as_str().map(str::to_owned)
                }
                Some("gap") => {
                    let mut value = value.clone();
                    value["gap_id"] = key[2].clone();
                    gaps.push(convert::<PublicHistoryGap>(value)?)
                }
                Some("local_usage") => {
                    let mut value = value.clone();
                    value["version"] = json!(1);
                    local.push(convert::<
                        codex_info_rest_contract::recorded_usage::LocalUsage,
                    >(value)?)
                }
                _ => {}
            }
        }
        samples.sort_by_key(|s| (s.reset_at, s.timestamp));
        state.history.samples = samples
            .iter()
            .filter_map(main_sample_from_public_observation_v3)
            .collect();
        state.history.observations = samples
            .iter()
            .map(store_observation_from_public_v3)
            .collect();
        state.service_history_samples = samples;
        state.history_gaps = gaps;
        state.service_history_local_usage = local;
        state.service_history_pair = page.views.values().next().map(|(image, _)| image.clone());
        if let Some(selected) = state
            .service_history_periods
            .iter()
            .find(|p| Some(p.id.as_str()) == period.as_deref())
        {
            state.selected_reset_at = Some(selected.reset_at);
            state.selected_history_period = selected.label.clone();
        }
        state.service_history_period_id = period;
        state.service_history_pending_reset_at = None;
        state.service_history_error = None;
        state.service_history_cursor = None;
        state.service_history_cursor_reset_required = false;
    }
    if target.starts_with("/threads?") {
        let threads = rows(page)
            .filter(|(key, _)| key[0] == "thread")
            .map(|(key, value)| {
                let mut value = value.clone();
                value["id"] = key[1].clone();
                convert::<PublicThread>(value)
            })
            .collect::<Result<Vec<_>, String>>()?;
        state.active_threads = threads
            .iter()
            .map(|thread| ActiveThread {
                id: thread.id.clone(),
                activity_status: thread.activity_status.unwrap_or_default(),
                created_at: thread.created_at,
                updated_at: thread
                    .last_user_message_at
                    .or(thread.created_at)
                    .unwrap_or(1),
                title: thread.title.clone(),
                model: thread.model.clone(),
                model_label: thread.model_label.clone(),
                total_tokens: thread.total_tokens,
                context_usage_tokens: thread.context_usage_tokens,
                context_window_tokens: thread.context_window_tokens,
                last_user_message_at: thread.last_user_message_at,
                is_subagent: thread.is_subagent,
                parent_thread_id: thread.parent_thread_id.clone(),
                depth: thread.depth,
            })
            .collect();
        state.service_threads_pair = page.views.values().next().map(|(image, _)| image.clone());
        state.service_threads_error = None;
        state.thread_error = false;
        state.thread_checking = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaced_period_selection_requests_the_same_current_period_as_loading() {
        let periods = vec![PublicHistoryPeriod {
            id: "opaque-period".into(),
            start_at: 10,
            end_at: 20000,
            reset_at: 20000,
            label: "current".into(),
            current: true,
            origin_at: Some(10),
        }];
        assert_eq!(
            requested_period(&periods, Some(1000)).map(|p| p.id.as_str()),
            Some("opaque-period")
        );
        assert_eq!(requested_period(&[], Some(1000)), None);
    }
}
