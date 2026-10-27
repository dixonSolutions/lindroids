#!/usr/bin/env bash
# Host integration test: the real server, a stand-in for the browser page, and a real Wayland
# client, all on an ordinary Linux machine (CI: ubuntu-latest with `apt install foot`).
#
#   tests/host-integration.sh            build the CLI (debug) and run the test
#   LINDROIDS_BIN=path/to/lindroids tests/host-integration.sh
#   KEEP=1 tests/host-integration.sh     keep the work dir (frames, logs) for inspection
#
# What it checks:
#   1. `lindroids server` starts with an isolated TMPDIR / LINDROIDS_STATE_DIR;
#   2. `lindroids debug-app` (crates/lindroids-cli/src/debug.rs) pairs with the code and links;
#   3. `lindroids run foot` opens a window whose frames reach the "app" as a PNG that is not blank;
#   4. text typed through the app link (`type`, `key 66` = Enter) changes what foot shows.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

TIMEOUT=${TIMEOUT:-30}
command -v foot >/dev/null || { echo "host-integration: foot is not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "host-integration: python3 is required" >&2; exit 1; }

if [ -z "${LINDROIDS_BIN:-}" ]; then
  cargo build -p lindroids
  LINDROIDS_BIN=$ROOT/target/debug/lindroids
fi
BIN=$(readlink -f "$LINDROIDS_BIN")

mkdir -p "$ROOT/target"
WORK=$(mktemp -d "$ROOT/target/host-integration.XXXXXX")
export TMPDIR=$WORK/tmp LINDROIDS_STATE_DIR=$WORK/state
mkdir -p "$TMPDIR" "$LINDROIDS_STATE_DIR"
RT=$TMPDIR/lindroids-$(id -u)
FRAMES=$WORK/frames
# A private abstract socket, so the test does not collide with a server already running here.
SOCKET=lindroids.server.test.$$
PIDS=()

cleanup() {
  local st=$?
  exec 3>&- 2>/dev/null || true
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  [ -f "$RT/server.pid" ] && kill "$(cat "$RT/server.pid")" 2>/dev/null || true
  sleep 0.3
  for p in "${PIDS[@]}"; do kill -9 "$p" 2>/dev/null || true; done
  if [ $st -ne 0 ]; then
    echo "---- server.log";    tail -n 40 "$WORK/server.log" 2>/dev/null || true
    echo "---- debug-app.log"; tail -n 40 "$WORK/debug-app.log" 2>/dev/null || true
    echo "---- run.log";       tail -n 40 "$WORK/run.log" 2>/dev/null || true
    echo "host-integration: FAILED (work dir $WORK)"
  elif [ -z "${KEEP:-}" ]; then
    rm -rf "$WORK"
  fi
  exit $st
}
trap cleanup EXIT
trap 'exit 130' INT TERM

wait_for() { # <seconds> <description> <command...>
  local secs=$1 what=$2; shift 2
  local end=$((SECONDS + secs))
  until "$@" >/dev/null 2>&1; do
    if [ $SECONDS -ge $end ]; then
      echo "host-integration: timed out after ${secs}s waiting for $what" >&2
      return 1
    fi
    sleep 0.2
  done
}

# Prints "<colours> <non-background fraction> <sha256>" for an 8-bit RGBA/RGB PNG.
png_stats() {
  python3 - "$1" <<'EOF'
import hashlib, struct, sys, zlib
from collections import Counter
data = open(sys.argv[1], "rb").read()
assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
pos, idat, w = 8, b"", 0
while pos < len(data):
    n, kind = struct.unpack(">I4s", data[pos:pos + 8])
    body = data[pos + 8:pos + 8 + n]
    if kind == b"IHDR":
        w, h, depth, ctype = struct.unpack(">IIBB", body[:10])
        assert depth == 8 and ctype in (2, 6), f"unsupported PNG depth={depth} type={ctype}"
        bpp = 4 if ctype == 6 else 3
    elif kind == b"IDAT":
        idat += body
    pos += 12 + n
raw = zlib.decompress(idat)
stride = w * bpp
prev = bytearray(stride)
px = Counter()
for y in range(h):
    f = raw[y * (stride + 1)]
    line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
    for i in range(stride):
        a = line[i - bpp] if i >= bpp else 0
        b = prev[i]
        c = prev[i - bpp] if i >= bpp else 0
        if f == 1: line[i] = (line[i] + a) & 255
        elif f == 2: line[i] = (line[i] + b) & 255
        elif f == 3: line[i] = (line[i] + (a + b) // 2) & 255
        elif f == 4:
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
    for i in range(0, stride, bpp):
        px[bytes(line[i:i + 3])] += 1
    prev = line
total = w * h
top = px.most_common(1)[0][1]
print(len(px), "%.5f" % ((total - top) / total), hashlib.sha256(raw).hexdigest())
EOF
}

echo "==> server ($BIN)"
"$BIN" version
"$BIN" server --runtime-dir "$RT" --socket-name "$SOCKET" > "$WORK/server.log" 2>&1 &
PIDS+=($!)
wait_for "$TIMEOUT" "the server" "$BIN" status
wait_for "$TIMEOUT" "a pairing code" test -s "$RT/pairing-code"

echo "==> debug-app"
mkfifo "$WORK/cmds"
exec 3<>"$WORK/cmds"
"$BIN" debug-app --socket-name "$SOCKET" --dir "$FRAMES" --size 800x600 < "$WORK/cmds" > "$WORK/debug-app.log" 2>&1 &
PIDS+=($!)
wait_for "$TIMEOUT" "the app link" grep -q '^linked' "$WORK/debug-app.log"
grep '^linked' "$WORK/debug-app.log"

echo "==> lindroids run foot"
SHELL=/bin/sh ENV=/dev/null PS1='$ ' "$BIN" run foot /bin/sh > "$WORK/run.log" 2>&1 &
PIDS+=($!)
wait_for "$TIMEOUT" "a window" grep -q '^window ' "$WORK/debug-app.log"
WIN=$(sed -n 's/^window \([0-9][0-9]*\) .*/\1/p' "$WORK/debug-app.log" | head -1)
PNG=$FRAMES/window-$WIN.png
echo "    window $WIN: $(grep '^window ' "$WORK/debug-app.log" | head -1)"
wait_for "$TIMEOUT" "$PNG" test -s "$PNG"

# The first frames can be an empty surface; wait until foot has drawn something. foot asks for
# server-side decorations and gets them, so there is no title bar: its prompt and cursor alone
# (about 140 pixels of 800x600) are what it draws.
nonblank() {
  local s
  s=$(png_stats "$PNG") || return 1
  set -- $s
  [ "$1" -ge 2 ] && python3 -c "import sys; sys.exit(0 if float('$2') > 0.0001 else 1)"
}
wait_for "$TIMEOUT" "a non-blank frame" nonblank
sleep 1
before=$(png_stats "$PNG")
echo "    before typing: colours/non-bg/sha = $before"

echo "==> typing"
echo "type $WIN echo lindroids-ok" >&3
sleep 0.3
echo "key $WIN 66" >&3
changed() {
  local now
  now=$(png_stats "$PNG") || return 1
  [ "${now##* }" != "${before##* }" ]
}
wait_for "$TIMEOUT" "the typed text to change the frame" changed
sleep 0.5
after=$(png_stats "$PNG")
echo "    after typing:  colours/non-bg/sha = $after"
cp "$PNG" "$WORK/../host-integration-last.png" 2>/dev/null || true

echo "host-integration: PASSED (window $WIN, $(stat -c %s "$PNG") byte PNG)"
