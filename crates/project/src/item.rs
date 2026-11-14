use super::comp::Comp;
use super::footage::{Footage, FootageKind, Proxy, Solid};
use super::layer::LayerSource;
use super::settings::{APP_VERSION, ProjectError, ProjectSettings, SCHEMA_VERSION, SavedView};
use super::{Arc, BTreeMap, Deserialize, FrameRate, Label, Project, Serialize, Tick, file_check, limits};

/// `base` numbered with the lowest counter from 2 for which `taken` is false: a copy of an item or
/// another layer of the same name. A number set off by a space is a counter and is replaced
/// (`Red Solid 1` → `Red Solid 2`, `shot 2` → `shot 3`); digits that end a word belong to the
/// name and stay (`v-b1.mp4` → `v-b1.mp4 2`, `clip2.mov` → `clip2.mov 2`).
/// `base` without a trailing ` N`: digits are dropped only when a space separates them from a
/// nonempty stem, so `v-b1.mp4` and `Layer1` keep theirs.
pub(crate) fn name_stem(base: &str) -> &str {
    let head = base.trim_end_matches(|c: char| c.is_ascii_digit());
    match head.strip_suffix(' ') {
        Some(stem) if head.len() < base.len() && !stem.trim_end().is_empty() => stem.trim_end(),
        _ => base.trim_end(),
    }
}

pub(crate) fn numbered_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let stem = name_stem(base);
    (2u64..).map(|k| format!("{stem} {k}")).find(|n| !taken(n)).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u64);

/// An explicit "None" in a layer parameter whose unset value means the owning layer (Compound
/// Blur's Blur Layer, Displacement Map's map, Set Matte's matte): `Value::Layer(Some(NO_LAYER))`.
/// Ids start at 1, so it never names a layer; it resolves to no layer and shows as "None".
pub const NO_LAYER: u64 = 0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum ItemKind {
    Folder,
    Comp(Arc<Comp>),
    Footage(Footage),
    Solid(Solid),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub label: Label,
    #[serde(default)]
    pub comment: String,
    /// Containing folder (None = project root).
    #[serde(default)]
    pub parent: Option<ItemId>,
    pub kind: ItemKind,
    /// The item's proxy (footage and compositions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Box<Proxy>>,
}

impl Item {
    /// Whether an external original or proxy is known to be offline, even if the other can
    /// still provide a preview. Generated items and embedded data have no external dependency.
    pub fn has_missing_footage(&self) -> bool {
        let missing = |f: &crate::Footage| f.missing && !f.path.is_empty() && f.kind != FootageKind::Data;
        matches!(&self.kind, ItemKind::Footage(f) if missing(f)) || self.proxy.as_ref().is_some_and(|p| missing(&p.footage))
    }

    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            ItemKind::Folder => "Folder",
            ItemKind::Comp(_) => "Composition",
            ItemKind::Footage(f) => match f.kind {
                FootageKind::Video => "Video",
                FootageKind::Still => "Image",
                FootageKind::Sequence => "Image Sequence",
                FootageKind::Audio => "Audio",
                FootageKind::Model => "3D Model",
                FootageKind::Data => "Data",
            },
            ItemKind::Solid(_) => "Solid",
        }
    }
    pub fn as_comp(&self) -> Option<&Comp> {
        if let ItemKind::Comp(c) = &self.kind { Some(c) } else { None }
    }
    pub fn is_folder(&self) -> bool {
        matches!(self.kind, ItemKind::Folder)
    }
    /// (width, height) for visual items.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.kind {
            ItemKind::Comp(c) => Some((c.width, c.height)),
            ItemKind::Footage(f) if f.has_video => Some((f.width, f.height)),
            ItemKind::Solid(s) => Some((s.width, s.height)),
            _ => None,
        }
    }
    pub fn duration(&self) -> Option<Tick> {
        match &self.kind {
            ItemKind::Comp(c) => Some(c.duration),
            ItemKind::Footage(f) if f.kind != FootageKind::Still => Some(f.looped_duration()),
            _ => None,
        }
    }
    pub fn frame_rate(&self) -> Option<FrameRate> {
        match &self.kind {
            ItemKind::Comp(c) => Some(c.frame_rate),
            ItemKind::Footage(f) if f.kind == FootageKind::Video || f.kind == FootageKind::Sequence => Some(f.frame_rate),
            _ => None,
        }
    }
}

