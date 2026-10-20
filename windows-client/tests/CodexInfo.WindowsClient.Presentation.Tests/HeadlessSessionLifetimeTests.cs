using Avalonia;
using Avalonia.Headless;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class HeadlessSessionLifetimeTests
{
    [Fact]
    public void ImmediateSynchronousDisposeDoesNotRaceStartup()
    {
        for (var iteration = 0; iteration < 10_000; iteration++)
        {
            var session = HeadlessUnitTestSession.StartNew(
                typeof(SessionApplication));
            session.Dispose();
        }
    }

    public sealed class SessionApplication : Application
    {
        public static AppBuilder BuildAvaloniaApp() =>
            AppBuilder.Configure<SessionApplication>()
                .UseHeadless(new AvaloniaHeadlessPlatformOptions());
    }
}
