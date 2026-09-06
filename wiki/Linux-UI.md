> **位置づけ:** 非規範的な索引です。製品境界は[`PRODUCT_REQUIREMENTS.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/PRODUCT_REQUIREMENTS.md)、表示意味は[`WINDOWS_UX_SPEC.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/WINDOWS_UX_SPEC.md)が所有します。

# Linux UI

## 起動と依存

通常入口はinstalled launcher `$HOME/.local/bin/codex-info --ui`です。repository clone、Cargo build、raw payload直起動は顧客導線ではありません。launcherはmanaged daemonを同じverified generationへ収束させてからX UIを開きますが、daemon/REST起動に失敗してもUIを即終了せず、接続失敗と再試行を表示します。

UIは`GET /v3/details`一応答だけを表示rootとして受理し、exact 404時だけv2→v1へfallbackします。SQLite、Session、app-serverを直接読まず、再収集・再計算・複数世代mergeをしません。

## surfaceと状態

Linux側の実装入口は`ui/app.slint`、`ui/components.slint`、`ui/theme.slint`と`src/main.rs`です。

- Main: service状態、quota、reset、任意model使用量、現在running thread概要。
- Graph: period、ドル/トークン、RemainingとSOL/TERRA/LUNA/ASTRAの系列。掲載modelの実測、不完全集合、欠測を混同しない。
- Threads: 同じdetails rootのrunning threadを親子関係・model・tokenとともに表示。
- 認証、取得失敗、stale、quota警告は別状態。通信失敗で確定済み表示を0やemptyへ戻さない。
- 明示logout/account switchだけが旧account表示を消去する。

ASTRAは独立した既知modelです。v3の未知model token事実も保存・Main詳細へ渡し、価格未定を0ドル確定値へ変えません。Graphの固定toggle集合と、API/DBの任意model集合は別の境界です。

## 直接oracle

master IDは`X-START-01..06`、`X-THREAD-01`、`CUM-138-06`、`API-V3-MODELS-01`です。状態・値はRustの対応case、描画境界を変更した場合だけ既存X11 visual gateで確認します。Wiki画像は参考であり、現行revisionの合格証拠ではありません。

## 未確認の扱い

current revisionの状態/value caseを確認していない状態は`INCONCLUSIVE`です。描画境界を変更した場合は同じcandidateのX11証拠が必要で、Wiki画像、起動したという事実、別revisionの画面をPASSへ読み替えません。
