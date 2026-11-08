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

# Loopback REST API

Refs #690. Authorityは2026-11-07のユーザー要求、API仕様、wire補遺、bootstrap/hash補正である。文書pathと既存契約IDは参照用であり公開APIの版番号ではない。collector/recorder DB、原本資格、期間意味、価格演算、保持期間、UIの既存操作と描画計算は変更しない。PRODUCT/DATA ownerの資格をwireの都合で弱めない。

## 公開resource

`API-DEPRECATION-01`: 公開するのは次の5本だけ。版番号、api_version、旧schema/route/別名/fallback、独立health/runtime/accounts/receipt/管理APIを廃止する。

| method/path | query | UI用途 |
| --- | --- | --- |
| GET /state | 必須parts、必要時account、任意image/cursor | Main、account選択、Settingsの製品版 |
| GET /history/periods | 必須account/purpose、window時必須from/until、任意image/cursor | Graph期間選択/表示範囲 |
| GET /history | 必須account/period、任意image/cursor | Graph sample/gap/局所使用区間 |
| GET /threads | 必須account、任意image/cursor | Threads詳細 |
| POST /subscription | queryなし | UI共有clientの購読/受領/再開 |

partsはmain,account_choices,account_context,runtimeの重複なしcomma listで、この順に正規化。main/account_contextのaccount省略時だけ確認済みcurrentを選ぶ。確認不能なら境界状態とし歴史accountを推測選択しない。account_choices/runtimeだけの要求でaccountを禁止する。選択肢/context/runtimeは、それを使うUIが可視のときだけ要求し、Mainへ常時同梱しない。

purposeはselectable/window。selectableはPRODUCTの選択掲載条件を満たす期間だけでfrom/until禁止。windowはUTC半開区間[from,until)に交差する期間だけ、from<until必須。不要な全一覧を送らない。cursorはquery/account/image/受領基点にbindし、image併記時の不一致は400。

## 型・HTTP・期限

U64は0..18446744073709551615の正確なJSON整数。Timeは既存の正のUTC Unix秒domain（半開区間の排他的終端だけ直後の終端を許す）。Time/区間長はi64、depthは既存のint32値域をi64としてhash化する。Textはsourceで受理済みのUTF-8文字列。Opaqueは1..1024 byteのURL-unreserved ASCIIでclientは解析/生成しない。DigestはSHA256の64桁lowercase hex。Refはexact {"ref":U64}、IDは正。Moneyは保存値へround-tripする有限binary64、Percentは有限binary64の0..100。整数をdoubleへ変換せず、epsilon/表示丸め/再価格計算を適用しない。

既存loopback listener/SSH TCP経路上でHTTP/2 prior knowledgeを使う。non-loopback bind、HTTP/1 fallback、HTTP/2 PUSH_PROMISE、redirect、proxy、cookie、HTTP汎用圧縮を禁止する。GET bodyは禁止。targetは2048 byte、headerは32 field/aggregate8192 byte/各value1024 byte以下。GET responseはapplication/json; charset=utf-8とCache-Control: no-store、固定Content-LengthはUTF-8 bytesと一致。秘密/token/raw AccountKey/path/session本文/raw errorを送らない。login IDはstateのaccount表示contextだけ。

GET必須headerはCodex-Info-Consumer:Opaque、Codex-Info-Cache-Root:Digest、Codex-Info-Budget-Ms:整数1..3000。新規UI profileの初回GETだけconsumer省略と空rootを許し、serverが同名response headerでIDを発行する。clientはIDを永続化してから本文受領を確定する。既存profileのID/cache消失は初回へ偽装しない。

UI共有受信器の起動時のjournal復元・資格確認は、UI workerの準備工程で行う。準備中のUI要求は正常な準備中状態を直ちに返し、HTTP要求をqueueへ隠して保留したり、local復元時間を接続失敗へ分類しない。受信器準備後の最初の接続・購読・GETは同じ3秒予算を共有する。通常GETは共有clientへの呼出開始から接続/queue/header/完全bodyまで合計3.000秒。serverはheader完了から1.000秒かつ残り共通deadline以内。段階ごとに3秒を足さず、未開始queueへ待ちを隠さない。handlerは準備済みimmutable image/index/dataを使い、未受領pageだけを既定境界内でencodingし、collector/外部API/SQLite lock/Session全走査/全履歴再計算/他resource更新を待たない。準備はHTTP処理外で行い、未準備はpendingで通知を待つ。deadlineで処理/streamを終了し受領位置を保持する。

