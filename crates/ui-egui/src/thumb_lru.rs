//! Bounded thumbnail caches kept in egui's temp data (Media Browser, Project panel): past a
//! cap, the least recently used thumbnail is dropped (its texture freed), so browsing many
//! folders or selecting many items doesn't keep every texture for the rest of the session.

use std::collections::VecDeque;

/// Note that the temp entry `id` (of type `T`) of the cache `list` was used this frame; past
/// `cap` entries, the least recently used ones are removed from `ctx`'s data.
pub(crate) fn touch<T: 'static + Clone + Send + Sync>(ctx: &egui::Context, list: &'static str, id: egui::Id, cap: usize) {
    let order_id = egui::Id::new(("thumb-lru", list));
    ctx.data_mut(|d| {
        let order = d.get_temp_mut_or_default::<VecDeque<egui::Id>>(order_id);
        // The newest at the back; a thumbnail shown again moves there.
        if order.back() != Some(&id) {
            order.retain(|i| *i != id);
            order.push_back(id);
        }
        let mut gone = Vec::new();
        while order.len() > cap.max(1) {
            if let Some(old) = order.pop_front() {
                gone.push(old);
            }
        }
        for old in gone {
            d.remove::<T>(old);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::touch;

    #[test]
    fn the_least_recently_used_thumbnail_goes_past_the_cap() {
        let ctx = egui::Context::default();
        let id = |i: u32| egui::Id::new(("t", i));
        let put = |i: u32| {
            ctx.data_mut(|d| d.insert_temp(id(i), i));
            touch::<u32>(&ctx, "test", id(i), 2);
        };
        let has = |i: u32| ctx.data(|d| d.get_temp::<u32>(id(i))).is_some();
        put(1);
        put(2);
        // 1 is used again, so 2 is now the oldest.
        touch::<u32>(&ctx, "test", id(1), 2);
        put(3);
        assert!(has(1) && has(3) && !has(2));
        // Another list's entries are kept apart.
        ctx.data_mut(|d| d.insert_temp(id(9), 9u32));
        touch::<u32>(&ctx, "other", id(9), 1);
        assert!(has(1) && has(3) && has(9));
    }
}
