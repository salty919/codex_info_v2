<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->
<!-- codex-info-requirement-owner: WIRE -->
<!-- codex-info-master-ids:
REST-129
WIN-PARITY-WIRE-01
WIN-PARITY-PAIR-01
API-V3-MODELS-01
API-DEPRECATION-01
-->

# Loopback REST API v1 / v2 / v3

## API世代と廃止境界

`API-V3-MODELS-01`: v3の`current`と`history` resourceはcommit済みdomain snapshotを、有界な`models`配列として返す。model ID、token内訳、価格計算可否を事実として分離し、UI固定列や表示文言をwire fieldにしない。`/health`はAPI世代から独立したread-only readiness endpointとし、collector、DB writer、外部quota取得の生存状態を混同しない。

v3の各履歴rowは`models`と`models_complete`を持つ。各model rowの`total_tokens`と`total_dollars`は個別に確認できた累計、入力・cached入力・cache write入力・出力は確認できたfieldだけを持ち、未確認fieldを反復`null`で送らない。旧`usage_history`のSOL/TERRA/LUNA列は既知の`total_tokens`と`total_dollars`として保持し、同時刻の不完全なgeneric model集合へ名前単位でmergeする。旧schemaにない内訳を0や推測値で補わない。`models_complete=true`は同じ観測で全モデル集合を確定できた場合だけ許可し、`model_source=confirmed`と非nullの`models`を必要とする。保持ログからASTRAだけを回収した場合や旧3モデルだけが既知の場合は`models_complete=false`、`model_source=legacy-unknown`とし、配列にないモデルを0と解釈しない。clientは掲載modelの実測値を通常線で表示し、当該model自体の未観測区間だけを破線または切断で示す。集合の不完全性だけで掲載modelを予測値へ降格しない。

`API-DEPRECATION-01`: `/v1/details`、`/v2/details`、全表示情報を一体化した`/v3/details`は互換adapterである。互換期間中は同じatomic generationから生成し、既存field、値型、header allowlistを変更しない。新clientはv3 split resourceを優先し、`/v3/current`がexact 404の場合だけ`/v3/details`、さらにexact 404の場合だけv2、v1へfallbackし、世代をmergeしない。廃止日は未決定であり、決定前に`Sunset`を送らない。将来の削除対象は旧details route、adapter、client fallbackだけで、Session collector、SQLite writer、domain model、`/health`は対象外とする。

## 目的と境界

Linux / WSL 上で起動する Codex Info のresident serviceと、Linux / Windows UI向け読み取り専用 APIを、
`codex_info`の1プロセスで所有する。このdaemon modeはSlint WindowやX event loopを生成せず、
X UIを表示しない。resident serviceだけがquota、local usage、historyのauthority/writerである。
`RecorderDaemon`はservice process内のbounded workerとしてsource JSONLを検証し、storage admissionを通過したraw rowを
SQLiteへtransactionalに書く。`HistoryCanonicalizer`はcommit済みraw rowのread-only viewからpublic logical sampleを作る。
`SnapshotPublisher`がcommit済みの完全な`DataGeneration/DataHash`と現行account admission tuple
`(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, AccountUpdateGeneration, CollectorEpoch, CycleSeq)`、および別責務の`SupervisorLeaseIdentity`からimmutableなdetails
generationを構築し、native UIとREST workerへ同じgenerationをread-onlyで渡す。HTTP要求からCodex
app-server、認証 URL、セッションファイル、SQLite、Slint / X11へ直接到達する経路は持たない。

このAPIはインターネット公開、LANへの直接公開、ブラウザー向けCORS、書込み
操作、ログイン操作を対象外とする。既存の Linux / X11 UI は引き続きローカルで
動作する。

Windows clientの接続設定はREST resourceではなく、`language`、`setupCompleted`、
`connectionConfigured`、`timeZoneId`、`connectionProfile`、`connectionSelector`の6 keyだけを
ローカルに保存する。profileは`none|wsl|sshConfigAlias`、selectorはWSL exact distribution tokenまたは
literal OpenSSH Host alias grammarに限定し、secret、展開済み値、raw host/user/pathを保存・送信しない。
saved selectorのauto reconnectは`ArgumentList`＋`BatchMode=yes`で起動し、auth argvもsaved profileから作るが、
起動成功、health readiness、strict details受理は別stateとする。4-key recoveryはMain disconnected＋Settingsだけであり、製品判定は`PRODUCT_PENDING`。

| 要望 | v1での対応 |
| --- | --- |
| Linux / WSL をサーバー化する | 引数なしまたは`codex_info --port 8787`でdaemon+RESTを1プロセスとして起動する。Windowは生成しない。 |
| Windowsから監視する | SSHローカルポート転送先の固定JSONを、`windows-client/` の Windows 監視クライアントが表示する。 |
| Linuxネイティブ環境を残す | 引数なし/`--port PORT`はdaemon+RESTのみ、`--ui`はdaemon+REST+X UI、`--ui --port PORT`は指定portで起動する。 |
| イントラネットだけを対象にする | loopbackだけへ束縛し、SSHを暗号化・認証境界にする。 |
| インターネット経由は別設定にする | v1の設定や認証を再利用せず、今回の対象外として分離する。 |

## 起動と SSH トンネル

user-systemdを使うprofileでは`codex-info.service`が
installed環境ではlauncherがRelease/local generationをreconcileしてuser-systemdのmanaged serviceを開始する。
待受アドレスとportは`127.0.0.1:8787`に固定する。

```bash
"$HOME/.local/bin/codex-info" --start
"$HOME/.local/bin/codex-info" --ui
"$HOME/.local/bin/codex-info" --status
```

