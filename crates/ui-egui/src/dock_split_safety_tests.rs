//! Synthetic CPU-only geometry and pointer fixtures; no renderer or adapter.
use super::*;
use egui::{Context, Event, Modifiers, PointerButton, RawInput};

fn pair(vertical: bool, size: SplitSize) -> DockNode {
    DockNode::Split { vertical, size, a: Box::new(tabs(&[PanelKind::Project], 0)), b: Box::new(tabs(&[PanelKind::Composition], 0)) }
}
fn bounds(vertical: bool, extent: f32) -> Rect {
    Rect::from_min_size(pos2(20.0, 30.0), if vertical { vec2(96.0, extent) } else { vec2(extent, 96.0) })
}
fn frame(ctx: &Context, node: &mut DockNode, rect: Rect, tokens: &Tokens, registry: &mut crate::automation::Registry, events: Vec<Event>) -> Vec<Group> {
    let mut groups = Vec::new();
    registry.begin_frame();
    ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
        layout(ui, node, rect, tokens, "split-safety", &mut groups, registry);
    })
    .drop_without_applying_deltas();
    groups
}
fn contained(parent: Rect, child: Rect) {
    assert!([child.min.x, child.min.y, child.max.x, child.max.y, child.width(), child.height()].into_iter().all(f32::is_finite), "nonfinite {child:?}");
    assert!(child.width() >= 0.0 && child.height() >= 0.0, "inverted {child:?}");
    assert!(parent.contains_rect(child), "{child:?} escapes {parent:?}");
}
fn check_groups(parent: Rect, groups: &[Group], registry: &crate::automation::Registry) {
    for group in groups {
        contained(parent, group.rect);
        contained(group.rect, group.content);
    }
    for element in &registry.elements {
        let [x, y, width, height] = element.rect;
        contained(parent, Rect::from_min_size(pos2(x, y), vec2(width, height)));
    }
}

#[test]
fn split_all_modes_and_axes_fit_zero_tiny_and_normal_parents() {
    let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    for vertical in [false, true] {
        for size in [SplitSize::Ratio(0.25), SplitSize::FixedA(30.0), SplitSize::FixedB(30.0)] {
            // Start at 30 available points: the old clamp receives min=20, max=10 and panics.
            for available in [30.0, 0.0, 1.0, 10.0, 19.0, 20.0, 39.0, 40.0, 80.0, 200.0] {
                let mut node = pair(vertical, size);
                let before = node.clone();
                let rect = bounds(vertical, tokens.gap + available);
                let mut registry = crate::automation::Registry::default();
                let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
                assert_eq!(groups.len(), 2);
                check_groups(rect, &groups, &registry);
                let extent = |rect: Rect| {
                    if vertical { rect.height() } else { rect.width() }
                };
                let first = extent(groups[0].rect);
                let second = extent(groups[1].rect);
                assert!((first + second - available).abs() < 0.001);
                if available < 40.0 {
                    assert!((first - available * 0.5).abs() < 0.001);
                    assert!((second - first).abs() < 0.001);
                } else {
                    assert!(first >= 20.0 && second >= 20.0);
                }
                if available == 200.0 {
                    let expected = match size {
                        SplitSize::Ratio(_) => 50.0,
                        SplitSize::FixedA(_) => 30.0,
                        SplitSize::FixedB(_) => 170.0,
                    };
                    assert_eq!(first, expected, "normal authored sizing must be preserved");
                }
                assert_eq!(node, before, "painting must not rewrite saved sizes");
            }
            for total in [0.0, tokens.gap * 0.5] {
                let mut node = pair(vertical, size);
                let rect = bounds(vertical, total);
                let mut registry = crate::automation::Registry::default();
                let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
                assert_eq!(groups.len(), 2);
                check_groups(rect, &groups, &registry);
            }
        }
    }
}

#[test]
fn nested_workspaces_keep_every_emitted_group_and_content_inside_tiny_bounds() {
    let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    for name in ["Standard", "Default"] {
        for extent in [0.0, 1.0, 10.0, 32.0, 64.0, 90.0, 900.0] {
            let mut node = workspace(name);
            let before = node.clone();
            let rect = Rect::from_min_size(pos2(20.0, 30.0), vec2(extent, extent));
            let mut registry = crate::automation::Registry::default();
            let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
            assert!(!groups.is_empty());
            check_groups(rect, &groups, &registry);
            if name == "Standard" {
                assert_eq!(groups.len(), 5, "all five tab groups remain represented");
            }
            assert_eq!(node, before);
        }
    }
}

