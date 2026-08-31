<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Codex Info

Codex Info reads rate limits, reset periods, local usage history, and running
threads from the Codex App Server and displays them in a Rust/Slint X11 window.
This file is the source/developer quick start. Customer installation uses the
Linux release bundle; see [the customer operations runbook](docs/CUSTOMER_OPERATIONS_RUNBOOK.md)
for the download, checksum, install, health, and removal flow.

## Quick start

```bash
git clone https://github.com/salty919/codex_info_v2.git
cd codex_info_v2
./run.sh --ui
```

The host needs Rust/Cargo (the launcher also checks the standard Rustup
toolchain location), an X11 display (WSLg is supported), and a `codex`
CLI that can run `codex app-server --stdio`. Authentication remains owned by
the Codex CLI; this application does not save passwords, API keys, or tokens.

Without arguments, `./run.sh` starts only the resident daemon and loopback
REST service on `127.0.0.1:8787`. Use `--port PORT` to change only the port,
`--ui` (optionally followed by `--port PORT`) to add the X11 UI, `--stop` to
stop this profile's verified resident daemon, and `--help` for localized help.

If Rustup is not installed, install it and load its environment before running
the launcher:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## Localization and time zones

Fixed UI copy selects the first non-empty value in `LC_ALL`, `LC_MESSAGES`,
and `LANG`. Japanese, English, Simplified Chinese, Korean, Spanish, French,
German, Portuguese, Italian, and Russian are included; `C`, `POSIX`, and
unsupported locales use English. The selected language and IANA time zone are
pinned at process startup. Absolute timestamps use that time zone, while
elapsed and remaining durations use UTC seconds. `TZ` accepts an IANA ID and
uses UTC for invalid values.

Japanese and Korean fonts are embedded from `assets/`. See
[LOCALIZATION.md](docs/LOCALIZATION.md) for the locale, time-zone, and font
specification.

## Usage display

Running threads show context usage and its limit as `usage% / limit tokens`,
derived from the cumulative token count and `model_context_window`.
The graph offers individual visibility controls for remaining quota and
LUNA/TERRA/SOL; all series start visible, and hidden series use muted color and
labels. Intervals where no model's cumulative usage changes appear as faint
background bands, and right-edge values use series-colored leader lines to the
corresponding endpoints. The registered top-level surface inventory is exactly
Main, Setup, Settings, Graph, Threads, and Legal (six); Help is Main-internal
with 0 additional HWND. Runtime open HWND is Main=1 plus an open child subset
of 0..5, total 1..6, with singleton children; all five children produce six
only when open together. Main, Setup, Settings, Threads, and Legal use fixed
logical client `initial=min=max=900x480`; Graph uses `initial=940x640`,
`min=700x480`, `max=unbounded`, and is resizable. All six registered surfaces
provide Minimize/Close, while only Graph provides native resize and
Maximize/Restore. Native title bars are disabled; each surface provides its
own embedded-font title area, and any non-button surface can be dragged to move
it. Graph Maximize/Restore uses the current monitor work area.

## Licensing

Original source and documentation are GPL-3.0-only. The authoritative GPLv3
text is [LICENSE](LICENSE), and [LICENSE.ja.md](LICENSE.ja.md) provides a
Japanese guide. Noto fonts, generated protocol schemas, Slint, Cargo
dependencies, and the Windows client's Avalonia/NuGet dependencies retain
their upstream licenses. Source and binary distributions include
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md),
[assets/NOTICE.txt](assets/NOTICE.txt), and [LICENSES/](LICENSES/). A Windows
publish must also run the notice collection procedure in
[WINDOWS_CLIENT.md](docs/WINDOWS_CLIENT.md).
The Windows distribution is installed with the generated
`CodexInfo.WindowsClient.Setup.exe`; it creates the Start-menu and per-user
uninstall registration, and the uninstaller removes the client without
deleting server history.
