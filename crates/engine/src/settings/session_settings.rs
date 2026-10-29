//! Session side of Settings, keyboard shortcut presets, auto-save and crash recovery.

use std::path::Path;

use crate::autosave::{self, Sentinel};
use crate::prefs::{PREFS_FILE, Prefs};
use crate::shortcuts::{Keymaps, SHORTCUTS_FILE, ShortcutTable, UiCommand};
use crate::{EngineError, Result, Session};

impl Session {
    /// Load settings, shortcut presets and scripts' settings from the config store and apply
    /// them.
    pub fn load_settings(&mut self) {
        if let Some(c) = &self.config {
            if let Some(t) = c.read(PREFS_FILE) {
                self.prefs = Prefs::from_json(&t);
            }
            if let Some(t) = c.read(SHORTCUTS_FILE) {
                self.keymaps = Keymaps::from_json(&t);
            }
        }
        crate::commands::scripts::load_script_settings(self);
        self.prefs_changed();
        self.shortcuts_changed();
    }

    /// Settings changed: apply what the engine owns (cache budgets) and tell frontends.
    pub fn prefs_changed(&mut self) {
        self.prefs.normalize();
        self.apply_cache_budgets();
        self.configure_disk_cache();
        self.footage.set_conform_folder(self.conformed_audio_folder());
        // Switches Affect Nested Comps changes what precomp layers render: drop cached pixels.
        let nested = self.prefs.general.switches_affect_nested_comps;
        if self.applied_nested_switches.replace(nested).is_some_and(|was| was != nested) {
            self.layer_cache.clear();
            self.events.push(crate::Event::PurgeCaches);
        }
        // Fewer undo levels apply right away.
        let levels = self.prefs.general.undo_levels.max(1) as usize;
        let current = self.project.clone();
        self.history.trim(levels, &current);
        // Settings ▸ Roto Brush / Face Tracking: load (or drop) the chosen trained models.
        self.apply_models();
        self.prefs_revision += 1;
    }

    /// A Disk settings folder: the setting, or (for frontends with a settings store) `name` next
    /// to the platform disk cache folder. Headless sessions without the setting stay off disk.
    fn cache_folder(&self, setting: &str, name: &str) -> Option<std::path::PathBuf> {
        let s = setting.trim();
        if !s.is_empty() {
            return Some(std::path::PathBuf::from(s));
        }
        // (the browser has no file system: no default folder there)
        if self.config.is_none() || cfg!(target_arch = "wasm32") {
            return None;
        }
        let base = effectcraft_render::disk_cache::default_folder();
        Some(base.parent().map(|p| p.join(name)).unwrap_or_else(|| base.join(name)))
    }

    /// Settings ▸ Disk ▸ Database and Cache Folder (media cache: audio waveform summaries).
    pub fn media_cache_folder(&self) -> Option<std::path::PathBuf> {
        self.cache_folder(&self.prefs.disk.media_cache_folder, "Media Cache")
    }

    /// Settings ▸ Disk ▸ Conformed Audio Folder (decoded footage audio, see
    /// `MediaPool::set_conform_folder`).
    pub fn conformed_audio_folder(&self) -> Option<std::path::PathBuf> {
        self.cache_folder(&self.prefs.disk.conformed_media_folder, "Conformed Audio")
    }

    /// Apply the Memory & CPU cache budgets, limited by RAM Reserved for Other Applications and
    /// reduced while the system is low on memory (see [`crate::prefs::Prefs::cache_budgets`]).
    pub fn apply_cache_budgets(&mut self) {
        let b = self.prefs.cache_budgets(self.sys_memory);
        self.layer_cache.set_budget(b.layer);
        self.footage.set_cache_budget(b.media);
    }

