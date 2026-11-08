using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using CodexInfo.WindowsClient.Core;
namespace CodexInfo.WindowsClient.Core.Tests;

internal sealed record FixtureRow(object[] Key, string Kind, JsonElement Value);
internal class WireFacadeFixture : HttpMessageHandler
{
    readonly Dictionary<string, ulong> ids = new(StringComparer.Ordinal);
    readonly Dictionary<ulong, WireDefinition> pool = new();
    readonly Dictionary<string, string> images = new(StringComparer.Ordinal);
    int deliverySequence;
    ChannelWriter<byte[]>? notifications;
    public void NotifyChanged() => notifications?.TryWrite(JsonSerializer.SerializeToUtf8Bytes(new { type = "changed", notify_seq = 1ul, views = images.Keys.Select(id => new { view_id = id, target_image = (string?)null }) }).Concat(new byte[] { 10 }).ToArray());
    public List<string> Gets { get; } = []; public HttpStatusCode Status { get; set; } = HttpStatusCode.OK; public Exception? Failure { get; set; }
    public string? ControlResult { get; set; }
    public string? RawBody { get; set; }
    public bool NoContentType { get; set; }
    public bool NoLength { get; set; }
    public bool NoStore { get; set; } = true;
    public Func<string, IReadOnlyList<FixtureRow>> Rows { get; set; } = _ => [new(["main"], "main", MainValue())];
    public static JsonElement Element(object? value) => JsonSerializer.SerializeToElement(value);
    public static JsonElement MainValue(string state = "ready", string modelsStatus = "complete", bool empty = false) => Element(new { acquisition_state = state, observed_at = (long?)1800000000, authenticated = true, plan_label = (string?)null, selection = new { account_id = "account-1", login_id = (string?)null, is_current = true }, quota = new { remaining_percent = 50.0, reset_at = 1800003600L, window_seconds = 3600L, monthly = false }, models_status = modelsStatus, models = empty ? Array.Empty<object>() : new object[] { new { model = "gpt-6.1-sol", total_tokens = ulong.MaxValue, input_tokens = ulong.MaxValue, cached_input_tokens = 0ul, cache_write_input_tokens = (ulong?)0, output_tokens = 0ul, estimated_cost = new { price_revision = "saved-price", ordinary_input_dollars = 0.0, cached_input_dollars = 0.0, cache_write_input_dollars = -0.0, output_dollars = 0.0, total_dollars = -0.0 } } }, thread_summary = (object?)null });
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token)
    {
        if (Failure is not null) throw Failure;
        if (request.Method == HttpMethod.Post) { var channel = Channel.CreateUnbounded<byte[]>(); notifications = channel.Writer; _ = Task.Run(async () => { try { await request.Content!.CopyToAsync(new ControlSink(channel.Writer), token); } catch (OperationCanceledException) when (token.IsCancellationRequested) { } finally { channel.Writer.TryComplete(); } }, token); return Task.FromResult(Response(new StreamContent(new Notifications(channel.Reader)), HttpStatusCode.OK, "application/x-ndjson")); }
        var target = request.RequestUri!.PathAndQuery; Gets.Add(target); byte[] bytes; var deliveryId = "facade-delivery-" + Interlocked.Increment(ref deliverySequence);
        if (RawBody is not null) bytes = Encoding.UTF8.GetBytes(RawBody);
        else if (ControlResult is not null) bytes = JsonSerializer.SerializeToUtf8Bytes(new { result = ControlResult, reason = "preparing", view_id = "pending", target_image = (string?)null, control_seq = 1 });
        else
        {
            var rows = Rows(target);
            var added = new List<WireDefinition>();
            var rowIds = rows.Select(row =>
            {
                var identity = Convert.ToHexStringLower(WireCodec.Digest(row.Kind, row.Value, pool));
                if (!ids.TryGetValue(identity, out var id)) { id = (ulong)ids.Count + 1; ids[identity] = id; var definition = new WireDefinition(id, row.Kind, row.Value); pool[id] = definition; added.Add(definition); }
                return id;
            }).ToArray();
            var entries = rows.Select((row, index) => (WireCodec.Canonical(null, Element(row.Key)), WireCodec.Digest(row.Kind, row.Value, pool))).ToArray();
            var image = Convert.ToHexStringLower(WireCodec.Manifest(entries)); var viewKey = WireTransport.Keys(target).Single(); var viewId = Convert.ToHexStringLower(SHA256.HashData(WireCodec.Canonical(null, viewKey)));
            images.TryGetValue(viewId, out var previous); images[viewId] = image;
            bytes = JsonSerializer.SerializeToUtf8Bytes(new { result = "data", delivery_id = deliveryId, definitions = added.Select(d => new { id = d.Id, kind = d.Kind, value = d.Value }), patches = Array.Empty<object>(), fragments = Array.Empty<object>(), changes = new[] { new { view_id = viewId, account = (string?)null, base_image = previous, target_image = image, sets = rows.Select((row, index) => new { key = row.Key, value = new Dictionary<string, ulong> { { "ref", rowIds[index] } } }), removes = Array.Empty<object>() } }, completed_views = new[] { viewId }, next_cursor = (string?)null });
        }
        var content = new ByteArrayContent(bytes); var response = Response(content, Status, NoContentType ? null : "application/json"); if (NoLength) response.Content.Headers.ContentLength = null; if (ControlResult is not null) return Task.FromResult(response); response.Headers.Add("Codex-Info-Delivery", deliveryId); response.Headers.Add("Codex-Info-Body-Sha256", Convert.ToHexStringLower(SHA256.HashData(bytes))); response.Headers.Add("Codex-Info-Total-Bytes", bytes.Length.ToString()); response.Headers.Add("Codex-Info-Offset", "0"); return Task.FromResult(response);
    }
    HttpResponseMessage Response(HttpContent content, HttpStatusCode status, string? type) { var response = new HttpResponseMessage(status) { Version = HttpVersion.Version20, Content = content }; if (type is not null) content.Headers.ContentType = new(type) { CharSet = "utf-8" }; if (NoStore) response.Headers.CacheControl = new() { NoStore = true }; response.Headers.Add("Codex-Info-Consumer", "facade-consumer"); return response; }
    sealed class Notifications(ChannelReader<byte[]> records) : Stream
    {
        byte[]? current; int offset; public override bool CanRead => true; public override bool CanWrite => false; public override bool CanSeek => false; public override long Length => throw new NotSupportedException(); public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
        public override async ValueTask<int> ReadAsync(Memory<byte> bytes, CancellationToken token = default) { if (current is null || offset == current.Length) { current = await records.ReadAsync(token); offset = 0; } var count = Math.Min(bytes.Length, current.Length - offset); current.AsMemory(offset, count).CopyTo(bytes); offset += count; return count; }
        public override int Read(byte[] bytes, int offset, int count) => throw new NotSupportedException(); public override void Flush() { }
        public override void SetLength(long value) => throw new NotSupportedException(); public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException(); public override void Write(byte[] bytes, int offset, int count) => throw new NotSupportedException();
    }
    sealed class ControlSink(ChannelWriter<byte[]> output) : Stream
    {
        readonly MemoryStream buffer = new(); public override bool CanRead => false; public override bool CanWrite => true; public override bool CanSeek => false; public override long Length => buffer.Length; public override long Position { get => buffer.Position; set => throw new NotSupportedException(); }
        public override void Write(byte[] bytes, int offset, int count) { for (var i = offset; i < offset + count; i++) { buffer.WriteByte(bytes[i]); if (bytes[i] != '\n') continue; using var doc = JsonDocument.Parse(buffer.ToArray()); buffer.SetLength(0); var value = doc.RootElement; object record = value.GetProperty("type").GetString() == "open" ? new { type = "ready", ack_seq = value.GetProperty("seq").GetUInt64(), session = "fixture-session", cache_state = "matched", views = Array.Empty<object>() } : (object)new { type = "ack", ack_seq = value.GetProperty("seq").GetUInt64() }; output.TryWrite(JsonSerializer.SerializeToUtf8Bytes(record).Concat(new[] { (byte)'\n' }).ToArray()); } }
        public override ValueTask WriteAsync(ReadOnlyMemory<byte> bytes, CancellationToken token = default) { Write(bytes.ToArray(), 0, bytes.Length); return ValueTask.CompletedTask; }
        public override void Flush() { }
        public override int Read(byte[] bytes, int offset, int count) => throw new NotSupportedException(); public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException(); public override void SetLength(long value) => throw new NotSupportedException();
    }
}
