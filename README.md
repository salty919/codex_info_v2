<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

> **文書の位置づけ:** 本書は製品要件の非規範的な案内／実装説明です。要求の入口とowner registryは `docs/PRODUCT_REQUIREMENTS.md` および同文書から参照される REST/DATA/UX/LOCALIZATION 仕様です。要求変更時は該当する master ID と owner に従い、本書だけで契約や監査成果物を追加・変更しません。

# Codex Info（Linux bundle / X Window / WSLg）

English quick start: [README.en.md](README.en.md) · [製品要件](docs/PRODUCT_REQUIREMENTS.md) · [Windowsクライアント](docs/WINDOWS_CLIENT.md) · [REST API](docs/REST_API_V1.md) · [データ保護](docs/DATA_PROTECTION_POLICY.md) · [多言語化](docs/LOCALIZATION.md) · [顧客運用](docs/CUSTOMER_OPERATIONS_RUNBOOK.md)

Codex App ServerからChatGPT/Codexアカウントのレート制限と週次または月間のリセット時刻を取得し、WSLのX Windowに表示します。UIはRustの宣言的GUIツールキットSlintで構成しています。必要な場合だけ、SSHトンネル経由のWindows監視用にloopback限定の読み取り専用REST APIも開始できます。

## Linux bundleで導入・起動（通常導線）

公開済み・stableな`windows-vX.Y.Z` Releaseから、同じversionのLinux asset一式をダウンロードします。
Release全体がWindows 2 assetとLinux 3 assetのexact 5 asset（`CodexInfo.WindowsClient.Setup.exe`、
`CodexInfo.WindowsClient.update.json`、`x86_64-unknown-linux-gnu` archive、対応する`.sha256` checksum、
対応する`*.manifest.json`）だけを持つことを確認してください。draft、prerelease、partial、extra、
malformed Releaseや導入済みversionより古いReleaseは使いません。同versionはlocal generationのverified repairにだけ使います。
`X.Y.Z`はReleaseのversionへ置き換えてください。

```bash
version='X.Y.Z'
asset="codex-info-${version}-x86_64-unknown-linux-gnu.tar.gz"
checksum="${asset}.sha256"
manifest="${asset%.tar.gz}.manifest.json"
bundle_dir="$HOME/.local/share/codex-info/bundle-v${version}"
mkdir -p "$bundle_dir"
# 上記ReleaseからLinuxの3 assetをbundle_dirへ置く（Windows 2 assetはRelease全体のidentity確認用）。
(cd "$bundle_dir" && sha256sum -c "$checksum")
test -s "$bundle_dir/$manifest"
mkdir -p "$bundle_dir/extracted"
tar -xzf "$bundle_dir/$asset" -C "$bundle_dir/extracted"
bash "$bundle_dir/extracted/install.sh" \
  --bundle "$bundle_dir/$asset" \
  --manifest "$bundle_dir/$manifest" \
  --sha256 "$bundle_dir/$checksum"
"$HOME/.local/bin/codex-info" --status
"$HOME/.local/bin/codex-info" --ui
```

targetは`x86_64-unknown-linux-gnu`に固定です。互換性として表明するのはmanifestの実測
`glibc_minimum`以上だけで、他のdistributionやarchitectureへの対応、署名済み、publisher検証済みとは表明しません。
checksum、manifest、archive content、install、service切替、source-bound healthのいずれかが失敗した場合は利用を開始しません。
既存世代が安全ならmanaged+healthyへrollbackし、local世代も安全でなければ明示的`SAFE_BLOCKED`になります。

archiveにはrepository版とbyte-identicalな`run.sh`が入り、installed launcher
`$HOME/.local/bin/codex-info`になります。このlauncherはCargo buildやrepository/`target` fallbackを行いません。
検証済みgeneration、persistent installer、manifest、`codex-info-update.service`/`codex-info-update.timer`も配置されます。
timerは導入5分後と以後1時間ごとに、固定repository `salty919/codex_info_v2`の公開Releaseだけを確認します。
起動時と手動更新も同じresolverを使い、新版へ収束します。同versionでもlocal memberが壊れていればverified repairし、
導入済みversionより古いReleaseへはdowngradeしません。

