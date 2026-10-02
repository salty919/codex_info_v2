# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Read-only dependency inventory, advisory queries and license provenance.

NuGet vulnerability findings belong to the existing locked restore. License
metadata collection is not approval; the Linux Rust allowlist is not reused.
"""

import argparse
import ast
import gzip
import io
import json
import os
import pathlib
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone

import yaml

OSV_BATCH = "https://api.osv.dev/v1/querybatch"
NUGET = "https://api.nuget.org/v3/registration5-gz-semver2"
GITHUB = "https://api.github.com/repos"
PYPI = "https://pypi.org/pypi"
HOSTS = {"api.osv.dev", "api.nuget.org", "api.github.com", "pypi.org"}
LIMIT = 16 * 1024 * 1024
ACTION = re.compile(r"^([\w.-]+/[\w.-]+)(?:/[\w./-]+)?@([\w./-]+)$")
SHA = re.compile(r"^[0-9a-f]{40}$")
PIN = re.compile(r"^([A-Za-z0-9_.-]+)==([A-Za-z0-9_.+-]+) --hash=sha256:([0-9a-f]{64})$")
INLINE_PIN = re.compile(r"defusedxml==[A-Za-z0-9_.+-]+ --hash=sha256:[0-9a-f]{64}")
VERSION = re.compile(r"^v?(\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?)$")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("metadata redirects are not accepted")


class Client:
    def __init__(self):
        self.opener = urllib.request.build_opener(NoRedirect())
        self.cache = {}

    def __call__(self, url, payload=None):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != "https" or parsed.hostname not in HOSTS or parsed.port not in (None, 443):
            raise ValueError("metadata endpoint is outside the fixed HTTPS hosts")
        if parsed.username or parsed.password or parsed.fragment:
            raise ValueError("metadata endpoint has unexpected authority fields")
        key = (url, json.dumps(payload, sort_keys=True))
        if key in self.cache:
            return self.cache[key]
        headers = {"Accept": "application/json", "Accept-Encoding": "identity",
                   "User-Agent": "codex-info-dependency-audit"}
        token = os.environ.get("GITHUB_TOKEN")
        if parsed.hostname == "api.github.com" and token:
            headers["Authorization"] = "Bearer " + token
        data = None
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=data, headers=headers)
        try:
            with self.opener.open(request, timeout=30) as response:
                body = response.read(LIMIT + 1)
                if len(body) > LIMIT:
                    raise ValueError("metadata response exceeds the size limit")
                encoding = response.headers.get("Content-Encoding", "identity")
                if encoding == "gzip":
                    with gzip.GzipFile(fileobj=io.BytesIO(body)) as stream:
                        body = stream.read(LIMIT + 1)
                elif encoding != "identity":
                    raise ValueError("unsupported metadata content encoding")
                if len(body) > LIMIT:
                    raise ValueError("decoded metadata exceeds the size limit")
                result = json.loads(body)
        except urllib.error.HTTPError as error:
            # GitHub uses 404 specifically for a missing root license file.
            if error.code == 404 and parsed.hostname == "api.github.com" and "/license?" in url:
                result = {"license": None, "metadata_status": "absent"}
            else:
                raise
        self.cache[key] = result
        return result


def steps(data):
    jobs = data.get("jobs", {})
    if not isinstance(jobs, dict):
        raise ValueError("workflow jobs are not a mapping")
    for job in jobs.values():
        if not isinstance(job, dict):
            raise ValueError("workflow job is not a mapping")
        if "uses" in job:
            yield job
        for step in job.get("steps", []):
            if not isinstance(step, dict):
                raise ValueError("workflow step is not a mapping")
            yield step


def inventory(root):
    packages = {}
    locks = sorted(root.glob("windows-client/**/packages.lock.json"))
    for path in locks:
        lock = json.loads(path.read_text(encoding="utf-8"))
        if lock.get("version") != 1 or not isinstance(lock.get("dependencies"), dict):
            raise ValueError("unsupported NuGet lock schema: " + str(path.relative_to(root)))
        for target, entries in lock["dependencies"].items():
            if not isinstance(entries, dict):
                raise ValueError("NuGet target graph is incomplete")
            for name, entry in entries.items():
                kind = entry.get("type")
                if kind == "Project":
                    continue
                if kind not in ("Direct", "Transitive") or not entry.get("resolved"):
                    raise ValueError("NuGet dependency lacks a resolved package version")
                key = (name.lower(), entry["resolved"])
                package = packages.setdefault(key, {"ecosystem": "NuGet", "name": name,
                                                      "version": entry["resolved"], "occurrences": []})
                package["occurrences"].append({"path": str(path.relative_to(root)),
                                               "target": target, "type": kind})
    requirements = root / ".github/requirements-native-proof.txt"
    python = []
    declarations = {}
    for line in requirements.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = PIN.fullmatch(line)
        if not match:
            raise ValueError("Python requirement is not a hash-pinned exact version")
        name, version, digest = match.groups()
        declarations[name] = line
        python.append({"ecosystem": "PyPI", "name": name, "version": version,
                       "sha256": digest, "supply_status": "hash_pinned", "inline_suppliers": []})
    actions = {}
    local = []
    for path in sorted((root / ".github/workflows").glob("*.yml")):
        data = yaml.safe_load(path.read_text())
        if not isinstance(data, dict):
            raise ValueError("workflow is not a mapping")
        for step in steps(data):
            for declaration in INLINE_PIN.findall(step.get("run", "")):
                if declaration != declarations.get("defusedxml"):
                    raise ValueError("inline Python dependency differs from the hash-pinned requirement")
                package = next(p for p in python if p["name"] == "defusedxml")
                if "--require-hashes" not in step["run"]:
                    raise ValueError("inline Python supplier does not enforce hashes")
                package["inline_suppliers"].append(str(path.relative_to(root)))
            use = step.get("uses")
            if use is None:
                continue
            if not isinstance(use, str):
                raise ValueError("workflow uses is not a literal string")
            if use.startswith("./"):
                local.append({"path": str(path.relative_to(root)), "uses": use})
                continue
            match = ACTION.fullmatch(use)
            if not match:
                raise ValueError("unsupported external workflow dependency: " + use)
            name, ref = match.groups()
            action = actions.setdefault((name, ref), {"name": name, "ref": ref, "occurrences": []})
            action["occurrences"].append({"path": str(path.relative_to(root)), "uses": use})
    python.append({"ecosystem": "PyPI", "name": "PyYAML", "version": yaml.__version__,
                   "supply_status": "review_required", "supplier": "existing runner Python environment",
                   "reason": "PyYAML is used by existing CI scripts but has no repository hash lock"})
    external = set()
    files = [*root.glob("scripts/**/*.py"), *root.glob(".github/**/*.py")]
    local_names = {path.stem for path in files} | {"scripts"}
    for path in files:
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            if isinstance(node, ast.Import):
                names = [alias.name.split(".")[0] for alias in node.names]
            elif isinstance(node, ast.ImportFrom) and node.level == 0:
                names = [(node.module or "").split(".")[0]]
            else:
                continue
            external.update(name for name in names if name and name not in sys.stdlib_module_names
                            and name not in local_names)
    return {"nuget_lockfiles": [str(p.relative_to(root)) for p in locks],
            "packages": [*packages.values(), *python], "actions": list(actions.values()),
            "local_actions": local, "external_python_imports": sorted(external),
            "unknown_python_imports": sorted(external - {"yaml", "defusedxml"})}


def license_metadata(data, ecosystem, source):
    if not isinstance(data, dict):
        raise ValueError("license metadata is not an object")
    if ecosystem == "NuGet":
        expression, url = data.get("licenseExpression"), data.get("licenseUrl")
        text, classifiers = None, []
    elif ecosystem == "PyPI":
        expression, text = data.get("license_expression"), data.get("license")
        urls = data.get("project_urls") or {}
        url = urls.get("License") or urls.get("license")
        classifiers = [item for item in data.get("classifiers", []) if item.startswith("License ::")]
    else:
        expression = (data.get("license") or {}).get("spdx_id")
        url, text, classifiers = data.get("html_url"), None, []
    if expression == "NOASSERTION":
        expression = None
    return {"expression": expression, "text": text, "classifiers": classifiers,
            "url": url, "source": source, "approval_status": "review_required",
            "reason": "No general non-Rust approved license policy is defined; preserve upstream terms"}


def action_metadata(action, fetch):
    name, ref = action["name"], action["ref"]
    base = GITHUB + "/" + name
    commit = fetch(base + "/commits/" + urllib.parse.quote(ref, safe=""))
    sha = commit.get("sha")
    if not isinstance(sha, str) or not SHA.fullmatch(sha) or (SHA.fullmatch(ref) and sha != ref):
        raise ValueError("action reference did not resolve to its exact commit")
    tags = fetch(base + "/tags?per_page=100")
    if not isinstance(tags, list):
        raise ValueError("action tags metadata is not an array")
    versions = set()
    for tag in tags:
        match = VERSION.fullmatch(tag.get("name", ""))
        if match and tag.get("commit", {}).get("sha") == sha:
            versions.add(match.group(1))
    url = base + "/license?ref=" + sha
    return {**action, "resolved_sha": sha, "immutable": bool(SHA.fullmatch(ref)),
            "versions": sorted(versions), "tag_lookup_limit": 100,
            "version_coverage": "exact_tag_at_resolved_sha" if versions else "review_required",
            "license": license_metadata(fetch(url), "GitHub Actions", url),
            "license_scope": "action repository root license; embedded tools are not approved by this result"}


def audit_osv(queries, fetch):
    if not queries:
        return []
    response = fetch(OSV_BATCH, {"queries": queries})
    results = response.get("results")
    if not isinstance(results, list) or len(results) != len(queries):
        raise ValueError("OSV response does not cover the complete ordered query batch")
    output = []
    for query, result in zip(queries, results):
        if not isinstance(result, dict) or result.get("next_page_token"):
            raise ValueError("OSV response is malformed or requires another page")
        advisories = result.get("vulns", [])
        if not isinstance(advisories, list) or any(not isinstance(v, dict) or not v.get("id") for v in advisories):
            raise ValueError("OSV advisory collection is malformed")
        output.append({**query, "advisories": sorted({v["id"] for v in advisories})})
    return output


def collect(data, fetch):
    packages = []
    for original in data["packages"]:
        package = dict(original)
        name, version = package["name"], package["version"]
        if package["ecosystem"] == "NuGet":
            url = NUGET + "/" + urllib.parse.quote(name.lower(), safe="") + "/" + urllib.parse.quote(version.lower(), safe="") + ".json"
            leaf = fetch(url)
            entry = leaf.get("catalogEntry")
            if isinstance(entry, str):
                url = entry
                entry = fetch(url)
            if not isinstance(entry, dict):
                raise ValueError("NuGet registration lacks catalog metadata")
            if entry.get("id", "").lower() != name.lower() or entry.get("version", "").lower() != version.lower():
                raise ValueError("NuGet metadata identity differs from the locked package")
            package["license"] = license_metadata(entry, "NuGet", url)
            package["vulnerability_owner"] = "Windows locked NuGet restore; no duplicate OSV query"
        else:
            url = PYPI + "/" + urllib.parse.quote(name, safe="") + "/" + urllib.parse.quote(version, safe="") + "/json"
            metadata = fetch(url)
            info = metadata.get("info")
            if not isinstance(info, dict) or info.get("version") != version:
                raise ValueError("PyPI metadata differs from the inventoried version")
            if "sha256" in package and not any(asset.get("digests", {}).get("sha256") == package["sha256"]
                                               for asset in metadata.get("urls", [])):
                raise ValueError("hash-pinned Python artifact is absent from official PyPI metadata")
            package["license"] = license_metadata(info, "PyPI", url)
            package["upstream_requires_dist"] = info.get("requires_dist") or []
        packages.append(package)
    actions = [action_metadata(action, fetch) for action in data["actions"]]
    queries = [{"package": {"ecosystem": "PyPI", "name": p["name"]}, "version": p["version"]}
               for p in packages if p["ecosystem"] == "PyPI"]
    queries.extend({"package": {"ecosystem": "GitHub Actions", "name": a["name"]}, "version": version}
                   for a in actions for version in a["versions"])
    audit = audit_osv(queries, fetch)
    uncovered = [{"ecosystem": "GitHub Actions", "name": a["name"], "ref": a["ref"],
                  "reason": "No full release version at this commit in the first 100 upstream tags"}
                 for a in actions if not a["versions"]]
    uncovered.extend({"ecosystem": "PyPI", "name": name, "reason": "Import lacks an inventoried version"}
                     for name in data["unknown_python_imports"])
    for package in packages:
        if package.get("upstream_requires_dist"):
            uncovered.append({"ecosystem": "PyPI", "name": package["name"],
                              "reason": "Upstream dependency declarations need a transitive lock review"})
    return {"collection_status": "complete", "utc": datetime.now(timezone.utc).isoformat(),
            "nuget_lockfiles": data["nuget_lockfiles"], "packages": packages, "actions": actions,
            "local_actions": data["local_actions"], "external_python_imports": data["external_python_imports"],
            "osv_queries": audit, "vulnerabilities": sorted({v for q in audit for v in q["advisories"]}),
            "coverage_status": "review_required" if uncovered else "covered_osv_versions_queried",
            "uncovered": uncovered, "license_policy_status": "review_required",
            "existing_notice_source": "THIRD_PARTY_NOTICES.md; Linux Rust approval remains deny.toml",
            "scope": "NuGet/Python/Actions metadata; Rust full graph and NuGet vulnerabilities have existing owners"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args(argv)
    try:
        result = collect(inventory(args.root.resolve()), Client())
        status = 2 if result["vulnerabilities"] else 0
    except (OSError, ValueError, KeyError, TypeError, yaml.YAMLError) as error:
        result = {"collection_status": "failed", "error": type(error).__name__ + ": " + str(error),
                  "coverage_status": "inconclusive", "license_policy_status": "review_required"}
        status = 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print("dependency metadata:", result["collection_status"], "exit=" + str(status))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
