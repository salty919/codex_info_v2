<!-- codex-info-requirement-owner: UX -->
<!-- codex-info-master-ids:
RUNTIME-SETTINGS-481
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
WIN-SETTINGS-TABS-590
WIN-WINDOW-BOUNDARY-603
WIN-SETTINGS-REFRESH-603
WIN-CONNECTION-GUIDANCE-603
-->

# Windowsクライアント UX設計仕様

## 0. 状態と適用範囲

本書は登録UX契約の正本である。接続・保存・headlessの責務はPRODUCT/DATA/WIREのownerへ参照で結合し、installed起動commandはPRODUCTのlauncher/service契約に従う。仕様の規範性と、実装・host・artifact・fresh imageによる品質確認の進捗を混同しない。文書が存在することだけで製品PASSを主張しない。

この文書は、Windows版を「表示できるもの」にするためではなく、顧客が迷わず
監視・接続・復旧・設定・更新・削除できる製品として設計するためのUX正本である。
X版はデータ意味論、状態、所有権の参照元であり、Windows版のレイアウトや操作を
無条件に複製する根拠にはならない。Windows固有の判断は、この文書に目的・代替案・
採用理由・影響する要求ID・受入証拠を登録しない限り採用してはならない。

実装開始には、対象経路の全入力、採否、状態、操作結果、失敗時保持、外部依存がowner仕様から一意に決まることを必須とする。未決定条件を実装へ持ち越さない。対象はユーザーが許可した要求とその直接依存に限り、他の契約の文書化や過去の実装許可を理由にscopeを増やさない。品質確認の未完了は仕様未決定を意味せず、仕様確定も品質確認完了を意味しない。

## RECORDED-USAGE-SCOPE-604 — 既存の期間・価格・描画仕様の保持

PRODUCTの同IDは原本集計と保存値の資格を定め、既存UIを置換する権限を与えない。ユーザーが要求していない「記録範囲」、rolling＋UTC月の期間一覧、記録分／保存観測の切替、全保存値の点表示、階段強制は採用しない。

- Mainの期間・モデル別tokenと隣接する参考ドルは既存契約を維持する。必要成分と採用済みcatalogがある参考額を常時「価格未確定」へ変更しない。保存済み過去額の保持と参考額計算はPRODUCT `MODEL-PRICING-463`に従う。
- Graphは週期間・24時間・1週間・1か月、日付group内の各期間選択、前後移動を維持する。期間一覧とoriginは既存のexact保存key・掲載条件から構成し、補助APIのrolling/UTC月一覧へ置換しない。
- 月は表示timezoneの暦月を使う。モデル系列ON/OFFで全modelの月・日合計を変えない。未計測と既知0を区別し、正の利用記録がない日は既存月仕様どおり控えめにする。
- 正常な連続run、PCHIP、実線/破線、hover、idle、forecastは既存G137と本書の各詳細規則に従う。period identityが不明であることだけで既知観測の全連結を停止しない。疎な観測と真の欠測・非所有・reset・数値異常を区別し、未知の0/100%や利用のない帯を作らない。
- 必要resourceを同account・同pairで受理してからatomic更新する。失敗・不一致では該当surfaceのlast-goodを保持する。補助集計の採用や新しい操作を、既存画面を利用する条件にしない。

この復旧は2026-10-18のユーザー指示に従う既存UIへの復旧であり、新UI仕様の承認ではない。従来の各節と矛盾する旧置換記述を本節の根拠として再導入しない。

## WIN-WINDOW-BOUNDARY-603 — Windows各Windowの境界

registered top-level surfaceであるMain、Setup、Settings、Graph、Threads、Legalの各Windowは、全周に連続した可視境界を持つ。境界線は1 logical DIP以上、Window内面に対する相対輝度contrast ratioは3:1以上とする。境界は同じthemeのWindow同士が重なった場合も各Windowの端を見分けられ、shadowだけを境界の代わりにしない。6 Windowすべてで同じtheme roleを使い、既存のlogical client size、内容配置、native controls、移動・resize動作を変えない。全themeで境界が読め、主要内容や操作を隠さないことを実Windowsの重なり表示で確認する。

6画面共通の視覚ルールは本節をownerとし、`App.axaml`のstyle/resourceを共通実装とする。画面見出しは22 DIP/SemiBold、補助文は13 DIP/補助色（数値表・密集した注記は12 DIP）、window操作は36×28 DIPの共通styleとし、操作のprimary/secondary役割・角丸・hover表示を共有する。枠、見出し、操作、状態色は画面別に再定義しない。本文の間隔は4 DIP単位（8/12/16/24）を基本に、情報密度と既存の固定geometryを保持する。status/モデル色の意味は既存roleを保持し、装飾色として流用しない。

アイコンは共通のvector geometryを使い、設定・推移・接続・スレッド・ライセンスの見出し/入口を識別する。操作iconは16 DIP、見出しiconは20 DIP、文字との間隔は8 DIPを基本とする。閉じる等の標準window操作以外は文字を併記し、iconだけに意味を委ねない。フォントの絵文字/glyphを機能iconとして混在させない。Mainの入口buttonは最低70 DIPを維持し、iconと翻訳labelに合わせて幅を伸ばす。

## WIN-SETTINGS-TABS-590 — Windows Settingsの2タブ切替

Windows Settingsは現行の900×480を維持し、タイトルの下に「表示」「接続状態」の2タブを置く。初期表示は「表示」タブとする。タブ内容は一度に一つだけ切り替え、設定画面下部の操作footerは常時固定する。

タイトルを除く設定内容は幅660 DIPの中央列に収め、タブ帯・内容フォーム・footerの左右端を揃える。タブ列は列幅660 DIPとし、各タブは高さ36 DIP、文字に合わせた幅（最小88 DIP）、間隔4 DIPとする。ラベルを折り返したり切り詰めたりせず、両タブは同じ控えめな面・枠で表示する。選択状態にだけ小さな色差を付けて現在位置を判別できるようにし、選択中のタブ自体を大きな面や独立カードとして強調しない。keyboard focus-visibleは選択状態と混同しない既存focus表示を維持し、装飾アニメーションは使わない。

「表示」タブには言語、表示タイムゾーン、外観（テーマ）、表示対象アカウントをまとめる。「接続状態」タブには接続先、接続・認証状態、Recorder/REST versionをまとめる。既存の項目と選択肢を欠落・重複させない。

各タブの本文はfooterから視覚的に分かれ、関連する設定項目を一つの控えめな枠でまとめる。「表示」の4項目は同じ枠の中に置く。「接続状態」は接続情報と更新状態を別々の枠で示す。枠は選択状態やkeyboard focusと混同しない。既存の900×480 Window、中央660 DIP列、入力寸法、固定footerを維持し、枠を加えるために主要情報を欠落・切り詰めない。

「表示」タブの4項目は同じラベル・入力欄の整列したフォームに置き、列はラベル160 DIP、間隔12 DIP、入力488 DIP（合計660 DIP）、各入力欄は高さ36 DIP、項目間は12 DIPとする。フォームの左右に追加marginを置かず、タブ・footerと同じ中央列の端に合わせる。表示対象アカウントはこのフォームの1項目として「表示対象」と選択中アカウントを表示する。タイトル行、タブ、独立カードには置かず、アカウント別設定と誤認させる説明文を加えない。フォームは高さ36 DIPの4行を持つGridとし、account行にselectorだけを含む`SettingsAccountSelectorAnchor`を置く。`AccountMenu`の外枠は同じフォームGridの2列目に重ね、selectorの上へ開く。外枠の幅と最大幅はselectorと同じ488 DIP、最大高144 DIPとし、selectorの上端にメニュー下端を合わせる。続きの選択肢は既存ListBox内でscrollする。メニュー開閉でフォーム内容を移動させない。実際の隣接表示、画面内での操作可能性、内容が移動しないことは実Windowsで確認する。アカウント選択は既存どおりMain、Graph、Threadsと共有する表示対象を即時に切り替える。認証、設定値、Save対象にはせず、タブ切替、Save、Closeで選択を戻さない。

「表示」タブでは設定項目を同じSettingsViewModelへbindingし、タブ切替は表示だけを切り替える。選択値の初期化、保存、破棄を発生させず、既存のdraft保持とSave動作を維持する。Saveに加えてCancelを常時表示する。Cancel、WindowのClose、Escapeは保存していない言語・timezone・theme draftを破棄してWindowを閉じ、theme previewは保存済みpaletteへ戻す。表示対象accountは即時に切り替わる既存契約を維持し、Cancel/Closeで戻さずSave対象にも含めない。設定項目、選択肢、保存形式、Linux画面は変更しない。外観はテーマ選択欄だけを置き、星印の装飾、`AppearanceDescription`の説明カード、補足段落を表示しない。

「接続状態」タブでは接続先、通常status、認証操作、Recorder/REST version、version statusを実データの行としてまとめ、各version値は名称と同じ行に置く。接続先などの通常情報はこのタブ内だけに表示し、他タブやWindow見出しに重ねない。空欄を埋める文や装飾的な情報カードを追加しない。状態の詳細は更新結果として`WIN-SETTINGS-REFRESH-603`に従って表示する。root scrollを要求せず、入力、実status、固定操作の全てへ900×480内で到達できるようにする。

Save失敗時だけ固定footer直上の共通欄を表示し、どちらのタブからも理由を確認できるようにする。保存エラー欄にsnapshot説明を表示しない。下部footerでは接続設定／ライセンス情報を左のsecondary操作群、CancelとSaveを右側の操作群として分け、Saveをprimaryとする。ライセンス情報はSettingsから開き、Main上部にはLegal入口を置かない。Settings footerの接続先編集入口は`ConnectionSettings`、ライセンス画面見出しとSettings入口は`LicenseInformation`を使い、`docs/LOCALIZATION.md`の全対応localeで表示する。接続先編集ボタンは既存Setup画面を開き、クリック直後に前面で操作可能になる。既存Setup画面をSettingsから開いた場合のタイトルと見出しは`WIN-CONNECTION-GUIDANCE-603`に従う。

## WIN-SETTINGS-REFRESH-603 — Settings接続状態の更新

Settingsの「接続状態」タブにある操作は、認証確認だけを行うように見せず、実際に呼び出す既存Main refreshとRecorder/REST runtime version更新を示すローカライズ済み「状態を更新」相当のlabelにする。操作中は更新中であることを同じタブに表示し、完了後はMainの状態ownerが返す接続結果と詳細を同じタブ内に表示する。正常接続の利用枠/リセット警告を接続失敗と混在させず「接続済み」と更新時刻を表示し、失敗/認証要求ではその理由を表示する。成功、認証が必要、接続失敗を区別できる文言を示し、結果を背面のMainだけに表示しない。Save失敗の欄は設定保存のownerとして独立して保ち、接続状態更新で上書きしない。更新は設定draftを保存・破棄しない。

## WIN-CONNECTION-GUIDANCE-603 — 接続設定の目的と案内

Settings footerから既存Setupを開いた時は、Setup初回導入のwelcome title/headingを再利用せず、既存`ConnectionSettings` catalog valueをWindow titleとページ見出しに使う。初回導入から開くSetupのwelcome headingは現行の全対応locale値を維持する。どちらの入口も既存のSetup状態遷移、認証順序、保存条件、完了条件を共有し、入口によって別の接続動作を作らない。

Connection guideは長文で手順を説明せず、接続先・現在の状態・次の操作を近接して配置する。接続、認証、完了の現在ステップを視覚表示し、そのステップの操作を提示する。手動SSH設定と起動コマンドは展開式の詳細に収め、通常の接続選択と競合させない。既存の詳しい案内は詳細内から参照できる。手動で用意したloopback接続は「既存の接続を使う」と表示し、内部値`none`を接続先欄に見せない。更新ボタンは認証専用を意味する名前にせず、進行中と結果を表示する。設定から開いた時に「初期設定」やwelcomeを主目的として表示しない。機能、順序、保存境界を保持し、全対応localeと実Windowsで確認する。

## WIN-THEME-422 — Windows版の組込みカラーテーマ

