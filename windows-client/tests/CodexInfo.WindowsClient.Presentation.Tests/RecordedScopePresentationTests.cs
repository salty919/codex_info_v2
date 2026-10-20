// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using System.Reflection;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class RecordedScopePresentationTests : IDisposable
{
    private const long Now = 1792228061;
    private readonly string oldLanguage = LocalizationService.Current.LanguageCode;
    private readonly bool oldUtc = LocalizationService.DisplayTimeZone.Equals(TimeZoneInfo.Utc);

    public RecordedScopePresentationTests()
    {
        LocalizationService.SetLanguage("ja");
        LocalizationService.SetTimeZone("UTC");
    }

    public void Dispose()
    {
        LocalizationService.SetLanguage(oldLanguage);
        LocalizationService.SetTimeZone(oldUtc ? "UTC" : "local");
    }

    [Fact]
    public async Task ExistingGraphKeepsDateGroupsLocalMonthCurvesAndAllModelTotals()
    {
        using var handler = new FixtureHandler { ExistingUi = true };
        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchCurrentAsync("account-1");
        Assert.True(result.IsSuccess);
        Assert.Equal("v4", result.Snapshot!.ApiVersion);
        var threads = await client.FetchThreadsAsync("account-1");
        Assert.True(threads.IsSuccess);
        Assert.Equal(result.Snapshot.PublishedPair, threads.Snapshot!.PublishedPair);
        using var main = CreateMain(client, result.Snapshot, threads.Snapshot.Threads);
        Assert.Equal("13", Assert.Single(main.Models).TotalTokensText);
        Assert.Contains("10.00", main.EstimatedCostText, StringComparison.Ordinal);
        Assert.Equal(1UL, main.ActiveThreadCount);
        Assert.Single(main.DetailsSnapshot!.Threads);
        using var graph = CreateGraph(main);
        await Settled(graph);
        Assert.Equal(new[] { "1792823932" }, graph.Periods.Select(period => period.Id));
        Assert.DoesNotContain(handler.Paths, path => path.StartsWith("/v6/", StringComparison.Ordinal));
        var line = GraphPlotProjection.BuildModelLines(graph.Scene, graph.Scene.Sol);
        Assert.NotEmpty(line.Rising.X);
        Assert.Contains(1_000_000d, line.Rising.Y);
        Assert.Contains(2_000_000d, line.Rising.Y);
        graph.ShowSol = false;
        graph.SelectedTimeRange = GraphTimeRange.CalendarMonth;
        await Settled(graph);
        Assert.Equal("2026/10", graph.RangeLabel);
        Assert.True(graph.IsMonthView);
        Assert.False(graph.HasLinePlot);
        Assert.Contains(graph.CalendarDays, day => day.ValueText == "2 M");
        graph.IsTokensMetric = false;
        await Settled(graph);
        Assert.Contains(graph.CalendarDays, day => day.ValueText == "$5.00");
        Assert.False(graph.ShowSol);
    }

    private static MainWindowViewModel CreateMain(LoopbackStatusClient client, ApiCurrentSnapshot current,
        IReadOnlyList<ApiThreadDetails>? threads = null)
    {
        var main = new MainWindowViewModel(new Health(), client);
        typeof(MainWindowViewModel).GetMethod("ApplyAccountsSnapshot", BindingFlags.Instance | BindingFlags.NonPublic)!
            .Invoke(main, [new ApiAccountsSnapshot("account-1", [new("account-1", true, 1, null)])]);
        typeof(MainWindowViewModel).GetMethod("ApplyCurrentGeneration", BindingFlags.Instance | BindingFlags.NonPublic)!
            .Invoke(main, [current, threads ?? Array.Empty<ApiThreadDetails>()]);
        return main;
    }

    private static GraphWindowViewModel CreateGraph(MainWindowViewModel main) =>
        (GraphWindowViewModel)typeof(GraphWindowViewModel).GetConstructor(BindingFlags.Instance | BindingFlags.NonPublic,
            null, [typeof(MainWindowViewModel), typeof(Action<Action>), typeof(Func<long>)], null)!
            .Invoke([main, (Action<Action>)(action => action()), (Func<long>)(() => Now)]);

    private static async Task Settled(GraphWindowViewModel graph)
    {
        for (var attempt = 0; graph.IsLoading && attempt < 200; attempt++) await Task.Delay(10);
        Assert.False(graph.IsLoading);
    }

    private sealed class Health : ILoopbackHealthClient
    {
        public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
            Task.FromResult(HealthFetchResult.Success(new("v1", "codex-info", "1.0.0")));
    }

    private sealed class FixtureHandler : HttpMessageHandler
    {
        internal readonly List<string> Paths = [];
        internal bool ExistingUi;

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var uri = request.RequestUri!;
            Paths.Add(uri.PathAndQuery);
            if (ExistingUi && uri.AbsolutePath == "/v4/current")
                return Task.FromResult(Response("""
                    {"api_version":"v4","models_status":"partial","state":"ready","observed_at":1792228061,
                     "authenticated":true,"plan_label":"Plus","quota":{"remaining_percent":88,"reset_at":1792823932,
                     "window_seconds":604800,"monthly":false},"models":[{"model":"ASTRA","total_tokens":13,
                     "input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":1,"output_tokens":3,
                     "estimated_cost":{"price_version":"ASTRA_USER_2026-09-05","ordinary_input_dollars":1,
                     "cached_input_dollars":2,"cache_write_input_dollars":3,"output_dollars":4,"total_dollars":10}}],
                     "active_thread_count":1,"open_session_thread_count":1}
                    """));
            if (ExistingUi && uri.AbsolutePath == "/v3/threads")
                return Task.FromResult(Response("""
                    {"api_version":"v3","threads":[{"id":"thread-1","title":"active","parent_thread_id":null,
                     "model":"ASTRA","model_label":"ASTRA","total_tokens":13,"context_usage_tokens":null,
                     "context_window_tokens":null,"created_at":1,"last_user_message_at":1,"is_subagent":false,
                     "depth":0,"activity_status":"running"}]}
                    """));
            if (ExistingUi && uri.AbsolutePath.StartsWith("/v5/history/periods", StringComparison.Ordinal))
                return Task.FromResult(Response("""
                    {"api_version":"v5","history_periods":[{"id":"1792823932","start_at":1792219132,
                     "end_at":1792228061,"reset_at":1792823932,"origin_at":null,"current":true,"label":"existing period"}],
                     "next_cursor":null}
                    """));
            if (ExistingUi && uri.AbsolutePath == "/v3/history")
                return Task.FromResult(Response("""
                    {"api_version":"v3","history_gaps":[],"next_cursor":null,"resume_cursor":"existing-last",
                     "history_samples":[
                      {"timestamp":1792227960,"reset_at":1792823932,"remaining_percent":89,
                       "model_source":"legacy-unknown","models_complete":false,"models":[
                        {"model":"gpt-6.1-sol","total_tokens":1000000,"total_dollars":2.5},
                        {"model":"gpt-6-astra","total_tokens":2000000,"total_dollars":5}]},
                      {"timestamp":1792228020,"reset_at":1792823932,"remaining_percent":88,
                       "model_source":"legacy-unknown","models_complete":false,"models":[
                        {"model":"gpt-6.1-sol","total_tokens":2000000,"total_dollars":5},
                        {"model":"gpt-6-astra","total_tokens":3000000,"total_dollars":7.5}]}]}
                    """));
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
        }

        private HttpResponseMessage Response(string body)
        {
            var response = new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(body) };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json") { CharSet = "utf-8" };
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation("Codex-Info-Published-Pair",
                "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001");
            return response;
        }
    }
}
