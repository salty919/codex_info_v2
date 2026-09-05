// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Net;
using System.Net.Http.Headers;
using System.Reflection;
using System.Text;
using CodexInfo.WindowsClient.Core;
using Xunit;

namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class LoopbackBoundaryCoverageTests
{
    private const string PublishedPairHeader = "Codex-Info-Published-Pair";
    private const string CanonicalPublishedPair =
        "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";

    [Fact]
    public void NullHandlerAndDefaultConstructionHaveExplicitContracts()
    {
        Assert.Throws<ArgumentNullException>(() => new LoopbackStatusClient(null!));

        using var client = new LoopbackStatusClient();
        var field = typeof(LoopbackStatusClient).GetField(
            "_httpClient",
            BindingFlags.NonPublic | BindingFlags.Instance);
        var httpClient = Assert.IsType<HttpClient>(field!.GetValue(client));
        Assert.Equal(TimeSpan.FromSeconds(1), httpClient.Timeout);
    }

    [Fact]
    public void SuppliedHttpClientHandlerReceivesTheSameRestrictedPolicy()
    {
        using var handler = new HttpClientHandler
        {
            UseProxy = true,
            AllowAutoRedirect = true,
            AutomaticDecompression = DecompressionMethods.GZip,
            UseCookies = true,
            MaxResponseHeadersLength = 64,
        };

        using var client = new LoopbackStatusClient(handler);

        Assert.False(handler.UseProxy);
        Assert.False(handler.AllowAutoRedirect);
        Assert.Equal(DecompressionMethods.None, handler.AutomaticDecompression);
        Assert.False(handler.UseCookies);
        Assert.Equal(8, handler.MaxResponseHeadersLength);
    }

    [Fact]
    public async Task DetailsRejectsUnmatchedSamplesInsteadOfDroppingAndPreservesOrphanValidation()
    {
        var unmatched = DetailsJson().Replace(
            "\"timestamp\":253402300680",
            "\"timestamp\":253402300800",
            StringComparison.Ordinal);
        var orphan = DetailsJson().Replace(
            "\"parent_thread_id\":null",
            "\"parent_thread_id\":\"missing\"",
            StringComparison.Ordinal);

        var unmatchedResult = await FetchDetails(unmatched);
        var orphanResult = await FetchDetails(orphan);

        Assert.Equal(DetailsFetchFailure.Response, unmatchedResult.Failure);
        Assert.Null(unmatchedResult.Snapshot);
        Assert.True(orphanResult.IsSuccess);
        Assert.True(orphanResult.Snapshot!.Threads[0].IsOrphan);
    }

    [Fact]
    public async Task DetailsPreservesCanonicalSampleOrderAndTraversesParentChain()
    {
        var child = "{\"id\":\"thread-2\",\"title\":\"Child\",\"parent_thread_id\":\"thread-1\",\"model\":\"TERRA\",\"model_label\":\"TERRA\",\"total_tokens\":null,\"context_usage_tokens\":null,\"context_window_tokens\":null,\"created_at\":null,\"last_user_message_at\":null,\"is_subagent\":true,\"depth\":null}";
        var sample = "{\"timestamp\":253402300620,\"reset_at\":253402300799,\"remaining_percent\":null,\"sol_dollars\":0,\"terra_dollars\":0,\"luna_dollars\":0,\"sol_tokens\":0,\"terra_tokens\":0,\"luna_tokens\":0}";
        var originalSample = "{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":null,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}";
        var json = DetailsJson()
            .Replace("\"remaining_percent\":42.5", "\"remaining_percent\":null", StringComparison.Ordinal)
            .Replace(originalSample, sample + "," + originalSample, StringComparison.Ordinal)
            .Replace("}],\"estimated_cost_label\":", "}," + child + "],\"estimated_cost_label\":", StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Equal(2, result.Snapshot!.HistoryPeriods[0].Samples.Count);
        Assert.Equal(253402300620, result.Snapshot.HistoryPeriods[0].Samples[0].Timestamp);
        Assert.Equal("thread-2", result.Snapshot.Threads[1].Id);
        Assert.Equal((ulong?)null, result.Snapshot.Threads[1].CumulativeTokens);
    }

    [Theory]
    [InlineData("models", "null")]
    [InlineData("history_periods", "null")]
    [InlineData("history_samples", "null")]
    [InlineData("threads", "null")]
    public async Task DetailsRejectsMissingArrayShapes(string property, string value)
    {
        var result = await FetchDetails(ReplacePropertyValue(DetailsJson(), property, value));

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Theory]
    [InlineData("estimated_cost_label", "\"\"")]
    [InlineData("plan_label", "1")]
    [InlineData("active_thread_count", "1.0")]
    public async Task DetailsRejectsInvalidTopLevelValues(string property, string value)
    {
        var result = await FetchDetails(ReplacePropertyValue(DetailsJson(), property, value));

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
    }

    [Theory]
    [InlineData("models", "[{\"name\":\"UNKNOWN\",\"input_tokens\":1,\"cached_input_tokens\":2,\"output_tokens\":3,\"input_dollars\":0,\"cached_input_dollars\":0,\"output_dollars\":0}]")]
    [InlineData("models", "[{\"name\":\"SOL\",\"input_tokens\":1,\"cached_input_tokens\":2,\"output_tokens\":3,\"input_dollars\":-1,\"cached_input_dollars\":0,\"output_dollars\":0}]")]
    public async Task DetailsRejectsUnsupportedAndInvalidModelUsage(string property, string value)
    {
        var result = await FetchDetails(ReplacePropertyValue(DetailsJson(), property, value));

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
    }

    [Fact]
    public async Task DetailsAcceptsNullableThreadFields()
    {
        var json = DetailsJson()
            .Replace("\"parent_thread_id\":null", "\"parent_thread_id\":null", StringComparison.Ordinal)
            .Replace("\"total_tokens\":20", "\"total_tokens\":null", StringComparison.Ordinal)
            .Replace("\"context_usage_tokens\":10", "\"context_usage_tokens\":null", StringComparison.Ordinal)
            .Replace("\"context_window_tokens\":80", "\"context_window_tokens\":null", StringComparison.Ordinal)
            .Replace("\"created_at\":1", "\"created_at\":null", StringComparison.Ordinal)
            .Replace("\"last_user_message_at\":1", "\"last_user_message_at\":null", StringComparison.Ordinal)
            .Replace("\"depth\":0", "\"depth\":null", StringComparison.Ordinal);

        var result = await FetchDetails(json);

        Assert.True(result.IsSuccess);
        Assert.Equal("Pro", result.Snapshot!.PlanLabel);
        Assert.Equal(98.5, result.Snapshot.Quota!.RemainingPercent);
        Assert.Null(result.Snapshot.Threads[0].CumulativeTokens);
        Assert.Null(result.Snapshot.Threads[0].Depth);
    }

    [Fact]
    public async Task DetailsRejectsMalformedJsonAndInvalidThreadShape()
    {
        var malformed = await FetchDetails("{\"api_version\":");
        var invalidThread = await FetchDetails(DetailsJson().Replace("\"depth\":0", "\"depth\":1025", StringComparison.Ordinal));

        Assert.Equal(DetailsFetchFailure.Response, malformed.Failure);
        Assert.Equal(DetailsFetchFailure.Response, invalidThread.Failure);
    }

    [Theory]
    [InlineData("operation")]
    [InlineData("request")]
    [InlineData("io")]
    [InlineData("other")]
    public async Task DetailsTransportExceptionsNeverEscape(string kind)
    {
        Exception exception = kind switch
        {
            "operation" => new OperationCanceledException(),
            "request" => new HttpRequestException(),
            "io" => new IOException(),
            _ => new InvalidOperationException(),
        };
        using var client = new LoopbackStatusClient(new StubHandler(_ => throw exception));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Transport, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsBodyReadExceptionsAreTransportFailures()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ =>
        {
            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new ThrowingContent(new InvalidOperationException("read failed")),
            };
            response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
            return response;
        }));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Transport, result.Failure);
        Assert.Null(result.Snapshot);
    }

    [Fact]
    public async Task DetailsDeclaredOversizeBodyIsRejected()
    {
        using var client = new LoopbackStatusClient(new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new DeclaredLengthContent(32L * 1024 * 1024 + 1),
        }));

        var result = await client.FetchDetailsAsync(CancellationToken.None);

        Assert.Equal(DetailsFetchFailure.Response, result.Failure);
    }

    private static async Task<DetailsFetchResult> FetchDetails(string json)
    {
        using var client = new LoopbackStatusClient(new StubHandler(request =>
            request.RequestUri?.AbsolutePath is "/v3/details" or "/v2/details"
                ? new HttpResponseMessage(HttpStatusCode.NotFound)
                : JsonResponse(json, includePublishedPair: true)));
        return await client.FetchDetailsAsync(CancellationToken.None);
    }

    private static string ReplacePropertyValue(string json, string property, string value)
    {
        var marker = $"\"{property}\":";
        var start = json.IndexOf(marker, StringComparison.Ordinal);
        Assert.True(start >= 0, $"Property '{property}' was not found.");
        start += marker.Length;
        var end = FindJsonValueEnd(json, start);
        return json[..start] + value + json[end..];
    }

    private static int FindJsonValueEnd(string json, int start)
    {
        var depth = 0;
        var inString = false;
        var escaped = false;
        for (var index = start; index < json.Length; index++)
        {
            var character = json[index];
            if (inString)
            {
                if (escaped)
                {
                    escaped = false;
                }
                else if (character == '\\')
                {
                    escaped = true;
                }
                else if (character == '"')
                {
                    inString = false;
                }
                continue;
            }

            if (character == '"')
            {
                inString = true;
            }
            else if (character is '[' or '{')
            {
                depth++;
            }
            else if (character is ']' or '}')
            {
                if (depth == 0)
                {
                    return index;
                }
                depth--;
            }
            else if (character == ',' && depth == 0)
            {
                return index;
            }
        }

        return json.Length;
    }

    private static HttpResponseMessage JsonResponse(string json, bool includePublishedPair = false)
    {
        var response = new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent(json, Encoding.UTF8, "application/json"),
        };
        response.Headers.CacheControl = new CacheControlHeaderValue { NoStore = true };
        if (includePublishedPair)
        {
            response.Headers.TryAddWithoutValidation(PublishedPairHeader, CanonicalPublishedPair);
        }

        return response;
    }

    private static string DetailsJson() =>
        "{\"api_version\":\"v1\",\"state\":\"ready\",\"observed_at\":253402300740,\"authenticated\":true,\"plan_label\":\"Pro\",\"quota\":{\"remaining_percent\":98.5,\"reset_at\":253402300799,\"window_seconds\":604800,\"monthly\":false},\"models\":[{\"name\":\"SOL\",\"input_tokens\":10,\"cached_input_tokens\":2,\"output_tokens\":3,\"input_dollars\":0.5,\"cached_input_dollars\":0.25,\"output_dollars\":0.5}],\"active_thread_count\":1,\"history_periods\":[{\"id\":\"253402300799\",\"start_at\":253341820740,\"end_at\":253402300740,\"reset_at\":253402300799,\"label\":\"2026/08/01 — 2026/08/08\",\"current\":true}],\"history_samples\":[{\"timestamp\":253402300680,\"reset_at\":253402300799,\"remaining_percent\":42.5,\"sol_dollars\":1.25,\"terra_dollars\":0.0,\"luna_dollars\":0.0,\"sol_tokens\":6,\"terra_tokens\":0,\"luna_tokens\":0}],\"history_gaps\":[],\"threads\":[{\"id\":\"thread-1\",\"title\":\"Task\",\"parent_thread_id\":null,\"model\":\"SOL\",\"model_label\":\"SOL\",\"total_tokens\":20,\"context_usage_tokens\":10,\"context_window_tokens\":80,\"created_at\":1,\"last_user_message_at\":1,\"is_subagent\":false,\"depth\":0}],\"estimated_cost_label\":\"概算 $1\"}";

    private sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> responder) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken) =>
            Task.FromResult(responder(request));
    }

    private sealed class DeclaredLengthContent(long length) : HttpContent
    {
        protected override Task SerializeToStreamAsync(Stream stream, TransportContext? context) =>
            Task.CompletedTask;

        protected override bool TryComputeLength(out long contentLength)
        {
            contentLength = length;
            return true;
        }
    }

    private sealed class ThrowingContent : HttpContent
    {
        private readonly Exception _exception;

        public ThrowingContent(Exception exception)
        {
            _exception = exception;
            Headers.ContentType = new MediaTypeHeaderValue("application/json")
            {
                CharSet = "utf-8",
            };
        }

        protected override Task<Stream> CreateContentReadStreamAsync() =>
            Task.FromException<Stream>(_exception);

        protected override Task SerializeToStreamAsync(Stream stream, TransportContext? context) =>
            Task.FromException(_exception);

        protected override bool TryComputeLength(out long length)
        {
            length = 1;
            return true;
        }
    }
}
