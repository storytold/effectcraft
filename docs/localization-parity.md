# Localization parity

> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (catalogs recounted at `upstream/main` 470ea7a3: menus in 10 languages, panels in 6, Hangul probe (#599)) · **Target:** Adobe After Effects 2026

How much of EffectCraft's interface is translated, language by language, compared with After
Effects. [gaps.md](gaps.md) gap 16 and workstream G9 hold the work;
[target-app-parity.md](target-app-parity.md) the overall numbers.

## What After Effects ships

The installed After Effects 2026 (26.5.0.89) bundle has 11 `.lproj` localisations: English,
French, German, Italian, Japanese, Korean, Portuguese (Brazil), Russian, Spanish, Simplified
Chinese (`zh_CN`) and Traditional Chinese (`zh_TW`). Each translates the whole interface,
including effect names and parameters. It ships no Hindi, Arabic, Indonesian or Vietnamese
interface; Arabic and Hebrew text *in layers* is supported through its Middle Eastern text engine.

## How EffectCraft translates

- **Menus:** `crates/ui-egui/src/i18n.rs`, one catalog per language keyed by
  `(command, English label)`. A test (`every_translation_key_exists_in_the_actual_menu_tree`)
  fails if any fixed menu entry is untranslated or a row is stale, so every catalog covers all
  fixed entries (624 catalog rows each at 470ea7a3, counted). The generated Effect menu is not part of that fixed tree: its entries
  and category submenus take their labels from the effect catalog below.
- **Panels, dialogs, buttons, tooltips:** `crates/ui-egui/src/i18n/ui.rs`, keyed by the English
  source string, with English fallback. Catalogs exist for Japanese (410 rows), Simplified and
  Traditional Chinese (506 each), Ukrainian (505), Brazilian Portuguese (400) and Czech (501);
  counted at 470ea7a3.
- **Effect names:** `crates/ui-egui/src/i18n/effects.rs`, keyed by **effect id**, in Simplified
  Chinese (306 rows, one per effect in the registry, plus its 21 categories). The registry's
  English name stays the effect's identity: `effect.list`, `effect.apply {"effect": …}`, scripts
  and expressions match on it, and the property tree stores it as the effect group's name. Only
  the drawing — the Effect menu, Effect Controls, the Timeline and Effects & Presets — goes
  through the catalog, so a project file is identical whichever interface language opens it.
  A repeat (`Gaussian Blur 2`) keeps the index the engine appends, and a group the user renamed
  in Effect Controls stays verbatim. Adding a language there is two tables and one arm, and no
  panel changes: Traditional Chinese and Japanese have their tables in place and empty, so those
  languages draw English until a translator fills them (the Japanese set for #821 is what that
  slot is for). Each language decides for itself which names stay English — Adobe's Japanese
  interface leaves the `CC`, `VR` and `OCIO` sets in English while its Chinese one translates
  them — so a row repeating the English name is a choice, not a missing row.
- **Not translated anywhere yet:** effect *parameter* names (≈ 1,650, `crates/effects`),
  engine-generated text (property names, errors, undo labels), the Home screen's unwrapped
  literals (#574), Learn tutorials and documentation.
- **Language choice:** Settings ▸ General ▸ Language, or Match System
  (`match_system_uses_the_system_language_where_there_is_a_catalog`).
- **Fonts:** no bundled CJK font; the UI installs system fallbacks probed by script (Kana, Han,
  Arabic, Hebrew, Devanagari, Bengali, Gurmukhi, Gujarati, Tamil, Telugu, Kannada, Malayalam,
  Thai and Hangul: `crates/ui-egui/src/theme.rs`; the Hangul probe was added for #599, closed). The web build has no system fonts.
- **Script support in the interface:** egui draws text without complex shaping or right-to-left
  layout, so an Arabic or Hindi interface would render unshaped and left to right. **In text
  layers** (`crates/text`) shaping (harfrust), bidi (UAX #9), vertical text and Tate-Chu-Yoko
  work; text-layer typing goes through the IME (`viewer_text.rs` handles preedit and commit).

## Measurement

Measured 10 October 2026 at `origin/main` 1446e27 and recounted on 12 October at `upstream/main`
470ea7a3 by counting catalog rows (`const` tables in `crates/ui-egui/src/i18n.rs` and
`i18n/ui.rs`; menu rows 624 per language, so the share is computed from 624 rather than 618). The
denominator, **≈ 3,900 user-visible strings**, was estimated on 10 October and is kept: 618 fixed menu entries + ≈ 1,320 capitalised UI
literals in `crates/ui-egui/src` outside tests and catalogs (a regex count, so an upper bound) +
≈ 1,650 distinct effect parameter names + 306 effect names. Engine messages are not counted, so
the true percentages are slightly lower.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | To `full` |
|---|---|---|---|---|---|---|---|
| English | `en` | ≈ 3,900 (100%, source) | All; docs in English | — | yes | full | — |
| Simplified Chinese | `zh-hans` | 1,130 (≈ 29%): menus 624 rows, panels 506 | Panels and dialogs partly; no help, effect names translated (306) and their parameter names not; Home screen literals unwrapped (#574, closed) | System CJK fallback; Han follows the UI language; CJK text in comps renders wrongly (#453); IME in text layers | no | partial | 6–9 h + review |
| Spanish | `es` | 624 (≈ 16%) | Menus only (panel catalog in open PR #996) | Latin, fine | no | menus only | 7–10 h + review |
| Hindi | `hi` | 0 | — | UI: Devanagari fallback font but no shaping; text layers shape Devanagari | no | none (After Effects: none) | 9–13 h + shaping in the UI |
| Arabic | `ar` | 0 | — | UI: Arabic fallback font but no shaping and no RTL layout; text layers: bidi + shaping | no | none (After Effects: none) | 9–13 h + 15–25 h RTL UI |
| French | `fr` | 624 (≈ 16%) | Menus only | Latin, fine | no | menus only | 7–10 h + review |
| Portuguese (Brazil) | `pt-br` | 1,024 (≈ 26%): menus 624 rows, panels 400 | Panels and dialogs partly | Latin, fine | no | partial | 6–9 h + review |
| Indonesian | `id` | 0 | — | Latin, fine | no | none (After Effects: none) | 8–12 h + review |
| Japanese | `ja` | 1,034 (≈ 27%): menus 624 rows, panels 410 | Panels and dialogs partly | Japanese system fallback; vertical text and Tate-Chu-Yoko in text layers; IME | no | partial | 6–9 h + review |
| German | `de` | 624 (≈ 16%) | Menus only (open PR #666 adds interface translations) | Latin, fine | no | menus only | 7–10 h + review |
| Korean | `ko` | 0 | — | Hangul fallback probe added (#599, closed); not checked on macOS here | no | none | 9–13 h + review |
| Vietnamese | `vi` | 0 | — | Latin with stacked diacritics; untested | no | none (After Effects: none) | 8–12 h + review |

Other languages EffectCraft ships (3):

| Language | Code | UI strings translated | Status | To `full` |
|---|---|---|---|---|
| Traditional Chinese | `zh-hant` | 1,130 (≈ 29%): menus 624 rows, panels 506 | partial | 6–9 h + review |
| Ukrainian | `uk` | 1,129 (≈ 29%): menus 624 rows, panels 505 | partial | 6–9 h + review |
| Czech | `cs` | 1,125 (≈ 29%): menus 624 rows, panels 501 | Existing catalog entries covered; unwrapped UI, effects and engine text still in English; AI-assisted, not yet native-speaker proofread | Remaining strings + independent native review |

Russian (`ru`) has a menu catalog (624 rows, ≈ 16%); After Effects also ships Italian, which EffectCraft does not (8–12 h).

**Overall: ≈ 18%**, the average coverage of After Effects' 10 non-English languages
(Simplified and Traditional Chinese 29%, Japanese 27%, Brazilian Portuguese 26%, Spanish, French,
German and Russian 16%, Korean and Italian 0%; 175 / 10, was ≈ 12% on 10 October). This is the localization row in
[target-app-parity.md](target-app-parity.md#by-dimension).

**Effort.** One-time work first: route parameter names and engine messages through
a catalog, wrap the remaining panel literals, and add a Hangul probe (10–20 h). Then each language
needs ≈ 2,900 more strings (≈ 5–9 h per language at the pace of #398 and #401). Parity with After
Effects' 10 non-English languages: **≈ 60–110 h**; the four extra languages of the standard list
(Hindi, Arabic, Indonesian, Vietnamese) add ≈ 50–80 h, most of it right-to-left layout and
complex shaping in the interface. Every language needs native-speaker review, which only a human
can give; none has had it.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-12 | minor | Recounted: Russian, French, German and Czech menus (#873, #987), Brazilian Portuguese and Czech panels (#830, #987), Hangul probe (#599); overall ≈ 12% to ≈ 18%; Korean and Italian still absent |
| 2026-10-10 | major | First localization parity document: catalog rows counted for all six languages, the twelve-language table, After Effects' 11 shipped languages from its bundle |
| 2026-10-11 | minor | Ukrainian panel catalog brought level with the Chinese ones (495 of 496 rows, counted at this commit); its share rises to ≈ 29%. The other languages' panel counts are the 10 October measurement |
| 2026-10-13 | minor | Effect names and the generated Effect menu leave English: `i18n/effects.rs` adds 306 rows keyed by effect id (Simplified Chinese, from After Effects 2026's interface and Adobe's Chinese documentation for the `CC` series) and the 21 category names. Parameter names are still English; no other language has rows |
