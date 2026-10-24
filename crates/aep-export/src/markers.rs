//! Layer and composition markers (`docs/aep-format.md` 3.11; cases 122, 123, 249, 347-349, 509,
//! 510). A marker group is `tdmn ADBE Marker` + `LIST mrst`: a `tdbs` whose key list holds one
//! 16-byte record per marker (the time in the owner's time units, bytes 4 and 5 = 3, the rest
//! written 0: After Effects leaves session bytes there) and a `LIST mrky` with one `LIST Nmrd` per
//! marker (`NmHd`: cue type, number of cue parameters, duration in 1/600 s, 600, label; then the
//! comment, chapter, URL, frame target and cue point name and the parameter pairs as `Utf8`).

use effectcraft_aep::Chunk;
use effectcraft_project::Marker;

use crate::report::{Collector, LossCategory};
use crate::timebase::TimeBase;
use crate::tree::{R, child_data, kids_mut, list_mut, put, put_u8, put_u32, tpl};

/// The units per second of a marker duration (`NmHd` @12, 249).
const DURATION_UNITS: f64 = 600.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MarkerPlan {
    pub time: i32,
    pub duration: u32,
    pub label: u8,
    /// 4 event cue (also without a cue point), 5 navigation cue (347, 348).
    pub cue: u32,
    /// Comment, chapter, URL, frame target, cue point name (249).
    pub strings: [String; 5],
    pub params: Vec<(String, String)>,
}

/// The markers in the file's units (`tb` is the owner's time base: the composition's, or the
/// layer's for layer markers); what cannot be written is reported.
pub(crate) fn plan(markers: &[Marker], tb: &TimeBase, what: &str, col: &mut Collector) -> Vec<MarkerPlan> {
    let mut out: Vec<MarkerPlan> = Vec::with_capacity(markers.len());
    for m in markers {
        let (u, inexact) = tb.units(m.time);
        let Ok(time) = i32::try_from(u) else {
            col.lose(LossCategory::Markers, format!("{what}: a marker time out of range is not written"));
            continue;
        };
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: marker at {:.4} s moved to the time base", m.time.seconds()));
        }
        let d = m.duration.seconds() * DURATION_UNITS;
        let duration = if d.is_finite() && d > 0.0 { d.round().min(f64::from(u32::MAX)) as u32 } else { 0 };
        if d.is_finite() && d > 0.0 && (d - d.round()).abs() > 1e-6 {
            col.lose(LossCategory::RoundedTimes, format!("{what}: a marker duration is rounded to 1/600 s"));
        }
        if m.protected {
            col.lose(LossCategory::Markers, format!("{what}: the protected-region switch of a marker is not written"));
        }
        let (cue, cue_name, params) = match &m.cue_point {
            Some(c) => (if c.navigation { 5 } else { 4 }, c.name.clone(), c.params.clone()),
            None => (4, String::new(), Vec::new()),
        };
        out.push(MarkerPlan {
            time,
            duration,
            label: crate::plan::label_index(m.label),
            cue,
            strings: [m.comment.clone(), m.chapter.clone(), m.url.clone(), m.frame_target.clone(), cue_name],
            params,
        });
    }
    out.sort_by_key(|m| m.time);
    col.exported.markers = col.exported.markers.saturating_add(out.len());
    out
}

/// The marker group (`tdmn` + `LIST mrst`) for `plans`, cloned from the prototype (249).
pub(crate) fn build(proto: &[Chunk<'static>], plans: &[MarkerPlan]) -> R<Vec<Chunk<'static>>> {
    let mut group = proto.to_vec();
    let mrst = group.get_mut(1).filter(|c| c.is_list(b"mrst")).ok_or_else(|| tpl("marker prototype"))?;
    let n = u32::try_from(plans.len()).map_err(|_| tpl("too many markers"))?;
    {
        let tdbs = list_mut(mrst, b"tdbs")?;
        let list = list_mut(tdbs, b"list")?;
        let lhd3 = child_data(list, b"lhd3")?;
        put_u32(lhd3, 8, n)?;
        put_u32(lhd3, 12, n.div_ceil(4))?;
        put_u32(lhd3, 28, n.div_ceil(4).saturating_mul(4))?;
        let ldat = child_data(list, b"ldat")?;
        ldat.clear();
        for p in plans {
            let mut rec = [0u8; 16];
            put(&mut rec, 0, &p.time.to_be_bytes())?;
            rec[4] = 3;
            rec[5] = 3;
            ldat.extend_from_slice(&rec);
        }
    }
    let mrky = list_mut(mrst, b"mrky")?;
    let nmrd_proto = kids_mut(mrky)?.first().cloned().ok_or_else(|| tpl("marker prototype without Nmrd"))?;
    let mut entries = Vec::with_capacity(plans.len());
    for p in plans {
        let mut nmrd = nmrd_proto.clone();
        {
            let h = child_data(&mut nmrd, b"NmHd")?;
            put_u32(h, 0, p.cue)?;
            put_u32(h, 4, u32::try_from(p.params.len()).unwrap_or(0))?;
            put_u32(h, 8, p.duration)?;
            put_u32(h, 12, 600)?;
            put_u8(h, 16, p.label)?;
        }
        let kids = kids_mut(&mut nmrd)?;
        kids.retain(|c| !c.is(b"Utf8"));
        for s in &p.strings {
            kids.push(Chunk::data(*b"Utf8", s.as_bytes().to_vec()));
        }
        for (k, v) in &p.params {
            kids.push(Chunk::data(*b"Utf8", k.as_bytes().to_vec()));
            kids.push(Chunk::data(*b"Utf8", v.as_bytes().to_vec()));
        }
        entries.push(nmrd);
    }
    *kids_mut(mrky)? = entries;
    Ok(group)
}

/// Inserts the marker group as the first property of a top property group (`tdgp`): right after
/// its `tdsb` / `tdsn` header, before Time Remapping or the Transform group (123, 509).
pub(crate) fn insert(tdgp: &mut Chunk<'static>, group: Vec<Chunk<'static>>) -> R<()> {
    let kids = kids_mut(tdgp)?;
    let at = kids.iter().position(|c| c.is(b"tdmn")).unwrap_or(kids.len());
    for (i, c) in group.into_iter().enumerate() {
        kids.insert(at.saturating_add(i), c);
    }
    Ok(())
}
