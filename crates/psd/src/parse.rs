use super::limits::{Error, Result};
use super::model::{
    Adjustment, ChannelInfo, ColorMode, Layer, LayerMask, LinkedFile, MAX_GROUP_DEPTH, Node, Rect, Section, SmartObject, SubPath, TextInfo, VectorMask,
};
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
        let (read, shift) = read_file(&data);
        let (head, file, warnings) = match read {
            Ok(file) => {
                // The compression field before the merged image's data.
                let merged = file.image_data.stored.as_ref().map_or(data.len(), |s| ((s.offset + shift) as usize).saturating_sub(2));
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
            psd.layers = info.layers.iter().enumerate().map(|(i, rec)| psd.layer(rec, i, shift)).collect();
        }
        for b in &file.global_blocks {
            if let Some(stored) = b.stored.as_ref().filter(|_| linked(&b.key, 0)) {
                let start = (stored.offset + shift) as usize;
                psd.linked_files(&data, start, start.saturating_add(stored.len as usize).min(data.len()));
            }
        }
        Ok(psd)
    }

    /// One layer record of the model, its additional information read into [`Layer`].
    /// `shift` moves craft-psd's offsets back into `self.data` (see [`read_file`]).
    fn layer(&self, rec: &craft_psd::LayerRecord, index: usize, shift: u64) -> Layer {
        let channels = rec
            .channels
            .iter()
            .map(|c| match &c.stored {
                // `ChannelInfo` covers the compression field and the data after it.
                Some(s) => ChannelInfo { id: c.id, offset: ((s.offset + shift) as usize).saturating_sub(2), len: s.len as usize + 2 },
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
            // A malformed block only loses that block's information.
            let _ = self.layer_block(&mut l, &b.key, &b.data);
        }
        l
    }

    /// The embedded bytes of linked file `uuid`.
    pub fn linked_data(&self, uuid: &str) -> Option<&[u8]> {
        let f = self.linked.iter().find(|f| f.uuid == uuid)?;
        self.data.get(f.data.clone()?)
    }

    /// Linked layer data records (spec: "Linked Layer"): length, kind (`liFD` embedded data,
    /// `liFE` external file, `liFA` alias), version, unique id, original file name, type,
    /// creator, data length, optional descriptors, then the data. A damaged record ends the list.
    fn linked_files(&mut self, data: &[u8], start: usize, end: usize) {
        let mut pos = start;
        while pos + 8 <= end {
            let mut r = Reader::at(&data[..end], pos);
            let Ok(len) = r.u64() else { break };
            let body = r.pos;
            let Some(rec_end) = body.checked_add(len as usize).filter(|e| *e <= end) else { break };
            let mut r = Reader::at(&data[..rec_end], body);
            let rec = (|| -> Result<LinkedFile> {
                let kind = r.tag()?;
                let version = r.u32()?;
                let n = r.u8()? as usize;
                let uuid = String::from_utf8_lossy(r.bytes(n)?).into_owned();
                let name = descriptor::read_unicode(&mut r)?;
                let file_type = String::from_utf8_lossy(&r.tag()?).trim().to_string();
                let _creator = r.tag()?;
                let dlen = r.u64()? as usize;
                if r.u8()? != 0 {
                    let _v = r.u32()?;
                    descriptor::read_descriptor(&mut r)?;
                }
                let mut out = LinkedFile { uuid, name, file_type, data: None };
                match &kind {
                    b"liFD" => {
                        let s = r.pos;
                        r.skip(dlen)?;
                        out.data = Some(s..s + dlen);
                    }
                    b"liFE" => {
                        let _v = r.u32()?;
                        descriptor::read_descriptor(&mut r)?;
                        if version > 3 {
                            r.skip(4 + 4 + 8)?;
                        }
                        let _size = r.u64()?;
                        if version > 2 && r.remaining() >= dlen && dlen > 0 {
                            let s = r.pos;
                            out.data = Some(s..s + dlen);
                        }
                    }
                    _ => {}
                }
                Ok(out)
            })();
            match rec {
                Ok(f) => self.linked.push(f),
                Err(_) => break,
            }
            pos = rec_end.div_ceil(4) * 4;
        }
    }

    fn layer_block(&self, l: &mut Layer, key: &[u8; 4], b: &[u8]) -> Result<()> {
        let mut r = Reader::new(b);
        match key {
            b"luni" => {
                let name = descriptor::read_unicode(&mut r)?;
                if !name.is_empty() {
                    l.name = name;
                }
            }
            b"lyid" => l.id = Some(r.u32()?),
            b"iOpa" => l.fill_opacity = r.u8()?,
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
            b"brit" => {
                let brightness = r.i16()? as f64;
                let contrast = r.i16()? as f64;
                if l.adjustment.is_none() {
                    l.adjustment = Some(Adjustment::BrightnessContrast { brightness, contrast, legacy: true });
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
            b"hue2" | b"hue " => {
                let _ver = r.u16()?;
                let colorize = r.u8()? != 0;
                r.skip(1)?;
                let ch = (r.i16()? as f64, r.i16()? as f64, r.i16()? as f64);
                let m = (r.i16()? as f64, r.i16()? as f64, r.i16()? as f64);
                let (hue, saturation, lightness) = if colorize { ch } else { m };
                l.adjustment = Some(Adjustment::HueSaturation { hue, saturation, lightness, colorize });
            }
            b"levl" => {
                let _ver = r.u16()?;
                let (ib, iw, ob, ow, g) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?, r.i16()?);
                l.adjustment =
                    Some(Adjustment::Levels { in_black: ib as f64, in_white: iw as f64, gamma: g as f64 / 100.0, out_black: ob as f64, out_white: ow as f64 });
            }
            b"expA" => {
                let _ver = r.u16()?;
                let mut finite = |default: f64| -> Result<f64> {
                    let v = f64::from(r.f32()?);
                    Ok(if v.is_finite() { v } else { default })
                };
                let exposure = finite(0.0)?;
                let offset = finite(0.0)?;
                let gamma = finite(1.0)?;
                l.adjustment = Some(Adjustment::Exposure { exposure, offset, gamma });
            }
            b"curv" | b"blnc" | b"post" | b"thrs" | b"selc" | b"mixr" | b"phfl" | b"vibA" | b"blwh" | b"grdm" | b"clrL" if l.adjustment.is_none() => {
                l.adjustment = Some(Adjustment::Other(String::from_utf8_lossy(key).into_owned()));
            }
            b"SoLd" | b"SoLE" => {
                let _id = r.tag()?;
                let _ver = r.u32()?;
                let _dver = r.u32()?;
                let d = descriptor::read_descriptor(&mut r)?;
                let uuid = d.text("Idnt").unwrap_or_default().to_string();
                let t = placed::eight(&d, "Trnf").unwrap_or([0.0; 8]);
                // The non-affine transform when present (perspective), else the transform.
                let t = placed::eight(&d, "nonAffineTransform").unwrap_or(t);
                let size = d.obj("Sz  ").and_then(|s| Some([s.num("Wdth")?, s.num("Hght")?]));
                let warp = d.obj("warp").and_then(placed::Warp::read);
                l.smart_object = Some(SmartObject { uuid, quad: placed::quad_from(t), size, warp });
            }
            b"PlLd" if l.smart_object.is_none() => {
                let _id = r.tag()?;
                let _ver = r.u32()?;
                let n = r.u8()? as usize;
                let uuid = String::from_utf8_lossy(r.bytes(n)?).into_owned();
                let _page = r.u32()?;
                let _pages = r.u32()?;
                let _aa = r.u32()?;
                let _ty = r.u32()?;
                let mut t = [0.0; 8];
                for v in &mut t {
                    *v = r.f64_finite()?;
                }
                l.smart_object = Some(SmartObject { uuid, quad: placed::quad_from(t), size: None, warp: None });
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
    matches!(key, b"lnk2" | b"lnkD" | b"lnk3" | b"lnkE")
}

/// Reads `data` with craft-psd. The image resources are not part of the model, so when the file
/// does not read, it is read again with an empty resource section: a damaged resource must not
/// cost the layers. The second value is how far that copy's offsets sit before `data`'s.
fn read_file(data: &Arc<[u8]>) -> (std::result::Result<craft_psd::PsdFile, craft_psd::PsdError>, u64) {
    let read = |d: Arc<[u8]>| craft_psd::PsdFile::from_source_deferring(craft_psd::source::Source::new(d), linked, |_| Ok(false), |_| false).map(|(f, _)| f);
    let first = read(data.clone());
    let be32 = |at: usize| data.get(at..at.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if first.is_ok() {
        return (first, 0);
    }
    let Some(res_at) = be32(26).and_then(|cmd| cmd.checked_add(30)) else { return (first, 0) };
    let Some(res_len) = be32(res_at).filter(|&n| n > 0) else { return (first, 0) };
    let Some(rest) = res_at.checked_add(4 + res_len).and_then(|end| data.get(end..)) else { return (first, 0) };
    let mut copy = Vec::with_capacity(res_at + 4 + rest.len());
    copy.extend_from_slice(&data[..res_at]);
    copy.extend_from_slice(&[0; 4]);
    copy.extend_from_slice(rest);
    match read(copy.into()) {
        Ok(file) => (Ok(file), res_len as u64),
        Err(_) => (first, 0),
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