更新を今すぐ確認する場合は、展開済みbundleやrepositoryを再取得せず、次を実行します。

```bash
"$HOME/.local/bin/codex-info" --update
"$HOME/.local/bin/codex-info" --status
journalctl --user -u codex-info-update.service --no-pager
```

今回のbootだけ停止する場合、自動起動も停止する場合、unitを解除する場合は次を使い分けます。

```bash
"$HOME/.local/bin/codex-info" --stop
"$HOME/.local/bin/codex-info" --disable-autostart
"$HOME/.local/bin/codex-info" --remove
```

`--remove`もmain/update unitだけを外し、installed generation、launcher、installer、manifest、
履歴DB、DB backup、reset hint、gap/recorder/control state、Codex session JSONL、設定は削除しません。
launcherの公開操作は引数なし/`--start`、`--ui`、`--update`、`--status`、`--stop`、
`--disable-autostart`、`--remove`、`--help`だけです。managed serviceのportは127.0.0.1:8787固定です。
Linux / Windows UIは、現行の製品要件に従い、同じproduct versionのresident serviceが公開する単一generationの`GET /v1/details`を表示します。version整合とAPIの受理条件は、`docs/PRODUCT_REQUIREMENTS.md`から参照されるREST仕様に従います。

初回起動時の画面内タイトルは`Codex Info`です。ネイティブタイトルバーは使わず、アプリ内では認証パネルが接続状態を案内します。

1. 「認証を開始」を押す
2. 「認証ページを開く」を押す
3. ブラウザでOpenAIアカウントにログインし、Codexへのアクセスを許可する
4. X Windowに戻り、「認証状態を確認」を押す

このアプリにユーザー名、パスワード、APIキーを入力する必要はありません。認証処理とトークン保存はCodex CLIに任せます。認証済みなら起動時に自動接続します。

## 必要環境

- `x86_64-unknown-linux-gnu`を実行でき、bundle manifestの実測`glibc_minimum`以上を持つLinux環境
- `bash`、`tar`、`sha256sum`、`python3`、`curl`、`flock`、user-systemd（`systemctl --user`）
- WSLgまたはXサーバー（`DISPLAY`が設定されていること）
- `codex` CLI（`codex app-server --stdio`が実行できること）

日本語・韓国語表示用フォントは`assets/`に同梱してSlintへ埋め込み、ホスト側フォントに依存しません。著作権とライセンスは[第三者ライセンス通知](THIRD_PARTY_NOTICES.md)と[assets/NOTICE.txt](assets/NOTICE.txt)に記載します。

画面が出ない場合は次を確認してください。

```bash
echo "$DISPLAY"
codex login status
codex app-server --help
```

## 取得する情報

アプリはCodex App ServerへJSON-RPC接続し、次のAPIを利用します。

- `account/read` — 認証状態とアカウント情報
- `account/login/start` — 未認証時のChatGPTログイン開始
- `account/rateLimits/read` — 使用率とリセット時刻
- 内部の`thread/list`／`rollout`収集 — resident serviceが検証済みthread snapshotを構成する実装手段。UIは公開済み`GET /v1/details`の値だけを使う。

取得したトークンやパスワードはアプリのファイルへ保存しません。Codex側の認証ストアが管理します。

## データ保護と変更ゲート

履歴DBの一意性、複数collector、app-server停止時の復旧、3世代バックアップ、schema mismatch、migration、prune、障害時の保持境界は[データ保護規約](docs/DATA_PROTECTION_POLICY.md)に従います。変更時のローカル確認には`bash scripts/pre_pr_gate.sh`を使えますが、要求のownerや完了判定は`docs/PRODUCT_REQUIREMENTS.md`と参照先仕様に従います。

## Windowsからのイントラネット監視

