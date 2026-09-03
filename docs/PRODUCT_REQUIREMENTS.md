# Codex Info 製品要件

この文書は、WindowsクライアントとLinux側collector/APIの実装判断に必要な要件だけをまとめた正本である。
監査履歴、作業経過、文書SHA一覧は製品要件ではないため含めない。詳細なwire schemaは
`REST_API_V1.md`、データ保持は`DATA_PROTECTION_POLICY.md`、画面仕様は`WINDOWS_UX_SPEC.md`を参照する。

## 1. 製品境界

- WindowsクライアントはLinux側の利用状況、残量、履歴、実行中threadを読み取り表示する。
- 製品の通信は固定loopback API、利用者が選択したWSL、またはOpenSSH configのliteral `Host` aliasに限定する。
- password、token、private key、展開済みSSH接続情報を保存しない。保存できるのは再接続に必要な非秘密selectorだけである。
- telemetryは送信しない。更新確認とLinux bundleの更新に必要な読み取りだけは固定GitHub repository
  `salty919/codex_info_v2` の公開Release API/assetへ行える。これ以外の未登録の外向き通信、
  暗黙のsupport upload、raw diagnostic uploadを行わない。

## 2. 状態と表示

- Setup、未認証、通常、警告、取得失敗を区別する。取得失敗を未認証や正常emptyへ変換しない。
- 更新candidateが不完全、不正、世代不一致の場合は画面の一部だけを更新せず、最後の完全な表示を保持する。
- 同じ事実の表示所有者は1か所とし、残量、reset countdown、status、connection情報を言い換えて重複表示しない。
- 定期的なquota再取得は不完全な中間結果であり、次のローカル使用量取得が完了するまで前回コミット済みのモデル・履歴・thread表示を保持する。認証主体の変更または明示的なログアウト以外で、主画面を空の初期状態へ戻してはならない。
- 最小viewport、対応locale、keyboard、UIA、高contrast、text scaleで主要操作を失わない。root scrollで欠落を隠さない。
- Back、Close、Escape、再入、遅延callbackは世代tokenで一度だけ処理する。古いPID/HWND/generationへfocus、message、route変更を行わない。
- 顧客向けLinux入口はinstalled launcher `$HOME/.local/bin/codex-info`一つとする。引数なし/`--start`は更新収束後にmanaged daemonを有効化・起動する。`--ui`は同じ収束後の検証済みinstalled payloadを実行し、local generation/payload identityが完全だがservice/details接続だけが失敗した場合もX UIの失敗状態と再試行手段を表示する。generation不整合またはforeign/unknown ownerの`SAFE_BLOCKED`ではpayload UIを実行しない。`--stop`は同一bootだけ有効な停止意図を記録してmain serviceだけを停止し、timerと次回bootの自動起動を維持する。`--disable-autostart`はmain serviceとupdate timerを停止・無効化し、`--remove`はその両unitを解除する。`--status`はread-onlyの完全identityを表示し整合時だけ成功、`--update`は同じ更新authorityを即時実行、`--help`は副作用なしとする。
- installed launcherは未知・混在・重複引数と`--port`をmutation前に拒否する。service・開発・E2Eが直接使うpayload `codex_info`のCLIは、引数なし/`--port PORT`のdaemon+REST、`--ui`/`--ui --port PORT`、検証済みownerだけを停止する`--stop`、`--help`/`--h`/`-h`に限定し、待受アドレスを常に127.0.0.1へ固定する。payloadの`--stop`はSIGKILLへ昇格せず、lock identityを証明できない場合は何もsignal・削除しない。
- repositoryの`run.sh`はarchive内の`run.sh`とbyte-identicalなruntime launcherであり、installed launcherへ委譲するだけとする。Cargo build、`target/`、source tree、別binaryへのfallbackを行わない。
- CLIヘルプを含む利用者向け固定メッセージは、画面本体と同じ対応localeのi18nカタログから導出する。起動スクリプトへ単一言語の製品文言を複製しない。

## 3. 収集・API・live判定

