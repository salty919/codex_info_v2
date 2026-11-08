// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
using System.Net.Http;
using System.Net.Sockets;
using System.Text.Json;

namespace CodexInfo.WindowsClient.Core;

/// <summary>Explicit, bounded E2E diagnostics. No payload, selector, or exception text.</summary>
internal static class WireTrace
{
    static readonly object Gate = new();
    internal static void Record(string stage, Exception? failure = null, string? state = null)
    {
        var path = Environment.GetEnvironmentVariable("CODEX_INFO_WIRE_TRACE_PATH");
        if (string.IsNullOrEmpty(path)) return;
        try
        {
            var socket = failure?.GetBaseException() as SocketException;
            var line = JsonSerializer.Serialize(new
            {
                utc = DateTimeOffset.UtcNow,
                stage,
                state,
                failure = failure?.GetType().Name,
                socket_error = socket?.SocketErrorCode.ToString(),
                http_error = (failure as HttpRequestException)?.HttpRequestError.ToString(),
                http_status = (int?)(failure as HttpRequestException)?.StatusCode,
            });
            lock (Gate)
            {
                if (File.Exists(path) && new FileInfo(path).Length >= 1024 * 1024) return;
                File.AppendAllText(path, line + "\n");
            }
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException)
        {
            // Diagnostics never alter receiver, transport, or UI outcomes.
        }
    }
}
