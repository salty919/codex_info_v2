<!-- Working requirements ledger for Windows parity work. -->

> **文書の位置づけ:** 本書は製品要件の非規範的な案内／実装説明です。要求の入口とowner registryは `docs/PRODUCT_REQUIREMENTS.md` および同文書から参照される REST/DATA/UX/LOCALIZATION 仕様です。要求変更時は該当する master ID と owner に従い、本書だけで契約や監査成果物を追加・変更しません。

# Windows client parity ledger

Document state: `REQUIREMENTS_SELECTED / PRODUCT_PENDING` (requirements selection is not
implementation or product-acceptance evidence).

SSH-001/RC-061〜063の追加契約もこのledgerへ参照としてjoinする。状態は
`REQUIREMENTS_SELECTED / PRODUCT_PENDING`であり、installed API serviceのexact install/start/stop/restart/
uninstall/rollback command、実装、host、artifact、fresh image、独立製品判定は未取得である。

This ledger records the user correction that the Windows client must not lose
functionality that already exists in the native Rust/Slint client.

## Objective

Provide the existing Codex Info behavior on Windows without weakening the
loopback/SSH trust boundary or exposing credentials and backend error details.

## New request clauses (2026-08-22)

The Windows client must also provide a Windows-native, polished visual system
without removing any native/X functionality. Icons and free/open-source assets
may be used when their licenses are included in the distribution notices. The
client must support the same user-facing language choices as the native client,
with deterministic fallback and locale-aware dates/numbers. First launch,
connection setup, authentication, settings, recovery, and help must form a
complete usable path rather than exposing a bare error screen. Any additional
view, setting, or decoration must preserve the existing fixed loopback/SSH
boundary and must not hide or duplicate values defined by the linked master.

## Non-goals

- Do not remove or redesign the existing Linux/WSL native UI.
- Do not accept a monitoring-only subset as feature parity.
- Do not expose Codex tokens, passwords, account email, filesystem paths, or
  raw backend errors through the Windows API.

## Invariants

- The Windows client communicates through the authenticated SSH local tunnel
  and loopback API only.
- Existing native state ownership, reset-period semantics, token accounting,
  and failure isolation remain governed by the linked master.
- A failed auxiliary request must not clear an unrelated last-good value.
- Window geometry, DPI, placement, native input, and viewport rules are Windows
  expression contracts only; X/native values, periods, ordering, ownership,
  and failure retention remain unchanged.

## SSH-001 / RC-061〜063 connection boundary

- Settings keys are exactly `language`, `setupCompleted`, `connectionConfigured`, `timeZoneId`,
  `connectionProfile`, and `connectionSelector`; the six-key object is serialized, flushed, validated,
  and atomically replaced.
- `connectionProfile` is `none|wsl|sshConfigAlias`. `connectionSelector` is literal `none`, an installed
  WSL distribution exact token, or a literal OpenSSH `Host` alias matching
  `^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$`.
- Password, token, key, path, OpenSSH-expanded values, raw manual host/user, API URL, argv, and stderr are
  never persisted. Raw manual host/user is one-session recovery only and never durable completion.
- Old four-key settings, corrupt settings, and invalid profile/selector do not enter a Welcome loop: Main is
  disconnected, Settings owns recovery, and automatic recovery command count is zero.
- SSH/WSL/bootstrap/tunnel children use direct executable + `ArgumentList`; shell/cmd/PowerShell process count is
  zero. Automatic Remote uses exact argv `[ssh.exe,-o,BatchMode=yes,-N,-L,8787:127.0.0.1:8787,<validated alias>]`,
  with `BatchMode=yes` and hidden prompt=0. Unregistered or changed host keys are not
  connected; only an explicit CTA may request one OpenSSH-owned interactive.
- An app-wide supervisor owns one bootstrap/tunnel child, reaps it, and performs next-launch auto-reconnect from
  the saved selector. Concurrent tunnel count is one, orphan tunnel count is zero, and same-generation automatic
  infinite retry is zero. Recorder ownership continues after app/tunnel exit.
- Setup is `server/API prepare → listener → readiness health → details → auth-start when needed → separate auth-check → details`.
  Health, details, auth-start, and auth-check are not interchangeable. Authentication operations are control-only;
  their response is never merged into the display root, and Setup/app confirmation is not repeated on
  poll, reconnect, or same-generation rebuild.
