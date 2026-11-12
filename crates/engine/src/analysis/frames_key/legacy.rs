//! Signatures as 0.22–0.24 wrote them, to recognise the keys those releases saved. Nothing new is
//! keyed with them. They hash derived `Debug` text, so `tests::analysis_signature` pins keys
//! those releases saved: if a type change alters this text, write that part out here.

use effectcraft_project::{Footage, ItemId, ItemKind, Layer, LayerSource, Project, Uid};

use super::fnv;

/// The signatures a saved key of `layer` may have been made from. The footage's availability is
/// tried both ways: those releases hashed it, though it isn't part of the footage (#3534). Before
/// `Footage::placed_page` was added in 0.23, the footage text ended without that field.
pub(crate) fn signatures(p: &Project, comp: ItemId, layer: &Layer) -> Vec<String> {
    let src = match &layer.source {
        LayerSource::Footage { item } | LayerSource::Solid { item } | LayerSource::Comp { item } => match p.item(*item).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) => {
                let mut out = Vec::with_capacity(4);
                for missing in [false, true] {
                    out.push(signature_with_source(p, comp, layer, &footage_text(f, missing, true)));
                    if matches!(layer.source, LayerSource::Footage { .. }) && !f.placed_page {
                        out.push(signature_with_source(p, comp, layer, &footage_text(f, missing, false)));
                    }
                }
                return out;
            }
            Some(ItemKind::Solid(s)) => format!("{s:?}"),
            Some(ItemKind::Comp(c)) => format!("{}x{}@{:?}/{:?}", c.width, c.height, c.frame_rate, c.duration),
            _ => String::new(),
        },
        _ => String::new(),
    };
    vec![signature_with_source(p, comp, layer, &src)]
}

/// A Roto Brush Input Key as 0.22–0.24 made it from a [`signatures`] entry.
pub(crate) fn input_key_of(mut s: String, layer: &Layer, uid: Uid) -> String {
    if let Some(m) = layer.masks() {
        s.push_str(&format!("{m:?}"));
    }
    if let Some(fx) = layer.effects() {
        for g in fx.groups() {
            if g.uid == uid {
                break;
            }
            s.push_str(&format!("{g:?}"));
        }
    }
    format!("{:016x}", fnv(&s))
}

/// A footage item as 0.23–0.24 wrote it into a signature (without `placed_page` before 0.23),
/// with `missing` in place of the item's.
pub(crate) fn footage_text(f: &Footage, missing: bool, placed_page: bool) -> String {
    format!("{:?}", FootageText { f, missing, placed_page })
}

struct FootageText<'a> {
    f: &'a Footage,
    missing: bool,
    placed_page: bool,
}

impl std::fmt::Debug for FootageText<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every field is named so that a new one doesn't compile until it is decided here: a new
        // field was never in these keys, so it stays out.
        let Footage {
            path,
            kind,
            width,
            height,
            pixel_aspect,
            frame_rate,
            native_rate,
            duration,
            has_video,
            has_audio,
            alpha,
            premul_color,
            loop_count,
            codec,
            missing: _,
            sequence,
            color_profile,
            layer,
            fields,
            invert_alpha,
            linear_light,
            preserve_rgb,
            cineon,
            data,
            page,
            alphabetical,
            start_timecode,
            file_timecode,
            markers,
            placed_page,
        } = self.f;
        let mut d = out.debug_struct("Footage");
        d.field("path", path)
            .field("kind", kind)
            .field("width", width)
            .field("height", height)
            .field("pixel_aspect", pixel_aspect)
            .field("frame_rate", frame_rate)
            .field("native_rate", native_rate)
            .field("duration", duration)
            .field("has_video", has_video)
            .field("has_audio", has_audio)
            .field("alpha", alpha)
            .field("premul_color", premul_color)
            .field("loop_count", loop_count)
            .field("codec", codec)
            .field("missing", &self.missing)
            .field("sequence", sequence)
            .field("color_profile", color_profile)
            .field("layer", layer)
            .field("fields", fields)
            .field("invert_alpha", invert_alpha)
            .field("linear_light", linear_light)
            .field("preserve_rgb", preserve_rgb)
            .field("cineon", cineon)
            .field("data", data)
            .field("page", page)
            .field("alphabetical", alphabetical)
            .field("start_timecode", start_timecode)
            .field("file_timecode", file_timecode)
            .field("markers", markers);
        if self.placed_page {
            d.field("placed_page", placed_page);
        }
        d.finish()
    }
}

fn signature_with_source(p: &Project, comp: ItemId, layer: &Layer, src: &str) -> String {
    let rate = p.comp(comp).map(|c| format!("{:?}", c.frame_rate)).unwrap_or_default();
    let s = format!(
        "{:?}|{src}|{:?}|{:?}|{:?}|{}|{:?}|{rate}",
        layer.source,
        layer.in_point,
        layer.out_point,
        layer.start_time,
        layer.stretch,
        layer.props.get("timeRemap")
    );
    format!("{:016x}", fnv(&s))
}
