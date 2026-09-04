<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

> **文書の位置づけ:** 本書は製品要件の非規範的な案内／実装説明です。要求の入口とowner registryは `docs/PRODUCT_REQUIREMENTS.md` および同文書から参照される REST/DATA/UX/LOCALIZATION 仕様です。要求変更時は該当する master ID と owner に従い、本書だけで契約や監査成果物を追加・変更しません。

# Windows クライアント

`windows-client/` は、Linux / WSL で動く Codex Info の既存機能を Windows から利用する
デスクトップクライアントである。無料の Visual Studio Community で solution を開ける
Avalonia / .NET 10 プロジェクトであり、Windows が配布先、Linux / X11 が開発時の
実画面確認先になる。既存の Rust + Slint ネイティブ画面を置き換えず、同じサーバー状態を表示する。
日本語表示には既存配布物の `assets/NotoSansJP.ttf` を client assembly に埋め込み、
Linux と Windows の両方で同じ font fallback に依存しない。ライセンスは既存の
`assets/NOTICE.txt` の記載を引き継ぐ。

状態: `REQUIREMENTS_SELECTED / PRODUCT_PENDING`。SSH-001/RC-061〜063の設定、接続、headless、
supervisor、recorder、service lifecycleは要求から導出した実装説明であり、実装・host・artifact・fresh image・
独立製品証拠を取得するまで製品PASSを主張しない。installed API serviceとrecorder serviceのexact
install/start/stop/restart/uninstall/rollback commandは未確定で、読者にpathやbinaryを推測させない。

## 利用者向け導入手順（Start メニューから起動）

### 1. Linux / WSL サーバー側

UIありの起動契約は次のとおりです。

```bash
./run.sh
```

UIなしsilent REST（installed service）の起動契約は次のとおりです。

```bash
codex_info --port 8787
```

期待値はSlint component/window/event-loop生成=0、`DISPLAY`/Wayland/X11依存=0、Slint HWND=0
（visible/hiddenとも0）、listenerはloopback `127.0.0.1:8787`だけ、外部bind=0である。
headless snapshot builderとread-only publisherだけを許可する。このGUI依存ゼロ契約は実装未取得のため
PRODUCT_PENDINGである。

server/API prepare→listener→GET health→GET status→必要時auth-start→別auth-check→readyの順序を固定する。
installed API serviceのexact install/start/stop/restart/uninstall/rollback commandはRC-063時点で未確定で、
ここに実行可能なコマンドを発明しない。

recorderはUI/RESTと独立ownerであり、app/tunnel終了後も継続、同時tunnel=1、orphan tunnel=0、
same-generation auto retry infinite=0、child reap=1を要求する。recorder serviceのexact commandも
release manifest確定までPRODUCT_PENDINGである。

headless契約はSlint component/window/event-loopを生成せず、`DISPLAY`/Wayland/X11へ依存しない。
Windows側へ8787番ポートを公開するために、serverを`0.0.0.0`やLAN addressで待ち受けさせない。

### 2. Windows クライアントのインストール／アンインストール

配布物は `CodexInfo.WindowsClient.Setup.exe` というInno Setup 7.1.0の標準GUIウィザードである。
`windows-client/tools/Build-WindowsInstaller.ps1` がlocked restore、win-x64 self-contained publish、
第三者notice収集、`.iss`コンパイルを順に行う。SetupはUACを要求しないユーザー単位インストールで、既定の
`%LOCALAPPDATA%\Programs\Codex Info Monitor`、Start Menu、Windowsの「インストールされているアプリ」へ
登録する。同じAppIdの再実行は更新になり、稼働中の本体は標準のRestart Manager境界で閉じる。

アンインストールはInno Setupが生成する標準uninstallerから行う。本体payload、shortcut、Apps登録、既知の
空ディレクトリを除去する一方、`%LOCALAPPDATA%\CodexInfo` の設定とLinux側の3か月履歴DBは削除しない。
本体、Setup、uninstaller、shortcut、Apps登録のDisplayIconは同じ `CodexInfo.ico` を使用する。
旧自作bootstrapperの登録と `CodexInfo.WindowsClient.Uninstaller.exe` は初回更新時に移行削除する。
Setupは資格情報、SSH鍵、raw接続先を保存しない。Authenticode署名は外部のコード署名証明書があるrelease工程で
のみ実施でき、証明書がないローカルbuildを署名済みと表明してはならない。

GitHub配布版の更新確認は、固定repository `salty919/codex_info_v2` の公開済み
`windows-vX.Y.Z` Releaseだけを対象とする。起動時は新版の有無を確認して状態帯へ通知するだけで、
downloadもSetup起動も行わない。新版がある時だけ表示される「更新する」を利用者が押すと、exact-nameの
manifestとSetupを取得し、version、許可済みHTTPS authority、size、SHA-256を検証してから通常のInno Setup
GUIを起動する。silent install、unattended apply、自動再起動、常設Headerボタンは使用しない。

