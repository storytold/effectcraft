//! Vector conversions behind Layer ▸ Create: Create Shapes from Vector Layer (SVG, PDF,
//! Illustrator and EPS footage → shape layer), Create Shapes from Text and Create Masks from
//! Text (glyph outlines), and vector files imported as compositions (one layer per file layer).

use effectcraft_keyframe::{Gradient, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, Layer, LayerSource, MaskMode, Project, PropGroup};
use effectcraft_svg::{Affine, Doc, Geom, GradientKind, Node, Paint};
use kurbo::{BezPath, Point};

fn shape_paths(ids: &mut Ids, path: &BezPath) -> Vec<PropGroup> {
    effectcraft_path::from_kurbo(path)
        .into_iter()
        .enumerate()
        .map(|(i, sp)| {
            let mut g = build::shape_path(ids, sp);
            g.name = format!("Path {}", i + 1);
            g
        })
        .collect()
}

fn det_scale(m: Affine) -> f64 {
    let c = m.as_coeffs();
    (c[0] * c[3] - c[1] * c[2]).abs().sqrt()
}

/// Axis-aligned scale + translation only (basic shapes stay parametric).
fn axis_aligned(m: Affine) -> Option<(f64, f64, f64, f64)> {
    let c = m.as_coeffs();
    (c[1].abs() < 1e-9 && c[2].abs() < 1e-9 && c[0] > 0.0 && c[3] > 0.0).then_some((c[0], c[3], c[4], c[5]))
}

fn gradient_value(g: &effectcraft_svg::Gradient) -> Gradient {
    Gradient {
        colors: g.stops.iter().map(|(o, c)| (*o, [c[0] as f32, c[1] as f32, c[2] as f32, 1.0])).collect(),
        opacities: g.stops.iter().map(|(o, c)| (*o, c[3] as f32)).collect(),
    }
}

/// Gradient start/end points in shape-layer space (`m`: shape user space → layer space).
fn gradient_points(g: &effectcraft_svg::Gradient, m: Affine, bbox: kurbo::Rect) -> (bool, [f64; 2], [f64; 2]) {
    let unit = if g.bbox_units { Affine::new([bbox.width(), 0.0, 0.0, bbox.height(), bbox.x0, bbox.y0]) } else { Affine::IDENTITY };
    let full = m * unit * g.transform;
    let p = |x: f64, y: f64| {
        let q = full * Point::new(x, y);
        [q.x, q.y]
    };
    match g.kind {
        GradientKind::Linear { x1, y1, x2, y2 } => (false, p(x1, y1), p(x2, y2)),
        GradientKind::Radial { cx, cy, r, .. } => (true, p(cx, cy), p(cx + r, cy)),
    }
}

