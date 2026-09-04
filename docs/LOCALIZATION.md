<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->
<!-- codex-info-requirement-owner: I18N -->
<!-- codex-info-master-ids:
PROC-I18N-01
-->

# 多言語化・日本語表示仕様

Codex Infoの固定UI文言、時刻表示、フォント、配布ライセンスに関する仕様です。GPLv3の正式な条件は[LICENSE](../LICENSE)に定めます。

## 対応言語とフォント

起動時localeのprimary language subtagに対応するcatalogとフォントを選びます。

| primary subtag | 表示言語 | フォント |
| --- | --- | --- |
| `ja` | 日本語 | `Noto Sans JP` |
| `en` | English | `Noto Sans JP` |
| `zh` | 简体中文 | `Noto Sans CJK KR`（共通Han字形） |
| `ko` | 한국어 | `Noto Sans CJK KR` |
| `es` | Español | `Noto Sans JP` |
| `fr` | Français | `Noto Sans JP` |
| `de` | Deutsch | `Noto Sans JP` |
| `pt` | Português | `Noto Sans JP` |
| `it` | Italiano | `Noto Sans JP` |
| `ru` | Русский | `Noto Sans JP` |

localeは`LC_ALL`、`LC_MESSAGES`、`LANG`の順に最初の非空値を採用します。encoding suffix（`.UTF-8`）とmodifier（`@...`）を除き、`-`と`_`を同じ区切りとしてprimary subtagを判定します。`C`、`POSIX`、不正値、未対応言語は英語catalogへ対応付け、`zh_TW`と`zh-Hant`は簡体字catalogへ対応付けます。

localeとtimezoneはプロセス起動時に確定します。`TZ`には`Asia/Tokyo`や`Europe/Berlin`などのIANA IDを指定でき、無効値はUTCへ対応付けます。

## 時刻基準

- epoch秒、並び順、期間の境界はUTCで保持します。
- 絶対時刻、履歴期間、グラフ横軸は起動時timezoneへ変換し、数値UTC offset（例`+09:00`）を付けます。
- 日本語catalogの絶対時刻はGregorian calendar・ASCII digit・24時間表記の
  `yyyy/MM/dd HH:mm ±HH:MM`へ固定します。fixture `1787356800` はUTCで
  `2026/08/22 00:00 +00:00`、Asia/Tokyoで`2026/08/22 09:00 +09:00`です。
  timezone selectorの保存値は`local`または`UTC`だけとし、`local`の具体IANA zoneは実行hostから
  解決して表示へ使います。IANA zone名自体をWindows設定値として保存しません。
- 経過時間と残り時間はUTC秒の差分を各言語の単位へ変換します。
- 無効epochは値欄を`—`、グラフ軸を空値、履歴選択肢を除外として表示します。

## 固定文言

固定UI文言はcatalogで管理します。thread title、email、モデル名、製品名、ライセンス名、ログ生値は原文を表示し、数値とepoch秒だけを表示時のlocale・timezoneへ変換します。

日本語・韓国語フォントは`assets/NotoSansJP.ttf`と`assets/NotoSansKR.otf`をSlintへ埋め込み、起動時localeのフォントを各Windowへ適用します。フォントのOFL-1.1通知は[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)と[assets/NOTICE.txt](../assets/NOTICE.txt)に記載します。

ネイティブタイトルバーは使用しません。各Windowのボタン以外の画面領域を移動用に使い、画面内タイトル領域、操作記号、固定文言は埋め込みフォントでlocale表示します。Graphだけは四隅の広い対角領域と辺から枠をリサイズし、最大化／復元も提供します。

## Windows UIA・Setup・ページ翻訳の正本（RC-085〜087）

Decision ID: `UX-20260823-I18N-001`

状態: `EXTRACTION_DECISION_RECORDED / PRODUCT_PENDING`

### 利用者の課題

localeを変えると表示labelだけでなくUIAのName、shortcut、Tab列、Help scope、ページ割当まで別物に
なったり、長い翻訳がclipして主要操作へ到達できなくなったりする問題を許容しない。

### 目的

supported localeとunknown fallbackを一度だけ解決し、全surfaceの同一semantic topology、Setupの完全な
key集合、page単位の完全性を固定する。

### 検討案と棄却理由

1. WindowsのOS localeと文字数だけでlabel/pageをruntime生成する案は、fallback混在、途中切断、
   semantic itemの欠落・重複を検出できないため棄却する。
2. localeごとにUIA IDやTab列を設計する案は、支援技術の導線とrouteを変えて回帰を隠すため棄却する。
3. canonical catalog ID、locale不変のAutomationId、semantic page manifest、翻訳text hashをjoinする案を
   採用する。内容が長い場合は意味paragraph境界でpageを分割し、font縮小やscrollを許可しない。

### 採用するlocale集合と解決

supported catalog IDは次の10件に固定する。

`[ja,en,zh-Hans,ko,es,fr,de,pt,it,ru]`

入力localeは`LC_ALL`→`LC_MESSAGES`→`LANG`の最初の非空値から一度だけ解決する。`zh`、`zh-CN`、
`zh_CN`、`zh-TW`、`zh-Hant`を含む`zh` primaryはcatalog ID=`zh-Hans`、`ja-JP`等は対応するprimary
catalog、unknown・不正値・`C`・`POSIX`・未対応primaryはcatalog ID=`en`へ対応付ける。evidenceには
`input_locale`と`resolved_locale`を別fieldで保存し、1 surface内で複数localeのkeyを混在させない。

