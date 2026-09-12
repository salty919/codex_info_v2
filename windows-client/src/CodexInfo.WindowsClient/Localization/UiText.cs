// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.ComponentModel;
using System.Globalization;

namespace CodexInfo.WindowsClient.Localization;

public sealed record UiText(
    string LanguageCode,
    string LanguageName,
    string AppTitle,
    string AppSubtitle,
    string UsageStatus,
    string Graph,
    string Threads,
    string Legal,
    string Settings,
    string Refresh,
    string Refreshing,
    string RemainingQuota,
    string Account,
    string Authentication,
    string Plan,
    string ResetTime,
    string ObservedAt,
    string LastReceived,
    string RunningThreads,
    string NoRunningThreads,
    string Details,
    string ModelUsage,
    string Input,
    string CachedInput,
    string Output,
    string Tokens,
    string Dollars,
    string Connection,
    string ConnectionEndpoint,
    string Setup,
    string SetupTitle,
    string SetupIntro,
    string ConnectionGuide,
    string ConnectionGuideBody,
    string SshCommand,
    string ApiCommand,
    string Copy,
    string Copied,
    string Continue,
    string OpenSettings,
    string Save,
    string Cancel,
    string Close,
    string Minimize,
    string Maximize,
    string Language,
    string Appearance,
    string Connected,
    string Connecting,
    string Ready,
    string ApiError,
    string TransportError,
    string Initializing,
    string AuthRequired,
    string QuotaDanger,
    string QuotaWarning,
    string ResetWarning,
    string Unavailable,
    string AuthStart,
    string AuthCheck,
    string Retry,
    string Latest,
    string UnavailableDetails)
{
    public string Format(string template, params object[] values) => string.Format(CultureInfo.CurrentCulture, template, values);

    public string GraphWindowTitle => LanguageCode switch
    {
        "ja" => "利用状況の推移",
        "zh-Hans" => "使用情况趋势",
        "ko" => "사용량 추이",
        "es" => "Uso a lo largo del tiempo",
        "fr" => "Utilisation au fil du temps",
        "de" => "Nutzung im Zeitverlauf",
        "pt" => "Uso ao longo do tempo",
        "it" => "Utilizzo nel tempo",
        "ru" => "Использование с течением времени",
        _ => "Usage over time"
    };

    public string PeriodSelectorHeading => LanguageCode switch
    {
        "ja" => "期間",
        "zh-Hans" => "期间",
        "ko" => "기간",
        "es" => "Período",
        "fr" => "Période",
        "de" => "Zeitraum",
        "pt" => "Período",
        "it" => "Periodo",
        "ru" => "Период",
        _ => "Period"
    };

    public string GraphDollarMetric => LanguageCode switch
    {
        "ja" => "ドル",
        "zh-Hans" => "美元",
        "ko" => "달러",
        "es" => "Dólares",
        "fr" => "Dollars",
        "de" => "Dollar",
        "pt" => "Dólares",
        "it" => "Dollari",
        "ru" => "Доллары",
        _ => "Dollars"
    };

    public string GraphTokenMetric => Tokens;

    public string GraphDollarDescription => LanguageCode switch
    {
        "ja" => "時間ごとの累積消費ドル（モデル別） / 残量%",
        "zh-Hans" => "每小时累计消费美元（按模型）/ 剩余%",
        "ko" => "시간별 누적 소비 달러(모델별) / 잔여%",
        "es" => "Gasto acumulado por hora (por modelo) / % restante",
        "fr" => "Dépense cumulée par heure (par modèle) / % restant",
        "de" => "Kumulierte Ausgaben pro Stunde (nach Modell) / verbleibend %",
        "pt" => "Gasto acumulado por hora (por modelo) / % restante",
        "it" => "Spesa cumulativa oraria (per modello) / % restante",
        "ru" => "Накопленные расходы по часам (по моделям) / остаток %",
        _ => "Hourly cumulative spend (by model) / remaining %"
    };

    public string GraphTokenDescription => LanguageCode switch
    {
        "ja" => "時間ごとのトークン使用量（モデル別） / 残量%",
        "zh-Hans" => "每小时令牌使用量（按模型）/ 剩余%",
        "ko" => "시간별 토큰 사용량(모델별) / 잔여%",
        "es" => "Uso de tokens por hora (por modelo) / % restante",
        "fr" => "Utilisation horaire des jetons (par modèle) / % restant",
        "de" => "Token-Nutzung pro Stunde (nach Modell) / verbleibend %",
        "pt" => "Uso de tokens por hora (por modelo) / % restante",
        "it" => "Utilizzo orario dei token (per modello) / % restante",
        "ru" => "Использование токенов по часам (по моделям) / остаток %",
        _ => "Hourly token usage (by model) / remaining %"
    };

    public string ModelUsageDescription => LanguageCode == "ja"
        ? $"{Input} / {CachedInput} / {Output}（{Tokens}・{Dollars}）"
        : $"{Input} / {CachedInput} / {Output} ({Tokens} / {Dollars})";

    public string ModelLabel => LanguageCode switch
    {
        "ja" => "モデル",
        "zh-Hans" => "模型",
        "ko" => "모델",
        "es" => "Modelo",
        "fr" => "Modèle",
        "de" => "Modell",
        "pt" => "Modelo",
        "it" => "Modello",
        "ru" => "Модель",
        _ => "Model"
    };

    public string UnavailableValue => LanguageCode switch
    {
        "ja" => "未取得",
        "zh-Hans" => "未获取",
        "ko" => "가져오지 못함",
        "es" => "No disponible",
        "fr" => "Indisponible",
        "de" => "Nicht verfügbar",
        "pt" => "Indisponível",
        "it" => "Non disponibile",
        "ru" => "Недоступно",
        _ => "Unavailable"
    };

    public string GraphLoading => LanguageCode switch
    {
        "ja" => "期間データを読み込み中…",
        "zh-Hans" => "正在加载时间段数据…",
        "ko" => "기간 데이터를 불러오는 중…",
        "es" => "Cargando datos del período…",
        "fr" => "Chargement des données de la période…",
        "de" => "Zeitraumdaten werden geladen…",
        "pt" => "Carregando dados do período…",
        "it" => "Caricamento dei dati del periodo…",
        "ru" => "Загрузка данных периода…",
        _ => "Loading period data…"
    };

    public string GraphLoadFailed => LanguageCode switch
    {
        "ja" => "期間データを更新できません。前のグラフを保持しています。",
        "zh-Hans" => "无法更新时间段数据。仍显示之前的图表。",
        "ko" => "기간 데이터를 업데이트할 수 없습니다. 이전 그래프를 유지합니다.",
        "es" => "No se pudieron actualizar los datos. Se conserva el gráfico anterior.",
        "fr" => "Impossible d’actualiser les données. Le graphique précédent est conservé.",
        "de" => "Zeitraumdaten konnten nicht aktualisiert werden. Das vorherige Diagramm bleibt erhalten.",
        "pt" => "Não foi possível atualizar os dados. O gráfico anterior foi mantido.",
        "it" => "Impossibile aggiornare i dati. Il grafico precedente resta visualizzato.",
        "ru" => "Не удалось обновить данные. Предыдущий график сохранён.",
        _ => "Period data could not be updated. The previous graph is retained."
    };

    /// <summary>
    /// Explicitly identifies a durable settings failure without exposing the
    /// filesystem path or raw exception. The settings and setup surfaces use
    /// this message while keeping the window open for recovery.
    /// </summary>
    public string SettingsSaveFailed => LanguageCode switch
    {
        "ja" => "設定を保存できません。ファイルの権限または保存先を確認して、もう一度お試しください。",
        "zh-Hans" => "无法保存设置。请检查文件权限或保存位置，然后重试。",
        "ko" => "설정을 저장할 수 없습니다. 파일 권한이나 저장 위치를 확인한 후 다시 시도하세요.",
        "es" => "No se pudo guardar la configuración. Comprueba los permisos o la ubicación y vuelve a intentarlo.",
        "fr" => "Impossible d’enregistrer les paramètres. Vérifiez les droits ou l’emplacement, puis réessayez.",
        "de" => "Die Einstellungen konnten nicht gespeichert werden. Prüfen Sie Berechtigungen oder Speicherort und versuchen Sie es erneut.",
        "pt" => "Não foi possível salvar as configurações. Verifique as permissões ou o local e tente novamente.",
        "it" => "Impossibile salvare le impostazioni. Controlla i permessi o il percorso e riprova.",
        "ru" => "Не удалось сохранить настройки. Проверьте права или расположение и повторите попытку.",
        _ => "Settings could not be saved. Check the file permissions or location and try again."
    };

    public string QuotaWaiting => LanguageCode switch
    {
        "ja" => "利用枠の情報を待機しています",
        "zh-Hans" => "正在等待配额信息",
        "ko" => "사용량 정보를 기다리는 중입니다",
        "es" => "Esperando la información de cuota",
        "fr" => "En attente des informations de quota",
        "de" => "Warte auf Kontingentinformationen",
        "pt" => "Aguardando informações de cota",
        "it" => "In attesa delle informazioni sulla quota",
        "ru" => "Ожидание данных о квоте",
        _ => "Waiting for quota information"
    };

    public string WeeklyQuota => LanguageCode switch
    {
        "ja" => "7日周期：リセットまで",
        "zh-Hans" => "7天周期：重置倒计时",
        "ko" => "7일 주기: 재설정까지",
        "es" => "Ciclo de 7 días: hasta el restablecimiento",
        "fr" => "Cycle de 7 jours : jusqu’à la réinitialisation",
        "de" => "7-Tage-Zyklus: bis zum Zurücksetzen",
        "pt" => "Ciclo de 7 dias: até a redefinição",
        "it" => "Ciclo di 7 giorni: fino al ripristino",
        "ru" => "Цикл 7 дней: до сброса",
        _ => "7-day cycle: until reset"
    };

    public string MonthlyQuota => LanguageCode switch
    {
        "ja" => "月間：リセットまで",
        "zh-Hans" => "月度：重置倒计时",
        "ko" => "월간: 재설정까지",
        "es" => "Mensual: hasta el restablecimiento",
        "fr" => "Mensuel : jusqu’à la réinitialisation",
        "de" => "Monatlich: bis zum Zurücksetzen",
        "pt" => "Mensal: até a redefinição",
        "it" => "Mensile: fino al ripristino",
        "ru" => "Месячный: до сброса",
        _ => "Monthly: until reset"
    };

    public string LastReceivedUnavailable => LanguageCode switch
    {
        "ja" => "前回受信: 未取得",
        "zh-Hans" => "上次接收：未获取",
        "ko" => "마지막 수신: 가져오지 못함",
        "es" => "Última recepción: no disponible",
        "fr" => "Dernière réception : indisponible",
        "de" => "Letzter Empfang: nicht verfügbar",
        "pt" => "Último recebimento: indisponível",
        "it" => "Ultima ricezione: non disponibile",
        "ru" => "Последнее получение: недоступно",
        _ => "Last received: unavailable"
    };

    public string AppearanceDescription => LanguageCode switch
    {
        "ja" => "Windows向けの高コントラストなFluent配色、キーボードフォーカス、ツールチップを使用します。",
        "zh-Hans" => "使用适合 Windows 的高对比度 Fluent 配色、键盘焦点和工具提示。",
        "ko" => "Windows에 맞춘 고대비 Fluent 색상, 키보드 포커스 및 도구 설명을 사용합니다.",
        "es" => "Usa colores Fluent de alto contraste, foco de teclado y sugerencias para Windows.",
        "fr" => "Utilise une palette Fluent à contraste élevé, le focus clavier et des info-bulles Windows.",
        "de" => "Verwendet kontrastreiche Fluent-Farben, Tastaturfokus und Tooltips für Windows.",
        "pt" => "Usa cores Fluent de alto contraste, foco do teclado e dicas para Windows.",
        "it" => "Usa colori Fluent ad alto contrasto, focus da tastiera e suggerimenti per Windows.",
        "ru" => "Используются контрастная палитра Fluent, фокус клавиатуры и подсказки Windows.",
        _ => "Uses high-contrast Fluent colors, keyboard focus, and tooltips for Windows."
    };

    public string Other => LanguageCode switch { "ja" => "その他", "zh-Hans" => "其他", "ko" => "기타", "es" => "Otros", "fr" => "Autres", "de" => "Andere", "pt" => "Outros", "it" => "Altro", "ru" => "Другие", _ => "Other" };
    public string Parent => LanguageCode switch { "ja" => "親", "zh-Hans" => "父线程", "ko" => "상위", "es" => "Padre", "fr" => "Parent", "de" => "Übergeordnet", "pt" => "Pai", "it" => "Padre", "ru" => "Родитель", _ => "Parent" };
    public string ParentUnavailable => LanguageCode switch
    {
        "ja" => "親スレッドは現在非実行",
        "zh-Hans" => "父线程当前未运行",
        "ko" => "상위 스레드가 현재 실행 중이 아님",
        "es" => "El hilo principal no está activo",
        "fr" => "Le thread parent n’est pas actif",
        "de" => "Der übergeordnete Thread läuft derzeit nicht",
        "pt" => "A thread pai não está em execução",
        "it" => "Il thread principale non è in esecuzione",
        "ru" => "Родительский поток сейчас не выполняется",
        _ => "Parent thread is not currently running",
    };
    public string MainThread => LanguageCode switch { "ja" => "メイン", "zh-Hans" => "主线程", "ko" => "메인", "es" => "Principal", "fr" => "Principal", "de" => "Haupt", "pt" => "Principal", "it" => "Principale", "ru" => "Основной", _ => "Main" };
    public string SubThread => LanguageCode switch { "ja" => "サブ", "zh-Hans" => "子线程", "ko" => "하위", "es" => "Sub", "fr" => "Secondaire", "de" => "Untergeordnet", "pt" => "Sub", "it" => "Secondario", "ru" => "Подчинённый", _ => "Sub" };
    public string Context => LanguageCode switch { "ja" => "コンテキスト", "zh-Hans" => "上下文", "ko" => "컨텍스트", "es" => "Contexto", "fr" => "Contexte", "de" => "Kontext", "pt" => "Contexto", "it" => "Contesto", "ru" => "Контекст", _ => "Context" };
    public string Depth => LanguageCode switch { "ja" => "深さ", "zh-Hans" => "深度", "ko" => "깊이", "es" => "Profundidad", "fr" => "Profondeur", "de" => "Tiefe", "pt" => "Profundidade", "it" => "Profondità", "ru" => "Глубина", _ => "Depth" };
    public string Elapsed => LanguageCode switch { "ja" => "経過", "zh-Hans" => "已过", "ko" => "경과", "es" => "Transcurrido", "fr" => "Écoulé", "de" => "Vergangen", "pt" => "Decorrido", "it" => "Trascorso", "ru" => "Прошло", _ => "Elapsed" };
    public string Instruction => LanguageCode switch { "ja" => "指示", "zh-Hans" => "指令", "ko" => "지시", "es" => "Instrucción", "fr" => "Instruction", "de" => "Anweisung", "pt" => "Instrução", "it" => "Istruzione", "ru" => "Инструкция", _ => "Instruction" };
    public string ResetStartToNow => LanguageCode switch { "ja" => "リセット直後（0） → 現在時刻", "zh-Hans" => "重置后（0）→ 当前时间", "ko" => "재설정 직후(0) → 현재 시각", "es" => "Inicio del reinicio (0) → ahora", "fr" => "Début de réinitialisation (0) → maintenant", "de" => "Resetbeginn (0) → jetzt", "pt" => "Início da redefinição (0) → agora", "it" => "Inizio ripristino (0) → ora", "ru" => "Начало сброса (0) → сейчас", _ => "Reset start (0) → now" };
    public string BlueRemaining => LanguageCode switch { "ja" => "青: 残量", "zh-Hans" => "蓝：剩余", "ko" => "파랑: 잔여량", "es" => "Azul: restante", "fr" => "Bleu : restant", "de" => "Blau: verbleibend", "pt" => "Azul: restante", "it" => "Blu: residuo", "ru" => "Синий: остаток", _ => "Blue: remaining" };
    public string PurpleSol => LanguageCode switch { "ja" => "紫: SOL", "zh-Hans" => "紫：SOL", "ko" => "보라: SOL", "es" => "Morado: SOL", "fr" => "Violet : SOL", "de" => "Lila: SOL", "pt" => "Roxo: SOL", "it" => "Viola: SOL", "ru" => "Фиолетовый: SOL", _ => "Purple: SOL" };
    public string GreenTerra => LanguageCode switch { "ja" => "緑: TERRA", "zh-Hans" => "绿：TERRA", "ko" => "초록: TERRA", "es" => "Verde: TERRA", "fr" => "Vert : TERRA", "de" => "Grün: TERRA", "pt" => "Verde: TERRA", "it" => "Verde: TERRA", "ru" => "Зелёный: TERRA", _ => "Green: TERRA" };
    public string OrangeLuna => LanguageCode switch { "ja" => "橙: LUNA", "zh-Hans" => "橙：LUNA", "ko" => "주황: LUNA", "es" => "Naranja: LUNA", "fr" => "Orange : LUNA", "de" => "Orange: LUNA", "pt" => "Laranja: LUNA", "it" => "Arancione: LUNA", "ru" => "Оранжевый: LUNA", _ => "Orange: LUNA" };
    public string LegalCodeName => LanguageCode == "ja" ? "Codex Info" : "Codex Info";
    public string LegalWarrantyName => LanguageCode == "ja" ? "無保証" : "Warranty";
    public string LegalLicenseName => LanguageCode == "ja" ? "ライセンス" : "License";
    public string LegalFontName => LanguageCode == "ja" ? "フォント" : "Fonts";
    public string LegalProtocolName => LanguageCode == "ja" ? "プロトコルとAPI" : "Protocol and API";
    public string LegalSchemaName => LanguageCode == "ja" ? "スキーマ" : "Schema";
    public string LegalThirdPartyName => LanguageCode == "ja" ? "第三者ライセンス" : "Third-party licenses";
    public string LegalDetailsName => LanguageCode == "ja" ? "詳細" : "Details";
    public string LegalDistributionName => LanguageCode == "ja" ? "配布" : "Distribution";
    public string TimeZone => LanguageCode switch { "ja" => "表示タイムゾーン", "zh-Hans" => "显示时区", "ko" => "표시 시간대", "es" => "Zona horaria", "fr" => "Fuseau horaire", "de" => "Zeitzone", "pt" => "Fuso horário", "it" => "Fuso orario", "ru" => "Часовой пояс", _ => "Display time zone" };
    public string LocalTimeZone => LanguageCode switch { "ja" => "Windowsのローカル時刻", "zh-Hans" => "Windows 本地时间", "ko" => "Windows 현지 시간", "es" => "Hora local de Windows", "fr" => "Heure locale Windows", "de" => "Windows-Ortszeit", "pt" => "Hora local do Windows", "it" => "Ora locale di Windows", "ru" => "Местное время Windows", _ => "Windows local time" };
    public string UtcTimeZone => "UTC";
    public string CountUnit => LanguageCode switch { "ja" => "件", "zh-Hans" => "项", "ko" => "개", "es" => "", "fr" => "", "de" => "", "pt" => "", "it" => "", "ru" => "", _ => "" };
    public string EstimatedUnavailable => LanguageCode == "ja" ? "概算 —" : $"{Dollars} —";
    public string LastReceivedPrefix => LanguageCode switch { "ja" => "前回受信", "zh-Hans" => "上次接收", "ko" => "마지막 수신", "es" => "Última recepción", "fr" => "Dernière réception", "de" => "Letzter Empfang", "pt" => "Último recebimento", "it" => "Ultima ricezione", "ru" => "Последнее получение", _ => "Last received" };
    public string HistoricalLastRecordedPrefix => LanguageCode switch { "ja" => "最終記録", "zh-Hans" => "最后记录", "ko" => "최종 기록", "es" => "Último registro", "fr" => "Dernier enregistrement", "de" => "Letzte Aufzeichnung", "pt" => "Último registro", "it" => "Ultima registrazione", "ru" => "Последняя запись", _ => "Last recorded" };
    public string HistoricalQuotaPeriod => LanguageCode switch { "ja" => "記録終了時の期間", "zh-Hans" => "记录结束时的周期", "ko" => "기록 종료 시점의 기간", "es" => "Período al finalizar el registro", "fr" => "Période à la fin de l’enregistrement", "de" => "Zeitraum am Ende der Aufzeichnung", "pt" => "Período no fim do registro", "it" => "Periodo alla fine della registrazione", "ru" => "Период на конец записи", _ => "Recorded ending period" };
    public string HistoricalAccountDetail => LanguageCode switch { "ja" => "過去の記録を表示しています", "zh-Hans" => "正在显示历史记录", "ko" => "과거 기록을 표시하고 있습니다", "es" => "Se muestra un registro histórico", "fr" => "Un enregistrement historique est affiché", "de" => "Eine historische Aufzeichnung wird angezeigt", "pt" => "Um registro histórico está sendo exibido", "it" => "È visualizzata una registrazione storica", "ru" => "Показана историческая запись", _ => "A historical record is displayed" };
    public string HistoricalThreadsUnavailable => LanguageCode switch { "ja" => "記録された実行スレッドはありません", "zh-Hans" => "没有记录的运行线程", "ko" => "기록된 실행 중 스레드가 없습니다", "es" => "No hay hilos en ejecución registrados", "fr" => "Aucun thread en cours enregistré", "de" => "Keine laufenden Threads aufgezeichnet", "pt" => "Não há threads em execução registrados", "it" => "Nessun thread in esecuzione registrato", "ru" => "Нет записанных выполняющихся потоков", _ => "No running threads were recorded" };
    public string SignedInAccountSuffix => LanguageCode switch { "ja" => "［ログイン中］", "zh-Hans" => "［已登录］", "ko" => " [로그인 중]", "es" => " [sesión activa]", "fr" => " [connecté]", "de" => " [angemeldet]", "pt" => " [sessão ativa]", "it" => " [accesso]", "ru" => " [выполнен вход]", _ => " [signed in]" };
    public string HistoricalAccountSuffix => LanguageCode switch { "ja" => "［履歴］", "zh-Hans" => "［历史］", "ko" => " [기록]", "es" => " [historial]", "fr" => " [historique]", "de" => " [Verlauf]", "pt" => " [histórico]", "it" => " [cronologia]", "ru" => " [история]", _ => " [history]" };
    public string CurrentPeriodSuffix => LanguageCode switch { "ja" => "（現在）", "zh-Hans" => "（当前）", "ko" => " (현재)", "es" => " (actual)", "fr" => " (actuelle)", "de" => " (aktuell)", "pt" => " (atual)", "it" => " (corrente)", "ru" => " (текущий)", _ => " (current)" };

    public string FormatPeriodSelectorLabel(string localStart, bool current) => (LanguageCode, current) switch
    {
        ("ja", true) => $"現在｜開始 {localStart}",
        ("ja", false) => $"履歴｜開始 {localStart}",
        ("zh-Hans", true) => $"当前｜开始 {localStart}",
        ("zh-Hans", false) => $"历史｜开始 {localStart}",
        ("ko", true) => $"현재 · 시작 {localStart}",
        ("ko", false) => $"기록 · 시작 {localStart}",
        ("es", true) => $"Actual · Inicio {localStart}",
        ("es", false) => $"Historial · Inicio {localStart}",
        ("fr", true) => $"Actuelle · Début {localStart}",
        ("fr", false) => $"Historique · Début {localStart}",
        ("de", true) => $"Aktuell · Beginn {localStart}",
        ("de", false) => $"Verlauf · Beginn {localStart}",
        ("pt", true) => $"Atual · Início {localStart}",
        ("pt", false) => $"Histórico · Início {localStart}",
        ("it", true) => $"Corrente · Inizio {localStart}",
        ("it", false) => $"Cronologia · Inizio {localStart}",
        ("ru", true) => $"Текущий · Начало {localStart}",
        ("ru", false) => $"История · Начало {localStart}",
        (_, true) => $"Current · Start {localStart}",
        _ => $"History · Start {localStart}",
    };

    public string UpdateAvailableText(string version) => LanguageCode switch
    {
        "ja" => $"{version} を利用できます",
        "zh-Hans" => $"可用版本：{version}",
        "ko" => $"{version}을(를) 사용할 수 있습니다",
        "es" => $"{version} está disponible",
        "fr" => $"{version} est disponible",
        "de" => $"{version} ist verfügbar",
        "pt" => $"{version} está disponível",
        "it" => $"{version} è disponibile",
        "ru" => $"Доступна версия {version}",
        _ => $"{version} is available",
    };

    public string UpdateButtonText => LanguageCode switch
    {
        "ja" => "更新する",
        "zh-Hans" => "更新",
        "ko" => "업데이트",
        "es" => "Actualizar",
        "fr" => "Mettre à jour",
        "de" => "Aktualisieren",
        "pt" => "Atualizar",
        "it" => "Aggiorna",
        "ru" => "Обновить",
        _ => "Update",
    };

    public string UpdatePreparing => LanguageCode switch
    {
        "ja" => "更新を準備しています…",
        "zh-Hans" => "正在准备更新…",
        "ko" => "업데이트를 준비하는 중…",
        "es" => "Preparando la actualización…",
        "fr" => "Préparation de la mise à jour…",
        "de" => "Update wird vorbereitet…",
        "pt" => "Preparando a atualização…",
        "it" => "Preparazione dell’aggiornamento…",
        "ru" => "Подготовка обновления…",
        _ => "Preparing update…",
    };

    public string UpdateStarted => LanguageCode switch
    {
        "ja" => "更新を開始しました",
        "zh-Hans" => "更新已开始",
        "ko" => "업데이트를 시작했습니다",
        "es" => "La actualización ha comenzado",
        "fr" => "La mise à jour a démarré",
        "de" => "Update wurde gestartet",
        "pt" => "A atualização foi iniciada",
        "it" => "Aggiornamento avviato",
        "ru" => "Обновление запущено",
        _ => "Update started",
    };

    public string UpdateDownloadFailed => LanguageCode switch
    {
        "ja" => "更新のダウンロードに失敗しました",
        "zh-Hans" => "更新下载失败",
        "ko" => "업데이트 다운로드에 실패했습니다",
        "es" => "No se pudo descargar la actualización",
        "fr" => "Échec du téléchargement de la mise à jour",
        "de" => "Update konnte nicht heruntergeladen werden",
        "pt" => "Falha ao baixar a atualização",
        "it" => "Download dell’aggiornamento non riuscito",
        "ru" => "Не удалось скачать обновление",
        _ => "Update download failed",
    };

    public string UpdateIntegrityFailed => LanguageCode switch
    {
        "ja" => "更新の整合性を確認できませんでした",
        "zh-Hans" => "无法验证更新完整性",
        "ko" => "업데이트 무결성을 확인하지 못했습니다",
        "es" => "No se pudo verificar la integridad de la actualización",
        "fr" => "Impossible de vérifier l’intégrité de la mise à jour",
        "de" => "Integrität des Updates konnte nicht geprüft werden",
        "pt" => "Não foi possível verificar a integridade da atualização",
        "it" => "Impossibile verificare l’integrità dell’aggiornamento",
        "ru" => "Не удалось проверить целостность обновления",
        _ => "Update integrity check failed",
    };

    public string UpdateLaunchFailed => LanguageCode switch
    {
        "ja" => "更新を開始できませんでした",
        "zh-Hans" => "无法启动更新",
        "ko" => "업데이트를 시작하지 못했습니다",
        "es" => "No se pudo iniciar la actualización",
        "fr" => "Impossible de lancer la mise à jour",
        "de" => "Update konnte nicht gestartet werden",
        "pt" => "Não foi possível iniciar a atualização",
        "it" => "Impossibile avviare l’aggiornamento",
        "ru" => "Не удалось запустить обновление",
        _ => "Update could not be started",
    };

    public string StepConnection => LanguageCode switch { "ja" => "1. Linux API / SSH", "zh-Hans" => "1. Linux API / SSH", "ko" => "1. Linux API / SSH", _ => "1. Linux API / SSH" };
    public string StepAuth => LanguageCode switch { "ja" => "2. 認証", "zh-Hans" => "2. 认证", "ko" => "2. 인증", "es" => "2. Autenticación", "fr" => "2. Authentification", "de" => "2. Authentifizierung", "pt" => "2. Autenticação", "it" => "2. Autenticazione", "ru" => "2. Аутентификация", _ => "2. Authentication" };
    public string StepDone => LanguageCode switch { "ja" => "3. 完了", "zh-Hans" => "3. 完成", "ko" => "3. 완료", "es" => "3. Listo", "fr" => "3. Terminé", "de" => "3. Fertig", "pt" => "3. Concluído", "it" => "3. Fine", "ru" => "3. Готово", _ => "3. Done" };
    public string ConnectionProfileLabel => LanguageCode switch { "ja" => "接続プロファイル", "zh-Hans" => "连接配置", "ko" => "연결 프로필", _ => "Connection profile" };
    public string ConnectionSelectorLabel => LanguageCode switch { "ja" => "接続先", "zh-Hans" => "连接目标", "ko" => "연결 대상", _ => "Connection selector" };
    public string ConnectionProfileNone => LanguageCode switch { "ja" => "未設定", "zh-Hans" => "未设置", "ko" => "설정 안 함", _ => "Not configured" };
    public string ConnectionProfileWsl => "WSL";
    public string ConnectionProfileSsh => LanguageCode switch { "ja" => "SSH config alias", "zh-Hans" => "SSH config 别名", "ko" => "SSH config 별칭", _ => "SSH config alias" };
    public string ConnectionSelectorPlaceholder => LanguageCode switch { "ja" => "プロファイルを選択してください", "zh-Hans" => "请选择连接配置", "ko" => "연결 프로필을 선택하세요", _ => "Choose a connection profile" };

    public string FormatRemaining(long days, long hours, long minutes, bool lessThanMinute = false, bool immediate = false)
    {
        if (immediate) return LanguageCode switch
        {
            "ja" => "まもなくリセット",
            "zh-Hans" => "即将重置",
            "ko" => "곧 재설정됩니다",
            "es" => "Restablecimiento inminente",
            "fr" => "Réinitialisation imminente",
            "de" => "Wird bald zurückgesetzt",
            "pt" => "Redefinição em breve",
            "it" => "Ripristino imminente",
            "ru" => "Скоро сброс",
            _ => "Resetting soon"
        };
        if (lessThanMinute) return LanguageCode switch
        {
            "ja" => "残り 1分未満",
            "zh-Hans" => "剩余不到1分钟",
            "ko" => "1분 미만 남음",
            "es" => "Menos de 1 minuto",
            "fr" => "Moins d’une minute",
            "de" => "Weniger als 1 Minute",
            "pt" => "Menos de 1 minuto",
            "it" => "Meno di 1 minuto",
            "ru" => "Меньше минуты",
            _ => "Less than 1 minute"
        };
        if (days <= 0 && hours <= 0 && minutes <= 0)
        {
            return FormatRemaining(0, 0, 0, immediate: true);
        }
        var unitDay = LanguageCode switch { "ja" => "日", "zh-Hans" => "天", "ko" => "일", "es" => "d", "fr" => "j", "de" => "T", "pt" => "d", "it" => "g", "ru" => "д", _ => "d" };
        var unitHour = LanguageCode switch { "ja" => "時間", "zh-Hans" => "小时", "ko" => "시간", "es" => "h", "fr" => "h", "de" => "Std.", "pt" => "h", "it" => "h", "ru" => "ч", _ => "h" };
        var unitMinute = LanguageCode switch { "ja" => "分", "zh-Hans" => "分", "ko" => "분", "es" => "min", "fr" => "min", "de" => "Min.", "pt" => "min", "it" => "min", "ru" => "мин", _ => "min" };
        var prefix = LanguageCode switch { "ja" => "残り ", "zh-Hans" => "剩余 ", "ko" => "남은 시간 ", "es" => "Quedan ", "fr" => "Restant ", "de" => "Verbleibend ", "pt" => "Restam ", "it" => "Restano ", "ru" => "Осталось ", _ => "Remaining " };
        var parts = new List<string>(3);
        if (days > 0) parts.Add($"{days}{unitDay}");
        if (hours > 0) parts.Add($"{hours}{unitHour}");
        if (minutes > 0 || parts.Count == 0) parts.Add($"{minutes}{unitMinute}");
        return prefix + string.Join(" ", parts);
    }

    /// <summary>Formats an elapsed age without inventing a timestamp.</summary>
    public string FormatElapsed(long? timestamp, string label)
    {
        if (timestamp is not { } value)
        {
            return $"{label} —";
        }

        var seconds = Math.Max(0, DateTimeOffset.UtcNow.ToUnixTimeSeconds() - value);
        var amount = seconds / 3600;
        var unit = LanguageCode switch
        {
            "ja" => amount >= 24 ? "日" : amount > 0 ? "時間" : "分",
            "zh-Hans" => amount >= 24 ? "天" : amount > 0 ? "小时" : "分钟",
            "ko" => amount >= 24 ? "일" : amount > 0 ? "시간" : "분",
            "es" => amount >= 24 ? "d" : amount > 0 ? "h" : "min",
            "fr" => amount >= 24 ? "j" : amount > 0 ? "h" : "min",
            "de" => amount >= 24 ? "T" : amount > 0 ? "Std." : "Min.",
            "pt" => amount >= 24 ? "d" : amount > 0 ? "h" : "min",
            "it" => amount >= 24 ? "g" : amount > 0 ? "h" : "min",
            "ru" => amount >= 24 ? "д" : amount > 0 ? "ч" : "мин",
            _ => amount >= 24 ? "d" : amount > 0 ? "h" : "min",
        };
        amount = amount >= 24 ? amount / 24 : amount > 0 ? amount : Math.Max(1, seconds / 60);
        return $"{label} {amount}{unit}";
    }

    public string StatusDetailFor(string state, bool authLaunchFailed, bool hasSnapshot)
    {
        if (LanguageCode == "ja")
        {
            return state switch
            {
                "Connecting" => "SSH ローカルポート転送経由で Linux 側を確認しています。",
                "Ready" => "Linux 側の最新スナップショットを表示しています。",
                "QuotaDanger" => "残量は 2% 以下です。",
                "QuotaWarning" => "残量は 10% 以下です。",
                "ResetWarning" => "24 時間以内に利用枠がリセットされます。",
                "Initializing" => "準備が完了すると自動で更新します。",
                "AuthRequired" when authLaunchFailed => "認証プロセスを起動できませんでした。WSL と Codex CLI を確認して再試行してください。",
                "AuthRequired" => "「認証を開始」で Linux 側の Codex 認証を開き、完了後に自動更新します。",
                "ApiError" => "接続経路は利用できます。Linux 側の状態を確認してください。",
                "TransportError" when !hasSnapshot => "SSH トンネルまたは Linux アプリに接続できません。",
                "TransportError" => "SSH トンネルまたは Linux アプリに接続できません。前回受信の値を表示しています。",
                "ResponseError" when !hasSnapshot => "Linux 側から有効な応答を受け取れません。",
                "ResponseError" => "Linux 側から有効な応答を受け取れません。前回受信の値を表示しています。",
                _ => "SSH ローカルポート転送を確認してください。",
            };
        }
        if (LanguageCode == "zh-Hans") return state switch
        {
            "Connecting" => "正在通过 SSH 本地转发检查 Linux。",
            "Ready" => "正在显示最新的 Linux 快照。",
            "QuotaDanger" => "剩余配额不超过 2%。",
            "QuotaWarning" => "剩余配额不超过 10%。",
            "ResetWarning" => "配额将在 24 小时内重置。",
            "Initializing" => "Linux 准备完成后将自动刷新。",
            "AuthRequired" => "请开始 Linux 认证，完成后将自动刷新。",
            "ApiError" => "连接路径可用，请检查 Linux 状态。",
            "TransportError" => "SSH 隧道或 Linux 应用不可用。",
            "ResponseError" => "Linux 未返回有效响应。",
            _ => "请检查 SSH 本地转发。"
        };
        if (LanguageCode == "ko") return state switch
        {
            "Connecting" => "SSH 로컬 전달을 통해 Linux를 확인하는 중입니다.",
            "Ready" => "최신 Linux 스냅샷을 표시합니다.",
            "QuotaDanger" => "남은 사용량이 2% 이하입니다.",
            "QuotaWarning" => "남은 사용량이 10% 이하입니다.",
            "ResetWarning" => "24시간 이내에 사용량이 재설정됩니다.",
            "Initializing" => "Linux 준비가 끝나면 자동으로 새로 고칩니다.",
            "AuthRequired" => "Linux 인증을 시작하면 완료 후 자동으로 갱신합니다.",
            "ApiError" => "경로는 사용할 수 있습니다. Linux 상태를 확인하세요.",
            "TransportError" => "SSH 터널 또는 Linux 앱을 사용할 수 없습니다.",
            "ResponseError" => "Linux에서 유효한 응답을 받지 못했습니다.",
            _ => "SSH 로컬 전달을 확인하세요."
        };
        if (LanguageCode == "es") return state switch { "Connecting" => "Comprobando Linux mediante el túnel SSH.", "Ready" => "Mostrando la instantánea más reciente de Linux.", "QuotaDanger" => "La cuota restante es del 2% o menos.", "QuotaWarning" => "La cuota restante es del 10% o menos.", "ResetWarning" => "La cuota se restablece en 24 horas.", "Initializing" => "Se actualizará cuando Linux esté listo.", "AuthRequired" => "Inicia la autenticación de Linux para actualizar.", "ApiError" => "La ruta está disponible; comprueba Linux.", "TransportError" => "El túnel SSH o la aplicación Linux no están disponibles.", "ResponseError" => "Linux no devolvió una respuesta válida.", _ => "Comprueba el túnel SSH." };
        if (LanguageCode == "fr") return state switch { "Connecting" => "Vérification de Linux via le tunnel SSH.", "Ready" => "Dernier instantané Linux affiché.", "QuotaDanger" => "Le quota restant est inférieur ou égal à 2 %.", "QuotaWarning" => "Le quota restant est inférieur ou égal à 10 %.", "ResetWarning" => "Le quota sera réinitialisé sous 24 heures.", "Initializing" => "Actualisation dès que Linux est prêt.", "AuthRequired" => "Démarrez l’authentification Linux pour actualiser.", "ApiError" => "La route est disponible ; vérifiez Linux.", "TransportError" => "Le tunnel SSH ou l’application Linux est indisponible.", "ResponseError" => "Linux n’a pas renvoyé de réponse valide.", _ => "Vérifiez le tunnel SSH." };
        if (LanguageCode == "de") return state switch { "Connecting" => "Linux über den SSH-Tunnel wird geprüft.", "Ready" => "Der aktuelle Linux-Snapshot wird angezeigt.", "QuotaDanger" => "Das verbleibende Kontingent beträgt höchstens 2 %.", "QuotaWarning" => "Das verbleibende Kontingent beträgt höchstens 10 %.", "ResetWarning" => "Das Kontingent wird innerhalb von 24 Stunden zurückgesetzt.", "Initializing" => "Aktualisierung, sobald Linux bereit ist.", "AuthRequired" => "Linux-Authentifizierung starten, um zu aktualisieren.", "ApiError" => "Die Route ist verfügbar; Linux prüfen.", "TransportError" => "SSH-Tunnel oder Linux-App nicht verfügbar.", "ResponseError" => "Linux hat keine gültige Antwort geliefert.", _ => "SSH-Tunnel prüfen." };
        if (LanguageCode == "pt") return state switch { "Connecting" => "Verificando o Linux pelo túnel SSH.", "Ready" => "Exibindo o instantâneo mais recente do Linux.", "QuotaDanger" => "A cota restante é de 2% ou menos.", "QuotaWarning" => "A cota restante é de 10% ou menos.", "ResetWarning" => "A cota será redefinida em 24 horas.", "Initializing" => "Atualizará quando o Linux estiver pronto.", "AuthRequired" => "Inicie a autenticação do Linux para atualizar.", "ApiError" => "A rota está disponível; verifique o Linux.", "TransportError" => "O túnel SSH ou o app Linux está indisponível.", "ResponseError" => "O Linux não retornou uma resposta válida.", _ => "Verifique o túnel SSH." };
        if (LanguageCode == "it") return state switch { "Connecting" => "Controllo di Linux tramite tunnel SSH.", "Ready" => "Visualizzazione dell’istantanea Linux più recente.", "QuotaDanger" => "La quota residua è pari o inferiore al 2%.", "QuotaWarning" => "La quota residua è pari o inferiore al 10%.", "ResetWarning" => "La quota verrà ripristinata entro 24 ore.", "Initializing" => "Aggiornamento quando Linux sarà pronto.", "AuthRequired" => "Avvia l’autenticazione Linux per aggiornare.", "ApiError" => "Il percorso è disponibile; controlla Linux.", "TransportError" => "Il tunnel SSH o l’app Linux non è disponibile.", "ResponseError" => "Linux non ha restituito una risposta valida.", _ => "Controlla il tunnel SSH." };
        if (LanguageCode == "ru") return state switch { "Connecting" => "Проверка Linux через туннель SSH.", "Ready" => "Показан последний снимок Linux.", "QuotaDanger" => "Остаток квоты не превышает 2%.", "QuotaWarning" => "Остаток квоты не превышает 10%.", "ResetWarning" => "Квота будет сброшена в течение 24 часов.", "Initializing" => "Обновление после готовности Linux.", "AuthRequired" => "Запустите аутентификацию Linux для обновления.", "ApiError" => "Маршрут доступен; проверьте Linux.", "TransportError" => "Туннель SSH или приложение Linux недоступны.", "ResponseError" => "Linux не вернул корректный ответ.", _ => "Проверьте туннель SSH." };
        return state switch
        {
            "Connecting" => "Checking Linux through the SSH local forward.",
            "Ready" => "Showing the latest Linux snapshot.",
            "QuotaDanger" => "Remaining quota is 2% or less.",
            "QuotaWarning" => "Remaining quota is 10% or less.",
            "ResetWarning" => "The quota resets within 24 hours.",
            "Initializing" => "The monitor will refresh when Linux is ready.",
            "AuthRequired" when authLaunchFailed => "Could not start authentication. Check WSL and the Codex CLI, then retry.",
            "AuthRequired" => "Start Linux authentication and the monitor will refresh when it completes.",
            "ApiError" => "The route is available; check the Linux-side status.",
            "TransportError" when !hasSnapshot => "The SSH tunnel or Linux app is unavailable.",
            "TransportError" => "The SSH tunnel or Linux app is unavailable. Showing the last received values.",
            "ResponseError" when !hasSnapshot => "Linux returned no valid response.",
            "ResponseError" => "Linux returned no valid response. Showing the last received values.",
            _ => "Check the SSH local forward.",
        };
    }

    public string StaleValueSuffix => LanguageCode switch
    {
        "ja" => "（現在は更新できていません）",
        "zh-Hans" => "（当前无法更新）",
        "ko" => "(현재 업데이트할 수 없음)",
        "es" => " (no se puede actualizar ahora)",
        "fr" => " (mise à jour indisponible)",
        "de" => " (derzeit keine Aktualisierung)",
        "pt" => " (não é possível atualizar agora)",
        "it" => " (aggiornamento non disponibile)",
        "ru" => " (обновление недоступно)",
        _ => " (not updating now)"
    };

    public string SshCommandHint => LanguageCode switch
    {
        "ja" => "これは例示コマンドです。user@linux-host を実際のSSHユーザー名とホスト名またはIPアドレスへ置き換えてから実行してください。そのままでは接続できません。",
        "zh-Hans" => "这是示例命令。请先将 user@linux-host 替换为实际的 SSH 用户名和主机名或 IP 地址；不能直接照此执行。",
        "ko" => "예시 명령입니다. user@linux-host를 실제 SSH 사용자 이름과 호스트 이름 또는 IP 주소로 바꾼 뒤 실행하세요. 그대로는 연결되지 않습니다.",
        "de" => "Dies ist ein Beispiel. Ersetzen Sie user@linux-host durch den tatsächlichen SSH-Benutzer und Hostnamen oder die IP-Adresse; unverändert kann der Befehl keine Verbindung herstellen.",
        "fr" => "Commande d’exemple. Remplacez user@linux-host par l’utilisateur SSH et le nom d’hôte ou l’adresse IP réels avant l’exécution.",
        "es" => "Comando de ejemplo. Sustituye user@linux-host por el usuario SSH y el nombre de host o la IP reales antes de ejecutarlo.",
        "pt" => "Comando de exemplo. Substitua user@linux-host pelo usuário SSH e pelo nome do host ou IP reais antes de executar.",
        "it" => "Comando di esempio. Sostituisci user@linux-host con l'utente SSH e il nome host o IP reali prima di eseguirlo.",
        "ru" => "Это пример команды. Перед запуском замените user@linux-host на реальное имя пользователя SSH и имя хоста или IP-адрес.",
        _ => "Example command. Replace user@linux-host with the actual SSH user and host name or IP address before running it."
    };

    public string SshUserLabel => LanguageCode switch
    {
        "ja" => "SSHユーザー名",
        "zh-Hans" => "SSH 用户名",
        "ko" => "SSH 사용자 이름",
        "de" => "SSH-Benutzer",
        "fr" => "Utilisateur SSH",
        "es" => "Usuario SSH",
        "pt" => "Usuário SSH",
        "it" => "Utente SSH",
        "ru" => "Пользователь SSH",
        _ => "SSH user"
    };

    public string SshHostLabel => LanguageCode switch
    {
        "ja" => "Linuxホスト名 / IP",
        "zh-Hans" => "Linux 主机名 / IP",
        "ko" => "Linux 호스트 이름 / IP",
        "de" => "Linux-Hostname / IP",
        "fr" => "Hôte Linux / IP",
        "es" => "Host Linux / IP",
        "pt" => "Host Linux / IP",
        "it" => "Host Linux / IP",
        "ru" => "Имя хоста Linux / IP",
        _ => "Linux host / IP"
    };

    public string SshConfigAliasLabel => LanguageCode switch
    {
        "ja" => "SSH configから選択（任意）",
        "zh-Hans" => "从 SSH config 选择（可选）",
        "ko" => "SSH config에서 선택(선택 사항)",
        _ => "Choose from SSH config (optional)"
    };

    public string SshConfigAliasPlaceholder => LanguageCode switch
    {
        "ja" => "Host aliasが見つかった場合に選択できます",
        "zh-Hans" => "检测到 Host alias 时可在此选择",
        "ko" => "감지된 Host alias를 여기서 선택할 수 있습니다",
        _ => "Detected Host aliases appear here"
    };

    public string SshUserPlaceholder => LanguageCode == "ja" ? "任意: salty（configのUser使用可）" : "Optional: salty (config User allowed)";
    public string SshHostPlaceholder => LanguageCode == "ja" ? "例: 192.168.1.20 または Host alias" : "e.g. 192.168.1.20 or a Host alias";
    public string SshStart => LanguageCode switch
    {
        "ja" => "SSH転送を開始",
        "zh-Hans" => "启动 SSH 转发",
        "ko" => "SSH 전달 시작",
        "de" => "SSH-Weiterleitung starten",
        "fr" => "Démarrer le tunnel SSH",
        "es" => "Iniciar túnel SSH",
        "pt" => "Iniciar túnel SSH",
        "it" => "Avvia tunnel SSH",
        "ru" => "Запустить SSH-туннель",
        _ => "Start SSH forwarding"
    };

    public string SshStop => LanguageCode switch
    {
        "ja" => "SSH転送を停止",
        "zh-Hans" => "停止 SSH 转发",
        "ko" => "SSH 전달 중지",
        "de" => "SSH-Weiterleitung stoppen",
        "fr" => "Arrêter le tunnel SSH",
        "es" => "Detener túnel SSH",
        "pt" => "Parar túnel SSH",
        "it" => "Arresta tunnel SSH",
        "ru" => "Остановить SSH-туннель",
        _ => "Stop SSH forwarding"
    };

    public string SshNotReady => LanguageCode switch
    {
        "ja" => "上の user@linux-host は例です。Linuxホスト名/IPまたはSSH configのHost aliasを入力してください。ユーザー名はconfigのUserを使う場合は空欄にできます。入力値は保存されません。",
        "zh-Hans" => "上面的 user@linux-host 只是示例。请输入 Linux 主机名/IP 或 SSH config 的 Host alias。若使用 config 的 User，可将用户名留空。输入内容不会保存。",
        "ko" => "위의 user@linux-host는 예시입니다. Linux 호스트 이름/IP 또는 SSH config의 Host alias를 입력하세요. config의 User를 사용하면 사용자 이름을 비워둘 수 있습니다. 입력값은 저장되지 않습니다.",
        _ => "The user@linux-host text above is an example. Enter a Linux host/IP or an SSH config Host alias. Leave the user empty to use User from config. Values are not saved."
    };

    public string SshRunningStatus => LanguageCode switch
    {
        "ja" => "SSH転送を実行中です。接続を確認しています。",
        "zh-Hans" => "SSH 转发正在运行，正在检查连接。",
        "ko" => "SSH 전달이 실행 중이며 연결을 확인하고 있습니다.",
        _ => "SSH forwarding is running; checking the connection."
    };

    public string SshLaunchFailedStatus => LanguageCode switch
    {
        "ja" => "ssh.exeを起動できませんでした。Windows OpenSSHが利用可能か確認してください。",
        "zh-Hans" => "无法启动 ssh.exe。请确认 Windows OpenSSH 可用。",
        "ko" => "ssh.exe를 시작할 수 없습니다. Windows OpenSSH를 사용할 수 있는지 확인하세요.",
        _ => "ssh.exe could not be started. Check that Windows OpenSSH is available."
    };

    public string SshReadyStatus => LanguageCode switch
    {
        "ja" => "この接続先でSSH転送を開始できます。鍵・パスワード・ホスト鍵確認はssh.exeが表示します。",
        "zh-Hans" => "可以使用此目标启动 SSH 转发。密钥、密码和主机密钥确认由 ssh.exe 显示。",
        "ko" => "이 대상으로 SSH 전달을 시작할 수 있습니다. 키, 암호 및 호스트 키 확인은 ssh.exe가 표시합니다.",
        _ => "SSH forwarding is ready. ssh.exe will show key, password, and host-key prompts."
    };
}

