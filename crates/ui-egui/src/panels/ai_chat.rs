//! Window ▸ AI Chat: chat with a local model or agents exposed by a local bridge.
//!
//! This panel connects only to loopback addresses over plain HTTP. A separately configured
//! bridge can reach remote services while keeping their credentials out of the app.
//! Requests run on a worker thread and are polled every frame so the interface stays responsive.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{Align2, Rect, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::i18n::tr;
use crate::theme::Tokens;

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// `you`, `ai`, `error` or `info`.
    pub kind: String,
    pub text: String,
}

/// One message of the conversation sent to an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// `user`, `assistant` or `system`.
    pub role: String,
    pub content: String,
}

/// An agent the bridge reported (`GET /agents`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub note: String,
}

/// In-memory AI Chat state, excluded from the parent UI state's serialization.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiChat {
    pub input: String,
    pub lines: Vec<Line>,
    pub history: Vec<Message>,
    /// The bridge's agents, last fetched.
    pub agents: Vec<Agent>,
    pub status: String,
}

impl AiChat {
    fn push(&mut self, kind: &str, text: &str) {
        self.lines.push(Line { kind: kind.into(), text: text.to_string() });
        if self.lines.len() > 500 {
            let extra = self.lines.len() - 500;
            self.lines.drain(..extra);
        }
    }
}

/// A finished model request: the reply text and the id of the model that answered it.
pub struct Reply {
    pub agent: String,
    pub result: Result<(String, Option<String>), String>,
}

/// Worker threads behind the panel (not serialized; lives on the app).
#[derive(Default)]
pub struct Runtime {
    reply: Option<Receiver<Reply>>,
    agents: Option<Receiver<Result<Vec<Agent>, String>>>,
    pub busy: bool,
    fetched: bool,
}

/// Validate an HTTP endpoint and return its authority, including its required port.
/// Address resolution and loopback enforcement happen at the connection boundary.
pub fn host_of(url: &str) -> Result<&str, String> {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("endpoint must not contain whitespace or control characters".into());
    }
    let authority = if let Some((scheme, rest)) = url.split_once("://") {
        if !scheme.eq_ignore_ascii_case("http") {
            return Err("unsupported endpoint scheme: only plain HTTP is supported (no HTTPS)".into());
        }
        rest
    } else {
        url
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains(['@', '/', '\\', '?', '#', '%']) {
        return Err("endpoint must be a host and port, without userinfo, path, query or fragment".into());
    }
    let port = if let Some(rest) = authority.strip_prefix('[') {
        let (ip, port) = rest.split_once("]:").ok_or("IPv6 endpoints require [address]:port")?;
        ip.parse::<std::net::Ipv6Addr>().map_err(|_| "invalid IPv6 endpoint address")?;
        port
    } else {
        let (host, port) = authority.rsplit_once(':').ok_or("endpoint requires an explicit port")?;
        if host.is_empty()
            || host.len() > 253
            || !host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err("invalid endpoint hostname or IPv4 address".into());
        }
        port
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().ok().filter(|p| *p != 0).is_none() {
        return Err("endpoint port must be between 1 and 65535".into());
    }
    Ok(authority)
}

