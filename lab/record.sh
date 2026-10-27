#!/usr/bin/env bash
# Records a lab VM's screen for demos. The VMs have no hardware video encoder (screenrecord fails
# with err=-38), so this pulls raw `screencap` frames over adb (about 6 a second) and encodes them
# here.
#
#   lab/record.sh b16 start <name>          # in the background until stopped
#   lab/record.sh b16 stop <name> [width]   # -> media/demos/<name>.mp4 and .webp (README, 640 wide)
#
# Frames come at an uneven rate; the video is retimed to the real duration when it stops.
set -euo pipefail

VM=${1:?vm}; ACTION=${2:?start|stop}; NAME=${3:?name}
LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT_DIR=${OUT_DIR:-$LAB_DIR/../media/demos}
WORK=${TMPDIR:-/tmp}/lindroids-record/$VM-$NAME
ADB=${ADB:-$HOME/Android/Sdk/platform-tools/adb}
[[ -x $ADB ]] || ADB=$HOME/.local/opt/platform-tools/adb
read -r ADB_PORT _ < <("$LAB_DIR/vm.sh" "$VM" ports)
SERIAL=127.0.0.1:$ADB_PORT

case $ACTION in
start)
  rm -rf "$WORK"; mkdir -p "$WORK"
  "$ADB" -s "$SERIAL" exec-out screencap >"$WORK/first.raw"
  size=$(wc -c <"$WORK/first.raw")
  read -r w h < <(od -An -tu4 -N8 "$WORK/first.raw")
  header=$((size - w * h * 4))
  echo "$w $h $header" >"$WORK/geometry"
  (
    trap 'exit 0' TERM
    date +%s.%N >"$WORK/t0"
    n=0
    while [[ ! -f $WORK/stop ]]; do
      "$ADB" -s "$SERIAL" exec-out screencap | tail -c +$((header + 1))
      n=$((n + 1)); echo "$n" >"$WORK/frames"
    done
    date +%s.%N >"$WORK/t1"
  ) | ffmpeg -loglevel error -y -f rawvideo -pix_fmt rgba -s "${w}x$h" -framerate 6 -i - \
      -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p "$WORK/raw.mp4" &
  echo $! >"$WORK/pid"
  echo "recording $VM ($w x $h) -> $WORK"
  ;;
stop)
  touch "$WORK/stop"
  while kill -0 "$(cat "$WORK/pid")" 2>/dev/null; do sleep 0.5; done
  frames=$(cat "$WORK/frames"); secs=$(echo "$(cat "$WORK/t1") - $(cat "$WORK/t0")" | bc -l)
  fps=$(echo "$frames / $secs" | bc -l)
  width=${4:-640}
  mkdir -p "$OUT_DIR"
  # Real time again, then a steady 12 fps.
  ffmpeg -loglevel error -y -i "$WORK/raw.mp4" -vf "setpts=N/($fps)/TB,fps=12" \
    -c:v libx264 -preset slow -crf 26 -pix_fmt yuv420p -movflags +faststart "$OUT_DIR/$NAME.mp4"
  # Animated WebP for the README (GitHub shows it inline; an .mp4 in the repo is only a link).
  mkdir -p "$WORK/png"
  ffmpeg -loglevel error -y -i "$OUT_DIR/$NAME.mp4" -vf "fps=8,scale=$width:-2:flags=lanczos" "$WORK/png/%05d.png"
  img2webp -loop 0 -lossy -q 60 -m 6 -mixed -d 125 "$WORK"/png/*.png -o "$OUT_DIR/$NAME.webp" >/dev/null 2>&1
  printf '%s frames in %.1f s (%.1f fps) -> %s.mp4 (%s), %s.webp (%s)\n' "$frames" "$secs" "$fps" \
    "$OUT_DIR/$NAME" "$(du -h "$OUT_DIR/$NAME.mp4" | cut -f1)" "$OUT_DIR/$NAME" "$(du -h "$OUT_DIR/$NAME.webp" | cut -f1)"
  rm -rf "$WORK"
  ;;
*) echo "usage: $0 <vm> start|stop <name> [width]" >&2; exit 2 ;;
esac
