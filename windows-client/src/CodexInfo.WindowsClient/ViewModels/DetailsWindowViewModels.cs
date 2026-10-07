// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Globalization;
using System.Runtime.CompilerServices;
using System.Windows.Input;
using Avalonia.Threading;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Theme;

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
    private readonly Func<long> getUnixTimeSeconds;
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
    private CancellationTokenSource? timeWindowRequestCancellation;
    private string? resourceNextCursor;
    private PublishedPairIdentity? resourcePublishedPair;
    private ApiHistoryPeriod? resourcePeriod;
    private IReadOnlyList<ApiHistorySample> resourceSamples = Array.Empty<ApiHistorySample>();
    private IReadOnlyList<ApiHistoryGap> resourceGaps = Array.Empty<ApiHistoryGap>();
    private int selectedTimeRangeValue = (int)GraphTimeRange.ResetPeriod;
    private long pinnedWindowEndAt = long.MinValue;
    private long? windowNavigationOriginAt;
    private long timeWindowRevision;
    private long resetRangeCommitRevision = -1;
    private bool timeWindowRequestPending;
    private bool staticDetailsRebuildDeferred;
    private bool rebuildingPeriodDirectory;
    private IReadOnlyList<GraphWindowPeriodData> staticWindowPeriodData = Array.Empty<GraphWindowPeriodData>();
    private IReadOnlyList<ApiHistoryPeriod> windowPeriodDirectory = Array.Empty<ApiHistoryPeriod>();
    private IReadOnlyDictionary<string, GraphWindowPeriodData> windowPeriodData =
        new Dictionary<string, GraphWindowPeriodData>(StringComparer.Ordinal);
    private PublishedPairIdentity? windowPublishedPair;
    private Dictionary<WindowProjectionCacheKey, CachedWindowProjection> windowProjectionCache = [];

    public GraphWindowViewModel(MainWindowViewModel main)
        : this(main, action => Dispatcher.UIThread.Post(action), static () => DateTimeOffset.UtcNow.ToUnixTimeSeconds())
    {
    }

    internal GraphWindowViewModel(MainWindowViewModel main, Action<Action> postToUi)
        : this(main, postToUi, static () => DateTimeOffset.UtcNow.ToUnixTimeSeconds())
    {
    }

    internal GraphWindowViewModel(MainWindowViewModel main, Action<Action> postToUi, Func<long> getUnixTimeSeconds)
    {
        this.main = main;
        this.postToUi = postToUi;
        this.getUnixTimeSeconds = getUnixTimeSeconds;
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

    public GraphTimeRange SelectedTimeRange
    {
        get => (GraphTimeRange)Volatile.Read(ref selectedTimeRangeValue);
        set
        {
            if (!Enum.IsDefined(value))
            {
                throw new ArgumentOutOfRangeException(nameof(value));
            }

            var acceptedRange = SelectedTimeRange;
            if (value == acceptedRange && !timeWindowRequestPending)
            {
                return;
            }

            var pinnedEnd = value != GraphTimeRange.ResetPeriod &&
                acceptedRange != GraphTimeRange.ResetPeriod && pinnedWindowEndAt != long.MinValue
                    ? pinnedWindowEndAt
                    : (long?)null;
            var navigationOriginAt = pinnedEnd is null ? null : windowNavigationOriginAt;
            RequestTimeRange(value, pinnedEnd, navigationOriginAt);
        }
    }

    public bool IsPeriodView => SelectedTimeRange == GraphTimeRange.ResetPeriod;

    public bool Is24HourView => SelectedTimeRange == GraphTimeRange.Last24Hours;

    public bool IsWeekView => SelectedTimeRange == GraphTimeRange.Last7Days;

    public bool CanGoBack
    {
        get
        {
            if (SelectedTimeRange == GraphTimeRange.ResetPeriod)
            {
                var selectedIndex = GetSelectedPeriodIndex();
                return selectedIndex >= 0 && selectedIndex < periods.Count - 1;
            }

            return windowPeriodDirectory.Any(period => period.StartAt < scene.PeriodStartAt);
        }
    }

    public bool CanGoForward
    {
        get
        {
            if (SelectedTimeRange == GraphTimeRange.ResetPeriod)
            {
                return GetSelectedPeriodIndex() > 0;
            }

            return pinnedWindowEndAt != long.MinValue && pinnedWindowEndAt < getUnixTimeSeconds();
        }
    }

    public bool HasPlot => SelectedTimeRange != GraphTimeRange.ResetPeriod || HasPoints;

    public string RangeLabel
    {
        get
        {
            var range = SelectedTimeRange;
            var title = range switch
            {
                GraphTimeRange.Last24Hours => Texts.GraphDayRange,
                GraphTimeRange.Last7Days => Texts.GraphWeekRange,
                _ => Texts.GraphPeriodRange,
            };
            var startAt = range == GraphTimeRange.ResetPeriod
                ? selectedPeriod?.StartAt ?? (scene.HasPoints ? scene.PeriodStartAt : 0)
                : scene.PeriodStartAt;
            var endAt = range == GraphTimeRange.ResetPeriod
                ? selectedPeriod?.EndAt ?? (scene.HasPoints ? scene.PeriodEndAt : 0)
                : scene.PeriodEndAt;
            return startAt > 0 && endAt > startAt
                ? $"{title} · {FormatPeriodStart(startAt)} – {FormatPeriodStart(endAt)}"
                : title;
        }
    }

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

    public string SelectedAccountValueText => main.SelectedAccountText;

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
            if (value is null && (rebuildingPeriodDirectory || applyingSplitResourceState))
            {
                return;
            }

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
            Notify(nameof(SelectedPeriodValueText));
            Notify(nameof(SelectedPeriodStartAt));
            Notify(nameof(SelectedPeriodEndAt));
        }
    }

    public bool HasPoints => scene.IsViewport ? scene.HasPoints : points.Count > 0;

    public bool HasNoPoints => !IsLoading && !HasLoadError && !HasPoints;

    public bool HasBlockingLoadError => !IsLoading && HasLoadError && !HasPoints;

    public bool HasPeriods => periods.Count > 0;

    public string SelectedPeriodText => selectedPeriod is { } period
        ? $"{Texts.PeriodSelectorHeading}｜{period.Label}"
        : $"{Texts.PeriodSelectorHeading}｜{Texts.UnavailableValue}";

    public string SelectedPeriodValueText => selectedPeriod?.Label ?? Texts.UnavailableValue;

    public long SelectedPeriodStartAt => SelectedTimeRange != GraphTimeRange.ResetPeriod
        ? scene.PeriodStartAt
        : scene.HasPoints ? scene.PeriodStartAt : displayedPeriod?.StartAt ?? 0;

    public long SelectedPeriodEndAt => SelectedTimeRange != GraphTimeRange.ResetPeriod
        ? scene.PeriodEndAt
        : scene.HasPoints ? scene.PeriodEndAt : 0;

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
            AddIfRoom(selected, start, maximum);
            AddIfRoom(selected, Math.Max(start, endExclusive - 1), maximum);
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

    private static void AddIfRoom(SortedSet<int> selected, int index, int maximum)
    {
        if (selected.Count < maximum)
        {
            selected.Add(index);
        }
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

    public void GoBack()
    {
        var range = SelectedTimeRange;
        if (range == GraphTimeRange.ResetPeriod)
        {
            if (CanGoBack)
            {
                SelectedPeriod = periods[GetSelectedPeriodIndex() + 1];
            }

            return;
        }

        if (!CanGoBack)
        {
            return;
        }

        var now = getUnixTimeSeconds();
        long? pinnedEndAt = pinnedWindowEndAt == long.MinValue ? null : pinnedWindowEndAt;
        var navigationOriginAt = windowNavigationOriginAt ?? (pinnedEndAt is { } pinned
            ? pinned + GraphTimeWindow.GetDurationSeconds(range)
            : now);
        RequestTimeRange(
            range,
            GraphTimeWindow.StepBack(range, now, pinnedEndAt),
            navigationOriginAt,
            now);
    }

    public void GoForward()
    {
        var range = SelectedTimeRange;
        if (range == GraphTimeRange.ResetPeriod)
        {
            if (CanGoForward)
            {
                SelectedPeriod = periods[GetSelectedPeriodIndex() - 1];
            }

            return;
        }

        if (pinnedWindowEndAt == long.MinValue)
        {
            return;
        }

        var now = getUnixTimeSeconds();
        if (pinnedWindowEndAt >= now)
        {
            return;
        }

        var nextEndAt = GraphTimeWindow.StepForward(
            range,
            now,
            pinnedWindowEndAt,
            windowNavigationOriginAt);
        RequestTimeRange(range, nextEndAt, nextEndAt is null ? null : windowNavigationOriginAt, now);
    }

    private int GetSelectedPeriodIndex()
    {
        var selectedId = selectedPeriod?.Id;
        if (selectedId is null)
        {
            return -1;
        }

        for (var index = 0; index < periods.Count; index++)
        {
            if (periods[index].Id == selectedId)
            {
                return index;
            }
        }

        return -1;
    }

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }

        disposed = true;
        timeWindowRequestCancellation?.Cancel();
        timeWindowRequestCancellation?.Dispose();
        timeWindowRequestCancellation = null;
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
            Notify(nameof(SelectedAccountValueText));
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
            Notify(nameof(SelectedAccountText));
            Notify(nameof(SelectedAccountValueText));
            Notify(nameof(SelectedPeriodText));
            Notify(nameof(SelectedPeriodValueText));
            Notify(nameof(MetricAxisText));
            Notify(nameof(GraphGapHintText));
            Notify(nameof(RangeLabel));
        }
    }

    private void ClearAccountResourceState()
    {
        if (disposed)
        {
            return;
        }

        Interlocked.Increment(ref periodSelectionRevision);
        Interlocked.Increment(ref timeWindowRevision);
        resetRangeCommitRevision = -1;
        timeWindowRequestPending = false;
        staticDetailsRebuildDeferred = false;
        timeWindowRequestCancellation?.Cancel();
        timeWindowRequestCancellation?.Dispose();
        timeWindowRequestCancellation = null;
        Volatile.Write(ref selectedTimeRangeValue, (int)GraphTimeRange.ResetPeriod);
        pinnedWindowEndAt = long.MinValue;
        windowNavigationOriginAt = null;
        windowPeriodDirectory = Array.Empty<ApiHistoryPeriod>();
        windowPeriodData = new Dictionary<string, GraphWindowPeriodData>(StringComparer.Ordinal);
        windowPublishedPair = null;
        staticWindowPeriodData = Array.Empty<GraphWindowPeriodData>();
        windowProjectionCache.Clear();
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
        Notify(nameof(SelectedPeriodValueText));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
        Notify(nameof(HasPlot));
        Notify(nameof(RangeLabel));
        Notify(nameof(CanGoBack));
        Notify(nameof(CanGoForward));
        Notify(nameof(Points));
        Notify(nameof(Scene));
        Notify(nameof(HasPoints));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
        NotifyTimeRangeProperties();
    }

    private void RequestTimeRange(
        GraphTimeRange range,
        long? requestedPinnedEndAt,
        long? requestedNavigationOriginAt = null,
        long? requestNow = null)
    {
        if (disposed)
        {
            return;
        }

        var revision = Interlocked.Increment(ref timeWindowRevision);
        Interlocked.Increment(ref periodSelectionRevision);
        pointBuildCancellation.Cancel();
        pointBuildCancellation.Dispose();
        pointBuildCancellation = new CancellationTokenSource();
        Interlocked.Increment(ref pointBuildRevision);
        resetRangeCommitRevision = -1;
        timeWindowRequestCancellation?.Cancel();
        timeWindowRequestCancellation?.Dispose();
        timeWindowRequestCancellation = null;

        if (range == GraphTimeRange.ResetPeriod)
        {
            timeWindowRequestPending = true;
            resetRangeCommitRevision = revision;
            SetLoadError(false);
            SetLoading(true);
            NotifyTimeRangeProperties();

            if (resourceClient is not null && SelectedTimeRange != GraphTimeRange.ResetPeriod)
            {
                _ = RefreshSplitResourceCoreAsync(
                    initial: true,
                    requestedPeriodId: null,
                    Interlocked.Read(ref periodSelectionRevision),
                    resourcePollingCancellation?.Token ?? main.LifetimeToken);
                return;
            }

            RebuildPoints();
            return;
        }

        var now = requestNow ?? getUnixTimeSeconds();
        var pinnedEndAt = requestedPinnedEndAt is { } requested && requested < now
            ? requested
            : (long?)null;
        var navigationOriginAt = pinnedEndAt is null
            ? null
            : requestedNavigationOriginAt ?? windowNavigationOriginAt ??
                pinnedEndAt + GraphTimeWindow.GetDurationSeconds(range);
        var bounds = GraphTimeWindow.GetBounds(range, now, pinnedEndAt);
        timeWindowRequestPending = true;
        SetLoadError(false);
        SetLoading(true);
        NotifyTimeRangeProperties();

        if (resourceClient is null)
        {
            var data = staticWindowPeriodData
                .Where(item => Intersects(item.Period, bounds))
                .ToArray();
            var options = CaptureWindowBuildOptions();
            var cache = new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>(windowProjectionCache);
            if (data.Sum(item => item.Samples.Count) <= BackgroundBuildThreshold)
            {
                try
                {
                    PublishTimeWindow(range, pinnedEndAt, navigationOriginAt, revision, data, windowPeriodDirectory,
                        windowPublishedPair, BuildWindowProjection(bounds, data, options, cache),
                        accountId: null, accountGeneration: 0);
                }
                catch
                {
                    PublishTimeWindowFailure(revision, accountId: null, accountGeneration: 0);
                }
                return;
            }

            var buildSnapshot = new WindowBuildSnapshot(
                options,
                cache,
                Interlocked.Read(ref pointBuildRevision));
            BuildAndPublishWindowCandidate(
                range,
                bounds,
                pinnedEndAt,
                navigationOriginAt,
                revision,
                explicitRequest: true,
                candidateData: data,
                directory: windowPeriodDirectory,
                pair: windowPublishedPair,
                accountId: null,
                accountGeneration: 0,
                buildSnapshot: buildSnapshot);
            return;
        }

        if (TryPublishCachedWindow(range, bounds, pinnedEndAt, navigationOriginAt, revision))
        {
            return;
        }

        var cancellationToken = resourcePollingCancellation?.Token ?? main.LifetimeToken;
        timeWindowRequestCancellation = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        _ = RefreshTimeWindowCoreAsync(
            range,
            bounds,
            pinnedEndAt,
            navigationOriginAt,
            revision,
            forceRefresh: false,
            explicitRequest: true,
            timeWindowRequestCancellation.Token);
    }

    private void RebuildAcceptedWindowProjection()
    {
        var range = SelectedTimeRange;
        if (range == GraphTimeRange.ResetPeriod)
        {
            return;
        }

        var now = getUnixTimeSeconds();
        var pinnedEndAt = this.pinnedWindowEndAt == long.MinValue
            ? (long?)null
            : this.pinnedWindowEndAt;
        var bounds = GraphTimeWindow.GetBounds(range, now, pinnedEndAt);
        var data = resourceClient is null
            ? staticWindowPeriodData.Where(item => Intersects(item.Period, bounds)).ToArray()
            : GetCachedWindowData(bounds);
        if (resourceClient is not null &&
            data.Count != windowPeriodDirectory.Count(period => Intersects(period, bounds)))
        {
            RequestTimeRange(range, pinnedEndAt, windowNavigationOriginAt);
            return;
        }
        var revision = Interlocked.Increment(ref pointBuildRevision);
        var rangeRevision = Interlocked.Read(ref timeWindowRevision);
        var options = CaptureWindowBuildOptions();
        var cache = new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>(windowProjectionCache);
        var build = () => BuildWindowProjection(bounds, data, options, cache);
        if (data.Sum(item => item.Samples.Count) <= BackgroundBuildThreshold)
        {
            try
            {
                var projection = build();
                if (!disposed && revision == pointBuildRevision && rangeRevision == Interlocked.Read(ref timeWindowRevision))
                {
                    PublishWindowProjection(projection, range, pinnedEndAt, data, windowPeriodDirectory,
                        windowPublishedPair, accountId: main.SelectedAccountId,
                        accountGeneration: main.AccountSelectionGeneration, rangeRevision);
                }
            }
            catch
            {
                PublishLoadFailure(revision);
            }
            return;
        }

        SetLoadError(false);
        SetLoading(true);
        _ = Task.Run(build)
            .ContinueWith(
                task =>
                {
                    if (disposed || revision != pointBuildRevision || rangeRevision != Interlocked.Read(ref timeWindowRevision))
                    {
                        return;
                    }
                    postToUi(() =>
                    {
                        if (disposed || revision != pointBuildRevision || rangeRevision != Interlocked.Read(ref timeWindowRevision))
                        {
                            return;
                        }
                        if (task.Status == TaskStatus.RanToCompletion)
                        {
                            PublishWindowProjection(task.Result, range, pinnedEndAt, data, windowPeriodDirectory,
                                windowPublishedPair, main.SelectedAccountId,
                                main.AccountSelectionGeneration, rangeRevision);
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

    private bool TryPublishCachedWindow(
        GraphTimeRange range,
        GraphTimeBounds bounds,
        long? pinnedEndAt,
        long? navigationOriginAt,
        long revision)
    {
        if (windowPublishedPair is null)
        {
            return false;
        }

        var requiredCount = windowPeriodDirectory.Count(period => Intersects(period, bounds));
        var data = GetCachedWindowData(bounds);
        if (data.Count != requiredCount)
        {
            return false;
        }

        var accountId = main.SelectedAccountId;
        var accountGeneration = main.AccountSelectionGeneration;
        if (accountResourceClient is not null && accountId is null)
        {
            return false;
        }
        var options = CaptureWindowBuildOptions();
        var cache = new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>(windowProjectionCache);
        if (data.Sum(item => item.Samples.Count) <= BackgroundBuildThreshold)
        {
            try
            {
                var projection = BuildWindowProjection(bounds, data, options, cache);
                PublishTimeWindow(
                    range,
                    pinnedEndAt,
                    navigationOriginAt,
                    revision,
                    data,
                    windowPeriodDirectory,
                    windowPublishedPair,
                    projection,
                    accountId,
                    accountGeneration);
            }
            catch
            {
                PublishTimeWindowFailure(revision, accountId, accountGeneration);
            }
            return true;
        }

        BuildAndPublishWindowCandidate(
            range,
            bounds,
            pinnedEndAt,
            navigationOriginAt,
            revision,
            explicitRequest: true,
            candidateData: data,
            directory: windowPeriodDirectory,
            pair: windowPublishedPair,
            accountId: accountId,
            accountGeneration: accountGeneration,
            buildSnapshot: new WindowBuildSnapshot(
                options,
                cache,
                Interlocked.Read(ref pointBuildRevision)));
        return true;
    }

    private IReadOnlyList<GraphWindowPeriodData> GetCachedWindowData(GraphTimeBounds bounds)
    {
        var directory = windowPeriodDirectory;
        var cached = windowPeriodData;
        var required = directory.Where(period => Intersects(period, bounds)).ToArray();
        return required
            .Where(period => cached.TryGetValue(period.Id, out var item) && SamePeriodBounds(item.Period, period))
            .Select(period => cached[period.Id])
            .ToArray();
    }

    private IReadOnlyList<GraphWindowPeriodData> BuildStaticWindowPeriodData(
        IEnumerable<ApiHistoryPeriod> source)
    {
        var gaps = main.DetailsSnapshot?.HistoryGaps ?? Array.Empty<ApiHistoryGap>();
        return source.Select(period => new GraphWindowPeriodData(
            period,
            period.Samples,
            gaps.Where(gap => GapBelongsToPeriod(gap, period)).ToArray()))
            .ToArray();
    }

    private WindowBuildOptions CaptureWindowBuildOptions() => new(
        selectedMetric,
        showModels,
        showSol,
        showTerra,
        showLuna,
        showAstra,
        main.SelectedAccount?.OwnershipIntervals?
            .Select(interval => new GraphAccountOwnershipInterval(interval.StartAt, interval.EndAt))
            .ToArray());

    private async Task<WindowBuildSnapshot> CaptureWindowBuildSnapshotAsync(
        CancellationToken cancellationToken)
    {
        var completion = new TaskCompletionSource<WindowBuildSnapshot>(TaskCreationOptions.RunContinuationsAsynchronously);
        postToUi(() =>
        {
            if (cancellationToken.IsCancellationRequested || disposed)
            {
                completion.TrySetCanceled(cancellationToken);
                return;
            }

            completion.TrySetResult(new WindowBuildSnapshot(
                CaptureWindowBuildOptions(),
                new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>(windowProjectionCache),
                Interlocked.Read(ref pointBuildRevision)));
        });
        return await completion.Task.WaitAsync(cancellationToken).ConfigureAwait(false);
    }

    private void BuildAndPublishWindowCandidate(
        GraphTimeRange range,
        GraphTimeBounds bounds,
        long? pinnedEndAt,
        long? navigationOriginAt,
        long revision,
        bool explicitRequest,
        IReadOnlyList<GraphWindowPeriodData> candidateData,
        IReadOnlyList<ApiHistoryPeriod> directory,
        PublishedPairIdentity? pair,
        string? accountId,
        long accountGeneration,
        WindowBuildSnapshot buildSnapshot)
    {
        _ = Task.Run(() => BuildWindowProjection(
                bounds,
                candidateData,
                buildSnapshot.Options,
                buildSnapshot.Cache))
            .ContinueWith(
                task =>
                {
                    if (disposed || revision != Interlocked.Read(ref timeWindowRevision) ||
                        accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
                    {
                        return;
                    }

                    postToUi(() =>
                    {
                        if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                        {
                            return;
                        }

                        if (buildSnapshot.PointBuildRevision != Interlocked.Read(ref pointBuildRevision))
                        {
                            // A metric, series visibility, or account setting
                            // changed while this immutable candidate was being
                            // projected. Rebuild from the same fetched pair
                            // using the newest UI-owned settings; never publish
                            // the stale visual generation.
                            var latest = new WindowBuildSnapshot(
                                CaptureWindowBuildOptions(),
                                new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>(windowProjectionCache),
                                Interlocked.Read(ref pointBuildRevision));
                            BuildAndPublishWindowCandidate(
                                range,
                                bounds,
                                pinnedEndAt,
                                navigationOriginAt,
                                revision,
                                explicitRequest,
                                candidateData,
                                directory,
                                pair,
                                accountId,
                                accountGeneration,
                                latest);
                            return;
                        }

                        if (task.Status == TaskStatus.RanToCompletion)
                        {
                            PublishTimeWindowOnUi(
                                range,
                                pinnedEndAt,
                                navigationOriginAt,
                                revision,
                                candidateData,
                                directory,
                                pair,
                                task.Result,
                                accountId,
                                accountGeneration);
                        }
                        else
                        {
                            PublishTimeWindowFailure(revision, accountId, accountGeneration);
                        }
                    });
                },
                CancellationToken.None,
                TaskContinuationOptions.ExecuteSynchronously,
                TaskScheduler.Default);
    }

    private static bool Intersects(ApiHistoryPeriod period, GraphTimeBounds bounds) =>
        period.StartAt < bounds.EndAt && period.EndAt > bounds.StartAt;

    private static bool SamePeriodBounds(ApiHistoryPeriod left, ApiHistoryPeriod right) =>
        left.Id == right.Id && left.StartAt == right.StartAt && left.EndAt == right.EndAt &&
        left.ResetAt == right.ResetAt && left.Current == right.Current;

    private static bool AreWindowPeriodDataEqual(
        GraphWindowPeriodData left,
        GraphWindowPeriodData right) =>
        SamePeriodBounds(left.Period, right.Period) &&
        left.Gaps.SequenceEqual(right.Gaps) &&
        left.Samples.Count == right.Samples.Count &&
        left.Samples.Zip(right.Samples).All(pair => AreWindowSamplesEqual(pair.First, pair.Second));

    private static bool AreWindowSamplesEqual(ApiHistorySample left, ApiHistorySample right) =>
        left.Timestamp == right.Timestamp &&
        left.ResetAt == right.ResetAt &&
        left.RemainingPercent == right.RemainingPercent &&
        left.SolDollars == right.SolDollars &&
        left.TerraDollars == right.TerraDollars &&
        left.LunaDollars == right.LunaDollars &&
        left.SolTokens == right.SolTokens &&
        left.TerraTokens == right.TerraTokens &&
        left.LunaTokens == right.LunaTokens &&
        left.ModelSource == right.ModelSource &&
        left.ModelsComplete == right.ModelsComplete &&
        left.TaskActiveSincePrevious == right.TaskActiveSincePrevious &&
        left.IsSyntheticTail == right.IsSyntheticTail &&
        (left.ModelSamples is null) == (right.ModelSamples is null) &&
        (left.ModelSamples is null || left.ModelSamples.SequenceEqual(right.ModelSamples!));

    private static WindowGraphProjection BuildWindowProjection(
        GraphTimeBounds bounds,
        IReadOnlyList<GraphWindowPeriodData> data,
        WindowBuildOptions options,
        IReadOnlyDictionary<WindowProjectionCacheKey, CachedWindowProjection> priorCache)
    {
        var points = new List<GraphPointViewModel>();
        var children = new List<GraphScene>();
        var nextCache = new Dictionary<WindowProjectionCacheKey, CachedWindowProjection>();
        foreach (var item in data.OrderBy(item => item.Period.StartAt).ThenBy(item => item.Period.ResetAt))
        {
            var hiddenNames = BuildHiddenModelNames(
                item.Samples,
                options.ShowModels,
                options.ShowSol,
                options.ShowTerra,
                options.ShowLuna,
                options.ShowAstra);
            var key = new WindowProjectionCacheKey(
                item.Period.Id,
                item.Period.ResetAt,
                options.Metric,
                string.Join("\u001f", hiddenNames.OrderBy(name => name, StringComparer.Ordinal)),
                FormatOwnershipKey(options.AccountOwnershipIntervals));
            GraphProjection projection;
            if (priorCache.TryGetValue(key, out var cached) && ReferenceEquals(cached.Data, item))
            {
                projection = cached.Projection;
            }
            else
            {
                var period = item.Period with { Samples = item.Samples };
                projection = BuildProjection(
                    period,
                    options.Metric,
                    item.Gaps.Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt)).ToArray(),
                    hiddenNames,
                    options.AccountOwnershipIntervals,
                    appendPeriodTail: false);
            }

            nextCache[key] = new CachedWindowProjection(item, projection);
            foreach (var point in projection.Points)
            {
                if (point.Timestamp >= bounds.StartAt && point.Timestamp <= bounds.EndAt)
                {
                    points.Add(point);
                }
            }
            if (projection.Scene.HasPoints)
            {
                children.Add(projection.Scene);
            }
        }

        var viewport = GraphScene.CreateViewport(bounds.StartAt, bounds.EndAt, options.Metric, children);
        return new WindowGraphProjection(
            points.OrderBy(point => point.Timestamp).ToArray(),
            viewport,
            nextCache);
    }

    private static IReadOnlySet<string> BuildHiddenModelNames(
        IReadOnlyList<ApiHistorySample> samples,
        bool showModels,
        bool showSol,
        bool showTerra,
        bool showLuna,
        bool showAstra)
    {
        var hidden = new HashSet<string>(StringComparer.Ordinal);
        if (!showModels)
        {
            foreach (var model in samples.SelectMany(sample => sample.Models))
            {
                hidden.Add(model.Name);
            }
        }
        if (!showSol) hidden.Add("SOL");
        if (!showTerra) hidden.Add("TERRA");
        if (!showLuna) hidden.Add("LUNA");
        if (!showAstra) hidden.Add("ASTRA");
        return hidden;
    }

    private static string FormatOwnershipKey(IReadOnlyList<GraphAccountOwnershipInterval>? intervals) =>
        intervals is null
            ? string.Empty
            : string.Join(";", intervals.Select(interval => $"{interval.StartAt?.ToString(CultureInfo.InvariantCulture) ?? "_"}:{interval.EndAt?.ToString(CultureInfo.InvariantCulture) ?? "_"}"));

    private void PublishTimeWindow(
        GraphTimeRange range,
        long? pinnedEndAt,
        long? navigationOriginAt,
        long revision,
        IReadOnlyList<GraphWindowPeriodData> data,
        IReadOnlyList<ApiHistoryPeriod> directory,
        PublishedPairIdentity? pair,
        WindowGraphProjection projection,
        string? accountId,
        long accountGeneration)
    {
        postToUi(() => PublishTimeWindowOnUi(
            range,
            pinnedEndAt,
            navigationOriginAt,
            revision,
            data,
            directory,
            pair,
            projection,
            accountId,
            accountGeneration));
    }

    private void PublishTimeWindowOnUi(
        GraphTimeRange range,
        long? pinnedEndAt,
        long? navigationOriginAt,
        long revision,
        IReadOnlyList<GraphWindowPeriodData> data,
        IReadOnlyList<ApiHistoryPeriod> directory,
        PublishedPairIdentity? pair,
        WindowGraphProjection projection,
        string? accountId,
        long accountGeneration)
    {
        if (disposed || revision != Interlocked.Read(ref timeWindowRevision) ||
            accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
        {
            return;
        }

        if (resourceClient is not null)
        {
            var nextData = pair == windowPublishedPair
                ? windowPeriodData.ToDictionary(item => item.Key, item => item.Value, StringComparer.Ordinal)
                : new Dictionary<string, GraphWindowPeriodData>(StringComparer.Ordinal);
            foreach (var item in data)
            {
                nextData[item.Period.Id] = item;
            }
            windowPeriodData = nextData;
            windowPeriodDirectory = directory.ToArray();
            windowPublishedPair = pair;
        }
        else
        {
            windowPeriodDirectory = directory.ToArray();
        }

        windowProjectionCache = projection.Cache;
        Volatile.Write(ref selectedTimeRangeValue, (int)range);
        pinnedWindowEndAt = pinnedEndAt ?? long.MinValue;
        windowNavigationOriginAt = pinnedEndAt is null ? null : navigationOriginAt;
        timeWindowRequestPending = false;
        if (timeWindowRequestCancellation is not null && revision == Interlocked.Read(ref timeWindowRevision))
        {
            timeWindowRequestCancellation.Dispose();
            timeWindowRequestCancellation = null;
        }
        PublishWindowGraphProjection(projection, range, pinnedEndAt);
        NotifyTimeRangeProperties();
        DrainDeferredStaticDetailsRebuild();
    }

    private void PublishWindowProjection(
        WindowGraphProjection projection,
        GraphTimeRange range,
        long? pinnedEndAt,
        IReadOnlyList<GraphWindowPeriodData> data,
        IReadOnlyList<ApiHistoryPeriod> directory,
        PublishedPairIdentity? pair,
        string? accountId,
        long accountGeneration,
        long rangeRevision)
    {
        if (disposed || rangeRevision != Interlocked.Read(ref timeWindowRevision) ||
            accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
        {
            return;
        }

        windowProjectionCache = projection.Cache;
        PublishWindowGraphProjection(projection, range, pinnedEndAt);
        NotifyTimeRangeProperties();
        DrainDeferredStaticDetailsRebuild();
    }

    private void PublishWindowGraphProjection(
        WindowGraphProjection projection,
        GraphTimeRange range,
        long? pinnedEndAt)
    {
        points = projection.Points;
        scene = projection.Scene;
        displayedMetric = projection.Scene.Metric;
        displayedPeriod = selectedPeriod;
        Volatile.Write(ref selectedTimeRangeValue, (int)range);
        this.pinnedWindowEndAt = pinnedEndAt ?? long.MinValue;
        SetLoadError(false);
        SetLoading(false);
        Notify(nameof(Points));
        Notify(nameof(Scene));
        Notify(nameof(HasPoints));
        Notify(nameof(HasNoPoints));
        Notify(nameof(HasBlockingLoadError));
        Notify(nameof(HasPlot));
        Notify(nameof(MetricAxisText));
        Notify(nameof(IsDollars));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
        Notify(nameof(RangeLabel));
    }

    private void PublishTimeWindowFailure(long revision, string? accountId, long accountGeneration)
    {
        postToUi(() =>
        {
            if (disposed || revision != Interlocked.Read(ref timeWindowRevision) ||
                accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return;
            }

            timeWindowRequestPending = false;
            if (timeWindowRequestCancellation is not null)
            {
                timeWindowRequestCancellation.Dispose();
                timeWindowRequestCancellation = null;
            }
            SetLoadError(true);
            SetLoading(false);
            NotifyTimeRangeProperties();
            DrainDeferredStaticDetailsRebuild();
        });
    }

    private void NotifyTimeRangeProperties()
    {
        Notify(nameof(SelectedTimeRange));
        Notify(nameof(IsPeriodView));
        Notify(nameof(Is24HourView));
        Notify(nameof(IsWeekView));
        Notify(nameof(CanGoBack));
        Notify(nameof(CanGoForward));
        Notify(nameof(HasPlot));
        Notify(nameof(RangeLabel));
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
        CancellationToken cancellationToken)
    {
        if (timeWindowRequestPending)
        {
            return;
        }

        var range = SelectedTimeRange;
        if (range != GraphTimeRange.ResetPeriod)
        {
            var now = getUnixTimeSeconds();
            var pinnedEndAt = this.pinnedWindowEndAt == long.MinValue
                ? (long?)null
                : this.pinnedWindowEndAt;
            await RefreshTimeWindowCoreAsync(
                    range,
                    GraphTimeWindow.GetBounds(range, now, pinnedEndAt),
                    pinnedEndAt,
                    windowNavigationOriginAt,
                    Interlocked.Read(ref timeWindowRevision),
                    forceRefresh: true,
                    explicitRequest: false,
                    cancellationToken)
                .ConfigureAwait(false);
            return;
        }

        await RefreshSplitResourceCoreAsync(
                initial,
                requestedPeriodId,
                Interlocked.Read(ref periodSelectionRevision),
                cancellationToken)
            .ConfigureAwait(false);
    }

    private async Task RefreshTimeWindowCoreAsync(
        GraphTimeRange range,
        GraphTimeBounds bounds,
        long? pinnedEndAt,
        long? navigationOriginAt,
        long revision,
        bool forceRefresh,
        bool explicitRequest,
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

        string? accountId = null;
        var accountGeneration = 0L;
        var pageBudget = new WindowPageBudget();
        try
        {
            accountId = main.SelectedAccountId;
            accountGeneration = main.AccountSelectionGeneration;
            if (accountResourceClient is not null && accountId is null ||
                accountResourceClient is null && main.HasAccounts)
            {
                if (explicitRequest)
                {
                    PublishTimeWindowFailure(revision, accountId, accountGeneration);
                }
                return;
            }

            var allowCached = !forceRefresh && windowPublishedPair is not null;
            for (var alignmentAttempt = 0;
                alignmentAttempt < MaxSplitGenerationAlignmentAttempts;
                alignmentAttempt++)
            {
                if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                {
                    return;
                }

                var periodsResult = accountId is not null && accountResourceClient is not null
                    ? await accountResourceClient.FetchHistoryPeriodsAsync(accountId, cancellationToken).ConfigureAwait(false)
                    : await resourceClient.FetchHistoryPeriodsAsync(cancellationToken).ConfigureAwait(false);
                if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                {
                    return;
                }
                if (!periodsResult.IsSuccess || periodsResult.Snapshot is not { } periodsSnapshot ||
                    accountId is not null && periodsSnapshot.AccountId != accountId)
                {
                    PublishTimeWindowFailure(revision, accountId, accountGeneration);
                    return;
                }

                var sameAcceptedPair = periodsSnapshot.PublishedPair == windowPublishedPair;
                var priorPairData = sameAcceptedPair
                    ? windowPeriodData
                    : new Dictionary<string, GraphWindowPeriodData>(StringComparer.Ordinal);
                var reusable = allowCached && sameAcceptedPair
                    ? priorPairData
                    : new Dictionary<string, GraphWindowPeriodData>(StringComparer.Ordinal);
                var candidateData = new List<GraphWindowPeriodData>();
                var needsRealignment = false;
                foreach (var period in periodsSnapshot.Periods
                    .Where(period => Intersects(period, bounds))
                    .OrderBy(period => period.StartAt)
                    .ThenBy(period => period.ResetAt))
                {
                    if (reusable.TryGetValue(period.Id, out var cached) && SamePeriodBounds(cached.Period, period))
                    {
                        candidateData.Add(cached);
                        continue;
                    }
                    if (sameAcceptedPair && priorPairData.TryGetValue(period.Id, out cached) &&
                        !SamePeriodBounds(cached.Period, period))
                    {
                        PublishTimeWindowFailure(revision, accountId, accountGeneration);
                        return;
                    }

                    var fetched = await FetchWindowPeriodAsync(
                            period,
                            periodsSnapshot.PublishedPair,
                            accountId,
                            accountGeneration,
                            pageBudget,
                            cancellationToken)
                        .ConfigureAwait(false);
                    if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                    {
                        return;
                    }

                    if (fetched.Status == WindowPeriodFetchStatus.PairMismatch)
                    {
                        needsRealignment = true;
                        break;
                    }
                    if (fetched.Status != WindowPeriodFetchStatus.Success || fetched.Data is null)
                    {
                        PublishTimeWindowFailure(revision, accountId, accountGeneration);
                        return;
                    }
                    if (sameAcceptedPair && priorPairData.TryGetValue(period.Id, out cached))
                    {
                        if (!AreWindowPeriodDataEqual(cached, fetched.Data))
                        {
                            PublishTimeWindowFailure(revision, accountId, accountGeneration);
                            return;
                        }
                        candidateData.Add(cached);
                    }
                    else
                    {
                        candidateData.Add(fetched.Data);
                    }
                }

                if (needsRealignment)
                {
                    if (alignmentAttempt + 1 >= MaxSplitGenerationAlignmentAttempts)
                    {
                        PublishTimeWindowFailure(revision, accountId, accountGeneration);
                        return;
                    }
                    // A published pair advanced mid-candidate. Discard every
                    // staged period and make one complete candidate from a
                    // newly fetched periods root.
                    allowCached = false;
                    continue;
                }

                if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                {
                    return;
                }

                var buildSnapshot = await CaptureWindowBuildSnapshotAsync(cancellationToken).ConfigureAwait(false);
                if (!IsCurrentWindowRequest(revision, accountId, accountGeneration, explicitRequest))
                {
                    return;
                }

                BuildAndPublishWindowCandidate(
                    range,
                    bounds,
                    pinnedEndAt,
                    navigationOriginAt,
                    revision,
                    explicitRequest,
                    candidateData,
                    periodsSnapshot.Periods,
                    periodsSnapshot.PublishedPair,
                    accountId,
                    accountGeneration,
                    buildSnapshot);
                return;
            }

            PublishTimeWindowFailure(revision, accountId, accountGeneration);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // A newer range, account boundary, or graph close owns cancellation.
        }
        catch
        {
            PublishTimeWindowFailure(revision, accountId, accountGeneration);
        }
        finally
        {
            resourceRefreshGate.Release();
        }
    }

    private async Task<WindowPeriodFetchResult> FetchWindowPeriodAsync(
        ApiHistoryPeriod period,
        PublishedPairIdentity expectedPair,
        string? accountId,
        long accountGeneration,
        WindowPageBudget pageBudget,
        CancellationToken cancellationToken)
    {
        var samples = new List<ApiHistorySample>();
        var gaps = new List<ApiHistoryGap>();
        string? cursor = null;
        string? requestedCursor = null;
        var firstPage = true;
        while (true)
        {
            if (++pageBudget.PageRequests > MaxSplitHistoryPageRequests)
            {
                return WindowPeriodFetchResult.Failure;
            }

            var pageResult = accountId is not null && accountResourceClient is not null
                ? await accountResourceClient.FetchHistoryPageAsync(
                        accountId,
                        period.Id,
                        cursor,
                        cancellationToken)
                    .ConfigureAwait(false)
                : await resourceClient!.FetchHistoryPageAsync(
                        period.Id,
                        cursor,
                        cancellationToken)
                    .ConfigureAwait(false);
            if (accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration))
            {
                return WindowPeriodFetchResult.Stale;
            }
            if (!pageResult.IsSuccess || pageResult.Page is not { } page ||
                accountId is not null && page.AccountId != accountId ||
                page.PeriodId != period.Id)
            {
                return WindowPeriodFetchResult.Failure;
            }
            if (page.PublishedPair != expectedPair)
            {
                return WindowPeriodFetchResult.PairMismatch;
            }
            if (!ValidateHistoryPage(period, page) || (!firstPage && page.HistoryGaps.Count != 0))
            {
                return WindowPeriodFetchResult.Failure;
            }

            samples.AddRange(page.Samples);
            if (firstPage)
            {
                gaps.AddRange(page.HistoryGaps);
            }
            firstPage = false;
            if (page.NextCursor is null)
            {
                var mergedSamples = MergeHistorySamples(Array.Empty<ApiHistorySample>(), samples);
                var mergedGaps = MergeHistoryGaps(Array.Empty<ApiHistoryGap>(), gaps);
                if (mergedSamples is null || mergedGaps is null)
                {
                    return WindowPeriodFetchResult.Failure;
                }

                return WindowPeriodFetchResult.Success(new GraphWindowPeriodData(
                    period with { Samples = mergedSamples },
                    mergedSamples,
                    mergedGaps));
            }

            if (page.NextCursor == cursor || page.NextCursor == requestedCursor && samples.Count == 0)
            {
                return WindowPeriodFetchResult.Failure;
            }
            requestedCursor = cursor = page.NextCursor;
        }
    }

    private bool IsCurrentWindowRequest(
        long revision,
        string? accountId,
        long accountGeneration,
        bool explicitRequest) =>
        !disposed && revision == Interlocked.Read(ref timeWindowRevision) &&
        (accountId is null || main.IsAccountSelectionCurrent(accountId, accountGeneration)) &&
        (explicitRequest || !timeWindowRequestPending);

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
                Notify(nameof(SelectedPeriodValueText));
                Notify(nameof(SelectedPeriodStartAt));
                Notify(nameof(SelectedPeriodEndAt));
                NotifyTimeRangeProperties();
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
            if (resetRangeCommitRevision == Interlocked.Read(ref timeWindowRevision))
            {
                resetRangeCommitRevision = -1;
                timeWindowRequestPending = false;
            }
            SetLoadError(true);
            SetLoading(false);
            NotifyTimeRangeProperties();
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
        if (resourceClient is null && timeWindowRequestPending)
        {
            staticDetailsRebuildDeferred = true;
            return;
        }

        var previousId = selectedPeriod?.Id;
        rebuildingPeriodDirectory = true;
        try
        {
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
        }
        finally
        {
            rebuildingPeriodDirectory = false;
        }

        windowProjectionCache.Clear();
        if (resourceClient is null)
        {
            Interlocked.Increment(ref timeWindowRevision);
            timeWindowRequestPending = false;
            staticWindowPeriodData = BuildStaticWindowPeriodData(periods);
            windowPeriodDirectory = periods.ToArray();
        }

        RebuildPoints();
        Notify(nameof(HasPeriods));
        Notify(nameof(SelectedPeriod));
        Notify(nameof(SelectedPeriodText));
        Notify(nameof(SelectedPeriodStartAt));
        Notify(nameof(SelectedPeriodEndAt));
        NotifyTimeRangeProperties();
    }

    private void DrainDeferredStaticDetailsRebuild()
    {
        if (resourceClient is not null || timeWindowRequestPending || !staticDetailsRebuildDeferred)
        {
            return;
        }

        staticDetailsRebuildDeferred = false;
        Rebuild();
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
        // CORE has already projected the current quota-window StartAt. The
        // selector, Scene and X-axis consume that accepted bound unchanged.
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
        var resetRangePending = resetRangeCommitRevision == Interlocked.Read(ref timeWindowRevision);
        if (SelectedTimeRange != GraphTimeRange.ResetPeriod && !resetRangePending)
        {
            RebuildAcceptedWindowProjection();
            return;
        }

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
        var accountOwnershipIntervals = main.SelectedAccount?.OwnershipIntervals?
            .Select(interval => new GraphAccountOwnershipInterval(interval.StartAt, interval.EndAt))
            .ToArray();
        if (sourceCount <= BackgroundBuildThreshold)
        {
            try
            {
                PublishPoints(
                    BuildProjection(period, metric, confirmedGaps, hiddenModelNames, accountOwnershipIntervals),
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
                return BuildProjection(period, metric, confirmedGaps, hiddenModelNames, accountOwnershipIntervals);
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
        IReadOnlySet<string> hiddenModelNames,
        IReadOnlyList<GraphAccountOwnershipInterval>? accountOwnershipIntervals,
        bool appendPeriodTail = true)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var samples = appendPeriodTail
            ? BuildGraphSamples(period, now)
            : BuildWindowGraphSamples(period);
        var diagnosticSamples = ReduceGraphSamples(samples, MaxRenderedGraphPoints, confirmedGaps);
        var graphScene = GraphScene.Create(
            samples,
            metric,
            period.StartAt,
            EffectiveGraphEnd(period, now),
            confirmedGaps,
            hiddenModelNames,
            accountOwnershipIntervals,
            period.ResetAt);
        GraphPlotProjection.PrepareGeometry(graphScene);
        return new GraphProjection(
            diagnosticSamples.Select(sample => new GraphPointViewModel(sample, metric)).ToArray(),
            graphScene);
    }

    private static IReadOnlyList<ApiHistorySample> BuildWindowGraphSamples(ApiHistoryPeriod period) =>
        period.Samples
            .Where(sample => !sample.IsSyntheticTail &&
                sample.Timestamp >= period.StartAt && sample.Timestamp <= period.EndAt)
            .OrderBy(sample => sample.Timestamp)
            .ToArray();

    private void PublishPoints(
        GraphProjection next,
        ApiHistoryPeriod? period,
        GraphMetric metric)
    {
        var commitsResetRange = resetRangeCommitRevision == Interlocked.Read(ref timeWindowRevision);
        points = next.Points;
        scene = next.Scene;
        displayedPeriod = period;
        displayedMetric = metric;
        if (commitsResetRange)
        {
            resetRangeCommitRevision = -1;
            timeWindowRequestPending = false;
            Volatile.Write(ref selectedTimeRangeValue, (int)GraphTimeRange.ResetPeriod);
            pinnedWindowEndAt = long.MinValue;
            windowNavigationOriginAt = null;
        }
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
        Notify(nameof(HasPlot));
        Notify(nameof(RangeLabel));
        if (commitsResetRange)
        {
            NotifyTimeRangeProperties();
            DrainDeferredStaticDetailsRebuild();
        }
        Notify(nameof(CanGoBack));
        Notify(nameof(CanGoForward));
    }

    private readonly record struct GraphProjection(
        IReadOnlyList<GraphPointViewModel> Points,
        GraphScene Scene);

    private sealed record GraphWindowPeriodData(
        ApiHistoryPeriod Period,
        IReadOnlyList<ApiHistorySample> Samples,
        IReadOnlyList<ApiHistoryGap> Gaps);

    private sealed record WindowBuildOptions(
        GraphMetric Metric,
        bool ShowModels,
        bool ShowSol,
        bool ShowTerra,
        bool ShowLuna,
        bool ShowAstra,
        IReadOnlyList<GraphAccountOwnershipInterval>? AccountOwnershipIntervals);

    private sealed record WindowBuildSnapshot(
        WindowBuildOptions Options,
        Dictionary<WindowProjectionCacheKey, CachedWindowProjection> Cache,
        long PointBuildRevision);

    private readonly record struct WindowProjectionCacheKey(
        string PeriodId,
        long ResetAt,
        GraphMetric Metric,
        string HiddenModelNames,
        string OwnershipIntervals);

    private sealed record CachedWindowProjection(
        GraphWindowPeriodData Data,
        GraphProjection Projection);

    private sealed record WindowGraphProjection(
        IReadOnlyList<GraphPointViewModel> Points,
        GraphScene Scene,
        Dictionary<WindowProjectionCacheKey, CachedWindowProjection> Cache);

    private enum WindowPeriodFetchStatus
    {
        Success,
        PairMismatch,
        Failure,
        Stale,
    }

    private sealed record WindowPeriodFetchResult(
        WindowPeriodFetchStatus Status,
        GraphWindowPeriodData? Data)
    {
        public static WindowPeriodFetchResult Failure { get; } = new(WindowPeriodFetchStatus.Failure, null);

        public static WindowPeriodFetchResult PairMismatch { get; } = new(WindowPeriodFetchStatus.PairMismatch, null);

        public static WindowPeriodFetchResult Stale { get; } = new(WindowPeriodFetchStatus.Stale, null);

        public static WindowPeriodFetchResult Success(GraphWindowPeriodData data) =>
            new(WindowPeriodFetchStatus.Success, data);
    }

    private sealed class WindowPageBudget
    {
        public int PageRequests { get; set; }
    }

    private void PublishLoadFailure(long revision)
    {
        if (disposed || revision != pointBuildRevision)
        {
            return;
        }
        var resetRangeFailed = resetRangeCommitRevision == Interlocked.Read(ref timeWindowRevision);
        if (resetRangeFailed)
        {
            resetRangeCommitRevision = -1;
            timeWindowRequestPending = false;
        }
        SetLoadError(true);
        SetLoading(false);
        if (resetRangeFailed)
        {
            NotifyTimeRangeProperties();
            DrainDeferredStaticDetailsRebuild();
        }
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
        ThemePalette.Changed += OnThemeChanged;
        resourceThreads = main.DetailsSnapshot?.Threads ?? Array.Empty<ApiThreadDetails>();
        Rebuild();
        if (resourceClient is not null)
        {
            resourcePollingCancellation = new CancellationTokenSource();
            _ = RunSplitResourcePollingAsync(resourcePollingCancellation.Token);
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ThreadItemViewModel> Threads { get; }

    public IReadOnlyList<ThreadTreeConnection> TreeConnections { get; private set; } = Array.Empty<ThreadTreeConnection>();

    public IReadOnlyList<int> TreeRootRows { get; private set; } = Array.Empty<int>();

    public int TreeSurfaceHeight => Math.Max(96, threads.Count * 96);

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
        ThemePalette.Changed -= OnThemeChanged;
    }

    private void OnThemeChanged(object? sender, EventArgs eventArgs)
    {
        if (!disposed) Rebuild();
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
                TreeConnections = Array.Empty<ThreadTreeConnection>();
                TreeRootRows = Array.Empty<int>();
                hasLoadError = false;
                Notify(nameof(HasThreads));
                Notify(nameof(HasNoThreads));
                Notify(nameof(TreeConnections));
                Notify(nameof(TreeRootRows));
                Notify(nameof(TreeSurfaceHeight));
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
            if (eventArgs.PropertyName == nameof(MainWindowViewModel.DetailsSnapshot))
            {
                resourceThreads = main.DetailsSnapshot?.Threads ?? Array.Empty<ApiThreadDetails>();
                hasLoadError = false;
                Notify(nameof(HasLoadError));
                Rebuild();
            }
            else if (resourceClient is null)
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
        var mainGeneration = main.DetailsSnapshot;
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
                accountId is not null && !main.IsAccountSelectionCurrent(accountId, accountGeneration) ||
                !ReferenceEquals(main.DetailsSnapshot, mainGeneration))
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

            if (mainGeneration is null || snapshot.PublishedPair != mainGeneration.PublishedPair ||
                (ulong)snapshot.Threads.Count != main.AcceptedWireOpenSessionThreadCount ||
                (ulong)snapshot.Threads.Count(thread =>
                    thread.ActivityStatus == ApiThreadActivityStatus.Running) != main.AcceptedWireActiveThreadCount)
            {
                // Only Main's accepted pair can replace the visible detail rows.
                // A newer independent response waits for Main's next atomic update.
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
        var connections = new List<ThreadTreeConnection>();
        var rootRows = new List<int>();
        var source = main.IsSelectedAccountHistorical
            ? Array.Empty<ApiThreadDetails>()
            : resourceClient is not null
            ? resourceThreads
            : main.DetailsSnapshot?.Threads ?? Array.Empty<ApiThreadDetails>();
        source = MainWindowViewModel.WithoutStoppedParentSubtrees(source);
        if (source.Count > 0)
        {
            var ordered = ParentFirst(source);
            var byId = source.ToDictionary(thread => thread.Id, StringComparer.Ordinal);
            var rowById = ordered
                .Select((thread, row) => (thread.Id, row))
                .ToDictionary(item => item.Id, item => item.row, StringComparer.Ordinal);
            var acceptedParentIds = AcceptedParentIds(source);
            var displayDepthById = new Dictionary<string, int>(StringComparer.Ordinal);
            for (var index = 0; index < ordered.Count; index++)
            {
                var thread = ordered[index];
                var parentExists = thread.ParentId is { } parentId && byId.ContainsKey(parentId);
                var displayDepth = 0;
                var hasValidParent = parentExists && !thread.IsOrphan;
                if (hasValidParent && thread.ParentId is { } connectionParentId && rowById.TryGetValue(connectionParentId, out var parentRow))
                {
                    displayDepth = Math.Min(displayDepthById.GetValueOrDefault(connectionParentId) + 1, 32);
                    connections.Add(new ThreadTreeConnection(parentRow, index, displayDepth - 1));
                }
                else
                {
                    rootRows.Add(index);
                }
                displayDepthById[thread.Id] = displayDepth;
                var parentTitle = thread.ParentId is { } id && byId.TryGetValue(id, out var parent)
                    ? parent.Title
                    : string.Empty;
                threads.Add(new ThreadItemViewModel(this, thread, Math.Min(displayDepth, 3), parentExists && !thread.IsOrphan,
                    parentTitle, acceptedParentIds.Contains(thread.Id)));
            }
        }

        TreeConnections = connections.AsReadOnly();
        TreeRootRows = rootRows.AsReadOnly();

        Notify(nameof(HasThreads));
        Notify(nameof(HasNoThreads));
        Notify(nameof(TreeConnections));
        Notify(nameof(TreeRootRows));
        Notify(nameof(TreeSurfaceHeight));
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
        var roots = source.Where(thread => thread.ParentId is null || !byId.ContainsKey(thread.ParentId)).ToArray();
        foreach (var root in roots.Where(thread => thread.ActivityStatus == ApiThreadActivityStatus.Running)) Visit(root);
        foreach (var root in roots.Where(thread => thread.ActivityStatus != ApiThreadActivityStatus.Running)) Visit(root);
        foreach (var item in source) Visit(item);
        return result;
    }

    private static IReadOnlySet<string> AcceptedParentIds(IReadOnlyList<ApiThreadDetails> source)
    {
        var ids = source.Select(thread => thread.Id).ToHashSet(StringComparer.Ordinal);
        return source
            .Where(thread => thread.ParentId is { } parentId && !thread.IsOrphan && ids.Contains(parentId))
            .Select(thread => thread.ParentId!)
            .ToHashSet(StringComparer.Ordinal);
    }

    private void Notify([CallerMemberName] string? propertyName = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}

public sealed class ThreadItemViewModel
{
    public ThreadItemViewModel(ThreadsWindowViewModel owner, ApiThreadDetails thread, int treeDepth,
        bool connectedToParent, string parentTitle, bool isParent)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        Id = thread.Id;
        Title = FormatThreadTitle(owner.Texts, thread);
        // Kept for non-rendered compatibility with existing presentation
        // coverage. ThreadsWindow no longer binds this diagnostic role value.
        RoleText = thread.IsSubAgent ? owner.Texts.SubThread : owner.Texts.MainThread;
        ParentText = string.IsNullOrWhiteSpace(parentTitle)
            ? owner.ParentText(thread)
            : $"{owner.ParentText(thread)} / {parentTitle}";
        ModelText = owner.ModelText(thread);
        ActivityStatusText = thread.ActivityStatus switch
        {
            ApiThreadActivityStatus.Running => owner.Texts.ThreadRunning,
            ApiThreadActivityStatus.Stopped => owner.Texts.ThreadStopped,
            _ => owner.Texts.ThreadUnknown,
        };
        ActivityStatusHex = ThemePalette.Resolve(thread.ActivityStatus == ApiThreadActivityStatus.Running
            ? "#EF6A6A"
            : "#A8B7CA");
        ModelAccentHex = ThemePalette.Resolve(FormatModelAccent(ModelText));
        ContextText = owner.ContextText(thread);
        ContextUsageText = FormatContextUsage(owner.Texts, thread);
        TokenText = owner.TokenText(thread);
        DisplayTokenText = FormatDisplayToken(owner.Texts, thread);
        DepthText = thread.Depth is { } depth ? $"{owner.Texts.Depth} {depth}" : $"{owner.Texts.Depth} —";
        AgeText = owner.Texts.FormatElapsed(thread.CreatedAt, owner.Texts.Elapsed);
        InstructionAgeText = owner.Texts.FormatElapsed(thread.LastUserMessageAt, owner.Texts.Instruction);
        ElapsedMinutesText = FormatMinuteAge(owner.Texts, thread.CreatedAt, owner.Texts.Elapsed, now);
        InstructionMinutesText = FormatMinuteAge(owner.Texts, thread.LastUserMessageAt, owner.Texts.Instruction, now);
        TreeDepth = treeDepth;
        ConnectedToParent = connectedToParent;
        ParentTitle = parentTitle;
        IsParent = isParent;
        CardBackgroundHex = ThemePalette.Resolve(FormatCardBackground(isParent));
        IsRootThread = !connectedToParent && !thread.IsOrphan;
    }

    public string Id { get; }
    public string Title { get; }
    public string RoleText { get; }
    public string ParentText { get; }
    public string ModelText { get; }
    public string ActivityStatusText { get; }
    public string ActivityStatusHex { get; }
    public string ModelAccentHex { get; }
    public string ContextText { get; }
    public string ContextUsageText { get; }
    public bool HasContextUsage => ContextUsageText.Length > 0;
    public string TokenText { get; }
    public string DisplayTokenText { get; }
    public bool HasDisplayToken => DisplayTokenText.Length > 0;
    public string DepthText { get; }
    public string AgeText { get; }
    public string InstructionAgeText { get; }
    public string ElapsedMinutesText { get; }
    public bool HasElapsedMinutes => ElapsedMinutesText.Length > 0;
    public string InstructionMinutesText { get; }
    public bool HasInstructionMinutes => InstructionMinutesText.Length > 0;
    public int TreeDepth { get; }
    public bool ConnectedToParent { get; }
    public string ParentTitle { get; }
    public bool IsParent { get; }
    public string CardBackgroundHex { get; }
    public bool IsRootThread { get; }

    internal static string FormatThreadTitle(UiText texts, ApiThreadDetails thread) =>
        string.IsNullOrWhiteSpace(thread.Title) ? texts.ThreadNameUnset : thread.Title;

    internal static string FormatCardBackground(bool isParent) => isParent ? "#243E5A" : "#151F2D";

    internal static string FormatModelAccent(string model)
    {
        var normalized = model.ToUpperInvariant();
        if (normalized.Contains("ASTRA", StringComparison.Ordinal))
        {
            return "#E86E9F";
        }

        if (normalized.Contains("LUNA", StringComparison.Ordinal))
        {
            return "#F1B35A";
        }

        if (normalized.Contains("TERRA", StringComparison.Ordinal))
        {
            return "#71D39A";
        }

        if (normalized.Contains("SOL", StringComparison.Ordinal))
        {
            return "#B79BFF";
        }

        return "#A8B7CA";
    }

    internal static string FormatContextUsage(UiText texts, ApiThreadDetails thread)
    {
        if (thread.ContextTokens is not { } used ||
            thread.ContextLimit is not { } limit ||
            limit == 0)
        {
            return $"{texts.Context} {texts.ContextUnobserved}";
        }

        var percent = thread.ContextPercent is { } observed && double.IsFinite(observed)
            ? observed
            : (double)used / limit * 100d;

        return string.Create(
            CultureInfo.CurrentCulture,
            $"{texts.Context} {percent:0.##}%\n{used:N0} / {limit:N0} {texts.Tokens}");
    }

    internal static string FormatMinuteAge(UiText texts, long? timestamp, string label, long now)
    {
        if (timestamp is not { } value)
        {
            return string.Empty;
        }

        var minutes = Math.Max(0, now - value) / 60;
        var unit = texts.LanguageCode switch
        {
            "ja" => "分",
            "zh-Hans" => "分钟",
            "ko" => "분",
            "de" => "Min.",
            "ru" => "мин",
            _ => "min",
        };
        return string.Create(CultureInfo.CurrentCulture, $"{label} {minutes:N0} {unit}");
    }

    internal static string FormatDisplayToken(UiText texts, ApiThreadDetails thread) =>
        thread.CumulativeTokens is { } tokens
            ? string.Create(CultureInfo.CurrentCulture, $"{texts.Tokens} {tokens:N0}")
            : string.Empty;

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
