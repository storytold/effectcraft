//! Hostile and damaged input: nothing may panic; whatever parses must write back identically.

mod common;

use common::fixture;
use effectcraft_aep::{AepError, parse, write};

/// Parses; on success, the invariant "parse then write is the identity" must hold.
fn check(bytes: &[u8]) -> Result<(), AepError> {
    let file = parse(bytes)?;
    let back = write(&file).map_err(|_| AepError::TooLarge)?;
    assert!(back == bytes, "round trip of an accepted input differs ({} bytes)", bytes.len());
    Ok(())
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn empty_and_tiny_inputs() {
    assert_eq!(check(&[]), Err(AepError::NotRifx));
    for n in 0..16 {
        let bytes = fixture("001-empty.aep");
        let _ = check(&bytes[..n]);
    }
}

#[test]
fn truncated_at_every_length() {
    let bytes = fixture("001-empty.aep");
    let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize + 8;
    for len in 0..=bytes.len() {
        let r = check(&bytes[..len]);
        if len < declared {
            assert!(r.is_err(), "length {len} of {declared} must be rejected");
        } else {
            assert!(r.is_ok(), "length {len} (>= declared end) must parse: {r:?}");
        }
    }
}

#[test]
fn truncated_larger_fixture_at_sampled_lengths() {
    let bytes = fixture("090-mask-rect.aep");
    let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize + 8;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..400 {
        let len = rng.below(bytes.len() + 1);
        let r = check(&bytes[..len]);
        assert_eq!(r.is_ok(), len >= declared, "len {len}");
    }
}

#[test]
fn random_byte_flips() {
    for (name, iterations) in [("001-empty.aep", 4000), ("000-base.aep", 1500), ("110-text-A.aep", 600)] {
        let original = fixture(name);
        let mut rng = Rng(0x1234_5678_9abc_def1 ^ original.len() as u64);
        let (mut ok, mut err) = (0, 0);
        for _ in 0..iterations {
            let mut b = original.clone();
            for _ in 0..=rng.below(8) {
                let at = rng.below(b.len());
                b[at] = rng.next() as u8;
            }
            match check(&b) {
                Ok(()) => ok += 1,
                Err(_) => err += 1,
            }
        }
        assert_eq!(ok + err, iterations, "{name}");
    }
}

#[test]
fn flips_in_the_headers_and_sizes() {
    // Aim at the bytes that matter most: the first 200 bytes, every value of every byte.
    let original = fixture("001-empty.aep");
    for at in 0..200 {
        for v in [0u8, 1, 0x7f, 0x80, 0xff] {
            let mut b = original.clone();
            b[at] = v;
            let _ = check(&b);
        }
    }
}

#[test]
fn oversized_chunk_sizes() {
    let original = fixture("001-empty.aep");
    // Every chunk header's size field set to huge values, one at a time, at every 4-byte aligned
    // offset of the first 2 KB.
    for at in (12..2048).step_by(2) {
        for v in [u32::MAX, 0x8000_0000, 0x0100_0000, original.len() as u32, 1 << 20] {
            let mut b = original.clone();
            b[at..at + 4].copy_from_slice(&v.to_be_bytes());
            let _ = check(&b);
        }
    }
    // The container size itself.
    for v in [0u32, 3, 4, 5, u32::MAX, original.len() as u32] {
        let mut b = original.clone();
        b[4..8].copy_from_slice(&v.to_be_bytes());
        let _ = check(&b);
    }
}

#[test]
fn garbage_after_a_valid_header() {
    let mut rng = Rng(42);
    for _ in 0..2000 {
        let n = rng.below(200);
        let mut b = b"RIFX\0\0\0\0Egg!".to_vec();
        for _ in 0..n {
            b.push(rng.next() as u8);
        }
        let size = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&size.to_be_bytes());
        let _ = check(&b);
    }
}

// ----------------------------------------------------------------------------------- typed view

use effectcraft_aep::{AepFile, Body, Chunk, Comp, Layer, Project, PropGroup, PropNode, Property};

