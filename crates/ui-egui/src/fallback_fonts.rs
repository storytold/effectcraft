//! Installed fallback faces, read when the interface first paints text that needs them.
//!
//! The interface draws Japanese, Chinese, Arabic, Hebrew, Indic, Thai, Georgian and Hangul text
//! (layer, item and comment names, the Georgian and CJK catalogs) with installed faces
//! (`theme::install_script_fallbacks`). Those files are large (Apple SD Gothic Neo alone is 53 MB)
//! and most sessions never draw a character from most of them, so only the faces the interface
//! language needs are read when the fonts are installed. [`Watch`], an egui plugin, looks at the
//! text each pass painted: when a character is in the script of a face that hasn't been read, and
//! the fonts installed so far can't draw it (or draw it only with a face that comes later in the
//! fallback order), the face is read, the fonts are installed again and egui redoes the pass
//! before anything is shown. The first frame that shows the text already has its glyphs.
//!
//! A character in no fallback's script that nothing can draw (an arrow or a math sign only an
//! installed face has) is looked up in the faces not read yet, whose character maps are read once
//! without keeping the files, and the face that has it is read: so nothing that drew before draws
//! as a box now, and a character no face has (a newer emoji) reads nothing.
//!
//! egui itself asks [`Provider`], an egui [`egui::FontProvider`], for a font while it lays out a
//! character no installed font has, before it would draw a box: the same decision ([`plan`]) reads
//! the face there, so even the pass that first lays the text out draws it (and a test harness that
//! treats a missing glyph as a bug sees none). egui appends such a face to the family; [`Watch`]
//! then installs the fonts again, every face read in its place in the fallback order.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};

use egui::epaint::{ClippedShape, Shape};
use egui::{FontDefinitions, FontFamily, FontId};

/// What [`plan`] needs to know of a fallback face.
pub(crate) trait Candidate {
    /// Whether `c` is in the script the face is the fallback for.
    fn in_script(&self, c: char) -> bool;
    fn is_read(&self) -> bool;
    /// Whether the face, once read, draws `c`.
    fn covers(&self, c: char) -> bool;
    /// Read the face; false when there is nothing to read.
    fn read(&self) -> bool;
    /// Whether the face, read or not, draws `c`. A face not read yet is looked up in its character
    /// map, read once without keeping the file.
    fn might_cover(&self, c: char) -> bool;
}

/// How the fonts a pass was laid out with draw a character.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// A face ahead of the fallbacks draws it, or egui draws nothing for it.
    early: bool,
    /// Some face draws it.
    any: bool,
}

/// Read the fallbacks (in fallback order) the painted character `c` needs, given how the fonts
/// the pass was laid out with draw it, so that the same face draws `c` as when every fallback was
/// read at start-up. Returns whether a face was read.
///
/// - Nothing is read when a face ahead of the fallbacks draws `c`.
/// - Otherwise the first fallback that has `c` draws it: one already read, or one whose script has
///   `c`, which is read (in order, until one has `c`).
/// - A character no face draws reads the first fallback not read yet whose character map has it
///   (a symbol only an installed face has), and nothing when none has (a newer emoji). One that a
///   face after the fallbacks draws (a Chinese catalog's extra face) is left to it.
fn plan(c: char, drawn: Drawn, fallbacks: &[&dyn Candidate]) -> bool {
    if drawn.early {
        return false;
    }
    let mut read_any = false;
    for f in fallbacks {
        if f.is_read() {
            if f.covers(c) {
                return read_any;
            }
        } else if f.in_script(c) && f.read() {
            read_any = true;
            if f.covers(c) {
                return true;
            }
        }
    }
    if !drawn.any
        && let Some(f) = fallbacks.iter().find(|f| !f.is_read() && !f.in_script(c) && f.might_cover(c))
    {
        read_any |= f.read();
    }
    read_any
}

