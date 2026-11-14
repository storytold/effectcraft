//! After Effects' Puppet effect (`ADBE FreePin3`) arrives as EffectCraft's Puppet
//! (`ec.distort.puppet`) in its place in the stack, with its name and fx switch, but **without
//! its meshes and pins**: how After Effects stores a pin's rest place (a vertex index and offset
//! into the hidden mesh data) and its Position (pixels or a fraction of the layer) is not decoded
//! yet, and a guessed rest place would deform the layer wrongly without a word. What the instance
//! holds is reported instead, mesh by mesh: Triangles, Expansion, and per pin container (Deform,
//! Stiffness, Overlap) the number of pins, how many are keyed or carry an expression, and the
//! match names of their atoms (Advanced and Bend pins show up there). Any entry this reader does
//! not know is named verbatim.
//!
//! The tree read here (docs/aep-format.md 3.7, Puppet) comes from an external scripting listing
//! of the effect's property tree, not from the corpus (no corpus file has a Puppet): **?** until a
//! byte sample exists. It is read with the same verified group reader as masks, shapes and layer
//! styles; a file whose tree differs is reported, never guessed at.

use effectcraft_project::build::Ids;
use effectcraft_project::{Node, PropGroup};

use crate::import::effects::{LayerCtx, carries_nothing};
use crate::import::report::{Collector, LossCategory};
use crate::{Effect as AepEffect, EffectDefinitions, ParamValue, PropGroup as AepGroup, PropNode, Property as AepProperty};

/// After Effects' Puppet effect.
pub(crate) const MATCH_NAME: &str = "ADBE FreePin3";
/// The hidden group that holds the meshes (`arap`).
const ARAP: &str = "ADBE FreePin3 ARAP Group";
/// Auto-traced Shapes, beside the meshes.
const OUTLINES: &str = "ADBE FreePin3 Outlines";
/// The hidden group of the meshes.
const MESH_GROUP: &str = "ADBE FreePin3 Mesh Group";
/// One mesh (`Mesh 1`).
const MESH_ATOM: &str = "ADBE FreePin3 Mesh Atom";
/// A mesh's hidden triangulation.
const MESH_DATA: &str = "ADBE FreePin3 Mesh";
const TRIANGLES: &str = "ADBE FreePin3 Mesh Tri Count";
const EXPANSION: &str = "ADBE FreePin3 Mesh Expansion";
/// At most this many meshes, pin atom names or unrecognised names are listed in a report line;
/// the rest are counted (`and 3 more`).
const LIST_CAP: usize = 10;
/// The pin containers of a mesh: match name and the name After Effects shows.
const PIN_GROUPS: [(&str, &str); 3] = [("ADBE FreePin3 PosPins", "Deform"), ("ADBE FreePin3 HghtPins", "Overlap"), ("ADBE FreePin3 StarchPins", "Stiffness")];