/// Touches the property accessors; none may panic on a damaged property.
fn exercise_property(p: &Property<'_, '_>) {
    let _ = (p.header(), p.dims(), p.is_animated(), p.static_value(), p.time_scale(), p.flags(), p.is_static_flag_set());
    let _ = (p.expression(), p.expression_flags().map(|f| (f.all_set(), f.disabled())), p.expression_enabled(), p.user_name(), p.first_key_value());
    let _ = p.color_argb();
    if let Some(keys) = p.keys() {
        let scale = p.time_scale().unwrap_or(0);
        for k in keys.keyframes() {
            let _ = (k.time_units(), k.spatial_value(), k.raw().len(), k.kind(), k.seconds(scale), k.seconds(0));
            let _ = (k.interpolation_in(), k.interpolation_out(), k.flags(), k.label(), k.is_roving());
            let _ = (k.scalar_value(), k.ease(), k.spatial(), k.components(), k.value(), k.plain_ease_slots());
            let _ = (k.ease_components(), k.color_distance());
            for component in [0, 1, 2, 3, 7, 255, usize::MAX] {
                let _ = k.ease_component(component);
            }
            let _ = k.spatial().map(|s| (s.value.len(), s.in_tangent.len(), s.out_tangent.len(), s.ease));
        }
    }
}

fn exercise_group(g: &PropGroup<'_, '_>, depth: usize) {
    if depth > 24 {
        return;
    }
    let _ = (g.flags(), g.names(), g.user_name());
    for e in g.entries() {
        match e.node {
            PropNode::Group(g) => exercise_group(&g, depth + 1),
            PropNode::Property(p) => exercise_property(&p),
            PropNode::Other(c) => {
                if let Some(g) = c.child_list(b"tdgp").and_then(PropGroup::from_chunk) {
                    exercise_group(&g, depth + 1);
                }
            }
        }
    }
}

