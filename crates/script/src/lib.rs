//! # effectcraft-script (L4)
//!
//! Scripting with an After Effects-style object model, on the boa JavaScript engine (the one the
//! expressions use). Scripts written for After Effects' documented scripting API mostly run
//! unchanged:
//!
//! ```text
//! app.beginUndoGroup("Build");
//! var comp = app.project.items.addComp("Main", 1920, 1080, 1, 10, 30);
//! var solid = comp.layers.addSolid([1, 0, 0], "Red", 1920, 1080, 1);
//! solid.property("ADBE Transform Group").property("ADBE Position").setValueAtTime(0, [0, 540]);
//! solid.Effects.addProperty("ADBE Gaussian Blur 2").property("Blurriness").setValue(20);
//! app.endUndoGroup();
//! ```
//!
//! * **Object model** (`prelude.js`): `app` (project, open/newProject, undo groups,
//!   `executeCommand`/`findMenuCommandId`, `scheduleTask`, `settings` kept in the settings
//!   folder), `Project`, `ItemCollection`,
//!   `CompItem`/`FootageItem`/`FolderItem`, `LayerCollection`, `AVLayer`/`TextLayer`/
//!   `ShapeLayer`/`CameraLayer`/`LightLayer`, `PropertyGroup`/`Property` (values, keyframes,
//!   eases, expressions), `MaskPropertyGroup`, `TextDocument`, `Shape`, `KeyframeEase`,
//!   `MarkerValue`, `RenderQueue`/`RenderQueueItem`/`OutputModule`, `ImportOptions`, `File`/
//!   `Folder`, `$`, `alert`/`writeLn`, and the enums (`BlendingMode`, `KeyframeInterpolationType`,
//!   `TrackMatteType`, `LightType`, `ParagraphJustification`, `PropertyValueType`…).
//! * **Edits are engine commands**: every mutating call runs a command through
//!   [`Session::execute`](effectcraft_engine::Session::execute), so it is undoable, journaled
//!   and identical to the UI's action; `app.beginUndoGroup`/`endUndoGroup` fold everything in
//!   between into one undo step.
//! * **Match names** ([`matchnames`]): After Effects' documented match names for our properties
//!   and effects (`ADBE Transform Group`, `ADBE Position`, `ADBE Gaussian Blur 2`…).
//! * **Security**: scripts can import/open/save projects and render, but `File` reads are limited
//!   to the project's folder and writes (and the network) are off unless Preferences ▸ Scripting
//!   & Expressions ▸ Allow Scripts to Write Files and Access Network is on. A script can't turn
//!   that setting on itself (`prefs.set` refuses it, also inside `engine.batch`); the host must.
//! * **ScriptUI** (`scriptui.js`, `ui.rs`): `Window` (dialog / palette / window), `Panel`,
//!   `Group`, `Button`, `StaticText`, `EditText`, `Checkbox`, `RadioButton`, `Slider`,
//!   `Progressbar`, `DropDownList`, `ListBox`, `TabbedPanel`/`Tab`, with `add()`, `orientation`,
//!   `alignChildren`, `alignment`, `margins`, `spacing`, `preferredSize`, `onClick` /
//!   `onChange` / `onChanging` / `onClose`, `show()` / `close()` and `layout.layout()`. Windows
//!   are published to [`Session::script_ui`](effectcraft_engine::Session::script_ui) for
//!   frontends and agents (`scriptui.*` commands); modal dialogs block `show()` until they close.
//!   Scripts in the ScriptUI Panels folder run with `this` = a dockable `Panel`. Resource
//!   strings (`new Window("dialog { ok: Button { text: 'OK' } }")`, `add("group { … }")`) build
//!   whole trees; `onDraw` handlers paint with `ScriptUIGraphics` (`newPen`/`newBrush`/`rectPath`/
//!   `ellipsePath`/`moveTo`/`lineTo`/`fillPath`/`strokePath`/`drawString`/`measureString`) into a
//!   serde draw list frontends paint; edit texts and sliders fire `onChanging` live.
//! * **Sockets** (`socket.rs`): `Socket` (`open`/`listen`/`poll`/`read`/`readln`/`write`/`close`)
//!   over TCP, behind the same network preference; on the web `open` returns false.
//!
//! Entry points: [`install`] sets [`Session::script`](effectcraft_engine::Session::script), which
//! the `script.run` command, File ▸ Scripts ▸ Run Script File… (`.jsx`/`.js`), the Script Console
//! panel, `effectcraft-cli script` and the MCP `run_script` tool use.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![recursion_limit = "256"]

pub mod matchnames;
mod model;
mod runtime;
mod socket;
mod ui;

use effectcraft_engine::{ScriptRequest, Session};

pub use runtime::{Outcome, ScriptError, run};
pub use ui::dispatch_ui;

/// The [`effectcraft_engine::ScriptRunner`] this crate provides.
pub fn runner(s: &mut Session, req: &ScriptRequest) -> serde_json::Value {
    run(s, req).to_json()
}

/// Enable scripting (and ScriptUI) on a session.
pub fn install(s: &mut Session) {
    s.script = Some(runner);
    s.script_ui.dispatch = Some(dispatch_ui);
}

/// Run `code` as a one-off script named `name`.
pub fn run_code(s: &mut Session, code: &str, name: &str) -> Outcome {
    run(s, &ScriptRequest { code, name, console: false })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ease_presets;
#[cfg(test)]
mod tests_panels;
#[cfg(test)]
mod tests_ui;
#[cfg(test)]
mod tests_ui_more;
