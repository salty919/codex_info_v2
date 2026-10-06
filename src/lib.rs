// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
// Rebuild the release after the publication workflow repair (Refs #535).

#![deny(unsafe_code)]

/// Distribution version compiled into each process that uses this crate.
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod app_server_sqlite;
pub mod i18n;
pub mod protocol_contract;
pub mod security;
pub mod server;
pub mod thread_contract;
pub mod thread_state;
pub mod usage_store;