impl Default for Project {
    fn default() -> Self {
        Project {
            schema: SCHEMA_VERSION,
            settings: ProjectSettings::default(),
            items: BTreeMap::new(),
            next_id: 1,
            render_queue: Vec::new(),
            render_templates: Default::default(),
            render_prefs: Default::default(),
        }
    }
}

impl Project {
    /// Some layer has an expression, which can read markers (`marker.key(1)`, `numKeys`,
    /// `nearestKey`...). Only then do the markers of comps and layers affect what is rendered, so
    /// frame caches ignore them otherwise.
    pub fn reads_markers(&self) -> bool {
        // Called for every frame identity and disk-cache key: stops at the first expression and
        // builds no paths (`PropGroup::walk` formats one per node).
        fn any_expr(g: &crate::PropGroup) -> bool {
            g.children.iter().any(|c| match c {
                crate::Node::Prop(p) => p.expr.is_some(),
                crate::Node::Group(sub) => any_expr(sub),
            })
        }
        self.comps().any(|(_, c)| c.layers.iter().any(|l| any_expr(&l.props)))
    }

    /// The layers whose audio something draws ([`Heard`]): those a layer picks in an effect (an
    /// Audio Spectrum or Audio Waveform's Audio Layer, even on itself), and the layers inside a
    /// picked precomp, whose audio it mixes. With an expression anywhere (it can read any layer's
    /// Audio Levels) every layer may be heard ([`Heard::contains`]).
    pub fn heard_layers(&self) -> Heard {
        let mut layers = std::collections::BTreeSet::new();
        let mut stack: Vec<(ItemId, LayerId)> =
            self.comps().flat_map(|(id, c)| c.layers.iter().flat_map(move |l| l.picked_layers().into_iter().map(move |r| (*id, r)))).collect();
        // (bounded: each (comp, layer) is visited once)
        while let Some((cid, lid)) = stack.pop() {
            if !layers.insert((cid, lid)) {
                continue;
            }
            let inner = self.comp(cid).and_then(|c| c.layer(lid)).and_then(|l| match l.source {
                LayerSource::Comp { item } => Some(item),
                _ => None,
            });
            if let Some(inner) = inner
                && let Some(c) = self.comp(inner)
            {
                stack.extend(c.layers.iter().map(|l| (inner, l.id)));
            }
        }
        Heard { expressions: self.reads_markers(), audio_exprs: self.reads_audio(), layers }
    }

