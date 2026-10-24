//! The generated template the exporter edits: a project that After Effects saved (one
//! composition `C` with one red solid layer `L` in the `Solids` folder), plus the prototypes of
//! the property records of a layer with static Transform values, of a layer with keyed Transform
//! values and of a null layer, all from generated reference projects, and the mask prototypes
//! from our fixtures 091, 093 and 208 (see `docs/aep-export.md`).
//!
//! The bytes are embedded; they are parsed with `effectcraft-aep` on every export (about 90 KB).

use effectcraft_aep::{AepFile, Chunk, parse};

use crate::tree::{R, tpl};

const BASE: &[u8] = include_bytes!("../template/base.aep");
const PROTOS: &[u8] = include_bytes!("../template/protos.aep");
const MASKS: &[u8] = include_bytes!("../template/masks.aep");
pub(crate) const EFFECTS: &[u8] = include_bytes!("../template/effects.aep");
const FOOTAGE: &[u8] = include_bytes!("../template/footage.aep");
const TEXT: &[u8] = include_bytes!("../template/text.aep");
const OBJECTS: &[u8] = include_bytes!("../template/objects.aep");
const STYLES: &[u8] = include_bytes!("../template/styles.aep");

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
    pub masks: crate::masks::MaskProtos,
    /// The effect prototypes (cases 356-397).
    pub effects: crate::effects::FxProtos,
    /// The footage item prototypes (cases 351, 353, 603-607).
    pub footage: crate::footage::FtProtos,
    /// The layer blocks of a point (110) and a box (245) text layer.
    pub text_point: Vec<Chunk<'static>>,
    pub text_box: Vec<Chunk<'static>>,
    /// The text animator prototypes (579, 580-592, 728).
    pub anims: crate::textanim::AnimProtos,
    /// The marker group (`tdmn ADBE Marker` + `LIST mrst`) of fixture 249.
    pub marker_group: Vec<Chunk<'static>>,
    /// The layer blocks of a two-node camera (256), a spot (254) and a point light (252).
    pub camera_block: Vec<Chunk<'static>>,
    pub spot_block: Vec<Chunk<'static>>,
    pub point_block: Vec<Chunk<'static>>,
    /// Camera and light option records (342-345, 254).
    pub options: crate::camlight::OptionProtos,
    /// Shape layer, item and record prototypes (519, 240, 451-471, 239, 242).
    pub shapes: crate::shapes::ShapeProtos,
    /// Layer-style record prototypes (977, 976).
    pub styles: crate::styles::StyleProtos,
}

fn is_f_record(c: &Chunk<'_>) -> bool {
    c.id[0] == b'f' && !c.is_list(b"FEE ")
}

