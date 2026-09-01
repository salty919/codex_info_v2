<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Codex Info 運用手順

## 起動構成

同一release binary `codex_info`は、次の公開操作だけを提供する。

| モード | command | 常駐所有者 |
| --- | --- | --- |
| daemon+REST | `codex_info` または `codex_info --port PORT` | Windowを生成せず、1 processがrecorder workerとloopback RESTを所有 |
| daemon+REST+X UI | `codex_info --ui` | healthyな既存serviceを再利用。なければserviceを1つ起動してX UIを追加 |
| 停止 | `codex_info --stop` | 同じprofileの検証済みlock ownerだけへTERMを送り、lock解放を待つ |
| ヘルプ | `codex_info --help` | daemon/REST/UIを起動せず、locale別の契約を表示 |

`--port`はnumeric portだけを受理し、待受アドレスは`127.0.0.1`に固定する。
引数なしは常にWindowを生成しないdaemon+REST modeである。待受addressは指定できず、常に`127.0.0.1`である。
`--help`/`--h`/`-h`は起動せず、環境のロケールに応じたモード一覧を表示する。

bundleから導入したbinaryを直接起動する場合は、同じ引数を`$HOME/.local/bin/codex_info`へ渡す。

```bash
"$HOME/.local/bin/codex_info"
"$HOME/.local/bin/codex_info" --port 9876
"$HOME/.local/bin/codex_info" --ui
"$HOME/.local/bin/codex_info" --ui --port 9876
"$HOME/.local/bin/codex_info" --stop
"$HOME/.local/bin/codex_info" --help
```

## Linux bundleの導入とuser-systemd

公開済み・stableな`windows-vX.Y.Z` Releaseから、同じversionのLinux asset一式を取得する。
Release全体は次のWindows 2 assetとLinux 3 assetのexact 5 assetだけを持つ必要がある。
`CodexInfo.WindowsClient.Setup.exe`、`CodexInfo.WindowsClient.update.json`、
`x86_64-unknown-linux-gnu` archive、対応する`.sha256` checksum、対応する`*.manifest.json`である。
draft、prerelease、partial、extra、malformed Release、導入済みversion以下のReleaseは使わない。
導入対象はmanifestの実測`glibc_minimum`以上のglibcを持つ環境だけとし、他のdistribution/architecture、
署名済み、publisher検証済みとは表明しない。
導入には`bash`、`tar`、`sha256sum`、`python3`、`curl`、`flock`、user-systemd（`systemctl --user`）が必要である。

### downloadとchecksum検証

Release画面で、次の名前のassetを同じ`windows-vX.Y.Z` Releaseからダウンロードする。
`X.Y.Z`はReleaseのversionへ置き換える。

```bash
version='X.Y.Z'
asset="codex-info-${version}-x86_64-unknown-linux-gnu.tar.gz"
checksum="${asset}.sha256"
manifest="${asset%.tar.gz}.manifest.json"
bundle_dir="$HOME/.local/share/codex-info/bundle-v${version}"
mkdir -p "$bundle_dir"
# 上記ReleaseからLinuxの3 assetをbundle_dirへ置いてから続行する。
(cd "$bundle_dir" && sha256sum -c "$checksum")
test -s "$bundle_dir/$manifest"
```

checksum、manifest、archive contentの検証が失敗した場合は展開・導入を行わず、取得したassetを削除して
Releaseから再取得する。検証できないReleaseを採用せず、既存の導入状態も変更しない。

### extractとinstall

```bash
mkdir -p "$bundle_dir/extracted"
tar -xzf "$bundle_dir/$asset" -C "$bundle_dir/extracted"
bash "$bundle_dir/extracted/install.sh" \
    --bundle "$bundle_dir/$asset" \
    --manifest "$bundle_dir/$manifest" \
    --sha256 "$bundle_dir/$checksum"
```

bundle内のscriptはrelease binaryを`$HOME/.local/bin/codex_info`へ置き、installerを
`$HOME/.local/libexec/codex-info-install.sh`へ永続化し、installed manifestを
`$HOME/.local/share/codex-info/manifest.json`へ置く。さらに
`codex-info.service`、`codex-info-update.service`、`codex-info-update.timer`を配置する。
`codex-info.service`を有効化・再起動し、update timerはboot時と毎日1回、固定repository
`salty919/codex_info_v2`の公開Releaseだけを確認する。導入後は元の展開ディレクトリやrepositoryを必要としない。

### statusとhealth

```bash
systemctl --user status codex-info.service --no-pager
curl --fail http://127.0.0.1:8787/v1/health
```

`health`の応答versionがReleaseの`X.Y.Z`と一致しない場合は利用を開始せず、導入をやり直す。

### 自動更新と手動確認

timerは導入済みversionより新しいstableなReleaseだけを候補にし、exact 5 assetのidentity、checksum、manifest、
archive contentを検証してから既存のatomic install、service restart、health確認へ進む。新版がない、draft/prerelease、
partial、extra、malformed、identity不一致、検証失敗、service切替失敗、health不一致のいずれでもインストール状態を
変更せず、途中fileを残さない。切替後のhealth失敗を含む更新失敗時は旧binary、旧unit、
`$HOME/.local/libexec/codex-info-install.sh`、profile dataへrollbackする。

今すぐ確認する場合は次を実行する。

```bash
systemctl --user start codex-info-update.service
systemctl --user status codex-info-update.service --no-pager
systemctl --user status codex-info-update.timer --no-pager
journalctl --user -u codex-info-update.service --no-pager
```

最後のコマンドで更新確認、候補拒否、適用、rollbackの結果を確認できる。journalへ秘密やraw responseは記録しない。

## daemon自動起動・自動更新から外す

```bash
bash "$bundle_dir/extracted/install.sh" --remove
```

この操作は`codex-info.service`と`codex-info-update.service`/`.timer`を停止・無効化し、unit fileを削除する。
導入binary、`$HOME/.local/libexec/codex-info-install.sh`、installed manifest、profile dataと次のデータは削除しない。

- `history/usage_history.sqlite3`
- DB backup
- `history/usage_reset_hint.json`
- Codex session JSONL
- 設定

履歴データ自体の削除は、このbundle removeとは別の明示操作として扱う。
install、update、reinstall、service切替失敗でも、履歴DB、DB backup、reset hint、Codex session JSONL、設定は保持する。

## 停止と確認

```bash
"$HOME/.local/bin/codex_info" --stop
systemctl --user stop codex-info.service
systemctl --user is-active codex-info.service
curl --max-time 1 http://127.0.0.1:8787/v1/health
```

正常停止ではREST listenerを閉じ、recorder workerを停止し、singleton lockを解放する。
停止中の未取得値を推測・補間せず、既存DBとlast-good値を保持する。

## Windowsクライアント

WindowsクライアントはWSL/Ubuntu側のserviceへSSH local port forwarding経由で接続する。
X UIを併用する場合もserviceを増やさず、`--ui`を起動する。
保持期間、1回の取得上限、REST SLOは[REST API v1](REST_API_V1.md)と
[データ保護規約](DATA_PROTECTION_POLICY.md)を正本とする。
