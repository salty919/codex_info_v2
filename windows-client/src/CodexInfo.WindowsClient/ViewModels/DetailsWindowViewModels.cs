// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Globalization;
using System.Runtime.CompilerServices;
using System.Windows.Input;
using Avalonia.Threading;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.ViewModels;

public sealed class GraphPointViewModel
{
    private readonly GraphMetric metric;
    private readonly IReadOnlyList<(string Name, double Value)> modelValues;

    public GraphPointViewModel(ApiHistorySample sample, GraphMetric metric)
    {
        this.metric = metric;
        Timestamp = sample.Timestamp;
        TimestampText = TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(sample.Timestamp), LocalizationService.DisplayTimeZone)
            .ToString("g", CultureInfo.CurrentCulture);
        RemainingPercent = sample.RemainingPercent;

        var values = new List<(string Name, double Value)>();
        foreach (var model in sample.Models)
        {
            var value = metric == GraphMetric.Dollars
                ? model.Dollars ?? double.NaN
                : model.TotalTokens is ulong totalTokens
                    ? (double)totalTokens
                    : double.NaN;
            values.Add((model.Name, value));
            switch (model.Name)
            {
                case "SOL":
                    SolValue = value;
                    break;
                case "TERRA":
                    TerraValue = value;
                    break;
                case "LUNA":
                    LunaValue = value;
                    break;
                case "ASTRA":
                    AstraValue = value;
                    break;
                default:
                    break;
            }
        }

        modelValues = values;
    }

    public long Timestamp { get; }

    public string TimestampText { get; }

    public double? RemainingPercent { get; }

    public double SolValue { get; }

    public double TerraValue { get; }

    public double LunaValue { get; }

    public double AstraValue { get; }

    public IReadOnlyDictionary<string, double> ModelValues =>
        modelValues
            .GroupBy(item => item.Name, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.Last().Value, StringComparer.Ordinal);

    public string RemainingText => RemainingPercent is { } value
        ? string.Create(CultureInfo.CurrentCulture, $"{LocalizationService.Current.RemainingQuota} {value:0.#}%")
        : $"{LocalizationService.Current.RemainingQuota} —";

    public string ModelsText => string.Join(
        " / ",
        modelValues.Select(item =>
            $"{item.Name} {FormatModelValue(item.Value, dollars: metric == GraphMetric.Dollars)}"));

    private static string FormatModelValue(double value, bool dollars) =>
        double.IsFinite(value)
            ? dollars
                ? string.Create(CultureInfo.CurrentCulture, $"${value:N2}")
                : value.ToString("N0", CultureInfo.CurrentCulture)
            : "—";
}

public sealed class GraphWindowViewModel : INotifyPropertyChanged, IDisposable
{
    // Bound the legacy diagnostic point view-model collection. GraphScene and
    // the rendered evidence retain every admitted minute: reducing before
    // semantic projection would turn ordinary 60-second idle observations
    // into periodic gaps on long histories.
    internal const int MaxRenderedGraphPoints = 2_048;
    // This is a DoS guard derived from the existing one-month history admission
    // envelope in Core. It is not a normal page-size or payload requirement.
    private const int MaxSplitHistoryPageRequests = 31 * 24 * 60 + 1;
    private const int MaxSplitGenerationAlignmentAttempts = 2;
    private const int BackgroundBuildThreshold = 2_048;
    private readonly MainWindowViewModel main;
    private readonly Action<Action> postToUi;
    private readonly ILoopbackResourceClient? resourceClient;
    private readonly ILoopbackAccountResourceClient? accountResourceClient;
    private readonly SemaphoreSlim resourceRefreshGate = new(1, 1);
    private readonly ObservableCollection<ApiHistoryPeriod> periods = [];
    private IReadOnlyList<GraphPointViewModel> points = Array.Empty<GraphPointViewModel>();
    private GraphScene scene = GraphScene.Empty();
    private ApiHistoryPeriod? selectedPeriod;
    private ApiHistoryPeriod? displayedPeriod;
    private GraphMetric selectedMetric = GraphMetric.Dollars;
    private GraphMetric displayedMetric = GraphMetric.Dollars;
    private IReadOnlyList<string> metricOptions = Array.Empty<string>();
    private bool showRemaining = true;
    private bool showModels = true;
    private bool showSol = true;
    private bool showTerra = true;
    private bool showLuna = true;
    private bool showAstra = true;
    private CancellationTokenSource pointBuildCancellation = new();
    private long pointBuildRevision;
    private long periodSelectionRevision;
    private bool isLoading;
    private bool hasLoadError;
    private bool disposed;
    private bool applyingSplitResourceState;
    private bool resourceCursorResetRequired;
    private CancellationTokenSource? resourcePollingCancellation;
    private string? resourceNextCursor;
    private PublishedPairIdentity? resourcePublishedPair;
    private ApiHistoryPeriod? resourcePeriod;
    private IReadOnlyList<ApiHistorySample> resourceSamples = Array.Empty<ApiHistorySample>();
    private IReadOnlyList<ApiHistoryGap> resourceGaps = Array.Empty<ApiHistoryGap>();

    public GraphWindowViewModel(MainWindowViewModel main)
        : this(main, action => Dispatcher.UIThread.Post(action))
    {
    }

