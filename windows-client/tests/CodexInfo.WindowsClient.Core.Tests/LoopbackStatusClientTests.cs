// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using System.Reflection;
using System.Text;
using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class LoopbackStatusClientTests
{
    [Fact]
    public async Task Issue604HistoryKeepsKnownTotalsWithOptionalComponentsAndIndependentCoverage()
    {
        static string Page(string models) => """
            {"api_version":"v3","history_samples":[{"timestamp":1800000000,"reset_at":1800604800,
            "remaining_percent":31,"task_active_since_previous":null,"models_complete":false,
            "model_source":"confirmed","models":MODELS}],"history_gaps":[],
            "next_cursor":null,"resume_cursor":"saved-prefix"}
            """.Replace("MODELS", models, StringComparison.Ordinal);
        const string known = """
            [{"model":"gpt-6.1-sol","total_tokens":10,"input_tokens":8,"cached_input_tokens":2,
              "cache_write_input_tokens":0,"output_tokens":2,"total_dollars":0.125},
             {"model":"TERRA","total_tokens":7,"total_dollars":0.5},
             {"model":"gpt-6-astra","total_tokens":20,"cached_input_tokens":4}]
            """;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            Assert.Equal("/v3/history", request.RequestUri!.AbsolutePath);
            return JsonResponse(Page(known), includePublishedPair: true);
        }));
        var result = await client.FetchHistoryPageAsync("account-7", "1800604800", null, CancellationToken.None);
        Assert.True(result.IsSuccess, "known history totals must survive optional component gaps");
        var sample = Assert.Single(result.Page!.Samples);
        Assert.Equal(ApiHistorySample.ConfirmedModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Equal(31, sample.RemainingPercent);
        var models = sample.ModelSamples!;
        Assert.Equal(3, models.Count);
        Assert.Equal(10UL, models[0].TotalTokens);
        Assert.Equal(8UL, models[0].InputTokens);
        Assert.Equal(2UL, models[0].CachedInputTokens);
        Assert.Equal(2UL, models[0].OutputTokens);
        Assert.Equal(BitConverter.DoubleToInt64Bits(0.125), BitConverter.DoubleToInt64Bits(models[0].TotalDollars!.Value));
        Assert.Equal(7UL, models[1].TotalTokens);
        Assert.Equal(0.5, models[1].TotalDollars);
        Assert.Null(models[1].InputTokens);
        Assert.Null(models[1].CachedInputTokens);
        Assert.Null(models[1].OutputTokens);
        Assert.Null(models[1].CacheWriteInputTokens);
        Assert.Equal(20UL, models[2].TotalTokens);
        Assert.Equal(4UL, models[2].CachedInputTokens);
        Assert.Null(models[2].InputTokens);
        Assert.Null(models[2].OutputTokens);
        Assert.Null(models[2].TotalDollars);

        foreach (var invalid in new[]
        {
            "{\"model\":\"SOL\",\"total_tokens\":10,\"input_tokens\":11}",
            "{\"model\":\"SOL\",\"total_tokens\":10,\"input_tokens\":8,\"output_tokens\":3}",
            "{\"model\":\"SOL\",\"total_tokens\":10,\"input_tokens\":8,\"cached_input_tokens\":8,\"cache_write_input_tokens\":1}",
            "{\"model\":\"SOL\",\"total_tokens\":10,\"cached_input_tokens\":11}",
        })
        {
            using var rejected = new LoopbackStatusClient(new StubHandler(_ =>
                JsonResponse(Page($"[{invalid}]"), includePublishedPair: true)));
            Assert.False((await rejected.FetchHistoryPageAsync("account-7", "1800604800", null, CancellationToken.None)).IsSuccess);
        }
    }

    [Fact]
    public async Task Issue604V5DirectoryCommitsAllPagesAndReusesOnlyTheCompleteCache()
    {
        var paths = new List<string>();
        var cursor = "server.opaque-next";
        var cycle = 0;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.PathAndQuery);
            Assert.Equal("/v5/history/periods/selectable", request.RequestUri.AbsolutePath);
            if (cycle == 1)
            {
                Assert.Equal("?account=account-7", request.RequestUri.Query);
                Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
                return Issue604NotModified(CanonicalPublishedPair);
            }
            Assert.Empty(request.Headers.IfNoneMatch);
            return request.RequestUri.Query.Contains("cursor=", StringComparison.Ordinal)
                ? JsonResponse(Issue604DirectoryPage([2, 3], next: null), includePublishedPair: true)
                : JsonResponse(Issue604DirectoryPage([0, 1], cursor), includePublishedPair: true);
        }, versionedPeriods: true, pagedPeriods: true));
        var first = await client.FetchSelectableHistoryPeriodsAsync("account-7");
        Assert.True(first.IsSuccess);
        Assert.Equal("v5", first.Snapshot!.ApiVersion);
        Assert.Equal("account-7", first.Snapshot.AccountId);
        Assert.Equal(["1800604980", "1800604920", "1800604860", "1800604800"], first.Snapshot.Periods.Select(p => p.Id));
        Assert.Equal(1800000180L, Assert.Single(first.Snapshot.Periods, p => p.Current).OriginAt);
        cycle = 1;
        var second = await client.FetchSelectableHistoryPeriodsAsync("account-7");
        Assert.True(second.IsSuccess);
        Assert.Same(first.Snapshot, second.Snapshot);
        Assert.Equal(new[]
        {
            "/v5/history/periods/selectable?account=account-7",
            "/v5/history/periods/selectable?account=account-7&cursor=server.opaque-next",
            "/v5/history/periods/selectable?account=account-7",
        }, paths);
    }

    [Theory]
    [InlineData("pair")]
    [InlineData("duplicate")]
    [InlineData("order")]
    [InlineData("empty")]
    [InlineData("repeat")]
    [InlineData("current")]
    [InlineData("missing-cursor")]
    [InlineData("extra-key")]
    [InlineData("404")]
    [InlineData("503")]
    public async Task Issue604V5DirectoryRejectsAnIncompleteCandidateAndKeepsLastGood(string defect)
    {
        var phase = 0;
        var calls = new List<string>();
        var changedPair = $"v1:{new string('b', 64)}";
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            calls.Add(request.RequestUri!.PathAndQuery);
            Assert.Equal("/v5/history/periods", request.RequestUri.AbsolutePath);
            if (phase == 0)
                return JsonResponse(Issue604DirectoryPage([0], null), includePublishedPair: true);
            if (phase == 2)
            {
                Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
                return Issue604NotModified(CanonicalPublishedPair);
            }
            var continuation = request.RequestUri.Query.Contains("cursor=", StringComparison.Ordinal);
            if (continuation && defect == "404") return NotFoundResponse();
            if (continuation && defect == "503") return new(HttpStatusCode.ServiceUnavailable);
            var json = continuation
                ? defect switch
                {
                    "duplicate" => Issue604DirectoryPage([1], null),
                    "order" => Issue604DirectoryPage([0], null),
                    "empty" => Issue604DirectoryPage([], "different.cursor"),
                    "repeat" => Issue604DirectoryPage([2], "next.cursor"),
                    "current" => Issue604DirectoryPage([3], null),
                    "missing-cursor" => Issue604DirectoryPage([2], null).Replace(",\"next_cursor\":null", "", StringComparison.Ordinal),
                    "extra-key" => Issue604DirectoryPage([2], null).Replace("\"api_version\":\"v5\"", "\"api_version\":\"v5\",\"extra\":0", StringComparison.Ordinal),
                    _ => Issue604DirectoryPage([2], null),
                }
                : Issue604DirectoryPage([1], "next.cursor", currentIndex: defect == "current" ? 1 : 3);
            var response = JsonResponse(json, includePublishedPair: true);
            response.Headers.Remove(PublishedPairHeader);
            response.Headers.TryAddWithoutValidation(PublishedPairHeader,
                continuation && defect == "pair" ? CanonicalPublishedPair : changedPair);
            return response;
        }, versionedPeriods: true, pagedPeriods: true));
        var accepted = await client.FetchHistoryPeriodsAsync("account-7");
        Assert.True(accepted.IsSuccess);
        phase = 1;
        var rejected = await client.FetchHistoryPeriodsAsync("account-7");
        Assert.False(rejected.IsSuccess);
        Assert.Equal(3, calls.Count); // no fallback or same-callback restart
        phase = 2;
        var retained = await client.FetchHistoryPeriodsAsync("account-7");
        Assert.True(retained.IsSuccess);
        Assert.Same(accepted.Snapshot, retained.Snapshot);
    }

    [Theory]
    [InlineData(false, "404")]
    [InlineData(true, "404")]
    [InlineData(false, "503")]
    [InlineData(true, "malformed")]
    [InlineData(true, "timeout")]
    public async Task Issue604V5DirectoryFallsBackOnlyForTheFirstExact404(bool selectable, string state)
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            var path = request.RequestUri!.AbsolutePath;
            paths.Add(path);
            Assert.Equal("?account=account-7", request.RequestUri.Query);
            if (path.StartsWith("/v5/", StringComparison.Ordinal))
                return state switch
                {
                    "404" => NotFoundResponse(),
                    "503" => new(HttpStatusCode.ServiceUnavailable),
                    "malformed" => JsonResponse("{}", includePublishedPair: true),
                    _ => throw new TaskCanceledException(),
                };
            if (path.StartsWith("/v4/", StringComparison.Ordinal) || path.EndsWith("/selectable", StringComparison.Ordinal))
                return NotFoundResponse();
            return JsonResponse(Issue646PeriodsJson("archive"), includePublishedPair: true);
        }, versionedPeriods: true, pagedPeriods: true));
        var result = selectable
            ? await client.FetchSelectableHistoryPeriodsAsync("account-7")
            : await client.FetchHistoryPeriodsAsync("account-7");
        Assert.Equal(state == "404", result.IsSuccess);
        var suffix = selectable ? "/selectable" : "";
        Assert.Equal(state == "404"
            ? selectable
                ? new[] { $"/v5/history/periods{suffix}", $"/v4/history/periods{suffix}", $"/v3/history/periods{suffix}", "/v3/history/periods" }
                : new[] { "/v5/history/periods", "/v4/history/periods", "/v3/history/periods" }
            : new[] { $"/v5/history/periods{suffix}" }, paths);
    }

    private static string Issue604DirectoryPage(int[] rows, string? next, int currentIndex = 3) =>
        System.Text.Json.JsonSerializer.Serialize(new
        {
            api_version = "v5",
            history_periods = rows.Select(index => new
            {
                id = (1800604800L + index * 60).ToString(System.Globalization.CultureInfo.InvariantCulture),
                start_at = 1800000000L + index * 60,
                end_at = 1800000059L + index * 60,
                reset_at = 1800604800L + index * 60,
                label = $"2027/01/15 17:{index:00}:00 +09:00 ～ 2027/01/15 17:{index:00}:59 +09:00",
                current = index == currentIndex,
                origin_at = 1800000000L + index * 60,
            }).ToArray(),
            next_cursor = next,
        });

    private static HttpResponseMessage Issue604NotModified(string pair)
    {
        var response = new HttpResponseMessage(HttpStatusCode.NotModified) { Content = new ByteArrayContent([]) };
        response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json") { CharSet = "utf-8" };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        response.Headers.TryAddWithoutValidation(PublishedPairHeader, pair);
        return response;
    }

    [Fact]
    public async Task Issue604NormalResetCountKeepsAllExactPeriods()
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            Assert.Equal("/v4/history/periods", request.RequestUri!.AbsolutePath);
            var rows = Enumerable.Range(0, 188).Reverse().Select(index =>
            {
                var start = 1800000000L + index * 60;
                return new
                {
                    id = (start + 604800).ToString(System.Globalization.CultureInfo.InvariantCulture),
                    start_at = start,
                    end_at = start + 59,
                    reset_at = start + 604800,
                    current = index == 187,
                    label = "official",
                    origin_at = start,
                };
            }).ToArray();
            return JsonResponse(System.Text.Json.JsonSerializer.Serialize(new
            {
                api_version = "v4",
                history_periods = rows,
            }), includePublishedPair: true);
        }, versionedPeriods: true));
        var result = await client.FetchHistoryPeriodsAsync("account-7");
        Assert.True(result.IsSuccess);
        Assert.Equal(188, result.Snapshot!.Periods.Count);
        Assert.Equal(188, result.Snapshot.Periods.Select(period => period.ResetAt).Distinct().Count());
        Assert.Equal("account-7", result.Snapshot.AccountId);
    }

    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const string CanonicalPublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    [Fact]
    public async Task Issue614HistoryOriginUsesVersionedAuthorityAndExact404Fallback()
    {
        foreach (var state in new[] { "known", "earlier", "unknown", "404", "503", "invalid" })
        {
            var paths = new List<string>();
            using var client = new LoopbackStatusClient(new StubHandler(request =>
            {
                var path = request.RequestUri!.AbsolutePath;
                paths.Add(path);
                if (path == "/v3/history/periods")
                    return JsonResponse(Issue646PeriodsJson("period"), includePublishedPair: true);
                Assert.Equal("/v4/history/periods", path);
                if (state == "404") return NotFoundResponse();
                if (state == "503") return new HttpResponseMessage(HttpStatusCode.ServiceUnavailable);
                var origin = state == "unknown" ? "null" : state == "invalid" ? "1800000001" : state == "earlier" ? "1799999940" : "1800000000";
                var json = Issue646PeriodsJson("period").Replace("\"v3\"", "\"v4\"", StringComparison.Ordinal)
                    .Replace("\"current\":true", $"\"current\":true,\"origin_at\":{origin}", StringComparison.Ordinal);
                return JsonResponse(json, includePublishedPair: true);
            }, versionedPeriods: true));
            var result = await client.FetchHistoryPeriodsAsync("account-7");
            Assert.Equal("/v4/history/periods", paths[0]);
            Assert.Equal(state is "known" or "earlier" or "unknown" or "404", result.IsSuccess);
            Assert.Equal(state == "404" ? 2 : 1, paths.Count);
            if (result.IsSuccess)
            {
                var period = Assert.Single(result.Snapshot!.Periods);
                var property = typeof(ApiHistoryPeriod).GetProperty("OriginAt");
                Assert.NotNull(property);
                Assert.Equal(state == "known" ? (long?)1800000000 : state == "earlier" ? 1799999940 : null, property.GetValue(period));
            }
        }
    }

    [Fact]
    public async Task Issue646SelectablePeriodsFallBackOnlyAfterExact404()
    {
        foreach (var state in new[] { "ok", "404", "500", "503", "malformed", "timeout" })
        {
            var paths = new List<string>();
            using var client = new LoopbackStatusClient(new StubHandler(request =>
            {
                var path = request.RequestUri!.AbsolutePath;
                paths.Add(path);
                Assert.Equal("?account=account-7", request.RequestUri.Query);
                if (path == "/v3/history/periods")
                {
                    return JsonResponse(Issue646PeriodsJson("archive"), includePublishedPair: true);
                }
                Assert.Equal("/v3/history/periods/selectable", path);
                return state switch
                {
                    "ok" => JsonResponse(Issue646PeriodsJson("selected"), includePublishedPair: true),
                    "404" => NotFoundResponse(),
                    "500" => new HttpResponseMessage(HttpStatusCode.InternalServerError),
                    "503" => new HttpResponseMessage(HttpStatusCode.ServiceUnavailable),
                    "malformed" => JsonResponse("{}", includePublishedPair: true),
                    _ => throw new TaskCanceledException("fixture transport timeout"),
                };
            }));
            var result = await Issue646FetchSelectableAsync(client, "account-7");
            Assert.Equal(state is "ok" or "404", result.IsSuccess);
            Assert.Equal(state == "404" ? 2 : 1, paths.Count);
            if (result.IsSuccess)
            {
                Assert.Equal(state == "404" ? "archive" : "selected", Assert.Single(result.Snapshot!.Periods).Id);
            }
        }
    }

    [Fact]
    public async Task Issue646SelectablePeriodsKeepDirectoryAndAccountCachesSeparate()
    {
        var seen = new HashSet<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            var key = request.RequestUri!.PathAndQuery;
            var selectable = request.RequestUri.AbsolutePath.EndsWith("/selectable", StringComparison.Ordinal);
            if (seen.Add(key))
            {
                Assert.Empty(request.Headers.IfNoneMatch);
                return JsonResponse(Issue646PeriodsJson(selectable ? "selected" : "archive"), includePublishedPair: true);
            }
            Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
            var response = new HttpResponseMessage(HttpStatusCode.NotModified)
            {
                Content = new ByteArrayContent([]),
            };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json") { CharSet = "utf-8" };
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
            return response;
        }));
        for (var index = 0; index < 2; index++)
        {
            var selected = await Issue646FetchSelectableAsync(client, "account-7");
            var archive = await client.FetchHistoryPeriodsAsync("account-7");
            Assert.True(selected.IsSuccess);
            Assert.True(archive.IsSuccess);
            Assert.Equal("selected", Assert.Single(selected.Snapshot!.Periods).Id);
            Assert.Equal("archive", Assert.Single(archive.Snapshot!.Periods).Id);
        }
        var switched = await Issue646FetchSelectableAsync(client, "account-13");
        Assert.True(switched.IsSuccess);
        Assert.Equal("account-13", switched.Snapshot!.AccountId);
    }

    private static Task<HistoryPeriodsFetchResult> Issue646FetchSelectableAsync(LoopbackStatusClient client, string accountId)
    {
        var method = typeof(LoopbackStatusClient).GetMethod("FetchSelectableHistoryPeriodsAsync", [typeof(string), typeof(CancellationToken)]);
        Assert.NotNull(method);
        return Assert.IsAssignableFrom<Task<HistoryPeriodsFetchResult>>(method.Invoke(client, [accountId, CancellationToken.None]));
    }

    private static string Issue646PeriodsJson(string id) =>
        $$"""{"api_version":"v3","history_periods":[{"id":"{{id}}","start_at":1800000000,"end_at":1800000060,"reset_at":1800604800,"label":"fixture","current":true}]}""";

    [Fact]
    public async Task BetaHealthAndRuntimePreserveProductAndProcessVersions()
    {
        using var health = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(HealthJson("1.0.110-beta.7.1"), includePublishedPair: false)));
        var healthy = await health.FetchHealthAsync(CancellationToken.None);
        Assert.True(healthy.IsSuccess);
        Assert.Equal("1.0.110-beta.7.1", healthy.Snapshot!.ProductVersion);
        foreach (var (rest, recorder, status) in new (string, string?, string)[]
        {
            ("1.0.110-beta.7.1", "1.0.110-beta.7.1", "available"),
            ("1.0.110-beta.9.1", "1.0.110-beta.10.1", "mismatch"),
            ("1.0.109", "1.0.110-beta.7.1", "mismatch"),
            ("1.0.110-beta.7.1", "1.0.109", "mismatch"),
            ("1.0.110-beta.7.1", null, "unavailable"),
        })
        {
            var recorderJson = recorder is null ? "null" : $"\"{recorder}\"";
            using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(
                $$"""{"api_version":"v1","rest_version":"{{rest}}","recorder_version":{{recorderJson}},"recorder_status":"{{status}}"}""",
                includePublishedPair: false)));
            var result = await client.FetchRuntimeVersionsAsync(CancellationToken.None);
            Assert.True(result.IsSuccess);
            Assert.Equal(rest, result.Snapshot!.RestVersion);
            Assert.Equal(recorder, result.Snapshot.RecorderVersion);
            Assert.Equal(status, result.Snapshot.RecorderStatus);
        }
        foreach (var invalid in new[]
        {
            """{"api_version":"v1","rest_version":"1.0.110-beta.7.1","recorder_version":"1.0.110-beta.8.1","recorder_status":"available"}""",
            """{"api_version":"v1","rest_version":"1.0.110-beta.7.1","recorder_version":null,"recorder_status":"available"}""",
            """{"api_version":"v1","rest_version":"1.0.110-beta.7.1","recorder_version":"1.0.110-beta.07.1","recorder_status":"mismatch"}""",
        })
        {
            using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(invalid, includePublishedPair: false)));
            var result = await client.FetchRuntimeVersionsAsync(CancellationToken.None);
            Assert.False(result.IsSuccess);
            Assert.Null(result.Snapshot);
        }
    }

    [Fact]
    public async Task Issue481RuntimeVersionsKeepSeparateProcessesAndUnavailableState()
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            Assert.Equal("/v1/runtime", request.RequestUri!.AbsolutePath);
            return JsonResponse("""
                {"api_version":"v1","rest_version":"1.2.3","recorder_version":"1.2.2","recorder_status":"mismatch"}
                """, includePublishedPair: false);
        }));
        // Dynamic dispatch keeps the new operation's missing implementation a
        // behavioral RED rather than a compilation failure.
        dynamic result = await ((dynamic)client).FetchRuntimeVersionsAsync(CancellationToken.None);
        Assert.True((bool)result.IsSuccess);
        Assert.Equal("1.2.3", (string)result.Snapshot.RestVersion);
        Assert.Equal("1.2.2", (string)result.Snapshot.RecorderVersion);
        Assert.Equal("mismatch", (string)result.Snapshot.RecorderStatus);

        using var unavailable = new LoopbackStatusClient(new StubHandler(_ => JsonResponse("""
            {"api_version":"v1","rest_version":"1.2.3","recorder_version":null,"recorder_status":"unavailable"}
            """, includePublishedPair: false)));
        dynamic missing = await ((dynamic)unavailable).FetchRuntimeVersionsAsync(CancellationToken.None);
        Assert.True((bool)missing.IsSuccess);
        Assert.Null((string?)missing.Snapshot.RecorderVersion);
        Assert.Equal("unavailable", (string)missing.Snapshot.RecorderStatus);

        using var failed = new LoopbackStatusClient(new StubHandler(_ => NotFoundResponse()));
        dynamic failure = await ((dynamic)failed).FetchRuntimeVersionsAsync(CancellationToken.None);
        Assert.False((bool)failure.IsSuccess);
        Assert.Null((object?)failure.Snapshot);
    }

    [Fact]
    public async Task PublishedPairIsRetainedForDetails()
    {
        var details = await FetchDetails(ValidDetailsJson());

        Assert.True(details.IsSuccess);
        Assert.Equal(CanonicalPublishedPair, details.Snapshot!.PublishedPair?.ToString());
    }

    [Fact]
    public async Task LegacyV1AndV2InputPresentationBucketsAreNotNormalizedTwice()
    {
        var v1 = await FetchDetails(ValidDetailsJson());
        using var v2Client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri?.AbsolutePath == "/v3/details"
                ? NotFoundResponse()
                : JsonResponse(ValidDetailsV2Json(), includePublishedPair: true)));
        var v2 = await v2Client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(v1.IsSuccess);
        Assert.True(v2.IsSuccess);
        foreach (var snapshot in new[] { v1.Snapshot!, v2.Snapshot! })
        {
            var model = Assert.Single(snapshot.Models);
            Assert.Equal(10UL, model.InputTokens);
            Assert.Equal(2UL, model.CachedInputTokens);
            Assert.Equal(0.5, model.InputDollars);
            Assert.Equal(0.25, model.CachedInputDollars);
        }
    }

    [Fact]
    public async Task DetailsV3IsPreferredAndCarriesAstraHistory()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return JsonResponse(ValidDetailsV3Json(), includePublishedPair: true);
        }));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v3/details"], paths);
        Assert.Equal("v3", result.Snapshot!.ApiVersion);
        Assert.Equal("ASTRA", Assert.Single(result.Snapshot.Models).Name);
        Assert.Equal("ASTRA", Assert.Single(result.Snapshot.HistorySamples[0].Models).Name);
        Assert.Equal(ApiHistorySample.ConfirmedModelSource, result.Snapshot.HistorySamples[0].ModelSource);
        Assert.Null(result.Snapshot.HistorySamples[0].TaskActiveSincePrevious);
    }

    [Fact]
    public async Task DetailsV3PreservesConfirmedEmptyModelSet()
    {
        var json = ValidDetailsV3Json().Replace(
            "\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}]",
            "\"models\":[]",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.NotNull(sample.ModelSamples);
        Assert.Empty(sample.ModelSamples);
        Assert.Equal(ApiHistorySample.ConfirmedModelSource, sample.ModelSource);
        Assert.True(sample.ModelsComplete);
    }

    [Theory]
    [InlineData("false", false)]
    [InlineData("true", true)]
    public async Task DetailsV3CarriesOptionalTaskActivityMarker(string jsonValue, bool expected)
    {
        var json = ValidDetailsV3Json().Replace(
            "\"model_source\":\"confirmed\"",
            $"\"model_source\":\"confirmed\",\"task_active_since_previous\":{jsonValue}",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(expected, result.Snapshot!.HistorySamples[0].TaskActiveSincePrevious);
    }

    [Fact]
    public async Task DetailsV3CarriesNullTaskActivityMarker()
    {
        var json = ValidDetailsV3Json().Replace(
            "\"model_source\":\"confirmed\"",
            "\"model_source\":\"confirmed\",\"task_active_since_previous\":null",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Snapshot!.HistorySamples[0].TaskActiveSincePrevious);
    }

    [Fact]
    public async Task DetailsV3PreservesReconstructedSessionSourceAsNonConfirmed()
    {
        var json = ValidDetailsV3Json()
            .Replace(
                "\"models_complete\":true,\"model_source\":\"confirmed\"",
                "\"models_complete\":false,\"model_source\":\"reconstructed-from-session\"",
                StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.ReconstructedFromSessionModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Empty(sample.Models);
    }

    [Fact]
    public async Task DetailsV3StripsReconstructedSessionValuesEvenWhenMarkedComplete()
    {
        var json = ValidDetailsV3Json()
            .Replace(
                "\"model_source\":\"confirmed\"",
                "\"model_source\":\"reconstructed-from-session\"",
                StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.ReconstructedFromSessionModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Empty(sample.Models);
    }

    [Fact]
    public async Task DetailsV3AcceptsReconstructedSessionMetadataWithoutModelsWhenMarkedComplete()
    {
        var json = ValidDetailsV3Json()
            .Replace(
                "\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}]",
                "\"models\":null",
                StringComparison.Ordinal)
            .Replace(
                "\"model_source\":\"confirmed\"",
                "\"model_source\":\"reconstructed-from-session\"",
                StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.ReconstructedFromSessionModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Empty(sample.Models);
    }

    [Theory]
    [InlineData("future-source")]
    [InlineData(null)]
    public async Task DetailsV3KeepsQuotaMetadataButDiscardsModelsForUnknownOrMissingSource(
        string? source)
    {
        var sourceProperty = source is null ? string.Empty : $",\"model_source\":\"{source}\"";
        var json = ValidDetailsV3Json()
            .Replace(
                "\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}]",
                "\"models\":{\"malformed\":true}",
                StringComparison.Ordinal)
            .Replace(
                ",\"model_source\":\"confirmed\"",
                sourceProperty,
                StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.UnavailableModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Empty(sample.Models);
        Assert.Equal(42.5, sample.RemainingPercent);
    }

    [Fact]
    public async Task DetailsV2PreservesReconstructedSessionSourceAsNonConfirmed()
    {
        var json = ValidDetailsV2Json()
            .Replace(
                "\"model_source\":\"confirmed\"",
                "\"model_source\":\"reconstructed-from-session\"",
                StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == "/v3/details"
                ? NotFoundResponse()
                : JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.ReconstructedFromSessionModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Null(sample.SolTokens);
        Assert.All(sample.Models, model =>
        {
            Assert.Null(model.TotalTokens);
            Assert.Null(model.TotalDollars);
        });
    }

    [Fact]
    public async Task DetailsV2FixedColumnsRemainLegacyDisplayOnly()
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == "/v3/details"
                ? NotFoundResponse()
                : JsonResponse(ValidDetailsV2Json(), includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.LegacyUnknownModelSource, sample.ModelSource);
        Assert.False(sample.ModelsComplete);
        Assert.Equal(6UL, sample.SolTokens);
        Assert.Equal(1.25, sample.SolDollars);
    }

    [Fact]
    public async Task DetailsV3ReusesTheAcceptedGenerationWithAZeroBody304()
    {
        var requestCount = 0;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requestCount++;
            Assert.Equal("/v3/details", request.RequestUri!.AbsolutePath);
            if (requestCount == 1)
            {
                return JsonResponse(ValidDetailsV3Json(), includePublishedPair: true);
            }

            Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
            var response = new HttpResponseMessage(HttpStatusCode.NotModified)
            {
                Content = new ByteArrayContent([]),
            };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json")
            {
                CharSet = "utf-8",
            };
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
            return response;
        }));

        var first = await client.FetchDetailsAsync(CancellationToken.None);
        var second = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(first.IsSuccess);
        Assert.True(second.IsSuccess);
        Assert.Same(first.Snapshot, second.Snapshot);
        Assert.Equal(2, requestCount);
    }

    [Fact]
    public async Task CurrentUsesTheSplitRouteWithoutRequestingHistoryOrThreads()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return JsonResponse(ValidCurrentJson(), includePublishedPair: true);
        }));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v4/current"], paths);
        Assert.Equal("ASTRA", result.Snapshot!.Models.Single().Name);
        Assert.Equal(1UL, result.Snapshot.ActiveThreadCount);
    }

    [Theory]
    [InlineData("error", ApiState.Error)]
    [InlineData("ready", ApiState.Ready)]
    public async Task CurrentKeepsQuotaWithoutModelsAndPreservesAcquisitionState(
        string state,
        ApiState expectedState)
    {
        const string json = """
            {"api_version":"v4","models_status":"unavailable","state":"error","observed_at":1800000020,
             "authenticated":true,"plan_label":null,
             "quota":{"remaining_percent":99.0,"reset_at":1800604800,"window_seconds":604800,"monthly":false},
             "models":[],"active_thread_count":0}
            """;
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return JsonResponse(json.Replace("\"state\":\"error\"", $"\"state\":\"{state}\"", StringComparison.Ordinal),
                includePublishedPair: true);
        }));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v4/current"], paths);
        Assert.Equal(expectedState, result.Snapshot!.State);
        Assert.Equal(ApiCurrentModelsStatus.Unavailable, result.Snapshot.ModelsStatus);
        Assert.True(result.Snapshot.Authenticated);
        Assert.Equal(99.0, result.Snapshot.Quota!.RemainingPercent);
        Assert.Equal(1800604800, result.Snapshot.Quota.ResetAt);
        Assert.Empty(result.Snapshot.Models);
    }

    [Fact]
    public async Task Issue362CurrentAndThreadsPreserveStoppedOpenSession()
    {
        var currentJson = ValidCurrentJson().Replace(
            "\"active_thread_count\":1}",
            "\"active_thread_count\":0,\"open_session_thread_count\":1}",
            StringComparison.Ordinal);
        var threadsJson = "{\"api_version\":\"v3\",\"threads\":[" +
            "{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":null," +
            "\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":20," +
            "\"context_usage_tokens\":null,\"context_window_tokens\":null," +
            "\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false," +
            "\"depth\":0,\"activity_status\":\"stopped\"}]}";
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath switch
            {
                "/v4/current" => JsonResponse(currentJson, includePublishedPair: true),
                "/v3/threads" => JsonResponse(threadsJson, includePublishedPair: true),
                _ => throw new InvalidOperationException("unexpected route"),
            }));

        var current = await client.FetchCurrentAsync(CancellationToken.None);
        var threads = await client.FetchThreadsAsync(CancellationToken.None);

        Assert.True(current.IsSuccess);
        Assert.True(threads.IsSuccess);
        Assert.Equal(0UL, current.Snapshot!.ActiveThreadCount);
        Assert.Equal(1UL, current.Snapshot.OpenSessionThreadCount);
        Assert.Equal("thread-1", Assert.Single(threads.Snapshot!.Threads).Id);
        Assert.Equal(ApiThreadActivityStatus.Stopped, threads.Snapshot.Threads[0].ActivityStatus);
    }

    [Fact]
    public async Task AccountsReadsTheDirectoryWithUnknownLabelsWithoutLifecycleText()
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == "/v3/accounts"
                ? JsonResponse(ValidAccountsJson())
                : NotFoundResponse()));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal("account-7", result.Snapshot!.DefaultAccountId);
        Assert.Equal(2, result.Snapshot.Accounts.Count);
        Assert.Equal("account-7", result.Snapshot.Accounts[0].Id);
        Assert.Equal("アカウント 7 · ID未復元", result.Snapshot.Accounts[0].DisplayLabel);
        Assert.Equal("アカウント 13 · ID未復元", result.Snapshot.Accounts[1].DisplayLabel);
        Assert.DoesNotContain("email", result.Snapshot.Accounts[0].DisplayLabel, StringComparison.OrdinalIgnoreCase);
        Assert.DoesNotContain("account-7", result.Snapshot.Accounts[0].DisplayLabel, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AccountsKeepEmptyOwnershipMarkersWithoutTreatingThemAsCoverage()
    {
        const string json =
            "{\"api_version\":\"v3\",\"default_account_id\":null,\"accounts\":[" +
            "{\"id\":\"account-7\",\"is_current\":false,\"activation_at\":300,\"deactivation_at\":300," +
            "\"ownership_intervals\":[{\"start_at\":100,\"end_at\":300},{\"start_at\":150,\"end_at\":150},{\"start_at\":300,\"end_at\":300}]}]}";
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(
            [new ApiAccountOwnershipInterval(100, 300), new ApiAccountOwnershipInterval(150, 150), new ApiAccountOwnershipInterval(300, 300)],
            result.Snapshot!.Accounts.Single().OwnershipIntervals);
    }

    [Fact]
    public async Task AccountsAcceptLogoutDirectoryWithNoCurrentAccount()
    {
        const string json =
            "{\"api_version\":\"v3\",\"default_account_id\":null,\"accounts\":[" +
            "{\"id\":\"account-7\",\"is_current\":false,\"activation_at\":1789167600,\"deactivation_at\":1789168200," +
            "\"ownership_intervals\":[{\"start_at\":1789167600,\"end_at\":1789167800},{\"start_at\":1789168000,\"end_at\":1789168200}]," +
            "\"login_id\":\"previous@example.com\"}]}";
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == "/v3/accounts"
                ? JsonResponse(json)
                : NotFoundResponse()));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Snapshot!.DefaultAccountId);
        Assert.DoesNotContain(result.Snapshot.Accounts, account => account.IsCurrent);
        Assert.Equal("previous@example.com", result.Snapshot.Accounts.Single().LoginId);
        Assert.Equal(
            [new ApiAccountOwnershipInterval(1789167600, 1789167800),
                new ApiAccountOwnershipInterval(1789168000, 1789168200)],
            result.Snapshot.Accounts.Single().OwnershipIntervals);
    }

    [Fact]
    public async Task AccountsDisambiguateDuplicateLoginLabelsWithPublicIds()
    {
        const string json =
            "{\"api_version\":\"v3\",\"default_account_id\":\"account-7\",\"accounts\":[" +
            "{\"id\":\"account-7\",\"is_current\":true,\"activation_at\":1789167600,\"deactivation_at\":null}," +
            "{\"id\":\"account-13\",\"is_current\":false,\"activation_at\":null,\"deactivation_at\":null,\"login_id\":\"same@example.com\"}," +
            "{\"id\":\"account-19\",\"is_current\":false,\"activation_at\":null,\"deactivation_at\":null,\"login_id\":\"same@example.com\"}]}";
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var previous = result.Snapshot!.Accounts.Where(account => !account.IsCurrent).ToArray();
        Assert.Equal(2, previous.Length);
        Assert.All(previous, account => Assert.EndsWith($" · {account.Id}", account.DisplayLabel, StringComparison.Ordinal));
        Assert.NotEqual(previous[0].DisplayLabel, previous[1].DisplayLabel);
    }

    [Fact]
    public async Task AccountsUseTrimmedLoginIdsInCurrentAndHistoricalLabels()
    {
        const string json =
            "{\"api_version\":\"v3\",\"default_account_id\":\"account-7\",\"accounts\":[" +
            "{\"id\":\"account-7\",\"is_current\":true,\"activation_at\":1789167600,\"deactivation_at\":null,\"login_id\":\"current@example.com\"}," +
            "{\"id\":\"account-13\",\"is_current\":false,\"activation_at\":null,\"deactivation_at\":null,\"login_id\":\"previous@example.com\"}]}";
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal("current@example.com", result.Snapshot!.Accounts[0].LoginId);
        Assert.Equal("previous@example.com", result.Snapshot.Accounts[1].LoginId);
        Assert.Equal("current@example.com", result.Snapshot.Accounts[0].DisplayLabel);
        Assert.Equal("previous@example.com", result.Snapshot.Accounts[1].DisplayLabel);
        Assert.DoesNotContain("account-7", result.Snapshot.Accounts[0].DisplayLabel, StringComparison.Ordinal);
        Assert.DoesNotContain("1789167600", result.Snapshot.Accounts[0].DisplayLabel, StringComparison.Ordinal);
    }

    [Theory]
    [InlineData(" ")]
    [InlineData(" current@example.com")]
    [InlineData("current@example.com ")]
    [InlineData("current\u0007@example.com")]
    public async Task AccountsRejectInvalidOptionalLoginIds(string loginId)
    {
        var escapedLoginId = System.Text.Json.JsonSerializer.Serialize(loginId);
        var json = ValidAccountsJson().Replace(
            "\"deactivation_at\":null}",
            $"\"deactivation_at\":null,\"login_id\":{escapedLoginId}}}",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task AccountsAcceptNullOptionalLoginId()
    {
        var json = ValidAccountsJson().Replace(
            "\"deactivation_at\":null}",
            "\"deactivation_at\":null,\"login_id\":null}",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Snapshot!.Accounts[0].LoginId);
    }

    [Fact]
    public async Task AccountsRejectLoginIdsLongerThan254UnicodeScalars()
    {
        var loginId = new string('a', 255);
        var json = ValidAccountsJson().Replace(
            "\"deactivation_at\":null}",
            $"\"deactivation_at\":null,\"login_id\":{System.Text.Json.JsonSerializer.Serialize(loginId)}}}",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));

        var result = await client.FetchAccountsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task AccountsRejectsZeroAndUnknownFields()
    {
        using var zeroClient = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(ValidAccountsJson().Replace("account-7", "account-0", StringComparison.Ordinal))));
        using var unknownClient = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(ValidAccountsJson().Replace(
                "\"deactivation_at\":null}",
                "\"deactivation_at\":null,\"email\":\"private@example.com\"}",
                StringComparison.Ordinal))));

        var zero = await zeroClient.FetchAccountsAsync(CancellationToken.None);
        var unknown = await unknownClient.FetchAccountsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, zero.Failure);
        Assert.Equal(DetailsFetchFailure.Response, unknown.Failure);
    }

    [Theory]
    [InlineData("account-0")]
    [InlineData("account-01")]
    [InlineData("account-x")]
    public async Task ScopedResourcesRejectInvalidPublicAccountIdsBeforeSendingRequests(string accountId)
    {
        var requestCount = 0;
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
        {
            requestCount++;
            return JsonResponse(ValidCurrentJson(), includePublishedPair: true);
        }));

        var result = await client.FetchCurrentAsync(accountId, CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Equal(0, requestCount);
    }

    [Fact]
    public async Task ScopedCurrentUsesEncodedAccountAndDropsThePreviousConditionalCache()
    {
        var requests = new List<HttpRequestMessage>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requests.Add(request);
            return request.RequestUri!.AbsolutePath == "/v4/current"
                ? JsonResponse(ValidCurrentJson(), includePublishedPair: true)
                : NotFoundResponse();
        }));

        var first = await client.FetchCurrentAsync("account-7", CancellationToken.None);
        var second = await client.FetchCurrentAsync("account-13", CancellationToken.None);

        Assert.True(first.IsSuccess);
        Assert.True(second.IsSuccess);
        Assert.Equal("account-7", first.Snapshot!.AccountId);
        Assert.Equal("account-13", second.Snapshot!.AccountId);
        Assert.Equal("?account=account-7", requests[0].RequestUri!.Query);
        Assert.Equal("?account=account-13", requests[1].RequestUri!.Query);
        Assert.Empty(requests[1].Headers.IfNoneMatch);

        var unscopedThreads = await client.FetchThreadsAsync(CancellationToken.None);
        var unscopedCurrent = await client.FetchCurrentAsync(CancellationToken.None);
        var unscopedPeriods = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var unscopedPage = await client.FetchHistoryPageAsync(
            "period",
            cancellationToken: CancellationToken.None);
        var unscopedDetails = await client.FetchDetailsAsync(CancellationToken.None);
        Assert.Equal(DetailsFetchFailure.Response, unscopedThreads.Failure);
        Assert.Equal(DetailsFetchFailure.Response, unscopedCurrent.Failure);
        Assert.Equal(DetailsFetchFailure.Response, unscopedPeriods.Failure);
        Assert.Equal(DetailsFetchFailure.Response, unscopedPage.Failure);
        Assert.Equal(DetailsFetchFailure.Response, unscopedDetails.Failure);
        Assert.Equal(2, requests.Count);
    }

    [Fact]
    public async Task EveryScopedV3ResourceCarriesTheSelectedAccountQuery()
    {
        var requests = new List<Uri>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requests.Add(request.RequestUri!);
            return request.RequestUri!.AbsolutePath switch
            {
                "/v4/current" => JsonResponse(ValidCurrentJson(), includePublishedPair: true),
                "/v3/history/periods" => JsonResponse(
                    "{\"api_version\":\"v3\",\"history_periods\":[]}",
                    includePublishedPair: true),
                "/v3/history" => JsonResponse(ValidHistoryPageJson(), includePublishedPair: true),
                "/v3/threads" => JsonResponse(
                    "{\"api_version\":\"v3\",\"threads\":[]}",
                    includePublishedPair: true),
                _ => throw new InvalidOperationException("unexpected route"),
            };
        }));

        var current = await client.FetchCurrentAsync("account-13", CancellationToken.None);
        var periods = await client.FetchHistoryPeriodsAsync("account-13", CancellationToken.None);
        var page = await client.FetchHistoryPageAsync(
            "account-13",
            "period/opaque",
            null,
            CancellationToken.None);
        var threads = await client.FetchThreadsAsync("account-13", CancellationToken.None);

        Assert.True(current.IsSuccess);
        Assert.True(periods.IsSuccess);
        Assert.True(page.IsSuccess);
        Assert.True(threads.IsSuccess);
        Assert.Equal("account-13", current.Snapshot!.AccountId);
        Assert.Equal("account-13", periods.Snapshot!.AccountId);
        Assert.Equal("account-13", page.Page!.AccountId);
        Assert.Equal("account-13", threads.Snapshot!.AccountId);
        Assert.Equal(
            [
                "?account=account-13",
                "?account=account-13",
                "?period=period%2Fopaque&account=account-13",
                "?account=account-13",
            ],
            requests.Select(request => request.Query).ToArray());
    }

    [Fact]
    public async Task CurrentKeepsReadyUsageWhenTheOptionalPlanLabelIsUnavailable()
    {
        var json = ValidCurrentJson().Replace(
            "\"plan_label\":\"Pro\"",
            "\"plan_label\":null",
            StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Snapshot!.PlanLabel);
        Assert.NotNull(result.Snapshot.Quota);
        Assert.NotEmpty(result.Snapshot.Models);
    }

    [Fact]
    public async Task CurrentV4KeepsSuccessfulAcquisitionWithPartialModels()
    {
        var json = ValidCurrentJson().Replace(
            "\"models_status\":\"complete\"",
            "\"models_status\":\"partial\"",
            StringComparison.Ordinal);
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return JsonResponse(json, includePublishedPair: true);
        }));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.Equal(["/v4/current"], paths);
        Assert.True(result.IsSuccess);
        Assert.Equal(ApiState.Ready, result.Snapshot!.State);
        Assert.Equal(98.5, result.Snapshot.Quota!.RemainingPercent);
        Assert.Single(result.Snapshot.Models);
        Assert.Equal(ApiCurrentModelsStatus.Partial, result.Snapshot.ModelsStatus);
    }

    [Fact]
    public async Task CurrentV4RejectsMalformedMetadataWithoutFallingBack()
    {
        var valid = ValidCurrentJson();
        foreach (var json in new[]
        {
            valid.Replace("\"models_status\":\"complete\",", "", StringComparison.Ordinal),
            valid.Replace("\"models_status\":\"complete\"", "\"models_status\":\"invented\"", StringComparison.Ordinal),
            valid.Replace("\"models_status\":\"complete\"", "\"models_status\":null", StringComparison.Ordinal),
            valid.Insert(1, "\"models_status\":\"partial\","),
            valid.Insert(1, "\"unexpected\":true,"),
            valid.Replace("\"api_version\":\"v4\"", "\"api_version\":\"v3\"", StringComparison.Ordinal),
        })
        {
            var paths = new List<string>();
            using var client = new LoopbackStatusClient(new StubHandler(request =>
            {
                paths.Add(request.RequestUri!.AbsolutePath);
                return JsonResponse(json, includePublishedPair: true);
            }));
            var result = await client.FetchCurrentAsync(CancellationToken.None);
            Assert.False(result.IsSuccess);
            Assert.Equal(DetailsFetchFailure.Response, result.Failure);
            Assert.Equal(["/v4/current"], paths);
        }
    }

    [Theory]
    [InlineData(null)]
    [InlineData("account-7")]
    public async Task CurrentV4NotFoundPreservesTheAcceptedV3ConditionalCache(string? accountId)
    {
        var v3 = ValidCurrentJson()
            .Replace("\"api_version\":\"v4\"", "\"api_version\":\"v3\"", StringComparison.Ordinal)
            .Replace("\"models_status\":\"complete\",", "", StringComparison.Ordinal);
        var v3Requests = 0;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            if (request.RequestUri!.AbsolutePath == "/v4/current")
            {
                Assert.Empty(request.Headers.IfNoneMatch);
                return NotFoundResponse();
            }
            Assert.Equal("/v3/current", request.RequestUri.AbsolutePath);
            if (++v3Requests == 1)
            {
                Assert.Empty(request.Headers.IfNoneMatch);
                return JsonResponse(v3, includePublishedPair: true);
            }
            Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
            return NotModifiedResponse();
        }));
        Task<CurrentFetchResult> Fetch() => accountId is null
            ? client.FetchCurrentAsync(CancellationToken.None)
            : client.FetchCurrentAsync(accountId, CancellationToken.None);
        var first = await Fetch();
        var second = await Fetch();
        Assert.True(first.IsSuccess);
        Assert.True(second.IsSuccess);
        Assert.Same(first.Snapshot, second.Snapshot);
        Assert.Equal(2, v3Requests);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("account-7")]
    public async Task CurrentV4FallsBackToV3ForTheSameAccountOnlyAfter404(string? accountId)
    {
        var v3 = ValidCurrentJson()
            .Replace("\"api_version\":\"v4\"", "\"api_version\":\"v3\"", StringComparison.Ordinal)
            .Replace("\"models_status\":\"complete\",", "", StringComparison.Ordinal);
        var requests = new List<Uri>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requests.Add(request.RequestUri!);
            return request.RequestUri!.AbsolutePath switch
            {
                "/v4/current" => NotFoundResponse(),
                "/v3/current" => JsonResponse(v3, includePublishedPair: true),
                _ => throw new InvalidOperationException("unexpected fallback"),
            };
        }));
        var result = accountId is null
            ? await client.FetchCurrentAsync(CancellationToken.None)
            : await client.FetchCurrentAsync(accountId, CancellationToken.None);
        Assert.True(result.IsSuccess);
        Assert.Equal("v3", result.Snapshot!.ApiVersion);
        Assert.Null(result.Snapshot.ModelsStatus);
        Assert.Equal(accountId, result.Snapshot.AccountId);
        Assert.Equal(["/v4/current", "/v3/current"], requests.Select(uri => uri.AbsolutePath));
        Assert.All(requests, uri => Assert.Equal(
            accountId is null ? "" : "?account=account-7", uri.Query));
    }

    [Fact]
    public async Task CurrentFallsBackToLegacyDetailsOnlyAfterAnExact404()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return request.RequestUri.AbsolutePath switch
            {
                "/v4/current" or "/v3/current" or "/v3/details" or "/v2/details" => NotFoundResponse(),
                "/v1/details" => JsonResponse(ValidDetailsJson(), includePublishedPair: true),
                _ => throw new InvalidOperationException("unexpected route"),
            };
        }));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v4/current", "/v3/current", "/v3/details", "/v2/details", "/v1/details"], paths);
        Assert.Equal("v1", result.Snapshot!.ApiVersion);
    }

    [Fact]
    public async Task LegacyModeReusesTheLatestDetailsRootForGraphAndThreadsWithoutSplitRequests()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return request.RequestUri.AbsolutePath switch
            {
                "/v4/current" or "/v3/current" or "/v3/details" or "/v2/details" => NotFoundResponse(),
                "/v1/details" => JsonResponse(ValidDetailsJson(), includePublishedPair: true),
                _ => throw new InvalidOperationException("split route requested in legacy mode"),
            };
        }));

        var current = await client.FetchCurrentAsync(CancellationToken.None);
        var periods = await client.FetchHistoryPeriodsAsync(CancellationToken.None);
        var page = await client.FetchHistoryPageAsync(
            "253402300799",
            cancellationToken: CancellationToken.None);
        var threads = await client.FetchThreadsAsync(CancellationToken.None);

        Assert.True(current.IsSuccess);
        Assert.True(periods.IsSuccess);
        Assert.True(page.IsSuccess);
        Assert.True(threads.IsSuccess);
        Assert.Equal(["/v4/current", "/v3/current", "/v3/details", "/v2/details", "/v1/details"], paths);
    }

    [Theory]
    [InlineData("periods")]
    [InlineData("page")]
    [InlineData("threads")]
    public async Task CurrentSuccessDoesNotDowngradeWhenASplitRouteReturns404(string surface)
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            var path = request.RequestUri!.AbsolutePath;
            paths.Add(path);
            return path switch
            {
                "/v4/current" => JsonResponse(ValidCurrentJson(), includePublishedPair: true),
                "/v3/history/periods" when surface == "periods" => NotFoundResponse(),
                "/v3/history" when surface == "page" => NotFoundResponse(),
                "/v3/threads" when surface == "threads" => NotFoundResponse(),
                "/v3/details" or "/v2/details" or "/v1/details" =>
                    throw new InvalidOperationException("split 404 must not enter legacy mode"),
                _ => throw new InvalidOperationException("unexpected route"),
            };
        }));

        var current = await client.FetchCurrentAsync(CancellationToken.None);
        var failure = surface switch
        {
            "periods" => (await client.FetchHistoryPeriodsAsync(CancellationToken.None)).Failure,
            "page" => (await client.FetchHistoryPageAsync("period", cancellationToken: CancellationToken.None)).Failure,
            "threads" => (await client.FetchThreadsAsync(CancellationToken.None)).Failure,
            _ => throw new InvalidOperationException("unexpected surface"),
        };

        Assert.True(current.IsSuccess);
        Assert.Equal(DetailsFetchFailure.Response, failure);
        Assert.Equal(
            ["/v4/current", surface switch
            {
                "periods" => "/v3/history/periods",
                "page" => "/v3/history",
                "threads" => "/v3/threads",
                _ => throw new InvalidOperationException("unexpected surface"),
            }],
            paths);
    }

    [Fact]
    public async Task HistoryPageCacheRetainsOnlyTheNewestAcceptedCursorGeneration()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(ValidHistoryPageJson(), includePublishedPair: true)));

        var first = await client.FetchHistoryPageAsync(
            "period",
            "cursor-a",
            CancellationToken.None);
        var second = await client.FetchHistoryPageAsync(
            "period",
            "cursor-b",
            CancellationToken.None);

        Assert.True(first.IsSuccess);
        Assert.True(second.IsSuccess);
        var field = typeof(LoopbackStatusClient).GetField(
            "_lastHistoryPages",
            BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.NotNull(field);
        var pages = Assert.IsType<Dictionary<string, ApiHistoryPage>>(field!.GetValue(client));
        Assert.Single(pages);
        Assert.Contains("cursor-b", pages.Keys.Single(), StringComparison.Ordinal);
    }

    [Fact]
    public async Task Current304ReusesOnlyTheAcceptedCurrentGeneration()
    {
        var requestCount = 0;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requestCount++;
            Assert.Equal("/v4/current", request.RequestUri!.AbsolutePath);
            if (requestCount == 1)
            {
                return JsonResponse(ValidCurrentJson(), includePublishedPair: true);
            }

            Assert.Equal($"\"{CanonicalPublishedPair}\"", Assert.Single(request.Headers.IfNoneMatch).Tag);
            return NotModifiedResponse();
        }));

        var first = await client.FetchCurrentAsync(CancellationToken.None);
        var second = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(first.IsSuccess);
        Assert.True(second.IsSuccess);
        Assert.Same(first.Snapshot, second.Snapshot);
        Assert.Equal(2, requestCount);
    }

    [Theory]
    [InlineData("daemon_stop_unrecoverable")]
    [InlineData("reset_hint_expired")]
    [InlineData("auth_epoch_tombstoned")]
    public async Task HistoryPageRetainsGapRowsWithoutPeriodMetadata(string reason)
    {
        var gaps = $$"""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"{{reason}}"},{"gap_id":"fedcba9876543210fedcba9876543210","reset_at":200,"start_at":130,"end_at":140,"reason":"{{reason}}"},{"gap_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","reset_at":201,"start_at":100,"end_at":120,"reason":"{{reason}}"}]""";
        var result = await FetchHistoryPageWithGaps(gaps);

        Assert.True(result.IsSuccess);
        Assert.Equal("period/opaque", result.Page!.PeriodId);
        Assert.Equal(3, result.Page.HistoryGaps.Count);
        Assert.Equal(new ApiHistoryGap("0123456789abcdef0123456789abcdef", 200, 100, 120, reason), result.Page.HistoryGaps[0]);
        Assert.Equal(new ApiHistoryGap("fedcba9876543210fedcba9876543210", 200, 130, 140, reason), result.Page.HistoryGaps[1]);
        Assert.Equal(new ApiHistoryGap("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 201, 100, 120, reason), result.Page.HistoryGaps[2]);
    }

    [Theory]
    [InlineData("null")]
    [InlineData("{}")]
    [InlineData("[null]")]
    [InlineData("""[{"gap_id":"0123456789ABCDEF0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcde","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"g123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":-1,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100.5,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":253402300800,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":121,"end_at":120,"reason":"daemon_stop_unrecoverable"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"not-a-reason"}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable","extra":true}]""")]
    [InlineData("""[{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable","reason":"reset_hint_expired"}]""")]
    public async Task HistoryPageRejectsInvalidGapRows(string gaps)
    {
        var result = await FetchHistoryPageWithGaps(gaps);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Page);
    }

    [Theory]
    [InlineData("duplicate-id")]
    [InlineData("misordered-time")]
    [InlineData("misordered-reset")]
    [InlineData("overlap")]
    [InlineData("touch")]
    public async Task HistoryPageRejectsDuplicateMisorderedOrOverlappingGapRows(string violation)
    {
        const string first = """{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":100,"end_at":120,"reason":"daemon_stop_unrecoverable"}""";
        var second = violation switch
        {
            "duplicate-id" => """{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":200,"start_at":130,"end_at":140,"reason":"reset_hint_expired"}""",
            "misordered-time" => """{"gap_id":"fedcba9876543210fedcba9876543210","reset_at":200,"start_at":50,"end_at":80,"reason":"reset_hint_expired"}""",
            "misordered-reset" => """{"gap_id":"fedcba9876543210fedcba9876543210","reset_at":199,"start_at":130,"end_at":140,"reason":"reset_hint_expired"}""",
            "overlap" => """{"gap_id":"fedcba9876543210fedcba9876543210","reset_at":200,"start_at":110,"end_at":130,"reason":"reset_hint_expired"}""",
            "touch" => """{"gap_id":"fedcba9876543210fedcba9876543210","reset_at":200,"start_at":120,"end_at":140,"reason":"reset_hint_expired"}""",
            _ => throw new InvalidOperationException("unexpected gap violation"),
        };
        var result = await FetchHistoryPageWithGaps($"[{first},{second}]");

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Page);
    }

    [Theory]
    [InlineData(4096, true)]
    [InlineData(4097, false)]
    public async Task HistoryPageGapArrayLimitIsEnforced(int count, bool accepted)
    {
        var rows = Enumerable.Range(0, count)
            .Select(index => $$"""{"gap_id":"{{index:x32}}","reset_at":10000,"start_at":{{index * 2 + 1}},"end_at":{{index * 2 + 1}},"reason":"daemon_stop_unrecoverable"}""");
        var result = await FetchHistoryPageWithGaps($"[{string.Join(",", rows)}]");

        Assert.Equal(accepted, result.IsSuccess);
        if (accepted)
        {
            Assert.Equal(count, result.Page!.HistoryGaps.Count);
        }
        else
        {
            Assert.Equal(DetailsFetchFailure.Response, result.Failure);
            Assert.Null(result.Page);
        }
    }

    [Theory]
    [InlineData("v1", 253402300738L, 253402300620L, 253402300640L)]
    [InlineData("v3", 253402300738L, 253402300620L, 253402300640L)]
    [InlineData("v1", 253402300799L, 253341820739L, 253402300640L)]
    [InlineData("v3", 253402300799L, 253341820739L, 253402300640L)]
    [InlineData("v1", 253402300799L, 253402300620L, 253402300741L)]
    [InlineData("v3", 253402300799L, 253402300620L, 253402300741L)]
    public async Task DetailsHistoryGapsRequirePeriodMembership(string version, long resetAt, long startAt, long endAt)
    {
        var gap = $$"""{"gap_id":"0123456789abcdef0123456789abcdef","reset_at":{{resetAt}},"start_at":{{startAt}},"end_at":{{endAt}},"reason":"daemon_stop_unrecoverable"}""";
        var json = (version == "v3" ? ValidDetailsV3Json() : ValidDetailsJson())
            .Replace("\"history_gaps\":[]", $"\"history_gaps\":[{gap}]", StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            version == "v1" && request.RequestUri!.AbsolutePath != "/v1/details"
                ? NotFoundResponse()
                : JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task HistoryPageUsesOpaquePeriodAndCursorAndAcceptsExplicitNullCompletionCursor()
    {
        var requests = new List<Uri>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            requests.Add(request.RequestUri!);
            return JsonResponse(ValidHistoryPageJson(), includePublishedPair: true);
        }));

        var result = await client.FetchHistoryPageAsync(
            "period/opaque",
            "cursor+opaque",
            CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal("period/opaque", result.Page!.PeriodId);
        Assert.Null(result.Page.NextCursor);
        Assert.Equal("resume-opaque", result.Page.ResumeCursor);
        Assert.Equal(
            "http://127.0.0.1:8787/v3/history?period=period%2Fopaque&cursor=cursor%2Bopaque",
            requests.Single().AbsoluteUri);
    }

    [Fact]
    public async Task HistoryPageRecognizesOnlyTheExactBoundedStaleCursorResponseForASavedCursor()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"stale_cursor\"}")));

        var result = await client.FetchHistoryPageAsync(
            "period",
            "saved-cursor",
            CancellationToken.None);

        Assert.False(result.IsSuccess);
        Assert.Null(result.Page);
        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.True(result.CursorRejected);
    }

    [Fact]
    public async Task HistoryPageDoesNotTreatMalformedOrCursorlessBadRequestsAsStaleCursor()
    {
        var cases = new Func<HttpResponseMessage>[]
        {
            () => StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"stale_cursor\",\"extra\":true}"),
            () => StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"other\"}"),
            () =>
            {
                var response = StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"stale_cursor\"}");
                response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
                return response;
            },
            () =>
            {
                var response = StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"stale_cursor\"}");
                response.Headers.CacheControl = null;
                return response;
            },
        };

        foreach (var responseFactory in cases)
        {
            using var client = new LoopbackStatusClient(new StubHandler(_ => responseFactory()));
            var malformed = await client.FetchHistoryPageAsync(
                "period",
                "saved-cursor",
                CancellationToken.None);
            Assert.False(malformed.CursorRejected);
            Assert.Equal(DetailsFetchFailure.Transport, malformed.Failure);
        }

        using var cursorlessClient = new LoopbackStatusClient(new StubHandler(_ =>
            StaleCursorResponse("{\"api_version\":\"v1\",\"error\":\"stale_cursor\"}")));
        var cursorless = await cursorlessClient.FetchHistoryPageAsync(
            "period",
            cancellationToken: CancellationToken.None);
        Assert.False(cursorless.CursorRejected);
        Assert.Equal(DetailsFetchFailure.Transport, cursorless.Failure);
    }

    [Fact]
    public async Task SplitResourceRequiresTheResumeCursorOnEveryHistoryPage()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(
                ValidHistoryPageJson().Replace(
                    ",\"resume_cursor\":\"resume-opaque\"",
                    String.Empty,
                    StringComparison.Ordinal),
                includePublishedPair: true)));

        var result = await client.FetchHistoryPageAsync("period", cancellationToken: CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Page);
    }

    [Fact]
    public async Task EmptyInitialHistoryAcceptsAnExplicitNullResumeCursor()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(
                "{\"api_version\":\"v3\",\"history_samples\":[],\"history_gaps\":[],\"next_cursor\":null,\"resume_cursor\":null}",
                includePublishedPair: true)));

        var result = await client.FetchHistoryPageAsync(
            "period",
            cancellationToken: CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Page!.NextCursor);
        Assert.Null(result.Page.ResumeCursor);
    }

    [Fact]
    public async Task DetailsFallsBackToV1OnlyWhenV3AndV2ReturnNotFound()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return request.RequestUri.AbsolutePath is "/v3/details" or "/v2/details"
                ? NotFoundResponse()
                : JsonResponse(ValidDetailsJson(), includePublishedPair: true);
        }));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v3/details", "/v2/details", "/v1/details"], paths);
        Assert.Equal("v1", result.Snapshot!.ApiVersion);
        Assert.Equal(ApiHistorySample.LegacyUnknownModelSource, result.Snapshot.HistorySamples[0].ModelSource);
    }

    [Fact]
    public async Task DetailsV2DiscardsUnavailableModelVectorWithoutRejectingQuotaMetadata()
    {
        var json = ValidDetailsV2Json()
            .Replace("\"model_source\":\"confirmed\"", "\"model_source\":\"unavailable\"", StringComparison.Ordinal)
            .Replace("\"sol_dollars\":1.25", "\"sol_dollars\":null", StringComparison.Ordinal);
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            if (request.RequestUri.AbsolutePath == "/v3/details")
            {
                return NotFoundResponse();
            }
            return JsonResponse(json, includePublishedPair: true);
        }));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.UnavailableModelSource, sample.ModelSource);
        Assert.Null(sample.SolDollars);
        Assert.Null(sample.SolTokens);
        Assert.Equal(42.5, sample.RemainingPercent);
        Assert.Equal(["/v3/details", "/v2/details"], paths);
    }

    [Theory]
    [InlineData("future-source")]
    [InlineData(null)]
    public async Task DetailsV2KeepsQuotaMetadataButDiscardsModelsForUnknownOrMissingSource(
        string? source)
    {
        var sourceProperty = source is null ? string.Empty : $",\"model_source\":\"{source}\"";
        var json = ValidDetailsV2Json()
            .Replace("\"sol_dollars\":1.25", "\"sol_dollars\":{\"malformed\":true}", StringComparison.Ordinal)
            .Replace(",\"model_source\":\"confirmed\"", sourceProperty, StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == "/v3/details"
                ? NotFoundResponse()
                : JsonResponse(json, includePublishedPair: true)));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var sample = Assert.Single(result.Snapshot!.HistorySamples);
        Assert.Equal(ApiHistorySample.UnavailableModelSource, sample.ModelSource);
        Assert.Null(sample.SolDollars);
        Assert.Null(sample.SolTokens);
        Assert.Equal(42.5, sample.RemainingPercent);
    }

    [Fact]
    public async Task PublishedPairIsRequiredForDetails()
    {
        var details = await FetchDetailsWithoutPublishedPair(ValidDetailsJson());

        Assert.Equal(DetailsFetchFailure.Response, details.Failure);
        Assert.Null(details.Snapshot);
    }

    [Fact]
    public async Task PublishedPairRejectsMalformedAndNonCanonicalValues()
    {
        var invalidValues = new[]
        {
            "",
            "v1",
            "v2:00112233445566778899aabbccddeeff00000000000000000000000000000001",
            "V1:00112233445566778899aabbccddeeff00000000000000000000000000000001",
            "v1:00112233445566778899aabbccddeeff0000000000000000000000000000000A",
            "v1:00112233445566778899aabbccddeeff0000000000000000000000000000000g",
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001 ",
            CanonicalPublishedPair + "," + CanonicalPublishedPair,
        };

        foreach (var value in invalidValues)
        {
            var details = await FetchDetailsWithPairValues(ValidDetailsJson(), value);

            Assert.Equal(DetailsFetchFailure.Response, details.Failure);
            Assert.Null(details.Snapshot);
        }

        var duplicateDetails = await FetchDetailsWithPairValues(
            ValidDetailsJson(),
            CanonicalPublishedPair,
            CanonicalPublishedPair);

        Assert.Equal(DetailsFetchFailure.Response, duplicateDetails.Failure);
        Assert.Null(duplicateDetails.Snapshot);
    }

    [Fact]
    public async Task PublishedPairHeaderNameIsCaseInsensitive()
    {
        var details = await FetchDetailsWithPairValues(
            ValidDetailsJson(),
            CanonicalPublishedPair,
            headerName: PublishedPairHeader.ToUpperInvariant());

        Assert.True(details.IsSuccess);
    }

    [Fact]
    public async Task HealthDoesNotRequirePublishedPair()
    {
        var withoutPair = await FetchHealth(HealthJson());

        var withPairHandler = new StubHandler(_ => JsonResponse(HealthJson(), includePublishedPair: true));
        using var withPairClient = new LoopbackStatusClient(withPairHandler);
        var withPair = await withPairClient.FetchHealthAsync(CancellationToken.None);

        Assert.True(withoutPair.IsSuccess);
        Assert.True(withPair.IsSuccess);
        Assert.Equal(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version), withoutPair.Snapshot);
        Assert.Equal(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version), withPair.Snapshot);
    }

    [Fact]
    public async Task HealthEndpointAndMethodAreFixedAndStrictlyParsed()
    {
        var handler = new StubHandler(request =>
        {
            Assert.Equal(HttpMethod.Get, request.Method);
            Assert.Equal("http://127.0.0.1:8787/v1/health", request.RequestUri!.AbsoluteUri);
            return JsonResponse(HealthJson());
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchHealthAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version), result.Snapshot);
    }

    [Fact]
    public async Task HealthRequiresHttp200()
    {
        var handler = new StubHandler(_ =>
        {
            var response = JsonResponse(HealthJson());
            response.StatusCode = HttpStatusCode.Created;
            return response;
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchHealthAsync(CancellationToken.None);

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task HealthRequiresDeclaredContentLength()
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new UnknownLengthContent(Encoding.UTF8.GetBytes(HealthJson())),
        };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };

        using var client = new LoopbackStatusClient(new StubHandler(_ => response));
        var result = await client.FetchHealthAsync(CancellationToken.None);

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task HealthRejectsDeclaredContentLengthMismatch()
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new DeclaredLengthContent(HealthJson().Length),
        };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };

        using var client = new LoopbackStatusClient(new StubHandler(_ => response));
        var result = await client.FetchHealthAsync(CancellationToken.None);

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Theory]
    [InlineData("api_version", "api_version2")]
    [InlineData("service", "Service")]
    [InlineData("product_version", "product_version2")]
    public async Task HealthRejectsUnknownOrWrongFixedValues(string property, string replacement)
    {
        var json = HealthJson().Replace($"\"{property}\":", $"\"{replacement}\":", StringComparison.Ordinal);
        var result = await FetchHealth(json);

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task HealthRejectsMissingProductVersion()
    {
        var result = await FetchHealth("{\"api_version\":\"v1\",\"service\":\"codex-info\"}");

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task HealthAcceptsMismatchedProductVersionForDiagnostics()
    {
        var result = await FetchHealth(HealthJson("0.0.0"));

        Assert.Null(result.Failure);
        Assert.Equal("0.0.0", result.Snapshot?.ProductVersion);
    }

    [Theory]
    [InlineData("1.0")]
    [InlineData("01.0.0")]
    [InlineData("1.0.0-mismatch")]
    [InlineData("1.0.110-beta.0.1")]
    [InlineData("1.0.110-beta.07.1")]
    [InlineData("1.0.110-beta.7.01")]
    [InlineData("1.0.110-beta.7.1+sha")]
    public async Task HealthRejectsMalformedProductVersion(string productVersion)
    {
        var result = await FetchHealth(HealthJson(productVersion));

        Assert.Equal(HealthFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsRequiresHttp200()
    {
        var handler = new StubHandler(_ =>
        {
            var response = JsonResponse(ValidDetailsJson());
            response.StatusCode = HttpStatusCode.Created;
            return response;
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Transport, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task ContentTypeMustBeApplicationJson()
    {
        var handler = new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent(ValidDetailsJson(), Encoding.UTF8, "text/plain"),
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Theory]
    [InlineData(false, true)]
    [InlineData(true, false)]
    public async Task Utf8AndNoStoreAreBothRequired(bool includeUtf8, bool includeNoStore)
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent(ValidDetailsJson(), Encoding.UTF8, "application/json"),
        };
        if (!includeUtf8)
        {
            response.Content.Headers.ContentType!.CharSet = null;
        }
        if (includeNoStore)
        {
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        }
        using var client = new LoopbackStatusClient(new StubHandler(_ => response));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsNon2xxAndTransportFailuresAreClassifiedWithoutPrivateDetails()
    {
        var nonSuccess = new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.BadGateway)
        {
            Content = new StringContent("not public", Encoding.UTF8, "text/plain"),
        });
        var thrown = new StubHandler(_ => throw new HttpRequestException("private detail"));

        using var nonSuccessClient = new LoopbackStatusClient(nonSuccess);
        using var thrownClient = new LoopbackStatusClient(thrown);
        var nonSuccessResult = await nonSuccessClient.FetchDetailsAsync(CancellationToken.None);
        var thrownResult = await thrownClient.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Transport, nonSuccessResult.Failure);
        Assert.Null(nonSuccessResult.Snapshot);
        Assert.Equal(DetailsFetchFailure.Transport, thrownResult.Failure);
        Assert.Null(thrownResult.Snapshot);
    }

    [Fact]
    public async Task DetailsEndpointParsesHistoryThreadsAndSamplesStrictly()
    {
        var handler = new StubHandler(request =>
        {
            Assert.Equal(HttpMethod.Get, request.Method);
            if (request.RequestUri!.AbsolutePath is "/v3/details" or "/v2/details")
            {
                return NotFoundResponse();
            }

            Assert.Equal("http://127.0.0.1:8787/v1/details", request.RequestUri!.AbsoluteUri);
            return JsonResponse(ValidDetailsJson(), includePublishedPair: true);
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        var details = Assert.IsType<ApiDetailsSnapshot>(result.Snapshot);
        Assert.Single(details.Models);
        Assert.Equal(1.25, details.Models[0].TotalDollars);
        Assert.Single(details.HistoryPeriods);
        Assert.Single(details.HistoryPeriods[0].Samples);
        Assert.Equal(42.5, details.HistoryPeriods[0].Samples[0].RemainingPercent);
        Assert.Single(details.Threads);
        Assert.Equal("SOL", details.Threads[0].Model);
        Assert.Equal("Pro", details.PlanLabel);
        Assert.Equal("概算 $1", details.EstimatedCostLabel);
    }

    [Theory]
    [InlineData("ready", false)]
    [InlineData("auth_required", true)]
    public async Task DetailsRejectsInconsistentStateAuthenticationDomain(
        string state,
        bool authenticated)
    {
        var json = ValidDetailsJson()
            .Replace("\"state\":\"ready\"", $"\"state\":\"{state}\"", StringComparison.Ordinal)
            .Replace("\"authenticated\":true", $"\"authenticated\":{authenticated.ToString().ToLowerInvariant()}", StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Theory]
    [InlineData("無料", false)]
    [InlineData("Go", false)]
    [InlineData("Plus", false)]
    [InlineData("Pro", false)]
    [InlineData("Pro Lite", false)]
    [InlineData("Team", false)]
    [InlineData("Business", false)]
    [InlineData("エンタープライズ", true)]
    [InlineData("教育", false)]
    [InlineData("プラン未設定", false)]
    public async Task DetailsAcceptsCanonicalPlanMonthlyDomain(string planLabel, bool monthly)
    {
        var json = ValidDetailsJson()
            .Replace("\"plan_label\":\"Pro\"", $"\"plan_label\":\"{planLabel}\"", StringComparison.Ordinal)
            .Replace("\"monthly\":false", $"\"monthly\":{monthly.ToString().ToLowerInvariant()}", StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Equal(planLabel, result.Snapshot!.PlanLabel);
        Assert.Equal(monthly, result.Snapshot.Quota!.Monthly);
    }

    [Fact]
    public async Task DetailsAcceptsUnavailableOptionalPlanWithAuthenticatedUsage()
    {
        var json = ValidDetailsJson().Replace(
            "\"plan_label\":\"Pro\"",
            "\"plan_label\":null",
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Null(result.Snapshot!.PlanLabel);
        Assert.NotNull(result.Snapshot.Quota);
        Assert.NotEmpty(result.Snapshot.Models);
    }

    [Theory]
    [InlineData("\"pro\"", false)]
    [InlineData("\"Enterprise\"", true)]
    [InlineData("\"エンタープライズ\"", false)]
    [InlineData("\"Pro\"", true)]
    public async Task DetailsRejectsNonCanonicalPlanOrMonthlyDomain(
        string planLabelJson,
        bool monthly)
    {
        var json = ValidDetailsJson()
            .Replace("\"plan_label\":\"Pro\"", $"\"plan_label\":{planLabelJson}", StringComparison.Ordinal)
            .Replace("\"monthly\":false", $"\"monthly\":{monthly.ToString().ToLowerInvariant()}", StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task OpaqueHistoryPeriodIdStillJoinsSamplesByResetBoundary()
    {
        var json = ValidDetailsJson().Replace(
            "\"id\":\"253402300799\"",
            "\"id\":\"current-period\"",
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Single(result.Snapshot!.HistoryPeriods[0].Samples);
        Assert.Equal(253_402_300_799, result.Snapshot.HistoryPeriods[0].ResetAt);
    }

    [Theory]
    [InlineData("v1")]
    [InlineData("v2")]
    [InlineData("v3")]
    public async Task OfficialResetIdentitiesOneSecondApartStaySeparate(string version)
    {
        var json = System.Text.Json.Nodes.JsonNode.Parse(version switch
        {
            "v3" => ValidDetailsV3Json(),
            "v2" => ValidDetailsV2Json(),
            _ => ValidDetailsJson(),
        })!;
        const long oldReset = 253_402_300_798;
        var periods = json["history_periods"]!.AsArray();
        var oldPeriod = periods[0]!.DeepClone();
        oldPeriod["id"] = oldReset.ToString(System.Globalization.CultureInfo.InvariantCulture);
        oldPeriod["reset_at"] = oldReset;
        oldPeriod["start_at"] = 253_402_300_620L;
        oldPeriod["end_at"] = 253_402_300_620L;
        oldPeriod["current"] = false;
        periods.Insert(0, oldPeriod);
        var samples = json["history_samples"]!.AsArray();
        var oldSample = samples[0]!.DeepClone();
        oldSample["reset_at"] = oldReset;
        oldSample["timestamp"] = 253_402_300_620L;
        oldSample["remaining_percent"] = 10.0;
        samples.Insert(0, oldSample);
        json["quota"]!["remaining_percent"] = 100.0;
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri!.AbsolutePath == $"/{version}/details"
                ? JsonResponse(json.ToJsonString(), includePublishedPair: true)
                : NotFoundResponse()));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(100.0, result.Snapshot!.Quota!.RemainingPercent);
        Assert.Equal(2, result.Snapshot.HistoryPeriods.Count);
        Assert.All(result.Snapshot.HistoryPeriods, period =>
        {
            var sample = Assert.Single(period.Samples);
            Assert.Equal(period.ResetAt, sample.ResetAt);
        });
    }

    [Fact]
    public async Task ResetJitterCanonicalCollisionRejectsTheCompleteDetailsGeneration()
    {
        const string original = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        const string jittered = "{\"timestamp\":253402300680,\"reset_at\":253402300739,\"remaining_percent\":null,\"sol_dollars\":2.0,\"terra_dollars\":3.0,\"luna_dollars\":4.0,\"sol_tokens\":12,\"terra_tokens\":13,\"luna_tokens\":14}";
        var json = ValidDetailsJson().Replace(
            original,
            jittered + "," + original,
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task ResetJitterAtDifferentMinutesRemainsTwoCanonicalSamples()
    {
        const string original = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        const string distinctMinute = "{\"timestamp\":253402300620,\"reset_at\":253402300739,\"remaining_percent\":14.0,\"sol_dollars\":9.0,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":9,\"terra_tokens\":0,\"luna_tokens\":0}";
        var json = ValidDetailsJson().Replace(
            original,
            distinctMinute + "," + original,
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Equal(2, result.Snapshot!.HistoryPeriods[0].Samples.Count);
        Assert.All(result.Snapshot.HistoryPeriods[0].Samples, sample => Assert.Equal(253_402_300_799, sample.ResetAt));
        Assert.Equal(14.0, result.Snapshot.HistoryPeriods[0].Samples[0].RemainingPercent);
        Assert.Equal(42.5, result.Snapshot.HistoryPeriods[0].Samples[1].RemainingPercent);
    }

    [Fact]
    public async Task DuplicateHistorySampleIdentitiesRejectTheCompleteDetailsGeneration()
    {
        const string original = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        const string secondObservation = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":0.0,\"terra_dollars\":0.0,\"luna_dollars\":2.0,\"sol_tokens\":0,\"terra_tokens\":0,\"luna_tokens\":9}";
        var json = ValidDetailsJson().Replace(
            original,
            original + "," + secondObservation,
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DuplicateIdentityWithQuotaOnlyConflictRejectsInsteadOfInventingHistory()
    {
        const string original = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        const string quotaOnlyConflict = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":14.0,\"sol_dollars\":0.0,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":0,\"terra_tokens\":0,\"luna_tokens\":0}";
        var json = ValidDetailsJson().Replace(
            original,
            original + "," + quotaOnlyConflict,
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Theory]
    [InlineData(30)]
    [InlineData(60)]
    public async Task MovingResetAtDifferentMinutesRemainsReachable(int driftSeconds)
    {
        const string original = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        const string spend = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":88.0,\"sol_dollars\":1.0,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        var collision = $"{{\"timestamp\":253402300620,\"reset_at\":{253402300799L - driftSeconds},\"remaining_percent\":14.0,\"sol_dollars\":0.0,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":0,\"terra_tokens\":0,\"luna_tokens\":0}}";
        var later = "{\"timestamp\":253402300740,\"reset_at\":253402300799,\"remaining_percent\":87.0,\"sol_dollars\":2.0,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":7,\"terra_tokens\":0,\"luna_tokens\":0}";
        var json = ValidDetailsJson().Replace(
            original,
            collision + "," + spend + "," + later,
            StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        var samples = result.Snapshot!.HistoryPeriods[0].Samples;
        Assert.Equal(3, samples.Count);
        Assert.Equal(14.0, samples[0].RemainingPercent);
        Assert.Equal(88.0, samples[1].RemainingPercent);
        Assert.Equal(87.0, samples[2].RemainingPercent);
    }

    [Theory]
    [InlineData("unknown")]
    [InlineData("models")]
    public async Task DetailsUnknownOrDuplicateKeysAreRejected(string field)
    {
        var json = field == "unknown"
            ? ValidDetailsJson().Replace("\"estimated_cost_label\":", "\"unknown\":1,\"estimated_cost_label\":", StringComparison.Ordinal)
            : ValidDetailsJson().Replace(
                "\"models\":[",
                "\"models\":[",
                StringComparison.Ordinal)
                .Replace(
                    "],\"active_thread_count\":",
                    "],\"models\":[],\"active_thread_count\":",
                    StringComparison.Ordinal);
        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsHistoryGapsIsRequiredAndTopLevelKeysRemainExact()
    {
        var missing = await FetchDetails(ValidDetailsJson().Replace(
            ",\"history_gaps\":[]",
            "",
            StringComparison.Ordinal));
        var unknown = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            "\"history_gaps\":[],\"unknown\":1",
            StringComparison.Ordinal));
        var duplicate = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            "\"history_gaps\":[],\"history_gaps\":[]",
            StringComparison.Ordinal));
        var caseChanged = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            "\"history_Gaps\":[]",
            StringComparison.Ordinal));

        Assert.All(
            new[] { missing, unknown, duplicate, caseChanged },
            result =>
            {
                Assert.Equal(DetailsFetchFailure.Response, result.Failure);
                Assert.Null(result.Snapshot);
            });
    }

    [Fact]
    public async Task ValidHistoryGapIsRetainedInTheDetailsDomain()
    {
        var result = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            "\"history_gaps\":[{\"gap_id\":\"0123456789abcdef0123456789abcdef\",\"reset_at\":253402300799,\"start_at\":253402300620,\"end_at\":253402300640,\"reason\":\"daemon_stop_unrecoverable\"}]",
            StringComparison.Ordinal));

        Assert.True(result.IsSuccess);
        var gap = Assert.Single(result.Snapshot!.HistoryGaps);
        Assert.Equal("0123456789abcdef0123456789abcdef", gap.GapId);
        Assert.Equal(253402300799, gap.ResetAt);
        Assert.Equal(253402300620, gap.StartAt);
        Assert.Equal(253402300640, gap.EndAt);
        Assert.Equal("daemon_stop_unrecoverable", gap.Reason);
    }

    [Theory]
    [InlineData("gap_id", "0123456789ABCDEF0123456789abcdef")]
    [InlineData("reason", "not-a-reason")]
    [InlineData("start_at", "253402300650")]
    [InlineData("reset_at", "253402300738")]
    public async Task InvalidHistoryGapIsRejectedAsACompleteGeneration(string field, string value)
    {
        var gap = "{\"gap_id\":\"0123456789abcdef0123456789abcdef\",\"reset_at\":253402300799,\"start_at\":253402300620,\"end_at\":253402300640,\"reason\":\"daemon_stop_unrecoverable\"}";
        var json = ValidDetailsJson().Replace("\"history_gaps\":[]", $"\"history_gaps\":[{gap}]", StringComparison.Ordinal);
        if (field == "gap_id")
        {
            json = json.Replace("0123456789abcdef0123456789abcdef", value, StringComparison.Ordinal);
        }
        else if (field == "reason")
        {
            json = json.Replace("daemon_stop_unrecoverable", value, StringComparison.Ordinal);
        }
        else
        {
            json = json.Replace($"\"{field}\":{(field == "start_at" ? "253402300620" : "253402300799")}", $"\"{field}\":{value}", StringComparison.Ordinal);
        }

        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task OverlappingOrMisorderedHistoryGapsAreRejected()
    {
        const string first = "{\"gap_id\":\"0123456789abcdef0123456789abcdef\",\"reset_at\":253402300799,\"start_at\":253402300620,\"end_at\":253402300640,\"reason\":\"daemon_stop_unrecoverable\"}";
        const string second = "{\"gap_id\":\"fedcba9876543210fedcba9876543210\",\"reset_at\":253402300799,\"start_at\":253402300635,\"end_at\":253402300650,\"reason\":\"reset_hint_expired\"}";
        const string reversed = "{\"gap_id\":\"fedcba9876543210fedcba9876543210\",\"reset_at\":253402300799,\"start_at\":253402300650,\"end_at\":253402300680,\"reason\":\"reset_hint_expired\"},{\"gap_id\":\"0123456789abcdef0123456789abcdef\",\"reset_at\":253402300799,\"start_at\":253402300620,\"end_at\":253402300640,\"reason\":\"daemon_stop_unrecoverable\"}";

        var overlapping = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            $"\"history_gaps\":[{first},{second}]",
            StringComparison.Ordinal));
        var misordered = await FetchDetails(ValidDetailsJson().Replace(
            "\"history_gaps\":[]",
            $"\"history_gaps\":[{reversed}]",
            StringComparison.Ordinal));

        Assert.Equal(DetailsFetchFailure.Response, overlapping.Failure);
        Assert.Null(overlapping.Snapshot);
        Assert.Equal(DetailsFetchFailure.Response, misordered.Failure);
        Assert.Null(misordered.Snapshot);
    }

    [Theory]
    [InlineData("v1")]
    [InlineData("v2")]
    [InlineData("v3")]
    public async Task HistoryBoundaryMinuteIsAccepted(string version)
    {
        var json = (version switch
        {
            "v1" => ValidDetailsJson(),
            "v2" => ValidDetailsV2Json(),
            _ => ValidDetailsV3Json(),
        }).Replace("253402300740", "1800000080", StringComparison.Ordinal)
          .Replace("253402300799", "1800604841", StringComparison.Ordinal)
          .Replace("253341820740", "1800000041", StringComparison.Ordinal)
          .Replace("253402300680", "1800000000", StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri?.AbsolutePath == $"/{version}/details"
                ? JsonResponse(json, includePublishedPair: true) : NotFoundResponse()));
        var result = await client.FetchDetailsAsync(CancellationToken.None);
        Assert.True(result.IsSuccess, $"{version}: {result.Failure}");
        Assert.Equal(1_800_000_041, Assert.Single(result.Snapshot!.HistoryPeriods).StartAt);
        Assert.Equal(1_800_000_000, Assert.Single(result.Snapshot.HistorySamples).Timestamp);
    }

    [Theory]
    [InlineData("253402300681")]
    [InlineData("253402300800")]
    [InlineData("253402300738")]
    public async Task HistorySampleMinuteRangeAndCanonicalPeriodAreRequired(string timestamp)
    {
        var json = ValidDetailsJson().Replace("253402300680", timestamp, StringComparison.Ordinal);
        var result = await FetchDetails(json);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task CurrentPeriodRequiresObservedEndAndNonNullObservation()
    {
        var missingObservedAt = await FetchDetails(ValidDetailsJson().Replace(
            "\"observed_at\":253402300740",
            "\"observed_at\":null",
            StringComparison.Ordinal));
        var futureEnd = await FetchDetails(ValidDetailsJson().Replace(
            "\"end_at\":253402300740",
            "\"end_at\":253402300799",
            StringComparison.Ordinal));

        Assert.Equal(DetailsFetchFailure.Response, missingObservedAt.Failure);
        Assert.Null(missingObservedAt.Snapshot);
        Assert.Equal(DetailsFetchFailure.Response, futureEnd.Failure);
        Assert.Null(futureEnd.Snapshot);
    }

    [Fact]
    public async Task ThreadCycleRejectsTheCompleteDetailsGeneration()
    {
        var cycle = ValidDetailsJson().Replace(
            "\"parent_thread_id\":null",
            "\"parent_thread_id\":\"thread-1\"",
            StringComparison.Ordinal);

        var result = await FetchDetails(cycle);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task MissingParentIsAcceptedAndDerivedAsOrphan()
    {
        var orphan = ValidDetailsJson().Replace(
            "\"parent_thread_id\":null",
            "\"parent_thread_id\":\"missing-parent\"",
            StringComparison.Ordinal);

        var result = await FetchDetails(orphan);

        Assert.True(result.IsSuccess);
        var thread = Assert.Single(result.Snapshot!.Threads);
        Assert.Equal("missing-parent", thread.ParentId);
        Assert.True(thread.IsOrphan);
    }

    [Fact]
    public async Task TwoNodeThreadCycleRejectsTheCompleteDetailsGeneration()
    {
        const string first = "{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":\"thread-2\",\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":20,\"context_usage_tokens\":10,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}";
        const string second = "{\"id\":\"thread-2\",\"title\":\"Task 2\",\"parent_thread_id\":\"thread-1\",\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":21,\"context_usage_tokens\":11,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}";
        var cycle = ValidDetailsJson()
            .Replace(
                "{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":null,\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":20,\"context_usage_tokens\":10,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}",
                first + "," + second,
                StringComparison.Ordinal);

        var result = await FetchDetails(cycle);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsOversizeBodyIsRejected()
    {
        var handler = new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new DeclaredLengthContent(32L * 1024 * 1024 + 1),
        });

        using var client = new LoopbackStatusClient(handler);
        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
    }

    [Fact]
    public void DefaultHandlerDisablesProxyRedirectDecompressionAndCookies()
    {
        var method = typeof(LoopbackStatusClient).GetMethod(
            "CreateDefaultHandler",
            BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(method);
        using var handler = Assert.IsType<HttpClientHandler>(method!.Invoke(null, null));

        Assert.False(handler.UseProxy);
        Assert.False(handler.AllowAutoRedirect);
        Assert.Equal(DecompressionMethods.None, handler.AutomaticDecompression);
        Assert.False(handler.UseCookies);
        Assert.Equal(8, handler.MaxResponseHeadersLength);
    }

    [Fact]
    public void ClientTimeoutMatchesTheRestRequestBudget()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(ValidDetailsJson())));
        var field = typeof(LoopbackStatusClient).GetField("_httpClient", BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.NotNull(field);
        var httpClient = Assert.IsType<HttpClient>(field!.GetValue(client));

        Assert.Equal(TimeSpan.FromSeconds(3), httpClient.Timeout);
    }

    [Fact]
    public void DetailsHistoryCapacityIsExactlyOneThirtyOneDayMinuteWindow()
    {
        var field = typeof(LoopbackStatusClient).GetField(
            "MaxHistorySamples",
            BindingFlags.NonPublic | BindingFlags.Static);

        Assert.NotNull(field);
        Assert.Equal(44_640, field!.GetRawConstantValue());
    }

    private static async Task<HistoryPageFetchResult> FetchHistoryPageWithGaps(string gaps)
    {
        var json = ValidHistoryPageJson().Replace("\"history_gaps\":[]", $"\"history_gaps\":{gaps}", StringComparison.Ordinal);
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
            JsonResponse(json, includePublishedPair: true)));

        return await client.FetchHistoryPageAsync("period/opaque", cancellationToken: CancellationToken.None);
    }

    private static async Task<DetailsFetchResult> FetchDetails(string json)
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri?.AbsolutePath is "/v3/details" or "/v2/details"
                ? NotFoundResponse()
                : JsonResponse(json, includePublishedPair: true)));
        return await client.FetchDetailsAsync(CancellationToken.None);
    }

    private static async Task<DetailsFetchResult> FetchDetailsWithoutPublishedPair(string json)
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri?.AbsolutePath is "/v3/details" or "/v2/details"
                ? NotFoundResponse()
                : JsonResponse(json)));
        return await client.FetchDetailsAsync(CancellationToken.None);
    }

    private static async Task<DetailsFetchResult> FetchDetailsWithPairValues(
        string json,
        string firstValue,
        string? secondValue = null,
        string headerName = PublishedPairHeader)
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            if (request.RequestUri?.AbsolutePath is "/v3/details" or "/v2/details")
            {
                return NotFoundResponse();
            }

            var response = JsonResponse(json);
            response.Headers.TryAddWithoutValidation(headerName, firstValue);
            if (secondValue is not null)
            {
                response.Headers.TryAddWithoutValidation(headerName, secondValue);
            }

            return response;
        }));
        return await client.FetchDetailsAsync(CancellationToken.None);
    }

    private static async Task<HealthFetchResult> FetchHealth(string json)
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(json)));
        return await client.FetchHealthAsync(CancellationToken.None);
    }

    private static HttpResponseMessage JsonResponse(string json, bool includePublishedPair = false)
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent(json, Encoding.UTF8, "application/json"),
        };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        if (includePublishedPair)
        {
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
        }

        return response;
    }

    private static HttpResponseMessage NotFoundResponse() =>
        new(HttpStatusCode.NotFound);

    private static HttpResponseMessage StaleCursorResponse(string json)
    {
        var response = JsonResponse(json);
        response.StatusCode = HttpStatusCode.BadRequest;
        return response;
    }

    private static HttpResponseMessage NotModifiedResponse()
    {
        var response = new HttpResponseMessage(HttpStatusCode.NotModified)
        {
            Content = new ByteArrayContent([]),
        };
        response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json")
        {
            CharSet = "utf-8",
        };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
        return response;
    }

    private static string HealthJson(string? productVersion = null) =>
        $"{{\"api_version\":\"v1\",\"service\":\"codex-info\",\"product_version\":\"{productVersion ?? ProductInfo.Version}\"}}";

    private static string ValidDetailsJson() =>
        "{\"api_version\":\"v1\",\"state\":\"ready\",\"observed_at\":253402300740,\"authenticated\":true,\"plan_label\":\"Pro\",\"quota\":{\"remaining_percent\":98.5,\"reset_at\":253402300799,\"window_seconds\":604800,\"monthly\":false},\"models\":[{\"name\":\"SOL\",\"input_tokens\":10,\"cached_input_tokens\":2,\"output_tokens\":3,\"input_dollars\":0.5,\"cached_input_dollars\":0.25,\"output_dollars\":0.5}],\"active_thread_count\":1,\"history_periods\":[{\"id\":\"253402300799\",\"start_at\":253341820740,\"end_at\":253402300740,\"reset_at\":253402300799,\"label\":\"2026/08/01 — 2026/08/08\",\"current\":true}],\"history_samples\":[{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}],\"history_gaps\":[],\"threads\":[{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":null,\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":20,\"context_usage_tokens\":10,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}],\"estimated_cost_label\":\"概算 $1\"}";

    private static string ValidDetailsV2Json() =>
        ValidDetailsJson()
            .Replace("\"api_version\":\"v1\"", "\"api_version\":\"v2\"", StringComparison.Ordinal)
            .Replace(
                "\"luna_tokens\":0}],\"history_gaps\"",
                "\"luna_tokens\":0,\"model_source\":\"confirmed\"}],\"history_gaps\"",
                StringComparison.Ordinal);

    private static string ValidDetailsV3Json() =>
        "{\"api_version\":\"v3\",\"state\":\"ready\",\"observed_at\":253402300740,\"authenticated\":true,\"plan_label\":\"Pro\",\"quota\":{\"remaining_percent\":98.5,\"reset_at\":253402300799,\"window_seconds\":604800,\"monthly\":false},\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":13,\"input_tokens\":10,\"cached_input_tokens\":2,\"cache_write_input_tokens\":1,\"output_tokens\":3,\"estimated_cost\":{\"price_version\":\"ASTRA_USER_2026-09-05\",\"ordinary_input_dollars\":1.0,\"cached_input_dollars\":2.0,\"cache_write_input_dollars\":3.0,\"output_dollars\":4.0,\"total_dollars\":10.0}}],\"active_thread_count\":1,\"history_periods\":[{\"id\":\"253402300799\",\"start_at\":253341820740,\"end_at\":253402300740,\"reset_at\":253402300799,\"label\":\"2026/08/01 — 2026/08/08\",\"current\":true}],\"history_samples\":[{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}],\"models_complete\":true,\"model_source\":\"confirmed\"}],\"history_gaps\":[],\"threads\":[{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":null,\"model\":\"ASTRA\",\"model_label\":\"ASTRA\",\"total_tokens\":20,\"context_usage_tokens\":10,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}]}";

    private static string ValidCurrentJson() =>
        "{\"api_version\":\"v4\",\"models_status\":\"complete\",\"state\":\"ready\",\"observed_at\":253402300740,\"authenticated\":true,\"plan_label\":\"Pro\",\"quota\":{\"remaining_percent\":98.5,\"reset_at\":253402300799,\"window_seconds\":604800,\"monthly\":false},\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":13,\"input_tokens\":10,\"cached_input_tokens\":2,\"cache_write_input_tokens\":1,\"output_tokens\":3,\"estimated_cost\":{\"price_version\":\"ASTRA_USER_2026-09-05\",\"ordinary_input_dollars\":1.0,\"cached_input_dollars\":2.0,\"cache_write_input_dollars\":3.0,\"output_dollars\":4.0,\"total_dollars\":10.0}}],\"active_thread_count\":1}";

    private static string ValidAccountsJson() =>
        "{\"api_version\":\"v3\",\"default_account_id\":\"account-7\",\"accounts\":[{\"id\":\"account-7\",\"is_current\":true,\"activation_at\":1789167600,\"deactivation_at\":null},{\"id\":\"account-13\",\"is_current\":false,\"activation_at\":null,\"deactivation_at\":null}]}";

    private static string ValidHistoryPageJson() =>
        $"{{\"api_version\":\"v3\",\"history_samples\":[{{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"models\":[{{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}}],\"models_complete\":true,\"model_source\":\"confirmed\"}}],\"history_gaps\":[],\"next_cursor\":null,\"resume_cursor\":\"resume-opaque\"}}";

    private sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> responder, bool versionedPeriods = false, bool pagedPeriods = false) : HttpMessageHandler
    {
        public HttpRequestMessage? LastRequest { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            LastRequest = request;
            // Existing fixtures explicitly model servers without the newer routes.
            if (request.RequestUri!.AbsolutePath == "/v5/current")
                return Task.FromResult(NotFoundResponse());
            if (!pagedPeriods && request.RequestUri!.AbsolutePath.StartsWith("/v5/history/periods", StringComparison.Ordinal))
                return Task.FromResult(NotFoundResponse());
            if (!versionedPeriods && request.RequestUri!.AbsolutePath.StartsWith("/v4/history/periods", StringComparison.Ordinal))
                return Task.FromResult(NotFoundResponse());
            return Task.FromResult(responder(request));
        }
    }

    private sealed class UnknownLengthContent : HttpContent
    {
        private readonly byte[] _payload;

        public UnknownLengthContent(byte[] payload)
        {
            _payload = payload;
            Headers.ContentType = new MediaTypeHeaderValue("application/json");
        }

        protected override async Task SerializeToStreamAsync(Stream stream, TransportContext? context)
        {
            await stream.WriteAsync(_payload);
        }

        protected override bool TryComputeLength(out long length)
        {
            length = 0;
            return false;
        }

    }

    private sealed class DeclaredLengthContent : HttpContent
    {
        public DeclaredLengthContent(long length)
        {
            Headers.ContentType = new MediaTypeHeaderValue("application/json");
            Headers.ContentLength = length;
        }

        protected override Task SerializeToStreamAsync(Stream stream, TransportContext? context) =>
            Task.CompletedTask;

        protected override bool TryComputeLength(out long length)
        {
            length = Headers.ContentLength ?? 0;
            return false;
        }
    }
}
