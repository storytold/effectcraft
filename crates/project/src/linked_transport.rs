//! Render workers receive bounded shared source snapshots; project files keep references.

use crate::{FootageKind, ItemId, ItemKind, Project, ProjectError};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Arc;

const VERSION: u32 = 1;
const MAX_LINKS: usize = 64;
const MAX_SOURCES: usize = 8;
const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RENDER_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCE_ITEMS: usize = 4096;
const MAX_SOURCE_LAYERS: usize = 8192;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkSnapshot {
    source: Option<usize>,
    resolved_comp: Option<ItemId>,
    error: Option<String>,
    generation: u64,
}

#[derive(Serialize)]
struct RenderProject<'a> {
    #[serde(flatten)]
    project: &'a Project,
    #[serde(rename = "__linkedTransport", skip_serializing_if = "Option::is_none")]
    transport: Option<Transport<'a>>,
}

#[derive(Serialize)]
struct Transport<'a> {
    version: u32,
    sources: &'a [Box<RawValue>],
    links: BTreeMap<ItemId, LinkSnapshot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BorrowedTransport<'a> {
    version: u32,
    #[serde(borrow)]
    sources: RawSources<'a>,
    links: Links,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "__linkedTransport", default, borrow)]
    transport: Option<BorrowedTransport<'a>>,
    #[serde(rename = "__linkedSnapshots", default, borrow)]
    obsolete: Option<&'a RawValue>,
}

// Borrow raw objects and stop at the limit before allocating any source Project.
struct RawSources<'a>(Vec<&'a RawValue>);
impl<'de: 'a, 'a> Deserialize<'de> for RawSources<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourcesVisitor;
        impl<'de> Visitor<'de> for SourcesVisitor {
            type Value = RawSources<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded source snapshot list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut sources = Vec::new();
                while let Some(source) = seq.next_element::<&RawValue>()? {
                    if sources.len() == MAX_SOURCES {
                        return Err(de::Error::custom("too many linked source snapshots"));
                    }
                    if source.get().len() > MAX_SOURCE_BYTES {
                        return Err(de::Error::custom("linked source snapshot exceeds the byte limit"));
                    }
                    sources.push(source);
                }
                Ok(RawSources(sources))
            }
        }
        deserializer.deserialize_seq(SourcesVisitor)
    }
}

struct Links(BTreeMap<ItemId, LinkSnapshot>);
impl<'de> Deserialize<'de> for Links {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LinksVisitor;
        impl<'de> Visitor<'de> for LinksVisitor {
            type Value = Links;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded map of unique linked item references")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut links = BTreeMap::new();
                while let Some(id) = map.next_key::<ItemId>()? {
                    if links.len() == MAX_LINKS {
                        return Err(de::Error::custom("too many linked composition references"));
                    }
                    if links.insert(id, map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate linked item reference"));
                    }
                }
                Ok(Links(links))
            }
        }
        deserializer.deserialize_map(LinksVisitor)
    }
}

impl Project {
    /// Render transport with one copy of each resolved source. Actual snapshot content,
    /// rather than project ID, distinguishes divergent saved copies of one source project.
    pub fn to_render_json(&self) -> Result<String, ProjectError> {
        let mut sources: Vec<Arc<Project>> = Vec::new();
        let mut links = BTreeMap::new();
        for item in self.items.values() {
            let ItemKind::Footage(f) = &item.kind else { continue };
            if f.kind != FootageKind::LinkedComposition {
                continue;
            }
            if links.len() == MAX_LINKS {
                return Err(invalid("too many linked composition references"));
            }
            let link = f.linked_comp.as_ref().ok_or_else(|| invalid("linked footage has no composition reference"))?;
            let index = match (&link.resolved, link.resolved_comp) {
                (Some(source), Some(comp)) => {
                    validate_reference(source, comp, link)?;
                    if let Some(index) = sources.iter().position(|saved| Arc::ptr_eq(saved, source) || saved.same_content(source)) {
                        Some(index)
                    } else {
                        if sources.len() == MAX_SOURCES {
                            return Err(invalid("too many linked source snapshots"));
                        }
                        validate_source(source)?;
                        sources.push(source.clone());
                        Some(sources.len() - 1)
                    }
                }
                (None, None) => None,
                _ => return Err(invalid("render snapshot is missing its source project or composition")),
            };
            links.insert(item.id, LinkSnapshot { source: index, resolved_comp: link.resolved_comp, error: link.error.clone(), generation: link.generation });
        }
        let mut raw_sources = Vec::new();
        let mut source_bytes = 0usize;
        for source in sources {
            let text = bounded_json(&source, MAX_SOURCE_BYTES.min(MAX_RENDER_BYTES - source_bytes))?;
            source_bytes += text.len();
            raw_sources.push(RawValue::from_string(text).map_err(invalid)?);
        }
        let transport = (!links.is_empty()).then_some(Transport { version: VERSION, sources: &raw_sources, links });
        bounded_json(&RenderProject { project: self, transport }, MAX_RENDER_BYTES)
    }

