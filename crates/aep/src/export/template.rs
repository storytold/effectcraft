//! The generated template the exporter edits: a project that After Effects saved (one
//! composition `C` with one red solid layer `L` in the `Solids` folder), plus the prototypes of
//! the property records of a layer with static Transform values, of a layer with keyed Transform
//! values and of a null layer, all from generated reference projects, and the mask prototypes
//! from our fixtures 091, 093 and 208 (see `docs/aep-export.md`).
//!
//! The bytes are embedded; they are parsed with `effectcraft-aep` on every export (about 90 KB).

use crate::{AepFile, Chunk, parse};

use crate::export::tree::{R, tpl};

#[cfg(test)]
pub(crate) const BASE: &[u8] = include_bytes!("../../template/base.aep");
#[cfg(test)]
const MASKS: &[u8] = include_bytes!("../../template/masks.aep");
#[cfg(test)]
pub(crate) const EFFECTS: &[u8] = include_bytes!("../../template/effects.aep");
const FOOTAGE: &[u8] = include_bytes!("../../template/footage.aep");
pub(crate) const TEXT: &[u8] = include_bytes!("../../template/text.aep");
#[cfg(test)]
pub(crate) const OBJECTS: &[u8] = include_bytes!("../../template/objects.aep");
#[cfg(test)]
pub(crate) const STYLES: &[u8] = include_bytes!("../../template/styles.aep");

/// The chunks cloned by the exporter.
pub(crate) struct Template {
    /// The whole base project; its `Fold` children are replaced by the export.
    pub file: AepFile<'static>,
    /// The base composition item without its layer blocks.
    pub comp_shell: Chunk<'static>,
    /// Index in the shell's children where the layer blocks go.
    pub layer_at: usize,
    /// `Layr`, `Ewst` and the f-records that follow the layer `L`.
    pub layer_block: Vec<Chunk<'static>>,
    /// The footage item of the solid `L` (inside `Solids`).
    pub solid_item: Chunk<'static>,
    /// The `Solids` folder item.
    pub folder_item: Chunk<'static>,
    /// The f-records that follow a footage item inside a folder.
    pub fset: Vec<Chunk<'static>>,
    /// The `FEE ` list that follows a composition item.
    pub fee: Chunk<'static>,
    /// `tdmn`, `tdbs` pairs of the five Transform properties with a static value.
    pub stat: Vec<Chunk<'static>>,
    /// The same with two keys.
    pub keyd: Vec<Chunk<'static>>,
    /// The layer block of a null layer.
    pub null_block: Vec<Chunk<'static>>,
    /// The hidden solid item of a null layer.
    pub null_item: Chunk<'static>,
    /// The mask prototypes (fixtures 091, 093, 208).
    pub masks: crate::export::masks::MaskProtos,
    /// The effect prototypes (cases 356-397).
    pub effects: crate::export::effects::FxProtos,
    /// The footage item prototypes (cases 351, 353, 603-607).
    pub footage: crate::export::footage::FtProtos,
    /// The layer blocks of a point (110) and a box (245) text layer.
    pub text_point: Vec<Chunk<'static>>,
    pub text_box: Vec<Chunk<'static>>,
    /// The text animator prototypes (579, 580-592, 728).
    pub anims: crate::export::textanim::AnimProtos,
    /// The marker group (`tdmn ADBE Marker` + `LIST mrst`) of fixture 249.
    pub marker_group: Vec<Chunk<'static>>,
    /// The layer blocks of a two-node camera (256), a spot (254) and a point light (252).
    pub camera_block: Vec<Chunk<'static>>,
    pub spot_block: Vec<Chunk<'static>>,
    pub point_block: Vec<Chunk<'static>>,
    /// Camera and light option records (342-345, 254).
    pub options: crate::export::camlight::OptionProtos,
    /// Shape layer, item and record prototypes (519, 240, 451-471, 239, 242).
    pub shapes: crate::export::shapes::ShapeProtos,
    /// Layer-style record prototypes (977, 976).
    pub styles: crate::export::styles::StyleProtos,
}

/// The `tdmn`, `tdbs` pairs of the five Transform properties, static or keyed, built in code
/// (aep-format.md 3.4): Anchor Point and Position spatial, Scale, Rotation and Opacity plain;
/// Scale and Opacity carry their limits.
fn transform_records(keyed: bool) -> Vec<Chunk<'static>> {
    use crate::export::plan::PropKind;
    use crate::export::synth::{KeyTemplate, RecSpec, class};
    PropKind::ALL
        .iter()
        .flat_map(|k| {
            let spec = match k {
                PropKind::Anchor | PropKind::Position => {
                    RecSpec { dims: 3, template: KeyTemplate::Spatial(3), class: class::SPATIAL, limits: None, byte5: None }
                }
                PropKind::Scale => RecSpec { dims: 3, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 0.0)), byte5: None },
                PropKind::Rotation => RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: None, byte5: None },
                PropKind::Opacity => RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::LAYER_SCALAR, limits: Some((0.0, 100.0)), byte5: None },
            };
            [crate::export::synth::tdmn(k.match_name()), crate::export::synth::tdbs(&spec, keyed, 25_600)]
        })
        .collect()
}

