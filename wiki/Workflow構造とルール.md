> **位置づけ:** 非規範的な索引です。Workflow契約は[`PRODUCT_REQUIREMENTS.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/PRODUCT_REQUIREMENTS.md)とrepositoryの`AGENTS.md`が所有します。

# Workflow構造とルール

## 二つのPR経路

```text
codex/* → feat/next
  feat-integration.yml
    classify complete diff
      → selected advisory owner jobs

feat/next → main（ユーザーだけが操作）
  main-quality.yml
    trusted exact-head diff/version validation
      → selected Release-quality jobs
  merged + successful final-head quality
      → release.yml resolve → publish
```

`codex/* -> feat/next`はreview用のadvisory経路です。完全diffをownerへ分類し、関係する実jobだけを表示します。失敗、取消、pending、未実行は非greenのままですが、required checkやrulesetでmergeを禁止しません。GitHub nativeのボタン色そのものは製品acceptanceではありません。

`feat/next -> main`はRelease品質経路です。Codexはmain向けPR、merge、approve、Releaseを操作しません。trusted baseだけがexact headのversionとownerをread-only検証し、Actionsはbranchを変更しません。mainのmerge可否判断もユーザーが行い、Workflowは品質状態と公開可否を別に表します。

## Workflow別責務

| file | 唯一の責務 |
| --- | --- |
| `feat-integration.yml` | feat PRの完全diff分類とadvisory owner呼出し |
| `main-quality.yml` | main PRのread-only版数検証とRelease品質呼出し |
| `selective-quality.yml` | DOCS/GOVERNANCE/Linux backend/Linux UI/Windows/CodeQL/distributionの選択 |
| `rust.yml` | 選択されたRust ownerの直接check |
| `linux-ui-quality.yml` | 選択されたLinux UI check |
| `windows-client.yml` | 選択されたWindows checkとRelease candidate |
| `codeql.yml` | 選択言語だけのCodeQL |
| `linux-release-quality.yml` | main binary向けLinux test/UI/build/bundleを1回で実行 |
| `release.yml` | mergedとsuccessful final-head qualityを解決し、binary impact時だけ同一candidateを公開。non-binary時は公開しない |

`release.yml`はWindows 2 assetとLinux 3 assetを同じstable Releaseへ揃えます。表示名も`Release product`へ統一し、旧Windows単独名を残しません。

## 失敗と再実行

- PRのWorkflowを実験場所にしません。localの同じselector/callerで先に確認します。
- 同じrevision・同じ前提条件の失敗をrerunしません。原因修正または外部状態変化をread-backしてから一度だけ再実行します。
- infrastructure/control failureはIssueへrun、revision、症状、原因、過剰checkの有無、再発防止を記録します。
- same-headの最新failureを過去のsuccessへのfallbackで隠しません。
- feat向けtriggerをmain向けtriggerの単純拡張にせず、feat経路でversion、candidate、tag、Releaseを変更しません。

要求追加・障害・Release準備で実行する仕様から最適化までの流れは[検証ハーネス](検証ハーネス#開発プロダクトライン)に集約します。Workflowはその一部であり、古い仕様、重複test、未変更ownerのcheckを自動的に増やす根拠にはしません。

## 定義・検証状態

- owner / master ID: `PRODUCT`の`WF-BINARY-IMPACT-01`、`WF-FEAT-SELECTIVE-01`、`WF-QUALITY-ONCE-01`、`WF-NONBLOCKING-QUALITY-01`、`WF-POSTMERGE-01`、`WF-SERIAL-01`。
- 実装入口: `.github/workflows/*.yml`、`scripts/ci_change_scope.py`、`scripts/quality_plan.py`、`scripts/resolve_pr_quality.py`。
- 直接oracle: actionlint、resolver/publisher/selectorの有限直接fixture、GitHub上のexact revision job graph。
- 未確認の扱い: local fixtureはremote jobの成功へ読み替えません。exact revisionのjob graph、candidate、publicationを確認していない状態は`INCONCLUSIVE`であり、このページ自体はmerge可否やRelease PASSを決めません。
