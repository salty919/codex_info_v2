# 回帰防止規約

本書は製品契約の正本ではない。正本は
[`PRODUCT_REQUIREMENTS.md`](PRODUCT_REQUIREMENTS.md) のowner registryに登録された文書だけである。
本書は、その正本から実装と直接オラクルへの最短経路を選び、回帰防止と過剰品質防止を
同時に行うための手順だけを定める。

## 変更順序

仕様文書の正本性は、エージェントが製品仕様を決め直す権限を意味しない。ユーザー要求と、ユーザーの実際の依頼・承認へ遡れる既存契約を変更の根拠とし、baseの文書、エージェント作成Issue、エージェント自身が追記した条件を、その条件の承認根拠にしてはならない。特に、価格を表示しない、期間選択を別の時間範囲へ置き換える、連続線を点や別の描画方式へ変えるなど、既存UI仕様の変更にはユーザー本人の明示承認が必要である。不明値の保全を理由としても、根拠のある既存機能を一律に無効化しない。バグ修正・安全化・文書化の許可や、モデルへの一般的な担当委譲だけではUI仕様変更を許可されたことにならない。

実装と実画面の判定は、変更前の承認済み契約とユーザーの修正指示に遡って行う。文書・test期待値・review結果を実装に合わせて書き換え、元要求との相違を合格にしてはならない。矛盾する無断変更は採用せず、承認済み仕様へ復旧する。同じ仕様への復旧と不具合修正について、ユーザーへ不要な再承認を求めない。

1. `PRODUCT_REQUIREMENTS.md`のowner registryから対象領域の唯一のowner文書とmaster IDを決める。
2. ownerの重複、未登録owner、参照先のないID、同じ観測結果の複数定義があれば、実装・build・test・
   agent reviewを開始せず、まず正本の矛盾を解消する。
3. 契約変更は `owner正本 → 要求台帳 → 実装 → 直接オラクル` の順に行う。古い台帳、test、
   README、運用文書へ実装を合わせない。
4. 高コストの確認前に `scripts/quality_plan.py` で完全な変更pathを分類する。分類できないpath、
   未知のcheck、重複check、影響領域外のcheckは実行前に停止する。
   提出前のGit差分もCIと同じrename/copy検出と`paths_from_name_status`を用い、両端を分類する。
   commit済み差分はCIと同じ`base...HEAD`とし、base側だけで進んだ別作業をPR差分に含めない。
   committed・staged・unstaged・untrackedの読取りが全て成功するまで計画を公開しない。
   一部の読取り失敗を、先に取得できたpathだけの検証で代替しない。
5. 選択されたcheckは共有呼出しをまとめ、同一revisionで各1回だけ実行する。失敗を確認なしのrerunで
   上書きせず、原因または外部状態が変わった新しい入力でだけ1回再実行する。

## 仕様確定の条件

実装開始前にowner正本だけで、別の実装者が同じ機能上の結果を導ける状態にする。対象となる入力の意味・外部依存と版・採否条件・計算・状態遷移・失敗時保持・互換性・固定値のscopeを定義し、相互参照の矛盾を解消する。「後で決める」、実装定数の追認、実装やtestを読まないと答えが決まらない条件を残さない。クラス分割など観測結果を変えない実装詳細を全て文書化する意味ではない。

実装に必要な条件を後続Issue、実装者の裁量、将来の評価へ保留した仕様は未成立である。入力値が不明であることと、仕様の判断が未決定であることを区別する。前者には採否・保持・表示・回復条件を正本で定め、後者は実装前に解消する。証拠が存在しない値を決める必要はなく、その値を不明として扱う動作を決める。今回要求された全対象の必要仕様に未解消が1件でもあれば、製品コード・実行時fixtureの変更、品質完了判定、PRを行わない。実装中に新たな未定義・矛盾を発見した場合も仕様段階へ戻り、ownerの条件を確定してから変更を再開する。

現データに説明不能な値・線・帯・件数・空表示が残る場合は、その原因分類と原本復元を先に行い、一般評価へ進まない。仕様に必要な判断が実装中に不明になった場合はownerへ戻り、対症patchで埋めない。文書の構造検査は意味の正しさや実画面受入を代替しない。全画面の対象範囲はPRODUCTの表を正本とする。

## PR前の品質確認と提示

要求台帳の状態は`in-progress / implemented / verified`の3値に限定する。通常の文書構造検査は3値を全て受理し、owner・master ID・台帳の参照整合だけを確認する。最終検査（`--final`）は全行が`verified`であることを必要とし、`in-progress`も`implemented`も拒否する。未完了の正直な記載を構造不正にせず、構造PASSを仕様確定・品質確認完了・PR許可へ読み替えない。状態の意味は要求台帳の冒頭定義に従う。

