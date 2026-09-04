<!-- codex-info-requirement-owner: DATA -->
<!-- codex-info-master-ids:
CUM-138-01
CUM-138-02
CUM-138-03
CUM-138-05
CUM-138-07
AUTH-129
GEN-129
DB-129
SESSION-129
LEGACY-129
WIN-PARITY-HISTORY-01
LINUX-BUNDLE-RETENTION-01
U128-15
U128-18
U128-19
-->

# Codex Info データ保護規約

この文書は、利用履歴・ローカルセッションログ・thread情報・SQLiteデータベースを変更する全実装の正本である。`DESIGN.md`の補足ではなく、変更を許可するための拘束条件として扱う。

## 1. 目的と適用範囲

対象は次のデータフローである。

```text
Codex app-server / session JSONL / thread rollout
  -> bounded validation
  -> immutable cycle snapshot
  -> transactional SQLite upsert
  -> reload/graph/REST/Windows presentation
```

対象ファイルを変更する場合は、実装、関連仕様、再現可能な自動テストを同じ変更で更新する。

対象ファイル（直接変更・間接変更を含む）:

- `src/main.rs`
- `src/thread_contract.rs`
- `src/usage_store.rs`
- `src/server.rs`
- `DESIGN.md`
- `docs/REST_API_V1.md`
- `docs/WINDOWS_CLIENT.md`
- `windows-client/`
- `run.sh`

## 2. 絶対不変条件

以下を満たせない変更は不合格とし、fallback値・ゼロ値・空DBで通過させない。

1. 既存の有効なDB行を、収集失敗・認証失敗・通信切断・UI終了・migration失敗で削除、上書き、推測変換しない。
2. canonical DBは`(ProfileScopeId, AccountScopeId, StorageEpoch)`ごとに物理fileを分け、DB内のusage rowは`(reset_at, timestamp)`で一意である。必須`storage_partition` singletonの`partition_id`は
   その3値に結合し、`timestamp`は有効なUTC event秒を
   `floor(event_epoch / 60) * 60`へ変換したminute-startであり、同一キーの再計測は行を増やさず、
   残量はcanonical順序の最後の有効値、累積cost/tokenは列ごとの最大既知値を保持する。元event秒は
   同一minuteのcanonical順序を決めるためだけに使い、REST/DBのtimestampへ書き戻さない。
3. DB書き込みはtransaction内だけで行う。busy、I/O、full、corrupt、schema不一致、migration中断はrollbackし、旧DBと旧メモリ世代を保持する。
4. 有効な完全snapshotだけを公開する。account usageのcommit/publish admissionは現行の
   `(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, AccountUpdateGeneration, CollectorEpoch, CycleSeq)` tupleだけを正本とし、candidateの7要素が全て現行値と一致する場合だけDB、memory、REST、UIへ進める。`SupervisorLeaseIdentity`は同一profile serviceの単一publisher所有権を別に固定し、account generationの代用にしない。
   stale lease/epoch/cycleまたはtuple欠落・不一致はcandidateを破棄し、DB、memory、REST、UIを0変更とする。部分的な履歴、thread、model usage、REST応答を成功値として公開しない。
5. local usage JSONLとlive rolloutを同じrecord隔離規則へ丸めない。live rolloutではUTF-8、JSON、
   envelope、event kind、task-stateへの非影響を完全検証できない改行済みrecordを含むcycleはfail-closedにする。
   oversize recordを隔離できるのは、bounded streaming parserがduplicate/unknown envelope key 0の正規eventを
   完全検証し、livenessを変更しないtool payloadであると証明した場合だけである。証明不能、known state eventの
   型不正、invalid UTF-8/JSONは旧完全thread snapshot＋未確認を保持し、古い`task_started`をrunningへ流用しない。
   live rolloutのEOF直前の未改行tail（同一inodeへの追記待ち）だけは次cycleへ保留し、途中状態を公開しない。
   local usage側の改行済み不正record隔離は、後続のvalidated cumulative snapshotで対象列の欠落を覆えるcaseだけを
   許可し、後続snapshotなし・usage eventか判定不能・EOF以外の部分行・I/O・file差替え・資源上限はfile/candidate
   単位でrollbackする。local readerがEOFで検出した未完了recordはvalid/invalid UTF-8・oversizeを問わずrollbackする。
6. app-server停止中でも、Codex Infoプロセスが動作し、`history/usage_reset_hint.json`とcanonical sessions root配下のappend-onlyログが存在する場合だけ、
   outage epochにつき1回のbounded one-shot backfillを許可する。hintはschema `reset-hint-v1`、UTF-8 JSON、最大4KiBとし、
   `state`（`active`/`expired`/`tombstoned`）、`reset_at`、`window_seconds`、`observed_at`、file cursor、
   個人識別値を含まないopaqueな`auth_epoch_nonce`を保持する。
   現在の認証epoch・nonceに束縛されたauthenticated hintだけを受理し、未認証中の復旧値は公開しない。
7. 同一障害中にlocal JSONL全走査やapp-server再起動を無限反復しない。通常の全走査はquota cycle、明示更新、または一度だけの障害復旧に限定する。
   fingerprint不変ならscan/writeは0、変化時も1 cycleにつき1 scan・1 transactionだけとする。
8. collector全停止中の未取得データは後から捏造しない。installed profileではuser-systemdの
   `codex-info.service`だけが常駐daemon、loopback REST、`UsageStore`、singleton recorder leaseを所有する。
   顧客操作は`$HOME/.local/bin/codex-info`がmanaged serviceへ収束させ、`--ui`のpayloadを含む別processを
   writer/REST ownerにしない。service/development/E2Eが直接使うpayloadの引数なし/`--port PORT`は同一processで
   daemon+RESTを所有するが、installed launcherの更新・制御authorityではない。stop/disable/removeはDB、backup、
   hint、gap/recorder/control state、source JSONL、installed generationを保持する。
9. canonical DB profileごとに`MaintenanceOwner`を1つだけ許可する。起動時pruneの前に、writer admissionを止めた同一排他境界で
   SQLite online backupを3世代作成・検証する。backup失敗時はpruneを実行しない。検証失敗・writer競合時もpruneを実行しない。バックアップは`0600`、DBディレクトリは`0700`とする。
10. 現行版は旧schemaを暗黙migrationしない。schema mismatchは拒否する（read/writeを拒否する）。将来migrationは別名DB、全行validate、件数/hash/期間境界比較、
   3世代backup保持、検証後のatomic switchの順序だけを許可する。candidate失敗時はDB、backup、memory、公開rootを旧世代のまま保持する。
