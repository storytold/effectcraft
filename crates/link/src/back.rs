//! `link.back`: render the bound comp, ask FilmCraft to move to the playhead, then activate it.
//!
//! Order matters. The result movie is rendered first, so it exists even when FilmCraft is not
//! running. Then `link.focus`, then `app.activate`. A failure at either step returns a
//! [`BackError`] that carries the finished render in `data.rendered`. No step reports success it
//! did not get from FilmCraft.

use craft_ipc::RuntimeDir;
use craft_ipc::server::Activation;
use effectcraft_engine::Session;
use serde_json::{Value, json};

use crate::activation::ActivationBackend;
use crate::peer::{FilmCraftPeer, PeerError};
use crate::result::{RenderError, RenderOutcome, render_result};

/// Failure of [`back_to_filmcraft`].
#[derive(Debug)]
pub struct BackError {
    pub code: &'static str,
    pub message: String,
    /// The result movie, when the render finished before this step failed.
    pub rendered: Option<RenderOutcome>,
}

impl BackError {
    pub fn comp_missing(message: impl Into<String>) -> Self {
        Self { code: "comp_missing", message: message.into(), rendered: None }
    }

    fn from_render(err: RenderError) -> Self {
        Self { code: err.code, message: err.message, rendered: None }
    }

    fn from_peer(err: PeerError, rendered: RenderOutcome) -> Self {
        let message = match &err {
            PeerError::NotRunning(why) => format!("{why} The result movie is rendered (epoch {}) and waits at {}.", rendered.epoch, rendered.path),
            PeerError::Rejected(_) => format!("{err}. The result movie is rendered (epoch {}) at {}.", rendered.epoch, rendered.path),
        };
        Self { code: err.code(), message, rendered: Some(rendered) }
    }

    /// The control-channel reply object.
    pub fn reply(&self) -> Value {
        let data = match &self.rendered {
            Some(done) => json!({"rendered": {"link_id": done.link_id, "epoch": done.epoch, "path": done.path}}),
            None => json!({}),
        };
        json!({
            "ok": false,
            "error": {"code": self.code, "message": self.message, "data": data}
        })
    }
}

impl std::fmt::Display for BackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BackError {}

/// True only when the active comp is the `comp` of some `craft_links` entry.
pub fn back_enabled(session: &Session) -> bool {
    bound_link(session).is_some()
}

/// Wire value of `app.activate` `activated`.
pub fn activation_value(activation: Activation) -> Value {
    match activation {
        Activation::Yes => Value::Bool(true),
        Activation::No => Value::Bool(false),
        Activation::Requested => Value::String("requested".into()),
    }
}

/// Render the link bound to the active comp, send `link.focus`, then `app.activate` FilmCraft.
///
/// `link.focus` params are `{"link_id", "place": {"seconds": <playhead seconds>}}`. The token for
/// `app.activate` comes from `backend`. `activated` in the reply is what FilmCraft answered.
/// `false` is not an error: FilmCraft asked for user attention instead of taking focus.
///
/// `runtime` `None` scans the real Craft Link runtime directory.
pub fn back_to_filmcraft(session: &mut Session, runtime: Option<&RuntimeDir>, backend: &dyn ActivationBackend) -> Result<Value, BackError> {
    let Some((link_id, source)) = bound_link(session) else {
        return Err(BackError::comp_missing("the active composition is not bound to a Craft Link"));
    };
    let outcome = render_result(session, &link_id).map_err(BackError::from_render)?;
    // The binding stores the new epoch now. A reopen without EffectCraft reads it from the sidecar.
    crate::live::flush_sidecars(session);
    let params = json!({
        "link_id": outcome.link_id,
        "place": {"seconds": session.time().seconds()}
    });
    let mut peer = FilmCraftPeer::focus(runtime, &source, &params).map_err(|err| BackError::from_peer(err, outcome.clone()))?;
    let token = crate::activation::mint_token(backend);
    let activated =
        peer.activate(token.as_deref(), json!({"link_id": outcome.link_id})).map_err(|err| BackError::from_peer(PeerError::Rejected(err), outcome.clone()))?;
    Ok(json!({
        "link_id": outcome.link_id,
        "epoch": outcome.epoch,
        "path": outcome.path,
        "activated": activation_value(activated)
    }))
}