Windows Settingsの外観欄は`classic-dark`（従来配色・既定）、`graphite-dark`、`light`、`paper-light`、`sand-light`、`steel-light`、`ocean-dark`、`teal-dark`、`ember-dark`、`ink-dark`、`neon-dark`、`lavender-light`、`mint-light`、`forest-dark`、`tangerine-dark`、`rose-dark`の順に16種類の組込みpresetを選択できる。新規presetの表示名は順にPaper Light、Sand Light、Steel Light、Ocean Dark、Teal Dark、Ember Dark、Ink Darkとし、日本語では順にペーパー ライト、サンド ライト、スチール ライト、オーシャン ダーク、ティール ダーク、エンバー ダーク、インク ダークとする。これらは独自の配色であり、他製品の同名themeとの色互換を表明しない。VS Codeのように利用者がpresetを切り替える操作を提供し、外部themeの取込みや任意色編集は含めない。テーマ選択時に、開いているMain、Settings、Setup、Graph、Threads、Legalの全Windowへ即時プレビューする（Issue #571）。選択だけでは永続設定と実行中の保存済み設定generationを変更しない。既存の保存操作がDATA ownerの`WIN-THEME-PREF-422`に従って成功した時だけ選択を確定し、取消・設定画面を閉じる操作または保存失敗時は保存済みの配色へ戻す。保存失敗時はテーマ選択欄も保存済みの値へ戻し、表示名と配色を一致させる。保存成功後に閉じても確定済みの配色を保持する。次回起動時は保存したpresetをMain表示前に適用する。theme変更によってquota、Graph系列データ、Threadsの状態、接続、取得要求を変更しない。

| preset | window背景 | card面 | 主要文字 | 補助文字 | accent | Graph背景 | grid | idle基準色 |
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
| quota gaugeの充填面 | `#56B2F5` | `#69B5F7` | `#176AAB` |
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
| quota gaugeの充填面 | `#356C91` | `#1B748A` | `#275FA8` | `#56B8F2` | `#4FB3C3` | `#E7BC62` | `#6DD3FF` |
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

上の2表に記した21表示roleは、同じclassic色を共有するcard面／Threads子cardとaccent／quota充填をまとめると19個のclassic色キーになる。paletteの72キー全件について、新規presetの色は次の決定的な規則で定める。19キーは上表のexact値を優先する。残り53キーの基準値は次の設計表とし、実装内のpaletteを正本にしない。classic色キー自体がclassicの色であり、他presetへの派生はこの表の`light`列または`graphite-dark`列のRGB 8-bit値へ、後続の符号付き差分を各channelに加えて0..255へclampし、大文字`#RRGGBB`にする。`ink-dark`だけは各channel `c`に対して`c < 128`なら`floor(3c/4)`、それ以外なら`min(255, floor(5c/4))`とする。未知のclassic色キーは例外で拒否し、元の暗色へのfallbackを行わない。19の明示roleと補助面・状態色を同じ基準から派生させ、保存済みpresetで画面の一部だけ配色が変わることを防ぐ。

| 補助色のclassicキー | `graphite-dark`基準 | `light`基準 |
| --- | --- | --- |
| `#0A111A` | `#15171B` | `#F1F4F8` |
| `#101925` | `#1D2026` | `#F0F4F8` |
| `#101F2D` | `#20242B` | `#EAF0F6` |
| `#111A27` | `#20232A` | `#E9EFF6` |
| `#172C42` | `#1D354A` | `#E7F1FA` |
| `#18283A` | `#292F39` | `#EAF0F6` |
| `#1B2735` | `#292D34` | `#EBEFF3` |
| `#1B2838` | `#292F39` | `#EAF0F6` |
| `#1B2A3A` | `#2B313A` | `#E8EEF5` |
| `#1C5D91` | `#285A80` | `#D7EAF7` |
| `#1D2A38` | `#292D34` | `#EDF1F5` |
| `#236B9E` | `#2676A8` | `#176AAB` |
| `#24415E` | `#30485D` | `#D7EAF7` |
| `#263548` | `#3A444F` | `#D5DFE9` |
| `#294968` | `#35506A` | `#D9E9F7` |
| `#2A3A4B` | `#454C55` | `#C7D0DA` |
| `#2C4258` | `#424B57` | `#C5D0DC` |
| `#2D3D56` | `#444F5C` | `#C0CDDA` |
| `#2D6193` | `#3972A0` | `#5A91C2` |
| `#304052` | `#454C55` | `#C7D0DA` |
| `#304A63` | `#4B596B` | `#B6C5D4` |
| `#356087` | `#3D617F` | `#C7E0F4` |
| `#36516B` | `#506071` | `#B8C8D7` |
| `#3A1D24` | `#41262B` | `#FDECEF` |
| `#3A2A13` | `#42331B` | `#FFF3DC` |
| `#3B506F` | `#536073` | `#B6C5D4` |
| `#405779` | `#54677C` | `#AEC1D2` |
| `#47769F` | `#5B7E9E` | `#82A9CB` |
| `#5DC98A` | `#70CF99` | `#16794B` |
| `#5EA7E5` | `#6EB5E9` | `#267DB7` |
| `#61B4EB` | `#72BDEA` | `#267DB7` |
| `#718196` | `#9AA8B8` | `#5F7183` |
| `#71C7FF` | `#84C7F3` | `#176AAB` |
| `#71D39A` | `#83D4A7` | `#16794B` |
| `#78879C` | `#A2ADB8` | `#526579` |
| `#8A651F` | `#A98037` | `#B8892D` |
| `#8E3D4D` | `#AB5362` | `#C77382` |
| `#8EA1B5` | `#9DAAB6` | `#687A8A` |
| `#9FB0C5` | `#B5BEC9` | `#526579` |
| `#A88CF5` | `#B69CF1` | `#6A4BCC` |
| `#B79BFF` | `#C5AFF5` | `#6A4BCC` |
| `#B8C6D7` | `#C9D0DA` | `#526579` |
| `#D5A43A` | `#E0B255` | `#805800` |
| `#E06B7A` | `#E5808C` | `#A32746` |
| `#E6A23C` | `#EBB460` | `#985F08` |
| `#E6B85C` | `#ECC779` | `#805800` |
| `#E7C26A` | `#EBCF89` | `#805800` |
| `#E86E9F` | `#EC87AC` | `#B23553` |
| `#EAF2FB` | `#F2F4F7` | `#1C2834` |
| `#F0A35B` | `#F2BA82` | `#985F08` |
| `#F1B35A` | `#EFC176` | `#985F08` |
| `#F2F6FC` | `#F7F8FA` | `#1C2834` |
| `#F5FAFF` | `#F8FBFF` | `#FFFFFF` |

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

- Main: 残量、リセット、状態、更新、メニュー、Graph/Settings入口を同一viewportに置く。ライセンス情報はSettingsから開く。Windowsのモデル内訳だけは先頭6行を完全表示し、7行目以降を同じ表内で縦scrollする（`MODEL-USAGE-DISPLAY-01`）。
- Setup: 現在の手順、入力、検証結果、次へ/戻る/キャンセルを同一viewportに置く。
- Settings: 編集対象、現在値、保存、取消、復旧、戻るを同一viewportに置く。
- Graph: 期間、metric、系列操作、plot、現在値を同一viewportに置く。
- Threads: Windowを900×480から拡大せず、空状態または先頭4件の固定行カードと閉じる操作を
  同一viewportに置く。5件目以降だけ一覧内の縦スクロールを許可し、画面全体はスクロールしない。
- Legal: 本文を分割表示できる章/ページとし、戻る・閉じるを常時表示する。長文を理由にアプリ全体の
  ナビゲーションをスクロールの下へ追いやらない。

スクロールバー、マウスホイール、トラックパッドによる画面移動を、主要画面の到達手段として
採用しない。長い一覧・本文はページング、章切替、選択詳細、折りたたみで分割し、現在位置と
次の操作を固定表示する。Threadsは先頭4件を完全表示した同じ一覧内
`ScrollViewer`で5件目以降へ到達してよい。Windows Mainのモデル内訳は先頭6行を完全表示した同じ表内で7行目以降へ到達してよい。画面全体のスクロール、Threadsの4件以下、Windows Mainのモデル6行以下でのscrollbarはFAILとする。
  ページングや折りたたみでも主要情報を同時に比較できない場合は、レイアウトを再設計する。

### 2.1.1 Window geometry、DPI、topologyの正本

この仕様のgeometryは要求抽出の正本であり、現行実装、既存のfresh画像、fixtureの都合から
昇格・変更してはならない。寸法はOS frameを含まないlogical client sizeで表し、HelpはMain内
surfaceとして扱う。

| surface | registered top-level surface | runtime open HWND | logical client initial | logical client min | logical client max | resize | native controls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Main | yes | 1 | 900×542 | 900×542 | 900×542 | fixed | minimize, close |
| Setup | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Settings | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Graph | yes | 0..1 (singleton) | 940×640 | 700×480 | unbounded | resizable | minimize, maximize/restore, close |
| Threads | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Legal | yes | 0..1 (singleton) | 900×480 | 900×480 | 900×480 | fixed | minimize, close |
| Main内 Help | no (owner=Main) | 0 additional | Main client 900×542内 | Main client 900×542内 | Main client 900×542内 | Mainに従う | 独自Window controlsなし |

registered top-level surface inventoryはMain、Setup、Settings、Graph、Threads、Legalの正確に6個で
固定し、Helpを第7 Windowへ分離しない。runtime open HWNDはMain=1＋open child subset=0..5、合計1..6で、
5 childを全て開いた時だけ6となる。各childはsingletonで、runtime cardinalityを6へ固定しない。`700×480`はGraphのminimumだけに属する。Mainのsupported work areaは少なくとも`900×542 logical`、Setup、Settings、
Threads、Legalは少なくとも`900×480 logical`、Graphのsupported
work-area minimumは少なくとも`700×480 logical`である。各境界未満はsupported matrix外として
`unsupported_scope` manifestへ記録し、font縮小、clip、scroll、PASS値の捏造で回避しない。
この境界は新しいproduct failure classや第7 Windowを追加する根拠ではない。

surface/monitor/DPI/sizeのsupported predicateは次のANDで固定する。

```text
supported = client_threshold AND frame_fit
client_threshold(fixed Main) = logical >= 900×542
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
置く。ただしWindows Mainのモデル内訳は`MODEL-USAGE-DISPLAY-01`の6行＋表内scrollを適用する。Graphもperiod/metric/series/plot/現在値とBack/Closeを同一viewportへ置く。Threadsは
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
- Main上部は`Monitor / Account / Trends / Settings`の順で両platformを一致させる。Legal入口はSettings footerへ集約する。
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

Mainはstrict validation済み`/v4/current`、GraphはWIREのversion優先順位に従うdirectoryと選択期間のv3 history page、Threadsは`/v3/threads`を使う。account selectorは`/v3/accounts`の非秘密IDを使い、起動時の既定と起動後の選択維持は`ACCOUNT-LIFECYCLE-134`に従う。`default_account_id=null`の新規起動または現accountへの追従中は選択中accountを0件にし、Linux/Windowsとも旧accountのidentity、quota、reset、model、threadを同じ境界で消去して、selectorなしのcurrent resourceが確認した`auth_required`、`initializing`、`error`の該当状態を表示する。default nullだけを未認証の証拠にしない。選択変更時はMain、Graph、Threadsの旧account値、pair、cursor、pending、errorを一括破棄し、選択accountのresourceだけを再取得する。各data取得cycleは必要なsnapshot応答が全て同じaccountかつ同じpublished pairの場合だけatomic置換する。global account catalogはWIREに従って独立受理し、明示選択した歴史accountのdata pairとの一致を要求しない。catalog更新からdata pair/cursorを作らず、account選択の境界だけを適用する。Graph差分だけは直前cursorで既取得prefix不変が証明され、かつ新pairの全pageを受理した場合に限りatomic appendする。prefix補正時は先頭から再取得する。`/v4/current`のexact404だけ`/v3/current`、さらにexact404の旧serviceだけ単一`/v3/details`、さらにexact 404の場合だけ単一v2、v1へfallbackする。
exact 404でlegacy details modeへ入った接続は、一つの受理済みdetails rootをMain、Graph、Threadsへ同時投影し、split routeを追加要求しない。再接続時に`/v4/current`から能力判定をやり直す。
Mainは10秒、Graph差分はopen中60秒、Threadsはopen中5秒で確認し、Graph/Threadsを閉じている間は対応requestを送らない。SQLite、別pair、認証control応答でfieldを補完せず、quota/history/threadの再収集、
値の再計算、同一minuteのmerge/max/last/null化をUIで行わない。候補拒否時は該当surfaceだけが同じlast-good rootを保持し、他surfaceやrecorderを変更しない。
Mainが新しいcurrent/threads bundleを受理した周期には、開いたThreads詳細も同じthread行と0件状態へ直ちに追従する。独立5秒取得を追加せず、Main受理前に始めた独立応答で追従結果を戻さない。

明示選択IDの消失でPRODUCTが未選択へ遷移させた場合は、Main/Graph/Threadsの旧account値と未完了要求を同じ境界で破棄する。
Mainは「アカウント未選択」と、設定から表示対象を選べることを表示localeで示し、primary CTAを「設定」とする。
quota/model/threadの未知を0や100%で表示せず、selectorなしdata要求によって別のdefault accountを表示しない。
未選択中もglobal catalogの通常確認、Settings、Legal、Window操作は利用可能である。新規起動やcurrent追従中の
default nullでは、前段のselectorなしstrict boundary表示を使う。この二つの未選択理由を相互に置換しない。

### 3.3 詳細・設定・法的情報

`API-V3-MODELS-01`の不完全なcurrentを受理した場合も、確認できた最新modelの内訳・概算額とGraphを表示し続ける。Mainの詳細状態文言とUI Automationを最新/readyにせず、Graphにも同じ取得状態の警告を表示する。警告はplotや履歴読取りエラーのoverlayとは独立し、収集回復時に同じ状態通知で解除する。通信失敗時は既存last-goodを保持して接続エラーを区別する。

子Windowは単一インスタンスとし、既に開いていれば前面化する。子Windowを開いたことでMonitorの
状態やlast-good値を消さない。戻る/閉じるは常時利用可能で、終了時にタイマー・RPC・購読を解除する。

## 4. 画面別UX仕様

この節と本書の各契約節が画面の情報所有権、順序、logical layout、操作と失敗状態を所有する。
`DESIGN.md`は非規範の構造案内であり、本書を上書きしない。過去のDecision IDだけを参照して、
本書にない状態遷移・定数・操作結果を実装者が補わない。取得と保存の採否はPRODUCTが登録する各ownerを参照する。

### 4.1 Monitor

- 画面上部は両platformとも`CODEX情報→推移→法的通知→設定`の順とし、
  window controlsをその後に置く。Threads入口を上部へ置かない。
- account selectorは両platformともSettingsだけに置き、MainとGraphに切替操作を置かない。
  selectorとそのdropdownは検証済みlogin IDまたはpublic account番号だけを表示し、
  `ログイン中`、`履歴`および各localeで同じ意味の状態語を付けない。現在認証中accountだけ、
  labelの直前にsuccess green `#5DC98A`の`●`を置き、過去accountにはmarkerを置かない。
  選択、partition、logout、Graph/Threadsのaccount表示意味は変更しない。
