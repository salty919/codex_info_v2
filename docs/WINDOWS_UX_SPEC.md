# Windowsクライアント UX設計仕様

## 0. 状態と適用範囲

状態: `REQUIREMENTS_SELECTED / PRODUCT_PENDING`

SSH-001/RC-061〜063の接続・保存・headless契約は本仕様へ伝播する抽出正本であり、状態は
`REQUIREMENTS_SELECTED / PRODUCT_PENDING`とする。installed API serviceのexact command、実装、host、
artifact、fresh image、独立製品判定は未取得で、文書から製品PASSを主張しない。

この文書は、Windows版を「表示できるもの」にするためではなく、顧客が迷わず
監視・接続・復旧・設定・更新・削除できる製品として設計するためのUX正本である。
X版はデータ意味論、状態、所有権の参照元であり、Windows版のレイアウトや操作を
無条件に複製する根拠にはならない。Windows固有の判断は、この文書に目的・代替案・
採用理由・影響する要求ID・受入証拠を登録しない限り採用してはならない。

要求抽出が `EXTRACTION_COMPLETE` になるまで、この文書の変更は文書化に限定する。
実装、テスト、ビルド、インストール、画面評価、成果物差し替えは行わない。

## 1. UXの目的、利用者、主要タスク

### 1.1 目的

1. 起動直後に「接続できているか」「残量はいくつか」「何が変化したか」を一読で判断できる。
2. 初回導入で、Linux/WSL API・SSH local forwarding・Windows UIの関係を知識なしで理解できる。
3. 認証、接続失敗、設定破損、サーバー停止、再起動、更新、アンインストールの各状態で、
   次に実行する安全な操作が明示される。
4. X版の値・期間・グラフ意味論を失わず、Windows作法のメニュー、フォーカス、キーボード、
   高DPI、マルチモニタを備える。
5. 監視のために画面を探し回ったり、ページを上下にスクロールしたり、ユーザーのマウスを
   奪ったりしない。

### 1.2 想定利用者

| 利用者 | 必要な結果 | 設計上の制約 |
| --- | --- | --- |
| 初回導入者 | 接続から監視画面まで到達する | SSH専門知識、設定ファイル編集を必須にしない |
| 日常監視者 | 残量、リセット、実行中スレッド、利用推移を一画面で把握する | 主情報にページスクロールを要求しない |
| 障害対応者 | API/SSH/認証/DBのどの境界で失敗したかを切り分ける | raw秘密情報・raw backend errorを表示しない |
| 管理者 | インストール、更新、rollback、アンインストールを安全に行う | 設定・履歴の意図しない削除を禁止する |
| 支援技術利用者 | キーボード、フォーカス、読み上げで操作する | 色やマウスだけを必須にしない |

### 1.3 UXの非目的

- X版の見た目をそのまま複製すること。
- 1画面に情報を詰め込んで、文字を小さくしたりページスクロールで隠したりすること。
- 成立しているだけの仮アイコン、飾りのカード、意味の重複する説明文を増やすこと。
- password/token/key/path、OpenSSH展開値、raw manual host/user、API URL、argv、stderrを保存すること。
  再接続に必要な非秘密selector（`connectionProfile`と`connectionSelector`）だけは、6-key設定へ
  atomic保存する。

## 2. 絶対UX原則

### 2.1 非スクロール原則

「画面をスクロールしないと主要操作や主要情報へ到達できない」設計はUX合格としない。
対象はMain、Setup、Settings、Graph、Threads、Legalの全Windowであり、Main内Helpにも同じviewport条件を適用する。

- Main: 残量、リセット、状態、更新、メニュー、Graph/Threads/Legal入口を同一viewportに置く。
- Setup: 現在の手順、入力、検証結果、次へ/戻る/キャンセルを同一viewportに置く。
- Settings: 編集対象、現在値、保存、取消、復旧、戻るを同一viewportに置く。
- Graph: 期間、metric、系列操作、plot、現在値を同一viewportに置く。
- Threads: Windowを900×480から拡大せず、空状態または先頭6件の2行カードと閉じる操作を
  同一viewportに置く。7件目以降だけ一覧内の縦スクロールを許可し、画面全体はスクロールしない。