fn exercise_layer(project: &Project<'_, '_>, comp_size: (u32, u32), layer: &Layer<'_, '_>) {
    let _ = (layer.name(), layer.is_three_d(), layer.id(), layer.track_matte());
    if let Some(h) = layer.header() {
        let _ = (h.in_point(), h.out_point(), h.in_point_seconds(), h.out_point_seconds(), h.start_seconds(), h.stretch_percent());
        let _ = (h.stretch_ratio(), h.stretch_den);
        let _ = (h.is_shy(), h.is_locked(), h.is_solo(), h.is_three_d(), h.is_motion_blur(), h.is_adjustment(), h.is_guide());
        let _ = (h.is_null_object(), h.collapses_transformations(), h.name_is_user_set(), h.quality, h.blend_mode, h.layer_type);
        let _ = (h.is_enabled(), h.is_video_enabled(), h.is_audio_enabled(), h.is_effects_enabled(), h.is_auto_oriented(), h.light_type(), h.blend_mode.code());
    }
    let _ = (layer.is_enabled(), layer.is_audio_enabled(), layer.is_effects_enabled());
    let _ = (layer.camera_position().map(|p| p.reported(comp_size.0, comp_size.1, true)), layer.camera_point_of_interest(), layer.light_intensity());
    let _ = project.layer_anchor_px(layer);
    let _ = (layer.camera_zoom(), layer.camera_depth_of_field(), layer.camera_focus_distance(), layer.camera_aperture());
    let _ = (layer.light_color(), layer.light_cone_angle(), layer.light_cone_feather());
    let _ = layer
        .time_remap()
        .map(|t| (t.enabled, t.keys.iter().fold(0.0, |acc, k| acc + k.value_seconds + k.time_seconds.unwrap_or(0.0) + f64::from(k.time_units))));
    for g in layer.shape_groups() {
        let _ = (g.name, g.position, g.details.len());
        for d in &g.details {
            match d {
                effectcraft_aep::ShapeDetail::Polystar { star_type, outer_radius, inner_radius, inner_roundness, item_index } => {
                    let _ = (star_type.value, outer_radius.value, inner_radius.value, inner_roundness.map(|r| r.value), *item_index);
                }
                effectcraft_aep::ShapeDetail::Gradient { gradient_type, highlight_length, highlight_angle, item_index } => {
                    let _ = (gradient_type.value, highlight_length.map(|t| t.value), highlight_angle.map(|t| t.value), *item_index);
                }
                effectcraft_aep::ShapeDetail::RectPosition { position, item_index } => {
                    let _ = (position.value, *item_index);
                }
            }
        }
        for item in g.items {
            if let effectcraft_aep::ShapeItem::Path { paths, .. } = item {
                for s in paths {
                    let _ = s.vertices((1.0, 1.0)).iter().fold(0.0, |acc, v| acc + v.in_tangent()[0] + v.out_tangent()[1]);
                }
            }
        }
    }
    let _ = (project.layer_source(layer).map(|i| i.id()), project.layer_display_name(layer), project.layer_source_size(layer));
    if let Some(t) = layer.transform(comp_size.0, comp_size.1) {
        let _ = (t.anchor_px(f64::from(comp_size.0), f64::from(comp_size.1)), t.position.value, t.opacity_percent.value);
    }
    if let Some(root) = layer.properties() {
        exercise_group(&root, 0);
    }
    for e in layer.effects() {
        let _ = (e.match_name().len(), e.name(), e.declared_param_count(), e.has_own_definitions(), project.definition_source(&e));
        for p in e.params().into_iter().chain(project.effect_params(&e)) {
            let _ = (p.index, p.kind, p.name.len(), p.items.as_ref().map(Vec::len), p.pard().len(), p.value(), p.stored_value(), p.pard_default());
            let _ = (p.is_default(), p.popup_item_count(), p.popup_selected_index(), p.popup_selected_item(), p.valid_max(), p.is_animated());
            let _ = (p.valid_min(), p.slider_range());
            if let Some(keys) = p.keys() {
                for k in keys.keyframes() {
                    let _ = (k.value(), k.spatial(), k.time_units(), k.ease());
                }
            }
            for owner in [0, 1, 0x1b, u32::MAX] {
                let _ = p.layer_ref(owner);
            }
            for size in [0.0, 1920.0, f64::NAN, f64::INFINITY, f64::MAX] {
                let _ = p.point_px(size, size);
            }
            if let Some(s) = p.stored() {
                exercise_property(&s);
            }
        }
    }
    for m in layer.masks() {
        let _ = (m.name.clone(), m.mode, m.inverted, m.counter, m.feather, m.opacity_percent, m.expansion, m.path_animated, m.path_key_seconds.len());
        let _ = project.mask_vertices_px(layer, &m).map(|shapes| shapes.iter().map(Vec::len).sum::<usize>());
        for s in &m.shapes {
            let _ = (s.vertex_count(), s.vertices((1920.0, 1080.0)).len(), s.vertices((f64::NAN, f64::INFINITY)).len(), s.open);
        }
    }
    if let Some(t) = layer.text_document() {
        let _ = (t.fill_rgb(), t.fonts.len(), t.text.len(), t.raw_text.len(), t.font_size, t.font.clone(), t.paragraph_texts().len());
        let _ = (t.paragraphs.iter().fold(0, |acc, p| acc + p.length), t.style_runs.iter().fold(0.0, |acc, r| acc + r.tracking + r.fill_argb[0]), t.text_box);
        let _ = t
            .style_runs
            .iter()
            .fold(0.0, |acc, r| acc + r.leading.unwrap_or(0.0) + f64::from(u8::from(r.faux_bold)) + f64::from(u8::from(r.faux_italic)) + r.font_index as f64);
        let _ = t.style_runs.iter().map(|r| r.caps).collect::<Vec<_>>();
        let _ = t.paragraphs.iter().map(|p| p.justification).collect::<Vec<_>>();
    }
    for m in layer.markers() {
        let _ = (m.comment.len(), m.chapter.len(), m.url.len(), m.frame_target.len(), m.cue_point_name.len(), m.duration_seconds);
        let _ = (m.label, m.cue_type, m.cue_params.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>());
    }
}