    /// Structured worker synchronization with the same byte and count limits.
    pub fn render_json_value(&self) -> Result<serde_json::Value, ProjectError> {
        serde_json::from_str(&self.to_render_json()?).map_err(invalid)
    }

    /// Decode a patched worker replica without first allocating an unbounded JSON string.
    pub fn from_render_value(value: &serde_json::Value) -> Result<Self, ProjectError> {
        Self::from_render_json(&bounded_json(value, MAX_RENDER_BYTES)?)
    }

    /// Restore snapshots matching the link identities. Bounds are checked before decoding
    /// unique source projects; ordinary project JSON is also accepted.
    pub fn from_render_json(text: &str) -> Result<Self, ProjectError> {
        if text.len() > MAX_RENDER_BYTES {
            return Err(invalid("render project exceeds the byte limit"));
        }
        let envelope: Envelope<'_> = serde_json::from_str(text).map_err(invalid)?;
        if envelope.obsolete.is_some() {
            return Err(invalid("obsolete linked snapshot transport; resend the project"));
        }
        let Some(transport) = envelope.transport else { return Self::from_json(text) };
        if transport.version != VERSION {
            return Err(invalid("unsupported linked snapshot transport version"));
        }
        let used: BTreeSet<usize> = transport.links.0.values().filter_map(|link| link.source).collect();
        if used.len() != transport.sources.0.len() || used.iter().copied().ne(0..transport.sources.0.len()) {
            return Err(invalid("linked source table contains missing or unused references"));
        }
        let mut project = Self::from_json(text)?;
        let linked_count = project.items.values().filter(|item| matches!(&item.kind, ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition)).count();
        if linked_count != transport.links.0.len() {
            return Err(invalid("render transport is missing a linked item reference"));
        }
        for (id, snapshot) in &transport.links.0 {
            if !matches!(project.item(*id).map(|item| &item.kind), Some(ItemKind::Footage(f)) if f.kind == FootageKind::LinkedComposition && f.linked_comp.is_some())
            {
                return Err(invalid("render snapshot does not name linked footage"));
            }
            if snapshot.source.is_some() != snapshot.resolved_comp.is_some() {
                return Err(invalid("render snapshot is missing its source project or composition"));
            }
        }
        for raw in &transport.sources.0 {
            preflight_source(raw.get())?;
        }
        let mut sources = Vec::new();
        for raw in transport.sources.0 {
            let source = Self::from_json(raw.get())?;
            validate_source(&source)?;
            sources.push(Arc::new(source));
        }
        for (id, snapshot) in transport.links.0 {
            let Some(ItemKind::Footage(f)) = project.item_mut(id).map(|item| &mut item.kind) else {
                return Err(invalid("render snapshot does not name linked footage"));
            };
            if f.kind != FootageKind::LinkedComposition {
                return Err(invalid("render snapshot does not name linked footage"));
            }
            let link = f.linked_comp.as_mut().ok_or_else(|| invalid("render snapshot has no composition reference"))?;
            link.resolved = match (snapshot.source, snapshot.resolved_comp) {
                (Some(index), Some(comp)) => {
                    let source = sources.get(index).ok_or_else(|| invalid("linked source reference is out of range"))?;
                    validate_reference(source, comp, link)?;
                    link.source_color = Some(source.settings.linked_color_space());
                    Some(source.clone())
                }
                (None, None) => None,
                _ => return Err(invalid("render snapshot is missing its source project or composition")),
            };
            link.resolved_comp = snapshot.resolved_comp;
            link.error = snapshot.error;
            link.generation = snapshot.generation;
        }
        Ok(project)
    }
}

fn validate_reference(source: &Project, comp: ItemId, link: &crate::LinkedComposition) -> Result<(), ProjectError> {
    if source.project_id.is_empty()
        || source.project_id != link.source_project_id
        || source.comp(comp).is_none()
        || !source.item(comp).is_some_and(|item| !item.stable_id.is_empty() && item.stable_id == link.composition_id)
    {
        return Err(invalid("render snapshot does not match its linked composition identity"));
    }
    Ok(())
}

fn validate_source(source: &Project) -> Result<(), ProjectError> {
    if source.items.len() > MAX_SOURCE_ITEMS
        || source.items.values().filter_map(|item| source.comp(item.id)).map(|comp| comp.layers.len()).sum::<usize>() > MAX_SOURCE_LAYERS
    {
        return Err(invalid("linked source snapshot exceeds its item or layer limit"));
    }
    let mut identities = BTreeSet::new();
    for item in source.items.values() {
        if item.stable_id.is_empty() || !identities.insert(&item.stable_id) {
            return Err(invalid("linked source snapshot has missing or duplicate item identities"));
        }
        let linked = |f: &crate::Footage| f.kind == FootageKind::LinkedComposition || f.linked_comp.is_some();
        if matches!(&item.kind, ItemKind::Footage(f) if linked(f)) || item.proxy.as_ref().is_some_and(|proxy| linked(&proxy.footage)) {
            return Err(invalid("nested linked compositions are not supported by this prototype"));
        }
    }
    Ok(())
}

