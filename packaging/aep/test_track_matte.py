"""Track mattes: AE 23+ names the matte layer; a Track Matte type with no layer is no matte."""
import unittest
from types import SimpleNamespace
from aep_ir_export import Exporter


class TrackMatteTest(unittest.TestCase):
    def test_versions_that_name_matte_layers(self):
        self.assertTrue(Exporter.names_matte_layers("25.5x4"))
        self.assertTrue(Exporter.names_matte_layers("23.0x59"))
        self.assertFalse(Exporter.names_matte_layers("22.6x61"))
        self.assertFalse(Exporter.names_matte_layers(None))

    def test_type_without_a_layer_is_no_matte_in_ae23_projects(self):
        # SceneYard "Retro Desktop Movie Popups", comp "movie 1": the picture's Track Matte
        # type is Alpha with no layer; py-aep offered the "text" layer above it.
        above = SimpleNamespace(name="text")
        layer = SimpleNamespace(_matte_layer_id=0)
        self.assertIsNone(Exporter.track_matte(layer, above, True))
        # Pre-23 projects: the layer above is the matte.
        self.assertIs(Exporter.track_matte(layer, above, False), above)

    def test_named_matte_layer_is_kept(self):
        matte = SimpleNamespace(name="Movie pic Matte")
        self.assertIs(Exporter.track_matte(SimpleNamespace(_matte_layer_id=464), matte, True), matte)
        # Unknown parser internals: keep what py-aep resolved.
        self.assertIs(Exporter.track_matte(SimpleNamespace(), matte, True), matte)


if __name__ == "__main__":
    unittest.main()
