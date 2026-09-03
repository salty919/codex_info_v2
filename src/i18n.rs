// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Startup-localized presentation helpers.
//!
//! The application keeps protocol and timestamp values language neutral.  This
//! module is the only owner of user-facing fixed copy, locale selection, and
//! timezone conversion.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use std::env;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Japanese,
    English,
    SimplifiedChinese,
    Korean,
    Spanish,
    French,
    German,
    Portuguese,
    Italian,
    Russian,
}

impl Language {
    pub const ALL: [Self; 10] = [
        Self::Japanese,
        Self::English,
        Self::SimplifiedChinese,
        Self::Korean,
        Self::Spanish,
        Self::French,
        Self::German,
        Self::Portuguese,
        Self::Italian,
        Self::Russian,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Japanese => "ja",
            Self::English => "en",
            Self::SimplifiedChinese => "zh-Hans",
            Self::Korean => "ko",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::Portuguese => "pt",
            Self::Italian => "it",
            Self::Russian => "ru",
        }
    }

    /// Localized command-line help.  Startup messages are product copy too;
    /// keeping them here prevents the shell launcher and the binary from
    /// silently diverging in language or launch semantics.
    pub const fn launch_help(self) -> &'static str {
        match self {
            Self::Japanese => "使用法: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (引数なし)       daemon+REST（127.0.0.1:8787）\n  --port PORT      daemon+RESTのポート（アドレスは127.0.0.1に固定）\n  --ui             daemon+REST + X UI\n  --ui --port PORT 指定ポートでdaemon+REST + X UI\n  --stop           常駐daemonを停止\n  --help, --h, -h  このヘルプを表示",
            Self::English => "Usage: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (no arguments)   daemon+REST (127.0.0.1:8787)\n  --port PORT      daemon+REST port (address fixed to 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI on the selected port\n  --stop           stop the resident daemon\n  --help, --h, -h  show this help",
            Self::SimplifiedChinese => "用法: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (无参数)         daemon+REST（127.0.0.1:8787）\n  --port PORT      daemon+REST端口（地址固定为127.0.0.1）\n  --ui             daemon+REST + X UI\n  --ui --port PORT 在指定端口启动daemon+REST + X UI\n  --stop           停止常驻daemon\n  --help, --h, -h  显示此帮助",
            Self::Korean => "사용법: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (인수 없음)      daemon+REST (127.0.0.1:8787)\n  --port PORT      daemon+REST 포트 (주소는 127.0.0.1로 고정)\n  --ui             daemon+REST + X UI\n  --ui --port PORT 지정 포트에서 daemon+REST + X UI\n  --stop           상주 daemon 중지\n  --help, --h, -h  이 도움말 표시",
            Self::Spanish => "Uso: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (sin argumentos) daemon+REST (127.0.0.1:8787)\n  --port PORT      puerto daemon+REST (dirección fija a 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI en el puerto indicado\n  --stop           detener el daemon residente\n  --help, --h, -h  mostrar esta ayuda",
            Self::French => "Usage : codex_info [--ui] [--port PORT] | --stop | --help\n\n  (aucun argument) daemon+REST (127.0.0.1:8787)\n  --port PORT      port daemon+REST (adresse fixée à 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI sur le port choisi\n  --stop           arrêter le daemon résident\n  --help, --h, -h  afficher cette aide",
            Self::German => "Aufruf: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (keine Argumente) daemon+REST (127.0.0.1:8787)\n  --port PORT      daemon+REST-Port (Adresse fest auf 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI am gewählten Port\n  --stop           residenten Daemon stoppen\n  --help, --h, -h  diese Hilfe anzeigen",
            Self::Portuguese => "Uso: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (sem argumentos) daemon+REST (127.0.0.1:8787)\n  --port PORT      porta daemon+REST (endereço fixado em 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI na porta escolhida\n  --stop           parar o daemon residente\n  --help, --h, -h  mostrar esta ajuda",
            Self::Italian => "Uso: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (nessun argomento) daemon+REST (127.0.0.1:8787)\n  --port PORT      porta daemon+REST (indirizzo fissato a 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI sulla porta scelta\n  --stop           arresta il daemon residente\n  --help, --h, -h  mostra questo aiuto",
            Self::Russian => "Использование: codex_info [--ui] [--port PORT] | --stop | --help\n\n  (без аргументов) daemon+REST (127.0.0.1:8787)\n  --port PORT      порт daemon+REST (адрес фиксирован: 127.0.0.1)\n  --ui             daemon+REST + X UI\n  --ui --port PORT daemon+REST + X UI на выбранном порту\n  --stop           остановить daemon\n  --help, --h, -h  показать эту справку",
        }
    }

    /// Localized help for the installed Linux launcher.  The launcher and the
    /// raw payload intentionally expose different option sets; `run.sh` asks
    /// the installed payload for this catalog instead of duplicating product
    /// copy in shell.
    pub const fn launcher_help(self) -> &'static str {
        match self {
            Self::Japanese => "使用法: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (引数なし), --start  stable版を確認し、管理対象daemonを有効化して起動\n  --ui               同じ確認後に管理対象daemonとX UIを起動\n  --stop             今回のbootではdaemonを停止（更新timerと次回bootは維持）\n  --disable-autostart daemonと更新timerを停止・無効化\n  --remove           unitを解除（導入済みprogramとprofile dataは保持）\n  --status           generation・owner・healthの整合を読取り専用で確認\n  --update           stable版の確認と更新を直ちに実行\n  --help             このヘルプを表示",
            Self::English => "Usage: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (no arguments), --start  check stable and enable/start the managed daemon\n  --ui                     start the verified managed daemon and X UI\n  --stop                   stop the daemon for this boot; keep updates and next boot\n  --disable-autostart       stop and disable the daemon and update timer\n  --remove                 detach units; keep the installed program and profile data\n  --status                 read-only generation, owner, and health consistency check\n  --update                 check stable and update immediately\n  --help                   show this help",
            Self::SimplifiedChinese => "用法: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  （无参数）, --start     检查stable版并启用、启动受管理daemon\n  --ui                   经同样检查后启动受管理daemon和X UI\n  --stop                 本次boot停止daemon；保留更新timer和下次boot\n  --disable-autostart     停止并禁用daemon和更新timer\n  --remove               移除unit；保留已安装程序和profile data\n  --status               只读检查generation、owner和health一致性\n  --update               立即检查stable版并更新\n  --help                 显示此帮助",
            Self::Korean => "사용법: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (인수 없음), --start   stable 버전을 확인하고 관리 daemon을 활성화·시작\n  --ui                   같은 확인 후 관리 daemon과 X UI 시작\n  --stop                 이번 boot에서 daemon 중지; 업데이트 timer와 다음 boot 유지\n  --disable-autostart     daemon과 업데이트 timer 중지·비활성화\n  --remove               unit 제거; 설치된 프로그램과 profile data 유지\n  --status               generation, owner, health 일치 여부를 읽기 전용으로 확인\n  --update               stable 버전 확인과 업데이트를 즉시 실행\n  --help                 이 도움말 표시",
            Self::Spanish => "Uso: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (sin argumentos), --start comprobar stable y activar/iniciar el daemon gestionado\n  --ui                         iniciar el daemon verificado y la X UI\n  --stop                       detener el daemon en este arranque; conservar actualizaciones y próximo arranque\n  --disable-autostart           detener y desactivar el daemon y el temporizador de actualización\n  --remove                     retirar las unidades; conservar programa instalado y datos del perfil\n  --status                     comprobar en modo lectura la coherencia de generation, owner y health\n  --update                     comprobar stable y actualizar ahora\n  --help                       mostrar esta ayuda",
            Self::French => "Usage : codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (sans argument), --start vérifier stable et activer/démarrer le daemon géré\n  --ui                      démarrer le daemon vérifié et la X UI\n  --stop                    arrêter le daemon pour ce démarrage ; conserver mises à jour et prochain démarrage\n  --disable-autostart        arrêter et désactiver le daemon et le minuteur de mise à jour\n  --remove                  retirer les unités ; conserver le programme installé et les données du profil\n  --status                  vérifier en lecture seule la cohérence generation, owner et health\n  --update                  vérifier stable et mettre à jour immédiatement\n  --help                    afficher cette aide",
            Self::German => "Aufruf: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (keine Argumente), --start stable prüfen und verwalteten Daemon aktivieren/starten\n  --ui                       geprüften Daemon und X UI starten\n  --stop                     Daemon für diesen Boot stoppen; Updates und nächsten Boot beibehalten\n  --disable-autostart         Daemon und Update-Timer stoppen/deaktivieren\n  --remove                   Units entfernen; installiertes Programm und Profildaten behalten\n  --status                   Konsistenz von generation, owner und health schreibgeschützt prüfen\n  --update                   stable prüfen und sofort aktualisieren\n  --help                     diese Hilfe anzeigen",
            Self::Portuguese => "Uso: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (sem argumentos), --start verificar stable e ativar/iniciar o daemon gerido\n  --ui                       iniciar o daemon verificado e a X UI\n  --stop                     parar o daemon neste boot; manter atualizações e próximo boot\n  --disable-autostart         parar e desativar o daemon e o timer de atualização\n  --remove                   remover as units; manter programa instalado e dados do perfil\n  --status                   verificar em modo somente leitura generation, owner e health\n  --update                   verificar stable e atualizar imediatamente\n  --help                     mostrar esta ajuda",
            Self::Italian => "Uso: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (nessun argomento), --start controlla stable e abilita/avvia il daemon gestito\n  --ui                          avvia il daemon verificato e la X UI\n  --stop                        arresta il daemon per questo boot; mantiene aggiornamenti e boot successivo\n  --disable-autostart            arresta e disabilita il daemon e il timer di aggiornamento\n  --remove                      rimuove le unit; mantiene programma installato e dati del profilo\n  --status                      controlla in sola lettura generation, owner e health\n  --update                      controlla stable e aggiorna subito\n  --help                        mostra questo aiuto",
            Self::Russian => "Использование: codex-info [--start | --ui | --stop | --disable-autostart | --remove | --status | --update | --help]\n\n  (без аргументов), --start проверить stable и включить/запустить управляемый daemon\n  --ui                       запустить проверенный daemon и X UI\n  --stop                     остановить daemon до следующего boot; сохранить обновления\n  --disable-autostart         остановить и отключить daemon и timer обновлений\n  --remove                   удалить units; сохранить программу и данные профиля\n  --status                   только проверить согласованность generation, owner и health\n  --update                   немедленно проверить stable и обновиться\n  --help                     показать эту справку",
        }
    }

    fn from_primary(primary: &str) -> Option<Self> {
        Some(match primary {
            "ja" => Self::Japanese,
            "en" => Self::English,
            // The product deliberately uses one simplified Chinese catalog;
            // regional and script tags are presentation hints, not catalogs.
            "zh" => Self::SimplifiedChinese,
            "ko" => Self::Korean,
            "es" => Self::Spanish,
            "fr" => Self::French,
            "de" => Self::German,
            "pt" => Self::Portuguese,
            "it" => Self::Italian,
            "ru" => Self::Russian,
            _ => return None,
        })
    }

    /// Detect a locale from the POSIX precedence chain. Empty values are
    /// skipped; the first non-empty unsupported/C/POSIX value is English and
    /// does not fall through to a lower-priority variable.
    pub fn detect_from_values(
        lc_all: Option<&str>,
        lc_messages: Option<&str>,
        lang: Option<&str>,
    ) -> Self {
        for value in [lc_all, lc_messages, lang] {
            let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
                continue;
            };
            return Self::parse_locale(value).unwrap_or(Self::English);
        }
        Self::English
    }

    pub fn detect() -> Self {
        // A non-Unicode environment value is an invalid first candidate. We
        // intentionally do not let a lower-priority variable change it.
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            match env::var_os(key) {
                None => continue,
                Some(value) => {
                    let Some(value) = value.to_str() else {
                        return Self::English;
                    };
                    if value.trim().is_empty() {
                        continue;
                    }
                    return Self::parse_locale(value).unwrap_or(Self::English);
                }
            }
        }
        Self::English
    }

    fn parse_locale(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized == "c"
            || normalized == "posix"
            || normalized.starts_with("c.")
            || normalized.starts_with("c@")
        {
            return None;
        }
        let normalized = normalized
            .split_once('.')
            .map_or(normalized.as_str(), |(head, _)| head)
            .split_once('@')
            .map_or(normalized.as_str(), |(head, _)| head)
            .replace('-', "_");
        let primary = normalized.split('_').next().unwrap_or_default();
        Self::from_primary(primary)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodKind {
    Weekly,
    Monthly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKey {
    FontFamily,
    WindowUnauthenticated,
    PlanUnset,
    PlanFree,
    PlanEnterprise,
    PlanEducation,
    UsageStatus,
    Graph,
    LegalNotices,
    Running,
    ModelThreads,
    Other,
    Details,
    NoRunningThreads,
    LegalCode,
    LegalWarranty,
    LegalLicense,
    LegalFont,
    LegalProtocol,
    LegalSchema,
    LegalDependencies,
    LegalThirdParty,
    LegalDetails,
    LegalDistribution,
    Close,
    ActiveThreads,
    Context,
    Instruction,
    Tokens,
    Model,
    Input,
    Cached,
    Output,
    Retry,
    UsageTrend,
    Remaining,
    GraphTokenDescription,
    GraphDollarDescription,
    NoRecords,
    ConnectAccount,
    AuthBrowserInstructions,
    AuthManaged,
    OpenAuthPage,
    StartAuth,
    Checking,
    CheckAuth,
    AuthCli,
    NoHistory,
    On,
    Off,
    Connecting,
    UpdatingUsage,
    CheckingAuthStatus,
    AuthenticatedLoading,
    UnauthenticatedStart,
    AuthUrlIssued,
    IssuingAuthUrl,
    AuthUrlOpenFailed,
    CannotFetchUsage,
    CannotDisplayStatus,
    QuotaNearlyGone,
    QuotaLow,
    ResetWithinDay,
    LastUpdated,
    PartialHistoryThreads,
    PartialHistory,
    PartialThreads,
    MainRole,
    SubRole,
    ParentNotRunning,
    ParentPrefix,
    CurrentSuffix,
    DeadlinePrefix,
    EstimatePrefix,
    SoonReset,
    FixedLimitNone,
    QuotaRemaining,
    MonthlyQuotaRemaining,
    UsageLimit,
    DollarMetric,
    TokenMetric,
}

impl TextKey {
    pub const ALL: &'static [Self] = &[
        Self::FontFamily,
        Self::WindowUnauthenticated,
        Self::PlanUnset,
        Self::PlanFree,
        Self::PlanEnterprise,
        Self::PlanEducation,
        Self::UsageStatus,
        Self::Graph,
        Self::LegalNotices,
        Self::Running,
        Self::ModelThreads,
        Self::Other,
        Self::Details,
        Self::NoRunningThreads,
        Self::LegalCode,
        Self::LegalWarranty,
        Self::LegalLicense,
        Self::LegalFont,
        Self::LegalProtocol,
        Self::LegalSchema,
        Self::LegalDependencies,
        Self::LegalThirdParty,
        Self::LegalDetails,
        Self::LegalDistribution,
        Self::Close,
        Self::ActiveThreads,
        Self::Context,
        Self::Instruction,
        Self::Tokens,
        Self::Model,
        Self::Input,
        Self::Cached,
        Self::Output,
        Self::Retry,
        Self::UsageTrend,
        Self::Remaining,
        Self::GraphTokenDescription,
        Self::GraphDollarDescription,
        Self::NoRecords,
        Self::ConnectAccount,
        Self::AuthBrowserInstructions,
        Self::AuthManaged,
        Self::OpenAuthPage,
        Self::StartAuth,
        Self::Checking,
        Self::CheckAuth,
        Self::AuthCli,
        Self::NoHistory,
        Self::On,
        Self::Off,
        Self::Connecting,
        Self::UpdatingUsage,
        Self::CheckingAuthStatus,
        Self::AuthenticatedLoading,
        Self::UnauthenticatedStart,
        Self::AuthUrlIssued,
        Self::IssuingAuthUrl,
        Self::AuthUrlOpenFailed,
        Self::CannotFetchUsage,
        Self::CannotDisplayStatus,
        Self::QuotaNearlyGone,
        Self::QuotaLow,
        Self::ResetWithinDay,
        Self::LastUpdated,
        Self::PartialHistoryThreads,
        Self::PartialHistory,
        Self::PartialThreads,
        Self::MainRole,
        Self::SubRole,
        Self::ParentNotRunning,
        Self::ParentPrefix,
        Self::CurrentSuffix,
        Self::DeadlinePrefix,
        Self::EstimatePrefix,
        Self::SoonReset,
        Self::FixedLimitNone,
        Self::QuotaRemaining,
        Self::MonthlyQuotaRemaining,
        Self::UsageLimit,
        Self::DollarMetric,
        Self::TokenMetric,
    ];
}

/// Fixed command-line and service lifecycle copy. CLI messages are kept in a
/// separate finite catalog because they are not rendered inside a window,
/// while still sharing the same process locale authority as the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliTextKey {
    InvalidPort,
    ServiceExecutableUnavailable,
    ServiceStartFailed,
    ServiceCleanupFailed,
    ServiceStateUnavailable,
    ServiceExitedBeforeHealthy,
    ServiceNotHealthy,
    ServiceAlreadyOwned,
    ServiceReused,
    StopLockUnavailable,
    StopLockInvalid,
    StopSignalFailed,
    StopOwnerChanged,
    StopTimeout,
    StopUnsupported,
}

