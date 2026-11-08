// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Collections.Concurrent;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using CodexInfo.WindowsClient.Core;
using Microsoft.AspNetCore.Hosting.Server;
using Microsoft.AspNetCore.Hosting.Server.Features;
using Microsoft.AspNetCore.Server.Kestrel.Core;

var arguments = Enumerable.Range(0, args.Length / 2).ToDictionary(i => args[i * 2], i => args[i * 2 + 1]);
if (arguments.TryGetValue("--probe", out var endpoint))
{
    using var client = new WireTransport(WireTransport.DefaultHandler(), new Uri(endpoint), arguments["--cache"]);
    using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(12));
    var target = arguments["--target"]; var rows = new List<object>(); var pages = new List<JsonElement>();
    WirePage page;
    while (true)
    {
        var revision = client.ChangeRevision;
        try { page = await client.GetAsync(target, deadline.Token); }
        catch (WirePendingException) { await client.WaitForChangeAsync(revision, deadline.Token); continue; }
        catch (WireHttpException error) when (arguments.TryGetValue("--expect-http-status", out var expected) && (int)error.StatusCode.GetValueOrDefault() == int.Parse(expected))
        { Console.WriteLine(JsonSerializer.Serialize(new { http_status = (int)error.StatusCode.GetValueOrDefault() })); return; }
        if (page.Delivery is JsonElement data) pages.Add(data.Clone());
        if (page.Delivery is JsonElement delivery && delivery.GetProperty("next_cursor").ValueKind == JsonValueKind.String)
        {
            target = arguments["--target"] + "&cursor=" + delivery.GetProperty("next_cursor").GetString();
            continue;
        }
        if (page.Views.Count != WireTransport.Keys(arguments["--target"]).Count)
        { await client.WaitForChangeAsync(revision, deadline.Token); continue; }
        break;
    }
    Console.WriteLine(JsonSerializer.Serialize(new { views = page.Views.Select(v => new { id = v.Id, image = v.Image, rows = v.Rows.Select(row => new { key = row.Key, value = row.Value }) }), deliveries = pages }));
    return;
}

var sourcePath = Path.GetFullPath(arguments["--source"]);
var controlPath = Path.GetFullPath(arguments["--control"]);
var statePath = Path.GetFullPath(arguments["--state"]);
var builder = WebApplication.CreateSlimBuilder();
builder.Logging.ClearProviders();
builder.WebHost.ConfigureKestrel(options =>
{
    options.Listen(IPAddress.Loopback, int.Parse(arguments.GetValueOrDefault("--port", "0")), listen => listen.Protocols = HttpProtocols.Http2);
    options.Limits.Http2.MaxStreamsPerConnection = 8;
    options.Limits.Http2.InitialStreamWindowSize = 65535;
    options.Limits.MaxRequestBodySize = null; // A bounded-record subscription has an open lifetime.
});
var app = builder.Build();
using var fixture = new Fixture(sourcePath, controlPath, statePath);
app.Run(fixture.HandleAsync);
await app.StartAsync();
var address = app.Services.GetRequiredService<IServer>().Features.Get<IServerAddressesFeature>()!.Addresses.Single();
fixture.Ready(new Uri(address).Port);
await app.WaitForShutdownAsync();

sealed record Row(JsonElement Key, ulong Id);
sealed record View(JsonElement Key, string Id, string Image, IReadOnlyList<Row> Rows);
sealed record Offered(JsonElement Data, IReadOnlyDictionary<string, JsonElement> Keys);
sealed record Body(string Target, byte[] Bytes);
sealed class Peer
{
    public readonly object Gate = new();
    public WireReceiver Receiver = new();
    public readonly List<Offered> Accepted = [];
    public readonly List<Offered> Pending = [];
    public JsonElement[] Targets = [];
    public Channel<object>? Output;
    public ulong NotifySequence;
    public ulong LastSequence;
    public readonly Dictionary<string, string> Notified = new(StringComparer.Ordinal);
    public readonly Dictionary<string, Body> Bodies = new(StringComparer.Ordinal);
    public readonly HashSet<string> Blocked = new(StringComparer.Ordinal);
}

