<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->
<!-- codex-info-requirement-owner: WIRE -->
<!-- codex-info-master-ids:
RUNTIME-VERSION-481
REST-FAILURE-482
REST-129
ACCOUNT-SELECT-134
WIN-PARITY-WIRE-01
WIN-PARITY-PAIR-01
API-V3-MODELS-01
API-DEPRECATION-01
-->

# Loopback REST API v1 / v2 / v3 / v4 / v5

## API世代と廃止境界

### current v4: 取得状態とモデル集計品質の分離

`API-V3-MODELS-01`: `GET /v4/current` は `/v3/current` と同じ取得済み値に、
`models_status`（`complete` / `partial` / `unavailable`）を加え、`api_version` を `v4` とする。
`state` はrecorderがadmissionした取得・読取り・公開処理の状態を表す。REST reader/publisher自身の読取り失敗はstateを生成せず、下記のHTTP失敗境界を使う。未完のSession行や未解決の過去ログがあることだけで、
取得できた利用枠を `error` としない。集計済みの有効なモデル値はそのまま返す。

| 状況 | state | models_status | 保存済み利用枠・モデル値 |
| --- | --- | --- | --- |
| 取得成功・モデル集合が完全 | ready | complete | 最新の受理済み値 |
| 取得成功・モデル集合が部分的 | ready | partial | 最新の部分集合。以前の完全集合へ戻さない |
| 取得成功・モデル値が未取得 | ready | unavailable | 利用枠を保持。モデルは空配列 |
| 完全な空集合を観測 | ready | complete | モデルは空配列。未取得とは区別する |
| recorderが不正なcurrent入力・投影失敗を取得状態としてadmission | error | partial または unavailable | 利用枠と、有効性を確認できたモデル値を保持 |
| recorderが取得失敗・DB再読取り失敗を取得状態としてadmission | error | 保持世代の品質 | 最後の有効な値を保持 |
| REST reader/publisher自身のcommit済みdomain読取り・admission失敗 | snapshotを返さずHTTP 503 `snapshot_unavailable` | snapshotを返さない | 旧root/state/pairを保持。固定error bodyだけを返し、取得状態を新規生成しない |

未認証・初期化中・アカウント境界では既存のstrict empty契約を維持し、
`models_status=unavailable` とする。`account` selector、published pair、ETag、HTTP headerの制約は
v3 split resourceと共通であり、状態または品質の変化を古い304で隠さない。
履歴・スレッドは同じ公開世代のv3 resourceを使用する。
旧v1〜v3のschemaは変更せず、集計未完了を `error` に含める旧契約の互換経路として維持する。
client移行時はv4を優先し、exact 404の場合だけv3以下の既存経路を使用する。
独立した履歴resourceの生成失敗は、同じ世代のv4 currentをerrorへ変更する理由にしない。current自身の取得・読取り・投影の失敗と区別する。

`ACCOUNT-SELECT-134`: `GET /v3/accounts`は初期化済みaccount partitionの有限catalogを返す。各要素は
`id`（`account-<StorageEpoch>`形式の非秘密selector）、`is_current`、nullableな`activation_at`、
`deactivation_at`、`login_id`と、任意の`ownership_intervals`を持つ。各intervalのnullableな`start_at`と`end_at`は
半開区間の境界であり、field欠落時は空配列として扱う。区間の正本はDATA8.6「記録上のaccount所有区間と未確認境界」である。`start_at=null`または配列欠落/空は開始不明であり、全時刻所有・未使用・完全coverageを意味しない。`end_at=null`は記録上の未終了であり、未来の所属やprovider課金先の保証ではない。`activation_at/deactivation_at`は当該accountの最後の記録上の区間の境界を返し、区間がなければ両方nullとする。この要約二値からA→B→Aの二つのA区間を一つへ広げず、全区間を使う。認証主体とその初期化済みpartitionのidentityを確認できた場合だけ、rootの`default_account_id`が唯一の`is_current=true`要素を
指す。認証を確認してもpartitionの初期化・identity検証が未完了なら、defaultはnull・currentは0件のinitializing境界とする。ログアウト中も`default_account_id=null`かつ`is_current=true`が0件となる（保持済みaccountを履歴要素として
返してもよい）。`login_id`は前後の同一
`account/read`で確認し、owner-onlyな当該account DBへ保存した1..254 Unicode scalar、trim済み・control文字なしの
表示専用値であり、欠落時はnullとする。この値はメール形式でも同じ表示専用login IDとして返せる。メール形式を理由に伏字・別名・nullへ変換せず、値を取得できない場合にメールを生成しない。これを許すroute/fieldは本catalogの`login_id`だけであり、一般のdetailsや診断にemail fieldを追加する許可ではない。AccountScopeId、partition ID、raw AccountKey、token、pathをwireへ出さない。
`/v4/current`、`/v3/details`、`/v3/current`、`/v4/history/periods`、`/v4/history/periods/selectable`、`/v5/history/periods`、`/v5/history/periods/selectable`、`/v3/history/periods`、`/v3/history/periods/selectable`、`/v3/history`、`/v3/threads`は
任意の`account=<id>`を一つだけ受理し、省略時は現accountを選ぶ。未知・重複・不正selectorは別accountへfallbackせず
400とする。published pairとcursorはaccount IDへnamespace bindし、別accountのETag、cursor、last-goodを受理しない。
ログアウト、account切替、またはidentity検証失敗中のselectorなしrouteは、旧accountのquota・reset・model・threadを
返さず、strict emptyな`auth_required`、`initializing`、または`error` rootだけを返す。

`API-V3-MODELS-01`: v3の`current`と`history` resourceはcommit済みdomain snapshotを、有界な`models`配列として返す。model ID、token内訳、価格計算可否を事実として分離し、UI固定列や表示文言をwire fieldにしない。top-level current modelの`input_tokens`はcached入力と、存在する場合はcache write入力を含むraw入力総数であり、UI表示へのprojectionはPRODUCTの`MODEL-USAGE-DISPLAY-01`だけを参照する。モデルの数値は同じmodel keyを持つ直接観測(`model_source=confirmed`)のraw値、または保存済みの同じmodel keyをlosslessに保持する`legacy-unknown`値だけを公開する。`reconstructed-from-session`、`unknown`、`unavailable`はmodel key/sourceと欠損metadataだけを返し、モデル数値を返さない。`legacy-unknown`は集計・予測のauthorityにはせず、`G137-5`の限定条件をrun全体で満たす場合だけidle判定に利用できる。補間、hold、smoothing、予測および派生値はUI presentation-onlyで、API/DBへ書き戻さない。`/health`はAPI世代から独立したread-only readiness endpointとし、collector、DB writer、外部quota取得の生存状態を混同しない。

`MODEL-PRICING-463`の正確なモデルIDはv3のmodel keyとして保持する。currentの`estimated_cost`は共通料金表で計算できた場合だけ返し、historyの`total_dollars`は保存済みの観測時参考額を返す。保存額がないモデル版へ最新単価を遡及適用しない。旧SOL/TERRA/LUNA固定列は従来のfamily単価で集約した互換値を維持し、新しいexact-ID行の公式料金authorityにはしない。v3はfamily行を含め保存済みsidecar金額を優先し、旧family行に保存額がない場合だけ旧固定列を参照する。不完全な履歴へ旧family行を補完する場合も、同じfamilyの正確なID行が存在するなら重複した固定列合計を追加しない。画面向けfamily集約はwireの後段で行う。

v3の各履歴rowは`models`、`models_complete`、nullable boolean `task_active_since_previous`を持つ。
currentのモデル値は、選択account・公開period/window内の最新観測を先に固定し、そのexact keyの検証済み内訳から投影する。最新集合が不完全でも取得済みmodelを表示可能なまま公開し、旧complete集合へ戻さない。v1〜v3では不完全・欠測・不正なmodelがある場合に既存の`state=error`を併記する。v4では上記の取得状態と`models_status`を使い分け、数値の存在を全件収集成功へ読み替えない。最新観測が明示的な確定空集合なら空集合を採用し、未提供・不完全空集合を確定zeroにしない。履歴の保存価格や固定family補完をcurrentの数値正本にしない。回復は新しいcommitの値・状態を同じpublished pairで置換し、error時のETagが正常回復を隠さない。これは取得途中・通信失敗時のlast-good保持と区別する。
`task_active_since_previous`は同じcanonical periodの直前rowより後から当該row時刻までにtaskが実行中だったかを表し、
graphのinterval判定ではafter rowの値として扱う。
完全に索引済みのtask lifecycleで1件以上のoverlapを確認した場合だけ`true`、完全coverageで0件の場合だけ`false`、
period先頭、未索引範囲、矛盾または旧schemaは`null`とする。欠落fieldを受理する互換clientも`null`として扱い、
`false`や履歴の見た目へ推測変換しない。各model rowの`total_tokens`と`total_dollars`は個別に確認できた累計、
入力・cached入力・cache write入力・出力は確認できたfieldだけを持ち、未確認fieldを反復`null`で送らない。
旧`usage_history`のSOL/TERRA/LUNA列は、同じmodel keyの保存済み値である場合だけ`legacy-unknown`として表示投影へ渡す。異なるkeyの値をgeneric modelへmergeせず、旧schemaにない内訳や未掲載modelを0・推測値で補わない。`models_complete=true`は同じ直接観測で全モデル集合を確定できた場合だけ許可し、`model_source=confirmed`と非nullの`models`を必要とする。保持ログの一部からmodelを推測しただけで全採用資格がない場合は欠損metadataを返し、数値を生成しない。原本から資格を回復した記録分と未資格の再構成の区別はPRODUCT `API-LIFECYCLE-01`に従い、資格済みのpartial集合の既知値は保持する。clientは直接観測または保存済みlossless `legacy-unknown`の掲載modelを実線で表示し、補間・hold・予測だけをpresentation-onlyの破線とする。

Graphの観測採否、整数精度、metric別の線種、idleの全条件と限定1 slot規則はPRODUCT ownerの`G137-GRAPH-01`だけに従う。wireの`models_complete`は集合の完備性であり、掲載modelの既知metricの妥当性とは別である。task、tokenまたはUSDの一つから、他metricの数値や未使用を生成しない。Remainingの反転を形だけで不正にせず、token同値から保存USDを水平補正しない。

`API-DEPRECATION-01`: `/v1/details`、`/v2/details`、全表示情報を一体化した`/v3/details`は互換adapterである。互換期間中は同じ確認済み完全frameから生成し、既存body field・値型を変更しない。履歴の確認時刻・更新待ちは本書の追加headerで明示する。新clientは`/v4/current`とv3のhistory/threads resourceを優先し、`/v4/current`がexact 404の場合だけ`/v3/current`、さらにexact 404の場合だけ`/v3/details`、さらにexact 404の場合だけv2、v1へfallbackし、世代をmergeしない。互換routeは期限なしで維持し、`Sunset`を送らず、自動廃止を行わない。削除を契約変更として行う場合の対象は旧details route、adapter、client fallbackだけで、Session collector、SQLite writer、domain model、`/health`は対象外とする。

RESTの`api_version`はwire互換性だけのauthorityであり、製品Release版およびdaemonのrecorder-state/DB schema版から独立させる。daemon内部形式だけの変更でREST版を進めず、wireの意味またはschemaを非互換に変更するときだけ新しいREST版を追加する。同一binaryで稼働するdaemonとRESTに、独立deployを装う重複SemVerを設けない。

## 目的と境界

Linux / WSL上の常駐構成はPRODUCTの`LINUX-BUNDLE-CONTENTS-01`と`PROC-LAUNCH-01`を正本とする。installed launcherは`codex-info-recorder.service`の記録processと`codex-info-rest.service`のread-only API processを管理し、UIはその公開値を読む。REST processはquota再取得やSession再収集をせず、HTTP要求を契機にwriterを作らない。開発・互換用のraw `codex_info --port PORT`のembedded構成とは区別する。いずれのheadless構成もSlint WindowやX event loopを生成しない。
Recorderだけがquota、local usage、historyのwriterであり、source JSONLの検証とstorage admission後のtransaction commitを所有する。保存前のcanonical化はDATAの`HISTORY-CANONICAL-134`が所有する。公開adapterはcommit済みcanonical rowの資格を検証してread-onlyに投影し、HTTP側で旧重複rowの勝者選択、期間merge、値の再計算をしない。
`SnapshotPublisher`がcommit済みの有効な`DataGeneration/DataHash`とDATA owner第2章が定める現行account admission tuple、および別責務の`SupervisorLeaseIdentity`からimmutableなdetails
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
起動成功、health readiness、strict current受理は別stateとする。4-key recoveryはMain disconnected＋Settingsだけに限定する。設定と画面の動作はUX owner、設定の保存境界はDATA ownerを参照する。

