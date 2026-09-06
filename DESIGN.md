<!-- Copyright (C) 2026 salty919 -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

> **文書の位置づけ:** 本書は非規範的な構造案内です。仕様の唯一の入口は`docs/PRODUCT_REQUIREMENTS.md`のowner registryです。詳細値、状態、schema、画面、testを本書で再定義しません。

# Codex Info 構造案内

## authorityと依存方向

| 境界 | 正本 |
| --- | --- |
| 製品、収集、起動、更新、配布、Workflow | `docs/PRODUCT_REQUIREMENTS.md` |
| REST route/schema/effect | `docs/REST_API_V1.md` |
| DB、transaction、backup、migration、retention | `docs/DATA_PROTECTION_POLICY.md` |
| Linux/Windowsの表示意味と操作 | `docs/WINDOWS_UX_SPEC.md` |
| locale、固定文言、時刻、font | `docs/LOCALIZATION.md` |
| trust boundary | `SECURITY.md` |

```text
Codex app-server + validated Session JSONL
              ↓
Linux resident service（collector / recorder / DB writer / REST publisher）
       ├─ profile・account単位のSQLite
       └─ immutable REST details generation
                 ├─ Linux Slint UI
                 └─ SSH local forwarding → Windows Avalonia UI
```

collectorとDB domainは特定UI、固定field、client versionへ依存しない。Linux/Windows UIはSQLite、Session、app-serverを直接読まず、strict validation済みの単一details generationをatomic rootとして表示する。現行は`GET /v3/details`で、旧serviceがexact 404を返した場合だけv2、さらにexact 404の場合だけv1へfallbackする。別応答、世代、DB rowをmergeしない。

## component

- `src/account_scope.rs`: profile/account/storage partition identity。
- `src/usage_store.rs`: SQLite schema、transaction、migration、retention、recovery。
- `src/server.rs`: loopback read-only REST v1/v2/v3 adapter。
- `src/main.rs`: resident orchestration、collector、publisher、Linux client projection。
- `ui/`: SlintのLinux Main/Graph/Threadsと共通theme/component。
- `windows-client/...Core`: REST取得とstrict validation。
- `windows-client/...ViewModels`: Windows状態遷移と表示projection。
- `windows-client/...Graphing`: Graphの純粋projection。
- `windows-client/...Infrastructure`、AXAML、Controls: OS/process/input/render adapter。
- `packaging/`: Linux bundle installer、launcher、systemd service/timer。

## dataと表示

resident serviceだけがquota、local model usage、history、running threadを収集して保存する。remote quota障害とlocal Session収集障害を分離し、一方の失敗で他方の記録を止めない。既存DB、cursor、last-good generationを保持し、0や推測値で欠損を隠さない。

v3はASTRAを含む有界な任意model配列を持つ。未知modelのtoken事実と価格未定を分ける。Graphの現行toggleはRemaining/SOL/TERRA/LUNA/ASTRAだが、DB/APIの任意model集合をこの固定表示集合へ縮退させない。掲載modelの実測、model集合の不完全性、model自体の未観測、confirmed gapを別状態として扱う。

状態、優先順位、Graph線幅・欠測表現、画面geometry、accessibilityはUX ownerを参照する。`ui/theme.slint`はLinuxのtheme token、`ui/components.slint`は共通部品、`ui/app.slint`はlayoutを所有する。platform adapterへ別の計算基準を複製しない。

## version

product、REST API、SQLite `user_version`、partition metadata、storage epoch、bundle/update manifest、Release tagは別のversion軸である。product version差だけでclientを停止せず、REST schema互換性で受理を決める。詳細はWikiの「版数管理」と各owner文書を参照する。

## 変更と検証

変更は`owner正本 → 要求台帳 → 実装 → 直接oracle`の順に行う。`scripts/ci_change_scope.py`が完全diffをownerへ分類し、`scripts/quality_plan.py`と`scripts/pre_pr_gate.sh`が影響範囲の既存checkだけを各1回選ぶ。変更していないplatform、全suite、全直積、PR上の実験を追加しない。詳細は`docs/REGRESSION_PREVENTION_POLICY.md`を参照する。
