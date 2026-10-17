//! The generated template the exporter edits: a project that After Effects saved (one
//! composition `C` with one red solid layer `L` in the `Solids` folder), plus the prototypes of
//! the property records of a layer with static Transform values, of a layer with keyed Transform
//! values and of a null layer, all from generated reference projects (see `docs/aep-export.md`).
//!
//! The bytes are embedded; they are parsed with `effectcraft-aep` on every export (about 90 KB).

use effectcraft_aep::{AepFile, Chunk, parse};

use crate::tree::{R, tpl};

const BASE: &[u8] = include_bytes!("../template/base.aep");
const PROTOS: &[u8] = include_bytes!("../template/protos.aep");

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
        Ok(Template { file, comp_shell, layer_at: first, layer_block, solid_item, folder_item, fset, fee, stat, keyd, null_block, null_item })
    }
}