impl CliTextKey {
    pub const ALL: [Self; 15] = [
        Self::InvalidPort,
        Self::ServiceExecutableUnavailable,
        Self::ServiceStartFailed,
        Self::ServiceCleanupFailed,
        Self::ServiceStateUnavailable,
        Self::ServiceExitedBeforeHealthy,
        Self::ServiceNotHealthy,
        Self::ServiceAlreadyOwned,
        Self::ServiceReused,
        Self::StopLockUnavailable,
        Self::StopLockInvalid,
        Self::StopSignalFailed,
        Self::StopOwnerChanged,
        Self::StopTimeout,
        Self::StopUnsupported,
    ];
}

#[derive(Clone, Debug)]
pub struct I18n {
    language: Language,
    timezone: Tz,
}

impl I18n {
    pub fn detect() -> Self {
        Self {
            language: Language::detect(),
            timezone: detect_timezone(),
        }
    }

    pub fn from_parts(language: Language, timezone: Tz) -> Self {
        Self { language, timezone }
    }

    pub const fn language(&self) -> Language {
        self.language
    }

    pub const fn timezone(&self) -> Tz {
        self.timezone
    }

    pub fn cli_text(&self, key: CliTextKey) -> &'static str {
        let index = CliTextKey::ALL
            .iter()
            .position(|candidate| *candidate == key)
            .expect("all CLI translation keys must be listed");
        match self.language {
            Language::Japanese => CLI_JA[index],
            Language::English => CLI_EN[index],
            Language::SimplifiedChinese => CLI_ZH[index],
            Language::Korean => CLI_KO[index],
            Language::Spanish => CLI_ES[index],
            Language::French => CLI_FR[index],
            Language::German => CLI_DE[index],
            Language::Portuguese => CLI_PT[index],
            Language::Italian => CLI_IT[index],
            Language::Russian => CLI_RU[index],
        }
    }

    pub fn text(&self, key: TextKey) -> &'static str {
        use TextKey::*;
        match self.language {
            Language::Japanese => match key {
                FontFamily => "Noto Sans JP",
                WindowUnauthenticated => "アカウント未接続 — プラン未設定",
                PlanUnset => "プラン未設定",
                PlanFree => "無料",
                PlanEnterprise => "エンタープライズ",
                PlanEducation => "教育",
                UsageStatus => "利用状況",
                Graph => "グラフ",
                LegalNotices => "法的通知",
                Running => "稼働",
                ModelThreads => "モデル別スレッド",
                Other => "その他",
                Details => "詳細",
                NoRunningThreads => "実行中のスレッドなし",
                LegalCode => "Codex Info の独自コードと文書: GPL-3.0-only",
                LegalWarranty => {
                    "本ソフトウェアは無保証です。GPL-3.0-only の条件で再配布できます。"
                }
                LegalLicense => "ライセンス本文: LICENSE",
                LegalFont => "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
                LegalProtocol => "プロトコルとAPI: Apache-2.0 / Copyright 2025 OpenAI",
                LegalSchema => "Codex生成スキーマ: Apache-2.0 / Copyright 2025 OpenAI",
                LegalDependencies => "Slint と Rust 依存クレートは各上流ライセンスを保持します。",
                LegalThirdParty => "第三者ライセンス: MIT / BSD-3-Clause / その他",
                LegalDetails => "詳細: THIRD_PARTY_NOTICES.md と LICENSES/",
                LegalDistribution => "バイナリ配布時は各依存の LICENSE/NOTICE を同梱してください。",
                Close => "閉じる",
                ActiveThreads => "実行中のスレッド",
                Context => "コンテキスト使用率",
                Instruction => "指示",
                Tokens => "トークン",
                Model => "モデル",
                Input => "入力",
                Cached => "キャッシュ",
                Output => "出力",
                Retry => "再試行",
                UsageTrend => "利用状況の推移",
                Remaining => "残量",
                GraphTokenDescription => "時間ごとのトークン使用量（モデル別） / 残量%",
                GraphDollarDescription => "時間ごとの累積消費ドル（モデル別） / 残量%",
                NoRecords => "記録なし",
                ConnectAccount => "Codexアカウントを接続",
                AuthBrowserInstructions => {
                    "ブラウザで認証を完了してください。完了後、自動的に確認します。"
                }
                AuthManaged => "認証はCodexが管理します。このアプリは認証情報を保存しません。",
                OpenAuthPage => "認証ページを開く",
                StartAuth => "認証を開始",
                Checking => "確認中…",
                CheckAuth => "認証状態を確認",
                AuthCli => "Codex CLIの認証状態を利用します。",
                NoHistory => "履歴なし",
                On => "オン",
                Off => "オフ",
                Connecting => "Codex app-serverへ接続しています…",
                UpdatingUsage => "利用状況を更新しています…",
                CheckingAuthStatus => "認証状態を確認しています…",
                AuthenticatedLoading => "認証済みです。利用量を取得しています…",
                UnauthenticatedStart => "未認証です。認証を開始してください。",
                AuthUrlIssued => "認証URLを発行しました。「認証ページを開く」を押してください。",
                IssuingAuthUrl => "認証URLを発行しています…",
                AuthUrlOpenFailed => "認証URLを開けませんでした。",
                CannotFetchUsage => {
                    "利用状況を取得できません。Codex app-serverへの接続を確認してください。"
                }
                CannotDisplayStatus => "状態を表示できません。",
                QuotaNearlyGone => "残り利用枠はほぼありません。",
                QuotaLow => "残り利用枠が少なくなっています。",
                ResetWithinDay => "リセット前後24時間です。",
                LastUpdated => "最終更新",
                PartialHistoryThreads => {
                    "利用枠は更新しました。履歴とスレッドは前回値を保持しています。"
                }
                PartialHistory => "利用枠は更新しました。履歴は前回値を保持しています。",
                PartialThreads => "利用枠は更新しました。スレッド表示は前回値を保持しています。",
                MainRole => "メイン",
                SubRole => "サブ",
                ParentNotRunning => "親スレッドは現在非実行",
                ParentPrefix => "親",
                CurrentSuffix => "（現在）",
                DeadlinePrefix => "期限",
                EstimatePrefix => "概算",
                SoonReset => "まもなくリセット",
                FixedLimitNone => "固定上限なし",
                QuotaRemaining => "残り利用枠",
                MonthlyQuotaRemaining => "月間残り利用枠",
                UsageLimit => "利用枠",
                DollarMetric => "ドル",
                TokenMetric => "トークン",
            },
            Language::English => english_text(key),
            Language::SimplifiedChinese => chinese_text(key),
            Language::Korean => korean_text(key),
            Language::Spanish => spanish_text(key),
            Language::French => french_text(key),
            Language::German => german_text(key),
            Language::Portuguese => portuguese_text(key),
            Language::Italian => italian_text(key),
            Language::Russian => russian_text(key),
        }
    }

    pub fn font_family(&self) -> &'static str {
        self.text(TextKey::FontFamily)
    }

    pub fn format_elapsed(&self, now: i64, timestamp: Option<i64>) -> String {
        let Some(timestamp) = timestamp else {
            return "—".into();
        };
        if DateTime::<Utc>::from_timestamp(timestamp, 0).is_none() {
            return "—".into();
        }
        let age = now.saturating_sub(timestamp).max(0);
        let (amount, unit) = if age < 60 {
            (age, Unit::Second)
        } else if age < 3_600 {
            let minutes = age / 60;
            let seconds = age % 60;
            return if seconds == 0 {
                self.unit_text(minutes, Unit::Minute)
            } else {
                format!(
                    "{}{}{}",
                    self.unit_text(minutes, Unit::Minute),
                    self.elapsed_separator(),
                    self.unit_text(seconds, Unit::Second)
                )
            };
        } else if age < 86_400 {
            let hours = age / 3_600;
            let minutes = (age % 3_600) / 60;
            return if minutes == 0 {
                self.unit_text(hours, Unit::Hour)
            } else {
                format!(
                    "{}{}{}",
                    self.unit_text(hours, Unit::Hour),
                    self.elapsed_separator(),
                    self.unit_text(minutes, Unit::Minute)
                )
            };
        } else {
            let days = age / 86_400;
            let hours = (age % 86_400) / 3_600;
            return if hours == 0 {
                self.unit_text(days, Unit::Day)
            } else {
                format!(
                    "{}{}{}",
                    self.unit_text(days, Unit::Day),
                    self.elapsed_separator(),
                    self.unit_text(hours, Unit::Hour)
                )
            };
        };
        self.unit_text(amount, unit)
    }

    pub fn format_period_remaining(&self, seconds: i64, kind: PeriodKind) -> String {
        let seconds = seconds.max(0);
        if seconds < 60 {
            return self.text(TextKey::SoonReset).into();
        }
        let (days, hours, minutes) = (
            seconds / 86_400,
            (seconds / 3_600) % 24,
            (seconds / 60) % 60,
        );
        let mut parts = Vec::new();
        if days > 0 {
            parts.push(self.unit_text(days, Unit::Day));
        }
        if hours > 0 {
            parts.push(self.unit_text(hours, Unit::Hour));
        }
        if minutes > 0 {
            parts.push(self.unit_text(minutes, Unit::Minute));
        }
        let duration = parts.join(self.separator());
        match (self.language, kind) {
            (Language::Japanese, PeriodKind::Weekly) => format!("7日間、あと{duration}"),
            (Language::Japanese, PeriodKind::Monthly) => format!("月間、あと{duration}"),
            (Language::SimplifiedChinese, PeriodKind::Weekly) => format!("7天，剩余{duration}"),
            (Language::SimplifiedChinese, PeriodKind::Monthly) => format!("每月，剩余{duration}"),
            (Language::Korean, PeriodKind::Weekly) => format!("7일 기간, {duration} 남음"),
            (Language::Korean, PeriodKind::Monthly) => format!("월간, {duration} 남음"),
            (Language::English, PeriodKind::Weekly) => {
                format!("7-day period, {duration} remaining")
            }
            (Language::English, PeriodKind::Monthly) => format!("Monthly, {duration} remaining"),
            (Language::Spanish, PeriodKind::Weekly) => {
                format!("Periodo de 7 días: quedan {duration}")
            }
            (Language::Spanish, PeriodKind::Monthly) => format!("Mensual: quedan {duration}"),
            (Language::French, PeriodKind::Weekly) => {
                format!("Période de 7 jours : {duration} restantes")
            }
            (Language::French, PeriodKind::Monthly) => format!("Mensuel : {duration} restantes"),
            (Language::German, PeriodKind::Weekly) => format!("7-Tage-Zeitraum, {duration} übrig"),
            (Language::German, PeriodKind::Monthly) => format!("Monatlich, {duration} übrig"),
            (Language::Portuguese, PeriodKind::Weekly) => {
                format!("Período de 7 dias: restam {duration}")
            }
            (Language::Portuguese, PeriodKind::Monthly) => format!("Mensal: restam {duration}"),
            (Language::Italian, PeriodKind::Weekly) => {
                format!("Periodo di 7 giorni: restano {duration}")
            }
            (Language::Italian, PeriodKind::Monthly) => format!("Mensile: restano {duration}"),
            (Language::Russian, PeriodKind::Weekly) => {
                format!("Период 7 дней: осталось {duration}")
            }
            (Language::Russian, PeriodKind::Monthly) => format!("За месяц осталось {duration}"),
        }
    }

    pub fn format_timestamp(&self, timestamp: i64) -> Option<String> {
        let time = DateTime::<Utc>::from_timestamp(timestamp, 0)?;
        Some(
            time.with_timezone(&self.timezone)
                .format("%Y/%m/%d %H:%M:%S %:z")
                .to_string(),
        )
    }

    pub fn format_graph_time(&self, timestamp: i64) -> Option<String> {
        let time = DateTime::<Utc>::from_timestamp(timestamp, 0)?;
        Some(
            time.with_timezone(&self.timezone)
                .format("%m/%d %H:%M")
                .to_string(),
        )
    }

    pub fn format_clock(&self, timestamp: i64) -> Option<String> {
        let time = DateTime::<Utc>::from_timestamp(timestamp, 0)?;
        Some(
            time.with_timezone(&self.timezone)
                .format("%H:%M")
                .to_string(),
        )
    }

    pub fn format_period(&self, start: i64, end: i64) -> Option<String> {
        Some(format!(
            "{}{}{}",
            self.format_timestamp(start)?,
            self.period_separator(),
            self.format_timestamp(end)?
        ))
    }

    pub fn format_deadline_suffix(&self, timestamp: i64) -> Option<String> {
        let timestamp = self.format_timestamp(timestamp)?;
        Some(match self.language {
            Language::Japanese | Language::SimplifiedChinese | Language::Korean => {
                format!("（{} {}）", self.text(TextKey::DeadlinePrefix), timestamp)
            }
            _ => format!(" ({} {})", self.text(TextKey::DeadlinePrefix), timestamp),
        })
    }

    pub fn format_grouped_unsigned(&self, value: u128) -> String {
        group_digits(value.to_string(), self.group_separator())
    }

    pub fn format_grouped_i64(&self, value: i64) -> String {
        if value < 0 {
            format!(
                "-{}",
                self.format_grouped_unsigned(value.unsigned_abs() as u128)
            )
        } else {
            self.format_grouped_unsigned(value as u128)
        }
    }

    pub fn format_dollar(&self, value: f64) -> String {
        let decimal = if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        };
        let raw = format!("{decimal:.2}");
        let (whole, fraction) = raw.split_once('.').unwrap_or((raw.as_str(), "00"));
        let number = format!(
            "{}{}{}",
            self.format_grouped_unsigned(whole.parse::<u128>().unwrap_or(0)),
            self.decimal_separator(),
            fraction
        );
        format!("${number}")
    }

    pub fn format_thread_count(&self, count: usize) -> String {
        let n = self.format_grouped_unsigned(count as u128);
        match self.language {
            Language::Japanese => format!("{n}件"),
            Language::SimplifiedChinese => format!("{n}条"),
            Language::Korean => format!("{n}개"),
            Language::English => format!("{n} threads"),
            Language::Spanish => format!("{n} hilos"),
            Language::French => format!("{n} fils"),
            Language::German => format!("{n} Threads"),
            Language::Portuguese => format!("{n} threads"),
            Language::Italian => format!("{n} thread"),
            Language::Russian => format!("{n} потоков"),
        }
    }

    pub fn format_token_value(&self, value: u64) -> String {
        let number = self.format_grouped_unsigned(value as u128);
        if matches!(
            self.language,
            Language::Japanese | Language::SimplifiedChinese | Language::Korean
        ) {
            format!("{number}{}", self.text(TextKey::Tokens))
        } else {
            format!("{number} {}", self.text(TextKey::Tokens))
        }
    }

    /// Format the current token total as a percentage of the model's context
    /// window. The caller may pair this value with the localized token limit
    /// so the percentage has an explicit scale.
    pub fn format_context_usage(&self, used_tokens: u64, context_window: u64) -> String {
        if context_window == 0 {
            return "—".to_owned();
        }
        let tenths = (u128::from(used_tokens)
            .saturating_mul(1_000)
            .saturating_add(u128::from(context_window / 2)))
            / u128::from(context_window);
        let tenths = tenths.min(1_000);
        let whole = tenths / 10;
        let fraction = tenths % 10;
        let whole = self.format_grouped_unsigned(whole);
        if fraction == 0 {
            format!("{whole}%")
        } else {
            format!("{whole}{}{fraction}%", self.decimal_separator())
        }
    }

    pub fn format_role(&self, is_subagent: bool, depth: Option<i32>) -> String {
        let base = self.text(if is_subagent {
            TextKey::SubRole
        } else {
            TextKey::MainRole
        });
        if !is_subagent {
            return base.into();
        }
        depth.map_or_else(|| base.into(), |depth| format!("{base} D{}", depth.max(0)))
    }

    pub fn format_parent_title(&self, title: &str) -> String {
        if title.is_empty() {
            return String::new();
        }
        format!("{}: {title}", self.text(TextKey::ParentPrefix))
    }

    pub fn format_estimate(&self, value: f64) -> String {
        format!(
            "{} {}",
            self.text(TextKey::EstimatePrefix),
            self.format_dollar(value)
        )
    }

    pub fn format_last_updated(&self, timestamp: Option<i64>) -> String {
        let time = timestamp
            .and_then(|ts| self.format_clock(ts))
            .unwrap_or_else(|| "—".into());
        format!("{} {}", self.text(TextKey::LastUpdated), time)
    }

    pub fn format_stale_status(&self, timestamp: Option<i64>) -> String {
        let time = timestamp
            .and_then(|ts| self.format_clock(ts))
            .unwrap_or_else(|| "—".into());
        match self.language {
            Language::Japanese => format!("最新情報を取得できません。表示は{time}時点の値です。"),
            Language::English => format!("Unable to fetch the latest data. Showing values from {time}."),
            Language::SimplifiedChinese => format!("无法获取最新信息。显示{time}时的数据。"),
            Language::Korean => format!("최신 정보를 가져올 수 없습니다. {time} 기준 값을 표시합니다."),
            Language::Spanish => format!("No se pudo obtener la información más reciente. Se muestran los valores de {time}."),
            Language::French => format!("Impossible d’obtenir les dernières données. Valeurs de {time}."),
            Language::German => format!("Die neuesten Daten konnten nicht abgerufen werden. Werte von {time}."),
            Language::Portuguese => format!("Não foi possível obter os dados mais recentes. Valores de {time}."),
            Language::Italian => format!("Impossibile ottenere i dati più recenti. Valori delle {time}."),
            Language::Russian => format!("Не удалось получить последние данные. Показаны значения на {time}."),
        }
    }

    fn separator(&self) -> &'static str {
        if self.language == Language::Japanese {
            "と"
        } else {
            ", "
        }
    }
    fn elapsed_separator(&self) -> &'static str {
        if matches!(
            self.language,
            Language::Japanese | Language::SimplifiedChinese | Language::Korean
        ) {
            ""
        } else {
            " "
        }
    }
    fn period_separator(&self) -> &'static str {
        if self.language == Language::Japanese {
            " ～ "
        } else {
            " — "
        }
    }
    fn group_separator(&self) -> char {
        match self.language {
            Language::French | Language::Russian => '\u{202f}',
            Language::German | Language::Spanish | Language::Italian | Language::Portuguese => '.',
            _ => ',',
        }
    }
    fn decimal_separator(&self) -> char {
        match self.language {
            Language::French
            | Language::German
            | Language::Spanish
            | Language::Portuguese
            | Language::Italian
            | Language::Russian => ',',
            _ => '.',
        }
    }
    fn unit_text(&self, value: i64, unit: Unit) -> String {
        let n = self.format_grouped_i64(value);
        match (self.language, unit) {
            (Language::Japanese, Unit::Second) => format!("{n}秒"),
            (Language::Japanese, Unit::Minute) => format!("{n}分"),
            (Language::Japanese, Unit::Hour) => format!("{n}時間"),
            (Language::Japanese, Unit::Day) => format!("{n}日"),
            (Language::SimplifiedChinese, Unit::Second) => format!("{n}秒"),
            (Language::SimplifiedChinese, Unit::Minute) => format!("{n}分钟"),
            (Language::SimplifiedChinese, Unit::Hour) => format!("{n}小时"),
            (Language::SimplifiedChinese, Unit::Day) => format!("{n}天"),
            (Language::Korean, Unit::Second) => format!("{n}초"),
            (Language::Korean, Unit::Minute) => format!("{n}분"),
            (Language::Korean, Unit::Hour) => format!("{n}시간"),
            (Language::Korean, Unit::Day) => format!("{n}일"),
            (Language::English, unit) => format!("{n} {}", english_unit(value, unit)),
            (Language::Spanish, unit) => format!("{n} {}", spanish_unit(value, unit)),
            (Language::French, unit) => format!("{n} {}", french_unit(value, unit)),
            (Language::German, unit) => format!("{n} {}", german_unit(value, unit)),
            (Language::Portuguese, unit) => format!("{n} {}", portuguese_unit(value, unit)),
            (Language::Italian, unit) => format!("{n} {}", italian_unit(value, unit)),
            (Language::Russian, unit) => format!("{n} {}", russian_unit(value, unit)),
        }
    }
}

