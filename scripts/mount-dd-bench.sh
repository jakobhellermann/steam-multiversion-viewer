#!/usr/bin/env bash
# TODO(ai-review): review for style and correctness
#
# e2e read benchmark through the FUSE mount: reads a fully-present file
# of a cached manifest via the kernel (dd), so the number includes the
# FUSE round trips and the backend's read pipeline on top of the store.
# In-process comparison point: `cargo bench -p steam-depot-vfs -- e2e`
# (read_full, no kernel in between).
#
# The backend must be running and logged in (the mount start needs the
# Steam session). If the mount is not up, the script starts it and stops
# it again at the end; an already-running mount is left as is.
#
# Only fetch-free files give meaningful numbers: pass a file whose chunks
# are all in the store, else the mount pulls them from the CDN mid-read.

set -euo pipefail
# dd's stats line is locale-dependent (e.g. "kopiert, 0,5 s" under de_DE).
export LC_ALL=C

BASE="${BASE:-http://127.0.0.1:6556}"
APP_ID="${APP_ID:-1030300}"
DEPOT_ID="${DEPOT_ID:-1030301}"
MANIFEST_GID="${MANIFEST_GID:-4421626056705534276}"
FILE="${FILE:-Hollow Knight Silksong_Data/StreamingAssets/aa/StandaloneWindows64/tk2dcollections_assets_areacoral.bundle}"
RUNS="${RUNS:-3}"

api() { curl -sS --max-time 30 "$@"; }
status_json() { api "$BASE/api/mount/status"; }

STARTED_BY_US=0
MOUNTPOINT=
case "$(status_json | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])')" in
    mounted)
        MOUNTPOINT=$(status_json | python3 -c 'import json,sys; print(json.load(sys.stdin)["mountpoint"])')
        ;;
    idle)
        api -f -X POST "$BASE/api/mount/start" >/dev/null
        STARTED_BY_US=1
        MOUNTPOINT=$(status_json | python3 -c 'import json,sys; print(json.load(sys.stdin)["mountpoint"])')
        ;;
    *) echo "unexpected mount state" >&2; exit 1 ;;
esac
cleanup() {
    if [[ "$STARTED_BY_US" == 1 ]]; then
        api -f -X POST "$BASE/api/mount/stop" >/dev/null
    fi
}
trap cleanup EXIT
SRC="$MOUNTPOINT/$APP_ID/$DEPOT_ID/$MANIFEST_GID/$FILE"
if [[ ! -f "$SRC" ]]; then
    echo "not on the mount: $SRC" >&2
    exit 1
fi
SIZE=$(stat -c %s "$SRC")
printf 'file: %s\nsize: %s B\nmount: %s\n\n' "$FILE" "$SIZE" "$MOUNTPOINT"

# Warm-up fills the page cache so the measured runs are warm, and surfaces
# fetch-related read errors before the timing runs.
dd if="$SRC" of=/dev/null bs=1M status=none

# dd prints its stats on stderr; "of=" means stdout stays empty.
dd_mb_s() {
    local stats
    stats=$(dd if="$SRC" of=/dev/null bs=1M 2>&1)
    python3 -c '
import re, sys
m = re.search(r"(\d+) bytes .* copied, ([\d.]+) s", sys.argv[1])
if not m:
    sys.exit(f"could not parse dd stats: {sys.argv[1]!r}")
print(f"{int(m.group(1)) / 1e6 / float(m.group(2)):.0f}")
' "$stats"
}

printf 'warm dd (bs=1M), %s runs:\n' "$RUNS"
rates=()
for _ in $(seq "$RUNS"); do
    rates+=("$(dd_mb_s)")
    printf '  %6s MB/s\n' "${rates[-1]}"
done
printf 'median: %s MB/s\n' \
    "$(python3 -c '
import statistics, sys
print(f"{statistics.median(map(float, sys.argv[1:])):.0f}")
' "${rates[@]}")"
