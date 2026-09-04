# 要求管理台帳

契約意味はPRODUCT_REQUIREMENTSのowner registryとowner文書だけが正本。台帳はID→owner/実装scope/直接oracle/statusの派生索引で、要求/境界を定義しない。owner変更が先。

| ID | owner | 実装範囲 | 直接オラクル | 状態 |
| --- | --- | --- | --- | --- |
| CUM-138-01 | DATA | `src/main.rs`、`src/security.rs`、`src/usage_store.rs` | durable baseline＋2GiB overflow＋verified tail固定caseでbaseline保持とtail 1回加算、256MiB超sourceのinventory採用とcheckpoint tail取得 | implemented |
| CUM-138-02 | DATA | `src/main.rs`、`src/usage_store.rs` | 同じrangeのrepeat/restart、変更prefix、stale generation固定caseで非重複・非mutation | implemented |
| CUM-138-03 | DATA | `src/main.rs`、`src/usage_store.rs` | same-period drift＋checkpoint＋restartとconfirmed rolloverの固定case | implemented |
| CUM-138-04 | PRODUCT | `src/main.rs`、`src/usage_store.rs` | 1 details root内のモデルtoken/価格合計とhistory latestのexact一致、失敗cycleのroot不変 | implemented |
| CUM-138-05 | DATA | product data path、実Linux daemon/API、Linux/Windows UI | 修正前後sentinel digest、selected byte count、同一candidateの実API/UI read-back | implemented |
| CUM-138-06 | UX | `src/main.rs` graph projection、Windows Graphing/ViewModel/Control | shared rollover、first observation、delayed/missing quota、whole-vector回帰/回復、confirmed gap、unattributed quota、current/historical右端、no-historyの9 causal caseを固定fixtureからX/Windowsで独立検証 | implemented |
| CUM-138-07 | DATA | resident scheduler、local collector、recorder、usage store | transport error→local-only pending row/quota NULL、local error→fresh quota＋durable model pending row、stale admission拒否、次周期single-flight、DB exact batch retry、live DB timestamp/generation進行 | implemented |
| AUTH-129 | DATA | `src/account_scope.rs`、`src/protocol_contract.rs`、`src/main.rs` | `account_key_contract_rejects_missing_types_controls_limits_and_symlinks`、`account_authority_rejects_a_same_size_mid_read_replacement`、`account_update_generation_is_local_strict_and_permanently_invalidated`、`hmac_scopes_are_stable_and_account_separated`、`raw_identity_canaries_are_absent_from_partition_paths_metadata_and_candidate_db` | verified |
| GEN-129 | DATA | `src/account_scope.rs`、`src/main.rs`、`src/daemon.rs` | `profile_registry_reuses_epoch_and_separates_physical_paths`、`registry_rejects_orphans_unknown_artifacts_and_missing_initialized_database`、`auth_epoch_overflow_requires_process_recovery_and_cannot_resurrect_usage`、`seven_account_switch_crash_images_restart_without_old_account_fallback` | verified |
| DB-129 | DATA | `src/account_scope.rs`、`src/usage_store.rs`、`src/daemon.rs`、`src/main.rs` | `account_partitions_isolate_same_keys_metadata_backups_and_gap_ledgers`、`wrong_partition_schema_is_rejected_by_a_read_only_probe`、`recorder_writer_persists_only_the_caller_generation_and_holds_the_profile_lock`、`allocated_candidate_and_renamed_final_resume_without_a_stale_writer_lock`、7点crash test | verified |
| SESSION-129 | DATA | `src/main.rs`、`src/usage_store.rs`、`src/daemon.rs` | `account_boundary_baselines_existing_sessions_and_only_commits_verified_appends`、`post_scan_identity_mismatch_discards_rows_cursors_markers_and_publication`、`changed_session_prefix_is_freshly_baselined_without_cursor_or_marker_reuse`、`session_range_checkpoint_marker_and_generation_commit_atomically`、`recorded_overflow_cleanup_deletes_only_fresh_authorized_files`、`session_traversal_budgets_and_symlink_rejection_have_exact_boundaries`、`aggregate_overflow_collects_latest_prefix_and_fatal_selected_file_has_no_fallback` | verified |
| REST-129 | WIRE | `src/main.rs`、`src/server.rs`、`docs/REST_API_V1.md` | `public_snapshot_is_whitelisted_and_tracks_auth_state`、`post_scan_identity_mismatch_discards_rows_cursors_markers_and_publication`、`seven_account_switch_crash_images_restart_without_old_account_fallback`、`stale_thread_and_local_results_are_complete_no_ops`、`server::tests::published_pair_generation_matches_fixed_vectors` | verified |
| LEGACY-129 | DATA | `src/main.rs`、`src/account_scope.rs`、`docs/CUSTOMER_OPERATIONS_RUNBOOK.md` | `legacy_database_backups_and_sessions_remain_byte_identical`、account partition loadによるlegacy projection 0、production `migrate_verified` caller 0件 | verified |
| WIN-PARITY-DATA | PRODUCT | resident SnapshotPublisher、Linux UI、Windows Core/ViewModels/Graphing | shared rollover `100% / $1 → 41% / $323.674247`、`graph_delayed_quota`、同一revisionのX/Windows表示 | implemented |
| WIN-PARITY-WIRE-01 | WIRE | `src/server.rs`、Linux/Windows details parser/domain | exact valid fixture、各1-key欠落/追加/重複、header不正、旧root保持unit | implemented |
| WIN-PARITY-HISTORY-01 | DATA | resident collector/canonicalizer/`src/usage_store.rs`、REST projection、Linux/Windows sink | conflict/comparable/incomparable/共有境界/ambiguous boundary/shadow reset/duplicate固定fixture、`tests/fixtures/graph_weekly_reset_rollover.json`のshared rollover exact値、minute隔離/全体reject/raw row不変 | implemented |
| WIN-PARITY-STATE | PRODUCT | Linux launcher/service、Windows details client、ViewModels、Infrastructure、MainWindow | version一致/欠落/不一致、旧→新service交代、state-machine unit、API integration、実Windows startup/refresh/failure UIA | implemented |
| WIN-PARITY-PAIR-01 | WIRE | `docs/REST_API_V1.md`、Rust SnapshotPublisher、Linux/Windows details parser/state root | 注入epoch固定vector、起動直後counter1、data counter2、reject/read不変、details-only atomic commit、status route不存在unit | implemented |
| WIN-PARITY-RETRY-01 | UX | Windows connection supervisor、Main state owner | generation ID、child起動/reap回数、stale callback廃棄を数えるfinite state test | verified |
| WIN-PARITY-UX | UX | `windows-client/src/CodexInfo.WindowsClient/*.axaml*`、window/input/accessibility adapter、Presentation tests、E2E | UIA geometry/pixel/keyboard checks、全登録surfaceのfresh Windows capture、独立意味棚卸し | verified |
| WIN-PARITY-CTA-01 | UX | Windows Main/Setup view-modelとAXAML | state×CTA cardinality、control後details未更新のlast-good、fresh Windows capture | implemented |
| WIN-PARITY-LEGAL-01 | UX | Windows legal catalog/plain-text projection/window/view-model、Presentation tests、E2E | 固定Markdown→plain oracle、全埋込み`.md`の禁止記法scan、全page到達unit、900×480 UIA/text/capture意味棚卸し | verified |
| WIN-PARITY-OPS | PRODUCT | `windows-client/src/*`、installer/update tools、Windows tests/E2E | process argv test、settings persistence、installer/update transaction tests、実Windows導入/復旧証拠 | verified |
| WIN-PARITY-RECOVERY-01 | SECURITY | Windows Setup/connection/settings/process adapter | persistence/file/log scan、process argv test、再起動後非再利用test | verified |
| X-START-01 | UX | Linux details client、`ui/app.slint`、`src/main.rs::sync_ui` | native startup details state test、X11 fresh startup visual gate | implemented |
| X-START-02 | UX | Linux details client、`ui/app.slint` | details boundary unit、X11 startup visual gate | implemented |
| X-START-03 | UX | Linux details client、`src/main.rs::native_startup_loading` | `native_startup_failure_releases_loading_surface`、失敗→回復details test | implemented |
| X-START-04 | UX | `src/main.rs`、`run.sh`、`scripts/x11_startup_visual_gate.sh` | visible position test、X11可視範囲ゲート、実画面キャプチャ | verified |
| X-START-05 | UX | `src/main.rs`、`run.sh` | verified-local＋失敗portでの実起動保持と、unsafe-generation/foreign-ownerでUI process 0の有限2-path test | implemented |
| X-START-06 | UX | `scripts/x11_startup_visual_gate.sh`、`scripts/x11_service_recovery_visual_gate.sh` | 実resident service + isolated app-server fixtureのX11画像で、ready利用枠バー→停止/error保持→同一port復旧/readyを状態別にpixel判定 | implemented |
| X-THREAD-01 | UX | `src/thread_contract.rs`、`src/main.rs` | `active_thread_adapter_rejects_partial_rollout_fallback`、`multiple_running_threads_are_all_published_with_stable_order`、`recoverable_rollout_parser_skips_malformed_non_state_records_only`、実Codex active-path取得 | implemented |
| WIN-VERSION-01 | UX | `Cargo.toml`、`Cargo.lock`、`windows-client/Directory.Build.props`、`windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml`、`windows-client/src/CodexInfo.WindowsClient/ViewModels/MainWindowViewModel.cs` | Windows contract gate、実Windows UI Automation | verified |
| PROC-LAUNCH-01 | PRODUCT | `src/main.rs`、service、record/CLI E2E、REST/data docs | 全受理形・port境界unit test、direct payload mode別実行 | implemented |
| PROC-STOP-01 | PRODUCT | `Cargo.toml`、`Cargo.lock`、`src/main.rs`、`src/daemon.rs`、`scripts/cli_contract_e2e.sh`、`docs/CUSTOMER_OPERATIONS_RUNBOOK.md` | isolated HOMEでstart→health→stop→lock/listener消失、停止済み冪等test | verified |
| PROC-OPTIONS-01 | PRODUCT | `src/main.rs`、`src/i18n.rs`、payload E2E | finite accept/reject matrix、helpとparser集合一致 | verified |
| PROC-HELP-01 | PRODUCT | `src/i18n.rs`、`src/main.rs`、`run.sh`、`docs/CUSTOMER_OPERATIONS_RUNBOOK.md` | 全対応言語のcatalog test、help mode unit test、`LC_ALL`を切り替えた`run.sh --help`実行ログ | verified |
| PROC-I18N-01 | I18N | `src/i18n.rs`、`src/main.rs`、`run.sh`、product docs | 全対応言語help、`LC_ALL`切替launcher/payload実行 | verified |
| WF-BINARY-IMPACT-01 | PRODUCT | `AGENTS.md`、`scripts/ci_change_scope.py`、`scripts/test_ci_change_scope.py`、`.github/workflows/*` | tracked全path、実Git copy、add/delete/rename/unknown/空差分、actionlint | implemented |
| WF-FEAT-SELECTIVE-01 | PRODUCT | `.github/workflows/feat-integration.yml`、`.github/workflows/selective-quality.yml`、分類器 | authority-only、history-graph、workflow-selection、profile欠落/重複/未知/所有外path、rename/copy、release非縮小の8 causal caseと実PR job graph | implemented |
| WF-QUALITY-ONCE-01 | PRODUCT | `.github/workflows/*`、`scripts/selected_quality_gate.py`、`scripts/test_selected_quality_gate.py`、`scripts/workflow_quality_gate.py`、`scripts/test_codeql_workflow.py`、`scripts/windows_client_contract_gate.sh` | 選択・非選択・失敗・取消・欠落の有限因果例、PR側oracle改変拒否、draft/stale cancel、closed observer、H0→H1 observer→H2、手動version | implemented |
| WF-NONBLOCKING-QUALITY-01 | PRODUCT | `.github/workflows/version-prepare.yml`、`.github/workflows/feat-integration.yml`、repository rules | actionlint、local acceptance成功/異常、custom check mutation 0、remote ruleset/protection/check-runs | implemented |
| WF-POSTMERGE-01 | PRODUCT | `.github/workflows/release.yml`、`.github/workflows/windows-client.yml` | 両event順序、draft/stale/generated observer、same-head failure→success、Windows skip/success、candidate欠落/期限切れ/複数、実artifact transfer | implemented |
| VER-AUTO-PATCH-01 | PRODUCT | `.github/workflows/version-prepare.yml`、`.github/workflows/windows-client.yml`、`scripts/ci_change_scope.py`、`scripts/product_version.py`、`Cargo.toml`、root `Cargo.lock`、`windows-client/Directory.Build.props` | patch境界fixture、PR #30 no-binary-impact、version mutation 0、byte不変検査 | implemented |
| VER-SERIES-FIXED-01 | PRODUCT | `.github/workflows/version-prepare.yml`、`.github/workflows/windows-client.yml`、`scripts/product_version.py`、`scripts/test_product_version.py` | major/minor不変fixture、通常PRの無断series変更fail-closed検査 | verified |
| WF-SERIAL-01 | PRODUCT | `.github/workflows/version-prepare.yml`、`.github/workflows/release.yml` | stale head、push拒否、同tag二信号、別tag並行、orphan/Draft/partial/mismatch | implemented |
| SEC-CI-TRUST-01 | SECURITY | version・release workflow、local trust gate | actionlint、untrusted checkout検査、non-fast-forward fixture、custom check mutation不存在 | implemented |
| SEC-CODEQL-LOG-01 | SECURITY | `src/protocol_contract.rs`のCodeQL指摘経路 | alerts #2-#5 dataflow、対象unit、CodeQL再解析 | verified |
| SEC-RELEASE-CODEQL-01 | SECURITY | `.github/workflows/codeql.yml`、`.github/workflows/selective-quality.yml`、`scripts/test_codeql_workflow.py`、`scripts/selected_quality_gate.py`、`scripts/workflow_quality_gate.py` | 変更された解析対象source→actions/csharp/python/rustの有限代表例、selected/nonselected結果mutation、PR #67/#68相当、実CodeQL check | implemented |
| LINUX-BUNDLE-TARGET-01 | PRODUCT | `scripts/build_linux_bundle.sh`、`scripts/test_linux_bundle.sh`、`.github/workflows/linux-distribution.yml`、`docs/PRODUCT_REQUIREMENTS.md` | targetとglibc minimumの固定manifest、欠落・不一致のfail-closed fixture | implemented |
| LINUX-BUNDLE-RELEASE-01 | PRODUCT | `.github/workflows/version-prepare.yml`、`scripts/ci_change_scope.py`、`.github/workflows/linux-distribution.yml`、`.github/workflows/selective-quality.yml`、`.github/workflows/release.yml`、`scripts/workflow_quality_gate.py`、`tests/release_linux_rollout.rs`、`docs/PRODUCT_REQUIREMENTS.md` | candidate identityと既存Release asset集合のlocal resolver/publisher fixture、Linux-only差分のWindows追加選択、product-impact runのLinux authority欠落fixture | implemented |
| LINUX-BUNDLE-CONTENTS-01 | PRODUCT | bundle build/test、installer、unit | isolated archive inventory、manifest/member identity fixture | implemented |
| LINUX-BUNDLE-OPERATIONS-01 | PRODUCT | launcher、installer、README/runbook/wiki | bundle-only installと全launcher commandの有限test | implemented |
| LINUX-BUNDLE-AUTOUPDATE-01 | PRODUCT | installer、service/timer、launcher、運用文書 | isolated HOME unit inventory、timer値、全trigger同一resolver test | implemented |
| LINUX-BUNDLE-AUTOUPDATE-02 | PRODUCT | resolver、installer、security/product spec | stable/equal/older/no-candidate/100-entry/invalid matrix | implemented |
| LINUX-BUNDLE-AUTOUPDATE-03 | PRODUCT | Linux update resolver、`packaging/install_linux_bundle.sh`、`packaging/codex-info.service`、`packaging/codex-info-update.service`、`docs/PRODUCT_REQUIREMENTS.md`、`SECURITY.md` | checksum/manifest/archive content tamper、service/health/details failure fixtureのold binary/unit/profile sentinel、single atomic switch/rollback count、concurrent operation rejection | implemented |
| LINUX-BUNDLE-RETENTION-01 | DATA | `packaging/install_linux_bundle.sh`、`packaging/codex-info.service`、`packaging/codex-info-update.service`、`packaging/codex-info-update.timer`、`docs/CUSTOMER_OPERATIONS_RUNBOOK.md`、`README.md`、`wiki/導入と起動ガイド.md` | isolated HOMEのbinary/installer/manifest/profile sentinelとdaemon/update unit lifecycle、rollback retention fixture | implemented |
| U128-01 | PRODUCT | launcher、installer、units | live/fixture stable convergenceとfull tuple read-back | verified |
| U128-02 | PRODUCT | launcher、daemon identity | inactive-managed＋unmanaged/foreign listener matrix | verified |
| U128-03 | PRODUCT | launcher、service ExecStartPre | 各入口のold/equal/new generation fixture | verified |
| U128-05 | PRODUCT | resolver、installer | phase別fault injection、details error rejection、sentinel byte/hash | implemented |
| U128-06 | PRODUCT | bundle、installer、status/health resolver | 各identity要素の差替えとfunctional readiness rejection | implemented |
| U128-07 | PRODUCT | installer、daemon/store | lock競合、wait-for graph、caller scan | verified |
| U128-09 | PRODUCT | transaction/status/control | running A/Bとstopped/disabled/removedのterminal state truth table | implemented |
| U128-10 | PRODUCT | convergence resolver | prestate×trigger有限表 | verified |
| U128-12 | PRODUCT | launcher、server/daemon | tuple各要素のTOCTOU・mismatchとdetails error rejection test | implemented |
| U128-13 | PRODUCT | launcher、convergence test | old healthy→new delayed/fail regression | verified |
| U128-14 | PRODUCT | daemon、service、launcher | concurrent activation/lease E2E | verified |
| U128-15 | DATA | daemon、usage store | recorder-state exact schema/mode/order test | verified |
| U128-16 | PRODUCT | daemon、service | worker fault、`RestartSec=5s`、`StartLimitIntervalSec=0` unit contract | implemented |
| U128-17 | PRODUCT | installer、launcher | phase別rollback terminal oracle | verified |
| U128-18 | DATA | daemon、usage store | outage/backfill/zero-new-row fixtures | verified |
| U128-19 | DATA | usage store、server | gap transition/idempotence/projection tests | verified |
| U128-21 | PRODUCT | launcher、daemon/server | desired×actual state matrix | verified |
| U128-25 | PRODUCT | installer | phaseごとのcrash image recovery | verified |
| U128-26 | PRODUCT | installer、launcher、daemon | path/mode/symlink matrix | verified |
| U128-27 | PRODUCT | launcher、installer | fallback canary/path scan | verified |
| U128-28 | PRODUCT | `run.sh`、`src/i18n.rs`、`src/main.rs`、`scripts/test_run_launcher_version_sync.sh`、`scripts/cli_contract_e2e.sh` | accept/reject argv matrix | verified |
| U128-30 | PRODUCT | convergence test | reverse launcher/version matrixとpredecessor managed/unmanaged recovery | verified |
| U128-33 | PRODUCT | installer、launcher/UI launch | rollback identity/attempt counter test | verified |
| U128-35 | PRODUCT | launcher、installer | exact constant/timeout boundary tests | verified |
| U128-36 | PRODUCT | `run.sh`、`packaging/install_linux_bundle.sh`、`packaging/codex-info.service`、`packaging/codex-info-update.service`、`packaging/codex-info-update.timer`、`scripts/build_linux_bundle.sh`、`scripts/test_linux_bundle.sh`、`scripts/test_run_launcher_version_sync.sh`、`scripts/install_systemd_recorder.sh`、`scripts/record_daemon_e2e.sh`、`scripts/cli_contract_e2e.sh`、`scripts/test_linux_update_convergence.sh`、`src/main.rs`、`src/daemon.rs`、`src/usage_store.rs`、`src/server.rs`、`src/account_scope.rs`、`src/security.rs`、`tests/db_protection_runtime.rs`、`tests/release_linux_rollout.rs` | schema/domain/unknown-key matrix | verified |
