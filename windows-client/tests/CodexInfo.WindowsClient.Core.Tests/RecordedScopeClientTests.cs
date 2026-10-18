// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;
using System.Text.Json.Nodes;
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

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task CompleteScopedPagesPreserveFifteenAndRejectAnEmptyNonterminalDirectory(bool emptyNonterminal)
    {
        var fixture = JsonNode.Parse(File.ReadAllText(FixturePath()))!;
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new Handler(request =>
        {
            var uri = request.RequestUri!;
            paths.Add(uri.PathAndQuery);
            Assert.Contains("account=account-1", uri.Query);
            var continuation = uri.Query.Contains("cursor=", StringComparison.Ordinal);
            var key = uri.AbsolutePath switch
            {
                "/v6/history/scopes" => "scopes",
                "/v4/history/recordings" => "recordings",
                "/v4/history/observations" => "observations",
                _ => throw new InvalidOperationException("Unexpected fallback"),
            };
            var body = fixture[key]!.DeepClone();
            if (key == "scopes")
            {
                var all = body["scopes"]!.AsArray();
                body["scopes"] = !continuation && emptyNonterminal ? new JsonArray() :
                    continuation && emptyNonterminal ? all.DeepClone() :
                    new JsonArray(all[continuation ? 1 : 0]!.DeepClone());
            }
            else if (key == "recordings")
            {
                var all = body["samples"]!.AsArray();
                body["samples"] = new JsonArray(all[continuation ? 1 : 0]!.DeepClone());
                if (!continuation) body["quota_observations"] = new JsonArray();
            }
            else
            {
                var all = body["observations"]!.AsArray();
                body["observations"] = new JsonArray(all[continuation ? 1 : 0]!.DeepClone());
            }
            body["next_cursor"] = continuation ? null : key + ".next";
            return Json(body.ToJsonString());
        }));

        var result = await ((ILoopbackRecordedScopeClient)client).FetchRecordedGraphAsync("account-1");
        if (emptyNonterminal) Assert.False(result.IsSuccess);
        else
        {
            Assert.True(result.IsSuccess);
            Assert.Equal(2, result.Bundle!.Scopes.Count);
            Assert.Equal(new ulong[] { 10, 15 }, result.Bundle.Samples.Select(sample => sample.Models.Single().TotalTokens));
            Assert.Equal(new ulong?[] { 66123101, 3763895 }, result.Bundle.Observations.Select(row => row.Models.Single().TotalTokens));
            Assert.Equal(6, paths.Count);
        }
    }

    [Fact]
    public async Task CursorlessRejectionDoesNotRestartTheSameRecordedBundle()
    {
        var requests = 0;
        using var client = new LoopbackStatusClient(new Handler(_ =>
        {
            requests++;
            var response = Json("""{"api_version":"v1","error":"stale_cursor"}""");
            response.StatusCode = HttpStatusCode.BadRequest;
            response.Headers.Remove("Codex-Info-Published-Pair");
            return response;
        }));
        var result = await ((ILoopbackRecordedScopeClient)client).FetchRecordedGraphAsync("account-1");
        Assert.False(result.IsSuccess);
        Assert.Equal(1, requests);
    }

    internal static string FixturePath()
    {
        var root = new DirectoryInfo(AppContext.BaseDirectory);
        while (root is not null)
        {
            var path = Path.Combine(root.FullName, "tests", "fixtures", "recorded_scope_contract.json");
            if (File.Exists(path)) return path;
            root = root.Parent;
        }
        throw new FileNotFoundException("Independent recorded scope fixture was not found.");
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
