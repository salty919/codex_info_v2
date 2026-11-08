using System.Text.Json;
using CodexInfo.WindowsClient.Core;
using Xunit;
namespace CodexInfo.WindowsClient.Core.Tests;

public sealed class WireReceiverTests
{
    [Fact]
    public void RustDefinitionsAndChangedFieldPatchHaveIdenticalReceiptsAndExactValues()
    {
        using var fixture = JsonDocument.Parse(File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "receiver_interop.json"))); var receiver = new WireReceiver(); var pages = fixture.RootElement.GetProperty("pages").EnumerateArray().ToArray();
        foreach (var page in pages)
        {
            var keys = page.GetProperty("view_keys").EnumerateObject().ToDictionary(p => p.Name, p => p.Value.Clone()); receiver.Apply(page.GetProperty("data"), keys); Assert.Equal(page.GetProperty("root").GetString(), receiver.Root);
            var main = receiver.Resolve(page.GetProperty("main_id").GetUInt64()); Assert.Equal(ulong.MaxValue, main.GetProperty("models")[0].GetProperty("total_tokens").GetUInt64()); Assert.Equal(BitConverter.DoubleToInt64Bits(-0.0), BitConverter.DoubleToInt64Bits(main.GetProperty("models")[0].GetProperty("estimated_cost").GetProperty("ordinary_input_dollars").GetDouble()));
        }
        Assert.Equal(101, receiver.Resolve(pages[^1].GetProperty("main_id").GetUInt64()).GetProperty("observed_at").GetInt64()); Assert.Single(pages[1].GetProperty("data").GetProperty("patches").EnumerateArray());
        var before = receiver.Root; Assert.Throws<InvalidDataException>(() => receiver.Apply(pages[1].GetProperty("data"))); Assert.Equal(before, receiver.Root);
    }
    [Fact]
    public void LocalJournalWriteFailureIsCacheRecoveryAndDoesNotAdvanceReceiver()
    {
        var path = Path.Combine(Path.GetTempPath(), "codex-api-690-cache-io-" + Guid.NewGuid().ToString("N"));
        try { var cache = new WireCache(path); cache.Issue("consumer-io"); var root = cache.Receiver.Root; File.Delete(Path.Combine(path, "receipts")); var error = Assert.Throws<WireCacheRecoveryException>(() => cache.DiscardPartials("view-a")); Assert.Equal("cache_io", error.Reason); Assert.IsType<FileNotFoundException>(error.InnerException); Assert.Equal(root, cache.Receiver.Root); }
        finally { if (Directory.Exists(path)) Directory.Delete(path, true); File.Delete(Path.ChangeExtension(path, "identity")); }
    }
    [Fact]
    public void MissingEntireCacheUsesDurableIdentityAnchorAndCannotBootstrapAgain()
    {
        var path = Path.Combine(Path.GetTempPath(), "codex-api-690-cache-loss-" + Guid.NewGuid().ToString("N")); var held = path + "-held";
        try { var cache = new WireCache(path); cache.Issue("original-consumer"); Directory.Move(path, held); var error = Assert.Throws<WireCacheRecoveryException>(() => new WireCache(path)); Assert.Equal("cache_missing", error.Reason); Assert.False(Directory.Exists(path)); }
        finally { if (Directory.Exists(held)) Directory.Delete(held, true); if (Directory.Exists(path)) Directory.Delete(path, true); File.Delete(Path.ChangeExtension(path, "identity")); }
    }
}