fn shape_items(ids: &mut Ids, s: &effectcraft_svg::Shape, m: Affine) -> Vec<PropGroup> {
    use kurbo::Shape as _;
    let mut items = vec![];
    // Geometry: parametric when the transform keeps rectangles/ellipses axis-aligned.
    match (&s.geom, axis_aligned(m)) {
        (Geom::Rect { x, y, w, h, rx, ry }, Some((sx, sy, tx, ty))) if (rx - ry).abs() < 1e-9 && (*rx == 0.0 || (sx - sy).abs() < 1e-9) => {
            let (w2, h2) = (w * sx, h * sy);
            let pos = [tx + (x + w / 2.0) * sx, ty + (y + h / 2.0) * sy];
            let mut g = build::shape_rect(ids, [w2, h2], pos, rx * sx);
            g.name = "Rectangle Path 1".into();
            items.push(g);
        }
        (Geom::Ellipse { cx, cy, rx, ry }, Some((sx, sy, tx, ty))) => {
            items.push(build::shape_ellipse(ids, [2.0 * rx * sx, 2.0 * ry * sy], [tx + cx * sx, ty + cy * sy]));
        }
        (g, _) => items.extend(shape_paths(ids, &(m * g.to_path()))),
    }
    let bbox = s.geom.to_path().bounding_box();
    if let Some(st) = &s.stroke {
        let width = st.width * det_scale(m);
        let mut g = match &st.paint {
            Paint::Color(c) => build::shape_stroke(ids, [c[0], c[1], c[2], 1.0], width),
            Paint::Gradient(gr) => {
                let (radial, a, b) = gradient_points(gr, m, bbox);
                build::shape_gradient_stroke(ids, radial, a, b, gradient_value(gr), width)
            }
        };
        set(&mut g, "opacity", Value::Scalar(st.opacity * 100.0));
        set(&mut g, "cap", Value::Enum(st.cap as u32));
        set(&mut g, "join", Value::Enum(st.join as u32));
        set(&mut g, "miter", Value::Scalar(st.miter));
        if let Some((d, off)) = &st.dash
            && let Some(dg) = g.sub_mut("dashes")
        {
            let k = det_scale(m);
            set(dg, "dash", Value::Scalar(d.first().copied().unwrap_or(0.0) * k));
            set(dg, "gap", Value::Scalar(d.get(1).or(d.first()).copied().unwrap_or(0.0) * k));
            set(dg, "offset", Value::Scalar(off * k));
        }
        items.push(g);
    }
    if let Some(f) = &s.fill {
        let mut g = match f {
            Paint::Color(c) => build::shape_fill(ids, [c[0], c[1], c[2], 1.0]),
            Paint::Gradient(gr) => {
                let (radial, a, b) = gradient_points(gr, m, bbox);
                build::shape_gradient_fill(ids, radial, a, b, gradient_value(gr))
            }
        };
        set(&mut g, "opacity", Value::Scalar(s.fill_opacity * 100.0));
        set(&mut g, "rule", Value::Enum(u32::from(s.fill_rule == effectcraft_svg::FillRule::EvenOdd)));
        items.push(g);
    }
    items
}

fn set(g: &mut PropGroup, k: &str, v: Value) {
    if let Some(p) = g.get_mut(k) {
        p.value = v;
    }
}

fn group(ids: &mut Ids, name: &str, items: Vec<PropGroup>, opacity: f64) -> PropGroup {
    let mut g = build::shape_group(ids, name, items);
    if let Some(p) = g.prop_mut("transform/opacity") {
        p.value = Value::Scalar(opacity * 100.0);
    }
    g
}

/// A Merge Paths item in `mode` (0 Merge, 3 Intersect).
fn merge_item(ids: &mut Ids, mode: u32) -> Option<PropGroup> {
    let mut g = build::shape_simple_op(ids, "merge")?;
    set(&mut g, "mode", Value::Enum(mode));
    Some(g)
}

/// A shape inside clipping groups: its geometry merged into one compound path (an inner group
/// with Merge Paths), the clip paths, and Merge Paths ▸ Intersect before its paint, so the fill
/// is exactly the clipped area. Open stroked paths stay unclipped (an intersection would close
/// them).
fn clipped_items(ids: &mut Ids, s: &effectcraft_svg::Shape, m: Affine, clips: &[BezPath]) -> Vec<PropGroup> {
    let items = shape_items(ids, s, m);
    let path = s.geom.to_path();
    let closed = path.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)) || s.fill.is_some();
    if clips.is_empty() || !closed {
        return items;
    }
    let (geom, paint): (Vec<PropGroup>, Vec<PropGroup>) =
        items.into_iter().partition(|g| !matches!(g.match_id.as_str(), "fill" | "stroke" | "gfill" | "gstroke"));
    let mut inner = geom;
    inner.extend(merge_item(ids, 0));
    let mut out = vec![build::shape_group(ids, "Clipped Path", inner)];
    for (i, c) in clips.iter().enumerate() {
        let mut parts = shape_paths(ids, c);
        parts.extend(merge_item(ids, 0));
        out.push(build::shape_group(ids, &format!("Clip Path {}", i + 1), parts));
    }
    out.extend(merge_item(ids, 3));
    out.extend(paint);
    out
}

