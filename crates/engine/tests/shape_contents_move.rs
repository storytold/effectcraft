//! Shape Contents moves preserve animated item identity and undo atomically.

use effectcraft_engine::Session;
use effectcraft_project::{LayerId, PropGroup};
use serde_json::{Value, json};

fn setup() -> (Session, u64, u64, u64) {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Shapes", "width": 320, "height": 180, "duration": 4})).unwrap();
    let layer = session.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let contents = props(&session, layer).sub("contents").unwrap();
    let root = contents.uid;
    let group = contents.groups().next().unwrap().uid;
    (session, layer, root, group)
}

fn props(session: &Session, layer: u64) -> &PropGroup {
    &session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props
}

fn add(session: &mut Session, layer: u64, kind: &str, group: Option<u64>) -> u64 {
    let mut params = json!({"layer": layer, "kind": kind});
    if let Some(group) = group {
        params["group"] = json!(group);
    }
    session.execute("layer.addShapeItem", params).unwrap()["uid"].as_u64().unwrap()
}

fn children(session: &Session, layer: u64, parent: u64) -> Vec<u64> {
    props(session, layer).find_group(parent).unwrap().children.iter().map(|node| node.uid()).collect()
}

fn project(session: &Session) -> Value {
    serde_json::to_value(session.project.as_ref()).unwrap()
}

fn rejected(session: &mut Session, params: Value) {
    let before = project(session);
    let history = session.history.undo.len();
    assert!(session.execute("prop.moveGroup", params).is_err());
    assert_eq!(project(session), before, "a rejected move cannot lose any content");
    assert_eq!(session.history.undo.len(), history, "a rejected move adds no undo step");
}

#[test]
fn root_stroke_moves_into_shape_group_with_its_keys_and_one_undo() {
    let (mut session, layer, root, group) = setup();
    let stroke = add(&mut session, layer, "stroke", None);
    let width = props(&session, layer).find_group(stroke).unwrap().get("width").unwrap().uid;
    session.execute("prop.addKey", json!({"layer": layer, "prop": width, "time": 0, "value": 2})).unwrap();
    session.execute("prop.addKey", json!({"layer": layer, "prop": width, "time": 1, "value": 12})).unwrap();
    let stroke_before = props(&session, layer).find_group(stroke).unwrap().clone();
    let before = project(&session);
    let history = session.history.undo.len();
    let result = session.execute_checked("prop.moveGroup", json!({"layer": layer, "prop": stroke, "parent": group})).unwrap();
    let contents = props(&session, layer).find_group(group).unwrap().sub("contents").unwrap().uid;
    assert_eq!(result["parent"], contents);
    assert!(!children(&session, layer, root).contains(&stroke));
    assert_eq!(children(&session, layer, contents).last(), Some(&stroke));
    assert_eq!(props(&session, layer).find_group(stroke).unwrap(), &stroke_before);
    assert_eq!(session.history.undo.len(), history + 1);
    let after = project(&session);
    session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(project(&session), before);
    session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(project(&session), after);
}

#[test]
fn selected_items_move_together_in_visual_order_and_a_repeat_is_a_noop() {
    let (mut session, layer, root, group) = setup();
    let stroke = add(&mut session, layer, "stroke", None);
    let trim = add(&mut session, layer, "trim", None);
    let contents = props(&session, layer).find_group(group).unwrap().sub("contents").unwrap().uid;
    let history = session.history.undo.len();
    let params = json!({"layer": layer, "props": [trim, stroke, stroke], "parent": contents});
    let result = session.execute_checked("prop.moveGroup", params.clone()).unwrap();
    assert_eq!(result["props"], json!([stroke, trim]));
    let items = children(&session, layer, contents);
    assert_eq!(&items[items.len() - 2..], &[stroke, trim]);
    assert_eq!(children(&session, layer, root), vec![group]);
    assert_eq!(session.state.selected_props, vec![(LayerId(layer), stroke), (LayerId(layer), trim)]);
    assert_eq!(session.history.undo.len(), history + 1);
    session.execute("prop.moveGroup", params).unwrap();
    assert_eq!(session.history.undo.len(), history + 1, "dropping in place adds no history");
}