- Headless silent REST requires Slint component/window/event-loop generation=0, `DISPLAY`/Wayland/X11 dependency=0,
  Slint visible/hidden HWND=0, and headless snapshot builder + read-only publisher only. This remains PRODUCT_PENDING.

## Native baseline inventory

The baseline is the current Rust/Slint application, not an earlier Windows
binary. Its observable surfaces are:

- Main authenticated surface: account/auth state, plan, quota percentage,
  weekly or monthly period, reset countdown, seven-cell period gauge, model
  token/cost table, status/retry, active-thread summary, graph button, legal
  notice button, and custom move/minimize/close controls.
- Main unauthenticated surface: auth start, validated browser-open action, auth
  confirmation/check, error/retry, and legal notice.
- Graph surface: one instance, current/older reset-period selection, dollar or
  token metric, remaining/SOL/TERRA/LUNA/ASTRA visibility toggles, cumulative lines,
  current labels, unused intervals, and resident-service-backed history from the
  strict details root.
- Threads surface: one instance, empty/single/multiple rows, parent-first
  depth-first subtree-contiguous order, role/depth/orphan guides, model,
  context usage, cumulative tokens, thread age, and instruction age.
- Legal surface: one fixed-viewport, chapter-paged instance containing GPL
  warranty and third-party font/schema/dependency/distribution notices; the
  chapter controls, Back, and Close remain in the same viewport.
- Runtime states: initializing, unauthenticated, authenticated normal, quota
  warning/danger, reset warning, API error, local-history error, thread error,
  transport failure, and stale last-good retention.

## Windows geometry/DPI/non-scroll reference

The following is the requirements reference for the six top-level Windows
surfaces. It must not be inferred from the current Avalonia/Win32 source,
installed images, or a fixture's host dimensions. Evidence is intentionally
pending until the corresponding Windows runtime checks are executed.

All dimensions below are logical client dimensions and exclude the OS frame.
`unbounded` means that the product contract places no maximum on that axis.
Help is an in-Main information surface, not a Window or HWND.

| Surface | Registered top-level surface | Runtime open HWND | Logical client initial | Logical client min | Logical client max | Resize | Native controls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Main | yes | 1 | 900×480 | 900×480 | 900×480 | fixed | Minimize, Close |
| Setup | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | Minimize, Close |
| Settings | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | Minimize, Close |
| Graph | yes | 0..1 (singleton) | 940×640 | 700×480 | unbounded | resizable | Minimize, Maximize/Restore, Close |
| Threads | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | Minimize, Close |
| Legal | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | Minimize, Close |
| Main-internal Help | no (owner=Main) | 0 additional | Main client 900×480 | Main client 900×480 | Main client 900×480 | follows Main | no independent controls |

The registered top-level surface inventory is exactly Main, Setup, Settings,
Graph, Threads, and Legal (six). Runtime open HWND is Main=1 plus an open child
subset of 0..5, total 1..6; each child is a singleton and only all five open
children produce six runtime HWND. Help remains Main-internal with 0 additional
HWND; the inventory count is not a runtime always-open count. `700×480` belongs
only to Graph's minimum. Fixed surfaces require a supported work area of at
least `900×480 logical`; Graph requires at least `700×480 logical`. A smaller
work area is outside the supported matrix and is recorded in the topology
manifest as `unsupported_scope`; it is not passed by shrinking fonts, clipping,
using scroll, or fabricating a PASS value. This boundary does not introduce a
new product failure class or another top-level Window.

Main, Setup, Settings, Threads, and Legal use page/step/detail/chapter controls
to keep all primary information, the primary action, Back, and Close in the
same viewport. Graph keeps period, metric, series, plot, current value, Back,
and Close in the same viewport. The complete X/native collection is preserved;
only the Windows arrival method is split into these semantic units.

Threads keeps its `900×480 logical` client size and shows six complete compact
two-line rows without a vertical scrollbar. Each row is 56 logical pixels high
with a 4-pixel gap. Only the seventh and later rows may use the list's internal
vertical scrollbar; the Window itself never scrolls. A single row is not
expanded, fonts are not reduced to increase capacity, and tree rails never
overlap title or parent text.

