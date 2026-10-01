<!-- codex-info-requirement-owner: UX -->
<!-- codex-info-master-ids:
CUM-138-06
ACCOUNT-UX-134
WIN-PARITY-RETRY-01
WIN-PARITY-UX
WIN-PARITY-CTA-01
WIN-PARITY-LEGAL-01
X-START-01
X-START-02
X-START-03
X-START-04
X-START-05
X-START-06
X-THREAD-01
WIN-VERSION-01
WIN-THEME-422
-->

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

要求抽出が `EXTRACTION_COMPLETE` になるまで、未確定の契約は文書化に限定し、実装、テスト、
ビルド、インストール、画面評価、成果物差し替えを行わない。Issue #349で利用者が確定した
`WIN-PARITY-UX`、`ACCOUNT-UX-134`およびI18N ownerの`PROC-I18N-01`に属する下記Main構成と
Linux timezone設定、Issue #422で利用者が選択したWindows版の`WIN-THEME-422`だけは有限scopeの実装・直接評価対象とする。この限定決定は本書全体の
`PRODUCT_PENDING`を解除せず、他の未確定契約を`EXTRACTION_COMPLETE`として扱う根拠にしない。

## WIN-THEME-422 — Windows版の組込みカラーテーマ

Windows Settingsの外観欄は`classic-dark`（従来配色・既定）、`graphite-dark`、`light`、`paper-light`、`sand-light`、`steel-light`、`ocean-dark`、`teal-dark`、`ember-dark`、`ink-dark`、`neon-dark`、`lavender-light`、`mint-light`、`forest-dark`、`tangerine-dark`、`rose-dark`の順に16種類の組込みpresetを選択できる。新規presetの表示名は順にPaper Light、Sand Light、Steel Light、Ocean Dark、Teal Dark、Ember Dark、Ink Darkとし、日本語では順にペーパー ライト、サンド ライト、スチール ライト、オーシャン ダーク、ティール ダーク、エンバー ダーク、インク ダークとする。これらは独自の配色であり、他製品の同名themeとの色互換を表明しない。VS Codeのように利用者がpresetを切り替える操作を提供し、外部themeの取込みや任意色編集は含めない。選択中は現在表示を変えず、既存の保存操作がDATA ownerの`WIN-THEME-PREF-422`に従って成功した後に、開いているMain、Settings、Setup、Graph、Threads、Legalの全Windowへ反映する。取消または保存失敗時は表示中の色と永続設定を変えない。次回起動時は保存したpresetをMain表示前に適用する。theme変更によってquota、Graph系列データ、Threadsの状態、接続、取得要求を変更しない。

| preset | window背景 | card面 | 主要文字 | 補助文字 | accent | Graph背景 | grid | idle band |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `classic-dark` | `#0E141E` | `#151F2D` | `#E9EFF8` | `#A8B7CA` | `#56B2F5` | `#121C2C` | `#263850` | `#1A2838` |
| `graphite-dark` | `#181A1F` | `#242830` | `#F1F3F5` | `#B5BEC9` | `#69B5F7` | `#20242B` | `#3C4652` | `#303944` |
| `light` | `#F4F7FB` | `#FFFFFF` | `#1C2834` | `#526579` | `#176AAB` | `#FFFFFF` | `#CFD9E4` | `#E4EDF5` |
| `paper-light` | `#F7F6F2` | `#FFFFFC` | `#252B31` | `#59636B` | `#356C91` | `#FFFFFC` | `#D8E0E5` | `#ECEFEB` |
| `sand-light` | `#FDF6E3` | `#FFFBEF` | `#334650` | `#566367` | `#1B748A` | `#FFFBEF` | `#C9D6D2` | `#EBE4D2` |
| `steel-light` | `#F3F5F8` | `#FFFFFF` | `#202B38` | `#586978` | `#275FA8` | `#FFFFFF` | `#D5DEE9` | `#E8EEF5` |
| `ocean-dark` | `#10182A` | `#18263D` | `#EAF3FF` | `#ACBED3` | `#56B8F2` | `#111D33` | `#304968` | `#1E304B` |
| `teal-dark` | `#002B36` | `#073642` | `#E6F0E9` | `#A8C0BC` | `#4FB3C3` | `#073642` | `#3C6570` | `#174550` |
| `ember-dark` | `#202126` | `#2B2D32` | `#F4F0E9` | `#BCBDB7` | `#E7BC62` | `#26272C` | `#505258` | `#35373D` |
| `ink-dark` | `#000000` | `#121212` | `#FFFFFF` | `#D8D8D8` | `#6DD3FF` | `#050505` | `#787878` | `#242424` |

E2Eで画面内の色を判定する追加roleは次のexact値とする。`status`はfixtureの正常時に見えるMainのready状態を指す。Graphの線色は描画系列、Threadsの色はカードと接続線へ適用し、文字や面の存在だけを色判定の代用にしない。

| role | `classic-dark` | `graphite-dark` | `light` |
| --- | --- | --- | --- |
| quota gaugeの未充填面 | `#326799` | `#4A6B89` | `#A9CDE8` |
| quota gaugeの充填面・Graph Remaining線 | `#56B2F5` | `#69B5F7` | `#176AAB` |
| Main ready status背景 | `#143426` | `#18362A` | `#E5F5EC` |
| Main ready status枠 | `#276C49` | `#327653` | `#4A9469` |
| Main ready status強調 | `#4FB878` | `#5CC88A` | `#176E42` |
| Threads親card背景 | `#1A2C40` | `#303844` | `#DDEAF5` |
| Threads子・独立card背景 | `#151F2D` | `#242830` | `#FFFFFF` |
| Threads card枠 | `#2B425B` | `#4B5A6B` | `#B6C5D4` |
| Threads接続線・junction | `#76A7CC` | `#8CACBF` | `#6B839A` |
| Threads動作中の状態文字 | `#EF6A6A` | `#EF8585` | `#B23553` |
| Graph selector popup面 | `#111B2C` | `#222730` | `#EEF3F8` |
| Graph selector popup選択行 | `#244D74` | `#344D63` | `#D9EBF8` |
| キーボードfocus枠 | `#8BD4FF` | `#9AD7F8` | `#176AAB` |

新規presetの同じ追加roleは次のexact値とする。

