// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class ContractsTests
{
    [Fact]
    public void ProductVersionIsDerivedFromTheCoreAssemblyVersion()
    {
        var assemblyVersion = typeof(ProductInfo).Assembly.GetName().Version;

        Assert.NotNull(assemblyVersion);
        Assert.Equal(
            $"{assemblyVersion!.Major}.{assemblyVersion.Minor}.{assemblyVersion.Build}",
            ProductInfo.Version);
        Assert.Equal(
            $"v{assemblyVersion!.Major}.{assemblyVersion.Minor}.{assemblyVersion.Build}",
            ProductInfo.DisplayVersion);
    }

    [Fact]
    public void DetailsCompatibilityConstructorPreservesCollectionsAndDefaults()
    {
        var models = new[]
        {
            new ApiDetailsModelUsage("SOL", 1, 2, 3, 0.5, 0.25, 0.75),
        };
        var periods = new[]
        {
            new ApiHistoryPeriod("period", 10, 20, true, "label"),
        };
        var threads = new[]
        {
            new ApiThreadDetails("thread", "title", null, "SOL", "SOL", null, null, null, null, null, false, null, false),
        };
        var notices = new[]
        {
            new ApiLegalNotice("license", "text"),
        };

        var snapshot = new ApiDetailsSnapshot(models, periods, threads, notices);

        Assert.Equal(ApiState.Ready, snapshot.State);
        Assert.Null(snapshot.ObservedAt);
        Assert.True(snapshot.Authenticated);
        Assert.Null(snapshot.PlanLabel);
        Assert.Null(snapshot.Quota);
        Assert.Same(models, snapshot.Models);
        Assert.Equal((ulong)0, snapshot.ActiveThreadCount);
        Assert.Same(periods, snapshot.HistoryPeriods);
        Assert.Same(periods, snapshot.History);
        Assert.Empty(snapshot.HistorySamples);
        Assert.Same(threads, snapshot.Threads);
        Assert.Equal("未取得", snapshot.EstimatedCostLabel);
        Assert.Same(notices, snapshot.LegalNotices);
    }

    [Fact]
    public void DerivedUsageAndHistoryValuesFollowTheirContracts()
    {
        var usage = new ApiDetailsModelUsage("TERRA", 1, 2, 3, 1.25, 2.5, 4.75);
        Assert.Equal(8.5, usage.TotalDollars);

        var zeroLength = new ApiHistoryPeriod("zero", 100, 100, false, "zero");
        Assert.Equal(100, zeroLength.ResetAt);
        Assert.Equal(1, zeroLength.WindowSeconds);
        Assert.False(zeroLength.Monthly);

        var monthly = new ApiHistoryPeriod("monthly", 100, 100 + (long)TimeSpan.FromDays(9).TotalSeconds, false, "monthly");
        Assert.Equal((long)TimeSpan.FromDays(9).TotalSeconds, monthly.WindowSeconds);
        Assert.True(monthly.Monthly);

        var samples = new[]
        {
            new ApiHistorySample(1, 2, 50, 1.5, 2.5, 3.5, 4, 5, 6),
        };
        var period = new ApiHistoryPeriod("with-samples", 1, 2, true, "label")
        {
            Samples = samples,
        };
        Assert.Same(samples, period.Samples);
    }

    [Fact]
    public void HistorySampleModelsExposeEachProviderValues()
    {
        var sample = new ApiHistorySample(1, 2, null, 1.5, 2.5, 3.5, 4, 5, 6);

        var models = sample.Models;

        Assert.Collection(
            models,
            sol =>
            {
                Assert.Equal("SOL", sol.Name);
                Assert.Equal((ulong)4, sol.InputTokens);
                Assert.Equal((ulong)0, sol.CachedInputTokens);
                Assert.Equal((ulong)0, sol.OutputTokens);
                Assert.Equal(1.5, sol.Dollars);
            },
            terra =>
            {
                Assert.Equal("TERRA", terra.Name);
                Assert.Equal((ulong)5, terra.InputTokens);
                Assert.Equal(2.5, terra.Dollars);
            },
            luna =>
            {
                Assert.Equal("LUNA", luna.Name);
                Assert.Equal((ulong)6, luna.InputTokens);
                Assert.Equal(3.5, luna.Dollars);
            });
    }

    [Theory]
    [InlineData(50UL, 100UL, 50.0)]
    [InlineData(150UL, 100UL, 100.0)]
    public void ThreadContextPercentHandlesValidAndUnavailableContext(
        ulong used,
        ulong limit,
        double expected)
    {
        var thread = new ApiThreadDetails(
            "thread",
            "title",
            null,
            "SOL",
            "SOL",
            1,
            used,
            limit,
            1,
            1,
            false,
            0,
            false);

        Assert.Equal(expected, thread.ContextPercent);
    }

    [Fact]
    public void ThreadContextPercentIsUnavailableWithoutPositiveLimit()
    {
        var thread = new ApiThreadDetails(
            "thread",
            "title",
            null,
            "SOL",
            "SOL",
            1,
            50,
            0,
            1,
            1,
            false,
            0,
            false);

        Assert.Null(thread.ContextPercent);
    }

    [Fact]
    public void DetailsResultFactoriesExposeSuccessAndFailureStates()
    {
        var snapshot = new ApiDetailsSnapshot(
            ApiState.Initializing,
            null,
            true,
            null,
            null,
            Array.Empty<ApiDetailsModelUsage>(),
            0,
            Array.Empty<ApiHistoryPeriod>(),
            Array.Empty<ApiHistorySample>(),
            Array.Empty<ApiThreadDetails>(),
            "未取得");

        var success = DetailsFetchResult.Success(snapshot);
        var failure = DetailsFetchResult.FromFailure(DetailsFetchFailure.Transport);

        Assert.True(success.IsSuccess);
        Assert.Same(snapshot, success.Snapshot);
        Assert.Null(success.Failure);
        Assert.False(failure.IsSuccess);
        Assert.Null(failure.Snapshot);
        Assert.Equal(DetailsFetchFailure.Transport, failure.Failure);
        Assert.Throws<ArgumentNullException>(() => DetailsFetchResult.Success(null!));
    }

    [Fact]
    public void ContractRecordsRetainValuesThroughWithUpdates()
    {
        var quota = new ApiQuota(50, 10, 20, false) with
        {
            RemainingPercent = 75,
            ResetAt = 11,
            WindowSeconds = 21,
            Monthly = true,
        };
        Assert.Equal(75, quota.RemainingPercent);
        Assert.Equal(11, quota.ResetAt);
        Assert.Equal(21, quota.WindowSeconds);
        Assert.True(quota.Monthly);

        var historySample = new ApiHistorySample(1, 2, 3, 4, 5, 6, 7, 8, 9) with
        {
            Timestamp = 10,
            ResetAt = 11,
            RemainingPercent = null,
            SolDollars = 12,
            TerraDollars = 13,
            LunaDollars = 14,
            SolTokens = 15,
            TerraTokens = 16,
            LunaTokens = 17,
        };
        Assert.Equal(10, historySample.Timestamp);
        Assert.Equal(11, historySample.ResetAt);
        Assert.Null(historySample.RemainingPercent);
        Assert.Equal(12, historySample.SolDollars);
        Assert.Equal(13, historySample.TerraDollars);
        Assert.Equal(14, historySample.LunaDollars);
        Assert.Equal((ulong)15, historySample.SolTokens);
        Assert.Equal((ulong)16, historySample.TerraTokens);
        Assert.Equal((ulong)17, historySample.LunaTokens);

        var historyModel = new ApiHistoryModelSample("SOL", 1, 2, 3, 4) with
        {
            Name = "LUNA",
            InputTokens = 5,
            CachedInputTokens = 6,
            OutputTokens = 7,
            Dollars = 8,
        };
        Assert.Equal("LUNA", historyModel.Name);
        Assert.Equal((ulong)5, historyModel.InputTokens);
        Assert.Equal((ulong)6, historyModel.CachedInputTokens);
        Assert.Equal((ulong)7, historyModel.OutputTokens);
        Assert.Equal(8, historyModel.Dollars);

        var period = new ApiHistoryPeriod("old", 1, 2, false, "old") with
        {
            Id = "new",
            StartAt = 3,
            EndAt = 4,
            Current = true,
            Label = "new label",
            Samples = new[] { historySample },
        };
        Assert.Equal("new", period.Id);
        Assert.Equal(3, period.StartAt);
        Assert.Equal(4, period.EndAt);
        Assert.True(period.Current);
        Assert.Equal("new label", period.Label);
        Assert.Single(period.Samples);

        var thread = new ApiThreadDetails(
            "old",
            "old title",
            null,
            "SOL",
            "SOL",
            null,
            null,
            null,
            null,
            null,
            false,
            null,
            false) with
        {
            Id = "new",
            Title = "new title",
            ParentId = "parent",
            Model = "TERRA",
            ModelLabel = "Terra",
            CumulativeTokens = 1,
            ContextTokens = 2,
            ContextLimit = 3,
            CreatedAt = 4,
            LastUserMessageAt = 5,
            IsSubAgent = true,
            Depth = 6,
            IsOrphan = true,
        };
        Assert.Equal("new", thread.Id);
        Assert.Equal("new title", thread.Title);
        Assert.Equal("parent", thread.ParentId);
        Assert.Equal("TERRA", thread.Model);
        Assert.Equal("Terra", thread.ModelLabel);
        Assert.Equal((ulong)1, thread.CumulativeTokens);
        Assert.Equal((ulong)2, thread.ContextTokens);
        Assert.Equal((ulong)3, thread.ContextLimit);
        Assert.Equal(4, thread.CreatedAt);
        Assert.Equal(5, thread.LastUserMessageAt);
        Assert.True(thread.IsSubAgent);
        Assert.Equal(6, thread.Depth);
        Assert.True(thread.IsOrphan);

        var notice = new ApiLegalNotice("old", "old text") with
        {
            Name = "new",
            Text = "new text",
        };
        Assert.Equal("new", notice.Name);
        Assert.Equal("new text", notice.Text);

        var detailsModel = new ApiDetailsModelUsage("SOL", 1, 2, 3, 1, 2, 3) with
        {
            Name = "TERRA",
            InputTokens = 4,
            CachedInputTokens = 5,
            OutputTokens = 6,
            InputDollars = 4,
            CachedInputDollars = 5,
            OutputDollars = 6,
        };
        Assert.Equal("TERRA", detailsModel.Name);
        Assert.Equal((ulong)4, detailsModel.InputTokens);
        Assert.Equal((ulong)5, detailsModel.CachedInputTokens);
        Assert.Equal((ulong)6, detailsModel.OutputTokens);
        Assert.Equal(15, detailsModel.TotalDollars);

        var details = new ApiDetailsSnapshot(
            ApiState.Ready,
            1,
            true,
            "Pro",
            quota,
            new[] { detailsModel },
            1,
            new[] { period },
            new[] { historySample },
            new[] { thread },
            "old") with
        {
            State = ApiState.Error,
            ObservedAt = 2,
            Authenticated = false,
            PlanLabel = "Team",
            Quota = null,
            Models = Array.Empty<ApiDetailsModelUsage>(),
            ActiveThreadCount = 2,
            HistoryPeriods = Array.Empty<ApiHistoryPeriod>(),
            HistorySamples = Array.Empty<ApiHistorySample>(),
            Threads = Array.Empty<ApiThreadDetails>(),
            EstimatedCostLabel = "new",
            LegalNotices = new[] { notice },
        };
        Assert.Equal(ApiState.Error, details.State);
        Assert.Equal(2, details.ObservedAt);
        Assert.False(details.Authenticated);
        Assert.Equal("Team", details.PlanLabel);
        Assert.Null(details.Quota);
        Assert.Empty(details.Models);
        Assert.Equal((ulong)2, details.ActiveThreadCount);
        Assert.Empty(details.HistoryPeriods);
        Assert.Empty(details.HistorySamples);
        Assert.Empty(details.Threads);
        Assert.Equal("new", details.EstimatedCostLabel);
        Assert.Single(details.LegalNotices);

        var detailsResult = new DetailsFetchResult(details, null) with
        {
            Snapshot = null,
            Failure = DetailsFetchFailure.Response,
        };
        Assert.Null(detailsResult.Snapshot);
        Assert.Equal(DetailsFetchFailure.Response, detailsResult.Failure);

    }
}
