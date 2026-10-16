"""SceneYard Puppet Control Documentary: the right puppeteer hand sat ~970 px too deep because
py-aep reads an AV layer's Anchor Point z in units of its source height (1.238 x 1024 px)."""
import unittest
from aep_ir_export import Exporter


def props(anchor, keys=()):
    return [{"match_name": "ADBE Transform Group", "children": [
        {"match_name": "ADBE Anchor Point", "value": anchor,
         "keyframes": [{"value": v, "in_spatial_tangent": [0, 0, 1], "out_spatial_tangent": [0, 0, -1]} for v in keys]}]}]


class AvAnchorDepthTest(unittest.TestCase):
    def anchor(self, p):
        return p[0]["children"][0]

    def test_z_scales_by_source_height(self):
        p = props([861.9, -149.8, 1.25])
        Exporter.av_anchor_depth(p, 1024)
        self.assertEqual(self.anchor(p)["value"], [861.9, -149.8, 1280.0])
        self.assertEqual(self.anchor(p)["value_origin"], "ae_av_anchor_z_height")

    def test_keyframes_and_tangents(self):
        p = props([0, 0, 0], keys=[[0, 0, 0.5], [1, 1, 0.25]])
        Exporter.av_anchor_depth(p, 100)
        keys = self.anchor(p)["keyframes"]
        self.assertEqual([k["value"][2] for k in keys], [50.0, 25.0])
        self.assertEqual(keys[0]["in_spatial_tangent"], [0, 0, 100])

    def test_flat_anchor_untouched(self):
        p = props([50.0, 50.0, 0.0])
        Exporter.av_anchor_depth(p, 100)
        self.assertEqual(self.anchor(p)["value"], [50.0, 50.0, 0.0])
        self.assertNotIn("value_origin", self.anchor(p))

    def test_unknown_height_untouched(self):
        for h in (None, 0, float("nan")):
            p = props([0.0, 0.0, 2.0])
            Exporter.av_anchor_depth(p, h)
            self.assertEqual(self.anchor(p)["value"], [0.0, 0.0, 2.0])


if __name__ == "__main__":
    unittest.main()
