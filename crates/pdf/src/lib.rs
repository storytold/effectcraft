//! Vector footage: PDF, PDF-compatible Illustrator files (`.ai`) and Encapsulated PostScript
//! (see README.md for what is read and the specifications used).
//!
//! Files parse into the same render tree as SVG ([`effectcraft_svg::Doc`]): document pixels are
//! PostScript points (1/72 in), y down, so footage rasterises at any scale (Continuously
//! Rasterize) and converts to shape layers (Create Shapes from Vector Layer). The root group's
//! children are the document's **layers**: one group per top-level optional-content group
//! (Illustrator layers), with content outside them gathered into groups of its own.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod build;
mod ccitt;
mod cff;
mod color;
mod encoding;
mod font;
mod image;
mod object;
mod page;
mod ps;
mod read;
mod shading;
mod type1;
pub mod write;

pub use effectcraft_svg::Doc;
use effectcraft_svg::{Affine, Group, Node};

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    NotVector,
    Encrypted,
    NoPages,
    PageOutOfRange(usize),
    NoBoundingBox,
    Read(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotVector => write!(f, "not a PDF, Illustrator or EPS file"),
            Error::Encrypted => write!(f, "encrypted PDF files are not supported"),
            Error::NoPages => write!(f, "the PDF has no pages"),
            Error::PageOutOfRange(n) => write!(f, "the PDF has no page {}", n.saturating_add(1)),
            Error::NoBoundingBox => write!(f, "the EPS file has no %%BoundingBox"),
            Error::Read(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// File extensions read as vector footage by this crate.
pub const EXTENSIONS: &[&str] = &["pdf", "ai", "eps", "epsf", "epsi"];

/// The format of a file, by its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Eps,
}

/// Sniff a PDF (`%PDF-` near the start, as PDF-compatible `.ai` files are) or an EPS file.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(Format::Pdf);
    }
    if bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) || bytes.starts_with(b"%!PS") {
        return Some(Format::Eps);
    }
    None
}

/// A footage codec name for a sniffed file ("PDF", "AI", "EPS").
pub fn codec(path: &str, bytes: &[u8]) -> Option<&'static str> {
    let f = sniff(bytes)?;
    let ai = path.to_ascii_lowercase().ends_with(".ai");
    Some(match (f, ai) {
        (_, true) => "AI",
        (Format::Pdf, false) => "PDF",
        (Format::Eps, false) => "EPS",
    })
}

/// Parse the first page of a PDF / `.ai`, or an EPS file.
pub fn parse(bytes: &[u8]) -> Result<Doc, Error> {
    parse_page(bytes, 0)
}

/// Parse page `index` of a PDF (EPS files have one page).
pub fn parse_page(bytes: &[u8], index: usize) -> Result<Doc, Error> {
    match sniff(bytes) {
        Some(Format::Pdf) => parse_pdf(bytes, index),
        Some(Format::Eps) => parse_eps(bytes),
        None => Err(Error::NotVector),
    }
}

/// Number of pages (1 for EPS).
pub fn page_count(bytes: &[u8]) -> usize {
    match sniff(bytes) {
        Some(Format::Pdf) => object::File::parse(bytes).map(|file| page::pages(&file).len()).unwrap_or(0),
        Some(Format::Eps) => 1,
        None => 0,
    }
}

/// Build the document: `root` children become layer groups (see the crate docs).
fn finish(nodes: Vec<Node>, flags: Vec<bool>, skipped: Vec<String>, width: f64, height: f64, to_doc: Affine) -> Doc {
    let mut layers: Vec<Node> = vec![];
    let mut loose: Vec<Node> = vec![];
    let flush = |loose: &mut Vec<Node>, layers: &mut Vec<Node>| {
        if !loose.is_empty() {
            let mut g = build::group("");
            g.children = std::mem::take(loose);
            layers.push(Node::Group(g));
        }
    };
    for (n, is_layer) in nodes.into_iter().zip(flags.into_iter().chain(std::iter::repeat(false))) {
        if is_layer {
            flush(&mut loose, &mut layers);
            layers.push(n);
        } else {
            loose.push(n);
        }
    }
    flush(&mut loose, &mut layers);
    // Name unnamed layers "Layer N" (bottom first, as Illustrator numbers them).
    for (i, l) in layers.iter_mut().enumerate() {
        if let Node::Group(g) = l
            && g.name.is_empty()
        {
            g.name = format!("Layer {}", i + 1);
        }
    }
    let mut root = Group::new("page");
    root.transform = to_doc;
    root.children = layers;
    Doc { width, height, root, skipped }
}

