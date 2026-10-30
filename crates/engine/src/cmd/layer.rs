//! Layer menu.

use effectcraft_color::{BlendMode, Label};
use effectcraft_keyframe::{Justify, TextDoc, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    Comp, FootageKind, FrameBlend, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, MaskMode, MatteKind, Node, Project, PropGroup, Quality, Solid,
    TrackMatte, limits,
};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, has_layers, layer_mut, layer_p, layers_p, merge_p, resolve_layer, str_p, unlocked};
use crate::{EngineError, Result, Session, cmd};

mod contents;
mod create;
mod parent;
mod replace;
mod select;
mod switches;
mod timing;
mod transform;

use contents::{add_mask, add_shape_item, mask_props, set_text};
use create::{add_item, new_adjustment, new_null, new_shape, new_solid, new_text};
pub(crate) use create::{color_p, index_p, insert_layer, insert_layer_at, place, position_p};
pub(crate) use parent::parent_fixes;
use parent::set_parent;
use replace::{has_replace_source, replace_source};
use select::{select, select_next, select_prev, select_step};
use switches::{blend, rename, set_comment, set_switch, track_matte};
use timing::{arrange, move_frames, precompose, slip, timing};
use transform::{has_layer_settings, layer_settings, transform_op};
pub(crate) use transform::{offset_position, transform_default, update_position};

