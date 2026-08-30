# Codex Info 製品要件

この文書は、WindowsクライアントとLinux側collector/APIの実装判断に必要な要件だけをまとめた正本である。
監査履歴、作業経過、文書SHA一覧は製品要件ではないため含めない。詳細なwire schemaは
`REST_API_V1.md`、データ保持は`DATA_PROTECTION_POLICY.md`、画面仕様は`WINDOWS_UX_SPEC.md`を参照する。

## 1. 製品境界

- WindowsクライアントはLinux側の利用状況、残量、履歴、実行中threadを読み取り表示する。
- 製品の通信は固定loopback API、利用者が選択したWSL、またはOpenSSH configのliteral `Host` aliasに限定する。
- password、token、private key、展開済みSSH接続情報を保存しない。保存できるのは再接続に必要な非秘密selectorだけである。
- telemetryは送信しない。更新確認だけは固定GitHub repository
  `salty919/codex_info_v2` の公開Release API/assetを読み取れる。これ以外の未登録の外向き通信、
  暗黙のsupport upload、raw diagnostic uploadを行わない。

## 2. 状態と表示

- Setup、未認証、通常、警告、取得失敗を区別する。取得失敗を未認証や正常emptyへ変換しない。
- 更新candidateが不完全、不正、世代不一致の場合は画面の一部だけを更新せず、最後の完全な表示を保持する。
- 同じ事実の表示所有者は1か所とし、残量、reset countdown、status、connection情報を言い換えて重複表示しない。
- 定期的なquota再取得は不完全な中間結果であり、次のローカル使用量取得が完了するまで前回コミット済みのモデル・履歴・thread表示を保持する。認証主体の変更または明示的なログアウト以外で、主画面を空の初期状態へ戻してはならない。
- 最小viewport、対応locale、keyboard、UIA、高contrast、text scaleで主要操作を失わない。root scrollで欠落を隠さない。
- Back、Close、Escape、再入、遅延callbackは世代tokenで一度だけ処理する。古いPID/HWND/generationへfocus、message、route変更を行わない。
- 起動モードは固定する。引数なし/`--port PORT`はdaemon+RESTのみ、`--ui`はdaemon+REST+X UI、`--ui --port PORT`は指定ポートで同じ動作をする。待受アドレスは常に127.0.0.1へ固定する。
- `--stop`は同一profileの完全に検証できたlock ownerだけへTERMを1回送り、lock解放を最大5秒待つ。lockが無ければ成功とし、lockがあるのにowner identityを証明できない場合、別ownerへの交代、timeout、signal失敗は何も削除せず失敗する。SIGKILLへ昇格しない。
- 公開引数は上記、`--stop`、`--help`/`--h`/`-h`だけとする。旧・未知・誤記・重複・欠落・逆順・余分な引数、範囲外portを、daemon、REST、UI、DB、lockを作る前に拒否する。
- CLIヘルプを含む利用者向け固定メッセージは、画面本体と同じ対応localeのi18nカタログから導出する。起動スクリプトへ単一言語の製品文言を複製しない。

## 3. 収集・API・live判定

- 同一profileのwriter、recorder、REST publisherはそれぞれ有効ownerを1つだけ持つ。旧lease、旧epoch、旧cycleの結果は公開しない。
- DBは履歴inventoryであり、実行中判定の単独根拠にしない。同一cycleで検証したprocess identityとrollout terminal stateの両方を用いる。
- live rolloutでUTF-8、JSON、event kind、task stateを検証できないrecordがあるcycleは拒否し、最後の完全snapshotを保持する。
- RESTはread-onlyである。未知route/method、不正header/schema、oversize requestからDB、settings、cursor、processを変更しない。
- GUIなしserverはwindow、Slint component、display backendを生成せず、明示したservice lifecycleで起動・停止・復旧する。

## 4. データ保護