| role | `paper-light` | `sand-light` | `steel-light` | `ocean-dark` | `teal-dark` | `ember-dark` | `ink-dark` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| quota gaugeの未充填面 | `#B7CDD8` | `#B3C9C5` | `#A9C4E1` | `#3D668B` | `#3C7583` | `#77725F` | `#808080` |
| quota gaugeの充填面・Graph Remaining線 | `#356C91` | `#1B748A` | `#275FA8` | `#56B8F2` | `#4FB3C3` | `#E7BC62` | `#6DD3FF` |
| Main ready status背景 | `#E6F3EB` | `#E3F0E2` | `#E5F2EA` | `#16372F` | `#124B40` | `#244437` | `#002B17` |
| Main ready status枠 | `#5C9976` | `#6D9B72` | `#6EAA83` | `#3A8266` | `#47866A` | `#5D9974` | `#78E8A4` |
| Main ready status強調 | `#216543` | `#2B714A` | `#1E7047` | `#72CDA3` | `#79CAA3` | `#9FDC9E` | `#78E8A4` |
| Threads親card背景 | `#E2EDF2` | `#E1E9D9` | `#DDE9F6` | `#213958` | `#123A46` | `#3D3D47` | `#202A34` |
| Threads子・独立card背景 | `#FFFFFC` | `#FFFBEF` | `#FFFFFF` | `#18263D` | `#073642` | `#2B2D32` | `#121212` |
| Threads card枠 | `#B7C7D0` | `#BCCBBC` | `#BACBDD` | `#45617F` | `#3D6C75` | `#686973` | `#FFFFFF` |
| Threads接続線・junction | `#708D9E` | `#728D84` | `#728BA9` | `#82A9C5` | `#76AEB3` | `#A5A3A0` | `#FFFFFF` |
| Threads動作中の状態文字 | `#B23553` | `#A83D48` | `#B42F49` | `#F28B9B` | `#F47D88` | `#F58A94` | `#FF8BA1` |
| Graph selector popup面 | `#F1F4F1` | `#F5EDDA` | `#EDF2F8` | `#192B45` | `#0B3D49` | `#303238` | `#101010` |
| Graph selector popup選択行 | `#DAE9EE` | `#DCE8DB` | `#D7E5F7` | `#284B70` | `#1E5B67` | `#555147` | `#174A66` |
| キーボードfocus枠 | `#276A91` | `#126A7F` | `#275FA8` | `#7CD2FF` | `#74D2DB` | `#F2CB78` | `#FFFFFF` |

上の2表に記した21表示roleは、同じclassic色を共有するcard面／Threads子cardとaccent／quota充填／Graph Remainingをまとめると19個のclassic色キーになる。既存paletteの72キー全件について、新規presetの色は次の決定的な規則で定める。19キーは上表のexact値を優先する。残り53キーは現行`ThemePalette.Colors`の`light`列または`graphite-dark`列のRGB 8-bit値へ、下表の符号付き差分を各channelに加えて0..255へclampし、大文字`#RRGGBB`にする。`ink-dark`だけは各channel `c`に対して`c < 128`なら`floor(3c/4)`、それ以外なら`min(255, floor(5c/4))`とする。未知のclassic色キーは例外で拒否し、元の暗色へのfallbackを行わない。

| preset | 基準列 | R差分 | G差分 | B差分 |
| --- | --- | ---: | ---: | ---: |
| `paper-light` | `light` | +3 | +1 | -5 |
| `sand-light` | `light` | +10 | +3 | -22 |
| `steel-light` | `light` | -3 | -1 | +3 |
| `ocean-dark` | `graphite-dark` | -8 | 0 | +14 |
| `teal-dark` | `graphite-dark` | -22 | +14 | +13 |
| `ember-dark` | `graphite-dark` | +11 | +5 | -6 |
| `ink-dark` | `graphite-dark` | 上記の段階式 | 同左 | 同左 |

`classic-dark`の状態色・model色は維持し、親card背景だけは下記の読みやすさ修正を適用する。他のpresetは同じ状態・modelの識別を色相と状態文の組で保ち、明色面では読める濃色を使う。Graphの線・grid・idle band、Mainのquota/status、Threadsの親card・接続線・動作状態、focus・selector・popupも選択paletteから描画し、一部だけ旧暗色を残さない。画面geometry、情報と状態の意味、Linux版の色は変更しない。preset名と選択操作は対応言語のcatalogとUI Automationで識別できるようにする。実Windowsの同一最終buildで選択、保存、再起動復元、6 Windowの表示色を確認する。

以下の既存節にあるWindows固定HEXのpixel oracleは`classic-dark`へ適用する。他の15 presetでは、同じ情報・状態・描画geometryを保持しつつ、この節のpaletteへ変換した色を照合する。Linuxの固定HEXとpixel oracleは従来どおりとする。

### カラフルな追加6 preset

追加presetの表示名（英語／日本語）はNeon Dark／ネオン ダーク、Lavender Light／ラベンダー ライト、Mint Light／ミント ライト、Forest Dark／フォレスト ダーク、Tangerine Dark／タンジェリン ダーク、Rose Dark／ローズ ダークとする。柔らかいパステル面には濃色の文字を使う。次表は既存19 roleキーの順（window、card、主要文字、補助文字、accent、Graph、grid、idle、quota未充填、ready背景、ready枠、ready強調、Threads親card、Threads枠、Threads接続線、動作中文字、popup、popup選択行、focus）のexact色を固定する。残り53キーは明色presetでlight列、暗色presetでgraphite-dark列を差分0で使い、状態とモデルの色の識別を保持する。

