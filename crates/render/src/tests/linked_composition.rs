//! Synthetic source projects exercise the flat cross-project render boundary.

use std::sync::Arc;

use effectcraft_color::{ColorSpace, Label};
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, Expression, Footage, FootageKind, ItemId, ItemKind, LayerSource, LinkedComposition, Project, Property, build};
use effectcraft_time::{FrameRate, Tick};

use crate::{EvalCtx, ExprHost, FootageSource, Image, LayerCache, NoFootage, RenderOpts, Renderer, audio, reads::ReadLog};

fn scene() -> (Project, ItemId, Comp) {
    let mut project = Project::default();
    project.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(16, 16, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let id = project.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (project, id, comp)
}

fn linked(source: Arc<Project>, source_comp: ItemId) -> Footage {
    let comp = source.comp(source_comp).unwrap();
    Footage {
        kind: FootageKind::LinkedComposition,
        path: "source.ecproj".into(),
        width: comp.width,
        height: comp.height,
        duration: comp.duration,
        frame_rate: comp.frame_rate,
        has_video: true,
        linked_comp: Some(LinkedComposition {
            source_project_id: "source".into(),
            composition_id: "comp".into(),
            last_known_name: "Comp".into(),
            resolved: Some(source.clone()),
            resolved_comp: Some(source_comp),
            ..Default::default()
        }),
        ..Default::default()
    }
}

struct SourceOpacity;
impl ExprHost for SourceOpacity {
    fn eval(&self, ctx: &EvalCtx, _: &effectcraft_project::Layer, _: &Property, _: &Value) -> Result<Value, String> {
        Ok(Value::Scalar(if ctx.project.project_id == "source" { 50.0 } else { 0.0 }))
    }
}

#[test]
fn linked_picture_keeps_source_expression_context_and_local_instance_opacity() {
    let (mut source, source_id, source_comp) = scene();
    source.project_id = "source".into();
    let mut fill = super::solid(&mut source, &source_comp, [1.0, 0.0, 0.0], 8, 8);
    fill.props.prop_mut("transform/opacity").unwrap().expr = Some(Expression { text: "source project".into(), enabled: true });
    source.comp_mut(source_id).unwrap().layers.push(fill);
    let (mut destination, comp_id, comp) = scene();
    destination.project_id = "destination".into();
    let item = destination.add_item("Linked", Label::Sandstone, None, ItemKind::Footage(linked(Arc::new(source), source_id)));
    let mut instance = build::layer(&mut destination, &comp, "Linked", LayerSource::Footage { item }, (16, 16), None);
    instance.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(50.0);
    destination.comp_mut(comp_id).unwrap().layers.push(instance);
    let mut renderer = Renderer::new(&destination, &NoFootage, RenderOpts::default());
    renderer.expr = Some(&SourceOpacity);
    let frame = renderer.comp_frame(comp_id, Tick::ZERO);
    assert!(frame.get(8, 8)[0] > 0.0, "source expression saw the source project");
    assert!(frame.get(8, 8)[3] < 0.5, "destination opacity applies to the source result");
    assert!(frame.get(0, 0)[3] < frame.get(8, 8)[3], "source alpha survives the boundary");
}

#[test]
fn a_replaced_link_snapshot_never_reuses_cached_source_pixels() {
    let (mut source, source_id, source_comp) = scene();
    let fill = super::solid(&mut source, &source_comp, [1.0, 0.0, 0.0], 16, 16);
    source.comp_mut(source_id).unwrap().layers.push(fill);
    let (mut destination, comp_id, comp) = scene();
    let item = destination.add_item("Linked", Label::Sandstone, None, ItemKind::Footage(linked(Arc::new(source.clone()), source_id)));
    let instance = build::layer(&mut destination, &comp, "Linked", LayerSource::Footage { item }, (16, 16), None);
    destination.comp_mut(comp_id).unwrap().layers.push(instance);
    let cache = LayerCache::default();
    let render = |project: &Project| {
        let log = ReadLog::new();
        let mut renderer = Renderer::new(project, &NoFootage, RenderOpts::default());
        renderer.cache = Some(&cache);
        renderer.reads = Some(&log);
        let image = renderer.comp_frame(comp_id, Tick::ZERO);
        assert!(log.files().is_unkeyed(), "prototype frames cannot enter the persistent cache");
        image
    };
    let before = render(&destination);
    let solid = source.items.values_mut().find_map(|item| if let ItemKind::Solid(s) = &mut item.kind { Some(s) } else { None }).unwrap();
    solid.color = [0.0, 1.0, 0.0];
    let ItemKind::Footage(footage) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
    footage.linked_comp.as_mut().unwrap().resolved = Some(Arc::new(source));
    let after = render(&destination);
    assert_ne!(before.data, after.data);
    assert!(after.get(8, 8)[1] > after.get(8, 8)[0]);
}

struct Sound;

#[test]
fn unused_links_keep_unrelated_layer_and_frame_caches_available() {
    let (source, source_id, _) = scene();
    let (mut destination, comp_id, comp) = scene();
    let item = destination.add_item("Linked", Label::Brown, None, ItemKind::Footage(linked(Arc::new(source), source_id)));
    let fill = super::solid(&mut destination, &comp, [1.0, 0.0, 0.0], 16, 16);
    destination.comp_mut(comp_id).unwrap().layers.push(fill);
    // Even another comp using the linked item must leave this unrelated comp cacheable.
    let other = destination.add_item("Uses link", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let layer = build::layer(&mut destination, &comp, "Linked", LayerSource::Footage { item }, (16, 16), None);
    destination.comp_mut(other).unwrap().layers.push(layer);
    let fill = &destination.comp(comp_id).unwrap().layers[0];
    let ctx = EvalCtx::new(&destination, comp_id, destination.comp(comp_id).unwrap(), Tick::ZERO);
    assert!(crate::cache::input_key(&ctx, fill, 1.0, false, false, 0).is_some());
    let log = ReadLog::new();
    let mut renderer = Renderer::new(&destination, &NoFootage, RenderOpts::default());
    renderer.reads = Some(&log);
    renderer.comp_frame(comp_id, Tick::ZERO);
    assert!(!log.files().is_unkeyed(), "unused links cannot prevent persistence of an unrelated frame");
    renderer.comp_frame(other, Tick::ZERO);
    assert!(log.files().is_unkeyed(), "the comp using the link must still decline persistent caching");
}

#[test]
fn named_and_computed_expression_reads_keep_linked_frames_uncached() {
    for text in ["footage(\"Linked\").width", "footage(\"Link\" + \"ed\").width"] {
        let (source, source_id, _) = scene();
        let (mut destination, comp_id, comp) = scene();
        destination.add_item("Linked", Label::Brown, None, ItemKind::Footage(linked(Arc::new(source), source_id)));
        let mut fill = super::solid(&mut destination, &comp, [1.0, 0.0, 0.0], 16, 16);
        fill.props.prop_mut("transform/opacity").unwrap().expr = Some(Expression { text: text.into(), enabled: true });
        destination.comp_mut(comp_id).unwrap().layers.push(fill);
        let fill = &destination.comp(comp_id).unwrap().layers[0];
        let ctx = EvalCtx::new(&destination, comp_id, destination.comp(comp_id).unwrap(), Tick::ZERO);
        assert!(crate::cache::input_key(&ctx, fill, 1.0, false, false, 0).is_none(), "{text}");
        let log = ReadLog::new();
        let mut renderer = Renderer::new(&destination, &NoFootage, RenderOpts::default());
        renderer.reads = Some(&log);
        renderer.comp_frame(comp_id, Tick::ZERO);
        assert!(log.files().is_unkeyed(), "expression-reachable links cannot enter the persistent cache: {text}");
    }
}

impl FootageSource for Sound {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        None
    }
    fn audio(&self, _: ItemId, f: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
        assert_eq!(f.path, "source.wav", "linked audio is rendered from its hidden source items");
        Some(vec![0.25; frames * 2])
    }
}

#[test]
fn linked_audio_is_mixed_from_source_and_stops_at_source_duration() {
    let (mut source, source_id, source_comp) = scene();
    let item = source.add_item(
        "Sound",
        Label::Sandstone,
        None,
        ItemKind::Footage(Footage { kind: FootageKind::Audio, path: "source.wav".into(), has_audio: true, ..Default::default() }),
    );
    let mut sound = build::layer(&mut source, &source_comp, "Sound", LayerSource::Footage { item }, (0, 0), None);
    sound.out_point = Tick::from_seconds_f64(2.0);
    source.comp_mut(source_id).unwrap().layers.push(sound);
    let (mut destination, comp_id, comp) = scene();
    let item = destination.add_item("Linked", Label::Sandstone, None, ItemKind::Footage(Footage { has_audio: true, ..linked(Arc::new(source), source_id) }));
    let mut instance = build::layer(&mut destination, &comp, "Linked", LayerSource::Footage { item }, (16, 16), None);
    instance.out_point = Tick::from_seconds_f64(2.0);
    destination.comp_mut(comp_id).unwrap().layers.push(instance);
    let active = audio::mix_comp(&destination, &Sound, None, comp_id, Tick::ZERO, 80, 8000);
    let after_end = audio::mix_comp(&destination, &Sound, None, comp_id, Tick::from_seconds_f64(1.0), 80, 8000);
    assert!(active.iter().any(|sample| sample.abs() > 0.0));
    assert!(after_end.iter().all(|sample| sample.abs() < 1e-6));
}

struct WorkingPicture(Arc<Image>);

impl FootageSource for WorkingPicture {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<Image>> {
        Some(self.0.clone())
    }
}

fn colored_source(space: ColorSpace, color: [f32; 3]) -> (Arc<Project>, ItemId, [f32; 4]) {
    let (mut source, id, comp) = scene();
    source.settings.working_space = Some(space);
    source.settings.color_engine = if space == ColorSpace::AcesCg { effectcraft_project::ColorEngine::Ocio } else { effectcraft_project::ColorEngine::Adobe };
    source.settings.output_space = Some(ColorSpace::DisplayP3);
    source.settings.hdr = effectcraft_project::HdrMode::ToneMap;
    let mut fill = super::solid(&mut source, &comp, color, 16, 16);
    fill.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(50.0);
    source.comp_mut(id).unwrap().layers.push(fill);
    (Arc::new(source), id, [color[0] * 0.5, color[1] * 0.5, color[2] * 0.5, 0.5])
}

fn color_destination(source: Arc<Project>, source_comp: ItemId, space: ColorSpace, opacity: f64) -> (Project, ItemId, ItemId) {
    let (mut destination, id, comp) = scene();
    destination.settings.working_space = Some(space);
    destination.settings.color_engine =
        if space == ColorSpace::AcesCg { effectcraft_project::ColorEngine::Ocio } else { effectcraft_project::ColorEngine::Adobe };
    let mut footage = linked(source.clone(), source_comp);
    footage.linked_comp.as_mut().unwrap().source_color = Some(source.settings.linked_color_space());
    let item = destination.add_item("Linked", Label::Sandstone, None, ItemKind::Footage(footage));
    let mut instance = build::layer(&mut destination, &comp, "Linked", LayerSource::Footage { item }, (16, 16), None);
    instance.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(opacity);
    destination.comp_mut(id).unwrap().layers.push(instance);
    (destination, id, item)
}

/// The reference is ordinary profile-tagged float footage of the source's known working
/// picture. It exercises the existing interpretation pipeline, independently of link rendering.
fn rendered_picture_reference(project: &Project, comp: ItemId, item: ItemId, pixel: [f32; 4], working: bool) -> Image {
    let mut reference = project.clone();
    let ItemKind::Footage(footage) = &mut reference.item_mut(item).unwrap().kind else { panic!("linked footage") };
    let tag = footage.linked_color_space().unwrap();
    footage.kind = FootageKind::Still;
    footage.path = "working-picture.png".into();
    footage.linked_comp = None;
    footage.color_profile = Some(tag.space);
    footage.linear_light = tag.linear;
    let picture = WorkingPicture(Arc::new(Image::filled(16, 16, pixel)));
    let renderer = Renderer::new(&reference, &picture, Default::default());
    if working { renderer.comp_frame_working(comp, Tick::ZERO) } else { renderer.comp_frame(comp, Tick::ZERO) }
}

fn assert_picture_matches(actual: &Image, reference: &Image) {
    assert_eq!((actual.width, actual.height), (reference.width, reference.height));
    let error = actual.data.iter().zip(&reference.data).flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs())).fold(0.0f32, f32::max);
    assert!(error < 1e-5, "linked picture differs from equivalent tagged footage by {error}");
}

