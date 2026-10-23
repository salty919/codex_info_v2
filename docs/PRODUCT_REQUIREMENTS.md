<!-- codex-info-requirement-owner: PRODUCT -->
<!-- codex-info-master-ids:
LINUX-UPDATE-DIAGNOSTICS-479
ASTRA-COST-01
MODEL-PRICING-463
MODEL-PRICE-UPDATE-463
MODEL-USAGE-DISPLAY-01
API-LIFECYCLE-01
RECORDED-USAGE-SCOPE-604
ACCOUNT-LIFECYCLE-134
LIVE-RESET-129
CURRENT-PERIOD-BOUNDS-129
REST-172
THREAD-TITLE-362
THREAD-CONTEXT-362
THREAD-OPEN-362
CUM-138-04
G137-GRAPH-01
HISTORY-RECOVERY-614
WIN-PARITY-DATA
WIN-PARITY-STATE
WIN-PARITY-OPS
PROC-LAUNCH-01
PROC-STOP-01
PROC-OPTIONS-01
PROC-HELP-01
WF-BINARY-IMPACT-01
WF-FEAT-SELECTIVE-01
WF-QUALITY-ONCE-01
WF-NONBLOCKING-QUALITY-01
WF-POSTMERGE-01
VER-AUTO-PATCH-01
VER-SERIES-FIXED-01
VER-BETA-IDENTITY-467
VER-BETA-BUILD-CORE-467
VER-BETA-LINUX-BUNDLE-467
VER-BETA-LINUX-CHANNEL-467
WF-SERIAL-01
LINUX-BUNDLE-TARGET-01
LINUX-BUNDLE-RELEASE-01
LINUX-BUNDLE-CONTENTS-01
LINUX-BUNDLE-OPERATIONS-01
LINUX-BUNDLE-AUTOUPDATE-01
LINUX-BUNDLE-AUTOUPDATE-02
LINUX-BUNDLE-AUTOUPDATE-03
LINUX-BUNDLE-RETENTION-01
U128-01
U128-02
U128-03
U128-05
U128-06
U128-07
U128-09
U128-10
U128-12
U128-13
U128-14
U128-16
U128-17
U128-21
U128-25
U128-26
U128-27
U128-28
U128-30
U128-33
U128-35
U128-36
-->

# Codex Info 製品要件

この文書は製品要件の唯一の入口であり、各契約領域のownerを次表で一意に登録する。

`ASTRA-COST-01`: ASTRAの概算はユーザー指定単価（100万token当たり通常入力$10、cached入力$1、cache write入力$12.50、出力$50）による。入力総数をI、cached入力をC、cache write入力をW、出力をOとして、通常入力はI-C-W、代数上の合計は `((I-C-W)*10+C*1+W*12.50+O*50)/1_000_000`であり、実際の演算順とbinary64丸めは下記の新規参考額計算規則へ従う。未提供のWや矛盾する内訳を0として確定表示しない。この旧familyデータ用の固定参考単価は保持する。正確なmodel IDがある新規記録の公式単価は`MODEL-PRICING-463`を使い、旧familyを新モデルへ推測変換しない。
`MODEL-PRICING-463`: API相当USD参考額は公式 `https://developers.openai.com/api/docs/pricing` のStandard・短文脈（100万token単位）を基準とし、通常入力・cached入力・cache write入力・出力の4単価を正確なmodel IDごとに保持する。対象はgpt-5.6-sol/terra/luna、gpt-6-astra/sol/luna、gpt-6.1-sol。単価catalogは出典、観測時点、内容revisionを持ち、更新時は旧revisionを保持する。未観測のcache writeや矛盾したtoken内訳、未知modelの料金を0や別モデル価格へ置換しない。世代が失われた旧family履歴のtoken・保存済み金額は維持し、最新modelへ再割当しない。価格lookupと表示familyの集約は分離する。これはStandard・短文脈を基準にした参考額であり、実際のspeed/context料金、ChatGPT請求額、quota、creditsを表さない。既存記録済み履歴金額を最新料金で上書きしない。旧APIの固定family金額欄は従来の単価・family集約を維持し、v3はモデルごとの保存額を優先して旧familyと正確なIDが混在しても二重加算しない。

上記の「従来の単価」は次の固定互換契約である。SOL/TERRA/LUNAの値は旧製品の参考計算値を保持するための実装値であり、現在の公式料金やexact modelの単価を保証するものではない。ASTRAは`ASTRA-COST-01`のユーザー指定値である。単位は100万token当たりUSDとする。

| 対象 | 通常入力 | cached入力 | cache write入力 | 出力 | 固定価格版 |
| --- | ---: | ---: | ---: | ---: | --- |
| SOL互換 | 5 | 0.5 | 独立項なし | 30 | `LOCAL_ESTIMATE_V1_2026-08-14` |
| TERRA互換 | 2 | 0.2 | 独立項なし | 12 | `LOCAL_ESTIMATE_V1_2026-08-14` |
| LUNA互換 | 0.2 | 0.02 | 独立項なし | 1.2 | `LOCAL_ESTIMATE_V1_2026-08-14` |
| ASTRA旧family | 10 | 1 | 12.5 | 50 | `ASTRA_USER_2026-09-05` |

旧familyとの互換対応は、SOL=`gpt-5.6-sol|gpt-6-sol|gpt-6.1-sol`、TERRA=`gpt-5.6-terra`、LUNA=`gpt-5.6-luna|gpt-6-luna`、ASTRA=`gpt-6-astra`だけとする。各旧family key自身も同じfamilyに属する。大小文字を変更せず、未知IDを部分文字列で割り当てない。この対応は旧互換額・表示group・既存旧family eventを保持する照合のためだけに使い、exact-ID料金lookup、DBのkey置換、旧履歴の版名推定に使わない。

SOL/TERRA/LUNAの互換計算は、同familyの検証済みI/C/Oを整数として合算してから通常入力を`I-C`とする。cache writeは元Iに含まれるままで、独立項を作らず、元Wを既知0へ変更しない。I/C/Oの不足または`C>I`では価格不明とする。ASTRA旧familyとexact-IDは`I-C-W`を通常入力とし、I/C/W/Oの不足または矛盾では価格不明とする。unknown modelをこの表へ推測割当しない。

新しく計算する参考額の演算はIEEE 754 binary64、round-to-nearest/ties-to-evenで固定する。整数の成分・整数差を先に正確に求め、各成分nの金額を`round64(round64(float64(n) / 1_000_000) * rate)`とする。単価の十進値も同じroundingでbinary64へ変換する。合計は通常入力、cached入力、cache write入力、出力の順に各加算をround64する。独立項のない互換計算は通常入力、cached入力、出力の3項とする。途中の表示桁丸め、積和融合、eventごとの丸め額の累算、式の並べ替えを行わない。価格算出後の表示familyの合算・履歴差分・日別配賦には、下記`G137-GRAPH-01`の保存binary64数量を正確に扱う規則を使う。各modelの保存額はmodel keyのUTF-8 unsigned byte昇順で列挙し、表示集計を価格の再計算へ戻さない。非有限値や必要な未知額があれば全額不明とし、部分額や0へ置換しない。例としてSOL互換のI=134789644、C=130605312、O=1099250はbinary64の`119.20181600000001`になる。代数的に同値な式で得る`119.201816`への変更は同じ保存値ではない。

この計算定義は、新しい観測と原本検証を満たす派生履歴の復元に適用する。同じtoken内訳・同じ価格版の保存済みsidecar額は通常の再読込み・再保存でbit値と欠損を保持する。旧固定列もmigrationやreader/UIでは再計算しない。既存minuteの変更は`HISTORY-RECOVERY-614`の全原本prefix・期間終端の一致・backup・transaction条件を全て満たす復元だけに限定し、その場合の旧固定列は上表の同じ互換単価と演算でprefixから再構成する。保存済みexact-ID価格版は保持し、未観測の版を最新価格で補わない。丸め差の大小だけを復元資格、許容epsilon、未使用判定またはgraphでの置換根拠にしない。

`MODEL-PRICE-UPDATE-463`: 公式料金MarkdownのStandard表を日次および手動で取得・検証し、対象モデルの4単価に変更があればcatalogの新revisionだけを追加した`codex/issue-463-* -> feat/next`のPRを作る。取得失敗、表の列変更、重複、対象model欠落、不正な値ではcatalog/refを更新せず失敗する。価格不変なら更新commit/PRを作らない。検証済み旧revisionを削除・書換えず、未処理の自動更新PRがあれば重複作成しない。mainへの直接反映、auto-merge、稼働アプリのnetwork取得は行わない。定期起動にはユーザーによるdefault branchへのworkflow統合とrepositoryのPR作成権限が必要で、その外部有効化をlocal検証済みと混同しない。

`MODEL-USAGE-DISPLAY-01`: 価格は正確なmodel IDごとに算出する。Windows Mainの内訳は価格catalogで採用済みの正確なIDごとに版名（例: 6 Astra、6.1 Sol、6 Sol、5.6 Tera、6 Luna）を表示し、今回期間の公開済み合計利用額の降順、同額はmodel keyの安定順、金額不明は末尾とする。既知IDの価格が未定でも版名を保持する。未対応の新modelはその他へ集約し、版が失われた旧familyは従来名のまま維持して最新の版へ推定変換しない。最大6行を同時表示し、超過行は表内の縦scrollで全件確認可能にする。GraphとLinux Mainは既知IDの表示family集約を維持する。DB/RESTの元model keyとtoken事実は保持し、表示対応を価格lookupに使わない。表示groupに価格未確定のmodelが含まれる場合、そのgroupの合計金額を既知部分だけの値や0として表示しない。現在期間のモデル別利用量は、LinuxとWindowsで同じ3組の表示bucketを使う。v3/v4 current wireの入力総数をI、cached入力をC、cache write入力をW、出力をOとすると、表示Inputはtoken `I-C`と概算ドル「通常入力＋cache write」、表示Cached inputはtoken Cとcached入力ドル、表示Outputはtoken Oと出力ドルを必ず隣接させる。したがって独立列を持たないcache writeはInputのtokenとドルの双方に含め、片方だけへ含めない。v3/v4 adapterだけがraw値をこの表示契約へ正規化し、既に表示bucketを返すv1/v2を再減算しない。有限の概算ドルは小数2桁、価格未定または非有限値は0へ推測せず利用不可として表示する。
`API-LIFECYCLE-01`: canonicalな収集・DB domainはUIの固定field、表示文言、画面構成および特定client versionへ依存させない。public APIはcommit済みdomain snapshotから作るversion別read-only adapterとし、client変更でcollectorまたはDB writerを変更しない。現行UIはv4の現在値、v5の有限directory、v3の選択期間の有限履歴page・Threadsを独立resourceとして使用し、各UIは可視surfaceに必要なresourceだけを取得する。現在値とThreadsの組、およびdirectoryと履歴pageの組は、それぞれ一つの確認済みimmutable frameとopaqueなpublished pairを持つ。同じsurfaceの全resource/pageは同一account・同一pairで受理した場合だけatomic表示する。新しい現在値の保存・公開中も、同じ所有資格に属する確認済み履歴はその元pair、期間、値、確認時刻を保持して公開する。履歴の更新待ちは最新範囲の未確認として明示し、旧履歴へ新しい現在値のpairを付けず、未確認の点・期間・価格を生成しない。Graphの履歴組へMainのcurrentをjoinせず、両者のpair一致を要求しない。旧detailsは一つの確認済み完全frameを返し、最新currentと旧履歴のfieldを混ぜない。accountを選ぶためのglobal catalog（`/v3/accounts`）は選択accountのdata bundleへjoinせず、WIREのdefault/boundary frameに属するpairで独立受理する。catalogのdefault accountと明示選択した歴史accountが異なる場合にpair一致を要求せず、catalog取得で選択accountのdata pairを置換しない。health/runtimeもdata bundleへjoinしない。履歴差分だけは、直前rootのcursorが同じperiodの既取得prefix不変を証明した場合に限り、新pairの全pageを揃えて旧prefixへatomic appendできる。prefixのsampleまたはgapが補正された場合はcursorを拒否し、先頭から再取得する。modelは固定3列でなく有界な配列として公開し、未知モデルのtoken事実と価格未確定を区別する。旧v1/v2と`/v3/details`は互換期間中だけ同一snapshotのdeprecated projectionとして保持し、旧API削除はadapter・route・client fallbackだけで完結させ、記録、DB schema、常駐監視、安定health endpointを変更しない。既存互換routeは期限なしで維持し、自動Sunset・自動削除を行わない。
履歴の読取り中に現在値のcommitが進んでも、元の受理済みackと同じread transactionで資格を得た履歴は、同じaccount・partition・owner・collector epochが維持される限り独立して公開できる。その履歴自身のcommitと確認時刻を保持し、既公開履歴の世代と確認時刻を後退させず、現在値を置換しない。最新現在値へ追いつくまで`refreshing`とし、未知の期間や点を合成しない。元ack不一致、所有境界、同一世代の保存値改変は受理しない。

REST/DBの公開応答に含めるモデル数値は、同じmodel keyを持つ直接観測(`model_source=confirmed`)のraw値、または保存済みの同じmodel keyをlosslessに保持する`legacy-unknown`値だけとする。`reconstructed-from-session`、`unknown`、`unavailable`はモデル数値を公開せず、model key/sourceと欠損metadataだけを公開する。`legacy-unknown`は集計・予測のauthorityにはせず、`G137-5`の同一非空model集合・全lossless raw tokenとraw Remainingのexact不変・欠測／異常／confirmed gapなしをrun全体で満たす場合だけidle判定に利用できる。補間、hold、smoothing、予測およびそれらの派生値はUI presentation-onlyであり、API/DBへ書き戻さない。

ここで`confirmed`を決めるのは復元した日時や処理名ではなく、DATA `RECORDER-MODEL-01`の原本・数値・model・連続所有区間・公式periodの全採用資格である。同じqualified eventの正確なprefix和を保存する通常収集と、`HISTORY-RECOVERY-614`で同じ資格を原本から回復した履歴は、この資格に関して同じ結果にする。原本の消費とmodelを一意に証明したprefix和は`confirmed`として公開でき、集合の取り漏れや他sourceの未解決は`models_complete=false`で独立して保持する。復元を実行したという事実だけで`confirmed`へ昇格せず、逆に全資格を得た記録分を復元処理由来という理由だけで数値非公開にしない。最後のmodel名や似たtokenから割当てた未資格の再構成は`reconstructed-from-session`のままであり、数値公開・集計に使わない。旧`legacy-unknown`値のschema移行・再読込みではsourceを変更せず、新しい原本資格を得た場合のprovenance更新も同じbackup付き修復transactionへ含める。これは保存key、既知数値、保存価格を推測変更する許可ではない。
旧v1/v2の固定family名・固定family数値だけは、`MODEL-PRICING-463`の凍結対応に従うversion別のread-only互換projectionである。これはcanonicalのmodel keyを別keyの直接観測へ変換する例外ではなく、旧形式が要求する表示bucketへの写像である。元のkey・token・sourceはDB/v3以後で保持する。完全性不明の記録分を完全なfamily利用量へ昇格させず、未知成分・未知額の0補完をせず、互換projectionのknownness/source/表現不能はWIREの版別規則に従う。
`MODEL-USAGE-DISPLAY-01`の部分額非表示、およびDirect以外を集計authorityにしない規則に対し、Windows Graphの`G137-7`期間注記だけは、保存済みの有効な部分額を利用相当量として表示上合算できる。確定した期間合計やmodel groupの確定額とはせず、DB/API・モデル線・idle・予測のauthorityへ反映しない。Windowsの月表示でもmodel keyと価格の対応は維持し、価格未知のmemberを含む日のfamily金額を既知部分や0で表示しない。日別の記録分の意味は`G137-GRAPH-01`に従う。
`ACCOUNT-LIFECYCLE-134`: 一つのCodex profileは同時に一つの`auth.json` authorityだけを持つため、resident recorderが同時に書き込むのは現在認証を再確認できた一つのaccount partitionだけとする。認証されていないaccountを並行取得・推測更新しない。認証済みになった全accountのpartitionは削除せず、切替後も保持し、同一accountへ戻った場合は同じpartitionを境界後から再開する。REST/UIは初期化済みpartitionを列挙し、起動ごとに検証済み`/v3/accounts`の現accountを表示対象にする。現accountを確認できない新規起動では履歴accountを推測選択しない。起動後は利用者が設定画面で一つだけ選択して読み出せる。一覧に残る明示選択は同一processの一覧更新で保持し、次回起動の既定には保存しない。完全に受理した一覧から明示選択IDが消えた場合は、その選択を解除して未選択とし、別のdefaultや履歴IDへ自動変更しない。一覧取得の失敗・途中pageはID消失の証拠にしない。消失したIDが再掲載されても自動再選択せず、利用者の次の選択またはprocess再起動まで未選択を保持する。選択変更はcurrent、history、threads、pair、cursor、last-goodを同じaccount境界で一括破棄・再取得し、複数partitionの値を一画面へ混合しない。
一つの外部観測可能な契約IDを複数ownerへ置かず、下流文書、要求台帳、実装、testは登録済みownerの
契約IDを参照する。監査履歴、作業経過、文書SHA一覧、test名やagent運用は製品要件ではない。

| owner ID | 唯一のowner | 所有する契約境界 |
| --- | --- | --- |
| `PRODUCT` | `docs/PRODUCT_REQUIREMENTS.md` | 製品境界、cross-component状態、収集、導入・更新・配布 |
| `WIRE` | `docs/REST_API_V1.md` | REST route、schema、header、resource limit、read-only effect |
| `DATA` | `docs/DATA_PROTECTION_POLICY.md` | 保存scope、transaction、backup、retention、recovery |
| `UX` | `docs/WINDOWS_UX_SPEC.md` | 表示意味、画面状態、操作、geometry、accessibility |
| `I18N` | `docs/LOCALIZATION.md` | locale、固定文言、時刻、font |
| `SECURITY` | `SECURITY.md` | trust boundary、秘密、入力検証、残余risk |

利用枠の取得原本保存・再処理の契約はDATA owner `QUOTA-DURABLE-604`へ委譲する。公式応答を解釈・期間判定・Session集計より先に保存し、後段の失敗による原本消失を防ぐ。過去Session原本に残るcached windowは、同DATA ownerのsource・account検証と全候補の一致条件を満たす場合だけ、取得済みの同じ公式resetの期間長を補うために使える。snapshot単独から新しいperiodやquota観測点を作らない。最初の保存時刻から推測せず、現在quota・元のmodel値・receiptの取得順を変えない。

通常の期間利用枠では、公式応答が長さとリセット日時を示した窓のうち、最も長い窓を表示・記録対象にする。これは短期枠より週枠を表示するための製品側の選択であり、公式APIがprimary/secondaryの優先順位を保証するという意味ではない。同じ長さなら後のリセット日時、両者も同じならprimaryを選ぶ。公式型が許すnullの期間長・リセット日時を持つ窓は選択対象外とし、別の完全に報告された窓を拒否する理由にしない。選択できる窓がない場合は期間や残量を推測せず、取得原本を保持して解釈不能として扱う。

owner文書が他領域の契約を必要とする場合は、その契約を複製せずowner IDを参照する。
`DESIGN.md`、README、Wiki、runbook、Windows説明文書、回帰規約、要求台帳は派生物でありownerではない。
同じ契約の複数owner、未登録owner、参照先のない契約IDを検出した場合は実装・高コスト検証を開始しない。

## 実装判断の基準と外部契約（API-LIFECYCLE-01 / LIVE-RESET-129 / #604）

この節は収集→保存→公開→全画面を結ぶ仕様の入口である。実装中の未確定点はowner仕様へ戻して解消する。テストの既存期待値、現行コードの定数、見た目が改善することを正しさの根拠にしない。以下のowner参照、条件、失敗結果が実装前に揃っている必要がある。実装のクラス分割やデータ構造そのものは正本に固定しない。

### 外部から与えられる事実と製品が決める規則

