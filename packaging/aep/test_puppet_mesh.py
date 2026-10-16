"""Puppet PEdt topology layouts decode to the mesh's own faces."""
import struct
import unittest
from aep_ir_export import Exporter

SQUARE = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
FACES = [(0, 1, 2), (0, 2, 3)]


def pedt(stream):
    head = b"\x0a\xd0\xbe\xef\x00\x00\x00\x02\x01" + b"\x00" * 4
    head += struct.pack(">4I", len(SQUARE), 3 * len(FACES), 0, 0)
    body = b"".join(struct.pack(">2d", *p) for p in SQUARE)
    return head + body + struct.pack(f">{len(stream)}I", *stream)


class PuppetMeshTest(unittest.TestCase):
    def test_plain_faces(self):
        mesh = Exporter.puppet_mesh(pedt([i for f in FACES for i in f]))
        self.assertEqual([tuple(t) for t in mesh["triangles"]], FACES)

    def test_rotated_pairs_are_one_face_each(self):
        # SceneYard Hand balance collage: (a,b,c,b,c,a) per face; reading it as plain faces
        # kept half of them, each twice, and the hand rendered as fragments.
        stream = [i for a, b, c in FACES for i in (a, b, c, b, c, a)]
        mesh = Exporter.puppet_mesh(pedt(stream))
        self.assertEqual([tuple(t) for t in mesh["triangles"]], FACES)
        self.assertEqual(len({i for t in mesh["triangles"] for i in t}), len(SQUARE))


if __name__ == "__main__":
    unittest.main()