- 同一profileのresident serviceをquota、local usage、historyの唯一のauthority/writerとする。service内のrecorderとREST publisherは同じ有効owner、lease、epoch、cycleに従い、旧世代の結果を公開しない。
- DBは履歴inventoryであり、実行中判定の単独根拠にしない。同一cycleで検証したprocess identityとrollout terminal stateの両方を用いる。
- live rolloutでUTF-8、JSON、event kind、task stateを検証できないrecordがあるcycleは拒否し、最後の完全snapshotを保持する。
- RESTはread-onlyである。未知route/method、不正header/schema、oversize requestからDB、settings、cursor、processを変更しない。
- GUIなしserverはwindow、Slint component、display backendを生成せず、明示したservice lifecycleで起動・停止・復旧する。
- resident serviceは一つの完全candidateからimmutable details generationを公開する。Linux / WindowsのMain、Graph、Threadsはstrict validation済みの単一`GET /v1/details` generationだけをatomic表示rootとして消費し、SQLite/JSONL/app-serverの再収集、値の再計算、複数row/endpointのmergeを行わない。公開するread-only endpointはreadiness用`GET /v1/health`と表示正本`GET /v1/details`だけとする。
- `GET /v1/health`の200はresident serviceがread-only snapshot requestを受理できるreadinessと、Cargo/Windowsの単一authorityから導出したstable product versionを表す。Linux launcherはsystemd `MainPID`、process starttime、実行fileのdevice/inode/SHA-256、profile lock identity、port 8787のsocket inodeとそのPIDのFD対応をhealth取得の前後で同一と確認し、manifestのsource generationへ結合する。PID、listener、health body、versionのいずれか単独ではcurrent ownerと判定しない。既知の旧Codex Info ownerだけを一度交代し、unknown・foreign・malformed ownerはsignalせず30秒以内に`SAFE_BLOCKED`とする。Linux / Windows clientは自身と異なるversion、version欠落、unknown healthをdetails読取りへ進めず、旧serviceのlast-goodをcurrent表示にしない。認証済み、data `state=ready`、最新収集成功を意味しない。認証開始・確認はcontrol-onlyであり、control応答を表示dataとしてcommitせず、その後に受理した新しいdetails generationだけが画面を変えられる。

## 4. データ保護

- 履歴DB、verified backup、設定、Linux側履歴は、install、update、rollback、uninstall、restore失敗で削除しない。
- canonical ChatGPT account identityは、owner-onlyかつstableに読めた`CODEX_HOME/auth.json`のexact `tokens.account_id`と前後2回の`account/read`、その間の`account/updated`世代不変で確定する。emailを保存scopeへ使わず、raw AccountKey、email、tokenをpath、DB、metadata、log、RESTへ保存しない。
- 保存単位を`(OS user, ProfileScopeId, AccountScopeId, StorageEpoch)`とし、accountごとに`history/accounts/v1/<AccountScopeId>/epoch-<StorageEpoch>/usage_history.sqlite3`へ物理分離する。同一accountへの再切替は同じStorageEpoch/DBを再利用し、別accountのrow、writer lock、backup、Session checkpoint/marker/gapを混合しない。
- logout、account切替、identity/metadata/DB検証失敗では旧accountのdurable dataとSession sourceを保持し、current公開rootだけをstrict emptyの`auth_required`、`initializing`、または`error`へ切り替える。旧account DBへのfallbackと自動migrationを行わない。
- history canonicalization、candidate reject、UI表示を理由に既存のraw SQLite rowをrewrite/deleteしない。競合または境界不明時はraw DBとlast-good publicationを保持する。
- migration、restore、updateはcandidateを完全検証してからatomic switchする。検証またはswitch失敗時は旧世代だけをcurrentとして保持する。
- cursorはsource identityと結合し、rotate、truncate、replaceを区別する。古いoffsetによるskipと二重登録を防ぐ。
- process startおよび各identity boundaryでは既存Sessionを現在EOFへfresh baselineし、同一CollectorEpochの次cycle以後に検証した完全append rangeだけを現accountへ帰属する。A→B→AでもAの旧cursorを再開せず、partial tail、変更prefix、legacy、未帰属rangeをどのaccountにも推測帰属しない。
- backup作成または検証に失敗した場合は既存のverified世代をpruneしない。
- crash、reboot、再実行はjournalの同一operationを再開し、commit、publication、deleteを各1回以下にする。
- local session inventoryは既存のdepth・file数・1 file・1 line上限を先に検証し、mtime nanoseconds降順・canonical path降順のwhole-file prefixから最大2GiBだけを収集する。全inventoryの2GiB超過だけでLinux detailsを停止しない。
- 2GiBから外れた古いsessionは、その完全fingerprintのusageとrecorded markerがSQLiteへ同一transactionでcommit済みで、fresh DB readback・path/identity再検査・Codex open-FD不在を満たす場合だけruntimeに1 fileずつ削除できる。未記録・legacy・変更済み・active・selected sessionと、履歴DB/backup/reset hint/recovery JSONLは保持する。

