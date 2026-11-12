//! Step 1 of the camera analysis: long feature tracks through the clip.
//!
//! Every frame, the active features are followed from the previous frame with pyramidal
//! Lucas–Kanade and a forward–backward check ([`crate::klt::track`]); a feature that fails ends
//! its track. When fewer than ~85 % of the wanted features remain, new Shi–Tomasi corners are
//! detected away from the active ones and start new tracks. Tracks shorter than three frames are
//! dropped at the end.

use crate::Frame;
use crate::klt::{CornerOpts, GrayPyramid, LkOpts, analysis_factor, good_features, track};

use super::{CameraTracks, Track2D};

/// Analysis options.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyzeOpts {
    /// Detailed Analysis: twice the resolution and more features.
    pub detailed: bool,
}

/// Incremental feature tracking: push the clip's frames in order.
pub struct TrackAnalyzer {
    opts: AnalyzeOpts,
    prev: Option<GrayPyramid>,
    /// (track index, level-0 plane position) of the live features.
    active: Vec<(usize, [f64; 2])>,
    tracks: Vec<Track2D>,
    frame: u32,
    size: [f64; 2],
    factor: f64,
    /// (analysis long side, wanted features) overriding the options' defaults.
    budget: Option<(u32, usize)>,
}

impl TrackAnalyzer {
    pub fn new(size: [f64; 2], opts: AnalyzeOpts) -> TrackAnalyzer {
        TrackAnalyzer { opts, prev: None, active: vec![], tracks: vec![], frame: 0, size, factor: 1.0, budget: None }
    }

    /// An analyzer working at most at `max_side` pixels with about `want` live features (the
    /// Warp Stabilizer's trajectories for Subspace Warp).
    pub fn with_budget(size: [f64; 2], max_side: u32, want: usize) -> TrackAnalyzer {
        TrackAnalyzer { budget: Some((max_side, want.max(8))), ..TrackAnalyzer::new(size, AnalyzeOpts::default()) }
    }

    pub fn frames(&self) -> u32 {
        self.frame
    }

    /// Features currently followed.
    pub fn active(&self) -> usize {
        self.active.len()
    }

    fn params(&self) -> (u32, usize) {
        if let Some(b) = self.budget {
            return b;
        }
        if self.opts.detailed { (1280, 900) } else { (720, 500) }
    }

    /// Analyse the next frame.
    pub fn push(&mut self, frame: &Frame) {
        let (max_side, _) = self.params();
        let factor = analysis_factor(frame.img.width, frame.img.height, max_side);
        let pyr = GrayPyramid::from_image(frame.img, frame.offset, factor, 4);
        let prev = self.prev.take();
        self.push_pyramid(prev.as_ref(), &pyr);
        self.prev = Some(pyr);
    }

    /// Track using caller-owned consecutive pyramids. The caller retains the full pyramid
    /// for the next frame; standalone camera analysis uses the same path through `push`.
    pub(crate) fn push_pyramid(&mut self, prev: Option<&GrayPyramid>, pyr: &GrayPyramid) {
        let (_, want) = self.params();
        self.factor = pyr.factor;
        let k = self.frame;
        match prev {
            Some(prev) if prev.width() == pyr.width() && prev.height() == pyr.height() && prev.offset == pyr.offset => {
                let pts: Vec<[f64; 2]> = self.active.iter().map(|a| a.1).collect();
                let q = track(prev, pyr, &pts, None, &LkOpts { radius: 6, iterations: 30, epsilon: 0.005, fb_max: 0.5, ..Default::default() });
                let mut next = Vec::with_capacity(self.active.len());
                for ((ti, _), q) in self.active.iter().zip(q) {
                    if let Some(q) = q {
                        let l = pyr.to_layer(q);
                        self.tracks[*ti].pts.push([l[0] as f32, l[1] as f32]);
                        next.push((*ti, q));
                    }
                }
                self.active = next;
            }
            _ => self.active.clear(),
        }
        // Re-detect where there are no features.
        if (self.active.len() as f64) < want as f64 * 0.85 {
            let md = (pyr.width().max(pyr.height()) as f64 / (want as f64).sqrt() * 0.6).clamp(5.0, 40.0);
            let cell = md;
            let (gw, gh) = ((pyr.width() as f64 / cell).ceil() as usize + 1, (pyr.height() as f64 / cell).ceil() as usize + 1);
            let mut occ = vec![false; gw * gh];
            for (_, p) in &self.active {
                let (cx, cy) = ((p[0] / cell) as usize, (p[1] / cell) as usize);
                for yy in cy.saturating_sub(1)..=(cy + 1).min(gh - 1) {
                    for xx in cx.saturating_sub(1)..=(cx + 1).min(gw - 1) {
                        occ[yy * gw + xx] = true;
                    }
                }
            }
            let free = |p: [f64; 2]| {
                let (cx, cy) = ((p[0] / cell) as usize, (p[1] / cell) as usize);
                !occ.get(cy * gw + cx).copied().unwrap_or(true)
            };
            let opts = CornerOpts { max_features: want - self.active.len().min(want), min_distance: md, quality: 0.001, window: 2, border: 8 };
            for p in good_features(pyr, &opts, Some(&free)) {
                let l = pyr.to_layer(p);
                let id = self.tracks.len() as u32;
                self.tracks.push(Track2D { id, start: k, pts: vec![[l[0] as f32, l[1] as f32]] });
                self.active.push((id as usize, p));
            }
        }
        self.frame += 1;
    }

    pub fn finish(self, start: f64, frame_duration: f64) -> CameraTracks {
        let mut tracks: Vec<Track2D> = self.tracks.into_iter().filter(|t| t.pts.len() >= 3).collect();
        // Round to 1/100 pixel (smaller JSON) and renumber densely.
        for (i, t) in tracks.iter_mut().enumerate() {
            t.id = i as u32;
            for p in t.pts.iter_mut() {
                *p = [(p[0] * 100.0).round() / 100.0, (p[1] * 100.0).round() / 100.0];
            }
        }
        CameraTracks { version: 1, size: self.size, start, frame_duration, frames: self.frame, factor: self.factor, detailed: self.opts.detailed, tracks }
    }
}
