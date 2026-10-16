"""Saved source bindings remain available without opening/relinking the artwork."""
import struct
import unittest
from types import SimpleNamespace
from aep_ir_export import Exporter


class SourceLayerTest(unittest.TestCase):
    def source(self, index=1, bounds=(20.25, 20.5, 40.5, 60.75)):
        data = bytearray(596)
        struct.pack_into(">4i", data, 16, *(round(v * 65536) for v in bounds))
        return SimpleNamespace(_opti=SimpleNamespace(text_layer_name="Art", text_document_layers=2,
                               tobytes=lambda: bytes(data)), _sspc=SimpleNamespace(layer_index=index))

    def test_fractional_offline_binding(self):
        self.assertEqual(Exporter.vector_source_layer(self.source()),
                         {"index":1, "name":"Art", "layer_size":True,
                          "crop_bounds":[20.25,20.5,40.5,60.75]})

    def test_whole_document_is_not_a_layer(self):
        self.assertIsNone(Exporter.vector_source_layer(SimpleNamespace()))

    def test_invalid_saved_binding_is_not_silently_dropped(self):
        for source in [self.source(index=4294967295), self.source(bounds=(20,0,10,10))]:
            with self.assertRaises(ValueError):
                Exporter.vector_source_layer(source)


class PhotoshopLayerTest(unittest.TestCase):
    """SceneYard Patient ID: "Clipboard" and "stick" are layers 0 and 1 of one PSB; both drew
    the merged document (the clip became a big white sheet)."""
    def opti(self, **kw):
        base = dict(asset_type="8BPS", psd_flattened=False, psd_layer_count=2, psd_layer_index=1,
                    psd_group_name="Layer 1", psd_layer_left=449, psd_layer_top=139,
                    psd_layer_right=1065, psd_layer_bottom=441, psd_canvas_width=1549,
                    psd_canvas_height=2132)
        base.update(kw)
        return SimpleNamespace(_opti=SimpleNamespace(**base))

    def test_document_size_layer(self):
        self.assertEqual(Exporter.psd_source_layer(self.opti(), 1549, 2132),
                         {"index": 1, "name": "Layer 1", "layer_size": False})

    def test_layer_size_layer(self):
        binding = Exporter.psd_source_layer(self.opti(), 616, 302)
        self.assertTrue(binding["layer_size"])
        self.assertEqual(binding["crop_bounds"], [449.0, 139.0, 1065.0, 441.0])
        self.assertNotIn("crop_bounds", Exporter.psd_source_layer(self.opti(), 1549, 2132))

    def test_merged_import_is_not_a_layer(self):
        self.assertIsNone(Exporter.psd_source_layer(self.opti(psd_flattened=True), 1549, 2132))
        self.assertIsNone(Exporter.psd_source_layer(SimpleNamespace(), 1549, 2132))

    def test_invalid_index_is_reported(self):
        with self.assertRaises(ValueError):
            Exporter.psd_source_layer(self.opti(psd_layer_index=2), 1549, 2132)


if __name__ == "__main__":
    unittest.main()
