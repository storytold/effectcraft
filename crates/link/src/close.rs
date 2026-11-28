//! `link.close`: FilmCraft undid the hand-off, so the comp is no longer a link.
//!
//! The binding goes. The comp, its layers and every edit stay in the project, so Undo in FilmCraft
//! never throws EffectCraft work away. `edited` tells FilmCraft that the comp has work it should
//! name to the user: the live link counts an epoch for each result render, and the first edit
//! after the hand-off starts one.

use effectcraft_engine::Session;
use serde_json::{Value, json};

use crate::attach::AttachError;

/// Unbind `params.link_id`. The reply carries the comp id and name and `edited`.
pub fn close_link(session: &mut Session, params: &Value) -> Result<Value, AttachError> {
    let link_id = params.get("link_id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| AttachError::bad_params("`link_id` is required"))?;
    let binding = session
        .project_mut()
        .craft_links
        .remove(link_id)
        .ok_or_else(|| AttachError { code: "link_unknown", message: format!("no composition is bound to link {link_id}") })?;
    let name = session.project.item(binding.comp).map(|item| item.name.clone()).unwrap_or_default();
    Ok(json!({"link_id": link_id, "comp": binding.comp.0, "comp_name": name, "edited": binding.epoch > 0, "kept": true}))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use effectcraft_engine::color::Label;
    use effectcraft_project::{Comp, CraftLinkBinding, ItemKind};
    use effectcraft_time::FrameRate;

    use super::*;

    const LINK: &str = "66666666-6666-4666-8666-666666666666";

    fn bound(epoch: u64) -> (Session, effectcraft_project::ItemId) {
        let mut session = Session::default();
        let rate = FrameRate::new(24, 1);
        let comp = session.project_mut().add_item("Shot (nest)", Label::Sandstone, None, ItemKind::Comp(Arc::new(Comp::new(64, 36, rate, rate.tick_of(1)))));
        session.project_mut().craft_links.insert(
            LINK.into(),
            CraftLinkBinding {
                link_id: LINK.into(),
                comp,
                source: json!({"app": "filmcraft", "doc_id": "0", "item": "1"}),
                result_path: "/tmp/never.mov".into(),
                epoch,
            },
        );
        (session, comp)
    }

    #[test]
    fn close_unbinds_and_keeps_the_comp() {
        let (mut session, comp) = bound(0);
        let reply = close_link(&mut session, &json!({"link_id": LINK})).unwrap();
        assert!(session.project.craft_links.is_empty());
        assert!(session.project.comp(comp).is_some(), "the comp must stay: it may hold the user's work");
        assert_eq!(reply["comp_name"], "Shot (nest)");
        assert_eq!(reply["edited"], false);
    }

    #[test]
    fn a_comp_with_a_result_render_counts_as_edited() {
        let (mut session, _) = bound(3);
        assert_eq!(close_link(&mut session, &json!({"link_id": LINK})).unwrap()["edited"], true);
    }

    #[test]
    fn closing_an_unknown_link_says_so() {
        let (mut session, _) = bound(0);
        let err = close_link(&mut session, &json!({"link_id": "nope"})).unwrap_err();
        assert_eq!(err.code, "link_unknown");
        assert_eq!(close_link(&mut session, &json!({})).unwrap_err().code, "bad_params");
        assert_eq!(session.project.craft_links.len(), 1);
    }
}
