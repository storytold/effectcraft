use super::*;

/// Concurrent language requests must neither choose another context's font nor reuse its
/// cached family order. Synthetic fonts in a child process keep this independent of the host
/// font installation and of other tests that exercise the process-global database/locale.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn cjk_requests_keep_their_language_and_cache_entries() {
    const CHILD: &str = "EFFECTCRAFT_TEST_ISOLATED_CJK";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fonts::tests::cjk_requests_keep_their_language_and_cache_entries", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let families = [("ja", "Yu Gothic"), ("zh-hans", "Microsoft YaHei"), ("zh-hant", "Microsoft JhengHei"), ("ko", "Malgun Gothic")];
    let mut expected = Vec::new();
    {
        let mut d = db().write().unwrap();
        d.scanned = true; // This test process uses only bundled and synthetic fonts.
        for (language, family) in families {
            let mut data = test_font(family);
            let table = data[12..12 + 8 * 16].as_chunks::<16>().0.iter().find(|t| &t[..4] == b"cmap").unwrap();
            let cmap = u32::from_be_bytes(table[8..12].try_into().unwrap()) as usize + 12;
            // Map 页 to the fixture's triangle glyph instead of its private-use range.
            // All four fonts cover the probe, so coverage cannot choose the right language.
            for (offset, value) in [(16, '页' as u16), (24, '页' as u16), (30, 2u16.wrapping_sub('页' as u16))] {
                data[cmap + offset..cmap + offset + 2].copy_from_slice(&value.to_be_bytes());
            }
            let data = Arc::new(data);
            let names = read_faces_bytes(&data).into_iter().next().unwrap();
            let id = push(&mut d, info_from(names, FaceData::Shared(data), "test"));
            expected.push((language, id));
        }
    }
    let base = resolve("Inter", "Regular").face;
    for &(_, id) in &expected {
        assert!(face(id).has_char('页'));
    }
    // Force the original interleaving: Chinese starts, then Japanese replaces the global
    // language before Chinese looks up its probe. The explicit request must still win.
    set_cjk_locale("zh-hans");
    set_cjk_locale("ja");
    assert_eq!(fallback_for('页', base), expected[0].1);
    assert_eq!(fallback_for_locale('页', base, "zh-hans"), expected[1].1);
    // Do not clear the cache here: different policies must coexist in the same cache.
    for _ in 0..3 {
        for &(language, id) in &expected {
            assert_eq!(fallback_for_locale('页', base, language), id, "{language}");
        }
    }
    let barrier = Arc::new(std::sync::Barrier::new(expected.len()));
    let threads: Vec<_> = expected
        .into_iter()
        .map(|(language, id)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..128 {
                    set_cjk_locale(if language == "ja" { "zh-hans" } else { "ja" });
                    assert_eq!(fallback_for_locale('页', base, language), id, "{language}");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn extra_font_dirs_come_from_a_path_list() {
    assert!(extra_dirs(None).is_empty());
    let v = std::env::join_paths(["/a/fonts", "/b/more"]).unwrap_or_default();
    assert_eq!(extra_dirs(Some(&v)), vec![PathBuf::from("/a/fonts"), PathBuf::from("/b/more")]);
    assert!(extra_dirs(Some(std::ffi::OsStr::new(""))).is_empty());
}

#[test]
fn bundled_families_and_resolution() {
    let fams = families();
    let inter = fams.iter().find(|(f, _)| f == "Inter").expect("Inter");
    for s in ["Regular", "Medium", "SemiBold", "Bold", "Italic"] {
        assert!(inter.1.iter().any(|x| x == s), "{s} in {:?}", inter.1);
    }
    assert!(fams.iter().any(|(f, _)| f == "Noto Serif"));
    assert!(fams.iter().any(|(f, _)| f == "JetBrains Mono"));
    let b = resolve("inter", "bold");
    assert_eq!(face(b.face).info.style, "Bold");
    assert!(!b.synth_bold);
    let faces = all_faces();
    assert!(faces.iter().any(|f| f.info.family == "Noto Serif" && f.info.style == "Regular" && f.info.origin == "bundled"));
    // A real installed Bold Italic takes priority. Partial installed families can instead
    // supply the nearest weight/slant, so neither synthesis flag is fixed by the OS.
    for family in ["Noto Serif", "Inter"] {
        let resolved = resolve(family, "Bold Italic");
        let selected = face(resolved.face);
        assert!(selected.info.family.eq_ignore_ascii_case(family));
        assert!(!resolved.missing);
        let candidates: Vec<_> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
        if let Some(exact) = candidates.iter().find(|f| f.info.style.eq_ignore_ascii_case("Bold Italic")) {
            assert_eq!(resolved.face, exact.id, "real style wins for {family}");
            assert!(!resolved.synth_bold && !resolved.synth_italic);
        } else {
            // Italic mismatch dominates any supported font weight distance.
            let distance = |f: &Face| u32::from(f.info.weight.abs_diff(700)) + if f.info.italic { 0 } else { 1_000 };
            let nearest = candidates.iter().map(|f| distance(f)).min().unwrap();
            assert_eq!(distance(&selected), nearest, "closest available style for {family}");
            assert_eq!(resolved.synth_bold, selected.info.weight <= 500);
            assert_eq!(resolved.synth_italic, !selected.info.italic);
        }
    }
}

#[test]
fn missing_family_falls_back_to_inter() {
    let r = resolve("No Such Font Family 123", "Regular");
    assert_eq!(face(r.face).info.family, "Inter");
    assert!(r.missing);
}

/// Native: the system scan reads name tables only and resolves families installed there.
#[test]
fn system_scan_finds_fonts() {
    let t = std::time::Instant::now();
    scan_system(); // (another test may have triggered the scan already)
    let n = all_faces().iter().filter(|f| f.info.origin == "system").count();
    eprintln!("system fonts: {n} faces in {:?}", t.elapsed());
    assert_eq!(scan_system(), 0, "scans once");
    if cfg!(target_os = "macos") {
        assert!(n > 20, "{n}");
        let r = resolve("Helvetica", "Bold");
        assert_eq!(face(r.face).info.family, "Helvetica");
        let l = crate::layout_text("Helvetica", &crate::TextStyle { family: "Helvetica".into(), size: 30.0, ..Default::default() }, &Default::default());
        assert!(!l.missing_font && l.glyphs.iter().all(|g| g.id != 0));
    }
}

#[test]
fn postscript_and_full_names_resolve() {
    // Bundled Inter: name id 6 is `Inter-Bold`, name id 4 is `Inter Bold`.
    let ps = resolve("Inter-Bold", "");
    assert_eq!(face(ps.face).info.style, "Bold");
    assert!(!ps.missing && !ps.synth_bold);
    let full = resolve("inter semibold", "");
    assert_eq!(face(full.face).info.style, "SemiBold");
    assert!(!full.missing);
    // A family name still wins over a name lookup.
    assert_eq!(face(resolve("Inter", "Bold").face).info.style, "Bold");
}

/// #1563: a variable font's named instances are styles of its family. `fontStyle = "Black"`,
/// the instance's PostScript name (`font = "Figtree-Black"`) and its full name select the
/// instance's axis values instead of the default instance made bold synthetically, and the
/// font menus list every instance.
#[test]
fn variable_named_instances_resolve_and_list() {
    const FAMILY: &str = "EffectCraft Instance Test";
    let instances = [
        ("Light", 300.0, Some("EffectCraftInstanceTest-Light")),
        ("Regular", 400.0, Some("EffectCraftInstanceTest-Regular")),
        ("Black", 900.0, Some("EffectCraftInstanceTest-Black")),
        ("Extra Bold", 800.0, None),
    ];
    assert_eq!(add_font_data(crate::sfnt::test_font::variable(FAMILY, &instances, true)).len(), 1);
    let wght = |r: &Resolved| instance_variations(r);
    // By style name.
    let black = resolve(FAMILY, "Black");
    assert!(!black.missing && !black.synth_bold && !black.synth_italic, "{black:?}");
    assert_eq!(wght(&black), vec![("wght".to_string(), 900.0)]);
    assert_eq!(wght(&resolve(FAMILY, "regular")), vec![("wght".to_string(), 400.0)]);
    // The face's own style is its default instance: no axis values to set.
    let light = resolve(FAMILY, "Light");
    assert_eq!((light.face, light.instance), (black.face, None));
    // A style the family lacks takes the nearest instance's weight, no synthetic bold.
    let bold = resolve(FAMILY, "Bold");
    assert!(!bold.synth_bold, "{bold:?}");
    assert_eq!(wght(&bold), vec![("wght".to_string(), 800.0)]);
    // By PostScript name, the font's own and the derived one, and by full name.
    let ps = resolve("EffectCraftInstanceTest-Black", "Regular");
    assert!(!ps.missing && ps.face == black.face, "{ps:?}");
    assert_eq!(wght(&ps), vec![("wght".to_string(), 900.0)]);
    assert_eq!(wght(&resolve("EffectCraftInstanceTest-ExtraBold", "")), vec![("wght".to_string(), 800.0)]);
    assert_eq!(wght(&resolve("effectcraft instance test extra bold", "")), vec![("wght".to_string(), 800.0)]);
    // The font menu lists every instance, in weight order.
    let fams = families();
    let styles = &fams.iter().find(|(f, _)| f == FAMILY).unwrap().1;
    assert_eq!(styles, &["Light", "Regular", "Extra Bold", "Black"]);
    // Static fonts are untouched.
    assert_eq!(resolve("Inter", "Bold").instance, None);
}

/// Axis values set on the text win over the instance's; the others keep the instance's.
#[test]
fn explicit_variations_override_the_instance() {
    let base = vec![("wdth".to_string(), 75.0), ("wght".to_string(), 900.0)];
    assert_eq!(merge_variations(&base, &[("wght".to_string(), 450.0)]), vec![("wdth".to_string(), 75.0), ("wght".to_string(), 450.0)]);
    assert_eq!(merge_variations(&base, &[]), base);
    assert_eq!(merge_variations(&[], &[("opsz".to_string(), 12.0)]), vec![("opsz".to_string(), 12.0)]);
}

#[test]
fn scripts_are_classified() {
    assert_eq!(script_of('あ'), FallbackScript::Japanese);
    assert_eq!(script_of('ｶ'), FallbackScript::Japanese);
    assert_eq!(script_of('한'), FallbackScript::Korean);
    assert_eq!(script_of('水'), FallbackScript::Han);
    assert_eq!(script_of('、'), FallbackScript::Han);
    assert_eq!(script_of('Ａ'), FallbackScript::Han);
    assert_eq!(script_of('\u{20000}'), FallbackScript::Han);
    assert_eq!(script_of('A'), FallbackScript::Other);
    assert_eq!(script_of('م'), FallbackScript::Arabic);
    assert_eq!(script_of('\u{fefb}'), FallbackScript::Arabic);
    assert_eq!(script_of('\u{5d0}'), FallbackScript::Hebrew);
    assert_eq!(script_of('क'), FallbackScript::Indic);
    assert_eq!(script_of('த'), FallbackScript::Indic);
    assert_eq!(script_of('ก'), FallbackScript::Thai);
    assert_eq!(script_of('ა'), FallbackScript::Georgian);
    assert_eq!(script_of('\u{1c90}'), FallbackScript::Georgian, "Mtavruli");
    assert_eq!(script_of('\u{feff}'), FallbackScript::Other, "the byte order mark is no Arabic letter");
}

/// Native: Arabic, Hebrew, Indic and Thai text in a Latin-only family falls back to the
/// platform's plain text family for its script (#394), not to the first installed face that
/// covers it (on Windows a calligraphic Arabic face sorts first). Skipped per script where no
/// family from its list is installed.
#[test]
fn other_scripts_fall_back_to_a_plain_system_family() {
    scan_system();
    let inter = resolve("Inter", "Regular").face;
    for (c, families) in [('م', ARABIC_FAMILIES), ('א', HEBREW_FAMILIES), ('क', INDIC_FAMILIES), ('த', INDIC_FAMILIES), ('ก', THAI_FAMILIES)] {
        let covering = families.iter().any(|n| resolve_in(n, "Regular").is_some_and(|r| face(r.face).has_char(c)));
        if !covering {
            eprintln!("{c}: no family from the list is installed; skipping");
            continue;
        }
        let f = face(fallback_for(c, inter));
        assert!(f.has_char(c), "{c}: {} lacks it", f.info.family);
        assert!(families.iter().any(|n| f.info.family.eq_ignore_ascii_case(n)), "{c} → {}", f.info.family);
    }
}

/// Native: Mkhedruli in a Latin-only family falls back to a family from the Georgian list,
/// ahead of faces already loaded and of the scan order. Skipped where no family from the list
/// is installed.
#[test]
fn georgian_falls_back_to_a_georgian_system_family() {
    scan_system();
    let c = 'ა';
    if !GEORGIAN_FAMILIES.iter().any(|n| resolve_in(n, "Regular").is_some_and(|r| face(r.face).has_char(c))) {
        eprintln!("no Georgian family from the list is installed; skipping");
        return;
    }
    let f = face(fallback_for(c, resolve("Inter", "Regular").face));
    assert!(f.has_char(c), "{} lacks it", f.info.family);
    assert!(GEORGIAN_FAMILIES.iter().any(|n| f.info.family.eq_ignore_ascii_case(n)), "{c} → {}", f.info.family);
}

/// Native: Japanese text in a Latin-only family falls back to an installed Japanese family
/// (whichever the platform ships) instead of the missing-glyph box, without any layer having
/// loaded a CJK font first. Skipped where no family from the list is installed.
#[test]
fn cjk_falls_back_to_a_system_family() {
    scan_system();
    let inter = resolve("Inter", "Bold").face;
    let installed = JAPANESE_FAMILIES.iter().any(|f| resolve_in(f, "Regular").is_some());
    if !installed {
        eprintln!("no Japanese family installed; skipping");
        return;
    }
    for c in ['水', 'あ', 'ア', '。'] {
        let id = fallback_for(c, inter);
        let f = face(id);
        assert!(f.has_char(c), "{c}: {} {} lacks it", f.info.family, f.info.style);
        assert!(JAPANESE_FAMILIES.iter().any(|n| f.info.family.eq_ignore_ascii_case(n)), "{c} → {}", f.info.family);
    }
    // The bold cut of the fallback family is chosen for bold text when the family has one.
    let f = face(fallback_for('水', inter));
    let (w, _) = style_wants(&f.info.style);
    let has_bold = all_faces().iter().any(|g| g.info.family == f.info.family && style_wants(&g.info.style).0 >= 600);
    assert!(!has_bold || w >= 600, "{} {}", f.info.family, f.info.style);
    // Cached: the second call is a lookup.
    assert_eq!(fallback_for('水', inter), f.id);
}

/// A minimal TrueType font built in code (no font file in the repo): U+0020 and U+E000 map to
/// an empty glyph, U+E001 to a triangle.
fn blank_glyph_font() -> Vec<u8> {
    test_font("Blank Glyph Test")
}

/// The blank-glyph test font under another family name.
fn test_font(family: &str) -> Vec<u8> {
    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    let delta_space = 1u16.wrapping_sub(0x20);
    let cmap4 = {
        let mut t = u16s(&[4, 0, 0, 6, 4, 1, 2]);
        t.extend(u16s(&[0x20, 0xE001, 0xFFFF, 0, 0x20, 0xE000, 0xFFFF, delta_space, 0x2001, 1, 0, 0, 0]));
        let len = t.len() as u16;
        t[2..4].copy_from_slice(&len.to_be_bytes());
        t
    };
    let mut cmap = u16s(&[0, 1, 3, 1, 0, 12]);
    cmap.extend(cmap4);
    // glyph 2: one contour of three on-curve points (100,0) (500,0) (300,600)
    let mut glyf = u16s(&[1, 100, 0, 500, 600, 2, 0]);
    glyf.extend([1u8, 1, 1]);
    glyf.extend(u16s(&[100, 400, (-200i16) as u16, 0, 0, 600]));
    glyf.push(0);
    let loca = u16s(&[0, 0, 0, (glyf.len() / 2) as u16]);
    let mut head = u16s(&[1, 0, 1, 0, 0, 0, 0x5F0F, 0x3CF5, 0, 1000]);
    head.extend([0u8; 16]);
    head.extend(u16s(&[100, 0, 500, 600, 0, 8, 2, 0, 0]));
    let mut hhea = u16s(&[1, 0, 800, (-200i16) as u16, 0, 600, 0, 0, 500, 1, 0, 0]);
    hhea.extend([0u8; 8]);
    hhea.extend(u16s(&[0, 3]));
    let hmtx = u16s(&[600, 0, 600, 0, 600, 0]);
    let maxp = u16s(&[1, 0, 3, 3, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
    let strings: Vec<Vec<u8>> = [family, "Regular"].iter().map(|s| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect();
    let mut name = u16s(&[0, 2, 30]);
    let mut off = 0;
    for (id, st) in strings.iter().enumerate() {
        name.extend(u16s(&[3, 1, 0x409, id as u16 + 1, st.len() as u16, off]));
        off += st.len() as u16;
    }
    strings.iter().for_each(|st| name.extend(st));
    let tables: [(&[u8; 4], Vec<u8>); 8] =
        [(b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"name", name)];
    let mut out = u16s(&[1, 0, tables.len() as u16, 128, 3, 0]);
    let mut offset = 12 + 16 * tables.len();
    let mut body = vec![];
    for (tag, data) in &tables {
        out.extend(*tag);
        out.extend([0u8; 4]);
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        let padded = data.len().div_ceil(4) * 4;
        body.extend(data);
        body.resize(body.len() + padded - data.len(), 0);
        offset += padded;
    }
    out.extend(body);
    out
}

/// A face that maps a character to an empty glyph doesn't cover it, so fallback skips it
/// (#285); a blank space still counts, and a drawn glyph is found as before.
#[test]
fn an_empty_glyph_does_not_cover_its_character() {
    let ids = add_font_data(blank_glyph_font());
    let &[id] = ids.as_slice() else { panic!("one face expected: {ids:?}") };
    let f = face(id);
    assert_eq!(f.info.family, "Blank Glyph Test");
    assert!(f.glyph('\u{e000}').is_some(), "the cmap maps it");
    assert!(!f.has_char('\u{e000}'), "but its glyph is empty");
    assert!(f.has_char('\u{e001}'), "a drawn glyph covers its character");
    assert!(f.has_char(' '), "a space is blank by nature");
    let inter = resolve("Inter", "Regular").face;
    assert_ne!(fallback_for('\u{e000}', inter), id, "fallback never picks the empty glyph");
    assert_eq!(fallback_for('\u{e001}', inter), id, "fallback finds the drawn one");
}

/// A Microsoft family that isn't installed renders in its installed metric-compatible
/// substitute instead of Inter, and still reports itself missing (#1677).
#[test]
fn microsoft_families_resolve_to_metric_compatible_substitutes() {
    add_font_data(test_font("Carlito"));
    add_font_data(test_font("Caladea"));
    let calibri = resolve("Calibri", "Regular");
    assert_eq!(face(calibri.face).info.family, "Carlito");
    assert!(calibri.missing, "the original name stays missing for Find Missing Fonts");
    let cambria = resolve("Cambria", "Bold");
    assert_eq!(face(cambria.face).info.family, "Caladea");
    assert!(cambria.missing);
    let unknown = resolve("No Such Family 1677", "Regular");
    assert_eq!(face(unknown.face).info.family, DEFAULT_FAMILY);
    assert!(unknown.missing);
}

#[test]
fn the_han_order_follows_the_cjk_locale() {
    // Chinese first for any Chinese tag, whether or not it names a script.
    for tag in ["zh", "zh-CN", "zh_Hans", "ZH-Hant-TW.UTF-8", "zh_MO"] {
        assert_eq!(han_order_for(tag)[0], FallbackScript::Han, "{tag}");
    }
    assert_eq!(han_order_for("ko-KR"), [FallbackScript::Korean, FallbackScript::Japanese, FallbackScript::Han]);
    // Japanese, and anything else, keeps the Japanese face first for Han text.
    for tag in ["ja_JP.UTF-8", "en-US", "de", ""] {
        assert_eq!(han_order_for(tag), [FallbackScript::Japanese, FallbackScript::Han, FallbackScript::Korean], "{tag}");
    }
    // The host's request is what `han_order` reads (the ui test covers the effect on the
    // registered fallbacks; this only checks that clearing it returns to the environment).
    assert_eq!(requested_cjk_locale(), None, "tests run without a request");
    assert_eq!(han_order(), han_order_for(&active_cjk_locale()));
}

#[test]
fn the_chinese_family_list_follows_the_script_the_locale_asks_for() {
    for tag in ["zh", "zh-CN", "zh_Hans", "zh_CN.UTF-8", "zh-SG"] {
        assert_eq!(chinese_families_for(tag), SIMPLIFIED_CHINESE_FAMILIES, "{tag}");
    }
    for tag in ["zh-Hant", "zh-Hant-TW", "zh_TW", "zh-HK", "zh_MO.UTF-8"] {
        assert_eq!(chinese_families_for(tag), TRADITIONAL_CHINESE_FAMILIES, "{tag}");
    }
    // Each list carries its own script's faces and not the other's, which is the point of
    // splitting them: `PingFang SC` covers the same code points as `PingFang TC`, so the
    // fallback search would have kept it and drawn Simplified forms in a Traditional interface.
    assert!(SIMPLIFIED_CHINESE_FAMILIES.iter().any(|f| f.contains("PingFang SC") && !f.contains("Source")));
    assert!(!SIMPLIFIED_CHINESE_FAMILIES.iter().any(|f| f.contains("TC") || f.contains("JhengHei")));
    assert!(TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains("PingFang TC")));
    assert!(!TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains(" SC") || f.contains("YaHei") || f.contains("WenQuanYi")));
    assert!(TRADITIONAL_CHINESE_FAMILIES.iter().any(|f| f.contains("Heiti TC") || f.contains("Hiragino Sans CNS")));
    // The arm the fallback itself walks: Han hands over the list the locale's script picked.
    assert_eq!(families_for(FallbackScript::Han, CjkLocale::from_tag(&active_cjk_locale())), chinese_families_for(&active_cjk_locale()));
    // Han text is the only thing these lists serve; the Japanese order does not change.
    assert_eq!(han_order_for("ja_JP.UTF-8")[0], FallbackScript::Japanese);
}

#[test]
fn the_rendered_order_keeps_the_print_face_first() {
    // A composed frame renders with these lists, so they keep the platform's print order: the
    // interface prefers the platform's UI faces on its own (see `install_script_fallbacks` in
    // the ui crate), and an existing project must not render differently because a window's
    // chrome changed.
    let pos = |list: &[&str], name: &str| list.iter().position(|f| *f == name).unwrap_or(usize::MAX);
    assert!(
        pos(JAPANESE_FAMILIES, "Yu Gothic") < pos(JAPANESE_FAMILIES, "Yu Gothic UI"),
        "the print face must come before its UI sibling: {JAPANESE_FAMILIES:?}"
    );
    for name in ["Meiryo UI", "MS UI Gothic"] {
        assert!(!JAPANESE_FAMILIES.contains(&name), "{name} belongs to the interface order, not this list");
    }
    assert_eq!(SIMPLIFIED_CHINESE_FAMILIES[0], "Microsoft YaHei");
    assert_eq!(TRADITIONAL_CHINESE_FAMILIES[0], "Microsoft JhengHei");
    for name in ["Microsoft YaHei UI", "Microsoft JhengHei UI", "PMingLiU"] {
        assert!(
            !SIMPLIFIED_CHINESE_FAMILIES.contains(&name) && !TRADITIONAL_CHINESE_FAMILIES.contains(&name),
            "{name} belongs to the interface order, not these lists"
        );
    }
}

#[test]
fn bundled_ui_fonts_cover_the_ukrainian_alphabet() {
    const ALPHABET: &str = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя’«»";
    for (name, bytes) in
        [("Inter Regular", INTER_REGULAR), ("Inter Medium", INTER_MEDIUM), ("Inter SemiBold", INTER_SEMIBOLD), ("JetBrains Mono", JETBRAINS_MONO_REGULAR)]
    {
        let font = FontRef::new(bytes).unwrap();
        for c in ALPHABET.chars() {
            assert!(covers(&font, c), "{name} cannot draw {c}");
        }
    }
}

#[test]
fn metrics_and_coverage() {
    let f = face(resolve("Inter", "Regular").face);
    let m = f.metrics(100.0);
    assert!(m.ascent > 80.0 && m.ascent < 110.0, "{m:?}");
    assert!(m.descent > 10.0 && m.descent < 40.0);
    assert!(m.cap_height > 60.0 && m.cap_height < 80.0);
    assert!(f.has_char('A') && f.has_char('Ж'));
    assert!(!f.has_char('\u{5d0}'), "Inter has no Hebrew");
    assert!(f.has_feature(b"liga") || f.has_feature(b"calt"));
}
