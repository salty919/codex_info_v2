#!/usr/bin/env bash
set -euo pipefail

# Producer for the self-contained Linux release handoff.  This script may
# read the checkout and may invoke Cargo to produce a release binary.  The
# runtime launcher and the installed consumer do not share this authority.
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
  --run-id VALUE             producer run id recorded in the manifest
  --run-attempt NUMBER        producer run attempt recorded in the manifest
  --target TARGET             must be x86_64-unknown-linux-gnu
  --compatibility VALUE       must be glibc
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
        *) die "unknown argument: $1" ;;
    esac
done

[[ "$TARGET" == x86_64-unknown-linux-gnu ]] ||
    die "unsupported target: $TARGET (only x86_64-unknown-linux-gnu is supported)"
[[ "$COMPATIBILITY" == glibc ]] ||
    die "unsupported compatibility baseline: $COMPATIBILITY (only glibc is supported)"
if [[ -z "$VERSION" ]]; then
    VERSION="$(awk -F '"' '/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/{print $2; exit}' "$ROOT_DIR/Cargo.toml")"
fi
[[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
    die "invalid product version: $VERSION"
[[ "$SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] ||
    die 'source SHA must be a 40-character lowercase hexadecimal value'
[[ "$RUN_ID" =~ ^[1-9][0-9]*$ ]] ||
    die 'run id must be a positive decimal workflow run id'
[[ "$RUN_ATTEMPT" =~ ^[1-9][0-9]*$ ]] ||
    die 'run attempt must be a positive integer'

if [[ -z "$BINARY" ]]; then
    command -v cargo >/dev/null 2>&1 || die 'cargo is required when --binary is not supplied'
    (cd -- "$ROOT_DIR" && cargo build --release --locked --target "$TARGET")
    BINARY="$ROOT_DIR/target/$TARGET/release/codex_info"
fi
[[ -f "$BINARY" && -x "$BINARY" && ! -L "$BINARY" ]] ||
    die "release binary is not an executable regular file: $BINARY"

for required_file in \
    "$ROOT_DIR/run.sh" \
    "$ROOT_DIR/packaging/codex-info.service" \
    "$ROOT_DIR/packaging/codex-info-update.service" \
    "$ROOT_DIR/packaging/codex-info-update.timer" \
    "$ROOT_DIR/packaging/install_linux_bundle.sh" \
    "$ROOT_DIR/LICENSE" "$ROOT_DIR/LICENSE.ja.md" "$ROOT_DIR/COPYRIGHT" \
    "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$ROOT_DIR/assets/NOTICE.txt"; do
    [[ -f "$required_file" && ! -L "$required_file" ]] ||
        die "required bundle source is missing or not regular: $required_file"
done
command -v "$OBJDUMP_BIN" >/dev/null 2>&1 || die 'objdump is required to measure glibc minimum'
GLIBC_MINIMUM="$("$OBJDUMP_BIN" -T -- "$BINARY" 2>/dev/null |
    grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' |
    sed 's/^GLIBC_//' | LC_ALL=C sort -V | tail -n 1 || true)"
[[ "$GLIBC_MINIMUM" =~ ^[0-9]+(\.[0-9]+)+$ ]] ||
    die 'could not measure a glibc minimum from the release binary'

mkdir -p -- "$OUTPUT_DIR"
OUTPUT_DIR="$(cd -- "$OUTPUT_DIR" && pwd)"
work_dir="$(mktemp -d "$OUTPUT_DIR/.codex-info-linux-bundle.XXXXXX")"
cleanup() { rm -r -- "$work_dir"; }
trap cleanup EXIT
payload="$work_dir/payload"
mkdir -- "$payload" "$payload/LICENSES"

# The exact executable contract is three 0755 files.  Everything else in the
# archive is a regular 0644 file, including every legal notice.
install -m 0755 -- "$BINARY" "$payload/codex_info"
install -m 0755 -- "$ROOT_DIR/run.sh" "$payload/run.sh"
install -m 0755 -- "$ROOT_DIR/packaging/install_linux_bundle.sh" "$payload/install.sh"
install -m 0644 -- "$ROOT_DIR/packaging/codex-info.service" "$payload/codex-info.service"
install -m 0644 -- "$ROOT_DIR/packaging/codex-info-update.service" "$payload/codex-info-update.service"
install -m 0644 -- "$ROOT_DIR/packaging/codex-info-update.timer" "$payload/codex-info-update.timer"
install -m 0644 -- "$ROOT_DIR/LICENSE" "$payload/LICENSE"
install -m 0644 -- "$ROOT_DIR/LICENSE.ja.md" "$payload/LICENSE.ja.md"
install -m 0644 -- "$ROOT_DIR/COPYRIGHT" "$payload/COPYRIGHT"
install -m 0644 -- "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$payload/THIRD_PARTY_NOTICES.md"
install -m 0644 -- "$ROOT_DIR/assets/NOTICE.txt" "$payload/NOTICE.txt"
for license_file in "$ROOT_DIR"/LICENSES/*.txt; do
    [[ -f "$license_file" && ! -L "$license_file" ]] || continue
    install -m 0644 -- "$license_file" "$payload/LICENSES/$(basename -- "$license_file")"
done

manifest_stage="$work_dir/manifest.json"
python3 - "$payload" "$manifest_stage" "$SCHEMA" "$PRODUCT" "$VERSION" \
    "$SOURCE_SHA" "$RUN_ID" "$RUN_ATTEMPT" "$TARGET" "$COMPATIBILITY" \
    "$GLIBC_MINIMUM" <<'PY'
import hashlib
import json
import pathlib
import stat
import sys

(payload_name, manifest_name, schema, product, version, source_sha, run_id,
 run_attempt, target, compatibility, glibc_minimum) = sys.argv[1:]
payload = pathlib.Path(payload_name)
manifest = pathlib.Path(manifest_name)
files = []
for path in sorted(payload.rglob("*"), key=lambda p: p.relative_to(payload).as_posix()):
    if path.is_dir():
        continue
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"payload member is not a regular file: {path}")
    rel = path.relative_to(payload).as_posix()
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while True:
            chunk = stream.read(1024 * 1024)
            if not chunk:
                break
            size += len(chunk)
            digest.update(chunk)
    files.append({"path": rel, "size": size, "sha256": digest.hexdigest(),
                  "mode": stat.S_IMODE(path.stat().st_mode)})
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
manifest.write_text(json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY

install -m 0644 -- "$manifest_stage" "$payload/manifest.json"
sum_stage="$work_dir/SHA256SUMS"
(
    cd -- "$payload"
    find . -type f ! -name SHA256SUMS -printf '%P\n' | LC_ALL=C sort |
        while IFS= read -r member; do sha256sum -- "$member"; done
) > "$sum_stage"
install -m 0644 -- "$sum_stage" "$payload/SHA256SUMS"

archive_name="${ARCHIVE_PREFIX}-${VERSION}-${TARGET}.tar.gz"
archive_stage="$work_dir/$archive_name"
(
    cd -- "$payload"
    find . -type f -printf '%P\n' | LC_ALL=C sort |
        GZIP=-n tar --create --gzip --file="$archive_stage" --format=ustar \
            --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 \
            --numeric-owner --no-recursion --files-from=-
)
checksum_stage="$work_dir/$archive_name.sha256"
(cd -- "$work_dir" && sha256sum -- "$archive_name") > "$checksum_stage"
manifest_name="${ARCHIVE_PREFIX}-${VERSION}-${TARGET}.manifest.json"

mv -f -- "$archive_stage" "$OUTPUT_DIR/$archive_name"
mv -f -- "$checksum_stage" "$OUTPUT_DIR/$archive_name.sha256"
mv -f -- "$manifest_stage" "$OUTPUT_DIR/$manifest_name"
printf 'bundle=%s\nchecksum=%s\nmanifest=%s\ntarget=%s\n' \
    "$OUTPUT_DIR/$archive_name" "$OUTPUT_DIR/$archive_name.sha256" \
    "$OUTPUT_DIR/$manifest_name" "$TARGET"