受入条件は、ユーザーが許可した全要求について正本の条件・境界・失敗動作を網羅した品質確認が完了し、現データの全対象画面を説明できることである。test/gate/CIの成功だけを受入条件にしない。要求と品質項目、実装経路、独立した期待値、最終revisionの証拠を対応付け、未対応・未検証・FAIL・INCONCLUSIVEが1件でもあればPRを作成・更新しない。既知のworkflow失敗を未処置のまま提出せず、失敗後は漏れた品質項目と影響範囲を見直す。古いPASSやfixture成功で現DB・実画面の未確認を埋めない。

Main・Graphの全mode/metric・Threads・Settings・Setup・Legalについて、値だけでなく表示状態、全操作の実効果、無効条件、失敗時保持を確認する。受入に使用した実画面画像はchatにも提示し、画像を見たユーザーの具体的な指摘には原本/正本/同revisionの動作で答える。答えられない箇所は未確認として受入を止め、既存の品質確認済み判定を流用しない。画面に変更があれば依存する画像を取り直す。

発見したCodacy指摘は放置せず、該当source/workflowへの修正、または原始結果に基づく誤検知の説明と処置を完了する。未処置の指摘をwarning、古いもの、別のPASSという理由だけで除外しない。解析の実行/取得に失敗した場合を指摘0件としない。

実行権限とモデルの担当はユーザーの最新明示指示、`AGENTS.md`、Issue #104に従う。本作業で指定された親SOL6.1が要求・設計・実装・判断を所有し、子Luna-maxは調査・評価だけを行う。権限は製品仕様へ混在させない。許可されたPR経路は`codex/<task> -> feat/next`に限定し、merge・auto-merge・main・Releaseの操作は行わない。品質確認の完了とユーザーの統合判断は別である。

## 最小で十分な品質

- 影響するmaster IDごとに、その観測結果を直接判定できるオラクルを少なくとも1件持つ。
- 同じ観測結果のcheck ownerは1件にし、他のgateからtest名、呼出し数、実装文字列を二重監視しない。
- 変更のない製品ownerのtest、build、installer、実画面、実OS E2E、CodeQL言語は実行しない。
- mainの自動生成H1だけは、既存Release品質runが同じheadの単一commit statusを`pending`から最終結果へ更新する。
  feat、H2、binary impactなしheadへ重複statusを作らず、repository ruleでMergeを強制blockしない。
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

直接オラクルは、配布されるbinaryの入口から対象producer/consumerへ到達する実行経路と、`cfg`/build条件を確認して選ぶ。testがproductionの関数を呼ぶことと、`cfg(test)`内に複製した旧parser/collectorが通ることを区別する。後者の成功は現在の収集品質を証明しない。Session消費の記録経路はrecorder binaryのcycleから`scan_source`へ入る経路を対象にし、UI binary側のtest専用`collect_session_append`/`collect_session_usage_records`で代替しない。共通の純粋moduleの直接検証も、未接続の収集・保存・公開経路まで合格としない。

## 失敗と証拠

- current取得契約を変更するPRでは、`windows-contract`の直接検証として
  `bash scripts/windows_client_contract_gate.sh --current-acquisition`を提出前に実行する。
  `LoopbackStatusClientTests`（HTTP・schema・cache・互換）、`MainWindowViewModelTests`
  （受理世代から表示・thread・復旧）、`ModelUsageDisplayContractTests`
  （実HTTP parserから共有数値oracle）の3 consumerを一つの受入境界にする。
  編集したtest名だけのfilterやViewModel単独の成功で代替しない。
  full Windows gateはこの3 consumerを含むため代替可能だが、無関係な全suiteを追加実行しない。
  focused gateは3 consumerの実行証拠欠落・SKIP・失敗・0件を拒否する。
- 上記の実行は影響範囲の確定を代替しない。PR全差分を製品正本の契約、producer、
  全直接consumer、独立期待値、検証結果へ対応づけ、未対応がある間は提出不可とする。
  hosted failure時はこの対応とPR全体の受入を無効化し、原因と漏れを既存Issueへ記録する。
  変更していない経路のnamed結果は保持するが、単独でPR全体の合格とはしない。

- PASSは、実際に実行した直接オラクルの原始結果だけで示す。0件、SKIP、INCONCLUSIVEをPASSに変換しない。
- 外部環境でしか得られない証拠は、local gateの成功に混ぜず未確認として残す。
- Actionsの基盤・制御失敗はIssueへrevision、症状、原因、過剰checkの有無、再発防止を記録する。
- 回帰検出後は過去のPASSを現行成果物の証拠に流用せず、影響経路だけを新しいrevisionで1回再検証する。
- DB・Session原本を削除・再生成せず、値を推測補完して見かけ上回復させない。原本に基づく派生値の修復はPRODUCT `HISTORY-RECOVERY-614`とDATAの全資格・backup・transaction・冪等性に従う。通常の再投影、誤計算を証明した派生eventの修復、schemaだけの移行を混同しない。
