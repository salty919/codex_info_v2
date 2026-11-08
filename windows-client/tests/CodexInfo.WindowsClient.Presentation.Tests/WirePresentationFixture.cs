// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Text.Json;
using System.Text.Json.Nodes;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Core.Tests;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

// Preserve the historical renderer oracle documents as input facts, while
// exercising the current resource decoder. No legacy HTTP route is served.
internal static class WirePresentationFixture
{
    internal static JsonElement MainRecord(JsonElement input)
    {
        var value = JsonNode.Parse(input.GetRawText())!.AsObject();
        value.Remove("api_version");
        value["acquisition_state"] = value["state"]?.DeepClone() ?? "ready";
        value.Remove("state");
        value["models_status"] ??= "complete";
        value["selection"] = JsonSerializer.SerializeToNode(new { account_id = "account-1", login_id = (string?)null, is_current = true });
        var running = value["active_thread_count"]?.GetValue<ulong>() ?? 0;
        var opened = value["open_session_thread_count"]?.GetValue<ulong>() ?? running;
        value["thread_summary"] = JsonSerializer.SerializeToNode(new { open_count = opened, running_count = running, unknown_activity_count = 0ul, by_model = opened == 0 ? Array.Empty<object>() : new object[] { new { model = (string?)null, open_count = opened, running_count = running, unknown_activity_count = 0ul } } });
        foreach (var model in value["models"]!.AsArray())
        {
            var m = model!.AsObject();
            if (m.ContainsKey("name"))
            {
                m["model"] = m["name"]!.DeepClone(); m.Remove("name");
                var ordinary = m["input_tokens"]!.GetValue<ulong>();
                var cached = m["cached_input_tokens"]!.GetValue<ulong>();
                var output = m["output_tokens"]!.GetValue<ulong>();
                m["input_tokens"] = ordinary + cached; m["total_tokens"] = ordinary + cached + output;
                m["cache_write_input_tokens"] = null;
                var inputCost = m["input_dollars"]!.GetValue<double>();
                var cacheCost = m["cached_input_dollars"]!.GetValue<double>();
                var outputCost = m["output_dollars"]!.GetValue<double>();
                m["estimated_cost"] = JsonSerializer.SerializeToNode(new { price_revision = "fixture-saved-price", ordinary_input_dollars = inputCost, cached_input_dollars = cacheCost, cache_write_input_dollars = 0.0, output_dollars = outputCost, total_dollars = inputCost + cacheCost + outputCost });
                m.Remove("input_dollars"); m.Remove("cached_input_dollars"); m.Remove("output_dollars");
            }
            if (m["estimated_cost"] is JsonObject cost && cost.ContainsKey("price_version"))
            { cost["price_revision"] = cost["price_version"]!.DeepClone(); cost.Remove("price_version"); }
        }
        foreach (var name in new[] { "active_thread_count", "open_session_thread_count", "history_periods", "history_samples", "history_gaps", "threads", "estimated_cost_label" }) value.Remove(name);
        return JsonSerializer.SerializeToElement(value);
    }
    internal static IReadOnlyList<FixtureRow> Periods(JsonElement document) => document.GetProperty("history_periods").EnumerateArray().Select(p =>
    {
        var value = JsonNode.Parse(p.GetRawText())!.AsObject(); value["origin_at"] ??= null;
        return new FixtureRow(["period", p.GetProperty("id").GetString()!], "period", JsonSerializer.SerializeToElement(value));
    }).ToArray();
    internal static IReadOnlyList<FixtureRow> History(JsonElement document, string period)
    {
        var rows = new List<FixtureRow>();
        foreach (var sample in document.GetProperty("history_samples").EnumerateArray())
        {
            var reset = sample.GetProperty("reset_at").GetInt64(); var timestamp = sample.GetProperty("timestamp").GetInt64();
            if (document.TryGetProperty("history_periods", out var directory))
            {
                var selected = directory.EnumerateArray().Single(p => p.GetProperty("id").GetString() == period);
                if (reset != selected.GetProperty("reset_at").GetInt64() || timestamp < selected.GetProperty("start_at").GetInt64() || timestamp > selected.GetProperty("end_at").GetInt64()) continue;
            }
            var value = JsonNode.Parse(sample.GetRawText())!.AsObject(); value.Remove("reset_at"); value.Remove("timestamp");
            if (!value.ContainsKey("models"))
            {
                var models = new JsonArray();
                foreach (var family in new[] { "SOL", "TERRA", "LUNA" })
                {
                    var key = family.ToLowerInvariant();
                    if (value[key + "_tokens"] is not null || value[key + "_dollars"] is not null)
                        models.Add(new JsonObject { ["model"] = family, ["total_tokens"] = value[key + "_tokens"]?.DeepClone(), ["total_dollars"] = value[key + "_dollars"]?.DeepClone() });
                    value.Remove(key + "_tokens"); value.Remove(key + "_dollars");
                }
                value["models"] = models;
                value["model_source"] = "legacy-unknown";
            }
            value["model_source"] ??= "legacy-unknown"; value["models_complete"] ??= false;
            if (!value.ContainsKey("task_active_since_previous")) value["task_active_since_previous"] = null;
            rows.Add(new(["sample", period, reset, timestamp], "sample", JsonSerializer.SerializeToElement(value)));
        }
        if (document.TryGetProperty("history_gaps", out var gaps)) foreach (var gap in gaps.EnumerateArray())
        {
            var value = JsonNode.Parse(gap.GetRawText())!.AsObject();
            var id = value["gap_id"]?.GetValue<string>() ?? value["id"]!.GetValue<string>();
            value.Remove("gap_id"); value.Remove("id");
            rows.Add(new(["gap", period, id], "gap", JsonSerializer.SerializeToElement(value)));
        }
        return rows;
    }
    internal static async Task<DetailsFetchResult> Details(LoopbackStatusClient client, JsonElement source)
    {
        var current = await client.FetchCurrentAsync("account-1"); Assert.True(current.IsSuccess);
        var periods = await client.FetchHistoryPeriodsAsync("account-1"); Assert.True(periods.IsSuccess);
        var samples = new List<ApiHistorySample>();
        var populated = new List<ApiHistoryPeriod>();
        foreach (var period in periods.Snapshot!.Periods)
        {
            var history = await client.FetchHistoryPageAsync(accountId: "account-1", periodId: period.Id); Assert.True(history.IsSuccess);
            var accepted = history.Page!.Samples.Select(LegacyColumns).ToArray();
            samples.AddRange(accepted); populated.Add(period with { Samples = accepted });
        }
        var threads = source.TryGetProperty("threads", out var original) && original.GetArrayLength() > 0 ? await client.FetchThreadsAsync("account-1") : null;
        if (threads is not null) Assert.True(threads.IsSuccess);
        var c = current.Snapshot!;
        if (source.TryGetProperty("history_samples", out var originalSamples))
        {
            var order = originalSamples.EnumerateArray().Select((s, i) => (Key: (s.GetProperty("reset_at").GetInt64(), s.GetProperty("timestamp").GetInt64()), i)).ToDictionary(s => s.Key, s => s.i);
            samples = samples.OrderBy(s => order[(s.ResetAt, s.Timestamp)]).ToList();
        }
        return DetailsFetchResult.Success(new ApiDetailsSnapshot(c.State, c.ObservedAt, c.Authenticated, c.PlanLabel, c.Quota, c.Models, c.ActiveThreadCount, populated, samples, threads?.Snapshot?.Threads ?? [], source.TryGetProperty("estimated_cost_label", out var label) ? label.GetString()! : "") { ApiVersion = "", ModelsStatus = c.ModelsStatus, PublishedPair = c.PublishedPair, UseStoredPrices = false, ThreadSummary = c.ThreadSummary });
    }
    static ApiHistorySample LegacyColumns(ApiHistorySample sample)
    {
        ApiHistoryModelSample? Model(string name) => sample.ModelSamples?.SingleOrDefault(m => m.Name == name);
        return sample with { SolDollars = Model("SOL")?.TotalDollars, TerraDollars = Model("TERRA")?.TotalDollars, LunaDollars = Model("LUNA")?.TotalDollars, SolTokens = Model("SOL")?.TotalTokens, TerraTokens = Model("TERRA")?.TotalTokens, LunaTokens = Model("LUNA")?.TotalTokens };
    }
    internal static IReadOnlyList<FixtureRow> Threads(JsonElement document) => document.GetProperty("threads").EnumerateArray().Select(t =>
    {
        var value = JsonNode.Parse(t.GetRawText())!.AsObject(); var id = value["id"]!.GetValue<string>(); value.Remove("id");
        value["activity_status"] ??= "unknown";
        return new FixtureRow(["thread", id], "thread", JsonSerializer.SerializeToElement(value));
    }).ToArray();
}