TCPはconsumerごと1本/listener最大16。singleflightはstateの4projectionと他GET3種の計7単位、subscriptionを加え最大8stream。取得済み/取得中projectionへ合流し、新partsだけを並列取得、後続要求で先行deadlineを延長しない。view変更は旧処理取消し/最新1件への置換。F=16384 byteはframe/アプリbuffer境界。Rust server受信はstream F/connection 8F。標準.NET client受信stream windowは65535に固定しdynamic window sizingを無効にする。非公開のclient connection window値を設定/保証しない。通知は最大8192 byteを1件だけ未ack送出し追加を最新へ集約するため、通知だけでHTTP/2の初期connection credit 65535を使い切らない。全GETのnetwork読取りをUIから分離し、streamごとのアプリbufferをFに制限する。control frame優先/data frame巡回、credit/送信待ちも同じdeadlineで終了する。方向ごとの設定差をAPI版や別transportとして公開しない。根拠はMicrosoft LearnのInitialHttp2StreamWindowSize (.NET 10)とHTTP/2 dynamic window scaling仕様である。

構文不正/重複・未知query/未知enum/型不正/必須欠落/入力矛盾は400。構文上正しい旧image/cursorや消失account/periodはsync_requiredで別対象へfallbackしない。初期化/未認証/収集中/未更新/partial model/確定空/同期は正常状態。TCP確立不能は接続失敗、途中切断/deadlineは通信失敗、HTTP error/cache不良は接続エラーにしない。

`REST-FAILURE-482`: 真のsource資格拒否は503、serialization不良は500。error bodyはexact {"error":code}、codeはbad_request,unknown_projection,invalid_definition,request_too_large,capacity_exceeded,snapshot_unavailable,serialization_failed。診断保存はDATA REST-LOG-RETENTION-482に従う。H2 failureは固定path/stage/reason/statusだけを16TCP×8streamぶんの有界queueへ渡し、filesystem I/Oと保持整理はHTTP worker外の同一FailureLog ownerで行う。診断失敗/queue満杯でdata/HTTPを変更しない。

## 定義・参照・差分

全画面共有の永続受領cache/取得担当を一つ持つ。受領済み同一論理内容の本文再送、定期snapshot、ほぼ同一snapshot、304定期pollを禁止する。新時刻で累計値が同じなら関連付けだけを送り観測を残す。画面/endpoint間でも定義を共用する。共通header/参照metadataと値本文を区別し、TCP再送や無期限の全過去byteへ禁止範囲を拡大しない。

```text
Data={result:"data",delivery_id:Opaque,definitions:Definition[],patches:Patch[],
 changes:ViewChange[],fragments:Fragment[],completed_views:Opaque[],next_cursor:Opaque|null}
Definition={id:U64,kind:Kind,value:WireValue}
Patch={id:U64,kind:Kind,base:Ref,ops:FieldOp[]}
FieldOp={op:"set",path:JsonPointer,value:WireValue}|{op:"remove",path:JsonPointer}
ViewChange={view_id:Opaque,account:Opaque|null,base_image:Opaque|null,target_image:Opaque,
 sets:[{key:LogicalKey,value:Ref}],removes:LogicalKey[]}
Fragment={id:U64,kind:"definition",total_bytes:U64,offset:U64,
 content_hash:Digest,data:Base64Url,last:boolean,descriptor:FragmentDescriptor|null}
Control={result:"pending"|"sync_required"|"cache_recovery_required",view_id:Opaque,
 reason:Reason,target_image:Opaque|null,control_seq:U64}
Reason=preparing|receiving|image_changed|cursor_outdated|account_missing|period_missing|
 receipt_unknown|cache_invalid|cache_missing|response_prefix_mismatch
```