- Legal: 本文を分割表示できる章/ページとし、戻る・閉じるを常時表示する。長文を理由にアプリ全体の
  ナビゲーションをスクロールの下へ追いやらない。

スクロールバー、マウスホイール、トラックパッドによる画面移動を、主要画面の到達手段として
採用しない。長い一覧・本文はページング、章切替、選択詳細、折りたたみで分割し、現在位置と
次の操作を固定表示する。例外はThreadsの7件目以降だけであり、先頭6件を完全表示した同じ一覧内
`ScrollViewer`で追加行へ到達してよい。画面全体のスクロールや6件以下でのscrollbarはFAILとする。
  ページングや折りたたみでも主要情報を同時に比較できない場合は、レイアウトを再設計する。

### 2.1.1 Window geometry、DPI、topologyの正本

この仕様のgeometryは要求抽出の正本であり、現行実装、既存のfresh画像、fixtureの都合から
昇格・変更してはならない。寸法はOS frameを含まないlogical client sizeで表し、HelpはMain内
surfaceとして扱う。

| surface | registered top-level surface | runtime open HWND | logical client initial | logical client min | logical client max | resize | native controls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Main | yes | 1 | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Setup | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Settings | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Graph | yes | 0..1 (singleton) | 940×640 | 700×480 | unbounded | resizable | minimize, maximize/restore, close |
| Threads | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Legal | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Main内 Help | no (owner=Main) | 0 additional | Main client 900×480内 | Main client 900×480内 | Main client 900×480内 | Mainに従う | 独自Window controlsなし |

registered top-level surface inventoryはMain、Setup、Settings、Graph、Threads、Legalの正確に6個で
固定し、Helpを第7 Windowへ分離しない。runtime open HWNDはMain=1＋open child subset=0..5、合計1..6で、
5 childを全て開いた時だけ6となる。各childはsingletonで、runtime cardinalityを6へ固定しない。`700×480`はGraphのminimumだけに属する。Main、Setup、Settings、
Threads、Legalのsupported work areaは少なくとも`900×480 logical`、Graphのsupported
work-area minimumは少なくとも`700×480 logical`である。各境界未満はsupported matrix外として
`unsupported_scope` manifestへ記録し、font縮小、clip、scroll、PASS値の捏造で回避しない。
この境界は新しいproduct failure classや第7 Windowを追加する根拠ではない。

surface/monitor/DPI/sizeのsupported predicateは次のANDで固定する。

```text
supported = client_threshold AND frame_fit
client_threshold(fixed Main/Setup/Settings/Threads/Legal) = logical >= 900×480
client_threshold(Graph) = logical >= 700×480
frame_fit = DPI変換後のDWM.visible_frame全体が対象MONITORINFO.rcWork内へ完全包含
```

`frame_fit`はphysical rectのleft/top/right/bottomを全て判定する。logical client thresholdは必要条件で、
thresholdだけでsupportedとはしない。thresholdとframe-fitのANDがsupportedの十分条件である。

DPI authorityは`GetDpiForWindow`相当のOS-reported integer `dpi`、`scale=dpi/96`である。
100%/150%/200%（`96/144/192`）は必須fixtureだが、対応domainをこの3値に限定しない。正の
logical width/heightからphysical sizeへの変換は各値を
`floor(logical*dpi/96 + 0.5)`で丸める。geometry evidenceでは、
`logical_client`、`physical_client`、DWM visible frame boundsの`visible_frame`、
`MONITORINFO.rcWork`の`work_area`を別fieldとして記録し、origin/size/unitを混同しない。

