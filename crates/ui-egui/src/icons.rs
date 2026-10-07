//! Vector icons drawn in code on a 16×16 design grid (crisp at any DPI, recolourable). Original
//! drawings following generic motion-graphics conventions; no third-party or Adobe artwork.

use egui::epaint::{PathShape, PathStroke};
use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Icon {
    // tools
    Home,
    Selection,
    Hand,
    Zoom,
    Orbit,
    PanCamera,
    Dolly,
    Rotate,
    PanBehind,
    Rectangle,
    RoundedRect,
    Ellipse,
    Polygon,
    Star,
    Pen,
    PenAdd,
    PenDelete,
    PenConvert,
    MaskFeather,
    Type,
    TypeVertical,
    Brush,
    Clone,
    Eraser,
    RotoBrush,
    RefineEdge,
    /// Puppet Position Pin: a pushpin. The other pin tools add a badge at the lower right.
    Puppet,
    PuppetAdvanced,
    PuppetBend,
    PuppetStarch,
    PuppetOverlap,
    // switches & timeline
    Eye,
    Speaker,
    Solo,
    Lock,
    Shy,
    Collapse,
    Quality,
    Fx,
    FrameBlend,
    MotionBlur,
    Adjustment,
    Cube,
    Draft3D,
    Graph,
    Stopwatch,
    Keyframe,
    PickWhip,
    Flowchart,
    // project items
    Folder,
    Comp,
    Footage,
    Image,
    Audio,
    Solid,
    TextLayer,
    ShapeLayer,
    Null,
    Camera,
    Light,
    // ui
    Search,
    Trash,
    NewFolder,
    NewComp,
    Gear,
    Hamburger,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    Play,
    Pause,
    StepBack,
    StepFwd,
    First,
    Last,
    Loop,
    Snapshot,
    ShowSnapshot,
    Grid,
    MaskVis,
    Checker,
    Region,
    /// Extended Viewer: a small frame with corner ticks reaching outward.
    ExtendedViewer,
    Plus,
    Minus,
    Close,
    /// A generic circular return arrow for restoring default values.
    Reset,
    /// A generic circled information marker.
    Info,
    Link,
    MountainSmall,
    MountainLarge,
    Fullscreen,
    Channel,
    Exposure,
    // links
    Chat,
    Globe,
    Code,
    Sparkle,
}

pub struct Pen16<'a> {
    painter: &'a Painter,
    rect: Rect,
    color: Color32,
    width: f32,
}

impl Pen16<'_> {
    fn s(&self) -> f32 {
        self.rect.width().min(self.rect.height()) / 16.0
    }
    fn p(&self, x: f32, y: f32) -> Pos2 {
        let s = self.s();
        let o = self.rect.center() - vec2(8.0 * s, 8.0 * s);
        pos2(o.x + x * s, o.y + y * s)
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.p(*x, *y)).collect();
        self.painter.add(PathShape::line(v, PathStroke::new(self.width * self.s(), self.color)));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.p(*x, *y)).collect();
        self.painter.add(PathShape::closed_line(v, PathStroke::new(self.width * self.s(), self.color)));
    }
    fn fill(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.p(*x, *y)).collect();
        self.painter.add(PathShape::convex_polygon(v, self.color, Stroke::NONE));
    }
    fn circle(&self, x: f32, y: f32, r: f32) {
        self.painter.circle_stroke(self.p(x, y), r * self.s(), Stroke::new(self.width * self.s(), self.color));
    }
    fn dot(&self, x: f32, y: f32, r: f32) {
        self.painter.circle_filled(self.p(x, y), r * self.s(), self.color);
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let r = Rect::from_min_max(self.p(x0, y0), self.p(x1, y1));
        self.painter.rect_stroke(r, 1.0 * self.s(), Stroke::new(self.width * self.s(), self.color), egui::StrokeKind::Middle);
    }
    fn rrect(&self, x0: f32, y0: f32, x1: f32, y1: f32, rad: f32) {
        let r = Rect::from_min_max(self.p(x0, y0), self.p(x1, y1));
        self.painter.rect_stroke(r, rad * self.s(), Stroke::new(self.width * self.s(), self.color), egui::StrokeKind::Middle);
    }
    fn rect_fill(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let r = Rect::from_min_max(self.p(x0, y0), self.p(x1, y1));
        self.painter.rect_filled(r, 0.8 * self.s(), self.color);
    }
    fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let n = 24;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.line(&pts);
    }
    /// An upright pushpin centred on `cx`: a cap, a body flaring to its base and a needle.
    fn pushpin(&self, cx: f32) {
        self.fill(&[(cx - 3.0, 1.5), (cx + 3.0, 1.5), (cx + 2.0, 3.0), (cx - 2.0, 3.0)]);
        self.closed(&[(cx - 2.0, 3.0), (cx + 2.0, 3.0), (cx + 2.4, 7.0), (cx + 4.0, 8.8), (cx - 4.0, 8.8), (cx - 2.4, 7.0)]);
        self.line(&[(cx, 8.8), (cx, 14.8)]);
    }
    fn ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32) {
        let n = 32;
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                (cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect();
        self.closed(&pts);
    }
    fn star(&self, cx: f32, cy: f32, ro: f32, ri: f32, n: usize) -> Vec<(f32, f32)> {
        (0..n * 2)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
                let r = if i % 2 == 0 { ro } else { ri };
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }
}

