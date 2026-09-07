> **文書の位置づけ:** 本書はWindows実装・運用の非規範的な案内です。製品要件は`PRODUCT_REQUIREMENTS.md`、wireは`REST_API_V1.md`、表示は`WINDOWS_UX_SPEC.md`、文言は`LOCALIZATION.md`が所有します。

# Windowsクライアント

## 構成

WindowsクライアントはAvalonia / .NET 10の別processで、Linux / WSLのresident serviceが公開する同じ表示generationを利用する。Windows UIの変更でcollector、SQLite schema、Session解析を変更しない。

| project領域 | 責務 |
| --- | --- |
| `CodexInfo.WindowsClient.Core` | REST/manifest取得、strict schema/header/domain検証 |
| `ViewModels` | Setup、接続、状態、Main/Graph/Threads/Settings/更新の表示projection |
| `Graphing` | 軸、系列、gap、labelの純粋projection |
| `Infrastructure` | WSL/SSH process、listener、設定、OS adapter |
| AXAML / Controls | binding、入力、描画、window lifecycle |

## 接続

```text
Windows UI -- HTTP / 127.0.0.1:8787 --> SSH local forwarding
                                                  |
                                                  v
Linux / WSL resident service -- 127.0.0.1:8787
```

WSL installed distribution token、またはOpenSSH configのliteral `Host` aliasを非秘密selectorとして保存できる。password、token、private key、展開済みhost/user/pathは保存しない。SSHはWindows標準`ssh.exe`をshellなしのArgumentListで起動し、auto reconnectは`BatchMode=yes`を使う。RESTをLANへbindせず、端末間の暗号化とpeer authenticationはSSHが担当する。

Setupはprofile準備、forward/listener、`GET /health`（`/v1/health`互換）、最初の完全なcurrent、必要なCodex認証の順で進む。healthはservice readinessとproduct versionだけを示し、認証済み、data ready、最新収集成功を意味しない。

## REST受理とversion差

現行Main rootは`GET /v3/current`一応答、Graphは`GET /v3/history/periods`と選択期間の`GET /v3/history`、Threadsは`GET /v3/threads`である。各取得cycleは同じpublished pairの完全集合だけをatomic commitする。Graph差分だけは、直前rootのopaque cursorが同periodの既取得sample/gap prefix不変をserverで証明した場合に限り、新pairの全pageを揃えて旧prefixへatomic appendする。過去補正時はcursor拒否後に先頭から再取得する。新resourceを持たない旧serviceが`/v3/current`へexact 404を返した場合だけ`GET /v3/details`、さらにexact 404の場合だけv2、v1へfallbackする。timeout、別status、schema/header/body不正ではfallbackせず、該当surfaceの直前完全rootを保持する。未証明の異なるpair、control結果、SQLite rowをmergeしない。

`/v3/current`がexact 404の接続はlegacy details modeとし、受理した一つのdetails rootをMain、Graph、Threadsへ同時投影する。このmodeではsplit history/threadsを要求せず、Mainの10秒周期でdetailsだけを更新する。Graph/Threadsは開いた時点の最新legacy rootを使い、再接続時に`/v3/current`から能力判定をやり直す。

Windows clientとLinux daemonのschema-validな異なるproduct versionは診断情報として保持し、それだけでsnapshot取得を停止しない。互換性はv3 strict validationと上記fallbackで決める。v1/v2と全体`/v3/details`は互換projection、v3 split resourceはASTRAと将来modelを含む有界な任意model配列である。正確なkey、型、OOM境界、cursor、published pair、effectは`REST_API_V1.md`だけを参照する。

## 表示と失敗

Main、Graph、Threadsは同じpublished domain generationから各surfaceに必要なresourceだけを使う。Mainは10秒、Graph差分はopen中60秒、Threadsはopen中5秒で確認し、閉じたsurfaceのresourceを取得しない。Setup、Main、Graph、Threads、Settings、Legalのsurface、状態優先順位、geometry、DPI、accessibility、Graphの実測/欠測/idle表現は`WINDOWS_UX_SPEC.md`を参照する。

- `auth_required`は旧account表示を同じroot updateで消去し、認証導線だけを表示する。
- transport、HTTP、schema、remote収集失敗は原因classを分け、last-goodがあればstaleとして保持する。
- 初回失敗では0や推測値を表示せず「未取得」と復旧操作を示す。
- ASTRAは独立modelで、cache writeを含む指定単価を使う。価格未定modelを0ドルへ確定しない。
- GraphはRemaining/SOL/TERRA/LUNA/ASTRAを操作できる。掲載modelの実測を集合不完全だけで予測線へ降格しない。

## 更新と導入

同じstable `windows-vX.Y.Z` Releaseの`CodexInfo.WindowsClient.Setup.exe`をユーザー単位で導入する。起動時は新しいstable Releaseを確認して通知だけを表示し、利用者が「更新する」を選んだ場合だけupdate manifestとSetupを検証して標準GUI Setupを起動する。silent install、自動適用、自動再起動は行わない。

LinuxとWindowsの更新時刻を原子的に同期する前提は置かない。API互換期間で旧/newの組合せを動かし、壊れたdaemon更新がWindowsの表示を永久停止させない。

## 検証

Core/ViewModel/Graphingの意味はWindows unitで確認する。process argv、設定、update transaction、installer、実window、UI Automation、DPIは、それぞれの境界を変更した場合またはRelease candidateで既存Windows harnessを使う。Linuxやmockの結果を実Windows PASSへ読み替えず、変更していない境界のE2Eを追加しない。

追跡master IDは`WIN-PARITY-DATA`、`WIN-PARITY-WIRE-01`、`WIN-PARITY-STATE`、`WIN-PARITY-UX`、`WIN-PARITY-OPS`、`WIN-PARITY-RECOVERY-01`、`WIN-PARITY-RETRY-01`、`API-V3-MODELS-01`、`API-DEPRECATION-01`である。