For every surface/monitor/DPI/size cell, the supported predicate is the
following AND:

```text
supported = client_threshold AND frame_fit
client_threshold(fixed Main/Setup/Settings/Threads/Legal) = logical >= 900×480
client_threshold(Graph) = logical >= 700×480
frame_fit = DPI変換後のDWM.visible_frame全体が対象MONITORINFO.rcWork内へ完全包含
```

`frame_fit` checks the physical visible-frame left/top/right/bottom against
`rcWork`. The logical client threshold is necessary, but threshold alone is
not supported; the threshold AND frame-fit is sufficient. A frame that does
not fit is `unsupported_scope`, even when the client threshold is met.

DPI reference is the OS-reported integer `dpi` from a
`GetDpiForWindow`-equivalent query, with `scale=dpi/96`. 96/144/192 (100/150/
200%) are mandatory fixtures, not the complete supported domain. Positive
logical width/height converts to physical pixels with
`floor(logical*dpi/96+0.5)` for each dimension. Every geometry record keeps
`logical_client`, `physical_client`, DWM visible `visible_frame`, and
`MONITORINFO.rcWork` `work_area` as separate fields, each with origin, size,
and unit.

For a fresh Main launch, use the monitor of the foreground Window immediately
before launch, falling back to the primary monitor when that Window is invalid.
The first open of a child uses its owner's monitor. Center only fresh Main and
the first user-open. On reopen, restore the last stable OS rectangle only when
its DWM visible frame is fully contained in any current `MONITORINFO.rcWork`.
If monitor removal or resolution shrink makes it invalid, Main falls back
foreground monitor→primary and a child falls back valid owner monitor→primary,
centers once as `topology_recovery`, and records the reason raw. If no monitor
meets the supported predicate, record `unsupported_scope` and do not fabricate
PASS. Using `MONITORINFO.rcWork` and DWM visible frame bounds, compute once in
physical coordinates:

`origin = work_origin + floor((work_size - frame_size) / 2)`

The double-coordinate residual is at most one on each axis:
`abs((2*frame_origin+frame_size)-(2*work_origin+work_size))<=1`. There is no
timer/poll/reopen/drag-after recenter, `Window.Position` loop, or cursor
synthesis; normal timer/poll/reopen/drag recenter is zero, with only the one-time
`topology_recovery` center for invalid reopen as an exception.

The six registered surfaces use one native move per gesture and have zero control hits
converted to move. Every Window has Minimize and Close. Only Graph has native
resize and Maximize/Restore; Graph Maximize uses the current monitor's work
area, and fullscreen is not a product action.

Required topology cells are same-DPI crossing, different-DPI crossing,
negative/nonzero monitor origin, and taskbar-shrunk work area. The target
monitor must meet the surface's supported boundary and frame-fit predicate;
below-boundary or non-fitting work areas remain only in `unsupported_scope`
manifest evidence.

These additions exist to make client/frame/work-area units, DPI transitions,
monitor placement, native drag ownership, and fixed/minimum boundaries
unambiguous. The reason is to prevent current implementation behavior from
becoming an unverified specification and to prevent Graph's minimum from being
applied to fixed surfaces. X/native data values, period boundaries, ordering,
state transitions, ownership, and failure retention are unchanged; only the
Windows viewport/OS expression and input boundary is specified here.

Evidence is planned, not collected: launch a fresh process at every matrix ×
state × supported topology/DPI cell under one artifact SHA; save raw OS DPI,
logical/physical/frame/work-area rectangles, rounding and residual calculations,
registered inventory, runtime HWND subset/singleton, frame-fit, reopen fallback
and reason, native move/resize/control-hit counters, foreground/cursor trace,
and viewport/page/chapter bounds. A reviewer independent of the implementer
recomputes the values. Until that happens, this section cannot make an
implementation or acceptance claim.

## API contract decisions

