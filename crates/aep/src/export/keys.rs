//! Key lists shared by the exporters: the times of a property's keys in the file's units, the
//! spatial key record, and the rewrite of a key list (`LIST list`: `lhd3` + `ldat`, aep-format.md
//! 3.5).

use effectcraft_keyframe::Keyframe;

use crate::Chunk;
use crate::export::plan::KeyPlan;
use crate::export::report::{Collector, LossCategory};
use crate::export::timebase::TimeBase;
use crate::export::tree::{R, child_data, kids_mut, list_mut, put, put_f64, put_u8, put_u16, put_u32, tpl};

/// The keys of a property at their times in the file's units, in time order. A key that does not
/// fall on the time base is reported as moved; keys that end up at the same time are merged (the
/// first one is kept) and reported once. `noun` names the key in the report ("key", "path key").
pub(crate) fn timed_keys<'k>(keys: &'k [Keyframe], tb: &TimeBase, what: &str, noun: &str, col: &mut Collector) -> Vec<(i64, &'k Keyframe)> {
    let mut timed: Vec<(i64, &Keyframe)> = Vec::with_capacity(keys.len());
    for k in keys {
        let (u, inexact) = tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: {noun} at {:.4} s moved to the time base", k.time.seconds()));
        }
        timed.push((u, k));
    }
    timed.sort_by_key(|(u, _)| *u);
    let before = timed.len();
    timed.dedup_by_key(|(u, _)| *u);
    if timed.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: {noun}s at the same time were merged"));
    }
    timed
}

/// A key of a point (flags `0x0007`, `u32@8` the automatic tangent switch; the spatial layout of
/// a 2D Position, 853) or a colour (flags `0x0001`, 854): the temporal ease of a point at 24..55,
/// then the `n` values at 56 and, for a point, the spatial in and out tangents after them.
pub(crate) fn spatial_record(k: &KeyPlan, n: usize, point: bool, size: usize) -> R<Vec<u8>> {
    let mut r = vec![0u8; size];
    put(&mut r, 0, &k.time.to_be_bytes())?;
    put_u8(&mut r, 4, k.in_interp)?;
    put_u8(&mut r, 5, k.out_interp)?;
    let base: u16 = if point { 0x0007 } else { 0x0001 };
    put_u16(&mut r, 6, base | (u16::from(k.label) << 6))?;
    if point {
        put_u32(&mut r, 8, u32::from(k.spatial_auto))?;
        let (i, o) = (k.in_ease.first().copied().unwrap_or_default(), k.out_ease.first().copied().unwrap_or_default());
        put_f64(&mut r, 24, i.speed)?;
        put_f64(&mut r, 32, i.influence)?;
        put_f64(&mut r, 40, o.speed)?;
        put_f64(&mut r, 48, o.influence)?;
    }
    for j in 0..n {
        put_f64(&mut r, 56 + 8 * j, k.value.get(j).copied().unwrap_or(0.0))?;
        if point {
            put_f64(&mut r, 56 + 8 * n + 8 * j, k.spatial_in.get(j).copied().unwrap_or(0.0))?;
            put_f64(&mut r, 56 + 16 * n + 8 * j, k.spatial_out.get(j).copied().unwrap_or(0.0))?;
        }
    }
    Ok(r)
}

/// Fills a key list (`LIST list`) with `count` records of `size` bytes: the count words of `lhd3`
/// and `ldat` (added when the list has none).
pub(crate) fn fill_key_list(list: &mut Chunk<'static>, records: Vec<u8>, count: usize, size: usize) -> R<()> {
    let n = u32::try_from(count).map_err(|_| tpl("too many keys"))?;
    let size = u32::try_from(size).map_err(|_| tpl("record size"))?;
    crate::export::synth::put_key_counts(child_data(list, b"lhd3")?, n, size)?;
    let kids = kids_mut(list)?;
    match kids.iter_mut().find(|c| c.is(b"ldat")) {
        Some(c) => *c = Chunk::data(*b"ldat", records),
        None => kids.push(Chunk::data(*b"ldat", records)),
    }
    Ok(())
}

/// Rewrites the key list of a keyed property record (`LIST tdbs`), see [`fill_key_list`].
pub(crate) fn set_key_list(tdbs: &mut Chunk<'static>, records: Vec<u8>, count: usize, size: usize) -> R<()> {
    fill_key_list(list_mut(tdbs, b"list")?, records, count, size)
}