/// Shape layer contents for an SVG document, in document pixels (transforms are baked into the
/// geometry; group and element opacity go to the group transforms, blend modes to the groups'
/// Blend Mode). Top of the paint order first.
///
/// Clipping groups (PDF / Illustrator clipping masks): shapes inside them are intersected with
/// the clip paths through Merge Paths ▸ Intersect (see `clipped_items`). Clips enclosing the
/// whole document are layer masks instead ([`vector_clip_masks`], used by
/// [`shapes_from_vector`]). Images are skipped (Layer ▸ Create makes them footage layers, see
/// [`split_at_images`]); soft masks are not converted.
pub fn svg_contents(ids: &mut Ids, doc: &Doc) -> Vec<PropGroup> {
    fn walk(ids: &mut Ids, nodes: &[Node], m: Affine, clips: &[BezPath]) -> Vec<PropGroup> {
        let mut out = vec![];
        for n in nodes.iter().rev() {
            match n {
                Node::Group(g) => {
                    let gm = m * g.transform;
                    let mut inner_clips = clips.to_vec();
                    inner_clips.extend(g.clip.iter().map(|(p, _)| gm * p.clone()));
                    let items = walk(ids, &g.children, gm, &inner_clips);
                    if !items.is_empty() {
                        let mut grp = group(ids, &g.name, items, g.opacity);
                        if g.blend != effectcraft_svg::BlendMode::Normal {
                            set(&mut grp, "blend", Value::Enum(blend_index(g.blend)));
                        }
                        out.push(grp);
                    }
                }
                Node::Shape(s) => {
                    let sm = m * s.transform;
                    let items = if clips.is_empty() { shape_items(ids, s, sm) } else { clipped_items(ids, s, sm, clips) };
                    if !items.is_empty() {
                        out.push(group(ids, &s.name, items, s.opacity));
                    }
                }
                Node::Image(_) => {}
            }
        }
        out
    }
    let root = &doc.root;
    let mut clips = vec![];
    clips.extend(root.clip.iter().map(|(p, _)| root.transform * p.clone()));
    let items = walk(ids, &root.children, root.transform, &clips);
    if (root.opacity - 1.0).abs() > 1e-9 { vec![group(ids, "svg", items, root.opacity)] } else { items }
}

/// The shape-layer Blend Mode index of an SVG / PDF blend mode.
fn blend_index(b: effectcraft_svg::BlendMode) -> u32 {
    use effectcraft_color::BlendMode as P;
    use effectcraft_svg::BlendMode as S;
    let p = match b {
        S::Normal => P::Normal,
        S::Multiply => P::Multiply,
        S::Screen => P::Screen,
        S::Overlay => P::Overlay,
        S::Darken => P::Darken,
        S::Lighten => P::Lighten,
        S::ColorDodge => P::ColorDodge,
        S::ColorBurn => P::ColorBurn,
        S::HardLight => P::HardLight,
        S::SoftLight => P::SoftLight,
        S::Difference => P::Difference,
        S::Exclusion => P::Exclusion,
        S::Hue => P::Hue,
        S::Saturation => P::Saturation,
        S::Color => P::Color,
        S::Luminosity => P::Luminosity,
    };
    P::ALL.iter().position(|x| *x == p).unwrap_or(0) as u32
}

/// Clips that enclose the whole document (on a chain of single-child groups from the root, as
/// an Illustrator artboard clip or a PDF page clip), in document pixels, and the document with
/// those clips removed: Create Shapes from Vector Layer makes them layer masks.
pub fn vector_clip_masks(doc: &Doc) -> (Vec<BezPath>, Doc) {
    let mut out = doc.clone();
    let mut masks = vec![];
    let mut m = Affine::IDENTITY;
    let mut g = &mut out.root;
    loop {
        m *= g.transform;
        for (p, _) in g.clip.drain(..) {
            masks.push(m * p);
        }
        if g.children.len() != 1 {
            break;
        }
        match &mut g.children[0] {
            Node::Group(sub) if sub.mask.is_none() && sub.blend == effectcraft_svg::BlendMode::Normal => g = sub,
            _ => break,
        }
    }
    (masks, out)
}

