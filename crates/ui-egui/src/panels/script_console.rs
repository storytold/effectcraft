//! Window ▸ Script Console: a JavaScript REPL over the After Effects-style scripting object model
//! (`app.project`, comps, layers, properties…). Each run goes through the `script.run` command
//! in the console's persistent context, so variables survive between runs; edits are undoable.

use crate::i18n::tr;
use egui::{Align2, Rect, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::theme::Tokens;

/// One line of console output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConsoleLine {
    pub text: String,
    /// `input`, `output`, `result` or `error`.
    pub kind: String,
}

/// Script Console state (serde, so agents can read it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptConsole {
    pub input: String,
    pub log: Vec<ConsoleLine>,
    /// Previously run inputs (Up/Down recall).
    #[serde(default)]
    pub history: Vec<String>,
}

impl ScriptConsole {
    fn push(&mut self, kind: &str, text: &str) {
        for l in text.lines() {
            self.log.push(ConsoleLine { text: l.to_string(), kind: kind.to_string() });
        }
        if self.log.len() > 2000 {
            let extra = self.log.len() - 2000;
            self.log.drain(..extra);
        }
    }

    /// Record a `script.run` result.
    pub fn record(&mut self, code: &str, r: &Result<Value, String>) {
        self.push("input", &format!("> {}", code.trim()));
        match r {
            Ok(v) => {
                if let Some(o) = v["output"].as_str().filter(|o| !o.is_empty()) {
                    self.push("output", o);
                }
                if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
                    let line = e["line"].as_u64().map(|l| format!("line {l}: ")).unwrap_or_default();
                    self.push("error", &format!("{line}{}", e["message"].as_str().unwrap_or("error")));
                } else if !v["result"].is_null() {
                    self.push("result", &v["result"].to_string());
                }
            }
            Err(e) => self.push("error", e),
        }
    }
}

/// Run the console input.
pub fn run(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let code = app.ui.script_console.input.trim().to_string();
    if code.is_empty() {
        return;
    }
    let r = crate::menus::invoke(app, ctx, "script.run", json!({"code": code, "name": "Script Console", "console": true}));
    let con = &mut app.ui.script_console;
    con.record(&code, &r);
    if con.history.last() != Some(&code) {
        con.history.push(code);
    }
    con.input.clear();
}

/// The Script Console panel: the output log (newest lines at the bottom, coloured by kind), the
/// multi-line script input and Run/Clear. Run — the button or ⌘/Ctrl+Enter while typing — hands the
/// input to the script runner; Clear only empties the log. The input text and log are UI state.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let input_h = 86.0;
    let bar_h = 26.0;
    let out_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - input_h - bar_h));
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(out_rect, 0.0, t.field_bg);
    // Output, newest at the bottom.
    let line_h = 16.0;
    let visible = ((out_rect.height() - 8.0) / line_h).max(1.0) as usize;
    let log = &app.ui.script_console.log;
    let start = log.len().saturating_sub(visible);
    for (i, l) in log[start..].iter().enumerate() {
        let color = match l.kind.as_str() {
            "error" => t.danger,
            "input" => t.text_faint,
            "result" => t.accent,
            _ => t.text,
        };
        p.text(pos2(out_rect.min.x + 8.0, out_rect.min.y + 4.0 + i as f32 * line_h), Align2::LEFT_TOP, &l.text, Tokens::mono(11.5), color);
    }
    if log.is_empty() {
        p.text(
            out_rect.center(),
            Align2::CENTER_CENTER,
            "JavaScript with the After Effects scripting object model — try app.project.numItems",
            Tokens::ui(12.0),
            t.text_faint,
        );
    }
    app.auto.add("scriptConsole.output", out_rect, "Script Console output");
    // Input.
    let in_rect = Rect::from_min_size(pos2(rect.min.x + 4.0, out_rect.max.y + 4.0), vec2(rect.width() - 8.0, input_h - 8.0));
    let mut run_now = false;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(in_rect));
    let te = egui::TextEdit::multiline(&mut app.ui.script_console.input)
        .id(egui::Id::unique("script-console-input"))
        .font(Tokens::mono(12.0))
        .desired_width(in_rect.width())
        .desired_rows(4)
        .hint_text(tr("Script… (⌘/Ctrl+Enter runs)"));
    let resp = child.add_sized(in_rect.size(), te);
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
        run_now = true;
    }
    app.auto.add("scriptConsole.input", in_rect, "Script Console input");
    // Buttons.
    let bar = Rect::from_min_size(pos2(rect.min.x, rect.max.y - bar_h), vec2(rect.width(), bar_h));
    let run_r = Rect::from_min_size(pos2(bar.max.x - 64.0, bar.min.y + 2.0), vec2(56.0, 20.0));
    let clear_r = Rect::from_min_size(pos2(bar.max.x - 128.0, bar.min.y + 2.0), vec2(56.0, 20.0));
    let run_resp = ui.interact(run_r, egui::Id::unique("script-console-run"), Sense::click());
    p.rect_filled(run_r, 3.0, if run_resp.hovered() { t.accent_hover } else { t.accent });
    p.text(run_r.center(), Align2::CENTER_CENTER, tr("Run"), Tokens::ui(12.0), egui::Color32::WHITE);
    let clear_resp = ui.interact(clear_r, egui::Id::unique("script-console-clear"), Sense::click());
    p.rect_stroke(clear_r, 3.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    p.text(clear_r.center(), Align2::CENTER_CENTER, tr("Clear"), Tokens::ui(12.0), t.text);
    app.auto.add("scriptConsole.run", run_r, "Run");
    app.auto.add("scriptConsole.clear", clear_r, "Clear");
    if run_resp.clicked() {
        run_now = true;
    }
    if clear_resp.clicked() {
        app.ui.script_console.log.clear();
    }
    if run_now {
        let ctx = ui.ctx().clone();
        run(app, &ctx);
    }
}
