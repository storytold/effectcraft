use super::*;
use crate::Session;
use serde_json::json;

fn round<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> T {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

#[test]
fn diff_and_patch_round_trip() {
    let a = json!({"a": 1, "b": [1, 2, {"c": null}], "d": {"e": "x", "f": [1]}, "g": null});
    let b = json!({"a": 2, "b": [1, 3, {"c": 4}], "d": {"f": [1, 2]}, "g": null, "h": {"i": null}});
    let ops = diff(&a, &b);
    assert!(!ops.is_empty());
    let mut x = a.clone();
    patch(&mut x, &round(&ops)).unwrap();
    assert_eq!(x, b);
    // Removal and the reverse direction.
    let mut y = b.clone();
    patch(&mut y, &diff(&b, &a)).unwrap();
    assert_eq!(y, a);
    assert!(diff(&a, &a).is_empty());
    // A whole-document change.
    let mut z = json!(1);
    patch(&mut z, &diff(&json!(1), &json!([1]))).unwrap();
    assert_eq!(z, json!([1]));
    // A patch for another document fails cleanly.
    assert!(patch(&mut json!(3), &diff(&a, &b)).is_err());
}

fn server(s: &Session) -> FrameServer {
    FrameServer::new(s.footage.clone(), s.expr.clone())
}

/// Pixels the page itself would render.
fn local(s: &Session, comp: ItemId, t: Tick, opts: RenderOpts) -> Vec<u8> {
    let mut r = Renderer::new(&s.project, s.footage.as_ref(), opts);
    r.expr = s.expr.as_deref();
    rgba8_premultiplied(&r.comp_frame(comp, t))
}

/// Layer buffers through the disk store: a worker's slow buffers queue up as entries; the
/// page plans what another worker fetches before its frame (recent misses on disk); that
/// worker's mid-render lookups find them and the pixels match.
#[test]
fn layer_buffers_go_through_the_disk_store() {
    use effectcraft_render::disk_cache::{DiskIndex, Kind, LayerPrefetch};
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let comp = s.active_comp_id().unwrap();
    let opts = RenderOpts { scale: 0.25, ..Default::default() };
    let t = Tick::from_seconds_f64(1.0);
    let req = |id: u64, layers: bool, prefetch: Vec<String>| {
        round(&FrameMsg::Render { id, revision: s.revision, comp, time: t, opts: Box::new(opts), disk: None, layers, prefetch })
    };
    let worker = || {
        let mut w = server(&s);
        let store = Arc::new(PrefetchStore::new(64 << 20).with_min_ms(0));
        w.set_layer_store(Some(store.clone()));
        let mut m = Mirror::default();
        assert!(w.handle(round(&m.sync(s.revision, &s.project).unwrap())).is_empty());
        (w, store)
    };
    let unhex = |v: &[String]| v.iter().map(|k| u128::from_str_radix(k, 16).unwrap()).collect::<Vec<u128>>();
    // Worker A: everything misses; its buffers queue up for the disk.
    let (mut a, store_a) = worker();
    let out = a.handle(req(1, true, vec![]));
    let (FrameReply::Frame { layer_misses, layer_hits, .. }, Some(px_a)) = &out[0] else { panic!("{:?}", out[0].0) };
    assert!(!layer_misses.is_empty() && layer_hits.is_empty());
    let writes = store_a.take_writes();
    assert!(!writes.is_empty());
    assert!(writes.iter().all(|(k, _)| (*k >> 64) as u64 == a.salt()), "keys are salted per project");
    let mut index = DiskIndex::default();
    let mut disk = std::collections::HashMap::new();
    for (k, e) in writes {
        index.insert(Kind::Layer, k, e.len() as u64);
        disk.insert(k, e);
    }
    index.insert(Kind::Frame, 1, 10);
    // The page: recent misses that are on disk, most recent first.
    let mut plan = LayerPrefetch::default();
    plan.note_misses(unhex(layer_misses));
    let keys = plan.plan(&index, 64);
    assert!(!keys.is_empty() && keys.len() <= index.len());
    // Worker B fetches them before its first frame: its lookups hit.
    let (mut b, store_b) = worker();
    for k in &keys {
        assert!(store_b.provide(*k, &disk[k]));
    }
    assert!(!store_b.provide(1, b"damaged"));
    let out = b.handle(req(1, true, keys.iter().map(|k| format!("{k:032x}")).collect()));
    let (FrameReply::Frame { layer_misses: mb, layer_hits: hb, .. }, Some(px_b)) = &out[0] else { panic!("{:?}", out[0].0) };
    assert_eq!(unhex(hb).len(), keys.len(), "every fetched buffer was used");
    assert!(mb.len() < layer_misses.len());
    assert_eq!(px_a, px_b);
    assert_eq!(*px_b, local(&s, comp, t, opts));
    // Nothing new to write: what B rendered itself was rendered by A too.
    assert!(store_b.take_writes().iter().all(|(k, _)| disk.contains_key(k)));
    // Without `layers` the store is not consulted.
    let (mut c, store_c) = worker();
    c.handle(req(1, false, vec![]));
    assert!(store_c.take_misses().is_empty() && store_c.take_writes().is_empty());
    // Purge empties the store.
    b.handle(round(&FrameMsg::Purge));
    assert!(!store_b.has(keys[0]));
    // LRU across frames and layers in the shared index: a touched layer outlives the frame.
    index.touch(Kind::Layer, keys[0]);
    let order = index.lru_order();
    assert_eq!(order.last(), Some(&(Kind::Layer, keys[0])));
    assert!(order.iter().position(|e| *e == (Kind::Frame, 1)).unwrap() < order.len() - 1);
}

#[test]
fn replica_follows_edits_through_diffs_and_renders_the_same_frames() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let comp = s.active_comp_id().unwrap();
    let opts = RenderOpts { scale: 0.25, guides: true, ..Default::default() };
    let mut m = Mirror::default();
    let mut w = server(&s);
    let send = |m: FrameMsg, w: &mut FrameServer| w.handle(round(&m));
    // First sync: the whole project.
    let first = m.sync(s.revision, &s.project).unwrap();
    assert!(matches!(first, FrameMsg::Project { .. }));
    assert!(send(first, &mut w).is_empty());
    assert!(m.sync(s.revision, &s.project).is_none(), "nothing new");
    let t = Tick::from_seconds_f64(1.0);
    let render = |w: &mut FrameServer, rev: u64, id: u64| {
        w.handle(round(&FrameMsg::Render { id, revision: rev, comp, time: t, opts: Box::new(opts), disk: None, layers: false, prefetch: vec![] }))
    };
    let out = render(&mut w, s.revision, 1);
    let (FrameReply::Frame { id, width, height, .. }, Some(px)) = &out[0] else { panic!("{:?}", out[0].0) };
    assert_eq!((*id, *width as usize * *height as usize * 4), (1, px.len()));
    assert_eq!(*px, local(&s, comp, t, opts));
    // An edit travels as a small diff; the replica renders the edited frame.
    s.execute("layer.newSolid", json!({"color": "#ff8800", "name": "Orange"})).unwrap();
    let msg = m.sync(s.revision, &s.project).unwrap();
    let FrameMsg::Patch { ops, .. } = &msg else { panic!("expected a patch") };
    assert!(!ops.is_empty() && serde_json::to_string(ops).unwrap().len() < s.project.to_json().len() / 2);
    assert!(send(msg, &mut w).is_empty());
    assert_eq!(**w.project().unwrap(), *s.project);
    let out = render(&mut w, s.revision, 2);
    assert_eq!(out[0].1.as_deref(), Some(&local(&s, comp, t, opts)[..]));
    // A request for another revision is refused as stale.
    let out = render(&mut w, s.revision + 7, 3);
    assert!(matches!(out[0].0, FrameReply::Failed { id: 3, stale: true, .. }));
    // A patch from the wrong base asks for a resync; the mirror then sends the project.
    let bad = FrameMsg::Patch { from: 999, revision: 1000, ops: vec![] };
    assert!(matches!(send(bad, &mut w)[0].0, FrameReply::Resync { .. }));
    m.reset();
    assert!(matches!(m.sync(s.revision, &s.project), Some(FrameMsg::Project { .. })));
}

