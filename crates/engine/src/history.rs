//! Branching undo history (the History panel; better than After Effects, whose undo is a
//! straight line).
//!
//! [`History`] keeps After Effects' linear model as its working line — `undo` (the states
//! before each step, oldest first) and `redo` (undone states, next first) — and adds
//! **branches**: when you undo and then do something new, the undone states are not thrown
//! away but kept as a [`Branch`] hanging off the state they grew from. [`History::tree`] lists
//! every state as a tree and [`Session::goto_history`] jumps to any of them, on any branch: the
//! path to the target becomes the working line, and the line you left becomes a branch.
//!
//! States are whole-project snapshots (`Arc<Project>`), identified by their address while the
//! history holds them ([`HistoryNode::id`]), so ids stay valid across undo, redo and jumps.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_project::Project;
use serde::Serialize;

use crate::Session;

/// Label of the oldest state in the history (the project as opened, or the oldest state kept).
pub const ROOT_LABEL: &str = "Original";

/// Undo history of whole-project snapshots.
#[derive(Clone, Default)]
pub struct History {
    /// The states before each step on the working line, oldest first: (step label, state).
    pub undo: Vec<(String, Arc<Project>)>,
    /// Undone steps, the next one last: (step label, the state it leads to).
    pub redo: Vec<(String, Arc<Project>)>,
    /// Key of the last merged step: a continuous gesture with the same key folds into one step.
    pub merge_key: Option<String>,
    /// Alternative futures kept when a new step was made after undoing.
    pub branches: Vec<Branch>,
}

/// A line of states that grows from another state of the history.
#[derive(Clone)]
pub struct Branch {
    /// The state the branch grows from.
    pub parent: Arc<Project>,
    /// The branch's states in order: (label of the step that made it, state).
    pub steps: Vec<(String, Arc<Project>)>,
}

/// One state of the history tree (`edit.history.list`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryNode {
    /// Position in the listing (depth-first, the working line's continuation first).
    pub index: usize,
    /// Stable id of the state while the history keeps it.
    pub id: String,
    /// The step that made this state ([`ROOT_LABEL`] for the oldest).
    pub label: String,
    /// Index of the state it was made from.
    pub parent: Option<usize>,
    /// Branch nesting (0 = the working line).
    pub depth: usize,
    /// The current state.
    pub current: bool,
    /// On the working line (Undo / Redo walk it).
    pub line: bool,
    /// On the working line after the current state (Redo reaches it).
    pub future: bool,
}

fn key(p: &Arc<Project>) -> usize {
    Arc::as_ptr(p) as usize
}

/// Id of a state (its address while the history holds it).
pub fn state_id(p: &Arc<Project>) -> String {
    format!("s{:x}", key(p))
}

/// The whole tree as an arena (internal).
struct Arena {
    label: Vec<String>,
    state: Vec<Arc<Project>>,
    parent: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    /// Index of each working-line state, oldest first, and the current state's position in it.
    line: Vec<usize>,
    current: usize,
}

