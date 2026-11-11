//! Focus hand-off for EffectCraft. Both directions of `app.activate`.
//!
//! [`EguiActivation`] is the [`ActivationBackend`] for the desktop app. It maps the backend calls
//! to egui viewport commands. [`SinkActivate`] is the server side: it hands an incoming
//! `app.activate` to the UI thread, where [`handle_activate`] applies it.
//!
//! Limits on Linux, both from stock winit 0.30:
//!
//! - Wayland `focus_window()` is empty, and no winit call applies an activation token to a window
//!   that already exists (rust-windowing/winit#3633). [`EguiActivation::can_apply_token`] is
//!   therefore `false`. A token from FilmCraft is ignored. The answer to `app.activate` is `false`
//!   and the window asks for user attention.
//! - eframe can mint a token (`Frame::request_activation_token`, then
//!   `Frame::take_activation_token` on a later frame). Only the UI update has a `Frame`, and the
//!   token has no user until winit can apply one. [`EguiActivation::mint_token`] returns `None`
//!   and `app.activate` goes out with `"token": null`.
//!
//! Nobody has seen focus move on Wayland with this code.

use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

pub use craft_ipc::activate::{ActivationBackend, FocusResult};
use craft_ipc::server::{Activate, Activation, ControlSink};
use effectcraft_engine::Session;
use serde_json::{Value, json};

/// How long the server thread waits for the UI thread to apply `app.activate`.
const UI_REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Sender side token for `app.activate`: [`craft_ipc::activate::mint_token`].
pub fn mint_token(backend: &dyn ActivationBackend) -> Option<String> {
    craft_ipc::activate::mint_token(backend)
}

/// [`ActivationBackend`] over an egui context. Call it on the UI thread.
#[derive(Clone)]
pub struct EguiActivation {
    ctx: egui::Context,
}

impl EguiActivation {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }
}

impl ActivationBackend for EguiActivation {
    /// Always `None`. See the module note: the adapter has no `eframe::Frame`.
    fn mint_token(&self) -> Option<String> {
        None
    }

    /// Always `false`. Stock winit has no call that applies a token to an existing window.
    fn can_apply_token(&self) -> bool {
        false
    }

    /// Never called while `can_apply_token` is `false`.
    fn set_pending_token(&self, _token: &str) {}

    /// Sends `Focus`. A window that is already focused is `Focused`. Otherwise the compositor
    /// decides later, so the answer is `Pending`.
    fn focus(&self) -> FocusResult {
        let already = self.ctx.input(|input| input.viewport().focused) == Some(true);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if already { FocusResult::Focused } else { FocusResult::Pending }
    }

    fn request_user_attention(&self) {
        self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }

    /// macOS hand-off is not wired in this build. `false` makes the caller ask for attention.
    fn yield_activation_to(&self, _bundle_id: &str) -> bool {
        false
    }

    /// macOS hand-off is not wired in this build. `false` makes the caller ask for attention.
    fn activate_application(&self) -> bool {
        false
    }
}

/// The `app.activate` request as the UI thread sees it.
#[derive(Debug, PartialEq)]
pub struct ActivateRequest {
    pub token: Option<String>,
    pub open_link: Option<String>,
}

impl ActivateRequest {
    /// Parse the `app.activate` params. A wrong type is an error, not a default.
    pub fn parse(params: &Value) -> Result<Self, String> {
        let token = match params.get("token") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err("token must be a string or null".into()),
        };
        let open_link = match params.get("open") {
            None | Some(Value::Null) => None,
            Some(Value::Object(open)) => match open.get("link_id") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) => Some(id.clone()),
                Some(_) => return Err("open.link_id must be a string".into()),
            },
            Some(_) => return Err("open must be an object".into()),
        };
        Ok(Self { token, open_link })
    }
}

/// Apply an incoming `app.activate`. Returns the `{"ok", "result"}` reply object.
///
/// `open.link_id` of a bound link opens that comp first. An unknown link id does not fail the
/// activation: the app still comes forward. Bad params are an error reply.
pub fn handle_activate(session: &mut Session, backend: &dyn ActivationBackend, params: &Value) -> Value {
    let request = match ActivateRequest::parse(params) {
        Ok(request) => request,
        Err(message) => return json!({"ok": false, "error": {"code": "bad_params", "message": message, "data": {}}}),
    };
    if let Some(link_id) = &request.open_link {
        let comp = session.project.craft_links.get(link_id).map(|binding| binding.comp);
        if let Some(comp) = comp {
            session.open_comp(comp);
        }
    }
    let activated = craft_ipc::activate::apply_activation(backend, request.token.as_deref());
    json!({"ok": true, "result": {"activated": crate::back::activation_value(activated)}})
}