/// `p` with `key` set.
fn with(p: &Value, key: &str, v: Value) -> Value {
    let mut p = if p.is_object() { p.clone() } else { json!({}) };
    p[key] = v;
    p
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.newText",
            "Text",
            ["Layer", "New"],
            Some("Cmd+Alt+Shift+T"),
            "{text?, name? (layer name; default the text), position? [x,y], box? [x,y,w,h] (paragraph text, comp space), vertical?, edit? (start editing), font?, style?, size?, fill?, stroke?, applyFill?, applyStroke?, strokeWidth?, tracking?, leading?, baselineShift?, hScale?, vScale?, tsume?, fauxBold?, fauxItalic?, allCaps?, smallCaps?, baseline?, superscript?, subscript?, kerning?, ligatures?, justify?, indentLeft?, indentRight?, indentFirst?, spaceBefore?, spaceAfter?, direction?, composer?, hangingPunctuation?, strokeOverFill?, lineJoin?} (attributes as layer.setText)",
            has_comp,
            new_text
        ),
        cmd!("layer.newSolid", "Solid...", ["Layer", "New"], Some("Cmd+Y"), "{name?, color? #hex|[r,g,b], width?, height?, pixelAspect?}", has_comp, new_solid),
        cmd!("layer.newNull", "Null Object", ["Layer", "New"], Some("Cmd+Alt+Shift+Y"), "{name?}", has_comp, new_null),
        cmd!(
            "layer.newShape",
            "Shape Layer",
            ["Layer", "New"],
            None,
            "{kind?: rect|rounded|ellipse|star|polygon|none, name?, size?, fill?, fillType?, fillBlend?, fillOpacity?, stroke?, strokeType?, strokeBlend?, strokeOpacity?, strokeWidth?, position?}",
            has_comp,
            new_shape
        ),
        cmd!("layer.newAdjustment", "Adjustment Layer", ["Layer", "New"], Some("Cmd+Alt+Y"), "{name?}", has_comp, new_adjustment),
        cmd!(
            "layer.settings",
            "Layer Settings...",
            ["Layer"],
            Some("Cmd+Shift+Y"),
            "{layer?, name?, color?, width?, height?, pixelAspect?, affectAll?: bool (default true; false gives this layer its own copy of a solid other layers use)} → {layer, solid?}",
            has_layer_settings,
            layer_settings
        ),
        cmd!(
            "layer.addItem",
            "Add Footage to Comp",
            ["File"],
            Some("Cmd+/"),
            "{item: id|name, time? (s, the In point), index? (1-based stack position; default above the selected layer), position? ([x, y] comp px; default the centre), duration? (s, for a still)}",
            has_comp,
            add_item
        ),
        cmd!(
            "layer.replaceSource",
            "Replace Layer Source",
            [],
            Some("Cmd+Alt+/"),
            "{comp?, layers?, layer?, item?: id|name (default the Project panel selection), fixExpressions?: bool (accepted for After Effects compatibility)} → {layers, item}",
            has_replace_source,
            replace_source
        ),
        cmd!("layer.select", "Select Layers", [], None, "{layers: [id|name|#n], add?, toggle?}", has_comp, select),
        cmd!("layer.selectNext", "Select Next Layer", [], Some("Cmd+ArrowDown"), "{add?}", has_comp, select_next),
        cmd!("layer.selectPrevious", "Select Previous Layer", [], Some("Cmd+ArrowUp"), "{add?}", has_comp, select_prev),
        cmd!("layer.extendSelectionNext", "Extend Selection to Next Layer", [], Some("Cmd+Shift+ArrowDown"), "{}", has_comp, |s, _| select_step(
            s,
            &json!({"add": true}),
            1
        )),
        cmd!("layer.extendSelectionPrevious", "Extend Selection to Previous Layer", [], Some("Cmd+Shift+ArrowUp"), "{}", has_comp, |s, _| select_step(
            s,
            &json!({"add": true}),
            -1
        )),
        cmd!(
            "layer.setSwitch",
            "Layer Switch",
            [],
            None,
            "{layers?, switch: video|audio|solo|lock|shy|collapse|quality|fx|frameBlend|motionBlur|adjustment|threeD|guide|preserveTransparency, value?, merge?, others? (the named layers alone: the other layers get the opposite state, or come back)}",
            has_layers,
            set_switch
        ),
        cmd!("layer.rename", "Rename", [], Some("Enter"), "{layer?, name}", has_layers, rename),
        cmd!("layer.setComment", "Layer Comment", [], None, "{layers?, comment}", has_layers, set_comment),
        cmd!("layer.setBlendMode", "Blending Mode", [], None, "{layers?, mode?: Normal|Multiply|Screen|…, step?: ±1}", has_layers, blend),
        cmd!(
            "layer.setTrackMatte",
            "Track Matte",
            [],
            None,
            "{layers? | layer?, matte: layer|null, kind?: alpha|alphaInverted|luma|lumaInverted|preserve}",
            has_layers,
            track_matte
        ),
        cmd!("layer.setParent", "Parent", [], None, "{layers?, parent: layer|null, compensate?: bool}", has_layers, set_parent),
        cmd!(
            "layer.timing",
            "Layer Timing",
            [],
            None,
            "{layers?, op?: moveInToTime|moveOutToTime|moveInToStart|moveOutToEnd|trimInToTime|trimOutToTime, delta?, start?, in?, out?, inDelta?, outDelta? (seconds, per layer), merge?}",
            has_layers,
            timing
        ),
        cmd!("layer.moveInToStart", "Move Layer In Point to Beginning of Comp", [], Some("Alt+Home"), "{layers?}", has_layers, |s, p| timing(
            s,
            &with(p, "op", json!("moveInToStart"))
        )),
        cmd!("layer.moveOutToEnd", "Move Layer Out Point to End of Comp", [], Some("Alt+End"), "{layers?}", has_layers, |s, p| timing(
            s,
            &with(p, "op", json!("moveOutToEnd"))
        )),
        cmd!("layer.moveEarlier", "Move Layers 1 Frame Earlier", [], Some("Alt+PageUp"), "{layers?}", has_layers, |s, p| move_frames(s, p, -1)),
        cmd!("layer.moveLater", "Move Layers 1 Frame Later", [], Some("Alt+PageDown"), "{layers?}", has_layers, |s, p| move_frames(s, p, 1)),
        cmd!("layer.moveEarlier10", "Move Layers 10 Frames Earlier", [], Some("Alt+Shift+PageUp"), "{layers?}", has_layers, |s, p| move_frames(s, p, -10)),
        cmd!("layer.moveLater10", "Move Layers 10 Frames Later", [], Some("Alt+Shift+PageDown"), "{layers?}", has_layers, |s, p| move_frames(s, p, 10)),
        cmd!("layer.slip", "Slip Edit", [], None, "{layers?, delta? (seconds) | frames?, merge?}", has_layers, slip),
        cmd!("layer.slipBack", "Slip Edit 1 Frame Earlier", [], None, "{layers?}", has_layers, |s, p| {
            let mut p = p.clone();
            p["frames"] = json!(-1);
            slip(s, &p)
        }),
        cmd!("layer.slipForward", "Slip Edit 1 Frame Later", [], None, "{layers?}", has_layers, |s, p| {
            let mut p = p.clone();
            p["frames"] = json!(1);
            slip(s, &p)
        }),
        cmd!("layer.arrange", "Arrange", ["Layer", "Arrange"], None, "{layers?, to: front|forward|backward|back, index?, above?: layer}", has_layers, arrange),
        cmd!(
            "layer.precompose",
            "Pre-compose...",
            ["Layer"],
            Some("Cmd+Shift+C"),
            "{layers?, name?, mode?: move|leave, adjustDuration?, open?}",
            has_layers,
            precompose
        ),
        cmd!(
            "layer.addMask",
            "New Mask",
            ["Layer", "Mask"],
            Some("Cmd+Shift+N"),
            "{layer?, shape?: rect|ellipse|rounded|polygon|star, rect? [x,y,w,h] (layer space; a polygon or star fits its width), mode?}",
            has_layers,
            add_mask
        ),
        cmd!("layer.setMask", "Mask Mode", [], None, "{layer?, mask: index|uid|name, mode?, inverted?}", has_layers, mask_props),
        cmd!(
            "layer.addShapeItem",
            "Add (Shape)",
            [],
            None,
            "{layer?, kind: group|rect|ellipse|star|polygon|path|fill|stroke|gfill|gstroke|trim|repeater|round|offset|pucker|twist|zigzag|wiggle|wiggler|merge|frontColor|bevelColor|sideColor|backColor, group?: uid|path} → {uid, path} (frontColor…backColor: the group's Front / Bevel / Side / Back item, which colours that surface of every extruded paint in the group in Advanced 3D)",
            has_layers,
            add_shape_item
        ),
        cmd!(
            "layer.setText",
            "Edit Text",
            [],
            None,
            "{layer?, range?: [start, end] (characters; default all), text?, font?, style?, size?, fill?, stroke?, applyFill?, applyStroke?, strokeWidth?, tracking?, leading?: px|\"auto\", baselineShift? px, hScale? %, vScale? %, tsume? %, fauxBold?, fauxItalic?, allCaps?, smallCaps?, baseline?: normal|superscript|subscript, superscript?, subscript?, kerning?: metrics|optical|number, ligatures?, discretionaryLigatures? (OpenType), contextualAlternates?, stylisticAlternates?, stylisticSets?: [1–20], ss01?, ss02?, ss03?, ss04?, ss05?, ss06?, ss07?, ss08?, ss09?, ss10?, ss11?, ss12?, ss13?, ss14?, ss15?, ss16?, ss17?, ss18?, ss19?, ss20?, swash?, titling?, ordinals?, fractions?, allSmallCaps?, figureStyle?: default|lining|oldStyle, figureWidth?: default|proportional|tabular, figures?, variations?: {tag: value} (variable font axes; null resets), justify?: left|center|right|justifyLeft|justifyCenter|justifyRight|justifyAll, indentLeft?, indentRight?, indentFirst?, spaceBefore?, spaceAfter?, direction?: ltr|rtl, composer?: everyLine|singleLine, hangingPunctuation?, strokeOverFill?, lineJoin?: miter|round|bevel, box?: [x,y,w,h]|null (layer space), vertical?}",
            has_layers,
            set_text
        ),
        cmd!(
            "layer.transform",
            "Transform",
            ["Layer", "Transform"],
            None,
            "{layers?, op: reset|center|fit|fitWidth|fitHeight|flipH|flipV}",
            has_layers,
            transform_op
        ),
    ]
}

#[allow(dead_code)]
fn unused(_: &Comp) {}
