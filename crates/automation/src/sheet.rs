//! Contact sheets: frames of one comp at several times, tiled row by row into one image in the
//! order given, each cell labelled with its comp time (`render_frame` / `render_preview` with
//! `times`). The labels use the effects' stroke font, so no font file is involved.

use effectcraft_engine::effects::glyph_strokes;
use image::{Rgba, RgbaImage};

use crate::{Error, Result};

/// Most frames one sheet holds.
pub const MAX_CELLS: usize = 64;
/// Longest side of a sheet in pixels: under 8000, the most some MCP clients accept on a side.
/// It also bounds the sheet's memory (at most 225 MiB of RGBA).
pub const MAX_SIDE: u32 = 7680;
/// Default longest side of each cell. MCP clients scale big images down before a model sees
/// them, so larger cells mostly add bytes: 64 cells in the default grid are then at most 2560
/// pixels on a side.
pub const CELL_SIDE: u32 = 320;

/// Where the cells of a sheet go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub width: u32,
    pub height: u32,
}

impl Layout {
    /// `count` cells of `cell` (width, height) pixels in `columns` columns (at most `count`), or
    /// near-square without. Refuses more than [`MAX_CELLS`] cells, empty cells, and a sheet over
    /// [`MAX_SIDE`] on a side.
    pub fn new(count: usize, columns: Option<u64>, cell: (u32, u32)) -> Result<Layout> {
        let n = u32::try_from(count)
            .ok()
            .filter(|n| (1..=MAX_CELLS).contains(&(*n as usize)))
            .ok_or_else(|| Error::BadArgs(format!("a contact sheet holds 1 to {MAX_CELLS} frames, not {count}")))?;
        let (cell_width, cell_height) = cell;
        if cell_width == 0 || cell_height == 0 {
            return Err(Error::Other(format!("the comp rendered an empty {cell_width} × {cell_height} frame")));
        }
        let columns = match columns {
            Some(c) => u32::try_from(c.clamp(1, u64::from(n))).unwrap_or(n),
            None => near_square(n),
        };
        let rows = n.div_ceil(columns);
        let width = columns.checked_mul(cell_width).filter(|w| *w <= MAX_SIDE);
        let height = rows.checked_mul(cell_height).filter(|h| *h <= MAX_SIDE);
        let (Some(width), Some(height)) = (width, height) else {
            return Err(Error::BadArgs(format!(
                "a {columns} × {rows} contact sheet of {cell_width} × {cell_height} cells is over {MAX_SIDE} pixels on a side: lower `max_side` or change `columns`"
            )));
        };
        Ok(Layout { columns, rows, cell_width, cell_height, width, height })
    }

    /// Top-left corner of cell `index` (row by row).
    fn origin(&self, index: u32) -> Option<(u32, u32)> {
        let (column, row) = (index.checked_rem(self.columns)?, index.checked_div(self.columns)?);
        Some((column.checked_mul(self.cell_width)?, row.checked_mul(self.cell_height)?))
    }
}

/// The fewest columns that make a grid of `n` cells no taller than it is wide.
fn near_square(n: u32) -> u32 {
    (1..=n).find(|c| c.saturating_mul(*c) >= n).unwrap_or(n).max(1)
}

/// A sheet being filled in.
pub struct Sheet {
    layout: Layout,
    image: RgbaImage,
    labels: bool,
}

