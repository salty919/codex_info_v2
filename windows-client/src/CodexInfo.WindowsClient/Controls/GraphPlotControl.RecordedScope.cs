// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Graphing;

namespace CodexInfo.WindowsClient.Controls;

public sealed partial class GraphPlotControl
{
    private void PopulateRecordedPlot(PlotPresentation presentation, GraphAxisProjection axes)
    {
        var scene = presentation.Scene;
        AddPlotGrid(presentation, scene, axes);
        ModelSeriesVisual? Models(IReadOnlyList<double> values, ScottPlot.Color color)
        {
            var indices = Enumerable.Range(0, scene.Timestamps.Count).Where(i => double.IsFinite(values[i])).ToArray();
            if (indices.Length == 0) return null;
            var scatter = presentation.Plot.Add.Scatter(indices.Select(i => scene.Timestamps[i]).ToArray(),
                indices.Select(i => values[i]).ToArray(), color);
            scatter.Axes.YAxis = presentation.Plot.Axes.Left;
            scatter.LineWidth = 0;
            scatter.MarkerSize = 4;
            if (scene.IsSavedObservation) return new(null, null, scatter, null);
            var steps = GraphPlotProjection.RecordedStepLine(scene, values);
            var line = presentation.Plot.Add.Scatter(steps.X.ToArray(), steps.Y.ToArray(), color);
            line.Axes.YAxis = presentation.Plot.Axes.Left;
            line.LineWidth = MeasuredModelLineWidth;
            line.MarkerSize = 0;
            return new(null, scatter, line, null);
        }
        presentation.SolSeries = Models(scene.Sol, SolColor);
        presentation.TerraSeries = Models(scene.Terra, TerraColor);
        presentation.LunaSeries = Models(scene.Luna, LunaColor);
        presentation.AstraSeries = Models(scene.Astra, AstraColor);
        // Unknown saved names remain visible as their own point series.
        foreach (var member in scene.ModelSeries.Where(member => ViewModels.ModelUsageViewModel.DisplayFamilyName(member.Key) is null))
            Models(member.Value, MutedColor);
        var quota = Enumerable.Range(0, scene.Timestamps.Count).Where(i => scene.RemainingObserved[i]).ToArray();
        if (quota.Length > 0)
        {
            var scatter = presentation.Plot.Add.Scatter(quota.Select(i => scene.Timestamps[i]).ToArray(),
                quota.Select(i => scene.Remaining[i]).ToArray(), RemainingColor);
            scatter.Axes.YAxis = presentation.Plot.Axes.Right;
            scatter.LineWidth = 0;
            scatter.MarkerSize = 4;
            presentation.RemainingMarkers = scatter;
        }
        ApplyAxes(presentation, scene, axes);
        if (!scene.IsSavedObservation) AddPeriodCostPanel(presentation, scene);
        ApplyVisibility(presentation);
    }
}