WireValueはschemaで型が決まるJSON値またはRef。Ref以外でrefを予約keyにしない。Kindはmain,selection,quota,model_usage,model_cost,thread_summary,thread_model_count,account_choices,account_choice,account_context,ownership_interval,runtime,period,sample,history_model,gap,local_usage,local_interval,thread,text,u64,i64,f64,bool,null,list。listは参照配列。IDを再利用せずpatchは新immutable定義を作る。kind不整合/cycle/未解決参照/存在しないremove/型不整合setを拒否。JsonPointerは/区切り、~0は~、~1は/。配列所属変更は参照配列をsetし本文を再掲しない。

completed_viewsだけをatomic交換可能。途中定義/page/fragmentは永続保存しても途中表示へ昇格させない。変更なし明示確認は204/bodyなしで、定期確認に使わない。pending/制御応答を同じ入力でpollしない。

## payload・source・論理key

`API-V3-MODELS-01`: このIDは既存参照用で公開版ではない。以下はRef解決後の型で、wireでは既知部分をRefで表す。?は欠落可能、nullは明示不明。

```text
Main={acquisition_state:initializing|ready|auth_required|error,observed_at:Time|null,
 authenticated:bool,plan_label:Text|null,selection:Selection|null,quota:Quota|null,
 models_status:complete|partial|unavailable,models:ModelUsage[],
 thread_summary:ThreadSummary|null}
Selection={account_id:Opaque,login_id:Text|null,is_current:bool}
Quota={remaining_percent:Percent,reset_at:Time,window_seconds:positive integer,monthly:bool}
ModelUsage={model:Text,total_tokens:U64,input_tokens:U64,cached_input_tokens:U64,
 cache_write_input_tokens:U64|null,output_tokens:U64,estimated_cost:ModelCost|null}
ModelCost={price_revision:Text,ordinary_input_dollars:Money,cached_input_dollars:Money,
 cache_write_input_dollars:Money,output_dollars:Money,total_dollars:Money}
ThreadSummary={open_count:U64,running_count:U64,unknown_activity_count:U64,
 by_model:ThreadModelCount[]}
ThreadModelCount={model:Text|null,open_count:U64,running_count:U64,unknown_activity_count:U64}
AccountChoices={default_account_id:Opaque|null,accounts:[{id:Opaque,login_id:Text|null,is_current:bool}]}
AccountContext={id:Opaque,activation_at:Time|null,deactivation_at:Time|null,
 ownership_intervals:[{start_at:Time|null,end_at:Time|null}]}
Runtime={rest_product_version:Text,recorder_product_version:Text|null,
 recorder_status:available|unavailable|mismatch}
Period={id:Opaque,start_at:Time,end_at:Time,reset_at:Time,label:Text,current:bool,origin_at:Time|null}
HistoryModel={model:Text,total_tokens:U64,input_tokens?:U64,cached_input_tokens?:U64,
 cache_write_input_tokens?:U64,output_tokens?:U64,total_dollars?:Money}
SampleValue={remaining_percent:Percent|null,task_active_since_previous:bool|null,
 models:HistoryModel[]|null,models_complete:bool,
 model_source:confirmed|legacy-unknown|reconstructed-from-session|unknown|unavailable}
Gap={reset_at:Time,start_at:Time,end_at:Time,reason:Text}
LocalUsage={start_at:Time,end_at:exclusive Time,
 intervals:[{start_at:Time,end_at:exclusive Time,state:used|unused|unavailable}]}
ThreadValue={title:Text,activity_status:running|stopped|unknown,parent_thread_id:Opaque|null,
 model:Text|null,model_label:Text,total_tokens:U64|null,context_usage_tokens:U64|null,
 context_window_tokens:U64|null,created_at:Time|null,last_user_message_at:Time|null,
 is_subagent:bool,depth:integer|null}
```

入力総数はcached/cache writeを含み、表示bucketはPRODUCT MODEL-USAGE-DISPLAY-01に従う。保存履歴額を最新価格で再計算しない。要約は同じ確認済みthread集合から導出し合計を一致させ、不明を0にしない。Mainの件数のためthread全行を取得しない。summaryと/threadsは既存Unix/Windows UIと同じく停止したrootと停止した親配下を除いた可視treeだけを対象にする。UIが使用しないraw件数/非表示rowを重ねて送らず、保存原本は変更しない。MainはThreadsの全体digestを持たず、完成したThreads可視row集合の集計だけをMain summaryと照合する。title/context/timeだけの変更はMainのimage/通知を変えない。

