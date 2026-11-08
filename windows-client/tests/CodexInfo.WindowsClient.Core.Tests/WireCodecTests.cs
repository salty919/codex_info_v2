using System.Text;
using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using Xunit;
namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class WireCodecTests
{
    [Fact]
    public void TypedValuesMatchIndependentByteOracleIncludingUInt64MoneyZerosAndReferences()
    {
        using var fixture = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "wire_vectors.json")));
        foreach (var vector in fixture.RootElement.GetProperty("values").EnumerateArray())
        {
            var kind = vector.GetProperty("kind").GetString()!; var value = vector.GetProperty("value").Clone();
            Dictionary<ulong, WireDefinition> definitions = [];
            if (kind == "f64") value = JsonSerializer.SerializeToElement(BitConverter.Int64BitsToDouble(unchecked((long)Convert.ToUInt64(value.GetString(), 16))));
            if (kind == "ref") definitions[7] = new(7, "f64", JsonSerializer.SerializeToElement(1.0));
            Assert.Equal(vector.GetProperty("digest").GetString(), Convert.ToHexStringLower(WireCodec.Digest(kind is "object" or "ref" ? null : kind, value, definitions)));
            if (vector.TryGetProperty("canonical", out var canonical)) Assert.Equal(canonical.GetString(), Encoding.UTF8.GetString(WireCodec.Canonical(kind, value)));
        }
    }
    [Fact]
    public void ManifestMatchesIndependentEmptyAndSortedTreeOracle()
    {
        using var fixture = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "wire_vectors.json"))); var manifest = fixture.RootElement.GetProperty("manifest");
        Assert.Equal(manifest.GetProperty("empty").GetString(), Convert.ToHexStringLower(WireCodec.Manifest([])));
        var entries = manifest.GetProperty("entries").EnumerateArray().Select(e => (Encoding.UTF8.GetBytes(e.GetProperty("key").GetString()!), Convert.FromHexString(e.GetProperty("value_digest").GetString()!))).ToArray();
        Assert.Equal(manifest.GetProperty("one_leaf").GetString(), Convert.ToHexStringLower(WireCodec.Manifest(entries.Take(1))));
        Assert.Equal(manifest.GetProperty("two_leaves").GetString(), Convert.ToHexStringLower(WireCodec.Manifest(entries.Reverse())));
    }
}