/// Paint `icon` centred in `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let pen = Pen16 { painter, rect, color, width: 1.25 };
    use Icon::*;
    match icon {
        Home => {
            pen.closed(&[(2.5, 7.5), (8.0, 2.5), (13.5, 7.5), (13.5, 13.5), (2.5, 13.5)]);
            pen.rect(6.5, 9.5, 9.5, 13.5);
        }
        Selection => pen.fill(&[(4.5, 2.0), (4.5, 13.0), (7.2, 10.5), (9.2, 14.6), (10.8, 13.9), (8.9, 9.9), (12.5, 9.7)]),
        Hand => {
            pen.line(&[(5.0, 9.0), (5.0, 4.0)]);
            pen.line(&[(7.3, 8.0), (7.3, 2.6)]);
            pen.line(&[(9.6, 8.0), (9.6, 3.2)]);
            pen.line(&[(11.8, 8.8), (11.8, 5.0)]);
            pen.line(&[(5.0, 9.0), (3.2, 7.4), (2.4, 8.4), (5.6, 13.4), (11.0, 14.0), (12.4, 11.0), (11.8, 8.8)]);
        }
        Zoom => {
            pen.circle(6.8, 6.8, 4.3);
            pen.line(&[(10.0, 10.0), (14.0, 14.0)]);
        }
        Orbit => {
            pen.ellipse(8.0, 8.0, 6.0, 2.8);
            pen.dot(8.0, 8.0, 1.6);
            pen.fill(&[(12.0, 3.0), (14.5, 5.2), (11.4, 6.0)]);
        }
        PanCamera => {
            pen.line(&[(8.0, 1.8), (8.0, 14.2)]);
            pen.line(&[(1.8, 8.0), (14.2, 8.0)]);
            pen.fill(&[(8.0, 1.0), (6.2, 3.5), (9.8, 3.5)]);
            pen.fill(&[(8.0, 15.0), (6.2, 12.5), (9.8, 12.5)]);
            pen.fill(&[(1.0, 8.0), (3.5, 6.2), (3.5, 9.8)]);
            pen.fill(&[(15.0, 8.0), (12.5, 6.2), (12.5, 9.8)]);
        }
        Dolly => {
            pen.line(&[(8.0, 1.8), (8.0, 14.2)]);
            pen.fill(&[(8.0, 1.0), (5.5, 4.5), (10.5, 4.5)]);
            pen.fill(&[(8.0, 15.0), (5.5, 11.5), (10.5, 11.5)]);
            pen.rect(5.0, 6.5, 11.0, 9.5);
        }
        Rotate => {
            pen.arc(8.0, 8.0, 5.0, -60.0, 220.0);
            pen.fill(&[(10.2, 1.8), (13.6, 4.6), (9.6, 5.8)]);
        }
        PanBehind => {
            pen.circle(8.0, 8.0, 3.0);
            pen.line(&[(8.0, 1.5), (8.0, 5.0)]);
            pen.line(&[(8.0, 11.0), (8.0, 14.5)]);
            pen.line(&[(1.5, 8.0), (5.0, 8.0)]);
            pen.line(&[(11.0, 8.0), (14.5, 8.0)]);
        }
        Rectangle => pen.rect(2.5, 3.5, 13.5, 12.5),
        RoundedRect => pen.rrect(2.5, 3.5, 13.5, 12.5, 3.0),
        Ellipse => pen.ellipse(8.0, 8.0, 5.8, 4.8),
        Polygon => {
            let pts: Vec<(f32, f32)> = (0..6)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 6.0 - std::f32::consts::FRAC_PI_2;
                    (8.0 + 6.0 * a.cos(), 8.4 + 6.0 * a.sin())
                })
                .collect();
            pen.closed(&pts);
        }
        Star => {
            let pts = pen.star(8.0, 8.6, 6.4, 2.7, 5);
            pen.closed(&pts);
        }
        Pen => {
            pen.closed(&[(8.0, 1.8), (12.0, 9.0), (8.0, 14.0), (4.0, 9.0)]);
            pen.dot(8.0, 8.6, 1.1);
            pen.line(&[(8.0, 1.8), (8.0, 7.6)]);
        }
        Type => {
            pen.line(&[(3.0, 3.5), (13.0, 3.5)]);
            pen.line(&[(8.0, 3.5), (8.0, 13.5)]);
            pen.line(&[(6.0, 13.5), (10.0, 13.5)]);
        }
        TypeVertical => {
            pen.line(&[(12.5, 3.0), (12.5, 13.0)]);
            pen.line(&[(12.5, 8.0), (3.0, 8.0)]);
            pen.line(&[(3.0, 6.0), (3.0, 10.0)]);
        }
        Brush => {
            pen.line(&[(13.5, 2.5), (7.0, 9.0)]);
            pen.fill(&[(7.6, 8.4), (4.4, 9.4), (2.6, 13.6), (6.8, 12.0), (7.8, 9.0)]);
        }
        Clone => {
            pen.rect(6.0, 8.0, 10.0, 14.0);
            pen.line(&[(8.0, 8.0), (8.0, 5.5)]);
            pen.ellipse(8.0, 3.8, 3.0, 1.8);
        }
        Eraser => {
            pen.closed(&[(9.0, 2.5), (14.0, 7.5), (8.0, 13.5), (3.0, 8.5)]);
            pen.line(&[(5.5, 6.0), (11.5, 11.0)]);
            pen.line(&[(6.0, 14.0), (14.0, 14.0)]);
        }
        RotoBrush => {
            pen.line(&[(13.0, 2.5), (8.0, 7.5)]);
            pen.ellipse(6.0, 10.0, 3.6, 3.6);
            pen.dot(6.0, 10.0, 1.0);
        }
        RefineEdge => {
            // A brush over a soft (dashed) edge.
            pen.line(&[(13.5, 2.5), (9.0, 7.0)]);
            pen.ellipse(7.0, 9.0, 2.6, 2.6);
            pen.line(&[(2.0, 14.0), (3.5, 12.5)]);
            pen.line(&[(5.0, 14.0), (6.5, 12.5)]);
            pen.line(&[(8.0, 14.0), (9.5, 12.5)]);
            pen.line(&[(11.0, 14.0), (12.5, 12.5)]);
        }
        Puppet => pen.pushpin(8.0),
        PuppetAdvanced => {
            // Rotation ring with its square scale handle.
            pen.pushpin(6.5);
            pen.circle(12.0, 12.0, 2.4);
            pen.rect_fill(13.6, 11.2, 15.2, 12.8);
        }
        PuppetBend => {
            // A curved arrow.
            pen.pushpin(6.5);
            pen.arc(11.8, 13.2, 3.0, 200.0, 340.0);
            pen.fill(&[(14.9, 11.0), (15.4, 13.8), (12.9, 12.6)]);
        }
        PuppetStarch => {
            // Stiff bars.
            pen.pushpin(6.5);
            for y in [10.5, 12.75, 15.0] {
                pen.line(&[(10.5, y), (15.0, y)]);
            }
        }
        PuppetOverlap => {
            // Two overlapping sheets.
            pen.pushpin(6.5);
            pen.rect(10.0, 9.8, 13.4, 13.2);
            pen.rect_fill(11.8, 11.6, 15.2, 15.0);
        }
        Eye => {
            pen.line(&[(1.5, 8.0), (4.0, 5.0), (8.0, 3.8), (12.0, 5.0), (14.5, 8.0), (12.0, 11.0), (8.0, 12.2), (4.0, 11.0), (1.5, 8.0)]);
            pen.dot(8.0, 8.0, 2.2);
        }
        Speaker => {
            pen.fill(&[(2.0, 6.0), (5.0, 6.0), (9.0, 2.5), (9.0, 13.5), (5.0, 10.0), (2.0, 10.0)]);
            pen.arc(9.0, 8.0, 3.0, -50.0, 50.0);
            pen.arc(9.0, 8.0, 5.5, -50.0, 50.0);
        }
        Solo => {
            pen.circle(8.0, 8.0, 4.5);
            pen.dot(8.0, 8.0, 2.4);
        }
        Lock => {
            pen.rect(3.5, 7.0, 12.5, 14.0);
            pen.arc(8.0, 7.0, 3.0, 180.0, 360.0);
            pen.dot(8.0, 10.5, 1.0);
        }
        Shy => {
            pen.circle(8.0, 8.0, 5.8);
            pen.dot(6.0, 7.0, 0.8);
            pen.dot(10.0, 7.0, 0.8);
            pen.arc(8.0, 8.5, 2.6, 30.0, 150.0);
        }
        Collapse => {
            pen.circle(8.0, 8.0, 2.8);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                pen.line(&[(8.0 + 4.3 * a.cos(), 8.0 + 4.3 * a.sin()), (8.0 + 6.3 * a.cos(), 8.0 + 6.3 * a.sin())]);
            }
        }
        Quality => pen.line(&[(4.0, 2.5), (12.0, 13.5)]),
        Fx => {
            pen.line(&[(7.0, 3.0), (5.6, 3.0), (4.6, 4.2), (4.6, 13.0)]);
            pen.line(&[(3.0, 7.0), (7.0, 7.0)]);
            pen.line(&[(8.5, 7.0), (13.0, 13.0)]);
            pen.line(&[(13.0, 7.0), (8.5, 13.0)]);
        }
        FrameBlend => {
            pen.rect(2.0, 4.0, 9.0, 11.0);
            pen.rect(7.0, 5.0, 14.0, 12.0);
        }
        MotionBlur => {
            pen.circle(10.5, 8.0, 3.5);
            pen.circle(7.0, 8.0, 3.5);
            pen.circle(4.0, 8.0, 3.0);
        }
        Adjustment => {
            pen.circle(8.0, 8.0, 5.5);
            pen.fill(&[(8.0, 2.5), (8.0, 13.5), (5.2, 12.7), (3.2, 10.6), (2.5, 8.0), (3.2, 5.4), (5.2, 3.3)]);
        }
        Cube => {
            pen.closed(&[(8.0, 2.0), (13.5, 5.0), (13.5, 11.0), (8.0, 14.0), (2.5, 11.0), (2.5, 5.0)]);
            pen.line(&[(2.5, 5.0), (8.0, 8.0), (13.5, 5.0)]);
            pen.line(&[(8.0, 8.0), (8.0, 14.0)]);
        }
        Draft3D => {
            pen.closed(&[(8.0, 2.0), (13.5, 5.0), (13.5, 11.0), (8.0, 14.0), (2.5, 11.0), (2.5, 5.0)]);
            pen.line(&[(5.0, 9.0), (11.0, 9.0)]);
        }
        Graph => {
            pen.line(&[(2.0, 13.0), (5.0, 12.0), (7.0, 8.0), (9.0, 4.5), (11.5, 3.2), (14.0, 3.0)]);
            pen.line(&[(2.0, 2.0), (2.0, 14.0), (14.0, 14.0)]);
        }
        Stopwatch => {
            pen.circle(8.0, 9.0, 5.2);
            pen.line(&[(8.0, 9.0), (8.0, 5.8)]);
            pen.line(&[(6.5, 2.0), (9.5, 2.0)]);
            pen.line(&[(8.0, 2.0), (8.0, 3.8)]);
        }
        Keyframe => pen.fill(&[(8.0, 2.5), (13.5, 8.0), (8.0, 13.5), (2.5, 8.0)]),
        PickWhip => {
            pen.arc(8.0, 8.0, 5.5, 0.0, 300.0);
            pen.arc(8.0, 8.0, 3.0, 40.0, 340.0);
            pen.dot(8.0, 8.0, 1.0);
        }
        Flowchart => {
            pen.rect(1.5, 2.5, 6.0, 6.5);
            pen.rect(10.0, 9.5, 14.5, 13.5);
            pen.line(&[(6.0, 4.5), (12.25, 4.5), (12.25, 9.5)]);
        }
        Folder => pen.closed(&[(1.5, 4.0), (6.0, 4.0), (7.5, 5.5), (14.5, 5.5), (14.5, 13.0), (1.5, 13.0)]),
        Comp => {
            pen.rect(2.0, 3.0, 14.0, 13.0);
            pen.rect_fill(4.0, 5.0, 8.0, 9.0);
            pen.rect(7.0, 7.5, 12.0, 11.0);
        }
        Footage => {
            pen.rect(2.0, 3.0, 14.0, 13.0);
            for y in [5.0, 8.0, 11.0] {
                pen.dot(3.8, y, 0.6);
                pen.dot(12.2, y, 0.6);
            }
            pen.line(&[(5.5, 3.0), (5.5, 13.0)]);
            pen.line(&[(10.5, 3.0), (10.5, 13.0)]);
        }
        Image => {
            pen.rect(2.0, 3.0, 14.0, 13.0);
            pen.line(&[(2.5, 11.5), (6.0, 8.0), (9.0, 10.5), (11.0, 9.0), (13.5, 11.5)]);
            pen.dot(10.5, 5.8, 1.1);
        }
        Audio => {
            pen.line(&[(6.0, 12.0), (6.0, 3.5), (13.0, 2.5), (13.0, 10.5)]);
            pen.dot(4.4, 12.0, 1.8);
            pen.dot(11.4, 10.5, 1.8);
        }
        Solid => pen.rect_fill(3.0, 3.0, 13.0, 13.0),
        TextLayer => {
            pen.line(&[(3.5, 3.5), (12.5, 3.5)]);
            pen.line(&[(8.0, 3.5), (8.0, 13.0)]);
        }
        ShapeLayer => {
            let pts = pen.star(8.0, 8.6, 6.4, 2.7, 5);
            pen.closed(&pts);
        }
        Null => pen.rect(3.0, 3.0, 13.0, 13.0),
        Camera => {
            pen.rect(1.5, 5.0, 10.5, 12.0);
            pen.closed(&[(10.5, 7.5), (14.5, 5.5), (14.5, 11.5), (10.5, 9.5)]);
        }
        Light => {
            pen.circle(8.0, 6.5, 3.8);
            pen.line(&[(6.3, 10.0), (6.3, 12.5), (9.7, 12.5), (9.7, 10.0)]);
            pen.line(&[(6.8, 14.2), (9.2, 14.2)]);
        }
        Search => {
            pen.circle(6.8, 6.8, 4.0);
            pen.line(&[(9.8, 9.8), (13.5, 13.5)]);
        }
        Trash => {
            pen.line(&[(2.5, 4.0), (13.5, 4.0)]);
            pen.closed(&[(4.0, 4.0), (5.0, 14.0), (11.0, 14.0), (12.0, 4.0)]);
            pen.line(&[(6.0, 4.0), (6.5, 2.0), (9.5, 2.0), (10.0, 4.0)]);
        }
        NewFolder => {
            pen.closed(&[(1.5, 4.0), (6.0, 4.0), (7.5, 5.5), (14.5, 5.5), (14.5, 13.0), (1.5, 13.0)]);
            pen.line(&[(8.0, 7.5), (8.0, 11.5)]);
            pen.line(&[(6.0, 9.5), (10.0, 9.5)]);
        }
        NewComp => {
            pen.rect(2.0, 3.0, 14.0, 13.0);
            pen.line(&[(8.0, 5.5), (8.0, 10.5)]);
            pen.line(&[(5.5, 8.0), (10.5, 8.0)]);
        }
        Gear => {
            pen.circle(8.0, 8.0, 2.2);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                pen.line(&[(8.0 + 4.0 * a.cos(), 8.0 + 4.0 * a.sin()), (8.0 + 6.0 * a.cos(), 8.0 + 6.0 * a.sin())]);
            }
            pen.circle(8.0, 8.0, 4.2);
        }
        Hamburger => {
            for y in [4.5, 8.0, 11.5] {
                pen.line(&[(3.0, y), (13.0, y)]);
            }
        }
        ChevronDown => pen.fill(&[(4.0, 6.0), (12.0, 6.0), (8.0, 11.0)]),
        ChevronRight => pen.fill(&[(6.0, 4.0), (11.0, 8.0), (6.0, 12.0)]),
        ChevronLeft => pen.fill(&[(10.0, 4.0), (5.0, 8.0), (10.0, 12.0)]),
        Play => pen.fill(&[(4.5, 2.5), (13.0, 8.0), (4.5, 13.5)]),
        Pause => {
            pen.rect_fill(4.0, 3.0, 6.8, 13.0);
            pen.rect_fill(9.2, 3.0, 12.0, 13.0);
        }
        StepBack => {
            pen.fill(&[(12.5, 3.5), (6.0, 8.0), (12.5, 12.5)]);
            pen.rect_fill(3.5, 3.5, 5.0, 12.5);
        }
        StepFwd => {
            pen.fill(&[(3.5, 3.5), (10.0, 8.0), (3.5, 12.5)]);
            pen.rect_fill(11.0, 3.5, 12.5, 12.5);
        }
        First => {
            pen.fill(&[(13.5, 3.5), (8.0, 8.0), (13.5, 12.5)]);
            pen.fill(&[(8.5, 3.5), (3.0, 8.0), (8.5, 12.5)]);
            pen.rect_fill(1.8, 3.5, 3.0, 12.5);
        }
        Last => {
            pen.fill(&[(2.5, 3.5), (8.0, 8.0), (2.5, 12.5)]);
            pen.fill(&[(7.5, 3.5), (13.0, 8.0), (7.5, 12.5)]);
            pen.rect_fill(13.0, 3.5, 14.2, 12.5);
        }
        Loop => {
            pen.arc(8.0, 8.0, 5.0, 200.0, 520.0);
            pen.fill(&[(1.8, 5.5), (5.2, 5.5), (3.2, 8.5)]);
        }
        Reset => {
            pen.arc(8.0, 8.0, 5.0, -110.0, 160.0);
            pen.line(&[(3.0, 2.5), (3.0, 6.0), (6.5, 6.0)]);
        }
        Info => {
            pen.circle(8.0, 8.0, 6.0);
            pen.dot(8.0, 4.7, 0.7);
            pen.line(&[(7.0, 7.0), (8.0, 7.0), (8.0, 11.3)]);
            pen.line(&[(6.8, 11.3), (9.2, 11.3)]);
        }
        Snapshot => {
            pen.closed(&[(2.0, 5.0), (5.0, 5.0), (6.0, 3.5), (10.0, 3.5), (11.0, 5.0), (14.0, 5.0), (14.0, 12.5), (2.0, 12.5)]);
            pen.circle(8.0, 8.7, 2.3);
        }
        ShowSnapshot => {
            pen.line(&[(1.5, 8.0), (4.0, 5.0), (8.0, 3.8), (12.0, 5.0), (14.5, 8.0), (12.0, 11.0), (8.0, 12.2), (4.0, 11.0), (1.5, 8.0)]);
            pen.rect(6.0, 6.5, 10.0, 9.5);
        }
        Grid => {
            pen.rect(2.0, 2.0, 14.0, 14.0);
            pen.line(&[(6.0, 2.0), (6.0, 14.0)]);
            pen.line(&[(10.0, 2.0), (10.0, 14.0)]);
            pen.line(&[(2.0, 6.0), (14.0, 6.0)]);
            pen.line(&[(2.0, 10.0), (14.0, 10.0)]);
        }
        MaskVis => {
            pen.rect(2.0, 2.0, 14.0, 14.0);
            pen.ellipse(8.0, 8.0, 3.8, 3.8);
        }
        Checker => {
            pen.rect(2.0, 2.0, 14.0, 14.0);
            pen.rect_fill(2.0, 2.0, 8.0, 8.0);
            pen.rect_fill(8.0, 8.0, 14.0, 14.0);
        }
        Region => {
            pen.rect(2.5, 2.5, 13.5, 13.5);
            pen.rect(5.5, 5.5, 10.5, 10.5);
        }
        ExtendedViewer => {
            pen.rect(5.0, 5.0, 11.0, 11.0);
            for (x, y, dx, dy) in [(2.0, 2.0, 1.0, 1.0), (14.0, 2.0, -1.0, 1.0), (2.0, 14.0, 1.0, -1.0), (14.0, 14.0, -1.0, -1.0)] {
                pen.line(&[(x + dx * 3.0, y), (x, y), (x, y + dy * 3.0)]);
            }
        }
        Plus => {
            pen.line(&[(8.0, 3.0), (8.0, 13.0)]);
            pen.line(&[(3.0, 8.0), (13.0, 8.0)]);
        }
        Minus => pen.line(&[(3.0, 8.0), (13.0, 8.0)]),
        Close => {
            pen.line(&[(4.0, 4.0), (12.0, 12.0)]);
            pen.line(&[(12.0, 4.0), (4.0, 12.0)]);
        }
        Link => {
            pen.arc(5.5, 8.0, 3.0, 90.0, 270.0);
            pen.arc(10.5, 8.0, 3.0, -90.0, 90.0);
            pen.line(&[(5.5, 5.0), (8.0, 5.0)]);
            pen.line(&[(5.5, 11.0), (8.0, 11.0)]);
            pen.line(&[(8.0, 5.0), (10.5, 5.0)]);
            pen.line(&[(8.0, 11.0), (10.5, 11.0)]);
        }
        MountainSmall => pen.fill(&[(4.0, 12.0), (8.0, 7.0), (12.0, 12.0)]),
        MountainLarge => {
            pen.fill(&[(1.0, 13.0), (7.0, 4.0), (13.0, 13.0)]);
            pen.fill(&[(8.0, 13.0), (11.5, 8.0), (15.0, 13.0)]);
        }
        Fullscreen => {
            pen.line(&[(2.0, 6.0), (2.0, 2.0), (6.0, 2.0)]);
            pen.line(&[(10.0, 2.0), (14.0, 2.0), (14.0, 6.0)]);
            pen.line(&[(14.0, 10.0), (14.0, 14.0), (10.0, 14.0)]);
            pen.line(&[(6.0, 14.0), (2.0, 14.0), (2.0, 10.0)]);
        }
        Channel => {
            pen.circle(6.0, 6.5, 3.6);
            pen.circle(10.0, 6.5, 3.6);
            pen.circle(8.0, 10.0, 3.6);
        }
        Exposure => {
            pen.circle(8.0, 8.0, 5.5);
            pen.fill(&[(8.0, 2.5), (13.5, 8.0), (8.0, 13.5)]);
        }
        Chat => {
            pen.closed(&[(2.0, 3.0), (14.0, 3.0), (14.0, 11.0), (7.0, 11.0), (4.0, 14.0), (4.5, 11.0), (2.0, 11.0)]);
            pen.dot(5.3, 7.0, 0.95);
            pen.dot(8.0, 7.0, 0.95);
            pen.dot(10.7, 7.0, 0.95);
        }
        Globe => {
            pen.circle(8.0, 8.0, 6.0);
            pen.ellipse(8.0, 8.0, 2.6, 6.0);
            pen.line(&[(2.0, 8.0), (14.0, 8.0)]);
            pen.line(&[(3.2, 4.8), (12.8, 4.8)]);
            pen.line(&[(3.2, 11.2), (12.8, 11.2)]);
        }
        Code => {
            pen.line(&[(5.0, 4.0), (1.8, 8.0), (5.0, 12.0)]);
            pen.line(&[(11.0, 4.0), (14.2, 8.0), (11.0, 12.0)]);
            pen.line(&[(9.4, 2.8), (6.6, 13.2)]);
        }
        Sparkle => {
            pen.fill(&[(8.0, 1.5), (9.4, 6.6), (14.5, 8.0), (9.4, 9.4), (8.0, 14.5), (6.6, 9.4), (1.5, 8.0), (6.6, 6.6)]);
        }
        PenAdd | PenDelete => {
            // A small pen nib with a plus or minus beside it.
            pen.closed(&[(6.0, 2.5), (9.0, 8.0), (6.0, 12.0), (3.0, 8.0)]);
            pen.line(&[(6.0, 2.5), (6.0, 7.0)]);
            pen.line(&[(10.0, 11.5), (14.5, 11.5)]);
            if icon == PenAdd {
                pen.line(&[(12.25, 9.25), (12.25, 13.75)]);
            }
        }
        PenConvert => {
            // An open caret: a corner vertex with two straight sides.
            pen.line(&[(2.5, 13.0), (8.0, 3.5), (13.5, 13.0)]);
            pen.dot(8.0, 3.5, 1.4);
        }
        MaskFeather => {
            pen.closed(&[(6.0, 2.5), (9.0, 8.0), (6.0, 12.0), (3.0, 8.0)]);
            pen.line(&[(6.0, 2.5), (6.0, 7.0)]);
            pen.line(&[(10.5, 4.0), (12.5, 6.5), (13.5, 10.0), (12.5, 13.5)]);
        }
    }
}