fn exercise_comp(project: &Project<'_, '_>, comp: &Comp<'_, '_>) {
    let tb = comp.time_base();
    let _ = tb.map(|t| (t.frame_rate(), t.seconds(i32::MIN), t.seconds(i32::MAX)));
    let _ = (comp.name(), comp.renderer(), comp.markers());
    let mut size = (1920, 1080);
    if let Some(h) = comp.header() {
        size = (u32::from(h.width), u32::from(h.height));
        let _ = (h.frame_rate(), h.frame_rate_from_fixed(), h.duration_seconds(), h.work_area_start_seconds(), h.work_area_end_seconds());
        let _ = (h.frame_rate_exact(), h.duration_frames());
        let _ = (h.background_rgb(), h.pixel_aspect_ratio(), h.pixel_aspect.reduced(), h.shutter_angle, h.motion_blur);
        let _ = (h.shutter_phase, h.display_start, h.display_start_seconds(), h.display_start_frame());
    }
    for layer in comp.layers() {
        exercise_layer(project, size, &layer);
    }
}

/// Touches every accessor of the typed view; none may panic on a damaged tree.
fn exercise(file: &AepFile<'_>) {
    let Some(project) = Project::new(file) else { return };
    for item in project.all_items() {
        let _ = (item.kind(), item.name(), item.id(), item.label(), item.used_item_ids(), item.footage(), item.display_name());
        let _ = item
            .footage()
            .map(|f| (f.duration, f.native_frame_rate, f.bits_per_pixel, f.file_size, f.audio_sample_rate, f.is_sequence, f.sequence_frame_count()));
        let _ = item.id().and_then(|id| project.item_by_id(id));
        if let Some(comp) = item.comp() {
            exercise_comp(&project, &comp);
        }
    }
}

/// The fixtures whose typed fields are decoded, one per kind of chunk body.
const DECODED: &[&str] = &[
    "000-base.aep",
    "010-layer-name.aep",
    "022-layer-parent-null.aep",
    "030-comp-size-1280x720.aep",
    "035-comp-workarea-1-2.aep",
    "039-comp-renderer-advanced3d.aep",
    "044-static-anchor-10-20.aep",
    "052-key-position-2key-ease.aep",
    "054-key-position-2key-spatial-bezier.aep",
    "056-key-position-3key-roving-key2.aep",
    "058-key-opacity-2key.aep",
    "060-expr-opacity-wiggle.aep",
    "071-fx-slider-value77.aep",
    "073-fx-color-0.2-0.6-1.aep",
    "077-fx-dropdown-default.aep",
    "080-fx-dropshadow-default.aep",
    "091-mask-rect-props.aep",
    "093-mask-path-2keys.aep",
    "113-text-Hello-size60-fillorange.aep",
    "122-marker-comp-1s.aep",
    "123-marker-layer-1s.aep",
    "124-precomp-Pre.aep",
    "126-footage-png-layer.aep",
    "202-key-out-hold-in-linear.aep",
    "204-key-ease-asymmetric.aep",
    "205-key-opacity-ease-asymmetric.aep",
    "206-key-scale-2key-linear.aep",
    "208-key-mask-feather-2key-linear.aep",
    "210-key-fill-color-2key-linear.aep",
    "211-anchor-100x100-solid-10-20.aep",
    "213-expr-opacity-wiggle-disabled.aep",
    "214-fx-gaussianblur-default.aep",
    "215-fx-fill-default.aep",
    "218-fx-point-111-222.aep",
    "220-fx-layercontrol-to-layer1.aep",
    "221-fx-slider-2key.aep",
    "222-layer-video-off.aep",
    "228-layer-blend-difference.aep",
    "229-comp-fps-29.97.aep",
    "232-mask-ellipse.aep",
    "233-mask-open-path.aep",
    "237-mask-mode-difference.aep",
    "239-shape-stroke-green-w8-round.aep",
    "240-shape-custom-path-tangents.aep",
    "241-shape-gradient-fill-linear.aep",
    "242-shape-group-position-50-50.aep",
    "245-text-box-400x200-Hello.aep",
    "248-text-range-fill-red-1-3.aep",
    "249-marker-layer-full.aep",
    "251-camera-one-node.aep",
    "252-light-point.aep",
    "253-camera-position-100-200-m1000.aep",
    "254-light-spot-intensity-50.aep",
    "259-solid-blue-0-0-1.aep",
    "300-fx-gaussianblur-0-explicit.aep",
    "305-key-scale-bezier-ease.aep",
    "306-key-maskfeather-bezier-ease.aep",
    "307-key-fillcolor-bezier.aep",
    "309-solid-100x100-mask-rect.aep",
    "310-solid-100x100-pointcontrol.aep",
    "317-layer-stretch150.aep",
    "318-layer-inpoint-0.3333.aep",
    "319-precomp-timeremap.aep",
    "321-fx-layercontrol-none.aep",
    "324-fx-dropdown-5items-select3.aep",
    "327-comp-displaystartframe-100.aep",
    "328-mask-two-ellipse-rect.aep",
    "329-mask-ellipse-mode-none.aep",
    "331-shape-polystar-polygon-5-100.aep",
    "332-shape-gradient-fill-radial.aep",
    "333-shape-group-two-rects.aep",
    "334-text-box-right-align.aep",
    "335-text-point-fauxbold.aep",
    "340-text-range-two-styles.aep",
    "342-camera-dof-800-30.aep",
    "343-light-spot-color-red.aep",
    "349-marker-layer-event-cue-params.aep",
    "351-footage-wav-as-layer.aep",
    "352-footage-pngseq-imported.aep",
    "354-footage-pngseq-timeremap.aep",
    "364-fx-tint-white-2key.aep",
    "382-fx-ramp-start-2key.aep",
    "383-fx-ramp-on-100x100-start-10-20.aep",
    "389-fx-exposure-default.aep",
    "395-fx-exposure-individual-red-1.aep",
    "397-fx-exposure-master-2key.aep",
];

