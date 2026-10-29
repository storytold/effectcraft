//! Effect instances built in code from the effect table (`fx_defs`): the default `sspc` of every
//! effect type the exporter writes, its project copy (`EfDf`) and the value records of each
//! parameter type (aep-format.md 3.7).
//!
//! What After Effects needs (read-backs in `docs/aep-export.md`, "Built in code"): a `parT` entry
//! per parameter (`tdmn` + a 148-byte `pard`) whose bytes may be zero, since After Effects takes
//! the definitions from the effect itself (e1-zpardfull); no `sdat` (e2-nosdat). The `pard` fields
//! the importer reads back (type, default, valid maximum, slider range, popup item count) are
//! written from the table so that an exported file imports again.

use crate::Chunk;
use crate::effect_map::{Conv, by_match_name};
use crate::export::fx_defs::{BUILT_IN, FxDef, PDef};
use crate::export::synth::{KeyTemplate, NO_NAME, RecSpec, class, tdmn, tdsn};
use crate::{CURVE_IDENTITY, CurvesData, MeshData};

/// The layer a prototype's layer parameters name when their default is the owning layer; the
/// exporter replaces it with the owning layer's id (`effects::instance`).
pub(crate) const PROTO_OWNER: u32 = 1;

/// Layer parameters whose default is None instead of the owning layer (Timewarp's Matte Layer and
/// Warp Layer, 529).
const LAYER_NONE: &[&str] = &["ADBE Timewarp-0029", "ADBE Timewarp-0031"];

/// The record of a parameter type (`pard` code): its components, key template and class word.
pub(crate) fn value_spec(code: u8, range: Option<(f64, f64)>) -> Option<RecSpec> {
    let scalar = |class| RecSpec { dims: 1, template: KeyTemplate::Plain, class, limits: None, byte5: None };
    Some(match code {
        // Sliders carry their range as `tdum` / `tduM`.
        1 | 2 | 10 => RecSpec { limits: Some(range.unwrap_or((0.0, 0.0))), ..scalar(class::EFFECT_NUMBER) },
        3 => scalar(class::EFFECT_NUMBER),
        4 | 7 => scalar(class::EFFECT_CHOICE),
        5 => RecSpec { dims: 4, template: KeyTemplate::Spatial(1), class: class::COLOR, limits: None, byte5: None },
        6 => RecSpec { dims: 2, template: KeyTemplate::Spatial(3), class: class::SPATIAL, limits: None, byte5: None },
        0 | 13 | 14 | 15 => scalar(class::EFFECT_OTHER),
        _ => return None,
    })
}