/// Whether text (the system clipboard's, say) is SVG markup: Edit ▸ Paste makes it a shape
/// layer (`edit.pasteSvg`).
pub fn is_svg_text(text: &str) -> bool {
    effectcraft_svg::looks_like_svg(text.as_bytes())
}

/// Whether footage is a vector file (SVG / PDF / AI / EPS).
pub fn is_vector(f: &effectcraft_project::Footage) -> bool {
    effectcraft_render::is_vector_footage(f) || f.path.to_ascii_lowercase().ends_with(".svg")
}

/// The vector document of an SVG / PDF / AI / EPS file (`None` for other formats): page
/// `page` of a PDF, restricted to one layer when `layer` names it.
pub fn vector_doc(path: &str, bytes: &[u8], layer: Option<&effectcraft_project::SourceLayer>, page: u32) -> Option<Result<Doc, String>> {
    if path.to_ascii_lowercase().ends_with(".svg") || effectcraft_svg::looks_like_svg(bytes) {
        return Some(effectcraft_svg::parse(bytes).map_err(|e| format!("{path}: {e}")));
    }
    effectcraft_pdf::sniff(bytes)?;
    Some(
        effectcraft_pdf::parse_page(bytes, page as usize)
            .map(|d| match layer {
                Some(l) => effectcraft_pdf::layer_doc_with_sizes(&d, l.index as usize, l.layer_size).0,
                None => d,
            })
            .map_err(|e| format!("{path}: {e}")),
    )
}

/// Import page `page` of a PDF / Illustrator / EPS document as a composition the size of the
/// page: one footage item per file layer (optionally cropped to its artwork, continuously
/// rasterisable), top layer first. Returns (comp, folder, items).
pub fn import_vector_comp(
    proj: &mut Project,
    path: &str,
    bytes: &[u8],
    name: &str,
    page: u32,
    retain: bool,
    timing: (effectcraft_time::FrameRate, effectcraft_time::Tick),
) -> Result<(ItemId, ItemId, Vec<ItemId>), String> {
    use effectcraft_color::Label;
    use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemKind, SourceLayer};
    let (rate, duration) = timing;
    let doc = effectcraft_pdf::parse_page(bytes, page as usize).map_err(|e| format!("{path}: {e}"))?;
    let codec = effectcraft_pdf::codec(path, bytes).unwrap_or("PDF");
    let (w, h) = doc.pixel_size();
    let file = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let folder = proj.add_item(&format!("{name} Layers"), Label::Yellow, None, ItemKind::Folder);
    let mut comp = Comp::new(w, h, rate, duration);
    let mut items = vec![];
    let names = effectcraft_pdf::layer_names(&doc);
    for (i, lname) in names.iter().enumerate() {
        let (size, offset) = if retain {
            let (layer, offset) = effectcraft_pdf::layer_doc_with_sizes(&doc, i, true);
            (layer.pixel_size(), offset)
        } else {
            ((w, h), [0.0, 0.0])
        };
        let f = Footage {
            path: path.to_string(),
            kind: FootageKind::Still,
            width: size.0,
            height: size.1,
            pixel_aspect: 1.0,
            frame_rate: rate,
            has_video: true,
            alpha: AlphaMode::Straight,
            loop_count: 1,
            codec: codec.into(),
            layer: Some(SourceLayer { index: i as u32, name: lname.clone(), layer_size: retain, ..Default::default() }),
            page,
            ..Default::default()
        };
        let id = proj.add_item(&format!("{lname}/{file}"), Label::Lavender, Some(folder), ItemKind::Footage(f));
        items.push(id);
        let mut l = build::layer(proj, &comp, lname, LayerSource::Footage { item: id }, size, None);
        if retain && let Some(position) = l.props.prop_mut("transform/position") {
            position.value = Value::Vec3([offset[0] + f64::from(size.0) / 2.0, offset[1] + f64::from(size.1) / 2.0, 0.0]);
        }
        l.name = comp.unique_layer_name(lname);
        comp.layers.insert(0, l);
    }
    let cid = proj.add_item(name, Label::Sandstone, None, ItemKind::Comp(std::sync::Arc::new(comp)));
    Ok((cid, folder, items))
}

