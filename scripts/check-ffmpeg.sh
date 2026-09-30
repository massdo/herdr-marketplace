#!/bin/sh
# Checks a private FFmpeg built by scripts/build-ffmpeg.sh: it turns
# tests/fixtures/tiny.mp4 (10 frames of 160 × 90, H.264 High with B-frames,
# index at the end) into raw frames with the plugin's own arguments, and it
# cannot read an address. Usage: sh scripts/check-ffmpeg.sh <ffmpeg>
set -eu

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: sh scripts/check-ffmpeg.sh <ffmpeg>" >&2
  exit 2
fi
ffmpeg=$1
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' HUP INT TERM

"$ffmpeg" -nostdin -v error -threads 2 -protocol_whitelist file -f mov -t 600 \
  -i "$ROOT/tests/fixtures/tiny.mp4" -an \
  -vf 'fps=10,scale=160:90:force_original_aspect_ratio=decrease,pad=160:90:(ow-iw)/2:(oh-ih)/2' \
  -f rawvideo -pix_fmt rgb24 pipe:1 > "$tmp/frames"
size=$(wc -c < "$tmp/frames" | tr -d ' ')
if [ "$size" -ne 432000 ]; then
  echo "tiny.mp4 gave $size bytes of frames, expected 432000 (10 × 160 × 90 × 3)" >&2
  exit 1
fi

if "$ffmpeg" -nostdin -v error -i https://example.com/x.mp4 \
  -f rawvideo -pix_fmt rgb24 pipe:1 > /dev/null 2> "$tmp/refused"; then
  echo "$ffmpeg read https://example.com/x.mp4" >&2
  exit 1
fi
if ! grep -q 'Protocol not found' "$tmp/refused"; then
  echo "$ffmpeg did not refuse https://example.com/x.mp4 for its protocol:" >&2
  cat "$tmp/refused" >&2
  exit 1
fi
echo ffmpeg_ok
