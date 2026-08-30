// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

#![deny(unsafe_code)]

mod daemon;

use chrono::{DateTime, Months, Utc};
use codex_info::i18n::{CliTextKey, I18n, PeriodKind, TextKey};
use codex_info::protocol_contract;
use codex_info::security;
use codex_info::server::{
    validate_public_threads, ApiServer, ApiServerConfig, PublicDetailedModelUsage, PublicDetails,
    PublicHistoryPeriod, PublicHistorySample, PublicModelUsage, PublicQuota, PublicSnapshot,
    PublicState, PublicThread,
};
use codex_info::thread_contract::{
    self, PageAcceptance, ThreadCycleAccumulator, ThreadCycleOutcome, ThreadTopologyNode,
    ValidatedThreadCandidate,
};
use codex_info::thread_state;
use codex_info::usage_store::{self, UsageStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{CloseRequestResponse, ComponentHandle, Model, Timer, TimerMode};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

slint::include_modules!();

#[derive(Clone, Copy)]
enum AccountCommand {
    Read,
    Login,
    Stop,
}

#[derive(Clone, Copy)]
enum ThreadCommand {
    Read { auth_epoch: u64 },
    Stop,
}

#[derive(Clone, Copy)]
enum LocalCommand {
    Collect {
        auth_epoch: u64,
        reset_at: i64,
        window_seconds: i64,
    },
    Stop,
}

struct UsageEvent {
    remaining_percent: Option<f64>,
    reset_at: i64,
    window_seconds: i64,
    limit_name: String,
    quota_title: String,
    monthly: bool,
}

enum Event {
    Ready,
    Account {
        email: Option<String>,
        authenticated: bool,
        plan_type: Option<String>,
    },
    AuthUrl(String),
    Usage(Box<UsageEvent>),
    Error(String),
}

enum ThreadEvent {
    Ready,
    Update {
        auth_epoch: u64,
        update: ActiveThreadUpdate,
    },
    Error {
        auth_epoch: u64,
        message: String,
    },
}

struct LocalUsageResult {
    auth_epoch: u64,
    reset_at: i64,
    window_seconds: i64,
    model_usage: ModelUsageTotals,
    history_samples: Vec<UsageHistorySample>,
}

enum LocalEvent {
    Usage(LocalUsageResult),
    Error {
        auth_epoch: u64,
        reset_at: i64,
        window_seconds: i64,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct TokenSnapshot {
    total: u64,
    input: u64,
    cached_input: u64,
    output: u64,
}

const LOCAL_ESTIMATE_PRICE_VERSION: &str = "LOCAL_ESTIMATE_V1_2026-08-14";
const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
const SOL_PRICE_PER_MILLION: (f64, f64, f64) = (5.0, 0.5, 30.0);
const TERRA_PRICE_PER_MILLION: (f64, f64, f64) = (2.0, 0.2, 12.0);
const LUNA_PRICE_PER_MILLION: (f64, f64, f64) = (0.2, 0.02, 1.2);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ModelUsageRow {
    name: String,
    tokens: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

impl ModelUsageRow {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    fn add(&mut self, snapshot: TokenSnapshot) {
        self.tokens = self.tokens.saturating_add(snapshot.total);
        self.input_tokens = self.input_tokens.saturating_add(snapshot.input);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(snapshot.cached_input);
        self.output_tokens = self.output_tokens.saturating_add(snapshot.output);
    }

    fn dollar_costs(&self) -> (f64, f64, f64) {
        // The version is intentionally fixed with the rates; changing either
        // requires updating the contract fixture rather than silent drift.
        let _ = LOCAL_ESTIMATE_PRICE_VERSION;
        let (input_rate, cached_rate, output_rate) = match self.name.as_str() {
            "SOL" => SOL_PRICE_PER_MILLION,
            "TERRA" => TERRA_PRICE_PER_MILLION,
            "LUNA" => LUNA_PRICE_PER_MILLION,
            _ => (0.0, 0.0, 0.0),
        };
        let input = self.input_tokens.saturating_sub(self.cached_input_tokens) as f64;
        (
            input * input_rate / 1_000_000.0,
            self.cached_input_tokens as f64 * cached_rate / 1_000_000.0,
            self.output_tokens as f64 * output_rate / 1_000_000.0,
        )
    }
}

#[derive(Clone, Debug)]
struct ModelUsageTotals {
    sol: ModelUsageRow,
    terra: ModelUsageRow,
    luna: ModelUsageRow,
}

#[derive(Clone, Copy, Debug, Default)]
struct ModelDollarTotals {
    sol: f64,
    terra: f64,
    luna: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModelTokenTotals {
    sol: u64,
    terra: u64,
    luna: u64,
}

impl Default for ModelUsageTotals {
    fn default() -> Self {
        Self {
            sol: ModelUsageRow::new("SOL"),
            terra: ModelUsageRow::new("TERRA"),
            luna: ModelUsageRow::new("LUNA"),
        }
    }
}

impl ModelUsageTotals {
    fn add(&mut self, model: &str, snapshot: TokenSnapshot) {
        match Self::recognized_model(model) {
            Some("SOL") => self.sol.add(snapshot),
            Some("TERRA") => self.terra.add(snapshot),
            Some("LUNA") => self.luna.add(snapshot),
            _ => {}
        }
    }

    fn recognized_model(model: &str) -> Option<&'static str> {
        let model = model.to_ascii_lowercase();
        if model.contains("sol") {
            Some("SOL")
        } else if model.contains("terra") {
            Some("TERRA")
        } else if model.contains("luna") {
            Some("LUNA")
        } else {
            None
        }
    }

    fn rows(self) -> Vec<ModelUsageRow> {
        [self.sol, self.terra, self.luna]
            .into_iter()
            .filter(|row| row.tokens > 0)
            .collect()
    }

    fn dollar_totals(&self) -> ModelDollarTotals {
        fn total(row: &ModelUsageRow) -> f64 {
            let (input, cached_input, output) = row.dollar_costs();
            input + cached_input + output
        }

        ModelDollarTotals {
            sol: total(&self.sol),
            terra: total(&self.terra),
            luna: total(&self.luna),
        }
    }

    fn token_totals(&self) -> ModelTokenTotals {
        ModelTokenTotals {
            sol: self.sol.tokens,
            terra: self.terra.tokens,
            luna: self.luna.tokens,
        }
    }
}

impl ModelDollarTotals {
    fn from_rows(rows: &[ModelUsageRow]) -> Self {
        let mut totals = Self::default();
        for row in rows {
            let (input, cached_input, output) = row.dollar_costs();
            let total = input + cached_input + output;
            match row.name.as_str() {
                "SOL" => totals.sol = total,
                "TERRA" => totals.terra = total,
                "LUNA" => totals.luna = total,
                _ => {}
            }
        }
        totals
    }
}

const WEEK_SECONDS: i64 = 7 * 86_400;
const RESET_AT_TOLERANCE_SECONDS: i64 = 60;
// Some quota snapshots move the weekly reset timestamp forward together with
// the observation timestamp (for example 11:54 -> 11:56 -> 11:58).  Those
// rows are one period, not three two-minute periods.  Keep the ordinary
// sixty-second authority boundary, but admit this bounded moving-reset shape
// when both timestamps advance by the same amount.
const MOVING_RESET_GROUP_MAX_DRIFT_SECONDS: i64 = 5 * 60;
// Polls are nominally minute-spaced, but the quota service can advance the
// deadline by one or two poll intervals at once. Allow that bounded step
// jitter; a real period boundary is still separated by hours or days.
const MOVING_RESET_STEP_TOLERANCE_SECONDS: i64 = 180;
// Rows at one exact observation timestamp normally represent the same
// snapshot.  A reset-id drift of only a few seconds is collector jitter and
// keeps the latest observed quota; a larger drift is ambiguous and must not
// let row order manufacture a quota drop.
const SAME_TIMESTAMP_RESET_JITTER_SECONDS: i64 = 5;
// A minute bucket is the collector's contiguous observation unit. Beyond this
// boundary the elapsed interval is not observed, so a cumulative model
// increase must be shown as an idle horizontal segment followed by a point
// change, never as an invented diagonal rate.
const MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS: i64 = 60;
const MOVING_RESET_MIN_HORIZON_SECONDS: i64 = 86_400;
const ROLLING_RESET_ARTIFACT_MAX_JUMP_SECONDS: i64 = 2 * 86_400;
const ROLLING_RESET_ARTIFACT_MIN_PREVIOUS_REMAINING_PERCENT: f64 = 95.0;
const ROLLING_RESET_ARTIFACT_MAX_OBSERVATION_GAP_SECONDS: i64 = 2 * 3_600;
const LEGACY_MOVING_RESET_HORIZON_TOLERANCE_SECONDS: i64 = 120;
const LEGACY_MOVING_RESET_PAIR_GAP_SECONDS: i64 = 3_600;
const LEGACY_MOVING_RESET_PAIR_HORIZON_TOLERANCE_SECONDS: i64 = 60;

/// Return the start of the collector's minute bucket using mathematical
/// floor semantics, including for timestamps before the Unix epoch.
///
/// The authoritative period boundary is an external timestamp and therefore
/// cannot be allowed to wrap if converting its bucket index back to seconds
/// ever overflows. Callers that cannot represent the bucket must fail closed.
fn minute_start(timestamp: i64) -> Option<i64> {
    timestamp.div_euclid(60).checked_mul(60)
}

#[cfg(test)]
const GRAPH_METRIC_OPTIONS: [&str; 2] = ["ドル", "トークン"];
const FIXED_WINDOW_WIDTH: u32 = 900;
const FIXED_WINDOW_HEIGHT: u32 = 480;
const GRAPH_WINDOW_WIDTH: u32 = 940;
const GRAPH_WINDOW_HEIGHT: u32 = 640;
const LEGAL_WINDOW_WIDTH: u32 = 720;
const LEGAL_WINDOW_HEIGHT: u32 = 520;
const UNAUTHENTICATED_WINDOW_TITLE: &str = "アカウント未接続 — プラン未設定";
// Keep the native title-bar purpose suffix ASCII: some X11 window managers
// render `_NET_WM_NAME` with a fallback font that turns Japanese glyphs into
// tofu. The in-window headings remain Japanese and carry the full meaning.
#[cfg(test)]
const THREADS_WINDOW_PURPOSE: &str = "Threads";
#[cfg(test)]
const GRAPH_WINDOW_PURPOSE: &str = "Graph";

#[derive(Clone, Copy)]
enum WindowPurpose {
    Threads,
    Graph,
    Legal,
}

impl WindowPurpose {
    fn native_label(self) -> &'static str {
        match self {
            Self::Threads => "Threads",
            Self::Graph => "Graph",
            Self::Legal => "Legal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixedResizeDecision {
    Propagate,
    RejectAndRestore,
}

#[cfg(test)]
fn fixed_resize_decision(width: u32, height: u32) -> FixedResizeDecision {
    fixed_resize_decision_for_size(width, height, FIXED_WINDOW_WIDTH, FIXED_WINDOW_HEIGHT)
}

fn fixed_resize_decision_for_size(
    width: u32,
    height: u32,
    expected_width: u32,
    expected_height: u32,
) -> FixedResizeDecision {
    if width == 0 || height == 0 || (width == expected_width && height == expected_height) {
        FixedResizeDecision::Propagate
    } else {
        FixedResizeDecision::RejectAndRestore
    }
}

fn physical_size_for_logical(
    logical_width: u32,
    logical_height: u32,
    scale_factor: f64,
) -> (u32, u32) {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let size = winit::dpi::LogicalSize::new(logical_width as f64, logical_height as f64)
        .to_physical::<u32>(scale_factor);
    (size.width, size.height)
}

fn fixed_resize_decision_for_scale(
    width: u32,
    height: u32,
    logical_width: u32,
    logical_height: u32,
    scale_factor: f64,
) -> FixedResizeDecision {
    let (expected_width, expected_height) =
        physical_size_for_logical(logical_width, logical_height, scale_factor);
    fixed_resize_decision_for_size(width, height, expected_width, expected_height)
}

fn install_fixed_window_guard(window: &slint::Window) {
    install_window_size_guard(window, FIXED_WINDOW_WIDTH, FIXED_WINDOW_HEIGHT);
}

fn visible_window_position(
    monitor_position: winit::dpi::PhysicalPosition<i32>,
    monitor_size: winit::dpi::PhysicalSize<u32>,
    window_size: winit::dpi::PhysicalSize<u32>,
) -> winit::dpi::PhysicalPosition<i32> {
    const MARGIN: i64 = 32;
    let offset_x = if i64::from(monitor_size.width) >= i64::from(window_size.width) + MARGIN * 2 {
        MARGIN
    } else {
        0
    };
    let offset_y = if i64::from(monitor_size.height) >= i64::from(window_size.height) + MARGIN * 2 {
        MARGIN
    } else {
        0
    };
    let x = i64::from(monitor_position.x)
        .saturating_add(offset_x)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let y = i64::from(monitor_position.y)
        .saturating_add(offset_y)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    winit::dpi::PhysicalPosition::new(x, y)
}

/// Put the first window at a visible top-left position on the primary monitor
/// so a multi-monitor/XWayland placement cannot make a successful launch look
/// like a blank `run.sh`.
fn place_main_window_on_primary_monitor(window: &slint::Window) {
    let _ = window.with_winit_window(|winit_window| {
        let Some(monitor) = winit_window.primary_monitor() else {
            return;
        };
        let position = visible_window_position(
            monitor.position(),
            monitor.size(),
            winit_window.outer_size(),
        );
        winit_window.set_outer_position(position);
        winit_window.focus_window();
    });
}

fn install_resizable_window(window: &slint::Window) {
    let _ = window.with_winit_window(|winit_window| winit_window.set_resizable(true));
}

#[derive(Clone, Copy)]
enum ManualX11WindowAction {
    Move,
    Resize(winit::window::ResizeDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ManualX11Geometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

const MANUAL_X11_POLL_INTERVAL: Duration = Duration::from_millis(4);

static ACTIVE_MANUAL_X11_ACTIONS: OnceLock<Mutex<BTreeSet<X11Window>>> = OnceLock::new();

struct ManualX11ActionLease {
    keys: [X11Window; 2],
}

fn active_manual_x11_actions() -> &'static Mutex<BTreeSet<X11Window>> {
    ACTIVE_MANUAL_X11_ACTIONS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn claim_manual_x11_action(target: X11Window, client: X11Window) -> Option<ManualX11ActionLease> {
    let mut active = active_manual_x11_actions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if active.contains(&target) || (client != target && active.contains(&client)) {
        return None;
    }
    active.insert(target);
    active.insert(client);
    drop(active);
    Some(ManualX11ActionLease {
        keys: [target, client],
    })
}

impl Drop for ManualX11ActionLease {
    fn drop(&mut self) {
        let mut active = active_manual_x11_actions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active.remove(&self.keys[0]);
        active.remove(&self.keys[1]);
    }
}

fn finish_manual_x11_action(connection: &RustConnection, target: X11Window) {
    // A final round trip makes all requests flushed by this worker visible to
    // the X server before the per-target lease is released. This prevents a
    // new drag from racing an older configure request on the same window.
    let _ = connection.flush();
    if let Ok(cookie) = connection.get_geometry(target) {
        let _ = cookie.reply();
    }
}

fn configure_manual_x11_geometry(
    connection: &RustConnection,
    target: X11Window,
    action: ManualX11WindowAction,
    geometry: ManualX11Geometry,
) -> bool {
    let width = u32::try_from(geometry.width.max(1)).unwrap_or(u32::MAX);
    let height = u32::try_from(geometry.height.max(1)).unwrap_or(u32::MAX);
    let values = match action {
        ManualX11WindowAction::Move => ConfigureWindowAux::new().x(geometry.x).y(geometry.y),
        ManualX11WindowAction::Resize(_) => ConfigureWindowAux::new()
            .x(geometry.x)
            .y(geometry.y)
            .width(width)
            .height(height),
    };
    if connection.configure_window(target, &values).is_err() || connection.flush().is_err() {
        return false;
    }
    true
}

/// WSLg's Weston wrapper does not consistently honor `_NET_WM_MOVERESIZE` for
/// frameless clients. Keep the same left-button gesture usable there by
/// tracking the pointer on a private X11 connection and issuing configure
/// requests directly. On other backends the native winit operation remains
/// the fallback.
fn start_manual_x11_window_action(window: &slint::Window, action: ManualX11WindowAction) -> bool {
    let Some(window_id) = x11_window_id(window) else {
        return false;
    };
    let Ok((connection, screen_num)) = x11rb::connect(None) else {
        return false;
    };
    let Some(screen) = connection.setup().roots.get(screen_num) else {
        return false;
    };
    let root = screen.root;
    let target = x11_top_level_parent(&connection, window_id, root).unwrap_or(window_id);
    // A down event can reach more than one drag surface while the Slint item
    // tree is settling a grab. Only the first callback may own this target;
    // otherwise two polling workers can apply different pointer baselines and
    // visibly pull the window back and forth.
    // Keep the client XID in the lease as well as the managed wrapper.  The
    // wrapper lookup can transiently fall back to the client while a
    // compositor reparents the surface; the stable client key still prevents
    // two baselines from configuring one visible window.
    let Some(action_lease) = claim_manual_x11_action(target, window_id) else {
        return true;
    };
    let Ok(pointer_cookie) = connection.query_pointer(root) else {
        return false;
    };
    let Ok(pointer) = pointer_cookie.reply() else {
        return false;
    };
    let Ok(target_geometry_cookie) = connection.get_geometry(target) else {
        return false;
    };
    let Ok(target_geometry) = target_geometry_cookie.reply() else {
        return false;
    };
    let Ok(client_position_cookie) = connection.translate_coordinates(window_id, root, 0, 0) else {
        return false;
    };
    let Ok(client_position) = client_position_cookie.reply() else {
        return false;
    };
    let Ok(client_geometry_cookie) = connection.get_geometry(window_id) else {
        return false;
    };
    let Ok(client_geometry) = client_geometry_cookie.reply() else {
        return false;
    };
    let initial = ManualX11Geometry {
        // Pointer coordinates are relative to the X11 root.  The client
        // position must use that same coordinate space; the managed wrapper's
        // get_geometry() position is offset by the compositor frame.
        x: match action {
            ManualX11WindowAction::Move => i32::from(client_position.dst_x),
            ManualX11WindowAction::Resize(_) => i32::from(target_geometry.x),
        },
        y: match action {
            ManualX11WindowAction::Move => i32::from(client_position.dst_y),
            ManualX11WindowAction::Resize(_) => i32::from(target_geometry.y),
        },
        // Configure requests target the managed wrapper on WSLg, while its
        // width/height request is interpreted as the child client size.
        width: i32::from(client_geometry.width),
        height: i32::from(client_geometry.height),
    };
    let pointer_x = i32::from(pointer.root_x);
    let pointer_y = i32::from(pointer.root_y);

    thread::spawn(move || {
        let _action_lease = action_lease;
        let mut observed_button = false;
        let mut last_geometry = initial;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let Ok(pointer_cookie) = connection.query_pointer(root) else {
                break;
            };
            let Ok(pointer) = pointer_cookie.reply() else {
                break;
            };
            let pressed = pointer.mask.contains(KeyButMask::BUTTON1);
            if !pressed {
                if observed_button {
                    // The release sample is the final pointer position.  Apply
                    // it once before ending the worker so a fast circular
                    // gesture cannot stop on an older queued coordinate.
                    let delta_x = i32::from(pointer.root_x) - pointer_x;
                    let delta_y = i32::from(pointer.root_y) - pointer_y;
                    let geometry = manual_window_geometry(initial, action, delta_x, delta_y);
                    if geometry != last_geometry
                        && !configure_manual_x11_geometry(&connection, target, action, geometry)
                    {
                        break;
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(4));
                continue;
            }
            observed_button = true;
            let delta_x = i32::from(pointer.root_x) - pointer_x;
            let delta_y = i32::from(pointer.root_y) - pointer_y;
            let geometry = manual_window_geometry(initial, action, delta_x, delta_y);
            if geometry == last_geometry {
                thread::sleep(MANUAL_X11_POLL_INTERVAL);
                continue;
            }
            if !configure_manual_x11_geometry(&connection, target, action, geometry) {
                break;
            }
            last_geometry = geometry;
            thread::sleep(MANUAL_X11_POLL_INTERVAL);
        }
        finish_manual_x11_action(&connection, target);
    });
    true
}

fn x11_top_level_parent(
    connection: &RustConnection,
    window: X11Window,
    root: X11Window,
) -> Option<X11Window> {
    let mut current = window;
    for _ in 0..8 {
        let reply = connection.query_tree(current).ok()?.reply().ok()?;
        if reply.parent == root || reply.parent == 0 {
            return Some(current);
        }
        current = reply.parent;
    }
    Some(current)
}

fn manual_window_geometry(
    initial: ManualX11Geometry,
    action: ManualX11WindowAction,
    delta_x: i32,
    delta_y: i32,
) -> ManualX11Geometry {
    match action {
        ManualX11WindowAction::Move => ManualX11Geometry {
            x: initial.x.saturating_add(delta_x),
            y: initial.y.saturating_add(delta_y),
            ..initial
        },
        ManualX11WindowAction::Resize(direction) => {
            manual_resize_geometry(initial, direction, delta_x, delta_y)
        }
    }
}

fn manual_resize_geometry(
    initial: ManualX11Geometry,
    direction: winit::window::ResizeDirection,
    delta_x: i32,
    delta_y: i32,
) -> ManualX11Geometry {
    const MIN_WIDTH: i32 = 700;
    const MIN_HEIGHT: i32 = 480;
    let east = matches!(
        direction,
        winit::window::ResizeDirection::East
            | winit::window::ResizeDirection::NorthEast
            | winit::window::ResizeDirection::SouthEast
    );
    let west = matches!(
        direction,
        winit::window::ResizeDirection::West
            | winit::window::ResizeDirection::NorthWest
            | winit::window::ResizeDirection::SouthWest
    );
    let north = matches!(
        direction,
        winit::window::ResizeDirection::North
            | winit::window::ResizeDirection::NorthEast
            | winit::window::ResizeDirection::NorthWest
    );
    let south = matches!(
        direction,
        winit::window::ResizeDirection::South
            | winit::window::ResizeDirection::SouthEast
            | winit::window::ResizeDirection::SouthWest
    );
    let width = if east {
        (initial.width.saturating_add(delta_x)).max(MIN_WIDTH)
    } else if west {
        (initial.width.saturating_sub(delta_x)).max(MIN_WIDTH)
    } else {
        initial.width
    };
    let height = if south {
        (initial.height.saturating_add(delta_y)).max(MIN_HEIGHT)
    } else if north {
        (initial.height.saturating_sub(delta_y)).max(MIN_HEIGHT)
    } else {
        initial.height
    };
    ManualX11Geometry {
        x: if west {
            initial
                .x
                .saturating_add(initial.width.saturating_sub(width))
        } else {
            initial.x
        },
        y: if north {
            initial
                .y
                .saturating_add(initial.height.saturating_sub(height))
        } else {
            initial.y
        },
        width,
        height,
    }
}

fn begin_window_drag(window: &slint::Window) {
    if start_manual_x11_window_action(window, ManualX11WindowAction::Move) {
        return;
    }
    let _ = window.with_winit_window(|winit_window| winit_window.drag_window());
}

fn minimize_window(window: &slint::Window) {
    let _ = window.with_winit_window(|winit_window| winit_window.set_minimized(true));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GraphRestoreGeometry {
    position: Option<winit::dpi::PhysicalPosition<i32>>,
    size: winit::dpi::PhysicalSize<u32>,
}

#[derive(Debug, Default)]
struct GraphMaximizeState {
    restore: Option<GraphRestoreGeometry>,
    maximized: bool,
}

fn graph_monitor_geometry(
    window: &slint::Window,
) -> Option<(
    winit::dpi::PhysicalPosition<i32>,
    winit::dpi::PhysicalSize<u32>,
)> {
    window
        .with_winit_window(|winit_window| {
            let monitor = winit_window
                .current_monitor()
                .filter(|monitor| {
                    let size = monitor.size();
                    size.width > 1 && size.height > 1
                })
                .or_else(|| {
                    winit_window.available_monitors().find(|monitor| {
                        let size = monitor.size();
                        size.width > 1 && size.height > 1
                    })
                })?;
            let size = monitor.size();
            Some((monitor.position(), size))
        })
        .flatten()
}

fn graph_restore_geometry(window: &slint::Window) -> Option<GraphRestoreGeometry> {
    window
        .with_winit_window(|winit_window| {
            Some(GraphRestoreGeometry {
                position: winit_window.outer_position().ok(),
                size: winit_window.inner_size(),
            })
        })
        .flatten()
}

/// Maximizes Graph to the active monitor instead of asking the X11 window
/// manager to use the virtual desktop root. WSLg can expose a root that spans
/// several monitors; native `_NET_WM_STATE_MAXIMIZED_*` then gives Slint a
/// surface much wider than the actual monitor and the software renderer can
/// appear to freeze. This keeps the maximize/restore interaction intact while
/// choosing the real monitor geometry for the client surface.
fn toggle_graph_maximize(
    window: &slint::Window,
    graph: &GraphWindow,
    state: &Rc<RefCell<GraphMaximizeState>>,
) {
    if state.borrow().maximized {
        let restore = state.borrow_mut().restore.take();
        if let Some(restore) = restore {
            let _ = window.with_winit_window(|winit_window| {
                // Clear a maximize state that may have been set by the window
                // manager before restoring the user's previous geometry.
                if winit_window.is_maximized() {
                    winit_window.set_maximized(false);
                }
                if let Some(position) = restore.position {
                    winit_window.set_outer_position(position);
                }
                let _ = winit_window.request_inner_size(restore.size);
            });
        }
        state.borrow_mut().maximized = false;
        graph.set_app_maximized(false);
        return;
    }

    let Some((position, size)) = graph_monitor_geometry(window) else {
        // A monitor can be unavailable during display hot-plugging. Do not
        // fall back to native maximize here: that would reintroduce the
        // virtual-root resize this path is designed to avoid.
        return;
    };

    let restore = graph_restore_geometry(window);
    let _ = window.with_winit_window(|winit_window| {
        // Graph is not left in native maximized state. Applying the monitor's
        // physical geometry directly avoids the virtual-root resize entirely.
        if winit_window.is_maximized() {
            winit_window.set_maximized(false);
        }
        winit_window.set_outer_position(position);
        let _ = winit_window.request_inner_size(size);
    });
    let mut graph_state = state.borrow_mut();
    graph_state.restore = restore;
    graph_state.maximized = true;
    drop(graph_state);
    graph.set_app_maximized(true);
}

fn parse_resize_direction(direction: &str) -> Option<winit::window::ResizeDirection> {
    Some(match direction {
        "east" => winit::window::ResizeDirection::East,
        "north" => winit::window::ResizeDirection::North,
        "north-east" => winit::window::ResizeDirection::NorthEast,
        "north-west" => winit::window::ResizeDirection::NorthWest,
        "south" => winit::window::ResizeDirection::South,
        "south-east" => winit::window::ResizeDirection::SouthEast,
        "south-west" => winit::window::ResizeDirection::SouthWest,
        "west" => winit::window::ResizeDirection::West,
        _ => return None,
    })
}

fn begin_window_resize(window: &slint::Window, direction: &str) {
    let Some(direction) = parse_resize_direction(direction) else {
        return;
    };
    if start_manual_x11_window_action(window, ManualX11WindowAction::Resize(direction)) {
        return;
    }
    let _ = window.with_winit_window(|winit_window| winit_window.drag_resize_window(direction));
}

fn install_window_size_guard(window: &slint::Window, expected_width: u32, expected_height: u32) {
    let _ = window.with_winit_window(|winit_window| winit_window.set_resizable(false));
    window.on_winit_window_event(move |slint_window, event| {
        let winit::event::WindowEvent::Resized(size) = event else {
            return EventResult::Propagate;
        };
        let scale_factor = slint_window
            .with_winit_window(|winit_window| winit_window.scale_factor())
            .unwrap_or(1.0);
        match fixed_resize_decision_for_scale(
            size.width,
            size.height,
            expected_width,
            expected_height,
            scale_factor,
        ) {
            FixedResizeDecision::Propagate => EventResult::Propagate,
            FixedResizeDecision::RejectAndRestore => {
                let (expected_width, expected_height) =
                    physical_size_for_logical(expected_width, expected_height, scale_factor);
                let _ = slint_window.with_winit_window(|winit_window| {
                    winit_window.set_resizable(false);
                    let _ = winit_window.request_inner_size(winit::dpi::PhysicalSize::new(
                        expected_width,
                        expected_height,
                    ));
                });
                EventResult::PreventDefault
            }
        }
    });
}

/// Shows an existing secondary window and asks the native window manager to
/// activate and raise it.  `Window::show()` only maps a hidden Slint window; it
/// does not change the stacking order when the window already exists.
fn show_and_focus_window(
    window: &slint::Window,
    x11_monitor: Option<&X11WindowStateMonitor>,
) -> Result<(), slint::PlatformError> {
    let was_visible = window.is_visible();
    let window_id = was_visible.then(|| x11_window_id(window)).flatten();
    // Weston (the Xwayland window manager used by WSLg) ignores an explicit
    // raise request for a client that is already mapped.  Remapping the same
    // native window gives the WM its normal MapRequest path, which reliably
    // moves the existing window above its siblings without recreating it.
    if window_id.is_some() {
        window.hide()?;
    }
    window.show()?;
    let _ = window.with_winit_window(|winit_window| winit_window.focus_window());
    if let Some(x11_monitor) = x11_monitor {
        x11_monitor.raise_and_activate(window);
    }
    Ok(())
}

#[cfg(test)]
fn account_window_title(authenticated: bool, email: Option<&str>, plan_label: &str) -> String {
    if !authenticated {
        return UNAUTHENTICATED_WINDOW_TITLE.into();
    }
    let email = email
        .and_then(|value| security::bounded_email(value).ok())
        .filter(|value| !value.trim().is_empty());
    let Some(email) = email else {
        return UNAUTHENTICATED_WINDOW_TITLE.into();
    };
    let plan = security::bounded_plan(plan_label)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "プラン未設定".into());
    format!("{email} — {plan}")
}

#[cfg(test)]
fn detail_window_title(account_title: &str, purpose: &str) -> String {
    if account_title == UNAUTHENTICATED_WINDOW_TITLE {
        account_title.to_owned()
    } else {
        format!("{account_title} — {purpose}")
    }
}

#[cfg_attr(test, allow(dead_code))]
fn localized_plan_label(i18n: &I18n, plan_label: &str) -> String {
    match plan_label {
        "プラン未設定" => i18n.text(TextKey::PlanUnset).into(),
        "無料" => i18n.text(TextKey::PlanFree).into(),
        "エンタープライズ" => i18n.text(TextKey::PlanEnterprise).into(),
        "教育" => i18n.text(TextKey::PlanEducation).into(),
        other => other.to_owned(),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn localized_account_window_title(
    i18n: &I18n,
    authenticated: bool,
    email: Option<&str>,
    plan_label: &str,
) -> String {
    if !authenticated {
        return i18n.text(TextKey::WindowUnauthenticated).into();
    }
    let email = email
        .and_then(|value| security::bounded_email(value).ok())
        .filter(|value| !value.trim().is_empty());
    let Some(email) = email else {
        return i18n.text(TextKey::WindowUnauthenticated).into();
    };
    let plan = security::bounded_plan(plan_label)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| localized_plan_label(i18n, &value))
        .unwrap_or_else(|| i18n.text(TextKey::PlanUnset).into());
    format!("{email} — {plan}")
}

fn native_detail_window_title(
    _i18n: &I18n,
    authenticated: bool,
    account_title: &str,
    purpose: WindowPurpose,
) -> String {
    if !authenticated {
        return "Codex Info".into();
    }
    format!(
        "{} - {}",
        native_account_window_title(account_title),
        purpose.native_label()
    )
}

/// Native title bars may be rendered by a window-manager fallback font that
/// does not contain the localized CJK glyphs. Keep the title-bar identity
/// ASCII-only while the in-window headings continue to use the locale catalog.
fn native_account_window_title(account_title: &str) -> String {
    if account_title == UNAUTHENTICATED_WINDOW_TITLE {
        return "Codex Info".into();
    }
    let Some((identity, plan)) = account_title.split_once(" — ") else {
        return "Codex Info".into();
    };
    let identity = ascii_title_part(identity, "Codex");
    let plan = ascii_title_part(plan, "Plan");
    format!("{identity} - {plan}")
}

fn ascii_title_part(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' ')
    {
        value.to_owned()
    } else {
        fallback.to_owned()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct RateLimitSnapshot {
    remaining_percent: Option<f64>,
    reset_at: i64,
    window_seconds: i64,
    limit_name: String,
    quota_title: String,
    monthly: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ActiveThread {
    id: String,
    created_at: Option<i64>,
    updated_at: i64,
    title: String,
    model: String,
    model_label: String,
    total_tokens: Option<u64>,
    context_usage_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    last_user_message_at: Option<i64>,
    is_subagent: bool,
    parent_thread_id: Option<String>,
    depth: Option<i32>,
}

impl ActiveThread {
    fn to_public_thread(&self) -> PublicThread {
        PublicThread {
            id: self.id.clone(),
            title: self.title.clone(),
            parent_thread_id: self.parent_thread_id.clone(),
            model: self.model.clone(),
            model_label: self.model_label.clone(),
            total_tokens: self.total_tokens,
            context_usage_tokens: self.context_usage_tokens,
            context_window_tokens: self.context_window_tokens,
            created_at: self.created_at,
            last_user_message_at: self.last_user_message_at,
            is_subagent: self.is_subagent,
            depth: self.depth,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadPresentationRow {
    index: usize,
    forest_depth: usize,
    connected_to_parent: bool,
    has_children: bool,
    has_next_sibling: bool,
    ancestor_guides: [bool; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveThreadUpdate {
    Snapshot(Vec<ActiveThread>),
    NoThread,
    Failed,
}

/// Opt-in runtime diagnostics for investigating live update regressions.
///
/// The normal client remains silent and never writes account/session data to
/// logs.  Setting `CODEX_INFO_DEBUG=1` emits only bounded counters and state
/// transitions, which makes a broken worker/update boundary observable without
/// exposing email addresses, URLs, paths, prompts, or token contents.
fn debug_runtime(message: impl AsRef<str>) {
    if std::env::var_os("CODEX_INFO_DEBUG").is_some_and(|value| value == "1") {
        eprintln!("[codex-info] {}", message.as_ref());
    }
}

fn plan_type_label(plan_type: Option<&str>) -> String {
    protocol_contract::plan_label(plan_type)
}

fn monthly_window_seconds(reset_at: i64) -> i64 {
    let Some(end) = DateTime::<Utc>::from_timestamp(reset_at, 0) else {
        return 31 * 86_400;
    };
    end.checked_sub_months(Months::new(1))
        .map(|start| (end - start).num_seconds().max(1))
        .unwrap_or(31 * 86_400)
}

fn parse_rate_limits(
    rate: &Value,
    plan_type: Option<&str>,
    _now: i64,
) -> Result<RateLimitSnapshot, ()> {
    protocol_contract::decode_quota_for_plan(rate, plan_type)
        .map_err(|_| ())?
        .ok_or(())
        .map(|quota| RateLimitSnapshot {
            remaining_percent: quota.remaining_percent.map(f64::from),
            reset_at: quota.reset_at,
            window_seconds: quota.window_seconds,
            limit_name: quota.limit_name,
            quota_title: if quota.monthly {
                "月間残り利用枠".into()
            } else if quota.unlimited {
                "利用枠".into()
            } else {
                "残り利用枠".into()
            },
            monthly: quota.monthly,
        })
}
fn same_rollout_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_rollout_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.is_file() && right.is_file()
}

fn complete_rollout_prefix_len(file: &mut File, snapshot_len: u64) -> Result<u64, ()> {
    if snapshot_len == 0 {
        return Ok(0);
    }
    let tail_len = snapshot_len.min(security::MAX_JSONL_LINE_BYTES as u64 + 1);
    let tail_start = snapshot_len.checked_sub(tail_len).ok_or(())?;
    file.seek(SeekFrom::Start(tail_start)).map_err(|_| ())?;
    let capacity = usize::try_from(tail_len).map_err(|_| ())?;
    let mut tail = Vec::with_capacity(capacity);
    file.take(tail_len).read_to_end(&mut tail).map_err(|_| ())?;
    if tail.len() != capacity {
        return Err(());
    }
    if tail.last() == Some(&b'\n') {
        return Ok(snapshot_len);
    }
    if let Some(position) = tail.iter().rposition(|byte| *byte == b'\n') {
        return tail_start
            .checked_add(u64::try_from(position).map_err(|_| ())?)
            .and_then(|position| position.checked_add(1))
            .ok_or(());
    }
    if snapshot_len > security::MAX_JSONL_LINE_BYTES as u64 {
        return Err(());
    }
    Ok(0)
}

fn read_thread_rollout(
    sessions_root: &Path,
    candidate: &ValidatedThreadCandidate,
) -> Result<thread_contract::ValidatedRollout, ()> {
    let candidate_path = candidate.path().ok_or(())?;
    read_thread_rollout_path(sessions_root, Path::new(candidate_path))
}

fn read_thread_rollout_path(
    sessions_root: &Path,
    candidate_path: &Path,
) -> Result<thread_contract::ValidatedRollout, ()> {
    let canonical =
        security::canonical_regular_file_under(sessions_root, candidate_path).map_err(|_| ())?;
    let before_path = fs::symlink_metadata(&canonical).map_err(|_| ())?;
    if before_path.file_type().is_symlink() || !before_path.is_file() {
        return Err(());
    }
    let mut file = File::open(&canonical).map_err(|_| ())?;
    let before_file = file.metadata().map_err(|_| ())?;
    if !same_rollout_identity(&before_path, &before_file)
        || before_file.len() > security::MAX_SESSION_FILE_BYTES
    {
        return Err(());
    }
    let snapshot_len = before_file.len();
    let complete_len = complete_rollout_prefix_len(&mut file, snapshot_len)?;
    file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
    let rollout = {
        let mut reader = BufReader::new((&mut file).take(complete_len));
        match thread_contract::parse_rollout_reader_recoverable(&mut reader, complete_len) {
            Ok(rollout) => rollout,
            Err(error) => {
                debug_runtime(format!(
                    "thread rollout parse rejected reason={}",
                    error.message()
                ));
                return Err(());
            }
        }
    };

    let after_file = file.metadata().map_err(|_| ())?;
    let after_path = fs::symlink_metadata(&canonical).map_err(|_| ())?;
    if after_path.file_type().is_symlink()
        || !after_path.is_file()
        || !same_rollout_identity(&before_file, &after_file)
        || !same_rollout_identity(&after_file, &after_path)
        || after_file.len() < snapshot_len
    {
        return Err(());
    }
    Ok(rollout)
}

const MAX_PROC_PROCESS_ENTRIES: usize = 65_536;
const MAX_CODEX_PROCESS_FDS: usize = 16_384;
const MAX_OPEN_SESSION_FILES: usize = 1_024;

fn open_codex_session_paths(
    proc_root: &Path,
    sessions_root: &Path,
) -> Result<BTreeSet<PathBuf>, ()> {
    let mut process_entries = 0usize;
    let mut open_files = BTreeSet::new();
    for process in fs::read_dir(proc_root).map_err(|_| ())? {
        process_entries = process_entries.checked_add(1).ok_or(())?;
        if process_entries > MAX_PROC_PROCESS_ENTRIES {
            return Err(());
        }
        let process = match process {
            Ok(process) => process,
            Err(_) => continue,
        };
        let name = process.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let process_path = process.path();
        let comm = match File::open(process_path.join("comm")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                if file.take(64).read_to_end(&mut bytes).is_err() {
                    continue;
                }
                bytes
            }
            Err(_) => continue,
        };
        if comm.strip_suffix(b"\n") != Some(b"codex") && comm.as_slice() != b"codex" {
            continue;
        }
        let executable = match fs::read_link(process_path.join("exe")) {
            Ok(executable) => executable,
            Err(_) => continue,
        };
        if executable.file_name().and_then(|name| name.to_str()) != Some("codex") {
            continue;
        }
        let descriptors = match fs::read_dir(process_path.join("fd")) {
            Ok(descriptors) => descriptors,
            Err(_) => continue,
        };
        let mut descriptor_count = 0usize;
        for descriptor in descriptors {
            descriptor_count = descriptor_count.checked_add(1).ok_or(())?;
            if descriptor_count > MAX_CODEX_PROCESS_FDS {
                return Err(());
            }
            let descriptor = match descriptor {
                Ok(descriptor) => descriptor,
                Err(_) => continue,
            };
            let target = match fs::read_link(descriptor.path()) {
                Ok(target) => target,
                Err(_) => continue,
            };
            let Ok(canonical) = security::canonical_regular_file_under(sessions_root, &target)
            else {
                continue;
            };
            if canonical
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("jsonl")
            {
                continue;
            }
            open_files.insert(canonical);
            if open_files.len() > MAX_OPEN_SESSION_FILES {
                return Err(());
            }
        }
    }
    Ok(open_files)
}

fn active_thread_paths(codex_root: &Path) -> Result<(PathBuf, BTreeSet<PathBuf>), ()> {
    let sessions_root = codex_root.join("sessions");
    let active_paths = open_codex_session_paths(Path::new("/proc"), &sessions_root)?;
    debug_runtime(format!("thread active paths={}", active_paths.len()));
    Ok((sessions_root, active_paths))
}

fn fetch_active_thread_update(
    input: &mut impl Write,
    output: &Receiver<RpcReadEvent>,
    next_id: &mut u64,
    sessions_root: &Path,
    active_paths: &BTreeSet<PathBuf>,
    codex_root: &Path,
) -> ActiveThreadUpdate {
    fetch_active_thread_update_for_paths_and_state(
        input,
        output,
        next_id,
        sessions_root,
        active_paths,
        Some(codex_root),
    )
}

#[cfg(test)]
fn fetch_active_thread_update_for_paths(
    input: &mut impl Write,
    output: &Receiver<RpcReadEvent>,
    next_id: &mut u64,
    sessions_root: &Path,
    active_paths: &BTreeSet<PathBuf>,
) -> ActiveThreadUpdate {
    fetch_active_thread_update_for_paths_and_state(
        input,
        output,
        next_id,
        sessions_root,
        active_paths,
        None,
    )
}

fn fetch_active_thread_update_for_paths_and_state(
    input: &mut impl Write,
    output: &Receiver<RpcReadEvent>,
    next_id: &mut u64,
    sessions_root: &Path,
    active_paths: &BTreeSet<PathBuf>,
    codex_root: Option<&Path>,
) -> ActiveThreadUpdate {
    let mut accumulator = ThreadCycleAccumulator::new();
    let mut cursor: Option<String> = None;
    loop {
        let params = match thread_contract::thread_list_request(cursor.as_deref()) {
            Ok(params) => params,
            Err(_) => {
                debug_runtime("thread list request construction failed");
                return ActiveThreadUpdate::Failed;
            }
        };
        let request_id = *next_id;
        let Some(following_id) = next_id.checked_add(1) else {
            return ActiveThreadUpdate::Failed;
        };
        *next_id = following_id;
        let page = match request(input, output, request_id, "thread/list", params) {
            Ok(page) => page,
            Err(_) => {
                debug_runtime("thread list RPC failed");
                return ActiveThreadUpdate::Failed;
            }
        };
        match accumulator.accept_page(&page) {
            Ok(PageAcceptance::NeedNextPage { cursor: next }) => cursor = Some(next),
            Ok(PageAcceptance::Terminal) => break,
            Err(_) => {
                debug_runtime("thread list page rejected");
                return ActiveThreadUpdate::Failed;
            }
        }
    }

    let owner_root_ids = match accumulator.clone().ordered_candidates() {
        Ok(candidates) => candidates
            .into_iter()
            .filter(|candidate| {
                candidate
                    .path()
                    .and_then(|path| {
                        security::canonical_regular_file_under(sessions_root, Path::new(path)).ok()
                    })
                    .is_some_and(|path| active_paths.contains(&path))
            })
            .map(|candidate| candidate.id().to_owned())
            .collect::<BTreeSet<_>>(),
        Err(_) => {
            debug_runtime("thread candidate ordering failed");
            return ActiveThreadUpdate::Failed;
        }
    };
    debug_runtime(format!(
        "thread active owner roots={}",
        owner_root_ids.len()
    ));

    let root_outcome = thread_contract::select_active_threads_parsed_where(
        accumulator,
        |candidate| {
            candidate
                .path()
                .and_then(|path| {
                    security::canonical_regular_file_under(sessions_root, Path::new(path)).ok()
                })
                .is_some_and(|path| active_paths.contains(&path))
        },
        |candidate| {
            let result = read_thread_rollout(sessions_root, candidate);
            if result.is_err() {
                debug_runtime(format!(
                    "thread rollout rejected candidate={}",
                    candidate.id()
                ));
            }
            result
        },
    );
    let root_snapshots = match root_outcome {
        ThreadCycleOutcome::Snapshots(snapshots) => snapshots,
        ThreadCycleOutcome::NoThread => Vec::new(),
        ThreadCycleOutcome::CycleError => {
            debug_runtime("thread active candidate selection failed");
            return ActiveThreadUpdate::Failed;
        }
    };
    debug_runtime(format!("thread root snapshots={}", root_snapshots.len()));

    let mut threads = root_snapshots
        .into_iter()
        .map(|snapshot| ActiveThread {
            id: snapshot.thread_id,
            created_at: Some(snapshot.created_at),
            updated_at: snapshot.updated_at,
            title: snapshot.title,
            model: snapshot.model,
            model_label: snapshot.model_label,
            total_tokens: snapshot.total_tokens,
            context_usage_tokens: snapshot.context_usage_tokens,
            context_window_tokens: snapshot.context_window_tokens,
            last_user_message_at: snapshot.last_user_message_at,
            is_subagent: snapshot.is_subagent,
            parent_thread_id: snapshot.parent_thread_id,
            depth: snapshot.depth,
        })
        .collect::<Vec<_>>();

    if let Some(codex_root) = codex_root {
        let descendants =
            match thread_state::load_native_descendants(codex_root, sessions_root, &owner_root_ids)
            {
                Ok(descendants) => descendants,
                Err(_) => {
                    debug_runtime("thread descendant load failed");
                    return ActiveThreadUpdate::Failed;
                }
            };
        let mut descendant_snapshots = 0usize;
        let mut skipped_inactive_descendants = 0usize;
        for descendant in descendants {
            // The native state database is historical and keeps completed or
            // abandoned child rows.  A rollout parser can only tell us that
            // an old file ended after `task_started`; it cannot prove that
            // the child is still owned by a live app-server.  Require the
            // child rollout to be one of the files currently held by a
            // running Codex process, just as root candidates are filtered.
            if !active_paths.contains(&descendant.rollout_path) {
                skipped_inactive_descendants = skipped_inactive_descendants.saturating_add(1);
                continue;
            }
            let rollout = match read_thread_rollout_path(sessions_root, &descendant.rollout_path) {
                Ok(rollout) => rollout,
                Err(_) => {
                    debug_runtime("thread descendant rollout parse failed");
                    return ActiveThreadUpdate::Failed;
                }
            };
            if !rollout.is_running() {
                continue;
            }
            descendant_snapshots = descendant_snapshots.saturating_add(1);
            threads.push(ActiveThread {
                id: descendant.id,
                created_at: descendant.created_at,
                updated_at: descendant.updated_at,
                title: descendant.title,
                model: rollout.model().to_owned(),
                model_label: rollout.model_label().to_owned(),
                total_tokens: rollout.total_tokens(),
                context_usage_tokens: rollout.context_usage_tokens(),
                context_window_tokens: rollout.context_window_tokens(),
                last_user_message_at: rollout.last_user_message_at(),
                is_subagent: true,
                parent_thread_id: Some(descendant.parent_thread_id),
                depth: Some(descendant.depth),
            });
        }
        debug_runtime(format!(
            "thread descendant snapshots={}",
            descendant_snapshots
        ));
        debug_runtime(format!(
            "thread descendants skipped inactive={}",
            skipped_inactive_descendants
        ));
    }

    threads.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    if threads.is_empty() {
        ActiveThreadUpdate::NoThread
    } else {
        ActiveThreadUpdate::Snapshot(threads)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct UsageHistorySample {
    timestamp: i64,
    reset_at: i64,
    remaining_percent: f64,
    sol_dollars: f64,
    terra_dollars: f64,
    luna_dollars: f64,
    #[serde(default)]
    sol_tokens: u64,
    #[serde(default)]
    terra_tokens: u64,
    #[serde(default)]
    luna_tokens: u64,
}

impl UsageHistorySample {
    fn from_store(sample: usage_store::UsageHistorySample) -> Self {
        Self {
            timestamp: sample.timestamp,
            reset_at: sample.reset_at,
            remaining_percent: sample.remaining_percent.unwrap_or(-1.0),
            sol_dollars: sample.sol_dollars,
            terra_dollars: sample.terra_dollars,
            luna_dollars: sample.luna_dollars,
            sol_tokens: sample.sol_tokens,
            terra_tokens: sample.terra_tokens,
            luna_tokens: sample.luna_tokens,
        }
    }

    fn to_store(&self) -> usage_store::UsageHistorySample {
        usage_store::UsageHistorySample {
            timestamp: self.timestamp,
            reset_at: self.reset_at,
            remaining_percent: (self.remaining_percent >= 0.0).then_some(self.remaining_percent),
            sol_dollars: self.sol_dollars,
            terra_dollars: self.terra_dollars,
            luna_dollars: self.luna_dollars,
            sol_tokens: self.sol_tokens,
            terra_tokens: self.terra_tokens,
            luna_tokens: self.luna_tokens,
        }
    }

    #[cfg(test)]
    fn new(
        timestamp: i64,
        reset_at: i64,
        remaining_percent: f64,
        costs: ModelDollarTotals,
    ) -> Self {
        Self::new_with_usage(
            timestamp,
            reset_at,
            remaining_percent,
            costs,
            ModelTokenTotals::default(),
        )
    }

    fn new_with_usage(
        timestamp: i64,
        reset_at: i64,
        remaining_percent: f64,
        costs: ModelDollarTotals,
        tokens: ModelTokenTotals,
    ) -> Self {
        Self {
            // 1分ごとの取得値として、同じ分に複数回届いた場合も1点にまとめる。
            timestamp: timestamp.div_euclid(60) * 60,
            reset_at,
            remaining_percent: remaining_percent.clamp(0.0, 100.0),
            sol_dollars: costs.sol.max(0.0),
            terra_dollars: costs.terra.max(0.0),
            luna_dollars: costs.luna.max(0.0),
            sol_tokens: tokens.sol,
            terra_tokens: tokens.terra,
            luna_tokens: tokens.luna,
        }
    }

    #[cfg(test)]
    fn from_model_history(timestamp: i64, reset_at: i64, costs: ModelDollarTotals) -> Self {
        Self::from_model_history_with_usage(timestamp, reset_at, costs, ModelTokenTotals::default())
    }

    fn from_model_history_with_usage(
        timestamp: i64,
        reset_at: i64,
        costs: ModelDollarTotals,
        tokens: ModelTokenTotals,
    ) -> Self {
        Self {
            timestamp: timestamp.div_euclid(60) * 60,
            reset_at,
            // セッションログには残り利用枠の履歴がないため、グラフでは欠測として扱う。
            remaining_percent: -1.0,
            sol_dollars: costs.sol.max(0.0),
            terra_dollars: costs.terra.max(0.0),
            luna_dollars: costs.luna.max(0.0),
            sol_tokens: tokens.sol,
            terra_tokens: tokens.terra,
            luna_tokens: tokens.luna,
        }
    }

    fn is_valid(&self) -> bool {
        self.timestamp > 0
            && self.reset_at > 0
            && self.remaining_percent.is_finite()
            && self.sol_dollars.is_finite()
            && self.terra_dollars.is_finite()
            && self.luna_dollars.is_finite()
    }
}

fn same_reset_period(left: i64, right: i64) -> bool {
    left.abs_diff(right) <= RESET_AT_TOLERANCE_SECONDS as u64
}

/// Decide whether a newly reported reset timestamp is a real period boundary
/// or merely the service's rolling `now + window` value moving between polls.
/// The latter is common in the live response and must never make the main
/// screen throw away a complete model/history snapshot.
fn reset_transition_is_boundary(
    previous_reset: Option<i64>,
    previous_remaining: Option<f64>,
    next_reset: i64,
    next_remaining: Option<f64>,
    previous_observed_at: Option<i64>,
    now: i64,
    window_seconds: i64,
) -> bool {
    let Some(previous_reset) = previous_reset else {
        return next_reset > 0;
    };
    if next_reset <= 0 || same_reset_period(previous_reset, next_reset) {
        return false;
    }

    // A real reset is accompanied by a material quota refill.  A one-point
    // rounding fluctuation is not enough to change period identity.
    if let (Some(previous), Some(next)) = (previous_remaining, next_remaining) {
        if next.is_finite() && previous.is_finite() && next >= previous + 5.0 {
            return true;
        }
    }

    // If the prior observation was close to its boundary and the new one is
    // a full window ahead, this is a genuine rollover even when the quota
    // percentage is unavailable. Otherwise, two full-window horizons are the
    // same rolling period regardless of their absolute reset timestamps.
    let previous_at = previous_observed_at.unwrap_or(now);
    let previous_horizon = previous_reset.saturating_sub(previous_at);
    let next_horizon = next_reset.saturating_sub(now);
    let boundary_proximity = window_seconds.clamp(60, 3_600);
    if previous_horizon <= boundary_proximity && next_horizon >= window_seconds / 2 {
        return true;
    }
    false
}

fn merge_sample_values(existing: &mut UsageHistorySample, incoming: UsageHistorySample) {
    // Session backfill has no remaining-quota observation. Keep an existing
    // observed value while allowing a later API observation to replace it.
    let remaining_percent = if incoming.remaining_percent >= 0.0 {
        incoming.remaining_percent
    } else {
        existing.remaining_percent
    };
    let sol_dollars = existing.sol_dollars.max(incoming.sol_dollars);
    let terra_dollars = existing.terra_dollars.max(incoming.terra_dollars);
    let luna_dollars = existing.luna_dollars.max(incoming.luna_dollars);
    let sol_tokens = existing.sol_tokens.max(incoming.sol_tokens);
    let terra_tokens = existing.terra_tokens.max(incoming.terra_tokens);
    let luna_tokens = existing.luna_tokens.max(incoming.luna_tokens);
    *existing = incoming;
    existing.remaining_percent = remaining_percent;
    existing.sol_dollars = sol_dollars;
    existing.terra_dollars = terra_dollars;
    existing.luna_dollars = luna_dollars;
    existing.sol_tokens = sol_tokens;
    existing.terra_tokens = terra_tokens;
    existing.luna_tokens = luna_tokens;
}

fn merge_exact_sample(samples: &mut Vec<UsageHistorySample>, incoming: UsageHistorySample) {
    if let Some(existing) = samples.iter_mut().find(|existing| {
        existing.reset_at == incoming.reset_at && existing.timestamp == incoming.timestamp
    }) {
        merge_sample_values(existing, incoming);
    } else {
        samples.push(incoming);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryPeriod {
    canonical_reset_at: i64,
    start: i64,
    end: i64,
    label: String,
}

fn legacy_moving_reset_artifact(
    candidate: &UsageHistorySample,
    samples: &[&UsageHistorySample],
    exact_reset_counts: &BTreeMap<i64, usize>,
) -> bool {
    if candidate.remaining_percent != 100.0
        || exact_reset_counts.get(&candidate.reset_at) != Some(&1)
    {
        return false;
    }
    let candidate_horizon = i128::from(candidate.reset_at) - i128::from(candidate.timestamp);
    if candidate_horizon.abs_diff(i128::from(WEEK_SECONDS))
        > LEGACY_MOVING_RESET_HORIZON_TOLERANCE_SECONDS as u128
    {
        return false;
    }
    samples.iter().any(|other| {
        if other.reset_at == candidate.reset_at
            || other.remaining_percent != 100.0
            || exact_reset_counts.get(&other.reset_at) != Some(&1)
            || candidate.timestamp.abs_diff(other.timestamp)
                > LEGACY_MOVING_RESET_PAIR_GAP_SECONDS as u64
        {
            return false;
        }
        let other_horizon = i128::from(other.reset_at) - i128::from(other.timestamp);
        other_horizon.abs_diff(i128::from(WEEK_SECONDS))
            <= LEGACY_MOVING_RESET_HORIZON_TOLERANCE_SECONDS as u128
            && candidate_horizon.abs_diff(other_horizon)
                <= LEGACY_MOVING_RESET_PAIR_HORIZON_TOLERANCE_SECONDS as u128
    })
}

fn display_history_samples(samples: &[UsageHistorySample]) -> Vec<&UsageHistorySample> {
    samples.iter().filter(|sample| sample.is_valid()).collect()
}

#[derive(Clone, Debug)]
struct ResetSampleGroup {
    canonical_reset_at: i64,
    start: i64,
    samples: Vec<UsageHistorySample>,
}

fn moving_reset_observation_belongs_to_anchor(
    anchor: &UsageHistorySample,
    candidate: &UsageHistorySample,
) -> bool {
    // History is evaluated in observation order. A reset timestamp that jumps
    // forward without the observation moving by the same amount is a new
    // period (the boundary seen in the affected database), not a jittered
    // member of the previous period. Do not use saturating subtraction here:
    // accepting a backwards reset would silently merge unrelated periods.
    if candidate.timestamp < anchor.timestamp {
        return false;
    }
    let signed_reset_delta = candidate.reset_at - anchor.reset_at;
    // A moving quota response can wobble by a few seconds between adjacent
    // polls. Keep that same-period jitter, but never allow a large backwards
    // jump to cross a real reset boundary.
    if signed_reset_delta < 0
        && signed_reset_delta.unsigned_abs() > MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
    {
        return false;
    }
    let reset_delta = signed_reset_delta.max(0);
    if candidate.reset_at == anchor.reset_at || reset_delta <= RESET_AT_TOLERANCE_SECONDS {
        return true;
    }
    let timestamp_delta = candidate.timestamp.saturating_sub(anchor.timestamp);
    // The collector can emit two quota snapshots for the same minute while
    // the server advances reset_at between them. Treat that as one moving
    // observation as long as the jump is still bounded; a real boundary is
    // orders of magnitude larger and remains isolated.
    if timestamp_delta == 0 && reset_delta <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS {
        return true;
    }
    let anchor_horizon = anchor.reset_at.saturating_sub(anchor.timestamp);
    let candidate_horizon = candidate.reset_at.saturating_sub(candidate.timestamp);
    reset_delta <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
        && timestamp_delta > 0
        && anchor_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && candidate_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && anchor_horizon.abs_diff(candidate_horizon) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
        && reset_delta.abs_diff(timestamp_delta) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
}

fn reset_sample_groups(samples: &[UsageHistorySample]) -> Vec<ResetSampleGroup> {
    let mut sorted = display_history_samples(samples)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    // The wire payload is an observation timeline. Sort by observation first
    // so a rolling reset can be joined one step at a time even when its
    // cumulative drift is hours. A true period boundary has a large reset
    // jump at essentially the same observation timestamp and starts a new
    // group. Equal-reset fragments are coalesced below so legacy singleton
    // quota rows cannot split a spend period.
    sorted.sort_by_key(|sample| (sample.timestamp, sample.reset_at));

    let mut groups = Vec::new();
    let mut index = 0;
    while let Some(anchor) = sorted.get(index).cloned() {
        let mut members = Vec::new();
        let mut canonical_reset_at = anchor.reset_at;
        let mut moving_started = false;
        while let Some(candidate) = sorted.get(index) {
            let previous = members.last().unwrap_or(&anchor);
            // Several reset values at one exact observation timestamp are
            // legitimate only after the sequence has already demonstrated a
            // forward-moving timeline. Without this guard, unrelated
            // same-timestamp reset IDs would chain into one period.
            if !moving_started
                && candidate.timestamp == anchor.timestamp
                && candidate.reset_at.abs_diff(anchor.reset_at) > RESET_AT_TOLERANCE_SECONDS as u64
            {
                let has_forward_observation = sorted.get(index + 1..).is_some_and(|remaining| {
                    remaining.iter().any(|future| {
                        future.timestamp > anchor.timestamp
                            && future.reset_at >= candidate.reset_at
                            && future.reset_at - candidate.reset_at
                                <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
                    })
                });
                if !has_forward_observation {
                    break;
                }
            }
            if !moving_started
                && candidate.reset_at < anchor.reset_at
                && candidate.reset_at.abs_diff(anchor.reset_at) > RESET_AT_TOLERANCE_SECONDS as u64
            {
                break;
            }
            if !moving_reset_observation_belongs_to_anchor(
                if moving_started { previous } else { &anchor },
                candidate,
            ) {
                break;
            }
            if candidate.timestamp > anchor.timestamp && candidate.reset_at > anchor.reset_at {
                moving_started = true;
            }
            canonical_reset_at = canonical_reset_at.max(candidate.reset_at);
            members.push(candidate.clone());
            index += 1;
        }
        groups.push(ResetSampleGroup {
            canonical_reset_at,
            start: members
                .iter()
                .map(|sample| sample.timestamp)
                .min()
                .unwrap_or(anchor.timestamp),
            samples: members,
        });
    }
    let mut same_reset_merged = Vec::with_capacity(groups.len());
    for mut group in groups {
        if let Some(existing) =
            same_reset_merged
                .iter_mut()
                .find(|existing: &&mut ResetSampleGroup| {
                    existing.canonical_reset_at == group.canonical_reset_at
                })
        {
            existing.start = existing.start.min(group.start);
            existing.samples.append(&mut group.samples);
        } else {
            same_reset_merged.push(group);
        }
    }
    // Group first, then discard only groups made entirely of identified
    // legacy moving-reset artifacts. Filtering rows before grouping breaks a
    // continuous rolling chain at every singleton and manufactures an empty
    // 100% period. A group containing any real model snapshot or any
    // non-artifact quota snapshot remains visible.
    let valid_refs = samples
        .iter()
        .filter(|sample| sample.is_valid())
        .collect::<Vec<_>>();
    let mut exact_reset_counts = BTreeMap::new();
    for sample in &valid_refs {
        *exact_reset_counts.entry(sample.reset_at).or_insert(0) += 1;
    }
    let same_reset_merged = same_reset_merged
        .into_iter()
        .filter(|group| {
            !group.samples.iter().all(|sample| {
                legacy_moving_reset_artifact(sample, &valid_refs, &exact_reset_counts)
            })
        })
        .collect::<Vec<_>>();
    let has_model_usage = |sample: &UsageHistorySample| {
        sample.sol_dollars > 0.0
            || sample.terra_dollars > 0.0
            || sample.luna_dollars > 0.0
            || sample.sol_tokens > 0
            || sample.terra_tokens > 0
            || sample.luna_tokens > 0
    };
    let model_timestamps = same_reset_merged
        .iter()
        .flat_map(|group| group.samples.iter())
        .filter(|sample| has_model_usage(sample))
        .map(|sample| sample.timestamp)
        .collect::<BTreeSet<_>>();
    let drop_ambiguous_quota_groups = same_reset_merged
        .iter()
        .map(|group| {
            let only_missing_quota = group
                .samples
                .iter()
                .all(|sample| !has_model_usage(sample) && sample.remaining_percent < 0.0);
            only_missing_quota
                && group
                    .samples
                    .iter()
                    .any(|sample| model_timestamps.contains(&sample.timestamp))
        })
        .collect::<Vec<_>>();
    let same_reset_merged = same_reset_merged
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !drop_ambiguous_quota_groups[*index])
        .map(|(_, group)| group)
        .collect::<Vec<_>>();
    let mut merged: Vec<ResetSampleGroup> = Vec::with_capacity(same_reset_merged.len());
    let mut rolling_artifact_chain = false;
    for mut group in same_reset_merged {
        let group_end = group
            .samples
            .iter()
            .map(|sample| sample.timestamp)
            .max()
            .unwrap_or(group.start);
        let group_is_full_quota = group
            .samples
            .iter()
            .filter(|sample| sample.timestamp == group_end)
            .any(|sample| sample.remaining_percent >= 100.0)
            && group
                .samples
                .iter()
                .filter(|sample| sample.timestamp == group_end)
                .all(|sample| sample.remaining_percent < 0.0 || sample.remaining_percent >= 100.0);
        // The live service can emit a chain of quota-only rows while it
        // advances reset_at every poll. These rows are not independent
        // periods: they sit directly between the last spend observation and
        // the next spend observation. Keep the chain attached to the spend
        // period, but require a real spend anchor and a bounded timeline
        // so a genuine full-quota reset remains a separate period. A bounded
        // observation gap is allowed because the collector can be offline
        // for several polls while the service keeps advancing reset_at.
        let should_attach_rolling_artifact = merged.last().is_some_and(|previous| {
            let previous_end = previous
                .samples
                .iter()
                .map(|sample| sample.timestamp)
                .max()
                .unwrap_or(previous.start);
            let previous_has_model_usage = previous.samples.iter().any(|sample| {
                sample.sol_dollars > 0.0
                    || sample.terra_dollars > 0.0
                    || sample.luna_dollars > 0.0
                    || sample.sol_tokens > 0
                    || sample.terra_tokens > 0
                    || sample.luna_tokens > 0
            });
            let previous_end_has_observed_quota = previous
                .samples
                .iter()
                .filter(|sample| sample.timestamp == previous_end)
                .any(|sample| sample.remaining_percent >= 0.0);
            let previous_end_is_near_full = previous_end_has_observed_quota
                && previous
                    .samples
                    .iter()
                    .filter(|sample| {
                        sample.timestamp == previous_end && sample.remaining_percent >= 0.0
                    })
                    .all(|sample| {
                        sample.remaining_percent
                            >= ROLLING_RESET_ARTIFACT_MIN_PREVIOUS_REMAINING_PERCENT
                    });
            group_is_full_quota
                && group.start.saturating_sub(previous_end)
                    <= ROLLING_RESET_ARTIFACT_MAX_OBSERVATION_GAP_SECONDS
                && group
                    .canonical_reset_at
                    .saturating_sub(previous.canonical_reset_at)
                    <= ROLLING_RESET_ARTIFACT_MAX_JUMP_SECONDS
                && previous_end_is_near_full
                && (rolling_artifact_chain || previous_has_model_usage)
        });
        if should_attach_rolling_artifact {
            if let Some(previous) = merged.last_mut() {
                previous.canonical_reset_at =
                    previous.canonical_reset_at.max(group.canonical_reset_at);
                previous.start = previous.start.min(group.start);
                previous.samples.append(&mut group.samples);
            }
            rolling_artifact_chain = true;
            continue;
        }
        rolling_artifact_chain = false;
        let should_attach_to_previous = group.samples.len() == 1
            && group.samples[0].sol_dollars == 0.0
            && group.samples[0].terra_dollars == 0.0
            && group.samples[0].luna_dollars == 0.0
            && group.samples[0].sol_tokens == 0
            && group.samples[0].terra_tokens == 0
            && group.samples[0].luna_tokens == 0
            // Only a full-quota singleton is a known rolling-reset artifact.
            // A lower remaining value at the same timestamp can be a real
            // separate period; attaching it would overwrite the spend
            // period's quota observation (the observed 88% -> 14% failure).
            && group.samples[0].remaining_percent >= 100.0
            && merged.last().is_some_and(|previous: &ResetSampleGroup| {
                previous.samples.iter().any(|sample| {
                    sample.timestamp == group.samples[0].timestamp
                        && (sample.sol_dollars > 0.0
                            || sample.terra_dollars > 0.0
                            || sample.luna_dollars > 0.0
                            || sample.sol_tokens > 0
                            || sample.terra_tokens > 0
                            || sample.luna_tokens > 0)
                })
            });
        if should_attach_to_previous {
            if let Some(previous) = merged.last_mut() {
                let sample = group.samples.remove(0);
                merge_exact_sample(&mut previous.samples, sample);
                previous.start = previous
                    .samples
                    .iter()
                    .map(|sample| sample.timestamp)
                    .min()
                    .unwrap_or(previous.start);
            }
        } else {
            merged.push(group);
        }
    }
    merged
}

/// Groups reset observations by an anchored sixty-second window, with a
/// bounded moving-reset exception for snapshots whose reset and observation
/// timestamps advance together. The anchor is never advanced by a member of
/// the group, which prevents a chain of small jitters from swallowing a
/// distinct period.
fn history_periods_for_samples(
    samples: &[UsageHistorySample],
    now: i64,
    current_reset_at: Option<i64>,
) -> Vec<HistoryPeriod> {
    let groups = reset_sample_groups(samples);

    let mut periods = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            // A legacy database can contain an unrelated singleton at the
            // exact same observation minute. It is its own period, but it is
            // not a boundary for the selected spend period; use the next
            // strictly later observation as the visual end instead of
            // collapsing the graph to a zero-width interval.
            let next_start = groups
                .iter()
                .skip(index + 1)
                .map(|next| next.start)
                .find(|start| *start > group.start);
            let period_end = next_start.map_or(group.canonical_reset_at, |next| {
                group.canonical_reset_at.min(next)
            });
            let is_current = current_reset_at.is_some_and(|current| {
                current.abs_diff(group.canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
                    && now < group.canonical_reset_at
            });
            let end = if is_current {
                now.max(group.start).min(group.canonical_reset_at)
            } else {
                period_end
            };
            // Labels are a presentation concern. Production UI labels are
            // rebuilt by `CodexInfoState::history_periods` with the one
            // startup-pinned I18n/timezone instance. The test-only label
            // keeps the legacy grouping fixtures readable without allowing a
            // fixed JST formatter into the runtime path.
            #[cfg(test)]
            let mut label = format_period_label(group.start, period_end);
            #[cfg(not(test))]
            let label = String::new();
            #[cfg(test)]
            if is_current {
                label.push_str("（現在）");
            }
            HistoryPeriod {
                canonical_reset_at: group.canonical_reset_at,
                start: group.start,
                end,
                label,
            }
        })
        .collect::<Vec<_>>();
    // A burst of several reset groups can share the same observed start and
    // therefore the same base interval label. ComboBox selection must never
    // rely on an ambiguous label, so only colliding labels receive the
    // canonical reset timestamp as a deterministic, user-readable suffix.
    #[cfg(test)]
    {
        let base_labels = periods
            .iter()
            .map(|period| period.label.clone())
            .collect::<Vec<_>>();
        for index in 0..periods.len() {
            let same_start_count = periods
                .iter()
                .filter(|candidate| candidate.start == periods[index].start)
                .count();
            if base_labels
                .iter()
                .filter(|label| **label == base_labels[index])
                .count()
                > 1
                || same_start_count > 1
            {
                let canonical_reset_at = periods[index].canonical_reset_at;
                let reset_label = format_period_timestamp(canonical_reset_at)
                    .unwrap_or_else(|| "時刻不明".into());
                periods[index]
                    .label
                    .push_str(&format!("（期限 {reset_label}）"));
            }
        }
    }
    periods.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.canonical_reset_at.cmp(&left.canonical_reset_at))
    });
    periods
}

fn current_history_period_reset(
    periods: &[HistoryPeriod],
    current_reset_at: Option<i64>,
    now: i64,
) -> Option<i64> {
    let current = current_reset_at?;
    periods
        .iter()
        .filter(|period| now < period.canonical_reset_at)
        .filter(|period| {
            current.abs_diff(period.canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
        })
        .min_by(|left, right| {
            current
                .abs_diff(left.canonical_reset_at)
                .cmp(&current.abs_diff(right.canonical_reset_at))
                .then_with(|| right.canonical_reset_at.cmp(&left.canonical_reset_at))
        })
        .map(|period| period.canonical_reset_at)
}

/// Build the shared presentation/publication view without mutating retained
/// history.  A moving full-quota observation whose reset horizon advances
/// with its acquisition minute is not a sample from the later authoritative
/// cycle when it precedes that cycle's start.  Remove only that already-known
/// acquisition shape from the current group; any lower/non-zero/ambiguous row
/// remains so the existing authoritative-bounds and REST validation gates can
/// fail closed.
fn authoritative_history_projection_samples(
    samples: &[UsageHistorySample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
) -> Vec<UsageHistorySample> {
    let Some(current_reset_at) = current_reset_at else {
        return samples.to_vec();
    };
    let groups = reset_sample_groups(samples);
    let Some(current_group) = groups.iter().find(|group| {
        current_reset_at.abs_diff(group.canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
    }) else {
        return samples.to_vec();
    };
    let Some(authoritative_start) = (window_seconds > 0)
        .then(|| current_group.canonical_reset_at.checked_sub(window_seconds))
        .flatten()
        .and_then(minute_start)
    else {
        return samples.to_vec();
    };

    let valid_refs = samples
        .iter()
        .filter(|sample| sample.is_valid())
        .collect::<Vec<_>>();
    let mut exact_reset_counts = BTreeMap::new();
    for sample in &valid_refs {
        *exact_reset_counts.entry(sample.reset_at).or_insert(0) += 1;
    }
    let excluded = current_group
        .samples
        .iter()
        .filter(|sample| sample.timestamp < authoritative_start)
        .filter(|sample| {
            sample.sol_dollars == 0.0
                && sample.terra_dollars == 0.0
                && sample.luna_dollars == 0.0
                && sample.sol_tokens == 0
                && sample.terra_tokens == 0
                && sample.luna_tokens == 0
        })
        .filter(|sample| legacy_moving_reset_artifact(sample, &valid_refs, &exact_reset_counts))
        .map(|sample| (sample.reset_at, sample.timestamp))
        .collect::<BTreeSet<_>>();
    if excluded.is_empty() {
        return samples.to_vec();
    }

    samples
        .iter()
        .filter(|sample| !excluded.contains(&(sample.reset_at, sample.timestamp)))
        .cloned()
        .collect()
}

/// Apply the authoritative quota bounds to the current raw period only.
///
/// `UsageHistory` remains the owner of raw inventory and historical grouping.
/// This bounded projection gives current consumers the quota reset/window
/// boundary without deleting or clipping any stored sample. A current period
/// with an owned row before the authoritative start is rejected wholesale so
/// no caller can publish a mixed or invented interval.
fn apply_authoritative_current_bounds(
    mut periods: Vec<HistoryPeriod>,
    samples: &[UsageHistorySample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
    observed_at: i64,
) -> Vec<HistoryPeriod> {
    let Some(current_reset_at) = current_reset_at else {
        return periods;
    };
    let Some(current_index) = periods.iter().position(|period| {
        current_reset_at.abs_diff(period.canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
    }) else {
        return periods;
    };
    let canonical_reset_at = periods[current_index].canonical_reset_at;
    let Some(authoritative_start) = (window_seconds > 0)
        .then(|| canonical_reset_at.checked_sub(window_seconds))
        .flatten()
    else {
        periods.remove(current_index);
        return periods;
    };
    let Some(start) = minute_start(authoritative_start) else {
        periods.remove(current_index);
        return periods;
    };
    let end = canonical_reset_at.min(observed_at);
    if end < start {
        periods.remove(current_index);
        return periods;
    }

    let owned_group = reset_sample_groups(samples).into_iter().find(|group| {
        group.canonical_reset_at.abs_diff(canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
    });
    if owned_group.is_some_and(|group| group.samples.iter().any(|sample| sample.timestamp < start))
    {
        periods.remove(current_index);
        return periods;
    }

    periods[current_index].start = start;
    periods[current_index].end = end;
    periods
}

#[derive(Debug, Default)]
struct UsageHistory {
    db_path: Option<PathBuf>,
    samples: Vec<UsageHistorySample>,
    startup_maintenance_done: bool,
}

#[derive(Default)]
struct SameTimestampRemainingState {
    first_remaining: Option<f64>,
    values_differ: bool,
    min_reset_at: Option<i64>,
    max_reset_at: Option<i64>,
    quota_only: bool,
    reset_values: BTreeMap<i64, f64>,
}

impl UsageHistory {
    fn load() -> Self {
        let now = Utc::now();
        let mut history = Self::load_from_db_path_at(usage_history_db_path(), now);
        history.startup_maintenance(now);
        history
    }

    #[cfg(test)]
    fn load_from_db_path(db_path: Option<PathBuf>) -> Self {
        Self::load_from_db_path_at(db_path, Utc::now())
    }

    fn load_from_db_path_at(db_path: Option<PathBuf>, now: DateTime<Utc>) -> Self {
        let samples = db_path
            .as_ref()
            .and_then(|path| UsageStore::open(path).ok())
            // Startup must never materialize an unbounded database. The
            // bounded recent read uses the timestamp/reset index and the same
            // cardinality ceiling as the public details contract.
            .and_then(|store| store.load_recent_one_month(now).ok())
            .unwrap_or_default();

        let samples = samples
            .into_iter()
            .map(UsageHistorySample::from_store)
            .collect();
        let mut history = Self {
            db_path,
            samples,
            startup_maintenance_done: false,
        };
        history.normalize();
        history.mark_existing_ambiguous_same_timestamp_remaining();
        history
    }

    /// Return a bounded period hint for a startup backfill when the account
    /// bridge is unavailable. The persisted reset timestamp is evidence from
    /// the local log/DB, not a substitute for a fresh quota snapshot.
    fn latest_period_hint(&self) -> Option<(i64, i64)> {
        daemon::load_reset_hint().or_else(|| {
            self.samples
                .iter()
                .max_by_key(|sample| sample.timestamp)
                .map(|sample| (sample.reset_at, WEEK_SECONDS))
        })
    }

    fn preview(now: i64, reset_at: i64, costs: ModelDollarTotals) -> Self {
        let fractions = [0.08, 0.28, 0.48, 0.68, 0.88, 1.0];
        let preview_period =
            |period_reset: i64, period_start: i64, period_end: i64, cost_scale: f64| {
                let elapsed = period_end.saturating_sub(period_start).max(1) as f64;
                fractions
                    .into_iter()
                    .enumerate()
                    .map(move |(index, fraction)| {
                        // プレビュー点を現在時刻までの実測可能な範囲へ分散し、
                        // 未来の点を現在時刻へ丸めて同一X座標に重ねない。
                        let timestamp = period_start + (elapsed * fraction) as i64;
                        // The graph preview deliberately includes one repeated
                        // quota endpoint while model totals continue to advance.
                        // This exercises the missing-sample interpolation path in
                        // the actual X11 image instead of only in unit tests.
                        let used_percent = [16.0, 31.0, 31.0, 51.0, 51.0, 86.0][index];
                        // Keep one model snapshot unchanged to show the
                        // legitimate idle interval as horizontal, while the
                        // later repeated quota value still exercises active
                        // interpolation.
                        let scale_fraction = if index == 2 { 0.28 } else { fraction };
                        let sol_scale = (0.18 + 0.82 * scale_fraction) * cost_scale;
                        let terra_scale = (1.0 - 0.65 * scale_fraction).max(0.1) * cost_scale;
                        let luna_scale =
                            (0.35 + 0.65 * (1.0 - scale_fraction).powi(2)).max(0.1) * cost_scale;
                        UsageHistorySample::new_with_usage(
                            timestamp,
                            period_reset,
                            100.0 - used_percent,
                            ModelDollarTotals {
                                sol: costs.sol * sol_scale,
                                terra: costs.terra * terra_scale,
                                luna: costs.luna * luna_scale,
                            },
                            ModelTokenTotals {
                                sol: (159_278_976.0 * sol_scale) as u64,
                                terra: (30_885_887.0 * terra_scale) as u64,
                                luna: (155_294_770.0 * luna_scale) as u64,
                            },
                        )
                    })
            };
        let previous_reset_at = reset_at.saturating_sub(WEEK_SECONDS);
        let previous = preview_period(
            previous_reset_at,
            previous_reset_at.saturating_sub(WEEK_SECONDS),
            previous_reset_at,
            0.72,
        );
        let current = preview_period(
            reset_at,
            reset_at.saturating_sub(WEEK_SECONDS),
            now.min(reset_at),
            1.0,
        );
        let samples = previous.chain(current).collect();
        Self {
            db_path: None,
            samples,
            startup_maintenance_done: true,
        }
    }

    /// Performs the one destructive history operation during normal startup.
    ///
    /// The visible in-memory set is always bounded, even if persistent pruning
    /// is unavailable. A storage failure must never expose an old or future row.
    fn startup_maintenance(&mut self, now: DateTime<Utc>) {
        if self.startup_maintenance_done {
            return;
        }
        self.startup_maintenance_done = true;

        if let Some(path) = self.db_path.as_ref() {
            // Pruning is the only normal destructive operation. A consistent
            // three-generation SQLite backup must succeed first; otherwise
            // leave every historical row untouched and continue read-only.
            if UsageStore::backup_generations(path, 3).is_ok() {
                if let Ok(mut store) = UsageStore::open(path) {
                    let _ = store.prune_older_than_three_months(now);
                }
            }
        }

        let cutoff = three_months_before_utc(now);
        self.samples
            .retain(|sample| sample.timestamp >= cutoff && sample.timestamp <= now.timestamp());
        self.normalize();
        self.mark_existing_ambiguous_same_timestamp_remaining();
    }

    fn record(&mut self, sample: UsageHistorySample) {
        if !sample.is_valid() {
            return;
        }
        let mut sample = sample;
        self.mark_ambiguous_same_timestamp_remaining(&mut sample);
        let acquisition_end = sample.timestamp;
        sample.reset_at = self.canonical_reset_at(sample.reset_at);
        merge_exact_sample(&mut self.samples, sample);
        self.normalize();
        self.retain_acquisition_window(acquisition_end);
        self.save();
    }

    fn apply_backfill_samples(&mut self, reset_at: i64, samples: Vec<UsageHistorySample>) {
        if samples.is_empty() {
            return;
        }
        let acquisition_end = samples
            .iter()
            .filter(|sample| sample.is_valid())
            .map(|sample| sample.timestamp)
            .max();
        let storage_reset_at = self.canonical_reset_at(reset_at);
        for mut sample in samples {
            if !sample.is_valid() {
                continue;
            }
            self.mark_ambiguous_same_timestamp_remaining(&mut sample);
            sample.reset_at = storage_reset_at;
            merge_exact_sample(&mut self.samples, sample);
        }
        self.normalize();
        if let Some(acquisition_end) = acquisition_end {
            self.retain_acquisition_window(acquisition_end);
        }
        self.save();
    }

    fn mark_ambiguous_same_timestamp_remaining(&mut self, incoming: &mut UsageHistorySample) {
        let incoming_remaining = incoming.remaining_percent;
        if incoming_remaining < 0.0 {
            return;
        }
        let incoming_quota_only = incoming.sol_dollars == 0.0
            && incoming.terra_dollars == 0.0
            && incoming.luna_dollars == 0.0
            && incoming.sol_tokens == 0
            && incoming.terra_tokens == 0
            && incoming.luna_tokens == 0;
        let mut conflict = false;
        for existing in &self.samples {
            if existing.timestamp != incoming.timestamp
                || existing.reset_at == incoming.reset_at
                || existing.remaining_percent < 0.0
                || (existing.remaining_percent - incoming_remaining).abs() <= f64::EPSILON
            {
                continue;
            }
            let reset_span = existing.reset_at.abs_diff(incoming.reset_at);
            let existing_quota_only = existing.sol_dollars == 0.0
                && existing.terra_dollars == 0.0
                && existing.luna_dollars == 0.0
                && existing.sol_tokens == 0
                && existing.terra_tokens == 0
                && existing.luna_tokens == 0;
            if reset_span == 0
                || reset_span > SAME_TIMESTAMP_RESET_JITTER_SECONDS as u64
                || existing_quota_only
                || incoming_quota_only
            {
                conflict = true;
                break;
            }
        }
        if conflict {
            for existing in &mut self.samples {
                if existing.timestamp == incoming.timestamp {
                    existing.remaining_percent = -1.0;
                }
            }
            incoming.remaining_percent = -1.0;
        }
    }

    /// Sanitize legacy rows already present in SQLite before any consumer can
    /// observe them.  New writes call `mark_ambiguous_same_timestamp_remaining`
    /// before merging; startup must apply the identical rule to pre-existing
    /// rows so `canonical_samples`, period lists, and graph payloads share one
    /// fail-closed boundary.  This only changes the in-memory view; the raw
    /// retention database remains untouched until a normal write occurs.
    fn mark_existing_ambiguous_same_timestamp_remaining(&mut self) {
        // Aggregate by timestamp/reset instead of comparing every pair. A
        // malformed database may contain the full bounded month at one
        // timestamp; startup must remain O(n log n), not O(n²).
        let mut states = BTreeMap::<i64, SameTimestampRemainingState>::new();
        for sample in &self.samples {
            if sample.remaining_percent < 0.0 {
                continue;
            }
            let state = states.entry(sample.timestamp).or_default();
            if let Some(first) = state.first_remaining {
                state.values_differ |= (first - sample.remaining_percent).abs() > f64::EPSILON;
            } else {
                state.first_remaining = Some(sample.remaining_percent);
            }
            state.min_reset_at = Some(
                state
                    .min_reset_at
                    .map_or(sample.reset_at, |value| value.min(sample.reset_at)),
            );
            state.max_reset_at = Some(
                state
                    .max_reset_at
                    .map_or(sample.reset_at, |value| value.max(sample.reset_at)),
            );
            state.quota_only |= sample.sol_dollars == 0.0
                && sample.terra_dollars == 0.0
                && sample.luna_dollars == 0.0
                && sample.sol_tokens == 0
                && sample.terra_tokens == 0
                && sample.luna_tokens == 0;
            if let Some(previous) = state
                .reset_values
                .insert(sample.reset_at, sample.remaining_percent)
            {
                state.values_differ |= (previous - sample.remaining_percent).abs() > f64::EPSILON;
            }
        }
        let mut ambiguous_timestamps = BTreeSet::new();
        for (timestamp, state) in states {
            let reset_span = state
                .min_reset_at
                .zip(state.max_reset_at)
                .map(|(minimum, maximum)| minimum.abs_diff(maximum))
                .unwrap_or(0);
            if state.values_differ
                && (reset_span == 0
                    || reset_span > SAME_TIMESTAMP_RESET_JITTER_SECONDS as u64
                    || state.quota_only)
            {
                ambiguous_timestamps.insert(timestamp);
            }
        }
        for sample in &mut self.samples {
            if ambiguous_timestamps.contains(&sample.timestamp) {
                sample.remaining_percent = -1.0;
            }
        }
    }

    fn canonical_reset_at(&self, reset_at: i64) -> i64 {
        history_periods_for_samples(&self.samples, 0, None)
            .into_iter()
            .find(|period| {
                self.samples.iter().any(|sample| {
                    sample.reset_at.abs_diff(period.canonical_reset_at)
                        <= RESET_AT_TOLERANCE_SECONDS as u64
                        && sample.reset_at.abs_diff(reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
                })
            })
            .map_or(reset_at, |period| period.canonical_reset_at)
    }

    fn graph_data_for_reset(&self, reset_at: i64) -> String {
        let samples = self.samples_for_reset(Some(reset_at));
        serde_json::to_string(&samples).unwrap_or_else(|_| "[]".into())
    }

    fn reset_periods_desc(&self) -> Vec<i64> {
        history_periods_for_samples(&self.samples, 0, None)
            .into_iter()
            .map(|period| period.canonical_reset_at)
            .collect()
    }

    fn periods(&self, now: i64, current_reset_at: Option<i64>) -> Vec<HistoryPeriod> {
        history_periods_for_samples(&self.samples, now, current_reset_at)
    }

    fn period_for_id(
        &self,
        canonical_reset_at: i64,
        now: i64,
        current_reset_at: Option<i64>,
    ) -> Option<HistoryPeriod> {
        self.periods(now, current_reset_at)
            .into_iter()
            .find(|period| period.canonical_reset_at == canonical_reset_at)
    }

    #[cfg(test)]
    fn period_id_for_label(
        &self,
        label: &str,
        now: i64,
        current_reset_at: Option<i64>,
    ) -> Option<i64> {
        self.periods(now, current_reset_at)
            .into_iter()
            .find(|period| period.label == label)
            .map(|period| period.canonical_reset_at)
    }

    #[cfg(test)]
    fn period_options(&self, now: i64, current_reset_at: Option<i64>) -> Vec<String> {
        let periods = self.periods(now, current_reset_at);
        if periods.is_empty() {
            vec!["履歴なし".into()]
        } else {
            periods.into_iter().map(|period| period.label).collect()
        }
    }

    fn samples_for_reset(&self, reset_at: Option<i64>) -> Vec<UsageHistorySample> {
        let Some(reset_at) = reset_at else {
            return Vec::new();
        };
        let canonical_reset_at = self
            .period_for_id(reset_at, 0, None)
            .map(|period| period.canonical_reset_at)
            .or_else(|| {
                self.periods(0, None)
                    .into_iter()
                    .find(|period| {
                        period.canonical_reset_at.abs_diff(reset_at)
                            <= RESET_AT_TOLERANCE_SECONDS as u64
                    })
                    .map(|period| period.canonical_reset_at)
            });
        let Some(canonical_reset_at) = canonical_reset_at else {
            return Vec::new();
        };
        let mut selected = reset_sample_groups(&self.samples)
            .into_iter()
            .find(|group| group.canonical_reset_at == canonical_reset_at)
            .map(|group| group.samples)
            .unwrap_or_default();
        selected.sort_by_key(|sample| (sample.timestamp, sample.reset_at));
        let mut merged: Vec<UsageHistorySample> = Vec::with_capacity(selected.len());
        let mut index = 0;
        while index < selected.len() {
            let timestamp = selected[index].timestamp;
            let mut rows = Vec::new();
            while index < selected.len() && selected[index].timestamp == timestamp {
                rows.push(selected[index].clone());
                index += 1;
            }
            let mut sample = rows.last().cloned().expect("timestamp group is non-empty");
            let remaining_values = rows
                .iter()
                .filter(|row| row.remaining_percent >= 0.0)
                .map(|row| row.remaining_percent)
                .collect::<Vec<_>>();
            let conflicting_remaining = remaining_values
                .windows(2)
                .any(|values| (values[0] - values[1]).abs() > f64::EPSILON);
            let reset_span = rows
                .iter()
                .map(|row| row.reset_at)
                .min()
                .unwrap_or(sample.reset_at)
                .abs_diff(
                    rows.iter()
                        .map(|row| row.reset_at)
                        .max()
                        .unwrap_or(sample.reset_at),
                );
            // Preserve the historical jitter contract (a few-second drift
            // keeps the latest value), but fail closed for a same-ID conflict,
            // a real reset-id disagreement, or any quota-only collision.  In
            // particular this prevents a 30/60-second moving-reset row with
            // no model usage from overwriting an 88% spend observation with
            // 14%.
            let has_quota_only_row = rows.iter().any(|row| {
                row.sol_dollars == 0.0
                    && row.terra_dollars == 0.0
                    && row.luna_dollars == 0.0
                    && row.sol_tokens == 0
                    && row.terra_tokens == 0
                    && row.luna_tokens == 0
            });
            if conflicting_remaining
                && (reset_span == 0
                    || reset_span > SAME_TIMESTAMP_RESET_JITTER_SECONDS as u64
                    || has_quota_only_row)
            {
                sample.remaining_percent = -1.0;
            } else if let Some(remaining) = remaining_values.last().copied() {
                sample.remaining_percent = remaining;
            }
            sample.reset_at = canonical_reset_at;
            sample.sol_dollars = rows.iter().map(|row| row.sol_dollars).fold(0.0, f64::max);
            sample.terra_dollars = rows.iter().map(|row| row.terra_dollars).fold(0.0, f64::max);
            sample.luna_dollars = rows.iter().map(|row| row.luna_dollars).fold(0.0, f64::max);
            sample.sol_tokens = rows.iter().map(|row| row.sol_tokens).max().unwrap_or(0);
            sample.terra_tokens = rows.iter().map(|row| row.terra_tokens).max().unwrap_or(0);
            sample.luna_tokens = rows.iter().map(|row| row.luna_tokens).max().unwrap_or(0);
            merged.push(sample);
        }
        merged
    }

    fn canonical_samples(&self) -> Vec<UsageHistorySample> {
        reset_sample_groups(&self.samples)
            .into_iter()
            .flat_map(|group| {
                group.samples.into_iter().map(move |mut sample| {
                    sample.reset_at = group.canonical_reset_at;
                    sample
                })
            })
            .collect()
    }

    fn normalize(&mut self) {
        self.samples.retain(UsageHistorySample::is_valid);
        self.samples
            .sort_by_key(|sample| (sample.reset_at, sample.timestamp));
        let mut normalized: Vec<UsageHistorySample> = Vec::with_capacity(self.samples.len());
        for sample in self.samples.drain(..) {
            if let Some(existing) = normalized.last_mut() {
                if existing.reset_at == sample.reset_at && existing.timestamp == sample.timestamp {
                    merge_sample_values(existing, sample);
                    continue;
                }
            }
            normalized.push(sample);
        }
        self.samples = normalized;
    }

    /// Bounds the in-memory/API/graph working set without deleting SQLite
    /// retention rows. Persistent deletion remains exclusively the three-month
    /// startup prune.
    fn retain_acquisition_window(&mut self, end_timestamp: i64) {
        let Some(end) = DateTime::<Utc>::from_timestamp(end_timestamp, 0) else {
            return;
        };
        let cutoff = one_month_before_utc(end);
        self.samples
            .retain(|sample| sample.timestamp > cutoff && sample.timestamp <= end_timestamp);
    }

    fn save(&self) {
        let Some(path) = &self.db_path else {
            return;
        };
        if let Ok(mut store) = UsageStore::open(path) {
            let samples = self
                .samples
                .iter()
                .map(UsageHistorySample::to_store)
                .collect::<Vec<_>>();
            let _ = store.upsert_samples(&samples);
        }
    }
}

fn usage_history_db_path() -> Option<PathBuf> {
    Some(
        usage_data_root()?
            .join("history")
            .join("usage_history.sqlite3"),
    )
}

fn default_codex_root() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn validated_configured_root(path: PathBuf) -> Option<PathBuf> {
    security::validate_absolute_root(&path).ok()
}

fn prepared_data_root(path: PathBuf) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if !path.exists() {
        let ancestor = path.ancestors().find(|ancestor| ancestor.exists())?;
        security::validate_absolute_root(ancestor).ok()?;
        fs::create_dir_all(&path).ok()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).ok()?;
    }
    validated_configured_root(path)
}

fn usage_data_root() -> Option<PathBuf> {
    let path = std::env::var_os("CODEX_INFO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_codex_root);
    prepared_data_root(path)
}

fn three_months_before_utc(now: DateTime<Utc>) -> i64 {
    now.checked_sub_months(Months::new(3))
        .expect("subtracting three calendar months from UTC now must be representable")
        .timestamp()
}

fn one_month_before_utc(now: DateTime<Utc>) -> i64 {
    now.checked_sub_months(Months::new(1))
        .expect("subtracting one calendar month from UTC now must be representable")
        .timestamp()
}

#[derive(Default)]
struct GraphPaths {
    remaining: String,
    remaining_markers: Vec<RemainingMarkerPosition>,
    unused_intervals: Vec<UnusedIntervalPosition>,
    sol: String,
    terra: String,
    luna: String,
    sol_flat: String,
    sol_rising: String,
    terra_flat: String,
    terra_rising: String,
    luna_flat: String,
    luna_rising: String,
    dollar_labels: [String; 5],
    current_remaining_label: String,
    current_sol_label: String,
    current_terra_label: String,
    current_luna_label: String,
    current_remaining_point_y: f32,
    current_sol_point_y: f32,
    current_terra_point_y: f32,
    current_luna_point_y: f32,
    current_remaining_y: f32,
    current_sol_y: f32,
    current_terra_y: f32,
    current_luna_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RemainingMarkerPosition {
    x: f64,
    y: f64,
    boundary: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct UnusedIntervalPosition {
    start: f64,
    width: f64,
    preserve_boundary: bool,
}

fn graph_paths(samples: &[&UsageHistorySample], period_start: i64, period_end: i64) -> GraphPaths {
    let remaining_points = remaining_graph_points(samples, period_start, period_end);
    let raw_minute = graph_time_endpoints(
        minute_model_spend_for_metric(samples, false),
        period_start,
        period_end,
    );
    let minute = smooth_model_spend(&raw_minute);
    // Dollar series are independent cumulative values.  The old stacked
    // implementation used the sum here, which made a model's line depend on
    // whether another model was enabled and could make a flat SOL history
    // appear to move.  A shared axis still gives the three lines a meaningful
    // comparison, but its ceiling is the largest individual model value.
    let dollar_max = minute
        .iter()
        .map(|point| point.sol.max(point.terra).max(point.luna))
        .fold(0.0_f64, f64::max);
    let has_model_data = dollar_max > 0.0;
    let latest = minute.last().copied().unwrap_or_default();
    let has_remaining_observation = samples
        .iter()
        .any(|sample| sample.remaining_percent.is_finite() && sample.remaining_percent >= 0.0);
    // Use the same smoothed endpoint that is rendered by `remaining-path` so
    // the right-edge percentage cannot disagree with the visible line after
    // a non-monotonic reread is clamped.
    let remaining = has_remaining_observation
        .then(|| remaining_points.last().map(|(_, value)| *value))
        .flatten();
    let graph_y = |value: f64, maximum: f64| -> f32 {
        if maximum > 0.0 {
            ((99.0 - value / maximum * 98.0) / 100.0).clamp(0.01, 0.99) as f32
        } else {
            0.99
        }
    };
    // Detect idle bands from raw cumulative snapshots, not smoothed lines.
    let unused_intervals = unused_interval_positions(&raw_minute, period_start, period_end);
    GraphPaths {
        remaining: graph_path_from_points(&remaining_points, period_start, period_end, 100.0),
        remaining_markers: remaining_marker_positions_on_points(
            &remaining_points,
            period_start,
            period_end,
        ),
        unused_intervals,
        luna: metric_line_path(&minute, period_start, period_end, dollar_max, |point| {
            point.luna
        }),
        terra: metric_line_path(&minute, period_start, period_end, dollar_max, |point| {
            point.terra
        }),
        sol: metric_line_path(&minute, period_start, period_end, dollar_max, |point| {
            point.sol
        }),
        dollar_labels: dollar_axis_labels(dollar_max),
        current_remaining_label: remaining.map(format_percent).unwrap_or_else(|| "—".into()),
        current_sol_label: if has_model_data {
            format!("${:.2}", latest.sol)
        } else {
            String::new()
        },
        current_terra_label: if has_model_data {
            format!("${:.2}", latest.terra)
        } else {
            String::new()
        },
        current_luna_label: if has_model_data {
            format!("${:.2}", latest.luna)
        } else {
            String::new()
        },
        current_remaining_y: graph_y(remaining.unwrap_or(0.0), 100.0),
        current_sol_y: graph_y(latest.sol, dollar_max),
        current_terra_y: graph_y(latest.terra, dollar_max),
        current_luna_y: graph_y(latest.luna, dollar_max),
        current_remaining_point_y: graph_y(remaining.unwrap_or(0.0), 100.0),
        current_sol_point_y: graph_y(latest.sol, dollar_max),
        current_terra_point_y: graph_y(latest.terra, dollar_max),
        current_luna_point_y: graph_y(latest.luna, dollar_max),
        ..GraphPaths::default()
    }
}

/// Builds a view from the monotonic cumulative snapshots. Flat and increasing
/// segments are kept in separate paths so the UI can render distinct widths.
fn graph_paths_for_selection(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
    show_luna: bool,
    show_terra: bool,
    show_sol: bool,
    show_tokens: bool,
) -> GraphPaths {
    let mut paths = graph_paths(samples, period_start, period_end);
    let minute = graph_time_endpoints(
        minute_model_spend_for_metric(samples, show_tokens),
        period_start,
        period_end,
    );
    // Keep the remaining line on the same activity metric as the visible
    // model lines. A legacy row with dollars but no token counters must not
    // make the token graph slope while every visible model line is flat.
    let remaining_points =
        remaining_graph_points_for_metric(samples, period_start, period_end, show_tokens);
    let has_remaining_observation = samples
        .iter()
        .any(|sample| sample.remaining_percent.is_finite() && sample.remaining_percent >= 0.0);
    if has_remaining_observation {
        if let Some(remaining) = remaining_points.last().map(|(_, value)| *value) {
            paths.remaining =
                graph_path_from_points(&remaining_points, period_start, period_end, 100.0);
            paths.remaining_markers =
                remaining_marker_positions_on_points(&remaining_points, period_start, period_end);
            paths.current_remaining_label = format_percent(remaining);
            let normalized = ((99.0 - remaining * 0.98) / 100.0).clamp(0.01, 0.99) as f32;
            paths.current_remaining_y = normalized;
            paths.current_remaining_point_y = normalized;
        }
    }
    paths.unused_intervals = unused_interval_positions(&minute, period_start, period_end);
    let maximum = minute
        .iter()
        .map(|point| {
            if show_tokens {
                point.sol.max(point.terra).max(point.luna)
            } else {
                [
                    show_luna.then_some(point.luna),
                    show_terra.then_some(point.terra),
                    show_sol.then_some(point.sol),
                ]
                .into_iter()
                .flatten()
                .fold(0.0_f64, f64::max)
            }
        })
        .fold(0.0_f64, f64::max);
    let scale_maximum = maximum.max(1.0);
    let latest = minute.last().copied().unwrap_or_default();
    paths.dollar_labels = if show_tokens {
        token_axis_labels(scale_maximum)
    } else {
        dollar_axis_labels(scale_maximum)
    };
    paths.sol.clear();
    paths.terra.clear();
    paths.luna.clear();
    paths.sol_flat.clear();
    paths.sol_rising.clear();
    paths.terra_flat.clear();
    paths.terra_rising.clear();
    paths.luna_flat.clear();
    paths.luna_rising.clear();
    paths.current_sol_label.clear();
    paths.current_terra_label.clear();
    paths.current_luna_label.clear();
    paths.current_sol_point_y = 0.99;
    paths.current_terra_point_y = 0.99;
    paths.current_luna_point_y = 0.99;
    paths.current_sol_y = 0.99;
    paths.current_terra_y = 0.99;
    paths.current_luna_y = 0.99;
    let graph_y =
        |value: f64| ((99.0 - value / scale_maximum * 98.0) / 100.0).clamp(0.01, 0.99) as f32;
    if show_luna {
        (paths.luna_flat, paths.luna_rising) =
            split_metric_line_paths(&minute, period_start, period_end, scale_maximum, |point| {
                point.luna
            });
        paths.luna = metric_line_path(&minute, period_start, period_end, scale_maximum, |point| {
            point.luna
        });
        paths.current_luna_label = if maximum > 0.0 {
            format_metric_value(latest.luna, show_tokens)
        } else {
            String::new()
        };
        paths.current_luna_y = graph_y(latest.luna);
        paths.current_luna_point_y = paths.current_luna_y;
    }
    if show_terra {
        (paths.terra_flat, paths.terra_rising) =
            split_metric_line_paths(&minute, period_start, period_end, scale_maximum, |point| {
                point.terra
            });
        paths.terra = metric_line_path(&minute, period_start, period_end, scale_maximum, |point| {
            point.terra
        });
        paths.current_terra_label = if maximum > 0.0 {
            format_metric_value(latest.terra, show_tokens)
        } else {
            String::new()
        };
        paths.current_terra_y = graph_y(latest.terra);
        paths.current_terra_point_y = paths.current_terra_y;
    }
    if show_sol {
        (paths.sol_flat, paths.sol_rising) =
            split_metric_line_paths(&minute, period_start, period_end, scale_maximum, |point| {
                point.sol
            });
        paths.sol = metric_line_path(&minute, period_start, period_end, scale_maximum, |point| {
            point.sol
        });
        paths.current_sol_label = if maximum > 0.0 {
            format_metric_value(latest.sol, show_tokens)
        } else {
            String::new()
        };
        paths.current_sol_y = graph_y(latest.sol);
        paths.current_sol_point_y = paths.current_sol_y;
    }
    paths
}

#[cfg(test)]
fn graph_paths_for_model(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
    model: &str,
) -> GraphPaths {
    graph_paths_for_selection(
        samples,
        period_start,
        period_end,
        model == "ALL" || model == "LUNA",
        model == "ALL" || model == "TERRA",
        model == "ALL" || model == "SOL",
        false,
    )
}

#[cfg(test)]
fn remaining_marker_positions(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
) -> Vec<RemainingMarkerPosition> {
    let points = graph_points(samples, period_start, period_end, 100.0, |sample| {
        sample.remaining_percent
    });
    remaining_marker_positions_on_points(&points, period_start, period_end)
}

fn remaining_marker_positions_on_points(
    points: &[(i64, f64)],
    period_start: i64,
    period_end: i64,
) -> Vec<RemainingMarkerPosition> {
    let span = (period_end - period_start).max(1) as f64;
    let mut markers = Vec::new();
    let mut seen_boundaries = BTreeSet::new();
    let Some(&(mut previous_timestamp, mut previous_value)) = points.first() else {
        return markers;
    };

    // Pathと同じ平滑化済み点列を走査し、各整数%境界をその線分上で補間する。
    for &(timestamp, current) in points.iter().skip(1) {
        if timestamp >= previous_timestamp && current < previous_value {
            let mut boundary = previous_value.floor() as i32;
            if (previous_value - boundary as f64).abs() <= f64::EPSILON {
                boundary -= 1;
            }
            let lowest_boundary = current.ceil() as i32;
            while boundary >= lowest_boundary {
                let boundary_value = boundary as f64;
                if boundary_value < previous_value
                    && boundary_value >= current
                    && seen_boundaries.insert(boundary)
                {
                    let fraction = ((boundary_value - previous_value) / (current - previous_value))
                        .clamp(0.0, 1.0);
                    let marker_timestamp = previous_timestamp as f64
                        + (timestamp - previous_timestamp) as f64 * fraction;
                    let x =
                        ((marker_timestamp - period_start as f64) / span * 100.0).clamp(0.0, 100.0);
                    markers.push(RemainingMarkerPosition {
                        x,
                        y: remaining_graph_y(boundary_value),
                        boundary,
                    });
                }
                boundary -= 1;
            }
        }
        previous_timestamp = timestamp;
        previous_value = current;
    }

    markers
}

fn graph_time_endpoints(
    points: Vec<HourlyModelSpend>,
    period_start: i64,
    period_end: i64,
) -> Vec<HourlyModelSpend> {
    if points.is_empty() {
        return points;
    }
    let mut extended = Vec::with_capacity(points.len() + 2);
    // セッション開始から最初の記録までは累積0を明示する。
    extended.push(HourlyModelSpend {
        timestamp: period_start,
        ..HourlyModelSpend::default()
    });
    for point in points
        .iter()
        .copied()
        .filter(|point| point.timestamp >= period_start && point.timestamp < period_end)
    {
        if let Some(last) = extended.last_mut() {
            if last.timestamp == point.timestamp {
                *last = point;
                continue;
            }
        }
        extended.push(point);
    }
    // 時間バケットの途中で終わらず、期間の観測終端を右端に固定する。
    // 期間外（たとえば reset 後に遅れて届いた未来行）の値を terminal
    // endpoint へ持ち込まない。歴史期間の canonical reset 観測は
    // `<= period_end` なので、X と Windows が同じ終端値を描く。
    if let Some(last) = points
        .iter()
        .copied()
        .rfind(|point| point.timestamp <= period_end)
    {
        let endpoint = HourlyModelSpend {
            timestamp: period_end,
            ..last
        };
        if let Some(existing) = extended.last_mut() {
            if existing.timestamp == period_end {
                *existing = endpoint;
            } else {
                extended.push(endpoint);
            }
        } else {
            extended.push(endpoint);
        }
    }
    extended
}

#[derive(Clone, Copy, Debug, Default)]
struct HourlyModelSpend {
    timestamp: i64,
    sol: f64,
    terra: f64,
    luna: f64,
}

#[cfg(test)]
fn minute_model_spend(samples: &[&UsageHistorySample]) -> Vec<HourlyModelSpend> {
    minute_model_spend_for_metric(samples, false)
}

fn minute_model_spend_for_metric(
    samples: &[&UsageHistorySample],
    show_tokens: bool,
) -> Vec<HourlyModelSpend> {
    let mut buckets: Vec<UsageHistorySample> = Vec::new();
    for sample in samples {
        let minute = sample.timestamp.div_euclid(60) * 60;
        let mut sample = (*sample).clone();
        sample.timestamp = minute;
        if let Some(previous) = buckets.last_mut() {
            if previous.timestamp == minute {
                *previous = sample;
                continue;
            }
        }
        buckets.push(sample);
    }
    if show_tokens {
        // The raw session counters are cumulative totals.  Older history
        // rows can contain zero because the token fields did not exist (or a
        // provider did not report them), so a zero after a known value must be
        // treated as an unknown sample and carried forward.  Taking the
        // maximum also protects the graph from stale/out-of-order rows.
        let mut cumulative = [0.0_f64; 3];
        return buckets
            .into_iter()
            .map(|sample| {
                let current = [
                    sample.sol_tokens as f64,
                    sample.terra_tokens as f64,
                    sample.luna_tokens as f64,
                ];
                for index in 0..3 {
                    if current[index] > cumulative[index] {
                        cumulative[index] = current[index];
                    }
                }
                HourlyModelSpend {
                    timestamp: sample.timestamp,
                    sol: cumulative[0],
                    terra: cumulative[1],
                    luna: cumulative[2],
                }
            })
            .collect();
    }

    let mut cumulative = [0.0_f64; 3];
    buckets
        .into_iter()
        .map(|sample| {
            let current = [
                sample.sol_dollars,
                sample.terra_dollars,
                sample.luna_dollars,
            ];
            for index in 0..3 {
                // Dollar history is also persisted as a cumulative snapshot.
                // A later API scan can temporarily report a smaller snapshot
                // (for example while a session file is still being indexed),
                // so never add the positive difference twice after such a
                // regression. Keep the greatest observed cumulative value,
                // just as the token path does above.
                if current[index] > cumulative[index] {
                    cumulative[index] = current[index];
                }
            }
            HourlyModelSpend {
                timestamp: sample.timestamp,
                sol: cumulative[0],
                terra: cumulative[1],
                luna: cumulative[2],
            }
        })
        .collect()
}

#[cfg(test)]
fn stacked_area_path(
    points: &[HourlyModelSpend],
    period_start: i64,
    period_end: i64,
    maximum: f64,
    bounds: impl Fn(&HourlyModelSpend) -> (f64, f64),
) -> String {
    if points.is_empty() {
        return String::new();
    }
    // すべて$0の期間も、下端に0基線を描いて「データなし」と区別する。
    if maximum <= 0.0 {
        return "M0.00 99.00 L100.00 99.00".into();
    }
    let span = (period_end - period_start).max(1) as f64;
    let coordinate = |timestamp: i64, value: f64| {
        let x = ((timestamp - period_start) as f64 / span * 100.0).clamp(0.0, 100.0);
        // strokeがclip領域の外へ半分切れないよう、0/最大値を内側へ1%だけ寄せる。
        let y = (99.0 - value / maximum * 98.0).clamp(1.0, 99.0);
        (x, y)
    };
    let upper = points
        .iter()
        .map(|point| coordinate(point.timestamp, bounds(point).1))
        .collect::<Vec<_>>();
    let lower = points
        .iter()
        .rev()
        .map(|point| coordinate(point.timestamp, bounds(point).0))
        .collect::<Vec<_>>();
    let mut commands = format!("M{:.2} {:.2}", upper[0].0, upper[0].1);
    for (x, y) in upper.iter().skip(1) {
        commands.push_str(&format!(" L{x:.2} {y:.2}"));
    }
    for (x, y) in lower {
        commands.push_str(&format!(" L{x:.2} {y:.2}"));
    }
    commands.push_str(" Z");
    commands
}

/// Draws one metric independently from the other model series. Token mode
/// uses this path so enabling LUNA cannot turn SOL into a LUNA+SOL boundary.
fn metric_line_path(
    points: &[HourlyModelSpend],
    period_start: i64,
    period_end: i64,
    maximum: f64,
    value: impl Fn(&HourlyModelSpend) -> f64,
) -> String {
    if points.is_empty() {
        return String::new();
    }
    if maximum <= 0.0 {
        return "M0.00 99.00 L100.00 99.00".into();
    }
    let span = (period_end - period_start).max(1) as f64;
    let coordinate = |timestamp: i64, raw: f64| {
        let x = ((timestamp - period_start) as f64 / span * 100.0).clamp(0.0, 100.0);
        let y = (99.0 - raw.max(0.0) / maximum * 98.0).clamp(1.0, 99.0);
        (x, y)
    };
    let mut iter = points.iter();
    let first = iter.next().expect("points is not empty");
    let (x, y) = coordinate(first.timestamp, value(first));
    let mut commands = format!("M{x:.2} {y:.2}");
    let mut previous = first;
    for point in iter {
        let (x, y) = coordinate(point.timestamp, value(point));
        let unobserved_gap = point.timestamp.saturating_sub(previous.timestamp)
            > MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS;
        if unobserved_gap && value(point) > value(previous) {
            let (_, previous_y) = coordinate(point.timestamp, value(previous));
            commands.push_str(&format!(" L{x:.2} {previous_y:.2} L{x:.2} {y:.2}"));
        } else {
            commands.push_str(&format!(" L{x:.2} {y:.2}"));
        }
        previous = point;
    }
    commands
}

fn split_metric_line_paths(
    points: &[HourlyModelSpend],
    period_start: i64,
    period_end: i64,
    maximum: f64,
    value: impl Fn(&HourlyModelSpend) -> f64,
) -> (String, String) {
    let span = (period_end - period_start).max(1) as f64;
    let scale = maximum.max(1.0);
    let coordinate = |point: &HourlyModelSpend| {
        let x = ((point.timestamp - period_start) as f64 / span * 100.0).clamp(0.0, 100.0);
        let y = (99.0 - value(point).max(0.0) / scale * 98.0).clamp(1.0, 99.0);
        (x, y)
    };
    let mut flat = String::new();
    let mut rising = String::new();
    for pair in points.windows(2) {
        let previous = value(&pair[0]);
        let current = value(&pair[1]);
        if !previous.is_finite() || !current.is_finite() || current < previous {
            continue;
        }
        let (x1, y1) = coordinate(&pair[0]);
        let (x2, y2) = coordinate(&pair[1]);
        // The reset anchor is synthetic when the first observation arrives
        // later. Keep the unknown interval at zero and show the observed
        // increase at its actual timestamp instead of implying a diagonal
        // increase throughout the unobserved interval.
        if pair[0].timestamp == period_start
            && pair[1].timestamp.saturating_sub(pair[0].timestamp) > 60
            && previous == 0.0
            && current > 0.0
        {
            if !flat.is_empty() {
                flat.push(' ');
            }
            flat.push_str(&format!("M{x1:.2} {y1:.2} L{x2:.2} {y1:.2}"));
            if !rising.is_empty() {
                rising.push(' ');
            }
            rising.push_str(&format!("M{x2:.2} {y1:.2} L{x2:.2} {y2:.2}"));
            continue;
        }
        let unobserved_gap = pair[1].timestamp.saturating_sub(pair[0].timestamp)
            > MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS;
        if unobserved_gap && current > previous {
            if !flat.is_empty() {
                flat.push(' ');
            }
            flat.push_str(&format!("M{x1:.2} {y1:.2} L{x2:.2} {y1:.2}"));
            if !rising.is_empty() {
                rising.push(' ');
            }
            rising.push_str(&format!("M{x2:.2} {y1:.2} L{x2:.2} {y2:.2}"));
            continue;
        }
        let target = if current == previous {
            &mut flat
        } else {
            &mut rising
        };
        if !target.is_empty() {
            target.push(' ');
        }
        target.push_str(&format!("M{x1:.2} {y1:.2} L{x2:.2} {y2:.2}"));
    }
    (flat, rising)
}

/// Return horizontal bands where none of the three cumulative model series
/// changes. These bands make idle time visible even when all flat paths sit on
/// top of one another at the chart baseline.
fn unused_interval_positions(
    points: &[HourlyModelSpend],
    period_start: i64,
    period_end: i64,
) -> Vec<UnusedIntervalPosition> {
    let span = (period_end - period_start).max(1) as f64;
    let to_x =
        |timestamp: i64| ((timestamp - period_start) as f64 / span * 100.0).clamp(0.0, 100.0);
    let mut intervals: Vec<UnusedIntervalPosition> = Vec::new();
    for pair in points.windows(2) {
        let [previous, current] = pair else {
            continue;
        };
        if current.timestamp <= previous.timestamp {
            continue;
        }
        let interval_start = previous.timestamp.max(period_start);
        let interval_end = current.timestamp.min(period_end);
        if interval_end <= interval_start {
            continue;
        }
        let unchanged = [
            (previous.sol, current.sol),
            (previous.terra, current.terra),
            (previous.luna, current.luna),
        ]
        .into_iter()
        .all(|(before, after)| before.is_finite() && after.is_finite() && before == after);
        let synthetic_zero_gap = previous.timestamp == period_start
            && current.timestamp.saturating_sub(previous.timestamp) > 60
            && previous.sol == 0.0
            && previous.terra == 0.0
            && previous.luna == 0.0
            && [current.sol, current.terra, current.luna]
                .into_iter()
                .any(|value| value.is_finite() && value > 0.0);
        // A long observation gap ending at a later cumulative value contains
        // no evidence of when usage occurred. Render the whole unobserved
        // interval as idle, then let the model line make the vertical change
        // at the observed endpoint; never leave daytime gaps unmarked.
        let unobserved_active_gap = current.timestamp.saturating_sub(previous.timestamp)
            > MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS
            && [
                (previous.sol, current.sol),
                (previous.terra, current.terra),
                (previous.luna, current.luna),
            ]
            .into_iter()
            .any(|(before, after)| after > before);
        if !unchanged && !synthetic_zero_gap && !unobserved_active_gap {
            continue;
        }
        let start = to_x(interval_start);
        let end = to_x(interval_end);
        if end <= start {
            continue;
        }
        if let Some(last) = intervals.last_mut() {
            let last_end = last.start + last.width;
            if !last.preserve_boundary
                && !synthetic_zero_gap
                && !unobserved_active_gap
                && (last_end - start).abs() <= f64::EPSILON
            {
                last.width = end - last.start;
                continue;
            }
        }
        intervals.push(UnusedIntervalPosition {
            start,
            width: end - start,
            preserve_boundary: synthetic_zero_gap || unobserved_active_gap,
        });
    }
    intervals
}

/// Keeps all visible right-edge labels inside the plot and at least 16px
/// apart at the minimum 204px path height. GraphWindow's 700x480 minimum
/// produces that path height; resizing only increases the physical spacing.
fn separate_current_label_positions(
    paths: &mut GraphPaths,
    show_remaining: bool,
    show_luna: bool,
    show_terra: bool,
    show_sol: bool,
) {
    const MIN_PATH_HEIGHT: f32 = 204.0;
    const HALF_LABEL: f32 = 8.0 / MIN_PATH_HEIGHT;
    const MIN_SEPARATION: f32 = 16.0 / MIN_PATH_HEIGHT;
    const LOWER: f32 = HALF_LABEL;
    const UPPER: f32 = 1.0 - HALF_LABEL;

    let mut labels = Vec::with_capacity(4);
    if show_remaining && !paths.current_remaining_label.is_empty() {
        labels.push((0_u8, paths.current_remaining_y));
    }
    if show_luna && !paths.current_luna_label.is_empty() {
        labels.push((1, paths.current_luna_y));
    }
    if show_terra && !paths.current_terra_label.is_empty() {
        labels.push((2, paths.current_terra_y));
    }
    if show_sol && !paths.current_sol_label.is_empty() {
        labels.push((3, paths.current_sol_y));
    }
    labels.sort_by(|left, right| {
        left.1
            .total_cmp(&right.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    if labels.is_empty() {
        return;
    }

    labels[0].1 = labels[0].1.clamp(LOWER, UPPER);
    for index in 1..labels.len() {
        labels[index].1 = labels[index]
            .1
            .clamp(LOWER, UPPER)
            .max(labels[index - 1].1 + MIN_SEPARATION);
    }
    if labels.last().is_some_and(|label| label.1 > UPPER) {
        let last = labels.len() - 1;
        labels[last].1 = UPPER;
        for index in (0..last).rev() {
            labels[index].1 = labels[index].1.min(labels[index + 1].1 - MIN_SEPARATION);
        }
    }

    for (kind, position) in labels {
        match kind {
            0 => paths.current_remaining_y = position,
            1 => paths.current_luna_y = position,
            2 => paths.current_terra_y = position,
            3 => paths.current_sol_y = position,
            _ => unreachable!("label kind is internal and bounded"),
        }
    }
}

/// Draw a short, color-matched leader from the series endpoint to its
/// right-edge label. Labels may be vertically separated to avoid overlap, so
/// the connector preserves the correspondence without stacking text on top of
/// another value.
fn current_label_connector_path(point_y: f32, label_y: f32, has_label: bool) -> String {
    if !has_label || !point_y.is_finite() || !label_y.is_finite() {
        return String::new();
    }
    let point_y = point_y.clamp(0.0, 1.0) * 100.0;
    let label_y = label_y.clamp(0.0, 1.0) * 100.0;
    format!("M0.00 {point_y:.2} L100.00 {label_y:.2}")
}

fn dollar_axis_labels(maximum: f64) -> [String; 5] {
    [1.0, 0.75, 0.5, 0.25, 0.0].map(|fraction| format!("${:.2}", maximum * fraction))
}

fn token_axis_labels(maximum: f64) -> [String; 5] {
    [1.0, 0.75, 0.5, 0.25, 0.0].map(|fraction| format_token_axis_value(maximum * fraction))
}

fn format_token_axis_value(value: f64) -> String {
    let value = value.max(0.0);
    if value >= 1_000_000_000.0 {
        format!("{:.1}B", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else {
        format_token_count(value.round() as u64)
    }
}

fn format_metric_value(value: f64, show_tokens: bool) -> String {
    if show_tokens {
        format_token_count(value.max(0.0).round() as u64)
    } else {
        format!("${value:.2}")
    }
}

#[cfg(test)]
fn graph_points(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
    maximum: f64,
    value: impl Fn(&UsageHistorySample) -> f64,
) -> Vec<(i64, f64)> {
    smooth_remaining_points(&collapse_remaining_change_points(&raw_graph_points(
        samples,
        period_start,
        period_end,
        maximum,
        value,
    )))
}

#[cfg(test)]
fn raw_graph_points(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
    maximum: f64,
    value: impl Fn(&UsageHistorySample) -> f64,
) -> Vec<(i64, f64)> {
    let mut observed = samples
        .iter()
        .filter_map(|sample| {
            let raw = value(sample);
            (raw.is_finite() && raw >= 0.0).then_some((sample.timestamp, raw))
        })
        .collect::<Vec<_>>();
    observed.sort_by_key(|(timestamp, _)| *timestamp);
    let has_observation = !observed.is_empty();

    // リセット開始時点は仕様上、残り利用枠100%である。最初の実測値が
    // 取得された時刻より後でも、グラフの左端を欠落させない。
    let mut points = vec![(period_start, maximum)];
    for (timestamp, raw) in observed {
        let timestamp = timestamp.clamp(period_start, period_end);
        if points
            .last()
            .is_some_and(|(last_timestamp, _)| *last_timestamp == timestamp)
        {
            points.last_mut().expect("points has an anchor").1 = raw;
            continue;
        }
        points.push((timestamp, raw));
    }

    if has_observation {
        if let Some((last_timestamp, last_raw)) = points.last().copied() {
            if last_timestamp < period_end {
                // 最新の実測値は現在時刻まで水平に保持する。未知の値を
                // 現在時刻へ斜めに補間しない。
                points.push((period_end, last_raw));
            }
        }
    }
    points
}

/// Builds the remaining-quota line from the same cumulative model snapshots
/// used by the SOL/TERRA/LUNA lines. Model snapshots define active versus idle
/// intervals: idle intervals stay horizontal, while remaining samples in
/// contiguous active intervals are connected by straight lines. If model
/// usage advances while a quota reread repeats, the repeated value is treated
/// as a stale/missed sample and interpolated between the surrounding changes;
/// a `1 -> 1 -> 3` sequence therefore becomes `1 -> 2 -> 3`, not a false
/// horizontal-then-drop corner. A first lower quota observation arriving
/// after an unobserved active interval closes that interval even when the
/// model snapshot has already stopped changing; a genuinely idle period that
/// never had model usage remains horizontal.
fn remaining_graph_points(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
) -> Vec<(i64, f64)> {
    remaining_graph_points_for_metric(samples, period_start, period_end, false)
}

fn remaining_graph_points_for_metric(
    samples: &[&UsageHistorySample],
    period_start: i64,
    period_end: i64,
    show_tokens: bool,
) -> Vec<(i64, f64)> {
    // A history read is normally period-scoped before it reaches this
    // function. Keep a second, fail-closed boundary here as well: legacy
    // databases can contain two different reset periods at the same minute.
    // If those rows disagree, choosing the last row would manufacture a
    // vertical quota drop (for example 88% -> 14% with no model usage).
    // Ignore the conflicting timestamp until a period-scoped observation is
    // available instead of inventing a value from row order.
    let mut remaining_candidates = BTreeMap::<i64, Vec<f64>>::new();
    for sample in samples {
        let value = sample.remaining_percent;
        if value.is_finite() && value >= 0.0 {
            let timestamp = sample.timestamp.clamp(period_start, period_end);
            remaining_candidates
                .entry(timestamp)
                .or_default()
                .push(value);
        }
    }
    let scoped_remaining = remaining_candidates
        .into_iter()
        .filter_map(|(timestamp, values)| {
            let first = values.first().copied()?;
            let conflicting = values
                .iter()
                .any(|value| (value - first).abs() > f64::EPSILON);
            (!conflicting).then_some((timestamp, first.clamp(0.0, 100.0)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut raw_remaining = vec![(period_start, 100.0)];
    for (timestamp, value) in scoped_remaining.iter() {
        if *timestamp == period_start {
            raw_remaining[0].1 = *value;
        } else {
            raw_remaining.push((*timestamp, *value));
        }
    }
    if scoped_remaining
        .keys()
        .next_back()
        .is_some_and(|timestamp| *timestamp < period_end)
    {
        let last_value = raw_remaining
            .last()
            .map(|(_, value)| *value)
            .unwrap_or(100.0);
        raw_remaining.push((period_end, last_value));
    }
    if raw_remaining.len() < 2 {
        return raw_remaining;
    }

    let model_points = graph_time_endpoints(
        minute_model_spend_for_metric(samples, show_tokens),
        period_start,
        period_end,
    );
    let remaining_by_timestamp = raw_remaining.iter().copied().collect::<BTreeMap<_, _>>();

    let initial_remaining = remaining_by_timestamp
        .get(&period_start)
        .copied()
        .unwrap_or(100.0)
        .clamp(0.0, 100.0);
    if model_points.len() < 2 {
        return vec![
            (period_start, initial_remaining),
            (period_end, initial_remaining),
        ];
    }

    // Walk the same minute intervals used by the model paths. A segment is
    // active only when at least one cumulative model value increases across
    // that interval; quota rereads in an idle segment are ignored. Idle model
    // intervals remain horizontal, while repeated active quota samples are
    // completed by interpolation in the smoothing pass below.
    let mut points = vec![(period_start, initial_remaining)];
    let mut active_segments = Vec::with_capacity(model_points.len() + 1);
    let mut previous_remaining = initial_remaining;
    let mut previous_model = model_points[0];
    let mut previous_timestamp = period_start;
    // A quota value can arrive after the last model snapshot (the recorder
    // and quota poll are independent). Keep track of whether a real quota
    // endpoint has already been observed since the latest model change; only
    // the first lower value after an unobserved active interval may close that
    // interval. This preserves genuinely idle flat segments while ensuring a
    // delayed 1% quota response is not discarded merely because model totals
    // have stopped changing.
    let mut quota_observed_since_model_change = true;
    for current_model in model_points.iter().copied().skip(1) {
        let timestamp = current_model.timestamp;
        if timestamp <= previous_timestamp {
            previous_model = current_model;
            continue;
        }
        let model_changed = current_model.sol > previous_model.sol
            || current_model.terra > previous_model.terra
            || current_model.luna > previous_model.luna;
        let synthetic_zero_gap = previous_timestamp == period_start
            && timestamp.saturating_sub(previous_timestamp) > 60
            && previous_model.sol == 0.0
            && previous_model.terra == 0.0
            && previous_model.luna == 0.0
            && model_changed;
        let active = model_changed && !synthetic_zero_gap;
        let matched_remaining = if model_changed {
            latest_remaining_for_model_change(
                &remaining_by_timestamp,
                timestamp,
                Some(previous_timestamp),
            )
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0.0, 100.0))
        } else {
            None
        };
        let delayed_quota = if !model_changed && !quota_observed_since_model_change {
            remaining_by_timestamp
                .range((previous_timestamp + 1)..=timestamp)
                .next_back()
                .map(|(_, value)| *value)
                .filter(|value| value.is_finite() && *value < previous_remaining)
                .map(|value| value.clamp(0.0, 100.0))
        } else {
            None
        };
        let matched_quota = matched_remaining.is_some();
        let delayed_quota_applied = delayed_quota.is_some();
        let next_remaining = matched_remaining
            .or(delayed_quota)
            .map(|value| previous_remaining.min(value))
            .unwrap_or(previous_remaining);
        if model_changed {
            quota_observed_since_model_change = matched_quota;
        } else if delayed_quota_applied {
            quota_observed_since_model_change = true;
        }

        if synthetic_zero_gap {
            // Keep an unobserved reset-to-first-use gap horizontal, then make
            // the first observed quota value explicit at its real timestamp.
            points.push((timestamp, previous_remaining));
            active_segments.push(false);
            if next_remaining != previous_remaining {
                points.push((timestamp, next_remaining));
                active_segments.push(false);
            }
        } else {
            points.push((timestamp, next_remaining));
            active_segments.push(active);
        }
        previous_remaining = next_remaining;
        previous_model = current_model;
        previous_timestamp = timestamp;
    }

    if previous_timestamp < period_end {
        points.push((period_end, previous_remaining));
        active_segments.push(false);
    }

    collapse_repeated_idle_remaining_points(&mut points, &mut active_segments);
    smooth_remaining_points_with_activity(&points, &active_segments)
}

fn collapse_repeated_idle_remaining_points(
    points: &mut Vec<(i64, f64)>,
    active_segments: &mut Vec<bool>,
) {
    let mut index = 1;
    while index + 1 < points.len() {
        let repeated = points[index - 1].1 == points[index].1;
        let is_vertical_boundary =
            points[index - 1].0 == points[index].0 || points[index].0 == points[index + 1].0;
        let joins_idle_segments = active_segments.get(index - 1) == Some(&false)
            && active_segments.get(index) == Some(&false);
        if repeated && !is_vertical_boundary && joins_idle_segments {
            points.remove(index);
            active_segments.remove(index);
        } else {
            index += 1;
        }
    }
}

fn latest_remaining_for_model_change(
    remaining_by_timestamp: &BTreeMap<i64, f64>,
    change_timestamp: i64,
    previous_change: Option<i64>,
) -> Option<f64> {
    // Model snapshots are grouped into minute buckets before their change
    // points are drawn. Match quota observations to that same bucket so a
    // sample at t=100 is not lost when its model point is drawn at t=60.
    let change_bucket = change_timestamp.div_euclid(60) * 60;
    let bucket_candidate = remaining_by_timestamp
        .iter()
        .filter(|(observed_at, value)| {
            value.is_finite()
                && observed_at.div_euclid(60) * 60 == change_bucket
                && previous_change.is_none_or(|previous| {
                    observed_at.div_euclid(60) * 60 > previous.div_euclid(60) * 60
                        || **observed_at > previous
                })
        })
        .map(|(_, value)| *value)
        .next_back();
    if bucket_candidate.is_some() {
        return bucket_candidate;
    }

    // If a bucket has no quota reread, a later observation in the active
    // interval is still a valid endpoint. This fallback is deliberately not
    // used for the first model change: pre-change observations belong to an
    // idle interval and must not create a delayed slope.
    previous_change.and_then(|previous| {
        remaining_by_timestamp
            .iter()
            .filter(|(observed_at, value)| {
                value.is_finite() && **observed_at > previous && **observed_at <= change_timestamp
            })
            .map(|(_, value)| *value)
            .next_back()
    })
}

fn smooth_remaining_points_with_activity(
    points: &[(i64, f64)],
    active_segments: &[bool],
) -> Vec<(i64, f64)> {
    debug_assert_eq!(active_segments.len(), points.len().saturating_sub(1));
    if points.len() < 3 {
        return points.to_vec();
    }

    let mut smoothed = points.to_vec();
    let mut interpolated = vec![false; points.len()];
    let active_duration_between = |start: usize, end: usize| {
        (start..end)
            .filter(|segment| active_segments.get(*segment).copied().unwrap_or(false))
            .map(|segment| {
                points[segment + 1]
                    .0
                    .saturating_sub(points[segment].0)
                    .max(0) as f64
            })
            .sum::<f64>()
    };
    // A model can advance through several minute buckets while the quota
    // endpoint is still the previous reread. Treat a repeated value as a
    // missed sample only when it is bounded by a later, observed lower value.
    // For example, `1 -> 1 -> 3` becomes `1 -> 2 -> 3`. A terminal plateau
    // remains at the latest observation: model activity is not evidence that
    // the service's quota reading has fallen.
    //
    // The search deliberately crosses short idle intervals. The line must not
    // slope while every model is idle, but an idle interval between two active
    // samples must not prevent the active samples on either side from being
    // completed. Interpolation therefore advances by active duration rather
    // than wall-clock duration: active segments slope, idle segments hold the
    // last value.
    for index in 1..points.len() {
        if interpolated[index] {
            continue;
        }
        if points[index - 1].0 == points[index].0 || points[index - 1].1 != points[index].1 {
            continue;
        }

        let left_index = index - 1;
        let left_value = points[left_index].1;
        let mut right_index = index + 1;
        while right_index < points.len() && points[right_index].1 >= left_value {
            right_index += 1;
        }
        if right_index >= points.len() {
            // There is no later observed lower quota value. Do not turn the
            // latest real reading into a consumption-rate estimate.
            continue;
        }
        if right_index <= left_index || points[right_index].0 <= points[left_index].0 {
            continue;
        }

        let active_duration = active_duration_between(left_index, right_index);
        if active_duration <= f64::EPSILON {
            continue;
        }
        let right_value = points[right_index].1;
        if right_value >= left_value {
            continue;
        };

        let mut active_elapsed = 0.0;
        for point_index in index..right_index {
            if point_index > left_index
                && active_segments
                    .get(point_index - 1)
                    .copied()
                    .unwrap_or(false)
            {
                active_elapsed += points[point_index]
                    .0
                    .saturating_sub(points[point_index - 1].0)
                    .max(0) as f64;
            }
            let fraction = (active_elapsed / active_duration).clamp(0.0, 1.0);
            smoothed[point_index].1 = left_value + (right_value - left_value) * fraction;
            interpolated[point_index] = true;
        }
        // For a measured lower endpoint the loop above stops before
        // right_index, so a later pass can use it as the next anchor without
        // accumulating floating-point error.
    }

    for index in 1..points.len() - 1 {
        let before_active = active_segments.get(index - 1).copied().unwrap_or(false);
        let after_active = active_segments.get(index).copied().unwrap_or(false);
        if !before_active
            || !after_active
            || points[index - 1].0 == points[index].0
            || points[index].0 == points[index + 1].0
            // Keep the explicitly interpolated line intact. A second moving
            // average would move its surrounding anchors and reintroduce a
            // fold at the edge of the completed sampling gap.
            || interpolated[index.saturating_sub(1)]
            || interpolated[index]
            || interpolated[index + 1]
        {
            continue;
        }
        let average = (smoothed[index - 1].1 + (2.0 * points[index].1) + points[index + 1].1) / 4.0;
        smoothed[index].1 = average.min(smoothed[index - 1].1);
    }

    for index in 1..smoothed.len() {
        let active = active_segments.get(index - 1).copied().unwrap_or(false);
        if !active && smoothed[index - 1].0 != smoothed[index].0 {
            smoothed[index].1 = smoothed[index - 1].1;
        } else if active {
            smoothed[index].1 = smoothed[index].1.min(smoothed[index - 1].1);
        }
    }
    smoothed
}

#[cfg(test)]
fn collapse_remaining_change_points(points: &[(i64, f64)]) -> Vec<(i64, f64)> {
    if points.len() < 2 {
        return points.to_vec();
    }

    let mut collapsed = Vec::with_capacity(points.len());
    collapsed.push(points[0]);
    for (index, &(timestamp, raw)) in points.iter().enumerate().skip(1) {
        let previous = collapsed.last().expect("first point is present").1;
        // Remaining quota is monotonic between resets. Clamp a transient
        // upward reread before deciding whether this is a visible change.
        let value = raw.min(previous);
        let is_period_end = index + 1 == points.len();
        if value < previous || is_period_end {
            collapsed.push((timestamp, value));
        }
    }
    collapsed
}

fn graph_path_from_points(
    points: &[(i64, f64)],
    period_start: i64,
    period_end: i64,
    maximum: f64,
) -> String {
    let span = (period_end - period_start).max(1) as f64;
    points
        .iter()
        .enumerate()
        .map(|(index, (timestamp, raw))| {
            let x = ((timestamp - period_start) as f64 / span * 100.0).clamp(0.0, 100.0);
            let y = if maximum > 0.0 {
                (99.0 - raw / maximum * 98.0).clamp(1.0, 99.0)
            } else {
                99.0
            };
            let command = if index == 0 { "M" } else { "L" };
            format!("{command}{x:.2} {y:.2}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn remaining_graph_y(remaining: f64) -> f64 {
    (99.0 - remaining * 0.98).clamp(1.0, 99.0)
}

#[cfg(test)]
fn smooth_remaining_points(points: &[(i64, f64)]) -> Vec<(i64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut smoothed = Vec::with_capacity(points.len());
    smoothed.push(points[0]);
    for index in 1..points.len() - 1 {
        let average = (points[index - 1].1 + 2.0 * points[index].1 + points[index + 1].1) / 4.0;
        // 利用枠はリセットまで増えないため、計測ノイズによる逆戻りを除く。
        let value = average.min(smoothed.last().expect("anchor exists").1);
        smoothed.push((points[index].0, value));
    }
    let last = points.last().expect("points has at least three items");
    smoothed.push((
        last.0,
        last.1.min(smoothed.last().expect("anchor exists").1),
    ));
    smoothed
}

fn smooth_model_spend(points: &[HourlyModelSpend]) -> Vec<HourlyModelSpend> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut smoothed = Vec::with_capacity(points.len());
    smoothed.push(points[0]);
    for index in 1..points.len() - 1 {
        let previous = *smoothed.last().expect("zero anchor exists");
        let current = points[index];
        let next = points[index + 1];
        let previous_gap = current.timestamp.saturating_sub(previous.timestamp)
            > MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS;
        let next_gap = next.timestamp.saturating_sub(current.timestamp)
            > MODEL_CONTIGUOUS_SAMPLE_MAX_GAP_SECONDS;
        if previous_gap || next_gap {
            smoothed.push(HourlyModelSpend {
                timestamp: current.timestamp,
                sol: current.sol.max(previous.sol),
                terra: current.terra.max(previous.terra),
                luna: current.luna.max(previous.luna),
            });
            continue;
        }
        let smooth = |before: f64, value: f64, after: f64, floor: f64| {
            ((before + 2.0 * value + after) / 4.0).max(floor)
        };
        smoothed.push(HourlyModelSpend {
            timestamp: current.timestamp,
            sol: smooth(previous.sol, current.sol, next.sol, previous.sol),
            terra: smooth(previous.terra, current.terra, next.terra, previous.terra),
            luna: smooth(previous.luna, current.luna, next.luna, previous.luna),
        });
    }
    let last = *points.last().expect("points has at least three items");
    let previous = *smoothed.last().expect("anchor exists");
    smoothed.push(HourlyModelSpend {
        timestamp: last.timestamp,
        sol: last.sol.max(previous.sol),
        terra: last.terra.max(previous.terra),
        luna: last.luna.max(previous.luna),
    });
    smoothed
}

fn local_sessions_root() -> Option<PathBuf> {
    codex_home_root().map(|root| root.join("sessions"))
}

fn codex_home_root() -> Option<PathBuf> {
    let path = std::env::var_os("CODEX_HOME")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    validated_configured_root(path)
}

fn delegation_usage_recovery_path() -> Option<PathBuf> {
    codex_home_root().map(|root| root.join("history").join("delegation_usage_recovery.jsonl"))
}

#[derive(Default)]
struct SessionTraversalBudget {
    files: usize,
    total_bytes: u64,
}

impl SessionTraversalBudget {
    fn admit_file(
        &mut self,
        relative_depth: usize,
        bytes: u64,
    ) -> Result<(), security::SecurityError> {
        if relative_depth > security::MAX_SESSION_DEPTH || bytes > security::MAX_SESSION_FILE_BYTES
        {
            return Err(security::SecurityError::new(
                security::SecurityErrorKind::LimitExceeded,
            ));
        }
        let files = self.files.checked_add(1).ok_or_else(|| {
            security::SecurityError::new(security::SecurityErrorKind::LimitExceeded)
        })?;
        let total_bytes = self.total_bytes.checked_add(bytes).ok_or_else(|| {
            security::SecurityError::new(security::SecurityErrorKind::LimitExceeded)
        })?;
        if files > security::MAX_SESSION_FILES || total_bytes > security::MAX_SESSION_TOTAL_BYTES {
            return Err(security::SecurityError::new(
                security::SecurityErrorKind::LimitExceeded,
            ));
        }
        self.files = files;
        self.total_bytes = total_bytes;
        Ok(())
    }
}

fn session_jsonl_files(root: &Path) -> Result<Vec<PathBuf>, security::SecurityError> {
    fn visit(
        directory: &Path,
        depth: usize,
        budget: &mut SessionTraversalBudget,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), security::SecurityError> {
        if depth > security::MAX_SESSION_DEPTH {
            return Err(security::SecurityError::new(
                security::SecurityErrorKind::LimitExceeded,
            ));
        }
        let entries = fs::read_dir(directory)
            .map_err(|_| security::SecurityError::new(security::SecurityErrorKind::UnsafePath))?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                security::SecurityError::new(security::SecurityErrorKind::UnsafePath)
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                security::SecurityError::new(security::SecurityErrorKind::UnsafePath)
            })?;
            if metadata.file_type().is_symlink() {
                return Err(security::SecurityError::new(
                    security::SecurityErrorKind::UnsafePath,
                ));
            }
            if metadata.is_dir() {
                visit(&path, depth + 1, budget, files)?;
                continue;
            }
            if !metadata.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
            {
                continue;
            }
            budget.admit_file(depth + 1, metadata.len())?;
            files.push(path);
        }
        Ok(())
    }

    let root = security::validate_absolute_root(root)?;
    let mut files = Vec::new();
    visit(&root, 0, &mut SessionTraversalBudget::default(), &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_local_model_usage(
    reset_at: i64,
    window_seconds: i64,
) -> Result<ModelUsageTotals, security::SecurityError> {
    if reset_at <= 0 {
        return Ok(ModelUsageTotals::default());
    }
    let mut totals = ModelUsageTotals::default();
    let window_start = reset_at.saturating_sub(window_seconds.max(0));
    if let Some(root) = local_sessions_root() {
        let paths = session_jsonl_files(&root)?;
        debug_runtime(format!("local session files={}", paths.len()));
        for path in paths {
            if let Err(error) = collect_session_file(&path, &mut totals, window_start) {
                debug_runtime(format!(
                    "local session parse failed kind={:?}",
                    error.kind()
                ));
                return Err(error);
            }
        }
    }
    let window_end = reset_at;
    add_recovery_usage(
        delegation_usage_recovery_path().as_deref(),
        window_start,
        window_end,
        &mut totals,
    );
    Ok(totals)
}

#[derive(Debug, Deserialize)]
struct DelegationUsageRecoveryEntry {
    timestamp: i64,
    thread_id: String,
    model: String,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
}

impl DelegationUsageRecoveryEntry {
    fn snapshot(&self) -> TokenSnapshot {
        // Recovery records contain reasoning tokens for auditing, but the
        // usage display's total is intentionally input plus output only.
        let _ = self.reasoning_tokens;
        TokenSnapshot {
            total: self.input_tokens.saturating_add(self.output_tokens),
            input: self.input_tokens,
            cached_input: self.cached_input_tokens,
            output: self.output_tokens,
        }
    }
}

fn read_recovery_entries(
    path: &Path,
    window_start: i64,
    window_end: i64,
) -> Vec<DelegationUsageRecoveryEntry> {
    if window_start > window_end {
        return Vec::new();
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > security::MAX_SESSION_FILE_BYTES
    {
        return Vec::new();
    }
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut seen_threads = BTreeSet::new();
    let mut entries = Vec::new();
    let mut reader = BufReader::new(file);
    loop {
        let line = match read_recoverable_session_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => return Vec::new(),
        };
        let Ok(entry) = serde_json::from_str::<DelegationUsageRecoveryEntry>(&line) else {
            continue;
        };
        if entry.timestamp < window_start
            || entry.timestamp > window_end
            || entry.timestamp <= 0
            || entry.thread_id.trim().is_empty()
            || ModelUsageTotals::recognized_model(&entry.model).is_none()
        {
            continue;
        }
        if seen_threads.insert(entry.thread_id.clone()) {
            entries.push(entry);
        }
    }
    entries
}

fn add_recovery_usage(
    path: Option<&Path>,
    window_start: i64,
    window_end: i64,
    totals: &mut ModelUsageTotals,
) {
    let Some(path) = path else {
        return;
    };
    for entry in read_recovery_entries(path, window_start, window_end) {
        totals.add(&entry.model, entry.snapshot());
    }
}

struct TimedModelUsage {
    timestamp: i64,
    model: String,
    delta: TokenSnapshot,
}

/// Read one session record while isolating a malformed/oversized line.
///
/// Codex rollout files are append-only and may contain a very large tool
/// payload.  One such payload must not make every valid token snapshot in the
/// same file disappear from the graph.  The bounded reader consumes the bad
/// record before returning the error, so skipping only `LimitExceeded` and
/// `Parse` is both recoverable and bounded; I/O failures remain fatal.
fn read_recoverable_session_line<R: BufRead>(
    reader: &mut R,
) -> Result<Option<String>, security::SecurityError> {
    loop {
        match security::read_bounded_jsonl_record(reader) {
            Ok(Some((line, true))) => return Ok(Some(line)),
            Ok(Some((_line, false))) => {
                return Err(security::SecurityError::new(
                    security::SecurityErrorKind::Unterminated,
                ));
            }
            Ok(None) => return Ok(None),
            Err(error)
                if matches!(
                    error.kind(),
                    security::SecurityErrorKind::LimitExceeded | security::SecurityErrorKind::Parse
                ) =>
            {
                debug_runtime(format!(
                    "skipped malformed session record kind={:?}",
                    error.kind()
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

fn session_event_type(value: &Value) -> Option<&str> {
    let outer_type = value.get("type").and_then(Value::as_str);
    match outer_type {
        Some("token_count" | "turn_context" | "thread_context" | "thread_settings_applied") => {
            outer_type
        }
        _ => value
            .get("payload")
            .and_then(Value::as_object)
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str),
    }
}

fn session_event_model(value: &Value) -> Option<String> {
    let payload = value.get("payload").and_then(Value::as_object);
    let root_model = value.get("model").and_then(Value::as_str);
    let model = match session_event_type(value) {
        Some("turn_context" | "thread_context") => payload
            .and_then(|payload| payload.get("model").and_then(Value::as_str))
            .or(root_model),
        Some("thread_settings_applied") => payload
            .and_then(|payload| {
                payload
                    .get("thread_settings")
                    .and_then(Value::as_object)
                    .and_then(|settings| settings.get("model"))
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("model").and_then(Value::as_str))
            })
            .or(root_model),
        _ => None,
    }?;
    (!model.trim().is_empty()).then(|| model.to_owned())
}

fn session_token_snapshot(value: &Value) -> Option<TokenSnapshot> {
    if session_event_type(value) != Some("token_count") {
        return None;
    }
    let payload = value.get("payload").and_then(Value::as_object)?;
    let total_usage = payload
        .get("info")
        .and_then(|info| info.get("total_token_usage"))
        .and_then(Value::as_object)?;
    let total = total_usage.get("total_tokens").and_then(Value::as_u64)?;
    Some(TokenSnapshot {
        total,
        input: total_usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_input: total_usage
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output: total_usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn session_event_timestamp(value: &Value) -> i64 {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .unwrap_or(0)
}

fn recovery_timed_usage(path: &Path, window_start: i64, window_end: i64) -> Vec<TimedModelUsage> {
    read_recovery_entries(path, window_start, window_end)
        .into_iter()
        .map(|entry| {
            let delta = entry.snapshot();
            TimedModelUsage {
                timestamp: entry.timestamp,
                model: entry.model,
                delta,
            }
        })
        .collect()
}

fn model_usage_timeline_from_events(
    mut events: Vec<TimedModelUsage>,
    reset_at: i64,
) -> Vec<UsageHistorySample> {
    events.sort_by_key(|event| event.timestamp);

    let mut totals = ModelUsageTotals::default();
    let mut samples: Vec<UsageHistorySample> = Vec::new();
    for event in events {
        let minute = event.timestamp.div_euclid(60) * 60;
        totals.add(&event.model, event.delta);
        let costs = totals.dollar_totals();
        let sample = UsageHistorySample::from_model_history_with_usage(
            minute,
            reset_at,
            costs,
            totals.token_totals(),
        );
        if let Some(previous) = samples.last_mut() {
            if previous.timestamp == sample.timestamp {
                *previous = sample;
                continue;
            }
        }
        samples.push(sample);
    }
    samples
}

fn collect_local_model_usage_timeline(
    reset_at: i64,
    window_seconds: i64,
) -> Result<Vec<UsageHistorySample>, security::SecurityError> {
    if reset_at <= 0 {
        return Ok(Vec::new());
    }
    let window_start = reset_at.saturating_sub(window_seconds.max(0));
    let now = Utc::now().timestamp().min(reset_at);
    let mut events = Vec::new();
    if let Some(root) = local_sessions_root() {
        let paths = session_jsonl_files(&root)?;
        debug_runtime(format!("local timeline files={}", paths.len()));
        for path in paths {
            if let Err(error) = collect_session_timeline_file(&path, window_start, now, &mut events)
            {
                debug_runtime(format!(
                    "local timeline parse failed kind={:?}",
                    error.kind()
                ));
                return Err(error);
            }
        }
    }
    if let Some(path) = delegation_usage_recovery_path() {
        events.extend(recovery_timed_usage(&path, window_start, now));
    }
    Ok(model_usage_timeline_from_events(events, reset_at))
}

fn collect_session_timeline_file(
    path: &Path,
    window_start: i64,
    window_end: i64,
    events: &mut Vec<TimedModelUsage>,
) -> Result<(), security::SecurityError> {
    let file = File::open(path)
        .map_err(|_| security::SecurityError::new(security::SecurityErrorKind::UnsafePath))?;
    let initial_len = events.len();
    let mut model: Option<String> = None;
    let mut previous = TokenSnapshot::default();
    let mut reader = BufReader::new(file);
    loop {
        let line = match read_recoverable_session_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                events.truncate(initial_len);
                return Err(error);
            }
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // `thread_settings_applied` can precede the actual turn context. Keep
        // the previous model until `turn_context` confirms the model for the
        // token-count event; otherwise setup metadata is charged to the next
        // model (ccusage-compatible attribution).
        if session_event_type(&value) != Some("thread_settings_applied") || model.is_none() {
            if let Some(next_model) = session_event_model(&value) {
                model = Some(next_model);
            }
        }
        let Some(current) = session_token_snapshot(&value) else {
            continue;
        };
        let delta = TokenSnapshot {
            total: current.total.saturating_sub(previous.total),
            input: current.input.saturating_sub(previous.input),
            cached_input: current.cached_input.saturating_sub(previous.cached_input),
            output: current.output.saturating_sub(previous.output),
        };
        previous = current;
        let timestamp = session_event_timestamp(&value);
        if timestamp < window_start || timestamp > window_end {
            continue;
        }
        if let Some(model) = model.as_deref() {
            events.push(TimedModelUsage {
                timestamp,
                model: model.to_owned(),
                delta,
            });
        }
    }
    Ok(())
}

fn collect_session_file(
    path: &Path,
    totals: &mut ModelUsageTotals,
    window_start: i64,
) -> Result<(), security::SecurityError> {
    let file = File::open(path)
        .map_err(|_| security::SecurityError::new(security::SecurityErrorKind::UnsafePath))?;
    let original = totals.clone();
    let mut model: Option<String> = None;
    let mut previous = TokenSnapshot::default();
    let mut reader = BufReader::new(file);
    loop {
        let line = match read_recoverable_session_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                *totals = original;
                return Err(error);
            }
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if session_event_type(&value) != Some("thread_settings_applied") || model.is_none() {
            if let Some(next_model) = session_event_model(&value) {
                model = Some(next_model);
            }
        }
        let Some(current) = session_token_snapshot(&value) else {
            continue;
        };
        let delta = TokenSnapshot {
            total: current.total.saturating_sub(previous.total),
            input: current.input.saturating_sub(previous.input),
            cached_input: current.cached_input.saturating_sub(previous.cached_input),
            output: current.output.saturating_sub(previous.output),
        };
        previous = current;
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp())
            .unwrap_or(0);
        if timestamp < window_start {
            continue;
        }
        if let Some(model) = model.as_deref() {
            totals.add(model, delta);
        }
    }
    Ok(())
}

fn resolved_executable(override_name: &str, command_name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(override_name) {
        return security::resolve_executable_path(Path::new(&path)).ok();
    }
    let path = std::env::var_os("PATH")?;
    security::resolve_executable_from_path(command_name, path).ok()
}

enum RpcReadEvent {
    Line(security::RpcLine),
    Closed,
    Failed,
}

fn rpc_reader(stdout: std::process::ChildStdout) -> Receiver<RpcReadEvent> {
    let (tx, rx) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match security::RpcLine::read(&mut reader) {
                Ok(Some(line)) => {
                    if tx.send(RpcReadEvent::Line(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(RpcReadEvent::Closed);
                    break;
                }
                Err(_) => {
                    let _ = tx.send(RpcReadEvent::Failed);
                    break;
                }
            }
        }
    });
    rx
}

struct AppServerBridge<C, E> {
    tx: Sender<C>,
    rx: Receiver<E>,
}

impl<C, E> AppServerBridge<C, E> {
    fn inactive() -> Self {
        let (tx, _commands) = mpsc::channel();
        let (_events, rx) = mpsc::channel();
        Self { tx, rx }
    }

    fn send(&self, command: C) -> bool {
        self.tx.send(command).is_ok()
    }
}

impl AppServerBridge<AccountCommand, Event> {
    fn start() -> Self {
        let (tx, commands) = mpsc::channel::<AccountCommand>();
        let (events, rx) = mpsc::channel::<Event>();
        thread::spawn(move || account_server_worker(commands, events));
        Self { tx, rx }
    }
}

impl AppServerBridge<ThreadCommand, ThreadEvent> {
    fn start() -> Self {
        let (tx, commands) = mpsc::channel::<ThreadCommand>();
        let (events, rx) = mpsc::channel::<ThreadEvent>();
        thread::spawn(move || thread_server_worker(commands, events));
        Self { tx, rx }
    }
}

struct LocalUsageBridge {
    tx: Sender<LocalCommand>,
    rx: Receiver<LocalEvent>,
}

impl LocalUsageBridge {
    fn start() -> Self {
        let (tx, commands) = mpsc::channel::<LocalCommand>();
        let (events, rx) = mpsc::channel::<LocalEvent>();
        thread::spawn(move || local_usage_worker(commands, events));
        Self { tx, rx }
    }

    fn inactive() -> Self {
        let (tx, _commands) = mpsc::channel();
        let (_events, rx) = mpsc::channel();
        Self { tx, rx }
    }

    fn send(&self, command: LocalCommand) -> bool {
        self.tx.send(command).is_ok()
    }
}

fn account_server_worker(commands: Receiver<AccountCommand>, events: Sender<Event>) {
    debug_runtime("account worker starting");
    let Some(codex) = resolved_executable("CODEX_INFO_CODEX_BIN", "codex") else {
        let _ = events.send(Event::Error(
            "Codex app-serverの安全な実行ファイルを確認できません。".into(),
        ));
        return;
    };
    let child_result = Command::new(codex)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let child = match child_result {
        Ok(child) => child,
        Err(_) => {
            let _ = events.send(Event::Error(
                "Codex app-serverを起動できませんでした。".into(),
            ));
            return;
        }
    };
    let mut child = security::ChildGuard::new(child);
    let Some(mut input) = child.child_mut().ok().and_then(|child| child.stdin.take()) else {
        let _ = events.send(Event::Error(
            "Codex app-serverの入出力を初期化できませんでした。".into(),
        ));
        return;
    };
    let Some(stdout) = child.child_mut().ok().and_then(|child| child.stdout.take()) else {
        let _ = events.send(Event::Error(
            "Codex app-serverの入出力を初期化できませんでした。".into(),
        ));
        return;
    };
    let output = rpc_reader(stdout);
    if let Err(e) = request(
        &mut input,
        &output,
        1,
        "initialize",
        json!({"clientInfo":{"name":"codex-info","version":"0.3.0"},"capabilities":{"experimentalApi":true}}),
    ) {
        let _ = events.send(Event::Error(e));
        return;
    }
    let _ = events.send(Event::Ready);
    debug_runtime("account worker ready");
    let mut id = 2u64;
    while let Ok(command) = commands.recv() {
        match command {
            AccountCommand::Stop => {
                let _ = child.kill_and_reap();
                break;
            }
            AccountCommand::Login => {
                match request(
                    &mut input,
                    &output,
                    id,
                    "account/login/start",
                    json!({"type":"chatgpt"}),
                ) {
                    Ok(result) => {
                        if let Some(url) = result.get("authUrl").and_then(Value::as_str) {
                            match security::validate_auth_url(url) {
                                Ok(url) => {
                                    let _ = events.send(Event::AuthUrl(url.to_string()));
                                }
                                Err(_) => {
                                    let _ = events.send(Event::Error(
                                        "Codexから安全な認証URLを受け取れませんでした。".into(),
                                    ));
                                }
                            }
                        } else {
                            let _ = events.send(Event::Error(
                                "Codexから認証URLを受け取れませんでした。".into(),
                            ));
                        }
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(e));
                    }
                }
                id += 1;
            }
            AccountCommand::Read => {
                debug_runtime("account read requested");
                let account = request(&mut input, &output, id, "account/read", json!({}));
                id += 1;
                match account {
                    Ok(result) => {
                        let (email, authenticated, plan_type) =
                            match protocol_contract::decode_account(&result) {
                                Ok(protocol_contract::AccountOutcome::Supported {
                                    email,
                                    plan_type,
                                }) => (Some(email), true, Some(plan_type.as_str().to_owned())),
                                Ok(protocol_contract::AccountOutcome::AuthRequired)
                                | Ok(protocol_contract::AccountOutcome::UnsupportedNoData) => {
                                    (None, false, None)
                                }
                                Err(_) => {
                                    let _ = events.send(Event::Error(
                                        "アカウントの正本データを取得できませんでした。".into(),
                                    ));
                                    continue;
                                }
                            };
                        let _ = events.send(Event::Account {
                            email: email.clone(),
                            authenticated,
                            plan_type: plan_type.clone(),
                        });
                        debug_runtime(format!("account read authenticated={authenticated}"));
                        if authenticated {
                            let rate_request_id = id;
                            id = id.saturating_add(1);
                            match request(
                                &mut input,
                                &output,
                                rate_request_id,
                                "account/rateLimits/read",
                                Value::Null,
                            ) {
                                Ok(rate) => {
                                    match parse_rate_limits(
                                        &rate,
                                        plan_type.as_deref(),
                                        Utc::now().timestamp(),
                                    ) {
                                        Ok(snapshot) => {
                                            let RateLimitSnapshot {
                                                remaining_percent,
                                                reset_at,
                                                window_seconds,
                                                limit_name,
                                                quota_title,
                                                monthly,
                                            } = snapshot;
                                            let recheck_id = id;
                                            let Some(next_id) = id.checked_add(1) else {
                                                let _ = events.send(Event::Error(
                                                    "Codex APIの要求IDが上限に達しました。".into(),
                                                ));
                                                continue;
                                            };
                                            id = next_id;
                                            let recheck = request(
                                                &mut input,
                                                &output,
                                                recheck_id,
                                                "account/read",
                                                json!({}),
                                            );
                                            let identity_is_current = match recheck {
                                                Ok(result) => {
                                                    match protocol_contract::decode_account(&result)
                                                    {
                                                        Ok(protocol_contract::AccountOutcome::Supported {
                                                            email: current_email,
                                                            plan_type: current_plan,
                                                        }) => {
                                                            let current_plan = current_plan
                                                                .as_str()
                                                                .to_owned();
                                                            if email.as_deref()
                                                                == Some(current_email.as_str())
                                                                && plan_type.as_deref()
                                                                    == Some(current_plan.as_str())
                                                            {
                                                                true
                                                            } else {
                                                                let _ = events.send(Event::Account {
                                                                    email: Some(current_email),
                                                                    authenticated: true,
                                                                    plan_type: Some(current_plan),
                                                                });
                                                                false
                                                            }
                                                        }
                                                        Ok(protocol_contract::AccountOutcome::AuthRequired)
                                                        | Ok(protocol_contract::AccountOutcome::UnsupportedNoData) => {
                                                            let _ = events.send(Event::Account {
                                                                email: None,
                                                                authenticated: false,
                                                                plan_type: None,
                                                            });
                                                            false
                                                        }
                                                        Err(_) => {
                                                            let _ = events.send(Event::Error(
                                                                "アカウントの再確認に失敗しました。"
                                                                    .into(),
                                                            ));
                                                            false
                                                        }
                                                    }
                                                }
                                                Err(error) => {
                                                    let _ = events.send(Event::Error(error));
                                                    false
                                                }
                                            };
                                            if !identity_is_current {
                                                continue;
                                            }
                                            let _ =
                                                events.send(Event::Usage(Box::new(UsageEvent {
                                                    remaining_percent,
                                                    reset_at,
                                                    window_seconds,
                                                    limit_name,
                                                    quota_title,
                                                    monthly,
                                                })));
                                            debug_runtime(format!(
                                                "usage received reset_at={reset_at} window_seconds={window_seconds}"
                                            ));
                                        }
                                        Err(()) => {
                                            let _ = events.send(Event::Error(
                                                "利用枠の正本データを取得できませんでした。".into(),
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = events.send(Event::Error(e));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(e));
                    }
                }
            }
        }
    }
}

struct RunningAppServer {
    child: security::ChildGuard,
    input: std::process::ChildStdin,
    output: Receiver<RpcReadEvent>,
}

fn start_app_server() -> Result<RunningAppServer, String> {
    let Some(codex) = resolved_executable("CODEX_INFO_CODEX_BIN", "codex") else {
        return Err("Codex app-serverの安全な実行ファイルを確認できません。".into());
    };
    let child = Command::new(codex)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Codex app-serverを起動できませんでした。".to_owned())?;
    let mut child = security::ChildGuard::new(child);
    let Some(mut input) = child.child_mut().ok().and_then(|child| child.stdin.take()) else {
        return Err("Codex app-serverの入出力を初期化できませんでした。".into());
    };
    let Some(stdout) = child.child_mut().ok().and_then(|child| child.stdout.take()) else {
        return Err("Codex app-serverの入出力を初期化できませんでした。".into());
    };
    let output = rpc_reader(stdout);
    request(
        &mut input,
        &output,
        1,
        "initialize",
        json!({"clientInfo":{"name":"codex-info","version":"0.3.0"},"capabilities":{"experimentalApi":true}}),
    )?;
    Ok(RunningAppServer {
        child,
        input,
        output,
    })
}

fn thread_server_worker(commands: Receiver<ThreadCommand>, events: Sender<ThreadEvent>) {
    debug_runtime("thread worker starting");
    // The thread bridge is lazy: construction of CodexInfoState does not issue
    // thread/list before account authentication succeeds. Once started, this
    // worker owns its own child, stdin/stdout, reader and request-id sequence.
    let mut server: Option<RunningAppServer> = None;
    let mut server_active_paths = BTreeSet::new();
    let mut next_id = 2u64;
    while let Ok(command) = commands.recv() {
        match command {
            ThreadCommand::Stop => {
                if let Some(mut server) = server.take() {
                    let _ = server.child.kill_and_reap();
                }
                break;
            }
            ThreadCommand::Read { auth_epoch } => {
                debug_runtime(format!("thread read requested epoch={auth_epoch}"));
                let Some(codex_root) = codex_home_root() else {
                    let _ = events.send(ThreadEvent::Error {
                        auth_epoch,
                        message: "スレッド情報を安全に取得できませんでした。".into(),
                    });
                    continue;
                };
                let (sessions_root, active_paths) = match active_thread_paths(&codex_root) {
                    Ok(paths) => paths,
                    Err(()) => {
                        debug_runtime("thread active path scan failed");
                        let _ = events.send(ThreadEvent::Error {
                            auth_epoch,
                            message: "スレッド情報を安全に取得できませんでした。".into(),
                        });
                        continue;
                    }
                };
                if active_paths.is_empty() {
                    if let Some(mut idle) = server.take() {
                        let _ = idle.child.kill_and_reap();
                    }
                    server_active_paths.clear();
                    next_id = 2;
                    let _ = events.send(ThreadEvent::Update {
                        auth_epoch,
                        update: ActiveThreadUpdate::NoThread,
                    });
                    continue;
                }

                // Codex app-server snapshots its thread index when it starts.
                // Reusing that process after the live rollout set changes can
                // therefore publish a false zero to REST while the X client,
                // started later, sees the running thread. Refresh exactly on
                // the process-owned session-set boundary; steady-state polls
                // keep the same child and do not create process churn.
                if server.is_some() && server_active_paths != active_paths {
                    if let Some(mut stale) = server.take() {
                        let _ = stale.child.kill_and_reap();
                    }
                    server_active_paths.clear();
                    next_id = 2;
                }
                if server.is_none() {
                    match start_app_server() {
                        Ok(started) => {
                            server = Some(started);
                            server_active_paths = active_paths.clone();
                            let _ = events.send(ThreadEvent::Ready);
                        }
                        Err(message) => {
                            let _ = events.send(ThreadEvent::Error {
                                auth_epoch,
                                message,
                            });
                            continue;
                        }
                    }
                }
                let Some(server_ref) = server.as_mut() else {
                    continue;
                };
                let update = fetch_active_thread_update(
                    &mut server_ref.input,
                    &server_ref.output,
                    &mut next_id,
                    &sessions_root,
                    &active_paths,
                    &codex_root,
                );
                if update == ActiveThreadUpdate::Failed {
                    debug_runtime("thread read failed");
                    let _ = events.send(ThreadEvent::Error {
                        auth_epoch,
                        message: "スレッド情報を安全に取得できませんでした。".into(),
                    });
                    // A framing, timeout, EOF or protocol-budget failure can
                    // leave this connection unusable. Reap only this isolated
                    // thread server so the next scheduled read starts cleanly.
                    if let Some(mut failed) = server.take() {
                        let _ = failed.child.kill_and_reap();
                    }
                    server_active_paths.clear();
                    next_id = 2;
                } else {
                    debug_runtime(match &update {
                        ActiveThreadUpdate::Snapshot(rows) => {
                            format!("thread snapshot rows={}", rows.len())
                        }
                        ActiveThreadUpdate::NoThread => "thread snapshot rows=0".to_owned(),
                        ActiveThreadUpdate::Failed => "thread snapshot failed".to_owned(),
                    });
                    let _ = events.send(ThreadEvent::Update { auth_epoch, update });
                }
            }
        }
    }
}

fn local_usage_worker(commands: Receiver<LocalCommand>, events: Sender<LocalEvent>) {
    debug_runtime("local usage worker starting");
    while let Ok(command) = commands.recv() {
        match command {
            LocalCommand::Stop => break,
            LocalCommand::Collect {
                auth_epoch,
                reset_at,
                window_seconds,
            } => {
                debug_runtime(format!(
                    "local collect requested epoch={auth_epoch} reset_at={reset_at} window_seconds={window_seconds}"
                ));
                let result = (|| {
                    let model_usage = collect_local_model_usage(reset_at, window_seconds)?;
                    let history_samples =
                        collect_local_model_usage_timeline(reset_at, window_seconds)?;
                    Ok::<_, security::SecurityError>((model_usage, history_samples))
                })();
                match result {
                    Ok((model_usage, history_samples)) => {
                        debug_runtime(format!(
                            "local collect succeeded rows={} samples={}",
                            model_usage.clone().rows().len(),
                            history_samples.len()
                        ));
                        let _ = events.send(LocalEvent::Usage(LocalUsageResult {
                            auth_epoch,
                            reset_at,
                            window_seconds,
                            model_usage,
                            history_samples,
                        }));
                    }
                    Err(_) => {
                        debug_runtime("local collect failed");
                        let _ = events.send(LocalEvent::Error {
                            auth_epoch,
                            reset_at,
                            window_seconds,
                        });
                    }
                }
            }
        }
    }
}

fn request(
    input: &mut impl Write,
    output: &Receiver<RpcReadEvent>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    request_with_timeout(
        input,
        output,
        id,
        method,
        params,
        security::RPC_RESPONSE_TIMEOUT,
    )
}

fn request_with_timeout(
    input: &mut impl Write,
    output: &Receiver<RpcReadEvent>,
    id: u64,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    writeln!(input, "{message}")
        .map_err(|_| "Codex app-serverへ要求を送信できませんでした。".to_owned())?;
    input
        .flush()
        .map_err(|_| "Codex app-serverへ要求を送信できませんでした。".to_owned())?;
    let deadline = Instant::now() + timeout;
    let limits = security::RpcLimits::standard();
    let mut ignored = 0usize;
    loop {
        let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
            return Err("Codex app-serverの応答がタイムアウトしました。".into());
        };
        let line = match output.recv_timeout(wait) {
            Ok(RpcReadEvent::Line(line)) => line,
            Ok(RpcReadEvent::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Err("Codex app-serverが終了しました。".into());
            }
            Ok(RpcReadEvent::Failed) => {
                return Err("Codex app-serverから安全に応答を読めませんでした。".into());
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err("Codex app-serverの応答がタイムアウトしました。".into());
            }
        };
        let value: Value = match serde_json::from_str(line.as_str()) {
            Ok(value) => value,
            Err(_) => {
                limits
                    .record_ignored_message(&mut ignored)
                    .map_err(|_| "Codex app-serverの応答数が上限を超えました。".to_owned())?;
                continue;
            }
        };
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            limits
                .record_ignored_message(&mut ignored)
                .map_err(|_| "Codex app-serverの応答数が上限を超えました。".to_owned())?;
            continue;
        }
        if value.get("error").is_some() {
            return Err("Codex APIが要求を完了できませんでした。".into());
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

struct CodexInfoState {
    i18n: I18n,
    bridge: AppServerBridge<AccountCommand, Event>,
    thread_bridge: Option<AppServerBridge<ThreadCommand, ThreadEvent>>,
    local_bridge: LocalUsageBridge,
    auth_epoch: u64,
    email: Option<String>,
    authenticated: bool,
    plan_label: String,
    auth_url: Option<String>,
    remaining_percent: Option<f64>,
    has_quota_percent: bool,
    has_usage: bool,
    reset_at: Option<i64>,
    window_seconds: i64,
    limit_name: String,
    quota_title: String,
    monthly: bool,
    account_error: Option<String>,
    error: Option<String>,
    status: String,
    checking: bool,
    last_poll: Instant,
    last_success_at: Option<i64>,
    model_usage: Vec<ModelUsageRow>,
    active_threads: Vec<ActiveThread>,
    estimated_cost_label: String,
    history: UsageHistory,
    selected_reset_at: Option<i64>,
    selected_history_period: String,
    selected_metric: String,
    preview: bool,
    auth_polling: bool,
    thread_checking: bool,
    thread_error: bool,
    local_usage_error: bool,
    /// A quota event is not a complete usage snapshot.  On the first load the
    /// public/native views stay loading until the independent local collector
    /// commits; after that, the last committed snapshot remains visible while
    /// a periodic refresh is pending.
    local_usage_pending: bool,
    /// A completed snapshot remains visible while a later quota-only refresh
    /// collects the next local payload. This is cleared only with account
    /// identity, never at each periodic refresh or reset timestamp update.
    usage_snapshot_committed: bool,
    last_thread_poll: Instant,
    /// The last persisted reset period is enough to backfill local session
    /// usage while app-server/REST is unavailable. It is never exposed until
    /// a fresh authenticated quota snapshot is committed.
    recovery_period: Option<(i64, i64)>,
    recovery_requested: bool,
    /// In UI mode, the service listener is the single owner of the visible
    /// snapshot. Keep a failed selected endpoint latched until that same
    /// endpoint becomes healthy; never fall back to the default port.
    service_endpoint_error: Option<String>,
}

impl CodexInfoState {
    fn projected_history(&self) -> UsageHistory {
        UsageHistory {
            samples: authoritative_history_projection_samples(
                &self.history.samples,
                self.reset_at,
                self.window_seconds,
            ),
            ..UsageHistory::default()
        }
    }

    fn usage_ready(&self) -> bool {
        self.has_usage && !self.local_usage_pending
    }

    fn has_visible_usage(&self) -> bool {
        self.usage_ready() || self.usage_snapshot_committed
    }

    /// Build the only data shape allowed to cross into the loopback API.
    ///
    /// This intentionally does not include email, auth URL, local paths,
    /// session content, or detailed backend errors. The HTTP worker cannot
    /// access this state directly; it receives only this immutable copy.
    fn public_snapshot(&self) -> PublicSnapshot {
        let state = if self.error.is_some() || self.account_error.is_some() {
            PublicState::Error
        } else if !self.authenticated {
            if self.checking {
                PublicState::Initializing
            } else {
                PublicState::AuthRequired
            }
        } else if self.has_visible_usage() {
            PublicState::Ready
        } else {
            PublicState::Initializing
        };
        let quota = self
            .has_quota_percent
            .then_some(())
            .filter(|_| self.has_visible_usage())
            .and_then(|_| self.remaining_percent.zip(self.reset_at))
            .map(|(remaining_percent, reset_at)| PublicQuota {
                remaining_percent: remaining_percent.clamp(0.0, 100.0),
                reset_at,
                window_seconds: self.window_seconds.max(1),
                monthly: self.monthly,
            });
        let models = self
            .authenticated
            .then_some(())
            .filter(|_| self.has_visible_usage())
            .map(|_| {
                self.model_usage
                    .iter()
                    .filter(|row| matches!(row.name.as_str(), "SOL" | "TERRA" | "LUNA"))
                    .map(|row| PublicModelUsage {
                        name: row.name.clone(),
                        input_tokens: row.input_tokens.saturating_sub(row.cached_input_tokens),
                        cached_input_tokens: row.cached_input_tokens,
                        output_tokens: row.output_tokens,
                    })
                    .collect()
            })
            .unwrap_or_default();
        PublicSnapshot {
            state,
            observed_at: if self.authenticated && self.has_visible_usage() {
                self.last_success_at.filter(|timestamp| *timestamp > 0)
            } else {
                None
            },
            authenticated: self.authenticated,
            plan_label: self
                .authenticated
                .then(|| self.plan_label.trim())
                .filter(|label| !label.is_empty())
                .map(str::to_owned),
            quota,
            models,
            active_thread_count: if self.authenticated && self.has_visible_usage() {
                u64::try_from(self.active_threads.len()).unwrap_or(u64::MAX)
            } else {
                0
            },
        }
    }

    /// Build the additive read-only document consumed by the Windows client.
    /// This is deliberately derived from the same state as `public_snapshot`
    /// so status and details are published atomically as one generation.
    fn public_details(&self) -> PublicDetails {
        self.public_details_at(Utc::now().timestamp())
    }

    fn public_details_at(&self, now: i64) -> PublicDetails {
        let snapshot = self.public_snapshot();
        let projected_history = self.projected_history();
        let models = if self.authenticated && self.has_visible_usage() {
            self.model_usage
                .iter()
                .filter(|row| matches!(row.name.as_str(), "SOL" | "TERRA" | "LUNA"))
                .map(|row| {
                    let (input_dollars, cached_input_dollars, output_dollars) = row.dollar_costs();
                    PublicDetailedModelUsage {
                        name: row.name.clone(),
                        input_tokens: row.input_tokens.saturating_sub(row.cached_input_tokens),
                        cached_input_tokens: row.cached_input_tokens,
                        output_tokens: row.output_tokens,
                        input_dollars,
                        cached_input_dollars,
                        output_dollars,
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let history_cutoff = DateTime::<Utc>::from_timestamp(now, 0)
            .map(one_month_before_utc)
            .unwrap_or(now);
        let history_periods = if self.authenticated && self.has_visible_usage() {
            let observed_at = snapshot.observed_at.unwrap_or(now);
            let periods = self.history_periods_at(observed_at);
            let current_period_reset =
                current_history_period_reset(&periods, self.reset_at, observed_at);
            periods
                .into_iter()
                .map(|period| {
                    let current = current_period_reset == Some(period.canonical_reset_at);
                    PublicHistoryPeriod {
                        id: period.canonical_reset_at.to_string(),
                        start_at: period.start,
                        // The same bounded period instance feeds graph,
                        // labels, and the public details document.
                        end_at: period.end,
                        reset_at: period.canonical_reset_at,
                        label: period.label,
                        current,
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut history_samples = if self.authenticated && self.has_visible_usage() {
            projected_history
                .canonical_samples()
                .into_iter()
                // SQLite retains three calendar months, but one REST
                // document materializes only `(one month before now, now]`.
                // Enforce the boundary here as well as at the store/working-
                // set boundary so a malformed or synthetic in-memory state
                // cannot freeze publication with old or future rows.
                .filter(|sample| sample.timestamp > history_cutoff && sample.timestamp <= now)
                .map(|sample| PublicHistorySample {
                    timestamp: sample.timestamp,
                    reset_at: sample.reset_at,
                    remaining_percent: (sample.remaining_percent >= 0.0)
                        .then_some(sample.remaining_percent),
                    sol_dollars: sample.sol_dollars,
                    terra_dollars: sample.terra_dollars,
                    luna_dollars: sample.luna_dollars,
                    sol_tokens: sample.sol_tokens,
                    terra_tokens: sample.terra_tokens,
                    luna_tokens: sample.luna_tokens,
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        history_samples.sort_by_key(|sample| (sample.reset_at, sample.timestamp));
        // Do not silently truncate an over-capacity candidate. The REST
        // publisher validates the complete candidate and keeps the previous
        // atomic status/details generation when the public limit is exceeded.

        let mut threads = if self.authenticated && self.has_visible_usage() {
            self.active_threads
                .iter()
                .map(ActiveThread::to_public_thread)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        // Ensure no stale unauthenticated rows survive if an auxiliary worker
        // completed just as the account state changed.
        if !snapshot.authenticated {
            threads.clear();
        }
        PublicDetails {
            state: snapshot.state,
            observed_at: snapshot.observed_at,
            authenticated: snapshot.authenticated,
            plan_label: snapshot.plan_label,
            quota: snapshot.quota,
            models,
            active_thread_count: snapshot.active_thread_count,
            history_periods,
            history_samples,
            // No confirmed recorder_gap_ledger authority is connected to the
            // Rust producer yet. Publish the field explicitly without
            // inferring gaps from missing local samples.
            history_gaps: Vec::new(),
            threads,
            estimated_cost_label: self.estimated_cost_label.clone(),
        }
    }

    #[allow(clippy::needless_return)]
    fn window_title(&self) -> String {
        #[cfg(test)]
        {
            return account_window_title(
                self.authenticated,
                self.email.as_deref(),
                &self.plan_label,
            );
        }
        #[cfg(not(test))]
        localized_account_window_title(
            &self.i18n,
            self.authenticated,
            self.email.as_deref(),
            &self.plan_label,
        )
    }

    fn new() -> Self {
        let i18n = I18n::detect();
        let bridge = AppServerBridge::<AccountCommand, Event>::start();
        bridge.send(AccountCommand::Read);
        let history = UsageHistory::load();
        let recovery_period = history.latest_period_hint();
        Self {
            i18n,
            bridge,
            thread_bridge: None,
            local_bridge: LocalUsageBridge::start(),
            auth_epoch: 0,
            email: None,
            authenticated: false,
            plan_label: String::new(),
            auth_url: None,
            remaining_percent: None,
            has_quota_percent: false,
            has_usage: false,
            reset_at: None,
            window_seconds: WEEK_SECONDS,
            limit_name: "Codex".into(),
            quota_title: "残り利用枠".into(),
            monthly: false,
            account_error: None,
            error: None,
            status: "Codex app-serverへ接続しています…".into(),
            checking: true,
            last_poll: Instant::now(),
            last_success_at: None,
            model_usage: Vec::new(),
            active_threads: Vec::new(),
            estimated_cost_label: "概算 —".into(),
            history,
            selected_reset_at: None,
            selected_history_period: "履歴なし".into(),
            selected_metric: "ドル".into(),
            preview: false,
            auth_polling: false,
            thread_checking: false,
            thread_error: false,
            local_usage_error: false,
            local_usage_pending: false,
            usage_snapshot_committed: false,
            last_thread_poll: Instant::now(),
            recovery_period,
            recovery_requested: false,
            service_endpoint_error: None,
        }
    }

    fn preview(kind: &str) -> Self {
        let i18n = I18n::detect();
        let bridge = AppServerBridge::<AccountCommand, Event>::inactive();
        let now = Utc::now().timestamp();
        let reset_at = now + 6 * 86_400 + 14 * 3_600;
        let model_usage = vec![
            preview_model_row("SOL", 159_278_976, 110_000_000, 30_000_000, 19_278_976),
            preview_model_row("TERRA", 30_885_887, 20_000_000, 7_000_000, 3_885_887),
            preview_model_row("LUNA", 155_294_770, 100_000_000, 40_000_000, 15_294_770),
        ];
        let preview_costs = ModelDollarTotals::from_rows(&model_usage);
        let mut state = Self {
            i18n,
            bridge,
            thread_bridge: None,
            local_bridge: LocalUsageBridge::inactive(),
            auth_epoch: 0,
            email: Some("preview@example.com".into()),
            authenticated: true,
            plan_label: "Pro".into(),
            auth_url: None,
            remaining_percent: Some(14.0),
            has_quota_percent: true,
            has_usage: true,
            reset_at: Some(reset_at),
            limit_name: "Codex".into(),
            quota_title: "残り利用枠".into(),
            monthly: false,
            account_error: None,
            error: None,
            status: String::new(),
            checking: false,
            last_poll: Instant::now(),
            last_success_at: Some(now - 60),
            window_seconds: WEEK_SECONDS,
            history: UsageHistory::preview(now, reset_at, preview_costs),
            model_usage,
            active_threads: vec![ActiveThread {
                id: "preview-thread".into(),
                created_at: Some(now - 600),
                updated_at: now,
                title: "長めの日本語タイトルで表示確認を行う実行中スレッド".into(),
                model: "gpt-5.6-sol".into(),
                model_label: "gpt-5.6-sol".into(),
                total_tokens: Some(12_345),
                context_usage_tokens: Some(12_345),
                context_window_tokens: Some(258_400),
                last_user_message_at: Some(now - 8),
                is_subagent: false,
                parent_thread_id: None,
                depth: None,
            }],
            estimated_cost_label: format_estimated_cost(preview_costs),
            preview: true,
            selected_reset_at: Some(reset_at),
            selected_history_period: String::new(),
            selected_metric: "ドル".into(),
            auth_polling: false,
            thread_checking: false,
            thread_error: false,
            local_usage_error: false,
            local_usage_pending: false,
            // Preview starts with a complete in-memory payload, so
            // `usage_ready()` is sufficient. Keep this false to exercise the
            // first-load (not-yet-committed) pending contract in tests.
            usage_snapshot_committed: false,
            last_thread_poll: Instant::now(),
            recovery_period: None,
            recovery_requested: false,
            service_endpoint_error: None,
        };
        match kind {
            "startup-loading" => {
                // Authenticated identity is known, but the first local usage
                // collection has not committed a complete generation yet.
                // This fixture keeps quota/model data private behind the
                // startup surface so the X11 evidence can inspect the exact
                // no-partial-frame contract.
                state.authenticated = true;
                state.has_usage = true;
                state.local_usage_pending = true;
                state.usage_snapshot_committed = false;
                state.last_success_at = None;
                state.status = "認証済みです。利用量を取得しています…".into();
            }
            "initializing" => {
                // Match the first safe public snapshot: the native application
                // has started its read but has not established either identity
                // or usage data yet. This gives the REST client a deterministic
                // visual fixture without changing live authentication state.
                state.authenticated = false;
                state.email = None;
                state.plan_label.clear();
                state.remaining_percent = None;
                state.has_quota_percent = false;
                state.has_usage = false;
                state.reset_at = None;
                state.last_success_at = None;
                state.model_usage.clear();
                state.active_threads.clear();
                state.estimated_cost_label = "概算 —".into();
                state.history = UsageHistory::default();
                state.selected_reset_at = None;
                state.selected_history_period = "履歴なし".into();
                state.checking = true;
                // Keep the canonical internal status string so `display_status`
                // resolves it through the startup-pinned catalog just like a
                // live first request does.
                state.status = "Codex app-serverへ接続しています…".into();
            }
            "auth" => {
                state.authenticated = false;
                state.email = None;
                state.remaining_percent = None;
                state.has_quota_percent = false;
                state.has_usage = false;
                state.reset_at = None;
                state.active_threads.clear();
                state.status = "未認証です。認証を開始してください。".into();
            }
            "idle" => {
                state.active_threads.clear();
                state.status = state.normal_status();
            }
            "multi-thread" => {
                // Keep this preview deliberately dense so the fixed detail window
                // exercises parent/child relationships and vertical scrolling.
                // Input is deliberately shuffled and every child is newer than
                // its parent so the presentation projection proves parent-first
                // subtree ordering instead of inheriting acquisition order.
                let model = "gpt-5.6-sol-subagent-review".to_owned();
                state.active_threads = vec![
                    ActiveThread {
                        id: "thread-child-tests".into(),
                        created_at: Some(now - 1_800),
                        updated_at: now - 1,
                        title: "複数候補と回帰テストを確認しているサブスレッド".into(),
                        model: "gpt-5.6-luna".into(),
                        model_label: "gpt-5.6-luna".into(),
                        total_tokens: Some(123_456),
                        context_usage_tokens: Some(123_456),
                        context_window_tokens: Some(258_400),
                        last_user_message_at: Some(now - 12),
                        is_subagent: true,
                        parent_thread_id: Some("thread-z".into()),
                        depth: Some(1),
                    },
                    ActiveThread {
                        id: "thread-orphan".into(),
                        created_at: Some(now - 7_200),
                        updated_at: now - 30,
                        title: "親が完了した後も実行中のサブスレッド".into(),
                        model: "gpt-5.6-luna".into(),
                        model_label: "gpt-5.6-luna".into(),
                        total_tokens: Some(43_210),
                        context_usage_tokens: Some(43_210),
                        context_window_tokens: Some(258_400),
                        last_user_message_at: Some(now - 90),
                        is_subagent: true,
                        parent_thread_id: Some("completed-parent".into()),
                        depth: Some(1),
                    },
                    ActiveThread {
                        id: "thread-grandchild-security".into(),
                        created_at: Some(now - 3_600),
                        updated_at: now + 1,
                        title: "脆弱性境界を確認する孫サブスレッド".into(),
                        model: "gpt-5.6-luna".into(),
                        model_label: "gpt-5.6-luna".into(),
                        total_tokens: Some(88_765),
                        context_usage_tokens: Some(88_765),
                        context_window_tokens: Some(258_400),
                        last_user_message_at: Some(now - 25),
                        is_subagent: true,
                        parent_thread_id: Some("thread-child-review".into()),
                        depth: Some(2),
                    },
                    ActiveThread {
                        id: "thread-second-child".into(),
                        created_at: Some(now - 5_400),
                        updated_at: now - 5,
                        title: "別の親に属するサブスレッド".into(),
                        model: "gpt-5.6-terra".into(),
                        model_label: "gpt-5.6-terra".into(),
                        total_tokens: Some(54_321),
                        context_usage_tokens: Some(54_321),
                        context_window_tokens: Some(200_000),
                        last_user_message_at: Some(now - 40),
                        is_subagent: true,
                        parent_thread_id: Some("thread-second-parent".into()),
                        depth: Some(1),
                    },
                    ActiveThread {
                        id: "thread-z".into(),
                        created_at: Some(now - 14_400),
                        updated_at: now - 10,
                        title: "利用状況画面を更新する親スレッド".into(),
                        model: model.clone(),
                        model_label: security::bounded_model_label(&model)
                            .expect("preview model is within the accepted bound"),
                        total_tokens: Some(9_876_543_210),
                        context_usage_tokens: Some(245_000),
                        context_window_tokens: Some(258_400),
                        last_user_message_at: Some(now - 60),
                        is_subagent: false,
                        parent_thread_id: None,
                        depth: None,
                    },
                    ActiveThread {
                        id: "thread-review-source".into(),
                        created_at: Some(now - 9_000),
                        updated_at: now - 40,
                        title: "親IDを持たないレビュー用サブスレッド".into(),
                        model: "gpt-5.6-terra".into(),
                        model_label: "gpt-5.6-terra".into(),
                        total_tokens: Some(32_109),
                        context_usage_tokens: Some(32_109),
                        context_window_tokens: Some(200_000),
                        last_user_message_at: Some(now - 120),
                        is_subagent: true,
                        parent_thread_id: None,
                        depth: None,
                    },
                    ActiveThread {
                        id: "thread-second-parent".into(),
                        created_at: Some(now - 10_800),
                        updated_at: now - 20,
                        title: "別の作業を進めている親スレッド".into(),
                        model: "gpt-5.6-sol".into(),
                        model_label: "gpt-5.6-sol".into(),
                        total_tokens: Some(765_432),
                        context_usage_tokens: Some(75_432),
                        context_window_tokens: Some(258_400),
                        last_user_message_at: Some(now - 180),
                        is_subagent: false,
                        parent_thread_id: None,
                        depth: None,
                    },
                    ActiveThread {
                        id: "thread-child-review".into(),
                        created_at: Some(now - 2_400),
                        updated_at: now,
                        title: "表示崩れを独立評価しているサブスレッド".into(),
                        model: "gpt-5.6-terra".into(),
                        model_label: "gpt-5.6-terra".into(),
                        total_tokens: Some(456_789),
                        context_usage_tokens: Some(156_789),
                        context_window_tokens: Some(200_000),
                        last_user_message_at: Some(now - 8),
                        is_subagent: true,
                        parent_thread_id: Some("thread-z".into()),
                        depth: Some(1),
                    },
                ];
                state.status = state.normal_status();
            }
            "graph-many" => {
                // Visual fixture: exercise the period list's bounded scroll
                // path with more entries than the four-row viewport.
                let mut samples = state.history.samples.clone();
                for index in 2..=7 {
                    let period_reset = reset_at.saturating_sub(index * WEEK_SECONDS);
                    samples.extend(
                        UsageHistory::preview(
                            now,
                            period_reset,
                            ModelDollarTotals::from_rows(&state.model_usage),
                        )
                        .samples,
                    );
                }
                state.history.samples = samples;
                state.selected_reset_at = Some(reset_at);
                state.status = state.normal_status();
            }
            "graph-collision" => {
                // Visual regression fixture for the affected history: an
                // unrelated 14% singleton shares a minute with the selected
                // period's 88% observation. The rendered graph must keep the
                // quota line monotone (88% -> 87%) and must not draw a
                // fabricated vertical 14% drop.
                let period_start = now.saturating_sub(4 * 86_400);
                let selected_reset = now + 3 * 86_400;
                let conflicting_reset = selected_reset + 80_000;
                let cumulative = ModelDollarTotals {
                    sol: preview_costs.sol,
                    terra: preview_costs.terra,
                    luna: preview_costs.luna,
                };
                state.history = UsageHistory {
                    samples: vec![
                        UsageHistorySample::new_with_usage(
                            period_start,
                            selected_reset,
                            88.0,
                            ModelDollarTotals {
                                sol: cumulative.sol * 0.35,
                                terra: cumulative.terra,
                                luna: cumulative.luna * 0.45,
                            },
                            ModelTokenTotals::default(),
                        ),
                        UsageHistorySample::new_with_usage(
                            period_start,
                            conflicting_reset,
                            14.0,
                            ModelDollarTotals::default(),
                            ModelTokenTotals::default(),
                        ),
                        UsageHistorySample::new_with_usage(
                            period_start + 2 * 86_400,
                            selected_reset,
                            87.0,
                            cumulative,
                            ModelTokenTotals::default(),
                        ),
                    ],
                    ..UsageHistory::default()
                };
                state.remaining_percent = Some(87.0);
                state.reset_at = Some(selected_reset);
                state.selected_reset_at = Some(selected_reset);
                state.status = state.normal_status();
            }
            "history-empty" => {
                state.history = UsageHistory::default();
                state.selected_reset_at = None;
                state.selected_history_period = "履歴なし".into();
                state.status = state.normal_status();
            }
            "monthly" => {
                let monthly_reset_at = now + 20 * 86_400 + 5 * 3_600;
                state.plan_label = "エンタープライズ".into();
                state.remaining_percent = Some(73.0);
                state.has_quota_percent = true;
                state.has_usage = true;
                state.reset_at = Some(monthly_reset_at);
                state.window_seconds = monthly_window_seconds(monthly_reset_at);
                state.quota_title = "月間残り利用枠".into();
                state.monthly = true;
                state.history = UsageHistory::preview(
                    now,
                    monthly_reset_at,
                    ModelDollarTotals::from_rows(&state.model_usage),
                );
                state.selected_reset_at = Some(monthly_reset_at);
                state.status = state.normal_status();
            }
            "unlimited" => {
                state.plan_label = "エンタープライズ".into();
                state.remaining_percent = None;
                state.has_quota_percent = false;
                state.has_usage = true;
                state.reset_at = None;
                state.model_usage.clear();
                state.window_seconds = WEEK_SECONDS;
                state.quota_title = "利用枠".into();
                state.monthly = false;
                state.history = UsageHistory::default();
                state.selected_reset_at = None;
                state.active_threads.clear();
                state.status = state.normal_status();
            }
            "warning" => {
                state.remaining_percent = Some(5.0);
                let warning_reset_at = now + 20 * 3_600;
                state.reset_at = Some(warning_reset_at);
                state.history = UsageHistory::preview(
                    now,
                    warning_reset_at,
                    ModelDollarTotals::from_rows(&state.model_usage),
                );
                state.selected_reset_at = Some(warning_reset_at);
                state.status = state.normal_status();
            }
            "reset-warning" => {
                state.remaining_percent = Some(50.0);
                let warning_reset_at = now + 20 * 3_600;
                state.reset_at = Some(warning_reset_at);
                state.history = UsageHistory::preview(
                    now,
                    warning_reset_at,
                    ModelDollarTotals::from_rows(&state.model_usage),
                );
                state.selected_reset_at = Some(warning_reset_at);
                state.status = state.normal_status();
            }
            "zero" => {
                state.remaining_percent = Some(0.0);
                state.status = state.normal_status();
            }
            "full" => {
                state.remaining_percent = Some(100.0);
                state.status = state.normal_status();
            }
            "error" => {
                state.error = Some("preview".into());
                state.status = state.i18n.format_stale_status(state.last_success_at);
            }
            _ => {
                state.status = state.normal_status();
            }
        }
        state
    }

    fn request_read(&mut self, status: &str) {
        if self.preview || self.service_endpoint_error.is_some() {
            return;
        }
        if !self.bridge.send(AccountCommand::Read) {
            self.bridge = AppServerBridge::<AccountCommand, Event>::start();
            if !self.bridge.send(AccountCommand::Read) {
                self.apply_account_error(
                    "Codex app-serverへ更新要求を送信できませんでした。".into(),
                );
                return;
            }
        }
        self.checking = true;
        self.status = status.into();
    }

    fn hold_service_endpoint_error(&mut self, error: String) {
        if self.service_endpoint_error.is_none() {
            self.advance_auth_epoch();
            self.thread_checking = false;
        }
        self.service_endpoint_error = Some(error.clone());
        self.checking = false;
        self.account_error = Some(error.clone());
        self.error = Some(error);
        self.status =
            "利用状況を取得できません。指定されたREST endpointへの接続を確認してください。".into();
    }

    fn clear_service_endpoint_error(&mut self) {
        if self.service_endpoint_error.take().is_some() {
            self.account_error = None;
            self.error = None;
            self.checking = true;
            self.status = "Codex app-serverへ接続しています…".into();
            self.last_poll = Instant::now();
        }
    }

    fn advance_auth_epoch(&mut self) {
        self.auth_epoch = self.auth_epoch.saturating_add(1);
    }

    fn stop_thread_bridge(&mut self) {
        if let Some(bridge) = self.thread_bridge.take() {
            let _ = bridge.send(ThreadCommand::Stop);
        }
    }

    fn ensure_thread_bridge(&mut self) {
        if !self.preview && self.thread_bridge.is_none() {
            self.thread_bridge = Some(AppServerBridge::<ThreadCommand, ThreadEvent>::start());
        }
    }

    fn request_thread_update(&mut self) {
        if self.preview || !self.authenticated {
            return;
        }
        self.ensure_thread_bridge();
        let command = ThreadCommand::Read {
            auth_epoch: self.auth_epoch,
        };
        let sent = self
            .thread_bridge
            .as_ref()
            .is_some_and(|bridge| bridge.send(command));
        if !sent {
            self.thread_bridge = Some(AppServerBridge::<ThreadCommand, ThreadEvent>::start());
        }
        if self
            .thread_bridge
            .as_ref()
            .is_some_and(|bridge| sent || bridge.send(command))
        {
            self.thread_checking = true;
            self.last_thread_poll = Instant::now();
        } else {
            self.apply_thread_error(
                self.auth_epoch,
                "スレッド取得workerへ要求を送信できませんでした。".into(),
            );
        }
    }

    fn request_local_usage(&mut self, reset_at: i64, window_seconds: i64) {
        if !self.preview && reset_at > 0 {
            let command = LocalCommand::Collect {
                auth_epoch: self.auth_epoch,
                reset_at,
                window_seconds,
            };
            if !self.local_bridge.send(command) {
                self.local_bridge = LocalUsageBridge::start();
                if !self.local_bridge.send(command) {
                    self.apply_local_usage_error(self.auth_epoch, reset_at, window_seconds);
                }
            }
        }
    }

    fn clear_account_visible_state(&mut self) {
        self.advance_auth_epoch();
        self.stop_thread_bridge();
        self.email = None;
        self.authenticated = false;
        self.plan_label.clear();
        self.auth_url = None;
        self.remaining_percent = None;
        self.has_quota_percent = false;
        self.has_usage = false;
        self.reset_at = None;
        self.window_seconds = WEEK_SECONDS;
        self.limit_name = "Codex".into();
        self.quota_title = "残り利用枠".into();
        self.monthly = false;
        self.account_error = None;
        self.error = None;
        self.last_success_at = None;
        self.model_usage.clear();
        self.active_threads.clear();
        self.estimated_cost_label = "概算 —".into();
        self.thread_checking = false;
        self.thread_error = false;
        self.local_usage_error = false;
        self.local_usage_pending = false;
        self.usage_snapshot_committed = false;
        self.recovery_requested = false;
        self.history = UsageHistory::default();
        self.selected_reset_at = None;
        self.selected_history_period = "履歴なし".into();
    }

    fn admit_active_thread_update(&mut self, update: ActiveThreadUpdate) -> bool {
        match update {
            ActiveThreadUpdate::Snapshot(threads) => {
                let public_threads = threads
                    .iter()
                    .map(ActiveThread::to_public_thread)
                    .collect::<Vec<_>>();
                if validate_public_threads(&public_threads).is_err() {
                    return true;
                }
                let topology = public_threads
                    .iter()
                    .map(|thread| ThreadTopologyNode {
                        id: thread.id.as_str(),
                        parent_thread_id: thread.parent_thread_id.as_deref(),
                    })
                    .collect::<Vec<_>>();
                if thread_contract::validate_selected_thread_topology(&topology).is_err() {
                    return true;
                }
                self.active_threads = threads;
                false
            }
            ActiveThreadUpdate::NoThread => {
                if validate_public_threads(&[]).is_err()
                    || thread_contract::validate_selected_thread_topology(&[]).is_err()
                {
                    return true;
                }
                self.active_threads.clear();
                false
            }
            // A failed read is not a valid empty candidate. Keep the last
            // complete rows and expose the existing failure state instead.
            ActiveThreadUpdate::Failed => true,
        }
    }

    fn apply_active_thread_update(&mut self, update: ActiveThreadUpdate) -> bool {
        self.admit_active_thread_update(update)
    }

    fn apply_usage_event(&mut self, event: UsageEvent) {
        let UsageEvent {
            remaining_percent,
            reset_at,
            window_seconds,
            limit_name,
            quota_title,
            monthly,
        } = event;
        let previous_reset_at = self.reset_at;
        let now = Utc::now().timestamp();
        let reset_changed = reset_transition_is_boundary(
            previous_reset_at,
            self.remaining_percent,
            reset_at,
            remaining_percent,
            self.last_success_at,
            now,
            self.window_seconds,
        );
        self.has_quota_percent = remaining_percent.is_some();
        self.has_usage = true;
        self.local_usage_pending = !self.preview;
        self.remaining_percent = remaining_percent.map(|value| value.clamp(0.0, 100.0));
        self.reset_at = (reset_at > 0).then_some(reset_at);
        self.window_seconds = window_seconds;
        self.recovery_period = (reset_at > 0).then_some((reset_at, window_seconds));
        if !self.preview && reset_at > 0 {
            // The daemon and the one-shot app-server recovery share this
            // bounded hint, but neither path ever reconstructs quota from
            // local logs.  A failed metadata write leaves the previous hint
            // untouched and does not invalidate the authenticated snapshot.
            let _ = daemon::persist_reset_hint(reset_at, window_seconds);
        }
        self.recovery_requested = false;
        self.limit_name = limit_name;
        self.quota_title = quota_title;
        self.monthly = monthly;
        self.account_error = None;
        if reset_changed {
            // A graph that was following the previously current period must
            // follow the newly announced period as soon as its local payload
            // arrives. Preserve an explicitly selected older period. The
            // previous complete model snapshot remains visible until the
            // matching local collector commits; reset_at can legitimately
            // drift while the service reports a rolling boundary.
            let follows_current = self.selected_reset_at.is_none()
                || previous_reset_at.is_some_and(|previous| {
                    self.selected_reset_at
                        .is_some_and(|selected| same_reset_period(selected, previous))
                });
            if follows_current {
                self.selected_reset_at = self.reset_at;
                self.selected_history_period.clear();
            }
        }
        if self.selected_reset_at.is_none() {
            self.selected_reset_at = self.reset_at;
        }
        self.checking = false;
        self.last_success_at = Some(now);
        debug_runtime(format!(
            "state usage applied authenticated={} reset_at={} window_seconds={} auth_epoch={}",
            self.authenticated, reset_at, window_seconds, self.auth_epoch
        ));
        // Quota is committed before the independent local worker is asked to
        // collect usage. The request carries the exact auth/period tuple.
        self.request_local_usage(reset_at, window_seconds);
        self.refresh_partial_failure_status();
    }

    fn apply_account_error(&mut self, error: String) {
        // The failed account connection is a publication boundary. Results
        // requested before this error may still be queued on the independent
        // thread/local channels, so invalidate their epoch without clearing
        // the last valid visible values. Let the thread scheduler issue a
        // fresh request instead of remaining stuck behind the stale one.
        self.advance_auth_epoch();
        self.thread_checking = false;
        self.checking = false;
        self.account_error = Some(error.clone());
        self.error = Some(error);
        self.status =
            "利用状況を取得できません。Codex app-serverへの接続を確認してください。".into();
        // Keep this latch set for the entire account outage. The account
        // bridge may respawn and report several errors before authentication
        // succeeds again; those retries must not rescan every JSONL file.
        if !self.recovery_requested {
            if let Some((reset_at, window_seconds)) = self.recovery_period {
                self.recovery_requested = true;
                self.request_local_usage(reset_at, window_seconds);
            }
        }
    }

    fn apply_account_event(
        &mut self,
        email: Option<String>,
        authenticated: bool,
        plan_type: Option<String>,
    ) {
        let was_authenticated = self.authenticated;
        let next_plan_label = plan_type_label(plan_type.as_deref());
        let account_changed = self.authenticated
            && authenticated
            && (self.email != email || self.plan_label != next_plan_label);
        let entering_authenticated = !was_authenticated && authenticated;
        if !authenticated || account_changed {
            self.clear_account_visible_state();
        } else if entering_authenticated {
            // No auxiliary request is admitted before authentication, so an
            // epoch change is enough. Keep the durable history loaded at
            // startup, or reload it after an unauthenticated clear.
            self.advance_auth_epoch();
            if self.history.samples.is_empty() {
                self.history = UsageHistory::load();
            }
        }
        self.email = email;
        self.authenticated = authenticated;
        self.plan_label = if authenticated {
            next_plan_label
        } else {
            String::new()
        };
        self.checking = authenticated;
        if authenticated || was_authenticated {
            self.auth_polling = false;
        }
        if authenticated {
            self.auth_url = None;
        }
        self.status = if authenticated {
            "認証済みです。利用量を取得しています…"
        } else {
            "未認証です。認証を開始してください。"
        }
        .into();
        if authenticated
            && (entering_authenticated || account_changed || self.thread_bridge.is_none())
        {
            self.ensure_thread_bridge();
            self.request_thread_update();
        }
    }

    fn current_local_period_matches(&self, reset_at: i64, window_seconds: i64) -> bool {
        if self.authenticated {
            return self.window_seconds == window_seconds
                && if reset_at > 0 {
                    self.reset_at == Some(reset_at)
                } else {
                    self.reset_at.is_none()
                };
        }
        self.recovery_period == Some((reset_at, window_seconds))
    }

    fn apply_local_usage_success(&mut self, result: LocalUsageResult) {
        let period_matches = self.recovery_period == Some((result.reset_at, result.window_seconds));
        let unauthenticated_recovery = !self.authenticated && period_matches;
        let recovery_result = self.recovery_requested && period_matches;
        if (!unauthenticated_recovery && result.auth_epoch != self.auth_epoch)
            || !self.current_local_period_matches(result.reset_at, result.window_seconds)
        {
            debug_runtime(format!(
                "local result discarded epoch={} current_epoch={} period_match={}",
                result.auth_epoch,
                self.auth_epoch,
                self.current_local_period_matches(result.reset_at, result.window_seconds)
            ));
            return;
        }
        let model_costs = result.model_usage.dollar_totals();
        let model_tokens = result.model_usage.token_totals();
        let history_sample_count = result.history_samples.len();
        self.local_usage_error = false;
        self.local_usage_pending = false;
        // A recovery result closes the one-shot backfill, but it must remain
        // marked as attempted until a fresh authenticated quota event arrives.
        // Otherwise an app-server restart loop would launch the same full
        // session scan once per failed account worker instead of once per
        // outage period.
        if !recovery_result {
            self.recovery_requested = false;
        }
        if unauthenticated_recovery && self.history.db_path.is_none() {
            // Reattach the durable store only at the recovery commit boundary;
            // an unauthenticated clear still keeps all visible history empty.
            self.history = UsageHistory::load();
        }
        if self.authenticated {
            self.model_usage = result.model_usage.rows();
            self.estimated_cost_label = format_estimated_cost(model_costs);
            self.usage_snapshot_committed = true;
        }
        if !self.preview {
            self.history
                .apply_backfill_samples(result.reset_at, result.history_samples);
        }
        if let Some(remaining_percent) = self.remaining_percent {
            self.history.record(UsageHistorySample::new_with_usage(
                Utc::now().timestamp(),
                result.reset_at,
                remaining_percent,
                model_costs,
                model_tokens,
            ));
        }
        self.refresh_partial_failure_status();
        debug_runtime(format!(
            "state local usage applied rows={} history_samples={} history_total={}",
            self.model_usage.len(),
            history_sample_count,
            self.history.samples.len()
        ));
    }

    fn apply_local_usage_error(&mut self, auth_epoch: u64, reset_at: i64, window_seconds: i64) {
        if !self.authenticated
            || auth_epoch != self.auth_epoch
            || !self.current_local_period_matches(reset_at, window_seconds)
        {
            return;
        }
        self.local_usage_error = true;
        self.local_usage_pending = false;
        self.recovery_requested = false;
        self.refresh_partial_failure_status();
    }

    fn apply_thread_result(&mut self, auth_epoch: u64, update: ActiveThreadUpdate) {
        if !self.authenticated || auth_epoch != self.auth_epoch {
            return;
        }
        let failed = self.apply_active_thread_update(update);
        self.thread_checking = false;
        self.thread_error = failed;
        self.last_thread_poll = Instant::now();
        debug_runtime(format!(
            "state thread result rows={}",
            self.active_threads.len()
        ));
        self.refresh_partial_failure_status();
    }

    fn apply_thread_error(&mut self, auth_epoch: u64, message: String) {
        if !self.authenticated || auth_epoch != self.auth_epoch {
            return;
        }
        self.thread_checking = false;
        // The worker could not establish a fresh live snapshot. Keep the
        // previous complete rows while exposing the existing error state.
        self.thread_error = true;
        let _ = message;
        self.refresh_partial_failure_status();
    }

    fn refresh_partial_failure_status(&mut self) {
        if let Some(account_error) = self.account_error.clone() {
            self.error = Some(account_error);
            self.status =
                "利用状況を取得できません。Codex app-serverへの接続を確認してください。".into();
            return;
        }
        if self.local_usage_pending {
            self.error = None;
            self.status = "利用量と履歴を取得しています…".into();
            return;
        }
        match (self.local_usage_error, self.thread_error) {
            (true, true) => {
                self.error =
                    Some("ローカル履歴とスレッド情報を安全に取得できませんでした。".into());
                self.status = "利用枠は更新しました。履歴とスレッド情報の取得に失敗し、実行中の状態は未確認です。"
                    .into();
            }
            (true, false) => {
                self.error = Some("ローカル利用履歴を安全に集計できませんでした。".into());
                self.status = "利用枠は更新しました。履歴は前回値を保持しています。".into();
            }
            (false, true) => {
                self.error = Some("スレッド情報を安全に取得できませんでした。".into());
                self.status =
                    "利用枠は更新しました。スレッド情報の取得に失敗し、実行中の状態は未確認です。"
                        .into();
            }
            (false, false) => {
                self.error = None;
                self.status = self.normal_status();
            }
        }
    }

    /// Apply one FIFO batch from the current account bridge. An account error
    /// invalidates the connection, so later events already drained from that
    /// same receiver must not cross the replacement boundary.
    fn apply_account_event_batch(&mut self, events: Vec<Event>) -> bool {
        for event in events {
            match event {
                Event::Ready => {
                    if self.account_error.is_none() && self.checking {
                        self.status = "認証状態を確認しています…".into();
                    }
                }
                Event::Account {
                    email,
                    authenticated,
                    plan_type,
                } => self.apply_account_event(email, authenticated, plan_type),
                Event::AuthUrl(url) => {
                    self.auth_url = Some(url);
                    self.checking = false;
                    self.auth_polling = false;
                    self.account_error = None;
                    self.error = None;
                    self.status =
                        "認証URLを発行しました。「認証ページを開く」を押してください。".into();
                }
                Event::Usage(event) => self.apply_usage_event(*event),
                Event::Error(error) => {
                    self.apply_account_error(error);
                    return true;
                }
            }
        }
        false
    }

    fn poll(&mut self) {
        if self.preview {
            return;
        }
        let mut account_events = Vec::new();
        while let Ok(event) = self.bridge.rx.try_recv() {
            account_events.push(event);
        }
        if self.apply_account_event_batch(account_events) {
            // Do not respawn immediately on every initialize/protocol error.
            // The failed worker's channel is left as the retry sentinel;
            // the next scheduled/explicit read observes send failure and
            // creates one replacement. This bounds process churn while the
            // app-server is unavailable and keeps recovery one-shot.
            let _ = self.bridge.send(AccountCommand::Stop);
        }

        let mut thread_events = Vec::new();
        if let Some(bridge) = self.thread_bridge.as_ref() {
            while let Ok(event) = bridge.rx.try_recv() {
                thread_events.push(event);
            }
        }
        for event in thread_events {
            match event {
                ThreadEvent::Ready => {}
                ThreadEvent::Update { auth_epoch, update } => {
                    self.apply_thread_result(auth_epoch, update);
                }
                ThreadEvent::Error {
                    auth_epoch,
                    message,
                } => self.apply_thread_error(auth_epoch, message),
            }
        }

        let mut local_events = Vec::new();
        while let Ok(event) = self.local_bridge.rx.try_recv() {
            local_events.push(event);
        }
        for event in local_events {
            match event {
                LocalEvent::Usage(result) => self.apply_local_usage_success(result),
                LocalEvent::Error {
                    auth_epoch,
                    reset_at,
                    window_seconds,
                } => self.apply_local_usage_error(auth_epoch, reset_at, window_seconds),
            }
        }
    }

    #[allow(clippy::needless_return)]
    fn normal_status(&self) -> String {
        #[cfg(test)]
        {
            return normal_status_text(
                self.remaining_percent.unwrap_or(50.0),
                if self.has_quota_percent {
                    self.seconds_to_reset()
                } else {
                    i64::MAX
                },
                Some("12:34"),
            );
        }
        #[cfg(not(test))]
        {
            if !self.has_quota_percent {
                return self.i18n.format_last_updated(self.last_success_at);
            }
            let remaining = self.remaining_percent.unwrap_or(0.0);
            if remaining <= 2.0 {
                self.i18n.text(TextKey::QuotaNearlyGone).into()
            } else if remaining <= 10.0 {
                self.i18n.text(TextKey::QuotaLow).into()
            } else if self.seconds_to_reset().abs() <= 86_400 {
                self.i18n.text(TextKey::ResetWithinDay).into()
            } else {
                self.i18n.format_last_updated(self.last_success_at)
            }
        }
    }

    fn history_periods(&self) -> Vec<HistoryPeriod> {
        self.history_periods_at(Utc::now().timestamp())
    }

    fn history_periods_at(&self, observed_at: i64) -> Vec<HistoryPeriod> {
        let projected_history = self.projected_history();
        let mut periods = projected_history.periods(observed_at, self.reset_at);
        periods = apply_authoritative_current_bounds(
            periods,
            &projected_history.samples,
            self.reset_at,
            self.window_seconds,
            observed_at,
        );
        periods.retain(|period| {
            DateTime::<Utc>::from_timestamp(period.start, 0).is_some()
                && DateTime::<Utc>::from_timestamp(period.end, 0).is_some()
                && DateTime::<Utc>::from_timestamp(period.canonical_reset_at, 0).is_some()
        });
        let current_period_reset =
            current_history_period_reset(&periods, self.reset_at, observed_at);
        for period in &mut periods {
            let is_current = current_period_reset == Some(period.canonical_reset_at);
            // The visible current period runs through its next reset, while
            // `end` is intentionally clipped to `now` for graph rendering.
            let label_end = if is_current {
                period.canonical_reset_at
            } else {
                period.end
            };
            let Some(mut label) = self.i18n.format_period(period.start, label_end) else {
                period.label.clear();
                continue;
            };
            if is_current {
                label.push_str(self.i18n.text(TextKey::CurrentSuffix));
            }
            period.label = label;
        }
        let base_labels = periods
            .iter()
            .map(|period| period.label.clone())
            .collect::<Vec<_>>();
        for index in 0..periods.len() {
            if base_labels
                .iter()
                .filter(|label| **label == base_labels[index])
                .count()
                > 1
            {
                if let Some(suffix) = self
                    .i18n
                    .format_deadline_suffix(periods[index].canonical_reset_at)
                {
                    periods[index].label.push_str(&suffix);
                }
            }
        }
        periods.retain(|period| !period.label.is_empty());
        periods
    }

    fn history_period_options(&self) -> Vec<String> {
        let periods = self.history_periods();
        if periods.is_empty() {
            vec![self.i18n.text(TextKey::NoHistory).into()]
        } else {
            periods.into_iter().map(|period| period.label).collect()
        }
    }

    fn selected_history_period_label(&self) -> String {
        let periods = self.history_periods();
        if let Some(period) = periods
            .iter()
            .find(|period| period.label == self.selected_history_period)
        {
            return period.label.clone();
        }
        if let Some(selected) = self.selected_reset_at {
            if let Some(period) = periods
                .iter()
                .find(|period| period.canonical_reset_at == selected)
            {
                return period.label.clone();
            }
        }
        if let Some(current) = self.reset_at {
            if let Some(period) = periods.iter().find(|period| {
                period.canonical_reset_at.abs_diff(current) <= RESET_AT_TOLERANCE_SECONDS as u64
            }) {
                return period.label.clone();
            }
        }
        periods
            .first()
            .map(|period| period.label.clone())
            .unwrap_or_else(|| self.i18n.text(TextKey::NoHistory).into())
    }

    fn select_history(&mut self, label: &str) {
        if let Some(period) = self
            .history_periods()
            .into_iter()
            .find(|period| period.label == label)
        {
            self.selected_history_period = label.into();
            self.selected_reset_at = Some(period.canonical_reset_at);
        }
    }

    fn select_metric(&mut self, metric: &str) {
        if metric == "ドル" || metric == self.i18n.text(TextKey::DollarMetric) {
            self.selected_metric = "ドル".into();
        } else if metric == "トークン" || metric == self.i18n.text(TextKey::TokenMetric) {
            self.selected_metric = "トークン".into();
        }
    }

    fn graph_data(&self) -> String {
        let Some(reset_at) = self.selected_history_reset() else {
            return "[]".into();
        };
        self.projected_history().graph_data_for_reset(reset_at)
    }

    fn selected_history_reset(&self) -> Option<i64> {
        let periods = self.history_periods();
        periods
            .iter()
            .find(|period| period.label == self.selected_history_period)
            .map(|period| period.canonical_reset_at)
            .or_else(|| {
                self.selected_reset_at.and_then(|selected| {
                    periods
                        .iter()
                        .find(|period| {
                            period.canonical_reset_at.abs_diff(selected)
                                <= RESET_AT_TOLERANCE_SECONDS as u64
                        })
                        .map(|period| period.canonical_reset_at)
                })
            })
            .or(self.reset_at)
            .or_else(|| periods.first().map(|period| period.canonical_reset_at))
    }

    fn select_latest_history(&mut self) {
        let periods = self.history_periods();
        let selected = self
            .reset_at
            .and_then(|reset| {
                periods
                    .iter()
                    .find(|period| {
                        period.canonical_reset_at.abs_diff(reset)
                            <= RESET_AT_TOLERANCE_SECONDS as u64
                    })
                    .or_else(|| periods.first())
            })
            .or_else(|| periods.first());
        if let Some(period) = selected {
            self.selected_history_period = period.label.clone();
            self.selected_reset_at = Some(period.canonical_reset_at);
        } else {
            self.selected_history_period = "履歴なし".into();
            self.selected_reset_at = None;
        }
    }

    fn select_older_history(&mut self) {
        let periods = self.history_periods();
        let Some(current) = self.selected_history_reset() else {
            return;
        };
        if let Some(index) = periods
            .iter()
            .position(|period| period.canonical_reset_at == current)
        {
            if let Some(period) = periods.get(index + 1) {
                self.select_history(&period.label.clone());
            }
        }
    }

    #[cfg(test)]
    fn select_newer_history(&mut self) {
        let periods = self.history_periods();
        let Some(current) = self.selected_history_reset() else {
            return;
        };
        if let Some(index) = periods
            .iter()
            .position(|period| period.canonical_reset_at == current)
        {
            if index > 0 {
                if let Some(period) = periods.get(index - 1) {
                    self.select_history(&period.label.clone());
                }
            }
        }
    }

    #[cfg(test)]
    fn history_navigation(&self) -> (bool, bool) {
        let periods = self.history_periods();
        let Some(current) = self.selected_history_reset() else {
            return (false, false);
        };
        let Some(index) = periods
            .iter()
            .position(|period| period.canonical_reset_at == current)
        else {
            return (false, false);
        };
        (index + 1 < periods.len(), index > 0)
    }

    fn period_seconds_for_reset(&self, reset_at: i64) -> i64 {
        let current_period_seconds = if self.monthly {
            monthly_window_seconds(reset_at)
        } else {
            self.window_seconds.max(WEEK_SECONDS)
        };
        if self
            .reset_at
            .is_some_and(|current| same_reset_period(current, reset_at))
        {
            return current_period_seconds;
        }

        // Historical periods do not inherit the current plan's calendar
        // month. Use the nearest newer reset as the period boundary when the
        // observed distance is plausible; this keeps an old weekly period
        // from being rendered as a 31-day window after a monthly switch.
        let periods = self.history.reset_periods_desc();
        if let Some(index) = periods
            .iter()
            .position(|period| same_reset_period(*period, reset_at))
        {
            if let Some(newer_reset) = index.checked_sub(1).and_then(|i| periods.get(i)) {
                let distance = newer_reset.saturating_sub(reset_at);
                if (3_600..=45 * 86_400).contains(&distance) {
                    return distance;
                }
            }
        }
        self.window_seconds.max(WEEK_SECONDS)
    }

    #[allow(dead_code)]
    fn graph_paths_for_selection(
        &self,
        show_luna: bool,
        show_terra: bool,
        show_sol: bool,
        show_tokens: bool,
    ) -> GraphPaths {
        self.graph_paths_for_selection_at(
            Utc::now().timestamp(),
            show_luna,
            show_terra,
            show_sol,
            show_tokens,
        )
    }

    fn selected_history_reset_for_periods(&self, periods: &[HistoryPeriod]) -> Option<i64> {
        if let Some(period) = periods
            .iter()
            .find(|period| period.label == self.selected_history_period)
        {
            return Some(period.canonical_reset_at);
        }
        if let Some(selected) = self.selected_reset_at {
            return periods
                .iter()
                .find(|period| {
                    period.canonical_reset_at.abs_diff(selected)
                        <= RESET_AT_TOLERANCE_SECONDS as u64
                })
                .map(|period| period.canonical_reset_at);
        }
        if let Some(current) = self.reset_at {
            return periods
                .iter()
                .find(|period| {
                    period.canonical_reset_at.abs_diff(current) <= RESET_AT_TOLERANCE_SECONDS as u64
                })
                .map(|period| period.canonical_reset_at);
        }
        periods.first().map(|period| period.canonical_reset_at)
    }

    fn graph_paths_for_selection_at(
        &self,
        observed_at: i64,
        show_luna: bool,
        show_terra: bool,
        show_sol: bool,
        show_tokens: bool,
    ) -> GraphPaths {
        let periods = self.history_periods_at(observed_at);
        let Some(selected_reset) = self.selected_history_reset_for_periods(&periods) else {
            return GraphPaths::default();
        };
        let samples = self
            .projected_history()
            .samples_for_reset(Some(selected_reset));
        let Some(period) = periods
            .iter()
            .find(|period| period.canonical_reset_at == selected_reset)
        else {
            return GraphPaths::default();
        };
        let period_start = period.start;
        let period_end = period.end.max(period_start + 1);
        let sample_references = samples.iter().collect::<Vec<_>>();
        let mut paths = graph_paths_for_selection(
            &sample_references,
            period_start,
            period_end,
            show_luna,
            show_terra,
            show_sol,
            show_tokens,
        );
        if !self.has_quota_percent {
            paths.remaining.clear();
            paths.remaining_markers.clear();
            paths.current_remaining_label.clear();
            paths.current_remaining_y = 0.99;
        }
        paths
    }
}

fn sync_graph_window(state: &CodexInfoState, graph: &GraphWindow) {
    graph.set_strings(ui_strings(&state.i18n));
    graph.set_window_title(
        native_detail_window_title(
            &state.i18n,
            state.authenticated,
            &state.window_title(),
            WindowPurpose::Graph,
        )
        .into(),
    );
    let token_metric = state.selected_metric == "トークン"
        || state.selected_metric == state.i18n.text(TextKey::TokenMetric);
    graph.set_show_tokens(token_metric);
    let observed_at = Utc::now().timestamp();
    let mut paths = state.graph_paths_for_selection_at(
        observed_at,
        graph.get_show_luna(),
        graph.get_show_terra(),
        graph.get_show_sol(),
        graph.get_show_tokens(),
    );
    separate_current_label_positions(
        &mut paths,
        graph.get_show_remaining(),
        graph.get_show_luna(),
        graph.get_show_terra(),
        graph.get_show_sol(),
    );
    let time_labels = state.graph_time_labels_at(observed_at);
    graph.set_graph_data(state.graph_data().into());
    graph.set_unused_intervals(slint::ModelRc::new(slint::VecModel::from(
        paths
            .unused_intervals
            .iter()
            .map(|interval| GraphUnusedInterval {
                start: interval.start as f32,
                width: interval.width as f32,
            })
            .collect::<Vec<_>>(),
    )));
    let history_period_options = state.history_period_options();
    graph.set_has_history_options(
        !history_period_options.is_empty()
            && history_period_options[0] != state.i18n.text(TextKey::NoHistory),
    );
    let selected_history_period = state.selected_history_period_label();
    let selected_history_index = history_period_options
        .iter()
        .position(|period| period == &selected_history_period)
        .unwrap_or(0);
    graph.set_history_period_options(slint::ModelRc::new(slint::VecModel::from(
        history_period_options
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    )));
    graph.set_selected_history_index(i32::try_from(selected_history_index).unwrap_or(i32::MAX));
    graph.set_metric_options(slint::ModelRc::new(slint::VecModel::from(vec![
        slint::SharedString::from(state.i18n.text(TextKey::DollarMetric)),
        slint::SharedString::from(state.i18n.text(TextKey::TokenMetric)),
    ])));
    graph.set_selected_metric_index(if token_metric { 1 } else { 0 });
    graph.set_time_start_label(time_labels[0].clone().into());
    graph.set_time_25_label(time_labels[1].clone().into());
    graph.set_time_50_label(time_labels[2].clone().into());
    graph.set_time_75_label(time_labels[3].clone().into());
    graph.set_time_end_label(time_labels[4].clone().into());
    graph.set_remaining_path(paths.remaining.into());
    graph.set_remaining_markers(slint::ModelRc::new(slint::VecModel::from(
        paths
            .remaining_markers
            .iter()
            .map(|marker| RemainingMarker {
                x: marker.x as f32,
                y: marker.y as f32,
            })
            .collect::<Vec<_>>(),
    )));
    graph.set_sol_flat_path(paths.sol_flat.into());
    graph.set_sol_rising_path(paths.sol_rising.into());
    graph.set_terra_flat_path(paths.terra_flat.into());
    graph.set_terra_rising_path(paths.terra_rising.into());
    graph.set_luna_flat_path(paths.luna_flat.into());
    graph.set_luna_rising_path(paths.luna_rising.into());
    graph.set_dollar_top_label(paths.dollar_labels[0].clone().into());
    graph.set_dollar_75_label(paths.dollar_labels[1].clone().into());
    graph.set_dollar_50_label(paths.dollar_labels[2].clone().into());
    graph.set_dollar_25_label(paths.dollar_labels[3].clone().into());
    graph.set_dollar_bottom_label(paths.dollar_labels[4].clone().into());
    let has_current_remaining_label = !paths.current_remaining_label.is_empty();
    let has_current_sol_label = !paths.current_sol_label.is_empty();
    let has_current_terra_label = !paths.current_terra_label.is_empty();
    let has_current_luna_label = !paths.current_luna_label.is_empty();
    graph.set_current_remaining_label(paths.current_remaining_label.into());
    graph.set_current_sol_label(paths.current_sol_label.into());
    graph.set_current_terra_label(paths.current_terra_label.into());
    graph.set_current_luna_label(paths.current_luna_label.into());
    graph.set_current_remaining_connector_path(
        current_label_connector_path(
            paths.current_remaining_point_y,
            paths.current_remaining_y,
            has_current_remaining_label,
        )
        .into(),
    );
    graph.set_current_sol_connector_path(
        current_label_connector_path(
            paths.current_sol_point_y,
            paths.current_sol_y,
            has_current_sol_label,
        )
        .into(),
    );
    graph.set_current_terra_connector_path(
        current_label_connector_path(
            paths.current_terra_point_y,
            paths.current_terra_y,
            has_current_terra_label,
        )
        .into(),
    );
    graph.set_current_luna_connector_path(
        current_label_connector_path(
            paths.current_luna_point_y,
            paths.current_luna_y,
            has_current_luna_label,
        )
        .into(),
    );
    graph.set_current_remaining_y(paths.current_remaining_y);
    graph.set_current_sol_y(paths.current_sol_y);
    graph.set_current_terra_y(paths.current_terra_y);
    graph.set_current_luna_y(paths.current_luna_y);
}

fn classify_active_thread_model(model_label: &str) -> &'static str {
    let mut match_name = None;
    let mut known_count = 0;
    for token in model_label
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let candidate = match token {
            "sol" => Some("SOL"),
            "terra" => Some("TERRA"),
            "luna" => Some("LUNA"),
            _ => None,
        };
        if let Some(candidate) = candidate {
            known_count += 1;
            match_name = Some(candidate);
        }
    }
    if known_count == 1 {
        match_name.unwrap_or("OTHER")
    } else {
        "OTHER"
    }
}

#[cfg(test)]
fn active_thread_model_counts(threads: &[ActiveThread]) -> String {
    if threads.is_empty() {
        return String::new();
    }
    let [sol, terra, luna, other] = active_thread_model_count_values(threads);
    format!("SOL {sol}  TERRA {terra}  LUNA {luna}  その他 {other}")
}

fn active_thread_model_count_values(threads: &[ActiveThread]) -> [i32; 4] {
    let mut sol = 0usize;
    let mut terra = 0usize;
    let mut luna = 0usize;
    let mut other = 0usize;
    for thread in threads {
        match classify_active_thread_model(&thread.model_label) {
            "SOL" => sol += 1,
            "TERRA" => terra += 1,
            "LUNA" => luna += 1,
            _ => other += 1,
        }
    }
    [
        i32::try_from(sol).unwrap_or(i32::MAX),
        i32::try_from(terra).unwrap_or(i32::MAX),
        i32::try_from(luna).unwrap_or(i32::MAX),
        i32::try_from(other).unwrap_or(i32::MAX),
    ]
}

#[cfg(test)]
fn format_elapsed(now: i64, timestamp: Option<i64>) -> String {
    let Some(timestamp) = timestamp else {
        return "—".into();
    };
    if DateTime::<Utc>::from_timestamp(timestamp, 0).is_none() {
        return "—".into();
    }
    let age = now.saturating_sub(timestamp).max(0);
    if age < 60 {
        format!("{age}秒")
    } else if age < 3_600 {
        let minutes = age / 60;
        let seconds = age % 60;
        if seconds == 0 {
            format!("{minutes}分")
        } else {
            format!("{minutes}分{seconds}秒")
        }
    } else if age < 86_400 {
        let hours = age / 3_600;
        let minutes = (age % 3_600) / 60;
        if minutes == 0 {
            format!("{hours}時間")
        } else {
            format!("{hours}時間{minutes}分")
        }
    } else {
        let days = age / 86_400;
        let hours = (age % 86_400) / 3_600;
        if hours == 0 {
            format!("{days}日")
        } else {
            format!("{days}日{hours}時間")
        }
    }
}

fn sort_thread_indices(indices: &mut [usize], threads: &[ActiveThread]) {
    indices.sort_by(|left, right| {
        threads[*right]
            .updated_at
            .cmp(&threads[*left].updated_at)
            .then_with(|| threads[*right].id.cmp(&threads[*left].id))
    });
}

fn push_thread_subtree(
    index: usize,
    forest_depth: usize,
    has_next_sibling: bool,
    ancestor_guides: [bool; 3],
    children: &[Vec<usize>],
    visited: &mut [bool],
    rows: &mut Vec<ThreadPresentationRow>,
) {
    if visited[index] {
        return;
    }
    visited[index] = true;
    rows.push(ThreadPresentationRow {
        index,
        forest_depth,
        connected_to_parent: forest_depth > 0,
        has_children: !children[index].is_empty(),
        has_next_sibling: forest_depth > 0 && has_next_sibling,
        ancestor_guides,
    });

    let child_count = children[index].len();
    for (position, child) in children[index].iter().copied().enumerate() {
        let mut child_guides = ancestor_guides;
        if forest_depth > 0 {
            let visible_level = forest_depth.min(3);
            // A display depth of three is a capped lane. Once any ancestor
            // at that lane needs a continuation, a deeper descendant must
            // keep the guide even when its immediate parent is the last
            // sibling; assignment here would incorrectly erase that path.
            child_guides[visible_level - 1] |= has_next_sibling;
        }
        push_thread_subtree(
            child,
            forest_depth.saturating_add(1),
            position + 1 < child_count,
            child_guides,
            children,
            visited,
            rows,
        );
    }
}

fn thread_presentation_rows(threads: &[ActiveThread]) -> Vec<ThreadPresentationRow> {
    let mut by_id = BTreeMap::new();
    for (index, thread) in threads.iter().enumerate() {
        by_id.entry(thread.id.as_str()).or_insert(index);
    }

    let parent_indices = threads
        .iter()
        .map(|thread| {
            thread
                .parent_thread_id
                .as_deref()
                .and_then(|parent_id| by_id.get(parent_id).copied())
        })
        .collect::<Vec<_>>();
    let mut children = vec![Vec::new(); threads.len()];
    let mut roots = Vec::new();
    for (index, parent) in parent_indices.iter().copied().enumerate() {
        if let Some(parent) = parent {
            children[parent].push(index);
        } else {
            roots.push(index);
        }
    }
    sort_thread_indices(&mut roots, threads);
    for siblings in &mut children {
        sort_thread_indices(siblings, threads);
    }

    let mut visited = vec![false; threads.len()];
    let mut rows = Vec::with_capacity(threads.len());
    for root in roots {
        push_thread_subtree(
            root,
            0,
            false,
            [false; 3],
            &children,
            &mut visited,
            &mut rows,
        );
    }

    // Native acquisition rejects cycles atomically. This deterministic
    // fallback keeps hand-built/defensive inputs total without guessing an
    // edge: every unreachable node becomes one disconnected top-level row.
    let mut disconnected = visited
        .iter()
        .enumerate()
        .filter_map(|(index, was_visited)| (!*was_visited).then_some(index))
        .collect::<Vec<_>>();
    sort_thread_indices(&mut disconnected, threads);
    for index in disconnected {
        visited[index] = true;
        rows.push(ThreadPresentationRow {
            index,
            forest_depth: 0,
            connected_to_parent: false,
            has_children: false,
            has_next_sibling: false,
            ancestor_guides: [false; 3],
        });
    }
    rows
}

fn active_thread_rows_at_with_i18n(
    threads: &[ActiveThread],
    now: i64,
    i18n: &I18n,
) -> Vec<ActiveThreadRow> {
    thread_presentation_rows(threads)
        .into_iter()
        .map(|presentation| {
            let thread = &threads[presentation.index];
            let relation = if thread.is_subagent {
                let depth = if presentation.connected_to_parent {
                    i32::try_from(presentation.forest_depth).ok()
                } else {
                    thread.depth.filter(|depth| *depth > 0)
                };
                match depth {
                    Some(depth) if depth > 99 => format!("{} D99+", i18n.text(TextKey::SubRole)),
                    Some(depth) => format!("{} D{depth}", i18n.text(TextKey::SubRole)),
                    None => i18n.text(TextKey::SubRole).to_owned(),
                }
            } else {
                i18n.text(TextKey::MainRole).to_owned()
            };
            let parent_title = thread
                .parent_thread_id
                .as_deref()
                .map(|parent_id| {
                    threads
                        .iter()
                        .find(|candidate| candidate.id == parent_id)
                        .map(|parent| i18n.format_parent_title(&parent.title))
                        .unwrap_or_else(|| i18n.text(TextKey::ParentNotRunning).to_owned())
                })
                .unwrap_or_default();
            ActiveThreadRow {
                relation: relation.into(),
                is_main: !thread.is_subagent,
                title: security::shorten_unicode(&thread.title, security::MAX_THREAD_TITLE_SCALARS)
                    .into(),
                parent_title: security::shorten_unicode(
                    &parent_title,
                    security::MAX_THREAD_TITLE_SCALARS,
                )
                .into(),
                model: security::shorten_unicode(
                    &thread.model_label,
                    security::MAX_ACCOUNT_ACTIVITY_LABEL_SCALARS,
                )
                .into(),
                tokens: thread
                    .total_tokens
                    .map(|total| i18n.format_token_value(total))
                    .unwrap_or_else(|| "—".to_owned())
                    .into(),
                context_usage: match (thread.context_usage_tokens, thread.context_window_tokens) {
                    (Some(used), Some(window)) if window > 0 => format!(
                        "{} / {}",
                        i18n.format_context_usage(used, window),
                        i18n.format_token_value(window)
                    ),
                    _ => "—".to_owned(),
                }
                .into(),
                thread_age: i18n.format_elapsed(now, thread.created_at).into(),
                instruction_age: i18n.format_elapsed(now, thread.last_user_message_at).into(),
                tree_depth: i32::try_from(presentation.forest_depth).unwrap_or(i32::MAX),
                connected_to_parent: presentation.connected_to_parent,
                has_children: presentation.has_children,
                has_next_sibling: presentation.has_next_sibling,
                ancestor_guide_1: presentation.ancestor_guides[0],
                ancestor_guide_2: presentation.ancestor_guides[1],
                ancestor_guide_3: presentation.ancestor_guides[2],
            }
        })
        .collect()
}

#[cfg(test)]
fn active_thread_rows_at(threads: &[ActiveThread], now: i64) -> Vec<ActiveThreadRow> {
    active_thread_rows_at_with_i18n(
        threads,
        now,
        &I18n::from_parts(codex_info::i18n::Language::Japanese, chrono_tz::Tz::UTC),
    )
}

#[cfg(test)]
#[allow(dead_code)]
fn active_thread_rows(threads: &[ActiveThread]) -> Vec<ActiveThreadRow> {
    active_thread_rows_at(threads, Utc::now().timestamp())
}

fn sync_threads_window(state: &CodexInfoState, threads_window: &ThreadsWindow) {
    threads_window.set_strings(ui_strings(&state.i18n));
    threads_window.set_thread_count_label(
        state
            .i18n
            .format_thread_count(state.active_threads.len())
            .into(),
    );
    threads_window.set_window_title(
        native_detail_window_title(
            &state.i18n,
            state.authenticated,
            &state.window_title(),
            WindowPurpose::Threads,
        )
        .into(),
    );
    threads_window.set_thread_rows(slint::ModelRc::new(slint::VecModel::from(
        active_thread_rows_at_with_i18n(&state.active_threads, Utc::now().timestamp(), &state.i18n),
    )));
}

const LEGAL_PAGE_CHUNK_SCALARS: usize = 620;
type NativeLegalPageCache =
    BTreeMap<&'static str, (Vec<slint::SharedString>, Vec<slint::SharedString>)>;

fn native_legal_pages(i18n: &I18n) -> (Vec<slint::SharedString>, Vec<slint::SharedString>) {
    static CACHE: OnceLock<Mutex<NativeLegalPageCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let language_code = i18n.language().code();
    if let Some((names, pages)) = cache
        .lock()
        .expect("native legal page cache lock")
        .get(language_code)
    {
        return (names.clone(), pages.clone());
    }

    let chapters = [
        (
            i18n.text(TextKey::LegalCode),
            include_str!("../LICENSE.ja.md"),
        ),
        (
            i18n.text(TextKey::LegalWarranty),
            include_str!("../LICENSE"),
        ),
        (i18n.text(TextKey::LegalLicense), include_str!("../LICENSE")),
        (
            i18n.text(TextKey::LegalFont),
            concat!(
                include_str!("../THIRD_PARTY_NOTICES.md"),
                "\n\n",
                include_str!("../LICENSES/OFL-1.1.txt")
            ),
        ),
        (
            i18n.text(TextKey::LegalProtocol),
            concat!(
                include_str!("../THIRD_PARTY_NOTICES.md"),
                "\n\n",
                include_str!("../LICENSES/Apache-2.0.txt"),
                "\n\n",
                include_str!("../LICENSES/OPENAI-CODEX-NOTICE.txt")
            ),
        ),
        (
            i18n.text(TextKey::LegalSchema),
            concat!(
                include_str!("../LICENSES/Apache-2.0.txt"),
                "\n\n",
                include_str!("../LICENSES/OPENAI-CODEX-NOTICE.txt")
            ),
        ),
        (
            i18n.text(TextKey::LegalThirdParty),
            concat!(
                include_str!("../THIRD_PARTY_NOTICES.md"),
                "\n\n",
                include_str!("../LICENSES/MIT.txt"),
                "\n\n",
                include_str!("../LICENSES/BSD-3-Clause-ANGLE.txt")
            ),
        ),
        (
            i18n.text(TextKey::LegalDetails),
            concat!(
                include_str!("../THIRD_PARTY_NOTICES.md"),
                "\n\n",
                include_str!("../assets/NOTICE.txt"),
                "\n\n",
                include_str!("../windows-client/THIRD_PARTY_NOTICES.md")
            ),
        ),
        (
            i18n.text(TextKey::LegalDistribution),
            concat!(
                include_str!("../LICENSE.ja.md"),
                "\n\n",
                include_str!("../LICENSES/Inno-Setup.txt"),
                "\n\n",
                include_str!("../windows-client/THIRD_PARTY_NOTICES.md")
            ),
        ),
    ];
    let mut names = Vec::new();
    let mut pages = Vec::new();
    for (name, source) in chapters {
        let chars: Vec<char> = source.chars().collect();
        if chars.is_empty() {
            names.push(name.into());
            pages.push(slint::SharedString::default());
            continue;
        }
        for chunk in chars.chunks(LEGAL_PAGE_CHUNK_SCALARS) {
            names.push(name.into());
            pages.push(chunk.iter().collect::<String>().into());
        }
    }
    cache
        .lock()
        .expect("native legal page cache lock")
        .insert(language_code, (names.clone(), pages.clone()));
    (names, pages)
}

fn native_legal_navigation(i18n: &I18n) -> (&'static str, &'static str, &'static str) {
    match i18n.language().code() {
        "ja" => ("戻る", "次へ", "ページ"),
        "zh-Hans" => ("返回", "下一页", "第"),
        "ko" => ("뒤로", "다음", "페이지"),
        "es" => ("Atrás", "Siguiente", "Página"),
        "fr" => ("Retour", "Suivant", "Page"),
        "de" => ("Zurück", "Weiter", "Seite"),
        "pt" => ("Voltar", "Próximo", "Página"),
        "it" => ("Indietro", "Avanti", "Pagina"),
        "ru" => ("Назад", "Далее", "Страница"),
        _ => ("Back", "Next", "Page"),
    }
}

fn ui_strings(i18n: &I18n) -> UiStrings {
    let (legal_page_names, legal_pages) = native_legal_pages(i18n);
    let (legal_back, legal_next, legal_page_position) = native_legal_navigation(i18n);
    UiStrings {
        font_family: i18n.text(TextKey::FontFamily).into(),
        product_version: format!("v{PRODUCT_VERSION}").into(),
        legal_page_names: slint::ModelRc::new(slint::VecModel::from(legal_page_names)),
        legal_pages: slint::ModelRc::new(slint::VecModel::from(legal_pages)),
        legal_back: legal_back.into(),
        legal_next: legal_next.into(),
        legal_page_position: legal_page_position.into(),
        usage_status: i18n.text(TextKey::UsageStatus).into(),
        graph: i18n.text(TextKey::Graph).into(),
        legal_notices: i18n.text(TextKey::LegalNotices).into(),
        running: i18n.text(TextKey::Running).into(),
        model_threads: i18n.text(TextKey::ModelThreads).into(),
        other: i18n.text(TextKey::Other).into(),
        details: i18n.text(TextKey::Details).into(),
        no_running_threads: i18n.text(TextKey::NoRunningThreads).into(),
        legal_code: i18n.text(TextKey::LegalCode).into(),
        legal_warranty: i18n.text(TextKey::LegalWarranty).into(),
        legal_license: i18n.text(TextKey::LegalLicense).into(),
        legal_font: i18n.text(TextKey::LegalFont).into(),
        legal_protocol: i18n.text(TextKey::LegalProtocol).into(),
        legal_schema: i18n.text(TextKey::LegalSchema).into(),
        legal_dependencies: i18n.text(TextKey::LegalDependencies).into(),
        legal_third_party: i18n.text(TextKey::LegalThirdParty).into(),
        legal_details: i18n.text(TextKey::LegalDetails).into(),
        legal_distribution: i18n.text(TextKey::LegalDistribution).into(),
        close: i18n.text(TextKey::Close).into(),
        active_threads: i18n.text(TextKey::ActiveThreads).into(),
        context_usage: i18n.text(TextKey::Context).into(),
        instruction: i18n.text(TextKey::Instruction).into(),
        tokens: i18n.text(TextKey::Tokens).into(),
        model: i18n.text(TextKey::Model).into(),
        input: i18n.text(TextKey::Input).into(),
        cached: i18n.text(TextKey::Cached).into(),
        output: i18n.text(TextKey::Output).into(),
        retry: i18n.text(TextKey::Retry).into(),
        usage_trend: i18n.text(TextKey::UsageTrend).into(),
        remaining: i18n.text(TextKey::Remaining).into(),
        graph_token_description: i18n.text(TextKey::GraphTokenDescription).into(),
        graph_dollar_description: i18n.text(TextKey::GraphDollarDescription).into(),
        no_records: i18n.text(TextKey::NoRecords).into(),
        connect_account: i18n.text(TextKey::ConnectAccount).into(),
        auth_browser_instructions: i18n.text(TextKey::AuthBrowserInstructions).into(),
        auth_managed: i18n.text(TextKey::AuthManaged).into(),
        open_auth_page: i18n.text(TextKey::OpenAuthPage).into(),
        start_auth: i18n.text(TextKey::StartAuth).into(),
        checking: i18n.text(TextKey::Checking).into(),
        check_auth: i18n.text(TextKey::CheckAuth).into(),
        auth_cli: i18n.text(TextKey::AuthCli).into(),
        no_history: i18n.text(TextKey::NoHistory).into(),
    }
}

#[cfg(test)]
fn normal_status_text(remaining: f64, seconds: i64, last_success_at: Option<&str>) -> String {
    let quota_notice = if remaining <= 2.0 {
        Some("残り利用枠はほぼありません。")
    } else if remaining <= 10.0 {
        Some("残り利用枠が少なくなっています。")
    } else {
        None
    };
    if let Some(notice) = quota_notice {
        notice.into()
    } else if seconds.abs() <= 86_400 {
        "リセット前後24時間です。".into()
    } else {
        format!("最終更新 {}", last_success_at.unwrap_or("—"))
    }
}

fn automatic_refresh_interval(authenticated: bool, auth_polling: bool) -> Duration {
    if !authenticated && auth_polling {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(60)
    }
}

/// Decide whether the account bridge may receive its next periodic read.
/// Keeping this predicate independent from the timer callback makes the
/// transient-worker-failure boundary testable: a failed worker clears
/// `checking`, and the next bounded interval is still admitted without an
/// immediate respawn loop.
fn account_refresh_due(
    now: Instant,
    last_poll: Instant,
    checking: bool,
    authenticated: bool,
    auth_polling: bool,
) -> bool {
    !checking
        && now.duration_since(last_poll) >= automatic_refresh_interval(authenticated, auth_polling)
}

/// The authenticated main surface is withheld until its first complete
/// usage generation is ready. A local-collector failure releases the spinner
/// so the error/retry state is visible instead of looking like a hang.
fn native_startup_loading(
    authenticated: bool,
    has_visible_usage: bool,
    local_usage_error: bool,
    account_error: bool,
    error: bool,
) -> bool {
    authenticated && !has_visible_usage && !local_usage_error && !account_error && !error
}

fn open_validated_auth_url(value: &str) -> bool {
    let Ok(url) = security::validate_auth_url(value) else {
        return false;
    };
    let executables = if let Some(path) = std::env::var_os("CODEX_INFO_BROWSER_BIN") {
        security::resolve_executable_path(Path::new(&path))
            .ok()
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        ["wslview", "xdg-open"]
            .into_iter()
            .filter_map(|name| resolved_executable("CODEX_INFO_BROWSER_BIN", name))
            .collect::<Vec<_>>()
    };
    for executable in executables {
        let child = Command::new(executable)
            .arg(url.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            thread::spawn(move || {
                let _ = child.wait();
            });
            return true;
        }
    }
    false
}

impl CodexInfoState {
    fn status_level(&self) -> &'static str {
        if self.error.is_some() {
            "error"
        } else if self.reset_at.is_some() && self.seconds_to_reset().abs() <= 86_400
            || (self.has_quota_percent && self.remaining_percent.unwrap_or(0.0) <= 10.0)
        {
            "warning"
        } else {
            "info"
        }
    }

    fn seconds_to_reset(&self) -> i64 {
        self.reset_at
            .and_then(|value| DateTime::from_timestamp(value, 0))
            .map(|time| (time - Utc::now()).num_seconds())
            .unwrap_or(0)
    }

    fn display_status(&self) -> String {
        if self.account_error.is_some() {
            return if self.last_success_at.is_some() {
                self.i18n.format_stale_status(self.last_success_at)
            } else {
                self.i18n.text(TextKey::CannotFetchUsage).into()
            };
        }
        match self.status.as_str() {
            "Codex app-serverへ接続しています…" => {
                self.i18n.text(TextKey::Connecting).into()
            }
            "認証状態を確認しています…" | "認証完了を確認しています…" => {
                self.i18n.text(TextKey::CheckingAuthStatus).into()
            }
            "認証済みです。利用量を取得しています…" => {
                self.i18n.text(TextKey::AuthenticatedLoading).into()
            }
            "未認証です。認証を開始してください。" => {
                self.i18n.text(TextKey::UnauthenticatedStart).into()
            }
            "認証URLを発行しました。「認証ページを開く」を押してください。" => {
                self.i18n.text(TextKey::AuthUrlIssued).into()
            }
            "認証URLを発行しています…" => {
                self.i18n.text(TextKey::IssuingAuthUrl).into()
            }
            "認証URLを開けませんでした。"
            | "認証URLを開けません。Codex CLIから認証を完了してください。" => {
                self.i18n.text(TextKey::AuthUrlOpenFailed).into()
            }
            "利用状況を更新しています…" => {
                self.i18n.text(TextKey::UpdatingUsage).into()
            }
            "利用状況を取得できません。Codex app-serverへの接続を確認してください。" => {
                self.i18n.text(TextKey::CannotFetchUsage).into()
            }
            "利用枠は更新しました。履歴とスレッド情報の取得に失敗し、実行中の状態は未確認です。" => {
                self.status.clone()
            }
            "利用枠は更新しました。履歴は前回値を保持しています。" => {
                self.i18n.text(TextKey::PartialHistory).into()
            }
            "利用枠は更新しました。スレッド情報の取得に失敗し、実行中の状態は未確認です。" => {
                self.status.clone()
            }
            "状態を表示できません。" => {
                self.i18n.text(TextKey::CannotDisplayStatus).into()
            }
            _ if self.status.is_empty() => self.i18n.text(TextKey::CannotDisplayStatus).into(),
            // `normal_status` is already formatted by the startup-pinned
            // catalog (for example, a localized last-updated clock). Keep
            // that value instead of replacing it with a generic Japanese
            // fallback. All asynchronous status keys above are canonical
            // internal values and are translated before reaching this arm.
            _ => self.status.clone(),
        }
    }

    fn open_auth(&mut self) {
        if self.service_endpoint_error.is_some() {
            return;
        }
        if let Some(url) = self.auth_url.clone() {
            let opened = open_validated_auth_url(&url);
            if !opened {
                self.apply_account_error(
                    "認証URLを開けません。Codex CLIから認証を完了してください。".into(),
                );
                self.status = "認証URLを開けませんでした。".into();
                self.auth_polling = false;
            } else {
                self.auth_polling = true;
                self.request_read("認証完了を確認しています…");
                self.last_poll = Instant::now();
            }
        } else {
            if !self.bridge.send(AccountCommand::Login) {
                self.bridge = AppServerBridge::<AccountCommand, Event>::start();
                if !self.bridge.send(AccountCommand::Login) {
                    self.apply_account_error(
                        "Codex app-serverへ認証要求を送信できませんでした。".into(),
                    );
                    return;
                }
            }
            self.checking = true;
            self.auth_polling = false;
            self.status = "認証URLを発行しています…".into();
        }
    }

    fn sync_ui(&self, ui: &MainWindow) {
        let remaining = self
            .remaining_percent
            .map(|remaining| remaining.clamp(0.0, 100.0))
            .unwrap_or(0.0);
        let seconds = self.seconds_to_reset();
        let period_seconds = self
            .reset_at
            .map(|reset_at| self.period_seconds_for_reset(reset_at))
            .unwrap_or(self.window_seconds.max(WEEK_SECONDS));
        ui.set_authenticated(self.authenticated);
        ui.set_strings(ui_strings(&self.i18n));
        ui.set_has_usage(self.has_visible_usage());
        ui.set_has_auth_url(self.auth_url.is_some());
        ui.set_checking(self.checking);
        ui.set_has_error(self.error.is_some());
        ui.set_startup_loading(native_startup_loading(
            self.authenticated,
            self.has_visible_usage(),
            self.local_usage_error,
            self.account_error.is_some(),
            self.error.is_some(),
        ));
        ui.set_window_title(native_account_window_title(&self.window_title()).into());
        let quota_title = if self.monthly {
            self.i18n.text(TextKey::MonthlyQuotaRemaining)
        } else if self.quota_title == "利用枠" {
            self.i18n.text(TextKey::UsageLimit)
        } else {
            self.i18n.text(TextKey::QuotaRemaining)
        };
        ui.set_quota_title(
            security::shorten_unicode(quota_title, security::MAX_LIMIT_NAME_SCALARS).into(),
        );
        ui.set_has_quota_percent(self.has_quota_percent);
        ui.set_remaining_label(
            if self.has_quota_percent {
                format_percent(remaining)
            } else {
                self.i18n.text(TextKey::FixedLimitNone).into()
            }
            .into(),
        );
        ui.set_week_label(if self.has_quota_percent {
            self.i18n
                .format_period_remaining(
                    seconds,
                    if self.monthly {
                        PeriodKind::Monthly
                    } else {
                        PeriodKind::Weekly
                    },
                )
                .into()
        } else {
            "".into()
        });
        let (
            model_names,
            input_tokens,
            input_costs,
            cached_tokens,
            cached_costs,
            output_tokens,
            output_costs,
        ) = format_model_usage_columns(&self.model_usage);
        ui.set_has_model_usage(!model_names.is_empty());
        ui.set_model_usage_names(model_names.into());
        ui.set_model_usage_input_tokens(input_tokens.into());
        ui.set_model_usage_input_costs(input_costs.into());
        ui.set_model_usage_cached_tokens(cached_tokens.into());
        ui.set_model_usage_cached_costs(cached_costs.into());
        ui.set_model_usage_output_tokens(output_tokens.into());
        ui.set_model_usage_output_costs(output_costs.into());
        ui.set_model_usage_period(self.model_usage_period().into());
        let estimate = if self.model_usage.is_empty() {
            format!("{} —", self.i18n.text(TextKey::EstimatePrefix))
        } else {
            let total = self
                .model_usage
                .iter()
                .map(ModelUsageRow::dollar_costs)
                .map(|(sol, terra, luna)| sol + terra + luna)
                .sum::<f64>();
            self.i18n.format_estimate(total)
        };
        ui.set_estimated_cost_label(estimate.into());
        ui.set_status(self.display_status().into());
        ui.set_status_level(self.status_level().into());
        ui.set_remaining_percent(remaining as f32);
        ui.set_remaining_days(if self.has_quota_percent {
            (seconds.max(0) as f32 / period_seconds.max(1) as f32 * 7.0).clamp(0.0, 7.0)
        } else {
            0.0
        });
        if !self.active_threads.is_empty() {
            ui.set_has_active_thread(true);
            ui.set_active_thread_count(
                i32::try_from(self.active_threads.len()).unwrap_or(i32::MAX),
            );
            ui.set_active_thread_count_label(
                self.i18n
                    .format_thread_count(self.active_threads.len())
                    .into(),
            );
            let [sol, terra, luna, other] = active_thread_model_count_values(&self.active_threads);
            ui.set_active_thread_sol_count(sol);
            ui.set_active_thread_terra_count(terra);
            ui.set_active_thread_luna_count(luna);
            ui.set_active_thread_other_count(other);
        } else {
            ui.set_has_active_thread(false);
            ui.set_active_thread_count(0);
            ui.set_active_thread_sol_count(0);
            ui.set_active_thread_terra_count(0);
            ui.set_active_thread_luna_count(0);
            ui.set_active_thread_other_count(0);
            ui.set_active_thread_count_label(self.i18n.format_thread_count(0).into());
        }
    }

    fn model_usage_period(&self) -> String {
        self.history_periods()
            .into_iter()
            .find(|period| {
                self.reset_at.is_some_and(|reset| {
                    period.canonical_reset_at.abs_diff(reset) <= RESET_AT_TOLERANCE_SECONDS as u64
                })
            })
            .map(|period| period.label)
            .unwrap_or_else(|| "履歴なし".into())
    }

    #[allow(dead_code)]
    fn graph_time_labels(&self) -> [String; 5] {
        self.graph_time_labels_at(Utc::now().timestamp())
    }

    fn graph_time_labels_at(&self, observed_at: i64) -> [String; 5] {
        let periods = self.history_periods_at(observed_at);
        let Some(reset_at) = self.selected_history_reset_for_periods(&periods) else {
            return Default::default();
        };
        let Some(period) = periods
            .iter()
            .find(|period| period.canonical_reset_at == reset_at)
        else {
            return Default::default();
        };
        let period_start = period.start;
        let period_end = period.end.max(period_start + 1);
        let span = (period_end - period_start).max(1) as f64;
        [0.0, 0.25, 0.5, 0.75, 1.0].map(|fraction| {
            let timestamp = period_start + (span * fraction) as i64;
            self.i18n.format_graph_time(timestamp).unwrap_or_default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::needless_return)]
fn format_period_timestamp(timestamp: i64) -> Option<String> {
    let time = DateTime::from_timestamp(timestamp, 0)?;
    Some(
        time.with_timezone(&chrono_tz::Asia::Tokyo)
            .format("%Y/%m/%d %H:%M:%S JST")
            .to_string(),
    )
}

#[cfg(test)]
fn format_period_label(start: i64, end: i64) -> String {
    let Some(start) = format_period_timestamp(start) else {
        return String::new();
    };
    let Some(end) = format_period_timestamp(end) else {
        return String::new();
    };
    format!("{start} ～ {end}")
}

impl Drop for CodexInfoState {
    fn drop(&mut self) {
        let _ = self.bridge.send(AccountCommand::Stop);
        self.stop_thread_bridge();
        let _ = self.local_bridge.send(LocalCommand::Stop);
    }
}

#[cfg(test)]
fn duration_parts(seconds: i64) -> (i64, i64, i64, i64) {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, seconds) = (rest / 60, rest % 60);
    (days, hours, minutes, seconds)
}

#[cfg(test)]
fn week_remaining_text(seconds: i64) -> String {
    let (days, hours, minutes, _) = duration_parts(seconds.max(0));
    if days > 0 {
        format!("7日中、あと{days}日と{hours}時間{minutes}分")
    } else if hours > 0 {
        format!("7日中、あと{hours}時間{minutes}分")
    } else {
        format!("7日中、あと{minutes}分")
    }
}

#[cfg(test)]
fn period_remaining_text(seconds: i64, period_seconds: i64, monthly: bool) -> String {
    if monthly {
        let (days, hours, minutes, _) = duration_parts(seconds.max(0));
        let duration = if days > 0 {
            format!("{days}日と{hours}時間{minutes}分")
        } else if hours > 0 {
            format!("{hours}時間{minutes}分")
        } else if minutes > 0 {
            format!("{minutes}分")
        } else {
            "まもなくリセット".into()
        };
        format!("月間、あと{duration}")
    } else {
        // Keep the established seven-day copy and avoid an unnatural “0日”.
        let _ = period_seconds;
        week_remaining_text(seconds)
    }
}

fn format_percent(value: f64) -> String {
    if value.fract().abs() < 0.0001 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

fn preview_model_row(
    name: &str,
    tokens: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
) -> ModelUsageRow {
    ModelUsageRow {
        name: name.into(),
        tokens,
        input_tokens,
        cached_input_tokens,
        output_tokens,
    }
}

fn format_model_usage_columns(
    rows: &[ModelUsageRow],
) -> (String, String, String, String, String, String, String) {
    let names = rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
    let input_tokens = rows
        .iter()
        .map(|row| format_token_count(row.input_tokens.saturating_sub(row.cached_input_tokens)))
        .collect::<Vec<_>>();
    let input_costs = rows
        .iter()
        .map(|row| format_dollar_cost(row.dollar_costs().0))
        .collect::<Vec<_>>();
    let cached_tokens = rows
        .iter()
        .map(|row| format_token_count(row.cached_input_tokens))
        .collect::<Vec<_>>();
    let cached_costs = rows
        .iter()
        .map(|row| format_dollar_cost(row.dollar_costs().1))
        .collect::<Vec<_>>();
    let output_tokens = rows
        .iter()
        .map(|row| format_token_count(row.output_tokens))
        .collect::<Vec<_>>();
    let output_costs = rows
        .iter()
        .map(|row| format_dollar_cost(row.dollar_costs().2))
        .collect::<Vec<_>>();
    (
        names.join("\n"),
        input_tokens.join("\n"),
        input_costs.join("\n"),
        cached_tokens.join("\n"),
        cached_costs.join("\n"),
        output_tokens.join("\n"),
        output_costs.join("\n"),
    )
}

fn format_dollar_cost(value: f64) -> String {
    format!("${}", value.max(0.0) as u64)
}

fn format_estimated_cost(costs: ModelDollarTotals) -> String {
    let total = [costs.sol, costs.terra, costs.luna]
        .into_iter()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .sum::<f64>();
    let total = if total.is_finite() && total >= 0.0 {
        total.min(u64::MAX as f64).round() as u64
    } else {
        0
    };
    format!("概算 ${}", format_token_count(total))
}

fn format_token_count(value: u64) -> String {
    format_unsigned_count(u128::from(value))
}

fn format_unsigned_count(value: u128) -> String {
    let mut reversed = String::new();
    for (index, character) in value.to_string().chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            reversed.push(',');
        }
        reversed.push(character);
    }
    reversed.chars().rev().collect()
}

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, KeyButMask,
    PropMode, StackMode, Window as X11Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct X11StateAtoms {
    wm_state: Atom,
    fullscreen: Atom,
    maximized_vert: Atom,
    maximized_horz: Atom,
    active_window: Option<Atom>,
}

struct X11WindowStateMonitor {
    connection: RustConnection,
    root: X11Window,
    atoms: X11StateAtoms,
    motif_wm_hints: Option<Atom>,
}

fn x11_window_id(window: &slint::Window) -> Option<X11Window> {
    let slint_handle = window.window_handle();
    let handle = <slint::WindowHandle as HasWindowHandle>::window_handle(&slint_handle)
        .ok()?
        .as_raw();
    match handle {
        RawWindowHandle::Xlib(handle) => {
            let window = handle.window as u32;
            (window != 0).then_some(window)
        }
        RawWindowHandle::Xcb(handle) => {
            let window = handle.window.get();
            (window != 0).then_some(window)
        }
        _ => None,
    }
}

const MOTIF_HINTS_FUNCTIONS: u32 = 1;
const MOTIF_FUNCTION_ALL: u32 = 1;
const MOTIF_FUNCTION_RESIZE: u32 = 1 << 1;
const MOTIF_FUNCTION_MOVE: u32 = 1 << 2;
const MOTIF_FUNCTION_MINIMIZE: u32 = 1 << 3;
const MOTIF_FUNCTION_MAXIMIZE: u32 = 1 << 4;
const MOTIF_FUNCTION_CLOSE: u32 = 1 << 5;

fn motif_wm_functions(existing_flags: u32) -> (u32, u32) {
    (
        existing_flags | MOTIF_HINTS_FUNCTIONS,
        MOTIF_FUNCTION_MOVE | MOTIF_FUNCTION_MINIMIZE | MOTIF_FUNCTION_CLOSE,
    )
}

fn motif_wm_resizable_functions(existing_flags: u32, existing_functions: u32) -> (u32, u32) {
    let functions = if existing_functions & MOTIF_FUNCTION_ALL == 0 {
        existing_functions
            | MOTIF_FUNCTION_RESIZE
            | MOTIF_FUNCTION_MOVE
            | MOTIF_FUNCTION_MINIMIZE
            | MOTIF_FUNCTION_MAXIMIZE
            | MOTIF_FUNCTION_CLOSE
    } else {
        existing_functions
    };
    (existing_flags | MOTIF_HINTS_FUNCTIONS, functions)
}

fn forbidden_x11_states(states: &[Atom], atoms: &X11StateAtoms) -> (bool, bool) {
    (
        states.contains(&atoms.fullscreen),
        states
            .iter()
            .any(|state| *state == atoms.maximized_vert || *state == atoms.maximized_horz),
    )
}

impl X11WindowStateMonitor {
    fn intern_atom(connection: &RustConnection, name: &[u8]) -> Option<Atom> {
        connection
            .intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|reply| reply.atom)
    }

    fn connect() -> Option<Self> {
        let (connection, screen_num) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen_num)?.root;
        let atoms = X11StateAtoms {
            wm_state: Self::intern_atom(&connection, b"_NET_WM_STATE")?,
            fullscreen: Self::intern_atom(&connection, b"_NET_WM_STATE_FULLSCREEN")?,
            maximized_vert: Self::intern_atom(&connection, b"_NET_WM_STATE_MAXIMIZED_VERT")?,
            maximized_horz: Self::intern_atom(&connection, b"_NET_WM_STATE_MAXIMIZED_HORZ")?,
            active_window: Self::intern_atom(&connection, b"_NET_ACTIVE_WINDOW"),
        };
        let motif_wm_hints = Self::intern_atom(&connection, b"_MOTIF_WM_HINTS");
        Some(Self {
            connection,
            root,
            atoms,
            motif_wm_hints,
        })
    }

    fn remove_state(&self, window: X11Window, first: Atom, second: Atom) {
        let event =
            ClientMessageEvent::new(32, window, self.atoms.wm_state, [0, first, second, 1, 0]);
        let event_mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        if self
            .connection
            .send_event(false, self.root, event_mask, event)
            .is_ok()
        {
            let _ = self.connection.flush();
        }
    }

    fn enforce_motif_functions(&self, window: X11Window, motif_wm_hints: Atom) {
        let Ok(cookie) =
            self.connection
                .get_property(false, window, motif_wm_hints, AtomEnum::ANY, 0, 5)
        else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };

        let mut hints = [0_u32; 5];
        match reply.format {
            0 => {}
            32 => {
                let Some(values) = reply.value32() else {
                    return;
                };
                for (hint, value) in hints.iter_mut().zip(values) {
                    *hint = value;
                }
            }
            _ => return,
        }

        let (flags, functions) = motif_wm_functions(hints[0]);
        if hints[0] == flags && hints[1] == functions {
            return;
        }
        hints[0] = flags;
        hints[1] = functions;
        if self
            .connection
            .change_property32(
                PropMode::REPLACE,
                window,
                motif_wm_hints,
                motif_wm_hints,
                &hints,
            )
            .is_ok()
        {
            let _ = self.connection.flush();
        }
    }

    fn allow_resize(&self, window: &slint::Window) {
        let Some(window_id) = x11_window_id(window) else {
            return;
        };
        let Some(motif_wm_hints) = self.motif_wm_hints else {
            return;
        };
        let Ok(cookie) =
            self.connection
                .get_property(false, window_id, motif_wm_hints, AtomEnum::ANY, 0, 5)
        else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        let Some(values) = reply.value32() else {
            return;
        };
        let mut hints = [0_u32; 5];
        for (hint, value) in hints.iter_mut().zip(values) {
            *hint = value;
        }
        (hints[0], hints[1]) = motif_wm_resizable_functions(hints[0], hints[1]);
        if self
            .connection
            .change_property32(
                PropMode::REPLACE,
                window_id,
                motif_wm_hints,
                motif_wm_hints,
                &hints,
            )
            .is_ok()
        {
            let _ = self.connection.flush();
        }
    }

    fn enforce(&self, window: &slint::Window) {
        let Some(window_id) = x11_window_id(window) else {
            return;
        };
        if let Some(motif_wm_hints) = self.motif_wm_hints {
            self.enforce_motif_functions(window_id, motif_wm_hints);
        }
        let Ok(cookie) = self.connection.get_property(
            false,
            window_id,
            self.atoms.wm_state,
            AtomEnum::ATOM,
            0,
            u32::MAX,
        ) else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        let Some(states) = reply.value32() else {
            return;
        };
        let states: Vec<Atom> = states.collect();
        let (fullscreen, maximized) = forbidden_x11_states(&states, &self.atoms);
        if fullscreen {
            self.remove_state(window_id, self.atoms.fullscreen, 0);
        }
        if maximized {
            self.remove_state(
                window_id,
                self.atoms.maximized_vert,
                self.atoms.maximized_horz,
            );
        }
    }

    fn raise_and_activate(&self, window: &slint::Window) {
        let Some(window_id) = x11_window_id(window) else {
            return;
        };
        // Xwayland/WSLg can place the client inside a compositor-owned
        // wrapper. Raising only the Slint client leaves the main window above
        // a frameless secondary surface, so raise the wrapper as well.
        let raise_target = self
            .connection
            .query_tree(window_id)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.parent)
            .filter(|parent| *parent != self.root)
            .unwrap_or(window_id);
        let stack_mode = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        let _ = self.connection.configure_window(raise_target, &stack_mode);
        if raise_target != window_id {
            let _ = self.connection.configure_window(window_id, &stack_mode);
        }

        if let Some(active_window) = self.atoms.active_window {
            let event_mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
            let event = ClientMessageEvent::new(
                32,
                window_id,
                active_window,
                [1, x11rb::CURRENT_TIME, 0, 0, 0],
            );
            let _ = self
                .connection
                .send_event(false, self.root, event_mask, event);
        }
        let _ = self.connection.flush();
    }
}

/// Parses the visual-review size override without applying window-specific
/// bounds. Invalid values leave the Slint defaults untouched; the graph
/// preview applies its minimum dimensions after parsing.
fn parse_preview_size(value: Option<&str>) -> Option<(u32, u32)> {
    let value = value?.trim();
    let (width, height) = value.split_once('x')?;
    if width.is_empty() || height.is_empty() || height.contains('x') {
        return None;
    }
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    Some((width, height))
}

fn clamp_graph_preview_size((width, height): (u32, u32)) -> (u32, u32) {
    (width.max(700), height.max(480))
}

const DEFAULT_SERVICE_ADDRESS: &str = "127.0.0.1:8787";
const BACKGROUND_SERVICE_START_TIMEOUT: Duration = Duration::from_secs(5);
const BACKGROUND_CHILD_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

fn cli_error(key: CliTextKey) -> String {
    I18n::detect().cli_text(key).to_owned()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaunchMode {
    /// Mode 1: one resident owner containing recorder + REST.
    Service(ApiServerConfig),
    /// Mode 2: ensure a resident service exists at this address, then add X UI.
    All(ApiServerConfig),
    /// Stop this profile's verified resident service and wait for its lock to disappear.
    Stop,
    /// Print CLI usage without starting a daemon or UI.
    Help,
}

fn default_service_config() -> Result<ApiServerConfig, String> {
    DEFAULT_SERVICE_ADDRESS
        .parse::<SocketAddr>()
        .map_err(|_| cli_error(CliTextKey::ServiceStartFailed))
        .and_then(|address| {
            ApiServerConfig::new(address).map_err(|_| cli_error(CliTextKey::ServiceStartFailed))
        })
}

fn service_config_for_port(value: &std::ffi::OsStr) -> Result<ApiServerConfig, String> {
    let port = value
        .to_str()
        .ok_or_else(|| cli_error(CliTextKey::InvalidPort))?
        .parse::<u16>()
        .map_err(|_| cli_error(CliTextKey::InvalidPort))?;
    if port == 0 {
        return Err(cli_error(CliTextKey::InvalidPort));
    }
    ApiServerConfig::new(SocketAddr::from(([127, 0, 0, 1], port)))
        .map_err(|_| cli_error(CliTextKey::ServiceStartFailed))
}

fn parse_launch_mode<I>(arguments: I) -> Result<LaunchMode, String>
where
    I: IntoIterator<Item = OsString>,
{
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => default_service_config().map(LaunchMode::Service),
        [value] if value == "--help" || value == "--h" || value == "-h" => Ok(LaunchMode::Help),
        [value] if value == "--ui" => default_service_config().map(LaunchMode::All),
        [stop] if stop == "--stop" => Ok(LaunchMode::Stop),
        [port, value] if port == "--port" => {
            service_config_for_port(value).map(LaunchMode::Service)
        }
        [ui, port, value] if ui == "--ui" && port == "--port" => {
            service_config_for_port(value).map(LaunchMode::All)
        }
        _ => Err(I18n::detect().language().launch_help().to_owned()),
    }
}

fn is_service_health_response(response: &[u8]) -> bool {
    if !response.starts_with(b"HTTP/1.1 200 ") {
        return false;
    }
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let body = &response[header_end + 4..];
    serde_json::from_slice::<Value>(body).is_ok_and(|value| {
        value.get("api_version").and_then(Value::as_str) == Some("v1")
            && value.get("service").and_then(Value::as_str) == Some("codex-info")
    })
}

fn service_is_healthy(address: SocketAddr) -> bool {
    let timeout = Duration::from_millis(150);
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    let request =
        format!("GET /v1/health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream.write_all(request.as_bytes()).is_err()
    {
        return false;
    }
    let mut response = Vec::with_capacity(512);
    let mut buffer = [0_u8; 512];
    while response.len() < 4096 {
        let remaining = 4096 - response.len();
        let read_limit = remaining.min(buffer.len());
        match stream.read(&mut buffer[..read_limit]) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                // The server supplies Content-Length and may keep the socket
                // open. A complete, validated body is success; EOF is not a
                // health requirement.
                if is_service_health_response(&response) {
                    return true;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => return false,
        }
    }
    is_service_health_response(&response)
}

fn healthy_combined_service_owner(address: SocketAddr) -> Option<u32> {
    service_is_healthy(address)
        .then(daemon::current_daemon_owner_pid)
        .flatten()
}

fn terminate_and_reap_owned_child(child: &mut Child) -> bool {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return true;
    }
    // The child is ours, but still pin its process instance before requesting
    // termination. A forceful Child::kill would bypass the pidfd contract used
    // by the public --stop path.
    if !daemon::send_term_to_owned_process(child.id()) {
        return false;
    }
    let deadline = Instant::now() + BACKGROUND_CHILD_CLEANUP_TIMEOUT;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn ensure_background_service(config: ApiServerConfig) -> Result<(), String> {
    let address = config.listen_addr();
    if healthy_combined_service_owner(address).is_some() {
        return Ok(());
    }
    let executable =
        std::env::current_exe().map_err(|_| cli_error(CliTextKey::ServiceExecutableUnavailable))?;
    let port_text = address.port().to_string();
    let child = Command::new(executable)
        .args(["--port", port_text.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| cli_error(CliTextKey::ServiceStartFailed))?;
    let child_pid = child.id();
    let mut owned_child = Some(child);
    let deadline = Instant::now() + BACKGROUND_SERVICE_START_TIMEOUT;
    loop {
        let healthy_owner = healthy_combined_service_owner(address);
        if healthy_owner == Some(child_pid) {
            // This is the resident child this UI+service invocation intentionally
            // created. Dropping the process handle detaches it; it must remain
            // alive after the X UI closes.
            return Ok(());
        }
        if healthy_owner.is_some() {
            // A concurrent UI/service launcher won recorder ownership and became
            // healthy. This invocation must reap only the child it spawned
            // before attaching its UI to that winner.
            if let Some(child) = owned_child.as_mut() {
                if !terminate_and_reap_owned_child(child) {
                    return Err(cli_error(CliTextKey::ServiceCleanupFailed));
                }
            }
            return Ok(());
        }

        if let Some(child) = owned_child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => owned_child = None,
                Ok(None) => {}
                Err(_) => {
                    let reaped = terminate_and_reap_owned_child(child);
                    return Err(if reaped {
                        cli_error(CliTextKey::ServiceStateUnavailable)
                    } else {
                        cli_error(CliTextKey::ServiceCleanupFailed)
                    });
                }
            }
        }
        if owned_child.is_none() && daemon::current_daemon_owner_pid().is_none() {
            return Err(cli_error(CliTextKey::ServiceExitedBeforeHealthy));
        }
        if Instant::now() >= deadline {
            let reaped = owned_child
                .as_mut()
                .is_none_or(terminate_and_reap_owned_child);
            return Err(if reaped {
                cli_error(CliTextKey::ServiceNotHealthy)
            } else {
                cli_error(CliTextKey::ServiceCleanupFailed)
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn poll_service_state(state: &mut CodexInfoState, service_endpoint: SocketAddr) {
    if !service_is_healthy(service_endpoint) {
        let error = state
            .service_endpoint_error
            .clone()
            .unwrap_or_else(|| cli_error(CliTextKey::ServiceStateUnavailable));
        state.hold_service_endpoint_error(error);
        return;
    }
    state.clear_service_endpoint_error();
    state.poll();
    if account_refresh_due(
        Instant::now(),
        state.last_poll,
        state.checking,
        state.authenticated,
        state.auth_polling,
    ) {
        let status = if state.auth_polling && !state.authenticated {
            "認証完了を確認しています…"
        } else {
            "利用状況を更新しています…"
        };
        state.request_read(status);
        state.last_poll = Instant::now();
    }
    if state.authenticated
        && !state.thread_checking
        && state.last_thread_poll.elapsed() >= Duration::from_secs(5)
    {
        state.request_thread_update();
    }
}

async fn service_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let ctrl_c = tokio::signal::ctrl_c();
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = ctrl_c => {}
                _ = terminate.recv() => {}
            }
        } else {
            let _ = ctrl_c.await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn run_combined_service(config: ApiServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut recorder = daemon::RecorderWorker::start()
        .map_err(|_| std::io::Error::other(cli_error(CliTextKey::ServiceStartFailed)))?;
    if !recorder.is_active() {
        recorder.shutdown();
        return Err(std::io::Error::other(cli_error(CliTextKey::ServiceAlreadyOwned)).into());
    }
    // Bind REST only after this process owns the recorder. Concurrent service
    // children therefore exit before publishing a listener, and an API bind
    // failure drops the worker and releases its exact lock identity.
    let mut api_server = ApiServer::start(config)
        .map_err(|_| std::io::Error::other(cli_error(CliTextKey::ServiceStartFailed)))?;
    let publisher = api_server.publisher();
    let mut state = CodexInfoState::new();
    publisher.publish_details(state.public_details())?;
    let mut last_publish_error = None;
    eprintln!(
        "codex-info: daemon+REST listening on {} recorder_owner={}",
        api_server.local_addr(),
        recorder.is_active()
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let shutdown = service_shutdown_signal();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = ticker.tick() => {
                    poll_service_state(&mut state, config.listen_addr());
                    match publisher.publish_details(state.public_details()) {
                        Ok(()) => {
                            if last_publish_error.take().is_some() {
                                eprintln!("codex-info: REST snapshot publication recovered");
                            }
                        }
                        Err(error) => {
                            if last_publish_error != Some(error) {
                                eprintln!("codex-info: REST snapshot publication rejected: {error}");
                                last_publish_error = Some(error);
                            }
                        }
                    }
                }
            }
        }
    });
    recorder.shutdown();
    api_server.shutdown();
    Ok(())
}

fn run_service_mode(config: ApiServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    if healthy_combined_service_owner(config.listen_addr()).is_some() {
        eprintln!(
            "codex-info: {}",
            I18n::detect().cli_text(CliTextKey::ServiceReused)
        );
        return Ok(());
    }
    run_combined_service(config)
}

fn stop_service_mode() -> Result<(), Box<dyn std::error::Error>> {
    daemon::stop_daemon().map_err(|error| {
        let key = match error {
            daemon::StopError::LockUnavailable => CliTextKey::StopLockUnavailable,
            daemon::StopError::LockInvalid => CliTextKey::StopLockInvalid,
            daemon::StopError::OwnerChanged => CliTextKey::StopOwnerChanged,
            daemon::StopError::SignalFailed => CliTextKey::StopSignalFailed,
            daemon::StopError::Timeout => CliTextKey::StopTimeout,
            daemon::StopError::Unsupported => CliTextKey::StopUnsupported,
        };
        std::io::Error::other(cli_error(key)).into()
    })
}

fn run_ui(
    initial_service_error: Option<String>,
    service_config: ApiServerConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let ui = MainWindow::new()?;
    install_fixed_window_guard(ui.window());
    place_main_window_on_primary_monitor(ui.window());
    let preview_size = std::env::var("CODEX_INFO_PREVIEW_SIZE")
        .ok()
        .and_then(|value| parse_preview_size(Some(value.as_str())));
    let graph_preview_size = preview_size.map(clamp_graph_preview_size);
    let preview_kind = std::env::var("CODEX_INFO_PREVIEW").ok();
    let state = Rc::new(RefCell::new(
        preview_kind
            .clone()
            .map(|kind| CodexInfoState::preview(&kind))
            .unwrap_or_else(CodexInfoState::new),
    ));
    if let Some(error) = initial_service_error {
        // A failed --ui service must not make the GUI disappear. Publish a
        // visible retry/error state and keep the window available for recovery.
        state.borrow_mut().hold_service_endpoint_error(error);
    }
    // One graph window owns the three model toggles. The initial state keeps
    // every series enabled, preserving the combined cumulative view.
    let graph_window = Rc::new(RefCell::new(None::<GraphWindow>));
    let graph_maximize_state = Rc::new(RefCell::new(GraphMaximizeState::default()));
    let threads_window = Rc::new(RefCell::new(None::<ThreadsWindow>));
    let legal_notice_window = Rc::new(RefCell::new(None::<LegalNoticeWindow>));
    let x11_monitor = Rc::new(X11WindowStateMonitor::connect());

    {
        let weak_ui = ui.as_weak();
        ui.on_begin_window_drag(move || {
            if let Some(ui) = weak_ui.upgrade() {
                begin_window_drag(ui.window());
            }
        });
    }
    {
        let weak_ui = ui.as_weak();
        ui.on_minimize_window(move || {
            if let Some(ui) = weak_ui.upgrade() {
                minimize_window(ui.window());
            }
        });
    }
    {
        let weak_ui = ui.as_weak();
        ui.on_close_window(move || {
            if let Some(ui) = weak_ui.upgrade() {
                let _ = ui.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }

    {
        let state = Rc::clone(&state);
        ui.on_begin_auth(move || state.borrow_mut().open_auth());
    }
    {
        let state = Rc::clone(&state);
        ui.on_check_auth(move || state.borrow_mut().request_read("認証状態を確認しています…"));
    }
    {
        let state = Rc::clone(&state);
        ui.on_retry(move || state.borrow_mut().request_read("利用状況を更新しています…"));
    }
    {
        let state = Rc::clone(&state);
        let graph_window = Rc::clone(&graph_window);
        let graph_maximize_state = Rc::clone(&graph_maximize_state);
        let x11_monitor = Rc::clone(&x11_monitor);
        let graph_old_preview = preview_kind.as_deref() == Some("graph-old");
        let graph_period_preview =
            matches!(preview_kind.as_deref(), Some("graph-period" | "graph-many"));
        ui.on_open_graph(move || {
            if !graph_old_preview {
                state.borrow_mut().select_latest_history();
            }
            let mut graph_window = graph_window.borrow_mut();
            if graph_window.is_none() {
                if let Ok(graph) = GraphWindow::new() {
                    graph.set_open_history_on_start(graph_period_preview);
                    let (graph_width, graph_height) =
                        graph_preview_size.unwrap_or((GRAPH_WINDOW_WIDTH, GRAPH_WINDOW_HEIGHT));
                    if graph_preview_size.is_some() {
                        graph.window().set_size(slint::LogicalSize::new(
                            graph_width as f32,
                            graph_height as f32,
                        ));
                    }
                    install_resizable_window(graph.window());
                    let weak_graph = graph.as_weak();
                    graph.on_begin_window_drag(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            begin_window_drag(graph.window());
                        }
                    });
                    let weak_graph = graph.as_weak();
                    graph.on_begin_window_resize(move |direction| {
                        if let Some(graph) = weak_graph.upgrade() {
                            begin_window_resize(graph.window(), direction.as_str());
                        }
                    });
                    let weak_graph = graph.as_weak();
                    graph.on_minimize_window(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            minimize_window(graph.window());
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let graph_maximize_state = Rc::clone(&graph_maximize_state);
                    graph.on_toggle_maximize_window(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            toggle_graph_maximize(graph.window(), &graph, &graph_maximize_state);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    graph.on_close_graph(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_reset_close_buttons(true);
                            graph.set_reset_close_buttons(false);
                            let _ = graph.hide();
                        }
                    });
                    let weak_graph = graph.as_weak();
                    graph.on_close_window(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_reset_close_buttons(true);
                            graph.set_reset_close_buttons(false);
                            let _ = graph.hide();
                        }
                    });
                    let weak_graph = graph.as_weak();
                    graph.window().on_close_requested(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_reset_close_buttons(true);
                            graph.set_reset_close_buttons(false);
                            if graph.hide().is_ok() {
                                return CloseRequestResponse::KeepWindowShown;
                            }
                        }
                        CloseRequestResponse::HideWindow
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_toggle = Rc::clone(&state);
                    graph.on_toggle_remaining(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_show_remaining(!graph.get_show_remaining());
                            sync_graph_window(&state_for_toggle.borrow(), &graph);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_metric = Rc::clone(&state);
                    graph.on_select_metric(move |metric| {
                        if let Some(graph) = weak_graph.upgrade() {
                            state_for_metric.borrow_mut().select_metric(&metric);
                            sync_graph_window(&state_for_metric.borrow(), &graph);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_toggle = Rc::clone(&state);
                    graph.on_toggle_luna(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_show_luna(!graph.get_show_luna());
                            sync_graph_window(&state_for_toggle.borrow(), &graph);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_toggle = Rc::clone(&state);
                    graph.on_toggle_terra(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_show_terra(!graph.get_show_terra());
                            sync_graph_window(&state_for_toggle.borrow(), &graph);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_toggle = Rc::clone(&state);
                    graph.on_toggle_sol(move || {
                        if let Some(graph) = weak_graph.upgrade() {
                            graph.set_show_sol(!graph.get_show_sol());
                            sync_graph_window(&state_for_toggle.borrow(), &graph);
                        }
                    });
                    let weak_graph = graph.as_weak();
                    let state_for_history = Rc::clone(&state);
                    graph.on_select_history(move |label| {
                        if let Some(graph) = weak_graph.upgrade() {
                            state_for_history
                                .borrow_mut()
                                .select_history(label.as_str());
                            sync_graph_window(&state_for_history.borrow(), &graph);
                        }
                    });
                    *graph_window = Some(graph);
                }
            }
            if let Some(graph) = graph_window.as_ref() {
                graph.set_reset_close_buttons(true);
                graph.set_reset_close_buttons(false);
                sync_graph_window(&state.borrow(), graph);
                let _ = show_and_focus_window(graph.window(), x11_monitor.as_ref().as_ref());
                if let Some(monitor) = x11_monitor.as_ref() {
                    monitor.allow_resize(graph.window());
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        let threads_window = Rc::clone(&threads_window);
        let x11_monitor = Rc::clone(&x11_monitor);
        ui.on_open_threads(move || {
            let mut threads_window = threads_window.borrow_mut();
            if threads_window.is_none() {
                if let Ok(window) = ThreadsWindow::new() {
                    install_fixed_window_guard(window.window());
                    let weak_window = window.as_weak();
                    window.on_begin_window_drag(move || {
                        if let Some(window) = weak_window.upgrade() {
                            begin_window_drag(window.window());
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_minimize_window(move || {
                        if let Some(window) = weak_window.upgrade() {
                            minimize_window(window.window());
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_close_threads(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            let _ = window.hide();
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_close_window(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            let _ = window.hide();
                        }
                    });
                    let weak_window = window.as_weak();
                    window.window().on_close_requested(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            if window.hide().is_ok() {
                                return CloseRequestResponse::KeepWindowShown;
                            }
                        }
                        CloseRequestResponse::HideWindow
                    });
                    *threads_window = Some(window);
                }
            }
            if let Some(window) = threads_window.as_ref() {
                window.set_reset_close_buttons(true);
                window.set_reset_close_buttons(false);
                sync_threads_window(&state.borrow(), window);
                let _ = show_and_focus_window(window.window(), x11_monitor.as_ref().as_ref());
            }
        });
    }
    {
        let state = Rc::clone(&state);
        let legal_notice_window = Rc::clone(&legal_notice_window);
        let x11_monitor = Rc::clone(&x11_monitor);
        ui.on_open_legal_notice(move || {
            let mut legal_notice_window = legal_notice_window.borrow_mut();
            if legal_notice_window.is_none() {
                if let Ok(window) = LegalNoticeWindow::new() {
                    install_window_size_guard(
                        window.window(),
                        LEGAL_WINDOW_WIDTH,
                        LEGAL_WINDOW_HEIGHT,
                    );
                    let weak_window = window.as_weak();
                    window.on_begin_window_drag(move || {
                        if let Some(window) = weak_window.upgrade() {
                            begin_window_drag(window.window());
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_minimize_window(move || {
                        if let Some(window) = weak_window.upgrade() {
                            minimize_window(window.window());
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_close_legal_notice(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            let _ = window.hide();
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_close_window(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            let _ = window.hide();
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_legal_page_back(move || {
                        if let Some(window) = weak_window.upgrade() {
                            let index = window.get_legal_page_index();
                            window.set_legal_page_index(index.saturating_sub(1));
                        }
                    });
                    let weak_window = window.as_weak();
                    window.on_legal_page_next(move || {
                        if let Some(window) = weak_window.upgrade() {
                            let index = window.get_legal_page_index();
                            let page_count = window.get_strings().legal_pages.row_count();
                            if index + 1 < i32::try_from(page_count).unwrap_or(i32::MAX) {
                                window.set_legal_page_index(index.saturating_add(1));
                            }
                        }
                    });
                    let weak_window = window.as_weak();
                    window.window().on_close_requested(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_reset_close_buttons(true);
                            window.set_reset_close_buttons(false);
                            if window.hide().is_ok() {
                                return CloseRequestResponse::KeepWindowShown;
                            }
                        }
                        CloseRequestResponse::HideWindow
                    });
                    *legal_notice_window = Some(window);
                }
            }
            if let Some(window) = legal_notice_window.as_ref() {
                let state_ref = state.borrow();
                window.set_strings(ui_strings(&state_ref.i18n));
                window.set_window_title(
                    native_detail_window_title(
                        &state_ref.i18n,
                        state_ref.authenticated,
                        &state_ref.window_title(),
                        WindowPurpose::Legal,
                    )
                    .into(),
                );
                window.set_legal_page_index(0);
                window.set_reset_close_buttons(true);
                window.set_reset_close_buttons(false);
                let _ = show_and_focus_window(window.window(), x11_monitor.as_ref().as_ref());
            }
        });
    }

    if matches!(
        preview_kind.as_deref(),
        Some("graph" | "graph-old" | "graph-many" | "graph-period" | "graph-collision")
    ) {
        if preview_kind.as_deref() == Some("graph-old") {
            state.borrow_mut().select_latest_history();
            state.borrow_mut().select_older_history();
        }
        ui.invoke_open_graph();
    }
    if matches!(
        preview_kind.as_deref(),
        Some("multi-thread" | "single-thread")
    ) {
        ui.invoke_open_threads();
    }
    if preview_kind.as_deref() == Some("legal") {
        ui.invoke_open_legal_notice();
    }

    state.borrow().sync_ui(&ui);
    let weak_ui_for_bounds = ui.as_weak();
    let monitor = Rc::clone(&x11_monitor);
    let graph_window_for_resize = Rc::clone(&graph_window);
    let threads_window_for_bounds = Rc::clone(&threads_window);
    let legal_notice_window_for_bounds = Rc::clone(&legal_notice_window);
    let main_monitor_timer = Timer::default();
    main_monitor_timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        if let Some(ui) = weak_ui_for_bounds.upgrade() {
            if let Some(monitor) = monitor.as_ref() {
                monitor.enforce(ui.window());
                if let Some(graph) = graph_window_for_resize.borrow().as_ref() {
                    if graph.window().is_visible() {
                        monitor.allow_resize(graph.window());
                    }
                }
                if let Some(window) = threads_window_for_bounds.borrow().as_ref() {
                    if window.window().is_visible() {
                        monitor.enforce(window.window());
                    }
                }
                if let Some(window) = legal_notice_window_for_bounds.borrow().as_ref() {
                    if window.window().is_visible() {
                        monitor.enforce(window.window());
                    }
                }
            }
        }
    });
    let weak_ui_for_position = ui.as_weak();
    let main_window_position_timer = Timer::default();
    main_window_position_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(100),
        move || {
            if let Some(ui) = weak_ui_for_position.upgrade() {
                place_main_window_on_primary_monitor(ui.window());
            }
        },
    );
    let weak_ui = ui.as_weak();
    let graph_window_for_timer = Rc::clone(&graph_window);
    let threads_window_for_timer = Rc::clone(&threads_window);
    let timer = Timer::default();
    if !state.borrow().preview {
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            if let Some(ui) = weak_ui.upgrade() {
                let mut state = state.borrow_mut();
                poll_service_state(&mut state, service_config.listen_addr());
                state.sync_ui(&ui);
                if let Some(graph) = graph_window_for_timer.borrow().as_ref() {
                    if graph.window().is_visible() {
                        sync_graph_window(&state, graph);
                    }
                }
                if let Some(window) = threads_window_for_timer.borrow().as_ref() {
                    if window.window().is_visible() {
                        sync_threads_window(&state, window);
                    }
                }
            }
        });
    }
    ui.run()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mode = parse_launch_mode(arguments).map_err(std::io::Error::other)?;
    match mode {
        LaunchMode::Service(config) => run_service_mode(config),
        LaunchMode::Stop => stop_service_mode(),
        LaunchMode::All(config) => {
            let startup_error = ensure_background_service(config).err();
            run_ui(startup_error, config)
        }
        LaunchMode::Help => {
            println!("{}", I18n::detect().language().launch_help());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::winit;
    use super::{
        account_refresh_due, account_window_title, active_thread_model_counts,
        active_thread_rows_at, add_recovery_usage, automatic_refresh_interval,
        clamp_graph_preview_size, collapse_remaining_change_points, collect_session_file,
        complete_rollout_prefix_len, current_history_period_reset, current_label_connector_path,
        detail_window_title, fetch_active_thread_update_for_paths,
        fetch_active_thread_update_for_paths_and_state, fixed_resize_decision,
        fixed_resize_decision_for_scale, format_elapsed, format_estimated_cost,
        format_model_usage_columns, format_percent, format_period_label, graph_paths,
        graph_paths_for_selection, graph_points, graph_time_endpoints, is_service_health_response,
        minute_model_spend, minute_model_spend_for_metric, model_usage_timeline_from_events,
        monthly_window_seconds, native_account_window_title, native_legal_pages,
        native_startup_loading, normal_status_text, one_month_before_utc, open_codex_session_paths,
        parse_launch_mode, parse_preview_size, parse_rate_limits, parse_resize_direction,
        period_remaining_text, physical_size_for_logical, plan_type_label, poll_service_state,
        preview_model_row, read_recovery_entries, read_thread_rollout_path, recovery_timed_usage,
        remaining_graph_points, remaining_graph_points_for_metric, remaining_graph_y,
        remaining_marker_positions, remaining_marker_positions_on_points, request_with_timeout,
        reset_transition_is_boundary, same_rollout_identity, separate_current_label_positions,
        service_is_healthy, session_event_model, session_event_type, session_jsonl_files,
        session_token_snapshot, smooth_model_spend, smooth_remaining_points,
        split_metric_line_paths, stacked_area_path, terminate_and_reap_owned_child,
        thread_presentation_rows, three_months_before_utc, unused_interval_positions,
        visible_window_position, week_remaining_text, ActiveThread, ActiveThreadUpdate, ApiServer,
        ApiServerConfig, CodexInfoState, Event, FixedResizeDecision, GraphPaths, GraphWindow,
        HourlyModelSpend, I18n, LaunchMode, LocalUsageResult, ManualX11Geometry,
        ManualX11WindowAction, ModelDollarTotals, ModelTokenTotals, ModelUsageRow,
        ModelUsageTotals, RpcReadEvent, SessionTraversalBudget, TokenSnapshot,
        UnusedIntervalPosition, UsageEvent, UsageHistory, UsageHistorySample, UsageStore,
        DEFAULT_SERVICE_ADDRESS, FIXED_WINDOW_HEIGHT, FIXED_WINDOW_WIDTH, GRAPH_METRIC_OPTIONS,
        GRAPH_WINDOW_PURPOSE, LOCAL_ESTIMATE_PRICE_VERSION, PRODUCT_VERSION,
        THREADS_WINDOW_PURPOSE, UNAUTHENTICATED_WINDOW_TITLE, WEEK_SECONDS,
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GraphFixture {
        expected_period_start: i64,
        expected_period_end: i64,
        expected_reset_at: i64,
        expected_raw_timestamps: Vec<i64>,
        #[serde(default)]
        expected_retained_timestamps: Vec<i64>,
        expected_graph_timestamps: Vec<i64>,
        expected_remaining: Vec<f64>,
        expected_sol_max: f64,
        expected_period_count: usize,
        #[serde(default)]
        moving_full_acquisition_samples: Vec<GraphFixtureHistorySample>,
        details_response: GraphFixtureDetailsResponse,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GraphFixtureDetailsResponse {
        api_version: String,
        state: String,
        observed_at: i64,
        authenticated: bool,
        plan_label: String,
        quota: GraphFixtureQuota,
        models: Vec<Value>,
        active_thread_count: usize,
        history_periods: Vec<GraphFixtureHistoryPeriod>,
        history_samples: Vec<GraphFixtureHistorySample>,
        history_gaps: Vec<Value>,
        threads: Vec<Value>,
        estimated_cost_label: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GraphFixtureQuota {
        remaining_percent: f64,
        reset_at: i64,
        window_seconds: i64,
        monthly: bool,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GraphFixtureHistoryPeriod {
        id: String,
        start_at: i64,
        end_at: i64,
        reset_at: i64,
        label: String,
        current: bool,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GraphFixtureHistorySample {
        timestamp: i64,
        reset_at: i64,
        remaining_percent: Option<f64>,
        sol_dollars: f64,
        terra_dollars: f64,
        luna_dollars: f64,
        sol_tokens: u64,
        terra_tokens: u64,
        luna_tokens: u64,
    }

    impl GraphFixtureHistorySample {
        fn to_usage_history_sample(&self) -> UsageHistorySample {
            UsageHistorySample {
                timestamp: self.timestamp,
                reset_at: self.reset_at,
                remaining_percent: self.remaining_percent.unwrap_or(-1.0),
                sol_dollars: self.sol_dollars,
                terra_dollars: self.terra_dollars,
                luna_dollars: self.luna_dollars,
                sol_tokens: self.sol_tokens,
                terra_tokens: self.terra_tokens,
                luna_tokens: self.luna_tokens,
            }
        }
    }

    fn state_from_graph_fixture(fixture: &GraphFixture) -> CodexInfoState {
        let details = &fixture.details_response;
        let samples = details
            .history_samples
            .iter()
            .map(GraphFixtureHistorySample::to_usage_history_sample)
            .collect::<Vec<_>>();
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };
        let canonical_reset_at = history.canonical_reset_at(details.quota.reset_at);
        let mut state = CodexInfoState::preview("normal");
        state.authenticated = details.authenticated;
        state.plan_label = details.plan_label.clone();
        state.remaining_percent = Some(details.quota.remaining_percent);
        state.has_quota_percent = true;
        state.has_usage = true;
        state.local_usage_pending = false;
        state.reset_at = Some(details.quota.reset_at);
        state.window_seconds = details.quota.window_seconds;
        state.monthly = details.quota.monthly;
        state.last_success_at = Some(details.observed_at);
        state.history = history;
        state.selected_reset_at = Some(canonical_reset_at);
        state.selected_history_period.clear();
        state
    }

    fn active_thread_fixture(index: usize, updated_at: i64) -> ActiveThread {
        let parent_thread_id = (index % 3 == 2).then(|| format!("parent-{index:03}"));
        ActiveThread {
            id: format!("thread-{index:03}"),
            created_at: Some(1_000 + index as i64),
            updated_at,
            title: format!("title-{index:03}"),
            model: format!("model-{index:03}"),
            model_label: format!("model-label-{index:03}"),
            total_tokens: Some(index as u64),
            context_usage_tokens: Some(index as u64 + 1),
            context_window_tokens: Some(10_000 + index as u64),
            last_user_message_at: Some(2_000 + index as i64),
            is_subagent: parent_thread_id.is_some(),
            parent_thread_id,
            depth: (index % 3 == 2).then_some((index % 8) as i32),
        }
    }

    fn assert_public_thread_matches(active: &ActiveThread, public: &super::PublicThread) {
        assert_eq!(public.id, active.id);
        assert_eq!(public.title, active.title);
        assert_eq!(public.parent_thread_id, active.parent_thread_id);
        assert_eq!(public.model, active.model);
        assert_eq!(public.model_label, active.model_label);
        assert_eq!(public.total_tokens, active.total_tokens);
        assert_eq!(public.context_usage_tokens, active.context_usage_tokens);
        assert_eq!(public.context_window_tokens, active.context_window_tokens);
        assert_eq!(public.created_at, active.created_at);
        assert_eq!(public.last_user_message_at, active.last_user_message_at);
        assert_eq!(public.is_subagent, active.is_subagent);
        assert_eq!(public.depth, active.depth);
    }

    fn raw_loopback_get(address: SocketAddr, route: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request =
            format!("GET {route} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn raw_loopback_body(response: &str) -> Value {
        let (_, body) = response
            .split_once("\r\n\r\n")
            .expect("loopback response has headers and body");
        serde_json::from_str(body).expect("loopback response body is JSON")
    }

    fn raw_loopback_pair(response: &str) -> String {
        response
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("Codex-Info-Published-Pair")
                    .then(|| value.trim().to_owned())
            })
            .expect("published pair header")
    }

    use super::{
        claim_manual_x11_action, forbidden_x11_states, manual_resize_geometry,
        manual_window_geometry, motif_wm_functions, motif_wm_resizable_functions, X11StateAtoms,
    };
    use chrono::{TimeZone, Utc};
    use codex_info::security;
    use codex_info::server::{ApiSnapshotError, PublicState, MAX_PUBLIC_THREADS};
    use codex_info::thread_contract;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs::{self, File};
    use std::io::{BufReader, Read, Seek, SeekFrom, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn launch_args(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn launch_options_follow_the_public_contract() {
        let default_address = DEFAULT_SERVICE_ADDRESS.parse().unwrap();
        let LaunchMode::Service(default_config) = parse_launch_mode(launch_args(&[])).unwrap()
        else {
            panic!("default mode was not service-only");
        };
        assert_eq!(default_config.listen_addr(), default_address);
        let LaunchMode::All(ui_config) = parse_launch_mode(launch_args(&["--ui"])).unwrap() else {
            panic!("--ui mode was not all");
        };
        assert_eq!(ui_config.listen_addr(), default_address);
        assert_eq!(
            parse_launch_mode(launch_args(&["--help"])).unwrap(),
            LaunchMode::Help
        );
        assert_eq!(
            parse_launch_mode(launch_args(&["--h"])).unwrap(),
            LaunchMode::Help
        );
        assert_eq!(
            parse_launch_mode(launch_args(&["-h"])).unwrap(),
            LaunchMode::Help
        );
        assert_eq!(
            parse_launch_mode(launch_args(&["--stop"])).unwrap(),
            LaunchMode::Stop
        );
        let LaunchMode::Service(config) =
            parse_launch_mode(launch_args(&["--port", "9876"])).unwrap()
        else {
            panic!("service mode was not selected");
        };
        assert_eq!(config.listen_addr(), "127.0.0.1:9876".parse().unwrap());
        let LaunchMode::All(config) =
            parse_launch_mode(launch_args(&["--ui", "--port", "4321"])).unwrap()
        else {
            panic!("UI mode with explicit port was not selected");
        };
        assert_eq!(config.listen_addr(), "127.0.0.1:4321".parse().unwrap());

        for port in ["1", "65535"] {
            assert!(parse_launch_mode(launch_args(&["--port", port])).is_ok());
        }
        for invalid in ["0", "65536", "-1", "abc", "127.0.0.1:9876", ""] {
            assert!(
                parse_launch_mode(launch_args(&["--port", invalid])).is_err(),
                "invalid port accepted: {invalid:?}"
            );
        }
        for legacy in [
            "--service",
            "--ui-only",
            "--all",
            "--listen",
            "--record-daemon",
            "--once",
            "--ui-onlry",
        ] {
            assert!(
                parse_launch_mode(launch_args(&[legacy])).is_err(),
                "legacy or misspelled option accepted: {legacy}"
            );
        }
        for invalid in [
            &["--port"][..],
            &["--ui", "--port"][..],
            &["--port", "9876", "--ui"][..],
            &["--ui", "--ui"][..],
            &["--stop", "--port", "9876"][..],
            &["--help", "--ui"][..],
        ] {
            assert!(
                parse_launch_mode(launch_args(invalid)).is_err(),
                "invalid option combination accepted: {invalid:?}"
            );
        }
    }

    #[test]
    fn ui_polling_holds_selected_endpoint_error_until_that_endpoint_is_healthy() {
        let blocker = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = blocker.local_addr().unwrap();
        let mut state = CodexInfoState::preview("normal");
        state.preview = false;
        state.service_endpoint_error = Some("selected endpoint unavailable".into());

        poll_service_state(&mut state, endpoint);
        assert_eq!(
            state.service_endpoint_error.as_deref(),
            Some("selected endpoint unavailable")
        );

        drop(blocker);
        let mut server =
            ApiServer::start(ApiServerConfig::new(endpoint).unwrap()).expect("selected endpoint");
        poll_service_state(&mut state, endpoint);
        assert!(state.service_endpoint_error.is_none());
        server.shutdown();
    }

    #[test]
    fn service_health_requires_a_complete_success_json_body_without_waiting_for_eof() {
        let valid = b"HTTP/1.1 200 OK\r\nContent-Length: 43\r\nConnection: keep-alive\r\n\r\n{\"api_version\":\"v1\",\"service\":\"codex-info\"}";
        assert!(is_service_health_response(valid));
        assert!(!is_service_health_response(
            b"HTTP/1.1 200 OK\r\nX-Service: codex-info\r\n\r\n{}"
        ));
        assert!(!is_service_health_response(
            b"HTTP/1.1 500 Error\r\n\r\n{\"api_version\":\"v1\",\"service\":\"codex-info\"}"
        ));
        assert!(!is_service_health_response(
            b"HTTP/1.1 200 OK\r\n\r\n{\"api_version\":\"v1\",\"service\":\"codex-info\""
        ));
    }

    #[test]
    fn service_health_accepts_the_live_loopback_server_response() {
        let server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        assert!(service_is_healthy(server.local_addr()));
    }

    #[test]
    #[cfg(unix)]
    fn owned_background_child_cleanup_is_bounded_and_reaps() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert!(terminate_and_reap_owned_child(&mut child));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn public_snapshot_is_whitelisted_and_tracks_auth_state() {
        let normal = CodexInfoState::preview("normal");
        let snapshot = normal.public_snapshot();
        assert_eq!(snapshot.state, PublicState::Ready);
        assert!(snapshot.authenticated);
        assert_eq!(snapshot.plan_label.as_deref(), Some("Pro"));
        assert_eq!(
            snapshot.quota.as_ref().map(|quota| quota.remaining_percent),
            Some(14.0)
        );
        assert_eq!(snapshot.models.len(), 3);
        assert_eq!(snapshot.models[0].name, "SOL");
        assert_eq!(snapshot.models[0].input_tokens, 80_000_000);
        assert_eq!(snapshot.models[0].cached_input_tokens, 30_000_000);
        assert_eq!(snapshot.active_thread_count, 1);
        let json = serde_json::to_value(snapshot).expect("public snapshot serializes");
        assert!(json.get("email").is_none());
        assert!(json.get("auth_url").is_none());
        assert!(json.get("error").is_none());
        assert!(json.get("history").is_none());
        assert!(json.get("session_path").is_none());

        let details = normal.public_details();
        assert_eq!(details.models.len(), 3);
        assert!(details.models[0].input_dollars.is_finite());
        assert!(details.models[0].input_dollars > 0.0);
        assert!(!details.history_periods.is_empty());
        assert!(!details.history_samples.is_empty());
        assert!(details.history_gaps.is_empty());
        assert_eq!(details.threads.len(), 1);
        assert_eq!(details.threads[0].model_label, "gpt-5.6-sol");
        let details_json = serde_json::to_value(details).expect("public details serializes");
        assert!(details_json.get("email").is_none());
        assert!(details_json.get("auth_url").is_none());
        assert!(details_json.get("session_path").is_none());

        let auth = CodexInfoState::preview("auth").public_snapshot();
        assert_eq!(auth.state, PublicState::AuthRequired);
        assert!(!auth.authenticated);
        assert!(auth.plan_label.is_none());
        assert!(auth.observed_at.is_none());
        assert!(auth.quota.is_none());
        assert!(auth.models.is_empty());
        assert_eq!(auth.active_thread_count, 0);

        let initializing = CodexInfoState::preview("initializing").public_snapshot();
        assert_eq!(initializing.state, PublicState::Initializing);
        assert!(!initializing.authenticated);
        assert!(initializing.plan_label.is_none());
        assert!(initializing.observed_at.is_none());
        assert!(initializing.quota.is_none());
        assert!(initializing.models.is_empty());
        assert_eq!(initializing.active_thread_count, 0);

        let startup = CodexInfoState::preview("startup-loading");
        assert!(startup.authenticated);
        assert!(!startup.has_visible_usage());
        assert!(native_startup_loading(
            startup.authenticated,
            startup.has_visible_usage(),
            startup.local_usage_error,
            startup.account_error.is_some(),
            startup.error.is_some(),
        ));
        assert_eq!(startup.public_snapshot().state, PublicState::Initializing);
        assert!(startup.public_snapshot().models.is_empty());
        assert!(startup.public_snapshot().quota.is_none());
        assert!(startup.public_details().history_samples.is_empty());

        let unlimited = CodexInfoState::preview("unlimited").public_snapshot();
        assert_eq!(unlimited.state, PublicState::Ready);
        assert!(unlimited.quota.is_none());
    }

    #[test]
    fn public_details_preserves_256_thread_rows_and_all_fields_for_publication() {
        let mut state = CodexInfoState::preview("normal");
        state.active_threads = (0..MAX_PUBLIC_THREADS)
            .map(|index| active_thread_fixture(index, 10_000 - index as i64))
            .collect();
        state.history = UsageHistory::default();

        let details = state.public_details();
        assert_eq!(details.threads.len(), 256);
        assert_eq!(details.active_thread_count, 256);
        for (active, public) in state.active_threads.iter().zip(&details.threads) {
            assert_public_thread_matches(active, public);
        }
        assert_eq!(
            details
                .threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            (0..256)
                .map(|index| format!("thread-{index:03}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        assert_eq!(server.publisher().publish_details(details), Ok(()));
        server.shutdown();
    }

    #[test]
    fn public_details_preserves_257_thread_rows_and_rejects_publication_atomically() {
        let mut state = CodexInfoState::preview("normal");
        state.active_threads = (0..=MAX_PUBLIC_THREADS)
            .map(|index| active_thread_fixture(index, 10_000 - index as i64))
            .collect();
        state.history = UsageHistory::default();
        let over_capacity = state.public_details();
        assert_eq!(over_capacity.threads.len(), 257);
        assert_eq!(over_capacity.active_thread_count, 257);

        state.active_threads.truncate(MAX_PUBLIC_THREADS);
        let known_good = state.public_details();
        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        let publisher = server.publisher();
        publisher.publish_details(known_good).unwrap();
        let before_pair = publisher.published_pair();
        let before_status = raw_loopback_get(server.local_addr(), "/v1/status");
        let before_details = raw_loopback_get(server.local_addr(), "/v1/details");

        assert_eq!(
            publisher.publish_details(over_capacity),
            Err(ApiSnapshotError::ListTooLong)
        );
        assert_eq!(publisher.published_pair(), before_pair);
        assert_eq!(
            raw_loopback_get(server.local_addr(), "/v1/status"),
            before_status
        );
        assert_eq!(
            raw_loopback_get(server.local_addr(), "/v1/details"),
            before_details
        );
        server.shutdown();
    }

    #[test]
    fn service_publishes_old_rows_error_and_latest_quota_without_freeze() {
        let mut state = CodexInfoState::preview("normal");
        state.history = UsageHistory::default();
        let old = active_thread_fixture(0, 100);
        state.active_threads = vec![old.clone()];

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        let publisher = server.publisher();
        publisher.publish_details(state.public_details()).unwrap();
        let before_pair = publisher.published_pair();

        let mut cycle_a = active_thread_fixture(10, 220);
        cycle_a.parent_thread_id = Some("thread-011".into());
        let mut cycle_b = active_thread_fixture(11, 219);
        cycle_b.parent_thread_id = Some("thread-010".into());
        state.apply_thread_result(
            state.auth_epoch,
            ActiveThreadUpdate::Snapshot(vec![cycle_a, cycle_b]),
        );
        assert_eq!(state.active_threads.as_slice(), std::slice::from_ref(&old));
        assert!(state.thread_error);

        let reset_at = state.reset_at.expect("preview reset");
        state.apply_usage_event(usage_event(Some(37.0), reset_at));
        publisher.publish_details(state.public_details()).unwrap();

        let status = raw_loopback_get(server.local_addr(), "/v1/status");
        let details = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_eq!(raw_loopback_pair(&status), raw_loopback_pair(&details));
        assert_ne!(publisher.published_pair(), before_pair);
        assert_eq!(raw_loopback_body(&status)["state"], "error");
        assert_eq!(raw_loopback_body(&details)["state"], "error");
        assert_eq!(
            raw_loopback_body(&status)["quota"]["remaining_percent"],
            37.0
        );
        assert_eq!(
            raw_loopback_body(&details)["quota"]["remaining_percent"],
            37.0
        );
        assert_eq!(raw_loopback_body(&details)["active_thread_count"], 1);
        assert_eq!(raw_loopback_body(&details)["threads"][0]["id"], old.id);

        state.apply_usage_event(usage_event(Some(29.0), reset_at));
        publisher.publish_details(state.public_details()).unwrap();
        assert_eq!(raw_loopback_body(&status)["state"], "error");
        let advanced_status = raw_loopback_get(server.local_addr(), "/v1/status");
        assert_eq!(
            raw_loopback_body(&advanced_status)["quota"]["remaining_percent"],
            29.0
        );

        let replacement = active_thread_fixture(1, 230);
        state.apply_thread_result(
            state.auth_epoch,
            ActiveThreadUpdate::Snapshot(vec![replacement.clone()]),
        );
        assert_eq!(
            state.active_threads.as_slice(),
            std::slice::from_ref(&replacement)
        );
        assert!(!state.thread_error);
        publisher.publish_details(state.public_details()).unwrap();
        let recovered = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_eq!(raw_loopback_body(&recovered)["state"], "ready");
        assert_eq!(
            raw_loopback_body(&recovered)["threads"][0]["id"],
            replacement.id
        );
        server.shutdown();
    }

    #[test]
    fn public_details_materializes_exactly_one_calendar_month() {
        let mut state = CodexInfoState::preview("normal");
        let now = Utc::now();
        let now_timestamp = now.timestamp();
        let cutoff = one_month_before_utc(now);
        let reset_at = now_timestamp + WEEK_SECONDS;
        state.history.samples = [cutoff, cutoff + 1, now_timestamp, now_timestamp + 60]
            .into_iter()
            .map(|timestamp| UsageHistorySample {
                timestamp,
                reset_at,
                remaining_percent: 80.0,
                sol_dollars: 0.0,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 0,
                terra_tokens: 0,
                luna_tokens: 0,
            })
            .collect();

        let timestamps = state
            .public_details()
            .history_samples
            .into_iter()
            .map(|sample| sample.timestamp)
            .collect::<Vec<_>>();

        assert_eq!(timestamps, vec![cutoff + 1, now_timestamp]);
    }

    #[test]
    fn public_details_current_period_end_is_effective_observed_end() {
        let now = Utc::now().timestamp();
        let reset_at = now + 3_600;
        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(reset_at);
        state.last_success_at = Some(now - 120);
        state.history.samples = vec![UsageHistorySample::new(
            now - 60,
            reset_at,
            80.0,
            ModelDollarTotals::default(),
        )];

        let details = state.public_details();
        let observed_at = details.observed_at.expect("fixture has an observation");
        let current = details
            .history_periods
            .iter()
            .find(|period| period.current)
            .expect("fixture has a current period");
        assert_eq!(current.end_at, reset_at.min(observed_at));
        assert!(current.end_at < current.reset_at);
    }

    #[test]
    fn nearby_future_reset_periods_publish_only_one_current_period() {
        let now = Utc::now().timestamp();
        let first_reset = now + 3_600;
        let second_reset = now + 3_700;
        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(now + 3_650);
        state.history.samples = vec![
            UsageHistorySample::new(now - 100, first_reset, 80.0, ModelDollarTotals::default()),
            UsageHistorySample::new(now - 200, second_reset, 70.0, ModelDollarTotals::default()),
        ];

        let periods = state.history_periods();
        assert_eq!(periods.len(), 2);
        let current = current_history_period_reset(&periods, state.reset_at, now);
        assert!(current.is_some());
        let details = state.public_details();
        assert_eq!(
            details
                .history_periods
                .iter()
                .filter(|period| period.current)
                .count(),
            1
        );
    }

    #[test]
    fn enterprise_individual_limit_wins_and_uses_calendar_month() {
        let reset_at = 1_735_689_600;
        let fixture = json!({
            "rateLimits": {
                "planType": "enterprise",
                "primary": {"usedPercent": 12, "resetsAt": reset_at + 604800, "windowDurationMins": 10080},
                "individualLimit": {
                    "remainingPercent": 73,
                    "resetsAt": reset_at,
                    "limit": "1000000",
                    "used": "270000"
                }
            }
        });
        let parsed = parse_rate_limits(&fixture, Some("enterprise"), reset_at - 86_400)
            .expect("individualLimit fixture should parse");
        assert_eq!(parsed.remaining_percent, Some(73.0));
        assert!(parsed.monthly);
        assert_eq!(parsed.quota_title, "月間残り利用枠");
        assert_eq!(parsed.window_seconds, monthly_window_seconds(reset_at));
        assert_ne!(parsed.window_seconds, 7 * 86_400);
    }

    #[test]
    fn individual_limit_is_monthly_only_for_exact_enterprise_plans() {
        let monthly_reset_at = 1_735_689_600;
        let fixed_reset_at = monthly_reset_at + 604_800;
        for plan in [
            "enterprise",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
        ] {
            let fixture = json!({
                "rateLimits": {
                    "planType": plan,
                    "individualLimit": {
                        "remainingPercent": 73,
                        "resetsAt": monthly_reset_at,
                        "limit": "100",
                        "used": "27"
                    },
                    "primary": {
                        "usedPercent": 12,
                        "resetsAt": fixed_reset_at,
                        "windowDurationMins": 10080
                    }
                }
            });
            let parsed = parse_rate_limits(&fixture, Some(plan), 0)
                .expect("exact enterprise plan should parse individualLimit");
            assert!(parsed.monthly, "plan={plan}");
            assert_eq!(parsed.reset_at, monthly_reset_at, "plan={plan}");
            assert_eq!(parsed.remaining_percent, Some(73.0), "plan={plan}");
        }

        for plan in ["business", "self_serve_business_prolite", "pro"] {
            let fixture = json!({
                "rateLimits": {
                    "planType": plan,
                    "individualLimit": {
                        "remainingPercent": 73,
                        "resetsAt": monthly_reset_at,
                        "limit": "100",
                        "used": "27"
                    },
                    "primary": {
                        "usedPercent": 12,
                        "resetsAt": fixed_reset_at,
                        "windowDurationMins": 10080
                    }
                }
            });
            let parsed = parse_rate_limits(&fixture, Some(plan), 0)
                .expect("a fixed bucket should remain available for non-enterprise plans");
            assert!(!parsed.monthly, "plan={plan}");
            assert_eq!(parsed.remaining_percent, Some(88.0), "plan={plan}");
            assert_eq!(parsed.reset_at, fixed_reset_at, "plan={plan}");
            assert_eq!(parsed.window_seconds, 10080 * 60, "plan={plan}");
        }

        let alias = json!({"rateLimits": {"planType": "enterprise"}});
        assert!(
            parse_rate_limits(&alias, Some("chatgpt-enterprise"), 0).is_err(),
            "schema-external aliases must not be normalized"
        );
    }

    #[test]
    fn fixed_rate_limit_chooses_the_longest_valid_secondary_bucket() {
        let fixture = json!({
            "rateLimits": {
                "limitName": "Codex",
                "primary": {"usedPercent": 20, "resetsAt": 3000, "windowDurationMins": 10080},
                "secondary": {"usedPercent": 30, "resetsAt": 4000, "windowDurationMins": 43200}
            },
            "rateLimitsByLimitId": {
                "ignored": {"primary": {"usedPercent": 0, "resetsAt": 9000, "windowDurationMins": 527040}}
            }
        });
        let parsed =
            parse_rate_limits(&fixture, Some("pro"), 0).expect("fixed bucket should parse");
        assert_eq!(parsed.remaining_percent, Some(70.0));
        assert_eq!(parsed.reset_at, 4000);
        assert_eq!(parsed.window_seconds, 43200 * 60);
    }

    #[test]
    fn quota_candidate_tie_break_order_is_total_and_deterministic() {
        let fixed = json!({"rateLimits": {
            "limitName": "Codex",
            "primary":{"usedPercent":5, "resetsAt":8000, "windowDurationMins":10080},
            "secondary":{"usedPercent":3, "resetsAt":8000, "windowDurationMins":10080}
        }});
        let selected = parse_rate_limits(&fixed, Some("pro"), 0).unwrap();
        assert_eq!(selected.window_seconds, 10_080 * 60);
        assert_eq!(selected.reset_at, 8000);
        assert_eq!(selected.limit_name, "Codex");
        assert_eq!(selected.remaining_percent, Some(95.0));

        let later_secondary = json!({"rateLimits": {
            "primary":{"usedPercent":5, "resetsAt":8000, "windowDurationMins":10080},
            "secondary":{"usedPercent":3, "resetsAt":8001, "windowDurationMins":10080}
        }});
        assert_eq!(
            parse_rate_limits(&later_secondary, Some("pro"), 0)
                .unwrap()
                .remaining_percent,
            Some(97.0)
        );
    }

    #[test]
    fn local_estimate_price_version_cost_rounding_and_large_tokens_are_fixed() {
        assert_eq!(LOCAL_ESTIMATE_PRICE_VERSION, "LOCAL_ESTIMATE_V1_2026-08-14");
        let rows = [
            ModelUsageRow {
                name: "SOL".into(),
                tokens: 3_000_000,
                input_tokens: 2_000_000,
                cached_input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            ModelUsageRow {
                name: "TERRA".into(),
                tokens: 3_000_000,
                input_tokens: 2_000_000,
                cached_input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            ModelUsageRow {
                name: "LUNA".into(),
                tokens: 3_000_000,
                input_tokens: 2_000_000,
                cached_input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
        ];
        let totals = ModelDollarTotals::from_rows(&rows);
        assert_eq!(totals.sol, 35.5);
        assert_eq!(totals.terra, 14.2);
        assert!((totals.luna - 1.42).abs() < f64::EPSILON);
        assert_eq!(format_estimated_cost(totals), "概算 $51");
        assert_eq!(
            format_estimated_cost(ModelDollarTotals {
                sol: 1_234.5,
                terra: 0.0,
                luna: 0.0,
            }),
            "概算 $1,235"
        );
        assert_eq!(
            format_estimated_cost(ModelDollarTotals::default()),
            "概算 $0"
        );

        let maximum = ModelUsageRow {
            name: "SOL".into(),
            tokens: u64::MAX,
            input_tokens: u64::MAX,
            cached_input_tokens: 0,
            output_tokens: u64::MAX,
        };
        let costs = maximum.dollar_costs();
        assert!(costs.0.is_finite() && costs.2.is_finite());
        assert!(
            format_estimated_cost(ModelDollarTotals::from_rows(&[maximum])).starts_with("概算 $")
        );

        let mut state = CodexInfoState::preview("normal");
        let before = state.estimated_cost_label.clone();
        state.select_latest_history();
        state.select_older_history();
        assert_eq!(state.estimated_cost_label, before);
    }

    #[test]
    fn unlimited_credits_never_create_a_fake_percentage() {
        let fixture = json!({"rateLimits": {
            "credits": {"hasCredits": false, "unlimited": true, "balance": null}
        }});
        let parsed =
            parse_rate_limits(&fixture, Some("enterprise"), 0).expect("unlimited should parse");
        assert_eq!(parsed.remaining_percent, None);
        assert_eq!(parsed.quota_title, "利用枠");
    }

    #[test]
    fn enterprise_plan_variants_have_a_single_japanese_display_name() {
        for plan in [
            "enterprise",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
        ] {
            assert_eq!(plan_type_label(Some(plan)), "エンタープライズ");
        }
        for plan in [
            "chatgpt_enterprise",
            "enterprise_trial",
            "enterprise_customer",
            "enterprise-edu",
        ] {
            assert_eq!(plan_type_label(Some(plan)), "プラン未設定");
        }
    }

    #[test]
    fn codex_plan_values_have_stable_display_labels() {
        for (plan, expected) in [
            ("free", "無料"),
            ("go", "Go"),
            ("plus", "Plus"),
            ("pro", "Pro"),
            ("prolite", "Pro Lite"),
            ("team", "Team"),
            ("self_serve_business_prolite", "Business"),
            ("self_serve_business_usage_based", "Business"),
            ("business", "Business"),
            ("ent26", "エンタープライズ"),
            ("enterprise_cbp_automation", "エンタープライズ"),
            ("enterprise_cbp_usage_based", "エンタープライズ"),
            ("enterprise", "エンタープライズ"),
            ("edu", "教育"),
            ("unknown", "プラン未設定"),
        ] {
            assert_eq!(plan_type_label(Some(plan)), expected, "plan={plan}");
        }
        assert_eq!(plan_type_label(Some("chatgpt-plus")), "プラン未設定");
        assert_eq!(plan_type_label(Some("chatgpt-business")), "プラン未設定");
    }

    #[test]
    fn plan_normalization_and_monthly_boundary_matrix() {
        for plan in [
            None,
            Some(""),
            Some("unknown-plan"),
            Some("エンタープライズ"),
            Some(" \tenterprise\r\n"),
            Some("ENTERPRISE"),
            Some("Pro-Lite"),
        ] {
            assert_eq!(plan_type_label(plan), "プラン未設定", "plan={plan:?}");
        }

        let leap_month_end = Utc.with_ymd_and_hms(2024, 3, 31, 12, 0, 0).unwrap();
        let previous = Utc.with_ymd_and_hms(2024, 2, 29, 12, 0, 0).unwrap();
        assert_eq!(
            monthly_window_seconds(leap_month_end.timestamp()),
            (leap_month_end - previous).num_seconds()
        );
    }

    #[test]
    fn rate_limit_parser_projects_31_used_to_69_remaining_without_fallback() {
        let fixture = json!({
            "rateLimits": {
                "limitName": "Codex weekly",
                "primary": {
                    "usedPercent": 31,
                    "resetsAt": 2000,
                    "windowDurationMins": 10080
                }
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "primary": {
                        "usedPercent": 0,
                        "resetsAt": 9000,
                        "windowDurationMins": 10080
                    }
                }
            }
        });
        let parsed = parse_rate_limits(&fixture, Some("pro"), 0)
            .expect("canonical fixed bucket should parse");
        assert_eq!(parsed.remaining_percent, Some(69.0));
        assert_eq!(parsed.limit_name, "Codex weekly");
        assert_eq!(parsed.quota_title, "残り利用枠");

        let invalid_numeric_string = json!({"rateLimits": {
            "primary": {"usedPercent": "31", "resetsAt": 2000, "windowDurationMins": 10080}
        }});
        assert!(
            parse_rate_limits(&invalid_numeric_string, Some("pro"), 0).is_err(),
            "schema-invalid input must be unavailable, never synthesized as 100% remaining"
        );
    }

    fn usage_event(remaining_percent: Option<f64>, reset_at: i64) -> UsageEvent {
        UsageEvent {
            remaining_percent,
            reset_at,
            window_seconds: 604_800,
            limit_name: "Codex".into(),
            quota_title: "残り利用枠".into(),
            monthly: false,
        }
    }

    #[test]
    fn quota_projection_and_thread_state_transitions_are_atomic() {
        let mut state = CodexInfoState::preview("normal");
        let previous = state.active_threads.clone();
        let reset_at = state.reset_at.expect("preview quota has reset");
        let history_before = state.history.samples.clone();

        state.apply_usage_event(usage_event(Some(69.0), reset_at));
        assert_eq!(state.remaining_percent, Some(69.0));
        assert_eq!(state.active_threads, previous);
        assert_eq!(state.history.samples, history_before);

        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::Failed);
        assert!(state.thread_error);
        assert_eq!(state.active_threads, previous);
        assert_eq!(
            state.public_snapshot().active_thread_count,
            previous.len() as u64
        );
        assert_eq!(state.public_details().threads.len(), previous.len());
        assert_eq!(
            active_thread_rows_at(&state.active_threads, 0).len(),
            previous.len()
        );
        assert_eq!(
            state.status,
            "利用枠は更新しました。スレッド情報の取得に失敗し、実行中の状態は未確認です。"
        );

        let replacement = ActiveThread {
            id: "replacement".into(),
            created_at: Some(60),
            updated_at: 123,
            title: "replacement title".into(),
            model: "gpt-5.6-terra".into(),
            model_label: "gpt-5.6-terra".into(),
            total_tokens: Some(98_765),
            context_usage_tokens: None,
            context_window_tokens: None,
            last_user_message_at: Some(120),
            is_subagent: true,
            parent_thread_id: Some("parent".into()),
            depth: Some(1),
        };
        state.apply_thread_result(
            state.auth_epoch,
            ActiveThreadUpdate::Snapshot(vec![replacement.clone()]),
        );
        assert_eq!(state.active_threads, [replacement]);
        assert!(state.error.is_none());

        state.apply_usage_event(usage_event(Some(67.0), reset_at));
        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::NoThread);
        assert!(state.active_threads.is_empty());
    }

    #[test]
    fn periodic_quota_refresh_retains_last_good_main_snapshot() {
        let mut state = CodexInfoState::preview("normal");
        let previous_models = state.model_usage.clone();
        let previous_threads = state.active_threads.len();
        let previous_history = state.history.samples.clone();
        let previous_estimate = state.estimated_cost_label.clone();
        let previous_reset = state.reset_at.expect("preview reset");

        // A moving reset timestamp must not be treated as an account change or
        // as proof that the current model table is invalid.  This is the
        // exact failure mode that made the main screen blink every cycle.
        state.apply_usage_event(usage_event(Some(14.0), previous_reset + 120));
        assert_eq!(state.model_usage, previous_models);

        // A periodic quota response is intentionally incomplete.  Simulate
        // the interval after that response and before the local collector's
        // matching commit; the previously committed values must remain
        // visible instead of reverting to an empty/initial screen.
        state.local_usage_pending = true;
        state.usage_snapshot_committed = true;

        // A single successful assertion is not enough: the production bug
        // appeared only after several timer-driven refreshes. Exercise a
        // finite sequence of rolling reset values and verify that every
        // intermediate publish keeps the same last-good snapshot.
        for cycle in 0..8 {
            state.apply_usage_event(usage_event(
                Some(14.0 - f64::from(cycle) * 0.25),
                previous_reset + 120 + i64::from(cycle + 1) * 120,
            ));
            state.local_usage_pending = true;
            state.usage_snapshot_committed = true;

            let snapshot = state.public_snapshot();
            assert_eq!(snapshot.state, PublicState::Ready);
            assert_eq!(snapshot.models.len(), previous_models.len());
            assert_eq!(snapshot.active_thread_count as usize, previous_threads);
            assert_eq!(state.history.samples, previous_history);
            assert_eq!(state.estimated_cost_label, previous_estimate);
            assert_eq!(state.selected_reset_at, Some(previous_reset));
            let details = state.public_details();
            assert_eq!(details.models.len(), previous_models.len());
            assert_eq!(details.threads.len(), previous_threads);
            assert!(state.has_visible_usage());
        }
    }

    #[test]
    fn rolling_reset_timestamp_drift_is_not_a_new_period() {
        let now = 2_000_000_000;
        let window = WEEK_SECONDS;
        assert!(!reset_transition_is_boundary(
            Some(now + window - 60),
            Some(87.0),
            now + window + 60,
            Some(86.0),
            Some(now),
            now + 60,
            window,
        ));
        assert!(reset_transition_is_boundary(
            Some(now + 60),
            Some(1.0),
            now + window + 60,
            Some(100.0),
            Some(now),
            now + 60,
            window,
        ));
    }

    #[test]
    fn local_usage_failure_keeps_valid_quota_and_never_invents_zero_history() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview quota has reset");
        let history_len = state.history.samples.len();
        let previous_cost = state.estimated_cost_label.clone();
        let previous_columns = format_model_usage_columns(&state.model_usage);
        let threads = state.active_threads.clone();

        state.apply_usage_event(usage_event(Some(24.0), reset_at));
        state.apply_thread_result(
            state.auth_epoch,
            ActiveThreadUpdate::Snapshot(threads.clone()),
        );
        state.apply_local_usage_error(state.auth_epoch, reset_at, WEEK_SECONDS);

        assert!(state.has_usage);
        assert_eq!(state.remaining_percent, Some(24.0));
        assert_eq!(state.reset_at, Some(reset_at));
        assert_eq!(state.active_threads, threads);
        assert_eq!(state.estimated_cost_label, previous_cost);
        assert_eq!(
            format_model_usage_columns(&state.model_usage),
            previous_columns
        );
        assert_eq!(state.history.samples.len(), history_len);
        assert!(state.local_usage_error);
        assert_eq!(
            state.status,
            "利用枠は更新しました。履歴は前回値を保持しています。"
        );

        state.model_usage.clear();
        state.estimated_cost_label = "概算 —".into();
        state.history = UsageHistory::default();
        state.reset_at = None;
        let next_reset = reset_at + WEEK_SECONDS;
        state.apply_usage_event(usage_event(Some(24.0), next_reset));
        state.apply_local_usage_error(state.auth_epoch, next_reset, WEEK_SECONDS);
        assert!(state.has_usage);
        assert_eq!(state.remaining_percent, Some(24.0));
        assert!(state.model_usage.is_empty());
        assert_eq!(state.estimated_cost_label, "概算 —");
        assert!(state.history.samples.is_empty());
    }

    #[test]
    fn persisted_period_backfill_is_admitted_before_auth_without_publishing_usage() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview quota has reset");
        state.authenticated = false;
        state.auth_epoch = 7;
        state.recovery_period = Some((reset_at, WEEK_SECONDS));
        let db_path = std::env::temp_dir().join(format!(
            "codex-info-recovery-test-{}.sqlite3",
            std::process::id()
        ));
        let _ = fs::remove_file(&db_path);
        state.history = UsageHistory::load_from_db_path(Some(db_path.clone()));
        state.model_usage.clear();
        let sample = UsageHistorySample::new_with_usage(
            Utc::now().timestamp(),
            reset_at,
            -1.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 0.0,
                luna: 0.0,
            },
            ModelTokenTotals {
                sol: 100,
                terra: 0,
                luna: 0,
            },
        );
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: 0,
            reset_at,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![sample],
        });
        assert_eq!(state.history.samples.len(), 1);
        assert!(state.model_usage.is_empty());
        assert!(!state.public_snapshot().authenticated);
        assert!(state.public_details().history_samples.is_empty());
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn recovery_backfill_is_one_shot_until_authenticated_quota_returns() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview quota has reset");
        state.authenticated = false;
        state.auth_epoch = 7;
        state.recovery_period = Some((reset_at, WEEK_SECONDS));
        let db_path = std::env::temp_dir().join(format!(
            "codex-info-recovery-latch-{}.sqlite3",
            std::process::id()
        ));
        state.history = UsageHistory {
            db_path: Some(db_path.clone()),
            samples: Vec::new(),
            startup_maintenance_done: true,
        };

        state.apply_account_error("account bridge unavailable".into());
        assert!(state.recovery_requested);
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: 0,
            reset_at,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: Vec::new(),
        });
        assert!(state.recovery_requested);
        state.apply_account_error("account bridge retry failed".into());
        assert!(state.recovery_requested);

        state.apply_usage_event(usage_event(Some(80.0), reset_at));
        assert!(!state.recovery_requested);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn quota_event_is_pure_and_account_read_branch_has_no_thread_or_local_calls() {
        let source = include_str!("main.rs");
        let usage_definition = source
            .split_once("struct UsageEvent {")
            .and_then(|(_, rest)| rest.split_once("}\n\nenum Event"))
            .map(|(body, _)| body)
            .expect("UsageEvent definition must remain explicit");
        assert!(!usage_definition.contains("ActiveThread"));
        assert!(!usage_definition.contains("ModelUsage"));
        assert!(!usage_definition.contains("model_cost"));

        let account_worker = source
            .split_once("fn account_server_worker")
            .and_then(|(_, rest)| rest.split_once("fn start_app_server"))
            .map(|(body, _)| body)
            .expect("account worker boundary must remain explicit");
        assert!(!account_worker.contains("fetch_active_thread_update"));
        assert!(!account_worker.contains("collect_local_model_usage"));
        assert!(!account_worker.contains("LocalCommand"));

        let thread_worker = source
            .split_once("fn thread_server_worker")
            .and_then(|(_, rest)| rest.split_once("fn local_usage_worker"))
            .map(|(body, _)| body)
            .expect("thread worker boundary must remain explicit");
        assert!(thread_worker.contains("server.take()"));
        assert!(thread_worker.contains("kill_and_reap()"));
        assert!(thread_worker.contains("next_id = 2"));
    }

    #[test]
    fn periodic_ui_timer_only_polls_account_and_threads_not_session_files() {
        let source = include_str!("main.rs");
        let timer = source
            .split_once("let timer = Timer::default();")
            .and_then(|(_, rest)| rest.split_once("    ui.run()?;"))
            .map(|(body, _)| body)
            .expect("account timer boundary must remain explicit");
        assert!(!timer.contains("collect_local_model_usage"));
        assert!(!timer.contains("session_jsonl_files"));
        assert!(timer.contains("poll_service_state"));

        let poll = source
            .split_once("fn poll_service_state")
            .and_then(|(_, rest)| rest.split_once("async fn service_shutdown_signal"))
            .map(|(body, _)| body)
            .expect("shared poll boundary must remain explicit");
        assert!(!poll.contains("collect_local_model_usage"));
        assert!(!poll.contains("session_jsonl_files"));
        assert!(poll.contains("request_thread_update"));
        assert!(poll.contains("account_refresh_due"));
    }

    #[test]
    fn thread_failure_preserves_quota_plan_reset_and_history() {
        let mut state = CodexInfoState::preview("normal");
        let previous_threads = state.active_threads.clone();
        let reset_at = state.reset_at.expect("preview reset");
        state.apply_usage_event(usage_event(Some(23.0), reset_at));
        let plan = state.plan_label.clone();
        let history = state.history.samples.clone();
        state.thread_checking = true;
        state.apply_thread_error(state.auth_epoch, "thread failure".into());

        assert_eq!(state.remaining_percent, Some(23.0));
        assert_eq!(state.reset_at, Some(reset_at));
        assert_eq!(state.plan_label, plan);
        assert_eq!(state.history.samples, history);
        assert!(state.thread_error);
        assert!(!state.thread_checking);
        assert_eq!(state.active_threads, previous_threads);
        assert_eq!(
            state.status,
            "利用枠は更新しました。スレッド情報の取得に失敗し、実行中の状態は未確認です。"
        );
    }

    #[test]
    fn thread_failure_recovery_requires_a_new_complete_snapshot() {
        let mut state = CodexInfoState::preview("normal");
        let old = active_thread_fixture(0, 100);
        state.active_threads = vec![old.clone()];
        state.thread_checking = true;

        // A failed cycle keeps the previous complete row visible.
        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::Failed);
        assert_eq!(state.active_threads, [old]);
        assert!(state.thread_error);
        assert!(!state.thread_checking);

        // Recovery is only allowed through a subsequent complete snapshot.
        let replacement = active_thread_fixture(1, 120);
        state.apply_thread_result(
            state.auth_epoch,
            ActiveThreadUpdate::Snapshot(vec![replacement.clone()]),
        );
        assert_eq!(state.active_threads, [replacement]);
        assert!(!state.thread_error);
        assert!(!state.thread_checking);
    }

    #[test]
    fn local_failure_preserves_same_period_quota_and_history() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview reset");
        state.apply_usage_event(usage_event(Some(23.0), reset_at));
        let plan = state.plan_label.clone();
        let history = state.history.samples.clone();
        let model_usage = state.model_usage.clone();
        let cost = state.estimated_cost_label.clone();
        state.apply_local_usage_error(state.auth_epoch, reset_at, WEEK_SECONDS);

        assert_eq!(state.remaining_percent, Some(23.0));
        assert_eq!(state.reset_at, Some(reset_at));
        assert_eq!(state.plan_label, plan);
        assert_eq!(state.history.samples, history);
        assert_eq!(state.model_usage, model_usage);
        assert_eq!(state.estimated_cost_label, cost);
        assert!(state.local_usage_error);
    }

    #[test]
    fn stale_thread_and_local_results_are_complete_no_ops() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview reset");
        let old_threads = state.active_threads.clone();
        let old_history = state.history.samples.clone();
        let old_usage = state.model_usage.clone();
        let old_cost = state.estimated_cost_label.clone();
        let old_status = state.status.clone();
        state.auth_epoch = 9;
        state.thread_checking = true;
        state.thread_error = false;
        state.local_usage_error = false;

        state.apply_thread_result(
            8,
            ActiveThreadUpdate::Snapshot(vec![ActiveThread {
                id: "stale".into(),
                ..ActiveThread::default()
            }]),
        );
        state.apply_thread_result(8, ActiveThreadUpdate::Failed);
        state.apply_thread_error(8, "stale thread error".into());
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: 8,
            reset_at,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![UsageHistorySample::new(
                10,
                reset_at,
                1.0,
                ModelDollarTotals::default(),
            )],
        });
        state.apply_local_usage_error(8, reset_at, WEEK_SECONDS);

        assert_eq!(state.active_threads, old_threads);
        assert_eq!(state.history.samples, old_history);
        assert_eq!(state.model_usage, old_usage);
        assert_eq!(state.estimated_cost_label, old_cost);
        assert_eq!(state.status, old_status);
        assert!(state.thread_checking);
        assert!(!state.thread_error);
        assert!(!state.local_usage_error);
    }

    #[test]
    fn stale_local_result_from_old_period_is_a_no_op() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview reset");
        let old_history = state.history.samples.clone();
        let old_usage = state.model_usage.clone();
        let old_cost = state.estimated_cost_label.clone();
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: state.auth_epoch,
            reset_at: reset_at + WEEK_SECONDS,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![UsageHistorySample::new(
                10,
                reset_at + WEEK_SECONDS,
                0.0,
                ModelDollarTotals::default(),
            )],
        });
        state.apply_local_usage_error(state.auth_epoch, reset_at + WEEK_SECONDS, WEEK_SECONDS);
        assert_eq!(state.history.samples, old_history);
        assert_eq!(state.model_usage, old_usage);
        assert_eq!(state.estimated_cost_label, old_cost);
        assert!(!state.local_usage_error);
    }

    #[test]
    fn local_success_is_the_only_path_that_commits_usage_and_history() {
        let mut state = CodexInfoState::preview("normal");
        let reset_at = state.reset_at.expect("preview reset");
        let mut totals = ModelUsageTotals::default();
        totals.add(
            "gpt-5.6-sol",
            TokenSnapshot {
                total: 12,
                input: 8,
                cached_input: 2,
                output: 4,
            },
        );
        state.apply_usage_event(usage_event(Some(23.0), reset_at));
        // A quota response alone is not a graph-ready snapshot.  The REST
        // pair and native view must remain loading until local usage commits.
        state.local_usage_pending = true;
        let pending = state.public_snapshot();
        assert_eq!(pending.state, PublicState::Initializing);
        assert!(pending.models.is_empty());
        assert_eq!(pending.active_thread_count, 0);
        assert!(state.public_details().history_periods.is_empty());

        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: state.auth_epoch,
            reset_at,
            window_seconds: WEEK_SECONDS,
            model_usage: totals,
            history_samples: vec![UsageHistorySample::new_with_usage(
                20,
                reset_at,
                23.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            )],
        });

        assert_eq!(state.model_usage.len(), 1);
        assert_eq!(state.model_usage[0].name, "SOL");
        assert!(state
            .history
            .samples
            .iter()
            .any(|sample| sample.remaining_percent == 23.0));
        assert!(!state.local_usage_pending);
        assert_eq!(state.public_snapshot().state, PublicState::Ready);
        assert!(!state.local_usage_error);
    }

    #[test]
    fn quota_reset_moves_an_auto_selected_graph_to_the_new_period() {
        let mut state = CodexInfoState::preview("normal");
        let previous_reset = state.reset_at.expect("preview reset");
        state.select_latest_history();
        assert_eq!(state.selected_reset_at, Some(previous_reset));

        let next_reset = previous_reset + WEEK_SECONDS;
        state.apply_usage_event(usage_event(Some(22.0), next_reset));
        assert_eq!(state.selected_reset_at, Some(next_reset));
        assert!(state.selected_history_period.is_empty());

        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: state.auth_epoch,
            reset_at: next_reset,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![UsageHistorySample::new(
                Utc::now().timestamp(),
                next_reset,
                22.0,
                ModelDollarTotals::default(),
            )],
        });
        assert_eq!(state.selected_history_reset(), Some(next_reset));
    }

    #[test]
    fn quota_reset_jitter_within_authority_moves_selection_to_new_period() {
        let offsets = [-60_i64, -1, 0, 1, 60];
        let mut failures = Vec::new();

        for offset in offsets {
            let mut state = CodexInfoState::preview("normal");
            let previous_reset = state.reset_at.expect("preview reset");
            let next_reset = previous_reset + WEEK_SECONDS;
            state.selected_reset_at = Some(previous_reset + offset);
            state.selected_history_period = "stale selected period".into();

            state.apply_usage_event(usage_event(Some(99.0), next_reset));

            if state.selected_reset_at != Some(next_reset)
                || !state.selected_history_period.is_empty()
            {
                failures.push(offset);
            }
        }

        assert!(
            failures.is_empty(),
            "within-authority offsets must follow the new period; exact-equality RED failures: {failures:?}"
        );
    }

    #[test]
    fn explicit_historical_selection_outside_authority_stays_selected_after_reset() {
        let previous_reset = CodexInfoState::preview("normal")
            .reset_at
            .expect("preview reset");
        let next_reset = previous_reset + WEEK_SECONDS;

        for selected_reset in [previous_reset - 61, previous_reset - WEEK_SECONDS] {
            let mut state = CodexInfoState::preview("normal");
            state.selected_reset_at = Some(selected_reset);
            state.selected_history_period = "explicit historical period".into();

            state.apply_usage_event(usage_event(Some(99.0), next_reset));

            assert_eq!(state.selected_reset_at, Some(selected_reset));
            assert_eq!(state.selected_history_period, "explicit historical period");
        }
    }

    #[test]
    fn rolling_reset_drift_without_boundary_keeps_selection_and_label() {
        let mut state = CodexInfoState::preview("normal");
        let previous_reset = state.reset_at.expect("preview reset");
        state.selected_reset_at = Some(previous_reset);
        state.selected_history_period = "explicit current period".into();

        state.apply_usage_event(usage_event(Some(13.0), previous_reset + 120));

        assert_eq!(state.selected_reset_at, Some(previous_reset));
        assert_eq!(state.selected_history_period, "explicit current period");
    }

    #[test]
    fn jittered_rollover_commits_new_period_for_label_graph_and_samples() {
        const PREVIOUS_RESET: i64 = 2_000_000_000;
        const NEXT_RESET: i64 = PREVIOUS_RESET + WEEK_SECONDS;
        const OBSERVED_AT: i64 = PREVIOUS_RESET + 86_400;
        const OLD_SAMPLE_AT: i64 = PREVIOUS_RESET - 3_600;
        const NEW_SAMPLE_AT_1: i64 = PREVIOUS_RESET + 3_600;
        const NEW_SAMPLE_AT_2: i64 = PREVIOUS_RESET + 7_200;

        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(PREVIOUS_RESET);
        state.remaining_percent = Some(20.0);
        state.last_success_at = Some(PREVIOUS_RESET - 60);
        state.selected_reset_at = Some(PREVIOUS_RESET + 1);
        state.selected_history_period = "stale selected period".into();
        state.history = UsageHistory {
            db_path: None,
            samples: vec![UsageHistorySample::new(
                OLD_SAMPLE_AT,
                PREVIOUS_RESET,
                20.0,
                ModelDollarTotals {
                    sol: 0.25,
                    terra: 0.5,
                    luna: 0.75,
                },
            )],
            startup_maintenance_done: true,
        };

        state.apply_usage_event(usage_event(Some(80.0), NEXT_RESET));
        assert_eq!(state.selected_reset_at, Some(NEXT_RESET));
        assert!(state.selected_history_period.is_empty());

        // Admit a fixed, matching local payload through the production commit
        // path. The preview state is switched off only to exercise backfill
        // admission; its database path is absent, so this remains read-only.
        state.preview = false;
        state.last_success_at = Some(OBSERVED_AT);
        // Keep the fixed backfill rows as the sole admitted graph payload;
        // the live commit would otherwise append a wall-clock sample and
        // move the bounded acquisition window away from this deterministic
        // fixture.
        state.remaining_percent = None;
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: state.auth_epoch,
            reset_at: NEXT_RESET,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![
                UsageHistorySample::new(
                    NEW_SAMPLE_AT_1,
                    NEXT_RESET,
                    80.0,
                    ModelDollarTotals {
                        sol: 1.0,
                        terra: 2.0,
                        luna: 3.0,
                    },
                ),
                UsageHistorySample::new(
                    NEW_SAMPLE_AT_2,
                    NEXT_RESET,
                    79.0,
                    ModelDollarTotals {
                        sol: 2.0,
                        terra: 4.0,
                        luna: 6.0,
                    },
                ),
            ],
        });
        state.remaining_percent = Some(80.0);

        let periods = state.history_periods_at(OBSERVED_AT);
        let selected_period = periods
            .iter()
            .find(|period| period.canonical_reset_at == NEXT_RESET)
            .expect("new period is present");
        let old_period = periods
            .iter()
            .find(|period| period.canonical_reset_at == PREVIOUS_RESET)
            .expect("old period is present");
        assert_eq!(selected_period.start, PREVIOUS_RESET.div_euclid(60) * 60);
        assert_eq!(selected_period.end, OBSERVED_AT);
        assert_ne!(selected_period.label, old_period.label);
        assert_eq!(
            state.selected_history_reset_for_periods(&periods),
            Some(NEXT_RESET)
        );

        let selected_samples = state.history.samples_for_reset(Some(NEXT_RESET));
        assert_eq!(selected_samples.len(), 2);
        assert!(selected_samples.iter().all(|sample| {
            sample.reset_at == NEXT_RESET
                && sample.timestamp >= PREVIOUS_RESET
                && sample.timestamp <= OBSERVED_AT
        }));
        assert!(selected_samples
            .iter()
            .all(|sample| sample.timestamp != OLD_SAMPLE_AT));

        let details = state.public_details_at(OBSERVED_AT);
        let public_period = details
            .history_periods
            .iter()
            .find(|period| period.reset_at == NEXT_RESET)
            .expect("new public period is present");
        assert_eq!(public_period.label, selected_period.label);
        assert_eq!(public_period.start_at, PREVIOUS_RESET.div_euclid(60) * 60);
        assert_eq!(public_period.end_at, OBSERVED_AT);

        let paths = state.graph_paths_for_selection_at(OBSERVED_AT, true, true, true, false);
        assert!(paths.remaining.starts_with("M0.00 "));
        assert!(paths.remaining.contains("L100.00 "));
        assert!(paths.sol.starts_with("M0.00 "));
        assert!(paths.sol.contains("L100.00 "));
    }

    #[test]
    fn clearing_or_changing_authentication_advances_epoch() {
        let mut state = CodexInfoState::preview("normal");
        let initial = state.auth_epoch;
        state.clear_account_visible_state();
        assert_eq!(state.auth_epoch, initial + 1);
        state.clear_account_visible_state();
        assert_eq!(state.auth_epoch, initial + 2);
    }

    #[test]
    fn account_error_does_not_clear_thread_failure_state() {
        let mut state = CodexInfoState::preview("normal");
        state.thread_checking = true;
        state.thread_error = true;
        state.apply_account_error("account failure".into());
        assert!(!state.thread_checking);
        assert!(state.thread_error);

        let account_status = state.status.clone();
        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::NoThread);
        assert!(state.account_error.is_some());
        assert!(state.error.is_some());
        assert_eq!(state.status, account_status);
    }

    #[test]
    fn native_startup_loading_requires_a_complete_authenticated_generation() {
        assert!(!native_startup_loading(false, false, false, false, false));
        assert!(native_startup_loading(true, false, false, false, false));
        assert!(!native_startup_loading(true, true, false, false, false));
        assert!(!native_startup_loading(true, false, true, false, false));
        assert!(!native_startup_loading(true, false, false, true, false));
        assert!(!native_startup_loading(true, false, false, false, true));
    }

    #[test]
    fn native_startup_failure_releases_loading_surface() {
        let mut state = CodexInfoState::preview("startup-loading");
        let reset_at = state.reset_at.expect("startup preview reset");
        state.apply_local_usage_error(state.auth_epoch, reset_at, state.window_seconds);
        assert!(state.local_usage_error);
        assert!(state.error.is_some());
        assert!(!native_startup_loading(
            state.authenticated,
            state.has_visible_usage(),
            state.local_usage_error,
            state.account_error.is_some(),
            state.error.is_some(),
        ));
        assert_eq!(state.public_snapshot().state, PublicState::Error);
    }

    #[test]
    fn account_error_fences_later_events_from_the_failed_bridge_batch() {
        let mut state = CodexInfoState::preview("normal");
        let previous_remaining = state.remaining_percent;
        let previous_reset = state.reset_at;
        let previous_history = state.history.samples.clone();
        let previous_threads = state.active_threads.clone();

        let restart = state.apply_account_event_batch(vec![
            Event::Error("failed bridge".into()),
            Event::Usage(Box::new(usage_event(Some(100.0), 9_999_999_999))),
        ]);

        assert!(restart);
        assert_eq!(state.remaining_percent, previous_remaining);
        assert_eq!(state.reset_at, previous_reset);
        assert_eq!(state.history.samples, previous_history);
        assert_eq!(state.active_threads, previous_threads);
        assert!(state.account_error.is_some());

        let mut ordered = CodexInfoState::preview("normal");
        let reset_at = ordered.reset_at.expect("preview reset");
        let restart = ordered.apply_account_event_batch(vec![
            Event::Usage(Box::new(usage_event(Some(33.0), reset_at))),
            Event::Error("later failure".into()),
        ]);
        assert!(restart);
        assert_eq!(ordered.remaining_percent, Some(33.0));
        assert!(ordered.account_error.is_some());
    }

    #[test]
    fn account_error_fences_queued_thread_and_local_results_without_clearing_last_valid_values() {
        let mut state = CodexInfoState::preview("normal");
        let stale_epoch = state.auth_epoch;
        let reset_at = state.reset_at.expect("preview reset");
        let remaining = state.remaining_percent;
        let plan = state.plan_label.clone();
        let history = state.history.samples.clone();
        let model_usage = state.model_usage.clone();
        let cost = state.estimated_cost_label.clone();
        let threads = state.active_threads.clone();
        state.thread_checking = true;

        state.apply_account_error("failed account bridge".into());
        let error_status = state.status.clone();

        assert_eq!(state.auth_epoch, stale_epoch + 1);
        assert!(!state.thread_checking);
        assert_eq!(state.remaining_percent, remaining);
        assert_eq!(state.plan_label, plan);
        assert_eq!(state.history.samples, history);
        assert_eq!(state.model_usage, model_usage);
        assert_eq!(state.estimated_cost_label, cost);
        assert_eq!(state.active_threads, threads);

        state.apply_thread_result(
            stale_epoch,
            ActiveThreadUpdate::Snapshot(vec![ActiveThread {
                id: "stale-thread".into(),
                ..ActiveThread::default()
            }]),
        );
        state.apply_thread_error(stale_epoch, "stale thread error".into());
        state.apply_local_usage_success(LocalUsageResult {
            auth_epoch: stale_epoch,
            reset_at,
            window_seconds: WEEK_SECONDS,
            model_usage: ModelUsageTotals::default(),
            history_samples: vec![UsageHistorySample::new(
                10,
                reset_at,
                0.0,
                ModelDollarTotals::default(),
            )],
        });
        state.apply_local_usage_error(stale_epoch, reset_at, WEEK_SECONDS);

        assert_eq!(state.remaining_percent, remaining);
        assert_eq!(state.plan_label, plan);
        assert_eq!(state.history.samples, history);
        assert_eq!(state.model_usage, model_usage);
        assert_eq!(state.estimated_cost_label, cost);
        assert_eq!(state.active_threads, threads);
        assert_eq!(state.status, error_status);
        assert!(state.account_error.is_some());
        assert!(!state.thread_error);
        assert!(!state.local_usage_error);
    }

    #[test]
    fn initial_authenticated_event_preserves_loaded_history_and_advances_epoch() {
        let mut state = CodexInfoState::preview("normal");
        let expected_history = state.history.samples.clone();
        assert!(!expected_history.is_empty());
        state.authenticated = false;
        state.email = None;
        state.plan_label.clear();
        let old_epoch = state.auth_epoch;

        state.apply_account_event(Some("preview@example.com".into()), true, Some("pro".into()));

        assert_eq!(state.auth_epoch, old_epoch + 1);
        assert_eq!(state.history.samples, expected_history);
        assert!(state.authenticated);
    }

    #[test]
    fn account_loss_or_switch_clears_every_visible_account_value() {
        let mut state = CodexInfoState::preview("normal");
        state.clear_account_visible_state();

        assert!(!state.authenticated);
        assert!(state.email.is_none());
        assert!(state.plan_label.is_empty());
        assert!(!state.has_usage);
        assert!(!state.has_quota_percent);
        assert!(state.remaining_percent.is_none());
        assert!(state.reset_at.is_none());
        assert!(state.model_usage.is_empty());
        assert!(state.active_threads.is_empty());
        assert_eq!(state.estimated_cost_label, "概算 —");
        assert!(state.history.samples.is_empty());
        assert_eq!(state.selected_history_period, "履歴なし");
    }

    #[test]
    fn window_titles_use_only_validated_identity_for_every_state() {
        for preview in [
            "normal",
            "warning",
            "reset-warning",
            "error",
            "zero",
            "full",
            "idle",
            "multi-thread",
            "single-thread",
            "history-empty",
        ] {
            assert_eq!(
                CodexInfoState::preview(preview).window_title(),
                "preview@example.com — Pro",
                "preview state: {preview}"
            );
        }
        for preview in ["monthly", "unlimited"] {
            assert_eq!(
                CodexInfoState::preview(preview).window_title(),
                "preview@example.com — エンタープライズ",
                "preview state: {preview}"
            );
        }
        assert_eq!(
            CodexInfoState::preview("auth").window_title(),
            UNAUTHENTICATED_WINDOW_TITLE
        );
        assert_eq!(
            account_window_title(false, Some("stale@example.com"), "Pro"),
            UNAUTHENTICATED_WINDOW_TITLE
        );
        assert_eq!(
            account_window_title(true, Some("user@example.com"), "プラン未設定"),
            "user@example.com — プラン未設定"
        );
        assert_eq!(
            detail_window_title("user@example.com — Pro", THREADS_WINDOW_PURPOSE),
            "user@example.com — Pro — Threads"
        );
        assert_eq!(
            detail_window_title("user@example.com — Pro", GRAPH_WINDOW_PURPOSE),
            "user@example.com — Pro — Graph"
        );
        assert_eq!(
            detail_window_title(UNAUTHENTICATED_WINDOW_TITLE, THREADS_WINDOW_PURPOSE),
            UNAUTHENTICATED_WINDOW_TITLE
        );
    }

    #[test]
    fn native_title_bars_are_ascii_safe_and_keep_move_context() {
        assert_eq!(
            native_account_window_title("salty919@gmail.com — Pro Lite"),
            "salty919@gmail.com - Pro Lite"
        );
        assert_eq!(
            native_account_window_title("salty919@gmail.com — エンタープライズ"),
            "salty919@gmail.com - Plan"
        );
        assert_eq!(
            super::native_detail_window_title(
                &super::I18n::detect(),
                true,
                "salty919@gmail.com — Pro Lite",
                super::WindowPurpose::Threads,
            ),
            "salty919@gmail.com - Pro Lite - Threads"
        );
        assert!(super::native_detail_window_title(
            &super::I18n::detect(),
            true,
            "salty919@gmail.com — エンタープライズ",
            super::WindowPurpose::Graph,
        )
        .is_ascii());
    }

    #[test]
    fn window_title_email_is_one_line_bounded_and_control_free() {
        assert_eq!(
            account_window_title(true, Some("a\n\tb"), "Pro"),
            "a b — Pro"
        );
        assert_eq!(
            account_window_title(true, Some("a   b"), "Pro"),
            "a b — Pro"
        );
        for forbidden in [
            '\u{0000}', '\u{001f}', '\u{007f}', '\u{009f}', '\u{061c}', '\u{200e}', '\u{200f}',
            '\u{2028}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            let email = format!("a{forbidden}{forbidden}b");
            assert_eq!(
                account_window_title(true, Some(&email), "Pro"),
                "a b — Pro",
                "forbidden scalar U+{:04X}",
                forbidden as u32
            );
        }
        let email_254 = "x".repeat(254);
        assert_eq!(
            account_window_title(true, Some(&email_254), "Pro"),
            format!("{email_254} — Pro")
        );
        let email_255 = "x".repeat(255);
        assert_eq!(
            account_window_title(true, Some(&email_255), "Pro"),
            format!("{}… — Pro", "x".repeat(253))
        );
        assert_eq!(
            account_window_title(true, Some("   "), "Pro"),
            UNAUTHENTICATED_WINDOW_TITLE
        );
        assert_eq!(
            account_window_title(true, Some("user@example.com"), &"p".repeat(65)),
            "user@example.com — プラン未設定"
        );
    }

    #[test]
    fn window_title_retains_valid_identity_on_refresh_error_and_clears_on_switch() {
        let mut state = CodexInfoState::preview("normal");
        state.email = Some("a@example.com".into());
        assert_eq!(state.window_title(), "a@example.com — Pro");
        state.apply_account_error("refresh failed".into());
        assert_eq!(state.window_title(), "a@example.com — Pro");

        state.clear_account_visible_state();
        assert_eq!(state.window_title(), UNAUTHENTICATED_WINDOW_TITLE);
        state.apply_account_event(Some("b@example.com".into()), true, Some("plus".into()));
        assert_eq!(state.window_title(), "b@example.com — Plus");
    }

    #[test]
    fn active_thread_rows_preserve_all_threads_and_expose_parent_relationships() {
        let threads = vec![
            ActiveThread {
                id: "parent".into(),
                created_at: Some(10),
                updated_at: 20,
                title: "親タイトル".into(),
                model: "model-parent".into(),
                model_label: "model-parent".into(),
                total_tokens: Some(u64::MAX),
                context_usage_tokens: Some(225_000),
                context_window_tokens: Some(258_400),
                last_user_message_at: Some(19),
                is_subagent: false,
                parent_thread_id: None,
                depth: None,
            },
            ActiveThread {
                id: "child".into(),
                created_at: Some(10),
                updated_at: 19,
                title: "子タイトル".into(),
                model: "model-child".into(),
                model_label: "model-child".into(),
                total_tokens: Some(1_234),
                context_usage_tokens: None,
                context_window_tokens: None,
                last_user_message_at: Some(18),
                is_subagent: true,
                parent_thread_id: Some("parent".into()),
                depth: Some(1),
            },
            ActiveThread {
                id: "orphan".into(),
                created_at: None,
                updated_at: 18,
                title: "親が完了済みの子".into(),
                model: "model-orphan".into(),
                model_label: "model-orphan".into(),
                total_tokens: None,
                context_usage_tokens: None,
                context_window_tokens: None,
                last_user_message_at: None,
                is_subagent: true,
                parent_thread_id: Some("completed-parent".into()),
                depth: Some(120),
            },
        ];

        let rows = active_thread_rows_at(&threads, 20);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].relation.as_str(), "メイン");
        assert_eq!(
            rows[0].tokens.as_str(),
            "18,446,744,073,709,551,615トークン"
        );
        assert_eq!(rows[0].context_usage.as_str(), "87.1% / 258,400トークン");
        assert_eq!(rows[1].relation.as_str(), "サブ D1");
        assert_eq!(rows[1].context_usage.as_str(), "—");
        assert_eq!(rows[1].tree_depth, 1);
        assert!(rows[1].connected_to_parent);
        assert!(!rows[1].has_next_sibling);
        assert_eq!(rows[1].parent_title.as_str(), "親: 親タイトル");
        assert_eq!(rows[1].thread_age.as_str(), "10秒");
        assert_eq!(rows[1].instruction_age.as_str(), "2秒");
        assert_eq!(rows[2].relation.as_str(), "サブ D99+");
        assert_eq!(rows[2].tree_depth, 0);
        assert!(!rows[2].connected_to_parent);
        assert_eq!(rows[2].parent_title.as_str(), "親スレッドは現在非実行");
        assert_eq!(rows[2].tokens.as_str(), "—");
    }

    #[test]
    fn thread_presentation_is_parent_first_subtree_contiguous_and_total() {
        let thread = |id: &str,
                      updated_at: i64,
                      is_subagent: bool,
                      parent: Option<&str>,
                      depth: Option<i32>| ActiveThread {
            id: id.into(),
            created_at: Some(updated_at.saturating_sub(10)),
            updated_at,
            title: id.into(),
            model: "model".into(),
            model_label: "model".into(),
            total_tokens: None,
            context_usage_tokens: None,
            context_window_tokens: None,
            last_user_message_at: Some(updated_at.saturating_sub(1)),
            is_subagent,
            parent_thread_id: parent.map(str::to_owned),
            depth,
        };
        let threads = vec![
            thread("grand", 60, true, Some("child-new"), None),
            thread("orphan", 30, true, Some("missing"), Some(7)),
            thread("cycle-b", 90, true, Some("cycle-a"), Some(1)),
            thread("z-child", 70, true, Some("root-z"), Some(99)),
            thread("root-a", 10, false, None, Some(8)),
            thread("sibling", 40, true, Some("root-a"), Some(-4)),
            thread("parentless", 25, true, None, None),
            thread("cycle-a", 100, true, Some("cycle-b"), Some(1)),
            thread("child-new", 50, true, Some("root-a"), Some(42)),
            thread("root-z", 20, false, None, None),
        ];

        let presentation = thread_presentation_rows(&threads);
        let ids = presentation
            .iter()
            .map(|row| threads[row.index].id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "orphan",
                "parentless",
                "root-z",
                "z-child",
                "root-a",
                "child-new",
                "grand",
                "sibling",
                "cycle-a",
                "cycle-b",
            ]
        );
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<_>>(),
            threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<BTreeSet<_>>()
        );

        let child = presentation
            .iter()
            .find(|row| threads[row.index].id == "child-new")
            .unwrap();
        assert_eq!(child.forest_depth, 1);
        assert!(child.connected_to_parent);
        assert!(child.has_children);
        assert!(child.has_next_sibling);
        assert_eq!(child.ancestor_guides, [false; 3]);
        let grand = presentation
            .iter()
            .find(|row| threads[row.index].id == "grand")
            .unwrap();
        assert_eq!(grand.forest_depth, 2);
        assert_eq!(grand.ancestor_guides, [true, false, false]);
        let orphan = presentation
            .iter()
            .find(|row| threads[row.index].id == "orphan")
            .unwrap();
        assert_eq!(orphan.forest_depth, 0);
        assert!(!orphan.connected_to_parent);
        let cycle = presentation
            .iter()
            .find(|row| threads[row.index].id == "cycle-a")
            .unwrap();
        assert!(!cycle.connected_to_parent);
        assert!(!cycle.has_children);

        let rows = active_thread_rows_at(&threads, 100);
        let rows_by_title = rows
            .iter()
            .map(|row| (row.title.as_str(), row))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(rows_by_title["child-new"].relation.as_str(), "サブ D1");
        assert_eq!(rows_by_title["grand"].relation.as_str(), "サブ D2");
        assert_eq!(rows_by_title["sibling"].relation.as_str(), "サブ D1");
        assert_eq!(rows_by_title["orphan"].relation.as_str(), "サブ D7");
        assert_eq!(rows_by_title["parentless"].relation.as_str(), "サブ");
        assert_eq!(rows_by_title["root-a"].relation.as_str(), "メイン");
    }

    #[test]
    fn thread_presentation_keeps_capped_ancestor_guide_through_deeper_rows() {
        let thread = |id: &str, updated_at: i64, parent: Option<&str>| ActiveThread {
            id: id.into(),
            created_at: Some(updated_at.saturating_sub(10)),
            updated_at,
            title: id.into(),
            model: "model".into(),
            model_label: "model".into(),
            total_tokens: None,
            context_usage_tokens: None,
            context_window_tokens: None,
            last_user_message_at: None,
            is_subagent: parent.is_some(),
            parent_thread_id: parent.map(str::to_owned),
            depth: None,
        };
        let threads = vec![
            thread("root", 100, None),
            thread("level-1", 90, Some("root")),
            thread("level-2", 80, Some("level-1")),
            thread("level-3-first", 70, Some("level-2")),
            thread("level-3-last", 60, Some("level-2")),
            thread("level-4", 50, Some("level-3-first")),
            thread("level-5", 40, Some("level-4")),
        ];

        let presentation = thread_presentation_rows(&threads);
        let level_3_first = presentation
            .iter()
            .find(|row| threads[row.index].id == "level-3-first")
            .expect("level 3 first sibling");
        assert!(level_3_first.has_next_sibling);
        let level_5 = presentation
            .iter()
            .find(|row| threads[row.index].id == "level-5")
            .expect("deep descendant");
        assert_eq!(level_5.forest_depth, 5);
        assert!(level_5.ancestor_guides[2]);
    }

    #[test]
    fn active_thread_model_counts_use_exact_known_tokens_and_keep_named_zeroes() {
        let thread = |id: &str, model_label: &str| ActiveThread {
            id: id.into(),
            created_at: Some(1),
            updated_at: 1,
            title: id.into(),
            model: "model".into(),
            model_label: model_label.into(),
            total_tokens: None,
            context_usage_tokens: None,
            context_window_tokens: None,
            last_user_message_at: None,
            is_subagent: false,
            parent_thread_id: None,
            depth: None,
        };

        assert_eq!(active_thread_model_counts(&[]), "");
        assert_eq!(
            active_thread_model_counts(&[
                thread("sol", "gpt-5.6-SOL"),
                thread("terra", "gpt-5.6-terra"),
                thread("luna", "gpt-5.6-luna"),
                thread("unknown", "gpt-5.6-sol-terra"),
            ]),
            "SOL 1  TERRA 1  LUNA 1  その他 1"
        );
    }

    #[test]
    fn thread_age_uses_fixed_boundaries_and_clamps_future() {
        let now = 86_400;
        assert_eq!(format_elapsed(now, Some(now)), "0秒");
        assert_eq!(format_elapsed(now, Some(now - 59)), "59秒");
        assert_eq!(format_elapsed(now, Some(now - 60)), "1分");
        assert_eq!(format_elapsed(now, Some(now - 83)), "1分23秒");
        assert_eq!(format_elapsed(now, Some(now - 3_599)), "59分59秒");
        assert_eq!(format_elapsed(now, Some(now - 3_600)), "1時間");
        assert_eq!(format_elapsed(now, Some(now - 3_661)), "1時間1分");
        assert_eq!(format_elapsed(now, Some(now - 86_399)), "23時間59分");
        assert_eq!(format_elapsed(now, Some(now - 86_400)), "1日");
        assert_eq!(format_elapsed(now, Some(now + 60)), "0秒");
        assert_eq!(format_elapsed(now, Some(i64::MAX)), "—");
        assert_eq!(format_elapsed(now, None), "—");
    }

    #[cfg(unix)]
    #[test]
    fn open_codex_session_paths_accepts_only_bounded_codex_fds_under_sessions_root() {
        use std::os::unix::fs::symlink;

        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-proc-fixture-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        let sessions = root.join("sessions");
        let proc_root = root.join("proc");
        let process = proc_root.join("100");
        let ignored_process = proc_root.join("200");
        fs::create_dir_all(process.join("fd")).unwrap();
        fs::create_dir_all(ignored_process.join("fd")).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        fs::write(process.join("comm"), "codex\n").unwrap();
        fs::write(ignored_process.join("comm"), "not-codex\n").unwrap();
        let executable = root.join("codex");
        fs::write(&executable, "fixture").unwrap();
        symlink(&executable, process.join("exe")).unwrap();
        symlink(&executable, ignored_process.join("exe")).unwrap();
        let active = sessions.join("active.jsonl");
        let ignored = sessions.join("ignored.jsonl");
        let outside = root.join("outside.jsonl");
        fs::write(&active, "{}\n").unwrap();
        fs::write(&ignored, "{}\n").unwrap();
        fs::write(&outside, "{}\n").unwrap();
        symlink(&active, process.join("fd/3")).unwrap();
        symlink(&outside, process.join("fd/4")).unwrap();
        symlink(&ignored, ignored_process.join("fd/3")).unwrap();

        assert_eq!(
            open_codex_session_paths(&proc_root, &sessions).unwrap(),
            BTreeSet::from([fs::canonicalize(active).unwrap()])
        );
        let _ = fs::remove_dir_all(root);
    }

    fn thread_list_item(id: &str, updated_at: i64, path: &Path) -> Value {
        json!({
            "cliVersion": "0.147.0",
            "createdAt": 1,
            "cwd": "/tmp/codex-info",
            "ephemeral": false,
            "id": id,
            "modelProvider": "openai",
            "preview": format!("preview-{id}"),
            "sessionId": format!("session-{id}"),
            "source": "cli",
            "status": {"type": "idle"},
            "turns": [],
            "updatedAt": updated_at,
            "name": format!("title-{id}"),
            "path": path.to_string_lossy()
        })
    }

    #[test]
    fn active_thread_adapter_rejects_partial_rollout_fallback() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-thread-adapter-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let newest_path = root.join("newest.jsonl");
        let fallback_path = root.join("fallback.jsonl");
        fs::write(
            &newest_path,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"}}\n",
        )
        .unwrap();
        fs::write(
            &fallback_path,
            [
                json!({"type":"event_msg","payload":{"type":"task_started"}}),
                json!({"type":"event_msg","payload":{"type":"turn_context","model":"gpt-5.6-sol"}}),
                json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":12345}}}}),
            ]
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n",
        )
        .unwrap();

        let (sender, receiver) = mpsc::channel();
        for response in [
            json!({
                "id": 50,
                "result": {
                    "data": [thread_list_item("newest", 20, &newest_path)],
                    "nextCursor": "page-2"
                }
            }),
            json!({
                "id": 51,
                "result": {
                    "data": [thread_list_item("fallback", 10, &fallback_path)]
                }
            }),
        ] {
            sender
                .send(RpcReadEvent::Line(
                    super::security::RpcLine::new(response.to_string()).unwrap(),
                ))
                .unwrap();
        }

        let mut input = Vec::new();
        let mut next_id = 50;
        let active_paths = BTreeSet::from([
            fs::canonicalize(&newest_path).unwrap(),
            fs::canonicalize(&fallback_path).unwrap(),
        ]);
        let update = fetch_active_thread_update_for_paths(
            &mut input,
            &receiver,
            &mut next_id,
            &root,
            &active_paths,
        );
        assert_eq!(update, ActiveThreadUpdate::Failed);
        assert_eq!(next_id, 52);

        let requests = String::from_utf8(input)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["id"], 50);
        assert!(requests[0]["params"].get("cursor").is_none());
        assert_eq!(requests[1]["id"], 51);
        assert_eq!(requests[1]["params"]["cursor"], "page-2");
        assert_eq!(
            requests[0]["params"]["sourceKinds"],
            json!([
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn multiple_running_threads_are_all_published_with_stable_order() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-multiple-running-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let completed_path = root.join("completed.jsonl");
        let running_a_path = root.join("running-a.jsonl");
        let running_z_path = root.join("running-z.jsonl");
        fs::write(
            &completed_path,
            [
                json!({"type":"task_started"}),
                json!({"type":"task_complete"}),
            ]
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join("\n")
                + "\n",
        )
        .unwrap();
        for (path, model, total_tokens) in [
            (&running_a_path, "model-a", 111_u64),
            (&running_z_path, "model-z", 999_u64),
        ] {
            fs::write(
                path,
                [
                    json!({"type":"thread_context","model":model}),
                    json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":total_tokens}}}}),
                    json!({"type":"task_started"}),
                ]
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n")
                    + "\n",
            )
            .unwrap();
        }

        let mut child_item = thread_list_item("thread-a", 20, &running_a_path);
        child_item["source"] = json!({"subAgent":{"thread_spawn":{
            "parent_thread_id":"thread-z","depth":1
        }}});
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 70,
                        "result": {
                            "data": [
                                child_item,
                                thread_list_item("completed-newest", 30, &completed_path),
                                thread_list_item("thread-z", 20, &running_z_path)
                            ]
                        }
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();

        let mut input = Vec::new();
        let mut next_id = 70;
        let active_paths = BTreeSet::from([
            fs::canonicalize(&completed_path).unwrap(),
            fs::canonicalize(&running_a_path).unwrap(),
            fs::canonicalize(&running_z_path).unwrap(),
        ]);
        let update = fetch_active_thread_update_for_paths(
            &mut input,
            &receiver,
            &mut next_id,
            &root,
            &active_paths,
        );
        assert_eq!(
            update,
            ActiveThreadUpdate::Snapshot(vec![
                ActiveThread {
                    id: "thread-z".into(),
                    created_at: Some(1),
                    updated_at: 20,
                    title: "title-thread-z".into(),
                    model: "model-z".into(),
                    model_label: "model-z".into(),
                    total_tokens: Some(999),
                    context_usage_tokens: None,
                    context_window_tokens: None,
                    last_user_message_at: None,
                    is_subagent: false,
                    parent_thread_id: None,
                    depth: None,
                },
                ActiveThread {
                    id: "thread-a".into(),
                    created_at: Some(1),
                    updated_at: 20,
                    title: "title-thread-a".into(),
                    model: "model-a".into(),
                    model_label: "model-a".into(),
                    total_tokens: Some(111),
                    context_usage_tokens: None,
                    context_window_tokens: None,
                    last_user_message_at: None,
                    is_subagent: true,
                    parent_thread_id: Some("thread-z".into()),
                    depth: Some(1),
                },
            ])
        );
        assert_eq!(next_id, 71);
        assert_eq!(String::from_utf8(input).unwrap().lines().count(), 1);

        let _ = fs::remove_dir_all(root);
    }

    fn create_native_state_schema(root: &Path) {
        fs::create_dir_all(root.join("sessions")).unwrap();
        let connection = Connection::open(root.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    archived INTEGER NOT NULL,
                    name TEXT,
                    preview TEXT NOT NULL,
                    thread_source TEXT
                );
                CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL PRIMARY KEY,
                    status TEXT NOT NULL
                );",
            )
            .unwrap();
    }

    fn add_native_state_thread(root: &Path, id: &str, rollout_path: &Path) {
        let connection = Connection::open(root.join("state_5.sqlite")).unwrap();
        connection
            .execute(
                "INSERT INTO threads
                 (id, rollout_path, updated_at, archived, name, preview, thread_source)
                 VALUES (?1, ?2, 1, 0, ?3, ?4, 'subagent')",
                rusqlite::params![
                    id,
                    rollout_path.to_string_lossy().as_ref(),
                    format!("title-{id}"),
                    format!("preview-{id}"),
                ],
            )
            .unwrap();
    }

    fn add_native_state_edge(root: &Path, parent: &str, child: &str) {
        let connection = Connection::open(root.join("state_5.sqlite")).unwrap();
        connection
            .execute(
                "INSERT INTO thread_spawn_edges
                 (parent_thread_id, child_thread_id, status) VALUES (?1, ?2, 'active')",
                rusqlite::params![parent, child],
            )
            .unwrap();
    }

    fn write_native_rollout(path: &Path, completed: bool) {
        let mut records = vec![
            json!({"type":"thread_context","model":"native-model"}),
            json!({"type":"task_started"}),
        ];
        if completed {
            records.push(json!({"type":"task_complete"}));
        }
        fs::write(
            path,
            records
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }

    fn write_distinct_running_rollout(
        path: &Path,
        model: &str,
        total_tokens: u64,
        context_usage_tokens: u64,
        context_window_tokens: u64,
        user_timestamp: &str,
        completed: bool,
    ) {
        let mut records = vec![
            json!({"type":"thread_context","model":model}),
            json!({
                "type":"event_msg",
                "timestamp":user_timestamp,
                "payload":{"type":"user_message"}
            }),
            json!({
                "type":"event_msg",
                "payload":{
                    "type":"token_count",
                    "info":{
                        "total_token_usage":{"total_tokens":total_tokens},
                        "last_token_usage":{"total_tokens":context_usage_tokens},
                        "model_context_window":context_window_tokens
                    }
                }
            }),
            json!({"type":"task_started"}),
        ];
        if completed {
            records.push(json!({"type":"task_complete"}));
        }
        fs::write(
            path,
            records
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }

    fn add_native_state_thread_with_values(
        root: &Path,
        id: &str,
        rollout_path: &Path,
        name: &str,
        preview: &str,
        updated_at: i64,
    ) {
        let connection = Connection::open(root.join("state_5.sqlite")).unwrap();
        connection
            .execute(
                "INSERT INTO threads
                 (id, rollout_path, updated_at, archived, name, preview, thread_source)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, 'subagent')",
                rusqlite::params![
                    id,
                    rollout_path.to_string_lossy().as_ref(),
                    updated_at,
                    name,
                    preview,
                ],
            )
            .unwrap();
    }

    #[test]
    fn root_native_duplicate_candidate_is_preserved_then_rejected_atomically() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-root-native-duplicate-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_native_state_schema(&root);
        let sessions = root.join("sessions");
        let owner_rollout = sessions.join("owner.jsonl");
        let root_rollout = sessions.join("root.jsonl");
        let native_rollout = sessions.join("native.jsonl");
        let root_timestamp = "2026-01-01T00:00:42Z";
        let native_timestamp = "2026-01-02T00:00:43Z";
        write_distinct_running_rollout(
            &owner_rollout,
            "owner-model",
            1,
            2,
            3,
            root_timestamp,
            true,
        );
        write_distinct_running_rollout(
            &root_rollout,
            "root-model",
            111,
            222,
            333,
            root_timestamp,
            false,
        );
        write_distinct_running_rollout(
            &native_rollout,
            "native-model",
            999,
            888,
            777,
            native_timestamp,
            false,
        );
        add_native_state_thread_with_values(
            &root,
            "collision",
            &native_rollout,
            "native-title",
            "native-preview",
            1,
        );
        add_native_state_edge(&root, "owner", "collision");

        let (sender, receiver) = mpsc::channel();
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 200,
                        "result": {"data": [
                            thread_list_item("collision", 20, &root_rollout),
                            thread_list_item("owner", 10, &owner_rollout)
                        ]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let active_paths = BTreeSet::from([
            fs::canonicalize(&owner_rollout).unwrap(),
            fs::canonicalize(&root_rollout).unwrap(),
            fs::canonicalize(&native_rollout).unwrap(),
        ]);
        let mut input = Vec::new();
        let mut next_id = 200;
        let update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &sessions,
            &active_paths,
            Some(&root),
        );
        let rows = match update {
            ActiveThreadUpdate::Snapshot(rows) => rows,
            other => panic!("duplicate fixture did not produce a snapshot: {other:?}"),
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "collision");
        assert_eq!(rows[1].id, "collision");
        assert_eq!(
            rows[0],
            ActiveThread {
                id: "collision".into(),
                created_at: Some(1),
                updated_at: 20,
                title: "title-collision".into(),
                model: "root-model".into(),
                model_label: "root-model".into(),
                total_tokens: Some(111),
                context_usage_tokens: Some(222),
                context_window_tokens: Some(333),
                last_user_message_at: Some(
                    chrono::DateTime::parse_from_rfc3339(root_timestamp)
                        .unwrap()
                        .timestamp(),
                ),
                is_subagent: false,
                parent_thread_id: None,
                depth: None,
            }
        );
        assert_eq!(
            rows[1],
            ActiveThread {
                id: "collision".into(),
                created_at: None,
                updated_at: 1,
                title: "native-title".into(),
                model: "native-model".into(),
                model_label: "native-model".into(),
                total_tokens: Some(999),
                context_usage_tokens: Some(888),
                context_window_tokens: Some(777),
                last_user_message_at: Some(
                    chrono::DateTime::parse_from_rfc3339(native_timestamp)
                        .unwrap()
                        .timestamp(),
                ),
                is_subagent: true,
                parent_thread_id: Some("owner".into()),
                depth: Some(1),
            }
        );

        let mut state = CodexInfoState::preview("normal");
        state.history = UsageHistory::default();
        state.active_threads = rows;
        let candidate = state.public_details();
        assert_eq!(candidate.threads.len(), 2);
        assert_eq!(candidate.active_thread_count, 2);

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        let publisher = server.publisher();
        let mut known_good_state = CodexInfoState::preview("normal");
        known_good_state.history = UsageHistory::default();
        publisher
            .publish_details(known_good_state.public_details())
            .unwrap();
        let before_pair = publisher.published_pair();
        let before_status = raw_loopback_get(server.local_addr(), "/v1/status");
        let before_details = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_eq!(
            publisher.publish_details(candidate),
            Err(ApiSnapshotError::InvalidThread)
        );
        assert_eq!(publisher.published_pair(), before_pair);
        assert_eq!(
            raw_loopback_get(server.local_addr(), "/v1/status"),
            before_status
        );
        assert_eq!(
            raw_loopback_get(server.local_addr(), "/v1/details"),
            before_details
        );
        server.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn root_and_native_candidates_follow_fixed_updated_at_then_id_order() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-root-native-order-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_native_state_schema(&root);
        let sessions = root.join("sessions");
        let owner_rollout = sessions.join("owner.jsonl");
        write_distinct_running_rollout(
            &owner_rollout,
            "owner-model",
            1,
            2,
            3,
            "2026-01-01T00:00:01Z",
            true,
        );

        let root_specs = [
            ("root-late", 40_i64, "2026-01-01T00:00:10Z"),
            ("z-root", 20_i64, "2026-01-01T00:00:11Z"),
            ("root-old", 10_i64, "2026-01-01T00:00:12Z"),
        ];
        let mut active_paths = BTreeSet::from([fs::canonicalize(&owner_rollout).unwrap()]);
        let mut root_items = Vec::new();
        for (index, (id, updated_at, timestamp)) in root_specs.into_iter().enumerate() {
            let path = sessions.join(format!("{id}.jsonl"));
            write_distinct_running_rollout(
                &path,
                &format!("{id}-model"),
                100 + index as u64,
                200 + index as u64,
                300 + index as u64,
                timestamp,
                false,
            );
            active_paths.insert(fs::canonicalize(&path).unwrap());
            root_items.push(thread_list_item(id, updated_at, &path));
        }

        let native_specs = [
            ("native-mid", 30_i64, "2026-01-02T00:00:10Z"),
            ("a-native", 20_i64, "2026-01-02T00:00:11Z"),
            ("native-old", 5_i64, "2026-01-02T00:00:12Z"),
        ];
        for (index, (id, updated_at, timestamp)) in native_specs.into_iter().enumerate() {
            let path = sessions.join(format!("{id}.jsonl"));
            write_distinct_running_rollout(
                &path,
                &format!("{id}-model"),
                400 + index as u64,
                500 + index as u64,
                600 + index as u64,
                timestamp,
                false,
            );
            active_paths.insert(fs::canonicalize(&path).unwrap());
            add_native_state_thread_with_values(
                &root,
                id,
                &path,
                &format!("native-title-{id}"),
                &format!("native-preview-{id}"),
                updated_at,
            );
            add_native_state_edge(&root, "owner", id);
        }

        let (sender, receiver) = mpsc::channel();
        root_items.push(thread_list_item("owner", 1, &owner_rollout));
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({"id": 210, "result": {"data": root_items}}).to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let mut input = Vec::new();
        let mut next_id = 210;
        let update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &sessions,
            &active_paths,
            Some(&root),
        );
        let rows = match update {
            ActiveThreadUpdate::Snapshot(rows) => rows,
            other => panic!("order fixture did not produce a snapshot: {other:?}"),
        };
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            [
                "root-late",
                "native-mid",
                "z-root",
                "a-native",
                "root-old",
                "native-old"
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_completed_rollout_is_excluded_from_published_snapshot() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-native-completed-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_native_state_schema(&root);
        let sessions = root.join("sessions");
        let root_rollout = sessions.join("root.jsonl");
        let completed_rollout = sessions.join("completed-child.jsonl");
        write_native_rollout(&root_rollout, false);
        write_native_rollout(&completed_rollout, true);
        add_native_state_thread(&root, "completed-child", &completed_rollout);
        add_native_state_edge(&root, "root", "completed-child");

        let (sender, receiver) = mpsc::channel();
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 80,
                        "result": {"data": [thread_list_item("root", 10, &root_rollout)]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let active_paths = BTreeSet::from([
            fs::canonicalize(&root_rollout).unwrap(),
            fs::canonicalize(&completed_rollout).unwrap(),
        ]);
        let mut input = Vec::new();
        let mut next_id = 80;
        let update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &sessions,
            &active_paths,
            Some(&root),
        );
        assert_eq!(
            update,
            ActiveThreadUpdate::Snapshot(vec![ActiveThread {
                id: "root".into(),
                created_at: Some(1),
                updated_at: 10,
                title: "title-root".into(),
                model: "native-model".into(),
                model_label: "native-model".into(),
                total_tokens: None,
                context_usage_tokens: None,
                context_window_tokens: None,
                last_user_message_at: None,
                is_subagent: false,
                parent_thread_id: None,
                depth: None,
            }])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_stale_running_descendant_not_held_open_is_excluded() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-native-stale-descendant-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_native_state_schema(&root);
        let sessions = root.join("sessions");
        let root_rollout = sessions.join("root.jsonl");
        let stale_child_rollout = sessions.join("stale-child.jsonl");
        write_native_rollout(&root_rollout, false);
        // The child has no terminal event, so rollout parsing alone would
        // call it running. It is deliberately absent from active_paths: the
        // native DB row is historical, not proof of a live app-server handle.
        write_native_rollout(&stale_child_rollout, false);
        add_native_state_thread(&root, "stale-child", &stale_child_rollout);
        add_native_state_edge(&root, "root", "stale-child");

        let (sender, receiver) = mpsc::channel();
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 82,
                        "result": {"data": [thread_list_item("root", 10, &root_rollout)]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let active_paths = BTreeSet::from([fs::canonicalize(&root_rollout).unwrap()]);
        let mut input = Vec::new();
        let mut next_id = 82;
        let update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &sessions,
            &active_paths,
            Some(&root),
        );
        assert!(
            matches!(update, ActiveThreadUpdate::Snapshot(rows) if rows.len() == 1 && rows[0].id == "root")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_live_state_matrix_is_fail_closed_across_path_and_rollout_states() {
        #[derive(Clone, Copy)]
        enum ChildFixture {
            Running,
            Completed,
            Invalid,
            Missing,
        }

        let cases = [
            ("running-active", ChildFixture::Running, true, "two"),
            ("running-inactive", ChildFixture::Running, false, "root"),
            ("completed-active", ChildFixture::Completed, true, "root"),
            ("invalid-inactive", ChildFixture::Invalid, false, "root"),
            ("invalid-active", ChildFixture::Invalid, true, "failed"),
            ("missing-row", ChildFixture::Missing, true, "failed"),
        ];

        for (index, (label, fixture, child_active, expected)) in cases.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "codex-info-live-state-matrix-{}-{index}",
                std::process::id()
            ));
            create_native_state_schema(&root);
            let sessions = root.join("sessions");
            let root_rollout = sessions.join("root.jsonl");
            let child_rollout = sessions.join("child.jsonl");
            write_native_rollout(&root_rollout, false);
            match fixture {
                ChildFixture::Running => write_native_rollout(&child_rollout, false),
                ChildFixture::Completed => write_native_rollout(&child_rollout, true),
                ChildFixture::Invalid => fs::write(&child_rollout, b"{not-json}\n").unwrap(),
                ChildFixture::Missing => write_native_rollout(&child_rollout, false),
            }
            if !matches!(fixture, ChildFixture::Missing) {
                add_native_state_thread(&root, "child", &child_rollout);
            }
            add_native_state_edge(&root, "root", "child");

            let (sender, receiver) = mpsc::channel();
            sender
                .send(RpcReadEvent::Line(
                    super::security::RpcLine::new(
                        json!({
                            "id": 90,
                            "result": {"data": [thread_list_item("root", 10, &root_rollout)]}
                        })
                        .to_string(),
                    )
                    .unwrap(),
                ))
                .unwrap();
            let mut active_paths = BTreeSet::from([fs::canonicalize(&root_rollout).unwrap()]);
            if child_active {
                active_paths.insert(fs::canonicalize(&child_rollout).unwrap());
            }
            let mut input = Vec::new();
            let mut next_id = 90;
            let update = fetch_active_thread_update_for_paths_and_state(
                &mut input,
                &receiver,
                &mut next_id,
                &sessions,
                &active_paths,
                Some(&root),
            );
            match expected {
                "two" => assert!(
                    matches!(update, ActiveThreadUpdate::Snapshot(rows) if rows.len() == 2),
                    "{label}"
                ),
                "root" => assert!(
                    matches!(update, ActiveThreadUpdate::Snapshot(rows) if rows.len() == 1 && rows[0].id == "root"),
                    "{label}"
                ),
                "failed" => assert_eq!(update, ActiveThreadUpdate::Failed, "{label}"),
                _ => unreachable!(),
            }
            let _ = fs::remove_dir_all(root);
        }

        let root_cases = [
            ("root-running-active", false, true, "one"),
            ("root-running-inactive", false, false, "empty"),
            ("root-terminal-active", true, true, "empty"),
            ("root-invalid-active", false, true, "failed"),
        ];
        for (index, (label, completed, root_active, expected)) in root_cases.into_iter().enumerate()
        {
            let root = std::env::temp_dir().join(format!(
                "codex-info-live-state-root-matrix-{}-{index}",
                std::process::id()
            ));
            create_native_state_schema(&root);
            let sessions = root.join("sessions");
            let root_rollout = sessions.join("root.jsonl");
            if label == "root-invalid-active" {
                fs::write(&root_rollout, b"{not-json}\n").unwrap();
            } else {
                write_native_rollout(&root_rollout, completed);
            }
            let (sender, receiver) = mpsc::channel();
            sender
                .send(RpcReadEvent::Line(
                    super::security::RpcLine::new(
                        json!({
                            "id": 100,
                            "result": {"data": [thread_list_item("root", 10, &root_rollout)]}
                        })
                        .to_string(),
                    )
                    .unwrap(),
                ))
                .unwrap();
            let active_paths = if root_active {
                BTreeSet::from([fs::canonicalize(&root_rollout).unwrap()])
            } else {
                BTreeSet::new()
            };
            let mut input = Vec::new();
            let mut next_id = 100;
            let update = fetch_active_thread_update_for_paths_and_state(
                &mut input,
                &receiver,
                &mut next_id,
                &sessions,
                &active_paths,
                Some(&root),
            );
            match expected {
                "one" => assert!(
                    matches!(update, ActiveThreadUpdate::Snapshot(rows) if rows.len() == 1),
                    "{label}"
                ),
                "empty" => assert_eq!(update, ActiveThreadUpdate::NoThread, "{label}"),
                "failed" => assert_eq!(update, ActiveThreadUpdate::Failed, "{label}"),
                _ => unreachable!(),
            }
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn native_descendant_failure_rejects_root_snapshot_atomically() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-native-atomic-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_native_state_schema(&root);
        let sessions = root.join("sessions");
        let root_rollout = sessions.join("root.jsonl");
        let invalid_rollout = sessions.join("invalid-child.jsonl");
        write_native_rollout(&root_rollout, false);
        fs::write(&invalid_rollout, b"{not-json}\n").unwrap();
        add_native_state_thread(&root, "invalid-child", &invalid_rollout);
        add_native_state_edge(&root, "root", "invalid-child");

        let (sender, receiver) = mpsc::channel();
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 81,
                        "result": {"data": [thread_list_item("root", 10, &root_rollout)]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let active_paths = BTreeSet::from([
            fs::canonicalize(&root_rollout).unwrap(),
            fs::canonicalize(&invalid_rollout).unwrap(),
        ]);
        let mut input = Vec::new();
        let mut next_id = 81;
        let update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &sessions,
            &active_paths,
            Some(&root),
        );
        assert_eq!(update, ActiveThreadUpdate::Failed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn session_traversal_budgets_and_symlink_rejection_have_exact_boundaries() {
        assert_eq!(super::security::MAX_SESSION_FILE_BYTES, 256 * 1024 * 1024);
        assert_eq!(
            super::security::MAX_SESSION_TOTAL_BYTES,
            2 * 1024 * 1024 * 1024
        );
        assert!(SessionTraversalBudget::default()
            .admit_file(1, 64 * 1024 * 1024 + 1)
            .is_ok());

        let mut files = SessionTraversalBudget::default();
        for _ in 0..super::security::MAX_SESSION_FILES {
            files.admit_file(1, 0).expect("file budget boundary");
        }
        assert!(files.admit_file(1, 0).is_err());

        let mut total = SessionTraversalBudget::default();
        for _ in 0..8 {
            total
                .admit_file(1, super::security::MAX_SESSION_FILE_BYTES)
                .expect("total byte boundary");
        }
        assert_eq!(total.total_bytes, super::security::MAX_SESSION_TOTAL_BYTES);
        assert!(total.admit_file(1, 1).is_err());
        assert!(SessionTraversalBudget::default()
            .admit_file(1, super::security::MAX_SESSION_FILE_BYTES + 1)
            .is_err());
        assert!(SessionTraversalBudget::default()
            .admit_file(super::security::MAX_SESSION_DEPTH + 1, 0)
            .is_err());

        let root = std::env::temp_dir().join(format!(
            "codex-info-traversal-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir(&root).unwrap();
        let safe = root.join("safe.jsonl");
        fs::write(&safe, "{}\n").unwrap();
        assert_eq!(session_jsonl_files(&root).unwrap(), vec![safe]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("safe.jsonl", root.join("linked.jsonl")).unwrap();
            assert!(session_jsonl_files(&root).is_err());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_rollout_snapshot_accepts_append_and_defers_partial_tail() {
        let root = std::env::temp_dir().join(format!(
            "codex-info-rollout-append-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("active.jsonl");
        let complete = concat!(
            "{\"type\":\"thread_context\",\"model\":\"gpt-5.6-sol\"}\n",
            "{\"type\":\"task_started\"}\n"
        );
        let partial = "{\"type\":\"event_msg\"";
        fs::write(&path, format!("{complete}{partial}")).unwrap();

        let before = fs::metadata(&path).unwrap();
        let mut file = File::open(&path).unwrap();
        let snapshot_len = file.metadata().unwrap().len();
        let complete_len = complete_rollout_prefix_len(&mut file, snapshot_len).unwrap();
        assert_eq!(complete_len, complete.len() as u64);
        file.seek(SeekFrom::Start(0)).unwrap();
        let rollout = {
            let mut reader = BufReader::new((&mut file).take(complete_len));
            thread_contract::parse_rollout_reader(&mut reader, complete_len).unwrap()
        };
        assert!(rollout.is_running());
        assert_eq!(rollout.model(), "gpt-5.6-sol");
        assert_eq!(rollout.total_tokens(), None);

        let remainder = concat!(
            ",\"payload\":{\"type\":\"token_count\",\"info\":{",
            "\"total_token_usage\":{\"total_tokens\":77}}}}\n"
        );
        let mut append = fs::OpenOptions::new().append(true).open(&path).unwrap();
        append.write_all(remainder.as_bytes()).unwrap();
        append.flush().unwrap();
        drop(append);
        let after = fs::metadata(&path).unwrap();
        assert!(same_rollout_identity(&before, &after));
        assert!(after.len() > before.len());

        let mut file = File::open(&path).unwrap();
        let complete_len = complete_rollout_prefix_len(&mut file, after.len()).unwrap();
        assert_eq!(complete_len, after.len());
        file.seek(SeekFrom::Start(0)).unwrap();
        let rollout = {
            let mut reader = BufReader::new((&mut file).take(complete_len));
            thread_contract::parse_rollout_reader(&mut reader, complete_len).unwrap()
        };
        assert!(rollout.is_running());
        assert_eq!(rollout.total_tokens(), Some(77));

        let other = root.join("other.jsonl");
        fs::write(&other, "{}\n").unwrap();
        assert!(!same_rollout_identity(
            &after,
            &fs::metadata(&other).unwrap()
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn production_rollout_reader_separates_record_recovery_from_candidate_failure() {
        let root = std::env::temp_dir().join(format!(
            "codex-info-rollout-boundaries-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir(&root).unwrap();

        let recoverable = root.join("recoverable.jsonl");
        let prefix = concat!(
            "{\"type\":\"thread_context\",\"model\":\"gpt-5.6-sol\"}\n",
            "{\"type\":\"task_started\"}\n"
        );
        let oversized = format!(
            "{{\"type\":\"response_item\",\"payload\":\"{}\"}}\n",
            "x".repeat(security::MAX_JSONL_LINE_BYTES + 128)
        );
        let suffix = concat!(
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",",
            "\"info\":{\"total_token_usage\":{\"total_tokens\":321}}}}\n"
        );
        let mut bytes = prefix.as_bytes().to_vec();
        bytes.extend_from_slice(oversized.as_bytes());
        bytes.extend_from_slice(&[b'{', 0xff, b'}', b'\n']);
        bytes.extend_from_slice(suffix.as_bytes());
        fs::write(&recoverable, bytes).unwrap();
        let rollout = read_thread_rollout_path(&root, &recoverable)
            .expect("oversized/invalid-UTF8 records are isolated by the production reader");
        assert!(rollout.is_running());
        assert_eq!(rollout.total_tokens(), Some(321));

        let malformed = root.join("malformed.jsonl");
        fs::write(
            &malformed,
            concat!(
                "{\"type\":\"thread_context\",\"model\":\"gpt-5.6-sol\"}\n",
                "{not-json}\n",
                "{\"type\":\"task_started\"}\n"
            ),
        )
        .unwrap();
        assert!(read_thread_rollout_path(&root, &malformed).is_err());

        let known_event_error = root.join("known-event-error.jsonl");
        fs::write(
            &known_event_error,
            concat!(
                "{\"type\":\"thread_context\",\"model\":\"gpt-5.6-sol\"}\n",
                "{\"type\":\"event_msg\",\"payload\":{}}\n"
            ),
        )
        .unwrap();
        assert!(read_thread_rollout_path(&root, &known_event_error).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&malformed, root.join("symlink.jsonl")).unwrap();
            assert!(read_thread_rollout_path(&root, &root.join("symlink.jsonl")).is_err());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rpc_request_enforces_mismatch_timeout_and_error_redaction() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..super::security::MAX_RPC_IGNORED_MESSAGES {
            tx.send(RpcReadEvent::Line(
                super::security::RpcLine::new(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#).unwrap(),
            ))
            .unwrap();
        }
        tx.send(RpcReadEvent::Line(
            super::security::RpcLine::new(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
                .unwrap(),
        ))
        .unwrap();
        let mut input = Vec::new();
        assert_eq!(
            request_with_timeout(
                &mut input,
                &rx,
                1,
                "test/read",
                Value::Null,
                Duration::from_millis(50),
            )
            .unwrap(),
            json!({"ok":true})
        );

        let (tx, rx) = mpsc::channel();
        for _ in 0..=super::security::MAX_RPC_IGNORED_MESSAGES {
            tx.send(RpcReadEvent::Line(
                super::security::RpcLine::new(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#).unwrap(),
            ))
            .unwrap();
        }
        assert!(request_with_timeout(
            &mut Vec::new(),
            &rx,
            1,
            "test/read",
            Value::Null,
            Duration::from_millis(50),
        )
        .unwrap_err()
        .contains("上限"));

        let (_tx, rx) = mpsc::channel();
        assert!(request_with_timeout(
            &mut Vec::new(),
            &rx,
            1,
            "test/read",
            Value::Null,
            Duration::from_millis(1),
        )
        .unwrap_err()
        .contains("タイムアウト"));

        let (tx, rx) = mpsc::channel();
        tx.send(RpcReadEvent::Line(
            super::security::RpcLine::new(
                r#"{"jsonrpc":"2.0","id":1,"error":{"secret":"token-value"}}"#,
            )
            .unwrap(),
        ))
        .unwrap();
        let error = request_with_timeout(
            &mut Vec::new(),
            &rx,
            1,
            "test/read",
            Value::Null,
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(!error.contains("token-value"));
    }

    #[test]
    fn preview_size_parser_only_parses_syntax() {
        assert_eq!(parse_preview_size(Some("1200x800")), Some((1200, 800)));
        assert_eq!(parse_preview_size(Some("699x479")), Some((699, 479)));
        assert_eq!(parse_preview_size(Some("700x480x1")), None);
        assert_eq!(parse_preview_size(Some("not-a-size")), None);
        assert_eq!(parse_preview_size(None), None);
    }

    #[test]
    fn login_confirmation_poll_is_fast_only_while_authentication_is_pending() {
        assert_eq!(
            automatic_refresh_interval(false, true),
            Duration::from_secs(2)
        );
        assert_eq!(
            automatic_refresh_interval(false, false),
            Duration::from_secs(60)
        );
        assert_eq!(
            automatic_refresh_interval(true, true),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn transient_account_worker_failure_still_admits_the_next_periodic_read() {
        let mut state = CodexInfoState::preview("normal");
        state.checking = true;
        state.authenticated = true;
        state.last_poll = Instant::now() - Duration::from_secs(61);

        // A failed worker is a publication error, not a permanent polling
        // disable. It clears the in-flight marker while retaining last-good
        // quota/history state.
        state.apply_account_error("transient worker failure".into());
        assert!(!state.checking);
        assert!(account_refresh_due(
            Instant::now(),
            state.last_poll,
            state.checking,
            state.authenticated,
            state.auth_polling,
        ));

        // The timer owns one read at a time; once a read is in flight it must
        // not schedule a second request until the worker reports or fails.
        assert!(!account_refresh_due(
            Instant::now(),
            state.last_poll,
            true,
            state.authenticated,
            state.auth_polling,
        ));
    }

    #[test]
    fn preview_size_keeps_main_fixed_and_applies_graph_minimums() {
        assert_eq!(
            parse_preview_size(Some("600x400")).map(|_| (900, 480)),
            Some((900, 480))
        );
        assert_eq!(
            parse_preview_size(Some("1200x800")).map(|_| (900, 480)),
            Some((900, 480))
        );
        assert_eq!(
            clamp_graph_preview_size(parse_preview_size(Some("600x400")).unwrap()),
            (700, 480)
        );
        assert_eq!(
            clamp_graph_preview_size(parse_preview_size(Some("1200x800")).unwrap()),
            (1200, 800)
        );
        assert_eq!(parse_preview_size(Some("700x540x")), None);
    }

    #[test]
    fn fixed_resize_decision_preserves_minimize_and_rejects_every_wrong_surface() {
        assert_eq!(
            fixed_resize_decision(FIXED_WINDOW_WIDTH, FIXED_WINDOW_HEIGHT),
            FixedResizeDecision::Propagate
        );
        for (width, height) in [(0, 0), (0, 480), (900, 0)] {
            assert_eq!(
                fixed_resize_decision(width, height),
                FixedResizeDecision::Propagate,
                "zero-sized minimize event {width}x{height}"
            );
        }
        for (width, height) in [(899, 480), (901, 480), (900, 479), (900, 481), (1080, 600)] {
            assert_eq!(
                fixed_resize_decision(width, height),
                FixedResizeDecision::RejectAndRestore,
                "non-zero resize {width}x{height}"
            );
        }
    }

    #[test]
    fn main_window_position_is_visible_on_the_primary_monitor() {
        assert_eq!(
            visible_window_position(
                winit::dpi::PhysicalPosition::new(1723, 149),
                winit::dpi::PhysicalSize::new(1920, 1080),
                winit::dpi::PhysicalSize::new(900, 480),
            ),
            winit::dpi::PhysicalPosition::new(1755, 181)
        );
        assert_eq!(
            visible_window_position(
                winit::dpi::PhysicalPosition::new(-500, -200),
                winit::dpi::PhysicalSize::new(400, 300),
                winit::dpi::PhysicalSize::new(900, 480),
            ),
            winit::dpi::PhysicalPosition::new(-500, -200)
        );
    }

    #[test]
    fn fixed_resize_decision_follows_the_current_os_scale_factor() {
        assert_eq!(physical_size_for_logical(900, 480, 1.0), (900, 480));
        assert_eq!(physical_size_for_logical(900, 480, 1.25), (1125, 600));
        assert_eq!(physical_size_for_logical(900, 480, 2.5), (2250, 1200));
        assert_eq!(
            fixed_resize_decision_for_scale(2250, 1200, 900, 480, 2.5),
            FixedResizeDecision::Propagate
        );
        assert_eq!(
            fixed_resize_decision_for_scale(900, 480, 900, 480, 2.5),
            FixedResizeDecision::RejectAndRestore
        );
    }

    #[test]
    fn graph_resize_handles_cover_all_edges_and_corners() {
        let directions = [
            ("north", winit::window::ResizeDirection::North),
            ("south", winit::window::ResizeDirection::South),
            ("east", winit::window::ResizeDirection::East),
            ("west", winit::window::ResizeDirection::West),
            ("north-east", winit::window::ResizeDirection::NorthEast),
            ("north-west", winit::window::ResizeDirection::NorthWest),
            ("south-east", winit::window::ResizeDirection::SouthEast),
            ("south-west", winit::window::ResizeDirection::SouthWest),
        ];
        for (name, expected) in directions {
            assert_eq!(parse_resize_direction(name), Some(expected));
        }
        assert_eq!(parse_resize_direction("invalid"), None);

        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        assert!(graph.contains("callback begin-window-resize(string);"));
        for direction in [
            "direction: \"north\";",
            "direction: \"south\";",
            "direction: \"east\";",
            "direction: \"west\";",
            "direction: \"north-east\";",
            "direction: \"north-west\";",
            "direction: \"south-east\";",
            "direction: \"south-west\";",
        ] {
            assert!(
                graph.contains(direction),
                "missing graph resize handle: {direction}"
            );
        }
        for marker in [
            "width: root.width - 28px;",
            "height: 14px;",
            "height: 28px;",
            "resize-cursor: MouseCursor.nwse-resize;",
            "resize-cursor: MouseCursor.nesw-resize;",
            "corner: true;",
        ] {
            assert!(
                graph.contains(marker),
                "missing resize affordance: {marker}"
            );
        }
        let main = include_str!("../src/main.rs");
        assert!(main.contains("graph.on_begin_window_resize"));
        assert!(main.contains("drag_resize_window(direction)"));
    }

    #[test]
    fn manual_resize_geometry_keeps_corner_direction_and_minimum() {
        let initial = ManualX11Geometry {
            x: 100,
            y: 80,
            width: 940,
            height: 640,
        };
        assert_eq!(
            manual_resize_geometry(initial, winit::window::ResizeDirection::SouthEast, 120, 80,),
            ManualX11Geometry {
                x: 100,
                y: 80,
                width: 1060,
                height: 720,
            }
        );
        assert_eq!(
            manual_resize_geometry(initial, winit::window::ResizeDirection::NorthWest, 120, 80,),
            ManualX11Geometry {
                x: 220,
                y: 160,
                width: 820,
                height: 560,
            }
        );
        assert_eq!(
            manual_resize_geometry(
                initial,
                winit::window::ResizeDirection::NorthWest,
                1_000,
                1_000,
            ),
            ManualX11Geometry {
                x: 340,
                y: 240,
                width: 700,
                height: 480,
            }
        );
    }

    #[test]
    fn manual_move_geometry_preserves_client_origin_and_applies_pointer_delta() {
        let initial = ManualX11Geometry {
            x: 2_506,
            y: 1_296,
            width: 900,
            height: 480,
        };
        assert_eq!(
            manual_window_geometry(initial, ManualX11WindowAction::Move, 0, 0),
            initial
        );
        assert_eq!(
            manual_window_geometry(initial, ManualX11WindowAction::Move, 60, 40),
            ManualX11Geometry {
                x: 2_566,
                y: 1_336,
                ..initial
            }
        );
        assert_eq!(
            manual_window_geometry(initial, ManualX11WindowAction::Move, -40, -30),
            ManualX11Geometry {
                x: 2_466,
                y: 1_266,
                ..initial
            }
        );
    }

    #[test]
    fn manual_x11_action_claim_is_exclusive_per_target_and_released_after_finish() {
        let target = u32::MAX - 17;
        let other_target = target - 1;
        let client = target - 2;
        let other_client = target - 3;
        let lease =
            claim_manual_x11_action(target, client).expect("first target claim should succeed");
        assert!(claim_manual_x11_action(target, other_client).is_none());
        assert!(claim_manual_x11_action(other_target, client).is_none());
        let other_lease = claim_manual_x11_action(other_target, other_target);
        assert!(other_lease.is_some());
        drop(lease);
        let final_lease = claim_manual_x11_action(target, target);
        assert!(final_lease.is_some());
        drop(final_lease);
        drop(other_lease);
    }

    #[test]
    fn manual_x11_move_uses_root_client_coordinates_and_skips_static_click_configure() {
        let source = include_str!("../src/main.rs");
        assert!(source.contains("connection.translate_coordinates(window_id, root, 0, 0)"));
        assert!(source.contains("let mut last_geometry = initial;"));
        assert!(source.contains("if geometry == last_geometry"));
        assert!(source.contains("finish_manual_x11_action(&connection, target);"));
        assert!(source.contains(
            "ManualX11WindowAction::Move => ConfigureWindowAux::new().x(geometry.x).y(geometry.y)"
        ));
    }

    #[test]
    fn native_window_contracts_keep_non_graph_windows_move_only() {
        let main = include_str!("../ui/app.slint");
        assert!(main.contains("title: root.window-title;"));
        assert!(main.contains("no-frame: true;"));
        assert!(main.contains("resize-border-width: 0px;"));
        assert!(main.contains("z: -5;"));
        assert!(main.contains("width: root.width;\n        height: root.height;"));
        assert!(!main.contains("title: \"Codex Info\";"));
        let components = include_str!("../ui/components.slint");
        assert!(components.contains("export component WindowControls"));
        assert!(components.contains("export component WindowDragArea"));
        let header = components
            .split("export component Header inherits Rectangle {")
            .nth(1)
            .and_then(|source| source.split("export component RemainingQuota").next())
            .expect("Header component");
        assert!(header.contains("private property <length> action-start:"));
        assert!(header.contains("width: root.action-start;"));
        let threads = components
            .split("export component ThreadsWindow inherits Window {")
            .nth(1)
            .expect("ThreadsWindow");
        let graph = components
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        let legal_notice = components
            .split("export component LegalNoticeWindow inherits Window {")
            .nth(1)
            .expect("LegalNoticeWindow");
        assert!(threads.contains("title: root.window-title;"));
        assert!(threads.contains("no-frame: true;"));
        assert!(threads.contains("resize-border-width: 0px;"));
        assert!(threads.contains("WindowControls"));
        assert!(threads.contains("WindowDragArea"));
        assert!(threads.contains("z: -5;"));
        assert!(threads.contains("width: root.width;\n        height: root.height;"));
        assert!(graph.contains("title: root.window-title;"));
        assert!(graph.contains("no-frame: true;"));
        assert!(graph.contains("resize-border-width: 6px;"));
        assert!(graph.contains("show-maximize: true;"));
        assert!(graph.contains("WindowControls"));
        assert!(graph.contains("WindowDragArea"));
        assert!(graph.contains("z: -5;"));
        assert!(graph.contains("width: root.width;\n        height: root.height;"));
        assert!(legal_notice.contains("title: root.window-title;"));
        assert!(legal_notice.contains("no-frame: true;"));
        assert!(legal_notice.contains("resize-border-width: 0px;"));
        assert!(legal_notice.contains("WindowControls"));
        assert!(legal_notice.contains("WindowDragArea"));
        assert!(legal_notice.contains("z: -5;"));
        assert!(legal_notice.contains("width: root.width;\n        height: root.height;"));
        for marker in [
            "preferred-width: 720px;",
            "preferred-height: 520px;",
            "min-width: 720px;",
            "max-width: 720px;",
            "min-height: 520px;",
            "max-height: 520px;",
        ] {
            assert!(
                legal_notice.contains(marker),
                "missing LegalNoticeWindow marker: {marker}"
            );
        }
        assert!(main.contains("callback open-legal-notice();"));
        for fixed_source in [main, threads] {
            assert!(fixed_source.contains("min-width: 900px;"));
            assert!(fixed_source.contains("max-width: 900px;"));
        }
        assert!(!graph.contains("max-width: 940px;"));
        let rust_source = include_str!("main.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("production source");
        assert!(rust_source.contains("on_winit_window_event"));
        assert!(rust_source.contains("EventResult::PreventDefault"));
        assert!(rust_source.contains("request_inner_size"));
        assert_eq!(
            rust_source
                .matches("install_fixed_window_guard(ui.window())")
                .count(),
            1
        );
        assert_eq!(
            rust_source
                .matches("install_fixed_window_guard(window.window())")
                .count(),
            1
        );
        assert!(rust_source.contains("install_resizable_window(graph.window());"));
        assert!(rust_source.contains("fn toggle_graph_maximize("));
        assert!(rust_source.contains(".current_monitor()"));
        assert!(rust_source.contains("winit_window.request_inner_size(size)"));
        assert!(rust_source.contains("graph.set_app_maximized(true)"));
        assert!(!rust_source.contains("fn toggle_maximize_window("));
        assert!(!rust_source
            .contains("install_window_size_guard(graph.window(), graph_width, graph_height);"));
        assert!(rust_source.contains(
            "install_window_size_guard(\n                        window.window(),\n                        LEGAL_WINDOW_WIDTH,\n                        LEGAL_WINDOW_HEIGHT,\n                    );"
        ));
        assert!(rust_source.contains("winit_window.set_resizable(false)"));
        assert!(rust_source.contains("winit_window.set_resizable(true)"));
        assert_eq!(rust_source.matches("LegalNoticeWindow::new()").count(), 1);
        assert!(rust_source.contains("ui.on_open_legal_notice"));
    }

    #[test]
    fn product_version_is_visible_once_on_native_main_surface() {
        assert!(!PRODUCT_VERSION.is_empty());
        assert!(PRODUCT_VERSION.split('.').all(
            |component| !component.is_empty() && component.chars().all(|c| c.is_ascii_digit())
        ));

        let source = include_str!("../ui/components.slint");
        assert!(source.contains("product-version: string"));
        let marker = "root.strings.usage-status + \" · \" + root.strings.product-version";
        assert_eq!(source.matches(marker).count(), 1);
        assert!(
            !source.contains("root.strings.usage-trend + \" · \" + root.strings.product-version")
        );
        assert!(!source
            .contains("root.strings.active-threads + \" · \" + root.strings.product-version"));
        assert!(
            !source.contains("root.strings.legal-notices + \" · \" + root.strings.product-version")
        );
        for component in [
            "export component GraphWindow inherits Window {",
            "export component ThreadsWindow inherits Window {",
            "export component LegalNoticeWindow inherits Window {",
        ] {
            let body = source.split(component).nth(1).expect(component);
            let body = body
                .split("export component ")
                .next()
                .expect("component body");
            assert!(
                !body.contains("product-version"),
                "child component directly references the product version: {component}"
            );
        }

        let main = include_str!("main.rs");
        assert!(main.contains("product_version: format!(\"v{PRODUCT_VERSION}\").into()"));
    }

    #[test]
    fn native_legal_surface_paginates_complete_packaged_documents() {
        let source = include_str!("../ui/components.slint");
        for marker in [
            "legal-page-names: [string]",
            "legal-pages: [string]",
            "callback legal-page-back();",
            "callback legal-page-next();",
            "root.strings.legal-pages[root.legal-page-index]",
            "root.strings.legal-page-position",
        ] {
            assert!(
                source.contains(marker),
                "missing native legal contract: {marker}"
            );
        }
        for document in [
            include_str!("../LICENSE"),
            include_str!("../THIRD_PARTY_NOTICES.md"),
            include_str!("../LICENSES/Apache-2.0.txt"),
            include_str!("../LICENSES/MIT.txt"),
            include_str!("../LICENSES/OFL-1.1.txt"),
            include_str!("../LICENSES/Inno-Setup.txt"),
        ] {
            assert!(!document.trim().is_empty());
        }
        let (names, pages) = native_legal_pages(&I18n::from_parts(
            codex_info::i18n::Language::Japanese,
            chrono_tz::Tz::UTC,
        ));
        assert_eq!(names.len(), pages.len());
        assert!(pages.len() > 9);
        let mut chapter_names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
        chapter_names.dedup();
        assert_eq!(chapter_names.len(), 9);
        assert!(chapter_names[4].contains("プロトコル"));
        assert!(chapter_names[6].contains("第三者"));
        assert!(pages
            .iter()
            .any(|page| page.contains("GNU GENERAL PUBLIC LICENSE")));
        assert!(pages
            .iter()
            .any(|page| page.contains("THIRD_PARTY_NOTICES.md")));
    }

    #[test]
    fn thread_rails_have_fixed_geometry_and_sufficient_contrast() {
        let source = include_str!("../ui/components.slint");
        for marker in [
            "width: 2px;",
            "height: root.thread-row-height;",
            "property <length> tree-base-x: 24px;",
            "property <length> tree-depth-step: 16px;",
            "property <length> tree-junction-y: 36px;",
            "x: parent.tree-base-x + parent.tree-depth-step;",
            "x: parent.tree-base-x + 2 * parent.tree-depth-step;",
            "y: parent.tree-junction-y - 1px;",
            "width: parent.title-x - self.x - 20px;",
            "background: DesignTokens.warning;",
            "height: root.thread-row-height - parent.tree-junction-y;",
            "border-radius: 2px;",
            "ancestor-guide-1",
            "ancestor-guide-2",
            "ancestor-guide-3",
        ] {
            assert!(source.contains(marker), "missing rail geometry: {marker}");
        }
        assert!(!source.contains("tree-guide"));
        assert!(!source.contains("row.indent"));

        fn luminance(rgb: [u8; 3]) -> f64 {
            let linear = rgb.map(|component| {
                let component = f64::from(component) / 255.0;
                if component <= 0.04045 {
                    component / 12.92
                } else {
                    ((component + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
        }
        let rail = luminance([0xe6, 0xa2, 0x3c]);
        for row in [[0x0d, 0x13, 0x1e], [0x14, 0x1d, 0x2d]] {
            let background = luminance(row);
            assert!((rail + 0.05) / (background + 0.05) >= 7.719);
        }
    }

    #[test]
    fn thread_rails_keep_every_text_lane_outside_the_tree_gutter() {
        let source = include_str!("../ui/components.slint");
        for marker in [
            "property <length> tree-base-x: 24px;",
            "property <length> tree-depth-step: 16px;",
            "property <length> tree-junction-y: 36px;",
            "x: root.single-thread ? 20px : 72px;",
            ": 172px + self.display-depth * 24px;",
            "width: parent.title-x - self.x - 20px;",
            "x: parent.title-x - 24px;",
            "if !root.single-thread && row.ancestor-guide-1 : Rectangle {",
            "if !root.single-thread && row.connected-to-parent : Rectangle {",
            "if !root.single-thread && row.has-children : Rectangle {",
        ] {
            assert!(
                source.contains(marker),
                "missing non-overlap contract: {marker}"
            );
        }
    }

    #[test]
    fn forbidden_x11_states_identifies_fullscreen_maximize_and_unrelated_atoms() {
        let atoms = X11StateAtoms {
            wm_state: 1,
            fullscreen: 2,
            maximized_vert: 3,
            maximized_horz: 4,
            active_window: None,
        };
        assert_eq!(forbidden_x11_states(&[], &atoms), (false, false));
        assert_eq!(
            forbidden_x11_states(&[atoms.fullscreen], &atoms),
            (true, false)
        );
        assert_eq!(
            forbidden_x11_states(&[atoms.maximized_horz], &atoms),
            (false, true)
        );
        assert_eq!(
            forbidden_x11_states(&[atoms.wm_state], &atoms),
            (false, false)
        );
    }

    #[test]
    fn motif_functions_allow_move_minimize_close_without_resize_or_maximize() {
        assert_eq!(motif_wm_functions(0), (1, 0x2c));
        assert_eq!(motif_wm_functions(0x40), (0x41, 0x2c));
    }

    #[test]
    fn motif_functions_allow_graph_resize_and_maximize() {
        assert_eq!(motif_wm_resizable_functions(0x2, 0), (0x3, 0x3e));
        assert_eq!(motif_wm_resizable_functions(0x3, 1), (0x3, 1));
    }

    #[test]
    fn preview_size_bounds_match_slint_window_constraints() {
        let main = include_str!("../ui/app.slint");
        assert!(main.contains("min-width: 900px;"));
        assert!(main.contains("max-width: 900px;"));
        assert!(main.contains("preferred-width: 900px;"));
        assert!(main.contains("min-height: 480px;"));
        assert!(main.contains("max-height: 480px;"));
        assert!(main.contains("preferred-height: 480px;"));
        for marker in [
            "changed maximized =>",
            "changed full-screen =>",
            "self.maximized = false;",
            "self.full-screen = false;",
        ] {
            assert!(
                main.contains(marker),
                "missing MainWindow runtime guard: {marker}"
            );
        }
        for marker in ["changed width =>", "changed height =>"] {
            assert!(
                !main.contains(marker),
                "unexpected MainWindow size guard: {marker}"
            );
        }

        let threads = include_str!("../ui/components.slint")
            .split("export component ThreadsWindow inherits Window {")
            .nth(1)
            .expect("ThreadsWindow");
        for marker in [
            "preferred-width: 900px;",
            "preferred-height: 480px;",
            "min-width: 900px;",
            "max-width: 900px;",
            "min-height: 480px;",
            "max-height: 480px;",
            "changed maximized =>",
            "changed full-screen =>",
            "for row[index] in root.thread-rows",
        ] {
            assert!(
                threads.contains(marker),
                "missing ThreadsWindow contract: {marker}"
            );
        }

        let graph = include_str!("../ui/components.slint")
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        assert!(graph.contains("min-width: 700px;"));
        assert!(graph.contains("preferred-width: 940px;"));
        assert!(graph.contains("min-height: 480px;"));
        assert!(graph.contains("preferred-height: 640px;"));
        assert!(graph.contains("in-out property <bool> app-maximized: false;"));
        assert!(graph.contains("maximized: root.app-maximized;"));
        for marker in [
            "max-width: 940px;",
            "max-height: 640px;",
            "changed width =>",
            "changed height =>",
            "changed maximized =>",
            "changed full-screen =>",
        ] {
            assert!(
                !graph.contains(marker),
                "unexpected GraphWindow bound or runtime guard: {marker}"
            );
        }
    }

    #[test]
    fn graph_layout_formula_matches_minimum_initial_and_expanded_contract() {
        let source = include_str!("../ui/components.slint");
        for expression in [
            "20px + (root.width - 700px) / 24",
            "root.width - 2 * root.content-x",
            "root.content-width - root.plot-left - root.current-label-gap - root.current-label-width - root.current-label-right-padding",
            "height: parent.height - root.history-toggle-y - 32px;",
            "height: parent.height - 52px;",
        ] {
            assert!(
                source.contains(expression),
                "missing layout formula: {expression}"
            );
        }
        let geometry = |width: f64, height: f64| {
            let margin = (20.0 + (width - 700.0) / 24.0).clamp(20.0, 30.0);
            let content = width - 2.0 * margin;
            // 92px plot gutter + 10px leader gap + 80px dollar labels + 4px
            // right padding. Token mode reserves a wider label column.
            let plot_width = content - 186.0;
            let plot_height = height - 276.0;
            (margin, plot_width, plot_height)
        };
        assert_eq!(geometry(700.0, 480.0), (20.0, 474.0, 204.0));
        assert_eq!(geometry(940.0, 640.0), (30.0, 694.0, 364.0));
        assert_eq!(geometry(1_200.0, 800.0), (30.0, 954.0, 524.0));
        assert_eq!(geometry(1_201.0, 801.0).1 - geometry(1_200.0, 800.0).1, 1.0);
        assert_eq!(geometry(1_201.0, 801.0).2 - geometry(1_200.0, 800.0).2, 1.0);
    }

    #[test]
    fn fixed_windows_have_x11_state_monitor_without_runtime_size_repair() {
        let source = include_str!("main.rs");
        let source = source
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .unwrap();
        assert!(!source.contains("struct WindowBounds"));
        assert!(!source.contains("enforce_window_bounds"));
        assert!(!source.contains("window.set_size(slint::LogicalSize::new("));
        assert!(source.contains("monitor.enforce(ui.window());"));
        assert!(source.contains("monitor.enforce(window.window());"));
        assert!(!source.contains("monitor.enforce(graph.window());"));
        assert_eq!(source.matches("Duration::from_millis(100)").count(), 2);
        assert_eq!(source.matches("GraphWindow::new()").count(), 1);
        assert_eq!(
            source
                .matches("show_and_focus_window(graph.window(),")
                .count(),
            1
        );
        assert_eq!(source.matches("graph.hide()").count(), 3);
    }

    #[test]
    fn existing_secondary_windows_are_raised_without_recreation() {
        let source = include_str!("main.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("production source");
        assert!(source.contains("fn show_and_focus_window("));
        assert!(source.contains("let was_visible = window.is_visible()"));
        assert!(
            source.contains("window.with_winit_window(|winit_window| winit_window.focus_window())")
        );
        assert!(source.contains("x11_monitor.raise_and_activate(window)"));
        assert!(source.contains("ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)"));
        assert_eq!(
            source
                .matches("show_and_focus_window(graph.window(),")
                .count(),
            1
        );
        assert_eq!(
            source
                .matches("show_and_focus_window(window.window(),")
                .count(),
            2
        );
        assert_eq!(source.matches("ThreadsWindow::new()").count(), 1);
        assert_eq!(source.matches("LegalNoticeWindow::new()").count(), 1);
        assert!(!source.contains("graph.show()"));
        assert!(!source.contains("let _ = window.show();"));
    }

    #[test]
    fn secondary_close_hides_before_native_close_and_skips_hidden_work() {
        let source = include_str!("main.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("production source");
        assert_eq!(source.matches("on_close_requested(move ||").count(), 3);
        assert_eq!(
            source
                .matches("CloseRequestResponse::KeepWindowShown")
                .count(),
            3
        );
        assert!(source.contains("if graph.hide().is_ok()"));
        assert!(source.contains("if window.hide().is_ok()"));
        assert!(source.contains("if graph.window().is_visible()"));
        assert!(source.contains("if window.window().is_visible()"));

        let components = include_str!("../ui/components.slint");
        let action_button = components
            .split_once("component ActionButton inherits Rectangle {")
            .and_then(|(_, source)| source.split_once("export component WeekGauge"))
            .map(|(source, _)| source)
            .expect("ActionButton component");
        assert!(action_button.contains("touch-area := TouchArea"));
        assert!(action_button.contains("touch-area.pressed"));
        assert!(action_button.contains("activate-on-press: false"));
        assert!(action_button.contains("reset-press-state: false"));
        assert!(action_button.contains("changed pressed =>"));
        assert_eq!(components.matches("activate-on-press: true;").count(), 0);
        assert_eq!(
            components
                .matches("reset-press-state: root.reset-close-buttons;")
                .count(),
            0
        );
        assert_eq!(source.matches("set_reset_close_buttons(true)").count(), 12);
    }

    #[test]
    fn run_script_has_one_exec_without_retry_loop() {
        let run = include_str!("../run.sh");
        assert_eq!(run.matches("run --manifest-path").count(), 1);
        assert!(run.contains("exec \"$CODEX_INFO_CARGO\" run --manifest-path"));
        assert!(run.contains("$HOME/.cargo/bin/cargo"));
        assert!(run.contains("rustup which cargo"));
        assert!(run.contains("unset WAYLAND_DISPLAY WAYLAND_SOCKET WINIT_X11_SCALE_FACTOR"));
        assert!(!run.contains("export WINIT_X11_SCALE_FACTOR"));
        assert!(run.contains("--release --locked"));
        assert!(run.contains("E_CARGO_NOT_FOUND"));
        assert!(!run.contains("デーモン"));
        assert!(!run.contains("Linux/X11版"));
        assert!(!run.contains("for attempt"));
        assert!(!run.contains("sleep 1"));
    }

    #[test]
    fn historical_period_uses_the_nearest_newer_reset_boundary() {
        let mut state = CodexInfoState::preview("monthly");
        let current_reset = 1_800_000_000;
        let previous_reset = current_reset - 7 * 86_400;
        state.reset_at = Some(current_reset);
        state.monthly = true;
        state.history.samples = vec![
            UsageHistorySample::new(
                1_700_000_000,
                current_reset,
                80.0,
                ModelDollarTotals::default(),
            ),
            UsageHistorySample::new(
                1_700_000_001,
                previous_reset,
                80.0,
                ModelDollarTotals::default(),
            ),
        ];
        assert_eq!(state.period_seconds_for_reset(previous_reset), 7 * 86_400);
        assert!(state.period_seconds_for_reset(current_reset) > 7 * 86_400);
    }

    #[test]
    fn codex_state_selects_latest_older_newer_periods_and_navigation_flags() {
        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(300);
        state.history.samples = [300, 200, 100]
            .into_iter()
            .map(|reset_at| {
                UsageHistorySample::new(reset_at, reset_at, 80.0, ModelDollarTotals::default())
            })
            .collect();
        state.selected_reset_at = Some(100);

        state.select_latest_history();
        assert_eq!(state.selected_history_reset(), Some(300));
        assert_eq!(state.history_navigation(), (true, false));

        state.select_older_history();
        assert_eq!(state.selected_history_reset(), Some(200));
        assert_eq!(state.history_navigation(), (true, true));

        state.select_older_history();
        assert_eq!(state.selected_history_reset(), Some(100));
        assert_eq!(state.history_navigation(), (false, true));

        state.select_newer_history();
        assert_eq!(state.selected_history_reset(), Some(200));
        state.select_newer_history();
        assert_eq!(state.selected_history_reset(), Some(300));
        assert_eq!(state.history_navigation(), (true, false));
    }

    #[test]
    fn unlimited_status_has_no_countdown_copy() {
        assert_eq!(
            normal_status_text(50.0, i64::MAX, Some("12:34")),
            "最終更新 12:34"
        );
        let state = CodexInfoState::preview("unlimited");
        assert!(!state.has_quota_percent);
        assert!(state.reset_at.is_none());
        assert!(state.model_usage.is_empty());
        assert_eq!(state.quota_title, "利用枠");
        assert_eq!(state.normal_status(), "最終更新 12:34");
    }

    #[test]
    fn monthly_copy_avoids_zero_day_and_zero_hour_phrases() {
        let text = period_remaining_text(30 * 60, 31 * 86_400, true);
        assert!(text.contains("月間、あと30分"));
        assert!(!text.contains("0日"));
        assert!(!text.contains("0時間"));
    }

    #[test]
    fn history_periods_navigate_newest_older_and_newer() {
        let mut history = UsageHistory::default();
        for (timestamp, reset_at) in [(100, 300), (200, 200), (300, 100)] {
            history.samples.push(UsageHistorySample::new(
                timestamp,
                reset_at,
                80.0,
                ModelDollarTotals::default(),
            ));
        }
        assert_eq!(history.reset_periods_desc(), vec![100, 200, 300]);
    }

    #[test]
    fn percentage_precision_is_limited_to_one_decimal() {
        assert_eq!(format_percent(64.04), "64.0%");
        assert_eq!(format_percent(64.0), "64%");
    }

    #[test]
    fn status_does_not_repeat_the_countdown() {
        assert_eq!(
            normal_status_text(5.0, 19 * 3_600, Some("12:34")),
            "残り利用枠が少なくなっています。"
        );
        assert_eq!(
            normal_status_text(50.0, 19 * 3_600, Some("12:34")),
            "リセット前後24時間です。"
        );
    }

    #[test]
    fn reset_warning_preview_exposes_the_reset_notice_without_low_quota_precedence() {
        let state = CodexInfoState::preview("reset-warning");
        assert_eq!(state.status, "リセット前後24時間です。");
        assert_eq!(state.status_level(), "warning");
    }

    #[test]
    fn refresh_copy_has_one_display_owner() {
        let slint = include_str!("../ui/app.slint");
        let rust = include_str!("main.rs");
        let rust_production = rust
            .split_once("#[cfg(test)]\nmod tests {")
            .map_or(rust, |(production, _)| production);
        let old_interval_copy = ["1分ごと", "に更新"].concat();
        assert!(!slint.contains(&old_interval_copy));
        assert!(!rust_production.contains(&old_interval_copy));
        assert_eq!(slint.matches("自動更新").count(), 0);
        assert_eq!(slint.matches("確認中…").count(), 0);
        assert!(rust_production.contains("最終更新 {}"));
    }

    #[test]
    fn account_activity_places_model_counts_on_a_separate_row() {
        let slint = include_str!("../ui/components.slint");
        let account = slint
            .split("export component AccountActivity inherits Rectangle {")
            .nth(1)
            .and_then(|body| {
                body.split("export component ThreadsWindow inherits Window {")
                    .next()
            })
            .expect("AccountActivity");
        assert!(account.contains("text: root.active-thread-count-label;"));
        assert!(account.contains("text: root.strings.model-threads;"));
        assert!(account.contains("label: \"SOL\";"));
        assert!(account.contains("label: \"TERRA\";"));
        assert!(account.contains("label: \"LUNA\";"));
        assert!(account.contains("label: root.strings.other;"));
        assert!(account.contains("x: parent.width - 112px;"));
        assert!(account.contains("width: 100px;\n        height: 24px;"));
    }

    #[test]
    fn dollar_graph_is_presented_as_independent_lines() {
        let slint = include_str!("../ui/components.slint");
        assert!(slint.contains("root.strings.graph-dollar-description"));
        assert!(!slint.contains("累積消費ドル（積み上げ）"));
        assert!(slint.contains("model: root.metric-options;"));
        assert!(slint.contains("current-index: root.selected-metric-index;"));
        assert!(!slint.contains("current-value:"));
        for marker in [
            "current-remaining-connector-path",
            "current-sol-connector-path",
            "current-terra-connector-path",
            "current-luna-connector-path",
            "current-label-gap: 10px;",
            "current-label-width: root.show-tokens ? 112px : 80px;",
            "current-label-right-padding: 4px;",
        ] {
            assert!(
                slint.contains(marker),
                "missing graph label mapping: {marker}"
            );
        }
        // Connector coordinates are normalized to the 0..100 viewbox. They
        // must fill the narrow label gap; otherwise Slint treats the values as
        // raw pixels and paints stray lines near the plot center/top.
        assert_eq!(
            slint
                .matches("fit: fill;\n                commands: root.current-")
                .count(),
            4
        );
        // An open SVG path must never be implicitly closed and painted to the
        // baseline; that was the visual source of the old stacked-area graph.
        assert_eq!(slint.matches("fill: transparent;").count(), 11);
    }

    #[test]
    fn model_usage_is_explicitly_token_based() {
        assert_eq!(
            format_model_usage_columns(&[
                preview_model_row("SOL", 1_234_567, 1_234_567, 234_567, 234_567),
                preview_model_row("TERRA", 99, 99, 0, 0),
                preview_model_row("LUNA", 42, 42, 0, 0),
            ]),
            (
                "SOL\nTERRA\nLUNA".into(),
                "1,000,000\n99\n42".into(),
                "$5\n$0\n$0".into(),
                "234,567\n0\n0".into(),
                "$0\n$0\n$0".into(),
                "234,567\n0\n0".into(),
                "$7\n$0\n$0".into()
            )
        );
    }

    #[test]
    fn model_rows_exclude_unknown_models() {
        let mut totals = ModelUsageTotals::default();
        totals.add(
            "gpt-5.6-sol",
            TokenSnapshot {
                total: 10,
                input: 8,
                cached_input: 2,
                output: 2,
            },
        );
        totals.add(
            "some-other-model",
            TokenSnapshot {
                total: 999,
                input: 999,
                cached_input: 0,
                output: 0,
            },
        );
        assert_eq!(
            totals
                .rows()
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["SOL"]
        );
    }

    #[test]
    fn nested_session_events_preserve_the_sol_model_for_token_counts() {
        let context = json!({
            "type": "event_msg",
            "payload": {"type": "turn_context", "model": "gpt-5.6-sol"}
        });
        assert_eq!(session_event_type(&context), Some("turn_context"));
        assert_eq!(
            session_event_model(&context).as_deref(),
            Some("gpt-5.6-sol")
        );
        let top_level_context = json!({"type": "turn_context", "model": "gpt-5.6-sol"});
        assert_eq!(
            session_event_model(&top_level_context).as_deref(),
            Some("gpt-5.6-sol")
        );

        let token_count = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {"total_token_usage": {
                    "total_tokens": 120,
                    "input_tokens": 100,
                    "cached_input_tokens": 80,
                    "output_tokens": 20
                }}
            }
        });
        assert_eq!(session_event_type(&token_count), Some("token_count"));
        assert_eq!(session_token_snapshot(&token_count).unwrap().total, 120);
    }

    #[test]
    fn oversized_tool_records_do_not_hide_following_usage_samples() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-oversized-session-{}.jsonl",
            std::process::id()
        ));
        let context = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "turn_context",
            "model": "gpt-5.6-luna"
        });
        let token_count = json!({
            "timestamp": "2026-08-11T10:00:02Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": {
                "total_tokens": 120, "input_tokens": 100,
                "cached_input_tokens": 80, "output_tokens": 20
            }}}
        });
        let oversized = format!(
            "{{\"type\":\"response_item\",\"payload\":\"{}\"}}",
            "x".repeat(security::MAX_JSONL_LINE_BYTES + 128)
        );
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&context).unwrap(),
                oversized,
                serde_json::to_string(&token_count).unwrap()
            ),
        )
        .unwrap();
        let mut totals = ModelUsageTotals::default();
        collect_session_file(&path, &mut totals, 0).unwrap();
        assert_eq!(totals.luna.tokens, 120);
        assert_eq!(totals.luna.output_tokens, 20);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn malformed_tool_records_do_not_hide_following_usage_samples() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-malformed-session-{}.jsonl",
            std::process::id()
        ));
        let context = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "turn_context",
            "model": "gpt-5.6-luna"
        });
        let token_count = json!({
            "timestamp": "2026-08-11T10:00:02Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": {
                "total_tokens": 120, "input_tokens": 100,
                "cached_input_tokens": 80, "output_tokens": 20
            }}}
        });
        fs::write(
            &path,
            format!(
                "{}\n{{\"type\":\"response_item\",\"payload\":\n{}\n",
                serde_json::to_string(&context).unwrap(),
                serde_json::to_string(&token_count).unwrap()
            ),
        )
        .unwrap();
        let mut totals = ModelUsageTotals::default();
        collect_session_file(&path, &mut totals, 0).unwrap();
        assert_eq!(totals.luna.tokens, 120);
        assert_eq!(totals.luna.output_tokens, 20);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unterminated_session_record_rolls_back_the_whole_local_input() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-unterminated-session-{}.jsonl",
            std::process::id()
        ));
        let valid = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": {
                "total_tokens": 120, "input_tokens": 100,
                "cached_input_tokens": 80, "output_tokens": 20
            }}}
        });
        let mut bytes = serde_json::to_vec(&valid).unwrap();
        bytes.extend_from_slice(b"\n{\xff");
        fs::write(&path, bytes).unwrap();
        let mut totals = ModelUsageTotals::default();
        totals.luna.tokens = 7;
        let before = totals.clone();
        let error = collect_session_file(&path, &mut totals, 0)
            .expect_err("EOF-incomplete local record must fail closed");
        assert_eq!(error.kind(), security::SecurityErrorKind::Unterminated);
        assert_eq!(totals.luna.tokens, before.luna.tokens);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn valid_json_unterminated_session_record_rolls_back_the_whole_local_input() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-valid-unterminated-session-{}.jsonl",
            std::process::id()
        ));
        let valid = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": {
                "total_tokens": 120, "input_tokens": 100,
                "cached_input_tokens": 80, "output_tokens": 20
            }}}
        });
        let mut bytes = serde_json::to_vec(&valid).unwrap();
        bytes.extend_from_slice(b"\n{}");
        fs::write(&path, bytes).unwrap();
        let mut totals = ModelUsageTotals::default();
        totals.luna.tokens = 7;
        let before = totals.clone();
        let error = collect_session_file(&path, &mut totals, 0)
            .expect_err("valid EOF-incomplete local record must fail closed");
        assert_eq!(error.kind(), security::SecurityErrorKind::Unterminated);
        assert_eq!(totals.luna.tokens, before.luna.tokens);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn oversized_unterminated_session_record_rolls_back_the_whole_local_input() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-oversized-unterminated-session-{}.jsonl",
            std::process::id()
        ));
        let prefix = json!({
            "timestamp": "2026-08-11T10:00:00Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"total_token_usage": {
                "total_tokens": 120, "input_tokens": 100,
                "cached_input_tokens": 80, "output_tokens": 20
            }}}
        });
        let mut bytes = serde_json::to_vec(&prefix).unwrap();
        bytes.extend_from_slice(b"\n");
        bytes.extend(std::iter::repeat_n(
            b'x',
            security::MAX_JSONL_LINE_BYTES + 1,
        ));
        fs::write(&path, bytes).unwrap();
        let mut totals = ModelUsageTotals::default();
        totals.luna.tokens = 7;
        let before = totals.clone();
        let error = collect_session_file(&path, &mut totals, 0)
            .expect_err("oversized EOF-incomplete local record must fail closed");
        assert_eq!(error.kind(), security::SecurityErrorKind::Unterminated);
        assert_eq!(totals.luna.tokens, before.luna.tokens);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn session_collector_counts_sol_when_model_context_is_nested() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-sol-session-{}.jsonl",
            std::process::id()
        ));
        let lines = [
            json!({
                "timestamp": "2026-08-11T10:00:00Z",
                "type": "event_msg",
                "payload": {"type": "turn_context", "model": "gpt-5.6-sol"}
            }),
            json!({
                "timestamp": "2026-08-11T10:00:01Z",
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"total_token_usage": {
                    "total_tokens": 100, "input_tokens": 80,
                    "cached_input_tokens": 20, "output_tokens": 20
                }}}
            }),
            json!({
                "timestamp": "2026-08-11T10:00:02Z",
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"total_token_usage": {
                    "total_tokens": 150, "input_tokens": 120,
                    "cached_input_tokens": 30, "output_tokens": 30
                }}}
            }),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(|line| serde_json::to_string(line).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let mut totals = ModelUsageTotals::default();
        collect_session_file(&path, &mut totals, 0).unwrap();
        assert_eq!(totals.sol.tokens, 150);
        assert_eq!(totals.sol.output_tokens, 30);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn session_settings_do_not_reassign_tokens_before_the_next_turn_context() {
        let path = std::env::temp_dir().join(format!(
            "codex-info-model-switch-{}.jsonl",
            std::process::id()
        ));
        let lines = [
            json!({
                "timestamp": "2026-08-11T10:00:00Z",
                "type": "turn_context",
                "model": "gpt-5.6-luna"
            }),
            json!({
                "timestamp": "2026-08-11T10:00:01Z",
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"total_token_usage": {
                    "total_tokens": 100, "input_tokens": 80,
                    "cached_input_tokens": 20, "output_tokens": 20
                }}}
            }),
            json!({
                "timestamp": "2026-08-11T10:00:02Z",
                "type": "event_msg",
                "payload": {"type": "thread_settings_applied",
                    "thread_settings": {"model": "gpt-5.6-sol"}}
            }),
            json!({
                "timestamp": "2026-08-11T10:00:03Z",
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"total_token_usage": {
                    "total_tokens": 150, "input_tokens": 120,
                    "cached_input_tokens": 30, "output_tokens": 30
                }}}
            }),
            json!({
                "timestamp": "2026-08-11T10:00:04Z",
                "type": "turn_context",
                "model": "gpt-5.6-sol"
            }),
            json!({
                "timestamp": "2026-08-11T10:00:05Z",
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"total_token_usage": {
                    "total_tokens": 200, "input_tokens": 160,
                    "cached_input_tokens": 40, "output_tokens": 40
                }}}
            }),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(|line| serde_json::to_string(line).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let mut totals = ModelUsageTotals::default();
        collect_session_file(&path, &mut totals, 0).unwrap();
        assert_eq!(totals.luna.tokens, 150);
        assert_eq!(totals.sol.tokens, 50);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn week_text_includes_minutes_for_a_full_countdown() {
        assert_eq!(
            week_remaining_text(6 * 86_400 + 9 * 3_600 + 12 * 60),
            "7日中、あと6日と9時間12分"
        );
    }

    #[test]
    fn history_replaces_a_minute_without_discarding_the_previous_reset_period() {
        let mut history = UsageHistory::default();
        let previous_reset_at = 1_700_100_000;
        let next_reset_at = 1_700_200_000;
        history.record(UsageHistorySample::new(
            1_700_000_001,
            previous_reset_at,
            80.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 2.0,
                luna: 3.0,
            },
        ));
        history.record(UsageHistorySample::new(
            1_700_000_039,
            previous_reset_at,
            75.0,
            ModelDollarTotals {
                sol: 4.0,
                terra: 5.0,
                luna: 6.0,
            },
        ));
        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].remaining_percent, 75.0);
        assert_eq!(history.samples[0].sol_dollars, 4.0);

        history.record(UsageHistorySample::new(
            1_700_000_120,
            next_reset_at,
            70.0,
            ModelDollarTotals::default(),
        ));
        assert_eq!(history.samples.len(), 2);
        assert!(history
            .samples
            .iter()
            .any(|sample| sample.reset_at == previous_reset_at));
        assert!(history
            .samples
            .iter()
            .any(|sample| sample.reset_at == next_reset_at));
        assert_eq!(history.samples_for_reset(Some(previous_reset_at)).len(), 1);
        assert_eq!(history.samples_for_reset(Some(next_reset_at)).len(), 1);
    }

    #[test]
    fn reset_at_jitter_is_one_period_and_duplicate_timestamps_merge_for_display() {
        let mut history = UsageHistory::default();
        let first = UsageHistorySample::new(
            1_700_000_001,
            1_700_100_000,
            80.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 2.0,
                luna: 3.0,
            },
        );
        let jittered = UsageHistorySample::new(
            1_700_000_039,
            1_700_100_003,
            75.0,
            ModelDollarTotals {
                sol: 4.0,
                terra: 5.0,
                luna: 6.0,
            },
        );

        history.record(first);
        history.record(jittered);

        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].reset_at, 1_700_100_000);
        assert_eq!(history.samples[0].remaining_percent, 75.0);
        assert_eq!(history.samples_for_reset(Some(1_700_100_003)).len(), 1);

        history.samples = vec![
            UsageHistorySample::new(
                1_700_000_000,
                1_700_100_000,
                80.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 2.0,
                    luna: 3.0,
                },
            ),
            UsageHistorySample::new(
                1_700_000_000,
                1_700_100_003,
                75.0,
                ModelDollarTotals {
                    sol: 4.0,
                    terra: 5.0,
                    luna: 6.0,
                },
            ),
        ];
        let displayed = history.samples_for_reset(Some(1_700_100_000));
        assert_eq!(displayed.len(), 1);
        // A sixty-second jitter group uses its greatest observed reset time as
        // the stable period identifier.
        assert_eq!(displayed[0].reset_at, 1_700_100_003);
        assert_eq!(displayed[0].remaining_percent, 75.0);
        assert_eq!(displayed[0].sol_dollars, 4.0);
    }

    #[test]
    fn session_backfill_keeps_an_observed_remaining_value() {
        let mut history = UsageHistory {
            samples: vec![UsageHistorySample::new(
                60,
                1_000,
                42.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 1.0,
                    luna: 1.0,
                },
            )],
            ..UsageHistory::default()
        };
        let backfill = UsageHistorySample::from_model_history(
            60,
            1_003,
            ModelDollarTotals {
                sol: 9.0,
                terra: 8.0,
                luna: 7.0,
            },
        );

        history.apply_backfill_samples(1_003, vec![backfill]);

        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].remaining_percent, 42.0);
        assert_eq!(history.samples[0].sol_dollars, 9.0);
        assert_eq!(history.samples[0].reset_at, 1_000);
    }

    #[test]
    fn record_rejects_alias_quota_collision_before_canonical_merge() {
        let mut history = UsageHistory::default();
        let base_reset = 2_000_000_i64;
        history.record(UsageHistorySample::new(
            960_000,
            base_reset,
            88.0,
            ModelDollarTotals {
                sol: 1.0,
                ..ModelDollarTotals::default()
            },
        ));
        history.record(UsageHistorySample::new(
            960_000,
            base_reset + 30,
            14.0,
            ModelDollarTotals::default(),
        ));

        let selected = history.samples_for_reset(Some(base_reset));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].remaining_percent, -1.0);
        assert_eq!(selected[0].sol_dollars, 1.0);
        assert!(!selected
            .iter()
            .any(|sample| (sample.remaining_percent - 14.0).abs() < f64::EPSILON));
    }

    #[test]
    fn same_timestamp_reset_drift_above_jitter_fails_closed() {
        let reset_at = 2_000_000_i64;
        let history = UsageHistory {
            samples: vec![
                UsageHistorySample::new(
                    960_000,
                    reset_at,
                    97.0,
                    ModelDollarTotals {
                        luna: 7.6,
                        ..ModelDollarTotals::default()
                    },
                ),
                // This mirrors the live history's 12-second reset drift: both
                // rows have model usage, but they cannot be ordered into a
                // fabricated 97% -> 98% quota transition.
                UsageHistorySample::new(
                    960_000,
                    reset_at + 12,
                    98.0,
                    ModelDollarTotals {
                        luna: 7.6,
                        ..ModelDollarTotals::default()
                    },
                ),
            ],
            ..UsageHistory::default()
        };

        let selected = history.samples_for_reset(Some(reset_at));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].remaining_percent, -1.0);
        assert_eq!(selected[0].luna_dollars, 7.6);
    }

    #[test]
    fn startup_load_sanitizes_legacy_same_timestamp_quota_collision() {
        let db_path = test_history_path("startup-same-timestamp-collision");
        let timestamp = 1_999_999_940;
        let rows = [
            UsageHistorySample {
                timestamp,
                reset_at: 2_000_000_500,
                remaining_percent: 88.0,
                sol_dollars: 1.0,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 0,
                terra_tokens: 0,
                luna_tokens: 0,
            },
            UsageHistorySample {
                timestamp,
                reset_at: 2_000_000_530,
                remaining_percent: 14.0,
                sol_dollars: 0.0,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 0,
                terra_tokens: 0,
                luna_tokens: 0,
            },
        ];
        let mut store = UsageStore::open(&db_path).unwrap();
        store
            .upsert_samples(
                &rows
                    .iter()
                    .map(UsageHistorySample::to_store)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        drop(store);

        let now = Utc.timestamp_opt(2_000_001_000, 0).single().unwrap();
        let history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), now);
        assert_eq!(
            history
                .samples
                .iter()
                .filter(|sample| sample.timestamp == timestamp)
                .count(),
            2
        );
        assert!(history
            .samples
            .iter()
            .filter(|sample| sample.timestamp == timestamp)
            .all(|sample| sample.remaining_percent < 0.0));
        assert!(history
            .canonical_samples()
            .iter()
            .filter(|sample| sample.timestamp == timestamp)
            .all(|sample| sample.remaining_percent < 0.0));
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn startup_maintenance_prunes_before_the_calendar_cutoff_only_once() {
        let db_path = test_history_path("startup-maintenance");
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let cutoff = three_months_before_utc(now);
        let samples = [
            UsageHistorySample {
                timestamp: cutoff - 1,
                reset_at: cutoff + 10_000,
                remaining_percent: 80.0,
                sol_dollars: 1.0,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 0,
                terra_tokens: 0,
                luna_tokens: 0,
            },
            UsageHistorySample {
                timestamp: cutoff,
                reset_at: cutoff + 20_000,
                remaining_percent: 70.0,
                sol_dollars: 2.0,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 0,
                terra_tokens: 0,
                luna_tokens: 0,
            },
        ];
        let mut store = UsageStore::open(&db_path).unwrap();
        store
            .upsert_samples(
                &samples
                    .iter()
                    .map(UsageHistorySample::to_store)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        drop(store);

        let mut history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), now);
        history.startup_maintenance(now);

        assert_eq!(
            history
                .samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            Vec::<i64>::new()
        );
        let persisted = UsageStore::open(&db_path).unwrap().load_all().unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].timestamp, cutoff);

        history.samples.push(samples[0].clone());
        history.startup_maintenance(now);
        assert!(history
            .samples
            .iter()
            .any(|sample| sample.timestamp == cutoff - 1));
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn startup_maintenance_still_bounds_visible_memory_when_store_pruning_fails() {
        let db_path = test_history_path("startup-maintenance-error");
        let db_path = db_path.parent().unwrap().join("not-a-database");
        fs::create_dir_all(&db_path).unwrap();
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let sample = |timestamp| UsageHistorySample {
            timestamp,
            reset_at: now.timestamp() + 1,
            remaining_percent: 80.0,
            sol_dollars: 1.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let mut history = UsageHistory {
            db_path: Some(db_path.clone()),
            samples: vec![
                sample(1),
                sample(now.timestamp()),
                sample(now.timestamp() + 1),
            ],
            startup_maintenance_done: false,
        };

        history.startup_maintenance(now);

        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].timestamp, now.timestamp());
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    fn write_recovery_ledger(name: &str, rows: &[serde_json::Value]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "codex-info-recovery-{name}-{}-{}.jsonl",
            std::process::id(),
            rows.len()
        ));
        let contents = rows
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn recovery_ledger_filters_invalid_unknown_duplicate_and_out_of_window_rows() {
        let path = write_recovery_ledger(
            "filters",
            &[
                json!({
                    "timestamp": 100,
                    "thread_id": "luna-thread",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 100,
                    "cached_input_tokens": 25,
                    "output_tokens": 30,
                    "reasoning_tokens": 40
                }),
                json!({
                    "timestamp": 101,
                    "thread_id": "luna-thread",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 900,
                    "cached_input_tokens": 0,
                    "output_tokens": 900,
                    "reasoning_tokens": 0
                }),
                json!({
                    "timestamp": 200,
                    "thread_id": "sol-thread",
                    "model": "gpt-5.6-sol",
                    "input_tokens": 8,
                    "cached_input_tokens": 2,
                    "output_tokens": 4,
                    "reasoning_tokens": 1
                }),
                json!({
                    "timestamp": 150,
                    "thread_id": "unknown-thread",
                    "model": "gpt-5.6-unknown",
                    "input_tokens": 1000,
                    "cached_input_tokens": 0,
                    "output_tokens": 1000,
                    "reasoning_tokens": 0
                }),
                json!({
                    "timestamp": 99,
                    "thread_id": "before-thread",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 1000,
                    "cached_input_tokens": 0,
                    "output_tokens": 1000,
                    "reasoning_tokens": 0
                }),
                json!({
                    "timestamp": 201,
                    "thread_id": "after-thread",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 1000,
                    "cached_input_tokens": 0,
                    "output_tokens": 1000,
                    "reasoning_tokens": 0
                }),
                json!({"timestamp": 150, "thread_id": "invalid"}),
                json!({
                    "timestamp": 150,
                    "thread_id": "blank-model",
                    "model": "",
                    "input_tokens": 1,
                    "cached_input_tokens": 0,
                    "output_tokens": 1,
                    "reasoning_tokens": 0
                }),
            ],
        );

        let entries = read_recovery_entries(&path, 100, 200);
        assert_eq!(entries.len(), 2);
        let mut totals = ModelUsageTotals::default();
        add_recovery_usage(Some(&path), 100, 200, &mut totals);
        assert_eq!(totals.luna.tokens, 130);
        assert_eq!(totals.luna.input_tokens, 100);
        assert_eq!(totals.luna.cached_input_tokens, 25);
        assert_eq!(totals.luna.output_tokens, 30);
        assert_eq!(totals.sol.tokens, 12);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recovery_ledger_events_become_timestamped_cumulative_timeline_points() {
        let path = write_recovery_ledger(
            "timeline",
            &[
                json!({
                    "timestamp": 120,
                    "thread_id": "first",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 100,
                    "cached_input_tokens": 20,
                    "output_tokens": 30,
                    "reasoning_tokens": 10
                }),
                json!({
                    "timestamp": 180,
                    "thread_id": "second",
                    "model": "gpt-5.6-luna",
                    "input_tokens": 50,
                    "cached_input_tokens": 0,
                    "output_tokens": 10,
                    "reasoning_tokens": 5
                }),
            ],
        );

        let events = recovery_timed_usage(&path, 120, 180);
        let samples = model_usage_timeline_from_events(events, 240);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].timestamp, 120);
        assert_eq!(samples[1].timestamp, 180);
        assert!(samples[1].luna_dollars > samples[0].luna_dollars);
        let _ = fs::remove_file(path);
    }

    fn test_history_path(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "codex-info-history-{name}-{}-{id}",
            std::process::id()
        ));
        root.join("usage_history.sqlite3")
    }

    #[test]
    fn sqlite_history_cutoff_and_period_list_integration() {
        let db_path = test_history_path("cutoff-period-list");
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let cutoff = three_months_before_utc(now);
        let record = |timestamp, reset_at, remaining_percent| UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let records = [
            record(cutoff - 1, cutoff + 10_000, 90.0),
            record(cutoff, cutoff + 20_000, 80.0),
            record(now.timestamp(), now.timestamp() + 30_000, 70.0),
            record(now.timestamp() + 1, now.timestamp() + 40_000, 60.0),
        ];
        let mut store = UsageStore::open(&db_path).unwrap();
        store
            .upsert_samples(
                &records
                    .iter()
                    .map(UsageHistorySample::to_store)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        drop(store);

        let mut history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), now);
        history.startup_maintenance(now);
        assert_eq!(
            history
                .samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            vec![now.timestamp()]
        );
        let periods = history.periods(now.timestamp(), Some(now.timestamp() + 30_000));
        assert_eq!(periods.len(), 1);
        assert_eq!(history.period_options(now.timestamp(), None).len(), 1);
        for period in periods {
            assert_eq!(
                history.period_id_for_label(
                    &period.label,
                    now.timestamp(),
                    Some(now.timestamp() + 30_000),
                ),
                Some(period.canonical_reset_at)
            );
        }
        let persisted = UsageStore::open(&db_path).unwrap().load_all().unwrap();
        assert_eq!(
            persisted
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            vec![cutoff, now.timestamp(), now.timestamp() + 1]
        );
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn usage_history_uses_sqlite_only_and_does_not_touch_json() {
        let db_path = test_history_path("sqlite-only");
        let json_path = db_path.with_extension("json");
        let legacy_contents = br#"[{"timestamp":1,"reset_at":2}]"#.to_vec();
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(&json_path, &legacy_contents).unwrap();

        let sqlite_sample = UsageHistorySample::new(
            1_700_000_120,
            1_700_200_000,
            70.0,
            ModelDollarTotals {
                sol: 4.0,
                terra: 5.0,
                luna: 6.0,
            },
        );
        let store = UsageStore::open(&db_path).unwrap();
        store.upsert_sample(&sqlite_sample.to_store()).unwrap();
        drop(store);

        let load_at = chrono::DateTime::<Utc>::from_timestamp(1_700_000_180, 0).unwrap();
        let mut history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), load_at);
        assert_eq!(history.samples, vec![sqlite_sample]);
        history.record(UsageHistorySample::new(
            1_700_000_180,
            1_700_200_000,
            60.0,
            ModelDollarTotals::default(),
        ));
        assert_eq!(fs::read(&json_path).unwrap(), legacy_contents);

        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn history_period_grouping_boundary_and_label_mapping_are_unambiguous() {
        let db_path = test_history_path("period-grouping-contract");
        let samples = [1_000, 1_060, 1_061, 1_121, 1_122].map(|reset_at| {
            UsageHistorySample::new(100, reset_at, 80.0, ModelDollarTotals::default())
        });
        let mut store = UsageStore::open(&db_path).unwrap();
        store
            .upsert_samples(
                &samples
                    .iter()
                    .map(UsageHistorySample::to_store)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        drop(store);
        let load_at = chrono::DateTime::<Utc>::from_timestamp(100, 0).unwrap();
        let history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), load_at);

        let periods = history.periods(2_000, None);
        assert_eq!(
            periods
                .iter()
                .map(|period| period.canonical_reset_at)
                .collect::<Vec<_>>(),
            vec![1_122, 1_121, 1_060]
        );
        let labels = periods
            .iter()
            .map(|period| period.label.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(labels.len(), periods.len());
        assert!(
            periods
                .iter()
                .filter(|period| period.label.contains("（期限 "))
                .count()
                >= 2
        );
        assert!(periods
            .iter()
            .filter(|period| period.label.contains("（期限 "))
            .all(|period| period.label.contains("JST")));
        for period in &periods {
            assert_eq!(
                history.period_id_for_label(&period.label, 2_000, None),
                Some(period.canonical_reset_at)
            );
            assert_eq!(
                history
                    .samples_for_reset(Some(period.canonical_reset_at))
                    .len(),
                1
            );
        }
        assert_eq!(
            UsageHistory::default().period_options(2_000, None),
            vec!["履歴なし"]
        );
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn moving_reset_snapshots_stay_in_one_period_and_keep_model_values() {
        let samples = [
            UsageHistorySample::new_with_usage(
                1_800_000_000,
                1_800_604_800,
                80.0,
                ModelDollarTotals {
                    sol: 3.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 30,
                    ..ModelTokenTotals::default()
                },
            ),
            // The quota endpoint moved reset_at by the same 120 seconds as
            // the observation. These are still one weekly period.
            UsageHistorySample::new_with_usage(
                1_800_000_120,
                1_800_604_920,
                70.0,
                ModelDollarTotals {
                    sol: 9.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 90,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                1_800_000_240,
                1_800_605_040,
                60.0,
                ModelDollarTotals {
                    sol: 12.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 120,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        let history = UsageHistory {
            samples: samples.to_vec(),
            ..UsageHistory::default()
        };

        let periods = history.periods(1_800_700_000, None);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].canonical_reset_at, 1_800_605_040);
        let selected = history.samples_for_reset(Some(1_800_605_040));
        assert_eq!(selected.len(), 3);
        assert_eq!(selected[1].sol_dollars, 9.0);
        assert_eq!(selected[2].sol_tokens, 120);
        assert!(history
            .canonical_samples()
            .iter()
            .all(|sample| sample.reset_at == 1_800_605_040));
    }

    #[test]
    fn historical_week_fixture_preserves_each_period_and_graph_samples() {
        // Regression fixture for the real failure class: multiple weekly
        // periods are present in one acquisition window and reset boundaries
        // are far apart, so a moving-reset matcher must not drop or mix rows.
        let first_reset = 1_700_604_800;
        let second_reset = first_reset + WEEK_SECONDS;
        let third_reset = second_reset + WEEK_SECONDS;
        let samples = vec![
            UsageHistorySample::new_with_usage(
                1_700_000_000,
                first_reset,
                80.0,
                ModelDollarTotals {
                    sol: 10.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 100,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                first_reset - 60,
                first_reset,
                70.0,
                ModelDollarTotals {
                    sol: 20.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 200,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                first_reset,
                second_reset,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                second_reset - 60,
                second_reset,
                60.0,
                ModelDollarTotals {
                    luna: 30.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 300,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                second_reset,
                third_reset,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
        ];
        let history = UsageHistory {
            samples: samples.clone(),
            ..UsageHistory::default()
        };
        let periods = history.periods(third_reset + 60, None);
        assert_eq!(
            periods
                .iter()
                .map(|period| period.canonical_reset_at)
                .collect::<Vec<_>>(),
            vec![third_reset, second_reset, first_reset]
        );

        let grouped_count = periods
            .iter()
            .map(|period| {
                history
                    .samples_for_reset(Some(period.canonical_reset_at))
                    .len()
            })
            .sum::<usize>();
        assert_eq!(grouped_count, samples.len());
        for period in periods {
            let selected = history.samples_for_reset(Some(period.canonical_reset_at));
            assert!(selected
                .iter()
                .all(|sample| sample.reset_at == period.canonical_reset_at));
        }
    }

    #[test]
    fn affected_timestamp_does_not_mix_a_singleton_reset_period_into_history() {
        // Exact collision shape observed in the user's 2026-08-22 07:17 JST
        // history: the primary period reported 88%, while an unrelated
        // singleton reset period reported 14% at the same minute.  Selecting
        // the spend period must retain 88% and exclude the singleton entirely.
        let timestamp = 1_787_350_620;
        let primary_reset = 1_787_835_664;
        let samples = vec![
            UsageHistorySample::new(
                timestamp,
                1_787_835_614,
                88.0,
                ModelDollarTotals {
                    terra: 30.5047316,
                    luna: 6.38893528,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::from_model_history(
                timestamp,
                1_787_835_661,
                ModelDollarTotals {
                    terra: 30.5047316,
                    luna: 6.38893528,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::from_model_history(
                timestamp,
                1_787_835_662,
                ModelDollarTotals {
                    sol: 140.97,
                    terra: 30.5047316,
                    luna: 6.38893528,
                },
            ),
            UsageHistorySample::from_model_history(
                timestamp,
                1_787_835_663,
                ModelDollarTotals {
                    sol: 420.405,
                    terra: 30.5047316,
                    luna: 6.38893528,
                },
            ),
            UsageHistorySample::from_model_history(
                timestamp,
                primary_reset,
                ModelDollarTotals {
                    sol: 420.405,
                    terra: 30.5047316,
                    luna: 6.38893528,
                },
            ),
            UsageHistorySample::new(timestamp, 1_787_919_479, 14.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                timestamp + 60,
                primary_reset,
                87.0,
                ModelDollarTotals {
                    sol: 420.405,
                    terra: 30.5047316,
                    luna: 6.38893528,
                },
            ),
        ];
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        let periods = history.periods(timestamp + 60, None);
        let selected = history.samples_for_reset(Some(primary_reset));
        assert!(periods.iter().any(|period| {
            period.canonical_reset_at == primary_reset && period.start == timestamp
        }));
        assert!(selected.iter().any(|sample| {
            sample.timestamp == timestamp && (sample.remaining_percent - 88.0).abs() < f64::EPSILON
        }));
        assert!(selected
            .iter()
            .all(|sample| (sample.remaining_percent - 14.0).abs() > f64::EPSILON));
        assert!(selected
            .iter()
            .all(|sample| sample.reset_at == primary_reset));
    }

    #[test]
    fn ambiguous_missing_quota_row_at_a_spend_timestamp_is_not_a_period() {
        // A legacy quota-only row at the same minute as a model snapshot can
        // be invalidated to remaining=-1 during startup. It must not survive
        // as a separate zero-usage history period.
        let timestamp = 1_787_350_620;
        let spend_reset = 1_787_835_664;
        let ambiguous_reset = 1_787_919_479;
        let history = UsageHistory {
            samples: vec![
                UsageHistorySample::new(
                    timestamp,
                    spend_reset,
                    88.0,
                    ModelDollarTotals {
                        terra: 30.5,
                        luna: 6.3,
                        ..ModelDollarTotals::default()
                    },
                ),
                UsageHistorySample {
                    timestamp,
                    reset_at: ambiguous_reset,
                    remaining_percent: -1.0,
                    sol_dollars: 0.0,
                    terra_dollars: 0.0,
                    luna_dollars: 0.0,
                    sol_tokens: 0,
                    terra_tokens: 0,
                    luna_tokens: 0,
                },
            ],
            ..UsageHistory::default()
        };

        assert_eq!(history.periods(timestamp + 60, None).len(), 1);
        assert!(history.samples_for_reset(Some(ambiguous_reset)).is_empty());
    }

    #[test]
    fn observed_moving_reset_sequence_keeps_the_spend_in_the_selected_graph() {
        // This is the shape found in the affected database: reset_at advances
        // with each observation, while the first few snapshots are idle and
        // the later snapshots contain the actual spend.
        let observations = [
            (1_787_540_040, 1_788_144_861),
            (1_787_540_100, 1_788_144_861),
            (1_787_540_100, 1_788_144_930),
            (1_787_540_160, 1_788_144_930),
            (1_787_540_160, 1_788_144_980),
            (1_787_540_220, 1_788_144_980),
            (1_787_540_220, 1_788_145_050),
            (1_787_540_280, 1_788_145_050),
            (1_787_540_280, 1_788_145_101),
            (1_787_540_340, 1_788_145_101),
        ];
        let samples = observations
            .into_iter()
            .enumerate()
            .map(|(index, (timestamp, reset_at))| {
                UsageHistorySample::new_with_usage(
                    timestamp,
                    reset_at,
                    100.0,
                    ModelDollarTotals {
                        luna: if index < 8 { 0.0 } else { 0.01528528 },
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals {
                        luna: if index < 8 { 0 } else { 255_973 },
                        ..ModelTokenTotals::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        assert!(super::moving_reset_observation_belongs_to_anchor(
            &history.samples[0],
            &history.samples[4]
        ));

        let periods = history.periods(1_788_145_200, None);
        assert_eq!(periods.len(), 1);
        let selected = history.samples_for_reset(Some(periods[0].canonical_reset_at));
        assert_eq!(selected.len(), 6);
        let references = selected.iter().collect::<Vec<_>>();
        let graph = graph_paths_for_selection(
            &references,
            periods[0].start,
            periods[0].end,
            true,
            false,
            false,
            false,
        );
        assert!(graph.luna_rising.contains('L'));
        assert_eq!(graph.current_luna_label, "$0.02");
    }

    #[test]
    fn affected_period_keeps_sol_spend_and_unobserved_quota_distinct() {
        // Regression fixture for the 8/20-8/24 history: the reset deadline
        // moves by a few seconds, session backfill contributes SOL totals
        // without a quota observation, and only the final quota poll reports
        // the 1% balance. The period must stay a single selection, while the
        // missing quota interval must not be turned into a fabricated slope.
        let base = 1_999_999_980;
        let reset = base + 10_000;
        let samples = vec![
            UsageHistorySample::new_with_usage(
                base,
                reset,
                87.0,
                ModelDollarTotals {
                    terra: 30.50,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::from_model_history(
                base + 60,
                reset + 47,
                ModelDollarTotals {
                    sol: 140.97,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::from_model_history(
                base + 120,
                reset + 48,
                ModelDollarTotals {
                    sol: 420.40,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                base + 240,
                reset + 50,
                1.0,
                ModelDollarTotals {
                    sol: 420.40,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals::default(),
            ),
        ];
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        let periods = history.periods(base + 600, None);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].canonical_reset_at, reset + 50);
        let selected = history.samples_for_reset(Some(reset + 50));
        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .map(|sample| sample.sol_dollars)
                .fold(0.0, f64::max),
            420.40
        );
        assert_eq!(selected.last().unwrap().remaining_percent, 1.0);

        let references = selected.iter().collect::<Vec<_>>();
        let remaining = remaining_graph_points(&references, base, base + 300);
        assert_eq!(
            remaining,
            vec![
                (base, 87.0),
                (base + 60, 44.0),
                (base + 120, 1.0),
                (base + 240, 1.0),
                (base + 300, 1.0)
            ]
        );
    }

    #[test]
    fn shared_graph_fixture_is_the_x_history_oracle() {
        const SPEC_REMAINING: [f64; 6] = [100.0, 87.0, 44.0, 1.0, 1.0, 1.0];
        const SPEC_SOL_MAX: f64 = 420.40;
        const SPEC_PERIOD_COUNT: usize = 1;
        let fixture: GraphFixture =
            serde_json::from_str(include_str!("../tests/fixtures/graph_delayed_quota.json"))
                .expect("valid shared graph fixture");
        // These literals are the reviewed acceptance oracle. The fixture is
        // test input, not an implementation-generated expected-value source.
        assert_eq!(fixture.expected_remaining, SPEC_REMAINING);
        assert_eq!(fixture.expected_sol_max, SPEC_SOL_MAX);
        assert_eq!(fixture.expected_period_count, SPEC_PERIOD_COUNT);
        assert_eq!(fixture.expected_raw_timestamps.len(), 5);
        assert_eq!(fixture.expected_graph_timestamps.len(), 6);

        let wire = &fixture.details_response;
        assert_eq!(wire.api_version, "v1");
        assert_eq!(wire.state, "ready");
        assert!(wire.authenticated);
        assert_eq!(wire.active_thread_count, 0);
        assert_eq!(wire.models.len(), 3);
        assert!(wire.history_gaps.is_empty());
        assert!(wire.threads.is_empty());
        assert_eq!(wire.estimated_cost_label, "概算 $451");
        assert_eq!(wire.quota.reset_at, fixture.expected_reset_at);
        assert!(wire.quota.window_seconds > 0);
        let wire_period = wire
            .history_periods
            .first()
            .expect("fixture has one history period");
        assert_eq!(wire.history_periods.len(), SPEC_PERIOD_COUNT);
        assert_eq!(wire_period.id, fixture.expected_reset_at.to_string());
        assert_eq!(wire_period.start_at, fixture.expected_period_start);
        assert_eq!(wire_period.end_at, fixture.expected_period_end);
        assert_eq!(wire_period.reset_at, fixture.expected_reset_at);
        assert_eq!(wire_period.label, "history");
        assert!(wire_period.current);
        assert_eq!(
            wire.history_samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_raw_timestamps
        );

        let state = state_from_graph_fixture(&fixture);
        let observed_at = wire.observed_at;
        let expected_samples = wire
            .history_samples
            .iter()
            .map(GraphFixtureHistorySample::to_usage_history_sample)
            .collect::<Vec<_>>();
        let periods = state.history_periods_at(observed_at);
        assert_eq!(periods.len(), SPEC_PERIOD_COUNT);
        let period = periods.first().expect("state has one current period");
        assert_eq!(period.start, fixture.expected_period_start);
        assert_eq!(period.end, fixture.expected_period_end);
        assert_eq!(period.canonical_reset_at, fixture.expected_reset_at);

        let selected = state
            .history
            .samples_for_reset(Some(period.canonical_reset_at));
        assert_eq!(selected.len(), fixture.expected_raw_timestamps.len());
        assert_eq!(selected, expected_samples);
        assert_eq!(
            selected
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_raw_timestamps
        );
        assert_eq!(
            selected.last().map(|sample| sample.timestamp),
            Some(fixture.expected_period_end)
        );

        let references = selected.iter().collect::<Vec<_>>();
        let remaining = remaining_graph_points(
            &references,
            fixture.expected_period_start,
            fixture.expected_period_end,
        );
        let remaining_by_timestamp = remaining.iter().fold(BTreeMap::new(), |mut values, point| {
            values.insert(point.0, point.1);
            values
        });
        assert_eq!(remaining_by_timestamp.len(), SPEC_REMAINING.len());
        assert_eq!(
            remaining_by_timestamp.keys().copied().collect::<Vec<_>>(),
            fixture.expected_graph_timestamps
        );
        for (point, expected) in remaining_by_timestamp.values().zip(SPEC_REMAINING) {
            assert!((point - expected).abs() < 0.000_001);
        }
        assert_eq!(
            selected
                .iter()
                .map(|sample| sample.sol_dollars)
                .fold(0.0, f64::max),
            SPEC_SOL_MAX
        );
        assert_eq!(
            remaining.first(),
            Some(&(fixture.expected_period_start, 100.0))
        );
        assert_eq!(remaining.get(1).map(|point| point.1), Some(100.0));
        assert_eq!(remaining.get(2).map(|point| point.1), Some(87.0));
        assert_eq!(
            remaining.get(1).map(|point| point.0),
            remaining.get(2).map(|point| point.0)
        );
        assert!(remaining.windows(2).any(|pair| {
            pair[0].0 == pair[1].0 && (pair[0].1 - pair[1].1).abs() > f64::EPSILON
        }));
        assert!(!remaining.windows(2).take(2).any(|pair| {
            pair[0].0 < selected[0].timestamp
                && pair[1].0 == selected[0].timestamp
                && (pair[0].1 - pair[1].1).abs() > f64::EPSILON
        }));

        let minute = graph_time_endpoints(
            minute_model_spend_for_metric(&references, false),
            fixture.expected_period_start,
            fixture.expected_period_end,
        );
        assert_eq!(
            minute
                .iter()
                .map(|point| point.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_graph_timestamps
        );
        let graph = state.graph_paths_for_selection_at(observed_at, true, true, true, false);
        let expected_graph = graph_paths_for_selection(
            &references,
            fixture.expected_period_start,
            fixture.expected_period_end,
            true,
            true,
            true,
            false,
        );
        assert_eq!(graph.remaining, expected_graph.remaining);
        assert!(graph
            .unused_intervals
            .iter()
            .any(|interval| { interval.start.abs() < 0.000_001 && interval.width > 0.0 }));

        let labels = state.graph_time_labels_at(observed_at);
        let expected_labels = [0.0, 0.25, 0.5, 0.75, 1.0].map(|fraction| {
            let span = (fixture.expected_period_end - fixture.expected_period_start) as f64;
            let timestamp = fixture.expected_period_start + (span * fraction) as i64;
            state.i18n.format_graph_time(timestamp).unwrap_or_default()
        });
        assert_eq!(labels, expected_labels);

        let details = state.public_details_at(observed_at);
        let public_period = details
            .history_periods
            .iter()
            .find(|period| period.current)
            .expect("public details has one current period");
        assert_eq!(public_period.start_at, fixture.expected_period_start);
        assert_eq!(public_period.end_at, fixture.expected_period_end);
        assert_eq!(public_period.reset_at, fixture.expected_reset_at);
        assert_eq!(
            details
                .history_samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_raw_timestamps
        );
        let published_samples = details
            .history_samples
            .iter()
            .map(|sample| UsageHistorySample {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent.unwrap_or(-1.0),
                sol_dollars: sample.sol_dollars,
                terra_dollars: sample.terra_dollars,
                luna_dollars: sample.luna_dollars,
                sol_tokens: sample.sol_tokens,
                terra_tokens: sample.terra_tokens,
                luna_tokens: sample.luna_tokens,
            })
            .collect::<Vec<_>>();
        assert_eq!(published_samples, expected_samples);
    }

    #[test]
    fn weekly_reset_rollover_projects_one_current_cycle_without_mixing() {
        let fixture: GraphFixture = serde_json::from_str(include_str!(
            "../tests/fixtures/graph_weekly_reset_rollover.json"
        ))
        .expect("valid weekly reset rollover fixture");
        let mut state = state_from_graph_fixture(&fixture);
        state.history.samples.extend(
            fixture
                .moving_full_acquisition_samples
                .iter()
                .map(GraphFixtureHistorySample::to_usage_history_sample),
        );
        state.history.normalize();
        let raw_before_projection = state.history.samples.clone();
        assert_eq!(
            raw_before_projection
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_retained_timestamps
        );

        state.select_latest_history();
        let observed_at = fixture.details_response.observed_at;
        let graph = state.graph_paths_for_selection_at(observed_at, true, true, true, false);
        assert_eq!(graph.current_sol_label, "$0.20");

        let details = state.public_details_at(observed_at);
        assert_eq!(details.history_periods.len(), fixture.expected_period_count);
        let current_periods = details
            .history_periods
            .iter()
            .filter(|period| period.current)
            .collect::<Vec<_>>();
        assert_eq!(current_periods.len(), 1);
        assert_eq!(current_periods[0].reset_at, fixture.expected_reset_at);
        assert_eq!(current_periods[0].start_at, fixture.expected_period_start);
        assert_eq!(
            details.quota.as_ref().map(|quota| quota.reset_at),
            Some(fixture.expected_reset_at)
        );
        assert_eq!(
            details
                .history_samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            fixture.expected_raw_timestamps
        );
        let current_samples = details
            .history_samples
            .iter()
            .filter(|sample| sample.reset_at == fixture.expected_reset_at)
            .collect::<Vec<_>>();
        assert_eq!(
            current_samples
                .iter()
                .map(|sample| sample.timestamp)
                .collect::<Vec<_>>(),
            [
                fixture.expected_period_start,
                fixture.expected_period_start + 60
            ]
        );
        assert!(current_samples.iter().all(|sample| {
            sample.timestamp >= fixture.expected_period_start
                && sample.sol_dollars <= fixture.expected_sol_max
        }));
        assert_eq!(state.history.samples, raw_before_projection);

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        assert!(server.publisher().publish_details(details).is_ok());
        assert!(server.publisher().published_pair().is_some());
        server.shutdown();
    }

    #[test]
    fn current_period_bounds_stay_canonical_across_quota_reset_jitter() {
        const CANONICAL_RESET: i64 = 2_000_010_000;
        const WINDOW_SECONDS: i64 = 3_600;
        const OBSERVED_AT: i64 = CANONICAL_RESET - 30;
        const CANONICAL_START: i64 = CANONICAL_RESET - WINDOW_SECONDS;
        const CANONICAL_END: i64 = OBSERVED_AT;
        const OFFSETS: [i64; 6] = [-60, -30, -1, 1, 30, 60];

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        for offset in OFFSETS {
            let quota_reset = CANONICAL_RESET + offset;
            let raw_samples = vec![
                UsageHistorySample {
                    timestamp: CANONICAL_START + 60,
                    reset_at: CANONICAL_RESET,
                    remaining_percent: 92.0,
                    sol_dollars: 1.0,
                    terra_dollars: 0.0,
                    luna_dollars: 0.0,
                    sol_tokens: 1,
                    terra_tokens: 0,
                    luna_tokens: 0,
                },
                UsageHistorySample {
                    timestamp: CANONICAL_RESET - 60,
                    reset_at: CANONICAL_RESET,
                    remaining_percent: 80.0,
                    sol_dollars: 42.0,
                    terra_dollars: 0.0,
                    luna_dollars: 0.0,
                    sol_tokens: 42,
                    terra_tokens: 0,
                    luna_tokens: 0,
                },
            ];
            let mut state = CodexInfoState::preview("normal");
            state.reset_at = Some(quota_reset);
            state.window_seconds = WINDOW_SECONDS;
            state.last_success_at = Some(OBSERVED_AT);
            state.history = UsageHistory {
                samples: raw_samples,
                ..UsageHistory::default()
            };
            state.selected_reset_at = Some(quota_reset);
            state.selected_history_period.clear();

            let periods = state.history_periods_at(OBSERVED_AT);
            assert_eq!(periods.len(), 1, "offset={offset}");
            assert_eq!(
                periods
                    .iter()
                    .filter(|period| {
                        current_history_period_reset(&periods, state.reset_at, OBSERVED_AT)
                            == Some(period.canonical_reset_at)
                    })
                    .count(),
                1,
                "offset={offset}"
            );
            let period = periods.first().expect("one jitter period");
            assert_eq!(
                period.canonical_reset_at, CANONICAL_RESET,
                "offset={offset}"
            );
            assert_eq!(period.start, CANONICAL_START, "offset={offset}");
            assert_eq!(period.end, CANONICAL_END, "offset={offset}");

            let selected = state.history.samples_for_reset(Some(quota_reset));
            assert_eq!(selected.len(), 2, "offset={offset}");
            assert!(selected.iter().all(|sample| {
                sample.reset_at == CANONICAL_RESET
                    && sample.reset_at.abs_diff(quota_reset)
                        <= super::RESET_AT_TOLERANCE_SECONDS as u64
            }));
            let references = selected.iter().collect::<Vec<_>>();
            let graph = state.graph_paths_for_selection_at(OBSERVED_AT, true, true, true, false);
            let expected_graph = graph_paths_for_selection(
                &references,
                CANONICAL_START,
                CANONICAL_END,
                true,
                true,
                true,
                false,
            );
            assert_eq!(graph.remaining, expected_graph.remaining, "offset={offset}");

            let labels = state.graph_time_labels_at(OBSERVED_AT);
            let expected_labels = [0.0, 0.25, 0.5, 0.75, 1.0].map(|fraction| {
                let span = (CANONICAL_END - CANONICAL_START) as f64;
                let timestamp = CANONICAL_START + (span * fraction) as i64;
                state.i18n.format_graph_time(timestamp).unwrap_or_default()
            });
            assert_eq!(labels, expected_labels, "offset={offset}");

            let details = state.public_details_at(OBSERVED_AT);
            let public_periods = &details.history_periods;
            assert_eq!(public_periods.len(), 1, "offset={offset}");
            let public_period = public_periods.first().expect("one public jitter period");
            assert!(public_period.current, "offset={offset}");
            assert_eq!(
                public_period.id,
                CANONICAL_RESET.to_string(),
                "offset={offset}"
            );
            assert_eq!(public_period.reset_at, CANONICAL_RESET, "offset={offset}");
            assert_eq!(public_period.start_at, CANONICAL_START, "offset={offset}");
            assert_eq!(
                public_period.end_at,
                public_period.reset_at.min(OBSERVED_AT),
                "offset={offset}"
            );
            assert_eq!(public_period.end_at, CANONICAL_END, "offset={offset}");
            assert_eq!(
                state
                    .history
                    .samples
                    .iter()
                    .map(|sample| sample.reset_at)
                    .collect::<Vec<_>>(),
                vec![CANONICAL_RESET, CANONICAL_RESET],
                "offset={offset}"
            );
            assert!(
                server.publisher().publish_details(details).is_ok(),
                "offset={offset}"
            );
        }
        server.shutdown();
    }

    #[test]
    fn fresh_startup_selects_current_minute_canonical_period_end_to_end() {
        const WINDOW_SECONDS: i64 = 3_600;
        let observed_at = Utc::now().timestamp();
        let raw_start = (observed_at - 3_000).div_euclid(60) * 60 + 34;
        let canonical_reset = raw_start + WINDOW_SECONDS;
        let current_alias = canonical_reset - 1;
        let old_reset = raw_start - 60;
        let floor_start = raw_start.div_euclid(60) * 60;
        assert_ne!(raw_start, floor_start);

        let current_samples = [
            UsageHistorySample::new_with_usage(
                raw_start,
                current_alias,
                92.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                raw_start + 60,
                canonical_reset,
                80.0,
                ModelDollarTotals {
                    sol: 3.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals::default(),
            ),
        ];
        assert_eq!(current_samples[0].timestamp, floor_start);
        assert!(floor_start <= current_samples[0].timestamp);

        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(current_alias);
        state.window_seconds = WINDOW_SECONDS;
        state.last_success_at = Some(observed_at);
        state.history = UsageHistory {
            samples: vec![
                UsageHistorySample::new_with_usage(
                    raw_start - 120,
                    old_reset,
                    65.0,
                    ModelDollarTotals {
                        sol: 99.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
                current_samples[0].clone(),
                current_samples[1].clone(),
            ],
            ..UsageHistory::default()
        };
        // Fresh startup has no persisted selection; all downstream consumers
        // must resolve the same current canonical period.
        state.selected_reset_at = None;
        state.selected_history_period.clear();

        let periods = state.history_periods_at(observed_at);
        let current = periods
            .iter()
            .find(|period| period.canonical_reset_at == canonical_reset)
            .expect("current minute period survives authoritative bounds");
        assert_eq!(periods.len(), 2);
        assert_eq!(current.start, floor_start);
        assert!(current.start <= current_samples[0].timestamp);
        assert!(!current.label.is_empty());

        state.select_latest_history();
        assert_eq!(state.selected_reset_at, Some(canonical_reset));
        assert_eq!(state.selected_history_reset(), Some(canonical_reset));
        assert_eq!(
            state.selected_history_period,
            state
                .history_periods_at(observed_at)
                .into_iter()
                .find(|period| period.canonical_reset_at == canonical_reset)
                .expect("current period label")
                .label
        );

        let graph_samples: Vec<UsageHistorySample> =
            serde_json::from_str(&state.graph_data()).expect("selected graph JSON");
        assert_eq!(graph_samples.len(), current_samples.len());
        assert!(graph_samples
            .iter()
            .all(|sample| sample.reset_at == canonical_reset));
        assert!(graph_samples.iter().any(|sample| {
            (sample.sol_dollars - current_samples[1].sol_dollars).abs() < f64::EPSILON
        }));

        let paths = state.graph_paths_for_selection_at(observed_at, true, true, true, false);
        assert!(!paths.remaining.is_empty());
        assert!(!paths.sol.is_empty());
        assert_eq!(paths.current_sol_label, "$3.00");
    }

    #[test]
    fn fresh_startup_rejects_current_period_with_one_minute_early_owned_row() {
        const WINDOW_SECONDS: i64 = 3_600;
        let observed_at = Utc::now().timestamp();
        let raw_start = (observed_at - 3_000).div_euclid(60) * 60 + 34;
        let canonical_reset = raw_start + WINDOW_SECONDS;
        let current_alias = canonical_reset - 1;
        let old_reset = raw_start - 60;
        let floor_start = raw_start.div_euclid(60) * 60;

        let mut state = CodexInfoState::preview("normal");
        state.reset_at = Some(current_alias);
        state.window_seconds = WINDOW_SECONDS;
        state.last_success_at = Some(observed_at);
        state.history = UsageHistory {
            samples: vec![
                UsageHistorySample::new_with_usage(
                    raw_start - 120,
                    old_reset,
                    65.0,
                    ModelDollarTotals {
                        sol: 99.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
                // The constructor keeps the same minute-start representation
                // as the collector, but this row is one full bucket early.
                UsageHistorySample::new_with_usage(
                    raw_start - 60,
                    current_alias,
                    92.0,
                    ModelDollarTotals {
                        sol: 1.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
                UsageHistorySample::new_with_usage(
                    raw_start + 60,
                    canonical_reset,
                    80.0,
                    ModelDollarTotals {
                        sol: 3.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
            ],
            ..UsageHistory::default()
        };
        state.selected_reset_at = None;
        state.selected_history_period.clear();

        let periods = state.history_periods_at(observed_at);
        assert!(periods
            .iter()
            .all(|period| period.canonical_reset_at != canonical_reset));
        assert_eq!(
            periods
                .iter()
                .map(|period| period.canonical_reset_at)
                .collect::<Vec<_>>(),
            vec![old_reset]
        );
        assert!(periods.iter().all(|period| period.start < floor_start));

        state.select_latest_history();
        assert_eq!(state.selected_reset_at, Some(old_reset));
        let graph_samples: Vec<UsageHistorySample> =
            serde_json::from_str(&state.graph_data()).expect("selected old graph JSON");
        assert_eq!(graph_samples.len(), 1);
        assert_eq!(graph_samples[0].reset_at, old_reset);
        assert_eq!(
            state
                .graph_paths_for_selection_at(observed_at, true, true, true, false)
                .current_sol_label,
            "$99.00"
        );
    }

    #[test]
    fn shared_graph_current_period_rejects_an_early_owned_row() {
        let fixture: GraphFixture =
            serde_json::from_str(include_str!("../tests/fixtures/graph_delayed_quota.json"))
                .expect("valid shared graph fixture");
        let mut state = state_from_graph_fixture(&fixture);
        let quota = &fixture.details_response.quota;
        let computed_start = quota
            .reset_at
            .checked_sub(quota.window_seconds)
            .expect("fixture reset/window fit in i64");
        state.history.samples.push(UsageHistorySample {
            timestamp: computed_start - 60,
            reset_at: quota.reset_at,
            remaining_percent: 92.0,
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        });

        let raw = state.history.samples_for_reset(Some(quota.reset_at));
        assert_eq!(
            raw.len(),
            fixture.details_response.history_samples.len() + 1
        );
        assert_eq!(
            raw.first().map(|sample| sample.timestamp),
            Some(computed_start - 60)
        );
        let periods = state.history_periods_at(fixture.details_response.observed_at);
        assert!(periods.is_empty(), "invalid current period must be omitted");

        let graph = state.graph_paths_for_selection_at(
            fixture.details_response.observed_at,
            true,
            true,
            true,
            false,
        );
        assert!(graph.remaining.is_empty());
        assert!(graph.unused_intervals.is_empty());
        assert_eq!(
            state.graph_time_labels_at(fixture.details_response.observed_at),
            <[String; 5]>::default()
        );
        let details = state.public_details_at(fixture.details_response.observed_at);
        assert!(details.history_periods.is_empty());
        assert_eq!(details.history_samples.len(), raw.len());
        let raw_payload: Vec<UsageHistorySample> =
            serde_json::from_str(&state.history.graph_data_for_reset(quota.reset_at))
                .expect("raw graph payload remains serializable");
        assert_eq!(raw_payload.len(), raw.len());
        assert_eq!(
            raw_payload.first().map(|sample| sample.timestamp),
            Some(computed_start - 60)
        );
    }

    #[test]
    fn long_rolling_reset_sequence_stays_in_one_period_after_a_real_boundary() {
        // The affected local history contains a genuine boundary followed by
        // hundreds of quota observations whose reset_at advances every
        // minute. The cumulative drift is hours, so a group-wide five-minute
        // cap incorrectly turned the current graph into many tiny periods.
        let base = 1_800_000_000;
        let stable_reset = base + WEEK_SECONDS;
        let rolling_start = base + 120;
        let rolling_reset = stable_reset + 10_000;
        let mut samples = vec![
            UsageHistorySample::new_with_usage(
                base,
                stable_reset,
                72.0,
                ModelDollarTotals {
                    luna: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 10,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                base + 60,
                stable_reset,
                71.0,
                ModelDollarTotals {
                    luna: 2.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 20,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        for index in 0..12 {
            let timestamp = rolling_start + index * 60;
            samples.push(UsageHistorySample::new_with_usage(
                timestamp,
                rolling_reset + index * 60,
                100.0,
                ModelDollarTotals {
                    luna: if index == 11 { 4.0 } else { 0.0 },
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: if index == 11 { 40 } else { 0 },
                    ..ModelTokenTotals::default()
                },
            ));
        }
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        let periods = history.periods(rolling_start + 12 * 60 + 1, None);
        assert_eq!(periods.len(), 2);
        let rolling_period = periods
            .iter()
            .find(|period| period.start == rolling_start)
            .expect("rolling period");
        assert_eq!(rolling_period.canonical_reset_at, rolling_reset + 11 * 60);
        let selected = history.samples_for_reset(Some(rolling_period.canonical_reset_at));
        assert_eq!(selected.len(), 12);
        assert_eq!(selected.last().unwrap().luna_dollars, 4.0);
        assert!(selected
            .iter()
            .all(|sample| sample.reset_at == rolling_period.canonical_reset_at));
    }

    #[test]
    fn quota_only_reset_fragments_stay_with_the_adjacent_spend_period() {
        // The production failure was not a single moving sequence: the
        // service emitted several independent full-quota/zero-usage reset
        // rows between a spend row and the next spend row. Selecting one of
        // those fragments produced a flat 100% graph with no model data.
        let base = 1_810_000_000;
        let stable_reset = base + WEEK_SECONDS;
        let samples = vec![
            UsageHistorySample::new(
                base - 600,
                stable_reset - 50_000,
                48.0,
                ModelDollarTotals {
                    luna: 0.5,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                base,
                stable_reset,
                98.0,
                ModelDollarTotals {
                    luna: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 10,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                base + 60,
                stable_reset,
                97.0,
                ModelDollarTotals {
                    luna: 2.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 20,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new(
                base + 120,
                stable_reset + 10_000,
                100.0,
                ModelDollarTotals::default(),
            ),
            UsageHistorySample::new(
                base + 180,
                stable_reset + 11_000,
                100.0,
                ModelDollarTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                base + 240,
                stable_reset + 12_000,
                100.0,
                ModelDollarTotals {
                    luna: 4.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    luna: 40,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        let periods = history.periods(base + 300, None);
        assert_eq!(periods.len(), 2);
        let rolling_start = base.div_euclid(60) * 60;
        let rolling_period = periods
            .iter()
            .find(|period| period.start == rolling_start)
            .expect("rolling spend period");
        let selected = history.samples_for_reset(Some(rolling_period.canonical_reset_at));
        assert_eq!(selected.len(), 5);
        assert_eq!(selected.last().unwrap().luna_dollars, 4.0);
        assert_eq!(selected.last().unwrap().luna_tokens, 40);
        assert!(selected
            .iter()
            .any(|sample| sample.remaining_percent == 100.0));
    }

    #[test]
    fn live_rolling_quota_chain_does_not_expose_an_empty_past_period() {
        // Production SQLite rows can contain two quota snapshots in the first
        // minute and then one moving-reset row per minute until the next model
        // snapshot.  Filtering those singleton links before grouping used to
        // expose a false period whose graph was flat at 100% with no usage.
        let base = 1_787_683_320; // 2026-08-26 03:42 JST
        let rolling_reset = base + WEEK_SECONDS + 54;
        let mut samples = vec![
            UsageHistorySample::new(base, rolling_reset, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                base + 60,
                rolling_reset,
                100.0,
                ModelDollarTotals::default(),
            ),
        ];
        for index in 1..47 {
            let timestamp = base + index * 60;
            samples.push(UsageHistorySample::new(
                timestamp,
                rolling_reset + index * 60,
                100.0,
                ModelDollarTotals::default(),
            ));
        }
        let spend_timestamp = base + 47 * 60;
        samples.push(UsageHistorySample::from_model_history_with_usage(
            spend_timestamp,
            rolling_reset + 47 * 60,
            ModelDollarTotals {
                luna: 0.03,
                ..ModelDollarTotals::default()
            },
            ModelTokenTotals {
                luna: 269_934,
                ..ModelTokenTotals::default()
            },
        ));
        let history = UsageHistory {
            samples,
            ..UsageHistory::default()
        };

        let periods = history.periods(spend_timestamp + 60, None);
        assert_eq!(periods.len(), 1, "rolling chain must remain one period");
        let period = periods.first().expect("rolling spend period");
        let selected = history.samples_for_reset(Some(period.canonical_reset_at));
        assert!(selected.iter().any(|sample| sample.luna_dollars > 0.0));
        assert!(selected
            .iter()
            .any(|sample| sample.remaining_percent == 100.0));
    }

    #[test]
    fn singleton_reset_snapshot_overlapping_a_spend_period_stays_separate() {
        let history = UsageHistory {
            samples: vec![
                UsageHistorySample::from_model_history_with_usage(
                    1_800_000_000,
                    1_800_604_800,
                    ModelDollarTotals {
                        sol: 100.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
                UsageHistorySample::from_model_history_with_usage(
                    1_800_000_120,
                    1_800_604_800,
                    ModelDollarTotals {
                        sol: 420.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
                UsageHistorySample::new(
                    1_800_000_120,
                    1_800_650_000,
                    14.0,
                    ModelDollarTotals::default(),
                ),
                UsageHistorySample::from_model_history_with_usage(
                    1_800_000_240,
                    1_800_604_800,
                    ModelDollarTotals {
                        sol: 420.0,
                        ..ModelDollarTotals::default()
                    },
                    ModelTokenTotals::default(),
                ),
            ],
            ..UsageHistory::default()
        };

        let periods = history.periods(1_800_700_000, None);
        assert_eq!(periods.len(), 2);
        let spend_period = periods
            .iter()
            .find(|period| period.canonical_reset_at == 1_800_604_800)
            .expect("spend period");
        let selected = history.samples_for_reset(Some(spend_period.canonical_reset_at));
        assert_eq!(selected.len(), 3);
        assert!(selected
            .iter()
            .all(|sample| (sample.remaining_percent - 14.0).abs() > f64::EPSILON));
        assert_eq!(
            history
                .samples_for_reset(Some(1_800_650_000))
                .first()
                .map(|sample| sample.remaining_percent),
            Some(14.0)
        );
        let references = selected.iter().collect::<Vec<_>>();
        let graph = graph_paths_for_selection(
            &references,
            spend_period.start,
            spend_period.end,
            false,
            false,
            true,
            false,
        );
        assert_eq!(graph.current_sol_label, "$420.00");
    }

    #[test]
    fn period_list_hides_legacy_moving_resets_but_keeps_real_singletons_and_db_rows() {
        let db_path = test_history_path("rolling-reset-artifacts");
        let base = 1_699_999_980;
        let stable_reset = base + 400_000;
        let ghost_one = base + 120 + WEEK_SECONDS + 23;
        let ghost_two = base + 180 + WEEK_SECONDS + 48;
        let real_singleton_timestamp = base + 7_200;
        let real_singleton_reset = real_singleton_timestamp + WEEK_SECONDS + 30;
        let samples = [
            UsageHistorySample::new(base, stable_reset, 72.0, ModelDollarTotals::default()),
            UsageHistorySample::new(base + 60, stable_reset, 71.0, ModelDollarTotals::default()),
            UsageHistorySample::new(base + 120, ghost_one, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(base + 180, ghost_two, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                real_singleton_timestamp,
                real_singleton_reset,
                100.0,
                ModelDollarTotals::default(),
            ),
        ];
        let mut store = UsageStore::open(&db_path).unwrap();
        store
            .upsert_samples(
                &samples
                    .iter()
                    .map(UsageHistorySample::to_store)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        drop(store);

        let load_at = chrono::DateTime::<Utc>::from_timestamp(real_singleton_timestamp, 0).unwrap();
        let history = UsageHistory::load_from_db_path_at(Some(db_path.clone()), load_at);
        let period_ids = history
            .periods(base + 300, None)
            .into_iter()
            .map(|period| period.canonical_reset_at)
            .collect::<Vec<_>>();
        assert_eq!(period_ids, vec![real_singleton_reset, stable_reset]);
        assert!(history.samples_for_reset(Some(ghost_one)).is_empty());
        assert_eq!(
            history.samples_for_reset(Some(real_singleton_reset)).len(),
            1
        );
        assert_eq!(
            UsageStore::open(&db_path)
                .unwrap()
                .load_all()
                .unwrap()
                .len(),
            5
        );
        let _ = fs::remove_dir_all(db_path.parent().unwrap());
    }

    #[test]
    fn graph_json_only_contains_the_selected_reset_period() {
        let history = UsageHistory {
            samples: vec![
                UsageHistorySample::new(
                    1_700_000_000,
                    1_700_100_000,
                    50.0,
                    ModelDollarTotals::default(),
                ),
                UsageHistorySample::new(
                    1_700_000_060,
                    1_700_200_000,
                    40.0,
                    ModelDollarTotals::default(),
                ),
            ],
            ..UsageHistory::default()
        };
        let data = history.graph_data_for_reset(1_700_200_000);
        assert!(data.contains("1700200000"));
        assert!(!data.contains("1700100000"));
        assert!(data.contains("remaining_percent"));
    }

    #[test]
    fn historical_graph_keeps_terminal_reset_sample_for_model_lines() {
        let reset_at = 1_700_100_000;
        let samples = [
            UsageHistorySample::new_with_usage(
                1_700_000_000,
                reset_at,
                80.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 0.0,
                    luna: 0.0,
                },
                ModelTokenTotals {
                    sol: 20,
                    terra: 0,
                    luna: 0,
                },
            ),
            // A quota/usage observation exactly at the historical reset
            // boundary is owned by that period's canonical reset_at.  It is
            // the last visible SOL point after the next period begins.
            UsageHistorySample::new_with_usage(
                reset_at,
                reset_at,
                70.0,
                ModelDollarTotals {
                    sol: 9.0,
                    terra: 0.0,
                    luna: 0.0,
                },
                ModelTokenTotals {
                    sol: 90,
                    terra: 0,
                    luna: 0,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = graph_time_endpoints(
            minute_model_spend_for_metric(&references, false),
            samples[0].timestamp,
            reset_at,
        );
        assert_eq!(points.last().map(|point| point.timestamp), Some(reset_at));
        assert_eq!(points.last().map(|point| point.sol), Some(9.0));
    }

    #[test]
    fn graph_endpoint_does_not_import_a_future_reset_sample() {
        let start = 1_700_000_000;
        let end = 1_700_100_000;
        let samples = [
            UsageHistorySample::new_with_usage(
                end - 60,
                end,
                70.0,
                ModelDollarTotals {
                    sol: 3.0,
                    terra: 0.0,
                    luna: 0.0,
                },
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                end + 60,
                end + WEEK_SECONDS,
                100.0,
                ModelDollarTotals {
                    sol: 99.0,
                    terra: 0.0,
                    luna: 0.0,
                },
                ModelTokenTotals::default(),
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = graph_time_endpoints(
            minute_model_spend_for_metric(&references, false),
            start,
            end,
        );
        assert_eq!(points.last().map(|point| point.timestamp), Some(end));
        assert_eq!(points.last().map(|point| point.sol), Some(3.0));
    }

    #[test]
    fn preview_history_points_are_spread_before_now() {
        let now = 1_700_000_000;
        let history = UsageHistory::preview(now, now + 6 * 86_400, ModelDollarTotals::default());
        assert_eq!(history.samples.len(), 12);
        assert_eq!(history.reset_periods_desc().len(), 2);
        assert_eq!(
            history
                .samples
                .iter()
                .filter(|sample| sample.reset_at == now + 6 * 86_400)
                .count(),
            6
        );
        assert_eq!(
            history
                .samples
                .iter()
                .filter(|sample| sample.reset_at == now - 86_400)
                .count(),
            6
        );
        assert!(history
            .samples
            .windows(2)
            .all(|pair| pair[0].timestamp < pair[1].timestamp));
        assert!(history.samples.iter().all(|sample| sample.timestamp <= now));
    }

    #[test]
    fn graph_preview_contains_a_repeated_quota_sample_with_advancing_models() {
        let now = 1_700_000_000;
        let reset_at = now + 6 * 86_400;
        let history = UsageHistory::preview(
            now,
            reset_at,
            ModelDollarTotals {
                sol: 100.0,
                terra: 50.0,
                luna: 25.0,
            },
        );
        let current = history
            .samples
            .iter()
            .filter(|sample| sample.reset_at == reset_at)
            .collect::<Vec<_>>();
        assert_eq!(
            current
                .iter()
                .map(|sample| sample.remaining_percent)
                .collect::<Vec<_>>(),
            [84.0, 69.0, 69.0, 49.0, 49.0, 14.0]
        );
        assert!(current
            .windows(2)
            .all(|pair| pair[0].sol_dollars <= pair[1].sol_dollars));
        assert!(current
            .windows(2)
            .any(|pair| pair[0].sol_dollars == pair[1].sol_dollars));
        assert!(current
            .windows(2)
            .any(|pair| pair[0].sol_dollars < pair[1].sol_dollars));
    }

    #[test]
    fn model_cost_history_keeps_cumulative_totals() {
        let reset_at = 1_700_100_000;
        let first = UsageHistorySample::new(
            1_700_000_000,
            reset_at,
            50.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 2.0,
                luna: 0.0,
            },
        );
        let second = UsageHistorySample::new(
            1_700_001_800,
            reset_at,
            45.0,
            ModelDollarTotals {
                sol: 3.0,
                terra: 4.0,
                luna: 1.0,
            },
        );
        let third = UsageHistorySample::new(
            1_700_004_000,
            reset_at,
            40.0,
            ModelDollarTotals {
                sol: 5.0,
                terra: 5.0,
                luna: 2.0,
            },
        );
        let samples = [&first, &second, &third];
        let minute = minute_model_spend(&samples);
        assert_eq!(minute.len(), 3);
        assert_eq!(
            (minute[0].sol, minute[0].terra, minute[0].luna),
            (1.0, 2.0, 0.0)
        );
        assert_eq!(
            (minute[1].sol, minute[1].terra, minute[1].luna),
            (3.0, 4.0, 1.0)
        );
        assert_eq!(
            (minute[2].sol, minute[2].terra, minute[2].luna),
            (5.0, 5.0, 2.0)
        );
    }

    #[test]
    fn dollar_graph_does_not_recount_a_regressed_snapshot() {
        let reset_at = 1_700_100_000;
        let samples = [
            UsageHistorySample::new(
                100,
                reset_at,
                90.0,
                ModelDollarTotals {
                    sol: 50.0,
                    terra: 2.0,
                    luna: 1.0,
                },
            ),
            // A transiently incomplete scan must not reset the cumulative
            // total and make the next 51-dollar observation count as +51.
            UsageHistorySample::new(
                160,
                reset_at,
                89.0,
                ModelDollarTotals {
                    sol: 49.0,
                    terra: 2.0,
                    luna: 1.0,
                },
            ),
            UsageHistorySample::new(
                220,
                reset_at,
                88.0,
                ModelDollarTotals {
                    sol: 51.0,
                    terra: 2.0,
                    luna: 1.0,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = minute_model_spend(&references);
        assert_eq!(points[0].sol, 50.0);
        assert_eq!(points[1].sol, 50.0);
        assert_eq!(points[2].sol, 51.0);

        let graph = graph_paths_for_selection(&references, 0, 240, false, false, true, false);
        assert_eq!(graph.current_sol_label, "$51.00");
    }

    #[test]
    fn graph_model_points_are_anchored_to_start_and_now() {
        let points = graph_time_endpoints(
            vec![HourlyModelSpend {
                timestamp: 120,
                sol: 3.0,
                terra: 2.0,
                luna: 1.0,
            }],
            100,
            300,
        );
        assert_eq!(points.first().map(|point| point.timestamp), Some(100));
        assert_eq!(
            (points[0].sol, points[0].terra, points[0].luna),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(points.last().map(|point| point.timestamp), Some(300));
        assert_eq!(
            (points[2].sol, points[2].terra, points[2].luna),
            (3.0, 2.0, 1.0)
        );
    }

    #[test]
    fn graph_model_endpoints_do_not_duplicate_measurement_x_coordinates() {
        let points = graph_time_endpoints(
            vec![HourlyModelSpend {
                timestamp: 100,
                sol: 3.0,
                terra: 2.0,
                luna: 1.0,
            }],
            100,
            100,
        );
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].timestamp, 100);
        assert_eq!(
            (points[0].sol, points[0].terra, points[0].luna),
            (3.0, 2.0, 1.0)
        );
    }

    #[test]
    fn graph_paths_span_from_reset_start_to_current_time() {
        let reset_at = 7_200;
        let first = UsageHistorySample::new(
            600,
            reset_at,
            80.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 0.0,
                luna: 0.0,
            },
        );
        let latest = UsageHistorySample::new(
            3_600,
            reset_at,
            70.0,
            ModelDollarTotals {
                sol: 4.0,
                terra: 2.0,
                luna: 1.0,
            },
        );
        let paths = graph_paths(&[&first, &latest], 0, 3_900);
        assert!(paths.remaining.starts_with("M0.00 1.00"));
        assert!(paths.remaining.contains("L15.38"));
        assert!(paths.remaining.contains("L15.38 1.00 L15.38"));
        assert!(paths.remaining.contains("L100.00"));
        assert!(paths.sol.starts_with("M0.00 99.00"));
        assert!(paths.sol.contains("L100.00"));
        assert!(paths.terra.contains("L100.00"));
        assert!(paths.luna.contains("L100.00"));
        assert_eq!(paths.current_remaining_label, "70%");
        assert_eq!(paths.current_sol_label, "$4.00");
        assert_eq!(paths.current_terra_label, "$2.00");
        assert_eq!(paths.current_luna_label, "$1.00");
        assert!((paths.current_sol_y - 0.01).abs() < 0.0001);
        assert!((paths.current_terra_y - 0.50).abs() < 0.0001);
        assert!((paths.current_luna_y - 0.745).abs() < 0.0001);
    }

    #[test]
    fn remaining_graph_stays_flat_when_models_never_change() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(60, 1_000, 90.0, ModelDollarTotals::default()),
            UsageHistorySample::new(120, 1_000, 90.0, ModelDollarTotals::default()),
            UsageHistorySample::new(180, 1_000, 70.0, ModelDollarTotals::default()),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        // Quota rereads during an entirely idle model history cannot create a
        // slope. The production path must collapse the whole period to one
        // horizontal segment rather than relying on the legacy helper.
        assert_eq!(
            remaining_graph_points(&references, 0, 240),
            vec![(0, 100.0), (240, 100.0)]
        );
        let path = graph_paths(&references, 0, 240).remaining;
        assert_eq!(path, "M0.00 1.00 L100.00 1.00");
    }

    #[test]
    fn remaining_graph_does_not_infer_quota_loss_from_model_spend() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::from_model_history(
                60,
                1_000,
                ModelDollarTotals {
                    sol: 10.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::from_model_history(
                120,
                1_000,
                ModelDollarTotals {
                    sol: 20.0,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();

        // Session-derived model costs have no quota observation. A flat
        // remaining line is intentionally retained; deriving a percentage
        // from pricing would fabricate a credit balance and make a valid
        // historical graph look like an account deduction oracle.
        assert_eq!(
            remaining_graph_points(&references, 0, 180),
            vec![(0, 100.0), (60, 100.0), (120, 100.0), (180, 100.0)]
        );
    }

    #[test]
    fn remaining_graph_rejects_conflicting_reset_rows_at_one_timestamp() {
        let samples = [
            UsageHistorySample::new(
                0,
                1_000,
                88.0,
                ModelDollarTotals {
                    terra: 30.5,
                    luna: 6.3,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                60,
                1_000,
                88.0,
                ModelDollarTotals {
                    terra: 30.5,
                    luna: 6.4,
                    ..ModelDollarTotals::default()
                },
            ),
            // This mirrors the observed 2026-08-22 07:17 collision: a
            // different reset period reports 14% at the same timestamp but
            // carries no model usage. It must never overwrite the 88% row.
            UsageHistorySample::new(60, 2_000, 14.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                120,
                1_000,
                87.0,
                ModelDollarTotals {
                    terra: 30.5,
                    luna: 6.5,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = remaining_graph_points(&references, 0, 180);

        assert!(points.iter().all(|(_, value)| *value >= 87.0));
        assert!(points
            .iter()
            .any(|(timestamp, value)| *timestamp == 120 && (*value - 87.0).abs() < f64::EPSILON));
        assert!(!points
            .iter()
            .any(|(_, value)| (*value - 14.0).abs() < f64::EPSILON));
    }

    #[test]
    fn moving_reset_collision_at_30_and_60_seconds_fails_closed() {
        for drift in [30_i64, 60_i64] {
            let base_reset = 2_000_000_i64;
            let history = UsageHistory {
                samples: vec![
                    UsageHistorySample::new(
                        960_000,
                        base_reset,
                        88.0,
                        ModelDollarTotals {
                            sol: 1.0,
                            ..ModelDollarTotals::default()
                        },
                    ),
                    // Same timestamp, moving reset alias, and no model
                    // usage: this row must never overwrite the spend row.
                    UsageHistorySample::new(
                        960_000,
                        base_reset + drift,
                        14.0,
                        ModelDollarTotals::default(),
                    ),
                    // A later moving-reset observation keeps the aliases in
                    // one rolling group, covering the previously untested
                    // "subsequent moving-reset row" path.
                    UsageHistorySample::new(
                        960_060,
                        base_reset + drift + 30,
                        87.0,
                        ModelDollarTotals {
                            sol: 2.0,
                            ..ModelDollarTotals::default()
                        },
                    ),
                ],
                ..UsageHistory::default()
            };
            let selected = history.samples_for_reset(Some(base_reset + drift + 30));
            assert_eq!(selected.len(), 2, "drift={drift}");
            assert_eq!(selected[0].remaining_percent, -1.0, "drift={drift}");
            assert_eq!(selected[1].remaining_percent, 87.0, "drift={drift}");
            let references = selected.iter().collect::<Vec<_>>();
            let points = remaining_graph_points(&references, 960_000, 960_120);
            assert!(
                points.iter().all(|(_, value)| *value >= 87.0),
                "drift={drift}"
            );
            assert!(!points
                .iter()
                .any(|(_, value)| (*value - 14.0).abs() < f64::EPSILON));
        }
    }

    #[test]
    fn remaining_graph_holds_idle_flat_and_connects_active_change_points() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(60, 1_000, 95.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                120,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 0.0,
                    luna: 0.0,
                },
            ),
            UsageHistorySample::new(
                180,
                1_000,
                85.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 0.0,
                    luna: 0.0,
                },
            ),
            UsageHistorySample::new(
                240,
                1_000,
                80.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 0.0,
                    luna: 0.0,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = remaining_graph_points(&references, 0, 240);

        assert_eq!(
            points,
            vec![
                (0, 100.0),
                (60, 100.0),
                (120, 90.0),
                (180, 90.0),
                (240, 80.0)
            ]
        );
        let path = graph_paths(&references, 0, 240).remaining;
        assert_eq!(
            path,
            "M0.00 1.00 L25.00 1.00 L50.00 10.80 L75.00 10.80 L100.00 20.60"
        );
    }

    #[test]
    fn remaining_graph_aligns_quota_with_minute_bucket_model_changes() {
        let mut initial = UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default());
        let mut idle = UsageHistorySample::new(0, 1_000, 95.0, ModelDollarTotals::default());
        let mut first_active = UsageHistorySample::new(
            0,
            1_000,
            90.0,
            ModelDollarTotals {
                sol: 1.0,
                ..ModelDollarTotals::default()
            },
        );
        let mut second_active = UsageHistorySample::new(
            0,
            1_000,
            80.0,
            ModelDollarTotals {
                sol: 2.0,
                ..ModelDollarTotals::default()
            },
        );
        // Keep quota observations at their actual timestamps while model
        // snapshots are drawn at minute-bucket starts (60 and 120).
        initial.timestamp = 0;
        idle.timestamp = 40;
        first_active.timestamp = 100;
        second_active.timestamp = 160;
        let samples = [initial, idle, first_active, second_active];
        let references = samples.iter().collect::<Vec<_>>();

        assert_eq!(
            remaining_graph_points(&references, 0, 240),
            vec![(0, 100.0), (60, 90.0), (120, 80.0), (240, 80.0)]
        );
        assert_eq!(
            graph_paths(&references, 0, 240).remaining,
            "M0.00 1.00 L25.00 10.80 L50.00 20.60 L100.00 20.60"
        );
    }

    #[test]
    fn remaining_graph_interpolates_repeated_active_quota_samples() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                60,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
            ),
            // Model usage continues, but the quota reread is unchanged. Treat
            // the repeated value as a missed sample and interpolate it from
            // the surrounding endpoints instead of drawing a false fold.
            UsageHistorySample::new(
                120,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 2.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                180,
                1_000,
                80.0,
                ModelDollarTotals {
                    sol: 3.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                240,
                1_000,
                80.0,
                ModelDollarTotals {
                    sol: 4.0,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();

        assert_eq!(
            remaining_graph_points(&references, 0, 240),
            vec![
                (0, 100.0),
                (60, 90.0),
                (120, 85.0),
                (180, 80.0),
                (240, 80.0)
            ]
        );
        assert_eq!(
            graph_paths(&references, 0, 240).remaining,
            "M0.00 1.00 L25.00 10.80 L50.00 15.70 L75.00 20.60 L100.00 20.60"
        );
    }

    #[test]
    fn remaining_graph_token_mode_keeps_the_latest_terminal_observation() {
        let samples = [
            UsageHistorySample::new_with_usage(
                0,
                1_000,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                60,
                1_000,
                99.0,
                ModelDollarTotals::default(),
                ModelTokenTotals {
                    luna: 1,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                120,
                1_000,
                99.0,
                ModelDollarTotals::default(),
                ModelTokenTotals {
                    luna: 2,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                180,
                1_000,
                99.0,
                ModelDollarTotals::default(),
                ModelTokenTotals {
                    luna: 3,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();

        let points = remaining_graph_points_for_metric(&references, 0, 180, true);
        assert_eq!(points.last(), Some(&(180, 99.0)));
        assert!(points.iter().all(|(_, remaining)| *remaining >= 99.0));
        let graph = graph_paths_for_selection(&references, 0, 180, true, true, true, true);
        assert_eq!(graph.current_remaining_label, "99%");
        assert!(graph.remaining.ends_with("L100.00 1.98"));
    }

    #[test]
    fn remaining_graph_interpolates_across_an_idle_sampling_gap() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                60,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
            ),
            // The model advances again, but the quota reread repeats. This is
            // a missing sample, not a reason to draw a horizontal fold.
            UsageHistorySample::new(
                120,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 2.0,
                    ..ModelDollarTotals::default()
                },
            ),
            // No model advances in this interval. The completed line must
            // hold its value here rather than inventing a quota slope.
            UsageHistorySample::new(
                180,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 2.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                240,
                1_000,
                80.0,
                ModelDollarTotals {
                    sol: 3.0,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();

        assert_eq!(
            remaining_graph_points(&references, 0, 240),
            vec![
                (0, 100.0),
                (60, 90.0),
                (120, 85.0),
                (180, 85.0),
                (240, 80.0)
            ]
        );
    }

    #[test]
    fn remaining_graph_smooths_the_line_at_internal_model_change_points() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                60,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                120,
                1_000,
                80.0,
                ModelDollarTotals {
                    sol: 2.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                180,
                1_000,
                60.0,
                ModelDollarTotals {
                    sol: 3.0,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = remaining_graph_points(&references, 0, 180);

        // The internal 120-second change point is smoothed from its two
        // neighboring active endpoints: (90 + 2*80 + 60) / 4 = 77.5.
        assert_eq!(
            points,
            vec![(0, 100.0), (60, 90.0), (120, 77.5), (180, 60.0)]
        );
        assert!(points.windows(2).all(|pair| pair[0].1 >= pair[1].1));
    }

    #[test]
    fn remaining_change_point_collapse_clamps_upward_rereads() {
        let points = collapse_remaining_change_points(&[
            (0, 100.0),
            (60, 80.0),
            (120, 85.0),
            (180, 70.0),
            (240, 70.0),
        ]);
        assert_eq!(
            points,
            vec![(0, 100.0), (60, 80.0), (180, 70.0), (240, 70.0)]
        );
    }

    #[test]
    fn remaining_label_matches_smoothed_path_endpoint() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(60, 1_000, 0.0, ModelDollarTotals::default()),
            // A transient upward reread must not move the line endpoint back
            // above the last monotonic value.
            UsageHistorySample::new(120, 1_000, 10.0, ModelDollarTotals::default()),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let paths = graph_paths(&references, 0, 120);
        assert_eq!(paths.current_remaining_label, "100%");
        assert!(paths.remaining.ends_with("L100.00 1.00"));
    }

    #[test]
    fn remaining_markers_interpolate_each_integer_boundary() {
        let first = UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default());
        let second = UsageHistorySample::new(60, 1_000, 99.0, ModelDollarTotals::default());
        let third = UsageHistorySample::new(120, 1_000, 97.0, ModelDollarTotals::default());

        let markers = remaining_marker_positions(&[&first, &second, &third], 0, 120);

        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.boundary)
                .collect::<Vec<_>>(),
            [99, 98, 97]
        );
        assert!((markers[0].x - 40.0).abs() < f64::EPSILON);
        assert!((markers[1].x - (500.0 / 7.0)).abs() < 0.000_000_1);
        assert!((markers[2].x - 100.0).abs() < f64::EPSILON);
        for (marker, boundary) in markers.iter().zip([99.0, 98.0, 97.0]) {
            let expected_y = 99.0 - boundary * 0.98;
            assert!((marker.y - expected_y).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn remaining_markers_are_on_the_same_smoothed_line_segments() {
        let first = UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default());
        let second = UsageHistorySample::new(60, 1_000, 99.0, ModelDollarTotals::default());
        let third = UsageHistorySample::new(120, 1_000, 97.0, ModelDollarTotals::default());
        let samples = [&first, &second, &third];
        let points = graph_points(&samples, 0, 120, 100.0, |sample| sample.remaining_percent);
        let markers = remaining_marker_positions_on_points(&points, 0, 120);

        assert_eq!(points.first(), Some(&(0, 100.0)));
        assert!(points.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for marker in markers {
            let marker_timestamp = marker.x / 100.0 * 120.0;
            let Some([before, after]) = points.windows(2).find_map(|window| {
                let [before, after] = window else { return None };
                (marker_timestamp >= before.0 as f64
                    && marker_timestamp <= after.0 as f64
                    && after.0 > before.0)
                    .then_some([before, after])
            }) else {
                panic!("marker must lie on a remaining path segment: {marker:?}");
            };
            let fraction = (marker_timestamp - before.0 as f64) / (after.0 - before.0) as f64;
            let line_value = before.1 + (after.1 - before.1) * fraction;
            assert!((marker.y - remaining_graph_y(line_value)).abs() < 0.000_000_1);
        }
    }

    #[test]
    fn remaining_markers_use_the_reset_anchor_for_the_first_observation() {
        let sample = UsageHistorySample::new(60, 1_000, 97.0, ModelDollarTotals::default());

        let markers = remaining_marker_positions(&[&sample], 0, 120);

        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.boundary)
                .collect::<Vec<_>>(),
            [99, 98, 97]
        );
    }

    #[test]
    fn remaining_markers_do_not_duplicate_a_boundary() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(60, 1_000, 99.0, ModelDollarTotals::default()),
            UsageHistorySample::new(120, 1_000, 97.0, ModelDollarTotals::default()),
            UsageHistorySample::new(180, 1_000, 98.0, ModelDollarTotals::default()),
            UsageHistorySample::new(240, 1_000, 96.0, ModelDollarTotals::default()),
        ];

        let references = samples.iter().collect::<Vec<_>>();
        let markers = remaining_marker_positions(&references, 0, 240);

        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.boundary)
                .collect::<Vec<_>>(),
            [99, 98, 97, 96]
        );
    }

    #[test]
    fn remaining_markers_filter_out_missing_values() {
        let first = UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default());
        let second = UsageHistorySample::new(60, 1_000, 99.0, ModelDollarTotals::default());
        let missing =
            UsageHistorySample::from_model_history(120, 1_000, ModelDollarTotals::default());
        let last = UsageHistorySample::new(180, 1_000, 97.0, ModelDollarTotals::default());

        let markers = remaining_marker_positions(&[&first, &second, &missing, &last], 0, 180);

        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.boundary)
                .collect::<Vec<_>>(),
            [99, 98, 97]
        );
    }

    #[test]
    fn remaining_markers_keep_reset_anchor_before_multiple_missing_values() {
        let first_missing =
            UsageHistorySample::from_model_history(0, 1_000, ModelDollarTotals::default());
        let second_missing =
            UsageHistorySample::from_model_history(60, 1_000, ModelDollarTotals::default());
        let first_observed =
            UsageHistorySample::new(120, 1_000, 97.0, ModelDollarTotals::default());

        let markers =
            remaining_marker_positions(&[&first_missing, &second_missing, &first_observed], 0, 120);

        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.boundary)
                .collect::<Vec<_>>(),
            [99, 98, 97]
        );
        assert_eq!(markers.last().map(|marker| marker.x), Some(100.0));
    }

    #[test]
    fn remaining_markers_are_empty_without_data() {
        assert!(remaining_marker_positions(&[], 0, 120).is_empty());
    }

    #[test]
    fn graph_paths_hide_model_labels_without_spend_data() {
        let empty = graph_paths(&[], 100, 300);
        assert_eq!(empty.current_remaining_label, "—");
        assert!(empty.current_sol_label.is_empty());
        assert!(empty.current_terra_label.is_empty());
        assert!(empty.current_luna_label.is_empty());

        let zero = UsageHistorySample::new(120, 0, 70.0, ModelDollarTotals::default());
        let all_zero = graph_paths(&[&zero], 0, 300);
        assert_eq!(all_zero.current_remaining_label, "100%");
        assert!(all_zero.current_sol_label.is_empty());
        assert!(all_zero.current_terra_label.is_empty());
        assert!(all_zero.current_luna_label.is_empty());
    }

    #[test]
    fn current_label_connector_path_links_series_endpoint_to_displaced_label() {
        assert_eq!(
            current_label_connector_path(0.80, 0.68, true),
            "M0.00 80.00 L100.00 68.00"
        );
        assert_eq!(current_label_connector_path(0.80, 0.68, false), "");
        assert_eq!(current_label_connector_path(f32::NAN, 0.68, true), "");
    }

    #[test]
    fn focused_model_graph_rebases_the_selected_area_to_zero() {
        let samples = [
            UsageHistorySample::new(
                100,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 4.0,
                    terra: 2.0,
                    luna: 1.0,
                },
            ),
            UsageHistorySample::new(
                160,
                1_000,
                85.0,
                ModelDollarTotals {
                    sol: 8.0,
                    terra: 3.0,
                    luna: 2.0,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let paths = super::graph_paths_for_model(&references, 0, 200, "TERRA");
        assert!(paths.sol.is_empty());
        assert!(paths.luna.is_empty());
        assert!(paths.terra.starts_with("M0.00"));
        assert_eq!(paths.current_terra_label, "$3.00");
        assert!(paths.current_sol_label.is_empty());
    }

    #[test]
    fn token_graph_uses_token_axis_and_current_labels_without_changing_dollars() {
        let samples = [
            UsageHistorySample::new_with_usage(
                100,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 2.0,
                    luna: 3.0,
                },
                ModelTokenTotals {
                    sol: 1_000,
                    terra: 2_000,
                    luna: 3_000,
                },
            ),
            UsageHistorySample::new_with_usage(
                160,
                1_000,
                85.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 4.0,
                    luna: 5.0,
                },
                ModelTokenTotals {
                    sol: 2_000,
                    terra: 4_000,
                    luna: 8_000,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let dollars = graph_paths_for_selection(&references, 0, 200, true, true, true, false);
        let tokens = graph_paths_for_selection(&references, 0, 200, true, true, true, true);
        assert_eq!(dollars.dollar_labels[0], "$5.00");
        assert_eq!(tokens.dollar_labels[0], "8.0K");
        assert_eq!(tokens.current_luna_label, "8,000");
        assert_eq!(dollars.current_luna_label, "$5.00");
        assert!(!tokens.sol.contains('Z'));
        assert!(!tokens.luna.contains('Z'));
        assert!(!dollars.sol.contains('Z'));
        let sol_only = graph_paths_for_selection(&references, 0, 200, false, false, true, true);
        assert_eq!(sol_only.dollar_labels[0], "8.0K");
        assert_eq!(sol_only.sol, tokens.sol);
    }

    #[test]
    fn token_graph_carries_cumulative_values_across_legacy_zero_rows() {
        let samples = [
            UsageHistorySample::new_with_usage(
                100,
                1_000,
                95.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 2.0,
                    luna: 3.0,
                },
                ModelTokenTotals {
                    sol: 1_000,
                    terra: 2_000,
                    luna: 3_000,
                },
            ),
            // This is the shape of a legacy row: dollars are present, but
            // token columns were not available yet. It must not reset the
            // cumulative counters or turn the next observation into a delta.
            UsageHistorySample::new(
                160,
                1_000,
                94.0,
                ModelDollarTotals {
                    sol: 1.5,
                    terra: 2.5,
                    luna: 3.5,
                },
            ),
            UsageHistorySample::new_with_usage(
                220,
                1_000,
                93.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 3.0,
                    luna: 4.0,
                },
                ModelTokenTotals {
                    sol: 2_000,
                    terra: 4_000,
                    luna: 8_000,
                },
            ),
            // A later legacy row must not erase the latest known endpoint.
            UsageHistorySample::new(
                280,
                1_000,
                92.0,
                ModelDollarTotals {
                    sol: 2.5,
                    terra: 3.5,
                    luna: 4.5,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let points = minute_model_spend_for_metric(&references, true);
        assert_eq!(points.len(), 4);
        assert_eq!(
            (points[0].sol, points[0].terra, points[0].luna),
            (1_000.0, 2_000.0, 3_000.0)
        );
        assert_eq!(
            (points[1].sol, points[1].terra, points[1].luna),
            (1_000.0, 2_000.0, 3_000.0)
        );
        assert_eq!(
            (points[2].sol, points[2].terra, points[2].luna),
            (2_000.0, 4_000.0, 8_000.0)
        );
        assert_eq!(
            (points[3].sol, points[3].terra, points[3].luna),
            (2_000.0, 4_000.0, 8_000.0)
        );

        let graph = graph_paths_for_selection(&references, 0, 300, true, true, true, true);
        assert_eq!(graph.current_luna_label, "8,000");
        assert_eq!(graph.current_terra_label, "4,000");
        assert_eq!(graph.current_sol_label, "2,000");
    }

    #[test]
    fn token_remaining_line_stays_flat_when_only_dollar_rows_move() {
        let samples = [
            UsageHistorySample::new_with_usage(
                0,
                1_000,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                60,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 0.0,
                    luna: 0.0,
                },
                ModelTokenTotals::default(),
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let dollars = graph_paths_for_selection(&references, 0, 60, true, true, true, false);
        let tokens = graph_paths_for_selection(&references, 0, 60, true, true, true, true);

        assert_eq!(dollars.current_remaining_label, "90%");
        assert_eq!(tokens.current_remaining_label, "100%");
        assert!(tokens.remaining.ends_with("L100.00 1.00"));
    }

    #[test]
    fn token_remaining_line_connects_token_change_points() {
        let samples = [
            UsageHistorySample::new_with_usage(
                0,
                1_000,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                100,
                1_000,
                90.0,
                ModelDollarTotals::default(),
                ModelTokenTotals {
                    sol: 100,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                160,
                1_000,
                80.0,
                ModelDollarTotals::default(),
                ModelTokenTotals {
                    sol: 200,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let tokens = graph_paths_for_selection(&references, 0, 240, true, true, true, true);

        assert_eq!(
            tokens.remaining,
            "M0.00 1.00 L25.00 10.80 L50.00 20.60 L100.00 20.60"
        );
    }

    #[test]
    fn metric_selector_and_fixed_token_scale_contract() {
        let mut state = CodexInfoState::preview("normal");
        assert_eq!(state.selected_metric, "ドル");
        state.select_metric("トークン");
        assert_eq!(state.selected_metric, "トークン");
        state.select_metric("不正値");
        assert_eq!(state.selected_metric, "トークン");

        assert_eq!(GRAPH_METRIC_OPTIONS, ["ドル", "トークン"]);

        let sample = UsageHistorySample::new_with_usage(
            60,
            300,
            73.0,
            ModelDollarTotals {
                sol: 1.0,
                terra: 10.0,
                luna: 5.0,
            },
            ModelTokenTotals {
                sol: 100,
                terra: 1_000,
                luna: 500,
            },
        );
        let references = [&sample];
        let dollars_all = graph_paths_for_selection(&references, 0, 120, true, true, true, false);
        let dollars_sol = graph_paths_for_selection(&references, 0, 120, false, false, true, false);
        assert_eq!(dollars_all.dollar_labels[0], "$10.00");
        assert_eq!(dollars_sol.dollar_labels[0], "$1.00");

        let tokens_all = graph_paths_for_selection(&references, 0, 120, true, true, true, true);
        let tokens_sol = graph_paths_for_selection(&references, 0, 120, false, false, true, true);
        assert_eq!(tokens_all.dollar_labels, tokens_sol.dollar_labels);
        assert_eq!(tokens_all.dollar_labels[0], "1.0K");
        assert_eq!(tokens_all.sol_flat, tokens_sol.sol_flat);
        assert_eq!(tokens_all.sol_rising, tokens_sol.sol_rising);
        assert_eq!(tokens_all.remaining, dollars_all.remaining);
        assert_eq!(tokens_all.current_remaining_label, "73%");

        let zero = UsageHistorySample::new_with_usage(
            60,
            300,
            100.0,
            ModelDollarTotals::default(),
            ModelTokenTotals::default(),
        );
        let zero_paths = graph_paths_for_selection(&[&zero], 0, 120, true, true, true, true);
        assert_eq!(zero_paths.dollar_labels[0], "1");

        let source = include_str!("../ui/components.slint");
        assert!(source.contains("x: parent.width - 244px;"));
        assert!(source.contains("model: root.metric-options;"));
        assert!(source.contains("selected(value) => { root.select-metric(value); }"));
    }

    #[test]
    fn graph_selection_recalculates_axis_for_independent_enabled_models() {
        let samples = [
            UsageHistorySample::new(
                100,
                1_000,
                90.0,
                ModelDollarTotals {
                    sol: 4.0,
                    terra: 2.0,
                    luna: 1.0,
                },
            ),
            UsageHistorySample::new(
                160,
                1_000,
                85.0,
                ModelDollarTotals {
                    sol: 8.0,
                    terra: 3.0,
                    luna: 2.0,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let all = graph_paths_for_selection(&references, 0, 200, true, true, true, false);
        let terra_only = graph_paths_for_selection(&references, 0, 200, false, true, false, false);
        assert_eq!(all.dollar_labels[0], "$8.00");
        assert_eq!(terra_only.dollar_labels[0], "$3.00");
        assert!(terra_only.terra.starts_with("M0.00"));
        assert!(!all.sol.contains('Z'));
        assert!(!all.terra.contains('Z'));
        assert!(!all.luna.contains('Z'));
        assert!(terra_only.luna.is_empty());
        assert!(terra_only.sol.is_empty());
    }

    #[test]
    fn dollar_paths_are_independent_and_keep_sol_shape_when_other_lines_toggle() {
        let samples = [
            UsageHistorySample::new(
                0,
                1_000,
                100.0,
                ModelDollarTotals {
                    sol: 1.0,
                    terra: 0.0,
                    luna: 0.0,
                },
            ),
            UsageHistorySample::new(
                60,
                1_000,
                99.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 1.0,
                    luna: 0.5,
                },
            ),
            UsageHistorySample::new(
                120,
                1_000,
                98.0,
                ModelDollarTotals {
                    sol: 2.0,
                    terra: 1.0,
                    luna: 0.5,
                },
            ),
            UsageHistorySample::new(
                180,
                1_000,
                97.0,
                ModelDollarTotals {
                    sol: 4.0,
                    terra: 1.0,
                    luna: 0.5,
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let all = graph_paths_for_selection(&references, 0, 240, true, true, true, false);
        let sol_only = graph_paths_for_selection(&references, 0, 240, false, false, true, false);

        assert_eq!(all.dollar_labels[0], "$4.00");
        assert_eq!(all.current_sol_label, "$4.00");
        assert_eq!(all.sol, sol_only.sol);
        assert!(!all.sol.contains('Z'));
        assert!(!all.terra.contains('Z'));
        assert!(!all.luna.contains('Z'));

        let spend = smooth_model_spend(&graph_time_endpoints(
            minute_model_spend(&references),
            0,
            240,
        ));
        assert!(spend.windows(2).all(|pair| {
            pair[0].sol <= pair[1].sol
                && pair[0].terra <= pair[1].terra
                && pair[0].luna <= pair[1].luna
        }));
    }

    #[test]
    fn independent_lines_hold_zero_until_the_first_real_measurement() {
        let sample = UsageHistorySample::new(
            180,
            1_000,
            90.0,
            ModelDollarTotals {
                sol: 4.0,
                terra: 0.0,
                luna: 0.0,
            },
        );
        let paths = graph_paths(&[&sample], 0, 240);
        // x=75 is the first recorded point; x=0..75 must remain at the
        // baseline rather than becoming a fabricated diagonal spend trend.
        assert!(paths.sol.contains("L75.00 99.00 L75.00"));
    }

    #[test]
    fn segment_splitter_never_connects_an_invalid_decrease() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                sol: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 60,
                sol: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 120,
                sol: 3.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 180,
                sol: 2.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 240,
                sol: 2.0,
                ..HourlyModelSpend::default()
            },
        ];
        let (flat, rising) = split_metric_line_paths(&points, 0, 240, 3.0, |point| point.sol);

        assert!(flat.contains("M0.00 66.33 L25.00 66.33"));
        assert!(flat.contains("M75.00 33.67 L100.00 33.67"));
        assert!(rising.contains("M25.00 66.33 L50.00 1.00"));
        // The 3 -> 2 decrease at x=50..75 is a disconnected boundary.
        assert!(!flat.contains("M50.00"));
        assert!(!rising.contains("M50.00"));
    }

    #[test]
    fn model_graph_does_not_invent_spend_during_an_unobserved_gap() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                luna: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 60,
                luna: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 3_600,
                luna: 2.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 3_660,
                luna: 2.0,
                ..HourlyModelSpend::default()
            },
        ];
        let smoothed = smooth_model_spend(&points);
        assert_eq!(smoothed[1].luna, 1.0);
        assert_eq!(smoothed[2].luna, 2.0);
        let (flat, rising) = split_metric_line_paths(&smoothed, 0, 3_660, 2.0, |point| point.luna);
        // The 60..3600 interval is unobserved: stay at $1.00, then rise at
        // the observed 3600-second point. A diagonal here falsely claims
        // daytime consumption.
        assert!(flat.contains("M1.64 50.00 L98.36 50.00"));
        assert!(rising.contains("M98.36 50.00 L98.36 1.00"));
    }

    #[test]
    fn graph_selection_uses_one_monotonic_series_for_lines_and_current_values() {
        let reset_at = 1_000;
        let sample = |timestamp, dollars, tokens| {
            UsageHistorySample::new_with_usage(
                timestamp,
                reset_at,
                80.0,
                ModelDollarTotals {
                    sol: dollars,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: tokens,
                    ..ModelTokenTotals::default()
                },
            )
        };
        let samples = [
            sample(60, 10.0, 100),
            sample(120, 0.0, 0),
            sample(180, 12.0, 120),
        ];
        let references = samples.iter().collect::<Vec<_>>();

        let dollars = graph_paths_for_selection(&references, 0, 240, false, false, true, false);
        assert_eq!(dollars.current_sol_label, "$12.00");
        assert!(!dollars.sol_rising.contains("M50.00 99.00"));
        assert!(dollars.sol_flat.contains("M25.00 17.33 L50.00 17.33"));

        let tokens = graph_paths_for_selection(&references, 0, 240, false, false, true, true);
        assert_eq!(tokens.current_sol_label, "120");
        assert!(!tokens.sol_rising.contains("M50.00 99.00"));
        assert!(tokens.sol_flat.contains("M25.00 17.33 L50.00 17.33"));
    }

    #[test]
    fn first_observation_does_not_fabricate_a_diagonal_rise() {
        let points = graph_time_endpoints(
            vec![HourlyModelSpend {
                timestamp: 180,
                sol: 4.0,
                ..HourlyModelSpend::default()
            }],
            0,
            240,
        );
        let (flat, rising) = split_metric_line_paths(&points, 0, 240, 4.0, |point| point.sol);

        assert!(flat.contains("M0.00 99.00 L75.00 99.00"));
        assert!(flat.contains("M75.00 1.00 L100.00 1.00"));
        assert!(rising.contains("M75.00 99.00 L75.00 1.00"));
        assert!(!rising.contains("M0.00 99.00 L75.00 1.00"));
    }

    #[test]
    fn unused_intervals_mark_idle_segments_and_preserve_first_use_boundary() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 60,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 120,
                sol: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 180,
                sol: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 240,
                sol: 2.0,
                ..HourlyModelSpend::default()
            },
        ];
        assert_eq!(
            unused_interval_positions(&points, 0, 240),
            vec![
                UnusedIntervalPosition {
                    start: 0.0,
                    width: 25.0,
                    preserve_boundary: false,
                },
                UnusedIntervalPosition {
                    start: 50.0,
                    width: 25.0,
                    preserve_boundary: false,
                },
            ]
        );

        let first_use = [
            HourlyModelSpend {
                timestamp: 0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 180,
                sol: 4.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 240,
                sol: 4.0,
                ..HourlyModelSpend::default()
            },
        ];
        assert_eq!(
            unused_interval_positions(&first_use, 0, 240),
            vec![
                UnusedIntervalPosition {
                    start: 0.0,
                    width: 75.0,
                    preserve_boundary: true,
                },
                UnusedIntervalPosition {
                    start: 75.0,
                    width: 25.0,
                    preserve_boundary: false,
                },
            ]
        );
    }

    #[test]
    fn unused_intervals_mark_long_gap_before_observed_spend() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                sol: 1.0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 3_600,
                sol: 2.0,
                ..HourlyModelSpend::default()
            },
        ];
        assert_eq!(
            unused_interval_positions(&points, 0, 3_600),
            vec![UnusedIntervalPosition {
                start: 0.0,
                width: 100.0,
                preserve_boundary: true,
            }]
        );
    }

    #[test]
    fn unused_intervals_merge_adjacent_flat_segments() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                sol: 2.0,
                terra: 1.0,
                luna: 3.0,
            },
            HourlyModelSpend {
                timestamp: 60,
                sol: 2.0,
                terra: 1.0,
                luna: 3.0,
            },
            HourlyModelSpend {
                timestamp: 120,
                sol: 2.0,
                terra: 1.0,
                luna: 3.0,
            },
            HourlyModelSpend {
                timestamp: 180,
                sol: 2.0,
                terra: 1.0,
                luna: 3.0,
            },
        ];
        assert_eq!(
            unused_interval_positions(&points, 0, 180),
            vec![UnusedIntervalPosition {
                start: 0.0,
                width: 100.0,
                preserve_boundary: false,
            }]
        );
    }

    #[test]
    fn unused_intervals_use_the_selected_dollar_or_token_metric() {
        let samples = [
            UsageHistorySample::new_with_usage(
                0,
                1_000,
                100.0,
                ModelDollarTotals::default(),
                ModelTokenTotals::default(),
            ),
            UsageHistorySample::new_with_usage(
                60,
                1_000,
                99.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 100,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                120,
                1_000,
                98.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 200,
                    ..ModelTokenTotals::default()
                },
            ),
            UsageHistorySample::new_with_usage(
                180,
                1_000,
                97.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
                ModelTokenTotals {
                    sol: 200,
                    ..ModelTokenTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let dollars = graph_paths_for_selection(&references, 0, 180, true, true, true, false);
        let tokens = graph_paths_for_selection(&references, 0, 180, true, true, true, true);

        assert_eq!(dollars.unused_intervals.len(), 1);
        assert!((dollars.unused_intervals[0].start - 33.3333333333).abs() < 0.000_001);
        assert!((dollars.unused_intervals[0].width - 66.6666666667).abs() < 0.000_001);
        assert_eq!(tokens.unused_intervals.len(), 1);
        assert!((tokens.unused_intervals[0].start - 66.6666666667).abs() < 0.000_001);
        assert!((tokens.unused_intervals[0].width - 33.3333333333).abs() < 0.000_001);
    }

    #[test]
    fn dollar_idle_bands_use_raw_cumulative_values_before_line_smoothing() {
        let samples = [
            UsageHistorySample::new(0, 1_000, 100.0, ModelDollarTotals::default()),
            UsageHistorySample::new(60, 1_000, 99.0, ModelDollarTotals::default()),
            UsageHistorySample::new(
                120,
                1_000,
                98.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
            ),
            UsageHistorySample::new(
                180,
                1_000,
                97.0,
                ModelDollarTotals {
                    sol: 1.0,
                    ..ModelDollarTotals::default()
                },
            ),
        ];
        let references = samples.iter().collect::<Vec<_>>();
        let paths = graph_paths(&references, 0, 180);

        assert_eq!(paths.unused_intervals.len(), 2);
        assert!((paths.unused_intervals[0].start - 0.0).abs() < 0.000_001);
        assert!((paths.unused_intervals[0].width - 33.3333333333).abs() < 0.000_001);
        assert!((paths.unused_intervals[1].start - 66.6666666667).abs() < 0.000_001);
        assert!((paths.unused_intervals[1].width - 33.3333333333).abs() < 0.000_001);
    }

    #[test]
    fn current_graph_labels_are_clamped_and_separated_at_minimum_size() {
        let mut paths = GraphPaths {
            current_remaining_label: "50%".into(),
            current_luna_label: "$1.00".into(),
            current_terra_label: "$1.00".into(),
            current_sol_label: "$1.00".into(),
            current_remaining_y: 0.5,
            current_luna_y: 0.5,
            current_terra_y: 0.5,
            current_sol_y: 0.5,
            ..GraphPaths::default()
        };
        separate_current_label_positions(&mut paths, true, true, true, true);
        let mut positions = [
            paths.current_remaining_y,
            paths.current_luna_y,
            paths.current_terra_y,
            paths.current_sol_y,
        ];
        positions.sort_by(f32::total_cmp);
        let minimum = 8.0 / 204.0;
        let maximum = 1.0 - minimum;
        assert!(positions[0] >= minimum);
        assert!(positions[3] <= maximum);
        for pair in positions.windows(2) {
            assert!((pair[1] - pair[0]) * 204.0 >= 15.999);
        }
    }

    #[test]
    fn remaining_graph_keeps_the_reset_start_anchor_without_observations() {
        let paths = graph_paths(&[], 100, 300);
        assert_eq!(paths.remaining, "M0.00 1.00");
    }

    #[test]
    fn smoothing_keeps_remaining_and_spend_cumulative() {
        let remaining =
            smooth_remaining_points(&[(0, 100.0), (60, 60.0), (120, 70.0), (180, 20.0)]);
        assert!(remaining.windows(2).all(|pair| pair[0].1 >= pair[1].1));

        let spend = smooth_model_spend(&[
            HourlyModelSpend {
                timestamp: 0,
                sol: 0.0,
                terra: 0.0,
                luna: 0.0,
            },
            HourlyModelSpend {
                timestamp: 60,
                sol: 1.0,
                terra: 2.0,
                luna: 3.0,
            },
            HourlyModelSpend {
                timestamp: 120,
                sol: 4.0,
                terra: 5.0,
                luna: 6.0,
            },
        ]);
        assert!(spend.windows(2).all(|pair| {
            pair[0].sol <= pair[1].sol
                && pair[0].terra <= pair[1].terra
                && pair[0].luna <= pair[1].luna
        }));
    }

    #[test]
    fn zero_cost_period_draws_a_visible_baseline() {
        let points = [
            HourlyModelSpend {
                timestamp: 0,
                ..HourlyModelSpend::default()
            },
            HourlyModelSpend {
                timestamp: 60,
                ..HourlyModelSpend::default()
            },
        ];
        assert_eq!(
            stacked_area_path(&points, 0, 60, 0.0, |_| (0.0, 0.0)),
            "M0.00 99.00 L100.00 99.00"
        );
    }

    #[test]
    fn period_label_has_month_and_day_for_both_endpoints() {
        let label = format_period_label(1_700_000_000, 1_700_086_400);
        assert_eq!(label, "2023/11/15 07:13:20 JST ～ 2023/11/16 07:13:20 JST");
        assert_eq!(label.matches('/').count(), 4);
        assert_eq!(label.matches(" JST").count(), 2);
        assert_eq!(label.matches(" ～ ").count(), 1);
        let endpoints: Vec<_> = label.split(" ～ ").collect();
        assert_eq!(endpoints.len(), 2);
        assert!(endpoints
            .iter()
            .all(|endpoint| endpoint.contains('/') && endpoint.contains(':')));
    }

    #[test]
    fn graph_period_row_only_displays_period_label() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        assert!(graph.contains("model: root.history-period-options;"));
        assert!(graph.contains("current-index: root.selected-history-index;"));
        assert!(!graph.contains("古い期間"));
        assert!(!graph.contains("新しい期間"));
    }

    #[test]
    fn graph_history_placeholder_is_not_selectable() {
        let source = include_str!("../ui/components.slint");
        let graph_select = source
            .split_once("export component GraphSelect inherits Rectangle {")
            .and_then(|(_, source)| source.split_once("export component Header"))
            .map(|(source, _)| source)
            .expect("GraphSelect component");
        assert!(graph_select.contains(
            "enabled: root.model.length > 0 && !(root.model.length == 1 && root.model[0] == \"履歴なし\");"
        ));
    }

    #[test]
    fn graph_history_popup_overlays_plot_without_reflowing_it() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        assert!(graph.contains("popup-above: false;"));
        assert!(graph.contains("y: 72px;"));
        assert!(graph.contains("history-toggle-y: 144px;"));
        assert!(!graph.contains("history-toggle-y: history-select.popup-open ?"));
        assert!(graph.contains("z: 2;"));
        assert!(graph.contains("y: root.history-toggle-y + 32px;"));
        assert!(source.contains("y: root.popup-above ? 0px : root.height;"));
        assert!(source.contains(
            "out property <length> popup-height: min(130px, max(root.item-height + 2px, root.model.length * root.item-height + 2px));"
        ));
        assert!(source.contains("popup-list := ListView"));
    }

    #[test]
    fn graph_metric_popup_uses_the_reserved_left_band() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        let metric = graph
            .split("model: root.metric-options;")
            .nth(1)
            .expect("metric selector");
        assert!(metric.contains("popup-above: true;"));
        assert!(metric.contains("popup-x: -128px;"));
    }

    #[test]
    fn graph_controls_use_one_visual_boundary_and_show_short_histories() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        assert!(source.contains(
            "out property <length> popup-height: min(130px, max(root.item-height + 2px, root.model.length * root.item-height + 2px));"
        ));
        assert!(graph.contains("background: DesignTokens.graph-control-surface;"));
        assert!(graph.contains("opacity: 0.72;"));
        assert!(graph.contains("in-out property <[GraphUnusedInterval]> unused-intervals;"));
        assert!(graph.contains("for interval in root.unused-intervals: Rectangle"));
        assert!(graph.contains("background: DesignTokens.graph-idle-band;"));
        assert!(graph.contains("opacity: 0.22;"));
        let theme = include_str!("../ui/theme.slint");
        assert!(theme.contains("graph-idle-band: #3f5d7c;"));
        let toggle = source
            .split("component GraphToggle inherits Rectangle {")
            .nth(1)
            .and_then(|body| body.split("export struct RemainingMarker").next())
            .expect("GraphToggle");
        assert!(toggle.contains("background: transparent;"));
        assert!(!toggle.contains("border-width: 1px;"));
        assert!(toggle.contains("text: root.label;"));
        assert!(!toggle.contains("strings.on"));
        assert!(!toggle.contains("strings.off"));
    }

    #[test]
    fn graph_idle_model_paths_use_quiet_strokes() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        for path_name in ["luna-flat-path", "terra-flat-path", "sol-flat-path"] {
            let path = graph
                .split("Path {")
                .find(|body| body.contains(&format!("commands: root.{path_name};")))
                .expect(path_name);
            assert!(path.contains("stroke-width: 1px;"), "{path_name}");
            assert!(path.contains("opacity: 0.5;"), "{path_name}");
        }
    }

    #[test]
    fn remaining_graph_stroke_overlays_small_boundary_markers() {
        let source = include_str!("../ui/components.slint");
        let graph = source
            .split("export component GraphWindow inherits Window {")
            .nth(1)
            .expect("GraphWindow");
        let marker_start = graph
            .find("for marker in root.remaining-markers: Rectangle {")
            .expect("remaining marker loop");
        let path_start = graph[marker_start..]
            .find("commands: root.remaining-path;")
            .map(|offset| marker_start + offset)
            .expect("remaining path");
        assert!(path_start > marker_start);
        let marker_block = &graph[marker_start..path_start];
        assert!(marker_block.contains("width: 2px;"));
        assert!(marker_block.contains("height: 2px;"));
        assert!(marker_block.contains("border-radius: 1px;"));
    }

    #[test]
    fn graph_many_preview_exercises_scrollable_period_history() {
        let state = CodexInfoState::preview("graph-many");
        assert!(state.history_periods().len() >= 6);
        assert!(state.history_period_options().len() >= 6);
    }

    #[test]
    fn graph_collision_preview_matches_the_historical_singleton_oracle() {
        let state = CodexInfoState::preview("graph-collision");
        let reset = state
            .selected_history_reset()
            .expect("selected preview period");
        let period = state
            .history
            .period_for_id(reset, Utc::now().timestamp(), state.reset_at)
            .expect("preview period");
        let selected = state.history.samples_for_reset(Some(reset));
        assert!(selected
            .iter()
            .all(|sample| (sample.remaining_percent - 14.0).abs() > f64::EPSILON));
        let references = selected.iter().collect::<Vec<_>>();
        let points = remaining_graph_points(&references, period.start, period.end);
        assert_eq!(points.first().map(|(_, value)| *value), Some(88.0));
        assert_eq!(points.last().map(|(_, value)| *value), Some(87.0));
        assert!(points.iter().all(|(_, value)| *value >= 87.0));

        // Exercise the same serialized graph payload that `sync_graph_window`
        // publishes to Slint. The unrelated 14% singleton must not cross the
        // selected-period boundary into the rendered graph.
        let payload: Vec<UsageHistorySample> =
            serde_json::from_str(&state.graph_data()).expect("graph payload serializes");
        assert!(!payload
            .iter()
            .any(|sample| (sample.remaining_percent - 14.0).abs() < f64::EPSILON));
        assert!(payload.iter().all(|sample| sample.reset_at == reset));
    }

    #[test]
    fn graph_period_preview_opens_the_history_selector_for_visual_review() {
        let source = include_str!("../ui/components.slint");
        assert!(source.contains("in property <bool> open-on-start: false;"));
        assert!(source.contains("open-on-start: root.open-history-on-start;"));
        assert!(source.contains("interval: 100ms;"));
        let main = include_str!("main.rs");
        assert!(
            main.contains(
                "Some(\"graph\" | \"graph-old\" | \"graph-many\" | \"graph-period\" | \"graph-collision\")"
            )
        );
        assert!(main.contains("graph.set_open_history_on_start(graph_period_preview);"));
    }

    #[test]
    fn non_graph_surfaces_do_not_add_outer_frames() {
        let source = include_str!("../ui/components.slint");
        for name in [
            "export component RemainingQuota inherits Rectangle {",
            "export component WeekGauge inherits Rectangle {",
            "export component AccountActivity inherits Rectangle {",
            "export component ModelUsage inherits Rectangle {",
            "export component StatusBanner inherits Rectangle {",
        ] {
            let body = source.split(name).nth(1).expect(name);
            let header = body.lines().take(12).collect::<Vec<_>>().join("\n");
            assert!(
                !header.contains("border-width: 1px;"),
                "unexpected frame: {name}"
            );
        }
    }

    #[test]
    fn graph_window_receives_a_full_width_graph_path() {
        let Ok(graph) = GraphWindow::new() else {
            return;
        };
        let commands = "M0.00 99.00 L100.00 99.00";
        graph.set_sol_flat_path(commands.into());
        graph.set_sol_rising_path(commands.into());
        assert_eq!(graph.get_sol_flat_path().as_str(), commands);
        assert_eq!(graph.get_sol_rising_path().as_str(), commands);
        assert!(graph.get_show_remaining());
        assert!(graph.get_show_luna());
        assert!(graph.get_show_terra());
        assert!(graph.get_show_sol());
        assert!(!graph.get_show_tokens());
        graph.set_show_remaining(false);
        assert!(!graph.get_show_remaining());
        graph.set_show_tokens(true);
        assert!(graph.get_show_tokens());
    }

    #[test]
    fn threads_window_list_explicitly_clips_rows_below_the_fixed_header() {
        let threads = include_str!("../ui/components.slint")
            .split("export component ThreadsWindow inherits Window {")
            .nth(1)
            .expect("ThreadsWindow");
        let thread_list_clip = threads
            .split("thread-list-clip := Rectangle {")
            .nth(1)
            .expect("thread-list clip rectangle");
        assert!(thread_list_clip.contains("y: 76px;"));
        assert!(thread_list_clip.contains("width: 840px;"));
        assert!(thread_list_clip.contains("height: 384px;"));
        assert!(thread_list_clip.contains("clip: true;"));
        assert!(thread_list_clip.contains("thread-list := ListView {"));
    }

    #[test]
    fn threads_window_uses_readable_primary_metadata_layout() {
        let threads = include_str!("../ui/components.slint")
            .split("export component ThreadsWindow inherits Window {")
            .nth(1)
            .expect("ThreadsWindow");
        assert!(threads.contains("property <bool> single-thread: root.thread-rows.length == 1;"));
        assert!(threads
            .contains("property <length> thread-row-height: root.single-thread ? 384px : 128px;"));
        assert!(threads.contains("height: root.thread-row-height;"));
        assert!(threads.contains("font-size: root.single-thread ? 28px : 20px;"));
        assert!(threads.contains("font-size: root.single-thread ? 22px : 18px;"));
        assert!(threads.contains("font-size: 28px;"));
        assert!(threads.contains("font-size: 24px;"));
        assert!(threads.contains("font-size: 18px;"));
        assert!(threads.contains("width: root.single-thread ? 90px : 78px;"));
        assert!(threads.contains("width: 268px;"));
        assert!(threads.contains("text: row.model;"));
        assert!(threads.contains("text: root.strings.running + \" \" + row.thread-age;"));
        assert!(threads.contains("text: root.strings.instruction + \" \" + row.instruction-age;"));
        assert!(threads.contains("text: root.strings.running;"));
        assert!(threads.contains("text: root.strings.instruction;"));
        assert!(threads.contains("text: root.strings.tokens;"));
        assert!(threads.contains("text: row.tokens;"));
        assert!(threads.contains("text: row.context-usage;"));
        assert!(threads.contains("text: root.strings.context-usage;"));
        assert!(threads.contains("text: row.context-usage;"));
        assert!(threads.contains("text: root.strings.context-usage;"));
        assert!(threads.contains("width: parent.width - 486px;"));
        assert!(threads.contains("property <bool> has-parent-title: row.parent-title != \"\";"));
        assert!(threads.contains("visible: !root.single-thread || parent.has-parent-title;"));
        assert!(threads.contains("y: parent.has-parent-title ? 132px : 84px;"));
        assert!(!threads.contains("row.elapsed"));
        assert!(threads.contains("x: root.single-thread ? 400px : parent.width - 560px;"));
        assert!(threads.contains("x: parent.width - 300px;"));
    }

    #[test]
    fn single_thread_preview_uses_the_full_detail_viewport() {
        let source = include_str!("main.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("production source");
        assert!(source.contains("Some(\"multi-thread\" | \"single-thread\")"));
        let state = CodexInfoState::preview("single-thread");
        assert_eq!(state.active_threads.len(), 1);
    }

    #[test]
    fn threads_window_header_stays_above_the_scrolling_rows() {
        let threads = include_str!("../ui/components.slint")
            .split("export component ThreadsWindow inherits Window {")
            .nth(1)
            .expect("ThreadsWindow");
        let header = threads
            .split("header-panel := Rectangle {")
            .nth(1)
            .and_then(|source| source.split("thread-list-clip := Rectangle {").next())
            .expect("fixed header panel");
        assert!(header.contains("x: 30px;"));
        assert!(header.contains("y: 20px;"));
        assert!(header.contains("width: 840px;"));
        assert!(header.contains("height: 48px;"));
        assert!(header.contains("background: DesignTokens.canvas;"));
        assert!(header.contains("font-size: 22px;"));
        assert!(header.contains("font-size: 16px;"));
        assert!(header.contains("z: 2;"));
    }

    #[test]
    fn shared_admission_rejects_invalid_snapshot_before_commit_in_service_and_ui() {
        let old = active_thread_fixture(0, 100);
        let mut service_state = CodexInfoState::preview("normal");
        let mut ui_state = CodexInfoState::preview("normal");
        service_state.active_threads = vec![old.clone()];
        ui_state.active_threads = vec![old.clone()];

        let over_capacity = (0..=256)
            .map(|index| active_thread_fixture(index, 200 - index as i64))
            .collect::<Vec<_>>();
        for state in [&mut service_state, &mut ui_state] {
            state.apply_thread_result(
                state.auth_epoch,
                ActiveThreadUpdate::Snapshot(over_capacity.clone()),
            );
            assert_eq!(state.active_threads.as_slice(), std::slice::from_ref(&old));
            assert!(state.thread_error);
            assert!(!state
                .active_threads
                .iter()
                .any(|thread| thread.id == "thread-256"));
        }

        let duplicate = vec![active_thread_fixture(1, 210), active_thread_fixture(1, 209)];
        for state in [&mut service_state, &mut ui_state] {
            state.apply_thread_result(
                state.auth_epoch,
                ActiveThreadUpdate::Snapshot(duplicate.clone()),
            );
            assert_eq!(state.active_threads.as_slice(), std::slice::from_ref(&old));
            assert!(state.thread_error);
        }

        let mut cycle_a = active_thread_fixture(10, 220);
        cycle_a.parent_thread_id = Some("thread-011".into());
        let mut cycle_b = active_thread_fixture(11, 219);
        cycle_b.parent_thread_id = Some("thread-010".into());
        for state in [&mut service_state, &mut ui_state] {
            state.apply_thread_result(
                state.auth_epoch,
                ActiveThreadUpdate::Snapshot(vec![cycle_a.clone(), cycle_b.clone()]),
            );
            assert_eq!(state.active_threads.as_slice(), std::slice::from_ref(&old));
            assert!(state.thread_error);
        }
    }

    #[test]
    fn failed_thread_update_preserves_rows_but_nothread_clears_after_success() {
        let old = active_thread_fixture(0, 100);
        let mut state = CodexInfoState::preview("normal");
        state.active_threads = vec![old.clone()];

        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::Failed);
        assert_eq!(state.active_threads, [old]);
        assert!(state.thread_error);

        state.apply_thread_result(state.auth_epoch, ActiveThreadUpdate::NoThread);
        assert!(state.active_threads.is_empty());
        assert!(!state.thread_error);
    }

    #[test]
    fn every_parent_id_defines_x_hierarchy_even_without_subagent_flag() {
        let mut parent = active_thread_fixture(0, 100);
        parent.is_subagent = false;
        parent.parent_thread_id = None;
        let mut child = active_thread_fixture(1, 90);
        child.is_subagent = false;
        child.parent_thread_id = Some(parent.id.clone());

        let rows = thread_presentation_rows(&[parent, child]);
        assert_eq!(rows.len(), 2);
        assert!(rows[1].connected_to_parent);
        assert_eq!(rows[1].forest_depth, 1);
    }

    #[test]
    fn real_acquisition_cycle_rejection_preserves_x_rest_and_recovers() {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "codex-info-real-acquisition-cycle-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let accepted_root_path = root.join("accepted-root.jsonl");
        let accepted_child_path = root.join("accepted-child.jsonl");
        let cycle_a_path = root.join("cycle-a.jsonl");
        let cycle_b_path = root.join("cycle-b.jsonl");
        let replacement_path = root.join("replacement.jsonl");
        write_distinct_running_rollout(
            &accepted_root_path,
            "accepted-root-model",
            100,
            10,
            1000,
            "2026-01-01T00:00:01Z",
            false,
        );
        write_distinct_running_rollout(
            &accepted_child_path,
            "accepted-child-model",
            200,
            20,
            1000,
            "2026-01-01T00:00:02Z",
            false,
        );
        write_distinct_running_rollout(
            &cycle_a_path,
            "cycle-a-model",
            300,
            30,
            1000,
            "2026-01-01T00:00:03Z",
            false,
        );
        write_distinct_running_rollout(
            &cycle_b_path,
            "cycle-b-model",
            400,
            40,
            1000,
            "2026-01-01T00:00:04Z",
            false,
        );
        write_distinct_running_rollout(
            &replacement_path,
            "replacement-model",
            500,
            50,
            1000,
            "2026-01-01T00:00:05Z",
            false,
        );

        let named_item = |id: &str, updated_at: i64, path: &Path| {
            let mut item = thread_list_item(id, updated_at, path);
            item["name"] = json!(id);
            item
        };
        let accepted_root = named_item("accepted-root", 200, &accepted_root_path);
        let mut accepted_child = named_item("accepted-child", 190, &accepted_child_path);
        accepted_child["source"] = json!({
            "subAgent": {"thread_spawn": {
                "parent_thread_id": "accepted-root",
                "depth": 1
            }}
        });

        let active_paths = BTreeSet::from([
            fs::canonicalize(&accepted_root_path).unwrap(),
            fs::canonicalize(&accepted_child_path).unwrap(),
            fs::canonicalize(&cycle_a_path).unwrap(),
            fs::canonicalize(&cycle_b_path).unwrap(),
            fs::canonicalize(&replacement_path).unwrap(),
        ]);
        let (sender, receiver) = mpsc::channel();
        let mut next_id = 700_u64;
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 700,
                        "result": {"data": [accepted_root, accepted_child]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let mut input = Vec::new();
        let accepted_update = fetch_active_thread_update_for_paths_and_state(
            &mut input,
            &receiver,
            &mut next_id,
            &root,
            &active_paths,
            None,
        );
        assert!(matches!(
            &accepted_update,
            ActiveThreadUpdate::Snapshot(rows)
                if rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()
                    == ["accepted-root", "accepted-child"]
        ));
        assert_eq!(next_id, 701);
        assert_eq!(String::from_utf8(input).unwrap().lines().count(), 1);

        let mut state = CodexInfoState::preview("normal");
        state.history = UsageHistory::default();
        state.apply_thread_result(state.auth_epoch, accepted_update);
        assert!(!state.thread_error);
        let expected_old_ids = ["accepted-root", "accepted-child"];
        assert_eq!(
            state
                .active_threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            expected_old_ids
        );
        let old_x_rows = active_thread_rows_at(&state.active_threads, 10_000);
        assert_eq!(
            old_x_rows
                .iter()
                .map(|row| row.title.to_string())
                .collect::<Vec<_>>(),
            expected_old_ids
        );

        let mut server =
            ApiServer::start(ApiServerConfig::new("127.0.0.1:0".parse().unwrap()).unwrap())
                .unwrap();
        let publisher = server.publisher();
        publisher.publish_details(state.public_details()).unwrap();
        let old_status = raw_loopback_get(server.local_addr(), "/v1/status");
        let old_details = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_eq!(
            raw_loopback_pair(&old_status),
            raw_loopback_pair(&old_details)
        );
        assert_eq!(raw_loopback_body(&old_status)["state"], "ready");
        assert_eq!(
            raw_loopback_body(&old_details)["threads"][0]["id"],
            "accepted-root"
        );
        assert_eq!(
            raw_loopback_body(&old_details)["threads"][1]["id"],
            "accepted-child"
        );

        let mut cycle_a = named_item("cycle-a", 400, &cycle_a_path);
        cycle_a["source"] = json!({
            "subAgent": {"thread_spawn": {
                "parent_thread_id": "cycle-b",
                "depth": 1
            }}
        });
        let mut cycle_b = named_item("cycle-b", 390, &cycle_b_path);
        cycle_b["source"] = json!({
            "subAgent": {"thread_spawn": {
                "parent_thread_id": "cycle-a",
                "depth": 1
            }}
        });
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 701,
                        "result": {"data": [cycle_a, cycle_b]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let cycle_update = fetch_active_thread_update_for_paths_and_state(
            &mut Vec::new(),
            &receiver,
            &mut next_id,
            &root,
            &active_paths,
            None,
        );
        assert!(matches!(
            &cycle_update,
            ActiveThreadUpdate::Snapshot(rows)
                if rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()
                    == ["cycle-a", "cycle-b"]
        ));
        assert_eq!(next_id, 702);
        state.apply_thread_result(state.auth_epoch, cycle_update);
        assert!(state.thread_error);
        assert_eq!(
            state
                .active_threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            expected_old_ids
        );
        let cycle_x_rows = active_thread_rows_at(&state.active_threads, 10_000);
        let cycle_x_titles = cycle_x_rows
            .iter()
            .map(|row| row.title.to_string())
            .collect::<Vec<_>>();
        assert_eq!(cycle_x_titles, expected_old_ids);
        assert!(!cycle_x_titles
            .iter()
            .any(|id| id == "cycle-a" || id == "cycle-b"));

        let reset_at = state.reset_at.expect("preview quota has reset");
        state.apply_usage_event(usage_event(Some(37.0), reset_at));
        state.model_usage = vec![ModelUsageRow {
            name: "SOL".into(),
            tokens: 1_750,
            input_tokens: 1_500,
            cached_input_tokens: 500,
            output_tokens: 250,
        }];
        publisher.publish_details(state.public_details()).unwrap();
        let cycle_status = raw_loopback_get(server.local_addr(), "/v1/status");
        let cycle_details = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_ne!(
            raw_loopback_pair(&cycle_status),
            raw_loopback_pair(&old_status)
        );
        assert_eq!(
            raw_loopback_pair(&cycle_status),
            raw_loopback_pair(&cycle_details)
        );
        for body in [
            raw_loopback_body(&cycle_status),
            raw_loopback_body(&cycle_details),
        ] {
            assert_eq!(body["state"], "error");
            assert_eq!(body["quota"]["remaining_percent"], 37.0);
            assert_eq!(body["models"][0]["name"], "SOL");
            assert_eq!(body["models"][0]["input_tokens"], 1_000);
            assert_eq!(body["models"][0]["cached_input_tokens"], 500);
            assert_eq!(body["models"][0]["output_tokens"], 250);
        }
        let cycle_details_body = raw_loopback_body(&cycle_details);
        assert_eq!(cycle_details_body["active_thread_count"], 2);
        assert_eq!(cycle_details_body["threads"][0]["id"], "accepted-root");
        assert_eq!(cycle_details_body["threads"][1]["id"], "accepted-child");
        assert!(!cycle_details_body["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| { thread["id"] == "cycle-a" || thread["id"] == "cycle-b" }));

        let replacement = named_item("replacement", 500, &replacement_path);
        sender
            .send(RpcReadEvent::Line(
                super::security::RpcLine::new(
                    json!({
                        "id": 702,
                        "result": {"data": [replacement]}
                    })
                    .to_string(),
                )
                .unwrap(),
            ))
            .unwrap();
        let replacement_update = fetch_active_thread_update_for_paths_and_state(
            &mut Vec::new(),
            &receiver,
            &mut next_id,
            &root,
            &active_paths,
            None,
        );
        assert!(matches!(
            &replacement_update,
            ActiveThreadUpdate::Snapshot(rows)
                if rows.len() == 1 && rows[0].id == "replacement"
        ));
        assert_eq!(next_id, 703);
        state.apply_thread_result(state.auth_epoch, replacement_update);
        assert!(!state.thread_error);
        assert_eq!(
            state
                .active_threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["replacement"]
        );
        let replacement_x_rows = active_thread_rows_at(&state.active_threads, 10_000);
        assert_eq!(
            replacement_x_rows
                .iter()
                .map(|row| row.title.to_string())
                .collect::<Vec<_>>(),
            ["replacement"]
        );
        publisher.publish_details(state.public_details()).unwrap();
        let recovered_status = raw_loopback_get(server.local_addr(), "/v1/status");
        let recovered_details = raw_loopback_get(server.local_addr(), "/v1/details");
        assert_eq!(
            raw_loopback_pair(&recovered_status),
            raw_loopback_pair(&recovered_details)
        );
        assert_eq!(raw_loopback_body(&recovered_status)["state"], "ready");
        assert_eq!(
            raw_loopback_body(&recovered_status)["active_thread_count"],
            1
        );
        let recovered_details_body = raw_loopback_body(&recovered_details);
        assert_eq!(recovered_details_body["state"], "ready");
        assert_eq!(recovered_details_body["active_thread_count"], 1);
        assert_eq!(
            recovered_details_body["threads"].as_array().unwrap().len(),
            1
        );
        assert_eq!(recovered_details_body["threads"][0]["id"], "replacement");
        assert!(!recovered_details_body["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| {
                matches!(
                    thread["id"].as_str(),
                    Some("accepted-root" | "accepted-child" | "cycle-a" | "cycle-b")
                )
            }));
        server.shutdown();
        let _ = fs::remove_dir_all(root);
    }
}
