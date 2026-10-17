//! `audio.analyze`: levels, loudness, clipping and silences of the comp's audio mix (MCP
//! `analyze_audio`). Headless agents can't hear; the measurements are [`crate::audio_analysis`].

use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use super::layout_cmds::{r2, r4};
use super::{CommandSpec, b_p, bad, comp_id, f_p, resolve_layer, str_p};
use crate::audio_analysis::{self, Analyzer, db};
use crate::{EngineError, Result, Session, query};

/// Longest span analysed in one call (seconds).
const MAX_AUDIO_SPAN: f64 = 3600.0;
/// Most silent ranges listed.
const MAX_SILENCES: usize = 500;

fn audio_analyze(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "audio.analyze";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let start = f_p(p, "start").unwrap_or(0.0);
    let end = f_p(p, "end").unwrap_or(comp.duration.seconds());
    if !start.is_finite() || !end.is_finite() {
        return Err(bad(C, "start and end are seconds"));
    }
    if end <= start {
        return Err(bad(C, format!("empty range: end ({end} s) must be after start ({start} s)")));
    }
    if end - start > MAX_AUDIO_SPAN {
        return Err(bad(C, format!("analyse at most {MAX_AUDIO_SPAN} s at a time (asked for {} s)", end - start)));
    }
    let silence_db = f_p(p, "silenceDb").unwrap_or(-60.0);
    if !(-200.0..=0.0).contains(&silence_db) {
        return Err(bad(C, "silenceDb is a level in dBFS (≤ 0, default -60)"));
    }
    let min_silence = f_p(p, "minSilence").unwrap_or(0.25);
    if !(0.0..=MAX_AUDIO_SPAN).contains(&min_silence) {
        return Err(bad(C, "minSilence is seconds (≥ 0, default 0.25)"));
    }
    let waveform = b_p(p, "waveform").unwrap_or(false) || str_p(p, "path").is_some();
    let width = f_p(p, "width").unwrap_or(800.0);
    let height = f_p(p, "height").unwrap_or(160.0);
    if waveform && (!(16.0..=4096.0).contains(&width) || !(16.0..=2048.0).contains(&height)) {
        return Err(bad(C, "waveform width is 16-4096 and height 16-2048 pixels"));
    }
    let layer = match p.get("layer").filter(|v| !v.is_null()) {
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad(C, format!("no layer {v} (use a layer id, \"#n\" or a name from comp.info)")))?),
        None => None,
    };
    let mut base = json!({"comp": cid.0, "layer": layer.map(|l| l.0), "start": start, "end": end, "duration": end - start});
    // Mix one layer: solo it in a copy of the project.
    let solo;
    let project = match layer {
        Some(lid) => {
            let l = comp.layer(lid).ok_or_else(|| bad(C, "no such layer"))?;
            if !effectcraft_render::audio::audible(&s.project, l) {
                base["hasAudio"] = json!(false);
                base["why"] = json!(if l.switches.audio { "this layer has no audio" } else { "this layer's Audio switch is off" });
                return Ok(base);
            }
            let mut p2 = (*s.project).clone();
            let c2 = p2.comp_mut(cid).ok_or(EngineError::NoComp)?;
            for l in &mut c2.layers {
                l.switches.solo = l.id == lid;
            }
            solo = p2;
            &solo
        }
        None => {
            if !effectcraft_render::audio::comp_has_audio(&s.project, cid) {
                base["hasAudio"] = json!(false);
                base["why"] = json!("the comp has no audible layers (footage with audio, precomps with audio, or a Tone effect)");
                return Ok(base);
            }
            &*s.project
        }
    };
    let rate = audio_analysis::RATE;
    let frames = ((end - start) * rate as f64).round() as u64;
    if frames == 0 {
        return Err(bad(C, "the range is shorter than one sample"));
    }
    let mut an = Analyzer::new(audio_analysis::Settings {
        frames,
        silence_db,
        min_silence,
        waveform: if waveform { width as usize } else { 0 },
        true_peak: b_p(p, "truePeak").unwrap_or(true),
    });
    let t0 = Tick::from_seconds_f64(start);
    let per = TICKS_PER_SECOND / rate as i64;
    let mut done = 0u64;
    while done < frames {
        let n = (frames - done).min(rate as u64) as usize;
        let t = t0 + Tick((done as i64).saturating_mul(per));
        let buf = effectcraft_render::audio::mix_comp(project, s.footage.as_ref(), s.expr.as_deref(), cid, t, n, rate);
        an.push(&buf);
        done += n as u64;
    }
    let rep = an.finish();
    let dbr = |a: f64| db(a).map(r2);
    let channels: Vec<Value> = ["L", "R"]
        .iter()
        .zip(&rep.channels)
        .map(|(name, c)| {
            json!({"channel": name, "peakDbfs": dbr(c.peak), "rmsDbfs": dbr(c.rms), "truePeakDbtp": rep.true_peak.and(dbr(c.true_peak)), "clippedSamples": c.clipped})
        })
        .collect();
    let silences: Vec<Value> =
        rep.silences.iter().take(MAX_SILENCES).map(|(a, b)| json!({"start": r4(start + a), "end": r4(start + b), "duration": r4(b - a)})).collect();
    let mut v = base;
    v["hasAudio"] = json!(true);
    v["sampleRate"] = json!(rate);
    v["silent"] = json!(rep.peak == 0.0);
    v["peakDbfs"] = json!(dbr(rep.peak));
    v["truePeakDbtp"] = json!(rep.true_peak.and_then(dbr));
    v["rmsDbfs"] = json!(dbr(rep.rms));
    v["integratedLufs"] = json!(rep.integrated.map(r2));
    v["momentaryMaxLufs"] = json!(rep.momentary_max.map(r2));
    v["shortTermMaxLufs"] = json!(rep.short_term_max.map(r2));
    v["clippedSamples"] = json!(rep.channels.iter().map(|c| c.clipped).sum::<u64>());
    v["channels"] = json!(channels);
    v["silenceThresholdDb"] = json!(silence_db);
    if rep.silences.len() > MAX_SILENCES {
        v["silencesTruncated"] = json!(rep.silences.len());
    }
    v["silences"] = json!(silences);
    if waveform {
        let cols: Vec<Value> = rep.columns.iter().map(|c| json!(c.map(|x| r4(x as f64)))).collect();
        v["waveform"] = json!({"width": rep.columns.len(), "height": height as u32, "columns": cols});
        if let Some(path) = str_p(p, "path") {
            let (w, h, rgba) = audio_analysis::waveform_rgba(&rep.columns, height as u32);
            let png = encode_png(w, h, &rgba).map_err(|e| bad(C, e))?;
            s.services.write_file(path, &png).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
            v["path"] = json!(path);
        }
    }
    Ok(v)
}

/// Opaque RGBA8 → RGB PNG bytes.
fn encode_png(w: u32, h: u32, rgba: &[u8]) -> std::result::Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).map_err(|e| format!("png: {e}"))?;
    Ok(out)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![query!(
        "audio.analyze",
        "Analyze Audio",
        "{comp?, start? (s), end? (s), layer?: id|name|#n (that layer alone), silenceDb? (default -60), minSilence? (s, default 0.25), truePeak? (default true), waveform?, width?, height?, path? (waveform PNG)} → peakDbfs, truePeakDbtp, rmsDbfs, integratedLufs (BS.1770), clippedSamples, silences, channels",
        audio_analyze
    )]
}