- 履歴DB、verified backup、設定、Linux側履歴は、install、update、rollback、uninstall、restore失敗で削除しない。
- migration、restore、updateはcandidateを完全検証してからatomic switchする。検証またはswitch失敗時は旧世代だけをcurrentとして保持する。
- cursorはsource identityと結合し、rotate、truncate、replaceを区別する。古いoffsetによるskipと二重登録を防ぐ。
- backup作成または検証に失敗した場合は既存のverified世代をpruneしない。
- crash、reboot、再実行はjournalの同一operationを再開し、commit、publication、deleteを各1回以下にする。

## 5. Windows導入・更新・削除

- アプリ起動後の更新確認は通知だけを生成し、download、Setup起動、既存payload変更を行わない。
  新版がある場合だけ状態帯に更新操作を表示し、利用者がその操作を明示実行した後に限ってdownloadと
  標準GUI Setupを開始する。常設の更新ボタン、silent install、unattended apply、自動再起動を行わない。
- 更新候補は公開済み・非prereleaseの`windows-vX.Y.Z` Releaseだけから選び、同Releaseのexact-name
  installerとmanifestについてversion、URL authority、byte size、SHA-256を完全検証する。不一致、
  redirect逸脱、途中download、oversize、起動失敗は既存payloadを変更せず、部分fileを公開しない。
- install、update、rollback、uninstallは別transactionとして扱い、stage中のfile、shortcut、HKCU、Apps登録を成功状態として公開しない。
- update失敗時は旧payload、shortcut、registry、versionを起動可能な状態で保持する。初回install失敗時は未公開状態へ戻す。
- uninstallは設定と履歴を保持する。途中失敗は完全復元または再開可能なjournal状態のどちらかにし、部分削除を成功と表示しない。
- 同一install rootの同時操作は単一leaseで直列化する。PID再利用、foreign owner、reparse差替え、token変化を検出した操作はmutation 0とする。
- interactive/silent等のmode、exit code、対応Windows、architecture、署名者、version policyはrelease authority inputで決める。
  署名者authorityが未設定なら署名済みと表明せず、設定済みauthorityと不一致なら更新候補を拒否する。
  利用者が開始するunsigned OSS buildは、exact GitHub repository、release tag、manifest、size、SHA-256の
  検証を満たす場合だけ標準GUI Setupへ渡し、Windowsが示すpublisher警告を隠さない。

## 6. 配布・顧客向け表明

- Windows製品版とX版は単一のstable `X.Y.Z`を共有する。バイナリ影響ありのPRはmajor/minorを変更せず、mergeごとに
  自動採番処理がpatchを十進整数としてちょうど1増やす。patchからminorへ桁上がりさせず、`1.0.9`の次は
  `1.0.10`とする。major/minorは利用者の明示指示を要する別変更でだけ更新し、自動採番処理は変更しない。
  `Cargo.toml`、root packageの`Cargo.lock`、`windows-client/Directory.Build.props`が開始時点で同値でない場合、
  または期待元versionとmainが一致しない場合は、3ファイルを一つも変更せず停止する。
- `main`向けPRは、配布するRust/Windows binaryまたはinstaller/payloadが消費するsource、依存関係・lockfile、組込みasset、
  製品build・packaging入力を1件でも変更する場合を「バイナリ影響あり」、それ以外を「バイナリ影響なし」とする。
  workflow・CI検査・test・文書・repository ruleだけの変更はバイナリ影響なしとする。分類はこの二つだけとし、rename/copyは
  変更前後pathを両方判定する。eventのbase/head commitによる完全なGit差分を単一分類器へ渡し、空差分、未知path、
  欠損したrename/copy情報は分類結果を返さず、versionまたはRelease mutationを開始しない。