#[test]
fn typed_view_reads_every_undamaged_fixture() {
    for name in common::FIXTURES {
        let bytes = fixture(name);
        exercise(&parse(&bytes).unwrap());
    }
}

#[test]
fn typed_view_survives_damaged_files() {
    for (name, iterations) in [("000-base.aep", 1500), ("055-key-position-3key-inserted-t2.5.aep", 800), ("071-fx-slider-value77.aep", 800)] {
        let original = fixture(name);
        let mut rng = Rng(0xfeed_beef_1234_5678 ^ original.len() as u64);
        for _ in 0..iterations {
            let mut b = original.clone();
            for _ in 0..=rng.below(12) {
                let at = rng.below(b.len());
                b[at] = rng.next() as u8;
            }
            if let Ok(file) = parse(&b) {
                exercise(&file);
            }
        }
    }
}

#[test]
fn typed_view_survives_byte_flips_in_the_decoded_fixtures() {
    for name in DECODED {
        let original = fixture(name);
        let mut rng = Rng(0x0dd_ba11_cafe_f00d ^ original.len() as u64);
        let (mut parsed, mut total) = (0, 0);
        for _ in 0..250 {
            let mut b = original.clone();
            for _ in 0..=rng.below(16) {
                let at = rng.below(b.len());
                b[at] = rng.next() as u8;
            }
            total += 1;
            if let Ok(file) = parse(&b) {
                parsed += 1;
                exercise(&file);
            }
        }
        // Flips that land inside bodies keep the file parseable; sizes that break the structure do not.
        assert!(parsed > 0 && parsed <= total, "{name}: {parsed} of {total}");
    }
}

#[test]
fn truncated_typed_files_never_panic() {
    // A cut inside the container is an error; a cut in the trailer still parses. Either way no panic.
    for name in DECODED {
        let original = fixture(name);
        let declared = u32::from_be_bytes([original[4], original[5], original[6], original[7]]) as usize + 8;
        let mut rng = Rng(0x7e57_7e57 ^ original.len() as u64);
        for _ in 0..120 {
            let len = rng.below(original.len() + 1);
            if let Ok(file) = parse(&original[..len]) {
                assert!(len >= declared, "{name}: {len} < {declared}");
                exercise(&file);
            }
        }
        // Also cut right before and after the declared end.
        for len in [declared - 1, declared, declared + 1] {
            if let Ok(file) = parse(&original[..len.min(original.len())]) {
                exercise(&file);
            }
        }
    }
}

