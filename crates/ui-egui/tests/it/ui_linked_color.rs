//! Linked footage exposes automatic source color and destination interpretation in the UI.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ColorSpace, Footage, ItemId, ItemKind};
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::{Dialog, EffectcraftApp, menus};
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let element = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    let point = pos2(element.rect[0] + element.rect[2] / 2.0, element.rect[1] + element.rect[3] / 2.0);
    h.input_mut().events.push(Event::PointerMoved(point));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: point, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn open(h: &mut Harness<'_, EffectcraftApp>) {
    let context = h.ctx.clone();
    menus::invoke(h.state_mut(), &context, "file.interpretFootage", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Form));
}

fn assign(h: &mut Harness<'_, EffectcraftApp>, profile: &str) {
    click(h, "form.field.colorProfile");
    h.get_by_label(profile).click();
    h.run_steps(3);
}

fn choose(h: &mut Harness<'_, EffectcraftApp>, field: &str, label: &str) {
    click(h, &format!("form.field.{field}"));
    h.query_all_by_label(label).last().expect("the open choice offers the requested value").click();
    h.run_steps(3);
}

fn linked_source(settings: effectcraft_engine::project::ProjectSettings) -> Footage {
    use effectcraft_engine::project::LinkedComposition;
    let mut source = Session::default();
    std::sync::Arc::make_mut(&mut source.project).settings = settings;
    source.execute("comp.new", json!({"name":"Source","width":16,"height":16,"duration":2})).unwrap();
    Footage {
        path: "target/test-out/ui-linked-color-model/Source.ecproj".into(),
        kind: effectcraft_engine::project::FootageKind::LinkedComposition,
        width: 16,
        height: 16,
        duration: Tick::from_seconds_f64(2.0),
        has_video: true,
        linked_comp: Some(LinkedComposition {
            source_project_id: source.project.project_id.clone(),
            composition_id: source.project.item(source.active_comp_id().unwrap()).unwrap().stable_id.clone(),
            last_known_name: "Source".into(),
            source_color: Some(source.project.settings.linked_color_space()),
            resolved: Some(source.project.clone()),
            resolved_comp: source.active_comp_id(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn footage<'a>(h: &'a Harness<'_, EffectcraftApp>, id: ItemId) -> &'a Footage {
    let ItemKind::Footage(f) = &h.state().session.project.item(id).unwrap().kind else { panic!("linked footage") };
    f
}

#[test]
fn interpret_linked_color_assigns_and_resets_a_profile_across_refresh() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/ui-linked-color");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("Color Source.ecproj");
    let mut source = Session::default();
    source.execute("file.projectSettings", json!({"colorEngine":"ocio", "workingSpace":"acescg", "bitDepth":32})).unwrap();
    source.execute("comp.new", json!({"name":"Source", "width":16, "height":16, "duration":1})).unwrap();
    source.execute("layer.newSolid", json!({"color":"#406080"})).unwrap();
    source.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();

    let mut destination = Session::default();
    destination.execute("file.projectSettings", json!({"workingSpace":"rec709", "bitDepth":32})).unwrap();
    destination.execute("comp.new", json!({"name":"Destination", "width":64, "height":64, "duration":1})).unwrap();
    let main = destination.active_comp_id().unwrap();
    let link = destination.execute("file.linkComposition", json!({"path":path.to_string_lossy(),"comp":"Source"})).unwrap();
    let item = ItemId(link["item"].as_u64().unwrap());
    destination.execute("layer.addItem", json!({"item":item.0})).unwrap();
    let before = destination.render(main, Tick::ZERO, Default::default());
    let layers = destination.project.comp(main).unwrap().layers.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(destination));
    h.run_steps(3);

    open(&mut h);
    assert!(h.query_by_label_contains("Automatic source color space: ACEScg").is_some());
    assert!(h.state().auto.find("form.field.frameRate").is_none(), "source timing is not submitted by the color dialog");
    let linear = h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Interpret As Linear Light");
    assert_eq!(linear.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    // Merely accepting the automatic tag must not make it a fixed assignment.
    click(&mut h, "form.ok");
    assert!(footage(&h, item).color_profile.is_none());

    open(&mut h);
    assign(&mut h, "sRGB IEC61966-2.1");
    click(&mut h, "form.field.linearLight");
    click(&mut h, "form.ok");
    assert_eq!(footage(&h, item).color_profile, Some(ColorSpace::Srgb));
    assert!(!footage(&h, item).linear_light);
    assert_ne!(h.state().session.render(main, Tick::ZERO, Default::default()).data, before.data);
    assert_eq!(h.state().session.project.comp(main).unwrap().layers, layers);

    source.execute("file.projectSettings", json!({"workingSpace":"aces2065"})).unwrap();
    source.execute("file.save", json!({})).unwrap();
    h.state_mut().session.execute("file.reloadFootage", json!({"item":item.0})).unwrap();
    h.run_steps(3);
    assert_eq!(footage(&h, item).color_profile, Some(ColorSpace::Srgb), "source refresh preserves the destination override");
    assert_eq!(footage(&h, item).linked_comp.as_ref().unwrap().source_color.unwrap().space, ColorSpace::Aces2065);
    open(&mut h);
    assign(&mut h, "Automatic source color space");
    click(&mut h, "form.ok");
    assert!(footage(&h, item).color_profile.is_none());
    assert_eq!(footage(&h, item).linked_color_space().unwrap().space, ColorSpace::Aces2065);
    assert!(footage(&h, item).linked_color_space().unwrap().linear);

    // Changing just the encoding of an automatically tagged linearized RGB source should
    // create an override, rather than silently submitting Auto and discarding the edit.
    source.execute("file.projectSettings", json!({"colorEngine":"adobe", "workingSpace":"srgb", "linearize":true})).unwrap();
    source.execute("file.save", json!({})).unwrap();
    h.state_mut().session.execute("file.reloadFootage", json!({"item":item.0})).unwrap();
    h.run_steps(3);
    open(&mut h);
    assign(&mut h, "sRGB IEC61966-2.1");
    click(&mut h, "form.field.linearLight");
    assign(&mut h, "Automatic source color space");
    click(&mut h, "form.ok");
    assert!(footage(&h, item).color_profile.is_none(), "selecting Auto restores source encoding after trying a manual interpretation");
    assert!(footage(&h, item).linked_color_space().unwrap().linear);
    open(&mut h);
    click(&mut h, "form.field.linearLight");
    click(&mut h, "form.ok");
    assert_eq!(footage(&h, item).color_profile, Some(ColorSpace::Srgb));
    assert!(!footage(&h, item).linked_color_space().unwrap().linear);
}

#[test]
fn mixed_linked_interpretation_keeps_individual_settings_and_applies_only_chosen_color_changes() {
    use effectcraft_engine::project::{AlphaMode, FootageKind, ProjectSettings};
    for ordinary_first in [true, false] {
        let mut destination = Session::default();
        destination.execute("comp.new", json!({"name":"Destination","width":64,"height":64,"duration":2})).unwrap();
        let ordinary = Footage {
            path: "target/test-out/ui-linked-color-model/Ordinary.png".into(),
            kind: FootageKind::Still,
            frame_rate: effectcraft_engine::time::FrameRate::new(24, 1),
            width: 16,
            height: 16,
            has_video: true,
            alpha: AlphaMode::Premultiplied,
            pixel_aspect: 1.25,
            color_profile: Some(ColorSpace::DisplayP3),
            linear_light: false,
            preserve_rgb: true,
            ..Default::default()
        };
        let mut linked = linked_source(ProjectSettings::default());
        linked.color_profile = Some(ColorSpace::Rec709);
        linked.linear_light = true;
        let project = std::sync::Arc::make_mut(&mut destination.project);
        let normal = project.add_item("Ordinary", Default::default(), None, ItemKind::Footage(ordinary));
        let external = project.add_item("Linked", Default::default(), None, ItemKind::Footage(linked));
        destination.state.project_selection = if ordinary_first { vec![normal, external] } else { vec![external, normal] };
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(destination));
        h.run_steps(3);
        let before = [footage(&h, normal).clone(), footage(&h, external).clone()];
        let undo = h.state().session.history.undo.len();
        open(&mut h);
        assert!(h.state().auto.find("form.field.frameRate").is_none());
        let profile = h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Assign Profile");
        assert_eq!(profile.accesskit_node().value().as_deref(), Some("Keep individual profiles"));
        click(&mut h, "form.ok");
        assert!(h.state().dialog.is_none());
        assert_eq!([footage(&h, normal).clone(), footage(&h, external).clone()], before, "accepting the shared controls leaves each source unchanged");
        assert_eq!(h.state().session.history.undo.len(), undo);

        open(&mut h);
        assign(&mut h, "sRGB IEC61966-2.1");
        click(&mut h, "form.ok");
        for (item, original) in [normal, external].into_iter().zip(&before) {
            let changed = footage(&h, item);
            assert_eq!(changed.color_profile, Some(ColorSpace::Srgb));
            assert_eq!(
                (changed.linear_light, changed.preserve_rgb),
                (original.linear_light, original.preserve_rgb),
                "profile assignment does not copy another source's other flags"
            );
            assert_eq!((changed.frame_rate, changed.alpha, changed.pixel_aspect), (original.frame_rate, original.alpha, original.pixel_aspect));
        }

        open(&mut h);
        choose(&mut h, "linearLight", "Encoded");
        choose(&mut h, "preserveRgb", "Preserve RGB");
        click(&mut h, "form.ok");
        for item in [normal, external] {
            assert!(!footage(&h, item).linear_light);
            assert!(footage(&h, item).preserve_rgb);
        }
    }
}

#[test]
fn linked_color_note_identifies_unmanaged_assumptions_and_offline_last_known_tags() {
    use effectcraft_engine::project::ProjectSettings;
    let mut destination = Session::default();
    destination.execute("comp.new", json!({"name":"Destination","width":64,"height":64,"duration":2})).unwrap();
    let project = std::sync::Arc::make_mut(&mut destination.project);
    let unmanaged = project.add_item("Unmanaged", Default::default(), None, ItemKind::Footage(linked_source(ProjectSettings::default())));
    let managed = project.add_item(
        "Managed",
        Default::default(),
        None,
        ItemKind::Footage(linked_source(ProjectSettings { working_space: Some(ColorSpace::Srgb), ..Default::default() })),
    );
    let mut offline = linked_source(ProjectSettings::default());
    let record = offline.linked_comp.as_mut().unwrap();
    record.error = Some("Source is unavailable".into());
    record.resolved = None;
    record.resolved_comp = None;
    offline.missing = true;
    let offline = project.add_item("Offline", Default::default(), None, ItemKind::Footage(offline));
    destination.state.project_selection = vec![unmanaged];
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(destination));
    h.run_steps(3);
    open(&mut h);
    assert!(h.query_by_label_contains("Unmanaged source: assuming sRGB").is_some());
    click(&mut h, "form.cancel");
    h.state_mut().session.execute("project.select", json!({"items":[managed.0]})).unwrap();
    h.run_steps(3);
    open(&mut h);
    assert!(h.query_by_label_contains("Automatic source color space: sRGB").is_some());
    assert!(h.query_by_label_contains("Unmanaged source: assuming").is_none());
    click(&mut h, "form.cancel");
    h.state_mut().session.execute("project.select", json!({"items":[offline.0]})).unwrap();
    h.run_steps(3);
    open(&mut h);
    assert!(h.query_by_label_contains("Last known linked color interpretation").is_some());
    assert!(h.query_by_label_contains("The source is offline").is_some());
}