## 5. Linux bundle導入・自動更新・削除

- Linux bundleはbyte-identicalなruntime launcher `run.sh`、payload `codex_info`、`install.sh`、main/update serviceとtimer、license/notice、version/target情報を含む。外部manifestとarchive内manifestはbyte-identicalで、launcherを含む全regular memberのpath、mode、size、SHA-256を同じcandidate identityへ結合する。install後の固定入口はlauncher `$HOME/.local/bin/codex-info`、payload `$HOME/.local/bin/codex_info`、persistent installer `$HOME/.local/libexec/codex-info-install.sh`、manifest `$HOME/.local/share/codex-info/manifest.json`、unitは`$HOME/.config/systemd/user/`配下のexact 3名である。各固定入口はowner-onlyな`$HOME/.local/share/codex-info/generations/<version>-<source_sha>-<manifest_sha256>/`のregular fileをatomic `current` symlink経由で参照する。repository、Cargo、元bundle、`target/`へ依存しない。
- manual、launcher startup、service `ExecStartPre`、timer、bootは一つのRelease resolver/installer authorityを使用する。timerは`OnActiveSec=5min`、`OnUnitActiveSec=1h`、`AccuracySec=1s`で、公開後の次回発見上限を1時間1秒とする。`codex-info.service`の起動前reconcileも同じauthorityを通るため、停止していたdaemonを新しく起動した場合に旧versionを無条件で開始しない。新版がなく同versionでも、local世代が不整合なら検証済みReleaseから修復する。
- resolverは固定HTTPS API `https://api.github.com/repos/salty919/codex_info_v2/releases?per_page=100`の単一応答だけを読み、100件ならpagination曖昧として拒否する。非draft・非prereleaseでexact `windows-vSemVer`の最高stable Releaseを選び、`CodexInfo.WindowsClient.Setup.exe`、`CodexInfo.WindowsClient.update.json`、`codex-info-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz`、対応`.sha256`、対応`.manifest.json`のexact 5 asset、canonical URL、size、digestを検証する。導入済みversionより古いcandidateへdowngradeしない。同versionは外部manifest identityが一致すればno-op、不一致ならlocal coherenceのverified repairとして扱う。candidateがなくてもlocal世代が完全ならその旧世代を使用できるが、localも不完全なら`SAFE_BLOCKED`とする。
- draft、prerelease、partial、extra asset、malformed Release、version/tag/asset identity、size/digest、HTTPS GitHub asset redirect境界の不一致はcandidateを拒否する。redirectは最大3回でAPI/repository Release asset境界だけを許可し、Release取得30秒、各asset取得300秒、更新全体20分を上限とする。archiveはchecksum、external/internal manifest同一性、全member名・mode・size・digestを完全検証するまでinstallation stateを変更しない。
- installはowner-onlyな`.install.lock`をnonblockingで取得し、`install-transaction.json`へ各phaseをatomic write・file fsync・parent directory fsyncしてからlegacy退避、entrypoint link、candidate publish、`current` switch、activation、health確認へ進む。stale transactionは同じoperationを一意にresumeまたはrollbackしてから新operationを開始する。`current` switch後を含む失敗では旧generationへ戻し、旧serviceをsystemd管理下で復旧してから終了する。更新開始時に存在した使用可能な旧世代をunmanaged processとして残さない。
- generation/current導入前の既知predecessor flat installは、旧manifestのexact schemaと記録済みpath/size/SHA-256、固定配置、owner/mode、同一sourceのbinary/installer/3 unitを全て再検証できる場合だけ一方向bootstrap入力にできる。旧installerがL1を保持したまま新serviceを最初に起動する区間は`ExecStartPre`が同じlockを待たず、検証済みflat payloadの起動を妨げない。lock解放後の最初のmanual/startup/timer/repository launcher操作は同Release archiveを取得・完全検証してgeneration/current/launcherへ移行し、旧flat状態をcompleted terminalにしない。検証不能なflat file、repository binary、`target/`、version文字列だけをbootstrapやrollbackへ使わない。
- `desired_state=running`で完了したlauncher/startup/timer/update収束のterminal stateは、完全に検証した新世代がmanagedかつhealthyなA、または完全に検証した旧世代がmanagedかつhealthyなBのいずれかだけとする。`desired_state=stopped|disabled|removed`の操作は、対応するservice/timer/unit状態、listener不在、保持対象、local generation整合をread-backした場合だけ別の正常な非稼働terminalとする。unknown/foreign/malformed listenerまたはlockを安全に識別できない場合だけ、何も停止・上書きせず30秒以内に明示的`SAFE_BLOCKED`で終了できる。この安全例外を成功やA/Bへ読み替えず、次のmanual/startup/timer triggerを妨げない。manual/startupは20分30秒、control RPCは30秒、local validate/publishは60秒、stopは20秒、readinessは30秒、rollbackは60秒以内で必ずterminalになる。
- installer/controlはL1 `.install.lock`だけ、resident runtimeはL2 profile recorder lockの後に必要な場合だけL3 account writer lockを取得する。installerはL2/L3を取得せず、serviceはL1を取得しない。systemd start/restartは`--no-block`で要求後にread-only pollする。これを唯一のlock順序として、launcher、startup、timer、removeのcycleを作らない。
- `codex-info.service`はunexpected exitを`Restart=always`、`RestartSec=5s`、60秒中2回のstart limitで1回だけ自動復旧させ、2回目はfailed latchとする。launcherまたはupdateによる明示activationだけがresetして新epochを開始する。recorder workerは1秒ごとに監視し、2秒以内に死亡を検知する。admitted accountでは新規rowが0件でも各scheduled generationをtransaction commitし、commit確認後だけowner-only `recorder-state.json`の`last_commit_unix`を更新する。全write stateのheartbeat `updated_at_unix`とadmitted accountの`last_commit_unix`はfuture skewを拒否しfreshness上限150秒とする。account未確定時は`idle_no_account`とし、架空のpartition/commitを作らない。
- recorderの最初の一時障害は同callbackでretryせず`degraded`へ進み、次のscheduled cycle（60秒以内）で一度だけ再試行する。2回目またはfatal errorは非0終了してsystemdへ復旧を委ねる。停止区間のSession usageだけは検証済みsource cursorからbounded backfillできるが、quota/残量を補間・複製しない。gapは`pending/recovered/confirmed/rejected`のledgerへ記録し、回収不能をsource identity/cursorで確定した`confirmed`だけを既存`history_gaps`へ公開する。
- 利用者操作は`codex-info --update`、`codex-info --status`、`codex-info --stop`、`codex-info --disable-autostart`、`codex-info --remove`を使用する。raw `systemctl`は診断用で、直接stopは永続的な製品停止意図ではなく次の更新でmanaged runningへ正規化され得る。`--remove`はmain/update unitだけを停止・無効化・解除し、installed generation、launcher、installer、manifest、履歴DB、verified backup、reset hint、gap/recorder/control state、Codex session JSONL、設定を削除しない。

