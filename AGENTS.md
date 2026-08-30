# Codex Info 開発ガイド

- ユーザーの変更と無関係な差分を戻さない。依頼なしにcommit、push、PR作成をしない。
- 製品要件の正本は`docs/PRODUCT_REQUIREMENTS.md`とし、wire、データ、UIの詳細は同文書が参照する仕様へ置く。
- 監査版ごとのEvidence、文書SHA一覧、サブエージェント作業台帳をrepositoryへ追加しない。再現可能なtestと必要最小限の仕様を残す。
- 要件漏れを理由にN倍、N二乗、N階乗、全直積へ展開しない。同じ観測結果は既存要件へ統合し、因果関係のある有限caseだけを追加する。
- ユーザーが述べていない要件、状態、分岐、品質目標を追加しない。実装上の入力errorや例外を製品要件の選択肢へ昇格させず、追加する各条項をユーザーの明示要求へ直接対応付けられない場合は削除する。
- 各mutationの直前と、編集、test、委譲、commit、push、PR作成へ段階が移る時に、その操作を明示要求、Issueの受入条件、owned scopeへ直接対応付ける。対応付けできない、またはscopeが広がった場合は即停止する。この判定結果は実装前、検証前、完了報告前に短くchatへ報告する。
- 実施申請に事実誤認、要求の取違い、scope誤りが判明した場合、その申請への許可だけを失効させ、要求自体は継続する。誤りと訂正差分を示して申請をやり直し、ユーザーが申請内容の誤りを発見することを安全境界にしない。
- 各taskの初期要求と直接関係しない事項は、同じtask、branch、PRへ追加せず、別のGitHub Issueへ登録するだけとし、追加調査、編集、test、委譲、commit、push、PR作成へ着手しない。逸脱した場合は実行中のサブエージェントも止め、取り繕う追加修正やcleanupを行わず、現在の差分を報告する。
- 発見Issueのうちユーザーへ確認するのは`priority:P0`と`priority:P1`だけとし、それらも専門用語だけに偏らず、何が問題で何を行うかをユーザーが理解できる短い言葉で説明し、そのIssueへの明示許可を得るまで着手しない。`priority:P2`、`priority:P3`、`priority:info`はIssueへの記録だけとする。
- 文書ごとのSHAを完了ブロッカーにしない。製品artifactを一意に識別するhashは記録できるが、内容評価の代用にしない。
- 外部authority値を推測しない。publisher、certificate、対応OS、保証値が未指定なら、公開・対応表明・mutationを行わないfail-closed動作を要件化する。
- lockfileと既存package managerを守る。検索は`rg`を優先する。
- 変更後は対象に最も近いformatter、check、testを実行する。testが0件の結果をPASSにしない。
- UI変更は実画面、Windows固有動作は実Windowsで確認する。環境上確認できない項目をPASSと報告しない。
- 画面キャプチャを取得した事実だけをUI評価の証拠にしない。各画像では可視色・文言・状態・配置を意味単位で棚卸しし、正本または他platformとの対応を説明できない色・表示差を1件でも残したままPASSにしない。可能な項目はpixel/UI Automationで機械判定し、その結果と同じ最新画面をレビューする。

## 完了判定と確認証拠