Windows製品版は`windows-client/Directory.Build.props`のstable `X.Y.Z`を参照する。PRは版番号の
単調増加とbuild/test/installer/E2Eを検証するだけでReleaseへ書き込まない。`main`へmergeされた版番号が
上がり、全Windows gateがPASSした時だけ`windows-vX.Y.Z`とSetup/update manifestを公開する。mainの
release処理は同一refで直列化し、HTTP 404だけを不存在として受理する。tagを原子的に新規作成した後、
非公開Draftへexact 2資産をuploadし、名前・byte size・upload状態・tagのcommit SHAを検証してから公開する。
既存tag/Release、通信失敗、5xx、部分uploadへ上書き・追記して継続しない。

### クライアント変更とバージョンの必須対応

`windows-client/src/**`、`windows-client/installer/**`、`windows-client/Directory.Build.props`、
または配布物・更新マニフェストを生成する`windows-client/tools/Build-WindowsInstaller.ps1`、
`New-WindowsUpdateManifest.ps1`、`Collect-ThirdPartyNotices.ps1`を変更する場合は、同じ変更で
`Directory.Build.props`の`Version`を直前の安定版より上げる。版番号を上げない実装変更をPRへ出してはならない。
版番号を上げないテストだけの変更は、配布物を変えないためこの規則の対象外である。

PRのWindows workflowはbuild/testを通すだけでは配布反映を意味しない。`main`へmerge後のpushで、版番号が
単調増加し、全Windows gateがPASSした場合だけ新しいSetup、`windows-vX.Y.Z` Release、update manifestが
生成される。したがって「Windows workflowが成功した」ことと「利用者のクライアントが更新される」ことを
同一視しない。版番号が据え置きの実装変更はrelease holdとする。

### 3. SSH 転送と初回セットアップ

初回起動時は、画面の「初期設定」に従って次の順序で進む。
Windows OpenSSH の設定ファイルは `%USERPROFILE%\.ssh\config` を参照し、保存するのはそこに定義された
literal `Host` aliasだけとする。

1. `connectionProfile`と`connectionSelector`を選択する。profileは`none|wsl|sshConfigAlias`、WSL selectorはinstalled distribution exact token、SSH selectorはliteral Host alias（`^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$`）。
2. server/API prepare→listener→`GET /v1/health`でservice readinessを確認し、`GET /v1/details`から最初の完全な表示世代を取得する。
3. 必要な場合だけauth-startを明示し、auth-start成功をreadyとしない。
4. 別のauth-checkでlater details generationを取得し、wireの`ready` booleanは使わず、`state=ready AND authenticated=true`の導出条件だけでMainへ進む（ready wire boolean field=0）。
5. `language/setupCompleted/connectionConfigured/timeZoneId/connectionProfile/connectionSelector`の6-key objectをflush・validate後atomic replaceする。

自動RemoteはOpenSSHを直接ArgumentListで起動し、shell/cmd/PowerShell=0、`BatchMode=yes`、hidden prompt=0とする。
未登録/変更host keyはconnectedとせず、明示CTA時だけOpenSSH-ownedの一回のinteractiveを許可する。
raw manual host/userはone-session raw recoveryだけで、settings、selector、完了状態へ昇格しない。
old 4-key、corrupt settings、invalid profile/selectorはWelcome loopにせず、Main disconnected+Settings recovery、
automatic recovery command count=0とする。保存selectorが有効なら次回launchでapp-wide supervisorがbootstrap/tunnelを
自動再構築し、poll/reconnect/same-generation rebuildでSetup/app confirmationを再表示しない。

保存禁止はpassword/token/key/path、OpenSSH expanded values、raw manual host/user、API URL、argv、stderrである。
`HostName/User/Port/IdentityFile/Include`はclientが展開せず、literal Host labelだけをOpenSSHへ渡す。

## 通信境界

クライアントがHTTPで読む接続先は編集不可の
service readiness用`http://127.0.0.1:8787/v1/health`と、唯一のruntime data用
`http://127.0.0.1:8787/v1/details`だけである。
Linux の実アドレス、LAN アドレス、ホスト名、インターネット URL をHTTP endpointとして
入力・保存しない。SSH転送開始に必要なraw Linux host/IPまたはraw userはone-session raw recoveryとして
メモリ上だけで扱い、settings、shortcut、ログへ保存しない。durableに保存するのはprofileと、WSL installed
distribution exact tokenまたはSSH literal Host aliasの`connectionSelector`だけである。

