using System.Text.Json;
using System.Net;
using CodexInfo.WindowsClient.Core;
using Xunit;
namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class LiveWireClientTests
{
    [CandidateFact]
    public async Task CandidateRestDeliversNonemptyMainAndSharedRuntimeWithoutLegacyCalls()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT") ?? throw new InvalidOperationException("explicit owned candidate fixture port required"); var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT") ?? throw new InvalidOperationException("explicit owned output directory required"); var directory = Path.Combine(parent, "dotnet-cache-" + Guid.NewGuid().ToString("N"));
        var requests = new CountingHandler { InnerHandler = WireTransport.DefaultHandler() }; using var client = new WireTransport(requests, new Uri($"http://127.0.0.1:{port}/"), directory); using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(12));
        var revision = client.ChangeRevision; var main = await client.GetAsync("/state?parts=main", deadline.Token); var value = Assert.Single(main.Views).Rows.Single(row => row.Key[0].GetString() == "main").Value;
        while (value.GetProperty("acquisition_state").GetString() == "initializing")
        {
            await client.WaitForChangeAsync(revision, deadline.Token); revision = client.ChangeRevision; main = await client.GetAsync("/state?parts=main", deadline.Token); value = Assert.Single(main.Views).Rows.Single(row => row.Key[0].GetString() == "main").Value;
        }
        Assert.Equal("ready", value.GetProperty("acquisition_state").GetString()); Assert.True(value.GetProperty("authenticated").GetBoolean()); Assert.Equal(50.0, value.GetProperty("quota").GetProperty("remaining_percent").GetDouble()); Assert.NotEmpty(value.GetProperty("models").EnumerateArray()); Assert.Equal(1ul, value.GetProperty("thread_summary").GetProperty("open_count").GetUInt64()); Assert.False(value.TryGetProperty("threads_image", out _));
        foreach (var (model, total) in new[] { ("SOL", 11ul), ("TERRA", 22ul), ("LUNA", 33ul) }) { var saved = value.GetProperty("models").EnumerateArray().Single(row => row.GetProperty("model").GetString() == model); Assert.Equal(total, saved.GetProperty("total_tokens").GetUInt64()); Assert.Equal(total, saved.GetProperty("input_tokens").GetUInt64()); }
        var before = requests.GetCount; var same = await client.GetAsync("/state?parts=main", deadline.Token); Assert.Null(same.Delivery); Assert.Equal(before, requests.GetCount); Assert.Equal(main.Views[0].Image, same.Views[0].Image);
        var runtime = await client.GetAsync("/state?parts=runtime", deadline.Token); var version = Assert.Single(runtime.Views).Rows.Single(row => row.Key[0].GetString() == "runtime").Value; Assert.Equal("1.0.173", version.GetProperty("rest_product_version").GetString()); Assert.Equal("unavailable", version.GetProperty("recorder_status").GetString());
        Assert.DoesNotContain(runtime.Delivery!.Value.GetProperty("definitions").EnumerateArray(), definition => definition.GetProperty("kind").GetString() == "main");
        var periods = await GetReady(client, "/history/periods?account=account-1&purpose=selectable", deadline.Token); var period = Assert.Single(periods.Views).Rows.First(row => row.Key[0].GetString() == "period").Value.GetProperty("id").GetString(); var history = await GetReady(client, "/history?account=account-1&period=" + period, deadline.Token); var sample = Assert.Single(history.Views).Rows.First(row => row.Key[0].GetString() == "sample").Value;
        foreach (var (model, usd) in new[] { ("SOL", 1.0), ("TERRA", 2.0), ("LUNA", 3.0) }) Assert.Equal(BitConverter.DoubleToInt64Bits(usd), BitConverter.DoubleToInt64Bits(sample.GetProperty("models").EnumerateArray().Single(row => row.GetProperty("model").GetString() == model).GetProperty("total_dollars").GetDouble()));
        File.WriteAllText(Path.Combine(parent, "dotnet-live-accepted.json"), JsonSerializer.Serialize(new { main_image = main.Views[0].Image, quota = 50.0, threads = 1, models = value.GetProperty("models").GetArrayLength(), runtime = version, cache = directory }));
    }
    [CandidateFact]
    public async Task FreshParallelProjectionsShareOneIdentityAndEachReturnsWithinOneCallDeadline()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT") ?? throw new InvalidOperationException("fixture port required"); var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT") ?? throw new InvalidOperationException("output required"); var directory = Path.Combine(parent, "parallel-cache-" + Guid.NewGuid().ToString("N")); using var client = new WireTransport(WireTransport.DefaultHandler(), new Uri($"http://127.0.0.1:{port}/"), directory); var watch = System.Diagnostics.Stopwatch.StartNew(); var pages = await Task.WhenAll(client.GetAsync("/state?parts=main"), client.GetAsync("/state?parts=runtime")); Assert.True(watch.Elapsed < TimeSpan.FromSeconds(3)); Assert.Equal("ready", Assert.Single(pages[0].Views).Rows.Single().Value.GetProperty("acquisition_state").GetString()); Assert.Equal("1.0.173", Assert.Single(pages[1].Views).Rows.Single().Value.GetProperty("rest_product_version").GetString()); using var profile = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(directory, "profile"))); Assert.False(string.IsNullOrEmpty(profile.RootElement.GetProperty("consumer").GetString()));
    }
    [CandidateFact]
    public async Task InterruptedJsonPrefixIsDurableAndNextSocketReadStartsAfterReceivedBytes()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT") ?? throw new InvalidOperationException("fixture port required"); var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT") ?? throw new InvalidOperationException("output required"); var directory = Path.Combine(parent, "cut-cache-" + Guid.NewGuid().ToString("N")); var handler = new CutHandler { InnerHandler = WireTransport.DefaultHandler() };
        using (var first = new WireTransport(handler, new Uri($"http://127.0.0.1:{port}/"), directory)) { await Assert.ThrowsAsync<IOException>(() => first.GetAsync("/state?parts=main")); }
        var raw = Assert.Single(Directory.GetFiles(directory, "incoming-*"), path => !path.EndsWith(".json", StringComparison.Ordinal)); Assert.Equal(257, new FileInfo(raw).Length); var prefix = File.ReadAllBytes(raw); Assert.Contains("definitions", System.Text.Encoding.UTF8.GetString(prefix));
        var resumed = new ResumeHandler { InnerHandler = WireTransport.DefaultHandler() }; using var second = new WireTransport(resumed, new Uri($"http://127.0.0.1:{port}/"), directory); var page = await second.GetAsync("/state?parts=main"); var main = Assert.Single(page.Views).Rows.Single().Value; Assert.Equal("ready", main.GetProperty("acquisition_state").GetString()); Assert.Equal(3, main.GetProperty("models").GetArrayLength()); Assert.Equal(257ul, resumed.RequestOffset); Assert.Equal(257ul, resumed.ResponseOffset); Assert.Equal(handler.FullLength - 257, resumed.SuffixLength); Assert.Empty(Directory.GetFiles(directory, "incoming-*"));
    }
    [PagedCandidateFact]
    public async Task UiFacadeCommitsAllPagesBeforePublishingThreadsAndHistory()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT")!; var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT")!; var directory = Path.Combine(parent, "paged-ui-cache-" + Guid.NewGuid().ToString("N")); using var client = new LoopbackStatusClient(WireTransport.DefaultHandler(), new Uri($"http://127.0.0.1:{port}/"), directory); client.SetViewVisible("threads", true); client.SetViewVisible("graph", true); using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        var revision = client.ChangeRevision; var threads = await client.FetchThreadsAsync("account-1", deadline.Token); while (threads.Failure == DetailsFetchFailure.Pending) { await client.WaitForChangeAsync(revision, deadline.Token); revision = client.ChangeRevision; threads = await client.FetchThreadsAsync("account-1", deadline.Token); }
        Assert.True(threads.IsSuccess); Assert.Equal(257, threads.Snapshot!.Threads.Count); Assert.Contains(threads.Snapshot.Threads, row => row.Title == "API 690 paged thread 0256" && row.CumulativeTokens == 267);
        revision = client.ChangeRevision; var periods = await client.FetchSelectableHistoryPeriodsAsync("account-1", deadline.Token); while (periods.Failure == DetailsFetchFailure.Pending) { await client.WaitForChangeAsync(revision, deadline.Token); revision = client.ChangeRevision; periods = await client.FetchSelectableHistoryPeriodsAsync("account-1", deadline.Token); }
        Assert.True(periods.IsSuccess); var period = Assert.Single(periods.Snapshot!.Periods); revision = client.ChangeRevision; var history = await client.FetchHistoryPageAsync("account-1", period.Id, null, deadline.Token); while (history.Failure == DetailsFetchFailure.Pending) { await client.WaitForChangeAsync(revision, deadline.Token); revision = client.ChangeRevision; history = await client.FetchHistoryPageAsync("account-1", period.Id, null, deadline.Token); }
        Assert.True(history.IsSuccess); Assert.Equal(1025, history.Page!.Samples.Count); var latest = history.Page.Samples.OrderBy(sample => sample.Timestamp).Last(); foreach (var (name, total, usd) in new[] { ("SOL", 11ul, 1.0), ("TERRA", 22ul, 2.0), ("LUNA", 33ul, 3.0) }) { var model = latest.Models.Single(model => model.Name == name); Assert.Equal(total, model.TotalTokens); Assert.Equal(BitConverter.DoubleToInt64Bits(usd), BitConverter.DoubleToInt64Bits(model.Dollars!.Value)); }
    }
    [PagedCandidateFact]
    public async Task InterruptedClosedViewFinishesOnlyIssuedSuffixBeforeSharedMainDependencies()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT")!; var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT")!; var directory = Path.Combine(parent, "closed-prefix-cache-" + Guid.NewGuid().ToString("N")); var cut = new CutHandler { InnerHandler = WireTransport.DefaultHandler() };
        using (var first = new WireTransport(cut, new Uri($"http://127.0.0.1:{port}/"), directory)) { await first.SetUiScopesAsync(false, false, true); await Assert.ThrowsAsync<IOException>(() => first.GetAsync("/threads?account=account-1")); }
        var counted = new ClosedResumeHandler { InnerHandler = WireTransport.DefaultHandler() }; using var second = new WireTransport(counted, new Uri($"http://127.0.0.1:{port}/"), directory); await second.SetUiScopesAsync(false, false, false); using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(12)); var page = await GetReady(second, "/state?parts=main", deadline.Token); Assert.Equal("ready", Assert.Single(page.Views).Rows.Single().Value.GetProperty("acquisition_state").GetString()); Assert.Equal(257ul, counted.Offset); Assert.Equal(1, counted.ThreadRequests); Assert.Empty(Directory.GetFiles(directory, "incoming-*"));
    }
    sealed class ClosedResumeHandler : DelegatingHandler
    {
        public ulong Offset { get; private set; }
        public int ThreadRequests { get; private set; }
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) { if (request.Method == HttpMethod.Get && request.RequestUri!.AbsolutePath == "/threads") { ThreadRequests++; Offset = ulong.Parse(request.Headers.GetValues("Codex-Info-Offset").Single()); } return await base.SendAsync(request, token); }
    }
    [FragmentCandidateFact]
    public async Task FragmentedModelSummaryIsCommittedOnceAndSharedByUiFacade()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT")!; var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT")!; var directory = Path.Combine(parent, "fragment-ui-cache-" + Guid.NewGuid().ToString("N"));
        using (var wire = new WireTransport(WireTransport.DefaultHandler(), new Uri($"http://127.0.0.1:{port}/"), directory))
        {
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(3)); var page = await wire.GetAsync("/state?parts=main", deadline.Token); var data = page.Delivery!.Value; var fragments = data.GetProperty("fragments").EnumerateArray().ToArray(); Assert.True(fragments.Length >= 2); foreach (var fragment in fragments) { Assert.InRange(WireReceiver.Base64(fragment.GetProperty("data").GetString()!).Length, 1, 8192); Assert.InRange(System.Text.Encoding.UTF8.GetByteCount(fragment.GetRawText()), 1, 16384); }
            Assert.Null(data.GetProperty("next_cursor").GetString()); var main = Assert.Single(page.Views).Rows.Single().Value; Assert.Equal(300ul, main.GetProperty("thread_summary").GetProperty("open_count").GetUInt64()); Assert.Equal(300, main.GetProperty("thread_summary").GetProperty("by_model").GetArrayLength()); Assert.Equal(3, main.GetProperty("models").GetArrayLength());
        }
        using var client = new LoopbackStatusClient(WireTransport.DefaultHandler(), new Uri($"http://127.0.0.1:{port}/"), directory); using var read = new CancellationTokenSource(TimeSpan.FromSeconds(3)); var result = await client.FetchMainAsync(null, false, read.Token); Assert.Null(result.Failure); Assert.Equal(300ul, result.Current!.ThreadSummary!.OpenCount); Assert.Equal(300, result.Current.ThreadSummary.ByModel.Count); Assert.Equal(66ul, result.Current.Models.Aggregate(0ul, (sum, model) => sum + model.TotalTokens));
    }
    [RestartCandidateFact]
    public async Task InterruptedResponseSurvivesServerProcessRestartUsingSameOutboxAndReceiver()
    {
        var port = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT")!; var parent = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_OUTPUT")!; var marker = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_RESTART_FILE") ?? throw new InvalidOperationException("owned restart marker required"); var mode = Environment.GetEnvironmentVariable("CODEX_INFO_API_690_RESTART_MODE");
        if (mode == "seed")
        {
            var directory = Path.Combine(parent, "restart-cache-" + Guid.NewGuid().ToString("N")); var handler = new CutHandler { InnerHandler = WireTransport.DefaultHandler() }; using (var first = new WireTransport(handler, new Uri($"http://127.0.0.1:{port}/"), directory)) { await Assert.ThrowsAsync<IOException>(() => first.GetAsync("/state?parts=main")); }
            var raw = Assert.Single(Directory.GetFiles(directory, "incoming-*"), path => !path.EndsWith(".json", StringComparison.Ordinal)); Assert.Equal(257, new FileInfo(raw).Length); using var output = new FileStream(marker, FileMode.CreateNew, FileAccess.Write); JsonSerializer.Serialize(output, new { directory, length = handler.FullLength }); output.Flush(true); return;
        }
        Assert.Equal("resume", mode); using var previous = JsonDocument.Parse(File.ReadAllBytes(marker)); var cache = previous.RootElement.GetProperty("directory").GetString()!; var length = previous.RootElement.GetProperty("length").GetInt64(); var resumed = new ResumeHandler { InnerHandler = WireTransport.DefaultHandler() }; using var second = new WireTransport(resumed, new Uri($"http://127.0.0.1:{port}/"), cache); var page = await second.GetAsync("/state?parts=main"); Assert.Equal("ready", Assert.Single(page.Views).Rows.Single().Value.GetProperty("acquisition_state").GetString()); Assert.Equal(257ul, resumed.RequestOffset); Assert.Equal(257ul, resumed.ResponseOffset); Assert.Equal(length - 257, resumed.SuffixLength); Assert.Empty(Directory.GetFiles(cache, "incoming-*"));
    }
    sealed class ResumeHandler : DelegatingHandler
    {
        public ulong RequestOffset { get; private set; }
        public ulong ResponseOffset { get; private set; }
        public long SuffixLength { get; private set; }
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) { var response = await base.SendAsync(request, token); if (request.Method == HttpMethod.Get) { RequestOffset = ulong.Parse(request.Headers.GetValues("Codex-Info-Offset").Single()); ResponseOffset = ulong.Parse(response.Headers.GetValues("Codex-Info-Offset").Single()); SuffixLength = response.Content.Headers.ContentLength!.Value; } return response; }
    }
    sealed class CutHandler : DelegatingHandler
    {
        public long FullLength { get; private set; }
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) { var response = await base.SendAsync(request, token); if (request.Method == HttpMethod.Get) { FullLength = response.Content.Headers.ContentLength!.Value; Assert.True(response.Headers.Contains("Codex-Info-Delivery")); response.Content = new CutContent(response.Content); } return response; }
    }
    sealed class CutContent : HttpContent
    {
        readonly HttpContent inner; public CutContent(HttpContent inner) { this.inner = inner; foreach (var header in inner.Headers) Headers.TryAddWithoutValidation(header.Key, header.Value); }
        protected override bool TryComputeLength(out long length) { length = inner.Headers.ContentLength ?? 0; return inner.Headers.ContentLength is not null; }
        protected override async Task<Stream> CreateContentReadStreamAsync() => new CutStream(await inner.ReadAsStreamAsync());
        protected override async Task<Stream> CreateContentReadStreamAsync(CancellationToken token) => new CutStream(await inner.ReadAsStreamAsync(token));
        protected override async Task SerializeToStreamAsync(Stream stream, TransportContext? context) { using var cut = new CutStream(await inner.ReadAsStreamAsync()); await cut.CopyToAsync(stream); }
        protected override void Dispose(bool disposing) { if (disposing) inner.Dispose(); base.Dispose(disposing); }
    }
    sealed class CutStream(Stream inner) : Stream
    {
        int received; public override bool CanRead => true; public override bool CanSeek => false; public override bool CanWrite => false; public override long Length => throw new NotSupportedException(); public override long Position { get => received; set => throw new NotSupportedException(); }
        public override int Read(byte[] bytes, int offset, int count) => ReadAsync(bytes.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();
        public override async ValueTask<int> ReadAsync(Memory<byte> bytes, CancellationToken token = default) { if (received == 257) throw new IOException("owned fixture transport cut after 257 received bytes"); var count = await inner.ReadAsync(bytes[..Math.Min(bytes.Length, 257 - received)], token); received += count; return count; }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException(); public override void SetLength(long length) => throw new NotSupportedException(); public override void Write(byte[] bytes, int offset, int count) => throw new NotSupportedException(); protected override void Dispose(bool disposing) { if (disposing) inner.Dispose(); base.Dispose(disposing); }
    }
    static async Task<WirePage> GetReady(WireTransport client, string target, CancellationToken token) { var current = target; var seen = new HashSet<string>(StringComparer.Ordinal); while (true) { var revision = client.ChangeRevision; try { var page = await client.GetAsync(current, token); if (page.Delivery is JsonElement data && data.GetProperty("next_cursor").GetString() is string cursor) { if (!seen.Add(cursor)) throw new InvalidDataException("repeated cursor"); current = target + "&cursor=" + cursor; continue; } return page; } catch (WirePendingException) { await client.WaitForChangeAsync(revision, token); } } }
    sealed class CountingHandler : DelegatingHandler { int count; public int GetCount => Volatile.Read(ref count); protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) { if (request.Method == HttpMethod.Get) Interlocked.Increment(ref count); return base.SendAsync(request, token); } }
}

public sealed class CandidateFactAttribute : FactAttribute { public CandidateFactAttribute() { if (Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PORT") is null) Skip = "Requires explicit owned candidate REST fixture"; } }

public sealed class RestartCandidateFactAttribute : FactAttribute { public RestartCandidateFactAttribute() { if (Environment.GetEnvironmentVariable("CODEX_INFO_API_690_RESTART_MODE") is null) Skip = "Requires explicit seed or resume phase and owned server restart"; } }

public sealed class PagedCandidateFactAttribute : FactAttribute { public PagedCandidateFactAttribute() { if (Environment.GetEnvironmentVariable("CODEX_INFO_API_690_PAGED") != "1") Skip = "Requires explicit 257-thread 1025-sample fixture"; } }
public sealed class FragmentCandidateFactAttribute : FactAttribute { public FragmentCandidateFactAttribute() { if (Environment.GetEnvironmentVariable("CODEX_INFO_API_690_FRAGMENT") != "1") Skip = "Requires explicit 300-model thread summary fixture"; } }