fn put(b: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(dst) = b.get_mut(at..at + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
}

fn fixed(v: f64) -> [u8; 4] {
    ((v * 65536.0).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32).to_be_bytes()
}

/// The 148-byte `pard` of a parameter: the type at 12, the default at 56 and the copies After
/// Effects reads it from (s8-e1..s12-e2), a popup's item count at 60, the valid range, slider
/// range and display word of a slider (float: `f32` at 104 / 108 / 112 / 116, default 120,
/// display 124; fixed and integer: 124 / 128 / 132 / 136, default 140, display 144). Every other
/// byte is zero (the switches at 48 are not needed: s12-e2-nofl).
pub(crate) fn pard(p: &PDef, popup_items: Option<u16>) -> Vec<u8> {
    let mut b = vec![0u8; 148];
    put(&mut b, 12, &u32::from(p.code).to_be_bytes());
    let d = |i: usize| p.default.get(i).copied().unwrap_or(0.0);
    // After Effects takes the default of a parameter without a value record from here (s8-e1:
    // the copies below are the ones it reads).
    match p.code {
        10 => {
            put(&mut b, 56, &d(0).to_be_bytes());
            // The default again as `f32` at 120, the display word (decimals) at 124.
            put(&mut b, 120, &(d(0) as f32).to_be_bytes());
            put(&mut b, 124, &p.display.to_be_bytes());
        }
        2 => put(&mut b, 56, &fixed(d(0))),
        // An angle: the default again at 60.
        3 => {
            put(&mut b, 56, &fixed(d(0)));
            put(&mut b, 60, &fixed(d(0)));
        }
        1 => put(&mut b, 56, &(d(0).round() as i32).to_be_bytes()),
        // A checkbox: the default again as a byte at 60.
        4 => {
            put(&mut b, 56, &(d(0).round().max(0.0) as u32).to_be_bytes());
            put(&mut b, 60, &[u8::from(d(0) != 0.0)]);
        }
        // A popup: the default again as `u16` at 62 (the item count is at 60).
        7 => {
            let sel = d(0).round().clamp(0.0, f64::from(u16::MAX));
            put(&mut b, 56, &(sel as u32).to_be_bytes());
            put(&mut b, 62, &(sel as u16).to_be_bytes());
        }
        6 => {
            // Fractions of the layer at 56 / 60 and the same in percent at 68 / 72.
            put(&mut b, 56, &fixed(d(0)));
            put(&mut b, 60, &fixed(d(1)));
            put(&mut b, 68, &fixed(d(0) * 100.0));
            put(&mut b, 72, &fixed(d(1) * 100.0));
        }
        5 => {
            // ARGB at 56 and the same at 60.
            let c: Vec<u8> = (0..4).map(|i| d(i).round().clamp(0.0, 255.0) as u8).collect();
            put(&mut b, 56, &c);
            put(&mut b, 60, &c);
        }
        _ => {}
    }
    if p.code == 7 {
        let n = popup_items.unwrap_or_else(|| u16::try_from(p.display).unwrap_or(0));
        put(&mut b, 60, &n.to_be_bytes());
    }
    match p.code {
        10 => {
            if let Some(m) = p.valid_min {
                put(&mut b, 104, &(m as f32).to_be_bytes());
            }
            if let Some(m) = p.valid_max {
                put(&mut b, 108, &(m as f32).to_be_bytes());
            }
            if let Some((lo, hi)) = p.range {
                put(&mut b, 112, &(lo as f32).to_be_bytes());
                put(&mut b, 116, &(hi as f32).to_be_bytes());
            }
        }
        1 | 2 => {
            let enc = |v: f64| if p.code == 2 { fixed(v) } else { (v.round() as i32).to_be_bytes() };
            if let Some(m) = p.valid_min {
                put(&mut b, 124, &enc(m));
            }
            if let Some(m) = p.valid_max {
                put(&mut b, 128, &enc(m));
            }
            if let Some((lo, hi)) = p.range {
                put(&mut b, 132, &enc(lo));
                put(&mut b, 136, &enc(hi));
            }
            // The default again at 140, the display word (decimals, percent) at 144.
            put(&mut b, 140, &enc(d(0)));
            put(&mut b, 144, &p.display.to_be_bytes());
        }
        _ => {}
    }
    b
}

/// The default arbitrary data of an effect (its type-11 parameter), from the decoded layouts
/// (aep-format.md 3.7).
pub(crate) fn default_data(match_name: &str) -> Option<Vec<u8>> {
    match match_name {
        // Master hue, saturation, lightness (0), then six colour ranges of four hue bounds and
        // their hue, saturation and lightness (0): Reds centred at 0 degrees, then every 60, each
        // range 30 degrees wide with 30 degree fall-off (443-446).
        "ADBE HUE SATURATION" => {
            let mut out = vec![0u8; 12];
            for c in 0..6i32 {
                let centre = 60 * c;
                for off in [-45, -15, 15, 45] {
                    out.extend((centre + off).rem_euclid(360).to_be_bytes());
                }
                out.extend([0u8; 12]);
            }
            Some(out)
        }
        // A header word `0x000f10a7` and the version 1, then five channels (RGB, red, green, blue,
        // alpha) of input black, input white, gamma, output black, output white as `f32`
        // (511-516).
        "ADBE Easy Levels2" | "ADBE Pro Levels2" => {
            let mut out = Vec::with_capacity(108);
            out.extend(0x000f_10a7_u32.to_be_bytes());
            out.extend(1u32.to_be_bytes());
            for _ in 0..5 {
                for v in [0.0f32, 1.0, 1.0, 0.0, 1.0] {
                    out.extend(v.to_be_bytes());
                }
            }
            Some(out)
        }
        "ADBE CurvesCustom" => CurvesData { curves: std::array::from_fn(|_| CURVE_IDENTITY.to_vec()) }.to_bytes(),
        // 7 x 7 cells, undistorted (Rows / Columns default 7, 985).
        "ADBE MESH WARP" => {
            let (columns, rows) = (7, 7);
            let vertices = (0..=rows).flat_map(|j| (0..=columns).map(move |i| MeshData::default_vertex(columns, rows, i, j))).collect();
            MeshData { columns, rows, tail: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0d, 0x0d], vertices }.to_bytes()
        }
        _ => None,
    }
}