Main freshの対象monitorは起動直前foreground Windowのmonitor（無効時はprimary）、child初回
openの対象monitorはowner monitorとする。fresh Mainと初回user-openだけをcenterする。reopenは
現在のいずれかの`rcWork`へvisible frame全体が包含される時だけlast stable OS rectを復元する。
monitor除去/解像度縮小で無効ならMainはforeground monitor→primary、childは有効owner monitor→primary
の順で一度だけ`topology_recovery center`し、reasonをraw記録する。どのmonitorもsupported predicateを
満たさない場合は`unsupported_scope`とし、PASSを捏造しない。`MONITORINFO.rcWork`とDWM visible frame boundsからphysical座標で
`origin=work_origin+floor((work_size-frame_size)/2)`を一度だけ求め、各軸のdouble-coordinate
residualを`abs((2*frame_origin+frame_size)-(2*work_origin+work_size))<=1`とする。通常のtimer、poll、
reopen、drag後のrecenter、`Window.Position` loop、cursor合成は0件であり、無効reopen時の一度だけの
`topology_recovery center`だけを例外とする。

登録された6 surfaceのtitle領域はnative moveを1 gestureにつき1回だけ開始し、control hitを0件にする。
全Windowはminimize/closeを持ち、native resize/maximize/restoreはGraphだけに許す。Graphの
maximizeはcurrent monitorのwork areaへ適用し、fullscreenはproduct actionにしない。

same-DPI crossing、different-DPI crossing、negative/nonzero origin、taskbar-shrunk work areaを
必須topology cellとする。対象monitorはsurfaceのsupported boundary以上であり、未満は
`unsupported_scope` manifestへ記録する。これらの追加はOS配置・入力の表現契約であり、X版の
値、期間、状態、情報所有権、失敗時保持、Graph/Threadsの順序といったデータ意味論は変更しない。

この変更の目的は、logical clientとphysical client/frame/work-areaを分離し、DPI変更・複数monitor・
native moveの境界を要求抽出時に一意化することである。理由は、固定WindowへGraph minimumを
誤適用したり、実装のcenter helperを正本へ昇格したりすると、未確認のgeometryを仕様として
固定するためである。検証は、一つの評価対象release artifactで各軸を最低1回、既知の相互作用を
有限のrisk-based caseとして起動し、raw OS DPI、logical/physical/client/frame/work-area rect、round式、center
residual、HWND count、native move/resize/control-hit、foreground/cursor traceを採取し、
実装者とは別の担当が再計算する。未実行のWindows caseは製品PASSとみなさない。

### 2.1.2 旧スクロール要求のsupersede境界

rootまたは内部`ScrollViewer`だけを到達手段にする旧要求は、このDecision
`UX-20260822-UX-002`により明示的にsupersedeする。Main、Setup、Settings、Legalは、
page/step/detail/chapter/collapseで全主要情報、primary action、Back、Closeを同一viewportへ
置く。Graphもperiod/metric/series/plot/現在値とBack/Closeを同一viewportへ置く。Threadsは
先頭6件とWindow操作を同一viewportへ置き、7件目以降に限って一覧内scrollを使う。既存のX版
データを削除・要約・再順序化しない。

### 2.2 一目で分かる情報階層

1. 残り利用枠（最重要）
2. 接続/認証/エラーの状態
3. リセット時刻と期間ゲージ
4. 実行中スレッドの概要
5. 現在期間のモデル別利用量
6. 推移・詳細・法的情報への入口

各事実の表示所有者は `DESIGN.md` と要求IDで一つに固定する。同じ事実を別名、別カード、
別画面の補助文言として再掲しない。追加表示には、何の判断を助けるかを記録する。

### 2.3 Windows作法と製品らしさ

- 最上位の移動先はメニューまたは一貫したナビゲーション領域から開く。
- メニュー項目はアイコンだけでなく文字名、ショートカット、アクセシブル名を持つ。
- `Monitor / Trends / Threads / Settings / Legal / Help` の名称・順序・位置を全surfaceで一貫させる。
- 現在位置、戻る、閉じる、処理中、無効、エラーを同じ視覚規則で表す。
- ネイティブタイトルバーを置き換える場合は、全Windowの移動・最小化・閉じると、Graphだけの
  最大化/復元・リサイズを明示し、OSの作法を欠落させない。画面中央の見出しをタイトルバーの
  代用品として重複表示しない。