- 認証済みMainのcomponent順は
  `Header→RemainingQuota→WeekGauge→AccountActivity→ModelUsage→StatusBanner`で固定する。
  残量を最初の主値とし、状態は常時viewport内のStatusBannerだけが所有する。状態を上段の
  duplicate cardへ増やさず、StatusBannerが末尾でもBack/Close/復旧CTAを隠さない。
- Windows Main clientは`900×542 logical`、Linux Mainは従来の`900×498 logical`を維持する。外周は左右`22px`・上下`14px`、内容幅は
  `856px`とする。Windowsの6行の`y/height`はclient座標で順に`14/52`、`74/82`、`164/78`、
  `250/56`、`314/164`、`486/42`とする。Linuxの末尾2行は従来の`314/120`、`442/42`。行間は全て`8px`、末尾余白は`14px`とする。
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
  `22px`のCODEX情報title、`12px`のversionを置く。中央列には切替操作を置かない。
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
  下段はcard内`y=45px`から表示し、日本語・900×542の参照画面ではreset時刻の文字左端が
  約`318px`、observed labelの文字左端が約`445px`、observed時刻の文字右端が約`865px`となる。
  Mainの時刻表記は`PROC-I18N-01`に従い、quota値、期間境界、
  reset/observed epochをUIで再計算しない。
  期間名は受理済みの正の`window_seconds`から作る。86400、3600、60の順に割り切れる最初の単位を使い、整数の「N日周期」「N時間周期」「N分周期」、どれでもなければ「N秒周期」の意味をlocaleへ翻訳する。`monthly`やplan labelから7日・1か月を代入しない。7等幅区分は表示上の分割数であり、各区分が1日であるという意味ではない。時間barの残存割合は`clamp((reset_at-t)/window_seconds,0,1)`で、tはlive accountではUIの現在UTC時刻、historical accountでは下記の最終quota観測時刻とする。残り時間も同じtに対する`max(reset_at-t,0)`を使い、秒未満を切り捨て、日/時/分の商と余りを表示する。quotaを受理できていない初回は割合・期間名・残り時間を未知表示にし、0や7日を仮置きしない。絶対reset/observed時刻は元のepochの表示だけを行う。
- AccountActivityはWindowsのtotal＋model別件数構成を使い、件数は`THREAD-OPEN-362`の公開対象となった現在openのSession thread集合とする。受理済みの現在accountの
  `open_session_thread_count=0`でも同じ`56px` cardに`0件`、5 modelの`0`、`68×30px`の
  `Details`を表示し、Detailsから空のThreads画面へ進める。未受理のcurrentやhistorical
  accountを0件とみなさず、historical accountではlive件数を表示しないが固定cardは保持する。historical accountの保存済みstateが`error`でも、同accountの認証済み記録を正常に取得できた場合は残量期間を最終観測時刻で固定し、記録終了時の取得エラーを明示する。現在の通信エラーやlive countdownへ置換しない。受理済みのthread行はmodel集計の不完全状態だけを理由に隠さない。
  v3のmodel別表示は`動作中（生成済み）`の順とし、SOL 0件動作中/1件openなら`0（1）`、LUNA 1件動作中/2件openなら`1（2）`と表示する。openが0件なら`0`と表示する。総数は公開対象となったopen行の全件数とする。
  同一pairの元行数・動作中行数を検証した後、停止中の親なし行と、停止中の親を持つ子とその子孫を画面から除外した行集合だけでMainの合計とmodel別件数を算出する。動作中の親の子は、動作中も停止中も表示する。wireの元件数を表示件数へ流用しない。
- ModelUsageはLinuxの単一table構成を使い、各modelのInput/Cached input/Outputについてtokenと
  隣接する概算ドルを同じrowに置く。Windowsは`MODEL-USAGE-DISPLAY-01`の版別・利用額順の表を6行×22pxのviewportに表示し、7行以上は表内だけ縦scrollする。headerは固定し、headerと行の右側に共通の`16px`を確保してoverlay scrollbarが金額を覆わないようにする。0件でも`164px`のcardと見出しを保持してemptyを明示する。Linuxは従来の4行×22px・`120px` cardを維持する。
  tokenと金額のbucketの意味は`MODEL-USAGE-DISPLAY-01`を変更しない。
- StatusBannerはWindowsのstate title、detail、該当する単一CTA、最終受信表示の構成を使う。
  Mainの「前回受信」はLinux/Windows各StatusBannerの高さ方向中央に配置する。platform間で他の座標・寸法を一致させない。
  選択accountのID/labelをtitleまたはdetailへ重複表示しない。
- 0%、中間、100%、未取得、警告、危険、APIエラーで同じcard構造を保つ。認証要求では同じ
  Header・6行の割当・末尾Statusを保ち、中央4 data cardの非表示だけを切り替える。
- エラーは既存値を保持するか未取得として明示し、0や100を仮の有効値として表示しない。
- 数値、単位、説明、状態、操作の文字サイズと太さに役割差を付ける。細すぎるフォント、薄すぎる文字、
  余白だけで分断されたカードは採用しない。
- `ui/components.slint`のMain component root styleを変更する場合は、同fileを`include_str!`で参照する
  既存Rust source oracleを逆引きし、変更したcomponentを直接検査する関数の有限集合と実画面gateを
  実行する。実行対象・元のassert・0件でない実件数を保持し、focused selectorまたはpixel gateだけで
  既存source oracleとの整合をverifiedにしない。巨大なMain test assemblyを必要としない自己完結した
  source oracleは、元の関数本文と`#[test]`のbyte同一性および参照先を確認した小さいharnessでも
  実行できる。その結果は当該source oracleの証拠に限定し、Main全体の動作や実描画へ昇格しない。
  関連しないworkspace/all-targets suiteを必須条件にせず、追加の直接consumerが見つかった場合だけ
  実行前に有限集合を更新する（Issue #104の直接影響範囲に従う）。

### 全surfaceの表示と操作の受理（WIN-WINDOW-BOUNDARY-603 / #604）

Main / Setup / Settings / Graph / Threads / Legalの6 Windowを対象とする。実装された各button、toggle、selector、scroll、共通Window操作には、入力後に起きる既存ownerの効果と、有効/無効の条件が必要である。見えるだけで呼出し先がない操作を残さない。

| surface・操作 | 完了として観測する効果 | データ不足・失敗時 |
| --- | --- | --- |
| Mainの推移・設定・thread詳細 | 対応するsingleton Windowが前面で操作可能。thread詳細0件も開ける | 遷移不能を黙殺せず既存の失敗表示。0件の作成で未取得を偽装しない |
| Main再試行・認証操作 | 再試行は本書4.6の待機条件を満たすときだけ既存single-flight取得を1回開始。認証control後はfresh currentを再受理 | listener/health/controlだけでreadyにしない。旧有効値と失敗の表示を分離 |
| Graph mode・metric・系列ON/OFF | 入力を直ちに反映し、完成した対応sceneを一括表示。隠した系列は描画/hoverからも除く | 作成途中はloading。失敗はlast-good。未取得を記録なしにしない |
| Graph日付見出し・period・前後 | 見出しは展開だけ。exact period選択、隣接periodまたは固定幅/月単位移動。端は既存規則で無効 | 部分directoryを使って端を確定しない。選択先の値を前periodの値と混ぜない |
| Threads | 同じmembershipの行・tree・実state/context。Main更新で同じ行集合へ置換 | 独立詳細取得失敗をMain障害にしない。未観測を停止中にしない |
| Settingsのaccount・draft・tabs・Save/Cancel | accountだけ即時の表示対象変更。tabはdraft保持。Saveは永続化成功後にactive化。Cancel/Closeはdraft破棄 | Save失敗は開いたままactive値保持。theme選択/previewは`WIN-THEME-422`の保存済み値へ戻し、他のdraftは保持。account即時選択はCancelで戻さない |
| Settings接続再確認・Setup・Legal入口 | 各既存actionが観測可能な状態更新または対応Windowを前面化 | process起動と接続/データ成功を混同しない。Legalは監視障害で塞がない |
| Setup接続/認証 | prepare→listener→health→必要時control→fresh strict currentの既存順序 | 失敗した段階を保持し、該当する既存CTAを操作可能にする |
| 共通最小化・Close・移動・focus・scroll | OS標準の効果、可視範囲、tab/focus順、隠れた全行への到達 | 操作対象が存在しないときだけ無効。データ取得待ちでWindow全体を凍結しない |

fresh startupで未受理の値は`—`/既存の未取得表現とする。model未取得を確定0行、thread未取得を確定0件、quota未取得を100%として表示しない。WIREが定める初回の有効なcurrent Error応答では、その応答にある既知quota/modelだけを失敗状態・元観測時刻とともに表示し、thread件数は未知とする。成功bundleを受理したことにはせず、正常な空/正常な100%へ読み替えない。既受理世代があれば、同じaccountの失敗は有効な最後の表示と失敗状態を併記する。account境界の消去はPRODUCTに従う。

月カレンダーでは、正の利用記録がない日を控えめな面と文字で表示する。ただし「記録分0」「未計測」「未来日の空欄」を同じ値にしない。未計測日に0を印字して未使用と断定しない。日別値・月合計・期間累計の意味はPRODUCTの差分規則に従い、tooltip/アクセシビリティも同じ正確な数値と既知性を伝える。

### 4.2 Trends / Graph（master: `CUM-138-06`）

