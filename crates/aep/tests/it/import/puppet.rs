//! After Effects' Puppet (`ADBE FreePin3`, #3093) arrives as EffectCraft's Puppet
//! (`ec.distort.puppet`) in its place in the stack, and what its meshes and pins hold is reported
//! mesh by mesh instead of the one line "has the effect 'Puppet'". The effect is built into the
//! committed `synthetic.aep` in place of its Glow (`fx_inject`, in memory), in the tree
//! docs/aep-format.md 3.7 gives for Puppet. That tree comes from an external scripting listing,
//! not a byte sample: these tests prove the reader's walk and its report, not the format.

use effectcraft_aep::Chunk;
use effectcraft_aep::import::{ImportOptions, LossCategory, import};
use effectcraft_project::Project;

use crate::import::fx_inject::{bytes_with_effect, import_bytes, keyed_record, losses, nested, record, tdmn, utf8};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");
const PUPPET: &str = "ADBE FreePin3";
const POS_ATOM: &str = "ADBE FreePin3 PosPin Atom";

fn tdsb(v: u32) -> Chunk<'static> {
    Chunk::data(*b"tdsb", v.to_be_bytes().to_vec())
}

/// A property group of `entries` (match name, node).
fn group(entries: Vec<(&str, Chunk<'static>)>) -> Chunk<'static> {
    let mut kids = vec![tdsb(1)];
    for (name, node) in entries {
        kids.push(tdmn(name));
        kids.push(node);
    }
    Chunk::list(*b"tdgp", kids)
}

/// A pin atom with its hidden vertex records and Position (keyed when `keys`). The values are
/// one-component records: the reader only asks whether a pin is keyed.
fn pin(t: &Chunk<'_>, atom: &'static str, keys: bool) -> (&'static str, Chunk<'static>) {
    let position = if keys { keyed_record(t, &[(0, 1.0), (25_600, 2.0)]) } else { record(t, 1.0) };
    (
        atom,
        group(vec![
            ("ADBE FreePin3 PosPin Vtx Offset", record(t, 0.0)),
            ("ADBE FreePin3 PosPin Vtx Index", record(t, 7.0)),
            ("ADBE FreePin3 PosPin Position", position),
        ]),
    )
}

/// `Mesh 1`: Triangles 500, Expansion 5, four Deform pins (one keyed, one with an atom name
/// nobody knows) and a property among them, no Overlap pin, one Stiffness pin, and a group
/// nobody knows.
fn full_mesh(t: &Chunk<'_>) -> Chunk<'static> {
    let mesh_data =
        Chunk::list(*b"tdbs", vec![tdsb(1), Chunk::data(*b"cdat", vec![0u8; 4]), Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", vec![1, 2, 3, 4])])]);
    let deform = group(vec![
        pin(t, POS_ATOM, false),
        pin(t, POS_ATOM, true),
        pin(t, POS_ATOM, false),
        pin(t, "ADBE FreePin3 Test Atom", false),
        ("ADBE FreePin3 Test Prop", record(t, 3.0)),
    ]);
    let starch = group(vec![("ADBE FreePin3 StarchPin Atom", group(vec![("ADBE FreePin3 StarchPin Amount", record(t, 0.5))]))]);
    group(vec![
        ("ADBE FreePin3 Mesh", mesh_data),
        ("ADBE FreePin3 Mesh Tri Count", record(t, 500.0)),
        ("ADBE FreePin3 Mesh Expansion", record(t, 5.0)),
        ("ADBE FreePin3 PosPins", deform),
        ("ADBE FreePin3 HghtPins", group(vec![])),
        ("ADBE FreePin3 StarchPins", starch),
        ("ADBE FreePin3 Test Group", group(vec![])),
    ])
}

/// The Puppet instance: `fx_switch` is the value group's `tdsb`, `arap` its mesh tree (`None`:
/// none), `extra` more entries of its value group.
fn sspc(fx_switch: u32, arap: Option<Chunk<'static>>, extra: Vec<(&str, Chunk<'static>)>) -> Chunk<'static> {
    let pard = |kind: u8| {
        let mut b = vec![0u8; 148];
        b[15] = kind;
        Chunk::data(*b"pard", b)
    };
    let part = vec![Chunk::data(*b"parn", 2u32.to_be_bytes().to_vec()), tdmn("ADBE FreePin3-0000"), pard(0), tdmn("ADBE Effect Built In Params"), pard(9)];
    let mut values = vec![tdsb(fx_switch)];
    if let Some(arap) = arap {
        values.push(tdmn("ADBE FreePin3 ARAP Group"));
        values.push(arap);
    }
    for (name, node) in extra {
        values.push(tdmn(name));
        values.push(node);
    }
    Chunk::list(*b"sspc", vec![nested(*b"fnam", vec![utf8("Puppet")]), Chunk::list(*b"parT", part), Chunk::list(*b"tdgp", values)])
}

/// The layer that holds `effect`, the effect's place in that layer's stack and the stack's size.
fn stack_place(bytes: &[u8], effect: &str) -> Option<(String, usize, usize)> {
    let mut project = Project::default();
    let none = |_: &str| false;
    import(&mut project, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    project.comps().flat_map(|(_, c)| c.layers.iter()).find_map(|l| {
        let fx: Vec<_> = l.props.sub("effects")?.children.iter().filter_map(|n| n.as_group()).collect();
        let at = fx.iter().position(|g| g.match_id == effect)?;
        Some((l.name.clone(), at, fx.len()))
    })
}

#[test]
fn puppet_arrives_and_its_pins_are_reported_mesh_by_mesh() {
    let bytes = bytes_with_effect(PUPPET, |t| {
        let arap = group(vec![("ADBE FreePin3 Outlines", group(vec![])), ("ADBE FreePin3 Mesh Group", group(vec![("ADBE FreePin3 Mesh Atom", full_mesh(t))]))]);
        sspc(1, Some(arap), vec![("ADBE FreePin3 On Transparent", record(t, 1.0))])
    });
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    let g = g.expect("an EffectCraft Puppet in the Glow's place");
    assert!(g.enabled);
    assert_eq!(g.name, "Puppet");
    // No pin is made up: the rest places are not decoded.
    assert_eq!(effectcraft_effects::puppet::pin_count(&g), 0);
    // In the Glow's place in its layer's stack (the second of two effects there).
    assert!(stack_place(SYNTHETIC, "ec.stylize.glow").is_some_and(|(_, at, n)| at > 0 && n > 1));
    assert_eq!(stack_place(&bytes, effectcraft_effects::puppet::ID), stack_place(SYNTHETIC, "ec.stylize.glow"));
    let l = losses(&r, LossCategory::Effects, PUPPET);
    assert_eq!(l.len(), 2, "{l:#?}");
    assert!(!l.iter().any(|d| d.contains("has the effect")), "not the old one-line report: {l:#?}");
    assert!(l.iter().any(|d| d.ends_with("property ADBE FreePin3 On Transparent (1) not imported")), "{l:#?}");
    let mesh = l.iter().find(|d| d.contains("imported without its meshes and pins")).expect("the mesh report");
    assert!(
        mesh.ends_with(
            ": Mesh 1 (Triangles 500, Expansion 5): Deform 4 pins (1 keyed) [ADBE FreePin3 PosPin Atom, ADBE FreePin3 Test Atom], \
             Overlap no pins, Stiffness 1 pin (0 keyed) [ADBE FreePin3 StarchPin Atom]; not recognised: ADBE FreePin3 Test Prop, ADBE FreePin3 Test Group"
        ),
        "{mesh}"
    );
    assert_eq!(losses(&r, LossCategory::DamagedData, PUPPET), Vec::<String>::new());
}

#[test]
fn many_meshes_are_listed_up_to_ten_and_names_once() {
    let bytes = bytes_with_effect(PUPPET, |_| {
        let mesh = || group(vec![("ADBE FreePin3 Test Group", group(vec![]))]);
        let meshes = (0..12).map(|_| ("ADBE FreePin3 Mesh Atom", mesh())).collect();
        sspc(1, Some(group(vec![("ADBE FreePin3 Mesh Group", group(meshes))])), vec![])
    });
    let (_, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    let l = losses(&r, LossCategory::Effects, PUPPET);
    assert_eq!(l.len(), 1, "{l:#?}");
    assert!(l[0].ends_with("Mesh 9: no pin groups; Mesh 10: no pin groups; and 2 more; not recognised: ADBE FreePin3 Test Group"), "{}", l[0]);
}

#[test]
fn a_puppet_without_a_mesh_loses_nothing_and_keeps_its_fx_switch() {
    let bytes = bytes_with_effect(PUPPET, |_| sspc(0, Some(group(vec![("ADBE FreePin3 Mesh Group", group(vec![]))])), vec![]));
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    let g = g.expect("an EffectCraft Puppet");
    assert!(!g.enabled, "switched off in the Effect Controls");
    assert_eq!(losses(&r, LossCategory::Effects, PUPPET), Vec::<String>::new());
}

#[test]
fn a_puppet_whose_mesh_tree_is_not_found_is_reported() {
    let bytes = bytes_with_effect(PUPPET, |t| sspc(1, None, vec![("ADBE FreePin3 Other Group", group(vec![("x", record(t, 1.0))]))]));
    let (g, r) = import_bytes(&bytes, effectcraft_effects::puppet::ID);
    assert!(g.is_some(), "the effect still arrives");
    let l = losses(&r, LossCategory::Effects, PUPPET);
    assert_eq!(l.len(), 1, "{l:#?}");
    assert!(
        l[0].ends_with(
            "imported without meshes or pins: its mesh group (ADBE FreePin3 ARAP Group / ADBE FreePin3 Mesh Group) was not found or is not a group; \
             not recognised: ADBE FreePin3 Other Group"
        ),
        "{}",
        l[0]
    );
}
