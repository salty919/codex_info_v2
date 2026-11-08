// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Security.Cryptography;
using System.Collections.Immutable;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
namespace CodexInfo.WindowsClient.Core;

public sealed record WireView(string Image, ImmutableSortedDictionary<string, ulong> Bindings) { public WireManifestTree ReceiptTree { get; init; } = new(); }
public sealed record WirePartial(string DeliveryId, ulong Id, string Kind, string ContentHash, ulong TotalBytes, JsonElement Descriptor, byte[] Bytes);
public sealed class WireReceiver
{
    public ImmutableSortedDictionary<ulong, WireDefinition> Definitions { get; private set; } = ImmutableSortedDictionary<ulong, WireDefinition>.Empty;
    public ImmutableDictionary<string, WireView> Views { get; private set; } = ImmutableDictionary.Create<string, WireView>(StringComparer.Ordinal);
    public ImmutableDictionary<string, JsonElement> ViewKeys { get; private set; } = ImmutableDictionary.Create<string, JsonElement>(StringComparer.Ordinal);
    public ImmutableDictionary<ulong, WirePartial> Partials { get; private set; } = ImmutableDictionary<ulong, WirePartial>.Empty;
    ImmutableDictionary<string, WireView> pending = ImmutableDictionary.Create<string, WireView>(StringComparer.Ordinal);
    ImmutableDictionary<ulong, byte[]> digests = ImmutableDictionary<ulong, byte[]>.Empty;
    WireManifestTree tree = new();
    ImmutableDictionary<string, ImmutableHashSet<string>> changedBindings = ImmutableDictionary.Create<string, ImmutableHashSet<string>>(StringComparer.Ordinal);
    public string Root { get; private set; } = Convert.ToHexStringLower(WireCodec.Manifest([]));
    public static JsonElement Element(object? value) => JsonSerializer.SerializeToElement(value);
    static ulong Id(JsonElement value) => value.GetUInt64() is var id && id > 0 ? id : throw new InvalidDataException("invalid ID");
    internal WireReceiver Fork() => (WireReceiver)MemberwiseClone();
    public void Apply(JsonElement data, IReadOnlyDictionary<string, JsonElement>? viewKeys = null)
    {
        if (data.GetProperty("result").GetString() != "data") throw new InvalidDataException("invalid delivery");
        var nextTree = tree.Fork(); var changed = changedBindings.ToBuilder();
        var definitions = Definitions.ToBuilder(); var hashes = digests.ToBuilder(); var partials = Partials.ToBuilder(); var added = new List<ulong>();
        void Add(WireDefinition definition) { if (!definitions.TryAdd(definition.Id, definition)) throw new InvalidDataException("duplicate definition"); added.Add(definition.Id); }
        foreach (var definition in data.GetProperty("definitions").EnumerateArray()) Add(new(Id(definition.GetProperty("id")), definition.GetProperty("kind").GetString()!, definition.GetProperty("value").Clone()));
        foreach (var patch in data.GetProperty("patches").EnumerateArray())
        {
            var id = Id(patch.GetProperty("id")); var kind = patch.GetProperty("kind").GetString()!; var baseId = Id(patch.GetProperty("base").GetProperty("ref"));
            if (!definitions.TryGetValue(baseId, out var basis) || basis.Kind != kind) throw new InvalidDataException("patch base mismatch");
            var value = JsonNode.Parse(basis.Value.GetRawText())!;
            foreach (var op in patch.GetProperty("ops").EnumerateArray())
            {
                var path = op.GetProperty("path").GetString()!; var segments = path.Split('/'); if (segments.Length < 2 || segments[0] != "") throw new InvalidDataException("invalid pointer");
                JsonNode parent = value; for (var i = 1; i < segments.Length - 1; i++) parent = parent[Unescape(segments[i])] ?? throw new InvalidDataException("missing patch parent");
                if (parent is not JsonObject obj) throw new InvalidDataException("patch parent not object"); var key = Unescape(segments[^1]);
                if (op.GetProperty("op").GetString() == "set") obj[key] = JsonNode.Parse(op.GetProperty("value").GetRawText()); else if (op.GetProperty("op").GetString() != "remove" || !obj.Remove(key)) throw new InvalidDataException("invalid remove");
            }
            Add(new(id, kind, JsonSerializer.SerializeToElement(value)));
        }
        var fragmentBuffers = new Dictionary<ulong, MemoryStream>();
        foreach (var fragment in data.GetProperty("fragments").EnumerateArray())
        {
            var id = Id(fragment.GetProperty("id")); var offset = fragment.GetProperty("offset").GetUInt64(); var total = fragment.GetProperty("total_bytes").GetUInt64(); var last = fragment.GetProperty("last").GetBoolean(); var bytes = Base64(fragment.GetProperty("data").GetString()!); var kind = fragment.GetProperty("kind").GetString()!; var hash = fragment.GetProperty("content_hash").GetString()!;
            if (bytes.Length is 0 or > 8192 || !last && bytes.Length != 8192 || offset > total || total - offset < (ulong)bytes.Length || last != (offset + (ulong)bytes.Length == total)) throw new InvalidDataException("invalid fragment boundary");
            if (!partials.TryGetValue(id, out var partial)) { if (offset != 0 || !fragment.TryGetProperty("descriptor", out var descriptor) || descriptor.ValueKind != JsonValueKind.Object) throw new InvalidDataException("missing fragment descriptor"); partial = new(data.GetProperty("delivery_id").GetString()!, id, kind, hash, total, descriptor.Clone(), []); }
            if (!fragmentBuffers.TryGetValue(id, out var accumulated)) { accumulated = new MemoryStream(); accumulated.Write(partial.Bytes); fragmentBuffers[id] = accumulated; }
            if (partial.Kind != kind || partial.ContentHash != hash || partial.TotalBytes != total || (ulong)accumulated.Length != offset) throw new InvalidDataException("fragment prefix mismatch");
            if (fragment.TryGetProperty("descriptor", out var supplied) && supplied.ValueKind != JsonValueKind.Null && !WireCodec.Canonical(null, supplied).AsSpan().SequenceEqual(WireCodec.Canonical(null, partial.Descriptor))) throw new InvalidDataException("fragment descriptor mismatch");
            accumulated.Write(bytes); var prefix = accumulated.GetBuffer().AsMemory(0, checked((int)accumulated.Length));
            if (last) { if (Convert.ToHexStringLower(SHA256.HashData(prefix.Span)) != hash || kind != "definition") throw new InvalidDataException("fragment content mismatch"); using var document = JsonDocument.Parse(prefix); var d = document.RootElement; if (Id(d.GetProperty("id")) != id) throw new InvalidDataException("fragment ID mismatch"); Add(new(id, d.GetProperty("kind").GetString()!, d.GetProperty("value").Clone())); partials.Remove(id); fragmentBuffers.Remove(id); accumulated.Dispose(); } else partials[id] = partial;
        }
        foreach (var (id, buffer) in fragmentBuffers) { partials[id] = partials[id] with { Bytes = buffer.ToArray() }; buffer.Dispose(); }
        foreach (var id in added) { var definition = definitions[id]; hashes[id] = WireCodec.Digest(definition.Kind, definition.Value, definitions, hashes); var hash = hashes[id]; nextTree.Set(WireCodec.Canonical(null, Element(new object[] { "received", Convert.ToHexStringLower(hash) })), hash); nextTree.Set(WireCodec.Canonical(null, Element(new object[] { "alias", id })), hash); }
        foreach (var id in data.GetProperty("fragments").EnumerateArray().Select(f => f.GetProperty("id").GetUInt64()).Distinct())
        {
            if (Partials.TryGetValue(id, out var old)) nextTree.Remove(WireCodec.Canonical(null, Element(new object[] { "partial", old.DeliveryId, id })));
            if (partials.TryGetValue(id, out var current)) nextTree.Set(WireCodec.Canonical(null, Element(new object[] { "partial", current.DeliveryId, id })), SHA256.HashData(current.Bytes));
        }
        var staged = pending.ToBuilder();
        var views = Views.ToBuilder(); var keys = ViewKeys.ToBuilder(); if (viewKeys is not null) foreach (var (key, value) in viewKeys) keys[key] = value.Clone();
        foreach (var change in data.GetProperty("changes").EnumerateArray())
        {
            var id = change.GetProperty("view_id").GetString()!; var target = change.GetProperty("target_image").GetString()!;
            if (!staged.TryGetValue(id, out var view)) { view = views.TryGetValue(id, out var current) ? new(current.Image, current.Bindings) { ReceiptTree = current.ReceiptTree.Fork() } : new("", ImmutableSortedDictionary.Create<string, ulong>(StringComparer.Ordinal)); }
            if (view.Image != target) { var basis = change.GetProperty("base_image"); var expected = views.TryGetValue(id, out var current) ? current.Image : null; if (basis.ValueKind == JsonValueKind.Null ? expected is not null : basis.GetString() != expected) throw new InvalidDataException("view base mismatch"); view = views.TryGetValue(id, out var accepted) ? new(target, accepted.Bindings) { ReceiptTree = accepted.ReceiptTree.Fork() } : new(target, ImmutableSortedDictionary.Create<string, ulong>(StringComparer.Ordinal)); changed.Remove(id); }
            view = view with { ReceiptTree = view.ReceiptTree.Fork() };
            if (!changed.TryGetValue(id, out var changedKeys)) { changedKeys = ImmutableHashSet.Create<string>(StringComparer.Ordinal); }
            foreach (var set in change.GetProperty("sets").EnumerateArray()) { var definition = Id(set.GetProperty("value").GetProperty("ref")); if (!definitions.ContainsKey(definition)) throw new InvalidDataException("unresolved binding"); var logical = CanonicalKey(set.GetProperty("key")); view = view with { Bindings = view.Bindings.SetItem(logical, definition) }; if (keys.TryGetValue(id, out var viewKey)) view.ReceiptTree.Set(WireCodec.Canonical(null, Element(new object[] { "binding", viewKey, set.GetProperty("key") })), hashes[definition]); changedKeys = changedKeys.Add(logical); }
            foreach (var remove in change.GetProperty("removes").EnumerateArray()) { var logical = CanonicalKey(remove); if (!view.Bindings.ContainsKey(logical)) throw new InvalidDataException("missing remove binding"); view = view with { Bindings = view.Bindings.Remove(logical) }; if (keys.TryGetValue(id, out var viewKey)) view.ReceiptTree.Remove(WireCodec.Canonical(null, Element(new object[] { "binding", viewKey, remove }))); changedKeys = changedKeys.Add(logical); }
            staged[id] = view; changed[id] = changedKeys;
        }
        foreach (var complete in data.GetProperty("completed_views").EnumerateArray()) { var id = complete.GetString()!; if (!staged.Remove(id, out var view)) throw new InvalidDataException("missing completed view"); if (keys.TryGetValue(id, out var key)) { var prefix = WireCodec.Canonical(null, Element(new object[] { "binding", key })); prefix[^1] = (byte)','; nextTree.ReplacePrefix(prefix, view.ReceiptTree); } changed.Remove(id); views[id] = view; }
        tree = nextTree; changedBindings = changed.ToImmutable();
        Definitions = definitions.ToImmutable(); digests = hashes.ToImmutable(); Partials = partials.ToImmutable(); pending = staged.ToImmutable(); Views = views.ToImmutable(); ViewKeys = keys.ToImmutable(); Root = Convert.ToHexStringLower(tree.Root);
    }
    public JsonElement Resolve(ulong id)
    {
        using var bytes = new MemoryStream(); using (var writer = new Utf8JsonWriter(bytes)) { Write(Element(new Dictionary<string, ulong> { { "ref", id } }), writer, [], 0); }
        using var result = JsonDocument.Parse(bytes.ToArray()); return result.RootElement.Clone();
    }
    void Write(JsonElement value, Utf8JsonWriter writer, HashSet<ulong> visiting, int depth)
    {
        if (depth > 128) throw new InvalidDataException("reference depth exceeded");
        if (value.ValueKind == JsonValueKind.Object && value.TryGetProperty("ref", out var reference)) { var id = Id(reference); if (!visiting.Add(id) || !Definitions.TryGetValue(id, out var definition)) throw new InvalidDataException("invalid reference"); Write(definition.Value, writer, visiting, depth + 1); visiting.Remove(id); return; }
        if (value.ValueKind == JsonValueKind.Object) { writer.WriteStartObject(); foreach (var field in value.EnumerateObject()) { writer.WritePropertyName(field.Name); Write(field.Value, writer, visiting, depth + 1); } writer.WriteEndObject(); }
        else if (value.ValueKind == JsonValueKind.Array) { writer.WriteStartArray(); foreach (var child in value.EnumerateArray()) Write(child, writer, visiting, depth + 1); writer.WriteEndArray(); } else value.WriteTo(writer);
    }
    public IReadOnlyList<(JsonElement Key, byte[] Hash, ulong? Id)> Manifest()
    {
        var entries = new List<(JsonElement, byte[], ulong?)>(); var received = new HashSet<string>(StringComparer.Ordinal);
        foreach (var (id, hash) in digests) { var digest = Convert.ToHexStringLower(hash); if (received.Add(digest)) entries.Add((Element(new object[] { "received", digest }), hash, null)); entries.Add((Element(new object[] { "alias", id }), hash, id)); }
        foreach (var (id, view) in Views) if (ViewKeys.TryGetValue(id, out var key)) foreach (var (logical, definition) in view.Bindings) { using var document = JsonDocument.Parse(logical); entries.Add((Element(new object[] { "binding", key, document.RootElement }), digests[definition], definition)); }
        foreach (var (id, p) in Partials) entries.Add((Element(new object[] { "partial", p.DeliveryId, id }), SHA256.HashData(p.Bytes), id)); return entries;
    }