/// Imports a Puppet instance (the caller checked the match name) into `target`.
pub(crate) fn import_puppet<'f, 'a>(
    defs: &EffectDefinitions<'f, 'a>,
    fx: &AepEffect<'f, 'a>,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    target: &mut PropGroup,
    col: &mut Collector,
) {
    let shown = fx.display_name().unwrap_or_else(|| fx.match_name().to_string());
    let here = format!("{}: '{}' effect '{shown}' ({MATCH_NAME})", cx.comp, cx.layer);
    let Some(spec) = effectcraft_effects::find(effectcraft_effects::puppet::ID) else {
        col.lose(LossCategory::Effects, format!("{}: '{}' has the effect '{shown}' ({MATCH_NAME})", cx.comp, cx.layer));
        return;
    };
    let size = [f64::from(cx.size.0), f64::from(cx.size.1)];
    let mut group = effectcraft_effects::instantiate(spec, ids, &shown, size);
    match fx.enabled() {
        Some(on) => group.enabled = on,
        None => col.lose(LossCategory::DamagedData, format!("{}: '{}': the fx switch of '{shown}' is unreadable, imported switched on", cx.comp, cx.layer)),
    }
    // The ordinary parameters (`parT`): none is mapped yet, so a stored one is reported.
    let params = defs.params(fx);
    for p in &params {
        if carries_nothing(p) {
            continue;
        }
        if p.stored().is_some() && !matches!(p.stored_value(), Some(ParamValue::Layer(0))) {
            let label = if p.name.is_empty() { p.match_name.clone() } else { format!("{} ({})", p.name, p.match_name) };
            col.lose(LossCategory::Effects, format!("{here}: parameter {label} not imported"));
        }
    }
    report_meshes(fx, &params.iter().map(|p| p.match_name.as_str()).collect::<Vec<_>>(), &here, col);
    // The effect itself arrives (an EffectCraft Puppet the user can pin again), so it counts as
    // imported; what it held is in the losses above.
    col.imported.effects = col.imported.effects.saturating_add(1);
    target.children.push(Node::Group(group));
}

/// Reports the meshes and pins of `fx` and every entry of its value group that is neither a
/// `parT` parameter (`param_names`) nor the mesh tree.
fn report_meshes(fx: &AepEffect<'_, '_>, param_names: &[&str], here: &str, col: &mut Collector) {
    let mut unknown: Vec<String> = Vec::new();
    let values = fx.chunk().child_list(b"tdgp").and_then(AepGroup::from_chunk);
    let mut arap = None;
    for e in values.map(|g| g.entries()).unwrap_or_default() {
        if e.match_name == ARAP {
            if let PropNode::Group(g) = e.node {
                if arap.is_none() {
                    arap = Some(g);
                }
            } else {
                push_unique(&mut unknown, e.match_name);
            }
        } else if !param_names.contains(&e.match_name.as_str()) {
            match e.node {
                // A value outside `parT` (On Transparent, ...): reported with what it holds.
                PropNode::Property(p) => col.lose(LossCategory::Effects, format!("{here}: property {} ({}) not imported", e.match_name, describe(&p))),
                PropNode::Group(_) | PropNode::Other(_) => push_unique(&mut unknown, e.match_name),
            }
        }
    }
    let mut meshes: Vec<String> = Vec::new();
    let mut found = false;
    if let Some(arap) = arap {
        for e in arap.entries() {
            match (e.match_name.as_str(), e.node) {
                (MESH_GROUP, PropNode::Group(g)) if !found => {
                    found = true;
                    for m in g.entries() {
                        match (m.match_name.as_str(), m.node) {
                            (MESH_ATOM, PropNode::Group(mesh)) => {
                                let n = meshes.len().saturating_add(1);
                                meshes.push(mesh_summary(&mesh, n, &mut unknown));
                            }
                            _ => push_unique(&mut unknown, m.match_name.clone()),
                        }
                    }
                }
                // Auto-traced Shapes are made from the layer again (EffectCraft traces the alpha).
                (OUTLINES, _) => {}
                _ => push_unique(&mut unknown, e.match_name.clone()),
            }
        }
    }
    let extra = if unknown.is_empty() { String::new() } else { format!("; not recognised: {}", capped(&unknown, ", ")) };
    if !found {
        col.lose(
            LossCategory::Effects,
            format!("{here}: imported without meshes or pins: its mesh group ({ARAP} / {MESH_GROUP}) was not found or is not a group{extra}"),
        );
    } else if !meshes.is_empty() {
        col.lose(
            LossCategory::Effects,
            format!(
                "{here}: imported without its meshes and pins (After Effects' pin rest places and positions are not decoded yet): {}{extra}",
                capped(&meshes, "; ")
            ),
        );
    } else if !unknown.is_empty() {
        col.lose(LossCategory::Effects, format!("{here}: no mesh{extra}"));
    }
}