#[test]
fn nonfinite_split_preferences_and_chrome_never_emit_nonfinite_rectangles() {
    for vertical in [false, true] {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, -f32::MAX] {
            for size in [SplitSize::Ratio(value), SplitSize::FixedA(value), SplitSize::FixedB(value)] {
                for gap in [3.0, f32::NAN, f32::INFINITY, -3.0] {
                    let mut tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
                    tokens.gap = gap;
                    tokens.tab_h = f32::NAN;
                    let mut node = pair(vertical, size);
                    let rect = bounds(vertical, 100.0);
                    let mut registry = crate::automation::Registry::default();
                    let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
                    assert_eq!(groups.len(), 2);
                    check_groups(rect, &groups, &registry);
                }
            }
        }
    }
    let mut node =
        stack(&[(PanelKind::Preview, true, Some(f32::NAN)), (PanelKind::Properties, true, Some(f32::INFINITY)), (PanelKind::Audio, true, Some(-20.0))]);
    let mut tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    tokens.tab_h = f32::INFINITY;
    let rect = bounds(false, 100.0);
    let mut registry = crate::automation::Registry::default();
    let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
    assert_eq!(groups.len(), 1, "entries after the exhausted height are omitted");
    check_groups(rect, &groups, &registry);
}

#[test]
fn actual_gutter_drag_preserves_finite_saved_sizes_at_small_and_zero_extents() {
    let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    for vertical in [false, true] {
        for size in [SplitSize::Ratio(0.25), SplitSize::FixedA(30.0), SplitSize::FixedB(30.0)] {
            for available in [0.0, 0.5, 30.0, 100.0] {
                let ctx = Context::default();
                let mut node = pair(vertical, size);
                let rect = bounds(vertical, tokens.gap + available);
                let mut registry = crate::automation::Registry::default();
                frame(&ctx, &mut node, rect, &tokens, &mut registry, vec![]);
                let element = registry.elements.iter().find(|e| e.id == "dock.gutter.split-safety").unwrap();
                let [x, y, width, height] = element.rect;
                let start = pos2(x + width * 0.5, y + height * 0.5);
                let end = start + if vertical { vec2(0.0, 80.0) } else { vec2(80.0, 0.0) };
                frame(
                    &ctx,
                    &mut node,
                    rect,
                    &tokens,
                    &mut registry,
                    vec![
                        Event::PointerMoved(start),
                        Event::PointerButton { pos: start, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
                    ],
                );
                frame(&ctx, &mut node, rect, &tokens, &mut registry, vec![Event::PointerMoved(end)]);
                let groups = frame(
                    &ctx,
                    &mut node,
                    rect,
                    &tokens,
                    &mut registry,
                    vec![Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
                );
                check_groups(rect, &groups, &registry);
                let DockNode::Split { size: saved, .. } = &node else { panic!("split expected") };
                if available == 0.0 {
                    assert_eq!(*saved, size, "a zero-available drag cannot save an invalid ratio");
                } else {
                    let expected_first = if available < 80.0 { available * 0.5 } else { 60.0 };
                    let expected = match size {
                        SplitSize::Ratio(_) => expected_first / available,
                        SplitSize::FixedA(_) => expected_first,
                        SplitSize::FixedB(_) => available - expected_first,
                    };
                    let actual = match saved {
                        SplitSize::Ratio(x) | SplitSize::FixedA(x) | SplitSize::FixedB(x) => *x,
                    };
                    assert!(actual.is_finite() && (actual - expected).abs() < 0.001, "{size:?}, available={available}, saved={saved:?}");
                }
            }
        }
    }
}

#[test]
fn invalid_parent_rectangles_are_skipped_without_mutating_the_tree() {
    let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    for rect in [
        Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(10.0, 10.0)),
        Rect::from_min_max(pos2(10.0, 10.0), pos2(0.0, 0.0)),
        Rect::from_min_max(pos2(-f32::MAX, 0.0), pos2(f32::MAX, 10.0)),
    ] {
        let mut node = pair(false, SplitSize::Ratio(0.5));
        let before = node.clone();
        let mut registry = crate::automation::Registry::default();
        let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
        assert!(groups.is_empty() && registry.elements.is_empty());
        assert_eq!(node, before);
    }
}

#[test]
fn overdeep_layout_branches_are_omitted_without_rewriting_the_workspace() {
    let tokens = Tokens::for_kind(crate::theme::ThemeKind::Dark);
    let mut node = tabs(&[PanelKind::Project], 0);
    // The display traversal has a documented 128-level bound. Keep the fixture
    // independent of the production constant so it also compiles before the fix.
    const DOCUMENTED_LAYOUT_DEPTH: usize = 128;
    for _ in 0..DOCUMENTED_LAYOUT_DEPTH + 3 {
        node = hsplit(SplitSize::Ratio(0.5), node, tabs(&[PanelKind::Composition], 0));
    }
    let before = node.clone();
    let rect = bounds(false, 300.0);
    let mut registry = crate::automation::Registry::default();
    let groups = frame(&Context::default(), &mut node, rect, &tokens, &mut registry, vec![]);
    assert!(!groups.is_empty() && groups.len() < DOCUMENTED_LAYOUT_DEPTH);
    check_groups(rect, &groups, &registry);
    assert_eq!(node, before);
}
