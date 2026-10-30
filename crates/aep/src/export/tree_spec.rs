//! Property trees described as data: groups and records by match name with their decoded fields
//! (aep-format.md 3.4), built into chunks by [`build`]. The camera, light, shape and text
//! skeletons the exporter fills are tables of these (`tree_defs`).

use crate::Chunk;
use crate::export::synth::{KeyTemplate, RecSpec, key_list, tdb4, tdmn, tdsn};

/// A property record: its state (`tdsb`), user name, the `tdb4` header fields (components, key
/// template, class word, byte 5, decoded bytes that differ from what [`tdb4`] derives), the
/// static value (`cdat`: its length and the leading values, the rest zero) or the record size of
/// an empty key list, the limits, and a layer id (`tdpi` / `tdps`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rec {
    pub state: u32,
    pub label: &'static str,
    pub dims: u8,
    pub template: u8,
    pub class: u32,
    pub byte5: u8,
    pub header: &'static [(u8, u8)],
    pub cdat_len: u16,
    pub cdat: &'static [f64],
    pub key_size: u16,
    pub limits: Option<(f64, f64)>,
    pub tdpi: Option<u32>,
}

/// A node of a property tree.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Node {
    /// A group: match name, state, user name, children (the closing `ADBE Group End` is added).
    G(&'static str, u32, &'static str, &'static [Node]),
    /// A property record under its match name.
    R(&'static str, Rec),
    /// Orientation: its record in an `otst` with `otky` holding one zero `otda` (three degrees)
    /// per key (aep-format.md 3.5, keyed Orientation).
    O(&'static str, Rec),
    /// A path property (`om-s`): its record and `omks` with one empty shape (aep-format.md 3.8).
    P(&'static str, Rec),
}

fn record(r: &Rec) -> Chunk<'static> {
    let template = if r.template == 0 { KeyTemplate::Plain } else { KeyTemplate::Spatial(r.template) };
    let spec = RecSpec { dims: r.dims, template, class: r.class, limits: r.limits, byte5: Some(r.byte5) };
    let keyed = r.key_size > 0;
    let mut header = tdb4(&spec, keyed, 25_600);
    for (at, v) in r.header {
        if let Some(slot) = header.get_mut(usize::from(*at)) {
            *slot = *v;
        }
    }
    let mut kids = vec![Chunk::data(*b"tdsb", r.state.to_be_bytes().to_vec()), tdsn(r.label), Chunk::data(*b"tdb4", header)];
    if keyed {
        kids.push(key_list(u32::from(r.key_size), 0, Vec::new()));
    } else {
        let mut cdat = vec![0u8; usize::from(r.cdat_len)];
        for (slot, v) in cdat.as_chunks_mut::<8>().0.iter_mut().zip(r.cdat) {
            slot.copy_from_slice(&v.to_be_bytes());
        }
        kids.push(Chunk::data(*b"cdat", cdat));
    }
    if let Some((lo, hi)) = r.limits {
        kids.push(Chunk::data(*b"tdum", lo.to_be_bytes().to_vec()));
        kids.push(Chunk::data(*b"tduM", hi.to_be_bytes().to_vec()));
    }
    if let Some(id) = r.tdpi {
        kids.push(Chunk::data(*b"tdpi", id.to_be_bytes().to_vec()));
        kids.push(Chunk::data(*b"tdps", vec![0; 4]));
    }
    Chunk::list(*b"tdbs", kids)
}

/// The chunks of `nodes` (each a `tdmn` and its list).
pub(crate) fn build(nodes: &[Node]) -> Vec<Chunk<'static>> {
    let mut out = Vec::with_capacity(nodes.len() * 2);
    for n in nodes {
        match n {
            Node::G(name, state, label, kids) => {
                let mut body = vec![Chunk::data(*b"tdsb", state.to_be_bytes().to_vec()), tdsn(label)];
                body.extend(build(kids));
                body.push(tdmn("ADBE Group End"));
                out.push(tdmn(name));
                out.push(Chunk::list(*b"tdgp", body));
            }
            Node::R(name, r) => {
                out.push(tdmn(name));
                out.push(record(r));
            }
            Node::P(name, r) => {
                let shap = Chunk::list(
                    *b"shap",
                    vec![Chunk::data(*b"shph", crate::export::masks::shph()), key_list(8, 0, Vec::new()), Chunk::data(*b"omtn", Vec::new())],
                );
                out.push(tdmn(name));
                out.push(Chunk::list(*b"om-s", vec![record(r), Chunk::list(*b"omks", vec![shap])]));
            }
            Node::O(name, r) => {
                out.push(tdmn(name));
                out.push(Chunk::list(*b"otst", vec![record(r), Chunk::list(*b"otky", vec![Chunk::data(*b"otda", vec![0; 24])])]));
            }
        }
    }
    out
}
