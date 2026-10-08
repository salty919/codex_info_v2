// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia.Platform;
using SkiaSharp;

namespace CodexInfo.WindowsClient.Controls;

/// <summary>
/// Resolves dedicated aliases for period-cost labels from packaged fonts.
/// All other font names continue through ScottPlot's existing resolvers.
/// </summary>
internal sealed class GraphPeriodCostFontResolver : ScottPlot.IFontResolver
{
    internal const string JapaneseAlias = "CodexInfo Period Cost Noto Sans JP";
    internal const string KoreanAlias = "CodexInfo Period Cost Noto Sans KR";

    private const string JapaneseAsset = "avares://CodexInfo.WindowsClient/Assets/NotoSansJP-Medium.ttf";
    private const string KoreanAsset = "avares://CodexInfo.WindowsClient/Assets/NotoSansKR.otf";
    private static readonly object RegistrationLock = new();

    internal static string AliasForLanguage(string languageCode) =>
        languageCode == "ko" ? KoreanAlias : JapaneseAlias;

    internal static void EnsureRegistered()
    {
        lock (RegistrationLock)
        {
            if (ScottPlot.Fonts.FontResolvers.Any(resolver => resolver is GraphPeriodCostFontResolver))
            {
                return;
            }

            // The unique aliases are handled before the system resolver. Do not
            // replace ScottPlot's default font or affect axes and other labels.
            ScottPlot.Fonts.FontResolvers.Insert(0, new GraphPeriodCostFontResolver());
        }
    }

    public SKTypeface? CreateTypeface(
        string fontName,
        ScottPlot.FontWeight weight,
        ScottPlot.FontSlant slant,
        ScottPlot.FontSpacing spacing) => CreateTypeface(fontName);

    public SKTypeface? CreateTypeface(string fontName, bool bold, bool italic) => CreateTypeface(fontName);

    private static SKTypeface? CreateTypeface(string fontName)
    {
        var resource = fontName switch
        {
            JapaneseAlias => JapaneseAsset,
            KoreanAlias => KoreanAsset,
            _ => null,
        };
        if (resource is null)
        {
            return null;
        }

        var loader = new StandardAssetLoader();
        loader.SetDefaultAssembly(typeof(GraphPeriodCostFontResolver).Assembly);
        var stream = loader.Open(new Uri(resource));
        return SKTypeface.FromStream(stream) ??
            throw new InvalidDataException($"The packaged period-cost font could not be loaded: {resource}");
    }
}
