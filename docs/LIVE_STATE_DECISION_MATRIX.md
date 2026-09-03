# Live state 判定マトリクス（設計正本）

状態: `REQUIREMENTS_AUTHORITY / PRODUCT_PENDING / FRESH_AUDIT_PENDING`

目的は、DBの履歴inventoryを「現在実行中」と誤って表示しないこと。native収集候補の
表示可否は、同一cycle内で得た次のAND条件だけで決める。

```text
candidate schema valid
  AND canonical rollout path is inside sessions root
  AND canonical rollout path ∈ eligible_workload_active_paths(current cycle)
  AND at least one stable eligible ProcessIdentity owns that path
  AND rollout parser last task state == running
  AND owner/root/edge graph is valid
  AND ProfileScopeId/AccountScopeId/StorageEpoch/auth_epoch/AccountUpdateGeneration/CollectorEpoch/CycleSeq admission is current
  AND SupervisorLeaseIdentity owns the current profile publisher
```

どれか一つでも不成立なら、そのcandidateを公開しない。envelope、graph、DB、epochがcycle単位で不確実な場合は部分結果を公開せず、旧完全snapshotを保持して未確認状態を表示する。

## process ownerとcycle境界

- `ProcessIdentity`は`(pid,starttime_ticks,exe_device,exe_inode)`である。`/proc/<pid>/stat`の
  starttimeと`/proc/<pid>/exe`のdevice/inodeをFD列挙の前後で再取得し、完全一致したprocessだけを
  同一ownerとして受理する。PIDだけ、process名だけ、時刻だけでは受理しない。
- eligible workloadは`comm`と実行file名がともに`codex`であり、実ユーザー操作を所有するprocessである。
  exact Codex Info native/server artifactを祖先に持つ、監視・account/read・thread/list専用の
  `codex app-server` childはobserverであり、別のCodex Info processがspawnしたものを含めて除外する。
  VS Code等、Codex Info以外の利用者clientが所有するCodex workloadは除外しない。
- 祖先またはidentityを安全に確定できない、列挙中にidentityが変わる、FD集合の一部しか読めない、
  observer childがreparentされる、上限を超える場合は当該cycle全体をrejectする。部分集合を正常emptyへ
  読み替えない。observer childは生成したCodex Info processが必ずreapし、orphan observerを許さない。
- `eligible_workload_active_paths`は`canonical path -> nonempty ProcessIdentity set`のmapであり、
  pathだけの世代なし集合ではない。process snapshot、FD、rollout、DB graph、RPC envelopeをcycle間で混ぜない。
- account usage admission keyは`(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, AccountUpdateGeneration, CollectorEpoch, CycleSeq)`とする。
  `SupervisorLeaseIdentity`はprofile publisherの単一所有権を別に固定する。
  同じprofile/accountでは現行leaseの単一publisherだけがsnapshotを置換できる。別server/collectorの
  stale lease、旧epoch、同一以下のCycleSeqはcomplete responseであってもno-opで、DB・memory・REST・UIを
  上書きしない。identity値は内部判定専用でUI/RESTへ追加表示しない。
- installed serviceのlive ownerは、同一probe windowのsystemd MainPID、process starttime、executable
  device/inode/SHA-256、profile lock、port 8787 socket inodeと同PID FD、health前後が同じmanifest source
  generationへ結合する場合だけ成立する。PID、listener、health 200、version文字列のいずれか単独を
  `SupervisorLeaseIdentity`またはmanaged ownerへ読み替えない。known旧Codex Info ownerだけを交代でき、
  unknown/foreign/malformed ownerはsignalせず`SAFE_BLOCKED`とする。

## native収集とREST presentationの段階境界

native DB/rollout収集では、owner rootから到達しないrow、edgeのchild row欠落、dangling edge、cycle、
duplicate、schema/query failure、partial sourceをcycle全体のFAIL-CLOSEDとする。これらからorphan表示を
派生しない。

一方、`/v1/details`のtop-level・threads array・全PublicThread rowを完全受理した後のWindows
presentationでは、accepted set内にnon-null `parent_thread_id`が存在しないrowだけをvalid orphanとする。
これはtransport partialやnative danglingの救済ではなく、既に非実行となった親をwire集合へ含めない
完全snapshotの表現である。SUB roleを保持して`親スレッドは現在非実行`を一度表示し、別rootへ接続しない。

## ケース表

