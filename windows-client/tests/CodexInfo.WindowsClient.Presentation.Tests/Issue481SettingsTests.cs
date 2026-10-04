// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Text;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Settings;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class Issue481SettingsTests
{
    [Fact]
    public async Task ExistingSettingsShowsBothRuntimeVersionsAndClearsUnavailableValues()
    {
        using var handler = new RuntimeHandler();
        using var client = new LoopbackStatusClient(handler);
        using var main = new MainWindowViewModel(client);
        var root = Directory.CreateTempSubdirectory("codex-info-runtime-settings-");
        var settings = new SettingsViewModel(new ClientSettingsStore(Path.Combine(root.FullName, "settings.json")), main);
        try
        {
            var refresh = typeof(SettingsViewModel).GetMethod("RefreshRuntimeVersionsAsync");
            Assert.NotNull(refresh);
            await (Task)refresh.Invoke(settings, null)!;
            Assert.Equal("1.2.3", typeof(SettingsViewModel).GetProperty("RestVersion")!.GetValue(settings));
            Assert.Equal("1.2.2", typeof(SettingsViewModel).GetProperty("RecorderVersion")!.GetValue(settings));
            Assert.Equal("mismatch", typeof(SettingsViewModel).GetProperty("RuntimeVersionState")!.GetValue(settings));
            handler.Unavailable = true;
            await (Task)refresh.Invoke(settings, null)!;
            Assert.Equal(settings.Texts.UnavailableValue, typeof(SettingsViewModel).GetProperty("RestVersion")!.GetValue(settings));
            Assert.Equal(settings.Texts.UnavailableValue, typeof(SettingsViewModel).GetProperty("RecorderVersion")!.GetValue(settings));
            Assert.Equal("unavailable", typeof(SettingsViewModel).GetProperty("RuntimeVersionState")!.GetValue(settings));
        }
        finally
        {
            settings.Dispose();
            root.Delete(recursive: true);
        }
    }

    private sealed class RuntimeHandler : HttpMessageHandler
    {
        public bool Unavailable { get; set; }
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            Assert.Equal("/v1/runtime", request.RequestUri!.AbsolutePath);
            var response = new HttpResponseMessage(Unavailable ? HttpStatusCode.ServiceUnavailable : HttpStatusCode.OK)
            {
                Content = new StringContent("{\"api_version\":\"v1\",\"rest_version\":\"1.2.3\",\"recorder_version\":\"1.2.2\",\"recorder_status\":\"mismatch\"}", Encoding.UTF8, "application/json"),
            };
            response.Headers.CacheControl = new System.Net.Http.Headers.CacheControlHeaderValue { NoStore = true };
            return Task.FromResult(response);
        }
    }
}
