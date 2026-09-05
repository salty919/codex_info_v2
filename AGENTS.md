# Codex Info 開発ガイド

- ユーザーの変更と無関係な差分を戻さない。依頼なしにcommit、push、PR作成をしない。
- 製品要件の正本は`docs/PRODUCT_REQUIREMENTS.md`とし、wire、データ、UIの詳細は同文書が参照する仕様へ置く。
- 発見Issueのうちユーザーへ確認するのは`priority:P0`と`priority:P1`だけとし、それらも専門用語だけに偏らず、何が問題で何を行うかをユーザーが理解できる短い言葉で説明し、そのIssueへの明示許可を得るまで着手しない。`priority:P2`、`priority:P3`、`priority:info`はIssueへの記録だけとする。
- 外部authority値を推測しない。publisher、certificate、対応OS、保証値が未指定なら、公開・対応表明・mutationを行わないfail-closed動作を要件化する。
- lockfileと既存package managerを守る。検索は`rg`を優先する。
- **常駐必須規則:** [Issue #104（常駐課題）](https://github.com/salty919/codex_info_v2/issues/104) の「常駐必須条項」を、全ての調査・編集・test・委譲・GitHub mutation・完了判定の前に読み、必ず適用する。Issue #104は本ファイルから移動した過剰品質防止・要求/証拠・設計整合性の正本であり、同Issueの未確認条項を推測でPASSにしない。

## 共有repositoryのbranch・worktreeガバナンス

### 正本、用語、branchの責務

- `origin`は共有GitHub repositoryを指し、remote headは`origin`上の`refs/heads/*`を指す。Codexは他利用者が所有するbranch、worktree、変更、PRを推測で変更または削除してはならない。
- `/home/salty/code/codex_info_v2`をユーザー通常worktreeとする。`feat/next`はユーザーが確認・統合するbranchであり、Codexにとってread-onlyとする。
- `main`は本番・Releaseの正本とする。Codexは`main`を変更する判断、`main`向けPRの作成・更新・操作、`main`への直接pushを行わない。`feat/next`から`main`へのPR、承認、merge、close、Release判断はユーザー本人だけが行う。
- Codexが書込み可能なbranchは、宣言済みの完全な`origin/feat/next` SHAから作る一時branch`codex/<task>`だけとする。Codexが作成・更新できるPRは`codex/<task> -> feat/next`だけとし、`feat/next -> main`はCodexの操作範囲外とする。
- `active task`は、ユーザーが宣言内容を明示許可してからcleanupが完了するまでを指す。`at rest`はactive taskが0件の状態を指し、Codexが作成したlocal/remoteの`codex/*` branchと追加worktreeが残ってはならない。未知のbranchは他利用者所有として保持し、Codexは削除せず報告する。

### worktreeを使う理由と安全境界

- Codexによる編集、test、build、formatter、生成物作成は、許可済み一時branchをcheckoutした一時worktree内でだけ行う。ユーザー通常worktreeでは`status`、`diff`、`show`、`log`、`worktree list`、`branch list`、`ls-remote`等の読取りだけを許可する。
- Codexはユーザー通常worktreeでcheckout、switch、add、commit、push、reset、rebase、merge、cherry-pick、stash、clean、update-ref、Git config変更、test、build、formatter、生成物作成を行ってはならない。`fetch`も共有Git状態を変更するため、許可前は行わない。
- worktreeには、ユーザー通常worktreeを汚さない、branch切替を不要にする、固定SHAで検証できる、独立したbuild出力を持てる、Git objectを共有して高速・省容量になる利点がある。
- worktreeは`.git`、object database、refs、config、hooks、remoteを共有するため完全なsandboxではない。branch責務、明示許可、path所有権、fail-closed停止を安全境界とし、これらを省略してworktreeを使用してはならない。
- 書込み作業で一時worktreeを使用する利点がない場合、Codexはユーザー通常worktreeへfallbackせず、その書込み作業を開始しない。

### 作成前のpreflightと明示許可

- Codexはbranchまたはworktreeを作る前に、`git status`、`git worktree list --porcelain`、local/remote branch、Open PR、対象pathの所有・dirty状態を読取り確認する。
- baseは`git ls-remote origin refs/heads/feat/next`が返す単一の40桁object IDとする。不存在、複数、取得不能、malformedの場合は停止し、localの`feat/next`、`main`、`FETCH_HEAD`、古いremote-tracking refで代替してはならない。
- Codexは次の全項目をchatで宣言する。先に会話とGOALの既存許可を読み、今回の目的と操作を含む継続自走許可があれば再確認せず進める。既存許可で扱えない新しい目的や操作だけをユーザーへ確認する。宣言自体を許可と解釈してはならない。

```text
Worktree使用申請
目的:
必要性と利点:
代替手段では不足する理由:
canonical worktree path:
一時branch名:
origin/feat/nextの完全なbase SHA:
owned files / paths:
変更しない範囲:
許可を求める操作（edit/test/commit/push/PR等）:
予定時間と完了予定時刻:
検証方法:
統合方法とPR target:
cleanup条件と削除予定:
```

- 許可の継続と再確認は下記「認可状態の継続と自動GOAL継続」を正本とする。branch、path、base SHA、予定時間は作業状態として報告し、その変更だけを許可失効と扱わない。
- 許可後のfetchは、宣言した`origin/feat/next` refと完全SHAの取得に限定する。fetchが`FETCH_HEAD`と共有object database等を変更することを前提とし、作成直前にremote SHAが宣言値と同一であることを再確認する。
- 一時branch名は一意な`codex/<task>`、canonical worktree pathは`/home/salty/code/codex_info_v2-wt-<task>`とする。既存local/remote branch、既存path、symlink、別worktreeと衝突する場合は作成せず停止する。

### 認可状態の継続と自動GOAL継続

- `同一申請`は、完全なbase SHA、一時branch名、canonical worktree path、owned files / paths、変更しない範囲、許可を求める操作、予定時間と完了予定時刻、統合方法とPR target、cleanup条件が同じ申請を指す。同一申請はchatで1回だけ提示し、重複して許可を求めない。
- 申請提示後にユーザーが明示許可したら、認可状態はactive taskのcleanup、ユーザーによる取消し、または下記の失効条件まで継続する。宣言済みowned scopeと操作のedit、test、commit、push、PR作成の各段階で、ユーザーへの追加許可確認を挿入しない。継続自走を明示したactive GOALでは、この認可範囲を完了または真の外部blockerまで連続実行する。
- `自動GOAL継続`は、ユーザーの新しい意思表示を含まず、永続するactive GOALの作業継続だけを求めるイベントを指す。これを許可、拒否、取消し、scope変更、再申請理由と解釈せず、認可状態を変更しない。認可状態は、新しいユーザーメッセージが明示的に許可、拒否、取消し、または申請内容の変更を示した場合だけ変更する。
- 同一の未認可状態で自動GOAL継続が発生しても、同じ申請、説明、read-only監査を繰り返さず、それだけを理由にactive / blockedを往復しない。認可待ちの状態を保持し、無関係なtest、worktree、Issue、workflow実験を追加しない。
- ユーザーがGOALや会話で本修正の連続自走を許可し、repository規約より優先すると指定した場合は、その目的に必要な調査・編集・直接検証・commit・push・feat向けPR更新・worktree作成とcleanupを継続する。個別申請の重複、予定時間超過、ターン終了、worktreeの再作成だけで停止しない。ユーザーが明示した作業終了期限は守る。未統合PRがあることだけで、他の許可済み作業を停止しない。
- 再確認は、ユーザーの取消し、許可された目的を外れる変更、新たな破壊的操作、またはユーザー判断なしに解消できない所有権・要求の衝突がある場合に限る。事実誤認は訂正して影響を示し、訂正後も既存許可内なら続行する。明示的にユーザーだけが行うmerge・main・Release操作は代行しない。
- 予定済み依存PRの統合でbase SHAが進んだ場合は、旧新SHAとowned pathsの差分を確認し、競合と目的変更がなければ既存許可を維持する。予期しない差分がある場合も、影響を調査してから判断し、SHAの変化だけで再許可待ちにしない。必要な検証は影響範囲だけ更新する。

### ownership、実装、検証、統合

- 1 taskにつき一時branch 1本、一時worktree 1個とする。同じfile/pathにwriterを1人だけ割り当て、owned pathsと変更禁止範囲を宣言する。
- `AGENTS.md`、workflow、lockfile、共通仕様、要件台帳等のcross-cutting fileは排他的所有とし、同時に別のwritable taskを走らせてはならない。ユーザー通常worktreeの同じpathにdirty/untracked変更がある、ownershipが重複する、または一時worktreeに不明差分が現れた場合は停止する。
- サブエージェントは宣言済み一時worktreeとowned pathsだけを扱う。サブエージェントによるbranch/worktreeの作成・削除、ref/config/remote操作、commit、push、PR操作を禁止し、管理は主担当だけが行う。
- Codexは宣言した最小のformatter、check、testを実行し、0件のtestをPASSにしてはならない。既往障害、security、cross-cutting governance等で独立判断が必要な場合だけfresh evaluatorを使う。
- commitはユーザーが明示許可した場合に限り、owned filesだけをstageして行う。pushとPR作成も宣言に含まれ明示許可された場合だけ行い、Codexが作るPRのbaseは`feat/next`に限定する。
- Codexは、ユーザーから依頼または許可を受けた場合も、いかなるPRもmergeせず、auto-mergeを設定または解除しない。この禁止に例外はなく、merge操作はユーザー本人だけが行う。Codexは`codex/<task> -> feat/next`のPR作成・更新と作業証拠のcommentを行える。PRのapprove、ready化、closeおよびworkflowのapproveまたはrerunは、exact targetと操作についてユーザーの明示許可がある場合だけ実施できる。
- CodexはPRのURL、base/headの完全SHA、変更file、検証結果、未確認事項、`main`へ統合した場合の影響を提示し、ユーザーが変更と動作を確認できる状態でmerge前に停止する。Codex自身の実装・検証・review結果を、ユーザーによる統合判断の代替にしてはならない。
- pushまたはPR作成が許可されていない作業を「統合済み」または「完了」と報告してはならない。実装済み、local検証済み、未統合を区別して報告する。
- pushまたはPR作成直前に`origin/feat/next`の完全SHAを再確認する。進んでいる場合は旧新SHAとowned pathsへの影響を報告し、「認可状態の継続と自動GOAL継続」に従って継続可否を判断する。履歴書換えを要しない既存branchの更新を、base SHAの変化だけで停止しない。
- `codex/<task> -> feat/next`はtrusted `feat-integration.yml`が完全なPR差分を有限ownerへ分類し、関係するremote qualityだけをadvisoryに実行する。実owner job、CodeQL、distributionの失敗は赤のまま表示するが、`selected-quality`集約と`feat-acceptance`は実行せず、workflow結果でユーザーのmergeを禁止しない。この経路ではversion、candidate、Release、tag、branch refをmutationしない。`feat/next -> main`は同じ分類正本を使用するが、`selected-quality`・`acceptance`・`version-prepared`はRelease品質と公開可否を別のmain経路で所有し、merge判断はユーザーだけが行う。feat向けtriggerをmain向けtriggerの単純な拡張にしてはならない。
- Git差分callerはrename/copy検出を明示し、両端を単一分類器へ渡す。mainのRelease向けselected/non-selected結果だけをtrusted base版gateで集約し、feat向けPRは選択された実job自身の結果を表示する。
- workflowの`GITHUB_TOKEN`によるref更新が別のActions runを起動すると仮定しない。main向けversion生成H1は同じtrusted DAGでRelease品質を評価し、生成commitの固定trailerとproducer runでH0/H1を対応付ける。H1へcustom `version-prepared`・`acceptance` checkを登録せず、同じH1のeventは正規trailerを確認できた場合だけowner再実行を抑止する。この確認はbyte-identicalな手動commitとの区別だけを目的とし、poll、retry、mutation readback、表示URL照合、証拠専用artifactを追加しない。

### race、cleanup、復旧、報告

- cleanupは主担当だけが、宣言済みbranchとcanonical pathに対して行う。merge/fast-forwardではcommit ancestryを確認し、squash、rebase、cherry-pick相当ではdeclared diffがtargetへ反映されたことと受入checkを確認する。PRの状態だけを統合証拠にしてはならず、同等性が曖昧な場合は削除しない。
- cleanup前に一時worktreeのtracked/untracked差分と生成物を確認する。不明差分、未commit変更、未統合のunique commitがある場合は保持し、完全SHA、対象path、PR状態、復旧方法を報告してユーザーのintegrate/discard判断を待つ。
- cleanupにはforceなしの`git worktree remove`、統合済みbranchに対する`git branch -d`、許可済みremote一時branchの削除だけを使用する。`rm -rf`、`git branch -D`、`git worktree remove --force`、`git clean`、force pushを禁止する。
- task開始時と終了時に、時刻、canonical worktree path、branch、完全なbase/HEAD SHA、dirty状態、owned/non-owned scope、許可された操作と期限、check結果、commit、push、PR URL/state、残存worktree/local/remote ref、cleanup結果、復旧手段を、terminalで再現できるコマンドとともにchatで報告する。repositoryへagent台帳やEvidence文書を追加してはならない。

## GitHub Issuesガバナンス

### Issueを作業の正本にする

- 今後の機能追加、不具合対策、品質管理、security、運用、文書、調査は、実装branchまたはworktreeを作る前にGitHub Issueへ登録する。Issueが存在しない状態では読取り調査とIssue案の作成だけを許可し、編集、test、commit、push、PR等のmutationを開始しない。
- ユーザーがchatで新しい取り組みを依頼した場合、その依頼を、Codexが認証済みのユーザーaccountで対応Issueを作成・分類し、作業履歴を更新するstanding authorizationとする。作成直前にIssue title、分類、scope、既存Issueとの重複結果をchatで明示する。
- 既存Issueと同じ観測結果または成果を扱う場合は新規Issueを作らず、既存Issueを正本として参照する。重複か独立scopeか、parent、破壊的操作に曖昧さがある場合は作成・更新を停止してユーザーへ確認する。発見Issue以外でpriorityが未決定ならCodexは候補と理由を記録し、priority labelを付けず`status:triage`で登録する。Codexが作業中に発見して登録するIssueには観測した影響に対応するpriorityを必ず付け、ユーザー指定が複数または矛盾している場合は推測せず確認する。
- Issue serviceの取得・作成・更新に失敗した場合は、local branchや文書をfallback正本にせず停止する。token、secret、private data、raw logをIssueへ記録してはならない。
- Issue番号を得た後、一時branchの`<task>`は`issue-<number>-<slug>`、worktreeの`<task>`も同じ値とする。branch、commit、PR、進捗コメントは必ずIssue番号を参照する。

### 分類と状態

- 大分類labelは次の集合からexact 1件とする。`bug`は不具合対策、`enhancement`は機能追加、`quality`は品質管理、`security`はsecurity/trust/data protection、`operations`はbuild/CI/Release/repository運用、`documentation`は文書・governance、`investigation`は調査・意思決定を表す。
- area labelは`area:native`、`area:windows`、`area:ui`、`area:data`、`area:ci`、`area:release`、`area:docs`、`area:governance`から1件以上を付ける。責務境界を跨ぐ場合だけ複数areaを許可する。
- priority labelは`priority:P0`（緊急）、`priority:P1`（高）、`priority:P2`（中）、`priority:P3`（低）、`priority:info`（情報）とする。Codexが作る発見Issueでは観測した影響に基づくexact 1件を設定する。それ以外はユーザーが決定し、`status:triage`中だけ未設定を許し、`ready`、`in-progress`、`blocked`、`review`ではexact 1件を必須とする。
- open Issueのstatus labelは`status:triage`、`status:ready`、`status:in-progress`、`status:blocked`、`status:review`のexact 1件とする。scope、acceptance criteria、priorityが未確定なら`triage`、着手可能なら`ready`、許可済み作業中なら`in-progress`、外部依存で進行不能なら`blocked`、全実装と証拠が揃いユーザーclose待ちなら`review`とする。closed stateと重複する`status:done`は作らない。
- Issue作成時は大分類exact 1件、area 1件以上、`status:triage`を設定する。priorityはCodexが作る発見Issue、またはユーザー確認済みの場合だけ設定する。Codexは実際の状態遷移に合わせてIssue labelと節目コメントを更新する。状態を進めるために未確認のscopeやevidenceを推測してはならない。

### sub-issueと依存関係

- 独立した成果、異なるowner/path、依存順序、別PR、別受入証拠のいずれかを持ち、親Issueを単一のbounded changeでcloseできない場合だけnative sub-issueへ分割する。要求数だけを理由にN倍へ展開しない。
- Codexは分割前にparent、予定sub-issue、各scope、依存DAG、close条件を提示し、ユーザーの明示許可後にだけsub-issueを作成してnative parent relationを設定する。Markdown checklistや重複本文をhierarchyのauthorityにしてはならない。
- 実行順を制約する関係はnative Issue dependencyとして登録する。単なる関連はcommentまたはIssue linkに留め、誤ったblocked関係を作らない。
- 各sub-issueは単独で検証・reviewできるacceptance criteriaを持つ。親Issueは子の実装詳細を複製せず、全sub-issueの状態と親固有の統合criteriaだけを管理する。
- parent Issueは全sub-issueがユーザーによりcloseされ、dependencyが解消し、親固有criteriaが検証されるまで`status:review`またはopenのまま保持する。

### 履歴、PR、close authority

- CodexはIssueへ、着手、宣言済みbranch/worktree/base SHA、scope決定、重要な設計判断、blocker、検証結果、commit、PR URL、cleanup、closure reportを節目ごとにコメントする。短周期polling、逐次command出力、重複status、raw logを投稿してはならない。
- Actionsの基盤・制御失敗（選択された製品test自体の正当な失敗を除く）は、同じ観測結果なら作業中Issue、独立した結果なら別Issueへ、run、revision、症状、原因、過剰だった確認または処理、再発防止を記録する。同じrevision・前提条件のまま再実行せず、結果を変える修正または外部状態の変化をread-backしてから1回だけ再試行する。未解決または再発が24時間を超えた場合は通常進行として扱わず、blockerと経過をユーザーへ報告する。
- PR本文とcommit messageでは`Refs #<number>`を使用する。`Fixes`、`Closes`、`Resolves`等のauto-close keywordを使用してはならず、PR mergeだけでIssueをcloseさせない。
- closure reportには、Issue番号、要求とacceptance criteriaの対応、最新revisionのcheck evidence、関連PRの状態、open sub-issue 0件、dependency解消、未解決finding、残作業、branch/worktree cleanup、推奨close reason（`completed`または`not planned`）を含める。
- Issueの最終closeとreopenはユーザー本人だけが行う。Codexは明示依頼を受けてもIssueのcloseまたはreopen APIを実行せず、`status:review`とclosure reportを整えてユーザー判断を待つ。
- closure report後にrevision、criteria、sub-issue、dependency、PR、findingが変化した場合、以前のreportを無効化し、最新状態で再作成する。Codexはcloseを完了扱いせず、GitHub上のstateをread-backしてユーザーによるcloseを確認する。