`GET /v1/details` and `GET /v2/details` remain deprecated read-only compatibility contracts.
`GET /v3/details` is the current generic-model atomic root and is versioned by `api_version: "v3"`.
Together with readiness-only `GET /health` (`/v1/health` compatibility), these routes are the complete public read surface. V1/v2
top-level shape is the exact set `api_version`, `state`, `observed_at`,
`authenticated`, `plan_label`, `quota`, `models`, `active_thread_count`,
`history_periods`, `history_samples`, `history_gaps`, `threads`, and `estimated_cost_label`.
V3 has the same top-level set without the UI-owned `estimated_cost_label`; current and history `models` are
bounded generic-model arrays, and each history row separately carries `models_complete` and `model_source`.
The exact details contract revision is `rest-v1-details-reset-at-20260823`; each history period carries
its canonical `reset_at` independently from its potentially clipped graph `end_at`, and `history_gaps` contains only
confirmed, redacted `recorder-gap-ledger-v1` projections. V2 adds exact per-history-row `model_source`
(`confirmed`, `unavailable`, or `legacy-unknown`) and nullable model values: unavailable requires all six model
values to be null, while the other states require all six to be present. A details `api_version` mismatch rejects the
whole candidate and retains the last complete root; a schema-valid health `product_version` mismatch is diagnostic
only and does not block details retrieval.
The server-side published-generation header contract revision is `rest-v1-published-pair-header-20260827`.
Every successful v1, v2, or v3 details response contains exactly one
`Codex-Info-Published-Pair` header whose value is `v1:` followed by 64 lowercase
hex characters: a 128-bit process server epoch followed by a 128-bit successful-publish
counter. The resident service publishes all representations from one immutable details generation. Windows first
requests one strict `/v3/details` response and falls back to one strict v2, then one strict v1 response only for exact 404.
No second response completes, compares, or repairs an accepted root. After a valid v3 root, its pair may be sent as one
quoted `If-None-Match` on v3 only; matching 304/body zero retains last-good and is not a failure. V1/v2 fallback sends no
conditional header. A missing, duplicate, malformed, or case-altered details header rejects
the complete candidate and retains the last complete root. Health and error responses do not
carry this header. The production UI treats the details header only as that response's opaque generation identity;
it does not derive data meaning from either component.
It contains bounded model cost rows, reset periods, minute history samples,
bounded active-thread rows, and the aggregate cost label. Timestamps are
positive Unix seconds; percentages and dollar values are finite and
non-negative; v3 model names are bounded generic IDs and ASTRA is not grouped as other; all user-visible
labels are one-line bounded Unicode text. Unknown or duplicate keys, malformed
JSON, oversized bodies, and values outside those limits are rejected without
replacing the last valid details snapshot.
After reset-tolerance canonicalization, `(period.id, timestamp)` is also unique.
A collision rejects the complete candidate; Windows must not merge, maximize,
select the last row, null a conflicting remaining value, or render multiple
vertical values at one canonical minute.

Before a public details candidate exists, the resident service `HistoryCanonicalizer` groups only rows with the
same admitted storage scope, validated cycle, and minute. It emits one logical sample only when distinct non-null
quota has cardinality at most one and one existing cumulative vector `(sol_dollars, terra_dollars, luna_dollars,
sol_tokens, terra_tokens, luna_tokens)` uniquely componentwise-dominates every row. Zero non-null quota yields
`null`; quota conflict, incomparable/non-unique/no dominant vector, or unknown scope/cycle/minute/period boundary
rejects the whole candidate and retains last-good. Values shaped as 100%, seven days, or quota-only are not filtered.
Neither REST nor Windows synthesizes component maxima, last-row, null-on-conflict, or arbitrary merges.

The API does not expose account email, filesystem paths, raw backend errors,
session contents, or credentials. Authentication actions remain a separate
explicit command boundary and must never be represented as a successful read
when they are unavailable.

## Requirements

### Installation lifecycle requirements (2026-08-22 clarification)