impl Template {
    pub fn load() -> R<Template> {
        let file = AepFile { chunks: crate::export::base::project_chunks(), trailer: std::borrow::Cow::Borrowed(&[]) };
        let fee = crate::export::base::fee();
        let fset = crate::export::skeleton::f_records();
        let folder_item = crate::export::base::folder_item();
        let solid_item = crate::export::base::solid_item();
        let (comp_shell, first) = crate::export::base::comp_shell();
        let layer_block = crate::export::skeleton::av_layer();
        let stat = transform_records(false);
        let keyd = transform_records(true);
        // A null layer has the AV layer's block; `ldta` marks it (byte 38 bit 0x80).
        let null_block = crate::export::skeleton::av_layer();
        // The hidden solid of a null layer is an ordinary solid item (022).
        let null_item = crate::export::base::solid_item();
        let masks = crate::export::masks::MaskProtos::build();
        let effects = crate::export::effects::FxProtos::build();
        let styles = crate::export::styles::StyleProtos::from_table();
        let ft_file = parse(FOOTAGE).map_err(|_| tpl("the embedded footage prototypes do not parse"))?.into_owned();
        let footage = crate::export::footage::FtProtos::from_file(&ft_file)?;
        let tx_file = parse(TEXT).map_err(|_| tpl("the embedded text prototypes do not parse"))?.into_owned();
        let text_point = tx_file.list(b"txpt").ok_or_else(|| tpl("text prototype missing"))?.children().to_vec();
        let text_box = tx_file.list(b"txbx").ok_or_else(|| tpl("text prototype missing"))?.children().to_vec();
        let anims = crate::export::textanim::AnimProtos {
            group: crate::export::tree_spec::build(crate::export::tree_defs::TEXT_ANIMATORS),
            animator: crate::export::tree_spec::build(crate::export::tree_defs::TEXT_ANIMATOR),
            records: crate::export::skeleton::pairs(crate::export::tree_defs::TEXT_ANIMATOR_RECORDS),
        };
        let marker_group = crate::export::skeleton::marker_group();
        use crate::export::{skeleton, tree_defs};
        let camera_block = skeleton::layer_block(tree_defs::CAMERA_ROOT);
        let spot_block = skeleton::layer_block(tree_defs::SPOT_ROOT);
        let point_block = skeleton::layer_block(tree_defs::POINT_ROOT);
        let records = skeleton::pairs(tree_defs::OPTIONS).into_iter().map(|(name, [_, node])| (name, node)).collect();
        let options = crate::export::camlight::OptionProtos { records };
        // The key list of the keyed Opacity prototype (one-component plain keys; its header is
        // rewritten for every keyed shape property).
        let key_list = keyd
            .iter()
            .rev()
            .find(|c| c.is_list(b"tdbs"))
            .and_then(|c| c.child_list(b"list"))
            .cloned()
            .ok_or_else(|| tpl("keyed prototype without a key list"))?;
        let shapes = crate::export::shapes::ShapeProtos {
            block: skeleton::layer_block(tree_defs::SHAPE_ROOT),
            items: skeleton::pairs(tree_defs::SHAPE_ITEMS),
            records: skeleton::pairs(tree_defs::SHAPE_RECORDS),
            key_list,
        };
        Ok(Template {
            file,
            comp_shell,
            layer_at: first,
            layer_block,
            solid_item,
            folder_item,
            fset,
            fee,
            stat,
            keyd,
            null_block,
            null_item,
            masks,
            effects,
            footage,
            text_point,
            text_box,
            anims,
            marker_group,
            camera_block,
            spot_block,
            point_block,
            options,
            shapes,
            styles,
        })
    }
}

#[cfg(test)]
mod tests {
    /// The mask prototypes built in code have the template's structure (the same chunks in the
    /// same order) and its decoded bytes.
    #[test]
    fn the_built_mask_prototypes_match_the_template() {
        let f = crate::parse(super::MASKS).unwrap().into_owned();
        let old = crate::export::masks::MaskProtos::from_file(&f).unwrap();
        let new = crate::export::masks::MaskProtos::build();
        fn shape(c: &crate::Chunk<'_>) -> Vec<String> {
            let mut out = vec![String::from_utf8_lossy(&c.id).into_owned()];
            if let crate::Body::List { list_type, .. } = &c.body {
                out.push(String::from_utf8_lossy(list_type).into_owned());
            }
            if c.is(b"tdmn") {
                out.push(String::from_utf8_lossy(c.bytes().unwrap()).into_owned());
            }
            for k in c.children() {
                out.extend(shape(k));
            }
            out
        }
        for (a, b) in [(&old.atom, &new.atom), (&old.keyed_path, &new.keyed_path), (&old.keyed_feather, &new.keyed_feather), (&old.mkif, &new.mkif)] {
            assert_eq!(shape(a), shape(b));
        }
        assert_eq!(old.parade_header.len(), new.parade_header.len());
    }
}