### 2.4 データ意味論と見た目の分離

X版から必ず継承するのは、値の正本、期間境界、欠測/重複/初回観測の扱い、系列順、色の意味、
状態遷移、失敗時の保持である。Windows固有に変更できるのは、ナビゲーション、入力、余白、
フォント、アイコン、ウィンドウ管理などの表現面だけであり、変更理由と同値性証拠を要求台帳へ置く。

比較画像の差分だけで「見た目が違う」と判断せず、同じfixture、同じartifact世代、同じ期間、
同じtimezoneで、値・時刻・折れ点・軸・系列visibility・ラベルを別々に比較する。

## 3. 情報構造と導線

```text
起動
 ├─ 初回/未設定 ─ Setup（profile/selector → server/API prepare → listener → readiness health → details → auth-start → auth-check → details）
 ├─ 設定済み/未接続 ─ Main（disconnected） → Settings recovery / Setup
 ├─ 設定済み/接続済み ─ Main（saved selectorで次回自動再接続）
 │    ├─ Trends（Graph）
 │    ├─ Threads
 │    ├─ Settings
 │    ├─ Legal
 │    └─ Help / Connection guide
 └─ 起動後の失敗 ─ Monitor（last-good保持または未取得） → 明示された復旧操作
```

### 3.1 初回導入

Setupはウィザード型の段階表示とする。各段階は「現在地」「入力/結果」「次の操作」を持ち、
無効な入力では次へ進めない。保存schemaは`language/setupCompleted/connectionConfigured/timeZoneId/connectionProfile/connectionSelector`
の6-keyに固定し、`connectionProfile=none|wsl|sshConfigAlias`、WSL selectorはinstalled distributionの
exact token、SSH selectorはliteral Host alias（`^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$`）とする。
raw manual host/userはone-session raw recoveryだけで、durable settings・完了状態・再接続selectorにしない。
SSH/WSL childはshell、cmd、PowerShellを介さず、実行ファイルと個別tokenのArgumentListで起動する。

### 3.2 日常監視

Mainを既定の到達先とし、保存済みselectorで次回自動再接続する。接続確認・poll・同一generationの
自動再構築ごとにSetup/app確認を再表示しない。更新は明示ボタンとbounded自動更新を同じ状態機械で扱い、
更新中の再クリック、重複要求、値の一時消去を禁止する。

Main、Graph、Threadsは同じstrict validation済み`/v1/details`一応答を一つのatomic rootとして置換する。
SQLite、別poll、認証control応答でfieldを補完せず、quota/history/threadの再収集、
値の再計算、同一minuteのmerge/max/last/null化をUIで行わない。候補拒否時は全surfaceが同じlast-good rootを保持する。

### 3.3 詳細・設定・法的情報

子Windowは単一インスタンスとし、既に開いていれば前面化する。子Windowを開いたことでMonitorの
状態やlast-good値を消さない。戻る/閉じるは常時利用可能で、終了時にタイマー・RPC・購読を解除する。

## 4. 画面別UX仕様

この節の抽象的な列挙順より、`DESIGN.md`の情報所有権とlogical layout式、および後発Decision
`UX-20260822-UX-002`、`UX-20260822-GRAPH-001`、`UX-20260822-SSH-001`、
`UX-20260823-ERROR-001`、`UX-20260823-KEYBOARD-001`の具体的な順序・分割・状態遷移を優先する。列挙順を理由に、正本の
component順や表示所有者を変更しない。

### 4.1 Monitor

- 画面上部にアプリ内タイトルとメニューを置く。
- 認証済みMainのcomponent順は
  `Header→RemainingQuota→WeekGauge→AccountActivity→ModelUsage→StatusBanner`で固定する。
  残量を最初の主値とし、状態は常時viewport内のStatusBannerだけが所有する。状態を上段の
  duplicate cardへ増やさず、StatusBannerが末尾でもBack/Close/復旧CTAを隠さない。