11. スレッドのライブ状態はDB履歴を根拠に再生しない。root/childとも、同一cycleで前後identityを検証した
    eligible Codex workloadの`canonical path -> nonempty ProcessIdentity set`にrolloutが存在し、最後のtask状態が
    runningである場合だけnative candidateにできる。`ProcessIdentity=(pid,starttime_ticks,exe_device,exe_inode)`とし、
    exact Codex Info artifactを祖先に持つ観測用app-server childは全Codex Info process分を除外する。
    identity変化、祖先不明、FD scan部分失敗、native DBのduplicate/root非到達/cycle/dangling/partialはcycle全体を
    fail-closedにし、旧完全snapshot＋未確認を保持する。完全受理済みREST PublicThread集合内のmissing parentだけは
    presentation orphanでありnative DB danglingの救済ではない。`docs/LIVE_STATE_DECISION_MATRIX.md`は
    このDATA契約から導出した非規範的な判定表とする。
12. Windows clientの設定永続化は`language`、`setupCompleted`、`connectionConfigured`、`timeZoneId`、
    `connectionProfile`、`connectionSelector`の6 keyだけを許可する。`connectionProfile`は`none|wsl|sshConfigAlias`のexact enum、
    `connectionSelector`はWSLのexact distribution tokenまたはliteral OpenSSH Host alias grammarだけを許可し、秘密、展開済み値、raw host/user/pathを0件とする。
    saved selectorによるauto reconnectはこの6 keyを再検証して行い、remote自動起動は`ArgumentList`と`BatchMode=yes`を使う。
    auth argvもsaved profileから構築するが、起動成功とstatus再確認を別stateに分ける。4-key recoveryはMain disconnectedとSettingsだけに残し、
    設定不正・接続失敗時は保存値とDBを破壊せずSettings recoveryへ戻す。この設定経路の製品判定は`PRODUCT_PENDING`であり、RESTへselector/secretを送らない。

## 3. 失敗時の保持契約

| 障害 | 保持するもの | 破棄するもの | 再試行 |
| --- | --- | --- | --- |
| app-server/REST停止 | 既存のquota、履歴、thread、DB | 未取得の新規値 | 次の明示/周期要求。local backfillは障害期間1回 |
| daemon/recorder unexpected exit / restart budget超過 | 直前の完全snapshot、DB、hint、source cursor、gap ledger、last committed recorder state | 停止区間の推測quota、未検証の再起動candidate | worker死亡は2秒以内に検知し次cycleまでに1回だけ再試行。process exitはsystemdが5秒後に1回だけ再起動し、2回目はFailed latch。明示launcher/update activationでreset |
| oversized/不正な1レコード | 同一ファイル内の前後の有効レコード | そのレコード | 次のcycleで再読込可能 |
| ローカル履歴のEOF未完了レコード | 直前の完全snapshot、DB | 不完全レコードを含むローカル入力 | 次のcycle |
| I/O、差替え、EOF以外の部分行、資源上限 | 直前の完全snapshot、DB | 失敗cycleの部分結果 | 次のcycle |
| SQLite busy | 旧DB、旧backup、旧メモリ/root | 2.000秒でrollbackした未commit transaction | same-callback/polling retry 0。次の通常scheduled cycleまたは明示操作で最大1 attempt、同じ2.000秒deadline |
| SQLite full/corrupt | 旧DB、旧backup、旧メモリ/root | 未commit transaction/candidate | same-callback retry 0。容量・明示restore等の原因解消後だけ新cycle |
| SQLite open/read/write I/O | 旧DB、旧backup、旧memory/root | 失敗batch、partial candidate | 同一callbackでは再試行せず次cycleまたは明示操作 |
| schema mismatch/migration失敗 | 旧DB、旧backup世代 | 新DB候補 | migrationを修正して別途再実行 |
| backup/rotation/restore失敗 | 現行DB、検証済み3世代、旧memory/root | partial backup/candidate、未検証世代 | 次のmaintenanceまたは明示restore。自動復元0 |
| confirmed daemon stop gap | 前後のvalid sample、gap ledger、旧完全root | gap区間の補間値・複製値 | source cursor確認後にのみ確定。確定後は次の実sampleで再開 |
| status/details pair不一致・stale admission tuple | 直前の完全status/details pair、現行`(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, AccountUpdateGeneration, CollectorEpoch, CycleSeq)`とprofile publisher lease | 片側だけ進んだcandidate、stale account/lease/epoch/cycle candidate | 次cycleで現行tupleを再取得・検証し、DB/memory/REST/UIは不一致中0変更 |
| 認証喪失・アカウント切替 | 非認証root（旧account表示は消去）と非破壊DB | 旧accountの画面公開値 | 認証成功後に再読込 |
| reset hint expired / auth epoch切替 | source log、旧DB、旧root、tombstone hint | `now >= reset_at`の旧期間scan/row、次期間への誤帰属、旧epochの公開値 | 旧hintをexpiredまたはtombstonedとして無効化し、source logを保持。current epochのfresh authenticated hint後だけ次のbounded one-shot |
| UI/REST publish失敗 | DBのcommit済み世代、旧表示snapshot | 失敗した公開試行 | 次のroot更新または明示操作 |
| thread DB履歴とlive pathの不一致 | 旧完全thread snapshot、quota、履歴、DB | DBだけから復元した停止済みroot/child | 次のcycleでactive pathとterminal状態を再検証 |

## 4. 同時実行とバックアップ

- すべてのcollectorは`UsageStore`を通してSQLiteへ書く。直接JSON、直接SQL、別形式の履歴DBは禁止する。
- SQLite transaction lockとbounded busy timeoutを正本とする。ロックを無視した上書き、DB削除、DB再生成は禁止する。
- `usage_history.sqlite3.bak.1`〜`.bak.3`は時系列の完全SQLite snapshotであり、同じ件数である必要はない。各世代は`PRAGMA quick_check`と再読込で検証する。
- backup、prune、migrationの失敗は元DBを変更しない。pruneはbackup成功後だけ許可する。
- migrationは`UsageStore::migrate_verified`を入口とし、候補DBを別名で作成して全行の型・値・一意キー、`quick_check`、row count、決定的fingerprint、reset-period境界を検証する。検証後だけ元DBを退避してcandidateをatomic switchし、旧DBと3世代backupを残す。candidate検証失敗・switch失敗・lock競合は元DBをそのまま保持する。
- backup世代の復元は、対象プロセスを停止し、現在DBを別名退避してから、quick check・schema check・row/hash監査を通した世代だけで行う。通常起動が自動復元を試みてはならない。

### 4.1 RecorderSupervisor、lease、backfill、gap

- worker supervisorの状態は`Absent → Starting → Running → StopRequested → Stopped`または`Failed`だけを進む。
  worker死亡は1秒probeで2秒以内に検知する。最初の一時障害は同callbackでretryせず`degraded`へ進み、次の
  scheduled cycle（60秒以内）で1回だけ再試行する。2回目またはfatal errorはprocessを非0終了させる。
  user-systemdは`Restart=always`、`RestartSec=5s`、`StartLimitIntervalSec=60s`、`StartLimitBurst=2`で
  1回だけprocessを自動restartし、次の失敗でFailedへlatchする。明示launcher/update activationだけがlimitをresetする。