| preset | 基準列 | 19 role色（上記順） |
| --- | --- | --- |
| `neon-dark` | `graphite-dark` | `#16122A` `#241C3B` `#F2ECFF` `#C4B8E2` `#70CBFF` `#1B1530` `#493B68` `#302448` `#37436A` `#163D33` `#4D8B70` `#8BDEB6` `#33264F` `#63517D` `#AF9AD0` `#FF929F` `#2C2144` `#493369` `#B19BFF` |
| `lavender-light` | `light` | `#F2EAFB` `#FFFAFF` `#29233C` `#615570` `#7046AE` `#FFFAFF` `#D8CBE3` `#EBE0F4` `#DDD4EF` `#E3F3E9` `#5C9571` `#216C44` `#E7DDF5` `#BCAACE` `#78658F` `#AC2853` `#EFE7F8` `#DDD0EF` `#7046AE` |
| `mint-light` | `light` | `#E8F7EE` `#F7FFFA` `#19382F` `#3F5F50` `#14765F` `#F7FFFA` `#C8DECF` `#DDEDE3` `#BEDCCD` `#D9F2E1` `#579772` `#207343` `#D7EDE0` `#A8C8B5` `#4D806C` `#AF2D4C` `#E8F6EC` `#C8E8D6` `#11735B` |
| `forest-dark` | `graphite-dark` | `#11231B` `#1B3427` `#EDF8EA` `#ADC9B5` `#94DB75` `#14291E` `#355642` `#263F30` `#40623A` `#193E29` `#508264` `#8CDCAC` `#284833` `#526F5B` `#8BAF90` `#FF949B` `#203F2E` `#31533A` `#B5EA94` |
| `tangerine-dark` | `graphite-dark` | `#29180F` `#3B261A` `#FFF3E5` `#DFC1A5` `#FFB46E` `#2C1D13` `#614533` `#453022` `#735035` `#213D28` `#567D59` `#A3D892` `#4D3424` `#876346` `#D3A37A` `#FF9A96` `#3D2B1D` `#68462C` `#FFCA86` |
| `rose-dark` | `graphite-dark` | `#281523` `#3A2233` `#FCECF5` `#DAB9CD` `#F49DC7` `#2D1B29` `#604258` `#482D3F` `#704762` `#1D3D32` `#507E68` `#9CDBBC` `#4C2F44` `#835A75` `#CC94B5` `#FF969F` `#412838` `#67425B` `#FFBDDF` |

追加6 presetでは主要・補助文字と状態文字は実際の面に対して4.5:1以上、Graphの各系列とfocus枠は3:1以上のsRGB輝度コントラストを保つ。gridとidle面は補助背景として扱い、系列色とは区別する。色の測定は画面表示・UIAの実Windows確認を代替しない。

### 既存presetの読みやすさ修正

利用者の既存配色修正要求により、実際のThreads文字／ボタン文字が4.5:1未満の組だけを修正する。Classic／Graphite／Tealの動作中文字は親card面に対して旧値で3.63／4.32／3.83:1、Sandの補助文字は親card面に対して4.44:1、Tealのprimaryボタン文字は面に対して4.00:1だった。親cardとボタン面の色相を保って暗くし、Sandの補助文字だけを濃くする。

- `classic-dark`はclassicキー`#243E5A`を`#1A2C40`へ対応付ける。他の71キーは従来の同一色を保つ。
- `graphite-dark`の同じ親cardキーは`#303844`とする。この値を他presetの派生基準にも使うが、親cardのexact overrideは優先する。
- `sand-light`の補助文字キー`#A8B7CA`は`#566367`とする。
- `teal-dark`の親cardキー`#243E5A`は`#123A46`とする。さらにprimaryボタン面キー`#236B9E`はRGB差分による派生値をoverrideして`#0D759F`とする。このpresetは20 exactキー／残り52派生キーとなる。

Graph selectorの矢印は文字本文ではなく操作アイコンとして3:1基準を適用する。既存10 presetの矢印と、Graph各系列はこの基準を満たしており変更しない。

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
  再接続に必要な非秘密selector（`connectionProfile`と`connectionSelector`）は、旧6-keyまたは
  `WIN-THEME-PREF-422`の新7-key設定へatomic保存する。

## 2. 絶対UX原則

### 2.1 非スクロール原則

「画面をスクロールしないと主要操作や主要情報へ到達できない」設計はUX合格としない。
対象はMain、Setup、Settings、Graph、Threads、Legalの全Windowであり、Main内Helpにも同じviewport条件を適用する。

- Main: 残量、リセット、状態、更新、メニュー、Graph/Threads/Legal入口を同一viewportに置く。
- Setup: 現在の手順、入力、検証結果、次へ/戻る/キャンセルを同一viewportに置く。
- Settings: 編集対象、現在値、保存、取消、復旧、戻るを同一viewportに置く。
- Graph: 期間、metric、系列操作、plot、現在値を同一viewportに置く。
- Threads: Windowを900×480から拡大せず、空状態または先頭4件の固定行カードと閉じる操作を
  同一viewportに置く。5件目以降だけ一覧内の縦スクロールを許可し、画面全体はスクロールしない。
- Legal: 本文を分割表示できる章/ページとし、戻る・閉じるを常時表示する。長文を理由にアプリ全体の
  ナビゲーションをスクロールの下へ追いやらない。

スクロールバー、マウスホイール、トラックパッドによる画面移動を、主要画面の到達手段として
採用しない。長い一覧・本文はページング、章切替、選択詳細、折りたたみで分割し、現在位置と
次の操作を固定表示する。例外はThreadsの5件目以降だけであり、先頭4件を完全表示した同じ一覧内
`ScrollViewer`で追加行へ到達してよい。画面全体のスクロールや4件以下でのscrollbarはFAILとする。
  ページングや折りたたみでも主要情報を同時に比較できない場合は、レイアウトを再設計する。

### 2.1.1 Window geometry、DPI、topologyの正本

この仕様のgeometryは要求抽出の正本であり、現行実装、既存のfresh画像、fixtureの都合から
昇格・変更してはならない。寸法はOS frameを含まないlogical client sizeで表し、HelpはMain内
surfaceとして扱う。

| surface | registered top-level surface | runtime open HWND | logical client initial | logical client min | logical client max | resize | native controls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Main | yes | 1 | 900×498 | 900×498 | 900×498 | fixed | minimize, close |
| Setup | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Settings | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Graph | yes | 0..1 (singleton) | 940×640 | 700×480 | unbounded | resizable | minimize, maximize/restore, close |
| Threads | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Legal | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Main内 Help | no (owner=Main) | 0 additional | Main client 900×498内 | Main client 900×498内 | Main client 900×498内 | Mainに従う | 独自Window controlsなし |

registered top-level surface inventoryはMain、Setup、Settings、Graph、Threads、Legalの正確に6個で
固定し、Helpを第7 Windowへ分離しない。runtime open HWNDはMain=1＋open child subset=0..5、合計1..6で、
5 childを全て開いた時だけ6となる。各childはsingletonで、runtime cardinalityを6へ固定しない。`700×480`はGraphのminimumだけに属する。Mainのsupported work areaは少なくとも`900×498 logical`、Setup、Settings、
Threads、Legalは少なくとも`900×480 logical`、Graphのsupported
work-area minimumは少なくとも`700×480 logical`である。各境界未満はsupported matrix外として
`unsupported_scope` manifestへ記録し、font縮小、clip、scroll、PASS値の捏造で回避しない。
この境界は新しいproduct failure classや第7 Windowを追加する根拠ではない。

