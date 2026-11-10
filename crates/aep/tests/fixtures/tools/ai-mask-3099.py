"""Writes hand-made one-page PDFs named .ai with distinct page boxes (issue 3099 repro)."""
import os
import sys

out = sys.argv[1]
os.makedirs(out, exist_ok=True)


def pdf(page_extra, content):
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R " + page_extra.encode() + b" /Contents 4 0 R >>",
        b"<< /Length " + str(len(content)).encode() + b" >>\nstream\n" + content + b"\nendstream",
    ]
    data = b"%PDF-1.4\n"
    offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(data))
        data += str(i).encode() + b" 0 obj\n" + o + b"\nendobj\n"
    x = len(data)
    data += b"xref\n0 " + str(len(objs) + 1).encode() + b"\n0000000000 65535 f \n"
    for o in offs:
        data += b"%010d 00000 n \n" % o
    data += b"trailer\n<< /Size " + str(len(objs) + 1).encode() + b" /Root 1 0 R >>\nstartxref\n" + str(x).encode() + b"\n%%EOF\n"
    return data


def boxes_content(boxes):
    # One outline per box (different colours) plus a filled marker at the media box origin corner.
    cols = ["0 0 0", "1 0 0", "0 0.6 0", "0 0 1", "1 0.5 0"]
    s = b"0.85 0.85 0.85 rg "
    x0, y0, x1, y1 = boxes[0]
    s += b"%g %g %g %g re f\n" % (x0, y0, x1 - x0, y1 - y0)
    for c, (a, b, cc, d) in zip(cols, boxes):
        s += c.encode() + b" RG 1 w %g %g %g %g re S\n" % (a + 0.5, b + 0.5, cc - a - 1, d - b - 1)
    # A solid square in the lower-left of the last (innermost) box: shows y orientation.
    a, b, _, _ = boxes[-1]
    s += b"0.2 0.2 0.8 rg %g %g 20 10 re f" % (a + 4, b + 4)
    return s


def arr(b):
    return "[%s]" % " ".join("%g" % v for v in b)


# A: every box distinct, nested, non-square.
media, crop, bleed, trim, art = [0, 0, 420, 220], [5, 5, 415, 215], [10, 10, 410, 210], [20, 20, 400, 200], [60, 50, 300.6, 170.4]
a = pdf(
    f"/MediaBox {arr(media)} /CropBox {arr(crop)} /BleedBox {arr(bleed)} /TrimBox {arr(trim)} /ArtBox {arr(art)}",
    boxes_content([media, crop, bleed, trim, art]),
)
# B: only a fractional media box.
mb = [0, 0, 300.6, 120.4]
b = pdf(f"/MediaBox {arr(mb)}", boxes_content([mb]))
# C: media box with an offset origin and /Rotate 90.
mc = [100, 50, 400, 150]
c = pdf(f"/MediaBox {arr(mc)} /Rotate 90", boxes_content([mc]))
# D: media + trim + art, no crop box (bleed around the artboard).
md, td, ad = [0, 0, 418, 218], [9, 9, 409, 209], [40, 30, 250, 150]
d = pdf(f"/MediaBox {arr(md)} /TrimBox {arr(td)} /BleedBox {arr(md)} /ArtBox {arr(ad)}", boxes_content([md, td, ad]))

for name, data in [("i3099-a-boxes.ai", a), ("i3099-b-frac.ai", b), ("i3099-c-rot.ai", c), ("i3099-d-trim.ai", d)]:
    with open(os.path.join(out, name), "wb") as f:
        f.write(data)
    print(name, len(data))