/// A copy of `src`'s transform group with fresh uids.
fn copy_transform(proj: &mut Project, src: &Layer, dst: &mut Layer) {
    let Some(tr) = src.props.sub("transform") else { return };
    let mut tr = tr.clone();
    let mut n = proj.next_id;
    tr.reassign_uids(&mut n);
    proj.next_id = n + 1;
    if let Some(slot) = dst.props.children.iter_mut().find(|c| c.match_id() == "transform") {
        *slot = tr.into();
    }
}

fn like_source(dst: &mut Layer, src: &Layer) {
    dst.start_time = src.start_time;
    dst.in_point = src.in_point;
    dst.out_point = src.out_point;
    dst.stretch = src.stretch;
    dst.parent = src.parent;
    dst.switches.three_d = src.switches.three_d;
}

/// A raster image of a vector document (Create Shapes from Vector Layer makes it a footage
/// layer: shape layers have no images).
pub struct VectorImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Straight RGBA8.
    pub rgba: std::sync::Arc<Vec<u8>>,
    /// Image pixels → document pixels.
    pub transform: Affine,
    pub opacity: f64,
    /// Inside a clipping group or under a soft mask (not carried over).
    pub clipped: bool,
}

/// The document's images, bottom first, and how many soft-masked groups it has.
pub fn vector_images(doc: &Doc) -> (Vec<VectorImage>, usize) {
    fn walk(g: &effectcraft_svg::Group, m: Affine, opacity: f64, clipped: bool, out: &mut Vec<VectorImage>, masks: &mut usize) {
        let m = m * g.transform;
        let opacity = opacity * g.opacity;
        let clipped = clipped || !g.clip.is_empty() || g.mask.is_some();
        if g.mask.is_some() {
            *masks += 1;
        }
        for c in &g.children {
            match c {
                Node::Group(sub) => walk(sub, m, opacity, clipped, out, masks),
                Node::Image(im) => out.push(VectorImage {
                    name: im.name.clone(),
                    width: im.width,
                    height: im.height,
                    rgba: im.rgba.clone(),
                    transform: m * im.transform,
                    opacity: opacity * im.opacity,
                    clipped,
                }),
                Node::Shape(_) => {}
            }
        }
    }
    let (_, doc) = vector_clip_masks(doc);
    let mut out = vec![];
    let mut masks = 0;
    walk(&doc.root, Affine::IDENTITY, 1.0, false, &mut out, &mut masks);
    (out, masks)
}

/// The document split at its images (paint order): segment `k` keeps the shapes drawn between
/// image `k − 1` and image `k` (groups, clips and blend modes kept, images dropped), so shape
/// layers and image footage layers can stack in the document's order. One more segment than
/// images.
pub fn split_at_images(doc: &Doc) -> Vec<Doc> {
    fn count(g: &effectcraft_svg::Group) -> usize {
        g.children
            .iter()
            .map(|c| match c {
                Node::Group(s) => count(s),
                Node::Image(_) => 1,
                Node::Shape(_) => 0,
            })
            .sum()
    }
    /// Keep the shapes between images `seg − 1` and `seg`; `seen` counts images passed.
    fn prune(g: &effectcraft_svg::Group, seg: usize, seen: &mut usize) -> effectcraft_svg::Group {
        let mut out = g.clone();
        out.children = vec![];
        for c in &g.children {
            match c {
                Node::Image(_) => *seen += 1,
                Node::Shape(_) => {
                    if *seen == seg {
                        out.children.push(c.clone());
                    }
                }
                Node::Group(s) => {
                    let p = prune(s, seg, seen);
                    if !p.children.is_empty() {
                        out.children.push(Node::Group(p));
                    }
                }
            }
        }
        out
    }
    let n = count(&doc.root);
    (0..=n)
        .map(|seg| {
            let mut d = doc.clone();
            d.root = prune(&doc.root, seg, &mut 0);
            d
        })
        .collect()
}

