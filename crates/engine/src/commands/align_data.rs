//! Layer ▸ Time ▸ Align Video to Data: move a footage layer in time so its frames line up with
//! the samples of a data file (telemetry, GPS, sensor logs imported as data footage).
//!
//! The data's sample with timestamp `T` sits at comp time `dataStart + (T − T₀)` (`T₀` = the
//! first sample's timestamp; `dataStart` defaults to 0). A video frame at source time `s` was
//! recorded at `V₀ + s`, where `V₀` is the video's start: `videoStart` if given, else the
//! footage file's creation date (desktop). Aligning sets the layer's Start Time to
//! `dataStart + V₀ − T₀`, so the frame recorded at `T` plays when the data says `T`.
//!
//! **Supported data** (File ▸ Import as data footage; also in `docs/architecture.md`):
//! - JSON: an array of objects, or an object holding such an array (the first array of objects
//!   found at the top level, e.g. `{"samples": [...]}`);
//! - CSV / TSV with a header row (quoted fields allowed).
//!
//! The time key is `key`, else the first field named `time`, `timestamp`, `t`, `date`,
//! `datetime`, `utc`, `gps_time`, `seconds` or `time_s` (any case). Timestamps may be ISO 8601
//! date-times (`2026-05-01T10:15:30.5Z`, offsets `±hh:mm`), times of day (`10:15:30.5`),
//! timecode (`10:15:30:12` at the footage's rate), Unix seconds, or Unix milliseconds (numbers
//! above 10¹¹). When the data has only times of day, `V₀` is reduced to its UTC time of day.

use effectcraft_project::{FootageKind, ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, bad, f_p, has_layers, layer_p, str_p};
use crate::{EngineError, Result, Session, cmd};

/// A parsed timestamp: seconds, and whether it carries a date (absolute) or is a time of day /
/// relative number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub secs: f64,
    pub dated: bool,
}

/// Days since 1970-01-01 of the proleptic-Gregorian date `(y, m, d)`. `d` is added linearly (as
/// the previous hand-rolled `days_from_civil` did), so a day past the month's end rolls over;
/// `None` when the year/month is out of chrono's range. `m` must be 1..=12.
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    let year = i32::try_from(y).ok()?;
    let month = u32::try_from(m).ok()?;
    let first = chrono::NaiveDate::from_ymd_opt(year, month, 1)?;
    // Days since the Unix epoch: `num_days_from_ce` counts from 0001-01-01; 719_162 days precede 1970-01-01.
    use chrono::Datelike;
    i64::from(first.num_days_from_ce()).checked_sub(719_163)?.checked_add(d.checked_sub(1)?)
}

/// `hh:mm:ss(.fff)` or timecode `hh:mm:ss:ff` / `hh:mm:ss;ff` → seconds.
fn clock(s: &str, fps: f64) -> Option<f64> {
    let parts: Vec<&str> = s.split([':', ';']).collect();
    match parts.as_slice() {
        [h, m, sec] => Some(h.trim().parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?),
        [h, m, sec, f] => {
            Some(h.trim().parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()? + f.parse::<f64>().ok()? / fps.max(1.0))
        }
        [m, sec] => Some(m.trim().parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?),
        _ => None,
    }
}

/// Parse a timestamp value (see the module docs). `fps` reads timecode frames.
pub fn parse_stamp(v: &Value, fps: f64) -> Option<Stamp> {
    match v {
        Value::Number(n) => {
            let x = n.as_f64()?;
            Some(if x > 1e11 { Stamp { secs: x / 1000.0, dated: true } } else { Stamp { secs: x, dated: x > 1e8 } })
        }
        Value::String(s) => {
            let s = s.trim();
            if let Ok(x) = s.parse::<f64>() {
                return parse_stamp(&json!(x), fps);
            }
            // ISO 8601: date [T| ] time [zone].
            if s.len() >= 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
                let (y, m, d) = (s[0..4].parse::<i64>().ok()?, s[5..7].parse::<i64>().ok()?, s[8..10].parse::<i64>().ok()?);
                let mut secs = days_from_civil(y, m, d)? as f64 * 86_400.0;
                let rest = s[10..].trim_start_matches(['T', 't', ' ']);
                if !rest.is_empty() {
                    // Zone: Z or ±hh:mm / ±hhmm.
                    let (time, zone) = match rest.find(['Z', 'z', '+']).or_else(|| rest.rfind('-')) {
                        Some(i) => (&rest[..i], &rest[i..]),
                        None => (rest, ""),
                    };
                    secs += clock(time, fps)?;
                    if zone.len() > 1 {
                        let sign = if zone.starts_with('-') { -1.0 } else { 1.0 };
                        let z = zone[1..].replace(':', "");
                        let (zh, zm) = (z.get(0..2)?.parse::<f64>().ok()?, z.get(2..4).and_then(|m| m.parse::<f64>().ok()).unwrap_or(0.0));
                        secs -= sign * (zh * 3600.0 + zm * 60.0);
                    }
                }
                return Some(Stamp { secs, dated: true });
            }
            clock(s, fps).map(|secs| Stamp { secs, dated: false })
        }
        _ => None,
    }
}