const CLI_JA: [&str; 15] = [
    "--port には1〜65535の整数を指定してください。",
    "サービス実行ファイルを利用できません。",
    "サービスを起動できませんでした。",
    "競合したサービスの後始末に失敗しました。",
    "サービスの状態を確認できません。",
    "サービスは正常になる前に終了しました。",
    "サービスが正常状態になりませんでした。",
    "サービスは別のプロセスが所有しています。",
    "既存の正常なサービスを再利用します。",
    "daemonのlockを利用できません。",
    "daemonのlock所有者を検証できません。",
    "daemonへ停止信号を送れませんでした。",
    "停止中にdaemonの所有者が変わりました。",
    "daemonがlockを解放する前にタイムアウトしました。",
    "この環境ではdaemonを停止できません。",
];

const CLI_EN: [&str; 15] = [
    "--port requires an integer from 1 to 65535.",
    "The service executable is unavailable.",
    "The service could not be started.",
    "A competing service could not be cleaned up.",
    "The service state is unavailable.",
    "The service exited before becoming healthy.",
    "The service did not become healthy.",
    "The service is already owned by another process.",
    "Reusing the existing healthy service.",
    "The daemon lock is unavailable.",
    "The daemon lock owner cannot be verified.",
    "The daemon could not be sent a stop signal.",
    "The daemon owner changed while stopping.",
    "The daemon lock was not released before timeout.",
    "Stopping the daemon is unsupported on this platform.",
];

