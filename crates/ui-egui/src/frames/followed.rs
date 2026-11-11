//! Frames made of a followed file (a LUT file an effect names, #3493) are checked against it.
//! Renders read such a file as it is now, and nothing in a frame's key says which version it was
//! made of: a pass on the frame pool stamps the followed files the kept frames read (each file
//! once) and drops the frames made of another version. The frame's own file list says which
//! files are followed, as rendered or as read from the disk cache. A superseded version is
//! remembered as stale, so a frame still rendering with it is not kept when it finishes
//! ([`Cache::insert_with`]).

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

use effectcraft_engine::render::reads::ReadFiles;
#[cfg(not(target_arch = "wasm32"))]
use effectcraft_engine::render::reads::{FileStamp, ReadFile};

use super::Cache;
#[cfg(not(target_arch = "wasm32"))]
use super::{FrameKey, Frames};

/// Superseded file versions remembered at most ([`Cache::stale`]). Past it some are forgotten:
/// a late frame made of one is kept until the next pass drops it.
#[cfg(not(target_arch = "wasm32"))]
const MAX_STALE: usize = 1024;

/// Whether `files` holds a followed file (checks are due while such a frame is kept).
pub(super) fn follows(files: Option<&ReadFiles>) -> bool {
    files.is_some_and(|f| f.files().iter().any(|f| f.followed))
}

impl Cache {
    /// Whether `files` holds a version of a followed file known to be superseded.
    pub(super) fn is_stale(&self, files: Option<&ReadFiles>) -> bool {
        !self.stale.is_empty() && files.is_some_and(|f| f.files().iter().any(|f| self.stale.contains(f)))
    }

    /// The followed files the kept frames read, each version once.
    #[cfg(not(target_arch = "wasm32"))]
    fn followed_reads(&self) -> HashSet<ReadFile> {
        self.map.values().filter_map(|c| c.files.as_deref()).flat_map(|f| f.files()).filter(|f| f.followed).cloned().collect()
    }

    /// Forget the stale versions that are current (`now`) again, drop the frames made of a
    /// stale or `superseded` version, and remember `superseded` as stale (at most
    /// [`MAX_STALE`]). Returns how many frames were dropped.
    #[cfg(not(target_arch = "wasm32"))]
    fn drop_stale(&mut self, now: &HashMap<Arc<str>, FileStamp>, superseded: HashSet<ReadFile>) -> usize {
        self.stale.retain(|f| now.get(&f.path) != Some(&f.stamp));
        let made_of_stale = |c: &super::Cached| c.files.as_deref().is_some_and(|f| f.files().iter().any(|f| superseded.contains(f) || self.stale.contains(f)));
        let gone: Vec<FrameKey> = self.map.iter().filter(|(_, c)| made_of_stale(c)).map(|(k, _)| *k).collect();
        for k in &gone {
            self.remove(k);
        }
        self.order.retain(|k| self.map.contains_key(k));
        if self.stale.len().saturating_add(superseded.len()) > MAX_STALE {
            self.stale.clear();
        }
        self.stale.extend(superseded.into_iter().take(MAX_STALE));
        gone.len()
    }
}

/// One pass: stamp the followed files the frames in `cache` read and drop the frames made of
/// another version. Returns how many were dropped and whether any frame kept reads a followed
/// file. The files are stamped without the cache's lock, each path once.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn check(cache: &Mutex<Cache>) -> (usize, bool) {
    // (the stale versions' files too: one written back as it was is current again)
    let (seen, stale) = cache.lock().map(|c| (c.followed_reads(), c.stale.iter().map(|f| f.path.clone()).collect::<Vec<_>>())).unwrap_or_default();
    let paths: HashSet<Arc<str>> = seen.iter().map(|f| f.path.clone()).chain(stale).collect();
    let now: HashMap<Arc<str>, FileStamp> = paths.into_iter().map(|p| (p.clone(), FileStamp::native(&p).unwrap_or(FileStamp::Missing))).collect();
    // (only versions a kept frame read before the stamps were taken: a frame made since may have
    // read a newer one)
    let superseded: HashSet<ReadFile> = seen.into_iter().filter(|f| now.get(&f.path) != Some(&f.stamp)).collect();
    let Ok(mut c) = cache.lock() else { return (0, false) };
    let dropped = c.drop_stale(&now, superseded);
    c.follows = c.map.values().any(|k| follows(k.files.as_deref()));
    (dropped, c.follows)
}

#[cfg(not(target_arch = "wasm32"))]
impl Frames {
    /// Check the kept frames against the followed files they read, on the frame pool (one pass
    /// at a time); the viewer repaints when frames were dropped.
    pub fn check_followed_files(&self) {
        if self.followed_check.swap(true, Ordering::AcqRel) {
            return;
        }
        let (cache, busy, ctx) = (self.cache.clone(), self.followed_check.clone(), self.ctx.clone());
        let work = move || {
            let (dropped, _) = check(&cache);
            busy.store(false, Ordering::Release);
            if dropped > 0
                && let Some(ctx) = &ctx
            {
                ctx.request_repaint();
            }
        };
        match &self.pool {
            Some(pool) => pool.spawn(work),
            None => rayon::spawn(work),
        }
    }

