// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
//! Test-only headless consumer of the same qualified receiver as the Unix UI.
//! This example adds no product endpoint or installed CLI operation.
use codex_info::wire_client::{Client, Failure, Page};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut values = BTreeMap::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(key) = arguments.next() {
        if !key.starts_with("--") || values.contains_key(&key) {
            return Err("invalid probe argument".into());
        }
        let value = arguments.next().ok_or("probe argument value missing")?;
        values.insert(key, value);
    }
    for key in values.keys() {
        if ![
            "--port",
            "--cache",
            "--target",
            "--wait-ms",
            "--ready",
            "--min-model-total",
            "--sample-total",
            "--sample-source",
            "--sample-model",
            "--samples",
        ]
        .contains(&key.as_str())
        {
            return Err("unknown probe argument".into());
        }
    }
    let port = values
        .get("--port")
        .ok_or("port required")?
        .parse::<u16>()?;
    if port == 0 {
        return Err("invalid port".into());
    }
    let path = PathBuf::from(values.get("--cache").ok_or("owned cache required")?);
    if !path.is_absolute() {
        return Err("absolute owned cache required".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let target = values.get("--target").ok_or("UI target required")?.clone();
    codex_info_rest_contract::wire::request::Target::parse(&target)?.ok_or("unknown UI target")?;
    let wait = values
        .get("--wait-ms")
        .map(|v| v.parse::<u64>())
        .transpose()?
        .unwrap_or(10_000);
    if !(1..=30_000).contains(&wait) {
        return Err("invalid finite operation budget".into());
    }
    let minimum = values
        .get("--min-model-total")
        .map(|v| v.parse::<u64>())
        .transpose()?;
    let sample_total = values
        .get("--sample-total")
        .map(|v| v.parse::<u64>())
        .transpose()?;
    let sample_source = values.get("--sample-source");
    let sample_model = values
        .get("--sample-model")
        .map(String::as_str)
        .unwrap_or("gpt-5.6-luna");
    let ready = values.get("--ready").is_some_and(|v| v == "true");
    let samples = values.get("--samples").is_some_and(|v| v == "true");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result=runtime.block_on(async{tokio::time::timeout(Duration::from_millis(wait),async{
        let client=Client::connect(format!("127.0.0.1:{port}").parse().unwrap(),&path).await?;client.set_targets(std::slice::from_ref(&target)).await?;let mut changes=client.changes();
        loop{
            let page=async{
                let mut page=client.get(&target).await?;let mut seen=BTreeSet::new();
                while let Some(cursor)=page.data.as_ref().and_then(|d|d.next_cursor.as_ref()).cloned(){if !seen.insert(cursor.clone()){return Err(Failure::Invalid)}page=client.get(&format!("{target}&cursor={cursor}")).await?;}
                Ok::<Page,Failure>(page)
            }.await;
            match page{
                Ok(page)=>{
                    let rows=page.views.values().flat_map(|(_,rows)|rows).collect::<Vec<_>>();
                    let main=rows.iter().find(|(key,_)|key[0]=="main").map(|(_,value)|value);
                    let ready_ok=!ready||main.is_some_and(|value|value["acquisition_state"]=="ready"&&value["authenticated"]==true);
                    let minimum_ok=minimum.is_none_or(|minimum|main.and_then(|v|v["models"].as_array()).is_some_and(|models|models.iter().filter_map(|model|model["total_tokens"].as_u64()).map(u128::from).sum::<u128>()>=u128::from(minimum)));
                    let sample_rows=rows.iter().filter(|(key,_)|key[0]=="sample").map(|(_,value)|value).collect::<Vec<_>>();
                    let samples_ok=!samples||!sample_rows.is_empty();
                    let sample_ok=sample_total.is_none_or(|total|sample_rows.iter().any(|value|sample_source.is_none_or(|source|value["model_source"]==source.as_str())&&value["models"].as_array().is_some_and(|models|models.iter().any(|model|model["model"]==sample_model&&model["total_tokens"].as_u64()==Some(total)))));
                    if ready_ok&&minimum_ok&&samples_ok&&sample_ok{
                        let receiver=client.receiver();let views=page.views.iter().map(|(id,(image,rows))|json!({"view_key":receiver.view_keys.get(id),"image":image,"rows":rows.iter().map(|(key,value)|json!({"key":key,"value":value})).collect::<Vec<_>>()})).collect::<Vec<_>>();
                        return Ok(json!({"views":views}));
                    }
                },Err(Failure::Pending|Failure::Sync)=>{},Err(error)=>return Err(error),
            }
            changes.changed().await.map_err(|_|Failure::Transport)?;
        }
    }).await});
    match result {
        Ok(Ok(value)) => {
            println!("{value}");
            Ok(())
        }
        Ok(Err(error)) => {
            Err(std::io::Error::other(format!("wire probe failed: {error:?}")).into())
        }
        Err(_) => {
            Err(std::io::Error::other("wire probe finite operation deadline exceeded").into())
        }
    }
}
