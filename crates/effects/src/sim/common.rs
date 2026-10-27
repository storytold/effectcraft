use super::*;

pub(crate) fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Simulation", params, render, gpu: false, float: true }
}

pub(crate) fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

pub(crate) fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

/// Deterministic hash → [0, 1).
#[inline]
pub(crate) fn h(i: u32, k: u32, seed: u32) -> f32 {
    hash1(i, k, seed)
}

/// Signed hash → [-1, 1).
#[inline]
pub(crate) fn hs(i: u32, k: u32, seed: u32) -> f32 {
    hash1(i, k, seed) * 2.0 - 1.0
}