const CLI_ZH: [&str; 15] = [
    "--port 需要 1 到 65535 之间的整数。",
    "无法使用服务可执行文件。",
    "无法启动服务。",
    "无法清理竞争的服务。",
    "无法获取服务状态。",
    "服务在变为健康前已退出。",
    "服务未能变为健康状态。",
    "服务已由其他进程拥有。",
    "正在复用现有的健康服务。",
    "无法使用 daemon lock。",
    "无法验证 daemon lock 所有者。",
    "无法向 daemon 发送停止信号。",
    "停止期间 daemon 所有者已改变。",
    "daemon lock 在超时前未释放。",
    "此平台不支持停止 daemon。",
];

const CLI_KO: [&str; 15] = [
    "--port에는 1에서 65535 사이의 정수가 필요합니다.",
    "서비스 실행 파일을 사용할 수 없습니다.",
    "서비스를 시작할 수 없습니다.",
    "경쟁 서비스 정리에 실패했습니다.",
    "서비스 상태를 확인할 수 없습니다.",
    "서비스가 정상 상태가 되기 전에 종료되었습니다.",
    "서비스가 정상 상태가 되지 않았습니다.",
    "서비스는 이미 다른 프로세스가 소유하고 있습니다.",
    "기존의 정상 서비스를 재사용합니다.",
    "daemon lock을 사용할 수 없습니다.",
    "daemon lock 소유자를 확인할 수 없습니다.",
    "daemon에 중지 신호를 보낼 수 없습니다.",
    "중지 중 daemon 소유자가 변경되었습니다.",
    "시간 초과 전에 daemon lock이 해제되지 않았습니다.",
    "이 플랫폼에서는 daemon 중지를 지원하지 않습니다.",
];

const CLI_ES: [&str; 15] = [
    "--port requiere un entero entre 1 y 65535.",
    "El ejecutable del servicio no está disponible.",
    "No se pudo iniciar el servicio.",
    "No se pudo limpiar el servicio en conflicto.",
    "El estado del servicio no está disponible.",
    "El servicio terminó antes de estar saludable.",
    "El servicio no alcanzó un estado saludable.",
    "El servicio ya pertenece a otro proceso.",
    "Se reutiliza el servicio saludable existente.",
    "El lock del daemon no está disponible.",
    "No se puede verificar el propietario del lock del daemon.",
    "No se pudo enviar la señal de detención al daemon.",
    "El propietario del daemon cambió durante la detención.",
    "El lock del daemon no se liberó antes del tiempo límite.",
    "Esta plataforma no admite detener el daemon.",
];

const CLI_FR: [&str; 15] = [
    "--port nécessite un entier compris entre 1 et 65535.",
    "L’exécutable du service est indisponible.",
    "Impossible de démarrer le service.",
    "Impossible de nettoyer le service concurrent.",
    "L’état du service est indisponible.",
    "Le service s’est arrêté avant d’être sain.",
    "Le service n’est pas devenu sain.",
    "Le service est déjà détenu par un autre processus.",
    "Réutilisation du service sain existant.",
    "Le verrou du daemon est indisponible.",
    "Impossible de vérifier le propriétaire du verrou du daemon.",
    "Impossible d’envoyer le signal d’arrêt au daemon.",
    "Le propriétaire du daemon a changé pendant l’arrêt.",
    "Le verrou du daemon n’a pas été libéré à temps.",
    "L’arrêt du daemon n’est pas pris en charge sur cette plateforme.",
];

const CLI_DE: [&str; 15] = [
    "--port benötigt eine Ganzzahl von 1 bis 65535.",
    "Die Dienstdatei ist nicht verfügbar.",
    "Der Dienst konnte nicht gestartet werden.",
    "Der konkurrierende Dienst konnte nicht bereinigt werden.",
    "Der Dienststatus ist nicht verfügbar.",
    "Der Dienst wurde beendet, bevor er bereit war.",
    "Der Dienst wurde nicht bereit.",
    "Der Dienst gehört bereits einem anderen Prozess.",
    "Der vorhandene bereite Dienst wird wiederverwendet.",
    "Die Daemon-Sperre ist nicht verfügbar.",
    "Der Besitzer der Daemon-Sperre kann nicht überprüft werden.",
    "Dem Daemon konnte kein Stoppsignal gesendet werden.",
    "Der Daemon-Besitzer hat sich während des Stopps geändert.",
    "Die Daemon-Sperre wurde nicht rechtzeitig freigegeben.",
    "Das Stoppen des Daemons wird auf dieser Plattform nicht unterstützt.",
];

const CLI_PT: [&str; 15] = [
    "--port requer um inteiro de 1 a 65535.",
    "O executável do serviço não está disponível.",
    "Não foi possível iniciar o serviço.",
    "Não foi possível limpar o serviço concorrente.",
    "O estado do serviço não está disponível.",
    "O serviço saiu antes de ficar saudável.",
    "O serviço não ficou saudável.",
    "O serviço já pertence a outro processo.",
    "Reutilizando o serviço saudável existente.",
    "O lock do daemon não está disponível.",
    "Não é possível verificar o proprietário do lock do daemon.",
    "Não foi possível enviar o sinal de parada ao daemon.",
    "O proprietário do daemon mudou durante a parada.",
    "O lock do daemon não foi liberado antes do tempo limite.",
    "Parar o daemon não é compatível com esta plataforma.",
];

const CLI_IT: [&str; 15] = [
    "--port richiede un intero da 1 a 65535.",
    "L’eseguibile del servizio non è disponibile.",
    "Impossibile avviare il servizio.",
    "Impossibile ripulire il servizio concorrente.",
    "Lo stato del servizio non è disponibile.",
    "Il servizio è terminato prima di diventare sano.",
    "Il servizio non è diventato sano.",
    "Il servizio è già di proprietà di un altro processo.",
    "Riutilizzo del servizio sano esistente.",
    "Il lock del daemon non è disponibile.",
    "Impossibile verificare il proprietario del lock del daemon.",
    "Impossibile inviare il segnale di arresto al daemon.",
    "Il proprietario del daemon è cambiato durante l’arresto.",
    "Il lock del daemon non è stato rilasciato prima del timeout.",
    "L’arresto del daemon non è supportato su questa piattaforma.",
];

const CLI_RU: [&str; 15] = [
    "--port требует целое число от 1 до 65535.",
    "Исполняемый файл службы недоступен.",
    "Не удалось запустить службу.",
    "Не удалось очистить конкурирующую службу.",
    "Состояние службы недоступно.",
    "Служба завершилась до перехода в рабочее состояние.",
    "Служба не перешла в рабочее состояние.",
    "Служба уже принадлежит другому процессу.",
    "Повторно используется существующая рабочая служба.",
    "Блокировка daemon недоступна.",
    "Не удалось проверить владельца блокировки daemon.",
    "Не удалось отправить daemon сигнал остановки.",
    "Владелец daemon изменился во время остановки.",
    "Блокировка daemon не освобождена до истечения времени ожидания.",
    "Остановка daemon не поддерживается на этой платформе.",
];

