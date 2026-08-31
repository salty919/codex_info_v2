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
            usage.InputDollars,
            usage.CachedInputDollars,
            usage.OutputDollars)
    {
    }

    private ModelUsageViewModel(
        string name,
        ulong inputTokens,
        ulong cachedInputTokens,
        ulong outputTokens,
        double? inputDollars,
        double? cachedInputDollars,
        double? outputDollars)
    {
        Name = name;
        this.inputTokens = inputTokens;
        this.cachedInputTokens = cachedInputTokens;
        this.outputTokens = outputTokens;
        this.inputDollars = inputDollars;
        this.cachedInputDollars = cachedInputDollars;
        this.outputDollars = outputDollars;
        LocalizationService.LanguageChanged += OnLanguageChanged;
    }

    private readonly ulong inputTokens;
    private readonly ulong cachedInputTokens;
    private readonly ulong outputTokens;
    private readonly double? inputDollars;
    private readonly double? cachedInputDollars;
    private readonly double? outputDollars;
    private bool disposed;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string Name { get; }

    public string InputTokensText => FormatTokens(inputTokens);

    public string CachedInputTokensText => FormatTokens(cachedInputTokens);

    public string OutputTokensText => FormatTokens(outputTokens);

    public string InputDollarsText => FormatDollars(inputDollars);

    public string CachedInputDollarsText => FormatDollars(cachedInputDollars);

    public string OutputDollarsText => FormatDollars(outputDollars);

    public string InputLabel => LocalizationService.Current.Input;
    public string CachedInputLabel => LocalizationService.Current.CachedInput;
    public string OutputLabel => LocalizationService.Current.Output;

    private void OnLanguageChanged(object? sender, EventArgs eventArgs)
    {
        Notify(nameof(InputTokensText));
        Notify(nameof(CachedInputTokensText));
        Notify(nameof(OutputTokensText));
        Notify(nameof(InputDollarsText));
        Notify(nameof(CachedInputDollarsText));
        Notify(nameof(OutputDollarsText));
        Notify(nameof(InputLabel));
        Notify(nameof(CachedInputLabel));
        Notify(nameof(OutputLabel));
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

    private static string FormatDollars(double? value)
    {
        return value is { } amount && double.IsFinite(amount)
            ? string.Create(CultureInfo.CurrentCulture, $"${amount:N2}")
            : LocalizationService.Current.UnavailableValue;
    }
}