- `main`向けPRはsame-repositoryであればhead branch名を制限しない。trusted `pull_request_target`が、version追加前の
  利用者差分（H0）の全pathをDOCS・GOVERNANCE・LINUX_BACKEND・LINUX_UI・WINDOWSの有限ownerへ一度だけ分類する。
  選択ownerだけをimmutableな最終head（H1）で各1回実行し、非選択ownerは実行しない。CodeQLも同じ選択から
  actions・python・rust・csharpの必要言語だけを一度実行する。選択ownerのmissing/failure/cancel/skip、非選択ownerの実行、
  CodeQL言語の余分・欠落はすべて失敗とする。Windowsだけの変更はWINDOWSとcsharpだけ、governanceだけの変更は
  GOVERNANCEとactions/pythonだけを実行する。branch名、branchの作成元、`feat/next`との包含関係は品質選択へ使用しない。
- `main`のtrusted `pull_request_target`を唯一のpre-merge authorityとし、別dispatchへ品質判定を転送しない。
  workflow run名には固定schemaでPR番号、event head、event action、event時のdraft状態をGitHub eventから記録する。
  event時または開始時にdraftであるrun、開始時にclosedであるrun、開始時にcurrent PR headではなくなったrunはobserverとしてowner、分類、
  version mutationを0件にする。open・non-draft・current headだけをownerとする。event headを評価する通常経路はActions job名
  `version-prepared`と`acceptance`をrequired checkとする。workflowの
  `GITHUB_TOKEN`がversion commit H1をpushした経路だけは、同じH0 runがH1を評価し、H0 job checkがH1へ移らない分の
  `version-prepared`と`acceptance`をH1へ最終結果として各1回作る。この2件にはproducer run IDとattemptを同じ外部IDで記録するが、
  登録後のpoll、retry、URL・時刻・表示値の照合を行わない。`acceptance`は選択jobの結果だけを集約する。
  WINDOWS選択時はWindows job自身が実Windows評価後にrelease candidateを作り、
  Linuxだけの変更ではWindows評価・candidateを生成しない。live repository ruleの再監査、選択済み製品testの再実行、
  branch名allowlist、上記2件以外のcheck登録を追加しない。
- バイナリ影響ありPRだけ、品質確認を開始する前にPR branch上のversion 3ファイルをexact next patchへ自動更新する。
  versionがbaseのH0は完全差分を1回分類し、PR、H0、producer run ID、attemptを固定trailerに持つH1 commitをnon-force pushして、
  同じrunが保存済み選択を使ってH1のownerを各1回実行する。H1 commit自身がmappingを持つため、push後にrunがcancelされても
  H0とH1の対応を失わない。後からH1/H2 eventが起動した場合は、base以降でversionを最後に変更したfirst-parent commitが管理3ファイル
  だけのbyte-exact自動更新で、最終headでもその3ファイルが不変な場合だけ、その3 pathを差分から除外する。tipが正規trailerを持つ
  生成H1の`synchronize` eventだけownerを再実行しない。identityがない同じbytesの手動commitは通常どおり評価し、H2の後続利用者commitも
  除外しない。event内の分類器は1回だけ使用する。これによりH2でも生成versionをLinux ownerへ誤分類しない。
  バイナリ影響なしPRはversionを変更しない。
- Release判定はmerged `closed`と`Main PR quality` completedの二信号を受け、どちらが先でも同じfinal headへ収束する。quality未完了時の
  `closed`とPR未merge時のcompletedはmutation 0で終了し、後着信号が再評価する。workflow runの非成功はMain Qualityの失敗を正本とし、
  Release側へ同じ赤を追加しない。final headを評価した生成producerとdirect runの全attemptを集合化し、draft・stale・generated observerを
  除外する。本来のattemptにmissing、failure、cancelその他の非成功が1件でもあれば、同じheadの後続rerunまたはreopen successで
  上書きせず、新headまで公開をHOLDする。旧成功runへfallbackしない。