// Borrow the source item objects, and count layers without constructing them. Even a small
// hostile source cannot allocate millions of model objects before its limits are checked.
fn preflight_source(text: &str) -> Result<(), ProjectError> {
    #[derive(Deserialize)]
    struct Source<'a> {
        #[serde(borrow)]
        items: RawItems<'a>,
        #[serde(rename = "__linkedTransport", default, borrow)]
        transport: Option<&'a RawValue>,
        #[serde(rename = "__linkedSnapshots", default, borrow)]
        obsolete: Option<&'a RawValue>,
    }
    #[derive(Deserialize)]
    struct ItemShape {
        kind: KindShape,
    }
    #[derive(Deserialize)]
    struct KindShape {
        #[serde(default)]
        layers: LayerCount,
    }
    let source: Source<'_> = serde_json::from_str(text).map_err(invalid)?;
    if source.transport.is_some() || source.obsolete.is_some() {
        return Err(invalid("a source snapshot cannot contain another transport envelope"));
    }
    let mut layers = 0usize;
    for item in source.items.0 {
        let shape: ItemShape = serde_json::from_str(item.get()).map_err(invalid)?;
        layers += shape.kind.layers.0;
        if layers > MAX_SOURCE_LAYERS {
            return Err(invalid("linked source snapshot exceeds its layer limit"));
        }
    }
    Ok(())
}

struct RawItems<'a>(Vec<&'a RawValue>);
impl<'de: 'a, 'a> Deserialize<'de> for RawItems<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ItemsVisitor;
        impl<'de> Visitor<'de> for ItemsVisitor {
            type Value = RawItems<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded map of source items")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                let mut ids = BTreeSet::new();
                while let Some(id) = map.next_key::<ItemId>()? {
                    if items.len() == MAX_SOURCE_ITEMS || !ids.insert(id) {
                        return Err(de::Error::custom("too many or duplicate linked source items"));
                    }
                    items.push(map.next_value()?);
                }
                Ok(RawItems(items))
            }
        }
        deserializer.deserialize_map(ItemsVisitor)
    }
}

#[derive(Default)]
struct LayerCount(usize);
impl<'de> Deserialize<'de> for LayerCount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LayersVisitor;
        impl<'de> Visitor<'de> for LayersVisitor {
            type Value = LayerCount;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded layer list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut count = 0;
                while seq.next_element::<de::IgnoredAny>()?.is_some() {
                    count += 1;
                    if count > MAX_SOURCE_LAYERS {
                        return Err(de::Error::custom("linked source snapshot exceeds its layer limit"));
                    }
                }
                Ok(LayerCount(count))
            }
        }
        deserializer.deserialize_seq(LayersVisitor)
    }
}

fn bounded_json(value: &impl Serialize, limit: usize) -> Result<String, ProjectError> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("render project exceeds the byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer { bytes: Vec::new(), limit };
    serde_json::to_writer(&mut buffer, value).map_err(invalid)?;
    String::from_utf8(buffer.bytes).map_err(invalid)
}