## 6. Windows導入・更新・削除

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

## 7. 配布・顧客向け表明

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
- `main`のtrusted `pull_request_target`を唯一のRelease品質authorityとし、別dispatchへ品質判定を転送しない。
  workflow run名には固定schemaでPR番号、event head、event action、event時のdraft状態をGitHub eventから記録する。
  event時または開始時にdraftであるrun、開始時にclosedであるrun、開始時にcurrent PR headではなくなったrunはobserverとしてowner、分類、
  version mutationを0件にする。open・non-draft・current headだけをownerとする。`version-prepared`と`acceptance`は同じrunの
  Release品質結果であり、branch mergeのrequired checkにはしない。workflowの`GITHUB_TOKEN`がversion commit H1をpushした経路は、
  同じH0 runが保存済み選択でH1を評価し、生成commitの固定trailerとproducer runからH0/H1を対応付ける。
  H1へcustom `version-prepared`・`acceptance` checkを登録せず、poll、retry、URL・時刻・表示値の照合も行わない。
  `acceptance`はmain向けに選択jobの結果だけを集約し、失敗時はRelease公開をHOLDするがmergeを禁止しない。
  Windowsを含むmain向けrelease candidateでは、Windows job自身が実Windows評価後にrelease candidateを作る。
  Linux-only変更も、Linux archiveを既存`windows-vX.Y.Z` ReleaseへWindows Setup/manifestと同居させるため、
  main向けrelease candidateではWindows評価・candidateを追加で実行する。`feat/next`向け通常integrationは差分選択を維持し、
  選択された実owner job、CodeQL、distributionだけをadvisoryに実行する。feat向け`selected-quality`集約と`feat-acceptance`、
  Windows release candidateは生成しない。実jobの失敗は赤のまま表示するがmergeを禁止しない。live repository ruleの再監査、
  選択済み製品testの再実行、branch名allowlist、custom check登録を追加しない。
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
  Release mutationだけはcurrent repository限定の短命GitHub App installation token（Contents write / Workflows write）を使い、
  解決、lock後再検証、artifact取得はread-onlyの組み込みtokenを使う。
