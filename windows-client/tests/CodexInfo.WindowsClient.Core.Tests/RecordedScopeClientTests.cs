// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class RecordedScopeClientTests
{
    [Fact]
    public async Task ProductionCurrentStartsWithV4WithoutRecordedScopeDependency()
    {
        const string current = """
            {"api_version":"v4","models_status":"partial","state":"ready","observed_at":1792228061,
             "authenticated":true,"plan_label":"Plus","quota":{"remaining_percent":88,"reset_at":1792823932,
             "window_seconds":604800,"monthly":false},"models":[{"model":"ASTRA","total_tokens":13,
             "input_tokens":10,"cached_input_tokens":2,"cache_write_input_tokens":1,"output_tokens":3,
             "estimated_cost":{"price_version":"ASTRA_USER_2026-09-05","ordinary_input_dollars":1,
             "cached_input_dollars":2,"cache_write_input_dollars":3,"output_dollars":4,"total_dollars":10}}],
             "active_thread_count":0,"open_session_thread_count":0}
            """;
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new Handler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return request.RequestUri.AbsolutePath == "/v4/current"
                ? Json(current) : new HttpResponseMessage(HttpStatusCode.ServiceUnavailable);
        }));
        var result = await client.FetchCurrentAsync();
        Assert.Equal(new[] { "/v4/current" }, paths);
        Assert.True(result.IsSuccess);
        Assert.Equal("v4", result.Snapshot!.ApiVersion);
        var model = Assert.Single(result.Snapshot.Models);
        Assert.Equal(13UL, model.TotalTokens);
        Assert.Equal(10d, model.EstimatedTotalDollars);
        Assert.Equal(ApiCurrentModelsStatus.Partial, result.Snapshot.ModelsStatus);
    }

    internal static HttpResponseMessage Json(string body)
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(body) };
        response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json") { CharSet = "utf-8" };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        response.Headers.TryAddWithoutValidation("Codex-Info-Published-Pair",
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001");
        return response;
    }

    private sealed class Handler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) =>
            Task.FromResult(respond(request));
    }
}
