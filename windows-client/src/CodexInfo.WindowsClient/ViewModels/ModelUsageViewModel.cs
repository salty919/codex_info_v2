// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.ViewModels;

public sealed class ModelUsageViewModel : INotifyPropertyChanged, IDisposable
{
    public ModelUsageViewModel(ApiDetailsModelUsage usage)
        : this(
            usage.Name,
            usage.InputTokens,
            usage.CachedInputTokens,
            usage.OutputTokens,
            usage.TotalTokens,
            usage.CacheWriteInputTokens,
            usage.InputDollars,
            usage.CachedInputDollars,
            usage.OutputDollars,
            usage.CacheWriteInputDollars,
            usage.HasEstimatedCost ? usage.EstimatedTotalDollars : usage.TotalDollars)
    {
    }

    private ModelUsageViewModel(
        string name,
        ulong inputTokens,
        ulong cachedInputTokens,
        ulong outputTokens,
        ulong totalTokens,
        ulong? cacheWriteInputTokens,
        double? inputDollars,
        double? cachedInputDollars,
        double? outputDollars,
        double cacheWriteInputDollars,
        double? totalDollars)
    {
        Name = name;
        this.inputTokens = inputTokens;
        this.cachedInputTokens = cachedInputTokens;
        this.outputTokens = outputTokens;
        this.totalTokens = totalTokens;
        this.cacheWriteInputTokens = cacheWriteInputTokens;
        this.inputDollars = inputDollars;
        this.cachedInputDollars = cachedInputDollars;
        this.outputDollars = outputDollars;
        this.cacheWriteInputDollars = cacheWriteInputDollars;
        this.totalDollars = totalDollars;
        LocalizationService.LanguageChanged += OnLanguageChanged;
    }

    private readonly ulong inputTokens;
    private readonly ulong cachedInputTokens;
    private readonly ulong outputTokens;
    private readonly ulong totalTokens;
    private readonly ulong? cacheWriteInputTokens;
    private readonly double? inputDollars;
    private readonly double? cachedInputDollars;
    private readonly double? outputDollars;
    private readonly double cacheWriteInputDollars;
    private readonly double? totalDollars;
    private bool disposed;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string Name { get; }

    public string InputTokensText => FormatTokens(inputTokens);

    public string CachedInputTokensText => FormatTokens(cachedInputTokens);

    public string OutputTokensText => FormatTokens(outputTokens);

    public string TotalTokensText => FormatTokens(totalTokens);

    public string CacheWriteInputTokensText => FormatTokens(cacheWriteInputTokens);

    public string InputDollarsText => FormatDollars(inputDollars);

    public string CachedInputDollarsText => FormatDollars(cachedInputDollars);

    public string OutputDollarsText => FormatDollars(outputDollars);

    public string CacheWriteInputDollarsText => FormatDollars(cacheWriteInputDollars);

    public string TotalDollarsText => FormatDollars(totalDollars);

    public string InputLabel => LocalizationService.Current.Input;
    public string CachedInputLabel => LocalizationService.Current.CachedInput;
    public string OutputLabel => LocalizationService.Current.Output;
    public string CacheWriteInputLabel => "Cache write";
    public string TotalLabel => "Total";

    private void OnLanguageChanged(object? sender, EventArgs eventArgs)
    {
        Notify(nameof(InputTokensText));
        Notify(nameof(CachedInputTokensText));
        Notify(nameof(OutputTokensText));
        Notify(nameof(TotalTokensText));
        Notify(nameof(CacheWriteInputTokensText));
        Notify(nameof(InputDollarsText));
        Notify(nameof(CachedInputDollarsText));
        Notify(nameof(OutputDollarsText));
        Notify(nameof(CacheWriteInputDollarsText));
        Notify(nameof(TotalDollarsText));
        Notify(nameof(InputLabel));
        Notify(nameof(CachedInputLabel));
        Notify(nameof(OutputLabel));
        Notify(nameof(CacheWriteInputLabel));
        Notify(nameof(TotalLabel));
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        LocalizationService.LanguageChanged -= OnLanguageChanged;
    }

    private void Notify([CallerMemberName] string? propertyName = null) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));

    private static string FormatTokens(ulong value)
    {
        return value.ToString("N0", CultureInfo.CurrentCulture);
    }

    private static string FormatTokens(ulong? value) =>
        value is { } tokens
            ? FormatTokens(tokens)
            : LocalizationService.Current.UnavailableValue;

    private static string FormatDollars(double? value)
    {
        return value is { } amount && double.IsFinite(amount)
            ? string.Create(CultureInfo.CurrentCulture, $"${amount:N2}")
            : LocalizationService.Current.UnavailableValue;
    }
}
