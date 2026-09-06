# 回帰防止規約

本書は製品契約の正本ではない。正本は
[`PRODUCT_REQUIREMENTS.md`](PRODUCT_REQUIREMENTS.md) のowner registryに登録された文書だけである。
本書は、その正本から実装と直接オラクルへの最短経路を選び、回帰防止と過剰品質防止を
同時に行うための手順だけを定める。

## 変更順序

1. `PRODUCT_REQUIREMENTS.md`のowner registryから対象領域の唯一のowner文書とmaster IDを決める。
2. ownerの重複、未登録owner、参照先のないID、同じ観測結果の複数定義があれば、実装・build・test・
   agent reviewを開始せず、まず正本の矛盾を解消する。
3. 契約変更は `owner正本 → 要求台帳 → 実装 → 直接オラクル` の順に行う。古い台帳、test、
   README、運用文書へ実装を合わせない。
4. 高コストの確認前に `scripts/quality_plan.py` で完全な変更pathを分類する。分類できないpath、
   未知のcheck、重複check、影響領域外のcheckは実行前に停止する。
5. 選択されたcheckは共有呼出しをまとめ、同一revisionで各1回だけ実行する。失敗を確認なしのrerunで
   上書きせず、原因または外部状態が変わった新しい入力でだけ1回再実行する。

## 最小で十分な品質

- 影響するmaster IDごとに、その観測結果を直接判定できるオラクルを少なくとも1件持つ。
- 同じ観測結果のcheck ownerは1件にし、他のgateからtest名、呼出し数、実装文字列を二重監視しない。
- 変更のない製品ownerのtest、build、installer、実画面、実OS E2E、CodeQL言語は実行しない。
- test件数、coverage率、「念のため」、「安心のため」、全直積、routineのAI再評価はcheck追加の理由にしない。
- 実OS、installer、統合画面は、その境界を変更したとき、Release candidate、または記録済み障害経路の
  再現確認に限る。
- 独立agentは、機械的な直接オラクルで判定できない境界、security、または過去障害と同一の高risk経路に
  独立判断が必要な場合だけ使う。全変更へ一律に追加しない。

## 有限checkの責務

| check ID | 実行条件 | 唯一の判定責務 |
| --- | --- | --- |
| `requirements-authority` | owner正本、要求台帳、派生仕様の変更 | owner registry、master ID、台帳参照の一意性 |
| `governance-contract` | workflow、gate、selector、repository運用の変更 | 対象scriptの有限fixtureとsyntax |
| `rust-format` | Rust sourceまたはRust build inputの変更 | Rust書式だけ |
| `rust-test` | Linux backend/UIの観測可能な動作変更 | 影響master IDに登録したRust直接オラクル |
| `windows-contract` | Windows sourceまたはbuild inputの変更 | locked restore、format、Windows unitを一つの共有呼出しで各1回 |

check IDの追加は、既存checkでは観測できない独立した失敗境界と、そのmaster IDがある場合だけ許す。
既存checkと同じ結果を判定する場合は追加せず、既存ownerへ統合する。

## 失敗と証拠

- PASSは、実際に実行した直接オラクルの原始結果だけで示す。0件、SKIP、INCONCLUSIVEをPASSに変換しない。
- 外部環境でしか得られない証拠は、local gateの成功に混ぜず未確認として残す。
- Actionsの基盤・制御失敗はIssueへrevision、症状、原因、過剰checkの有無、再発防止を記録する。
- 回帰検出後は過去のPASSを現行成果物の証拠に流用せず、影響経路だけを新しいrevisionで1回再検証する。
- DB・Session・履歴を削除、再生成、推測補完して見かけ上回復させない。
