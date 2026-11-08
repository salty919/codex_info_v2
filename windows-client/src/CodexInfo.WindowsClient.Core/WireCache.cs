// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text.Json;
namespace CodexInfo.WindowsClient.Core;

public sealed class WireCacheRecoveryException(string reason, Exception? inner = null) : IOException(reason, inner) { public string Reason { get; } = reason; }
public sealed class WireCache
{
    readonly string directory;
    public WireReceiver Receiver { get; private set; } = new();
    public string? Consumer { get; private set; }
    public ulong Sequence { get; private set; }
    readonly Dictionary<string, string> pendingCursors = new(StringComparer.Ordinal); public string? PendingCursor(string target) => pendingCursors.GetValueOrDefault(target);
    public WireCache(string directory)
    {
        this.directory = directory;
        try { Open(); } catch (WireCacheRecoveryException) { throw; } catch (Exception error) when (error is IOException or UnauthorizedAccessException) { throw new WireCacheRecoveryException("cache_io", error); }
    }
    void Open()
    {
        var anchor = Path.ChangeExtension(directory, "identity"); if (!Directory.Exists(directory) && File.Exists(anchor)) throw new WireCacheRecoveryException("cache_missing");
        var fresh = !Directory.Exists(directory);
        if (fresh) { Directory.CreateDirectory(directory); using (var empty = new FileStream(Path.Combine(directory, "receipts"), FileMode.CreateNew, FileAccess.Write, FileShare.Read)) { empty.Flush(true); } SaveAnchor(); SaveProfile(); return; }
        if (!File.Exists(Path.Combine(directory, "profile")) || !File.Exists(Path.Combine(directory, "receipts"))) throw new WireCacheRecoveryException("cache_missing");
        try
        {
            using var profile = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(directory, "profile"))); var p = profile.RootElement; Consumer = p.GetProperty("consumer").ValueKind == JsonValueKind.Null ? null : p.GetProperty("consumer").GetString(); Sequence = p.GetProperty("seq").GetUInt64(); var expected = p.GetProperty("root").GetString(); var qualified = Receiver.Root == expected;
            using var journal = new FileStream(Path.Combine(directory, "receipts"), FileMode.Open, FileAccess.ReadWrite, FileShare.Read); long committed = 0;
            Span<byte> lengthBytes = stackalloc byte[8]; Span<byte> seal = stackalloc byte[1];
            while (journal.Position < journal.Length)
            {
                if (!Read(journal, lengthBytes)) break; var length = BinaryPrimitives.ReadUInt64BigEndian(lengthBytes); if (length > 33_570_816) throw new InvalidDataException("cache record too large");
                var hash = new byte[32]; var bytes = new byte[checked((int)length)]; if (!Read(journal, hash) || !Read(journal, bytes) || !Read(journal, seal)) break;
                if (seal[0] != 0xa5 || !SHA256.HashData(bytes).AsSpan().SequenceEqual(hash)) throw new InvalidDataException("cache seal mismatch");
                using var record = JsonDocument.Parse(bytes); var root = record.RootElement;
                if (root.TryGetProperty("target", out var target) && target.ValueKind == JsonValueKind.String) { if (root.TryGetProperty("data", out var cursorData) && cursorData.ValueKind != JsonValueKind.Null && cursorData.TryGetProperty("next_cursor", out var cursor) && cursor.ValueKind == JsonValueKind.String) pendingCursors[target.GetString()!] = cursor.GetString()!; else pendingCursors.Remove(target.GetString()!); }
                if (root.TryGetProperty("data", out var data) && data.ValueKind != JsonValueKind.Null) { var keys = root.GetProperty("view_keys").EnumerateObject().ToDictionary(e => e.Name, e => e.Value.Clone()); Receiver.Apply(data, keys); }
                if (root.TryGetProperty("drop_partials", out var drop) && drop.ValueKind == JsonValueKind.String) Receiver.DiscardPartials(drop.GetString()!);
                qualified |= Receiver.Root == expected; committed = journal.Position;
            }
            if (!qualified || Consumer is null && Receiver.Definitions.Count != 0) throw new InvalidDataException("cache root mismatch");
            if (journal.Length != committed) { journal.SetLength(committed); journal.Flush(true); }
            if (File.Exists(anchor)) { var identity = JsonSerializer.Deserialize<string?>(File.ReadAllBytes(anchor)); if (identity is not null) { if (Consumer is null && Receiver.Definitions.Count == 0) Consumer = identity; else if (Consumer != identity) throw new WireCacheRecoveryException("cache_invalid"); } }
            SaveAnchor(); SaveProfile();
        }
        catch (WireCacheRecoveryException) { throw; }
        catch (Exception error) when (error is JsonException or InvalidDataException or KeyNotFoundException or FormatException) { throw new WireCacheRecoveryException("cache_invalid"); }
    }
    static bool Read(Stream stream, Span<byte> bytes) { var offset = 0; while (offset < bytes.Length) { var count = stream.Read(bytes[offset..]); if (count == 0) return false; offset += count; } return true; }
    static void CacheIo(Action action) { try { action(); } catch (WireCacheRecoveryException) { throw; } catch (Exception error) when (error is IOException or UnauthorizedAccessException) { throw new WireCacheRecoveryException("cache_io", error); } }
    void SaveAnchor() => CacheIo(() => { var path = Path.ChangeExtension(directory, "identity"); var temporary = Path.ChangeExtension(directory, "identity-next"); using (var output = new FileStream(temporary, FileMode.Create, FileAccess.Write, FileShare.None)) { output.Write(JsonSerializer.SerializeToUtf8Bytes(Consumer)); output.Flush(true); } File.Move(temporary, path, true); });
    void SaveProfile() => CacheIo(() => { var bytes = JsonSerializer.SerializeToUtf8Bytes(new { consumer = Consumer, seq = Sequence, root = Receiver.Root }); var temporary = Path.Combine(directory, "profile-next"); using (var output = new FileStream(temporary, FileMode.Create, FileAccess.Write, FileShare.None)) { output.Write(bytes); output.Flush(true); } File.Move(temporary, Path.Combine(directory, "profile"), true); });
    public void Issue(string id) { if (string.IsNullOrEmpty(id) || Consumer is not null && Consumer != id) throw new WireCacheRecoveryException("cache_invalid"); Consumer = id; SaveAnchor(); SaveProfile(); }
    void Append(byte[] bytes) => CacheIo(() => { using var journal = new FileStream(Path.Combine(directory, "receipts"), FileMode.Open, FileAccess.Write, FileShare.Read); journal.Seek(0, SeekOrigin.End); Span<byte> length = stackalloc byte[8]; BinaryPrimitives.WriteUInt64BigEndian(length, (ulong)bytes.Length); journal.Write(length); journal.Write(SHA256.HashData(bytes)); journal.Write(bytes); journal.WriteByte(0xa5); journal.Flush(true); });
    public void Commit(JsonElement data, IReadOnlyDictionary<string, JsonElement> viewKeys, string? target = null)
    {
        if (Consumer is null) throw new WireCacheRecoveryException("cache_invalid"); var next = Receiver.Fork(); next.Apply(data, viewKeys); Append(JsonSerializer.SerializeToUtf8Bytes(new { target, view_keys = viewKeys, data, drop_partials = (string?)null })); Receiver = next; if (target is not null) { if (data.GetProperty("next_cursor").ValueKind == JsonValueKind.String) pendingCursors[target] = data.GetProperty("next_cursor").GetString()!; else pendingCursors.Remove(target); }
        SaveProfile();
    }
    public void DiscardPartials(string viewId) { var next = Receiver.Fork(); next.DiscardPartials(viewId); Append(JsonSerializer.SerializeToUtf8Bytes(new { view_keys = new Dictionary<string, JsonElement>(), data = (object?)null, drop_partials = viewId })); Receiver = next; SaveProfile(); }
    public void ClearCursor(string target) { Append(JsonSerializer.SerializeToUtf8Bytes(new { target, view_keys = new Dictionary<string, JsonElement>(), data = (object?)null, drop_partials = (string?)null })); pendingCursors.Remove(target); SaveProfile(); }
    public ulong NextSequence() { Sequence = checked(Sequence + 1); SaveProfile(); return Sequence; }
}