```text
Windows client -- HTTP / 127.0.0.1:8787 --> SSH local forwarding
                                                  |
                                                  | encrypted + peer-authenticated by SSH
                                                  v
Linux / WSL -- 127.0.0.1:8787 --> Codex Info native UI + REST v1
```

クライアントは保存selectorを1 argv tokenとして、Windows標準の`ssh.exe`へ
`-o BatchMode=yes -N -L 8787:127.0.0.1:8787 <validated alias>`を直接ArgumentListで引き渡す。
自動Remoteは`BatchMode=yes`、hidden prompt=0、shell/cmd/PowerShell=0とする。
認証開始ボタンは、WSL profileではinstalled distribution tokenを含む`wsl.exe` ArgumentList、remote SSH
profileではliteral Host aliasを含む`ssh.exe` ArgumentListを一回だけ起動する。どちらも認証情報を受け取らず、
開始直後を認証完了とは扱わない。「認証を確認」で同じprofileのlater `/v1/details` generationを取得し、
`state=ready`かつ`authenticated=true`になった場合だけ完了し、ready wire boolean field=0とする。未登録/変更host keyのautomatic routeは
connectedにせず、明示CTAの一回のOpenSSH-owned interactiveだけを許可する。

コピー用の表示文字列をlaunch inputへ再利用せず、shell/cmd/PowerShell経由の実行は行わない。
Linux / WSL側のinstalled service起動契約は`codex_info --port 8787`である。

HTTPS はここで必要としない。HTTP が使われるのは二つの loopback 終点と SSH トンネルの
内側だけであり、端末間の暗号化と相手認証は SSH が担当する。インターネット経由の利用は
この設定を広げず、別の認証・脅威モデルとして設計する。

## Windows clientの実装・受入境界

Visual Studio/.NET project、NuGet restore、build、test、client起動のexact commandは、今回の要求抽出の
実行契約ではない。製品/runtime/evaluationの証拠、artifact SHA、physical Windows host証拠が揃うまで
PRODUCT_PENDING/HOLDを維持し、読者が未確定のpathやcommandを推測して実行できる形にしない。実行時通信の
意味契約は固定loopback URLへのGET、SSH/WSL childの直接ArgumentList、shell/cmd/PowerShell=0である。

## 表示と更新

初回取得を直ちに行い、完了から 10 秒後に次の取得を行う。3 秒で応答がなければ失敗と
する。Headerに手動の取得更新ボタンは置かない。周期取得は単一要求ゲートを使い、前の要求が
残っている間に別要求を待ち行列へ積まない。Window を閉じると、タイマーと要求を
cancellation token で停止する。ソフトウェア新版の「更新する」は、上記のとおり新版がある時だけ
StatusBannerへ現れる別操作であり、利用者が押すまでdownloadもSetup起動も行わない。

| 入力状態 | 状態帯 | 値の扱い |
| --- | --- | --- |
| `ready`（残量2%以下） | 利用枠の危険 | 最新の有効 snapshot を表示し、残量不足を赤で示す。 |
| `ready`（残量10%以下） | 利用枠の警告 | 最新の有効 snapshot を表示し、残量不足をアンバーで示す。 |
| `ready`（リセットまで24時間以内） | リセット警告 | 最新の有効 snapshot を表示し、リセット接近をアンバーで示す。 |
| `ready`（上記以外） | 正常 | 最新の有効 snapshot を表示する。 |
| `initializing` | Linux 側で準備中 | 有効 snapshot を表示する。 |
| `auth_required` | Linux 側で認証が必要 | 正常に受理した状態遷移として旧account可視値を直ちに空へ置換し、認証操作だけを表示する。旧quota/model/history/threadを現在値として表示しない。Linux側DB自体は削除しない。 |
| `error` | Linux 側の取得エラー | 直前の有効 snapshot があればstaleとして保持し、なければ未取得を表示する。通信障害とは混同しない。 |
| timeout、接続不能、HTTP 非 2xx | 接続エラー | 直前の有効 snapshot があれば保持し、「現在は更新できていません」と示す。 |
| content-type、サイズ、JSON、契約の不正 | 応答エラー | 直前の有効 snapshot があれば保持し、「現在は更新できていません」と示す。 |

`ready` の派生状態は、危険（残量2%以下）→警告（残量10%以下）→リセット警告
（リセットまで24時間以内）→正常の順に一つだけ選ぶ。`auth_required`、`error`、通信障害、
応答障害はこれより優先し、Wire の `state` を変更しない。

