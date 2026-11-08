// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Net;
using CodexInfo.WindowsClient.Core;
using Xunit;
namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class LoopbackBoundaryCoverageTests
{
    [Fact] public void DefaultHandlerHasFiniteHttp2CreditsAndNoAmbientProxyCookiesOrRedirects() { using var handler = WireTransport.DefaultHandler(); Assert.False(handler.UseProxy); Assert.False(handler.UseCookies); Assert.False(handler.AllowAutoRedirect); Assert.Equal(DecompressionMethods.None, handler.AutomaticDecompression); Assert.Equal(65535, handler.InitialHttp2StreamWindowSize); Assert.Equal(1, handler.MaxConnectionsPerServer); Assert.False(handler.EnableMultipleHttp2Connections); Assert.Equal(TimeSpan.FromSeconds(3), handler.ConnectTimeout); }
    [Fact] public void InvalidEndpointAndNullHandlerCannotCreateAReceiver() { var path = Path.Combine(Path.GetTempPath(), "codex-690-boundary-" + Guid.NewGuid().ToString("N")); try { Assert.Throws<ArgumentException>(() => new WireTransport(WireTransport.DefaultHandler(), new Uri("https://example.invalid/"), path)); Assert.False(Directory.Exists(path)); Assert.Throws<ArgumentNullException>(() => new WireTransport(null!, new Uri("http://127.0.0.1:8787/"), path)); Assert.False(Directory.Exists(path)); } finally { if (Directory.Exists(path)) Directory.Delete(path, true); File.Delete(Path.ChangeExtension(path, "identity")); } }
    [Theory][InlineData("bad/id")][InlineData("")][InlineData("bad?selector")][InlineData("bad%20id")] public async Task InvalidSelectorsFailBeforeAnyHttpRequest(string account) { using var fixture = new WireFacadeFixture(); using var client = new LoopbackStatusClient(fixture); var result = await client.FetchMainAsync(account, false); Assert.Equal(DetailsFetchFailure.Response, result.Failure); Assert.Empty(fixture.Gets); }
    [Fact] public async Task MalformedJsonIsResponseFailure() { using var fixture = new WireFacadeFixture { RawBody = "{" }; using var client = new LoopbackStatusClient(fixture); Assert.Equal(DetailsFetchFailure.Response, (await client.FetchMainAsync(null, false)).Failure); Assert.Single(fixture.Gets); }
    [Theory][InlineData(true, false)][InlineData(false, true)] public async Task DeclaredJsonAndNoStoreAreRequired(bool missingType, bool missingNoStore) { using var fixture = new WireFacadeFixture { NoContentType = missingType, NoStore = !missingNoStore }; using var client = new LoopbackStatusClient(fixture); Assert.Equal(DetailsFetchFailure.Response, (await client.FetchMainAsync(null, false)).Failure); Assert.Single(fixture.Gets); }
}