impl Arena {
    fn build(h: &History, current: &Arc<Project>) -> Arena {
        let mut a = Arena { label: vec![], state: vec![], parent: vec![], children: vec![], line: vec![], current: 0 };
        let add = |a: &mut Arena, label: String, st: Arc<Project>, parent: Option<usize>| {
            let i = a.state.len();
            a.label.push(label);
            a.state.push(st);
            a.parent.push(parent);
            a.children.push(vec![]);
            if let Some(p) = parent {
                a.children[p].push(i);
            }
            i
        };
        // The working line: undo states, the current state, then the redo states.
        let mut prev: Option<usize> = None;
        let mut next_label = ROOT_LABEL.to_string();
        for (label, st) in &h.undo {
            let i = add(&mut a, std::mem::take(&mut next_label), st.clone(), prev);
            a.line.push(i);
            prev = Some(i);
            next_label = label.clone();
        }
        let cur = add(&mut a, next_label, current.clone(), prev);
        a.line.push(cur);
        a.current = cur;
        prev = Some(cur);
        for (label, st) in h.redo.iter().rev() {
            let i = add(&mut a, label.clone(), st.clone(), prev);
            a.line.push(i);
            prev = Some(i);
        }
        // Branches, attached to whichever state they grow from (branches of branches need the
        // parent branch first, hence the passes). Orphans (their state was dropped) are skipped.
        let mut index: HashMap<usize, usize> = a.state.iter().enumerate().map(|(i, s)| (key(s), i)).collect();
        let mut pending: Vec<&Branch> = h.branches.iter().collect();
        loop {
            let before = pending.len();
            pending.retain(|b| {
                let Some(&p) = index.get(&key(&b.parent)) else { return true };
                let mut prev = p;
                for (label, st) in &b.steps {
                    if index.contains_key(&key(st)) {
                        // Already in the tree (shared state): continue from it.
                        prev = index[&key(st)];
                        continue;
                    }
                    let i = add(&mut a, label.clone(), st.clone(), Some(prev));
                    index.insert(key(st), i);
                    prev = i;
                }
                false
            });
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        a
    }

    fn on_line(&self) -> Vec<bool> {
        let mut v = vec![false; self.state.len()];
        for &i in &self.line {
            v[i] = true;
        }
        v
    }

    /// Depth-first listing: the working line's child first, then branches in creation order.
    fn listing(&self) -> Vec<HistoryNode> {
        let on_line = self.on_line();
        let pos_in_line: HashMap<usize, usize> = self.line.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        let cur_pos = pos_in_line[&self.current];
        let mut out: Vec<HistoryNode> = vec![];
        let mut order: HashMap<usize, usize> = HashMap::new();
        // (node, depth)
        let mut stack: Vec<(usize, usize)> = vec![(self.line[0], 0)];
        while let Some((n, depth)) = stack.pop() {
            order.insert(n, out.len());
            let line = on_line[n];
            out.push(HistoryNode {
                index: out.len(),
                id: state_id(&self.state[n]),
                label: self.label[n].clone(),
                parent: self.parent[n].and_then(|p| order.get(&p).copied()),
                depth,
                current: n == self.current,
                line,
                future: line && pos_in_line[&n] > cur_pos,
            });
            // Push in reverse so the first child pops first; the line child continues at the
            // same depth, branch children one deeper; a branch's own continuation (its first
            // child) stays at the branch's depth.
            for (k, &c) in self.children[n].iter().enumerate().rev() {
                let cont = if line { on_line[c] } else { k == 0 };
                stack.push((c, if cont { depth } else { depth + 1 }));
            }
        }
        out
    }
}

impl History {
    /// Record a new step made from `before`: the redo line (if any) is kept as a branch.
    pub fn record(&mut self, label: &str, before: Arc<Project>, levels: usize) {
        self.abandon_redo(&before);
        self.undo.push((label.to_string(), before.clone()));
        self.merge_key = None;
        self.trim(levels, &before);
    }

    /// Keep the undone states as a branch growing from `from` (the state a new step starts from).
    pub fn abandon_redo(&mut self, from: &Arc<Project>) {
        if self.redo.is_empty() {
            return;
        }
        let steps: Vec<(String, Arc<Project>)> = self.redo.drain(..).rev().collect();
        self.branches.push(Branch { parent: from.clone(), steps });
    }

    /// Apply the undo-levels limit: drop the oldest working-line states, the branches that grew
    /// from them, and the oldest branches when branches hold more states than `levels`.
    pub fn trim(&mut self, levels: usize, current: &Arc<Project>) {
        let levels = levels.max(1);
        if self.undo.len() > levels {
            let extra = self.undo.len() - levels;
            self.undo.drain(..extra);
        }
        let mut total: usize = self.branches.iter().map(|b| b.steps.len()).sum();
        while total > levels && !self.branches.is_empty() {
            total -= self.branches.remove(0).steps.len();
        }
        self.prune(current);
    }

