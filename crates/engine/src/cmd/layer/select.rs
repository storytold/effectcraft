use super::*;

pub(super) fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ids: Vec<LayerId> = match p.get("layers").or(p.get("layer")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| resolve_layer(comp, v)).collect(),
        Some(v) => resolve_layer(comp, v).into_iter().collect(),
        None => vec![],
    };
    let add = b_p(p, "add").unwrap_or(false);
    let toggle = b_p(p, "toggle").unwrap_or(false);
    if toggle {
        for id in ids {
            if let Some(i) = s.state.selected_layers.iter().position(|l| *l == id) {
                s.state.selected_layers.remove(i);
            } else {
                s.state.selected_layers.push(id);
            }
        }
    } else if add {
        for id in ids {
            if !s.state.selected_layers.contains(&id) {
                s.state.selected_layers.push(id);
            }
        }
    } else {
        s.state.selected_layers = ids;
        s.state.selected_props.clear();
        s.state.selected_keys.clear();
        let keep = s.state.selected_layers.clone();
        s.state.selected_vertices.retain(|v| keep.contains(&v.layer));
    }
    Ok(json!(s.state.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>()))
}

pub(super) fn select_step(s: &mut Session, p: &Value, dir: i64) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    if comp.layers.is_empty() {
        return Ok(Value::Null);
    }
    let cur = s.state.selected_layers.first().and_then(|id| comp.layers.iter().position(|l| l.id == *id));
    let n = comp.layers.len() as i64;
    // The next layer that can be selected (locked and hidden shy layers are stepped over); at
    // the end of the stack the selection stays.
    let start = match cur {
        Some(i) => i as i64 + dir,
        None if dir > 0 => 0,
        None => n - 1,
    };
    let next = std::iter::successors(Some(start), |i| Some(i + dir))
        .take_while(|i| (0..n).contains(i))
        .find(|i| crate::cmd::selectable(comp, &comp.layers[*i as usize]));
    let Some(i) = next.or(cur.map(|i| i as i64)) else { return Ok(Value::Null) };
    let id = comp.layers[i as usize].id;
    let add = b_p(p, "add").unwrap_or(false);
    if add {
        if !s.state.selected_layers.contains(&id) {
            s.state.selected_layers.insert(0, id);
        }
    } else {
        s.state.selected_layers = vec![id];
    }
    Ok(json!(id.0))
}

pub(super) fn select_next(s: &mut Session, p: &Value) -> Result<Value> {
    select_step(s, p, 1)
}

pub(super) fn select_prev(s: &mut Session, p: &Value) -> Result<Value> {
    select_step(s, p, -1)
}
