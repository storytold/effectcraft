//! CPU-only docking geometry regressions: no panels, renderer, fonts, or GPU.

use effectcraft_ui_egui::{
    automation::Registry,
    dock::{self, DockNode, PanelKind, SplitSize},
    theme::{ThemeKind, Tokens},
};
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};

fn split(vertical: bool, size: SplitSize) -> DockNode {
    DockNode::Split {
        vertical,
        size,
        a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 }),
        b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Timeline], active: 0 }),
    }
}

fn draw(ctx: &egui::Context, node: &mut DockNode, rect: Rect, tokens: &Tokens, events: Vec<Event>) -> Rect {
    let mut groups = vec![];
    let mut registry = Registry::default();
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        dock::layout(ui, node, rect, tokens, "fixture", &mut groups, &mut registry);
    })
    .drop_without_applying_deltas();
    assert_eq!(groups.len(), 2);
    for group in groups {
        let r = group.rect;
        assert!([r.min.x, r.min.y, r.max.x, r.max.y].iter().all(|v| v.is_finite()));
        assert!(r.width() >= 0.0 && r.height() >= 0.0, "inverted group: {r:?}");
        assert!(rect.contains(r.min) && rect.contains(r.max), "group escaped parent: {r:?} vs {rect:?}");
    }
    let gutter = registry.find("dock.gutter.fixture").unwrap();
    Rect::from_min_size(pos2(gutter.rect[0], gutter.rect[1]), vec2(gutter.rect[2], gutter.rect[3]))
}

#[test]
fn compact_splits_keep_both_groups_inside_the_parent() {
    let tokens = Tokens::for_kind(ThemeKind::Dark);
    for vertical in [false, true] {
        for size in [SplitSize::Ratio(0.5), SplitSize::FixedA(100.0), SplitSize::FixedB(100.0)] {
            for extent in [tokens.gap + 30.0, tokens.gap + 10.0, tokens.gap * 0.5, 300.0] {
                let dimensions = if vertical { vec2(400.0, extent) } else { vec2(extent, 400.0) };
                let rect = Rect::from_min_size(pos2(20.0, 20.0), dimensions);
                draw(&egui::Context::default(), &mut split(vertical, size), rect, &tokens, vec![]);
            }
        }
    }
}

#[test]
fn dragging_a_compact_gutter_preserves_valid_saved_split_sizes() {
    let tokens = Tokens::for_kind(ThemeKind::Dark);
    for vertical in [false, true] {
        for size in [SplitSize::Ratio(0.5), SplitSize::FixedA(15.0), SplitSize::FixedB(15.0)] {
            let ctx = egui::Context::default();
            let mut node = split(vertical, size);
            let extent = tokens.gap + 30.0;
            let rect = Rect::from_min_size(pos2(20.0, 20.0), if vertical { vec2(400.0, extent) } else { vec2(extent, 400.0) });
            let start = draw(&ctx, &mut node, rect, &tokens, vec![]).center();
            draw(
                &ctx,
                &mut node,
                rect,
                &tokens,
                vec![
                    Event::PointerMoved(start),
                    Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
                ],
            );
            let end = start + if vertical { vec2(0.0, 60.0) } else { vec2(60.0, 0.0) };
            draw(&ctx, &mut node, rect, &tokens, vec![Event::PointerMoved(end)]);
            draw(
                &ctx,
                &mut node,
                rect,
                &tokens,
                vec![Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
            );
            let DockNode::Split { size, .. } = node else { unreachable!() };
            match size {
                SplitSize::Ratio(r) => assert!((0.0..=1.0).contains(&r), "invalid saved ratio: {r}"),
                SplitSize::FixedA(px) | SplitSize::FixedB(px) => assert!((0.0..=30.0).contains(&px), "invalid saved fixed size: {px}"),
            }
        }
    }
}
