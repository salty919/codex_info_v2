<!-- codex-info-requirement-owner: DATA -->
<!-- codex-info-master-ids:
REST-LOG-RETENTION-482
QUOTA-DURABLE-604
RECORDER-MODEL-01
CUM-138-01
CUM-138-02
CUM-138-03
CUM-138-05
CUM-138-07
AUTH-129
GEN-129
ACCOUNT-PARTITION-134
HISTORY-CANONICAL-134
DB-129
SESSION-129
LEGACY-129
WIN-PARITY-HISTORY-01
WIN-THEME-PREF-422
U128-15
U128-18
U128-19
-->

# Codex Info データ保護規約

## QUOTA-DURABLE-604 — 利用枠の取得原本と再処理

公式quotaの取得原本を先に永続化し、期間判定・Session集計・公開をその原本のconsumerにする。取得原本は派生履歴を複製する互換表ではなく、Session JSONLと同じ再処理入力である。`HISTORY-CANONICAL-134`はSession由来のmodel履歴と旧混合履歴を所有する。その判定を公式quota原本の採否に使わない。

1. 完全な公式quota応答のpayload、取得時刻、取得順序、credentialを含まないaccount scopeを、quota解釈・期間採否・通知queue・Session処理より先にdurable commitする。解釈不能な応答も保持する。token、raw AccountKey、account/read payload、login IDは保存しない。
2. 通知queueは原本を所有しない。満杯、最新値への集約、consumer停止、前回batch再試行で保存済み原本を削除・上書き・処理済みにしない。quota取得とSession集計は独立して進む。 residentはaccount/partitionとprofile leaseを確保後、Session DB初期化・復旧より先にquota原本取得を開始する。Session writerを開始できない間も原本を保存し、writer復旧後に再処理する。
3. 不変な原本IDと取得順序を使用し、解釈済み公式quotaと適用済みIDをaccount DBの`quota_receipts`へ同じtransactionでcommitする。commit前の停止は同じ原本から再開し、commit後の通知欠落は二重加算しない。現在quotaは取得順序が最後の受理済み公式応答とし、過去再生やローカル時計の前後で巻き戻さない。履歴quotaも同じ受理済み記録から直接投影し、Session由来の派生履歴への二度目の書込みや別の完了位置を必要としない。同じexact reset/minuteでは最後に取得した公式観測を表示し、その前の観測も取得原本・受理済み記録から削除しない。異なるexact resetが同minuteにある場合は双方を保持し、旧detailsを含む公開sampleもWIREのexact keyごとの規則に従う。modelはexactな`reset_at`とminuteが一致する観測だけを結合する。modelの欠落・読取り失敗・旧quotaとの不一致を公式quotaの破棄理由にしない。履歴projectionのSQL失敗・公開domainの不成立も、別に受理済みの現在quotaを取り消さない。正常なreset総件数は公開domainの不成立に含めない。真正の履歴projection失敗では生成できない履歴だけを明示的に利用不能とする。旧v1〜v3 detailsの一体投影は現在値を含めたError応答とするが、独立したv4 currentの取得状態・models_statusを履歴失敗だけで変更しない。current自身の失敗と初回Error応答の表示はWIREのcurrent契約とsurface admissionに従う。有効なmodel・threadを0や未取得へ置換しない。空履歴を正常な履歴として返さず、保存済み原本・receipt・Session eventは変更しない。
   受理済みreceiptのcommitでは、同じexact reset/minuteのquota投影を同じtransaction内で保存する。既存model minuteがあれば`usage_history.remaining_percent`と対応するdurable observationの同fieldだけを取得順で更新し、model・token成分・保存USD bits・価格revision・source・coverageは変更しない。modelの無いminuteは`model_source=unavailable`かつmodel数値NULLのdurable observationとし、0 token/0 USDのmodel rowを作らない。後着receipt、Session解析失敗、集合未完備、別periodの同minuteを理由にこの保存を待たせない。既存DBの復旧では、原本・account/epoch・所有区間・exact keyが一致する公式receiptを根拠とし、NULL補完、quota-only行の新設、既知quotaとの競合を分ける。競合前の値は非回転backupに保持し、公式receiptの取得順をquotaだけの優先順位とする。receiptと取得原本は変更しない。
4. account確認前後が不一致の応答も原本として保持し、確認済みaccountへ推測で帰属させない。原本保存と公開資格を別の状態として扱う。 当該accountへの受理済み記録があれば、再起動後の公開に新しいAPI応答や通知を要求しない。取得障害時も受理済みquotaを障害状態とともに公開する。
5. 保存失敗時は成功通知・処理済み更新・新しい応答による未保存値の上書きを行わず、未保存応答を保持して保存再開を所有し、障害を公開する。保存後の解析・集計・公開失敗でも原本を保持し、再起動後に未反映分を再処理する。 未解釈の原本は後続の解釈可能な応答を止めず、未処理として残す。過去の未処理件数と現在の取得成否は区別する。取得順で後続の確認済み公式応答が解釈・保存できた場合、過去の未解釈原本だけを理由に現在値を取得失敗としない。最新の確認が終了した入力が未解釈なら障害とし、受理済み記録と同じtransactionで保存する。通信・原本保存の成否と、保存済み入力の解釈成否を独立して保持する。正常な後続原本の保存後に再起動しても、再生で解釈障害から回復でき、新たなAPI応答や通知を必要としない。実際の通信・保存失敗は引き続き障害として公開し、過去の原本を再生しただけで取り消さない。取得・再生の障害状態はthread記録の有無に依存せずgenerationとともに永続化・公開する。
   取得ownerのスコープ解放では新しいpollを開始せず、進行中の取得・原本保存が終了するまでworkerを所有する。周期待機は解放時に起こし、通常のreturn/Errが未保存応答を持つworkerだけを残して終了しない。SIGTERMはmainの停止要求として処理し、startup・周期・Thread結果の待機を起こして同じowner解放へ進む。保存失敗の再試行間隔は1秒とし、API取得間隔を流用しない。子app-serverも停止した場合は受信済みpayloadを原本に残し、account再確認が完了しなかった記録を確認済みに偽装しない。原本保存前のSIGKILL（保存障害中に停止猶予を超えて強制終了された場合を含む）と保存装置そのものの喪失は、ユーザー合意により保証対象外とする。通常の処理失敗・保存失敗・再処理失敗・再起動・SIGTERMでは、上記の所有と再処理を維持する。
6. quota数値の原本復元は本書8.7.0の公式取得provenance・元時刻・取得順を必要とし、cached Session snapshotを新しい実測点へ変換しない。原本のない残量を推測・補間・旧値複製で保存しない。model利用量の復元とquota実測の復元を区別する。Session原本のwindowは、同account・所有区間・accepted range/hashを検証し、全候補が一致する場合だけ、取得済みの同じexact resetの不足window metadataをsession_quota_windowsへsource pointerとともに保持できる。snapshotだけからperiodを新設せず、現在の取得順・quota_receipts・model値を変えない。readerは元の公式resetと補助pointerの資格を確認し、一意なwindowだけをorigin_atへ公開する。所有外pointerからは補完せず、既存Wとの矛盾を上書きしない。schema v16へのmigrationは本書`RECORDER-MODEL-01`の保存表現に従う。v15は既存のquota-window metadata schemaとして維持し、v14起点では不足metadataの追加とtoken成分のNULL許容変更を同じbackup・transaction境界で行う。v15起点では既存metadataを保持し、NULL許容変更とexact resetを含まない旧unique制約の除去を行う。既に稼働可能なv15を異なる制約で再定義しない。migration自体は既存canonical値・sidecarの値・event・checkpointを再計算しない。原本に基づく値の復元はmigrationと区別し、`HISTORY-RECOVERY-614`の別の採否条件を満たす場合だけ行う。
7. 取得→保存→解釈→適用→公開→再起動の各失敗について、原本の件数・内容・順序、未反映位置、二重計上なし、現在quotaの優先、DB/REST/実画面を立証する。保証対象は処理・保存・再処理のソフトウェア上の欠落である。原本保存前のSIGKILLと保存装置そのものの喪失を対象に含めない。保証対象内の未解決境界を残して完了にせず、ソースの実装・直接検証と配布済み環境への反映を区別して報告する。

## WIN-THEME-PREF-422 — Windows theme preference

Windows clientの設定JSONは従来のexact 6 keyを有効な旧形式として読み、theme未指定時は`classic-dark`をメモリ内の既定値にする。読込みだけで既存fileを書き換えない。新しい保存は従来6 keyに`themeId`を加えたexact 7 keyをatomic replaceし、値は`classic-dark`、`graphite-dark`、`light`、`paper-light`、`sand-light`、`steel-light`、`ocean-dark`、`teal-dark`、`ember-dark`、`ink-dark`、`neon-dark`、`lavender-light`、`mint-light`、`forest-dark`、`tangerine-dark`、`rose-dark`だけを許す。既存3 IDの保存値はそのまま有効とし、key数や移行形式を変更しない。重複・未知keyまたは不正theme IDを含む新形式は既存の設定破損経路で拒否し、接続資格情報を推測しない。保存失敗は永続fileと実行中の設定generationを保持し、成功した保存だけを設定generationへ公開する。`WIN-THEME-422`の選択中プレビューは表示paletteだけを変更し、保存成功まで設定を書き換えない。取消・閉じる・保存失敗では保存済み設定の配色へ復元する。旧形式のconnection profile/selectorとsetup状態はtheme移行で失わない。

## RECORDER-MODEL-01 — モデル追加・source障害時の記録継続

- `MODEL-PRICING-463`の新規記録は正確なmodel IDを保存し、既存family keyをモデル版へ推測変換しない。canonical schema v12は`usage_model_history`へNULL許容の`total_dollars`/`price_version`を追加し、旧行はNULLのまま維持する。既にcanonicalなv11からは検証済みbackup後の列追加・version更新だけを行い、期間再分類や履歴再集計をしない。新規履歴の金額は観測時に検証済みcatalog revisionで確定し、同じ観測の再読込み・修復では保存済みrevisionまたは観測時に固定されたrevisionを使う。価格表更新で旧履歴の保存額を上書きしない。互換用の固定family金額列は既知の正確なmodel IDも従来のfamily単価で集約し、新規sidecarの旧family行には当該行だけの固定参考額を保存する。v3で固定列の全family合計と正確なIDの保存額を二重加算しない。既存物理sourceの同一byte範囲と同一token payloadが証明するreplayでは旧familyイベントを保持し、新しいmodel IDへ置換・二重計上しない。

固定互換単価、cache writeを独立項にしない旧3familyの意味、参考額のbinary64演算順、保存値を保持する通常経路と原本prefixから再計算する復元経路の境界はPRODUCTの`MODEL-PRICING-463`を唯一の計算定義とする。同じ5成分のminute間で1 ULPだけ金額が変わる場合も、epsilonで同値化したりUIで前値へ変更しない。原本復元の全資格が揃わない限り、保存値と異常の説明を保持する。

- Sessionが返す有効なmodel IDは固定allowlistで捨てず、正規化したIDごとに既存account DBへ記録する。既知の`gpt-6-astra` / `ASTRA`を含め、入力、cached入力、cache write入力、出力の各tokenを保持する。価格未登録モデルもtoken事実は保存し、価格だけを未確定とする。
- account lifecycleの異なるSession eventを同じ累積値へ混在させない。writerはregistryの全half-open interval、保存済みevent、保存値の三者を照合する。有効な既存eventからの通常再投影と、原本が旧派生eventの誤計算を証明する修復はPRODUCT `HISTORY-RECOVERY-614`の別資格に従い、どちらもverified backupを前提にする。保存合計との一致条件を原本による誤計算修復へ流用しない。raw Session/accepted rangeは保持し、event/checkpointを変更できる範囲も同要件に限定する。所属を証明できない履歴はUnavailableとして公開し、reader/UIの後付け補償を行わない。
- `cache_write_input_tokens` の未提供と明示的な0を区別する。提供されない内訳を推測して確定値にしない。
- model totalsとsource checkpointは同じtransactionで保存し、再起動・同一range再取得で重複加算しない。未対応モデルを未保存のままrecorded markerとして確定しない。
- 既存cursorより前に読み飛ばされたASTRAは、保持sourceからモデル別に回収する。既存SOL/TERRA/LUNAの確定累計や履歴を再加算・削除しない。回収済みの範囲を永続化し、再実行で同じ補正を加えない。
- 一部モデルだけを回収した履歴は不完全フラグを永続化し、未掲載モデルを0または確定値として公開しない。全モデル集合を同じ観測から再構成できない限り、確定履歴へ昇格させない。
- 週切替を含む各観測では、モデルvector未提供、明示的な空集合、非空集合と、その完備性を同じ期間・timestampのtransactionで保持する。非空集合の数値は既存model sidecarを正本とし、行を持たない空集合は既存durable observationの任意boolean `empty_model_set_complete` で表す。収集に未処理・失敗sourceがあれば完備性はfalseとする。再読出し・再起動・同一commit再適用でも確定空集合を欠測や固定3モデルの0へ変換せず、v3では `models=[] / models_complete=true / model_source=confirmed` として両clientまで保持する。不完全な空集合は `models=[] / models_complete=false / model_source=legacy-unknown` とし、未使用確定や欠測の補完対象にしない。宣言のない旧観測は既存の既知値だけを保持し、過去の空集合・完備性を推測しない。空宣言と非空sidecar・非zero固定値が矛盾する場合は確定値として公開しない。
- 旧`usage_history`に保存済みのSOL/TERRA/LUNA別token・dollarは、全モデル集合やtoken内訳が不完全でも既知値まで捨てない。旧列にない入力・cached入力・cache write入力・出力はNULL、全モデル集合は不完全として公開し、0や推測内訳をDBへ書き込まない。
- 既存account境界、DB履歴、quota欠測を保持する。旧REST consumerにASTRAを理解したふりをさせず、記録対応と表示対応の完了を分けて報告する。
- 1つのSession sourceのsymlink、差替え、読取り/I/O失敗では当該sourceの未commit cursorを進めず、他の独立した有効sourceを継続する。改行済み不正recordと未改行tailには第2章のlocal usage隔離・未処理range・LF prefixのcheckpoint規則を使い、source全体の一律rollbackへ置き換えない。外部quota/app-serverの停止およびREST clientの有無をSession記録の停止条件にしない。
- readerの保持範囲、読み出し処理予算、wireの一page/一body容量を別の責務として管理する。raw reset alias行数、全pageの合計件数、互換31日窓の44,640点をcanonical履歴の妥当性上限へ使わず、reader/輸送の保護判定でDB writerを停止させない。

## HISTORY-CANONICAL-134 — account履歴の永続的な正規化

- account registryが宣言し、`storage_partition`のidentityと`quick_check`を検証できる全初期化済みpartitionを対象とする。現在ログイン中か、過去accountか、表示用login IDがあるかを対象判定に使わない。registry外、orphan、symlink、unreadable、schema不正、identity不一致のfileは変更せずdegradedとして列挙し、空DBへの置換、別accountへのfallback、推測修復を行わない。全対象partitionが同じalgorithm versionへ到達するまでmigration完了としない。
- この変換の目的は、旧writerが作った重複`reset_at` aliasと範囲外quota sentinelを稼働DBから完全に排除し、将来のreader・graph・修復処理がそれらを再解釈する余地をなくすことである。移行前の原形は検証済みSQLite backupだけに保存し、稼働DB内へraw互換表、canonical別表、revision同期表、例外補償用stateを追加してはならない。Session由来の派生model履歴と旧混合履歴の正本は`usage_history`一表だけとする。公式quota取得原本とその受理済み記録は`QUOTA-DURABLE-604`が所有し、この旧履歴の移行規則へ入力しない。
- `HistoryCanonicalizer`は本節の規則を実装する唯一の変換ownerであり、現行実装のrolling/jitter定数を同一cycleの証明にしない。既知の旧sentinelであるexact numeric `-1`はmigration入力でだけ「quota未観測」と解釈して`NULL`へ置換する。通常reader/writerでは`-1`を含む範囲外値を許容しない。原本の取得時刻、raw reset、成分、既知性はbackupとsourceから追跡可能にする。
- **period所属**: 公式receiptが証明する異なるexact resetは統合しない。旧raw行のresetを別resetへ写像できるのは、`HISTORY-RECOVERY-614`と8.7のaccount所有・source range/hash・event/checkpoint・lossless成分照合によって、その行の観測が一つの公式resetへ帰属すると証明できる場合だけである。対応候補0件は元のrecorded resetとlegacy-unknownを保持し、公式window起点を作らない。候補が複数、公式receiptとの矛盾、または同じraw観測を複数periodのどちらへ写すか決まらない場合はpartition migration全体をrollbackする。各raw観測のexact resetがそれぞれ証明済みなら、同minuteに複数periodが存在することを所有不明やrollbackの理由にしない。reset差が60秒以内、時刻の前進量が等しい、quotaが100%、backfill断片が他periodの内部にある、といった形だけでmerge・除外・再所属しない。
- **同一minuteの一意化**: 上記でsame periodが証明された行だけをminuteでgroup化する。distinct non-null quotaは最大1個とする。元の固定6成分と全model sidecarの既知成分を一つのlossless vectorとして比較し、model key集合・optional成分の既知性・provenance資格・price revisionが異なるものを0補完して比較しない。同じ資格のvectorについて、全既知成分が他の全候補以上である既存vectorが一意に存在する場合だけその元行を採用する。成分ごとのmaxで新vectorを作らない。完全に同じ値・資格のduplicateは冪等とし、採用元keyは`(raw timestamp,raw reset_at)`の昇順先頭に固定する。quota競合、非比較、dominant不存在はそのminuteと対応sidecarだけをlive集合から除外し、原形をverified backupへ保持する。別periodの所有不明と同じ失敗にしない。
- 初回migrationはpartition writer lock取得後、現行SQLite connectionからonline backupを作成し、`.bak.1`〜`.bak.3`の既存検証・保持規則を通した後だけ`BEGIN IMMEDIATE`する。DB/WAL/SHMを個別file copyせず、backupはcommit済みsnapshotを含む。backupのraw row countとordered deterministic raw-row fingerprintが移行直前のsourceと一致することを確認し、transaction内で`usage_history`をcanonical row集合へ置換する。`HistoryCanonicalizer`は各canonical rowと同時に、実際に採用したraw usage rowのexact旧`timestamp/reset_at`を返す。対応する`usage_model_history`はそのexact旧キーに結び付いた一群だけを同じcanonical keyへ移し、採用元決定後に、捨てたreset aliasだけのmodel群へ取り替えたり、混合・補間・再選択したりしない。既存`durable_state`内のhistory observationもexact旧キーが採用rowと一致し、かつ全usage vectorがcanonical rowと一致するものだけを同transactionで再所属する。所属不明、canonical vector不一致、または捨てたaliasのsidecarは正しい観測へ作り替えずlive sidecarから除去し、原形はbackupだけに残す。旧alias row、`-1`、orphan sidecar、並列履歴表を稼働DBへ残さない。
- 移行後の`usage_history`は`(reset_at,timestamp)`を一意、`remaining_percent`を`NULL`または有限な`0..100`とする。固定familyのcost/tokenは非NULLなら有限な非負値とし、schema v16のNULLは下記`RECORDER-MODEL-01`の旧形式への表現不能・不明を表す。NULLを含む行を丸ごと捨てず、同じkeyのsidecarの既知値と独立したquotaを保持する。`usage_model_history`は`(reset_at,timestamp,model)`を一意とし、同じexact keyのcanonical `usage_history`に対応しないrowを禁止する。通常writeは同じexact period内の既存rowとincoming batchをreconcileして一transactionで保存し、証明なく別resetへ写像したalias、不正quota、orphan sidecarを生成できない。readerはこの一表のcanonical `reset_at`をSession由来modelの期間所属として直接読み、同じexact keyのmodel・provenanceだけを結合する。公式quotaとその期間は`QUOTA-DURABLE-604`の受理済み記録を優先し、その公開window内にあるという理由で別periodのmodelをcurrentへ再所属させない。current recorderの既存exact R/W集計は報告枠の保存keyを区別する。同じR/Wを同じ保存key、異なるR/Wを別keyへ保存するが、新しい実利用期間が生じたとは判定しない。記録分はPRODUCT RECORDED-USAGE-SCOPE-604の独立indexから公開する。異なるexact resetの有効な観測が同一minuteにある場合は両方のrowとsidecarを保持する。新periodの再計算済みvectorは新しいexact keyへ保存し、旧periodの累計をcarryしたり、同minuteという理由で旧periodの観測を削除・置換したりしない。Session原本・event・checkpointも保持する。raw互換分岐、読出し時merge、UI/RESTでの補償を行わない。
- backup、lock、identity、schema、raw fingerprint、period/cycle境界、sidecar対応、DB制約、commitのどれかが失敗した場合は全rollbackし、source DB、verified backup、last-good publicationを保持する。同一period内の局所quota/vector不成立は上記canonical除外規則で処理し、partition失敗へ読み替えない。crashがcommit前なら旧schema、commit後ならcanonical単一表とschema versionが一括で可視になる。commit後のfresh read-only readbackに失敗した場合はpublicationを進めない。完成済みschemaの再実行は履歴rowとbackup generationを変更しないno-opとする。
- 再発防止の合格条件は、製品schema/codeに`canonical_usage_history`、`usage_history_observations`、`history_canonical_state`およびそれらの互換分岐が0件、移行後DBに範囲外quota・同一`(reset_at,timestamp)`重複・採用元と異なる旧resetのhistory observation・orphan model sidecarが0件であることを直接確認することである。異なるexact resetの同minute rowはduplicateに数えない。新しい永続表・revision・例外補償を必要とする案は本目的のscope外として実装せず、先に要件変更の明示確認を必要とする。

**通常収集の同minute更新**: 上記の旧重複rowの支配vector選択は、検証済みの新しいcollection frameの代わりに古いcomplete vectorを選ぶ規則ではない。同じaccount/exact reset/minuteへの通常更新では、8.7/8.8のsource・prefix・現在owner・新commitを検証した最新のmodel frameを、集合の完備性と成分のknownnessを含め一組で保存する。同じ所属証拠集合の下で新たな非負deltaを加える通常更新では、既知modelのTを後退させない。公式証拠集合の変更による所属資格・依存累計の変更には8.7「数値eventの公式期間所属」の再判定を使い、raw数値・model自体の訂正には原本復元の採否条件を使う。前者を数値後退の隠蔽、後者を単なる所属再判定として扱わない。新しい有効なframeの未知成分を旧vectorの成分maxや0で埋めず、旧complete集合を最新と称しない。derived rowと同key sidecar/durable metadataは同transactionで更新し、元のSession event/range/checkpointと公式receiptは保持する。source障害で有効な新model frameがない場合と、既知値を持つ有効なpartial frameを区別する。前者はmodel観測の更新をせず、独立したquotaだけを受理できる。後者は最新の既知値とpartialを保持する。quota receiptの保存だけではmodel観測を消さず、新しいquota時刻へ旧vectorを複製しない。公式証拠集合が変わった場合の所属再判定と依存観測の置換は上記の別のusage transactionで行う。異なるexact resetの同minuteは別keyとして両方を保持する。これを同periodへの最新frame置換と混同せず、別periodのmodelを混ぜない。

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

1. 既存の有効なDB行を、収集失敗・認証失敗・通信切断・UI終了・migration失敗で削除、上書き、推測変換しない。`HISTORY-CANONICAL-134`の成功済み移行だけは、検証済みbackupへ原形を保持したうえで、旧aliasと既知sentinelをcanonical rowへ置換する。
2. canonical DBは`(ProfileScopeId, AccountScopeId, StorageEpoch)`ごとに物理fileを分け、DB内のcanonical usage rowは`(reset_at,timestamp)`で一意である。異なるexact resetには同じminuteの有効観測を保持できる。必須`storage_partition` singletonの`partition_id`は
   その3値に結合し、`timestamp`は有効なUTC event秒を
   `floor(event_epoch / 60) * 60`へ変換したminute-startである。同じexact periodの同一canonical minuteのdistinct non-null quotaは
   最大1個、累積cost/tokenは列ごとの最大値を合成しない。取得順を証明できない旧重複rowのmigrationは、同minuteに実在して他の全raw vectorを
   componentwiseに支配する一つの観測vectorだけを保持する。支配vectorがない旧minuteはprojection外、cycle境界が
   一意でないcandidateは全rejectとする。採用済みの新しいcollection commitによる同minute更新は`HISTORY-CANONICAL-134`の通常収集規則に従う。元event秒は同一minuteの順序を検証するためだけに使い、canonical
   timestampへ書き戻さない。backup保持と単一表置換の詳細は`HISTORY-CANONICAL-134`を正本とする。
3. DBの記録データの書き込みはtransaction内だけで行う。busy、I/O、full、corrupt、schema不一致、migration中断はrollbackし、旧DBと旧メモリ世代を保持する。稼働account partitionでは、REST等のread-only snapshotが記録側のcommitを阻害しないようWALを使用する。既存partitionはidentity・schemaのread-only照合に成功した後でだけ書込み接続のjournal modeを変更する。backupはSQLite online backupによって未checkpointのcommitも含める。backup先を単一fileで復元できるDELETE journal modeへ確定してから検証・世代交代する。旧形式DBのfile置換migrationへWAL化を適用しない。
4. 有効なatomic snapshotだけを公開する。収集中のaccount usage candidateのcommit/publish admissionは現行の
   `(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, CollectorEpoch, CycleSeq)` tupleだけを正本とし、candidateの6要素が全て現行値と一致する場合だけDB、memory、REST、UIへ進める。`SupervisorLeaseIdentity`は同一profile serviceの単一publisher所有権を別に固定し、account generationの代用にしない。
   stale lease/epoch/cycleまたはtuple欠落・不一致はcandidateを破棄し、DB、memory、REST、UIを0変更とする。これは未commitの収集candidateを現ownerへ取り込む規則であり、検証済みの過去commitを現在の収集tupleへ付け替える規則ではない。RESTの歴史account読出しは、そのpartitionのidentity・registry所有区間・保存済みcommitを検証し、WIREのaccount frame admissionに従う。現在ownerのepoch/cycleと異なることだけを理由に歴史accountを拒否しない。surfaceが必要とするresource/pageの一部だけ、未検証のthread collection、途中までparseしたREST bodyを完成済み表示として公開しない。model集合の完備性はこのatomic性と別である。採用済みの既知modelと明示したpartial/unavailable状態を一組で公開することは許し、WIREのv4 currentでは取得成功なら`state=ready,models_status=partial`も有効である。`models_complete=false`だけで既知値を破棄したり、取得失敗へ変更したりしない。取得そのものの失敗と過去rangeの未解決はPRODUCTのlane分離に従う。
5. local usage JSONLとlive rolloutを同じrecord隔離規則へ丸めない。live rolloutではUTF-8、JSON、
   envelope、event kind、task-stateへの非影響を完全検証できない改行済みrecordを含むcycleはfail-closedにする。
   oversize recordを隔離できるのは、bounded streaming parserがduplicate/unknown envelope key 0の正規eventを
   完全検証し、livenessを変更しないtool payloadであると証明した場合だけである。証明不能、known state eventの
   型不正、invalid UTF-8/JSONは旧完全thread snapshot＋未確認を保持し、古い`task_started`をrunningへ流用しない。
   live rolloutのEOF直前の未改行tail（同一inodeへの追記待ち）だけは次cycleへ保留し、途中状態を公開しない。
   local usage側はRECORDER-MODEL-01のrecord隔離と差分区間資格を使う。改行済み不正recordは原本・未処理range・前回anchorを保持し、後続の独立して検証できたrecordの読取りを継続する。未判定のusage/controlを跨ぐmodel帰属は確定せず、後続counterだけで当該欠落の完備性を確定しない。未改行tailはcheckpointをその行の前に保留し、tailの途中までのusageをcommitしない。I/O・source差替え・source/account資格の失敗はそのsourceの未commit candidateをrollbackし、既にcommitしたprefixと他の有効sourceを保持する。資源上限で停止する場合も改行済みの検証済みprefixだけを同じrange/checkpoint transactionで進め、残suffixを未処理として保持する。raw本文を失わせるための削除・飛越しは行わない。
   一つの改行済みrecordについて複数の採否条件が不成立でも、同じsource identity・prefix generation・byte範囲・原本hashを指す未処理記録は一つにまとめる。独立した不成立理由は全て識別できる形で保持し、理由の一つを直しただけで残りも解消したとは扱わない。例えば同じtoken_countの累積counterとcontext pairが双方不正なら、双方の不正と前回counter/pairの保持を表す一件である。具体的な不正から必然的に導かれる一般的な拒否は別件にしない。例えば圧縮snapshotの不正が同じrecordのNoOp不成立を意味する場合、snapshot不正を保持すれば足りる。この集約は同じ原本recordの診断だけに適用し、別record・別範囲・異なるhash・独立したprefix欠損の統合や、競合値の上書きには使わない。writerは重複・競合candidateを引き続き拒否し、scannerが同じ拒否記録を重複生成して正常な別sourceのtransactionを失敗させることを認めない。未処理の集約・checkpoint・独立して有効な事実は同じusage transactionへ保存し、再起動・batch分割で不成立理由や数値が変わってはならない。
6. app-serverのtimeout・無応答・切断はlocal Session収集を停止する理由にしない。確認済みaccount partitionとsource cursorがある場合は通常周期で差分保存を継続する。
   確認済みsourceの意味と追記ごとの再検証は8.7のsource/account結合に従う。初回のRPC照合が未成立の新sourceを、障害中という理由で確認済みにしない。
   現在のaccountとの一致を確認できないsourceは他accountへ帰属させず原本を保持する。quotaは最終実測時刻と取得失敗を表示し、未観測の値を保存しない。
   外部API復帰後は新しい実測値だけを採用する。旧reset hintを現在のaccount authorityや継続収集の必須条件にしない。
7. 同一障害中にlocal JSONL全走査やapp-server再起動を無限反復しない。通常の全走査はquota cycle、明示更新、または一度だけの障害復旧に限定する。
   処理済みprefixのfingerprintが不変で未処理batchもなければ、Session sourceの再scan・差分writeは0とする。変更検出と検証結果の再利用条件は8.7「不変入力の再読省略と再検証条件」に従い、単に同じpath/sizeであることや、新しいcycleであることから判定しない。変化したsourceは1 cycleにつき1 scanとし、受理したSession batchを一つのusage transactionへまとめる。この回数は独立したquota原本保存・quota receipt/health transaction・maintenanceを合算した上限ではない。保存失敗で保持したexact pending batchの次scheduled cycleでの再試行と、新しいcollection cycleのcommitはPRODUCT `REST-172`および8.8に従い、不変なfingerprintだけでは抑止しない。
8. collector全停止中の未取得データは後から捏造しない。installed profileの所有はPRODUCTの
   `PROC-LAUNCH-01`に従い、`codex-info-recorder.service`がwriterとsingleton recorder lease、
   `codex-info-rest.service`がread-only loopback RESTを所有する。同じmanifest source generationの二役割を検証する。
   顧客操作は`$HOME/.local/bin/codex-info`がmanaged serviceへ収束させ、`--ui`のpayloadを含む別processを
   writer/REST ownerにしない。開発・互換用embedded payloadの引数なし/`--port PORT`だけは同一processで
   daemon+RESTを所有するが、installed launcherの更新・制御authorityではない。stop/disable/removeはDB、backup、
   hint、gap/recorder/control state、source JSONL、installed generationを保持する。
9. canonical DB profileごとに`MaintenanceOwner`を1つだけ許可する。起動時pruneの前に、writer admissionを止めた同一排他境界で
   4.2のSQLite online backupを1世代作成・検証し、過去のverified世代と合わせて最大3世代を保持する。初回に同じsnapshotを3個へ複製しない。世代が1個または2個であることだけではpruneを拒否せず、今回のbackupと既存verified世代を4.2の検証・rotation規則で確定する。backup失敗時はpruneを実行しない。検証失敗・writer競合時もpruneを実行しない。バックアップは`0600`、DBディレクトリは`0700`とする。
10. schema変更前にSQLiteの整合したbackupを保持し、複製で旧行・期間・checkpoint保持とrollbackを検証する。既存型・primary key・値を変えないnullable列追加、およびv16の既存token成分・固定family互換列のNULL許容への緩和と、exact resetを含まない旧unique indexの除去は、検証済みbackup後に既存account DBの単一transactionで行う。table再作成が必要でも全既存値をそのまま移し、stampまで同じtransactionに含める。旧readerへ未知componentを0として渡さない。
   account履歴の正規化は`HISTORY-CANONICAL-134`の専用transaction、非partitionの旧形式fileの変換は4.3の別名candidate・全行validate・atomic switchとする。方式を実装者が選び替えない。いずれも失敗時に旧DBやsourceを初期化・削除しない。
