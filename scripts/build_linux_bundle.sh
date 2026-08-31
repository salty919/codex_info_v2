#!/usr/bin/env bash
set -euo pipefail

# Build the self-contained Linux release handoff.  This script is allowed to
# read the repository because it is the producer of the handoff.  The
# installer deliberately has no corresponding repository lookup.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-linux-gnu"
SCHEMA="codex-info-linux-bundle-v1"
PRODUCT="codex_info"
ARCHIVE_PREFIX="codex-info"
COMPATIBILITY="${COMPATIBILITY:-glibc}"
OBJDUMP_BIN="${OBJDUMP_BIN:-objdump}"
BINARY=""
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT_DIR/dist}"
VERSION="${VERSION:-}"
SOURCE_SHA="${SOURCE_SHA:-${GITHUB_SHA:-}}"
RUN_ID="${RUN_ID:-${GITHUB_RUN_ID:-}}"
RUN_ATTEMPT="${RUN_ATTEMPT:-${GITHUB_RUN_ATTEMPT:-1}}"

usage() {
    cat <<'EOF'
usage: build_linux_bundle.sh [options]

Options:
  --binary PATH              pre-built codex_info binary
  --output-dir PATH          directory for the three release assets
  --version VERSION          product version (defaults to Cargo.toml)
  --source-sha VALUE         source revision recorded in the manifest
  --run-id VALUE              producer run id recorded in the manifest
  --run-attempt NUMBER        producer run attempt recorded in the manifest
  --target TARGET             must be x86_64-unknown-linux-gnu
  --compatibility VALUE      libc compatibility recorded in the manifest
  -h, --help                 show this help
EOF
}

die() {
    echo "linux-bundle-build: $*" >&2
    exit 1
}

