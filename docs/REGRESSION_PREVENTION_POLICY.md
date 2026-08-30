# 回帰防止規約（必須）

一度受入した機能を次の修正で失うことは、単なるテスト漏れではなく納品禁止の回帰である。本規約は今回限りではなく、以後すべての変更に適用する。

## 固定契約

| 契約ID | 固定する振る舞い | 回帰オラクル |
| --- | --- | --- |
| REG-WIN-DRAG | 全ボーダーレス画面は上端タイトル領域の明示的な左ドラッグで移動でき、ボタン操作を奪わない | `WindowDragBehavior.Attach` / `BeginMoveDrag`、Presentationテスト、対象release artifactの実機証拠（物理入力は明示許可時のみ） |
| REG-GRAPH-END | 現行グラフの終端は`min(reset_at, now)`で、開始アンカーから右端まで使用する | `EffectiveGraphEnd`テスト、グラフfresh画像、X版差分監査 |
| REG-GRAPH-SERIES | 累積最大値、初回未観測区間の水平保持、flat/rising線幅・opacity、未使用期間の専用ニュートラル帯（X/Windows共通の`#3F5D7C`・opacity `0.22`）を維持する。モデル使用後に遅れて届いた低い残量観測は反映し、残量観測が無い区間を料金から推測しない。通常sample内の両側実測で挟まれた欠測だけは正本規則で補間し、source cursorから回収不能と確定したdaemon gapは補間・旧値複製・推測をしない。 | Graph fixture、遅延残量観測回帰テスト、gap種別付きsource/DB/API契約検査、色定数・opacityテスト、独立X/Windowsグラフ監査 |
| REG-MAIN-REFRESH | 定期的なquota再取得は不完全な中間イベントであり、前回コミット済みのモデル・履歴・thread表示を空に戻さない。認証主体の変更/明示的なログアウトだけが可視状態をクリアし、rolling reset_atの揺れは同一期間として扱う。 | `periodic_quota_refresh_retains_last_good_main_snapshot`、quota→local commitの状態遷移テスト、X版fresh画面、Windows API parity証跡 |
| REG-STARTUP-FRAME | X/Windows初回起動は、Xのquota/local usage/threadまたはWindowsのhealth/status/detailsの完全世代が揃うまで固定レイアウトのスピナーだけを表示し、部分世代の段階描画・画面全体のばたつきを許可しない。初回失敗時はスピナーを解除して最後の完全表示または失敗状態と再試行手段を表示する。X起動ウィンドウは主モニターの可視デスクトップ内へ配置し、見えない位置での起動を成功扱いにしない。`--ui` のservice起動失敗でもGUIを終了させない。 | `native_startup_loading_requires_a_complete_authenticated_generation`、`Startup_keeps_content_hidden_until_the_first_snapshot_is_complete`、X fresh起動画面、`Main.StartupLoading` UIA、実Windows初回起動画面、X11可視範囲ゲート、失敗ポートでの`--ui`実起動 |
| REG-SETUP-ONCE | 接続確認済みならSetupを再表示せず、6-key設定へ再接続に必要な非秘密profile selectorだけを保存する。raw host/user、OpenSSH展開値、password/token/key/path/API URL/commandは保存しない | 6-key atomic保存・old4 recovery・次回自動再接続・`ShouldOpenSetup`テスト、設定JSON secret scan |
| REG-NO-MOUSE-STEAL | 製品コードはカーソルを操作しない。物理入力試験はテストプロセスからだけ明示opt-inで実行し、製品実行へ混入させない | 製品ソースAPIスキャン、smokeの既定SKIP raw出力、CI受入時の`-AllowPhysicalInput`実行ログ |
| REG-I18N-CLI | CLIヘルプを含む利用者向け固定メッセージは、画面本体と同じ対応localeのi18nカタログから導出し、起動スクリプトに単一言語の製品文言を複製しない | 全対応言語の`launch_help`テスト、`LC_ALL`切替の`run.sh --help`実行、スクリプト固定文言検査 |
| REG-CLI-LIFECYCLE | 公開argvを無印、`--ui`、`--port PORT`、`--ui --port PORT`、`--stop`、help aliasだけへ限定する。addressは127.0.0.1固定、停止は検証済み同一profile ownerへのTERM 1回とlock解放待ちだけとし、拒否入力・停止でDB/sourceを変更しない | parser有限受理・拒否unit、release binaryを使う`scripts/cli_contract_e2e.sh`（loopback health、stop冪等性、DB/source保持、invalid lock fail-closed） |