#[derive(Clone, Copy)]
enum Unit {
    Second,
    Minute,
    Hour,
    Day,
}

fn detect_timezone() -> Tz {
    let configured = env::var("TZ").ok();
    let os_timezone = iana_time_zone::get_timezone().ok();
    timezone_from_names(configured.as_deref(), os_timezone.as_deref())
}

fn timezone_from_names(configured: Option<&str>, os_timezone: Option<&str>) -> Tz {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    let name = configured.or(os_timezone.map(str::trim).filter(|value| !value.is_empty()));
    name.and_then(|name| Tz::from_str(name.strip_prefix(':').unwrap_or(name)).ok())
        .unwrap_or(Tz::UTC)
}

fn group_digits(value: String, separator: char) -> String {
    let mut output = String::with_capacity(value.len() + value.len() / 3);
    for (index, ch) in value.chars().enumerate() {
        if index > 0 && (value.len() - index).is_multiple_of(3) {
            output.push(separator);
        }
        output.push(ch);
    }
    output
}

fn english_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "second",
        (Unit::Second, false) => "seconds",
        (Unit::Minute, true) => "minute",
        (Unit::Minute, false) => "minutes",
        (Unit::Hour, true) => "hour",
        (Unit::Hour, false) => "hours",
        (Unit::Day, true) => "day",
        (Unit::Day, false) => "days",
    }
}
fn spanish_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "segundo",
        (Unit::Second, false) => "segundos",
        (Unit::Minute, true) => "minuto",
        (Unit::Minute, false) => "minutos",
        (Unit::Hour, true) => "hora",
        (Unit::Hour, false) => "horas",
        (Unit::Day, true) => "día",
        (Unit::Day, false) => "días",
    }
}
fn french_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "seconde",
        (Unit::Second, false) => "secondes",
        (Unit::Minute, true) => "minute",
        (Unit::Minute, false) => "minutes",
        (Unit::Hour, true) => "heure",
        (Unit::Hour, false) => "heures",
        (Unit::Day, true) => "jour",
        (Unit::Day, false) => "jours",
    }
}
fn german_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "Sekunde",
        (Unit::Second, false) => "Sekunden",
        (Unit::Minute, true) => "Minute",
        (Unit::Minute, false) => "Minuten",
        (Unit::Hour, true) => "Stunde",
        (Unit::Hour, false) => "Stunden",
        (Unit::Day, true) => "Tag",
        (Unit::Day, false) => "Tage",
    }
}
fn portuguese_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "segundo",
        (Unit::Second, false) => "segundos",
        (Unit::Minute, true) => "minuto",
        (Unit::Minute, false) => "minutos",
        (Unit::Hour, true) => "hora",
        (Unit::Hour, false) => "horas",
        (Unit::Day, true) => "dia",
        (Unit::Day, false) => "dias",
    }
}
fn italian_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value == 1) {
        (Unit::Second, true) => "secondo",
        (Unit::Second, false) => "secondi",
        (Unit::Minute, true) => "minuto",
        (Unit::Minute, false) => "minuti",
        (Unit::Hour, true) => "ora",
        (Unit::Hour, false) => "ore",
        (Unit::Day, true) => "giorno",
        (Unit::Day, false) => "giorni",
    }
}
fn russian_unit(value: i64, unit: Unit) -> &'static str {
    match (unit, value % 10 == 1 && value % 100 != 11) {
        (Unit::Second, true) => "секунду",
        (Unit::Second, false) => "секунд",
        (Unit::Minute, true) => "минуту",
        (Unit::Minute, false) => "минут",
        (Unit::Hour, true) => "час",
        (Unit::Hour, false) => "часов",
        (Unit::Day, true) => "день",
        (Unit::Day, false) => "дней",
    }
}

// The non-Japanese catalogs intentionally use one complete match each. This
// keeps missing keys a compile-time review concern instead of a runtime mix.
fn english_text(key: TextKey) -> &'static str {
    basic_text(key, "en")
}
fn chinese_text(key: TextKey) -> &'static str {
    basic_text(key, "zh")
}
fn korean_text(key: TextKey) -> &'static str {
    basic_text(key, "ko")
}
fn spanish_text(key: TextKey) -> &'static str {
    basic_text(key, "es")
}
fn french_text(key: TextKey) -> &'static str {
    basic_text(key, "fr")
}
fn german_text(key: TextKey) -> &'static str {
    basic_text(key, "de")
}
fn portuguese_text(key: TextKey) -> &'static str {
    basic_text(key, "pt")
}
fn italian_text(key: TextKey) -> &'static str {
    basic_text(key, "it")
}
fn russian_text(key: TextKey) -> &'static str {
    basic_text(key, "ru")
}

fn basic_text(key: TextKey, language: &str) -> &'static str {
    use TextKey::*;
    match (language, key) {
        (_, FontFamily) => {
            // The JP subset does not contain every Simplified Chinese code
            // point used by the catalog. The embedded CJK KR face carries the
            // shared Han coverage, so use it for both CJK catalogs while
            // keeping Japanese on the JP face for Japanese glyph forms.
            if language == "ko" || language == "zh" {
                "Noto Sans CJK KR"
            } else {
                "Noto Sans JP"
            }
        }
        ("en", WindowUnauthenticated) => "Account not connected — Plan not set",
        ("en", PlanUnset) => "Plan not set",
        ("en", PlanFree) => "Free",
        ("en", PlanEnterprise) => "Enterprise",
        ("en", PlanEducation) => "Education",
        ("en", UsageStatus) => "Usage",
        ("en", Graph) => "Graph",
        ("en", LegalNotices) => "Legal notices",
        ("en", Running) => "Running",
        ("en", ModelThreads) => "Threads by model",
        ("en", Other) => "Other",
        ("en", Details) => "Details",
        ("en", NoRunningThreads) => "No running threads",
        ("en", LegalCode) => "Codex Info code and documents: GPL-3.0-only",
        ("en", LegalWarranty) => {
            "This software comes without warranty. Redistribution is allowed under GPL-3.0-only."
        }
        ("en", LegalLicense) => "License text: LICENSE",
        ("en", LegalFont) => "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
        ("en", LegalProtocol) => "Protocol and API: Apache-2.0 / Copyright 2025 OpenAI",
        ("en", LegalSchema) => "Codex-generated schema: Apache-2.0 / Copyright 2025 OpenAI",
        ("en", LegalDependencies) => {
            "Slint and Rust dependency crates retain their upstream licenses."
        }
        ("en", LegalThirdParty) => "Third-party licenses: MIT / BSD-3-Clause / other",
        ("en", LegalDetails) => "Details: THIRD_PARTY_NOTICES.md and LICENSES/",
        ("en", LegalDistribution) => {
            "Include each dependency's LICENSE/NOTICE when distributing binaries."
        }
        ("en", Close) => "Close",
        ("en", ActiveThreads) => "Running threads",
        ("en", Context) => "Context usage",
        ("en", Instruction) => "Instruction",
        ("en", Tokens) => "tokens",
        ("en", Model) => "Model",
        ("en", Input) => "Input",
        ("en", Cached) => "Cached",
        ("en", Output) => "Output",
        ("en", Retry) => "Retry",
        ("en", UsageTrend) => "Usage over time",
        ("en", Remaining) => "Remaining",
        ("en", GraphTokenDescription) => "Hourly token usage (by model) / remaining %",
        ("en", GraphDollarDescription) => "Hourly cumulative spend (by model) / remaining %",
        ("en", NoRecords) => "No records",
        ("en", ConnectAccount) => "Connect Codex account",
        ("en", AuthBrowserInstructions) => {
            "Complete authentication in your browser. It will be checked automatically."
        }
        ("en", AuthManaged) => {
            "Authentication is managed by Codex; this app does not store credentials."
        }
        ("en", OpenAuthPage) => "Open authentication page",
        ("en", StartAuth) => "Start authentication",
        ("en", Checking) => "Checking…",
        ("en", CheckAuth) => "Check authentication",
        ("en", AuthCli) => "Uses the Codex CLI authentication state.",
        ("en", NoHistory) => "No history",
        ("en", On) => "ON",
        ("en", Off) => "OFF",
        ("en", Connecting) => "Connecting to Codex app-server…",
        ("en", UpdatingUsage) => "Updating usage…",
        ("en", CheckingAuthStatus) => "Checking authentication…",
        ("en", AuthenticatedLoading) => "Authenticated. Loading usage…",
        ("en", UnauthenticatedStart) => "Not authenticated. Start authentication.",
        ("en", AuthUrlIssued) => "Authentication URL issued. Select “Open authentication page”.",
        ("en", IssuingAuthUrl) => "Issuing authentication URL…",
        ("en", AuthUrlOpenFailed) => "Authentication URL could not be opened.",
        ("en", CannotFetchUsage) => "Unable to fetch usage. Check the Codex app-server connection.",
        ("en", CannotDisplayStatus) => "Unable to display status.",
        ("en", QuotaNearlyGone) => "Almost no usage remains.",
        ("en", QuotaLow) => "Usage is running low.",
        ("en", ResetWithinDay) => "Within 24 hours of reset.",
        ("en", LastUpdated) => "Last updated",
        ("en", PartialHistoryThreads) => {
            "Usage updated. Previous history and threads are retained."
        }
        ("en", PartialHistory) => "Usage updated. Previous history is retained.",
        ("en", PartialThreads) => "Usage updated. Previous thread display is retained.",
        ("en", MainRole) => "Main",
        ("en", SubRole) => "Sub",
        ("en", ParentNotRunning) => "Parent thread is not running",
        ("en", ParentPrefix) => "Parent",
        ("en", CurrentSuffix) => " (current)",
        ("en", DeadlinePrefix) => "deadline",
        ("en", EstimatePrefix) => "Estimate",
        ("en", SoonReset) => "Resetting soon",
        ("en", FixedLimitNone) => "No fixed limit",
        ("en", QuotaRemaining) => "Remaining usage",
        ("en", MonthlyQuotaRemaining) => "Monthly remaining usage",
        ("en", UsageLimit) => "Usage limit",
        ("en", DollarMetric) => "Dollars",
        ("en", TokenMetric) => "Tokens",
        // Compact catalogs cover every key while keeping the source reviewable.
        ("zh", WindowUnauthenticated) => "账号未连接 — 未设置套餐",
        ("zh", PlanUnset) => "未设置套餐",
        ("zh", PlanFree) => "免费",
        ("zh", PlanEnterprise) => "企业版",
        ("zh", PlanEducation) => "教育版",
        ("zh", UsageStatus) => "使用情况",
        ("zh", Graph) => "图表",
        ("zh", LegalNotices) => "法律声明",
        ("zh", Running) => "运行中",
        ("zh", ModelThreads) => "按模型统计的线程",
        ("zh", Other) => "其他",
        ("zh", Details) => "详情",
        ("zh", NoRunningThreads) => "没有运行中的线程",
        ("zh", LegalCode) => "Codex Info 代码和文档：GPL-3.0-only",
        ("zh", LegalWarranty) => "本软件不提供保证，可按 GPL-3.0-only 条款再分发。",
        ("zh", LegalLicense) => "许可证文本：LICENSE",
        ("zh", LegalFont) => "Noto Sans JP / Noto Sans CJK KR：OFL-1.1 / Adobe 2014-2021",
        ("zh", LegalProtocol) => "协议和 API：Apache-2.0 / Copyright 2025 OpenAI",
        ("zh", LegalSchema) => "Codex 生成的架构：Apache-2.0 / Copyright 2025 OpenAI",
        ("zh", LegalDependencies) => "Slint 和 Rust 依赖库保留其上游许可证。",
        ("zh", LegalThirdParty) => "第三方许可证：MIT / BSD-3-Clause / 其他",
        ("zh", LegalDetails) => "详情：THIRD_PARTY_NOTICES.md 和 LICENSES/",
        ("zh", LegalDistribution) => "分发二进制文件时请附带各依赖的 LICENSE/NOTICE。",
        ("zh", Close) => "关闭",
        ("zh", ActiveThreads) => "运行中的线程",
        ("zh", Context) => "上下文使用率",
        ("zh", Instruction) => "指令",
        ("zh", Tokens) => "令牌",
        ("zh", Model) => "模型",
        ("zh", Input) => "输入",
        ("zh", Cached) => "缓存",
        ("zh", Output) => "输出",
        ("zh", Retry) => "重试",
        ("zh", UsageTrend) => "使用情况趋势",
        ("zh", Remaining) => "剩余",
        ("zh", GraphTokenDescription) => "按小时令牌用量（按模型）/ 剩余%",
        ("zh", GraphDollarDescription) => "按小时累计美元消耗（按模型）/ 剩余%",
        ("zh", NoRecords) => "没有记录",
        ("zh", ConnectAccount) => "连接 Codex 账号",
        ("zh", AuthBrowserInstructions) => "请在浏览器中完成认证。完成后会自动检查。",
        ("zh", AuthManaged) => "认证由 Codex 管理；本应用不保存凭据。",
        ("zh", OpenAuthPage) => "打开认证页面",
        ("zh", StartAuth) => "开始认证",
        ("zh", Checking) => "检查中…",
        ("zh", CheckAuth) => "检查认证",
        ("zh", AuthCli) => "使用 Codex CLI 的认证状态。",
        ("zh", NoHistory) => "没有历史记录",
        ("zh", On) => "开",
        ("zh", Off) => "关",
        ("zh", Connecting) => "正在连接 Codex app-server…",
        ("zh", UpdatingUsage) => "正在更新使用情况…",
        ("zh", CheckingAuthStatus) => "正在检查认证状态…",
        ("zh", AuthenticatedLoading) => "已认证，正在读取用量…",
        ("zh", UnauthenticatedStart) => "未认证，请开始认证。",
        ("zh", AuthUrlIssued) => "已生成认证 URL，请选择“打开认证页面”。",
        ("zh", IssuingAuthUrl) => "正在生成认证 URL…",
        ("zh", AuthUrlOpenFailed) => "无法打开认证 URL。",
        ("zh", CannotFetchUsage) => "无法获取使用情况，请检查 Codex app-server 连接。",
        ("zh", CannotDisplayStatus) => "无法显示状态。",
        ("zh", QuotaNearlyGone) => "剩余用量几乎为零。",
        ("zh", QuotaLow) => "剩余用量较少。",
        ("zh", ResetWithinDay) => "将在 24 小时内重置。",
        ("zh", LastUpdated) => "最后更新",
        ("zh", PartialHistoryThreads) => "用量已更新，保留之前的历史和线程。",
        ("zh", PartialHistory) => "用量已更新，保留之前的历史。",
        ("zh", PartialThreads) => "用量已更新，保留之前的线程显示。",
        ("zh", MainRole) => "主线程",
        ("zh", SubRole) => "子线程",
        ("zh", ParentNotRunning) => "父线程当前未运行",
        ("zh", ParentPrefix) => "父线程",
        ("zh", CurrentSuffix) => "（当前）",
        ("zh", DeadlinePrefix) => "期限",
        ("zh", EstimatePrefix) => "估算",
        ("zh", SoonReset) => "即将重置",
        ("zh", FixedLimitNone) => "无固定上限",
        ("zh", QuotaRemaining) => "剩余用量",
        ("zh", MonthlyQuotaRemaining) => "每月剩余用量",
        ("zh", UsageLimit) => "用量上限",
        ("zh", DollarMetric) => "美元",
        ("zh", TokenMetric) => "令牌",
        _ => translated_text(language, key),
    }
}

