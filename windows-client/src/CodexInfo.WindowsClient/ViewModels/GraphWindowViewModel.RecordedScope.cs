// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.ViewModels;

public sealed partial class GraphWindowViewModel
{
    private readonly ILoopbackRecordedScopeClient? recordedScopeClient;
    private ApiRecordedGraphBundle? recordedBundle;
    private TimeZoneInfo recordedDisplayTimeZone = LocalizationService.DisplayTimeZone;
    private bool legacyRecordedScopeFallback;
    private bool showSavedObservations;

    public string RecordingScopeHeading => Texts.PeriodSelectorHeading;
    public bool GroupScopeEntriesByDate => true;
    public string RecordingModeText => RecordedUsageText.Recorded + " / " + RecordedUsageText.Saved;
    public string EmptyGraphText => recordedBundle is { Scope: null } ? RecordedUsageText.NoScope : Texts.UnavailableDetails;
    public bool CanSwitchRecordingMode => recordedBundle?.Scope is not null && !legacyRecordedScopeFallback;
    public bool ShowSavedObservations
    {
        get => legacyRecordedScopeFallback || showSavedObservations;
        set
        {
            if (legacyRecordedScopeFallback || value == showSavedObservations) return;
            showSavedObservations = value;
            RebuildRecordedProjection();
            Notify(nameof(ShowSavedObservations));
        }
    }

    private async Task<bool> TryRefreshRecordedScopeAsync(GraphTimeRange range, string? scopeId,
        GraphTimeBounds? bounds, long? pinnedEnd, long? navigationOrigin,
        long rangeRevision, long selectionRevision, CancellationToken token)
    {
        if (recordedScopeClient is null || legacyRecordedScopeFallback) return false;
        if (disposed || token.IsCancellationRequested) return true;
        try { await resourceRefreshGate.WaitAsync(token).ConfigureAwait(false); }
        catch (OperationCanceledException) { return true; }
        try
        {
            var account = main.SelectedAccountId;
            var accountGeneration = main.AccountSelectionGeneration;
            if (account is null && main.HasAccounts) return true;
            if (range == GraphTimeRange.CalendarMonth)
            {
                var at = pinnedEnd is { } historicalEnd ? historicalEnd - 1 : getUnixTimeSeconds();
                var utc = DateTimeOffset.FromUnixTimeSeconds(at);
                var month = new DateTimeOffset(utc.Year, utc.Month, 1, 0, 0, 0, TimeSpan.Zero);
                var monthEnd = utc.Year == 9999 && utc.Month == 12 ? 253402300800 : month.AddMonths(1).ToUnixTimeSeconds();
                var monthStart = month.ToUnixTimeSeconds();
                var published = recordedBundle?.Scopes.FirstOrDefault(scope => scope.Kind == "utc-month" &&
                    scope.StartAt == monthStart && scope.EndAt == monthEnd);
                scopeId = published?.Id;
                bounds = published is null ? new(monthStart, monthEnd) : null;
            }
            var result = await recordedScopeClient.FetchRecordedGraphAsync(account, scopeId,
                bounds?.StartAt, bounds?.EndAt, token).ConfigureAwait(false);
            if (result.LegacyNotFound)
            {
                legacyRecordedScopeFallback = true;
                postToUi(() => { Notify(nameof(ShowSavedObservations)); Notify(nameof(CanSwitchRecordingMode)); });
                return false;
            }
            postToUi(() =>
            {
                if (disposed || token.IsCancellationRequested || !IsAccountIdentityCurrent(account, accountGeneration) ||
                    rangeRevision != Interlocked.Read(ref timeWindowRevision) ||
                    selectionRevision != Interlocked.Read(ref periodSelectionRevision)) return;
                timeWindowRequestPending = false;
                resetRangeCommitRevision = -1;
                Volatile.Write(ref pendingGraphRequest, null);
                if (!result.IsSuccess)
                {
                    SetLoadError(true);
                    SetLoading(false);
                    NotifyTimeRangeProperties();
                    return;
                }
                var bundle = result.Bundle!;
                var zone = LocalizationService.DisplayTimeZone;
                // Prepare every value before publishing selector, mode, metric and plot together.
                var candidateScene = bundle.Scope is null ? GraphScene.Empty(selectedMetric) :
                    GraphScene.CreateRecorded(bundle, selectedMetric, ShowSavedObservations,
                        range == GraphTimeRange.CalendarMonth, zone, HiddenRecordedFamilies());
                var directory = bundle.Scopes.Select(scope => new ApiHistoryPeriod(scope.Id, scope.StartAt,
                    scope.EndAt - 1, scope.Current, RecordedUsageText.Range(scope.StartAt, scope.EndAt, zone))).ToArray();
                var candidateSelection = directory.FirstOrDefault(period => period.Id == bundle.SelectedScope?.Id);
                applyingSplitResourceState = true;
                try
                {
                    recordedBundle = bundle;
                    recordedDisplayTimeZone = zone;
                    scene = candidateScene;
                    displayedMetric = selectedMetric;
                    selectedPeriod = candidateSelection;
                    displayedPeriod = candidateSelection;
                    resourcePublishedPair = bundle.PublishedPair;
                    Volatile.Write(ref selectedTimeRangeValue, (int)range);
                    pinnedWindowEndAt = pinnedEnd ?? long.MinValue;
                    windowNavigationOriginAt = navigationOrigin;
                    points = [];
                    periods.Clear();
                    foreach (var period in directory) periods.Add(period);
                }
                finally { applyingSplitResourceState = false; }
                SetLoadError(false);
                SetLoading(false);
                NotifyRecordedProjection();
            });
            return true;
        }
        finally { resourceRefreshGate.Release(); }
    }

