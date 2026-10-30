# Localization parity

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-30 · **Change:** minor (Polish (`pl`): menus 630, panels 695; recounted at this tree after #1774: Japanese 1,923 of ≈ 4,200 interface strings (46%, with 297 effect names), Simplified Chinese 1,935 (46%, with 307 effect names), Traditional Chinese 1,581 (38%, its panel catalog 951 rows after #2244); overall ≈ 24%; previously: minor, the panel/dialog catalogs grow by 303 rows in Japanese, Simplified Chinese and Traditional Chinese: panel tabs, form and dialog labels, context menus, tracker/graph/text-panel labels, credits and File-panel footers; previously: minor, catalogs recounted at `upstream/main` 470ea7a3: menus in 10 languages, panels in 6, Hangul probe (#599)) · **Target:** Adobe After Effects 2026

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
  `(command, English label)`. A test (`menu_catalogs_have_no_stale_or_duplicate_rows`) fails on a
  row whose entry left the menu tree or on a repeated row, so every catalog covers all fixed
  entries (630 catalog rows each at this tree, counted; 629 in Georgian). The generated Effect menu is not part of
  that fixed tree: its entries and category submenus take their labels from the effect catalog
  below.
- **Panels, dialogs, buttons, tooltips:** `crates/ui-egui/src/i18n/ui.rs`, keyed by the English
  source string, with English fallback. Catalogs exist for Japanese (996 rows), Simplified
  Chinese (998), Traditional Chinese (951, after #2244 pruned the table), Ukrainian (524),
  Brazilian Portuguese (695), Czech (518) and Spanish (513); counted at this tree. Georgian has
  the Simplified Chinese catalog's 694 rows, plus 191 for panels converted to the catalog since
  (885 in all).
- **Effect names:** `crates/ui-egui/src/i18n/effects.rs`, keyed by **effect id**, in Simplified
  Chinese (307 rows, one per effect in the registry), Japanese (297 of the 307 ids) and Georgian
  (307 rows, plus its 21 categories). The registry's
  English name stays the effect's identity: `effect.list`, `effect.apply {"effect": …}`, scripts
  and expressions match on it, and the property tree stores it as the effect group's name. Only
  the drawing — the Effect menu, Effect Controls, the Timeline and Effects & Presets — goes
  through the catalog, so a project file is identical whichever interface language opens it.
  A repeat (`Gaussian Blur 2`) keeps the index the engine appends, and a group the user renamed
  in Effect Controls stays verbatim. Adding a language there is two tables and one arm, and no
  panel changes: Traditional Chinese has its table in place and empty, and Japanese fills 297 of
  the 307 ids, so the rest draw English until a translator completes them. Each language decides
  for itself which names stay English — Adobe's Japanese interface leaves the `CC`, `VR` and
  `OCIO` sets in English while its Chinese one translates them — so a row repeating the English
  name is a choice, not a missing row.
- **Missing strings:** a string with no row in the interface language shows in English, and no
  test, build or release fails on a missing translation. The `missing_translations_report` test
  (`crates/ui-egui/src/i18n.rs`) prints, per language, the fixed menu entries, panel strings, effect
  names and effect categories that have no row. It always passes; `cargo xtask ci` runs it with
  `--nocapture`, so the list is in the gate log. Locally:
  `cargo test -p effectcraft-ui-egui --lib missing_translations_report -- --nocapture`.
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

Measured 10 October 2026 at `origin/main` 1446e27, recounted on 22 October 2026 at `ad8b6acb`,
and recounted again at this tree after #1774 by counting catalog rows (`const` tables in
`crates/ui-egui/src/i18n.rs`, `i18n/ui.rs` and `i18n/effects.rs`: 630 menu rows, 998
panel/dialog sources in Simplified Chinese, 951 in Traditional Chinese and 996 in Japanese). The
denominator, **≈ 4,200 user-visible strings**, is a rounded estimate: the 630 fixed menu entries,
the 998 panel/dialog sources, 307 effect names, ≈ 1,650 distinct effect parameter names and
≈ 620 other unwrapped interface strings (Home screen literals, engine-built tooltips, error and
status messages). It counts distinct strings, not catalog rows; engine messages are only partly
counted, so the true percentages are slightly lower.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | To `full` |
|---|---|---|---|---|---|---|---|
| English | `en` | ≈ 4,200 (100%, source) | All; docs in English | — | yes | full | — |
| Simplified Chinese | `zh-hans` | 1,935 (≈ 46%): menus 630 rows, panels 998, effect names 307 | Panels and dialogs partly; no help, effect names translated and their parameter names not; Home screen literals unwrapped (#574, closed) | System CJK fallback; Han follows the UI language; CJK text in comps renders wrongly (#453); IME in text layers | no | partial | 6–9 h + review |
| Spanish | `es` | 1,142 (≈ 29%): menus 629 rows, panels 513 | Menus only; the panel catalog from #996 | Latin, fine | no | partial | 9–12 h + review |
| Hindi | `hi` | 0 | — | UI: Devanagari fallback font but no shaping; text layers shape Devanagari | no | none (After Effects: none) | 9–13 h + shaping in the UI |
| Arabic | `ar` | 0 | — | UI: Arabic fallback font but no shaping and no RTL layout; text layers: bidi + shaping | no | none (After Effects: none) | 9–13 h + 15–25 h RTL UI |
| French | `fr` | 624 (≈ 16%) | Menus only | Latin, fine | no | menus only | 7–10 h + review |
| Portuguese (Brazil) | `pt-br` | 1,324 (≈ 34%): menus 629 rows, panels 695 | Panels and dialogs at the Chinese catalogs' source list; the six keys main added after that count are translated too; AI-assisted, not yet native-speaker proofread | Latin, fine | no | partial | independent native review |
| Indonesian | `id` | 0 | — | Latin, fine | no | none (After Effects: none) | 8–12 h + review |
| Japanese | `ja` | 1,923 (≈ 46%): menus 630 rows, panels 996, effect names 297 | Panels and dialogs partly; no help, effect names translated for 297 of the 307 ids and their parameter names not; Home screen literals unwrapped (#574, closed) | Japanese system fallback; vertical text and Tate-Chu-Yoko in text layers; IME | no | partial | 6–9 h + review |
| German | `de` | 624 (≈ 16%) | Menus only (open PR #666 adds interface translations) | Latin, fine | no | menus only | 7–10 h + review |
| Korean | `ko` | 0 | — | Hangul fallback probe added (#599, closed); not checked on macOS here | no | none | 9–13 h + review |
| Vietnamese | `vi` | 0 | — | Latin with stacked diacritics; untested | no | none (After Effects: none) | 8–12 h + review |

Other languages EffectCraft ships (5):

| Language | Code | UI strings translated | Status | To `full` |
|---|---|---|---|---|
| Traditional Chinese | `zh-hant` | 1,581 (≈ 38%): menus 630 rows, panels 951 | partial | 6–9 h + review |
| Ukrainian | `uk` | 1,153 (≈ 29%): menus 629 rows, panels 524 | partial | 6–9 h + review |
| Polish | `pl` | 1,325 (≈ 34%): menus 630 rows, panels 695 | partial; unwrapped UI, effects and engine text still in English; AI-drafted, native review welcome | Remaining strings + native review |
| Czech | `cs` | 1,147 (≈ 29%): menus 629 rows, panels 518 | Existing catalog entries covered; unwrapped UI, effects and engine text still in English; AI-assisted, not yet native-speaker proofread | Remaining strings + independent native review |
| Georgian | `ka` | 1,514 (≈ 39%): menus 629 rows, panels 885 (the Simplified Chinese catalog's 694 plus 191 Georgian-only rows: form dialogs, context menus, column headers), effect names 307 + 21 categories; native-speaker reviewed | partial | 5–8 h |

Russian (`ru`) has a menu catalog (630 rows, ≈ 15%); After Effects also ships Italian, which EffectCraft does not (8–12 h).

**Overall: ≈ 24%**, the average coverage of After Effects' 10 non-English languages
(Simplified Chinese 46%, Traditional Chinese 38%, Japanese 46%, Brazilian Portuguese 34%,
Spanish 29%, French, German and Russian 16%, Korean and Italian 0%; 241 / 10, was ≈ 21% on
22 October; Ukrainian (≈ 29%) and Czech (≈ 29%) are EffectCraft-only and stay out of that
average). This is the localization row in [target-app-parity.md](target-app-parity.md#by-dimension).

**Effort.** One-time work first: route parameter names and engine messages through
a catalog, wrap the remaining panel literals, and add a Hangul probe (10–20 h). Then each language
needs ≈ 2,600 more strings (≈ 5–9 h per language at the pace of #398 and #401). Parity with After
Effects' 10 non-English languages: **≈ 60–110 h**; the four extra languages of the standard list
(Hindi, Arabic, Indonesian, Vietnamese) add ≈ 50–80 h, most of it right-to-left layout and
complex shaping in the interface. Every language needs native-speaker review, which only a human
can give; only Georgian, contributed by a native speaker (#1843), has had it.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | Recounted on the merged tree after #1774: Japanese 1,923 (46%, 297 effect names), Simplified Chinese 1,935 (46%, 307 effect names), Traditional Chinese 1,581 (38%, 951 panel rows after #2244's prune); overall ≈ 24% |
| 2026-10-22 | minor | Panel/dialog second step: 300 more catalog rows in Japanese, Simplified Chinese and Traditional Chinese, plus the three rows the review found missing (Basic, Advanced and Play/Stop) — panel tabs, form and dialog labels, panel context menus, the tracker and graph controls, text-panel options, File-panel footers and credits; the panel tables are now 996 Japanese and 998 Simplified Chinese rows, all drawn through `tr`/`tr_args`, with the tests keeping the three catalogs listing the same sources; the 30 October recount above carries the share |
| 2026-10-18 | minor | Brazilian Portuguese completes its panel catalog (278 new rows, 417 → 695, the Chinese catalogs' source list); the doc recount closes the drift the batch merges left in every language's panel count; AI-assisted, not yet native-speaker proofread |
| 2026-10-14 | minor | Georgian (`ka`): menus 628, panels 886 (191 Georgian-only, in `GEORGIAN_PANELS`), effect names and categories in `effects.rs` |
| 2026-10-29 | minor | Polish (`pl`): menus 630, panels 695 |
| 2026-10-13 | minor | Effect names and the generated Effect menu leave English: `i18n/effects.rs` adds 306 rows keyed by effect id (Simplified Chinese, from After Effects 2026's interface and Adobe's Chinese documentation for the `CC` series) and the 21 category names. Parameter names are still English; no other language has rows |
| 2026-10-12 | minor | Recounted: Russian, French, German and Czech menus (#873, #987), Brazilian Portuguese and Czech panels (#830, #987), Hangul probe (#599); overall ≈ 12% to ≈ 18%; Korean and Italian still absent |
| 2026-10-11 | minor | Ukrainian panel catalog brought level with the Chinese ones (495 of 496 rows, counted at this commit); its share rises to ≈ 29%. The other languages' panel counts are the 10 October measurement |
| 2026-10-10 | major | First localization parity document: catalog rows counted for all six languages, the twelve-language table, After Effects' 11 shipped languages from its bundle |