11. スレッドのライブ状態はDB履歴を根拠に再生しない。root/childとも、同一cycleで前後identityを検証した
    eligible Codex workloadの`canonical path -> nonempty ProcessIdentity set`にrolloutが存在し、最後のtask状態が
    runningである場合だけnative candidateにできる。`ProcessIdentity=(pid,starttime_ticks,exe_device,exe_inode)`とし、
    exact Codex Info artifactを祖先に持つ観測用app-server childは全Codex Info process分を除外する。
    identity変化、祖先不明、FD scan部分失敗、native DBのduplicate/root非到達/cycle/dangling/partialはcycle全体を
    fail-closedにし、旧完全snapshot＋未確認を保持する。完全受理済みREST PublicThread集合内のmissing parentだけは
    presentation orphanでありnative DB danglingの救済ではない。`docs/LIVE_STATE_DECISION_MATRIX.md`は
    このDATA契約から導出した非規範的な判定表とする。
12. Windows clientの設定永続化は`WIN-THEME-PREF-422`に従い、従来の`language`、`setupCompleted`、`connectionConfigured`、`timeZoneId`、
    `connectionProfile`、`connectionSelector`の6 keyを旧形式として読み、新規保存には`themeId`を加える。`connectionProfile`は`none|wsl|sshConfigAlias`のexact enum、
    `connectionSelector`はWSLのexact distribution tokenまたはliteral OpenSSH Host alias grammarだけを許可し、秘密、展開済み値、raw host/user/pathを0件とする。
    saved selectorによるauto reconnectはconnection値を再検証して行い、remote自動起動は`ArgumentList`と`BatchMode=yes`を使う。
    auth argvもsaved profileから構築するが、起動成功とstatus再確認を別stateに分ける。4-key recoveryはMain disconnectedとSettingsだけに残し、
    設定不正・接続失敗時は保存値とDBを破壊せずSettings recoveryへ戻す。RESTへselector/secretを送らない。品質確認の有無を設定の採否条件へ混ぜず、動作はこの契約とUX ownerで決める。

## 3. 失敗時の保持契約

次表のSQLite/集計再試行は、保存済み入力から作る派生projectionの責務である。未保存のquota応答を所有する原本保存workerは`QUOTA-DURABLE-604`第5項の1秒再試行・終了待機に従う。異なるownerの再試行条件を互いへ流用しない。

| 障害 | 保持するもの | 破棄するもの | 再試行 |
| --- | --- | --- | --- |
| app-server/REST停止 | 既存のquota、履歴、thread、DB | 未取得の新規値 | 次の明示/周期要求。local backfillは障害期間1回 |
| daemon/recorder unexpected exit | 直前の完全snapshot、DB、hint、source cursor、gap ledger、last committed recorder state | 停止区間の推測quota、未検証の再起動candidate | worker死亡は2秒以内に検知し次cycleまでに1回だけ再試行。process exitはsystemdが5秒後に再起動し続け、start limitで永久inactiveにしない。明示stop/disable/removeだけが停止を持続 |
| oversized/不正な1レコード | 同一ファイル内の前後の有効レコード | そのレコード | 次のcycleで再読込可能 |
| ローカル履歴のEOF未完了レコード | 直前の完全snapshot、DB | 不完全レコードを含むローカル入力 | 次のcycle |
| I/O、差替え、EOF以外の部分行、資源上限 | 直前の完全snapshot、DB | 失敗cycleの部分結果 | 次のcycle |
| SQLite busy | 旧DB、旧backup、旧メモリ/root | 2.000秒でrollbackした未commit transaction | same-callback/polling retry 0。次の通常scheduled cycleまたは明示操作で最大1 attempt、同じ2.000秒deadline |
| SQLite full/corrupt | 旧DB、旧backup、旧メモリ/root | 未commit transaction/candidate | same-callback retry 0。容量・明示restore等の原因解消後だけ新cycle |
| SQLite open/read/write I/O | 旧DB、旧backup、旧memory/root | 失敗batch、partial candidate | 同一callbackでは再試行せず次cycleまたは明示操作 |
| schema mismatch/migration失敗 | 旧DB、旧backup世代 | 新DB候補 | migrationを修正して別途再実行 |
| backup/rotation/restore失敗 | 現行DB、検証済み3世代、旧memory/root | partial backup/candidate、未検証世代 | 次のmaintenanceまたは明示restore。自動復元0 |
| confirmed daemon stop gap | 前後と復元済みのvalid sample、gap ledger、旧完全root | gapを埋めたとする推測値・複製値 | source cursor確認後にのみ確定。原本復元とmetric別表示coverageはPRODUCTを参照。表示専用破線をDBへ保存しない |
| status/details pair不一致・stale admission tuple | 直前の完全status/details pair、現行`(ProfileScopeId, AccountScopeId, StorageEpoch, auth_epoch, CollectorEpoch, CycleSeq)`とprofile publisher lease | 片側だけ進んだcandidate、stale account/lease/epoch/cycle candidate | 次cycleで現行tupleを再取得・検証し、DB/memory/REST/UIは不一致中0変更 |
| 認証喪失・アカウント切替 | 非認証root（旧account表示は消去）と非破壊DB | 旧accountの画面公開値 | 認証成功後に再読込 |
| reset hint expired / auth epoch切替 | source log、旧DB、旧root、tombstone hint | `now >= reset_at`の旧期間scan/row、次期間への誤帰属、旧epochの公開値 | 旧hintをexpiredまたはtombstonedとして無効化し、source logを保持。current epochのfresh authenticated hint後だけ次のbounded one-shot |
| UI/REST publish失敗 | DBのcommit済み世代、旧表示snapshot | 失敗した公開試行 | 次のroot更新または明示操作 |
| thread DB履歴とlive pathの不一致 | 旧完全thread snapshot、quota、履歴、DB | DBだけから復元した停止済みroot/child | 次のcycleでactive pathとterminal状態を再検証 |

## 4. 同時実行とバックアップ

- すべてのcollectorは`UsageStore`を通してSQLiteへ書く。直接JSON、直接SQL、別形式の履歴DBは禁止する。
- SQLite transaction lockとbounded busy timeoutを正本とする。ロックを無視した上書き、DB削除、DB再生成は禁止する。
- `usage_history.sqlite3.bak.1`〜`.bak.3`は時系列の完全SQLite snapshotであり、同じ件数である必要はない。各世代は`PRAGMA quick_check`と再読込で検証する。
- backup、prune、migrationの失敗は元DBを変更しない。backup/migration候補の検証前は元DBをread-only接続だけで読み、schema・index・permissionを修復しない。pruneはbackup成功後だけ許可する。
- `UsageStore::migrate_verified`は旧形式の非partition履歴だけを対象とする。account partitionを渡した場合はtransform・candidate作成・切替を行わず拒否する。account DBの履歴変換は`HISTORY-CANONICAL-134`の検証済みbackup後の専用経路だけが所有し、成功時にcanonical単一表と`PRAGMA user_version`を同時commitする。
- backup世代の復元は、対象プロセスを停止し、現在DBを別名退避してから、quick check・schema check・row/hash監査を通した世代だけで行う。通常起動が自動復元を試みてはならない。

### 4.1 RecorderSupervisor、lease、backfill、gap

- worker supervisorの状態は`Absent → Starting → Running → StopRequested → Stopped`または`Failed`だけを進む。
  writer threadの実終了は1秒owner loopで`JoinHandle`から確認し、processを非0終了させる。DBのbusy、full、
  readonly、corrupt、timeout等の業務結果はworker死亡へ変換しない。`degraded`とexact pending batchを保持し、
  同callbackでretryせず次のscheduled cycle（60秒以内）に1回だけ再試行する。失敗回数だけで正常に生存するworkerを終了しない。
  user-systemdは`Restart=always`、`RestartSec=5s`、`StartLimitIntervalSec=0`でunexpected exit後も再起動を継続し、
  start limitによる永久inactiveを作らない。反復失敗は失敗のまま可視化し、記録停止を正常状態へ読み替えない。
- installed構成ではsystemdがRecorderとread-only RESTの別serviceを監視する。各processの役割・
  同一generation・lock/listenerの結合はPRODUCT `PROC-LAUNCH-01`に従う。
  両serviceの`ExecStartPre`はpersistent installerのstartup reconcileをbounded実行する。active transaction journalが
  switch後phaseならread-only検証して継続し、journalなしならL1 install lock下のshared resolverを使用するが、
  startup modeは自分自身をrestartしない。更新試行が失敗しても、完全なcurrent generation、committed以外のtransactionなし、
  listenerなしを再検証できた場合だけ更新をdeferredとしてExecStartへ返し、既存世代の記録を開始する。local不整合、未解決transaction、
  listenerの存在または曖昧性は成功へ変換しない。launcher `--ui`はmanaged ownerへ収束後に別UI processを追加するだけである。
- singletonのscopeは正規化canonical DB pathとprofileの組である。lease schemaは最大4KiBのUTF-8 JSON
  `recorder-lease-v1`（`pid`、`process_start`、`owner_nonce`、`canonical_db_path`、`device_or_volume_serial`、`file_index_or_inode`）とし、
  writer processは同じ`UsageStore`のtransaction/upsert契約を使う。通常のrecorder二重起動はlease前にno-op、競合試験だけが別の許可済みwriter processを使い、
  製品経路でleaseを無効化しない。
- recorder状態はowner-only 0600の`recorder-state.json`へatomic writeし、schema
  `codex-info-recorder-state-v1`、必須key集合
  `schema,pid,process_starttime,owner_nonce,write_state,partition_id_hash,data_generation,collector_epoch,
  cycle_seq,last_commit_unix,updated_at_unix`を持つ。新producerはcompile済み配布versionの診断metadata `recorder_version` も公開する。consumerは旧11-key公開も受理し、この診断metadataを既存のreadiness/DB authority判定へ追加しない。`write_state`は
  `idle_no_account|ready|degraded`だけである。lock/state遷移またはDB transactionのacknowledged commit後だけ
  対応fieldを更新し、heartbeatからcommitを捏造しない。account admitted時は新規usage rowが0件でも各scheduled
  generationをcommitし、`last_commit_unix` freshnessを150秒以内に保つ。consumerは一回のstate検証で同じOS UTC Unix秒`now`を使い、
  全write stateのheartbeat `updated_at_unix`と、非nullの`last_commit_unix`を、それぞれ正の整数かつ
  `now-150 <= value <= now`の場合だけfreshとする。150秒は既定60秒収集の停止を検出する製品側の猶予であり、
  providerの観測時刻・公式window・DB履歴・公開基準Tへ適用しない。未来側に追加の猶予を設けない。
  時計の逆行でこの条件を満たさなくなったstateも失効とし、時刻のclampやcommit補完をせず、同accountの旧frame/pairと
  reader 503を保持する。次の通常owner監視でfreshな完全stateを確認した後にだけ通常のcommit照合へ戻る。
  account未確定時は`idle_no_account`とし、
  架空partition/commitを作らない。古いheartbeat、古いcommit、`degraded`をmanaged recorderのhealthyへ読み替えない。
- recorder-stateのcommit fieldは`data_generation,collector_epoch,cycle_seq,last_commit_unix`の組で検証する。
  `ready`は全field必須、`idle_no_account`はpartitionとこの組を全てnull、`degraded`は有効なpartitionと
  最後のacknowledged commitの完全な組、または組全体nullだけを許す。組の一部だけnullは不正である。
  `degraded`の完全な組は最後のcommitを示し、失敗したattemptのcommitを示さない。組全体nullはcommit未確認を示し、
  0、前回の値、最新attemptの値で補完しない。この形はpartition admission中とcommit失敗後の両方に生じ得るため、
  account切替の有無は検証済みregistry/partition identityから判定し、nullだけからlogoutを推論しない。
  `degraded`だけからcurrent quotaの取得失敗、履歴pending、またはDB読取り失敗のいずれかを選んで捏造しない。
  quota/models/historyの公開状態はそれぞれ受理したDB domainのauthorityで決め、recorder ownerの健康判定とは区別する。
  commit未確認・古いcommit・owner失効の間は同accountの旧root/pairを保存した上でsnapshot読取りを503とし、
  新しいError generationや正常な空データを生成しない。registryが確認したaccount境界は別途strict boundaryを公開する。
  DB側のcommit組の正本は`collection_generation` singleton 1の`data_generation,collector_epoch,cycle_seq`であり、
  domainと同じread transactionで検証する。generation/sequenceはcanonical UInt64十進TEXT、epochは32桁lowercase hex
  のUInt128またはNULLとし、epochがNULLであることとsequenceが0であることは同値である。初期値は0/NULL/0であり、
  epochのない旧DBへepochを捏造しない。collection tableがない旧形式は既存のgeneration fallbackだけを使い、
  recorderの完全commit組と照合済みであるとは主張しない。存在するcollection tableの欠損singleton、不正な型・値・組は
  legacy fallbackへ逃がさずcandidateを拒否する。durable observationのtimestampをcollector sequenceへ代用しない。
  state fileのunknown/missing/duplicate key、型不正、非canonical identityを拒否し、PIDとprocess starttimeが
  生存する同じprocessを示すことを確認する。診断用versionが読めることをowner検証の代わりにしない。
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
  処理済みprefixのfingerprintが不変で未処理batchもないcycleはJSONL全走査とSession差分writeを0とする。新しいcollection cycleのcommit、独立quota transaction、保持したexact pending batchの再試行は第2章7および8.8の別条件に従う。
- app-server outage epochでpersisted `history/usage_reset_hint.json`とappend-only sourceが両方有効なときだけ、backfill latchを1回消費する。
  hintは`active → expired`または`active → tombstoned`へ一方向に進む。`now >= reset_at`になった旧hintはexpiredとして扱い、
  新規source変更を理由に旧期間のscan/DB row作成をせず、source logだけを保持する。quota cycleまたは明示更新が別に起動しても、
  current authenticated reset periodを検証できない限り旧期間へ帰属させない。current AuthEpochに束縛された
  fresh authenticated hint（`reset_at > now`、同じcurrent source identity、旧hintでない）が受理された後だけ、新期間へ帰属する
  bounded one-shot backfillを開始する。hint、cursor、source identity、AuthEpoch/nonceが不一致ならcandidate全体を破棄し、gapを埋めず旧rootを保持する。
  logout、token失効、AccountKey変更ではAuthEpochを先に増やし、persisted hintを`state=tombstoned`へatomicに更新してcursorと公開候補を無効化する。
  hintへlogin ID、raw AccountKeyその他の個人識別値を保存せず、DBの表示専用`storage_partition.login_id`も
  epoch境界の判定には使わない。opaque nonceとprocess内AuthEpochだけでepoch境界を検証する。
- daemon停止区間は`RecorderGapLedger`がsource identity/cursorと停止・再開monotonic時刻から回収不能を
  確定した場合だけgapとする。既存REST v1のexact 13-key detailsにある`history_gaps`へconfirmed rowだけを
  projectionし、timestamp不連続だけではmarkerを作らない。確定gapを埋めるDB値に補間、quota/残量推測、旧値複製を使わず、backfillが成功した区間だけ実sampleで回復する。表示専用の破線・metric別coverageはPRODUCTの`G137-GRAPH-01`だけに従う。
  quota gapのsource closureは同じaccount/periodのfresh authenticated quota結果だけで判定し、Session inventoryの
  2GiB選択外file、model scanの完結性、REST clientの有無へ依存させない。Sessionのtoken/model/taskだけをquota gapのclosure証拠にしない。Sessionのcached quota payloadはfreshness証明がないためquota gapの回復証拠にしない。公式取得journal・receipt・旧durable observationの復元だけを8.7.0に従って扱う。最新quota一つで停止中の全minuteを回復したことにはしない。quota sourceの再検証が未完なら`pending`を保持し、完結したが回復できない区間は`confirmed`とする。
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

### 4.3 Schema更新と旧履歴migration

- **account DB startup**: `open_partitioned`はaccount identityと対応schemaをread-only probeで確認する。旧schemaの場合は、全初期化済みpartitionを列挙するrecorder startupだけがverified backup後の専用transactionを所有する。既にcanonicalな入力では`RECORDER-MODEL-01`のschema変更だけを行い、既存historyのkey・値・sidecarを再分類・再集計しない。旧混合履歴のcanonical化が必要な入力だけに`HISTORY-CANONICAL-134`の採用元・局所除外・rollback規則を適用する。両者をschema versionが古いという理由だけで混同しない。transaction失敗時はschema versionを進めない。
- **unsupported account migration**: account DBへ並列raw/canonical表やreader補償を追加する自動経路は持たない。`migrate_verified`へaccount partitionを渡しても旧9列履歴への縮退変換はせず、元DBを変更せずに拒否する。
- **legacy history migration**: 明示操作で旧形式の非partition履歴を変換する場合だけ`migrate_verified`を使う。read-only sourceから別名candidateを作り、型・値・一意キー、`quick_check`、row count、fingerprint、reset-period境界を検証後に切り替え、旧DBとbackupを保持する。失敗時は元DBを保持し、同callbackで再試行しない。

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
| local Session inventory | depth・file数と数え方はSECURITY Required controls 4 | depth/file-count上限超過、またはinventory全体の列挙・総件数の検証不能では、そのinventoryを使う新しいSession usage候補全体を保留する。列挙順の先頭だけを正常inventoryとして採用せず、旧DB・checkpoint・model値と原本を保持する。quota/threadの独立laneを空にしない |
| local JSONL record / whole-file snapshot / selected session prefix | line 4MiB / whole-file snapshot 256MiB / latest whole-file prefix 2GiB、decode前の受信bytes。resident recorderの継続sourceは既存のcycle byte budgetと検証済みLF prefixで分割し、総file bytesをwhole-file snapshot上限へ代入しない | 第2章のlocal usage隔離規則を使う。改行済みのoversize/invalid recordは原本と未処理rangeを保持し、当該fileのrecorded marker=0。未改行tailはその行の前でcheckpointを保留し、先行する検証済みLF prefixまでのcommitは許す。個別fileのI/O・source差替え・適用対象のsize境界・metadata・containment違反は当該sourceの未commit candidateをrollbackする。全inventoryのaggregate超過だけではrollbackせず、最初に収まらないfile以降を保持中overflowとする |
| local Session JSONの構造 | 同時に開くobject/arrayは最大128。root objectを1、内側のobject/arrayごとに1加算し、文字列の長さやfield数を深さへ加えない | 129段目を必要とするrecordは不正recordとして隔離し、元bytes・未処理rangeを保持する。前回の有効anchorを保持し、当該recordを跨ぐusage/controlの帰属・完備性を推測しない |
| live rollout record / original prefix / active paths | line payload 4MiB / active path 1024。総file sizeによる256MiB拒否を適用せず、先頭から固定EOFまでbounded-memory streamingで検証する。initialize・RPC・Session読取り・prefix hash再検証は同じ15秒deadlineを共有し、buffer補充とhash読取りの各段階で期限を照合する | oversizeはstreaming envelopeでliveness非変更を完全証明した場合だけpayload隔離。invalid UTF-8/JSON/envelope/state event、identity/ancestry/FD partial、期限超過はlive cycle全rollback。全pathの完全な原本検証、account/owner前後確認を終えるまで部分snapshotや新cacheを公開しない。従来whole-file snapshot parserの256MiB境界は別entryに保持する |
| internal validated snapshot | 単一のlive collection入力のcanonical JSON 1MiB。保持履歴index・period directory・全page集合は対象外 | 当該collection candidateをrejectし旧snapshot保持。公式quota原本のdurable化と独立。REST transfer bodyへ流用しない |
| REST response headers / health・error / snapshot body | WIRE ownerのencoded byte境界を参照。別のstatus上限を定義しない | Content-Lengthの事前検査と最初の超過byteで停止。candidateのみ拒否 |
| 通常収集のtransaction batch / retry | usage rowsは最大1024かつbatchのUTF-8 JSON表現1MiB、recorded session markerはinventory上限と同じ最大4096 rows、backfill latch=1、scan/restart retry=1 | 検証済みLF prefixのbatchとrange/checkpoint/未処理状態を一括commitし、未commitの残suffixを次cycleへ保持する。markerは同cycleのusage transactionへ同梱し、全fileを完備と誤称しない。途中のtransactionを公開しない |

上表の数値は本製品が選んだ処理・保存・輸送の安全境界であり、Codexが返す値の上限や履歴domainの妥当性ではない。MiBは`2^20` bytes、GiBは`2^30` bytesとする。local lineは終端LFを含む元bytesを数え、file/prefixは読み出す元file bytes、snapshot/batchはstoreへ渡すUTF-8 JSON payloadの実byte数を数える。JSONのfield順や内部chunk分割で最終的な採用event・knownness・model/period帰属を変えてはならない。budget到達を未使用・完備・処理済みへ変換せず、前回baselineと区間資格、未処理rangeを保持する。

inventoryのdepth・件数の上限と数え方はSECURITYのRequired controls 4を参照する。JSONの構造深さはpath depthとは独立した製品入力境界である。streaming実装が本文を保存しない場合も、opaqueなobject/arrayを含む全構造についてこの境界とJSON/UTF-8の正常性を検証し、本文を途中で切って正常recordにしない。model/IDのscalar上限は8.7の識別子の資格であり、内部buffer byte数を別の採否上限へ置換しない。

通常収集の1024 row/1MiBは、一回のbounded appendを分割する予算である。`HISTORY-RECOVERY-614`の起動時quota/model再投影・原本で証明した派生event修復、およびschema移行のtransactionへ流用しない。それらは各ownerの一括commit/rollbackを守り、対象partition、保持対象期間、検証済みaccepted prefix、依存する全suffixで有限範囲を閉じる。SQL実行や入力の読取りを小さな内部chunkに分けても、中間commit・部分成功公開を行わない。必要資源を確保できなければ元DB/rootと検証済みbackupを保持して当該laneを未成功とし、件数を減らして成功にしない。

session selectionとcache fingerprintは同じmtime/path降順vectorを使い、overflowを含む直前verified inventoryをCollectorEpoch内だけ保持する。SQLite `recorded_sessions` はcanonical sessions root identity、UTF-8 root-relative normal path、size、mtime nanoseconds、device/inodeの完全fingerprintを複合keyとし、旧source markerを上書きしない。同じaccount/sourceの検証済みcheckpointがあるfileは8.7に従い再開する。rotation/truncation/prefix不一致で旧anchorがある場合は8.7.2へ渡し、EOFへ進めて旧anchorを上書きしない。identity boundary時の所有を証明できない既存fileは現在値を未帰属baselineとして保持し、境界以前を新accountへ自動加算しない。未checkpointであることだけをfully attributedの根拠にしない。新規fileも先頭の観測をbaselineとする規則とsource/所有検証に従い、過去範囲は検証済み原本復元でのみ帰属する。cleanupはcommit後の別maintenanceであり、offset 0からEOFまでfully attributedなfresh read-only marker、regular/non-symlink containment、直前まで同一のfingerprint、bounded `/proc` scanによるCodex open-FD不在がすべて成立したoverflow fileだけを削除する。missing、unmarked、baseline、legacy、partial、changed、active、selected、DB/process scan失敗は保持し、同callback retryは0とする。file削除後のmarker削除に失敗した場合はfingerprint-boundのstale markerを保持し、usage history、durable state、verified backup、reset hint、delegation recoveryを変更しない。

### QUOTA-DURABLE-604: 公開窓・原本・時計の分離

以下は新規の保存期限追加ではなく、「履歴3暦月」と「取得原本を消さない」の適用対象を確定する定義である。

- **公開基準時刻T**: publisherが候補構築開始時に一度取得するOSのUTC Unix秒。公式quotaの取得順序、`observed_at`、deadline、Sessionの最新時刻で代用しない。同じ候補のretained/details/period/gap投影は同じTを使い、成功した候補と一緒にimmutableに保持する。HTTP要求ごとに時計を取り直さない。新しい公式入力がなくても既存の60秒鮮度cycleで窓を進める。
- **原観測と公開観測**: quota receiptの元`observed_at`は受理原本の事実として保存し、取得順序を置換しない。公開details/currentの同名fieldはPRODUCT `CURRENT-PERIOD-BOUNDS-129`の同periodのデータ観測終端であり、quota以外の有効観測を含み得る。これはTではない。currentの終端をquotaの元観測時刻で切って後続modelを失うこと、逆に公開観測終端でquotaの原時刻を書き換えることを共に禁止する。
- **保持開始C(T)**: TをUTCの年月日時分秒に変換し、3暦月を減じ、対象月に同じ日がなければその月末へ切り詰める。時分秒は保持する。windowは`[C(T), T]`。例: 2026-05-31 12:34:56→2026-02-28 12:34:56、2024-05-31→2024-02-29。公式windowの長さとこの保持月数は独立する。
- **観測単位**: rawの元時刻を保存したまま、canonical minuteの代表時刻が`[floor(C(T)/60)*60, floor(T/60)*60]`にある履歴点を対象にする。これは切断された最初の1分を失わないための表示粒度であり、Cを1秒早める規則ではない。gapは`[C(T),T]`と交差する部分を投影し、元ledger区間を変更しない。
- **公式期間と境界minute**: canonical保存形式のcurrent modelにも、公式起点`reset_at-window_seconds`と交差する最初のminuteを含める。ただしcurrent modelも同じ保持窓の対象であり、代表時刻の下限は`max(floor((reset_at-window_seconds)/60)*60,floor(C(T)/60)*60)`、上限は`min(reset_at,floor(T/60)*60+59)`とする。同じexact reset・所有区間の検証を必須とする。元時刻を保存するlegacy形式では下限を`max(reset_at-window_seconds,C(T))`、上限を`min(reset_at,T)`とし、canonical minuteとの同一視で過去範囲を広げない。公式起点・reset自体は丸めず、隣のresetの観測を取り込まない。公式windowが3暦月より長くても保持開始を延長せず、起点metadataは公式値のまま保持する。保持内のmodel観測がなければWIREのunavailableを使い、保持外の旧vectorや`session_model_totals`で補わない。取得順で決まる現在quotaはこのmodel保持窓とは独立である。
- **同minuteの異なるreset**: 複数model観測同士、またはquota receiptとmodel観測のexact resetが異なる場合、retained履歴では各`(reset_at,timestamp)`を保持する。別resetのquotaを当該modelへ結合せず、quotaのminute代表を選ぶ処理で別resetの有効modelを消さない。quota-only側へmodel値を複製しない。互換detailsもWIREの同じexact keyごとの規則を使い、輸送窓や件数上限を保存値・retained履歴・独立currentの資格へ逆流させない。
- **履歴の独立した元時刻**: 過去periodの観測終端はPRODUCTのhistorical metadata規則へ従い、保持内に採用した当該periodのreceiptの元秒も使う。canonical minuteの先頭へ切り下げて元秒を失わない。Session原本のwindow証拠は、そのaccountの所有時刻と公式window内にあり、元観測が公開Tの対象minuteまでに入る場合だけorigin候補にする。別periodのcurrent observed_atや後続periodの起点を資格上限にせず、Tより後のminuteの原本を先取りしない。originの競合・根拠不足はnullとして保持し、別periodの時計や最初の保存時刻で補完しない。
- **origin競合の解決条件**: currentの公式windowは現在quotaと同じ最新受理receiptを使う。過去periodでは、採用対象receiptの同じexact resetに異なる公式起点があればoriginを不明にする。取得順で後のwindowを期間全体の唯一の起点とはみなさない。receiptだけで起点が一意ならそれを使い、receiptに起点証拠がない場合だけ受理済みSession windowの一意性で補う。receiptの競合を一つのSession windowで消さない。不明な過去periodのstart/endはPRODUCTの観測・gap境界を使い、競合する起点に合わせて有効観測を切り捨てない。
- **directory**: 保持内の観測またはgapに結び付くexact IDと現在IDから作る。保持外になったreceiptをdirectory全体へ再加算しない。現在quotaは窓外の時刻でも永続取得順の最新受理receiptを使い、clockを根拠に古いquotaへ戻さない。現在IDのmetadataは残し、保持内観測がなければsampleは空である。
- **物理pruneの対象**: 既存の派生表示履歴（`usage_history`とそのmodel sidecar、durable observation）の保持外部分だけ。pruneの判定も同じUTC暦月関数を使い、canonical最初のminuteを残す。通常のwrite transactionの中で行う。published snapshotが参照する確定結果を途中で書き換えない。
- **物理pruneの対象外**: quota取得原本、`quota_receipts`の取得順序・適用済み原本ID、Session原本と既存のsource checkpoint、account ownership、検証済みsource pointer、backup保護対象。これらは回復と再処理の根拠であり、表示窓を理由に削除しない。別の保存期限を暗黙に追加しない。全ledgerを履歴publishのたびに再生せず、indexed time rangeと独立した最新取得順位置を読む。
- **再生・復元**: 適用済み原本IDは再度追加せず、保持外の派生minuteをjournal再生だけで復活させない。元の観測時刻と現在のC/Tが復元範囲を決める。回復の採否は`HISTORY-RECOVERY-614`のaccount/window/hash/成分一致で決め、retentionを広げるために時刻を付け替えない。
- **時計の逆行・未来入力**: 次candidateはその時点のTで窓を求める。逆行により窓が広がっても、既に正規prune済みの行を残存していると仮定しない。保持内へ戻った範囲に検証可能な原本があれば通常の復元条件で再構成し、なければ未記録として区別する。未来時刻の履歴入力は原本を保ち、Tを超えるminuteを過去のグラフへ移さない。currentの公式観測時刻は事実のまま表示する。OS時計の正確さは外部依存であり、receiptの時刻大小を新しい期間の採否へ流用して解決したことにしない。

長い公式windowの境界例: T=`2026-10-16T00:00:00Z`ならC(T)=`2026-07-16T00:00:00Z`である。同じexact resetの公式起点が`2026-06-16T00:00:00Z`でも、`2026-07-15T23:59:00Z`のmodel観測はcurrent/historyとも保持窓外、`2026-07-16T00:00:00Z`の有効観測は対象になる。前者しかなければmodelはunavailableであり0ではない。公式起点は6月16日のままで、現在quotaは最新の受理取得順を使う。この例は保持窓と公式windowの共通部分という同じ規則の適用であり、長いwindow用の別保持期間を追加しない。

quota current、model latest、thread membership、history coverageは別の資格を持つ。一つの正常なreset追加やhistory転送制約で他の有効な値を消さない。明示logout・account境界だけはPRODUCTのstrict empty規則を適用する。原本に存在しないzero/100%は記録せず、UXの表示専用anchorをDBへ逆流させない。

current modelを公開する読取りは、全履歴の描画用projectionを前提にしない。quota取得順からcurrent exact reset/windowを決め、
所有区間・Tと、そのcurrent windowと保持窓の共通部分に属する最新の保存観測keyを選び、そのkeyのmodel vectorとsource/集合完備を直接検証する。
数値vectorは`usage_model_history`、sourceと空集合の完備性は対応する`durable_state`観測から読む。6列の
`session_model_totals`単体にはreset/元観測時刻/source/集合完備がないため、これだけで確認済みcurrentとしない。
最新観測のsourceが数値を公開できない場合は、過去の完全観測へ戻して新しい完全値と主張しない。既存のlossless legacy資格と
raw成分の検証をそのまま適用し、有効なmodelだけを残してpartial、有効行なしはunavailable、確認済みの空完全集合だけはcompleteとする。
model laneのschema/SQL/成分不良はその資格の不成立であり、独立したquotaの取得失敗を捏造しない。v4 currentの取得状態は
受理したquota/acquisition状態、models_statusはmodel資格でそれぞれ決める。全DB connection/identity/transactionが成立しない失敗は
WIREのreader 503であり、資格のない部分データからcurrent frameを構成しない。履歴固有の失敗はhistoryだけを利用不能とし、
独立に確認したquota/model/threadを保つ。

同じexact resetで公式window長が変わった場合も原本を保持する。currentの公開資格は最新受理windowを使い、その外の旧観測を
current用へ移動・clamp・ゼロ化しない。counter/checkpointはwindow長の変化だけでreset/rebaseしない。current window内に新しい
model観測がまだなければ、旧window外の値を新しい確認値へ昇格させずpartial/unavailableを使う。completed periodでは全保存事実から
historical metadataを求め、公式originが一意でなければPRODUCTのorigin不明・最初の観測/gapの規則を使う。window長の恒常性を
providerの保証として仮定しない。

## 5. 変更管理の制約

データ保護対象ファイルの変更者は、次を満たさない限り完了を宣言してはならない。

