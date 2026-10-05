// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Controls;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Graphing;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphRefreshPresentationTests
{
    [Theory]
    [InlineData(2, GraphMetric.Dollars)]
    [InlineData(8_192, GraphMetric.Tokens)]
    public void PublishingNextSceneDoesNotChangeThePreviouslySubmittedDrawing(
        int sampleCount,
        GraphMetric metric)
    {
        var control = new GraphPlotControl
        {
            Scene = CreateScene(sampleCount, metric, remaining: 90, seed: 1),
        };
        // Avalonia can submit a drawing before the compositor executes it.
        // That accepted drawing must remain complete while the next scene is
        // prepared and published. The oracle is its actual previous raster,
        // not a second copy of the graph projection calculations.
        var submittedPlot = control.Plot;
        using var before = submittedPlot.GetImage(940, 480);
        var acceptedPixels = before.GetImageBytes();

        control.Scene = CreateScene(sampleCount, metric, remaining: 60, seed: 100);

        using var retained = submittedPlot.GetImage(940, 480);
        Assert.Equal(acceptedPixels, retained.GetImageBytes());
        using var replacement = control.Plot.GetImage(940, 480);
        Assert.False(acceptedPixels.SequenceEqual(replacement.GetImageBytes()),
            "The complete replacement must publish its new values.");
    }

    [Fact]
    public void ExplicitEmptySceneRemovesThePreviousAccountDrawing()
    {
        var control = new GraphPlotControl
        {
            Scene = CreateScene(2, GraphMetric.Tokens, remaining: 90, seed: 1),
        };
        Assert.NotEmpty(control.Plot.GetPlottables());

        // The view model sends an explicit empty scene when the account
        // boundary changes. Retention must never override that boundary.
        control.Scene = GraphScene.Empty(GraphMetric.Tokens);

        Assert.Empty(control.Plot.GetPlottables());
    }

    private static GraphScene CreateScene(
        int sampleCount,
        GraphMetric metric,
        double remaining,
        int seed)
    {
        const long start = 1_000;
        var end = start + (sampleCount - 1) * 60L;
        var samples = Enumerable.Range(0, sampleCount)
            .Select(index => new ApiHistorySample(
                start + index * 60L,
                end + 600,
                remaining - index / (double)sampleCount,
                seed + index,
                seed + index * 2,
                seed + index * 3,
                (ulong)(seed + index),
                (ulong)(seed + index * 2),
                (ulong)(seed + index * 3),
                ApiHistorySample.ConfirmedModelSource))
            .ToArray();
        return GraphScene.Create(samples, metric, start, end);
    }
}
