//! A comp's layer stack with structural sharing.
//!
//! Every undoable edit keeps the previous project for undo, so the comp it touches is shared and
//! `Arc::make_mut` copies it. With the layers stored inline that copy is the whole stack, every
//! layer and every keyframe, so an edit costs time in proportion to the comp's size and a script
//! that sets many keys in a big comp goes quadratic. Here each layer sits behind its own `Arc`:
//! copying the stack copies pointers, and only the layer an edit actually changes is copied.
//!
//! The API mirrors the `Vec<Layer>` it replaces. Serialized form is identical (a list of layers).

use std::ops::{Index, IndexMut};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::layer::Layer;

/// Layer #1 first (top of the stack).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers(Vec<Arc<Layer>>);

impl Layers {
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    pub fn with_capacity(n: usize) -> Self {
        Self(Vec::with_capacity(n))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Layer> + ExactSizeIterator + Clone {
        self.0.iter().map(|l| &**l)
    }
    /// Mutable access to every layer: each one that is still shared with an undo state is copied,
    /// so prefer [`Layers::get_mut`] (or `iter` to find, then `get_mut`) when only some change.
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Layer> + ExactSizeIterator {
        self.0.iter_mut().map(Arc::make_mut)
    }
    /// Mutable access to just the layers `keep` accepts. `keep` reads each layer without copying
    /// it; only the matching ones are copied (if shared). Use this, not `iter_mut().filter(..)`.
    pub fn iter_mut_where<'a>(&'a mut self, mut keep: impl FnMut(&Layer) -> bool + 'a) -> impl Iterator<Item = &'a mut Layer> + 'a {
        self.0.iter_mut().filter(move |l| keep(l)).map(Arc::make_mut)
    }
    /// The first layer `pred` accepts, copied only if it is that one. Use this, not `iter_mut().find(..)`.
    pub fn find_mut(&mut self, mut pred: impl FnMut(&Layer) -> bool) -> Option<&mut Layer> {
        self.0.iter_mut().find(|l| pred(l)).map(Arc::make_mut)
    }
    pub fn get(&self, i: usize) -> Option<&Layer> {
        self.0.get(i).map(|l| &**l)
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut Layer> {
        self.0.get_mut(i).map(Arc::make_mut)
    }
    pub fn first(&self) -> Option<&Layer> {
        self.get(0)
    }
    pub fn last(&self) -> Option<&Layer> {
        self.0.last().map(|l| &**l)
    }
    pub fn first_mut(&mut self) -> Option<&mut Layer> {
        self.get_mut(0)
    }
    pub fn last_mut(&mut self) -> Option<&mut Layer> {
        self.0.last_mut().map(Arc::make_mut)
    }
    pub fn push(&mut self, l: Layer) {
        self.0.push(Arc::new(l));
    }
    pub fn insert(&mut self, i: usize, l: Layer) {
        self.0.insert(i, Arc::new(l));
    }
    pub fn remove(&mut self, i: usize) -> Layer {
        Arc::unwrap_or_clone(self.0.remove(i))
    }
    pub fn pop(&mut self) -> Option<Layer> {
        self.0.pop().map(Arc::unwrap_or_clone)
    }
    pub fn retain(&mut self, mut f: impl FnMut(&Layer) -> bool) {
        self.0.retain(|l| f(l));
    }
    pub fn swap(&mut self, a: usize, b: usize) {
        self.0.swap(a, b);
    }
    pub fn truncate(&mut self, n: usize) {
        self.0.truncate(n);
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn reverse(&mut self) {
        self.0.reverse();
    }
    pub fn sort_by_key<K: Ord>(&mut self, mut f: impl FnMut(&Layer) -> K) {
        self.0.sort_by_key(|l| f(l));
    }
    pub fn sort_by(&mut self, mut f: impl FnMut(&Layer, &Layer) -> std::cmp::Ordering) {
        self.0.sort_by(|a, b| f(a, b));
    }
    pub fn rotate_left(&mut self, n: usize) {
        self.0.rotate_left(n);
    }
    pub fn rotate_right(&mut self, n: usize) {
        self.0.rotate_right(n);
    }
    /// `layers[range].rotate_left(n)` (a slice of `Arc`s can't be handed out as a `&mut [Layer]`).
    pub fn rotate_range_left(&mut self, r: impl std::slice::SliceIndex<[Arc<Layer>], Output = [Arc<Layer>]>, n: usize) {
        if let Some(s) = self.0.get_mut(r) {
            s.rotate_left(n);
        }
    }
    pub fn rotate_range_right(&mut self, r: impl std::slice::SliceIndex<[Arc<Layer>], Output = [Arc<Layer>]>, n: usize) {
        if let Some(s) = self.0.get_mut(r) {
            s.rotate_right(n);
        }
    }
    pub fn split_off(&mut self, at: usize) -> Layers {
        Layers(self.0.split_off(at))
    }
    pub fn append(&mut self, other: &mut Layers) {
        self.0.append(&mut other.0);
    }
    /// Remove a range of layers and return them (owned).
    pub fn drain(&mut self, r: impl std::ops::RangeBounds<usize>) -> std::vec::IntoIter<Layer> {
        self.0.drain(r).map(Arc::unwrap_or_clone).collect::<Vec<_>>().into_iter()
    }
    /// Whether `self` and `other` hold the very same (unchanged) layer at `i`: a cheap "did this
    /// layer change" test between two project states.
    pub fn same_at(&self, other: &Layers, i: usize) -> bool {
        matches!((self.0.get(i), other.0.get(i)), (Some(a), Some(b)) if Arc::ptr_eq(a, b))
    }
    /// The shared layers themselves, for parallel iteration (`rayon` over `&[Arc<Layer>]`).
    pub fn as_arcs(&self) -> &[Arc<Layer>] {
        &self.0
    }
    pub fn to_vec(&self) -> Vec<Layer> {
        self.iter().cloned().collect()
    }
}

impl Index<usize> for Layers {
    type Output = Layer;
    fn index(&self, i: usize) -> &Layer {
        &self.0[i]
    }
}

impl IndexMut<usize> for Layers {
    fn index_mut(&mut self, i: usize) -> &mut Layer {
        Arc::make_mut(&mut self.0[i])
    }
}

impl From<Vec<Layer>> for Layers {
    fn from(v: Vec<Layer>) -> Self {
        Self(v.into_iter().map(Arc::new).collect())
    }
}

impl FromIterator<Layer> for Layers {
    fn from_iter<I: IntoIterator<Item = Layer>>(it: I) -> Self {
        Self(it.into_iter().map(Arc::new).collect())
    }
}

impl Extend<Layer> for Layers {
    fn extend<I: IntoIterator<Item = Layer>>(&mut self, it: I) {
        self.0.extend(it.into_iter().map(Arc::new));
    }
}

impl IntoIterator for Layers {
    type Item = Layer;
    type IntoIter = std::iter::Map<std::vec::IntoIter<Arc<Layer>>, fn(Arc<Layer>) -> Layer>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().map(Arc::unwrap_or_clone as fn(Arc<Layer>) -> Layer)
    }
}

impl<'a> IntoIterator for &'a Layers {
    type Item = &'a Layer;
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Arc<Layer>>, fn(&'a Arc<Layer>) -> &'a Layer>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|l| &**l)
    }
}

impl<'a> IntoIterator for &'a mut Layers {
    type Item = &'a mut Layer;
    type IntoIter = std::iter::Map<std::slice::IterMut<'a, Arc<Layer>>, fn(&'a mut Arc<Layer>) -> &'a mut Layer>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut().map(Arc::make_mut)
    }
}

impl Serialize for Layers {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Layers {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Vec::<Layer>::deserialize(d).map(Layers::from)
    }
}