/// The chunks whose bodies the typed view decodes.
const DECODED_IDS: &[&[u8; 4]] = &[
    b"ldta", b"cdta", b"prin", b"idta", b"idpi", b"tdb4", b"cdat", b"tdsb", b"lhd3", b"ldat", b"mkif", b"shph", b"parn", b"pard", b"sspc", b"opti", b"alas",
    b"Utf8", b"tdmn", b"NmHd", b"btdk", b"tdpi",
];

/// Applies a random mutation to a body.
fn damage(body: &mut Vec<u8>, rng: &mut Rng) {
    match rng.below(7) {
        0 => body.truncate(rng.below(body.len() + 1)),
        1 => body.clear(),
        2 => {
            for _ in 0..=rng.below(4) {
                if !body.is_empty() {
                    let at = rng.below(body.len());
                    body[at] = rng.next() as u8;
                }
            }
        }
        3 => {
            // A 4-byte window set to an extreme value (sizes, counts, offsets).
            if body.len() >= 4 {
                let at = rng.below(body.len() - 3);
                let v = [0xffff_ffffu32, 0x8000_0000, 0x7fff_ffff, 0, 1][rng.below(5)];
                body[at..at + 4].copy_from_slice(&v.to_be_bytes());
            }
        }
        4 => body.extend((0..=rng.below(64)).map(|_| rng.next() as u8)),
        5 => {
            // An 8-byte window set to NaN / infinity / a huge double.
            if body.len() >= 8 {
                let at = rng.below(body.len() - 7);
                let v = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e308, -1e308][rng.below(5)];
                body[at..at + 8].copy_from_slice(&v.to_be_bytes());
            }
        }
        _ => body.iter_mut().for_each(|b| *b = 0xff),
    }
}

