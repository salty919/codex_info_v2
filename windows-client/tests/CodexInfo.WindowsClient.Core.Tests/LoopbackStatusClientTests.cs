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
    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const string CanonicalPublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    [Fact]
    public async Task PublishedPairIsRetainedForDetails()
    {
        var details = await FetchDetails(ValidDetailsJson());

        Assert.True(details.IsSuccess);
        Assert.Equal(CanonicalPublishedPair, details.Snapshot!.PublishedPair?.ToString());
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
    public async Task DetailsV2RejectsPartiallyNullUnavailableModelVector()
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

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
        Assert.Equal(["/v3/details", "/v2/details"], paths);
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

    [Theory]
    [InlineData("\"pro\"", false)]
    [InlineData("\"Enterprise\"", true)]
    [InlineData("\"エンタープライズ\"", false)]
    [InlineData("\"Pro\"", true)]
    [InlineData("null", false)]
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
    public void ClientTimeoutIsFixedAtOneSecond()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ => JsonResponse(ValidDetailsJson())));
        var field = typeof(LoopbackStatusClient).GetField("_httpClient", BindingFlags.NonPublic | BindingFlags.Instance);
        Assert.NotNull(field);
        var httpClient = Assert.IsType<HttpClient>(field!.GetValue(client));

        Assert.Equal(TimeSpan.FromSeconds(1), httpClient.Timeout);
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

    private sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> responder) : HttpMessageHandler
    {
        public HttpRequestMessage? LastRequest { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            LastRequest = request;
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