#[test]
fn frame_messages_round_trip() {
    for m in [
        FrameMsg::Purge,
        FrameMsg::Segs { segs: vec![SegData { key: 7, data: "AAAA".into() }] },
        FrameMsg::Render {
            id: 3,
            revision: 4,
            comp: ItemId(2),
            time: Tick::from_seconds_f64(0.5),
            opts: Box::new(RenderOpts { scale: 0.5, draft: true, roi: Some([1.0, 2.0, 3.0, 4.0]), ..Default::default() }),
            disk: Some(format!("{:032x}", 0xabcdu128)),
            layers: true,
            prefetch: vec![format!("{:032x}", 7u128)],
        },
    ] {
        let a = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::to_string(&round(&m)).unwrap(), a);
        assert!(a.starts_with("{\"type\":"), "{a}");
    }
    for r in [
        FrameReply::Frame { id: 1, width: 2, height: 3, ms: 4.5, passes: 2, gpu: true, layer_misses: vec!["0a".into()], layer_hits: vec![] },
        FrameReply::Stored { key: "00ff".into(), bytes: 9, layer: false },
        FrameReply::Stored { key: "00fe".into(), bytes: 8, layer: true },
        FrameReply::Info { gpu: Some("Adapter (BrowserWebGpu)".into()), error: None },
        FrameReply::Failed { id: 1, error: "x".into(), stale: true },
        FrameReply::Resync { error: "y".into() },
    ] {
        assert_eq!(round(&r), r);
    }
}