- 0%、中間、100%、未取得、警告、危険、APIエラー、認証要求で同じ構造を保つ。
- エラーは既存値を保持するか未取得として明示し、0や100を仮の有効値として表示しない。
- 数値、単位、説明、状態、操作の文字サイズと太さに役割差を付ける。細すぎるフォント、薄すぎる文字、
  余白だけで分断されたカードは採用しない。

### 4.2 Trends / Graph

- 期間、ドル/トークン、Remaining/LUNA/TERRA/SOLの操作を上部固定帯に置く。
- 期間・metricのリストはpointer pressの1回で展開する。REST/DB/poll完了を待たず、物理入力から
  user-visible paintまで、系列ON/OFFはP90 75 ms以下・P95 100 ms以下、期間/metricリストは
  P90 100 ms以下・P95 150 ms以下、いずれもcold max 250 ms以下とする。10,080点と契約最大1暦月
  44,640点の双方で30回以上測定し、一つでも未測定・超過ならUX FAILとする。
- このP90/P95は固定Windows実機または専用self-hosted runnerで測る。負荷と画面capture速度が共有される
  GitHub-hosted runnerは機能E2Eとハング検知にだけ使い、絶対性能の合否判定には使わない。
- 系列ON/OFFはpointer pressで状態とボタン面を先に更新する。P90/P95はこのuser-visible acknowledgementを
  測り、同じ入力でplot画像も必ず変化したことを別のbounded postconditionとして確認する。
- 期間/metricの各測定sampleは閉状態から1回の物理クリックで展開する同一操作とし、次sample用の
  Escape閉鎖は測定区間へ混ぜない。物理入力、UI状態、実描画を別経路で確認し、同一点の機械的な
  高速連打や開閉の異なる操作を一つのP90/P95へ合成しない。pollやlocale通知による同値候補の
  再公開をユーザー選択へ読み替えず、開いているリストを自動で閉じない。
- 期間変更は`idle → loading → ready|failed`の有限状態遷移とする。選択表示は入力直後に更新し、
  accepted details rootのparseとpresentation projectionはUI thread外で行う。SQLite再読込やsampleの
  canonicalization/merge/recalculationは行わない。既存の遅延残量補間と終端保持はpresentation-onlyで行い、
  導出点をdetailsやDBへ書き戻さない。処理が次paintまでに終わらない場合は操作を塞がない
  indeterminate progressと「期間データを読み込み中…」を表示する。loading中は直前に完成したgraph・
  metric・軸を保持し、候補完成時だけ1回のUI publishで全てを同時に差し替える。
- 期間を連続選択した場合は旧候補をcancelし、最新revisionだけをpublishする。失敗・timeout・cancelを
  空graphや部分graphへ変換せず、直前graphを保持してbounded errorを表示する。キャッシュ済みで次paint
  までに切替できる場合はprogressを点滅させない。クリック反応SLOと期間データ完成時間P90/P95を混同しない。
- plotの横軸はX版の期間意味論を維持し、現在期間は観測時刻までを右端とする。
- 右端現在値の表示域は、初期940×640 logical表示時に各metricで確保される幅をドル／トークン別に固定する。Graphを横へリサイズした差分はplotへ割り当て、現在値、系列色、leader、縦位置を変えない。
- Remainingは独立0–100%意味、モデル系列は累積値として扱う。残量をドル軸へ誤って合わせない。
- Remainingとモデル使用量は別の観測値であり、モデル使用後に遅れて届いた最初の低い残量観測はその観測時刻へ反映する。残量観測が存在しない区間を料金・tokenから逆算してはならず、未観測区間を正常な残量低下として表示しない。
- X版とWindows版は同一の履歴fixtureと固定期待値（期間境界、累積SOL、遅延残量、未観測区間）を通過しなければならない。片方の描画ヘルパーが生成した値をもう片方の期待値には使用せず、fixtureの独立oracleを使う。
- shared rollover fixtureのperiod A→Bで`100% / $1 → 41% / $323.674247`を保持し、
  `graph_delayed_quota`と既存history/rolling/delayed/gap/no-history/REST/Windows回帰を同一revisionで確認する。
  値形状による100%・7日・quota-only除外や、新workflow gate・全直積は追加しない。
