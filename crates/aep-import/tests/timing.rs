//! A layer of timed footage ends at its source's end, as After Effects ends it when it opens a
//! project (export test f1, aep-export R31): the three-frame OpenEXR sequence layer of 603 and the
//! 1 s WAV layer of 351 with their out points moved to 5 s import as long as their source; the
//! remapped layer of 354 keeps its out point.

use effectcraft_aep::{Body, Chunk, parse, write};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::{ItemKind, LayerSource, Project};

fn fixture(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    std::fs::read(dir.join(name)).expect(name)
}

/// Moves the out point of every `Layr` of the file to `seconds` (relative to its start).
fn with_out_point(bytes: &[u8], seconds: u32) -> Vec<u8> {
    fn walk(cs: &mut [Chunk<'static>], seconds: u32) {
        for c in cs.iter_mut() {
            if c.is_list(b"Layr") {
                if let Body::List { children, .. } = &mut c.body
                    && let Some(ldta) = children.iter_mut().find(|k| k.is(b"ldta"))
                    && let Body::Data(d) = &mut ldta.body
                {
                    let den = u32::from_be_bytes(d[32..36].try_into().unwrap());
                    let mut v = d.to_vec();
                    v[28..32].copy_from_slice(&(seconds * den).to_be_bytes());
                    *d = v.into();
                }
                continue;
            }
            if let Body::List { children, .. } = &mut c.body {
                walk(children, seconds);
            }
        }
    }
    let mut f = parse(bytes).expect("parse").into_owned();
    walk(&mut f.chunks, seconds);
    write(&f).expect("write")
}

/// (layer span, source duration) of every footage layer, in seconds.
fn footage_layers(bytes: &[u8]) -> Vec<(f64, f64)> {
    let mut p = Project::default();
    let none = |_: &str| false;
    import(&mut p, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
    let mut out = Vec::new();
    for item in p.items.values() {
        let ItemKind::Comp(c) = &item.kind else { continue };
        for l in &c.layers {
            if let LayerSource::Footage { item } = l.source
                && let Some(ItemKind::Footage(f)) = p.item(item).map(|i| &i.kind)
            {
                out.push(((l.out_point - l.start_time).seconds(), f.duration.seconds()));
            }
        }
    }
    out
}

#[test]
fn a_footage_layer_ends_at_its_source() {
    for name in ["603-footage-exrseq-0101-as-layer.aep", "351-footage-wav-as-layer.aep"] {
        let long = with_out_point(&fixture(name), 5);
        let layers = footage_layers(&long);
        assert_eq!(layers.len(), 1, "{name}");
        let (span, source) = layers[0];
        assert!(source > 0.0 && source < 5.0, "{name}: {source}");
        assert!((span - source).abs() < 1e-9, "{name}: {span} vs {source}");
    }
    // The remapped layer keeps its 5 s.
    let remapped = with_out_point(&fixture("354-footage-pngseq-timeremap.aep"), 5);
    let layers = footage_layers(&remapped);
    assert!((layers[0].0 - 5.0).abs() < 1e-9, "{layers:?}");
}

#[test]
fn a_reversed_layer_plays_backwards() {
    // 982: stretch -100 %, start 5 s, Opacity keys at layer time 1 s (20 %) and 5 s (100 %);
    // After Effects reads 80 % at comp 1 s and 40 % at 3 s (MANIFEST).
    let mut p = effectcraft_project::Project::default();
    let none = |_: &str| false;
    let opts = effectcraft_aep_import::ImportOptions { allow_losses: false, ..effectcraft_aep_import::ImportOptions::new(&none) };
    effectcraft_aep_import::import(&mut p, &fixture("982-layer-stretch-minus100-opacity-keys.aep"), &opts).expect("lossless");
    let l = p.comps().next().map(|(_, c)| c.layers[0].clone()).expect("layer");
    assert_eq!(l.stretch, -100.0);
    assert_eq!((l.in_point.seconds(), l.out_point.seconds()), (0.0, 5.0));
    let op = l.props.prop("transform/opacity").expect("opacity");
    for (comp_s, want) in [(1.0, 80.0), (3.0, 40.0)] {
        let t = l.layer_time(effectcraft_time::Tick::from_seconds_f64(comp_s));
        let v = op.value_at(t).as_f64();
        assert!((v - want).abs() < 0.01, "{comp_s}: {v}");
    }
}