#[test]
fn linked_color_converts_between_builtin_and_ocio_without_baking_the_source_view() {
    for (source_space, destination_space) in [(ColorSpace::Srgb, ColorSpace::AcesCg), (ColorSpace::AcesCg, ColorSpace::DisplayP3)] {
        let (source, source_comp, pixel) = colored_source(source_space, [0.18, 0.3, 0.48]);
        let (mut destination, comp, item) = color_destination(source.clone(), source_comp, destination_space, 75.0);
        let actual = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
        assert_picture_matches(&actual, &rendered_picture_reference(&destination, comp, item, pixel, false));
        assert!(actual.get(8, 8)[3] > 0.0 && actual.get(8, 8)[3] < pixel[3], "local opacity and source alpha survive conversion");

        let mut changed_view = (*source).clone();
        changed_view.settings.output_space = Some(ColorSpace::Rec2020);
        changed_view.settings.hdr = effectcraft_project::HdrMode::Compand;
        let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
        f.linked_comp.as_mut().unwrap().resolved = Some(Arc::new(changed_view));
        let after = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
        assert_eq!(after.data, actual.data, "only the destination's output/view transform applies");
    }
}

#[test]
fn linked_color_assignment_overrides_the_auto_tag_and_can_return_to_auto() {
    let (source, source_comp, pixel) = colored_source(ColorSpace::AcesCg, [0.18, 0.3, 0.48]);
    let (mut destination, comp, item) = color_destination(source, source_comp, ColorSpace::Srgb, 100.0);
    let auto = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
    let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
    f.color_profile = Some(ColorSpace::Srgb);
    f.linear_light = false;
    let assigned = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
    assert_ne!(assigned.data, auto.data, "assignment changes interpretation of incoming numbers");
    assert_picture_matches(&assigned, &rendered_picture_reference(&destination, comp, item, pixel, false));

    let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
    f.linear_light = true;
    let assigned_linear = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
    assert_ne!(assigned_linear.data, assigned.data, "the assigned profile's linear-light override changes the input transfer");
    assert_picture_matches(&assigned_linear, &rendered_picture_reference(&destination, comp, item, pixel, false));

    let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
    f.color_profile = None;
    let restored = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
    assert_eq!(restored.data, auto.data, "automatic mode follows the source tag again");

    let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
    f.preserve_rgb = true;
    let preserved = Renderer::new(&destination, &NoFootage, Default::default()).comp_frame(comp, Tick::ZERO);
    assert_picture_matches(&preserved, &rendered_picture_reference(&destination, comp, item, pixel, false));
    assert_ne!(preserved.data, auto.data, "Preserve RGB bypasses the source-space conversion");
}