| ID | Source / contract | Boundaries and failure behavior | Oracle | Status |
| --- | --- | --- | --- | --- |
| WIN-INSTALL-01 | The Windows client must be delivered with a standard GUI installer program. | The artifact is a runnable x64 Windows GUI `CodexInfo.WindowsClient.Setup.exe`, built by the pinned Inno Setup 7.1.0 wizard and containing the self-contained client payload; a shortcut-only helper or hand-written copy bootstrapper is not sufficient. | Installer compiler gate, PE inspection, fresh physical Windows wizard image | verified |
| WIN-INSTALL-02 | Installation and update must add the client in the normal Windows way. | Per-user installation requires no administrator privilege, copies the complete published payload, creates a Start-menu entry, registers an uninstall entry in the per-user Apps registry, and re-running the same AppId performs an update. Failed/cancelled Setup uses the installer engine's transactional rollback and must not publish a partial Start-menu or Apps entry. | Fresh physical Windows install + update lifecycle, shortcut/registry/payload inspection | verified |
| WIN-INSTALL-03 | The installed client must launch from the Start menu. | The shortcut targets the installed executable, uses the installed directory as working directory, and does not store credentials, SSH keys, or editable remote endpoints. | Shortcut target/working-directory inspection and installed-process smoke test | verified |
| WIN-INSTALL-04 | Standard uninstallation must remove the installed functionality. | The generated standard uninstaller removes Start-menu/Desktop shortcuts, Apps registration, installed binaries, installer metadata, and known-empty product directories. It must not delete `%LOCALAPPDATA%\CodexInfo` settings or the Linux-side history DB. | Fresh physical Windows uninstall + reinstall lifecycle and settings sentinel | verified |
| WIN-INSTALL-05 | Windows shell surfaces must show a real product icon. | Setup, installed client, generated uninstaller, Start-menu shortcut, and Apps `DisplayIcon` all resolve to the same multi-resolution `CodexInfo.ico`; an empty/default executable icon is a failure. | ExtractAssociatedIcon hashes, shortcut inspection, Apps registry inspection, fresh wizard image | verified |
| WIN-UPDATE-01 | A version increase merged into `main` must produce the Windows release through the normal GitHub flow. | `Directory.Build.props` supplies the stable `X.Y.Z` value. PRs validate but cannot publish. A `main` merge creates `windows-vX.Y.Z` only when the version increased and every Windows gate passed. Main releases are serialized; only HTTP 404 means absent; the tag is created atomically; exact Setup and manifest are verified on a private draft before publication. Existing tag/Release, unchanged/decreased/invalid versions, network/5xx failure, and partial upload never publish or clobber. | PR and main workflow fixtures, version gate, fail-closed 200/404 status oracle, Release asset inspection | implemented; local decision/manifest/actionlint gates PASS; same-SHA GitHub run pending |
| WIN-UPDATE-02 | Installing an available version starts only from an explicit user action. | Startup/background work may check and notify only. The update action is absent when no newer release exists and appears only in StatusBanner when one exists. Download and ordinary GUI Setup launch begin only after the user presses it; permanent Header update, silent/unattended install, automatic restart, and background mutation are forbidden. Auth recovery CTA has priority and update failure does not modify backend status or last-good data. | View-model tests, network/launcher spies, fresh no-update/update/auth images, installed UIA | local PASS: targeted tests, three installed states, and exact no-argument Windows launcher GUI; same-SHA CI pending |
| WIN-UPDATE-03 | An untrusted or incomplete release must never be launched. | Accept only published non-prerelease exact-repository `windows-vX.Y.Z`; validate exact manifest schema, stable version/tag, installer name/URL origin, positive size and lowercase SHA-256; limit redirects and bytes; remove partial files. Any transport/response/integrity/launch failure keeps the installed version runnable and exposes no raw body/exception. | Core boundary fixtures, coordinator filesystem/launcher tests, tampered installer E2E | local independent PASS: updater Core 25/25 and full Core 120/120; same-SHA CI pending |
| WIN-QUAL-01 | Unit-testable Windows product logic must pass the finite behavior tests for its parser, state, persistence, formatting, and graph contracts. | Coverage is measured for diagnosis across Core, ViewModels, Settings, Localization, Graphing, preview fixtures, and pure window geometry, but an arbitrary percentage is not a product acceptance condition. Generated XAML, bootstrap/composition, window lifecycle, and Avalonia/ScottPlot drawing adapters are confirmed by physical Windows E2E. | Pinned `CodeCoverage.runsettings`, non-zero behavior-test results, reported Cobertura rate, physical Windows E2E | IMPLEMENTED locally; physical Windows E2E remains release evidence |

Graph/historyの外部契約と有限oracleは本表へ複製せず、UX owner文書
`docs/WINDOWS_UX_SPEC.md`の唯一master `CUM-138-06`だけを参照する。

