// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
// Version-route/fallback/304 assertions retired by the explicit single-contract request.
// Exact scalar/hash/receipt coverage lives in WireCodecTests/WireReceiverTests;
// these tests retain the UI facade's source facts and normal/error distinctions.
using System.Net;
using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using Xunit;
namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class LoopbackStatusClientTests
{
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task HistoryDirectoriesRetainNewestFirstNavigationOrder(bool selectable)
    {
        const long now = 1800000000;
        using var fixture = new WireFacadeFixture
        {
            Rows = _ => [
         new(["period","past-opaque"],"period",WireFacadeFixture.Element(new{id="past-opaque",start_at=now-1200,end_at=now-600,reset_at=now-600,current=false,label="past",origin_at=(long?)null})),
   new(["period","current-opaque"],"period",WireFacadeFixture.Element(new{id="current-opaque",start_at=now-60,end_at=now,reset_at=now+3600,current=true,label="current",origin_at=(long?)null}))
        ]
        };
        using var client = new LoopbackStatusClient(fixture); client.SetViewVisible("graph", true);
        var result = selectable ? await client.FetchSelectableHistoryPeriodsAsync("account-1") : await client.FetchWindowPeriodsAsync("account-1", now - 1800, now + 3601);
        Assert.True(result.IsSuccess);
        Assert.Equal(new[] { "current-opaque", "past-opaque" }, result.Snapshot!.Periods.Select(period => period.Id));
        Assert.Equal(new[] { now + 3600, now - 600 }, result.Snapshot.Periods.Select(period => period.ResetAt));
        Assert.Equal(new[] { now - 60, now - 1200 }, result.Snapshot.Periods.Select(period => period.StartAt));
        Assert.Single(fixture.Gets);
    }
    [Fact]
    public async Task MainUsesOneUiProjectionAndPreservesUnsignedTokensSavedPriceAndUnknownThreads()
    {
        using var fixture = new WireFacadeFixture(); using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(null, false); Assert.Null(result.Failure); var value = Assert.IsType<ApiCurrentSnapshot>(result.Current); Assert.Equal(ApiState.Ready, value.State); Assert.Equal(50.0, value.Quota!.RemainingPercent); var model = Assert.Single(value.Models); Assert.Equal(ulong.MaxValue, model.TotalTokens); Assert.Equal(ulong.MaxValue, model.InputTokens); Assert.Equal("saved-price", model.PriceVersion); Assert.Equal(BitConverter.DoubleToInt64Bits(-0.0), BitConverter.DoubleToInt64Bits(model.EstimatedTotalDollars!.Value)); Assert.Null(value.ThreadSummary); Assert.Equal(new[] { "/state?parts=main" }, fixture.Gets); var same = await client.FetchMainAsync(null, false); Assert.NotNull(same.Current); Assert.Single(fixture.Gets);
    }
    [Theory]
    [InlineData("partial", false)]
    [InlineData("complete", true)]
    [InlineData("unavailable", true)]
    public async Task PartialConfirmedEmptyAndUnknownModelsAreNormalStates(string state, bool empty)
    {
        using var fixture = new WireFacadeFixture { Rows = _ => [new(["main"], "main", WireFacadeFixture.MainValue(modelsStatus: state, empty: empty))] }; using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(null, false); Assert.Null(result.Failure); Assert.Equal(ApiState.Ready, result.Current!.State); Assert.Equal(empty, result.Current.Models.Count == 0);
    }
    [Fact]
    public async Task PreparingIsPendingAndNeverConnectionFailure()
    {
        using var fixture = new WireFacadeFixture { ControlResult = "pending" }; using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(null, false); Assert.Equal(DetailsFetchFailure.Pending, result.Failure); Assert.Single(fixture.Gets);
    }
    [Fact]
    public async Task AcquisitionErrorIsDomainDataAndKeepsKnownQuota()
    {
        using var fixture = new WireFacadeFixture { Rows = _ => [new(["main"], "main", WireFacadeFixture.MainValue(state: "error"))] }; using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(null, false); Assert.Null(result.Failure); Assert.Equal(ApiState.Error, result.Current!.State); Assert.Equal(50.0, result.Current.Quota!.RemainingPercent); Assert.Equal(ulong.MaxValue, Assert.Single(result.Current.Models).TotalTokens);
    }
    [Theory]
    [InlineData(400)]
    [InlineData(404)]
    [InlineData(500)]
    public async Task ReturnedHttpFailuresNeverTriggerAnotherRouteOrTransportFailure(int code)
    {
        using var fixture = new WireFacadeFixture { Status = (HttpStatusCode)code }; using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(null, false); Assert.Equal(DetailsFetchFailure.Response, result.Failure); Assert.Equal(new[] { "/state?parts=main" }, fixture.Gets);
    }
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task NetworkIoAndConnectFailuresAreTransport(bool io)
    {
        using var fixture = new WireFacadeFixture { Failure = io ? new IOException("fixture disconnect") : new HttpRequestException("fixture connect") }; using var client = new LoopbackStatusClient(fixture); Assert.Equal(DetailsFetchFailure.Transport, (await client.FetchMainAsync(null, false)).Failure);
    }
    [Fact]
    public async Task HistoryRetainsOpaquePeriodKnownTotalsUnknownComponentsAndSavedDollarBits()
    {
        const string period = "period_opaque.A"; using var fixture = new WireFacadeFixture { Rows = _ => [new(["sample", period, 1800604800L, 1800000000L], "sample", WireFacadeFixture.Element(new { remaining_percent = (double?)null, task_active_since_previous = (bool?)null, models_complete = true, model_source = "legacy-unknown", models = new[] { new { model = "alpha", total_tokens = 40ul, cached_input_tokens = 3ul, output_tokens = 8ul, total_dollars = -0.0 } } }))] }; using var client = new LoopbackStatusClient(fixture); client.SetViewVisible("graph", true); var result = await client.FetchHistoryPageAsync("account-1", period, null, CancellationToken.None); Assert.True(result.IsSuccess); Assert.Equal(period, result.Page!.PeriodId); var sample = Assert.Single(result.Page.Samples); Assert.Null(sample.RemainingPercent); Assert.True(sample.ModelsComplete); var model = Assert.Single(sample.Models); Assert.Equal(40ul, model.TotalTokens); Assert.Null(model.InputTokens); Assert.Equal(3ul, model.CachedInputTokens); Assert.Null(model.CacheWriteInputTokens); Assert.Equal(BitConverter.DoubleToInt64Bits(-0.0), BitConverter.DoubleToInt64Bits(model.Dollars!.Value)); Assert.Equal(new[] { "/history?account=account-1&period=" + period }, fixture.Gets);
    }
    [Fact]
    public async Task RuntimeUsesOnlySettingsProjectionAndPreservesUnavailableRecorder()
    {
        using var fixture = new WireFacadeFixture { Rows = _ => [new(["runtime"], "runtime", WireFacadeFixture.Element(new { rest_product_version = "1.0.173", recorder_product_version = (string?)null, recorder_status = "unavailable" }))] }; using var client = new LoopbackStatusClient(fixture); client.SetViewVisible("settings", true); var result = await client.FetchRuntimeVersionsAsync(); Assert.True(result.IsSuccess); Assert.Equal("1.0.173", result.Snapshot!.RestVersion); Assert.Null(result.Snapshot.RecorderVersion); Assert.Equal(new[] { "/state?parts=runtime" }, fixture.Gets);
    }
}