- `codex-info.service`がinstalledならsystemdがdaemon+RESTのsupervisorである。service processは
  verified current generationの`codex_info --port 8787`でrecorder leaseとREST listenerを同時に所有する。
  serviceの`ExecStartPre`はpersistent installerのstartup reconcileをbounded実行する。active transaction journalが
  switch後phaseならread-only検証して継続し、journalなしならL1 install lock下のshared resolverを使用するが、
  startup modeは自分自身をrestartしない。launcher `--ui`はmanaged ownerへ収束後に別UI processを追加するだけである。
- singletonのscopeは正規化canonical DB pathとprofileの組である。lease schemaは最大4KiBのUTF-8 JSON
  `recorder-lease-v1`（`pid`、`process_start`、`owner_nonce`、`canonical_db_path`、`device_or_volume_serial`、`file_index_or_inode`）とし、
  writer processは同じ`UsageStore`のtransaction/upsert契約を使う。通常のrecorder二重起動はlease前にno-op、競合試験だけが別の許可済みwriter processを使い、
  製品経路でleaseを無効化しない。
- recorder状態はowner-only 0600の`recorder-state.json`へatomic writeし、schema
  `codex-info-recorder-state-v1`、exact key
  `schema,pid,process_starttime,owner_nonce,write_state,partition_id_hash,data_generation,collector_epoch,
  cycle_seq,last_commit_unix,updated_at_unix`を持つ。`write_state`は
  `idle_no_account|ready|degraded`だけである。lock/state遷移またはDB transactionのacknowledged commit後だけ
  対応fieldを更新し、heartbeatからcommitを捏造しない。account admitted時は新規usage rowが0件でも各scheduled
  generationをcommitし、`last_commit_unix` freshnessを150秒以内に保つ。全write stateのheartbeat
  `updated_at_unix`もfuture skewを拒否し150秒以内に保つ。account未確定時は`idle_no_account`とし、
  架空partition/commitを作らない。古いheartbeat、古いcommit、`degraded`をmanaged recorderのhealthyへ読み替えない。
- launcherのdesired stateはowner-only 0700 directory内の
  `$HOME/.local/share/codex-info/control-state.json`へ0600・atomic write/fsyncし、schema
  `codex-info-control-state-v1`、exact key
  `schema,desired_state,boot_id,operation_id,generation_id,updated_at_unix`を持つ。
  `desired_state`は`running|stopped|disabled|removed`だけである。`stopped`は同じBootIdだけ有効で、
  boot変更時にenabled serviceをrunningへ戻す。raw `systemctl stop`を永続的な製品停止意図へ推測しない。
- installation recovery journalは
  `$HOME/.local/share/codex-info/install-transaction.json`、0600、schema付きexact key
  `schema,operation_id,owner_pid,owner_starttime,boot_id,phase,old_generation,new_generation,desired_state,
  updated_at_unix`とする。phaseは
  `prepared,legacy_backed_up,entrypoints_linked,candidate_published,current_switched,activation_requested,
  candidate_verified,rollback_switched,rollback_verified,committed`だけで、各遷移をatomic write・file
  fsync・parent directory fsyncしてから次へ進む。ownerがstaleな未terminal operationは同じjournal identityから
  resume/rollbackし、回復前に別operationやwriter publicationを開始しない。
- stale lockはPID不存在またはprocess-start不一致を必要とし、削除直前に同じpathをreopenして取得時のfile identityと再比較する。
  24時間は診断用の経過時間であり削除条件ではない。年齢だけの削除、別ownerの削除、path/identity不一致の削除は常に0件とする。
- fingerprintはcanonical sessions root配下のregular・non-symlink JSONLだけを相対path辞書順で並べ、各fileのdevice/inode、size、mtime_ns、
  最後の完全行offset、最後の完全行SHA-256をLF区切りcanonical bytesへ連結してSHA-256化する。appendはcursor以降、rotation/truncationは同一fileのcursorを捨てて1回だけ再検査する。
  fingerprint不変のcycleはJSONL全走査・DB write・retryを0とする。
- app-server outage epochでpersisted `history/usage_reset_hint.json`とappend-only sourceが両方有効なときだけ、backfill latchを1回消費する。
  hintは`active → expired`または`active → tombstoned`へ一方向に進む。`now >= reset_at`になった旧hintはexpiredとして扱い、
  新規source変更を理由に旧期間のscan/DB row作成をせず、source logだけを保持する。quota cycleまたは明示更新が別に起動しても、
  current authenticated reset periodを検証できない限り旧期間へ帰属させない。current AuthEpochに束縛された
  fresh authenticated hint（`reset_at > now`、同じcurrent source identity、旧hintでない）が受理された後だけ、新期間へ帰属する
  bounded one-shot backfillを開始する。hint、cursor、source identity、AuthEpoch/nonceが不一致ならcandidate全体を破棄し、gapを埋めず旧rootを保持する。
  logout、token失効、AccountKey変更ではAuthEpochを先に増やし、persisted hintを`state=tombstoned`へatomicに更新してcursorと公開候補を無効化する。
  hint/DBへemail、account ID、その他の個人識別値を保存せず、opaque nonceとprocess内AuthEpochだけでepoch境界を検証する。
- daemon停止区間は`RecorderGapLedger`がsource identity/cursorと停止・再開monotonic時刻から回収不能を
  確定した場合だけgapとする。既存REST v1のexact 13-key detailsにある`history_gaps`へconfirmed rowだけを
  projectionし、timestamp不連続だけではmarkerを作らない。確定gapは補間、quota/残量推測、旧値複製の対象外であり、
  backfillが成功した区間だけ実sampleで置換する。
  ledger rowはexact key `gap_id,partition_id,source_identity_before,source_identity_after,cursor_before,cursor_after,
  stopped_at_monotonic_ns,resumed_at_monotonic_ns,start_at,end_at,reset_at,reason,state,owner_collector_epoch,
  confirmation_cycle_seq`を持つ。`state`は`pending|recovered|confirmed|rejected`、`reason`は
  `daemon_stop_unrecoverable|reset_hint_expired|auth_epoch_tombstoned`だけで、一つの`gap_id`を同じterminalへ
  再適用してもDB row・public gapを増やさない。

### 4.2 Maintenance、backup、restore

- canonical DB profileごとに`MaintenanceOwner`を1つだけ許可する。maintenanceはwriter admissionを止めた同じ排他境界で
  `online backup candidate → flush → quick_check/schema/row count/deterministic fingerprint/reset-period境界検証 → verified rotation → prune transaction`
  の順に進む。検証前のcandidate、writer競合、検証失敗ではpruneは0件で、現行DB・旧memory・既存backupを変更しない。