const TIME_KEYS: &[&str] = &["time", "timestamp", "t", "date", "datetime", "utc", "gps_time", "gpstime", "seconds", "time_s", "times"];

/// Split a CSV/TSV line (quotes allowed).
fn split_row(line: &str, sep: char) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => q = !q,
            c if c == sep && !q => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

/// The time column of data footage: (key used, timestamps in row order).
pub fn data_times(text: &str, format: &str, key: Option<&str>) -> std::result::Result<(String, Vec<Value>), String> {
    let pick = |names: &[String]| -> Option<String> {
        if let Some(k) = key {
            return names.iter().find(|n| n.eq_ignore_ascii_case(k)).cloned();
        }
        TIME_KEYS.iter().find_map(|t| names.iter().find(|n| n.eq_ignore_ascii_case(t)).cloned())
    };
    if format == "json" {
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("invalid JSON: {e}"))?;
        let rows = match &v {
            Value::Array(a) => a.clone(),
            Value::Object(o) => {
                o.values().find_map(|x| x.as_array().filter(|a| a.first().is_some_and(Value::is_object)).cloned()).ok_or("no array of samples in the JSON")?
            }
            _ => return Err("the JSON holds no samples".into()),
        };
        let names: Vec<String> = rows.first().and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let k = pick(&names)
            .ok_or_else(|| format!("no time field (looked for {}); pass `key`", key.map(|k| k.to_string()).unwrap_or_else(|| TIME_KEYS.join(", "))))?;
        let times = rows.iter().filter_map(|r| r.get(&k).cloned()).collect();
        return Ok((k, times));
    }
    let sep = if format == "tsv" { '\t' } else { ',' };
    let mut lines = text.trim_start_matches('\u{feff}').lines().filter(|l| !l.trim().is_empty());
    let header = split_row(lines.next().ok_or("the file is empty")?, sep);
    let k = pick(&header)
        .ok_or_else(|| format!("no time column (looked for {}); pass `key`", key.map(|k| k.to_string()).unwrap_or_else(|| TIME_KEYS.join(", "))))?;
    let col = header.iter().position(|h| *h == k).unwrap_or(0);
    let times = lines.filter_map(|l| split_row(l, sep).get(col).map(|c| json!(c))).collect();
    Ok((k, times))
}