| 要望 | v1での対応 |
| --- | --- |
| Linux / WSL をサーバー化する | installed launcherの`--start`でPRODUCTが定めるRecorder/REST serviceを起動する。Windowは生成しない。raw embedded commandは開発・互換用に分離する。 |
| Windowsから監視する | SSHローカルポート転送先の固定JSONを、`windows-client/` の Windows 監視クライアントが表示する。 |
| Linuxネイティブ環境を残す | installed launcherの`--ui`はmanaged Recorder/RESTへ収束した後でUIを表示する。raw embedded commandのport指定をinstalled launcherへ渡さない。 |
| イントラネットだけを対象にする | loopbackだけへ束縛し、SSHを暗号化・認証境界にする。 |
| インターネット経由は別設定にする | v1の設定や認証を再利用せず、今回の対象外として分離する。 |

## 起動と SSH トンネル

installed環境ではlauncherがPRODUCTのRelease/local generationをreconcileして、user-systemdのRecorder/REST serviceを開始する。
待受アドレスとportは`127.0.0.1:8787`に固定する。

```bash
"$HOME/.local/bin/codex-info" --start
"$HOME/.local/bin/codex-info" --ui
"$HOME/.local/bin/codex-info" --status
```

引数なし/`--start`はWindowを表示せず、managed Recorderがprofile writer lock、managed RESTがloopback listenerをそれぞれ所有する。
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

このときLinux / Windows UIは`http://127.0.0.1:8787`のversion別split resource（current v4、directory v5、history/threads v3）から、可視surfaceに必要な
current、period/history page、またはThreadsだけを取得する。各取得cycleは同じpublished pairの完全集合だけを
atomic commitし、部分pageをmergeしない。履歴差分だけは、直前rootのcursorが既取得prefix不変を証明した場合に限り、新pairの完全集合を旧prefixへatomic appendできる。新resourceがない旧serviceへの互換fallbackだけは
`/v3/details`、v2、v1をexact 404順で一応答ずつ受理する。HTTPはLinux側のloopbackと
SSH トンネルの端点の間だけで使用し、端末間の暗号化・相手認証は SSH が担当する。
そのため v1 では HTTPS 証明書を扱わない。

## リソース

