// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using Avalonia;
using Avalonia.Automation.Peers;
using Avalonia.Controls;
using Avalonia.Media;
using Avalonia.Platform;
using Avalonia.Rendering.SceneGraph;
using Avalonia.Skia;
using Avalonia.Threading;
using CodexInfo.WindowsClient.Graphing;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.Theme;
using ScottPlot.TickGenerators;

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Thin Avalonia/ScottPlot adapter. All graph calculations are owned by the
/// framework-independent Graphing layer; this control only applies theme,
/// axes, visibility, and projected labels.
/// </summary>
public sealed class GraphPlotControl : Control
{
    internal const string RemainingColorHex = "#56b2f5";
    internal const string SolColorHex = "#a88cf5";
    internal const string TerraColorHex = "#5dc98a";
    internal const string LunaColorHex = "#e6a23c";
    internal const string AstraColorHex = "#ef6a6a";
    internal const string AxisTextColorHex = "#78879c";
    internal const string GridColorHex = "#263850";
    internal const string MidnightGuideColorHex = "#FFFFFF";
    internal const string ResetGuideColorHex = "#D6A45C";
    internal const string PlotColorHex = "#121c2c";
    internal const string RemainingColorRole = ThemePalette.GraphRemaining;
    internal const string SolColorRole = ThemePalette.GraphSol;
    internal const string TerraColorRole = ThemePalette.GraphTerra;
    internal const string LunaColorRole = ThemePalette.GraphLuna;
    internal const string AstraColorRole = ThemePalette.GraphAstra;
    private ScottPlot.Color RemainingColor => new(ThemePalette.Resolve(RemainingColorRole));
    private ScottPlot.Color SolColor => new(ThemePalette.Resolve(SolColorRole));
    private ScottPlot.Color TerraColor => new(ThemePalette.Resolve(TerraColorRole));
    private ScottPlot.Color LunaColor => new(ThemePalette.Resolve(LunaColorRole));
    private ScottPlot.Color AstraColor => new(ThemePalette.Resolve(AstraColorRole));
    internal const string IdleBandColorHex = "#1A2838";
    internal const double IdleBandOpacity = 1.0;
    internal const float MeasuredModelLineWidth = 3f;
    internal const float MeasuredFlatModelLineWidth = MeasuredModelLineWidth;
    internal const float MeasuredRemainingLineWidth = 3f;
    internal const float IdleLineWidth = 1f;
    internal const float InferredLineWidth = 1f;
    internal static string ResolvedIdleBandColorHex => BlendOpaqueHalfUp(
        ThemePalette.Resolve(PlotColorHex),
        ThemePalette.Resolve(IdleBandColorHex));
    private ScottPlot.Color IdleBandColor => new(ResolvedIdleBandColorHex);
    private ScottPlot.Color MutedColor => new(ThemePalette.Resolve(AxisTextColorHex));
    private ScottPlot.Color GridColor => new(ThemePalette.Resolve(GridColorHex));
    private ScottPlot.Color MidnightGuideColor => new ScottPlot.Color(MidnightGuideColorHex).WithOpacity(0.30);
    private ScottPlot.Color ResetGuideColor => new ScottPlot.Color(ResetGuideColorHex).WithOpacity(0.70);
    private ScottPlot.Color PlotColor => new(ThemePalette.Resolve(PlotColorHex));

    private PlotPresentation presentation = new(GraphScene.Empty());
    private double? referenceControlWidth;
    private readonly Dictionary<GraphMetric, double> referenceDataAreaWidths = [];
    private int sceneRevision;

    public GraphPlotControl()
    {
        Focusable = true;
        ClipToBounds = true;
        SizeChanged += OnControlSizeChanged;
        AttachedToVisualTree += (_, _) => ThemePalette.Changed += OnThemeChanged;
        DetachedFromVisualTree += (_, _) => ThemePalette.Changed -= OnThemeChanged;
        ApplyScene();
    }

    private void OnThemeChanged(object? sender, EventArgs eventArgs) => ApplyScene();

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
    public static readonly StyledProperty<bool> ShowAstraProperty =
        AvaloniaProperty.Register<GraphPlotControl, bool>(nameof(ShowAstra), true);

