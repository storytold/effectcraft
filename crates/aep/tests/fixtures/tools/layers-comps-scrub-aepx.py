# Fixture hygiene for the combined .aepx pair (aepx-pair.aepx): the XMP packet (CDATA of
# ProjectXMPMetadata) replaced by the base project's packet, as scrub.py does for the .aep, so
# that the .aep and .aepx of the pair carry the same trailer.
# Usage: layers-comps-scrub-aepx.py <in.aepx> <out.aepx> <000-base.aep>
import sys, zlib
src, dst, base_path = sys.argv[1], sys.argv[2], sys.argv[3]
d = open(src, 'rb').read()
base = open(base_path, 'rb').read()
at = base.find(b'<?xpacket begin')
assert at >= 0, 'the base project has no XMP packet'
packet = base[at:]
tag = b'<ProjectXMPMetadata><![CDATA['
start = d.find(tag)
assert start >= 0
start += len(tag)
end = d.find(b']]></ProjectXMPMetadata>', start)
assert end > start
out = d[:start] + packet + d[end:]
# Local folder and machine names that must not remain (comma-separated, from the environment so
# that they are not written here): e.g. SCRUB_FORBID=<folder name>,<host name prefix>.
import os
for bad in [w.encode() for w in os.environ.get('SCRUB_FORBID', '').split(',') if w]:
    assert bad not in out, bad
open(dst, 'wb').write(out)
print('raw', len(out), 'zlib', len(zlib.compress(out, 9)))
