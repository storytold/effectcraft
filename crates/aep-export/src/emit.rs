//! Builds the chunk tree from the plan by cloning prototypes of the template and editing only the
//! fields whose layout is verified (`docs/aep-format.md`, section 3, and `docs/aep-export.md`).

use std::collections::HashMap;

use effectcraft_aep::{AepFile, Chunk};
use effectcraft_project::{ItemId, LayerId};

use crate::plan::{CompPlan, Entry, ItemPlan, KeyPlan, LayerPlan, LayerSrc, ProjectPlan, PropKind, PropPlan, SolidPlan, natural_key};
use crate::template::Template;
use crate::tree::{R, child_data, data_mut, kids_mut, list_mut, put, put_bits, put_cstr, put_f32, put_f64, put_i32, put_u8, put_u16, put_u32, set_utf8, tpl};

/// The first id given to anything the export creates. The template's `Solids` folder keeps the id
/// 0x19 that the project header (`sfnm`/`sfid`) names; everything else is allocated from here.
const FIRST_ID: u32 = 0x1c;
const SOLIDS_FOLDER_ID: u32 = 0x19;
/// A composition item uses its own id and one id per built-in view layer (3D views, custom views,
/// the markers layer).
const VIEW_LAYERS: u32 = 11;
/// The name field of a solid (`opti` 26 to 281) holds 255 bytes and a NUL (a 140 character name
/// saved by After Effects fills it as far as it goes).
pub(crate) const MAX_SOLID_NAME: usize = 255;

struct Ids {
    next: u32,
}

impl Ids {
    fn alloc(&mut self) -> R<u32> {
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or_else(|| tpl("more than 2^32 ids"))?;
        Ok(id)
    }
}

