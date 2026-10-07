// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using Avalonia.Media;
using Avalonia.Styling;

namespace CodexInfo.WindowsClient.Theme;

/// <summary>Built-in Windows palettes. Keys are the existing dark colors.</summary>
public static class ThemePalette
{
    public const string ClassicDark = "classic-dark";
    public const string GraphiteDark = "graphite-dark";
    public const string Light = "light";
    public const string PaperLight = "paper-light";
    public const string SandLight = "sand-light";
    public const string SteelLight = "steel-light";
    public const string OceanDark = "ocean-dark";
    public const string TealDark = "teal-dark";
    public const string EmberDark = "ember-dark";
    public const string InkDark = "ink-dark";

    public const string NeonDark = "neon-dark";
    public const string LavenderLight = "lavender-light";
    public const string MintLight = "mint-light";
    public const string ForestDark = "forest-dark";
    public const string TangerineDark = "tangerine-dark";
    public const string RoseDark = "rose-dark";

    public const string GraphRemaining = "GraphRemaining";
    public const string GraphLuna = "GraphLuna";
    public const string GraphTerra = "GraphTerra";
    public const string GraphSol = "GraphSol";
    public const string GraphAstra = "GraphAstra";

    public static IReadOnlyList<string> PresetIds { get; } =
    [
        ClassicDark, GraphiteDark, Light, PaperLight, SandLight, SteelLight,
        OceanDark, TealDark, EmberDark, InkDark,
        NeonDark, LavenderLight, MintLight, ForestDark, TangerineDark, RoseDark,
    ];

    // The classic mapping preserves status and model colors. The documented
    // parent-card correction improves running-text contrast.
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
            ["#243E5A"] = ("#303844", "#DDEAF5"),
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

    private static readonly string[] RoleKeys =
    [
        "#0E141E", "#151F2D", "#E9EFF8", "#A8B7CA", "#56B2F5", "#121C2C", "#263850", "#1A2838",
        "#326799", "#143426", "#276C49", "#4FB878", "#243E5A", "#2B425B", "#76A7CC", "#EF6A6A",
        "#111B2C", "#244D74", "#8BD4FF",
    ];

    private static readonly IReadOnlyDictionary<string, (string Dark, string Light)> GraphRoleColors =
        new Dictionary<string, (string, string)>(StringComparer.OrdinalIgnoreCase)
        {
            [GraphRemaining] = ("#56B2F5", "#176AAB"),
            [GraphLuna] = ("#E6A23C", "#985F08"),
            [GraphTerra] = ("#5DC98A", "#16794B"),
            [GraphSol] = ("#A88CF5", "#6A4BCC"),
            [GraphAstra] = ("#EF6A6A", "#B23553"),
        };

    private static readonly HashSet<string> LightThemeIds = new(StringComparer.Ordinal)
    {
        Light, PaperLight, SandLight, SteelLight, LavenderLight, MintLight,
    };

    private sealed record AdditionalPreset(
        bool UsesLightBase,
        int RedDelta,
        int GreenDelta,
        int BlueDelta,
        IReadOnlyDictionary<string, string> Overrides);

