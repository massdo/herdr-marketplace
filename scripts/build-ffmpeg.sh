#!/bin/sh
# Builds the private FFmpeg that plays README videos: FFmpeg 9.0.2 with only
# what they need, H.264 in MP4 or MOV read from a local file and turned into
# raw frames. No network, no other format. About 4 MB instead of 45 to 80 MB
# for a full FFmpeg. Usage: sh scripts/build-ffmpeg.sh <output>
set -eu

VERSION=9.0.2
ARCHIVE="ffmpeg-$VERSION.tar.xz"
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: sh scripts/build-ffmpeg.sh <output>" >&2
  exit 2
fi
case $1 in
  /*) out=$1 ;;
  *) out="$PWD/$1" ;;
esac

if [ -f "$out" ] && version=$("$out" -version 2>/dev/null); then
  case $version in
    "ffmpeg version $VERSION"*)
      echo "ffmpeg $VERSION already built"
      exit 0
      ;;
  esac
fi

# FFmpeg's SIMD code for x86_64 is assembled with nasm.
if [ "$(uname -m)" = x86_64 ] && ! command -v nasm >/dev/null 2>&1; then
  echo "nasm is required to build FFmpeg for x86_64" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' HUP INT TERM

curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --retry 2 --output "$tmp/$ARCHIVE" "https://ffmpeg.org/releases/$ARCHIVE"
if command -v sha256sum >/dev/null 2>&1; then
  sum=$(sha256sum "$tmp/$ARCHIVE")
else
  sum=$(shasum -a 256 "$tmp/$ARCHIVE")
fi
if [ "${sum%% *}" != "$SHA256" ]; then
  echo "SHA-256 mismatch for $ARCHIVE" >&2
  exit 1
fi
tar -xJf "$tmp/$ARCHIVE" -C "$tmp"
cd "$tmp/ffmpeg-$VERSION"

# Linux gets a static binary, which runs on any distribution.
static=""
if [ "$(uname -s)" = Linux ]; then
  static=--extra-ldflags=-static
fi
./configure --disable-everything --disable-autodetect --disable-network --disable-doc \
  --disable-debug --disable-ffplay --disable-ffprobe --disable-avdevice --disable-swresample \
  --enable-protocol=file,pipe --enable-demuxer=mov --enable-decoder=h264 --enable-parser=h264 \
  --enable-filter=scale,fps,pad,format,null,buffer,buffersink --enable-encoder=rawvideo \
  --enable-muxer=rawvideo --disable-shared --enable-static $static
cores=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)
make -j"$cores"
strip ffmpeg

mkdir -p "$(dirname "$out")"
cp ffmpeg "$out"
chmod 755 "$out"
echo "ffmpeg $VERSION built: $out"
