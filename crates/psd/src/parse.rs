use super::limits::{Error, Result};
use super::model::{Adjustment, ChannelInfo, ColorMode, Layer, LayerMask, LinkedFile, MAX_GROUP_DEPTH, Node, Rect, Section, SubPath, TextInfo, VectorMask};
use super::{Psd, Reader, descriptor, engine_data, placed};
use std::sync::Arc;

/// Whether `bytes` start like a Photoshop document.
pub fn is_psd(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && &bytes[..4] == b"8BPS" && matches!(u16::from_be_bytes([bytes[4], bytes[5]]), 1 | 2)
}

impl Psd {
    /// Reads the structure with [`craft_psd::PsdFile::from_source`]: header, colour mode data,
    /// layer records with their additional information and the global blocks. Channel data and
    /// the merged image stay in `data` (as ranges), decoded on demand.
    pub fn parse(data: impl Into<Arc<[u8]>>) -> Result<Psd> {
        let data: Arc<[u8]> = data.into();
        if !is_psd(&data) {
            return Err(Error::NotPsd);
        }
        let (read, splice) = read_file(&data);
        let (head, file, warnings) = match read {
            Ok(file) => {
                // The compression field before the merged image's data.
                let merged = file.image_data.stored.as_ref().map_or(data.len(), |s| (splice.map(s.offset) as usize).saturating_sub(2));
                ((file.header.clone(), file.color_mode_data.clone(), merged), Some(file), vec![])
            }
            // A damaged layer section must not discard the document: keep the header and the
            // merged image, drop the layers and say so. Without a merged image there is nothing to
            // fall back to, so the original error stands.
            Err(e) => match flattened(&data) {
                Some(head) => (head, None, vec![format!("Layers could not be read ({}); only the flattened image is available", error(&e))]),
                None => return Err(error(&e)),
            },
        };
        let (h, palette, merged_offset) = head;
        let mode_v = h.color_mode.as_u16();
        let mode = ColorMode::from_u16(mode_v).ok_or_else(|| Error::Unsupported(format!("colour mode {mode_v}")))?;
        if !matches!(h.depth, 1 | 8 | 16 | 32) {
            return Err(Error::Unsupported(format!("{}-bit", h.depth)));
        }
        if h.width == 0 || h.height == 0 || h.width > 300_000 || h.height > 300_000 {
            return Err(Error::Invalid(format!("size {}x{}", h.width, h.height)));
        }
        let mut psd = Psd {
            psb: h.version.is_psb(),
            width: h.width,
            height: h.height,
            channels: h.channels,
            depth: h.depth,
            mode,
            layers: vec![],
            merged_alpha: false,
            palette,
            linked: vec![],
            warnings,
            data: data.clone(),
            merged_offset,
        };
        let Some(file) = file else {
            psd.merged_alpha = section_merged_alpha(&data);
            return Ok(psd);
        };
        if let Some((block, e)) = file.unreadable_layer_block() {
            // A 16/32-bit file's layers live in a global block; when it is damaged the file reads
            // as flattened. Its layer count still says whether the merged image has alpha.
            psd.merged_alpha = block.data.get(..2).is_some_and(|c| i16::from_be_bytes([c[0], c[1]]) < 0);
            psd.warnings.push(format!("Layers could not be read ({}); only the flattened image is available", error(&e)));
        }
        if let Some(info) = &file.layer_info {
            psd.merged_alpha = info.merged_alpha;
            psd.layers = info.layers.iter().enumerate().map(|(i, rec)| psd.layer(rec, i, splice)).collect::<Result<Vec<_>>>()?;
        }
        for b in &file.global_blocks {
            if let Some(stored) = b.stored.as_ref().filter(|_| linked(&b.key, 0)) {
                let start = splice.map(stored.offset) as usize;
                psd.linked_files(&data, start, start.saturating_add(stored.len as usize).min(data.len()));
            }
        }
        Ok(psd)
    }

