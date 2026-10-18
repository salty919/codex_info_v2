// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using CodexInfo.WindowsClient.Core;

namespace CodexInfo.WindowsClient.Localization;

internal static class RecordedUsageText
{
    private static string Pick(params string[] values) => values[LocalizationService.Current.LanguageCode switch
    { "ja" => 0, "zh-Hans" => 2, "ko" => 3, "es" => 4, "fr" => 5, "de" => 6, "pt" => 7, "it" => 8, "ru" => 9, _ => 1 }];
    internal static string Unknown => Pick("不明", "Unknown", "未知", "알 수 없음", "Desconocido", "Inconnu", "Unbekannt", "Desconhecido", "Sconosciuto", "Неизвестно");
    internal static string PriceUnknown => Pick("価格未確定", "Price unknown", "价格未确定", "가격 미확정", "Precio desconocido", "Prix inconnu", "Preis unbekannt", "Preço desconhecido", "Prezzo sconosciuto", "Цена неизвестна");
    internal static string OfficialQuota => Pick("利用枠（公式報告）", "Quota (official report)", "配额（官方报告）", "할당량 (공식 보고)", "Cuota (informe oficial)", "Quota (rapport officiel)", "Kontingent (offizieller Bericht)", "Cota (relatório oficial)", "Quota (rapporto ufficiale)", "Квота (официальные данные)");
    internal static string NextReset => Pick("次回リセット予定", "Next scheduled reset", "下次计划重置", "다음 재설정 예정", "Próximo reinicio previsto", "Prochaine réinitialisation prévue", "Nächste geplante Rücksetzung", "Próxima redefinição prevista", "Prossimo ripristino previsto", "Следующий плановый сброс");
    internal static string Recorded => Pick("記録分", "Recorded usage", "已记录用量", "기록된 사용량", "Uso registrado", "Usage enregistré", "Erfasste Nutzung", "Uso registrado", "Utilizzo registrato", "Записанное использование");
    internal static string Saved => Pick("保存観測", "Saved observations", "保存的观测", "저장된 관측", "Observaciones guardadas", "Observations conservées", "Gespeicherte Beobachtungen", "Observações salvas", "Osservazioni salvate", "Сохранённые наблюдения");
    internal static string LegacySaved => Pick("旧形式の保存観測", "Legacy saved observations", "旧格式保存的观测", "이전 형식의 저장 관측", "Observaciones de formato anterior", "Observations de l’ancien format", "Gespeicherte Beobachtungen im Altformat", "Observações do formato anterior", "Osservazioni nel vecchio formato", "Наблюдения в старом формате");
    internal static string ScopeHeading => Pick("記録範囲", "Recording range", "记录范围", "기록 범위", "Intervalo de registro", "Plage d’enregistrement", "Aufzeichnungsbereich", "Intervalo de registro", "Intervallo registrato", "Диапазон записи");
    internal static string PeriodUnknown => Pick("利用期間未確定", "Usage period unknown", "使用期间未确定", "사용 기간 미확정", "Periodo de uso desconocido", "Période d’usage inconnue", "Nutzungszeitraum unbekannt", "Período de uso desconhecido", "Periodo di utilizzo sconosciuto", "Период использования неизвестен");
    internal static string Partial => Pick("一部記録", "Partial recording", "部分记录", "부분 기록", "Registro parcial", "Enregistrement partiel", "Teilweise erfasst", "Registro parcial", "Registrazione parziale", "Частичная запись");
    internal static string CoverageUnknown => Pick("記録範囲未確認", "Recording coverage unknown", "记录覆盖未确认", "기록 범위 미확인", "Cobertura desconocida", "Couverture inconnue", "Erfassungsumfang unbekannt", "Cobertura desconhecida", "Copertura sconosciuta", "Полнота записи неизвестна");
    internal static string RecordedZero => Pick("記録上0", "Recorded zero", "记录值为0", "기록상 0", "Cero registrado", "Zéro enregistré", "Erfasste Null", "Zero registrado", "Zero registrato", "Записанный ноль");
    internal static string NoRecord => Pick("記録なし／不明", "No recording / unknown", "无记录／未知", "기록 없음 / 알 수 없음", "Sin registro / desconocido", "Aucun enregistrement / inconnu", "Keine Aufzeichnung / unbekannt", "Sem registro / desconhecido", "Nessuna registrazione / sconosciuto", "Нет записи / неизвестно");
    internal static string NoScope => Pick("記録範囲なし／記録なし・不明", "No recording range / no recording or unknown", "无记录范围／无记录或未知", "기록 범위 없음 / 기록 없음 또는 알 수 없음", "Sin intervalo / sin registro o desconocido", "Aucune plage / aucun enregistrement ou inconnu", "Kein Aufzeichnungsbereich / keine Aufzeichnung oder unbekannt", "Sem intervalo / sem registro ou desconhecido", "Nessun intervallo / nessuna registrazione o sconosciuto", "Нет диапазона / нет записи или неизвестно");
    internal static string ForecastUnavailable => Pick("予測不可（利用期間未確定）", "Forecast unavailable (usage period unknown)", "无法预测（使用期间未确定）", "예측 불가 (사용 기간 미확정)", "Previsión no disponible (periodo desconocido)", "Prévision indisponible (période inconnue)", "Prognose nicht verfügbar (Zeitraum unbekannt)", "Previsão indisponível (período desconhecido)", "Previsione non disponibile (periodo sconosciuto)", "Прогноз недоступен (период неизвестен)");
    internal static string SavedMonthUnavailable => Pick("保存観測は月合計に配賦できません", "Saved observations cannot be allocated to monthly totals", "保存的观测无法分配到月总计", "저장된 관측은 월 합계에 배분할 수 없습니다", "Las observaciones no se pueden distribuir al total mensual", "Les observations ne peuvent pas être réparties dans le total mensuel", "Beobachtungen können keiner Monatssumme zugeordnet werden", "Observações não podem ser distribuídas no total mensal", "Le osservazioni non possono essere assegnate al totale mensile", "Наблюдения нельзя распределить по месячным итогам");

