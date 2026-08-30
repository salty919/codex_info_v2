// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using Avalonia;
using Avalonia.Automation.Peers;
using Avalonia.Controls;
using Avalonia.Threading;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using ScottPlot.Avalonia;
using ScottPlot.TickGenerators;

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Thin Avalonia/ScottPlot adapter. All graph calculations are owned by the
/// framework-independent Graphing layer; this control only applies theme,
/// axes, visibility, and projected labels.
/// </summary>
public sealed class GraphPlotControl : AvaPlot
{
    private static readonly ScottPlot.Color RemainingColor = new("#56B2F5");
    private static readonly ScottPlot.Color SolColor = new("#A88CF5");
    private static readonly ScottPlot.Color TerraColor = new("#5DC98A");
    private static readonly ScottPlot.Color LunaColor = new("#E6A23C");
    internal const string IdleBandColorHex = "#3F5D7C";
    internal const double IdleBandOpacity = 0.22;
    private static readonly ScottPlot.Color IdleBandColor = new(IdleBandColorHex);
    private static readonly ScottPlot.Color MutedColor = new("#A8B7CA");
    private static readonly ScottPlot.Color GridColor = new("#263548");
    private static readonly ScottPlot.Color PlotColor = new("#101925");

    private ScottPlot.Plottables.Scatter? remainingSeries;
    private ModelSeriesVisual? solSeries;
    private ModelSeriesVisual? terraSeries;
    private ModelSeriesVisual? lunaSeries;
    private ScottPlot.Plottables.Scatter? remainingConnector;
    private ScottPlot.Plottables.Scatter? solConnector;
    private ScottPlot.Plottables.Scatter? terraConnector;
    private ScottPlot.Plottables.Scatter? lunaConnector;
    private ScottPlot.Plottables.Text? remainingLabel;
    private ScottPlot.Plottables.Text? solLabel;
    private ScottPlot.Plottables.Text? terraLabel;
    private ScottPlot.Plottables.Text? lunaLabel;
    private double[] remainingConnectorX = [];
    private double[] solConnectorX = [];
    private double[] terraConnectorX = [];
    private double[] lunaConnectorX = [];
    private double? referenceControlWidth;
    private readonly Dictionary<GraphMetric, double> referenceDataAreaWidths = [];
    private int sceneRevision;

    public GraphPlotControl()
    {
        UserInputProcessor.Disable();
        HandleMouseWheelEvent = false;
        ClipToBounds = true;
        SizeChanged += OnControlSizeChanged;
        ApplyScene();
    }