#[test]
fn linked_color_keeps_hdr_negative_rgb_and_premultiplied_alpha_in_working_pixels() {
    let (source, source_comp, pixel) = colored_source(ColorSpace::AcesCg, [2.0, 0.05, 0.2]);
    let (mut destination, comp, item) = color_destination(source, source_comp, ColorSpace::Srgb, 100.0);
    destination.settings.linearize = true;
    let renderer = Renderer::new(&destination, &NoFootage, Default::default());
    let actual = renderer.comp_frame_working(comp, Tick::ZERO);
    assert_picture_matches(&actual, &rendered_picture_reference(&destination, comp, item, pixel, true));
    let p = actual.get(8, 8);
    assert!(p[..3].iter().any(|v| *v > p[3]), "HDR values are not clipped at the link boundary");
    assert!(p[..3].iter().any(|v| *v < 0.0), "out-of-gamut negative RGB survives the matrix transform");
    assert!((p[3] - pixel[3]).abs() < 1e-6, "conversion preserves alpha rather than converting premultiplied RGB as straight");
}

#[test]
fn linked_footage_preview_uses_destination_interpretation_and_output_once() {
    let (source, source_comp, _) = colored_source(ColorSpace::AcesCg, [0.18, 0.3, 0.48]);
    let (mut destination, comp, item) = color_destination(source, source_comp, ColorSpace::Srgb, 100.0);
    destination.settings.hdr = effectcraft_project::HdrMode::ToneMap;
    destination.settings.output_space = Some(ColorSpace::Rec709);
    for assigned in [None, Some(ColorSpace::DisplayP3)] {
        let ItemKind::Footage(f) = &mut destination.item_mut(item).unwrap().kind else { panic!("linked footage") };
        f.color_profile = assigned;
        for scale in [1.0, 0.5] {
            let renderer = Renderer::new(&destination, &NoFootage, RenderOpts { scale, ..Default::default() });
            let ItemKind::Footage(f) = &destination.item(item).unwrap().kind else { panic!("linked footage") };
            let preview = renderer.linked_footage_preview(f, Tick::ZERO).unwrap();
            let frame = renderer.comp_frame(comp, Tick::ZERO);
            assert_picture_matches(&preview, &frame);
        }
    }
}