    internal static string Reason(string reason) => reason switch
    {
        "period-identity-unknown" => PeriodUnknown,
        "source-recovery-pending" => Pick("回復実装／処理未完", "Source recovery pending", "来源恢复待处理", "원본 복구 대기", "Recuperación pendiente", "Récupération en attente", "Quellwiederherstellung ausstehend", "Recuperação pendente", "Recupero in sospeso", "Восстановление источника ожидается"),
        "source-evidence-unavailable" => Pick("原本証拠不足", "Source evidence unavailable", "原始证据不足", "원본 증거 부족", "Evidencia original no disponible", "Preuves d’origine indisponibles", "Quellnachweis nicht verfügbar", "Evidência original indisponível", "Prova originale non disponibile", "Нет исходных доказательств"),
        "duplicate-unresolved" => Pick("重複未解決", "Unresolved duplicate", "重复未解决", "중복 미해결", "Duplicado sin resolver", "Doublon non résolu", "Duplikat ungeklärt", "Duplicação não resolvida", "Duplicato irrisolto", "Дубликат не разрешён"),
        _ => Pick("保存値の資格不成立", "Invalid saved value", "保存值无效", "저장 값 유효하지 않음", "Valor guardado no válido", "Valeur conservée invalide", "Ungültiger gespeicherter Wert", "Valor salvo inválido", "Valore salvato non valido", "Некорректное сохранённое значение"),
    };

    internal static string Scope(ApiRecordedUsageScope? scope)
    {
        if (scope is null) return $"{Recorded} · {CoverageUnknown} · {PeriodUnknown}";
        var duration = scope.EndAt - scope.StartAt;
        var seconds = duration.ToString("N0", CultureInfo.CurrentCulture);
        var recent = duration == 604800
            ? Pick("直近7日", "last 7 days", "最近7天", "최근 7일", "últimos 7 días", "7 derniers jours", "letzte 7 Tage", "últimos 7 dias", "ultimi 7 giorni", "последние 7 дней")
            : Pick($"直近{seconds}秒", $"last {seconds} s", $"最近{seconds}秒", $"최근 {seconds}초", $"últimos {seconds} s", $"{seconds} dernières s", $"letzte {seconds} s", $"últimos {seconds} s", $"ultimi {seconds} s", $"последние {seconds} с");
        return $"{Recorded}（{recent}） {Range(scope.StartAt, scope.EndAt)} · {Coverage(scope.Coverage)} · {PeriodUnknown}";
    }

    internal static string Coverage(ApiRecordedCoverage coverage) => coverage switch
    { ApiRecordedCoverage.Partial => Partial, ApiRecordedCoverage.Unknown => CoverageUnknown, _ => Recorded };

    internal static string Range(long start, long end, TimeZoneInfo? zone = null) =>
        $"{TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(start), zone ?? LocalizationService.DisplayTimeZone).ToString("yyyy/MM/dd HH:mm:ss zzz", CultureInfo.CurrentCulture)} – {TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(end - 1), zone ?? LocalizationService.DisplayTimeZone).ToString("yyyy/MM/dd HH:mm:ss zzz", CultureInfo.CurrentCulture)}";
}