    /// One layer record of the model, its additional information read into [`Layer`].
    /// `splice` maps craft-psd's offsets back into `self.data` (see [`read_file`]).
    fn layer(&self, rec: &craft_psd::LayerRecord, index: usize, splice: Splice) -> Result<Layer> {
        let channels = rec
            .channels
            .iter()
            .map(|c| match &c.stored {
                // `ChannelInfo` covers the compression field and the data after it.
                Some(s) => ChannelInfo { id: c.id, offset: (splice.map(s.offset) as usize).saturating_sub(2), len: s.len as usize + 2 },
                None => ChannelInfo { id: c.id, offset: 0, len: 0 },
            })
            .collect();
        let r = rec.rect;
        let mut l = Layer {
            index,
            rect: Rect { top: r.top, left: r.left, bottom: r.bottom, right: r.right },
            blend: rec.blend_mode.key(),
            opacity: rec.opacity,
            fill_opacity: 255,
            clipping: rec.clipping != 0,
            hidden: rec.flags.hidden(),
            protect_transparency: rec.flags.transparency_protected(),
            channels,
            name: mac_roman_string(&rec.name),
            ..Default::default()
        };
        if let craft_psd::MaskData::Mask(m) = &rec.mask {
            let rect = Rect { top: m.rect.top, left: m.rect.left, bottom: m.rect.bottom, right: m.rect.right };
            l.mask = Some(LayerMask { rect, default_color: m.default_color, disabled: m.flags & 2 != 0, inverted: m.flags & 4 != 0 });
        }
        for b in &rec.blocks {
            l.keys.push(b.key);
            // A malformed optional block only loses that block's information.
            // Unsupported modern placement geometry must not silently become an
            // unwarped layer or fall back to a less expressive PlLd in either order.
            match self.layer_block(&mut l, b) {
                Err(e @ Error::Unsupported(_)) if matches!(&b.key, b"SoLd" | b"SoLE") => return Err(e),
                _ => {}
            }
        }
        Ok(l)
    }

    /// The embedded bytes of linked file `uuid`.
    pub fn linked_data(&self, uuid: &str) -> Option<&[u8]> {
        let f = self.linked.iter().find(|f| f.uuid == uuid)?;
        self.data.get(f.data.clone()?)
    }

    /// Linked layer data records (spec: "Linked Layer"): length, kind (`liFD` embedded data,
    /// `liFE` external file, `liFA` alias), version, unique id, original file name, type,
    /// creator, data length, optional descriptors, then the data. A damaged record ends the list.
    /// Ranges in [`LinkedFile::data`] are in the document bytes (`self.data`); craft-psd reports
    /// them relative to the block, so the block's start is added back.
    fn linked_files(&mut self, data: &[u8], start: usize, end: usize) {
        let Some(block) = data.get(start..end) else { return };
        for f in craft_psd::smart::linked_files(block) {
            self.linked.push(LinkedFile { uuid: f.uuid, name: f.name, file_type: f.file_type, data: f.data.map(|r| (start + r.start)..(start + r.end)) });
        }
    }

