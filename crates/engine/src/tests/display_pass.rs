//! The viewer's display pass (`viewer::DisplayPass`): exposure on the float range of 32 bpc
//! frames, the HDR step after exposure, the display colour after both, and the frame encodings.

use effectcraft_color::{ColorSpace, linear_to_srgb};
use effectcraft_project::HdrMode;
use effectcraft_render::RenderOpts;
use effectcraft_render::color::{compand, tone_map};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::viewer::{Channel, DisplayColor, DisplayPass, FrameEncoding, PassCurve, SimProfile, Simulation};

const SRGB: FrameEncoding = FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::Clip };

fn rgb(encoding: FrameEncoding, stops: f32) -> DisplayPass {
    DisplayPass::new(Channel::Rgb, false, stops, encoding, None)
}

/// An opaque grey of linear light `l`, sRGB-encoded as 32 bpc frames store it.
fn grey(l: f32) -> [f32; 4] {
    let e = PassCurve::Srgb.encode(l);
    [e, e, e, 1.0]
}

fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}

/// The issue's case: over-range values come back with negative exposure (4.0 at -2 EV is white)
/// and 0.5 at +1 EV is white; values of the 8-bit range keep today's bytes.
#[test]
fn exposure_reaches_over_range_values() {
    let table: [(f32, [u8; 5]); 6] = [
        (0.25, [137, 99, 71, 34, 188]),
        (0.5, [188, 137, 99, 49, 255]),
        (1.0, [255, 188, 137, 71, 255]),
        (2.0, [255, 255, 188, 99, 255]),
        (4.0, [255, 255, 255, 137, 255]),
        (16.0, [255, 255, 255, 255, 255]),
    ];
    for (l, want) in table {
        for (stops, w) in [0.0, -1.0, -2.0, -4.0, 1.0].into_iter().zip(want) {
            assert_eq!(rgb(SRGB, stops).pixel(grey(l))[0], w, "linear {l} at {stops} EV");
        }
    }
    assert_eq!(rgb(SRGB, -2.0).pixel(grey(4.0)), [255, 255, 255, 255]);
    assert_eq!(rgb(SRGB, 1.0).pixel(grey(0.5)), [255, 255, 255, 255]);
    // Premultiplied: alpha stays, colour scales with it.
    let e = PassCurve::Srgb.encode(4.0);
    let half = rgb(SRGB, -2.0).pixel([e * 0.5, e * 0.5, e * 0.5, 0.5]);
    assert!(half[3] == 128 && half[..3].iter().all(|c| (127..=128).contains(c)), "{half:?}");
}

/// Compand / Tone Map run after exposure: a value under the knee that exposure pushes over it
/// is companded (not clipped), and highlights darkened by exposure are not companded first.
#[test]
fn the_hdr_step_runs_after_exposure() {
    let compand_enc = FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::Compand };
    // 0.6 is below the 0.8 knee: unchanged at 0 EV, companded to about 0.97 at +1 EV.
    assert_eq!(rgb(compand_enc, 0.0).pixel(grey(0.6))[0], q(linear_to_srgb(0.6)));
    let up = rgb(compand_enc, 1.0).pixel(grey(0.6))[0];
    assert_eq!(up, q(linear_to_srgb(compand(1.2))));
    assert!(up < 255, "companded after exposure, not clipped: {up}");
    // 3.0 at -2 EV is 0.75, under the knee: compand first would have made it 0.25.
    let down = rgb(compand_enc, -2.0).pixel(grey(3.0))[0];
    assert_eq!(down, q(linear_to_srgb(0.75)));
    assert!(down > q(linear_to_srgb(compand(3.0) * 0.25)) + 50, "{down}");
    // Tone Map (luminance): the same order.
    let tm = FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::ToneMap };
    for (l, stops) in [(0.6f32, 1.0f32), (3.0, -2.0), (16.0, -1.0), (0.1, 0.0)] {
        let k = 2f32.powf(stops);
        let want = q(linear_to_srgb(tone_map([l * k; 3])[0]));
        assert_eq!(rgb(tm, stops).pixel(grey(l))[0], want, "{l} at {stops}");
    }
}

