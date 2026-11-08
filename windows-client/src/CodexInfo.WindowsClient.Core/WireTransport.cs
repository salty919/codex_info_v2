// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
namespace CodexInfo.WindowsClient.Core;

public interface IUiChangeClient { long ChangeRevision { get; } Task<long> WaitForChangeAsync(long after, CancellationToken cancellationToken); }
public sealed class WirePendingException(string reason) : Exception(reason) { public string Reason { get; } = reason; }
public sealed class WireHttpException(HttpStatusCode status) : HttpRequestException("HTTP response failure", null, status);
public sealed record WireRow(JsonElement Key, JsonElement Value);
public sealed record WireViewSnapshot(string Id, string Image, IReadOnlyList<WireRow> Rows);
public sealed record WirePage(JsonElement? Delivery, IReadOnlyList<WireViewSnapshot> Views);

public sealed class WireTransport : IUiChangeClient, IDisposable
{
    readonly HttpClient http; readonly Uri endpoint; readonly WireCache? cache; readonly WireCacheRecoveryException? initialFailure; readonly string cacheDirectory; WireCache Cache => cache ?? throw (initialFailure ?? new WireCacheRecoveryException("cache_invalid")); readonly object cacheGate = new(), gate = new(); readonly SemaphoreSlim slots = new(7), subscriptionGate = new(1), controlGate = new(1), identityGate = new(1), receiptGate = new(1); readonly CancellationTokenSource lifetime = new();
    readonly Dictionary<string, TaskCompletionSource<WirePage>> flights = new(StringComparer.Ordinal); readonly Dictionary<string, string> dirty = new(StringComparer.Ordinal); readonly Dictionary<string, JsonElement> targets = new(StringComparer.Ordinal); readonly ConcurrentDictionary<ulong, TaskCompletionSource<bool>> acks = new(); HashSet<string>? uiUnits;
    Channel<byte[]>? controls; Task? subscription; CancellationTokenSource? subscriptionCancellation; Exception? subscriptionFailure; TaskCompletionSource<bool>? ready; bool syncing; volatile bool subscriptionLive; long revision; TaskCompletionSource<long> signal = NewSignal(); readonly HashSet<string> resuming = new(StringComparer.Ordinal);
    static TaskCompletionSource<long> NewSignal() => new(TaskCreationOptions.RunContinuationsAsynchronously);
    public long ChangeRevision { get { lock (gate) return revision; } }
    public Task<long> WaitForChangeAsync(long after, CancellationToken cancellationToken) { lock (gate) return revision != after ? Task.FromResult(revision) : signal.Task.WaitAsync(cancellationToken); }
    internal void Wake() { TaskCompletionSource<long> previous; long value; lock (gate) { value = ++revision; previous = signal; signal = NewSignal(); } previous.TrySetResult(value); }
    public WireTransport(HttpMessageHandler handler, Uri endpoint, string cacheDirectory)
    {
        ArgumentNullException.ThrowIfNull(handler);
        if (handler is SocketsHttpHandler sockets) { sockets.UseProxy = false; sockets.AllowAutoRedirect = false; sockets.AutomaticDecompression = DecompressionMethods.None; sockets.UseCookies = false; sockets.MaxConnectionsPerServer = 1; sockets.EnableMultipleHttp2Connections = false; sockets.InitialHttp2StreamWindowSize = 65535; }
        if (handler is HttpClientHandler supplied) { supplied.UseProxy = false; supplied.AllowAutoRedirect = false; supplied.AutomaticDecompression = DecompressionMethods.None; supplied.UseCookies = false; supplied.UseDefaultCredentials = false; }
        if (!endpoint.IsLoopback || endpoint.Scheme != "http" || endpoint.AbsolutePath != "/") throw new ArgumentException("fixed loopback endpoint required"); this.endpoint = endpoint; this.cacheDirectory = cacheDirectory; try { Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(cacheDirectory))!); cache = new(cacheDirectory); } catch (WireCacheRecoveryException error) { initialFailure = error; } catch (Exception error) when (error is IOException or UnauthorizedAccessException) { initialFailure = new("cache_io", error); }
        http = new(handler, true) { Timeout = Timeout.InfiniteTimeSpan, DefaultRequestVersion = HttpVersion.Version20, DefaultVersionPolicy = HttpVersionPolicy.RequestVersionExact };
    }
    public static SocketsHttpHandler DefaultHandler()
    {
        AppContext.SetSwitch("System.Net.SocketsHttpHandler.Http2FlowControl.DisableDynamicWindowSizing", true);
        return new() { UseProxy = false, AllowAutoRedirect = false, AutomaticDecompression = DecompressionMethods.None, UseCookies = false, MaxConnectionsPerServer = 1, EnableMultipleHttp2Connections = false, InitialHttp2StreamWindowSize = 65535, MaxResponseHeadersLength = 8, ConnectTimeout = TimeSpan.FromSeconds(3), PooledConnectionLifetime = Timeout.InfiniteTimeSpan, PooledConnectionIdleTimeout = Timeout.InfiniteTimeSpan };
    }
    public async Task<WirePage> GetAsync(string target, CancellationToken cancellationToken = default)
    {
        var started = Stopwatch.GetTimestamp(); var keys = Keys(target); var state = target.StartsWith("/state?", StringComparison.Ordinal) && !target.Contains("cursor=", StringComparison.Ordinal);
        var logical = state ? keys.Select(WireReceiver.CanonicalKey).ToArray() : new[] { target };
        var tasks = new List<Task<WirePage>>(); TaskCompletionSource<WirePage>? owner = null; var owned = new List<string>(); var missing = new List<JsonElement>();
        lock (gate) { for (var i = 0; i < logical.Length; i++) { if (flights.TryGetValue(logical[i], out var existing)) { tasks.Add(existing.Task); continue; } owner ??= new(TaskCreationOptions.RunContinuationsAsynchronously); flights[logical[i]] = owner; owned.Add(logical[i]); if (state) missing.Add(keys[i]); } if (owner is not null) tasks.Add(owner.Task); }
        if (owner is not null) { var actual = state ? StateTarget(missing) : target; _ = ExecuteAsync(actual, owned, owner, started); }
        using var wait = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, lifetime.Token); wait.CancelAfter(Remaining(started)); var results = await Task.WhenAll(tasks.Distinct()).WaitAsync(wait.Token).ConfigureAwait(false);
        if (!state) return results[0]; lock (cacheGate) return new(results.Length == 1 ? results[0].Delivery : null, FindViews(keys));
    }
    static string StateTarget(IReadOnlyList<JsonElement> keys)
    {
        var order = new[] { "main", "account_choices", "account_context", "runtime" }; var parts = keys.Select(key => key[1].GetProperty("parts").GetString()!).OrderBy(part => Array.IndexOf(order, part)); var scoped = keys.FirstOrDefault(key => key[1].TryGetProperty("account", out var account) && account.ValueKind == JsonValueKind.String);
        return "/state?parts=" + string.Join(',', parts) + (scoped.ValueKind == JsonValueKind.Undefined ? "" : "&account=" + scoped[1].GetProperty("account").GetString());
    }
    static TimeSpan Remaining(long start) { var remaining = TimeSpan.FromSeconds(3) - Stopwatch.GetElapsedTime(start); return remaining > TimeSpan.Zero ? remaining : TimeSpan.Zero; }
    async Task ExecuteAsync(string target, IReadOnlyList<string> owned, TaskCompletionSource<WirePage> flight, long start)
    {
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token); deadline.CancelAfter(Remaining(start));
        try { flight.TrySetResult(await GetOwnedAsync(target, start, deadline.Token).ConfigureAwait(false)); } catch (Exception error) { WireTrace.Record("get_failed", error); flight.TrySetException(error); } finally { lock (gate) foreach (var key in owned) flights.Remove(key); }
    }
    async Task<WirePage> GetOwnedAsync(string target, long start, CancellationToken token)
    {
        if (!target.Contains("cursor=", StringComparison.Ordinal)) { string? continuation; lock (cacheGate) continuation = Cache.PendingCursor(TransferKey(target)); if (continuation is not null) target += "&cursor=" + continuation; }
        string? currentConsumer; lock (cacheGate) currentConsumer = Cache.Consumer; if (currentConsumer is not null) { await EnsureSubscriptionAsync(token).ConfigureAwait(false); lock (gate) if (syncing) throw new WirePendingException("receipt_unknown"); }
        var previous = WireIncoming.Load(cacheDirectory, target); var keys = Keys(target); bool targetsChanged = false; lock (gate) foreach (var key in keys) { var canonical = WireReceiver.CanonicalKey(key); if (previous is not null || uiUnits is not null && !uiUnits.Contains(Unit(key))) continue; if (!targets.ContainsKey(canonical)) { foreach (var old in targets.Where(pair => Unit(pair.Value) == Unit(key)).Select(pair => pair.Key).ToArray()) targets.Remove(old); targets[canonical] = key; targetsChanged = true; } }
        if (targetsChanged && subscription is not null) await ControlAsync(new JsonObject { { "type", "subscribe" }, { "targets", JsonSerializer.SerializeToNode(SubscriptionTargets()) } }, true, token).ConfigureAwait(false);
        bool cached;
        lock (cacheGate) { var views = FindViews(keys); lock (gate) cached = views.Count == keys.Count && views.All(view => !dirty.TryGetValue(view.Id, out var image) || image == view.Image); if (cached && !target.Contains("cursor=", StringComparison.Ordinal)) return new(null, views); }
        await slots.WaitAsync(token).ConfigureAwait(false);
        bool ownsIdentity = false;
        try
        {
            string? consumer; string root; lock (cacheGate) consumer = Cache.Consumer;
            if (consumer is null) { await identityGate.WaitAsync(token).ConfigureAwait(false); ownsIdentity = true; }
            lock (cacheGate) { consumer = Cache.Consumer; root = Cache.Receiver.Root; }
            if (consumer is not null) { await EnsureSubscriptionAsync(token).ConfigureAwait(false); lock (gate) if (syncing) throw new WirePendingException("receipt_unknown"); }
            using var request = Request(HttpMethod.Get, target, consumer, root, Remaining(start)); if (previous is not null) { request.Headers.Add("Codex-Info-Delivery", previous.Metadata.Delivery); request.Headers.Add("Codex-Info-Offset", previous.Offset.ToString(System.Globalization.CultureInfo.InvariantCulture)); request.Headers.Add("Codex-Info-Prefix-Sha256", previous.Prefix); }
            using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token).ConfigureAwait(false);
            if (response.StatusCode == HttpStatusCode.NoContent) { lock (cacheGate) return new(null, FindViews(keys)); }
            if (response.StatusCode != HttpStatusCode.OK) throw new WireHttpException(response.StatusCode);
            var media = response.Content.Headers.ContentType; if (media?.MediaType?.Equals("application/json", StringComparison.OrdinalIgnoreCase) != true || media.CharSet?.Trim('"').Equals("utf-8", StringComparison.OrdinalIgnoreCase) != true || response.Headers.CacheControl?.NoStore != true) throw new InvalidDataException("finite response requires JSON UTF-8 and no-store");
            if (response.Content.Headers.ContentLength is not long declared || declared < 0 || declared > 33554432) throw new InvalidDataException("finite response length missing or outside page bound");
            if (!response.Headers.TryGetValues("Codex-Info-Consumer", out var issued)) throw new InvalidDataException("receiver identity missing"); var identity = issued.Single(); if (identity.Length is 0 or > 1024 || identity.Any(ch => !char.IsAsciiLetterOrDigit(ch) && ch is not ('-' or '_' or '.' or '~'))) throw new InvalidDataException("invalid receiver identity");
            lock (cacheGate) Cache.Issue(identity);
            WireIncoming? incoming = null;
            if (response.Headers.TryGetValues("Codex-Info-Delivery", out var deliveries))
            {
                string Header(string name) => response.Headers.TryGetValues(name, out var values) ? values.Single() : throw new InvalidDataException("response resume header missing");
                var metadata = new WireIncoming.Header(target, deliveries.Single(), identity, ulong.Parse(Header("Codex-Info-Total-Bytes"), System.Globalization.CultureInfo.InvariantCulture), Header("Codex-Info-Body-Sha256")); incoming = WireIncoming.Begin(cacheDirectory, metadata); if (incoming.Offset != ulong.Parse(Header("Codex-Info-Offset"), System.Globalization.CultureInfo.InvariantCulture)) throw new InvalidDataException("response resume offset mismatch");
            }
            else if (previous is not null) throw new WireCacheRecoveryException("response_prefix_missing");
            await using var stream = await response.Content.ReadAsStreamAsync(token).ConfigureAwait(false); using var body = new MemoryStream(); var buffer = new byte[16384]; long received = 0;
            while (true) { var count = await stream.ReadAsync(buffer, token).ConfigureAwait(false); if (count == 0) break; received += count; if (received > 33_554_432) throw new InvalidDataException("response page too large"); if (incoming is not null) incoming.Append(buffer, count); else body.Write(buffer, 0, count); }
            if (response.Content.Headers.ContentLength is long expected && expected != received) throw new InvalidDataException("response length mismatch");
            using var document = JsonDocument.Parse(incoming is null ? body.ToArray() : incoming.Bytes()); var data = document.RootElement;
            switch (data.GetProperty("result").GetString())
            {
                case "pending": await EnsureSubscriptionAsync(token).ConfigureAwait(false); throw new WirePendingException(data.GetProperty("reason").GetString()!);
                case "sync_required": if (data.GetProperty("reason").GetString() == "cursor_outdated") { lock (cacheGate) Cache.ClearCursor(TransferKey(target)); Wake(); } if (data.GetProperty("reason").GetString() == "receipt_unknown") { await RestartSubscriptionAsync(token).ConfigureAwait(false); } if (data.GetProperty("reason").GetString() == "image_changed") { lock (cacheGate) Cache.DiscardPartials(data.GetProperty("view_id").GetString()!); Wake(); } throw new WirePendingException(data.GetProperty("reason").GetString()!);
                case "cache_recovery_required": throw new WireCacheRecoveryException(data.GetProperty("reason").GetString()!);
                case "data": if (incoming is null) throw new InvalidDataException("data response requires durable prefix headers"); break;
                default: throw new InvalidDataException("invalid data result");
            }
            await receiptGate.WaitAsync(token).ConfigureAwait(false); try
            {
                var viewKeys = MapKeys(data, keys); lock (cacheGate) { Cache.Commit(data, viewKeys, TransferKey(target)); incoming?.Finish(); lock (gate) foreach (var complete in data.GetProperty("completed_views").EnumerateArray()) { var id = complete.GetString()!; if (!dirty.TryGetValue(id, out var wanted) || Cache.Receiver.Views.TryGetValue(id, out var accepted) && accepted.Image == wanted) dirty.Remove(id); } }
                if (ownsIdentity) { identityGate.Release(); ownsIdentity = false; }
                Wake(); var fresh = subscription is null; await EnsureSubscriptionAsync(token).ConfigureAwait(false);
                if (!fresh) { string acceptedRoot; lock (cacheGate) acceptedRoot = Cache.Receiver.Root; await ControlAsync(new JsonObject { { "type", "receipt" }, { "delivery_id", data.GetProperty("delivery_id").GetString() }, { "complete_through", RecordCount(data) }, { "partial", null }, { "cache_root", acceptedRoot } }, true, token).ConfigureAwait(false); }
                lock (cacheGate) return new(data.Clone(), FindViews(keys));
            }
            finally { receiptGate.Release(); }
        }
        finally { if (ownsIdentity) identityGate.Release(); slots.Release(); }
    }
    HttpRequestMessage Request(HttpMethod method, string target, string? consumer, string root, TimeSpan remaining)
    {
        if (!target.StartsWith('/') || target.StartsWith("//", StringComparison.Ordinal) || target.Contains('#')) throw new ArgumentException("invalid UI target");
        var request = new HttpRequestMessage(method, new Uri(endpoint, target)) { Version = HttpVersion.Version20, VersionPolicy = HttpVersionPolicy.RequestVersionExact }; request.Headers.Add("Codex-Info-Cache-Root", root); request.Headers.Add("Codex-Info-Budget-Ms", Math.Clamp((long)remaining.TotalMilliseconds, 1, 3000).ToString(System.Globalization.CultureInfo.InvariantCulture)); if (consumer is not null) request.Headers.Add("Codex-Info-Consumer", consumer); return request;
    }
    static string TransferKey(string target) => string.Join("\n", Keys(target).Select(WireReceiver.CanonicalKey).OrderBy(key => key, StringComparer.Ordinal));
    public static IReadOnlyList<JsonElement> Keys(string target)
    {
        var parts = target.Split('?', 2); var query = new Dictionary<string, object?>(); if (parts.Length == 2) foreach (var pair in parts[1].Split('&')) { var entry = pair.Split('=', 2); if (entry.Length != 2 || query.ContainsKey(entry[0]) || entry[1].Length == 0) throw new ArgumentException("invalid UI selector"); if (entry[0] is "image" or "cursor") continue; query.Add(entry[0], entry[0] is "from" or "until" ? long.Parse(entry[1], System.Globalization.CultureInfo.InvariantCulture) : entry[1]); }
        if (parts[0] == "/state")
        {
            if (!query.TryGetValue("parts", out var projections)) throw new ArgumentException("parts required"); return projections!.ToString()!.Split(',').Select(part => { var projection = new Dictionary<string, object?> { { "parts", part } }; if (part is "main" or "account_context") projection["account"] = query.GetValueOrDefault("account"); return WireReceiver.Element(new object[] { parts[0], projection }); }).ToArray();
        }
        return [WireReceiver.Element(new object[] { parts[0], query })];
    }
    static Dictionary<string, JsonElement> MapKeys(JsonElement data, IReadOnlyList<JsonElement> keys)
    {
        var result = new Dictionary<string, JsonElement>(StringComparer.Ordinal); foreach (var change in data.GetProperty("changes").EnumerateArray())
        {
            var id = change.GetProperty("view_id").GetString()!; if (keys.Count == 1) { result[id] = keys[0]; continue; }
            var first = change.GetProperty("sets").EnumerateArray().FirstOrDefault(); if (first.ValueKind != JsonValueKind.Undefined) { var part = first.GetProperty("key")[0].GetString(); var matches = keys.Where(key => key[1].TryGetProperty("parts", out var projection) && projection.GetString() == part).ToArray(); if (matches.Length == 1) result[id] = matches[0]; }
        }
        return result;
    }
    IReadOnlyList<WireViewSnapshot> FindViews(IReadOnlyList<JsonElement> keys)
    {
        var canonical = keys.Select(WireReceiver.CanonicalKey).ToHashSet(StringComparer.Ordinal); return Cache.Receiver.ViewKeys.Where(entry => canonical.Contains(WireReceiver.CanonicalKey(entry.Value))).Where(entry => Cache.Receiver.Views.ContainsKey(entry.Key)).Select(entry => { var view = Cache.Receiver.Views[entry.Key]; return new WireViewSnapshot(entry.Key, view.Image, view.Bindings.Select(binding => new WireRow(JsonDocument.Parse(binding.Key).RootElement.Clone(), Cache.Receiver.Resolve(binding.Value))).ToArray()); }).ToArray();
    }
    static string Unit(JsonElement key) => key[0].GetString() == "/state" ? "/state/" + key[1].GetProperty("parts").GetString() : key[0].GetString()!;
    void Resolved(JsonElement value)
    {
        if (!value.TryGetProperty("views", out var views)) return;
        lock (cacheGate) lock (gate) foreach (var view in views.EnumerateArray())
        {
            var image = view.GetProperty("target_image"); var id = view.GetProperty("view_id").GetString()!; if (!Cache.Receiver.Views.TryGetValue(id, out var current) || image.ValueKind == JsonValueKind.Null || current.Image != image.GetString()) dirty[id] = image.ValueKind == JsonValueKind.Null ? "" : image.GetString()!;
        }
    }
    static int RecordCount(JsonElement data) => new[] { "definitions", "patches", "fragments", "changes" }.Sum(name => data.GetProperty(name).GetArrayLength());
    public async Task SetUiScopesAsync(bool settings, bool graph, bool threads, CancellationToken token = default)
    {
        var allowed = new HashSet<string>(StringComparer.Ordinal) { "/state/main" }; if (settings) { allowed.Add("/state/account_choices"); allowed.Add("/state/runtime"); }
        if (graph) { allowed.Add("/state/account_context"); allowed.Add("/history/periods"); allowed.Add("/history"); }
        if (threads) allowed.Add("/threads"); bool changed = false; lock (gate) { uiUnits = allowed; foreach (var key in targets.Where(pair => !allowed.Contains(Unit(pair.Value))).Select(pair => pair.Key).ToArray()) { targets.Remove(key); changed = true; } }
        if (changed && subscription is not null && subscriptionLive) await ControlAsync(new JsonObject { { "type", "subscribe" }, { "targets", JsonSerializer.SerializeToNode(SubscriptionTargets()) } }, true, token).ConfigureAwait(false);
    }
    public async Task SetTargetsAsync(IEnumerable<string> resources, CancellationToken token = default)
    {
        var next = resources.SelectMany(Keys).ToDictionary(WireReceiver.CanonicalKey, key => key, StringComparer.Ordinal); if (next.Count > 7) throw new ArgumentException("too many UI resource units"); bool changed; lock (gate) { changed = targets.Count != next.Count || next.Keys.Any(key => !targets.ContainsKey(key)); if (changed) { targets.Clear(); foreach (var (key, value) in next) targets[key] = value; } }
        if (changed && subscription is not null) await ControlAsync(new JsonObject { { "type", "subscribe" }, { "targets", JsonSerializer.SerializeToNode(SubscriptionTargets()) } }, true, token).ConfigureAwait(false);
    }
    object[] SubscriptionTargets()
    {
        lock (cacheGate) lock (gate) return targets.Values.Select(key => { var canonical = WireReceiver.CanonicalKey(key); var view = Cache.Receiver.ViewKeys.FirstOrDefault(pair => WireReceiver.CanonicalKey(pair.Value) == canonical); var query = key[1].EnumerateObject().Where(p => p.Value.ValueKind != JsonValueKind.Null).ToDictionary(p => p.Name, p => (object)p.Value.Clone()); return (object)new { view_id = view.Key, path = key[0].GetString(), query, accepted_image = view.Key is not null && Cache.Receiver.Views.TryGetValue(view.Key, out var accepted) ? accepted.Image : null }; }).ToArray();
    }
    async Task EnsureSubscriptionAsync(CancellationToken token)
    {
        await subscriptionGate.WaitAsync(token).ConfigureAwait(false); try
        {
            if (subscription is not null && subscriptionLive) { if (ready is not null) await ready.Task.WaitAsync(token).ConfigureAwait(false); return; }
            if (subscriptionFailure is WireCacheRecoveryException or InvalidDataException or WireHttpException) throw subscriptionFailure;
            subscriptionFailure = null; controls?.Writer.TryComplete(); subscriptionCancellation?.Dispose(); subscriptionCancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
            string? consumer; string root; ulong maximum; object[] partials; lock (cacheGate) { consumer = Cache.Consumer; root = Cache.Receiver.Root; maximum = Cache.Receiver.Definitions.Count == 0 ? 0 : Cache.Receiver.Definitions.Keys.Max(); partials = Cache.Receiver.PartialReceipts(); }
            if (consumer is null) return;
            controls = Channel.CreateBounded<byte[]>(new BoundedChannelOptions(8) { SingleReader = true, FullMode = BoundedChannelFullMode.Wait }); ready = new(TaskCreationOptions.RunContinuationsAsynchronously); var content = new ControlContent(controls.Reader, subscriptionCancellation.Token); content.Headers.TryAddWithoutValidation("Content-Type", "application/x-ndjson; charset=utf-8"); var request = Request(HttpMethod.Post, "/subscription", consumer, root, TimeSpan.FromSeconds(3)); request.Content = content;
            var open = new JsonObject { { "type", "open" }, { "consumer", consumer }, { "cache_root", root }, { "max_definition_id", maximum }, { "subscriptions", JsonSerializer.SerializeToNode(SubscriptionTargets()) }, { "partial_receipts", JsonSerializer.SerializeToNode(partials) }, { "incoming_deliveries", JsonSerializer.SerializeToNode(WireIncoming.Receipts(cacheDirectory)) } };
            WireTrace.Record("subscription_connect"); subscriptionLive = true; subscription = RunSubscriptionAsync(request, ready, subscriptionCancellation.Token); await ControlAsync(open, false, token).ConfigureAwait(false); await ready.Task.WaitAsync(token).ConfigureAwait(false);
        }
        finally { subscriptionGate.Release(); }
        foreach (var target in WireIncoming.Targets(cacheDirectory)) { lock (gate) if (!resuming.Add(target)) continue; _ = ResumeIncomingAsync(target); }
    }
    async Task ResumeIncomingAsync(string target)
    {
        try { while (!lifetime.IsCancellationRequested && WireIncoming.Load(cacheDirectory, target) is not null) { var before = ChangeRevision; try { await GetAsync(target, lifetime.Token).ConfigureAwait(false); return; } catch (WirePendingException) { await WaitForChangeAsync(before, lifetime.Token).ConfigureAwait(false); } } }
        catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
        catch (Exception error) { subscriptionFailure = error; Wake(); }
        finally { lock (gate) resuming.Remove(target); }
    }
    async Task RunSubscriptionAsync(HttpRequestMessage request, TaskCompletionSource<bool> established, CancellationToken token)
    {
        try
        {
            using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token).ConfigureAwait(false); if (response.StatusCode != HttpStatusCode.OK) throw new WireHttpException(response.StatusCode); await using var stream = await response.Content.ReadAsStreamAsync(token).ConfigureAwait(false); var buffer = new byte[16384]; using var record = new MemoryStream();
            while (true)
            {
                var length = await stream.ReadAsync(buffer, token).ConfigureAwait(false); if (length == 0) break; for (var i = 0; i < length; i++)
                {
                    record.WriteByte(buffer[i]); if (record.Length > 8192) throw new InvalidDataException("control record too large"); if (buffer[i] != '\n') continue; using var document = JsonDocument.Parse(record.GetBuffer().AsMemory(0, (int)record.Length)); record.SetLength(0); var value = document.RootElement;
                    switch (value.GetProperty("type").GetString())
                    {
                        case "ready": WireTrace.Record("subscription_ready", state: value.GetProperty("cache_state").GetString()); Resolved(value); lock (gate) syncing = value.GetProperty("cache_state").GetString() != "matched"; established.TrySetResult(true); Wake(); break;
                        case "ack": Resolved(value); if (acks.TryRemove(value.GetProperty("ack_seq").GetUInt64(), out var ack)) ack.TrySetResult(true); break;
                        case "changed": lock (gate) foreach (var view in value.GetProperty("views").EnumerateArray()) dirty[view.GetProperty("view_id").GetString()!] = view.GetProperty("target_image").ValueKind == JsonValueKind.Null ? "" : view.GetProperty("target_image").GetString()!; await ControlAsync(new JsonObject { { "type", "notify_ack" }, { "notify_seq", value.GetProperty("notify_seq").GetUInt64() } }, false, token).ConfigureAwait(false); Wake(); break;
                        case "probe": _ = SendManifestAsync(value.Clone(), token); break;
                        case "recovery_required": throw new WireCacheRecoveryException(value.GetProperty("reason").GetString()!);
                        default: throw new InvalidDataException("unknown subscription record");
                    }
                }
            }
            throw new IOException("subscription disconnected");
        }
        catch (Exception error) { var wasReady = established.Task.IsCompletedSuccessfully; WireTrace.Record("subscription_failed", error, wasReady ? "established" : "connecting"); established.TrySetException(error); if (ReferenceEquals(established, ready)) { subscriptionLive = false; subscriptionFailure = error; } foreach (var ack in acks.Values) ack.TrySetException(error); if (wasReady) Wake(); }
        finally { request.Dispose(); }
    }
    async Task SendManifestAsync(JsonElement probe, CancellationToken token)
    {
        try
        {
            IReadOnlyList<(JsonElement Key, byte[] Hash, ulong? Id)> manifest; lock (cacheGate) manifest = Cache.Receiver.Manifest(); var nodes = new JsonArray();
            foreach (var entry in manifest) { var node = new JsonObject { { "key", Convert.ToBase64String(WireCodec.Canonical(null, entry.Key)).TrimEnd('=').Replace('+', '-').Replace('/', '_') }, { "hash", Convert.ToHexStringLower(entry.Hash) }, { "id", entry.Id } }; nodes.Add(node); if (Encoding.UTF8.GetByteCount(nodes.ToJsonString()) > 6500) { await ControlAsync(new JsonObject { { "type", "manifest" }, { "probe", probe.GetProperty("probe").GetString() }, { "nodes", nodes }, { "next", "more" } }, true, token).ConfigureAwait(false); nodes = []; } }
            await ControlAsync(new JsonObject { { "type", "manifest" }, { "probe", probe.GetProperty("probe").GetString() }, { "nodes", nodes }, { "next", null } }, true, token).ConfigureAwait(false); lock (gate) syncing = false; WireTrace.Record("manifest_matched"); Wake();
        }
        catch (Exception error) { subscriptionFailure = error; subscriptionCancellation?.Cancel(); Wake(); }
    }
    async Task ControlAsync(JsonObject value, bool wait, CancellationToken token)
    {
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token, lifetime.Token); deadline.CancelAfter(TimeSpan.FromSeconds(3)); ulong seq; TaskCompletionSource<bool>? ack = null;
        await controlGate.WaitAsync(deadline.Token).ConfigureAwait(false); try
        {
            lock (cacheGate) seq = Cache.NextSequence(); value["seq"] = seq; var bytes = Encoding.UTF8.GetBytes(value.ToJsonString() + "\n"); if (bytes.Length > 8192) throw new InvalidDataException("control record too large");
            if (wait) { ack = new(TaskCreationOptions.RunContinuationsAsynchronously); acks[seq] = ack; }
            await (controls ?? throw new IOException("subscription unavailable")).Writer.WriteAsync(bytes, deadline.Token).ConfigureAwait(false);
        }
        finally { controlGate.Release(); }
        if (ack is not null) try { await ack.Task.WaitAsync(deadline.Token).ConfigureAwait(false); } finally { acks.TryRemove(seq, out _); }
    }

    async Task RestartSubscriptionAsync(CancellationToken token) { subscriptionCancellation?.Cancel(); controls?.Writer.TryComplete(); if (subscription is not null) await subscription.WaitAsync(token).ConfigureAwait(false); await EnsureSubscriptionAsync(token).ConfigureAwait(false); }
    public async Task CheckAsync(CancellationToken token = default) { using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token, lifetime.Token); deadline.CancelAfter(TimeSpan.FromSeconds(3)); await EnsureSubscriptionAsync(deadline.Token).ConfigureAwait(false); await ControlAsync(new JsonObject { { "type", "check" } }, true, deadline.Token).ConfigureAwait(false); }
    public void Dispose() { lifetime.Cancel(); subscriptionCancellation?.Cancel(); controls?.Writer.TryComplete(); http.Dispose(); subscriptionCancellation?.Dispose(); lifetime.Dispose(); }
    sealed class ControlContent(ChannelReader<byte[]> reader, CancellationToken lifetime) : HttpContent
    {
        protected override bool TryComputeLength(out long length) { length = 0; return false; }
        protected override Task SerializeToStreamAsync(Stream stream, TransportContext? context) => WriteAsync(stream, lifetime);
        protected override Task SerializeToStreamAsync(Stream stream, TransportContext? context, CancellationToken token) => WriteAsync(stream, token);
        async Task WriteAsync(Stream stream, CancellationToken token) { await foreach (var bytes in reader.ReadAllAsync(token)) { await stream.WriteAsync(bytes, token).ConfigureAwait(false); await stream.FlushAsync(token).ConfigureAwait(false); } }
    }
}
