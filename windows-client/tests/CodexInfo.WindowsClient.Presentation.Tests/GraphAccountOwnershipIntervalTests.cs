// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphAccountOwnershipIntervalTests
{
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

        var modelLines = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Luna);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            modelLines.Idle.Line,
            modelLines.Flat.Line,
            modelLines.Rising.Line,
            modelLines.Dashed.Line);

        var solModelLines = GraphPlotProjection.BuildCanonicalModelLines(scene, scene.Sol);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            solModelLines.Idle.Line,
            solModelLines.Flat.Line,
            solModelLines.Rising.Line,
            solModelLines.Dashed.Line);

        var remainingLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            scene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            remainingLines.Idle.Line,
            remainingLines.Solid.Line,
            remainingLines.Dashed.Line);

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

        var noOwnershipModelLines = GraphPlotProjection.BuildCanonicalModelLines(
            noOwnershipScene,
            noOwnershipScene.Luna);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            noOwnershipModelLines.Idle.Line,
            noOwnershipModelLines.Flat.Line,
            noOwnershipModelLines.Rising.Line);
        Assert.NotEmpty(noOwnershipModelLines.Dashed.Line.X);

        var noOwnershipRemainingLines = GraphPlotProjection.BuildCanonicalRemainingLines(
            noOwnershipScene,
            GraphRemainingBaselineMode.PeriodStartAtFullQuota);
        AssertNoOwnershipGapCrossings(
            ownershipGapStartAt,
            ownershipGapEndAt,
            noOwnershipRemainingLines.Idle.Line,
            noOwnershipRemainingLines.Solid.Line);
        Assert.NotEmpty(noOwnershipRemainingLines.Dashed.Line.X);
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