/// `Mesh n (Triangles t, Expansion e): Deform 3 pins (1 keyed) [atom names], Overlap no pins, ...`
fn mesh_summary(mesh: &AepGroup<'_, '_>, n: usize, unknown: &mut Vec<String>) -> String {
    let name = mesh.user_name().unwrap_or_else(|| format!("Mesh {n}"));
    let mut settings: Vec<String> = Vec::new();
    let mut containers: Vec<String> = Vec::new();
    for e in mesh.entries() {
        let label = match e.match_name.as_str() {
            TRIANGLES => Some("Triangles"),
            EXPANSION => Some("Expansion"),
            _ => None,
        };
        match (label, e.node) {
            (Some(label), PropNode::Property(p)) => settings.push(format!("{label} {}", describe(&p))),
            // The triangulation After Effects keeps; EffectCraft builds its own.
            _ if e.match_name == MESH_DATA => {}
            _ => match (PIN_GROUPS.iter().find(|(m, _)| *m == e.match_name), e.node) {
                (Some((_, shown)), PropNode::Group(g)) => containers.push(format!("{shown} {}", pins_summary(&g, unknown))),
                _ => push_unique(unknown, e.match_name),
            },
        }
    }
    let settings = if settings.is_empty() { String::new() } else { format!(" ({})", settings.join(", ")) };
    let containers = if containers.is_empty() { "no pin groups".to_string() } else { containers.join(", ") };
    format!("{name}{settings}: {containers}")
}

/// `3 pins (1 keyed, 1 with an expression) [ADBE FreePin3 PosPin Atom]`, or `no pins`. An entry
/// of the container that is not a group (a pin is one) goes to `unknown`.
fn pins_summary(container: &AepGroup<'_, '_>, unknown: &mut Vec<String>) -> String {
    let mut count = 0usize;
    let mut keyed = 0usize;
    let mut expressions = 0usize;
    let mut atoms: Vec<String> = Vec::new();
    for e in container.entries() {
        let PropNode::Group(pin) = e.node else {
            push_unique(unknown, e.match_name);
            continue;
        };
        count = count.saturating_add(1);
        push_unique(&mut atoms, e.match_name);
        let props: Vec<AepProperty<'_, '_>> =
            pin.entries().into_iter().filter_map(|p| if let PropNode::Property(p) = p.node { Some(p) } else { None }).collect();
        if props.iter().any(AepProperty::is_animated) {
            keyed = keyed.saturating_add(1);
        }
        if props.iter().any(|p| p.expression().is_some()) {
            expressions = expressions.saturating_add(1);
        }
    }
    if count == 0 {
        return "no pins".to_string();
    }
    let mut what = vec![format!("{keyed} keyed")];
    if expressions > 0 {
        what.push(format!("{expressions} with an expression"));
    }
    format!("{count} pin{} ({}) [{}]", if count == 1 { "" } else { "s" }, what.join(", "), capped(&atoms, ", "))
}

/// A stored property's value for the report: `keyed`, the static value, or `unreadable`.
fn describe(p: &AepProperty<'_, '_>) -> String {
    if p.is_animated() {
        return "keyed".to_string();
    }
    match p.static_value() {
        Some(v) if v.len() == 1 => v.first().map(|x| format!("{x}")).unwrap_or_default(),
        Some(v) => format!("[{}]", v.iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(", ")),
        None => "unreadable".to_string(),
    }
}

/// Adds `name` to `names` unless it is there already.
fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The first [`LIST_CAP`] of `items` joined by `sep`, and how many more there are.
fn capped(items: &[String], sep: &str) -> String {
    let shown: Vec<&str> = items.iter().take(LIST_CAP).map(String::as_str).collect();
    let mut out = shown.join(sep);
    let more = items.len().saturating_sub(LIST_CAP);
    if more > 0 {
        out.push_str(&format!("{sep}and {more} more"));
    }
    out
}