/// The display colour converts the exposed output numbers: exposure first, then the monitor
/// conversion (before: the conversion of the 8-bit frame, then exposure).
#[test]
fn display_colour_follows_exposure() {
    let dc =
        DisplayColor::new(Some(ColorSpace::Srgb), false, ColorSpace::Srgb, true, ColorSpace::DisplayP3, Simulation::default(), &Default::default()).unwrap();
    let pass = DisplayPass::new(Channel::Rgb, false, 1.0, SRGB, Some(&dc));
    let red = [PassCurve::Srgb.encode(0.4), 0.0, 0.0, 1.0];
    let exposed = [linear_to_srgb(0.8), 0.0, 0.0];
    let want = dc.apply_straight(exposed).map(q);
    assert_eq!(pass.pixel(red)[..3], want);
    // The conversion is not applied to a single channel's view.
    assert!(DisplayPass::new(Channel::Red, false, 0.0, SRGB, Some(&dc)).color.is_none());
}

/// The pass's display colour is `DisplayColor::apply_straight`: the same conversions in the same
/// order (a few ULPs apart where the pass uses `f32` for PQ / HLG).
#[test]
fn pass_colour_matches_display_color() {
    let custom = Default::default();
    let mut cases = vec![];
    for display in [ColorSpace::DisplayP3, ColorSpace::Rec709, ColorSpace::Rec2020, ColorSpace::Srgb] {
        for profile in SimProfile::ALL {
            for preserve_rgb in [false, true] {
                let sim = Simulation { profile, preserve_rgb };
                cases.extend(DisplayColor::new(Some(ColorSpace::Srgb), false, ColorSpace::Srgb, true, display, sim, &custom));
            }
        }
        cases.extend(DisplayColor::new(Some(ColorSpace::Rec2020), true, ColorSpace::Srgb, false, display, Simulation::default(), &custom));
    }
    // A LUT-based profile (an uneven, channel-mixing grid).
    cases.push(DisplayColor::from_lut(std::sync::Arc::new(test_lut())));
    assert!(cases.len() > 40);
    for dc in &cases {
        let pass = dc.pass();
        for i in 0..=20 {
            for c in [[i as f32 / 20.0, 0.3, 0.9], [0.05, i as f32 / 20.0, 0.5], [1.0, 1.0, i as f32 / 20.0]] {
                let (a, b) = (pass.apply(c), dc.apply_straight(c));
                for k in 0..3 {
                    // The simulation's 8-bit step can land either side of a tie.
                    assert!((a[k] - b[k]).abs() < 1e-4 || ((a[k] - b[k]).abs() < 1.5 / 255.0 && c != [1.0; 3]), "{dc:?} {c:?}: {a:?} vs {b:?}");
                }
            }
        }
    }
}

/// A 17³ LUT that mixes channels and bends the curve (as an ICC profile's device does).
pub(crate) fn test_lut() -> effectcraft_color::icc::Lut3d {
    effectcraft_color::icc::Lut3d::bake(17, |c| [c[0].powf(1.3) * 0.9 + c[1] * 0.1, c[1] * 0.8 + c[2] * 0.2, (c[2] * 0.7 + c[0] * 0.3).sqrt()])
}

/// Frames keep their encoding: the output space's curve (the Working Gamma's when unmanaged) and
/// the HDR step only when it was skipped and applies (managed, SDR output).
#[test]
fn frame_encodings() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 8, "height": 8, "duration": 1})).unwrap();
    assert_eq!(FrameEncoding::of(&s.project.settings, true), SRGB, "unmanaged: sRGB at Working Gamma 2.2, no HDR step");
    s.execute("file.projectSettings", json!({"bitDepth": 32, "workingSpace": "srgb", "hdr": "compand"})).unwrap();
    let settings = s.project.settings.clone();
    assert_eq!(FrameEncoding::of(&settings, true), FrameEncoding { curve: PassCurve::Srgb, hdr: HdrMode::Compand });
    assert_eq!(FrameEncoding::of(&settings, false), SRGB, "rendered with the step: nothing pending");
    s.execute("file.projectSettings", json!({"outputSpace": "rec2100pq"})).unwrap();
    assert_eq!(FrameEncoding::of(&s.project.settings, true), FrameEncoding { curve: PassCurve::Pq, hdr: HdrMode::Clip }, "HDR outputs never tone map");
    s.execute("file.projectSettings", json!({"outputSpace": "rec709"})).unwrap();
    assert_eq!(FrameEncoding::of(&s.project.settings, true).curve, PassCurve::Gamma(2.4));
}