- 操作帯を開閉してもplotの位置・高さを変えず、ラベルや右端値を隠さない。
- 記録なし、欠測、アイドル、活動、0/中間/100を明示的な設計状態として扱う。

### 4.3 Threads

- 900×480 logical clientを拡大せず、カードの余白と行高を抑えて先頭6件を完全表示する。
- 各カードは高56px・間隔4pxの2行構造とし、title/model/context/経過とparent/role/depth/token/指示年齢を配置する。1件だけでもカードを拡大しない。
- 6件以下では縦scrollbarを表示せず、7件目以降だけ同じ一覧内で縦scrollを許可する。
- 親子関係、role、model、context、token、経過、指示年齢を同じ行または選択詳細で追跡できる。
- 64pxのtree gutterと本文開始位置を分離し、rail・junction・title・parent textの重なりを0pxにする。
- stale、停止済みchild、orphan、cycle、部分snapshotは誤って実行中として表示しない。
- 一覧件数が増えても本文fontを縮小せず、Window拡大や空疎なcardで情報密度を下げない。

### 4.4 Setup / Settings

- profile/selector、API到達性、readiness health、details state、auth-start、auth-checkを別概念として表示する。
- exact settings keysは`language/setupCompleted/connectionConfigured/timeZoneId/connectionProfile/connectionSelector`。
  profile enumは`none|wsl|sshConfigAlias`、selectorは`none`、installed distribution exact token、または
  literal Host aliasだけとする。
- password/token/key/path、OpenSSH展開値、raw manual host/user、API URL、argv、stderrは保存0。SSH自動経路は
  `BatchMode=yes`、hidden prompt=0、unregistered/changed host keyはconnectedにしない。自動RemoteのArgListは
  `[ssh.exe,-o,BatchMode=yes,-N,-L,8787:127.0.0.1:8787,<validated alias>]`に固定する。明示CTA時だけ一回の
  OpenSSH-owned interactiveを許可する。
- 設定破損・空JSON・途中書込み・old 4-key・invalid selectorはWelcomeを無限表示せず、Main disconnectedと
  Settings recoveryへ遷移し、recovery command count=0とする。
- 保存成功、保存失敗、取消、再起動後の保持を同じ画面で確認可能にする。
- WSL/remote/one-session raw recovery、ArgumentList、API到達、認証開始、認証確認、app-wide single
  supervisor/tunnel/reapの境界は`UX-20260822-SSH-001`を正本とする。

Setupの順序はserver/API prepare→listener→readiness `GET /v1/health`→strict `GET /v1/details`→
必要時だけauth-start→別auth-check→新しいstrict detailsで固定する。auth-start/auth-checkはcontrol-onlyであり、
応答を表示rootへmergeしない。healthだけ、またはcontrol成功だけでdata readyとしない。

RC-121のprofile別action意味論も固定する。WSLのserver prepare/service start、Remoteのinstall/tunnel/raw
tunnelはそれぞれ独立したvisible+enabled Tab step/UIA actionであり、`action.StartForward`へ丸めない。
`action.StartForward`はforwardingだけを表し、SetupOperationGeneration・busy・stale completionは現行世代だけを
受理し、古い完了はcommitしない。

Setupの製品名と導入見出しを一つの文字列へ結合しない。`app_title` は全localeで
`Codex Info Monitor`、導入見出しと入口labelは次のcatalog値を正本とし、`/`で複数言語を併記しない。
未知localeは英語行へ一意fallbackする。

