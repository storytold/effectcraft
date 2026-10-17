//! Bounded, frame-spaced search for held expression motion. A sparse search cannot distinguish
//! a constant position from a short excursion returning to that position. Never enlarge the
//! sampling interval with the duration of the hold. Reuse already examined stationary intervals.
//! Coverage is at composition-frame resolution, within 60,000 frames in either direction,
//! at most 65,536 probes per search (including nested searches). Never use wall-clock time to
//! choose a heading: machine load must not change a successfully evaluated frame. A warm cache must
//! not extend that horizon. Subframe-only excursions require baking Position keys.
use effectcraft_geom::Vec3;
use effectcraft_time::Tick;
use std::cell::Cell;
use std::sync::Mutex;

const MAX_PROBES: usize = 65_536;
const MAX_HISTORY_FRAMES: i64 = 60_000;
const MAX_INTERVALS: usize = 64;

#[derive(Clone, Copy)]
struct Interval {
    key: u64,
    start: Tick,
    end: Tick,
    position: Vec3,
    heading: Option<Vec3>,
}

// Preview/export workers share completed intervals. Never hold this lock while evaluating JS.
static HELD: Mutex<Vec<Interval>> = Mutex::new(Vec::new());
thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static PROBES: Cell<usize> = const { Cell::new(MAX_PROBES) };
}

struct SearchGuard;
impl SearchGuard {
    fn enter() -> Option<Self> {
        let depth = DEPTH.get();
        if depth >= 16 {
            PROBES.set(0);
            return None;
        }
        if depth == 0 {
            PROBES.set(MAX_PROBES);
        }
        DEPTH.set(depth + 1);
        Some(Self)
    }
}
impl Drop for SearchGuard {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get().saturating_sub(1));
    }
}

fn probe() -> bool {
    let n = PROBES.get();
    PROBES.set(n.saturating_sub(1));
    n > 0
}

fn direction(v: Vec3) -> Option<Vec3> {
    let length = v.length();
    (length > 1e-9 && length.is_finite()).then(|| v.normalize())
}

fn cached(key: Option<u64>, t: Tick, position: Vec3, earliest: Tick) -> Option<Interval> {
    let key = key?;
    HELD.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .rev()
        .find(|e| e.key == key && e.start <= t && t <= e.end && (e.heading.is_none() || e.start >= earliest) && (e.position - position).length() <= 1e-9)
        .copied()
}

fn remember(key: Option<u64>, start: Tick, end: Tick, position: Vec3, heading: Option<Vec3>) {
    let Some(key) = key else { return };
    // A nested expression may have exhausted the shared budget; never persist partial results.
    if PROBES.get() == 0 {
        return;
    }
    {
        let mut entries = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Coalesce overlapping intervals, so sequential playback costs one new probe per frame.
        if let Some(e) =
            entries.iter_mut().find(|e| e.key == key && e.start <= end && start <= e.end && (e.position - position).length() <= 1e-9 && e.heading == heading)
        {
            e.start = e.start.min(start);
            e.end = e.end.max(end);
        } else {
            if entries.len() >= MAX_INTERVALS {
                entries.remove(0);
            }
            entries.push(Interval { key, start, end, position, heading });
        }
    }
}

enum Search {
    Found(Vec3),
    Empty,
    Exhausted,
    Cancelled,
}

/// No cache borrow survives a position evaluation: expressions may recursively read transforms.
pub(super) fn held(
    pos: &impl Fn(Tick) -> Vec3,
    time: Tick,
    start: Tick,
    end: Tick,
    frame: Tick,
    dt: Tick,
    key: Option<u64>,
    hold_start: &impl Fn() -> Option<Tick>,
    valid: &impl Fn() -> bool,
    cancelled: &impl Fn() -> bool,
) -> Option<Vec3> {
    let _guard = SearchGuard::enter()?;
    if cancelled() {
        return None;
    }
    let reach = frame.max(Tick(1)).mul_ratio(MAX_HISTORY_FRAMES, 1);
    let earliest = start.max(time - reach);
    let latest = end.min(time + reach);
    let position = pos(time);
    if let Some(hit) = cached(key.filter(|_| valid()), time, position, earliest) {
        return hit.heading;
    }
    let mut remaining = MAX_PROBES;
    let skip = hold_start().map(|t| t.clamp(earliest, time));
    match search(pos, time, earliest, frame, dt, key, position, skip, &mut remaining, valid, cancelled) {
        Search::Found(d) => Some(d),
        Search::Empty if earliest > start => exhausted(),
        Search::Empty => match search(pos, time, latest, frame, dt, key, position, None, &mut remaining, valid, cancelled) {
            Search::Found(d) => Some(d),
            Search::Empty if latest < end => exhausted(),
            Search::Empty => {
                if cancelled() {
                    return None;
                }
                remember(key.filter(|_| valid()), start, end, position, None);
                None
            }
            Search::Exhausted => exhausted(),
            Search::Cancelled => None,
        },
        // Do not claim there was no earlier motion when the search budget was exhausted.
        Search::Exhausted => exhausted(),
        Search::Cancelled => None,
    }
}

