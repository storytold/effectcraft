//! Proxies (File ▸ Create Proxy / Set Proxy / Interpret Footage ▸ Proxy): a stand-in file for
//! a footage item or a composition. The Project panel's proxy switch (`file.useProxy`) turns
//! it on or off per item; renders follow Render Settings ▸ Proxy Use (the viewer uses each
//! item's switch).

use effectcraft_project::render_queue::{Channels, OutputFormat, OutputModule, PostRenderAction, ProResProfile, RenderQuality, RenderQueueItem, TimeSpan};
use effectcraft_project::render_templates::TemplateSlot;
use effectcraft_project::{Footage, ItemId, ItemKind, Proxy};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, has_project_selection, str_p};
use crate::{EngineError, Result, Session, cmd};

/// Footage items and comps a proxy command acts on: `items`/`item`, else the Project panel
/// selection.
fn targets(s: &Session, p: &Value) -> Vec<ItemId> {
    let ids: Vec<ItemId> = match p.get("items").or(p.get("item")) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(ItemId).collect(),
        Some(Value::Number(n)) => n.as_u64().map(ItemId).into_iter().collect(),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id).into_iter().collect(),
        _ => s.state.project_selection.clone(),
    };
    ids.into_iter().filter(|i| matches!(s.project.item(*i).map(|x| &x.kind), Some(ItemKind::Footage(_) | ItemKind::Comp(_)))).collect()
}

fn has_targets(s: &Session) -> std::result::Result<(), String> {
    has_project_selection(s)?;
    if targets(s, &Value::Null).is_empty() { Err("select footage or a composition in the Project panel".into()) } else { Ok(()) }
}

fn has_proxy(s: &Session) -> std::result::Result<(), String> {
    has_targets(s)?;
    if targets(s, &Value::Null).iter().any(|i| s.project.item(*i).is_some_and(|x| x.proxy.is_some())) {
        Ok(())
    } else {
        Err("the selected items have no proxy".into())
    }
}

/// Probe a proxy file through the importer.
fn probe(s: &Session, path: &str, cmd: &str) -> Result<Footage> {
    if s.importer.is_none() {
        return Err(EngineError::Other("media import is not available in this build".into()));
    }
    // The first frame of a proxy sequence (Create Proxy ▸ Movie renders one) brings the rest.
    let f = s.probe_footage(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    if !f.has_video {
        return Err(bad(cmd, format!("{path} has no video to use as a proxy")));
    }
    Ok(f)
}

/// Attach `footage` as the proxy of `items` (Use Proxy on). One undo step.
pub(crate) fn set_proxy_footage(s: &mut Session, items: &[ItemId], footage: Footage) -> Result<()> {
    s.edit("Set Proxy", None, |proj, _| {
        for i in items {
            if let Some(it) = proj.item_mut(*i) {
                it.proxy = Some(Box::new(Proxy { footage: footage.clone(), enabled: true }));
            }
        }
        Ok(())
    })
}

fn set_proxy(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.setProxy";
    let items = targets(s, p);
    if items.is_empty() {
        return Err(bad(C, "select footage or a composition"));
    }
    let path = str_p(p, "path").ok_or_else(|| bad(C, "missing `path`"))?.to_string();
    let f = probe(s, &path, C)?;
    let (w, h) = (f.width, f.height);
    set_proxy_footage(s, &items, f)?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "path": path, "width": w, "height": h}))
}

