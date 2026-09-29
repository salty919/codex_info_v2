// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Media;
using Avalonia.Styling;

namespace CodexInfo.WindowsClient.Theme;

/// <summary>The three built-in Windows palettes. Keys are the existing dark colors.</summary>
public static class ThemePalette
{
    public const string ClassicDark = "classic-dark";
    public const string GraphiteDark = "graphite-dark";
    public const string Light = "light";

    public static IReadOnlyList<string> PresetIds { get; } = [ClassicDark, GraphiteDark, Light];

    // A one-to-one mapping keeps the existing dark appearance exact, including
    // semantic status and model colors, while changing every direct consumer.
    private static readonly IReadOnlyDictionary<string, (string Graphite, string Light)> Colors =
        new Dictionary<string, (string, string)>(StringComparer.OrdinalIgnoreCase)
        {
            ["#0A111A"] = ("#15171B", "#F1F4F8"),
            ["#0E141E"] = ("#181A1F", "#F4F7FB"),
            ["#101925"] = ("#1D2026", "#F0F4F8"),
            ["#101F2D"] = ("#20242B", "#EAF0F6"),
            ["#111A27"] = ("#20232A", "#E9EFF6"),
            ["#111B2C"] = ("#222730", "#EEF3F8"),
            ["#121C2C"] = ("#20242B", "#FFFFFF"),
            ["#143426"] = ("#18362A", "#E5F5EC"),
            ["#151F2D"] = ("#242830", "#FFFFFF"),
            ["#172C42"] = ("#1D354A", "#E7F1FA"),
            ["#18283A"] = ("#292F39", "#EAF0F6"),
            ["#1A2838"] = ("#303944", "#E4EDF5"),
            ["#1B2735"] = ("#292D34", "#EBEFF3"),
            ["#1B2838"] = ("#292F39", "#EAF0F6"),
            ["#1B2A3A"] = ("#2B313A", "#E8EEF5"),
            ["#1C5D91"] = ("#285A80", "#D7EAF7"),
            ["#1D2A38"] = ("#292D34", "#EDF1F5"),
            ["#236B9E"] = ("#2676A8", "#176AAB"),
            ["#243E5A"] = ("#343E4B", "#DDEAF5"),
            ["#24415E"] = ("#30485D", "#D7EAF7"),
            ["#244D74"] = ("#344D63", "#D9EBF8"),
            ["#263548"] = ("#3A444F", "#D5DFE9"),
            ["#263850"] = ("#3C4652", "#CFD9E4"),
            ["#276C49"] = ("#327653", "#4A9469"),
            ["#294968"] = ("#35506A", "#D9E9F7"),
            ["#2A3A4B"] = ("#454C55", "#C7D0DA"),
            ["#2B425B"] = ("#4B5A6B", "#B6C5D4"),
            ["#2C4258"] = ("#424B57", "#C5D0DC"),
            ["#2D3D56"] = ("#444F5C", "#C0CDDA"),
            ["#2D6193"] = ("#3972A0", "#5A91C2"),
            ["#304052"] = ("#454C55", "#C7D0DA"),
            ["#304A63"] = ("#4B596B", "#B6C5D4"),
            ["#326799"] = ("#4A6B89", "#A9CDE8"),
            ["#356087"] = ("#3D617F", "#C7E0F4"),
            ["#36516B"] = ("#506071", "#B8C8D7"),
            ["#3A1D24"] = ("#41262B", "#FDECEF"),
            ["#3A2A13"] = ("#42331B", "#FFF3DC"),
            ["#3B506F"] = ("#536073", "#B6C5D4"),
            ["#405779"] = ("#54677C", "#AEC1D2"),
            ["#47769F"] = ("#5B7E9E", "#82A9CB"),
            ["#4FB878"] = ("#5CC88A", "#176E42"),
            ["#56B2F5"] = ("#69B5F7", "#176AAB"),
            ["#5DC98A"] = ("#70CF99", "#16794B"),
            ["#5EA7E5"] = ("#6EB5E9", "#267DB7"),
            ["#61B4EB"] = ("#72BDEA", "#267DB7"),
            ["#718196"] = ("#9AA8B8", "#5F7183"),
            ["#71C7FF"] = ("#84C7F3", "#176AAB"),
            ["#71D39A"] = ("#83D4A7", "#16794B"),
            ["#76A7CC"] = ("#8CACBF", "#6B839A"),
            ["#78879C"] = ("#A2ADB8", "#526579"),
            ["#8A651F"] = ("#A98037", "#B8892D"),
            ["#8BD4FF"] = ("#9AD7F8", "#176AAB"),
            ["#8E3D4D"] = ("#AB5362", "#C77382"),
            ["#8EA1B5"] = ("#9DAAB6", "#687A8A"),
            ["#9FB0C5"] = ("#B5BEC9", "#526579"),
            ["#A88CF5"] = ("#B69CF1", "#6A4BCC"),
            ["#A8B7CA"] = ("#B5BEC9", "#526579"),
            ["#B79BFF"] = ("#C5AFF5", "#6A4BCC"),
            ["#B8C6D7"] = ("#C9D0DA", "#526579"),
            ["#D5A43A"] = ("#E0B255", "#805800"),
            ["#E06B7A"] = ("#E5808C", "#A32746"),
            ["#E6A23C"] = ("#EBB460", "#985F08"),
            ["#E6B85C"] = ("#ECC779", "#805800"),
            ["#E7C26A"] = ("#EBCF89", "#805800"),
            ["#E86E9F"] = ("#EC87AC", "#B23553"),
            ["#E9EFF8"] = ("#F1F3F5", "#1C2834"),
            ["#EAF2FB"] = ("#F2F4F7", "#1C2834"),
            ["#EF6A6A"] = ("#EF8585", "#B23553"),
            ["#F0A35B"] = ("#F2BA82", "#985F08"),
            ["#F1B35A"] = ("#EFC176", "#985F08"),
            ["#F2F6FC"] = ("#F7F8FA", "#1C2834"),
            ["#F5FAFF"] = ("#F8FBFF", "#FFFFFF"),
        };