    private IReadOnlySet<string> HiddenRecordedFamilies()
    {
        var hidden = new HashSet<string>(StringComparer.Ordinal);
        if (!showModels || !showSol) hidden.Add("SOL");
        if (!showModels || !showTerra) hidden.Add("TERRA");
        if (!showModels || !showLuna) hidden.Add("LUNA");
        if (!showModels || !showAstra) hidden.Add("ASTRA");
        return hidden;
    }

    private void RebuildRecordedProjection()
    {
        if (recordedBundle is null) return;
        var zone = LocalizationService.DisplayTimeZone;
        var candidateScene = recordedBundle.Scope is null ? GraphScene.Empty(selectedMetric) :
            GraphScene.CreateRecorded(recordedBundle, selectedMetric, ShowSavedObservations,
                IsMonthView, zone, HiddenRecordedFamilies());
        var directory = recordedBundle.Scopes.Select(scope => new ApiHistoryPeriod(scope.Id, scope.StartAt,
            scope.EndAt - 1, scope.Current, RecordedUsageText.Range(scope.StartAt, scope.EndAt, zone))).ToArray();
        var selection = directory.FirstOrDefault(period => period.Id == selectedPeriod?.Id);
        applyingSplitResourceState = true;
        try
        {
            scene = candidateScene;
            recordedDisplayTimeZone = zone;
            selectedPeriod = selection;
            displayedPeriod = selection;
            periods.Clear();
            foreach (var period in directory) periods.Add(period);
        }
        finally { applyingSplitResourceState = false; }
        displayedMetric = selectedMetric;
        NotifyRecordedProjection();
    }

    private void NotifyRecordedProjection()
    {
        foreach (var property in new[] { nameof(Scene), nameof(Points), nameof(Periods), nameof(SelectedPeriod),
            nameof(SelectedPeriodText), nameof(SelectedPeriodValueText), nameof(SelectedPeriodStartAt), nameof(SelectedPeriodEndAt),
            nameof(HasPeriods), nameof(HasPoints), nameof(HasNoPoints), nameof(HasPlot), nameof(HasLinePlot), nameof(HasCalendar),
            nameof(CalendarDays), nameof(CalendarWeekdays), nameof(GraphGapHintText), nameof(MetricAxisText), nameof(IsDollars),
            nameof(ShowSavedObservations), nameof(CanSwitchRecordingMode), nameof(RecordingScopeHeading), nameof(RecordingModeText),
            nameof(EmptyGraphText), nameof(GroupScopeEntriesByDate) })
            Notify(property);
        NotifyTimeRangeProperties();
    }
}