- 成功authorityの`windows-quality`が`skipped`ならcandidateは0件だけを許可し、`success`なら同じrun、attempt、PR、final head、versionに
  結び付く既存candidateをexact 1件要求する。期限切れ・削除を含む0件、複数、malformed、別identityは失敗とする。candidateは増やさず、
  既存のSetupとmanifestを持つ1件の名前へidentityを追加する。merge後にPRを再分類せず、quality test/build/CodeQLも再実行しない。
  公開jobだけをversion tag単位で直列化し、lock取得後にPR/final head、全attempt、candidate、tag、Release、assetsを1回再取得する。
  tagとReleaseがともに不存在の場合だけDraftを作って2資産をupload後に公開し、完全一致のpublished状態だけを成功済みno-opとする。
  orphan tag、Releaseだけの存在、Draft、partial、targetまたはasset不一致は自動修復せず失敗し、自動retry・cleanupを行わない。
- PR由来のcheckout、script、workflow、artifactを、repository contents・checks・Releaseへのwrite権限を持つjobで実行しない。
  write権限を持つ採番jobはtrusted baseだけをcheckout・実行し、PR headはGit object dataとしてだけ読む。
  same-repository headへexact 1 commitをnon-force pushし、競合pushはGit自身のnon-fast-forward拒否に任せてreadbackやretryを行わない。
  H1 check作成jobもsourceをcheckoutせず、trusted runの出力だけを入力にする。Releaseのread-only解決jobはGitHub objectとrun状態だけを読み、
  write jobはsourceをcheckout・実行せず、解決済みcandidateとlock取得後に再取得したremote状態だけを入力にする。
- 選択ownerからCodeQL言語が導出されるPRではその言語だけをmerge必須gateとし、critical/high findingをdismissやworkflow無効化で
  通過させない。CodeQL言語が選択されないPRとmerge後pushではCodeQL AnalyzeとAutobuildを実行せず、active code-scanning rulesetの
  設定はworkflow内で再監査しない。外部AI findingsが
  provider側の未対応modelで継続失敗する場合は、そのAI機能だけをrepository単位で無効化できるが、選択済みCodeQL、
  code-scanning alerts、required acceptanceは維持する。
- Codex code reviewはPRの変更が確定した最新headに対して`@codex review`を1回だけ起動する補助レビューとする。
  古いheadの結果や未解決かつnon-outdatedのP0/P1をready判定へ流用せず、独自API key workflowを追加しない。
  Codex reviewはCodeQL、required acceptance、必要な承認の代替にしない。
- `windows-vX.Y.Z` ReleaseはSetupとmanifestを非公開Draftへuploadしてから公開する。途中失敗を公開済み成功へ変換しない。
- release artifactはsource、lockfile、実payload、license/notice、署名、version、対象platformを一つのrelease identityで追跡する。
- publisher名、certificate、対応OS build、RPO/RTO、accessibility適合、support窓口を根拠なしに推測しない。
- authority inputがないclaimは「保証なし」「未対応」とし、認証済み、対応済み、測定済みと表示しない。
- recovery journalと顧客共有support bundleを分離する。support exportは明示操作、allowlist、owner-only ACL、秘密0を必須とし、自動送信しない。
- customer guideとdeveloper READMEを分離し、顧客手順にrepository clone、Cargo build、`run.sh`を通常導線として要求しない。

### Workflowに残す確認の理由