    public void DiscardPartials(string viewId) { tree = tree.Fork(); foreach (var id in Partials.Where(e => e.Value.Descriptor.GetProperty("view_id").GetString() == viewId).Select(e => e.Key).ToArray()) { tree.Remove(WireCodec.Canonical(null, Element(new object[] { "partial", Partials[id].DeliveryId, id }))); Partials = Partials.Remove(id); } Root = Convert.ToHexStringLower(tree.Root); }
    public object[] PartialReceipts() => Partials.Values.Select(p => (object)new { delivery_id = p.DeliveryId, id = p.Id, committed_offset = p.Bytes.Length, prefix_hash = Convert.ToHexStringLower(SHA256.HashData(p.Bytes)), content_hash = p.ContentHash, total_bytes = p.TotalBytes, descriptor = p.Descriptor }).ToArray();
    public static string CanonicalKey(JsonElement key) => Encoding.UTF8.GetString(WireCodec.Canonical(null, key));
    static string Unescape(string value) => value.Replace("~1", "/", StringComparison.Ordinal).Replace("~0", "~", StringComparison.Ordinal);
    public static byte[] Base64(string value) => Convert.FromBase64String(value.Replace('-', '+').Replace('_', '/') + new string('=', (4 - value.Length % 4) % 4));
}