#[test]
fn groups_reorder_as_siblings_and_selected_descendants_travel_once() {
    let (mut session, layer, root, group) = setup();
    let other = add(&mut session, layer, "group", None);
    let stroke = add(&mut session, layer, "stroke", Some(group));
    session.execute("prop.moveGroup", json!({"layer": layer, "props": [group, stroke], "parent": root, "index": 2})).unwrap();
    assert_eq!(children(&session, layer, root), vec![other, group]);
    assert!(props(&session, layer).find_group(group).unwrap().sub("contents").unwrap().children.iter().any(|n| n.uid() == stroke));
    // Existing scripting moveTo(index) remains compatible and returns its numeric position.
    assert_eq!(session.execute("prop.moveGroup", json!({"layer": layer, "prop": group, "index": 1})).unwrap(), json!(1));
    assert_eq!(children(&session, layer, root), vec![group, other]);
}

#[test]
fn cycle_fixed_groups_bad_indices_and_locked_layers_are_rejected_without_changes() {
    let (mut session, layer, root, group) = setup();
    let nested = add(&mut session, layer, "group", Some(group));
    let stroke = add(&mut session, layer, "stroke", None);
    let transform = props(&session, layer).find_group(group).unwrap().sub("transform").unwrap().uid;
    rejected(&mut session, json!({"layer": layer, "prop": group, "parent": group}));
    rejected(&mut session, json!({"layer": layer, "prop": group, "parent": nested}));
    rejected(&mut session, json!({"layer": layer, "prop": transform, "parent": root}));
    rejected(&mut session, json!({"layer": layer, "prop": stroke, "parent": transform}));
    rejected(&mut session, json!({"layer": layer, "prop": stroke, "parent": root, "index": 0}));
    rejected(&mut session, json!({"layer": layer, "prop": stroke, "parent": root, "index": u64::MAX}));
    session.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
    rejected(&mut session, json!({"layer": layer, "prop": stroke, "parent": group}));
}

#[test]
fn reparenting_cannot_exceed_the_shape_tree_depth_limit() {
    let (mut session, layer, _, group) = setup();
    let mut deepest = group;
    for _ in 0..30 {
        deepest = add(&mut session, layer, "group", Some(deepest));
    }
    let source = add(&mut session, layer, "group", None);
    add(&mut session, layer, "group", Some(source));
    rejected(&mut session, json!({"layer": layer, "prop": source, "parent": deepest}));
}

#[test]
fn a_contents_named_group_inside_a_paint_is_not_a_valid_destination() {
    let (mut session, layer, _, group) = setup();
    let stroke = add(&mut session, layer, "stroke", None);
    let comp = session.active_comp_id().unwrap();
    let fake = session
        .edit("Synthetic malformed paint", None, |project, _| {
            project.next_id += 1;
            let uid = project.next_id;
            let paint = project.comp_mut(comp).unwrap().layer_mut(LayerId(layer)).unwrap().props.find_group_mut(stroke).unwrap();
            paint.children.push(PropGroup::new(uid, "contents", "Contents").into());
            Ok(uid)
        })
        .unwrap();
    rejected(&mut session, json!({"layer": layer, "prop": group, "parent": fake}));
}

#[test]
fn an_extremely_wide_contents_tree_is_rejected_before_building_a_large_traversal_stack() {
    let (mut session, layer, root, group) = setup();
    let comp = session.active_comp_id().unwrap();
    session
        .edit("Synthetic wide contents", None, |project, _| {
            let contents = project.comp_mut(comp).unwrap().layer_mut(LayerId(layer)).unwrap().props.find_group_mut(root).unwrap();
            for uid in 1_000_000..1_100_001 {
                contents.children.push(PropGroup::new(uid, "group", "Empty synthetic group").into());
            }
            project.next_id = 1_100_001;
            Ok(())
        })
        .unwrap();
    let history = session.history.undo.len();
    let count = children(&session, layer, root).len();
    assert!(session.execute("prop.moveGroup", json!({"layer": layer, "prop": group, "parent": root})).is_err());
    assert_eq!(session.history.undo.len(), history);
    assert_eq!(props(&session, layer).find_group(root).unwrap().children.len(), count);
}