/// A 32 bpc Compand project with an over-range layer: `FrameEncoding::finish` on the frame
/// rendered without the HDR step gives the frame rendered with it, and Take Snapshot (now taken
/// without the step) exports the same bytes as before, within a rounding step.
#[test]
fn the_skipped_hdr_step_finishes_like_the_render() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 16, "height": 8, "duration": 1})).unwrap();
    s.execute("file.projectSettings", json!({"bitDepth": 32, "workingSpace": "srgb", "hdr": "compand"})).unwrap();
    let lid = s.execute("layer.newSolid", json!({"color": "#c08040", "width": 16, "height": 8})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [lid], "effect": "Exposure"})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "effects/#1/master/exposure", "value": 1.5})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let with = s.render(cid, Tick::ZERO, RenderOpts::default());
    let without = s.render(cid, Tick::ZERO, RenderOpts { skip_hdr_step: true, ..Default::default() });
    let enc = FrameEncoding::of(&s.project.settings, true);
    assert_eq!(enc.hdr, HdrMode::Compand);
    let (w, o) = (with.data[0], without.data[0]);
    assert!(o[0] > 1.0 && w[0] < 1.0, "over range without the step, companded with it: {o:?} {w:?}");
    let f = enc.finish([o[0], o[1], o[2]]);
    for k in 0..3 {
        assert!((f[k] - w[k]).abs() < 1e-4, "{f:?} vs {w:?}");
    }
    s.execute("view.takeSnapshot", json!({})).unwrap();
    assert_eq!(s.snapshot.as_ref().unwrap().encoding, enc);
    let (_, _, rgba) = crate::commands::viewer_cmds::snapshot_rgba8(&s).unwrap();
    let before = w.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    for k in 0..4 {
        assert!((rgba[k] as i32 - before[k] as i32).abs() <= 1, "{:?} vs {before:?}", &rgba[..4]);
    }
    // 8 bpc snapshots are rendered with the step, as before.
    s.execute("file.projectSettings", json!({"bitDepth": 8})).unwrap();
    s.execute("view.takeSnapshot", json!({})).unwrap();
    assert_eq!(s.snapshot.as_ref().unwrap().encoding.hdr, HdrMode::Clip);
}

/// The 8-bit CPU path's tables give what the pass gives pixel by pixel: every value of every
/// channel view, opaque and not, with and without exposure.
#[test]
fn the_8_bit_tables_are_the_pass() {
    for (channel, colorized) in
        [(Channel::Rgb, false), (Channel::RgbStraight, false), (Channel::Red, false), (Channel::Green, true), (Channel::Blue, false), (Channel::Alpha, false)]
    {
        for stops in [0.0f32, 1.0, -2.5] {
            let pass = DisplayPass::new(channel, colorized, stops, SRGB, None);
            let px: Vec<[u8; 4]> = (0..256 * 4)
                .map(|i| {
                    let v = (i % 256) as u8;
                    let a = [255u8, 255, 128, 3][i / 256];
                    let c = |k: u8| ((v as u32 * k as u32) % 256 * a as u32 / 255) as u8;
                    [c(1), c(7), c(13), a]
                })
                .collect();
            let mut got = px.clone();
            pass.apply_rgba8(&mut got);
            for (p, g) in px.iter().zip(&got) {
                assert_eq!(*g, pass.pixel(p.map(|v| v as f32 / 255.0)), "{channel:?} {colorized} {stops}: {p:?}");
            }
        }
    }
}
