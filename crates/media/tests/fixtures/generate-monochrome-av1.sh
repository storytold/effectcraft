#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")"
ffmpeg -hide_banner -loglevel error \
    -f lavfi -i "nullsrc=s=16x12:r=25,format=gray,geq=lum='16+64*N+X+2*Y'" \
    -frames:v 3 -c:v libaom-av1 -pix_fmt gray -color_range pc \
    -color_primaries bt709 -color_trc bt709 -colorspace bt709 \
    -g 1 -crf 0 -b:v 0 -aom-params lossless=1 -cpu-used 6 -threads 16 \
    -fflags +bitexact -flags:v +bitexact -f matroska -y monochrome-av1.mkv