    /// Take the latest reading of the system's memory and start the next one in the background
    /// (desktop frontends call this every few seconds; it never waits for the system), then
    /// re-apply the cache budgets when the reading changes them. Returns the effective budgets.
    /// The disk cache's free disk space is read the same way (it keeps a reserve free).
    pub fn memory_tick(&mut self) -> crate::prefs::CacheBudgets {
        self.poll_disk_space();
        let before = self.prefs.cache_budgets(self.sys_memory);
        self.sys_memory = self.memory_watch.latest().or(self.sys_memory);
        self.memory_watch.poll();
        let after = self.prefs.cache_budgets(self.sys_memory);
        if after != before {
            self.apply_cache_budgets();
            // Frontends re-apply their preview cache budget.
            self.prefs_revision += 1;
            if after.reduced && !before.reduced {
                self.toast("The system is low on memory: cache sizes were reduced");
            }
        }
        after
    }

    /// Read the disk cache's free disk space in the background and apply it there (the reserve
    /// it keeps free): the free space and the cache's size are taken at the same moment, and the
    /// reader holds its own cache, so a reading never reaches another folder's.
    fn poll_disk_space(&mut self) {
        if let Some(dc) = &self.disk_cache {
            let dc = dc.clone();
            self.disk_watch.poll_with("ec-disk-space", move || {
                dc.set_disk_space(crate::sysinfo::disk_space(dc.folder()));
                Some(())
            });
        }
    }

    /// Apply Settings ▸ Media & Disk Cache ▸ Disk Cache: open (or close) the persistent cache
    /// and attach it to the layer cache. Without a folder setting the platform cache folder is
    /// used, but only by frontends with a settings store (headless sessions stay off disk).
    pub fn configure_disk_cache(&mut self) {
        use effectcraft_render::disk_cache::{DiskCache, default_folder};
        let d = &self.prefs.disk;
        let folder = if d.disk_cache_folder.trim().is_empty() {
            (self.config.is_some() && !cfg!(target_arch = "wasm32")).then(default_folder)
        } else {
            Some(std::path::PathBuf::from(d.disk_cache_folder.trim()))
        };
        let max = d.disk_cache_max_gb.max(1) as u64 * (1 << 30);
        // (no file system in the browser: the disk cache stays off there)
        let Some(folder) = folder.filter(|_| d.disk_cache_enabled && !cfg!(target_arch = "wasm32")) else {
            self.disk_cache = None;
            self.disk_watch = Default::default();
            self.layer_cache.set_disk(None);
            return;
        };
        // Tests: the Roto/Object Matte segment store below is one per process, so a test that
        // opens a disk cache (often the platform folder, through a settings store) would take
        // over the store of a roto test running beside it. Serialise them with the roto tests.
        #[cfg(test)]
        crate::tests::roto::hold_roto_cache_test_lock();
        let dc = match &self.disk_cache {
            Some(c) if c.folder() == folder.as_path() => {
                c.set_max_bytes(max);
                c.clone()
            }
            // A new folder: the free-space reading was for the old one (a reader still running
            // holds the old watcher's state, so it can't write into the new one).
            _ => match DiskCache::open(&folder, max) {
                Ok(c) => {
                    self.disk_watch = Default::default();
                    c
                }
                Err(e) => {
                    log::warn!("disk cache at {}: {e}", folder.display());
                    self.disk_cache = None;
                    self.disk_watch = Default::default();
                    self.layer_cache.set_disk(None);
                    return;
                }
            },
        };
        self.disk_cache = Some(dc.clone());
        // The reserve applies from the start (a new folder's watcher is idle).
        self.poll_disk_space();
        let salt = self.refresh_disk_salt(true).unwrap_or_default();
        self.layer_cache.set_disk(Some((dc.clone(), salt)));
        // Roto Brush / Object Matte segmentations persist there too (held weakly: switching the
        // disk cache off drops it).
        let store: std::sync::Arc<dyn effectcraft_effects::roto::SegStore> = dc.clone();
        effectcraft_effects::roto::set_seg_store(Some(std::sync::Arc::downgrade(&store)));
        self.disk_cache = Some(dc);
    }

