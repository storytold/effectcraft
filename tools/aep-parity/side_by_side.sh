#!/usr/bin/env bash
# side_by_side.sh REFERENCE.mp4 OURS.mp4 OUT.mp4 [--font FONT.ttf] [--labels "After Effects" "EffectCraft"]
#
# Side-by-side video for eyeballing: the reference on the left, the EffectCraft render on the right,
# both decoded as delivered (square pixels restored with setsar=1 after scaling to the display size),
# with a label and a frame counter when a font is given (drawtext needs a font file; without --font
# the video has no text). Both inputs must have the same frame rate and frame count.
#
# A review helper only: it calls your own ffmpeg / ffprobe (on PATH, with libx264) to decode both
# videos and encode the comparison. EffectCraft never links, bundles or ships them.
set -euo pipefail
if [ $# -lt 3 ]; then sed -n '2,10p' "$0"; exit 2; fi
REF="$1"; OURS="$2"; OUT="$3"; shift 3
FONT=""; LA="After Effects (reference)"; LB="EffectCraft"
while [ $# -gt 0 ]; do
  case "$1" in
    --font) FONT="$2"; shift 2 ;;
    --labels) LA="$2"; LB="$3"; shift 3 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
# Display size of the reference (stored size x sample aspect ratio), even dimensions.
IFS=, read -r W H SAR < <(ffprobe -v error -select_streams v:0 -show_entries stream=width,height,sample_aspect_ratio \
  -of csv=p=0 "$REF")
case "$SAR" in ""|N/A|0:1) SAR=1:1 ;; esac
DW=$(( (W * ${SAR%:*} / ${SAR#*:} + 1) / 2 * 2 ))
fit="scale=$DW:$H:flags=bicubic,setsar=1"
lab() {
  [ -n "$FONT" ] || { echo "null"; return; }
  local t="${1//:/\\:}"
  echo "drawtext=fontfile='$FONT':text='$t':x=16:y=16:fontsize=h/24:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=8"
}
frame="null"
[ -n "$FONT" ] && frame="drawtext=fontfile='$FONT':text='frame %{frame_num}':x=w-tw-16:y=16:fontsize=h/28:fontcolor=white:box=1:boxcolor=black@0.6:boxborderw=8"
ffmpeg -v error -y -i "$REF" -i "$OURS" \
  -filter_complex "[0:v]$fit,$(lab "$LA")[a];[1:v]$fit,$(lab "$LB")[b];[a][b]hstack=inputs=2,$frame[v]" \
  -map "[v]" -c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p \
  -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv -movflags +faststart -an "$OUT"
echo "wrote $OUT"