- Windowsの単位切替は「ドル・スイッチ・トークン」を余白付きで並べ、両ラベルを常時表示する。Graphを開いたときはトークンを選択し、つまみの位置と選択状態で単位を明示する。二重のボタン枠は置かず、ウィンドウ操作ボタンとの間隔を保つ。pointer、キーボード、UI AutomationのToggle操作で同じ切替を行える。単位変更中はスイッチを先に更新し、完成まで既存の描画を保持する。
- 上部はタイトル／単位切替、表示タイプ・期間情報・前後移動を一つにまとめた行、凡例の3段とする。タイトル、操作行、凡例、グラフの各行間に8 DIPの縦余白を設け、操作行と凡例は同じ背景帯にまとめる。ヘッダー・中央の帯・グラフ枠を接触させず、最小サイズでもグラフの高さを確保する。週期間のselectorと固定時間の日時情報は同じ中央領域を使い、前後移動には方向と短い文字を表示する。ボタン群の角丸・高さ・余白・選択状態を揃え、通常940×640／最小700×480で情報や操作を重ねない。長い期間名は省略表示と全文tooltipを併用する。グラフに被る「時間ごとの累計…」説明文は表示せず、数値軸・単位スイッチ・凡例を維持する。
- ローカル日付の境界は、data rectangleの上に確保した日付軸で「月/日」と下向きの目印を表示する。日付文字をデータ線へ重ねず、画面外の境界や重なる日付ラベルを描かない。既存の現地0時の細線・reset線の意味とDST処理は維持する。
- 滑らかさは既存の単調PCHIPを維持し、描画用の曲線分割を1000px級のplotで1px以下の横刻みへ細分化する。accepted raw、変化点、極値、idle、欠測、補正、reset境界の位置と値は変更しない。欠測を実測線で結ぶ、曲線が観測範囲を越える、別reset間を結ぶことを禁止する。記録済みmodel累計が同値の区間の両端時刻・値を保持し、後続の増加を同値区間へ前倒ししない。task停止metadataだけで未使用帯や水平保持を作らず、実測の変化区間を前後へ移動しない。
- 未使用帯はPRODUCT `G137-5` / `GRAPH-LOCAL-UNUSED-673`のローカルCodexログ消費なしを表す。Remaining未取得／変化・外部Work利用・task状態・全端末coverageから分離し、Dollars/Tokensの両方で同じ600秒以上の区間を表示する。
- Graph専用の系列色は、暗色presetでRemaining `#56B2F5`、LUNA `#E6A23C`、TERRA `#5DC98A`、SOL `#A88CF5`、ASTRA `#EF6A6A`、明色preset（light/paper-light/sand-light/steel-light/lavender-light/mint-light）で順に `#176AAB`、`#985F08`、`#16794B`、`#6A4BCC`、`#B23553` とする。凡例・線・末尾値に同じ役割色を用い、themeのaccent色を系列の代用にしない。全16 presetのplot面とidle面に対して系列は3:1以上の輝度コントラストを保つ。Main/Threads等の共有色キーは変更しない。
- Windowsのidle面はWIN-THEME-422表のGraph背景とidle基準色を各RGB channelで半分ずつ混合し、0.5を切り上げた最終不透明色を使う。classic-darkは `#162232`、teal-darkは `#0F3E49`、lightは `#F2F6FA` となる。旧面より控えめにしつつ背景と区別できる帯として表示し、idleの判定や時間範囲は変えない。上表のidle色および他節の固定idle色はWindowsではこの最終色へ置換する。Linux/Xの色・描画契約は変更しない。
- 週期間・24時間・1週間の各reset期間の下部表示値も選択metricへ連動する。ドルでは保存ドルの整数表示、トークンでは全modelの保存token合計をM単位で表示する。モデル表示ON/OFFに依存せず、raw記録以外の補間／holdを合計しない。100万未満のトークンは桁区切り付き整数とし、100万以上はM表記を用いる。カレンダー・トークン軸・期間値は同じ書式を用い、M値は表示時だけ四捨五入した整数とする。軸の隣接目盛りが丸めで同じ表記になる場合は上側のラベルだけを表示し、目盛り位置と値域は変えない。hoverは桁区切り付きの正確な整数token数を表示し、raw記録の精度を維持する。
- Windowsの表示モードは「週期間」（既定）、「24時間」、「1週間」、「1か月」とする。表示mode群と中央の日時・期間欄の列間に12 DIPを追加し、従来の中央欄marginを維持する。「週期間」は以下の既存reset期間の範囲を維持する。固定時間モードは表示窓だけを独立させ、右端を現在時刻、幅をそれぞれ86,400秒／604,800秒とする。未観測の終端から現在時刻までは空白であり、観測値を延長しない。Linuxの表示・操作は変更しない（Issue #554、#600）。
- 「1か月」は表示timezoneの暦月を曜日7列のカレンダーで表示する。中央見出しは対象年月（yyyy/MM）だけを示し、選択済みモード名と日付範囲を重複表示しない。日曜／週初曜日は表示cultureに従い、月初前・月末後は空欄、未来日は日付のみとする。前後移動は暦月単位で未来月へ進めない。各日には全モデル（未知modelを含む）の合計使用量を一つ表示し、モデル別棒・曲線・軸・凡例は表示しない。ドル／トークン切替を日別合計へ連動し、ドルは小数2桁、トークンは日ごとの使用量を一目で比較できるよう、100万以上を四捨五入した整数のM単位（1 M = 1,000,000 tokens）、100万未満を桁区切り整数とする。日別値の文字サイズを維持し、通常幅と最小幅で量と単位を省略せずに表示する。正確なトークン数はtooltipとアクセシビリティ名に桁区切り整数で残し、保存値・集計値は丸めない。モデル表示ON/OFFは日別合計へ影響しない。日別値の採否は`G137-GRAPH-01`を維持し、記録分／未計測を区別、価格不明のモデルがある日の既知部分だけを全額として表示しない。今日を枠で示し、現在／過去の未計測日は「—」、未来日は値を出さない。正の利用記録がない日は背景と文字を控えめにし、利用記録のある日を優先して読めるようにする。記録分0と未計測はそれぞれの値・状態を残し、部分記録や未計測を一日未使用の確証へ読み替えない。最小幅ではセルを7列のまま保ち、縦方向へscroll可能とする。
- 固定時間モードの「＜」「＞」は窓の全幅だけ移動する。過去へ移動した右端は更新中も固定し、24時間／1週間間の変更でも維持する。現在へ到達すると現在追従へ戻り、未来へは進めない。週期間モードの「＜」「＞」は、後述のselectorと同じ全page受理済みselectable表示順を平坦化した一つの配列を使う。「＜」は選択index+1、「＞」はindex-1を選ぶ。先頭では「＞」、末尾では「＜」を無効にし、空または単件では両方を無効にする。取得directoryだけのquota/gap-only過去IDへ移動せず、固定日数を加減しない。期間の取得・失敗時保持・選択IDの意味をselectorと共有する。
- 表示窓に重なる全reset期間を同じaccount・published pairから完全に受理する。週期間・24時間・1週間では各期間をPRODUCTの累計・残量・欠測規則で個別投影してから表示窓に切り出し、期間をまたぐ線・idle帯・累計の足算を作らない。月表示では同じ受理済みraw履歴を日別投影に使う。数値軸と時刻軸は本節「数値軸・時刻軸の値と座標」に従う。空の時間窓でも選択した期間の軸を保つ。
- 週期間・24時間・1週間では表示タイムゾーンの日付が変わる0時に、幅0.5px・不透明度30%の白い細線を引く。公開された各期間の開始時刻`start_at`が表示窓内にある場合、その位置に幅1px・不透明度70%の黄色系テーマ解決色（`#E6B85C`）の区切り線をグラフ内だけに描く。期間識別に使う`reset_at`や予定リセット日時を実際の切替位置として描かない。区切り時刻をサンプルの増減や欠測、表示窓の左端から推定せず、同じ開始時刻は1本にまとめ、日付境界と重なった場合は期間区切り線を優先する。夏時間のある地域も現地暦の日付境界を用い、86,400秒ごとの線で代用しない。上部は本節の3段構成とし、空の固定高さ行を置かない。940×640と最小700×480で操作とplotが重ならない。
- 更新では受理済みの画面・軸・日時を保持し、全候補の準備後にまとめて切り替える。失敗と遅延応答によって部分的な期間や古い表示窓を公開しない。account変更では別accountの画面を保持しない。期間ごとの描画用geometryを再利用し、固定窓では範囲内と境界の描画点だけを渡す。元履歴を間引き・書換えせず、滑らかさのために観測値を変更しない。
- 期間、ドル/トークン、Remaining/LUNA/TERRA/SOL/ASTRAの操作を上部固定帯に置く。
  model名と累積値は同じaccepted v3 rootから取得し、ASTRAを「その他」へ集約しない。
- 期間リストはpointer pressの1回で展開し、metricスイッチは1回の操作で切替し、REST/DB/poll完了を待たずにuser-visible acknowledgementを返す。7日1分bucket由来の10,080点と互換31日窓由来の44,640点は対象データ規模であって、通常データを拒否する任意の上限ではない。通常処理時間の絶対msは保証せず、入力への即時acknowledgement、UI thread外処理、既存deadlineでの失敗隔離を合否条件とする。根拠のないcold maxは追加しない。
- 系列ON/OFFはpointer pressで状態とボタン面を先に更新し、完成sceneの描画・hover対象をその状態へ一致させる。対象系列の描画要素がある場合は、OFFでその要素が消え、ONで採用済みの同じ値・線種・境界の要素が戻る。対象系列に採用点も描画要素もない場合はplotのpixel差を要求せず、選択面と可視状態の変更を確認する。表示差を作るために値・線・帯を追加しない。この選択状態は以後のscene更新でも保持する。物理入力、UI状態、実描画/hoverへの反映を別経路で確認する。
- 期間リストは選択専用resourceの公開start_atを表示タイムゾーンの日付でまとめ、公開されたexact IDを個別に選べる。現期間は空でも残し、過去期間は範囲内の公開modelに正のtotal_tokensがある場合に掲載する。不完全・legacy-unknownの正tokenも残す。PRODUCTの未解消の旧利用記録を持つperiodも、serverが掲載した選択肢をそのまま残す。clientが掲載後の不明model値を見て再filterしない。選択後に確認できる数値がない場合は未取得と原本不足の表示を維持し、0・未使用・100%を作らない。この保持対象を除き、初めからquotaのみ・欠測区間のみ・正のmodel記録がない過去期間は通常一覧に載せないが、未使用とは判定しない。選択とpair再整列は同じ選択用resourceを使い、日・週・月の時間窓は従来の取得directoryでquota/欠測区間を取得する。取得cacheはresourceとaccountで分離し、選択resourceのexact404だけ旧directoryへ戻る。その他の失敗は既存の失敗表示とlast-good保持を維持する。見出しの展開では期間を変更せず、同日の期間を統合しない。ディレクトリ更新でも展開操作を保持し、選択は次項の明示的な追従規則で決める。31日より古い保存期間も同じ操作で選べる（#646）。
- selectorの表示順はwireの配列順から独立させる。各periodのstart_atをactive表示timezoneのlocal dateへ変換してgroup化し、groupは「currentを含む」優先、残りはlocal date降順とする。group内はcurrent優先、その後はstart_at、reset_at、IDの順で全て降順とし、IDの比較はUTF-8 unsigned byte辞書順とする。これを平坦化した同じ配列を選択indexと前後移動の唯一の順序とし、popup側で別にsortしない。公式resetが後方へ動いてもcurrentは先頭であり、全page受理済みの掲載selectable IDへ隣接移動できる。取得directoryだけのIDは前項の通り移動対象に含めない。この順序は表示用であり、providerの取得順や実際の期間の新旧を推定する規則ではない。日付・時刻・raw IDは変更しない。
- 選択intentは「current追従」と「過去ID固定」の二つとする。初回と、利用者がcurrentを選んだ場合はcurrent追従であり、一覧更新ごとに受理済み`current=true`のIDを選ぶ。currentがない完全な一覧では表示順の先頭、空一覧では選択なしとし、追従intentは保持する。利用者が過去periodを選んだ場合（前後移動を含む）はそのexact ID固定とし、更新後もIDがあれば保持する。固定IDが除外された場合はcurrent追従へ戻り、同じ初回規則で選ぶ。timezone・locale・metric変更、wire版変更、page分割だけでは選択intentと残存IDを変更しない。timezone変更ではgroupと順序だけを再計算し、架空のcurrentや近接resetの代替IDを作らない。
- 期間リストとmetricスイッチはユーザーの選択で状態を変え、pollやlocale通知による同値候補の再公開では選択へ読み替えない。開いているリストを自動で閉じない。
- 開いているpopupの未確定の強調候補は確定済み選択とは別に持ち、periodはexact ID、日付見出しは表示timezoneのlocal dateで識別する。
  完全な一覧更新で順序が変わっても、残る同じ候補を強調し、行番号を選択identityへ使わない。強調periodが消えた場合は、
  更新後の要求選択IDが残ればそれ、なければ表示順の最初のperiodを強調する。periodが0件なら強調なしとし、Enterで選択を作らない。
  強調見出しだけが消えた場合も同じperiodへの戻り先を使う。強調periodを表示するために必要なgroupだけを展開し、その他の残るgroupの展開状態を維持する。
  利用者が強調periodを含むgroupを折り畳んだ場合は、その見出しを強調し、Enterは再展開だけを行う。
  強調の移動・一覧更新だけではperiodを確定しない。Enterはその時点の強調対象にだけ作用し、取得失敗では前回の完全一覧と強調を保持する。