1. [製品要件](PRODUCT_REQUIREMENTS.md)または本書の該当箇所を更新し、要求と失敗境界を逆引きできる。
2. 既存DB行のread-only row count/hashを変更前後で比較する。減少・訂正を許すのは、通常の3か月prune、`HISTORY-CANONICAL-134`の検証済みbackupを伴うduplicate/非比較minute/orphanの除外、PRODUCT `HISTORY-RECOVERY-614`の「原本が証明する派生eventの修復」、および本書8.7の「数値eventの公式期間所属」に従う所属再判定だけである。canonical除外と原本修復はそれぞれの全資格を満たし、採用元・除外/訂正理由とbackup内の原形を対応付け、依存するevent・合計・履歴を正本の計算結果と照合する。所属再判定は、原本資格済みの同じ数値eventに対する公式証拠集合の変更を必要とし、source数値・model・checkpointを保持したまま旧新periodの全依存prefixを一括変更する。同節の旧所属provenance・未所属の既知消費・旧利用記録保持とrollbackを検証し、同じ証拠での再実行はno-opとする。通常の加算や既存event集合を変えないquota/minute再投影を、この再判定へ偽装しない。どの経路も各正本の全条件を満たさない削除・訂正や根拠のない減少はFAILとする。schema v16のNULL許容・unique index修正migrationだけでは行数・値を減らさず訂正もしない。
   表示履歴の保持は本書の3 UTC暦月規則に従い、期間一覧は同じaccountの保持内indexを扱う。選択期間は全保持点を有限pageで取得し、全page合計44,640点には制限しない。互換detailsだけ直近31日とする。取得上限を保持期間の短縮へ読み替えない（#604 / #646）。
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

本節の規則はDATA ownerの規範である。旧抽出工程のstatus値を実装の分岐条件にしない。製品・実機の品質確認とRelease可否は、仕様の確定とは別に最新の実物と受入証拠で判断する。

### 8.1 共通採用規則

- HTTP handlerはread-only consumerであり、health/status/details、404、405、server errorのいずれでも
  SQLite transactionを開始せず、DB/WAL/SHM、backup、migration、checkpoint、PublishedPairを変更しない。
  HTTPとは独立した背景readerだけが、WIREのreconciliation入力に従って一つのread transactionからcommit済みdomainを読み、
  検証後のimmutable frameを公開できる。背景readerもDB write、schema変更、backup、checkpoint、Session再収集を行わない。
- data resourceの組合せはPRODUCTのsurface単位のatomic admissionに従う。split APIではMain/Graph/Threadsが必要とするresourceだけを同じpairで受理する。healthや別surfaceのpairとの同時一致を要求しない。旧一体型detailsは一応答だけをrootとして採用し、status/別versionとのfield mergeをしない。
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

HTTP要求が公開済みrootを取得できない場合の503、応答生成の500/切断はWIRE ownerへ従う。その要求でDBやpublished pairを変更せず、clientは該当surfaceのlast-goodを保持する。一方、publisherが受理済みquota・有効model・threadと、真正の履歴projection失敗を一つの新generationとして構成できる場合は、`QUOTA-DURABLE-604`に従って旧v1〜v3のcurrent/detailsをerror、履歴resourceを利用不能として公開する。独立したv4 currentのstate・models_statusは履歴失敗だけで変更しない。recorderがadmissionした取得状態の失敗と、REST reader/publisher自身がcommit済みdomainを読めない失敗の区別はPRODUCT `REST-172`とWIREのHTTP失敗境界へ従い、後者からError generationを作らない。旧status/detailsのS1+D0禁止は版を混ぜる採用の禁止であり、独立surfaceの正常更新を止める規則ではない。
logout/account境界のstrict emptyはPRODUCTとWIREのsecurity visibility transitionに従い、旧accountを一括で消去する。新しい境界generationの公開と、clientで旧accountを不可視にする処理を区別し、HTTP要求からDBやpublicationを書き換えない。

### 8.4 DP-REST-003 / RC-141 — request resource owner

request line、header、body、connection、timeout、keep-alive、shutdown drainの値はREST wire ownerへ委譲し、
data ownerは全rejectでDB、memory pair、settings、checkpointのbefore/after hash一致を要求する。connection/request
identityはlistener generationへbindし、shutdown開始後の新規request admissionは0件とする。

### 8.5 DP-REST-004 / RC-142 — read-only effect分類

許可する副作用は、当該request lifetime内のheap buffer、bounded in-memory counter、loopback socketのread/write、
read-only file open/stat、および`REST-LOG-RETENTION-482`だけが所有する固定fieldのfailure logである。この診断logのappendと期限切れ同形式log削除は、別の診断ownerへ明示的に限定したeffectであり、失敗してもHTTP/DB/publicationを変更しない。禁止するeffectは、それ以外のpersistent access/error log、Event Log、persistent metric、
cache/temp file、registry、child process、非loopback socket/DNS、file create/write/rename/delete/fsync、
SQLite transactionとDB/WAL/SHM mutationである。OSがread-only openに伴って更新し得るatimeはproduct data mutation
の成功条件に使わず、DB content/inode/WAL/SHMとproduct syscall traceを判定ownerにする。診断ownerのpath/field/保持条件を満たさない書込みを含むallowlist外のeffectを
検出したresponseは成功証拠にせずreleaseをHOLDにする。

### 8.6 DP-REST-005 / RC-143 — storage partition identity

partition keyは`(ProfileScopeId, AccountScopeId, StorageEpoch)`であり、各partitionを
`history/accounts/v1/<AccountScopeId>/epoch-<StorageEpoch>/usage_history.sqlite3`へ物理分離する。
usage rowの一意keyは各物理DB内の`(reset_at,timestamp)`である。DBにはexactly oneの
`storage_partition` rowを置き、schema/profile/account/epoch/partition IDと`quick_check`が一致しないfileを開かない。

- `ProfileScopeId`: 保存profileを作成した時に生成する128-bit random opaque ID。raw WSL distro、SSH alias、pathを
  DBへ保存しない。
- `AccountScopeId`: authenticated app-server ownerがcanonical AccountKeyから、owner-only 256-bit install keyを使い
  `HMAC-SHA-256("codex-info-account-scope-v1" + NUL + AccountKey)`で生成する32-byte値。raw AccountKeyとtokenを
  DB・hint・logへ保存しない。stableな`account/read`で確認したlogin IDだけは表示専用値としてowner-only registryと
  当該account DBへ保存できるが、scope、path、hint、journal、logまたは認証判定へ使用しない。
- `StorageEpoch`: partition作成時のmonotonic unsigned 64-bit値。account/profile不一致やHMAC key欠落時は新規writeと
  publishを0件にし、自動的な空partitionや推測mergeを作らない。

永続identityの互換表現は、ProfileScopeIdが16 raw bytesのlowercase hex 32文字、AccountScopeIdとpartition IDが32 raw bytesのlowercase hex 64文字、install keyが32 raw bytesのlowercase hex 64文字である。長さ・文字種を相互に代用せず、大文字や短縮形を黙って正規化しない。StorageEpochは1..UInt64最大値でprofile内一意とし、再利用・wraparoundをしない。

partition IDは、install keyのraw 32 bytesを鍵にした`HMAC-SHA-256`で、messageをASCII `codex-info-storage-partition-v1`、1 byteのNUL、ProfileScopeIdのraw 16 bytes、AccountScopeIdのraw 32 bytes、StorageEpochのunsigned 8-byte big-endianの順に連結して計算し、lowercase hexで保存する。この既存の保存互換契約は、path・login label・JSON表示文字列から推定する別IDへ変更しない。registryとDBのschema/profile/account/epoch/partition IDを同時に照合し、部分一致では採用しない。

同一account/profileの再認証は同じpartitionを再利用し、別account/profileは別partitionにする。同時書込みauthorityは
現在認証済みpartition一つだけとし、非認証accountを推測更新しない。初期化済みpartitionは旧accountを含めて
read-only catalogへ公開し、画面は利用者が明示選択した一つだけを読む。旧partitionを削除・混合せず、account切替時は
別partitionのpair、cursor、last-goodを再利用しない。HMAC install keyは0600 owner-only fileへatomic保存し、
欠損時は既存AccountScopeIdを再生成せずrecovery-requiredとする。

canonical AccountKeyはowner-only・regular・0600・1..65536 bytesで前後identityが安定した
`CODEX_HOME/auth.json`の`tokens.account_id`をJSON文字列としてdecodeした、正規化なしのUTF-8 bytesである。JSONのquoteやescape表記そのものをHMACへ入れず、trim・大小文字変更・Unicode正規化もしない。raw AccountKeyが前後で不一致または再検証不能なら、DB/WAL/SHM、checkpoint、publishを0件にする。
前後`account/read`が不一致ならquota/thread取得結果とその公開更新だけを棄却し、raw AccountKeyが一致するSession記録は継続する。
raw AccountKeyとtokenはpath、profile metadata、DB、journal、log、RESTへ保存しない。login IDは1..254 Unicode scalar、
trim済み・control文字なしを再検証し、owner-only account registryと当該account DBの表示label、およびloopback限定
`GET /v3/accounts`のnullable `login_id`にだけ使用する。login IDの取得・保存失敗はusage記録を停止させない。

#### 記録上のaccount所有区間と未確認境界

この所有区間は、同じprofileで本製品が確認・記録した認証主体の区間である。providerが全process・端末に対して実際に認証や課金先を切り替えた時刻を表す外部保証ではない。Sessionの可読性、現在の`account/read`、または設定値だけから、その確認以前の区間を作らない。過去の採否には保存済みの区間と8.7の独立したsource結合を使う。

区間は`[start_at,end_at)`とし、開始を含み終了を含まない。既知の境界はUTC Unix秒の整数`1..253402300799`で保持し、minuteへ丸めない。有限の終了は開始以上とし、同秒の切替による`[T,T)`は有効な空区間で、eventを一件も含まない。時刻の近さを理由に離れた区間を結合しない。A→B→Aの二つのA区間は別の連続区間である。

空区間markerは保存・wire・読取り・両OSの描画まで有効な形として保持し、正の区間の重複判定や被覆計算では面積0として扱う。他の正の区間内に同時刻markerがあっても、そのmarker自体をoverlapや利用量の根拠にしない。空区間だけの集合はeventを一件も所有せず、非空の所有区間や通常描画線へ昇格しない。隣接する同account区間も、記録した境界を消すunionへの置換をしない。これは区間表現の条件であり、空配列/開始不明から新規所属を認める規則ではない。

minute読取りの被覆判定とquota-only prefixの終端選択にも、空区間を使用しない。例えば`[150,150)`だけでは代表時刻120のminuteを可視化せず、100から150までのquota-only prefixも作らない。正の区間が別にある場合は、その正の区間だけで従来の資格を判定する。

| 保存された区間入力 | 数値候補のaccount所属への結果 |
| --- | --- |
| 開始S・終了Eが既知 | `S <= event時刻 < E`だけをその区間の候補にする。legacy deltaは両端が同じ区間に必要 |
| 開始Sが既知・終了がnull | 記録上まだ閉じていない区間である。新規採用は、そのaccountを現在のraw AccountKey・admissionで再確認できたcycleの公開基準時刻Tまでに限定し、未来へ外挿しない。保存済みcommitの歴史読出しは現accountへ付け替えない |
| 開始がnull、区間配列が欠落/空、または開始を裏付ける保存根拠がない | 開始以前を非有界な所有と解釈しない。その入力から新しい消費の所属・完全性を作らず、旧記録・既知値・原本と不足理由を保持する。保存済みの有効な歴史値を0や別accountへ変更する規則ではなく、元のkeyと資格で公開可能な`legacy-unknown`値のread-only表示はPRODUCT/WIREに従う。表示できる旧値から、新しいsourceの所有証明を逆算しない |
| 既知の区間の間に空白がある | 未確認の区間として保持する。前後の同じaccountで埋めず、空白内の数値をどちらへも複製しない |
| 終了が開始より前、範囲外/型不正、既知区間の重複、または複数の未終了区間 | registryの所有資格を不成立とし、新しいusage/checkpointとそれに基づく公開更新を採用しない。旧registry、DB、原本を保持し、identityエラーのstrict boundaryを適用する。このsecurity表示境界を新しいusageの採用と混同しない。並替え・clamp・優先accountの推測で救済しない |

既存履歴のread-only投影と、新しいsource/eventの所属資格は区別する。interval metadataの不足だけを理由に、同じpartition・元のcanonical keyとprovenanceで表示資格を持つ旧値まで検索結果から消さない。公開できる旧値の範囲はPRODUCT/WIREに従い、不足した所有を公開上のspan説明にも保持する。また、canonical minuteの代表timestampだけで元eventのaccount所属をやり直さない。既に資格を得たeventは元時刻・accepted prefix/provenanceで判定し、minute化によってその点を別accountへ移したり削除したりしない。点の表示資格は、その前後の連続所有、idle、日別配賦の資格を自動的に作らない。

初回のcanonical account資格がない主体を開く境界Tは、8.6のraw AccountKeyと同一app-serverの前後`account/read`による主体確認に成功した取得群の開始UTC秒である。取得群の開始は最初のraw snapshotの読取り開始前に一度記録し、終了UTC秒は最後のraw/lease/admission照合後に記録する。両方が公開時刻範囲内で開始以上の終了を持つ取得群だけを有効とし、その群の全確認が終わるまでは開始Tも未確定候補である。quota応答を含む群では、その元観測時刻がこの群の中にあることも確認する。成功群内の確認より前の公式quotaを、確認完了時刻を区間開始にしたために所有外へ落とさない。逆に取得群開始より前の旧prefixへこの資格を外挿しない。

その資格を既に保存済みのaccountへ戻る場合も、同じAccountKeyの前後raw照合、保存partition資格、現admissionが成功した取得群の開始Tを使い、RPC成功をlocal記録の再開条件へ戻さない。初回確認資格を保存した事実が不明なら初回と同じ条件を必要とし、partition fileの存在だけを確認済み資格にしない。どちらも、新sourceの初回RPC結合と既存sourceの継続資格は8.7で別に判定する。

初期割当てとこの確認を混同しない。stableなraw AccountKeyと既存8.6のprofile/registry検証の下で、初期化前のpartition ID/StorageEpochを割り当てて確認候補のtupleを固定することはできる。その割当てだけでは所有区間・確認資格・usage/checkpoint・ready公開値を作らない。初回確認、資格と境界のatomic保存/readback、DB identity初期化の順に進め、全て成立した後だけ新usage候補を受理する。未確認の割当てをcatalogの初期化済みcurrentにせず、確認失敗時も新しい空DBや別accountで代用しない。再試行は同じ割当てを検証して再利用し、失敗ごとにStorageEpochを消費したり旧partitionを捨てたりしない。

この初回確認資格v1は、検証規則の版1、確認した`ProfileScopeId,AccountScopeId,StorageEpoch,partition_id`の完全tuple、元の確認群の開始/終了UTC秒をregistryへ結合して保存した記録を根拠とする。前後raw照合と同一子processの前後RPC、現lease/admissionの全条件が成立した後だけatomic保存し、fresh readbackで同じtuple/版/両時刻を確認してから新usageへ使う。欠落・未知版・tuple不一致・不正時刻を「確認済み」に補完しない。raw AccountKey、token、RPC原文、emailをこの資格へ複製せず、RESTへも公開しない。成功を記録した本製品内の資格であり、外部providerの署名や過去の課金先証明ではない。再利用で元確認時刻を現在時刻へ更新しない。具体的な内部型やfield配置は実装詳細だが、必要な保存事実・atomic性・欠落時の採否は変更してはならない。

資格fieldの欠落/nullは未確認の旧形式として扱えるが、提供された資格の未知版・tuple不一致・不正時刻は不正metadataである。後者を欠落扱いへ落として上書き・再初期化せず、新しいusageとregistry mutationを拒否して元の記録を保持する。旧区間に有限開始があることだけでも初回確認資格の代用にはならない。

初期化済みの旧accountで区間配列が欠落している場合、元の保存済みactivation境界があればその値、なければnullを旧開始として保全し、初回確認のTで閉じた旧区間と新しい`[T,null)`を区別する。旧activation境界を保存していなかった別accountの開始/終了を、このaccountの値から生成しない。旧区間は元の歴史値を保持するための表現であり、今回の初回確認を旧sourceの所属資格へ遡及する根拠ではない。未初期化の新しい割当てには、この旧履歴用区間を生成しない。

主体を失った境界は、下表で指定する不成立またはlogoutを確認した時点のUTC秒とする。これらは本製品が観測した境界であり、外部側で実際に切り替わった時刻へ遡及しない。確認中に日付が変わっても同じ規則を使う。Tが公開時刻範囲外、または保存済みの直近境界より前なら新しい境界を保存せず、旧値を保持して新candidateを拒否する。時刻を丸めたり、直近境界へ押し上げたりしない。次の通常cycleで新たな入力が資格を満たした場合だけ再判定する。

閉じる遷移では、全registry区間とTの公開範囲・直近の既知境界以上であることを、区間変更より前に検証する。不成立なら一部だけを閉じず元のmetadataを保持する。未終了区間が既に0件なら、同じ検証を通した後に区間を0件のまま保ち、新たな所有・境界を生成しない。未終了区間の開始とTが同秒なら、有効な`[T,T)`として閉じる。

| 認証・lifecycleの入力 | 区間の一意の遷移 |
| --- | --- |
| Aの初回確認資格v1を初めて保存 | 有限開始TのA区間を開く。Aに旧未終了区間がある場合は開始の既知/不明を保ったままTで閉じ、新区間と区別する。旧有限開始を初回確認へ遡及流用しない。既存fileの過去prefixや初回raw値を、その前のA利用として加算しない |
| 保存済みの初回確認資格v1が有効で、有限開始が既知の現在の未終了区間がAであり、同じAの確認が継続/再起動後に成功 | 保存済みの開始・旧区間・元の初回確認時刻を動かさない。再起動だけで新しい所有区間、過去の0利用、全sourceのEOF基準を作らない。sourceと未記録suffixには8.7の再検証が必要 |
| logout/主体不明で未終了区間が0件となった後、既存account Aの確認に成功 | Aの有限開始Tの新しい区間を開く。直前に閉じたaccountが同じAでも別accountでも同じとし、旧区間の終了は動かさない。例えばAが200で閉じ、230にAを再確認した場合はA=`[230,null)`を追加し、`[200,230)`を未確認のまま残す |
| AからBへの確認済み切替 | Aの未終了区間を境界Tで閉じ、Bを同じTから開く。先に未確認境界T0を記録していた場合はAの終了T0を保持し、Bを今回の確認Tから開いて`[T0,T)`を不明のまま残す。既存Bのpartition/StorageEpochは再利用するが、離れたB区間を結合しない |
| PRODUCTの前後RPC条件でlogoutを確定 | 現在の区間をTで閉じ、次の主体確認までは未終了区間を0件とする。初期化済みaccountと歴史区間は削除しない |
| raw AccountKeyの不一致/再検証不能、identity failureで主体を確認できない | registry自体が有効で保存可能なら現在の区間を不成立確認のTで閉じ、次の成功確認まで未確認区間を残す。logoutへ読み替えず、当該cycleの新しい数値を採用しない。registry/保存自体が不成立なら境界も推測更新せず、旧状態を保持する |
| registryの未終了主体Bと異なるraw主体Aを確認したが、Aの初回確認は未完 | Bをraw切替の確認T0で閉じ、Aの区間はまだ作らない。Bの境界保存後にAの確認群を開始し、成功した群の開始TからAを開く。確認が失敗した間と`[T0,T)`は不明のままとし、Bを開き直したりAをT0へ遡及させたりしない |
| 外部RPCだけが失敗/不一致だが、raw AccountKey・既存source結合・現admissionは一致 | 2.6/8.6のlane分離を適用し、検証済みlocal appendの所有区間を閉じない。初回のsource結合や新しい主体確認を成功したことにはしない |
| quota reset、通常commit、heartbeat、CollectorEpochの交代だけ | account所有境界を新設・移動しない。CollectorEpoch交代時のsource再検証は独立して行う |

旧start=nullの未終了区間を持つAを現在確認できても、nullを現在時刻や設定値で上書きして過去を確定しない。元の未知開始を保った区間を今回のTで閉じ、有限開始Tの新しいA区間を開く。区間が欠落する旧accountについても、既存履歴を保持したまま今回の有限開始だけを記録する。初回確認資格が欠落した有限開始の旧A区間も上表の初回規則へ従い、`[S,T)`と新しい`[T,null)`を同じAで隣接していることだけで結合しない。旧SがTより後なら時計/境界不整合として拒否し、旧Sを切り詰めない。これらは現在以後の確認区間を作る操作であり、旧数値・source資格の修復ではない。有効な初回確認資格と有限開始を持つAの通常再起動には、この旧形式の処理を適用しない。fixture用activation値、CLI設定、fileのmtime、最古のSession時刻で過去の開始を補わない。

registryの区間変更は単一publisherのaccount境界内でatomicに保存し、保存とfresh identity readbackが成功するまで新accountのusage候補をcommitしない。失敗は旧registry/DBを保持し、新しい所有を前提にしたcheckpointや公開値を作らない。区間の既知性とsource/account結合は別の資格であり、区間を作成しただけでは既存sourceの数値採用を許さない。旧形式の区間不足や非所有区間に残る消費は、8.7とPRODUCT `HISTORY-RECOVERY-614`の根拠が揃った場合だけ再判定する。

例えばA=`[100,200)`、B=`[200,250)`、A=`[250,null)`なら、t=200はB、t=250は後半Aの候補である。rawの両端t=180とt=260は同じA文字列でも一つの連続所有区間に入らず、その差をAへ加算しない。旧`[null,200)`はt=180の新規所属を証明せず、配列欠落も全時刻所有の根拠ではない。A=`[100,200)`、B=`[230,null)`の空白`[200,230)`も未確認のままである。いずれもsource/model/公式periodの独立条件を省略しない。

### 8.7 DP-REST-006 / RC-144 — cursorとDB transaction

authoritative checkpointは外部cursor fileではなく同じaccount SQLite DBの`session_checkpoints` tableに置き、usage row batch、
`session_ranges` dedupe、model totals、fully-attributed marker、`DataGeneration`と同じtransactionでcommitする。checkpoint keyは
`(sessions root identity,root-relative path,device,inode,prefix generation)`であり、旧source identityのcheckpoint/markerを
新sourceで上書きしない。外部hintはscan開始候補でありcommit authorityではない。

transaction commit前はrow/checkpoint/generationの全てが旧値、commit後は全てが新値である。commit後publish前のcrashでも、再起動後に同じaccount/source identityと既存prefix/hashを検証できればcommit済みcheckpointから再開し、未記録suffixをEOFまで読む。再起動だけを理由にEOFへ飛ばさない。identity boundaryでは旧accountのcheckpointを新accountへ流用せず、所有区間を検証し、継続不明なら8.7.2のanchor照合へ進む。source record identity
`(root identity,relative path,device,inode,prefix generation,start offset,end offset,range fingerprint v1のrecord_sha256)`はuniqueで、
この組は同じpartition内のrange重複判定keyである。`session_events`のevent keyはこの組へ非負の`event_index`を加え、同range内の異なるeventを区別する。CollectorEpochだけが異なる再読込では同じkeyを二重加算せず、partial rowとcursor先行を0件にする。異なるroot/path/prefix generationを省略した短いkeyで重複と判定しない。generationを跨ぐ同一物理recordの退役には、後段の全prefix検証による限定資格を必要とする。

収集予算で未読sourceを後回しにするための`committed_offset=0`のcheckpointは、原本を一文字も解析していない位置を表す。実際にbyte 0から解析するcandidateでは、model kindを`none`、差分区間を未観測、Session metadata・response分類状態を先頭未観測から構築し、正常なmetadataと最初のcontextも同じ入力契約で採用する。仮checkpointの存在やそこで保存したNULL/unknownだけを理由に、この先頭解析を「旧解析状態の継続」として扱わない。これは初回の正counterの加算、以前のusageが0であること、source/account結合、完備性を証明する規則ではない。それぞれ独立した既存条件を適用する。非zeroのcommit済みprefixで旧資格が欠ける場合の先頭再検証・後着contextからの遡及禁止、account切替で既存の物理EOFへ置く非zero境界は変更しない。

#### source keyの保存表現と診断namespace

Linux/WSL recorderの`sessions root identity`は、検証したcanonical sessionsディレクトリ自体の`(device,inode)`を保存する物理namespaceである。8.6のprofile/account/partition identityとは別であり、account認証や保存file内容の証明を代用しない。rootのcanonical化、通常ディレクトリであること、symlink・containmentの拒否はSECURITYに従う。

各source候補は、そのinventoryが記録したrootと、原本読取り開始前および読んだbyteのhash照合終了後のrootが、同じ通常ディレクトリの物理identityである場合だけ採用する。旧accepted prefixの読取り検証にも同じ前後条件を適用し、原本を再読取りしないresident証拠の再利用にも現在のroot一致を必要とする。root不一致・取得不能では当該候補の新しいrange、checkpoint、model数値、eventを採用せず、既存の値と原本を保持する。同じ条件を独立に満たした別sourceの結果を取り消さない。file自身のdevice/inode・内容が変わらないhardlink等でもrootの置換を許可せず、account/commit時の検査でこの読取り時の検査を代用しない。これはsourceごとの読取り資格であり、複数sourceの読取り全体をfilesystemの単一snapshotと見なす規則ではない。

保存文字列はASCIIの`unix:<device>:<inode>`とする。二つの値はOSから得たunsigned 64-bit値の十進表記とし、0以外の先頭0、符号、空白、区切りの追加を認めない。root path、mtime、size、account、CollectorEpochを文字列へ加えたり、その文字列を再hashしたりしない。これは既存Unix記録と照合するための製品側の保存互換規則であり、外部Codex protocolの定義ではない。同じ生存中ディレクトリのcanonical pathへの別表記は同じnamespaceとなり、同じpathの別ディレクトリへの置換は別namespaceとなる。ただしnamespace文字列の一致だけではsource継続を確定せず、8.7の完全key・全prefix/hash・Session/accountの資格を全て必要とする。

`root-relative path`の採用対象は、検証済みcanonical rootからの通常path componentで、全てがUTF-8かつbackslashを含まないものに限定する。その元byte列を保持して`/`で連結した文字列を保存する。絶対path、空component、`.`、`..`を含めず、大小文字変更・Unicode正規化・lossy変換を行わない。非UTF-8は本TEXT形式でlosslessに表現できず、backslashは保存互換の区切り規則で許可しないため、どちらも通常source keyを発行せず未採用として原本と既存記録を保持する。Unixのbackslash自体がdirectory区切りだと解釈する意味ではない。別の有効sourceは通常条件で処理できるが、inventory/model集合をcompleteにしない。この不成立を似た文字列の既存keyで救済せず、入力が上記の許可条件を満たし、原本とaccountの通常検証も成立した場合だけ改めて採用を判定する。rootディレクトリ自身の物理namespaceをpathの表示文字列から作る規則ではない。

表現できないpathが一つ以上あることは、当該rootのinventory全体に対する一つの未解決条件として保持する。同じinventoryで対象が複数あっても、この条件の診断markerは一つとし、個々のfileへ偽のsource keyを発行しない。marker件数は対象file数やrange coverageを意味せず、個々の原本は全て保持する。これは解析済みの別recordや既知sourceの別rangeを統合する規則ではない。liveのactive pathにも同じ表現資格を適用し、不成立なら当該thread取得cycleを失敗として旧完全snapshotを保持する。発行できないkeyのcheckpointを要求して再収集する経路へ流さず、RPCや原本解析で資格不足を埋めない。

inventoryのI/O診断も通常sourceのidentityと区別する。失敗箇所のroot-relative pathを上記の表現資格で保持できる場合は、末尾名だけでなく完全なrelative pathを診断位置に使う。root自身または表現できない位置は予約診断位置`__inventory__`とし、通常sourceが存在するという意味にはしない。`left/unavailable`と`right/unavailable`の失敗を同じ`unavailable`へ縮めない。

一つのinventory内の診断predicateは`(診断位置,元のreason文字列)`で区別し、完全一致するものだけを一件へまとめる。診断位置が同じでも異なるreasonを上書きしない。重複を除いたpredicateを、診断位置のUTF-8 unsigned byte昇順、同位置ならreasonの同じ昇順へ並べる。保存する診断markerの`prefix_generation`はこの列の1始まりのordinalとし、既存のunsigned 128-bit generation保存形式（小文字hex、先頭0を含む固定32文字）を使う。SQLite INTEGERの64-bit範囲を通常generationやこのTEXT保存値の上限へ流用しない。これは当該inventory内の診断keyの区別だけであり、通常sourceのprefix generation・継続・原本hashを証明しない。空列はmarkerなし、1始まりのordinalを保存形式へchecked変換できない候補は採用せず旧状態を保持する。

これらのmarkerは、取得できたroot identity（root未取得なら下記fallback）、診断位置、device/inode=0、byte範囲`[0,0)`、既存のempty-content hash、`complete=false`を保持する。ordinalをsource pointerやcheckpointへ流用しない。同じ`__inventory__`位置でdirectory読取り失敗と表現不適合が併存しても、別ordinalで両方を保持する。新cycleの列は、元のsource/event/rangeを変更せず、そのcycleの未完了inventory診断だけを同じusage transactionで置換する。診断集合が変わればordinalが変わり得るが、旧normal sourceを置換・再加算する根拠にはならない。異なるpredicateや既知sourceの別recordを無条件にまとめず、writerの重複・競合candidate拒否は維持する。診断自身のidentity衝突によって独立した有効sourceを失敗させない。

既存の`path-fnv1a64:<16桁lowercase hex>`や、上記のUnix形式として検証できないroot keyは、元のopaque文字列と記録値/provenanceを保持する。Linuxの物理rootへ推測変換・alias登録せず、新しいLinux sourceの継続資格に使わない。readerの過去表示まで削除・0化する意味ではない。対応する原本がある場合の独立した再資格付けと数値修復は、8.7.1とPRODUCT `HISTORY-RECOVERY-614`の条件を使う。非Unixの旧path hashを、新しい対応OSや物理source保証の根拠にしない。

rootを取得できないinventory障害の診断には、既存のfallback保存表現を区別して使う。入力Pは設定されたroot pathのlossy UTF-8表現のbyte列であり、canonical化できたrootの代用品ではない。値は`lowercase_hex(SHA256(SHA256(ASCII("codex-info-session-root-fallback-v1") + NUL + P)))`、各内側digestは表示hexでなくraw 32 bytesとする。この二重SHAは既存診断markerとの互換だけの規則である。通常rootの`unix:`形式、8.7のrange hash、response recordのhashを相互に代用しない。

このfallbackは未取得inventoryの理由を保持するための値に限る。pathのlossy変換やhashから、同じ物理root、source所有、解析完了を主張せず、usage/eventの新規加算・checkpoint継続・complete recorded marker・原本復元の資格に使わない。正常に取得したroot identityへfallbackの旧checkpoint/rangeを付け替えず、元の診断理由と新しい正常inventoryを区別する。rootは取得できたが個別pathを表現できない場合も、実在しないsource keyを作らず、原本を採用できないinventory理由として保持する。

例えばrootのdeviceが8、inodeが42なら保存値は`unix:8:42`である。同じ物理rootのrenameだけで再発行せず、同じpathがinode 43の別rootになれば`unix:8:43`となる。二つのrootのfile inodeやfilenameが同じでも旧cursorを自動流用しない。Unixの`a/b.jsonl`は通常relative keyとなり、component内にbackslashを持つ`a\b.jsonl`には通常keyを発行しない。後者を前者のkeyへ変換してはならない。これらは表示上の整形ではなく、保存済み原本を一意に照合するための条件である。

#### 不変入力の再読省略と再検証条件

この節はLinux/WSLのresident recorderが、既に検査したSession原本を次cycleでも読む必要があるかを決める規則である。保存source identityは8.7冒頭の完全keyのままとし、変更検出用metadataをidentityへ加えない。