| 外部契約・出典 | 変更できない入力の意味 | 本製品の採用・不明時の規則 |
| --- | --- | --- |
| [Codex App Server](https://learn.chatgpt.com/docs/app-server)、使用CLIが生成するversion別JSON schema | `account/read`の認証主体、`account/rateLimits/read`の報告値。primary/secondaryは必ず週窓になるという保証を持たない | account前後確認、窓選択は本書、原本保全と取得順はDATA。rate limit resetを消費するcontrolは監視目的で呼ばない |
| 同CLI schemaのquota window | `usedPercent`、nullableな`resetsAt`・`windowDurationMins`。0.159.2ではusedPercentは整数、残り2値はnullable整数 | 長さが正、残量が0..100、公開時刻範囲内でchecked演算可能な完全windowだけを選ぶ。期間長は分×60。null/未提供を7日・残量100・初回利用時刻へ置換しない。選べない原本は保持し解釈失敗を独立表示 |
| 同CLI schemaの`SpendControlLimitSnapshot` / `individualLimit` | 0.159.2の生成schemaには`limit,remainingPercent,resetsAt,used`があり、期間長・開始時刻・暦月の宣言がない | plan名またはresetだけからUTC暦月・31日・7日の期間を生成しない。quota windowの選択と混ぜず、下記の完全window規則を使う。rawの報告値は取得原本へ保持する |
| `rateLimits`と`rateLimitsByLimitId` | 後者は任意のbucket集合。metered limitのIDはwindowのresetとは別の識別子 | 現行製品の対象は互換`rateLimits` bucketである。異なるbucketを合算・推測選択しない。`rateLimits`が解釈不能でも別bucketを黙って代用しない。未知bucketは原本に保持する |
| 同CLI schemaのthread/token通知と`thread/read` | tokenはthread/turnの観測、context windowとstateは個別情報。notLoaded/systemErrorはidleを表さない。保存threadの可読性と現在の認証accountは別の事実で、0.159.2のThreadにaccount owner fieldはない | `THREAD-OPEN-362` / `THREAD-CONTEXT-362`、Session版別parserとDATA source検証を使う。`account/read`とID/pathの一致だけで過去threadの消費所属を決めず、DATAの元event時刻・記録済み所有区間・source結合を独立検証する。不足は未確定として保持する。account全体のusageや他端末利用量をローカルmodel消費へ逆算しない |
| [JSON Schema default](https://json-schema.org/understanding-json-schema/reference/annotations) | default annotationは欠落fieldを実測値に変えない | cache writeの欠落/nullはDATAの入力表どおり不明とする。default annotationや記載のないproducer版別normalizationで実測0へ変えない。明示的な数値0だけを既知0として採用する |
| [SQLite WAL snapshot isolation](https://www.sqlite.org/isolation.html) | 一つのread transactionはcommit済みの一貫したviewを見る。別HTTP要求まで自動的に同じviewにはならない | 同じtransactionからimmutable publicationを作る。HTTPはそれだけを読む。全pageのsame-pair admissionはWIREが所有する |
| OSのUTC clockとtimezone database、[TimeZoneInfo](https://learn.microsoft.com/en-us/dotnet/api/system.timezoneinfo.converttimefromutc?view=net-10.0) | 時計の正確さ、夏時間の遷移は外部条件。local日付のUTC長は一定ではない | 保持はDATAのUTC暦月、月表示は各UTC観測を選択timezoneの日付へ写像する。24時間固定長で暦日を切らない。quota取得順はclockから決めない |

0.159.2は上記nullable型を確認したCLI版であり、過去全ログのproducer版だという主張ではない。実行に選ばれたCLIの生成schemaと、各原本の認識済みformatを区別する。JSONLはファイル名だけでRPC schemaと同一視しない。未知format・型・component欠落は原本と理由を保持し、既知部分の採用資格をDATAの規則で判定する。外部仕様が変化した場合も推測adapterを足さず、この採否定義を先に改訂する。

quotaの有限window選択はplan名によらず同じ完全なprimary/secondary候補から行い、最長窓・同長なら後reset・同値ならprimaryを選ぶ。採用するRemaining、exact reset、期間長は全てその一つのwindowに由来し、`individualLimit`の残量と別windowの期間長を結合しない。`individualLimit`だけ、または完全windowがない入力は有限periodとして解釈できず、原本を保持して取得解釈失敗とし、同accountの最後の受理済みquotaと障害を公開する。初回なら既知期間・100%・0利用を生成しない。期間長のない値を暦月へ置換する既存実装や、enterpriseというplan名は期間の正本ではない。providerが暦月のlimitを持つ可能性を否定する規則ではなく、報告されていない期間長を本製品が生成しない規則である。Graphの月カレンダーは選択timezoneの日付による表示であり、このquota windowの長さとは独立する。

### RECORDED-USAGE-SCOPE-604 — 原本集計と既存UI仕様の保持

本節は、原本から一意なlocal消費を復元・照合する内部集計の境界を定める。ユーザーが要求していない「記録範囲」UIへの置換、Mainの期間を直近W秒へ変更すること、週期間一覧をrolling範囲とUTC月一覧へ置換することは行わない。2026-10-18のユーザー指示「UI仕様を勝手に変える権利は有りません」「記録範囲なるものは要求に存在しない」に従い、これらの置換仕様を撤回する。

- Mainは現在の公開period/windowの検証済みモデル値と、`MODEL-PRICING-463`のAPI相当参考額を表示する。公式quotaとlocalモデル消費のauthorityは独立である。providerが不変の実利用週IDを返さないことだけを理由に、既知のモデル値・参考額を空または常時不明へ変更しない。
- Graphは従来の週期間、24時間、1週間、1か月のモード、日付group内の各期間選択、前後移動、表示timezoneによる暦月を維持する。週期間は元のexact保存keyとorigin metadataで選択し、報告resetをproviderが保証する不変週IDと表明しない。異なるkeyの数値を無根拠に合算・付け替えしない。
- 初回利用前にreset予定だけが動く場合も、原本・exact保存keyを保持する。selectableの掲載条件はWIREの現在period、正の公開model値、または未解消の旧利用記録を持つperiodである。過去のquotaだけの空項目を増殖させず、履歴の正値や未解消記録を消さない。日付groupは表示上の分類であり保存keyの統合ではない。
- 正常な同period・同metricの連続した観測runは既存`G137-GRAPH-01`に従う実線とPCHIPで表示する。疎なtimestamp、期間identity metadataの不明、補助集計の存在だけを理由に、全観測を点へ変更したり階段へ強制したりしない。実際の欠測、非所有、reset境界、数値異常は既存の分割・破線規則を適用し、復元できる原本から先に回復する。描画補間を実測としてDB/APIへ書き戻さない。
- 月表示は選択timezoneの暦月・暦日を使い、元時刻と資格を満たす同一消費を一度だけ加算する。モデル系列のON/OFFで月合計を変えない。未計測と既知0を区別し、正の利用記録がない日の表示は既存月仕様どおり控えめにする。未知値を0へ変換しない。

内部のqualified factは、DATAのsource/account結合・検証済みprefix・数値・exact model・連続所有区間を満たした一つのlocal消費である。同じresponseの全5 ID再掲、またはDATAの完全な同physical/generation越し対応が証明された同じrecordは一度だけ数える。別bytes原本への対応は全旧event一対一条件を必要とする。既知factの元秒とlossless成分は保持し、報告Rの更新だけでは消費を削除・再加算しない。未資格の保存値、重複未解決、未処理suffixはfactへ昇格しない。保存済み旧値は元key・元時刻・価格bitsと未解決理由を保持する。

参考額の算出資格と、過去の保存額の同一性を区別する。既知token成分と正確なmodel IDには`MODEL-PRICING-463`の検証済みcatalogを適用でき、providerの期間identityや新しい価格永続schemaを前提にしない。保存済み履歴額は同契約の保持・復元規則に従い、最新価格で上書きしない。必要成分不足・矛盾・未知model・単価欠落の場合だけ対象額を不明とし、他の既知額へ波及させない。

補助的な明示時間範囲の集計は、accountとUTC half-open範囲`[start_at,end_at)`に元消費秒が入る一意なqualified factの成分和である。補助APIの既定rolling範囲は、最新受理quotaの窓長Wと、同accountの最新quota元秒・qualified fact元秒の最大Tから`[T+1-W,T+1)`とする。これは内部・補助resourceの計算であり、MainまたはGraphの既存期間を置換する根拠ではない。完備性不足・成分不明・未解決重複は対象ごとに保持する。異なるRの保存累計の和・最大・差だけをlocal消費や月合計の代用にしない。

current・directory・historyの公開は、従来の各値の資格と版別schemaに従う。期間identityのmetadataが不明であることだけを理由に、既存routeを一律503にしたり、全modelを空にしたりしない。clientは既存のv4 current、期間directoryとv3 historyを使い、exact404だけの互換取得、同account・同published pairのatomic受理を維持する。正当な対象別の不明と、実装されていない計算・無断変更による非表示を混同しない。

受入では、原本の独立数値照合に加え、承認済みUIの価格、週期間と日付group、timezone暦月、正常runの連続線、モデルON/OFFに依存しない月合計を実画面で照合する。要求外のUIへ合わせたtest期待値や文書追記を受入根拠にしない。

quota応答の型検証と、有限window候補の数値資格を分ける。入力のenvelopeと採用するschemaにある既知fieldの提供値はその版の型を満たす必要があり、型不正は入力全体を解釈失敗にする。欠落/nullを許す期間metadataは型不正ではない。数値候補は、usedPercentが0..100、reset Rが`1..253402300799`、正の整数分からchecked演算で得たWとorigin `R-W`が同じ公開時刻範囲内であるものだけとする。型は有効でも数値資格のないwindow、またはmetadataが欠落/nullのwindowは候補から外し、別の有効な完全windowを妨げない。527,040分などの実装上の既存上限を、外部durationの正当性へ流用しない。公開範囲とchecked演算を満たす、より長い報告windowも同じ規則で採用する。未知の追加fieldは原本に保持し、他のquota・model・金額の入力へ昇格しない。

「Remainingが100の間にreset予定が動く」は観測された報告値として受理するが、実消費0やproviderの恒久保証とは表明しない。同じexact R/Wは同じ報告枠の保存key、異なるR/Wは1秒差・前後方向・同一取得秒でも別の保存keyとして保持する。このkeyは実利用期間の外部identityではない。保持・表示・輸送の都合でraw keyを丸めず、実利用期間と記録分の意味はRECORDED-USAGE-SCOPE-604に従う。

公式の[rate limit field定義](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt)は現在の使用率と次回resetを報告するが、同一reset内の使用率の単調性を保証しない。このため、受理済みの0..100のRemainingが増加・反転したという形だけでは異常値として棄却しない。新しい報告枠の保存keyかどうかはexact R/Wで区別するが、新しい実利用期間かどうかをこのkeyやtoken量・料金・回復率から推測しない。これは累積event deltaの非負和として本製品が管理するmodel token/USDとは異なる外部authorityである。

accountの記録上の所有境界はDATA8.6「記録上のaccount所有区間と未確認境界」を正本とする。開始不明を全時刻所有へ変換せず、初回・再起動・切替・logout・主体不明・RPCだけの失敗を同節の別条件で扱う。設定されたactivation値や現時刻によって過去の所属を生成しない。

### 収集・公開の依存順序と失敗境界

```text
認証主体確認 → quota原本durable化 → 解釈・exact reset受理 → 最新取得順のquota
Session原本 + 所有区間 → 検証・event/prefix → exact reset/minuteのmodel
完全なopen Session集合 + thread/read → 公開membership・状態
上記のcommit済み値 + DATAの公開基準時刻 → immutable publication
publication → WIREのcurrent / directory / history pages / threads
必要resourceのsame-pair受理 → UXのsurface単位のatomic表示
```

quota原本はSession laneの成功を待たず、Session laneもremote quota障害を理由に停止しない。modelとquotaのminute/resetが一致しなければ別々の事実として保持する。periodの取得件数と掲載件数は入力の妥当性・未使用の証拠ではない。輸送失敗は該当surfaceに隔離し、真正のdomain投影異常でも有効な別laneの値を消さない。account境界は旧account値を消す既存の唯一の例外である。

### グラフの観測・欠測・派生値の判定順序

`G137-GRAPH-01`の前処理は、account/period所属検証→原本復元→model/metric別の数値検証→同値・欠測・異常runの分割→idle判定→描画geometry→日別表示、の順である。quota、token、USDの既知性は独立する。

| 状態・根拠 | 表示・計算への結果 | 異常調査の責務 |
| --- | --- | --- |
| exact時刻の有効な観測 | 元の値をanchorにする。隣接実測間の滑らかな線は実測を結ぶ表示処理 | 正常。線上の任意中間時刻まで観測済みと主張しない |
| 明示null、model/metric欠落、confirmed gap | 既知両端の間は破線、右端不明は最後の既知値の破線hold。原本があれば復元を先行する | 原本ありならbackend復元/公開の不良、原本なしなら記録欠損として期間別説明 |
| 累積model metricのcounter後退・isolated pulse | `G137-3`のRejected/recoveryを適用。別model/metricの値を消さない | 原本、DB、projectionを比較して壊れた層を特定。表示だけ正常化したと報告しない |
| period開始の時刻だけ既知、初回値不明 | 値のない先頭区間。0/100%の線を作らない | 回収可能な原本から復元。証拠のない以前の値は不明 |
| 終端と最終実測が同じminute | 最終実測で線を止める。端数秒へ予測値を作らない | サンプリング粒度の差。次minute以降の欠測とは区別 |
| 全非空token vectorとraw Remainingがexact不変 | `G137-5`の全条件と10分以上を満たす場合だけ未使用帯 | task停止だけ、丸め後の同値だけ、途中のtoken増加/減少は不可 |
| 同じtokenに対する保存USDの変化/欠落 | USD自身のaccepted値と欠測を保持し、tokenから額を作らない | 価格版・保存額・原本prefixを調査。理由を推定した表示補正で隠さない |

破線は実測にない値を表示処理でつないだ箇所を示す。破線の本数だけでdaemon停止を断定しないが、回復可能な原本があるのに破線が残る、または正常原本から誤った破線を生成するならbackend/表示の不良であり修正対象である。現期間の回復可能な不良を既知制約として残してPRしない。

月カレンダーは`G137-GRAPH-01`の日別差分規則を使う。同じaccount/period/model/metricの有効な累積差だけを一度加算し、resetを跨いで引き算しない。完全coverageの範囲では日別合計は同じ境界間の累積差と一致する。不完全coverageでは既知の日内差分だけを「記録分」とし、月合計を期間累計全体と同じ意味で表示しない。日跨ぎの不明差を按分しない。月も同じaccepted系列を使い、棄却した後退値を新baselineにしない。原本復元でaccepted履歴が訂正された場合だけ全必要pageから再配賦し、未知の差を0として一致させない。timezone変更時は同じUTC観測を再分類し、DB値は変えない。

### 画面全体の説明責務と受入対象

全表示には「原本で確認した値」「正本の式から得た値」「明示した未知/障害」「表示専用の加工」のいずれか一つの説明が必要である。後二者を実測、未使用、成功へ読み替えない。

| 対象 | 唯一の意味・操作owner | 必須の説明 |
| --- | --- | --- |
| Main | 本書の`MODEL-USAGE-DISPLAY-01`、`THREAD-OPEN-362`、UX Main | quotaとmodelの別authority、取得時刻、model部分/不明、同じmembershipの件数、失敗時保持、0と未知の区別 |
| Graphの期間・24h・7d・月、token/USD | `G137-GRAPH-01`、`HISTORY-RECOVERY-614`、WIRE page admission、UX Graph | 各点のexact period/modelと価格根拠、初期anchorの資格、全破線・帯・日別値と範囲、全期間の取漏れなし |
| Threads | `THREAD-TITLE/CONTEXT/OPEN-362`、UX Threads | 実title/state/context、Mainと同じ受理membership、未観測を停止中にしない、treeと件数の一致 |
| Settings | `ACCOUNT-LIFECYCLE-134`、UX Settings | 即時account選択と保存draftの違い、接続/認証/実versionの別、Save/Cancel/再確認の実効果 |
| Setup | UX Setup、WIREのreadiness、SECURITY | listener/health/control成功とstrict current受理の別、接続失敗時の保持と再試行 |
| Legal・共通window操作 | UX Legal / `WIN-WINDOW-BOUNDARY-603`、I18N | 許諾文と全入口、Close/最小化/移動/scroll/focus、孤立・無反応の操作なし |

Linuxに実装された対応surfaceにも同じデータの意味と取得失敗の隔離を適用する。WindowsだけのSetup/LegalをLinuxに新設する意味ではない。Graphだけ正しくても全画面受入にはならない。

現DBにある全対象account/periodを最初に照合する。矛盾候補はaccount/window/source/metricで分類し、復元可能なものはDATAと`HISTORY-RECOVERY-614`に従い復元する。残る破線・先頭欠落・帯・日別欠損は、取得原本の有無、未記録、回帰、confirmed gap、表示専用hold/補間のどれが根拠かを期間別に列挙する。破線自体を根拠にprovider利用量やローカル未使用を推測しない。新しい記録に収集・公開・表示の不良が残る状態では受入・PR不可とする。既存ログにない他端末のmodel利用を捏造して完全表示にしない。

仕様段階の完了条件は、上記の全対象について入力、採否、計算、状態遷移、失敗、互換、定数scope、owner参照が一意で矛盾しないことである。実装完了の証拠とは別であり、文書だけで全画面や現DBが正しくなったとは報告しない。実装時に確証がない条件を発見したら実装を止め、正本の該当条件を解消してから再開する。

未提供のtoken成分、未知価格、所有を証明できないsource、存在しない過去観測は入力の不明であり、仕様の保留ではない。それぞれの採否・既知値の保持・不明表示・復元条件をownerが定める。実装結果が分かれる条件を「後で決める」として残すこと、実装者や将来のtestに正解の決定を委ねることは禁止する。必要な条件は今回のowner仕様内で解消し、別Issue、将来の評価、実装時の選択へ移さない。今回要求された全対象の必要仕様が確定するまで仕様段階を完了せず、製品コード・実行時fixtureの変更へ進まない。「既存」「現行」「通常」という記述も、参照するowner条項から入力と結果が一意に決まる場合だけ使用する。コードや過去のtestを読んで動作を補う必要があれば未確定である。

## 1. 製品境界

- WindowsクライアントはLinux側の利用状況、残量、履歴、実行中threadを読み取り表示する。
- 製品の通信は固定loopback API、利用者が選択したWSL、またはOpenSSH configのliteral `Host` aliasに限定する。
- password、token、private key、展開済みSSH接続情報を保存しない。保存できるのは再接続に必要な非秘密selectorだけである。
- telemetryは送信しない。更新確認とLinux bundleの更新に必要な読み取りだけは固定GitHub repository
  `salty919/codex_info_v2` の公開Release API/assetへ行える。これ以外の未登録の外向き通信、
  暗黙のsupport upload、raw diagnostic uploadを行わない。

## 2. 状態と表示

- Setup、未認証、通常、警告、取得失敗を区別する。取得失敗を未認証や正常emptyへ変換しない。
- 可視surfaceに必要なresource/pageの取得途中、schema不正、account/pair不一致のcandidateは一部だけ表示せず、そのsurfaceの最後の受理済み表示を保持する。必要なbundleの取得・検証完了と、記録の完備性は別である。schema-validな`models_complete=false`やv4 currentの`models_status=partial/unavailable`を、それだけで未完の取得candidateとして棄却しない。採否は`API-LIFECYCLE-01`とWIREのsurface admissionへ従う。
- 同じ事実の表示所有者は1か所とし、残量、reset countdown、status、connection情報を言い換えて重複表示しない。
- quota再取得の処理中に、旧model・履歴・threadを空の初期値へ戻さない。受理済みquotaの保存・公開は`QUOTA-DURABLE-604`の独立laneで進め、Session収集や全modelの完備を待つ条件を追加しない。readerが同じimmutable frameから構成し、clientが必要なsame-pair bundleを受理した時点で、各値とpartial/unavailable/失敗状態を一括表示する。新resetへ旧resetのmodelを付け替えず、新periodの未取得を確定0にしない。認証主体変更・明示logoutのstrict boundaryを除き、取得途中・失敗という理由だけで主画面全体を空にしない。
- 最小viewport、対応locale、keyboard、UIA、高contrast、text scaleで主要操作を失わない。root scrollで欠落を隠さない。
- Windows Settingsの2タブ、項目のまとまり、固定footerとSave/CancelはUX ownerの`WIN-SETTINGS-TABS-590`に従う。各Windowの共通デザイン・アイコンと外周、接続状態の更新結果、Setup/Settings間の接続案内はそれぞれ`WIN-WINDOW-BOUNDARY-603`、`WIN-SETTINGS-REFRESH-603`、`WIN-CONNECTION-GUIDANCE-603`に従う。
- Back、Close、Escape、再入、遅延callbackは世代tokenで一度だけ処理する。古いPID/HWND/generationへfocus、message、route変更を行わない。
- 顧客向けLinux入口はinstalled launcher `$HOME/.local/bin/codex-info`一つとする。引数なし/`--start`は更新収束後にmanaged recorder/RESTを有効化・起動する。`--ui`は同じ収束後の検証済みinstalled payloadを実行し、初期windowの表示とevent loop開始を確認したら呼出元へ戻り、UIは独立して稼働する。初期化失敗・確認期限超過は非0で返す。local generation/payload identityが完全だがservice/details接続だけが失敗した場合もX UIの失敗状態と再試行手段を表示する。generation不整合またはforeign/unknown ownerの`SAFE_BLOCKED`ではpayload UIを実行しない。`--stop`は同一bootだけ有効な停止意図を記録してrecorder/REST serviceだけを停止し、timerと次回bootの自動起動を維持する。`--disable-autostart`はrecorder/REST serviceとupdate timerを停止・無効化し、`--remove`はrecorder/REST serviceとupdate service/timerの4 unitを解除する。`--status`はread-onlyの完全identityを表示し整合時だけ成功、`--update`は同じ更新authorityを即時実行、`--help`は副作用なしとする。
- 明示的に導入したローカル復旧世代は、manifestのproducerをActions実行と偽らず、`LOCAL_RECOVERY.json`のdigest・元source SHA・source差分digestを全member検証へ結合する。公開Release経路はlocal producerを受理しない。完全検証済みの復旧世代に対する明示`--start`／`--ui`は、Release探索・置換を前提とせず既存managed ownerへ接続する。更新timerの無効化で外部unit登録が外れていても、世代内timer payload・UI・recorder・RESTの検証とは区別する。外部timer登録がある場合のcanonical link、稼働owner・listenerの検査を維持し、復旧世代の導入・起動は既存timerの無効／停止状態を保持する。
- installed launcherは未知・混在・重複引数と`--port`をmutation前に拒否する。service・開発・E2Eが直接使うpayload `codex_info`のCLIは、引数なし/`--port PORT`のdaemon+REST、`--ui`/`--ui --port PORT`、検証済みownerだけを停止する`--stop`、`--help`/`--h`/`-h`に限定し、待受アドレスを常に127.0.0.1へ固定する。payloadの`--stop`はSIGKILLへ昇格せず、lock identityを証明できない場合は何もsignal・削除しない。
- repositoryの`run.sh`はarchive内の`run.sh`とbyte-identicalなruntime launcherであり、installed launcherへ委譲するだけとする。Cargo build、`target/`、source tree、別binaryへのfallbackを行わない。
- CLIヘルプを含む利用者向け固定メッセージは、画面本体と同じ対応localeのi18nカタログから導出する。起動スクリプトへ単一言語の製品文言を複製しない。

### 2.1 実行中Thread詳細の共通契約（Issue #362）

- Linux/WindowsのThreads詳細のgeometry、tree、色、lane配置はUX ownerの`X-THREAD-01`へ委譲する。両platformは同じWindows masterの表示契約を使用し、製品要件ではgeometryを再定義しない。
- `THREAD-TITLE-362`: `thread/read`で取得する上流保存名を表示する。生成元がtask_nameを外部の`thread/name/set`で保存している場合はその保存値をそのまま使い、空または未設定なら`未設定`を表示する。汎用状態名、preview値、agent_path、nickname、body、その他の推測値を表示名へ使わない。この製品はobserverであり、外部のsetter mutationを呼ばず、保存済み名称の取得と表示だけを行う。
- `THREAD-CONTEXT-362`: 同じthreadで観測したusage/window pairだけを表示する。usage=0かつ正のwindowは0%として表示し、pair欠落、windowが0以下、旧checkpointのNULLは`未観測`とする。NULLを0へ変換せず、累積値や別sourceからcontextを合成しない。保存・再起動・appendの境界はDATA ownerの`SESSION-129`/`DB-129`に従う。
- `THREAD-OPEN-362`: Mainの「生成済みスレッド」は現在Codex processがopenしているcanonical Session pathに結び付くthread IDのうち、公開対象となった集合とし、同一IDを1件と数え、Session終了後は除外する。完全なopen-Session collection cycleと全候補の`thread/read`・rollout取得が成功した後にだけ、nonemptyな`parent_thread_id`が候補ID集合に存在しない行とその子孫、および停止中の親なし行と、停止中の親を持つ子とその子孫を公開membership・件数から除外する。親の判定は除外前の完全な候補集合で行い、子孫自身が動作中でも除外する。parent IDが空または不明で欠落を証明できない行は保持し、parent IDが全て候補集合に存在する閉じたcycleもこのmembership filterの局所除外では救済しない。DATAのnative構造検証とWIREの公開thread集合の非循環条件で、表示filter前に候補全体を拒否する。親が動作中なら、その子は動作中も停止中も公開対象とする。停止中の親なし行は公開対象にしない。「未観測」の親を停止中とみなして子孫を除外しない。page、candidate、rolloutの収集失敗または不完全なcycleでは部分集合を公開せず、前回の完全snapshotを保持する。Mainの合計とmodel内訳は同じ公開対象集合から導出する。v3のmodel別表示は同じ公開対象行のうち動作中だけを先の数、公開対象open全件を括弧内の数とし、openが0件なら括弧を表示しない。詳細は各IDの`thread/read.status`がactiveなら「動作中」、idleなら「停止中」、notLoadedなら検証済みrolloutの最終taskイベントが明示する状態だけを用い、systemErrorまたは根拠欠落なら「未観測」とする。未観測を停止中・動作中へ推測しない。旧`active_thread_count`とv1/v2のactive rowsは公開対象行の動作中だけを維持し、v3のopen集合・状態とは区別する。旧APIにはopen全件の根拠がないため、model別の括弧内件数を表示しない。

- `THREAD-OPEN-362`の画面受理では、同じ公開pairのwire行数と動作中行数を先に照合し、その受理済み行集合から停止中の親なし行と、停止中の親を持つ子とその子孫を表示前にも除外する。動作中の親の子は、動作中も停止中も表示する。Linux/WindowsのMain生成済み合計・model別件数とThreadsの行・treeは同じ表示対象集合を使う。残す行のIDと`parent_thread_id`を変更せず、子を別の親へ付け替えたりrootへ昇格したりしない。wireの元件数は再取得の同一pair照合に保持し、画面上の件数として使わない。

## 3. 収集・API・live判定

`REST-172`の現行current公開では、取得・読取り・公開の成否と、Sessionモデル集合の完全性を別の事実として扱う。取得成功と部分集計は両立し、未完行・未確定anchor・未処理の過去ログだけでは取得エラーにしない。完全性が未確認の空集合を確定したゼロへ変換せず、最新の取得済みモデルだけを公開する。実際の取得失敗、破損したモデル値、読取り・公開失敗は引き続き障害として扱い、独立して受理済みの利用枠を取り消さない。recorderの取得状態と、REST reader/publisherによる応答可用性の失敗は以下の境界で分離する。現在値の新commitは、`API-LIFECYCLE-01`の確認済み履歴frameを失効させない。旧履歴の資格と確認時刻を保持し、履歴の新commitが独立して検証できた時にだけ履歴組を置換する。新旧を混合せず、account・partition・所有区間・recorder owner/epochの境界は保持資格を取り消す。この区別を伝えるcurrent v4のwireと旧世代の互換境界はWIRE owner `API-V3-MODELS-01`に従う。旧世代の「不完全な集計をerrorで表す」テストをcurrent v4の正常性判定へ流用しない。

- `REST-172`: 同一profileのresident serviceをquota、local usage、historyの唯一のauthority/writerとする。service内のrecorderとREST publisherは同じ有効owner、lease、epoch、cycleに従い、旧世代の結果を公開しない。recorder死亡監視は既存周期を維持し、REST snapshotの検証・互換JSON生成・published pair更新は、公開domain stateが実際に変化したworker event、recorder試行結果、または60秒周期の鮮度確認があるcycleだけで行う。入力通知も鮮度・収集deadlineの到来もない不変なtick、および同じcanonical Thread結果だけではDB、Session、履歴走査、JSON再生成を行わず、前回のimmutable snapshotを再利用する。予定された新しいcollection cycleのcommitと世代前進はDATA、60秒の公開鮮度照合はWIREに従い、このtickの省略条件に含めない。wait要求は既存probeの完了を待ち、次の60秒周期前に同一epochのvalid resultだけをcommitする。ackは既存のpublication policyに従い、quota-not-readyをready扱いせず、has_pendingとDegradedを保持する。履歴sourceの未処理rangeと現在の取得laneの失敗は独立した事実として保持し、過去の未処理rangeだけを理由に取得・保存済みの正常なcurrent/detailsを`state=error`へ上書きしない。未処理rangeの削除や完了扱いによって取得状態を回復させない。recorder内の現在の取得失敗・DB読取り失敗はlast-goodを保持した取得状態errorとし、同じusage generation内の取得回復も応答状態へ反映する。このerrorは有効なowner/epoch/cycleの完全candidateとしてadmissionされたときだけ公開できる。standalone REST reader/publisherの通常のDB I/O失敗では、同じ所有資格の確認済みlast-goodがあれば既知値を保持したHTTP 200・degraded表示を返す。degraded pairは正常pairと別の決定的な値とし、同じframeの各snapshot resourceで共通に使う。同じDB generationで回復しても通常pairのHTTP 200へ戻り、degraded ETagで回復を304に隠さない。確認済みframeがない場合、所有資格の失効、domain/source検証拒否、またはpublisherの継続不能は503 `snapshot_unavailable`とする。HTTPはpublisherが捕捉したframeと可用性だけを投影し、DB再読取りやrecorderのError generation生成を行わない。履歴だけの投影失敗は独立したv4 currentへ波及させず、WIREの確認済み履歴保持または`history_unavailable`として扱う。同一accountで起動後のquotaが未判定でも、同じ有効owner・lease・epochを検証したThread snapshotとSession利用記録は独立して保存し、そのcommit済みgenerationを公開する。初回の正当な空Thread観測も保存し、quotaの成功を待って破棄しない。未判定quotaは不明のまま保持し、成功・0・実失敗へ変換しない。実際のlane失敗はそのlaneの保存状態として保持し、確認済み成功は当該laneの失敗だけを回復する。Thread成功をquotaの回復へ流用せず、quota成功をThreadの回復へ流用しない。NoCommitではackせず、epoch不一致ではcommitもackもしない。wait中に新しい完全candidateを得られない場合は、wait結果を現在の公開state・published pairのまま返す。新たなactive Sessionと保存済みcheckpointの不一致は、同一owner・lease・account epoch内で一度だけguarded Session収集とread-back checkpointによる再取得を行う。非同期probeが返したcheckpoint不一致は、消費時の同一owner・lease・account epochに属する最新のcommit済みcheckpointに対して、報告された物理キー（root identity、relative path、device、inode）を照合してから再収集要求または再不一致の失敗へ採用する。報告された欠落が既に解消したことを証明できる場合、その古い不一致から失敗や追加の早期収集を生成せず、last-goodと取得状態を保持し、既存の単一in-flightとcadenceによる新しいprobeで完全snapshotの成否を確認する。checkpointの存在だけでThread取得成功やREADYへ昇格しない。最初の不一致だけでは既存の完全snapshot・取得状態を変更せず、不完全なcandidateを公開しない。再不一致、再収集失敗または再取得失敗ではlast-goodを保持してdegradedを公開し、同じ未回復episodeで早期収集を繰り返さない。既存degradedを解除できるのは検証済み完全snapshotの成功だけとし、account epochが変わった早期収集は開始しない。この有限なcheckpoint再照合と`THREAD-CADENCE-435`の明示的な適応probe例外を除いてcollection、session scan、probe/RPCの固定周期回数を増やさず、元の通常collection deadlineを維持し、未計測の1秒保証を表明しない。
- `THREAD-CADENCE-435`: 通常のrecorder/Session収集・quota周期は設定値（既定60秒）を維持する。同じaccount・epoch・thread IDの完全な受入済みsnapshotで、両方が既知の`total_tokens`が増加または減少した場合だけThreads probeを5秒周期にする。NULLは未知で0と区別し、初回・再起動・identity変更・未知値からの初観測はbaselineとする。失敗では受入値と最後の差分時刻を保持し、無変化の成功として扱わない。同時に一つだけprobeを進め、実行中の次probeのqueueや遅れた周期のまとめ実行を行わない。最後の有効差分から60秒以上経過し、同じidentity集合の既知値を比較できる成功probeが無変化（または連続する完全空snapshot）なら設定済み通常周期へ戻る。未知値だけでは安定を証明しない。適応modeはIssue #362の通常probe/RPC回数制約への承認済み例外であり、candidate scan、rollout読取り、短命app-server起動とRPCの負荷増加を伴う。完全snapshot、account/epoch、pair、error/last-goodを保ち、Sessionの全走査やquota収集の周期を増やさない。初回検知は通常周期まで待ち得る。5秒はprobe開始の目標周期であり、公開・Main取得・詳細取得・pair受理まで含む5秒鮮度を保証しない。詳細は`REST_API_V1.md`のThreads producer周期へ委譲する。
- coreの性能契約は入力規模に対する処理量で定義する。不変owner tickは`O(1)`かつDB/Session/history/JSON処理0、同じthread結果はactive thread数との比較だけ、currentはcurrent model数、periodsはperiod数、history初回は選択期間のsample×model数、deltaはcursor後のsample×model数、threadsはactive thread数にだけ比例させる。DB machine、CPU、storage、同時負荷および通常payload bytesは利用者ごとに変わるため、根拠のない通常応答の固定時間・固定件数を保証しない。固定するのはschema、整合性、保持・取得範囲、failure isolation、OOM/DoS安全境界であり、低性能環境で処理が遅れても記録を捨てたり、UI取得失敗をrecorder停止へ波及させたりしない。
- DBは履歴inventoryであり、現在openしているSessionの単独根拠にしない。同一cycleで検証したprocess identityとcanonical Session path、thread/read ID/pathを用いる。動作状態は`THREAD-OPEN-362`の明示authorityだけを用いる。
- live rolloutではtask lifecycle、model、token stateを決めるrecordをstrict検証し、不正ならcycleを拒否して最後の完全snapshotを保持する。表示内容recordもDATA第2章のenvelope・UTF-8/JSON・stateへの非影響を完全検証し、同章が許す本文隔離だけを使う。`response_item`という名前だけで壊れたJSONや途中切れrecordを無害と推測しない。EOF直前の未改行tailは次cycleへ保留し、最後の完全snapshotを保持する。local usageの改行済み不正record隔離と取り違えない。
- 現在openのthread候補はCodex processが現在openしているcanonical Session pathの有限集合だけを正本とする。各pathのDATA定義による先頭`session_meta.payload.id`に対して同一app-serverへ`thread/read(includeTurns=false)`を行い、返却ID/pathと完全Thread schemaを一致確認する。同一IDに異なるpathが対応したcycleは拒否する。全Sessionを読む`thread/list`、state DBだけを候補正本にするfallback、Session総量に比例するRPCを本番取得経路に置かず、initializeを含む1cycleを共通15秒deadlineで打ち切る。
- RESTはread-onlyである。未知route/method、不正header/schema、oversize requestからDB、settings、cursor、processを変更しない。
- GUIなしserverはwindow、Slint component、display backendを生成せず、明示したservice lifecycleで起動・停止・復旧する。
- resident serviceは`API-LIFECYCLE-01`に従い、現在値/Threadsと履歴の各完全candidateから独立したimmutable frameを公開する。Linux / WindowsのMainは`GET /v4/current`、GraphはWIREの優先順位で取得するv5 directoryと選択期間の`GET /v3/history` page、Threadsは`GET /v3/threads`を消費する。旧versionへの遷移はWIREのexact404規則だけに従う。各surfaceは任意model配列とmodel-source provenanceを持つstrict validation済みの同一pair page集合だけをatomic表示し、別endpointのfieldを混在させない。履歴差分は前項のprefix証明を満たす場合だけ直前pairのrootを新pairへ更新できる。新resourceを持たない旧serviceが`/v3/current`へexact 404を返す場合だけ、互換用`GET /v3/details`、さらにexact 404の場合だけ`GET /v2/details`、`GET /v1/details`へ一応答ずつfallbackする。timeout、malformed、上限超過、他statusではfallbackしない。SQLite/JSONL/app-serverの再収集、値の再計算、client都合のfield補完を行わない。正確なresource、容量、cursor、頻度および失敗境界は`docs/REST_API_V1.md`を参照する。
- `GET /health`（`/v1/health`互換）の200はresident serviceがread-only snapshot requestを受理できるreadinessと、Cargo/Windowsの単一authorityから導出したproduct versionを表す。wire値はcanonical stable/betaとする。beta consumerの実装・直接検証済み範囲は`VER-BETA-BUILD-CORE-467`のWindows CoreとLinux healthに限り、Linux更新比較/installerは未完了である。Linux launcherはmanaged Recorder/RESTの各systemd `MainPID`、process starttime、実行fileのdevice/inode/SHA-256をhealth取得の前後で確認する。profile lockはRecorder PID、port 8787のsocket inode/FDはREST PIDへ結合し、両processが同じmanifest source generationに属することを要求する。embedded構成だけは同じPIDが両役割を持てる。別processのlockとlistenerを一つのPIDに要求しない。PID、listener、health body、versionのいずれか単独ではcurrent ownerと判定しない。既知の旧Codex Info ownerだけを一度交代し、unknown・foreign・malformed ownerはsignalせず30秒以内に`SAFE_BLOCKED`とする。Linux / Windows clientはschema-validな異なるproduct versionを表示・診断情報として保持したままdetails受理へ進み、API互換性はWIREが定めるresource別のversion優先順位、strict validation、exact 404時だけのfallbackで判定する。version欠落・malformed healthでは進まない。認証済み、data `state=ready`、最新収集成功を意味しない。認証開始・確認はcontrol-onlyであり、control応答を表示dataとしてcommitせず、その後に受理した新しいdetails generationだけが画面を変えられる。

## 4. データ保護

- 履歴DB、verified backup、設定、Linux側履歴は、install、update、rollback、uninstall、restore失敗で削除しない。
- canonical ChatGPT account identityの初回確認は、owner-onlyかつstableに読めた`CODEX_HOME/auth.json`のexact `tokens.account_id`と同一app-server processの前後2回の`account/read`一致を必要とする。確認済みpartition/sourceでのlocal記録の継続はDATA §8.6/8.7のraw AccountKey・source/account結合・所有区間の検証に従い、毎cycleのRPC成功を必要条件へ戻さない。app-server通知の形式・頻度をaccount authorityやSession/DB停止の根拠にしない。emailを保存scopeや認証authorityへ使わず、raw AccountKeyとtokenをpath、DB、metadata、log、RESTへ保存しない。前後の同一`account/read`で確認した1..254 Unicode scalar、trim済み・control文字なしのlogin IDだけは表示専用値としてowner-only account registryと当該account DBの`storage_partition.login_id`へ保存し、loopback限定`GET /v3/accounts`のnullable `login_id`として返せる。login IDをpath、partition ID、hint、journal、診断logへ入れず、欠落しても記録を停止しない。
- 保存単位を`(OS user, ProfileScopeId, AccountScopeId, StorageEpoch)`とし、accountごとに`history/accounts/v1/<AccountScopeId>/epoch-<StorageEpoch>/usage_history.sqlite3`へ物理分離する。同一accountへの再切替は同じStorageEpoch/DBを再利用し、別accountのrow、writer lock、backup、Session checkpoint/marker/gapを混合しない。新規書込みは現在認証済みaccountだけ、保持済み履歴の読出しは明示選択した一つのaccountだけとする。`default_account_id=null`のaccount directory pollでも、明示選択したaccountが一覧に残る限りLinux/Windows Main・Graphの選択を保持する。直前の既定currentを追従していたlogout、または選択accountが一覧から消えた場合だけ選択を解除する。
- account lifecycleが`A→B→A`のように分割される場合、Session eventはDATA §8.7の一つの連続所有区間と公式periodの資格で所属判定し、writerだけが対象periodの累積model totalsと可視historyを再計算する。有効な既存eventだけから、旧保存合計に別所有区間が混ざったことを訂正する経路では、`all events`と`owned events`が一致せず保存値が`all events`と完全一致する場合だけ、verified SQLite backup後に補正する。この一致条件はその旧合計の由来を確認する資格であり、原本が旧event自体の誤計算を証明する`HISTORY-RECOVERY-614`の全prefix修復へ流用しない。後者は同要件の全条件で判定する。どちらの資格も満たさない不一致・不足・不明は旧有効値と理由を保持し、別accountのSession値をUI/readerで減算しない。
- 同じ物理Session JSONLの重複`prefix_generation`がaccount切替・再起動で検出された場合、重複するsource identity、byte range、token recordは一つの物理証拠へcanonicalizeし、重複event rowを保存しない。既存の重複rowはverified backup後のwriter repairで削除し、累積model totals、history、graphへ二重加算しない。source identity・payload・byte rangeが一致せず一意に重複と確定できない場合は正常データを削除せず`unattributed`として保持する。
- logout、account切替、identity/metadata/DB検証失敗では旧accountのdurable dataとSession sourceを保持し、current公開rootだけをstrict emptyの`auth_required`、`initializing`、または`error`へ切り替える。logoutは`auth.json`不在だけで確定せず、同一app-server processの2回の`account/read`がともに`account=null`かつ`requiresOpenaiAuth=true`で、両応答が一致する場合だけ確定する。不正応答、矛盾、transport失敗はlogoutへ変換せず`error`とする。 [公式account/read契約](https://learn.chatgpt.com/docs/app-server#1-check-auth-state)の`requiresOpenaiAuth`は選択providerのOpenAI credential要否であり、認証済みaccountの有無とは独立する。有効なChatGPT account objectの採用は、このfieldがbooleanのtrue/falseいずれでも同じaccount資格・同一processの2回照合を必要とし、trueをログイン欠如や矛盾へ読み替えない。account切替後は新account自身のquotaをcommitするまでcurrent公開を`initializing`に保ち、旧accountのquotaや`reset_at`を表示しない。旧account DBへのfallbackと自動migrationを行わない。
- current quotaの`reset_at`は、現在account partition・AuthEpochで受理した最新quotaのprovider `resetsAt`をUnix秒のまま保持し、同じ観測のWと保存する。応答順はdurable sequenceで保持し、observed_atやRの大小で変更しない。exact R/Wは報告枠の保存keyであって実利用週IDではない。旧報告枠のorigin/終端metadataを再現する場合は同じ原観測を用い、providerが保証した不変の実期間identityと表明しない。RECORDED-USAGE-SCOPE-604に従って既存Main/Graphの期間・モデル値・価格を保持し、補助rolling範囲へ置換しない。quota取得失敗、account切替、logout、identity失敗は既存のlast-good/strict-empty契約を維持する。
- `CURRENT-PERIOD-BOUNDS-129`の公開`observed_at`は、最新受理quota receiptの元観測時刻と、同じaccount/現在exact reset/所有区間にある公開資格済み履歴観測の終端・confirmed gap終端との最大値とする。履歴はDATAのTと保持窓で資格を判定し、raw終端が保存されていればその秒、canonical minuteしかなければそのtimestampを使う。公式receiptの元観測時刻・取得順序は変更せずDBに保持する。別periodの履歴、thread活動、heartbeat、公開処理のTだけではcurrentの観測終端を進めない。current model latestは同じT/所有権/exact periodで受理した最新model事実を使い、quota観測時刻より後という理由で省略しない。recentとretainedは共通domainで求めた同じcurrent観測時刻とmetadataを使い、31日adapterから観測時刻を再計算しない。quotaがない旧形式では同じ所属条件の実在観測・gapの最大値を使い、何もなければ`observed_at=null`とする。この公開時刻は全fieldが同時に取得されたことやquotaの再取得成功を意味しない。quotaの取得成否は独立した永続acquisition状態で表示し、古いquotaを再取得済みとして扱わない。
  固定例としてquota元観測1200、同resetの有効model観測1260、T=1380なら公開`observed_at=1260`、current終端は`min(reset_at,1260)`であり、1260の点を保持する。Tだけ1440へ進めても実在記録・窓が変わらなければこの値は変わらない。quota元観測1200は書き換えず、残量値も補間しない。
- `LIVE-RESET-129`: 公式quotaの取得・次回reset予定の変更はSession利用・開始・終了と非同期である。完全な公式応答は元のexact R/W・Remaining・取得順で保存し、Rの変化を丸めたり固定件数で拒否したりしない。取得時刻が後退しても応答順を変更しない。Rの変化は新しい報告値であり、実利用期間の切替や原因名の証明ではない。実期間の同一性と記録分の表示は`RECORDED-USAGE-SCOPE-604`に従う。Session累計counter・checkpointをquota更新でゼロ化せず、同じ消費を新しいRへ再加算しない。保存・公開窓と転送pageの容量境界はWIRE ownerに従い、報告値の受理・現在Thread・原本の保持を壊さない。
- `CURRENT-PERIOD-BOUNDS-129`のhistorical metadataは、同じaccount/published pairの保存事実からexact resetごとに求める。`HISTORY-RECOVERY-614`の検証を通った公式originが一意なら`start_at=origin_at`、なければ公開対象の最初の観測timestampまたはconfirmed gap開始の小さい方とし、`origin_at=null`とする。`end_at`は同じexact periodの公開対象の観測終端とgap終端の最大値をresetで上限clipした値とする。観測原時刻を保持している場合はその終端を使い、canonical minuteしか保存されていない場合はそのtimestampを使う。原時刻に架空の59秒を加えない。保持内の点もgapもない過去IDは通常directoryに掲載せず、直接historyはWIREの既知ID空pageとする。履歴の回復・保持窓の変化がない限り同じmetadataとなり、UIの現在時刻で延長しない。公式windowの後方移動等により異なるIDの表示範囲が重なっても有効であり、隣periodのstartを使ってclip・統合・再所属しない。固定時間窓でも各exact periodを独立投影し、重なりは同一観測の重複計上と区別する。`start_at<=end_at<=reset_at`を構成できない場合は、WIREの履歴利用不能とcurrent保持を使い、時刻を捏造しない。
- account partitionの旧`reset_at` aliasと既知の`-1` quota sentinelは、DATA owner `HISTORY-CANONICAL-134`の検証済みbackup後migrationで`usage_history`単一正本へ置換する。既存model/provenance sidecarは`HistoryCanonicalizer`が実際に採用したraw usage rowのexact旧キーに結び付くものだけを同じcanonical keyへ再所属し、捨てたaliasのsidecarを比較選択・混合・補間しない。現在・過去accountへ同じmigration/write契約を適用し、稼働DBにraw/canonical並列表、revision同期、reader/UI補償を残さない。同一period内でquotaまたはvectorを一意化できない局所minuteは値を作らずlive集合から除外し、period ownerまたはcycle境界不明時だけpartition全体を無変更としてverified backupとlast-good publicationを保持する。
- migration、restore、updateはcandidateを完全検証してからatomic switchする。検証またはswitch失敗時は旧世代だけをcurrentとして保持する。
- cursorはsource identityと結合し、rotate、truncate、replaceを区別する。古いoffsetによるskipと二重登録を防ぐ。
- Sessionの再起動・account境界・source継続はDATA owner `RECORDER-MODEL-01`と8.7.2に従う。再起動だけで検証済みcheckpoint以後の未読rangeをEOFへ飛ばさない。A→B→Aではaccount所有区間を再検証し、Bの区間をAへ帰属せず、検証可能なAの原本と継続証拠を捨てない。partial tail、変更prefix、legacy、未帰属rangeを推測で採用しない。
- `history_mode=paginated`と非nullの`history_base`は継続照合の入力であり、それだけで未計上利用を証明しない。境界情報・元anchorを別cycleや再起動後も保持し、DATA owner `RECORDER-MODEL-01`の原本prefix・成分・重複排除の規則で採用する。`last_token_usage`を別の加算則として使わない。不正な観測は次の累積基準へ昇格せず、REST/UIで補正値を捏造しない。
- append-only Sessionがinventory後に同一inodeのまま伸びた場合は、scan開始時の長さを固定したprefixだけを検証・収集し、それ以後の追記を次cycleへ残す。この正常な追記競合を未解決rangeや公開rootのerrorへ変換しない。inode変更、短縮、固定prefixの不完全・不正recordは従来どおり拒否する。
- backup作成または検証に失敗した場合は既存のverified世代をpruneしない。
- crash、reboot、再実行はjournalの同一operationを再開し、commit、publication、deleteを各1回以下にする。
- local session inventoryはSECURITYのRequired controls 4に定義されたdepth・file数・1 file・1 lineの製品資源上限と数え方を先に検証し、同条項のmtime nanoseconds降順・canonical path降順のwhole-file prefixだけを収集する。選択範囲とoverflow、個別sourceの失敗時保持は同条項とDATA §4.4に従い、全inventoryの2GiB超過だけでLinux detailsを停止しない。
- 2GiBは1 cycleで新規rangeを検証する入力上限であり、保存するlocal消費factの累計母集団ではない。source identity・checkpointで検証した未記録rangeだけを数値・cursorと同じtransactionでexactly once保存する。報告枠Rの保存集計は元のexact keyで保全するが、R更新だけで記録分を初期化・carry・再加算しない。実利用期間として表せる数値と記録分／保存観測の表示資格は`RECORDED-USAGE-SCOPE-604`に従う。source単独のrange/identity不成立はそのsource候補だけを取り消し、独立した有効sourceの記録分とpartialを保持する。共有account/admission tupleの不一致またはusage transaction失敗ではbatch全体を保持・rollbackし、部分commit・部分公開を行わない。period所属不明は既知の消費を0や最新periodへ変換する理由にしない。
- 2GiBから外れた古いsessionは、その完全fingerprintのusageとrecorded markerがSQLiteへ同一transactionでcommit済みで、fresh DB readback・path/identity再検査・Codex open-FD不在を満たす場合だけruntimeに1 fileずつ削除できる。未記録・legacy・変更済み・active・selected sessionと、履歴DB/backup/reset hint/recovery JSONLは保持する。

## 5. Linux bundle導入・自動更新・削除

- LINUX-BUNDLE-RETENTION-01: committed installではjournal.phase=committed、current == journal.new_generation、およびcurrent generation全member identityの再検証が必要であり、old_generationが空でなければその正確なidentityを検証した1世代だけをrollback用に保持する。committed rollbackではjournal.phase=committedかつ空でないold_generation == currentを要し、new_generationは空またはcanonical generation IDを許す。存在するnew_generationは失敗candidateへのjournal参照として保護するが、verified rollback世代とは数えない。old_generation == new_generation == currentはcurrent一世代とし、どちらにも確定できないjournalではpruneしない。初回installなどold_generationが空の場合は追加のrollback世代を作らない。current、稼働processの実行generation、unitまたはenable linkが参照するgeneration、journalのold/new generationを保護し、transactionが未解決・曖昧ならpruneしない。unknown/foreign entry、名前・owner/mode・manifest・member identityが不正なgenerationは削除しない。保護対象以外はgeneration directory自体と全regular memberを直前に再検証し、symlinkをたどらず削除してread-backする。参照processの列挙・owner・実行fileを検査できない場合はgenerationを削除せず、検証済みcommitまたはno-updateの結果を保持したまま`GENERATION_PRUNE_DEFERRED`としてcleanup延期を明示する。committed current/rollback generation・journal・削除直前のmember identityの不整合やunlink失敗は`GENERATION_PRUNE_FAILED`で非0終了し、committed current、履歴DB、verified backup、設定、Sessionをrollbackまたは変更しない。正常なmanaged generation数の上限はcurrent 1 + rollback 1 + distinct live process/unit/journal reference数とし、unknown/foreign entryや外部参照数を含む全directory・byte数の絶対上限は保証しない。pruneは起動前ExecStartPre、rollback/recovery途中、--removeでは実行しない。committed install後と、manual/timer no-updateでlocal generationおよびrunning時のmanaged readinessを確認した後に、残存obsolete generationを整理できる。

- Linux bundleはbyte-identicalなruntime launcher `run.sh`、payload `codex_info`・`codex_info_recorder`・`codex_info_rest`、`install.sh`、recorder/REST/update serviceとtimer、license/notice、version/target情報を含む。外部manifestとarchive内manifestはbyte-identicalで、launcherを含む全regular memberのpath、mode、size、SHA-256を同じcandidate identityへ結合する。install後の固定入口はlauncher `$HOME/.local/bin/codex-info`、各payload `$HOME/.local/bin/codex_info*`、persistent installer `$HOME/.local/libexec/codex-info-install.sh`、manifest `$HOME/.local/share/codex-info/manifest.json`、unitは`$HOME/.config/systemd/user/`配下のexact 4名である。各固定入口はowner-onlyな`$HOME/.local/share/codex-info/generations/<version>-<source_sha>-<manifest_sha256>/`のregular fileをatomic `current` symlink経由で参照する。repository、Cargo、元bundle、`target/`へ依存しない。
- manual、launcher startup、service `ExecStartPre`、timer、bootは一つのRelease resolver/installer authorityを使用する。timerは`OnActiveSec=5min`、`OnUnitActiveSec=1h`、`AccuracySec=1s`で、公開後の次回発見上限を1時間1秒とする。`codex-info-recorder.service`と`codex-info-rest.service`の起動前reconcileも同じauthorityを通るため、停止していたdaemonを新しく起動した場合に旧versionを無条件で開始しない。新版がなく同versionでも、local世代が不整合なら検証済みReleaseから修復する。ただし起動前の更新試行が失敗しても、`desired_state=running`、完全なcurrent generationと固定link、transaction不在またはexact `committed`、listener不在を再検証できた場合は、更新を`deferred`と失敗表示したままsystemdの`ExecStart`へ制御を返す。manual/timerの更新失敗、local不整合、未解決transaction、listenerの存在または曖昧性は成功へ変換しない。
- resolverは固定HTTPS API `https://api.github.com/repos/salty919/codex_info_v2/releases/latest`の単一応答からlatest Releaseを取得し、`windows-vSemVer`に対応するLinux archiveだけを選ぶ。Release履歴件数、Windows assets、checksum/外部manifest sidecar、無関係assetをLinux更新の条件にしない。取得したarchive bytesはGitHub asset APIのSHA-256 digestと一度照合する。archive内manifestはgeneration identityに必要な値を読むために使い、上流で完了したmanifest/member/content検査を再実行しない。ホストglibcがcandidateのminimumより古いと確認できた場合だけABI不適合として適用を止め、glibc versionを取得できないことだけでは止めない。既存v1.0.113は上書き導入で置き換え、本契約の更新受入は置き換え後のresolverから次候補へのLinux更新とする。 通常stableの比較は同じbundleに0644で同梱しmanifestへ結合した`product_version.py::compare_versions`を使い、canonical32文字のstable/beta数値順序を共有する。通常のlatest取得と`update`・`no-update`・同version不整合時のrepairを維持する。`--select-release --release-metadata FILE --current-version VERSION`はローカルmetadataの読み取り選択だけを行い、未指定channelはstable、明示`--channel beta`は非draft・prerelease・exact beta tagと一致するarchive/digest/URLだけを対象とする。beta候補なしは`no-candidate`として別channelへfallbackしない。選択はstate初期化より前に返り、download/install、channel保存、service/DB変更、低いstableへの実復帰を許可しない。
- 手動 `--bundle` installationは既存のchecksumと外部manifestを使う。自動更新はAPIのarchive digestで実転送を確認し、owner・同時変更回避、DB/current保持、transaction rollback、candidate展開後の実稼働結果を対象端末で確認する。Release publisherは選択candidateのrun/source identityとGitHubへのupload・公開結果を確認し、Linux packageのmember/content再検証はlinux-distribution ownerへ委ねる。downgradeは既存no-updateとして扱い、同versionでlocal generationが不整合ならlatest archiveからrepairする。
- installはowner-onlyな`.install.lock`をnonblockingで取得し、`install-transaction.json`へ各phaseをatomic write・file fsync・parent directory fsyncしてからlegacy退避、entrypoint link、candidate publish、`current` switch、activation、functional readiness確認へ進む。functional readinessはsource-bound health、recorder identityに加えて同じ30秒枠内の`/v4/current`のexact schemaと`state=ready|auth_required`を必須とする。`ready`は正の元`observed_at`を要し、`auth_required`は未取得の`observed_at=null`を許す。記録分のpartial・不明な実利用期間・モデル不明と、serviceの起動失敗を混同しない。新resourceのexact HTTP 404だけは旧世代との互換として`/v1/details`の同じ状態・正の`observed_at`検証へ進む。503・不正JSON・未知/重複field・`initializing|error`では旧resourceへfallbackせず、更新成功にしない。旧composite履歴の更新待ち503を、正常なcurrentの起動失敗へ転用しない。stale transactionは同じoperationを一意にresumeまたはrollbackしてから新operationを開始する。`current` switch後を含む失敗では旧generationへ戻し、旧serviceをsystemd管理下で復旧してから終了する。更新開始時に存在した使用可能な旧世代をunmanaged processとして残さない。
- generation/current導入前の既知predecessor flat installは、旧manifestのexact schemaと記録済みpath/size/SHA-256、固定配置、owner/mode、同一sourceのbinary/installer/3 unitを全て再検証できる場合だけ一方向bootstrap入力にできる。旧installerがL1を保持したまま新serviceを最初に起動する区間は`ExecStartPre`が同じlockを待たず、検証済みflat payloadの起動を妨げない。lock解放後の最初のmanual/startup/timer/repository launcher操作は同Release archiveを取得・完全検証してgeneration/current/launcherへ移行し、旧flat状態をcompleted terminalにしない。検証不能なflat file、repository binary、`target/`、version文字列だけをbootstrapやrollbackへ使わない。
- `desired_state=running`で完了したlauncher/startup/timer/update収束のterminal stateは、完全に検証した新世代がmanagedかつfunctionally readyなA、または完全に検証した旧世代がmanagedかつfunctionally readyなBのいずれかだけとする。`desired_state=stopped|disabled|removed`の操作は、対応するservice/timer/unit状態、listener不在、保持対象、local generation整合をread-backした場合だけ別の正常な非稼働terminalとする。unknown/foreign/malformed listenerまたはlockを安全に識別できない場合だけ、何も停止・上書きせず30秒以内に明示的`SAFE_BLOCKED`で終了できる。この安全例外を成功やA/Bへ読み替えず、次のmanual/startup/timer triggerを妨げない。manual/startupは20分30秒、control RPCは30秒、local validate/publishは60秒、stopは20秒、readinessは30秒、rollbackは60秒以内で必ずterminalになる。
- installer/controlはL1 `.install.lock`だけ、resident runtimeはL2 profile recorder lockの後に必要な場合だけL3 account writer lockを取得する。installerはL2/L3を取得せず、serviceはL1を取得しない。systemd start/restartは`--no-block`で要求後にread-only pollする。これを唯一のlock順序として、launcher、startup、timer、removeのcycleを作らない。
- `codex-info-recorder.service`と`codex-info-rest.service`はunexpected exitを`Restart=always`、`RestartSec=5s`、`StartLimitIntervalSec=0`で再起動し続け、start limitによる永久inactiveを作らない。反復失敗中は失敗のまま可視化し、成功扱いしない。admitted accountでは新規rowが0件でもrecorderが各scheduled generationをtransaction commitし、commit確認後だけowner-only `recorder-state.json`の`last_commit_unix`を更新する。全write stateのheartbeat `updated_at_unix`とadmitted accountの`last_commit_unix`はDATA §4.1の同一時刻・150秒freshness・未来値拒否で検証する。account未確定時は`idle_no_account`とし、架空のpartition/commitを作らない。
- recorderとquota laneのscheduled cycleはmonotonic anchorからのfixed-rateとし、cycle処理時間を次の60秒へ加算しない。deadline超過時はbusy catch-upせず1回だけ直ちに実行後、最初のfuture deadlineへ戻る。timestamp間隔だけからDB row、degraded、gapを作らない。DB書込み障害は同callbackでretryせず`degraded`へ進み、daemonとexact pending batchを保持したまま次のscheduled cycle（60秒以内）で一度だけ再試行する。timeout、busy、full、readonly、corrupt等のDB応答をworker死亡と推測してdaemonを終了しない。writer threadの実終了だけは1秒owner loopの`JoinHandle`で確定して非0終了し、systemdへ復旧を委ねる。停止区間のSession usageだけは検証済みsource cursorからbounded backfillできるが、quota/残量を補間・複製しない。gapの状態・証拠境界・公開は`U128-19`へ従う。
- 利用者操作は`codex-info --update`、`codex-info --status`、`codex-info --stop`、`codex-info --disable-autostart`、`codex-info --remove`を使用する。raw `systemctl`は診断用で、直接stopは永続的な製品停止意図ではなく次の更新でmanaged runningへ正規化され得る。`--remove`はrecorder/REST/update unitだけを停止・無効化・解除し、installed generation、launcher、installer、manifest、履歴DB、verified backup、reset hint、gap/recorder/control state、Codex session JSONL、設定を削除しない。

## 6. Windows導入・更新・削除

- アプリ起動後の更新確認は通知だけを生成し、download、Setup起動、既存payload変更を行わない。
  新版がある場合だけ状態帯に更新操作を表示し、利用者がその操作を明示実行した後に限ってdownloadと
  標準GUI Setupを開始する。常設の更新ボタン、silent install、unattended apply、自動再起動を行わない。
- 更新候補は公開済み・非prereleaseの`windows-vX.Y.Z` Releaseだけから選び、同Releaseのexact-name
  installerとmanifestについてversion、URL authority、byte size、SHA-256を完全検証する。不一致、
  redirect逸脱、途中download、oversize、起動失敗は既存payloadを変更せず、部分fileを公開しない。
- install、update、rollback、uninstallは別transactionとして扱い、stage中のfile、shortcut、HKCU、Apps登録を成功状態として公開しない。
- update失敗時は旧payload、shortcut、registry、versionを起動可能な状態で保持する。初回install失敗時は未公開状態へ戻す。
- 更新確認・download・検証・GUI Setup起動の各結果と再試行はUX §4.6「Windows更新の表示状態」に従う。Setup子processの起動成功をinstall完了へ変換せず、backendの取得状態・認証・last-good dataは更新状態から変更しない。
- uninstallは設定と履歴を保持する。途中失敗は完全復元または再開可能なjournal状態のどちらかにし、部分削除を成功と表示しない。
- 同一install rootの同時操作は単一leaseで直列化する。PID再利用、foreign owner、reparse差替え、token変化を検出した操作はmutation 0とする。
- interactive/silent等のmode、exit code、対応Windows、architecture、署名者、version policyはrelease authority inputで決める。
  署名者authorityが未設定なら署名済みと表明せず、設定済みauthorityと不一致なら更新候補を拒否する。
  利用者が開始するunsigned OSS buildは、exact GitHub repository、release tag、manifest、size、SHA-256の
  検証を満たす場合だけ標準GUI Setupへ渡し、Windowsが示すpublisher警告を隠さない。

## 7. 配布・顧客向け表明

- `VER-BETA-IDENTITY-467`: 採番CLIの`beta --stable-version X.Y.Z --run-number N --run-attempt A`は、
  callerが固定したmain stable snapshot由来のcanonical stable `X.Y.Z`と、GitHubの正canonical十進整数N/Aを入力とする。
  major/minorを維持しpatchだけ十進で1増やした`X.Y.(Z+1)-beta.N.A`、`windows-v`付きtag、`channel=beta`、
  `prerelease=true`、`make_latest=false`をidentity metadataとして出力する。同じ固定入力は同じidentity、新runまたはattemptは
  異なるidentityとする。例は`1.0.109/7/1 → 1.0.110-beta.7.1`、`2.7.99/7/1 → 2.7.100-beta.7.1`。
  非canonical stable、prerelease stable、N/Aの0・負値・leading zeroは出力前に拒否する。file・Git・networkは変更しない。
  stable `check/next/bump`と3 version fileのstable限定契約を維持する。tagの予約・存在/衝突確認、source identity固定、
  full versionのbinary stamp、公開producer、channel選択、stable復帰は後続責務であり、この計算の成功はRelease作成を意味しない。
- `VER-BETA-BUILD-CORE-467`: `stamp-beta`はcallerが固定した40桁lowercase source SHA、main stable、run N/A、
  期待source versionと明示build snapshotを受け取り、snapshotのroot Cargo package/root lock/.NET Versionを同じfull betaへ
  atomic同期する。checkout自身やsymlink経由の入力、不一致、非canonical入力は変更前に拒否する。既存依存・別process package
  は保持し、通常tree・branch/tagを採番で変更しない。InformationalVersionはfull betaと固定source SHA、Assembly/FileVersionは
  numeric baseの4成分（末尾0、各成分0～65534）とする。full versionは既存healthの32文字境界内とし、長すぎる入力は拒否する。
  例はsource version `1.0.119`、main `1.0.109`、run `7/1`から`1.0.110-beta.7.1`、numeric `1.0.110.0`。
  Windows Coreは一つのcanonical stable/beta modelで数値比較し、`1.0.110-beta.9.1 < 1.0.110-beta.10.1 < 1.0.110`、
  同runのattemptも数値順とする。
  Linux比較CLI `compare --left L --right R`は`scripts/product_version.py`の純粋`compare_versions`で同じliteral oracleを比較し、
  canonical stable/betaを32文字以内、ASCII十進数、beta N/Aは正数に限定する。base/run/attemptの数値順と同base beta<stableを使い、
  `comparison=-1/0/1`だけを出力する。非canonical入力はstdoutなしで拒否し、version-fileのpath解決・読取・変更より前に返す。
  後続Linux候補選択はこの同じ関数を再利用する。現行release selector/download/install/channel操作はこの比較CLIでは変更しない。
  ProductInfoはInformationalVersionのsource metadataを除いたfull product versionを`v`表示へ渡す。
  Windows Coreのhealthとruntime metadataは同じmodelでcanonical stable/betaを受理し、REST/recorder別processのversion、null/unavailable/mismatch
  と診断上のversion不一致を保持する。Linux health consumerも32文字以内のcanonical stable/betaを受理し、
  beta N/Aは正canonical十進整数とする。HTTP200、api_version、service、strict schemaと診断上のversion不一致を保持する。
  このconsumer・snapshot契約はpackage全体identity、update channel、Linux更新比較/installer、公開・実機の成立を表明しない。
  stable `check/next/bump`は引き続きstableだけを受理する。
- `VER-BETA-LINUX-BUNDLE-467`: Linux local bundle producerはcallerが固定したcanonical stable、または
  32文字以内のcanonical `X.Y.Z-beta.N.A`を受理し、同じfull versionを内部/外部manifest、archive/checksum/sidecar名に保持する。
  betaのN/Aは正canonical十進整数、Aはmanifestのrun_attemptと一致する。source_sha、run_id、既存schemaを保持し、
  run_idをbetaのN（run_number）へ読み替えない。非canonical・長すぎるbeta・attempt不一致は出力前に拒否する。
  同beta名のarchive/checksum/manifestが一つでも存在すれば再発行・置換せず拒否する。stableの既存入力・出力契約を維持する。
  incoming archive validatorも同じstable/beta形状とA/run_attempt一致を検証し、内部/外部manifestのfull version、
  source_sha、run_id、archive名/digest、member bytes/modeを保持した同一snapshotをprivate candidateへstageする。
  非canonical・長すぎるbeta・attempt不一致は既存payload/current/service意図のmutation前に拒否する。
  compiled full identity、実導入のgeneration/launcher/journal、Linux更新比較、公開5asset、channel選択・stable復帰・実機は
  後続責務とする。local package/private stagingの成功を公開・実binary・実導入の成功としない。
- `VER-BETA-LINUX-CHANNEL-467`: Linux installerの選択channelはinstallationごとの
  `~/.local/share/codex-info/update-channel.json`へ`codex-info-update-channel-v1`のexact schema/channel/revisionで保存する。
  未設定はstable、app directory0700/file0600・同一owner・regular single-link・atomic replaceとし、既存presentation settingsへkeyを追加しない。
  `--set-update-channel`/`--get-update-channel`は選択意図を保存/読戻しし、installed channel/versionはverified current manifestから別表示する。
  既存normal updaterは同じlease下でこの意図を読み、stable latestまたは最大5page/100件の完全なbeta release listから既存selectorを呼ぶ。
  channel snapshotをdiscovery/download/child applyへ固定し、途中変更・partial enumeration・候補なしの暗黙fallbackを拒否する。
  明示beta選択のみ、manifest-bound共有module付きbetaを既存staging/journal/current/readiness/rollbackへ渡し、modern identity consumerは同じcanonical32 oracleを使う。
  legacy形式は従来のstable限定を維持し、同channel downgradeや通常操作でのbeta→stable復帰は許さない。
  このsource接続の直接検証は合成HOME/proc/fake transportsとfixture identityに限定する。初期対応beta SHA、固定stable復帰、実DB reader/writer互換、
  両OS設定UI、公開assets/実機は未成立であり、dummy data sentinelの保全を実互換証明にしない。

- Windows製品版とX版は単一のstable `X.Y.Z`を共有する。バイナリ影響ありのPRはmajor/minorを変更せず、mergeごとに
  自動採番処理がpatchを十進整数としてちょうど1増やす。patchからminorへ桁上がりさせず、`1.0.9`の次は
  `1.0.10`とする。major/minorは利用者の明示指示を要する別変更でだけ更新し、自動採番処理は変更しない。
  `Cargo.toml`、root packageの`Cargo.lock`、`windows-client/Directory.Build.props`が開始時点で同値でない場合、
  または期待元versionとmainが一致しない場合は、3ファイルを一つも変更せず停止する。
- `main`向けPRは、配布するRust/Windows binaryまたはinstaller/payloadが消費するsource、依存関係・lockfile、組込みasset、
  製品build・packaging入力を1件でも変更する場合を「バイナリ影響あり」、それ以外を「バイナリ影響なし」とする。
  workflow・CI検査・test・文書・repository ruleだけの変更はバイナリ影響なしとする。分類はこの二つだけとし、rename/copyは
  変更前後pathを両方判定する。eventのbase/head commitによる完全なGit差分を単一分類器へ渡し、空差分、未知path、
  欠損したrename/copy情報は分類結果を返さず、versionまたはRelease mutationを開始しない。
- `main`向けPRはsame-repositoryであればhead branch名を制限しない。trusted `pull_request_target`が、version追加前の
  利用者差分（H0）の全pathをDOCS・GOVERNANCE・LINUX_BACKEND・LINUX_UI・WINDOWSの有限ownerへ一度だけ分類する。
  選択ownerだけをimmutableな最終head（H1）で各1回実行し、非選択ownerは実行しない。binary impactはowner名から
  推測せず、runtime/build/packaging入力のpathだけで判定する。そのためtestや検査scriptの変更は対応ownerの品質を
  実行してもversion更新とdistributionを起動しない。CodeQL言語は変更した解析対象sourceからだけ導出し、
  workflowのactions、Python sourceのpython、Rust製品sourceのrust、C#製品sourceのcsharp以外を追加しない。test、shell、文書、
  manifest、assetだけの変更でCodeQL言語を起動しない。選択ownerのmissing/failure/cancel/skip、非選択ownerの実行、
  CodeQL言語の余分・欠落は失敗とする。branch名、branchの作成元、`feat/next`との包含関係は品質選択へ使用しない。
- `main`のtrusted `pull_request_target`を唯一のRelease品質authorityとし、別dispatchへ品質判定を転送しない。
  workflow run名には固定schemaでPR番号、event head、event action、event時のdraft状態をGitHub eventから記録する。
  event時または開始時にdraftであるrun、開始時にclosedであるrun、開始時にcurrent PR headではなくなったrunはobserverとしてowner、分類、
  version mutationを0件にする。open・non-draft・current headだけをownerとする。`version-prepared`と`acceptance`は同じrunの
  Release品質結果であり、branch mergeのrequired checkにはしない。workflowの`GITHUB_TOKEN`がversion commit H1をpushした経路は、
  同じH0 runが保存済み選択でH1を評価し、生成commitの固定trailerとproducer runからH0/H1を対応付ける。
  H1へ`version-prepared`・`acceptance`のcustom check runまたは別jobを登録しない。生成H1だけは、同じtrusted runが単一の
  `main-pr-quality/current-head` commit statusをpush直後の`pending`から、選択job集約後の`success`または`failure`へ更新する。
  statusはrequired checkにせず、失敗・取消・未完了を緑へ変えずに利用者のMerge操作を残す。H2、binary impactなしhead、feat headは
  event自身の既存jobがexact headへ付くため、このstatusを重複作成しない。poll、retry、URL・時刻・表示値の照合も行わない。
  `acceptance`はmain向けに選択jobの結果だけを集約し、失敗時はRelease公開をHOLDするがmergeを禁止しない。
  Windowsを含むmain向けrelease candidateでは、Windows job自身が実Windows評価後にrelease candidateを作る。
  Linux-only変更ではLinux distribution・installer品質を実行するが、Windows評価・candidateを追加しない。
  Linux candidateのrelease buildは配布targetの1回に統合し、bundleから展開した同じUI・recorder・RESTで
  選択されたbackendのCLI/daemon品質とUIのstartup/graph品質を評価する。native unit/Clippyは別の品質責務として保持する。
  candidateがある場合のLinux UI品質はdistribution jobが所有し、独立UI jobのskipではなくdistributionの成功を必須とする。
  サービス復旧の実画面確認はstartupに含む場合と単独実行を排他にし、同じcandidateに対して1回だけ実行する。
  candidateがない通常UI品質ではUIだけをbuildし、使用しないREST・recorder executableをbuildしない。
  Rustの依存コンパイル成果物はrunner OS・compiler・依存・build設定に応じてcacheへ保存し、製品sourceや製品版番号だけの
  変更で依存全体を再buildしない。workspace製品とcoverage reportは再生成し、cache hitをtest成功へ読み替えない。
  native unit/Clippy、配布build、Windows、CodeQLは相互の終了待ちを追加せず並列実行を維持する。
  Windows評価・candidateはWINDOWS ownerが実際に選択された場合だけ実行する。`feat/next`向けPRは、PR本文や機能名の申告を
  品質選択へ使用しない。そのPRのevent base/headの完全差分を使い、mainからfeatへ既に積み上がった別PRの変更を再選択しない。
  各pathを安定した責務境界でDOCS・GOVERNANCE・LINUX_BACKEND・LINUX_UI・WINDOWSへ分類し、
  実際に影響するownerの通常品質だけを実行する。backendはformatとunit、Linux UIはbuildとgraph実画面、Windowsは
  restore・format・unit、文書は要求正本、workflowは変更workflowの構文とowner選択の直接契約を確認する。同一ownerの確認は
  1回へ統合し、別owner、installer、配布物、Release E2E、CodeQLを通常のfeat PRへ追加しない。
  featではPR headそのものを検証し、将来のRelease versionを仮に書き込んだtreeを作らない。
  mainでは公開対象の最終headそのものを検証し、異なるheadのfeat結果を品質成功として流用しない。機能別profile、PR本文宣言、
  機能ごとのexact path allowlistを設けず、新しいfileは既存の責務prefixで分類できる。責務不明のpath、空差分、malformedな
  rename/copyだけは、無関係な全suiteへ拡大せず分類前に停止する。main向けRelease candidateは同じowner分類を使い、
  binary impactがある場合だけdistribution、installer、実OS/UI品質を追加し、Windows ownerが実際に選択された場合だけWindows品質を追加する。feat向け`selected-quality`集約と`feat-acceptance`、Windows release
  candidateは生成しない。実jobの失敗は赤のまま表示するがmergeを禁止しない。live repository ruleの再監査、
  選択済み製品testの再実行、branch名allowlist、custom check登録を追加しない。
- バイナリ影響ありPRだけ、品質確認を開始する前にPR branch上のversion 3ファイルをexact next patchへ自動更新する。
  versionがbaseのH0は完全差分を1回分類し、PR、H0、producer run ID、attemptを固定trailerに持つH1 commitをnon-force pushして、
  同じrunが保存済み選択を使ってH1のownerを各1回実行する。H1 commit自身がmappingを持つため、push後にrunがcancelされても
  H0とH1の対応を失わない。後からH1/H2 eventが起動した場合は、base以降でversionを最後に変更したfirst-parent commitが管理3ファイル
  だけのbyte-exact自動更新で、最終headでもその3ファイルが不変な場合だけ、その3 pathを差分から除外する。tipが正規trailerを持つ
  生成H1の`synchronize` eventだけownerを再実行しない。identityがない同じbytesの手動commitは通常どおり評価し、H2の後続利用者commitも
  除外しない。event内の分類器は1回だけ使用する。これによりH2でも生成versionをLinux ownerへ誤分類しない。
  バイナリ影響なしPRはversionを変更しない。
- Release判定はmerged `closed`と`Main PR quality` completedの二信号を受け、どちらが先でも同じfinal headへ収束する。quality未完了時の
  `closed`とPR未merge時のcompletedはmutation 0で終了し、後着信号が再評価する。workflow runの非成功はMain Qualityの失敗を正本とし、
  Release側へ同じ赤を追加しない。final headを評価した生成producerとdirect runの全attemptを集合化し、draft・stale・generated observerを
  除外する。本来のattemptにmissing、failure、cancelその他の非成功が1件でもあれば、同じheadの後続rerunまたはreopen successで
  上書きせず、新headまで公開をHOLDする。旧成功runへfallbackしない。
- 成功authorityの`windows-quality`が`skipped`ならcandidateは0件だけを許可し、`success`なら同じrun、attempt、PR、final head、versionに
  結び付く既存candidateをexact 1件要求する。ただしWindows authorityが`success`でcandidate 0件、同じattemptのLinux distribution authorityが
  `skipped`かつLinux candidate 0件の場合は、非binaryの品質成功としてresolverを終了0・`publish=false`にする。Linux distributionが成功する場合や
  Linux candidateが存在する場合のWindows candidate欠落は失敗とする。期限切れcandidate、複数、malformed、別identityも失敗とする。candidateは増やさず、
  既存のSetupとmanifestを持つ1件の名前へidentityを追加する。merge後にPRを再分類せず、quality test/build/CodeQLも再実行しない。
  公開jobだけをversion tag単位で直列化し、lock取得後にPR/final head、全attempt、candidate、tag、Release、assetsを1回再取得する。
  tagとReleaseがともに不存在ならDraftを作る。既存tagだけが残る場合は、そのtagが選択済みmerge SHAを指すことを確認してReleaseを作る。
  同じtagのDraftが残る場合は既存assetのupload digestが選択candidateと一致するものを保持し、不足assetだけをuploadして公開を再開する。
  mismatched asset、Releaseだけの存在、または不一致のtagは公開せず、assetの削除・置換・自動cleanupは行わない。完全一致のpublished状態はno-opとする。
  Release mutationだけはcurrent repository限定の短命GitHub App installation token（Contents write / Workflows write）を使い、
  解決、lock後再検証、artifact取得はread-onlyの組み込みtokenを使う。
- PR由来のcheckout、script、workflow、artifactを、repository contents・checks・Releaseへのwrite権限を持つjobで実行しない。
  write権限を持つ採番jobはtrusted baseだけをcheckout・実行し、PR headはGit object dataとしてだけ読む。
  same-repository headへexact 1 commitをnon-force pushし、競合pushはGit自身のnon-fast-forward拒否に任せてreadbackやretryを行わない。
  H1 custom check作成jobは置かず、生成H1のstatus更新は既存のversion-preparedとacceptanceだけが所有する。Releaseのread-only解決jobはGitHub objectとrun状態だけを読み、
  write jobはsourceをcheckout・実行せず、解決済みcandidateとlock取得後に再取得したremote状態だけを入力にする。
- main向けPRの完全path分類からCodeQL言語が導出される場合、その言語だけを実行し、Release品質ではanalysis成功とcritical/high finding不在を
  公開条件とする。CodeQL失敗はworkflowへ表示するがbranch mergeを禁止しない。feat向けPR、CodeQL言語が選択されないPR、merge後pushでは
  CodeQL AnalyzeとAutobuildを実行せず、active code-scanning rulesetの
  設定はworkflow内で再監査しない。外部AI findingsが
  provider側の未対応modelで継続失敗する場合は、そのAI機能だけをrepository単位で無効化できるが、選択済みCodeQL、
  code-scanning alerts、Release品質acceptanceは維持する。
- AIによる補助reviewは、機械的な直接オラクルだけでは判定できないsecurityまたはcross-cuttingな
  高risk境界に限る。通常変更へ一律に起動せず、固定test、CodeQL、Release品質acceptance、必要な承認の
  代替にしない。
- `windows-vX.Y.Z` ReleaseはSetupとmanifestを非公開Draftへuploadしてから公開する。途中失敗を公開済み成功へ変換しない。
- Linux coreはtargetを`x86_64-unknown-linux-gnu`へ固定し、同じstable version・final source SHAのarchive、SHA-256 checksum、manifestを既存の`windows-vX.Y.Z` ReleaseへWindowsのSetup/manifestと同居させる。別Linux tag/channelを作らない。
- Linux bundleの互換性は、bundle manifestに記録した実測`glibc_minimum`（glibc minimum）を満たすことだけを表明する。manifestのtargetまたは実測minimumが欠落・不一致なら候補を公開・導入せず、他のdistribution、architecture、署名済み、publisher検証済みの対応を表明しない。
- Linux bundle archiveはbyte-identicalなruntime launcher `run.sh`、`codex_info`・`codex_info_recorder`・`codex_info_rest`、`codex-info-recorder.service`、`codex-info-rest.service`、永続化する`install.sh`、`codex-info-update.service`、`codex-info-update.timer`、license/noticeを含み、version/target情報と対応するchecksum/manifestを同じcandidate identityへ結び付ける。導入時は`run.sh`を`$HOME/.local/bin/codex-info`、各payloadと`install.sh`をcurrent generationへ参照させる。顧客の通常導線はRelease assetのdownload、checksum検証、extract、bundle内scriptによるinstall、installed launcher、自動更新、source-bound health、removeだけで完結し、repository clone、Cargo build、source treeの`run.sh`を要求しない。
- Linux bundleのinstall、update、reinstall、removeまたはその失敗は、導入binary、永続installer、履歴DB、verified backup、`history/usage_reset_hint.json`、Codex session JSONL、設定を削除しない。removeはrecorder/REST/updateのuser service/unitだけを解除し、部分導入を成功と表示しない。
- release artifactはsource、lockfile、実payload、license/notice、署名、version、対象platformを一つのrelease identityで追跡する。
- publisher名、certificate、対応OS build、RPO/RTO、accessibility適合、support窓口を根拠なしに推測しない。
- authority inputがないclaimは「保証なし」「未対応」とし、認証済み、対応済み、測定済みと表示しない。
- recovery journalと顧客共有support bundleを分離する。support exportは明示操作、allowlist、owner-only ACL、秘密0を必須とし、自動送信しない。
- customer guideとdeveloper READMEを分離し、顧客手順にrepository clone、Cargo build、source treeの`run.sh`を通常導線として要求しない。配布・導入済みruntime launcherは通常導線に含める。

### Workflowに残す確認の理由

| 確認 | 実際に到達するcaseと必要動作 | 確認しない場合の被害 | 上流保証と重複しない理由 |
| --- | --- | --- | --- |
| same-repository | fork PRではversionを書かず失敗する | 書込み不能または誤ったheadを対象にする | GitHubはfork PRを許可する |
| 完全Git差分とowner対応 | add/delete/rename/copyを両端まで分類し、未知pathは失敗する | 必要ownerの評価が欠落する | GitHubは製品ownerを知らない |
| main Release結果集約 | main向けbase版gateだけがselected success・non-selected skipを判定する | 未評価または無関係なcandidateを公開する | owner jobがsourceを評価することと、公開oracleの信頼性は別責務 |
| feat選択品質 | 完全差分から選択した実owner job・CodeQL・distributionだけを実行し、各結果をそのまま表示する | reviewerが対象revisionの実品質結果を確認できない | GitHubは製品ownerと必要言語を知らない |
| version 3ファイル | binary PRだけbaseからexact next patchへ同期更新する | Rust・lockfile・Windowsのversionが分裂する | GitHubは製品versionを保証しない |
| non-force push | version生成中にheadが進んだ場合はpushを拒否する | 利用者commitの上書きまたはstale commit追加 | Gitのnon-fast-forward拒否を唯一のrace authorityとして使う |
| event authority identity | job開始前cancelを含め、event時draft、event head、action、PRを復元し、draft・closed・stale eventをowner対象から外す | draftのcancelを評価失敗と誤認する、stale headを評価する、またはmerge後のbranchへ未統合H1をpushする | job出力は開始前cancelでは存在せず、runの通常metadataだけではPR head epochを復元できない |
| 生成H1 commit identity | H1 commitと同じGit objectにH0 producer run/attemptを記録し、正規生成H1だけowner再実行を抑止する | push後cancelでH0/H1対応を失う、または手動version commitを未評価にする | checkやartifactを後から作る方式にはpushとの間に原子的でない空白が残る |
| main selected job結果 | mainのRelease候補ではselectedはsuccess、non-selectedはskipped以外を失敗にする | 未評価または無関係なartifactを公開する | Actionsはowner選択と公開可否の意味を知らない |
| 実Windows評価 | WINDOWS選択時だけWindows runnerでinstaller/UIを評価する | 壊れたWindows配布物を公開する | LinuxやGitHubはWindows製品動作を保証しない |
| final-head attempt集合 | current final headを実際に評価した全non-observer attemptを見て、1件の失敗もsame-head successで上書きしない | rerunの偶然のsuccessが未修正の評価失敗を隠し、旧candidateを公開する | latest成功だけでは過去attemptのfailure barrierを表現できない |
| Windows jobとcandidate | Windows skippedは0件、successはown-run candidate exact 1を要求する。ただしLinux distribution skipped・Linux candidate 0件との組だけはcandidate-free非公開成功にする | 非Windowsで不要なReleaseを動かす、またはbinary片側欠落を公開する | run successだけではWindowsの選択有無とartifact保存状態を区別できない |
| merged/completed二信号 | quality先行とmerge先行のどちらも、両条件が揃った信号だけを公開候補にする | merge直後に未完了qualityを失敗扱いする、またはquality先行時に公開信号を失う | 両event間にGitHubの完了順序保証はない |
| tag単位lock後の公開状態 | 同じtagの二信号を直列化し、exact-target tagだけRelease作成に再利用する。同tag Draftは一致する既存assetを保ち、不足assetをuploadして再開する。published完全一致はno-opにし、不一致assetやReleaseだけの状態は変更しない | 二重公開、異なるcandidate bytesの混在、またはpublished済み不完全Releaseの成功扱い | Release作成と複数asset uploadは一つのtransactionではないため、失敗後に一致済みassetを保持し不足分を送る必要がある |
| local台帳schema | PR前は要求・範囲・オラクル・実装状態の欠落だけを失敗にし、remote証拠は完了時に`--final`で確認する | PRでしか得られない証拠をPR前に要求する循環で作業が停止する | local実装確認とGitHub実挙動は異なる観測点を持つ |

この表の4列を満たさない形式検査、identity再照合、poll、retry、証拠専用artifact、同一run内の二次分類は追加しない。
公開lock取得後の競合判定に必要な単回remote再取得を、汎用readbackやretryへ拡張しない。

## 8. 有限の検証規則

- 要件は重複を作らず、同じ観測結果は同じ要件へ統合する。
- 各境界軸の値を最低1回検査する。複数軸は、共有状態または既知の因果関係がある組だけを有限のrisk-based caseにする。
- 全直積、N倍、N二乗、N階乗のcase生成を行わない。
- 文書ごとのSHA一致を合否条件にしない。合否は観測結果、失敗時保持、副作用数、参照整合で決める。
- 製品artifactのhashは評価対象を一意に識別するために1つ記録できるが、内容評価の代用にしない。

## 9. 完了条件

- 上記要件と参照先仕様の間に、同じ入力へ異なる必須結果を要求する矛盾がない。

## G137 Linux / Windows 履歴グラフ同値契約

Session quotaのJSON位置・型、source fingerprint v1、同一minuteの複数source競合、partition単位の復元transactionはDATA ownerの8.7 / 8.7.0を唯一の採否定義とする。以下の復元条件に加えて同節を満たす必要があり、quotaらしいfieldがあるという理由だけで未知形式を採用しない。

`HISTORY-RECOVERY-614`: recorder起動時の保持対象3か月の原本回復は、account partitionの公式quota取得journal・受理済みreceipt・公式取得との保存provenanceを検証できる旧durable observationをquota数値の正本とする。元の時刻・取得順・exact reset/windowを保ち、NULL quotaおよび欠落minuteをDATA 8.7.0の資格と一意規則で復元する。token/modelの証拠がないquota点はmodel source unavailableのまま保存し、model行やtoken 0を作らない。Session token_countのrate_limitsはcached snapshotであり、新しいRemaining実測点、period IDまたは消費のperiod所属へ転用しない。期間長不明を固定7日で埋めず、実測原本のない残量欠測は回復不能範囲として保持する。取得済みSession rangeはmodel消費の原本として、同DATAの全prefix/hash・ID・数値・model・account・公式観測履歴への所属条件を検証する。quota回復とmodel回復は独立した資格であり、片側の回復を双方の完備へ読み替えない。取得元不明の旧残量はDBとbackupへ保持し、公開RemainingはNULLにする。NULLから実測線・未使用判定・確定値を作らず、破線が必要な場合はG137のmetric別欠測規則に従い、元のquota実測不足として期間別に説明する。異なるexact resetが同minuteにあっても、資格を得た各exact keyを保持する。既存非NULL quota、現在receipt、原本、Session checkpointを通常再投影で変更せず、保存をatomic・冪等にする。quotaの差から使用者・端末・model使用量を推定しない。

本契約の全復元条件において、既に旧family keyで保存されたeventだけは、exact model照合を`MODEL-PRICING-463`の固定互換対応で行い、元の旧family keyを保持する。原本のexact IDが同じfamilyに属し、同じroot/pathの検証済みaccepted range内で時刻・lossless 5成分差分が一つのrecordに完全一致することを要する。初回counterを0からのdeltaへ昇格せず、別のfamily・未知ID・複数候補を救済しない。これは旧family記録の再対応付けだけの例外であり、既存exact-ID event同士のmodel一致をfamily一致へ緩めない。照合したexact IDを旧eventのkeyや版名へ書き戻さず、追加利用として二重計上しない。

#### 原本が証明する派生eventの修復

schema移行、通常の非負event追加、公式証拠の変更による所属再判定、原本による数値/model訂正、および既存event集合を変えないminute再投影は、変更できる対象が異なる。以下の全prefix修復はraw由来の数値・model・旧未資格eventの採否を変更する処理を所有する。既に原本資格のあるeventについて、公式証拠の追加だけで所属資格が変わる処理はDATA 8.7「数値eventの公式期間所属」が所有し、この全prefix修復へ含めない。その場合の累計減少や未所属化は負の消費ではなく、同じ数値eventの再分類である。全依存prefix・coverage・selector metadataを同じ新世代へ置換し、raw数値・model・source checkpointは変えない。所属NULLの旧eventをこの区別だけで資格済みとみなさない。

Session JSONLのraw counter/contextと、DBへ保存した計算済みdeltaは同じauthorityではない。以下の通常の再投影は、有効な既存eventと保存合計の一致を前提にする。一方、旧parserの初回累積加算、後退値へのrebaseline、`last_token_usage`の別加算、誤ったmodel/period帰属または重複加算を原本が証明した場合は、その旧派生値との一致を修復の条件にしない。原本から同じ値を再現できないという事実だけでは誤計算の証明にならず、次の全条件を必要とする。

1. 対象は同じpartitionの受理済みphysical sourceの先頭`[0,N)`とし、全accepted rangeのidentity/hash、改行境界、session ID、所有区間をDATAで検証する。別の受理済みfileを旧byteの読取りcontainerにする場合は、DATA「取得済み原本を別の取得済みfileから読む条件」の双方の全prefix hash証明を必要とする。byte内容が異なる原本とのevent再対応付けは、同節の独立した全event照合条件だけを使い、旧physical prefixの検証や同一identityとみなさない。複数generationはDATAの物理同値証明で重複を除く。source外・未受理suffix・byte穴を含めず、sourceのどの旧eventがこのprefixから派生したかを保存pointerで閉じる。
2. そのprefix全体をDATAの版別入力・数値stream kind・response usageの照合/重複排除・raw baseline/delta・context・連続所有区間の規則で再生し、各数値recordのraw前後値と採否理由を保持する。usage/controlの未知形、未照合lineage、評価上限、競合するidentityが残るprefixを完全修復の根拠にしない。有効な完了responseの数値は対応token_countの有無から独立して検証し、DATAのmodel/period資格が不足する候補は既知消費と影響範囲を未処理として保持する。不足を補わず、資格の揃った独立eventだけを記録分として復元できる。候補の残存を完全復元やcoverage完備と表明せず、未知formatの無視とは区別する。初回累積baselineは消費eventにせず、それ以前の不明を保持する。独立した完了response usageはDATAの全資格を得た場合だけ一度採用でき、baseline全量やlastの別加算で代用しない。採用する各eventのmodelとexact reset/windowをDATAの原本・公式観測履歴から一意に決める。cached Session quotaを所属証明にしない。
3. 同じprefixの旧派生event集合と正本から再生した集合を全件比較する。同値の有効eventはkey・原本source pointer・保存価格を保持する。公開用model_sourceの資格はAPI-LIFECYCLE-01の規則で判定し、旧label自体を証拠へ代用しない。旧counter由来eventとroot responseがDATAの限定照合で同一消費と証明された場合は、原本修復として旧派生eventを退役し、root原時刻のeventへ置換できる。旧key・pointer・値はbackupとprovenanceへ保持し、同じ消費を両方に数えない。旧familyの保持は上記の完全一致条件に限る。原本が具体的に否定するdelta、model、periodまたは重複、および全prefix検証で確定deltaとしての採用資格がないと判明した旧値だけをcanonical eventから退役させ、一意に得た正しいeventだけを入れる。前baselineの証明がない初回累積は、その利用が存在しなかったと断定せず、退役後も初回以前の利用を不明として保持する。その位置の履歴をconfirmed-empty/model 0へ置換しない。旧eventの数値に合うよう原本、context、初回baselineを変更しない。新しいeventに旧family名や最新価格を推測転記しない。
4. 変更するeventに依存する同accountの各exact periodについて、対象prefixと他の保持済み有効eventからモデル累積・全変更minute/suffixを再構成する。未取得prefix、未帰属model、未知component/価格は不明として保持し、部分和を完全な期間合計にしない。旧offsetはその全被覆を原本から再構成できたものだけ退役させる。保存済み公式quota/receiptの値・取得順と、独立した他source/periodは変更しない。
5. DATAのverified backupと排他transactionを前提に、旧eventの退役、新event、model totals、全依存minute/sidecar、退役対象offset、公開品質・世代を一括commitする。transaction開始時の対象DB fingerprintが計算入力と異なれば全rollbackし、古い計算を適用しない。checkpointのcursorとsource identityを進めず、有効なcheckpoint値も保持する。計算済みcheckpoint値の誤りを同じprefix/位置のraw stateが否定する場合だけ、その値とknownnessを原本通りに修復し、旧値をbackupへ保持する。別account/epochの位置へ付け替えない。
6. raw Session、accepted range、公式receipt/journal、verified backupは変更しない。全prefixの再適用はevent件数・値・世代ともno-opとし、同じ誤計算を再導入する通常記録経路も同じDATAアルゴリズムへ揃える。資格が不足したprefixでは旧有効値と原本を保全し、未確認の値を正常・completeへ昇格させず、期間別に不足の根拠を残す。

この修復はschema移行とは別transactionである。v14/v15→v16の制約変更で旧event/値を変更しない規則を維持する。以下の通常のquota/prefix再投影における「元event/checkpointを変更しない」は、上記の全証明を伴う派生誤計算の修復を禁止する条件ではない。表示上の後退、token/料金の形、旧合計との不一致だけでこの修復を起動しない。

修復する金額もtokenと独立して資格を判定する。元のexact model・prefix成分・価格根拠が同値の既存観測は保存額のbit値を維持する。token/prefixの誤計算を訂正する観測では、その同じexact model・period・観測に対応付いた保存価格版とcatalogの単価を検証できる場合だけ、訂正したprefixへ`MODEL-PRICING-463`の式を適用する。別modelの旧価格版、期間終端だけにある版、現在のcatalogを欠けた観測へ推測転記しない。版を対応付けられない新規/訂正sidecarは金額不明とし、既知tokenを保持する。SOL/TERRA/LUNAの旧固定列だけは同要件に定めた凍結互換額の資格と式を使い、exact-ID sidecarへその価格を流用しない。未知価格を全期間のtoken復元失敗へ波及させず、未知額を0・旧誤計算額・既知部分だけの全額へ置換しない。

#### 有効な既存eventからのquota・minute再投影

通常cycleでも、新たに受理するSession rangeのhashと所有区間を確認し、その範囲の資格を得たmodel消費・cached windowの補助情報・range・checkpointを同じusage transactionで保存する。同じbatchの補助metadata保存に失敗した場合はbatch全体をrollbackし、modelだけを公開・受領済みにしない。新しいquota実測点は独立した公式取得原本からだけ保存し、その取得・原本保存・receipt受理の失敗では、有効なSession batchの保存を停止しない。既存非NULL quotaと現在receiptをこのmodel処理で上書きしない。通常cycleで過去range全件の再読取りや位置変更後の原本探索を行わず、起動時の原本復旧と処理範囲を区別する。durable JSONの数値表記は、同じSQLite connectionとSerdeの双方で元のbinary64へ一致することを確認し、保存金額や厳密照合guardを丸め差のために変更しない。

model累計の復元はquota回復から独立した操作とする。検証済みpartition backupとaccount lifecycleを前提に、公式reset/window内の帰属済みSession eventの5成分合計が現在の保存合計と一致する場合だけ、raw/durableの既存minute・公式quota観測minute・原本event minuteの累計を各minuteの既知原本prefixから再構成する。閉じた期間では、同じexact reset/windowの保存済み最新source frame、または最後のlossless canonical model観測の5成分が当該期間の全帰属済みSession event合計と一致し、原本rangeの全identity・hashと公式receiptの受信順で期間境界を検証できる場合だけ同じ復元を行う。旧offset退役後もこの既知成分の根拠を失わない。最初の原本event minuteから既存記録終端までのeventがないminuteも、同じ全原本合計の一致を前提に既知累計prefixを復元できる。quotaを観測していないminuteの残量はNULLのまま保持する。元event・receipt・checkpoint・現在合計を変更せず、既存の非NULL残量を同minuteのreceipt値で書き換えず、保存済み価格の根拠を保持し、原本の既知成分からmodel集合の完備を推定しない。異なる公式exact resetの有効なrowを同minuteという理由で置換しない。旧rowの誤所属自体をaccount所有・検証済み原本range・lossless event成分・一意な公式resetから証明し、旧rowの全既知値と対応sidecarを正しい所属へ再構成できる場合だけ、その誤所属rowを同じtransactionで退役できる。receiptの時刻や残量の一致だけを誤所属の証拠にしない。置換範囲を全て再構成できた旧派生offsetだけを同じtransactionで退役し、別期間の補正を保持する。再実行で値・世代を変えない。既存partitionのrecorder起動時に、非選択accountもregistryの所有区間と設定済みSession rootを使い、検証済みbackup後の同じ復元を行う。新たなSession集計cycleを過去accountへ実行せず、失敗しても新しい公式quotaの取得を停止しない。

取得journal導入前の閉じた期間も、DATAの公式観測履歴への所属判定を共通に使う。公式取得との保存provenanceを検証した旧canonical quotaは、元のminute精度の時間区間として使い、秒精度を発明しない。公式取得の根拠を結べないSession snapshotだけでは所属を決めない。全eventの所属と終端5成分の一致が成立したperiodだけを通常再投影し、不足候補がある場合は上記の全prefix修復により資格を得た記録分だけを再構成して不完備を保持する。receipt不在を利用量0、固定period、完全復元へ変換しない。

通常の後着eventも同じ原本prefixの意味で既存minuteの変更suffixへ反映する。後続のquota-only minuteを古い累計のまま残さず、同じ公式reset/windowの派生offsetがあればその先頭まで再構成範囲を広げる。writerは同じwrite transactionのsource合計・各minute prefix・全既存minuteと補正pointの被覆を検証してから旧投影と派生offsetを置換する。新しいquotaを持つminuteへ旧quotaを混ぜない。正規化で検証済みminuteが消える場合はtransaction全体を拒否し、原本・旧投影・補正・checkpointを保持する。通常cycleで原本全文再scanやbackupを追加しない。

Sessionのminute集計終端を最新quota receiptの観測時刻で打ち切らない。scan中にminuteが進んだ場合も、受理したeventをその記録minuteへ保存する。quotaのminuteへ付けるmodel値は、そのminute末尾までの検証済み原本prefixを使い、後のminuteの使用量を古いreceipt時刻へ加算しない。複数receiptがscan間に届いた場合も、同じexact window・所有区間内の未記録minuteを全て対象とし、各minute固有の残量と確認済みprefixを同じtransactionで反映する。新しいtokenがないcycleでも確認済みprefixを保持し、sourceの裏付けがない初期minuteや別exact periodの既存minuteを0や別期間の値へ置換しない。変更suffixより前の保存minuteを古いquotaの再利用で書き換えず、quota未観測のsource-only minuteは残量NULLのまま保持する。

minute復元でも価格の意味を混在させない。`usage_history`のSOL/TERRA/LUNA固定列は通常記録と同じ凍結された互換料金で既知prefixを評価し、exact model IDの料金や版を合算して上書きしない。モデルsidecarは保存済みのexact-ID料金・価格版を独立して保持する。

公式起点は同じ読取りsnapshotのexact reset/windowに一致するreceiptまたは現在quotaから検証し、最初の保存rowやlegacy補正を公式起点へ昇格しない。起点は時間範囲の証拠であり、その時刻のmodel 0や残量100%の証拠ではない。最初の観測より前に値を作らず、原本から復元できる観測は先に復元する。最初の実測が正値/89%ならその値と時刻を保持する。最後の直接観測と表示末尾が同じcanonical minuteに属する場合は、現在・過去periodとも末尾の端数秒を欠測の破線にしない。別minuteの欠測は保持する。

Windows Graph は表示範囲内の最初の実測日時と残量を表示し、初回89%などの記録を100%へ置換しない。account ownership 外の区間は当該 account を記録していない期間であり、区間内利用量は不明と説明する。範囲外の保存履歴の不存在や別利用者の使用を断定しない。破線の一般的な未確認・欠測の説明も保持する。

`G137-GRAPH-01` は、同じstrict validation済み履歴resourceからLinux版とWindows版が
同じ値、同じ線種、同じ連続区間、同じ未使用区間を表示するためのcross-platform契約である。

所有区間の入力はDATA8.6とWIRE `ACCOUNT-SELECT-134`を両platformで同じ意味に扱う。field欠落と空配列はともに未確認であり、全時刻所有にも確定した全時刻非所有にも変換しない。開始nullの区間も、未知の開始から任意の時刻までの正の所有証明にはならない。有限開始を持つ有効な一つの区間がspan全体を含む場合だけ、そのspanの連続所有を証明できる。明示された非所有区間には既存の非所有gap規則を使い、それ以外の所有を証明できないspanは所有未確認として区別する。

所有未確認だけを理由に、元のkey/provenanceで表示資格を持つ保存済みの点を捨てたり0へ変えたりしない。ただし未確認spanをまたぐ線は表示専用の破線とし、未使用帯・連続所有を要する月の日別差分・完全coverageの根拠にしない。点の存在から所有区間を生成しない。これは保存quota自体の値の棄却や新しいSession eventの採用ではなく、点からspanへ証拠を拡張しない表示規則である。原本・registryから所有が回復した場合だけ、同じ新しい受理bundleから線種・帯・月を一括で再計算する。
platformを明記した表示上の例外（`G137-2`のWindows既知0 memberの寄与など）はそのplatformにだけ適用する。
その例外を元観測、model別numeric採否、集計authority、idle証拠へ波及させない。
表示geometryと色はUX owner、wireのschemaとheaderはWIRE ownerが所有するが、次の
入力状態から表示上の事実へ至る意味とfailure isolationは本契約だけが所有する。

以下のisolated pulse判定は累積model metricだけに適用する。時刻順の元row列で連続する3 rowがsame period、当該model/metricの既知値を持ち、両隣接区間にconfirmed gapがない場合だけ行う。元row列は限定1 slot補完の表示上の除外より前の列であり、その欠測rowも残す。別model/metricの欠損は使わず、当該metricの欠損を飛ばしたnearest-finite列も作らない。全tripleの棄却位置を元列から同時に決めた後で、残る値を時刻順にbaseline/recovery判定する。削除後にtripleを作り直す反復処理や、将来点を知らない別順序の判定は行わない。raw rowは削除せず、棄却理由を表示上の当該区間へ保持する。公式Remainingの反転はこの棄却条件へ入力しない。

tokenの検証、大小・同値比較、baseline、差分、family/全model合計は整数のまま行う。各raw componentはWIREのUInt64範囲だが、表示合計にはUInt64を超える値もあり得るため、合計をwrap・飽和・浮動小数化しない。描画座標の近似だけに浮動小数を使い、hoverと日別差分は元の整数精度を保つ。例えば`9,007,199,254,740,992 → 9,007,199,254,740,993`は差1の増加であり、画素上重なっても同値・idleへ変換しない。USDとRemainingは保存/公開されたfinite binary64の値を使い、表示丸めや任意epsilonで別のraw値を同値にしない。金額の表示合計・差分はそのraw値が表す数量を次の規則で正確に扱い、表示時にだけ所定桁へ丸める。


保存USDのグラフ表示集計は、新しいモデル単価からbinary64の参考額を算出する処理と区別する。有効な非負binary64値を、そのbitが表す正確な数量`N / 2^1074`として扱う。指数fieldをE、52-bit仮数fieldをFとすると、E=0ではN=F、1<=E<=2046では`N=(2^52+F) * 2^(E-1)`である。負値・NaN・infinityは受理しない。既知の0は数量0であるが、元のbit値は変更しない。

model別累積差は`N_right-N_left`、family・日・月の合算はこれら非負整数の和とし、途中でbinary64、表示小数、通貨の最小桁へ丸めない。未知値、棄却、未配賦の扱いは各metricの既存規則を適用し、正確な足算のために未知寄与を0へ補完しない。この表現に特定の型・libraryを要求しないが、同じ数量と結果が必要である。geometryへ渡す座標の近似だけは許し、hoverと月の数値計算へ戻さない。

非負値qを小数k桁へ表示するときは、正確な数量に対して`floor(q * 10^k + 1/2) / 10^k`のround-half-upを一度適用する。月合計は未丸めの日別値を合算してから表示し、表示済みセルの文字列を加算しない。G137-7の整数USDはk=0、USD hover/Mainの2桁はk=2とする。Mトークンの整数表示も正確な整数比に同じ丸めを使う。表示桁が0になってもraw不変・idleの証明にはしない。

独立した数値例として、同一modelのaccepted保存USDが`124.831207 → 204.434995 → 943.773212`で両区間を同じ日に配賦できる場合、二つの正確な数量差の和は終端−始端に完全一致する。各差をbinary64へ丸めて足す`818.942005`と、終端差を一度binary64へ丸めた`818.9420049999999`の違いを、保存counterの後退や使用量差として扱わない。本規則の正確な数量を用いた最終2桁表示は両方とも`818.94`である。

`Direct`の資格は当該model/metricの検証済み観測に対するものであり、model集合全体の完備性ではない。`confirmed && models_complete=false`でも既知の有効metricは同じnumeric判定と実測線を使う。集合完備の条件は集合全体の集計資格とidleで別に判定する。metricの採否とbaselineはexact modelごとに完了してから表示familyへ集約し、別modelの値で回復や同値を証明しない。

confirmed gapはDATAが証明した収集欠損の閉区間であり、その後に回復した既知値を消す権限ではない。まずisolated pulseの資格を元のwire gapで決め、numeric採否とbaseline/recoveryを完了する。その後でmetric別のgap適用範囲を求め、numeric採否へ戻さない。gap内でも有効なraw pointは元時刻・元値のanchorとして保持する。同じexact periodの連続するcanonical minute（timestamp差60秒）に、当該metricの検証済みaccepted観測が両方ある区間は、そのmetricについて回復済みの表示coverageとする。modelは`confirmed`、Remainingは受理済みraw quotaを必要とし、`legacy-unknown`、補間・hold、棄却値、限定1 slot除外はこのcoverageを証明しない。gapからこのcoverageを差し引いた残りだけを、そのmetricの未回復gapとして線種と月配賦へ使う。疎な両端だけで中間を回復済みにせず、部分回復は他metricのgapを消さない。idleは全model tokenとRemainingの未回復gapが全てない区間だけを使う。以下の線種・idle・月配賦の「gapを跨ぐ」はこの未回復部分を意味し、isolated pulseの元gap判定とは区別する。元のledgerとwireのgapをUIが書き換える規則ではない。

期間の通常選択一覧は、現期間と、受理済み期間範囲内の公開modelに正の`total_tokens`がある期間を掲載する。modelが不完全または`legacy-unknown`でも正の記録は残す。さらに、以前のcanonical model観測に正のtokenがあり、原本修復でmodel/periodの証明が不足して公開値が不明になった期間は、未解消の利用記録がある期間として掲載を保持する。元のcanonical key・元観測範囲の先頭/末尾時刻・退役記録へのpointerを伴うこの保持資格は数値ではなくmetadataとし、修復と同じtransactionで保存する。公式originが不明なら不明のまま保持し、期間長や元時刻を生成しない。最新model値・0・新periodを作らず、取得directoryの既知exact IDを使う。全ての元正値が検証済み原本で別period由来・重複・非消費baselineと解決された場合だけ、この保持資格を解除する。未解消claimが1件でもあれば解除しない。この保持対象を除き、quota観測だけ・欠測区間だけ・初めから正のmodel利用記録がない過去期間は通常一覧に載せないが、未使用や利用量0を確定したとは扱わない。原本・全exact ID・直接history pageを保持し、後から正のtoken記録が入れば次snapshotで再掲載する。日・週・月など任意時間窓の取得directoryは選択一覧と分離し、quota観測や欠測区間を失わない。31日より古い保存期間も同じ条件で選択でき、互換detailsの直近31日projectionは維持する。Windowsの選択一覧は公開`start_at`を表示タイムゾーンの日付でまとめ、公開されたexact IDを個別に選べる。見出しの展開で選択を変更せず、periodの統合・累計加算・raw値の変更は行わない。選択追従と選択項目消失時の戻り先はUX ownerの規則を使う。旧serverに選択専用resourceがない場合だけ旧directoryを互換表示し、その場合は新しい掲載条件を保証しない（#646）。

selectorの掲載は次の入力例でも同じ結果を必要とする。いずれも同accountの保持窓内であり、「非掲載」は未使用確定を意味しない。

| 入力 | selectable | 数値の扱い |
| --- | --- | --- |
| current、model未取得 | 掲載 | 不明のまま |
| 過去、公開modelに正のT | 掲載 | 元のsource/knownnessを維持 |
| 過去、旧正値の原本不足で現在の数値は不明 | 掲載を保持 | 原本不足を示し0にしない |
| 過去、旧正値は全て別period・重複・非消費baselineと解決、正の公開値なし | 非掲載 | 当該periodの利用0を新たに確定しない |
| 過去、quotaだけ、旧正値の未解消記録なし | 非掲載 | quota原本・取得directoryは保持 |
| 過去、model未取得または実測T=0、旧正値の未解消記録なし | 非掲載 | 未取得と既知0を区別して保持 |
| 原本回復後、当該periodの正値を再確認 | 掲載 | 資格を得た数値だけを公開 |

selectorの日付group順、group内順、前後移動、currentがない場合の初期選択はUX ownerだけが定める。公式resetの後方移動とwire版ごとの異なる配列順を許しても、同じ選択用配列から掲載された全selectable IDへ到達できることを不変条件とする。取得directoryだけにあるIDはこの移動対象に含めず、日・週・月の窓と直接history取得の対象として保持する。日時の大小を公式reset受理やcurrent判定へ流用しない。

現行periodの表示範囲は公式`reset_at - window_seconds`から検証する。
period所属とmodel sidecar結合は公開されたexact `reset_at`を正本とし、同じ時間窓にあるという理由で
別の公式periodの観測を現行periodへ付け替えない。rawのsource reset/timestampはprovenanceとして保持し、
旧混合履歴の正規化はDATA ownerの`HISTORY-CANONICAL-134`、現在quotaは`QUOTA-DURABLE-604`に従う。
account lifecycleや外部利用を推測してmodel数値を付け替えない。
現行periodの先頭から最初のlocally-owned model observationまでに保存されたfiniteな`remaining_percent`も、providerから
直接取得したraw quota観測として元時刻・元値のまま履歴へ保持する。Session/model ownershipを確認できないrowは
`model_source=unavailable`、model数値なしで公開するが、そのmetadataだけでraw Remainingを欠損または予測へ降格させない。
有効なraw Remainingは他のraw anchorと同じ3px実線のauthorityであり、raw-null、競合、範囲外、明示gapまたは棄却異常だけを
1px破線にする。quota-window開始前のlifecycle interval、`A→B→A`の再入場、task activityはいずれもraw quotaを隠したり
線種・形状を変更したりしない。ownershipまたは再入場境界を一意に決められない場合はmodel帰属だけをfail-closedとし、raw
quotaを特定modelの消費へ付け替えたり、model 0へ補完したりしない。

1. `G137-1`: 選択期間の全accepted pageに実際に掲載された`models[].model`の和集合を
   model universe `U`とする。model名はexact wire stringをUTF-8 unsigned byte列の
   case-sensitive lexicographic順（Unicode normalizationなし）に並べる。未掲載の
   `ASTRA`その他を既知modelとして創作せず、`U`が空ならmodel activityと未使用区間を作らない。
2. `G137-2`: 公開・DB viewのmodel数値は、同じmodel keyを持つ直接観測(`model_source=confirmed`)の
   finiteかつnon-negativeなraw値だけを保持する。`reconstructed-from-session`、`unknown`、`unavailable`は
   model key/sourceと欠損metadataだけを公開し、`total_tokens`、dollar、内訳などのモデル数値を公開しない。
   `legacy-unknown`は保存済みの同じmodel keyの数値だけ表示へ投影でき、集計・予測のauthorityには利用しない。ただし
   同一runの全raw rowに同じ非空model集合とlosslessな`total_tokens`が存在し、全model値とraw Remainingがexactに
   不変で、欠測・異常・confirmed gapがない場合に限り、その保存raw vectorをidle判定に利用できる。
   掲載されないmodelを0へ補完しない。v1/v2のfixed-column rowも、保存されたmodel fieldをstrict parseした
   `legacy-unknown` raw vectorとして上記の限定idle条件だけに利用し、未掲載modelを創作しない。
   Windowsの既知family表示では、同じreset内で既に掲載された同一model keyの当該表示metricが明示0と観測済みで、
   そのmetricの以前の正値がなく、非syntheticの`confirmed && models_complete=true`の非null model集合からそのkeyが省略された場合だけ、
   そのmemberの当該metricの表示合計への寄与を0として現在掲載されたmemberと集約し、そのmetricの実線とhoverへ渡す。tokenとUSDでこの資格を独立に判定し、一方の既知0を他方の未知値へ転用しない。
   全memberがこの条件で省略されたfamilyは合計0となり、後から実際のモデル版の利用が始まっても旧0 memberを欠測扱いにしない。
   raw model行・token authority・集計額・idle証拠へ0を追加せず、`U`を増やさない。初掲載前、未知family、未完全集合、
   未提供集合、既知0でないmemberの欠落、以前に正値があるmemberの消失は補完しない。明示gapと非所有区間の線種・hover制約を維持する。
   旧family集計欄（例: `LUNA`）の明示0から同じfamilyの正確なmodel ID（例: `gpt-6-luna`）の保存実測へ切り替わる場合は、同じ表示系列の表現切替として扱う。旧0欄を独立した未取得modelとして再要求せず、現在の実測値を実線とhoverへ渡す。現在の集合が`legacy-unknown`で不完全でも、この名前の切替だけで実測を未確定扱いにしない。旧familyの当該metricに正値があった場合や、実モデルの欠落・未知価格・gapを0で補完する根拠にはしない。tokenと保存USDの確定性は独立に判定し、未知価格で記録済みtokenを破線化せず、token未提供で保存USDを欠測化しない。
3. `G137-3`: 同一periodの累積model値は減少しない。baselineはmodelごとの直前accepted finite値とし、
   source completenessを問わずbaseline未満のraw値は計測異常として表示値へ採用せずbaselineを更新しない。
   隣接する実測点`left,middle,right`で`left <= right`かつ`middle < left || middle > right`となる
   isolated pulseも計測異常として`middle`を採用しない。残る候補を時刻順に走査し、最初のfinite non-negative値を
   baselineとする。以後、候補がbaseline以上ならその値をacceptedとしてbaselineを更新し、baseline未満なら当該
   metricだけrecovery stateへ入り値を採用しない。recovery中もbaselineを固定し、最初のfinite non-negativeかつ
   `candidate >= baseline`の値だけをaccepted recoveryとしてstateを抜ける。それまでの候補は全て異常である。
   model pointは`Unknown`（当該metric欠測）、`Direct`（同じmodel keyの検証済み観測。原本prefix復元を含む）、`Interpolated`、`Held`、
   `Rejected`を区別する。`Direct`だけが集計に利用でき、保存済み`LegacyObserved` rawは`G137-2`の表示と`G137-5`の限定idle判定だけに利用できる。`Interpolated`、`Held`、`Rejected`はUI presentation-onlyの
   破線または欠損表示とする。この破線はDBまたは取得記録の欠損・異常を示すNG表示であり、正常な実線の代替ではない。
   保存済み累積ドルを通常表示のanchorとするが、参考額はtokenの別metricでありidle authorityにはしない。tokenが同値という条件だけで保存USDを左端額へ補正したり、欠けたUSDを確定額へ昇格したりしない。USDも自身の採用済みanchor・欠測・後退から線種を決める。token同値でUSDが変わった記録は価格版、原本eventと保存prefixを調べ、変化理由を特定する。原因未確認を価格改定や追加利用と断定せず、描画補正で隠さない。将来anchorから過去の額を作らず、未知区間は既知両端の間だけ表示専用に補間する。右anchorがなければ最後のacceptedドルをperiod endまで破線holdする。現在または単一versionの単価で過去の累積token全量を再評価して保存ドルを置換してはならない。
   rejected値の期間は直前baselineを細い破線holdし、accepted recoveryへ細い破線で接続する。period endまで復帰が
   なければ同じbaselineを終端まで破線holdする。これをraw tokenとraw dollarへ独立に適用し、tokenだけ異常なら
   token線だけ、dollarだけ異常ならdollar線だけを破線にする。他metricと他modelのaccepted実測は保持する。
   idle判定の根拠はaccepted raw token vectorとraw Remainingだけとし、confirmed gap、欠測、または観測値の矛盾・異常が
   ある区間はidle不可とする。`legacy-unknown`は、同一runの全rowに同じ非空model集合とlosslessなraw tokenがあり、
   全model値とraw Remainingがexactに不変な場合だけidle authorityに含める。task lifecycleは値変化の代替証拠にせず、
   exact equalの区間をactive metadataだけで使用済みへ変更しない。補間、hold、smoothing、予測はidleの根拠にしない。
   period境界でだけ累積のlineageを切り替え、period内補正を垂直落下または低い新lineageとして表示しない。0起点を描くにはその値の観測が必要であり、境界だけで生成しない。
4. `G137-4`: 同じmodel keyが両endpointの保存rawに存在し、当該表示metricの
   異常またはrecorder gapを跨がない区間だけを、そのmodelのcontiguous measuredとする。他modelの出現／消失、
   `confirmed`同士の共通modelはmodel集合の完全性だけを理由にこの区間を破線化しない。`legacy-unknown`は集計authorityには
   しないが、両endpointと全interior rowに同じ非空model集合のlossless raw tokenが存在する区間はidle authorityにできる。正常な観測endpoint間はtimestamp差だけで
   欠損へ降格せず、利用中の同値・増加は同じ3px実線で結ぶ。後述の10分以上の確定idleに含まれる同値区間だけは
   idle bandと同じX範囲の1px水平実線へ分離する。exact 60秒の1 sampling slotだけが`unavailable`で、その前後が
   same reset、前後が`confirmed && models_complete=true`の非synthetic観測で同一model集合を持ち、全modelのraw token tupleがexact equalかつnumeric採用済み、前後のraw Remainingがfiniteかつbitwise equal、元confirmed gapなしなら、
   tokenとRemainingはその1 slotを表示上だけ跨いで前後を3px実線で結べる。USDはmodelごとに、さらに両端のaccepted raw USDが既知かつbitwise equalで、そのmetricの異常を跨がない場合だけ同じ例外を適用する。token同値だけでUSDの明示欠測を実線へ変更しない。
   中央rowのRemainingはNULLまたは同じbit patternの有効値だけを許し、異なる観測値や不正値を除外しない。NULLを既知Remainingへ変更する規則ではない。
   この資格判定は元row列のnumeric採否完了後にmetric別で行い、元row列を全metricから削除しない。例えば同じtokenとRemainingでもUSDが`10,欠測,20,11`なら、20をisolated pulseとして棄却せず、20をbaselineへ受理して後続11だけを後退として扱う。USDの10→20間の欠測は破線のままである。
   この限定補完はDB、history、API rawを変更せず、hoverの欠測を既知値へ変えず、当該rowを直接観測または新anchorへ昇格させない。それ以外の明示欠測は1px破線とする。raw値は元時刻・元値の証拠として保持し、
   modelの同値区間は両端時刻・値を通る水平線を維持する。利用が増えた隣接観測間だけを単調かつovershootしないPCHIP
   （UX §4.2の導関数・端点式）で滑らかにつなぐ。当該modelの明示的欠測、後退／回復、confirmed recorder gap、bounded／terminal holdは既知endpoint間を1pxの
   破線で連続補完する。補間・holdはUI presentation-onlyで、API/DBへ書き戻さず、直接観測または集計・idleの
   根拠へ昇格させない。
   period endはaccepted periods resourceの同じpairにあるexact `end_at`とし、currentか
   completedか、local clockがどこかによって置換しない。accepted値が1点以上ある系列はperiod内の最初の
   accepted pointからperiod endまで空白区間を作らず、同一X座標のsegmentを作らない。最初のaccepted point
   より前は創作せず、accepted値が0点のmodelは線・右端labelを、全modelが0点なら未使用帯も作らない。
   最終accepted point後はperiod endまで細い破線holdとする。ただし公開終端と最終実測が同じcanonical minute内の場合は、現在・過去periodとも次minuteの欠測ではないため実測点で線を終える。公開終端・横軸は保持し、その端数秒へ実線や予測値を作らない。後続minuteの欠測はこの例外に含めない。破線は予測表示であり、それ単独では
   未使用の証拠にしない。
   model集合変更は、未掲載または新規model自身の線と未使用判定にだけ反映し、Remainingの形状・線種には反映しない。
   集合変更区間でも共通modelのaccepted値は実線を維持し、raw Remainingの両endpointがacceptedなら
   Remainingも実線を維持する。`source_mismatch`は欠測または利用不能という主原因があるbridgeの補助理由に限定する。
5. `G137-5`: idle判定はsame `reset_at`のperiod内でのみ行い、run全体が一つの確認済み連続所有区間に含まれることも必要とする。所有metadataの欠落・空配列・開始nullを無制限の所有へ変換しない。灰色帯の意味は、保存済みの全model tokenと利用枠が観測上不変である区間である。ローカルtask停止だけでは全利用量の未使用を証明できないため、`task_active_since_previous=false`を単独の帯生成・欠測補完・増分後送りの根拠にしない。task lifecycleの元区間は保持し、原本との矛盾調査に使うが、以下のraw vector条件の代わりにはしない。
   両endpointと全interior raw rowに同じ非空model key集合が揃い、
   各rowが`model_source=confirmed && models_complete=true`または保存済みlossless token vectorを持つ`legacy-unknown`であり、
   全modelのraw `total_tokens`がexact equal、raw `remaining_percent`がfiniteかつbitwise equalで、confirmed gap、欠測、
   観測値の矛盾がないことを必須とする。`unknown`、`unavailable`、欠損・補間・hold・smoothing・予測値をendpointまたは矛盾なしの証拠にはせず、
   lifecycle metadataをraw値変化またはraw非変化のauthorityにしない。ただしexact 60秒の1 sampling slotだけが
   `unavailable`で、前後の観測・中央Remaining・元gapが`G137-4`のtoken/Remaining限定補完条件を全て満たす場合は、周期不足／sampling jitterとしてidle比較でそのrowだけを例外とし、idle候補を
   分断しない。これはUSDの表示資格へ波及せず、numeric採否の元row列とhoverから当該rowを削除しない。この限定補完はDB、history、API rawを書き換えず、`unavailable` rowをauthorityへ昇格させない。2 slot以上連続、
   前後値またはmodel集合の相違、reset境界、confirmed gap、`unknown`、その他の不完全rowは候補を分断する。
   rowのtimestamp不連続だけはgapまたは利用の証拠にせず、正常なdirect endpointが上記条件を満たすintervalを分断しない。上記条件を満たす連続runが10分以上の場合だけ、その
   run全体をsession-levelのidle bandとして表示し、同じX範囲で既知・accepted・不変の当該metricを持つmodelとRemainingを1pxの水平実線で表示する。USDの実測増加は元値を通る3px実線を維持し、未知価格のmodel USDは実測線を作らず、欠測・棄却異常には破線規則を優先する。帯がUSDの既知性や正常性を証明することはない。
   USDの変化・欠測・異常をidleの開始・分断・終了条件にしない。token metricではbandと水平実線を同じ範囲にし、USD metricでは各保存額の既知性・変化・異常を優先する。10分未満のrun、cadenceの数や観測点数だけでの確定、direct endpointを
   欠く欠測時間だけのbridgeはidleへ昇格しない。画面幅やpixel数によって閾値を変えない。
   画面幅、pixel丸め、gridまたはsegment境界を理由にbandを削除・周期分断しない。

   periodの和集合`U`に存在しても両endpointで未掲載のmodelはその区間へ創作しない。表示丸めまたは単価から未使用を
   推測せず、ドルはidle authorityから完全に除外する。tokenの増加、片endpointだけのmodel欠落、model集合変更、unavailable、confirmed gap、
   token異常、Remaining変化、Remaining anomaly／terminal holdは未使用へ読み替えない。task lifecycleのactiveだけを
   使用の証拠にはせず、上記のexact equalなDirect値による判定を変更しない。Remainingがraw-nullまたは予測値の区間は、
   finite raw Remainingがないためidle候補にできない。
   lifecycle unknownだけでは他の完全な証拠をidleへ昇格しない。
   task停止の証拠があっても、同じ区間に保存済みtoken増減または利用枠の変化がある場合は、その区間を未使用帯に含めない。model単独の水平線はそのmodelのtoken同値で判定し、別modelや利用枠の変化を混同しない。
   raw token vectorの同値はmodel keyごとのT/I/C/W/Oのlossless tupleで比較する。Tは既知必須、optional成分は既知値と既知/不明の両方が一致しなければならず、不明を0へ変換しない。両側とも不明のcomponentがある帯は、保存された既知tokenが不変であるという意味に限り、未知componentまで未使用と証明しない。この比較を限定1 slot補完にも適用する。
   これらは当該局所区間だけを除外し、前後の独立した候補まで棄却しない。
6. `G137-6`: non-nullでfiniteな0..100のRemainingをaccepted raw観測として元時刻に保持し、繰返しrawを証拠として変更しない。同一exact reset内の増加・減少・反転はそれぞれ報告された変化であり、順序・形だけでAPI anomaly、reset、欠測へ変更しない。schema/domain不正、同一minuteの矛盾、source不一致等でDATA/WIREが採用できない点だけを未知・異常とし、有効な別時刻のraw観測を棄却しない。公式応答とDB/APIが異なる場合はbackend不良として原本から修復する。値の増加理由が原本にもなければ「公式報告値の増加」までを説明し、credits消費、手動reset、他端末利用などの原因を推定しない。

   Remainingのaccepted raw値は全て元時刻・元値の証拠として保持し、token増分、task lifecycle、model availabilityで
   移動または置換しない。Windowsの表示geometryは同値反復も含む全accepted観測点を通過し、横ばい区間と変化時刻を保つ。
   隣接観測間だけを各区間の増加・減少・不変の向きを保ちovershootしないPCHIP（UX §4.2の導関数・端点式）の3px実線でつなぎ、横ばいを傾斜へ変えない。反転点も元値を通過し、単調性のために隠さない。
   Linuxは既存のsampling同値反復を省くgeometryを維持する。確定idleは
   bandと同じX範囲をexactな1px水平実線として分離し、明示欠損・異常・reset/correctionの境界は平滑化で跨がない。変化量による別閾値は設けない。
   これはread-timeのgeometry生成だけであり、DB、history row、gap ledger、raw値を書き換えない。timestampの疎または
   sampling jitterだけでは欠損・予測へ降格しない。

   公式windowから確認できる`period_start`/`origin_at`は横軸の開始を定めるが、開始時の値を定めない。Linux/Windowsとも最初の実測以前に100%・model 0のsynthetic anchorを作らない。起点を確認できても同じである。起点から最初の観測まで欠測がある場合は、値を持たない未記録区間として残し、DATAの原本復元でのみ実測を回復する。初回値、全raw、hoverを変更しない。公式起点が不明なら最初の実測を開始として扱い、不明な起点をlocal clockやviewport左端から作らない。

   raw-null、`Held`、`Rejected`、上記の限定補完条件を満たさない明示的unavailableまたはRemainingの未回復gapを含む区間だけを予測とし、両側のaccepted raw
   anchor間は同じ単調補間geometryを1px破線で描く。予測した中間値を新しいanchorへ昇格しない。token anomalyや
   task activityはRemainingの形状・配分・線種を決めない。明示reset/correctionではspline runを分割し、境界を跨いで
   滑らかにつながない。
   raw Remaining labelは変更しない。DATA/WIREが確認したRemaining異常を跨ぐdropは配分せず、既知endpoint間を破線bridgeして
   未使用帯を作らない。右anchorなしのterminal nullは直前effectiveを
   carryする。accepted raw Remainingが1点以上あれば、最後のeffective pointからexact period endまでを破線holdとする。ただし両端が同じcanonical minuteの場合は`G137-4`と同じく最終実測点で止め、端数秒へholdを作らない。後続minuteの欠測では時間幅のあるholdを保ち、空白や同一X座標の垂直落下を作らない。`Remaining`のeffective値から
   model系列の値またはそのperiod tailを外挿しない。
7. `G137-7`: model線はaccepted raw同士の当該metricのexact値の増加と、10分未満またはsession-level idleではない不変区間を同じ3px実線とし、
   確定idleに含まれる当該metricの不変区間だけをbandと同じX範囲の1px水平実線へ分離する。timestamp差だけでは破線化しない。raw列を変更せず、USDも自身のaccepted観測を使う。
   記録済みmodel累計の同値区間は元の両端を通る水平線とし、利用が増えた隣接観測間だけを単調PCHIPで滑らかにつなぐ。途中に
   当該modelのunknown rowがあるnearest-finite接続、限定補完条件を満たさないraw-null補間点の両側、
   monotonic hold、bounded/terminal hold、synthetic tailは1px破線とする。raw quota同値のcontiguous区間は
   model availabilityと独立した実測実線である。raw-nullの`Interpolated`だけを欠測破線とする。model線は当該modelの当該表示metric anomaly、Remaining線はRemaining
   anomaly、全線はgapを跨ぐ区間だけを破線bridgeとし、別metricのanomalyを正常線へ波及させない。Linuxの右端model
   labelは最後のaccepted raw model値を表示し、実測0を未掲載と同一視して隠さない。Remaining labelは最後の
   raw観測時刻におけるeffective値を表示する。Windowsは右端ラベルを廃止し、選択した近傍実観測時刻の
   全表示系列をホバーで確認する。整数Tokens・accepted保存Dollars・raw Remainingの表示条件は
   UX ownerの§4.2に従い、欠測を補間や0として表示しない。

   Linux / Windowsのfamily表示・hoverは、同一reset period内でその時点までに掲載されたmodel keyを対象にする。period全体の和集合Uを、初掲載前の時刻に既知のmember集合として使わない。
   後から初掲載された正確なmodel IDを過去のfamily集合へ遡及追加して、保存済みの旧familyや旧版の値を消さない。
   一度掲載されたmemberの後続欠測・価格不明はunknownのままとし、未掲載をrawの0へ変換しない。
   旧familyと正確なIDの独立した保存値は既存の価格契約に従って表示上だけ合算する。
   tokens/dollars、週期間/24h/7d、hoverへ同じ表示membershipを適用し、raw token、idle・予測authority、DB/APIは変更しない。
   familyが保持値と新しい記録値を含む場合も、描画用の利用量を古い値の水平延長へ置換しない。途中点と終端の記録済み寄与を保持し、当該表示値に実際の欠測・補間・保持を含む区間だけを破線とする。表示名の切替や別系列の欠落だけでは実測値を破線化しない。後続の完全な観測が届いても途中の寄与を消さず、rawやidle判定を変更しない。

   Windowsの週期間・24時間・1週間では、各実reset期間の保存済み利用量を選択metricに連動して表示する。ドル選択は保存USD、トークン選択は保存total_tokensの全model合計をM単位（1 M = 1,000,000 tokens、100万未満は桁区切り付き整数）とする。月表示はUX正本の全モデル日別合計カレンダーとする。
   固定7日や表示窓の幅から期間を推定せず、canonicalなperiod start/endを使う。モデルの表示ON/OFFによって額を変えない。
   同一期間の最新の非syntheticモデルraw観測から全raw model keyの選択metric値（保存USD、旧ASTRAは既定式／保存total_tokens）を一度ずつ合算する。利用枠だけの後続観測はモデル利用量の記録を取り消さず、保存済み合計を消さない。
   最新のモデル観測が不完全でも過去の完全集合へ置換せず、その観測の有効な記録だけを使う。
   補間・hold・smoothing・予測値や過去の完全集合へfallbackしない。右端が窓内で途中まででも保存済み期間全体の累計を表す。
   不完全な集合、価格不明、無効・減少により採用できないmodel値がある場合は、そのmodelを額から除き、他の有効な額だけを表示する。
   保存額がある場合はその値を使い、再計算額との二進丸め差を別の履歴観測として比較しない。異なる観測同士の後退を任意の許容差で正常化しない。有効な額が一つもなければ「—」とし、0へ置換しない。
   ただし過去の掲載keyもない明示的なconfirmed空集合は選択metricの確定0とする。legacy-unknownの保存値の合算は表示専用例外とし、完全性を昇格させない。
   表示位置はグラフと横軸の下に設ける細い金額欄とし、各periodの可視区間の中央へ、読みやすい控えめな大きさの整数USDまたはMトークン値を置く。
   USDの小数は表示時だけ四捨五入する。トークンは100万以上を表示時だけ四捨五入した整数のM表記、100万未満を桁区切り整数とし、「期間合計」「記録分」「未確定」などの説明や開始日時を値の欄へ付加しない。
   24h/7d窓の左端が前periodの途中でも、そのperiod全体の値を可視区間中央へ表示する。
   金額の大きさはfooter全体ではなく各periodの可視幅に合わせ、中央位置を他periodへずらさない。隣の金額との余白を保ち、線や時刻目盛りに被せたり、金額を切ったりしない。期間区切りは金額と同じテーマ解決後の黄色系色でplot内だけに描き、金額欄に縦線を描かない。日区切りの細い白系補助線と区別する。
   履歴0点の場合は既存の未取得・履歴なし表示を維持する。金額はテーマへ追従し、個別modelの右端値やleaderを復活させない。
   金額用tooltip、追加説明、追加警告は導入せず、既存の系列hover・収集状態・API/DBの精度を変更しない。
8. `G137-8`: Graph candidateのpairはperiods応答のexact
   `Codex-Info-Published-Pair`を`P`とし、全history success pageの同headerがASCII
   case-sensitiveで`P`と一致した場合だけ全pageをatomic publishする。欠落、malformed、別値`Q`、
   同一periodのduplicate timestamp、repeating cursor、1応答のbyte境界またはhistory sampleの1 page最大1,024点を超える場合はcandidate全体を破棄しlast-goodを保持する。総page数に固定上限を置かず、全pageを前進して取得する。
9. `G137-9`: 保存cursor付きhistory requestが、pair headerなし、no-store、JSON content type、exact
   Content-Lengthかつ1KiB以下、exact key集合`api_version,error`と値`v1,stale_cursor`を持つHTTP 400を
   返した場合だけ、同cycleで同periodのcursorなしhead取得をexactly 1回行う。成功時だけ新pairの全pageを
   atomic publishする。回復取得失敗時は旧cursorを保持してreset-requiredとし、次cycleは旧cursorを送らず
   headから開始する。その他の400、malformed response、404/500/transportは同cycle retryしない。
10. `G137-10`: Graphの成功・全失敗経路はMain details/quota/model totals、Threads、authと各々の
    error/retry/suppression latch、cursor、timestampを変更しない。Graph失敗が変更できるのはGraph error、
    loading終了状態と前項のreset-requiredだけで、scene、受理済みの描画対象period、last-goodを部分更新しない。
    期間選択は`idle -> loading -> ready | confirmed-empty | failed`とし、選択先periodのsame-pair全history pageと
    projectionがatomic publishされるまでloadingを維持する。loading中はspinnerを表示し、`記録なし`または
    `利用不可`を表示しない。0件を確認した完全pageだけをconfirmed-emptyとし、失敗をemptyへ変換しない。
    操作で指定した要求ID/mode/metricと、完成済みsceneの受理ID/mode/metricを別の状態として扱う。
    要求の入力反応・selector highlight・追従intentは即時更新できるが、旧sceneの軸・hover・期間値を
    新しい要求のdataとして改称しない。旧sceneがあるloading/failedでは、旧sceneの対象期間・単位を保持し、
    要求先と「直前のグラフを表示中」をloading/error表示で明示する。sceneがない場合はloading/errorだけを示す。
    失敗後も要求intentを保持し、次の通常Graph周期または明示選択で取得する。同callbackの無限retryはしない。
    前後移動のindexは要求IDから求め、完成時だけsceneと受理ID/metric/軸/hoverを一括置換する。
    X/Rust、Windows Core/GraphScene/ScottPlot、保存fixture E2Eは同じliteral inputと期待区間を独立に検証し、
    一方のhelper出力を他方の期待値にしない。Release後の同値確認は同じsource SHAと単一accepted pair・period・
    latest timestamp/model/quotaを照合し、platform固有binary SHA-256が異なることを不一致と扱わない。

`G137-1`..`G137-10`の最小literal oracleは、accepted periods/historyのpair headerが全てASCII exact `P`、
有効な基準時刻`T`を持つcompleted period `id=T+600,start_at=T,end_at=T+600,current=false`、全rowがstrict
parse済みでtimestamp重複なしとして次の相対offsetを固定する。保存fixtureでは`T=2200000020`を使用し、表と期待segmentの
`0..600`はwire timestampそのものではなく`timestamp - T`である。model値は
`name:(raw total_tokens,raw total_dollars)`で表し、全記載値をliteral期待値とする。
以下の正常線・idle・日別配賦の固定例では、所有入力を別に示す例を除き、選択accountの
`ownership_intervals=[{start_at:T,end_at:T+601}]`を明示入力とする。periodの終端T+600もこの有限区間内である。
これはfixtureの前提であり、実入力のfield欠落・空配列・開始nullをこの値で埋める既定値ではない。
`V4`は`{ASTRA:(0,0),LUNA:(0,0),SOL:(20,0.000020),TERRA:(0,0)}`、`V3`は
`{LUNA:(0,0),SOL:(20,0.000020),TERRA:(0,0)}`を表す。

| offset | model evidence | `models_complete`,`model_source` | raw Remaining | `task_active_since_previous` | 固定期待 |
| ---: | --- | --- | ---: | --- | --- |
| 0 | `V4`の`SOL:(10,0.000010)`版 | `true,confirmed` | 90 | `null` | 最初のaccepted point。0から線を創作しない |
| 60 | 同上 | `true,confirmed` | 90 | `true` | 値不変の3px実線。active metadata単独は利用証拠にせず、この1分区間は10分閾値未満なので未使用帯にはしない |
| 120 | `V4` | `true,confirmed` | 89 | `true` | token増加かつquota低下。`[60,120]`はactive |
| 180 | `V3` | `false,legacy-unknown` | 89 | `false` | model集合変更／不完全vector。未使用帯なし |
| 240 | `V3` | `false,legacy-unknown` | 89 | `false` | 同値だが10分未満なので未使用帯なし |
| 300 | row自体なし | — | — | — | timestampの疎だけでは明示欠測にしない。240→360は正常な保存値同士の実線 |
| 360 | `V3` | `false,legacy-unknown` | 89 | `false` | 同値の正常anchor。10分未満なので未使用帯なし |
| 420 | `V3` | `false,legacy-unknown` | 89 | `false` | 同値だが10分未満なので未使用帯なし |
| 480 | `V3`の`SOL:(5,0.000005)`版 | `false,legacy-unknown` | 89 | `false` | token/dollar回帰異常。表示は20を破線holdし未使用でない |
| 540 | `V3`の`SOL:(21,0.000021)`版 | `false,legacy-unknown` | 88 | `false` | token/dollar recovery。破線bridgeで未使用でない |
| 600 | sampleなし | — | — | `null` | 全系列をexact period endまで破線hold |

このliteral oracleの明示停止runは10分未満のため、期待する未使用帯は`[]`である。明示的な`false`があっても、same `reset_at`、両endpointの
`confirmed`＋`models_complete=true`、同じmodel key集合、全raw tokenのexact equal、finite raw Remainingのbitwise equal、
confirmed gap／欠測／直接観測値の矛盾なしが全て揃わない区間は帯にしない。active metadataだけは値変化の証拠にしない。
このliteral oracleの`legacy-unknown`列はmodel集合変更、途中欠測、token異常またはRemaining変化を含むため、限定idle条件を満たさず、集計・予測・idleへ使わない。
空白区間と`x1 == x2`のsegmentは0件とする。明示的な欠測endpointはgrayでbridgeしないが、正常なaccepted raw endpoint間のtimestamp sparsityだけでは実線またはidle bandを分断しない。
追加反例として、同じドル値でもtokenが`100→101`かつdollarが`1.00→1.00`、accepted Remainingが局所的に
`90→89`、tokenのisolated pulse、confirmed gapを跨ぐ同値endpointはいずれも未使用0件とする。公式Remainingが`90→95→89`なら全3点を実測のまま保持し、両変化区間はidleにしない。反転を理由に95を棄却したり、90へholdしたりしない。別の反例として、時刻0/600/1200の3点が同じ完全token vectorとRemainingを持ち、dollarだけ`1.00→0.99→1.00`のisolated pulseなら、
dollar線だけを破線hold／recovery、直接観測のtoken線を1px水平実線、未使用帯を`[0,1200]`に表示する。逆にtokenだけ`100→90→100`ならdollarが
同値でもtoken線だけを破線にして未使用帯を表示しない。
valid-anchor smoothing oracleは、時系列tokenが`10,20,20,50`でraw Remainingが`100,100,100,99`なら、
raw/effective列を`100,100,100,99`のまま保持する。Windowsは表示geometryも最初の3点まで100で水平に保ち、最後の隣接区間だけを99へつなぐ。
Linuxは既存どおり中間の同値raw点を表示geometryの必須通過点にしない。正常区間は3px単調PCHIPとし、確定idleはexactな1px水平線、明示欠損を
挟む区間だけは1px破線予測とする。accepted raw tokenのexact-equalは確定idleの二値判定だけに使い、token deltaの量・比率や
task lifecycleでRemaining低下を配分または成形しない。
`correction_v2`は直接観測されたraw Remainingを元時刻のanchorとして保持し、terminal holdを含む
`remaining_values`を`[73.0,73.0,73.0,70.0,70.0,62.0,62.0]`とする。model累積値の後退・回復を
Remainingの配分または線種へ波及させず、直接観測anchor間は3px実線PCHIP、最後のraw anchorからperiod endだけを
1px破線holdとする。`Remaining`からmodel系列のtailを外挿しない。

### authorityを保持する境界入力の確定結果

次表は上記規則の独立した数値oracleであり、実装のhelper出力や丸め値から期待値を作らない。記載しないgapはなく、modelは同じexact period/key、時刻は連続minuteとする。

| 入力 | 必須結果 |
| --- | --- |
| confirmedのTが20→21、両rowのmodels_complete=false | 20と21を元時刻のaccepted観測として実線表示。集合が不完全なので全体idleはなし |
| 有効な公式Remainingが90→95→89 | 3点全てを保持し、隣接区間の上昇/下降を実線表示。95を棄却せず、resetも推定しない。両変化区間はidleなし |
| Tが20→20、保存USDが1→2 | USDの両観測を保持し、2を1へ置換しない。USD変化の理由は価格版・原本prefixで調べ、token増加を創作しない |
| accepted USDが1→未知→3 | 未知点のhoverは「—」、実測anchorは1と3。間の破線は表示専用。同日・同所有区間・未回復gapなしなら月の差分は2の記録分 |
| gap=[60,180]、model Tが時刻60/120/180で20/30/40としてconfirmed復元済み、Remainingの120はnull | modelの60→120→180は回復済みcoverageの実線、Remainingの欠測は破線。model復元だけでquota ledgerをrecoveredにせず、全体idleはなし |
| Tが9,007,199,254,740,992→9,007,199,254,740,993 | 差は正確な1。同値や未使用へ丸めない。同日配賦できる差も1 |
| 二つのmodelのTがそれぞれ18,446,744,073,709,551,615 | 表示合計は36,893,488,147,419,103,230。UInt64 overflowで負数/0/上限値へ変えない |

## グラフ表示への参照

Windowsのみ、既定のreset週期間、固定24時間／7日間、表示timezoneの暦月を選べる。固定時間窓に重なる各resetは同一account/published pairで全履歴を完全受理した別系列として投影し、表示窓が週の途中から始まってもそのresetの利用相当額を表示する。線は表示窓の左端で切り出し、観測終端から現在までを実測で埋めない。固定時間窓とreset期間の縦軸はmodel累積の選択metricとRemainingの百分率を独立に扱い、数値domainと座標はUX §4.2「数値軸・時刻軸の値と座標」に従う。Windowsの常時右端model値ラベルと専用余白は、近い実観測時刻の日時・全表示系列の値を示すホバー表示へ置き換える。操作とgeometryの詳細は`docs/WINDOWS_UX_SPEC.md` §4.2を正本とする（#554、#600）。

`G137-GRAPH-01`のWindows月表示は、同じaccepted seriesを次の順序で日別配賦する。

1. account・exact period・model・metricごとに、上記の既知性、isolated pulse、baseline/recoveryの判定を先に完了する。月だけ別の後退判定を実行しない。棄却したraw値を新baselineへ昇格せず、表示用の補間・hold・synthetic値は差分に使わない。`legacy-unknown`のlossless保存値は「記録分」への表示専用配賦だけを許し、sourceをconfirmedへ昇格しない。
2. 日Dは各UTC instantを表示timezoneへ変換したlocal dateがDになる範囲とする。夏時間の欠落・重複を実timezone規則で扱い、固定86,400秒で区切らない。当日は公開済み観測の範囲まで、未来日は値なしとする。同じperiod/model/metricの連続するaccepted観測について、差分区間全体が同じ日・同じ所有区間に属し、confirmed gapを跨がない場合だけ、非負の累積差をその日へ一度加算する。途中の明示欠測・棄却値はbaselineを0へ戻す理由にならず、既知両端から得た日内差分を「記録分」として保持する。日跨ぎ・非所有区間・confirmed gapを跨ぐ差分はどの日にも按分しない。
3. source JSONLの累積counterと、既にDBへ帰属済みのperiod/model累積を区別する。前者の初回値はDATAのbaselineであり、新規利用として加算しない。後者は採用済みevent deltaのperiod内合計である。公式`origin_at`と初回の帰属済み累積が同じ日・所有区間にあり、間にconfirmed gapがなければ、その保存済み初回累積をその日の記録分へ一度配賦できる。これはsourceの初回counterを丸ごと課金することでも、graphに観測のない0 anchorを描くことでもない。単にstart_atが同じ日、またはsource counterが正値という条件では配賦しない。
4. 数値を配賦できた日は「記録分」、配賦できない日は「未計測」とする。確定した日全体の利用量を主張するには、当該日の両UTC境界（当日は対象終端）における累積、全model/sourceの完全coverage、所有区間、期間境界、欠損と異常の不存在を証明する必要がある。通常v3履歴はminute代表の累積観測であり、models_completeだけではこの日境界coverageを証明しない。したがって月UIはv3履歴から得た値を常に記録分として扱う。23:59の10から翌00:01の13への差3は、graphでは正常な実測anchor間でも月では未配賦であり、どちらの日の0や3にも変換しない。記録分0と未計測を区別し、一日未使用の確定へ読み替えない。
5. 累積100→90→110で90が棄却される場合、月も100を保持し、同日・同所有区間・gapなしなら10だけを追加配賦する。90を新baselineにして20を計上したり、既存の有効配賦100を消したりしない。検証済み原本復元で既存のaccepted履歴自体が訂正された場合は、同じ新pairの全必要pageを受理してから月全体を元観測から再計算する。旧配賦への補正差を推測加算しない。
6. tokenと保存USDは独立に配賦する。未知価格・未知componentを0としない。tokenの既知totalは未知価格だけで捨てない。当日へ配賦した正のtoken差分について、同じperiod/modelの当日配賦可能なaccepted USD区間（または起点資格を満たす初回保存USD）がその差分区間を覆わない場合、その日の合算USDは「—」とする。他modelの既知USD部分和を日セル・tooltip・アクセシビリティ名へ数値表示しない。既知部分は計算上保持し、原本回復後に同じ全pageから再計算する。例えばAのtoken差分とUSD差1が既知、Bの正のtoken差分は既知でUSD差が不明なら、その日のTokens記録分は保持するがDollarsは「—」となる。一方、USDが1→未知→3でも同日・同所有区間・未回復gapなしのaccepted両端がtoken差分区間を覆えば、差2を配賦でき、途中nullだけで合算を不明へ落とさない。掲載されない未知modelを創作して部分集合の既知額を消す規則ではなく、集合不完全の記録分は不完全のまま保持する。全model合算、丸め、未使用日の控えめな表示はUX ownerに従う。日跨ぎ差を含まない完全な同一metric区間では、配賦和はその区間のaccepted累積差に一致する。未配賦差がある場合は一致を偽装せず、元の境界と不明理由を保持する。

一般のグラフgeometry、操作、色と線幅はUX ownerの`CUM-138-06`が
`docs/WINDOWS_UX_SPEC.md`で所有する。Issue #137で確定したcross-platformの値・線種・連続区間・
未使用・cursor回復・failure isolationは`G137-GRAPH-01`を正本とし、下流仕様とtestは
`G137-1`..`G137-10`を参照して意味を重複定義しない。

### 履歴・snapshot validationを残す理由

| validation | 防ぐcase | 外した場合に起きること |
| --- | --- | --- |
| profile-owned DB scope・exact `reset_at`とcanonical minuteによる各観測の一意な所属。同minuteの異なる公式resetは別keyで保持 | 別profile/cycle/minuteの混在、または正常な旧cycle末尾と新cycle先頭の同一分 | 異なる観測を一つに混ぜる、または正常なreset直後のdetailsを停止する |
| 同じexact period/minuteのdistinct non-null quotaが最大1個。取得順不明の旧不一致minuteだけを公開対象外にする | 同一minuteの相反する残量 | 任意のquotaを選ぶ、または1分の異常で全表示を停止する |
| 既存cumulative vectorのcomponentwise-dominance。不成立minuteだけを公開対象外にする | 非比較vectorやcomponent別maxによる合成 | sourceに存在しない累積値を作る、または1分の異常で全表示を停止する |
| public candidateのraw/canonical duplicate拒否 | 同じexact period/canonical minuteの複数値 | Graphへ垂直変化またはsink依存の値を渡す |
| detailsのstrict schema/domain/header検証 | 欠落・未知・重複・stale generation | Main/Graph/Threadsが異なるrootを表示する |
| health readiness | listenerだけが存在しsnapshotを返せない状態 | Setupが利用不能serviceを接続済みと扱う |

`HistoryCanonicalizer`の旧行の所属、同一minuteの採用元、比較する全model vector、局所除外とpartition rollbackはDATA owner `HISTORY-CANONICAL-134`だけが定義する。時刻の近さやresetの追随を根拠に、公式の別期間または帰属不明の旧行を同一cycleへ変換しない。account readerは検証済み`usage_history`とexact keyのsidecarを読み、公式quotaは`QUOTA-DURABLE-604`の受理済み記録から導出する。公開windowのclippingとmodelの期間再所属は別の処理であり、window内にあるという理由だけでmodelを移さない。

## 「正しい表示」の判定（必須不変条件）

次の条件をすべて満たした状態だけを正しい表示と定義する。

1. 同一の認証主体でquotaを定期更新している間は、受理済みモデル使用量・履歴・threadを保持する。更新途中の欠測を空、0、未取得へ置き換えない。収集済みの新しい部分観測を受理した場合は、`API-V3-MODELS-01`に従い確認できた最新model値と不完全状態を一組で表示し、古い完全集合へ戻さない。
2. 同じ公式`reset_at`は同一期間として扱う。より新しい公式quota観測で日時が変われば、quota回復やDBの観測時計に関係なく新期間として記録・公開する。
3. モデル使用量（ドル・token）と残量（%）は別観測として扱う。残量観測がない時間帯をモデル使用量から逆算せず、遅れて届いた残量観測はその時刻へ反映する。
4. DATA owner `HISTORY-CANONICAL-134`の原本で証明するperiod所属、lossless vector比較、採用元exact key、局所除外とrollbackに従う。公式quotaと別resetの受理は`QUOTA-DURABLE-604`を優先し、時刻の近さ・windowの重なり・固定期間長でmodelを再所属させない。同じexact period/minuteへの有効な新frameはDATAの通常更新規則でvectorとsidecarを同一transactionで保存する。公式取得順で新periodになった場合は新しいexact keyへ保存し、同minuteにある旧periodの有効なvectorとsidecar、Session原本・event・checkpointを保持する。UI/RESTの補償で値や所属を作り替えない。
5. 明示的なログアウトまたは認証主体変更だけが可視状態を消去する。通信失敗・quota更新中・local収集中は最後の受理済み表示を保持し、失敗状態は別途表示する。取得できた部分観測のmodel値は集合全体の不完全性だけで消去せず、MainとGraphで不完全状態を可視化し、正常・最新と誤称しない。
6. 製品バージョンはメイン画面に一度だけ表示し、子ウインドウのタイトルやボタンへ重複表示しない。値はX版・Windows版とも同じリリースversion authorityから導出する。
7. Windows版の初回起動では、health readiness後に最初のstrict validation済み`/v4/current` snapshot（exact404だけv3 current、さらにexact404の旧serviceだけ上記legacy chain）が揃うまで内容領域を表示せず、固定レイアウト上にスピナーを表示する。control応答とのmerge、途中fieldの順番描画をせず、初回取得失敗時はスピナーを解除して失敗状態と再試行手段を表示する。Linux / WindowsのMainに公開対象となった現在openのSession thread件数を表示するため、`open_session_thread_count>0`のcurrentに限り同じMain cycleでthreadsを1回取得し、同一published pair、open件数と行数、active件数と動作中行数が一致する組だけを一括表示する。`open_session_thread_count=0`はthreadsを要求せず同じcurrent pairのthread行を空として一括表示する。Mainの合計と`SOL/TERRA/LUNA/ASTRA/その他`はこの一つの受理済み行集合だけから導出し、常に合計＝各bucketの和とする。開いたThreads詳細はMainが新しいbundleを受理した同じUI更新でそのthread行（0件を含む）へ追従し、別のthreads requestを追加しない。Main更新前に始めた独立Threads取得の古い応答で、その更新を上書きしない。threads失敗・世代不一致・件数不一致ではcurrentだけを反映せず直前の完全表示を保持する。positive bundle失敗後は10秒後にcurrentを条件headerなしで1回だけ再取得し、そのbundleが成功するまでforce、Graph、独立Threads取得で待機を迂回せず、保持中のerrorを解除しない。Graphを開いていない間はGraph resourceを取得せず、Threads詳細windowを閉じている間はこのMain bundle以外の5秒周期threads取得を行わない。
8. X版の初回起動でも、health readiness後に最初のstrict validation済み`/v4/current` snapshot（exact404だけv3 current、さらにexact404の旧serviceだけ上記legacy chain）が揃うまで主画面の内容領域を公開せず、ヘッダー（製品バージョンを含む）を固定したままスピナーを表示する。current取得が失敗した場合はスピナーを解除し、最後の完全表示または失敗状態を表示する。GraphまたはThreadsを開いていない間は、前項のMain bundle例外を除きそのresourceを取得しない。Threads詳細windowの独立5秒pollが失敗しても、その失敗を詳細側に隔離し、受理済みMainのpair、quota、model、thread合計、状態表示を変更しない。Main自身のbundle失敗では前項どおり最後の完全表示とerror表示を保持する。X11の実service gateは、同じ候補binary・display・900x498・fontで作ったpreview部品templateと、同一pairのcurrent count 1＋threads 1件（SOL）を受理した非preview Mainの合計と5 model部品を個別比較し、`[1,1,0,0,0,0]`かつ合計＝bucket和でなければFAIL/HOLDとする。
9. X版の起動ウィンドウは主モニターの可視デスクトップ内へ配置し、別モニターや負座標へ出して利用者から見えない状態にしてはならない。起動成功は、可視範囲内の実ウィンドウと内容の実画面で確認する。
10. `--ui` のdaemon/REST起動に失敗しても、X版のGUIを消失・即時終了させず、接続失敗と再試行手段を表示する。
11. 同一periodのモデル別累積はmodelごとに独立したログ観測として扱う。1 modelが直前の確定値より後退したrowは、そのmodelだけを回復点まで欠測とし、同じrowで確定している他modelの実測値を欠測や前回値へ置換しない。model集合が不完全でも、掲載modelの実測値は無効にしない。
12. remote quotaとlocal Session/logは独立した取得元とする。remote transport障害中もresident local collectorとDB recorderを60秒以内の既存周期で継続し、local-only rowはquota `NULL`かつmodel source `confirmed`でcommitする。fresh remote quotaは`QUOTA-DURABLE-604`に従い、local取得・集計の成否と独立して取得原本と受理済み記録へ保存する。元の観測時刻と取得順序を保持し、同じminute/periodのmodelがないquota履歴はmodel source `unavailable`とする。直前のdurable model vectorを新しい確定rowとして複製しない。古いquotaを新しいtimestampへ複製しない。local取得失敗はsingle-flightを解放して次周期で再取得し、DB書込み失敗はprovenanceを含むexact pending batchを保持して同じdaemon内で再試行する。provenanceなしの旧rowは`legacy-unknown`であり、確定観測へ昇格しない。

## Linux更新の診断（Issue #479）

`LINUX-UPDATE-DIAGNOSTICS-479`: 既存updaterは更新の開始、選択した対象版、処理段階、結果、失敗理由を `~/.local/share/codex-info/update.log` のJSON Linesへ記録する。既存の適用失敗、rollback、commit後のcleanup失敗を別の段階・結果で識別する。記録できない場合はstderrへ診断を出す。更新判断、download digest、適用・稼働確認の契約は既存の更新ownerを維持する。
最後の更新失敗理由は `last-update-failure.txt` に保存し、既存launcherの `codex-info --ui` 起動時にstderrへ表示する。次の更新試行は前回の失敗表示を消して開始し、その試行で失敗した理由を保存する。
