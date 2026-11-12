//! Motioner: a built-in dockable motion assistant, rendered natively (no webview).
use crate::EffectcraftApp;
use egui::{Color32, Rect, RichText, UiBuilder, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
mod local;
#[cfg(not(target_arch = "wasm32"))]
pub use local::Runtime;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub tab: usize,
    pub prompt: String,
    pub messages: Vec<Message>,
    pub engine: String,
    pub executable: String,
    pub model: String,
    pub settings: bool,
    pub selected: String,
    pub duration: f64,
    pub stagger: f64,
    pub intensity: f64,
    pub mode: String,
    pub search: String,
    pub status: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            tab: 0,
            prompt: String::new(),
            messages: vec![],
            engine: "claude".into(),
            executable: String::new(),
            model: String::new(),
            settings: false,
            selected: "fade".into(),
            duration: 0.7,
            stagger: 0.0,
            intensity: 100.0,
            mode: "entrada".into(),
            search: String::new(),
            status: String::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}
fn button(app: &mut EffectcraftApp, ui: &mut egui::Ui, id: &str, label: &str) -> bool {
    let r = ui.add_sized([ui.available_width(), 36.0], egui::Button::new(label).wrap().corner_radius(7));
    app.auto.add_response(id, &r, label);
    r.clicked()
}
fn execute(app: &mut EffectcraftApp, ui: &egui::Ui, id: &str, p: Value) {
    match crate::menus::invoke(app, ui.ctx(), id, p) {
        Ok(_) => app.ui.motioner.status = "Aplicado à composição. Ctrl/Cmd + Z para desfazer.".into(),
        Err(e) => app.ui.motioner.status = e,
    }
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect.shrink(14.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    ui.set_clip_rect(rect);
    ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("motioner").size(23.0).strong().color(Color32::from_rgb(203, 225, 245)));
        ui.label(RichText::new("MOTION ASSISTANT").size(8.0).color(Color32::GRAY));
        let r = ui.small_button("Ajustes");
        app.auto.add_response("motioner.settings", &r, "Ajustes");
        if r.clicked() {
            app.ui.motioner.settings = !app.ui.motioner.settings;
        }
    });
    let title = app.session.active_comp_id().and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or("Abra uma composição".into());
    egui::Frame::new().fill(Color32::from_rgb(35, 39, 46)).corner_radius(8).inner_margin(10).show(&mut ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(title).size(12.0));
        ui.label(RichText::new(format!("{} camadas selecionadas", app.session.state.selected_layers.len())).size(10.0).color(Color32::GRAY));
    });
    if app.ui.motioner.settings {
        settings(app, &mut ui);
        return;
    }
    ui.horizontal(|ui| {
        for (i, label) in ["Chat", "Animar", "Ferramentas"].iter().enumerate() {
            let r = ui.selectable_label(app.ui.motioner.tab == i, *label);
            app.auto.add_response(&format!("motioner.tab.{i}"), &r, label);
            if r.clicked() {
                app.ui.motioner.tab = i;
                app.ui.motioner.search.clear();
            }
        }
    });
    ui.separator();
    match app.ui.motioner.tab {
        1 => animate(app, &mut ui),
        2 => tools(app, &mut ui),
        _ => chat(app, &mut ui),
    }
    if !app.ui.motioner.status.is_empty() {
        ui.add_space(5.0);
        ui.label(RichText::new(&app.ui.motioner.status).size(10.0).color(Color32::from_rgb(178, 196, 211)));
    }
}
fn settings(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    ui.heading("Conexão local");
    ui.label(RichText::new("Motioner · FelipeDevv").size(10.0).color(Color32::GRAY));
    ui.label("Sem conta do Motioner. Autentique o CLI no terminal.");
    ui.horizontal(|ui| {
        for (id, label) in [("claude", "Claude Code"), ("codex", "Codex")] {
            let r = ui.selectable_value(&mut app.ui.motioner.engine, id.into(), label);
            app.auto.add_response(&format!("motioner.engine.{id}"), &r, label);
        }
    });
    ui.label("Executável (opcional)");
    let r = ui.text_edit_singleline(&mut app.ui.motioner.executable);
    app.auto.add_response("motioner.executable", &r, "Caminho do CLI");
    ui.label("Modelo (opcional)");
    let r = ui.text_edit_singleline(&mut app.ui.motioner.model);
    app.auto.add_response("motioner.model", &r, "Modelo");
    ui.label(
        RichText::new("O pedido e o conteúdo consultado são enviados ao provedor de IA escolhido. Os presets funcionam sem conexão.")
            .size(10.0)
            .color(Color32::GRAY),
    );
    if button(app, ui, "motioner.settings.save", "Salvar preferências") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Err(e) = local::save(app) {
                app.ui.motioner.status = e;
                return;
            }
        }
        app.ui.motioner.settings = false;
    }
}
fn animate(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    ui.label(RichText::new("Encontre o seu ritmo.").size(21.0));
    let r = ui.add(egui::TextEdit::singleline(&mut app.ui.motioner.search).hint_text("Buscar movimento…"));
    app.auto.add_response("motioner.search", &r, "Buscar movimento");
    let presets = app.session.execute("motioner.presets", json!({})).unwrap_or(json!([]));
    egui::ScrollArea::vertical().id_salt("motioner.presets").max_height((ui.available_height() - 195.0).max(60.0)).show(ui, |ui| {
        let width = ((ui.available_width() - 8.0) / 2.0).max(60.0);
        ui.horizontal_wrapped(|ui| {
            for p in presets.as_array().into_iter().flatten() {
                let id = p["id"].as_str().unwrap_or_default();
                let name = p["name"].as_str().unwrap_or_default();
                if !name.to_lowercase().contains(&app.ui.motioner.search.to_lowercase()) {
                    continue;
                }
                let r =
                    ui.add_sized([width, 54.0], egui::Button::new(RichText::new(name).size(11.0)).selected(app.ui.motioner.selected == id).corner_radius(8));
                app.auto.add_response(&format!("motioner.preset.{id}"), &r, name);
                if r.clicked() {
                    app.ui.motioner.selected = id.into();
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("Duração");
        let r = ui.add(egui::DragValue::new(&mut app.ui.motioner.duration).range(0.05..=60.0).speed(0.05).suffix(" s"));
        app.auto.add_response("motioner.duration", &r, "Duração");
        ui.label("Intervalo");
        let r = ui.add(egui::DragValue::new(&mut app.ui.motioner.stagger).range(0.0..=120.0).suffix(" f"));
        app.auto.add_response("motioner.stagger", &r, "Intervalo");
    });
    ui.horizontal(|ui| {
        for (id, label) in [("entrada", "Entrada"), ("saida", "Saída"), ("ambos", "Ambos")] {
            let r = ui.selectable_value(&mut app.ui.motioner.mode, id.into(), label);
            app.auto.add_response(&format!("motioner.mode.{id}"), &r, label);
        }
    });
    let r = ui.add(egui::Slider::new(&mut app.ui.motioner.intensity, 1.0..=300.0).text("Intensidade %"));
    app.auto.add_response("motioner.intensity", &r, "Intensidade");
    if button(app, ui, "motioner.apply", "Aplicar animação →") {
        let s = &app.ui.motioner;
        let p = json!({"preset":s.selected,"duration":s.duration,"stagger":s.stagger,"intensity":s.intensity,"mode":s.mode});
        execute(app, ui, "motioner.animate", p);
    }
}
fn tools(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    ui.label(RichText::new("Pequenos atalhos.").size(21.0));
    let r = ui.add(egui::TextEdit::singleline(&mut app.ui.motioner.search).hint_text("Buscar efeito nativo…"));
    app.auto.add_response("motioner.tools.search", &r, "Buscar efeito");
    let effects = app.session.execute("effect.list", json!({"filter":app.ui.motioner.search})).unwrap_or(json!([]));
    egui::ScrollArea::vertical().id_salt("motioner.tools").show(ui, |ui| {
        for (id, label, cmd, p) in [
            ("text", "Converter PSD em texto editável", "layer.create", json!({"op":"editableText"})),
            ("3d", "Ativar camadas 3D", "layer.setSwitch", json!({"switch":"threeD","value":true})),
            ("camera", "Criar câmera", "layer.newCamera", json!({})),
            ("null", "Criar nulo", "layer.newNull", json!({})),
            ("glow", "Glow", "effect.apply", json!({"effect":"Glow"})),
            ("blur", "Desfoque gaussiano", "effect.apply", json!({"effect":"Gaussian Blur"})),
            ("turbulence", "Turbulência", "effect.apply", json!({"effect":"Turbulent Displace"})),
        ] {
            if button(app, ui, &format!("motioner.tool.{id}"), label) {
                execute(app, ui, cmd, p);
            }
        }
        ui.separator();
        ui.label(RichText::new("Biblioteca de efeitos").strong());
        for effect in effects.as_array().into_iter().flatten() {
            let id = effect["id"].as_str().unwrap_or_default();
            let name = effect["name"].as_str().unwrap_or_default();
            if button(app, ui, &format!("motioner.effect.{id}"), name) {
                execute(app, ui, "effect.apply", json!({"effect":id}));
            }
        }
        ui.label(
            RichText::new("A conversão de texto usa as fontes instaladas. Para preservar o visual do PSD, mantenha a prévia rasterizada.")
                .size(10.0)
                .color(Color32::GRAY),
        );
    });
}
fn chat(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    let busy = app.motioner_runtime.busy();
    #[cfg(target_arch = "wasm32")]
    let busy = false;
    let h = (ui.available_height() - 140.0).max(45.0);
    egui::ScrollArea::vertical().id_salt("motioner.messages").max_height(h).min_scrolled_height(h).stick_to_bottom(true).show(ui, |ui| {
        if app.ui.motioner.messages.is_empty() {
            ui.add_space(14.0);
            ui.label(RichText::new("Vamos dar\nmovimento.").size(30.0).strong());
            ui.label(RichText::new("Uma ideia sua. Keyframes na composição.").color(Color32::GRAY));
            ui.add_space(20.0);
            for (id, label, text) in [
                ("entry", "01  Uma entrada suave", "Anime a entrada das camadas selecionadas usando motioner.animate com preset subida-suave."),
                ("depth", "02  Profundidade à cena", "Leia a composição e crie movimento de câmera com profundidade."),
                ("review", "03  Encontre o próximo passo", "Leia minha composição e sugira melhorias para a animação."),
            ] {
                if button(app, ui, &format!("motioner.suggest.{id}"), label) {
                    app.ui.motioner.prompt = text.into();
                }
            }
        } else {
            for m in &app.ui.motioner.messages {
                ui.label(RichText::new(if m.role == "user" { "VOCÊ" } else { "MOTIONER" }).size(9.0).color(Color32::from_rgb(160, 194, 224)));
                ui.label(&m.text);
                ui.add_space(12.0);
            }
        }
        if busy {
            ui.spinner();
            ui.label("Trabalhando na composição…");
        }
    });
    let r = ui.add_sized([ui.available_width(), 58.0], egui::TextEdit::multiline(&mut app.ui.motioner.prompt).hint_text("O que vamos criar?"));
    app.auto.add_response("motioner.prompt", &r, "Pedido para o assistente");
    if busy {
        if button(app, ui, "motioner.stop", "Interromper") {
            #[cfg(not(target_arch = "wasm32"))]
            app.motioner_runtime.stop();
        }
    } else if button(app, ui, "motioner.send", &format!("Enviar · {} →", if app.ui.motioner.engine == "claude" { "Claude Code" } else { "Codex" })) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(e) = local::send(app, ui.ctx()) {
            app.ui.motioner.status = e;
        }
        #[cfg(target_arch = "wasm32")]
        {
            app.ui.motioner.status = "O assistente local está disponível no aplicativo desktop.".into();
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub fn poll(app: &mut EffectcraftApp, ctx: &egui::Context) {
    local::poll(app, ctx);
}
#[cfg(not(target_arch = "wasm32"))]
pub fn load(app: &mut EffectcraftApp) {
    local::load(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn core_controls_fit_a_narrow_native_panel_and_have_automation_ids() {
        for (width, height) in [(300.0, 460.0), (320.0, 460.0), (420.0, 740.0)] {
            for (tab, id) in [(0, "motioner.send"), (1, "motioner.apply"), (2, "motioner.tool.text")] {
                let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
                app.ui.motioner.tab = tab;
                let ctx = egui::Context::default();
                crate::theme::install(&ctx, &app.tokens, "en");
                let rect = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(rect), ..Default::default() }, |ui| show(&mut app, ui, rect));
                output.textures_delta.clear();
                let element = app.auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
                let [x, y, w, h] = element.rect;
                assert!(x >= 0.0 && y >= 0.0 && x + w <= width + 1.0 && y + h <= height + 1.0, "{id}: {:?} outside {rect:?}", element.rect);
            }
        }
    }
}