fn invalid(error: impl std::fmt::Display) -> ProjectError {
    ProjectError::Invalid(format!("invalid render project: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Comp, Footage, LinkedComposition};
    use effectcraft_color::Label;
    use effectcraft_time::{FrameRate, Tick};

    fn source() -> (Arc<Project>, ItemId) {
        let mut source = Project::default();
        let comp =
            source.add_item("Private source", Label::Brown, None, ItemKind::Comp(Comp::new(32, 24, FrameRate::FPS_24, Tick::from_seconds_f64(1.0)).into()));
        source.item_mut(comp).unwrap().comment = "source payload".repeat(20_000);
        (source.into(), comp)
    }

    fn link(destination: &mut Project, source: Arc<Project>, comp: ItemId, path: &str) -> ItemId {
        destination.add_item(
            "Linked",
            Label::Brown,
            None,
            ItemKind::Footage(Footage {
                path: path.into(),
                kind: FootageKind::LinkedComposition,
                linked_comp: Some(LinkedComposition {
                    source_project_id: source.project_id.clone(),
                    composition_id: source.item(comp).unwrap().stable_id.clone(),
                    last_known_name: "Private source".into(),
                    source_color: Some(source.settings.linked_color_space()),
                    resolved: Some(source),
                    resolved_comp: Some(comp),
                    generation: 7,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
    }

    fn resolved(project: &Project, id: ItemId) -> &Arc<Project> {
        let ItemKind::Footage(f) = &project.item(id).unwrap().kind else { panic!("linked footage") };
        f.linked_comp.as_ref().unwrap().resolved.as_ref().unwrap()
    }

    #[test]
    fn repeated_links_share_one_serialized_and_decoded_snapshot() {
        let (source, comp) = source();
        let mut project = Project::default();
        let first = link(&mut project, source.clone(), comp, "Titles.ecproj");
        let single = project.to_render_json().unwrap();
        let mut last = first;
        for _ in 1..MAX_LINKS {
            last = link(&mut project, source.clone(), comp, "Titles.ecproj");
        }
        let many = project.to_render_json().unwrap();
        assert!(many.len() < single.len() * 2, "adding instances must not duplicate the source payload");
        let worker = Project::from_render_json(&many).unwrap();
        assert!(Arc::ptr_eq(resolved(&worker, first), resolved(&worker, last)));
        assert!(!project.to_file_json().unwrap().contains("source payload"), "saved project files stay reference-only");
        link(&mut project, source, comp, "Titles.ecproj");
        assert!(project.to_render_json().is_err(), "sender refuses excess references");
    }

    #[test]
    fn divergent_saved_copies_with_shared_identity_stay_distinct() {
        let (source, comp) = source();
        let mut copy = (*source).clone();
        copy.item_mut(comp).unwrap().name = "Changed copy".into();
        let mut project = Project::default();
        let original = link(&mut project, source, comp, "Original.ecproj");
        let changed = link(&mut project, copy.into(), comp, "Copy.ecproj");
        let worker = Project::from_render_json(&project.to_render_json().unwrap()).unwrap();
        assert_ne!(resolved(&worker, original).item(comp).unwrap().name, resolved(&worker, changed).item(comp).unwrap().name);
        assert!(!Arc::ptr_eq(resolved(&worker, original), resolved(&worker, changed)));
    }

    #[test]
    fn malformed_transport_is_rejected_without_decoding_sources() {
        let (source, comp) = source();
        let mut project = Project::default();
        let item = link(&mut project, source, comp, "Titles.ecproj");
        let valid = project.render_json_value().unwrap();
        for change in ["index", "missing", "unused", "version", "identity", "nested"] {
            let mut wrong = valid.clone();
            match change {
                "index" => wrong["__linkedTransport"]["links"][item.0.to_string()]["source"] = 4.into(),
                "missing" => {
                    wrong["__linkedTransport"]["links"].as_object_mut().unwrap().clear();
                }
                "unused" => wrong["__linkedTransport"]["sources"].as_array_mut().unwrap().push(serde_json::json!({})),
                "version" => wrong["__linkedTransport"]["version"] = 999.into(),
                "identity" => wrong["__linkedTransport"]["sources"][0]["project_id"] = "unrelated".into(),
                "nested" => wrong["__linkedTransport"]["sources"][0]["__linkedTransport"] = serde_json::json!({}),
                _ => panic!("unknown test mutation"),
            }
            assert!(Project::from_render_json(&wrong.to_string()).is_err(), "{change}");
        }
        let source_objects = std::iter::repeat_n("{}", MAX_SOURCES + 1).collect::<Vec<_>>().join(",");
        let too_many = format!(r#"{{"__linkedTransport":{{"version":1,"sources":[{source_objects}],"links":{{}}}}}}"#);
        assert!(Project::from_render_json(&too_many).unwrap_err().to_string().contains("too many linked source"));
        let duplicate = r#"{"__linkedTransport":{"version":1,"sources":[],"links":{"1":{"source":null,"resolved_comp":null,"error":null,"generation":0},"1":{"source":null,"resolved_comp":null,"error":null,"generation":0}}}}"#;
        assert!(Project::from_render_json(duplicate).unwrap_err().to_string().contains("duplicate linked item"));
        assert!(bounded_json(&project, 10).is_err(), "streaming writer stops at its byte budget");
    }

    #[test]
    fn source_item_and_layer_counts_are_checked_before_model_decode() {
        let items = (0..=MAX_SOURCE_ITEMS).map(|id| format!(r#""{id}":{{}}"#)).collect::<Vec<_>>().join(",");
        let too_many_items = format!(r#"{{"items":{{{items}}}}}"#);
        assert!(preflight_source(&too_many_items).unwrap_err().to_string().contains("source items"));
        let layers = std::iter::repeat_n("{}", MAX_SOURCE_LAYERS + 1).collect::<Vec<_>>().join(",");
        let too_many_layers = format!(r#"{{"items":{{"1":{{"kind":{{"layers":[{layers}]}}}}}}}}"#);
        assert!(preflight_source(&too_many_layers).unwrap_err().to_string().contains("layer limit"));
    }
}