surface/monitor/DPI/sizeのsupported predicateは次のANDで固定する。

```text
supported = client_threshold AND frame_fit
client_threshold(fixed Main) = logical >= 900×498
client_threshold(fixed Setup/Settings/Threads/Legal) = logical >= 900×480
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
先頭4件とWindow操作を同一viewportへ置き、5件目以降に限って一覧内scrollを使う。既存のX版
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
- Main上部は`Monitor / Account / Trends / Legal / Settings`の順で両platformを一致させる。
  Threadsは上部に置かず、受理済みの現在accountのMain概要にある`Details`から開く。
  実行中threadが0件でも同じ導線を表示する。
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
 │    ├─ 現在accountの実行中thread概要 → Details（0件を含むThreads）
 │    ├─ Legal
 │    ├─ Settings
 │    └─ Help / Connection guide
 └─ 起動後の失敗 ─ Monitor（last-good保持または未取得） → 明示された復旧操作
```

### 3.1 初回導入

Setupはウィザード型の段階表示とする。各段階は「現在地」「入力/結果」「次の操作」を持ち、
無効な入力では次へ進めない。旧6-key schemaは`language/setupCompleted/connectionConfigured/timeZoneId/connectionProfile/connectionSelector`
とし、新規保存は`WIN-THEME-PREF-422`の`themeId`を加えた7-keyとする。`connectionProfile=none|wsl|sshConfigAlias`、WSL selectorはinstalled distributionの
exact token、SSH selectorはliteral Host alias（`^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$`）とする。
raw manual host/userはone-session raw recoveryだけで、durable settings・完了状態・再接続selectorにしない。
SSH/WSL childはshell、cmd、PowerShellを介さず、実行ファイルと個別tokenのArgumentListで起動する。

### 3.2 日常監視

Mainを既定の到達先とし、保存済みselectorで次回自動再接続する。接続確認・poll・同一generationの
自動再構築ごとにSetup/app確認を再表示しない。更新は明示ボタンとbounded自動更新を同じ状態機械で扱い、
更新中の再クリック、重複要求、値の一時消去を禁止する。

Mainはstrict validation済み`/v3/current`、Graphは`/v3/history/periods`と選択期間のhistory page、Threadsは`/v3/threads`を使う。account selectorは`/v3/accounts`の非秘密IDを使い、既定を現accountとし、過去accountを一つずつ選択できる。`default_account_id=null`はログアウトとして選択中accountを0件にし、Linux/Windowsとも旧accountのidentity、quota、reset、model、threadを同じ境界で消去して、selectorなし`/v3/current`の`auth_required`を表示する。選択変更時はMain、Graph、Threadsの旧account値、pair、cursor、pending、errorを一括破棄し、選択accountのresourceだけを再取得する。各取得cycleは必要な応答が全て同じaccountかつ同じpublished pairの場合だけatomic置換する。Graph差分だけは直前cursorで既取得prefix不変が証明され、かつ新pairの全pageを受理した場合に限りatomic appendする。prefix補正時は先頭から再取得する。`/v3/current`がexact 404の旧serviceだけ単一`/v3/details`、さらにexact 404の場合だけ単一v2、v1へfallbackする。
exact 404でlegacy details modeへ入った接続は、一つの受理済みdetails rootをMain、Graph、Threadsへ同時投影し、split routeを追加要求しない。再接続時に`/v3/current`から能力判定をやり直す。
Mainは10秒、Graph差分はopen中60秒、Threadsはopen中5秒で確認し、Graph/Threadsを閉じている間は対応requestを送らない。SQLite、別pair、認証control応答でfieldを補完せず、quota/history/threadの再収集、
値の再計算、同一minuteのmerge/max/last/null化をUIで行わない。候補拒否時は該当surfaceだけが同じlast-good rootを保持し、他surfaceやrecorderを変更しない。
Mainが新しいcurrent/threads bundleを受理した周期には、開いたThreads詳細も同じthread行と0件状態へ直ちに追従する。独立5秒取得を追加せず、Main受理前に始めた独立応答で追従結果を戻さない。

### 3.3 詳細・設定・法的情報

子Windowは単一インスタンスとし、既に開いていれば前面化する。子Windowを開いたことでMonitorの
状態やlast-good値を消さない。戻る/閉じるは常時利用可能で、終了時にタイマー・RPC・購読を解除する。

## 4. 画面別UX仕様

この節の抽象的な列挙順より、`DESIGN.md`の情報所有権とlogical layout式、および後発Decision
`UX-20260822-UX-002`、`UX-20260822-GRAPH-001`、`UX-20260822-SSH-001`、
`UX-20260823-ERROR-001`、`UX-20260823-KEYBOARD-001`の具体的な順序・分割・状態遷移を優先する。列挙順を理由に、正本の
component順や表示所有者を変更しない。

### 4.1 Monitor

- 画面上部は両platformとも`利用状況→アカウント→推移→法的通知→設定`の順とし、
  window controlsをその後に置く。Threads入口を上部へ置かない。
- Mainのaccount selectorとそのdropdownは検証済みlogin IDまたはpublic account番号だけを表示し、
  `ログイン中`、`履歴`および各localeで同じ意味の状態語を付けない。現在認証中accountだけ、
  labelの直前にsuccess green `#5DC98A`の`●`を置き、過去accountにはmarkerを置かない。
  選択、partition、logout、Graph/Threadsのaccount表示意味は変更しない。
- 認証済みMainのcomponent順は
  `Header→RemainingQuota→WeekGauge→AccountActivity→ModelUsage→StatusBanner`で固定する。
  残量を最初の主値とし、状態は常時viewport内のStatusBannerだけが所有する。状態を上段の
  duplicate cardへ増やさず、StatusBannerが末尾でもBack/Close/復旧CTAを隠さない。
- 両platformのMain clientは`900×498 logical`、外周は左右`22px`・上下`14px`、内容幅は
  `856px`とする。6行の`y/height`はclient座標で順に`14/52`、`74/82`、`164/78`、
  `250/56`、`314/120`、`442/42`とし、行間は全て`8px`、末尾余白は`14px`とする。
  current/historical account、thread/model/quotaの0件・未取得、warning/error、last-good保持で
  行またはcardを脱着・再flowせず、各固定行の内容だけを状態に応じて表示する。startup loadingは
  Headerを同じ位置に保持し、2行目から6行目までを単一surfaceで覆う。
  認証要求でも別のAuthPanelへ置換せず同じ6行の割当を保持する。Windowsの
  `ShowAuthenticatedContent=false`と同様に中央4 data cardは非表示にし、認証開始・認証ページ・
  確認中・再試行のうち該当する単一CTAだけを末尾StatusBannerに置く。認証要求の
  StatusBannerはWindowsと同じwarning配色を使う。