- PR由来のcheckout、script、workflow、artifactを、repository contents・checks・Releaseへのwrite権限を持つjobで実行しない。
  write権限を持つ採番jobはtrusted baseだけをcheckout・実行し、PR headはGit object dataとしてだけ読む。
  same-repository headへexact 1 commitをnon-force pushし、競合pushはGit自身のnon-fast-forward拒否に任せてreadbackやretryを行わない。
  H1 custom check作成jobは置かない。Releaseのread-only解決jobはGitHub objectとrun状態だけを読み、
  write jobはsourceをcheckout・実行せず、解決済みcandidateとlock取得後に再取得したremote状態だけを入力にする。
- 選択ownerからCodeQL言語が導出されるPRではその言語だけを実行し、main向けRelease品質ではanalysis成功とcritical/high finding不在を
  公開条件とする。CodeQL失敗はworkflowへ表示するがbranch mergeを禁止しない。CodeQL言語が選択されないPRとmerge後pushでは
  CodeQL AnalyzeとAutobuildを実行せず、active code-scanning rulesetの
  設定はworkflow内で再監査しない。外部AI findingsが
  provider側の未対応modelで継続失敗する場合は、そのAI機能だけをrepository単位で無効化できるが、選択済みCodeQL、
  code-scanning alerts、Release品質acceptanceは維持する。
- Codex code reviewはPRの変更が確定した最新headに対して`@codex review`を1回だけ起動する補助レビューとする。
  古いheadの結果や未解決かつnon-outdatedのP0/P1をready判定へ流用せず、独自API key workflowを追加しない。
  Codex reviewはCodeQL、Release品質acceptance、必要な承認の代替にしない。
- `windows-vX.Y.Z` ReleaseはSetupとmanifestを非公開Draftへuploadしてから公開する。途中失敗を公開済み成功へ変換しない。
- Linux coreはtargetを`x86_64-unknown-linux-gnu`へ固定し、同じstable version・final source SHAのarchive、SHA-256 checksum、manifestを既存の`windows-vX.Y.Z` ReleaseへWindowsのSetup/manifestと同居させる。別Linux tag/channelを作らない。
- Linux bundleの互換性は、bundle manifestに記録した実測`glibc_minimum`（glibc minimum）を満たすことだけを表明する。manifestのtargetまたは実測minimumが欠落・不一致なら候補を公開・導入せず、他のdistribution、architecture、署名済み、publisher検証済みの対応を表明しない。
- Linux bundle archiveはbyte-identicalなruntime launcher `run.sh`、`codex_info`、`codex-info.service`、永続化する`install.sh`、`codex-info-update.service`、`codex-info-update.timer`、license/noticeを含み、version/target情報と対応するchecksum/manifestを同じcandidate identityへ結び付ける。導入時は`run.sh`を`$HOME/.local/bin/codex-info`、`install.sh`を`$HOME/.local/libexec/codex-info-install.sh`からcurrent generationへ参照させる。顧客の通常導線はRelease assetのdownload、checksum検証、extract、bundle内scriptによるinstall、installed launcher、自動更新、source-bound health、removeだけで完結し、repository clone、Cargo build、source treeの`run.sh`を要求しない。
- Linux bundleのinstall、update、reinstall、removeまたはその失敗は、導入binary、永続installer、履歴DB、verified backup、`history/usage_reset_hint.json`、Codex session JSONL、設定を削除しない。removeはdaemon/updateのuser service/unitだけを解除し、部分導入を成功と表示しない。
- release artifactはsource、lockfile、実payload、license/notice、署名、version、対象platformを一つのrelease identityで追跡する。
- publisher名、certificate、対応OS build、RPO/RTO、accessibility適合、support窓口を根拠なしに推測しない。
- authority inputがないclaimは「保証なし」「未対応」とし、認証済み、対応済み、測定済みと表示しない。
- recovery journalと顧客共有support bundleを分離する。support exportは明示操作、allowlist、owner-only ACL、秘密0を必須とし、自動送信しない。
- customer guideとdeveloper READMEを分離し、顧客手順にrepository clone、Cargo build、source treeの`run.sh`を通常導線として要求しない。配布・導入済みruntime launcherは通常導線に含める。