while (($# > 0)); do
    case "$1" in
        --binary)
            (($# >= 2)) || die '--binary requires a path'
            BINARY="$2"
            shift 2
            ;;
        --output-dir|--out-dir|--output)
            (($# >= 2)) || die "$1 requires a path"
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --version)
            (($# >= 2)) || die '--version requires a value'
            VERSION="$2"
            shift 2
            ;;
        --source-sha)
            (($# >= 2)) || die '--source-sha requires a value'
            SOURCE_SHA="$2"
            shift 2
            ;;
        --run-id)
            (($# >= 2)) || die '--run-id requires a value'
            RUN_ID="$2"
            shift 2
            ;;
        --run-attempt)
            (($# >= 2)) || die '--run-attempt requires a value'
            RUN_ATTEMPT="$2"
            shift 2
            ;;
        --target)
            (($# >= 2)) || die '--target requires a value'
            TARGET="$2"
            shift 2
            ;;
        --compatibility)
            (($# >= 2)) || die '--compatibility requires a value'
            COMPATIBILITY="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

[[ "$TARGET" == 'x86_64-unknown-linux-gnu' ]] ||
    die "unsupported target: $TARGET (only x86_64-unknown-linux-gnu is supported)"

[[ "$COMPATIBILITY" == glibc ]] ||
    die "unsupported compatibility baseline: $COMPATIBILITY (only glibc is supported)"
[[ "$PRODUCT" =~ ^[a-z0-9][a-z0-9._-]*$ ]] || die 'invalid product name'
if [[ -z "$VERSION" ]]; then
    VERSION="$(awk -F '"' '/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/{print $2; exit}' "$ROOT_DIR/Cargo.toml")"
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    die "invalid product version: $VERSION"

[[ "$SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] ||
    die 'source SHA must be a 40-character lowercase hexadecimal value'
[[ "$RUN_ID" =~ ^[1-9][0-9]*$ ]] ||
    die 'run id must be a positive decimal workflow run id'
[[ "$RUN_ATTEMPT" =~ ^[1-9][0-9]*$ ]] ||
    die 'run attempt must be a positive integer'

if [[ -z "$BINARY" ]]; then
    command -v cargo >/dev/null 2>&1 || die 'cargo is required when --binary is not supplied'
    (
        cd -- "$ROOT_DIR"
        cargo build --release --locked --target "$TARGET"
    )
    BINARY="$ROOT_DIR/target/$TARGET/release/codex_info"
fi

[[ -f "$BINARY" && -x "$BINARY" ]] ||
    die "release binary is not executable: $BINARY"
[[ -f "$ROOT_DIR/packaging/codex-info.service" ]] ||
    die 'packaging/codex-info.service is missing'
[[ -f "$ROOT_DIR/LICENSE" ]] || die 'LICENSE is missing'
[[ -f "$ROOT_DIR/THIRD_PARTY_NOTICES.md" ]] ||
    die 'THIRD_PARTY_NOTICES.md is missing'
[[ -f "$ROOT_DIR/assets/NOTICE.txt" ]] || die 'assets/NOTICE.txt is missing'
[[ -f "$ROOT_DIR/COPYRIGHT" ]] || die 'COPYRIGHT is missing'
[[ -f "$ROOT_DIR/packaging/install_linux_bundle.sh" ]] ||
    die 'packaging/install_linux_bundle.sh is missing'

command -v "$OBJDUMP_BIN" >/dev/null 2>&1 || die 'objdump is required to measure glibc minimum'
GLIBC_MINIMUM="$("$OBJDUMP_BIN" -T -- "$BINARY" 2>/dev/null |
    grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' |
    sed 's/^GLIBC_//' | LC_ALL=C sort -V | tail -n 1 || true)"
[[ "$GLIBC_MINIMUM" =~ ^[0-9]+\.[0-9]+$ ]] ||
    die 'could not measure a glibc minimum from the release binary'

mkdir -p -- "$OUTPUT_DIR"
OUTPUT_DIR="$(cd -- "$OUTPUT_DIR" && pwd)"
work_dir="$(mktemp -d "$OUTPUT_DIR/.codex-info-linux-bundle.XXXXXX")"
cleanup() {
    rm -r -- "$work_dir"
}
trap cleanup EXIT

payload="$work_dir/payload"
mkdir -- "$payload" "$payload/LICENSES"
install -m 0755 -- "$BINARY" "$payload/codex_info"
install -m 0644 -- "$ROOT_DIR/packaging/codex-info.service" \
    "$payload/codex-info.service"
install -m 0755 -- "$ROOT_DIR/packaging/install_linux_bundle.sh" \
    "$payload/install.sh"
install -m 0644 -- "$ROOT_DIR/LICENSE" "$payload/LICENSE"
install -m 0644 -- "$ROOT_DIR/LICENSE.ja.md" "$payload/LICENSE.ja.md"
install -m 0644 -- "$ROOT_DIR/COPYRIGHT" "$payload/COPYRIGHT"
install -m 0644 -- "$ROOT_DIR/THIRD_PARTY_NOTICES.md" \
    "$payload/THIRD_PARTY_NOTICES.md"
install -m 0644 -- "$ROOT_DIR/assets/NOTICE.txt" "$payload/NOTICE.txt"
for license_file in "$ROOT_DIR"/LICENSES/*.txt; do
    [[ -f "$license_file" ]] || continue
    install -m 0644 -- "$license_file" "$payload/LICENSES/$(basename -- "$license_file")"
done

manifest_stage="$work_dir/manifest.json"
python3 - "$payload" "$manifest_stage" "$SCHEMA" "$PRODUCT" "$VERSION" \
    "$SOURCE_SHA" "$RUN_ID" "$RUN_ATTEMPT" "$TARGET" "$COMPATIBILITY" \
    "$GLIBC_MINIMUM" <<'PY'
import hashlib
import json
import pathlib
import sys

(
    payload_name,
    manifest_name,
    schema,
    product,
    version,
    source_sha,
    run_id,
    run_attempt,
    target,
    compatibility,
    glibc_minimum,
) = sys.argv[1:]
payload = pathlib.Path(payload_name)
manifest = pathlib.Path(manifest_name)
files = []
for path in sorted(payload.rglob("*")):
    if path.is_dir():
        continue
    if not path.is_file() or path.is_symlink():
        raise SystemExit(f"payload member is not a regular file: {path}")
    relative = path.relative_to(payload).as_posix()
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while True:
            chunk = stream.read(1024 * 1024)
            if not chunk:
                break
            size += len(chunk)
            digest.update(chunk)
    files.append({"path": relative, "size": size, "sha256": digest.hexdigest()})

document = {
    "schema": schema,
    "product": product,
    "version": version,
    "source_sha": source_sha,
    "run_id": run_id,
    "run_attempt": int(run_attempt),
    "target": target,
    "compatibility": compatibility,
    "glibc_minimum": glibc_minimum,
    "files": files,
}
with manifest.open("w", encoding="utf-8") as stream:
    json.dump(document, stream, ensure_ascii=False, indent=2)
    stream.write("\n")
PY

# The internal copy is part of the archive so the extracted bundle remains
# self-describing.  It is intentionally not listed in the external file list:
# including a manifest's own digest would be circular.
install -m 0644 -- "$manifest_stage" "$payload/manifest.json"
sums_stage="$work_dir/SHA256SUMS"
(
    cd -- "$payload"
    find . -type f ! -name SHA256SUMS -printf '%P\n' | LC_ALL=C sort |
        xargs -r sha256sum
) > "$sums_stage"
install -m 0644 -- "$sums_stage" "$payload/SHA256SUMS"

archive_name="${ARCHIVE_PREFIX}-${VERSION}-${TARGET}.tar.gz"
archive_stage="$work_dir/$archive_name"
(
    cd -- "$payload"
    find . -type f -printf '%P\n' | LC_ALL=C sort |
        tar --create --gzip --file="$archive_stage" \
            --format=ustar --sort=name --mtime='UTC 1970-01-01' \
            --owner=0 --group=0 --numeric-owner --no-recursion --files-from=-
)

checksum_stage="$work_dir/$archive_name.sha256"
(cd -- "$work_dir" && sha256sum -- "$archive_name") > "$checksum_stage"
manifest_name="${ARCHIVE_PREFIX}-${VERSION}-${TARGET}.manifest.json"

archive_destination="$OUTPUT_DIR/$archive_name"
checksum_destination="$OUTPUT_DIR/$archive_name.sha256"
manifest_destination="$OUTPUT_DIR/$manifest_name"
mv -f -- "$archive_stage" "$archive_destination"
mv -f -- "$checksum_stage" "$checksum_destination"
mv -f -- "$manifest_stage" "$manifest_destination"

printf 'bundle=%s\nchecksum=%s\nmanifest=%s\ntarget=%s\n' \
    "$archive_destination" "$checksum_destination" "$manifest_destination" "$TARGET"