fn exhausted() -> Option<Vec3> {
    log::warn!("Auto-orient could not finish sampling Position within its work budget; bake the expression to keyframes for this long/expensive path");
    None
}

#[allow(clippy::too_many_arguments)]
fn search(
    pos: &impl Fn(Tick) -> Vec3,
    time: Tick,
    bound: Tick,
    frame: Tick,
    dt: Tick,
    key: Option<u64>,
    held: Vec3,
    skip: Option<Tick>,
    remaining: &mut usize,
    valid: &impl Fn() -> bool,
    cancelled: &impl Fn() -> bool,
) -> Search {
    if bound == time {
        return Search::Empty;
    }
    let backwards = bound < time;
    let frame = frame.max(Tick(1));
    let mut near = skip.unwrap_or(time);
    while *remaining > 0 {
        if cancelled() {
            return Search::Cancelled;
        }
        if !probe() {
            return Search::Exhausted;
        }
        *remaining -= 1;
        // A composition-aligned grid makes the coverage independent of seek/subframe phase.
        let index = near.0.div_euclid(frame.0);
        let grid = if backwards {
            Tick(if near.0.rem_euclid(frame.0) == 0 { index.saturating_sub(1) } else { index }.saturating_mul(frame.0))
        } else {
            Tick(index.saturating_add(1).saturating_mul(frame.0))
        };
        let mut far = if backwards { grid.max(bound) } else { grid.min(bound) };
        if far == near {
            return Search::Empty;
        }
        if backwards
            && let Some(hit) = cached(key.filter(|_| valid()), far, held, bound)
            && let Some(heading) = hit.heading
        {
            remember(key.filter(|_| valid()), hit.start, time, held, Some(heading));
            return Search::Found(heading);
        }
        if direction(pos(far) - held).is_some() {
            let precision = Tick::from_seconds_f64(1e-7);
            for _ in 0..40 {
                if (far - near).abs() <= precision {
                    break;
                }
                if cancelled() {
                    return Search::Cancelled;
                }
                if !probe() {
                    return Search::Exhausted;
                }
                let mid = near + (far - near).mul_ratio(1, 2);
                if direction(pos(mid) - held).is_some() {
                    far = mid;
                } else {
                    near = mid;
                }
            }
            let delta = if backwards { pos(near) - pos(near - dt) } else { pos(near + dt) - pos(near) };
            if cancelled() {
                return Search::Cancelled;
            }
            if let Some(d) = direction(delta) {
                // Only cache the backward search: a pre-start heading must not be mistaken for
                // a previous heading when a later, different movement is evaluated.
                if backwards {
                    remember(key.filter(|_| valid()), near, time, held, Some(d));
                }
                return Search::Found(d);
            }
        }
        if far == bound {
            return Search::Empty;
        }
        near = far;
    }
    Search::Exhausted
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_geom::vec3;
    use std::cell::Cell;

    #[test]
    fn cached_hold_is_reused_extended_and_invalidated_by_identity() {
        let calls = Cell::new(0);
        let position = |t: Tick| {
            calls.set(calls.get() + 1);
            vec3(-t.seconds().clamp(0.0, 1.0), 0.0, 0.0)
        };
        let run = |t, key| {
            held(
                &position,
                Tick::from_seconds_f64(t),
                Tick::ZERO,
                Tick::from_seconds_f64(1800.0),
                Tick::from_units(1, 30),
                Tick::from_seconds_f64(0.005),
                Some(key),
                &|| None,
                &|| true,
                &|| false,
            )
        };
        assert_eq!(run(900.0, 1), Some(vec3(-1.0, 0.0, 0.0)));
        assert!(calls.get() > 20_000);
        calls.set(0);
        assert_eq!(run(899.0, 1), Some(vec3(-1.0, 0.0, 0.0)));
        assert_eq!(run(900.0 + 1.0 / 30.0, 1), Some(vec3(-1.0, 0.0, 0.0)));
        assert!(calls.get() <= 4, "warm/next frame used {} samples", calls.get());
        std::thread::spawn(|| {
            let count = Cell::new(0);
            let p = |t: Tick| {
                count.set(count.get() + 1);
                vec3(-t.seconds().clamp(0.0, 1.0), 0.0, 0.0)
            };
            assert_eq!(
                held(
                    &p,
                    Tick::from_seconds_f64(899.0),
                    Tick::ZERO,
                    Tick::from_seconds_f64(1800.0),
                    Tick::from_units(1, 30),
                    Tick::from_seconds_f64(0.005),
                    Some(1),
                    &|| None,
                    &|| true,
                    &|| false,
                ),
                Some(vec3(-1.0, 0.0, 0.0))
            );
            assert_eq!(count.get(), 1, "a new preview worker must reuse completed history");
        })
        .join()
        .unwrap();
        calls.set(0);
        assert_eq!(run(900.0, 2), Some(vec3(-1.0, 0.0, 0.0)));
        assert!(calls.get() > 20_000, "new definition must recompute motion");
    }

    #[test]
    fn slow_position_keeps_the_same_heading_as_fast_position() {
        // A stall used to exhaust the five-second clock and silently return no heading.
        // Keep the history tiny: this tests machine-speed independence, not search volume.
        let stalled = Cell::new(false);
        let position = |t: Tick| {
            if !stalled.replace(true) {
                std::thread::sleep(std::time::Duration::from_millis(5100));
            }
            vec3(0.0, -t.seconds().clamp(0.0, 1.0), 0.0)
        };
        let result = held(
            &position,
            Tick::from_seconds_f64(2.0),
            Tick::ZERO,
            Tick::from_seconds_f64(3.0),
            Tick::from_units(1, 30),
            Tick::from_seconds_f64(0.005),
            None,
            &|| None,
            &|| true,
            &|| false,
        );
        assert_eq!(result, Some(vec3(0.0, -1.0, 0.0)));
    }

    #[test]
    fn cancelled_history_stops_promptly_and_is_not_cached() {
        let calls = Cell::new(0);
        let position = |t: Tick| {
            calls.set(calls.get() + 1);
            vec3(-t.seconds().clamp(0.0, 1.0), 0.0, 0.0)
        };
        let result = held(
            &position,
            Tick::from_seconds_f64(900.0),
            Tick::ZERO,
            Tick::from_seconds_f64(1800.0),
            Tick::from_units(1, 30),
            Tick::from_seconds_f64(0.005),
            Some(0xCA11CE1),
            &|| None,
            &|| true,
            &|| calls.get() >= 12,
        );
        assert_eq!(result, None);
        assert_eq!(calls.get(), 12);
        assert!(!HELD.lock().unwrap().iter().any(|e| e.key == 0xCA11CE1));
    }

    #[test]
    fn huge_timelines_have_a_fixed_work_bound() {
        let count = Cell::new(0usize);
        let p = |_| {
            count.set(count.get() + 1);
            Vec3::ZERO
        };
        assert_eq!(held(&p, Tick::MAX, Tick::MIN, Tick::MAX, Tick(1), Tick(1), None, &|| None, &|| true, &|| false), None);
        assert!(count.get() <= MAX_PROBES + 1);
    }

    #[test]
    fn unversioned_reads_are_not_retained() {
        let valid = Cell::new(true);
        let p = |t: Tick| {
            valid.set(false);
            vec3(0.0, -t.seconds().clamp(0.0, 1.0), 0.0)
        };
        assert_eq!(
            held(
                &p,
                Tick::from_seconds_f64(2.0),
                Tick::ZERO,
                Tick::from_seconds_f64(3.0),
                Tick::from_units(1, 30),
                Tick::from_seconds_f64(0.005),
                Some(33),
                &|| None,
                &|| valid.get(),
                &|| false,
            ),
            Some(vec3(0.0, -1.0, 0.0))
        );
        assert!(!HELD.lock().unwrap().iter().any(|e| e.key == 33));
    }

    #[test]
    fn cache_does_not_extend_the_search_horizon() {
        let p = |t: Tick| vec3(-t.seconds().clamp(0.0, 1.0), 0.0, 0.0);
        let run = |time, key| {
            held(
                &p,
                Tick::from_seconds_f64(time),
                Tick::ZERO,
                Tick::from_seconds_f64(3600.0),
                Tick::from_units(1, 30),
                Tick::from_seconds_f64(0.005),
                Some(key),
                &|| None,
                &|| true,
                &|| false,
            )
        };
        assert_eq!(run(900.0, 44), Some(vec3(-1.0, 0.0, 0.0)));
        assert_eq!(run(3599.0, 44), None);
        assert_eq!(run(3599.0, 45), None);
    }
}
