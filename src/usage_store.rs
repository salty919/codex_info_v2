// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

// The writer implementation lives in the recorder-only dependency crate.
// The Linux UI keeps this re-export temporarily for its read-side types while
// the release artifact itself runs only as an HTTP client.
pub use codex_info_db_writer::*;