- backup名の世代順は`.bak.1=最新verified`、`.bak.2=次に新しいverified`、`.bak.3=最古verified`とする。
  1 maintenance activationで追加できる検証済み新世代は最大1件で、0→1→2→3 activationと実時間順に蓄積する。
  初回に同一snapshotを複製して不足数を埋めず、未検証・欠損・破損世代を3世代へ数えない。
  candidate fileをflush/fsyncし、parent directoryをfsyncした後、owner-only `backup-rotation-v1` journalへ
  old rank/path/inode/hash、candidate hash、各rename phaseをflush/fsyncしてからrenameする。各rename後もdirectoryを
  fsyncし、全rankとhashを再検証してjournalをcommit/removeする。crash/restart時はjournalとhashから完全rollbackまたは
  roll-forwardを一意に行い、回復完了前のwriter、prune、publishは0とする。
- restoreは通常起動から自動実行しない。明示restoreでは全writer/API/UIを停止して確認し、現DBを削除せず別名退避し、最新の完全verified世代からquick_check/schema/row/hash/period監査を通したものだけを同一filesystemへatomic replaceする。
  reloadとREST/UIのpair検証まで成功する前に旧DB・全backup・old memoryを破棄せず、どの段階の失敗でも旧世代を復元可能なまま保持する。

### 4.3 Migration 3経路とSQLite fault matrix

- **old-schema startup reject**: 現行schemaでないDBはread/writeを拒否し、旧DB、旧backup、old memory/rootをそのまま保持する。暗黙変換や空DB置換はしない。
- **candidate migration success**: `UsageStore::migrate_verified`が同一account partitionの別名candidateをtransactionで作り、全rowの型・値・物理DB内`(reset_at,timestamp)`一意性、exact partition row、quick_check、row count、deterministic fingerprint、reset-period境界を比較する。writer/API/UI停止後、旧DB/candidateをflush/fsyncしparent directoryをfsyncする。owner-only `migration-switch-v1` journalへold/candidate/current path・inode・hash・phaseをflush/fsyncし、各renameとdirectory fsyncを記録する。再読込/pair検証後だけjournalを`committed`へ進め、DataGenerationを1回だけ増やす。terminal journalの削除は別のretention処理で行い、commit成立条件へ混ぜない。
- **candidate validation/switch/crash failure**: candidate、lock競合、backup、rename、fsync、再読込、pair検証のいずれかが失敗した場合、または再起動時に未完了journalがある場合は、journalと実path/inode/hashから完全rollbackまたはroll-forwardを一意に選ぶ。current path不在、current DB二重、空DB自動作成を許さず、回復完了までwriter/publish=0、旧DB/backup/memory/rootを保持し、同callbackで再試行しない。

`migration-switch-v1`はowner-only 0600、UTF-8 JSON、64 KiB以下とし、exact key集合を
`schema_version,operation_id,operation_generation,owner_identity,phase,current_identity,current_sha256,
candidate_identity,candidate_sha256,quarantine_identity,quarantine_sha256,parent_data_generation,
result_data_generation_or_null,created_at_utc,updated_at_utc`に固定する。phaseは
`admission_closed,backup_verified,candidate_validated,switch_intent,current_quarantined,candidate_published,
pair_checked,committed,rollback_required,rolled_back`だけである。`pre_switch_crash,source_lock,candidate_lock,
rename_failure,post_intent_pre_commit_crash,validation_failure`を独立interruptとして扱い、verified `committed`前は
旧DBを唯一のcurrentとして保持する。同じoperation ID/generationの再入はresumeまたはno-opであり、新backup、rename、
DataGeneration、pair publicationを二重化しない。foreign/第二operationはBusyかつmutation 0である。

| fault | bounded action | retention / next state |
| --- | --- | --- |
| SQLite busy | 1 attemptのbusy deadline=2.000秒。deadline到達時はbatch全体rollbackし、same-callback/application polling retry=0。次の通常scheduled cycleまたは明示操作だけが最大1 attemptを開始し、そのattemptも2.000秒 | 1 cycle attempt=1、partial row/duplicate=0、旧DB/backup/memory/root保持。再BUSYは全rollbackし新cycleまで待つ |
| full / open-read-write I/O | transactionまたはcandidate単位でrollback。same callback retry=0 | 旧DB/backup/memory/root保持、次cycleまたは明示操作 |
| read-only filesystem / permission | writer admissionまたはcandidateを拒否し、same callback retry=0 | 旧DB/backup/memory/root保持、明示操作または権限回復後の次cycle |
| corrupt / quick_check failure | corrupt DBを読取成功扱いせず、candidate公開・自動再生成をしない | 旧DB/backup/memory/root保持、明示restore/migrationへ |
| schema mismatch / migration lock | old-schema rejectまたはcandidate switch=0 | 旧DB/backup/memory/root保持、migration修正後に別epoch |
| backup validation/rotation failure | partial candidateをpublishせずprune=0 | 現行DBと検証済みbackup 3世代保持 |

### 4.4 有限入力・snapshot境界

| resource | canonical bound / counting point | failure boundary |
| --- | --- | --- |
| reset hint | 4KiB、UTF-8 JSON bytes、hint path=`history/usage_reset_hint.json` | schema/size超過、expired/tombstoned hint、current AuthEpoch/nonce不一致はhint scan・backfill writeを拒否。source logは保持 |
| recorder lease | 4KiB、UTF-8 JSON bytes、`recorder-lease-v1` | schema/size超過、PID/process-start/file identity不一致はlease取得・stale reclaimを拒否 |
| local JSONL record / session file / selected session prefix | line 4MiB / fileはselected prefixと同じ2GiB / latest whole-file prefix 2GiB、decode前の受信bytes | oversize/invalid complete recordは表示集計を継続できても当該fileのrecorded marker=0。I/O・unterminated record・depth/file-count/file/metadata/containment違反はselected candidate rollback。全inventoryのaggregate超過だけはrollbackせず、最初に収まらないfile以降を保持中overflowとする |
| live rollout record / file / active paths | line payload 4MiB / fileはselected prefixと同じ2GiB / active path 1024、stream受信bytesとProcessIdentity前後値 | oversizeはstreaming envelopeでliveness非変更を完全証明した場合だけpayload隔離。invalid UTF-8/JSON/envelope/state event、identity/ancestry/FD partialはlive cycle全rollback |
| internal validated snapshot | canonical JSON 1MiB | candidate全体をrejectし旧snapshot保持。REST transfer bodyとは別resource |
| REST response headers / status body / details body | 8KiB / 64KiB / 32MiB、transfer後・decode前 | Content-Lengthは事前拒否、streamは最初の超過byteで停止 |
| transaction batch / retry | usage rowsは最大1024かつ1MiB、recorded session markerはinventory上限と同じ最大4096 rows、backfill latch=1、scan/restart retry=1 | 上限到達はpartial公開せず次cycleまたは明示操作。markerは同cycleのusage transactionへ同梱する |