変更検出用fingerprint Fは、完全source keyと、現在の通常fileから取得した`device,inode,size,mtime秒/nanosecond,ctime秒/nanosecond,mode,uid,gid`の組とする。時刻はOSが返した精度のまま比較し、丸め・時刻差の許容幅を設けない。atimeは自分の読取りでも変わり得るため含めない。各fieldの外部意味はLinux man-pagesの[stat(2)](https://man7.org/linux/man-pages/man2/stat.2.html)と[inode(7)](https://man7.org/linux/man-pages/man7/inode.7.html)に従う。metadataは原本内容のhashでもatomic snapshotの保証でもない。同じmetadataだけを根拠に、新しいusage・checkpoint・model資格を採用してはならない。

再利用するのは、同じ生存中recorder・account partition・auth境界・CollectorEpochで既に検証した結果だけである。その入力はF、commit済み位置C、照合した全accepted range/hashと版、保存済み解析資格、source/account結合と所有区間を含む。現在の入力が一つでも異なる場合は結果を流用しない。raw quotaのreset、独立したquota commit、heartbeatやDataGenerationの前進だけでは、Session原本の変更や解析資格の失効としない。所有区間や原本の資格が実際に変わった場合は別である。

再利用可能な結果は、その固定入力の必要な原本検証と解析を終え、読取り前後にFとpath/FDのidentityが一致し、対応するbatchのcommitとfresh readbackが成功したものに限る。commit前候補、rollbackした結果、readback不成功を新しい完了状態にしない。process再起動・CollectorEpoch交代後にはこの一時的な結果を引き継がず、DBの保存key・range/hash・解析資格を現在のsourceで再検証する。再起動だけでcommit済み数値を再加算・EOF基準化する許可にはならない。

| 今回の入力と保存結果 | 原本読取り・採否・保持の結果 |
| --- | --- |
| 上記の再利用条件が全て成立し、検査済みEOFまで到達済みで、新しい解析・資格再構成・exact pending batchがない | source内容の再読、解析、Session row/checkpoint writeは0。commit済みの値・既知/不明・未解決理由をそのまま保持する。必要な通常cycleのcommit、quota、heartbeatは各ownerで独立して進む |
| 同じ固定入力を最後まで検査済みだが、未改行tailまたは内容から確定した不成立理由が残る | 原本が同じままなら同じbytesを再解析しない。未処理位置・理由・旧anchorを保持し、complete・未使用・正常へ変更しない。内容不成立には、必要な全rangeを読めたうえで確定した保存hash不一致も含む |
| 予算で未読suffixが残る、旧NULL/v1の資格再構成が未完、または新しいsource/解析規則の検証結果がまだない | 未処理をEOFとせず、対応するownerの有界処理を進める。内部chunkの進捗を保持して同じ先頭を毎cycle読み直さず、完成前の資格を公開しない。入力不明のまま最後まで検査した状態と、まだ読んでいない状態を区別する |
| F、保存位置C、accepted range/hash・版、解析資格、source/account結合・所有区間のいずれかが変化した、または明示的な原本再検証を行う | 前回の再読省略結果を失効させる。旧解析状態から新しいsuffixを採用する前に、保存済み`[0,C)`の必要な全range/hashとsource継続を検証し、候補読取り後も継続を確認する。size/mtime一致や新suffixだけのhashでは代用しない。新しい数値の採用には、さらに元の数値・model・account・period条件を全て適用する |
| source/metadata取得または内容読取りがI/O失敗、あるいは検証中にF/identityが変化した | そのsourceの未commit候補を取り消し、旧checkpoint・数値・不足理由を保持する。読めなかった結果を内容一致・内容不一致の確定結果として再利用しない。次の通常cycleまたは明示操作で最大1回の再試行ができる。同callbackで再試行したり、失敗を理由に他sourceの全再読・app-server再起動を追加したりしない |
| DB保存失敗のexact pending batchが残る | DATA4.1/8.8の再試行規則を使う。失敗したbatchの解析結果をcommit済みとして再利用せず、再試行前のaccount/lease/epoch/source資格を確認する。原本が変化した場合は旧batchを新しいbytesへ流用しない |

actual account collectorのpending再試行では、未commitの正のcheckpointごとに、同じpartition/source/generationに結合した保存資格と、元の保存済みrangeおよび当該batchのrangeが覆う固定prefix `[0,C)` の全hash、LF終端、物理root/fileの継続を再確認する。Cより後ろへのappendだけなら、この固定prefixが一致する限り元のexact batchを一度保存でき、追記部分は次の通常cycleへ残す。未解析のoffset 0は数値の根拠にしない。一つでも不成立または読取不能なら、原子的に保存する予定だった未commit batch全体を取り消し、そのcycleを障害として公開する。旧durable checkpoint・数値・原本は保持し、次の通常cycleで既存のsource単位の採否へ戻る。同callbackで再解析・再保存せず、旧batchの一部だけを新しい原本へ付け替えない。source資格の確認後にも、現在のaccount/lease/epochの最終guardを通す。

再読省略は新しい観測ではない。これだけで最終実測時刻を進めたり、不明model・未処理rangeを解消したり、graphの未使用・実測・予測を決めたりしない。source/account等の現在の採用guardも省略しない。原本内容の失敗が確定した同一入力を通常周期の到来だけで全再読し続けることと、一時I/O失敗後の有界な通常再試行を区別する。具体的なcacheの型・配置・検索方法は実装詳細だが、この表で定めた読取り要否と採否を変更してはならない。

#### 保存済み解析資格の復元と、account境界の基準

旧checkpointの解析資格がNULL/v1で不足する場合、固定した改行済みprefixをbyte 0から同じ入力契約で再生して、元の位置で成立していたmetadata、stream kind、model kind・差分区間、raw anchorの位置/時刻、response分類状態を再構成する。先頭からのSession/lineage継続と保存済み全range/hash、現在のsource再確認を必要とし、末尾の数行だけや最後のmodel名で代用しない。内部chunkを分けても、再構成が対象位置まで到達する前の状態を完成したcheckpoint資格やUI snapshotへ公開しない。

この解析資格の復元と、過去数値の修復は別の操作である。解析した旧counter/responseを再び新規消費へ流さず、既存event・合計・履歴・保存価格・cursorを変えない。新資格とその原本根拠の保存は同じusage transactionで行い、保存失敗なら旧stateを保持する。旧数値または保存済みの既知raw anchorと再生結果が食い違う場合は、この資格追加で上書きせず、PRODUCT `HISTORY-RECOVERY-614`のbackup付き原本修復へ渡す。v1の旧response分類を後着recordから補うのでなく、そのroot位置から元の分類区間を再生する。

復元対象の終端Cは保存済みの改行済みcursorに固定し、Cより後ろの追記を復元prefixへ取り込まない。未完の再生位置・解析状態・入力証拠を内部に保持し、通常収集と同じ処理予算で続きから進む。復元中も保存cursorはCのままとし、未完状態を保存済み資格へ昇格させない。同じcheckpoint・全accepted range・source/account境界に対する進捗は、追記等で観測metadataが変わっても、旧固定prefix全体の再照合に成功した場合だけ継続できる。prefix不一致・identity不一致・必要証拠不足では資格追加と当該sourceのsuffix採用を行わず、既存記録と未処理理由を保持する。

Cへ達した時のraw照合では、保存済みbaselineが既知なら、再生したbaselineも既知でTが一致し、保存済みの既知I/C/W/Oがそれぞれ同じ既知値であることを必要とする。不明成分を0として比較しない。保存済みrawのknownness・値を資格追加で書き換えず、所有外のrawを新accountのdelta始点へ昇格させない。不一致は原本修復が必要な状態として保持し、同じ入力のまま毎cycle先頭から再生し直さない。再生中に認識した不正record・未分類rootも保持し、解析状態を復元したことだけで過去数値の正しさや集合のcompleteを宣言しない。復元で読み出した旧event、task event、quota、価格を新規加算・取得観測へ流さない。

復元中の処理予算による一時backlogは、内部進捗と現在cycleの未処理markerで表し、完成後の原本診断へ持ち越さない。完成時に追加する診断は固定prefix `[0,C)` 内で完結した原本recordの証拠に限定し、その元byte範囲/hash/source keyを保持したまま、資格を採用する現在のCollectorEpoch/cycleへ所属させる。過去のevent時刻を現在へ変更する規則ではなく、過去cycleの一時診断へwriterのadmission条件の例外を設けるものでもない。

| 再開時の状態 | 以後の一意の扱い |
| --- | --- |
| 保存済みのsource/account結合と解析資格が全て有効で、accepted prefixが一致する | そのcheckpointの状態から通常appendを再開する。再起動だけを理由に先頭再解析・EOF基準化をしない |
| 同じsourceに旧NULL/v1の資格不足がある | 不足する資格を固定prefixから再構成する。再構成できないstreamをlegacy-counterと仮定しない。新しい強contextで現在modelが判明しても、stream/metadata/raw時刻の不足まで解消したとはしない |
| account境界で以前の所有を証明できず、既存fileの物理EOFを観測した | その位置は旧範囲を新accountへ自動加算しないための境界であり、以前の利用量0や解析完了ではない。metadata/model/streamの解析資格は先頭から独立に検証する。既知の以前のrawも、端点が新accountの同じ一つの所有区間にない限り、そのaccountのdelta始点にしない |
| legacy rawの前端が所有外/時刻不明で、新しい所有区間で初めて有効counterを得た | 今回値を追加利用0の比較baselineとして保持する。次の有効counterとの区間だけを通常deltaの条件で判定する。境界を跨ぐ差分を以後のcounterへまとめて加算しない |
| 再構成したprefixにresponse markerがある | その位置以後はresponse-recordを維持する。後続counterだけを得てもlegacy差分へfallbackせず、root不足・未分類などを元の数値とともに保持する |
| prefixの解析状態は復元できるが、過去eventのaccount/model/公式periodの資格がない | 解析状態だけを回復し、その過去数値の公開加算・未処理解消・complete化はしない。過去数値の回復は各原本修復条件で判定する |

物理EOFの観測位置Bが未改行tail内にある場合、解析cursorはその行の前の完全LF位置Cまでとする。Bを改行済みcheckpointへ偽装したり、後続appendをBから読み始めて同じJSON recordの途中を飛ばしたりしない。Cからそのrecord全体の完結を待ち、元timestamp・所有区間と上表のbaseline規則で判定する。境界以前の未資格bytesを現在のconfirmed eventへ自動採用しない。資格を得た既存checkpointがある同accountの再起動を、この未資格fileのEOF基準化へ落としてはならない。

有限例として、legacy sourceの旧rawが15で所有外、境界後に同じ一つの所有区間でraw20、raw25を得た場合、20はbaseline、採用できる差は25-20の5だけである。以前の15も同じ所有区間の検証済みanchorなら、通常の条件で20-15と25-20の各5を扱える。どちらも初回累積15を加算しない。先行prefixにresponse markerがあれば、このlegacy計算を新規消費へ使わない。これらの条件は、資格の復元を一度に行う場合と複数の内部chunkへ分ける場合で変わらない。

### RECORDER-MODEL-01: 認識するSession JSONL入力と採否

この節は本製品のlocal usage adapterの入力契約であり、Codexの全版に対する外部JSONL安定保証ではない。RPCのcamelCase schemaを、そのまま保存JSONLのsnake_caseへ適用しない。原本のproducer版が分かる場合はprovenanceに残す。版が不明でも下表の構造とsource/account資格を全て満たす既知形は受理できるが、未知形を推測変換しない。

本adapterが状態を変更するevent名は`token_count`、rootの`token_usage_record`、`turn_context`、`thread_context`、`thread_settings_applied`、`task_started`、`task_complete`、`task_completed`、`turn_aborted`に限定する。identityを変更する`session_meta`はrootにある場合だけ認識する。型/版を認識できないrecordは原本を保持し、使用量0やtask停止へ変換しない。解釈できないrecordの存在を根拠に以前の有効anchorを消さず、そのrecordがusage/stateに無関係であることを認識済みformatから証明できないsourceはfully-attributed markerと完備性を確定しない。別sourceの検証済み値は継続する。表示内容だけの`response_item`はusage/stateを更新しない。未知形を採用可能にするにはこの入力契約を先に改訂する必要があり、実装中の推測分岐を認めない。

| 入力位置 | 認識する型・意味 | 欠落・不正時 |
| --- | --- | --- |
| root `type`, `payload.type` | 文字列。rootが直接の既知event名、またはroot=`event_msg`のpayload.typeが既知event名 | 両方が既知で矛盾すれば拒否。未知rootをpayload名だけで既知eventへ昇格しない |
| root `timestamp` | token/state/context eventはRFC3339文字列。offset込みからUTCへ変換した実時刻 | 受信時刻、mtime、epoch 0を代入しない。usage/stateを決めるeventが不正なら未処理として保持 |
| root envelopeのoptional `ordinal` | 不在、null、または0を含むunsigned 64-bit整数を認識する。これはpaginated rollout内の保存位置metadataだけであり、数値・model・task・account/Session/response identity・公式periodの根拠へ昇格しない。欠落を0で補わず、原byte順で既存recordを解釈する。文字列、bool、配列/object、負数、小数・指数表現、UInt64超過、同rootでの重複は正規のordinal形にしない | 不正ordinalは完備性の証明へ使わず、live候補全体を拒否して旧完全snapshot/cacheを保持する。local usageの独立した既知factと未処理rangeは第2章の規則へ従い、ordinalで未知値を補完しない |
| root `type="response_item"` 専用のoptional `metadata` | 不在、null、またはJSON object。固定producerがpayloadと分離して保存するhistory-only metadataであり、全JSON/UTF-8/構造depthをstreamingで検証する。内部の文字列・ID・model・usage・taskやcontrolに見える値を数値/state/認証主体の根拠へ再帰解釈しない。同じkeyをpayload内部に置いてroot metadataとして扱わず、ほかのroot recordへこのfieldを流用しない | scalar/array、重複metadata、別root typeへの付与は完全な既知envelopeの資格なし。liveは旧完全snapshot/cacheを保持する。localの既知独立factを捨てず、未処理rangeと完備性不足を保持する |
| root `type="session_meta"` の `payload.id` | payloadはobject、idはcontrol/bidiなしの1..512 Unicode scalarの非空文字列。大小文字・前後空白を正規化せず元IDを照合する。`payload.history_base`非nullは継続の標識。legacy-counterの最小metadataではsession_idの欠落を許すが、提供値は非nullの有効IDで、同sourceの既知session_idと一致を要する。後続metadataでの欠落だけでは保存済みの既知session_idを消さず、欠落値をresponseから補わない。response streamの追加条件は下節 | identityの欠落・不正・矛盾は未処理として保持し、baseline受理・fully-attributed markerを確定しない。標識だけで前回合計や未計上分を証明しない |
| `turn_context` / `thread_context`の`payload.model`、互換root `model` | 同じsource内で以後の観測へ効くmodel context | 両方がある場合は正規化後の一致必須。欠落を最新製品modelへ置換しない |
| `thread_settings_applied`の`payload.thread_settings.model`、`payload.model`、root `model` | 下記context kindが`none`の場合だけの初期model情報 | 複数の提供値は正規化後の一致必須。初期値またはturn contextが既にあれば有効な設定値でも上書きしない。kind不明を初期状態と扱わない |
| `token_count.payload.info` | object、またはnull（そのrecordにはsampleなし） | field欠落や別型を正常な0 sampleとしない。nullはno-opで前回counterを変更しない |
| `info.total_token_usage` | 累積object。`total_tokens`は必須のUInt64、`input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens`は提供されたUInt64を保持 | 欠落/null componentは不明。負値・小数・overflowは不正。必須totalがなければ数値eventにしない |
| `info.last_token_usage` | 同じtoken成分を持つ直近turnの観測。context使用量はこのtotalを使う | 累積へ別途加算しない。未提供は不明であり、total_token_usage全量の代用にしない |
| `info.model_context_window` | 任意のUInt64。context表示で使う場合は正値 | 未提供/nullは不明、0は有効な分母ではない。不正なcontext pairをusage合計の0へ変換しない |
| task event | `task_started`はactive、`task_complete/task_completed/turn_aborted`は停止 | 原本の完全coverageがない範囲はstate不明。未使用の判定はPRODUCTを参照 |

`event_msg`から認識する場合はrootのpayload objectとその文字列typeを必須とする。直接rootに既知event名を持つ互換形では、modelのroot位置など表が明示したfieldだけを使う。直接rootのtask eventは有効なtype/timestampで判定し、payloadは欠落を許すが、提供されていればobjectを必要とする。task状態の入力はこのevent名と元timestampであり、任意の本文・goal・item・context windowから開始/停止を生成しない。未知の追加fieldは原本に残し、表にない位置へ必須値を探索しない。

直接rootの互換形である`token_count,turn_context,thread_context,thread_settings_applied,task_started,task_complete,task_completed,turn_aborted`では、提供されたpayloadはobjectだけを許す。そのobjectの`type`は欠落を許すが、提供された場合はrootのevent名と同じ文字列を必要とする。null・非文字列・異なる文字列は、そのeventの不正envelopeである。直接rootのcontext/初期設定はpayload欠落でも表のroot modelだけで判定でき、直接rootのtoken_countは引き続きpayload.infoを必要とする。`event_msg`はpayload欠落を許さない。rootで既知と認識したeventの種類と、そのenvelopeの有効性を分離し、不正taskだけでmodel/counterを変更せず、不正counterだけでmodelを変更しない。未知rootをpayload名から救済する規則ではなく、NoOp本文・session metadata・response usageはそれぞれの専用入力表に従う。

強contextのresponse用IDは`payload.turn_id`と`payload.root_turn_id`である。各々欠落/nullまたは本節の有効IDだけを許し、rootの同名fieldや他recordの値から補わない。legacyのmodel帰属ではIDの欠落/nullを許すが、response用の同turn証明には両方の既知IDを必要とする。IDが提供されたのに型・安全境界が不正、またはこれらを読むpayload keyが重複していれば不正な強contextとして扱い、下記のkindと差分区間を不明にする。model自体が欠落/不正の場合を、有効IDだけで救済しない。

token/state/context eventの有効timestampは、RFC3339をUTCへ変換してfloorしたUnix秒が公開範囲`1..253402300799`に入ることを必要とする。source内の処理順はtimestampの大小ではなくbyte順である。tokenのtimestampが不正ならそのrecordの数値baseline・delta・quota・context pairを更新しない。認識済み`token_count`の型・数値・timestampの不正だけでは、有効な現在model contextと開いている差分区間の資格を変更しない。未知のusage/control recordによる連続性不明とは区別する。turn/thread contextのtimestamp、envelopeまたはmodel位置のいずれかが不正なら、不正な強contextとして下記のkind/差分区間を不明にする。認識済みの初期設定の不正には弱contextの保持規則を使う。既知task eventの不正は旧task状態を確定した継続状態へ流用せず、その位置を跨ぐtask区間を不明にする。以前の有効task値は監査用に保持し、次の有効task event以後の状態だけを更新する。いずれも不正recordの未処理と完備性欠如を保持し、taskの不正だけで有効なmodel context/raw counterを変更しない。

root `ordinal`の外部型は、固定producer commit `740e5af33c71225640e0c1c1555c514c2c93ab74`の[RolloutLine](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/history/src/lib.rs#L362-L369)の`Option<u64>`と[JSONL decoder](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/rollout/src/lib.rs#L55-L67)に基づく。[ordinal保存owner](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/rollout/src/ordinal.rs#L16-L53)がlegacy/paginatedの保存位置を区別することを、課金・account・livenessの保証へ拡張しない。ほかの未知root fieldを一括で許可する規則ではない。

固定producerのenvelopeは、上記RolloutLineのtimestamp/optional ordinalと、[RolloutItemWire](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/history/src/rollout_payload.rs#L30-L71)のtype/payloadから成る。response_itemだけはoptional metadataをrootへ持ち、[ResponseItemEnvelope/CodexHarnessMetadata](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/history/src/lib.rs#L52-L72)がhistory-onlyとして分離する。legacyのroot model位置は本表の既存契約を維持する。未知root fieldの一律拒否はproducerが要求するSerde保証ではなく、本製品のstrict live資格条件である。producer decoderが未知fieldを無視できることを、未知のusage/control拡張が本製品のstateに無関係という外部保証へ拡張しない。既知metadataをこの一律条件で誤拒否せず、上記の固定した形・責務を検証する。

#### live状態へ使える原本解析状態

第2章11のprocess資格は次の入力と判定を使う。製品名らしいbasenameだけをartifactの証明へ昇格しない。

| 入力 | 一意の採否・結果 |
| --- | --- |
| workload候補 | 現在のeffective UIDが所有する`/proc/<pid>`だけを対象とし、`comm`と実行fileの末尾名が共にexact `codex`であるprocessを調べる。他UIDは本profileのworkloadへ含めない。候補のidentity・祖先・FDを確認できなければcycle全体を拒否する |
| process identity・親子関係 | `stat`のpid/ppid/starttime、process directoryのUIDと、同UIDのprocessでは`exe`のdevice/inodeを読む。候補および親からPID 1までの祖先をFD列挙の前後で再確認し、公開へ使うworkload ownerとその祖先は候補集合公開直前にも再確認する。各tupleと親子関係の全一致を要し、PID再利用、reparent、読取不能、cycleは拒否する。異なるUIDの祖先はOSの保護でexeを読めない場合があるため、UID/pid/ppid/starttimeの一致を使い、本profileのartifact候補へ含めない。同UIDのexe不明をこの扱いへ逃がさない。PID 1は走査の終点であり、user clientの証明ではない |
| exact Codex Info artifact | collector自身の実行fileのdevice/inode、または以下のgeneration/member資格を満たすexact member `codex_info,codex_info_recorder,codex_info_rest`。実行fileのcanonical pathからそのgenerationを選び、current以外の稼働generationも同じ規則で扱う。currentへのsymlink、名前、version、隣接しているだけのmanifestでは代用しない |
| observer child | NUL区切りのargvでprogramの直後がexact `app-server`であり、上で確認した祖先にexact Codex Info artifactを含むCodex processは観測用であり、全Codex Info process分を除外する。argvは型・終端を確認し本文や環境変数を収集・保存しない。祖先がCodex Infoの製品member名を持つのに上記artifact資格が成立しなければ、利用者clientへ読み替えずcycleを拒否する |
| 祖先不明のapp-server | app-server自身がPID 1、または直親がPID 1である場合は、利用者clientかreparentされたobserverかを確認できないためcycleを拒否する。前cycleのobserver identityが残っている場合も現在の祖先を再確認し、以前の除外資格を流用しない。生成ownerは終了時にchildをreapする |
| FDとpath | Codex候補の全FDを有限境界内で列挙し、通常fileかつcanonical sessions root下のjsonlを持つownerだけをこのsurfaceの判定対象にする。適格workloadのpathを`canonical path -> nonempty ProcessIdentity set`へ加え、公開直前に同じ集合とowner/祖先資格を確認する。途中の権限・I/O・identity失敗、上限超過は部分集合やemptyに変換しない。同じpathを持つ別ownerを一つのPIDへ縮約しない。完全に列挙して対象pathを持たないprocessと、前後検証して除外したobserverの正常終了は、公開workloadの消失と扱わない |

process/FDの数値上限とraw読取り境界は第4.4章の製品資源上限であり、Codex/providerの仕様値ではない。cmdlineは64KiB、祖先数は重複なしの最大256 process、`stat`は64KiB、`comm`は64byteに限る。上限内でも必要な終端/fieldを得られなければ拒否する。これらの読取り上限をtaskの有効期間、idle時間、account/period所属の許容差へ流用しない。artifact検証はread-onlyであり、installer、更新、再起動、native設定変更を起動しない。

ここで使うgeneration/member資格は、PRODUCTのbundle identityをprocess除外へ適用する次の有限条件である。install/cleanupの全member検査やservice readinessの代用ではなく、この判定のために無関係なbundle memberを再走査しない。

- executableは現在のユーザーの`$HOME/.local/share/codex-info/generations/<G>/<member>`の直下にあるcanonical regular fileとする。`<G>`とmanifest、対象memberはsymlinkではなく、同UID所有を必要とする。generation directoryのmodeは0700、manifestは0644、対象memberは0755とする。processの`exe`のdevice/inodeと対象memberを前後照合する。
- `manifest.json`は1MiB以下の完全なUTF-8 JSON objectとし、どの階層もduplicate keyを拒否する。top keyはexact `schema,product,version,source_sha,run_id,run_attempt,target,compatibility,glibc_minimum,files`とする。`schema="codex-info-linux-bundle-v1"`、`product="codex_info"`、`target="x86_64-unknown-linux-gnu"`、`compatibility="glibc"`を必要とする。これらは本製品のbundle形式・対応target値であり、任意のprovider値を上書きする既定値ではない。
- versionはPRODUCTのcanonical stable/beta版（最大32 ASCII文字）、source_shaは小文字hex40桁、run_idは先頭0なしの正のASCII十進文字列、run_attemptは正のUInt64とする。betaのattemptはrun_attemptと一致させる。glibc_minimumはASCII十進成分をdotで結ぶ2成分以上の文字列とする。`<G>`はexact `version + "-" + source_sha + "-" + SHA256(manifest原本bytesの小文字hex64桁)`と一致する。
- filesはobjectの配列で、各objectのkeyはexact `path,size,sha256,mode`とする。pathは非空の相対POSIX pathで、絶対path、backslash、空/`.`/`..` component、`manifest.json`、`SHA256SUMS`を拒否する。pathはUTF-8 unsigned byte順の厳密昇順とし重複を許さない。sizeはUInt64、sha256は小文字hex64桁、modeは十進420（0644）または493（0755）だけを許す。
- 対象memberのentryはexact 1件で、mode=493とする。対象fileの元bytesのsize/SHA-256をentryと照合し、同じFDとpathのidentity、manifestのidentity/元bytesを前後検証する。不一致、切詰め、途中差替え、unknown schema、上限超過はartifact資格なしとし、上表の疑わしい祖先をnormal clientへ読み替えない。1MiBはmanifest読取りの製品資源上限であり、upstream protocolやgeneration有効期間の意味を持たない。

local usageのcheckpointは、未処理recordを隔離してその後の独立した数値を保存できる。このため、そのoffset・監査用last model/task・counterだけをlive rolloutの検証済みprefixとして使わない。live状態は第2章11の同一cycleの適格process identity集合とcanonical path、同じ取得群の`account/read`による認証主体前後確認、および`thread/read`によるID/path照合をそれぞれ満たしたsourceから得る。`thread/read`にaccount owner fieldがあると仮定しない。open FD、basename、mtime、最後のDB task値、未処理bytesの存在だけでrunningを生成しない。statusが`active|idle|systemError`の場合と、`notLoaded`で原本taskだけを使う場合の優先関係はPRODUCT `THREAD-OPEN-362`に従う。

liveのtask/contextに使うrecordにも、上の認識位置・重要key非重複・型・public範囲の元timestampを適用する。正常NoOpは下表の認識済み形と型を検証し、本文を状態へ昇格しない。未知のusage/control、不正なtask/context、invalid UTF-8/JSON、またはoversize recordが状態へ影響しないことを完全検証できない場合は、そのcycleのlive候補集合全体を拒否する。local保存での数値とcontext pairの独立採否と、live候補集合の採否は別である。liveに使用する既知token_countでも、提供されたcounter/context情報が上の型・raw算術・pairの採否に違反すればcycle全体を拒否する。有効な片側の値はlocal usage側へ保存できるが、不正recordを含む新live snapshotへ部分的に反映しない。info=nullや任意成分・pairの欠落/nullなど契約上許す不明は、この不正に含めない。counter不正からtask開始/停止やmodel切替を生成せず、壊れたJSON/envelopeを読み飛ばして正常としない。第2章5のstrict live隔離を緩めない。

再利用できるlive解析状態は、現在のcollector/account epoch内で、同じsource identity・session metadataのbyte 0から改行済み位置までをlive規則で検証した状態だけである。解析済み位置・そのprefixの原本hash・model資格・taskの既知性・context pairを一組にし、次の読取りでprefix継続と固定suffixの解析bytesを再確認する。usage checkpointのoffsetへ飛越しせず、未処理範囲をこのcacheの確認済み位置へ含めない。新source、identity/prefix不一致、account境界、cacheを保持するcollector processの再起動では旧cacheを資格へ流用せず、原本先頭から再構築する。各cycleのCodex process identityとFD資格はcacheとは独立して前後検証し、以前のprocess集合を現在openの証明へ再利用しない。不正cycleのcandidateからcache/表示の位置だけを進めず、直前の検証済みcacheと完全snapshotを保持する。部分的なprocess/FD収集で欠落pathを終了済みとせず、完全な新しいopen集合を受理した場合だけ閉じたsourceのcacheを除去できる。別の永続task正本やDB履歴からのlive再生を追加しない。

改行済みprefixの再構築中や同一inodeの未改行tail待機中は、部分Thread集合・確定0件・推定runningを公開しない。既受理の完全snapshotがあれば未確認として保持し、初回なら件数と状態を未知として保持する。必要なprefixが全て成立した後だけ、そのcycleの全候補を一括受理する。正常に検証したsourceにtask eventが一度もなければ、taskは既知falseではなく未観測であり、`notLoaded`を停止中へ変換しない。有効な最終taskを得た場合だけ、そのbyte順での開始/停止を使う。

liveの`last_user_message_at`は、正常な`event_msg/user_message`、またはroot `response_item`でpayloadの文字列`type="message",role="user"`を確認したrecordのpublic範囲のroot timestampだけから得る。source byte順で最後の該当recordを使い、本文、thread名、mtime、thread/readのupdated_atから生成しない。response_itemのtype/roleはこの時刻判定に限るmetadataであり、token/model/taskの観測には使わない。両keyは欠落を許すが提供値は文字列を必要とし、重複keyまたは型不正があればlive cycleを拒否する。該当recordがなければ未観測のまま、正常な別種recordだけでは以前の有効なuser-message時刻を変更しない。本文をDB、REST、診断logへ抽出しない。

#### model contextと差分区間の帰属

model contextの優先関係は、source lineageごとの`none / initial-settings / turn / unknown`で決める。検証済み先頭prefixからcontext未出現なら`none`、その状態で有効な初期設定を採用すれば`initial-settings`、有効なturn/thread contextなら以前のkindによらず`turn`とする。各context recordで提供されたmodel位置は全て型・正規化後の一致を検証し、不正なturn/thread contextはkindを`unknown`、有効なmodel帰属を不明にする。`unknown`から弱い初期設定で復帰せず、有効なturn/thread contextを必要とする。有効な初期設定はkindが`none`の場合だけ採用する。それ以外の有効な設定は現在の帰属を変えない。不正な初期設定は当該sourceを未処理・不完備とし、kindが`turn`以外なら`unknown`にする。既存の有効turn contextは初期設定の型不正だけでは上書きしない。

有効なcontext recordには、入力表に列挙したmodel位置の少なくとも一つに有効な文字列が必要である。turn/thread contextのどの位置にもmodelがない場合、全位置がnullの場合、または一つでも提供値が不正な場合は不正な強contextであり、`unknown`とする。model不記載を「前回modelの確認」としない。初期設定もmodel位置0件は不正な弱contextであり、上記の優先規則で扱う。明示nullは文字列の省略やmodelクリア命令ではない。

このkindは既存checkpointのnullable TEXT `last_model_context_kind`へ同じtransactionで保存する。許可値は上記4文字列とSQL NULLだけで、新しい既知状態にNULLを使わない。`none`なら有効なlast modelなし、`initial-settings/turn`なら有効なlast modelを必要とする。`unknown`の保存済みlast modelは監査用の旧値を保持してよいが、有効な帰属へ使わない。v14/v15からのv16 schema移行は列をNULLで追加して既存last modelを変更しない。この新列を含むcheckpoint shapeをv16未満の宣言versionとして受理せず、v16で新列が欠落したshapeも拒否する。versionとshapeを片方だけで判定しない。旧NULLはkind不明であり`none`にせず、同じaccepted prefixの再検証、または新しい有効なturn/thread contextで資格を得るまで新deltaのmodelへ流用しない。再起動もこの規則で復元し、counter 5成分の一致だけでは別sourceのmodel contextを継承しない。旧kind不明をsource fingerprintによる再検証省略の理由にしない。新しいsourceの検証済み先頭からは`none`で始め、byte継続を証明できないresumeは`unknown`で始める。

現在のmodel contextと、前回raw counterから次のraw counterまでの差分帰属は別の資格である。差分区間は同じlineageの前回受理済みraw counterの直後から、今回counterを含むsource byte順で定める。その区間で採用したcontextの正規化済みmodel IDが1種類ならそのID、contextの採用が0件なら前回counter時点から継続して有効なIDを候補にする。前回counterより前のIDは、新しいcontextが採用された区間の種類数へ加えない。同じIDの反復は1種類、異なるIDが2種類以上なら区間全体を未帰属とし、最後のIDやcounterの時刻に近いIDへ全量を付けない。優先規則で無視した有効な初期設定は区間の候補に加えない。

不正なturn/thread context、意味を判定できないusage/control record、または区間中のmodel状態の連続性を証明できない境界があれば、その区間は未帰属のままとする。後続の有効なcontextは以後の現在contextを回復させるが、既に混在・不明となった同じ差分区間を遡って一意にしない。初期設定と認識できたrecordの型不正だけでは、既存の有効なturn contextの帰属を取り消さない。ただしその原本の未処理・不完備は保持する。認識済みの正常なNoOp recordは差分帰属を変えない。

差分が算術上有効でもmodelを一意に決められない場合、raw比較baselineは今回値へ進め、数値差分・元のbyte区間・不明理由を内部の未帰属情報として保持する。公開modelの累積へ加算しない。この未帰属を理由に後続の独立した正常区間まで拒否しない。有効raw baselineの初回受理、通常deltaの受理（total差0も含む）、または認識済みcontext調整のbaseline更新を境に次の差分区間を開始する。`info=null`、不正counter、曖昧なcontext調整、未照合anchorでは新しい区間を開始せず、前回baselineと区間内の混在・不明を保持する。区間を開始しても以前の未処理range、source欠損、未照合anchor、完備性の欠如は解消しない。

この区間資格はcheckpointと同じtransactionで永続化する。保存すべき意味は、区間内で採用したIDが0種類か1種類か、その1種類のID、および混在/連続性不明があるかである。v16のnullable TEXT `last_model_interval`は、0種類を`none`、1種類を`model:`に正規化済みIDをそのまま続けた文字列、混在を`mixed`、連続性不明を`unknown`として保存する。ID部分には通常のmodel ID検証を適用し、空ID・別表現・既定値による補完を受理しない。1種類の状態は有効な現在context（`initial-settings/turn`）と同じlast modelを必要とし、矛盾する保存状態はDB資格エラーとして拒否する。2種類以上のID一覧や本文の保存は不要で、混在は次の区間開始まで解除しない。v14/v15移行では列をSQL NULLで追加して他の値を変えず、v16はこの列も含むshapeを必要とする。旧NULLは区間不明であり、最後のmodel名だけから復元しない。同じaccepted prefixの再検証で資格を復元するか、次の有効raw baseline更新までその差分を未帰属とする。途中でbatchが分割されても、再起動しても同じbyte列の帰属結果が変わってはならない。

legacyの採用済みcontextの元位置は、`source_admission` v2のoptionalな`adopted_context`へClassificationPointerとして保存する。これは現在のkind/last modelを決めた元recordの根拠であり、別のmodel正本ではない。採用した初期設定・turn/thread contextでだけ置換し、優先規則で無視した設定では置換しない。現在のkindがunknownへ移る場合は有効pointerも失わせる。pointer自体は元byte範囲/hashを持ち、timestampを重複保存しない。参照先のcontext原本には前掲入力表の有効timestampを引き続き必要とし、pointerの保存fieldに時刻がないことを時刻欠落recordの採用理由にしない。raw counter両端やresponse rootの原時刻を、このcontextの時刻で代用しない。

旧v2でこのfieldが欠落/nullならcontext根拠不明として読めるが、kindが`initial-settings/turn`の旧pointerを最新recordや最後のmodel名から発明しない。同じaccepted prefixをstate-onlyで再検証して元pointerを復元し、旧数値を再加算せずcheckpointと一緒に保存する。提供済みpointerの型・非空範囲・終端・single SHA-256が不正なら保存状態を拒否する。pointerを要しない`none/unknown`だけを理由に全prefix再解析を繰り返さず、既存の再読省略・失敗保持・再検証の規則を守る。

同じ再開状態には、検証済みsession_meta.idと既知の場合のsession_id、その資格、有効な強contextのturn_id/root_turn_idと原本位置・元時刻、および前回raw比較anchorの原本位置・元時刻も保持する。既存のlast_model/context kindと矛盾する独立したmodel正本は作らない。強contextが無効になればturn IDの有効資格も失わせ、監査用の旧IDを新responseへ流用しない。legacy入力でturn IDが未提供でもmodel帰属の既存資格を消さないが、responseの同turn証明には使えない。前anchorの時刻が不明な旧checkpointでは連続所有区間を推測せず、同じaccepted prefixから復元するまでその差分を加算しない。これらの資格はcounter値の一致や再起動だけでは復元しない。

accountへdeltaを帰属するには、両端のraw観測が同じ一つの連続したaccount所有区間に含まれることも必要である。A→B→Aの離れた二つのA区間に片端ずつ含まれるだけでは資格を満たさず、間のB利用をAへ加えない。所有境界を跨ぐrawは新しいaccountの消費量の証明にせず、そのaccountで検証した以後の比較baselineとしてだけ保持する。原本復元でも一つの所有区間を確認し、各端点を独立に`is_owned`判定するだけでは採用しない。sourceの連続性が不明なら、所有時刻の一致だけでその連続性を復元しない。

#### 数値eventの公式期間所属

ここで保存する既存v1のexact R/Wと`period` proofは、元の公式取得履歴との**報告枠への観測対応**を再現する資格である。PRODUCT `RECORDED-USAGE-SCOPE-604`の実利用期間identityとは分離する。下記のrunが成立しても不変の実週IDを得たことにはせず、旧proofをその意味へ改変しない。v1の同じRという保存keyの等値を、providerが同一実期間を保証したという根拠に使わない。source/account・元時刻・numeric・modelが確認済みの消費は、この観測対応の不明とは別にrecorded factとして保持できる。

##### 実利用期間に依存しない記録分の保存投影

`session_events`の元key・数値・元時刻・admission、およびsealed repairの原本資格を数値の正本とする。
以下の補助materializationはread-only consumerへ資格済みの記録分を渡す派生indexであり、別の数値authorityではない。
readerはwriter crateへ依存せず、Session原本や認証を再読しない。

- `recorded_usage_facts`は一意な消費factの非秘密ID、元観測秒、元model key、lossless T/I/C/W/O、
  元event keyへのprivateな参照、適用したsource/numeric資格と内容SHAを保持する。外へ返すID・値へraw AccountKey、
  path、session/turn/response ID、認証情報を含めない。fact IDのresponse形式は既存全5 IDのcanonical keyへ結び、
  counter形式はDATAがcanonicalizeした完全なsource event keyへ結ぶ。hash衝突や同IDの異値を同一性とみなさない。
- `recorded_usage_prefixes`はmodel・元観測秒ごとの派生prefixを持つ。Tと既知成分和、成分ごとの不明数をlosslessに保持し、
  範囲差で不明成分を0にしない。非負の正確な整数で計算し、公開UInt64へ表現できない値はclampせずその投影を利用不能にする。
  順序は元観測秒・canonical fact keyで固定し、遅着・訂正は影響modelの依存suffixを同じtransactionで再構成する。
- `recorded_usage_state`は一つのheadとしてformat version、fact集合revision、model key集合、coverage根拠を保持する。
  factの同一性・数値・元秒が変わらないperiod proofだけの更新ではfact revisionを進めない。予定されたcollection cycleや
  公式quotaのgenerationを取消す規則ではない。
- 三表はowner-only account DB内に作り、全て未作成の旧DBを読取り可能とする。部分的な組・未知format・不正shapeは
  資格済みindexへ通さない。作成・bootstrap・更新は同じusage transactionへ含める。旧DBを開くread-only readerが表を作らない。
  table/列/PK、必須NOT NULL、WITHOUT ROWIDを採用する表ではその形式もvalidatorが照合する。

materializationへ入るeventは、元source bindingと同じpartition・root/path/device/inode/generation、固定prefix、所有区間、
model context、legacy両端またはresponse全ID/usageを満たす。既存admissionの形だけで新しい原本照合を行ったとしない。
commit済みproducer資格はその元provenanceとして再利用できるが、admissionなしの旧event、未資格の復元候補、
counter lineage不明をbootstrapで昇格しない。period proofまたはR/Wだけは記録分の数値資格と分離する。

同5 IDのresponse再掲は一つのfactである。同physicalの異generationは下記の全prefix canonicalization条件を満たすときだけ
一つに退役し、証明がない組をkeyの大小だけで重複除去しない。別bytesへの全旧event一対一再対応でも旧保存key・数値・価格を保全し、
新しい根拠と旧pointerをsealed provenanceへ保持して、一つのfactへ対応させる。後から証明が回復したときは旧新indexと全依存prefixを
一つのtransactionで再構成し、追加消費として足さない。

復旧済みの旧pointerは、commit済みの全prefix修復provenanceと同じ読取りtransactionの採用済みcanonical factに対応する場合だけ、
保存観測の`source-recovery-pending`を解消する。原観測の数値・NULL・保存keyは置換せず、`period-identity-unknown`は残す。
alias／全event再対応付けは記録分の重複と資格を解決する操作であり、既存`usage_history`・model sidecar・durable観測の
旧数値や価格を新しい記録分の累計で上書きするauthorityにはならない。旧観測と新しいcanonical記録分はそれぞれの元時刻とscopeで保持する。

eventの追加・退役・数値訂正・source資格修復・canonicalizationの通常writer入口は全てこの投影を同じusage transactionで更新する。
quota receiptの独立した保存は変更しない。失敗時は元event・cursor・head・prefixを共にrollbackし、exact pendingとlast-goodを保持する。
同じ事実の再適用はSQL no-opとし、同じsourceを再scanしただけでfact revision・記録分を増やさない。
bootstrapは保存済み資格のDB読取りだけで行い、startupや公開requestを理由にSession全prefixを再scanしない。

coverage completeを得るには、範囲へ影響するsource inventoryと連続所有区間が既知で、対象prefix/rangeが全て検証済みであり、
未資格event・未処理suffix・重複未解決・元時刻不明・明示gapがないことを必要とする。時刻不明の未処理を任意の範囲の外へ追い出さない。
既知の対象欠落があればpartial、対象inventory自体が未確認ならunknownである。記録分のknown値はどちらでも保持する。

価格は同じfact集合・model・prefixへの保存根拠がある場合だけ派生indexへ結び付ける。元の保存USDを移行で再計算しない。
eventごとの価格根拠がない旧データを、現在catalogや同じtoken数だけで価格付きfactへ変換しない。旧額は元の保存観測に残し、
新しい記録scopeの価格は不明とする。verified backup、migration、再起動では三表とその元証拠をSQLite一貫copyへ含め、
直接consumer gateで元key・数値・価格bits・indexの保持と不正形式拒否を確認する。

本製品のmodel別期間集計は、原本の消費観測時刻を、同accountの公式quota観測履歴へ対応付けた集計である。公式APIはresponse IDと課金periodの対応を提供せず、tokenごとの実課金時刻・別端末の消費も提供しない。その対応が得られるとは表明しない。legacy-counterの消費観測時刻はdelta終点の原token_count timestamp、response-recordはroot token_usage_recordの原timestampであり、収集時刻へ移さない。Sessionのrate_limitsは8.7.0のcached snapshotであり、そのrecordの時刻に取得されたquota、または同recordの消費の所属証明とは扱わない。

所属判定に使う公式観測の時間区間は次の通りである。`QUOTA-DURABLE-604`の受理済みreceiptは元のobserved_atを両端とする点区間とし、元の取得sequenceも保持する。取得journal導入前の保存済みcanonical quotaは、公式取得値との保存provenanceを検証できる場合だけ、canonical minuteの先頭から末尾秒までの閉区間とする。receiptとの同一性を証明できればそのreceiptの点区間を使う。時刻精度を推測して狭めず、Session snapshot由来・取得元不明・矛盾する旧値は所属判定の観測に使わない。periodを支持する観測には検証済みexact reset Rとwindow Wを必要とし、区間を公式window `[R-W,R)`との共通部分に限定する。共通部分がなければperiodを支持せず、下記のbarrierとして扱う。保存値・取得順序をこのために変更しない。

##### 公式取得から所属証拠への投影

`quota_receipts`の有効な数値だけでは所属判定の入力集合は完結しない。同accountの取得journalに保存され、前後account確認・同一process・raw/lease/admissionの照合が成功してaccount確認済みとなった原本も、元の不変ID、取得sequence、observed_at、解釈結果を結んで扱う。account確認前・確認失敗・別scopeの原本を、そのaccountのbarrierやperiodへ昇格しない。元時刻が公開範囲内にあり、一つの連続所有区間へ結べることも必要である。

| 同じ公式取得原本の状態 | 所属判定への入力 | current quotaとの関係 |
| --- | --- | --- |
| PRODUCTの完全window選択が成功 | 選択した一つのR/Wを元時刻の点区間として渡す | 同じ選択値を通常のreceiptとして受理する |
| 別の候補だけがW不明・数値資格不足、または選択されなかった完全window | その候補を別観測・window競合・barrierとして追加しない | 選択済みの完全windowを妨げない |
| account確認済み原本だが、型不正・未知形式・完全windowなしで選択不能 | 元時刻の点をperiod不明のbarrierとして渡す。R/W/Remainingを生成しない | 最後の受理済みquotaを保持し、取得解釈失敗を独立に公開する |
| 保存provenanceを満たす旧公式観測でWだけ不明 | 元の精度の区間をbarrierとし、8.7.0の一意な補助metadataが成立した場合だけ同じ観測を再判定する | cached Session値から新しいquota receipt・実測点を生成しない |
| 通信失敗で応答原本なし、account確認不成立、または公式参照のない旧値 | 所属用の観測・barrierを生成しない。該当ownerの失敗・未確認診断を保持する | last-good、取得失敗、account境界はそれぞれのownerに従う |

「同じRに複数のWがある」は、各取得原本で上記選択を完了した後の公式観測同士について判定する。一つの応答に短期primaryと長期secondaryが同居するだけではwindow競合ではない。異なる取得原本で選ばれた同じRの正Wが異なる場合は、後述の競合規則を適用する。

R自体を解釈できないbarrierにも、実在する取得原本のID・元時刻・sequenceと理由を保持する。仮のresetやWを付けて有効観測型へ通さず、その点を含むcomponentをperiod不明として扱う。barrierのsequenceも時計逆行の検査に含める。barrierを挟む二つの同R/Wの観測から、その間を一つのrunへ補間しない。その後の独立した有効観測からは通常通り所属を再開できる。元時刻・所有区間自体を検証できない入力は、任意の時刻へbarrierを配置せず、accountの根拠不足診断として保持する。

原本のdurable保存、current quotaとしての受理、model所属証拠としての適用を別に記録する。current用に未適用の原本でも、上記の確認済みbarrierを所属証拠へ一度適用できる。取得原本は変更せず、同じ原本ID・同じ解釈の再適用で証拠集合や世代を増やさない。後に同じ原本を正しく解釈できた場合は、その原本のbarrierを解釈済みの選択観測へ置換し、元のsequence位置で全規則を再判定する。旧解釈と根拠は修復provenanceに残す。原本を新規取得したことにしたり、barrierと有効観測の二件へ複製しない。

所属の計算は一つのusage transactionが固定した証拠snapshotに対して行う。snapshotは全適用原本の不変参照・内容hash・解釈結果、補助windowの元pointer、所有区間と、計算に使った各範囲を再現可能にする。未commitの候補、別snapshotのreceipt、commit後に到着した原本を混ぜない。並行取得された原本は次の通常cycleで適用する。失敗pendingの再適用では固定した証拠snapshotを保持し、新証拠を黙って差し替えない。新証拠はpending成功後の次cycleで再判定する。source/accountの再検証失敗によるpending取消しは本節の原本資格規則に従う。

固定例: W=500、sequence1のt=600/R=1000、確認済み未解釈のsequence2のt=650、sequence3のt=700/R=1000では、625/650/675の消費は未所属である。sequence4のt=750/R=1000が加われば725はR=1000へ所属できる。sequence2の原本が解釈回復してt=650/R=1000/W=500となれば、同じ元時刻のまま625/650/675を再判定する。一方、sequence2の一応答内でprimary W=200、secondary W=500がともに有効なら、選択値はW=500だけであり、二つのWによる競合にはしない。

ここで公式取得との保存provenanceとは、保存済みの不変な公式取得原本IDまたはreceipt IDへの明示参照と、参照先のaccount scope・受理資格・時刻精度・exact R/W・Remainingの全一致である。原本でWが欠落している旧公式観測は、その欠落自体の一致まで検証し、W既知のperiod支持観測と区別する。補助metadataを適用しても、公式原本にWが存在したことにはしない。timestamp/reset/Remainingの同値探索だけ、model_source=confirmed、旧writerの通常経路、非NULLであることでは代用しない。旧durable observationに参照がなければそれ単独では資格なしとする。別途存在する受理済みreceiptは、その旧rowとの対応を発明せず、独立した公式観測としてその元時刻で利用できる。取得元不明の旧quota値はDB/backupへ保持するが、公開RemainingはNULLとし、period別診断に取得根拠不足を残す。これは保存値の0化やschema migrationでの値削除ではない。

期間判定へ渡す時間区間は、その観測の帰属を証明した一つの連続account所有区間との共通部分にも限定する。元の分精度区間が所有境界を跨ぐ場合はstart/endでclipし、別account側の秒を含めない。一つの所有区間へ一意に結べない観測は、別account/別所有区間の観測へ複製せず所属証拠から除く。公式quotaの取得成否・元時刻・取得順は変更しない。

同partitionの同じexact Rについて、受理済み公式原本に異なる正のWがある場合はwindow競合とする。そのRの観測をmodel所属の証拠へ使わず、originを不明にする。公式取得原本・各receiptと現在quotaの取得順は保持し、最新Wで過去Wを上書きしない。欠落Wは、8.7.0の一意な補助metadataが成立した場合だけ埋められる。競合する既知Wを多数決やSession snapshotで解消しない。

受理済み公式観測でも、W不明/競合または元時刻とwindowの共通部分がないものは、その元観測区間を所属判定のbarrierにする。上記のaccount確認済み未解釈原本も同じbarrierである。前後の同じRをこの位置を飛ばして一つのrunへ結ばない。barrier内の消費は未所属とし、その後の独立した有効観測からrunを再開する。受信できなかった通信失敗は公式観測ではなく、新しいperiodやbarrier観測点を生成しない。元の失敗・quota gapはそれぞれのownerで保持する。

同じ連続account所有区間で、上の時間区間が重なる観測を推移的に一つのcomponentへまとめる。各componentの下端は最小下端、上端は最大上端である。全観測のexact R/Wが一致するcomponentだけを単一periodの証拠にする。異なるR/Wを含むcomponentは競合であり、その内部を時刻やsequenceで分割しない。componentを時間順に並べ、間に別R/Wまたは競合componentを挟まない同じR/Wの連続列をrunとする。通常、消費観測時刻tをrunへ所属させられる範囲は、最初のcomponentの上端以上、最後のcomponentの下端以下である。単一の点componentは同じtだけを許し、幅のあるcomponent一つだけでは区間を確定しない。公式windowとの共通部分も必須とする。片側しか観測がない区間へ外挿せず、次の通常cycleの公式観測が保存されたときに未所属候補を再判定する。判定専用の追加RPCやbusy pollは行わない。

公式windowの起点S=R-Wで切替を確定できる場合だけ、上記のrun境界を拡張する。直前componentが競合のない別R/Wであり、その上端がSより前、後続runの最初のcomponentの下端がS以上、両者が同じ連続account所有区間にある場合は、切替時刻をSに定める。直前runの右端をS未満、後続runの左端をS以上にする。これにより公式起点以後の最初の消費を、最初のpoll前という理由だけで失わない。起点が直前観測より前へ動いた場合、競合componentを挟む場合、または所有が不連続な場合にはこの拡張を使わず、間の所属は不明とする。起点へ100%・token 0・quota実測点を生成する規則ではない。

公式receiptの取得sequenceが進んだのにobserved_atが後退した場合は、後退前後の時刻を含む閉区間を時計競合として上記runを分断する。同じ秒に異なるR/Wがある場合も競合componentになる。現在quotaは従来通り取得sequenceで選び、時刻順へ並べ替えて現在値を変更しない。競合区間を越えた後の独立した観測からrunを再開でき、過去の時計異常だけで以後全期間を不成立にしない。sequenceの数値が連番でないことだけでは欠落・時計異常にしない。

上記の範囲にtを含む一意なR/Wがあり、source・model・account所有の各資格も成立するeventだけをそのperiodへ所属させる。0件または複数なら、数値・model・原時刻・原本pointerと不成立理由を未所属として保持する。timestampがwindow内にあるだけ、最新receipt、近いreset、cached Session quota、隣のmodel値では埋めない。同じR/Wが別のrunで再登場しても報告枠の保存keyは同じであり、別R/Wを同じ保存keyへ丸めない。この等値は実利用期間の同一性の証明ではない。輸送障害によるquotaの欠測をmodel消費から修復せず、modelの所属証拠とquota pointの完備性は別に判定する。

既存session_eventsの同じeventにnullableなexact reset/windowの組と、判定に使った公式観測・時間範囲・規則のprovenanceを保存する。組は両方NULL、または公開範囲内のresetと正の期間長の両方既知だけを許し、片側だけを新規保存しない。v14/v15からのschema-only移行はNULLのまま旧eventを保持し、選択中periodやtimestamp窓から補わない。既存の対応原本を全prefix規則で検証し、上の公式観測履歴で所属を一意に決めた場合だけ復元できる。時刻窓で抽出した候補へ付けた対象resetは照合上の仮説であり、旧eventへ保存されたperiod値ではない。対象resetとSession snapshotのresetの不一致だけを、旧eventの誤所属の証明にしない。旧公開model frameの誤りは、実際の保存period key、losslessな全model vectorと、資格を得たevent prefixを別に照合して判定する。未所属eventをcanonical periodのmodel total/minuteへ加算せず、その影響がある範囲の完備性を確定しない。

未所属の有効responseの影響はperiodごとに判定する。公式reset/windowが既知なら、response原時刻がそのhalf-open window内にあるperiodだけを、その候補の影響範囲とする。該当periodではsource coverageの完備性を確定せず、公開する数値は検証済みの記録分として扱い、部分和を全期間利用量や確定0へ昇格しない。原時刻が既知windowの外にある過去の未処理だけで、そのperiodのmarkerや現在の取得laneを不成功へ変えない。期間長が不明なら包含を固定7日で決めず、そのperiodのcoverageを不明とする。原時刻も不正・欠落ならperiodへ推定配賦せず、accountの未処理診断に時刻不明として保持し、影響外を証明できないperiodをcompleteとしない。未知の値に対応する取得lane errorを作る規則ではなく、取得状態と履歴のcoverageは独立にする。

normal cycle、period切替時のseed、起動時復元、closed-periodのminute再投影は、全てこの保存済みexact reset/windowが対象と完全一致するeventだけを合計する。timestamp窓だけで異なるresetのeventを含めず、同一eventを重なった二つのperiodへ複製しない。時刻は当該window内の採否とminute位置の検証に使い、所属keyの代わりにはしない。遅着eventの原所属を保持し、現在のperiodへ付け替えない。初回のmodel totalが0か否かでeventの所属を変えず、quotaの変化とmodel消費の有無を分離する。

所属は消費量そのものと異なる派生資格であり、公式証拠の追加時にも同じ規則で再判定する。受理済みreceiptの追加、保存済み未解釈原本の解釈回復、または一意なwindow metadataの補完で証拠集合が変わったとき、未所属→所属、所属→未所属、所属R/W→別R/Wのいずれも新しい完全な証拠集合の結果だけに従う。以前の所属を保つため新証拠を無視せず、最新periodへ強制移動もしない。raw pointer・原時刻・usage・model資格が同一のeventを再加算せず、同じeventの所属provenanceを更新する。古い所属と根拠は修復provenanceに保持する。原本数値・model自体の食い違いはこの再判定の対象ではなく、通常の競合として隔離する。

通常cycleでの再判定は、影響を受ける保存済みeventの所属資格、旧新periodのmodel totals、依存するminute/suffix・sidecar・完備性・未解消の旧利用記録metadataを、一つのusage transactionで更新する。quota取得原本のdurable保存と受理済みreceiptは独立したownerのまま保持する。派生更新の失敗ではそのtransactionを全rollbackし、旧公開rootとexact pending batchを保持する。独立して受理済みの現在quotaを取り消さず、新しい公式取得も止めない。既存eventの数値・source checkpointを変更する操作ではないため、数値誤計算の全prefix修復へ偽装しない。数値自体の修復が必要な場合はPRODUCTのbackup付き原本修復を使う。金額は旧periodの額を移さず、変更後prefixに対応する保存価格の資格をPRODUCT通り判定する。

所属再判定では旧periodの累計減少・model行の消失も結果になり得る。これは原時刻順の系列へ負の消費eventを加える操作ではなく、その公式証拠集合に対する全依存prefixの再構成である。旧新のprefixを同じ公開世代へ混在させず、同じ新世代内では各periodの非負event和を保持する。資格を失った既知消費は未所属の数値候補へ残し、全modelが外れたことだけでcompleteな空集合・利用量0にしない。旧利用記録の保持metadataとselectorはPRODUCTの条件に従う。既にsource・数値・modelを原本で検証したeventの再判定に、毎回の新しいbackup・原本全prefix再scanは要求しない。一方、schema移行で所属NULLとなっただけの旧eventは、その資格をまだ持たず、前述の全prefix検証を省略できない。

固定例として、同一account所有区間、W=500、取得sequence1のt=600/R=1000とsequence3のt=700/R=1000だけが解釈済みなら、t=650の有効な10-token eventはR=1000へ属する。保存済み原本のsequence2が解釈回復しt=650/R=1001だった場合、同じeventはR=1001へ変わり、R=1000の依存prefixから10を除きR=1001へ一度だけ含める。t=625/675の候補は未所属となる。eventのusage・model・原時刻・raw pointerは変更せず、旧所属provenanceを保持する。sequence2の再適用はno-opである。新しい公式証拠がないのに同じ書換えを行うことは許可しない。

全証拠集合による判定が正解であり、通常cycleの実行方法は同じ結果を保った差分処理とする。新しいreceiptと取得sequenceの直前/直後の関係、新規または解消したclock barrier、Rごとのwindow競合、時間component/runの変更範囲を保持し、その範囲と旧新Rに依存するeventだけをindexed lookupで再判定する。復元した過去receiptも元のsequence位置へ入れる。同じ入力原本IDの再適用は証拠集合を増やさず、派生結果・世代を変えない。失敗pendingの入力集合は同一に保持する。startupで検証済み保存状態を読み直す場合を除き、全receipt ledger・全Session原本を公開ごとに再生しない。差分処理・再起動・全証拠からの再構成で同じ所属結果を必要とする。

#### event資格の保存形式

v16の`session_events`は、既存の数値・source keyを保持した同じrowにnullableな`reset_at INTEGER`、`window_seconds INTEGER`、`admission_json TEXT`を追加する。R/Wは双方NULLまたは双方既知とし、既知なら公開範囲内のR、正のW、公開範囲内の起点R-Wを要する。NULLは未所属であり、最新quota・選択period・timestamp窓で補わない。v14/v15からの移行はこの3列をNULLで追加し、旧rowの数値・時刻・key・件数を変更しない。旧版のread-only読取りも資格なしとして保持する。部分的な列の追加や、未知形式を有効資格とみなさない。

`admission_json`のversion 1は未知fieldを許さず、次の構造を使う。これは原本・公式取得を検証したproducerが保存する根拠であり、JSONの形が正しいだけで原本照合や認証を代替しない。

- `source_binding`: 本節の完全な既存v1 source/account結合。rowのpartition、root/path/device/inode/generationと一致する。
- `verified_prefix_end`: 当該数値採用時に全range/hashとLFを検証した固定prefix終端。結合元のprefix、rowのrangeおよび全根拠pointerを覆う。
- `ownership`: 有限の正の`start_at`とnullableな`end_at`による一つのhalf-open所有区間。legacyの両raw時刻、またはresponseの元root時刻がこの同じ区間に入る。初回RPC結合の場合は、その`began_at`より前の数値端点を採用しない。
- `numeric.kind=legacy-counter`: `before`と`after`の各原本pointer（start/end、single SHA-256、元timestamp）とrawの`total,input,cached_input,cache_write_input,output`を保持する。TはUInt64必須、他成分はUInt64/NULLであり、既存raw/delta算術をそのまま使う。`after`の元時刻とchecked deltaの値・knownnessがrowと完全一致する。`model_context`はnullableで、既知modelでは実際に採用したcontextの`pointer`（時刻を要求しない既存ClassificationPointer）、`kind`、既存表現の`interval`を必要とする。kindは前掲の優先規則で採用した`initial-settings`または`turn`だけとし、弱い設定を無条件に強contextへ昇格しない。初期kindが`none`で採用された初期設定からのlegacy差分は資格を得られる。有効な強contextの後で無視した設定は、元の有効contextの根拠を置換しない。intervalは`none`または同じmodelの`model:<ID>`だけを許し、model不足を最新名で埋めない。`none/unknown`は採用済みcontextのkindとして保存しない。提供されたmodel_contextの構造検証はmodel未帰属でも省略しない。このlegacyの初期設定資格を、response-recordの強context・同turn証明へ流用しない。
- `numeric.kind=legacy-family-counter`: PRODUCTの保存済み旧family key例外を全prefix修復で適用した行だけに使う。`proof`は上のlegacy-counterと同じ元pointer・raw差分・実際のmodel contextであり、`exact_model`に原本と一意に照合したexact IDを保持する。rowの旧keyはSOL/TERRA/LUNAのまま維持し、その固定互換familyとexact IDの対応を検証する。数値・knownnessは元rowと完全一致を必要とし、contextのintervalを書き換えず元exact IDとの一致を検証する。通常の新規記録や既存exact ID同士の照合をfamily一致へ緩める根拠にはしない。
- `numeric.kind=response-record`: `candidate`に既存の完全なPendingResponseを保持する。全5 ID、root原時刻、usage全6成分、分類根拠、元contextとSession scopeを検証し、資格済みmodelおよびusageのT/I/C/W/Oがrowと一致する。root pointerを主pointerとし、turn/thread snapshotを加算しない。重複keyと競合の扱いは前掲規則を変えない。
- 全prefix修復で旧family counterを対応rootへ退役する場合だけ、response-recordの`legacy_family_exact_model`に原本のexact IDを保持できる。rowの旧SOL/TERRA/LUNA keyと当該exact IDの固定互換対応、rootの資格済みmodelとの完全一致を要する。通常新規eventはこのfieldを持たない。同5 IDの再掲は保存された原本exact IDで同一性を検証し、旧family keyをexact名へ書き換えたり追加利用にしない。
- `period`: 未所属ならNULL。所属済みなら`rule_version=1`、所属可能な閉区間の`lower_at,upper_at`、公式証拠集合を読んだ`evidence_generation`（canonicalなUInt64十進文字列）、同集合の`evidence_sha256`（single SHA-256の小文字64桁）を保持する。R/Wはrowの2列を唯一の保存値とし、period proofの有無と一致させる。row原時刻がこの範囲・公式window・所有区間に入ることを必要とする。receiptの`applied_generation`と元の取得sequence、補助windowの原本pointer、所有区間から証拠集合を再現し、証拠追加後の再判定でも旧根拠を修復provenanceへ保持する。

`evidence_generation`はこのpartitionでcommit済みの所属証拠snapshotの世代であり、取得sequenceやcollector epochと同一視しない。`evidence_sha256`はその世代で保存した不変snapshot表現の元bytesに対する一回のSHA-256である。保存表現はversion、完全partition、元原本の参照と解釈・補助window・所有区間を欠落なく再現できる必要があり、後でJSONを再serializeした値や現在のledgerのhashで代用しない。内部の表分割・共有・差分保存は固定しないが、参照先も同じsnapshotへ不変に結合し、旧世代の読取りとhash照合を再現できることを必要とする。通常cycleは前述の差分処理を守り、hashの作成だけを理由に全ledger・全原本を毎回再scanしない。

旧rowのNULLなJSONはsource・model・period資格が不明であることを表す。JSONが提供された場合はversion、型、pointer、partition/source、原時刻、数値knownness、model、R/Wと根拠の一致を検証し、不成立なら読取り・採用を失敗させて旧rowを保持する。値を捨てずに読むraw adapterと、資格を得たeventだけを公開集計へ使うconsumerを区別する。再適用時に既存資格をNULLで消したり、部分成分を0へ変えたりしない。

#### 部分成分を含む履歴の保存

原本資格と公式所属を得たmodel prefixでは、T、各内訳、保存USDの既知性を独立に保持する。既存の`usage_history`の固定family欄、`usage_model_history`、対応durable observationにも同じknownnessを保存し、型が必須数値である旧adapterの都合で0を生成したり、既知Tまで捨てたりしない。固定familyのtokenはそのfamilyに属する資格済みTのchecked和、USDはそのprefixに対応する保存価格の資格を持つ全model額が既知のときだけの和である。model集合が不完全でfamilyの観測が一つもなければ、そのfamilyの値は不明であり0ではない。未知の一つの額を和から除いて部分額を完全なfamily額へ昇格しない。元model keyと個別の既知事実は残す。

同じexact reset/minuteを更新する通常収集では、資格済みevent集合が以前のprefixを包含することを必要とする。同じ公式証拠集合での非負event追加によりprefixが進んだ場合は、新prefixのTと各成分を一緒に保存する。新たなdeltaの成分が不明なら、その成分のprefix和は不明となり、前prefixの既知値を新prefixの合計として持ち越さない。逆に過去に不明な成分は、後続deltaが既知になっただけでは既知へ戻さない。同じevent集合・同じ数値/knownnessの再適用では保存USDと価格版を保持する。prefixが変わったときのUSDは変更後prefixに対する価格資格で決め、以前のprefixの額や最新catalogで穴埋めしない。同minute内の古いquotaはmodel prefix選択の根拠にせず、quota代表は公式取得順の規則だけを使う。

未検証の旧rowや別sourceのcandidateについて、timestampの新旧、Tの大小、NULLの大小比較だけではprefix包含を証明しない。包含を証明できないcandidateは元の事実と不成立理由を保持し、以前の有効rowを置換しない。旧値の訂正・未知成分の復元は全prefix修復、公式証拠の変更による所属差は前節の一括再判定を使う。全成分既知の旧形式に対するbackup付きcanonical移行と、通常収集の部分prefix更新を混同しない。この規則は保存正本のNULL表現を必要とするものであり、必須成分を持つcurrent/旧wireを変更するものではない。各versionが表現できない部分値はWIREの規則に従い、canonical保存とhistoryの既知値を変更しない。

#### 外部producerの確認範囲と、状態を変更しないrecord

保存原本で確認した`cli_version=0.148.0-alpha.9`に対応する外部参照は、OpenAI Codexのcommit `44271b1a725e3265d80adde7886d10e48f08d8cc`とする。これはその版の実装を固定して参照する根拠であり、全版の保存JSONL安定保証ではない。RPCの`thread/tokenUsage/updated`と保存JSONLを同一schemaにしない。[保存wire enum](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/history/src/rollout_payload.rs)、[event/payloadの定義](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/protocol/src/protocol.rs)、[producer](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/core/src/session/mod.rs)を区別して確認する。

同producerの`record_initial_history`は、resume/forkで以前の`TokenUsageInfo`を復元し、paginatedなcopied forkでは継承recordを新しいlocal rolloutへ再掲しない経路を持つ。`history_base`がnullのcopied forkもある。`TokenUsageInfo::new_or_append`が状態なしから0を初期化する経路を持つことだけでは、ある保存fileがその経路だったとは証明できない。このため、byte 0の`session_meta`、`history_base`不在、初回のtotalとlastの一致だけでは、利用開始0・未計上量・model帰属の証拠としない。初回の有効rawは前節のbaseline規則を使い、実際の既知0 counterまたは照合済みの以前のanchor/prefixがある場合だけその位置以後のdeltaを確定する。lastの再加算で初回不足を埋めず、元の継承prefixを検証できる場合は同じ原本復元経路で扱う。

次の認識済みrecordは、改行済みJSON object、重要keyの非重複、public範囲のroot timestampと表のpayload型を満たす場合、token counter、model context、taskの開始/停止を変更しない。原本・byte順を保持し、前回anchorを消さず、counterを0へ戻さず、これらのrecordの存在だけでfully-attributedを否定しない。本文やopaque stateの内容からtoken/model/taskを抽出しない。型不正なら通常の未認識recordと同じく原本と前回有効anchorを保持し、当該sourceの完備性を確定しない。

ここで重要keyとはrootの`type,timestamp,payload`、payloadの`type`、および下表の型判定に使う各fieldである。その値を持つobject内の同名key重複を拒否し、last-winsを使わない。判定に使わない本文やopaque state内部のkeyを、model/tokenの入力へ昇格しない。下表のIDはsession IDと同じ1..512 Unicode scalar・control/bidiなしの境界を使い、大小文字・前後空白を変更せず照合する。

| 認識するrecord | payloadの必要条件 | 状態へ使わない意味 |
| --- | --- | --- |
| root `response_item` | rootの`payload`はobject。root timestampは本表共通のpublic範囲を必要とし、本文の内部fieldはopaque | model-visible itemの保存。本文をtoken/model/taskの観測へ解釈しない |
| root `world_state` | rootの`payload`はobject。その中の`full`はboolean、`state` keyが存在するJSON値 | environmentのbaseline/patch。`full=true`をtokenの0 baselineや利用量の完備性へ転用しない |
| root `inter_agent_communication_metadata` | rootの`payload`はobject。その中の`trigger_turn`はboolean | turnをtriggerするためのmetadata。trueだけでtask開始を確定しない |
| root `compacted` | rootの`payload`はobject、`message`は文字列。任意fieldの資格は次段落 | model-visible historyの圧縮。本文・置換history・window IDをtoken/model/taskの観測へ転用しない |
| root `event_msg` / payload `context_compacted` | payloadはobject、文字列`type="context_compacted"`を持つ。共通のenvelope・timestamp・重要keyの非重複・全JSON構文条件を満たし、本文fieldは要求しない | 会話履歴が圧縮された通知。raw baseline・model context・差分区間・task・context pair・最後のuser-message時刻を変更せず、追加fieldを数値/stateの根拠にしない |
| root `event_msg` / payload `thread_goal_updated` | payloadはobject、camelCaseの`threadId`と任意`turnId`、`goal`の資格は次段落 | 長期goalのmetadata。goalのstatus・tokensUsed・tokenBudgetをtask開始/停止、Session counter、quotaへ転用しない |
| root `event_msg` / payload `item_completed` | object、`thread_id,turn_id`は有効な非空ID文字列、`item`はobject。`started_at_ms`は欠落/nullまたは0..signed 64-bit最大値のJSON整数、`completed_at_ms`は欠落または同範囲のJSON整数（bool/小数不可） | 個別itemの終了。`task_complete`や全actionの停止と同一視しない。IDは本節の文字列安全境界に従い、本文itemはusageの正本にしない |
| root `event_msg` / payload `agent_message,user_message` | object、`message`は文字列 | 本文の表示。本文をmodel/context設定として実行しない |
| root `event_msg` / payload `agent_reasoning,agent_reasoning_raw_content` | object、`text`は文字列 | 本文の表示。token/model/taskを変更しない |
| root `event_msg` / payload `agent_reasoning_section_break` | object、`item_id`は欠落または文字列、`summary_index`は欠落またはsigned 64-bit整数 | 本文のsection境界。counterのlineage境界ではない |
| root `event_msg` / payload `mcp_tool_call_begin,mcp_tool_call_end,web_search_end,sub_agent_activity,patch_apply_end` | payloadはobject。共通のenvelope・timestamp・重要keyの非重複・全JSON構文条件を満たし、本文はopaque | MCP呼出しの開始/終了、検索呼出しの結果、対象の子agentの活動、patch適用結果の通知。`call_id,turn_id,invocation,result,duration,action,kind,agent_path,stdout,stderr,success,changes,status`や内部のmodel/usage/task風の値を、当該Sessionの数値・model・task状態・account・response資格へ使わず、個別callのendや子agentの活動を当該taskの開始/終了にしない |

`context_compacted`は、実ログのfieldなし通知と、`rust-v0.159.2`の固定commit `ff6aec96948b70d94983af2641a6b67c94faeff5`の[EventMsg / ContextCompactedEvent定義](https://github.com/openai/codex/blob/ff6aec96948b70d94983af2641a6b67c94faeff5/codex-rs/protocol/src/protocol.rs#L2150)を根拠に認識する。root `compacted`のmessage・置換historyとは別の通知であり、usage/control一般の未知typeを許可する規則ではない。

上記5種類の呼出し・活動recordは、保存実測の`cli_version=0.162.0-alpha.17.2`の形と、固定commit `740e5af33c71225640e0c1c1555c514c2c93ab74`の[protocol定義](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/protocol/src/protocol.rs)にある`EventMsg`、`McpToolCallBeginEvent`、`McpToolCallEndEvent`、`WebSearchEndEvent`、`SubAgentActivityEvent`、`PatchApplyEndEvent`の責務を対応付けた本adapterの入力契約とする。固定sourceが当該alpha buildに完全一致するとは表明せず、`cli_version`文字列の一致を認識の前提にしない。未使用の本文fieldの型を記録・起動の新たな前提にせず、本文からmodel/usage/taskを再帰抽出しない。上表にない未知typeは従来の未知として保持し、一括許可しない。

上表のroot recordでも、rootの同名fieldを`payload`内の必須fieldの代用にしない。`full,state,trigger_turn`の重複key検査も実際に採用するpayload objectで行う。`compacted`では、`replacement_history`は欠落/nullまたはobjectだけを要素とする配列、`replacement_history_metadata`は欠落/nullまたは同じ長さのobject配列とし、後者が非nullなら前者の配列を必要とする。`window_number`は欠落/nullまたはUInt64整数、`first_window_id,previous_window_id`は欠落/nullまたは本節の有効ID、`window_id`は欠落/null・有効ID・旧形式のUInt64整数を許す。bool/小数を整数へ変換しない。これらのfieldは型だけを判定し、bodyの再生や継承modelの復元をしない。型を判定するpayload keyの重複は拒否する。認識済みの正常な圧縮recordだけではraw baseline・model context・差分区間・task・context pairを変更せず、後続の正式なcounter/context recordを通常規則で処理する。型不正は上記NoOp不正の未処理・不完備規則に従う。

`thread_goal_updated`の`threadId`は有効ID、`turnId`は欠落/nullまたは有効IDとする。`goal`はobjectで、`threadId`がpayloadのIDと完全一致し、`objective`は1..4,000 Unicode scalarの文字列、`status`は`active|paused|blocked|usageLimited|budgetLimited|complete`、`tokensUsed,timeUsedSeconds,createdAt,updatedAt`はsigned 64-bit JSON整数、`tokenBudget`は欠落/nullまたは同じ整数型を必要とする。bool/小数を整数へ変換しない。型判定に使うpayload/goal keyの重複を拒否し、objective本文はopaqueとして保存原本外へ抽出しない。これらは[固定commitのEvent定義](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/protocol/src/protocol.rs)に基づくmetadataの認識条件であり、本製品のtoken数値契約ではない。

圧縮recordの意味は、同commitの[保存wire](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/history/src/rollout_payload.rs)と[replace_compacted_history](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/core/src/session/mod.rs)に結び付ける。後者は圧縮済みhistory、world state、必要なturn contextを別recordで保存する。`token_usage_record`と`compacted.latest_token_usage_record`は、別に保存原本で確認した`cli_version=0.162.0-alpha.17.2`のcommit `740e5af33c71225640e0c1c1555c514c2c93ab74`へ固定して下節で定義する。0.148.0-alpha.9の同じfieldだとは表明しない。圧縮snapshotそのものを新規利用として数えず、二つのusage streamを加算しない。下表のcompactedが数値baseline・model・taskを変えない規則は維持し、usage streamの選択とsnapshotの認識だけは下節に従う。

#### 完了responseのusage recordと累積counterの重複排除

この既知形の根拠はcommit `740e5af33c71225640e0c1c1555c514c2c93ab74`の[TokenUsageRecord/TokenUsage](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/protocol/src/protocol.rs#L2230-L2262)、[response完了の二重記録経路](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/session/turn.rs#L2954-L2992)、[recordの累積処理](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/state/session.rs#L217-L253)である。同じresponse usageからroot recordとlegacy `token_count`を生成するため、独立した二つの消費ではない。`token_count`側にはresponse IDがなく、全event列の一対一対応を外部保証とはしない。下記の独立した数値・model・periodの資格を本製品の採用条件とする。版文字列だけで形を推測せず、この構造と資格を全て検証する。古いsession_metaの後へ新しいproducerがappendできるため、先頭のcli_versionだけで後続のこの既知形を拒否しない。

| 入力 | 必要条件と用途 |
| --- | --- |
| root `type=token_usage_record` | 改行済みobject、有効なroot timestamp、objectのpayload。rootのtype/timestamp/payloadおよび以下の判定keyは重複不可 |
| payloadの`thread_id,turn_id,session_id,root_turn_id,response_id` | 全て本節の有効ID。thread_idは検証済みsession_meta.idと完全一致。session_idはroot threadのIDでありthread_idと同一とは限らない。response streamではsession_meta.session_idも非nullの有効IDを必須とし、完全一致を要する。欠落をrecordの値で補わない |
| `usage,turn_token_usage,thread_token_usage` | 全てobject。各々`total_tokens,input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,reasoning_output_tokens`の6整数が必須。bool/小数/null不可、各値0..signed 64-bit最大値、T=I+O、C+W<=I、reasoning_output_tokens<=Oをchecked演算で検証。関係keyの重複不可 |
| `usage` | 一つの完了responseの消費。資格を得た場合だけこの5成分T/I/C/W/Oを一度保存し、reasoningはoutputの内数として別加算しない |
| `turn_token_usage,thread_token_usage` | producer内の累積snapshot。usageへの加算、legacy counterの置換、初回0の証明には使わない |
| `compacted.payload.latest_token_usage_record` | 欠落/nullまたは上記payload形のobject。非nullなら同じID/算術資格を検証。resume用snapshotであり、何度現れても消費event・quota・raw baseline・model contextを作らない。不正は未処理・不完備を保持 |

上記session scopeの必須性は、[0.148.0-alpha.9のSessionMeta](https://github.com/openai/codex/blob/44271b1a725e3265d80adde7886d10e48f08d8cc/codex-rs/protocol/src/protocol.rs#L2859-L2861)と[0.162.0-alpha.17.2のSessionMeta](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/protocol/src/protocol.rs#L3112-L3120)の両方で非optionalのSessionIdであることに基づく。legacy-counter用の最小metadata入力にidがあることだけでは、response streamのscope資格を代用しない。既知response recordを見てstream kindが切り替わっても、このmetadata資格が不足するresponseは原本と未処理情報へ保持し、旧anchor/旧有効値を消さず、legacy差分へfallbackしない。

source lineageは数値stream kindを`legacy-counter`または`response-record`として保持する。検証済み先頭からこのrootも非nullのcompacted snapshotも一度も現れていない区間だけをlegacy-counterとする。最初のroot token_usage_record、または非null latest_token_usage_recordの位置でresponse-recordへ切り替え、そのrecordの型不正を理由にlegacy-counterへfallbackしない。切替より前に確定済みのlegacy deltaは保持する。切替後はtoken_countの累積差分を新規消費へ一切加算せず、responseのusageだけを数値候補とする。これにより初回responseを累積baselineへ昇格せず復元でき、同じusageをlegacy差分と二重計上しない。token_countのquota、context pair、rawの比較状態はそれぞれ従来の独立した資格で保持し、raw counter異常をresponseのusageで隠したり消去しない。response候補のないtoken_countが正の通常deltaを示した場合は、対応responseの不足としてその区間を未処理にし、legacy差分を代わりに加算しない。認識済みcontext調整、差0、info=nullだけではresponse不足を新たに作らない。source/Sessionの継続を証明する通常append、period切替、batch分割、再起動ではstream kindを戻さない。別sourceで同じSessionを継続する場合も旧kindを保持し、独立した新Sessionだけは先頭から判定する。

旧counter専用の「差分なし」証明も、このstream境界を跨いではならない。root responseのT=0や、追加消費ではない圧縮snapshotだけの場合でも、response markerがあればlegacy counter比較だけの証明を成立させず、通常のsource採用でresponse stateを復元する。この証明を拒否することは、snapshotを消費として数えることではない。未知・不正control、未完のJSON、改行済みの不正JSONを含む範囲を、旧counterの同値だけで確認済みへ昇格させない。task backfillも不明なcontrol区間を飛ばして完全なtask coverageを作らず、同じ認識済み入力の採否を使う。

responseの数値・model・periodの資格は次の順で独立に判定する。対応token_countの存在やlast一致を、完了responseの消費量そのものの必須条件にしない。

1. 有効なroot recordのusageを、root原timestamp、source byte位置/hash、5 IDとともに一つの消費候補として保持する。原時刻が属する一つの連続account所有区間と、session_metaとのscope一致を必要とする。reasoningはOの内数であり別加算しない。T=0では消費eventを増やさず、照合とcoverageを保持する。初回responseも同じ条件で数値資格を得られるが、初回以前の利用を0やcompleteにしない。turn/thread累積snapshotは使用しない。
2. root位置の同じsource/Sessionで有効なkind=turnの強contextを、元pointer・model・turn_id/root_turn_idとともに候補として保持する。response用の候補は両IDが有効かつ既知の場合だけであり、弱い設定や最後のmodel名を候補へ昇格しない。通常responseのmodelを確定するには、その元contextの両IDとrootの両IDの完全一致を必要とする。これはrequestに指定されたmodel IDであり、provider/serverが実際に解決したmodelの保証ではない。rootのusage値からmodelを補わない。通常responseはrootに続く最初のtoken_countまたは次の明示的なturn/task境界までを分類区間とし、その間の全usage/control recordを認識でき、圧縮snapshotが現れなければroot位置の資格を確定する。別rootが先に来た場合もそこで区間を閉じ、それまでの有効contextと認識済みrecordから判定する。未知recordまたは不正contextがあればmodel未帰属のままにする。batch末尾なら区間の資格と原本を保存して次の通常appendを待ち、任意の将来contextを探索しない。
3. 上の分類区間に、同じ5 IDと3組の6成分が完全一致するcompacted.latest_token_usage_recordがあれば圧縮responseとする。snapshotは消費へ加算しない。一致しない非null snapshotは競合であり、それを飛ばして通常responseとして扱わない。圧縮requestのmodel候補は、root位置で保持した直前の有効な強contextのmodelと、同じ圧縮処理に続くreplacement turn_contextのmodelである。前者は前turnのcontextでもよく、その両IDがrootと一致することを圧縮の条件にはしない。source/Session/lineageが同じで、両IDが有効な元contextであることは必要とする。replacementのturn_id/root_turn_idがrootと一致し、両contextが有効、両model IDが同一の場合だけそのIDを採用する。両者が異なる、片側が不明、途中に別root・task/history/source境界・不正/未知usage/controlがある場合はmodel未帰属とする。通常responseの同turn条件をこの前turnの証拠へ適用せず、圧縮の限定資格を通常responseへ広げない。compaction_model_hashだけからexact IDを逆算しない。後続設定だけで圧縮前の消費モデルを決めない。
4. この限定は外部producerの実経路による。[previous-model compaction](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/session/turn.rs#L1354-L1435)はprevious modelをrequestへ使い、[V2 fallback](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/compact_remote_v2.rs#L230-L285)はreplacement側modelでも再実行し得る。保存するreplacement contextはrequest modelの記録ではない。通常圧縮とV2のCompletedはroot usageを作るが、[TokenBudget compaction](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/compact_token_budget.rs#L27-L70)はmodel request自体をしない。外部形式にexact request modelの証拠がない場合は、その不足を原本の制約として保持する。
5. periodは前節の公式観測履歴にroot原timestampを対応付けて決める。同じresponseに続くtoken_countのcached quotaや時刻を期間の正本にしない。modelとperiodが両方資格を得たときだけ、root原timestampをcanonical観測時刻とし、root pointerを数値の主pointerとする一つのeventを公開集計へ入れる。model未帰属・period未所属の数値は、既知usageと理由を保存した未処理情報として保持する。後続の公式観測でperiodだけが確定した場合も、model不足を補わない。
6. response重複keyは同partition内の順序付き5 ID `(thread_id,session_id,root_turn_id,turn_id,response_id)`とする。IDを正規化せず、各IDのUTF-8 byte長をunsigned 64-bit big-endianで前置して連結した列のSHA-256小文字hexを保存keyとする。source/generation/epochが違うだけで別利用にしない。同じkeyのroot原timestamp・usage全6成分・session資格が一致する場合だけ同一の数値候補であり、未帰属から資格を得たmodel/periodの追加は同候補の状態遷移にする。既知model同士、原時刻または数値が異なれば競合として既存有効値と双方の原本を保持し、新しい値で上書きしない。同じ数値eventのperiod資格だけが公式証拠の追加で変わる場合は、前節の所属再判定とし、response重複や数値競合へ変換しない。異なる原本側が異なるperiodを主張することだけでは再判定の根拠にならず、公式証拠集合の変更を必要とする。価格も同じ数値/価格根拠の再適用では再計算しない。別partitionへコピーして二重計上せず、forkの継承snapshotを新規responseにしない。
7. legacy counterとの照合は旧派生eventの同一性確認にだけ使う。rootに続く最初のtoken_countまでに、別root、資格を変えるcontext、task/history/source境界、未知usage/controlがなく、last全6成分がroot usageと一致する場合は対応counter pointerを保持できる。正常NoOpと同model/同turn IDの再確認は挟める。一致する圧縮snapshotがある場合はその完全一致を使え、圧縮後lastのcontext推定値を消費へ加算しない。対応がなくてもrootの数値資格は失わず、さらに後の似た数値を探索しない。旧eventがcounter原時刻で保存されている場合は、この照合で同一の消費を証明したうえ、PRODUCTの全prefix修復transactionで旧派生eventを退役し、root原時刻のeventへ一度だけ置換する。元key/pointer・旧値はverified backupと修復provenanceに保持する。schema-only移行や通常replayで旧timestampを書き換えない。
8. stream kind、分類中のroot候補、全ID/usage/root pointer、分類区間のcontext・snapshot資格、既知model/periodとその根拠、未処理理由をevent/checkpointと同じtransactionで永続化する。複数候補を最後の一件だけで上書きせず、保存前にcursorだけ進めない。再起動・batch分割・同じaccepted prefixの再生で数値event集合と未処理理由が変わってはならない。v14/v15からのschema-only移行は新資格をNULLで追加し旧数値を一切変えない。旧NULLからstream kind・model・periodを推測せず、全prefix再検証で資格を得るまで既存値を保持し、新しい消費量を加算しない。

分類の境界はsourceのbyte順で決め、次の表を適用する。これはroot位置のrequest modelを認定する規則であり、後続recordの数値や時刻をrootのusage・timestampへ代入する規則ではない。

| 分類中に続くrecord | そのroot候補への判定 |
| --- | --- |
| 正常なNoOp、正常な初期設定、snapshot未取得の状態でroot位置に保持した元contextと同じmodelかつ同じturn_id/root_turn_idを持つ正常な強contextの再確認 | 分類を継続する。同じ元contextの再確認だけを新しいturn境界にしない。初期設定の採用/無視は現在contextの優先規則へ従い、root位置で欠けていた強context資格を後の初期設定・新contextから補わない |
| rootと完全一致する非null圧縮snapshot | 通常responseとしての確定を止め、同じ圧縮処理のreplacement `turn_context`を待つ。同じsnapshotの再掲は追加消費・追加baselineにしない |
| snapshot待機後の最初の強context | この位置で分類を閉じる。`turn_context`で、rootと両IDが一致し、root位置とreplacementの両modelが同一の場合だけ圧縮modelを確定する。`thread_context`だけ、model/IDの相違・不明、不正なcontextでは未帰属とし、さらに先のcontextで救済しない |
| snapshotがない状態で、元contextからmodelまたはturn_id/root_turn_idが変わる正常な強context、次の明示task境界、次のroot response | その位置の直前で通常responseの分類を閉じる。root位置に有効な元contextがなければ、後着の強contextもこの境界とする。通常modelの同turn資格は元contextだけで判定し、新しいcontextを以前のrootへ遡って適用しない |
| snapshot待機中の別root、task境界 | replacementの資格不足として未帰属にする。別処理のcontextを探索しない |
| history/source境界 | snapshotの有無によらず、その位置で分類中のrootを未帰属にする。history境界は非null `session_meta.payload.history_base`による継続の標識、source境界はidentity/generation/Session/prefixの継続を証明できない切替である。有効な同scope metadataの再掲だけでは境界を生成しない。現在のEOF・batch切替・通常appendはsource境界ではない |
| 最初の一意に認識できるtoken_count | 分類を閉じる。snapshotなしなら通常response、snapshotありで有効replacement未取得ならmodel未帰属。counterの数値・sample・timestamp・context pairの不正だけでは、独立したroot位置のmodel資格を失わせない。不正counter自身の未処理・完備性欠如は保持する |
| 未知record、不正NoOp、不正context、root/payloadのevent名を一意に認識できないrecord | 当該rootのmodelを未帰属とし、元の数値・根拠不足を保持する。以後の正常contextでそのrootを遡って救済しない |
| batch末尾・現在のEOF | 分類中の全候補と資格を保存する。時間経過、収集cycleの終了、RPC task状態だけで通常/圧縮を確定しない |

ここで一意なevent名は、認識するroot位置、または`event_msg`のobject payloadにある文字列typeから決める。判定に使うtype keyの重複・既知名同士の矛盾は一意とは扱わない。token_countとtaskの数値・timestamp不正を、event名自体の不明と混同しない。数値の採否、現在modelの優先規則、task区間の不明化には、それぞれ前節の独立した規則を適用する。session metadataのscope資格がないrootは、そのsourceの強contextを当該responseのmodel証明へ流用しない。

分類名の入力は、重複のない文字列root.typeを必須とする。root=`event_msg`では、payload object内の重複のない文字列typeが8.7で認識するevent名である場合だけその名を使う。直接rootの互換8 eventでは、payloadにtype keyがなければroot名を使え、payload objectにtypeが提供されている場合は、重複なしの同じ文字列であることを要する。この判定keyのnull/別型/相違は分類名不明である。payload自体の欠落・別型と、info/数値/時刻/pairの不正はroot名を別名へ変えず、そのrecordの専用採否で扱う。rootのresponse/snapshot/metadata/正常NoOpは各専用表の認識位置と型を使い、未知rootをpayload.typeから昇格しない。これはresponse分類の入力規則であり、legacy counterや現在modelの独立採否を別規則へ置き換えない。

compactedのsnapshotが欠落/nullで、他のNoOp入力条件が有効な場合はNoOpとして分類を継続する。snapshotの一致証明を生成せず、後のnormal境界では表どおりroot位置の資格だけを使う。不正metadata、同source内のID/scope不一致は正常NoOpへ流さず、分類中のrootを未帰属にする。再起動前の分類区間を保存していない旧stateは、root位置のmodel文字列だけからopen/normal/compactionのいずれにも補完しない。旧root候補の数値と原本を保持し、同じprefixの再解析で区間資格を復元するまでmodel未資格とする。以後に新しく読む別rootの独立した分類は継続できる。

source_admissionの保存形式v2は、rootごとの分類待機・snapshot待機・normal/compactionの確定・未帰属と、その判定に使った元byte範囲/hashをcheckpointと一緒に保持する。v1は旧分類不明として読めるが、後着counter/contextから旧rootを確定しない。旧候補を保持したまま、新しく読むrootにv2の分類資格を作れる。分類の境界recordがcounter/taskとして一意でも、その元timestampが不正な場合は、判定証拠にbyte範囲/hashだけを保存する。rootの時刻や受信時刻を境界recordの時刻へ代入しない。source境界に新recordが存在しない場合は、その既存sourceの検証済み終端位置を保持し、架空のrecordを作らない。これらは本製品の内部保存版であり、Codexのprotocol版ではない。

`source_admission` v1/v2内のrecord pointerと分類用pointerの`sha256`は、同じsourceのhalf-open元byte範囲`[start,end)`に対する一回の`lowerhex(SHA256(bytes))`である。範囲は一つの改行済みrecord全体を終端LFごと含み、CRLFならCRも含む。JSON再serialize、改行・空白・Unicodeの正規化、hex文字列の再hashをしない。root、context、snapshot、counter、taskのpointerは全てこの定義を使う。これは下段の既存`session_ranges`等のrange fingerprint v1における`record_sha256`とは異なる版付きfieldであり、両者を同じ方式として検証・変換しない。pointerだけでaccount/source資格を作らず、親checkpointの完全なsource identityとaccepted prefixへ結合して検証する。

保存するroot位置のcontext候補と、確定したrequest modelは同一ではない。前turnの有効contextを保持しただけでは通常responseの同turn資格を満たさない。例えばAstraの元contextがturn=P、rootがturn=Nで、両方のroot_turn_idも異なる場合、完全一致する圧縮snapshotと、rootの両IDに一致する最初のreplacement turn_context=Astraがあれば圧縮modelをAstraとする。同じ列にsnapshotがなくcounterで分類を閉じた場合は通常model未資格、replacementがLunaなら圧縮model未資格である。元contextの両ID不明・不正、model不明を後続replacementだけで救済しない。旧保存stateに元context候補がなければ、同じprefixの再検証まで未資格を保持する。

数値候補の存在、model帰属、period所属、source全体のcoverage、現在の取得成否を別に保持する。不正・未帰属・未所属・競合を0や最新modelで補わず、有効な既知usageをpointerだけに縮退させない。後から資格が成立した候補は一度だけcanonical eventへ移し、依存coverageと未処理理由を同じtransactionで更新する。response recordが存在するだけでは、未受理suffix、欠測prefix、別accountの利用、snapshotにしかない利用を回復済みにしない。原本復元はPRODUCTの全prefix検証と排他transactionに従い、別の数値正本を作らない。

Session identityはsourceのcanonical path/device/inode/prefix generationと上記IDの組で検証する。同じsource内の複数session_metaは同じIDだけを許す。新しいlive sourceとaccountを初めて結ぶ際には、`thread/read`応答の`thread.id`とcanonical rollout path、process identity前後値の一致が必要で、同じIDが同時に異なるactive pathへ対応するcycleを拒否する。RPC応答全体のrequest IDをthread IDへ代用しない。過去sourceは、同じIDとsource range/hashを結ぶ受理済みcheckpoint/eventおよびaccount所有区間を検証する。metadata IDだけ、ファイル名だけ、mtimeだけでaccount所有を決めない。欠落・不一致は当該sourceの採用を保留し、原本と前回の有効stateを保持する。rotationでpathが変わったsourceは8.7.2の照合を通し、別の新規利用として二重加算しない。

liveのprocess identityは、当該`account/read`・`thread/read`を同じstdio接続で実行したローカルapp-server子processのPID、OSのprocess starttime、実行fileのdevice/inodeの組である。記録fileを書いた元processのIDがRPCで提供される、またはこの組だけでfile/account所有を証明できるという意味ではない。子processを終了させる前に、RPC群の前後で同じ組を検証する。同じRPC群で認証主体・account epochも前後一致させ、session_meta.idと`thread/read`の`thread.id`、返されたcanonical pathと固定したsourceのpath/device/inode/prefixを照合する。process・path・accountのいずれも取得不能なら初回結合を新設せず、原本と前回の有効stateを保持する。このRPC結果はその取得群と固定prefixの証拠であり、別子processの同一性や後続appendの内容を証明しない。

このRPC照合は、現在の認証主体とprofile-localなSessionの可読性・identityを別々に確認する必要条件であり、過去の消費accountをRPCだけで証明するものではない。確認済み0.159.2の`ThreadReadResponse.Thread`はaccount owner fieldを提供せず、`thread/read`成功を過去のprovider課金先の証明とする外部保証は採用していない。公式[Threads仕様](https://learn.chatgpt.com/docs/app-server#threads)の保存metadata読取りと、固定producer `740e5af33c71225640e0c1c1555c514c2c93ab74`の[thread reader呼出し](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/app-server/src/request_processors/thread_processor.rs#L2824-L2932)・[local reader](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/thread-store/src/local/read_thread.rs#L31-L133)を、その保証へ拡張しない。

本製品のaccount別ローカル観測への所属は、元のevent時刻とregistryの記録済み連続所有区間、同じpartition/source/Sessionに結合したaccepted prefixの資格を独立に満たす場合だけ判定する。現在Aで過去threadを読めたことから、そのthreadの旧prefix全体をAへ割り当てない。元時刻・所有区間・旧source結合が不足する場合はaccount所属を未確定として原本・既知の数量/model事実・旧記録を保持し、新しいaccount別合計へ加算・complete化しない。currentの認証確認を元のevent時刻へ遡及させず、過去所属を回復できるのは保存済み根拠または資格付き原本修復による場合だけとする。これはローカルで記録した利用の所属規則であり、他端末を含むprovider account全体のtoken利用量や課金先の保証ではない。

初回照合で受理したsource/account結合は、partition identity、source identity、session metadata、受理済みprefixとその照合根拠をcheckpointと同じtransactionへ保持する。以後のlocal appendは次の全条件で継続を判定し、各appendへの新しいRPC成功を必要条件にしない。

この結合資格v1は、完全なpartition tuple、root/relative path/device/inode/prefix generation、照合したSession metadata、照合済みの改行済みprefix終端N、および次のいずれか一つの取得根拠を保持する。prefixのbyte証拠は同じsource/generationの保存済みaccepted range/hashへ結び、別の数値正本や原本の複製を作らない。

| 取得根拠 | 新設時の条件と時間の意味 |
| --- | --- |
| 初回RPC | 同じ子processのaccount/read → thread/read → account/read、前後raw主体・現admission・process identity・source再確認が全て成立した群の開始/終了UTC秒と子process identityを保持する。開始Tより前のrecordは、この現在確認だけではaccount別消費へ新規加算しない。先頭からの解析事実は保持できるが、その過去数値には別途、保存済みの過去所属資格を必要とする |
| 既存accepted prefix | 当該partitionに既に保存されていたcheckpoint/eventの元source/generation、先頭からNまでの全accepted range/hash、同じprefixから復元した一意なSession metadata、元の数値・raw anchorの一致を検証する。さらに、根拠とする元数値recordのpointer/時刻が、有限開始を持つ一つの記録済み所有区間に属することを必要とする。その元record pointerを保持し、現在のRPC確認時刻や子processを過去の根拠へ代用しない |

初回RPCの前端Tは、その群で新たに結合したsourceの数値資格の下限でもある。legacy差分では前後のraw recordがともにT以後かつ同じ一つの記録済み所有区間にあることを必要とする。下限より前・所有外・元時刻不明のrawは、その後の最初の適格rawを追加利用0のbaselineとする規則に従う。responseの元時刻も同じ下限と所有区間で判定し、数量が既知の未所属候補を0や消失へ変えない。これは初回群からの所属を定める規則であり、保存済みaccepted prefixの元資格を再起動時刻まで切り詰める規則ではない。

資格の欠落/nullは旧未確認状態であり、未知版・不正shape・partition/source/Session不一致の提供済み資格とは区別する。後者を資格なしへ降格して初回扱いで上書きしない。新しい資格は解析資格が検証済みのcheckpointと同じusage transactionで保存し、保存失敗で資格だけを進めない。再開時は現在のDB tuple、source/generation、Session、Nを含むaccepted prefixを再検証し、保存した子process identityを現在のprocessの証明に使わない。異なるgeneration・rotation先へ資格をコピーせず、既存のsource置換・anchor照合条件を適用する。

結合時に未提供だったoptional `session_id`が、同じ一意なsession_meta.idと検証済みprefixの通常解析により後から判明した場合、元の未観測という結合記録を保持したまま継続できる。保存済み結合の既知idは全て一致を必要とし、既知session_idの消失・別値への変更を未観測として通さない。元の結合metadataを後着値で書き換えたり、Sessionの数値資格を後着値だけで過去へ追加したりしない。

| 条件 | 継続の判定・保持 |
| --- | --- |
| 同じpartition・同じ物理source・同じSessionであり、以前のaccepted prefixの全range/hashを照合でき、固定した新suffixの解析時bytesと再確認hashが一致する | 保存済みの結合から同じlineageの読取りを継続できる。以前のRPC成功だけを新suffixのbyte証拠へ流用しない |
| 現cycleのraw AccountKeyが前後で一致し、当該partitionに対応し、現admission tupleも一致する | そのcycleの新candidateだけを受理可能とする。再起動後もpartition/source/prefixの継続を再検証し、旧processのauth_epochや子process identityを現processへ移さない |
| 各数値候補が一つの連続account所有区間に含まれる | 8.7の両端・response原時刻の規則で所属を判定する。A→B→Aの保存済みA結合はB区間や境界を跨ぐdeltaを採用する根拠にならない |
| RPCがtimeout・切断・不正応答、または前後のaccount/readだけが不一致だが、上のlocal条件は全て成立する | quota/thread RPC候補を更新せず失敗を保持し、資格のあるlocal Session記録は継続する。raw AccountKey不一致と同一視しない |
| source/Session/partitionの結合が未取得、不一致、または以前のprefixを検証できない | 新しい結合を推測せず、そのsource候補を保留する。source置換は8.7.2、過去原本の復元はHISTORY-RECOVERY-614の資格でのみ回復する |

sourceの継続資格、各eventのmodel/period資格、source全体の完備性は別である。RPC停止中に検証済み数値を保存できても、公式periodの片側観測しかなく所属が決まらなければ、未所属候補として保持し後続の公式証拠で再判定する。現在quotaやthread状態をlocal数値から生成しない。

この検証のため、収集器は未commitの固定source候補を解析・照合できるが、候補cursorをcommit済みcheckpointまたは公開Thread snapshotとして扱ってはならない。まだcheckpointのない新sourceも同じ候補資格で`thread/read`照合を行い、既存checkpointの存在をRPC照合の前提にして循環依存を作らない。初回はRPC照合、その後は上表の結合継続と、いずれもsource再確認・account fenceが成立した候補だけを、数値資格と未処理情報を含む一つのusage transactionへ渡す。通常のUI用active-thread pollの成功をcollectorへ未接続のまま代用せず、逆にcollectorの未commit候補をUIへ公開しない。過去sourceは前段のaccepted prefixによる資格を使い、現在のRPCへ過去のprocessやmodelを証明させない。source単独の不成立はその候補を取り消し、独立した有効sourceと公式quota laneは維持する。raw AccountKey・現admission tupleの不一致、または同じthread IDへの異なるactive pathの衝突はcycle全体のusage候補を取り消し、旧commit済みstateを保持する。account/read応答だけの不一致には8.6と上表のlane別保持を適用する。

model IDの正規化は前後のUnicode White_Space削除だけとし、内部文字・大小文字・Unicode正規化を変えない。まず提供された文字列全体に、Unicode scalarでないsurrogate、control（U+0000..001F / U+007F..009F）、bidi control（U+061C / U+200E..200F / U+202A..202E / U+2066..2069）があれば拒否する。前後にあるcontrolもtrimで救済しない。その後、White_Space（U+0009..000D / U+0020 / U+0085 / U+00A0 / U+1680 / U+2000..200A / U+2028..2029 / U+202F / U+205F / U+3000）だけを両端から除き、残りが空または128 Unicode scalar超過なら拒否する。host言語の独自なstrip対象を追加せず、byte数やUTF-16 code unit数をscalar数へ代用しない。正規化後の128 scalarが占めるUTF-8の最大512 byteはbufferの導出値であり、前後White_Spaceを含む原文の長さ上限ではない。原文をstreamで検査し、許可済みの長い前後White_Spaceによって有効な正規化結果を拒否・切詰めしない。session/NoOpのIDにも上記control/bidi/surrogateの拒否を適用し、そちらはtrimなしの1..512 scalarとする。元byteは原本に残す。既知familyや最新modelへ置換せず、未帰属のtokenは内部区分`UNATTRIBUTED`として原本・未処理情報に保存し、公式model IDとして数値公開しない。後のcontextを過去へ遡って割り当てない。価格catalogにない正当なIDもtokenは保持する。

既存旧family eventを保持する原本復元照合だけは、PRODUCTの`HISTORY-RECOVERY-614`と`MODEL-PRICING-463`の限定互換対応に従う。これは元keyを保持するための照合であり、新規記録のmodel ID正規化、exact-ID同士の比較、価格版の推定には適用しない。

既存のsource range fingerprint v1の`record_sha256`は、half-open byte範囲`[range_start,range_end)`の正確なbytesをBとして、`lowerhex(SHA256(SHA256(B)))`である。内側の32 raw digest bytesを外側へ渡し、hex文字列を再hashしない。改行・UTF-8 bytesを正規化しない。この値は既存のevent/range/window pointerを検証する互換契約であり、schema v16でも一回のSHA256へ黙って再解釈・書換えしない。空範囲のcheckpoint sentinelだけで観測の所有を証明しない。将来別方式を採用する場合は明示的な版と移行契約を先に定める。

解析値へ結合するfingerprintは、parserが実際に消費した同じbyte列から得る。解析後にfileを別読取りして得たhashだけを、解析済み値の根拠にしない。commit候補では、その解析時hashと、同じ開いたsource identityの同じ採用範囲を再確認したhashが一致することも必要とする。長さが同じでも内容が変化すれば当該sourceの未commit数値・range・checkpointを一緒に取り消し、前回commitと他sourceは保持する。未改行tailは採用範囲のhashへ含めず、その前までのLF済みprefixを照合する。隔離した改行済み不正recordのbytesは採用prefixの原本証拠へ含めるが、そのrecordを有効な消費へ変換しない。

保存identityと物理recordの重複資格を区別する。保存keyの`prefix_generation`は省略せず、異なるgenerationのcheckpoint/markerを同じものとして上書きしない。一方、PRODUCTの重複prefix修復における「同じ物理証拠」は、同じroot/path/device/inode、同じ改行済みbyte区間と正確なbytes、同じsession ID・時刻・model帰属・lossless 5成分を必要とする。さらに両generationの受理済みrangeが当該record末尾までの先頭prefixを穴なく覆い、各保存hashを同じ原本bytesで検証できることを必要とする。単一行のhash/offset一致だけではtruncate後の再利用や別lineageを排除できず、generation越しの重複としない。

この全資格が成立した同じ物理recordだけは一度parse・一度加算し、元の全保存pointerをその一つの証拠へ対応付ける。資格のないgeneration間でanchor/model stateを継承せず、曖昧なeventを重複として削除しない。既存の等価eventを一つへ退役させる場合の代表pointerは、`(root identity,relative path,device,inode,prefix generation,range start,range end,range fingerprint,event index)`をこの順に、文字列はUTF-8 unsigned byte辞書順、数値は数値順で比較した最小keyとする。全候補の数値・意味の一致が前提であり、この順序はイベント時刻・provider取得順の推定ではない。原本とverified backupへ旧pointerを保持し、再実行で代表が変わらないようにする。

本書とPRODUCTの「5成分一致」は、各成分の既知/不明も含むlossless tupleの一致である。既知値は整数完全一致、不明同士は不明のままとし、不明と0を等値にしない。両側で不明の成分は復元後も不明であり、価格や完全性を証明しない。既知totalと検証済みsource prefixから復元できるtokenまで、不明なcache-writeだけを理由に0や全件取得不能へ変えない。

数値eventの必須Tが欠落している場合は上記入力契約の通り採用しない。I/C/Oが揃っていてもTを逆算してevent/model rowを作らず、原本・前回有効anchorを保持し、当該未処理sourceの完備性を確定しない。以下の「I/C/O既知・W不明」の保存例は、必須Tが既知で算術検証を通っている入力を意味する。Tを欠いた数値rowという別形式を認める規則ではない。

保存表現もこの既知性を保持する。canonical schema v16では既存の`session_model_totals`と`usage_model_history`のtokenをcanonical ASCII十進TEXT（`0`または先頭0のない正整数、UInt64範囲）とし、`total_tokens`は必須、`input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens`は未知の場合だけSQL NULLを許す。`session_events`と`session_checkpoints`の同じtoken成分もこのknownnessを保持する制約・型とし、数値event/前回counter観測のない状態を架空のT=0へ変換しない。既知0はTEXT `0`、未知はNULLであり、空文字・負数sentinel・浮動小数への変換を使わない。これはSQLiteのsigned INTEGER幅と公開UInt64を混同しないための保存境界であり、wireは引き続きJSON整数を返す。schema v14/v15からは検証済みbackup後のtransaction migrationでnullable制約を緩め、既存の数値文字列・NULL・履歴・event・checkpoint・金額・価格版を変更しない。新しい並列のモデル正本を追加しない。source event/checkpointも同じlossless knownnessを保持する。current必須成分が不明な行はWIRE規則で省略するが、historyの既知totalまで失わない。

同じv16 transactionで、`usage_history`の固定互換6列（`sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens`）もNULLを許す。既存のREAL/INTEGER型・非NULL保存値・binary64 bit値を変更しない。新規の固定family金額はPRODUCTの凍結互換式に必要な成分が全て既知の場合だけ保存し、不明ならNULLとする。新規の固定family tokenは既知memberのtotalを整数で正確に合算し、同じINTEGER列に非負signed 64-bitで保存できる場合だけ値を保存する。表現域を超える合算はNULLとし、REALへの昇格・clamp・桁落ちを行わない。これは旧固定列での表現不能であり、個々の正確なmodel keyのUInt64 totalをsidecarから消す根拠ではない。v16の不明を許すtoken・固定family列では、値の省略時もNULLとし、DEFAULT 0で未観測値を生成しない。移行前の既存0は変更せず、新規行の既知0と不明の区別を維持する。未知のmemberを0として足さず、集合の完備性は独立して保持する。新しく未観測である値を0で埋めず、明示的に確認した0だけを0として保存する。familyのmemberが0件であるときは、集合が完備で当該familyの利用がないことを確認した場合だけ固定値0とし、未完備の集合に当該familyが掲載されない場合はNULLとする。固定列のNULLは当該metricの互換値が利用不能であることを示す。sidecarに存在する値・未知成分・価格版を優先し、固定列で補完・再計算しない。対応するdurable observationも同じ既知性を保持する。v1/v2に表現できるかはWIREの版別規則で決め、保存を旧wireの都合で拒否しない。

v16では同じtransactionで`usage_history(timestamp)`および`usage_model_history(timestamp,model)`の旧global unique indexを除去し、既存primary keyの`usage_history(reset_at,timestamp)`と`usage_model_history(reset_at,timestamp,model)`を保持する。別resetの同minuteを禁止するunique制約を残さず、各exact keyの一意性とsidecarの対応を検証する。index名だけで適合と判定せず、列順・unique性・primary keyを照合する。v14/v15からのこのschema移行では既存rowのkey・値・件数・provenanceを変更せず、旧aliasの再分類や原本復元と混ぜない。v16の2回目migrationはno-opである。read-only readerは旧v14/v15を各版の制約で読み、v16へ無断stamp・index修正しない。旧binaryをv16 DBへ接続させる互換性は主張せず、更新時は対応reader/writerを一組にする。移行または検証失敗はversionを含めtransaction全体をrollbackし、検証済みbackupと旧DBを保持する。

新規に計算した固定family互換値と旧保存値の区別は、同じdurable observationの任意boolean `fixed_family_projection`で保持する。新しいexact model keyをPRODUCTの凍結対応でSOL/TERRA/LUNAの別keyへ集約して固定列を計算した観測だけtrueとし、元keyのsidecar・`model_source`・価格版を変更しない。単なるschema migration、同じsource/5成分/価格版が証明済みの冪等replay、旧保存額の再読では既存markerの有無とsourceをそのまま保持する。新規観測に該当するaliasがなければmarkerを省略する。欠落/falseは旧sourceを再分類する根拠にせず、trueはWIREのv2固定family projectionを`legacy-unknown`へ限定するためだけに使う。型がbooleanでない場合は当該durable observationを無効とし、別keyのsidecar、quota journal、currentの資格を取り消さない。固定6列の欠損、または元のreconstructed/unavailable等による数値公開禁止は、markerより先にWIREの規則を適用する。v3 exact modelのsource・knownness・保存価格・集合完備性にはmarkerを適用しない。このmetadataは数値の別正本や別履歴tableを作るものではない。

checkpointの前回counterの存在は既存の`token_baseline_known`で区別する。falseなら保存済みの旧初期値が数値0でも前回counterの実測と扱わず、全成分の比較baselineは不明である。移行時にその旧初期値を書き換えず、knownnessの解釈で0と不明を分ける。v16で新しく作る基準未取得checkpointはfalseと5成分全てNULLを保存する。trueなら`previous_total`は必須の既知値、他の4成分は原本で得たknownnessのままとし、trueなのにtotalがNULLの新candidateは拒否する。したがってcheckpointの前回5成分列はNULLを許すが、数値event/model rowのtotal必須とは異なる。`fully_attributed_from_zero`はbyte範囲の帰属資格であり、token値0の観測資格を代用しない。trueのtotalが実測0でも未知の内訳を0へ補わず、以後の成分差は両端のknownnessから求める。

このbyte範囲は受理済みの`[0,committed_offset)`とする。その外にある未改行tailや収集budgetによる未読suffixは取得残として保持するが、それだけで既に証明した受理済みprefixの資格を取り消さない。prefix内の独立した構文不正、原本の穴、account・model・数値の不一致は別に保持する。response候補を資格判定待ちとして一時保留したこと自体を、解決後も残る欠損にしない。同じtransactionの既存model・account・原本range・同一response検証が終わった時点で、残るprefix内の独立不正と未解決候補から当該commit候補の完備性を確定する。既存の資格済みcanonical eventへの完全一致を確認して保留候補を除いた場合、その候補の暫定未完備を残さず、消費を再加算しない。

既存の`fully_attributed_from_zero=false`は、保留数やresponse候補数が0という理由だけでtrueへ変更しない。response-recordの同一accepted prefixで今回の暫定判定残留に該当し得るものだけを回復候補とし、`account-counter-prefix-unknown`を含む独立したcomplete欠損があるprefixは対象外としてfalseを保持する。回復候補は既存の分割prefix再構成を使い、固定prefixをbyte 0から既存cycle budget内で再検証する。hash・物理identity・所有区間・保存rawの5成分・再構成した数値と保存済みcanonical証拠の完全な対応が成立した場合だけ、元のaccount bindingを再結合して通常transactionで資格を回復する。canonical照合は当該checkpointのexactな物理5要素と受理offset内の集合を一度取得して行い、raw行を資格済み証拠の代わりにせず、responseごとの全proof再loadや全DBのevent読取りを行わない。回復で既存数値を加算・置換せず、raw anchorや保存identityを変更しない。

この旧falseの回復は、当該sourceに未読suffixがなく、Thread再収集の対象でもなく、通常cycleの残budgetがある場合だけ開始・継続する。通常append、Thread再収集、他sourceの公開、停止を回復完了待ちにしない。未完・不一致なら元checkpointを保持し、cycleだけを書き換えて既存再構成の進捗を先頭へ戻さない。同一入力で終端不成立となった再構成は既存進捗に試行済みとして保持し、同じprefixの再読やcanonical全検査を毎cycle繰り返さない。checkpoint・受理range・所有境界・関連source修復が変わった場合だけ既存の失効条件で再判定する。globalなparser版の無効化、全原本の一括再走、新しいcache・queue・DB列はこの回復の条件にしない。

初回の正のraw baselineを加算しなかった範囲も、利用量0・確定空集合・完全収集へ変換しない。その前の利用を同じ原本から証明できない場合は、そのaccount所有区間内のbaseline以前を未取得のまま保持し、その範囲が必要なperiod観測の完備性を確定しない。以後の検証済みdeltaは記録分として保持できるが、未取得prefixを含む期間全利用量の証明とはしない。baseline時刻より後に始まる別の公式periodは、旧periodのこの欠測だけで不完備にせず、当該period内のsource coverageで判断する。初回rawの実測T=0、または同じ原本の受理済みprefix/eventによる復元で以前の利用まで証明できた場合だけ、その証拠の範囲で欠測を解消する。context調整の正のTや`last_token_usage <= total_token_usage`だけではこの証明にならない。

#### raw counterと実測deltaの境界

`token_count.info.total_token_usage`を常に生涯消費量の累積とは扱わない。上記producerの`TokenUsageInfo::fill_to_context_window`は、totalをcontext windowへ再設定して他成分をdefaultに置き換える。この操作は応答usageを加える`append_last_usage`と異なる。外部のこの挙動を変更できるとは表明せず、原本のraw counter、checkpoint/anchor、消費量のdelta/event/model累積を別の意味として検証する。contextの上限値をtoken消費として加算しない。

raw counterではT必須と各成分のUInt64/NULLを保持し、既知I/OはそれぞれT以下、両方既知ならchecked和がT以下であることを必要とする。C/Wに対する既知下限の検証は下記と同じである。TとI+Oの差を消費量や他componentへ転記しない。通常差分は両端のraw値からだけ取り、差分自身について下記のnumeric factの等式を満たすことを必要とする。初回rawはbaselineであり、TとI+Oが一致しないrawも、それを理由に利用量として加算しない。この違いを記録時のmodel totalや公開historyの等式緩和へ広げない。

同じrecord内でも資格を混ぜない。source/account/identityとtimestampは共通の前提である。その前提が成立した`event_msg/token_count`のquota window、累積counter、context表示用のlast/windowは各々の入力条件で判定する。有効なquotaを、不正またはnullのtoken infoを理由に捨てない。有効な累積counterを、欠落・型不正の`last_token_usage`やcontext windowだけを理由に捨てない。この場合context pairは更新せず、以前の有効pairを保持する。last/windowの問題を新しいtoken利用量、0、task停止へ変換しない。context調整の判定に使えるwindowがない場合は、その調整形を推測せず通常raw/deltaの資格だけで判定する。認識できたtask eventの不正はtaskの未処理を保持するが、taskの状態からmodelやcounterを変更しない。source全体の完備性、個別の数値資格、現在の取得成否は独立である。

context表示用のlastは、必須Tと提供された内訳の型・knownnessを検証し、raw counterと同じchecked不等式を満たすものだけを使う。context調整後のTが内訳の和を上回ることを、消費factの等式で拒否しない。同じrecordにこの有効なlast Tと正のwindowが両方ある場合だけpairを一緒に更新する。last/windowの欠落・nullは新pairの未観測であり、旧pairを消さない。提供されたlastの型/数値/算術不正、またはwindowの型不正・0はそのcontext情報の未処理理由と原本を保持し、source全体の完備性を確定しない。ただし独立して有効な同recordの累積counterは処理を続け、model帰属・前回raw anchor・確定deltaをcontext情報の不正で取り消さない。逆に累積counterの欠落・数値不正だけでも、同じrecordの有効なlast/window pairの保存を止めない。このときraw anchor・model数値は保持する。共通前提のtimestamp/envelope・重要key非重複が不成立なら、この独立採用は行わず旧counter/pairを保持する。後続の有効pairを受理しても、以前の不正原本を削除・正常化しない。

認識済み`token_count`で、正の`info.model_context_window=M`があり、raw T=M、I/C/Oがそれぞれ既知0、Wが欠落/nullまたは既知0である場合は、providerのcontext調整形として扱う。これを数値deltaにせず、同じsource lineageの以後の比較baselineだけをこのraw tupleへ更新する。既存model累積、period、checkpointの未処理range、以前のsource欠損を消さず、W不明を0へ変更しない。Tの減少もこの認識形だけなら消費量のrollbackではない。T=Mで、既知I/C/W/Oが全て0だが一部が不明で上記の確定形を満たさない場合は、通常消費とcontext調整を一意に区別できないため数値deltaもbaseline更新も行わず、原本と前回有効anchor・未処理rangeを保持する。M欠落/不正を固定context上限で補わない。

調整後は同じraw 5成分から通常の差分を再開する。調整値が差分で相殺されたことは、既知成分のchecked差とdelta自身の算術条件で検証する。source差替え、history_base、未認識record等で既に継続が不明なら、context調整形を見ただけでそれらを解消したとは扱わない。再起動後も保存済みraw tupleのknownnessから同じ判定を行い、T-I-Oを別のmodel累積や補正履歴へ永続化しない。raw counterの検証とnumeric factの検証を同じpredicateに混ぜない。

以下はlegacy-counter区間の消費deltaと、両kindに共通のraw比較状態についての優先順である。response-record区間では上節のusageだけを消費eventとし、表の通常deltaは数値へ加算しない。同じsourceのbyte順で適用する。表の「保持」は既存の数値・knownness・未処理理由を含み、失敗した値を次の基準へ置換する意味ではない。

| 条件（上から適用） | raw比較baseline | model数値と未処理情報 |
| --- | --- | --- |
| source/account/identityの前提が不成立、または別sourceのanchorを照合中 | 元anchorを保持。8.7.2の一致検証だけで継続を回復 | 当該未資格区間を加算しない。他sourceとquota laneの有効値を保持 |
| `info`がnull以外で、改行済みrecordのcounter必須T/型/raw算術が不正 | 前回baselineを保持 | そのrecordの数値を採用せず未処理を保持 |
| `info=null` | 変更しない | token sampleを作らない。有効な同record quotaは独立して採用可能 |
| context調整の確定形 | 継続前提が成立した場合だけ調整rawへ更新 | 消費eventを作らない。既存累積・未処理・完備性を保持 |
| context調整か通常消費かが曖昧 | 変更しない | 数値deltaを作らず、原本・未処理を保持 |
| 有効rawで前回実測baselineがない | 今回rawを初回baselineとして保存 | 初回累積を加算しない。以前の利用量を0としない |
| 通常rawが既知componentで後退、またはchecked deltaのnumeric算術が不成立 | 前回baselineを保持 | clamp・0再開・失敗値へのrebaselineをせず未処理を保持 |
| 通常deltaが有効、model帰属が一意 | 今回rawへ更新 | 元時刻・exact modelのdeltaを一度だけ保存。差0もbaseline更新は可能 |
| 通常deltaが有効、model帰属が混在/不明 | 今回rawへ更新 | 差分と根拠を内部の未帰属情報に保持し、公開modelへ加算しない |

数値不正の後に正常rawが来た場合も、表の保持baselineとの差で判定する。例えば前回T/I/O=`20/12/8`、不正な後退`10/6/4`、後続`25/15/10`なら、後退は基準へ採用せず、後続の差は`5/3/2`である。15を追加利用にしない。この後続が有効でも、前の不正recordとその原因未解決を消さず、source全体の完備性を自動回復しない。

採用手順は次の通りである。

1. 改行で完結したUTF-8 JSON objectをsource byte順で処理する。重要fieldを含むobjectの重複keyを拒否し、last-winsで採用しない。無関係なpayload本文は状態の根拠にしない。EOF・不正recordの隔離は本書第2章のlocal usage / live rollout別の規則に従う。
2. event種、timestamp、token成分、model contextを検証する。同一秒の順序はsource byte位置で保持し、別sourceのmtime等で順序を発明しない。T/I/C/W/Oはtotal/input/cached/cache-write/outputの略である。T,I,Oが既知ならT=I+O、C,Iが既知ならC<=I、C,W,Iが既知ならC+W<=Iをそれぞれchecked演算で検証する。未知componentはそのcomponentを必要とする等式だけを未判定とし、例えばW不明でもT/I/Oの矛盾を受理しない。加えて、未知componentに非負整数を入れて全条件を満たせることを検証する。具体的には既知Iと既知OはそれぞれT以下、既知のC/Wだけのchecked合計はIが既知ならI以下、Iが不明ならT以下とし、I不明でO既知ならその合計とOのchecked和もT以下でなければならない。C/Wが不明の箇所をこの下限検証に加算しないことは保存値への0代入ではなく、未知成分を保存・公開・逆算しない。T/I/O既知のchecked和がoverflowする場合、または上記の既知合計がoverflowする場合も拒否する。この有限条件を確定delta・event・model累積・読取り公開のそれぞれへ適用し（原本のraw counter/checkpointは前節の別条件を使う）、未知の内訳を理由に既知の矛盾を通さない。この整合条件は本製品のrawモデル契約であり、外部JSON schemaが算術を保証するという主張ではない。不成立は原本を保全して当該modelの当該観測を採用せず、前回の有効anchorを保持する。他modelを消さない。
3. legacy-counter区間では、あるsource lineageの初回累積はbaselineであり、初回値を新しい利用量として加算しない。既存checkpointの前回rawと完全一致を検証できる場合だけそこから再開する。baseline以前の利用は、既存の受理済みevent/prefixと同じ原本・所有区間から証明できる場合だけ履歴復元の対象になる。初回だから0から始まったとは扱わない。
4. 以後は同一lineageの連続する受理済み累積同士で、既知componentごとの非負差を取る。どちらか片端でも不明ならその成分のdeltaは不明とする（例: 前回W不明→今回W=5のdeltaは5ではなく不明）。今回の既知5は以後の比較baselineになり、次回7ならその区間差2だけは既知である。ただし途中の不明差を含むperiod累積成分は不明のままとし、区間差の既知部分を完全合計へ昇格しない。原本復元で欠けたbaselineが確認できた場合だけ再計算する。token eventと同じsource位置の有効model contextへ帰属する。ただし差分区間で複数modelへ切り替わり、どのmodelの差分かを原本から一意に決められなければ未帰属とする。不正なturn/thread context recordの後も旧modelを暗黙継続せず、次の有効なturn/thread contextまで帰属不明とする。model別累積を含まないSession全体counterを各modelへ丸ごと複製しない。
5. 前節の認識済みcontext調整を除くcounter後退、source差替え、history_base等でlineage継続を証明できなければ負差のclampや暗黙の0再開をしない。既存anchor・未処理rangeを保持し、本書8.7.2の成分一致または`HISTORY-RECOVERY-614`の完全な原本prefixで照合する。照合不能な範囲を採用済みとせず、その影響と独立した有効sourceだけを継続する。
6. `last_token_usage`はcontext pairと原本検証の補助証拠であり、通常差分にもhistory_base継続にも二重加算しない。last<=totalだけで「まだ数えていない」とは証明できない。継続前後の同じaccount/source eventと重複排除を証明できる場合は、別の加算則を作らず同じ原本prefix復元へ収束させる。
7. legacy-counterの確定deltaまたはresponse-recordの採用済みusageを、source identity/byte range/hash、model帰属、公式period/minute、raw componentの既知性とcheckpointと同一transactionで保存する。同じrangeの再読込みはno-op。期間切替はSession累積counterを0へ戻す理由にせず、遅着eventをreceipt取得時刻へ付け替えない。period帰属は`HISTORY-CANONICAL-134`を参照する。
8. 不明componentと不明価格を保持したまま、schemaで表現できる既知値だけを公開する。currentの必須component不足、historyのoptional component、legacy lossless値の違いはWIREが所有する。原本にない数値を埋めて「complete」としない。

### 8.7.0 HISTORY-RECOVERY-614 — Session quotaの認識と復元単位

Session quota adapterは、8.7のsource/account/range検証を通した改行済みJSON objectのうち、root `type="event_msg"`、`payload.type="token_count"`、root `timestamp`が有効なRFC3339文字列、`payload.rate_limits`がobjectである形だけを認識する。これはSession内のcached quota snapshotの認識であり、root timestampで新しく取得したquotaの認識ではない。tokenのinfoがnull、counter不変、snapshotの値が前回から変化した場合も、この区別を変えない。直接rootのtoken_count、camelCase RPC payload、別event内の似たfieldを推測変換しない。

外部制約の根拠は、固定commit `740e5af33c71225640e0c1c1555c514c2c93ab74`の[SessionState](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/state/session.rs#L349-L360)、[response処理](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/session/turn.rs#L2941-L3000)、[token_count出力](https://github.com/openai/codex/blob/740e5af33c71225640e0c1c1555c514c2c93ab74/codex-rs/core/src/session/mod.rs#L4906-L4913)である。RateLimits eventがないresponseも以前のSession snapshotを出力し、context再計算でも同じ出力を使う。payloadにはquotaの取得時刻、更新sequence、responseとの対応IDがない。旧固定commit `44271b1a725e3265d80adde7886d10e48f08d8cc`のsend_token_count_eventもstateからsnapshotを読む。これらの形式だけから個々のquotaのfreshnessを復元することはできない。

| 位置 | 採用する型と意味 | 不明・不正時 |
| --- | --- | --- |
| `rate_limits.primary` / `secondary` | objectまたはnull。それぞれ独立した候補window | 欠落/nullは候補なし。別型のwindowを数値へ変換しない |
| windowの`used_percent` | JSON整数または小数で表されたfinite binary64、0..100。Remainingは`100 - used_percent` | bool、文字列、null、範囲外、非有限は当該windowを候補にしない |
| windowの`window_minutes` | 正のJSON整数。60倍とresetからの減算が公開Unix秒範囲内でchecked演算可能 | null/欠落/小数/overflowは候補にしない。7日で補わない |
| windowの`resets_at` | 公開範囲内の正のJSON整数Unix秒 | null/欠落/小数/overflowは候補にしない |

同じrecordの完全な候補からPRODUCTの最長窓・同長なら後reset・同値ならprimaryの規則で一つ選ぶ。root原timestampは保存位置の時刻として保持し、quota取得時刻へ昇格しない。関連objectの重複keyは拒否する。credits、plan、limit名等を価格・model・消費期間の根拠へ転用しない。このsnake_caseの入力契約はRPC schemaからの類推ではなく、range fingerprintを検証できる保存原本の認識形である。

cached snapshot単独では、NULL/欠落minuteへのRemaining復元、新しい公式receipt/観測点、period IDの新規作成、eventのperiod所属、現在quotaの更新を一切行わない。別recordの同値・値の変化・source byte順・近い時刻・複数Session間の多数決でもfreshnessを補わない。同accountの取得済み公式resetに期間長が未保存であり、source/account/prefixを検証した全候補がそのexact resetについて一つのWで一致する場合だけ、その既知resetのwindow metadataを補える。複数W、公式保存Wとの矛盾、所有時刻不明では補わない。このmetadataだけではRemainingの観測点や消費所属を証明しない。補完で既存の公式観測が必要なR/Wを持つようになった場合は、8.7の全公式証拠による所属再判定へ入力する。補助metadataを保存するtransaction自体ではmodel値を更新せず、依存する所属・累計は同節の別のusage transactionで一括更新する。

quotaの数値履歴を復元できる原本は、`QUOTA-DURABLE-604`の公式取得journal、受理済みreceipt、および公式取得との保存provenanceを検証できる旧durable observationだけである。各原本の元の取得時刻・sequence・exact reset/windowとcanonical minuteを使い、minute代表は同契約の取得順で決める。独立した取得順を比較できない同minute候補は、値とexact R/Wが全て同値の場合だけ一つの事実として扱う。異値なら競合として不明を保持し、ファイル名や列挙順で勝者を作らない。既存非NULL値を復元候補で上書きしない。実測の原本がない欠測はquota未回復として残し、modelだけ復元できても破線のquota gapをrecoveredにしない。

起動時quota復元のwrite atomicityはaccount partition単位である。検証済みbackupを確定してから、そのpartitionで採用できた全quota point・window source pointer・対応durable observationを一つのtransactionで検証・保存する。SQL、storage、整合性検証、commitの失敗はそのpartitionのquota復元全体をrollbackし、そのtransaction開始前のDB/公開root/checkpointを保持する。中間の8件だけを成功公開しない。採用資格のないsource/minuteはwrite前に除外し、有効な独立sourceを消さない。別partitionで既に成功したcommitは巻き戻さず、未成功partitionを残したまま全対象quota復元完了とはしない。

起動時model復元は独立した資格判定と別transactionである。quota復元の条件不足だけでmodel復元を拒否せず、model合計/prefixの証拠不足やmodel復元transactionの失敗を理由に、成立したquota-only復元を拒否・rollbackしない。model側のtransactionでは同partitionの採用済み原本から再構成できる全minute/suffixと旧派生offsetの退役を一括commitし、失敗はそのmodel transaction開始前へ全rollbackする。quota/modelとも既存source event/checkpointと現在receiptを変更しない。各laneの成功・証拠不足・失敗を別に保持し、必要なlaneが未成功のまま全対象復元完了とはしない。quota原本取得laneをいずれの起動時復元待ちでも止めない。

上記は有効な既存eventからの再投影の境界である。原本が旧派生event/checkpointの誤計算または採用資格の欠如を証明した場合だけは、PRODUCT `HISTORY-RECOVERY-614`の「原本が証明する派生eventの修復」を使う。通常の再投影で元eventを変更しない条件と、この全prefix証明を伴う修復を混同しない。schema移行で旧値を変えないこと、原本/accepted range/公式receipt/verified backupを保持することは、どちらの経路でも維持する。

通常cycleでは起動時の全対象探索を行わず、新たに受理するsource batchのmodel・range・checkpointと、同じbatchに含まれる資格済みcached window補助metadataを同じusage transactionでcommitする。その補助metadataの保存が同じtransactionで失敗した場合はmodel/checkpointだけをcommitせず、batch全体をrollbackする。独立した公式quota取得・原本保存・receipt受理の失敗を、このsource batchのrollback条件へ読み替えない。公式quota laneが失敗中でも、確認済みaccount/source資格のあるSession記録は継続する。起動時・通常cycleとも、既に適用済みの原本rangeの再適用処理だけでは値・件数・世代を変えない。これは原本再生の重複排除であり、別の新しいcollection cycleを正式commitする場合の世代前進を抑止しない。保存失敗後の無変更同callback retryは行わない。

### 8.7.1 SESSION-129 / DB-129 — thread context観測値の保持

`session_checkpoints`の`context_usage_tokens`と`context_window_tokens`はnullableな観測値である。旧checkpointや
まだauthority pairを得ていないcheckpointのNULLはUIの`未観測`へ投影し、0へ変換したり、累積total・model値・別sourceの
windowから推測したりしない。usage=0かつ正のwindowという観測済みpairは有効な0%として保持する。

同じthreadのusage/window pairを次の観測で得た場合だけ、usage rowとcheckpointを同一transactionへ保存する。以後の
appendやrestartで新しいpairが欠けても、既存の観測値をNULLへ戻さず保持する。旧checkpointのNULLは新しいauthority
観測が得られるまでNULLのままとし、context補完だけを目的に旧履歴全体を再読込しない。追加pollやbackfillも行わず、
更新範囲は通常のbounded appendと、そのcommitで得たauthorityへ限定する。

#### 取得済み原本を別の取得済みfileから読む条件

これは起動時の原本修復だけに使う読取り資格であり、異なるdevice/inodeを同じphysical sourceやgeneration aliasへ変換する規則ではない。旧sourceが現在pathに存在せず、同じpartition・root/pathに現在のdevice/inodeで別途受理済みのrangeがある場合、次の全条件を満たす固定prefix `[0,N)`だけは旧sourceのbyte内容を証明する読取りcontainerとして使える。

1. 現containerはsymlinkなしで開き、現在のdevice/inode、root/pathと、同partitionで保存済みの現accepted rangeのidentityが一致する。読取り前後のFD/path identityと固定prefix内容が一致する。未受理の別file、filename/mtimeだけの一致、別accountのrangeでは代用しない。
2. 旧sourceのaccepted rangeと現containerのaccepted rangeが、それぞれ独立に先頭からNまで穴なく覆い、検証に使う全rangeのfingerprintを元のbyte範囲で再計算して全て完全一致させる。現rangeがNを越える場合は、その保存hashを検証するためのrange終端まで読む。片側のhashだけ、選んだ数行だけ、同じ数値だけでは資格を満たさない。Nは双方の完全な改行境界とする。
3. 同じbytesの全prefixを同じ入力契約で再生し、session ID、account所有区間と各eventの数値/model/period資格を検証する。旧sourceのbyte位置/hashは変えず、取得元の現container identityを別の読取り根拠として保持する。復元eventをコピー先の追加利用として登録しない。

一つでも不成立ならそのNの旧prefixを証明済みにせず、現在のfileで旧source identityを書き換えない。より短いNを使えるのは、そのNまでを独立に全検証でき、対象旧eventと依存minuteをそのprefix内で閉じられる場合だけである。未検証の旧suffix・pending・欠測とその期間別理由は残す。byte位置が異なる場合はPRODUCTの別受理rangeと正規化event完全一致による既存の位置変更条件を適用し、上記のbyte同一証明と混ぜない。通常の置換後収集には以下8.7.2のanchor照合を維持し、この読取り資格だけでcheckpointのidentity/cursorを進めない。

旧byte内容を証明できない場合の、別途受理済み原本へのevent再対応付けは別の操作である。同じpartition・root/pathにおいて、新physical sourceの改行済みprefix全体を、そのsource自身のidentity・accepted rangeの穴のない被覆・全hash・session metadata・所有区間・版別入力で独立検証する。そのうえで、対象旧physical sourceの保存済みevent集合を一部の期間だけへ絞らず全件取得し、旧eventの元観測秒・exact model・lossless 5成分と完全一致する採用済みeventが新prefixに各1件あり、旧新の対応が一対一であることを必要とする。旧familyだけはPRODUCTの限定互換照合を使う。同じ値が複数箇所にあれば対応を作らず、時刻の近さ、filenameだけ、最新model、選択中periodで救済しない。旧sourceの対象外suffixに別の保存eventが残る場合も全件一致ではない。

この全条件が成立した場合だけ、同値の旧eventの数値・保存key・旧pointer・保存価格を保持したまま、新prefix内の実在token_countと必要なresponse rootのpointer/hash・元時刻・response key、および公式観測履歴から一意に決めたexact reset/windowを同eventの資格provenanceへ結び付ける。responseのcanonical原時刻へ置換する場合だけは、前述の全prefix修復規則に従って旧eventを退役し、旧保存key/pointerはbackupと修復provenanceへ保持する。旧eventに新旧の二つの利用量を作らず、同じresponseを再加算しない。新prefix内の独立した採用済み追加eventは、同じresponse重複排除と所有・期間資格を満たす場合だけ一度追加できる。変更した資格とevent集合から、PRODUCTの原本修復transactionで依存する全period/minuteを再構成する。

旧eventが既知のResponseRecordを持つ場合は、新根拠でも同じ5 IDとusageの全6成分（reasoningを含む）を保つ。
この場合のcanonical代表は旧保存key・旧admissionのままとし、新physicalの実在pointerと独立資格は別の根拠relationとして保持する。
同じresponseの新raw行を重複INSERTしたり、新sourceのadmissionを旧sourceへ書き込んだりしない。
旧responseの別ID・第6成分だけの差・responseからcounterへの置換はいずれも拒否する。

これは同じ数値観測への検証済み根拠の再対応付けであり、旧新fileのbyte同一、旧prefixのhash検証成功、同じphysical identity、counter lineageの連続性を表明しない。旧range/hash・checkpoint identity/cursorを変更せず、新sourceのcontext/anchorを旧checkpointへ継承しない。全件一致しない場合はこの再対応付けを行わず、旧値と未確認理由を保持する。新prefixだけで証明できた独立した事実の保持と、旧値との重複が解消していない期間への数値追加を区別し、後者を行わない。原本・backupを保全し、再実行はno-opとする。

保存済み修復provenanceを通常記録・再投影・読取りの数値資格へ再利用する際は、全payloadの原JSON文字列または全payloadの展開済みJSONを同時保持しない。各保存rowのexact hash・JSON・mappingと参照元の検証を保ったまま、一件ごとに処理してその原JSON・展開済みJSONを解放し、後続処理には必要なpointer・数値・旧観測key等の検証済み集合だけを渡す。参照集合を先に求める複数passが必要な場合も、各pass内は一件ずつとし、同じ保存payload集合を読んだことをID/hashの固定集合で照合する。不一致・不正・参照消失ではcandidateを拒否し、前回の保存事実と公開stateを保持する。これは総RAMの固定上限や保存provenanceの削除・切詰め・検証省略を定めるものではなく、保存した履歴の総JSON量を同時展開する増幅を避ける読取り責務である。

候補の除外は数値資格の完全検証とは別の責務である。現在の保存producerはrootの`version`を一つだけ持つcompact JSONを生成する。そのroot終端byteが`,"version":1}`であるrowはv2の関係記録へ使用しないため、全文構文を検証せず候補から除外できる。この除外に一致しない保存形は完全検証の候補として残し、終端の判定だけでv2を採用してはならない。v2の完全資格はtop-level JSON objectに`version`が正確に一回だけ存在し、serde_jsonの整数表現で`u64`として`2`であることを要求する。`2.0`、`2e0`、文字列`"2"`、bool、null、欠落、root配列、重複したrootの`version`は資格を拒否する。空白とkey順の違いは、それ以外の全証拠が成立するJSON整数`2`なら受理できる。版の検査は既存の全JSON読取りの一回のdeserialize内で行い、浮動小数への変換や、解析後に重複keyを失ったValueだけの判定を使用しない。v2候補はexact hash・全JSON・版・mode・mapping・参照元を検証し、必要な複数pass間のID/hash集合照合も維持する。候補外のv1を検証済みと称さず、保存rowを変更しない。不正な候補・hash不一致・参照不一致では候補を拒否し、過去値と未解決状態を保持する。旧SQLiteのfirst-key解釈や重複した`version`の解釈は、この候補除外または数値資格の契約としない。

候補外canonical v1の本文を通常の候補取得ごとにSQLite内部でも走査しない。`source_prefix_repairs`の候補predicateと同一条件を持つ`repair_id`の部分indexを、通常writerの既存初期化transaction内で未作成時に構築し、以後SQLiteの同一transactionによる更新で維持する。これは候補探索用の派生indexであり、proofの数値資格を付与しない。既存proof本文・ID・hash・世代・数値行は変更せず、選択された候補の完全検証を維持する。同名の既存indexは変更せず、通常のSQLite plannerがpredicateの適合性を判断する。indexの定義一致を記録・数値資格の条件にはしない。読取り専用経路はDDLを行わず、index未導入DBの従来読取り互換を保持する。初回構築の走査は起動時の一回として区別し、通常収集・停止成功の代用証拠としない。

proofを数値資格として利用しない既存state維持/空入力処理をproof全検査に依存させない。現在accountの不足sourceを再収集するcycleでは、他accountの履歴復旧をその前提にしない。履歴復旧は次の通常cycleへ保持し、停止要求後に次の独立した履歴job・repair・通常収集を開始しない。開始済みtransactionと受領済みjournalの保全を放棄せず、未処理を完了扱いしない。

#### 通常取得と任意の履歴修復の実行順序

任意のinactive/current accepted-prefix修復を通常collectionの開始条件にしない。各周回で通常collection、Threadの既存submit/drainと実commitに対する公開処理を先行し、既存のThread待機・再収集制御を終えた後に履歴修復を行う。stopまたはaccount境界違反では新たな修復を開始せず、checkpoint再収集pendingでは次の通常収集を優先する。通常のNoCommit・degradedやACK未成立だけを、保存済み証拠による履歴修復の禁止条件にしない。`--once`は通常phase後に既存の有限修復を一度行って終了し、stopと再収集pendingを優先する。

通常収集と各履歴修復の既存chunk上限・原本range/hash・account/世代fence・transactionを保持する。chunk上限は各phaseに個別に適用し、周回の読取り量は実行した各phaseの消費量の和とする。履歴消費bytesを次の通常収集予算から差し引かない。履歴修復は先行する通常phaseが変更した保存stateを再読し、既存の別transactionでexact rangeとcheckpointをcommit時に再検査する。旧snapshotを順序変更後へ持ち越さない。履歴修復の通常エラーはそのlaneの失敗として保持し、既に実施した通常収集を取り消したり次回収集を省略したりしない。current修復が実commitした場合は同じaccount境界を再確認してその実generationを公開し、次cycleまで古いACKを残さない。未commit・無変更にACKやheartbeatを作らない。

### 8.7.2 SESSION-129 / DB-129 — source置換後のtoken anchor照合

既知のtoken checkpointを持つsourceが置き換わり、その累計anchorが新sourceの固定prefix内にまだ存在しない場合、
元anchor・source identity・照合cursor・pending keyを`session_token_anchor_recoveries`に保持する。
未照合prefixの累計を次回の基準へ昇格せず、元anchorの完全一致より前のdeltaをcanonical利用量へ加算しない。
同一sourceの後続appendと再起動でも元anchorを維持し、他の検証済みsourceの記録は継続する。
一致後の検証済みdeltaだけを、対応するpending解消・sidecar更新・range/event/checkpoint・generationと同じtransactionで保存する。
週切替では同じaccount/sourceの照合証拠を維持し、eventの既存period所属規則を守る。account identity境界は越えない。

照合sidecarの保存互換では、I/C/Oが全て既知のanchorは既存version 1のJSONとrecovery IDを維持する。I/C/Oのいずれかが不明な新anchorだけはversion 2とし、その成分をJSON null、既知成分を従来と同じcanonicalな10進文字列で保存する。Tは両版で必須、Wは両版で既知文字列またはnullである。version 1のI/C/Oへのnull混入、未提供key、不正文字列をversion 2として救済しない。version 2のIDはversion 1と異なるdomainを持ち、各optional成分の既知/不明を含む。schema-only移行で旧payloadやIDを再生成せず、原本上の値とknownnessの完全一致だけを照合条件にする。

IDのbyte列は、ASCII domain `codex-info-session-token-anchor-recovery-v1`または末尾が`v2`の同domainとNULに、以下を順に連結してSHA-256小文字hexとする。文字列はUTF-8 byte長をu64 big-endianで前置する。順序はpendingの`root_identity,relative_path,record_sha256,parser_version,reason`（文字列）、`file_device,file_inode,start_offset,end_offset,cycle_seq`（各u64 big-endian）、`collector_epoch,prefix_generation`（各u128 big-endian）、`complete`（false=0/true=1の1byte）、anchorの`root_identity,relative_path,prefix_sha256`（文字列）、`file_device,file_inode,committed_offset,cycle_seq,total_tokens`（u64）、`input_tokens,cached_input_tokens,output_tokens`、`prefix_generation,collector_epoch`（u128）、`fully_attributed_from_zero`（1byte）、`cache_write_input_tokens`である。整数のendiannessは全てbig-endianとする。I/C/Oはversion 1では各u64、version 2では不明をbyte 0、既知をbyte 1とu64の組にする。Wは両版で同じoptional表現を使う。version 2でI/C/Oが全て既知のpayloadも非canonicalとして拒否する。

raw anchorの一致資格とmodel contextの資格は別に判定する。source/session identity、accepted prefixとrecord hash、同じ連続所有区間、全raw成分とknownnessの一致を検証できた位置では、追加利用0の比較baselineとして照合を再開できる。照合中に有効なturn/thread contextが現れたことだけをraw再開の拒否理由にしない。別sourceの旧modelはcounter一致だけで引き継がず、現sourceで独立に資格を得た現在contextだけを使う。一致位置は次の差分区間の始点とし、それより前の混在/未帰属差分を遡って加算しない。再開後のmodel帰属は`RECORDER-MODEL-01`の差分区間規則で判定する。照合前の未処理・不正record・source欠損・完備性欠如は、一致だけでは解消しない。

`history_base`等でmodel contextのlineage継続を証明できない境界では、同じ物理file内でもkindを`unknown`にする。元のmodel名は監査用に保持できるが、数値anchor一致で有効contextへ戻さない。境界後の有効turn/thread context、または同じaccepted prefixに対応したcontext再構成が独立に成立した場合だけ現在contextを回復する。例えば旧raw20→history境界→有効な新SOL context→証明付きraw20は加算せず新baselineとし、続くraw25の差5をSOLへ帰属できる。新contextの証拠がない同じ列ではraw20の照合後もmodel不明を保持し、差5を旧modelへ加算しない。

canonical schema v13は検証済みbackup後にこの照合sidecarを追加し、既存の履歴・金額・event・checkpoint・pendingを
書き換えない。旧pendingでも、同じaccount/collector epochに属するpending発生前の保存checkpointと、
現在source上でhashが一致するpending原byte範囲が残っていれば、その元基準から通常のbounded収集へ復帰する。
pending発生後のcheckpointを元anchorとして採用しない。既知の非zero基準は全token成分の一致まで加算を保留する。
既知のzero基準では先頭から差分を収集し、存在しないzero token行を待たない。基準未取得の初期値0は既知zeroに昇格せず、
最初の累積counterを基準としてのみ採用する。再読込みで保存済みeventを二重加算せず、他sourceの継続収集と週切替を妨げない。
再走査cursorは旧pending範囲の終端とは独立して保持し、複数cycle・再起動でも元基準とpendingの識別を維持する。
anchorが一致しても観測済みsource末尾まで未読部分があれば集計完了にせず、次cycleで残りを収集する。
各cycleの集計完了判定は、そのcycleのinventoryと走査結果、および同じcommit後も残る未解決証拠から決める。
前cycleの一時的な読み残し等（`complete=0`）は今回のinventoryで置換するため、今回解消した記録を
今回のmodel集合の不完全理由として持ち越さない。利用値・pendingの置換・model集合の完了判定を同じtransactionへ保存する。
今回も未読部分や読取失敗がある場合、および未解決の原byte証拠（`complete=1`）は従来どおり完了を妨げる。
証拠が不足する旧pendingは削除しない。照合失敗・transaction失敗では旧状態を保持し、照合できていない状態を完全収集として公開しない。
同じpartitionに残る別collector epochの旧pendingは、世代が違うだけでは現在集計から除外しない。
現epochのcheckpointが元範囲を覆い、元範囲の終端から現checkpointまでに進んだ部分も、同じ物理source・現epoch・
現prefixの保存済みaccepted rangeで隙間なく覆われていることを要する。発生前checkpoint・物理identity・原byte hash・
完全な行境界を再検証して未計上deltaがないと証明できる場合だけ、そのcycleの現在model集合の完全性を妨げないものとする。
既知zeroでは全counterがzero、基準未取得では最初のcounterを基準とし以後全成分同値であることを要する。
history-base継続、未解析・不正record、counter増減、証拠不一致は証明不成立とする。旧pendingと旧履歴の不完全状態は保存し、
旧range/event/taskを新epochとして登録せず、現checkpointから後続appendを通常収集する。

### 8.8 DP-REST-007 / RC-145 — typed generation namespace

bare integerを異なるnamespace間で比較しない。採用型は次のとおりである。

- `BootId`: Linux `/proc/sys/kernel/random/boot_id`のUUID。
- `SupervisorLeaseIdentity`: canonical DB profile、BootId、PID、process start ticks、128-bit owner nonceのtuple。
- `auth_epoch`: logout、account change、identity failure、account worker restartごとに増えるprocess-local u64。overflowはprocess restartによるrecovery-requiredとする。
- `CollectorEpoch`: service startと各identity boundaryで生成する128-bit random ID。未commitの収集candidateは同じepochでだけ受理する。再起動を跨ぐ保存済みSession checkpointの継続資格は8.7/8.7.2のsource identity・prefix/hash・account所有照合で決まり、epoch変更だけで破棄・0再開しない。
- `CycleSeq`: CollectorEpoch内で1から始まり、admitted cycleごとに1増えるu64。
- `DataGeneration`: partition内で0から始まり、usage rowとcheckpointの同一transaction commitごとに1増えるu64。
- `BackupGeneration`: DB profile内のu64と128-bit backup ID。parent DataGenerationとDB SHAを必須にし、一activationで
  最大1世代だけ増える。0/1/2から同一snapshot複製で3世代を穴埋めしない。
- `MigrationGeneration`と`RestoreGeneration`: 128-bit operation ID、parent DataGeneration、source DB SHA、result
  DataGenerationのtuple。成功switch後だけresult DataGeneration=`current+1`、失敗時は未発行である。
- `ServiceGeneration`と`BootstrapGeneration`: service manager activation IDとartifact SHAのtuple。

`DataGeneration`は公開値の差分番号ではなく、受理した保存transactionの世代である。新しいcollection cycleの
usage/checkpoint transactionは、usage値が前回と同じでも世代を1増やし、対応する`CollectorEpoch`と`CycleSeq`を
同時に保存する。同一cycleの完全一致する再送は再commitせず、世代と完全tupleを保持する。
独立した新規quota receiptまたは取得healthの保存transactionも世代を1増やすが、collectionのepoch/seqを進めない。
同値のquotaでも新しいreceipt identityを保存する場合は新世代であり、同じreceiptの再送と区別する。
従って、同じ`DataGeneration`のまま新しいcollection cycleだけを受理する経路はない。
保存世代が進んでも公開domainが同じ場合のpair再利用はWIREの公開契約に従う。

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

systemd installed profileのprocess/service役割はPRODUCTのRecorder/REST構成に従う。Recorderがjournal/lease/checkpointを検証して書込みを所有し、RESTは検証済みaccount DBのcommit済み値だけを読み取る。REST listenerは初期化generationを公開できるが、Recorderのrecovery admissionを満たさない値を新しい正常data generationへ昇格しない。
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

対象構成は1 recorder、1 loopback API、1 Windows clientとし、UI要求頻度はWIRE ownerのsurface別周期へ従う。通常処理のCPU/RSS/絶対msはmachine、storage、入力規模、同時負荷に依存するため保証固定値にしない。旧記載のCPU合計2.0%等には対象host/baselineのauthorityがなく、製品の合否境界へ流用しない。

保証するのは次の仕事量と責務である。

- fingerprint不変のSession sourceは全再走査せず、既記録rangeを再加算せず、新規Session row/checkpointのwriteを0件にする。これは公式quota原本のdurable commit、scheduled generation/heartbeat、正規maintenanceを禁止する意味ではない。それぞれのownerが必要な最小writeを行う。
- UIの同じsurfaceはin-flight 1、poll queue 0とし、完了前に次requestを重ねない。request外にAPIがquota/sourceを再収集しない。必要なpage集合の複数requestと周期pollを混同しない。
- `changed,backfill,maintenance,recovery`は入力と処理範囲を区別する。source固定prefixの再検証要否は8.7の判定表で決め、検証が必要な一つの候補では同節の読取り前後の確認を行う。これは失敗後の同callback再試行と区別する。通常収集のbatchとselected Session prefixは4.4の処理予算を使う。起動時復元・修復は同節が区別するowner固有の一括transactionへ従う。overflowを保持し、別の古いfileで同じprefixの穴埋めをしない。commit/publicationの原子性、公式原本保存と再試行は各ownerの規則へ従う。
- 安全境界・deadlineの超過を成功へ変えず、該当lane/surfaceに隔離して他の有効値を保持する。負荷低減のためrawを捨てたり、未知値を0へ変えたり、不要なpollを追加しない。

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
quota sourceの完結性だけをrecorder generationへ明示して運び、Session inventory/model scanのoverflowや失敗を
quota gapの`pending`継続条件にしてはならない。

RESTへ公開するのはconfirmed rowだけで、raw path/cursor/process/ownerを除外した`history_gaps` projectionとする。field、domain、順序、輸送上限、版別のstrict schemaはWIRE ownerだけに従う。製品versionやmanifest版の一致をwire互換性の代用にしない。

Graphのraw anchor、未回復gapの破線、部分復元のmetric別coverageと終端はPRODUCTの`G137-GRAPH-01`へ委譲する。pending/rejected ledger、timestamp間隔だけ、transport errorからgapを推測しない。原本復元後の実sampleとgap集合は同じ新published pairでatomicに採用し、表示だけledgerをrecoveredへ変更しない。quotaの全missing minuteを元時刻の有効なquotaで回収できた場合だけledgerをrecoveredとし、modelだけの回復はquota ledgerのterminalを変えない。既存confirmedからの明示repairも同じsource/account/period証拠を必要とし、保存・再適用を冪等にする。

## 9. RC-167〜169 データ保護 fault / source checkpoint closure

この節は `WIN-J-007..016` に対する要件抽出の正本であり、製品実装済み・実機確認済みを意味しない。
既存行の `depends_on` と B2B projection header は変更せず、ここで定義する RC-167〜169 は
既存行の independent oracle へ join する。実行前提でない oracle join を新しい hard edgeへ昇格させない。

### RC-167 — source rotation / truncation checkpoint

source identityは8.7の完全keyを使う。file sizeとmtimeは、そのcycleで固定した読取り範囲とinventoryの観測metadataであり、保存identityの要素ではない。Windowsでdevice/inodeに対応する値は、それぞれ`device_or_volume_serial`、`file_index_or_inode`である。

`prefix_generation`は同じroot/pathで区別するlineageのopaque 128-bit値である。新lineageの発行値は、そのroot/pathの既知generationと重複させない。同じroot/path/device/inodeの保存済みsourceについて、固定prefixの継続を8.7で検証できた場合は、そのgenerationを再利用する。8.7.2のraw anchor一致だけでは、置換先へ旧source key/generationを移さない。CollectorEpochの交代、通常append、size/mtimeの変化、quotaのresetだけでは再発行しない。比較するprefixは、前回の検証済み改行位置Cまでの固定範囲`[0,C)`であり、新しいEOFまでのwhole-file hashではない。Cより後ろへの追記でhashの対象範囲を広げ、旧prefix変更と判定してはならない。

`source_event`は変更を観測した場合の`append`、`rotate`、`truncate`、`replace`の4値である。前回commit済みの観測と、現在cycleで固定した候補を次の順に分類する。解析・hash再確認中に候補のidentityまたは固定範囲のbytesが変わった場合は、分類を確定せず、そのsourceの未commit候補を取り消す。

| 比較できた事実 | 分類・generationの扱い |
| --- | --- |
| 同じroot/path/device/inodeで、sizeが以前の改行位置C未満へ縮小した | `truncate`。旧generationとanchorを保持し、新しいlineage候補を8.7.2で照合する |
| 同じroot/path/device/inodeだが、既存の固定prefix `[0,C)`が一致しない | `replace`。同sizeの書換えも含む。旧generationを上書きせず、新しいlineage候補を照合する |
| 旧pathのdevice/inodeが変わり、同じ完全inventory内の別pathに旧device/inodeのsourceがちょうど一つあり、その旧accepted prefixの全range/hashとSessionを一致検証できる | `rotate`。移動先を別pathのsourceとして8.7.2で照合し、旧keyを移動先keyへ書き換えない。path変更だけで新規利用を加算しない |
| 旧pathのdevice/inodeが変わったが、直上の移動証拠が成立しない | `replace`。filename・mtime・外部で未定義のmarkerからrotationを推測しない |
| 同じroot/path/device/inode、旧prefix一致、size増加 | `append`。同じgenerationを継承し、未読suffixだけを処理する |
| 同じroot/path/device/inode、旧prefix一致、sizeも同じ | 新しいsource eventなし。未完了batchや旧資格の再検証が必要な場合だけ各ownerの処理を続け、mtime変化だけで再加算・generation更新をしない |

sizeが減ってもC以上であり旧prefixが一致する場合、失われた部分は未commit suffixである。その未commit候補を取り消し、旧checkpoint/generationを保持してC以後を通常の有界読取りへ戻す。commit済みprefixのtruncationと同一視しない。現在pathが存在しない・読取不能の場合も、架空の置換sourceや新generationを作らず、旧source/anchorと不足理由を保持する。

別pathに同じ旧device/inodeが複数ある場合は移動先を一意に証明できないため、旧pathの変更は`replace`として扱う。列挙順やpath名で一つを選んだり、旧sourceを各pathへ複製したりしない。別pathの各候補はそれぞれの完全source keyと8.7/8.7.2の資格で検証し、この分類だけで既存range・model・eventを引き継がせない。

- `append` は同じ device/inode/prefix_generation の durable cursor
  `(last_complete_lf_offset,last_complete_row_sha256)` 以降だけを読む。
- `rotate`、`replace`、`truncate`、prefix不一致は旧source identity/anchorを保持し、8.7.2の成分照合へ進む。EOFへ飛ばして照合完了にせず、境界以前のbytesを新account/lineageへ自動帰属しない。
- 旧cursorで有効recordを `skip` する数は `skip_count=0`、dedupe keyによる重複insert数は `dedupe_insert_count=0` とする。
  range/eventのdedupe keyは8.7の完全keyを使う。partitionごとの物理DB分離はその外側のnamespaceであり、root/path/prefix generationを省略する理由にしない。異なるgenerationの旧event退役には同節の全prefix資格を適用する。
- 1 eventの解析scanは各cycleで最大1回、受理batchのDB transactionは最大1回とする。原本再読の要否と検証結果の再利用は8.7の判定表で決める。そこで必要とした解析bytesと再確認hashの照合は、この解析scanとは別の検証であり省略しない。同じcallback内のretryは`0`、次の通常cycleまたは明示操作ごとの再試行は最大1回とし、失敗を理由に同cycle内で繰り返さない。入力・通常batchの処理予算と起動時修復の一括transactionの区別は4.4を参照する。全inventoryのaggregateがselected prefix予算を超える場合はolder overflowを保持し、Session入力fingerprint不変かつ未処理batchも資格再検証も不要の場合は全走査と新規Session row/checkpoint writeを各`0`とする。公式quota原本・generation/heartbeatのwriteはそれぞれのownerに従う。既存marker cleanupは次の通常cycleに最大1回とする。

RC-167 oracle は `source_identity_before/after`、`prefix_generation_before/after`、cursor before/after、
`scan_event`、`scan_count`、`scan_bytes`、`scan_records`、`transaction_count`、`skip_count`、
`dedupe_insert_count=0`、前後の完全record hash、DB row/file SHA、publisher generation、restart traceを
同一case markerへ結合する。source/prefixの継続を証明しない旧cursor流用、無制限再走査、4 MiB超record受理、前後valid recordの削除、
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

## REST診断ログの保存（Issue #482）

`REST-LOG-RETENTION-482`: REST failure logは既存data root配下の `logs/rest/rest-YYYY-MM-DD.log` にJSON Linesで保存する。日付境界はUTCの暦日。当日と直前6日を保持し、次の記録時にそれより古い同形式のファイルを削除する。記録のない日について空ファイルを生成しない。再起動後も同日ファイルへappendする。保持中のファイルを直接読取り、直近7日の記録を閲覧できる。保存・削除の失敗は秘密値を含まない固定診断をstderrへ出し、HTTP結果やDB/publicationを変更しない。
