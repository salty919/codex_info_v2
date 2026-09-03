<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Codex Info 運用手順

## 起動構成

通常運用はinstalled launcher `$HOME/.local/bin/codex-info`だけを使う。launcherは起動前に固定Releaseと
local generationを同じresolverで照合し、検証済みのmanaged serviceへ収束してから操作を完了する。

| 操作 | command | 結果 |
| --- | --- | --- |
| daemon開始 | `codex-info` または `codex-info --start` | reconcile後、main serviceを有効化・起動 |
| Linux UI | `codex-info --ui` | reconcile後、同じgenerationのpayload UIを実行 |
| 即時更新 | `codex-info --update` | timerを待たず同じresolverを1回実行 |
| 整合確認 | `codex-info --status` | version/source/manifest、systemd owner、lock、socket、healthの完全tupleをread-only確認 |
| 今回のbootだけ停止 | `codex-info --stop` | main serviceだけ停止。timerと次回bootの自動起動は維持 |
| 自動起動も停止 | `codex-info --disable-autostart` | main serviceとupdate timerを停止・無効化 |
| unit解除 | `codex-info --remove` | main/update unitを解除し、programとprofile dataは保持 |
| ヘルプ | `codex-info --help` | 状態を変更せず操作一覧を表示 |

installed launcherへ`--port`、未知・重複・混在した引数を渡すと、service、filesystem、DBを変更する前に失敗する。
待受はmanaged serviceの`127.0.0.1:8787`に固定する。`codex_info --port PORT`等のraw payload CLIは
service・開発・E2E用であり、顧客の起動・停止・更新authorityではない。

```bash
"$HOME/.local/bin/codex-info" --start
"$HOME/.local/bin/codex-info" --ui
"$HOME/.local/bin/codex-info" --status
"$HOME/.local/bin/codex-info" --help
```

## Linux bundleの導入とuser-systemd

公開済み・stableな`windows-vX.Y.Z` Releaseから、同じversionのLinux asset一式を取得する。
Release全体は次のWindows 2 assetとLinux 3 assetのexact 5 assetだけを持つ必要がある。
`CodexInfo.WindowsClient.Setup.exe`、`CodexInfo.WindowsClient.update.json`、
`x86_64-unknown-linux-gnu` archive、対応する`.sha256` checksum、対応する`*.manifest.json`である。
draft、prerelease、partial、extra、malformed Release、導入済みversionより古いReleaseは使わない。
同versionはinstalled generationが完全ならno-op、不整合なら同じRelease assetによるverified repairだけに使う。
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

bundle内の`run.sh`はrepository版とbyte-identicalなruntime launcherで、Cargo buildを行わない。installerは
`$HOME/.local/share/codex-info/generations/<version>-<source_sha>-<manifest_sha256>/`へ検証済みregular fileを置き、
atomicな`current` symlinkからlauncher `$HOME/.local/bin/codex-info`、payload
`$HOME/.local/bin/codex_info`、persistent installer `$HOME/.local/libexec/codex-info-install.sh`、manifest
`$HOME/.local/share/codex-info/manifest.json`、
`codex-info.service`、`codex-info-update.service`、`codex-info-update.timer`を参照させる。
導入後は元の展開directory、repository、Cargo、`target/`を必要としない。

generation/current導入前の1.0.25以前のflat配置から更新する場合も、旧manifestとそのbinary、installer、3 unitが
記録済みsize/SHA-256、固定path、owner/modeへ完全一致する場合だけbootstrapする。最初の更新中に新serviceが
起動しても同じinstall lockを待たず、lock解放後の最初のlauncher/startup/timer操作で同じ公開Release archiveを
再検証してgeneration配置へ一方向移行する。旧fileが不明・変更済みならrepository buildで埋めず`SAFE_BLOCKED`とする。

既に導入済みの旧flat版自身が持つ日次timerは、新版を取得する前には遡及して短縮できない。その1回限りの
bootstrapは、上記の新版bundleを手動導入する方法を優先する。特にmanaged serviceがinactiveで旧unmanaged
listenerだけが残る既知障害状態では、変更不能な旧updaterを再実行せず、新版bundleのinstallerを直接使う。
旧managed serviceが正常またはlistener不在で、旧版のpersistent updaterから取得する場合だけ、
新版Release公開後に次を1回実行する。同じ入力の失敗を反復せず、journalを確認する。

```bash
systemctl --user start codex-info-update.service
systemctl --user status codex-info-update.service --no-pager
"$HOME/.local/libexec/codex-info-install.sh" --update
"$HOME/.local/bin/codex-info" --status
```

1行目は旧flat版から新版を取得するbootstrap例外、3行目は取得済みの新版installerでgeneration配置へ収束する操作である。
移行後の通常操作ではraw `systemctl`を使わずinstalled launcherを使う。