`ACCOUNT-SELECT-134`: 選択肢は確認済みcatalogから作る。所有区間は半開、不明開始を全時刻所有/未終了を未来保証へ変えない。確認済み区間だけを含み、空配列は区間の根拠がないことを表す。全域の非所有や連続所有を推測せず、activation_at/deactivation_atから欠けた所有区間を補完しない。default未確認で歴史accountを推測選択せず、明示選択をdefault変更だけで置換しない。account境界で可視view/pending/cursorを交換する。

`RUNTIME-VERSION-481`: runtimeは稼働製品版でAPI版でない。未知版や欠測を推測しない。

`REST-129`: 保存exact key、公式R/W、公開period identity/数値資格はPRODUCT LIVE-RESET-129/CURRENT-PERIOD-BOUNDS-129/RECORDED-USAGE-SCOPE-604とDATA HISTORY-CANONICAL-134に従う。公開IDはopaque、既存rg1 group/raw member namespaceを保持し、近接時刻/同値だけで合算/max/再所属しない。原時刻、保存値、価格bits、gapを保全し、projectionの表現不能だけで既知値を0/空にしない。period wire順は(reset_at,id)昇順（idはunsigned UTF8 byte順）、UI選択順/意味は維持する。

model数値公開はPRODUCT/DATA資格を満たすconfirmed/legacy-unknownだけ。他はmodels=null。models=[]とmodels_completeで確定空/未取得を区別し、未確認成分を反復nullで送らない。局所使用/quota/USD/全端末coverageから互いを推測しない。

```text
/state: ["main"], ["account_choices"], ["account_context",account_id], ["runtime"]
/history/periods: ["period",period_id]
/history: ["sample",period_id,reset_at,timestamp], ["gap",period_id,gap_id], ["local_usage",period_id]
/threads: ["thread",thread_id]
```

keyは所属viewと組にする。sample時刻/reset、thread IDをvalueへ二重掲載しない。period metadataの配信ownerはperiods、履歴は同じhistory source imageを参照。Main/Threadsのcount依存だけを同じ可視集合のsummaryで照合し、詳細が可視の場合だけ必要な両viewを揃える。GraphをMain imageへjoinしない。

viewはserver発行。初回GETで返す。初回購読はview_id/accepted_image=nullとpath/queryを許し、ready/購読ackのviews配列でpath,正規化query,view_id,target_image(nullable)を返す。

## subscription・受領再開

POST request/responseはapplication/x-ndjson; charset=utf-8。1recordはJSON object+LF。最初の200 HEADERS/完全readyまで呼出開始3秒でsessionを返す。以後の購読寿命は通常GETの未完了待ちではない。各制御recordも送出からackまで3秒/server完全record受領から1秒。

```text
Target={view_id:Opaque|null,path:DataPath,query:object,accepted_image:Opaque|null}
FragmentDescriptor={definition_kind:Kind,value_hash:Digest,view_id:Opaque,target_image:Opaque}
PartialReceipt={delivery_id:Opaque,id:U64,committed_offset:U64,prefix_hash:Digest,
 content_hash:Digest,total_bytes:U64,descriptor:FragmentDescriptor}
open={type:"open",seq:U64,consumer:Opaque,cache_root:Digest,max_definition_id:U64,
 subscriptions:Target[],partial_receipts:PartialReceipt[]}
ready={type:"ready",ack_seq:U64,session:Opaque,
 cache_state:matched|reconcile_required|recovery_required,views:ResolvedView[]}
subscribe={type:"subscribe",seq:U64,targets:Target[]}
receipt={type:"receipt",seq:U64,delivery_id:Opaque,complete_through:U64,
 partial:PartialReceipt|null,cache_root:Digest}
manifest={type:"manifest",seq:U64,probe:Opaque,
 nodes:[{key:Opaque,hash:Digest,id:U64|null}],next:Opaque|null}
notify_ack={type:"notify_ack",seq:U64,notify_seq:U64}
check={type:"check",seq:U64}
close={type:"close",seq:U64}
ack={type:"ack",ack_seq:U64} // subscribeだけviewsを含む
changed={type:"changed",notify_seq:U64,views:[{view_id:Opaque,target_image:Opaque|null}]}
probe={type:"probe",probe:Opaque,keys:Opaque[],expected_root:Digest}
recovery_required={type:"recovery_required",ack_seq:U64,reason:cache_invalid|cache_missing}
```