/// Server side [`Activate`]. It forwards `app.activate` through the app's [`ControlSink`], so the
/// UI thread applies it. A UI thread that does not answer is `No`, never a guess.
pub struct SinkActivate {
    sink: Arc<dyn ControlSink>,
}

impl SinkActivate {
    pub fn new(sink: Arc<dyn ControlSink>) -> Self {
        Self { sink }
    }
}

impl Activate for SinkActivate {
    fn activate(&self, token: Option<String>, from_pid: Option<u32>, open: Option<Value>) -> Activation {
        let params = json!({"token": token, "from_pid": from_pid, "open": open});
        let Some(reply) = self.sink.submit("app.activate".to_string(), params) else {
            log::warn!("app.activate: the UI thread is gone");
            return Activation::No;
        };
        let value = match reply.recv_timeout(UI_REPLY_TIMEOUT) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                log::warn!("app.activate: the UI thread did not answer in {UI_REPLY_TIMEOUT:?}");
                return Activation::No;
            }
            Err(RecvTimeoutError::Disconnected) => {
                log::warn!("app.activate: the UI thread dropped the request");
                return Activation::No;
            }
        };
        match value.pointer("/result/activated") {
            Some(Value::Bool(true)) => Activation::Yes,
            Some(Value::String(text)) if text == "requested" => Activation::Requested,
            Some(Value::Bool(false)) => Activation::No,
            _ => {
                log::warn!("app.activate: the UI thread answered with {value}");
                Activation::No
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use effectcraft_engine::Session;
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemKind};
    use effectcraft_time::FrameRate;

    use super::*;
    use crate::test_support::FakeBackend;

    #[test]
    fn parse_reads_token_and_link() {
        let got = ActivateRequest::parse(&json!({"token": "t", "from_pid": 7, "open": {"link_id": "L"}})).unwrap();
        assert_eq!(got, ActivateRequest { token: Some("t".into()), open_link: Some("L".into()) });
        let bare = ActivateRequest::parse(&json!({"token": null, "from_pid": null, "open": null})).unwrap();
        assert_eq!(bare, ActivateRequest { token: None, open_link: None });
        assert_eq!(ActivateRequest::parse(&json!({})).unwrap(), bare);
        assert!(ActivateRequest::parse(&json!({"token": 5})).is_err());
        assert!(ActivateRequest::parse(&json!({"open": "L"})).is_err());
        assert!(ActivateRequest::parse(&json!({"open": {"link_id": 3}})).is_err());
    }

    fn two_comp_session() -> (Session, effectcraft_project::ItemId, effectcraft_project::ItemId) {
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let mut add = |name: &str| {
            let comp = Comp::new(64, 36, rate, rate.tick_of(1));
            session.project_mut().add_item(name, Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)))
        };
        let first = add("First");
        let second = add("Second");
        session
            .project_mut()
            .craft_links
            .insert("L1".into(), CraftLinkBinding { link_id: "L1".into(), comp: second, source: json!({}), result_path: "/tmp/x.mov".into(), epoch: 0 });
        session.open_comp(first);
        (session, first, second)
    }

    #[test]
    fn activate_opens_the_bound_comp_then_focuses_with_the_token() {
        let (mut session, first, second) = two_comp_session();
        assert_eq!(session.active_comp_id(), Some(first));
        let backend = FakeBackend::default();
        let reply = handle_activate(&mut session, &backend, &json!({"token": "tok", "open": {"link_id": "L1"}}));
        assert_eq!(session.active_comp_id(), Some(second), "open.link_id did not open the bound comp");
        assert_eq!(reply, json!({"ok": true, "result": {"activated": "requested"}}));
        assert_eq!(backend.recorded(), ["pending:tok", "focus"]);
    }

    #[test]
    fn a_token_the_window_cannot_apply_is_not_a_request() {
        let (mut session, _, _) = two_comp_session();
        let backend = FakeBackend { cannot_apply: true, ..FakeBackend::default() };
        let reply = handle_activate(&mut session, &backend, &json!({"token": "tok", "open": {"link_id": "L1"}}));
        let linux = cfg!(target_os = "linux");
        assert_eq!(reply["result"]["activated"], if linux { json!(false) } else { json!("requested") });
        if linux {
            assert_eq!(backend.recorded(), ["focus", "attention"]);
        }
    }

    #[test]
    fn activate_without_a_token_asks_for_attention_and_says_false() {
        let (mut session, first, _) = two_comp_session();
        let backend = FakeBackend::default();
        let reply = handle_activate(&mut session, &backend, &json!({"token": null, "open": {"link_id": "no-such-link"}}));
        assert_eq!(session.active_comp_id(), Some(first), "an unknown link changed the active comp");
        // Linux cannot confirm a focus request without a token, so it must not say `requested`.
        let linux = cfg!(target_os = "linux");
        assert_eq!(reply["result"]["activated"], if linux { json!(false) } else { json!("requested") });
        if linux {
            assert_eq!(backend.recorded(), ["focus", "attention"]);
        }
    }

    #[test]
    fn activate_rejects_bad_params() {
        let (mut session, _, _) = two_comp_session();
        let backend = FakeBackend::default();
        let reply = handle_activate(&mut session, &backend, &json!({"token": 1}));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "bad_params");
        assert!(backend.recorded().is_empty());
    }

    struct FixedSink {
        reply: Mutex<Option<Value>>,
        seen: Mutex<Vec<(String, Value)>>,
    }

    impl ControlSink for FixedSink {
        fn submit(&self, method: String, params: Value) -> Option<std::sync::mpsc::Receiver<Value>> {
            self.seen.lock().unwrap().push((method, params));
            let (tx, rx) = channel();
            if let Some(value) = self.reply.lock().unwrap().take() {
                tx.send(value).unwrap();
            }
            Some(rx)
        }
    }

    fn sink_answer(reply: Option<Value>) -> (Activation, Vec<(String, Value)>) {
        let sink = Arc::new(FixedSink { reply: Mutex::new(reply), seen: Mutex::new(Vec::new()) });
        let activate = SinkActivate::new(sink.clone());
        let got = activate.activate(Some("t".into()), Some(9), Some(json!({"link_id": "L"})));
        let seen = sink.seen.lock().unwrap().clone();
        (got, seen)
    }

    #[test]
    fn sink_activate_forwards_to_the_ui_thread_and_maps_the_answer() {
        let (got, seen) = sink_answer(Some(json!({"ok": true, "result": {"activated": true}})));
        assert_eq!(got, Activation::Yes);
        assert_eq!(seen, [("app.activate".to_string(), json!({"token": "t", "from_pid": 9, "open": {"link_id": "L"}}))]);
        assert_eq!(sink_answer(Some(json!({"ok": true, "result": {"activated": "requested"}}))).0, Activation::Requested);
        assert_eq!(sink_answer(Some(json!({"ok": true, "result": {"activated": false}}))).0, Activation::No);
    }

    #[test]
    fn sink_activate_never_guesses_success() {
        // The UI thread dropped the request: the channel closes with no answer.
        assert_eq!(sink_answer(None).0, Activation::No);
        // An error reply or an unreadable one is not a success either.
        assert_eq!(sink_answer(Some(json!({"ok": false, "error": {"code": "bad_params", "message": "x", "data": {}}}))).0, Activation::No);
        assert_eq!(sink_answer(Some(json!({"ok": true, "result": {"activated": 1}}))).0, Activation::No);
    }

    #[test]
    fn egui_backend_sends_viewport_commands() {
        let ctx = egui::Context::default();
        let backend = EguiActivation::new(ctx.clone());
        let mut focus = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
            focus = Some(backend.focus());
            backend.request_user_attention();
        });
        assert_eq!(focus, Some(FocusResult::Pending), "an unfocused window cannot say Focused");
        let commands = &output.viewport_output.get(&egui::ViewportId::ROOT).expect("root viewport").commands;
        assert!(commands.iter().any(|c| matches!(c, egui::ViewportCommand::Focus)), "{commands:?}");
        assert!(commands.iter().any(|c| matches!(c, egui::ViewportCommand::RequestUserAttention(_))), "{commands:?}");
        output.textures_delta.clear();
        assert_eq!(backend.mint_token(), None);
    }

    #[test]
    fn egui_backend_cannot_apply_a_token() {
        let backend = EguiActivation::new(egui::Context::default());
        assert!(!backend.can_apply_token());
    }
}