### Workflowに残す確認の理由

| 確認 | 実際に到達するcaseと必要動作 | 確認しない場合の被害 | 上流保証と重複しない理由 |
| --- | --- | --- | --- |
| same-repository | fork PRではversionを書かず失敗する | 書込み不能または誤ったheadを対象にする | GitHubはfork PRを許可する |
| 完全Git差分とowner対応 | add/delete/rename/copyを両端まで分類し、未知pathは失敗する | 必要ownerの評価が欠落する | GitHubは製品ownerを知らない |
| main Release結果集約 | main向けbase版gateだけがselected success・non-selected skipを判定する | 未評価または無関係なcandidateを公開する | owner jobがsourceを評価することと、公開oracleの信頼性は別責務 |
| feat選択品質 | 完全差分から選択した実owner job・CodeQL・distributionだけを実行し、各結果をそのまま表示する | reviewerが対象revisionの実品質結果を確認できない | GitHubは製品ownerと必要言語を知らない |
| version 3ファイル | binary PRだけbaseからexact next patchへ同期更新する | Rust・lockfile・Windowsのversionが分裂する | GitHubは製品versionを保証しない |
| non-force push | version生成中にheadが進んだ場合はpushを拒否する | 利用者commitの上書きまたはstale commit追加 | Gitのnon-fast-forward拒否を唯一のrace authorityとして使う |
| event authority identity | job開始前cancelを含め、event時draft、event head、action、PRを復元し、draft・closed・stale eventをowner対象から外す | draftのcancelを評価失敗と誤認する、stale headを評価する、またはmerge後のbranchへ未統合H1をpushする | job出力は開始前cancelでは存在せず、runの通常metadataだけではPR head epochを復元できない |
| 生成H1 commit identity | H1 commitと同じGit objectにH0 producer run/attemptを記録し、正規生成H1だけowner再実行を抑止する | push後cancelでH0/H1対応を失う、または手動version commitを未評価にする | checkやartifactを後から作る方式にはpushとの間に原子的でない空白が残る |
| main selected job結果 | mainのRelease候補ではselectedはsuccess、non-selectedはskipped以外を失敗にする | 未評価または無関係なartifactを公開する | Actionsはowner選択と公開可否の意味を知らない |
| 実Windows評価 | WINDOWS選択時（main向けrelease candidateではLinux product ownerにも追加選択）だけWindows runnerでinstaller/UIを評価する | 壊れたWindows配布物を公開する | LinuxやGitHubはWindows製品動作を保証しない |
| final-head attempt集合 | current final headを実際に評価した全non-observer attemptを見て、1件の失敗もsame-head successで上書きしない | rerunの偶然のsuccessが未修正の評価失敗を隠し、旧candidateを公開する | latest成功だけでは過去attemptのfailure barrierを表現できない |
| Windows jobとcandidate | Windows skippedは0件、successはown-run candidate exact 1を要求する | 非Windowsで不要なReleaseを動かす、または期限切れ・欠落candidateを黙って無視する | run successだけではWindowsの選択有無とartifact保存状態を区別できない |
| merged/completed二信号 | quality先行とmerge先行のどちらも、両条件が揃った信号だけを公開候補にする | merge直後に未完了qualityを失敗扱いする、またはquality先行時に公開信号を失う | 両event間にGitHubの完了順序保証はない |
| tag単位lock後の公開状態 | 同じtagの二信号を直列化し、lock取得後の完全不存在だけ作成、完全一致publishedだけno-opにする | 二重公開、orphan tagの流用、不完全Draftの自動修復 | Release作成と複数asset uploadは一つのtransactionではない |
| local台帳schema | PR前は要求・範囲・オラクル・実装状態の欠落だけを失敗にし、remote証拠は完了時に`--final`で確認する | PRでしか得られない証拠をPR前に要求する循環で作業が停止する | local実装確認とGitHub実挙動は異なる観測点を持つ |