### statusとhealth

```bash
"$HOME/.local/bin/codex-info" --status
```

`--status`はmanifestと全installed member、systemd MainPID、process starttime/executable、
profile lock、port 8787のsocket FD、前後health、fresh recorder state、`/v1/details`が同じgenerationへ結合し、
detailsが`ready`または未認証時の`auth_required`で正の`observed_at`を持つ場合だけ成功する。
PID、listener、health 200、version文字列のいずれかだけ、またはdetailsが`initializing`/`error`の状態では成功しない。

### 自動更新と手動確認

timerは導入5分後に開始し、その後は1時間ごと（accuracy 1秒）に固定repositoryを確認する。
launcher起動、serviceの`ExecStartPre`、timer、手動更新は同じresolverを使うため、停止中に公開された新版も
次の起動時に確認する。highest stable exact-five Releaseへだけ進み、downgradeしない。同versionでもlocal memberが
manifestと不一致ならverified repairする。新版なし・同一でlocalが完全ならno-opでmanagedかつfunctionally readyな状態を再確認する。

今すぐ確認する場合は次を実行する。

```bash
"$HOME/.local/bin/codex-info" --update
"$HOME/.local/bin/codex-info" --status
systemctl --user status codex-info-update.timer --no-pager
journalctl --user -u codex-info-update.service --no-pager
```

更新全体は20分、launcher/startupは20分30秒以内に必ずterminalになる。検証済み新版をmanagedかつfunctionally readyにするか、
検証済み旧版をmanagedかつfunctionally readyへ戻した場合だけ完了である。unknown/foreign/malformed listener/lockは停止せず、
30秒以内に明示的`SAFE_BLOCKED`となる。この状態は成功ではないが、次のmanual/startup/timer実行を妨げない。
journalにはboundedな更新結果だけを記録し、秘密やraw responseを記録しない。

## daemon自動起動・自動更新から外す

```bash
"$HOME/.local/bin/codex-info" --disable-autostart
"$HOME/.local/bin/codex-info" --remove
```

`--disable-autostart`はunitを残したままmain serviceとtimerを停止・無効化する。`--remove`はmain/update unitだけを
解除する。どちらもinstalled generation、launcher、persistent installer、manifest、profile dataと次のデータは削除しない。

- legacy `history/usage_history.sqlite3`（read-only保持対象）
- account別`history/accounts/v1/<opaque-account-scope>/epoch-<storage-epoch>/usage_history.sqlite3`
- `history/account_profile_v1.json`
- DB backup
- `history/usage_reset_hint.json`
- Codex session JSONL
- 設定

履歴データ自体の削除は、このbundle removeとは別の明示操作として扱う。
install、update、reinstall、service切替失敗でも、履歴DB、DB backup、reset hint、Codex session JSONL、設定は保持する。
通常稼働中のresident serviceは、最新2GiBの収集対象から外れ、かつ同一fingerprintの利用量がSQLiteへ記録済みで、変更・open中でない古いsession JSONLだけを整理できる。このruntime整理はbundle removeとは別であり、未記録・legacy・変更済み・active sessionや履歴DBを削除しない。

account切替直後は、既存Sessionを現在EOFへbaselineしてから新しいappendだけを現accountへ記録するため、detailsが一時的に`initializing`のempty rootになる。`auth_required`は明示logout、`error`のempty rootは`auth.json`、profile metadata、account DBの安全性を確認できない状態である。後者では旧account DBへ戻さず、通常の次cycleまたはservice再起動で同じpartitionの回復を試す。`history/account_profile_v1.json`、`history/accounts/v1`、legacy DB、backup、Sessionを手作業で削除・rename・mergeして回復させてはならない。

## 停止と確認

```bash
"$HOME/.local/bin/codex-info" --stop
"$HOME/.local/bin/codex-info" --status
```

`--stop`は同一bootの停止意図をowner-only control stateへ記録し、main serviceだけを停止する。timerとenable状態は
維持され、次回bootは再びrunningをdesired stateとする。raw `systemctl --user stop`は永続的な製品停止意図ではなく、
次のupdateでmanaged runningへ正規化され得る。停止中の未取得quota/残量を推測・補間せず、既存DBとlast-good値を保持する。
再開後はverified Session rangeだけをbounded backfillし、回収不能区間だけをconfirmed gapとして公開する。

## Windowsクライアント

WindowsクライアントはWSL/Ubuntu側のserviceへSSH local port forwarding経由で接続する。
X UIを併用する場合もserviceを増やさず、`--ui`を起動する。
保持期間、1回の取得上限、REST SLOは[REST API v1](REST_API_V1.md)と
[データ保護規約](DATA_PROTECTION_POLICY.md)を正本とする。
