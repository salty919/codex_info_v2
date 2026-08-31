// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient;

/// <summary>
/// Opt-in, deterministic UI fixtures used by the Linux/X11 visual audit.
/// Nothing is enabled unless the caller explicitly sets
/// <c>CODEX_INFO_WINDOWS_PREVIEW</c>; normal startup always uses the fixed
/// loopback client and persisted setup state.
/// </summary>
public static class PreviewEnvironment
{
    public static string? Scenario
    {
        get
        {
            var value = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_PREVIEW")?.Trim();
            return string.IsNullOrWhiteSpace(value) ? null : value.ToLowerInvariant();
        }
    }

    public static bool Enabled => Scenario is not null;

    public static bool IsSetup => Scenario is "setup";

    public static bool IsChild(string child) => string.Equals(Scenario, child, StringComparison.OrdinalIgnoreCase);

    public static int GraphPointCount
    {
        get
        {
            var value = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_POINTS");
            return int.TryParse(value, out var count) && count is >= 2 and <= 44_640 ? count : 3;
        }
    }

    public static int GraphBuildDelayMilliseconds
    {
        get
        {
            var value = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_PREVIEW_GRAPH_BUILD_DELAY_MS");
            return int.TryParse(value, out var delay) && delay is >= 0 and <= 5_000 ? delay : 0;
        }
    }

    public static int ThreadCount => ParseThreadCount(
        Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_PREVIEW_THREAD_COUNT"));

    public static int ParseThreadCount(string? value) =>
        int.TryParse(value, out var count) && count is >= 0 and <= 7 ? count : 6;

    public static bool TryGetSize(out double width, out double height)
    {
        var value = Environment.GetEnvironmentVariable("CODEX_INFO_WINDOWS_PREVIEW_SIZE");
        return TryParseSize(value, out width, out height);
    }

    public static bool TryParseSize(string? value, out double width, out double height)
    {
        width = 0;
        height = 0;
        value = value?.Trim();
        if (string.IsNullOrWhiteSpace(value))
        {
            return false;
        }

        var separator = value.IndexOf('x');
        if (separator <= 0 || separator >= value.Length - 1 ||
            !double.TryParse(value[..separator], System.Globalization.NumberStyles.Integer, System.Globalization.CultureInfo.InvariantCulture, out width) ||
            !double.TryParse(value[(separator + 1)..], System.Globalization.NumberStyles.Integer, System.Globalization.CultureInfo.InvariantCulture, out height))
        {
            width = 0;
            height = 0;
            return false;
        }

        // Prevent an accidental fixture typo from creating an off-screen or
        // unrenderable test window.  The actual window minimums still own the
        // final layout constraint.
        if (!double.IsFinite(width) || !double.IsFinite(height) || width < 320 || height < 240)
        {
            width = 0;
            height = 0;
            return false;
        }

        return true;
    }
}

/// <summary>Stable authenticated data for graph/thread/visual fixture runs.</summary>
public sealed class PreviewLoopbackClient : ILoopbackHealthClient, ILoopbackDetailsClient, IDisposable
{
    private const string PreviewPublishedPairValue =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";
    private static readonly PublishedPairIdentity PreviewPublishedPair =
        PublishedPairIdentity.Create(PreviewPublishedPairValue);
    private readonly ApiDetailsSnapshot details;