public static class LocalizationService
{
    private static readonly UiText Japanese = new(
        "ja", "日本語", "Codex Info Monitor", "Windows 監視クライアント", "利用状況", "推移", "Threads", "法的通知", "設定", "更新", "更新中…",
        "残り利用枠", "アカウント", "認証", "プラン", "リセット時刻", "Linux の観測時刻", "前回受信", "実行中のスレッド", "実行中のスレッドはありません", "詳細",
        "モデル別利用量", "入力", "キャッシュ入力", "出力", "トークン", "概算ドル", "接続", "接続先: 127.0.0.1:8787（SSH ローカルポート転送専用）", "初期設定",
        "Codex Infoへようこそ", "Linux側のAPIとSSHローカル転送を確認して、安全に監視を始めます。認証情報やトークンは保存しません。", "接続ガイド",
        "SSHユーザー名とLinuxホスト名/IPまたはSSH configのHost aliasを入力し、「SSH転送を開始」を押してください。Linux側ではCodex InfoをUIなしのAPIモードで起動します。記録daemonも自動起動し、UIを閉じても履歴を保護します。", "ssh -N -L 8787:127.0.0.1:8787 user@linux-host", "codex_info --port 8787", "コピー", "コピーしました", "続行", "設定を開く", "保存", "キャンセル", "閉じる", "最小化", "最大化", "言語", "外観", "接続済み", "接続中", "正常", "Linux 側の取得エラー", "接続エラー", "Linux 側で準備中", "Linux 側で認証が必要です", "残量不足", "残量警告", "リセット警告", "未取得", "認証を開始", "認証を確認", "再試行", "最新", "詳細データは未取得");

