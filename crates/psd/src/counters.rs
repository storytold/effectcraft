use std::cell::Cell;
use std::thread::LocalKey;

thread_local! {
    /// Scratch rows allocated by the 32-bit ZIP un-predictor.
    pub static SCRATCH: Cell<usize> = const { Cell::new(0) };
    /// `inside the mask rect` evaluations of the layer-mask loop.
    pub static INSIDE: Cell<usize> = const { Cell::new(0) };
    /// `square_to_quad` solves.
    pub static QUAD: Cell<usize> = const { Cell::new(0) };
}

pub fn bump(c: &'static LocalKey<Cell<usize>>) {
    c.with(|c| c.set(c.get() + 1));
}
/// Read and reset.
pub fn take(c: &'static LocalKey<Cell<usize>>) -> usize {
    c.with(|c| c.replace(0))
}
