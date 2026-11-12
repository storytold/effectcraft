use super::*;

fn round<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> String {
    let a = serde_json::to_string(v).unwrap();
    let back: T = serde_json::from_str(&a).unwrap();
    let b = serde_json::to_string(&back).unwrap();
    assert_eq!(a, b);
    a
}

/// A group with properties `a` (uid 2) and `b` (uid 3) and a sub-group (uid 4) holding `c`
/// (uid 5).
fn group() -> PropGroup {
    use effectcraft_keyframe::Value as KV;
    use effectcraft_project::{Node, Property};
    let mut sub = PropGroup::new(4, "sub", "Sub");
    sub.children.push(Node::Prop(Property::new(5, "c", "C", KV::Scalar(0.0))));
    let mut g = PropGroup::new(1, "fx", "Effect");
    g.children.push(Node::Prop(Property::new(2, "a", "A", KV::Scalar(0.0))));
    g.children.push(Node::Prop(Property::new(3, "b", "B", KV::Scalar(0.0))));
    g.children.push(Node::Group(sub));
    g
}

fn set(g: &mut PropGroup, uid: Uid, v: f64) {
    g.find_mut(uid).unwrap().value = effectcraft_keyframe::Value::Scalar(v);
}

fn get(g: &PropGroup, uid: Uid) -> f64 {
    match g.find(uid).unwrap().value {
        effectcraft_keyframe::Value::Scalar(v) => v,
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn analysis_results_merge_with_edits_made_meanwhile() {
    use effectcraft_project::Node;
    let base = group();
    // The user changes `a` and switches the effect off; the analysis writes `b` and `c`.
    let mut live = base.clone();
    set(&mut live, 2, 1.0);
    live.enabled = false;
    let mut result = base.clone();
    set(&mut result, 3, 2.0);
    set(&mut result, 5, 3.0);
    let m = merge_group(&base, &live, &result).unwrap();
    assert_eq!((get(&m, 2), get(&m, 3), get(&m, 5), m.enabled), (1.0, 2.0, 3.0, false));
    // Unchanged on either side: the other side wins whole.
    assert_eq!(merge_group(&base, &base, &result).unwrap(), result);
    assert_eq!(merge_group(&base, &live, &base).unwrap(), live);
    // Both changed `b`, differently: a conflict (nothing is applied). The same way: fine.
    let mut both = live.clone();
    set(&mut both, 3, 9.0);
    assert!(merge_group(&base, &both, &result).is_none());
    set(&mut both, 3, 2.0);
    assert_eq!(get(&merge_group(&base, &both, &result).unwrap(), 3), 2.0);
    // The analysis adds a child (a result group) while the user edits `a`: both kept.
    let mut added = result.clone();
    added.children.push(Node::Group(PropGroup::new(6, "out", "Output")));
    let m = merge_group(&base, &live, &added).unwrap();
    assert!(m.find_group(6).is_some() && get(&m, 2) == 1.0);
    // The analysis drops the sub-group the user didn't touch: dropped.
    let mut dropped = result.clone();
    dropped.children.retain(|n| !matches!(n, Node::Group(g) if g.uid == 4));
    let m = merge_group(&base, &live, &dropped).unwrap();
    assert!(m.find_group(4).is_none() && get(&m, 2) == 1.0);
    // …but if the user changed something in it, that's a conflict.
    let mut touched = live.clone();
    set(&mut touched, 5, 7.0);
    assert!(merge_group(&base, &touched, &dropped).is_none());
}

#[test]
fn protocol_round_trips() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", serde_json::json!({})).unwrap();
    let comp = s.active_comp_id().unwrap();
    s.execute("renderQueue.add", serde_json::json!({})).unwrap();
    let item = s.project.render_queue[0].clone();
    let reqs = [
        WorkerJob::Render { items: vec![vec![(item.clone(), "/out.gif".into())]] },
        WorkerJob::Warp { comp, layer: LayerId(3), effect: 9 },
        WorkerJob::WarpPlan { comp, layer: LayerId(3), effect: 9, time: Tick::from_seconds_f64(0.5) },
        WorkerJob::RotoFreeze { comp, layer: LayerId(3), effect: 9 },
        WorkerJob::Camera { comp, layer: LayerId(3), effect: 9 },
        WorkerJob::RotoPropagate { comp, layer: LayerId(3), effect: 9, direction: crate::roto::Direction::Forward },
        WorkerJob::Track {
            comp,
            layer: LayerId(1),
            tracker: 4,
            direction: crate::tracking::Direction::Backward,
            settings: TrackerSettings::default(),
            points: vec![PointValues {
                uid: 5,
                spec: effectcraft_track::PointSpec { center: [1.0, 2.0], feature_size: [3.0, 4.0], search_offset: [0.5, 0.0], search_size: [9.0, 9.0] },
                attach_offset: [0.25, -1.0],
            }],
            times: vec![Tick::ZERO, Tick::from_seconds_f64(0.5)],
        },
        WorkerJob::MaskTrack {
            comp,
            layer: LayerId(2),
            mask: 7,
            direction: crate::tracking::Direction::Forward,
            path: ShapePath::ellipse([10.0, 20.0], 5.0, 6.0),
            method: MaskMethod::Perspective,
            times: vec![Tick::from_seconds_f64(1.0)],
        },
        WorkerJob::ContentFill {
            plan: Box::new(crate::commands::content_fill::FillPlan {
                comp,
                layer: LayerId(2),
                name: "Clip".into(),
                width: 4,
                height: 3,
                times: vec![Tick::ZERO],
                method: "object".into(),
                lighting: 0.5,
                expand: 1,
                reference: Some((LayerId(1), Tick::ZERO)),
                dir: "/Fill/Clip_Fill_1".into(),
            }),
        },
    ];
    for job in reqs {
        let kind = job.kind();
        let req = WorkerRequest { id: 42, project: s.project.to_json(), files: footage_files(&s.project), job };
        let t = round(&req);
        assert!(t.contains("\"kind\":"), "{kind:?}");
        let back: WorkerRequest = serde_json::from_str(&t).unwrap();
        assert_eq!(back.job.kind(), kind);
        assert_eq!(Project::from_json(&back.project).unwrap(), *s.project);
    }
    let g = s.active_comp().unwrap().layers[0].props.clone();
    for r in [
        WorkerReply::Render { state: JobState { current: Some(3), done: 4, total: 10, items_total: 1, ..Default::default() } },
        WorkerReply::Item { update: ItemUpdate::Started { id: 3, unix: 1_700_000_000 } },
        WorkerReply::Item { update: ItemUpdate::Finished { id: 3, status: RenderStatus::Failed("x".into()), seconds: 1.5, output: Some("/a.gif".into()) } },
        WorkerReply::Progress { done: 1, total: 2 },
        WorkerReply::WarpPlan {
            key: [1, u64::MAX],
            summary: Box::new(effectcraft_effects::warp_stab::PlanSummary {
                warps: vec![[[1.0, 0.0, 2.5], [0.0, 1.0, -1.0], [0.0, 0.0, 1.0]]],
                auto_scale: 1.08,
                crop: Some([1.0, 2.0, 300.0, 200.0]),
                valid_fraction: 0.9,
            }),
        },
        WorkerReply::Group { comp, layer: LayerId(1), group: Box::new(g), message: "ok".into() },
        WorkerReply::Segs { segs: vec![SegData::new(5, &effectcraft_track::roto::FrameSeg::empty(4, 3))] },
        WorkerReply::Fill {
            plan: Box::new(crate::commands::content_fill::FillPlan {
                comp,
                layer: LayerId(2),
                name: "Clip".into(),
                width: 4,
                height: 3,
                times: vec![Tick::ZERO],
                method: "object".into(),
                lighting: 0.5,
                expand: 1,
                reference: Some((LayerId(1), Tick::ZERO)),
                dir: "/Fill/Clip_Fill_1".into(),
            }),
            files: vec!["/Fill/Clip_Fill_1/fill_00000.png".into()],
        },
        WorkerReply::Failed { error: "boom".into() },
        WorkerReply::Done,
    ] {
        let t = round(&r);
        assert!(t.starts_with("{\"type\":"), "{t}");
    }
}

/// An [`Offload`] that runs the job right away in a second session, through JSON (what a
/// Web Worker does, minus the threads).
struct Inline;
impl Offload for Inline {
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
        let req: WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        let mut w = Session { exporter: Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() })), ..Default::default() };
        let post: Post = Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
        run_request(&mut w, req, &post);
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