| 確認 | 実際に到達するcaseと必要動作 | 確認しない場合の被害 | 上流保証と重複しない理由 |
| --- | --- | --- | --- |
| same-repository | fork PRではversionを書かず失敗する | 書込み不能または誤ったheadを対象にする | GitHubはfork PRを許可する |
| 完全Git差分とowner対応 | add/delete/rename/copyを両端まで分類し、未知pathは失敗する | 必要ownerの評価が欠落する | GitHubは製品ownerを知らない |
| trusted結果集約 | base版gateだけがselected success・non-selected skipを判定する | PRが判定scriptを無効化して未評価mergeできる | owner jobがsourceを評価することと、結果oracleの信頼性は別責務 |
| version 3ファイル | binary PRだけbaseからexact next patchへ同期更新する | Rust・lockfile・Windowsのversionが分裂する | GitHubは製品versionを保証しない |
| non-force push | version生成中にheadが進んだ場合はpushを拒否する | 利用者commitの上書きまたはstale commit追加 | Gitのnon-fast-forward拒否を唯一のrace authorityとして使う |
| 生成H1の必須2 check | `GITHUB_TOKEN` push後も同じrunでH1を評価し、H1へ最終結果を各1件作る | token起因pushは次runを起動せず、H1のrequired checkが永久に欠落する | ActionsのH0 job checkはworkflowが追加したH1へ移らない |
| event authority identity | job開始前cancelを含め、event時draft、event head、action、PRを復元し、draft・closed・stale eventをowner対象から外す | draftのcancelを評価失敗と誤認する、stale headを評価する、またはmerge後のbranchへ未統合H1をpushする | job出力は開始前cancelでは存在せず、runの通常metadataだけではPR head epochを復元できない |
| 生成H1 commit identity | H1 commitと同じGit objectにH0 producer run/attemptを記録し、正規生成H1だけowner再実行を抑止する | push後cancelでH0/H1対応を失う、または手動version commitを未評価にする | checkやartifactを後から作る方式にはpushとの間に原子的でない空白が残る |
| selected job結果 | selectedはsuccess、non-selectedはskipped以外を失敗にする | 未評価mergeまたは無関係な評価失敗が起きる | Actionsはowner選択の意味を知らない |
| 実Windows評価 | WINDOWS選択時だけWindows runnerでinstaller/UIを評価する | 壊れたWindows配布物を公開する | LinuxやGitHubはWindows製品動作を保証しない |
| final-head attempt集合 | current final headを実際に評価した全non-observer attemptを見て、1件の失敗もsame-head successで上書きしない | rerunの偶然のsuccessが未修正の評価失敗を隠し、旧candidateを公開する | latest成功だけでは過去attemptのfailure barrierを表現できない |
| Windows jobとcandidate | Windows skippedは0件、successはown-run candidate exact 1を要求する | 非Windowsで不要なReleaseを動かす、または期限切れ・欠落candidateを黙って無視する | run successだけではWindowsの選択有無とartifact保存状態を区別できない |
| merged/completed二信号 | quality先行とmerge先行のどちらも、両条件が揃った信号だけを公開候補にする | merge直後に未完了qualityを失敗扱いする、またはquality先行時に公開信号を失う | 両event間にGitHubの完了順序保証はない |
| tag単位lock後の公開状態 | 同じtagの二信号を直列化し、lock取得後の完全不存在だけ作成、完全一致publishedだけno-opにする | 二重公開、orphan tagの流用、不完全Draftの自動修復 | Release作成と複数asset uploadは一つのtransactionではない |
| local台帳schema | PR前は要求・範囲・オラクル・実装状態の欠落だけを失敗にし、remote証拠は完了時に`--final`で確認する | PRでしか得られない証拠をPR前に要求する循環で作業が停止する | local実装確認とGitHub実挙動は異なる観測点を持つ |

この表の4列を満たさない形式検査、identity再照合、poll、retry、証拠専用artifact、同一run内の二次分類は追加しない。
公開lock取得後の競合判定に必要な単回remote再取得を、汎用readbackやretryへ拡張しない。

## 7. 有限の検証規則

- 要件は重複を作らず、同じ観測結果は同じ要件へ統合する。
- 各境界軸の値を最低1回検査する。複数軸は、共有状態または既知の因果関係がある組だけを有限のrisk-based caseにする。
- 全直積、N倍、N二乗、N階乗のcase生成を行わない。
- 文書ごとのSHA一致を合否条件にしない。合否は観測結果、失敗時保持、副作用数、参照整合で決める。
- 製品artifactのhashは評価対象を一意に識別するために1つ記録できるが、内容評価の代用にしない。

