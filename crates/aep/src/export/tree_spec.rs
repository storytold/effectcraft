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

/// Prints the Rust table of a prototype's children (development: the tables in `tree_defs`
/// were listed this way from our own cases, through the decoded fields only).
#[cfg(test)]
#[allow(clippy::redundant_guards)]
pub(crate) fn describe(kids: &[Chunk<'_>], depth: usize) -> String {
    use crate::Body;
    let pad = "    ".repeat(depth);
    let name_of = |c: &Chunk<'_>| String::from_utf8_lossy(c.bytes().unwrap_or_default().split(|b| *b == 0).next().unwrap_or_default()).into_owned();
    let label_of = |c: &Chunk<'_>| {
        c.child(b"tdsn")
            .and_then(|t| t.children().iter().find(|u| u.is(b"Utf8")).and_then(|u| u.bytes()))
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    };
    let rec = |c: &Chunk<'_>| -> String {
        let t = c.child(b"tdb4").and_then(|x| x.bytes()).expect("tdb4");
        let state = u32::from_be_bytes(c.child(b"tdsb").and_then(|x| x.bytes()).expect("tdsb").try_into().expect("u32"));
        let (dims, template, byte5) = (t[3], t[7], t[5]);
        let class = u32::from_be_bytes(t[8..12].try_into().expect("u32"));
        let key_size =
            c.child_list(b"list").and_then(|l| l.child(b"lhd3")).and_then(|h| h.bytes()).map_or(0, |h| u32::from_be_bytes(h[16..20].try_into().expect("u32")));
        let tpl = if template == 0 { KeyTemplate::Plain } else { KeyTemplate::Spatial(template) };
        let spec = RecSpec { dims, template: tpl, class, limits: None, byte5: Some(byte5) };
        let derived = tdb4(&spec, key_size > 0, 25_600);
        let header: Vec<String> =
            [3usize, 5, 7, 68, 76, 77, 79, 119, 120].iter().filter(|i| derived[**i] != t[**i]).map(|i| format!("({i}, {})", t[*i])).collect();
        let (cdat_len, cdat) = match c.child(b"cdat").and_then(|x| x.bytes()) {
            Some(d) => {
                let mut v: Vec<f64> = d.as_chunks::<8>().0.iter().map(|b| f64::from_be_bytes(*b)).collect();
                while v.last() == Some(&0.0) {
                    v.pop();
                }
                (d.len(), v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", "))
            }
            None => (0, String::new()),
        };
        let lim = match (c.child(b"tdum").and_then(|x| x.bytes()), c.child(b"tduM").and_then(|x| x.bytes())) {
            (Some(a), Some(b)) => format!("Some(({:?}, {:?}))", f64::from_be_bytes(a.try_into().expect("f64")), f64::from_be_bytes(b.try_into().expect("f64"))),
            _ => "None".into(),
        };
        let tdpi = c.child(b"tdpi").and_then(|x| x.bytes()).map_or("None".into(), |b| format!("Some({})", u32::from_be_bytes(b.try_into().expect("u32"))));
        for k in c.children() {
            assert!(
                [*b"tdsb", *b"tdsn", *b"tdb4", *b"cdat", *b"tdum", *b"tduM", *b"tdpi", *b"tdps"].contains(&k.id) || k.is_list(b"list"),
                "record child {:?}",
                String::from_utf8_lossy(&k.id)
            );
        }
        format!(
            "Rec {{ state: {state}, label: {:?}, dims: {dims}, template: {template}, class: 0x{class:08x}, byte5: 0x{byte5:02x}, header: &[{}], cdat_len: {cdat_len}, cdat: &[{cdat}], key_size: {key_size}, limits: {lim}, tdpi: {tdpi} }}",
            label_of(c),
            header.join(", ")
        )
    };
    let mut out = String::new();
    let mut i = 0;
    while i < kids.len() {
        let c = &kids[i];
        if !c.is(b"tdmn") {
            i += 1;
            continue;
        }
        let name = name_of(c);
        let Some(node) = kids.get(i + 1).filter(|n| !n.is(b"tdmn")) else {
            assert_eq!(name, "ADBE Group End", "bare match name");
            i += 1;
            continue;
        };
        match &node.body {
            Body::List { list_type, children } if list_type == b"tdgp" => {
                let state = u32::from_be_bytes(node.child(b"tdsb").and_then(|x| x.bytes()).expect("tdsb").try_into().expect("u32"));
                out.push_str(&format!("{pad}G({name:?}, {state}, {:?}, &[\n{}{pad}]),\n", label_of(node), describe(children, depth + 1)));
            }
            Body::List { list_type, .. } if list_type == b"tdbs" => out.push_str(&format!("{pad}R({name:?}, {}),\n", rec(node))),
            Body::List { list_type, children } if list_type == b"om-s" => {
                let r = children.iter().find(|k| k.is_list(b"tdbs")).expect("path record");
                out.push_str(&format!(
                    "{pad}P({name:?}, {}),
",
                    rec(r)
                ));
            }
            Body::List { list_type, children } if list_type == b"otst" => {
                let r = children.iter().find(|k| k.is_list(b"tdbs")).expect("otst record");
                out.push_str(&format!("{pad}O({name:?}, {}),\n", rec(r)));
            }
            _ => panic!("{name}: node {:?}", String::from_utf8_lossy(&node.id)),
        }
        i += 2;
    }
    out
}

#[cfg(test)]
mod listing {
    fn show(ty: &[u8], kids: &[crate::Chunk<'_>]) {
        println!("// {}\n{}", String::from_utf8_lossy(ty), super::describe(kids, 1));
    }

    /// Prints the tables of the object and text prototypes (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_object_trees() {
        let f = crate::parse(crate::export::template::OBJECTS).unwrap();
        for ty in [b"camb", b"spot", b"pont", b"shpb"] {
            let block = f.list(ty).unwrap();
            let layr = block.children().iter().find(|c| c.is_list(b"Layr")).unwrap();
            show(ty, layr.child_list(b"tdgp").unwrap().children());
        }
        for ty in [b"opts", b"shpi", b"shpr"] {
            show(ty, f.list(ty).unwrap().children());
        }
        let b = crate::parse(crate::export::template::BASE).unwrap();
        let fold = b.list(b"Fold").unwrap();
        let comp = fold.children().iter().find(|c| c.is_list(b"Item")).unwrap();
        let layr = comp.children().iter().find(|c| c.is_list(b"Layr")).unwrap();
        let root = layr.child_list(b"tdgp").unwrap().children();
        let at = root.iter().position(|c| c.is(b"tdmn") && c.bytes().unwrap().starts_with(b"ADBE Transform Group")).unwrap();
        show(b"base-transform", &root[at..at + 2]);
        let t = crate::parse(crate::export::template::TEXT).unwrap();
        for ty in [b"tanm", b"tani", b"tarc"] {
            show(ty, t.list(ty).unwrap().children());
        }
    }
}
