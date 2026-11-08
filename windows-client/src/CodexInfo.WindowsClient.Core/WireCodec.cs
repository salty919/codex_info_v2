// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Buffers.Binary;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
namespace CodexInfo.WindowsClient.Core;

public sealed record WireDefinition(ulong Id, string Kind, JsonElement Value);
public static class WireCodec
{
    public static byte[] Digest(string? kind, JsonElement value, IReadOnlyDictionary<ulong, WireDefinition>? definitions = null, IReadOnlyDictionary<ulong, byte[]>? known = null) => HashValue(kind, value, definitions ?? new Dictionary<ulong, WireDefinition>(), known ?? new Dictionary<ulong, byte[]>(), [], 0);
    static byte[] HashValue(string? kind, JsonElement value, IReadOnlyDictionary<ulong, WireDefinition> definitions, IReadOnlyDictionary<ulong, byte[]> known, HashSet<ulong> visiting, int depth)
    {
        if (depth > 128) throw new InvalidDataException("reference depth exceeded");
        if (value.ValueKind == JsonValueKind.Object && value.TryGetProperty("ref", out var reference))
        {
            if (value.EnumerateObject().Count() != 1 || !reference.TryGetUInt64(out var id) || id == 0 || !visiting.Add(id) || !definitions.TryGetValue(id, out var target)) throw new InvalidDataException("invalid reference");
            if (kind is not null && target.Kind != "null" && !kind.EndsWith("[]", StringComparison.Ordinal) && kind != target.Kind) throw new InvalidDataException("reference kind mismatch");
            if (known.TryGetValue(id, out var cached)) { visiting.Remove(id); return cached; }
            var result = HashValue(target.Kind, target.Value, definitions, known, visiting, depth + 1); visiting.Remove(id); return result;
        }
        if (value.ValueKind == JsonValueKind.Null) return SHA256.HashData([0x10]);
        using var bytes = new MemoryStream();
        if (kind == "f64") { var number = value.GetDouble(); if (!double.IsFinite(number)) throw new InvalidDataException("invalid f64"); bytes.WriteByte(0x15); Write64(bytes, unchecked((ulong)BitConverter.DoubleToInt64Bits(number))); }
        else if (kind == "u64") { bytes.WriteByte(0x13); Write64(bytes, value.GetUInt64()); }
        else if (kind == "i64") { bytes.WriteByte(0x14); Write64(bytes, unchecked((ulong)value.GetInt64())); }
        else switch (value.ValueKind)
            {
                case JsonValueKind.True: bytes.WriteByte(0x12); break;
                case JsonValueKind.False: bytes.WriteByte(0x11); break;
                case JsonValueKind.Number:
                    if (value.TryGetUInt64(out var unsigned)) { bytes.WriteByte(0x13); Write64(bytes, unsigned); }
                    else if (value.TryGetInt64(out var signed)) { bytes.WriteByte(0x14); Write64(bytes, unchecked((ulong)signed)); }
                    else { var number = value.GetDouble(); if (!double.IsFinite(number)) throw new InvalidDataException("invalid f64"); bytes.WriteByte(0x15); Write64(bytes, unchecked((ulong)BitConverter.DoubleToInt64Bits(number))); }
                    break;
                case JsonValueKind.String: var text = Encoding.UTF8.GetBytes(value.GetString()!); bytes.WriteByte(0x16); Write64(bytes, (ulong)text.Length); bytes.Write(text); break;
                case JsonValueKind.Array:
                    bytes.WriteByte(0x17); Write64(bytes, (ulong)value.GetArrayLength()); var elementKind = kind?.EndsWith("[]", StringComparison.Ordinal) == true ? kind[..^2] : null;
                    foreach (var element in value.EnumerateArray()) bytes.Write(HashValue(elementKind, element, definitions, known, visiting, depth + 1)); break;
                case JsonValueKind.Object:
                    var fields = value.EnumerateObject().OrderBy(p => p.Name, Utf8Comparer.Instance).ToArray(); bytes.WriteByte(0x18); Write64(bytes, (ulong)fields.Length);
                    foreach (var field in fields) { var name = Encoding.UTF8.GetBytes(field.Name); Write64(bytes, (ulong)name.Length); bytes.Write(name); bytes.Write(HashValue(FieldKind(kind, field.Name), field.Value, definitions, known, visiting, depth + 1)); }
                    break;
                default: throw new InvalidDataException("invalid value");
            }
        return SHA256.HashData(bytes.GetBuffer().AsSpan(0, checked((int)bytes.Length)));
    }
    public static string? FieldKind(string? kind, string field) => (kind, field) switch
    {
        ("model_cost", "price_revision") => "text",
        ("model_cost", _) => "f64",
        ("quota" or "sample", "remaining_percent") => "f64",
        ("history_model", "total_dollars") => "f64",
        ("main", "selection") => "selection",
        ("main", "quota") => "quota",
        ("main", "models") => "model_usage[]",
        ("main", "thread_summary") => "thread_summary",
        ("model_usage", "estimated_cost") => "model_cost",
        ("sample", "models") => "history_model[]",
        ("account_choices", "accounts") => "account_choice[]",
        ("account_context", "ownership_intervals") => "ownership_interval[]",
        ("local_usage", "intervals") => "local_interval[]",
        ("thread_summary", "by_model") => "thread_model_count[]",
        (_, "observed_at" or "reset_at" or "start_at" or "end_at" or "origin_at" or "activation_at" or "deactivation_at" or "created_at" or "last_user_message_at" or "window_seconds" or "depth") => "i64",
        _ => null
    };
    static void Write64(Stream stream, ulong value) { Span<byte> bytes = stackalloc byte[8]; BinaryPrimitives.WriteUInt64BigEndian(bytes, value); stream.Write(bytes); }
    public static byte[] Canonical(string? kind, JsonElement value)
    {
        var output = new StringBuilder(); Encode(kind, value, output, 0); return Encoding.UTF8.GetBytes(output.ToString());
    }
    static void String(string value, StringBuilder output)
    {
        output.Append('"'); foreach (var ch in value) { if (ch == '"') output.Append("\\\""); else if (ch == '\\') output.Append("\\\\"); else if (ch < ' ') output.Append("\\u").Append(((int)ch).ToString("x4", CultureInfo.InvariantCulture)); else output.Append(ch); }
        output.Append('"');
    }
    static void Encode(string? kind, JsonElement value, StringBuilder output, int depth)
    {
        if (depth > 128) throw new InvalidDataException("value depth exceeded");
        if (value.ValueKind == JsonValueKind.Object && value.TryGetProperty("ref", out var reference)) { if (value.EnumerateObject().Count() != 1) throw new InvalidDataException("reserved ref"); output.Append("{\"ref\":").Append(reference.GetUInt64().ToString(CultureInfo.InvariantCulture)).Append('}'); return; }
        switch (value.ValueKind)
        {
            case JsonValueKind.Null: output.Append("null"); break;
            case JsonValueKind.True: output.Append("true"); break;
            case JsonValueKind.False: output.Append("false"); break;
            case JsonValueKind.Number:
                if (kind == "f64" || !value.TryGetUInt64(out _) && !value.TryGetInt64(out _)) output.Append(Float(value.GetDouble()));
                else if (value.TryGetUInt64(out var u)) output.Append(u.ToString(CultureInfo.InvariantCulture)); else output.Append(value.GetInt64().ToString(CultureInfo.InvariantCulture)); break;
            case JsonValueKind.String: String(value.GetString()!, output); break;
            case JsonValueKind.Array:
                output.Append('['); var first = true; var elementKind = kind?.EndsWith("[]", StringComparison.Ordinal) == true ? kind[..^2] : null;
                foreach (var element in value.EnumerateArray()) { if (!first) output.Append(','); first = false; Encode(elementKind, element, output, depth + 1); }
                output.Append(']'); break;
            case JsonValueKind.Object: output.Append('{'); first = true; foreach (var field in value.EnumerateObject().OrderBy(p => p.Name, Utf8Comparer.Instance)) { if (!first) output.Append(','); first = false; String(field.Name, output); output.Append(':'); Encode(FieldKind(kind, field.Name), field.Value, output, depth + 1); } output.Append('}'); break;
            default: throw new InvalidDataException("invalid JSON value");
        }
    }
    static string Float(double number)
    {
        if (!double.IsFinite(number)) throw new InvalidDataException("nonfinite f64"); if (number == 0) return BitConverter.DoubleToInt64Bits(number) < 0 ? "-0.0" : "0.0";
        var negative = number < 0; var encoded = Math.Abs(number).ToString("R", CultureInfo.InvariantCulture).ToLowerInvariant(); var pieces = encoded.Split('e'); var mantissa = pieces[0]; var exponent = pieces.Length == 2 ? int.Parse(pieces[1], CultureInfo.InvariantCulture) : 0;
        var point = mantissa.IndexOf('.'); var fraction = point < 0 ? 0 : mantissa.Length - point - 1; var digits = mantissa.Replace(".", "", StringComparison.Ordinal).TrimStart('0'); var power = exponent - fraction;
        while (digits.Length > 1 && digits.EndsWith('0')) { digits = digits[..^1]; power++; }
        point = digits.Length + power;
        var plain = point <= 0 ? "0." + new string('0', -point) + digits : point >= digits.Length ? digits + new string('0', point - digits.Length) : digits[..point] + "." + digits[point..];
        var scientific = (digits.Length == 1 ? digits : digits[..1] + "." + digits[1..]) + "e" + (point - 1).ToString(CultureInfo.InvariantCulture); var result = plain.Length <= scientific.Length ? plain : scientific; return negative ? "-" + result : result;
    }
    public static byte[] Manifest(IEnumerable<(byte[] Key, byte[] Digest)> entries)
    {
        var tree = new WireManifestTree(); var keys = new HashSet<string>(StringComparer.Ordinal); foreach (var (key, digest) in entries) { if (!keys.Add(Convert.ToHexStringLower(key))) throw new InvalidDataException("duplicate manifest key"); tree.Set(key, digest); }
        return tree.Root.ToArray();
    }

    sealed class Utf8Comparer : IComparer<string> { internal static readonly Utf8Comparer Instance = new(); public int Compare(string? x, string? y) => Encoding.UTF8.GetBytes(x!).AsSpan().SequenceCompareTo(Encoding.UTF8.GetBytes(y!)); }
    sealed class ByteComparer : IComparer<byte[]> { internal static readonly ByteComparer Instance = new(); public int Compare(byte[]? x, byte[]? y) => x!.AsSpan().SequenceCompareTo(y!); }
}