#[test]
fn render_runs_through_the_offload() {
    let mut s =
        Session { exporter: Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() })), offload: Some(Arc::new(Inline)), ..Default::default() };
    s.execute("file.openDemoProject", serde_json::json!({})).unwrap();
    s.execute("renderQueue.add", serde_json::json!({"output": "/x.gif"})).unwrap();
    let ids = s.start_render(false).unwrap();
    // (the inline offload has replied already; replies are applied when polled)
    s.poll_render();
    assert!(s.render_job.is_none());
    let it = s.project.render_queue.iter().find(|i| i.id == ids[0]).unwrap();
    assert_eq!(it.status, RenderStatus::Done, "{:?}", it.status);
    assert!(it.last_output.as_deref().is_some_and(|o| o.ends_with("x.gif")));
    // `wait` renders inline.
    s.execute("renderQueue.add", serde_json::json!({"output": "/y.gif"})).unwrap();
    s.start_render(true).unwrap();
    assert!(s.render_job.is_none() || s.render_job.as_ref().unwrap().remote.is_none());
}

/// Runs jobs in a session sharing `footage` (the synthetic clip of the tracking tests).
struct InlineWith(Arc<dyn effectcraft_render::FootageSource>);
impl Offload for InlineWith {
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
        let req: WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        let mut w = Session { footage: self.0.clone(), ..Default::default() };
        let post: Post = Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
        run_request(&mut w, req, &post);
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

/// Never replies (a job still running); records cancellations.
#[derive(Default)]
struct Hanging(Mutex<Vec<u64>>);
impl Offload for Hanging {
    fn start(&self, _: WorkerRequest, _: Arc<Inbox>) -> Result<(), String> {
        Ok(())
    }
    fn cancel(&self, id: u64) {
        self.0.lock().unwrap().push(id);
    }
}

fn pose(f: u32) -> ([f64; 2], f64, f64) {
    ([150.0 + 2.0 * f as f64, 110.0 + 1.0 * f as f64], 0.0, 1.0)
}

#[test]
fn track_analysis_runs_through_the_offload_as_one_undo_step() {
    let (mut s, clip, _) = crate::tests::track::setup(crate::tests::track::patch_frames(pose));
    s.offload = Some(Arc::new(InlineWith(s.footage.clone())));
    s.execute("track.motion", serde_json::json!({})).unwrap();
    s.execute("track.setPoint", serde_json::json!({"point": 1, "center": pose(0).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
    let undo = s.history.undo.len();
    s.execute("track.analyze", serde_json::json!({"direction": "forward"})).unwrap();
    assert!(s.is_tracking() && s.track_job.is_none(), "offloaded, not on a thread");
    assert!(s.track_progress().is_some());
    assert!(s.poll_offload());
    assert!(!s.is_tracking() && s.offloaded.is_empty());
    let l = s.active_comp().unwrap().layer(clip).unwrap();
    let tp = l.trackers().next().unwrap().0.track_points().next().unwrap().clone();
    let fc = tp.get("featureCenter").unwrap();
    assert_eq!(fc.keys.len(), 25);
    let v = fc.value_at(crate::tests::track::frame_time(12)).as_vec2();
    assert!((v[0] - pose(12).0[0]).abs() < 0.3 && (v[1] - pose(12).0[1]).abs() < 0.3, "{v:?}");
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Analyze Track");
    assert!(s.events.iter().any(|e| matches!(e, Event::Toast { message, error: false } if message.starts_with("Tracked"))));
    assert!(s.undo());
    let l = s.active_comp().unwrap().layer(clip).unwrap();
    assert!(l.trackers().next().unwrap().0.track_points().next().unwrap().get("featureCenter").unwrap().keys.is_empty());
    // `wait` runs inline.
    s.redo();
    s.execute("track.analyze", serde_json::json!({"direction": "forward", "wait": true})).ok();
    assert!(s.offloaded.is_empty());
}

#[test]
fn cancelling_offloaded_jobs() {
    let (mut s, _, _) = crate::tests::track::setup(crate::tests::track::patch_frames(pose));
    let off = Arc::new(Hanging::default());
    s.offload = Some(off.clone());
    s.exporter = Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() }));
    // Analysis: stopping drops the job, nothing is written.
    s.execute("track.motion", serde_json::json!({})).unwrap();
    let rev = s.revision;
    s.execute("track.analyze", serde_json::json!({})).unwrap();
    assert!(s.is_tracking());
    assert!(s.execute("track.analyze", serde_json::json!({})).is_err(), "one at a time");
    assert!(s.stop_track());
    s.poll_offload();
    assert!(!s.is_tracking());
    assert_eq!(off.0.lock().unwrap().len(), 1);
    assert!(s.events.iter().any(|e| matches!(e, Event::Toast { message, .. } if message.contains("stopped"))));
    assert!(s.revision > rev);
    // Render: the queued items become User Stopped.
    s.execute("renderQueue.add", serde_json::json!({"output": "/a.gif"})).unwrap();
    let ids = s.start_render(false).unwrap();
    assert!(s.is_rendering());
    assert!(s.stop_render());
    s.poll_render();
    assert!(!s.is_rendering());
    assert_eq!(off.0.lock().unwrap().len(), 2);
    let it = s.project.render_queue.iter().find(|i| i.id == ids[0]).unwrap();
    assert_eq!(it.status, RenderStatus::UserStopped);
}

#[test]
fn progress_hook_is_scoped() {
    report(1, 2); // no hook: nothing happens
    let seen = Rc::new(RefCell::new(vec![]));
    let s2 = seen.clone();
    effectcraft_render::passes::block_on(with_hook(Box::new(move |d, t| s2.borrow_mut().push((d, t))), async {
        report(1, 3);
        report(2, 3);
    }));
    report(3, 3);
    assert_eq!(*seen.borrow(), vec![(1, 3), (2, 3)]);
}