この表の4列を満たさない形式検査、identity再照合、poll、retry、証拠専用artifact、同一run内の二次分類は追加しない。
公開lock取得後の競合判定に必要な単回remote再取得を、汎用readbackやretryへ拡張しない。

## 8. 有限の検証規則

- 要件は重複を作らず、同じ観測結果は同じ要件へ統合する。
- 各境界軸の値を最低1回検査する。複数軸は、共有状態または既知の因果関係がある組だけを有限のrisk-based caseにする。
- 全直積、N倍、N二乗、N階乗のcase生成を行わない。
- 文書ごとのSHA一致を合否条件にしない。合否は観測結果、失敗時保持、副作用数、参照整合で決める。
- 製品artifactのhashは評価対象を一意に識別するために1つ記録できるが、内容評価の代用にしない。

## 9. 完了条件

- 上記要件と参照先仕様の間に、同じ入力へ異なる必須結果を要求する矛盾がない。

## グラフ表示の正本と受入境界

グラフの値の意味、期間境界、欠測・未使用区間、残量とモデル使用量の
独立性は `docs/WINDOWS_UX_SPEC.md` のグラフ意味論を正本とする。X版と
Windows版の実装は同じ `tests/fixtures/graph_delayed_quota.json` を入力に
し、fixture内の固定期待値（期間数、累積SOL、遅延して届く残量、未観測区間）
をそれぞれ独立に検証する。片方の描画結果や補間ヘルパーをもう片方の
期待値生成に使うことは禁止する。仕様・fixture・実装のいずれかが不一致、
または実機証跡が欠ける場合は合格ではなく保留とする。
- shared rollover fixtureはperiod AからBへの境界で残量/累積額が
  `100% / $1 → 41% / $323.674247`となる固定oracleを持つ。これと
  `graph_delayed_quota`、既存のhistory、rolling、delayed、gap、no-history、REST、Windows回帰を、
  同じproduct revisionに対して有限な既存testへ統合して確認する。別workflow gateや全直積を追加せず、
  どれか未実行・不一致ならそのrevisionを合格にしない。
- 自動検査は実行されたtestが0件でないことを確認する。UI変更は実画面、Windows固有動作は実Windowsで確認する。
- 未確認の外部authority値を創作してPASSにしない。値がない場合のfail-closed動作を検証する。

### 履歴・snapshot validationを残す理由

| validation | 防ぐcase | 外した場合に起きること |
| --- | --- | --- |
| profile-owned DB scope・`timestamp/reset_at`で検証したcycle・minuteの一致と、実resetを分へ丸めた境界bucketの後続cycle所有 | 別profile/cycle/minuteの混在、または正常な旧cycle末尾と新cycle先頭の同一分 | 異なる観測を一つに混ぜる、または正常なreset直後のdetailsを停止する |
| distinct non-null quotaが最大1個。不一致minuteだけを公開対象外にする | 同一minuteの相反する残量 | 任意のquotaを選ぶ、または1分の異常で全表示を停止する |
| 既存cumulative vectorのcomponentwise-dominance。不成立minuteだけを公開対象外にする | 非比較vectorやcomponent別maxによる合成 | sourceに存在しない累積値を作る、または1分の異常で全表示を停止する |
| public candidateのraw/canonical duplicate拒否 | 同一canonical minuteの複数値 | Graphへ垂直変化またはsink依存の値を渡す |
| detailsのstrict schema/domain/header検証 | 欠落・未知・重複・stale generation | Main/Graph/Threadsが異なるrootを表示する |
| health readiness | listenerだけが存在しsnapshotを返せない状態 | Setupが利用不能serviceを接続済みと扱う |