Linux / WSL 側でネイティブ画面を維持したまま、Windows クライアントから監視する場合は、`windows-client/` の Avalonia クライアントと loopback 限定 API、SSH ローカルポート転送を使用します。Windows配布物はInno Setup 7.1.0の標準GUIウィザードを使う `CodexInfo.WindowsClient.Setup.exe` とし、非管理者のユーザー単位インストール、更新、Startメニュー、「インストールされているアプリ」登録、標準アンインストールを提供します。本体、Setup、shortcut、uninstallerには同じ製品アイコンを設定します。インターネット公開や HTTPS 証明書の設定は必要ありません。Visual Studio Communityでのbuild、インストーラー作成、SSH接続、API契約、Windows側の表示仕様は[Windows 監視クライアント](docs/WINDOWS_CLIENT.md)を参照してください。

## 表示と更新

### 言語・時刻・日本語表示

- 固定UI文言は起動時に`LC_ALL` → `LC_MESSAGES` → `LANG`の順で選び、日本語、英語、中国語（簡体）、韓国語、スペイン語、フランス語、ドイツ語、ポルトガル語、イタリア語、ロシア語を表示します。`C`、`POSIX`、未対応localeは英語です。
- localeとIANA timezoneはプロセス起動時に確定します。絶対時刻・履歴期間・グラフ横軸はそのtimezone、経過時間・残り時間はUTC差分で表示します。`TZ`にはIANA IDを指定でき、無効値はUTCとして扱います。
- 固定文言のcatalog、locale境界、時刻表示の仕様は[多言語化仕様](docs/LOCALIZATION.md)にまとめています。