    public static readonly StyledProperty<GraphScene> SceneProperty =
        AvaloniaProperty.Register<GraphPlotControl, GraphScene>(nameof(Scene), GraphScene.Empty());
    public static readonly StyledProperty<bool> ShowRemainingProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowRemaining), true);
    public static readonly StyledProperty<bool> ShowModelsProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowModels), true);
    public static readonly StyledProperty<bool> ShowSolProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowSol), true);
    public static readonly StyledProperty<bool> ShowTerraProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowTerra), true);
    public static readonly StyledProperty<bool> ShowLunaProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowLuna), true);

    public GraphScene Scene { get => GetValue(SceneProperty); set => SetValue(SceneProperty, value); }
    public bool ShowRemaining { get => GetValue(ShowRemainingProperty); set => SetValue(ShowRemainingProperty, value); }
    public bool ShowModels { get => GetValue(ShowModelsProperty); set => SetValue(ShowModelsProperty, value); }
    public bool ShowSol { get => GetValue(ShowSolProperty); set => SetValue(ShowSolProperty, value); }
    public bool ShowTerra { get => GetValue(ShowTerraProperty); set => SetValue(ShowTerraProperty, value); }
    public bool ShowLuna { get => GetValue(ShowLunaProperty); set => SetValue(ShowLunaProperty, value); }

    protected override AutomationPeer OnCreateAutomationPeer() =>
        new GraphPlotAutomationPeer(this);

    protected override void OnPropertyChanged(AvaloniaPropertyChangedEventArgs change)
    {
        base.OnPropertyChanged(change);
        if (change.Property == SceneProperty)
        {
            ApplyScene();
        }
        else if (change.Property == ShowRemainingProperty ||
                 change.Property == ShowModelsProperty ||
                 change.Property == ShowSolProperty ||
                 change.Property == ShowTerraProperty ||
                 change.Property == ShowLunaProperty)
        {
            ApplyVisibility();
        }
    }

    private void ApplyScene()
    {
        var revision = ++sceneRevision;
        Plot.Clear();
        remainingSeries = null;
        solSeries = null;
        terraSeries = null;
        lunaSeries = null;
        remainingConnector = null;
        solConnector = null;
        terraConnector = null;
        lunaConnector = null;
        remainingLabel = null;
        solLabel = null;
        terraLabel = null;
        lunaLabel = null;
        remainingConnectorX = [];
        solConnectorX = [];
        terraConnectorX = [];
        lunaConnectorX = [];
        ApplyTheme();

        var scene = Scene;
        if (!scene.HasPoints)
        {
            Refresh();
            return;
        }

        var axes = BuildAxesForCurrentWidth(scene);
        foreach (var interval in GraphPlotProjection.BuildVisibleIdleIntervals(scene))
        {
            var band = Plot.Add.Rectangle(
                interval.StartAt,
                interval.EndAt,
                axes.ModelDisplayMinimum,
                axes.ModelDisplayMaximum);
            band.FillColor = IdleBandColor.WithOpacity(IdleBandOpacity);
            band.LineWidth = 0;
        }
        AddPlotGrid(scene, axes);

        lunaSeries = AddModelSeries(scene, scene.Luna, LunaColor);
        terraSeries = AddModelSeries(scene, scene.Terra, TerraColor);
        solSeries = AddModelSeries(scene, scene.Sol, SolColor);
        remainingSeries = AddLine(
            GraphPlotProjection.BuildRemainingLine(scene),
            RemainingColor,
            Plot.Axes.Right,
            2f);
        AddEndpointLabels(scene, axes);
        ApplyAxes(scene, axes);
        ApplyVisibility();
        ScheduleReferenceCapture(
            scene.Metric,
            revision,
            (long)Plot.RenderManager.RenderCount,
            attemptsRemaining: 20);
    }

    private GraphAxisProjection BuildAxesForCurrentWidth(GraphScene scene)
    {
        if (referenceControlWidth is { } controlWidth &&
            referenceDataAreaWidths.TryGetValue(scene.Metric, out var referenceDataAreaWidth))
        {
            var currentDataAreaWidth = referenceDataAreaWidth + Bounds.Width - controlWidth;
            if (currentDataAreaWidth > 0)
            {
                return GraphPlotProjection.BuildAxes(
                    scene,
                    LocalizationService.DisplayTimeZone,
                    CultureInfo.CurrentCulture,
                    currentDataAreaWidth,
                    referenceDataAreaWidth);
            }
        }

        return GraphPlotProjection.BuildAxes(
            scene,
            LocalizationService.DisplayTimeZone,
            CultureInfo.CurrentCulture);
    }

    private ModelSeriesVisual AddModelSeries(
        GraphScene scene,
        IReadOnlyList<double> values,
        ScottPlot.Color color)
    {
        var lines = GraphPlotProjection.BuildModelLines(scene, values);
        return new ModelSeriesVisual(
            AddLine(lines.Flat, color.WithOpacity(0.50), Plot.Axes.Left, 1f),
            AddLine(lines.Rising, color.WithOpacity(0.95), Plot.Axes.Left, 3f));
    }

    private ScottPlot.Plottables.Scatter? AddLine(
        GraphLineProjection line,
        ScottPlot.Color color,
        ScottPlot.IYAxis axis,
        float lineWidth)
    {
        if (line.X.Count < 2)
        {
            return null;
        }
        var series = Plot.Add.Scatter(line.X.ToArray(), line.Y.ToArray(), color);
        series.Axes.YAxis = axis;
        series.LineWidth = lineWidth;
        series.MarkerSize = 0;
        return series;
    }

    private void ApplyTheme()
    {
        Plot.FigureBackground.Color = PlotColor;
        Plot.DataBackground.Color = PlotColor;
        Plot.Axes.ContinuouslyAutoscale = false;
        Plot.Axes.Color(MutedColor);
        Plot.Axes.FrameColor(GridColor);
        Plot.Grid.MajorLineColor = GridColor;
        Plot.Grid.MinorLineColor = GridColor.WithOpacity(0.35);
        // ScottPlot's built-in horizontal grid spans the endpoint-label
        // gutter. X keeps the gutter clear, so bounded grid segments are
        // painted explicitly by AddPlotGrid().
        Plot.Grid.MajorLineWidth = 0;
        Plot.Grid.MinorLineWidth = 0;
        Plot.Font.Set("Noto Sans JP Medium");
    }

    private void AddPlotGrid(GraphScene scene, GraphAxisProjection axes)
    {
        foreach (var y in axes.ModelValues)
        {
            AddLine(
                new GraphLineProjection(
                    new double[] { scene.PeriodStartAt, scene.PeriodEndAt },
                    new double[] { y, y }),
                GridColor,
                Plot.Axes.Left,
                1f);
        }
        foreach (var x in axes.BottomValues)
        {
            AddLine(
                new GraphLineProjection(
                    new double[] { x, x },
                    new double[] { axes.ModelDisplayMinimum, axes.ModelDisplayMaximum }),
                GridColor,
                Plot.Axes.Left,
                1f);
        }
    }

    private void ApplyAxes(GraphScene scene, GraphAxisProjection axes)
    {
        ApplyLimits(scene, axes);
        Plot.Axes.Bottom.TickGenerator = new NumericManual(
            axes.BottomValues.ToArray(),
            axes.BottomLabels.ToArray());
        Plot.Axes.Left.TickGenerator = new NumericManual(
            axes.ModelValues.ToArray(),
            axes.ModelLabels.ToArray());
        Plot.Axes.Right.TickGenerator = new NumericManual(
            axes.RemainingValues.ToArray(),
            axes.RemainingLabels.ToArray());
        // The native graph owns remaining-percent semantics with its coloured
        // endpoint label. A second set of frame ticks steals the dedicated
        // label gutter and is not part of the X graph.
        Plot.Axes.Right.IsVisible = false;
    }

    private void ApplyLimits(GraphScene scene, GraphAxisProjection axes)
    {
        Plot.Axes.SetLimits(
            scene.PeriodStartAt,
            axes.DisplayEndAt,
            axes.ModelDisplayMinimum,
            axes.ModelDisplayMaximum,
            Plot.Axes.Bottom,
            Plot.Axes.Left);
        Plot.Axes.SetLimitsY(
            axes.RemainingDisplayMinimum,
            axes.RemainingDisplayMaximum,
            Plot.Axes.Right);
    }

    private void OnControlSizeChanged(object? sender, SizeChangedEventArgs change)
    {
        var currentControlWidth = change.NewSize.Width;
        if (referenceControlWidth is null &&
            double.IsFinite(currentControlWidth) && currentControlWidth > 0)
        {
            // The Graph window opens at its specified 940 logical-pixel
            // reference width. Remember the first arranged control width so
            // every metric can derive the same reference after its first
            // completed render, even if it is first selected after a resize.
            referenceControlWidth = currentControlWidth;
        }
        if (!double.IsFinite(currentControlWidth) || currentControlWidth <= 0)
        {
            return;
        }

        var scene = Scene;
        if (!scene.HasPoints || referenceControlWidth is null)
        {
            return;
        }
        if (!referenceDataAreaWidths.ContainsKey(scene.Metric))
        {
            var revision = ++sceneRevision;
            ScheduleReferenceCapture(
                scene.Metric,
                revision,
                (long)Plot.RenderManager.RenderCount,
                attemptsRemaining: 20);
            return;
        }
        ApplyResponsiveLayout(scene);
    }

    private void ScheduleReferenceCapture(
        GraphMetric metric,
        int revision,
        long priorRenderCount,
        int attemptsRemaining)
    {
        DispatcherTimer.RunOnce(() =>
        {
            var currentScene = Scene;
            if (sceneRevision != revision ||
                !currentScene.HasPoints ||
                currentScene.Metric != metric ||
                referenceDataAreaWidths.ContainsKey(metric))
            {
                return;
            }

            if ((long)Plot.RenderManager.RenderCount <= priorRenderCount)
            {
                if (attemptsRemaining > 1)
                {
                    ScheduleReferenceCapture(
                        metric,
                        revision,
                        priorRenderCount,
                        attemptsRemaining - 1);
                }
                return;
            }

            var currentControlWidth = Bounds.Width;
            var currentDataAreaWidth = Plot.LastRender.DataRect.Width;
            if (referenceControlWidth is not { } controlWidth ||
                !double.IsFinite(currentControlWidth) || currentControlWidth <= 0 ||
                !double.IsFinite(currentDataAreaWidth) || currentDataAreaWidth <= 0)
            {
                return;
            }
            var referenceDataAreaWidth = currentDataAreaWidth + controlWidth - currentControlWidth;
            if (!double.IsFinite(referenceDataAreaWidth) || referenceDataAreaWidth <= 0)
            {
                return;
            }
            referenceDataAreaWidths[metric] = referenceDataAreaWidth;

            // A metric first selected after a resize initially uses the
            // legacy proportional projection. Correct it only after a render
            // has completed, without subscribing to or mutating ScottPlot
            // inside its render callbacks.
            if (Math.Abs(currentControlWidth - controlWidth) > 0.5)
            {
                ApplyResponsiveLayout(currentScene);
            }
        }, TimeSpan.FromMilliseconds(25), DispatcherPriority.Background);
    }

    private void ApplyResponsiveLayout(GraphScene scene)
    {
        if (referenceControlWidth is not { } controlWidth ||
            !referenceDataAreaWidths.TryGetValue(scene.Metric, out var referenceDataAreaWidth))
        {
            return;
        }
        var currentDataAreaWidth = referenceDataAreaWidth + Bounds.Width - controlWidth;
        if (!double.IsFinite(currentDataAreaWidth) || currentDataAreaWidth <= 0)
        {
            return;
        }
        var axes = GraphPlotProjection.BuildAxes(
            scene,
            LocalizationService.DisplayTimeZone,
            CultureInfo.CurrentCulture,
            currentDataAreaWidth,
            referenceDataAreaWidth);
        ApplyLimits(scene, axes);
        UpdateEndpointLayout(remainingConnectorX, remainingLabel, axes.EndpointLabelAt);
        UpdateEndpointLayout(solConnectorX, solLabel, axes.EndpointLabelAt);
        UpdateEndpointLayout(terraConnectorX, terraLabel, axes.EndpointLabelAt);
        UpdateEndpointLayout(lunaConnectorX, lunaLabel, axes.EndpointLabelAt);
        Refresh();
    }

    private static void UpdateEndpointLayout(
        double[] connectorX,
        ScottPlot.Plottables.Text? label,
        double endpointLabelAt)
    {
        if (connectorX.Length == 2)
        {
            connectorX[1] = endpointLabelAt;
        }
        if (label is not null)
        {
            label.Location = new ScottPlot.Coordinates(endpointLabelAt, label.Location.Y);
        }
    }

    private void AddEndpointLabels(GraphScene scene, GraphAxisProjection axes)
    {
        foreach (var endpoint in GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.CurrentCulture))
        {
            var axis = endpoint.Series == GraphSeries.Remaining ? Plot.Axes.Right : Plot.Axes.Left;
            var color = endpoint.Series switch
            {
                GraphSeries.Remaining => RemainingColor,
                GraphSeries.Sol => SolColor,
                GraphSeries.Terra => TerraColor,
                GraphSeries.Luna => LunaColor,
                _ => MutedColor,
            };
            var connectorX = new double[] { scene.PeriodEndAt, axes.EndpointLabelAt };
            var connector = Plot.Add.Scatter(
                connectorX,
                new double[] { endpoint.PointAxisValue, endpoint.AxisValue },
                color.WithOpacity(0.75));
            connector.Axes.YAxis = axis;
            connector.LineWidth = 1;
            connector.MarkerSize = 0;

            var label = Plot.Add.Text(endpoint.Text, axes.EndpointLabelAt, endpoint.AxisValue);
            label.Axes.YAxis = axis;
            label.Alignment = ScottPlot.Alignment.MiddleLeft;
            label.OffsetX = 4;
            label.LabelFontColor = color;
            label.LabelFontName = "Noto Sans JP Medium";
            label.LabelFontSize = 11;
            label.LabelBackgroundColor = PlotColor.WithOpacity(0.82);
            label.LabelPadding = 2;
            switch (endpoint.Series)
            {
                case GraphSeries.Remaining:
                    remainingLabel = label; remainingConnector = connector; remainingConnectorX = connectorX; break;
                case GraphSeries.Sol:
                    solLabel = label; solConnector = connector; solConnectorX = connectorX; break;
                case GraphSeries.Terra:
                    terraLabel = label; terraConnector = connector; terraConnectorX = connectorX; break;
                case GraphSeries.Luna:
                    lunaLabel = label; lunaConnector = connector; lunaConnectorX = connectorX; break;
            }
        }
    }

    private void ApplyVisibility()
    {
        SetVisible(remainingSeries, remainingConnector, remainingLabel, ShowRemaining);
        SetVisible(solSeries, solConnector, solLabel, ShowModels && ShowSol);
        SetVisible(terraSeries, terraConnector, terraLabel, ShowModels && ShowTerra);
        SetVisible(lunaSeries, lunaConnector, lunaLabel, ShowModels && ShowLuna);
        // AvaPlot.Refresh() always posts at Background priority. Visibility
        // changes originate on the UI thread, so invalidate immediately and
        // let the next compositor frame paint both the toggle and the plot.
        // This avoids an extra dispatcher turn on every series ON/OFF action.
        InvalidateVisual();
    }

    private static void SetVisible(
        ScottPlot.Plottables.Scatter? series,
        ScottPlot.Plottables.Scatter? connector,
        ScottPlot.Plottables.Text? label,
        bool visible)
    {
        if (series is not null) series.IsVisible = visible;
        if (connector is not null) connector.IsVisible = visible;
        if (label is not null) label.IsVisible = visible;
    }

    private static void SetVisible(
        ModelSeriesVisual? series,
        ScottPlot.Plottables.Scatter? connector,
        ScottPlot.Plottables.Text? label,
        bool visible)
    {
        if (series?.Flat is not null) series.Flat.IsVisible = visible;
        if (series?.Rising is not null) series.Rising.IsVisible = visible;
        if (connector is not null) connector.IsVisible = visible;
        if (label is not null) label.IsVisible = visible;
    }

    private sealed record ModelSeriesVisual(
        ScottPlot.Plottables.Scatter? Flat,
        ScottPlot.Plottables.Scatter? Rising);

    private sealed class GraphPlotAutomationPeer : ControlAutomationPeer
    {
        public GraphPlotAutomationPeer(GraphPlotControl owner)
            : base(owner)
        {
        }

        protected override AutomationControlType GetAutomationControlTypeCore() =>
            AutomationControlType.Pane;

        protected override bool IsOffscreenCore()
        {
            var owner = (GraphPlotControl)Owner;
            return !owner.IsEffectivelyVisible ||
                owner.Bounds.Width <= 0 ||
                owner.Bounds.Height <= 0;
        }
    }

}