/// `craft_links` is ordered by link id. The first binding for the active comp is the one Back uses.
fn bound_link(session: &Session) -> Option<(String, Value)> {
    let active = session.active_comp_id()?;
    session.project.craft_links.iter().find(|(_, binding)| binding.comp == active).map(|(id, binding)| (id.clone(), binding.source.clone()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use effectcraft_engine::Session;
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemId, ItemKind};
    use effectcraft_time::FrameRate;
    use serde_json::json;

    use super::*;
    use crate::test_support::{FakeBackend, FakeFilmCraft, FakeReplies, TempRuntime};

    const LINK: &str = "44444444-4444-4444-8444-444444444444";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ec-back-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add_comp(session: &mut Session, frames: i64) -> ItemId {
        let rate = FrameRate::new(24, 1);
        let mut comp = Comp::new(128, 72, rate, rate.tick_of(frames));
        comp.background = [0.1, 0.2, 0.8];
        session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)))
    }

    fn bind(session: &mut Session, link_id: &str, comp: ItemId, result_path: &str) {
        // `link.open` writes this sidecar. The render accepts only a directory that holds it.
        if let Some(dir) = std::path::Path::new(result_path).parent().filter(|dir| dir.is_dir()) {
            std::fs::write(dir.join(format!("{link_id}.ecproj")), b"{}").unwrap();
        }
        session.project_mut().craft_links.insert(
            link_id.to_string(),
            CraftLinkBinding {
                link_id: link_id.to_string(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: result_path.to_string(),
                epoch: 0,
            },
        );
    }

    /// A session whose active comp is bound, with the playhead on frame 1 of 2.
    fn bound_session(result: &std::path::Path) -> (Session, f64) {
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let comp = add_comp(&mut session, 2);
        bind(&mut session, LINK, comp, &result.to_string_lossy());
        session.open_comp(comp);
        session.set_time(rate.tick_of(1));
        let seconds = session.time().seconds();
        assert_ne!(seconds, 0.0, "playhead is 0, so the focus place would not show a real position");
        (session, seconds)
    }

    #[test]
    fn back_disabled_when_the_comp_is_not_bound() {
        let runtime = TempRuntime::new("unbound");
        let peer = FakeFilmCraft::start(&runtime.dir, FakeReplies::default());
        let backend = FakeBackend::default();
        let mut session = Session::default();
        assert!(!back_enabled(&session));

        let comp = add_comp(&mut session, 1);
        session.open_comp(comp);
        assert_eq!(session.active_comp_id(), Some(comp));
        assert!(!back_enabled(&session));
        let err = back_to_filmcraft(&mut session, Some(&runtime.dir), &backend).unwrap_err();
        assert_eq!(err.code, "comp_missing");
        assert!(peer.calls().is_empty(), "FilmCraft was contacted for an unbound comp: {:?}", peer.methods());
        assert_eq!(backend.mint_calls.load(Ordering::SeqCst), 0);

        let other = add_comp(&mut session, 1);
        bind(&mut session, LINK, other, "/tmp/effectcraft-link-back/absent.mov");
        assert!(!back_enabled(&session), "a binding for a different comp enabled Back");
        session.open_comp(other);
        assert!(back_enabled(&session));
    }

    #[test]
    fn back_renders_then_sends_focus_then_activate() {
        let dir = scratch("flow");
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("flow");
        let peer = FakeFilmCraft::start(&runtime.dir, FakeReplies { activate: Activation::Requested, ..FakeReplies::default() });
        let backend = FakeBackend { minted: Some("tok-1".into()), ..FakeBackend::default() };
        let (mut session, seconds) = bound_session(&result);

        let reply = back_to_filmcraft(&mut session, Some(&runtime.dir), &backend).unwrap();

        assert!(result.exists(), "result file was not written");
        let probed = effectcraft_media::probe(&result).unwrap();
        assert!(probed.codec.contains("ProRes 4444"), "{}", probed.codec);
        let calls = peer.calls();
        assert_eq!(peer.methods(), ["link.focus", "app.activate"], "{calls:?}");
        assert_eq!(calls[0].1, json!({"link_id": LINK, "place": {"seconds": seconds}}));
        // Only Linux mints. Elsewhere the token is null.
        let want_token = if cfg!(target_os = "linux") { json!("tok-1") } else { Value::Null };
        assert_eq!(calls[1].1, json!({"token": want_token, "from_pid": std::process::id(), "open": {"link_id": LINK}}));
        assert_eq!(reply["link_id"], LINK);
        assert_eq!(reply["epoch"], 1);
        assert_eq!(reply["path"], result.to_string_lossy().as_ref());
        assert_eq!(reply["activated"], "requested");
        assert_eq!(session.project.craft_links[LINK].epoch, 1);
    }

    #[test]
    fn back_writes_the_sidecar_with_the_epoch_of_the_result() {
        let dir = scratch("sidecar");
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("sidecar");
        let _peer = FakeFilmCraft::start(&runtime.dir, FakeReplies::default());
        let (mut session, _) = bound_session(&result);
        back_to_filmcraft(&mut session, Some(&runtime.dir), &FakeBackend::default()).unwrap();
        let text = std::fs::read_to_string(dir.join(format!("{LINK}.ecproj"))).unwrap();
        let saved = effectcraft_project::Project::from_json(&text).expect("the sidecar is a project");
        assert_eq!(saved.craft_links[LINK].epoch, 1, "Back left the sidecar at the epoch before its render");
    }

    #[test]
    fn back_reports_the_activation_answer_it_got() {
        let dir = scratch("refused");
        let (mut session, _) = bound_session(&dir.join(format!("{LINK}.mov")));
        for (answer, want) in [(Activation::Yes, json!(true)), (Activation::No, json!(false)), (Activation::Requested, json!("requested"))] {
            let runtime = TempRuntime::new("answer");
            let _peer = FakeFilmCraft::start(&runtime.dir, FakeReplies { activate: answer, ..FakeReplies::default() });
            let reply = back_to_filmcraft(&mut session, Some(&runtime.dir), &FakeBackend::default()).unwrap();
            assert_eq!(reply["activated"], want, "{answer:?}");
        }
    }

    #[test]
    fn back_without_filmcraft_is_a_typed_error_and_still_renders() {
        let dir = scratch("absent");
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("absent");
        let backend = FakeBackend { minted: Some("tok".into()), ..FakeBackend::default() };
        let (mut session, _) = bound_session(&result);

        let err = back_to_filmcraft(&mut session, Some(&runtime.dir), &backend).unwrap_err();

        assert_eq!(err.code, "not_ready");
        assert!(err.message.contains("FilmCraft is not running"), "{}", err.message);
        assert!(result.exists(), "the result render must complete without FilmCraft");
        assert_eq!(session.project.craft_links[LINK].epoch, 1);
        let rendered = err.rendered.as_ref().expect("error carries the finished render");
        assert_eq!((rendered.epoch, rendered.path.as_str()), (1, result.to_str().unwrap()));
        let reply = err.reply();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "not_ready");
        assert_eq!(reply["error"]["data"]["rendered"]["epoch"], 1);
        assert_eq!(backend.mint_calls.load(Ordering::SeqCst), 0, "a token was minted for a peer that does not exist");
    }

    #[test]
    fn back_when_filmcraft_rejects_focus_does_not_activate() {
        let dir = scratch("reject");
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("reject");
        // A code the client does not know maps to `Invalid`, which reports as `internal`.
        let focus = json!({"ok": false, "error": {"code": "code_from_the_future", "message": "no such link", "data": {}}});
        let peer = FakeFilmCraft::start(&runtime.dir, FakeReplies { focus, ..FakeReplies::default() });
        let (mut session, _) = bound_session(&result);

        let err = back_to_filmcraft(&mut session, Some(&runtime.dir), &FakeBackend::default()).unwrap_err();

        assert_eq!(err.code, "internal");
        assert!(err.message.contains("no such link"), "{}", err.message);
        assert_eq!(peer.methods(), ["link.focus"], "app.activate followed a refused link.focus");
        assert!(result.exists());
    }

    #[test]
    fn back_when_filmcraft_does_not_know_the_link_reports_link_unknown() {
        let dir = scratch("unknown");
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("unknown");
        let focus = json!({"ok": false, "error": {"code": "link_unknown", "message": "no such link", "data": {}}});
        let peer = FakeFilmCraft::start(&runtime.dir, FakeReplies { focus, ..FakeReplies::default() });
        let (mut session, _) = bound_session(&result);

        let err = back_to_filmcraft(&mut session, Some(&runtime.dir), &FakeBackend::default()).unwrap_err();

        assert_eq!(err.code, "link_unknown");
        assert_eq!(peer.methods(), ["link.focus"], "app.activate followed a refused link.focus");
    }
}