    /// Drop branches whose parent state is no longer in the history (`current` = the current
    /// state, which the history doesn't store).
    pub fn prune(&mut self, current: &Arc<Project>) {
        if self.branches.is_empty() {
            return;
        }
        let mut live: std::collections::HashSet<usize> = self.undo.iter().chain(self.redo.iter()).map(|(_, p)| key(p)).collect();
        live.insert(key(current));
        let mut keep = vec![false; self.branches.len()];
        loop {
            let mut changed = false;
            for (i, b) in self.branches.iter().enumerate() {
                if !keep[i] && live.contains(&key(&b.parent)) {
                    keep[i] = true;
                    live.extend(b.steps.iter().map(|(_, p)| key(p)));
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut k = keep.into_iter();
        self.branches.retain(|_| k.next().unwrap_or(false));
    }

    /// Drop everything (Edit ▸ Purge ▸ Undo, New / Open Project).
    pub fn clear(&mut self) {
        *self = History::default();
    }

    /// The history as a tree, given the current state.
    pub fn tree(&self, current: &Arc<Project>) -> Vec<HistoryNode> {
        Arena::build(self, current).listing()
    }

    /// Number of states on branches (not on the working line).
    pub fn branch_states(&self) -> usize {
        self.branches.iter().map(|b| b.steps.len()).sum()
    }
}

impl Session {
    /// The undo history as a tree (the History panel, `edit.history.list`).
    pub fn history_tree(&self) -> Vec<HistoryNode> {
        self.history.tree(&self.project)
    }

    /// Jump to any state of the history (`id` from [`HistoryNode::id`]), including states on
    /// other branches: the path to it becomes the working line (its undone continuation the
    /// redo line), and the line left behind is kept as a branch. Returns false for an unknown
    /// id.
    pub fn goto_history(&mut self, id: &str) -> bool {
        let a = Arena::build(&self.history, &self.project);
        let Some(target) = a.state.iter().position(|s| state_id(s) == id) else { return false };
        if target == a.current {
            return true;
        }
        let on_line = a.on_line();
        // Path root → target.
        let mut path = vec![target];
        while let Some(p) = a.parent[*path.last().unwrap_or(&target)] {
            path.push(p);
        }
        path.reverse();
        // Continue past the target: the working line where it runs on, else each branch's own
        // continuation (its first child).
        let mut new_line = path.clone();
        let mut n = target;
        loop {
            let kids = &a.children[n];
            let next = if on_line[n] { kids.iter().copied().find(|&c| on_line[c]) } else { kids.first().copied() };
            match next {
                Some(c) => {
                    new_line.push(c);
                    n = c;
                }
                None => break,
            }
        }
        let mut in_new = vec![false; a.state.len()];
        for &i in &new_line {
            in_new[i] = true;
        }
        // Every state off the new line goes into branches: a chain per first child, the other
        // children starting branches of their own.
        fn chain(a: &Arena, start: usize, parent: usize, in_new: &[bool], out: &mut Vec<Branch>) {
            let mut steps = vec![];
            let mut n = start;
            let mut extra: Vec<(usize, usize)> = vec![];
            loop {
                steps.push((a.label[n].clone(), a.state[n].clone()));
                let kids: Vec<usize> = a.children[n].iter().copied().filter(|&c| !in_new[c]).collect();
                let Some((&first, rest)) = kids.split_first() else { break };
                extra.extend(rest.iter().map(|&c| (c, n)));
                n = first;
            }
            out.push(Branch { parent: a.state[parent].clone(), steps });
            for (c, p) in extra {
                chain(a, c, p, in_new, out);
            }
        }
        let mut branches = vec![];
        for &n in &new_line {
            for &c in &a.children[n] {
                if !in_new[c] {
                    chain(&a, c, n, &in_new, &mut branches);
                }
            }
        }
        let k = new_line.iter().position(|&i| i == target).unwrap_or(0);
        let undo: Vec<(String, Arc<Project>)> = (0..k).map(|i| (a.label[new_line[i + 1]].clone(), a.state[new_line[i]].clone())).collect();
        let redo: Vec<(String, Arc<Project>)> = new_line[k + 1..].iter().rev().map(|&i| (a.label[i].clone(), a.state[i].clone())).collect();
        let cur = std::mem::replace(&mut self.project, a.state[target].clone());
        self.keep_view_settings(&cur);
        self.history.undo = undo;
        self.history.redo = redo;
        self.history.branches = branches;
        self.history.merge_key = None;
        self.sanitize_state();
        self.bump();
        true
    }
}