- Mainのcanvasは`#0E141E`、通常cardはbackground `#151F2D`・border `#263548`/`1px`・
  radius `8px`とする。各Main cardのrootを唯一の外枠とし、同じcardを囲む追加wrapper frameを
  重ねない。status cardだけはWindowsのnormal
  `#143426/#276C49/#4FB878`、warning `#3A2A13/#8A651F/#D5A43A`、error
  `#3A1D24/#8E3D4D/#E06B7A`（background/border/accent）を使い、状態によってcanvas全体を
  着色しない。
- Headerは`210px / 250px / 残幅`の3列と`10px`列間隔を使い、左列に`36×36px`のmark、
  `22px`の利用状況title、`12px`のversionを置く。account selectorは`250×44px`とし、
  閉じたselector内のaccount labelとIDを左`28px`に揃え、矢印は幅全体の右端から`24px`の
  `14px`欄の中央へ置く。展開listの形状と選択動作は変更しない。
  右列に推移、法的通知、設定、最小化、閉じるを置く。期間labelをHeaderへ重複表示しない。
- RemainingQuotaはWindowsの単一card内で主値と概算を並べ、その下のbarをcard内の利用可能幅
  全体（左右`14px`を除く`828px`、高さ`6px`）へ伸ばす。WeekGaugeはWindows Mainの表示を参照し、
  上段の期間名と残り時間を左右に分け、中央に`20px`高の7等幅区分bar、下段にリセットと観測の
  各label・絶対時刻を別欄に置く。文字はWindowsと同じMedium相当の太さとし、上段の期間名`12px`・
  残り時間`13px`、下段のlabel`11px`・時刻`13px`とする。下段の時刻群の配置だけはLinux Mainを
  参照し、reset label、reset時刻、左に`12px`余白を持つobserved label、observed時刻の順に置く。
  各label・時刻の自然幅を確保し、reset時刻の前とobserved時刻の前へ残る横幅を等分する。
  上段の残り時間と下段のobserved時刻は右端を揃え、その列は両値の自然幅の大きい方で決める。
  Windowsでは横間隔`8px`の`Auto,*,Auto,Auto,*,Auto`の6列を使い、下段の4項目を
  0・2・3・5列へ置く。上段の期間名は0～4列、残り時間は5列、中央barは全6列を使う。
  下段はcard内`y=45px`から表示し、日本語・900×498の参照画面ではreset時刻の文字左端が
  約`318px`、observed labelの文字左端が約`445px`、observed時刻の文字右端が約`865px`となる。
  Mainの時刻表記は`PROC-I18N-01`に従い、quota値、期間境界、
  reset/observed epochをUIで再計算しない。
- AccountActivityはWindowsのtotal＋model別件数構成を使い、件数は`THREAD-OPEN-362`の公開対象となった現在openのSession thread集合とする。受理済みの現在accountの
  `open_session_thread_count=0`でも同じ`56px` cardに`0件`、5 modelの`0`、`68×30px`の
  `Details`を表示し、Detailsから空のThreads画面へ進める。未受理のcurrentやhistorical
  accountを0件とみなさず、historical accountではlive件数を表示しないが固定cardは保持する。
  v3のmodel別表示は`動作中（生成済み）`の順とし、SOL 0件動作中/1件openなら`0（1）`、LUNA 1件動作中/2件openなら`1（2）`と表示する。openが0件なら`0`と表示する。総数は公開対象となったopen行の全件数とする。
  同一pairの元行数・動作中行数を検証した後、停止中の親自身とその全子孫を画面から除外した行集合だけでMainの合計とmodel別件数を算出する。wireの元件数を表示件数へ流用しない。
- ModelUsageはLinuxの単一table構成を使い、各modelのInput/Cached input/Outputについてtokenと
  隣接する概算ドルを同じrowに置く。4 model×22pxをcard内に表示し、0件でも`120px`の固定cardと見出しを保持してemptyを明示する。
  値の意味は`MODEL-USAGE-DISPLAY-01`を変更しない。
- StatusBannerはWindowsのstate title、detail、該当する単一CTA、最終受信表示の構成を使う。
  Mainの「前回受信」はLinux/Windows各StatusBannerの高さ方向中央に配置する。platform間で他の座標・寸法を一致させない。
  選択accountのID/labelをtitleまたはdetailへ重複表示しない。
- 0%、中間、100%、未取得、警告、危険、APIエラーで同じcard構造を保つ。認証要求では同じ
  Header・6行の割当・末尾Statusを保ち、中央4 data cardの非表示だけを切り替える。
- エラーは既存値を保持するか未取得として明示し、0や100を仮の有効値として表示しない。
- 数値、単位、説明、状態、操作の文字サイズと太さに役割差を付ける。細すぎるフォント、薄すぎる文字、
  余白だけで分断されたカードは採用しない。
- `ui/components.slint`のMain component root styleを変更する場合は、同fileを`include_str!`で参照する
  既存Rust source oracleを逆引きし、Issue固有testと実画面gateに加えてworkspace/all-targets testを
  実行する。focused selectorまたはpixel gateだけで既存source oracleとの整合をverifiedにしない。

### 4.2 Trends / Graph（master: `CUM-138-06`）

- 期間、ドル/トークン、Remaining/LUNA/TERRA/SOL/ASTRAの操作を上部固定帯に置く。
  model名と累積値は同じaccepted v3 rootから取得し、ASTRAを「その他」へ集約しない。