fn parse_pdf(bytes: &[u8], index: usize) -> Result<Doc, Error> {
    let file = object::File::parse(bytes)?;
    let pages = page::pages(&file);
    if pages.is_empty() {
        return Err(Error::NoPages);
    }
    let pg = pages.get(index).ok_or(Error::PageOutOfRange(index))?;
    let [x0, y0, x1, y1] = pg.bbox;
    let (w, h) = (x1 - x0, y1 - y0);
    // Default user space (y up) → document pixels (y down), with the page's /Rotate.
    let (to_doc, dw, dh) = match pg.rotate {
        90 => (Affine::new([0.0, 1.0, 1.0, 0.0, -y0, -x0]), h, w),
        180 => (Affine::new([-1.0, 0.0, 0.0, 1.0, x1, -y0]), w, h),
        270 => (Affine::new([0.0, -1.0, -1.0, 0.0, y1, x1]), h, w),
        _ => (Affine::new([1.0, 0.0, 0.0, -1.0, -x0, y1]), w, h),
    };
    let content = page::page_content(&file, pg);
    let mut it = page::Interp::new(&file, pg.bbox);
    it.b.skipped.extend(file.repairs.iter().cloned());
    it.run(&content, &pg.resources, Affine::IDENTITY, 0);
    let (nodes, flags, skipped) = it.b.finish();
    Ok(finish(nodes, flags, skipped, dw.max(1.0), dh.max(1.0), to_doc))
}

fn parse_eps(bytes: &[u8]) -> Result<Doc, Error> {
    let (ps, bb) = ps::eps_parts(bytes).ok_or(Error::NoBoundingBox)?;
    let mut it = ps::Interp::new(bb);
    it.run(ps);
    let (nodes, flags, skipped) = it.b.finish();
    let to_doc = Affine::new([1.0, 0.0, 0.0, -1.0, -bb[0], bb[3]]);
    Ok(finish(nodes, flags, skipped, (bb[2] - bb[0]).max(1.0), (bb[3] - bb[1]).max(1.0), to_doc))
}

/// The document's layer names (the root's children), bottom first.
pub fn layer_names(doc: &Doc) -> Vec<String> {
    doc.root
        .children
        .iter()
        .map(|n| match n {
            Node::Group(g) => g.name.clone(),
            Node::Shape(s) => s.name.clone(),
            Node::Image(i) => i.name.clone(),
        })
        .collect()
}

/// The document with only layer `index` (for one layer of a file imported as a composition).
pub fn layer_doc(doc: &Doc, index: usize) -> Doc {
    let mut d = doc.clone();
    d.root.children = doc.root.children.get(index).cloned().into_iter().collect();
    d
}

/// One layer, optionally cropped to its artwork, and its original document-space top-left.
pub fn layer_doc_with_sizes(doc: &Doc, index: usize, retain: bool) -> (Doc, [f64; 2]) {
    let mut d = layer_doc(doc, index);
    if !retain {
        return (d, [0.0, 0.0]);
    }
    let page = kurbo::Rect::new(0.0, 0.0, doc.width, doc.height);
    let bounds = d.artwork_bounds().map(|b| b.intersect(page)).filter(|b| b.width() > 0.0 && b.height() > 0.0);
    let (x, y, w, h) = match bounds {
        Some(b) => {
            let (x, y) = (b.x0.floor(), b.y0.floor());
            (x, y, b.x1.ceil() - x, b.y1.ceil() - y)
        }
        None => (0.0, 0.0, 1.0, 1.0),
    };
    d.width = w;
    d.height = h;
    d.root.transform = Affine::translate((-x, -y)) * d.root.transform;
    (d, [x, y])
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_groups;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_reader;
#[cfg(test)]
mod tests_shading;