/// The popup item count of parameter position `index` from the shared effect table.
fn popup_items(fx: &FxDef, index: usize) -> Option<u16> {
    let map = by_match_name(fx.match_name)?;
    let pm = map.params.iter().find(|p| p.index == index)?;
    match pm.conv {
        Conv::Popup { items, .. } => u16::try_from(items.len()).ok(),
        _ => None,
    }
}

/// A record without a value of its own (layer parameters, groups, buttons): `tdsb` `state`, no
/// user name, the header and a zero `cdat`.
fn plain_record(state: u32, extra: Vec<Chunk<'static>>) -> Chunk<'static> {
    let spec = RecSpec { dims: 1, template: KeyTemplate::Plain, class: class::EFFECT_OTHER, limits: None, byte5: None };
    let mut kids = vec![
        Chunk::data(*b"tdsb", state.to_be_bytes().to_vec()),
        tdsn(NO_NAME),
        Chunk::data(*b"tdb4", crate::export::synth::tdb4(&spec, false, 25_600)),
        Chunk::data(*b"cdat", vec![0u8; spec.cdat_len()]),
    ];
    kids.extend(extra);
    Chunk::list(*b"tdbs", kids)
}

/// The arbitrary-data record (4-byte `cdat`, class `0x00060007`, colour-like template 1) and the
/// `LIST aRbs` with the data after it (443, 961).
fn data_record(data: Vec<u8>) -> [Chunk<'static>; 2] {
    let spec = RecSpec { dims: 1, template: KeyTemplate::Spatial(1), class: class::ARBITRARY, limits: None, byte5: None };
    let rec = Chunk::list(
        *b"tdbs",
        vec![
            Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()),
            tdsn(NO_NAME),
            Chunk::data(*b"tdb4", crate::export::synth::tdb4(&spec, false, 25_600)),
            Chunk::data(*b"cdat", vec![0u8; 4]),
        ],
    );
    [rec, Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", data)])]
}

/// The default `sspc` of an effect: its name (`fnam`), the parameter list (`parT`: the count,
/// then `tdmn` + `pard` per parameter, the default arbitrary data after its `pard`), the value
/// group (records only for layer parameters, groups, buttons and arbitrary data, the built-in
/// group) and `pgui` (zero).
pub(crate) fn default_sspc(fx: &FxDef) -> Chunk<'static> {
    let fnam = Chunk { id: *b"fnam", body: crate::Body::Chunks(vec![Chunk::data(*b"Utf8", fx.name.as_bytes().to_vec())]), pad: None };
    let count = u32::try_from(fx.params.len()).unwrap_or(u32::MAX);
    let mut part = vec![Chunk::data(*b"parn", count.to_be_bytes().to_vec())];
    let mut values = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn(NO_NAME)];
    for (i, p) in fx.params.iter().enumerate() {
        let record = fx.record(p);
        part.push(tdmn(&record));
        part.push(Chunk::data(*b"pard", pard(p, popup_items(fx, i))));
        if matches!(p.code, 4 | 7 | 15) {
            // Checkboxes, popups and buttons have a `pdnm` (their label or items); its text may
            // be empty.
            part.push(Chunk { id: *b"pdnm", body: crate::Body::Chunks(vec![Chunk::data(*b"Utf8", Vec::new())]), pad: None });
        }
        if p.id == BUILT_IN {
            values.push(tdmn(&record));
            values.push(Chunk::list(*b"tdgp", vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn(NO_NAME), tdmn("ADBE Group End")]));
            continue;
        }
        match p.code {
            0 => {
                let id = if LAYER_NONE.contains(&record.as_str()) { 0 } else { PROTO_OWNER };
                // The input layer (position 0) and every layer parameter name a layer (`tdpi`);
                // `tdps` 0 (320, 529).
                let state = if i == 0 { 3 } else { 1 };
                values.push(tdmn(&record));
                values.push(plain_record(state, vec![Chunk::data(*b"tdpi", id.to_be_bytes().to_vec()), Chunk::data(*b"tdps", 0u32.to_be_bytes().to_vec())]));
            }
            9 | 13..=15 => {
                values.push(tdmn(&record));
                values.push(plain_record(1, Vec::new()));
            }
            11 => {
                if let Some(data) = default_data(fx.match_name) {
                    part.push(Chunk::data(*b"aRbp", data.clone()));
                    values.push(tdmn(&record));
                    values.extend(data_record(data));
                }
            }
            _ => {}
        }
    }
    values.push(tdmn("ADBE Group End"));
    Chunk::list(*b"sspc", vec![fnam, Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values), Chunk::data(*b"pgui", vec![0u8; 16])])
}