`auth_required` はinvalid responseではなく、認証epochを切り替える有効な消去遷移である。
この遷移を通常のlast-good保持で上書きしてはならない。旧accountのplan/quota/model/history/threadを
画面とアクセシビリティtreeから同じroot updateで除去し、Graphのmetric/toggleのようなaccount非依存controlだけを
保持できる。消去rootをMain生存中に適用できない場合は旧account情報を表示し続けずcontrolled shutdownとする。

最初の取得が失敗したときは、値欄を `未取得` とし、推測した 0 や前回プロセスの値を
表示しない。API の `observed_at` が `null` のときは「Linux の観測時刻: 未取得」、
`plan_label` が `null` のときは「プラン: 未取得」、`quota` が `null` のときは
「残り利用枠: 未取得」、モデル配列が空のときは「モデル利用: 未取得」と表示する。

## REST v1 の受理契約

クライアントは [REST API v1](REST_API_V1.md) の `GET /v1/health`をservice readinessだけに使い、
実装では、現行REST v1仕様に従い、履歴・Threads・ドル内訳を含む `GET /v1/details`を受け取る。
`Content-Type` は `application/json`、response headerは8 KiB以下とする。本文はtransfer後・
decode前で、`/v1/details`は33,554,432 bytes以下とする。
`Content-Length` が各上限を超える場合は本文を読まず、chunkedまたは不明長の本文は読み取り
途中で各上限を超えた時点で停止する。自動解凍は無効なので`Content-Encoding`付き応答を
解凍して受理しない。SQLiteは過去3暦月を保持するが、`/v1/details`の1回の取得は最長1暦月である。
本文上限とは別にhistory periods 128件、history samples 44,640件、threads 256件、models 3件を
上限とし、どれか一つでも超えたcandidate全体を拒否する。

トップレベルでは `api_version`、`state`、`observed_at`、`authenticated`、
`plan_label`、`quota`、`models`、`active_thread_count` の全キーが必須である。
`observed_at`、`plan_label`、`quota` だけは `null` を許す。未知キー、大小文字の違う
キー、必須キーの欠落、型違い、任意の object 階層での重複キーを拒否する。

| field | 受理する値 |
| --- | --- |
| `api_version` | 正確に文字列 `v1` |
| `state` | `initializing` / `ready` / `auth_required` / `error` |
| `observed_at`, `reset_at` | `null`（`observed_at`のみ）または JSON 整数の Unix 秒 `1..253402300799` |
| `plan_label` | `null` または改行・control・bidi formattingを含まない1〜64 Unicode scalar |
| `remaining_percent` | 有限 JSON number の `0..100` |
| `window_seconds` | JSON 整数 `1..Int64.MaxValue` |
| token / active thread count | JSON 非負整数 `0..UInt64.MaxValue` |
| model | 最大3件、重複なしの `SOL` / `TERRA` / `LUNA` |

`quota` が object なら `remaining_percent`、`reset_at`、`window_seconds`、`monthly` を
すべて必須かつ null 不可とする。各 model object も `name`、`input_tokens`、
`cached_input_tokens`、`output_tokens` をすべて必須かつ null 不可とする。整数値に
`1.0`、指数表現、文字列を使うことは許可しない。検証失敗の本文や例外の生メッセージは
画面・ログに表示しない。

## 後段の実装・証拠取得

preview、Windows実機、UI画像、contract gate、物理入力smoke、build/test、process/DB/host監査は後段の
独立受入で行う。対象artifact SHAとfresh証拠が未取得の間は、通常・auth・error・DPI・SSH/WSL・installerの
どの状態も製品PASSへ変換しない。未確定の実行path、placeholder、service commandをこの抽出文書から推測して
実行してはならない。

## Windows 配布時のライセンス通知

通常顧客向け配布は `win-x64` のself-contained client payloadを内蔵した、self-contained
single-file installerに固定する。clean supported Windowsで別途.NET Desktop Runtime、SDK、
Visual Studio、payload folder、build操作を要求してはならない。Windowsでlocked restore後、
repository既定のinstaller buildだけで最終setup executableを生成する。

installer buildのexact commandはWindowsのrepository rootから
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows-client\tools\Build-WindowsInstaller.ps1`
とする。この一つのbuildが、
locked restore、self-contained publish、Avalonia/Skia/HarfBuzz/ANGLE、埋め込みフォント、
rootの`THIRD_PARTY_NOTICES.md`と`LICENSES/`、実payloadへ入った.NET runtimeの
ライセンス・通知の収集、Inno Setupコンパイルを順に実行する。必要なruntime/packageの
通知が一つでも欠ける場合はbuildまたは配布gateをFAILとし、その成果物を顧客へ渡さない。
ライセンス一覧と版は[第三者ライセンス通知](../THIRD_PARTY_NOTICES.md)を参照する。