    public ScottPlot.Plot Plot => presentation.Plot;

    public GraphScene Scene { get => GetValue(SceneProperty); set => SetValue(SceneProperty, value); }
    public bool ShowRemaining { get => GetValue(ShowRemainingProperty); set => SetValue(ShowRemainingProperty, value); }
    public bool ShowModels { get => GetValue(ShowModelsProperty); set => SetValue(ShowModelsProperty, value); }
    public bool ShowSol { get => GetValue(ShowSolProperty); set => SetValue(ShowSolProperty, value); }
    public bool ShowTerra { get => GetValue(ShowTerraProperty); set => SetValue(ShowTerraProperty, value); }
    public bool ShowLuna { get => GetValue(ShowLunaProperty); set => SetValue(ShowLunaProperty, value); }
    public bool ShowAstra { get => GetValue(ShowAstraProperty); set => SetValue(ShowAstraProperty, value); }

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
                 change.Property == ShowLunaProperty ||
                 change.Property == ShowAstraProperty)
        {
            ApplyVisibility();
        }
    }

    private void ApplyScene()
    {
        // Build away from the plot held by previously submitted draw operations.
        // Neither a slow preparation nor a failure can expose a partial plot.
        var next = new PlotPresentation(Scene);
        try
        {
            PopulatePlot(next);
        }
        catch
        {
            next.Plot.Dispose();
            throw;
        }

        presentation = next;
        var revision = ++sceneRevision;
        InvalidateVisual();
        if (next.Scene.HasPoints)
        {
            ScheduleReferenceCapture(
                next.Scene.Metric,
                revision,
                (long)next.Plot.RenderManager.RenderCount,
                attemptsRemaining: 20);
        }
    }

    private void PopulatePlot(PlotPresentation presentation)
    {
        ApplyTheme(presentation);

        var scene = presentation.Scene;
        if (!scene.HasPoints && !scene.IsViewport)
        {
            return;
        }

        var axes = BuildAxesForCurrentWidth(scene);
        AddPlotGrid(presentation, scene, axes);
        var idleIntervals = scene.IsViewport
            ? GraphPlotProjection.BuildViewportUnusedIntervals(scene)
            : GraphPlotProjection.BuildVisibleUnusedIntervals(scene);
        foreach (var interval in idleIntervals)
        {
            var band = presentation.Plot.Add.Rectangle(
                interval.StartAt,
                interval.EndAt,
                axes.ModelDisplayMinimum,
                axes.ModelDisplayMaximum);
            band.FillColor = IdleBandColor.WithOpacity(IdleBandOpacity);
            band.LineWidth = 0;
        }
        AddBoundaryGuides(presentation, scene, axes);

        // Match the native graph's painter order: endpoint leaders sit below
        // the data strokes, inferred model paths precede measured paths, and
        // Remaining is painted last over its boundary markers.
        AddEndpointLabels(presentation, scene, axes);
        var lunaLines = BuildModelLines(scene, GraphSeries.Luna);
        var terraLines = BuildModelLines(scene, GraphSeries.Terra);
        var solLines = BuildModelLines(scene, GraphSeries.Sol);
        var astraLines = BuildModelLines(scene, GraphSeries.Astra);
        var lunaDashed = AddLine(presentation, lunaLines.Dashed.Line, LunaColor.WithOpacity(0.72), presentation.Plot.Axes.Left, InferredLineWidth);
        var terraDashed = AddLine(presentation, terraLines.Dashed.Line, TerraColor.WithOpacity(0.72), presentation.Plot.Axes.Left, InferredLineWidth);
        var solDashed = AddLine(presentation, solLines.Dashed.Line, SolColor.WithOpacity(0.72), presentation.Plot.Axes.Left, InferredLineWidth);
        var astraDashed = AddLine(presentation, astraLines.Dashed.Line, AstraColor.WithOpacity(0.72), presentation.Plot.Axes.Left, InferredLineWidth);
        var lunaIdle = AddLine(presentation, lunaLines.Idle.Line, LunaColor.WithOpacity(0.95), presentation.Plot.Axes.Left, IdleLineWidth);
        var terraIdle = AddLine(presentation, terraLines.Idle.Line, TerraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, IdleLineWidth);
        var solIdle = AddLine(presentation, solLines.Idle.Line, SolColor.WithOpacity(0.95), presentation.Plot.Axes.Left, IdleLineWidth);
        var astraIdle = AddLine(presentation, astraLines.Idle.Line, AstraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, IdleLineWidth);
        var lunaFlat = AddLine(presentation, lunaLines.Flat.Line, LunaColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredFlatModelLineWidth);
        var astraFlat = AddLine(presentation, astraLines.Flat.Line, AstraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredFlatModelLineWidth);
        var astraRising = AddLine(presentation, astraLines.Rising.Line, AstraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredModelLineWidth);
        var lunaRising = AddLine(presentation, lunaLines.Rising.Line, LunaColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredModelLineWidth);
        var terraFlat = AddLine(presentation, terraLines.Flat.Line, TerraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredFlatModelLineWidth);
        var terraRising = AddLine(presentation, terraLines.Rising.Line, TerraColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredModelLineWidth);
        var solFlat = AddLine(presentation, solLines.Flat.Line, SolColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredFlatModelLineWidth);
        var solRising = AddLine(presentation, solLines.Rising.Line, SolColor.WithOpacity(0.95), presentation.Plot.Axes.Left, MeasuredModelLineWidth);
        presentation.LunaSeries = new ModelSeriesVisual(lunaIdle, lunaFlat, lunaRising, lunaDashed);
        presentation.TerraSeries = new ModelSeriesVisual(terraIdle, terraFlat, terraRising, terraDashed);
        presentation.SolSeries = new ModelSeriesVisual(solIdle, solFlat, solRising, solDashed);
        presentation.AstraSeries = new ModelSeriesVisual(astraIdle, astraFlat, astraRising, astraDashed);
        var remainingLines = scene.IsViewport
            ? GraphPlotProjection.BuildViewportRemainingLines(scene)
            : GraphPlotProjection.PrepareGeometry(scene).RemainingLines;
        presentation.RemainingDashedSeries = AddLine(
            presentation,
            remainingLines.Dashed.Line,
            RemainingColor.WithOpacity(0.72),
            presentation.Plot.Axes.Right,
            InferredLineWidth);
        presentation.RemainingIdleSeries = AddLine(
            presentation,
            remainingLines.Idle.Line,
            RemainingColor,
            presentation.Plot.Axes.Right,
            IdleLineWidth);
        presentation.RemainingMarkers = AddRemainingMarkers(presentation, scene);
        presentation.RemainingSeries = AddLine(
            presentation,
            remainingLines.Solid.Line,
            RemainingColor,
            presentation.Plot.Axes.Right,
            MeasuredRemainingLineWidth);
        ApplyAxes(presentation, scene, axes);
        ApplyVisibility(presentation);
    }

    private static GraphCanonicalModelLineProjection BuildModelLines(GraphScene scene, GraphSeries series) =>
        scene.IsViewport
            ? GraphPlotProjection.BuildViewportModelLines(scene, series)
            : GraphPlotProjection.PrepareGeometry(scene).ModelLines[series];

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

    private ScottPlot.Plottables.Scatter? AddLine(
        PlotPresentation presentation,
        GraphLineProjection line,
        ScottPlot.Color color,
        ScottPlot.IYAxis axis,
        float lineWidth)
    {
        if (line.X.Count < 2)
        {
            return null;
        }
        var series = presentation.Plot.Add.Scatter(line.X.ToArray(), line.Y.ToArray(), color);
        series.Axes.YAxis = axis;
        series.LineWidth = lineWidth;
        series.MarkerSize = 0;
        return series;
    }

    private ScottPlot.Plottables.Scatter? AddRemainingMarkers(PlotPresentation presentation, GraphScene scene)
    {
        var markers = scene.IsViewport
            ? GraphPlotProjection.BuildViewportRemainingMarkers(scene)
            : GraphPlotProjection.PrepareGeometry(scene).RemainingMarkers;
        if (markers.Count == 0)
        {
            return null;
        }
        var span = scene.PeriodEndAt - scene.PeriodStartAt;
        var x = markers
            .Select(marker => scene.PeriodStartAt + marker.X / 100 * span)
            .ToArray();
        var y = markers
            .Select(marker => Math.Clamp((99 - marker.YTop) / 0.98, 0, 100))
            .ToArray();
        var series = presentation.Plot.Add.Scatter(x, y, RemainingColor);
        series.Axes.YAxis = presentation.Plot.Axes.Right;
        series.LineWidth = 0;
        series.MarkerSize = 2;
        return series;
    }

    private void ApplyTheme(PlotPresentation presentation)
    {
        presentation.Plot.FigureBackground.Color = PlotColor;
        presentation.Plot.DataBackground.Color = PlotColor;
        presentation.Plot.Axes.ContinuouslyAutoscale = false;
        presentation.Plot.Axes.Color(MutedColor);
        presentation.Plot.Axes.FrameColor(GridColor);
        presentation.Plot.Grid.MajorLineColor = GridColor;
        presentation.Plot.Grid.MinorLineColor = GridColor.WithOpacity(0.35);
        // ScottPlot's built-in horizontal grid spans the endpoint-label
        // gutter. X keeps the gutter clear, so bounded grid segments are
        // painted explicitly by AddPlotGrid(presentation).
        presentation.Plot.Grid.MajorLineWidth = 0;
        presentation.Plot.Grid.MinorLineWidth = 0;
        presentation.Plot.Font.Set("Noto Sans JP Medium");
    }

    private void AddPlotGrid(PlotPresentation presentation, GraphScene scene, GraphAxisProjection axes)
    {
        foreach (var y in axes.ModelValues)
        {
            AddLine(
                presentation,
                new GraphLineProjection(
                    new double[] { scene.PeriodStartAt, scene.PeriodEndAt },
                    new double[] { y, y }),
                GridColor,
                presentation.Plot.Axes.Left,
                1f);
        }
        foreach (var x in axes.BottomValues)
        {
            AddLine(
                presentation,
                new GraphLineProjection(
                    new double[] { x, x },
                    new double[] { axes.ModelDisplayMinimum, axes.ModelDisplayMaximum }),
                GridColor,
                presentation.Plot.Axes.Left,
                1f);
        }
    }

    private void AddBoundaryGuides(
        PlotPresentation presentation,
        GraphScene scene,
        GraphAxisProjection axes)
    {
        var resetGuides = GraphPlotProjection.BuildResetGuides(scene);
        foreach (var timestamp in axes.MidnightGuideTimestamps)
        {
            if (resetGuides.Contains(timestamp))
            {
                continue;
            }
            AddLine(
                presentation,
                new GraphLineProjection(
                    new double[] { timestamp, timestamp },
                    new double[] { axes.ModelDisplayMinimum, axes.ModelDisplayMaximum }),
                MidnightGuideColor,
                presentation.Plot.Axes.Left,
                0.5f);
        }
        foreach (var timestamp in resetGuides)
        {
            AddLine(
                presentation,
                new GraphLineProjection(
                    new double[] { timestamp, timestamp },
                    new double[] { axes.ModelDisplayMinimum, axes.ModelDisplayMaximum }),
                ResetGuideColor,
                presentation.Plot.Axes.Left,
                1f);
        }
    }

    private void ApplyAxes(PlotPresentation presentation, GraphScene scene, GraphAxisProjection axes)
    {
        ApplyLimits(presentation, scene, axes);
        ApplyTopDateAxis(presentation, axes);
        presentation.Plot.Axes.Bottom.TickGenerator = new NumericManual(
            axes.BottomValues.ToArray(),
            axes.BottomLabels.ToArray());
        presentation.Plot.Axes.Left.TickGenerator = new NumericManual(
            axes.ModelValues.ToArray(),
            axes.ModelLabels.ToArray());
        presentation.Plot.Axes.Right.TickGenerator = new NumericManual(
            axes.RemainingValues.ToArray(),
            axes.RemainingLabels.ToArray());
        // The native graph owns remaining-percent semantics with its coloured
        // endpoint label. A second set of frame ticks steals the dedicated
        // label gutter and is not part of the X graph.
        presentation.Plot.Axes.Right.IsVisible = false;
    }

    private static void ApplyTopDateAxis(PlotPresentation presentation, GraphAxisProjection axes)
    {
        var topAxis = presentation.Plot.Axes.Top;
        topAxis.IsVisible = axes.TopDateValues.Count > 0;
        topAxis.TickGenerator = new NumericManual(
            axes.TopDateValues.ToArray(),
            axes.TopDateLabels.ToArray());
        topAxis.TickLabelStyle.FontName = "Noto Sans JP Medium";
        topAxis.TickLabelStyle.FontSize = 10;
    }

    private void ApplyLimits(PlotPresentation presentation, GraphScene scene, GraphAxisProjection axes)
    {
        presentation.Plot.Axes.SetLimits(
            scene.PeriodStartAt,
            axes.DisplayEndAt,
            axes.ModelDisplayMinimum,
            axes.ModelDisplayMaximum,
            presentation.Plot.Axes.Bottom,
            presentation.Plot.Axes.Left);
        presentation.Plot.Axes.SetLimits(
            scene.PeriodStartAt,
            axes.DisplayEndAt,
            axes.ModelDisplayMinimum,
            axes.ModelDisplayMaximum,
            presentation.Plot.Axes.Top,
            presentation.Plot.Axes.Left);
        presentation.Plot.Axes.SetLimitsY(
            axes.RemainingDisplayMinimum,
            axes.RemainingDisplayMaximum,
            presentation.Plot.Axes.Right);
    }

    private static string BlendOpaqueHalfUp(string first, string second)
    {
        static int Channel(string color, int offset) => Convert.ToInt32(color.Substring(offset, 2), 16);
        static int Average(int left, int right) => (left + right + 1) / 2;

        return $"#{Average(Channel(first, 1), Channel(second, 1)):X2}" +
            $"{Average(Channel(first, 3), Channel(second, 3)):X2}" +
            $"{Average(Channel(first, 5), Channel(second, 5)):X2}";
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

        var scene = presentation.Scene;
        if ((!scene.HasPoints && !scene.IsViewport) || referenceControlWidth is null)
        {
            return;
        }
        if (!referenceDataAreaWidths.ContainsKey(scene.Metric))
        {
            var revision = ++sceneRevision;
            ScheduleReferenceCapture(
                scene.Metric,
                revision,
                (long)presentation.Plot.RenderManager.RenderCount,
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
            var currentScene = presentation.Scene;
            if (sceneRevision != revision ||
                !currentScene.HasPoints ||
                currentScene.Metric != metric ||
                referenceDataAreaWidths.ContainsKey(metric))
            {
                return;
            }

            long renderCount;
            double currentDataAreaWidth;
            lock (presentation.Plot.Sync)
            {
                renderCount = (long)presentation.Plot.RenderManager.RenderCount;
                currentDataAreaWidth = presentation.Plot.LastRender.DataRect.Width;
            }
            if (renderCount <= priorRenderCount)
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
        lock (presentation.Plot.Sync)
        {
            ApplyLimits(presentation, scene, axes);
            ApplyTopDateAxis(presentation, axes);
            UpdateEndpointLayout(presentation.RemainingConnectorX, presentation.RemainingLabel, axes.EndpointLabelAt);
            UpdateEndpointLayout(presentation.SolConnectorX, presentation.SolLabel, axes.EndpointLabelAt);
            UpdateEndpointLayout(presentation.TerraConnectorX, presentation.TerraLabel, axes.EndpointLabelAt);
            UpdateEndpointLayout(presentation.LunaConnectorX, presentation.LunaLabel, axes.EndpointLabelAt);
            UpdateEndpointLayout(presentation.AstraConnectorX, presentation.AstraLabel, axes.EndpointLabelAt);
        }
        InvalidateVisual();
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

    private void AddEndpointLabels(PlotPresentation presentation, GraphScene scene, GraphAxisProjection axes)
    {
        foreach (var endpoint in GraphPlotProjection.BuildEndpointLabels(scene, CultureInfo.CurrentCulture))
        {
            var axis = endpoint.Series == GraphSeries.Remaining ? presentation.Plot.Axes.Right : presentation.Plot.Axes.Left;
            var color = endpoint.Series switch
            {
                GraphSeries.Remaining => RemainingColor,
                GraphSeries.Sol => SolColor,
                GraphSeries.Terra => TerraColor,
                GraphSeries.Luna => LunaColor,
                GraphSeries.Astra => AstraColor,
                _ => MutedColor,
            };
            var connectorX = new double[] { scene.PeriodEndAt, axes.EndpointLabelAt };
            var connector = presentation.Plot.Add.Scatter(
                connectorX,
                new double[] { endpoint.PointAxisValue, endpoint.AxisValue },
                color.WithOpacity(0.8));
            connector.Axes.YAxis = axis;
            connector.LineWidth = 1;
            connector.MarkerSize = 0;

            var label = presentation.Plot.Add.Text(endpoint.Text, axes.EndpointLabelAt, endpoint.AxisValue);
            label.Axes.YAxis = axis;
            label.Alignment = ScottPlot.Alignment.MiddleLeft;
            label.OffsetX = 0;
            label.LabelFontColor = color;
            label.LabelFontName = "Noto Sans JP Medium";
            label.LabelFontSize = 10;
            label.LabelBackgroundColor = PlotColor.WithOpacity(0);
            label.LabelPadding = 0;
            switch (endpoint.Series)
            {
                case GraphSeries.Remaining:
                    presentation.RemainingLabel = label; presentation.RemainingConnector = connector; presentation.RemainingConnectorX = connectorX; break;
                case GraphSeries.Sol:
                    presentation.SolLabel = label; presentation.SolConnector = connector; presentation.SolConnectorX = connectorX; break;
                case GraphSeries.Terra:
                    presentation.TerraLabel = label; presentation.TerraConnector = connector; presentation.TerraConnectorX = connectorX; break;
                case GraphSeries.Luna:
                    presentation.LunaLabel = label; presentation.LunaConnector = connector; presentation.LunaConnectorX = connectorX; break;
                case GraphSeries.Astra:
                    presentation.AstraLabel = label; presentation.AstraConnector = connector; presentation.AstraConnectorX = connectorX; break;
                default:
                    connector.IsVisible = false;
                    label.IsVisible = false;
                    break;
            }
        }
    }

    private void ApplyVisibility()
    {
        lock (presentation.Plot.Sync)
        {
            ApplyVisibility(presentation);
        }
        InvalidateVisual();
    }

    private void ApplyVisibility(PlotPresentation presentation)
    {
        SetVisible(
            presentation.RemainingSeries,
            presentation.RemainingIdleSeries,
            presentation.RemainingDashedSeries,
            presentation.RemainingMarkers,
            presentation.RemainingConnector,
            presentation.RemainingLabel,
            ShowRemaining);
        SetVisible(presentation.SolSeries, presentation.SolConnector, presentation.SolLabel, ShowModels && ShowSol);
        SetVisible(presentation.TerraSeries, presentation.TerraConnector, presentation.TerraLabel, ShowModels && ShowTerra);
        SetVisible(presentation.LunaSeries, presentation.LunaConnector, presentation.LunaLabel, ShowModels && ShowLuna);
        SetVisible(presentation.AstraSeries, presentation.AstraConnector, presentation.AstraLabel, ShowModels && ShowAstra);
    }

    private static void SetVisible(
        ScottPlot.Plottables.Scatter? series,
        ScottPlot.Plottables.Scatter? idleSeries,
        ScottPlot.Plottables.Scatter? dashedSeries,
        ScottPlot.Plottables.Scatter? markers,
        ScottPlot.Plottables.Scatter? connector,
        ScottPlot.Plottables.Text? label,
        bool visible)
    {
        if (series is not null) series.IsVisible = visible;
        if (idleSeries is not null) idleSeries.IsVisible = visible;
        if (dashedSeries is not null) dashedSeries.IsVisible = visible;
        if (markers is not null) markers.IsVisible = visible;
        if (connector is not null) connector.IsVisible = visible;
        if (label is not null) label.IsVisible = visible;
    }

    private static void SetVisible(
        ModelSeriesVisual? series,
        ScottPlot.Plottables.Scatter? connector,
        ScottPlot.Plottables.Text? label,
        bool visible)
    {
        if (series?.Idle is not null) series.Idle.IsVisible = visible;
        if (series?.Flat is not null) series.Flat.IsVisible = visible;
        if (series?.Rising is not null) series.Rising.IsVisible = visible;
        if (series?.Dashed is not null) series.Dashed.IsVisible = visible;
        if (connector is not null) connector.IsVisible = visible;
        if (label is not null) label.IsVisible = visible;
    }

    public override void Render(DrawingContext context)
    {
        // Avalonia may execute this operation after another Scene is accepted.
        // Capture the complete plot rather than a mutable control/multiplot.
        context.Custom(new GraphPlotDrawOperation(new Rect(Bounds.Size), presentation.Plot));
    }

    private sealed class GraphPlotDrawOperation(Rect bounds, ScottPlot.Plot plot) : ICustomDrawOperation
    {
        public Rect Bounds { get; } = bounds;

        public bool HitTest(Point point) => Bounds.Contains(point);

        public bool Equals(ICustomDrawOperation? other) => false;

        public void Dispose()
        {
            // A plot can belong to several queued operations. Its ordinary
            // graph primitives are managed and remain alive with this snapshot.
        }

        public void Render(ImmediateDrawingContext context)
        {
            var feature = context.TryGetFeature<ISkiaSharpApiLeaseFeature>();
            if (feature is null)
            {
                return;
            }
            using var lease = feature.Lease();
            using var canvasState = new SkiaSharp.SKAutoCanvasRestore(lease.SkCanvas, false);
            lease.SkCanvas.SaveLayer();
            plot.Render(lease.SkCanvas, new ScottPlot.PixelRect(0, (float)Bounds.Width, (float)Bounds.Height, 0));
        }
    }

    private sealed class PlotPresentation(GraphScene scene)
    {
        public GraphScene Scene { get; } = scene;

        public ScottPlot.Plot Plot { get; } = new();

        public ScottPlot.Plottables.Scatter? RemainingSeries;
        public ScottPlot.Plottables.Scatter? RemainingIdleSeries;
        public ScottPlot.Plottables.Scatter? RemainingDashedSeries;
        public ScottPlot.Plottables.Scatter? RemainingMarkers;
        public ModelSeriesVisual? SolSeries;
        public ModelSeriesVisual? TerraSeries;
        public ModelSeriesVisual? LunaSeries;
        public ModelSeriesVisual? AstraSeries;
        public ScottPlot.Plottables.Scatter? RemainingConnector;
        public ScottPlot.Plottables.Scatter? SolConnector;
        public ScottPlot.Plottables.Scatter? TerraConnector;
        public ScottPlot.Plottables.Scatter? LunaConnector;
        public ScottPlot.Plottables.Scatter? AstraConnector;
        public ScottPlot.Plottables.Text? RemainingLabel;
        public ScottPlot.Plottables.Text? SolLabel;
        public ScottPlot.Plottables.Text? TerraLabel;
        public ScottPlot.Plottables.Text? LunaLabel;
        public ScottPlot.Plottables.Text? AstraLabel;
        public double[] RemainingConnectorX = [];
        public double[] SolConnectorX = [];
        public double[] TerraConnectorX = [];
        public double[] LunaConnectorX = [];
        public double[] AstraConnectorX = [];
    }

    private sealed record ModelSeriesVisual(
        ScottPlot.Plottables.Scatter? Idle,
        ScottPlot.Plottables.Scatter? Flat,
        ScottPlot.Plottables.Scatter? Rising,
        ScottPlot.Plottables.Scatter? Dashed);

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