- 残り時間は日・時・分で読みやすく表示
- レート制限は1分ごとに再取得
- 定期再取得のquotaは中間イベントとして扱い、次のローカル集計が完了するまで前回確定済みのモデル・履歴・thread表示を保持します。rollingする`reset_at`の時刻差だけで画面を初期化しません。
- ネイティブタイトルバーは使わず、各Windowの画面内タイトル領域に埋め込みフォントの見出しと自前の移動・最小化・閉じる操作を配置します。固定Windowには最大化操作を表示しません。
- 認証済み画面の「グラフ」ボタンから1つのグラフウインドウを開きます。残り利用枠・LUNA/TERRA/SOLは凡例で個別に表示／非表示を切り替え、非表示系列は色とラベルのコントラストを下げて示します。初期状態は全系列表示です。各モデルの入力・キャッシュ・出力合計を独立した累積ラインで描き、表示中モデルの個別最大値をドル軸へ使います。全モデルの累積値が変化しない未使用区間は残量ラインを水平保持し、プロット下地の薄い帯で示します。モデルが進んでいるのに残量サンプルが同じ区間は、前後を実測された残量低下値で挟める場合だけ欠測サンプルとして線形補間し、`1→1→3` は `1→中点→3` として折返しや瞬間的な消費を描きません。次の実測値がない終端は最後の実測残量を保持します。右端の値は系列色のリーダー線で終端へ結びます。リセット直後（0）から現在時刻まで表示します
- グラフの1分サンプルはSQLite（`history/usage_history.sqlite3`）へ過去3暦月分保存します。同一リセット期間・同一分は最大値を保持して再計測で減少しません。通常起動で削除されるのは3暦月より古い行だけです。1回の取得・REST応答・グラフ表示はその保持データ中の最長1暦月（最大44,640分点）に限定し、DB全体を読みません。グラフ上部の「ドル／トークン」で、ドルは累積額、トークンは各モデルの時間帯別使用量へ切り替えられます（初期値はドル）。モデル使用量と残量は別の観測値です。使用後に遅れて届いた残量観測は反映しますが、残量観測が無い区間を料金から推測しません。
- `CODEX_INFO_DATA_DIR`を指定すると、そのディレクトリ配下へ履歴を保存します
- 週次または月間の対象期間の残り時間は、端数も含めた7セルのゲージで表示
- `~/.codex/sessions`に履歴がある場合は、週次または月間の対象期間を表示し、その期間内のSOL/TERRA/LUNAの入力（非キャッシュ）・キャッシュ入力・出力トークン数と、[OpenAI Developer Docsのモデル料金表](https://developers.openai.com/api/docs/models)に基づく予想ドル額（整数部のみ）を各カテゴリの独立したドル列に表示します。見出しはモデル・入力・キャッシュ・出力だけです。クレジット換算は行いません。連続する同一累積スナップショットは差分0として二重計上しません。その他のモデルは表示しません。
- 履歴は直近3カ月を保持し、グラフの期間履歴listから過去のリセット期間を選択できます。最新の実行中スレッドは全件を表示し、`last_token_usage.total_tokens`と`model_context_window`から算出した現在コンテキスト使用率と、累積tokenを`使用率% / 上限トークン`で併記します。Threads画面は親を先に置くdepth-first・subtree-contiguous順へ投影し、role/depth/orphanをtree guideで示します。
- プランはschema検証済みアカウント情報から判定します。Enterpriseの`individualLimit`は月間枠として扱い、`unlimited`は固定上限なしとして表示します。認証状態や固定月間上限を、レスポンスにない情報から推測しません。
- リセット前後24時間は状態バナーで明示
- 認証失敗・Codex未起動などは画面にエラー表示
- 画面右上の「法的通知」から、GPLの無保証条項と第三者素材のライセンスを確認できます。ソース配布物の`LICENSE`、`THIRD_PARTY_NOTICES.md`、`LICENSES/`も併せて保持してください。

## Windowサイズとプレビュー

登録top-level surface inventoryはMain、Setup、Settings、Graph、Threads、Legalの正確な6個で、HelpはMain内surface（追加HWND=0）です。runtime open HWNDはMain=1＋open child subset 0..5、合計1..6で、各childはsingleton、5 childを全て開いた時だけ6となります。Main/Setup/Settings/Threads/Legalはlogical client `initial=min=max=900×480` fixed、Graphは`initial=940×640`、`min=700×480`、`max=unbounded`、resizableです。登録された6 surfaceはMinimize/Closeを持ち、native resize/maximize/restoreはGraphだけです。ネイティブタイトルバーは全Windowで無効にし、ボタン以外の画面領域をドラッグして移動できます。物理サイズはOSのDPI／拡大率に連動し、Graphの最大化／復元は現在モニターのwork areaへ適用します。状態別の開発確認には`CODEX_INFO_PREVIEW=initializing|auth|normal|warning|reset-warning|error|zero|full|monthly|unlimited|idle|legal`を使い、グラフ表示は`CODEX_INFO_PREVIEW=graph|graph-old`で確認できます。`CODEX_INFO_PREVIEW_SIZE`はGraphの初期サイズを上書きするレイアウト検証用です。source treeでの例は`CODEX_INFO_PREVIEW=normal cargo run --locked -- --ui`であり、顧客向けlauncher `run.sh`はbuildに使いません。

## UIを調整する場所

画面の余白・色・文字サイズ・角丸は`ui/theme.slint`に集約しています。ヘッダー、利用枠、期間ゲージ、状態バナー、認証パネルは`ui/components.slint`の再利用部品です。レイアウトを変更するときはRustコードを座標調整せず、これらのトークンまたは部品のプロパティを変更してください。

## ライセンス

Copyright (C) 2026 salty919

別途明記された第三者素材を除き、このリポジトリの独自コードと文書は[GNU General Public License version 3](LICENSE)（SPDX: `GPL-3.0-only`）で提供します。英語の[LICENSE](LICENSE)がGPLv3の正文であり、正式な条件・定義・免責を定めます。[LICENSE.ja.md](LICENSE.ja.md)は日本語案内です。著作権一覧は[COPYRIGHT](COPYRIGHT)を参照してください。

同梱フォント、Codex CLIから生成したプロトコルスキーマ、Slint、Rust依存クレート、およびWindowsクライアントの Avalonia / NuGet 依存は各上流ライセンスで提供します。ソース・バイナリ配布物には[第三者ライセンス通知](THIRD_PARTY_NOTICES.md)、[assets/NOTICE.txt](assets/NOTICE.txt)、[LICENSES/](LICENSES/)を同梱します。Windows publish 時は[Windows 監視クライアント](docs/WINDOWS_CLIENT.md)の通知収集手順も実行してください。