    private static readonly UiText English = Japanese with
    {
        LanguageCode = "en",
        LanguageName = "English",
        AppTitle = "Codex Info Monitor",
        AppSubtitle = "Windows monitoring client",
        UsageStatus = "Usage",
        Graph = "Trends",
        Threads = "Threads",
        Legal = "Legal",
        Settings = "Settings",
        Refresh = "Refresh",
        Refreshing = "Refreshing…",
        RemainingQuota = "Remaining quota",
        Account = "Account",
        Authentication = "Authentication",
        Plan = "Plan",
        ResetTime = "Reset time",
        ObservedAt = "Linux observation",
        LastReceived = "Last received",
        RunningThreads = "Running threads",
        NoRunningThreads = "No running threads",
        Details = "Details",
        ModelUsage = "Usage by model",
        Input = "Input",
        CachedInput = "Cached input",
        Output = "Output",
        Tokens = "Tokens",
        Dollars = "Estimated dollars",
        Connection = "Connection",
        ConnectionEndpoint = "Endpoint: 127.0.0.1:8787 (SSH local forwarding)",
        Setup = "Setup",
        SetupTitle = "Welcome to Codex Info",
        SetupIntro = "We will verify the Linux API and SSH local forwarding before monitoring starts. Credentials and tokens are never stored.",
        ConnectionGuide = "Connection guide",
        ConnectionGuideBody = "Enter an SSH user and Linux host/IP or a Host alias from your SSH config, then press Start SSH forwarding. Launch Codex Info in headless API mode on Linux. The recorder daemon starts automatically and keeps history after the UI closes.",
        Copy = "Copy",
        Copied = "Copied",
        Continue = "Continue",
        OpenSettings = "Open settings",
        Save = "Save",
        Cancel = "Cancel",
        Close = "Close",
        Minimize = "Minimize",
        Maximize = "Maximize",
        Language = "Language",
        Appearance = "Appearance",
        Connected = "Connected",
        Connecting = "Connecting",
        Ready = "Ready",
        ApiError = "Linux API error",
        TransportError = "Connection error",
        Initializing = "Linux is preparing",
        AuthRequired = "Linux authentication required",
        QuotaDanger = "Quota critical",
        QuotaWarning = "Quota warning",
        ResetWarning = "Reset soon",
        Unavailable = "Unavailable",
        AuthStart = "Start authentication",
        AuthCheck = "Check authentication",
        Retry = "Retry",
        Latest = "Latest",
        UnavailableDetails = "Details unavailable"
    };

