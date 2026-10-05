// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

namespace CodexInfo.WindowsClient.Core;

/// <summary>Canonical stable/beta product identity and numeric SemVer precedence.</summary>
public sealed class ProductVersion : IComparable<ProductVersion>
{
    private readonly string value;
    private readonly string[] basis;
    private readonly string[]? beta;

    private ProductVersion(string value, string[] basis, string[]? beta)
    {
        this.value = value;
        this.basis = basis;
        this.beta = beta;
    }

    public static ProductVersion Parse(string value) =>
        TryParse(value, out var version) ? version! : throw new FormatException("Invalid canonical product version.");

    public static bool TryParse(string? value, out ProductVersion? version)
    {
        version = null;
        if (value is null || value.Length is < 1 or > 32)
        {
            return false;
        }

        var separator = value.IndexOf("-beta.", StringComparison.Ordinal);
        var basis = (separator < 0 ? value : value[..separator]).Split('.');
        if (basis.Length != 3 || !basis.All(part => IsDecimal(part, positive: false)))
        {
            return false;
        }

        string[]? beta = null;
        if (separator >= 0)
        {
            beta = value[(separator + 6)..].Split('.');
            if (beta.Length != 2 || !beta.All(part => IsDecimal(part, positive: true)))
            {
                return false;
            }
        }

        version = new ProductVersion(value, basis, beta);
        return true;
    }

    public int CompareTo(ProductVersion? other)
    {
        if (other is null)
        {
            return 1;
        }

        for (var index = 0; index < basis.Length; index++)
        {
            var comparison = CompareDecimal(basis[index], other.basis[index]);
            if (comparison != 0)
            {
                return comparison;
            }
        }

        if (beta is null || other.beta is null)
        {
            return beta is null ? (other.beta is null ? 0 : 1) : -1;
        }

        var run = CompareDecimal(beta[0], other.beta[0]);
        return run != 0 ? run : CompareDecimal(beta[1], other.beta[1]);
    }

    public override string ToString() => value;

    private static bool IsDecimal(string value, bool positive) =>
        value.Length > 0 &&
        (value[0] != '0' || (!positive && value.Length == 1)) &&
        value.All(character => character is >= '0' and <= '9');

    // Canonical decimal strings avoid integer overflow and lexical .10 < .9.
    private static int CompareDecimal(string left, string right) =>
        left.Length != right.Length ? left.Length.CompareTo(right.Length) : string.CompareOrdinal(left, right);
}