- 期間変更は`idle → loading → ready|confirmed-empty|failed`の有限状態遷移とする。選択表示は入力直後に更新し、
  accepted same-pair history page集合のparseとpresentation projectionはUI thread外で行う。SQLite再読込やsampleの
  canonicalization/merge/recalculationは行わない。既存の遅延残量補間と終端保持はpresentation-onlyで行い、
  導出点をdetailsやDBへ書き戻さない。処理が次paintまでに終わらない場合は操作を塞がない
  indeterminate progressと「期間データを読み込み中…」を表示し、`記録なし`や`利用不可`を先に表示しない。
  same-pairの全pageが0件と確定した場合だけ`confirmed-empty`へ進む。loading中は直前に完成したgraph・
  metric・軸を保持し、候補完成時だけ1回のUI publishで全てを同時に差し替える。
  入力中の要求ID/mode/metricと受理済みsceneはPRODUCT `G137-10`に従って分離する。
  旧graphを保持している間はその期間・単位を旧dataのまま示し、要求先と
  「直前のグラフを表示中」をloading/error表示へ含める。新要求の値を既に表示したように見せない。
- 期間を連続選択した場合は旧候補をcancelし、最新revisionだけをpublishする。失敗・timeout・cancelを
  空graphや部分graphへ変換せず、直前graphを保持してbounded errorを表示する。キャッシュ済みで次paint
  までに切替できる場合はprogressを点滅させない。入力への反応と期間データ完成は別条件として確認する。
- 週期間モードのcurrent期間の`start_at/end_at`は、COREが最新quotaの公式windowとPRODUCT `CURRENT-PERIOD-BOUNDS-129`の同periodデータ観測終端から投影したmetadataを唯一のauthorityとする。quota原時刻だけで後続model点を切り捨てたり、clientの現在時刻で終端を延ばしたりしない。
  期間欄、selected period start、plotの横軸左端・右端はaccepted periods resourceの
  同じpairをそのまま使い、Windowsで再計算しない。completed/historical期間もPRODUCTから投影・公開されたpairを使う。quotaの
  `reset_at`はリセット時刻表示の別項目であり、Mainのリセットまでの区間は公式originとresetを使う。current periodの
  `end_at`は同じatomic published rootのaccepted観測終端であり、UI取得後のlocal現在時刻へ延ばさない。
  completed periodは保存された固定`end_at`までを右端とする。
- 週期間モードの期間欄、横軸、折れ線、ホバー値は同じselected exact reset IDだけから一括投影する。60秒以内でも異なる公式IDを同一期間として扱わず、欄だけ旧期間・plotだけ現在期間の混在を禁止する。旧wire sampleの所属互換だけはWIREのexact一致優先規則に従い、selectorのID同一性へ流用しない。
- 期間更新時は上記の選択intentを使う。Mainが受理したquotaのreset/window変更は期間取得を促す通知として扱い、canonical IDは
  same-pairの期間ディレクトリから決める。同じquota pairの観測更新だけでは追加取得しない。通知以前の遅延応答は
  revisionで棄却し、選択やgraphを古い期間へ戻さない。
- 期間起点は時刻の証拠であり、値の証拠ではない。現期間・過去期間とも最初の実測以前へ0 token/100%の補助線を作らない。公開`origin_at`を確認できる場合は横軸起点として使い、最初の観測とその時刻を維持する。先頭の値が欠けている場合は未記録区間として示す。原本復元で回復した観測は通常の実測として表示する。raw/API/hoverに架空の起点を追加しない。
- Windowsグラフの角丸枠は内容の描画に覆われず全周を連続して表示する。内容のclipと枠線の描画順を分離し、枠線はplotのhoverや入力を妨げない。通常・最小サイズとresize時にも角を保持する。
- Windowsでは右端の常時数値ラベル・leader・専用gutterを置かず、その横幅をplotへ割り当てる。線端の切れを避ける通常の描画余白と軸ラベルの余白は、数値表示の専用領域とは分離する。横方向のresize差分はplotへ割り当て、縦軸の意味と期間の端点は変えない。Linuxの右端ラベルは従来どおり維持する。
- Windowsのplot内でマウスを動かすと、近い一つの実観測時刻の日時と表示中の全系列をまとめたツールチップを出す。日時は表示timezone、Tokensは元の整数精度、Dollarsはその観測点のaccepted保存額、Remainingはraw観測値を用い、単位を明示する。欠けた系列は`—`とし、補間・hold・synthetic tailを実観測値として出さない。viewport内の同じreset期間だけを探索し、confirmed gapや非所有区間を越えて別時刻の値を拾わない。非表示系列は除外し、pointer離脱・scene/metric/表示系列の変更・リサイズ時に旧ツールチップを残さない。観測索引はscene生成時に保持し、pointer移動で全履歴の再投影や全件走査を行わない。
  pointerのxをsceneの同じ横軸でUTC秒tへ逆変換し、そのtを含むperiod表示範囲・連続所有区間内で候補を求める。所有metadata欠落・空配列・開始nullにより所有が未確認の範囲は、PRODUCT G137の未確認spanとして扱う。その同じ未確認区間内の保存済み点も候補にできるが、tooltipは選んだ点の元日時/値を示すだけで、pointer時刻の観測や所有を証明しない。候補は表示中のmetric/Remainingの実観測値が一つ以上ある元timestampで、pointer位置と候補の間に当該表示系列の未回復gapまたは明示非所有区間がある場合は候補にしない。exactな実観測時刻tそのものはgap内でもその元値を表示できる。距離`abs(timestamp-t)`が最小の候補を選び、同距離なら早いUTC秒、同じUTC秒の別periodならexact IDのUTF-8 unsigned byte昇順を使う。週期間modeは選択IDだけを対象とする。複数periodが重なる固定時間modeでも一つのperiodの同じ観測rowだけを使い、他periodのmodelや別時刻のRemainingを同じtooltipへ継ぎ足さない。候補0件、全系列OFF、plot外ではtooltipを出さない。
- Remainingは独立0–100%意味、モデル系列は累積値として扱う。残量をドル軸へ誤って合わせない。
- Graph の線表示上部に、表示範囲内の最初の実測日時・残量、破線の未確認・欠測の意味を常時読める折返し文で示す（`Graph.HistoryEvidence`）。非所有区間がある場合は最長区間の開始・終了と「このアカウントを記録していない／区間内利用量不明」を示す。所有metadataが不足する範囲は「アカウントの記録区間が未確認／区間内利用量不明」と区別し、欠落・空配列を所有済みとして非表示にしない。いずれも未使用の意味にせず、保存済みの表示可能な点は保持する。残量から使用者・model 使用量を特定せず、範囲外の記録まで不明と断定しない。回復した quota-only 点は数値を表示するが、未取得 model を0や確認済みにしない（`HISTORY-RECOVERY-614`）。
- 完全集計から省略された未使用familyは、G137-2の同一reset・既知0・過去正値なし条件を満たす場合だけ、表示合計0を実線とhoverで示す。
  完全な空集合と取得不能を区別し、既知0 memberの省略を実際のモデル版の利用開始後も欠測扱いにしない。未取得・既知0でないmemberの欠落・正の累計の消失は従来の欠測表示を維持する。rawモデル行、集計額、idleの証拠へ表示0を追加しない。
- Remainingとモデル使用量は別の観測値であり、モデル使用後に遅れて届いた最初の低い残量観測はその観測時刻へ反映する。残量観測が存在しない区間を料金・tokenから逆算してはならず、未観測区間を正常な残量低下として表示しない。
- `reconstructed-from-session`、`unknown`、`unavailable`ではモデル数値を表示せず、
  model key/sourceと欠損metadataだけを表示する。`confirmed`の部分集合には確認できたmodelの数値を表示し、`models_complete=false`だけを理由に消さない。`legacy-unknown`は保存済みの同じmodel keyの値だけ表示でき、確定した集計・予測には使わない。月カレンダーだけは`G137-GRAPH-01`の日別配賦条件を満たす保存値を「記録分」へ表示上加算し、model集合の完備や確定した日合計へ昇格しない。局所未使用への利用資格はPRODUCT `G137-5`を参照し、Remainingの既知性や全端末の完備性へ結合しない。
  直接観測(`confirmed`)または保存済みlossless `legacy-unknown`の値を通常実線とし、補間・hold・smoothing・予測はUI presentation-onlyでAPI/DBへ書き戻さない。
  これらの破線はDBまたは取得記録の欠損・異常を示すNG表示であり、正常な実線の代替ではない。
  最終実測と公開終端が同じcanonical minuteならPRODUCTの終端規則に従い実測点で止める。後続minuteのmodel欠損を、current期間の末尾という理由だけで「集計待ち」と推測して隠さない。公開された時間軸と原観測、欠損・gap・棄却理由を保持する。