session selectionとcache fingerprintは同じmtime/path降順vectorを使い、overflowを含む直前verified inventoryをCollectorEpoch内だけ保持する。SQLite `recorded_sessions` はcanonical sessions root identity、UTF-8 root-relative normal path、size、mtime nanoseconds、device/inodeの完全fingerprintを複合keyとし、旧source markerを上書きしない。identity boundary時に存在したfile、直前inventoryに存在した未checkpoint file、rotation/truncation/prefix不一致fileは現在EOFへbaselineし、同epochの直前inventoryに存在しなかった新規fileだけoffset 0から帰属できる。cleanupはcommit後の別maintenanceであり、offset 0からEOFまでfully attributedなfresh read-only marker、regular/non-symlink containment、直前まで同一のfingerprint、bounded `/proc` scanによるCodex open-FD不在がすべて成立したoverflow fileだけを削除する。missing、unmarked、baseline、legacy、partial、changed、active、selected、DB/process scan失敗は保持し、同callback retryは0とする。file削除後のmarker削除に失敗した場合はfingerprint-boundのstale markerを保持し、usage history、durable state、verified backup、reset hint、delegation recoveryを変更しない。

## 5. 変更管理の制約

データ保護対象ファイルの変更者は、次を満たさない限り完了を宣言してはならない。

1. [製品要件](PRODUCT_REQUIREMENTS.md)または本書の該当箇所を更新し、要求と失敗境界を逆引きできる。
2. 既存DB行のread-only row count/hashを変更前後で比較する。通常の3か月prune以外の減少はFAILとする。
   DB保持は3暦月、1回の取得はその中の最長1暦月（最大44,640分点）であり、取得上限を保持期間の短縮へ読み替えない。
3. malformed、empty、multiple writer、app-server停止、再起動、認証境界、migration/schema mismatchを検査する。
4. 完全な変更pathと影響master IDを事前分類し、そのIDに直接紐付く既存の破壊操作scan、
   SQLite fixture、Rust testだけを共有呼出しごと1回実行する。データ保護境界を変更していない場合は
   `data_protection_gate.sh`とDB fixtureを実行しない。
5. 実環境確認が必要な変更では、新しいruntime traceを取り、前回の画像・ログを再利用しない。

### 禁止事項

- `rm`、DB削除、DB再生成、無検証の上書きで障害を隠すこと
- 有効値がないのに0%、0 token、0 dollar、空履歴を成功値として作ること
- migrationで旧行を推測変換すること
- 記録済み障害の再現に必要なmaster IDと直接オラクルを更新せず、一時的な手動実行だけで終了すること
- 機械的オラクルで判定できる結果に、理由のない独立評価や第二ゲートを追加すること

## 6. 既知の回帰と再発防止メモ

過去の回帰では、巨大なtool出力1行がstrict file parserを失敗させ、ローカル履歴とactive threadが同時に「取得失敗」になった。またlocal収集がquota応答に結びついていたため、app-server停止中の変動が未収集になった。

再発防止は次の3層で固定する。

- 実装制約: record isolation、persisted reset hint backfill、auth epoch、transaction/upsert、backup-before-prune、live path + rollout terminal stateの二重判定
- 自動検査: 影響するDATA master IDに登録したSQLite consistency、破壊操作scan、fault fixtureだけを実行し、
  同じDB結果を判定するfixtureを別gateに複製しない。
- 完了手順: 影響するIDとその直接オラクルを同一revisionで確認する。無関係な全IDの再評価、一律の独立評価、
  抽出中の`contract authored`や旧`verified`の現行PASSへの昇格を行わない。

「対応したつもり」「今回の実行で通った」は完了条件ではない。


## 8. DP-REST-001..011 DATA契約とWIRE参照

この節が所有するのはDB、transaction、lineage、失敗時保持のDATA契約だけである。route、HTTP status、
header、JSON schema、resource上限は `REST_API_V1.md` だけがWIRE ownerであり、本節のWIRE名はその入力条件を
指す非規範的な参照とする。実装と検証は影響master IDに登録した直接オラクルで判定する。

```text
decision_id = DP-REST-AUTHORITY-20260823-001
decision_version = dp-rest-authority-v1
authority_status = REQUIREMENTS_SELECTED
product_status = PRODUCT_PENDING
release_status = HOLD
```

### 8.1 共通採用規則

- REST workerはread-only consumerであり、health/status/details、404、405、server errorのいずれでも
  SQLite transaction、DB/WAL/SHM、backup、migration、checkpoint、PublishedPairを変更しない。
- statusとdetailsは同じ`DR-AdmissionTuple`に属する一つの`DR-LastGoodPair`としてだけ採用する。
  status単独またはdetails単独のcommit、表示、cache更新は0件とする。
- wire producerが返す全JSON responseのContent-Typeは正確に
  `application/json; charset=utf-8`である。consumerもmedia type=`application/json`かつ
  charset=`utf-8`の両方を要求し、charset欠落・別charset・parameter追加をrejectする。
- authority不一致・schema不正・resource超過・stale ownerではcandidate側のwrite、publish、表示を0件とし、
  直前の完全DB、checkpoint、PublishedPair、Windows accepted rootを保持する。

### 8.2 DP-REST-001 / RC-139 — healthとpair保持

healthはAPI listenerの到達性だけを所有し、認証、ready、DB健全性、snapshot鮮度を意味しない。
health候補がmalformed、oversize、foreign listener、schema/header不一致ならconnection stateを
`HealthUnavailable`へ遷移させるが、`DR-LastGoodPair`、DB、collector stateは変更しない。同じrequestを
再送してもdata generation、pair publication、DB writeは0件である。wire schemaと1 KiB上限は
`REST_API_V1.md`の§「DP-REST wire authority」を唯一のownerとする。

### 8.3 DP-REST-002 / RC-140 — server faultとatomic pair

publisher欠落、DB/root read fault、stale owner、内部read timeoutはREST側のcanonical 503、response生成失敗は
canonical 500またはresponse未commit時のconnection abortへ写像する。200 error bodyは使用しない。どのfaultも
status/detailsの片側だけを進めず、clientはS1+D0やS0+D1を作らない。特に`WIN-I-016`の採用規則は
`both-valid-and-same-pair→commit both`、それ以外は`retain S0+D0`であり、status-only commitを禁止する。
この禁止はdata pairのstatus/details store commitに適用する。
schema-validな`auth_required+authenticated=false`のauth-clearはsecurity visibility transitionとして
旧account可視値だけを一回で消去してよく、details/store/DB/pair bytesは不変にする（`WIN-I-016`）。

### 8.4 DP-REST-003 / RC-141 — request resource owner

request line、header、body、connection、timeout、keep-alive、shutdown drainの値はREST wire ownerへ委譲し、
data ownerは全rejectでDB、memory pair、settings、checkpointのbefore/after hash一致を要求する。connection/request
identityはlistener generationへbindし、shutdown開始後の新規request admissionは0件とする。

### 8.5 DP-REST-004 / RC-142 — read-only effect分類