fn data_item(s: &Session, p: &Value) -> Result<ItemId> {
    let c = "layer.alignVideoToData";
    let id = match p.get("data") {
        Some(Value::Number(n)) => n.as_u64().map(ItemId),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id),
        _ => s.project.items.values().find(|i| matches!(&i.kind, ItemKind::Footage(f) if f.kind == FootageKind::Data)).map(|i| i.id),
    }
    .ok_or_else(|| bad(c, "no data footage: import a JSON/CSV/TSV file and pass `data`"))?;
    Ok(id)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.alignVideoToData";
    let (cid, lid) = layer_p(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let footage = match layer.source.item().and_then(|i| s.project.item(i)).map(|i| &i.kind) {
        Some(ItemKind::Footage(f)) if f.kind != FootageKind::Data => f.clone(),
        _ => return Err(bad(c, "select a footage layer")),
    };
    let fps = footage.frame_rate.as_f64();
    let did = data_item(s, p)?;
    let Some(ItemKind::Footage(df)) = s.project.item(did).map(|i| &i.kind) else { return Err(bad(c, "not data footage")) };
    let text = df.data.clone().ok_or_else(|| bad(c, "the data item holds no data"))?;
    let format = df.codec.to_ascii_lowercase();
    let (key, raw) = data_times(&text, &format, str_p(p, "key").filter(|k| !k.is_empty())).map_err(|e| bad(c, e))?;
    let stamps: Vec<Stamp> = raw.iter().filter_map(|v| parse_stamp(v, fps)).collect();
    let first = *stamps.first().ok_or_else(|| bad(c, format!("no readable timestamps in `{key}`")))?;
    let video = match p.get("videoStart").filter(|v| v.as_str() != Some("")) {
        Some(v) => parse_stamp(v, fps).ok_or_else(|| bad(c, "videoStart: ISO 8601 date-time, hh:mm:ss, timecode or seconds"))?,
        None => {
            let (created, _, _) = if footage.missing { (None, None, None) } else { crate::media_browser::file_dates(&footage.path) };
            Stamp { secs: created.ok_or_else(|| bad(c, "the footage has no recording time; pass `videoStart`"))? as f64, dated: true }
        }
    };
    // Compare like with like: times of day when the data has no dates.
    let v0 = if video.dated && !first.dated { video.secs.rem_euclid(86_400.0) } else { video.secs };
    let data_start = f_p(p, "dataStart").unwrap_or(0.0);
    let fr = comp.frame_rate;
    let start = fr.snap_nearest(Tick::from_seconds_f64(data_start + v0 - first.secs));
    let shift = start - layer.start_time;
    s.edit("Align Video to Data", None, |proj, _| {
        let l = super::layer_mut(proj, cid, lid)?;
        l.start_time += shift;
        l.in_point += shift;
        l.out_point += shift;
        Ok(())
    })?;
    Ok(json!({
        "layer": lid.0,
        "startTime": start.seconds(),
        "shift": shift.seconds(),
        "key": key,
        "samples": stamps.len(),
        "firstSample": first.secs,
        "videoStart": v0,
        "dataStart": data_start,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "layer.alignVideoToData",
        "Align Video to Data",
        ["Layer", "Time"],
        None,
        "{layer?, data?: data footage id|name, key?: time field, videoStart?: ISO date-time | hh:mm:ss | timecode | seconds (default: file creation time), dataStart? (comp s of the first sample, 0)}",
        has_layers,
        run
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamps() {
        let p = |v: Value| parse_stamp(&v, 25.0).unwrap();
        assert_eq!(p(json!("1970-01-02T00:00:10Z")).secs, 86_410.0);
        assert_eq!(p(json!("2000-03-01T00:00:00Z")).secs, 951_868_800.0);
        assert_eq!(p(json!("1970-01-01T02:00:00+02:00")).secs, 0.0);
        assert_eq!(p(json!("1970-01-01 01:00:00.5-01:00")).secs, 7200.5);
        assert_eq!(p(json!("10:00:01.25")), Stamp { secs: 36_001.25, dated: false });
        assert_eq!(p(json!("10:00:01:05")).secs, 36_001.2);
        assert_eq!(p(json!(1_700_000_000_500u64)), Stamp { secs: 1_700_000_000.5, dated: true });
        assert_eq!(p(json!(12.5)), Stamp { secs: 12.5, dated: false });
        assert_eq!(p(json!("3.5")).secs, 3.5);
        assert!(parse_stamp(&json!("soon"), 25.0).is_none());
    }

    #[test]
    fn finds_time_columns() {
        let (k, t) = data_times("lat,Time,speed\n1,\"10:00:00\",3\n2,10:00:01,4\n", "csv", None).unwrap();
        assert_eq!(k, "Time");
        assert_eq!(t, vec![json!("10:00:00"), json!("10:00:01")]);
        let (k, t) = data_times("a\tts\n1\t5\n", "tsv", Some("ts")).unwrap();
        assert_eq!((k.as_str(), t), ("ts", vec![json!("5")]));
        let (k, t) = data_times(r#"{"meta": 1, "samples": [{"timestamp": 3, "v": 1}, {"timestamp": 4}]}"#, "json", None).unwrap();
        assert_eq!((k.as_str(), t), ("timestamp", vec![json!(3), json!(4)]));
        assert!(data_times("x,y\n1,2\n", "csv", None).is_err());
    }
}
