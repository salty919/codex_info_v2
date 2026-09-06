> **位置づけ:** 非規範的な索引です。製品境界は[`PRODUCT_REQUIREMENTS.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/PRODUCT_REQUIREMENTS.md)、画面正本は[`WINDOWS_UX_SPEC.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/WINDOWS_UX_SPEC.md)、接続実装案内は[`WINDOWS_CLIENT.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/WINDOWS_CLIENT.md)です。

# Windows UI

## 接続境界

Windows clientはAvalonia / .NET 10の別processです。WSL profileまたはOpenSSH configのliteral `Host` aliasからSSH local forwardingを作り、Linux daemonのloopback `127.0.0.1:8787`へ接続します。password、token、private key、展開済みhost/user/pathを保存しません。

healthのproduct versionは診断情報です。Windows clientとLinux daemonのversionが異なっても、それだけで停止せず、v3 strict validationとexact 404時のv2→v1 fallbackで互換性を判定します。APIが不正または取得不能ならlast-goodを保持し、接続/応答/認証/取得の状態を分けます。

## surface

| surface | 責務 |
| --- | --- |
| Setup | profile選択、listener、health、最初のdetails、必要な認証 |
| Main | 状態、quota、reset、任意model使用量、running thread概要 |
| Graph | period、metric、Remaining/SOL/TERRA/LUNA/ASTRA、欠測区間 |
| Threads | 同じrootのrunning threadと親子関係 |
| Settings | 非秘密selectorと接続復旧 |
| Legal / Help | license・第三者通知、運用案内 |

`Core`はREST受理、`ViewModels`は状態と表示意味、`Graphing`は純粋projection、`Infrastructure`/AXAML/ControlsはOS・入力・描画adapterを所有します。Windows UIの変更をcollectorやDB schemaへ波及させません。

## 更新

Windowsは起動時にstable Releaseを確認し、通知だけを表示します。利用者が更新を選んだ場合だけmanifestとSetupを検証し標準GUI Setupを起動します。Linux daemonの更新タイミングと原子的に同期する前提を置かず、API互換期間で連続動作させます。

## 直接oracle

master IDは`WIN-PARITY-DATA`、`WIN-PARITY-WIRE-01`、`WIN-PARITY-STATE`、`WIN-PARITY-UX`、`WIN-PARITY-OPS`、`WIN-PARITY-RETRY-01`、`CUM-138-06`です。実装入口は`windows-client/src/`のCore、ViewModels、Graphing、Infrastructure、ControlsとAXAMLです。Core/ViewModel/GraphingはWindows unit、installer・実window・UIAはRelease candidateまたはその境界変更時の実Windows E2Eで確認します。

## 未確認の扱い

current revisionのCore/ViewModel/Graphing caseを確認していない状態は`INCONCLUSIVE`です。installer・window・UIA境界を変更した場合はexact candidateの実Windows証拠が必要で、Linux PASSや過去installerの結果で代替しません。
