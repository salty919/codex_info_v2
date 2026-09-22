// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using CodexInfo.WindowsClient.Localization;
using Xunit;

namespace CodexInfo.WindowsClient.Presentation.Tests;

public sealed class LegalNoticeCatalogTests
{
    [Fact]
    public void MarkdownProjectionUsesTheFiniteLegalNoticePlainTextOracle()
    {
        const string source = "<!--\nCopyright (C) 2026 salty919\nSPDX-License-Identifier: GPL-3.0-only\n-->\n"
            + "# Heading\n"
            + "## Details ##\n"
            + "Paragraph [Label](https://example.test/docs) [Relative](../LICENSE) "
            + "[https://example.test/docs](https://example.test/docs) "
            + "<https://example.test/autolink> `inline` **bold** _italic_ ~~strike~~ GPL-3.0-only\n"
            + "- unordered\n"
            + "7) ordered\n"
            + "---\n"
            + "```csharp\n"
            + "code body\n"
            + "```\n"
            + "After";

        var actual = LegalNoticeCatalog.ProjectMarkdownToPlainText(source);

        const string expected = "\nCopyright (C) 2026 salty919\n"
            + "SPDX-License-Identifier: GPL-3.0-only\n"
            + "\nHeading\n"
            + "Details\n"
            + "Paragraph Label (https://example.test/docs) Relative (../LICENSE) https://example.test/docs "
            + "https://example.test/autolink inline bold italic strike GPL-3.0-only\n"
            + "• unordered\n"
            + "7. ordered\n"
            + "\n"
            + "code body\n"
            + "After";

        Assert.Equal(expected, actual);
        Assert.DoesNotContain("#", actual, StringComparison.Ordinal);
        Assert.DoesNotContain("`", actual, StringComparison.Ordinal);
        Assert.DoesNotContain("[Label](https://example.test/docs)", actual, StringComparison.Ordinal);
        Assert.DoesNotContain("<!--", actual, StringComparison.Ordinal);
        Assert.DoesNotContain("-->", actual, StringComparison.Ordinal);
        Assert.DoesNotContain("```", actual, StringComparison.Ordinal);
        Assert.Contains("GPL-3.0-only", actual, StringComparison.Ordinal);
    }

    [Fact]
    public void NonMarkdownResourcesRemainExactPassThrough()
    {
        const string source = "# not projected\r\n`still exact`\r\nGPL-3.0-only";

        Assert.Equal(source, LegalNoticeCatalog.ProjectForDisplay("Legal/LICENSE", source));
        Assert.Equal(source, LegalNoticeCatalog.ProjectForDisplay("Legal/LICENSES/NOTICE.txt", source));

        var license = LoadRepositoryFile("LICENSE");
        var ofl = LoadRepositoryFile("LICENSES", "OFL-1.1.txt");
        Assert.Equal(license, LegalNoticeCatalog.ProjectForDisplay("Legal/LICENSE", license));
        Assert.Equal(ofl, LegalNoticeCatalog.ProjectForDisplay("Legal/LICENSES/OFL-1.1.txt", ofl));
    }

    [Fact]
    public void MalformedMarkdownFailsWithTheExistingLoadExceptionType()
    {
        Assert.Throws<InvalidDataException>(() => LegalNoticeCatalog.ProjectMarkdownToPlainText("```csharp\nbody"));
        Assert.Throws<InvalidDataException>(() => LegalNoticeCatalog.ProjectMarkdownToPlainText("<!-- comment"));
        Assert.Throws<InvalidDataException>(() => LegalNoticeCatalog.ProjectMarkdownToPlainText("[label](https://example.test"));
        Assert.Throws<InvalidDataException>(() => LegalNoticeCatalog.ProjectMarkdownToPlainText("`code"));
    }