seqは永続cacheで単調増加し0/再利用禁止。complete_throughはdelivery内でdefinitions→patches→fragments→changesの各配列を順に連結した1始まりの連続確定ordinal（0は未確定）。JSON objectのproperty順とは無関係。参照依存の検証が閉じたprefixだけを確定し、末尾より前の受領ではcompleted_viewsを昇格させない。通常receiptは小さい位置をsubscription/次の必要要求へ載せ、全ID一覧や専用HTTP操作を毎回作らない。changedは実変更だけで未送信分を最新へ集約。checkは再表示/明示更新など必要時だけ。無通信を取得成功へ読み替えない。

切断後は受領位置付きで1回再接続し、失敗後は利用者/既存接続管理の新入力を待つ。ack消失はmetadataだけ、途中本文は永続prefix以後だけ。server再起動/差分履歴不在でも現在imageとmanifestの未一致枝から未受領だけを選ぶ。manifestのnode.keyはnamespaceを含むcanonical JSON bytesのpaddingなしBase64urlで、alias/bindingのidは参照ID、receivedはnull。probe keysが空なら未確認rootの最初の枝を要求し、1record8192 byte以内で必要な枝だけ進める。途中fragmentの型/値digestが現catalogに残る場合は旧ID/参照aliasで全recordを再構成し長/hash/prefixを照合してoffset以後だけ送る。現catalogに無い旧partialはsync_required/image_changedとし、clientはそのviewの未表示partialだけを永続transactionで破棄して最新targetへ進む。既表示値/原本を消さず、旧本文で埋めず、cache破損へ誤分類しない。cache破損は接続errorでなく復旧状態、復元元なしで自動本文再送せずconsumer付替えで迂回しない。永続化/表示交換はDATA CLIENT-RECEIPT-CACHE-690に従う。

## page・fragment・hash

data body全体33554432 byte以下（envelope/Base64/cursor込み）、history最大1024 sample/page、threads最大256行/page。256は今回の有限page処理量で保存総数/provider上限ではない。account/periodはbyte分割で128件総数制限なし。controlはLF込み8192 byte。fragment raw最大8192 byte、encoded record16384 byte以下。offsetは0から連続、非終端raw8192 byte、最終だけ短くできる。offset=0はdescriptor必須で、継続時は既受領descriptorを使い本文に反復同梱しない。server再起動で必要になった場合だけ部分受領metadataへdescriptor/全record長/hashを載せる。lastはoffset+raw長=total_bytesの場合だけtrue。Base64urlはpaddingなし。

非終端pageは行または未受領bytesを前進させる。32MiBを埋める目標、全件error、先頭だけの偽の完全応答を作らない。確定fragmentのbytes/offset/受領台帳は同じ永続transaction。resumeはidentity/total/prefix/offsetを照合してその先だけ。不一致はcache復旧状態。再組立/型/参照/内容検証が完成するまで表示を更新しない。