/// How `fonts` draws `c` in `family`, `has_glyph` being egui's answer. `late` names the faces that
/// come after the fallbacks' place in a family (the fallbacks themselves, and faces added after
/// them). `has_glyph` says whether some face draws `c`, not which one, so the faces' character maps
/// tell the faces ahead of the fallbacks from the others. (egui 0.36's `has_glyph` also answered no
/// for the characters of the face that supplies its replacement glyph; the maps cover that too.)
fn classify(fonts: &FontDefinitions, family: &FontFamily, c: char, has_glyph: bool, late: &[&str]) -> Drawn {
    let Some(stack) = fonts.families.get(family) else { return Drawn { early: true, any: true } };
    let maps = |late_faces: bool| {
        stack
            .iter()
            .filter(|name| late.contains(&name.as_str()) == late_faces)
            .filter_map(|name| fonts.font_data.get(name))
            .any(|data| effectcraft_text::fonts::maps_char(data.bytes(), data.index, c))
    };
    let (early, later) = (maps(false), maps(true));
    Drawn { early: early || (has_glyph && !later), any: has_glyph || early || later }
}

/// Characters that never need a fallback: ASCII, Latin-1 and Latin Extended-A (which every
/// interface font has), spacing and controls (egui draws nothing for them).
fn plain(c: char) -> bool {
    u32::from(c) < 0x180 || c.is_whitespace() || c.is_control()
}

/// The characters looked at so far, per font family (families differ in their faces): nothing more
/// to read for them. A bit per character of the Basic Multilingual Plane, a set for the others.
#[derive(Default)]
struct Settled(HashMap<FontFamily, (Vec<u64>, HashSet<char>)>);

impl Settled {
    fn insert(&mut self, family: FontFamily, c: char) {
        let (bits, rest) = self.0.entry(family).or_insert_with(|| (vec![0; 1024], HashSet::new()));
        let cp = u32::from(c);
        match bits.get_mut((cp / 64) as usize) {
            Some(word) => *word |= 1 << (cp % 64),
            None => {
                rest.insert(c);
            }
        }
    }

    #[cfg(test)]
    fn contains(&self, family: &FontFamily, c: char) -> bool {
        self.0.get(family).is_some_and(|entry| settled_in(entry, c))
    }
}

fn settled_in((bits, rest): &(Vec<u64>, HashSet<char>), c: char) -> bool {
    let cp = u32::from(c);
    match bits.get((cp / 64) as usize) {
        Some(word) => word & (1 << (cp % 64)) != 0,
        None => rest.contains(&c),
    }
}