引数なし/`--start`はWindowを表示せず、managed serviceの1 processがrecorder lockとREST listenerを所有する。
`--ui`は同じmanaged ownerへ収束後、verified current payloadのUIだけを追加する。installed launcherへ
`--port`は指定できない。raw payload `codex_info --port PORT`はservice・開発・E2E用で、待受addressは
その場合もloopback固定である。自動起動解除はlauncher `--disable-autostart`、unit解除は`--remove`を使い、
DB/history/installed generationを保持する。

Windows からは SSH のローカルポート転送を使う。

```powershell
ssh -N -o BatchMode=yes -L 8787:127.0.0.1:8787 <connectionSelector>
```

上記は接続関係を示す概念的なshell例であり、Windowsクライアントの実行argv順序を定義しない。
Windowsのcanonical `ArgumentList`はValue AuthoritiesおよびWIN-E-006..010の
`[ssh.exe,-o,BatchMode=yes,-N,-L,8787:127.0.0.1:8787,<validated alias>]`に従う。

このときLinux / Windows UIは`http://127.0.0.1:8787`のv3 split resourceから、可視surfaceに必要な
current、period/history page、またはThreadsだけを取得する。各取得cycleは同じpublished pairの完全集合だけを
atomic commitし、部分pageをmergeしない。履歴差分だけは、直前rootのcursorが既取得prefix不変を証明した場合に限り、新pairの完全集合を旧prefixへatomic appendできる。新resourceがない旧serviceへの互換fallbackだけは
`/v3/details`、v2、v1をexact 404順で一応答ずつ受理する。HTTPはLinux側のloopbackと
SSH トンネルの端点の間だけで使用し、端末間の暗号化・相手認証は SSH が担当する。
そのため v1 では HTTPS 証明書を扱わない。

## リソース

すべての応答は `Content-Type: application/json; charset=utf-8` と
`Cache-Control: no-store` を返し、response header aggregateは8 KiB以下とする。既知の固定bodyでは
`Content-Length`をUTF-8 body bytesと一致させ、未知長の受信側もstream中の上限超過で停止する。
`Set-Cookie`、`Location`、`Content-Encoding`、`WWW-Authenticate`、proxy/authentication headerは
返さない。response header allowlistはこの`Content-Type`/`Cache-Control`、固定body時の`Content-Length`、
snapshot resourceの`Codex-Info-Published-Pair`だけで、その他のapplication/proxy headerを追加しない。
v3 clientはrouteとqueryごとに直前に受理したpublished pairをquoted `If-None-Match`として一つだけ送信でき、同じ
published generationならserverは同じpair headerとbody 0の`304`を返す。旧client向け200応答へ新headerを
追加せず、v1/v2 fallbackへ条件headerを送らない。304を新しいsnapshotや失敗へ
読み替えずlast-good rootを維持し、pairが異なる場合だけ200の完全rootをatomic置換する。
許可する成功メソッドは `GET` だけである。自動解凍、redirect、cookie、proxyは使用しない。

| Request | Result |
| --- | --- |
| `GET /v1/health` | resident serviceがread-only snapshot requestを受理できるreadinessを示す。 |
| `GET /v1/details` | 旧client向けのschema-compatibleな単一atomic rootを返す。 |
| `GET /v2/details` | 旧client向けのprovenance付き単一atomic rootを返す。 |
| `GET /v3/details` | split resource非対応client向けの任意model単一atomic rootを返す。 |
| `GET /v3/current` | Mainに必要な状態、利用枠、model累計、active thread countだけを返す。 |
| `GET /v3/history/periods` | Graphの期間metadataだけを返す。 |
| `GET /v3/history?period=<opaque>&cursor=<opaque>` | 選択期間の履歴または`cursor`より後の差分を返す。`cursor`は初回だけ省略できる。 |
| `GET /v3/threads` | Threads surfaceを開いたときだけthread rowsを返す。 |

history成功bodyはexact 5 keys `api_version`、`history_samples`、`history_gaps`、`next_cursor`、`resume_cursor`を持つ。`next_cursor`は同じ取得cycleに後続pageがある場合だけ非nullで、その値を次requestへ使う。`resume_cursor`は応答の最後のsampleまでを証明し、最終pageでも非nullである。cursor付き要求に新sampleがない場合は受理したcursorを`resume_cursor`へ返し、初回要求にsampleがない場合だけnullとする。clientは`next_cursor=null`まで同じpairを集めた後、最後の`resume_cursor`を次回差分用に保存する。

未定義のパスは JSON の `404`、既知パスへの非 `GET` は JSON の `405` で返す。
応答に email、認証 URL、認証トークン、raw error、ローカルパス、セッション内容を
含めない。

path照合はcase-sensitiveかつ完全一致であり、URL decode/normalizationを行わない。case-altered path、
末尾slash追加、`/v3/history`以外のquery付きknown path、未定義prefixはunknown pathとして扱う。history queryは
`period`をexactly one必須、`cursor`を最大1件とし、未知・重複・空・percent encoding・fragmentを拒否する。拒否bodyは`api_version="v1"`と固定error codeだけを持つ
JSON objectとし、未知キー・raw error・秘密値を含めない。404/405を含む全responseは
SQLite transaction、WAL/SHM、migration、prune、backup、DB row/hash、published generationを変更しない。

### Response・read-only matrix

