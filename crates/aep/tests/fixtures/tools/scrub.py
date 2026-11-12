# Fixture hygiene for a combined file, as done for the single fixtures: footage alias JSON with a
# same-length neutral path, host name HOST000, neutral ascend counts; XMP replaced by the base
# project's packet. Usage: scrub.py <in.aep> <out.aep> <000-base.aep>
import re, sys, zlib
src, dst, base_path = sys.argv[1], sys.argv[2], sys.argv[3]
d = bytes(open(src, 'rb').read())
BS2 = bytes([92, 92])  # a JSON-escaped backslash

def fix(m):
    js = m.group(0)
    full = re.search(rb'"fullpath":"(.*?)"', js).group(1)
    name = full.split(BS2)[-1]
    pre = b'C:' + BS2 + b'fixtures' + BS2
    mid = len(full) - len(pre) - len(BS2) - len(name)
    neutral = pre + (b'dir' * 200)[:mid] + BS2 + name
    out = js.replace(full, neutral)
    out = re.sub(rb'"server_name":"([^"]*)"', lambda n: b'"server_name":"' + (b'HOST' + b'0' * 64)[:len(n.group(1))] + b'"', out)
    out = re.sub(rb'"ascendcount_base":\d', b'"ascendcount_base":1', out)
    out = re.sub(rb'"ascendcount_target":\d', b'"ascendcount_target":2', out)
    assert len(out) == len(js)
    return out

d, n = re.subn(rb'\{"ascendcount_base".*?\}', fix, d)
print('aliases scrubbed', n)
base = open(base_path, 'rb').read()
xi = d.find(b'<?xpacket begin')
xj = base.find(b'<?xpacket begin')
assert xi >= 0, 'the saved project has no XMP packet'
assert xj >= 0, 'the base project has no XMP packet'
out = d[:xi] + base[xj:]
# Local folder and machine names that must not remain (comma-separated, from the environment so
# that they are not written here): e.g. SCRUB_FORBID=<folder name>,<host name prefix>.
import os
for bad in [w.encode() for w in os.environ.get('SCRUB_FORBID', '').split(',') if w]:
    assert bad not in out, bad
open(dst, 'wb').write(out)
print('raw', len(out), 'zlib', len(zlib.compress(out, 9)))