    fn layer_block(&self, l: &mut Layer, block: &craft_psd::tagged::TaggedBlock) -> Result<()> {
        let key = &block.key;
        let b = block.data.as_slice();
        let mut r = Reader::new(b);
        match key {
            b"luni" => {
                // Shared framing rejects truncated counts/payloads before allocation. Only
                // the first record belongs to this block; UTF-16 repair and naming stay local.
                let units = craft_psd::unicode_names::records(b).next().ok_or(Error::Truncated)?;
                let mut name: String = char::decode_utf16(units.as_chunks::<2>().0.iter().map(|v| u16::from_be_bytes(*v)))
                    .map(|v| v.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect();
                name.truncate(name.trim_end_matches('\0').len());
                if !name.is_empty() {
                    l.name = name;
                }
            }
            b"lyid" | b"iOpa" => {
                use craft_psd::tagged::BlockData;
                // Borrow the original block; short fields keep the previous value,
                // while complete prefixes retain the shared parser's trailing-byte policy.
                match block.parsed().transpose().map_err(|e| error(&e))? {
                    Some(BlockData::LayerId(id)) => l.id = Some(id),
                    Some(BlockData::FillOpacity(opacity)) => l.fill_opacity = opacity,
                    _ => {}
                }
            }
            b"lsct" | b"lsdk" => {
                l.section = match r.u32()? {
                    1 => Section::OpenFolder,
                    2 => Section::ClosedFolder,
                    3 => Section::Divider,
                    _ => Section::Layer,
                };
                if b.len() >= 12 && &b[4..8] == b"8BIM" {
                    l.blend.copy_from_slice(&b[8..12]);
                }
            }
            b"TySh" => {
                let _ver = r.u16()?;
                let mut t = [0.0; 6];
                for v in &mut t {
                    *v = r.f64()?;
                }
                // A transform with NaN/inf is unusable: fall back to the identity.
                if t.iter().any(|v| !v.is_finite()) {
                    t = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                }
                let _text_ver = r.u16()?;
                let _desc_ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                let text = d.text("Txt ").unwrap_or_default().replace("\r\n", "\n").replace('\r', "\n");
                let text = text.strip_suffix('\n').map(str::to_string).unwrap_or(text);
                let style = d.raw("EngineData").map(|e| engine_data::text_style(&engine_data::parse_lossy(e))).unwrap_or_default();
                let bounds =
                    d.obj("bounds").or_else(|| d.obj("boundingBox")).and_then(|o| Some([o.num("Left")?, o.num("Top ")?, o.num("Rght")?, o.num("Btom")?]));
                l.text = Some(TextInfo { text, transform: t, style, bounds });
            }
            b"lfx2" => {
                let _ver = r.u32()?;
                let _dver = r.u32()?;
                l.effects = Some(descriptor::read_descriptor(&mut r)?);
            }
            b"SoCo" => {
                let _ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                l.fill = d.color("Clr ");
            }
            b"nvrt" => l.adjustment = Some(Adjustment::Invert),
            b"brit" | b"hue2" | b"hue " | b"levl" | b"expA" => {
                if let Some(adjustment) = craft_psd::adjust::Adjustment::from_block(key, b) {
                    let adjustment = adjustment.map_err(|e| error(&e))?;
                    // Photoshop can write brit beside CgEd: retain Effect's first-only
                    // brit policy while other complete typed blocks replace prior values.
                    if key != b"brit" || l.adjustment.is_none() {
                        l.adjustment = Some(from_shared_adjustment(adjustment));
                    }
                }
            }
            b"CgEd" => {
                let _ver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                l.adjustment = Some(Adjustment::BrightnessContrast {
                    brightness: d.num("Brgh").unwrap_or(0.0),
                    contrast: d.num("Cntr").unwrap_or(0.0),
                    legacy: d.bool("useLegacy").unwrap_or(false),
                });
            }
            b"curv" | b"blnc" | b"post" | b"thrs" | b"selc" | b"mixr" | b"phfl" | b"vibA" | b"blwh" | b"grdm" | b"clrL" if l.adjustment.is_none() => {
                l.adjustment = Some(Adjustment::Other(String::from_utf8_lossy(key).into_owned()));
            }
            b"SoLd" | b"SoLE" => {
                l.smart_object = Some(placed::from_craft_smart(
                    craft_psd::smart::SmartObject::from_placed_layer_data_with_limits(b, crate::descriptor::PARSE_LIMITS).map_err(|e| error(&e))?,
                ));
            }
            b"PlLd" if l.smart_object.is_none() => {
                l.smart_object = Some(placed::from_craft_smart(craft_psd::smart::SmartObject::from_placed_layer(b).map_err(|e| error(&e))?));
            }
            b"vmsk" | b"vsms" => {
                let _ver = r.u32()?;
                let flags = r.u32()?;
                l.vector_mask = Some(self.vector_paths(&mut r, flags)?);
            }
            _ => {}
        }
        Ok(())
    }

    fn vector_paths(&self, r: &mut Reader, flags: u32) -> Result<VectorMask> {
        let (w, h) = (self.width as f64, self.height as f64);
        let mut vm = VectorMask { inverted: flags & 1 != 0, disabled: flags & 4 != 0, subpaths: vec![] };
        let fixed = |r: &mut Reader| -> Result<f64> { Ok(r.i32()? as f64 / (1 << 24) as f64) };
        while r.remaining() >= 26 {
            let sel = r.u16()?;
            match sel {
                0 | 3 => {
                    let _n = r.u16()?;
                    r.skip(22)?;
                    vm.subpaths.push(SubPath { closed: sel == 0, knots: vec![] });
                }
                1 | 2 | 4 | 5 => {
                    let mut pts = [[0.0; 2]; 3];
                    for p in &mut pts {
                        let y = fixed(r)?;
                        let x = fixed(r)?;
                        *p = [x * w, y * h];
                    }
                    if let Some(sp) = vm.subpaths.last_mut() {
                        sp.knots.push(pts);
                    }
                }
                _ => r.skip(24)?,
            }
        }
        vm.subpaths.retain(|s| !s.knots.is_empty());
        Ok(vm)
    }

    /// The layer tree, top of the stack first. Groups nest at most [`MAX_GROUP_DEPTH`] deep (see
    /// [`Psd::tree_report`]).
    pub fn tree(&self) -> Vec<Node> {
        self.tree_report().0
    }

    /// The layer tree plus the number of groups nested deeper than [`MAX_GROUP_DEPTH`]. Those
    /// are flattened into their deepest allowed ancestor: their children stay, in order, and
    /// the group record itself becomes a plain [`Node::Layer`], so no layer is lost and the
    /// tree (and its `Drop`, `Clone` and `PartialEq`) cannot recurse deeper than the limit.
    pub fn tree_report(&self) -> (Vec<Node>, usize) {
        let mut stack: Vec<Vec<Node>> = vec![vec![]];
        // Dividers seen while `MAX_GROUP_DEPTH` groups were already open.
        let mut flattened_open = 0usize;
        let mut flattened = 0usize;
        for l in &self.layers {
            match l.section {
                Section::Divider => {
                    if stack.len() > MAX_GROUP_DEPTH {
                        flattened_open += 1;
                    } else {
                        stack.push(vec![]);
                    }
                }
                Section::OpenFolder | Section::ClosedFolder => {
                    if flattened_open > 0 {
                        flattened_open -= 1;
                        flattened += 1;
                        if let Some(top) = stack.last_mut() {
                            top.push(Node::Layer(l.index));
                        }
                        continue;
                    }
                    let children = if stack.len() > 1 { stack.pop().unwrap_or_default() } else { vec![] };
                    if let Some(top) = stack.last_mut() {
                        top.push(Node::Group { layer: l.index, children });
                    }
                }
                Section::Layer => {
                    if let Some(top) = stack.last_mut() {
                        top.push(Node::Layer(l.index));
                    }
                }
            }
        }
        while stack.len() > 1 {
            let c = stack.pop().unwrap_or_default();
            if let Some(top) = stack.last_mut() {
                top.extend(c);
            }
        }
        // Depth is at most `MAX_GROUP_DEPTH`, so recursion is bounded.
        fn rev(v: &mut Vec<Node>) {
            v.reverse();
            for n in v {
                if let Node::Group { children, .. } = n {
                    rev(children);
                }
            }
        }
        let mut out = stack.pop().unwrap_or_default();
        rev(&mut out);
        (out, flattened)
    }
}

/// The header, colour mode data and merged image offset of a document whose layer and mask
/// section cannot be read: `None` when the merged image cannot be reached either.
fn flattened(data: &[u8]) -> Option<(craft_psd::Header, Vec<u8>, usize)> {
    let head = craft_psd::head::HeadView::from_bytes(data).ok()?;
    let at = usize::try_from(head.layer_section_offset).ok()?;
    let len = if head.header.version.is_psb() {
        u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?)
    } else {
        u64::from(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let field = if head.header.version.is_psb() { 8 } else { 4 };
    let merged = at.checked_add(field)?.checked_add(usize::try_from(len).ok()?)?;
    (merged.checked_add(2)? <= data.len()).then(|| (head.header.clone(), head.color_mode_data.to_vec(), merged))
}

/// Linked-file blocks can be most of a file: leave them in the source, read in place.
fn linked(key: &[u8; 4], _len: u64) -> bool {
    craft_psd::smart::is_linked_layer_key(key)
}

/// Reads `data` with craft-psd. Where that loses what the old reader kept, it reads a copy with
/// the obstacle taken out, and the [`Splice`] maps the copy's offsets back into `data`:
/// - a damaged image resource (resources are not part of the model): no resources;
/// - damaged layer records in the section: none there, so a global `Lr16`/`Lr32`/`Layr` block
///   can still give the layers;
/// - a global layer block that starts with its own length (both layouts are accepted), or
///   that follows up to three stray bytes: the block without them.
fn read_file(data: &Arc<[u8]>) -> (std::result::Result<craft_psd::PsdFile, craft_psd::PsdError>, Splice) {
    let read = |d: Arc<[u8]>| craft_psd::PsdFile::from_source_deferring(craft_psd::source::Source::new(d), linked, |_| Ok(false), |_| false).map(|(f, _)| f);
    let layered = |f: &craft_psd::PsdFile| f.layer_info.as_ref().is_some_and(|i| !i.layers.is_empty());
    let retry = |copy: Option<(Vec<u8>, Splice)>, accept: &dyn Fn(&craft_psd::PsdFile) -> bool| {
        let (copy, splice) = copy?;
        read(copy.into()).ok().filter(|f| accept(f)).map(|f| (f, splice))
    };
    let first = read(data.clone());
    let got = match &first {
        Ok(f) if layered(f) => None,
        Ok(_) => retry(prefixed_layer_block(data), &layered),
        Err(_) => retry(without_resources(data), &|_| true).or_else(|| retry(without_section_layers(data), &layered)),
    };
    match got {
        Some((file, splice)) => (Ok(file), splice),
        None => (first, Splice::default()),
    }
}

/// A big-endian unsigned field of `width` bytes at `at`.
fn be(data: &[u8], at: usize, width: usize) -> Option<usize> {
    let b = data.get(at..at.checked_add(width)?)?;
    usize::try_from(b.iter().fold(0u64, |v, &b| v << 8 | u64::from(b))).ok()
}

/// The length-field width of the layer section and its long blocks.
fn field(data: &[u8]) -> usize {
    if data.get(4..6) == Some(&[0, 2]) { 8 } else { 4 }
}

fn without_resources(data: &[u8]) -> Option<(Vec<u8>, Splice)> {
    let res_at = be(data, 26, 4)?.checked_add(30)?;
    let res_len = be(data, res_at, 4).filter(|&n| n > 0)?;
    Splice::apply(data, res_at + 4, res_len, &[(res_at, 4)])
}

fn without_section_layers(data: &[u8]) -> Option<(Vec<u8>, Splice)> {
    let head = craft_psd::head::HeadView::from_bytes(data).ok()?;
    let ls = usize::try_from(head.layer_section_offset).ok()?;
    let w = field(data);
    let li_at = ls.checked_add(w)?;
    let li_len = be(data, li_at, w).filter(|&n| n > 0)?;
    Splice::apply(data, li_at + w, li_len, &[(li_at, w), (ls, w)])
}

/// The global layer block's repair, found by walking the top-level global blocks by their
/// declared lengths (so a linked file's payload is never searched): up to three stray bytes
/// before the first block, or the block's payload starting with its own length.
fn prefixed_layer_block(data: &[u8]) -> Option<(Vec<u8>, Splice)> {
    let head = craft_psd::head::HeadView::from_bytes(data).ok()?;
    let ls = usize::try_from(head.layer_section_offset).ok()?;
    let w = field(data);
    let end = be(data, ls, w)?.checked_add(ls + w)?.min(data.len());
    let li_at = ls + w;
    let gm_at = be(data, li_at, w)?.checked_add(li_at + w)?;
    let blocks = be(data, gm_at, 4)?.checked_add(gm_at + 4)?;
    let mut pos = blocks;
    let is_sig = |i: usize| matches!(data.get(i..i + 4), Some(b"8BIM" | b"8B64"));
    // Stray bytes before the first block hide every block from craft-psd.
    let mut lead = None;
    while pos < end {
        let skip = (0..=3).find(|&k| is_sig(pos + k))?;
        let at = pos + skip;
        // Leading bytes before the first block are recorded before its key is looked at.
        if lead.is_none() && pos == blocks && skip > 0 {
            lead = Some((pos, skip));
        }
        let key: [u8; 4] = data.get(at + 4..at + 8)?.try_into().ok()?;
        let width = if craft_psd::tagged::uses_long_length(head.header.version, &key) { 8 } else { 4 };
        let len = be(data, at + 8, width)?;
        let body = at + 8 + width;
        if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") {
            if let Some((at, n)) = lead {
                return Splice::apply(data, at, n, &[(ls, w)]);
            }
            // The payload's first field reads as a length that fits in it.
            be(data, body, w).filter(|&n| n > 0 && n.checked_add(w).is_some_and(|e| e <= len))?;
            return Splice::apply(data, body, w, &[(at + 8, width), (ls, w)]);
        }
        pos = body.checked_add(len)?;
    }
    None
}

/// A copy of the file with one range taken out, and the way from the copy's offsets back to the
/// original's.
#[derive(Debug, Clone, Copy, Default)]
struct Splice {
    at: u64,
    removed: u64,
}

impl Splice {
    fn map(self, offset: u64) -> u64 {
        if offset >= self.at { offset + self.removed } else { offset }
    }