## 8. 完了条件

- 上記要件と参照先仕様の間に、同じ入力へ異なる必須結果を要求する矛盾がない。

## グラフ表示の正本と受入境界

グラフの値の意味、期間境界、欠測・未使用区間、残量とモデル使用量の
独立性は `docs/WINDOWS_UX_SPEC.md` のグラフ意味論を正本とする。X版と
Windows版の実装は同じ `tests/fixtures/graph_delayed_quota.json` を入力に
し、fixture内の固定期待値（期間数、累積SOL、遅延して届く残量、未観測区間）
をそれぞれ独立に検証する。片方の描画結果や補間ヘルパーをもう片方の
期待値生成に使うことは禁止する。仕様・fixture・実装のいずれかが不一致、
または実機証跡が欠ける場合は合格ではなく保留とする。
- 自動検査は実行されたtestが0件でないことを確認する。UI変更は実画面、Windows固有動作は実Windowsで確認する。
- 未確認の外部authority値を創作してPASSにしない。値がない場合のfail-closed動作を検証する。

## 「正しい表示」の判定（必須不変条件）

次の条件をすべて満たした状態だけを正しい表示と定義する。

1. 同一の認証主体でquotaを定期更新している間は、前回の完全なモデル使用量・履歴・threadを保持する。更新途中の欠測を空、0、未取得へ置き換えない。
2. `reset_at`がサービスのrolling値として移動しても同一期間を維持する。実際の期間切替は、quotaの回復または期間境界を示す観測がある場合だけ新期間へ移す。
3. モデル使用量（ドル・token）と残量（%）は別観測として扱う。残量観測がない時間帯をモデル使用量から逆算せず、遅れて届いた残量観測はその時刻へ反映する。
4. 同一観測時刻に異なる`reset_at`の行が存在する場合、100%かつ使用量0の単独行だけをrolling更新の候補とし、それ以外の残量を選択中期間へ混在させない。行順で値を上書きせず、期間境界も同一時刻の単独行だけで短縮しない。
5. 過去期間はその期間に属する全モデル系列を累積値で描画し、未使用区間は専用の未使用帯として表示する。SOL/TERRA/LUNAのいずれかを0や欠測へ黙って変換しない。
6. 明示的なログアウトまたは認証主体変更だけが可視状態を消去する。通信失敗・quota更新中・local収集中は最後の完全表示を保持し、失敗状態は別途表示する。
7. X版とWindows版は同一fixtureの固定oracle（期間数、期間境界、累積SOL、遅延残量、未観測区間）を独立に満たす。どれか一つでも不一致、実機証跡欠落、またはテスト未実行なら合格ではなく保留とする。
8. 製品バージョンはメイン画面に一度だけ表示し、子ウインドウのタイトルやボタンへ重複表示しない。値はX版・Windows版とも同じリリースversion authorityから導出する。
9. Windows版の初回起動では、health・status・detailsの最初の完全な世代が揃うまで内容領域を表示せず、固定レイアウト上にスピナーを表示する。途中の部分世代を順番に描画して画面をばたつかせてはならない。初回取得が失敗した場合はスピナーを解除し、失敗状態と再試行手段を表示する。
10. X版の初回起動でも、認証済み状態へ遷移した後のquota・local usage・threadの最初の完全な世代が揃うまで主画面の内容領域を公開せず、ヘッダー（製品バージョンを含む）を固定したままスピナーを表示する。local収集またはapp-serverが失敗した場合はスピナーを解除し、最後の完全表示または失敗状態を表示する。
11. X版の起動ウィンドウは主モニターの可視デスクトップ内へ配置し、別モニターや負座標へ出して利用者から見えない状態にしてはならない。起動成功は、可視範囲内の実ウィンドウと内容の実画面で確認する。
12. `--ui` のdaemon/REST起動に失敗しても、X版のGUIを消失・即時終了させず、接続失敗と再試行手段を表示する。