    /// Whether a kept frame may be made of a followed file (more checks are due): set when one
    /// is kept, cleared by a check that finds none.
    pub fn follows_files(&self) -> bool {
        self.cache.lock().is_ok_and(|c| c.follows)
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use effectcraft_engine::project::ItemId;
    use effectcraft_engine::render::RenderOpts;
    use effectcraft_engine::render::disk_cache::DiskCache;
    use effectcraft_engine::render::reads::{FileStamp, ReadFile, ReadFiles};
    use effectcraft_engine::time::Tick;
    use serde_json::json;

    use super::{MAX_STALE, check};
    use crate::frames::{FrameImage, FrameKey, Frames, RenderSource};

    const BLACK: &str = "LUT_1D_SIZE 2\n0 0 0\n0 0 0\n";
    const WHITE: &str = "LUT_1D_SIZE 2\n1 1 1\n1 1 1\n";

    /// Write `text` to `path`, modified `secs` after the epoch (an edit to the same size is told
    /// apart whatever the clock's resolution).
    fn write_lut(path: &Path, text: &str, secs: u64) {
        std::fs::write(path, text).unwrap();
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
    }

    /// A grey solid through Apply Color LUT naming `lut`, as its value or (`expression`) as the
    /// string an expression returns, in a session with expressions, rendered with `disk`: the
    /// frame cache, the render source, the comp and the key of its first frame.
    fn graded(lut: &Path, expression: bool, disk: Option<Arc<DiskCache>>) -> (Frames, RenderSource, ItemId, FrameKey) {
        let mut s = effectcraft_host::session();
        let cid = ItemId(
            s.execute_checked("comp.new", json!({"name": "C", "width": 16, "height": 16, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap(),
        );
        let l = s.execute_checked("layer.newSolid", json!({"color": "#808080", "width": 16, "height": 16})).unwrap()["layer"].clone();
        s.execute_checked("effect.apply", json!({"layer": l, "effect": "Apply Color LUT"})).unwrap();
        let path = lut.to_string_lossy();
        if expression {
            let text = serde_json::to_string(&path).unwrap();
            s.execute_checked("prop.setExpression", json!({"layer": l, "path": "effects/#1/lut", "expression": text})).unwrap();
        } else {
            s.execute_checked("prop.set", json!({"layer": l, "path": "effects/#1/lut", "value": path})).unwrap();
        }
        let src = RenderSource {
            project: s.project.clone(),
            footage: s.footage.clone(),
            expr: s.expr.clone(),
            layer_cache: Arc::default(),
            gpu: None,
            gpu_display: false,
            disk,
        };
        let f = Frames::default();
        let key = FrameKey { revision: s.revision, content: f.content_of(&s.project, s.revision, cid), comp: cid.0, frame: 0, scale: 1000, view: 0, opts: 0 };
        (f, src, cid, key)
    }

    /// The red of the frame's centre pixel, rendered by the frame worker unless it is kept.
    fn red(f: &Frames, src: &RenderSource, cid: ItemId, key: FrameKey) -> u8 {
        f.request_urgent(src, key, cid, Tick::ZERO, RenderOpts::default());
        for _ in 0..2000 {
            if f.inflight() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Some(FrameImage::Cpu(img)) = f.get(&key) else { panic!("the frame is kept") };
        img.pixels[img.width() * (img.height() / 2) + img.width() / 2].r()
    }

    /// #3493: a kept frame made of a LUT file (named directly or by an expression) is dropped by
    /// the first check after the file changed, however long before that check it was rendered;
    /// a frame of the old file finishing late is not kept; the frame renders again with the new
    /// file, and checks while it is unchanged drop nothing.
    #[test]
    fn a_frame_made_of_an_edited_lut_file_renders_again() {
        for expression in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let lut = dir.path().join("grade.cube");
            write_lut(&lut, BLACK, 1_000);
            let (f, src, cid, key) = graded(&lut, expression, None);
            assert_eq!(red(&f, &src, cid, key), 0, "expression: {expression}");
            assert!(f.follows_files(), "checks are due once such a frame is kept, before any check");
            let late = f.cache.lock().unwrap().map.get(&key).map(|c| (c.img.clone(), c.files.clone())).unwrap();
            write_lut(&lut, WHITE, 2_000);
            assert!(f.get(&key).is_some(), "nothing has looked at the file yet");
            assert_eq!(check(&f.cache), (1, false), "the first check after the edit (expression: {expression})");
            assert!(f.get(&key).is_none());
            f.worker().finish_key(key, true, late.0, late.1, None, web_time::Instant::now());
            assert!(f.get(&key).is_none(), "a frame of the old file finishing after the check");
            assert_eq!(red(&f, &src, cid, key), 255, "expression: {expression}");
            assert_eq!(check(&f.cache), (0, true));
            assert_eq!(check(&f.cache), (0, true), "an unchanged file drops nothing");
            assert!(f.get(&key).is_some());
        }
    }

    /// A file created after the frame was made without it, and one written back as it was.
    #[test]
    fn a_created_or_restored_lut_file_renders_again() {
        let dir = tempfile::tempdir().unwrap();
        let lut = dir.path().join("later.cube");
        let (f, src, cid, key) = graded(&lut, false, None);
        assert_eq!(red(&f, &src, cid, key), 128, "no file: the layer as it is");
        write_lut(&lut, BLACK, 1_000);
        assert_eq!(check(&f.cache), (1, false), "a file created");
        assert_eq!(red(&f, &src, cid, key), 0);
        write_lut(&lut, WHITE, 2_000);
        assert_eq!(check(&f.cache), (1, false));
        write_lut(&lut, BLACK, 1_000);
        assert_eq!(check(&f.cache), (0, false), "nothing kept to drop");
        assert!(!f.follows_files(), "no checks are due");
        assert_eq!(red(&f, &src, cid, key), 0, "the version written back is current again");
        assert_eq!(check(&f.cache), (0, true));
    }

    /// #3493, through the frame pool as the app runs it: a kept frame renders again once the LUT
    /// file it was made of changes.
    #[test]
    fn a_kept_frame_renders_again_once_its_lut_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let lut = dir.path().join("pool.cube");
        write_lut(&lut, BLACK, 1_000);
        let (f, src, cid, key) = graded(&lut, false, None);
        assert_eq!(red(&f, &src, cid, key), 0);
        write_lut(&lut, WHITE, 2_000);
        for _ in 0..2000 {
            f.check_followed_files();
            if f.get(&key).is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(red(&f, &src, cid, key), 255);
    }

    /// A frame read from the disk cache by a new frame cache (the next session: nothing in this
    /// one rendered it) is checked against the LUT file it was made of like a rendered one.
    #[test]
    fn a_frame_from_the_disk_cache_renders_again_once_its_lut_file_changes() {
        for expression in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let lut = dir.path().join("disk.cube");
            write_lut(&lut, BLACK, 1_000);
            let disk = DiskCache::open(dir.path().join("cache"), 1 << 30).unwrap();
            let (first, src, cid, key) = graded(&lut, expression, Some(disk.clone()));
            assert_eq!(red(&first, &src, cid, key), 0);
            for _ in 0..400 {
                disk.flush();
                if disk.stats().entries >= 1 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(disk.stats().entries, 1, "stored (expression: {expression})");
            let again = Frames::default();
            let key = FrameKey { content: again.content_of(&src.project, key.revision, cid), ..key };
            let hits = disk.stats().hits;
            assert_eq!(red(&again, &src, cid, key), 0);
            assert_eq!(disk.stats().hits, hits + 1, "read from the disk cache, not rendered (expression: {expression})");
            assert!(again.follows_files(), "expression: {expression}");
            write_lut(&lut, WHITE, 2_000);
            assert_eq!(check(&again.cache), (1, false), "expression: {expression}");
            assert_eq!(red(&again, &src, cid, key), 255, "expression: {expression}");
        }
    }

    /// More superseded versions than are remembered: every frame made of one is dropped, at most
    /// [`MAX_STALE`] are remembered, and a late frame of a forgotten one is dropped by the next
    /// check.
    #[test]
    fn versions_past_the_bound_are_still_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let n = MAX_STALE + 8;
        // (each frame made of its own LUT file, gone since)
        let read = |i: usize| {
            let path = dir.path().join(format!("{i}.cube"));
            ReadFile::followed(path.to_string_lossy().as_ref(), FileStamp::Native { len: 1, modified: Some(1) })
        };
        let files = |i: usize| Some(Arc::new([read(i)].into_iter().collect::<ReadFiles>()));
        let key = |i: usize| FrameKey { revision: 0, content: 0, comp: 0, frame: i as i64, scale: 1000, view: 0, opts: 0 };
        let img = || FrameImage::Cpu(Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK])));
        let f = Frames::default();
        for i in 0..n {
            f.cache.lock().unwrap().insert_with(key(i), img(), files(i), None);
        }
        assert_eq!(check(&f.cache), (n, false));
        let stale = f.cache.lock().unwrap().stale.clone();
        assert!(stale.len() <= MAX_STALE, "{} remembered", stale.len());
        let forgotten = (0..n).find(|i| !stale.contains(&read(*i))).unwrap();
        f.cache.lock().unwrap().insert_with(key(forgotten), img(), files(forgotten), None);
        assert!(f.get(&key(forgotten)).is_some(), "kept until the next check");
        assert_eq!(check(&f.cache), (1, false));
        assert!(f.get(&key(forgotten)).is_none());
    }
}
