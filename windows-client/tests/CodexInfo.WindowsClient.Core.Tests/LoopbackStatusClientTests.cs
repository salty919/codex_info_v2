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
        Assert.Null(result.Snapshot.HistorySamples[0].TaskActiveSincePrevious);
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
        Assert.Equal(["/v3/current"], paths);
        Assert.Equal("ASTRA", result.Snapshot!.Models.Single().Name);
        Assert.Equal(1UL, result.Snapshot.ActiveThreadCount);
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
            return request.RequestUri!.AbsolutePath == "/v3/current"
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
                "/v3/current" => JsonResponse(ValidCurrentJson(), includePublishedPair: true),
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
    public async Task CurrentFallsBackToLegacyDetailsOnlyAfterAnExact404()
    {
        var paths = new List<string>();
        using var client = new LoopbackStatusClient(new StubHandler(request =>
        {
            paths.Add(request.RequestUri!.AbsolutePath);
            return request.RequestUri.AbsolutePath switch
            {
                "/v3/current" or "/v3/details" or "/v2/details" => NotFoundResponse(),
                "/v1/details" => JsonResponse(ValidDetailsJson(), includePublishedPair: true),
                _ => throw new InvalidOperationException("unexpected route"),
            };
        }));

        var result = await client.FetchCurrentAsync(CancellationToken.None);

        Assert.True(result.IsSuccess);
        Assert.Equal(["/v3/current", "/v3/details", "/v2/details", "/v1/details"], paths);
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
                "/v3/current" or "/v3/details" or "/v2/details" => NotFoundResponse(),
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
        Assert.Equal(["/v3/current", "/v3/details", "/v2/details", "/v1/details"], paths);
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
                "/v3/current" => JsonResponse(ValidCurrentJson(), includePublishedPair: true),
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
            ["/v3/current", surface switch
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
            Assert.Equal("/v3/current", request.RequestUri!.AbsolutePath);
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
        "{\"api_version\":\"v3\",\"state\":\"ready\",\"observed_at\":253402300740,\"authenticated\":true,\"plan_label\":\"Pro\",\"quota\":{\"remaining_percent\":98.5,\"reset_at\":253402300799,\"window_seconds\":604800,\"monthly\":false},\"models\":[{\"model\":\"ASTRA\",\"total_tokens\":13,\"input_tokens\":10,\"cached_input_tokens\":2,\"cache_write_input_tokens\":1,\"output_tokens\":3,\"estimated_cost\":{\"price_version\":\"ASTRA_USER_2026-09-05\",\"ordinary_input_dollars\":1.0,\"cached_input_dollars\":2.0,\"cache_write_input_dollars\":3.0,\"output_dollars\":4.0,\"total_dollars\":10.0}}],\"active_thread_count\":1}";

    private static string ValidAccountsJson() =>
        "{\"api_version\":\"v3\",\"default_account_id\":\"account-7\",\"accounts\":[{\"id\":\"account-7\",\"is_current\":true,\"activation_at\":1789167600,\"deactivation_at\":null},{\"id\":\"account-13\",\"is_current\":false,\"activation_at\":null,\"deactivation_at\":null}]}";

    private static string ValidHistoryPageJson() =>
        $"{{\"api_version\":\"v3\",\"history_samples\":[{{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"models\":[{{\"model\":\"ASTRA\",\"total_tokens\":6,\"input_tokens\":4,\"cached_input_tokens\":1,\"cache_write_input_tokens\":0,\"output_tokens\":2,\"total_dollars\":0.25}}],\"models_complete\":true,\"model_source\":\"confirmed\"}}],\"history_gaps\":[],\"next_cursor\":null,\"resume_cursor\":\"resume-opaque\"}}";

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
