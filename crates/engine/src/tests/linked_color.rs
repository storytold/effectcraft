//! Saved linked projects retain source colour interpretation across managed project boundaries.

use std::path::PathBuf;
use std::sync::Arc;

use effectcraft_project::{BitDepth, ColorEngine, ColorSpace, Footage, HdrMode, ItemId, ItemKind, LinkedColorSpace, ProjectSettings};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;

struct Fixture {
    source: Session,
    destination: Session,
    source_path: PathBuf,
    destination_path: PathBuf,
    main: ItemId,
    linked: ItemId,
}

fn settings(engine: ColorEngine) -> ProjectSettings {
    ProjectSettings {
        color_engine: engine,
        working_space: Some(if engine == ColorEngine::Ocio { ColorSpace::AcesCg } else { ColorSpace::DisplayP3 }),
        bit_depth: BitDepth::Bpc32,
        output_space: Some(ColorSpace::Rec709),
        hdr: HdrMode::ToneMap,
        ..Default::default()
    }
}

fn fixture(name: &str, source_engine: ColorEngine, destination_engine: ColorEngine) -> Fixture {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    let source_path = directory.join("Source.ecproj");
    let destination_path = directory.join("Destination.ecproj");
    let mut source = Session::default();
    Arc::make_mut(&mut source.project).settings = settings(source_engine);
    source.execute("comp.new", json!({"name":"Source", "width":16,"height":16,"duration":2})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#805533"})).unwrap();
    source.execute("file.saveAs", json!({"path":source_path.to_string_lossy()})).unwrap();
    let mut destination = Session::default();
    Arc::make_mut(&mut destination.project).settings = settings(destination_engine);
    destination.execute("comp.new", json!({"name":"Destination", "width":32,"height":32,"duration":2})).unwrap();
    let main = destination.active_comp_id().unwrap();
    let linked = destination.execute("file.linkComposition", json!({"path":source_path.to_string_lossy(),"comp":"Source"})).unwrap();
    let linked = ItemId(linked["item"].as_u64().unwrap());
    let layer = destination.execute("layer.addItem", json!({"item":linked.0})).unwrap()["layer"].as_u64().unwrap();
    destination.execute("prop.set", json!({"layer":layer,"path":"transform/opacity","value":75})).unwrap();
    Fixture { source, destination, source_path, destination_path, main, linked }
}

fn footage(s: &Session, item: ItemId) -> &Footage {
    let ItemKind::Footage(f) = &s.project.item(item).unwrap().kind else { panic!("linked footage") };
    f
}

fn interpret(f: &mut Fixture, params: Value) {
    let mut params = params;
    params["item"] = json!(f.linked.0);
    f.destination.execute("file.interpretFootage", params).unwrap();
}

fn pixels(f: &Fixture) -> Vec<[f32; 4]> {
    f.destination.render(f.main, Tick::ZERO, Default::default()).data
}

#[test]
fn linked_color_automatic_tag_crosses_managed_engines_without_baking_source_output() {
    for (name, source_engine, destination_engine) in
        [("linked-color-managed-to-ocio", ColorEngine::Adobe, ColorEngine::Ocio), ("linked-color-ocio-to-managed", ColorEngine::Ocio, ColorEngine::Adobe)]
    {
        let mut f = fixture(name, source_engine, destination_engine);
        let automatic = f.source.project.settings.linked_color_space();
        let link = footage(&f.destination, f.linked);
        assert_eq!(link.linked_comp.as_ref().unwrap().source_color, Some(automatic));
        assert_eq!(link.linked_color_space(), Some(automatic));
        assert!(link.color_profile.is_none(), "the source tag is automatic, not a destination override");
        let before = pixels(&f);
        assert!(before.iter().any(|p| p[3] > 0.0), "the mismatched engines render instead of rejecting the link");
        assert!(before.iter().flatten().all(|v| v.is_finite()));
        let layers = f.destination.project.comp(f.main).unwrap().layers.clone();
        let source_settings = &mut Arc::make_mut(&mut f.source.project).settings;
        source_settings.output_space = Some(ColorSpace::DisplayP3);
        source_settings.hdr = HdrMode::Clip;
        f.source.execute("file.save", json!({})).unwrap();
        f.destination.execute("file.reloadFootage", json!({"item":f.linked.0})).unwrap();
        assert_eq!(pixels(&f), before, "source output/view settings do not bake a second transform into the link");
        assert_eq!(f.destination.project.comp(f.main).unwrap().layers, layers);
    }
}

#[test]
fn linked_color_interpretation_override_survives_save_reload_and_source_retagging() {
    let mut f = fixture("linked-color-override", ColorEngine::Ocio, ColorEngine::Adobe);
    let source_bytes = std::fs::read(&f.source_path).unwrap();
    let layers = f.destination.project.comp(f.main).unwrap().layers.clone();
    let automatic = pixels(&f);
    interpret(&mut f, json!({"colorProfile":"srgb","linearLight":false}));
    let assigned = LinkedColorSpace { space: ColorSpace::Srgb, linear: false };
    assert_eq!(footage(&f.destination, f.linked).linked_color_space(), Some(assigned));
    assert_ne!(pixels(&f), automatic, "assigned profile changes what incoming values mean");
    assert_eq!(std::fs::read(&f.source_path).unwrap(), source_bytes, "interpretation is owned by the destination");
    f.destination.execute("file.saveAs", json!({"path":f.destination_path.to_string_lossy()})).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":f.destination_path.to_string_lossy()})).unwrap();
    f.destination = reopened;
    assert_eq!(footage(&f.destination, f.linked).linked_color_space(), Some(assigned));

    Arc::make_mut(&mut f.source.project).settings.working_space = Some(ColorSpace::Aces2065);
    f.source.execute("file.save", json!({})).unwrap();
    f.destination.execute("file.reloadFootage", json!({"item":f.linked.0})).unwrap();
    let linked = footage(&f.destination, f.linked);
    assert_eq!(linked.linked_comp.as_ref().unwrap().source_color.unwrap().space, ColorSpace::Aces2065);
    assert_eq!(linked.linked_color_space(), Some(assigned), "refresh updates automatic metadata while keeping the assigned override");
    assert_eq!(f.destination.project.comp(f.main).unwrap().layers, layers);
    interpret(&mut f, json!({"colorProfile":"auto"}));
    assert!(footage(&f.destination, f.linked).color_profile.is_none());
    assert_eq!(footage(&f.destination, f.linked).linked_color_space().unwrap().space, ColorSpace::Aces2065);
    Arc::make_mut(&mut f.source.project).settings.working_space = Some(ColorSpace::AcesCg);
    f.source.execute("file.save", json!({})).unwrap();
    f.destination.execute("file.reloadFootage", json!({"item":f.linked.0})).unwrap();
    assert_eq!(
        footage(&f.destination, f.linked).linked_color_space(),
        Some(f.source.project.settings.linked_color_space()),
        "automatic mode follows subsequent saved source changes"
    );
}

#[test]
fn linked_color_controls_change_interpretation_but_other_footage_controls_are_rejected() {
    let mut f = fixture("linked-color-controls", ColorEngine::Ocio, ColorEngine::Adobe);
    interpret(&mut f, json!({"colorProfile":"srgb","linearLight":false}));
    let encoded = pixels(&f);
    interpret(&mut f, json!({"linearLight":true}));
    assert!(footage(&f.destination, f.linked).linked_color_space().unwrap().linear);
    assert_ne!(pixels(&f), encoded, "linear and encoded RGB receive different conversions");
    let converted = pixels(&f);
    interpret(&mut f, json!({"preserveRgb":true}));
    assert!(footage(&f.destination, f.linked).preserve_rgb);
    assert_ne!(pixels(&f), converted, "preserve RGB passes source values without the profile conversion");
    for params in [json!({"frameRate":24}), json!({"alpha":"ignore"})] {
        let before = f.destination.project.clone();
        let mut params = params;
        params["item"] = json!(f.linked.0);
        assert!(f.destination.execute("file.interpretFootage", params).is_err());
        assert!(Arc::ptr_eq(&before, &f.destination.project), "unsupported interpretation does not partly modify the link");
    }
}

#[test]
fn linked_sources_refuse_interpretation_copy_and_paste_without_changing_other_targets_or_the_clipboard() {
    use effectcraft_project::{AlphaMode, FootageKind};
    use effectcraft_time::FrameRate;

    let mut f = fixture("linked-color-copy-guard", ColorEngine::Adobe, ColorEngine::Ocio);
    let ordinary = Arc::make_mut(&mut f.destination.project).add_item(
        "Ordinary",
        Default::default(),
        None,
        ItemKind::Footage(Footage {
            kind: FootageKind::Video,
            alpha: AlphaMode::Ignore,
            frame_rate: FrameRate::new(24, 1),
            native_rate: Some(FrameRate::new(30, 1)),
            loop_count: 3,
            pixel_aspect: 2.0,
            color_profile: Some(ColorSpace::Rec709),
            ..Default::default()
        }),
    );
    f.destination.execute_checked("file.rememberInterpretation", json!({"item":ordinary.0})).unwrap();
    let clipboard = f.destination.state.interpretation.clone();
    let before = f.destination.project.clone();
    let undo = f.destination.history.undo.len();
    let revision = f.destination.revision;
    let linked_layer = f.destination.project.comp(f.main).unwrap().layers[0].id;
    for (selection, timeline, params, checked) in [
        (vec![f.linked], false, json!({}), true),
        (vec![ordinary, f.linked], false, json!({}), true),
        (vec![f.linked, ordinary], false, json!({}), true),
        (vec![ordinary], false, json!({"item":f.linked.0}), true),
        (vec![ordinary], false, json!({"items":[ordinary.0,f.linked.0]}), false),
        (vec![], true, json!({}), true),
    ] {
        f.destination.state.project_selection = selection;
        f.destination.state.selected_layers = if timeline { vec![linked_layer] } else { vec![] };
        for command in ["file.applyInterpretation", "file.rememberInterpretation"] {
            if params.as_object().is_some_and(|params| params.is_empty()) {
                assert!(!f.destination.is_enabled(command), "linked selections disable {command}");
            }
            let error =
                if checked { f.destination.execute_checked(command, params.clone()) } else { f.destination.execute(command, params.clone()) }.unwrap_err();
            assert!(error.to_string().contains("Linked sources"), "{command}: {error}");
            assert!(Arc::ptr_eq(&before, &f.destination.project), "a mixed selection is refused atomically");
            assert_eq!(f.destination.state.interpretation, clipboard, "a refused copy leaves the ordinary footage clipboard intact");
            assert_eq!(f.destination.history.undo.len(), undo);
            assert_eq!(f.destination.revision, revision);
        }
    }
}