    public PreviewLoopbackClient()
    {
        var scenario = PreviewEnvironment.Scenario;
        var previewState = scenario switch
        {
            "auth" => ApiState.AuthRequired,
            "error" => ApiState.Error,
            _ => ApiState.Ready,
        };
        var authenticated = previewState == ApiState.Ready;
        var remainingPercent = scenario switch
        {
            "normal" => 48d,
            "zero" => 0d,
            "full" => 100d,
            // Keep the warning fixture inside the same <=10% threshold used by
            // the production presentation state mapper.  A preview scenario
            // must exercise the real warning branch, not merely show a low
            // number while remaining visually "Ready".
            "warning" => 10d,
            "danger" => 2d,
            _ => 48d,
        };
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        const long windowSeconds = 7 * 86_400;
        // Keep the preview gauge on the exact half-period boundary used by
        // Run-WindowsClientE2E. This makes the fractional cell deterministic
        // instead of accidentally depending on a 4/7 fixture ratio.
        var reset = now + windowSeconds / 2;
        var graphPointCount = PreviewEnvironment.GraphPointCount;
        var start = graphPointCount == 3
            ? reset - 7 * 86_400
            : now - (graphPointCount - 1L) * 60;
        var samples = graphPointCount == 3
            ?
            [
                new ApiHistorySample(start + 86_400, reset, 92, 1.25, 0.75, 0.25, 1_200, 640, 300),
                new ApiHistorySample(start + 3 * 86_400, reset, 67.5, 4.5, 2.25, 1.25, 4_800, 2_240, 1_100),
                new ApiHistorySample(now - 900, reset, remainingPercent, 8.75, 4.5, 2.75, 8_400, 4_200, 2_100),
            ]
            : Enumerable.Range(0, graphPointCount)
                .Select(index =>
                {
                    var fraction = index / (double)(graphPointCount - 1);
                    return new ApiHistorySample(
                        start + index * 60L,
                        reset,
                        100 - (100 - remainingPercent) * fraction,
                        8.75 * fraction,
                        4.5 * fraction,
                        2.75 * fraction,
                        (ulong)(8_400 * fraction),
                        (ulong)(4_200 * fraction),
                        (ulong)(2_100 * fraction));
                })
                .ToArray();
        var period = new ApiHistoryPeriod(
            reset.ToString(System.Globalization.CultureInfo.InvariantCulture),
            start,
            reset,
            true,
            LocalizationService.Current.Latest)
        {
            Samples = samples,
        };
        // The physical acceptance path exercises current -> past -> current
        // period selection. Keep the preview details contract equally rich so
        // that UIA runs do not silently skip the historical branch.
        var pastReset = now - 4 * 3_600;
        var pastStart = pastReset - 3 * 3_600;
        var pastSamples = new[]
        {
            new ApiHistorySample(pastStart + 60, pastReset, 98, 0.10, 0.20, 0.30, 50, 100, 150),
            new ApiHistorySample(pastReset - 60, pastReset, 84, 0.60, 1.20, 1.80, 600, 1_200, 1_800),
        };
        var pastPeriod = new ApiHistoryPeriod(
            pastReset.ToString(System.Globalization.CultureInfo.InvariantCulture),
            pastStart,
            pastReset,
            false,
            $"{pastStart} — {pastReset}")
        {
            ResetAt = pastReset,
            Samples = pastSamples,
        };
        var threads = new[]
        {
            new ApiThreadDetails("preview-root", "Preview root task", null, "gpt-preview-terra", "TERRA", 12_400, 4_000, 16_000, now - 5_400, now - 300, false, 0, false),
            new ApiThreadDetails("preview-child", "Child analysis", "preview-root", "gpt-preview-luna", "LUNA", 4_800, 2_100, 16_000, now - 3_600, now - 600, true, 1, false),
            new ApiThreadDetails("preview-grandchild", "Graph parity evaluation", "preview-child", "gpt-preview-sol", "SOL", 3_900, 1_600, 16_000, now - 3_000, now - 480, true, 2, false),
            new ApiThreadDetails("preview-second-root", "Windows installer verification", null, "gpt-preview-sol", "SOL", 9_200, 3_300, 16_000, now - 4_200, now - 420, false, 0, false),
            new ApiThreadDetails("preview-second-child", "REST boundary tests", "preview-second-root", "gpt-preview-terra", "TERRA", 5_600, 2_700, 16_000, now - 2_700, now - 240, true, 1, false),
            new ApiThreadDetails("preview-orphan", "Recovered worker", "missing-parent", "gpt-preview-sol", "SOL", 1_200, null, null, now - 1_800, null, true, null, true),
            new ApiThreadDetails("preview-third-root", "Release smoke verification", null, "gpt-preview-luna", "LUNA", 2_400, 900, 16_000, now - 1_500, now - 180, false, 0, false),
        }.Take(PreviewEnvironment.ThreadCount).ToArray();

        details = new ApiDetailsSnapshot(
            previewState,
            now,
            authenticated,
            "Pro",
            new ApiQuota(remainingPercent, reset, windowSeconds, false),
            [
                new ApiDetailsModelUsage("SOL", 8_400, 1_500, 900, 8.75, 1.50, 2.25),
                new ApiDetailsModelUsage("TERRA", 4_200, 900, 500, 4.50, 0.90, 1.35),
                new ApiDetailsModelUsage("LUNA", 2_100, 600, 240, 2.75, 0.60, 0.90),
            ],
            (ulong)threads.Length,
            [period, pastPeriod],
            [.. samples, .. pastSamples],
            threads,
            "概算 $25.20")
        {
            PublishedPair = PreviewPublishedPair,
        };
    }

    public Task<HealthFetchResult> FetchHealthAsync(CancellationToken cancellationToken = default) =>
        Task.FromResult(HealthFetchResult.Success(new ApiHealthSnapshot("v1", "codex-info", ProductInfo.Version)));

    public Task<DetailsFetchResult> FetchDetailsAsync(CancellationToken cancellationToken = default) =>
        Task.FromResult(DetailsFetchResult.Success(details));

    public void Dispose()
    {
    }
}
