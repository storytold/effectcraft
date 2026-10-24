//! The split locked viewer re-converts its pixels when the display profile or the output
//! simulation changes (egui_kittest, texture uploads captured).

use effectcraft_engine::{Session, render::RenderOpts, viewer::DisplayColor};
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::{Harness, TestRenderer};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

type Images = Rc<RefCell<HashMap<egui::TextureId, egui::ColorImage>>>;

struct Capture(Images);

impl TestRenderer for Capture {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        for (id, ops) in std::mem::take(&mut delta.set) {
            for op in ops {
                let egui::ImageData::Color(image) = op.image;
                if op.pos.is_none() && image.size[0] <= 256 && image.size[1] <= 256 {
                    self.0.borrow_mut().insert(id, (*image).clone());
                }
            }
        }
        for id in std::mem::take(&mut delta.free) {
            self.0.borrow_mut().remove(&id);
        }
    }

    fn render(&mut self, _: &egui::Context, _: &egui::FullOutput) -> Result<image::RgbaImage, String> {
        Err("Texture inspection only".into())
    }
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, command: &str, params: Value) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, command, params).unwrap();
    h.run_steps(3);
}

fn locked_center(h: &Harness<'_, EffectcraftApp>, images: &Images) -> [u8; 4] {
    let (_, texture) = h.ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(egui::Id::new("viewer-locked"))).unwrap();
    let images = images.borrow();
    let image = images.get(&texture.id()).unwrap();
    image.pixels[(image.size[1] / 2) * image.size[0] + image.size[0] / 2].to_array()
}

fn expected_center(h: &mut Harness<'_, EffectcraftApp>) -> [u8; 4] {
    let s = &mut h.state_mut().session;
    let comp = s.active_comp_id().unwrap();
    let image = s.render(comp, s.time(), RenderOpts::default());
    let ci = effectcraft_ui_egui::frames::to_color_image(&image);
    let mut pixels = vec![ci.pixels[(ci.size[1] / 2) * ci.size[0] + ci.size[0] / 2].to_array()];
    DisplayColor::of(s).unwrap().apply(&mut pixels);
    pixels[0]
}

#[test]
fn locked_viewer_follows_display_profile_and_simulation() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Color", "width": 64, "height": 64, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 64, "height": 64})).unwrap();
    s.execute("file.projectSettings", json!({"workingSpace": "srgb", "outputSpace": "rec2020", "renderer": "software"})).unwrap();
    s.execute("prefs.set", json!({"key": "previews.displayProfile", "value": "srgb"})).unwrap();
    s.execute("view.displayColorManagement", json!({"value": true})).unwrap();
    let images = Images::default();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).renderer(Capture(images.clone())).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    invoke(&mut h, "view.splitLockedViewer", json!({}));
    let first = expected_center(&mut h);
    assert_eq!(locked_center(&h, &images), first);

    invoke(&mut h, "prefs.set", json!({"key": "previews.displayProfile", "value": "p3"}));
    let after_display = expected_center(&mut h);
    assert_ne!(after_display, first);
    assert_eq!(locked_center(&h, &images), after_display);

    invoke(&mut h, "view.simulateOutput", json!({"profile": "rec709", "preserveRgb": true}));
    let after_sim = expected_center(&mut h);
    assert_ne!(after_sim, after_display);
    assert_eq!(locked_center(&h, &images), after_sim);
}