    /// Some expression may read a layer's audio: its text names it (`audio`, `"Audio Levels"`,
    /// `"ADBE Audio Group"`) or walks properties by index (`layer(1)(3)`).
    // ponytail: text heuristic; misses an index walk through a variable (`g = layer(1); g(3)`).
    // Upgrade: record audio reads in the expression host.
    fn reads_audio(&self) -> bool {
        fn reads(text: &str) -> bool {
            let t: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
            t.contains("audio") || t.match_indices(")(").any(|(i, _)| t.get(i + 2..).and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_digit()))
        }
        fn any(g: &crate::PropGroup) -> bool {
            g.children.iter().any(|c| match c {
                crate::Node::Prop(p) => p.expr.as_ref().is_some_and(|e| reads(&e.text)),
                crate::Node::Group(sub) => any(sub),
            })
        }
        self.comps().any(|(_, c)| c.layers.iter().any(|l| any(&l.props)))
    }

    /// Layers of `comp` (item `id`) that can't change its pixels: footage layers of files without
    /// video (music, voice-over) and layers with the Video switch off, unless another layer reads
    /// them (a hidden track matte) or something draws their audio ([`Self::heard_layers`]: the
    /// Audio Spectrum's Audio Layer, also from a comp that nests this one). Cameras, lights,
    /// nulls, environment layers and soloed layers (Solo hides every other layer) are never
    /// silent. Expressions, which can read any layer, are the caller's concern.
    pub fn silent_layers(&self, id: ItemId, comp: &Comp, heard: &Heard) -> std::collections::BTreeSet<LayerId> {
        let read: std::collections::BTreeSet<LayerId> =
            comp.layers.iter().flat_map(|l| l.referenced_layers().into_iter().filter(move |r| *r != l.id)).collect();
        comp.layers
            .iter()
            .filter(|l| {
                let no_picture = match &l.source {
                    LayerSource::Footage { item } => matches!(self.item(*item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if !f.has_video),
                    _ => false,
                };
                // (environment layers light Advanced 3D scenes without being drawn)
                (no_picture || l.source.is_av() && !l.switches.video)
                    && !l.switches.solo
                    && !l.environment
                    && !l.environment_background
                    && !read.contains(&l.id)
                    && !heard.picked(id, l.id)
            })
            .map(|l| l.id)
            .collect()
    }

    /// Allocate an id (items, layers and property uids share one counter).
    pub fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(&id)
    }
    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.get_mut(&id)
    }
    pub fn comp(&self, id: ItemId) -> Option<&Comp> {
        self.items.get(&id)?.as_comp()
    }
    /// A shared handle on a comp: a snapshot that costs a reference count, not a deep copy of
    /// its layers (frontends hold one across a frame instead of cloning the comp).
    pub fn comp_arc(&self, id: ItemId) -> Option<Arc<Comp>> {
        match &self.items.get(&id)?.kind {
            ItemKind::Comp(c) => Some(c.clone()),
            _ => None,
        }
    }
    /// Mutable comp (copy-on-write).
    pub fn comp_mut(&mut self, id: ItemId) -> Option<&mut Comp> {
        match &mut self.items.get_mut(&id)?.kind {
            ItemKind::Comp(c) => Some(Arc::make_mut(c)),
            _ => None,
        }
    }
    /// Adds an item; its size, frame rate and the like are brought within [`limits`] first.
    pub fn add_item(&mut self, name: &str, label: Label, parent: Option<ItemId>, mut kind: ItemKind) -> ItemId {
        limits::repair_kind(name, &mut kind, &mut vec![]);
        let id = ItemId(self.alloc());
        self.items.insert(id, Item { id, name: name.into(), label, comment: String::new(), parent, kind, proxy: None });
        id
    }
    /// Children of a folder (None = root), folders first then by name.
    pub fn children(&self, folder: Option<ItemId>) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.values().filter(|i| i.parent == folder).collect();
        v.sort_by(|a, b| b.is_folder().cmp(&a.is_folder()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }
    pub fn comps(&self) -> impl Iterator<Item = (&ItemId, &Comp)> {
        self.items.iter().filter_map(|(id, i)| i.as_comp().map(|c| (id, c)))
    }
    pub fn find_by_name(&self, name: &str) -> Option<&Item> {
        self.items.values().find(|i| i.name == name)
    }
    /// A numbered name for a copy of `base` that no project item has (Edit ▸ Duplicate), as
    /// `numbered_name` gives it (`Comp 1` → `Comp 2`, `Name` → `Name 2`, then `Name 3`,
    /// `v-b1.mp4` → `v-b1.mp4 2`). Items for which `ignore` holds don't count.
    pub fn numbered_item_name(&self, base: &str, ignore: impl Fn(&Item) -> bool) -> String {
        numbered_name(base, |n| self.items.values().any(|i| i.name == n && !ignore(i)))
    }
    /// `base` when no project item has that name, else the next free numbered name as
    /// [`Project::numbered_item_name`] gives it. Items for which `ignore` holds don't count.
    pub fn unique_item_name(&self, base: &str, ignore: impl Fn(&Item) -> bool) -> String {
        if self.items.values().any(|i| i.name == base && !ignore(i)) { self.numbered_item_name(base, ignore) } else { base.to_string() }
    }
    pub fn folder_named(&self, name: &str) -> Option<ItemId> {
        self.items.values().find(|i| i.is_folder() && i.name == name).map(|i| i.id)
    }
    /// Find the comp that contains `layer`.
    pub fn comp_of_layer(&self, layer: LayerId) -> Option<ItemId> {
        self.comps().find(|(_, c)| c.layer(layer).is_some()).map(|(id, _)| *id)
    }
    /// The compositions whose layers use `item` as their source (footage, solid, 3D model or
    /// precomp), each with how many of its layers do, ordered by comp name then id. Only direct
    /// layers count, as in After Effects' Project panel ("used 3 times"): a solid inside a precomp
    /// is used by the precomp, not by the comps that nest it. The total is the sum of the counts.
    pub fn item_users(&self, item: ItemId) -> Vec<(ItemId, usize)> {
        let mut users: Vec<(ItemId, String, usize)> = self
            .items
            .iter()
            .filter_map(|(id, it)| {
                let comp = it.as_comp()?;
                let n = comp.layers.iter().filter(|l| l.source.item() == Some(item)).count();
                (n > 0).then(|| (*id, it.name.to_lowercase(), n))
            })
            .collect();
        users.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        users.into_iter().map(|(id, _, n)| (id, n)).collect()
    }
    /// Whether comp `inner` is (transitively) nested inside comp `outer` (to prevent cycles).
    pub fn comp_contains(&self, outer: ItemId, inner: ItemId) -> bool {
        // A walk with a stack and a visited set rather than recursion (#1237): a damaged project
        // may hold a precomp cycle that doesn't pass `inner` (recursion never ended), a long
        // chain must not use up the call stack, and comps shared by many paths are visited once
        // (recursion re-walked them for every path).
        if outer == inner {
            return true;
        }
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![outer];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(c) = self.comp(id) else { continue };
            for l in &c.layers {
                if let LayerSource::Comp { item } = l.source {
                    if item == inner {
                        return true;
                    }
                    if !seen.contains(&item) {
                        stack.push(item);
                    }
                }
            }
        }
        false
    }
    /// Make sure `next_id` exceeds every id in the project (after loading or merging).
    pub fn fix_next_id(&mut self) {
        let mut m = self.next_id;
        for (id, it) in &self.items {
            // Saturating: a damaged file's ids must not overflow (the import then refuses it).
            m = m.max(id.0.saturating_add(1));
            if let ItemKind::Comp(c) = &it.kind {
                for l in &c.layers {
                    m = m.max(l.id.0.saturating_add(1)).max(l.props.max_uid().saturating_add(1));
                }
            }
        }
        self.next_id = m;
    }
    /// Whether `other` holds the same content. An allocated id counts (an edit may hand it out
    /// beyond the project, so it must stay taken). Shared compositions compare by pointer first,
    /// so after a copy-on-write edit only the comps it touched are compared field by field.
    pub fn same_content(&self, other: &Project) -> bool {
        let item_eq = |a: &Item, b: &Item| {
            let kind = match (&a.kind, &b.kind) {
                (ItemKind::Comp(x), ItemKind::Comp(y)) => Arc::ptr_eq(x, y) || x == y,
                (x, y) => x == y,
            };
            kind && a.id == b.id && a.name == b.name && a.label == b.label && a.comment == b.comment && a.parent == b.parent && a.proxy == b.proxy
        };
        self.schema == other.schema
            && self.next_id == other.next_id
            && self.settings == other.settings
            && self.items.len() == other.items.len()
            && self.items.iter().zip(&other.items).all(|((ka, a), (kb, b))| ka == kb && item_eq(a, b))
            && self.render_queue == other.render_queue
            && self.render_templates == other.render_templates
            && self.render_prefs == other.render_prefs
    }
    /// The project file's text: the project with the version that wrote it (`savedBy`), checked
    /// to read back, so a save never writes a file that can't be opened. Values JSON can't hold
    /// (NaN or infinity, written as `null`, and nesting too deep to parse) are refused on every
    /// save; debug builds also read the text back.
    pub fn to_file_json(&self) -> Result<String, ProjectError> {
        self.to_file_json_with_view(&SavedView::default(), false)
    }
    /// [`Project::to_file_json`] that also records the open compositions ([`SavedView`]), on one
    /// line when `compact` (Settings ▸ Project ▸ Save Compact Project Files).
    pub fn to_file_json_with_view(&self, view: &SavedView, compact: bool) -> Result<String, ProjectError> {
        craft_ecproj_format::check_schema(self.schema)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        let file = craft_ecproj_format::File::new(APP_VERSION, view, self);
        file_check::check(&file)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        let text =
            if compact { file.to_json_compact() } else { file.to_json() }.map_err(|e| ProjectError::Invalid(format!("the project can't be saved: {e}")))?;
        #[cfg(debug_assertions)]
        Project::from_json(&text)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        Ok(text)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
    /// [`Project::to_json`] without indentation: smaller and faster (auto-saves).
    pub fn to_json_compact(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Result<Project, ProjectError> {
        Project::from_json_repaired(s).map(|(p, _)| p)
    }
    /// [`Project::from_json`], also returning one line per value that was out of [`limits`] and
    /// was corrected (a composition of a billion pixels, a zero frame rate…), so a corrupt or
    /// hostile file still opens, within bounds, and the user can be told what changed.
    pub fn from_json_repaired(s: &str) -> Result<(Project, Vec<String>), ProjectError> {
        let mut p: Project = serde_json::from_str(s).map_err(|e| ProjectError::Invalid(e.to_string()))?;
        craft_ecproj_format::check_schema(p.schema).map_err(|e| ProjectError::Invalid(e.to_string()))?;
        p.fix_next_id();
        let mut notes = vec![];
        for item in p.items.values_mut() {
            limits::repair_item(item, &mut notes);
        }
        // An item inside a composition, a solid or footage (or inside an item that is not there) is
        // in no folder: it would not be listed under any parent. It moves to the top level.
        let folders: std::collections::BTreeSet<ItemId> = p.items.values().filter(|i| i.is_folder()).map(|i| i.id).collect();
        for item in p.items.values_mut() {
            if let Some(parent) = item.parent
                && !folders.contains(&parent)
            {
                notes.push(format!("{}: its parent is not a folder, moved to the top level", item.name));
                item.parent = None;
            }
        }
        Ok((p, notes))
    }
}

/// The layers whose audio something draws ([`Project::heard_layers`]): their Audio switch, Audio
/// group and audio effects change pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// Some layer has an expression (it can read anything).
    pub expressions: bool,
    /// Some expression may read a layer's audio ([`Project::reads_audio`]).
    audio_exprs: bool,
    layers: std::collections::BTreeSet<(ItemId, LayerId)>,
}