/// An accelerator with deferred readbacks: a GPU effect chain is "read back" one
/// [`Deferred::settle`] after it was first asked for (computed with the CPU effects, so the
/// finished frame equals the CPU render); until then it misses and returns its input.
#[derive(Default)]
struct Deferred {
    ready: std::sync::Mutex<std::collections::HashSet<u64>>,
    asked: std::sync::Mutex<std::collections::HashSet<u64>>,
    gate: Arc<std::sync::atomic::AtomicBool>,
    chains: std::sync::atomic::AtomicUsize,
    auto: effectcraft_render::AutoPick,
}

impl Deferred {
    fn settle(&self) {
        let asked: Vec<u64> = self.asked.lock().unwrap().drain().collect();
        self.ready.lock().unwrap().extend(asked);
    }
}

impl Accelerator for Deferred {
    fn name(&self) -> String {
        "deferred".into()
    }
    fn comp_frame(&self, _: &Renderer, _: ItemId, _: Tick) -> Option<Image> {
        None
    }
    fn supports_effect(&self, id: &str) -> bool {
        effectcraft_effects::GPU_EFFECTS.contains(&id)
    }
    fn effects(&self, chain: &[effectcraft_render::FxStep], buf: &effectcraft_effects::Buf, levels: Option<f32>) -> Option<effectcraft_effects::Buf> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in chain {
            s.spec.id.hash(&mut h);
            s.ctx.time.to_bits().hash(&mut h);
        }
        (buf.img.width, buf.img.height).hash(&mut h);
        for p in &buf.img.data {
            for c in p {
                c.to_bits().hash(&mut h);
            }
        }
        let key = h.finish();
        if !self.ready.lock().unwrap().contains(&key) {
            self.asked.lock().unwrap().insert(key);
            self.gate.store(true, std::sync::atomic::Ordering::Relaxed);
            return Some(buf.clone());
        }
        self.chains.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut b = buf.clone();
        for s in chain {
            b = effectcraft_effects::apply(s.spec, &s.ctx, b);
            if let Some(l) = levels {
                effectcraft_render::color::quantize(&mut b.img, l);
            }
        }
        Some(b)
    }
    fn auto_pick(&self) -> Option<&effectcraft_render::AutoPick> {
        Some(&self.auto)
    }
    fn frame_begin(&self) {
        self.ready.lock().unwrap().clear();
    }
    fn pass_begin(&self) {
        self.gate.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    fn pass_missed(&self) -> bool {
        self.gate.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn miss_gate(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        Some(self.gate.clone())
    }
}

#[test]
fn deferred_readbacks_render_in_passes() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let comp = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#3388ff", "name": "Blue", "width": 200, "height": 120})).unwrap();
    s.execute("effect.apply", json!({"effect": "ec.blur.gaussian"})).unwrap();
    s.execute("effect.apply", json!({"effect": "ec.stylize.glow"})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#ffcc00", "name": "Yellow", "width": 80, "height": 80})).unwrap();
    s.execute("effect.apply", json!({"effect": "ec.color.levels"})).unwrap();
    let opts = RenderOpts { scale: 0.25, backend: Backend::Gpu, ..Default::default() };
    let t = Tick::from_seconds_f64(1.0);
    let mut w = server(&s);
    let acc = Arc::new(Deferred::default());
    w.set_accel(Some(acc.clone()));
    let mut m = Mirror::default();
    w.handle(round(&m.sync(s.revision, &s.project).unwrap()));
    let req = |id| round(&FrameMsg::Render { id, revision: s.revision, comp, time: t, opts: Box::new(opts), disk: None, layers: false, prefetch: vec![] });
    assert!(w.handle(req(1)).is_empty() && w.waiting(), "the first pass misses");
    let mut out = vec![];
    for _ in 0..10 {
        acc.settle();
        out = w.resume();
        if !w.waiting() {
            break;
        }
    }
    let (FrameReply::Frame { id: 1, passes, gpu: true, .. }, Some(px)) = &out[0] else { panic!("{:?}", out.first().map(|o| &o.0)) };
    assert!(*passes >= 2, "{passes}");
    assert!(acc.chains.load(std::sync::atomic::Ordering::Relaxed) >= 2);
    // The same pixels as the CPU: no placeholder survived, in the frame or the layer cache.
    assert_eq!(*px, local(&s, comp, t, RenderOpts { backend: Backend::Cpu, ..opts }));
    let out = w.handle(req(2));
    let out = if out.is_empty() {
        acc.settle();
        w.resume()
    } else {
        out
    };
    assert_eq!(out[0].1.as_deref(), Some(&px[..]));
    // CPU requests never touch the accelerator; Auto times whole frames.
    let cpu = RenderOpts { backend: Backend::Cpu, ..opts };
    let out =
        w.handle(round(&FrameMsg::Render { id: 3, revision: s.revision, comp, time: t, opts: Box::new(cpu), disk: None, layers: false, prefetch: vec![] }));
    assert!(matches!(out[0].0, FrameReply::Frame { id: 3, passes: 1, gpu: false, .. }));
    for id in 4..12 {
        let auto = RenderOpts { backend: Backend::Auto, ..opts };
        let mut out =
            w.handle(round(&FrameMsg::Render { id, revision: s.revision, comp, time: t, opts: Box::new(auto), disk: None, layers: false, prefetch: vec![] }));
        while w.waiting() {
            acc.settle();
            out = w.resume();
        }
        assert_eq!(out[0].1.as_deref(), Some(&px[..]), "frame {id}");
    }
    let st = acc.auto.stats(effectcraft_render::AutoKey::new(comp, opts.scale, false)).unwrap();
    assert!(st.cpu_frames > 0 && st.gpu_frames > 0, "{st:?}");
}