`WIN-PARITY-WIRE-01`/`WIN-PARITY-PAIR-01`: Rust/.NETは同じ型/数値/unknown/image/受領結果を得る。H=SHA256、B=unsigned64BE。値Vはnull H(0x10)、false H(0x11)、true H(0x12)、u64 H(0x13||B)、i64 H(0x14||two's-complement64BE)、f64 H(0x15||IEEEbits64BE)、text H(0x16||B(UTF8長)||UTF8)、array H(0x17||B(count)||各V)、object H(0x18||B(count)||UTF8 key順のB(key長)||key||値V)。V(Ref)は参照先V。欠落/null、順序、+0.0/-0.0を区別しIDを内容digestに混ぜない。

manifest entryはcanonical key bytes KとVをK順で整列。空root E=H(0x02)、leaf=H(0x00||B(lenK)||K||V)、branch=H(0x01||left||right)。1件はleaf。複数はcanonical key bytesの最初に異なるbitで0側/1側へ分ける圧縮radix treeとし、空の単項branchを作らず、leafを複製しない。完全なcanonical JSON keyのproper-prefixは不正。分岐位置は件数に依存しないため、変更key経路だけをpersistent更新できる。確定root参照はO(1)、全manifestの列挙/正規化/sortは復旧時だけ行い、通常GET/receiptや小変更で全cacheを再構築しない。namespaceは[received,content_digest],[alias,id],[binding,canonical_view_key,logical_key],[partial,delivery,id]。view keyは意味を決めるpath/queryだけでcursor/budget/server view IDなし。

fragment canonical JSONは空白なし/UTF8 key順/配列元順/整数正確十進。quote/backslashをescape、制御文字はlowercase unicode escape、それ以外はUTF8。f64は元bitsへnearest/ties-to-evenで戻る最少1..17桁、同桁は元値に最近、同距離は最終偶数、さらに同値は絶対値小。固定/科学表現の短い方、同長は固定、exponent lowercase e/plus・余分0なし、zeroは0.0/-0.0。数値型はkind/field schemaで保ち、JSON numberの汎用推測に依存しない。hashをsource資格/価格演算のauthorityにしない。

### HTTP応答の途中切断をまたぐ受領

GETのdata応答には`Codex-Info-Delivery`、`Codex-Info-Body-Sha256`（全本文SHA256小文字hex64）、`Codex-Info-Total-Bytes`（U64）、`Codex-Info-Offset`（U64、初回0）を付ける。送信側は公開前にconsumer/正確な要求target/delivery/全本文長/hashとimmutable本文を専用送信journalへ永続化する。受信側はheaderと実際に受信した生bytesを専用受信journalへ保存・fsyncしてから次のHTTP/2 creditを返す。閉じていないJSON recordもこのbytesの対象である。表示用の論理受領rootには、全本文の長さ/hash/JSON/型/参照を検証してtransaction commitしたものだけを反映する。

途中切断後は同じtargetのGETへ`Codex-Info-Delivery`、`Codex-Info-Offset`（永続化済みbytes数）、`Codex-Info-Prefix-Sha256`（そのprefixのSHA256）を付ける。送信側はconsumer/target/length/prefix hashを照合し、offset以降だけを返す。Content-Lengthは今回のsuffix長、Total-Bytesは元の全本文長である。offset==Total-Bytesなら本文0bytesでも元のdataを受領journalから検証・commitできる。server再起動も同じ送信journalを参照する。sourceが更新されても未完了のimmutable発行本文を勝手に置換しない。journalの喪失・不一致は接続エラーや通常の空ではなく`cache_recovery_required/response_prefix_mismatch`とし、既受信prefixを初回扱いで再送しない。

購読openの`incoming_deliveries`は`{delivery_id:Opaque,offset:U64,prefix_hash:Hash}[]`であり、受領中本文がある場合だけ送る。受領中の値に依存する別GETは通常の`pending/receiving`を返す。初回identity発行は最初の本文を永続commitするまで合流し、それ以後はprojectionごとの並列取得を保つ。送信journalは完全受領receipt、または検証済み受領台帳との照合によって資格が確定した時に削除する。論理受領cacheと送受信journalのlocal I/O失敗は`cache_io`でありTransportに分類しない。

通常GETの受領済み照合は確定root参照で行い、過去全cacheを列挙しない。prepared viewのradix subtree hashを比較し、変更された論理keyだけを遅延列挙する。各pageは最大8192回の定義依存探索と既定のbytes/row境界で区切り、以後はcursorで続ける。変更行の未受領依存だけを送信し、次pageのrow本文を先送りしない。受領側は定義map・view bindings・Merkle nodeを構造共有し、最終pageのview公開もbinding namespace subtreeの一括差替えで行い、受領済み履歴全量のcopy/hashへ戻らない。

受領cacheの外側にconsumer identity anchorを保持し、cache directory全損を新規profileへ読み替えない。既存profile/anchor双方の明示的な利用者復旧は通常同期と区別する。未完page cursorをjournalへ保存して再起動/再表示で継続する。不可視viewに途中のHTTP本文が残る場合も、その発行済み本文のsuffixだけを確定し共有定義の保護を解く。不可視viewの次page取得/購読は再開しない。

Linux installer/launcherの起動readbackは、同じ共有clientを使うlocal CLI `codex_info --wire-readback` のmain/runtime一回取得を使う。独立health APIや旧routeへfallbackせず、既存のprocess/実行file/recorder同一性検査を維持する。
