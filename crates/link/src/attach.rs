//! `link.attach`: reopen a link FilmCraft already opened and report its format and epoch.
//!
//! This does not create a composition. When the session has no such link, the sidecar project
//! from `project` is opened first, unless the session holds unsaved edits (`busy`). An unknown
//! link, a missing sidecar, a missing comp item, or a `comp` string that is not the bound comp id
//! is `comp_missing`. The bound comp becomes the active comp, so `link.back` is enabled for it.

use effectcraft_engine::Session;
use serde_json::{Value, json};

use crate::receive::LinkFormat;

/// Failure of [`attach_link`]. [`Self::reply`] is the object the control channel returns.
#[derive(Debug)]
pub struct AttachError {
    pub code: &'static str,
    pub message: String,
}

impl AttachError {
    pub fn comp_missing(message: impl Into<String>) -> Self {
        Self { code: "comp_missing", message: message.into() }
    }
    pub fn bad_params(message: impl Into<String>) -> Self {
        Self { code: "bad_params", message: message.into() }
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self { code: "io_error", message: message.into() }
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self { code: "busy", message: message.into() }
    }
    pub fn reply(&self) -> Value {
        json!({
            "ok": false,
            "error": {"code": self.code, "message": self.message, "data": {}}
        })
    }
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AttachError {}

/// Reopen `params` (`link_id`, `project`, `comp`).
///
/// `project` is the sidecar path FilmCraft sends. The binding lives in the project that holds
/// the comp. A session that already has the binding does not read the file. A session that does
/// not have it opens the file, if the session has no unsaved edits. This does not create a comp.
pub fn attach_link(session: &mut Session, params: &Value) -> Result<Value, AttachError> {
    let link_id = required_str(params, "link_id")?;
    let project = required_str(params, "project")?;
    let comp = required_str(params, "comp")?;
    if !session.project.craft_links.contains_key(link_id) {
        open_sidecar(session, link_id, project)?;
    }
    let Some(binding) = session.project.craft_links.get(link_id) else {
        return Err(AttachError::comp_missing(format!("no link `{link_id}`")));
    };
    if binding.comp.0.to_string() != comp {
        return Err(AttachError::comp_missing(format!("comp `{comp}` is not the composition bound to link `{link_id}`")));
    }
    let epoch = binding.epoch;
    let comp_id = binding.comp;
    let Some(comp) = session.project.comp(comp_id) else {
        return Err(AttachError::comp_missing(format!("the composition for link `{link_id}` is not in the project")));
    };
    let (width, height, frame_rate, duration) = (comp.width, comp.height, comp.frame_rate, comp.duration);
    session.open_comp(comp_id);
    if session.active_comp_id() != Some(comp_id) {
        return Err(AttachError::comp_missing(format!("the composition for link `{link_id}` could not be opened")));
    }
    let format = LinkFormat { w: width, h: height, rate_num: frame_rate.num, rate_den: frame_rate.den, duration_ticks: duration.0, alpha: true };
    let format = serde_json::to_value(&format).map_err(|err| AttachError::io(format!("could not encode the link.attach format: {err}")))?;
    Ok(json!({"format": format, "epoch": epoch}))
}

/// Open the sidecar project that should hold `link_id`. The session keeps its project when this fails.
fn open_sidecar(session: &mut Session, link_id: &str, project: &str) -> Result<(), AttachError> {
    if !std::path::Path::new(project).is_file() {
        return Err(AttachError::comp_missing(format!("no link `{link_id}` is open and the sidecar project `{project}` does not exist")));
    }
    if session.is_dirty() {
        return Err(AttachError::busy(format!(
            "link `{link_id}` is in `{project}`, but the open project has unsaved changes. Save or close it, then attach again"
        )));
    }
    session.execute("file.open", json!({"path": project})).map_err(|err| AttachError::io(format!("could not open the sidecar project `{project}`: {err}")))?;
    Ok(())
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, AttachError> {
    params
        .get(key)
        .filter(|value| !value.is_null())
        .ok_or_else(|| AttachError::bad_params(format!("missing {key}")))?
        .as_str()
        .ok_or_else(|| AttachError::bad_params(format!("{key} must be a string")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use effectcraft_engine::Session;
    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemId, ItemKind};
    use effectcraft_time::FrameRate;
    use serde_json::json;

    use super::*;

    const LINK: &str = "33333333-3333-4333-8333-333333333333";

    fn add_comp(session: &mut Session) -> (ItemId, i64) {
        let rate = FrameRate::new(24, 1);
        let duration = rate.tick_of(1);
        let mut comp = Comp::new(128, 72, rate, duration);
        comp.background = [0.1, 0.2, 0.8];
        let id = session.project_mut().add_item("Link Comp", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp)));
        (id, duration.0)
    }

    fn bind(session: &mut Session, link_id: &str, comp: ItemId, epoch: u64) {
        session.project_mut().craft_links.insert(
            link_id.to_string(),
            CraftLinkBinding {
                link_id: link_id.to_string(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: "/tmp/effectcraft-link-attach/result.mov".to_string(),
                epoch,
            },
        );
    }

    fn params(link_id: &str, comp: &str) -> Value {
        json!({"link_id": link_id, "project": "/tmp/effectcraft-link-attach/link.ecproj", "comp": comp})
    }

    #[test]
    fn attach_missing_link_is_comp_missing() {
        let mut session = Session::default();
        let before = session.project.items.len();
        let err = attach_link(&mut session, &params("no-such-link", "1")).unwrap_err();
        let reply = err.reply();
        assert_eq!(err.code, "comp_missing");
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["code"], "comp_missing");
        assert!(reply["error"]["data"].as_object().unwrap().is_empty());
        assert_eq!(session.project.items.len(), before, "attach created a composition");

        let (comp, _) = add_comp(&mut session);
        bind(&mut session, LINK, comp, 0);
        let items = session.project.items.len();
        let mismatch = attach_link(&mut session, &params(LINK, "999")).unwrap_err();
        assert_eq!(mismatch.code, "comp_missing");
        assert_eq!(session.project.items.len(), items, "a comp mismatch created a composition");

        let mut gone = Session::default();
        bind(&mut gone, LINK, ItemId(9), 0);
        let missing = attach_link(&mut gone, &params(LINK, "9")).unwrap_err();
        assert_eq!(missing.code, "comp_missing");
        assert!(gone.project.comp(ItemId(9)).is_none());
    }

    #[test]
    fn attach_returns_format_and_epoch() {
        let mut session = Session::default();
        let (comp, duration_ticks) = add_comp(&mut session);
        bind(&mut session, LINK, comp, 3);
        let result = attach_link(&mut session, &params(LINK, &comp.0.to_string())).unwrap();
        assert_eq!(result["format"]["w"], 128);
        assert_eq!(result["format"]["h"], 72);
        assert_eq!(result["format"]["rate_num"], 24);
        assert_eq!(result["format"]["rate_den"], 1);
        assert_eq!(result["format"]["duration_ticks"], duration_ticks);
        assert_eq!(result["format"]["alpha"], true);
        assert_eq!(result["epoch"], 3);
        assert_eq!(session.project.craft_links[LINK].epoch, 3);
    }

    fn write_sidecar(name: &str, session: &Session) -> String {
        let dir = std::env::temp_dir().join(format!("ec-attach-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("link.ecproj");
        std::fs::write(&path, session.project.to_json()).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn attach_reopens_the_sidecar_project_in_a_fresh_session() {
        let mut origin = Session::default();
        let (comp, duration_ticks) = add_comp(&mut origin);
        bind(&mut origin, LINK, comp, 4);
        let sidecar = write_sidecar("fresh", &origin);

        let mut fresh = Session::default();
        assert!(fresh.project.craft_links.is_empty());
        assert_eq!(fresh.active_comp_id(), None);
        let request = json!({"link_id": LINK, "project": sidecar, "comp": comp.0.to_string()});
        let result = attach_link(&mut fresh, &request).unwrap();

        assert_eq!(result["epoch"], 4);
        assert_eq!(result["format"]["w"], 128);
        assert_eq!(result["format"]["duration_ticks"], duration_ticks);
        assert_eq!(fresh.active_comp_id(), Some(comp), "the attached comp is not the active comp");
        assert!(fresh.project.craft_links.contains_key(LINK));
        assert!(!fresh.is_dirty(), "attach left the session dirty");
    }

    #[test]
    fn attach_does_not_replace_a_project_with_unsaved_edits() {
        let mut origin = Session::default();
        let (comp, _) = add_comp(&mut origin);
        bind(&mut origin, LINK, comp, 0);
        let sidecar = write_sidecar("busy", &origin);

        let mut busy = Session::default();
        let rate = FrameRate::new(24, 1);
        let other = busy
            .edit("unsaved edit", None, |project, _state| {
                let comp = Comp::new(64, 36, rate, rate.tick_of(1));
                Ok(project.add_item("Unsaved", Label::Sandstone, None, ItemKind::Comp(Arc::new(comp))))
            })
            .unwrap();
        assert!(busy.is_dirty(), "the setup edit must leave the session with unsaved changes");
        let items = busy.project.items.len();
        let request = json!({"link_id": LINK, "project": sidecar, "comp": comp.0.to_string()});
        let err = attach_link(&mut busy, &request).unwrap_err();

        assert_eq!(err.code, "busy");
        assert_eq!(busy.project.items.len(), items, "the unsaved project was replaced");
        assert!(busy.project.comp(other).is_some());
        assert!(busy.project.craft_links.is_empty());
    }

    #[test]
    fn attach_with_a_sidecar_that_lacks_the_link_is_comp_missing() {
        let origin = Session::default();
        let sidecar = write_sidecar("lacks", &origin);
        let mut fresh = Session::default();
        let err = attach_link(&mut fresh, &json!({"link_id": LINK, "project": sidecar, "comp": "1"})).unwrap_err();
        assert_eq!(err.code, "comp_missing");
        let gone = attach_link(&mut fresh, &json!({"link_id": LINK, "project": "/tmp/effectcraft-link-attach/none.ecproj", "comp": "1"})).unwrap_err();
        assert_eq!(gone.code, "comp_missing");
        assert!(gone.message.contains("does not exist"), "{}", gone.message);
    }

    #[test]
    fn attach_makes_the_bound_comp_active() {
        let mut session = Session::default();
        let (first, _) = add_comp(&mut session);
        let (second, _) = add_comp(&mut session);
        bind(&mut session, LINK, second, 0);
        session.open_comp(first);
        assert_eq!(session.active_comp_id(), Some(first));
        attach_link(&mut session, &params(LINK, &second.0.to_string())).unwrap();
        assert_eq!(session.active_comp_id(), Some(second));
    }
}
