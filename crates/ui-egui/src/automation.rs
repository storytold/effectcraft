//! Automation registry: every interactive element registers a stable id and its screen rect each
//! frame, so agents can click/drag by id (`ui.click {id:"timeline.track.V1.lock"}`) and
//! `ui.inspect` can report the full on-screen widget tree.

use egui::Rect;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Element {
    pub id: String,
    pub label: String,
    pub rect: [f32; 4],
    #[serde(skip)]
    pub(crate) response_id: Option<egui::Id>,
}

#[derive(Default)]
pub struct Registry {
    pub elements: Vec<Element>,
    /// Last frame's elements (for lookups from control handlers between frames).
    pub previous: Vec<Element>,
    /// Elements from the frame before last, reversed, whose buffers `add` reuses.
    spare: Vec<Element>,
}

impl Registry {
    pub fn add(&mut self, id: &str, rect: Rect, label: &str) {
        let mut element = self.spare.pop().unwrap_or_else(|| Element { id: String::new(), label: String::new(), rect: [0.0; 4], response_id: None });
        element.id.clear();
        element.id.push_str(id);
        element.label.clear();
        element.label.push_str(label);
        element.rect = [rect.min.x, rect.min.y, rect.width(), rect.height()];
        element.response_id = None;
        self.elements.push(element);
    }
    /// Bind an automation name to the widget that actually receives keyboard input.
    pub fn add_response(&mut self, id: &str, response: &egui::Response, label: &str) {
        self.add(id, response.rect, label);
        if let Some(element) = self.elements.last_mut() {
            element.response_id = Some(response.id);
        }
    }
    pub fn begin_frame(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.elements);
        self.spare.clear();
        self.spare.extend(self.elements.drain(..).rev());
    }
    pub fn find(&self, id: &str) -> Option<&Element> {
        self.previous.iter().chain(self.elements.iter()).find(|e| e.id == id)
    }
    /// Elements whose id starts with a prefix.
    pub fn query(&self, prefix: &str) -> Vec<&Element> {
        self.previous.iter().filter(|e| e.id.starts_with(prefix)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Snap = (String, String, [f32; 4], Option<egui::Id>);

    fn r(x: f32) -> Rect {
        Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(10.0, 5.0))
    }

    fn snap(elements: &[Element]) -> Vec<Snap> {
        elements.iter().map(|e| (e.id.clone(), e.label.clone(), e.rect, e.response_id)).collect()
    }

    fn e(id: &str, label: &str, x: f32) -> Snap {
        (id.to_owned(), label.to_owned(), [x, 0.0, 10.0, 5.0], None)
    }

    fn add_panel_and_layer(reg: &mut Registry) {
        reg.add("panel.Project", r(1.0), "Project");
        reg.add("timeline.layer.1", r(2.0), "Timeline");
    }

    #[test]
    fn registry_contents_match_across_frames() {
        let mut reg = Registry::default();

        // Frame 1: three widgets, the middle one bound to a response id.
        reg.begin_frame();
        reg.add("a", r(1.0), "A");
        reg.add("b", r(2.0), "B");
        reg.add("c", r(3.0), "C");
        if let Some(element) = reg.elements.get_mut(1) {
            element.response_id = Some(egui::Id::unique("x"));
        }
        let mut f1 = vec![e("a", "A", 1.0), e("b", "B", 2.0), e("c", "C", 3.0)];
        if let Some(entry) = f1.get_mut(1) {
            entry.3 = Some(egui::Id::unique("x"));
        }
        assert_eq!(snap(&reg.elements), f1);
        assert_eq!(snap(&reg.previous), Vec::<Snap>::new());

        // Frame 2: a changed label and a new widget.
        reg.begin_frame();
        reg.add("a", r(1.0), "A");
        reg.add("b", r(2.0), "B2");
        reg.add("c", r(3.0), "C");
        reg.add("d", r(4.0), "D");
        let f2 = vec![e("a", "A", 1.0), e("b", "B2", 2.0), e("c", "C", 3.0), e("d", "D", 4.0)];
        assert_eq!(snap(&reg.elements), f2);
        assert_eq!(snap(&reg.previous), f1);

        // Frame 3: fewer widgets, reordered. `a` reuses frame 1's response-bound slot.
        reg.begin_frame();
        reg.add("d", r(4.0), "D");
        reg.add("a", r(1.0), "A");
        let f3 = vec![e("d", "D", 4.0), e("a", "A", 1.0)];
        assert_eq!(snap(&reg.elements), f3);
        assert_eq!(snap(&reg.previous), f2);
        assert_eq!(reg.find("b").map(|element| element.label.as_str()), Some("B2"));
        assert!(reg.find("zz").is_none());
        assert_eq!(reg.query("a").len(), 1);

        // Frame 4: no widgets at all.
        reg.begin_frame();
        assert_eq!(snap(&reg.elements), Vec::<Snap>::new());
        assert_eq!(snap(&reg.previous), f3);

        // Frame 5: five widgets again, one with a much longer label.
        reg.begin_frame();
        let long = "x".repeat(100);
        reg.add("a", r(1.0), &long);
        reg.add("b", r(2.0), "B");
        reg.add("c", r(3.0), "C");
        reg.add("d", r(4.0), "D");
        reg.add("e", r(5.0), "E");
        let f5 = vec![e("a", &long, 1.0), e("b", "B", 2.0), e("c", "C", 3.0), e("d", "D", 4.0), e("e", "E", 5.0)];
        assert_eq!(snap(&reg.elements), f5);
        assert_eq!(snap(&reg.previous), Vec::<Snap>::new());
    }

    #[test]
    fn steady_frames_reuse_buffers() {
        let mut reg = Registry::default();

        reg.begin_frame();
        add_panel_and_layer(&mut reg);
        assert_eq!(reg.elements.len(), 2);
        let elements_ptr = reg.elements.as_ptr();
        let id_ptrs: Vec<_> = reg.elements.iter().map(|element| element.id.as_ptr()).collect();
        let label_ptrs: Vec<_> = reg.elements.iter().map(|element| element.label.as_ptr()).collect();

        for _ in 0..2 {
            reg.begin_frame();
            add_panel_and_layer(&mut reg);
        }

        assert_eq!(reg.elements.as_ptr(), elements_ptr);
        assert_eq!(reg.elements.iter().map(|element| element.id.as_ptr()).collect::<Vec<_>>(), id_ptrs);
        assert_eq!(reg.elements.iter().map(|element| element.label.as_ptr()).collect::<Vec<_>>(), label_ptrs);
        assert_eq!(
            reg.elements.iter().map(|element| (element.id.as_str(), element.label.as_str())).collect::<Vec<_>>(),
            vec![("panel.Project", "Project"), ("timeline.layer.1", "Timeline")]
        );
    }
}