    private static readonly IReadOnlyDictionary<string, AdditionalPreset> AdditionalPresets =
        new Dictionary<string, AdditionalPreset>(StringComparer.Ordinal)
        {
            [PaperLight] = CreatePreset(true, 3, 1, -5,
                ["#F7F6F2", "#FFFFFC", "#252B31", "#59636B", "#356C91", "#FFFFFC", "#D8E0E5", "#ECEFEB", "#B7CDD8", "#E6F3EB", "#5C9976", "#216543", "#E2EDF2", "#B7C7D0", "#708D9E", "#B23553", "#F1F4F1", "#DAE9EE", "#276A91"]),
            [SandLight] = CreatePreset(true, 10, 3, -22,
                ["#FDF6E3", "#FFFBEF", "#334650", "#566367", "#1B748A", "#FFFBEF", "#C9D6D2", "#EBE4D2", "#B3C9C5", "#E3F0E2", "#6D9B72", "#2B714A", "#E1E9D9", "#BCCBBC", "#728D84", "#A83D48", "#F5EDDA", "#DCE8DB", "#126A7F"]),
            [SteelLight] = CreatePreset(true, -3, -1, 3,
                ["#F3F5F8", "#FFFFFF", "#202B38", "#586978", "#275FA8", "#FFFFFF", "#D5DEE9", "#E8EEF5", "#A9C4E1", "#E5F2EA", "#6EAA83", "#1E7047", "#DDE9F6", "#BACBDD", "#728BA9", "#B42F49", "#EDF2F8", "#D7E5F7", "#275FA8"]),
            [OceanDark] = CreatePreset(false, -8, 0, 14,
                ["#10182A", "#18263D", "#EAF3FF", "#ACBED3", "#56B8F2", "#111D33", "#304968", "#1E304B", "#3D668B", "#16372F", "#3A8266", "#72CDA3", "#213958", "#45617F", "#82A9C5", "#F28B9B", "#192B45", "#284B70", "#7CD2FF"]),
            [TealDark] = CreatePreset(false, -22, 14, 13,
                ["#002B36", "#073642", "#E6F0E9", "#A8C0BC", "#4FB3C3", "#073642", "#3C6570", "#174550", "#3C7583", "#124B40", "#47866A", "#79CAA3", "#123A46", "#3D6C75", "#76AEB3", "#F47D88", "#0B3D49", "#1E5B67", "#74D2DB"],
                new Dictionary<string, string> { ["#236B9E"] = "#0D759F" }),
            [EmberDark] = CreatePreset(false, 11, 5, -6,
                ["#202126", "#2B2D32", "#F4F0E9", "#BCBDB7", "#E7BC62", "#26272C", "#505258", "#35373D", "#77725F", "#244437", "#5D9974", "#9FDC9E", "#3D3D47", "#686973", "#A5A3A0", "#F58A94", "#303238", "#555147", "#F2CB78"]),
            [InkDark] = CreatePreset(false, 0, 0, 0,
                ["#000000", "#121212", "#FFFFFF", "#D8D8D8", "#6DD3FF", "#050505", "#787878", "#242424", "#808080", "#002B17", "#78E8A4", "#78E8A4", "#202A34", "#FFFFFF", "#FFFFFF", "#FF8BA1", "#101010", "#174A66", "#FFFFFF"]),
            [NeonDark] = CreatePreset(false, 0, 0, 0,
                ["#16122A", "#241C3B", "#F2ECFF", "#C4B8E2", "#70CBFF", "#1B1530", "#493B68", "#302448", "#37436A", "#163D33", "#4D8B70", "#8BDEB6", "#33264F", "#63517D", "#AF9AD0", "#FF929F", "#2C2144", "#493369", "#B19BFF"]),
            [LavenderLight] = CreatePreset(true, 0, 0, 0,
                ["#F2EAFB", "#FFFAFF", "#29233C", "#615570", "#7046AE", "#FFFAFF", "#D8CBE3", "#EBE0F4", "#DDD4EF", "#E3F3E9", "#5C9571", "#216C44", "#E7DDF5", "#BCAACE", "#78658F", "#AC2853", "#EFE7F8", "#DDD0EF", "#7046AE"]),
            [MintLight] = CreatePreset(true, 0, 0, 0,
                ["#E8F7EE", "#F7FFFA", "#19382F", "#3F5F50", "#14765F", "#F7FFFA", "#C8DECF", "#DDEDE3", "#BEDCCD", "#D9F2E1", "#579772", "#207343", "#D7EDE0", "#A8C8B5", "#4D806C", "#AF2D4C", "#E8F6EC", "#C8E8D6", "#11735B"]),
            [ForestDark] = CreatePreset(false, 0, 0, 0,
                ["#11231B", "#1B3427", "#EDF8EA", "#ADC9B5", "#94DB75", "#14291E", "#355642", "#263F30", "#40623A", "#193E29", "#508264", "#8CDCAC", "#284833", "#526F5B", "#8BAF90", "#FF949B", "#203F2E", "#31533A", "#B5EA94"]),
            [TangerineDark] = CreatePreset(false, 0, 0, 0,
                ["#29180F", "#3B261A", "#FFF3E5", "#DFC1A5", "#FFB46E", "#2C1D13", "#614533", "#453022", "#735035", "#213D28", "#567D59", "#A3D892", "#4D3424", "#876346", "#D3A37A", "#FF9A96", "#3D2B1D", "#68462C", "#FFCA86"]),
            [RoseDark] = CreatePreset(false, 0, 0, 0,
                ["#281523", "#3A2233", "#FCECF5", "#DAB9CD", "#F49DC7", "#2D1B29", "#604258", "#482D3F", "#704762", "#1D3D32", "#507E68", "#9CDBBC", "#4C2F44", "#835A75", "#CC94B5", "#FF969F", "#412838", "#67425B", "#FFBDDF"]),
        };

    private static AdditionalPreset CreatePreset(
        bool usesLightBase, int redDelta, int greenDelta, int blueDelta, string[] roleColors,
        IReadOnlyDictionary<string, string>? additionalOverrides = null)
    {
        var overrides = RoleKeys.Select((key, index) => (key, value: roleColors[index]))
            .ToDictionary(pair => pair.key, pair => pair.value, StringComparer.OrdinalIgnoreCase);
        if (additionalOverrides is not null)
        {
            foreach (var (key, value) in additionalOverrides) overrides[key] = value;
        }
        return new(usesLightBase, redDelta, greenDelta, blueDelta, overrides);
    }