- グラフでは`gpt-6-luna`をLUNAに集約する。旧family集計欄の0から同じfamilyの正確なmodel IDの保存実測へ表現が切り替わっても、その名前の違いを未取得と判定しない。実測線とhoverは同じfamilyの記録値を示し、本当に未確定の部分だけを破線にする。元model keyと保存値は変更しない。
- model系列の有限表示状態は次を正本とし、model名ごとの全直積には展開しない。

  | 入力状態 | 表示契約 |
  | --- | --- |
  | 同一periodの実測累積が増加／不変 | 増加と未使用確定でない不変は3px実線。同値区間の両端時刻・値を水平線で保持し、増加する隣接観測間だけを単調PCHIPで滑らかにつなぐ。確定idleだけはbandと同じX範囲の1px水平実線へ分離する |
  | 当該model値はログ実測、全model集合は不完全 | 直接観測(`confirmed`)の値だけを保持する。未掲載modelは欠損metadataだけとし、数値を作らない |
  | 他modelの出現／消失、または`confirmed`と`legacy-unknown`の切替 | `confirmed`の同じmodel keyは通常線を維持する。`legacy-unknown`は保存済み同じkeyの値だけを表示し、集計・予測には使わず、同一runが`G137-5`の限定条件を満たす場合だけidle判定に利用できる。新規modelは最初の直接観測時刻から開始し、消失modelのholdは表示専用とする |
  | 正常な直接観測点のtimestampだけが疎 | timestamp差だけでは欠損化せず、同値・増加とも3px実線で結ぶ。明示的なunavailable、confirmed gap、異常とは分離する（`G137-4`,`G137-7`） |
  | model値自体が未取得、または確認済みrecorder停止区間 | model key/sourceと欠損metadataだけを表示し、数値を0補完しない。必要な破線bridge／holdは表示専用で、API/DB、集計、idle判定へ反映しない。`Remaining`からmodel値やtailを外挿しない |
  | 累積が後退し、その後に回復 | source completenessを問わず最後のaccepted値を下回るrawを表示値へ採用せず、当該modelだけを回復点まで細い破線hold／bridgeとする（`G137-3`） |
  | period内の最初の既知点 | 公式起点を確認できない場合は0からの斜線を捏造せず、その値・時刻から開始する。公式起点を確認できても値は生成せず、先頭未記録区間の扱いは`G137-6`に従う |
  | 旧wireのreset alias／正式reset境界 | 旧sampleの所属受理はWIREのexact一致優先・一意候補の互換規則に従う。時刻の近さでperiod IDを統合しない。公式の別resetは別periodとして保持 |
  | 確認済み0／model行なし | 前者だけ0として描き、後者は未知として数値・線を作らない |
  | ドル／token切替 | 同じ観測時刻列を使い、単位と値だけを切り替える。各metric自身のaccepted観測を表示し、token同値を理由に保存USDの変化・欠測を補正しない。USDはidle authorityにしない |

  アイドル帯の採否はPRODUCT `G137-5`を唯一の定義とする。任意local-usage-v1の正常な空索引を含む未使用区間、または未対応serverのlossless raw比較を使う。具体的な時刻付き未処理数値と索引読取り失敗は未確定、offset-only pendingは全窓を遮断しない。所有境界は保持する。
  拡張に基づくTokensの局所未使用区間は、既知token anchorから1px水平実線を描き、quotaやmodel sampling欠落だけで予測破線へ戻さない。元row列・hover・raw数値は変えない。旧serverの限定1 sampling slotは`G137-4`の資格を維持する。USDとRemainingは自身の観測・異常規則で独立に描き、帯のために未知値を作らない。
  10分は画面幅に依存しない意味閾値とし、pixel幅filter、最小表示幅、gridまたはsegment境界によって削除・周期分断しない。
  Remainingは`G137-6`に従い、accepted raw値を元時刻・元値の証拠として保持し、token増分またはtask lifecycleでraw値を
  移動しない。表示geometryも同値反復を含む全accepted観測点を通過し、横ばい区間と変化時刻を保つ。隣接観測間だけを
  各隣接区間の増加・減少・不変を保つPCHIPの3px実線でつなぎ、実測の横ばいや反転を変更しない。確定idleだけはbandと同じX範囲のexactな1px水平実線として分離する。raw-null、unavailable、confirmed gap、異常、
  terminal holdだけを1px破線の予測とし、導出値をanchorへ昇格させずAPI/DBへ書き戻さない。
  `Remaining`のeffective値からmodel系列の値またはそのperiod tailを外挿しない。
  raw-null補間、gap、異常、終端hold等の欠測・予測の破線は
  X版では1px、Windows版では1px相当とする。通常のmodel／Remaining実測は3px、確定idle実線は1px、欠測・予測破線は1pxとし、solid/dashedで意味を区別する。
  破線は幅の広いplotでも切替点が判別できる短く密な周期とし、長い線片・隙間で通常線に見せない。

  plotの描画layerは`background/grid → idle band → boundary guides → series/labels`とする。idle bandはX版で最終合成色`#1A2838`、Windowsでは本節のGraph専用最終色を
  opacity 1でplot全高へ置き、gridを透過させない。既知idle band内のseriesとedgeを避けた同一Yで、major gridの
  X pixelと左右のnon-grid X pixelがいずれもそのplatform/themeの最終idle色となり、background色またはgrid色の縦columnがbandを
  分断しないことをX screenshotとWindows rasterで検査する。band geometryは時間intervalだけから決め、画面幅や
  pixel幅を理由にintervalを除外しない。Releaseのperiod geometry oracleは、不透明bandに隠れたgridを可視gridとして
  要求してはならない。2点以上の可視gridと、隠れた全grid位置で上下20pxを除く走査高の90%以上を占めるidle色
  （後描画seriesの交差だけを許容）から5点の等間隔gridが一意に定まる場合だけperiod境界を復元し、部分高のidle色、
  idle色のない疎grid、または複数のgrid解はfail-closedで拒否する。

  既知の不完全ASTRAが途中まで増加した後に確定値へ移る場合、左側の増加を消して最初の確定値だけを
  水平表示してはならない。開始・中間・終端値、線種、period ID、値表示（Linuxは右端ラベル、Windowsはホバー）を一つの表示candidateとして
  検証し、一項目でも不一致ならその表示を受入れない。
  familyの表示・hoverは`G137-7`の時点別membershipを使い、後から追加された版名によってそれ以前の保存値を
  消さない。一度掲載されたmemberの欠測・価格不明は0へ補完せず、rawの完全性やidle authorityも昇格させない。
  週期間・24時間・1週間の実reset期間の合計は`G137-7`に従い、選択中のドル／トークンに連動する。グラフ/x軸の下の細い値の欄へ、
  各periodの可視区間中央に、控えめで読みやすいテーマ色の値だけを表示する（例: `$75`、`125 M`、`850,000`）。
  USDは表示時だけ四捨五入した整数、トークンは100万以上をM表記、100万未満を桁区切り整数とし、説明文・開始日時は付けない。値がなければ「—」とし、確定0と区別する。
  左端で途中から始まる前periodも全期間の利用相当額を、そのperiodの可視区間中央へ表示し、線はviewport左端で切り出す。短い期間では中央を維持し、その区間の幅と左右余白に合わせて文字を縮小する。期間区切りは金額と同じ黄色系のテーマ色でplot内だけに描き、金額欄へ縦線を延長しない。日区切りの細い白系線は維持する。
  通常の系列hoverは維持し、金額専用tooltip、説明、警告を追加しない。個別modelの右端値やleaderは再導入しない。
- X版とWindows版は`G137-1`..`G137-10`を参照する同一の履歴fixtureと固定期待値（period/pair、
  累積model、raw/effective Remaining、gap、metric別anomaly、partial/unavailable、未使用区間、期間末）を通過しなければならない。
  片方の描画ヘルパーが生成した値をもう片方の期待値には使用せず、fixtureのliteral oracleを独立に使う。
- finite oracleは、shared rolloverのperiod A→B `100% / $1 → 41% / $323.674247`、
  `graph_delayed_quota`のfirst observation・遅延quota・missing quota、model別回帰/回復、
  confirmed gap、raw-null quota、current/historical右端、no-historyの9 causal caseとする。
  ASTRAのtoken/指定4単価、旧3モデルだけが既知のincomplete period、period選択後pollのexact reset ID維持は
  今回観測した同じ到達経路へ統合し、別の全直積を作らない。
  X/Windowsは同じfixtureの固定期待値を独立に検査し、値形状による100%・7日・quota-only除外、
  platform helperから期待値を生成する循環oracle、新workflow gate、全test/all-suite/全直積を追加しない。
- 操作帯を開閉してもplotの位置・高さを変えず、軸ラベルや値を確認する操作を隠さない。
- 記録なし、欠測、アイドル、活動、0/中間/100を明示的な設計状態として扱う。

#### 数値軸・時刻軸の値と座標

月カレンダーを除くGraphで、左軸は選択metricのmodel累積（TokensまたはUSD）、右軸はRemainingの百分率を表す。RemainingをtokenやUSDへ換算せず、左軸の目盛りをRemainingの量として読ませない。Remainingを表示するときは右の百分率目盛りを表示し、RemainingをOFFにしたときだけ右の目盛文字を隠す。系列OFFによる線の非表示だけでは左右の数値domainを変えない。右端のmodel値labelやleaderを置かない規則は、百分率軸を隠す規則ではない。これらはLinuxの対応Graphにも同じ数量の意味で適用し、platformごとのfont・色・window寸法を統一する意味ではない。

1. model側の最大値Vは、PRODUCTのnumeric採否とfamily表示membershipを完了した全model系列を表示窓へ切り出し、その窓内のaccepted anchorと描画される曲線・欠測bridge/holdの値の最大から求める。系列OFFも同じpotential系列を計算対象に残す。別periodの最大値を足し合わせず、窓外の値はその窓へ入る境界geometryを決めるためにだけ使う。表示曲線は下節の形状保持補間なので、窓内の端点と切出し境界で最大を求められる。値がない場合はV=0とするが、これは観測値0を作ることではない。描画domainの上端はM=max(1,V)、下端は0とする。最低spanの1は、全0・空データ・1 USD未満でも有限の軸を構成するための製品表示値であり、観測点・予算・公式上限ではない。
2. Remaining側のdomainは常に0..100%である。modelのM・選択metric・model欠測から変更せず、quotaからmodelのMを逆算しない。plot内の上から下への正規化座標をyとすると、model値vは`y=0.99-0.98*(v/M)`、Remaining値pは`y=0.99-0.98*(p/100)`へ対応する。上下1%は線の端を枠に重ねないための表示余白で、負の利用量や100%超のquotaではない。rawの採否と正確な整数計算を終えてから座標の近似計算へ移り、線を収めるためrawを丸め・clampしない。
3. major目盛りはi=0..4について、左が`M*i/4`、右が`25*i%`であり、上式による同じyに置く。下端の0はy=0.99、上端のM/100%はy=0.01の目盛りとして表示する。これらを余白外端のy=1/0へ移さず、ラベルの値と位置を一致させる。左のUSDは`$`と小数2桁、Tokensは本節のM/桁区切り整数書式を使い、表示時だけPRODUCTの丸め規則を適用する。右は0/25/50/75/100に`%`を付ける。丸めによる重複ラベルは本節の上側優先規則で省略し、目盛りの位置・M・hover値は変えない。空/全0でも軸を描けるが、数値が未取得のmodelの線・点・利用量0を追加しない。
4. 横軸の範囲は同じ受理済みsceneのS=start_at、E=end_atとする。S<Eなら`S+floor((E-S)*i/4)`（i=0..4）の整数UTC秒を候補とし、同じUTC秒の候補は一つにする。目盛りと観測のxは`(t-S)/(E-S)`へ対応し、raw時刻を等間隔bucketへ移さない。S=EならそのUTC秒を中央の一目盛りとし、実測点も中央へ置き、長さのある線や架空の終端を生成しない。固定時間modeの空窓も受理したS/Eを保持する。
5. 横軸目盛り・日付境界・hoverの時刻文字列はI18N `PROC-I18N-01`に従う。異なるUTC秒の目盛りが同じ表示文字になる場合は年・秒の精度またはoffsetで区別し、時刻をずらして文字だけを異ならせない。ラベル候補は元のiが0、4、2、1、3の優先順で置き、先に置いた文字の実測boundsと重なる候補の文字だけを省略する。重複UTC秒で除いたiは飛ばす。S=Eでは中央の一文字列だけを使う。省略によって軸domain・目盛り時刻・データgeometryは変えない。

独立した数量例としてM=200では、左`0/50/100/150/200`と右`0/25/50/75/100%`は、下から`y=0.99/0.745/0.5/0.255/0.01`に対応する。左100と右50%が同じ高さであることは、50%が100 tokenや100 USDであることを意味しない。V=0でも未知modelの点は0件のままである。S=100,E=102の時刻候補は`100/101/102`、S=E=100は中央の100だけとなる。これらの表示値をDB/APIへ保存しない。

#### PCHIPの値と端点