fn damage_tree(chunks: &mut [Chunk<'static>], rng: &mut Rng, depth: usize) {
    if depth > 70 {
        return;
    }
    for chunk in chunks {
        let id = chunk.id;
        match &mut chunk.body {
            Body::Data(d) => {
                if DECODED_IDS.contains(&&id) && rng.below(12) == 0 {
                    damage(d.to_mut(), rng);
                }
            }
            Body::OpaqueList { data, .. } => {
                if rng.below(3) == 0 {
                    damage(data.to_mut(), rng);
                }
            }
            Body::Chunks(c) | Body::List { children: c, .. } => damage_tree(c, rng, depth + 1),
        }
    }
}

#[test]
fn typed_view_survives_damaged_chunk_bodies() {
    // Damage the bodies the typed view decodes (not the container framing), directly in the tree:
    // wrong sizes, empty and truncated bodies, extreme counts, NaN doubles, unterminated text.
    for name in DECODED {
        let bytes = fixture(name);
        let original = parse(&bytes).unwrap().into_owned();
        let mut rng = Rng(0xbad_c0de_0001 ^ bytes.len() as u64);
        for _ in 0..120 {
            let mut file = original.clone();
            damage_tree(&mut file.chunks, &mut rng, 0);
            exercise(&file);
        }
    }
}

#[test]
fn extreme_values_in_the_decoded_headers() {
    use effectcraft_aep::{CompHeader, LayerHeader, Rational, ShapePath};
    // Every field at its maximum: no overflow, no division by zero, no panic.
    let h = LayerHeader::parse(&[0xff; 164]).unwrap();
    let _ = (h.in_point(), h.out_point(), h.in_point_seconds(), h.out_point_seconds(), h.start_seconds(), h.stretch_percent());
    let mut zero_den = [0u8; 164];
    zero_den[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    zero_den[12..16].copy_from_slice(&i32::MIN.to_be_bytes());
    let h = LayerHeader::parse(&zero_den).unwrap();
    assert_eq!((h.in_point(), h.out_point(), h.start_seconds()), (None, None, None));
    let mut wide = [0u8; 164];
    wide[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    wide[12..16].copy_from_slice(&i32::MIN.to_be_bytes());
    wide[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    wide[20..24].copy_from_slice(&i32::MAX.to_be_bytes());
    wide[24..28].copy_from_slice(&u32::MAX.to_be_bytes());
    let h = LayerHeader::parse(&wide).unwrap();
    let _ = (h.in_point(), h.in_point_seconds());
    let c = CompHeader::parse(&[0xff; 204]).unwrap();
    assert_eq!((c.duration_seconds(), c.work_area_start_seconds(), c.work_area_end_units), (None, None, None));
    let _ = (c.frame_rate(), c.frame_rate_from_fixed(), c.background_rgb(), c.pixel_aspect_ratio(), c.frame_rate_exact());
    assert_eq!(c.duration_frames(), Some(1));
    let zero = CompHeader::parse(&[0; 204]).unwrap();
    assert_eq!((zero.frame_rate(), zero.pixel_aspect_ratio(), zero.duration_seconds()), (None, None, None));
    assert_eq!((zero.frame_rate_exact(), zero.duration_frames()), (None, None));
    let all = LayerHeader::parse(&[0xff; 164]).unwrap();
    let _ = (all.is_enabled(), all.is_auto_oriented(), all.light_type(), all.blend_mode.code());
    assert_eq!(LayerHeader::parse(&[0u8; 164]).unwrap().light_type(), None);
    assert_eq!(Rational { num: i64::MAX, den: i64::MIN }.reduced(), None);
    assert_eq!(Rational { num: i64::MIN, den: i64::MIN }.reduced(), Rational::new(1, 1));
    // A path whose point count is not a multiple of three, and non-finite box and scale.
    let path = ShapePath { bbox: [f32::NAN, f32::INFINITY, f32::MIN, f32::MAX], closed_flag: 9, open: true, points: vec![[f32::NAN, 0.0]; 7] };
    assert_eq!(path.vertex_count(), 2);
    assert_eq!(path.vertices((f64::NAN, f64::MAX)).len(), 2);
}

/// Calls `f` on the body of every data chunk with this id, anywhere in the tree.
fn for_each_body(chunks: &mut [Chunk<'static>], id: &[u8; 4], f: &mut dyn FnMut(&mut Vec<u8>), depth: usize) {
    if depth > 70 {
        return;
    }
    for chunk in chunks {
        let this = chunk.id;
        match &mut chunk.body {
            Body::Data(d) if this == *id => f(d.to_mut()),
            Body::Data(_) | Body::OpaqueList { .. } => {}
            Body::Chunks(c) | Body::List { children: c, .. } => for_each_body(c, id, f, depth + 1),
        }
    }
}

#[test]
fn hostile_keyframe_layouts_never_panic() {
    // The property header selects the record template: every component count (0..=255) with the
    // template flag at 0, 1 and 0xff, over keyframe lists of every record size in these fixtures.
    for name in [
        "204-key-ease-asymmetric.aep",
        "206-key-scale-2key-linear.aep",
        "208-key-mask-feather-2key-linear.aep",
        "210-key-fill-color-2key-linear.aep",
        "093-mask-path-2keys.aep",
        "122-marker-comp-1s.aep",
        "305-key-scale-bezier-ease.aep",
        "306-key-maskfeather-bezier-ease.aep",
        "307-key-fillcolor-bezier.aep",
        "319-precomp-timeremap.aep",
    ] {
        let bytes = fixture(name);
        let original = parse(&bytes).unwrap().into_owned();
        for components in 0..=255u8 {
            for template in [0u8, 1, 0xff] {
                let mut file = original.clone();
                for_each_body(
                    &mut file.chunks,
                    b"tdb4",
                    &mut |body| {
                        if body.len() > 7 {
                            body[3] = components;
                            body[7] = template;
                        }
                    },
                    0,
                );
                exercise(&file);
            }
        }
    }
}

#[test]
fn hostile_text_dictionaries_and_marker_headers_never_panic() {
    // Damage the marker header (NmHd), the strings and the layer reference in many ways, and the
    // opaque text lists.
    for name in [
        "245-text-box-400x200-Hello.aep",
        "248-text-range-fill-red-1-3.aep",
        "249-marker-layer-full.aep",
        "220-fx-layercontrol-to-layer1.aep",
        "334-text-box-right-align.aep",
        "335-text-point-fauxbold.aep",
        "340-text-range-two-styles.aep",
        "349-marker-layer-event-cue-params.aep",
        "321-fx-layercontrol-none.aep",
        "324-fx-dropdown-5items-select3.aep",
        "352-footage-pngseq-imported.aep",
        "350-footage-wav-imported.aep",
    ] {
        let bytes = fixture(name);
        let original = parse(&bytes).unwrap().into_owned();
        let mut rng = Rng(0x07e4_70dd_ba11 ^ bytes.len() as u64);
        for _ in 0..300 {
            let mut file = original.clone();
            for id in [b"NmHd", b"Utf8", b"tdpi"] {
                for_each_body(&mut file.chunks, id, &mut |body| damage(body, &mut rng), 0);
            }
            damage_tree(&mut file.chunks, &mut rng, 0);
            exercise(&file);
        }
    }
}

#[test]
fn extreme_values_in_the_batch_3_fields() {
    use effectcraft_aep::{CompHeader, Footage, LayerHeader, Rational};
    // Time stretch: zero and maximal numerators and denominators (bytes 8 and 108), with extreme times.
    for (num, den) in [(0u32, 0u32), (1, 0), (0, 1), (u32::MAX, 1), (1, u32::MAX), (u32::MAX, u32::MAX), (3, 2)] {
        let mut ldta = [0u8; 164];
        ldta[8..12].copy_from_slice(&num.to_be_bytes());
        ldta[108..112].copy_from_slice(&den.to_be_bytes());
        ldta[12..16].copy_from_slice(&i32::MIN.to_be_bytes());
        ldta[16..20].copy_from_slice(&1u32.to_be_bytes());
        ldta[20..24].copy_from_slice(&i32::MAX.to_be_bytes());
        ldta[24..28].copy_from_slice(&u32::MAX.to_be_bytes());
        ldta[28..32].copy_from_slice(&i32::MIN.to_be_bytes());
        ldta[32..36].copy_from_slice(&1u32.to_be_bytes());
        let h = LayerHeader::parse(&ldta).unwrap();
        let _ = (h.stretch_ratio(), h.stretch_percent(), h.in_point(), h.out_point(), h.in_point_seconds(), h.out_point_seconds());
        if den == 0 {
            assert_eq!((h.stretch_ratio(), h.in_point(), h.out_point()), (None, None, None));
        }
    }
    // Display start and shutter phase from extreme bytes; a missing tail leaves them unknown.
    for fill in [0x00u8, 0x7f, 0x80, 0xff] {
        let c = CompHeader::parse(&[fill; 204]).unwrap();
        let _ = (c.shutter_phase, c.display_start, c.display_start_seconds(), c.display_start_frame());
    }
    let zero_rate = CompHeader::parse(&[0; 204]).unwrap();
    assert_eq!((zero_rate.display_start_seconds(), zero_rate.display_start_frame()), (None, None));
    // Footage durations: zero denominator, negative and maximal numerators, a huge frame rate.
    let base = Footage {
        file_type: String::new(),
        width: 0,
        height: 0,
        name: None,
        is_solid: false,
        solid_argb: None,
        path: None,
        duration: None,
        native_frame_rate: 0,
        bits_per_pixel: 0,
        file_size: 0,
        audio_sample_rate: None,
        is_sequence: true,
    };
    for (num, den, rate) in [(3i64, 30i64, 30u32), (3, 0, 30), (-3, 30, 30), (i64::MAX, 1, u32::MAX), (i64::MIN, -1, u32::MAX), (1, i64::MAX, 1), (0, 1, 0)] {
        let f = Footage { duration: Some(Rational { num, den }), native_frame_rate: rate, ..base.clone() };
        let _ = f.sequence_frame_count();
    }
    assert_eq!(Footage { duration: Some(Rational { num: 3, den: 30 }), native_frame_rate: 30, ..base.clone() }.sequence_frame_count(), Some(3));
    assert_eq!(Footage { duration: Some(Rational { num: 3, den: 0 }), native_frame_rate: 30, ..base.clone() }.sequence_frame_count(), None);
    assert_eq!(Footage { duration: None, ..base }.sequence_frame_count(), None);
}