    private static readonly IReadOnlyDictionary<string, string> ClassicOverrides =
        new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["#243E5A"] = "#1A2C40",
        };

    private static IReadOnlyDictionary<string, IBrush> brushes = CreateBrushes(ClassicDark);
    public static string CurrentId { get; private set; } = ClassicDark;
    public static event EventHandler? Changed;

    public static bool IsValid(string? id) => id is ClassicDark or GraphiteDark or Light
        or PaperLight or SandLight or SteelLight or OceanDark or TealDark or EmberDark or InkDark
        or NeonDark or LavenderLight or MintLight or ForestDark or TangerineDark or RoseDark;

    public static string Resolve(string classicColor) => GraphRoleColors.ContainsKey(classicColor)
        ? ResolveGraphRoleFor(CurrentId, classicColor)
        : ResolveFor(CurrentId, classicColor);

    private static string ResolveGraphRoleFor(string id, string roleKey)
    {
        if (!GraphRoleColors.TryGetValue(roleKey, out var colors))
        {
            throw new ArgumentOutOfRangeException(nameof(roleKey), roleKey, "Unknown graph color role.");
        }

        return LightThemeIds.Contains(id) ? colors.Light : colors.Dark;
    }

    private static string ResolveFor(string id, string classicColor)
    {
        if (!Colors.TryGetValue(classicColor, out var alternatives))
        {
            throw new ArgumentOutOfRangeException(nameof(classicColor), classicColor, "Unregistered theme color.");
        }
        if (id == ClassicDark) return ClassicOverrides.TryGetValue(classicColor, out var corrected)
            ? corrected : classicColor;
        if (id == GraphiteDark) return alternatives.Graphite;
        if (id == Light) return alternatives.Light;
        if (!AdditionalPresets.TryGetValue(id, out var preset))
        {
            throw new ArgumentOutOfRangeException(nameof(id), id, "Unknown theme.");
        }
        if (preset.Overrides.TryGetValue(classicColor, out var exact)) return exact;
        var source = preset.UsesLightBase ? alternatives.Light : alternatives.Graphite;
        var red = Convert.ToInt32(source.Substring(1, 2), 16);
        var green = Convert.ToInt32(source.Substring(3, 2), 16);
        var blue = Convert.ToInt32(source.Substring(5, 2), 16);
        if (id == InkDark)
        {
            static int Contrast(int channel) => channel < 128
                ? 3 * channel / 4
                : Math.Min(255, 5 * channel / 4);
            return $"#{Contrast(red):X2}{Contrast(green):X2}{Contrast(blue):X2}";
        }
        return $"#{Math.Clamp(red + preset.RedDelta, 0, 255):X2}" +
            $"{Math.Clamp(green + preset.GreenDelta, 0, 255):X2}" +
            $"{Math.Clamp(blue + preset.BlueDelta, 0, 255):X2}";
    }

    public static IBrush Brush(string classicColor) =>
        brushes.TryGetValue(classicColor, out var brush)
            ? brush
            : throw new ArgumentOutOfRangeException(nameof(classicColor), classicColor, "Unregistered theme color.");

    private static IReadOnlyDictionary<string, IBrush> CreateBrushes(string id)
    {
        var result = Colors.Keys.ToDictionary(
            key => key,
            key => (IBrush)new SolidColorBrush(Color.Parse(ResolveFor(id, key))),
            StringComparer.OrdinalIgnoreCase);
        foreach (var roleKey in GraphRoleColors.Keys)
        {
            result[roleKey] = new SolidColorBrush(Color.Parse(ResolveGraphRoleFor(id, roleKey)));
        }

        return result;
    }

    public static void Apply(string id)
    {
        if (!IsValid(id)) throw new ArgumentOutOfRangeException(nameof(id));
        var changed = CurrentId != id;
        if (changed) brushes = CreateBrushes(id);
        CurrentId = id;
        if (Application.Current is { } app)
        {
            app.RequestedThemeVariant = LightThemeIds.Contains(id)
                ? ThemeVariant.Light
                : ThemeVariant.Dark;
            foreach (var (classic, brush) in brushes)
            {
                if (GraphRoleColors.ContainsKey(classic))
                {
                    app.Resources[classic] = brush;
                }
                else
                {
                    app.Resources["Theme" + classic[1..].ToUpperInvariant()] = brush;
                }
            }
        }
        if (changed) Changed?.Invoke(null, EventArgs.Empty);
    }
}
