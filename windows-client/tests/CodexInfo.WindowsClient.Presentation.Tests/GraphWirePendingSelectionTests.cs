// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Diagnostics;
using System.Reflection;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Threading;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphWirePendingSelectionTests
{
    private const long Now = 1_800_003_600;
    private static readonly PublishedPairIdentity Image = PublishedPairIdentity.Create(new string('a', 64));

    [Fact]
    public async Task MainProjectionNotificationDoesNotEraseTheGraphsAcceptedAccountContext()
    {
        var client = new SelectionMainClient();
        using var main = new MainWindowViewModel(client);
        main.Start();
        await Eventually(() => main.DetailsSnapshot?.ObservedAt == Now);
        var context = new ApiAccount("account-1", true, Now - 600, null)
        {
            OwnershipIntervals = [new ApiAccountOwnershipInterval(Now - 600, null)],
        };
        main.ApplyAccountContext(context);
        var selectedChanges = 0;
        main.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(main.SelectedAccount)) selectedChanges++;
        };

        client.Notify();
        await Eventually(() => main.DetailsSnapshot?.ObservedAt == Now + 1);

        Assert.Equal(Now - 600, main.SelectedAccount!.ActivationAt);
        Assert.Equal(context.OwnershipIntervals, main.SelectedAccount.OwnershipIntervals);
        Assert.Equal(0, selectedChanges);
    }

    [Fact]
    public async Task PendingPeriodSelectionPublishesThroughTheBoundSelector()
    {
        using var session = HeadlessUnitTestSession.StartNew(typeof(GraphSelectInteractionTests.TestApplication));
        await session.Dispatch(() =>
        {
            var client = new PendingHistoryClient();
            using var main = new MainWindowViewModel(client);
            typeof(MainWindowViewModel).GetMethod("ApplyDetailsGeneration", BindingFlags.Instance | BindingFlags.NonPublic)!
                .Invoke(main, [new ApiDetailsSnapshot(ApiState.Ready, Now, true, "Pro", null, [], 0, [], [], [], "known") { PublishedPair = Image }]);
            using var graph = new GraphWindowViewModel(main, action => Dispatcher.UIThread.Post(action), () => Now);
            var window = new GraphWindow(graph);
            try
            {
                window.Show();
                PumpUntil(() => graph.SelectedPeriod?.Id == "current" && !graph.IsLoading);
                var selector = window.FindControl<GraphSelect>("PeriodSelector")!;
                var list = selector.FindControl<ListBox>("MenuList")!;
                var accepted = graph.Scene;
                list.SelectedItem = graph.Periods.Single(period => period.Id == "past");
                PumpUntil(() => client.PastPending.Task.IsCompleted);
                Assert.Equal("current", graph.SelectedPeriod?.Id);
                Assert.Same(accepted, graph.Scene);
                client.CompletePastAndNotify();
                PumpUntil(() => graph.SelectedPeriod?.Id == "past" && !graph.IsLoading);
                Assert.Equal("past", Assert.IsType<ApiHistoryPeriod>(selector.SelectedItem).Id);
                Assert.Equal(Now - 600, graph.Scene.PeriodStartAt);
                Assert.Equal(4, graph.SelectedPeriod!.Samples.Count);
                Assert.Equal(40, graph.Points[^1].SolValue);
            }
            finally { window.Close(); }
        }, CancellationToken.None);
    }

    private static void PumpUntil(Func<bool> condition)
    {
        var elapsed = Stopwatch.StartNew();
        while (!condition())
        {
            Dispatcher.UIThread.RunJobs();
            if (elapsed.Elapsed > TimeSpan.FromSeconds(2)) throw new TimeoutException("The bound selector was not published.");
            Thread.Sleep(5);
        }
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task PendingPeriodSelectionResumesFromReceiveNotification(bool reverseDirectory)
    {
        var client = new PendingHistoryClient(reverseDirectory);
        using var main = new MainWindowViewModel(client);
        // Isolate Graph's receive notification from unrelated Main refreshes.
        // The accepted Main is prepared once; it cannot replay this selection.
        var publish = typeof(MainWindowViewModel).GetMethod("ApplyDetailsGeneration", BindingFlags.Instance | BindingFlags.NonPublic)!;
        publish.Invoke(main, [new ApiDetailsSnapshot(ApiState.Ready, Now, true, "Pro", null, [], 0, [], [], [], "known") { PublishedPair = Image }]);
        using var graph = new GraphWindowViewModel(main, action => action(), () => Now);
        await Eventually(() => graph.SelectedPeriod?.Id == "current" && !graph.IsLoading);
        Assert.False(graph.CanGoForward);
        Assert.True(graph.CanGoBack);
        var acceptedScene = graph.Scene;

        graph.SelectedPeriod = graph.Periods.Single(period => period.Id == "past");
        await client.PastPending.Task.WaitAsync(TimeSpan.FromSeconds(2));
        Assert.Equal("current", graph.SelectedPeriod?.Id);
        Assert.Same(acceptedScene, graph.Scene);
        Assert.True(graph.IsLoading);
        Assert.False(graph.HasLoadError);
        Assert.Equal(1, client.PastCalls);

        client.CompletePastAndNotify();
        await Eventually(() => graph.SelectedPeriod?.Id == "past" && !graph.IsLoading);

        Assert.Equal(Now - 600, graph.Scene.PeriodStartAt);
        Assert.Equal(4, graph.SelectedPeriod!.Samples.Count);
        Assert.Equal(40, graph.Points[^1].SolValue);
        Assert.NotSame(acceptedScene, graph.Scene);
        Assert.False(graph.HasLoadError);
        Assert.True(graph.CanGoForward);
        Assert.False(graph.CanGoBack);
        graph.GoForward();
        await Eventually(() => graph.SelectedPeriod?.Id == "current" && !graph.IsLoading);
        Assert.False(graph.CanGoForward);
    }

    private static async Task Eventually(Func<bool> condition)
    {
        var elapsed = Stopwatch.StartNew();
        while (!condition())
        {
            if (elapsed.Elapsed > TimeSpan.FromSeconds(2)) throw new TimeoutException("The received period was not published.");
            await Task.Delay(10);
        }
    }

    private sealed class PendingHistoryClient(bool reverseDirectory = false) : HealthyDetailsClientBase, ILoopbackResourceClient, IUiChangeClient
    {
        private readonly object gate = new();
        private TaskCompletionSource<long> changed = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private long revision;
        private bool complete;
        private int pastCalls;
        public int PastCalls => Volatile.Read(ref pastCalls);
        public TaskCompletionSource PastPending { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public long ChangeRevision { get { lock (gate) return revision; } }
        public Task<long> WaitForChangeAsync(long after, CancellationToken token)
        {
            lock (gate) return revision == after ? changed.Task.WaitAsync(token) : Task.FromResult(revision);
        }
        public void CompletePastAndNotify()
        {
            TaskCompletionSource<long> signal; long next;
            lock (gate) { complete = true; next = ++revision; signal = changed; changed = new(TaskCreationOptions.RunContinuationsAsynchronously); }
            signal.TrySetResult(next);
        }
        private static ApiHistoryPeriod Period(string id) => id == "current"
            ? new(id, Now - 120, Now - 60, true, "current") { ResetAt = Now + 3600 }
            : new(id, Now - 600, Now - 420, false, "past") { ResetAt = Now - 420 };
        public Task<HistoryPeriodsFetchResult> FetchHistoryPeriodsAsync(CancellationToken token = default) => Task.FromResult(HistoryPeriodsFetchResult.Success(new(reverseDirectory ? [Period("past"), Period("current")] : [Period("current"), Period("past")], Image)));
        public Task<HistoryPageFetchResult> FetchHistoryPageAsync(string periodId, string? cursor = null, CancellationToken token = default)
        {
            if (periodId == "past")
            {
                Interlocked.Increment(ref pastCalls);
                lock (gate) if (!complete) { PastPending.TrySetResult(); return Task.FromResult(HistoryPageFetchResult.FromFailure(DetailsFetchFailure.Pending)); }
            }
            var period = Period(periodId); var count = periodId == "past" ? 4 : 2;
            var samples = Enumerable.Range(0, count).Select(i => new ApiHistorySample(period.StartAt + i * 60, period.ResetAt, 80, 1, 2, 3, (ulong)(i + 1) * 10, 20, 30, "confirmed") { ModelsComplete = true }).ToArray();
            return Task.FromResult(HistoryPageFetchResult.Success(new(periodId, samples, [], null, null, Image)));
        }
        public Task<CurrentFetchResult> FetchCurrentAsync(CancellationToken token = default) => throw new InvalidOperationException("Graph must not acquire Main.");
        public Task<ThreadsFetchResult> FetchThreadsAsync(CancellationToken token = default) => throw new InvalidOperationException("Graph must not acquire Threads.");
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken token = default) => throw new InvalidOperationException("The prepared Main must not refresh.");
    }

    private sealed class SelectionMainClient : HealthyDetailsClientBase, ILoopbackUiClient
    {
        private readonly object gate = new();
        private TaskCompletionSource<long> changed = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private long revision;
        public long ChangeRevision { get { lock (gate) return revision; } }
        public bool AccountChoicesVisible => false;
        public Task<long> WaitForChangeAsync(long after, CancellationToken token)
        {
            lock (gate) return revision != after ? Task.FromResult(revision) : changed.Task.WaitAsync(token);
        }
        public void Notify()
        {
            TaskCompletionSource<long> signal; long next;
            lock (gate) { next = ++revision; signal = changed; changed = new(TaskCreationOptions.RunContinuationsAsynchronously); }
            signal.TrySetResult(next);
        }
        public Task<UiMainResult> FetchMainAsync(string? accountId, bool includeChoices, CancellationToken cancellationToken = default) => Task.FromResult(new UiMainResult(
            new ApiCurrentSnapshot(ApiState.Ready, Now + ChangeRevision, true, "Pro", null, [], 0, PublishedPairIdentity.Create(new string(ChangeRevision == 0 ? 'a' : 'b', 64)))
            {
                AccountId = "account-1",
                Selection = new ApiAccount("account-1", true, null, null),
            }, null, null));
        public Task CheckAsync(CancellationToken cancellationToken = default) => Task.CompletedTask;
        public void SetViewVisible(string view, bool visible) { }
        public Task<ApiAccount?> FetchAccountContextAsync(string accountId, CancellationToken cancellationToken = default) => throw new InvalidOperationException("The Main projection must not acquire Graph context.");
        public Task<HistoryPeriodsFetchResult> FetchWindowPeriodsAsync(string accountId, long from, long until, CancellationToken cancellationToken = default) => throw new InvalidOperationException();
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) => throw new InvalidOperationException();
    }
}