| ID | Source / contract | Boundaries and failure behavior | Dependencies | Oracle | Status |
| --- | --- | --- | --- | --- | --- |
| WIN-PAR-01 | User correction: no requested functionality reduction. Windows must preserve the observable native feature set. | Any omitted native feature is a failure; no silent monitoring-only scope. | Native UI inventory, Windows UI inventory | Feature inventory diff, Core/Presentation tests, `WINDOWS_ACCEPTANCE_E2E_2026-08-22.md`, rendered controls | verified |
| WIN-PAR-02 | Preserve authentication start, browser-open, and authentication-check flow. | Unauthenticated, URL issued, browser failure, and retry states retain safe status text. | Codex app-server bridge, loopback command API | Setup presentation tests, auth-required fresh image, explicit auth command boundary and failure-safe UI contract | verified |
| WIN-PAR-03 | Preserve quota summary and period semantics. | Weekly/monthly, zero/full, warning/reset-boundary, null/error snapshots. | Strict details root | Protocol/presentation fixtures and six final host state images | open |
| WIN-PAR-04 | Preserve model token and expected-dollar presentation. | Input, cached input, output are separate token and dollar columns; unsupported/empty models are omitted. | Local session usage and pricing data | Presentation fixture totals, model view-model tests, source ownership table | verified |
| WIN-PAR-05 | Preserve seven-cell period gauge and countdown. | 0%, intermediate, 100%, zero-day wording, weekly/monthly period. The visible label identifies time until reset; all seven cells use only the specified accent/muted surfaces, with no themed ProgressBar state colors. | Quota reset/window data | Quota boundary tests, installed UI Automation palette sampling for full/partial/empty cells, and fresh 0/10/100% host images | current release pending |
| WIN-PAR-07 | Preserve active-thread details. | Empty, one thread, multiple parent/child/orphan rows, context/token data, stale/error retention. Keep the fixed 900×480 client; show six complete 56px two-line cards with 4px gaps and no scrollbar, then allow list-only vertical scrolling from row seven. A one-row result is not expanded and tree rails do not overlap text. | thread/list and model context data | Thread fixture tests plus fresh 900×480 images for six rows/no scrollbar and seven rows/list scrollbar, with pixel overlap measurement | local independent PASS: installed six/seven-row images; same-SHA CI pending |
| WIN-PAR-08 | Preserve legal-notice view and licensing text. | Accessible while authenticated and unauthenticated; `UX-20260822-UX-002` pre-authored chapter paging exposes the complete text in the fixed viewport, with chapter position, Back, and Close always visible. | Repository license notices, `UX-20260822-UX-002` chapter paging | Legal source/paragraph manifest, page-hash join, and fresh Legal image | unverified |
| WIN-PAR-09 | Preserve locale/timezone behavior. | Supported locales, fallback locale, absolute vs relative time semantics. | Existing localization/time helpers | Localization tests, en/de/unknown fresh images, timezone settings image | verified |
| WIN-PAR-10 | Preserve custom window behavior where applicable. | No accidental data loss on close; child windows are single-instance and cancellable; all six top-level surfaces support one native move gesture plus Minimize/Close, while only Graph supports native resize and Maximize/Restore. | Windows geometry/DPI/non-scroll reference, UX-002, window lifecycle | Fresh process HWND/rect/control trace and child-window captures | unverified |
| WIN-PAR-11 | Preserve strict transport/API safety. | Fixed endpoint policy, size/type/schema validation, no redirects/cookies/proxy/decompression, redacted failures. | REST contract | Existing security tests plus expanded endpoint tests, contract gate | verified |
| WIN-PAR-12 | Preserve information ownership and avoid semantic duplicates. | Countdown appears in the period gauge; main summary owns remaining quota; model table owns token/dollar totals; graph owns historical trends; status owns transport/backend state. | Native DESIGN.md ownership table | DESIGN ownership matrix, static text inventory, and fresh state images | verified |
| WIN-PAR-13 | Preserve persistence and acquisition semantics. | History is minute-bucketed; same admitted scope/cycle/minute is accepted only through the unique-quota plus existing componentwise-dominant-vector rule. Conflict/non-comparability/unknown boundary rejects the candidate, raw SQLite remains non-destructive, and DB/API/UI reload does not fabricate values. | Resident `HistoryCanonicalizer`, native SQLite store, details endpoint | Shared rollover `100% / $1 → 41% / $323.674247`, three-month retention, one-month range/capacity, DB protection and restart/reload tests | open |
| WIN-DES-01 | New request: Windows-native polished design. | Fluent visual hierarchy, icon affordances, keyboard/focus states, exact logical client matrix (fixed surfaces 900×480; Graph 940×640 initial/700×480 minimum/unbounded max), high-DPI layout, no clipped text or accidental empty space; no feature or state is removed. | Windows geometry/DPI/non-scroll reference, common theme and all windows | Fresh normal/minimum/high-DPI/topology images plus OS rect/DPI measurement and Start-menu/keyboard focus smoke | unverified |
| WIN-DES-02 | New request: icons and free libraries permitted. | Every icon asset is redistributable, has a recorded license, and has a text tooltip/accessible name; missing glyphs have a safe fallback. | Assets and third-party notices | Contract gate, embedded notices, Legal image, tooltip/AutomationProperties inventory | verified |
| WIN-I18N-01 | New request: multilingual support. | Language selection covers the native catalog choices, persists locally without credentials, falls back deterministically, and updates every Windows view including status/error/setup/legal copy. | Localization catalog and settings | Catalog tests, persisted settings normalization, en/de/unknown images, all-view bindings | verified |
| WIN-I18N-02 | New request: locale-aware presentation. | Dates, numbers, durations, decimal separators, and direction/line wrapping follow the selected locale without changing protocol values or quota semantics. | Existing validated timestamps and numeric values | Culture/timezone tests, settings image, and locale-aware view models | verified |
| WIN-SET-01 | New request: initial setup flow. | First launch explains the fixed SSH/loopback architecture; profile is one of [none,wsl,sshConfigAlias]; WSL uses an installed distribution exact token, SSH uses a literal Host alias matching `^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$`; setup performs readiness health→strict details→control-only auth-start→separate auth-check→strict details without persisting secrets or raw manual host/user. | Settings persistence, details client, auth command, Windows OpenSSH process | Extraction contract only; direct process/host/fresh image evidence remains PRODUCT_PENDING | unverified |
| WIN-SET-02 | New request: settings/recovery path. | Settings can change language and safe presentation preferences, show connection status/help, retry/recheck/authenticate, open legal notices, and recover from failed setup without losing last-good data. | Main VM lifecycle and settings store | Settings view-model tests, final Settings image, API error/auth fixtures, DB reload evidence | verified |
| WIN-SET-03 | UX correction: do not show the SSH confirmation/setup screen on every launch. | After readiness health/strict details and, when needed, separate auth-check followed by a new strict details root, atomically persist the exact six-key settings object including non-secret `connectionProfile` and `connectionSelector`. Subsequent launches use the saved selector for Main auto-reconnect; SSH raw manual host/user remains one-session only. | ClientSettingsStore, MainWindow startup gate, SetupWindow step transition | Extraction contract only; saved-selector reload and host evidence remain PRODUCT_PENDING | unverified |
| WIN-SET-04 | Regression found: corrupt or partially-written settings must not reopen the welcome wizard on every launch. | Old four-key, invalid JSON, and invalid selector are safe disconnected states, never configured connection; startup remains Main disconnected and Settings owns recovery, with automatic recovery command count zero. | ClientSettingsStore, MainWindow startup gate | malformed/empty/truncated/old4/invalid-selector fixtures; Windows host check remains PRODUCT_PENDING | unverified |
| WIN-ACC-01 | New request: missing perspectives must be addressed. | UI has visible focus, keyboard navigation, accessible names/tooltips, exact logical client/DPI matrix, reduced-motion-safe transitions, and no color-only status meaning. | Common controls, geometry/DPI reference, and all states | Focus styles, AutomationProperties, keyboard traversal smoke, matrix/topology images, text status labels | unverified |
| WIN-ACC-02 | Regression found: every borderless Windows surface must be movable. | Main, Setup, Settings, Graph, Threads, and Legal windows must respond to one native left-button drag on the title region; control hits must not start a move and close/minimize remain clickable. Only Graph exposes native resize/maximize/restore; Help remains Main-internal with no HWND. A source-only handler is insufficient. | Windows geometry/DPI and viewport reference, common title-bar behavior | Fresh host process drag/control/HWND/rect results, contract gate, independent visual review | unverified |
| WIN-ACC-03 | UX correction: the client must not take control of the user's mouse. | Product code never calls cursor-position or synthetic mouse APIs. Physical-input smoke is opt-in via `-AllowPhysicalInput`; default acceptance runs do not move the host cursor and report movement as unverified rather than PASS. | Window drag behavior and test harness | Static API scan, default smoke SKIP output, independent review | implemented |