    private static readonly Dictionary<string, UiText> Catalog = new(StringComparer.OrdinalIgnoreCase)
    {
        ["ja"] = Japanese,
        ["en"] = English,
        ["zh-Hans"] = English with
        {
            LanguageCode = "zh-Hans",
            LanguageName = "简体中文",
            AppSubtitle = "Windows 监控客户端",
            UsageStatus = "使用情况",
            Graph = "趋势",
            Legal = "法律声明",
            Settings = "设置",
            Refresh = "刷新",
            Refreshing = "刷新中…",
            RemainingQuota = "剩余配额",
            Account = "账户",
            Authentication = "认证",
            Plan = "套餐",
            ResetTime = "重置时间",
            ObservedAt = "Linux 观测时间",
            LastReceived = "上次接收",
            RunningThreads = "运行中的线程",
            NoRunningThreads = "没有运行中的线程",
            Details = "详细信息",
            ModelUsage = "按模型用量",
            Input = "输入",
            CachedInput = "缓存输入",
            Output = "输出",
            Tokens = "令牌",
            Dollars = "预估美元",
            Connection = "连接",
            Setup = "初始设置",
            SetupTitle = "欢迎使用 Codex Info",
            Continue = "继续",
            OpenSettings = "打开设置",
            Save = "保存",
            Close = "关闭",
            Language = "语言",
            Appearance = "外观",
            Connected = "已连接",
            Connecting = "连接中",
            Ready = "正常",
            AuthRequired = "需要 Linux 认证",
            QuotaDanger = "配额不足",
            QuotaWarning = "配额警告",
            ResetWarning = "即将重置",
            Unavailable = "不可用",
            AuthStart = "开始认证",
            AuthCheck = "检查认证",
            Retry = "重试",
            Latest = "最新",
            UnavailableDetails = "详细数据不可用"
        },
        ["ko"] = English with
        {
            LanguageCode = "ko",
            LanguageName = "한국어",
            AppSubtitle = "Windows 모니터링 클라이언트",
            UsageStatus = "사용 현황",
            Graph = "추이",
            Legal = "법적 고지",
            Settings = "설정",
            Refresh = "새로 고침",
            Refreshing = "새로 고치는 중…",
            RemainingQuota = "남은 사용량",
            Account = "계정",
            Authentication = "인증",
            Plan = "플랜",
            ResetTime = "재설정 시각",
            ObservedAt = "Linux 관측 시각",
            LastReceived = "마지막 수신",
            RunningThreads = "실행 중인 스레드",
            NoRunningThreads = "실행 중인 스레드가 없습니다",
            Details = "세부 정보",
            ModelUsage = "모델별 사용량",
            Input = "입력",
            CachedInput = "캐시 입력",
            Output = "출력",
            Tokens = "토큰",
            Dollars = "예상 달러",
            Connection = "연결",
            Setup = "초기 설정",
            SetupTitle = "Codex Info에 오신 것을 환영합니다",
            Continue = "계속",
            OpenSettings = "설정 열기",
            Save = "저장",
            Close = "닫기",
            Language = "언어",
            Appearance = "모양",
            Connected = "연결됨",
            Connecting = "연결 중",
            Ready = "정상",
            AuthRequired = "Linux 인증 필요",
            QuotaDanger = "잔여량 부족",
            QuotaWarning = "잔여량 경고",
            ResetWarning = "곧 재설정",
            Unavailable = "사용할 수 없음",
            AuthStart = "인증 시작",
            AuthCheck = "인증 확인",
            Retry = "다시 시도",
            Latest = "최신",
            UnavailableDetails = "세부 데이터를 가져올 수 없음"
        },
        ["es"] = English with
        {
            LanguageCode = "es",
            LanguageName = "Español",
            AppSubtitle = "Cliente de supervisión para Windows",
            UsageStatus = "Uso",
            Graph = "Tendencias",
            Legal = "Avisos legales",
            Settings = "Configuración",
            Refresh = "Actualizar",
            Refreshing = "Actualizando…",
            RemainingQuota = "Cuota restante",
            Account = "Cuenta",
            Authentication = "Autenticación",
            Plan = "Plan",
            ResetTime = "Hora de restablecimiento",
            ObservedAt = "Observación de Linux",
            LastReceived = "Última recepción",
            RunningThreads = "Hilos activos",
            NoRunningThreads = "No hay hilos activos",
            Details = "Detalles",
            ModelUsage = "Uso por modelo",
            Input = "Entrada",
            CachedInput = "Entrada en caché",
            Output = "Salida",
            Tokens = "Tokens",
            Dollars = "Dólares estimados",
            Connection = "Conexión",
            Setup = "Configuración inicial",
            SetupTitle = "Te damos la bienvenida a Codex Info",
            Continue = "Continuar",
            OpenSettings = "Abrir configuración",
            Save = "Guardar",
            Close = "Cerrar",
            Language = "Idioma",
            Appearance = "Apariencia",
            Connected = "Conectado",
            Connecting = "Conectando",
            Ready = "Listo",
            AuthRequired = "Se requiere autenticación de Linux",
            QuotaDanger = "Cuota crítica",
            QuotaWarning = "Advertencia de cuota",
            ResetWarning = "Restablecimiento próximo",
            Unavailable = "No disponible",
            AuthStart = "Iniciar autenticación",
            AuthCheck = "Comprobar autenticación",
            Retry = "Reintentar",
            Latest = "Más reciente",
            UnavailableDetails = "Detalles no disponibles"
        },
        ["fr"] = English with
        {
            LanguageCode = "fr",
            LanguageName = "Français",
            AppSubtitle = "Client de surveillance Windows",
            UsageStatus = "Utilisation",
            Graph = "Tendances",
            Legal = "Mentions légales",
            Settings = "Paramètres",
            Refresh = "Actualiser",
            Refreshing = "Actualisation…",
            RemainingQuota = "Quota restant",
            Account = "Compte",
            Authentication = "Authentification",
            Plan = "Forfait",
            ResetTime = "Heure de réinitialisation",
            ObservedAt = "Observation Linux",
            LastReceived = "Dernière réception",
            RunningThreads = "Threads actifs",
            NoRunningThreads = "Aucun thread actif",
            Details = "Détails",
            ModelUsage = "Utilisation par modèle",
            Input = "Entrée",
            CachedInput = "Entrée en cache",
            Output = "Sortie",
            Tokens = "Jetons",
            Dollars = "Dollars estimés",
            Connection = "Connexion",
            Setup = "Configuration initiale",
            SetupTitle = "Bienvenue dans Codex Info",
            Continue = "Continuer",
            OpenSettings = "Ouvrir les paramètres",
            Save = "Enregistrer",
            Close = "Fermer",
            Language = "Langue",
            Appearance = "Apparence",
            Connected = "Connecté",
            Connecting = "Connexion…",
            Ready = "Prêt",
            AuthRequired = "Authentification Linux requise",
            QuotaDanger = "Quota critique",
            QuotaWarning = "Alerte de quota",
            ResetWarning = "Réinitialisation imminente",
            Unavailable = "Indisponible",
            AuthStart = "Démarrer l’authentification",
            AuthCheck = "Vérifier l’authentification",
            Retry = "Réessayer",
            Latest = "À jour",
            UnavailableDetails = "Détails indisponibles"
        },
        ["de"] = English with
        {
            LanguageCode = "de",
            LanguageName = "Deutsch",
            AppSubtitle = "Windows-Überwachungsclient",
            UsageStatus = "Nutzung",
            Graph = "Verlauf",
            Legal = "Rechtliche Hinweise",
            Settings = "Einstellungen",
            Refresh = "Aktualisieren",
            Refreshing = "Wird aktualisiert…",
            RemainingQuota = "Verbleibendes Kontingent",
            Account = "Konto",
            Authentication = "Authentifizierung",
            Plan = "Tarif",
            ResetTime = "Zurücksetzzeit",
            ObservedAt = "Linux-Beobachtung",
            LastReceived = "Letzter Empfang",
            RunningThreads = "Aktive Threads",
            NoRunningThreads = "Keine aktiven Threads",
            Details = "Details",
            ModelUsage = "Nutzung nach Modell",
            Input = "Eingabe",
            CachedInput = "Cache-Eingabe",
            Output = "Ausgabe",
            Tokens = "Token",
            Dollars = "Geschätzte Dollar",
            Connection = "Verbindung",
            Setup = "Ersteinrichtung",
            SetupTitle = "Willkommen bei Codex Info",
            Continue = "Weiter",
            OpenSettings = "Einstellungen öffnen",
            Save = "Speichern",
            Close = "Schließen",
            Language = "Sprache",
            Appearance = "Darstellung",
            Connected = "Verbunden",
            Connecting = "Verbindung wird hergestellt",
            Ready = "Bereit",
            AuthRequired = "Linux-Authentifizierung erforderlich",
            QuotaDanger = "Kontingent kritisch",
            QuotaWarning = "Kontingentwarnung",
            ResetWarning = "Zurücksetzung steht bevor",
            Unavailable = "Nicht verfügbar",
            AuthStart = "Authentifizierung starten",
            AuthCheck = "Authentifizierung prüfen",
            Retry = "Erneut versuchen",
            Latest = "Aktuell",
            UnavailableDetails = "Details nicht verfügbar"
        },
        ["pt"] = English with
        {
            LanguageCode = "pt",
            LanguageName = "Português",
            AppSubtitle = "Cliente de monitoramento do Windows",
            UsageStatus = "Uso",
            Graph = "Tendências",
            Legal = "Avisos legais",
            Settings = "Configurações",
            Refresh = "Atualizar",
            Refreshing = "Atualizando…",
            RemainingQuota = "Cota restante",
            Account = "Conta",
            Authentication = "Autenticação",
            Plan = "Plano",
            ResetTime = "Hora de redefinição",
            ObservedAt = "Observação do Linux",
            LastReceived = "Último recebimento",
            RunningThreads = "Threads em execução",
            NoRunningThreads = "Nenhuma thread em execução",
            Details = "Detalhes",
            ModelUsage = "Uso por modelo",
            Input = "Entrada",
            CachedInput = "Entrada em cache",
            Output = "Saída",
            Tokens = "Tokens",
            Dollars = "Dólares estimados",
            Connection = "Conexão",
            Setup = "Configuração inicial",
            SetupTitle = "Boas-vindas ao Codex Info",
            Continue = "Continuar",
            OpenSettings = "Abrir configurações",
            Save = "Salvar",
            Close = "Fechar",
            Language = "Idioma",
            Appearance = "Aparência",
            Connected = "Conectado",
            Connecting = "Conectando",
            Ready = "Pronto",
            AuthRequired = "Autenticação do Linux necessária",
            QuotaDanger = "Cota crítica",
            QuotaWarning = "Alerta de cota",
            ResetWarning = "Redefinição próxima",
            Unavailable = "Indisponível",
            AuthStart = "Iniciar autenticação",
            AuthCheck = "Verificar autenticação",
            Retry = "Tentar novamente",
            Latest = "Mais recente",
            UnavailableDetails = "Detalhes indisponíveis"
        },
        ["it"] = English with
        {
            LanguageCode = "it",
            LanguageName = "Italiano",
            AppSubtitle = "Client di monitoraggio Windows",
            UsageStatus = "Utilizzo",
            Graph = "Andamento",
            Legal = "Note legali",
            Settings = "Impostazioni",
            Refresh = "Aggiorna",
            Refreshing = "Aggiornamento…",
            RemainingQuota = "Quota residua",
            Account = "Account",
            Authentication = "Autenticazione",
            Plan = "Piano",
            ResetTime = "Ora di ripristino",
            ObservedAt = "Osservazione Linux",
            LastReceived = "Ultima ricezione",
            RunningThreads = "Thread in esecuzione",
            NoRunningThreads = "Nessun thread in esecuzione",
            Details = "Dettagli",
            ModelUsage = "Utilizzo per modello",
            Input = "Input",
            CachedInput = "Input in cache",
            Output = "Output",
            Tokens = "Token",
            Dollars = "Dollari stimati",
            Connection = "Connessione",
            Setup = "Configurazione iniziale",
            SetupTitle = "Benvenuto in Codex Info",
            Continue = "Continua",
            OpenSettings = "Apri impostazioni",
            Save = "Salva",
            Close = "Chiudi",
            Language = "Lingua",
            Appearance = "Aspetto",
            Connected = "Connesso",
            Connecting = "Connessione in corso",
            Ready = "Pronto",
            AuthRequired = "Autenticazione Linux richiesta",
            QuotaDanger = "Quota critica",
            QuotaWarning = "Avviso quota",
            ResetWarning = "Ripristino imminente",
            Unavailable = "Non disponibile",
            AuthStart = "Avvia autenticazione",
            AuthCheck = "Verifica autenticazione",
            Retry = "Riprova",
            Latest = "Più recente",
            UnavailableDetails = "Dettagli non disponibili"
        },
        ["ru"] = English with
        {
            LanguageCode = "ru",
            LanguageName = "Русский",
            AppSubtitle = "Клиент мониторинга Windows",
            UsageStatus = "Использование",
            Graph = "Динамика",
            Legal = "Правовая информация",
            Settings = "Настройки",
            Refresh = "Обновить",
            Refreshing = "Обновление…",
            RemainingQuota = "Оставшаяся квота",
            Account = "Аккаунт",
            Authentication = "Аутентификация",
            Plan = "Тариф",
            ResetTime = "Время сброса",
            ObservedAt = "Наблюдение Linux",
            LastReceived = "Последнее получение",
            RunningThreads = "Активные потоки",
            NoRunningThreads = "Нет активных потоков",
            Details = "Подробности",
            ModelUsage = "Использование по моделям",
            Input = "Ввод",
            CachedInput = "Ввод из кэша",
            Output = "Вывод",
            Tokens = "Токены",
            Dollars = "Расчётные доллары",
            Connection = "Подключение",
            Setup = "Первоначальная настройка",
            SetupTitle = "Добро пожаловать в Codex Info",
            Continue = "Продолжить",
            OpenSettings = "Открыть настройки",
            Save = "Сохранить",
            Close = "Закрыть",
            Language = "Язык",
            Appearance = "Внешний вид",
            Connected = "Подключено",
            Connecting = "Подключение",
            Ready = "Готово",
            AuthRequired = "Требуется аутентификация Linux",
            QuotaDanger = "Критическая квота",
            QuotaWarning = "Предупреждение о квоте",
            ResetWarning = "Скорый сброс",
            Unavailable = "Недоступно",
            AuthStart = "Начать аутентификацию",
            AuthCheck = "Проверить аутентификацию",
            Retry = "Повторить",
            Latest = "Последние данные",
            UnavailableDetails = "Подробности недоступны"
        },
    };

