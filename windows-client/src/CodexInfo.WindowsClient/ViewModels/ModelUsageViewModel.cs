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
    internal static string? DisplayFamilyName(string modelName) =>
        modelName.Trim().ToLowerInvariant() switch
        {
            "sol" or "gpt-6.1-sol" or "gpt-6-sol" or "gpt-5.6-sol" => "SOL",
            "terra" or "gpt-5.6-terra" => "TERRA",
            "luna" or "gpt-6-luna" or "gpt-5.6-luna" => "LUNA",
            "astra" or "gpt-6-astra" => "ASTRA",
            _ => null,
        };

    public ModelUsageViewModel(ApiDetailsModelUsage usage)
        : this(usage, usage.Name, string.Equals(usage.Name, "Other", StringComparison.Ordinal))
    {
    }

    internal ModelUsageViewModel(ApiDetailsModelUsage usage, string displayName, bool isOther)
        : this(
            usage.Name,
            displayName,
            isOther,
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
        string displayName,
        bool isOther,
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
        this.displayName = displayName;
        this.isOther = isOther;
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

    private ulong inputTokens;
    private ulong cachedInputTokens;
    private ulong outputTokens;
    private ulong totalTokens;
    private ulong? cacheWriteInputTokens;
    private double? inputDollars;
    private double? cachedInputDollars;
    private double? outputDollars;
    private double cacheWriteInputDollars;
    private double? totalDollars;
    private readonly string displayName;
    private readonly bool isOther;
    private bool disposed;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string Name { get; }

    public string DisplayName => isOther ? LocalizationService.Current.Other : displayName;

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

    /// <summary>
    /// Updates one stable table row in place. Periodic snapshots normally
    /// retain the same model set, so replacing the ItemsControl children on
    /// every poll would cause a visible remove/recreate flash.
    /// </summary>
    public void Update(ApiDetailsModelUsage usage)
    {
        ArgumentNullException.ThrowIfNull(usage);
        if (!string.Equals(Name, usage.Name, StringComparison.Ordinal))
        {
            throw new ArgumentException("A model row cannot change identity.", nameof(usage));
        }

        var nextTotalDollars = usage.HasEstimatedCost
            ? usage.EstimatedTotalDollars
            : usage.TotalDollars;
        if (inputTokens == usage.InputTokens &&
            cachedInputTokens == usage.CachedInputTokens &&
            outputTokens == usage.OutputTokens &&
            totalTokens == usage.TotalTokens &&
            cacheWriteInputTokens == usage.CacheWriteInputTokens &&
            Nullable.Equals(inputDollars, usage.InputDollars) &&
            Nullable.Equals(cachedInputDollars, usage.CachedInputDollars) &&
            Nullable.Equals(outputDollars, usage.OutputDollars) &&
            cacheWriteInputDollars.Equals(usage.CacheWriteInputDollars) &&
            Nullable.Equals(totalDollars, nextTotalDollars))
        {
            return;
        }

        inputTokens = usage.InputTokens;
        cachedInputTokens = usage.CachedInputTokens;
        outputTokens = usage.OutputTokens;
        totalTokens = usage.TotalTokens;
        cacheWriteInputTokens = usage.CacheWriteInputTokens;
        inputDollars = usage.InputDollars;
        cachedInputDollars = usage.CachedInputDollars;
        outputDollars = usage.OutputDollars;
        cacheWriteInputDollars = usage.CacheWriteInputDollars;
        totalDollars = nextTotalDollars;

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
    }

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
        if (isOther)
        {
            Notify(nameof(DisplayName));
        }
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
