//! Variable fonts: a face's variation axes and glyph outlines at a point of its design space
//! (Animate Text ▸ Variable Font Axes).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use kurbo::BezPath;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::DrawSettings;
use skrifa::{GlyphId, MetadataProvider, Tag};

use crate::fonts::{self, FaceId};
use crate::{CharGlyph, Pen};

/// Intern a set of axis values (0 for none), so `Copy` glyphs can carry them.
pub fn intern(coords: &[(String, f32)]) -> u32 {
    if coords.is_empty() {
        return 0;
    }
    let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = t.iter().position(|c| c.as_slice() == coords) {
        return i as u32 + 1;
    }
    t.push(Arc::new(coords.to_vec()));
    t.len() as u32
}

/// The axis values of an interned id (empty for 0).
pub fn coords(id: u32) -> Arc<Vec<(String, f32)>> {
    if id == 0 {
        return Arc::new(Vec::new());
    }
    table().lock().unwrap_or_else(|e| e.into_inner()).get(id as usize - 1).cloned().unwrap_or_default()
}

fn table() -> &'static Mutex<Vec<Arc<Vec<(String, f32)>>>> {
    static T: OnceLock<Mutex<Vec<Arc<Vec<(String, f32)>>>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// One variation axis of a face, in user units.
#[derive(Clone, Debug, PartialEq)]
pub struct FontAxis {
    /// Four-letter tag (`wght`, `wdth`, `opsz`, `slnt`, `ital` or a custom one).
    pub tag: String,
    pub name: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// The variation axes of a face (empty for static fonts).
pub fn font_axes(face: FaceId) -> Vec<FontAxis> {
    let f = fonts::face(face);
    let Some(font) = f.font() else { return Vec::new() };
    font.axes()
        .iter()
        .map(|a| {
            let tag = String::from_utf8_lossy(&a.tag().to_be_bytes()).to_string();
            let name = font.localized_strings(a.name_id()).english_or_first().map(|s| s.to_string()).unwrap_or_else(|| tag.clone());
            FontAxis { tag, name, min: a.min_value(), default: a.default_value(), max: a.max_value() }
        })
        .collect()
}

/// A tag string as a [`Tag`] (padded with spaces, at most four characters).
pub(crate) fn tag_of(s: &str) -> Tag {
    let mut b = [b' '; 4];
    for (i, c) in s.bytes().take(4).enumerate() {
        b[i] = c;
    }
    Tag::new(&b)
}

type Key = (FaceId, u32, Vec<(String, i32)>);

/// A glyph's outline in font units at the given axis values (user units, unspecified axes at
/// their defaults).
pub fn outline_units_at(face: FaceId, gid: u32, coords: &[(String, f32)]) -> Option<Arc<BezPath>> {
    if coords.is_empty() {
        return crate::outline_units(face, gid);
    }
    static C: OnceLock<Mutex<HashMap<Key, Option<Arc<BezPath>>>>> = OnceLock::new();
    let c = C.get_or_init(Default::default);
    // Quantise to 1/100 of a unit so animation frames reuse outlines.
    let key: Key = (face, gid, coords.iter().map(|(t, v)| (t.clone(), (v * 100.0).round() as i32)).collect());
    if let Some(v) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let f = fonts::face(face);
    let v = f.font().and_then(|font| {
        let loc = font.axes().location(coords.iter().map(|(t, v)| (tag_of(t), *v)));
        let g = font.outline_glyphs().get(GlyphId::new(gid))?;
        let mut pen = Pen(BezPath::new());
        g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::from(&loc)), &mut pen).ok()?;
        Some(Arc::new(pen.0))
    });
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 20_000 {
        m.clear();
    }
    m.insert(key, v.clone());
    v
}

/// A laid-out character's outline (same placement as [`CharGlyph::path`]) with its face's axes
/// moved by `deltas` (tag, user-unit offset from the character's own axis value — its style's
/// Variable Font Axes setting, else the axis default).
pub fn char_outline_varied(g: &CharGlyph, deltas: &[(String, f32)]) -> Option<BezPath> {
    let axes = font_axes(g.face);
    let base = |t: &str, d: f32| g.variations.iter().find(|(x, _)| x.trim_end() == t.trim_end()).map(|(_, v)| *v).unwrap_or(d);
    let mut coords: Vec<(String, f32)> =
        deltas.iter().filter_map(|(t, d)| axes.iter().find(|a| a.tag == *t).map(|a| (t.clone(), (base(&a.tag, a.default) + d).clamp(a.min, a.max)))).collect();
    if coords.is_empty() {
        return None;
    }
    for (t, v) in &g.variations {
        if !coords.iter().any(|(c, _)| c == t) {
            coords.push((t.clone(), *v));
        }
    }
    let units = outline_units_at(g.face, g.gid, &coords)?;
    Some(g.outline_xf * (*units).clone())
}

/// A glyph's advance in font units at the given axis values (user units, unspecified axes at
/// their defaults).
pub fn advance_units_at(face: FaceId, gid: u32, coords: &[(String, f32)]) -> Option<f32> {
    let f = fonts::face(face);
    let font = f.font()?;
    let loc = font.axes().location(coords.iter().map(|(t, v)| (tag_of(t), *v)));
    font.glyph_metrics(Size::unscaled(), LocationRef::from(&loc)).advance_width(GlyphId::new(gid))
}