fn translated_text(language: &str, key: TextKey) -> &'static str {
    let index = TextKey::ALL
        .iter()
        .position(|candidate| *candidate == key)
        .expect("all translation keys must be listed");
    let catalog = match language {
        "ko" => KO_CATALOG,
        "es" => ES_CATALOG,
        "fr" => FR_CATALOG,
        "de" => DE_CATALOG,
        "pt" => PT_CATALOG,
        "it" => IT_CATALOG,
        "ru" => RU_CATALOG,
        _ => panic!("unknown translation catalog: {language}"),
    };
    catalog[index]
}

const KO_CATALOG: [&str; 81] = [
    "Noto Sans CJK KR",
    "계정이 연결되지 않음 — 요금제 미설정",
    "요금제 미설정",
    "무료",
    "엔터프라이즈",
    "교육",
    "사용량",
    "그래프",
    "법적 고지",
    "실행 중",
    "모델별 스레드",
    "기타",
    "세부 정보",
    "실행 중인 스레드 없음",
    "Codex Info 코드 및 문서: GPL-3.0-only",
    "이 소프트웨어는 보증 없이 제공되며 GPL-3.0-only로 재배포할 수 있습니다.",
    "라이선스 본문: LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "프로토콜 및 API: Apache-2.0 / Copyright 2025 OpenAI",
    "Codex 생성 스키마: Apache-2.0 / Copyright 2025 OpenAI",
    "Slint 및 Rust 의존 크레이트는 각 상위 라이선스를 유지합니다.",
    "타사 라이선스: MIT / BSD-3-Clause / 기타",
    "세부 정보: THIRD_PARTY_NOTICES.md 및 LICENSES/",
    "바이너리 배포 시 각 의존성의 LICENSE/NOTICE를 포함하세요.",
    "닫기",
    "실행 중인 스레드",
    "컨텍스트 사용률",
    "지시",
    "토큰",
    "모델",
    "입력",
    "캐시",
    "출력",
    "재시도",
    "사용량 추이",
    "남은 양",
    "시간별 토큰 사용량(모델별) / 남은 비율%",
    "시간별 누적 달러 사용량(모델별) / 남은 비율%",
    "기록 없음",
    "Codex 계정 연결",
    "브라우저에서 인증을 완료하세요. 완료 후 자동으로 확인합니다.",
    "인증은 Codex가 관리하며 이 앱은 인증 정보를 저장하지 않습니다.",
    "인증 페이지 열기",
    "인증 시작",
    "확인 중…",
    "인증 상태 확인",
    "Codex CLI 인증 상태를 사용합니다.",
    "기록 없음",
    "켜기",
    "끄기",
    "Codex app-server에 연결 중…",
    "사용량 업데이트 중…",
    "인증 상태 확인 중…",
    "인증됨. 사용량을 불러오는 중…",
    "인증되지 않았습니다. 인증을 시작하세요.",
    "인증 URL이 발급되었습니다. ‘인증 페이지 열기’를 선택하세요.",
    "인증 URL 발급 중…",
    "인증 URL을 열 수 없습니다.",
    "사용량을 가져올 수 없습니다. Codex app-server 연결을 확인하세요.",
    "상태를 표시할 수 없습니다.",
    "남은 사용량이 거의 없습니다.",
    "남은 사용량이 부족합니다.",
    "재설정까지 24시간 이내입니다.",
    "마지막 업데이트",
    "사용량이 업데이트되었습니다. 이전 기록과 스레드를 유지합니다.",
    "사용량이 업데이트되었습니다. 이전 기록을 유지합니다.",
    "사용량이 업데이트되었습니다. 이전 스레드 표시를 유지합니다.",
    "메인",
    "서브",
    "상위 스레드가 실행 중이 아님",
    "상위",
    " (현재)",
    "기한",
    "예상",
    "곧 재설정",
    "고정 한도 없음",
    "남은 사용량",
    "월간 남은 사용량",
    "사용량 한도",
    "달러",
    "토큰",
];

const ES_CATALOG: [&str; 81] = [
    "Noto Sans JP",
    "Cuenta no conectada — plan no establecido",
    "Plan no establecido",
    "Gratis",
    "Empresa",
    "Educación",
    "Uso",
    "Gráfico",
    "Avisos legales",
    "En ejecución",
    "Hilos por modelo",
    "Otros",
    "Detalles",
    "No hay hilos en ejecución",
    "Código y documentos de Codex Info: GPL-3.0-only",
    "Este software se ofrece sin garantía. Se permite redistribuirlo bajo GPL-3.0-only.",
    "Texto de licencia: LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "Protocolo y API: Apache-2.0 / Copyright 2025 OpenAI",
    "Esquema generado por Codex: Apache-2.0 / Copyright 2025 OpenAI",
    "Slint y las dependencias de Rust conservan sus licencias originales.",
    "Licencias de terceros: MIT / BSD-3-Clause / otras",
    "Detalles: THIRD_PARTY_NOTICES.md y LICENSES/",
    "Incluye las licencias LICENSE/NOTICE al distribuir binarios.",
    "Cerrar",
    "Hilos en ejecución",
    "Uso del contexto",
    "Instrucción",
    "tokens",
    "Modelo",
    "Entrada",
    "Caché",
    "Salida",
    "Reintentar",
    "Evolución del uso",
    "Restante",
    "Uso de tokens por hora (por modelo) / % restante",
    "Gasto acumulado por hora (por modelo) / % restante",
    "Sin registros",
    "Conectar cuenta de Codex",
    "Completa la autenticación en el navegador. Se comprobará automáticamente.",
    "Codex gestiona la autenticación; esta aplicación no guarda credenciales.",
    "Abrir página de autenticación",
    "Iniciar autenticación",
    "Comprobando…",
    "Comprobar autenticación",
    "Usa el estado de autenticación de Codex CLI.",
    "Sin historial",
    "ACTIVADO",
    "DESACTIVADO",
    "Conectando con Codex app-server…",
    "Actualizando el uso…",
    "Comprobando la autenticación…",
    "Autenticado. Cargando el uso…",
    "No autenticado. Inicia la autenticación.",
    "URL de autenticación emitida. Selecciona «Abrir página de autenticación».",
    "Emitiendo URL de autenticación…",
    "No se pudo abrir la URL de autenticación.",
    "No se pudo obtener el uso. Comprueba la conexión con Codex app-server.",
    "No se puede mostrar el estado.",
    "Queda muy poco uso.",
    "Queda poco uso.",
    "Faltan menos de 24 horas para el reinicio.",
    "Última actualización",
    "Uso actualizado. Se conservan el historial y los hilos anteriores.",
    "Uso actualizado. Se conserva el historial anterior.",
    "Uso actualizado. Se conserva la vista de hilos anterior.",
    "Principal",
    "Sub",
    "El hilo principal no está en ejecución",
    "Principal",
    " (actual)",
    "límite",
    "Estimación",
    "Reinicio inminente",
    "Sin límite fijo",
    "Uso restante",
    "Uso mensual restante",
    "Límite de uso",
    "Dólares",
    "Tokens",
];

