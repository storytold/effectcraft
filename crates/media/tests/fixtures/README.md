# Synthetic AAC layout fixture

`pce61-ffmpeg.aac-fixture` contains only original mathematical sine tones generated for these tests by Dylan M. Taylor. No recorded audio or third-party source/assets were copied; the generated fixture is covered by this repository's license. FFmpeg is a generation tool, not a runtime test dependency.

Generator: FFmpeg 7.1.5 (Debian 7.1.5-0+deb13u1, AAC encoder Lavc61.19.101).
SHA-256: `07c460636dce4aa4163df8ef35f3b76f1214c38bf217aacd23243e608d87071e`.

The seven PCM inputs are FL500Hz, FR700Hz, FC300Hz, LFE60Hz, BC900Hz, SL1100Hz, SR1300Hz, each amplitude0.25 at48kHz for2seconds. The real encoder emits its own PCE and matching element tags; unlike the previous synthetic configuration rewrite, no LFE element is mislabeled as an unlisted SCE.

Reproduce using FFmpeg/ffprobe and Python3:

```sh
ffmpeg -hide_banner -loglevel error -f lavfi -i 'aevalsrc=0.25*sin(2*PI*500*t)|0.25*sin(2*PI*700*t)|0.25*sin(2*PI*300*t)|0.25*sin(2*PI*60*t)|0.25*sin(2*PI*900*t)|0.25*sin(2*PI*1100*t)|0.25*sin(2*PI*1300*t):s=48000:d=2:c=6.1' -c:a aac -b:a 672k pce61.m4a
ffprobe -v error -show_streams -show_packets -show_data -of json pce61.m4a > pce61.json
python3 - <<'PY'
import json, struct
x = json.load(open('pce61.json'))
def raw(text):
    return bytes.fromhex(''.join(line.split(': ', 1)[1].split('  ', 1)[0].replace(' ', '')
                                for line in text.strip().splitlines()))
asc = raw(x['streams'][0]['extradata'])
units = [raw(p['data']) for p in x['packets']]
assert len(units) == 95
output = bytearray(b'PCE1')
for record in [asc, *units]:
    output += struct.pack('>I', len(record)) + record
open('pce61-ffmpeg.aac-fixture', 'wb').write(output)
PY
```

Envelope: ASCII `PCE1`, then a big-endian32-bit length and ASC bytes, then95 length-prefixed raw AAC access units. Container priming trim is deliberately not retained, matching the existing tests' encoder units; measurements start0.5seconds into decoded audio. Other encoder versions can yield different bytes; preserve actual PCE/AU consistency and all existing tone-gain assertions.
