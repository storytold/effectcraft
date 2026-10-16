//! Narrow import-loss policy for controls saved with inert values in the AE corpus.
//! The importer still stores these raw properties so they remain editable.

use serde_json::{Value as Json, json};

fn static_value(property: &Json) -> bool {
    property["keyframes"].as_array().is_some_and(Vec::is_empty)
        && property["expression"].as_str() == Some("")
        && property["expression_enabled"].as_bool() == Some(false)
}

fn property<'a>(effect: &'a Json, raw: &str) -> Option<&'a Json> {
    effect["children"].as_array()?.iter().find(|p| p["match_name"].as_str() == Some(raw))
}

fn fixed(effect: &Json, raw: &str, value: Json) -> bool {
    property(effect, raw).is_some_and(|p| static_value(p) && same(&p["value"], &value))
}

fn empty(effect: &Json, raw: &str) -> bool {
    property(effect, raw).is_some_and(|p| static_value(p) && p["value"].is_null())
}

fn same(actual: &Json, expected: &Json) -> bool {
    match (actual.as_f64(), expected.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => actual == expected,
    }
}

fn lumetri(raw: &str, child: &Json, effect: &Json) -> bool {
    if !static_value(child) {
        return false;
    }
    let value = &child["value"];
    // These are saved UI selectors, disabled automatic tools, or duplicate defaults.
    // Keep any changed value visible until its AE rendering semantics are established.
    match raw {
        "ADBE Lumetri-0127" => same(value, &json!(1)) && fixed(effect, "ADBE Lumetri-0005", json!(1)) && empty(effect, "ADBE Lumetri-0125"),
        "ADBE Lumetri-0128" => same(value, &json!(1)) && fixed(effect, "ADBE Lumetri-0025", json!(1)) && empty(effect, "ADBE Lumetri-0126"),
        "ADBE Lumetri-0123" => same(value, &json!(50)) && fixed(effect, "ADBE Lumetri-0124", json!(0)),
        "ADBE Lumetri-0094" => {
            same(value, &json!([0.7529411764705882, 0.7529411764705882, 0.7529411764705882, 1.0]))
                && fixed(effect, "ADBE Lumetri-0007", json!(0))
                && fixed(effect, "ADBE Lumetri-0008", json!(0))
        }
        "ADBE Lumetri-0130" => same(value, &json!(26)) && fixed(effect, "ADBE Lumetri-0025", json!(1)) && empty(effect, "ADBE Lumetri-0126"),
        "ADBE Lumetri-0065" | "ADBE Lumetri-0066" => same(value, &json!(100)) && fixed(effect, "ADBE Lumetri-0062", json!(0)),
        "ADBE Lumetri-0113" | "ADBE Lumetri-0114" | "ADBE Lumetri-0115" | "ADBE Lumetri-0116" | "ADBE Lumetri-0117" => {
            same(value, &json!([0.0, 0.0, 0.00392156862745098, 1.0]))
        }
        "ADBE Lumetri-0102" => same(value, &json!(1)) && fixed(effect, "ADBE Lumetri-0124", json!(0)),
        "ADBE Lumetri-0075" => same(value, &json!(1)),
        "ADBE Lumetri-0057" => same(value, &json!(1)) && fixed(effect, "ADBE Lumetri-0056", json!(0)),
        "ADBE Lumetri-0070" | "ADBE Lumetri-0071" | "ADBE Lumetri-0072" => same(value, &json!([0.0, 0.0, 0.0, 1.0])),
        "ADBE Lumetri-0097" | "ADBE Lumetri-0124" => same(value, &json!(0)),
        "ADBE Lumetri-0121" => same(value, &json!(-1)) && fixed(effect, "ADBE Lumetri-0124", json!(0)),
        _ => false,
    }
}

pub(crate) fn inert(effect_id: &str, raw: &str, child: &Json, effect: &Json) -> bool {
    match effect_id {
        "ec.color.lumetri" => lumetri(raw, child, effect),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(raw: &str, value: Json) -> Json {
        json!({"match_name":raw,"value":value,"keyframes":[],"expression":"","expression_enabled":false})
    }

    #[test]
    fn lumetri_exact_defaults_only() {
        let effect = json!({"children":[p("ADBE Lumetri-0005",json!(1)),p("ADBE Lumetri-0025",json!(1)),p("ADBE Lumetri-0062",json!(0)),p("ADBE Lumetri-0125",Json::Null),p("ADBE Lumetri-0126",Json::Null)]});
        let idle = p("ADBE Lumetri-0065", json!(100));
        assert!(inert("ec.color.lumetri", "ADBE Lumetri-0065", &idle, &effect));
        assert!(!inert("ec.color.lumetri", "ADBE Lumetri-0065", &p("ADBE Lumetri-0065", json!(101)), &effect));
        assert!(!inert("ec.color.lumetri", "ADBE Lumetri-0065", &idle, &json!({"children":[p("ADBE Lumetri-0062",json!(1))]})));
        let mut keyed = idle.clone();
        keyed["keyframes"] = json!([{"time":0}]);
        assert!(!inert("ec.color.lumetri", "ADBE Lumetri-0065", &keyed, &effect));
        let mut expressed = idle.clone();
        expressed["expression"] = json!("100");
        assert!(!inert("ec.color.lumetri", "ADBE Lumetri-0065", &expressed, &effect));
        assert!(inert("ec.color.lumetri", "ADBE Lumetri-0127", &p("ADBE Lumetri-0127", json!(1)), &effect));
        assert!(!inert("ec.color.lumetri", "ADBE Lumetri-0127", &p("ADBE Lumetri-0127", json!(2)), &effect));
    }
}
