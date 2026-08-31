// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class SetupViewModelTests
{
    [Fact]
    public void SshTargetIsTransientAndCommandIsGeneratedOnlyForSafeParts()
    {
        using var main = new MainWindowViewModel(new NeverCalledClient());
        using var setup = new SetupViewModel(main);

        Assert.False(setup.CanStartSsh);
        setup.SshHost = "192.168.1.20";

        Assert.True(setup.CanStartSsh);
        Assert.Equal("ssh -N -L 8787:127.0.0.1:8787 192.168.1.20", setup.SshCommand);

        setup.SshUser = "salty";
        Assert.Equal("ssh -N -L 8787:127.0.0.1:8787 salty@192.168.1.20", setup.SshCommand);

        setup.SshHost = "host; whoami";
        Assert.False(setup.CanStartSsh);
        Assert.Contains("user@linux-host", setup.SshCommand, StringComparison.Ordinal);
    }

    private sealed class NeverCalledClient : HealthyDetailsClientBase
    {
        protected override Task<DetailsFetchResult> FetchDetailsFixtureAsync(CancellationToken cancellationToken = default) =>
            throw new InvalidOperationException("The setup-only test must not start polling.");
    }
}
