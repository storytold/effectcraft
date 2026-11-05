//! `link.back` as a dynamic command.
//!
//! Registry ids for plugin-owned commands start with `plugin.<owner>.`, so owner `craft.link`
//! gives [`BACK_COMMAND`]. Enablement is `Cached`. [`sync_back_enabled`] writes it from the
//! session, because the registry never calls into the command to ask.

use std::sync::Arc;

use craft_command::{DynCommand, Enablement, Owner, RegisterError, Visibility};
use craft_ipc::RuntimeDir;
use effectcraft_engine::{EngineError, Session};
use serde_json::Value;

use crate::activation::ActivationBackend;
use crate::back::{back_enabled, back_to_filmcraft};

/// Owner of the command.
pub const OWNER: &str = "craft.link";
/// Registry id of Back to FilmCraft.
pub const BACK_COMMAND: &str = "plugin.craft.link.back";

const DISABLED_WHY: &str = "the active composition is not bound to a Craft Link";

/// Register [`BACK_COMMAND`] in `session`. `runtime` `None` scans the real runtime directory.
///
/// The command is off until [`sync_back_enabled`] finds a bound comp. A second call returns
/// [`RegisterError::Duplicate`].
pub fn install_back_command(
    session: &mut Session,
    runtime: Option<RuntimeDir>,
    backend: Arc<dyn ActivationBackend + Send + Sync>,
) -> Result<(), RegisterError> {
    let run = move |session: &mut Session, _params: &Value| -> Result<Value, EngineError> {
        back_to_filmcraft(session, runtime.as_ref(), backend.as_ref()).map_err(|err| EngineError::Other(err.to_string()))
    };
    session.registry.register(DynCommand {
        id: Arc::from(BACK_COMMAND),
        label: Arc::from("Back to FilmCraft"),
        params: Arc::from("{}"),
        shortcut: None,
        menu: Arc::from(Vec::new()),
        owner: Owner { plugin_id: Arc::from(OWNER), generation: 1 },
        enablement: Enablement::Cached(Err(DISABLED_WHY.to_string())),
        visibility: Visibility { menu: false, palette: true, shortcut: false },
        // It writes a movie and talks to another app. Replaying it from a journal is wrong.
        journal: false,
        run: Arc::new(run),
    })
}

/// True when [`install_back_command`] ran for this session.
pub fn back_installed(session: &Session) -> bool {
    session.registry.resolve(BACK_COMMAND).is_some()
}

/// Make the cached enablement match the session. Cheap when nothing changed.
pub fn sync_back_enabled(session: &Session) {
    let want = back_enabled(session);
    if session.is_enabled(BACK_COMMAND) != want {
        let state = if want { Ok(()) } else { Err(DISABLED_WHY.to_string()) };
        session.registry.set_enabled(BACK_COMMAND, state);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemKind};
    use effectcraft_time::FrameRate;
    use serde_json::json;

    use super::*;
    use crate::test_support::{FakeBackend, FakeFilmCraft, FakeReplies, TempRuntime};

    const LINK: &str = "55555555-5555-4555-8555-555555555555";

    fn session_with_comp() -> (Session, effectcraft_project::ItemId) {
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let mut comp = Comp::new(64, 36, rate, rate.tick_of(1));
        comp.background = [0.2, 0.2, 0.7];
        let id = session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
        session.open_comp(id);
        (session, id)
    }

    fn bind(session: &mut Session, comp: effectcraft_project::ItemId, result: &str) {
        // `link.open` writes this sidecar. The render accepts only a directory that holds it.
        if let Some(dir) = std::path::Path::new(result).parent() {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("{LINK}.ecproj")), b"{}").unwrap();
        }
        session.project_mut().craft_links.insert(
            LINK.into(),
            CraftLinkBinding {
                link_id: LINK.into(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: result.into(),
                epoch: 0,
            },
        );
    }

    fn install(session: &mut Session, runtime: &TempRuntime) {
        install_back_command(session, Some(runtime.dir.clone()), Arc::new(FakeBackend::default())).unwrap();
    }

    #[test]
    fn enablement_follows_the_binding_of_the_active_comp() {
        let runtime = TempRuntime::new("enable");
        let (mut session, comp) = session_with_comp();
        assert!(!back_installed(&session));
        install(&mut session, &runtime);
        assert!(back_installed(&session));
        sync_back_enabled(&session);
        assert!(!session.is_enabled(BACK_COMMAND), "enabled with no binding");
        let err = session.execute(BACK_COMMAND, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::Disabled(..)), "{err}");
        assert!(err.to_string().contains("not bound"), "{err}");

        bind(&mut session, comp, "/tmp/effectcraft-link-cmd/never.mov");
        sync_back_enabled(&session);
        assert!(session.is_enabled(BACK_COMMAND), "not enabled for a bound active comp");

        session.project_mut().craft_links.clear();
        sync_back_enabled(&session);
        assert!(!session.is_enabled(BACK_COMMAND), "still enabled after the binding is gone");
    }

    #[test]
    fn a_second_install_is_a_duplicate() {
        let runtime = TempRuntime::new("dup");
        let (mut session, _) = session_with_comp();
        install(&mut session, &runtime);
        let again = install_back_command(&mut session, Some(runtime.dir.clone()), Arc::new(FakeBackend::default()));
        assert_eq!(again.unwrap_err(), RegisterError::Duplicate);
    }

    #[test]
    fn the_command_renders_focuses_and_activates() {
        let dir = std::env::temp_dir().join(format!("ec-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("cmd");
        let peer = FakeFilmCraft::start(&runtime.dir, FakeReplies::default());
        let (mut session, comp) = session_with_comp();
        bind(&mut session, comp, &result.to_string_lossy());
        install(&mut session, &runtime);
        sync_back_enabled(&session);

        let done = session.execute(BACK_COMMAND, json!({})).unwrap();

        assert_eq!(done["activated"], "requested");
        assert_eq!(done["epoch"], 1);
        assert!(result.exists());
        assert_eq!(peer.methods(), ["link.focus", "app.activate"]);
    }

    #[test]
    fn the_command_fails_loudly_when_filmcraft_is_not_running() {
        let dir = std::env::temp_dir().join(format!("ec-cmd-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let result = dir.join(format!("{LINK}.mov"));
        let runtime = TempRuntime::new("cmdabs");
        let (mut session, comp) = session_with_comp();
        bind(&mut session, comp, &result.to_string_lossy());
        install(&mut session, &runtime);
        sync_back_enabled(&session);

        let err = session.execute(BACK_COMMAND, json!({})).unwrap_err();

        let text = err.to_string();
        assert!(text.contains("not_ready") && text.contains("FilmCraft is not running"), "{text}");
        assert!(result.exists(), "the render must finish even when FilmCraft is absent");
    }
}