許可する副作用は、当該request lifetime内のheap buffer、bounded in-memory counter、loopback socketのread/write、
read-only file open/statだけである。禁止する副作用は、persistent access/error log、Event Log、persistent metric、
cache/temp file、registry、child process、非loopback socket/DNS、file create/write/rename/delete/fsync、
SQLite transactionとDB/WAL/SHM mutationである。OSがread-only openに伴って更新し得るatimeはproduct data mutation
の成功条件に使わず、DB content/inode/WAL/SHMとproduct syscall traceを判定ownerにする。allowlist外のeffectを
検出したresponseは成功証拠にせずreleaseをHOLDにする。

### 8.6 DP-REST-005 / RC-143 — storage partition identity

partition keyは`(ProfileScopeId, AccountScopeId, StorageEpoch)`であり、各partitionを
`history/accounts/v1/<AccountScopeId>/epoch-<StorageEpoch>/usage_history.sqlite3`へ物理分離する。
usage rowの一意keyは各物理DB内の`(reset_at,timestamp)`である。DBにはexactly oneの
`storage_partition` rowを置き、schema/profile/account/epoch/partition IDと`quick_check`が一致しないfileを開かない。

- `ProfileScopeId`: 保存profileを作成した時に生成する128-bit random opaque ID。raw WSL distro、SSH alias、pathを
  DBへ保存しない。
- `AccountScopeId`: authenticated app-server ownerがcanonical AccountKeyから、owner-only 256-bit install keyを使い
  `HMAC-SHA-256("codex-info-account-scope-v1" + NUL + AccountKey)`で生成する32-byte値。raw AccountKey、email、tokenを
  DB・hint・logへ保存しない。
- `StorageEpoch`: partition作成時のmonotonic unsigned 64-bit値。account/profile不一致やHMAC key欠落時は新規writeと
  publishを0件にし、自動的な空partitionや推測mergeを作らない。

同一account/profileの再認証は同じpartitionを再利用し、別account/profileは別partitionにする。画面は現在認証済み
partitionだけを公開し、旧partitionを削除・混合しない。HMAC install keyは0600 owner-only fileへatomic保存し、
欠損時は既存AccountScopeIdを再生成せずrecovery-requiredとする。

canonical AccountKeyはowner-only・regular・0600・1..65536 bytesで前後identityが安定した
`CODEX_HOME/auth.json`のexact `tokens.account_id` bytesである。前後`account/read`とprocess-local
`AccountUpdateGeneration`を含むconfirmed windowが不一致なら、DB/WAL/SHM、checkpoint、publishを0件にする。
raw AccountKey、email、tokenはpath、profile metadata、DB、journal、log、RESTへ保存しない。

### 8.7 DP-REST-006 / RC-144 — cursorとDB transaction

authoritative checkpointは外部cursor fileではなく同じaccount SQLite DBの`session_checkpoints` tableに置き、usage row batch、
`session_ranges` dedupe、model totals、fully-attributed marker、`DataGeneration`と同じtransactionでcommitする。checkpoint keyは
`(sessions root identity,root-relative path,device,inode,prefix generation)`であり、旧source identityのcheckpoint/markerを
新sourceで上書きしない。外部hintはscan開始候補でありcommit authorityではない。

transaction commit前はrow/checkpoint/generationの全てが旧値、commit後は全てが新値である。commit後publish前のcrashは
再起動とidentity boundaryでは旧cursorを再開せず、現存fileをEOFへfresh baselineする。source record identity
`(root identity,relative path,device,inode,prefix generation,start offset,end offset,record SHA-256)`はuniqueで、
同一CollectorEpoch内の再読込はno-op、partial rowとcursor先行を0件にする。

### 8.8 DP-REST-007 / RC-145 — typed generation namespace

bare integerを異なるnamespace間で比較しない。採用型は次のとおりである。

- `BootId`: Linux `/proc/sys/kernel/random/boot_id`のUUID。
- `SupervisorLeaseIdentity`: canonical DB profile、BootId、PID、process start ticks、128-bit owner nonceのtuple。
- `auth_epoch`: logout、account change、identity failure、account worker restartごとに増えるprocess-local u64。overflowはprocess restartによるrecovery-requiredとする。
- `AccountUpdateGeneration`: 同じapp-server processでstrictな`account/updated`受理ごとに増えるprocess-local u64。別process値と比較せず、malformed/overflowでauthorityを失効する。
- `CollectorEpoch`: service startと各identity boundaryで生成する128-bit random ID。同じepochだけSession continuityを認める。
- `CycleSeq`: CollectorEpoch内で1から始まり、admitted cycleごとに1増えるu64。
- `DataGeneration`: partition内で0から始まり、usage rowとcheckpointの同一transaction commitごとに1増えるu64。
- `BackupGeneration`: DB profile内のu64と128-bit backup ID。parent DataGenerationとDB SHAを必須にし、一activationで
  最大1世代だけ増える。0/1/2から同一snapshot複製で3世代を穴埋めしない。
- `MigrationGeneration`と`RestoreGeneration`: 128-bit operation ID、parent DataGeneration、source DB SHA、result
  DataGenerationのtuple。成功switch後だけresult DataGeneration=`current+1`、失敗時は未発行である。
- `ServiceGeneration`と`BootstrapGeneration`: service manager activation IDとartifact SHAのtuple。

全ledger entryはnamespace tag、value、parent、operation kind、scope、source/result hashを持つ。unknown parent、回帰、
同namespace同値別hash、stale CollectorEpochはpublish 0である。

### 8.9 DP-REST-008 / RC-146 — restore journal

restore journalはowner-only 0600、UTF-8 JSON、64 KiB以下、schema=`codex-info-restore-journal-v1`とし、exact keyを
`schema_version,operation_id,operation_generation,owner_identity,phase,current_identity,current_sha256,
candidate_identity,candidate_sha256,quarantine_identity,quarantine_sha256,source_backup_generation,
parent_data_generation,result_data_generation_or_null,created_at_utc,updated_at_utc`に固定する。phaseは
`admission_closed,candidate_audited,current_quarantined,candidate_replaced,pair_checked,committed,rollback_required,
rolled_back`だけである。

各rename前後にfileとparent directoryをfsyncしてjournal phaseをatomic更新する。未terminal journalがある起動はwriter/API
admissionを閉じ、same operationだけをresumeする。第二・foreign restoreはBusyでmutation 0。current candidateの双方が
validでもjournal identity/hash/phaseだけで一意にrollbackまたはroll-forwardし、mtimeやfilename推測を使わない。
`pair_checked`完了前はold pairを保持し、current DBを削除しない。

### 8.10 DP-REST-009 / RC-147 — boot recovery order

systemd installed profileでは`codex-info.service`がDB/sourceとloopback RESTを同じactivationで所有する。
serviceはrecorderのjournal/lease/checkpoint検査が`RecoveryReady`になった後だけpublisher admissionを開く。
BootIdを毎activationで取得し、旧BootIdのlease、未terminal
maintenance journal、file identity不一致checkpointがあればwrite/publishを0にして`RecoveryRequired`へ置く。

同一BootId・service activation IDのStartLimit外再入は新CollectorEpochを作らずno-op、別BootIdでは旧callbackを全破棄して
新SupervisorLeaseIdentity/CollectorEpochを各1件だけ発行する。systemd外の明示service commandも同じrecovery gateを通る。