    internal GraphWindowViewModel(MainWindowViewModel main, Action<Action> postToUi)
    {
        this.main = main;
        this.postToUi = postToUi;
        Periods = new ReadOnlyObservableCollection<ApiHistoryPeriod>(periods);
        RebuildMetricOptions();
        main.PropertyChanged += OnMainPropertyChanged;
        resourceClient = main.SplitResourceClient;
        accountResourceClient = main.AccountResourceClient;
        if (resourceClient is null)
        {
            Rebuild();
        }
        else
        {
            SetLoading(true);
            resourcePollingCancellation = new CancellationTokenSource();
            _ = RunSplitResourcePollingAsync(resourcePollingCancellation.Token);
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ApiHistoryPeriod> Periods { get; }

    public IReadOnlyList<GraphPointViewModel> Points => points;

    public GraphScene Scene => scene;

    public UiText Texts => LocalizationService.Current;

    public ReadOnlyObservableCollection<ApiAccount> Accounts => main.Accounts;

    public bool HasAccounts => main.HasAccounts;

    public ApiAccount? SelectedAccount
    {
        get => main.SelectedAccount;
        set
        {
            if (value is not null)
            {
                main.SelectAccount(value.Id);
            }
        }
    }

    public string SelectedAccountText => $"{Texts.Account}｜{main.SelectedAccountText}";

    public IReadOnlyList<string> MetricOptions => metricOptions;

    public string SelectedMetric
    {
        get => selectedMetric == GraphMetric.Dollars ? Texts.GraphDollarMetric : Texts.GraphTokenMetric;
        set
        {
            var metric = value == Texts.GraphTokenMetric ? GraphMetric.Tokens : GraphMetric.Dollars;
            if (selectedMetric == metric)
            {
                return;
            }

            selectedMetric = metric;
            RebuildPoints();
            Notify();
        }
    }

    public ApiHistoryPeriod? SelectedPeriod
    {
        get => selectedPeriod;
        set
        {
            if (ReferenceEquals(selectedPeriod, value))
            {
                return;
            }

            var requiresResourceFetch = resourceClient is not null && value is not null &&
                !disposed && !applyingSplitResourceState;
            if (requiresResourceFetch)
            {
                var selectionRevision = Interlocked.Increment(ref periodSelectionRevision);
                SetLoadError(false);
                SetLoading(true);
                // Keep the accepted selector, scene, and axis as one visible
                // generation until the requested period has been fetched and
                // validated completely. Re-notify the accepted value so the
                // ComboBox cannot label the old scene as the requested period.
                Notify();
                _ = RefreshSplitResourceCoreAsync(
                    initial: true,
                    requestedPeriodId: value!.Id,
                    selectionRevision,
                    resourcePollingCancellation?.Token ?? main.LifetimeToken);
                return;
            }

            selectedPeriod = value;
            RebuildPoints();
            Notify();
            Notify(nameof(HasPoints));
            Notify(nameof(SelectedPeriodText));
            Notify(nameof(SelectedPeriodStartAt));
            Notify(nameof(SelectedPeriodEndAt));
        }
    }

    public bool HasPoints => points.Count > 0;

    public bool HasNoPoints => !IsLoading && !HasLoadError && !HasPoints;

    public bool HasBlockingLoadError => !IsLoading && HasLoadError && !HasPoints;

    public bool HasPeriods => periods.Count > 0;

    public string SelectedPeriodText => selectedPeriod is { } period
        ? $"{Texts.PeriodSelectorHeading}｜{period.Label}"
        : $"{Texts.PeriodSelectorHeading}｜{Texts.UnavailableValue}";

    public long SelectedPeriodStartAt => scene.HasPoints ? scene.PeriodStartAt : displayedPeriod?.StartAt ?? 0;

    public long SelectedPeriodEndAt => scene.HasPoints ? scene.PeriodEndAt : 0;

    // The accepted periods resource owns the graph boundary. Local clock skew
    // must not make Windows project a different X range than the X client.
    internal static long EffectiveGraphEnd(ApiHistoryPeriod period, long now)
    {
        _ = now;
        return period.EndAt;
    }

    internal static IReadOnlyList<ApiHistorySample> BuildGraphSamples(ApiHistoryPeriod period, long now)
    {
        var end = EffectiveGraphEnd(period, now);
        var observed = period.Samples
            // Both current and historical periods own their exact published
            // end. A 60-second cadence may start at any Unix-time phase, so
            // validity is bounded by the accepted period rather than modulo.
            .Where(sample => sample.Timestamp >= period.StartAt &&
                             sample.Timestamp <= end)
            .OrderBy(sample => sample.Timestamp)
            .ToList();
        if (observed.Count == 0)
        {
            return [];
        }

        // Core admission supplies strictly increasing minute-start rows with
        // one canonical owner for each period/timestamp. Preserve every
        // source vector as-is; graph code must not invent a pre-observation
        // baseline or repair model components from older rows.
        var normalized = observed.ToList();

        var result = new List<ApiHistorySample>(normalized.Count + 1);
        result.AddRange(normalized);
        var last = result[^1];
        if (last.Timestamp < end)
        {
            // Match the native graph's explicit selected-period endpoint.
            // This is presentation evidence, never a measured sample: model
            // and quota projectors therefore render the full hold as inferred
            // regardless of its duration or the last row's source quality.
            result.Add(last with
            {
                Timestamp = end,
                RemainingPercent = null,
                IsSyntheticTail = true,
            });
        }

        return result;
    }

    internal static IReadOnlyList<ApiHistorySample> ReduceGraphSamples(
        IReadOnlyList<ApiHistorySample> samples,
        int maximum = MaxRenderedGraphPoints,
        IReadOnlyList<GraphConfirmedGap>? confirmedGaps = null)
    {
        if (maximum < 2)
        {
            throw new ArgumentOutOfRangeException(nameof(maximum));
        }
        if (samples.Count <= maximum)
        {
            return samples;
        }

        var mandatory = new SortedSet<int> { 0, samples.Count - 1 };
        CompleteModelVector? lastReliableConfirmedVector = null;
        var inUnreliableInterval = false;
        var hasPreviousCompleteVector = false;
        var previousVector = default(CompleteModelVector);
        for (var index = 0; index < samples.Count; index++)
        {
            var current = samples[index];
            var currentHasCompleteVector = TryGetCompleteModelVector(current, out var currentVector);
            var currentIsConfirmed = current.ModelSource == ApiHistorySample.ConfirmedModelSource;
            if (index == 0)
            {
                if (currentIsConfirmed && currentHasCompleteVector)
                {
                    lastReliableConfirmedVector = currentVector;
                    inUnreliableInterval = false;
                }
                else
                {
                    inUnreliableInterval = true;
                }

                hasPreviousCompleteVector = currentHasCompleteVector;
                previousVector = currentVector;
                continue;
            }

            var previous = samples[index - 1];
            var sourceTransition = previous.ModelSource != current.ModelSource;
            var quotaTransition = (previous.RemainingPercent is null) != (current.RemainingPercent is null);
            var timestampGap = current.Timestamp - previous.Timestamp > 60;
            var modelRegression = hasPreviousCompleteVector && currentHasCompleteVector &&
                currentVector.IsLowerThan(previousVector);
            var modelRecovery = inUnreliableInterval && currentIsConfirmed && currentHasCompleteVector &&
                (lastReliableConfirmedVector is null || currentVector.IsAtLeast(lastReliableConfirmedVector.Value));
            var quotaDrop = current.RemainingPercent is { } currentRemaining &&
                previous.RemainingPercent is { } previousRemaining &&
                currentRemaining < previousRemaining &&
                !IsAttributableModelIncrement(previous, current);
            if (sourceTransition || quotaTransition || timestampGap || modelRegression || modelRecovery || quotaDrop)
            {
                mandatory.Add(index - 1);
                mandatory.Add(index);
            }

            if (modelRecovery)
            {
                lastReliableConfirmedVector = currentVector;
                inUnreliableInterval = false;
            }
            else if (currentIsConfirmed && currentHasCompleteVector &&
                     !inUnreliableInterval && !modelRegression)
            {
                lastReliableConfirmedVector = currentVector;
            }
            else if (!currentIsConfirmed || !currentHasCompleteVector || modelRegression)
            {
                inUnreliableInterval = true;
            }

            hasPreviousCompleteVector = currentHasCompleteVector;
            previousVector = currentVector;
        }

        if (confirmedGaps is not null)
        {
            foreach (var gap in confirmedGaps)
            {
                AddNearestBoundary(mandatory, samples, gap.StartAt, preferPrevious: true);
                AddNearestBoundary(mandatory, samples, gap.EndAt, preferPrevious: false);
            }
        }

        // All graph series are cumulative and therefore monotonic. Keep both
        // edges of each display bucket so a short change is not lost or moved
        // to a later bucket, while bounding paint work by viewport resolution.
        var selected = new SortedSet<int>(mandatory);
        if (mandatory.Count > maximum)
        {
            // The viewport maximum is a soft cap when correctness-critical
            // boundaries outnumber it.  The details endpoint bounds the
            // source history at 44,640 rows, so preserving every mandatory
            // boundary remains finite without silently creating a bridge.
            return mandatory.Select(index => samples[index]).ToArray();
        }

        var bucketCount = Math.Max(1, maximum / 2);
        for (var bucket = 0; bucket < bucketCount; bucket++)
        {
            var start = (int)((long)bucket * samples.Count / bucketCount);
            var endExclusive = (int)((long)(bucket + 1) * samples.Count / bucketCount);
            if (selected.Count < maximum)
            {
                selected.Add(start);
            }
            if (selected.Count < maximum)
            {
                selected.Add(Math.Max(start, endExclusive - 1));
            }
        }
        // Odd/small caller-supplied maxima can leave one slot. Fill it with a
        // uniformly located sample without disturbing the bucket edges.
        for (var slot = 1; selected.Count < maximum && slot < maximum - 1; slot++)
        {
            selected.Add((int)Math.Round(
                slot * (samples.Count - 1d) / (maximum - 1d),
                MidpointRounding.AwayFromZero));
        }
        if (!selected.Contains(samples.Count - 1))
        {
            selected.Remove(selected.Max);
            selected.Add(samples.Count - 1);
        }
        return selected.Take(maximum).Select(index => samples[index]).ToArray();
    }

    private static bool TryGetCompleteModelVector(
        ApiHistorySample sample,
        out CompleteModelVector vector)
    {
        if (sample.SolDollars is not { } solDollars || !double.IsFinite(solDollars) ||
            sample.TerraDollars is not { } terraDollars || !double.IsFinite(terraDollars) ||
            sample.LunaDollars is not { } lunaDollars || !double.IsFinite(lunaDollars) ||
            sample.SolTokens is not { } solTokens ||
            sample.TerraTokens is not { } terraTokens ||
            sample.LunaTokens is not { } lunaTokens)
        {
            vector = default;
            return false;
        }

        vector = new CompleteModelVector(
            solDollars,
            terraDollars,
            lunaDollars,
            solTokens,
            terraTokens,
            lunaTokens);
        return true;
    }

    private static bool HasCompleteModelVector(ApiHistorySample sample) =>
        TryGetCompleteModelVector(sample, out _);

    private readonly record struct CompleteModelVector(
        double SolDollars,
        double TerraDollars,
        double LunaDollars,
        ulong SolTokens,
        ulong TerraTokens,
        ulong LunaTokens)
    {
        public bool IsLowerThan(CompleteModelVector other) =>
            SolDollars < other.SolDollars ||
            TerraDollars < other.TerraDollars ||
            LunaDollars < other.LunaDollars ||
            SolTokens < other.SolTokens ||
            TerraTokens < other.TerraTokens ||
            LunaTokens < other.LunaTokens;

        public bool IsAtLeast(CompleteModelVector other) =>
            SolDollars >= other.SolDollars &&
            TerraDollars >= other.TerraDollars &&
            LunaDollars >= other.LunaDollars &&
            SolTokens >= other.SolTokens &&
            TerraTokens >= other.TerraTokens &&
            LunaTokens >= other.LunaTokens;
    }

    private static bool IsAttributableModelIncrement(
        ApiHistorySample previous,
        ApiHistorySample current) =>
        current.ModelSource == ApiHistorySample.ConfirmedModelSource &&
        previous.ModelSource == ApiHistorySample.ConfirmedModelSource &&
        HasCompleteModelVector(previous) &&
        HasCompleteModelVector(current) &&
        current.SolDollars >= previous.SolDollars &&
        current.TerraDollars >= previous.TerraDollars &&
        current.LunaDollars >= previous.LunaDollars &&
        current.SolTokens >= previous.SolTokens &&
        current.TerraTokens >= previous.TerraTokens &&
        current.LunaTokens >= previous.LunaTokens &&
        (current.SolDollars > previous.SolDollars ||
         current.TerraDollars > previous.TerraDollars ||
         current.LunaDollars > previous.LunaDollars ||
         current.SolTokens > previous.SolTokens ||
         current.TerraTokens > previous.TerraTokens ||
         current.LunaTokens > previous.LunaTokens);

    private static void AddNearestBoundary(
        SortedSet<int> selected,
        IReadOnlyList<ApiHistorySample> samples,
        long boundary,
        bool preferPrevious)
    {
        if (samples.Count == 0)
        {
            return;
        }

        var index = preferPrevious
            ? FindLastAtOrBefore(samples, boundary)
            : FindFirstAtOrAfter(samples, boundary);
        selected.Add(index);
    }

    private static int FindLastAtOrBefore(IReadOnlyList<ApiHistorySample> samples, long boundary)
    {
        var low = 0;
        var high = samples.Count - 1;
        var result = 0;
        while (low <= high)
        {
            var middle = low + (high - low) / 2;
            if (samples[middle].Timestamp <= boundary)
            {
                result = middle;
                low = middle + 1;
            }
            else
            {
                high = middle - 1;
            }
        }

        return result;
    }

    private static int FindFirstAtOrAfter(IReadOnlyList<ApiHistorySample> samples, long boundary)
    {
        var low = 0;
        var high = samples.Count - 1;
        var result = samples.Count - 1;
        while (low <= high)
        {
            var middle = low + (high - low) / 2;
            if (samples[middle].Timestamp >= boundary)
            {
                result = middle;
                high = middle - 1;
            }
            else
            {
                low = middle + 1;
            }
        }

        return result;
    }

    public string DetailsStatusText => main.DetailsStatusText;

    public bool IsLoading => isLoading;

    public bool HasLoadError => hasLoadError;

    public string LoadingText => Texts.GraphLoading;

    public string LoadErrorText => Texts.GraphLoadFailed;

    public string MetricAxisText => displayedMetric == GraphMetric.Dollars
        ? Texts.GraphDollarDescription
        : Texts.GraphTokenDescription;

    public string GraphGapHintText => Texts.LanguageCode switch
    {
        "ja" => "破線: 欠損・取得元不一致区間の参考補完（区間内は未確認）",
        "zh-Hans" => "虚线：缺失或来源不一致区间的参考补全（区间内未确认）",
        "ko" => "점선: 누락·출처 불일치 구간의 참고 보완(구간 내부 미확인)",
        _ => "Dashed: reference completion across missing or source-mismatched intervals",
    };

    public bool IsDollars => displayedMetric == GraphMetric.Dollars;

    public bool ShowRemaining
    {
        get => showRemaining;
        set
        {
            if (showRemaining == value) return;
            showRemaining = value;
            Notify();
        }
    }

    public bool ShowModels
    {
        get => showModels;
        set
        {
            if (showModels == value) return;
            showModels = value;
            RebuildPoints();
            Notify();
        }
    }

    public bool ShowSol
    {
        get => showSol;
        set
        {
            if (showSol == value) return;
            showSol = value;
            RebuildPoints();
            Notify();
        }
    }

    public bool ShowTerra
    {
        get => showTerra;
        set
        {
            if (showTerra == value) return;
            showTerra = value;
            RebuildPoints();
            Notify();
        }
    }

    public bool ShowLuna
    {
        get => showLuna;
        set
        {
            if (showLuna == value) return;
            showLuna = value;
            RebuildPoints();
            Notify();
        }
    }

    public bool ShowAstra
    {
        get => showAstra;
        set
        {
            if (showAstra == value) return;
            showAstra = value;
            RebuildPoints();
            Notify();
        }
    }

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }

        disposed = true;
        pointBuildCancellation.Cancel();
        pointBuildCancellation.Dispose();
        if (resourcePollingCancellation is not null)
        {
            resourcePollingCancellation.Cancel();
            resourcePollingCancellation.Dispose();
        }
        main.PropertyChanged -= OnMainPropertyChanged;
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName is nameof(MainWindowViewModel.Accounts) or
            nameof(MainWindowViewModel.HasAccounts) or
            nameof(MainWindowViewModel.SelectedAccount) or
            nameof(MainWindowViewModel.SelectedAccountText))
        {
            Notify(nameof(Accounts));
            Notify(nameof(HasAccounts));
            Notify(nameof(SelectedAccount));
            Notify(nameof(SelectedAccountText));
            if (eventArgs.PropertyName == nameof(MainWindowViewModel.SelectedAccount))
            {
                ClearAccountResourceState();
                if (resourceClient is null)
                {
                    Rebuild();
                }
                else if (!disposed)
                {
                    _ = RefreshSplitResourceAsync(
                        initial: true,
                        requestedPeriodId: null,
                        resourcePollingCancellation?.Token ?? main.LifetimeToken);
                }
            }
            return;
        }

