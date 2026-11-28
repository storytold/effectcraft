//! Transform ▸ Separate Dimensions: X and Y Position arrive as EffectCraft's `positionX` /
//! `positionY` instead of being reported (and the stale combined Position moving the layer).
//! The separated layout is the one of the projects saved by After Effects that showed it
//! (`ADBE Position` flags 2051, `ADBE Position_0` / `_1` stored), written into fixture 040 here.

use crate::common::try_fixture;
use crate::import::helpers::comp;
use effectcraft_aep::import::{ImportOptions, LossCategory, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_keyframe::Value;
use effectcraft_project::Project;

fn set(chunk: &mut Chunk<'_>, at: usize, bytes: &[u8]) {
    if let Body::Data(d) = &mut chunk.body {
        let d = d.to_mut();
        for (i, b) in bytes.iter().enumerate() {
            *d.get_mut(at + i).unwrap() = *b;
        }
    }
}

/// Sets the `tdsb` flags (and with `value` the first `cdat` number) of the Transform property
/// `name` wherever it occurs.
fn edit(chunks: &mut [Chunk<'_>], name: &str, flags: u32, value: Option<f64>) -> usize {
    let mut n = 0;
    for c in chunks.iter_mut() {
        if c.is_list(b"tdgp") {
            let mut pending: Option<String> = None;
            for k in c.children_mut().iter_mut() {
                if k.is(b"tdmn") {
                    pending = k.bytes().map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_string());
                } else if k.is_list(b"tdbs") && pending.take().as_deref() == Some(name) {
                    for p in k.children_mut().iter_mut() {
                        if p.is(b"tdsb") {
                            set(p, 0, &flags.to_be_bytes());
                        }
                        if p.is(b"cdat")
                            && let Some(v) = value
                        {
                            set(p, 0, &v.to_be_bytes());
                        }
                    }
                    n += 1;
                }
            }
        }
        n += edit(c.children_mut(), name, flags, value);
    }
    n
}

fn separated(x: f64, y: f64) -> Vec<u8> {
    let bytes = try_fixture("040-static-position-100-200.aep").unwrap();
    let mut file: AepFile<'_> = parse(&bytes).unwrap();
    assert!(edit(&mut file.chunks, "ADBE Position", 2051, None) > 0);
    assert!(edit(&mut file.chunks, "ADBE Position_0", 1, Some(x)) > 0);
    assert!(edit(&mut file.chunks, "ADBE Position_1", 1, Some(y)) > 0);
    write(&file).unwrap()
}

#[test]
fn separated_position_is_imported_as_x_and_y_position() {
    skip!(try_fixture("040-static-position-100-200.aep"));
    let bytes = separated(300.0, 400.0);
    let none = |_: &str| false;
    let mut project = Project::default();
    let r = import(&mut project, &bytes, &ImportOptions::new(&none)).unwrap();
    assert!(r.report.losses.iter().all(|l| l.category != LossCategory::TransformProperties), "{:?}", r.report.losses);
    let (_, c) = comp(&project, &r, "C");
    let tr = c.layers[0].props.sub("transform").unwrap();
    assert_eq!(tr.prop("positionX").unwrap().value, Value::Scalar(300.0));
    assert_eq!(tr.prop("positionY").unwrap().value, Value::Scalar(400.0));
    // Z Position: not stored, the combined Position's.
    assert_eq!(tr.prop("positionZ").unwrap().value, Value::Scalar(0.0));
    // The parts follow Position, as Separate Dimensions puts them.
    let ids: Vec<&str> = tr.props().map(|p| p.match_id.as_str()).collect();
    let at = ids.iter().position(|m| *m == "position").unwrap();
    assert_eq!(&ids[at + 1..at + 4], &["positionX", "positionY", "positionZ"]);
}

#[test]
fn a_layer_that_is_not_separated_gets_no_parts() {
    let (project, r) = load!("040-static-position-100-200.aep");
    let (_, c) = comp(&project, &r, "C");
    assert!(c.layers[0].props.sub("transform").unwrap().prop("positionX").is_none());
}
