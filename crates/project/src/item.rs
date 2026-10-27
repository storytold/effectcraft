use super::comp::Comp;
use super::footage::{Footage, FootageKind, Proxy, Solid};
use super::layer::LayerSource;
use super::settings::{APP_VERSION, ProjectError, ProjectSettings, SCHEMA_VERSION, SavedView};
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u64);

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
    /// A numbered name for a copy of `base` that no project item has (Edit ▸ Duplicate): its
    /// trailing number is dropped and the lowest free one from 2 is added (`Comp 1` → `Comp 2`,
    /// `Name` → `Name 2`, then `Name 3`). Items for which `ignore` holds don't count.
    pub fn numbered_item_name(&self, base: &str, ignore: impl Fn(&Item) -> bool) -> String {
        let stem = base.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
        let taken = |n: &str| self.items.values().any(|i| i.name == n && !ignore(i));
        (2u64..).map(|k| format!("{stem} {k}")).find(|n| !taken(n)).unwrap_or_default()
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
        self.to_file_json_with_view(&SavedView::default())
    }
    /// [`Project::to_file_json`] that also records the open compositions ([`SavedView`]).
    pub fn to_file_json_with_view(&self, view: &SavedView) -> Result<String, ProjectError> {
        if self.schema > SCHEMA_VERSION {
            return Err(ProjectError::Invalid(format!(
                "the project can't be saved: it holds a value a project file can't store (project schema {} is newer than this version supports ({SCHEMA_VERSION}))",
                self.schema
            )));
        }
        #[derive(Serialize)]
        struct File<'a> {
            #[serde(rename = "savedBy")]
            saved_by: &'a str,
            #[serde(flatten)]
            view: &'a SavedView,
            #[serde(flatten)]
            project: &'a Project,
        }
        let file = File { saved_by: APP_VERSION, view, project: self };
        file_check::check(&file)
            .map_err(|e| ProjectError::Invalid(format!("the project can't be saved: it holds a value a project file can't store ({e})")))?;
        let text = serde_json::to_string_pretty(&file).map_err(|e| ProjectError::Invalid(format!("the project can't be saved: {e}")))?;
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
        if p.schema > SCHEMA_VERSION {
            return Err(ProjectError::Invalid(format!("project schema {} is newer than this version supports ({SCHEMA_VERSION})", p.schema)));
        }
        p.fix_next_id();
        let mut notes = vec![];
        for item in p.items.values_mut() {
            limits::repair_item(item, &mut notes);
        }
        Ok((p, notes))
    }
}
