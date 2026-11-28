//! Selection helpers for Effect Controls (re-exported from `common`).
//!
//! Exists so the `mod selection;` declaration in the module root resolves; the
//! selection-clearing behavior an effect header's click triggers (#3828) lives in
//! `common::select_effect_only`.

pub(super) use super::common::select_effect_only;