- 期間・metricのリストはpointer pressの1回で展開し、REST/DB/poll完了を待たずにuser-visible acknowledgementを返す。7日1分bucket由来の10,080点と契約最大1暦月由来の44,640点は対象データ規模であって、通常データを拒否する任意の上限ではない。最低動作環境と承認baselineが定義されるまでは、根拠のない絶対ms値やcold maxをUX合否条件にしない。
- 系列ON/OFFはpointer pressで状態とボタン面を先に更新する。同じ入力でplot画像も必ず変化する。物理入力、UI状態、実描画を別経路で確認する。
- 期間・metricのリストはユーザーの選択で状態を変え、pollやlocale通知による同値候補の再公開では選択へ読み替えない。開いているリストを自動で閉じない。
- 期間変更は`idle → loading → ready|confirmed-empty|failed`の有限状態遷移とする。選択表示は入力直後に更新し、
  accepted same-pair history page集合のparseとpresentation projectionはUI thread外で行う。SQLite再読込やsampleの
  canonicalization/merge/recalculationは行わない。既存の遅延残量補間と終端保持はpresentation-onlyで行い、
  導出点をdetailsやDBへ書き戻さない。処理が次paintまでに終わらない場合は操作を塞がない
  indeterminate progressと「期間データを読み込み中…」を表示し、`記録なし`や`利用不可`を先に表示しない。
  same-pairの全pageが0件と確定した場合だけ`confirmed-empty`へ進む。loading中は直前に完成したgraph・
  metric・軸を保持し、候補完成時だけ1回のUI publishで全てを同時に差し替える。
- 期間を連続選択した場合は旧候補をcancelし、最新revisionだけをpublishする。失敗・timeout・cancelを
  空graphや部分graphへ変換せず、直前graphを保持してbounded errorを表示する。キャッシュ済みで次paint
  までに切替できる場合はprogressを点滅させない。入力への反応と期間データ完成は別条件として確認する。
- current期間の`start_at/end_at`はCOREが同じ最新quota観測から動的に投影したpairを唯一のauthorityとする。
  期間欄、メイン画面の利用期間、selected period start、plotの横軸左端・右端はaccepted periods resourceの
  同じpairをそのまま使い、Windowsで再計算しない。completed/historical期間は保存済みpairを使う。quotaの
  `reset_at`はリセット時刻表示の別項目である。current periodの
  `end_at`は同じatomic published rootのaccepted観測終端であり、UI取得後のlocal現在時刻へ延ばさない。
  completed periodは保存された固定`end_at`までを右端とする。
- 期間欄、横軸、折れ線、右端値は同じselected reset IDだけから一括投影する。poll後の
  bounded reset aliasは60秒以内だけ同一期間として選択を維持し、欄だけ旧期間・plotだけ現在期間の
  混在を禁止する。
- 右端現在値の表示域は、初期940×640 logical表示時に各metricで確保される幅をドル／トークン別に固定する。Graphを横へリサイズした差分はplotへ割り当て、現在値、系列色、leader、縦位置を変えない。
- Remainingは独立0–100%意味、モデル系列は累積値として扱う。残量をドル軸へ誤って合わせない。
- Remainingとモデル使用量は別の観測値であり、モデル使用後に遅れて届いた最初の低い残量観測はその観測時刻へ反映する。残量観測が存在しない区間を料金・tokenから逆算してはならず、未観測区間を正常な残量低下として表示しない。
- `reconstructed-from-session`、`unknown`、`unavailable`および（`legacy-unknown`を除く）`models_complete=false`ではモデル数値を表示せず、
  model key/sourceと欠損metadataだけを表示する。`legacy-unknown`は保存済みの同じmodel keyの値だけ表示でき、集計・予測には使わないが、同一runの同一非空model集合・lossless raw tokenとraw Remainingの完全不変・欠測／異常／confirmed gapなしを全て満たす場合だけidle判定に利用できる。
  直接観測(`confirmed`)または保存済みlossless `legacy-unknown`の値を通常実線とし、補間・hold・smoothing・予測はUI presentation-onlyでAPI/DBへ書き戻さない。
  これらの破線はDBまたは取得記録の欠損・異常を示すNG表示であり、正常な実線の代替ではない。