| locale | `setup_heading` | `setup_entry` |
| --- | --- | --- |
| `ja` | `Codex Infoへようこそ` | `初期設定` |
| `en` | `Welcome to Codex Info` | `Setup` |
| `zh-Hans` | `欢迎使用 Codex Info` | `初始设置` |
| `ko` | `Codex Info에 오신 것을 환영합니다` | `초기 설정` |
| `es` | `Te damos la bienvenida a Codex Info` | `Configuración inicial` |
| `fr` | `Bienvenue dans Codex Info` | `Configuration initiale` |
| `de` | `Willkommen bei Codex Info` | `Ersteinrichtung` |
| `pt` | `Boas-vindas ao Codex Info` | `Configuração inicial` |
| `it` | `Benvenuto in Codex Info` | `Configurazione iniziale` |
| `ru` | `Добро пожаловать в Codex Info` | `Первоначальная настройка` |

### 4.5 Legal / Help

- GPL、第三者フォント、schema、dependency、distribution noticeを省略しない。
- Legalは監視画面の主操作と分離し、戻る/閉じるを常時表示する。
- HelpはSSH、WSL、API、認証、更新、アンインストール、障害時の情報採取範囲を利用者向けに説明する。
- UIなしsilent RESTはSlint component/window/event-loop生成0、`DISPLAY`/Wayland/X11依存0、Slint HWND=0
  （visible/hidden HWNDとも0）、headless snapshot builder+read-only publisherだけとする。実装・host・artifact
  証拠未取得のためこのGUI依存ゼロ契約は`PRODUCT_PENDING`である。
- Help/Connection guideはMain client `900×480 logical`内の情報surfaceであり、独立Window/HWNDを
  作らない（additional HWND=0）。registered top-level surface inventoryはMain、Setup、Settings、
  Graph、Threads、Legalの正確な6個で、runtime HWNDはMain=1＋open child subset 0..5（合計1..6）である。

### 4.6 失敗と復旧

- failure classごとのCause、Impact、primary CTA、route、last-good保持は
  `UX-20260823-ERROR-001`を正本とする。
- 状態card内のprimary CTAは1個だけとし、同格の「再試行」「設定」「戻る」を並べて利用者へ
  選択を転嫁しない。Settings/Helpは共通navigationからsecondaryに到達できる。
- background failureはWindowを前面化せず、focus/cursorを奪わない。利用者がCTAを押した場合だけ
  action先へfocusを移す。
- app-wide supervisorはbootstrap/tunnel childを1つだけ所有し、child終了時にreapとlistener消失を確認する。
  同時tunnel=1、orphan tunnel=0、same-generation auto retry infinite=0。recorderはMain/app/tunnel終了後も
  独立ownerとして継続する。

## 5. 視覚・入力・アクセシビリティ

### 5.1 視覚ルール

- 色は状態を補強するだけで、状態文・アイコン・形状を併記する。
- アイコンは機能、状態、操作結果が一意に分かるものだけを使い、ツールチップと読み上げ名を持つ。
- フォントはlocaleごとに決定し、欠字、文字化け、過度な細字、小さすぎる注記を許可しない。
- 機能を保つために余白を削りすぎない。余白を増やした結果、主要情報が隠れる場合はレイアウトを再設計する。

### 5.2 入力ルール

- ユーザーのマウス、カーソル位置、フォーカス、キーボード入力を製品コードが奪わない。
- 物理入力を伴う試験は明示的許可がある環境だけで実施し、通常の受入でユーザー環境を操作しない。
- キーボードTab順、Enter、Escape、Alt/メニュー操作は`UX-20260823-KEYBOARD-001`の6 Window別
  exact route matrixと`windows-keyboard-v1` manifestに従う。
- フォーカス、hover、pressed、disabled、busy、errorを視認できる。focus indicatorの面積、
  2 logical pixel、3:1 contrast、DPI/high-contrast境界は同Decisionを正本とする。

### 5.3 マルチモニタ/DPI

- モニタ境界を跨いでもウィンドウ中心、タイトル領域、操作ボタン、plot、カード端がずれない。
- `GetDpiForWindow`相当のinteger dpiと`scale=dpi/96`を使用し、positive sizeは
  `floor(logical*dpi/96+0.5)`でphysicalへ丸める。96/144/192は必須fixtureだが全domainを
  限定しない。logical client、physical client、DWM visible frame、`MONITORINFO.rcWork`
  work areaは別fieldで記録する。