    /// Write settings to the config store.
    pub fn save_prefs(&self) {
        if let Some(c) = &self.config
            && let Err(e) = c.write(PREFS_FILE, &self.prefs.to_json())
        {
            log::warn!("saving settings: {e}");
        }
    }

    /// Shortcut presets changed: rebuild the table and save them.
    pub fn shortcuts_changed(&mut self) {
        self.shortcut_table = std::sync::OnceLock::new();
        if let Some(c) = &self.config
            && let Err(e) = c.write(SHORTCUTS_FILE, &self.keymaps.to_json())
        {
            log::warn!("saving keyboard shortcuts: {e}");
        }
    }

    /// The frontend's own bindable commands (tools, timeline navigation…).
    pub fn set_ui_commands(&mut self, cmds: Vec<UiCommand>) {
        self.ui_commands = cmds;
        self.shortcut_table = std::sync::OnceLock::new();
    }

    /// The active shortcut preset resolved against every bindable command.
    pub fn shortcuts(&self) -> &ShortcutTable {
        self.shortcut_table.get_or_init(|| ShortcutTable::build(&self.keymaps, &self.ui_commands))
    }

    /// Remember a project in File ▸ Open Recent and in the crash-recovery sentinel.
    pub(crate) fn note_project_path(&mut self, path: &str) {
        self.prefs.push_recent(path);
        self.save_prefs();
        let _ = self.autosave_wait();
        self.autosave =
            autosave::AutoSaveState { last: self.autosave.last, sentinel: self.autosave.sentinel, background: self.autosave.background, ..Default::default() };
        self.update_sentinel();
    }

    /// Refresh the crash-recovery sentinel (when one is active).
    pub fn update_sentinel(&self) {
        if let Some(c) = &self.config
            && self.autosave.sentinel
        {
            let s = Sentinel { pid: autosave::pid(), project: self.path.clone(), autosave: self.autosave.last_path.clone(), dirty: self.is_dirty() };
            autosave::update(c.as_ref(), &s);
        }
    }

    /// Fallback folder for auto-saves of untitled projects (next to the settings).
    pub fn default_autosave_root(&self) -> Option<std::path::PathBuf> {
        self.config.as_ref().and_then(|c| c.dir())
    }

    /// Where auto-saves live: the config store's [`crate::config::FileOps`], else the file system.
    pub fn file_ops(&self) -> &dyn crate::config::FileOps {
        match &self.config {
            Some(c) => c.files(),
            None => &crate::config::StdFiles,
        }
    }

    /// Auto-save preferences with the host's isolation folder applied, without changing or
    /// persisting the user's settings.
    pub fn autosave_prefs(&self) -> crate::prefs::Prefs {
        let mut prefs = self.prefs.clone();
        if let Some(folder) = &self.autosave_folder_override {
            prefs.auto_save.location = "custom".into();
            prefs.auto_save.folder = folder.to_string_lossy().into_owned();
        }
        prefs
    }