### Acceptance boundary

Automated tests must report a non-zero executed test count. Windows-only behavior requires a fresh Windows run;
physical cursor input remains opt-in and a skipped physical test is not a PASS for window dragging.

## Dependency DAG

```text
native state/data ownership
  -> versioned loopback API contracts
    -> Windows core clients and validated view models
      -> Main / Graph / Threads / Legal windows
        -> state-by-state rendered verification
```

## Native no-regression gate (2026-08-22)

The native `./run.sh` path is also a release surface. A Windows parity change
must not weaken it, and a native parser/storage change must prove that valid
records survive malformed append-only input.

| ID | Requirement | Acceptance evidence | Status |
| --- | --- | --- | --- |
| REG-01 | Normal `./run.sh` must continue polling account/quota state after startup and after a transient worker failure. | Opt-in bounded runtime trace plus a timer/state regression test; no credentials or paths in logs. | verified |
| REG-02 | Authenticated active root threads and native child threads remain visible; empty, multiple, child, completion, and worker-error states are distinct. | `thread/list`/rollout fixtures, current-process fixture, and fresh `multi-thread`/normal screenshots. | verified |
| REG-03 | Valid local usage snapshots create `HistoryCanonicalizer` minute samples and model/token graph lines; one oversized or malformed tool record must not turn the whole period into idle. The shared rollover fixture must preserve `100% / $1 → 41% / $323.674247`. | Oversized-record collector test, recoverable rollout test, persisted SQLite before/after counts, shared rollover oracle, and fresh graph screenshot. | open |
| REG-04 | Existing valid SQLite samples are never deleted or rewritten as fabricated values during recovery. Invalid rows/records are ignored or isolated; only the documented three-month prune is destructive. | Read-only row-count/hash audit before and after, store reload test, and recovery log with bounded counters. | verified |
| REG-06 | Recovery must not add a permanent full-session scan loop. | Local JSONL collection is quota-cycle/explicit-refresh driven; only the bounded thread RPC poll remains periodic. | verified |
| REG-07 | REST/app-server unavailability must not discard local usage that is already present in append-only session logs. | When the account bridge fails during startup/refresh, one bounded backfill uses the last persisted reset hint; results stay hidden until authentication and are reloaded from SQLite on auth. | verified |
| REG-08 | Multiple clients must not duplicate, regress, or mix history; the resident service remains the sole writer. | SQLite unique `(partition_id,reset_at,timestamp)` key, HistoryCanonicalizer unique-quota/dominant-vector admission, bounded busy timeout, cross-profile/account isolation, duplicate reject, raw-row preservation, and writer-lease test. No transactional max/coalesce repair is allowed. | open |
| REG-09 | The explicitly requested recorder daemon must not become hidden unbounded CPU load. | The daemon uses the same store contract, a singleton lease, bounded interval/change detection, explicit lifecycle, and fail-closed shutdown; when it is stopped, the unavoidable gap is documented rather than fabricated. | verified |
| REG-10 | Destructive maintenance and future migrations must be recoverable. | The DATA owner selects the existing backup/migration direct oracle only when an affected DATA master ID or recorded incident path requires it. The SQLite fixture covers three online-backup generations with `quick_check`/reload/row+file SHA-256 and restore-failure source preservation; another gate does not repeat the same observation. | verified |
| REG-11 | The recorder daemon is auto-started for the configured server path. | A release-manifest-defined launcher/service (exact install/start/stop/restart/uninstall/rollback command remains PRODUCT_PENDING) starts exactly one daemon independently of UI/REST, reports health, and recovers after an unclean exit without deleting or regenerating the DB. | PRODUCT_PENDING |

The gate is fail-closed: any missing trace, stale graph, missing thread, data
count decrease outside the documented prune, or unreviewed fresh image keeps
the change incomplete.