        if (eventArgs.PropertyName == nameof(MainWindowViewModel.DetailsSnapshot))
        {
            if (resourceClient is null)
            {
                Rebuild();
            }
            return;
        }

        if (eventArgs.PropertyName == nameof(MainWindowViewModel.DetailsStatusText))
        {
            Notify(nameof(DetailsStatusText));
            return;
        }

        if (eventArgs.PropertyName == nameof(MainWindowViewModel.Texts))
        {
            RebuildMetricOptions();
            ReformatPeriodLabels();
            RebuildPoints();
            Notify(nameof(Texts));
            Notify(nameof(MetricOptions));
            Notify(nameof(SelectedMetric));
            Notify(nameof(SelectedPeriodText));
            Notify(nameof(MetricAxisText));
            Notify(nameof(GraphGapHintText));
        }
    }

    private void ClearAccountResourceState()
    {
        if (disposed)
        {
            return;
        }

        Interlocked.Increment(ref periodSelectionRevision);
        pointBuildCancellation.Cancel();
        pointBuildCancellation.Dispose();
        pointBuildCancellation = new CancellationTokenSource();
        pointBuildRevision++;
        periods.Clear();
        selectedPeriod = null;
        displayedPeriod = null;
        resourceCursorResetRequired = false;
        resourceNextCursor = null;
        resourcePublishedPair = null;
        resourcePeriod = null;
        resourceSamples = Array.Empty<ApiHistorySample>();
        resourceGaps = Array.Empty<ApiHistoryGap>();
        points = Array.Empty<GraphPointViewModel>();
        scene = GraphScene.Empty(selectedMetric);
        SetLoadError(false);
        SetLoading(main.HasAccounts);
        Notify(nameof(Periods));
        Notify(nameof(SelectedPeriod));
        Notify(nameof(SelectedPeriodText));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
        Notify(nameof(Points));
        Notify(nameof(Scene));
        Notify(nameof(HasPoints));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
    }

    private void RebuildMetricOptions()
    {
        metricOptions = [Texts.GraphDollarMetric, Texts.GraphTokenMetric];
    }

    private async Task RunSplitResourcePollingAsync(CancellationToken cancellationToken)
    {
        try
        {
            await RefreshSplitResourceAsync(
                    initial: true,
                    requestedPeriodId: null,
                    cancellationToken)
                .ConfigureAwait(false);

            using var timer = new PeriodicTimer(TimeSpan.FromSeconds(60));
            while (await timer.WaitForNextTickAsync(cancellationToken).ConfigureAwait(false))
            {
                await RefreshSplitResourceAsync(
                        initial: false,
                        requestedPeriodId: selectedPeriod?.Id,
                        cancellationToken)
                    .ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Closing the graph owns cancellation.
        }
    }

    private async Task RefreshSplitResourceAsync(
        bool initial,
        string? requestedPeriodId,
        CancellationToken cancellationToken) =>
        await RefreshSplitResourceCoreAsync(
            initial,
            requestedPeriodId,
            Interlocked.Read(ref periodSelectionRevision),
            cancellationToken).ConfigureAwait(false);

    private async Task RefreshSplitResourceCoreAsync(
        bool initial,
        string? requestedPeriodId,
        long selectionRevision,
        CancellationToken cancellationToken)
    {
        if (resourceClient is null || disposed || cancellationToken.IsCancellationRequested)
        {
            return;
        }

        try
        {
            await resourceRefreshGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            return;
        }

        string? operationAccountId = null;
        long operationAccountGeneration = 0;
        bool operationCursorResetRequired = false;
        try
        {
            var accountId = main.SelectedAccountId;
            var accountGeneration = main.AccountSelectionGeneration;
            operationAccountId = accountId;
            operationAccountGeneration = accountGeneration;
            if (accountResourceClient is not null && accountId is null)
            {
                return;
            }

            if (accountResourceClient is null && main.HasAccounts)
            {
                PublishSplitResourceFailure(accountId, accountGeneration, selectionRevision);
                return;
            }

            var candidateResourcePeriod = resourcePeriod;
            var candidateResourceNextCursor = resourceNextCursor;
            var candidateCursorResetRequired = resourceCursorResetRequired;
            operationCursorResetRequired = candidateCursorResetRequired;
            var candidateResourceSamples = resourceSamples;
            var candidateResourceGaps = resourceGaps;
            var periodsResult = accountId is not null && accountResourceClient is not null
                ? await accountResourceClient.FetchHistoryPeriodsAsync(accountId, cancellationToken)
                : await resourceClient.FetchHistoryPeriodsAsync(cancellationToken)
                .ConfigureAwait(false);
            if (!periodsResult.IsSuccess || periodsResult.Snapshot is not { } periodsSnapshot)
            {
                PublishSplitResourceFailure(
                    accountId,
                    accountGeneration,
                    selectionRevision,
                    candidateCursorResetRequired);
                return;
            }

            if (accountId is not null &&
                (periodsSnapshot.AccountId != accountId ||
                 !main.IsAccountSelectionCurrent(accountId, accountGeneration)))
            {
                return;
            }

            var stagedPeriod = periodsSnapshot.Periods.FirstOrDefault(period =>
                period.Id == requestedPeriodId);
            stagedPeriod ??= periodsSnapshot.Periods.FirstOrDefault(period => period.Current)
                ?? periodsSnapshot.Periods.FirstOrDefault();
            if (stagedPeriod is null)
            {
                PublishSplitResourceState(
                    periodsSnapshot.Periods,
                    null,
                    Array.Empty<ApiHistorySample>(),
                    Array.Empty<ApiHistoryGap>(),
                    periodsSnapshot.PublishedPair,
                    nextCursor: null,
                    accountId,
                    accountGeneration,
                    selectionRevision,
                    nextCursorResetRequired: false);
                return;
            }

            var periodChanged = candidateResourcePeriod?.Id != stagedPeriod.Id;
            var canContinueFromPreviousCursor = !candidateCursorResetRequired &&
                !periodChanged &&
                candidateResourceNextCursor is not null;
            var fullRefresh = initial || periodChanged ||
                candidateCursorResetRequired ||
                !canContinueFromPreviousCursor;
            var cursor = fullRefresh ? null : candidateResourceNextCursor;
            var appendingProvenPrefix = canContinueFromPreviousCursor;
            var pageSamples = new List<ApiHistorySample>();
            var pageGaps = new List<ApiHistoryGap>();
            var requestedCursor = cursor;
            var pageCount = 0;
            var totalPageRequests = 0;
            var staleCursorRecoveryAttempted = false;
            var generationAlignmentAttempts = 1;
            string? resumeCursor = null;
            while (true)
            {
                if (++totalPageRequests > MaxSplitHistoryPageRequests)
                {
                    PublishSplitResourceFailure(
                        accountId,
                        accountGeneration,
                        selectionRevision,
                        candidateCursorResetRequired);
                    return;
                }

                pageCount++;
                var firstPage = pageCount == 1;
                var pageResult = accountId is not null && accountResourceClient is not null
                    ? await accountResourceClient.FetchHistoryPageAsync(
                            accountId,
                            stagedPeriod.Id,
                            cursor,
                            cancellationToken)
                    : await resourceClient.FetchHistoryPageAsync(
                            stagedPeriod.Id,
                            cursor,
                            cancellationToken)
                    .ConfigureAwait(false);
                if (accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
                {
                    return;
                }
                if (pageResult.CursorRejected &&
                    firstPage &&
                    appendingProvenPrefix &&
                    cursor is not null &&
                    !staleCursorRecoveryAttempted)
                {
                    // The server proved that the saved prefix can no longer
                    // be appended. Restart this candidate once from the head;
                    // the published scene and saved cursor remain untouched
                    // until the replacement page set is complete.
                    staleCursorRecoveryAttempted = true;
                    fullRefresh = true;
                    appendingProvenPrefix = false;
                    cursor = null;
                    requestedCursor = null;
                    pageCount = 0;
                    pageSamples.Clear();
                    pageGaps.Clear();
                    resumeCursor = null;
                    continue;
                }
                if (!pageResult.IsSuccess || pageResult.Page is not { } page ||
                    (accountId is not null && page.AccountId != accountId) ||
                    page.PeriodId != stagedPeriod.Id)
                {
                    // Only the exact stale-cursor response changes reset
                    // state. Ordinary failures retry the same state next
                    // cycle and never publish a partial candidate.
                    candidateCursorResetRequired = staleCursorRecoveryAttempted ||
                        candidateCursorResetRequired;
                    operationCursorResetRequired = candidateCursorResetRequired;
                    PublishSplitResourceFailure(
                        accountId,
                        accountGeneration,
                        selectionRevision,
                        candidateCursorResetRequired);
                    return;
                }

                if (page.PublishedPair != periodsSnapshot.PublishedPair)
                {
                    if (generationAlignmentAttempts >= MaxSplitGenerationAlignmentAttempts)
                    {
                        candidateCursorResetRequired = staleCursorRecoveryAttempted ||
                            candidateCursorResetRequired;
                        operationCursorResetRequired = candidateCursorResetRequired;
                        PublishSplitResourceFailure(
                            accountId,
                            accountGeneration,
                            selectionRevision,
                            candidateCursorResetRequired);
                        return;
                    }

                    generationAlignmentAttempts++;
                    var alignedPeriodsResult = accountId is not null && accountResourceClient is not null
                        ? await accountResourceClient.FetchHistoryPeriodsAsync(accountId, cancellationToken)
                        : await resourceClient.FetchHistoryPeriodsAsync(cancellationToken)
                        .ConfigureAwait(false);
                    if (!alignedPeriodsResult.IsSuccess ||
                        alignedPeriodsResult.Snapshot is not { } alignedPeriodsSnapshot ||
                        (accountId is not null &&
                            (alignedPeriodsSnapshot.AccountId != accountId ||
                             !main.IsAccountSelectionCurrent(accountId, accountGeneration))))
                    {
                        candidateCursorResetRequired = staleCursorRecoveryAttempted ||
                            candidateCursorResetRequired;
                        operationCursorResetRequired = candidateCursorResetRequired;
                        PublishSplitResourceFailure(
                            accountId,
                            accountGeneration,
                            selectionRevision,
                            candidateCursorResetRequired);
                        return;
                    }

                    var alignedPeriod = alignedPeriodsSnapshot.Periods.FirstOrDefault(period =>
                        period.Id == requestedPeriodId);
                    alignedPeriod ??= alignedPeriodsSnapshot.Periods.FirstOrDefault(period => period.Current)
                        ?? alignedPeriodsSnapshot.Periods.FirstOrDefault();
                    if (alignedPeriod is null)
                    {
                        PublishSplitResourceState(
                            alignedPeriodsSnapshot.Periods,
                            null,
                            Array.Empty<ApiHistorySample>(),
                            Array.Empty<ApiHistoryGap>(),
                            alignedPeriodsSnapshot.PublishedPair,
                            nextCursor: null,
                            accountId,
                            accountGeneration,
                            selectionRevision,
                            nextCursorResetRequired: false);
                        return;
                    }

                    // The global REST root advanced between periods and page.
                    // Keep the published scene untouched and rebuild one complete
                    // candidate from the new root instead of flashing an error.
                    periodsSnapshot = alignedPeriodsSnapshot;
                    stagedPeriod = alignedPeriod;
                    fullRefresh = true;
                    appendingProvenPrefix = false;
                    cursor = null;
                    requestedCursor = null;
                    pageCount = 0;
                    pageSamples.Clear();
                    pageGaps.Clear();
                    resumeCursor = null;
                    continue;
                }

                if (!ValidateHistoryPage(stagedPeriod, page))
                {
                    PublishSplitResourceFailure(
                        accountId,
                        accountGeneration,
                        selectionRevision,
                        candidateCursorResetRequired);
                    return;
                }

                // A cursor continuation proves that the selected period's
                // complete gap set is unchanged; delta pages therefore carry
                // samples only. A head request publishes the complete gap set
                // once, while later pages must not repeat or alter it.
                if ((appendingProvenPrefix || !firstPage) && page.HistoryGaps.Count != 0)
                {
                    PublishSplitResourceFailure(
                        accountId,
                        accountGeneration,
                        selectionRevision,
                        candidateCursorResetRequired);
                    return;
                }

                pageSamples.AddRange(page.Samples);
                if (firstPage)
                {
                    pageGaps.AddRange(page.HistoryGaps);
                }
                if (page.NextCursor is null)
                {
                    resumeCursor = page.IsLegacyFallback ? null : page.ResumeCursor;
                    cursor = resumeCursor;
                    break;
                }

                if (page.NextCursor == cursor ||
                    page.NextCursor == requestedCursor && pageSamples.Count == 0)
                {
                    PublishSplitResourceFailure(
                        accountId,
                        accountGeneration,
                        selectionRevision,
                        candidateCursorResetRequired);
                    return;
                }

                requestedCursor = cursor = page.NextCursor;
            }

            var mergedSamples = fullRefresh
                ? MergeHistorySamples(Array.Empty<ApiHistorySample>(), pageSamples)
                : MergeHistorySamples(candidateResourceSamples, pageSamples);
            var mergedGaps = fullRefresh
                ? MergeHistoryGaps(Array.Empty<ApiHistoryGap>(), pageGaps)
                : candidateResourceGaps;
            if (mergedSamples is null || mergedGaps is null)
            {
                PublishSplitResourceFailure(
                    accountId,
                    accountGeneration,
                    selectionRevision,
                    candidateCursorResetRequired);
                return;
            }

            if (accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return;
            }

            PublishSplitResourceState(
                periodsSnapshot.Periods,
                stagedPeriod,
                mergedSamples,
                mergedGaps,
                periodsSnapshot.PublishedPair,
                cursor,
                accountId,
                accountGeneration,
                selectionRevision,
                nextCursorResetRequired: false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Closing the graph owns cancellation.
        }
        catch
        {
            PublishSplitResourceFailure(
                operationAccountId,
                operationAccountGeneration,
                selectionRevision,
                operationCursorResetRequired);
        }
        finally
        {
            resourceRefreshGate.Release();
        }
    }

    private void PublishSplitResourceState(
        IReadOnlyList<ApiHistoryPeriod> nextPeriods,
        ApiHistoryPeriod? nextSelectedPeriod,
        IReadOnlyList<ApiHistorySample> nextSamples,
        IReadOnlyList<ApiHistoryGap> nextGaps,
        PublishedPairIdentity nextPair,
        string? nextCursor,
        string? accountId,
        long accountGeneration,
        long selectionRevision,
        bool nextCursorResetRequired)
    {
        postToUi(() =>
        {
            if (disposed ||
                selectionRevision != Interlocked.Read(ref periodSelectionRevision) ||
                accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return;
            }

            applyingSplitResourceState = true;
            try
            {
                ApiHistoryPeriod? publishedSelectedPeriod = null;
                periods.Clear();
                foreach (var displayPeriod in FormatPeriodsForDisplay(nextPeriods))
                {
                    var publishedPeriod = displayPeriod.Id == nextSelectedPeriod?.Id
                        ? displayPeriod with { Samples = nextSamples }
                        : displayPeriod;
                    periods.Add(publishedPeriod);
                    if (publishedPeriod.Id == nextSelectedPeriod?.Id)
                    {
                        publishedSelectedPeriod = publishedPeriod;
                    }
                }

                selectedPeriod = publishedSelectedPeriod;
                resourcePeriod = selectedPeriod;
                resourceSamples = nextSamples;
                resourceGaps = nextGaps;
                resourcePublishedPair = nextPair;
                resourceNextCursor = nextCursor;
                resourceCursorResetRequired = nextCursorResetRequired;
                SetLoadError(false);
                RebuildPoints();
                Notify(nameof(HasPeriods));
                Notify(nameof(SelectedPeriod));
                Notify(nameof(SelectedPeriodText));
                Notify(nameof(SelectedPeriodStartAt));
                Notify(nameof(SelectedPeriodEndAt));
            }
            finally
            {
                applyingSplitResourceState = false;
            }
        });
    }

    private void PublishSplitResourceFailure(
        string? accountId,
        long accountGeneration,
        long selectionRevision,
        bool nextCursorResetRequired = false)
    {
        if (disposed)
        {
            return;
        }

        postToUi(() =>
        {
            if (disposed ||
                selectionRevision != Interlocked.Read(ref periodSelectionRevision) ||
                accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return;
            }

            resourceCursorResetRequired = nextCursorResetRequired;
            SetLoadError(true);
            SetLoading(false);
        });
    }

    private static bool ValidateHistoryPage(ApiHistoryPeriod period, ApiHistoryPage page)
    {
        foreach (var sample in page.Samples)
        {
            if (sample.Timestamp < period.StartAt ||
                sample.Timestamp > period.EndAt ||
                sample.ResetAt < period.ResetAt - 60 ||
                sample.ResetAt > period.ResetAt)
            {
                return false;
            }
        }

        foreach (var gap in page.HistoryGaps)
        {
            if (gap.ResetAt != period.ResetAt ||
                gap.StartAt < period.StartAt ||
                gap.EndAt > period.EndAt)
            {
                return false;
            }
        }

        return true;
    }

    private static IReadOnlyList<ApiHistorySample>? MergeHistorySamples(
        IReadOnlyList<ApiHistorySample> existing,
        IReadOnlyList<ApiHistorySample> additions)
    {
        var merged = new Dictionary<(long ResetAt, long Timestamp), ApiHistorySample>();
        foreach (var sample in existing.Concat(additions))
        {
            var key = (sample.ResetAt, sample.Timestamp);
            // A timestamp may occur exactly once in the complete candidate.
            // Even byte-equivalent repeats are ambiguous wire evidence and
            // must not be silently normalized by the presentation layer.
            if (!merged.TryAdd(key, sample))
            {
                return null;
            }
        }

        return merged.Values
            .OrderBy(sample => sample.ResetAt)
            .ThenBy(sample => sample.Timestamp)
            .ToArray();
    }

    private static IReadOnlyList<ApiHistoryGap>? MergeHistoryGaps(
        IReadOnlyList<ApiHistoryGap> existing,
        IReadOnlyList<ApiHistoryGap> additions)
    {
        var merged = new Dictionary<string, ApiHistoryGap>(StringComparer.Ordinal);
        foreach (var gap in existing.Concat(additions))
        {
            if (merged.TryGetValue(gap.GapId, out var prior) && prior != gap)
            {
                return null;
            }

            merged[gap.GapId] = gap;
        }

        return merged.Values
            .OrderBy(gap => gap.ResetAt)
            .ThenBy(gap => gap.StartAt)
            .ThenBy(gap => gap.EndAt)
            .ThenBy(gap => gap.GapId, StringComparer.Ordinal)
            .ToArray();
    }

    private void Rebuild()
    {
        var previousId = selectedPeriod?.Id;
        periods.Clear();
        if (main.DetailsSnapshot is { } details)
        {
            foreach (var period in FormatPeriodsForDisplay(details.History))
            {
                periods.Add(period);
            }
        }

        selectedPeriod = periods.FirstOrDefault(period => period.Id == previousId)
            ?? periods.FirstOrDefault(period => period.Current)
            ?? periods.FirstOrDefault();

        RebuildPoints();
        Notify(nameof(HasPeriods));
        Notify(nameof(SelectedPeriod));
        Notify(nameof(SelectedPeriodText));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
    }

    private void ReformatPeriodLabels()
    {
        var selectedId = selectedPeriod?.Id;
        var resourceId = resourcePeriod?.Id;
        var displayedId = displayedPeriod?.Id;
        var formatted = FormatPeriodsForDisplay(periods);
        for (var index = 0; index < formatted.Count; index++)
        {
            periods[index] = formatted[index];
        }

        selectedPeriod = periods.FirstOrDefault(period => period.Id == selectedId);
        resourcePeriod = periods.FirstOrDefault(period => period.Id == resourceId);
        displayedPeriod = periods.FirstOrDefault(period => period.Id == displayedId);
    }

    private static IReadOnlyList<ApiHistoryPeriod> FormatPeriodsForDisplay(
        IEnumerable<ApiHistoryPeriod> source)
    {
        var formatted = source.Select(FormatPeriodForDisplay).ToArray();
        var totals = formatted
            .GroupBy(period => period.Label, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.Count(), StringComparer.Ordinal);
        var occurrences = new Dictionary<string, int>(StringComparer.Ordinal);
        for (var index = 0; index < formatted.Length; index++)
        {
            var label = formatted[index].Label;
            if (totals[label] <= 1)
            {
                continue;
            }

            occurrences.TryGetValue(label, out var occurrence);
            occurrence++;
            occurrences[label] = occurrence;
            formatted[index] = formatted[index] with
            {
                Label = $"{label} · {occurrence}/{totals[label]}",
            };
        }
        return formatted;
    }

    private static ApiHistoryPeriod FormatPeriodForDisplay(ApiHistoryPeriod period)
    {
        var label = LocalizationService.Current.FormatPeriodSelectorLabel(
            FormatPeriodStart(period.StartAt),
            period.Current);
        return period with { Label = label };
    }

    private static string FormatPeriodStart(long startAt) =>
        FormatPeriodStart(startAt, LocalizationService.Current.LanguageCode);

    internal static string FormatPeriodStart(long startAt, string languageCode)
    {
        var format = languageCode switch
        {
            "es" or "fr" or "de" or "pt" or "it" or "ru" => "dd/MM HH:mm",
            _ => "MM/dd HH:mm",
        };
        return TimeZoneInfo.ConvertTime(
                DateTimeOffset.FromUnixTimeSeconds(startAt),
                LocalizationService.DisplayTimeZone)
            .ToString(format, CultureInfo.InvariantCulture);
    }

    private void RebuildPoints()
    {
        pointBuildCancellation.Cancel();
        pointBuildCancellation.Dispose();
        pointBuildCancellation = new CancellationTokenSource();
        var cancellationToken = pointBuildCancellation.Token;
        var revision = ++pointBuildRevision;
        var period = selectedPeriod;
        var metric = selectedMetric;

        if (period is null)
        {
            SetLoading(false);
            PublishPoints(new GraphProjection(Array.Empty<GraphPointViewModel>(), GraphScene.Empty(metric)), null, metric);
            return;
        }

        var sourceCount = period.Samples.Count;
        var confirmedGaps = BuildConfirmedGaps(period);
        var hiddenModelNames = BuildHiddenModelNames(period.Samples);
        if (sourceCount <= BackgroundBuildThreshold)
        {
            try
            {
                PublishPoints(
                    BuildProjection(period, metric, confirmedGaps, hiddenModelNames),
                    period,
                    metric);
            }
            catch
            {
                PublishLoadFailure(revision);
            }
            return;
        }

        // Large history projection never runs on the UI thread.
        // The previously painted graph and its axis remain intact while the
        // selected period is prepared. Only the final transport-bounded,
        // immutable projection crosses back in one atomic publish.
        SetLoadError(false);
        SetLoading(true);
        var previewDelay = PreviewEnvironment.Enabled
            ? PreviewEnvironment.GraphBuildDelayMilliseconds
            : 0;
        _ = Task.Run(() =>
            {
                if (previewDelay > 0)
                {
                    Task.Delay(previewDelay, cancellationToken).GetAwaiter().GetResult();
                }
                return BuildProjection(period, metric, confirmedGaps, hiddenModelNames);
            }, cancellationToken)
            .ContinueWith(
                task =>
                {
                    if (disposed || cancellationToken.IsCancellationRequested || revision != pointBuildRevision)
                    {
                        return;
                    }
                    postToUi(() =>
                    {
                        if (disposed || cancellationToken.IsCancellationRequested || revision != pointBuildRevision)
                        {
                            return;
                        }
                        if (task.Status == TaskStatus.RanToCompletion)
                        {
                            PublishPoints(task.Result, period, metric);
                        }
                        else
                        {
                            PublishLoadFailure(revision);
                        }
                    });
                },
                CancellationToken.None,
                TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
    }

    private IReadOnlyList<GraphConfirmedGap> BuildConfirmedGaps(ApiHistoryPeriod period)
    {
        var end = EffectiveGraphEnd(period, DateTimeOffset.UtcNow.ToUnixTimeSeconds());
        if (resourceClient is not null)
        {
            return resourceGaps
                .Where(gap => GapBelongsToPeriod(gap, period) &&
                    gap.EndAt > period.StartAt && gap.StartAt < end)
                .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
                .ToArray();
        }

        return main.DetailsSnapshot?.HistoryGaps
                .Where(gap => GapBelongsToPeriod(gap, period) &&
                    gap.EndAt > period.StartAt && gap.StartAt < end)
                .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
                .ToArray()
            ?? Array.Empty<GraphConfirmedGap>();
    }

    private static bool GapBelongsToPeriod(ApiHistoryGap gap, ApiHistoryPeriod period) =>
        gap.ResetAt >= period.ResetAt - 60 && gap.ResetAt <= period.ResetAt;

    private IReadOnlySet<string> BuildHiddenModelNames(
        IReadOnlyList<ApiHistorySample> samples)
    {
        var hidden = new HashSet<string>(StringComparer.Ordinal);
        if (!showModels)
        {
            foreach (var model in samples.SelectMany(sample => sample.Models))
            {
                hidden.Add(model.Name);
            }
        }
        if (!showSol)
        {
            hidden.Add("SOL");
        }
        if (!showTerra)
        {
            hidden.Add("TERRA");
        }
        if (!showLuna)
        {
            hidden.Add("LUNA");
        }
        if (!showAstra)
        {
            hidden.Add("ASTRA");
        }
        return hidden;
    }

    private static GraphProjection BuildProjection(
        ApiHistoryPeriod period,
        GraphMetric metric,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps,
        IReadOnlySet<string> hiddenModelNames)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var samples = BuildGraphSamples(period, now);
        var diagnosticSamples = ReduceGraphSamples(samples, MaxRenderedGraphPoints, confirmedGaps);
        return new GraphProjection(
            diagnosticSamples.Select(sample => new GraphPointViewModel(sample, metric)).ToArray(),
            GraphScene.Create(
                samples,
                metric,
                period.StartAt,
                EffectiveGraphEnd(period, now),
                confirmedGaps,
                hiddenModelNames));
    }

    private void PublishPoints(
        GraphProjection next,
        ApiHistoryPeriod? period,
        GraphMetric metric)
    {
        points = next.Points;
        scene = next.Scene;
        displayedPeriod = period;
        displayedMetric = metric;
        SetLoadError(false);
        SetLoading(false);
        Notify(nameof(Points));
        Notify(nameof(Scene));
        Notify(nameof(HasPoints));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
        Notify(nameof(MetricAxisText));
        Notify(nameof(IsDollars));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
    }

    private readonly record struct GraphProjection(
        IReadOnlyList<GraphPointViewModel> Points,
        GraphScene Scene);

    private void PublishLoadFailure(long revision)
    {
        if (disposed || revision != pointBuildRevision)
        {
            return;
        }
        SetLoadError(true);
        SetLoading(false);
    }

    private void SetLoading(bool value)
    {
        if (isLoading == value)
        {
            return;
        }
        isLoading = value;
        Notify(nameof(IsLoading));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
    }

    private void SetLoadError(bool value)
    {
        if (hasLoadError == value)
        {
            return;
        }
        hasLoadError = value;
        Notify(nameof(HasLoadError));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
    }

    private void Notify([CallerMemberName] string? propertyName = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}

public sealed class ThreadsWindowViewModel : INotifyPropertyChanged, IDisposable
{
    private readonly MainWindowViewModel main;
    private readonly Action<Action> postToUi;
    private readonly ILoopbackResourceClient? resourceClient;
    private readonly ILoopbackAccountResourceClient? accountResourceClient;
    private readonly ObservableCollection<ThreadItemViewModel> threads = [];
    private bool disposed;
    private bool hasLoadError;
    private CancellationTokenSource? resourcePollingCancellation;
    private IReadOnlyList<ApiThreadDetails> resourceThreads = Array.Empty<ApiThreadDetails>();

    public ThreadsWindowViewModel(MainWindowViewModel main)
        : this(main, action => Avalonia.Threading.Dispatcher.UIThread.Post(action))
    {
    }

    internal ThreadsWindowViewModel(MainWindowViewModel main, Action<Action> postToUi)
    {
        this.main = main;
        this.postToUi = postToUi;
        resourceClient = main.SplitResourceClient;
        accountResourceClient = main.AccountResourceClient;
        Threads = new ReadOnlyObservableCollection<ThreadItemViewModel>(threads);
        main.PropertyChanged += OnMainPropertyChanged;
        if (resourceClient is null)
        {
            Rebuild();
        }
        else
        {
            resourcePollingCancellation = new CancellationTokenSource();
            _ = RunSplitResourcePollingAsync(resourcePollingCancellation.Token);
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ThreadItemViewModel> Threads { get; }

    public UiText Texts => LocalizationService.Current;

    public ReadOnlyObservableCollection<ApiAccount> Accounts => main.Accounts;

    public bool HasAccounts => main.HasAccounts;

    public ApiAccount? SelectedAccount
    {
        get => main.SelectedAccount;
        set
        {
            if (value is not null)
            {
                main.SelectAccount(value.Id);
            }
        }
    }

    public string SelectedAccountText => main.SelectedAccountText;

    public bool HasThreads => !main.IsSelectedAccountHistorical && threads.Count > 0;

    public bool HasNoThreads => !HasThreads;

    public bool HasLoadError => hasLoadError;

    public string EmptyText => main.IsSelectedAccountHistorical
        ? Texts.HistoricalThreadsUnavailable
        : Texts.NoRunningThreads;

    public string DetailsStatusText => main.DetailsStatusText;

    public string ThreadRole(ApiThreadDetails thread)
    {
        if (thread.IsOrphan)
        {
            return thread.IsSubAgent ? $"{Texts.SubThread} ({Texts.UnavailableValue})" : $"{Texts.MainThread} ({Texts.UnavailableValue})";
        }

        var prefix = thread.Depth is { } depth && depth > 0 ? new string('│', Math.Min(depth, 3)) + " " : string.Empty;
        return prefix + (thread.IsSubAgent
            ? thread.Depth is { } nestedDepth ? $"{Texts.SubThread} D{nestedDepth}" : Texts.SubThread
            : Texts.MainThread);
    }

    public string ParentText(ApiThreadDetails thread) => thread.ParentId is { } parent
        ? $"{Texts.Parent}: {parent}"
        : thread.IsOrphan && thread.IsSubAgent
            ? Texts.ParentUnavailable
            : Texts.UnavailableValue;

    public string ModelText(ApiThreadDetails thread) =>
        string.IsNullOrWhiteSpace(thread.ModelLabel) ? thread.Model : thread.ModelLabel;

    public string ContextText(ApiThreadDetails thread)
    {
        if (thread.ContextPercent is not { } percent)
        {
            return $"{Texts.Context} —";
        }

        return thread.ContextLimit is { } limit
            ? string.Create(CultureInfo.CurrentCulture, $"{Texts.Context} {percent:0.#}% / {limit:N0}")
            : string.Create(CultureInfo.CurrentCulture, $"{Texts.Context} {percent:0.#}%");
    }

    public string TokenText(ApiThreadDetails thread) => thread.CumulativeTokens is { } tokens
        ? string.Create(CultureInfo.CurrentCulture, $"{Texts.Tokens} {tokens:N0}")
        : $"{Texts.Tokens} —";

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }

        disposed = true;
        if (resourcePollingCancellation is not null)
        {
            resourcePollingCancellation.Cancel();
            resourcePollingCancellation.Dispose();
        }
        main.PropertyChanged -= OnMainPropertyChanged;
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName is nameof(MainWindowViewModel.Accounts) or
            nameof(MainWindowViewModel.HasAccounts) or
            nameof(MainWindowViewModel.SelectedAccount) or
            nameof(MainWindowViewModel.SelectedAccountText))
        {
            Notify(nameof(Accounts));
            Notify(nameof(HasAccounts));
            Notify(nameof(SelectedAccount));
            Notify(nameof(SelectedAccountText));
            Notify(nameof(HasThreads));
            Notify(nameof(HasNoThreads));
            Notify(nameof(EmptyText));
            if (eventArgs.PropertyName == nameof(MainWindowViewModel.SelectedAccount))
            {
                resourceThreads = Array.Empty<ApiThreadDetails>();
                threads.Clear();
                hasLoadError = false;
                Notify(nameof(HasThreads));
                Notify(nameof(HasNoThreads));
                Notify(nameof(HasLoadError));
                if (resourceClient is null)
                {
                    Rebuild();
                }
                else if (!disposed)
                {
                    _ = RefreshSplitResourceAsync(resourcePollingCancellation?.Token ?? main.LifetimeToken);
                }
            }
            return;
        }

        if (eventArgs.PropertyName is nameof(MainWindowViewModel.DetailsSnapshot) or
            nameof(MainWindowViewModel.DetailsStatusText) or nameof(MainWindowViewModel.Texts))
        {
            if (resourceClient is null)
            {
                Rebuild();
            }
            Notify(nameof(DetailsStatusText));
            Notify(nameof(Texts));
        }
    }

    private async Task RunSplitResourcePollingAsync(CancellationToken cancellationToken)
    {
        try
        {
            await RefreshSplitResourceAsync(cancellationToken).ConfigureAwait(false);
            using var timer = new PeriodicTimer(TimeSpan.FromSeconds(5));
            while (await timer.WaitForNextTickAsync(cancellationToken).ConfigureAwait(false))
            {
                await RefreshSplitResourceAsync(cancellationToken).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Closing the threads window owns cancellation.
        }
    }

    private async Task RefreshSplitResourceAsync(CancellationToken cancellationToken)
    {
        if (resourceClient is null || disposed || cancellationToken.IsCancellationRequested)
        {
            return;
        }

        var accountId = main.SelectedAccountId;
        var accountGeneration = main.AccountSelectionGeneration;
        if (main.IsSelectedAccountHistorical)
        {
            // Inactive accounts expose only a point-in-time current snapshot;
            // thread rows are intentionally not presented as live activity.
            return;
        }
        if (accountResourceClient is not null && accountId is null)
        {
            return;
        }

        ThreadsFetchResult result;
        if (accountResourceClient is null && main.HasAccounts)
        {
            result = ThreadsFetchResult.FromFailure(DetailsFetchFailure.Response);
        }
        else
        {
            try
            {
                result = accountId is not null && accountResourceClient is not null
                    ? await accountResourceClient.FetchThreadsAsync(accountId, cancellationToken)
                    : await resourceClient.FetchThreadsAsync(cancellationToken)
                        .ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                return;
            }
            catch
            {
                result = ThreadsFetchResult.FromFailure(DetailsFetchFailure.Transport);
            }
        }

        if (accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
        {
            return;
        }

        postToUi(() =>
        {
            if (disposed ||
                accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return;
            }

            if (!result.IsSuccess || result.Snapshot is not { } snapshot)
            {
                hasLoadError = true;
                Notify(nameof(HasLoadError));
                return;
            }

            if (accountId is not null && snapshot.AccountId != accountId)
            {
                hasLoadError = true;
                Notify(nameof(HasLoadError));
                return;
            }

            resourceThreads = snapshot.Threads;
            hasLoadError = false;
            Notify(nameof(HasLoadError));
            Rebuild();
        });
    }

    private void Rebuild()
    {
        threads.Clear();
        var source = main.IsSelectedAccountHistorical
            ? Array.Empty<ApiThreadDetails>()
            : resourceClient is not null
            ? resourceThreads
            : main.DetailsSnapshot?.Threads ?? Array.Empty<ApiThreadDetails>();
        if (source.Count > 0)
        {
            var ordered = ParentFirst(source);
            var byId = source.ToDictionary(thread => thread.Id, StringComparer.Ordinal);
            for (var index = 0; index < ordered.Count; index++)
            {
                var thread = ordered[index];
                var parentExists = thread.ParentId is { } parentId && byId.ContainsKey(parentId);
                var hasChildren = ordered.Any(candidate => candidate.ParentId == thread.Id);
                var hasNextSibling = ordered.Skip(index + 1).Any(candidate => candidate.ParentId == thread.ParentId);
                var depth = thread.Depth ?? CalculateDepth(thread, byId);
                var currentChain = AncestorChain(thread, byId);
                var ancestorGuides = new bool[3];
                for (var guide = 1; guide <= 3; guide++)
                {
                    ancestorGuides[guide - 1] = currentChain.Count >= guide && ordered.Skip(index + 1).Any(candidate =>
                    {
                        var candidateChain = AncestorChain(candidate, byId);
                        return candidateChain.Count >= guide && candidateChain[guide - 1] == currentChain[guide - 1];
                    });
                }
                var parentTitle = thread.ParentId is { } id && byId.TryGetValue(id, out var parent)
                    ? parent.Title
                    : string.Empty;
                threads.Add(new ThreadItemViewModel(this, thread, Math.Min(depth, 3), parentExists && !thread.IsOrphan,
                    hasChildren, hasNextSibling, ancestorGuides[0], ancestorGuides[1], ancestorGuides[2], parentTitle));
            }
        }

        Notify(nameof(HasThreads));
        Notify(nameof(HasNoThreads));
    }

    private static int CalculateDepth(ApiThreadDetails thread, IReadOnlyDictionary<string, ApiThreadDetails> byId)
    {
        var depth = 0;
        var current = thread;
        var seen = new HashSet<string>(StringComparer.Ordinal);
        while (current.ParentId is { } parentId && byId.TryGetValue(parentId, out var parent) && seen.Add(parentId))
        {
            depth++;
            current = parent;
        }
        return depth;
    }

    private static IReadOnlyList<string> AncestorChain(ApiThreadDetails thread, IReadOnlyDictionary<string, ApiThreadDetails> byId)
    {
        var reverse = new List<string> { thread.Id };
        var current = thread;
        var seen = new HashSet<string>(StringComparer.Ordinal) { thread.Id };
        while (current.ParentId is { } parentId && byId.TryGetValue(parentId, out var parent) && seen.Add(parentId))
        {
            reverse.Add(parent.Id);
            current = parent;
        }
        reverse.Reverse();
        return reverse;
    }

    private static IReadOnlyList<ApiThreadDetails> ParentFirst(IReadOnlyList<ApiThreadDetails> source)
    {
        var byId = source.ToDictionary(thread => thread.Id, StringComparer.Ordinal);
        var children = source.Where(thread => thread.ParentId is not null).GroupBy(thread => thread.ParentId!, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.ToList(), StringComparer.Ordinal);
        var result = new List<ApiThreadDetails>(source.Count);
        var visited = new HashSet<string>(StringComparer.Ordinal);
        void Visit(ApiThreadDetails item)
        {
            if (!visited.Add(item.Id)) return;
            result.Add(item);
            if (children.TryGetValue(item.Id, out var nested))
                foreach (var child in nested) Visit(child);
        }
        foreach (var root in source.Where(thread => thread.ParentId is null || !byId.ContainsKey(thread.ParentId))) Visit(root);
        foreach (var item in source) Visit(item);
        return result;
    }

    private void Notify([CallerMemberName] string? propertyName = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}

public sealed class ThreadItemViewModel
{
    public ThreadItemViewModel(ThreadsWindowViewModel owner, ApiThreadDetails thread, int treeDepth,
        bool connectedToParent, bool hasChildren, bool hasNextSibling,
        bool ancestorGuide1, bool ancestorGuide2, bool ancestorGuide3, string parentTitle)
    {
        Id = thread.Id;
        Title = thread.Title;
        RoleText = owner.ThreadRole(thread);
        ParentText = string.IsNullOrWhiteSpace(parentTitle)
            ? owner.ParentText(thread)
            : $"{owner.ParentText(thread)} / {parentTitle}";
        ModelText = owner.ModelText(thread);
        ContextText = owner.ContextText(thread);
        TokenText = owner.TokenText(thread);
        DepthText = thread.Depth is { } depth ? $"{owner.Texts.Depth} {depth}" : $"{owner.Texts.Depth} —";
        AgeText = owner.Texts.FormatElapsed(thread.CreatedAt, owner.Texts.Elapsed);
        InstructionAgeText = owner.Texts.FormatElapsed(thread.LastUserMessageAt, owner.Texts.Instruction);
        TreeDepth = treeDepth;
        ConnectedToParent = connectedToParent;
        HasChildren = hasChildren;
        HasNextSibling = hasNextSibling;
        AncestorGuide1 = ancestorGuide1;
        AncestorGuide2 = ancestorGuide2;
        AncestorGuide3 = ancestorGuide3;
        ParentTitle = parentTitle;
    }

    public string Id { get; }
    public string Title { get; }
    public string RoleText { get; }
    public string ParentText { get; }
    public string ModelText { get; }
    public string ContextText { get; }
    public string TokenText { get; }
    public string DepthText { get; }
    public string AgeText { get; }
    public string InstructionAgeText { get; }
    public int TreeDepth { get; }
    public bool ConnectedToParent { get; }
    public bool HasChildren { get; }
    public bool HasNextSibling { get; }
    public bool AncestorGuide1 { get; }
    public bool AncestorGuide2 { get; }
    public bool AncestorGuide3 { get; }
    public string ParentTitle { get; }

}

public sealed class LegalNoticesWindowViewModel : INotifyPropertyChanged, IDisposable
{
    private readonly MainWindowViewModel main;
    private readonly ObservableCollection<ApiLegalNotice> notices = [];
    private readonly AsyncCommand backCommand;
    private readonly AsyncCommand nextCommand;
    private int currentPageIndex;
    private bool disposed;

    public LegalNoticesWindowViewModel(MainWindowViewModel main)
    {
        this.main = main;
        Notices = new ReadOnlyObservableCollection<ApiLegalNotice>(notices);
        backCommand = new AsyncCommand(MoveBackAsync, () => CanGoBack);
        nextCommand = new AsyncCommand(MoveNextAsync, () => CanGoNext);
        main.PropertyChanged += OnMainPropertyChanged;
        Rebuild();
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ApiLegalNotice> Notices { get; }

    public UiText Texts => LocalizationService.Current;

    public bool HasNotices => notices.Count > 0;

    /// <summary>The zero-based chapter index used by the navigation state.</summary>
    public int CurrentPageIndex => currentPageIndex;

    /// <summary>The one-based chapter number shown to the user.</summary>
    public int CurrentPageNumber => notices.Count == 0 ? 0 : currentPageIndex + 1;

    // Keep a short alias for callers that use the display-oriented name.
    public int CurrentPage => CurrentPageNumber;

    public int PageCount => notices.Count;

    public ApiLegalNotice? CurrentNotice => notices.Count == 0 ? null : notices[currentPageIndex];

    public string CurrentNoticeName => CurrentNotice?.Name ?? string.Empty;

    public string CurrentNoticeText => CurrentNotice?.Text ?? string.Empty;

    public bool CanGoBack => currentPageIndex > 0;

    public bool CanGoNext => currentPageIndex + 1 < notices.Count;

    public string BackText => Texts.LanguageCode switch
    {
        "ja" => "戻る",
        "zh-Hans" => "返回",
        "ko" => "뒤로",
        "es" => "Atrás",
        "fr" => "Retour",
        "de" => "Zurück",
        "pt" => "Voltar",
        "it" => "Indietro",
        "ru" => "Назад",
        _ => "Back",
    };

    public string NextText => Texts.LanguageCode switch
    {
        "ja" => "次へ",
        "zh-Hans" => "下一页",
        "ko" => "다음",
        "es" => "Siguiente",
        "fr" => "Suivant",
        "de" => "Weiter",
        "pt" => "Próximo",
        "it" => "Avanti",
        "ru" => "Далее",
        _ => "Next",
    };

    public string PagePositionText => Texts.LanguageCode switch
    {
        "ja" => $"ページ {CurrentPageNumber} / {PageCount}",
        "zh-Hans" => $"第 {CurrentPageNumber} / {PageCount} 页",
        "ko" => $"페이지 {CurrentPageNumber} / {PageCount}",
        "es" => $"Página {CurrentPageNumber} / {PageCount}",
        "fr" => $"Page {CurrentPageNumber} / {PageCount}",
        "de" => $"Seite {CurrentPageNumber} / {PageCount}",
        "pt" => $"Página {CurrentPageNumber} / {PageCount}",
        "it" => $"Pagina {CurrentPageNumber} / {PageCount}",
        "ru" => $"Страница {CurrentPageNumber} / {PageCount}",
        _ => $"Page {CurrentPageNumber} / {PageCount}",
    };

    public ICommand BackCommand => backCommand;

    public ICommand NextCommand => nextCommand;

    public string DetailsStatusText => main.DetailsStatusText;

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }

        disposed = true;
        main.PropertyChanged -= OnMainPropertyChanged;
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName is nameof(MainWindowViewModel.DetailsSnapshot) or
            nameof(MainWindowViewModel.DetailsStatusText) or nameof(MainWindowViewModel.Texts))
        {
            Rebuild();
            Notify(nameof(DetailsStatusText));
            Notify(nameof(Texts));
        }
    }

    private void Rebuild()
    {
        notices.Clear();
        // Legal information remains reachable before authentication and when
        // the auxiliary endpoint is unavailable. It contains only packaged
        // repository documents and never depends on account/backend data.
        foreach (var notice in LegalNoticeCatalog.Load(Texts))
        {
            notices.Add(notice);
        }

        currentPageIndex = notices.Count == 0 ? 0 : Math.Clamp(currentPageIndex, 0, notices.Count - 1);
        NotifyPageProperties();
    }

    private Task MoveBackAsync()
    {
        SetPage(currentPageIndex - 1);
        return Task.CompletedTask;
    }

    private Task MoveNextAsync()
    {
        SetPage(currentPageIndex + 1);
        return Task.CompletedTask;
    }

    private void SetPage(int requestedIndex)
    {
        var nextIndex = notices.Count == 0 ? 0 : Math.Clamp(requestedIndex, 0, notices.Count - 1);
        if (nextIndex == currentPageIndex)
        {
            return;
        }

        currentPageIndex = nextIndex;
        NotifyPageProperties();
    }

    private void NotifyPageProperties()
    {
        Notify(nameof(HasNotices));
        Notify(nameof(CurrentPageIndex));
        Notify(nameof(CurrentPageNumber));
        Notify(nameof(CurrentPage));
        Notify(nameof(PageCount));
        Notify(nameof(CurrentNotice));
        Notify(nameof(CurrentNoticeName));
        Notify(nameof(CurrentNoticeText));
        Notify(nameof(CanGoBack));
        Notify(nameof(CanGoNext));
        Notify(nameof(BackText));
        Notify(nameof(NextText));
        Notify(nameof(PagePositionText));
        backCommand.RaiseCanExecuteChanged();
        nextCommand.RaiseCanExecuteChanged();
    }

    private void Notify([CallerMemberName] string? propertyName = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}