/// A footage layer for an image of a vector document, parented to `parent` (the shape layer,
/// whose layer space is document pixels) and placed by the image's transform (anchor at its
/// top-left corner; position, rotation and scale from the matrix). Returns a note when the
/// placement has a skew, which layer transforms cannot express.
pub fn image_layer(proj: &mut Project, comp: &Comp, item: ItemId, img: &VectorImage, parent: &Layer) -> (Layer, Option<String>) {
    let mut l = build::layer(proj, comp, &img.name, LayerSource::Footage { item }, (img.width, img.height), None);
    let c = img.transform.as_coeffs();
    let theta = c[1].atan2(c[0]);
    let (sn, cs) = theta.sin_cos();
    let sx = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let sy = -c[2] * sn + c[3] * cs;
    let skew = c[2] * cs + c[3] * sn;
    let set = |l: &mut Layer, path: &str, v: Value| {
        if let Some(p) = l.props.prop_mut(path) {
            p.value = v;
        }
    };
    set(&mut l, "transform/anchor", Value::Vec3([0.0, 0.0, 0.0]));
    set(&mut l, "transform/position", Value::Vec3([c[4], c[5], 0.0]));
    set(&mut l, "transform/scale", Value::Vec3([sx * 100.0, sy * 100.0, 100.0]));
    set(&mut l, "transform/rotation", Value::Scalar(theta.to_degrees()));
    set(&mut l, "transform/opacity", Value::Scalar((img.opacity * 100.0).clamp(0.0, 100.0)));
    l.start_time = parent.start_time;
    l.in_point = parent.in_point;
    l.out_point = parent.out_point;
    l.stretch = parent.stretch;
    l.parent = Some(parent.id);
    let note = (skew.abs() > 1e-3 * sy.abs().max(1e-9)).then(|| format!("{}: skewed image placed without its skew", img.name));
    (l, note)
}

/// Layer ▸ Create ▸ Create Shapes from Vector Layer: `doc` is the SVG shown by `src`. A clip
/// enclosing the whole document becomes the layer's masks (Add, then Intersect); nested clips
/// are kept with Merge Paths (see [`svg_contents`]).
pub fn shapes_from_vector(proj: &mut Project, comp: &Comp, src: &Layer, doc: &Doc) -> Layer {
    let mut l = build::layer(proj, comp, &format!("{} Outlines", src.name), LayerSource::Shape, (0, 0), None);
    add_doc_shapes(proj, &mut l, doc);
    copy_transform(proj, src, &mut l);
    like_source(&mut l, src);
    l.name = comp.unique_layer_name(&format!("{} Outlines", src.name));
    l
}

/// Edit ▸ Paste of SVG markup (After Effects 26.3 pastes copied vector artwork as shape layers):
/// a shape layer named `name` with the document's shapes, its Anchor Point at the centre of the
/// SVG canvas and its Position at the comp centre, so the canvas lands centred in the comp.
pub fn shapes_from_svg(proj: &mut Project, comp: &Comp, doc: &Doc, name: &str) -> Layer {
    let mut l = build::layer(proj, comp, name, LayerSource::Shape, (0, 0), None);
    add_doc_shapes(proj, &mut l, doc);
    let (w, h) = (doc.width, doc.height);
    if w.is_finite()
        && h.is_finite()
        && let Some(a) = l.props.sub_mut("transform").and_then(|t| t.get_mut("anchor"))
    {
        a.value = Value::Vec3([w / 2.0, h / 2.0, 0.0]);
    }
    l
}