fn set_proxy_none(s: &mut Session, p: &Value) -> Result<Value> {
    let items = targets(s, p);
    s.edit("Set Proxy None", None, |proj, _| {
        for i in &items {
            if let Some(it) = proj.item_mut(*i) {
                it.proxy = None;
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

/// The Project panel's proxy switch: use the proxy (filled box) or the full-resolution source.
fn use_proxy(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.useProxy";
    let items: Vec<ItemId> = targets(s, p).into_iter().filter(|i| s.project.item(*i).is_some_and(|x| x.proxy.is_some())).collect();
    if items.is_empty() {
        return Err(bad(C, "the selected items have no proxy"));
    }
    let first = s.project.item(items[0]).and_then(|x| x.proxy.as_ref()).is_some_and(|px| px.enabled);
    let on = b_p(p, "on").unwrap_or(!first);
    s.edit(if on { "Use Proxy" } else { "Use Full Resolution" }, None, |proj, _| {
        for i in &items {
            if let Some(px) = proj.item_mut(*i).and_then(|x| x.proxy.as_mut()) {
                px.enabled = on;
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "on": on}))
}

/// File ▸ Interpret Footage ▸ Proxy: Interpret Footage settings for the proxy file.
fn interpret_proxy(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.interpretProxy";
    let items: Vec<ItemId> = targets(s, p).into_iter().filter(|i| s.project.item(*i).is_some_and(|x| x.proxy.is_some())).collect();
    if items.is_empty() {
        return Err(bad(C, "the selected items have no proxy"));
    }
    let it = super::file_more::Interpretation::parse(p, C)?;
    let guesses: Vec<_> = items
        .iter()
        .map(|i| {
            let px = s.project.item(*i).and_then(|x| x.proxy.as_ref());
            match (p.get("alpha").and_then(Value::as_str) == Some("guess") || b_p(p, "guessAlpha") == Some(true), px) {
                (true, Some(px)) => super::file_more::guess_alpha(s, *i, &px.footage),
                _ => None,
            }
        })
        .collect();
    s.edit("Interpret Proxy", None, |proj, _| {
        for (i, g) in items.iter().zip(&guesses) {
            if let Some(px) = proj.item_mut(*i).and_then(|x| x.proxy.as_mut()) {
                it.apply(&mut px.footage, *g);
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

/// File ▸ Create Proxy ▸ Still / Movie: queue a half-resolution render of the composition whose
/// post-render action sets the result as its proxy.
fn create_proxy(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.createProxy";
    let cid = match p.get("comp").or(p.get("item")) {
        Some(_) => {
            let q = json!({"comp": p.get("comp").or(p.get("item")).cloned()});
            super::comp_id(s, &q)?
        }
        None => s
            .state
            .project_selection
            .iter()
            .copied()
            .find(|i| s.project.comp(*i).is_some())
            .or(s.active_comp_id())
            .ok_or_else(|| bad(C, "select a composition"))?,
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let still = match str_p(p, "kind").unwrap_or("movie") {
        "still" => true,
        "movie" => false,
        k => return Err(bad(C, format!("kind: still|movie, not `{k}`"))),
    };
    let mut it = RenderQueueItem::new(0, cid);
    // The Movie / Still Proxy Default templates, at draft quality and the proxy resolution.
    let templates = &s.project.render_templates;
    let slot = if still { TemplateSlot::StillProxy } else { TemplateSlot::MovieProxy };
    it.settings = templates.default_render_settings(slot);
    it.settings.quality = RenderQuality::Draft;
    it.settings.resolution = p.get("resolution").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.05, 1.0);
    if still {
        // One frame at the current time (the comp's poster frame when not the active comp).
        let t = if s.active_comp_id() == Some(cid) { s.time() } else { comp.poster_time };
        it.settings.time_span = TimeSpan::Custom { start: t, end: t + comp.frame_duration() };
        it.output = templates.default_output_module(slot);
        if !it.output.format.is_sequence() {
            it.output = OutputModule::for_format(OutputFormat::PngSequence);
        }
        it.output.output = "[compName]_proxy_[#####].[fileExtension]".into();
    } else {
        it.settings.time_span = TimeSpan::LengthOfComp;
        it.output = templates.default_output_module(slot);
        if !it.output.format.is_movie() {
            it.output = OutputModule::for_format(OutputFormat::ProRes);
            it.output.prores_profile = ProResProfile::P4444;
        }
        it.output.output = "[compName]_proxy.[fileExtension]".into();
    }
    it.output.channels = Channels::Rgba;
    if let Some(o) = str_p(p, "path").or(str_p(p, "output")) {
        it.output.output = o.to_string();
    }
    it.post_render = PostRenderAction::SetProxy;
    let id = s.edit("Create Proxy", None, |proj, _| {
        it.id = proj.render_queue.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        let id = it.id;
        proj.render_queue.push(it);
        Ok(id)
    })?;
    let path = s.project.render_queue.last().and_then(|i| s.resolve_output(i));
    Ok(json!({"item": id, "comp": cid.0, "kind": if still { "still" } else { "movie" }, "output": path, "time": Tick::ZERO.seconds()}))
}

fn has_comp_selection(s: &Session) -> std::result::Result<(), String> {
    if s.state.project_selection.iter().any(|i| s.project.comp(*i).is_some()) || s.active_comp_id().is_some() {
        Ok(())
    } else {
        Err("select a composition".into())
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.createProxy",
            "Create Proxy",
            [],
            None,
            "{kind: still|movie, comp?|item?, path?|output? (template), resolution? (default 0.5)}",
            has_comp_selection,
            create_proxy
        ),
        cmd!("file.setProxy", "File...", ["File", "Set Proxy"], None, "{path, items?|item?}", has_targets, set_proxy),
        cmd!("file.setProxyNone", "None", ["File", "Set Proxy"], None, "{items?|item?}", has_proxy, set_proxy_none),
        cmd!("file.useProxy", "Use Proxy", [], None, "{items?|item?, on?}", has_proxy, use_proxy),
        cmd!(
            "file.interpretProxy",
            "Proxy...",
            ["File", "Interpret Footage"],
            None,
            "{items?|item?, alpha?, matteColor?, invertAlpha?, frameRate?, loop?, pixelAspect?, fields?, colorProfile?, linearLight?}",
            has_proxy,
            interpret_proxy
        ),
    ]
}