    private static IReadOnlyDictionary<string, IBrush> brushes = CreateBrushes(ClassicDark);
    public static string CurrentId { get; private set; } = ClassicDark;
    public static event EventHandler? Changed;

    public static bool IsValid(string? id) => id is ClassicDark or GraphiteDark or Light;

    public static string Resolve(string classicColor)
    {
        if (!Colors.TryGetValue(classicColor, out var alternatives))
        {
            throw new ArgumentOutOfRangeException(nameof(classicColor), classicColor, "Unregistered theme color.");
        }
        return CurrentId switch
        {
            GraphiteDark => alternatives.Graphite,
            Light => alternatives.Light,
            _ => classicColor,
        };
    }

    public static IBrush Brush(string classicColor) =>
        brushes.TryGetValue(classicColor, out var brush)
            ? brush
            : throw new ArgumentOutOfRangeException(nameof(classicColor), classicColor, "Unregistered theme color.");

    private static IReadOnlyDictionary<string, IBrush> CreateBrushes(string id) =>
        Colors.ToDictionary(
            entry => entry.Key,
            entry => (IBrush)new SolidColorBrush(Color.Parse(id switch
            {
                GraphiteDark => entry.Value.Graphite,
                Light => entry.Value.Light,
                _ => entry.Key,
            })),
            StringComparer.OrdinalIgnoreCase);

    public static void Apply(string id)
    {
        if (!IsValid(id)) throw new ArgumentOutOfRangeException(nameof(id));
        var changed = CurrentId != id;
        if (changed) brushes = CreateBrushes(id);
        CurrentId = id;
        if (Application.Current is { } app)
        {
            app.RequestedThemeVariant = id == Light ? ThemeVariant.Light : ThemeVariant.Dark;
            foreach (var (classic, brush) in brushes)
            {
                app.Resources["Theme" + classic[1..].ToUpperInvariant()] = brush;
            }
        }
        if (changed) Changed?.Invoke(null, EventArgs.Empty);
    }
}