/// The characters (with the font family drawing them) in `shapes` that are not `settled`, each
/// once. Bounded: a pass with more new characters leaves the rest to the next pass.
fn new_characters(shapes: &[ClippedShape], settled: &Settled) -> Vec<(char, FontFamily)> {
    const MAX: usize = 4096;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut stack: Vec<&Shape> = shapes.iter().map(|s| &s.shape).collect();
    while let Some(shape) = stack.pop() {
        match shape {
            Shape::Vec(v) => stack.extend(v.iter()),
            Shape::Text(t) => {
                let job = &t.galley.job;
                if job.text.is_ascii() {
                    continue;
                }
                for section in &job.sections {
                    let Some(text) = job.text.get(section.byte_range.start.0..section.byte_range.end.0) else { continue };
                    let family = &section.format.font_id.family;
                    let done = settled.0.get(family);
                    for c in text.chars().filter(|c| !plain(*c) && !done.is_some_and(|d| settled_in(d, *c))) {
                        if out.len() >= MAX {
                            return out;
                        }
                        if seen.insert((c, family.clone())) {
                            out.push((c, family.clone()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The egui font provider that reads, during layout, the installed fallback a character no
/// installed font has needs (see the module docs). [`Watch`] adds it.
struct Provider {
    /// The interface language, kept current by [`Watch::set_language`].
    lang: Arc<RwLock<String>>,
}

impl egui::FontProvider for Provider {
    /// egui asks only when no installed font has the character, so it is a character nothing
    /// draws: read what [`plan`] reads for it, and hand egui the first face read that has it.
    fn font_for(&self, request: &egui::FallbackRequest<'_>) -> Option<egui::FontInsert> {
        let c = request.cluster.chars().next()?;
        let lang = self.lang.read().unwrap_or_else(PoisonError::into_inner).clone();
        let fallbacks = crate::theme::lazy_fallbacks(&lang);
        let candidates: Vec<&dyn Candidate> = fallbacks.iter().map(|f| f as &dyn Candidate).collect();
        plan(c, Drawn { early: false, any: false }, &candidates);
        fallbacks.iter().find(|f| f.covers(c)).and_then(crate::theme::LazyFallback::insert)
    }
}

/// The egui plugin that reads the installed fallbacks painted text needs (see the module docs).
/// `theme::install` adds it and keeps its interface language current.
pub(crate) struct Watch {
    /// The interface language the fonts were installed for (it orders the CJK faces).
    lang: String,
    /// The same, for [`Provider`].
    shared_lang: Arc<RwLock<String>>,
    /// The faces read when painted text needs them, in fallback order, for the language named.
    fallbacks: (String, Vec<crate::theme::LazyFallback>),
    settled: Settled,
    /// The faces read before this context installed them (the faces read are process-wide, so
    /// another context can read one after these fonts were installed) that it installed since.
    caught_up: HashSet<&'static str>,
    /// When the last read started, to log how long the redone pass took.
    #[cfg(not(target_arch = "wasm32"))]
    started: Option<std::time::Instant>,
}

impl Watch {
    pub(crate) fn new(lang: &str) -> Self {
        Self {
            lang: lang.to_owned(),
            shared_lang: Arc::new(RwLock::new(lang.to_owned())),
            fallbacks: (lang.to_owned(), crate::theme::lazy_fallbacks(lang)),
            settled: Settled::default(),
            caught_up: HashSet::new(),
            #[cfg(not(target_arch = "wasm32"))]
            started: None,
        }
    }

    /// The interface language the fonts are installed for from now on.
    pub(crate) fn set_language(&mut self, lang: &str) {
        lang.clone_into(&mut self.lang);
        lang.clone_into(&mut self.shared_lang.write().unwrap_or_else(PoisonError::into_inner));
    }
}

/// Redo the pass the fonts were just installed again for before anything is shown. egui allows a
/// limited number of passes per frame: when this was the last, the next frame shows the result.
fn redo_pass(ctx: &egui::Context, output: &mut egui::FullOutput, reason: &'static str) {
    if output.platform_output.num_completed_passes < ctx.options(|o| o.max_passes.get()) {
        output.platform_output.request_discard_reasons.push(egui::RepaintCause::new_reason(reason));
    } else {
        ctx.request_repaint();
    }
}

impl egui::Plugin for Watch {
    fn debug_name(&self) -> &'static str {
        "effectcraft fallback fonts"
    }

    /// A pass a `Grid` or an area's sizing already had redone can still be redone once more for a
    /// face read in it, so its text doesn't show boxes for a frame.
    fn setup(&mut self, ctx: &egui::Context) {
        if let Some(three) = std::num::NonZeroUsize::new(3) {
            ctx.options_mut(|o| o.max_passes = o.max_passes.max(three));
        }
        ctx.add_font_provider(Arc::new(Provider { lang: self.shared_lang.clone() }));
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(started) = self.started.take() {
            log::info!("fonts: the pass redone with the new fallback face took {:.1} ms in all", started.elapsed().as_secs_f64() * 1e3);
        }
        if self.fallbacks.0 != self.lang {
            // another interface language: other faces, in another order
            self.fallbacks = (self.lang.clone(), crate::theme::lazy_fallbacks(&self.lang));
            self.settled = Settled::default();
        }
        if ctx.fonts_mut(|f| !f.discovered_fonts().is_empty()) {
            // [`Provider`] read a face during layout and egui appended it to the family: install the
            // fonts again, the face in its place, and look at this text with them
            crate::theme::install_fonts(ctx, &self.lang);
            redo_pass(ctx, output, "put an installed fallback face in its place");
            return;
        }
        let fallbacks = &self.fallbacks.1;
        let installed: HashSet<String> = ctx.fonts_mut(|f| f.definitions().font_data.keys().cloned().collect());
        // a face read (here or by another context) that this context hasn't installed and may need
        let behind = |f: &crate::theme::LazyFallback| f.is_read() && !installed.contains(f.name()) && !self.caught_up.contains(f.name());
        if fallbacks.iter().all(|f| f.is_read()) && !fallbacks.iter().any(behind) {
            return;
        }
        let chars = new_characters(&output.shapes, &self.settled);
        if chars.is_empty() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        let candidates: Vec<&dyn Candidate> = fallbacks.iter().map(|f| f as &dyn Candidate).collect();
        let mut read = false;
        let mut catch_up = Vec::new();
        let late: Vec<&str> = fallbacks.iter().map(|f| f.name()).collect();
        for (c, family) in chars {
            // the fonts this pass was laid out with (new ones apply from the next pass)
            let drawn = ctx.fonts_mut(|f| {
                if !f.definitions().families.contains_key(&family) {
                    return Drawn { early: true, any: true }; // (painted text always has a family)
                }
                let has_glyph = f.has_glyph(&FontId::new(12.0, family.clone()), c);
                classify(f.definitions(), &family, c, has_glyph, &late)
            });
            read |= plan(c, drawn, &candidates);
            // the face that draws `c` was read by another context after this one installed its fonts
            if let Some(f) = fallbacks.iter().find(|f| f.is_read() && f.covers(c)).filter(|f| behind(f)) {
                catch_up.push(f.name());
            }
            self.settled.insert(family, c);
        }
        if !read && catch_up.is_empty() {
            return;
        }
        // once each: a face installed under another name (two scripts sharing one face) stays absent
        self.caught_up.extend(catch_up);
        crate::theme::install_fonts(ctx, &self.lang);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let names: Vec<&str> = fallbacks.iter().filter(|f| f.is_read()).map(|f| f.name()).collect();
            log::info!("fonts: read installed fallback faces for {names:?} in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
            self.started = Some(started);
        }
        redo_pass(ctx, output, "read an installed fallback face");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ops::RangeInclusive;

    /// A fallback for [`plan`]: its script, what it covers, whether it is installed, and how often
    /// it was read.
    struct Fake {
        script: RangeInclusive<char>,
        covers: &'static str,
        installed: bool,
        read: Cell<bool>,
        reads: Cell<u32>,
    }

    impl Fake {
        fn new(script: RangeInclusive<char>, covers: &'static str) -> Self {
            Self { script, covers, installed: true, read: Cell::new(false), reads: Cell::new(0) }
        }
        fn read_already(self) -> Self {
            self.read.set(true);
            self
        }
    }

    impl Candidate for Fake {
        fn in_script(&self, c: char) -> bool {
            self.script.contains(&c)
        }
        fn is_read(&self) -> bool {
            self.read.get()
        }
        fn covers(&self, c: char) -> bool {
            self.read.get() && self.covers.contains(c)
        }
        fn read(&self) -> bool {
            if !self.installed {
                return false;
            }
            if !self.read.get() {
                self.reads.set(self.reads.get() + 1);
            }
            self.read.set(true);
            true
        }
        fn might_cover(&self, c: char) -> bool {
            self.installed && self.covers.contains(c)
        }
    }

    const HANGUL: RangeInclusive<char> = '\u{ac00}'..='\u{d7a3}';
    const HAN: RangeInclusive<char> = '\u{4e00}'..='\u{9fff}';
    const TAMIL: RangeInclusive<char> = '\u{b80}'..='\u{bff}';
    const DEVANAGARI: RangeInclusive<char> = '\u{900}'..='\u{97f}';

    /// Drawn by a face ahead of the fallbacks; by a fallback (or a face after them) only; by none.
    const EARLY: Drawn = Drawn { early: true, any: true };
    const LATE: Drawn = Drawn { early: false, any: true };
    const NONE: Drawn = Drawn { early: false, any: false };

    #[test]
    fn text_the_fonts_draw_reads_nothing() {
        let korean = Fake::new(HANGUL, "한국");
        assert!(!plan('Ж', EARLY, &[&korean]));
        // Hangul the craft-fonts' Chinese face already draws, ahead of the Korean fallback
        assert!(!plan('한', EARLY, &[&korean]));
        assert_eq!(korean.reads.get(), 0);
    }

    #[test]
    fn text_nothing_draws_reads_its_scripts_face() {
        let hant = Fake::new(HAN, "中文");
        let korean = Fake::new(HANGUL, "한국");
        assert!(plan('한', NONE, &[&hant, &korean]));
        assert_eq!((hant.reads.get(), korean.reads.get()), (0, 1), "only the Korean face");
        assert!(!plan('국', NONE, &[&hant, &korean]), "the face read draws the rest of its script");
    }

    /// One face for several scripts (Nirmala UI): read for Tamil, it draws Devanagari too, so the
    /// Devanagari face isn't read.
    #[test]
    fn a_face_read_for_one_script_serves_the_next() {
        let tamil = Fake::new(TAMIL, "தக");
        let devanagari = Fake::new(DEVANAGARI, "क");
        assert!(plan('த', NONE, &[&tamil, &devanagari]));
        let tamil = Fake { covers: "தகक", ..tamil };
        assert!(!plan('क', LATE, &[&tamil, &devanagari]));
        assert_eq!(devanagari.reads.get(), 0);
    }

    #[test]
    fn faces_for_a_script_are_read_in_order_until_one_covers_the_character() {
        let sparse = Fake::new(HAN, "中");
        let full = Fake::new(HAN, "中文");
        let after = Fake::new(HAN, "中文");
        assert!(plan('文', NONE, &[&sparse, &full, &after]));
        assert_eq!((sparse.reads.get(), full.reads.get(), after.reads.get()), (1, 1, 0));
    }

    /// The fallback order decides which face draws a character: when only a face that comes later
    /// draws it, the earlier one is read so it draws it, as it did when every face was read at
    /// start-up.
    #[test]
    fn an_earlier_face_is_read_when_a_later_one_draws_its_script() {
        let hant = Fake::new(HAN, "中文");
        let korean = Fake::new(HANGUL, "한국中").read_already();
        assert!(plan('中', LATE, &[&hant, &korean]));
        assert_eq!(hant.reads.get(), 1);
        let hant = Fake::new(HAN, "中文").read_already();
        let korean = Fake::new(HANGUL, "한국中");
        assert!(!plan('中', LATE, &[&hant, &korean]));
        assert_eq!(korean.reads.get(), 0);
    }

    /// A character in no fallback's script that nothing draws reads the face whose character map
    /// has it (a symbol only an installed face has), and nothing when no face has it (a newer
    /// emoji, Ethiopic, a math sign nobody has).
    #[test]
    fn a_character_in_no_script_reads_only_the_face_that_has_it() {
        let hant = Fake::new(HAN, "中");
        let korean = Fake::new(HANGUL, "한⇢");
        let tamil = Fake::new(TAMIL, "த");
        assert!(plan('⇢', NONE, &[&hant, &korean, &tamil]));
        assert_eq!((hant.reads.get(), korean.reads.get(), tamil.reads.get()), (0, 1, 0), "only the face that has it");
        for c in ['\u{1fae0}', '\u{1200}', '\u{2a00}', '\u{f8ff}'] {
            assert!(!plan(c, NONE, &[&hant, &korean, &tamil]), "{c:?}: no face has it");
        }
        assert_eq!((hant.reads.get(), korean.reads.get(), tamil.reads.get()), (0, 1, 0));
    }

    #[test]
    fn a_face_that_is_not_installed_is_skipped() {
        let mut missing = Fake::new(HANGUL, "한");
        missing.installed = false;
        let other = Fake::new(HAN, "中");
        assert!(!plan('한', NONE, &[&missing]));
        assert!(plan('中', NONE, &[&missing, &other]));
        assert_eq!(other.reads.get(), 1);
    }

    /// Characters are settled per font family: one drawn in a family whose faces have it may still
    /// need a fallback in another (a ★ Noto Emoji draws in the proportional family, but not in a
    /// named family without Noto Emoji).
    #[test]
    fn characters_are_settled_per_family() {
        let semibold = FontFamily::Name("semibold".into());
        let mut settled = Settled::default();
        settled.insert(FontFamily::Proportional, '★');
        settled.insert(FontFamily::Proportional, '\u{1fae0}');
        assert!(settled.contains(&FontFamily::Proportional, '★'));
        assert!(settled.contains(&FontFamily::Proportional, '\u{1fae0}'));
        assert!(!settled.contains(&semibold, '★'));
        assert!(!settled.contains(&FontFamily::Proportional, '☆'));
    }

    #[test]
    fn only_new_characters_are_collected() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let font = FontId::proportional(12.0);
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap("Hello Ǆ ɕ 한국 한 ñ".into(), font.clone(), egui::Color32::WHITE));
        let text = Shape::galley(egui::Pos2::ZERO, galley, egui::Color32::WHITE);
        let shapes = vec![ClippedShape::new(egui::Rect::EVERYTHING, Shape::Vec(vec![text]))];
        let chars: Vec<char> = new_characters(&shapes, &Settled::default()).into_iter().map(|(c, _)| c).collect();
        assert_eq!(chars, ['Ǆ', 'ɕ', '한', '국'], "each new character once; ASCII, Latin-1 and spaces skipped");
        let mut settled = Settled::default();
        settled.insert(FontFamily::Proportional, '한');
        let chars: Vec<char> = new_characters(&shapes, &settled).into_iter().map(|(c, _)| c).collect();
        assert_eq!(chars, ['Ǆ', 'ɕ', '국']);
    }

    /// The character maps decide which faces draw a character, even when `has_glyph` answers no
    /// (as egui 0.36's did for Noto Emoji, the face of its replacement glyph): an emoji counts as
    /// drawn, by a face ahead of the fallbacks unless only fallbacks draw it.
    #[test]
    fn classify_finds_the_face_that_draws_a_character() {
        let fonts = FontDefinitions::default();
        let p = FontFamily::Proportional;
        assert_eq!(classify(&fonts, &p, '\u{1f3ac}', false, &[]), Drawn { early: true, any: true }, "🎬 is in Noto Emoji");
        let all_late = ["Ubuntu-Light", "egui-icons", "NotoEmoji-Regular", "emoji-icon-font"];
        assert_eq!(classify(&fonts, &p, '\u{1f3ac}', false, &all_late), Drawn { early: false, any: true }, "drawn, but only by late faces");
        assert_eq!(classify(&fonts, &p, '한', false, &[]), Drawn { early: false, any: false });
        assert_eq!(classify(&fonts, &p, '\u{200d}', true, &[]), Drawn { early: true, any: true }, "egui draws nothing for a joiner");
        assert_eq!(classify(&fonts, &FontFamily::Name("none".into()), 'A', false, &[]), Drawn { early: true, any: true });
    }
}
