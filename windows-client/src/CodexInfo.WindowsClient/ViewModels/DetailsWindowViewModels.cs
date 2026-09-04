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

    public GraphPointViewModel(ApiHistorySample sample, GraphMetric metric)
    {
        this.metric = metric;
        Timestamp = sample.Timestamp;
        TimestampText = TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(sample.Timestamp), LocalizationService.DisplayTimeZone)
            .ToString("g", CultureInfo.CurrentCulture);
        RemainingPercent = sample.RemainingPercent;

        foreach (var model in sample.Models)
        {
            var value = metric == GraphMetric.Dollars
                ? model.Dollars
                : model.InputTokens + model.CachedInputTokens + model.OutputTokens;
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
            }
        }
    }

    public long Timestamp { get; }

    public string TimestampText { get; }

    public double? RemainingPercent { get; }

    public double SolValue { get; }

    public double TerraValue { get; }

    public double LunaValue { get; }

    public string RemainingText => RemainingPercent is { } value
        ? string.Create(CultureInfo.CurrentCulture, $"{LocalizationService.Current.RemainingQuota} {value:0.#}%")
        : $"{LocalizationService.Current.RemainingQuota} —";

    public string ModelsText => metric == GraphMetric.Dollars
        ? string.Create(
            CultureInfo.CurrentCulture,
            $"SOL ${SolValue:N2} / TERRA ${TerraValue:N2} / LUNA ${LunaValue:N2}")
        : string.Create(
            CultureInfo.CurrentCulture,
            $"SOL {SolValue:N0} / TERRA {TerraValue:N0} / LUNA {LunaValue:N0}");
}

public sealed class GraphWindowViewModel : INotifyPropertyChanged, IDisposable
{
    // The transport keeps the complete one-month (44,640 minute) history, but
    // a 940 logical-pixel graph cannot expose that many distinct x positions.
    // Keep at least one sample per physical plot pixel at 200% DPI (and more
    // at standard DPI) so paint cost is bounded without changing endpoints.
    internal const int MaxRenderedGraphPoints = 2_048;
    private const int BackgroundBuildThreshold = 2_048;
    private readonly MainWindowViewModel main;
    private readonly Action<Action> postToUi;
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
    private CancellationTokenSource pointBuildCancellation = new();
    private long pointBuildRevision;
    private bool isLoading;
    private bool hasLoadError;
    private bool disposed;

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
        Rebuild();
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ApiHistoryPeriod> Periods { get; }

    public IReadOnlyList<GraphPointViewModel> Points => points;

    public GraphScene Scene => scene;

    public UiText Texts => LocalizationService.Current;

    public IReadOnlyList<string> MetricOptions => metricOptions;

