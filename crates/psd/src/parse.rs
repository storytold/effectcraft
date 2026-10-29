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
    pub fn parse(data: impl Into<Arc<[u8]>>) -> Result<Psd> {
        let data: Arc<[u8]> = data.into();
        let mut r = Reader::new(&data);
        if r.bytes(4).ok() != Some(b"8BPS") {
            return Err(Error::NotPsd);
        }
        let version = r.u16()?;
        let psb = match version {
            1 => false,
            2 => true,
            _ => return Err(Error::NotPsd),
        };
        r.skip(6)?;
        let channels = r.u16()?;
        let height = r.u32()?;
        let width = r.u32()?;
        let depth = r.u16()?;
        let mode_v = r.u16()?;
        let mode = ColorMode::from_u16(mode_v).ok_or_else(|| Error::Unsupported(format!("colour mode {mode_v}")))?;
        if !matches!(depth, 1 | 8 | 16 | 32) {
            return Err(Error::Unsupported(format!("{depth}-bit")));
        }
        if width == 0 || height == 0 || width > 300_000 || height > 300_000 {
            return Err(Error::Invalid(format!("size {width}x{height}")));
        }
        // Colour mode data.
        let n = r.u32()? as usize;
        let palette = r.bytes(n)?.to_vec();
        // Image resources.
        let n = r.u32()? as usize;
        r.skip(n)?;
        // Layer and mask information.
        let mut psd = Psd {
            psb,
            width,
            height,
            channels,
            depth,
            mode,
            layers: vec![],
            merged_alpha: false,
            palette,
            linked: vec![],
            warnings: vec![],
            data: data.clone(),
            merged_offset: 0,
        };
        let lm_len = r.length(psb)?;
        let lm_end = r.pos.checked_add(lm_len).filter(|&e| e <= data.len()).ok_or(Error::Truncated)?;
        if lm_len > 0 {
            let li_len = r.length(psb)?;
            let li_end = r.pos.checked_add(li_len).filter(|&e| e <= lm_end).ok_or(Error::Truncated)?;
            if li_len > 0
                && let Err(e) = psd.layer_info(&data, r.pos, li_end)
            {
                // A damaged layer section must not discard the document: keep the header and the
                // merged image, drop the layers and say so. Without a merged image there is
                // nothing to fall back to, so the original error stands.
                if lm_end.checked_add(2).is_none_or(|n| n > data.len()) {
                    return Err(e);
                }
                psd.layers.clear();
                psd.warnings.push(format!("Layers could not be read ({e}); only the flattened image is available"));
            }
            r.pos = li_end;
            if r.pos + 4 <= lm_end {
                let g = r.u32()? as usize;
                r.pos = (r.pos + g).min(lm_end);
            }
            // Global additional layer information (16/32-bit layer info lives here).
            while let Some((key, start, end)) = next_block(&data, &mut r.pos, lm_end, psb) {
                if matches!(&key, b"lnk2" | b"lnkD" | b"lnk3" | b"lnkE") {
                    psd.linked_files(&data, start, end);
                }
                if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") && psd.layers.is_empty() && end > start {
                    // The block holds the layer info, normally without its length field; accept
                    // both layouts.
                    if psd.layer_info(&data, start, end).is_err() || psd.layers.is_empty() {
                        psd.layers.clear();
                        let skip = if psb { 8 } else { 4 };
                        if start + skip < end && psd.layer_info(&data, start + skip, end).is_err() {
                            psd.layers.clear();
                        }
                    }
                }
                r.pos = end;
            }
        }
        if !psd.layers.is_empty() {
            // A 16/32-bit layer block recovered the layers.
            psd.warnings.clear();
        }
        psd.merged_offset = lm_end;
        Ok(psd)
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

    fn layer_info(&mut self, data: &[u8], start: usize, end: usize) -> Result<()> {
        let mut r = Reader::at(&data[..end], start);
        let count = r.i16()?;
        self.merged_alpha = count < 0;
        let n = count.unsigned_abs() as usize;
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            layers.push(self.layer_record(&mut r, i)?);
        }
        // Channel image data follows in record order.
        for l in &mut layers {
            for c in &mut l.channels {
                c.offset = r.pos;
                r.skip(c.len)?;
            }
        }
        self.layers = layers;
        Ok(())
    }

    fn layer_record(&self, r: &mut Reader, index: usize) -> Result<Layer> {
        let psb = self.psb;
        let (top, left, bottom, right) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        let nch = r.u16()? as usize;
        if nch > 64 {
            return Err(Error::Invalid("channel count".into()));
        }
        let mut channels = Vec::with_capacity(nch);
        for _ in 0..nch {
            let id = r.i16()?;
            let len = r.length(psb)?;
            channels.push(ChannelInfo { id, offset: 0, len });
        }
        if r.bytes(4)? != b"8BIM" {
            return Err(Error::Invalid("layer record signature".into()));
        }
        let blend = r.tag()?;
        let opacity = r.u8()?;
        let clipping = r.u8()? != 0;
        let flags = r.u8()?;
        r.skip(1)?;
        let extra = r.u32()? as usize;
        let extra_end = r.pos.checked_add(extra).filter(|&e| e <= r.data.len()).ok_or(Error::Truncated)?;
        let mut l = Layer {
            index,
            rect: Rect { top, left, bottom, right },
            blend,
            opacity,
            fill_opacity: 255,
            clipping,
            hidden: flags & 2 != 0,
            protect_transparency: flags & 1 != 0,
            channels,
            ..Default::default()
        };
        // Layer mask data.
        let mlen = r.u32()? as usize;
        let mstart = r.pos;
        if mlen >= 18 {
            let rect = Rect { top: r.i32()?, left: r.i32()?, bottom: r.i32()?, right: r.i32()? };
            let default_color = r.u8()?;
            let mflags = r.u8()?;
            l.mask = Some(LayerMask { rect, default_color, disabled: mflags & 2 != 0, inverted: mflags & 4 != 0 });
        }
        r.pos = mstart + mlen;
        // Blending ranges.
        let blen = r.u32()? as usize;
        r.skip(blen)?;
        l.name = r.pascal(4)?;
        let mut pos = r.pos;
        let data = r.data;
        while let Some((key, start, end)) = next_block(data, &mut pos, extra_end, psb) {
            l.keys.push(key);
            let block = &data[start..end];
            // A malformed block only loses that block's information.
            let _ = self.layer_block(&mut l, &key, block);
            pos = end;
        }
        r.pos = extra_end;
        Ok(l)
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
                let style = d.raw("EngineData").map(|e| engine_data::text_style(&engine_data::parse(e))).unwrap_or_default();
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

/// The next additional-information block at `*pos` (signature `8BIM`/`8B64`, key, length):
/// returns (key, data start, data end). Skips up to 3 padding bytes before a signature.
fn next_block(data: &[u8], pos: &mut usize, end: usize, psb: bool) -> Option<([u8; 4], usize, usize)> {
    let mut p = *pos;
    for _ in 0..4 {
        if p + 12 > end {
            return None;
        }
        if &data[p..p + 4] == b"8BIM" || &data[p..p + 4] == b"8B64" {
            break;
        }
        p += 1;
    }
    if p + 12 > end || !(&data[p..p + 4] == b"8BIM" || &data[p..p + 4] == b"8B64") {
        return None;
    }
    let mut key = [0u8; 4];
    key.copy_from_slice(&data[p + 4..p + 8]);
    let wide =
        psb && matches!(&key, b"LMsk" | b"Lr16" | b"Lr32" | b"Layr" | b"Mt16" | b"Mt32" | b"Mtrn" | b"Alph" | b"FMsk" | b"lnk2" | b"FEid" | b"FXid" | b"PxSD");
    let mut r = Reader::at(&data[..end], p + 8);
    let len = r.length(wide).ok()?;
    let start = r.pos;
    let stop = start.checked_add(len)?;
    if stop > end {
        return None;
    }
    *pos = stop;
    Some((key, start, stop))
}