const FR_CATALOG: [&str; 81] = [
    "Noto Sans JP",
    "Compte non connecté — forfait non défini",
    "Forfait non défini",
    "Gratuit",
    "Entreprise",
    "Éducation",
    "Utilisation",
    "Graphique",
    "Mentions légales",
    "En cours",
    "Fils par modèle",
    "Autre",
    "Détails",
    "Aucun fil en cours",
    "Code et documents Codex Info : GPL-3.0-only",
    "Ce logiciel est fourni sans garantie. La redistribution est autorisée sous GPL-3.0-only.",
    "Texte de licence : LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "Protocole et API : Apache-2.0 / Copyright 2025 OpenAI",
    "Schéma généré par Codex : Apache-2.0 / Copyright 2025 OpenAI",
    "Slint et les dépendances Rust conservent leurs licences amont.",
    "Licences tierces : MIT / BSD-3-Clause / autres",
    "Détails : THIRD_PARTY_NOTICES.md et LICENSES/",
    "Joignez les fichiers LICENSE/NOTICE des dépendances lors de la distribution.",
    "Fermer",
    "Fils en cours",
    "Utilisation du contexte",
    "Instruction",
    "jetons",
    "Modèle",
    "Entrée",
    "Cache",
    "Sortie",
    "Réessayer",
    "Évolution de l’utilisation",
    "Restant",
    "Utilisation horaire des jetons (par modèle) / % restant",
    "Dépense cumulée horaire (par modèle) / % restant",
    "Aucun enregistrement",
    "Connecter le compte Codex",
    "Terminez l’authentification dans le navigateur. Elle sera vérifiée automatiquement.",
    "L’authentification est gérée par Codex ; cette application ne stocke pas les identifiants.",
    "Ouvrir la page d’authentification",
    "Démarrer l’authentification",
    "Vérification…",
    "Vérifier l’authentification",
    "Utilise l’état d’authentification de Codex CLI.",
    "Aucun historique",
    "ACTIVÉ",
    "DÉSACTIVÉ",
    "Connexion à Codex app-server…",
    "Mise à jour de l’utilisation…",
    "Vérification de l’authentification…",
    "Authentifié. Chargement de l’utilisation…",
    "Non authentifié. Démarrez l’authentification.",
    "URL d’authentification créée. Sélectionnez « Ouvrir la page d’authentification ».",
    "Création de l’URL d’authentification…",
    "Impossible d’ouvrir l’URL d’authentification.",
    "Impossible de récupérer l’utilisation. Vérifiez la connexion Codex app-server.",
    "Impossible d’afficher l’état.",
    "Il reste presque aucune utilisation.",
    "L’utilisation restante est faible.",
    "Réinitialisation dans moins de 24 heures.",
    "Dernière mise à jour",
    "Utilisation mise à jour. Historique et fils précédents conservés.",
    "Utilisation mise à jour. Historique précédent conservé.",
    "Utilisation mise à jour. Affichage des fils précédent conservé.",
    "Principal",
    "Sous",
    "Le fil parent n’est pas en cours",
    "Parent",
    " (actuel)",
    "échéance",
    "Estimation",
    "Réinitialisation imminente",
    "Aucune limite fixe",
    "Utilisation restante",
    "Utilisation mensuelle restante",
    "Limite d’utilisation",
    "Dollars",
    "Jetons",
];

const DE_CATALOG: [&str; 81] = [
    "Noto Sans JP", "Konto nicht verbunden — Tarif nicht festgelegt", "Tarif nicht festgelegt", "Kostenlos", "Unternehmen", "Bildung", "Nutzung", "Diagramm", "Rechtliche Hinweise", "Läuft", "Threads nach Modell", "Sonstige", "Details", "Keine laufenden Threads", "Codex-Info-Code und Dokumente: GPL-3.0-only", "Diese Software wird ohne Gewährleistung bereitgestellt. Weitergabe unter GPL-3.0-only ist erlaubt.", "Lizenztext: LICENSE", "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021", "Protokoll und API: Apache-2.0 / Copyright 2025 OpenAI", "Von Codex erzeugtes Schema: Apache-2.0 / Copyright 2025 OpenAI", "Slint und Rust-Abhängigkeiten behalten ihre ursprünglichen Lizenzen.", "Lizenzen Dritter: MIT / BSD-3-Clause / weitere", "Details: THIRD_PARTY_NOTICES.md und LICENSES/", "Beim Verteilen von Binärdateien die LICENSE/NOTICE-Dateien beilegen.", "Schließen", "Laufende Threads", "Kontextnutzung", "Anweisung", "Tokens", "Modell", "Eingabe", "Cache", "Ausgabe", "Erneut versuchen", "Nutzungsverlauf", "Verbleibend", "Stündliche Token-Nutzung (nach Modell) / verbleibend %", "Kumulierte stündliche Ausgaben (nach Modell) / verbleibend %", "Keine Aufzeichnungen", "Codex-Konto verbinden", "Schließe die Authentifizierung im Browser ab. Sie wird automatisch geprüft.", "Die Authentifizierung wird von Codex verwaltet; diese App speichert keine Zugangsdaten.", "Authentifizierungsseite öffnen", "Authentifizierung starten", "Wird geprüft…", "Authentifizierung prüfen", "Verwendet den Authentifizierungsstatus der Codex CLI.", "Kein Verlauf", "AN", "AUS", "Verbindung mit Codex app-server…", "Nutzung wird aktualisiert…", "Authentifizierung wird geprüft…", "Authentifiziert. Nutzung wird geladen…", "Nicht authentifiziert. Authentifizierung starten.", "Authentifizierungs-URL erstellt. «Authentifizierungsseite öffnen» wählen.", "Authentifizierungs-URL wird erstellt…", "Authentifizierungs-URL konnte nicht geöffnet werden.", "Nutzung konnte nicht abgerufen werden. Codex-app-server-Verbindung prüfen.", "Status kann nicht angezeigt werden.", "Fast keine Nutzung mehr verfügbar.", "Nutzung wird knapp.", "Weniger als 24 Stunden bis zum Zurücksetzen.", "Zuletzt aktualisiert", "Nutzung aktualisiert. Vorheriger Verlauf und Threads bleiben erhalten.", "Nutzung aktualisiert. Vorheriger Verlauf bleibt erhalten.", "Nutzung aktualisiert. Vorherige Thread-Anzeige bleibt erhalten.", "Haupt", "Unter", "Übergeordneter Thread läuft nicht", "Übergeordnet", " (aktuell)", "Frist", "Schätzung", "Wird bald zurückgesetzt", "Keine feste Grenze", "Verbleibende Nutzung", "Verbleibende Monatsnutzung", "Nutzungsgrenze", "Dollar", "Token",
];

const PT_CATALOG: [&str; 81] = [
    "Noto Sans JP",
    "Conta não conectada — plano não definido",
    "Plano não definido",
    "Grátis",
    "Empresa",
    "Educação",
    "Uso",
    "Gráfico",
    "Avisos legais",
    "Em execução",
    "Threads por modelo",
    "Outros",
    "Detalhes",
    "Nenhuma thread em execução",
    "Código e documentos do Codex Info: GPL-3.0-only",
    "Este software é fornecido sem garantia. A redistribuição sob GPL-3.0-only é permitida.",
    "Texto da licença: LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "Protocolo e API: Apache-2.0 / Copyright 2025 OpenAI",
    "Esquema gerado pelo Codex: Apache-2.0 / Copyright 2025 OpenAI",
    "Slint e as dependências Rust mantêm suas licenças originais.",
    "Licenças de terceiros: MIT / BSD-3-Clause / outras",
    "Detalhes: THIRD_PARTY_NOTICES.md e LICENSES/",
    "Inclua LICENSE/NOTICE de cada dependência ao distribuir binários.",
    "Fechar",
    "Threads em execução",
    "Uso do contexto",
    "Instrução",
    "tokens",
    "Modelo",
    "Entrada",
    "Cache",
    "Saída",
    "Tentar novamente",
    "Evolução do uso",
    "Restante",
    "Uso de tokens por hora (por modelo) / % restante",
    "Gasto cumulativo por hora (por modelo) / % restante",
    "Sem registros",
    "Conectar conta Codex",
    "Conclua a autenticação no navegador. Ela será verificada automaticamente.",
    "A autenticação é gerenciada pelo Codex; este app não armazena credenciais.",
    "Abrir página de autenticação",
    "Iniciar autenticação",
    "Verificando…",
    "Verificar autenticação",
    "Usa o estado de autenticação da Codex CLI.",
    "Sem histórico",
    "LIGADO",
    "DESLIGADO",
    "Conectando ao Codex app-server…",
    "Atualizando o uso…",
    "Verificando autenticação…",
    "Autenticado. Carregando uso…",
    "Não autenticado. Inicie a autenticação.",
    "URL de autenticação emitida. Selecione «Abrir página de autenticação».",
    "Emitindo URL de autenticação…",
    "Não foi possível abrir a URL de autenticação.",
    "Não foi possível obter o uso. Verifique a conexão com o Codex app-server.",
    "Não é possível exibir o estado.",
    "Quase não resta uso.",
    "O uso restante está baixo.",
    "Faltam menos de 24 horas para a redefinição.",
    "Última atualização",
    "Uso atualizado. Histórico e threads anteriores foram mantidos.",
    "Uso atualizado. Histórico anterior foi mantido.",
    "Uso atualizado. A visualização anterior das threads foi mantida.",
    "Principal",
    "Sub",
    "A thread pai não está em execução",
    "Pai",
    " (atual)",
    "prazo",
    "Estimativa",
    "Redefinição próxima",
    "Sem limite fixo",
    "Uso restante",
    "Uso mensal restante",
    "Limite de uso",
    "Dólares",
    "Tokens",
];

const IT_CATALOG: [&str; 81] = [
    "Noto Sans JP",
    "Account non collegato — piano non impostato",
    "Piano non impostato",
    "Gratuito",
    "Aziendale",
    "Istruzione",
    "Utilizzo",
    "Grafico",
    "Note legali",
    "In esecuzione",
    "Thread per modello",
    "Altro",
    "Dettagli",
    "Nessun thread in esecuzione",
    "Codice e documenti Codex Info: GPL-3.0-only",
    "Questo software è fornito senza garanzia. La ridistribuzione è consentita con GPL-3.0-only.",
    "Testo della licenza: LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "Protocollo e API: Apache-2.0 / Copyright 2025 OpenAI",
    "Schema generato da Codex: Apache-2.0 / Copyright 2025 OpenAI",
    "Slint e le dipendenze Rust mantengono le licenze originali.",
    "Licenze di terze parti: MIT / BSD-3-Clause / altre",
    "Dettagli: THIRD_PARTY_NOTICES.md e LICENSES/",
    "Includi LICENSE/NOTICE di ogni dipendenza nella distribuzione dei binari.",
    "Chiudi",
    "Thread in esecuzione",
    "Utilizzo del contesto",
    "Istruzione",
    "token",
    "Modello",
    "Input",
    "Cache",
    "Output",
    "Riprova",
    "Andamento dell’utilizzo",
    "Rimanente",
    "Utilizzo token orario (per modello) / % rimanente",
    "Spesa cumulativa oraria (per modello) / % rimanente",
    "Nessun record",
    "Collega account Codex",
    "Completa l’autenticazione nel browser. Verrà verificata automaticamente.",
    "L’autenticazione è gestita da Codex; l’app non salva credenziali.",
    "Apri pagina di autenticazione",
    "Avvia autenticazione",
    "Verifica in corso…",
    "Verifica autenticazione",
    "Usa lo stato di autenticazione della Codex CLI.",
    "Nessuna cronologia",
    "ATTIVO",
    "DISATTIVO",
    "Connessione a Codex app-server…",
    "Aggiornamento utilizzo…",
    "Verifica autenticazione…",
    "Autenticato. Caricamento utilizzo…",
    "Non autenticato. Avvia l’autenticazione.",
    "URL di autenticazione emesso. Seleziona «Apri pagina di autenticazione».",
    "Emissione URL di autenticazione…",
    "Impossibile aprire l’URL di autenticazione.",
    "Impossibile recuperare l’utilizzo. Controlla la connessione a Codex app-server.",
    "Impossibile mostrare lo stato.",
    "Quasi nessun utilizzo rimanente.",
    "L’utilizzo rimanente è basso.",
    "Meno di 24 ore al ripristino.",
    "Ultimo aggiornamento",
    "Utilizzo aggiornato. Cronologia e thread precedenti conservati.",
    "Utilizzo aggiornato. Cronologia precedente conservata.",
    "Utilizzo aggiornato. Visualizzazione precedente dei thread conservata.",
    "Principale",
    "Secondario",
    "Il thread principale non è in esecuzione",
    "Principale",
    " (attuale)",
    "scadenza",
    "Stima",
    "Ripristino imminente",
    "Nessun limite fisso",
    "Utilizzo rimanente",
    "Utilizzo mensile rimanente",
    "Limite di utilizzo",
    "Dollari",
    "Token",
];

