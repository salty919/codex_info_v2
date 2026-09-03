# Codex Info Wiki

Codex Infoは、ChatGPT/Codexアカウントの利用枠、リセット時刻、モデル別使用量、履歴、実行中threadを表示する監視アプリです。Linux / WSL上の1つの常駐serviceがデータを収集し、LinuxのX Window UIとWindowsクライアントが同じ表示snapshotを利用します。

このWikiは利用者向けの入口です。製品要件を複製せず、詳細と最新状態はrepositoryの正本文書を参照します。

## 目次

- [導入と起動ガイド](https://github.com/salty919/codex_info_v2/wiki/導入と起動ガイド)
- [画面情報](https://github.com/salty919/codex_info_v2/wiki/画面情報)
- [開発・運用メモ](https://github.com/salty919/codex_info_v2/wiki/開発・運用メモ)

## 現在の構成

### Linux / WSL

- 通常導入は[最新のstable Release](https://github.com/salty919/codex_info_v2/releases/latest)にあるLinux bundleを使います。repository cloneとCargo buildは不要です。bundle内の`run.sh`は導入後のinstalled launcherと同じruntime入口です。
- bundleは`codex_info`、`codex-info.service`、installer、更新service/timer、license/noticeを含みます。
- 引数なしではdaemon+REST、`--ui`では同じserviceを利用するX Window UIを起動します。
- user-systemdの更新timerが導入5分後と以後1時間ごとに新しいstable Releaseを確認します。起動時と手動`--update`も同じresolverを実行し、検証や切替に失敗した場合は既存の導入状態とprofile dataを保持します。

### Windows

- 同じstable Releaseの`CodexInfo.WindowsClient.Setup.exe`からAvalonia / .NET 10クライアントをユーザー単位で導入します。
- Windowsクライアントは、WSLまたはOpenSSH configの`Host` aliasを使ったSSH local port forwardingでLinux側serviceへ接続します。
- RESTは`127.0.0.1`だけで待ち受けます。LANへ直接公開せず、端末間の暗号化と認証はSSHが担当します。
- 更新は起動時に通知するだけです。利用者が明示的に更新操作を選んだ場合だけ、検証後に標準Setupを起動します。

### 表示データ

- Codex App Serverから認証状態と利用枠を取得し、Codex session履歴からSOL / TERRA / LUNAのtokenと予想ドル額を集計します。
- Linux / WindowsのMain、Graph、Threadsは、常駐serviceが公開する同一の`GET /v1/details`応答を表示正本として使います。
- 定期更新の途中や取得失敗で確定済み表示を空や0に戻さず、最後の完全な表示を保持したまま失敗状態を示します。
- 認証情報、password、token、private keyはCodex Infoへ保存しません。Windows側に保存できる接続情報も非秘密selectorだけです。

## 画面

登録するtop-level surfaceはMain、Setup、Settings、Graph、Threads、Legalの6つです。Helpは独立WindowではなくMain内に表示します。Mainには製品versionを1回だけ表示し、初回の完全なsnapshotが揃うまでは固定レイアウト上にspinnerを表示します。

詳しい読み方は[画面情報](https://github.com/salty919/codex_info_v2/wiki/画面情報)を参照してください。

## 言語と時刻

固定UI文言のcatalogは日本語、英語、中国語（簡体）、韓国語、スペイン語、フランス語、ドイツ語、ポルトガル語、イタリア語、ロシア語の10言語です。未対応または不正なlocaleは英語へfallbackします。対応範囲と実装状態は[多言語化仕様](https://github.com/salty919/codex_info_v2/blob/main/docs/LOCALIZATION.md)を正本とします。

## 正本文書

- [README](https://github.com/salty919/codex_info_v2/blob/main/README.md) — 製品概要と通常導線
- [製品要件](https://github.com/salty919/codex_info_v2/blob/main/docs/PRODUCT_REQUIREMENTS.md) — 製品境界と受入条件
- [顧客運用手順](https://github.com/salty919/codex_info_v2/blob/main/docs/CUSTOMER_OPERATIONS_RUNBOOK.md) — install、service、更新、停止
- [REST API v1](https://github.com/salty919/codex_info_v2/blob/main/docs/REST_API_V1.md) — loopback APIとSSH接続
- [データ保護規約](https://github.com/salty919/codex_info_v2/blob/main/docs/DATA_PROTECTION_POLICY.md) — 履歴と失敗時の保持
- [Windowsクライアント](https://github.com/salty919/codex_info_v2/blob/main/docs/WINDOWS_CLIENT.md) — Windows配布・接続の設計と実装状態

## ライセンス

独自コードと文書はGPL-3.0-onlyです。正式な条件と第三者素材は[LICENSE](https://github.com/salty919/codex_info_v2/blob/main/LICENSE)、[LICENSE.ja.md](https://github.com/salty919/codex_info_v2/blob/main/LICENSE.ja.md)、[THIRD_PARTY_NOTICES.md](https://github.com/salty919/codex_info_v2/blob/main/THIRD_PARTY_NOTICES.md)、[LICENSES/](https://github.com/salty919/codex_info_v2/tree/main/LICENSES)を参照してください。