// Test fixture data is immutable between explicit file IPC changes. There is
// no timer, administrative HTTP route, old schema adapter or snapshot poll.
sealed class Fixture : IDisposable
{
    readonly object sourceGate = new();
    readonly string sourcePath, controlPath, statePath;
    readonly FileStream statusOutput;
    readonly Catalog catalog = new();
    JsonObject source = new();
    bool unavailable, authenticationRequired, partialModels, emptyThreads, recorderAvailable;
    readonly ConcurrentDictionary<string, Peer> peers = new(StringComparer.Ordinal);
    readonly FileSystemWatcher watcher;
    readonly Channel<bool> updates = Channel.CreateBounded<bool>(new BoundedChannelOptions(1) { FullMode = BoundedChannelFullMode.DropOldest });
    readonly CancellationTokenSource lifetime = new();
    readonly Task changes;
    long requests, partialRequests, oldRequests, appliedRevision;
    int port;
    public Fixture(string sourcePath, string controlPath, string statePath)
    {
        this.sourcePath = sourcePath; this.controlPath = controlPath; this.statePath = statePath;
        statusOutput = new FileStream(statePath, FileMode.CreateNew, FileAccess.Write, FileShare.ReadWrite | FileShare.Delete);
        Reload();
        watcher = new(Path.GetDirectoryName(controlPath)!) { NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName, EnableRaisingEvents = true };
        watcher.Changed += OnChange; watcher.Created += OnChange; watcher.Renamed += OnChange;
        changes = Task.Run(async () =>
        {
            try { await foreach (var _ in updates.Reader.ReadAllAsync(lifetime.Token)) { Reload(); foreach (var peer in peers.Values) Notify(peer); } }
            catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { }
            catch (Exception error) { Console.Error.WriteLine(JsonSerializer.Serialize(new { stage = "fixture_source_update", operation = error.Data["fixture_operation"], exception = error.GetType().FullName, hresult = error.HResult })); }
        });
    }
    void OnChange(object sender, FileSystemEventArgs args)
    {
        var name = Path.GetFileName(args.FullPath);
        if (name.StartsWith(Path.GetFileNameWithoutExtension(controlPath) + "-", StringComparison.Ordinal) && name.EndsWith(".json", StringComparison.Ordinal))
            updates.Writer.TryWrite(true);
    }
    void Reload()
    {
        var directory = Path.GetDirectoryName(controlPath)!;
        var published = Directory.EnumerateFiles(directory, Path.GetFileNameWithoutExtension(controlPath) + "-*.json")
            .OrderByDescending(path => Path.GetFileName(path), StringComparer.Ordinal).First();
        using var flags = JsonDocument.Parse(ReadInput(published, "read_control")); var value = flags.RootElement;
        if (value.GetProperty("revision").GetInt64() <= Interlocked.Read(ref appliedRevision)) return;
        var sourceFile = value.GetProperty("source_file").GetString()!;
        if (sourceFile != Path.GetFileName(sourceFile) || !sourceFile.StartsWith("fixture-source", StringComparison.Ordinal) || !sourceFile.EndsWith(".json", StringComparison.Ordinal)) throw new InvalidDataException("invalid fixture source reference");
        var next = JsonNode.Parse(ReadInput(Path.Combine(directory, sourceFile), "read_source"))!.AsObject();
        lock (sourceGate)
        {
            source = next;
            unavailable = value.GetProperty("current_unavailable").GetBoolean();
            authenticationRequired = value.GetProperty("authentication_required").GetBoolean();
            partialModels = value.GetProperty("partial_models").GetBoolean();
            emptyThreads = value.GetProperty("empty_threads").GetBoolean();
            recorderAvailable = value.GetProperty("recorder_available").GetBoolean();
            appliedRevision = value.GetProperty("revision").GetInt64();
        }
        try { Stats(); } catch (Exception error) { error.Data["fixture_operation"] = "write_status"; throw; }
    }
    static byte[] ReadInput(string path, string operation)
    {
        try { return ReadShared(path); }
        catch (Exception error) { error.Data["fixture_operation"] = operation; throw; }
    }
    static byte[] ReadShared(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        using var bytes = new MemoryStream(); stream.CopyTo(bytes); return bytes.ToArray();
    }
    public void Ready(int boundPort) { port = boundPort; Stats(); }
    void Stats()
    {
        lock (statePath)
        {
            var bytes = JsonSerializer.SerializeToUtf8Bytes(new { port, applied_revision = Interlocked.Read(ref appliedRevision), requests = Interlocked.Read(ref requests), partial_main_requests = Interlocked.Read(ref partialRequests), old_route_requests = Interlocked.Read(ref oldRequests) });
            statusOutput.Write(bytes); statusOutput.WriteByte(10); statusOutput.Flush(true);
        }
    }
    static JsonElement E(object? value) => JsonSerializer.SerializeToElement(value);
    static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));
    static string Id(JsonElement key) => Hash(WireCodec.Canonical(null, key));
    static string Target(JsonElement descriptor)
    {
        var query = descriptor.GetProperty("query").EnumerateObject().Select(p => p.Name + "=" + p.Value.ToString());
        return descriptor.GetProperty("path").GetString() + "?" + string.Join('&', query);
    }
    View Resolve(JsonElement key)
    {
        lock (sourceGate)
        {
            var path = key[0].GetString(); var query = key[1]; var values = new List<(object[] Key, string Kind, JsonNode? Value)>();
            if (path == "/state")
            {
                var part = query.GetProperty("parts").GetString();
                switch (part)
                {
                    case "main":
                        var main = source["main"]!.DeepClone().AsObject();
                        main["models_status"] = partialModels ? "partial" : "complete";
                        if (authenticationRequired) { main["acquisition_state"] = "auth_required"; main["authenticated"] = false; main["observed_at"] = null; main["plan_label"] = null; main["quota"] = null; main["models"] = new JsonArray(); main["models_status"] = "unavailable"; main["thread_summary"] = null; }
                        else if (emptyThreads) main["thread_summary"] = JsonSerializer.SerializeToNode(new { open_count = 0, running_count = 0, unknown_activity_count = 0, by_model = Array.Empty<object>() });
                        values.Add((["main"], "main", main)); break;
                    case "account_choices": values.Add((["account_choices"], "account_choices", source["account_choices"])); break;
                    case "account_context": values.Add((["account_context", "account-7"], "account_context", source["account_context"])); break;
                    case "runtime": values.Add((["runtime"], "runtime", JsonSerializer.SerializeToNode(new { rest_product_version = "1.2.3", recorder_product_version = recorderAvailable ? "1.2.2" : null, recorder_status = recorderAvailable ? "mismatch" : "unavailable" }))); break;
                    default: throw new InvalidDataException("unknown fixture projection");
                }
            }
            else if (path == "/history/periods")
            {
                foreach (var item in source["periods"]!.AsArray())
                {
                    if (query.GetProperty("purpose").GetString() == "window" && (item!["start_at"]!.GetValue<long>() >= query.GetProperty("until").GetInt64() || item["end_at"]!.GetValue<long>() < query.GetProperty("from").GetInt64())) continue;
                    values.Add((["period", item!["id"]!.GetValue<string>()], "period", item));
                }
            }
            else if (path == "/history")
            {
                var period = query.GetProperty("period").GetString()!;
                foreach (var item in source["histories"]![period]!["samples"]!.AsArray())
                {
                    var value = item!.DeepClone().AsObject(); var timestamp = value["timestamp"]!.GetValue<long>(); var reset = value["reset_at"]!.GetValue<long>(); value.Remove("timestamp"); value.Remove("reset_at");
                    values.Add((["sample", period, reset, timestamp], "sample", value));
                }
                foreach (var item in source["histories"]![period]!["gaps"]!.AsArray())
                { var value = item!.DeepClone().AsObject(); var gap = value["gap_id"]!.GetValue<string>(); value.Remove("gap_id"); values.Add((["gap", period, gap], "gap", value)); }
            }
            else if (path == "/threads")
            {
                if (!emptyThreads) foreach (var item in source["threads"]!.AsArray()) { var value = item!.DeepClone().AsObject(); var id = value["id"]!.GetValue<string>(); value.Remove("id"); values.Add((["thread", id], "thread", value)); }
            }
            else throw new InvalidDataException("unknown fixture resource");
            var rows = values.Select(v => new Row(E(v.Key), catalog.Intern(v.Kind, E(v.Value)))).OrderBy(row => WireReceiver.CanonicalKey(row.Key), StringComparer.Ordinal).ToArray();
            var image = Convert.ToHexStringLower(WireCodec.Manifest(rows.Select(row => (WireCodec.Canonical(null, row.Key), catalog.Digests[row.Id]))));
            if (path == "/state" && query.GetProperty("parts").GetString() == "main" && unavailable) image = Hash(Encoding.UTF8.GetBytes("unavailable:" + image));
            return new(key.Clone(), Id(key), image, rows);
        }
    }
    object[] Resolved(Peer peer) => peer.Targets.Select(Resolve).Select(v => (object)new { view_id = v.Id, target_image = v.Image }).ToArray();
    void Notify(Peer peer)
    {
        lock (peer.Gate)
        {
            if (peer.Output is null) return;
            var changed = peer.Targets.Select(Resolve).Where(v => (!peer.Receiver.Views.TryGetValue(v.Id, out var current) || current.Image != v.Image) && (!peer.Notified.TryGetValue(v.Id, out var image) || image != v.Image)).Select(v => new { view_id = v.Id, target_image = v.Image }).ToArray();
            if (changed.Length > 0 && peer.Output.Writer.TryWrite(new { type = "changed", notify_seq = ++peer.NotifySequence, views = changed })) foreach (var view in changed) peer.Notified[view.view_id] = view.target_image;
        }
    }
    static bool Confirm(Peer peer, string root)
    {
        if (peer.Receiver.Root == root) return true;
        for (var i = 0; i < peer.Pending.Count; i++)
        {
            var next = new WireReceiver(); foreach (var accepted in peer.Accepted) next.Apply(accepted.Data, accepted.Keys);
            next.Apply(peer.Pending[i].Data, peer.Pending[i].Keys);
            if (next.Root != root) continue;
            peer.Accepted.Add(peer.Pending[i]); peer.Pending.RemoveAt(i); peer.Receiver = next; return true;
        }
        return false;
    }
    public async Task HandleAsync(HttpContext context)
    {
        Interlocked.Increment(ref requests); Stats();
        context.Response.Headers.CacheControl = "no-store";
        var path = context.Request.Path.Value!;
        if (context.Request.Protocol != "HTTP/2") { context.Response.StatusCode = 400; return; }
        if (path is not ("/state" or "/history/periods" or "/history" or "/threads" or "/subscription")) { Interlocked.Increment(ref oldRequests); Stats(); context.Response.StatusCode = 404; return; }
        var identity = context.Request.Headers["Codex-Info-Consumer"].ToString();
        var root = context.Request.Headers["Codex-Info-Cache-Root"].ToString();
        if (identity.Length == 0) { if (root != new WireReceiver().Root || path == "/subscription") { context.Response.StatusCode = 400; return; } identity = "fixture-" + Guid.NewGuid().ToString("N"); peers[identity] = new(); }
        if (!peers.TryGetValue(identity, out var peer)) { context.Response.StatusCode = 409; return; }
        context.Response.Headers["Codex-Info-Consumer"] = identity;
        if (path == "/subscription") { if (context.Request.Method != "POST") { context.Response.StatusCode = 405; return; } await Subscription(context, identity, peer); return; }
        if (context.Request.Method != "GET") { context.Response.StatusCode = 405; return; }
        var requestTarget = path + context.Request.QueryString.Value;
        if (context.Request.Headers.TryGetValue("Codex-Info-Delivery", out var resumed))
        {
            Body? body; lock (peer.Gate) peer.Bodies.TryGetValue(resumed.ToString(), out body);
            if (body is null || body.Target != requestTarget || !int.TryParse(context.Request.Headers["Codex-Info-Offset"], out var offset) || offset < 0 || offset > body.Bytes.Length || Hash(body.Bytes.AsSpan(0, offset).ToArray()) != context.Request.Headers["Codex-Info-Prefix-Sha256"].ToString()) { context.Response.StatusCode = 409; return; }
            await WriteData(context, resumed.ToString(), body.Bytes, offset); return;
        }
        JsonElement response;
        lock (peer.Gate)
        {
            if (!Confirm(peer, root)) { response = E(new { result = "sync_required", reason = "receipt_unknown", view_id = "fixture", target_image = (string?)null, control_seq = peer.LastSequence }); }
            else
            {
                var keys = WireTransport.Keys(requestTarget); var views = keys.Select(Resolve).ToArray();
                if (views.Any(v => v.Key[0].GetString() == "/state" && v.Key[1].GetProperty("parts").GetString() == "main"))
                {
                    lock (sourceGate) { if (unavailable) { context.Response.StatusCode = 503; return; } if (partialModels) Interlocked.Increment(ref partialRequests); }
                    Stats();
                }
                if (!context.Request.Query.ContainsKey("cursor") && views.All(v => peer.Receiver.Views.TryGetValue(v.Id, out var accepted) && accepted.Image == v.Image)) { context.Response.StatusCode = 204; return; }
                var definitions = new List<WireDefinition>(); var known = peer.Receiver.Definitions.Keys.ToHashSet(); var changes = new List<object>(); var completed = new List<string>(); string? cursor = null;
                var offset = context.Request.Query.TryGetValue("cursor", out var supplied) ? int.Parse(supplied.ToString(), System.Globalization.CultureInfo.InvariantCulture) : 0;
                var offeredIds = peer.Pending.SelectMany(p => p.Data.GetProperty("definitions").EnumerateArray()).Select(d => d.GetProperty("id").GetUInt64()).ToHashSet();
                bool blocked; lock (sourceGate) blocked = views.Any(v => v.Rows.Skip(offset).Take(path == "/history" ? 2 : path == "/threads" ? 256 : int.MaxValue).Any(row => catalog.Transfer(row.Id, known).Any(d => offeredIds.Contains(d.Id))));
                if (blocked)
                {
                    foreach (var view in views) peer.Blocked.Add(view.Id);
                    response = E(new { result = "pending", reason = "delivery_pending", view_id = views[0].Id, target_image = views[0].Image, control_seq = peer.LastSequence });
                }
                else
                {
                    foreach (var view in views)
                    {
                        peer.Blocked.Remove(view.Id);
                        var basis = peer.Receiver.Views.GetValueOrDefault(view.Id); var maximum = path == "/history" ? 2 : path == "/threads" ? 256 : int.MaxValue;
                        var batch = view.Rows.Skip(offset).Take(maximum).ToArray();
                        var sets = new List<object>();
                        foreach (var row in batch)
                        {
                            if (basis is not null && basis.Bindings.TryGetValue(WireReceiver.CanonicalKey(row.Key), out var existing) && existing == row.Id) continue;
                            lock (sourceGate) foreach (var definition in catalog.Transfer(row.Id, known)) { definitions.Add(definition); known.Add(definition.Id); }
                            sets.Add(new { key = row.Key, value = new Dictionary<string, ulong> { { "ref", row.Id } } });
                        }
                        var existingKeys = basis?.Bindings.Keys ?? Enumerable.Empty<string>(); var live = view.Rows.Select(row => WireReceiver.CanonicalKey(row.Key)).ToHashSet(StringComparer.Ordinal);
                        var removes = offset == 0 ? existingKeys.Where(k => !live.Contains(k)).Select(k => JsonDocument.Parse(k).RootElement.Clone()).ToArray() : [];
                        changes.Add(new { view_id = view.Id, account = view.Key[1].TryGetProperty("account", out var account) ? account : E(null), base_image = basis?.Image, target_image = view.Image, sets, removes });
                        if (offset + batch.Length < view.Rows.Count) cursor = (offset + batch.Length).ToString(System.Globalization.CultureInfo.InvariantCulture); else completed.Add(view.Id);
                    }
                    response = E(new { result = "data", delivery_id = "delivery-" + Guid.NewGuid().ToString("N"), definitions = definitions.Select(d => new { id = d.Id, kind = d.Kind, value = d.Value }), patches = Array.Empty<object>(), fragments = Array.Empty<object>(), changes, completed_views = completed, next_cursor = cursor });
                    peer.Pending.Add(new(response.Clone(), views.ToDictionary(v => v.Id, v => v.Key, StringComparer.Ordinal)));
                }
            }
        }
        var bytes = JsonSerializer.SerializeToUtf8Bytes(response); context.Response.ContentType = "application/json; charset=utf-8"; context.Response.ContentLength = bytes.Length;
        if (response.GetProperty("result").GetString() == "data") { var delivery = response.GetProperty("delivery_id").GetString()!; lock (peer.Gate) peer.Bodies[delivery] = new(requestTarget, bytes); await WriteData(context, delivery, bytes, 0); return; }
        await context.Response.Body.WriteAsync(bytes, context.RequestAborted);
    }
    static async Task WriteData(HttpContext context, string delivery, byte[] bytes, int offset)
    {
        context.Response.ContentType = "application/json; charset=utf-8"; context.Response.ContentLength = bytes.Length - offset;
        context.Response.Headers["Codex-Info-Delivery"] = delivery; context.Response.Headers["Codex-Info-Body-Sha256"] = Hash(bytes); context.Response.Headers["Codex-Info-Total-Bytes"] = bytes.Length.ToString(); context.Response.Headers["Codex-Info-Offset"] = offset.ToString();
        await context.Response.Body.WriteAsync(bytes.AsMemory(offset), context.RequestAborted);
    }
    async Task Subscription(HttpContext context, string identity, Peer peer)
    {
        context.Response.ContentType = "application/x-ndjson; charset=utf-8"; await context.Response.StartAsync(context.RequestAborted);
        var output = Channel.CreateBounded<object>(new BoundedChannelOptions(16) { SingleReader = true, FullMode = BoundedChannelFullMode.Wait });
        lock (peer.Gate) peer.Output = output;
        var writer = Task.Run(async () => { await foreach (var item in output.Reader.ReadAllAsync(context.RequestAborted)) { var bytes = JsonSerializer.SerializeToUtf8Bytes(item); if (bytes.Length + 1 > 8192) throw new InvalidDataException("fixture control too large"); await context.Response.Body.WriteAsync(bytes, context.RequestAborted); await context.Response.Body.WriteAsync(new byte[] { 10 }, context.RequestAborted); await context.Response.Body.FlushAsync(context.RequestAborted); } });
        try
        {
            using var input = new StreamReader(context.Request.Body, Encoding.UTF8, false, 8192, true);
            while (await input.ReadLineAsync(context.RequestAborted) is string line)
            {
                if (Encoding.UTF8.GetByteCount(line) + 1 > 8192) throw new InvalidDataException("fixture control too large");
                using var document = JsonDocument.Parse(line); var record = document.RootElement; var type = record.GetProperty("type").GetString(); var seq = record.GetProperty("seq").GetUInt64(); object reply;
                lock (peer.Gate)
                {
                    if (seq <= peer.LastSequence) throw new InvalidDataException("fixture sequence must advance"); peer.LastSequence = seq;
                    switch (type)
                    {
                        case "open":
                            if (record.GetProperty("consumer").GetString() != identity || !Confirm(peer, record.GetProperty("cache_root").GetString()!)) throw new InvalidDataException("fixture receipt mismatch");
                            peer.Targets = record.GetProperty("subscriptions").EnumerateArray().Select(t => WireTransport.Keys(Target(t)).Single()).ToArray();
                            reply = new { type = "ready", ack_seq = seq, session = "fixture-session", cache_state = "matched", views = Resolved(peer) }; break;
                        case "subscribe": peer.Targets = record.GetProperty("targets").EnumerateArray().Select(t => WireTransport.Keys(Target(t)).Single()).ToArray(); reply = new { type = "ack", ack_seq = seq, views = Resolved(peer) }; break;
                        case "receipt":
                            var offered = peer.Pending.Concat(peer.Accepted).SingleOrDefault(p => p.Data.GetProperty("delivery_id").GetString() == record.GetProperty("delivery_id").GetString());
                            if (offered is null || record.GetProperty("complete_through").GetInt32() != new[] { "definitions", "patches", "fragments", "changes" }.Sum(name => offered.Data.GetProperty(name).GetArrayLength()) || !Confirm(peer, record.GetProperty("cache_root").GetString()!)) throw new InvalidDataException("fixture receipt mismatch");
                            reply = new { type = "ack", ack_seq = seq }; break;
                        case "check": reply = new { type = "ack", ack_seq = seq, views = Resolved(peer) }; break;
                        case "notify_ack": reply = new { type = "ack", ack_seq = seq }; break;
                        case "close": return;
                        default: throw new InvalidDataException("unknown fixture control");
                    }
                }
                await output.Writer.WriteAsync(reply, context.RequestAborted);
                if (type == "receipt")
                {
                    object[] released; lock (peer.Gate) { released = peer.Targets.Select(Resolve).Where(v => peer.Blocked.Contains(v.Id)).Select(v => (object)new { view_id = v.Id, target_image = v.Image }).ToArray(); }
                    if (released.Length > 0) await output.Writer.WriteAsync(new { type = "changed", notify_seq = ++peer.NotifySequence, views = released }, context.RequestAborted);
                }
            }
        }
        catch (OperationCanceledException) when (context.RequestAborted.IsCancellationRequested) { }
        catch (IOException) when (context.RequestAborted.IsCancellationRequested) { }
        finally { lock (peer.Gate) if (ReferenceEquals(peer.Output, output)) peer.Output = null; output.Writer.TryComplete(); try { await writer; } catch (OperationCanceledException) when (context.RequestAborted.IsCancellationRequested) { } }
    }
    public void Dispose() { watcher.Dispose(); lifetime.Cancel(); updates.Writer.TryComplete(); changes.GetAwaiter().GetResult(); statusOutput.Dispose(); lifetime.Dispose(); }
}