- model系列の有限表示状態は次を正本とし、model名ごとの全直積には展開しない。

  | 入力状態 | 表示契約 |
  | --- | --- |
  | 同一periodの実測累積が増加／不変 | 増加と未使用確定でない不変は3px実線。sampling由来の同値反復を折れ点にせず、有効な変化点間を単調PCHIPで滑らかにつなぐ。確定idleだけはbandと同じX範囲の1px水平実線へ分離する |
  | 当該model値はログ実測、全model集合は不完全 | 直接観測(`confirmed`)の値だけを保持する。未掲載modelは欠損metadataだけとし、数値を作らない |
  | 他modelの出現／消失、または`confirmed`と`legacy-unknown`の切替 | `confirmed`の同じmodel keyは通常線を維持する。`legacy-unknown`は保存済み同じkeyの値だけを表示し、集計・予測には使わず、同一runが`G137-5`の限定条件を満たす場合だけidle判定に利用できる。新規modelは最初の直接観測時刻から開始し、消失modelのholdは表示専用とする |
  | 正常な直接観測点のtimestampだけが疎 | timestamp差だけでは欠損化せず、同値・増加とも3px実線で結ぶ。明示的なunavailable、confirmed gap、異常とは分離する（`G137-4`,`G137-7`） |
  | model値自体が未取得、または確認済みrecorder停止区間 | model key/sourceと欠損metadataだけを表示し、数値を0補完しない。必要な破線bridge／holdは表示専用で、API/DB、集計、idle判定へ反映しない。`Remaining`からmodel値やtailを外挿しない |
  | 累積が後退し、その後に回復 | source completenessを問わず最後のaccepted値を下回るrawを表示値へ採用せず、当該modelだけを回復点まで細い破線hold／bridgeとする（`G137-3`） |
  | period内の最初の既知点 | 0からの斜線を捏造せず、その値・時刻から開始する |
  | 60秒以内のreset alias／正式reset境界 | 前者は同一periodへ正規化し、後者は別periodとして混ぜない |
  | 確認済み0／model行なし | 前者だけ0として描き、後者は未知として数値・線を作らない |
  | ドル／token切替 | 同じ観測時刻列を使い、単位と値だけを切り替える。ドルはtokenから得る派生表示値でありidle authorityにはしない。同じmodelのraw token不変runでドルだけが変化または欠測した場合は、DB/API rawを変えず左端の有限ドル値へread-timeで水平補正する |

  アイドル帯はsame `reset_at`のperiod内で、run全体が`confirmed && models_complete=true`または保存済みlossless token vectorを持つ`legacy-unknown`であり、同じ非空model key集合、全raw
  `total_tokens`がexact equal、raw Remainingがfiniteかつbitwise equalであり、confirmed gap、token／Remainingの欠測、観測値の矛盾が
  ない区間だけを候補にする。`unknown`、`unavailable`、欠損・補間・hold・smoothing・予測値はendpointまたは
  矛盾なしのauthorityにしない。ただしexact 60秒の1 sampling slotだけが`unavailable`で、その前後がsame reset、同一model集合、全raw token／Remaining exact equalかつgapなしなら、そのrowをread-time表示入力から除外して分断しない。2 slot以上、前後値またはmodel集合の相違、`unknown`、その他の不完全rowは分断する。task lifecycleだけは
  値変化のauthorityにせず、exact equalなaccepted raw値をactive metadataだけで使用済みに変更しない。timestamp不連続だけは
  gapまたは利用の証拠にせず、正常なaccepted raw endpointが上記条件を満たすintervalを分断しない。ドルの変化・欠測・異常はidleの開始・分断・終了条件にしない。上記条件を満たす連続10分以上のrunだけをgray表示し、同じX範囲の全modelとRemainingを1px水平実線にする。観測点数やaccepted raw endpointを欠く欠測時間だけで確定しない。
  10分は画面幅に依存しない意味閾値とし、pixel幅filter、最小表示幅、gridまたはsegment境界によって削除・周期分断しない。
  Remainingは`G137-6`に従い、accepted raw値を元時刻・元値の証拠として保持し、token増分またはtask lifecycleでraw値を
  移動しない。表示geometryでは未使用と確定できないsampling由来の同値反復を折れ点にせず、有効な変化点間を単調PCHIPの
  3px実線で滑らかにつなぐ。確定idleだけはbandと同じX範囲のexactな1px水平実線として分離する。raw-null、unavailable、confirmed gap、異常、
  terminal holdだけを1px破線の予測とし、導出値をanchorへ昇格させずAPI/DBへ書き戻さない。
  `Remaining`のeffective値からmodel系列の値またはそのperiod tailを外挿しない。
  raw-null補間、gap、異常、終端hold等の欠測・予測の破線は
  X版では1px、Windows版では1px相当とする。通常のmodel／Remaining実測は3px、確定idle実線は1px、欠測・予測破線は1pxとし、solid/dashedで意味を区別する。
  破線は幅の広いplotでも切替点が判別できる短く密な周期とし、長い線片・隙間で通常線に見せない。

  plotの描画layerは`background/grid → idle band → series/labels`とする。idle bandは最終合成色`#1A2838`を
  opacity 1でplot全高へ置き、gridを透過させない。既知idle band内のseriesとedgeを避けた同一Yで、major gridの
  X pixelと左右のnon-grid X pixelがいずれもexact `#1A2838`となり、background色またはgrid色の縦columnがbandを
  分断しないことをX screenshotとWindows rasterで検査する。band geometryは時間intervalだけから決め、画面幅や
  pixel幅を理由にintervalを除外しない。Releaseのperiod geometry oracleは、不透明bandに隠れたgridを可視gridとして
  要求してはならない。2点以上の可視gridと、隠れた全grid位置で上下20pxを除く走査高の90%以上を占めるidle色
  （後描画seriesの交差だけを許容）から5点の等間隔gridが一意に定まる場合だけperiod境界を復元し、部分高のidle色、
  idle色のない疎grid、または複数のgrid解はfail-closedで拒否する。

  既知の不完全ASTRAが途中まで増加した後に確定値へ移る場合、左側の増加を消して最初の確定値だけを
  水平表示してはならない。開始・中間・終端値、線種、period ID、右端ラベルを一つの表示candidateとして
  検証し、一項目でも不一致ならその表示を受入れない。
- X版とWindows版は`G137-1`..`G137-10`を参照する同一の履歴fixtureと固定期待値（period/pair、
  累積model、raw/effective Remaining、gap、metric別anomaly、partial/unavailable、未使用区間、期間末）を通過しなければならない。
  片方の描画ヘルパーが生成した値をもう片方の期待値には使用せず、fixtureのliteral oracleを独立に使う。
- finite oracleは、shared rolloverのperiod A→B `100% / $1 → 41% / $323.674247`、
  `graph_delayed_quota`のfirst observation・遅延quota・missing quota、model別回帰/回復、
  confirmed gap、raw-null quota、current/historical右端、no-historyの9 causal caseとする。
  ASTRAのtoken/指定4単価、旧3モデルだけが既知のincomplete period、period選択後pollのbounded reset aliasは
  今回観測した同じ到達経路へ統合し、別の全直積を作らない。
  X/Windowsは同じfixtureの固定期待値を独立に検査し、値形状による100%・7日・quota-only除外、
  platform helperから期待値を生成する循環oracle、新workflow gate、全test/all-suite/全直積を追加しない。
- 操作帯を開閉してもplotの位置・高さを変えず、ラベルや右端値を隠さない。
- 記録なし、欠測、アイドル、活動、0/中間/100を明示的な設計状態として扱う。

### 4.3 Threads

- Linux/Windows共通のWindows masterは900×480 logical client、viewport 384px、96pxの固定行4件とする。
  見出しの直下で一覧を`y=56px`から上詰めにし、件数行と詳細データ鮮度行は表示しない。
  画面中央への自動配置を行わない。5件目以降だけ同じ一覧内で縦scrollを許可し、画面全体はscrollしない。
- 各cardは左80px、右16px、上下6px、height84pxとし、情報laneは`*,180,208`、lane間隔は12pxとする。
  title、model/context、経過時間・指示年齢・tokenを各laneの上端から配置し、1行タイトルと2行タイトルで
  laneを縦方向にcenterしない。
- treeはbase x=10px、depth step=16px、表示depthの上限3、connectorのy=48pxとする。
  tree gutterは情報cardの開始位置と分離し、rail・junction・titleの重なりを0pxにする。
- treeのrow中心は`y=96*i+48`、cardは`x=80`、情報laneの開始は`x=95`とする。railは
  `x=10+16*min(parentDepth,3)`で求め、2pxの実線`#76A7CC`、opacity 1、round cap/joinとする。
  junctionは半径4pxの塗りつぶしdiamond、arrowはtip `(80,y)`、side `(73,y-5)`/`(73,y+5)`とする。
  有効な親を持たない全root row（独立nodeを含む）は`(10,y)`から`(80,y)`のroot segment、x=10のjunction、
  x=80のarrowを持つ。親子railは最後の直接childまで延長し、各childにもsegment、diamond、arrowを描く。
  parentDepthをrail計算前に3へcapするため、depth3 parentからdepth4 childへのrailはx=58となり、兄弟がある場合も最後のchildまで継続する。