| 軸 | 入力 | 判定 | 公開結果 | 固定試験 |
| --- | --- | --- | --- | --- |
| root | active path + running | PASS | root 1件 | `native_live_state_matrix...` |
| root | path不在 + running | REJECT | 0件（正常なempty） | `open_codex_session_paths...`、matrix |
| root | active path + terminal | REJECT | 0件 | `native_completed_rollout...` |
| root | active path + invalid UTF-8/JSON/envelope/known state-event型不正（他に有効rootがあっても同じ） | FAIL-CLOSED | 旧完全snapshot＋未確認。古いrunning状態を再利用しない | `native_descendant_failure...`、`thread_c_candidate_failure_rejects_the_complete_cycle`、rollout parser tests |
| root | EOF直前の未改行tail | HOLD UNTIL NEXT CYCLE | 途中状態を公開せず、次cycleで再読 | `complete_rollout_prefix_len` / append fixture |
| root | 改行済みoversize recordをbounded streamingし、duplicate/unknown envelope key 0の正規tool eventかつliveness非変更と完全証明 | PAYLOAD ISOLATION | event envelopeを受理しoversize payloadだけを保持対象外にする。証明不能ならFAIL-CLOSED | streaming-envelope/large-tool-output tests |
| root | EOF以外の部分行、途中縮小、inode差替え | FAIL-CLOSED | 旧完全snapshot、失敗状態 | secure-open path tests（production-path coverage待ち） |
| root | path不在（running/terminal） | REJECT | 0件（正常なempty） | `native_live_state_matrix...` root matrix |
| root | active path + running/terminal/invalid | 3-way | 1件/0件/FAIL | `native_live_state_matrix...` root matrix |
| child | DB row + edge + active path + running | PASS | rootとchild | `native_live_state_matrix...`、`multiple_running_threads...` |
| child | DB row + edge + path不在 + running | REJECT | rootのみ | `native_stale_running_descendant_not_held_open...` |
| child | DB row + edge + active path + terminal | REJECT | rootのみ | `native_completed_rollout...`、matrix |
| child | DB row + edge + active path + invalid | FAIL-CLOSED | cycle全体を公開しない | `native_descendant_failure...`、matrix |
| DB | edgeのchild row欠落 | FAIL-CLOSED | cycle全体を公開しない | matrix `missing-row` |
| native DB | duplicate/root非到達/cycle/dangling edge/partial | FAIL-CLOSED | native cycle全体を公開せず、orphanへ変換しない | `thread_state` graph/cycle/dangling tests |
| REST presentation | 完全受理済みPublicThread集合でparent IDが集合外 | VALID ORPHAN | SUB role＋`親スレッドは現在非実行`、別root接続0 | D-005/D-006 accepted-set fixture |
| process | eligible workload停止/再起動でowner identityまたはactive path変化 | 再計算 | 同一新cycleの完全snapshotだけ。PID再利用は別identity | identity-before/after、restart tests |
| process | Codex Info由来observer app-serverだけがpathをopen | REJECT | 0件。実利用として公開しない | 2 monitor process/2 observer child fixture |
| concurrent | Codex Info/collector複数、stale lease/epoch/cycle | ADMISSION REJECT | 現行単一publisherだけがatomic置換し、他世代はno-op | lease identity、stale epoch/cycle、concurrent publisher tests |
| installed owner | managed inactive＋healthy known unmanaged | RECONCILE | unmanaged ownerをexact identityで退役し、同generationのmanaged owner 1件だけを受理 | inactive-managed/unmanaged fixture |
| installed owner | healthy foreign/unknown/malformed listenerまたはlock | SAFE_BLOCKED | signal・link・unit・DB mutation 0、成功表示0 | foreign PID/lock/socket identity fixture |
| installed owner | health/version一致、source generation不一致またはprobe前後変化 | REJECT | current成功にせずold/newのverified managed terminalへ収束 | source/hash/TOCTOU fixture |
| transport | RPC timeout/error/invalid envelope、候補の一部読取失敗 | FAIL-CLOSED | 旧thread snapshot保持、未確認表示（部分snapshotは公開しない） | RPC/error tests、`thread_c_snapshot_rejects_partial_candidate_reads` |
| epoch | stale thread/local/account event | NO-OP | 現行世代不変 | `stale_thread_and_local_results_are_complete_no_ops` |
| account identity | logout | HARD BOUNDARY | `auth_required`のstrict empty root。旧DB/Sessionは保持 | `public_snapshot_is_whitelisted_and_tracks_auth_state` |
| account identity | confirmed A→BまたはB→A | HARD BOUNDARY | `initializing`のstrict empty root後、fresh EOF baselineと新account DBだけを公開 | account boundary/A-B partition tests |
| account identity | auth/metadata/partition不一致または世代overflow | FAIL-CLOSED | `error`のstrict empty root。旧account fallback、DB/WAL/SHM mutation 0 | auth/registry/schema/overflow tests |
| session attribution | identity boundary以前の既存file、partial tail、rotation/truncation/prefix不一致 | BASELINE/REJECT | usage/marker 0、sourceと旧checkpointを保持 | incremental baseline/prefix/partial tests |

## 解除（復帰）条件

失敗後に表示を復帰させるのは、次の全条件を満たす新しいcycleだけとする。

1. 新しい現行admission keyでRPC envelope・process identity・FD map・candidate・DB graph・rolloutを再検証する。
2. root/child全candidateのeligible owner identity、active path、terminal stateが同一cycleで一致する。
3. native収集のgraph不正0を確認し、その後の完全受理済みREST集合だけにpresentation orphan規則を適用する。
4. 0件なら正常なempty、1件以上なら完全snapshotとしてatomic publishする。
5. 途中の一部成功、別process snapshot、旧lease/epoch/cycleを前回snapshotへ混ぜない。

この判定を変更する場合は、実workload、observer、PID再利用、複数process再起動を有限のrisk-based
caseで再現し、atomic snapshotと停止済みthread非表示を確認する。実行していないcaseをPASSにしない。