/// The change of a laid-out character's advance (layout units) when its face's axes move by
/// `deltas` (as [`char_outline_varied`]): Variable Font Axes animators re-space the text.
pub fn char_advance_delta(g: &CharGlyph, deltas: &[(String, f32)]) -> Option<f64> {
    let axes = font_axes(g.face);
    let base = |t: &str, d: f32| g.variations.iter().find(|(x, _)| x.trim_end() == t.trim_end()).map(|(_, v)| *v).unwrap_or(d);
    let mut coords: Vec<(String, f32)> =
        deltas.iter().filter_map(|(t, d)| axes.iter().find(|a| a.tag == *t).map(|a| (t.clone(), (base(&a.tag, a.default) + d).clamp(a.min, a.max)))).collect();
    if coords.is_empty() {
        return None;
    }
    for (t, v) in &g.variations {
        if !coords.iter().any(|(c, _)| c == t) {
            coords.push((t.clone(), *v));
        }
    }
    let varied = advance_units_at(g.face, g.gid, &coords)?;
    let own = advance_units_at(g.face, g.gid, &g.variations)?;
    // Font units → layout units: the outline transform's horizontal scale.
    let sx = g.outline_xf.as_coeffs()[0];
    Some((varied - own) as f64 * sx)
}

/// The first installed face with variation axes (system fonts are scanned), for tests and
/// demos: (face, its axes).
pub fn find_variable_face() -> Option<(FaceId, Vec<FontAxis>)> {
    fonts::scan_system();
    (0..fonts::all_faces().len()).map(|f| (f, font_axes(f))).find(|(_, a)| a.iter().any(|x| x.max > x.min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn static_fonts_have_no_axes_and_variable_outlines_move() {
        let inter = crate::resolve("Inter", "Regular").face;
        assert!(font_axes(inter).is_empty());
        // A variable system font, when one is installed (macOS ships several): its outline
        // changes along an axis. Skipped on machines without one.
        let Some((face, axes)) = find_variable_face() else {
            eprintln!("no variable font installed: outline check skipped");
            return;
        };
        let a = axes.iter().find(|x| x.max > x.min).unwrap();
        let gid = fonts::face(face).glyph('H').or_else(|| fonts::face(face).glyph('A')).unwrap_or(1);
        let lo = outline_units_at(face, gid, &[(a.tag.clone(), a.min)]).unwrap();
        let hi = outline_units_at(face, gid, &[(a.tag.clone(), a.max)]).unwrap();
        assert_ne!(lo.area(), hi.area(), "{} {}..{}", a.tag, a.min, a.max);
        // The default location is the static outline.
        let d = outline_units_at(face, gid, &[(a.tag.clone(), a.default)]).unwrap();
        let s = crate::outline_units(face, gid).unwrap();
        assert!((d.area() - s.area()).abs() < 1e-3);
    }

    #[test]
    fn style_variations_change_advances_and_outlines() {
        assert_eq!(intern(&[]), 0);
        let id = intern(&[("wght".into(), 700.0)]);
        assert_eq!(intern(&[("wght".into(), 700.0)]), id);
        assert_eq!(coords(id).as_slice(), &[("wght".to_string(), 700.0)]);
        let Some((face, axes)) = find_variable_face() else {
            eprintln!("no variable font installed: skipped");
            return;
        };
        let info = crate::fonts::face(face).info.clone();
        // An axis that moves the advance: `wdth` and `wght` usually do, but a monospaced family
        // keeps its advances fixed along `wght` (Google Sans Code does) and moves them along its
        // own axis instead, so try the axes in order and take the first one that moves. A face
        // with no axis carrying a range is the only case that skips: a variation path that broke
        // completely moves neither the advances nor the outlines, and that has to fail below.
        let st = |a: &FontAxis, v: f32| crate::TextStyle {
            family: info.family.clone(),
            style: info.style.clone(),
            size: 100.0,
            variations: vec![(a.tag.clone(), v)],
            ..Default::default()
        };
        let ranged: Vec<&FontAxis> = axes.iter().filter(|a| a.max > a.min).collect();
        if ranged.is_empty() {
            eprintln!("no variable axis installed here carries a range: skipped");
            return;
        }
        let mut moving = None;
        for a in &ranged {
            let (lo, hi) = (crate::layout::measure("Hamburgefonts", &st(a, a.min)), crate::layout::measure("Hamburgefonts", &st(a, a.max)));
            if (lo - hi).abs() > 1.0 {
                moving = Some(((*a).clone(), lo, hi));
                break;
            }
        }
        if crate::resolve(&info.family, &info.style).face != face {
            eprintln!("variable face not reachable by family/style: skipped");
            return;
        }
        let ink = |a: &FontAxis, v: f32| {
            let l = crate::layout::layout("H", &st(a, v), &crate::ParagraphStyle::default());
            l.glyphs.iter().map(|g| crate::glyph_outline(g).area().abs()).sum::<f64>()
        };
        if let Some((a, lo, hi)) = moving {
            // The axis was picked because it moves the advance; the outline has to follow it too.
            assert!((lo - hi).abs() > 1.0, "{} {}..{}: advances {lo} vs {hi}", a.tag, a.min, a.max);
            let (i_lo, i_hi) = (ink(&a, a.min), ink(&a, a.max));
            assert!((i_lo - i_hi).abs() > 1.0, "{} {}..{}: outlines do not move", a.tag, a.min, a.max);
        } else {
            // Nothing here moves an advance. Holding the advances is what a monospaced family does
            // on purpose, but it is also what a broken variation path does, so this case still has
            // to fail rather than skip: the outline must move along `wght`, or, on a face without
            // one, along the first axis that carries a range.
            let a = axes.iter().find(|a| a.tag == "wght" && a.max > a.min).unwrap_or(ranged[0]);
            let (i_lo, i_hi) = (ink(a, a.min), ink(a, a.max));
            assert!((i_lo - i_hi).abs() > 1.0, "{} {}..{}: neither the advances nor the outlines move, so variation support looks broken", a.tag, a.min, a.max);
        }
    }
}