/// The picker label for an agent id.
fn label_of(selected: &str, agents: &[Agent]) -> String {
    if selected == "ollama" {
        return tr("Ollama (direct)").to_string();
    }
    agents.iter().find(|a| a.id == selected).map(|a| a.label.clone()).unwrap_or_else(|| selected.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
mod http {
    //! One HTTP/1.1 request to a local server, over `std::net` — the app has no HTTP client
    //! dependency. Only loopback destinations are allowed; TLS belongs to the bridge.

    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::time::Duration;

    pub(super) fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), String> {
        if addresses.is_empty() {
            return Err("endpoint resolved to no addresses".into());
        }
        if addresses.iter().any(|addr| !addr.ip().is_loopback()) {
            return Err("endpoint must resolve only to loopback addresses".into());
        }
        Ok(())
    }

    pub fn json(endpoint: &str, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Result<serde_json::Value, String> {
        let host = super::host_of(endpoint)?;
        if !matches!(method, "GET" | "POST") || !matches!(path, "/chat" | "/api/chat" | "/agents") {
            return Err("unsupported local API request".into());
        }
        let addresses: Vec<SocketAddr> = host.to_socket_addrs().map_err(|e| format!("bad address `{host}`: {e}"))?.collect();
        validate_addresses(&addresses)?;
        // Validate the entire resolution before connecting, and never resolve again when connecting.
        // Try both families so localhost works when the server listens on only IPv4 or IPv6.
        let mut connection = Err("no reachable loopback address".to_string());
        for addr in addresses {
            connection = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("cannot reach {host} ({e})"));
            if connection.is_ok() {
                break;
            }
        }
        let mut s = connection?;
        let _ = s.set_read_timeout(Some(timeout));
        let _ = s.set_write_timeout(Some(timeout));
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes())).map_err(|e| format!("write to {host}: {e}"))?;
        let mut raw = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(buf.get(..n).unwrap_or(&buf));
                    if raw.len() > 8_388_608 {
                        return Err("response too large".into());
                    }
                }
                // A server that closes with unread request data resets the connection; whatever
                // arrived is still parsed below (an incomplete response fails there, with the body).
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("no HTTP response from {host} ({} bytes)", text.len()))?;
        let status: u32 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad JSON from {host}: {e}"))?;
        if status != 200 {
            let msg = v.get("error").and_then(|e| e.as_str()).unwrap_or("HTTP error");
            return Err(format!("{msg} (HTTP {status})"));
        }
        Ok(v)
    }
}

/// One chat request; `ollama` talks to the local model, anything else to the bridge. The reply
/// carries the id of the model that answered (the bridge resolves `aiChat.agentModel`).
#[cfg(not(target_arch = "wasm32"))]
fn request(agent: &str, messages: &[Message], prefs: &effectcraft_engine::prefs::AiChatPrefs) -> Result<(String, Option<String>), String> {
    let msgs: Vec<Value> = messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect();
    if agent == "ollama" || agent.is_empty() {
        if prefs.model.trim().is_empty() {
            return Err("Configure an installed model in Settings ▸ AI Chat ▸ Ollama Model before sending.".into());
        }
        let body = json!({"model": prefs.model, "stream": false, "messages": msgs}).to_string();
        let v = http::json(&prefs.ollama_url, "POST", "/api/chat", Some(&body), Duration::from_secs(300))?;
        let reply = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or_default();
        Ok((if reply.is_empty() { "(empty reply)".into() } else { reply.to_string() }, Some(prefs.model.clone())))
    } else {
        let mut body = json!({"agent": agent, "messages": msgs});
        if !prefs.agent_model.is_empty() {
            body["model"] = json!(prefs.agent_model);
        }
        let v = http::json(&prefs.bridge_url, "POST", "/chat", Some(&body.to_string()), Duration::from_secs(600))?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(e.to_string());
        }
        let reply = v.get("reply").and_then(Value::as_str).unwrap_or("(empty reply)").to_string();
        let model = v.get("model").and_then(Value::as_str).map(str::to_string);
        Ok((reply, model))
    }
}