| request | method | result | response / side effect |
| --- | --- | --- | --- |
| `/v1/health` | `GET` | `200` | JSON health object、required `Content-Type`/`Cache-Control` headers、DB write/transaction=0 |
| `/v1/details` | `GET` | `200` | current immutable details generation、共通headerに加えて必須`Codex-Info-Published-Pair`、DB write/transaction=0 |
| `/v2/details` | `GET` | `200` | 同じcurrent immutable generationのprovenance付きprojection、同じ必須published pair、DB write/transaction=0 |
| `/v3/details` | `GET` | `200` | 同じcurrent immutable generationの任意model projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/current` | `GET` | `200` | history/period/thread rowsを含まないcurrent projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/history/periods` | `GET` | `200` | period metadata projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/history?period=...&cursor=...` | `GET` | `200` | 同一pair・選択periodの有限page、DB write/transaction=0 |
| `/v3/threads` | `GET` | `200` | thread projection、同じ必須published pair、DB write/transaction=0 |
| v3 snapshot resource＋route/queryのcurrent pair | `GET` | `304` | body 0、同じpublished pair、route-local last-good維持、DB write/transaction=0 |
| 上記known path | `HEAD/POST/PUT/PATCH/DELETE/OPTIONS`等全non-GET | `405` | 固定JSON error、同上header、DB/WAL/SHM/migration/prune/backup=0 |
| unknown、case-altered、末尾slash、query付きpath（methodを問わない） | any | `404` | 固定JSON error、同上header、DB/WAL/SHM/migration/prune/backup=0 |

RESTのtransfer body上限は既存のOOM/DoS安全境界であるdetails `32 MiB`、health/error `4 KiB`、response header `8 KiB`を全resourceで共有する。この値を通常データ件数の妥当性判定へ使わない。historyは次の正当なrowを加えると32 MiBへ達する場合だけ、そのrowの直前でpageを完了してopaque cursorを返し、次pageへ必ず前進する。単一row自体が安全境界を超える場合だけ`resource_too_large`とし、recorder、DB、公開generationを変更しない。
SQLiteの保持期間は過去3暦月である。一方、1回のDB取得と`details`応答が扱う履歴は観測時刻で終わる
最長1暦月の半開区間 `(one_month_before(observed_at), observed_at]` に限定する。history samples上限は
31日分の1分bucketに相当する`44,640`、history periods `128`、confirmed history gaps `4,096`、threads `256`である。v3 history pageのrow数は固定せず、選択期間の残件数と上記OOM境界から決まる。v1/v2のmodels上限は固定3件、v3のtop-levelおよび各history rowのmodels上限はOOM/DoS境界として`1,024`件である。

### 応答時間SLOと容量条件

Release buildを用い、warm loopback、in-flight 1で3回のwarm-up後、request送信開始からresponse body全受信までを各profile 30回測る。
値を昇順に並べnearest-rankのP90を27番目、P95を29番目とする。profileはhealth、current、periods、選択期間history初回、同cursor差分、threads、固定4xxの到達経路に限定する。各結果にはCPU、memory、storage、OS、build、実際のwire bytes、sample数、model数、thread数を併記し、異なる環境または入力規模を同じprofileとして比較しない。

応答時間はDBを置くmachine、storage、同時負荷、入力規模で変わる非固定値である。従って、最低動作環境と承認済みbaselineを定義するまでは、根拠のない絶対ms値、固定row数、任意の全直積をRelease拒否条件にしない。P90/P95は同じ環境・同じ入力規模に対する退行検出値として保存し、currentが履歴保持量に、history deltaが既取得prefixに比例して増大していないことを確認する。client hard timeoutは外部停止をUIへ閉じ込めるfailure-containment境界であり、serverの性能保証値ではない。timeout・欠測・安全境界超過を測定PASSへ丸めず、該当surfaceのlast-goodを保持してrecorderを継続する。

固定できる値はschema、整合性、保持期間、取得範囲、処理量の次数、failure isolationおよびOOM/DoS安全境界である。CPU時間、I/O時間、1 pageのrow数、通常時payload bytesは固定しない。最低動作環境が製品authorityとして定義された後だけ、その環境と規定datasetから絶対P90/P95のRelease閾値を導出できる。DB読出しはtimestamp/reset複合indexを使い、
3暦月を保持したDBから1暦月窓のraw候補を一度materializeし、同一分のreset aliasをcanonicalizeした公開sampleだけを44,640点以下にする。raw alias行数を44,640で打ち切ったり取得失敗にしたりせず、full table scan、保持3暦月全体の読出し、UI threadでの
行単位publishを禁止し、candidate失敗時はlast-good publicationを保持する。
ここで扱うpublic REST resourceは、`DATA_PROTECTION_POLICY.md`が別の入力境界に定義するinternal validated snapshotのOOM/DoS安全境界とは別物であり、その`1 MiB`をpublic response cap、通常件数、性能gateへ流用しない。どの安全境界もdecode後の推測値へ
置換しない。安全境界超過、malformed、unknown/case key、duplicate key、domain errorは該当resourceの
直前完全generationを保持し、部分候補を公開しない。現行 admission tupleのいずれかがstale・欠落・不一致の
candidateも同じ扱いとし、DB、memory、REST、UIを変更しない。

`state` は `initializing`、`ready`、`auth_required`、`error` のいずれかである。
`error` は接続先ではなく Codex 情報取得の失敗を表す。詳細な失敗内容は API に
公開しない。Windows クライアントは HTTP 接続エラーと `state` を別々に表示する。
認証開始・確認はcontrol-onlyであり、その応答をsnapshot fieldまたはgenerationとして採用しない。control完了後も
新しいstrict validation済みdetails generationを取得するまでlast-good表示を保持する。

account identity boundaryでは一般的なlast-good保持の例外として、新しいpublished-pairでempty rootを公開する。
明示logoutは`auth_required`、confirmed account switchからfresh collection完了までは`initializing`、
AccountKey/profile metadata/partition検証失敗は`error`とする。この3状態のrootはHTTP 200かつ
`observed_at=null`、`authenticated=false`、`plan_label=null`、`quota=null`、`models=[]`、
`active_thread_count=0`、`history_periods=[]`、`history_samples=[]`、`history_gaps=[]`、`threads=[]`、
`estimated_cost_label="概算 —"`で固定する。旧accountのquota/model/history/threadを混ぜず、同じconfirmed
accountのtransport/quota/local一時失敗だけは従来どおり最後の完全rootを保持して`error`へ遷移する。

wireに `ready` boolean keyは存在しない。dataの利用可能判定は、完全schemaを受理した一つのdetails rootについて
`state == "ready" && authenticated == true` の論理積だけである。
`state="ready",authenticated=false` と `state="auth_required",authenticated=true` はdomain不整合として
candidate generation全体をrejectし、直前の完全generationを保持する。`/v1/health`の200、process生存、listener存在、
認証開始processのexit codeだけをreadyへ読み替えない。fixture、Windows state、文書でも架空の
`ready=true` fieldを作らず、必ず上記2実在fieldを別々に記録する。

### `/v1/details` 完全schema

`/v1/details` はcontract revision `rest-v1-details-reset-at-20260823`の次のトップレベル13キーだけを持つ。
serverは一つの完全candidateからこのgenerationだけをpublishし、UI consumerはdetails一応答だけをstrict validationして
atomic commitする。wire上に`version`、
`snapshot_epoch`、`billing_period`、`thread.status`、`is_orphan`は存在しない。

| key | 型・境界 |
| --- | --- |
| `api_version` | 文字列`v1` |
| `state` | `initializing` / `ready` / `auth_required` / `error` |
| `observed_at` | `null`またはUnix秒整数`1..253402300799` |
| `authenticated` | boolean |
| `plan_label` | `null`または本節「PlanTypeから公開値への写像」のexact canonical label |
| `quota` | `null`またはstatusと同じ4必須キーのobject |
| `models` | 0..3件、`SOL`/`TERRA`/`LUNA`重複なし。各行は下記7必須キー |
| `active_thread_count` | JSON非負整数`0..UInt64.MaxValue` |
| `history_periods` | 0..128件。各行は下記6必須キー |
| `history_samples` | 0..44,640件。観測時刻までの最長1暦月。各行は下記9必須キー |
| `history_gaps` | 0..4,096件。`recorder_gap_ledger`のconfirmed rowだけを下記5必須キーへredactしたprojection |
| `threads` | 0..256件。各行は下記12必須キー |
| `estimated_cost_label` | control/bidi formattingなしの1..160 Unicode scalar。表示所有権は別途DESIGNで決め、schemaに存在するだけで重複表示を許可しない |

各model行は`name`、`input_tokens`、`cached_input_tokens`、`output_tokens`、
`input_dollars`、`cached_input_dollars`、`output_dollars`だけを持つ。tokenはJSON非負整数、
dollarは有限かつ0以上のJSON numberである。ドルはcreditや為替へ変換しない。

各history period行は`id`、`start_at`、`end_at`、`reset_at`、`label`、`current`だけを持つ。`id`は
1..512 Unicode scalarで集合内一意、時刻はUnix秒整数`1..253402300799`、
`start_at <= end_at <= reset_at`、`label`はcontrol/bidi formattingなしの1..512 Unicode scalar、
`current=true`は集合内で最大1件である。

`reset_at`はperiod groupのcanonical reset境界であり、sampleの所属判定に使う。`end_at`は現在期間では
観測時刻、途中で次期間へ切り替わった過去期間では次期間開始へclipできるため、`end_at`をcanonical
reset境界として代用してはならない。clientは`id`をparseせず、sampleの`reset_at`がperiodの
`reset_at - 60 <= sample.reset_at <= reset_at`に入るものだけを同periodへcanonicalizeする。各sampleは
exactly one periodへ所属し、そのperiodの`start_at <= timestamp <= end_at`を満たす。実reset境界の秒を
minute-startへ丸めたため旧cycle末尾と新cycle先頭が同じ分になる場合は、旧cycleがその分で終了し、
新cycleがその分から始まって後続分へ継続することを既存時系列から確認できるときだけ、新cycleが境界分を
所有する。旧cycle側の境界rowはpublic candidateへ含めずraw SQLiteには残す。それ以外でraw
`(sample.reset_at,timestamp)`が一意でもcanonicalize後の`(period.id,timestamp)`が衝突するcandidateは、
同一分に異なる残量・累積値を並べて垂直変化を作り得るため全体rejectする。merge、max、last-row、null化、
array順選択で衝突を隠さない。

public candidateを構築する前に、resident serviceの`HistoryCanonicalizer`は同一profileが所有する一つの
履歴DB readをstorage scopeとし、既存の`timestamp/reset_at`観測だけで同一と検証できたcycleと同じminute-startに
属するrowだけを一組にする。exact reset/許容内jitterと、reset前進量が観測時刻の前進量に追随するbounded rolling
chainだけを同cycleとし、group内最大`reset_at`をcanonical resetとする。新しいpartition列や永続CycleSeqを
legacy回復の前提にしない。quota観測を持たずquota確認済みcycleと時間範囲が重なるbackfill reset群と、
継続するquota確認済みcycleの時間範囲内だけに存在するreset断片はperiod authorityにせず、raw SQLiteへ残したまま
public viewから除外する。distinct non-null quotaが0または1個で、
既存のcumulative vector `(sol_dollars, terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens)`のうち
全rowをcomponentwiseに支配する既存vector値が存在する場合だけ、そのquota（0個なら`null`）とdominant vectorを
1 logical sampleとして採用する。quota競合またはdominant vector不存在・非比較となったminuteは値を選択・合成せず
そのminuteだけpublic viewへ含めない。別cycle間の所属を時系列から一意にできない場合はcandidate全体をrejectして
last-good details generationを保持する。
同値duplicateは同じvector値として冪等に扱う。
残量100%、7日窓、quota-onlyなど値の形を除外条件にせず、component別max、last-row、null化、任意mergeで
sourceにないvectorを作らない。canonicalization後もraw `(reset_at,timestamp)`とcanonical
`(period.id,timestamp)`のduplicate拒否を維持し、REST/Windows sinkで修復しない。

`label`はLinux/X側が同じperiod groupの表示に用いたreference labelであり、selection keyでもWindowsの
日時parse入力でもない。serverは`DESIGN.md`のcanonical period ID、start/end、起動時timezone、DST offset、
重複期限suffix、current suffixから一意に生成し、`label -> id`対応が1対1でない候補をpublishしない。
Windowsは選択・保持を`id`と受理array indexだけで行い、文字列をparseしない。Windows画面のperiod labelは、
同じ`start_at/end_at/current/id`を保存済み`timeZoneId`、locale、登録済みsuffix mappingで再renderする
presentation ownerとし、wire labelと異なる場合もcanonical ID、両端instant、offset、suffix roleの全てが
一対一に一致しなければならない。server reference labelをそのまま表示する実装も、選択中locale/timezoneの
mapping結果と完全一致する場合だけ許可する。

### PlanTypeから公開値への写像

`plan_label`と`quota.monthly`は任意文字列/任意booleanではなく、同じvalidated account/quota cycleの
PlanTypeからserverが生成する。WIRE上のexact enumと公開値の対応は下表だけを正本とし、`DESIGN.md`は
この表から導出する。
trim、lowercase、prefix/substring一致、schema外aliasを使わない。wireへPlanTypeやPlanFamily keyを追加せず、
次の関係だけを公開する。

| exact PlanType | canonical `plan_label` | `quota.monthly` |
| --- | --- | --- |
| `free` | `無料` | `false` |
| `go` | `Go` | `false` |
| `plus` | `Plus` | `false` |
| `pro` | `Pro` | `false` |
| `prolite` | `Pro Lite` | `false` |
| `team` | `Team` | `false` |
| `self_serve_business_prolite` / `self_serve_business_usage_based` / `business` | `Business` | `false` |
| `ent26` / `enterprise_cbp_automation` / `enterprise_cbp_usage_based` / `enterprise` | `エンタープライズ` | `true` |
| `edu` | `教育` | `false` |
| schema-valid `unknown` | `プラン未設定` | `false` |

`ready/authenticated` rootでaccount PlanTypeがある場合、`plan_label`は上表の非null値である。quotaが非nullなら
`monthly`も同じrowに一致する。空、大小文字差、schema外値、account/rate-limitのknown family不一致、
label/monthly不整合はcycle全体をrejectし、旧完全pairを保持する。Windowsは自由文字列からfamily/monthlyを
推測せず、server内部のredacted PlanType、schema hash、公開label/monthlyを同一cycle evidenceへ結合する。

### `/v2/details` model-source schema

`/v2/details`は`api_version`でversion付けした単一atomic表示rootである。
トップレベル13キー、上限、period、gap、thread、model、状態の意味はv1と同一で、`api_version`を
exact `v2`とする。`history_samples`の各rowはv1の9キーに`model_source`を加えたexact 10キーを持つ。

| `model_source` | model dollar/token 6値 | 意味と表示 |
| --- | --- | --- |
| `confirmed` | 全て非null | 同じlocal収集の証拠とatomic commitを持つ。ほかの連続条件も満たす隣接点だけ実線にできる |
| `unavailable` | 全てnull | そのtimestampのlocal model値は未取得。freshな`remaining_percent`だけは同時刻へ保持できるが、model線は確定値として描かない |
| `legacy-unknown` | 全て非null | provenance導入前またはv1 fallbackの実測累計。値は通常線にできるが、集合完全性やquota変化の帰属根拠には使わない |

`confirmed`/`legacy-unknown`でmodel 6値の一部だけがnull、または`unavailable`で一つでも非nullのcandidateは
全体rejectする。local取得失敗時に直前model vectorを新しいtimestampへ複製せず、quotaが取得できた場合だけ
その実測値を`unavailable` rowへ保存する。v1互換応答には`unavailable` rowも`model_source` fieldも含めない。
旧details clientは最初に`/v3/details`を一回要求し、exact routeの404時だけv2、さらにexact routeの404時だけv1を一回要求する。split対応clientは`/v3/current`を最初に要求し、そのexact 404時だけ同じdetails fallback列へ入る。他のstatus、schema/size/header不正、timeoutではfallbackせず該当surfaceのlast-good rootを保持する。複数versionまたはsplit resourceとdetailsの応答を比較・mergeしてはならない。

各history sample行は`timestamp`、`reset_at`、`remaining_percent`、`sol_dollars`、
`terra_dollars`、`luna_dollars`、`sol_tokens`、`terra_tokens`、`luna_tokens`だけを持つ。
`timestamp`は有効なUTC event秒を`floor(event_epoch / 60) * 60`へ変換したminute-startであり、
`(reset_at,timestamp)`は集合内一意、時刻は上記Unix秒範囲、`remaining_percent`は`null`
または有限な0..100、dollarは有限かつ0以上、tokenはJSON非負整数である。

各history gap行は`gap_id`、`reset_at`、`start_at`、`end_at`、`reason`だけを持つ。`gap_id`は
ASCII lowercase hex 32文字で集合内一意、3時刻はUnix秒整数で同じhistory period内、
`start_at<=end_at`とする。reasonは`daemon_stop_unrecoverable`、`reset_hint_expired`、
`auth_epoch_tombstoned`だけである。配列は`(reset_at,start_at,end_at,gap_id)`昇順、区間の重複・交差0。
pending/recovered/rejected ledger、raw cursor/path/process/ownerはwireへ出さない。server/client/release manifestの
details contract revisionが一致しない場合はpair全体をrejectし、旧完全pairを保持する。

各thread行は`id`、`title`、`parent_thread_id`、`model`、`model_label`、`total_tokens`、
`context_usage_tokens`、`context_window_tokens`、`created_at`、`last_user_message_at`、
`is_subagent`、`depth`だけを持つ。`id`は集合内一意の1..512 Unicode scalar、titleは
1..512、modelは1..128、model_labelは1..24 Unicode scalarで、全てcontrol/bidi
formattingを含まない。parentは`null`または1..512のID、3つのtokenは`null`またはJSON
非負整数、2つの時刻は`null`または上記Unix秒、`is_subagent`はboolean、depthは`null`
または整数0..1024である。Windowsのorphan表示は、完全に受理した同一threads集合に
`parent_thread_id`が存在しない場合だけ派生し、API fieldとして受け取らない。

`threads`配列はserver側canonical active snapshotの`updatedAt desc, id desc`順でpublishする。
`updatedAt`自体はwire fieldへ追加しない。Windowsは受理した配列indexをcanonical rankとして使い、
rootとsiblingの相対rankを保ったまま親先行depth-first・subtree-contiguousへpresentation投影する。
存在しない`updatedAt`をclientで推測したり、title・受信時刻・IDだけで別順へ再sortしたりしない。

全objectは上記キーを全て必須とし、未知、大小文字違い、同一object内の重複、型違い、
配列上限超過が1件でもあればcandidate全体を拒否する。serverの`SnapshotPublisher`は現行
`(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, AccountUpdateGeneration, CollectorEpoch, CycleSeq)`、profile publisherを所有する`SupervisorLeaseIdentity`、
`DataGeneration`、`DataHash`、canonical fingerprint、`RootHash`が一致する内部candidateだけをpublishし、
その成功publishへ一つの`Codex-Info-Published-Pair`を割り当てる。Linux / Windows UIは各surfaceが必要とするresourceまたはpage集合のstrict
schema/domain、同一の正規pair header、body/header sizeを全て満たす場合だけ、そのsurfaceを一つのrootとしてcommitする。
wireに存在しないserver内部値を推測・再計算せず、SQLite、別generation、control応答で欠落値を補わない。
取得失敗、更新競合、stale lease/epoch/cycleは架空の世代番号を補わず、DB、memory、REST、UIを変更せず
last-good generation/rootを保持して次cycleで再取得する。

## 互換移行

resident serviceはv1/v2/v3を同じloopback listenerで公開する。現行clientはv3 split resourceを優先し、全体detailsは`API-DEPRECATION-01`の互換adapterとしてだけ残す。旧adapterを将来削除してもcollector、DB、stable health、常駐監視は変更しない。詳細な接続・表示仕様は[Windowsクライアント](WINDOWS_CLIENT.md)とUX ownerを参照する。

インターネット経由の利用を将来追加する場合はloopback bindを緩めず、別の設定・認証・脅威モデルとして設計する。


## DP-REST wire authority（RC-139..142の採用値）

本節は前節のREST所有OPEN値を一意化する。data state、partition、checkpoint、generation、restore、boot、
lineage、load profileは`DATA_PROTECTION_POLICY.md` §8を正本とする。本節の追加は製品実装・実機・出荷PASSを
意味せず、状態は`REQUIREMENTS_SELECTED / PRODUCT_PENDING / HOLD`である。

### Health response

`GET /v1/health`の200 bodyはUTF-8 JSON objectでexact key集合を
`api_version,service,product_version`、値を`api_version="v1"`、`service="codex-info"`、
`product_version`をstable `X.Y.Z`形式へ固定する。unknown、missing、malformed、duplicate、
case-altered key、control/bidi、trailing non-whitespace、depth追加を拒否する。client自身と異なる有効な
product versionは診断情報として保持し、details取得を妨げない。transfer-decoded bodyは1 KiB以下である。旧2-key healthはLinux launcherが同一profileの検証済み
ownerを更新するためだけに識別し、そのserviceのdetailsは表示へ受理しない。
health 200はresident serviceがread-only snapshot requestを受理でき、schema-validなimmutable details generationを
保持しているreadinessを表す。認証済み、detailsの`state=ready`、DBの最新収集成功は意味しない。

Linux launcherのmanaged-generation判定はこの3-key wireを変更せず、health要求の前後でsystemd `MainPID`、
`/proc/<pid>/stat` starttime、executable device/inode/SHA-256、profile lock identity、port 8787のsocket
inodeと`/proc/<pid>/fd`対応が全て不変で、manifest source generationへ一致することをout-of-bandで要求する。
PID、listener、health 200、`product_version`のいずれか単独を成功へ読み替えない。known旧Codex Info ownerだけを
交代対象にし、foreign/unknown/malformed ownerはsignalせず`SAFE_BLOCKED`にする。

全JSON responseのproducer headerは次のexact意味を持つ。

- `Content-Type`は`application/json; charset=utf-8`。parameter追加、charset欠落、別charsetを生成しない。
- `Cache-Control`は`no-store`。
- fixed bodyでは`Content-Length`をUTF-8 bytesと一致させる。
- `/v1/details`、`/v2/details`、全v3 snapshot resourceの200応答とv3の304応答は`Codex-Info-Published-Pair`をexactly one持つ。値は
  ASCII `v1:`に128-bit server epochの32桁lowercase hex、続けて128-bit publish counterの
  32桁lowercase hexを置いた67 bytesだけとする。production UIはdetails headerのprefix/length/lowercase hexだけを検証し、
  epoch/counterを業務値としてparse、sort、永続化、表示せず、そのdetails応答のopaque generation identityとしてだけ扱う。
  全snapshot routeは同じpublished generationで同じpairを返す。`/health`、error、unknown/method拒否応答はこのheaderを持たない。
- response header aggregateは8 KiB以下。`Set-Cookie`、`Location`、`Content-Encoding`、
  `WWW-Authenticate`、authentication/proxy headerは0件。

clientはContent-Typeをcase-insensitive tokenとしてparseするが、media type=`application/json`かつ
唯一のparameter charset=`utf-8`を両方要求する。charsetなしを受理しない。body key順やJSON insignificant
whitespaceはidentityに使わず、parse後のexact key/value集合、型、値、配列順をfield-by-fieldで検査する。
clientはbodyのcanonical再serialization SHA、未公開のadmission tuple、`DataGeneration`、`DataHash`、
canonical fingerprint、`RootHash`を再計算または推測しない。それらはserver内部のpublisher admissionであり、
wire上では`Codex-Info-Published-Pair`がimmutable details generationの比較専用identityを所有する。

server epochはprocess起動時、listener bindより前にOS CSPRNGからexact 16 bytesを一度取得し、all-zeroなら
再試行せず起動を非0終了する。credential、profile/account値、path、admission tuple、body hashをepochへ混ぜない。
epochはprocess lifetime中不変で、単独ではwire・log・永続領域へ出さない。publish counterのownerは
`SnapshotPublisher`一つで、初期値0、最初の成功publishを1とする。完全candidateのschema/domain/admission検証後、
一つのpublisher write lock内でcounterをchecked-addし、epoch+counter tokenをpairへ設定してからrootを一度だけ
交換する。reject、cancel、read、HTTP request、同じgenerationの再応答はcounterとrootを変更しない。公開bodyが同じでも
新しい成功publishはcounterを増やす。同時publishはlock取得順に別counterを持つ。counterがu128::MAXなら旧pairを
変更せず、publisherをpermanent-failedとしてlistenerを閉じprocessを非0終了する。OS CSPRNGのcollision resistanceを
process再起動間の分離境界とし、同一process内のgeneration一意性はcounterで決定的に保証する。このheaderは外部が
bodyから再計算するcontent proofではなく、同じpublisher pairを結ぶ比較専用identityである。

server起動時はlistenerがrequestをacceptする前に、schema-validなdefault `initializing` detailsを最初の
成功publishとしてcounter=1へ構築する。従って起動直後からdetailsにはgeneration identityが存在し、最初の実data
publishはcounter=2となる。default generationのserializationまたはidentity構築に失敗した場合はbind済みsocketを公開せず
起動を非0終了し、pairなしの200応答を返さない。test用の未publish publisherはcounter=0から最初の明示publishを1として
固定vectorを検査できるが、production listenerは未publish stateを外部へ公開しない。

固定oracleは次のとおりとする。testではCSPRNG sourceを注入し、production CSPRNGを置換しない。

| server epoch (hex) | successful publish counter | exact header value |
| --- | ---: | --- |
| `00112233445566778899aabbccddeeff` | 1 | `v1:00112233445566778899aabbccddeeff00000000000000000000000000000001` |
| `00112233445566778899aabbccddeeff` | 2 | `v1:00112233445566778899aabbccddeeff00000000000000000000000000000002` |
| `00112233445566778899aabbccddeef0` | 1 | `v1:00112233445566778899aabbccddeef000000000000000000000000000000001` |

counter=1のgenerationをpublish後にinvalid candidateをrejectした場合、次のdetailsもcounter=1のexact headerを返す。
counter=1のgenerationを再度HTTP取得するだけでも同じ値を返し、counter=2へ進めない。

### Server error response

error bodyはexact key集合`api_version,error`、`api_version="v1"`、errorは次のclosed enumだけで、
transfer-decoded bodyは1 KiB以下とする。

| HTTP status | error | 適用条件 |
| ---: | --- | --- |
| 400 | `bad_request` | parse可能だがrequest line/target/header/body契約不正 |
| 404 | `not_found` | exact known pathでない |
| 405 | `method_not_allowed` | exact known pathに対するGET以外 |
| 408 | `request_timeout` | request header/body deadline超過 |
| 413 | `request_body_not_allowed` | GETにnon-zero bodyまたはTransfer-Encoding |
| 413 | `resource_too_large` | split resourceの単一rowが既存32 MiBのOOM/DoS安全境界内へ収まらない |
| 429 | `too_many_requests` | connection admission上限超過 |
| 431 | `request_headers_too_large` | header count/field/aggregate上限超過 |
| 500 | `internal_error` | response commit前のserialization/invariant failure |
| 503 | `snapshot_unavailable` | publisher/DB root/stale owner/internal read timeout、またはshutdown中 |

route/publisher faultを200や`state=error`へ丸めない。valid details generation自身の`state=error`だけはschema-valid
200 snapshotであり、server transport faultと別物である。serialization failureがheader/body commit後に起きた場合は
connectionをabortし、追加JSONやpartial-success markerを送らない。clientは全non-200、切断、長さ不一致で
該当resource candidate全体をrejectし、UI consumerはそのsurfaceの直前完全rootを保持する。

### Request resource contract

製品endpointはHTTP/1.1だけを受け、request targetはorigin-formのexact
`/health`、`/v1/health`、`/v1/details`、`/v2/details`、`/v3/details`、`/v3/current`、
`/v3/history/periods`、`/v3/history?period=<opaque>&cursor=<opaque>`、`/v3/threads`である。
historyのstrict queryを除きpercent decode、path normalization、query、fragment、absolute-form、authority-form、asterisk-formを許可しない。

| resource | 採用上限・規則 |
| --- | --- |
| request line | CRLF込み2,048 bytes以下。method tokenは8 bytes以下 |
| header count | 32以下 |
| header field | name 64 bytes以下、value 1,024 bytes以下、aggregate CRLF込み8 KiB以下 |
| Host | exactly one、productでは`127.0.0.1:8787`。duplicate/欠落/別authorityは400 |
| request body | GETは0 byte。Content-Lengthは欠落またはexact 0、Transfer-Encodingは0件 |
| active connections | listener generationあたり16以下。17件目以降は429またはparse前close |
| request per connection | 1。response後closeし、pipeline/upgrade/connectは拒否 |
| deadline | acceptからheader完了3.000秒、header完了からrequest完了1.000秒、全体3.000秒 |
| shutdown | 新規admissionを即時停止し、既存requestを最大3.000秒drain後cancel |

request header allowlistは`Host,Accept,User-Agent,Connection,Content-Length,If-None-Match`だけで、各fieldは最大1件、
`If-None-Match`はv3 snapshot resourceだけでquoted published pairを受け、他routeでは400とする。
ただしContent-Lengthは欠落可とする。`Authorization,Cookie,Proxy-Authorization,Forwarded,X-Forwarded-For,
X-Forwarded-Host,X-Forwarded-Proto,Upgrade,Expect,TE,Transfer-Encoding`は常に拒否する。obs-fold、NUL、CTL、
bare LF、invalid UTF-8を値として解釈せず400またはparse前closeにする。拒否requestはbodyを無制限drainせず
socketをcloseし、DB/published generation/settings/checkpointを0変更とする。

### Read-only effect set

全route・全statusで許可するproduct effectはrequest lifetime内heap、bounded in-memory counter、loopback socket
read/write、read-only open/statだけである。persistent log/Event Log/metric/cache/temp、registry、child process、
非loopback socket/DNS、file create/write/rename/delete/fsync、SQLite transaction、DB/WAL/SHM、backup、migration、
checkpoint、published generation mutationは禁止する。OS-managed atimeはproduct successの根拠にせず、content/inodeと
product syscall traceを検査する。同一request再入で副作用countが増えた場合はFAIL/HOLDである。

### Surface-scoped client admission

Main cycleはhealth readiness受理後にcurrentを1回取得する。Linux / Windows Mainは表示中のmodel別実行件数を確定するため、
`active_thread_count>0`のときだけ同じcycleでthreadsを1回取得し、currentと同一published pairかつ件数一致の場合だけ
一括commitする。`active_thread_count=0`ではthreads requestを行わず、同じcurrent pairのthread行を空として一括commitする。
Mainの合計と`SOL/TERRA/LUNA/ASTRA/その他`はその一つの受理済み行集合だけから導出し、合計は常にbucket和と一致させる。
threads失敗・pair不一致・件数不一致ではcurrent単独をcommitせず直前の完全表示を保持する。positive bundle失敗後は、
10秒後にcurrentを`If-None-Match`なしで1回だけ再取得する。そのbundle成功までforce pollを優先せず、Graphと独立Threads pollを
0件にし、last-good root、各surfaceのstateとerrorを保持する。bundle成功時だけretry markerとerrorを解除する。
Graphはopen/period選択時だけperiodsと選択periodの全page、Threads詳細はopen中だけ5秒周期でthreadsを取得する。各surfaceはbody全体のstrict schema/domain、size、exactly oneかつ同一の
`Codex-Info-Published-Pair` header形式を満たす場合だけ同じroot generationとして一括commitする。
timeout/non-200/切断、header欠落・重複・大小文字差・prefix/長さ/hex不正、body欠落・未知・重複key、domain不整合では
対象surfaceのcandidate全体をdiscardして直前の完全rootを保持する。route+queryのlast-goodがないclientは条件headerを送らず、
cacheなし304はrejectして同一callback内でretryせず次の通常周期に一度だけ無条件取得する。wireにJSON generation fieldを追加せず、published-generation headerは
snapshot応答のopaque generation identityとしてだけ使い、body SHAやcommon-core hashを別identityとして作らない。

Linux / Windows Mainは10秒周期、Graphのcursor差分はopen中60秒周期、Threads詳細はopen中5秒周期とする。
Graphが閉じている間は対応requestを0件とし、Threads詳細が閉じている間は上記Linux / Windows Mainの条件付き1回を除きthreads requestを0件とする。同じpairではbody 0の304を使い、current更新時も
history全体を取得しない。history cursorはperiod、最後の`(reset_at,timestamp)`、そこまでのsample canonical prefixと選択periodの完全gap集合のSHA-256 fingerprintへ結合したclient非解釈値である。serverはsnapshot構築時に累積fingerprintとkey indexを作り、requestではkeyの二分探索とfingerprint比較だけで旧cursorを検証する。現snapshotの同じsample prefixとgap集合が一致する場合だけ、旧pairで発行したcursorも受理し、cursor後のrowを現pairで返す。先頭からの完全取得では最初のpageだけが完全gap集合を持ち、後続pageは空のgap集合を持つ。delta pageもgap集合を反復せず、clientはその現pairの全pageを受理した後だけsampleを直前prefixへatomic appendして既存gap集合を保持する。過去row補正、gap追加・回復・補正、period変更、unknown、stale、malformedでは固定4xxとしてpage集合を破棄し、次の通常周期に先頭から一度取得する。これにより通常appendのrequest処理と通信はdelta量だけに比例し、prefixまたはgap変更時だけ完全再取得する。

`/v3/current`のexact 404を受けたclientは、その接続中をlegacy details modeとし、fallback列で受理した一つの完全details rootをMain、Graph、Threadsへ同時投影する。legacy modeではsplit history/threads routeを追加要求せず、Mainの10秒周期で同じdetails列だけを更新し、Graph/Threadsは最新の受理済みrootを表示する。再接続時には`/v3/current`から能力判定をやり直す。これにより旧serviceでもsurfaceを欠落させず、splitとlegacy rootを混在させない。

schema-validなdetailsの`state=auth_required|initializing|error,authenticated=false`が上記exact empty契約を満たす場合、
その同じdetails rootで旧account可視値を空にする。認証開始・確認controlの成功・失敗だけではdata rootを変更しない。