### 8.11 DP-REST-010 / RC-148 — lineage schema

`DR-DataRestLineage`はcanonical UTF-8 JSON objectでschema=`codex-info-data-lineage-v1`とする。必須fieldは
`schema_version,source_release_id,server_artifact_sha256,windows_artifact_sha256_or_null,profile_scope_id,
account_scope_id,storage_epoch,source_file_identity,source_fingerprint,cursor_start,cursor_end,source_record_set_hash,
collector_epoch,cycle_seq,transaction_id,data_generation,db_sha256,root_hash,published_pair_hash,listener_generation,
request_id,route,response_status,response_body_sha256,client_snapshot_hash_or_null,render_generation_or_null,
operation_started_at_utc,operation_committed_at_utc`である。

product常時運転でこの全objectを別logへ毎回保存せず、DBにはcheckpoint/generation/rootだけを保持する。test/evidence modeは
同じ内部IDsからobjectをbounded生成し、raw path、account、token、bodyを含めない。全edgeのhash/parent/time順を再結合できない
candidate、別artifact、同じlineage IDの別hashは受理0である。

### 8.12 DP-REST-011 / RC-149 — supported low-load profile

製品保証scopeは、1 recorder、1 loopback API、1 Windows client、10秒completion-based poll、in-flight 1、source不変、
maintenance停止中の`steady_idle`である。warm-up 2分後の30分窓で次を同時に満たす。

- recorder: CPU平均0.5%以下、1秒sample p95 2%以下、RSS増加16 MiB以下、full source scan/DB write/retry 0。
- API: request外CPU平均0.2%以下、1秒sample p95 1%以下、RSS増加8 MiB以下。各pollはhealth/status/detailsを各1回以下。
- Windows client: CPU平均1.0%以下、1秒sample p95 5%以下、RSS増加32 MiB以下、poll queue 0。
- Linux recorder+APIとWindows clientを各hostの1 logical CPUへ正規化した平均の合計は2.0%以下。source不変時の
  product DB/write bytesは0、networkはloopback/SSH tunnelのpoll bytesだけである。

`changed,backfill,maintenance,recovery`はidle保証へ混ぜず別profileとし、各generationでscan 1、DB transaction 1、
publish 1以下、batch 1024 rowまたは1 MiB、REST request境界は§8.4、backfillはlatest whole-file selected prefix 2 GiB上限を守り、older overflowを同じscanへ穴埋めしない。
測定不能または超過は要求PASSへ丸めずPRODUCT_FAIL/HOLDにし、負荷低減のためデータを捨てたりpollを重ねたりしない。

### 8.13 RC-067 — gap ledgerとREST projection

通常欠測と回収不能gapをtimestamp間隔だけから推測する設計は禁止する。DB ownerは
`recorder_gap_ledger`を持ち、schema=`recorder-gap-ledger-v1`、exact fieldを
`gap_id,partition_id,source_identity_before,source_identity_after,cursor_before,cursor_after,
stopped_at_monotonic_ns,resumed_at_monotonic_ns,start_at,end_at,reset_at,reason,state,
owner_collector_epoch,confirmation_cycle_seq`へ固定する。reasonは
`daemon_stop_unrecoverable,reset_hint_expired,auth_epoch_tombstoned`、stateは
`pending,confirmed,recovered,rejected`だけである。

stop/restartを検出した時点では`pending`であり、bounded source rescan/backfillが完了するまでREST/UIへgapを
公開せずlast-good Graph rootとbounded statusを保持する。全missing minuteがvalid source recordで回収できた場合だけ
`recovered`としgap projection 0、回収不能な閉区間をsource cursorと前後identityで証明した場合だけ`confirmed`とする。
invalid/foreign owner、時刻逆転、reset period外、overlap contradictionは`rejected`でcandidate rootを変更しない。

REST detailsへ公開するのはconfirmed rowだけで、raw path/cursor/process/ownerを除外した
`history_gaps` projectionとする。projectionは`gap_id,reset_at,start_at,end_at,reason`の5 exact field、
同一period内で`start_at<=end_at`、重複/交差なし、canonical `(reset_at,start_at,end_at,gap_id)`順、最大4096件である。
この追加は未出荷のREST v1 contract revision
`rest-v1-details-reset-at-20260823`として明示し、server/client/release manifestのrevision一致を必須にする。
旧12-key details clientは新13-key bodyをrejectしてfail closedし、旧artifactと新serverを混在させて成功扱いにしない。
API family/pathと`api_version="v1"`は維持するが、同一releaseに旧/new schemaを混在させない。

Graphはnormal unbracketed terminal missingでlast measured remainingを水平保持する。confirmed gapのstart直前でsubpathを
終了し、gap区間へremaining/model valueをcarry/interpolateせず、gap markerだけを描く。pending/rejected ledger、
timestamp間隔だけ、transport errorからgapを推測しない。gapが後の明示repairでrecoveredへ変わる場合は、完全な新details
rootの実sampleとgap集合を一括採用した時だけmarkerを除去する。

## 9. RC-167〜169 データ保護 fault / source checkpoint closure

この節は `WIN-J-007..016` に対する要件抽出の正本であり、製品実装済み・実機確認済みを意味しない。
既存行の `depends_on` と B2B projection header は変更せず、ここで定義する RC-167〜169 は
既存行の independent oracle へ join する。実行前提でない oracle join を新しい hard edgeへ昇格させない。

### RC-167 — source rotation / truncation checkpoint

`source_event` は `append`、`rotate`、`truncate`、`replace` の4値だけとする。source identity は
`(device,inode,size,prefix_generation)` とし、Windowsでの実体名はそれぞれ
`device_or_volume_serial`、`file_index_or_inode` と記録する。`prefix_generation` は canonical pathごとの
opaque 128-bit値で、CollectorEpoch、source identity、prefix SHA-256から導出する。完全prefix hashが変わった場合または
identity replacementが検出された場合は別generationとなり、mtime、filename、現在時刻から推測しない。

event分類は、再openした同一canonical pathの before/after identity と prefix hashを同じoracleで比較し、
`rotation_marker=1` かつ identity変更なら `rotate`、identity変更で markerがなければ `replace`、
identity不変かつ size減少なら `truncate`、identityとprefixが不変で size増加なら `append` とする。
それ以外は `replace` として fail-closed にする。

- `append` は同じ device/inode/prefix_generation の durable cursor
  `(last_complete_lf_offset,last_complete_row_sha256)` 以降だけを読む。
- `rotate`、`replace`、`truncate`、prefix不一致は新しいsourceを現在EOFへbaselineし、境界以前のbytesを自動帰属しない。
- 旧cursorで有効recordを `skip` する数は `skip_count=0`、dedupe keyによる重複insert数は `dedupe_insert_count=0` とする。
  dedupe key は `(partition_id,file_device,file_inode,start_offset,end_offset,record_sha256)` である。
