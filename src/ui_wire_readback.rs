// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
//! Installer/launcher readback uses the same UI API and a durable receiver.
use codex_info::wire_client::{Client, Failure};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
pub(super) fn run(address: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var_os("CODEX_INFO_WIRE_READBACK_CACHE_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_STATE_HOME")
                .map(|p| PathBuf::from(p).join("codex-info/readback-receipts"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".local/state/codex-info/readback-receipts"))
        })
        .ok_or("readback cache path unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let budget = std::env::var("CODEX_INFO_WIRE_READBACK_BUDGET_MS")
        .ok()
        .map(|raw| raw.parse::<u64>())
        .transpose()?
        .unwrap_or(3000);
    if !(1..=30000).contains(&budget) {
        return Err("invalid readback operation budget".into());
    }
    let result = runtime.block_on(async {
        tokio::time::timeout(Duration::from_millis(budget), async {
            let client = Client::connect(address, &path).await?;
            let targets = ["/state?parts=main,runtime".to_owned()];
            client.set_targets(&targets).await?;
            let mut changes = client.changes();
            let mut page =
                loop {
                    match client.get(&targets[0]).await {
                        Ok(page) => {
                            let initializing = page.views.values().flat_map(|(_, rows)| rows).any(
                                |(key, value)| {
                                    key[0] == "main" && value["acquisition_state"] == "initializing"
                                },
                            );
                            if !initializing {
                                break page;
                            }
                            changes.changed().await.map_err(|_| Failure::Transport)?;
                        }
                        Err(Failure::Pending | Failure::Sync) => {
                            changes.changed().await.map_err(|_| Failure::Transport)?
                        }
                        Err(error) => return Err(error),
                    }
                };
            let mut seen = std::collections::BTreeSet::new();
            while let Some(cursor) = page
                .data
                .as_ref()
                .and_then(|data| data.next_cursor.as_ref())
                .cloned()
            {
                if !seen.insert(cursor.clone()) {
                    return Err(Failure::Invalid);
                }
                page = client
                    .get(&format!("{}&cursor={cursor}", targets[0]))
                    .await?;
            }
            let mut result = serde_json::Map::new();
            for (_, rows) in page.views.values() {
                for (key, value) in rows {
                    if let Some(name) = key[0]
                        .as_str()
                        .filter(|name| matches!(*name, "main" | "runtime"))
                    {
                        result.insert(name.into(), value.clone());
                    }
                }
            }
            if result.len() != 2 {
                return Err(Failure::Pending);
            }
            Ok(serde_json::Value::Object(result))
        })
        .await
    });
    match result {
        Ok(Ok(value)) => {
            println!("{value}");
            Ok(())
        }
        Ok(Err(error)) => Err(std::io::Error::other(format!("UI readback: {error:?}")).into()),
        Err(_) => Err(std::io::Error::other("UI readback deadline exceeded").into()),
    }
}