/// The shapes of `doc` into shape layer `l`'s Contents; clips enclosing the whole document
/// become its masks (Add, then Intersect).
fn add_doc_shapes(proj: &mut Project, l: &mut Layer, doc: &Doc) {
    let (clips, doc) = vector_clip_masks(doc);
    let mut ids = Ids(&mut proj.next_id);
    let items = svg_contents(&mut ids, &doc);
    if let Some(c) = l.props.sub_mut("contents") {
        c.children.extend(items.into_iter().map(Into::into));
    }
    let mut n = 0;
    for (k, clip) in clips.iter().enumerate() {
        for sp in effectcraft_path::from_kurbo(clip) {
            n += 1;
            let mode = if k == 0 { MaskMode::Add } else { MaskMode::Intersect };
            let mk = build::mask(&mut ids, &format!("Mask {n}"), sp, mode, [255, 255, 0]);
            if let Some(masks) = l.props.sub_mut("masks") {
                masks.children.push(mk.into());
            }
        }
    }
}

/// Layer ▸ Create ▸ Create Shapes from Text: one group per character with its outline, fill and
/// stroke (as laid out at the comp time `ctx` was made for).
pub fn shapes_from_text(proj: &mut Project, comp: &Comp, ctx_project: &Project, cid: ItemId, src: &Layer, time: effectcraft_time::Tick) -> Option<Layer> {
    let ctx = effectcraft_render::EvalCtx::new(ctx_project, cid, comp, time);
    let geom = effectcraft_render::text::text_geom(&ctx, src)?;
    let chars: Vec<char> = geom.doc.text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut l = build::layer(proj, comp, &format!("{} Outlines", src.name), LayerSource::Shape, (0, 0), None);
    let mut groups = vec![];
    {
        let mut ids = Ids(&mut proj.next_id);
        for (i, g) in geom.glyphs.iter().enumerate() {
            let path = g.path();
            if path.elements().is_empty() {
                continue;
            }
            let mut items = shape_paths(&mut ids, &path);
            if g.stroke_width > 0.0 && g.stroke[3] > 0.0 {
                let c = g.stroke;
                items.push(build::shape_stroke(&mut ids, [c[0] as f64, c[1] as f64, c[2] as f64, 1.0], g.stroke_width));
            }
            if g.apply_fill {
                let c = g.fill;
                let mut f = build::shape_fill(&mut ids, [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]);
                set(&mut f, "opacity", Value::Scalar(c[3] as f64 * 100.0));
                items.push(f);
            }
            let name = chars.get(i).map(|c| c.to_string()).unwrap_or_else(|| format!("Glyph {}", i + 1));
            groups.push(group(&mut ids, &name, items, 1.0));
        }
    }
    // First character on top, as in After Effects.
    if let Some(c) = l.props.sub_mut("contents") {
        c.children.extend(groups.into_iter().map(Into::into));
    }
    copy_transform(proj, src, &mut l);
    like_source(&mut l, src);
    l.name = comp.unique_layer_name(&format!("{} Outlines", src.name));
    Some(l)
}

/// Glyph outlines of a text layer in comp space (for Create Masks from Text).
pub fn text_outlines_in_comp(project: &Project, cid: ItemId, comp: &Comp, src: &Layer, time: effectcraft_time::Tick) -> Vec<ShapePath> {
    let ctx = effectcraft_render::EvalCtx::new(project, cid, comp, time);
    let Some(geom) = effectcraft_render::text::text_geom(&ctx, src) else { return vec![] };
    let (m, _) = ctx.layer_to_comp(src);
    let mut out = vec![];
    for g in &geom.glyphs {
        let p = g.path();
        let p = effectcraft_path::transform(std::slice::from_ref(&p), &m).remove(0);
        out.extend(effectcraft_path::from_kurbo(&p));
    }
    out
}

/// A comp-sized white solid with one mask per glyph contour (Difference, so counters stay open).
pub fn masks_layer(proj: &mut Project, comp: &Comp, name: &str, outlines: Vec<ShapePath>, solid: ItemId) -> Layer {
    let mut l = build::layer(proj, comp, name, LayerSource::Solid { item: solid }, (comp.width, comp.height), None);
    let mut ids = Ids(&mut proj.next_id);
    if let Some(masks) = l.props.sub_mut("masks") {
        for (i, sp) in outlines.into_iter().enumerate() {
            masks.children.push(build::mask(&mut ids, &format!("Mask {}", i + 1), sp, MaskMode::Difference, [255, 255, 0]).into());
        }
    }
    l
}