    /// `data` without the `removed` bytes at `at`, the length fields at `fixes` (offset, width;
    /// all before `at`) shortened to match.
    fn apply(data: &[u8], at: usize, removed: usize, fixes: &[(usize, usize)]) -> Option<(Vec<u8>, Splice)> {
        let tail = data.get(at.checked_add(removed)?..)?;
        let mut copy = Vec::with_capacity(at + tail.len());
        copy.extend_from_slice(data.get(..at)?);
        copy.extend_from_slice(tail);
        for &(f, width) in fixes {
            let v = (be(&copy, f, width)?.checked_sub(removed)? as u64).to_be_bytes();
            copy.get_mut(f..f + width)?.copy_from_slice(&v[8 - width..]);
        }
        Some((copy, Splice { at: at as u64, removed: removed as u64 }))
    }
}

/// Whether the layer section's (unreadable) layer count is negative, which marks the merged
/// image's first extra channel as its transparency.
fn section_merged_alpha(data: &[u8]) -> bool {
    let Ok(head) = craft_psd::head::HeadView::from_bytes(data) else { return false };
    let field = if head.header.version.is_psb() { 8 } else { 4 };
    let Some(at) = usize::try_from(head.layer_section_offset).ok().and_then(|a| a.checked_add(2 * field)) else { return false };
    let has_info = data.get(at - field..at).is_some_and(|l| l.iter().any(|&b| b != 0));
    has_info && data.get(at..at + 2).is_some_and(|c| i16::from_be_bytes([c[0], c[1]]) < 0)
}

/// This crate's error for one from craft-psd.
fn error(e: &craft_psd::PsdError) -> Error {
    match e {
        craft_psd::PsdError::UnexpectedEof { .. } => Error::Truncated,
        craft_psd::PsdError::InvalidSignature { .. } | craft_psd::PsdError::UnsupportedVersion(_) => Error::NotPsd,
        craft_psd::PsdError::Unsupported(s) => Error::Unsupported(s.clone()),
        other => Error::Invalid(other.to_string()),
    }
}

/// A Pascal string's bytes in the system (Mac Roman) encoding; ASCII covers the common case.
fn mac_roman_string(b: &[u8]) -> String {
    b.iter().map(|&b| if b < 0x80 { b as char } else { super::read::mac_roman(b) }).collect()
}

/// App representation only; binary field framing belongs to craft-psd.
fn from_shared_adjustment(value: craft_psd::adjust::Adjustment) -> Adjustment {
    use craft_psd::adjust::Adjustment as Shared;
    match value {
        Shared::Invert => Adjustment::Invert,
        Shared::BrightnessContrast { brightness, contrast, legacy } => Adjustment::BrightnessContrast { brightness, contrast, legacy },
        Shared::HueSaturation { hue, saturation, lightness, colorize } => Adjustment::HueSaturation { hue, saturation, lightness, colorize },
        Shared::Levels { in_black, in_white, gamma, out_black, out_white } => Adjustment::Levels { in_black, in_white, gamma, out_black, out_white },
        Shared::Exposure { exposure, offset, gamma } => Adjustment::Exposure { exposure, offset, gamma },
        Shared::Other(key) => Adjustment::Other(String::from_utf8_lossy(&key).into_owned()),
    }
}