    public string SelectedMetric
    {
        get => selectedMetric == GraphMetric.Dollars ? Texts.Dollars : Texts.Tokens;
        set
        {
            var metric = value == Texts.Tokens ? GraphMetric.Tokens : GraphMetric.Dollars;
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

    public bool HasNoPoints => !HasPoints;

    public bool HasPeriods => periods.Count > 0;

    public string SelectedPeriodText => selectedPeriod?.Label ?? Texts.UnavailableValue;

    public long SelectedPeriodStartAt => scene.HasPoints ? scene.PeriodStartAt : displayedPeriod?.StartAt ?? 0;

    // The API keeps the canonical reset boundary in end_at so clients can
    // label the period consistently.  For the active period the X client
    // clips the plot's right edge to the observation time; using the future
    // reset boundary here leaves an empty tail and changes the graph meaning.
    public long SelectedPeriodEndAt => scene.HasPoints ? scene.PeriodEndAt : 0;

    internal static long EffectiveGraphEnd(ApiHistoryPeriod period, long now)
    {
        if (!period.Current)
        {
            return period.EndAt;
        }

        return Math.Max(period.StartAt, Math.Min(period.EndAt, now));
    }

    internal static IReadOnlyList<ApiHistorySample> BuildGraphSamples(ApiHistoryPeriod period, long now)
    {
        var end = EffectiveGraphEnd(period, now);
        var observed = period.Samples
            // Both current and historical periods own their effective-end
            // sample.  The active period is clipped to the observation time,
            // so rows after that endpoint remain excluded without dropping
            // the exact endpoint itself.
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
        // source vector as-is; graph code only adds documented quota/reset
        // anchors and never repairs model components from older rows.
        var normalized = observed.ToList();

        var hasRemainingObservation = normalized.Any(sample => sample.RemainingPercent is { } value && double.IsFinite(value));
        if (normalized[0].Timestamp == period.StartAt &&
            normalized[0].RemainingPercent is null &&
            hasRemainingObservation)
        {
            // raw_graph_points() always starts with a 100% reset anchor and
            // only replaces it when a quota observation exists at the same
            // timestamp.  Keep that distinction when the first model row is
            // present but its quota field is missing.
            normalized[0] = normalized[0] with { RemainingPercent = 100 };
        }
        var result = new List<ApiHistorySample>(normalized.Count + 2);
        if (normalized[0].Timestamp > period.StartAt)
        {
            result.Add(new ApiHistorySample(period.StartAt, normalized[0].ResetAt, hasRemainingObservation ? 100 : null, 0, 0, 0, 0, 0, 0));
        }

        result.AddRange(normalized);
        var last = result[^1];
        if (last.Timestamp < end && end - last.Timestamp <= 60)
        {
            // A recent local cumulative observation may be held only until
            // the next normal collection boundary.  The quota field is not
            // copied: the renderer owns the explicitly dashed last-known
            // projection.  A longer local-log outage must leave the model
            // path at its actual observation time.
            result.Add(last with { Timestamp = end, RemainingPercent = null });
        }

        return result;
    }

    internal static IReadOnlyList<ApiHistorySample> ReduceGraphSamples(
        IReadOnlyList<ApiHistorySample> samples,
        int maximum = MaxRenderedGraphPoints)
    {
        if (maximum < 2)
        {
            throw new ArgumentOutOfRangeException(nameof(maximum));
        }
        if (samples.Count <= maximum)
        {
            return samples;
        }

        // All graph series are cumulative and therefore monotonic. Keep both
        // edges of each display bucket so a short change is not lost or moved
        // to a later bucket, while bounding paint work by viewport resolution.
        var selected = new SortedSet<int> { 0, samples.Count - 1 };
        var bucketCount = Math.Max(1, maximum / 2);
        for (var bucket = 0; bucket < bucketCount; bucket++)
        {
            var start = (int)((long)bucket * samples.Count / bucketCount);
            var endExclusive = (int)((long)(bucket + 1) * samples.Count / bucketCount);
            selected.Add(start);
            selected.Add(Math.Max(start, endExclusive - 1));
        }
        // Odd/small caller-supplied maxima can leave one slot. Fill it with a
        // uniformly located sample without disturbing the bucket edges.
        for (var slot = 1; selected.Count < maximum && slot < maximum - 1; slot++)
        {
            selected.Add((int)Math.Round(
                slot * (samples.Count - 1d) / (maximum - 1d),
                MidpointRounding.AwayFromZero));
        }
        return selected.Take(maximum).Select(index => samples[index]).ToArray();
    }

    public string DetailsStatusText => main.DetailsStatusText;

    public bool IsLoading => isLoading;

    public bool HasLoadError => hasLoadError;

    public string LoadingText => Texts.GraphLoading;

    public string LoadErrorText => Texts.GraphLoadFailed;

    public string MetricAxisText => displayedMetric == GraphMetric.Dollars
        ? $"{Texts.Dollars} ({Texts.ModelUsage})"
        : $"{Texts.Tokens} ({Texts.ModelUsage})";

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
            Notify();
        }
    }