## 強制ゲート

1. 変更前に`docs/PRODUCT_REQUIREMENTS.md`と対象仕様を読み、変更する観測結果と失敗時動作を決める。
2. 変更後は`bash scripts/pre_pr_gate.sh`をローカルの単一入口として1回実行する。同入口はnative回帰、data protection契約、固定.NET SDKのCore/Presentation testを含むWindows契約を各1回だけ所有し、同じtest/gateを入口の外で再実行しない。要求台帳はlocalで観測できるschemaと実装状態だけを確認し、PR job・artifact・実OSなどremoteでしか得られない証拠を要求する`--final`は完了判定時だけ使用する。
3. インストーラを再発行した場合は、artifactとworkspace publish copyのSHAが一致し、ホストのインストール先SHAも一致することを確認する。
4. 独立サブエージェントが、実装者のPASS結論を見ずに上表を再評価する。1項目でもFAIL/INCONCLUSIVEなら`RELEASE HOLD`とする。
5. `docs/INDEPENDENT_AUDIT_LATEST.md`を`status: PASS`へ変更できるのは独立評価担当だけとし、主担当が手動でPASSへ書き換えてはならない。

6. `scripts/regression_guard.sh`は静的な文字列検査だけでPASSしてはならない。`cargo test --locked --all-targets -- --nocapture`を1回だけ実行し、履歴・グラフ・thread・data protectionの必須Rust回帰テスト（複数期間の境界、通常のmoving-reset、累積ドリフトする長時間moving-reset、使用量0の残量100% reset断片が前後の使用量期間を分割しないこと、観測されていない長時間を累積使用量の斜め線として描かないこと、モデル使用後の遅延した低残量観測を捨てないことを含む）が同じ出力内で実行・成功したことを名前付きで確認する。必須testの個別`--exact`再実行、対象test 0件、未実行、失敗は必ずFAILにする。さらにworking tree/index/current commitのdiff、format、全target check、release buildを同じゲートで検査する。`DISPLAY`が利用できる実行では`bash scripts/x11_graph_visual_gate.sh`を同じ実行で起動し、現行バイナリの940x640グラフ画像から残量線の連続性とLUNA/TERRA/SOLの色画素を機械判定する。履歴・グラフの変更は、この実行結果なしに完了判定してはならない。これはデグレード防止だけでなく、実行していない検証をPASS扱いする評価漏れの防止を目的とする。

7. WINDOWS ownerが選択されたPRだけ、Windows jobがunit test、installer lifecycle、UI Automation、物理window moveを各1回実行し、main経路では成功後にSetupとmanifestのcandidateを作る。WINDOWS非選択時はjobとcandidateをskipする。共通`selected-quality`はselectedのsuccessとnon-selectedのskipだけを集約し、証拠artifact、同じtestの再実行、live rule再監査を追加しない。

8. `main`向けsame-repository PRはhead branch名に依存せず、version追加前の全変更pathをDOCS・GOVERNANCE・LINUX_BACKEND・LINUX_UI・WINDOWSへ分類する。Git callerはrename/copy検出を明示して両端を同じ分類器へ渡す。選択ownerとそこから導出したCodeQL言語だけを最終headで各1回実行し、非選択ownerは実行しない。結果集約はtrusted base版gateだけを使う。Windows-onlyはWINDOWS+csharp、governance-onlyはGOVERNANCE+actions/pythonとする。workflow全体をpath filterで消さず、required `version-prepared`と`acceptance`は通常評価または生成H1 producerが最終headへ生成する。event時／開始時draft、開始時stale、正規生成H1 observerはownerを実行しない。分類の二次実装、branch名allowlist、live rule再監査、全製品ownerの一律実行を追加しない。