### Setup exact key集合

各supported catalogは次のkeyをちょうど1回ずつ持つ。unknown入力は同じ集合をresolved `en`から読む。

| 分類 | exact key集合 |
| --- | --- |
| title | `setup.title` |
| profile | `setup.profile.label`, `setup.profile.none`, `setup.profile.wsl`, `setup.profile.sshConfigAlias` |
| step | `setup.step.connection`, `setup.step.api`, `setup.step.auth`, `setup.step.ready` |
| primary | `action.connection.startForward`, `action.connection.checkApi`, `action.auth.start`, `action.auth.check`, `action.setup.continue`, `action.close` |
| navigation | `action.back`, `action.cancel` |
| SSH/auth error | `error.ssh.profile.invalid.cause`, `error.ssh.profile.invalid.impact`, `error.ssh.local-port-in-use.cause`, `error.ssh.local-port-in-use.impact`, `error.ssh.interaction-required.cause`, `error.ssh.interaction-required.impact`, `error.ssh.process-start-or-exit.cause`, `error.ssh.process-start-or-exit.impact`, `error.ssh.health-unavailable.cause`, `error.ssh.health-unavailable.impact`, `error.auth.required.cause`, `error.auth.required.impact` |

key欠落、未解決key、文字化け、catalog内locale混在は0とする。表示上のprofile/step/primary/Back/Cancel/error
はこの集合からjoinし、raw SSH stderr、host/user、password、token、pathはcatalog値へ連結しない。

### UIAとsemantic page manifest

Main、Setup、Settings、Graph、Threads、LegalおよびMain HWND内Helpは、localeに依存しない同一
`AutomationId`、focus topology、Tab/Shift+Tab逆順、Alt chord、Enter/Escape action、routeを使う。
各controlのUIA manifestは`AutomationId`、catalog由来の非空`Name`、`HelpText`/`Description`、
`AcceleratorKey`、visible `bounds`を持ち、Name/descriptionの未解決、ID重複、操作差を0とする。

各source item/paragraphは`surface`、`semantic_id`、source `text_hash`を持つ。localeごとのpage manifestは
`input_locale`、`resolved_locale`、`page_id`、`page_index`、`page_count`、`semantic_id`、翻訳
`text_hash`、`bounds`、`clip`を記録する。全page joinは各semantic IDがちょうど1回で、
`missing=0`、`extra=0`、`duplicate=0`、`clip=0`とする。長い翻訳はparagraph境界でpage assignmentを
変えられるが、semantic ID、順序、内容の削除・要約・重複は許可しない。

## X版との関係

X版の値、時刻、期間、item/paragraph、状態、順序、表示所有権は変更しない。Windowsではcatalog解決、
UIA metadata、page assignmentを追加するだけであり、locale切替を理由にprotocol値やデータ意味論を
変換・削除しない。

## 影響要求

`RC-085`, `RC-086`, `RC-087`, `WIN-E-001`, `WIN-G-004`, `WIN-G-013..016`, `WIN-M-008..010`,
`WIN-M-019`, `WIN-M-021`, `WIN-M-025..026`, `WIN-M-029`, `WIN-I18N-01..02`, `WIN-ACC-01`。

## 非スクロール影響

各localeのMain、Setup、Settings、Graph、Threads、LegalとMain内Helpで、主要情報、primary、Back、Close、
UIA focus controlを同一viewportへ収める。page/章/選択詳細だけを到達手段とし、root/internal ScrollViewer、
font縮小、文字数だけの途中切断へ逃げない。

## 証拠計画

同一release artifact SHAで、10 supported locale＋unknown入力、全surface＋Main内Help、全17 state、
supported size/DPI/theme/motionについて、catalog key join、UIA AutomationId/Name/Description/shortcut、
Tab topology、semantic page manifest、source/translation hash、bounds、clip、missing/extra/duplicateを
raw記録する。Setupはexact key集合の一回性、unknown→en、単一resolved localeを別joinで確認し、実装者と
異なる担当が三値判定する。実装、実画像、UIA hostログは未取得である。

## 未確定

locale集合、key集合、fallback、UIA/semantic join規則は本Decisionで確定した。実artifact、fresh画像、
スクリーンリーダー/UIA操作ログ、独立製品判定は未取得であり、製品状態は`PRODUCT_PENDING`である。
X版の意味論を変更する判断、locale別にtopologyを変える判断、未登録locale/key/stateの推測は未採用である。

## 配布ライセンス

英語の[LICENSE](../LICENSE)がGPLv3の正文です。[LICENSE.ja.md](../LICENSE.ja.md)は日本語案内です。独自コードと文書は`GPL-3.0-only`、生成スキーマはApache-2.0、同梱フォントはOFL-1.1、SlintとCargo依存クレートは各上流ライセンスで提供します。

ソース・バイナリ配布物には`LICENSE`、[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)、[assets/NOTICE.txt](../assets/NOTICE.txt)、[LICENSES/](../LICENSES/)を同梱します。
