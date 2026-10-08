// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Localization;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class GraphPeriodAmountFormattingTests
{
    [Theory]
    [InlineData(74.49, "$74")]
    [InlineData(74.50, "$75")]
    [InlineData(74.62, "$75")]
    [InlineData(0d, "$0")]
    [InlineData(null, "—")]
    [InlineData(-1d, "—")]
    [InlineData(double.NaN, "—")]
    public void PeriodAmountShowsOnlyRoundedWholeDollars(double? amount, string expected)
    {
        Assert.Equal(expected, LocalizationService.Current.FormatGraphPeriodCost(amount));
    }
}