    private static UiText current = Japanese;
    public static TimeZoneInfo DisplayTimeZone { get; private set; } = TimeZoneInfo.Local;

    public static event EventHandler? LanguageChanged;

    public static UiText Current => current;

    public static IReadOnlyList<UiText> Languages { get; } = Catalog.Values.DistinctBy(value => value.LanguageCode).ToArray();

    public static string NormalizeLanguageCode(string? code)
    {
        var normalized = string.IsNullOrWhiteSpace(code) ? "ja" : code.Trim();
        if (Catalog.TryGetValue(normalized, out var language)) return language.LanguageCode;

        var primary = normalized.Replace('_', '-').Split('-', 2)[0];
        if (string.Equals(primary, "zh", StringComparison.OrdinalIgnoreCase)) return "zh-Hans";
        return Catalog.TryGetValue(primary, out language) ? language.LanguageCode : English.LanguageCode;
    }

    public static void SetLanguage(string? code)
    {
        var normalized = NormalizeLanguageCode(code);
        if (!Catalog.TryGetValue(normalized, out var next)) next = English;
        var cultureName = next.LanguageCode switch
        {
            "ja" => "ja-JP",
            "zh-Hans" => "zh-CN",
            "pt" => "pt-BR",
            _ => next.LanguageCode,
        };
        var culture = CultureInfo.GetCultureInfo(cultureName);
        CultureInfo.CurrentUICulture = culture;
        CultureInfo.CurrentCulture = culture;
        if (ReferenceEquals(current, next)) return;
        current = next;
        LanguageChanged?.Invoke(null, EventArgs.Empty);
    }

    public static void SetTimeZone(string? id)
    {
        var next = string.Equals(id, "UTC", StringComparison.OrdinalIgnoreCase) ? TimeZoneInfo.Utc : TimeZoneInfo.Local;
        if (DisplayTimeZone.Equals(next)) return;
        DisplayTimeZone = next;
        // Date/time labels are part of the selected presentation locale. Reuse
        // this notification so every window refreshes after a timezone change.
        LanguageChanged?.Invoke(null, EventArgs.Empty);
    }
}