HTTP field名の照合は[RFC 9110 §5.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.1)に従ってcase-insensitiveとし、大小文字だけ異なる同名fieldも重複として数える。JSON key、route、account ID、cursor、published-pairの値は別であり、本書のexact/case-sensitive条件を使う。「pairの大小文字不正」は値のprefix/hexの不正を指し、field名の綴りを独自のcase-sensitive規則にしない。JSONの文字列encodingは[RFC 8259 §7](https://www.rfc-editor.org/rfc/rfc8259.html#section-7)に従い、v5 producerの具体的なescape・field順・byte数はv5 directory節で固定する。

すべての応答は `Content-Type: application/json; charset=utf-8` と
`Cache-Control: no-store` を返し、response header aggregateは8 KiB以下とする。既知の固定bodyでは
`Content-Length`をUTF-8 body bytesと一致させ、未知長の受信側もstream中の上限超過で停止する。
`Set-Cookie`、`Location`、`Content-Encoding`、`WWW-Authenticate`、proxy/authentication headerは
返さない。response header allowlistはこの`Content-Type`/`Cache-Control`、固定body時の`Content-Length`、
snapshot resourceの`Codex-Info-Published-Pair`だけで、その他のapplication/proxy headerを追加しない。
現行clientはv3/v4/v5のsnapshot routeとqueryごとに、直前に受理したpublished pairをquoted `If-None-Match`として一つだけ送信できる。cursorの資格などを先に検証し、同じpublished generationで条件が一致する場合だけ、同じpair headerとbody 0の304を返す。v1/v2 fallbackへ条件headerを送らず、304を受理しない。新版serverのv1/v2 detailsにも下記のpair headerを付ける。旧serverのv1/v2 detailsに限りpair header欠落を互換受理できるが、一つの完全bodyだけを採用し、pairを生成・推定せず他resourceとjoinしない。存在するpair headerの不正・重複は旧版でも拒否する。
304を新しいsnapshotや失敗へ読み替えずlast-good rootを維持する。新pairの完全rootは受理後にatomic置換する。
条件付き要求でも、指定resourceの利用可否・版別表現可能性・cursor資格・件数とencoded bodyの輸送上限を同じcaptured generationで先に検証する。失敗するresourceへ304を返さず、通常要求と同じerrorを返す。この検証のためにDBやSessionを再読せず、他resourceの公開state/pairを変更しない。
recorder自身の取得失敗を含むschema-validな新generationは、正常generationと別pairで公開できる。一方、REST reader/publisherの一時的な読取り失敗は503 `snapshot_unavailable`であり、そのHTTP要求で`state=error`への書換え、新pair生成、DB由来値の補完を行わない。旧root/pairを保持し、sourceの再受理と正常な公開が成立してから応答を回復する。
許可する成功メソッドは `GET` だけである。自動解凍、redirect、cookie、proxyは使用しない。

| Request | Result |
| --- | --- |
| `GET /v1/health` | resident serviceがread-only snapshot requestを受理できるreadinessを示す。 |
| `GET /v1/details` | 旧client向けのschema-compatibleな単一atomic rootを返す。 |
| `GET /v2/details` | 旧client向けのprovenance付き単一atomic rootを返す。 |
| `GET /v3/details` | split resource非対応client向けの任意model単一atomic rootを返す。 |
| `GET /v3/current` | Mainに必要な状態、利用枠、model累計、active thread countだけを返す。 |
| `GET /v3/history/periods` | Graphの期間metadataだけを返す。 |
| `GET /v3/history/periods/selectable` | reset期間選択用にPRODUCTの掲載条件を満たす期間metadataを返す。 |
| `GET /v3/history?period=<opaque>&cursor=<opaque>` | 選択期間の履歴または`cursor`より後の差分を返す。`cursor`は初回だけ省略できる。 |
| `GET /v3/threads` | Threads surfaceを開いたときだけthread rowsを返す。 |

history成功bodyはexact 5 keys `api_version`、`history_samples`、`history_gaps`、`next_cursor`、`resume_cursor`を持つ。`next_cursor`は同じ取得cycleに後続pageがある場合だけ非nullで、その値を次requestへ使う。`resume_cursor`は応答の最後のsampleまでを証明し、最終pageでも非nullである。cursor付き要求に新sampleがない場合は受理したcursorを`resume_cursor`へ返し、初回要求にsampleがない場合だけnullとする。clientは`next_cursor=null`まで同じpairを集めた後、最後の`resume_cursor`を次回差分用に保存する。

未定義のパスは JSON の `404`、既知パスへの非 `GET` は JSON の `405` で返す。
応答に認証 URL、認証トークン、raw error、ローカルパス、セッション内容を含めない。
accountの表示識別値は`ACCOUNT-SELECT-134`のcatalogに限定し、その`login_id`以外の応答へemailを含めない。

path照合はcase-sensitiveかつ完全一致であり、URL decode/normalizationを行わない。case-altered path、
末尾slash追加、未登録query付きpath、未定義prefixはunknown pathとして扱う。ACCOUNT-SELECT-134が列挙するrouteの`account`、v3 historyの必須`period`と任意`cursor`、v5 directoryの任意`cursor`だけを各1件許可する。account selector不正は400、history/cursor不正は対応する400、それ以外の未登録queryは404とする。重複・空・percent encoding・fragmentを拒否する。拒否bodyは`api_version="v1"`と固定error codeだけを持つ
JSON objectとし、未知キー・raw error・秘密値を含めない。404/405を含む全responseは
SQLite transaction、WAL/SHM、migration、prune、backup、DB row/hash、published generationを変更しない。

### Response・read-only matrix

| request | method | result | response / side effect |
| --- | --- | --- | --- |
| `/v1/runtime` | `GET` | `200` | 稼働version metadata、共通JSON/no-store header、snapshot pairなし、DB mutation=0 |
| `/v1/health` | `GET` | `200` | JSON health object、required `Content-Type`/`Cache-Control` headers、DB write/transaction=0 |
| `/v1/details` | `GET` | `200` | current immutable details generation、共通headerに加えて必須`Codex-Info-Published-Pair`、DB write/transaction=0 |
| `/v2/details` | `GET` | `200` | 同じcurrent immutable generationのprovenance付きprojection、同じ必須published pair、DB write/transaction=0 |
| `/v3/details` | `GET` | `200` | 同じcurrent immutable generationの任意model projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/current` と `/v4/current` | `GET` | `200` | history/period/thread rowsを含まないversion別current projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/accounts` | `GET` | `200` | ACCOUNT-SELECT-134のcatalog、global frame自身の必須pair、認証主体と履歴partitionを区別、DB mutation=0 |
| `/v3/history/periods` | `GET` | `200` | period metadata projection、同じ必須published pair、DB write/transaction=0 |
| `/v3/history/periods/selectable` | `GET` | `200` | 選択対象period metadata projection、同じ必須published pair、DB write/transaction=0 |
| `/v4/history/periods` | `GET` | `200` | 公式起点metadata付きperiod directory、同じ必須published pair、DB write/transaction=0 |
| `/v4/history/periods/selectable` | `GET` | `200` | 同じ公式起点metadata付き選択対象directory、DB write/transaction=0 |
| `/v5/history/periods` と `/selectable`（任意`account/cursor`） | `GET` | `200` | byte境界の有限directory page、同じ必須published pair、DB write/transaction=0 |
| `/v3/history?period=...&cursor=...` | `GET` | `200` | 同一pair・選択periodの有限page、DB write/transaction=0 |
| `/v3/threads` | `GET` | `200` | thread projection、同じ必須published pair、DB write/transaction=0 |
| v3/v4/v5 snapshot resource＋route/queryのcurrent pair | `GET` | `304` | body 0、同じpublished pair、route-local last-good維持、DB write/transaction=0 |
| 上記known path | `HEAD/POST/PUT/PATCH/DELETE/OPTIONS`等全non-GET | `405` | 固定JSON error、同上header、DB/WAL/SHM/migration/prune/backup=0 |
| unknown、case-altered、末尾slash、未登録query付きpath（methodを問わない） | any | `404` | 固定JSON error、同上header、DB/WAL/SHM/migration/prune/backup=0 |

RESTのtransfer body上限はsnapshot `32 MiB`、health/error `1 KiB`、response header `8 KiB`とする。health/errorの値は後述の完全schemaの上限と同一であり、別の4 KiB規則を併用しない。この値を通常データ件数の妥当性判定へ使わない。historyは次の正当なrowを加えると32 MiBを超える場合、そのrowの直前でpageを完了してopaque cursorを返し、次pageへ必ず前進する。history sampleの最大1,024点境界も適用する。必須envelopeを含む単一rowの応答が安全境界を超える場合だけ`resource_too_large`とし、recorder、DB、公開generationを変更しない。
### v3/v4 split bodyの完全schema（API-V3-MODELS-01）

共通のstate、時刻、認証、plan、quotaの型はv1 schemaを参照する。版による差は次表の通りで、tableにないtop-level keyを追加しない。

| resource | exact keys |
| --- | --- |
| v3 current | `api_version,state,observed_at,authenticated,plan_label,quota,models,active_thread_count,open_session_thread_count` |
| v4 current | v3 currentの9キーに`models_status`を追加 |
| v3 details | v3 currentの9キーに`history_periods,history_samples,history_gaps,threads`を追加。`estimated_cost_label`は持たない |
| v3 threads | `api_version,threads` |
| v3 history | `api_version,history_samples,history_gaps,next_cursor,resume_cursor` |

`api_version`はrouteに対応する`v3`または`v4`。modelはexact keyで一意、最大1,024件。各current modelのexact 7キーは`model,total_tokens,input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,estimated_cost`である。model keyはcontrol/bidiなしの1..128 Unicode scalar、tokenはUInt64範囲のJSON非負整数で、cache writeだけ必須nullable整数とする。`input_tokens`はcached/cache writeを含み、`total_tokens=input_tokens+output_tokens`、`cached_input_tokens<=input_tokens`、cache write既知なら`cached_input_tokens+cache_write_input_tokens<=input_tokens`をchecked演算で検証する。未提供と明示0を混同しない。要求されるraw成分がないmodelは同current schemaの数値行として表現できないため行を省き、v4の品質をpartial（他の有効行あり）/unavailable（有効行なし）とする。known totalだけがある履歴は次のhistory形で保持し、currentの必須値を0で埋めない。

`estimated_cost`は必須nullable object。非null時のexact 6キーは`price_version,ordinary_input_dollars,cached_input_dollars,cache_write_input_dollars,output_dollars,total_dollars`。price revisionはPRODUCTのcatalog identity、5金額はfinite・非負で同じrevisionの4成分の和をtotalとする。cache writeまたは必要価格が未知なら全objectをnullにし、一部既知価格だけのtotalを出さない。値の意味・四捨五入はPRODUCTの料金/表示契約を参照し、wire上で表示桁へ丸めない。

v3 history observationはexact 7キー`timestamp,reset_at,remaining_percent,task_active_since_previous,models,models_complete,model_source`。時刻とremainingの型は既存historyと同じ。taskは必須nullable boolean（旧client互換の欠落受理だけnull）。modelsはnullまたは最大1,024件の配列で、空の確定集合は`[]`としてnullと分ける。各modelは必須`model,total_tokens`に、既知の場合だけ`input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,total_dollars`を付ける。未知componentは省略し、繰返しnullを付けない。既知の内訳には上記token整合条件とDATA `RECORDER-MODEL-01`の非負整数による補完可能性の有限条件を適用し、未知componentから残りを逆算しない。sourceとmodels_completeの資格は本書冒頭の規則を参照する。

数値を公開する資格がない`reconstructed-from-session`、`unknown`、`unavailable`は、v3 observationでは`models=null,models_complete=false`とsourceを返す。内部のmodel keyと未処理情報は保持するが、必須total_tokensのないmodel-only objectや架空のtotal=0をwireへ足さない。confirmed/legacy-unknownの有効な既知totalはoptional内訳が不明でもhistoryへ保持する。明示空集合と不完全空集合のprovenanceはDATA ownerの集合保持規則に従い、nullと空配列を相互変換しない。

v3 threadは既存12キーに`activity_status`を加えた13キーで、型とrunning/open membershipは`THREAD-OPEN-362`の既存契約に従う。strict emptyは各resourceに実在するfieldだけへ適用し、split bodyへdetails-only fieldを付けない。数字0は取得済みの完全集合の0を表し、error応答の未確認placeholderを画面の確定0として採用しない。

### リセットの継続と互換性（REST-129 / #604）

異なる公式resetの採否はPRODUCT `LIVE-RESET-129`、原本・保持窓はDATA `QUOTA-DURABLE-604`を正本とする。period総数128件という制約を撤回する。これは公式上限でも、保存可能件数でもない。正常な129件目以降を不正データにしたり、currentのquota・有効models・threadsを空にしたりしない。

互換detailsのcanonical sample窓は`(T-31*86,400,T]`（TはDATAの公開基準時刻）とし、retainedの先頭minute包含とは別の互換窓である。sampleの一意keyは全版で`(reset_at,timestamp)`とし、異なるexact resetの同minuteは両方を保持する。同じexact keyに公式receiptがあれば取得順が最後のreceiptをquotaに用いる。receiptがない保存quotaはDATA 8.7の明示的な公式取得provenanceを検証できる場合だけ公開し、cached Session snapshot由来または取得元不明ならRemainingをNULLとする。DB保存値を変更せず、同じkeyのmodel資格も変更しない。modelも同じexact keyだけを結合する。receiptのない別resetのmodel観測を落としたり、別resetのquota代表へ移したりしない。互換detailsのsample順は既存wireの`(reset_at,timestamp)`昇順を維持する。これは既存consumerの順序検証に対する互換制約であり、期間を跨いだ全時刻の単調順を要求しない。period指定historyは同じresetだけを含むためtimestamp昇順となる。44,640 sampleと32 MiB encoded bodyは旧consumerの輸送境界であり、minuteごとの保存数・period総数の上限ではない。この互換resourceが境界を超えれば413 `resource_too_large`とし、先頭だけの200や同minuteの別resetを捨てた200を禁止する。互換detailsはこの直近31日の一体型projection、v3/v4 directoryは一体型metadataのままとする。旧bodyに`next_cursor`を追加しない。旧directoryも全対象を32 MiB内で返せる場合だけ200、超える場合は同じ413とする。この互換adapterの限界はdomain、DB、独立v4 current、新directory、period指定のv3 historyの正常性を変えない。旧clientの128件parser制約をserverの正しい仕様とは扱わない。

取得directoryの掲載条件は、同じaccount・保持窓にある観測点またはconfirmed gapを持つperiod、未解消の旧利用記録の保持資格を持つperiod、および現在periodである。元観測範囲を保持したmetadataから同じdirectory項目を構成し、origin不明を推測で補わない。selectableの掲載条件は、現在period、同じ窓の公開済みmodelに正の`total_tokens`があるperiod、またはPRODUCTで定義した未解消の旧利用記録を持つperiodである。最後の条件はmetadataによる掲載保持であり、model数値の公開資格を緩めない。source品質・集合の完備だけで保存済み正値を落とさず、旧利用記録の原本照合不足だけで選択肢を消さない。この保持対象を除き、初めからquotaだけ・gapだけ・model未取得の過去periodをselectableに載せないことは未使用の証明ではない。全exact IDは保持窓のindexで保持し、非掲載の既知IDへの直接history取得を未知IDへ変更しない。保持窓外の履歴は再公開せず、既知IDの取得結果は保持窓内の空pageとなる。

v3 directoryのtop-levelは従来の`api_version`、`history_periods`の2キー、各periodは既存の6キーである。v4は同じ2キーと、各periodに必須nullable整数`origin_at`を加えた7キーである。公式起点は同じread transactionのexact reset/window、またはDATAが受理したaccount所有内の原本証拠からだけ求める。競合・根拠欠落ではnull。初回保存時刻から起点を生成しない。`origin_at <= start_at`を要求する。

全版でproducerのperiod `id`はexactな正整数`reset_at`のcanonical ASCII十進表現とする。符号・先頭0・日付文字列・timezone・label・取得順suffixを含めない。同一accountの同じresetは同じID、異なるresetは別IDであり、account namespaceはselectorとpublished pairが所有する。clientはIDをparseせず再送する。v3/v4一体型directoryの配列順は`(start_at,reset_at,id)`の降順（idはUTF-8 unsigned byte辞書順）を維持する。v5の昇順は次節だけに適用し、UIの選択順をwire順の違いで変えない。

### v5 directory: byte境界による有限page（REST-129 / #604）

v5を追加する理由はdirectoryの分割に必要なwire非互換変更である。current、threads、history sampleのschemaを変える理由にはしない。

| route | 許可query | 200のexact top-level keys |
| --- | --- | --- |
| `/v5/history/periods` | 任意の`account`、任意の`cursor` | `api_version`, `history_periods`, `next_cursor` |
| `/v5/history/periods/selectable` | 任意の`account`、任意の`cursor` | 同上 |

`api_version`は`v5`。各periodはv4と同じ7キー、`next_cursor`は必須で非空ASCII文字列またはnullとする。掲載条件は上記と同じで、両directoryは別resourceである。

1. 同じimmutable published snapshotの対象集合を`(reset_at,id)`昇順（idはUTF-8 unsigned byte列の辞書順）に並べる。IDとresetは集合内で一意。currentは全page合計で最大1件。空集合も同じschemaで配列空、cursor nullを返す。
2. 開始offsetは初回0。offset以降の連続した最大prefixのうち、compact JSONのUTF-8符号化後のbody全体が32 MiB以下に収まるpageを返す。JSON escaping、全field、実際の次cursorもbytesに含める。文字はJSONで必須のescapeだけを行い、それ以外はUTF-8のまま、整数はcanonical十進で符号化する。field順はtop-levelが表の順、periodが`id,start_at,end_at,reset_at,label,current,origin_at`の順とする。固定128行を転送条件にしない。残りがあれば次offset、なければnull。単一行すら入らない場合だけ413 `resource_too_large`。非終端の空pageを返さない。
3. cursorはserver発行の`d1.<pairのv1:以後64桁lowerhex>.<aまたはs>.<offset>`。`a`は取得directory、`s`はselectable。offsetは正canonical十進整数で、対象集合の長さ未満。clientは内容を解釈・生成せずそのまま再送する。serverはaccount namespace、現pair、resource種、offsetをETag判定より先に検証する。HTTP target/query自体の構文拒否は上記route照合規則を使う。構文上受理したqueryのcursor値が不正、旧pair、別account、別resourceなら400 `stale_cursor`。bodyは既存error schemaの`api_version="v1"`, `error="stale_cursor"`だけで、published-pair headerなし。
4. 通常の新publishで旧pairのdirectory cursorは失効する。過去snapshotをHTTP要求のために保持したりDB transactionを延長したりしない。取得途中の世代変更では当該Graph candidate全体を破棄し、last-goodと取得失敗表示を保持する。同callbackで反復せず、次の通常Graph周期または利用者の明示再取得で先頭から取得する。正常なcurrent更新を止めない。既存v3 history cursorのprefix証明による差分取得とは別規則である。
5. clientは全directory pageの同じaccount/pair、順序、一意性、cursor非反復、非終端での行数前進、終端nullを検証する。続いて選択periodまたは時間窓に必要なv3 history pageを同じpairで全部受理し、一括commitする。最後のpageまで届かない部分一覧は成功表示しない。無関係なMain/Threadsのpairとの同時一致は要求しない。
6. 新clientは該当するv5 routeを優先する。そのrouteのexact404だけv4、さらにexact404だけ対応v3へfallbackする。selectableのv3もexact404なら旧取得directoryを使う既存互換動作を維持する。503、timeout、不正body、未知cursorではfallbackしない。route/version/account/選択種/queryごとにcacheを分ける。v5を受理後、同じ取得cycleの後続pageだけ旧versionへ変えない。
7. published-pair header、strict schema、body/header limit、GET-only、read-only effectは既存snapshotと同じ。cursor付き要求も通常のETagを使用できるが、cursorの検証を省略しない。cacheなし304は既存規則で拒否する。1回のHTTP要求のdeadlineは取得cycle全体の性能保証ではない。

### 保持・取得範囲・固定値の区別（REST-129 / #604）

| 値・条件 | authority / 適用scope | 到達・超過時の結果 |
| --- | --- | --- |
| 異なる公式resetは1秒差でも別ID | PRODUCT `LIVE-RESET-129` | 取得順で受理。日付・残量回復・累積件数で拒否しない |
| 3 UTC暦月 | DATAの履歴保持窓とcurrent modelの観測資格 | 公式windowの長さで延長しない。公式origin/resetのmetadataや取得順で決まる現在quotaを切り詰めず、原本・receipt/idempotence記録の保存期間と混同しない |
| 31日 / 44,640 sample | 互換detailsの時間窓 / 旧応答の件数上限 | 同minuteの別resetも1 sampleずつ数える。domain読取り・全期間・全calendar-month・split history合計の上限ではない |
| directory 128件 | 根拠のない旧総数上限 | 廃止。新たな固定128行/pageにも転用しない |
| 32 MiB/body、8 KiB/header | 既存のpublic輸送安全境界 | encoded bytesをproducer/consumer双方で検査。分割可能な一覧はpage化し、他resourceを空にしない |
| 1,024 history sample/page | 既存の1 request処理量境界 | 最大数であって固定長ではない。byte境界でさらに短くなる。全pageへ継続する |
| models 1,024/row、threads 256、gaps 4,096 | 既存のpublic schema安全境界 | 明示的な該当resource取得失敗。公式の上限とは表明せず、0件の成功へ変えない |
| idleの10分、1 slot 60秒 | PRODUCT `G137-5`の表示方針 | providerの無使用保証ではない。条件未成立なら帯を作らない |
| request deadline・接続数 | 本書Request resource contractの収容境界 | 切断/拒否をdata不正とせず、該当surfaceのlast-goodを保持 |

v3 historyはexact periodの全保持観測を最大1,024点かつ32 MiB以下のpageで返す。全page合計44,640点という制約、private retained indexの3×44,640という制約は撤回する。保持内の実際のcanonical key数と時間範囲で有限化し、raw alias行数で打ち切らない。互換detailsの`history_periods`とsamplesは直近31日projectionであり、全保持directoryを同bodyに入れない。旧v1/v2の固定3 modelとv3以後のmodel安全境界は別仕様として維持する。

互換detailsの判定順は、まず版ごとの表現可能性、次に応答件数・encoded bytesとする。v1で固定6列のunknown等を表現できない観測が含まれる場合は503 `history_unavailable`、表現可能だが44,640 sampleを超える場合はv1/v2/v3 detailsだけ413 `resource_too_large`とする。v2/v3はnullable表現を保ったまま同じ件数制限を適用する。domainの投影検証は、件数制限以外の型・値域・順序・exact keyの一意性・期間への所属を全対象で検証し、件数を減らしたりchunk単位の検証で全体の重複や順序を見逃したりしない。

暦月・暦日の定義はPRODUCT `G137-GRAPH-01` / UXに従う。timezoneの遷移規則により1日は常に86,400秒とは限らず、31日月も44,640 minute以下とは限らない。clientは選択暦月へUTC→表示timezoneで写る全観測を取得・分類し、32 MiB/page以外の都合で末尾を落とさない。公式window長を7日または31日へ丸めない。

### 容量条件とfailure isolation


応答時間はDBを置くmachine、storage、同時負荷、入力規模で変わる非固定値である。本仕様は通常応答の絶対msを保証しない。検証するのは本書のdeadlineによる失敗隔離、indexed range/pageごとの有限処理、UI応答性、正確な結果であり、根拠のない固定row数や任意の全直積をRelease拒否条件にしない。client hard timeoutは外部停止をUIへ閉じ込めるfailure-containment境界であり、serverの性能保証値ではない。timeout・欠測・安全境界超過を成功扱いせず、該当surfaceのlast-goodを保持してrecorderを継続する。

固定できる値はschema、整合性、保持期間、取得範囲、処理量の次数、failure isolationおよびOOM/DoS安全境界である。CPU時間、I/O時間、1 pageのrow数、通常時payload bytesは固定しない。DB読出しはtimestamp/reset複合indexを使い、
保持窓内のraw候補を必要なindexed rangeごとに一度materializeし、DATAが定めるcanonical keyへ投影する。互換detailsだけは31日の時間窓を適用する。raw alias行数やcalendar-monthのminute数を固定44,640で打ち切らず、requestごとのfull table scan、同じrangeの重複読出し、UI threadでの行単位publishを禁止する。candidate失敗時はlast-good publicationを保持する。
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

`quota.reset_at`はcurrent account partition・AuthEpochで最後に受理したquota観測のprovider `resetsAt` Unix秒で、
current periodの識別子でもある。同じ日時は同一periodにまとめ、より新しい観測で異なる日時が届けば新periodとして
記録する。`quota.window_seconds`は同じ観測と同じtransactionで保存し、応答順序は直列pollerの永続取得順序で保持する（DATA owner `QUOTA-DURABLE-604`）。ローカル`observed_at`やdeadlineの大小で公式応答を拒否しない。別account/epochの値は公開しない。current `history_periods[]`は同じquota観測のwindowから`start_at = quota.reset_at - quota.window_seconds`、PRODUCT `CURRENT-PERIOD-BOUNDS-129`が定める公開データ観測時刻から
`end_at = min(quota.reset_at, observed_at)`を一度だけ投影し、Linux/Windowsの期間欄、selected
period start、グラフ横軸はそのmetadataを使う。Mainのリセットまでの区間は公式`start_at,reset_at`であり、記録済み区間の終端とは別の意味である。`observed_at`をquota receiptの時刻だけへ戻して後続の有効model点を除外しない。period `id`、canonical `reset_at`、durable sample、model totalsは
期間識別子が同じ間だけ維持し、新periodより前のsampleは保存したままcurrent公開から除外する。completed/historical periodのmetadataはPRODUCTの`CURRENT-PERIOD-BOUNDS-129`を参照し、リセット時刻は同じrootの`quota.reset_at`を表示する。
OS時計と公式windowの不一致などでperiodの時刻domainを構成できない場合も、そのmetadataを架空の時刻へclampしない。受理済みquotaは保持し、履歴resourceを明示的に利用不能とする。
履歴projectionの失敗は現在quotaを拒否する理由にしない。受理済み現在quotaを`/v3/current`のHTTP 200・`state:error`で公開し、生成できない履歴resourceはHTTP 503・`history_unavailable`を返す。履歴は空の正常結果へ置き換えない。
履歴sample/gapの所属は正確な`reset_at`の一致を優先する。近接した公式resetの期間が存在するとき、60秒以内という理由で別期間へ結合したり、複数所属として応答全体を拒否しない。完全一致する期間がない旧wireのaliasだけに従来の許容幅を適用する。
履歴全体の時間資格は公開基準Tと各exact periodのmetadataで検証する。currentの公開`observed_at`を別periodの履歴の上限に使わない。
ローカル時計の逆行等で過去periodの記録時刻がcurrent観測より後でも、T/保持窓/所有権とそのperiod自身の範囲を満たす点は保持する。
その点でcurrentの観測時刻・範囲を延長したり、時刻順だけでcurrentを選び直したりしない。

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
| `observed_at` | `null`またはUnix秒整数`1..253402300799`。PRODUCT `CURRENT-PERIOD-BOUNDS-129`の公開データ観測時刻。quota receiptの原時刻・取得順序は別の保存authorityであり、この値から推測しない |
| `authenticated` | boolean |
| `plan_label` | `null`または本節「PlanTypeから公開値への写像」のexact canonical label |
| `quota` | `null`またはstatusと同じ4必須キーのobject |
| `models` | 0..3件、`SOL`/`TERRA`/`LUNA`重複なし。各行は下記7必須キー |
| `active_thread_count` | JSON非負整数`0..UInt64.MaxValue` |
| `history_periods` | 互換31日projectionに関係するexact periodと現在period。全保持directoryとは別。件数だけで正常データを拒否しない。各行は下記6必須キー |
| `history_samples` | 0..44,640件。公開基準時刻までの直近31日。各行は下記9必須キー |
| `history_gaps` | 0..4,096件。`recorder_gap_ledger`のconfirmed rowだけを下記5必須キーへredactしたprojection |
| `threads` | 0..256件。各行は下記12必須キー |
| `estimated_cost_label` | control/bidi formattingなしの1..160 Unicode scalar。v1/v2互換表示用。配置・重複禁止はUX Main、金額の根拠はPRODUCTを参照し、labelをparseして数値正本にしない |

各model行は`name`、`input_tokens`、`cached_input_tokens`、`output_tokens`、
`input_dollars`、`cached_input_dollars`、`output_dollars`だけを持つ。これらの数値はPRODUCTの旧v1/v2固定family projectionに従い、
直接観測またはlosslessな旧保存値から確認できた同じfamilyの既知memberだけを集約する。凍結対応外のmodelを推測して含めない。
tokenはJSON非負整数、dollarは有限かつ0以上のJSON numberである。ドルはcreditや為替へ変換しない。
固定schemaのv1ではsource metadataを表現できないため、未検証・再構成推測のmodel行を数値付きで出さず、欠損metadataを表現できる
v2/v3 projectionを使用する。正確なmodel keyの元値を保持し、互換projectionをDBやv3へ書き戻さない。

固定6値の欠損は版ごとに次の結果とする。v1のhistory sampleは6値を全て必要とし、保持対象のsampleに不明・表現不能の固定値があれば、そのv1 details応答だけを`503 / history_unavailable`とする。0を埋めたり、そのsampleを除外した正常な空履歴を返したりしない。v2 historyは6値を一組としてしか表現できないため、そのsampleの6値を全てnull、`model_source=unavailable`としてquotaと時刻を保持する。このv2 sourceは同版で数値vectorを表現できないことを意味し、canonicalのsource品質・取得状態・v3の既知model値を変更しない。ここでの欠損は同じkeyのcanonical固定値自体の未知・表現不能を指す。保存固定値が全て既知でも、元の`reconstructed-from-session`または`unavailable`が数値公開資格を持たないためにmetadata-onlyとなる場合は、従来のsource名と6値nullを保持し、このadapter欠損へ再分類しない。v3 historyは同じkeyの既知totalと任意成分・保存USDをそれぞれ独立して保持する。欠損がない旧保存sampleの値/sourceは変更しない。currentのv1/v2 model行も必要な全token/ドルbucketを表現できる行だけを掲載し、部分成分を0や部分額へ補わない。旧版での表現不能だけを理由にv4 current、v5 directory、v3 history/threadsを失敗させず、DBの保存・collection commitも止めない。

新しい固定family projectionは、集合が不完備でも既知memberから確認した記録分を保持できるが、全利用量の証明とはしない。明示した未知成分・未知価格のmemberを0扱いして部分額を確定値にしない。元のexact model keyと異なるfamily aliasへの集約を含むv2 history sampleは、6値が全て表現可能なら`legacy-unknown`とし、直接の同じkeyの観測である`confirmed`へ昇格させない。6値に欠損があれば上記の`unavailable`を使う。v1の同じ互換値はsource fieldを追加せず、v1 fallbackの表示限定・legacy-unknownとして扱う。欠損のない旧保存sampleの既存sourceを再分類する規則ではない。familyの表示合計の数値域・精度・未知寄与はPRODUCT `G137-GRAPH-01`の整数計算・独立metric規則に従い、固定INTEGER列やv1/v2のUInt64上限で表示をclampしない。

各history period行は`id`、`start_at`、`end_at`、`reset_at`、`label`、`current`だけを持つ。`id`は
1..512 Unicode scalarで集合内一意、時刻はUnix秒整数`1..253402300799`、
`start_at <= end_at <= reset_at`、`label`はcontrol/bidi formattingなしの1..512 Unicode scalar、
`current=true`は集合内で最大1件である。

`reset_at`はperiod識別子であり、sampleの所属判定では完全一致を優先する。`end_at`はPRODUCTの期間境界規則による観測終端であり、別periodの開始時刻ではclipせず、resetの代用にしない。公式quotaの
受理済み記録と期間はDATA owner `QUOTA-DURABLE-604`、Session由来modelのcanonical所属は
`HISTORY-CANONICAL-134`に従う。clientは`id`をparseせず、公開された所属を変更しない。

sampleの`timestamp`は観測を含む分の開始である。所属期間の開始が秒単位でも、
`floor(start_at / 60) * 60 <= timestamp <= end_at`を満たす最初の分を受理する。
期間の`start_at`自体は丸めない。異なる公式IDの表示範囲の重なりを不正としない。同じ分にresetが変わった場合も
各exact resetのsampleを保持し、同じexact reset/minuteのquotaだけを取得順序で選ぶ。取得原本と受理済み記録は全観測を保持する。
modelは同じexact reset/minuteの観測だけを結合し、別期間のmodelを移さない。
Linux/Windowsとも分の観測を開始時刻より前という理由で捨てず、描画は選択期間内へclipする。

旧混合行の正規化はDATA owner `HISTORY-CANONICAL-134`へ従う。原本で証明できない異なるresetをrolling/jitterの形だけで統合せず、HTTP要求・clientで再canonicalizeしない。公式receiptの別exact resetは優先される。元のraw key、source資格、採用元、局所除外またはrollbackはDATAで一意に決め、WIREはその公開結果だけを運ぶ。

旧wireのsample所属互換はDBのmigrationとは別である。まずexact reset一致を使う。一致するperiodが一つもない旧sampleだけは、`abs(sample.reset_at-period.reset_at)<=60`かつそのperiodの公開時間範囲内となる候補がexactly oneの場合に限りlegacy所属として受理できる。60秒は旧minute単位のwire互換許容であり、公式reset同一性の保証ではない。raw resetは保持し、period ID同士の統合、sourceのconfirmed化、DBへの再所属を行わない。候補0件/複数またはexact一致先のdomain不成立はrejectする。現行producerが受理済み公式periodを省略するためには使わない。

`label`は互換用のreference文字列であり、selection keyでもclientの日時parse入力でもない。互換referenceの時刻書式は固定`+09:00`のproleptic Gregorian `yyyy/MM/dd HH:mm:ss +09:00`（ASCII数字、年は最小4桁）とする。これは表示timezoneや実instantのauthorityではない。`start_at ～ right`を作り、rightはcurrentならreset_at、過去ならend_at、currentにだけ`（現在）`を付ける。同じbase labelが複数ある場合は各行へ`（期限 reset_atの同書式）`を付けて区別する。exact resetが異なるため秒とoffsetを含むsuffixで一意になる。非ownerのDESIGN.mdへID生成を委譲しない。
clientの表示は受理したid/start/end/reset/currentをUXとI18Nに従ってactive timezone/localeへ変換する。referenceの固定offsetを利用者設定へ逆流させず、文字列の一致でperiodを同一視しない。wire配列と表示並べ替えの間はIDで対応付ける。

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

この`monthly`は互換用に凍結したplan表示分類であり、providerが報告した期間長・暦月開始の証拠ではない。trueでもresetから1暦月を減算せず、falseでも7日を代入しない。有限quotaの期間長とoriginの資格はPRODUCTの完全window選択とDATAを使い、UIの実際の期間名・時間barもその`window_seconds`から得る。`monthly`の値型・上表の対応は互換のため維持し、別の意味の期間計算へ流用しない。Graphの暦月表示の選択にもこのflagを使わない。

### `/v2/details` model-source schema

`/v2/details`は`api_version`でversion付けした単一atomic表示rootである。
トップレベル13キー、上限、period、gap、thread、model、状態の意味はv1と同一で、`api_version`を
exact `v2`とする。`history_samples`の各rowはv1の9キーに`model_source`を加えたexact 10キーを持つ。

| `model_source` | model dollar/token 6値 | 意味と表示 |
| --- | --- | --- |
| `confirmed` | 全て非null | 同じmodel keyの直接観測とatomic commitを持つ。集計・idle判定の根拠になれる |
| `reconstructed-from-session` | 全てnull | sessionからの復元であり、model key/sourceと欠損metadataだけを返す。モデル線・集計・idle判定には使わない |
| `unknown` | 全てnull | 観測不明。model key/sourceと欠損metadataだけを返し、数値を推測しない |
| `unavailable` | 全てnull | そのtimestampのlocal model値は未取得。freshな`remaining_percent`だけを保持でき、model数値は返さない。現行quota windowの先頭から最初のlocally-owned observationまでの未帰属provider観測もこのsourceで公開する |
| `legacy-unknown` | 全て非null | provenance導入前またはv1 fallbackの保存済み同じmodel keyの値。集計・予測には使わず、`G137-5`の同一非空model集合・lossless token／Remaining完全不変・欠測／異常／gapなしをrun全体で満たす場合だけidle判定に利用できる |

`confirmed`/`legacy-unknown`でmodel 6値の一部だけがnull、または`reconstructed-from-session`/`unknown`/`unavailable`で
一つでも非nullのcandidateは全体rejectする。local取得失敗時に直前model vectorを新しいtimestampへ複製せず、quotaが取得できた場合だけ
その実測値を`unavailable` rowへ保存する。v1互換応答には欠損modelの数値を含めず、`model_source`を持たない場合も欠損metadataだけを維持する。
旧details clientは最初に`/v3/details`を一回要求し、exact routeの404時だけv2、さらにexact routeの404時だけv1を一回要求する。split対応clientは`/v4/current`を最初に要求し、そのexact404だけ`/v3/current`、さらにexact404だけ同じdetails fallback列へ入る。他のstatus、schema/size/header不正、timeoutではfallbackせず該当surfaceのlast-good rootを保持する。複数versionまたはsplit resourceとdetailsの応答を比較・mergeしてはならない。

各history sample行は`timestamp`、`reset_at`、`remaining_percent`、`sol_dollars`、
`terra_dollars`、`luna_dollars`、`sol_tokens`、`terra_tokens`、`luna_tokens`だけを持つ。
`timestamp`は有効なUTC event秒を`floor(event_epoch / 60) * 60`へ変換したminute-startであり、
`(reset_at,timestamp)`は集合内一意、時刻は上記Unix秒範囲、`remaining_percent`は`null`
または有限な0..100、dollarは有限かつ0以上、tokenはJSON非負整数である。

各history gap行は`gap_id`、`reset_at`、`start_at`、`end_at`、`reason`だけを持つ。`gap_id`は
ASCII lowercase hex 32文字で集合内一意、3時刻はUnix秒整数で同じhistory period内、
`start_at<=end_at`とする。reasonは`daemon_stop_unrecoverable`、`reset_hint_expired`、
`auth_epoch_tombstoned`だけである。配列は`(reset_at,start_at,end_at,gap_id)`昇順、区間の重複・交差0。
pending/recovered/rejected ledger、raw cursor/path/process/ownerはwireへ出さない。gapの部分復元とmetric別表示coverageはPRODUCTを参照する。互換性は要求routeの版・strict schema・domain・published pairで判断し、別product versionや非公開manifest値の違いだけで有効なpairを拒否しない。

各thread行は`id`、`title`、`parent_thread_id`、`model`、`model_label`、`total_tokens`、
`context_usage_tokens`、`context_window_tokens`、`created_at`、`last_user_message_at`、
`is_subagent`、`depth`だけを持つ。`id`は集合内一意の1..512 Unicode scalar、titleは
1..512、modelは1..128、model_labelは1..24 Unicode scalarで、全てcontrol/bidi
formattingを含まない。parentは`null`または1..512のID、3つのtokenは`null`またはJSON
非負整数、2つの時刻は`null`または上記Unix秒、`is_subagent`はboolean、depthは`null`
または整数0..1024である。Windowsのorphan表示は、完全に受理した同一threads集合に
`parent_thread_id`が存在しない場合だけ派生し、API fieldとして受け取らない。

v3の`/v3/current`と`/v3/details`には、`THREAD-OPEN-362`で公開対象となった現在openのcanonical Session thread ID数を示す非負整数`open_session_thread_count`を追加する。v3の`/v3/threads`と`/v3/details`の各thread行には必須の`activity_status`=`running|stopped|unknown`を追加する。`running`は動作中、`stopped`は停止中、`unknown`は状態未観測であり、`active_thread_count`は`running`行数だけを表す。`open_session_thread_count`は全v3行数で、同一IDの重複を許さない。v1/v2は従来の12キー行と`active_thread_count`を維持し、停止中・未観測行を含めない。旧形式の保存済みactive行だけは当時のrunning-only writer契約に従い動作中と解釈する。

公開thread集合の内部に実在するID同士のparent参照は、自己参照を含め循環を許さない。producerは完全候補の全行でこのdomainを検証し、consumerも表示filterより前の元行集合で検証する。循環があればThreads候補全体を拒否し、Mainのpositive bundleであれば同bundleも受理しない。直前の完全表示と失敗状態を保持し、cycleの一部を削除して正常な0件へ変更したり、親を付け替えてtreeを作ったりしない。集合外のmissing parentとnull parentはこの循環条件とは区別し、`THREAD-OPEN-362`の所属filterへ渡す。DATAが定めるnative DBのcycle/dangling拒否もこれによって緩和しない。

同一pairのv3 consumerは、受信した元行集合で`open_session_thread_count`と`active_thread_count`を先に照合する。停止中の親とその子孫が元行に含まれた場合も、Main/Threadsの画面表示前に親自身と全子孫を除外し、表示件数を残る行から導出する。この画面側の防御はserverが公開対象のみをpublishする義務やwire件数の意味を変更しない。残る行の`id`・`parent_thread_id`を付け替えない。

`threads`配列はserver側canonical open-session snapshotの`updatedAt desc, id desc`順でpublishする。
`updatedAt`自体はwire fieldへ追加しない。Windowsは受理した配列indexをcanonical rankとして使い、
rootとsiblingの相対rankを保ったまま親先行depth-first・subtree-contiguousへpresentation投影する。
存在しない`updatedAt`をclientで推測したり、title・受信時刻・IDだけで別順へ再sortしたりしない。

### Threads producerの適応取得周期（Issue #435）

recorderの通常Threads probeは設定済みcollection周期（既定60秒、環境変数
`CODEX_INFO_DAEMON_INTERVAL_SECS`または優先する`--interval-secs`）に従う。
Session収集とquotaの通常周期を変えず、同じaccount・epoch・thread IDの完全な受入済み
snapshot間で、両方が既知の`total_tokens`に不一致があるとThreads probeだけを5秒周期へ切り替える。
増加と減少を同じ差分として扱い、`null`を0へ変換しない。初回、再起動、identity変更、および
未知値からの初観測は比較baselineを作り、差分を捏造しない。

probeは一つだけを進行させ、実行中に次の要求をqueueせず、遅れた周期のまとめ実行もしない。
失敗・不完全candidate・epoch不一致で前回の受入値を置換せず、失敗を無変化の成功とみなさない。
最後の有効差分から60秒以上経過し、同じidentity集合の既知値を比較できる成功probeで差分が
なければ通常周期へ戻す。連続する完全な空snapshotも無変化とする。未知値しかない結果は
無変化の根拠にしない。新しい差分があれば60秒の安定待ちを再開する。

この適応取得は`REST-172` / Issue #362のprobe/RPC回数を増やさない通常規則への明示的な例外である。
5秒モードではcandidate scan、rollout読取り、短命app-serverの起動、initialize・account/read・
thread/readの回数が増える。Sessionの全走査やquota収集周期、完全snapshot・account/epoch検証、
published pairおよびerror/last-good契約は変えない。最初の差分検知は通常周期まで待ち得る。
5秒はprobe開始の目標周期であり、probe処理、公開、Mainの10秒取得および詳細の5秒取得と
同一pair受理の待ち時間を含む画面の5秒鮮度保証ではない。UI周期は後述のsurface契約に従う。

全objectは上記キーを全て必須とし、未知、大小文字違い、同一object内の重複、型違い、
配列上限超過が1件でもあればcandidate全体を拒否する。serverの`SnapshotPublisher`は現行
DATA owner第2章のaccount admission tuple、profile publisherを所有する`SupervisorLeaseIdentity`、
`DataGeneration`、`DataHash`、canonical fingerprint、`RootHash`が一致する内部candidateだけをpublishし、
その成功publishへ一つの`Codex-Info-Published-Pair`を割り当てる。Linux / Windows UIは各surfaceが必要とするresourceまたはpage集合のstrict
schema/domain、同一の正規pair header、body/header sizeを全て満たす場合だけ、そのsurfaceを一つのrootとしてcommitする。
wireに存在しないserver内部値を推測・再計算せず、SQLite、別generation、control応答で欠落値を補わない。
取得失敗、更新競合、stale lease/epoch/cycleは架空の世代番号を補わず、DB、memory、REST、UIを変更せず
last-good generation/rootを保持して次cycleで再取得する。

## 互換移行

resident serviceはv1/v2/v3/v4/v5を同じloopback listenerで公開する。現行clientはcurrent v4、directory v5、history/threads v3をresource別のexact404規則で取得し、全体detailsは`API-DEPRECATION-01`の互換adapterとしてだけ残す。旧adapterを将来削除してもcollector、DB、stable health、常駐監視は変更しない。詳細な接続・表示仕様は[Windowsクライアント](WINDOWS_CLIENT.md)とUX ownerを参照する。

インターネット経由の利用を将来追加する場合はloopback bindを緩めず、別の設定・認証・脅威モデルとして設計する。


## DP-REST wire authority（RC-139..142の採用値）

本節はRESTの採用値を定義する。data state、partition、checkpoint、generation、restore、boot、lineage、load profileは`DATA_PROTECTION_POLICY.md` §8を正本とする。本節の規範と、製品実装・実機・出荷の品質確認結果を区別する。工程のstatus文字列を入力採否の条件にしない。

### Health response

`GET /v1/health`の200 bodyはUTF-8 JSON objectでexact key集合を
`api_version,service,product_version`、値を`api_version="v1"`、`service="codex-info"`、
`product_version`のcanonical stable/beta値はPRODUCT ownerの`VER-BETA-BUILD-CORE-467`に従う。unknown、missing、malformed、duplicate、
case-altered key、control/bidi、trailing non-whitespace、depth追加を拒否する。client自身と異なる有効な
product versionは診断情報として保持し、details取得を妨げない。transfer-decoded bodyは1 KiB以下である。旧2-key healthはLinux launcherが同一profileの検証済み
ownerを更新するためだけに識別し、そのserviceのdetailsは表示へ受理しない。
health 200はresident serviceがread-only snapshot requestを受理でき、schema-validなimmutable details generationを
保持しているreadinessを表す。認証済み、detailsの`state=ready`、DBの最新収集成功は意味しない。

Linux launcherのmanaged-generation判定はこの3-key wireを変更しない。PRODUCTのmanaged Recorder/REST役割ごとにMainPID/starttime/executable identityを照合し、profile lockはRecorder、listener socket/FDはRESTへ結合して、同じmanifest source generationをout-of-bandで検証する。healthだけから記録processの生存・leaseを推測しない。
PID、listener、health 200、`product_version`のいずれか単独を成功へ読み替えない。known旧Codex Info ownerだけを
交代対象にし、foreign/unknown/malformed ownerはsignalせず`SAFE_BLOCKED`にする。

全JSON responseのproducer headerは次のexact意味を持つ。

- `Content-Type`は`application/json; charset=utf-8`。parameter追加、charset欠落、別charsetを生成しない。
- `Cache-Control`は`no-store`。
- fixed bodyでは`Content-Length`をUTF-8 bytesと一致させる。
- `/v1/details`、`/v2/details`、全v3/v4/v5 snapshot resourceの200応答と304応答は`Codex-Info-Published-Pair`をexactly one持つ。値は
  ASCII `v1:`に128-bit server epochの32桁lowercase hex、続けて128-bit publish counterの
  32桁lowercase hexを置いた67 bytesだけとする。production UIはdetails headerのprefix/length/lowercase hexだけを検証し、
  epoch/counterを業務値としてparse、sort、永続化、表示せず、そのdetails応答のopaque generation identityとしてだけ扱う。
  同じ確認済みframeからのsnapshot routeは同じpairを返す。現在値/Threadsと、directory/履歴/互換detailsは独立したframeを持てる。global catalog/boundary frameはstandalone節の別scopeとして扱う。`/health`、error、unknown/method拒否応答はこのheaderを持たない。
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
交換する。reject、cancel、read、HTTP request、同じ公開domainの再確認は公開root/pair/counterを変更しない。
成功publishとは公開domainまたは認証・accountの境界を交換する操作であり、その操作ごとにcounterを増やす。
境界ではbodyが偶然同じでも新しいpairを使う。境界でない保存世代・T・内部照合材料だけの更新は成功publishに数えない。
同時publishはlock取得順に別counterを持つ。次の単一publish、または一つの境界としてatomic交換する複数frameに必要なcounter数をchecked確保できない場合は、旧root/pair/counterを
変更せず、publisherをpermanent-failedとしてlistenerを閉じprocessを非0終了する。OS CSPRNGのcollision resistanceを
process再起動間の分離境界とし、同一process内のgeneration一意性はcounterで決定的に保証する。このheaderは外部が
bodyから再計算するcontent proofではなく、同じpublisher pairを結ぶ比較専用identityである。

server起動時はlistenerがrequestをacceptする前に、schema-validなdefault `initializing` detailsを最初の
成功publishとしてcounter=1へ構築する。従って起動直後からdefault initializing応答にはgeneration identityが存在し、次の成功
publishはcounter=2となる。account/catalogごとの成功publishと境界は同じ系列を使い、最初のaccount dataのcounterを別に固定しない。default generationのserializationまたはidentity構築に失敗した場合はbind済みsocketを公開せず
起動を非0終了し、pairなしの200応答を返さない。test用の未publish publisherはcounter=0から最初の明示publishを1として
固定vectorを検査できるが、production listenerは未publish stateを外部へ公開しない。

### Standalone reader publicationとHTTP処理の境界

上記のepoch/counter ownerはlistener全体で一つとする。公開scopeは次表とし、同じ確認済みframeのroute/version/pageは元pairを返し、projectionだけではcounterを進めない。

| immutable frameのscope | 内容・requestとの結合 |
| --- | --- |
| global routing/catalog frame | 検証済みcatalog、default account selector、defaultの境界状態。`/v3/accounts`はこのframeだけからbody/pairを取得する。selector省略のdata routeはここで現accountを解決し、account frameを読む。現accountを確定できない境界中は、このframeのstrict emptyな境界応答を使う |
| account current frame | 一つの初期化済みpartitionの確認済みcurrent/Threadsとaccount ID。明示selectorと、確定済みdefault selectorのcurrent/Threads routeに使う |
| account history frame | 同じpartitionで最後に確認済みの完全なperiod履歴projectionと、同じtransactionの互換details。directoryの全page、選択期間の全history page、互換detailsはこの元frameのpairを返す。更新中のcurrent frameとpairが異なってよい。新しいcurrentのfieldやpairを旧履歴へ転記しない |

period directory、period history、および互換detailsの200/304応答は、確認済み履歴がある場合に`Codex-Info-History-State`と`Codex-Info-History-Verified-At`を一つずつ持つ。stateは`verified`または`refreshing`、確認時刻はその履歴projectionが固定したUTC Unix秒Tである。`verified`は最新の受理済みcurrent commitと同じcommitで、current以上の公開窓Tを検証したことを表す。`refreshing`はその後のcommitまたはTの履歴が未確認であり、数値の完備・未使用・現在時刻までのcoverageを意味しない。本文のperiod/current flag、元時刻、値、gap、cursorはその履歴frameの確認時点のまま返す。元pairが履歴revisionを識別する。確認時刻とstateは保存値ではなく応答時の可用性metadataであり、stateだけの変化は本文pairやETagを変更しない。304でも現在のmetadataを返す。旧clientが追加headerを解釈しなくても値やschemaを補完しない。

Graphのbundleは同じaccountのdirectoryとhistory pageだけを同一pairで受理し、Main currentのpairへjoinしない。MainのcurrentとThreadsの同一pair条件は維持する。account変更時は両方のbundleを破棄する。新しい履歴の検証まで旧履歴を最新値と呼ばず、更新待ちを表示する。新periodのcurrentが先行しても、履歴frameへ新periodや0の点を合成しない。初回から確認済み履歴がない場合は従来の`history_unavailable`であり、空の成功にしない。

frameを実際に交換するときにだけ、一つのcounter系列から新tokenを割り当てる。異なるaccountまたはglobal scopeのframeへ同じpairを再利用しない。global catalogは選択accountのdata bundleへjoinせず、default accountを選択中でもpair一致を要求しない。catalog応答の受理だけではclientのdata pair/cursorを置換しない。HTTP要求中に可変registryを読み、captured frameと異なるcatalogを合成しない。

auth/account境界cycleでは、DATAの確認を通した新catalogとdefault境界を同じglobal frameへatomicに反映する。AからBへ切り替わった後にAを`is_current=true`のまま返さない。Bのpartitionがまだ初期化・検証されていない間は`default_account_id=null`、`is_current=true`は0件とし、default dataは`initializing`とする。初期化済みBのidentityを確認後はBを唯一のcurrentとして掲載し、fresh quotaのcommitまではBのcurrentをstrict initializingに保つ。保持済みの別accountを選択中の場合は、その選択と有効frameをdefault変更だけで破棄しない。catalog更新のためのprocess再起動を要求しない。registry/identity検証に失敗した場合はDATAのhard boundaryと下記可用性規則へ従い、旧catalogのcurrentを確認済みとして返さない。

standalone RESTのDB読取り・canonical candidate構築・publishはHTTP handlerの外のreader workerだけが行う。次の入力でだけ一回のreconciliation cycleを開始し、同じcycleの同じpartitionを重複読取りしない。

| 入力 | 読取り・公開の対象 |
| --- | --- |
| process起動 | default initializing rootをcounter=1で公開してから、registryと初期化済みの全account partitionを一回照合する。未検証catalogのrouteは503とし、空配列の成功へ変えない。各成功candidateは独立したaccount frameとしてatomic公開する |
| DATAのstrict recorder-stateで新しい有効なcommit/試行結果を受理 | 同じaccount/partitionのmarkerを照合し、変化した有効domainだけを再構成する。通知が同じcommitを反復しても二度publishしない |
| `REST-172`の60秒鮮度cycle | 全初期化済みpartitionのmarkerとDATAの公開基準時刻Tを照合し、保持窓・公開domainが変わるcandidateだけを再構成する。新しいTは候補全体で一つに固定する |
| 明示的な取得controlの完了 | 完了そのものはreaderへの独立したpublish入力にしない。結果として新しいstrict recorder試行tupleを受理した場合だけ上記の通知cycleを使う。control成功だけではdata frameを公開しない |
| auth/account/owner境界 | DATA/PRODUCTに従ってglobal routing/catalogと必要なaccount current境界を更新し、旧ownerの未完了candidateを破棄する。新旧accountを混合しない。検証済みの歴史account読出しをdefault ownerの変更だけで禁止しない |

commit/試行結果の同一性は、DATAが受理したrecorder-stateの`pid,process_starttime,owner_nonce,partition_id_hash,data_generation,collector_epoch,cycle_seq,write_state`のtupleとする。heartbeatの`updated_at_unix`だけの変化や、その同じtupleの反復ではDB照合・JSON生成・counter更新をしない。recorder-stateの観測には既存owner監視の1秒周期を使い、この1秒はread-onlyの状態file/生存確認だけの間隔であり、SQLite poll、collector周期、公式quotaの時刻・期間長へ流用しない。状態file不正・失効・owner不一致は同じtupleと見なして抑制せず、DATAの境界/失敗規則を適用する。起動時と60秒cycleにより、歴史accountの起動時復元や通常収集外の有効なDB変更もHTTP要求なしで照合する。

`ready`と`degraded`の完全commit組は同じDB commit照合を通す。`degraded`には履歴pendingを含む成功commitも存在するため、
write stateだけをv4 currentの`error`へ写さない。完全組のgeneration/collector epoch/cycle sequenceは同じread transactionで取得した
DBの最後のcommit組と全て一致しなければならない。DBの先行commitとstate fileの更新間は未確認候補として保持し、
一致しない新候補を公開しない。`degraded`のcommit組全体null、不正・失効したstateは同ownerのsource可用性を停止する。
old frame/pairは保持し、そのaccountのsnapshot routeを503にする。明示した歴史accountはそのpartitionのidentityと
registry所有条件を確認して独立に読める。nullや`degraded`からDB generation、quota値、Error frameを補完しない。

reconciliationは処理中に来た通知をpartitionごとの最新tuple一つと全体のfreshness pending一つへまとめる。通知列や60秒tickの数だけworkをqueueせず、取りこぼした周期を後でまとめて再実行しない。cycle開始時にその時点のpending集合を取り出し、Tをその構築開始時刻に一度固定する（DATAの公開窓契約）。待機中の通知時刻や前cycleのTを使わない。freshness pendingがあれば全初期化済みpartition、なければ通知のあったpartitionだけを照合する。両入力が同じpartitionを指定してもそのcycleでは一回である。partition順は非秘密account selectorのUTF-8 unsigned byte昇順とする。

current/Threadsと既存period履歴は、それぞれ同一partitionの一つのread transactionで資格を得る。currentは履歴を待たず公開できる。補助読取りは全accountで一つだけ進行させ、accountごとの未実行要求は最新一つにまとめる。新しい補助結果は同じpartition・所有資格・受理済みの完全commit組を照合してから公開し、既に公開した別resourceの値・公開窓を古い候補で戻さない。period履歴は読取り開始時に捕捉したackと同じcommitの候補を、同ownerの最新currentが先行していても独立history frameへ公開できる。履歴の世代と確認時刻Tは既公開history以上とし、同一世代の保存値・共通quota・既観測値の不変を確認する。Current/Threadsのframe・pair・可用性はこの公開で置換しない。現在値が先行する間の確認済み履歴は上記の独立history frameとして保持する。別accountまたは補助読取りの遅延・失敗は、確認済みcurrent/Threads/period履歴を失効させる理由にしない。

読取り中に新tupleや境界を受理したpartitionでは、current/Threadsの旧candidateをpublishせず、次のpendingへ最新tupleだけを残す。period履歴は上記の元ack・同owner・非後退条件を満たす候補を独立公開し、新tupleの履歴要求も最新一つだけ保持する。所有境界または元ack不一致による旧候補の取消しをreader障害や取得状態errorへ変換しない。境界でidentity/ownerの資格を失ったcandidateも破棄する。独立した歴史accountのcandidateは、そのaccountのpartition identityとregistry所有条件が同じなら有効である。読取り後のadmissionでは捕捉したscope/identity/markerが依然有効なことを再確認し、失効した値をcounter更新より先に拒否する。marker整合は同一read transaction内のgenerationとdomainの一致、およびworkerが受理した所有境界によって失効していないことで判定する。period履歴以外では最新tuple一致も必須とする。HTTP要求やheartbeat tickから追加SQLを行う規則ではない。DB commitとstrict state更新の間でまだ一致を確認できない候補は成功にもreader障害にもせず旧frameを保持し、新tupleまたは次の通常freshness入力を待つ。同じ旧tupleを直ちに再読取りしてloopしない。次cycleはpendingがあれば直ちに開始し、その開始時点で新たなTを固定する。通常の60秒deadlineはmonotonicな固定周期を維持し、通知や処理終了から毎回60秒へ延期しない。

marker不変かつ公開窓不変なら前回frameを再利用し、再読取りを成功publishとして数えない。schema/domain/identity不正、実際のI/O timeout、取消しでは新candidateを公開せず旧root/pairを保持する。可用性もframe参照と同じcapture単位で取得し、次表に従う。単にworker処理が長いことを新たなdata異常・空結果・根拠のない通常応答時間保証へ変換しない。

ここで再投影の不変条件を結び付けるのは、同じpartitionの`data_generation`が識別する保存domainである。
recorder-stateとDBを照合する完全なack tuple（`data_generation,collector_epoch,cycle_seq`）とは区別する。
新しいcollection cycleのcommitは、usage値が不変でもDATAの規則に従い`data_generation`を進める。
同一cycleの再送は完全tupleを保持する。独立したquota receipt等のcommitは世代を進め、collectionのepoch/seqは保持する。
旧新の完全tupleをそれぞれ対応するDB/stateで検証する。同じ`data_generation`でcollectionのepoch/seqだけが
変わった入力を正常なno-op collectionとして受理しない。保存世代の前進だけでは、新しいpairを発行する根拠にならない。

accountの公開domain不変は、同じaccount/認証境界内で次の全てが一致することで判定する。
配列の順序、null/未知、UInt64の全桁、保存USDを含む実値を比較し、DB世代を含むhashの一致だけで代用しない。

| 比較対象 | 不変であるべき観測結果 |
| --- | --- |
| current/details/threads | 対応する全versionの応答可用性と、成功時の公開fieldまたは失敗時の固定error。legacy thread filter後の件数・状態、v3の全thread集合と件数、v4の独立current state/models_status、取得不良によるstateの変更を含む。v1だけの`history_unavailable`も同じ版の観測結果として比較する |
| history可用性 | history/directoryの成功または`history_unavailable`の区別 |
| directory | all/selectableのそれぞれについて、掲載行の順序とID/start/end/reset/label/current/origin。v5の並べ替え・page境界はこの同じ行列と固定輸送規則から導出する |
| period指定history | direct lookupで既知とする全period IDの集合、各IDへ公開される順序付きsampleとgap、各sample prefixのresume cursorの資格。directoryに未掲載でも既知の空periodとunknown periodを区別する |

DB generation、collector tuple、raw receipt identity、内部indexのarray位置、照合材料、cacheは公開fieldではない。Tは履歴の確認時刻headerだけへ公開し、保存値や本文pairの同値性へ混ぜない。
ただし内部の変更が上表の観測結果を変えれば公開domain変更であり、保存原本の資格検証も省略しない。
どのrouteにも到達しない内部rowや未掲載periodの非公開metadataの差だけでpairを増やさない。
公開domainが同じ再確認では、最新の検証済み保存世代・tuple・T・内部照合材料を更新し、pairと派生page cacheを再利用する。
旧公開domainを表すpairを、新しい公開値や別accountへ使い回すことは禁止する。

同じ`data_generation`のT変更で保持窓・時間資格が変わる場合は、DB generationを捏造せず同じ保存domainから再投影できる。
同generationの任意の内容変更を許す規則ではない。旧新の保持minute窓の共通部分では、exact reset/minute/model key、
raw成分とknownness、保存USD、Remaining、集合完備・source品質をfield-by-fieldで一致させ、共通部分の点の追加・欠落・値変更を拒否する。
gapは同じgap ID/reset/reasonとし、旧新windowの共通部分へclipした区間を一致させる。quota receiptの取得順identity/元値、
ownership、plan、acquisition状態、thread集合等の時間窓によらない保存事実も同じでなければならない。
current latest modelの変化は、元観測時刻と同exact resetを内部に保持してTによる新規資格/失効を証明できる場合だけ許し、
同じ元観測の値・価格版の書換えを許さない。新規資格を持つ点は旧windowの外だったことを確認し、
現在TでのDB/所有権/domain検証を通常通り通す。証明できない変化はreader不良として503・旧root/pair保持とし、
Tの変化を理由に受理しない。この比較材料はheap内の読取り証拠であり、DB/REST/clientへ新たな正本やhash fieldを追加しない。
同じmodel観測keyの再確認では、model集合・値だけでなく集合品質も一致させる。既知のmodel観測が消える場合は、
その元観測が新しいTの資格範囲から外れたことを証明し、モデル値を空・品質をunavailableにする。
同じ資格範囲内の欠落・不正を、時間失効として受理しない。canonical minuteの資格はDATAの境界minute規則を使う。
公開fieldが変わらない場合は同じpairを再利用する。公開fieldが正当に変わった場合だけ別pairでatomic交換する。
history/modelの独立した資格が前回は利用不能で今回は検証できた場合も、同じ`data_generation`の再確認候補を構成できる。
既に確認済みの同じsource key/成分/元値を変更せず、新たに検証できたdomainだけを加える。以前利用不能だったことを根拠に
値を補完しない。投影資格・公開fieldが変われば別pair、単なるreader可用性の回復で公開domain不変なら旧pairを使う。
reader/source不良または未完了laneが残る場合は、marker不変を理由に次の通常入力での回復照合を省略しない。

この再投影証明と独立laneの失敗隔離は次の順で適用する。current quota/model・threadの元事実を同じ`data_generation`内で
検証できる一方、history固有の検証だけが失敗した場合は、そのcurrentを維持できる。historyは前述の確認済みframeを`refreshing`で返し、確認済みframeがない場合にだけ`history_unavailable`とする。historyの空配列を新しい確認済み履歴として扱わない。最後に検証できた同じ`data_generation`の履歴について、
そのT、既知の観測とgap、元値をheap内の照合材料として保持し、取得不能の間に既知事実を忘れない。
その後のhistory回復は、この最後の検証済み窓との共通部分で上記の一致を確認する。同じ`data_generation`でまだ検証済み履歴が
なければ、新たに資格を得たhistoryを通常のDB/所有権/domain条件で検証し、既に確認済みのcurrent等は変更しない。
より新しい`data_generation`を正式受理しても、同じaccount・partition・所有区間・recorder owner/epochに属する最後の確認済み履歴frameと照合材料を保持する。同じ完全tupleの再送やTの更新だけでも退役させない。新しい履歴candidateは最新の完全commit組と通常のDB/domain条件で独立検証し、同generationの回復では最後に確認した同generationの事実と照合する。旧generationの履歴を新generationの事実と比較・合成しない。
HTTPは確認済み履歴の元frame/pairと`refreshing`を返せる。新generationの未確認点を成功応答へ含める規則ではない。identity/所有区間/owner/epochの境界では旧履歴を退役させ、新たな資格が得られるまで再利用しない。未完了candidateの取消し、ack tuple照合、source不正時の503は従来どおり維持する。

Tが変わってlatest current modelの選択が変わる場合も、前回選ばれた元観測の成分・保存価格版を今回と同じread
transactionで照合する。前回の元観測が新しい公開窓で非適格になっても、この照合を省略しない。
current等の既知事実を書き換えた候補、またはその不変を証明できない候補はaccount reader不良として旧frame/pairを
保持し503にする。公開domainが同じでも、この元事実の不一致をhash一致で見逃さない。新しいack tupleやTの照合材料
だけを更新し公開domainが変わらなければpairとその派生page cacheを再利用し、成功publishとしてcounterを増やさない。

| 状況 | 応答と保持 |
| --- | --- |
| 明示指定accountのframeをまだ構築できない、または所有資格/domain/sourceの検証拒否 | そのaccountのsnapshot routeを503 `snapshot_unavailable`。別accountや検証済みglobal catalogは維持し、default initializingを明示accountの値へ代用しない |
| history固有の投影失敗または更新待ちだけを有効なdomainとして隔離できる | 同じ所有資格の確認済みhistory frameがあれば元pair・確認時刻・`refreshing`で返す。なければhistory/directoryは503 `history_unavailable`。v4 currentと独立したthreadsを維持する。互換detailsは確認済み完全frameだけを使う |
| global catalog自体が未検証・読取り不能 | `/v3/accounts`は503 `snapshot_unavailable`。旧catalogを新しい確認済み一覧として返さない。identityを維持できる明示accountの読出しは独立に継続する |
| DATAが確認したlogout/account/identity hard boundary | 同ownerの旧default可視値を消すstrict boundary frameを公開する。単なる同accountのDB再読取り失敗とは区別する。境界に必要なglobal/account frameの交換は一つのatomic visibility transitionとする |
| 同じ所有資格の確認済みframeを保持できる通常のDB I/O失敗 | 既知のquota/model/履歴を保持したHTTP 200・degraded表示。同じframeでは全snapshot resourceが正常pairと別の同じdegraded pairを使う。DBやrecorder generation、正常pairは変更しない |
| 通常のreader回復 | 同じ有効domain・公開窓なら通常pairへ戻す。degraded ETagにはHTTP 200を返し、通常pairと同じETagにだけ304を返す。回復だけを理由にDB generationやcounterを進めない |
| recorderのschema-validなError generationを新たに受理 | その完全candidateを別pairで公開できる。reader/HTTPの失敗から生成しない |

health/runtimeは上記snapshot不良へ巻き込まない。HTTP handlerはcaptured immutable frameとpublisherが固定した可用性だけを読み、要求中にmarker、SQLite、Session、canonicalizationへ戻らない。保持可能なread failureの表示とdegraded pairはこの捕捉済みhealthから投影し、要求内のDB読取り成否から生成しない。lazy history/directory pageは同じframeからheap内だけで生成・再利用する。serialize失敗は500、確認済みframeなし・所有資格/source拒否は503とし、拒否・timeout・page生成によってroot/counterを更新しない。

shutdownは新cycleとrequest admissionを止め、未実行pendingを破棄し、進行中candidateをpublish前に取り消す。既存requestは下記の最大3秒drainを使い、current readerをjoinする。補助readerの長いread-only transactionは終了時に閉じ、process終了をその完了待ちにしない。取消し後に遅れて終了したreadからのpublishは0件である。counter exhaustionと初期epoch/default root構築失敗の終了条件は上記から変更しない。複数frameを一つの境界として交換する場合も必要counter数を先にchecked検証し、途中のframeだけを公開しない。

current reader workerの予期しない終了・panicはaccount単独のread失敗ではなく、公開を継続できないprocess障害である。
旧root/pair/counterを変更せずpublisherをpermanent-failedにし、新cycleとrequestの受理を止め、listenerを閉じて
processを非0終了する。PRODUCTのservice restart規則に復旧を任せ、同じprocessで別workerを起動して黙って続行しない。
このprocess終了は、通常のsnapshot不良からhealth/runtimeを独立させる規則の例外である。
補助readerだけの終了は固定診断を残し、未確認の補助結果を成功へ変更せず、独立した公開を継続する。
OSのUTC時刻を有効範囲のUnix秒として得られないcycleは、新しいTを捏造せず各対象accountの読取り失敗として扱い、
旧root/pair保持・503とする。次の通常入力で有効な時刻を得られれば通常のreader回復規則を使う。

次のscope oracleでは、一つの注入epochの下、global initializing、account A、account B、更新global catalogの順に有効なpublishを明示実行する。非同期workerの完了時間をcounter値の根拠にしない。

| 操作 | 必須の結果 |
| --- | --- |
| 上記4回の成功publish | global initializing=1、A=2、B=3、catalog=4。epochは同じ、4 tokenは全て別 |
| Aのv4 current・directory・history・threadsを読む | 全てAのcounter=2のpair。Bの3またはcatalogの4へjoinしない。読取りによるcounter増加は0 |
| Aの通常のDB I/O失敗、Bとcatalogは有効 | Aの確認済みsnapshotは2と別の決定的degraded pairで200、Bは3、catalogは4のpairで200。正常frame/pairとcounterを変更しない。Aの所有資格/source失効ならAだけ503 |
| Aのreaderが同じdomain・公開窓で回復 | Aのpairは2。条件headerなしなら200、Aの有効な同route/query条件が一致したときだけ304 |
| M1読取り中に新M2とfreshnessを受理 | M1のpublishは0回。次cycle開始時のTで全初期化済みpartitionを各1回照合し、M2候補を通常admission。待機tickの数だけcycleを反復しない |
| current counter=u128::MAX-1、境界交換に2 frameが必要 | 片側だけをMAXへpublishせず、旧root/pair/counterを保持したままpermanent-failedで終了 |

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
| 400 | `stale_cursor` | history cursorのperiod/prefix/gap fingerprint不一致、またはv5 directory cursorのaccount/pair/種別/offset不正 |
| 404 | `not_found` | exact known pathでない |
| 405 | `method_not_allowed` | exact known pathに対するGET以外 |
| 408 | `request_timeout` | request header/body deadline超過 |
| 413 | `request_body_not_allowed` | GETにnon-zero bodyまたはTransfer-Encoding |
| 413 | `resource_too_large` | split resourceのenvelope込み単一row、非分割互換resourceの全bodyが32 MiBへ収まらない、または互換detailsの44,640 sample超過 |
| 429 | `too_many_requests` | connection admission上限超過 |
| 431 | `request_headers_too_large` | header count/field/aggregate上限超過 |
| 500 | `internal_error` | response commit前のserialization/invariant failure |
| 503 | `snapshot_unavailable` | 確認済みframeなし、所有資格/domain/source拒否、publisher継続不能、またはshutdown中。保持可能なDB I/O失敗は上記degraded 200 |
| 503 | `history_unavailable` | 現在quotaは読取り可能だが、履歴projectionの読取り・公開検証に失敗した場合のv3/v4/v5 directory、各selectable、および`/v3/history` |

route/publisher faultを200や`state=error`へ丸めない。valid details generation自身の`state=error`だけはschema-valid
200 snapshotであり、server transport faultと別物である。serialization failureがheader/body commit後に起きた場合は
connectionをabortし、追加JSONやpartial-success markerを送らない。clientは全non-200、切断、長さ不一致で
該当resource candidate全体をrejectし、UI consumerはそのsurfaceの直前完全rootを保持する。

### Request resource contract

製品endpointはHTTP/1.1だけを受け、request targetはorigin-formのexact
`/health`、`/v1/health`、`/v1/runtime`、`/v1/details`、`/v2/details`、`/v3/details`、`/v3/current`、`/v4/current`、
`/v3/history/periods`、`/v3/history/periods/selectable`、`/v4/history/periods`、`/v4/history/periods/selectable`、`/v5/history/periods`、`/v5/history/periods/selectable`、`/v3/history`、`/v3/threads`、`/v3/accounts`である。
queryはACCOUNT-SELECT-134のaccount指定、v3 historyのperiod/cursor、v5 directoryのcursorに限り、各最大1件、historyのperiodだけ必須とする。percent decode、path normalization、fragment、absolute-form、authority-form、asterisk-formを許可しない。

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
`If-None-Match`はpublished-pair headerを持つsnapshot resourceでquoted published pairを受け、health/runtime等の非snapshot routeでは400とする。v3限定の条件をv4/v5へ流用しない。
ただしContent-Lengthは欠落可とする。`Authorization,Cookie,Proxy-Authorization,Forwarded,X-Forwarded-For,
X-Forwarded-Host,X-Forwarded-Proto,Upgrade,Expect,TE,Transfer-Encoding`は常に拒否する。obs-fold、NUL、CTL、
bare LF、invalid UTF-8を値として解釈せず400またはparse前closeにする。拒否requestはbodyを無制限drainせず
socketをcloseし、DB/published generation/settings/checkpointを0変更とする。

### Read-only effect set

全route・全statusで許可するproduct effectはrequest lifetime内heap、bounded in-memory counter、loopback socket
read/write、read-only open/stat、およびDATA owner `REST-LOG-RETENTION-482`が定義するfailure logだけである。同logの固定field・pathへのappendと期限切れ同形式log削除は許可し、失敗してもHTTP結果やDB/publicationを変更しない。それ以外のpersistent log/Event Log/metric/cache/temp、registry、child process、
非loopback socket/DNS、file create/write/rename/delete/fsync、SQLite transaction、DB/WAL/SHM、backup、migration、
checkpoint、published generation mutationは禁止する。OS-managed atimeはproduct successの根拠にせず、content/inodeと
product syscall traceを検査する。同一request再入で副作用countが増えた場合はFAIL/HOLDである。

### Surface-scoped client admission

Main cycleはhealth readiness受理後にcurrentを1回取得する。`state=ready && authenticated=true`の受理候補だけを成功したMain bundleとする。`error`の応答で0件を初めて確定せず、既存の完全bundleがあれば保持し、なければ未受理のthread件数は未知とする。有効な保存quota/modelの存在と、bundle全体の取得成功は別である。Linux / Windows Mainは表示中のmodel別open Session件数を確定するため、
`open_session_thread_count>0`のときだけ同じcycleでthreadsを1回取得し、currentと同一published pair、open件数と行数、active件数とrunning行数の一致を確認して
一括commitする。`open_session_thread_count=0`ではthreads requestを行わず、同じcurrent pairのthread行を空として一括commitする。
Mainの合計と`SOL/TERRA/LUNA/ASTRA/その他`はその一つの受理済み行集合だけから導出し、合計は常にbucket和と一致させる。
初回からschema/domain/pair検証済みの`/v4/current`の`state=error && authenticated=true`だけを取得できた場合は、成功したMain bundleを作らず、同じ応答にある有効なquotaと掲載modelの既知値を一つの失敗表示へatomicに投影する。未掲載model、models_status、観測時刻を保ち、thread件数は正数/0を問わず未知とし、このError応答からMain用threadsを要求しない。未知を0件・0行・100%へ補完しない。既存の完全bundleがある場合はその全表示を保持し、Error応答の一部と混ぜない。完全bundleをまだ受理していない間の後続Error応答は、この失敗表示を同じ規則で置換できる。失敗表示はlast-good完全bundleへ昇格せず、成功した通常bundleだけが確定thread件数と正常な取得状態へ遷移させる。transport失敗、malformed、pair不正のbodyはこの例外へ使わない。v1〜v3の一体Error応答にはこのv4例外を適用せず、完全bundleがなければ未知と失敗を表示する。
開いているThreads詳細はMainが受理した同じbundleのthread行を同じUI更新で反映する。0件では行とtreeを空にし、追加requestを行わない。Main更新前に開始した独立Threads取得の古い応答は、この更新を上書きしない。
threads失敗・pair不一致・件数不一致ではcurrent単独をcommitせず直前の完全表示を保持する。positive bundle失敗後は、
10秒後にcurrentを`If-None-Match`なしで1回だけ再取得する。そのbundle成功までforce pollを優先せず、Graphと独立Threads pollを
0件にし、last-good root、各surfaceのstateとerrorを保持する。10秒は失敗確定時刻からのmonotonic deadlineとし、手動再試行や同じMain取得を呼ぶSettings更新でも短縮・延長しない。
待機・in-flight中のCTA、再失敗時の次の待機、認証主体/account/接続設定generation変更時の旧待機破棄はUX 4.6に従う。bundle成功時だけretry markerとerrorを解除する。
Graphはopen/period選択時に必要なdirectoryと選択periodの全pageを取得し、open中の更新は後述の60秒周期のcursor差分とperiod変更通知規則に従う。Threads詳細はopen中だけ5秒周期でthreadsを取得する。split modeの各surfaceはbody全体のstrict schema/domain、size、exactly oneかつ同一の
`Codex-Info-Published-Pair` header形式を満たす場合だけ同じroot generationとして一括commitする。旧v1/v2 detailsのheader欠落互換は§リソースの単一完全body・非join条件だけを使う。
timeout/non-200/切断、必須header欠落・重複・pair値の大小文字差・prefix/長さ/hex不正、body欠落・未知・重複key、domain不整合では
対象surfaceのcandidate全体をdiscardして直前の完全rootを保持する。route+queryのlast-goodがないclientは条件headerを送らず、
cacheなし304はrejectして同一callback内でretryせず次の通常周期に一度だけ無条件取得する。wireにJSON generation fieldを追加せず、published-generation headerは
snapshot応答のopaque generation identityとしてだけ使い、body SHAやcommon-core hashを別identityとして作らない。

Linux / Windows Mainは10秒周期、Graphのcursor差分はopen中60秒周期、Threads詳細はopen中5秒周期とする。
Graphが閉じている間は対応requestを0件とし、Threads詳細が閉じている間は上記Linux / Windows Mainの条件付き1回を除きthreads requestを0件とする。同じpairではbody 0の304を使い、current更新時も
history全体を取得しない。history cursorはperiod、最後の`(reset_at,timestamp)`、そこまでのsample canonical prefixと選択periodの完全gap集合のSHA-256 fingerprintへ結合したclient非解釈値である。serverはsnapshot構築時に累積fingerprintとkey indexを作り、requestではkeyの二分探索とfingerprint比較だけで旧cursorを検証する。現snapshotの同じsample prefixとgap集合が一致する場合だけ、旧pairで発行したcursorも受理し、cursor後のrowを現pairで返す。先頭からの完全取得では最初のpageだけが完全gap集合を持ち、後続pageは空のgap集合を持つ。delta pageもgap集合を反復せず、clientはその現pairの全pageを受理した後だけsampleを直前prefixへatomic appendして既存gap集合を保持する。過去row補正、gap追加・回復・補正、period変更、unknown、stale、malformedでは`400 stale_cursor`としてpage集合を破棄する。clientは`G137-9`のexact responseを保存cursorへの応答として受理した場合だけ、同cycleで先頭から1回取得できる。それ以外の4xx、malformedまたは回復取得失敗は同cycleで反復せず、last-goodを保持する。これにより通常appendのrequest処理と通信はdelta量だけに比例し、prefixまたはgap変更時だけ完全再取得する。

選択periodの先頭pageはGraphが実際に要求した時点でだけ同じimmutable published pair内へ遅延生成し、そのpairを読む全loopback接続で再利用する。未選択periodを先行serializeせず、新しいpairへのatomic switchで旧page cacheも同時に破棄する。v5 directoryのpageも同じ寿命であり、旧pairをpinしない。

cursorの継続資格とHTTP条件付き応答は別に判定する。v3 historyは上記のprefix/gap証明に成功したcursorだけを現pairへ継続し、cursor後のsuffixを有限pageとしてserializeする。v5 directoryは§v5 directoryの規則どおり、cursor内のpairが現pairと異なればETag判定より先に400 `stale_cursor`とし、旧pairのcursorから現pairのsuffixを返さない。有効なcursorを持つ両resourceとも、そのroute/queryの現pairに一致する`If-None-Match`がある場合だけ304とする。同pairであっても条件headerがないか一致しなければ200で対象pageを返す。request処理からDB、Session、履歴canonicalizationへ戻らない。

`/v3/current`のexact 404を受けたclientは、その接続中をlegacy details modeとし、fallback列で受理した一つの完全details rootをMain、Graph、Threadsへ同時投影する。legacy modeではsplit history/threads routeを追加要求せず、Mainの10秒周期で同じdetails列だけを更新し、Graph/Threadsは最新の受理済みrootを表示する。再接続時には`/v4/current`から能力判定をやり直す。これにより旧serviceでもsurfaceを欠落させず、splitとlegacy rootを混在させない。

schema-validなdetailsの`state=auth_required|initializing|error,authenticated=false`が上記exact empty契約を満たす場合、
その同じdetails rootで旧account可視値を空にする。認証開始・確認controlの成功・失敗だけではdata rootを変更しない。

## 稼働プロセスのバージョン（Issue #481）

`RUNTIME-VERSION-481`: `GET /v1/runtime` はaccount、DB snapshot、認証状態に依存しない診断メタデータである。HTTP 200のexact keysは `api_version`, `rest_version`, `recorder_version`, `recorder_status`。`api_version` は `v1`、RESTは応答中のprocessにcompileされた配布version、Recorderはfreshな状態公開と同一PID/starttimeの生存processが公開したcompile済みversionである。公開が旧schema・取得不可・失効の場合、Recorder値はnull、statusは `unavailable`。両値が等しければ `available`、異なれば `mismatch` とする。UI版や実行時のmanifestで値を補完しない。
配布versionはPRODUCTの既存beta契約に従う32文字以内のcanonical `X.Y.Z` または `X.Y.Z-beta.N.A` とし、N/Aは正canonical十進整数とする。共有`valid_runtime_version`をRESTのRecorder版採用、runtime DTO検証、Linux healthで使い、非canonical・未知suffix・上限超過を拒否する。full versionを保持し、beta同士/ stableとの不一致とnull/unavailableを区別する。文字列受理だけで実processのfresh schema/PID/starttime/生存を成立扱いしない。
既存healthの3-key schemaを維持する。runtime応答は共通JSON/no-store headerを持ち、snapshot published-pair headerを持たない。Settingsの表示意味はUX owner `RUNTIME-SETTINGS-481` が所有する。

## REST失敗の診断（Issue #482）

`REST-FAILURE-482`: 全登録routeと共有failure ownerはrequest解析、route/input拒否、snapshot/DB読取り、projection/応答生成、応答書込みの失敗を診断できるようにする。HTTP失敗は既存の固定error code、内部失敗は固定categoryとOS/SQLite error kindを理由として記録する。記録項目はtimestamp、canonical routeまたは固定の未解析/未知route識別子、stage、reason、HTTP status。raw URL/query/body/header、token、credential、account識別子、DB値・例外のraw文言を記録しない。ログ保存・閲覧境界はDATA owner `REST-LOG-RETENTION-482` に従う。
HTTP 200の本文で`state=error`を返す場合も記録する。現在の取得lane失敗は`acquisition_failed`、現在値projectionの不完全さは`incomplete_current_projection`、refresh失敗によるlast-good保持は`last_good_after_refresh_failure`、account境界のerrorは`account_boundary_error`として区別する。readerは履歴sourceの未処理rangeと現在の取得lane失敗を独立して渡す。過去の未処理rangeだけでは、正常に取得・保存されたcurrent/detailsをerrorへ変更しない。未処理range、モデルの欠測・部分値、履歴gapを削除・完全扱いせず保持する。usage generationが変わらない取得失敗・回復もcacheへ反映し、error時のETagが正常回復を304で隠さない。