`HistoryCanonicalizer`は同じprofile-owned DB read内で、値形状ではなく既存`timestamp/reset_at`のbounded
rolling規則から同一cycleと検証できたminuteのrowだけを対象にする。今回の回復に新しいpartition列や
永続CycleSeqを要求しない。quota観測のないbackfill reset群がquota確認済みcycleと重なる場合と、継続する
quota確認済みcycleの内部だけに存在するreset断片はperiod authorityにしない。
distinct non-null quotaが0または1個で、既存のcumulative vector
`(sol_dollars, terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens)`のうち全rowを
componentwiseに支配する値が一意に存在するときだけ、そのquota（0個なら`null`）とdominant vectorを
1 logical sampleにする。quota競合、非比較、dominant vector不存在となるminuteは値を推測せずそのminuteだけを
公開対象外にする。別cycle間の所属を一意にできない場合はcandidate全体をrejectしてlast-goodを保持する。
残量100%、7日窓、quota-onlyなど数値の形で除外せず、
component別max、last-row、null化、任意mergeを行わない。

## 「正しい表示」の判定（必須不変条件）

次の条件をすべて満たした状態だけを正しい表示と定義する。

1. 同一の認証主体でquotaを定期更新している間は、前回の完全なモデル使用量・履歴・threadを保持する。更新途中の欠測を空、0、未取得へ置き換えない。
2. `reset_at`がサービスのrolling値として移動しても同一期間を維持する。実際の期間切替は、quotaの回復または期間境界を示す観測がある場合だけ新期間へ移す。
3. モデル使用量（ドル・token）と残量（%）は別観測として扱う。残量観測がない時間帯をモデル使用量から逆算せず、遅れて届いた残量観測はその時刻へ反映する。
4. 同じprofile-owned DB read内で、既存`timestamp/reset_at`のbounded rolling規則から同一cycleと検証できた同じminuteのrowだけを`HistoryCanonicalizer`が1 logical sampleへ正規化する。quota観測を持たずquota確認済みcycleと重なるbackfill reset群と、継続cycleの内部だけにあるreset断片はperiod authorityにしない。実resetの秒をminute-startへ丸めて旧cycle末尾と新cycle先頭が同じ分になり、旧cycleがそこで終了して新cycleだけが後続分へ継続すると確認できる場合は、その境界分を新cycleへ一意に所属させる。distinct non-null quotaは最大1個、cumulative vectorは既存のcomponentwise-dominant値だけを採用し、同値duplicateは冪等に扱う。quota競合・非比較・dominant不存在はそのminuteだけを公開対象外とし、別cycle間の境界不明はcandidate全体をrejectする。100%・7日窓など数値の形では除外せず、UI/REST/Windowsでmerge/max/last/null化しない。
5. 過去期間はその期間に属する全モデル系列を累積値で描画し、未使用区間は専用の未使用帯として表示する。SOL/TERRA/LUNAのいずれかを0や欠測へ黙って変換しない。
6. 明示的なログアウトまたは認証主体変更だけが可視状態を消去する。通信失敗・quota更新中・local収集中は最後の完全表示を保持し、失敗状態は別途表示する。
7. X版とWindows版はshared rollover fixtureの`100% / $1 → 41% / $323.674247`と`graph_delayed_quota`を含む同一fixtureの固定oracle（期間数、期間境界、累積SOL、遅延残量、未観測区間）を独立に満たす。既存history/rolling/delayed/gap/no-history/REST/Windows回帰を同一revisionで確認し、どれか一つでも不一致、実機証跡欠落、またはテスト未実行なら合格ではなく保留とする。
8. 製品バージョンはメイン画面に一度だけ表示し、子ウインドウのタイトルやボタンへ重複表示しない。値はX版・Windows版とも同じリリースversion authorityから導出する。
9. Windows版の初回起動では、health readiness後に最初のstrict validation済み`/v1/details` generationが揃うまで内容領域を表示せず、固定レイアウト上にスピナーを表示する。control応答とのmerge、途中fieldの順番描画をせず、初回取得失敗時はスピナーを解除して失敗状態と再試行手段を表示する。
10. X版の初回起動でも、health readiness後に最初のstrict validation済み`/v1/details` generationが揃うまで主画面の内容領域を公開せず、ヘッダー（製品バージョンを含む）を固定したままスピナーを表示する。details取得が失敗した場合はスピナーを解除し、最後の完全表示または失敗状態を表示する。
11. X版の起動ウィンドウは主モニターの可視デスクトップ内へ配置し、別モニターや負座標へ出して利用者から見えない状態にしてはならない。起動成功は、可視範囲内の実ウィンドウと内容の実画面で確認する。
12. `--ui` のdaemon/REST起動に失敗しても、X版のGUIを消失・即時終了させず、接続失敗と再試行手段を表示する。
