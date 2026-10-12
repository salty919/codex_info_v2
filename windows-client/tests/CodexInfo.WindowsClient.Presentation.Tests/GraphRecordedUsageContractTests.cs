// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphRecordedUsageContractTests
{
    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void LaterCompleteObservationDoesNotEraseIntermediateRecordedFamilyUsage(GraphMetric metric)
    {
        static ApiHistorySample Row(long at, bool complete, params ApiHistoryModelSample[] models) =>
            new(at, 9000, 50, null, null, null, null, null, null,
                complete ? ApiHistorySample.ConfirmedModelSource : ApiHistorySample.LegacyUnknownModelSource)
            { ModelsComplete = complete, ModelSamples = models };
        ApiHistoryModelSample Model(string name, ulong amount) =>
            new(name, null, null, null, metric == GraphMetric.Dollars ? amount : null)
            { TotalTokens = metric == GraphMetric.Tokens ? amount : null };
        var scene = GraphScene.Create(
        [
            Row(1020, false, Model("LUNA", 0)),
            Row(1080, false, Model("gpt-6-luna", 200)),
            Row(1140, true, Model("gpt-6-luna", 300)),
        ], metric, 1020, 1140);
        var lines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        var points = lines.Rising.X.Zip(lines.Rising.Y)
            .Concat(lines.Flat.X.Zip(lines.Flat.Y))
            .Concat(lines.Dashed.X.Zip(lines.Dashed.Y));
        Assert.Contains((1080d, 200d), points);
    }

    [Theory]
    [InlineData(GraphMetric.Tokens, false, 26657919d, 10027685d, "144 M")]
    [InlineData(GraphMetric.Dollars, false, 0.51055206, 1.911146, "$143")]
    [InlineData(GraphMetric.Tokens, true, 26657919d, 10027685d, "144 M")]
    [InlineData(GraphMetric.Dollars, true, 0.51055206, 1.911146, "$143")]
    public async Task PublishedLegacyHistoryPreservesRecordedFamiliesAndPeriodAmount(
        GraphMetric metric, bool quotaOnlyTail, double luna, double sol, string amount)
    {
        using var client = new LoopbackStatusClient(new HistoryHandler(quotaOnlyTail));
        var result = await client.FetchHistoryPageAsync("1792385949");
        Assert.True(result.IsSuccess);
        var scene = GraphScene.Create(result.Page!.Samples, metric, 1791781149, 1791803280);

        // Independent sums of the three recorded model rows in the response.
        Assert.Equal(143630999m, scene.RecordedPeriodTokens);
        Assert.Equal(143.26962406, scene.PeriodCost.RecordedDollars!.Value, 8);
        Assert.Equal(amount, GraphUsageSummary.PeriodText(scene));
        Assert.Equal(luna, scene.Luna[^1], 8);
        Assert.Equal(sol, scene.Sol[^1], 8);
        var lunaLines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        Assert.Contains(luna,
            lunaLines.Rising.Y.Concat(lunaLines.Flat.Y).Concat(lunaLines.Idle.Y).Concat(lunaLines.Dashed.Y));
        foreach (var view in new[]
        {
            scene,
            GraphScene.CreateViewport(1791803280 - 86400, 1791803280, metric, [scene]),
            GraphScene.CreateViewport(1791803280 - 604800, 1791803280, metric, [scene]),
        })
        {
            var control = new GraphPlotControl { Scene = view };
            using var image = control.Plot.GetImage(900, 540);
            var panel = Assert.Single(control.Plot.Axes.GetPanels().OfType<GraphPeriodCostPanel>());
            Assert.Equal(amount, Assert.Single(panel.Amounts).LabelStyle.Text);
            var renderDirectory = Environment.GetEnvironmentVariable("CODEX_INFO_RECORDED_USAGE_RENDER_DIR");
            if (quotaOnlyTail && !view.IsViewport && !string.IsNullOrWhiteSpace(renderDirectory))
            {
                Directory.CreateDirectory(renderDirectory);
                control.Plot.SavePng(Path.Combine(renderDirectory, $"recorded-{metric}.png"), 900, 540);
            }
        }
    }

    private sealed class HistoryHandler(bool quotaOnlyTail) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request, CancellationToken cancellationToken)
        {
            Assert.Equal("/v3/history", request.RequestUri!.AbsolutePath);
            const string json = """
                {"api_version":"v3","history_gaps":[],"next_cursor":null,"resume_cursor":"saved-row",
                 "history_samples":[
                  {"timestamp":1791781260,"reset_at":1792385949,"remaining_percent":100,
                   "model_source":"legacy-unknown","models_complete":false,"task_active_since_previous":true,
                   "models":[{"model":"LUNA","total_tokens":0,"total_dollars":0},
                    {"model":"gpt-6-astra","total_tokens":168148,"total_dollars":0.361024},
                    {"model":"gpt-6.1-sol","total_tokens":36988,"total_dollars":0.033016}]},
                  {"timestamp":1791781980,"reset_at":1792385949,"remaining_percent":99,
                   "model_source":"unavailable","models_complete":false,"task_active_since_previous":true,"models":null},
                  {"timestamp":1791803220,"reset_at":1792385949,"remaining_percent":55,
                   "model_source":"legacy-unknown","models_complete":false,"task_active_since_previous":true,
                   "models":[{"model":"gpt-6-astra","total_tokens":106945395,"total_dollars":140.847926},
                    {"model":"gpt-6-luna","total_tokens":26657919,"total_dollars":0.51055206},
                    {"model":"gpt-6.1-sol","total_tokens":10027685,"total_dollars":1.911146}]}]}
                """;
            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(quotaOnlyTail
                    ? json[..^2] + """
                        ,{"timestamp":1791803280,"reset_at":1792385949,"remaining_percent":54,
                          "model_source":"unavailable","models_complete":false,
                          "task_active_since_previous":null,"models":null}]}
                        """
                    : json),
            };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json") { CharSet = "utf-8" };
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation("Codex-Info-Published-Pair",
                "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001");
            return Task.FromResult(response);
        }
    }
}