    [Fact]
    public void FencedCodePreservesHtmlCommentDelimitersWhileOutsideProjectionRemovesThem()
    {
        const string source = "Before <!--outside-->\n"
            + "```text\n"
            + "<!-- code marker -->\n"
            + "# code body\n"
            + "```\n"
            + "After <!--outside-->";

        var actual = LegalNoticeCatalog.ProjectMarkdownToPlainText(source);

        Assert.Equal("Before outside\n<!-- code marker -->\n# code body\nAfter outside", actual);
    }

    [Fact]
    public void MalformedInjectedMarkdownUsesTheExistingFailClosedLoadPage()
    {
        var texts = LocalizationService.Current;
        var notices = LegalNoticeCatalog.LoadForTest(texts, resource =>
            resource.Equals("Legal/LICENSE.ja.md", StringComparison.Ordinal)
                ? "<!-- malformed"
                : "placeholder");

        var notice = Assert.Single(notices);
        Assert.Equal(texts.LegalDetailsName, notice.Name);
        Assert.Contains(
            texts.LanguageCode == "ja" ? "再インストール" : "reinstalled",
            notice.Text,
            StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void PackagedLegalPagesKeepNavigationCountAndImportantText()
    {
        var notices = LegalNoticeCatalog.Load(LocalizationService.Current);

        Assert.Equal(9, notices.Count);
        Assert.Contains(notices, notice => notice.Text.Contains("GNU GENERAL PUBLIC LICENSE", StringComparison.Ordinal));
        Assert.Contains(notices, notice => notice.Text.Contains("NO WARRANTY", StringComparison.Ordinal));
        Assert.Contains(notices, notice => notice.Text.Contains("SIL OPEN FONT LICENSE", StringComparison.Ordinal));
        Assert.Contains(notices, notice => notice.Text.Contains("Apache License", StringComparison.Ordinal));
        Assert.Contains(notices, notice => notice.Text.Contains("MIT License", StringComparison.Ordinal));
        Assert.Contains(notices, notice => notice.Text.Contains("Inno Setup License", StringComparison.Ordinal));

        for (var noticeIndex = 0; noticeIndex < notices.Count; noticeIndex++)
        {
            var notice = notices[noticeIndex];
            var scanMarkdown = noticeIndex is not (1 or 2 or 5);
            foreach (var line in notice.Text.Split('\n'))
            {
                if (line.StartsWith('\uff3b') && line.EndsWith('\uff3d'))
                {
                    scanMarkdown = line.EndsWith(".md\uff3d", StringComparison.OrdinalIgnoreCase);
                    continue;
                }

                if (!scanMarkdown)
                {
                    continue;
                }

                Assert.DoesNotContain("<!--", line, StringComparison.Ordinal);
                Assert.DoesNotContain("-->", line, StringComparison.Ordinal);
                Assert.DoesNotContain("```", line, StringComparison.Ordinal);
                Assert.DoesNotContain("](", line, StringComparison.Ordinal);
                Assert.False(line.TrimStart().StartsWith('#'), notice.Name);
            }
        }
    }

    [Fact]
    public void EveryPackagedMarkdownUrlAndCodeBodySurvivesProjection()
    {
        var checkedDestinationCount = 0;
        var checkedCodeBlockCount = 0;
        var resources = new[]
        {
            ("Legal/LICENSE.ja.md", LoadRepositoryFile("LICENSE.ja.md")),
            ("Legal/THIRD_PARTY_NOTICES.md", LoadRepositoryFile("THIRD_PARTY_NOTICES.md")),
            ("Legal/Windows-THIRD_PARTY_NOTICES.md", LoadRepositoryFile("windows-client", "THIRD_PARTY_NOTICES.md")),
        };

        foreach (var (resource, source) in resources)
        {
            var projected = LegalNoticeCatalog.ProjectForDisplay(resource, source);
            foreach (var destination in ExtractMarkdownLinkDestinations(source).Distinct(StringComparer.Ordinal))
            {
                checkedDestinationCount++;
                Assert.Contains(destination, projected, StringComparison.Ordinal);
            }

            foreach (var body in ExtractFencedCodeBodies(source))
            {
                checkedCodeBlockCount++;
                if (body.Length > 0)
                {
                    Assert.Contains(body, projected, StringComparison.Ordinal);
                }
            }
        }
        Assert.True(checkedDestinationCount > 0);
        Assert.True(checkedCodeBlockCount > 0);

        var thirdParty = LegalNoticeCatalog.ProjectForDisplay(
            "Legal/THIRD_PARTY_NOTICES.md",
            resources[1].Item2);
        Assert.Contains("Collect-ThirdPartyNotices.ps1", thirdParty, StringComparison.Ordinal);
        Assert.Contains("-Destination", thirdParty, StringComparison.Ordinal);
        Assert.Contains("<publish-directory>", thirdParty, StringComparison.Ordinal);
    }

    private static IEnumerable<string> ExtractMarkdownLinkDestinations(string source)
    {
        var index = 0;
        while (index < source.Length)
        {
            if (source[index] != '[')
            {
                index++;
                continue;
            }

            var labelEnd = source.IndexOf(']', index + 1);
            if (labelEnd < 0 || labelEnd + 1 >= source.Length || source[labelEnd + 1] != '(')
            {
                index++;
                continue;
            }

            var destinationStart = labelEnd + 2;
            var parenthesisDepth = 0;
            var nextIndex = index + 1;
            for (var destinationEnd = destinationStart; destinationEnd < source.Length; destinationEnd++)
            {
                if (source[destinationEnd] == '(')
                {
                    parenthesisDepth++;
                }
                else if (source[destinationEnd] == ')')
                {
                    if (parenthesisDepth == 0)
                    {
                        yield return source[destinationStart..destinationEnd];
                        nextIndex = destinationEnd + 1;
                        break;
                    }

                    parenthesisDepth--;
                }
            }

            index = nextIndex;
        }
    }

    private static IEnumerable<string> ExtractFencedCodeBodies(string source)
    {
        var lines = source.Replace("\r\n", "\n", StringComparison.Ordinal).Replace('\r', '\n').Split('\n');
        int? fenceLength = null;
        var body = new List<string>();
        foreach (var line in lines)
        {
            if (fenceLength is null)
            {
                if (TryGetFenceLength(line, out var startLength))
                {
                    fenceLength = startLength;
                    body.Clear();
                }

                continue;
            }

            if (IsFenceEnd(line, fenceLength.Value))
            {
                yield return string.Join('\n', body);
                fenceLength = null;
                body.Clear();
                continue;
            }

            body.Add(line);
        }

        Assert.Null(fenceLength);
    }

    private static bool TryGetFenceLength(string line, out int length)
    {
        var start = 0;
        while (start < line.Length && start < 3 && (line[start] == ' ' || line[start] == '\t'))
        {
            start++;
        }

        var end = start;
        while (end < line.Length && line[end] == '`')
        {
            end++;
        }

        length = end - start;
        return length >= 3;
    }

    private static bool IsFenceEnd(string line, int fenceLength)
    {
        if (!TryGetFenceLength(line, out var length) || length < fenceLength)
        {
            return false;
        }

        var markerEnd = 0;
        while (markerEnd < line.Length && markerEnd < 3 && (line[markerEnd] == ' ' || line[markerEnd] == '\t'))
        {
            markerEnd++;
        }

        markerEnd += length;
        return line[markerEnd..].Trim(' ', '\t').Length == 0;
    }

    private static string LoadRepositoryFile(params string[] segments)
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
        {
            if (!File.Exists(Path.Combine(directory.FullName, "Cargo.toml"))
                || !Directory.Exists(Path.Combine(directory.FullName, "windows-client")))
            {
                continue;
            }

            var candidate = Path.Combine([directory.FullName, .. segments]);
            if (File.Exists(candidate))
            {
                return File.ReadAllText(candidate);
            }
        }

        throw new FileNotFoundException($"Could not locate repository file: {Path.Combine(segments)}");
    }
}