impl Template {
    pub fn load() -> R<Template> {
        let file = parse(BASE).map_err(|_| tpl("the embedded template does not parse"))?.into_owned();
        let protos = parse(PROTOS).map_err(|_| tpl("the embedded prototypes do not parse"))?.into_owned();
        let fold = file.list(b"Fold").ok_or_else(|| tpl("template without Fold"))?;
        let fk = fold.children();
        let comp = fk.iter().find(|c| c.is_list(b"Item")).ok_or_else(|| tpl("template without a composition"))?.clone();
        let fee_at = fk.iter().position(|c| c.is_list(b"FEE ")).ok_or_else(|| tpl("template without FEE"))?;
        let fee = fk.get(fee_at).ok_or_else(|| tpl("template without FEE"))?.clone();
        let fset: Vec<Chunk<'static>> = fk.iter().skip(fee_at + 1).take_while(|c| is_f_record(c)).cloned().collect();
        let mut folder_item = fk.iter().rev().find(|c| c.is_list(b"Item")).ok_or_else(|| tpl("template without Solids"))?.clone();
        let solid_item;
        {
            let sfdr = folder_item.child_list(b"Sfdr").ok_or_else(|| tpl("Solids without Sfdr"))?;
            solid_item = sfdr.children().iter().find(|c| c.is_list(b"Item")).ok_or_else(|| tpl("Solids without a solid"))?.clone();
        }
        crate::tree::kids_mut(crate::tree::list_mut(&mut folder_item, b"Sfdr")?)?.clear();

        let kids = comp.children();
        let first = kids.iter().position(|c| c.is_list(b"Layr")).ok_or_else(|| tpl("template without a layer"))?;
        let end = kids.iter().position(|c| c.is_list(b"DLay")).ok_or_else(|| tpl("template without view layers"))?;
        let layer_block: Vec<Chunk<'static>> = kids.get(first..end).ok_or_else(|| tpl("layer block"))?.to_vec();
        let mut comp_shell = comp.clone();
        crate::tree::kids_mut(&mut comp_shell)?.drain(first..end);

        let list_of = |ty: &[u8; 4]| -> R<Vec<Chunk<'static>>> { Ok(protos.list(ty).ok_or_else(|| tpl("prototype list missing"))?.children().to_vec()) };
        let stat = list_of(b"stat")?;
        let keyd = list_of(b"keyd")?;
        let null_block = list_of(b"nulb")?;
        let null_item = list_of(b"nuli")?.into_iter().next().ok_or_else(|| tpl("null item missing"))?;
        let mask_file = parse(MASKS).map_err(|_| tpl("the embedded mask prototypes do not parse"))?.into_owned();
        let masks = crate::masks::MaskProtos::from_file(&mask_file)?;
        let fx_file = parse(EFFECTS).map_err(|_| tpl("the embedded effect prototypes do not parse"))?.into_owned();
        let effects = crate::effects::FxProtos::from_file(&fx_file)?;
        let st_file = parse(STYLES).map_err(|_| tpl("the embedded style prototypes do not parse"))?.into_owned();
        let styles = crate::styles::StyleProtos::from_file(&st_file)?;
        let ft_file = parse(FOOTAGE).map_err(|_| tpl("the embedded footage prototypes do not parse"))?.into_owned();
        let footage = crate::footage::FtProtos::from_file(&ft_file)?;
        let tx_file = parse(TEXT).map_err(|_| tpl("the embedded text prototypes do not parse"))?.into_owned();
        let text_point = tx_file.list(b"txpt").ok_or_else(|| tpl("text prototype missing"))?.children().to_vec();
        let text_box = tx_file.list(b"txbx").ok_or_else(|| tpl("text prototype missing"))?.children().to_vec();
        let tx_list =
            |ty: &[u8; 4]| -> R<Vec<Chunk<'static>>> { Ok(tx_file.list(ty).ok_or_else(|| tpl("text animator prototype missing"))?.children().to_vec()) };
        let anim_records = {
            let mut out = Vec::new();
            let mut it = tx_list(b"tarc")?.into_iter();
            while let (Some(m), Some(node)) = (it.next(), it.next()) {
                let name = m.bytes().map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or_default()).to_string()).unwrap_or_default();
                out.push((name, [m, node]));
            }
            out
        };
        let anims = crate::textanim::AnimProtos { group: tx_list(b"tanm")?, animator: tx_list(b"tani")?, records: anim_records };
        let obj_file = parse(OBJECTS).map_err(|_| tpl("the embedded object prototypes do not parse"))?.into_owned();
        let marker_group = obj_file.list(b"mrkp").ok_or_else(|| tpl("marker prototype missing"))?.children().to_vec();
        let obj = |ty: &[u8; 4]| -> R<Vec<Chunk<'static>>> { Ok(obj_file.list(ty).ok_or_else(|| tpl("object prototype missing"))?.children().to_vec()) };
        let camera_block = obj(b"camb")?;
        let spot_block = obj(b"spot")?;
        let point_block = obj(b"pont")?;
        let mut records = Vec::new();
        let mut it = obj(b"opts")?.into_iter();
        while let (Some(m), Some(node)) = (it.next(), it.next()) {
            let name = m.bytes().map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or_default()).to_string()).unwrap_or_default();
            records.push((name, node));
        }
        let options = crate::camlight::OptionProtos { records };
        let pairs = |ty: &[u8; 4]| -> R<Vec<(String, [Chunk<'static>; 2])>> {
            let mut out = Vec::new();
            let mut it = obj(ty)?.into_iter();
            while let (Some(m), Some(node)) = (it.next(), it.next()) {
                let name = m.bytes().map(|b| String::from_utf8_lossy(b.split(|x| *x == 0).next().unwrap_or_default()).to_string()).unwrap_or_default();
                out.push((name, [m, node]));
            }
            Ok(out)
        };
        // The key list of the keyed Opacity prototype (one-component plain keys; its header is
        // rewritten for every keyed shape property).
        let key_list = keyd
            .iter()
            .rev()
            .find(|c| c.is_list(b"tdbs"))
            .and_then(|c| c.child_list(b"list"))
            .cloned()
            .ok_or_else(|| tpl("keyed prototype without a key list"))?;
        let shapes = crate::shapes::ShapeProtos { block: obj(b"shpb")?, items: pairs(b"shpi")?, records: pairs(b"shpr")?, key_list };
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