impl Sheet {
    /// An empty sheet. Cells left without a frame are transparent with `transparent`, else black.
    pub fn new(layout: Layout, transparent: bool, labels: bool) -> Sheet {
        let fill = Rgba(if transparent { [0, 0, 0, 0] } else { [0, 0, 0, 255] });
        Sheet { layout, image: RgbaImage::from_pixel(layout.width, layout.height, fill), labels }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Put `frame`, rendered at comp time `time` (seconds), in cell `index`: its top-left corner,
    /// cropped to the cell, labelled with the time unless labels are off.
    pub fn place(&mut self, index: usize, frame: &RgbaImage, time: f64) {
        let Some((x, y)) = u32::try_from(index).ok().and_then(|i| self.layout.origin(i)) else { return };
        let (w, h) = (self.layout.cell_width, self.layout.cell_height);
        let cell = image::imageops::crop_imm(frame, 0, 0, w, h);
        image::imageops::replace(&mut self.image, &*cell, i64::from(x), i64::from(y));
        if self.labels {
            draw_label(&mut self.image, (x, y, w, h), &label(time));
        }
    }

    pub fn into_image(self) -> RgbaImage {
        self.image
    }
}

/// A cell's label: its time in seconds with at most three decimals (`0.25`, `1`, `-0.5`).
pub fn label(time: f64) -> String {
    let fixed = format!("{time:.3}");
    let short = fixed.trim_end_matches('0').trim_end_matches('.');
    if short == "-0" { "0".to_string() } else { short.to_string() }
}

/// Glyph grid units (the glyphs are 4 wide and 6 tall) per character and around the text.
const ADVANCE: f64 = 6.0;
const PAD: f64 = 1.5;
/// Opacity of the box behind the text.
const BOX_ALPHA: f32 = 0.6;

/// Draw `text` in light strokes on a dark box in the top-left corner of the cell `(x, y, w, h)`.
/// The text is 8% of the cell's height (7 to 48 px), narrower to fit the cell's width; a cell too
/// small to read it in is left as it is.
fn draw_label(img: &mut RgbaImage, (x, y, w, h): (u32, u32, u32, u32), text: &str) {
    let n = text.chars().count() as f64;
    if n == 0.0 {
        return;
    }
    let (box_units_w, box_units_h) = (n * ADVANCE - 2.0 + 2.0 * PAD, 6.0 + 2.0 * PAD);
    let unit = ((f64::from(h) * 0.08).clamp(7.0, 48.0) / 6.0).min(f64::from(w) / box_units_w);
    if unit * 6.0 < 5.0 || unit * box_units_h > f64::from(h) * 0.5 {
        return;
    }
    let radius = (unit * 0.5).max(0.6);
    let segments = text_segments(text, [f64::from(x) + PAD * unit, f64::from(y) + PAD * unit], unit);
    let box_w = (box_units_w * unit).ceil().clamp(0.0, f64::from(w)) as u32;
    let box_h = (box_units_h * unit).ceil().clamp(0.0, f64::from(h)) as u32;
    for py in y..y.saturating_add(box_h) {
        for px in x..x.saturating_add(box_w) {
            let Some(pixel) = img.get_pixel_mut_checked(px, py) else { continue };
            over(pixel, [0.0; 3], BOX_ALPHA);
            let centre = [f64::from(px) + 0.5, f64::from(py) + 0.5];
            let d = segments.iter().map(|s| distance(centre, *s)).fold(f64::INFINITY, f64::min);
            let ink = (radius + 0.5 - d).clamp(0.0, 1.0) as f32;
            if ink > 0.0 {
                over(pixel, [1.0; 3], ink);
            }
        }
    }
}

/// The strokes of `text` as segments in pixels, glyph grid origin of the first character at
/// `origin`, `unit` pixels per grid unit. A single-point stroke (a dot) is a zero-length segment.
fn text_segments(text: &str, origin: [f64; 2], unit: f64) -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let left = origin[0] + i as f64 * ADVANCE * unit;
        for stroke in glyph_strokes(c) {
            let points: Vec<[f64; 2]> = stroke.iter().map(|p| [left + p[0] * unit, origin[1] + p[1] * unit]).collect();
            match points.as_slice() {
                [only] => out.push((*only, *only)),
                _ => out.extend(points.iter().zip(points.iter().skip(1)).map(|(a, b)| (*a, *b))),
            }
        }
    }
    out
}

/// Distance from `p` to the segment `(a, b)`.
fn distance(p: [f64; 2], (a, b): ([f64; 2], [f64; 2])) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a[0] + t * dx - p[0]).hypot(a[1] + t * dy - p[1])
}

/// Straight-alpha "over": `rgb` (0-1) at opacity `alpha` onto `pixel`.
fn over(pixel: &mut Rgba<u8>, rgb: [f32; 3], alpha: f32) {
    let [r, g, b, a] = pixel.0;
    let below = f32::from(a) / 255.0;
    let out = alpha + below * (1.0 - alpha);
    if out <= 0.0 {
        return;
    }
    let mix = |src: f32, dst: u8| to_u8((src * alpha + f32::from(dst) / 255.0 * below * (1.0 - alpha)) / out);
    pixel.0 = [mix(rgb[0], r), mix(rgb[1], g), mix(rgb[2], b), to_u8(out)];
}

fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_near_square_unless_columns_are_given() {
        let grid = |n, columns| {
            let l = Layout::new(n, columns, (10, 5)).unwrap();
            (l.columns, l.rows, l.width, l.height)
        };
        assert_eq!(grid(1, None), (1, 1, 10, 5));
        assert_eq!(grid(2, None), (2, 1, 20, 5));
        assert_eq!(grid(5, None), (3, 2, 30, 10));
        assert_eq!(grid(6, None), (3, 2, 30, 10));
        assert_eq!(grid(10, None), (4, 3, 40, 15));
        assert_eq!(grid(MAX_CELLS, None), (8, 8, 80, 40));
        assert_eq!(grid(6, Some(6)), (6, 1, 60, 5));
        assert_eq!(grid(5, Some(1)), (1, 5, 10, 25));
        assert_eq!(grid(4, Some(99)), (4, 1, 40, 5), "never more columns than frames");
        assert_eq!(grid(4, Some(u64::MAX)), (4, 1, 40, 5));
    }

    #[test]
    fn layout_refuses_what_a_sheet_cannot_hold() {
        assert!(Layout::new(0, None, (10, 10)).is_err());
        assert!(Layout::new(MAX_CELLS + 1, None, (10, 10)).is_err());
        assert!(Layout::new(MAX_CELLS, None, (10, 10)).is_ok());
        assert!(Layout::new(1, None, (MAX_SIDE, MAX_SIDE)).is_ok());
        assert!(Layout::new(1, None, (8000, 1)).is_err(), "some MCP clients refuse an image 8000 pixels wide");
        assert!(Layout::new(1, None, (1, 8000)).is_err(), "or tall");
        assert!(Layout::new(2, Some(2), (MAX_SIDE, 1)).is_err(), "too wide");
        assert!(Layout::new(2, Some(1), (1, MAX_SIDE)).is_err(), "too tall");
        assert!(Layout::new(2, Some(2), (u32::MAX, 1)).is_err(), "no overflow");
        assert!(Layout::new(1, None, (0, 4)).is_err(), "empty frame");
    }

    #[test]
    fn labels_read_seconds() {
        for (t, want) in
            [(0.0, "0"), (1.0, "1"), (12.0, "12"), (10.1, "10.1"), (0.25, "0.25"), (1.5, "1.5"), (-0.5, "-0.5"), (1.0 / 3.0, "0.333"), (-0.0001, "0")]
        {
            assert_eq!(label(t), want, "{t}");
        }
    }

    #[test]
    fn a_label_marks_only_its_cells_corner() {
        let layout = Layout::new(2, None, (64, 36)).unwrap();
        let grey = Rgba([128, 128, 128, 255]);
        let frame = RgbaImage::from_pixel(64, 36, grey);
        let (mut plain, mut labelled) = (Sheet::new(layout, false, false), Sheet::new(layout, false, true));
        for (i, t) in [0.0, 1.25].into_iter().enumerate() {
            plain.place(i, &frame, t);
            labelled.place(i, &frame, t);
        }
        assert!(plain.into_image().pixels().all(|p| *p == grey));
        let labelled = labelled.into_image();
        for x0 in [0, 64] {
            let corner: Vec<u8> = (0..12).flat_map(|y| (x0..x0 + 20).map(move |x| (x, y))).map(|(x, y)| labelled.get_pixel(x, y).0[0]).collect();
            assert!(corner.iter().any(|v| *v < 100), "a dark box behind the time");
            assert!(corner.iter().any(|v| *v > 200), "the time in light strokes");
            assert_eq!(labelled.get_pixel(x0 + 40, 30), &grey, "the rest of the cell is the frame");
        }
    }

    #[test]
    fn small_cells_keep_their_frame_and_big_frames_stay_in_their_cell() {
        let grey = Rgba([128, 128, 128, 255]);
        let mut tiny = Sheet::new(Layout::new(1, None, (4, 2)).unwrap(), false, true);
        tiny.place(0, &RgbaImage::from_pixel(4, 2, grey), 0.5);
        assert!(tiny.into_image().pixels().all(|p| *p == grey), "too small to read a label in");

        let mut sheet = Sheet::new(Layout::new(2, None, (8, 8)).unwrap(), true, false);
        sheet.place(0, &RgbaImage::from_pixel(20, 20, grey), 0.0);
        sheet.place(5, &RgbaImage::from_pixel(8, 8, grey), 0.0);
        let sheet = sheet.into_image();
        assert_eq!(sheet.get_pixel(7, 7), &grey);
        assert_eq!(sheet.get_pixel(8, 0).0, [0, 0, 0, 0], "cropped to its cell; an empty cell is transparent");
    }
}
