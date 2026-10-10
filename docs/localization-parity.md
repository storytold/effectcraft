# Localization parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first measurement of every catalog against the After Effects 2026 26.5 language set) · **Target:** Adobe After Effects 2026

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
  **618** fixed entries. The generated Effect menu (effect names) is deliberately left in English.
- **Panels, dialogs, buttons, tooltips:** `crates/ui-egui/src/i18n/ui.rs`, keyed by the English
  source string, with English fallback. Catalogs exist for Japanese, Simplified and Traditional
  Chinese and Ukrainian.
- **Not translated anywhere yet:** effect names and their ≈ 1,650 parameter names
  (`crates/effects`), engine-generated text (property names, errors, undo labels), the Home
  screen's unwrapped literals (#574), Learn tutorials and documentation.
- **Language choice:** Settings ▸ General ▸ Language, or Match System
  (`match_system_uses_the_system_language_where_there_is_a_catalog`).
- **Fonts:** no bundled CJK font; the UI installs system fallbacks probed by script (Kana, Han,
  Arabic, Hebrew, Devanagari, Bengali, Gurmukhi, Gujarati, Tamil, Telugu, Kannada, Malayalam,
  Thai: `crates/ui-egui/src/theme.rs`). There is no Hangul probe, so Korean renders as boxes on
  macOS (#599). The web build has no system fonts.
- **Script support in the interface:** egui draws text without complex shaping or right-to-left
  layout, so an Arabic or Hindi interface would render unshaped and left to right. **In text
  layers** (`crates/text`) shaping (harfrust), bidi (UAX #9), vertical text and Tate-Chu-Yoko
  work; text-layer typing goes through the IME (`viewer_text.rs` handles preedit and commit).

## Measurement

Measured 10 October 2026 at `origin/main` 1446e27 by counting catalog rows. The denominator,
**≈ 3,900 user-visible strings**, is estimated: 618 fixed menu entries + ≈ 1,320 capitalised UI
literals in `crates/ui-egui/src` outside tests and catalogs (a regex count, so an upper bound) +
≈ 1,650 distinct effect parameter names + 306 effect names. Engine messages are not counted, so
the true percentages are slightly lower.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | To `full` |
|---|---|---|---|---|---|---|---|
| English | `en` | ≈ 3,900 (100%, source) | All; docs in English | — | yes | full | — |
| Simplified Chinese | `zh-hans` | 1,081 (≈ 28%): menus 618/618, panels 463 | Panels and dialogs partly (≈ 35% of panel strings); no help, effects untranslated; Home screen literals unwrapped (#574) | System CJK fallback; Han follows the UI language; CJK text in comps renders wrongly (#453); IME in text layers | no | partial | 6–9 h + review |
| Spanish | `es` | 618 (≈ 16%) | Menus only | Latin, fine | no | menus only | 7–10 h + review |
| Hindi | `hi` | 0 | — | UI: Devanagari fallback font but no shaping; text layers shape Devanagari | no | none (After Effects: none) | 9–13 h + shaping in the UI |
| Arabic | `ar` | 0 | — | UI: Arabic fallback font but no shaping and no RTL layout; text layers: bidi + shaping | no | none (After Effects: none) | 9–13 h + 15–25 h RTL UI |
| French | `fr` | 0 | — | Latin, fine | no | none | 8–12 h + review |
| Portuguese (Brazil) | `pt-br` | 618 (≈ 16%) | Menus only | Latin, fine | no | menus only | 7–10 h + review |
| Indonesian | `id` | 0 | — | Latin, fine | no | none (After Effects: none) | 8–12 h + review |
| Japanese | `ja` | 1,015 (≈ 26%): menus 618/618, panels 397 | Panels and dialogs partly (≈ 30% of panel strings) | Japanese system fallback; vertical text and Tate-Chu-Yoko in text layers; IME | no | partial | 6–9 h + review |
| German | `de` | 0 | — | Latin, fine | no | none | 8–12 h + review |
| Korean | `ko` | 0 | — | No Hangul fallback probe: boxes on macOS (#599) | no | none | 9–13 h + review |
| Vietnamese | `vi` | 0 | — | Latin with stacked diacritics; untested | no | none (After Effects: none) | 8–12 h + review |

Other languages EffectCraft ships (2):

| Language | Code | UI strings translated | Status | To `full` |
|---|---|---|---|---|
| Traditional Chinese | `zh-hant` | 1,081 (≈ 28%): menus 618/618, panels 463 | partial | 6–9 h + review |
| Ukrainian | `uk` | 1,015 (≈ 26%): menus 618/618, panels 397 | partial | 6–9 h + review |

After Effects also ships Italian and Russian, which EffectCraft does not (8–12 h each).

**Overall: ≈ 12%**, the average coverage of After Effects' 10 non-English languages
(Simplified and Traditional Chinese 28%, Japanese 26%, Spanish and Portuguese 16%, French,
German, Korean, Italian and Russian 0%). This is the localization row in
[target-app-parity.md](target-app-parity.md#by-dimension).

**Effort.** One-time work first: route effect names, parameter names and engine messages through
a catalog, wrap the remaining panel literals, and add a Hangul probe (10–20 h). Then each language
needs ≈ 2,900 more strings (≈ 5–9 h per language at the pace of #398 and #401). Parity with After
Effects' 10 non-English languages: **≈ 60–110 h**; the four extra languages of the standard list
(Hindi, Arabic, Indonesian, Vietnamese) add ≈ 50–80 h, most of it right-to-left layout and
complex shaping in the interface. Every language needs native-speaker review, which only a human
can give; none has had it.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | First localization parity document: catalog rows counted for all six languages, the twelve-language table, After Effects' 11 shipped languages from its bundle |