    /// Write an auto-save now (whether or not the project is dirty). Returns its path.
    ///
    /// With [`autosave::AutoSaveState::background`] the project is serialised and written on a
    /// background thread (the path is returned at once; [`Session::autosave_wait`] waits).
    /// Auto-saves are compact JSON (opened like any project); File ▸ Save writes pretty JSON.
    pub fn autosave_now(&mut self) -> Result<String> {
        // One write at a time: the previous one must land before its slot rotates.
        self.autosave_wait()?;
        let root = self.default_autosave_root();
        let prefs = self.autosave_prefs();
        let fail = |e: std::io::Error| EngineError::Other(format!("auto-save failed: {e}"));
        let plan = autosave::plan_in(self.file_ops(), &prefs, self.path.as_deref(), root.as_deref(), self.autosave.last_slot).map_err(fail)?;
        let path = plan.path.to_string_lossy().into_owned();
        if self.autosave.background && !cfg!(target_arch = "wasm32") {
            let project = self.project.clone();
            let config = self.config.clone();
            let job = plan.clone();
            let pending =
                autosave::PendingWrite::spawn(plan.path.clone(), plan.slot, self.revision, self.path.clone(), self.script_ui.project_epoch, move || {
                    let json = project.to_json_compact();
                    match &config {
                        Some(c) => job.write(c.files(), json.as_bytes()),
                        None => job.write(&crate::config::StdFiles, json.as_bytes()),
                    }
                })
                .map_err(fail)?;
            self.autosave.pending = Some(pending);
        } else {
            let json = self.project.to_json_compact();
            plan.write(self.file_ops(), json.as_bytes()).map_err(fail)?;
            self.commit_autosave(path.clone(), plan.slot, self.revision);
        }
        Ok(path)
    }

    /// Advance recovery bookkeeping only for a successfully written snapshot.
    fn commit_autosave(&mut self, path: String, slot: u32, revision: u64) {
        self.autosave.last_slot = Some(slot);
        self.autosave.saved_revision = Some(revision);
        self.autosave.last_path = Some(path);
        self.update_sentinel();
    }

    /// Wait for a background auto-save ([`autosave::AutoSaveState::background`]) to land.
    pub fn autosave_wait(&mut self) -> Result<()> {
        match self.autosave.pending.take() {
            Some(w) => {
                w.join().map_err(|e| EngineError::Other(format!("auto-save failed: {e}")))?;
                // Open/New can replace the project before note_project_path collects this
                // write. Save As changes only its path. Never publish the old snapshot as the
                // new project's recovery record, even when that record's final reset fails.
                if w.project_path == self.path && w.project_epoch == self.script_ui.project_epoch {
                    self.commit_autosave(w.path.to_string_lossy().into_owned(), w.slot, w.revision);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Call periodically with wall-clock seconds: auto-saves a dirty project once the interval
    /// has passed since the last auto-save (or since the first call). Returns the written path.
    pub fn autosave_tick(&mut self, now: f64) -> Option<Result<String>> {
        if self.autosave.pending.as_ref().is_some_and(autosave::PendingWrite::is_finished)
            && let Err(e) = self.autosave_wait()
        {
            return Some(Err(e));
        }
        let a = &self.prefs.auto_save;
        if !a.enabled {
            self.autosave.last = Some(now);
            return None;
        }
        let interval = a.interval_minutes.max(1) as f64 * 60.0;
        let last = *self.autosave.last.get_or_insert(now);
        if now - last < interval {
            return None;
        }
        self.autosave.last = Some(now);
        if !self.is_dirty() || self.autosave.pending.is_some() || self.autosave.saved_revision == Some(self.revision) || self.render_job.is_some() {
            return None;
        }
        if self.path.is_none() && self.prefs.auto_save.location != "custom" && self.default_autosave_root().is_none() && self.autosave_folder_override.is_none()
        {
            return None;
        }
        let r = self.autosave_now();
        if let Ok(p) = &r {
            self.toast(format!("Auto-saved {}", Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
        }
        Some(r)
    }

    /// At launch: start crash recovery. Returns the previous run's state when it didn't exit
    /// cleanly (and the setting asks for it).
    pub fn begin_recovery(&mut self) -> Option<autosave::Recovery> {
        let c = self.config.clone()?;
        let r = autosave::begin(c.as_ref(), &self.prefs);
        self.autosave.sentinel = true;
        self.update_sentinel();
        r.filter(|_| self.prefs.startup.offer_crash_recovery)
    }

    /// Clean exit: remove the crash-recovery sentinel.
    pub fn end_recovery(&mut self) {
        if let Some(c) = &self.config
            && self.autosave.sentinel
        {
            autosave::end(c.as_ref());
            self.autosave.sentinel = false;
        }
    }
}