impl Heard {
    /// Something may draw the audio of layer `layer` of comp `comp`: a layer picks it
    /// ([`Self::picked`]) or an expression may read it.
    pub fn contains(&self, comp: ItemId, layer: LayerId) -> bool {
        self.audio_exprs || self.picked(comp, layer)
    }

    /// A layer picks layer `layer` of comp `comp` in an effect (or a precomp of it), so draws its
    /// audio. Expressions aside: they draw in their own layers.
    pub fn picked(&self, comp: ItemId, layer: LayerId) -> bool {
        self.layers.contains(&(comp, layer))
    }

    /// Heard in `self` or in `o` (the states before and after an edit).
    pub fn union(mut self, o: &Heard) -> Heard {
        self.expressions |= o.expressions;
        self.audio_exprs |= o.audio_exprs;
        self.layers.extend(o.layers.iter().copied());
        self
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;

    #[test]
    fn inventory_status_tracks_original_and_proxy_independently() {
        let footage = crate::Footage { path: "/original.png".into(), ..Default::default() };
        let mut item = Item {
            id: ItemId(1),
            name: "Plate".into(),
            label: Label::None,
            comment: String::new(),
            parent: None,
            kind: ItemKind::Footage(footage.clone()),
            proxy: None,
        };
        assert!(!item.has_missing_footage());
        item.proxy = Some(Box::new(Proxy { footage: crate::Footage { path: "/proxy.png".into(), missing: true, ..footage.clone() }, enabled: false }));
        assert!(item.has_missing_footage(), "unused missing proxies still need repair");
        item.proxy.as_mut().unwrap().footage.missing = false;
        assert!(!item.has_missing_footage());
        if let ItemKind::Footage(f) = &mut item.kind {
            f.missing = true;
        }
        item.proxy.as_mut().unwrap().enabled = true;
        assert!(item.has_missing_footage(), "a working proxy does not make the original available");
    }
}
