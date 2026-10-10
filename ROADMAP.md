# EffectCraft Roadmap

**Stage: alpha** · next: beta, ~30 points of "ready for real work" and ~350–620 h away

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (full re-measure against After Effects 2026 26.5; standard progress-docs layout) · **Target:** Adobe After Effects 2026 (26.5.0.89)

EffectCraft aims to do what After Effects does, with the same panels, menus and behaviour, written
from scratch in Rust. This page is the one-page summary; the evidence is in
[docs/target-app-parity.md](docs/target-app-parity.md), the work list in
[docs/gaps.md](docs/gaps.md), and the plan in [docs/roadmap.md](docs/roadmap.md).

## Headline numbers

| | Value | Kind |
|---|---|---|
| **Feature breadth** | **≈ 91%** | Estimated, weighted by area; the self-graded 92-item checklist says ≈ 99% |
| **Ready for real work** | **≈ 45%** (40–50%) | Estimated; one live fidelity measurement so far |
| **Mainstream practitioner** | **≈ 47%** | Estimated: the typical motion designer's weekly work ([how](docs/target-app-parity.md#mainstream-practitioner-and-essentials-user)) |
| **Essentials user** | **≈ 52%** | Estimated: core features only ([how](docs/target-app-parity.md#essentials-user)) |
| Remaining to **beta** | **≈ 350–620 Opus 5.5 agent-hours** | Estimated |
| Remaining to **full parity** | **≈ 850–1,500 Opus 5.5 agent-hours** | Estimated; ≈ 70% parallelises |

| Audience | Ready % | Hours to ~95% | Work that dominates |
|---|---:|---:|---|
| Full target | ≈ 45% | ≈ 850–1,500 h | `.aep`, fidelity corpus, bug backlog, performance, formats, hardware, localization |
| Mainstream practitioner | ≈ 47% | ≈ 280–530 h | `.aep` exchange, stability, everyday fidelity and bugs, preview speed |
| Essentials user | ≈ 52% | ≈ 150–300 h | Opening tutorial `.aep` files, start-up stability, core bugs, preview with sound |

About 60–70% of these hours parallelise ([detail](docs/target-app-parity.md#readiness-by-audience)).

Hours are one Opus 5.5 agent working sequentially, calibrated on this repository's history
([how](docs/target-app-parity.md#calibration-of-the-hours)).

### Stage

**Alpha**, because all six core workflows of the [alpha gate](docs/roadmap.md#alpha-gate)
(set up, animate, text and shapes, composite, preview, save and deliver) work end to end on macOS
and the work saves and reopens, and ready for real work (≈ 45%) is above the ≈ 40% bar. But: After Effects projects (`.aep`) can't be opened, which
alone rules out beta; fidelity against After Effects is measured for one narrow case (scalar ease,
168 samples); 134 issues are open, eight of them crash reports. Beta needs ≈ 75% ready and `.aep`
opening reliably: the `.aep` owner decision and importer (80–160 h), the fidelity harness and
P0/P1 corpus (60–100 h), the crash and bug backlog (40–80 h), performance (40–70 h),
Linux/Windows reliability (30–50 h) and depth in animation, masks, effects and text (100–160 h).

## By dimension

| Dimension | % | Hours | Doc |
|---|---:|---:|---|
| Features: breadth | 91% | — | [target-app-parity.md](docs/target-app-parity.md#by-feature-area) |
| Features: ready for real work | 50% | 360–660 | [target-app-parity.md](docs/target-app-parity.md#by-feature-area) |
| UI/UX fidelity | 65% | 60–120 | [ui-parity.md](docs/ui-parity.md) |
| File formats | 50% | 130–230 | [file-format-parity.md](docs/file-format-parity.md) |
| Hardware | 40% | 60–120 | [hardware-parity.md](docs/hardware-parity.md) |
| Localization | 12% | 60–110 + review | [localization-parity.md](docs/localization-parity.md) |
| Performance | 35% | 50–100 | [gaps.md](docs/gaps.md#g5-performance-at-real-world-scale) |
| Stability | 45% | 40–80 | [gaps.md](docs/gaps.md#g3-stability-and-a-green-main) |
| Platforms | 65% | 30–60 | [gaps.md](docs/gaps.md#g2-real-user-reliability-on-every-platform) |
| Ecosystem and plug-ins | 20% | 40–80 | [plugins.md](docs/plugins.md), [scripting-parity.md](docs/scripting-parity.md) |
| AI features | 35% | 40–80 | [gaps.md](docs/gaps.md) |
| Agent control (beyond After Effects) | ahead | — | [agents.md](docs/agents.md) |

## Features

| Area | Breadth | Ready | Hours |
|---|---:|---:|---:|
| Project, footage and import | 92% | 55% | 25–45 |
| Compositions, layers and compositing | 98% | 60% | 30–55 |
| Animation: keyframes, graph editor, motion paths, puppet | 97% | 55% | 35–60 |
| Expressions and scripting ([detail](docs/scripting-parity.md)) | 93% | 65% | 20–40 |
| Shapes, masks and roto | 93% | 45% | 30–50 |
| Text and typography | 94% | 50% | 20–35 |
| Effects and animation presets ([detail](docs/effects-parity.md)) | 85% | 40% | 70–130 |
| 3D (Classic and Advanced) | 82% | 35% | 35–65 |
| Tracking and AI-assisted tools | 80% | 35% | 35–70 |
| Preview and cache | 92% | 40% | 25–50 |
| Render queue and export | 85% | 55% | 25–45 |
| Audio | 85% | 45% | 6–12 |

Weights and evidence per row: [target-app-parity.md](docs/target-app-parity.md#by-feature-area).

## Languages

UI strings translated out of ≈ 3,900 (detail: [localization-parity.md](docs/localization-parity.md)).

| Language | Status | % |
|---|---|---:|
| English | full | 100% |
| Simplified Chinese | partial (menus, part of the panels) | 28% |
| Spanish | menus only | 16% |
| Hindi | none | 0% |
| Arabic | none | 0% |
| French | none | 0% |
| Portuguese (Brazil) | menus only | 16% |
| Indonesian | none | 0% |
| Japanese | partial (menus, part of the panels) | 26% |
| German | none | 0% |
| Korean | none | 0% |
| Vietnamese | none | 0% |

Also shipped: Traditional Chinese (partial, 28%) and Ukrainian (partial, 26%). No language has had
native-speaker review.

## Upcoming

Ranked; detail in [docs/roadmap.md](docs/roadmap.md) and [docs/gaps.md](docs/gaps.md).

| | Next | Hours |
|---|---|---:|
| 1 | Fix the open crash reports; run `cargo xtask ci` on every pull request (G3) | 15–30 |
| 2 | Fidelity harness against After Effects: rendered frames, a corpus case per P0/P1 feature (G1) | 60–100 |
| 3 | Real-user reliability on Linux and Windows; the open mask, effect, text and 3D bugs (G2) | 100–190 |
| 4 | `.aep` / `.aepx` import, after the owner's clean-room decision (G4) | 80–160 |
| 5 | Performance: benchmarks at 1080p / 4K, real-time cached playback, hardware video (G5) | 50–100 |
| 6 | Localization depth: effect names, all panels, French / German / Korean (G9) | 60–110 |

Come tell us what matters most to you on [Discord](https://discord.gg/artcraft).

## Progress log

- 2026-10-10: Full parity re-measure against After Effects 2026 26.5 (installed bundle, 26.2–26.5
  release notes, repository counts): breadth ≈ 91%, ready ≈ 45%, stage alpha. Progress docs
  follow the shared standard: `docs/parity.md` became `docs/target-app-parity.md`; new
  `docs/roadmap.md`, `localization-parity.md`, `file-format-parity.md`, `hardware-parity.md`,
  `ui-parity.md`, `effects-parity.md` and `scripting-parity.md`.
- 2026-10-10: Spanish (#460) and Brazilian Portuguese (#217) menu translations.
- 2026-10-10: Japanese now translates the interface below the menu bar too: `crates/ui-egui/src/i18n/ui.rs` has a Japanese table with the same 396 rows as the Chinese ones (panels, dialogs, buttons and tooltips; the shape test keeps the three catalogs listing the same source strings). Menus were already Japanese.
- 2026-10-09: Ukrainian now covers all 396 current panel-catalog strings, including dialogs, buttons and tooltips, alongside its menu translation. The catalog keeps the same source keys and placeholder counts as the Chinese catalogs; missing or engine-generated text still falls back to English.
- 2026-10-09: Settings ▸ General ▸ Language now translates the interface below the menu bar as well: panels, dialogs, buttons and tooltips read their text through `crates/ui-egui/src/i18n/ui.rs`, keyed by the English source string. Simplified and Traditional Chinese are complete (384 strings); Japanese rows can follow in the same file. A test fails when a converted panel gains a hard-coded string, and another keeps the catalog free of unused rows.
- 2026-10-08: `effectcraft-cli render --out` resolves a relative path against the working directory, like `--project` (it used the project's folder, which doubled a path that already named it). Render Queue outputs set in a project are unchanged.
- 2026-10-08: Long-press a grouped toolbar button to choose its tools, including Horizontal/Vertical Type; releasing the hold keeps the menu open. Existing point/paragraph text creation is unchanged.
- 2026-10-06: Settings ▸ General ▸ Language persists English/Japanese menu labels (`general.language`); native UI reuses installed Japanese font fallback, with no bundled CJK font. Dialog and panel contents remain English.
- 2026-10-05: Honest assessment ([docs/gaps.md](docs/gaps.md)): breadth ≈ 99% by our own checklist, real use ≈ 30–50%; workstreams G1–G9.
- 2026-10-04: Checklist audit at `d39c0e8`: 89 done / 3 partial / 0 missing of 92 features.
- 2026-10-01: First commit.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | minor | Hours to ≈ 95% for each audience |
| 2026-10-10 | minor | Headline adds mainstream practitioner (≈ 47%) and essentials user (≈ 52%) |
| 2026-10-10 | minor | Stage checked against the new core-workflow gate (all six pass; still alpha) |
| 2026-10-10 | major | Rewritten to the shared progress-docs standard: stage, two numbers, dimensions, features, languages, upcoming, progress log; milestones moved to docs/roadmap.md |
| 2026-10-05 | major | Two numbers (breadth vs real use) and workstreams G1–G9 |
