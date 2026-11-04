use super::{EffectSpec, ParamUi, Value, slider};

pub(super) fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Generate", params, render, gpu: false, float: true }
}

pub(super) fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

pub(super) fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}