sealed class Catalog
{
    public readonly Dictionary<ulong, WireDefinition> Definitions = [];
    public readonly Dictionary<ulong, byte[]> Digests = [];
    readonly Dictionary<string, ulong> byValue = new(StringComparer.Ordinal);
    readonly Dictionary<ulong, ulong[]> children = [];
    ulong next;
    public ulong Intern(string? kind, JsonElement value)
    {
        var owned = value.ValueKind switch { JsonValueKind.Null => "null", JsonValueKind.Array => "list", _ => kind ?? (value.ValueKind switch { JsonValueKind.String => "text", JsonValueKind.True or JsonValueKind.False => "bool", JsonValueKind.Number when value.TryGetUInt64(out _) => "u64", JsonValueKind.Number when value.TryGetInt64(out _) => "i64", _ => "f64" }) };
        var digest = WireCodec.Digest(kind, value); var key = owned + ":" + Convert.ToHexStringLower(digest); if (byValue.TryGetValue(key, out var existing)) return existing;
        var dependencies = new List<ulong>(); object? encoded;
        object Reference(string? childKind, JsonElement child) { var id = Intern(childKind, child); dependencies.Add(id); return new Dictionary<string, ulong> { { "ref", id } }; }
        if (value.ValueKind == JsonValueKind.Object) encoded = value.EnumerateObject().ToDictionary(p => p.Name, p => Reference(WireCodec.FieldKind(kind, p.Name), p.Value));
        else if (value.ValueKind == JsonValueKind.Array) { var elementKind = kind?.EndsWith("[]", StringComparison.Ordinal) == true ? kind[..^2] : null; encoded = value.EnumerateArray().Select(v => Reference(elementKind, v)).ToArray(); }
        else encoded = value.Clone();
        var assigned = ++next; Definitions[assigned] = new(assigned, owned, JsonSerializer.SerializeToElement(encoded)); Digests[assigned] = digest; children[assigned] = dependencies.ToArray(); byValue[key] = assigned; return assigned;
    }
    public IReadOnlyList<WireDefinition> Transfer(ulong root, HashSet<ulong> known)
    {
        var output = new List<WireDefinition>(); var seen = new HashSet<ulong>();
        void Walk(ulong id) { if (known.Contains(id) || !seen.Add(id)) return; foreach (var child in children[id]) Walk(child); output.Add(Definitions[id]); }
        Walk(root); return output;
    }
}