- 1 eventにつき scan は最大1回、DB transaction は最大1回、同じcallback内のretryは `0`、次の通常cycleまたは
  明示操作でのretryは最大1回とする。usage rowsは1 transaction最大1024かつ1 MiB、recorded markerは最大4096、1 JSONL recordは4 MiB、
  1 source fileの上限は1回のselected session prefixと同じ2 GiBとし、差分取得の前にそれより小さい独立file上限で拒否しない。全inventoryが2 GiBを超える場合はolder overflowを保持し、入力fingerprint不変時は全走査と新規DB writeを各 `0`、既存marker cleanupは次の通常cycleに最大1回とする。

RC-167 oracle は `source_identity_before/after`、`prefix_generation_before/after`、cursor before/after、
`scan_event`、`scan_count`、`scan_bytes`、`scan_records`、`transaction_count`、`skip_count`、
`dedupe_insert_count=0`、前後の完全record hash、DB row/file SHA、publisher generation、restart traceを
同一case markerへ結合する。old cursor継続、無制限再走査、4 MiB超record受理、前後valid recordの削除、
lease/generation不一致のpublishは FAIL とし、failure時は old checkpoint、DB、confirmed gap ledger を保持する。

### RC-168 — exact database fault matrix

fault enum は次の11値に固定する。各caseは `RC-168:<fault_enum>:<injection_point>:v1` の専用markerを持ち、
同じfault名を別注入点の結果へ流用しない。

| fault_enum | injection_point | exact SQLite result / exact OS result | required transition | retention and retry |
| --- | --- | --- | --- | --- |
| `BUSY` | usage transaction begin or write batch | `SQLITE_BUSY` / `NONE` | full transaction rollback | old DB, verified backups, history, root保持; same-callback retry=0; next cycle max1 |
| `LOCKED` | shared-cache transaction or backup read while competing writer owns lock | `SQLITE_LOCKED` / `NONE` | candidate and transaction mutation=0 | old DB/backup/history保持; lock解消後の別cycleだけmax1 |
| `IOERR` | source open/read, DB write, candidate fsync, or directory fsync | `SQLITE_IOERR` / `EIO` | batch or candidate rollback | partial row/candidate保持なし; same-callback retry=0 |
| `FULL` | DB page write or backup candidate fsync | `SQLITE_FULL` / `ENOSPC` | transaction rollback and prune=0 | current DB, backups, history保持; 容量解消後の別cycleだけ |
| `READONLY` | writer admission or DB/candidate open with read-only filesystem | `SQLITE_READONLY` / `EROFS` | writer/candidate admission=0 | old DB/backup/history保持; automatic repair=0 |
| `PERMISSION` | DB/backup open, rename, or prune delete permission check | `SQLITE_CANTOPEN` / `EACCES` | operation mutation=0 | old DB/backup/history保持; permission回復後だけ明示/次cycle |
| `CORRUPT` | DB open/read or `PRAGMA quick_check` | `SQLITE_CORRUPT` / `NONE` | corrupt source/candidate publish=0 | old readable DB/verified backups/history保持; empty DB再生成=0 |
| `BACKUP_VALIDATION` | candidate quick_check/schema/row/hash/period validation before rotation | `SQLITE_OK` plus `BACKUP_VALIDATION_FAILED` / `NONE` | candidate publish=0 and prune=0 | current DB and verified backup set保持; unverified generation採用=0 |
| `BACKUP_ROTATION` | backup journaled rename or parent-directory fsync | `SQLITE_OK` / `EIO` | rotation switch=0 and prune=0 | pre-fault current DB and verified generations保持; journal reconcileまでpublish=0 |
| `PRUNE_CONTENTION` | prune transaction after verified backup and before delete commit | `SQLITE_BUSY` / `NONE` | prune delete=0 and transaction rollback | current DB, all verified backups, history保持; next maintenanceだけ |
| `MIGRATION_LOCK` | migration lease/candidate switch admission | `SQLITE_BUSY` / `NONE` | migration switch/delete/publish=0 | old DB唯一current、candidate/journal保持; foreign operation takeover=0 |

各 fault は `operation_id`、injection point、SQLite/OS result、transaction id、canonical row SHA before/after、
current DB file SHA before/after、各 backup file SHA before/after、`quick_check`、candidate/backup/prune state、
restart後の open/read result を同じ raw recordへ入れる。fault cycleの `success_commit`、partial row、partial switch、
delete、publish、synthetic recovery は全て `0` とし、old DB・verified backup・historyを保持する。
restart後は old DBを open/read でき、検証済み世代は `quick_check=ok` でなければならない。faultの原因解消前に
同じcallbackで再試行せず、復旧後の新generationだけを1回 publishする。fault結果の流用、corrupt DBの上書き、
未検証backup採用、prune先行、空DB成功化は FAIL とする。

### RC-169 — migration atomic switch / J015-J016 re-entry

RC-169 は既存 `WIN-J-015` の「migration失敗時に旧DBを保持する」意味と、`WIN-J-016` の「clientはDBを
破壊的再生成しない」意味を変更しない。専用case markerは
`RC-169:<interrupt>:<operation_id>:<operation_generation>:migration-switch-v1` とする。
required interrupt は `pre_switch_crash`、`source_lock`、`candidate_lock`、`rename_failure`、
`post_intent_pre_commit_crash` の5値であり、既存J015の `validation_failure` は追加の候補検証失敗caseとして残す。

各caseは `owner_identity`、migration lease、old/candidate/intent/backupの path・device/inode・SHA、
exact journal key/phase、rename count、switch/delete/publish countを記録し、
`admission_closed → backup_verified → candidate_validated → switch_intent → current_quarantined →
candidate_published → pair_checked → committed` または rollback pathを一度だけ進める。割込み後のrestartは
journalと再取得したfile identity/hash/phaseだけから rollback または roll-forward を一意に選ぶ。

verified `committed` 前は old DBだけを logical current とし、lock・validation・rename・crashの全経路で
`switch=0`、`delete=0`、`publication=0`、新DataGeneration発行=0とする。成功経路だけが candidate current=1、
old DB retained=1、rename=1、publication=1、DataGeneration delta=1となる。missing/double/empty current、
foreign owner、stale journal、未検証candidate、old DB削除、synthetic commitは FAIL とする。

同一 `operation_id` と `operation_generation` の再入は journal の同じphaseから resume または no-op とし、
追加 backup、rename、switch、delete、generation、pair publicationを各 `0` にする。foreign/second operationは
Busyで mutation `0` とする。J016 client/REST consumerはこのmigration journalやLinux DB pathへopen/write/deleteせず、
invalid/partial/foreign pairは直前のaccepted rootを保持する。RC-169 oracle は5 interrupt＋validation_failureの
restart trace、old/candidate/current count、path identity/hash、journal phase、rename/publication、DB/backup/history SHAを
独立再計算し、J015/J016の専用marker、oracle、re-entry結果を同じ artifact lineageへ結合する。