本節のPCHIPは、[形状保持補間の導関数定義](https://docs.scipy.org/doc/scipy/reference/generated/scipy.interpolate.PchipInterpolator.html#notes)と[端点の片側推定式](https://github.com/scipy/scipy/blob/v1.18.0/scipy/interpolate/_cubic.py)を次の式で固定する。特定libraryの導入やその将来版への自動追従を要求する意味ではない。PRODUCTの同じperiod/metricの連続実測runを入力とし、reset、correction、明示欠測、非所有、未回復gapを越えて接線を共有しない。欠測bridgeはその既知両端だけを独立した2点入力とする。確定idleの水平部は式を変えず線幅だけ分離する。

- 入力を時刻昇順の`(x_i,y_i)`、`h_i=x_(i+1)-x_i>0`、区間傾きを`d_i=(y_(i+1)-y_i)/h_i`とする。2点の場合は両端の導関数を`d_0`とし、線形になる。1点に補間区間はない。
- 内点の導関数`m_i`は、隣の傾きのどちらかが0、または符号が異なる場合は0。それ以外は`w1=2*h_i+h_(i-1)`、`w2=h_i+2*h_(i-1)`として、`m_i=(w1+w2)/(w1/d_(i-1)+w2/d_i)`とする。
- 3点以上の左端は`m_0=((2*h_0+h_1)*d_0-h_0*d_1)/(h_0+h_1)`を候補とする。`m_0`と`d_0`の符号が違えば0。そうでなく`d_0`と`d_1`の符号が違い、`abs(m_0)>3*abs(d_0)`なら`3*d_0`とする。右端は時系列を反転した同じ片側規則を使う。
- 区間内の`u=(x-x_i)/h_i`に対して、値は`(2*u^3-3*u^2+1)*y_i+(u^3-2*u^2+u)*h_i*m_i+(-2*u^3+3*u^2)*y_(i+1)+(u^3-u^2)*h_i*m_(i+1)`とする。両端の元値を通過し、値をrun外へ外挿しない。rawの採否・同値・整数差分はこの近似座標計算に入る前に完了する。
- 独立した数値例は`x=[0,1,2],y=[0,1,1]`の最初の区間の`u=0.25/0.5/0.75`が`0.3671875/0.6875/0.9140625`、`x=[0,1,3],y=[0,1,2]`の導関数が`[7/6,9/13,1/6]`である。反転`y=[90,95,89]`の内点導関数は0であり、95を棄却しない。線をpixelへ分割する密度は実装詳細だが、この関数・anchor・線種・境界を変更してはならない。

月の日額に価格不明の寄与がある場合はPRODUCTの日別配賦規則を使い、Dollarsセルは「—」、tooltipとアクセシビリティは「金額不明」とする。既知部分の額だけを日額の位置へ出さない。Tokensの既知記録分と、無正値日の控えめな配色は独立に保持する。

### 4.3 Threads

- Linux/Windows共通のWindows masterは900×480 logical client、viewport 384px、96pxの固定行4件とする。
  見出しの直下で一覧を`y=56px`から上詰めにし、件数行と詳細データ鮮度行は表示しない。
  画面中央への自動配置を行わない。5件目以降だけ同じ一覧内で縦scrollを許可し、画面全体はscrollしない。
- Threadsの空状態は選択accountのcurrent/thread bundleを受理した場合だけ表示する。未受理は未取得とし、
  Mainまたは独立Threads取得の失敗を0件へ変換しない。失敗時だけ一覧の上にエラーを表示し、
  同じaccountの受理済み行は保持する。受理済み0件に対する独立取得を追加せず、回復時にエラーを解除する。
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
  初回受理行と同一pairの独立したThreads再取得行のどちらでも、停止中の親なし行と、停止中の親を持つ子とその子孫を除外する。動作中の親の子は、動作中も停止中も表示する。残る子は元の`parent_thread_id`を持つ親にだけ接続し、同じ表示名の別の子と混同しない。
  動作中の状態文字はdanger赤`#EF6A6A`、停止中と未観測は従来のsecondary文字色とする。親card背景とmodel accent色は変更しない。
- 動作中のrootを未観測のrootより先に表示し、同じ状態のrootと兄弟の既存順を維持する。各rootの直後に公開対象の子孫をdepth-firstで連続表示し、子だけを親から切り離して並べ替えない。
- contextは同じthreadの観測済みusage/window pairだけを表示し、usage=0は有効な0%として表示する。
  pairがない、windowが0以下、または旧checkpointのNULLは`未観測`とし、累積値や別sourceから推測・合成しない。文言は選択localeに従い、英語では`Context Unobserved`、日本語では`コンテキスト 未観測`と表示する。
  割合は整数比をround-half-upで小数点以下最大2桁へ丸め、末尾0を除去し、100%を上限とする。次の観測で得たpairは
  checkpointへ保存し、後続append/restartで未観測へ戻さない。
- stale Session、orphan、cycle、部分snapshotは誤って現在openとして表示しない。親が動作中なら、現在openの停止済みchildも停止中として表示する。
- 一覧件数が増えても本文fontを縮小せず、Window拡大や空疎なcardで情報密度を下げない。contextを埋めるための
  full old-history reread、追加poll、backfillを行わない。

### 4.4 Setup / Settings

- Settingsのaccount selectorは既存の表示対象を即時に切り替え、認証accountを変更しない。
  起動と選択維持は`ACCOUNT-LIFECYCLE-134`に従い、開いているSettingsも同じ一覧・選択へ追従する。
  account選択は保存操作を要さず、取消またはCloseで戻さず、永続設定へ追加しない。
  Linuxのselectorは`250×44px`とし、閉じたselector内のaccount labelとIDを左`28px`に揃え、
  矢印は幅全体の右端から`24px`の`14px`欄の中央へ置く。
- Linux Settingsはaccount selectorとtimezoneを持つsingleton Windowとする。timezoneの選択肢と保存値はexact
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
- WSL/remote/one-session raw recovery、ArgumentListと保存しない資格情報は本節とSECURITYのWindows bootstrap境界を正本とする。
  認証の順序は次段落、失敗時の操作は4.6、single supervisor/tunnel/reapは4.6末尾に従う。

Setupの順序はserver/API prepare→listener→readiness `GET /health`→strict `GET /v4/current`（exact404だけv3 current、さらにexact404の旧serviceだけv3 details→v2→v1）→
必要時だけauth-start→別auth-check→新しいstrict currentで固定する。auth-start/auth-checkはcontrol-onlyであり、
応答を表示rootへmergeしない。healthだけ、またはcontrol成功だけでdata readyとしない。

RC-121のprofile別action意味論も固定する。WSLのserver prepare/service start、Remoteのinstall/tunnel/raw
tunnelはそれぞれ独立したvisible+enabled Tab step/UIA actionであり、`action.StartForward`へ丸めない。
`action.StartForward`はforwardingだけを表し、SetupOperationGeneration・busy・stale completionは現行世代だけを
受理し、古い完了はcommitしない。

SetupのBackは、現在stepより前のstepがあり、Setup操作がin-flightでも取消し後の終了待ちでもない場合だけ有効とする。
有効なBackは前stepへ一つ戻り、旧SetupOperationGenerationを失効させる。busy中のBackは無効である。
Closeはbusy中も有効とし、popupのないEscapeは有効なBackがあればBack、なければCloseと同じ動作をする。
CloseはそのSetup操作を取消して世代を失効させ、遅れた結果で設定保存、次step、Window再表示、focus移動を行わない。
その操作だけが所有する未完了request/childは取消し対象で、app-wide supervisorが終了確認・reapまで所有を保持する。
Mainが既に使用中の検証済みtunnelや独立recorderをSetupのCloseで停止しない。再入場でも旧childが終了・reapする前に二つ目を起動せず、
終了待ちを表示してBackと処理CTAを無効にする。終了後は新世代で現在stepの入力から再確認し、旧完了を成功へ引き継がない。
完了済みの外部installや認証事実を画面の取消しだけで巻き戻したとは扱わない。

Windowsのselector寸法は`WIN-SETTINGS-TABS-590`に従う。

Setupの製品名と導入見出しを一つの文字列へ結合しない。`app_title` は全localeで
`Codex Info Monitor`、初回起動の導入見出しと入口labelは次のcatalog値を正本とし、`/`で複数言語を併記しない。
Settingsから開く場合は`WIN-CONNECTION-GUIDANCE-603`に従って接続設定の見出しを使う。
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
- 本文の正本は同じ配布物に同梱した`LICENSE`、`LICENSE.ja.md`、root/Windowsの`THIRD_PARTY_NOTICES.md`、`assets/NOTICE.txt`と各`LICENSES`本文である。外部networkやAPI正常性を表示条件にしない。章は次表の順とし、複数sourceの章は表内の順で出典名と全文を結合する。日本語の説明章だけで原licenseを置換しない。

| 章 | 同梱sourceの順（root基準、Windows noticesだけWindows project基準） |
| --- | --- |
| 本ソフトウェア | `LICENSE.ja.md` |
| 無保証 | `LICENSE` |
| ライセンス | `LICENSE` |
| フォント | `THIRD_PARTY_NOTICES.md`、`LICENSES/OFL-1.1.txt` |
| プロトコル | `THIRD_PARTY_NOTICES.md`、`LICENSES/Apache-2.0.txt`、`LICENSES/OPENAI-CODEX-NOTICE.txt` |
| schema | `LICENSES/Apache-2.0.txt`、`LICENSES/OPENAI-CODEX-NOTICE.txt` |
| 第三者 | `THIRD_PARTY_NOTICES.md`、`LICENSES/MIT.txt`、`LICENSES/BSD-3-Clause-ANGLE.txt` |
| 詳細 | `THIRD_PARTY_NOTICES.md`、`windows-client/THIRD_PARTY_NOTICES.md`、`assets/NOTICE.txt` |
| 配布 | `LICENSE.ja.md`、`LICENSES/Inno-Setup.txt`、`windows-client/THIRD_PARTY_NOTICES.md` |

  本文は章を跨いで省略・要約・重複除去しない。Markdownは書式記号だけを表示用に除去できるが、copyrightを含む本文、code内の文字、linkの表示文字とURLを失わない。
  Legalの本文内scrollへ依存せず、実際のfont/本文幅/本文高で折り返した行を先頭からpageへ詰める。次の段落全体が収まるなら同pageへ置き、収まらず現pageに本文があれば次pageへ送る。1段落だけで本文高を超える場合だけ表示行の境界で分割する。fontを縮小せず、page境界だけで原文へ文字を追加・削除しない。章を跨いで同pageに詰めない。
  全pageの順は章順・原文順、初回は先頭pageとする。前/次で1 pageずつ移動し、端だけ無効とする。前ボタンの「戻る」は前pageを意味し、Window終了は常時有効なCloseで行う。現在の章名と全体のpage位置を表示し、再flowでpage数が変わる場合は変更前に先頭だった原文位置を含むpageへ移す。
  source欠落・読取り失敗では不完全なlicenseを正常表示せず、法的文書を取得できず再インストールによる復旧が必要であることを表示localeで一つ表示する。前/次は無効、Closeと他画面の監視は利用可能とする。自動のインストールや配布許可の判断を行わない。全pageから復元した本文と同梱sourceの意味上の全文一致、切れ・重なりのない実画面、前/次/Closeの効果を受入で確認する。
- HelpはSSH、WSL、API、認証、更新、アンインストール、障害時の情報採取範囲を利用者向けに説明する。
- UIなしsilent RESTはSlint component/window/event-loop生成0、`DISPLAY`/Wayland/X11依存0、Slint HWND=0
  （visible/hidden HWNDとも0）、headless snapshot builder+read-only publisherだけとする。実装と実hostによる証拠がなければ、この契約の品質確認完了とは扱わない。
- Help/Connection guideはMain client `900×542 logical`内の情報surfaceであり、独立Window/HWNDを
  作らない（additional HWND=0）。registered top-level surface inventoryはMain、Setup、Settings、
  Graph、Threads、Legalの正確な6個で、runtime HWNDはMain=1＋open child subset 0..5（合計1..6）である。

### 4.6 失敗と復旧

#### Windows更新の表示状態

更新はPRODUCT §6とSECURITYのWindows updater境界を前提とする独立した状態である。下表の結果をSettingsの更新欄へ表示し、StatusBannerには新版候補がある場合だけ更新操作を置く。backendの接続・認証のprimary CTAを優先し、それが必要な間は更新操作を隠すが、更新の結果はSettingsで閲覧できる。更新状態をLinux側の取得エラーや認証失敗として表示せず、backendの時刻・数値・last-good dataを書き換えない。表示文はI18Nの対応localeへ翻訳し、raw応答・例外本文・保存先pathを含めない。

| 入力・遷移 | 表示と次の操作 |
| --- | --- |
| 新版未確認、確認中 | 更新情報は未確認または確認中。更新操作は表示しない。確認だけでdownloadしない |
| 確認成功、新版なし | 更新なし。更新操作は表示しない |
| 確認失敗、新版候補なし | 更新情報を取得できなかったと表示する。新版なし・更新済みとしない。常設の更新ボタンを追加せず、次のアプリ起動の更新確認で再判定する。失敗を理由に自動retryしない |
| Release/manifestの資格を満たす新版候補あり | 候補versionと「更新」相当の操作を表示する。この時点ではinstaller bytesの検証済みとは表示しない。利用者の1操作から、その試行のdownload・完全検証・標準GUI Setup起動だけを1回行う |
| download・検証・Setup起動中 | 該当する進行状態を表示し、同じ更新操作を無効にする。再クリックをqueueせず、backendの通常取得と独立して処理する |
| download・検証・Setup起動に失敗 | 失敗した段階を短く表示し、「更新を再試行」相当の操作を再び可能にする。候補versionは前回確認した候補であることを保持し、成功・適用済みにしない。自動で再実行しない。明示再試行はRelease確認から1回やり直し、新版なしなら操作を消し、確認失敗なら同じ再試行可能状態を保つ。有効な新版があればその試行で再取得・完全検証し、前の失敗fileや未検証候補を起動しない |
| GUI Setupの子processを正常に開始 | 「セットアップを起動しました」相当を表示する。起動中の同じ子processが終了するまでは更新操作を無効にし、二重起動しない。終了後は実行中アプリのversionを変更せず、候補がそのversionより新しければ更新操作を再び可能にする。子processの開始・終了・exit codeだけで更新済みにせず、次に起動したアプリ自身のversionを使用する。取消・失敗したSetupのrollbackはPRODUCT §6が所有する |
| アプリ終了・世代破棄 | 未完の確認/downloadを取消し、部分fileを破棄する。遅延結果で破棄済み画面を更新せず、成功へ変換しない。既に利用者が開いたGUI Setupは独立して継続し、自動restartは行わない |

新しい確認結果が既に受理済みの候補を置き換える場合は、その結果のversionと資格を一組で採用する。古い候補のversionと新しいmanifest/hashを混ぜない。再試行中も前のbackend表示と更新失敗の事実を保持し、進行中であることだけを追加する。更新の失敗・成功によって別surfaceの取得を開始しない。

#### 接続・認証・data取得の失敗

認証制御には次の二つのadapterがあり、応答を混同しない。いずれもHTTP data routeへ認証mutationを追加しない。

- raw embedded nativeのauth-startは、同processの認証control ownerがCodex App Serverの`account/login/start`へ`type="chatgpt"`を一度送る。認識するresultは必須の`type="chatgpt"`、非空文字列`loginId`、文字列`authUrl`を持つobjectだけとし、文字列はSECURITYのtext/URL制限を満たす必要がある。重複key・既知fieldの不正型・URL欠落/不正・別typeは失敗とする。追加fieldはopaqueとして無視し、表示・保存へ転用しない。このbrowser flowでURLなしの正常resultを推測しない。外部の型定義はCodex CLI 0.159.2の生成`v2/LoginAccountResponse`、意味は[公式App Server認証](https://learn.chatgpt.com/docs/app-server#authentication)を参照する。外部schemaの他typeを、この製品が要求したbrowser flowの応答へ代用しない。
- installed Linux UIとWindowsのauth-startは、対象Linux profileのCodex CLIに対する明示起動であり、JSON-RPC resultやURL文字列をUIへ返す操作ではない。ローカルadapterの結果は`Launched`（非nullの子process handleを取得）または`Failed`（起動例外/null handle）の二値とする。installed Linux UIはresidentと同じ検証済みCODEX_HOMEとSECURITYの実行file検証を使い`codex login`を起動する。WindowsのWSLは保存済みの検証済みdistributionを対象に`codex login`、SSHは保存済みの検証済みHost aliasを対象に`codex login --device-auth`を実行し、利用者が操作できるconsoleを表示する。入力可能な汎用commandは作らず、Windows側は検証済みwsl.exe/ssh.exeと構造化ArgListを使う。CLIの[公式login契約](https://learn.chatgpt.com/docs/developer-commands#codex-login)に従い、browser/device code・資格情報の取得/保存は選択sourceのCLIが所有する。stdoutをURL/資格情報としてparse・保存・logしない。Windowsのprofile=`none`では手動接続先のLinux hostを特定できないためdefault WSLへ代用せず、Setupで接続profileを指定する操作を提示する。
- `Launched`は認証済みではない。installed Linux/Windowsの認証確認actionは、既存profileの接続準備・healthの制御段階の後、別にfresh strict currentと必要なthreads bundleを一度取得する一つの明示操作である。起動成否・接続準備成否をdata rootへmergeせず、このbundleの受理済み認証状態だけで表示を遷移する。別のauth-check HTTP/RPC応答schemaを仮定せず、UIからApp Serverへの通常収集requestを追加しない。raw embeddedも認証確認後はfreshな公開dataの受理を必要とする。
- pendingの認証sessionは開始成功から、fresh bundleの認証済み受理、control/CLIの明示失敗・取消、またはgeneration破棄まで保持する。CLIの正常終了だけではpendingを消さず、認証確認を再び操作可能にする。CLIの非0終了・起動失敗、または同じloginIdの失敗通知では再開始可能にする。通知や終了コードから認証済みdataを生成しない。pending child/control/URLはgenerationへ結び、実行中の同じ対象へ二重auth-startを行わない。

失敗の分類は下表で固定する。原因は確認できた境界までを短く表示し、raw backend error、stderr、秘密情報を表示しない。
保存済み同accountの有効値を保持する場合は最終受理時刻と失敗を併記し、最新/正常と表示しない。last-goodのない値は未知である。
認証主体変更・logout・identity不成立の消去はPRODUCT/DATAのhard boundaryを優先し、別accountの値を保持しない。
取得待機/in-flightの無効条件を先に適用し、最新の取得失敗は保持中の古いready/auth_required/partial状態より優先してprimary CTAを決める。取得に成功してauth_requiredを再受理した場合だけ認証操作へ戻す。独立surfaceとSettings保存の失敗をMainの状態へ重ねて優先順位を作らない。

| 入力状態・原因 | 利用者への影響 | 単一primary CTAと結果 |
| --- | --- | --- |
| 設定fileが空・破損・schema/selector不正 | 接続設定を受理できずMainはdisconnected | 「設定」からSettings recoveryへ。自動のcommand実行・Welcome再表示をしない |
| prepare、WSL起動、SSH/tunnel起動またはlistener検証失敗 | APIへ到達できない。受理済み表示は保持 | Setupでは失敗した同じprofile stepの再試行。Mainでは再試行で既存接続手順を1回開始。未知ownerへsignalしない |
| health、HTTP、timeout、pair、schema/domain検証失敗 | 新しい表示bundleを受理できない | 「再試行」で同surfaceの取得を1回開始。health/controlだけで成功表示しない |
| positive Main bundleのthreads失敗・pair/件数不一致 | Mainと詳細の一組を受理できず直前の完全表示を保持 | 下記10秒待機規則。待機・取得中はCTA無効で、自動再取得を重複させない |
| Linux応答の`state=error`、または取得済みmodel集合のpartial/unavailable | Linux側の取得状態と確認できた値の範囲を表示。client接続失敗と混同しない | 「再試行」でMainのread-only取得を1回行う。daemon再起動・DB修復が成功したとは表示しない |
| Windowsで`auth_required`かつprofile=`none` | 手動接続先の認証を自動操作する対象が不明 | 「接続設定」でSetupへ。別sourceのCLIを起動しない |
| `auth_required`、認証開始前 | 認証完了まで新しい認証済みdataを受理できない | 「認証を開始」で既存profileのauth-startを1回実行。操作中は無効 |
| auth-startでSECURITYの検証を通した認証URLを取得 | 外部browserで利用者の認証が必要 | URLを1回開き、primaryを「認証を確認」にする。再度開くためのURLはsecondary linkとし、同格CTAを増やさない |
| CLI auth-startが`Launched` | 対象sourceのCLIで認証を開始した。dataではまだ未確認 | 「認証を確認」を操作可能にする。CLI起動/終了コードだけでreadyにせず、URLのないJSON正常応答と解釈しない |
| 認証URLのbrowser起動失敗 | 認証ページをまだ開けていない | 「認証ページを開く」で同じ検証済みURLを開く。成功後は「認証を確認」へ。URLを永続化しない |
| auth-start失敗、またはURL不正 | 認証を開始できていない | 「認証を開始」を再び操作可能にする。不正URLは開かず、controlの失敗でdata rootを変更しない |
| 認証確認で有効なfresh bundleを受理したが`auth_required` | 認証は未完了 | pendingの認証sessionがあれば「認証を確認」を再び操作可能にする。pendingがなければ「認証を開始」。成功したprocess exitだけでreadyにしない |
| 認証確認のcontrol段階だけ成功（raw embedded adapter） | 認証済みdataは未受理 | 同じ確認action内でfresh strict currentと必要なthreads bundleを1回取得し、その受理結果へ遷移。installed Linux/Windowsに独立したauth-check RPCを追加しない |
| 認証確認で認証済みのfresh bundleを受理 | 公開dataで認証状態を確認できた | この受理済みbundleをatomicに表示し、pending認証sessionを終了する。成功を理由に二度目のbundle取得を追加しない。`state=error`やpartial等の表示は受理した値と上記の状態規則に従い、認証済みだけでreadyとしない |
| Graph/Threads独立取得失敗 | 対象surfaceだけlast-goodと失敗を保持 | そのsurfaceの選択/通常周期で再取得。Mainを失敗に変更せず、Windowを勝手に前面化しない |
| Settings保存失敗 | 旧active設定とtheme以外のdraftを保持しWindowを閉じない。theme選択/previewは`WIN-THEME-422`に従って保存済み値へ戻す | 「保存」で現在のdraftを再試行可能。取消/Closeはdraftを破棄し、保存成功を偽装しない |
| 未知の失敗種別 | 成功とは判断できない | 同surfaceの一般取得失敗へ分類して再試行可能。新たな自動復旧commandや未知値を作らない |

Mainのpositive bundle失敗時は、失敗が確定したmonotonic時刻から10秒後を次の取得時刻とし、currentを条件headerなしで1回取得する。
その時刻より前、および取得中はMain再試行とSettings「状態を更新」の同じMain取得部分を無効にし、「再試行待ち」または「取得中」を表示する。
force入力をqueueして期限を短縮・延長せず、Graphと独立Threadsのrequestも開始しない。Windowの開閉やlast-goodの閲覧は可能とする。
再取得も失敗した場合はその失敗確定時刻から次の10秒待機とし、一つのcallback内で反復しない。成功したMain bundleだけが待機・errorを解除する。
認証主体/表示account/接続設定generationの確定変更では旧generationの待機とin-flight応答を破棄し、新対象の通常起動手順へ進む。
同じ対象でのforce、Window開閉、操作連打はgeneration変更ではない。

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
Tab/Shift+Tabは、現在表示中で有効な操作だけを次表の順で巡回する。非表示page、無効CTA、装飾labelへfocusしない。
同じ行のフォーム欄と選択肢は、当該pageの表示順（上から下、同じ行は左から右）とする。Windowを初めて開くときは最初の有効な内容操作へfocusし、背景更新でfocusを移さない。

| Window | 内容操作のTab順 | Escape（popupがない場合） |
| --- | --- | --- |
| Main | 推移→法的通知→設定→thread詳細→状態CTA→最小化→閉じる | 何も変更しない |
| Graph | mode表示順→metric→period selector→前→次→series表示順→最小化→閉じる | Graphを閉じる |
| Threads | 一覧scroll→最小化→閉じる | Threadsを閉じる |
| Settings | tab表示順→選択tabのフォーム/操作表示順→接続設定/Legal入口→Save→Cancel→閉じる | Cancelと同じdraft破棄 |
| Setup | 現stepの選択/入力/操作表示順→Back→primary CTA→閉じる | Backが有効なら前step、なければ閉じる。実行中処理の旧generation完了をcommitしない |
| Legal | 前page→次page→最小化→閉じる | Legalを閉じる |

Enter/Spaceはfocus中のbutton/toggleを実行する。single-line入力欄のEnterは、そのpageの有効なprimary CTAを1回実行し、なければ何もしない。
tabの左右矢印は隣のtabを選び、その内容を直ちに表示する。両端では移動せず、Settingsの未保存draftはtab切替で保持する。
閉じたselectorのEnter/Space、下矢印、Alt+下矢印は候補popupを開き、現在の選択を強調する。
popup内の上下矢印は表示中の隣の候補へ強調だけを移動し、両端では移動しない。Enterまたは候補のclickで選択を確定して閉じる。Settingsのaccount切替はこの確定時に反映し、Saveを待たない。
Graphの日付見出しはEnter/Spaceで展開状態を切り替え、右矢印で展開、左矢印で折り畳む。見出しの操作だけではperiodを選ばない。
popupが開いているEscapeはpopupだけを閉じ、確定済みの選択、Window、draftを変更しない。
Alt+F4は現在のWindowのCloseと同じ動作とし、単独Altや未定義のaccess keyに追加actionを割り当てない。
フォーカス、hover、pressed、disabled、busy、errorを視認できる。focus枠は対象外周の2 logical pixel、隣接面に対して3:1以上とし、
integer DPIのscaleで描画する。Windows high contrastではOSのfocus/foreground色を使い、theme固定色で不可視にしない。

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
  はMainで少なくとも900×542 logical、Setup/Settings/Threads/Legalで少なくとも900×480 logical、Graphで少なくとも700×480 logicalを必要とし、未満は
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
| 未決定条件 | 実装開始時は0件。発見したらowner仕様へ戻して解消する |

## 7. UX受入ゲート（対象仕様の確定後に使用）

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

## 既存Settingsの稼働バージョン（Issue #481）

`RUNTIME-SETTINGS-481`: Linuxの既存TimeZoneSettingsWindowとWindowsの既存Settingsで `RUNTIME-VERSION-481` のRecorder/REST値を別々に表示する。画面を開く際に既存loopback API clientから取得し、Windowsは既存Refresh操作でも更新する。一致、不一致、取得不可を同じ意味で示す。取得に失敗した値をUI自身のversionや前回値で補完せず、取得不可へ戻す。Linuxの既存Settingsはaccount selectorと稼働versionの3行を収めるため高さ356pxとし、幅・既存の設定操作を維持する。