9. 履歴比較を必要とするcheckoutは`fetch-depth: 0`を使用する。ローカルの全target testは実行件数0を許可せず、Windows testはpassed>0かつskipped=0を満たさなければならない。PRのCLI/recorder/実Windows UIは実行jobの成功を必須とし、未実行、SKIP、失敗をPASSへ変換しない。

10. X先行の変更凍結を必須とする。X版の正本要件（履歴期間、未使用帯、残量とモデル使用量の独立性、定期更新の前回表示保持、thread失敗時の全体破棄）を同一revisionの`pre_pr_gate.sh`内にある全target testと実画面検査で確認するまで、PRのCI・workflow再実行を開始してはならない。X確認後にソース・仕様を1行でも変更した場合、確認結果は無効化し、単一ローカルゲートからやり直す。変更が確定した同一revisionでは、ローカル全体ゲートとCIを各1回までとし、失敗時は原因を修正してから次のrevisionで一度だけ再実行する。未確認のまま先に進めるためのworkflow再実行は禁止する。

11. バイナリ影響ありでversionがbaseのH0は完全差分を1回分類し、patchを十進整数で1増やす3ファイルだけのcommitをnon-force pushして、同じrunでH1の選択jobを各1回実行する。生成commit自身へPR、H0、run ID、attemptを固定trailerとして持たせ、push後cancelでもH0/H1対応を失わない。`GITHUB_TOKEN` pushが次runを起動しないため、H1へ必要な`version-prepared`と`acceptance`だけを最終結果として作る。正規trailerを持つtipの生成H1 eventだけowner再実行を抑止する。identityのない手動version commitとH2は評価し、byte-exactな生成3 pathだけをowner差分から除外する。
12. Releaseはmerged `closed`とMain Quality completedの両順序を扱う。current final headを評価した全non-observer attemptの1件でも失敗した場合はsame-headの後続successで上書きせず、新headまで公開しない。Windows skippedはcandidate 0、Windows successはown-run candidate exact 1とし、欠落・期限切れ・複数・identity不一致で旧artifactへfallbackしない。公開jobだけをversion tag単位で直列化し、lock取得後の完全不存在だけを作成、完全一致publishedだけをno-opとする。不完全状態を自動修復せず、test/build/CodeQLを再実行しない。

13. PR由来codeをwrite tokenのあるjobでcheckoutまたは実行しない。変更分類器はtrusted baseの単一実装を使用し、eventのbase/head Git object差分を分類する。採番jobはPR headをdataとして読むだけにし、non-fast-forward拒否をrace境界とする。H1 check作成jobとpost-merge jobもsourceをcheckoutしない。post-mergeはfinal headの評価attempt集合とWindows job状態を使い、PR fileを再分類しない。分類失敗、selected job失敗、version未準備はmerge不可とする。

14. Codex code reviewは変更が確定したPR最新headへ`@codex review`を1回だけ投稿して起動する。新commit後は旧headのreviewを根拠にせず、non-outdatedかつ未解決のP0/P1がある間はready/releaseをHOLDする。Codex用の独自API key workflowやrequired statusを追加せず、CodeQL、required acceptance、必要な承認の代替にしない。

15. workflow変更は、owner全31組合せ、rename/delete/unknown/空差分、selected異常、binary false/base/H0→H1 observer/H1→H2/手動version/inconsistent/head race、生成H1 checkの成功・異常、event-draft/staleのjob開始前cancelとcurrent-draft/closedの完了observer、H0 push前後cancel、same-head failure後success、両event順序、Windows skip/success、candidate欠落・期限切れ・複数、同tag二信号、orphan/Draft/partial/mismatchをローカルの有限fixtureで先に確認する。このfixtureを通常のowner品質DAGへ追加せず、同じrevision・外部前提のActionsを再実行しない。実PRではevent起動、required checkのApp identity、artifact受渡し、tag/Release終端だけを確認する。予期しないActions失敗ではmatrixを停止し、Issueへ原因、過剰確認の有無、再発防止を記録して新revisionへ戻る。

## 回帰発生時

回帰を検出した時点で、前回のPASS証拠を現行成果物の証拠として再利用しない。影響する観測結果だけを再検証し、無関係な検査や文書hash更新を連鎖させない。過去DB・履歴を削除して見かけ上直すことは禁止する。