- Main freshは直前foreground monitor（無効時primary）、child初回openはowner monitorで一度だけ
  centerする。reopenはvisible_frame全体が現存いずれかのrcWorkへ包含される時だけlast stable OS rectを使い、
  無効時はMain=foreground→primary、child=owner→primaryへ一度だけ`topology_recovery center`しreasonをraw記録する。
  center式は
  `origin=work_origin+floor((work_size-frame_size)/2)`、double-coordinate residualは各軸≤1。
  通常のtimer/poll/reopen/drag後recenter、`Window.Position` loop、cursor合成は0件とし、無効reopen時の
  一度だけの`topology_recovery center`だけを例外とする。
- 最小幅、高DPI、最大化/復元、画面端、same/different-DPI crossing、negative/nonzero origin、
  taskbar-shrunk work areaで、supported boundary以上のmonitorに主要情報を表示する。fixed Window
  は少なくとも900×480 logical、Graphは少なくとも700×480 logicalを必要とし、未満は
  `unsupported_scope` manifestに記録する。DPI後DWM visible_frameのrcWork完全包含も必要条件とし、
  client thresholdだけでsupportedにしない。client thresholdとframe-fitのANDがsupportedの十分条件で、
  どのmonitorもpredicate不成立ならunsupported_scopeとする。
  timer/poll/drag後recenterは0件。ページscroll、font縮小、clipで逃げない。

## 6. UX判断記録フォーマット

新しいUI要素またはWindows固有差分は、実装前に次を記録する。

| 項目 | 内容 |
| --- | --- |
| Decision ID | `UX-YYYYMMDD-NNN` |
| 利用者の課題 | 誰が何に困るか |
| 目的 | どの判断/操作を改善するか |
| 代替案 | 少なくとも2案と棄却理由 |
| 採用案 | 表現、導線、状態、失敗時の挙動 |
| X版との関係 | 継承する意味論、変更する表現、変更理由 |
| 影響要求 | `WIN-A..M` のID |
| 非スクロール影響 | 主要操作/値がどのviewportに収まるか |
| 証拠 | fresh画像、操作ログ、rawデータ、SHA、独立評価 |
| 未確定 | 解消条件と担当 |

## 7. UX受入ゲート（実装開始後に使用するが、抽出中は実行禁止）

次をすべて満たさない限りUX PASSにしない。

1. 登録6 surfaceのruntime open HWND（Main=1＋child subset 0..5、合計1..6、child singleton）とMain内Help additional HWND=0を満たし、全surfaceの主要情報・主要操作・戻る/閉じるがページスクロールなしで到達できる。
2. 画面サイズ、DPI、マルチモニタ、locale、状態、エラー、空データ、長文の各状態で同じ優先順位を保つ。
3. メニューから全画面へ到達でき、子画面は単一インスタンスで再利用される。
4. 同一fixtureでX版とWindows版のデータ意味論が一致し、差分は判断記録にある。
5. 文字、アイコン、色、フォーカス、キーボード、読み上げ名、入力非奪取を独立評価する。
6. 主画面の値と状態、Graphの軸と折れ点、Threadsのlive判定、Setupの接続境界、設定/履歴保持が、
   最新artifact SHAとraw証拠に結び付いている。
7. 各surfaceのlogical client threshold AND DPI後DWM visible_frameのrcWork完全包含を満たし、
   reopen invalid時のtopology_recovery reasonとtimer/poll/drag後recenter=0を記録する。
8. 一つでも未確認、`INCONCLUSIVE`、`HOLD`、FAILがあればUXと製品を完了扱いにしない。
9. 全メニュー、selector、toggleの物理入力→paint latencyを4.2節の操作別SLOで測定する。
   backend poll中も同じ上限を満たし、平均値だけで代替しない。

このゲートは、実装者の「見た目は良い」「動いた」という自己判断を受入証拠の代わりにしない。