    public bool ShowSol { get => showSol; set { if (showSol == value) return; showSol = value; Notify(); } }
    public bool ShowTerra { get => showTerra; set { if (showTerra == value) return; showTerra = value; Notify(); } }
    public bool ShowLuna { get => showLuna; set { if (showLuna == value) return; showLuna = value; Notify(); } }

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }

        disposed = true;
        pointBuildCancellation.Cancel();
        pointBuildCancellation.Dispose();
        main.PropertyChanged -= OnMainPropertyChanged;
    }

    private void OnMainPropertyChanged(object? sender, PropertyChangedEventArgs eventArgs)
    {
        if (eventArgs.PropertyName == nameof(MainWindowViewModel.DetailsSnapshot))
        {
            Rebuild();
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
            RebuildPoints();
            Notify(nameof(Texts));
            Notify(nameof(MetricOptions));
            Notify(nameof(SelectedMetric));
            Notify(nameof(SelectedPeriodText));
            Notify(nameof(MetricAxisText));
            Notify(nameof(GraphGapHintText));
        }
    }

    private void RebuildMetricOptions()
    {
        metricOptions = [Texts.Dollars, Texts.Tokens];
    }

    private void Rebuild()
    {
        var previousId = selectedPeriod?.Id;
        periods.Clear();
        if (main.DetailsSnapshot is { } details)
        {
            foreach (var period in details.History)
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
        if (sourceCount <= BackgroundBuildThreshold)
        {
            try
            {
                PublishPoints(BuildProjection(period, metric, confirmedGaps), period, metric);
            }
            catch
            {
                PublishLoadFailure(revision);
            }
            return;
        }

        // Large history normalization/reduction never runs on the UI thread.
        // The previously painted graph and its axis remain intact while the
        // selected period is prepared. Only the final bounded immutable array
        // crosses back in one atomic publish.
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
                return BuildProjection(period, metric, confirmedGaps);
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
        return main.DetailsSnapshot?.HistoryGaps
                .Where(gap => gap.EndAt > period.StartAt && gap.StartAt < end)
                .Select(gap => new GraphConfirmedGap(gap.StartAt, gap.EndAt))
                .ToArray()
            ?? Array.Empty<GraphConfirmedGap>();
    }

    private static GraphProjection BuildProjection(
        ApiHistoryPeriod period,
        GraphMetric metric,
        IReadOnlyList<GraphConfirmedGap> confirmedGaps)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var samples = ReduceGraphSamples(BuildGraphSamples(period, now));
        return new GraphProjection(
            samples.Select(sample => new GraphPointViewModel(sample, metric)).ToArray(),
            GraphScene.Create(
                samples,
                metric,
                period.StartAt,
                EffectiveGraphEnd(period, now),
                confirmedGaps));
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
        SetLoading(false);
        SetLoadError(true);
    }

    private void SetLoading(bool value)
    {
        if (isLoading == value)
        {
            return;
        }
        isLoading = value;
        Notify(nameof(IsLoading));
    }

    private void SetLoadError(bool value)
    {
        if (hasLoadError == value)
        {
            return;
        }
        hasLoadError = value;
        Notify(nameof(HasLoadError));
    }

    private void Notify([CallerMemberName] string? propertyName = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }
}

public sealed class ThreadsWindowViewModel : INotifyPropertyChanged, IDisposable
{
    private readonly MainWindowViewModel main;
    private readonly ObservableCollection<ThreadItemViewModel> threads = [];
    private bool disposed;

    public ThreadsWindowViewModel(MainWindowViewModel main)
    {
        this.main = main;
        Threads = new ReadOnlyObservableCollection<ThreadItemViewModel>(threads);
        main.PropertyChanged += OnMainPropertyChanged;
        Rebuild();
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public ReadOnlyObservableCollection<ThreadItemViewModel> Threads { get; }

    public UiText Texts => LocalizationService.Current;

    public bool HasThreads => threads.Count > 0;

    public bool HasNoThreads => !HasThreads;

    public string EmptyText => Texts.NoRunningThreads;

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
        threads.Clear();
        if (main.DetailsSnapshot is { } details)
        {
            var ordered = ParentFirst(details.Threads);
            var byId = details.Threads.ToDictionary(thread => thread.Id, StringComparer.Ordinal);
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