- 0件ではtreeのsegment、rail、junction、arrowを描かず、同一windowで親子行を表示した後に0件へ戻っても線を残さない。
- 子を持つrowだけを`row.has-children`で親と判定し、親cardのbackgroundは`#1A2C40`、独立nodeとleafのbackgroundは
  `#151F2D`、全cardのborderは`#2B425B`とする。入れ子の親も対象にし、独立nodeとleafは親色にしない。
- model accentはASTRA=`#E86E9F`、LUNA=`#F1B35A`、TERRA=`#71D39A`、SOL=`#B79BFF`、その他=`#A8B7CA`とし、
  2px×12px、model文字の開始位置はaccent基準から8px後、laneは上詰めとする。
- 子threadの表示名は`THREAD-TITLE-362`に従い、`thread/read`で取得する上流保存名をそのまま使う。生成元がtask_nameを
  外部の`thread/name/set`で保存している場合はその値を表示し、保存名が空なら`未設定`とする。
  「アクティブなスレッド」などの汎用名やpreview値へfallbackしない。
- 一覧は`THREAD-OPEN-362`の公開対象となった現在openのSession threadを含み、各行の明示状態を「動作中」「停止中」「未観測」で表示する。未観測を推測で置換しない。
  初回受理行と同一pairの独立したThreads再取得行のどちらでも、停止中の親自身とその全子孫を除外する。残る子は元の`parent_thread_id`を持つ親にだけ接続し、同じ表示名の別の子と混同しない。
  動作中の状態文字はdanger赤`#EF6A6A`、停止中と未観測は従来のsecondary文字色とする。親card背景とmodel accent色は変更しない。
- 動作中のrootを停止中・未観測のrootより先に表示し、同じ状態のrootと兄弟の既存順を維持する。各rootの直後に公開対象の子孫をdepth-firstで連続表示し、子だけを親から切り離して並べ替えない。
- contextは同じthreadの観測済みusage/window pairだけを表示し、usage=0は有効な0%として表示する。
  pairがない、windowが0以下、または旧checkpointのNULLは`未観測`とし、累積値や別sourceから推測・合成しない。
  割合は整数比をround-half-upで小数点以下最大2桁へ丸め、末尾0を除去し、100%を上限とする。次の観測で得たpairは
  checkpointへ保存し、後続append/restartで未観測へ戻さない。
- stale Session、orphan、cycle、部分snapshotは誤って現在openとして表示しない。現在openの停止済みchildは、停止中の祖先を持たず公開対象なら停止中として表示する。
- 一覧件数が増えても本文fontを縮小せず、Window拡大や空疎なcardで情報密度を下げない。contextを埋めるための
  full old-history reread、追加poll、backfillを行わない。

### 4.4 Setup / Settings

- Linux Settingsはtimezone一項目だけを持つsingleton Windowとする。選択肢と保存値はexact
  `local|UTC`で、未保存変更はMain/Graphへ反映せず、取消またはCloseで破棄する。保存成功時だけ
  同一directory内のatomic renameを完了してから開いているMain/Graphの時刻表示へ即時反映し、Windowを閉じる。
  成功した保存値は通常のprocess再起動後に復元する。
  保存失敗時はactive timezoneと既存表示を保持し、Settingsを開いたまま失敗を表示する。
  このLinux timezone変更自体はWindows Settingsを変更しない。Windowsの後続theme設定と7-key保存は`WIN-THEME-422` / `WIN-THEME-PREF-422`に従う。
- profile/selector、API到達性、readiness health、details state、auth-start、auth-checkを別概念として表示する。
- 旧exact settings keysは`language/setupCompleted/connectionConfigured/timeZoneId/connectionProfile/connectionSelector`。
  新規保存の7番目の`themeId`は`WIN-THEME-PREF-422`が所有する。
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

Setupの順序はserver/API prepare→listener→readiness `GET /health`→strict `GET /v3/current`（新resourceがexact 404の旧serviceだけv3 details→v2→v1）→
必要時だけauth-start→別auth-check→新しいstrict currentで固定する。auth-start/auth-checkはcontrol-onlyであり、
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
- Help/Connection guideはMain client `900×498 logical`内の情報surfaceであり、独立Window/HWNDを
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
  はMainで少なくとも900×498 logical、Setup/Settings/Threads/Legalで少なくとも900×480 logical、Graphで少なくとも700×480 logicalを必要とし、未満は
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
| 証拠 | 影響master IDの直接オラクル。機械判定できない表示意味に限りfresh画像と独立評価 |
| 未確定 | 解消条件と担当 |

## 7. UX受入ゲート（実装開始後に使用するが、抽出中は実行禁止）

次をすべて満たさない限りUX PASSにしない。

1. 登録6 surfaceのruntime open HWND（Main=1＋child subset 0..5、合計1..6、child singleton）とMain内Help additional HWND=0を満たし、全surfaceの主要情報・主要操作・戻る/閉じるがページスクロールなしで到達できる。
2. 画面サイズ、DPI、マルチモニタ、locale、状態、エラー、空データ、長文の各状態で同じ優先順位を保つ。
3. メニューから全画面へ到達でき、子画面は単一インスタンスで再利用される。
4. 同一fixtureでX版とWindows版のデータ意味論が一致し、差分は判断記録にある。
5. 文字、アイコン、色、フォーカス、キーボード、読み上げ名、入力非奪取は影響する項目だけを直接検査する。
   画素・UIA・操作ログで判定不能な表示意味が残る場合だけ、その項目に独立評価を1回使う。
6. 主画面の値と状態、Graphの軸と折れ点、Threadsのlive判定、Setupの接続境界、設定/履歴保持が、
   最新artifact SHAとraw証拠に結び付いている。
7. 各surfaceのlogical client threshold AND DPI後DWM visible_frameのrcWork完全包含を満たし、
   reopen invalid時のtopology_recovery reasonとtimer/poll/drag後recenter=0を記録する。
8. 一つでも未確認、`INCONCLUSIVE`、`HOLD`、FAILがあればUXと製品を完了扱いにしない。
9. 全メニュー、selector、toggleで、物理入力に対する状態更新とpaintを別経路で確認する。
   backend poll中も入力を塞がず、同じ機能結果を確認する。

このゲートは、実装者の「見た目は良い」「動いた」という自己判断を受入証拠の代わりにしない。
