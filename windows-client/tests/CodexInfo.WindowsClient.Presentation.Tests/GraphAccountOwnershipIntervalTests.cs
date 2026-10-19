// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphAccountOwnershipIntervalTests
{
    [Fact]
    public void EmptyOwnershipMarkersDoNotChangeOwnedCoverage()
    {
        var samples = new[]
        {
            new ApiHistorySample(100, 400, 100, null, null, null, null, null, null, ApiHistorySample.ConfirmedModelSource),
            new ApiHistorySample(299, 400, 90, null, null, null, null, null, null, ApiHistorySample.ConfirmedModelSource),
        };
        var scene = GraphScene.Create(samples, GraphMetric.Tokens, 100, 300, confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals:
            [
                new GraphAccountOwnershipInterval(100, 300),
                new GraphAccountOwnershipInterval(150, 150),
                new GraphAccountOwnershipInterval(253_402_300_799, 253_402_300_799),
            ]);
        Assert.Empty(scene.NonOwnedIntervals);

        var empty = GraphScene.Create(samples, GraphMetric.Tokens, 100, 300, confirmedGaps: null, hiddenModelNames: null,
            accountOwnershipIntervals: [new GraphAccountOwnershipInterval(150, 150)]);
        var gap = Assert.Single(empty.NonOwnedIntervals);
        Assert.Equal(100, gap.StartAt);
        Assert.Equal(300, gap.EndAt);
        Assert.Empty(GraphPlotProjection.BuildVisibleUnusedIntervals(empty));
    }

    [Fact]
    public void LoggedOutOwnershipGapDoesNotBecomeVisibleUnusedIntervalInTokensGraphDespiteEndpointDelta()
    {
        const long firstTimestamp = 1_789_967_100;
        const long secondTimestamp = 1_790_077_980;
        const long resetAt = 1_790_461_476;
        const long periodEndAt = 1_790_078_040;
        const long ownershipGapStartAt = 1_789_967_251;
        const long ownershipGapEndAt = 1_790_077_988;

        var samples = new[]
        {
            new ApiHistorySample(
                firstTimestamp,
                resetAt,
                75,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new ApiHistoryModelSample("LUNA", null, null, null, null)
                    {
                        TotalTokens = 69_952_744,
                    },
                    new ApiHistoryModelSample("SOL", null, null, null, null)
                    {
                        TotalTokens = 12_322_064,
                    },
                ],
            },
            new ApiHistorySample(
                secondTimestamp,
                resetAt,
                67,
                null,
                null,
                null,
                null,
                null,
                null,
                ApiHistorySample.ConfirmedModelSource)
            {
                ModelsComplete = true,
                ModelSamples =
                [
                    new ApiHistoryModelSample("LUNA", null, null, null, null)
                    {
                        TotalTokens = 70_091_500,
                    },
                    new ApiHistoryModelSample("SOL", null, null, null, null)
                    {
                        TotalTokens = 12_322_064,
                    },
                ],
            },
        };

        var scene = GraphScene.Create(
            samples,
            GraphMetric.Tokens,
            firstTimestamp,
            periodEndAt,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals:
            [
                new GraphAccountOwnershipInterval(1_789_951_507, ownershipGapStartAt),
                new GraphAccountOwnershipInterval(ownershipGapEndAt, 1_790_091_547),
            ]);

        Assert.Empty(GraphPlotProjection.BuildVisibleUnusedIntervals(scene));
        Assert.Empty(scene.IdleIntervals);

        var modelLines = GraphPlotProjection.BuildModelLines(scene, scene.Luna);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            modelLines.Idle,
            modelLines.Flat,
            modelLines.Rising);
        AssertOwnershipDashedSegments(
            modelLines.Dashed,
            firstTimestamp,
            secondTimestamp);

        var solModelLines = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            solModelLines.Idle,
            solModelLines.Flat,
            solModelLines.Rising);
        AssertOwnershipDashedSegments(
            solModelLines.Dashed,
            firstTimestamp,
            secondTimestamp);

        var remainingLines = GraphPlotProjection.BuildRemainingLines(scene);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            remainingLines.Idle,
            remainingLines.Solid);
        AssertOwnershipDashedSegments(
            remainingLines.Dashed,
            firstTimestamp,
            secondTimestamp);

        var noOwnershipScene = GraphScene.Create(
            samples,
            GraphMetric.Tokens,
            firstTimestamp,
            periodEndAt,
            confirmedGaps: null,
            hiddenModelNames: null,
            accountOwnershipIntervals: null);

        Assert.Empty(GraphPlotProjection.BuildVisibleUnusedIntervals(noOwnershipScene));
        Assert.Empty(noOwnershipScene.IdleIntervals);

        var noOwnershipModelLines = GraphPlotProjection.BuildModelLines(
            noOwnershipScene,
            noOwnershipScene.Luna);
        // Without ownership or gap metadata, spacing alone cannot turn
        // confirmed observations into missing data. Only the tail is inferred.
        Assert.Contains((double)firstTimestamp, noOwnershipModelLines.Rising.X);
        Assert.Contains((double)secondTimestamp, noOwnershipModelLines.Rising.X);
        Assert.NotEmpty(noOwnershipModelLines.Dashed.X);
        Assert.All(noOwnershipModelLines.Dashed.X.Where(double.IsFinite),
            timestamp => Assert.True(timestamp >= secondTimestamp));

        var noOwnershipRemainingLines = GraphPlotProjection.BuildRemainingLines(
            noOwnershipScene);
        Assert.Contains((double)firstTimestamp, noOwnershipRemainingLines.Solid.X);
        Assert.Contains((double)secondTimestamp, noOwnershipRemainingLines.Solid.X);
        Assert.NotEmpty(noOwnershipRemainingLines.Dashed.X);
        Assert.All(noOwnershipRemainingLines.Dashed.X.Where(double.IsFinite),
            timestamp => Assert.True(timestamp >= secondTimestamp));
    }

    [Fact]
    public void IdleRequiresOneConfirmedOwnershipInterval()
    {
        var samples = new[]
        {
            CreateSample(1_000),
            CreateSample(1_600),
        };
        var cases = new (IReadOnlyList<GraphAccountOwnershipInterval>? Ownership, GraphIdleInterval[] Expected)[]
        {
            (null, []),
            ([], []),
            ([new GraphAccountOwnershipInterval(null, null)], []),
            ([new GraphAccountOwnershipInterval(null, 1_700)], []),
            ([new GraphAccountOwnershipInterval(900, null)], [new GraphIdleInterval(1_000, 1_600, false)]),
            ([new GraphAccountOwnershipInterval(900, 1_700)], [new GraphIdleInterval(1_000, 1_600, false)]),
            ([new GraphAccountOwnershipInterval(900, 1_600)], []),
        };

        foreach (var (ownership, expectedIdle) in cases)
        {
            var scene = GraphScene.Create(
                samples,
                GraphMetric.Dollars,
                1_000,
                1_600,
                confirmedGaps: [],
                hiddenModelNames: null,
                accountOwnershipIntervals: ownership);

            Assert.Equal(expectedIdle, scene.IdleIntervals);
            Assert.Equal([94d, 94d], scene.Remaining);

            var model = GraphPlotProjection.BuildModelLines(scene, scene.Sol);
            var measuredModelX = model.Idle.X.Concat(model.Flat.X).Concat(model.Rising.X).ToArray();
            Assert.Contains(1_000d, measuredModelX);
            Assert.Contains(1_600d, measuredModelX);
            Assert.Empty(model.Dashed.X);

            var remaining = GraphPlotProjection.BuildRemainingLines(scene);
            var measuredRemainingX = remaining.Idle.X.Concat(remaining.Solid.X).ToArray();
            Assert.Contains(1_000d, measuredRemainingX);
            Assert.Contains(1_600d, measuredRemainingX);
            Assert.Empty(remaining.Dashed.X);
        }
    }

    private static ApiHistorySample CreateSample(long timestamp) =>
        new(
            timestamp,
            2_000,
            94,
            null,
            null,
            null,
            null,
            null,
            null,
            ApiHistorySample.ConfirmedModelSource)
        {
            ModelsComplete = true,
            ModelSamples =
            [
                new ApiHistoryModelSample("SOL", 20, 0, 0, 1)
                {
                    TotalTokens = 20,
                    CacheWriteInputTokens = 0,
                },
            ],
        };

    private static void AssertOwnershipDashedSegments(
        GraphLineProjection line,
        long firstTimestamp,
        long secondTimestamp)
    {
        var pairs = line.X
            .Zip(line.X.Skip(1))
            .Where(pair => double.IsFinite(pair.First) && double.IsFinite(pair.Second))
            .Select(pair => ((long)pair.First, (long)pair.Second))
            .ToArray();
        Assert.Contains((firstTimestamp, secondTimestamp), pairs);
    }

    private static void AssertNoOwnershipGapCrossings(
        long gapStartAt,
        long gapEndAt,
        params GraphLineProjection[] lines)
    {
        var crossingPairs = lines
            .SelectMany(line => line.X.Zip(line.X.Skip(1)))
            .Where(pair => pair.First < gapEndAt && pair.Second > gapStartAt)
            .ToArray();

        Assert.Empty(crossingPairs);
    }
}