- Codexは、最新revisionに対する各要求と受入条件を、その条件が要求する実行環境と観測点で実際に確認した再現可能な証拠がある場合だけ`PASS`または`verified`と判定する。未実行、環境不足、権限不足、結果未取得、旧revisionだけの証拠は`未確認（INCONCLUSIVE）`とし、確認できなかったことを問題がなかったことへ読み替えてはならない。
- localの静的検査、mock、契約test、build、実装者自身のreviewは、それぞれが直接観測した範囲だけの証拠とする。GitHub上のworkflow挙動、remote mutation、Release、実OS、実画面、外部service等をlocalで直接確認できない場合、その制約と未確認項目を明記し、local証拠で代用してはならない。
- workflowまたはCI制御を変更する場合、localで再現可能な因果的に異なる有限経路をPR作成前に列挙し、呼出元から最終結果までを各経路の観測点で実行して全件PASSにする。未実行、`FAIL`、`INCONCLUSIVE`が1件でもある間、Actionsを試験代わりにするpush後のPR作成を禁止する。GitHubでしか観測できないevent起動、artifact転送、checkのApp identity、branch protectionだけを実PRの確認対象として残せる。
- Actionsの基盤・制御失敗後は、その失敗を再現して旧revisionを拒否し修正revisionを受理するlocal回帰testがPASSするまで、修正版PRの作成または同一PRを再起動する更新を禁止する。静的な文字列確認や部品単体testだけで、呼出元の入力配線を含む経路確認を代用してはならない。
- 要求または受入条件に`FAIL`、`INCONCLUSIVE`、未実行、未取得の証拠、未解決finding、未完了の依存作業が1件でもある間、Codexは作業全体を「完了」「要求達成」「全項目PASS」と報告せず、Issueを`status:review`へ進めず、closure reportを完成扱いしない。独立評価も証拠の欠落をPASSへ変更してはならない。
- 報告では`実装済み`、`local検証済み`、`remote/実環境で検証済み`、`未確認（INCONCLUSIVE）`、`失敗（FAIL）`、`未統合`を区別する。一部だけが完了した場合は、その完了範囲と未完了範囲を同じ報告内で明示する。
- 実環境確認にPRのmergeまたは`main`の変更が必要な場合、Codexは停止して必要な確認操作と未確認項目を報告し、ユーザー本人の操作を待つ。Release、その他の外部mutation、追加権限またはユーザー操作が必要で許可されていない場合も、許可済み範囲の終了時点で停止する。未確認の受入条件を、許可のない実環境操作で完了証拠へ変えてはならない。

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
- Codexは次の全項目をchatで宣言し、宣言後のユーザー本人による明示許可を待たなければならない。宣言自体や過去タスクの許可を、今回の許可と解釈してはならない。

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

- 許可は宣言したbranch、path、base SHA、owned scope、操作、期限にだけ有効とする。base、scope、path、操作、統合方法、予定時間が変化または超過した場合、あるいはユーザーが取消した場合は直ちに停止し、変更点を宣言して再許可を待つ。
- 許可後のfetchは、宣言した`origin/feat/next` refと完全SHAの取得に限定する。fetchが`FETCH_HEAD`と共有object database等を変更することを前提とし、作成直前にremote SHAが宣言値と同一であることを再確認する。
- 一時branch名は一意な`codex/<task>`、canonical worktree pathは`/home/salty/code/codex_info_v2-wt-<task>`とする。既存local/remote branch、既存path、symlink、別worktreeと衝突する場合は作成せず停止する。

### ownership、実装、検証、統合

- 1 taskにつき一時branch 1本、一時worktree 1個とする。同じfile/pathにwriterを1人だけ割り当て、owned pathsと変更禁止範囲を宣言する。
- `AGENTS.md`、workflow、lockfile、共通仕様、要件台帳等のcross-cutting fileは排他的所有とし、同時に別のwritable taskを走らせてはならない。ユーザー通常worktreeの同じpathにdirty/untracked変更がある、ownershipが重複する、または一時worktreeに不明差分が現れた場合は停止する。
- サブエージェントは宣言済み一時worktreeとowned pathsだけを扱う。サブエージェントによるbranch/worktreeの作成・削除、ref/config/remote操作、commit、push、PR操作を禁止し、管理は主担当だけが行う。
- Codexは宣言した最小のformatter、check、testを実行し、0件のtestをPASSにしてはならない。既往障害、security、cross-cutting governance等で独立判断が必要な場合だけfresh evaluatorを使う。
- commitはユーザーが明示許可した場合に限り、owned filesだけをstageして行う。pushとPR作成も宣言に含まれ明示許可された場合だけ行い、Codexが作るPRのbaseは`feat/next`に限定する。
- Codexは、ユーザーから依頼または許可を受けた場合も、いかなるPRもmergeせず、auto-mergeを設定または解除しない。この禁止に例外はなく、merge操作はユーザー本人だけが行う。Codexは`codex/<task> -> feat/next`のPR作成・更新と作業証拠のcommentを行える。PRのapprove、ready化、closeおよびworkflowのapproveまたはrerunは、exact targetと操作についてユーザーの明示許可がある場合だけ実施できる。
- CodexはPRのURL、base/headの完全SHA、変更file、検証結果、未確認事項、`main`へ統合した場合の影響を提示し、ユーザーが変更と動作を確認できる状態でmerge前に停止する。Codex自身の実装・検証・review結果を、ユーザーによる統合判断の代替にしてはならない。
- pushまたはPR作成が許可されていない作業を「統合済み」または「完了」と報告してはならない。実装済み、local検証済み、未統合を区別して報告する。
- pushまたはPR作成直前に`origin/feat/next`の完全SHAを再確認する。宣言baseから進んでいる場合は、旧SHA、新SHA、競合し得るowned pathsを報告して停止し、rebase、merge、reset、cherry-pick、stash、force pushを行わず再許可を待つ。
- `codex/<task> -> feat/next`はtrusted `feat-integration.yml`が完全なPR差分を有限ownerへ分類し、関係するremote qualityだけを実行して、最新headの`feat-acceptance`を成功させるまで統合しない。この経路ではversion、candidate、Release、tag、branch refをmutationしない。`feat/next -> main`は同じ分類正本を使用するが、required `acceptance`・`version-prepared`、version準備、選択build、Release前gateを別のmain経路で所有し、ユーザーだけがReleaseへ進む判断を行う。feat向けtriggerをmain向けtriggerの単純な拡張にしてはならない。

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

