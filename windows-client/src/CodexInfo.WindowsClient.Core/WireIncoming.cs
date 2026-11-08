// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
namespace CodexInfo.WindowsClient.Core;
/// <summary>Durable received bytes, including a JSON record cut by the transport.</summary>
sealed class WireIncoming
{
    internal sealed record Header(string Target, string Delivery, string Consumer, ulong Length, string Hash);
    public Header Metadata { get; }
    public ulong Offset { get; private set; }
    public string Prefix { get; }
    readonly string path;
    WireIncoming(Header header, string path, ulong offset, string prefix) { Metadata = header; this.path = path; Offset = offset; Prefix = prefix; }
    static T Io<T>(Func<T> action) { try { return action(); } catch (WireCacheRecoveryException) { throw; } catch (Exception error) when (error is IOException or UnauthorizedAccessException or JsonException) { throw new WireCacheRecoveryException("cache_io", error); } }
    static string PathFor(string directory, string target) => Path.Combine(directory, "incoming-" + Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(target))));
    public static WireIncoming? Load(string directory, string target) => Io(() =>
    {
        var path = PathFor(directory, target); if (!File.Exists(path + ".json")) return null; var header = JsonSerializer.Deserialize<Header>(File.ReadAllBytes(path + ".json")) ?? throw new WireCacheRecoveryException("cache_invalid"); if (header.Target != target || header.Length > 33554432) throw new WireCacheRecoveryException("cache_invalid"); var bytes = File.ReadAllBytes(path); if ((ulong)bytes.Length > header.Length) throw new WireCacheRecoveryException("cache_invalid"); return new WireIncoming(header, path, (ulong)bytes.Length, Convert.ToHexStringLower(SHA256.HashData(bytes)));
    });
    public static WireIncoming Begin(string directory, Header header) => Io(() =>
    {
        if (header.Length > 33554432 || header.Hash.Length != 64 || header.Hash.Any(ch => !char.IsAsciiHexDigitLower(ch)) || string.IsNullOrEmpty(header.Delivery) || string.IsNullOrEmpty(header.Consumer)) throw new WireCacheRecoveryException("cache_invalid");
        var current = Load(directory, header.Target); if (current is not null) { if (current.Metadata != header) throw new WireCacheRecoveryException("cache_invalid"); return current; }
        var path = PathFor(directory, header.Target); using (var output = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read)) { output.Flush(true); }
        using (var output = new FileStream(path + ".json", FileMode.CreateNew, FileAccess.Write, FileShare.Read)) { output.Write(JsonSerializer.SerializeToUtf8Bytes(header)); output.Flush(true); }
        return new WireIncoming(header, path, 0, Convert.ToHexStringLower(SHA256.HashData([])));
    });
    public void Append(byte[] bytes, int count) => Io(() => { if (count < 0 || count > bytes.Length || (ulong)count > Metadata.Length - Offset) throw new WireCacheRecoveryException("cache_invalid"); using var output = new FileStream(path, FileMode.Append, FileAccess.Write, FileShare.Read); if ((ulong)output.Length != Offset) throw new WireCacheRecoveryException("cache_invalid"); output.Write(bytes, 0, count); output.Flush(true); Offset += (ulong)count; return true; });
    public byte[] Bytes() => Io(() => { var bytes = File.ReadAllBytes(path); if ((ulong)bytes.Length != Metadata.Length || Convert.ToHexStringLower(SHA256.HashData(bytes)) != Metadata.Hash) throw new WireCacheRecoveryException("cache_invalid"); return bytes; });
    public static object[] Receipts(string directory) => Io(() => Directory.EnumerateFiles(directory, "incoming-*.json").Select(path => JsonSerializer.Deserialize<Header>(File.ReadAllBytes(path)) ?? throw new WireCacheRecoveryException("cache_invalid")).Select(header => { var incoming = Load(directory, header.Target) ?? throw new WireCacheRecoveryException("cache_invalid"); return (object)new { delivery_id = header.Delivery, offset = incoming.Offset, prefix_hash = incoming.Prefix }; }).ToArray());
    public static string[] Targets(string directory) => Io(() => Directory.EnumerateFiles(directory, "incoming-*.json").Select(path => (JsonSerializer.Deserialize<Header>(File.ReadAllBytes(path)) ?? throw new WireCacheRecoveryException("cache_invalid")).Target).ToArray());
    public void Finish() => Io(() => { File.Delete(path + ".json"); File.Delete(path); return true; });
}