struct Cx<'t> {
    t: &'t Template,
    ids: Ids,
    /// Item ids of the project's solids (for the layer source ids).
    solid_ids: HashMap<ItemId, u32>,
    /// Hidden solid items of null layers, written into the `Solids` folder.
    extra_solids: Vec<(String, Chunk<'static>)>,
    guid: u64,
    /// A root folder named `Solids` in the project is After Effects' own `Solids` folder (id 0x19).
    special: Option<ItemId>,
    special_slot: Option<(Chunk<'static>, Vec<Sorted>)>,
}

/// A sortable group of chunks: name, tie-break key, chunks.
type Sorted = (String, u64, Vec<Chunk<'static>>);

pub(crate) fn emit(plan: &ProjectPlan, extra: &[u8]) -> R<Vec<u8>> {
    let t = Template::load()?;
    let special = plan.root.iter().find(|e| e.name == "Solids" && matches!(e.plan, ItemPlan::Folder(_))).map(|e| e.ec);
    let mut cx = Cx {
        t: &t,
        ids: Ids { next: FIRST_ID },
        solid_ids: HashMap::new(),
        extra_solids: Vec::new(),
        guid: 0x9e37_79b9_7f4a_7c15,
        special,
        special_slot: None,
    };
    let mut ids_by_entry: HashMap<ItemId, u32> = HashMap::new();
    assign_ids(&plan.root, &mut cx, &mut ids_by_entry)?;

    let mut root: Vec<Sorted> = Vec::new();
    build_level(&plan.root, &mut cx, &ids_by_entry, &mut |name, key, chunks| root.push((name, key, chunks)))?;
    // The Solids folder: root-level solids and the hidden solids of null layers.
    let mut solids_children: Vec<Sorted> = Vec::new();
    for (name, key, chunks) in root.iter().filter(|(_, k, _)| *k & ROOT_SOLID != 0) {
        solids_children.push((name.clone(), *key, chunks.clone()));
    }
    root.retain(|(_, k, c)| *k & ROOT_SOLID == 0 && !c.is_empty());
    for (i, (name, chunk)) in cx.extra_solids.iter().enumerate() {
        let mut v = vec![chunk.clone()];
        v.extend(cx.t.fset.iter().cloned());
        solids_children.push((name.clone(), i as u64, v));
    }
    let mut folder = match cx.special_slot.take() {
        Some((item, inner)) => {
            solids_children.extend(inner);
            item
        }
        None => {
            let mut f = cx.t.folder_item.clone();
            set_item_header(&mut f, SOLIDS_FOLDER_ID, 2, "Solids")?;
            f
        }
    };
    sort_named(&mut solids_children);
    kids_mut(list_mut(&mut folder, b"Sfdr")?)?.extend(solids_children.into_iter().flat_map(|(_, _, c)| c));
    root.push(("Solids".to_string(), 0, vec![folder]));
    sort_named(&mut root);

    let mut file: AepFile<'static> = AepFile { chunks: t.file.chunks.clone(), trailer: std::borrow::Cow::Owned(extra.to_vec()) };
    {
        let fold = file.chunks.iter_mut().find(|c| c.is_list(b"Fold")).ok_or_else(|| tpl("template without Fold"))?;
        let kids = kids_mut(fold)?;
        let fdta = kids.iter().find(|c| c.is(b"fdta")).cloned().ok_or_else(|| tpl("template without fdta"))?;
        kids.clear();
        kids.push(fdta);
        kids.extend(root.into_iter().flat_map(|(_, _, c)| c));
    }
    {
        let head = file.chunks.iter_mut().find(|c| c.is(b"head")).ok_or_else(|| tpl("template without head"))?;
        put_u32(data_mut(head)?, 12, cx.ids.next)?;
    }
    effectcraft_aep::write(&file).map_err(crate::AepExportError::Container)
}

/// Marks a root entry that goes into the `Solids` folder.
const ROOT_SOLID: u64 = 1 << 63;

fn sort_named(v: &mut [Sorted]) {
    v.sort_by(|a, b| natural_cmp(&a.0, &b.0).then(a.1.cmp(&b.1)));
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (ka, kb) = (natural_key(a), natural_key(b));
    for (x, y) in ka.iter().zip(kb.iter()) {
        let c = match (x.0, y.0) {
            (true, true) => x.2.cmp(&y.2).then_with(|| x.1.cmp(&y.1)),
            (false, false) => x.1.cmp(&y.1),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
        };
        if c != std::cmp::Ordering::Equal {
            return c;
        }
    }
    ka.len().cmp(&kb.len())
}

fn assign_ids(entries: &[Entry], cx: &mut Cx<'_>, map: &mut HashMap<ItemId, u32>) -> R<()> {
    // Iterative to bound the stack by the folder depth the plan already limits.
    let mut stack: Vec<&Entry> = entries.iter().rev().collect();
    while let Some(e) = stack.pop() {
        let id = if Some(e.ec) == cx.special { SOLIDS_FOLDER_ID } else { cx.ids.alloc()? };
        match &e.plan {
            ItemPlan::Comp(_) => {
                for _ in 0..VIEW_LAYERS {
                    cx.ids.alloc()?;
                }
            }
            ItemPlan::Solid(_) => {
                cx.solid_ids.insert(e.ec, id);
            }
            ItemPlan::Folder(kids) => stack.extend(kids.iter().rev()),
        }
        map.insert(e.ec, id);
    }
    Ok(())
}

type Out<'a> = &'a mut dyn FnMut(String, u64, Vec<Chunk<'static>>);

/// Builds the chunks of one folder level (items and their trailing records) and hands each entry
/// to `sink`. Root-level solids are flagged [`ROOT_SOLID`] in their sort key.
fn build_level(entries: &[Entry], cx: &mut Cx<'_>, ids: &HashMap<ItemId, u32>, sink: Out<'_>) -> R<()> {
    build_level_depth(entries, cx, ids, sink, true, 0)
}

fn build_level_depth(entries: &[Entry], cx: &mut Cx<'_>, ids: &HashMap<ItemId, u32>, sink: Out<'_>, root: bool, depth: usize) -> R<()> {
    if depth > 80 {
        return Err(tpl("folders nested too deeply"));
    }
    for e in entries {
        let id = *ids.get(&e.ec).ok_or_else(|| tpl("item without an id"))?;
        match &e.plan {
            ItemPlan::Folder(kids) => {
                let mut item = cx.t.folder_item.clone();
                set_item_header(&mut item, id, e.label, &e.name)?;
                let mut inner: Vec<Sorted> = Vec::new();
                build_level_depth(kids, cx, ids, &mut |n, k, c| inner.push((n, k, c)), false, depth + 1)?;
                if root && Some(e.ec) == cx.special {
                    // Completed once the hidden solids of null layers are known.
                    cx.special_slot = Some((item, inner));
                    sink(e.name.clone(), e.ec.0, Vec::new());
                    continue;
                }
                sort_named(&mut inner);
                kids_mut(list_mut(&mut item, b"Sfdr")?)?.extend(inner.into_iter().flat_map(|(_, _, c)| c));
                sink(e.name.clone(), e.ec.0, vec![item]);
            }
            ItemPlan::Comp(cp) => {
                let item = build_comp(cx, id, e, cp)?;
                let mut v = vec![item, cx.t.fee.clone()];
                v.extend(cx.t.fset.iter().cloned());
                sink(e.name.clone(), e.ec.0, v);
            }
            ItemPlan::Solid(sp) => {
                let item = build_solid(cx, id, e.label, &e.name, sp)?;
                let mut v = vec![item];
                v.extend(cx.t.fset.iter().cloned());
                sink(e.name.clone(), e.ec.0 | if root { ROOT_SOLID } else { 0 }, v);
            }
        }
    }
    Ok(())
}

/// iide (little endian), idta id at 16 and label at 58, and the name.
fn set_item_header(item: &mut Chunk<'static>, id: u32, label: u8, name: &str) -> R<()> {
    put(child_data(item, b"iide")?, 0, &id.to_le_bytes())?;
    {
        let idta = child_data(item, b"idta")?;
        put_u32(idta, 16, id)?;
        put_u8(idta, 58, label)?;
    }
    set_utf8(item, name)
}

fn unique_guid(cx: &mut Cx<'_>) -> [u8; 16] {
    // A splitmix64 stream: distinct, deterministic, and no machine data.
    let mut out = [0u8; 16];
    for half in 0..2 {
        cx.guid = cx.guid.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = cx.guid;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        if let Some(dst) = out.get_mut(half * 8..half * 8 + 8) {
            dst.copy_from_slice(&z.to_be_bytes());
        }
    }
    out
}

fn solid_item(cx: &mut Cx<'_>, proto: &Chunk<'static>, id: u32, label: u8, name: &str, color: [f32; 3], w: u32, h: u32) -> R<Chunk<'static>> {
    let mut item = proto.clone();
    put(child_data(&mut item, b"iide")?, 0, &id.to_le_bytes())?;
    {
        let idta = child_data(&mut item, b"idta")?;
        put_u32(idta, 16, id)?;
        put_u8(idta, 58, label)?;
    }
    let guid = unique_guid(cx);
    let pin = list_mut(&mut item, b"Pin ")?;
    {
        let sspc = child_data(pin, b"sspc")?;
        put_u32(sspc, 30, w)?;
        put_u32(sspc, 34, h)?;
    }
    {
        let opti = child_data(pin, b"opti")?;
        let [r, g, b] = color;
        put_f32(opti, 10, 1.0)?;
        put_f32(opti, 14, r)?;
        put_f32(opti, 18, g)?;
        put_f32(opti, 22, b)?;
        put_cstr(opti, 26, MAX_SOLID_NAME + 1, name)?;
    }
    put(child_data(pin, b"pgui")?, 0, &guid)?;
    Ok(item)
}

fn build_solid(cx: &mut Cx<'_>, id: u32, label: u8, name: &str, sp: &SolidPlan) -> R<Chunk<'static>> {
    let proto = cx.t.solid_item.clone();
    solid_item(cx, &proto, id, label, name, sp.color, sp.width, sp.height)
}

/// Rebuilds all `tdb4` time scales (byte 12) below `c` to the composition's.
fn set_time_scales(c: &mut Chunk<'static>, ts: u32, depth: usize) -> R<()> {
    if depth > 64 {
        return Err(tpl("property tree nested too deeply"));
    }
    if c.is_list(b"tdbs") {
        let tdb4 = child_data(c, b"tdb4")?;
        put_u32(tdb4, 12, ts)?;
    }
    if let Ok(kids) = kids_mut(c) {
        for k in kids.iter_mut() {
            if k.list_type().is_some() {
                set_time_scales(k, ts, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn build_comp(cx: &mut Cx<'_>, id: u32, e: &Entry, cp: &CompPlan) -> R<Chunk<'static>> {
    let mut item = cx.t.comp_shell.clone();
    set_item_header(&mut item, id, e.label, &e.name)?;
    {
        let cdta = child_data(&mut item, b"cdta")?;
        put_u32(cdta, 4, cp.tb.frame_duration)?;
        put_u32(cdta, 8, cp.tb.timescale)?;
        put_u32(cdta, 16, 600)?;
        put_u32(cdta, 24, 600)?;
        put_u32(cdta, 28, cp.work_start)?;
        // Start and end of the work area are fractions: the time scale is their denominator once
        // a work area is set, 600 while it spans the whole composition.
        let set = cp.work_end.is_some() || cp.work_start != 0;
        put_u32(cdta, 32, if set { cp.tb.timescale } else { 600 })?;
        put_u32(cdta, 36, cp.work_end.unwrap_or(u32::MAX))?;
        put_u32(cdta, 40, if set { cp.tb.timescale } else { 600 })?;
        put_u32(cdta, 44, cp.duration_units)?;
        put_u32(cdta, 48, cp.tb.timescale)?;
        put(cdta, 52, &cp.background)?;
        put_bits(cdta, 139, 0x08, cp.motion_blur)?;
        put_u16(cdta, 140, cp.width)?;
        put_u16(cdta, 142, cp.height)?;
        put_u32(cdta, 144, cp.pixel_aspect.0)?;
        put_u32(cdta, 148, cp.pixel_aspect.1)?;
        put_u32(cdta, 156, crate::timebase::fixed_rate(cp.rate))?;
        put_u32(cdta, 164, cp.display_start)?;
        put_u32(cdta, 168, if cp.display_start == 0 { 1 } else { cp.tb.timescale })?;
        put_u32(cdta, 172, cp.shutter_angle)?;
        put_i32(cdta, 180, cp.shutter_phase)?;
    }
    // View layers: ids right after the composition's.
    {
        let mut view = id.checked_add(1).ok_or_else(|| tpl("id overflow"))?;
        for k in kids_mut(&mut item)?.iter_mut() {
            if k.is_list(b"DLay") || k.is_list(b"SLay") || k.is_list(b"CLay") || k.is_list(b"SecL") {
                put_u32(child_data(k, b"ldta")?, 0, view)?;
                view = view.checked_add(1).ok_or_else(|| tpl("id overflow"))?;
            }
        }
    }
    // The layers.
    let mut layer_ids: HashMap<LayerId, u32> = HashMap::new();
    for lp in &cp.layers {
        layer_ids.insert(lp.ec_id, cx.ids.alloc()?);
    }
    let mut blocks: Vec<Chunk<'static>> = Vec::new();
    for lp in &cp.layers {
        let lid = *layer_ids.get(&lp.ec_id).ok_or_else(|| tpl("layer without an id"))?;
        let block = build_layer(cx, lid, lp, cp, &layer_ids)?;
        blocks.extend(block);
    }
    let at = cx.t.layer_at.min(kids_mut(&mut item)?.len());
    kids_mut(&mut item)?.splice(at..at, blocks);
    // Every property of the composition counts keyframe times in its time scale.
    set_time_scales(&mut item, cp.tb.timescale, 0)?;
    Ok(item)
}

fn build_layer(cx: &mut Cx<'_>, id: u32, lp: &LayerPlan, cp: &CompPlan, layer_ids: &HashMap<LayerId, u32>) -> R<Vec<Chunk<'static>>> {
    let is_null = lp.src == LayerSrc::Null;
    let mut block = if is_null { cx.t.null_block.clone() } else { cx.t.layer_block.clone() };
    let source_id = match &lp.src {
        LayerSrc::Solid(item) => *cx.solid_ids.get(item).ok_or_else(|| tpl("layer source without an id"))?,
        LayerSrc::Null => {
            // The hidden solid every null object has.
            let item_id = cx.ids.alloc()?;
            let proto = cx.t.null_item.clone();
            let name = format!("Null {}", item_id);
            let item = solid_item(cx, &proto, item_id, 0, &name, [1.0, 1.0, 1.0], 100, 100)?;
            cx.extra_solids.push((name, item));
            item_id
        }
    };
    let layr = block.get_mut(0).ok_or_else(|| tpl("empty layer block"))?;
    {
        let ldta = child_data(layr, b"ldta")?;
        put_u32(ldta, 0, id)?;
        put_u8(ldta, 5, lp.quality)?;
        put_u32(ldta, 8, 1)?;
        put_u32(ldta, 108, 1)?;
        let den = cp.tb.timescale;
        put_i32(ldta, 12, lp.start)?;
        put_u32(ldta, 16, den)?;
        put_i32(ldta, 20, lp.in_rel)?;
        put_u32(ldta, 24, den)?;
        put_i32(ldta, 28, lp.out_rel)?;
        put_u32(ldta, 32, den)?;
        put_bits(ldta, 37, 0x01, !lp.name.is_empty())?;
        put_bits(ldta, 37, 0x02, lp.guide)?;
        put_bits(ldta, 38, 0x02, lp.adjustment)?;
        put_bits(ldta, 38, 0x04, false)?;
        put_bits(ldta, 38, 0x08, lp.solo)?;
        put_bits(ldta, 38, 0x80, is_null)?;
        put_bits(ldta, 39, 0x01, lp.video)?;
        put_bits(ldta, 39, 0x02, lp.audio)?;
        put_bits(ldta, 39, 0x04, lp.effects)?;
        put_bits(ldta, 39, 0x08, lp.motion_blur)?;
        put_bits(ldta, 39, 0x20, lp.locked)?;
        put_bits(ldta, 39, 0x40, lp.shy)?;
        put_bits(ldta, 39, 0x80, lp.collapse)?;
        put_u32(ldta, 40, source_id)?;
        put_u8(ldta, 61, lp.label)?;
        put_cstr(ldta, 64, 32, &lp.name)?;
        put_u8(ldta, 99, lp.blend)?;
        put_u32(ldta, 128, 0)?;
        let parent = lp.parent.and_then(|p| layer_ids.get(&p).copied()).unwrap_or(0);
        put_u32(ldta, 132, parent)?;
    }
    set_utf8(layr, &lp.name)?;
    put_properties(cx, layr, lp, cp)?;
    Ok(block)
}

const GROUP_ORDER: [&str; 12] = [
    "ADBE Anchor Point",
    "ADBE Position",
    "ADBE Position_0",
    "ADBE Position_1",
    "ADBE Scale",
    "ADBE Orientation",
    "ADBE Rotate X",
    "ADBE Rotate Y",
    "ADBE Rotate Z",
    "ADBE Opacity",
    "ADBE Envir Appear in Reflect",
    "ADBE Group End",
];

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect())
}

/// Inserts the properties of `lp` into the layer's Transform group, in the order the file uses.
fn put_properties(cx: &Cx<'_>, layr: &mut Chunk<'static>, lp: &LayerPlan, cp: &CompPlan) -> R<()> {
    if lp.props.is_empty() {
        return Ok(());
    }
    let root = list_mut(layr, b"tdgp")?;
    let kids = kids_mut(root)?;
    let at = kids
        .iter()
        .position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some("ADBE Transform Group"))
        .ok_or_else(|| tpl("layer without a Transform group"))?;
    let group = kids.get_mut(at + 1).ok_or_else(|| tpl("Transform group without a list"))?;
    let gk = kids_mut(group)?;
    // Split the group into its header (up to the first match name) and (tdmn, node) pairs.
    let first = gk.iter().position(|c| c.is(b"tdmn")).ok_or_else(|| tpl("empty Transform group"))?;
    let header: Vec<Chunk<'static>> = gk.drain(..first).collect();
    let mut pairs: Vec<(String, Vec<Chunk<'static>>)> = Vec::new();
    let mut it = std::mem::take(gk).into_iter();
    while let Some(m) = it.next() {
        let name = tdmn_name(&m).unwrap_or_default();
        if name == "ADBE Group End" {
            pairs.push((name, vec![m]));
            continue;
        }
        let node = it.next().ok_or_else(|| tpl("match name without a node"))?;
        pairs.push((name, vec![m, node]));
    }
    for p in &lp.props {
        let name = p.kind.match_name();
        let built = build_prop(cx, p, lp, cp)?;
        let tdmn = pairs.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.first().cloned()).unwrap_or_else(|| {
            cx.t.stat
                .iter()
                .chain(cx.t.keyd.iter())
                .find(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(name))
                .cloned()
                .unwrap_or_else(|| Chunk::data(*b"tdmn", Vec::new()))
        });
        let entry = vec![tdmn, built];
        if let Some(slot) = pairs.iter_mut().find(|(n, _)| n == name) {
            slot.1 = entry;
        } else {
            pairs.push((name.to_string(), entry));
        }
    }
    let rank = |n: &str| GROUP_ORDER.iter().position(|g| *g == n).unwrap_or(GROUP_ORDER.len());
    pairs.sort_by_key(|(n, _)| rank(n));
    gk.extend(header);
    gk.extend(pairs.into_iter().flat_map(|(_, v)| v));
    Ok(())
}

/// Round the count up to a multiple of four (the allocation of a key list).
fn round4(n: u32) -> u32 {
    n.div_ceil(4).saturating_mul(4)
}

fn build_prop(cx: &Cx<'_>, p: &PropPlan, lp: &LayerPlan, cp: &CompPlan) -> R<Chunk<'static>> {
    let idx = PropKind::ALL.iter().position(|k| *k == p.kind).ok_or_else(|| tpl("unknown property kind"))?;
    let pool = if p.keys.is_empty() { &cx.t.stat } else { &cx.t.keyd };
    let tdbs = pool.get(idx * 2 + 1).ok_or_else(|| tpl("prototype missing"))?;
    let mut tdbs = tdbs.clone();
    {
        let tdb4 = child_data(&mut tdbs, b"tdb4")?;
        put_u32(tdb4, 12, cp.tb.timescale)?;
        if p.kind == PropKind::Anchor {
            // The width / height of the layer's source (kept next to the anchor's other
            // normalisation constants).
            put_f64(tdb4, 24, f64::from(lp.source_size.0) / f64::from(lp.source_size.1.max(1)))?;
        }
        if let Some((_, enabled)) = &p.expr {
            put_u8(tdb4, 76, 1)?;
            put_u8(tdb4, 77, 1)?;
            put_u8(tdb4, 120, 1)?;
            put_u8(tdb4, 119, u8::from(!*enabled))?;
        }
    }
    if p.keys.is_empty() {
        let cdat = child_data(&mut tdbs, b"cdat")?;
        cdat.iter_mut().for_each(|b| *b = 0);
        for (i, v) in p.value.iter().enumerate() {
            put_f64(cdat, i * 8, *v)?;
        }
    } else {
        let list = list_mut(&mut tdbs, b"list")?;
        let n = u32::try_from(p.keys.len()).map_err(|_| tpl("too many keys"))?;
        let size = if p.kind.spatial() { 56 + 24 * p.kind.dims() } else { 8 + 40 * p.kind.dims() };
        {
            let lhd3 = child_data(list, b"lhd3")?;
            put_u32(lhd3, 8, n)?;
            put_u32(lhd3, 12, n.div_ceil(4))?;
            put_u32(lhd3, 16, u32::try_from(size).map_err(|_| tpl("record size"))?)?;
            put_u32(lhd3, 28, round4(n))?;
        }
        let mut ldat = Vec::with_capacity(size * p.keys.len());
        for k in &p.keys {
            ldat.extend(key_record(p.kind, k, size)?);
        }
        let ld = kids_mut(list)?;
        match ld.iter_mut().find(|c| c.is(b"ldat")) {
            Some(c) => *c = Chunk::data(*b"ldat", ldat),
            None => ld.push(Chunk::data(*b"ldat", ldat)),
        }
    }
    if let Some((text, _)) = &p.expr {
        let kids = kids_mut(&mut tdbs)?;
        let at = kids.iter().position(|c| c.is(b"tdum")).unwrap_or(kids.len());
        kids.insert(at, Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
    }
    Ok(tdbs)
}

/// One keyframe record (`docs/aep-format.md`, 3.5).
fn key_record(kind: PropKind, k: &KeyPlan, size: usize) -> R<Vec<u8>> {
    let mut r = vec![0u8; size];
    let n = kind.dims();
    put_i32(&mut r, 0, k.time)?;
    put_u8(&mut r, 4, k.in_interp)?;
    put_u8(&mut r, 5, k.out_interp)?;
    let base: u16 = if kind.spatial() { 0x0007 } else { 0 };
    let flags = base | if k.roving { 0x0020 } else { 0 } | (u16::from(k.label) << 6);
    put_u16(&mut r, 6, flags)?;
    if kind.spatial() {
        put_u32(&mut r, 8, u32::from(k.spatial_auto))?;
        let (i, o) = (k.in_ease.first().copied().unwrap_or_default(), k.out_ease.first().copied().unwrap_or_default());
        put_f64(&mut r, 24, i.speed)?;
        put_f64(&mut r, 32, i.influence)?;
        put_f64(&mut r, 40, o.speed)?;
        put_f64(&mut r, 48, o.influence)?;
        for j in 0..n {
            put_f64(&mut r, 56 + 8 * j, k.value.get(j).copied().unwrap_or(0.0))?;
            put_f64(&mut r, 56 + 8 * n + 8 * j, k.spatial_in.get(j).copied().unwrap_or(0.0))?;
            put_f64(&mut r, 56 + 16 * n + 8 * j, k.spatial_out.get(j).copied().unwrap_or(0.0))?;
        }
    } else {
        for j in 0..n {
            put_f64(&mut r, 8 + 8 * j, k.value.get(j).copied().unwrap_or(0.0))?;
        }
        let base = 8 + 8 * n;
        for j in 0..n {
            let i = k.in_ease.get(j).or(k.in_ease.last()).copied().unwrap_or_default();
            let o = k.out_ease.get(j).or(k.out_ease.last()).copied().unwrap_or_default();
            put_f64(&mut r, base + 8 * j, i.speed)?;
            put_f64(&mut r, base + 8 * n + 8 * j, i.influence)?;
            put_f64(&mut r, base + 16 * n + 8 * j, o.speed)?;
            put_f64(&mut r, base + 24 * n + 8 * j, o.influence)?;
        }
    }
    Ok(r)
}
