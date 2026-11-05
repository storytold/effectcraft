# Byte edits of the text family (the old fixtures 732 and 733 are 517 with one paragraph slot
# changed in the file; AE scripting cannot set those slots). Finds the comp item by name, then
# the text document (LIST btdk) of its layer, and makes the same one-byte edit there.
# Usage: patch-text.py <in.aep> <out.aep>
import struct, sys

EDITS = {
    "732-text-box-byteedit-para24-0": (b"/23 true /24 1 /25", b"/23 true /24 0 /25"),
    "733-text-box-byteedit-para35-0": (b"/34 0 /35 1 /36", b"/34 0 /35 0 /36"),
}

src, dst = sys.argv[1], sys.argv[2]
d = bytearray(open(src, "rb").read())
assert d[:4] == b"RIFX" and d[8:12] == b"Egg!"


def chunks(start, end):
    """(id, list type or None, body start, body end) of the chunks in [start, end)."""
    p = start
    while p + 8 <= end:
        cid = bytes(d[p:p + 4])
        size = struct.unpack(">I", d[p + 4:p + 8])[0]
        body = p + 8
        if cid == b"LIST":
            yield cid, bytes(d[body:body + 4]), body + 4, body + size
        else:
            yield cid, None, body, body + size
        p = body + size + (size & 1)


def walk_items(start, end, out):
    for cid, ltype, b, e in chunks(start, end):
        if cid != b"LIST" or ltype == b"btdk":
            continue
        if ltype == b"Item":
            name = None
            for c2, _, b2, e2 in chunks(b, e):
                if c2 == b"Utf8":
                    name = bytes(d[b2:e2]).decode("utf-8", "replace")
                    break
            out.append((name, b, e))
        walk_items(b, e, out)


def btdks(start, end, out):
    for cid, ltype, b, e in chunks(start, end):
        if cid == b"LIST" and ltype == b"btdk":
            out.append((b, e))
        elif cid == b"LIST":
            btdks(b, e, out)


items = []
walk_items(12, len(d), items)
done = 0
for name, b, e in items:
    if name not in EDITS:
        continue
    old, new = EDITS[name]
    docs = []
    btdks(b, e, docs)
    assert len(docs) == 1, (name, len(docs))
    db, de = docs[0]
    body = bytes(d[db:de])
    n = body.count(old)
    assert n == 1, (name, n)
    at = db + body.index(old)
    d[at:at + len(old)] = new
    print(name, "patched at", at)
    done += 1
assert done == len(EDITS), done
open(dst, "wb").write(bytes(d))
