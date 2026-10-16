"""Effect parameters AE left out of the file take the plugin default, not another instance's."""
import unittest
from types import SimpleNamespace
from aep_ir_export import Exporter


def param(value, default, live=False, parade=True, effect_name="ADBE Tint", match_name="ADBE Tint-0002"):
    parade_group = SimpleNamespace(match_name="ADBE Effect Parade", parent_property=None)
    effect = SimpleNamespace(match_name=effect_name, parent_property=parade_group if parade else None)
    return SimpleNamespace(value=value, default_value=default, last_value=value, parent_property=effect,
                           match_name=match_name, _is_live=lambda: live)


DARK_BLUE = [7 / 255, 7 / 255, 60 / 255, 1.0]
WHITE = [1.0, 1.0, 1.0, 0.0]


class EffectDefaultTest(unittest.TestCase):
    def test_untouched_parameter_takes_the_plugin_default(self):
        # SceneYard Student Computer Workflow: an untouched Tint's Map White To read as another
        # Tint's dark blue (the project's shared last value); aerender draws it white.
        self.assertEqual(Exporter.effect_default(param(DARK_BLUE, WHITE)), WHITE)

    def test_authored_and_non_effect_values_are_kept(self):
        self.assertIsNone(Exporter.effect_default(param(DARK_BLUE, WHITE, live=True)))
        self.assertIsNone(Exporter.effect_default(param(DARK_BLUE, WHITE, parade=False)))
        self.assertIsNone(Exporter.effect_default(param(34.0, None)))
        self.assertIsNone(Exporter.effect_default(param(34.0, 34.0)))


    def test_popup_and_slider_defaults_come_from_the_parameter_definition(self):
        # SceneYard Paper News Highlight: py-aep reports an untouched CC Page Turn Controls and
        # Back Opacity as another instance's 3 and 100; AE draws them at 6 and 85.
        definitions = {"CC Page Turn": {
            "CC Page Turn-0008": {"property_control_type": 7, "default_value": 0, "last_value": 3},
            "CC Page Turn-0009": {"property_control_type": 2, "default": 85, "last_value": 100.0}},
            "ADBE Glo2": {"ADBE Glo2-0005": {"property_control_type": 7, "default_value": 0, "last_value": 2}}}
        controls = param(3, 3, effect_name="CC Page Turn", match_name="CC Page Turn-0008")
        opacity = param(100.0, 100.0, effect_name="CC Page Turn", match_name="CC Page Turn-0009")
        self.assertEqual(Exporter.effect_default(controls, definitions), 6)
        self.assertEqual(Exporter.effect_default(opacity, definitions), 85)
        self.assertIsNone(Exporter.effect_default(opacity, {}))
        # pard's popup index (0) is not a default: Glow's Composite Original stays py-aep's 2.
        composite = param(2, 2, effect_name="ADBE Glo2", match_name="ADBE Glo2-0005")
        self.assertIsNone(Exporter.effect_default(composite, definitions))

    def test_only_a_parameter_another_instance_sets_is_replaced(self):
        # Levels / Hue/Saturation keep their real state in untouched-looking parameters: they are
        # replaced only when another instance of the effect sets that parameter.
        definitions = {"ADBE Tint": {"ADBE Tint-0002": {"property_control_type": 5, "default_value": WHITE}}}
        untouched = param(DARK_BLUE, DARK_BLUE)
        leaked = {("ADBE Tint", "ADBE Tint-0002"): {Exporter.value_key([0.0277, 0.0277, 0.2362, 1.0])}}
        self.assertEqual(Exporter.effect_default(untouched, definitions, leaked), WHITE)
        self.assertIsNone(Exporter.effect_default(untouched, definitions, {}))

    def test_keyframed_parameters_count_as_set_by_another_instance(self):
        # SceneYard Billiard Balls: the green ball keys CC Sphere Rotation X to 13; the other
        # balls' untouched Rotation X reads 13 and must take the plugin default 0.
        keyed = SimpleNamespace(match_name="CC Sphere-0002", value=-11.0, expression="",
                                keyframes=[SimpleNamespace(value=-11.0), SimpleNamespace(value=13.0)],
                                _is_live=lambda: True)
        expressed = SimpleNamespace(match_name="CC Sphere-0003", value=0.0, expression="time",
                                    keyframes=[], _is_live=lambda: True)

        class Effect(list):
            match_name = "CC Sphere"

        project = SimpleNamespace(compositions=[SimpleNamespace(layers=[SimpleNamespace(effects=[Effect([keyed, expressed])])])])
        authored = Exporter.authored_effect_values(project)
        self.assertEqual(authored, {("CC Sphere", "CC Sphere-0002"): {-11.0, 13.0}})
        definitions = {"CC Sphere": {"CC Sphere-0002": {"property_control_type": 3, "default": 0}}}
        untouched = param(13.0, 13.0, effect_name="CC Sphere", match_name="CC Sphere-0002")
        self.assertEqual(Exporter.effect_default(untouched, definitions, authored), 0)


if __name__ == "__main__":
    unittest.main()
