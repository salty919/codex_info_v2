// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Globalization;
using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;
using CodexInfo.WindowsClient.ViewModels;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class ModelUsageDisplayContractTests
{
    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const string PublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    [Fact]
    public async Task V3ParserAndPresentationMatchTheSharedCrossPlatformOracle()
    {
        var previousCulture = CultureInfo.CurrentCulture;
        var previousLanguage = LocalizationService.Current.LanguageCode;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("en-US");
            LocalizationService.SetLanguage("en");

            var fixturePath = Path.Combine(
                AppContext.BaseDirectory,
                "Fixtures",
                "model_usage_display_oracle.json");
            using var fixture = JsonDocument.Parse(await File.ReadAllBytesAsync(fixturePath));
            var root = fixture.RootElement;
            Assert.Equal("MODEL-USAGE-DISPLAY-01", root.GetProperty("contract_id").GetString());
            var rows = root.GetProperty("rows").EnumerateArray().ToArray();
            var wireModels = string.Join(
                ",",
                rows.Select(row => row.GetProperty("wire").GetRawText()));
            var currentJson =
                "{\"api_version\":\"v3\",\"state\":\"ready\",\"observed_at\":1790000000," +
                "\"authenticated\":true,\"plan_label\":\"Pro\"," +
                "\"quota\":{\"remaining_percent\":50.0,\"reset_at\":1790604800," +
                "\"window_seconds\":604800,\"monthly\":false},\"models\":[" +
                wireModels + "],\"active_thread_count\":0}";
            using var client = new LoopbackStatusClient(
                new CurrentFixtureHandler(Encoding.UTF8.GetBytes(currentJson)));

            var result = await client.FetchCurrentAsync(CancellationToken.None);

            Assert.True(result.IsSuccess);
            Assert.Equal(rows.Length, result.Snapshot!.Models.Count);
            foreach (var row in rows)
            {
                var wire = row.GetProperty("wire");
                var expected = row.GetProperty("expected");
                var name = wire.GetProperty("model").GetString();
                var model = result.Snapshot.Models.Single(candidate => candidate.Name == name);
                var rawInput = wire.GetProperty("input_tokens").GetUInt64();
                var cachedInput = wire.GetProperty("cached_input_tokens").GetUInt64();
                var cacheWrite = wire.GetProperty("cache_write_input_tokens");
                var cost = wire.GetProperty("estimated_cost");

                Assert.Equal(rawInput - cachedInput, model.InputTokens);
                Assert.Equal(cachedInput, model.CachedInputTokens);
                Assert.Equal(wire.GetProperty("output_tokens").GetUInt64(), model.OutputTokens);
                Assert.Equal(wire.GetProperty("total_tokens").GetUInt64(), model.TotalTokens);
                Assert.Equal(
                    cacheWrite.ValueKind == JsonValueKind.Null ? null : cacheWrite.GetUInt64(),
                    model.CacheWriteInputTokens);
                if (cost.ValueKind == JsonValueKind.Null)
                {
                    Assert.True(double.IsNaN(model.InputDollars));
                    Assert.True(double.IsNaN(model.TotalDollars));
                }
                else
                {
                    var visibleInputDollars =
                        cost.GetProperty("ordinary_input_dollars").GetDouble() +
                        cost.GetProperty("cache_write_input_dollars").GetDouble();
                    Assert.Equal(visibleInputDollars, model.InputDollars, precision: 12);
                    Assert.Equal(
                        cost.GetProperty("total_dollars").GetDouble(),
                        model.TotalDollars,
                        precision: 12);
                }

                using var viewModel = new ModelUsageViewModel(model);
                var expectedInputDollars = expected.GetProperty("input_dollars");
                var expectedCachedInputDollars = expected.GetProperty("cached_input_dollars");
                var expectedOutputDollars = expected.GetProperty("output_dollars");
                Assert.Equal(expected.GetProperty("input_tokens").GetString(), viewModel.InputTokensText);
                Assert.Equal(
                    expectedInputDollars.ValueKind == JsonValueKind.Null
                        ? LocalizationService.Current.UnavailableValue
                        : expectedInputDollars.GetString(),
                    viewModel.InputDollarsText);
                Assert.Equal(
                    expected.GetProperty("cached_input_tokens").GetString(),
                    viewModel.CachedInputTokensText);
                Assert.Equal(
                    expectedCachedInputDollars.ValueKind == JsonValueKind.Null
                        ? LocalizationService.Current.UnavailableValue
                        : expectedCachedInputDollars.GetString(),
                    viewModel.CachedInputDollarsText);
                Assert.Equal(expected.GetProperty("output_tokens").GetString(), viewModel.OutputTokensText);
                Assert.Equal(
                    expectedOutputDollars.ValueKind == JsonValueKind.Null
                        ? LocalizationService.Current.UnavailableValue
                        : expectedOutputDollars.GetString(),
                    viewModel.OutputDollarsText);
            }
        }
        finally
        {
            CultureInfo.CurrentCulture = previousCulture;
            LocalizationService.SetLanguage(previousLanguage);
        }
    }

    private sealed class CurrentFixtureHandler(byte[] body) : HttpMessageHandler
    {
        private readonly byte[] body = body.ToArray();

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken)
        {
            Assert.Equal(HttpMethod.Get, request.Method);
            Assert.Equal("/v3/current", request.RequestUri?.AbsolutePath);
            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new ByteArrayContent(body),
            };
            response.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json")
            {
                CharSet = "utf-8",
            };
            response.Content.Headers.ContentLength = body.LongLength;
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, PublishedPair);
            return Task.FromResult(response);
        }
    }
}
