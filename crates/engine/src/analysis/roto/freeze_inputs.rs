use std::collections::{BTreeMap, HashMap, HashSet};

use effectcraft_effects::layer_source_id;
use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, Footage, ItemId, ItemKind, Layer, LayerId, Node, Project, PropGroup, Uid, essential};
use sha2::{Digest, Sha256};

use crate::{Services, doc_path::StoredPaths};

pub(super) type EffectId = (ItemId, LayerId, Uid);

/// An expression can select pixel reads through a computed method name, parent transform,
/// control layer or another composition. A composition with active expressions cannot provide
/// a complete file provenance from the Layers and effect properties alone.
pub(super) fn comps_with_unverified_image_reads(p: &Project) -> HashSet<ItemId> {
    let mut expression_comps = HashSet::new();
    for (&cid, comp) in p.comps() {
        for layer in &comp.layers {
            layer.props.walk("", &mut |_, prop| {
                if prop.has_expression() {
                    expression_comps.insert(cid);
                }
            });
        }
    }
    expression_comps
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Effects {
    Before(Uid),
    All,
    None,
}

/// Dependencies of an Object Matte input across its span. The owning effect sees its own source
/// and only the stack above it. A referenced layer may be read with or without its effects.
pub(super) struct FreezeInputs<'a> {
    pub(super) effects: HashSet<EffectId>,
    // ID order is stable when import offsets IDs. Neither the IDs nor the paths enter the digest.
    footage: BTreeMap<ItemId, &'a Footage>,
    complete: bool,
}

