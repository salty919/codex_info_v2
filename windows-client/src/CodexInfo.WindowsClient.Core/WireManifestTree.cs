// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Buffers.Binary;
using System.Security.Cryptography;
namespace CodexInfo.WindowsClient.Core;
/// <summary>Persistent canonical radix tree. A normal root read is constant time.</summary>
public sealed class WireManifestTree
{
    sealed record Node(byte[] Key, byte[]? Value, int Bit, Node? Left, Node? Right, byte[] Hash);
    Node? root; public int LastVisits { get; private set; }
    public byte[] Root => root?.Hash ?? SHA256.HashData([2]);
    public WireManifestTree Fork() => new() { root = root };
    static bool Bit(byte[] key, int index) => (key[index / 8] & (0x80 >> (index % 8))) != 0;
    static int? Difference(byte[] left, byte[] right) { for (var i = 0; i < Math.Min(left.Length, right.Length); i++) { var xor = left[i] ^ right[i]; if (xor != 0) { var bit = 0; for (var mask = 128; (xor & mask) == 0; mask >>= 1) bit++; return i * 8 + bit; } } if (left.Length != right.Length) throw new InvalidDataException("manifest key is a proper prefix"); return null; }
    static Node Leaf(byte[] key, byte[] value) { var bytes = new byte[1 + 8 + key.Length + 32]; BinaryPrimitives.WriteUInt64BigEndian(bytes.AsSpan(1), (ulong)key.Length); key.CopyTo(bytes, 9); value.CopyTo(bytes, 9 + key.Length); return new(key, value, -1, null, null, SHA256.HashData(bytes)); }
    static Node Branch(int bit, Node left, Node right) { var bytes = new byte[65]; bytes[0] = 1; left.Hash.CopyTo(bytes, 1); right.Hash.CopyTo(bytes, 33); return new(left.Key, null, bit, left, right, SHA256.HashData(bytes)); }
    public void Set(byte[] key, byte[] value) { LastVisits = 0; root = Put(root, key, value); }
    Node Put(Node? node, byte[] key, byte[] value)
    {
        LastVisits++; if (node is null) return Leaf(key, value); var split = Difference(node.Key, key);
        if (split is int index && (node.Left is null || node.Bit > index)) { var leaf = Leaf(key, value); return Bit(key, index) ? Branch(index, node, leaf) : Branch(index, leaf, node); }
        if (node.Left is null) return node.Value!.AsSpan().SequenceEqual(value) ? node : Leaf(node.Key, value);
        return Bit(key, node.Bit) ? Branch(node.Bit, node.Left, Put(node.Right, key, value)) : Branch(node.Bit, Put(node.Left, key, value), node.Right!);
    }
    public void Remove(byte[] key) { LastVisits = 0; root = Remove(root ?? throw new InvalidDataException("missing manifest key"), key); }
    Node? Remove(Node node, byte[] key)
    {
        LastVisits++; if (node.Left is null) { if (!node.Key.AsSpan().SequenceEqual(key)) throw new InvalidDataException("missing manifest key"); return null; }
        if (node.Bit >= key.Length * 8) throw new InvalidDataException("missing manifest key");
        if (Bit(key, node.Bit)) { var right = Remove(node.Right!, key); return right is null ? node.Left : Branch(node.Bit, node.Left, right); }
        var left = Remove(node.Left, key); return left is null ? node.Right : Branch(node.Bit, left, node.Right!);
    }
    public void ReplacePrefix(byte[] prefix, WireManifestTree replacement) { LastVisits = 0; root = Splice(root, prefix, replacement.root); }
    Node? Splice(Node? old, byte[] prefix, Node? replacement)
    {
        LastVisits++; if (old is null) return replacement; int? shared = null; for (var i = 0; i < Math.Min(old.Key.Length, prefix.Length); i++) { var xor = old.Key[i] ^ prefix[i]; if (xor == 0) continue; var bit = 0; for (var mask = 128; (xor & mask) == 0; mask >>= 1) bit++; shared = i * 8 + bit; break; }
        var index = old.Left is null ? int.MaxValue : old.Bit;
        if ((shared is null || shared >= index) && index < prefix.Length * 8) { if (Bit(prefix, index)) { var right = Splice(old.Right, prefix, replacement); return right is null ? old.Left : Branch(index, old.Left!, right); } var left = Splice(old.Left, prefix, replacement); return left is null ? old.Right : Branch(index, left, old.Right!); }
        if (old.Key.AsSpan().StartsWith(prefix)) return replacement; if (replacement is null) return old; var split = Difference(old.Key, replacement.Key) ?? throw new InvalidDataException("namespace collision"); return Bit(replacement.Key, split) ? Branch(split, old, replacement) : Branch(split, replacement, old);
    }

}