## サブエージェントのコスト・速度ガバナンス

- 目的は総Codex消費量とwall-clock timeを同時に削減することであり、サブエージェント使用自体を成功指標にしてはならない。短期・決定論的・逐次的な作業は主担当がlocal commandで処理し、サブエージェントを使わない。
- 委譲は、独立した並列作業、専門的な独立判断、または低コスト監視によって、調整・待機・再読込を含むend-to-end総コストか完了時間が改善すると事前に説明できる場合だけ行う。固定のmodel familyを理由に委譲してはならない。
- 主担当SOLがサブエージェントの完了だけを待つactive turnや短周期pollingを継続してはならない。SOL側に有用な並行作業がなく、passive waitがSOL消費を発生させないと確認できない場合は、そもそも委譲しない。
- 委譲時は`fork_turns = "none"`、最小のtask-local context、限定owned scope、1つの有用なvalidation gate、最小出力を使用する。raw logや会話全履歴を渡さず、同じrevision・入力・役割・modelでの重複実行や、timeoutだけを理由とする再試行を禁止する。
- 主担当はサブエージェントと同じ調査・実装・監視を重複して行わない。利用可能なterminal結果を保持し、raw command evidenceをagent summaryより優先する。証拠と矛盾するverdictは無効とする。
- コスト削減効果は比較可能なend-to-end実測がある場合だけ主張する。利用量が取得できない場合は推測せず`unavailable`とし、wall-clockだけをtoken/cost削減の証拠にしない。
- 継続計測は製品deliveryと分離した、ユーザーが別途許可する評価作業として行う。計測のためだけにmodel、route、subagentをprobeせず、通常作業から自然に得られるtask分類、revision、solo/delegated、agent数、利用可能なusage、wall-clock、SOLのwait/poll回数、再作業、gate結果だけを最小量で収集する。
- 計測記録をこのrepositoryへ追加せず、製品taskのcritical pathへ入れない。比較可能なsampleが蓄積するまで一般的な節約効果を断定せず、評価方法自体のコストが便益を上回る場合は計測を停止してユーザーへ報告する。

## 設計整合性と実装方針

- 症状ごとに既存コードをコピー＆ペーストし、条件分岐・例外処理・別実装を継ぎ足す増改築を禁止する。変更前に正本、責務、状態遷移、データフロー、不変条件、失敗時の所有者を特定し、その全体設計に沿って実装する。
- 同じ製品機能を複数プラットフォームへ提供する場合、データ解釈、計算、表示意味、操作契約は一つの正本から導出する。UIフレームワーク固有コードは描画と入力のadapterに限定し、ユーザーの明示承認なしに独自仕様・独自画面・並行する計算ロジックを作らない。
- 既存の共通モデルまたは正本を拡張すれば解決できる問題に、第二のsource of truth、互換用コピー、場当たり的fallbackを追加しない。重複が既にある場合は、さらに分岐を足すのではなく責務境界を整理して収束させる。
- 不具合修正は、表示された症状だけを隠すパッチではなく、原因となった設計境界を修正する。例外的な分岐が必要な場合は、適用範囲と終了条件を有限の受入テストで固定する。
- reviewと完了判定では、変更行の局所的な正しさだけでなく、正本から最終表示・操作までの経路が一貫し、類似機能との不要な差異や新しい重複を生んでいないことを確認する。