impl<'a> FreezeInputs<'a> {
    pub(super) fn collect(p: &'a Project, (comp, layer, effect): EffectId, unverified_image_reads: &HashSet<ItemId>) -> Self {
        let mut inputs = Self { effects: HashSet::new(), footage: BTreeMap::new(), complete: true };
        let mut pending = vec![(comp, layer, Effects::Before(effect))];
        let mut visited = HashSet::new();
        while let Some((cid, lid, scope)) = pending.pop() {
            if !visited.insert((cid, lid, scope)) {
                continue;
            }
            let Some(l) = p.comp(cid).and_then(|c| c.layer(lid)) else { continue };
            inputs.complete &= !unverified_image_reads.contains(&cid);
            if let Some(item_id) = l.source.item()
                && let Some(item) = p.item(item_id)
            {
                if let Some(proxy) = item.proxy.as_ref().filter(|proxy| proxy.enabled && proxy.footage.has_video) {
                    inputs.footage.insert(item_id, &proxy.footage);
                } else {
                    match &item.kind {
                        ItemKind::Footage(f) => {
                            inputs.footage.insert(item_id, f);
                        }
                        ItemKind::Comp(nested) => {
                            inputs.include_media_replacements(p, l, nested);
                            let solo = nested.any_solo();
                            for child in nested.layers.iter().filter(|l| l.has_video() && !l.environment && !l.switches.guide && (!solo || l.switches.solo)) {
                                pending.push((item_id, child.id, Effects::All));
                                if let Some(matte) = child.track_matte {
                                    pending.push((item_id, matte.layer, Effects::All));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if scope != Effects::None
                && l.switches.effects
                && let Some(fx) = l.effects()
            {
                for group in fx.groups() {
                    if scope == Effects::Before(group.uid) {
                        break;
                    }
                    if group.enabled {
                        inputs.effects.insert((cid, lid, group.uid));
                        inputs.complete &= layer_reads(group, cid, lid, &mut pending);
                    }
                }
            }
        }
        inputs
    }

    /// Instance overrides can replace a nested layer's file. Include those files in addition
    /// to the originals, since the same comp may be read by different instances.
    fn include_media_replacements(&mut self, p: &'a Project, layer: &Layer, nested: &Comp) {
        let Some(controls) = nested.essential.as_ref() else { return };
        let Some(group) = essential::group(layer) else { return };
        let overridden = essential::overridden(layer);
        group.walk("", &mut |_, prop| {
            if !overridden.contains(&prop.uid) {
                return;
            }
            let Some(control) = essential::control_of(&prop.match_id).and_then(|id| controls.resolve(id)) else { return };
            match control.kind {
                essential::EgKind::Media { .. } => {
                    if prop.has_expression() || !prop.keys.is_empty() {
                        self.complete = false;
                    } else {
                        let id = ItemId(prop.value.as_f64().max(0.0) as u64);
                        if let Some(item) = p.item(id)
                            && let ItemKind::Footage(footage) = &item.kind
                        {
                            let active = item.proxy.as_ref().filter(|proxy| proxy.enabled && proxy.footage.has_video).map_or(footage, |proxy| &proxy.footage);
                            self.footage.insert(id, active);
                        }
                    }
                }
                essential::EgKind::Property { .. } if matches!(prop.value, Value::Layer(_)) => self.complete = false,
                _ => {}
            }
        });
    }

    /// Some empty fingerprint means no external files. None means at least one file could
    /// not be verified. The digest includes the files of an image sequence in playback order.
    pub(super) fn media_fingerprint(&self, services: &dyn Services, paths: &StoredPaths, files: &mut HashMap<String, Option<[u8; 32]>>) -> Option<String> {
        if !self.complete {
            return None;
        }
        let mut fingerprint = Sha256::new();
        let mut external = false;
        for f in self.footage.values() {
            let sources = if f.sequence.is_empty() { std::slice::from_ref(&f.path) } else { f.sequence.as_slice() };
            if sources.iter().all(String::is_empty) {
                continue;
            }
            if f.missing {
                return None;
            }
            external = true;
            fingerprint.update((sources.len() as u64).to_le_bytes());
            for path in sources {
                let resolved = paths.follow(path).ok()?;
                let resolved = resolved.to_str()?;
                let digest = files.entry(resolved.to_owned()).or_insert_with(|| services.file_fingerprint(resolved).ok());
                fingerprint.update((*digest)?);
            }
        }
        Some(if external { format!("{:x}", fingerprint.finalize()) } else { String::new() })
    }
}

fn layer_reads(group: &PropGroup, comp: ItemId, owner: LayerId, out: &mut Vec<(ItemId, LayerId, Effects)>) -> bool {
    let mut complete = true;
    for node in &group.children {
        match node {
            Node::Prop(p) => {
                if !matches!(p.value, Value::Layer(_)) {
                    continue;
                }
                complete &= !p.has_expression();
                let include_effects = group
                    .get(&layer_source_id(&p.match_id))
                    .is_none_or(|mode| mode.value.as_enum() >= 2 || mode.has_expression() || mode.keys.iter().any(|k| k.value.as_enum() >= 2));
                let scope = if include_effects { Effects::All } else { Effects::None };
                for id in std::iter::once(&p.value).chain(p.keys.iter().map(|key| &key.value)).filter_map(Value::as_layer).map(LayerId) {
                    if id != owner {
                        out.push((comp, id, scope));
                    }
                }
            }
            Node::Group(child) if child.enabled => complete &= layer_reads(child, comp, owner, out),
            _ => {}
        }
    }
    complete
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    #[test]
    fn media_replacement_changes_the_instance_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.png");
        let replacement = dir.path().join("replacement.png");
        std::fs::write(&original, b"old file").unwrap();
        std::fs::write(&replacement, b"new file").unwrap();

        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "Inner", "width": 16, "height": 16})).unwrap();
        let inner = s.active_comp_id().unwrap();
        let (original_item, replacement_item) = s
            .edit("import media", None, |p, _| {
                let mut add = |path: &std::path::Path| {
                    let footage = Footage { path: path.to_string_lossy().into_owned(), width: 16, height: 16, has_video: true, ..Default::default() };
                    p.add_item(&footage.path.clone(), Default::default(), None, ItemKind::Footage(footage))
                };
                Ok((add(&original), add(&replacement)))
            })
            .unwrap();
        let child = s.execute("layer.addItem", json!({"item": original_item.0})).unwrap()["layer"].as_u64().unwrap();
        let control = s.execute("essential.addMedia", json!({"layer": child})).unwrap()["control"].as_u64().unwrap();
        s.execute("comp.new", json!({"name": "Outer", "width": 16, "height": 16})).unwrap();
        let outer = s.active_comp_id().unwrap();
        let instance = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
        s.execute("essential.set", json!({"layer": instance, "control": control, "item": replacement_item.0})).unwrap();
        let matte = s.execute("objectMatte.apply", json!({"layer": instance, "point": [8, 8]})).unwrap()["effect"].as_u64().unwrap();
        let inputs = FreezeInputs::collect(&s.project, (outer, LayerId(instance), matte), &comps_with_unverified_image_reads(&s.project));
        let fingerprint = || inputs.media_fingerprint(s.services.as_ref(), &s.stored_paths, &mut HashMap::new()).unwrap();
        let before = fingerprint();
        std::fs::write(&replacement, b"bad file").unwrap();
        assert_ne!(before, fingerprint());
    }
}