/// Send the input as the next message (spawns a worker thread).
pub fn send(app: &mut EffectcraftApp, ctx: &egui::Context) {
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
    let text = app.ui.ai_chat.input.trim().to_string();
    if text.is_empty() || app.ai_chat.busy {
        return;
    }
    let prefs = app.session.prefs.ai_chat.clone();
    let agent = if prefs.agent.trim().is_empty() { "ollama".to_string() } else { prefs.agent.clone() };
    let chat = &mut app.ui.ai_chat;
    chat.input.clear();
    chat.push("you", &text);
    chat.history.push(Message { role: "user".into(), content: text });
    chat.status = format!("{} {agent} …", tr("asking"));
    let mut messages = vec![Message { role: "system".into(), content: prefs.system_prompt.clone() }];
    messages.extend(chat.history.iter().cloned());

    if cfg!(target_arch = "wasm32") {
        chat.push("error", "The browser build can't reach local servers; the AI Chat panel runs in the desktop app.");
        chat.status = "not available in the browser".into();
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        app.ai_chat.reply = Some(rx);
        app.ai_chat.busy = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = request(&agent, &messages, &prefs);
            let _ = tx.send(Reply { agent, result });
            ctx.request_repaint();
        });
    }
}

/// Fetch the bridge's agent list (spawns a worker thread).
pub fn fetch_agents(app: &mut EffectcraftApp, ctx: &egui::Context) {
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
    if cfg!(target_arch = "wasm32") {
        app.ui.ai_chat.status = "the bridge runs beside the desktop app".into();
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.ai_chat.agents.is_some() {
            return;
        }
        let url = app.session.prefs.ai_chat.bridge_url.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        app.ai_chat.agents = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = http::json(&url, "GET", "/agents", None, Duration::from_secs(3))
                .map(|v| v.get("agents").and_then(Value::as_array).cloned().unwrap_or_default())
                .map(|list| list.iter().filter_map(|a| serde_json::from_value::<Agent>(a.clone()).ok()).collect::<Vec<_>>());
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }
}

