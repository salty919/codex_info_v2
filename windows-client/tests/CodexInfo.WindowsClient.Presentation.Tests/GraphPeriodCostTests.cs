// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphPeriodCostTests
{
    [Theory]
    [InlineData(GraphMetric.Tokens)]
    [InlineData(GraphMetric.Dollars)]
    public void ActualPeriodUsesAllRawModelsOnceRegardlessOfMetricOrVisibility(GraphMetric metric)
    {
        var sample = Sample(1_000, Model("SOL", 0.5), Model("gpt-6-sol", 1.5), Model("future-model", 2));
        var scene = GraphScene.Create([sample], metric, 900, 1_500,
            confirmedGaps: null, hiddenModelNames: new HashSet<string> { "SOL", "future-model" });

        Assert.Equal(new GraphPeriodCostSummary(4, true), scene.PeriodCost);
        Assert.Empty(scene.ModelSeries);
        Assert.Equal(900, scene.PeriodStartAt);
        Assert.Equal(1_500, scene.PeriodEndAt);
    }

    [Fact]
    public void LatestPartialKeepsRecordedValuesInsteadOfTheOlderCompleteTotal()
    {
        var samples = new[]
        {
            Sample(1_000, Model("SOL", 1), Model("LUNA", 2)),
            Sample(1_060, Model("SOL", 4), Model("LUNA", null), Model("new-model", 0.5)) with { ModelsComplete = false },
        };

        Assert.Equal(new GraphPeriodCostSummary(4.5, false), Scene(samples).PeriodCost);
    }

    [Fact]
    public void DisappearingPreviouslyPublishedModelCannotProduceACompleteTotal()
    {
        var samples = new[]
        {
            Sample(1_000, Model("SOL", 1), Model("LUNA", 2)),
            Sample(1_060, Model("SOL", 4)),
        };

        Assert.Equal(new GraphPeriodCostSummary(4, false), Scene(samples).PeriodCost);
    }

    [Theory]
    [InlineData(ApiHistorySample.UnavailableModelSource)]
    [InlineData(ApiHistorySample.ReconstructedFromSessionModelSource)]
    [InlineData("unknown")]
    public void LatestUnavailableDoesNotFallBackToPriorCompleteOrInventZero(string source)
    {
        var samples = new[]
        {
            Sample(1_000, Model("SOL", 10)),
            Sample(1_060, Model("SOL", 20)) with { ModelSource = source, ModelsComplete = false },
        };

        Assert.Equal(default, Scene(samples).PeriodCost);
    }

    [Fact]
    public void ConfirmedEmptyIsZeroButAbsentOrLaterEmptyIsNot()
    {
        var empty = Sample(1_000);
        Assert.Equal(new GraphPeriodCostSummary(0, true), Scene([empty]).PeriodCost);
        Assert.Equal(default, Scene([empty with { ModelSamples = null }]).PeriodCost);
        Assert.Equal(default, Scene([empty with { ModelsComplete = false }]).PeriodCost);
        Assert.Equal(default, Scene([Sample(940, Model("SOL", 2)), empty]).PeriodCost);
    }

    [Fact]
    public void LegacyAstraUsesExistingComponentPriceOnlyAsUncertainRecordedAmount()
    {
        var astra = new ApiHistoryModelSample("ASTRA", 1_000_000, 200_000, 100_000, null)
        {
            TotalTokens = 1_100_000,
            CacheWriteInputTokens = 100_000,
        };
        var sample = Sample(1_000, astra) with
        {
            ModelSource = ApiHistorySample.LegacyUnknownModelSource,
            ModelsComplete = false,
        };

        // 700k ordinary * $10 + 200k cached * $1 + 100k write * $12.5 + 100k output * $50.
        Assert.Equal(new GraphPeriodCostSummary(13.45, false), Scene([sample]).PeriodCost);
    }

    [Theory]
    [InlineData(258.04085999999995, 260.04085999999995)]
    [InlineData(258.04085, 2)]
    public void LegacyCalculatedToPersistedPriceDistinguishesRoundingFromARealDecrease(
        double persistedPrice, double expectedTotal)
    {
        var components = new ApiHistoryModelSample("ASTRA", 200_878_864, 197_512_320, 537_262, null)
        {
            TotalTokens = 201_416_126,
            CacheWriteInputTokens = 0,
        };
        var samples = new[]
        {
            Sample(1_000, components, Model("SOL", 1)) with
            {
                ModelSource = ApiHistorySample.LegacyUnknownModelSource,
                ModelsComplete = false,
            },
            Sample(1_060, new ApiHistoryModelSample("ASTRA", 200_878_864, 197_512_320, 537_262, persistedPrice)
            { TotalTokens = 201_416_126, CacheWriteInputTokens = 0 }, Model("SOL", 2)) with
            {
                ModelSource = ApiHistorySample.LegacyUnknownModelSource,
                ModelsComplete = false,
            },
        };

        Assert.Equal(new GraphPeriodCostSummary(expectedTotal, false), Scene(samples).PeriodCost);
    }

    [Fact]
    public void SyntheticTailDoesNotReplaceTheLatestRecordedAmount()
    {
        var samples = new[]
        {
            Sample(1_000, Model("SOL", 2)),
            Sample(1_060, Model("SOL", 999)) with { IsSyntheticTail = true },
        };

        Assert.Equal(new GraphPeriodCostSummary(2, true), Scene(samples).PeriodCost);
    }

    [Fact]
    public void DollarRegressionIsExcludedWithoutHidingOtherValidAmounts()
    {
        var samples = new[]
        {
            Sample(1_000, Model("SOL", 10), Model("LUNA", 2)),
            Sample(1_060, Model("SOL", 1), Model("LUNA", 3)),
        };

        Assert.Equal(new GraphPeriodCostSummary(3, false), Scene(samples).PeriodCost);
    }

    [Theory]
    [InlineData(-1d)]
    [InlineData(double.NaN)]
    [InlineData(double.PositiveInfinity)]
    public void InvalidPriceCannotBecomeAConfirmedZero(double invalid)
    {
        Assert.Equal(new GraphPeriodCostSummary(2, false),
            Scene([Sample(1_000, Model("SOL", invalid), Model("LUNA", 2))]).PeriodCost);
    }

    private static GraphScene Scene(ApiHistorySample[] samples) =>
        GraphScene.Create(samples, GraphMetric.Tokens, samples[0].Timestamp, samples[^1].Timestamp + 60);

    private static ApiHistorySample Sample(long timestamp, params ApiHistoryModelSample[] models) =>
        new(timestamp, 9_000, 80, null, null, null, null, null, null)
        {
            ModelsComplete = true,
            ModelSamples = models,
        };

    private static ApiHistoryModelSample Model(string name, double? dollars) =>
        new(name, null, null, null, dollars) { TotalTokens = 100 };
}
