// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Reflection;
using CodexInfo.WindowsClient.Core;

namespace CodexInfo.WindowsClient.Presentation.Tests;

/// <summary>Shared immutable identity fixture for validated details responses.</summary>
internal static class PublishedPairTestFixtures
{
    private const string CanonicalValue =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    public static PublishedPairIdentity Canonical => Create(CanonicalValue);

    public static ApiDetailsSnapshot DetailsGeneration(
        ApiState state,
        long? observedAt,
        bool authenticated,
        string? planLabel,
        ApiQuota? quota,
        IReadOnlyList<ApiDetailsModelUsage> models,
        ulong activeThreadCount) =>
        new(
            state,
            observedAt,
            authenticated,
            planLabel,
            quota,
            models,
            activeThreadCount,
            [],
            [],
            [],
            "概算 —")
        {
            PublishedPair = Canonical,
        };

    private static PublishedPairIdentity Create(string value)
    {
        var method = typeof(PublishedPairIdentity).GetMethod(
            "TryCreate",
            BindingFlags.NonPublic | BindingFlags.Static);
        if (method is null)
        {
            throw new InvalidOperationException("PublishedPairIdentity.TryCreate is unavailable.");
        }

        var arguments = new object?[] { value, null };
        if (method.Invoke(null, arguments) is not true || arguments[1] is not PublishedPairIdentity identity)
        {
            throw new InvalidOperationException("The published-pair fixture value is invalid.");
        }

        return identity;
    }
}