/// Collect finished worker replies (called every frame).
pub fn poll(app: &mut EffectcraftApp, ctx: &egui::Context) {
    if let Some(rx) = &app.ai_chat.reply {
        match rx.try_recv() {
            Ok(r) => {
                app.ai_chat.reply = None;
                app.ai_chat.busy = false;
                let chat = &mut app.ui.ai_chat;
                match r.result {
                    Ok((text, model)) => {
                        chat.push("ai", &text);
                        chat.history.push(Message { role: "assistant".into(), content: text });
                        chat.status = match model {
                            Some(m) if !m.is_empty() => format!("ready · {m}"),
                            _ => "ready".into(),
                        };
                    }
                    Err(e) => {
                        // The failed turn doesn't stay in the conversation, so a retry doesn't duplicate it.
                        chat.history.pop();
                        chat.push("error", &e);
                        chat.status = format!("error: {e}");
                    }
                }
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(TryRecvError::Disconnected) => {
                app.ai_chat.reply = None;
                app.ai_chat.busy = false;
                app.ui.ai_chat.status = "the request thread stopped".into();
            }
        }
    }
    if let Some(rx) = &app.ai_chat.agents {
        match rx.try_recv() {
            Ok(Ok(list)) => {
                app.ai_chat.agents = None;
                let n = list.len();
                app.ui.ai_chat.agents = list;
                app.ui.ai_chat.status = format!("{}: {n}", tr("bridge agents"));
            }
            Ok(Err(e)) => {
                app.ai_chat.agents = None;
                app.ui.ai_chat.status = format!("{} ({e})", tr("bridge unavailable — check your local bridge"));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => app.ai_chat.agents = None,
        }
    }
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    poll(app, ui.ctx());
    let t = app.tokens;

    // First open: try the bridge once.
    if !app.ai_chat.fetched && app.ai_chat.agents.is_none() && app.ui.ai_chat.agents.is_empty() {
        app.ai_chat.fetched = true;
        let ctx = ui.ctx().clone();
        fetch_agents(app, &ctx);
    }

    // Top bar: agent picker and Refresh.
    let bar_h = 26.0;
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), bar_h));
    let agents = app.ui.ai_chat.agents.clone();
    let selected = {
        let a = app.session.prefs.ai_chat.agent.trim();
        if a.is_empty() { "ollama".to_string() } else { a.to_string() }
    };
    let mut pick: Option<String> = None;
    let mut refresh = false;
    {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(top));
        child.horizontal(|ui| {
            ui.label(tr("Agent:"));
            egui::ComboBox::from_id_salt("ai-chat-agent").selected_text(label_of(&selected, &agents)).width(128.0).show_ui(ui, |ui| {
                if ui.selectable_label(selected == "ollama", tr("Ollama (direct)")).clicked() {
                    pick = Some("ollama".into());
                }
                for a in &agents {
                    let text = if a.ready { a.label.clone() } else { format!("{} — {}", a.label, tr("not ready")) };
                    if ui.selectable_label(selected == a.id, text).clicked() {
                        pick = Some(a.id.clone());
                    }
                }
            });
            if ui.button(tr("Refresh")).clicked() {
                refresh = true;
            }
        });
    }
    app.auto.add("aiChat.agent", top, "AI Chat agent");
    if let Some(id) = pick {
        // The `aiChat.agent` pref is the single source of truth (the Settings page edits it too).
        let ctx = ui.ctx().clone();
        let _ = crate::menus::invoke(app, &ctx, "prefs.set", serde_json::json!({"key": "aiChat.agent", "value": id}));
    }
    if refresh {
        let ctx = ui.ctx().clone();
        fetch_agents(app, &ctx);
    }

    // Transcript.
    let input_h = 60.0;
    let status_h = 26.0;
    let body = Rect::from_min_max(pos2(rect.min.x, top.max.y + 2.0), pos2(rect.max.x, rect.max.y - input_h - status_h - 6.0));
    {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
        egui::ScrollArea::vertical().id_salt("ai-chat-log").auto_shrink([false, false]).stick_to_bottom(true).show(&mut child, |ui| {
            if app.ui.ai_chat.lines.is_empty() {
                ui.add_space(6.0);
                ui.weak(tr("Ask a local model about your project. Configure a local bridge to reach additional agents."));
            }
            for l in &app.ui.ai_chat.lines {
                let color = match l.kind.as_str() {
                    "you" => t.text_faint,
                    "error" => t.danger,
                    _ => t.text,
                };
                ui.colored_label(color, &l.text);
                ui.add_space(3.0);
            }
        });
    }
    app.auto.add("aiChat.transcript", body, "AI Chat transcript");

    // Input.
    let in_rect = Rect::from_min_size(pos2(rect.min.x + 4.0, rect.max.y - input_h - status_h - 4.0), vec2(rect.width() - 8.0, input_h - 6.0));
    let mut send_now = false;
    {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(in_rect));
        let te = egui::TextEdit::multiline(&mut app.ui.ai_chat.input)
            .id(egui::Id::new("ai-chat-input"))
            .font(Tokens::ui(12.5))
            .desired_width(in_rect.width())
            .desired_rows(2)
            .hint_text(tr("Message… (⌘/Ctrl+Enter sends)"));
        let resp = child.add_sized(in_rect.size(), te);
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
            send_now = true;
        }
    }
    app.auto.add("aiChat.input", in_rect, "AI Chat input");

    // Status and the Settings / Clear / Send buttons.
    let bar = Rect::from_min_size(pos2(rect.min.x, rect.max.y - status_h), vec2(rect.width(), status_h));
    let send_r = Rect::from_min_size(pos2(bar.max.x - 64.0, bar.center().y - 10.0), vec2(56.0, 20.0));
    let clear_r = Rect::from_min_size(pos2(send_r.min.x - 52.0, send_r.min.y), vec2(48.0, 20.0));
    let settings_r = Rect::from_min_size(pos2(clear_r.min.x - 68.0, send_r.min.y), vec2(64.0, 20.0));
    let send_resp = ui.interact(send_r, egui::Id::new("ai-chat-send"), Sense::click());
    let clear_resp = ui.interact(clear_r, egui::Id::new("ai-chat-clear"), Sense::click());
    let settings_resp = ui.interact(settings_r, egui::Id::new("ai-chat-settings"), Sense::click());
    let p = ui.painter().with_clip_rect(rect);
    let accent = if app.ai_chat.busy {
        t.field_border
    } else if send_resp.hovered() {
        t.accent_hover
    } else {
        t.accent
    };
    p.rect_filled(send_r, 3.0, accent);
    p.text(send_r.center(), Align2::CENTER_CENTER, tr("Send"), Tokens::ui(12.0), egui::Color32::WHITE);
    p.rect_stroke(clear_r, 3.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    p.text(clear_r.center(), Align2::CENTER_CENTER, tr("Clear"), Tokens::ui(12.0), t.text);
    p.rect_stroke(settings_r, 3.0, egui::Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    p.text(settings_r.center(), Align2::CENTER_CENTER, tr("Settings"), Tokens::ui(12.0), t.text);
    app.auto.add("aiChat.send", send_r, "Send");
    app.auto.add("aiChat.clear", clear_r, "Clear");
    app.auto.add("aiChat.settings", settings_r, "AI Chat settings");
    let status = app.ui.ai_chat.status.clone();
    if !status.is_empty() {
        let sp = ui.painter().with_clip_rect(Rect::from_min_max(pos2(rect.min.x, bar.min.y), pos2(settings_r.min.x - 4.0, bar.max.y)));
        sp.text(pos2(rect.min.x + 6.0, bar.center().y), Align2::LEFT_CENTER, &status, Tokens::ui(11.0), t.text_faint);
    }
    if settings_resp.clicked() {
        // One click from the panel to its Settings page (also Settings ▸ AI Chat… in the menu bar).
        let ctx = ui.ctx().clone();
        let _ = crate::menus::invoke(app, &ctx, "app.settings", serde_json::json!({"page": "aiChat"}));
    }
    if clear_resp.clicked() {
        app.ui.ai_chat.lines.clear();
        app.ui.ai_chat.history.clear();
        app.ui.ai_chat.status = tr("cleared").into();
    }
    if send_resp.clicked() {
        send_now = true;
    }
    if send_now {
        let ctx = ui.ctx().clone();
        send(app, &ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_validates_authority() {
        for (url, host) in [("http://127.0.0.1:8765/", "127.0.0.1:8765"), ("localhost:11434", "localhost:11434"), ("http://[::1]:8765/", "[::1]:8765")] {
            assert_eq!(host_of(url).unwrap(), host);
        }
        for url in [
            "https://127.0.0.1:8765",
            "ftp://localhost:8765",
            "file:///tmp/socket",
            "http://user@localhost:8765",
            "http://user:password@localhost:8765",
            "http://localhost:8765/chat",
            "http://localhost:8765//",
            "http://localhost:8765?x=1",
            "http://localhost:8765/#fragment",
            "localhost:8765\r\nX-Injected: yes",
            "localhost:8765\0",
            " localhost:8765",
            "localhost:8765 ",
            "local\thost:8765",
            "localhost:8765\\evil",
            "localhost%0d%0a:8765",
            "http://",
            "localhost",
            "localhost:",
            "localhost:0",
            "localhost:65536",
            "localhost:+80",
            ":8765",
            "::1:8765",
            "[::1]",
            "[bad]:8765",
            "http://http://localhost:8765",
            "-localhost:8765",
            "local..host:8765",
        ] {
            assert!(host_of(url).is_err(), "accepted {url:?}");
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn rejects_non_loopback_addresses_before_connecting() {
        for ip in ["8.8.8.8", "10.0.0.1", "172.16.0.1", "192.168.1.1", "0.0.0.0", "::", "2001:4860:4860::8888", "fc00::1", "fe80::1", "::ffff:127.0.0.1"] {
            let addr = std::net::SocketAddr::new(ip.parse().unwrap(), 8765);
            let err = http::json(&addr.to_string(), "GET", "/agents", None, Duration::from_millis(50)).unwrap_err();
            assert!(err.contains("loopback"), "{ip}: {err}");
        }
        let local = "127.0.0.1:8765".parse().unwrap();
        let public = "8.8.8.8:8765".parse().unwrap();
        assert!(http::validate_addresses(&[local, public]).is_err());
        assert!(http::validate_addresses(&[public, local]).is_err());
        assert!(http::validate_addresses(&[]).is_err());
        assert!(http::validate_addresses(&[local, "[::1]:8765".parse().unwrap()]).is_ok());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn https_and_injected_requests_never_connect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        for scheme in ["https", "HTTPS", "ftp"] {
            let err = http::json(&format!("{scheme}://{addr}"), "GET", "/agents", None, Duration::from_secs(1)).unwrap_err();
            assert!(err.contains("scheme"), "{err}");
        }
        for (method, path) in [("GET\r\nInjected: yes", "/agents"), ("GET", "/agents\r\nInjected: yes")] {
            assert!(http::json(&addr.to_string(), method, path, None, Duration::from_secs(1)).is_err());
        }
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn localhost_and_ipv6_round_trip() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK", r#"{"reply":"local"}"#);
        let endpoint = format!("http://localhost:{}/", addr.port());
        assert_eq!(http::json(&endpoint, "GET", "/agents", None, Duration::from_secs(2)).unwrap()["reply"], "local");
        assert!(server.join().unwrap().contains(&format!("Host: localhost:{}\r\n", addr.port())));

        let listener = std::net::TcpListener::bind("[::1]:0").unwrap();
        let (addr, server) = serve_listener(listener, "HTTP/1.1 200 OK", r#"{"reply":"ipv6"}"#);
        assert_eq!(http::json(&format!("http://{addr}/"), "GET", "/agents", None, Duration::from_secs(2)).unwrap()["reply"], "ipv6");
        server.join().unwrap();
    }

    #[test]
    fn ui_snapshots_omit_chat_and_ignore_legacy_conversations() {
        let mut ui = crate::state::UiState::default();
        ui.ai_chat.input = "private draft".into();
        ui.ai_chat.push("you", "private transcript");
        ui.ai_chat.history.push(Message { role: "user".into(), content: "private history".into() });
        let mut snapshot = serde_json::to_value(&ui).unwrap();
        assert!(snapshot.get("ai_chat").is_none());
        snapshot["ai_chat"] = serde_json::to_value(&ui.ai_chat).unwrap();
        let restored: crate::state::UiState = serde_json::from_value(snapshot).unwrap();
        assert_eq!(restored.ai_chat, AiChat::default());
    }

    #[test]
    fn push_keeps_the_transcript_bounded() {
        let mut chat = AiChat::default();
        for i in 0..520 {
            chat.push("ai", &format!("line {i}"));
        }
        assert_eq!(chat.lines.len(), 500);
        assert_eq!(chat.lines.first().map(|l| l.text.as_str()), Some("line 20"));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn http_json_round_trips_against_a_local_server() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK", r#"{"reply":"pong"}"#);
        let v = http::json(&addr.to_string(), "POST", "/chat", Some("{}"), Duration::from_secs(5)).unwrap();
        assert_eq!(v["reply"], "pong");
        server.join().unwrap();
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn http_json_reports_http_errors() {
        let (addr, server) = serve_once("HTTP/1.1 500 Internal Server Error", r#"{"error":"boom"}"#);
        let e = http::json(&addr.to_string(), "POST", "/chat", Some("{}"), Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("boom"), "{e}");
        server.join().unwrap();
    }

    /// A one-shot HTTP server that reads the whole request before replying (closing with unread
    /// request data would reset the connection instead of delivering the response). The thread
    /// returns the raw request it received.
    #[cfg(not(target_arch = "wasm32"))]
    fn serve_once(status_line: &str, body: &str) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
        serve_listener(std::net::TcpListener::bind("127.0.0.1:0").unwrap(), status_line, body)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn serve_listener(listener: std::net::TcpListener, status_line: &str, body: &str) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let addr = listener.local_addr().unwrap();
        let response = format!("{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut s = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("test server did not accept a connection: {e}"),
                }
            };
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                if let Some(end) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&req[..end]).to_string();
                    let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ").and_then(|v| v.trim().parse().ok())).unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = s.write_all(response.as_bytes());
            String::from_utf8_lossy(&req).into_owned()
        });
        (addr, handle)
    }

    /// The bridge gets `agent`, `messages` and — only when a model is chosen in Settings —
    /// `model`; it answers with the id of the model that replied, which the status line shows.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn request_sends_the_chosen_agent_model_to_the_bridge() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK", r#"{"reply":"model ok","model":"example-model-resolved"}"#);
        let prefs = effectcraft_engine::prefs::AiChatPrefs {
            bridge_url: addr.to_string(),
            agent: "example-agent".into(),
            agent_model: "custom/model-v2:fast".into(),
            ..Default::default()
        };
        let (reply, model) = request("example-agent", &[Message { role: "user".into(), content: "hi".into() }], &prefs).unwrap();
        assert_eq!(reply, "model ok");
        assert_eq!(model.as_deref(), Some("example-model-resolved"));
        let req = server.join().unwrap();
        assert!(req.contains(r#""agent":"example-agent""#), "{req}");
        assert!(req.contains(r#""model":"custom/model-v2:fast""#), "{req}");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn multi_turn_send_and_poll_preserve_history_over_tcp() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        app.session.prefs.ai_chat.agent = "example-agent".into();
        app.session.prefs.ai_chat.system_prompt = "Be helpful.".into();
        let mut expected = vec![json!({"role": "system", "content": "Be helpful."})];
        for (input, answer) in [("first question", "first answer"), ("follow-up question", "follow-up answer")] {
            let body = json!({"reply": answer}).to_string();
            let (addr, server) = serve_once("HTTP/1.1 200 OK", &body);
            app.session.prefs.ai_chat.bridge_url = format!("http://{addr}");
            app.ui.ai_chat.input = input.into();
            send(&mut app, &ctx);
            assert!(app.ai_chat.busy);
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while app.ai_chat.busy && std::time::Instant::now() < deadline {
                poll(&mut app, &ctx);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!app.ai_chat.busy, "chat worker did not finish");
            let raw = server.join().unwrap();
            let (_, body) = raw.split_once("\r\n\r\n").unwrap();
            let sent: Value = serde_json::from_str(body).unwrap();
            expected.push(json!({"role": "user", "content": input}));
            assert_eq!(sent["messages"], json!(expected));
            expected.push(json!({"role": "assistant", "content": answer}));
            assert_eq!(serde_json::to_value(&app.ui.ai_chat.history).unwrap(), json!(&expected[1..]));
            assert_eq!(app.ui.ai_chat.lines.last().unwrap().text, answer);
        }
    }

    /// At Agent Default the request carries no `model`, so the agent keeps its own.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn request_omits_the_model_at_agent_default() {
        let (addr, server) = serve_once("HTTP/1.1 200 OK", r#"{"reply":"hi"}"#);
        let prefs = effectcraft_engine::prefs::AiChatPrefs {
            bridge_url: addr.to_string(),
            agent: "example-agent".into(),
            agent_model: String::new(),
            ..Default::default()
        };
        let (reply, model) = request("example-agent", &[Message { role: "user".into(), content: "hi".into() }], &prefs).unwrap();
        assert_eq!(reply, "hi");
        assert_eq!(model, None);
        let req = server.join().unwrap();
        assert!(!req.contains(r#""model""#), "{req}");
    }
}