const RU_CATALOG: [&str; 81] = [
    "Noto Sans JP",
    "Аккаунт не подключён — тариф не задан",
    "Тариф не задан",
    "Бесплатный",
    "Корпоративный",
    "Образование",
    "Использование",
    "График",
    "Правовые уведомления",
    "Выполняется",
    "Потоки по модели",
    "Другое",
    "Подробнее",
    "Нет выполняющихся потоков",
    "Код и документы Codex Info: GPL-3.0-only",
    "Программа предоставляется без гарантий. Распространение разрешено по GPL-3.0-only.",
    "Текст лицензии: LICENSE",
    "Noto Sans JP / Noto Sans CJK KR: OFL-1.1 / Adobe 2014-2021",
    "Протокол и API: Apache-2.0 / Copyright 2025 OpenAI",
    "Схема Codex: Apache-2.0 / Copyright 2025 OpenAI",
    "Slint и зависимости Rust сохраняют исходные лицензии.",
    "Лицензии третьих сторон: MIT / BSD-3-Clause / другие",
    "Подробнее: THIRD_PARTY_NOTICES.md и LICENSES/",
    "При распространении бинарных файлов приложите LICENSE/NOTICE зависимостей.",
    "Закрыть",
    "Выполняющиеся потоки",
    "Использование контекста",
    "Инструкция",
    "токены",
    "Модель",
    "Ввод",
    "Кэш",
    "Вывод",
    "Повторить",
    "Динамика использования",
    "Осталось",
    "Почасовое использование токенов (по модели) / осталось %",
    "Накопленные почасовые расходы (по модели) / осталось %",
    "Нет записей",
    "Подключить аккаунт Codex",
    "Завершите аутентификацию в браузере. Она будет проверена автоматически.",
    "Аутентификацией управляет Codex; приложение не хранит учётные данные.",
    "Открыть страницу аутентификации",
    "Начать аутентификацию",
    "Проверка…",
    "Проверить аутентификацию",
    "Используется состояние аутентификации Codex CLI.",
    "Нет истории",
    "ВКЛ",
    "ВЫКЛ",
    "Подключение к Codex app-server…",
    "Обновление использования…",
    "Проверка аутентификации…",
    "Аутентификация выполнена. Загрузка использования…",
    "Нет аутентификации. Начните аутентификацию.",
    "URL аутентификации создан. Выберите «Открыть страницу аутентификации».",
    "Создание URL аутентификации…",
    "Не удалось открыть URL аутентификации.",
    "Не удалось получить использование. Проверьте подключение к Codex app-server.",
    "Не удалось показать состояние.",
    "Почти не осталось доступного использования.",
    "Доступное использование заканчивается.",
    "До сброса осталось менее 24 часов.",
    "Последнее обновление",
    "Использование обновлено. Предыдущие история и потоки сохранены.",
    "Использование обновлено. Предыдущая история сохранена.",
    "Использование обновлено. Предыдущее отображение потоков сохранено.",
    "Основной",
    "Дочерний",
    "Родительский поток не выполняется",
    "Родитель",
    " (текущий)",
    "срок",
    "Оценка",
    "Скорый сброс",
    "Без фиксированного лимита",
    "Оставшееся использование",
    "Оставшееся месячное использование",
    "Лимит использования",
    "Доллары",
    "Токены",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_precedence_and_fallbacks_are_deterministic() {
        assert_eq!(
            Language::detect_from_values(Some("en_US.UTF-8"), Some("ja_JP.UTF-8"), Some("de_DE")),
            Language::English
        );
        assert_eq!(
            Language::detect_from_values(Some(""), Some("ja_JP.UTF-8"), Some("en_US")),
            Language::Japanese
        );
        assert_eq!(
            Language::detect_from_values(Some("C.UTF-8"), Some("ja_JP.UTF-8"), Some("en_US")),
            Language::English
        );
        assert_eq!(
            Language::detect_from_values(None, None, None),
            Language::English
        );
        assert_eq!(
            Language::detect_from_values(Some("zh_TW.UTF-8"), None, None),
            Language::SimplifiedChinese
        );
        assert_eq!(
            Language::detect_from_values(Some("ko-KR"), None, None),
            Language::Korean
        );
        assert_eq!(
            Language::detect_from_values(Some("ar_SA"), None, None),
            Language::English
        );
    }

    #[test]
    fn launch_help_is_localized_for_every_supported_language() {
        let japanese = Language::Japanese.launch_help();
        let english = Language::English.launch_help();
        assert_ne!(japanese, english);
        for language in Language::ALL {
            let help = language.launch_help();
            assert!(help.contains("--ui"), "{}", language.code());
            assert!(help.contains("--port"), "{}", language.code());
            assert!(help.contains("--stop"), "{}", language.code());
            assert!(help.contains("--help, --h, -h"), "{}", language.code());
            for legacy in [
                "--service",
                "--ui-only",
                "--all",
                "--listen",
                "--record-daemon",
                "--once",
            ] {
                assert!(
                    !help.contains(legacy),
                    "legacy option {legacy} leaked into {}",
                    language.code()
                );
            }
            assert!(!help.trim().is_empty(), "{}", language.code());

            let launcher_help = language.launcher_help();
            for option in [
                "--start",
                "--ui",
                "--stop",
                "--disable-autostart",
                "--remove",
                "--status",
                "--update",
                "--help",
            ] {
                assert!(
                    launcher_help.contains(option),
                    "installed launcher help omitted {option} for {}",
                    language.code()
                );
            }
            assert!(
                !launcher_help.contains("--port"),
                "payload-only --port leaked into installed launcher help for {}",
                language.code()
            );
            assert!(!launcher_help.trim().is_empty(), "{}", language.code());
        }
    }

    #[test]
    fn cli_lifecycle_messages_are_localized_for_every_supported_language() {
        for key in CliTextKey::ALL {
            let mut messages = Vec::new();
            for language in Language::ALL {
                let message = I18n::from_parts(language, Tz::UTC).cli_text(key);
                assert!(!message.trim().is_empty(), "{} {:?}", language.code(), key);
                messages.push(message);
            }
            assert!(
                messages.windows(2).any(|pair| pair[0] != pair[1]),
                "CLI message was not localized: {:?}",
                key
            );
        }
    }

    #[test]
    fn every_catalog_key_is_non_empty() {
        for language in Language::ALL {
            let i18n = I18n::from_parts(language, Tz::UTC);
            for key in TextKey::ALL {
                assert!(
                    !i18n.text(*key).trim().is_empty(),
                    "{} {:?}",
                    language.code(),
                    key
                );
            }
        }
        assert_ne!(
            I18n::from_parts(Language::Korean, Tz::UTC).text(TextKey::UsageStatus),
            I18n::from_parts(Language::English, Tz::UTC).text(TextKey::UsageStatus)
        );
        assert_ne!(
            I18n::from_parts(Language::Russian, Tz::UTC).text(TextKey::UsageStatus),
            I18n::from_parts(Language::English, Tz::UTC).text(TextKey::UsageStatus)
        );
        assert_eq!(
            I18n::from_parts(Language::SimplifiedChinese, Tz::UTC).font_family(),
            "Noto Sans CJK KR"
        );
        assert_eq!(
            I18n::from_parts(Language::Japanese, Tz::UTC).font_family(),
            "Noto Sans JP"
        );
    }

    #[test]
    fn legal_font_notice_matches_embedded_font_attribution() {
        for language in Language::ALL {
            let notice = I18n::from_parts(language, Tz::UTC).text(TextKey::LegalFont);
            assert!(notice.contains("OFL-1.1"), "{}: {notice}", language.code());
            assert!(
                notice.contains("Adobe 2014-2021"),
                "{}: {notice}",
                language.code()
            );
            assert!(
                !notice.contains("Noto Sans KR:"),
                "{}: {notice}",
                language.code()
            );
        }
    }

    #[test]
    fn context_usage_is_derived_from_tokens_and_capped_at_full() {
        let english = I18n::from_parts(Language::English, Tz::UTC);
        assert_eq!(english.format_context_usage(0, 100), "0%");
        assert_eq!(english.format_context_usage(50, 100), "50%");
        assert_eq!(english.format_context_usage(1, 3), "33.3%");
        assert_eq!(english.format_context_usage(u64::MAX, 100), "100%");
        assert_eq!(english.format_context_usage(1, 0), "—");

        let french = I18n::from_parts(Language::French, Tz::UTC);
        assert_eq!(french.format_context_usage(1, 3), "33,3%");
    }

    #[test]
    fn elapsed_and_remaining_boundaries_use_utc_seconds() {
        let i18n = I18n::from_parts(Language::English, Tz::UTC);
        assert_eq!(i18n.format_elapsed(100, Some(100)), "0 seconds");
        assert_eq!(i18n.format_elapsed(100, Some(41)), "59 seconds");
        assert_eq!(i18n.format_elapsed(100, Some(40)), "1 minute");
        assert_eq!(i18n.format_elapsed(100, Some(-3_560)), "1 hour 1 minute");
        assert_eq!(
            i18n.format_period_remaining(0, PeriodKind::Weekly),
            "Resetting soon"
        );
        assert_eq!(
            i18n.format_period_remaining(86_400 + 3_600 + 60, PeriodKind::Weekly),
            "7-day period, 1 day, 1 hour, 1 minute remaining"
        );
    }

    #[test]
    fn named_timezone_keeps_dst_in_absolute_labels() {
        let i18n = I18n::from_parts(Language::English, Tz::America__New_York);
        assert!(i18n
            .format_timestamp(1_709_900_000)
            .unwrap()
            .ends_with("-05:00"));
        assert!(i18n
            .format_timestamp(1_715_000_000)
            .unwrap()
            .ends_with("-04:00"));
    }

    #[test]
    fn timezone_configuration_accepts_only_named_iana_ids() {
        assert_eq!(
            timezone_from_names(Some(":Asia/Tokyo"), Some("UTC")),
            Tz::Asia__Tokyo
        );
        assert_eq!(
            timezone_from_names(Some("Etc/GMT-9"), Some("UTC")),
            Tz::from_str("Etc/GMT-9").unwrap()
        );
        assert_eq!(
            timezone_from_names(Some("JST-9"), Some("America/New_York")),
            Tz::UTC
        );
        assert_eq!(
            timezone_from_names(Some("+09:00"), Some("America/New_York")),
            Tz::UTC
        );
        assert_eq!(
            timezone_from_names(Some("  "), Some("Europe/Paris")),
            Tz::Europe__Paris
        );
        assert_eq!(timezone_from_names(None, None), Tz::UTC);
    }

    #[test]
    fn startup_timezone_object_is_immutable_after_environment_resolution() {
        let startup = I18n::from_parts(Language::English, Tz::Asia__Tokyo);
        let before = startup.format_timestamp(0).unwrap();
        let after = startup.format_timestamp(0).unwrap();
        assert_eq!(before, after);
        assert!(before.ends_with("+09:00"));
    }
}
