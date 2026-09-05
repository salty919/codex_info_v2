// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using Avalonia;
using CodexInfo.WindowsClient.Infrastructure;
using CodexInfo.WindowsClient.Updates;

namespace CodexInfo.WindowsClient;

internal static class Program
{
    [STAThread]
    public static void Main(string[] args)
    {
        if (args.Any(argument => string.Equals(
                argument,
                "--update-only",
                StringComparison.OrdinalIgnoreCase)))
        {
            // Scheduled logon/hourly triggers must never initialize Avalonia
            // or create a Window. They use the same concrete coordinator as
            // the normal UI startup and return only the finite task contract.
            Environment.ExitCode = RunUpdateOnly();
            return;
        }

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
    }

    public static int RunUpdateOnly()
    {
        try
        {
            using var coordinator = WindowsUpdateCoordinator.CreateDefault();
            return (int)coordinator.RunUpdateOnlyAsync().GetAwaiter().GetResult();
        }
        catch
        {
            // A scheduled task must communicate failure through the finite
            // contract even when construction or disposal fails.
            return (int)UpdateOnlyExitCode.LaunchFailure;
        }
    }

    public static AppBuilder BuildAvaloniaApp()
    {
        return AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .LogToTrace();
    }
}