/// Keyframe glyph by interpolation (diamond = linear, hourglass = bezier/ease, circle = auto,
/// square = hold), split into in/out halves.
pub fn keyframe(
    painter: &Painter,
    center: Pos2,
    size: f32,
    left: effectcraft_engine::keyframe::KeyHalf,
    right: effectcraft_engine::keyframe::KeyHalf,
    fill: Color32,
    outline: Color32,
) {
    use effectcraft_engine::keyframe::KeyHalf as H;
    let r = size / 2.0;
    let half = |pts: Vec<Pos2>| {
        painter.add(PathShape::convex_polygon(pts, fill, Stroke::new(1.0, outline)));
    };
    let c = center;
    for (side, h) in [(-1.0f32, left), (1.0, right)] {
        match h {
            H::Linear => half(if side < 0.0 {
                vec![c + vec2(0.0, -r), c, c + vec2(0.0, r), c + vec2(-r, 0.0)]
            } else {
                vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c]
            }),
            H::Hold => half(if side < 0.0 {
                vec![c + vec2(-r * 0.8, -r * 0.8), c + vec2(0.0, -r * 0.8), c + vec2(0.0, r * 0.8), c + vec2(-r * 0.8, r * 0.8)]
            } else {
                vec![c + vec2(0.0, -r * 0.8), c + vec2(r * 0.8, -r * 0.8), c + vec2(r * 0.8, r * 0.8), c + vec2(0.0, r * 0.8)]
            }),
            H::Bezier => half(if side < 0.0 {
                vec![c + vec2(-r * 0.75, -r), c + vec2(0.0, -r * 0.35), c + vec2(0.0, r * 0.35), c + vec2(-r * 0.75, r)]
            } else {
                vec![c + vec2(0.0, -r * 0.35), c + vec2(r * 0.75, -r), c + vec2(r * 0.75, r), c + vec2(0.0, r * 0.35)]
            }),
            H::Auto => {
                let n = 10;
                // Simple half-disc.
                let mut hp = vec![c + vec2(0.0, -r * 0.8), c + vec2(0.0, r * 0.8)];
                for i in 1..n {
                    let t = std::f32::consts::PI * i as f32 / n as f32;
                    hp.push(c + vec2(side * r * 0.8 * t.sin(), r * 0.8 * t.cos()));
                }
                half(hp);
            }
        }
    }
}