/// Puts the key list `list` in the place of the static value (`cdat`) of a property record.
pub(crate) fn replace_cdat(tdbs: &mut Chunk<'static>, list: Chunk<'static>) -> R<()> {
    let slot = kids_mut(tdbs)?.iter_mut().find(|c| c.is(b"cdat")).ok_or_else(|| tpl("record without cdat"))?;
    *slot = list;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::synth::key_list;

    /// Keys are timed sorted merged and reported.
    #[test]
    fn keys_are_timed_sorted_merged_and_reported() {
        use effectcraft_keyframe::Value;
        use effectcraft_time::{TICKS_PER_SECOND, Tick};
        let tb = TimeBase { frame_duration: 1024, timescale: 25600 };
        // Out of order; 1/3 s is not on the time base; two keys meet at 1 s (the first one kept).
        let keys = [
            Keyframe::new(Tick(TICKS_PER_SECOND), Value::Scalar(1.0)),
            Keyframe::new(Tick(TICKS_PER_SECOND / 3), Value::Scalar(2.0)),
            Keyframe::new(Tick(0), Value::Scalar(3.0)),
            Keyframe::new(Tick(TICKS_PER_SECOND), Value::Scalar(4.0)),
        ];
        let mut col = Collector::default();
        let timed = timed_keys(&keys, &tb, "L", "path key", &mut col);
        let got: Vec<(i64, f64)> = timed.iter().map(|(u, k)| (*u, if let Value::Scalar(v) = k.value { v } else { f64::NAN })).collect();
        assert_eq!(got, [(0, 3.0), (8533, 2.0), (25600, 1.0)]);
        // The report lists its categories in a fixed order; one merge report for the pair.
        let report = col.finish();
        let lines: Vec<(LossCategory, &str)> = report.losses.iter().flat_map(|l| l.details.iter().map(move |d| (l.category, d.as_str()))).collect();
        assert_eq!(
            lines,
            [
                (LossCategory::ApproximatedKeyframes, "L: path keys at the same time were merged"),
                (LossCategory::RoundedTimes, "L: path key at 0.3333 s moved to the time base"),
            ]
        );
        // No keys, no report.
        let mut col = Collector::default();
        assert!(timed_keys(&[], &tb, "L", "key", &mut col).is_empty());
        assert!(col.finish().losses.is_empty());
    }

    /// Two keys at different times that round to the same time unit are merged too: the first
    /// one is kept, and both the rounding and the merge are reported.
    #[test]
    fn keys_that_round_to_the_same_time_are_merged() {
        use effectcraft_keyframe::Value;
        use effectcraft_time::Tick;
        let tb = TimeBase { frame_duration: 1024, timescale: 25600 };
        // One tick is far below one unit (1/25600 s): it rounds to unit 0, where the first key is.
        let keys = [Keyframe::new(Tick(0), Value::Scalar(1.0)), Keyframe::new(Tick(1), Value::Scalar(2.0))];
        let mut col = Collector::default();
        let timed = timed_keys(&keys, &tb, "L", "key", &mut col);
        let got: Vec<(i64, f64)> = timed.iter().map(|(u, k)| (*u, if let Value::Scalar(v) = k.value { v } else { f64::NAN })).collect();
        assert_eq!(got, [(0, 1.0)]);
        let report = col.finish();
        let lines: Vec<(LossCategory, &str)> = report.losses.iter().flat_map(|l| l.details.iter().map(move |d| (l.category, d.as_str()))).collect();
        assert_eq!(
            lines,
            [
                (LossCategory::ApproximatedKeyframes, "L: keys at the same time were merged"),
                (LossCategory::RoundedTimes, "L: key at 0.0000 s moved to the time base"),
            ]
        );
    }

    /// A key list is filled and replaces the value.
    #[test]
    fn a_key_list_is_filled_and_replaces_the_value() {
        let mut list = Chunk::list(*b"list", vec![Chunk::data(*b"lhd3", vec![0; 52])]);
        fill_key_list(&mut list, vec![7; 96], 2, 48).unwrap();
        let h = list.child(b"lhd3").and_then(|c| c.bytes()).unwrap();
        assert_eq!((h[11], h[19]), (2, 48));
        assert_eq!(list.child(b"ldat").and_then(|c| c.bytes()).map(<[u8]>::len), Some(96));
        // A second fill replaces the records instead of adding a second `ldat`.
        fill_key_list(&mut list, vec![1; 48], 1, 48).unwrap();
        assert_eq!(list.children().iter().filter(|c| c.is(b"ldat")).count(), 1);

        let mut rec = Chunk::list(*b"tdbs", vec![Chunk::data(*b"tdb4", vec![0; 124]), Chunk::data(*b"cdat", vec![0; 8])]);
        replace_cdat(&mut rec, key_list(48, 0, Vec::new())).unwrap();
        assert!(rec.child(b"cdat").is_none() && rec.child_list(b"list").is_some());
        // A record without a value is an error, not a quiet no-op.
        assert!(replace_cdat(&mut rec, key_list(48, 0, Vec::new())).is_err());
        set_key_list(&mut rec, vec![0; 48], 1, 48).unwrap();
        assert!(set_key_list(&mut Chunk::list(*b"tdbs", Vec::new()), Vec::new(), 0, 48).is_err());
    }

    /// A spatial record holds the point layout.
    #[test]
    fn a_spatial_record_holds_the_point_layout() {
        let k = KeyPlan {
            time: 5,
            value: vec![1.0, 2.0],
            in_interp: 2,
            out_interp: 1,
            in_ease: Vec::new(),
            out_ease: Vec::new(),
            spatial_in: [3.0; 3],
            spatial_out: [4.0; 3],
            spatial_auto: true,
            roving: false,
            label: 1,
        };
        let r = spatial_record(&k, 2, true, 56 + 24 * 2).unwrap();
        assert_eq!(&r[..12], &[0, 0, 0, 5, 2, 1, 0, 0x47, 0, 0, 0, 1]);
        assert_eq!(&r[64..72], &2.0f64.to_be_bytes());
        assert_eq!(&r[96..104], &4.0f64.to_be_bytes());
        // A colour has no tangents and flags 0x0001.
        let c = spatial_record(&k, 2, false, 56 + 24 * 2).unwrap();
        assert_eq!((c[7], c[11]), (0x41, 0));
        assert!(c[72..].iter().all(|b| *b == 0));
        // A record shorter than its layout is an error.
        assert!(spatial_record(&k, 2, true, 64).is_err());
    }
}
