use super::*;

// ---------------------------------------------------------------- ASC CDL

/// An ASC Colour Decision List correction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cdl {
    pub slope: [f64; 3],
    pub offset: [f64; 3],
    pub power: [f64; 3],
    pub sat: f64,
}

impl Default for Cdl {
    fn default() -> Self {
        Cdl { slope: [1.0; 3], offset: [0.0; 3], power: [1.0; 3], sat: 1.0 }
    }
}

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

impl Cdl {
    /// Forward CDL. `clamp` = the v1.2 style (results clamped to 0..1 after SOP and saturation).
    pub fn apply(&self, c: [f64; 3], clamp: bool) -> [f64; 3] {
        let mut o = [0.0; 3];
        for i in 0..3 {
            let v = c[i] * self.slope[i] + self.offset[i];
            o[i] = if clamp {
                v.clamp(0.0, 1.0).powf(self.power[i])
            } else if v >= 0.0 {
                v.powf(self.power[i])
            } else {
                v
            };
        }
        let l = o[0] * LUMA[0] + o[1] * LUMA[1] + o[2] * LUMA[2];
        let o = o.map(|v| l + self.sat * (v - l));
        if clamp { o.map(|v| v.clamp(0.0, 1.0)) } else { o }
    }
    /// Exact inverse of [`Cdl::apply`] (inside the clamped range).
    pub fn invert(&self, c: [f64; 3], clamp: bool) -> [f64; 3] {
        let c = if clamp { c.map(|v| v.clamp(0.0, 1.0)) } else { c };
        let l = c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
        let s = if self.sat.abs() < 1e-9 { 1e-9 } else { self.sat };
        let o = c.map(|v| l + (v - l) / s);
        let mut r = [0.0; 3];
        for i in 0..3 {
            let v = if clamp { o[i].clamp(0.0, 1.0) } else { o[i] };
            let pw = self.power[i].max(1e-9);
            let u = if v >= 0.0 { v.powf(1.0 / pw) } else { v };
            let sl = if self.slope[i].abs() < 1e-9 { 1e-9 } else { self.slope[i] };
            r[i] = (u - self.offset[i]) / sl;
        }
        if clamp { r.map(|v| v.clamp(0.0, 1.0)) } else { r }
    }
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = xml.find(&format!("<{name}"))?;
    let gt = open + xml[open..].find('>')?;
    if xml[..gt].ends_with('/') {
        return Some("");
    }
    let close = xml[gt..].find(&format!("</{name}>"))? + gt;
    Some(&xml[gt + 1..close])
}

fn nums3(s: &str) -> Option<[f64; 3]> {
    let v: Vec<f64> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

fn parse_cc(block: &str) -> Cdl {
    let mut c = Cdl::default();
    if let Some(s) = tag(block, "Slope").and_then(nums3) {
        c.slope = s;
    }
    if let Some(s) = tag(block, "Offset").and_then(nums3) {
        c.offset = s;
    }
    if let Some(s) = tag(block, "Power").and_then(nums3) {
        c.power = s;
    }
    if let Some(s) = tag(block, "Saturation").and_then(|t| t.trim().parse().ok()) {
        c.sat = s;
    }
    c
}

/// Parse ASC CDL XML (`.cc`, `.ccc`, `.cdl`): every `<ColorCorrection>` with its `id`.
pub fn parse_cdl_xml(xml: &str) -> Vec<(String, Cdl)> {
    let mut out = vec![];
    let mut rest = xml;
    while let Some(i) = rest.find("<ColorCorrection") {
        let after = &rest[i..];
        let head_end = after.find('>').unwrap_or(after.len());
        let head = &after[..head_end];
        let id = head
            .find("id=")
            .and_then(|k| {
                let q = head[k + 3..].chars().next()?;
                let s = &head[k + 4..];
                s.find(q).map(|e| s[..e].to_string())
            })
            .unwrap_or_default();
        let end = after.find("</ColorCorrection>").map(|e| e + "</ColorCorrection>".len()).unwrap_or(after.len());
        out.push((id, parse_cc(&after[..end])));
        rest = &after[end..];
    }
    out
}

pub(super) fn sl(lo: f64, hi: f64, smin: f64, smax: f64) -> ParamUi {
    slider(lo, hi, smin, smax, 3)
}
