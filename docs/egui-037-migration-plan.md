# EffectCraft — egui 0.37 migration plan

Current landing status, 2026-10-25: the working tree includes main `991904888a20b7da72b2a4880b21c7d912154736` and pins egui `6d518fc799681571963842f95b55a6b68431b15a`, winit `a517b3846ee8457d84ee4975110bfda6797c44dc` and shared UI `25da1314d443171b4adeaad2cd9454b273f35c6f`. The final IME adapter uses ordered native reset request identifiers and `ImeResetCompleted`; the earlier tombstone-only description below is historical.

Full current-source CI `egui037-effectcraft-current-main-ci-20261025-0231` recorded 3,961 passing tests, 14 failures and 31 ignored across 88 test summaries. All 14 failures are GPU tests: their names and diagnostic blocks match the frozen pre-migration `eb51087` baseline exactly after removing thread process numbers. Formatting, strict lint, layering, assets and the 32-crate WASM check completed. The 596-test UI regression suite passes. This is not a green full-CI result or hardware-GPU qualification. Native IME, visible final-source workflows and paired performance remain open.

The repository's green-only commit gate remains unmet by the GPU results; the reviewed worktree and complete evidence are retained for human review. No push, publication or merge is authorized by this status.

Historical implementation and research record follows. Its older pins, test counts and unacknowledged-reset description do not describe the current candidate.

Status: implementation in progress, 2026-10-24. The dependency/API port, IME context adapter,
screenshots, accessibility, font coverage and text-edit repairs are present in the dirty migration
worktree. The parent-managed local run
`egui037-effectcraft-candidate-20261024-upstream-tests-09` recorded **820 passed, 14 ignored**:
`effectcraft-ui-egui --lib` 297 passed / 3 ignored, `--test it` 521 passed / 11 ignored and
`--test ui_accessibility` 2 passed; `cargo clippy -p effectcraft-ui-egui --lib --tests -- -D warnings`
exited 0. That run covers `effectcraft-ui-egui` only; the engine suite, full `cargo xtask ci`, wasm,
native GUI and real-platform IME were not rerun for this revision. Evidence:
`/home/codex/work/ssd2/task-evidence/egui037-effectcraft-candidate-20261023/task-jobs/egui037-effectcraft-candidate-20261024-upstream-tests-09/output.log`.
The custom viewer text editor now has an explicit per-viewport IME composition lifecycle instead
of a frame-number suppression window: an active composition is owned by the edit session's
layer/session identity, and a target switch or editor end turns it into a tombstone. An empty
pre-edit is the observed end of the visible composition, but its terminal commit/delete can arrive
in a later event, so the owner stays associated with the cleared composition; if the target changes
before that terminal event, the tombstone rejects it instead of applying it to the replacement. A
lone non-empty old pre-edit keeps the tombstone across arbitrary idle frames. A rejected terminal
commit consumes the tombstone immediately, so a new pre-edit later in the same raw input batch
starts a composition for the current target. A rejected empty pre-edit or delete-surrounding keeps
the tombstone through the rest of that batch so its paired old commit is rejected too, and is
consumed at batch end when no commit follows. A composition started after that terminal boundary is
accepted, so the tombstone never becomes a permanent lock. `egui::ImeEvent`
payloads still carry no context generation, and the inspected `egui-winit` 0.37 source ignores
`Ime::Enabled`/`Ime::Disabled` and implements `should_interrupt_composition` by toggling
`set_ime_allowed` without an observable reset acknowledgement. The fallback therefore drops the
first ambiguous composition after an unobserved platform reset rather than claiming provenance it
cannot prove; a producer-side event epoch would be needed to close that residual, and this is not
claimed as tested on a real platform IME here. Two real-app headless regressions in
`crates/ui-egui/tests/it/ui_text_edit.rs` cover the observed-empty-preedit pending commit and a new
pre-edit after a stale terminal commit in the same raw batch; this revision adds them but does not
claim they were rerun here.
The later bounded same-owner interruption repair (2026-10-24) tracks the expected caret and
selection separately from that immutable layer/session owner. A live or terminal-pending
composition is invalidated before application pointer/selection changes can accept an old IME
mutation, including keys before a later IME event in the same raw batch and control-client
selection changes before a pass. Accepted IME mutations update the expected selection so the
IME deletion's own caret shift does not reject its paired commit. The existing terminal rules
still permit a fresh preedit after a rejected terminal commit, and a direct commit with no active
composition remains valid even after a pointer change in the same batch. Five new real-app
regressions cover15 scenarios with both deletion forms, pointer and keyboard interruption,
terminal-pending/external selection, undo preservation, fresh composition and direct commits.
These new source bytes and tests await parent-managed formatting, focused/full gates and fresh
independent review; no new test, native-IME or final dependency-graph acceptance is claimed here.

Every future push or new pull request still needs a fresh, explicit human approval of the exact
commit range and its validation; this document is not publication evidence.

Scope: `storytold/effectcraft` only (this checkout). Shared work in `storytold/craft-libs`
(PR 199 and the CL-2 capture helper), the suite egui fork and the suite winit fork are dependencies
with their own plans; this document records what EffectCraft needs from them, not what they should
do. Sibling apps, their plans and their dependency bumps are out of scope.

This plan deliberately separates:

- **facts** verified by reading the pinned source in this checkout, the egui checkout and the
  upstream 0.36.2/0.37.0 tags;
- **historical clues** (the 2026-10-22 audit compiler output), which are not current acceptance;
- **proposals** (refactors and acceptance criteria) that the implementation PRs must prove.

## 1. Baseline and provenance

### 1.1 Exact source under plan

| Item | Value |
| --- | --- |
| EffectCraft implementation checkout | `/home/codex/work/CraftApps/egui037-migration-20261022/effectcraft-implementation` |
| Current EffectCraft worktree | HEAD `049070d65c66a73f322b496b22521816e0018f14`; dirty migration worktree. The lock and manifests still use the local integration patch, not final immutable pins. |
| EffectCraft historical planning baseline | `9581b53c3f8945683252af0eb5fac79e0e43aed4` (`Release 0.16.0`, 2026-10-22), retained for baseline comparisons only |
| Branch | `codex/egui-037-plan-20261022` |
| Workspace | single root workspace `Cargo.toml` (`resolver = "3"`, members `crates/*`, `apps/*`, `xtask`) |
| Lockfile | `Cargo.lock`, lock format 4 |
| MSRV / edition | Rust 1.95, edition 2024 (`Cargo.toml:7-12`) |
| Target upstream release | egui tag `0.37.0`, peeled commit `841a328ba2dd8f6996beb7189ebb7fc00e668a2a` (tagged 2026-10-15) |
| Candidate suite egui (comparison input only) | `aiongg/craft/0.37` = `a4f9a17f64a6bae61350fb85804c9df0d9865916` (2026-10-21, E1–E21). Read-only source for this plan; **not a publication pin**. |
| Candidate suite winit (comparison input only) | `aiongg/craft/0.30.13-lang` = `8b35388bd719e8cc86a95971ba0bc026c9a8f058` (winit 0.30.13 + W1–W15, W17–W27). Read-only source; **not a publication pin**. |
| Production egui pin | **pending approved fork implementation**: the final reviewed and published `storytold/egui` P8 SHA containing E1–E21, P2b async W15P routing, P7 held-key pairing, P7b/E21b, P7c's selected `#8763`/`#8748` fixes and the justified `#8731`/`#8736` performance fixes if measurements require them. All egui-family crates must come from that one revision. |
| Production winit pin | **pending approved fork implementation**: the completed, final reviewed `storytold/winit` SHA containing W1–W27, W15P and the W27 lifecycle fixes. |
| Final dependency unit | one immutable set recorded together: the complete storytold egui P8 SHA + storytold winit SHA + craft-ui SHA with CL-1 and CL-2F, proved by a clean `--locked` fetch. This is the consumer's first and only compatibility landing: no stock-0.37 or intermediate fork production stage. |
| Suite shared UI dependency | `craft-ui` from `storytold/craft-libs`, reconciled merged-PR base `3832c47ef68a0e6214f2c1e85f3526b6adcb686e` (current shared-library implementation base). Historical planning rev `5c874d51eeb3de70e735c8ce87a59a7dedd47675` is superseded; the final publication pin is the reviewed immutable SHA containing CL-1 and CL-2F. |
| Shared PR history | `storytold/craft-libs#199` is merged at `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`. Historical port hunks `8614e4a04fae516fba16c22b31ff1008cb6be10b` and README `1764bca`, applied to old `main` `49190884278f16c9a9f5823d7698a8ffb5deb062`, are superseded. Never merge `b68de8c`/`ec8840a` as trees. |
| Separate language PR | `storytold/craft-libs#198` (fork E5/E9/E12–E21, winit W17–W27) |

All pins must be full commit SHAs. Branch names are not publication pins. The `aiongg` SHAs above
are source-inspection inputs for this review; the lock file in a publication branch must name only
the final storytold revisions (plus the existing unrelated craft-libs PDF pin).

### 1.2 Baseline dependency set (historical `9581b53c`; from `Cargo.lock` and manifests)

The implementation worktree now resolves the egui family at `0.37.0` and `craft-ui` at
`3832c47ef68a0e6214f2c1e85f3526b6adcb686e` through `local-integration-config.toml`; the current
`Cargo.lock` entries therefore have no registry `source` for those patched crates. That local
configuration is review evidence, not an immutable publication pin.

| Crate | Version / source | Where pinned |
| --- | --- | --- |
| `egui` | `0.36.2` crates.io, checksum `dc938cc2…` | `Cargo.toml:108` |
| `eframe` | `0.36.2` crates.io, checksum `55ad41fa…` | `Cargo.toml:109` (default features off) |
| `egui_kittest` | `0.36.2` | `Cargo.toml:110`, dev-only |
| `egui-wgpu` / `egui-winit` | `0.36.2` | transitive, one each |
| `kittest` | `0.4.0` | transitive of `egui_kittest` |
| `accesskit` / `accesskit_winit` | `0.24.1` / `0.32.2` | transitive |
| `winit` | `0.30.13` crates.io, checksum `a6755fa5…` | `apps/effectcraft/Cargo.toml:23` (`"0.30"`, default features off) and transitive |
| `wgpu` | `30.0.1` | `Cargo.toml:101` |
| `craft-ui` | baseline `0.1.0` git `storytold/craft-libs` rev `5c874d51…`; implementation resolves local `craft-libs-implementation` at `3832c47…` | `Cargo.toml:107` plus `local-integration-config.toml` |
| `craft-pdf` | git `storytold/craft-libs` rev `ca50d63b…` | `Cargo.toml:15` |
| `craft-docpath` | git `storytold/craft-libs` rev `82a24b00…` | `Cargo.toml:16` |
| `hayro-syntax` patch | git `storytold/craft-libs` rev `ca50d63b…`, `=0.7.2` | `Cargo.toml:213-214` |
| `wasm-bindgen` | `0.2.129` (exact crate and CLI), `apps/effectcraft-web/Cargo.toml:34` and `xtask/src/main.rs:254` | web only |
| filmcraft crates | git rev `2c5ba7a7…` | `Cargo.toml:76-95` |

There is exactly one egui family in the baseline. The target must preserve that invariant (see §6).

Manifest audit: 38 `Cargo.toml` files (root, 32 `crates/*`, 3 `apps/*`, `xtask`, and the standalone
plugin). Only `crates/ui-egui`, `apps/effectcraft`, `apps/effectcraft-web` and (through
`effectcraft-ui-egui`) `apps/effectcraft-cli` use the egui family. `examples/plugins/posterize-bands`
has its own workspace/lock and no egui. There are no vendored crates and no `[patch]` on the egui
family in the baseline; the only patch is `hayro-syntax`.

### 1.3 Package, binary, feature and entry-point targets

| Target | Kind | egui-family surface |
| --- | --- | --- |
| `effectcraft` (`apps/effectcraft`) | native binary | `eframe` with `default_fonts`, `wgpu`, `accesskit`, `links`, `wayland`, `x11` (`apps/effectcraft/Cargo.toml:18`); direct `winit`; native `Desktop` app (`apps/effectcraft/src/main.rs:426-456`) |
| `effectcraft-web` (`apps/effectcraft-web`) | `cdylib` + `rlib`, wasm32 | `eframe` web runner (`app.rs:88-122`, `start()` at `app.rs:185`); `eframe` features `default_fonts`, `wgpu` |
| `effectcraft-ui-egui` (`crates/ui-egui`) | library (swappable frontend) | the real UI, control channel, docking, viewer, timeline, kittest dev-dep `eframe` + `wgpu` (`crates/ui-egui/Cargo.toml:24-41`) |
| `effectcraft-cli` | binary + MCP server | depends on `effectcraft-ui-egui` for `bench --ops` (`apps/effectcraft-cli/Cargo.toml:19`) |
| `effectcraft-ui-egui --example snapshot` | example | headless kittest+wgpu screenshots (`crates/ui-egui/examples/snapshot.rs`) |
| `xtask` | tool | `ci`, `wasm`, `web`, `assets`, `layers` (`xtask/src/main.rs:409-428`) |
| `examples/plugins/posterize-bands` | separate nested workspace | no egui dependency; its own `Cargo.lock`; untouched |

Platform cfgs that the migration must respect:

- wasm32: `frames.rs` remote/local frame pump, `audio.rs` Web Audio, `theme.rs` no system fonts,
  `prefs_live.rs` no native drops, viewer deferred readbacks (`lib.rs:825-855`), `sys-locale` only
  off-wasm (`crates/ui-egui/Cargo.toml:36-37`), web storage/workers.
- native: pointer wrap (`pointer_wrap.rs:133`), system font fallbacks (`theme.rs:388-430`),
  `rfd`/`cpal`, native menu (macOS), `zbus` appearance (Linux), appimage (Linux).
- `eframe`/`winit` platform features: Wayland and X11 on Linux, `muda`/`objc2` on macOS,
  `winresource` on Windows.

### 1.4 Current test and CI commands (source-declared)

- `cargo xtask ci` = `cargo fmt --check`; `cargo clippy --workspace --all-targets --release -- -D warnings`;
  `cargo test --workspace --release`; PDF dependency check; `layers`; `assets`; `wasm`
  (`xtask/src/main.rs:409-418`).
- `cargo xtask wasm` checks every L0–L4 crate plus `effectcraft-ui-egui` and `effectcraft-web`
  for `wasm32-unknown-unknown` (`xtask/src/main.rs:235-252`).
- UI tests: 81 files in `crates/ui-egui/tests`, driven by `egui_kittest::Harness`
  (`build_eframe`, `run_steps`, synthetic `Event`s, automation-registry rects, AccessKit queries).
  `ui_gpu.rs` runs the viewer on egui-wgpu's device; `ui_agent_audit.rs` audits commands and
  automation coverage; `viewer_geometry.rs` has an opt-in visual matrix
  (`CRAFT_UI_VISUAL_DIR`).
- Web: `cargo xtask web --serve 8765`, then `node apps/effectcraft-web/tests/smoke.mjs`,
  `workers.mjs`, `page.mjs`, `storage.mjs` (`docs/web.md:412-445`).
- Performance: `effectcraft-cli bench --ops --json` (`apps/effectcraft-cli/src/main.rs:885-925`),
  `cargo xtask bench` (`xtask/src/bench.rs`), and the headless `ui_egui::bench::Driver`
  (`crates/ui-egui/src/bench.rs:13-108`).

## 2. What actually changes in egui 0.37 that affects EffectCraft

This section is limited to changes visible in the pinned sources. It corrects two assumptions that
must not enter the plan:

- **`App::logic` / `App::ui` and `Context::run_ui` are not new in 0.37.** They already exist in
  0.36.2 (`egui-0.36.2/crates/eframe/src/epi.rs:167,182`; `egui-0.36.2/crates/egui/src/context.rs:794`),
  and EffectCraft already uses them (`crates/ui-egui/src/lib.rs:1852-1935`, `apps/effectcraft/src/main.rs:426-456`,
  `apps/effectcraft-web/src/app.rs:88-122`). The migration is **not** a wholesale update-to-ui rewrite.
- **`Context::run_logic` is not a substitute for a pass.** It does not interpret time or events;
  `run_logic` only refreshes viewport state (`egui-0.37.0/crates/egui/src/context.rs:1190-1218`).
  Calling it cannot initialise fonts or consume input.

### 2.1 Compile-visible API changes (the historical audit is a clue, not acceptance)

The 2026-10-22 audit (`/home/codex/work/ssd2/task-evidence/egui037-audit-20261022/{fork,stock}/effectcraft.errors`)
compiled only `effectcraft-ui-egui` (lib and lib test), against a historical fork revision and
machine-local path patches. It reported the same 15 errors in fork and stock mode, with 602
(stock) / 603 (fork) warnings; the difference is warning counts only. Those errors are consistent with the upstream 0.37 sources read here:

| # | Anchor in this checkout | Error | 0.37 API and required change |
| --- | --- | --- | --- |
| 1 | `crates/ui-egui/src/color_picker.rs:19` | `Role::ColorButton` absent | `WidgetType` is a deprecated alias of `accesskit::Role`; use `Role::ColorWell` (`egui-0.37.0/crates/egui/src/lib.rs:644-659`) |
| 2 | `color_picker.rs:336` | `Role::Other` absent | `Role::Unknown` |
| 3 | `control.rs:371` | `Event::MouseWheel` missing `source` | add `source: MouseWheelSource::Wheel` (`egui-0.37.0/crates/egui/src/data/input/event.rs:175-200`) |
| 4 | `lib.rs:1539` | no `egui::UserData` | screenshots are callback based (`ScreenshotCallback`) |
| 5 | `lib.rs:1569` | no `Event::Screenshot` | receive the image from the callback, not from input events |
| 6 | `menu_keys.rs:84` | `UiStack.id` absent | `UiStack.unique_id` / `UiStack.scope_id` (`ui_stack.rs:133-150`); use `scope_id` for a stable panel identity |
| 7 | `panels/effect_controls.rs:1274` | `Role::SelectableLabel` | `Role::Button` with `WidgetInfo::selected` |
| 8 | `panels/project.rs:613` | `Role::Other` | `Role::Unknown` (or `Role::TreeItem` directly) |
| 9 | `panels/project.rs:781` | `Role::Other` | `Role::Unknown` |
| 10 | `panels/text_panels.rs:700` | `Role::SelectableLabel` | `Role::Button` |
| 11 | `theme.rs:732` | `&dyn AsRef<[u8]>` comparison | `FontData.font` is now a `Blob = Arc<dyn AsRef<[u8]> + Send + Sync>`; compare `FontData::bytes()` |
| 12 | `theme.rs:752` | same | same |
| 13 | `widgets.rs:24` | `UiStack.id` | `scope_id` |
| 14 | `widgets.rs:493` | expected `IdSalt`, found `Id` | `Atom::custom(IdSalt, …)` (`atomics/atom.rs:98`) |
| 15 | `widgets.rs:498` | expected `IdSalt` | `WidgetAtomResponse::rect(IdSalt)` (`atomics/widget_atom.rs:224`) |

Deprecations that `-D warnings` turns into failures (all 602 audit warnings are in this list):

| Deprecated item | Count in this checkout | Replacement |
| --- | --- | --- |
| `egui::Id::new` | 616 lines (`591` src, `25` tests); 626 occurrences (`601` src, `25` tests) across 59 src and 12 test files | `Id::unique` (same hash; `id.rs:101-104` delegates to it) |
| `egui::WidgetType` | 19 | `egui::Role` (variant renames in §2.2) |
| `Ui::id()` | 2 (`color_picker.rs:335,337`) | `Ui::scope_id()` (`ui.rs:363-371`) |
| `egui::AtomLayoutResponse` | 1 (`widgets.rs:497`) | `WidgetAtomResponse` |
| `TextEdit::vertical_align` | 1 (`widgets.rs:329`) | `TextEdit::align(Align2::LEFT_CENTER)` (`builder.rs:408-427`) |

Integration tests were not compiled by the audit and must be assumed to contain more breakage.
Two known classes are already visible by inspection:

- 11 `Event::MouseWheel { … }` constructions lack `source` (`control.rs:371`, `ui_timeline_zoom.rs:33,81`,
  `ui_scrolling.rs:34,157`, `viewer_geometry.rs:98,235`, `ui_panels.rs:386`, `ui_tail.rs:195`,
  `ui_layer_panel_zoom.rs:109`, `ui_camera_tracker.rs:173`). The 12th match is a pattern in `lib.rs:581`.
- `ui_agent_audit.rs:73` reads `WidgetInfo.typ`; it becomes `.role`.

There is **no** `PointerSource` in the target fork: `Event::PointerButton` keeps the 0.36 shape
(`aiongg/craft/0.37:crates/egui/src/data/input/event.rs:106-118`). The audit error at
`control.rs:371` was the wheel event, not the pointer button.

### 2.2 Role mapping used by this migration

`WidgetType` → `accesskit::Role` (egui's own mapping table, `egui/src/lib.rs:644-659`):

| Old | New |
| --- | --- |
| `Button` | `Role::Button` |
| `ColorButton` | `Role::ColorWell` |
| `Slider` | `Role::Slider` |
| `SelectableLabel` | `Role::Button` + `selected` |
| `Checkbox` | `Role::CheckBox` |
| `TextEdit` | `Role::TextInput` / `MultilineTextInput` |
| `DragValue` | `Role::SpinButton` |
| `CollapsingHeader` | `Role::DisclosureTriangle` |
| `Panel` | `Role::Pane` |
| `ResizeHandle` | `Role::Splitter` |
| `Other` | `Role::Unknown` |

### 2.3 Behaviour changes that compilation will not catch

These are the app-specific risks, each with a source anchor and a planned assertion.

1. **`default_fonts` is no longer a default feature of `egui`/`epaint`.** eframe still enables it by
   default, but EffectCraft disables eframe defaults and re-enables only
   `default_fonts`, `wgpu`, `accesskit`, `links`, `wayland`, `x11`
   (`crates/ui-egui/Cargo.toml:25`, `apps/effectcraft/Cargo.toml:18`). The app relies on
   `FontDefinitions::default()` at `theme.rs:334`; the feature must stay explicitly enabled on the
   `egui` dependency, not only through eframe, so `cargo check -p effectcraft-ui-egui` and test
   builds are deterministic. (`#8583`; `egui-0.37.0/crates/egui/Cargo.toml:40-46`.)
2. **The bundled emoji fonts are no longer in `FontDefinitions::default()`.** In 0.36.2 the default
   definitions include `NotoEmoji-Regular` and `emoji-icon-font` unconditionally
   (`egui-0.36.2/crates/epaint/src/text/fonts.rs:490-580`). In 0.37 they are behind
   `monochrome_emoji_fonts`, and the default only adds the small `egui-icons` font
   (`egui-0.37.0/crates/epaint/src/text/font_definitions.rs:105-180`). EffectCraft's own chrome is
   affected. A read-only cmap audit of the fonts actually bundled by this app and by egui found:
   - `✔` U+2714 (menu check gutter, `menus.rs:2176`) is in **only** NotoEmoji and emoji-icon-font;
   - `▣` U+25A3 (Media Browser folder, `media_panels.rs:198`; Essential Graphics media row,
     `essential.rs:247`) is in Hack and emoji-icon-font, but Hack is not in the app's
     Proportional family (`theme.rs:339-347` only prepends Inter and appends JetBrains Mono), so
     only the emoji font covers it;
   - `✓` U+2713 (checked label text, `widgets.rs:485`) is in Inter/JetBrains Mono and unaffected.
   Without `monochrome_emoji_fonts` these two become tofu, and kittest's default
   `MissingGlyphPolicy::Panic` turns that into test failures.
3. **Clipboard support became an eframe feature.** 0.36.2's eframe always enabled
   `egui-winit/clipboard` (`egui-0.36.2/crates/eframe/Cargo.toml:152`). 0.37 gates it behind
   `clipboard` (`egui-0.37.0/crates/eframe/Cargo.toml:142-144`), which is in eframe's default set
   but not enabled by EffectCraft's `default-features = false` manifests. Without it, cut/copy/paste
   to the OS clipboard silently degrade to an in-app simulated clipboard
   (`egui-winit-0.37.0/Cargo.toml:44-46`), breaking Edit ▸ Copy/Paste, text-field paste and
   `menus.rs:1867-1906` which maps clipboard events back to command keys.
4. **Screenshots are callback based** (`#8489`; `data/screenshot_callback.rs`,
   `ViewportCommand::Screenshot(ScreenshotCallback)`, `Context::request_screenshot`). The current
   correlation by `UserData` token through `Event::Screenshot` is impossible
   (`lib.rs:1530-1580`, `control.rs:439-444`). See §5.7.
5. **`WidgetType` and ID APIs are deprecated**, and the CI clippy gate is `-D warnings`
   (`xtask/src/main.rs:412`). This is 626 mechanical `Id::new` occurrences (616 lines) plus the 19
   source role edits and one more in `ui_agent_audit.rs` (see §5.2/§5.3).
6. **AccessKit roles/names are enforced by kittest 0.37.** `HarnessBuilder` defaults
   `check_accessibility: true` and `Harness::run` panics on visible input widgets without a name
   (`egui_kittest-0.37.0/src/builder.rs:50-51`, `src/accessibility.rs:9-45`). `is_input` covers
   `Button`, `CheckBox`, `ColorWell`, `ComboBox`, `Slider`, `SpinButton`, `Splitter`, `Tab`,
   `TextInput`, `TreeItem`, … (`egui-0.37.0/crates/egui/src/accessibility.rs:9-60`). Many of the
   app's stock `TextEdit`, `DragValue` and `Slider` fields are currently unnamed (39
   `TextEdit::singleline/multiline` constructors out of 44 `TextEdit::` references, and 51
   `DragValue`/`Slider` constructors), and custom `ui.interact` widgets have no `WidgetInfo` at all.
7. **Missing glyphs panic in kittest 0.37** (`#8601`; `MissingGlyphPolicy::Panic` is the harness
   default, `builder.rs:50`). Tests that deliberately render uninstalled scripts must opt out with
   `HarnessBuilder::allow_missing_glyphs()`; two are already known:
   `tests/ui_effect_names.rs:35` (the builder inside `harness()` at line 20, used by the
   `zh-hans` tests at 68, 81, 107 and 127, including `高斯模糊`) and
   `tests/ui_presets_folder.rs:40` (`ja`). Fontconfig on this planning host lists no faces at all, so
   those tests would panic rather than draw tofu until a CJK face is installed or the opt-out is used.
8. **`Ui::id` → `Ui::scope_id`, `UiBuilder::id` → `scope_id`, `Panel` constructors take
   `impl AsIdSalt`.** EffectCraft does not use `SidePanel`/`TopBottomPanel`, and uses
   `CentralPanel::default()` only (`dock_ui.rs:475`, `prefs_live.rs:207`), so no panel-order
   rewrite is needed. The root `Ui` is already the app surface (`lib.rs:1905-1935`,
   `lib.rs:1614-1723`). Never call `Context::root_ui` from a panel or a test closure: it panics
   while the root Ui is borrowed (`context.rs:1005-1055`).
9. **Style defaults changed** (roomier buttons, larger corner radii; `#8617`, `#8445`).
   EffectCraft pins `item_spacing`, `button_padding`, `interact_size.y`, `menu_margin`, scroll style
   and all widget corner radii from tokens (`theme.rs:510-530`, `theme.rs:474-495`), so the visible
   impact should be small, but atom-based `Button`/`ComboBox` layout can still move geometry. This
   is a geometry/pixel test target, not an assumption.
10. **Menus/popups and text editing changed** (`#7802`, `#7624`, `#8572`, `#8548`, `#8763`,
    `#8628`). EffectCraft builds its own menu bar and keyboard navigation
    (`menus.rs:2025-2050`, `menu_keys.rs`), with `MenuConfig` style overrides. Regression tests must
    cover hover-switch, arrows, submenu opening, outside-click close, and a button acting on a
    focused `TextEdit`'s selection (the `#8763` case). Container tooltips now also show when their
    children are hovered (`#7909`); check tooltip presence/absence on the app's large custom
    `interact` rects (viewer, timeline, dock) instead of assuming the 0.36 behaviour.
11. **Kinetic scrolling/rubber-banding** (`#8662`, `#8663`) and the new `MouseWheelSource` change
    wheel semantics. Synthetic wheel events must say `Wheel` (not `Unknown`) so tests and the
    control channel keep deterministic scroll behaviour.
12. **`Frame::default_fonts` + AccessKit bump**: accesskit 0.24.1 → 0.25.1, accesskit_winit
    0.32.2 → 0.34.1, kittest 0.4.0 → 0.5.0. The kittest trees are **not** identical: `lib.rs`
    re-exports `accesskit_consumer::NodeRef` as `AccessKitNode` where 0.4 re-exported `Node`, and
    `query.rs`/`filter.rs` also differ. The 26 query call sites must be compiled against 0.5, not
    assumed unchanged.
13. **New 0.37 lifecycle tools** that this plan uses:
    - `ViewportCommand::RequestPaintWhileHidden` and `ViewportInfo::visible()`
      (`viewport.rs:1202-1240`, `viewport_info.rs:90-101`); visibility remains derived from
      minimized/occluded even during a forced hidden UI paint;
    - `Context::request_screenshot` / `ScreenshotCallback` (`context.rs:4595-4606`). The call sends
      a `ViewportCommand`, which requests a repaint of that viewport; a pending screenshot also
      makes a hidden window take a full UI+paint pass (`wgpu_integration.rs:686-690`,
      `egui-winit/src/lib.rs:2116-2128`). It does **not** schedule a repaint when the callback
      completes; the completion slot owns that wake-up (§5.7);
    - `Context::set_repaint_observer` and `RepaintCause` (`context.rs:2290-2296`, `283-300`).
14. **Known upstream regressions at 0.37.0** (reported upstream; not measured by this task):
    - `#8731`: plain-text tessellation ≈61% more instructions (`tessellate_text` per-vertex colour
      check). The candidate comparison revision is unpatched: `aiongg/craft/0.37:crates/epaint/src/tessellator.rs:2295-2341`
      still has the conditional inside the vertex loop.
    - `#8736`: egui-wgpu pipeline creation is ~40 ms slower on Windows/DX12 (FXC) because of
      `wrap_texel_coord`.
    Both are measured acceptance gates in §9 and, if over threshold, must be fixed in the final
    P8 egui revision before P1 is frozen. P5 reports/retests; it does not change the pin.

## 3. Fork, winit and craft-libs integration decisions

### 3.1 What EffectCraft actually needs from the suite fork

EffectCraft has no `vendor/` directory and no local egui patch. Its only `[patch.crates-io]` entry is
`hayro-syntax`, inherited from the craft-libs PDF stack (`Cargo.toml:213-214`); it is unrelated to
egui and must stay.

The production dependency is the complete reviewed and published storytold egui **P8** revision,
not the personal `aiongg` branches used as comparison input here. P8 carries E1–E21, P2b async
W15P routing, P7 held-key pairing, P7b/E21b outgoing IME surrounding text (§5.5), P7c's selected
`#8763`/`#8748` fixes and the justified `#8731`/`#8736` performance fixes. The completed final
winit SHA carries W1–W27, W15P and the W27 lifecycle fixes. The two forks and `craft-ui` CL-1 +
CL-2F are approved as one immutable unit at P1; P4 adds extended native/language coverage without
a second compatibility graph (§10). There is no stock or intermediate-fork production stage;
stock 0.37.0 is a measurement control only.

| Fork patch | EffectCraft use | Decision |
| --- | --- | --- |
| E1 `viewport_hooks` | not used (craft-tablet only) | keep in fork; no app change |
| E2+E3 surface fit / wgpu error scopes | protects windows whose surface exceeds the adapter limit and reports configure/renderer failures; relevant to the app's "never crash" rule and its device-loss handling | **keep, benefit**; verify with a required-adapter `ui_gpu.rs` run and a device-loss test |
| E4 Wayland clipboard/PRIMARY | the app edits text in project rows, timeline fields, expression fields, Script UI and settings; Hyprland serves one data device per client | **keep**; requires winit W15 plus the new **W15P** PRIMARY path (§5.5/§8.7) |
| E5/E12/E13/E16 bidi and grapheme editing | UI labels and stock `TextEdit`s only; **text layers use `effectcraft-text`, not epaint** | **keep**; do not claim it fixes text layers |
| E6 IME focus | stock `TextEdit`s | keep |
| E7/E19 letter keys on non-Latin layouts | shortcuts (`menus::parse_shortcut`, `handle_shortcuts`); requires winit W25 | **keep**; manual platform test |
| E8 `TextEdit` glyph clipping | stock fields (Thai/Vietnamese/Arabic marks) | keep |
| E9 `Style::right_to_left` | app never sets it; its dock/layout is custom LTR | keep in fork; **do not enable** in EffectCraft (would mirror stock widgets inside an unmirrored custom layout) |
| E10 web named keys / Latin codes | web keyboard behaviour | keep; no app change |
| E11 vello_cpu multithreading | not used | keep in fork |
| E14 `FontTweak::fallback_align` | the app installs system fallback faces (`theme.rs:388-430`) | keep; **optional follow-up** to set it for CJK/Indic fallbacks, with screenshot proof |
| E15 `LineBreaker` | text layers use `effectcraft-text`; not adopted | keep in fork |
| E17/E18 `Context::set_language` / `Frame::set_ui_language` | screen-reader language and UI shaping language (the app has an interface language setting, `i18n::language`) | **adopt in the language stage** (fork-only; see §5.9) |
| E20 widget text language | goes with E17 | adopt with E17 |
| E21 `ImeEvent::DeleteSurroundingBytes` | the app has a **custom** viewer text editor that consumes IME events itself (`viewer_text.rs:397-508`) | **adopt in the IME stage**; stock 0.37 only has char-based `DeleteSurrounding` |
| **P2b** (new prerequisite) | async W15P PRIMARY routing in egui-winit: bounded read workers, owner-specific completion waker, no blocking UI path | **in P8**; gate in §5.5/§8.7 |
| **P7** (new prerequisite) | held-key pairing: the emitted egui key identity is stored by physical key and reused on release; clipboard shortcuts alone do not prove this | **in P8**; gate in §3.2/§5.5 |
| **P7b/E21b** (new prerequisite) | candidate egui-winit never calls winit W27's `set_ime_surrounding_text`; `IMEOutput` carries no logical text. The custom viewer editor is a required producer. | **in P8 before final acceptance**; see §5.5, §8.4 and `ime-surrounding-gap.md` |
| **P7c** (new prerequisite) | selected upstream fixes `#8763` (a button/menu command acting on a focused `TextEdit` selection) and `#8748` (hidden parent not updating deferred child viewports) | **in P8**; tested in §5.6/§5.8. Local workarounds are not the final unit. |

Removal conditions are the fork's own (`PATCHES.md`): each patch goes when an egui/winit release in
use contains it. EffectCraft must not add a second copy of any of these fixes locally.

### 3.2 Winit patches that matter here

| Patch | Why EffectCraft needs it | Gate |
| --- | --- | --- |
| W1 mixed-DPI rect | window placement and monitor hit tests (`apps/effectcraft/src/window.rs`) | Windows mixed-DPI manual test |
| W2/W5/W20/W21 X11 IME | the app's editable fields and expression editor | X11 IME under Xvfb+WM (qualified parts only; §8.7) |
| W3/W18/W19/W22/W24/W26/W27 IME | the custom text editor and stock fields; W26/W27 are required by E21 and by E21b/P7b | macOS press-and-hold/dictation, Wayland text-input-v3, Windows IME |
| **W15P** (new) | winit-owned Wayland PRIMARY on the existing connection/event queue, per-seat manager/device/source/offer from SCTK 0.19.2; no second regular data device | wlroots/Hyprland, KWin and Mutter capability matrix (§8.7) |
| W13 Wayland wheel phase | zoom/scrub shortcuts must not latch | Wayland manual test |
| W15 Wayland clipboard/files | needed by E4 | Hyprland clipboard check |
| W23 macOS Command keys | menu shortcuts | macOS layout test |
| W25 Latin layout | E19 shortcuts; preserve the emitted key identity by physical key across release | Russian/Arabic/Hebrew layout test plus a held Ctrl+Z/Cmd+K release after a layout/modifier change |

W27 also needs the lifecycle fixes recorded in `winit-review-triage.md`: clear the pending
preedit/commit/deletion batch on leave with an explicit epoch, test late `Done`, disable/reset, seat
removal and two windows, and preserve deletion-FIFO/commit pairing. Focus epochs and protocol
serials are distinct: a current-epoch `Done` with a mismatched serial still applies its incoming
edits exactly once and only defers outgoing surrounding state until a matching serial; a stale
(different owner/focus generation) batch is invalidated and discarded. `W25`/`W23`/`E19` must
preserve the emitted egui key identity by physical key across release (including layout/modifier
changes), clearing it on focus loss, IME interception and destruction, and test a non-clipboard
held key (Ctrl+Z/Cmd+K), repeat semantics and two physical keys mapping to one logical key.
W15 needs bounded, cancelable transfers with a 2 s total monotonic deadline and a 16 MiB total
byte budget; the egui adapter must not block the UI and must cancel stale pastes after
focus/target/generation changes.

### 3.3 craft-libs PR 199, CL-2 and language PR 198

- PR 199 is **merged** at `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`, the current reconciled
  shared-library implementation base. Do not merge its historical head `b68de8c5` (a stale tree
  relative to current `main`, missing `craft-platform` and reverting security/settings/docking/
  printer fixes) or audit merge `ec8840a` (stale lock, egui 0.36.2) as trees. The historical API
  port `8614e4a` and README `1764bca` on old `main` `4919088` are superseded by the merged base.
- EffectCraft pins `craft-ui` to the **reviewed CL-1 merge/rebase SHA on craft-libs `main` after the
  port**, not to the PR head. That revision must keep its egui dependency on registry `0.37` so
  EffectCraft's `[patch.crates-io]` (not a git URL patch) can redirect it.
- **CL-2 capture completion helper.** The shared plan's selected design is a bounded, cancelable
  per-request completion slot with a weak callback reference, aggregate image-byte and request
  limits, and an explicit viewport wake-up after the slot lock is released. EffectCraft is one of
  the eleven existing craft-ui consumers, so it pins **CL-1 + CL-2F in the same atomic dependency
  stage** as P1; only the DesignCraft/GridCraft pilots use the reviewed provisional CL-2 revision,
  and their adapters are reviewed before CL-2F freezes the API. Any consumer with an incompatible
  existing craft-ui bump must reconcile it in that same stage. EffectCraft does not pin the
  provisional revision. **No app-local result channel** and no generic UI compatibility layer.
  EffectCraft keeps its own request ids, paths, crop, deadlines and reply JSON.
- `craft-ui`'s production API used here (`docking::{Action, Zone, Placement, resolve_drop, zone_rect, drop_zone}`,
  `layout::{split_rect, SplitAxis, SplitSize, ResizeHandle, ResizeHandleStyle}`) is unchanged by
  the port; the additive `printing::tick` is not used by EffectCraft.
- PR 198 / the fork language work is adopted only where §3.1 says so. The app keeps its own
  catalogs (`i18n.rs`, `i18n/ui.rs`, `i18n/effects.rs`) and its own text engine; it does not adopt
  craft-text line breaking or a shared translation layer.
- Blocking: CL-1 and CL-2F must land before the EffectCraft dependency-pin PR can be reviewed
  honestly. Until then, implementation may use a local pinned SHA only for development, never in a
  publication branch, and never a machine-local path patch.

## 4. Root-`Ui`, lifecycle and multi-pass semantics

### 4.1 Current split (already on 0.36.2, must be preserved and completed)

Today `EffectcraftApp::logic` does GPU-failure application, language/theme/font install, FPS, control
drain, screenshot scheduling/collection (`lib.rs:1870-1903`), while `ui` calls `frame()` which does
**polls, playback, events, autosave, prefs, window title, close guard and the whole paint**
(`lib.rs:1905-1935`, `lib.rs:1614-1723`).

Verified integration facts (candidate source, same call sites as upstream 0.37):

- `eframe` runs `logic` and `ui` inside one `Context::run_ui` closure for every full pass
  (`eframe-0.37.0/src/native/epi_integration.rs:349-364`; web `app_runner.rs:289-292`). A full pass
  can run more than once per frame when `Context::request_discard` is used.
- While the root window is minimized/occluded, native eframe runs `Context::run_logic` with
  `App::logic` only (`epi_integration.rs:384-425`), unless a `Screenshot` or
  `RequestPaintWhileHidden` action is pending, in which case it runs a **full** UI pass with current
  input and paint (`wgpu_integration.rs:680-740`).
- `prepare_raw_input` calls `raw_input_hook` **before both** paths
  (`epi_integration.rs:414-428`). `update_logic_only` then stores the hook-mutated raw input in
  `pending_raw_input` for the next full pass (`epi_integration.rs:400`). Therefore synthetic
  events fed during a logic-only call are retained, not lost, and are applied exactly once by the
  next full pass.
- Native eframe throttles repaints of an invisible/minimized window to at least 100 ms
  (`eframe-0.37.0/src/native/run.rs:27,376-383`), so a hidden gesture or capture takes a few
  throttled full passes; the app's deadlines must allow for that and must not infer failure from the
  first 100 ms.
- `ViewportInfo::visible()` derives from `minimized`/`occluded`, not from a paint request. It stays
  `Some(false)` during a forced hidden UI pass, so visibility cannot distinguish "logic only" from
  "forced hidden paint".
- `run_logic` refreshes only `raw.viewports` and `raw.focused` (`egui-0.37.0/crates/egui/src/context.rs:1190-1218`);
  it leaves `InputState::time`, events and pointer state at the previous pass's values. eframe sets
  `RawInput::time` in `prepare_raw_input`, but `run_logic` does not copy it into `InputState`.
- `Context::request_screenshot` sends a viewport command, which requests a repaint, and a pending
  screenshot is a "wants paint" action even when hidden. The callback itself does not request a
  repaint when the pixels arrive.

Consequences, all pre-existing in 0.36.2:

- With the window minimized/occluded, job polling, playback advance, audio pumping, auto-save
  ticks, engine-event draining, the unsaved-close guard and the window-title update **do not run**
  on ordinary logic-only ticks.
- A discarded pass re-runs the `run_ui` closure, so `logic` runs again for the same frame.
  `Grid` requests a discard on first show (`egui-0.37.0/crates/egui/src/grid.rs:468`); EffectCraft
  has 18 `Grid::new` sites across dialogs/settings/forms.
- The control channel's `AfterInput` replies are completed in `logic` as soon as `synthetic` is
  empty, which is before the pass applies the last synthetic event (`lib.rs:1893-1901`,
  `lib.rs:1921-1930`). While hidden, an event can also have been fed into retained raw input with
  no pass yet applied.

### 4.2 Target boundaries

Introduce one explicit app tick and one draw pass; do not add a generic `Context` bridge:

```rust
// EffectcraftApp
now: f64,          // monotonic app clock value for this tick, used for deadlines
focused: bool,     // last known focus, refreshed from viewport info
last_input_at: f64,
last_draw_geom: Option<(egui::Rect, f32)>, // last non-discarded root draw: screen rect + ppp
geometry_epoch: u64,   // bumped whenever that screen rect or ppp changes
want_paint: bool,
clock: AppClock,   // real or injected (§4.3)

/// The only non-paint entry. Effects are idempotent; time may advance slightly per call.
pub fn tick(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame);

/// Paint only. Receives the root Ui from eframe.
pub fn draw(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame);

// eframe::App
fn raw_input_hook(&mut self, ctx, raw) { /* feed synthetic, pointer wrap; no clock, no replies */ }
fn logic(&mut self, ctx, frame) { self.tick(ctx, frame) }
fn ui(&mut self, ui, frame) { self.draw(ui, frame) }
```

`tick` must contain, in this order:

1. `init_gpu(frame)` (moved from `ui`, `lib.rs:761-806`), so the compositor exists before the first
   paint and while hidden; `apply_gpu_failure` + `close_after_device_loss` (moved from `ui`,
   `lib.rs:1905-1910`).
2. `i18n::set_current(language)`, theme/font install and `apply_prefs` (moved from `frame`).
3. Engine/background polling: `poll_render`, `poll_track`, `poll_mask_track`, `poll_jobs`,
   `poll_offload`, `poll_warp`, `poll_roto`, `poll_camera` (`lib.rs:1617-1655`), plus
   `frames.set_context(ctx)` so worker completions can request repaints while hidden. These are
   queue-draining and a second call in a discarded pass is a no-op (e.g. `render_queue.rs:579-583`
   takes the update queue).
4. `run_script_tasks`, `handle_events` (engine queue drained once), `command_inbox`; toast/status
   timestamps use `now`, not `ctx.input(|i| i.time)`.
5. `run_dialog_answers` and the native-menu answers.
6. `tick_autosave` (already wall-clock based, `lib.rs:1790-1816`) with the app clock, watch folder,
   `tick_footage_reload` using `self.focused`/`self.now`/`self.last_input_at` instead of `ctx.input`.
7. Control: `drain_control` with `now`, `finish_job_waiters` with a bounded repaint.
8. Screenshots: `collect_screenshots` (drain completion slots and validate each image against the
   geometry + epoch recorded at issuance, §5.7), then `issue_screenshots` (`now`), then timeouts
   and cancellations.
9. Playback: `advance_playback(ctx, scale, now)` using the last drawn viewer scale;
   audio pump; `cache_when_idle`.
10. Unsaved close guard (`panels/unsaved.rs:50-62`) and window-title update.
11. `prefs_live::memory_watch` / `home_on_open` (split out of `prefs_live::frame`,
    `prefs_live.rs:15-19`); `video_preview` stays in draw.
12. If `want_paint`, or synthetic input/`input_waiters` remain, or a control step needs a UI pass,
    send `ViewportCommand::RequestPaintWhileHidden` and clear `want_paint`. This also covers
    playback with an open Video Preview or floating panel window (§5.8).

`tick` never reads input events, pointer state or modifiers for decisions. `draw` keeps only paint
and direct manipulation: dock layout, panels, menus, dialogs, areas, `ui.inspect` registry, pointer
hover, shortcuts that read events, `finish_native`, `video_preview`, dropped-file overlay/import,
Home thumbnail capture, toast, workspace-state save on pointer-up, and `frames.pump` on wasm (or
move `pump` to `tick`; either is acceptable if tested, but it must not be the only progress path).
`draw` updates `last_input_at` from the pass's input activity and records
`last_draw_geom = Some((ui.ctx().screen_rect(), ui.ctx().pixels_per_point()))` at the end of every
pass that is not discarded (`!ui.ctx().will_discard()`), bumping `geometry_epoch` when the rect or
ppp differs from the previous value. Screenshot issuance copies this geometry + epoch; completion
rejects/retries if the epoch changed before the save (§5.7). The callback itself supplies only
pixels, so the issued record — not callback timing — is the geometry anchor.

**No `Context::root_ui` anywhere.** Areas/Windows/Modals keep taking `ctx` (they create their own
layers); panels receive the existing `&mut Ui`. `show_viewport_immediate` stays as is.

### 4.3 App clock, focus and input gating

- **The clock is app-owned and injected.** Neither `ctx.input(|i| i.raw.time)` nor
  `ctx.input(|i| i.time)` is valid in a logic-only tick: `run_logic` copies only viewports/focused
  (`context.rs:1190-1218`), so `i.raw.time` is the previous full pass's raw value and `i.time` is
  the previous pass's `InputState::time`; kittest leaves `RawInput::time` `None`.
  ```rust
  pub struct AppClock {
      source: ClockSource, // Real(web_time::Instant) | Manual(f64)
  }
  impl AppClock { fn now(&self) -> f64 { /* monotonic seconds */ } }
  let now = self.clock.now();
  ```
  `tick` stores `self.now = now` before using it. Tests inject `AppClock::manual()` and advance it;
  a regression must show that a stale `ctx.input(|i| i.time)` cannot satisfy a deadline. Native and
  web use the same `web_time::Instant` source; `EffectcraftApp::now()` exposes the last value so
  `WebApp::logic` can pass it to `persist::snapshot` (§8.6).
- Focus comes from the fresh viewport copy with a visible-pass fallback:
  ```rust
  let focused = ctx.input(|i| i.viewport().focused)
      .or_else(|| ctx.input(|i| i.focused));
  self.focused = focused.unwrap_or(self.focused);
  ```
  Do not read `i.events`/pointer in `tick`; input handling belongs to `draw` (the known UI phase).
- **Synthetic input is not gated on visibility, but is gated on an unapplied event.**
  `raw_input_hook` runs once per eframe prepare (frame or logic call), not per egui UI pass, and
  calls `self.feed_synthetic(raw_input)` then `pointer_wrap::prepare_input(...)`. In a logic-only
  call eframe retains the mutated raw input
  (`pending_raw_input`) and calls the hook again on the next call; in a forced hidden full pass it
  is applied immediately. Gating on `visible() == Some(false)` would skip feeding in both cases
  and leave the gesture (and its reply) pending forever.
  `feed_synthetic` drains **one pointer event per call** (`lib.rs:578-591`), so repeated
  logic-only updates before the forced full pass (eframe's overdue-`RedrawRequested` path and its
  100 ms invisible-window throttle) would append press, move and release to the same retained
  input and collapse a gesture into one pass. The hook must therefore set a guard when it moves a
  synthetic event into raw input and skip feeding while that guard is set; the guard is cleared
  when a known UI pass has applied that step once, even if layout is later discarded;
  record that application separately from screenshot settling/accepted-frame counters.
  RawInput events are consumed on the first pass and are not replayed in later passes.
  Never clear the guard in `tick`; never wait for the event to reappear in an accepted pass.
  The next step is fed only on a subsequent host input preparation. No event is discarded. §8.3 tests repeated logic-only ticks with a drag and a click.
- `raw_input_hook` performs no reply, no clock read and no side effect other than pointer-wrap state,
  the synthetic-pending guard and merge-key closing. `pointer_wrap::prepare_input` returning "gesture ended" clears
  `session.history.merge_key`; doing that during a logic-only call is safe because the events are
  retained and the same reset is idempotent.
- Replies for `AfterInput` are completed **only at the end of a non-discarded `draw` pass**
  (after the pass, check `!ui.ctx().will_discard()`) that ran after the last synthetic event was
  fed, and never in `tick`. A hidden request therefore replies only after the forced pass applied
  it, and a pass that egui throws away cannot produce an early success reply.
- If a control request needs a UI pass (synthetic input, a panel-window change, or a caller that
  explicitly wants fresh `ui.inspect`/`ui.elements` data), `tick` sends `RequestPaintWhileHidden`.
- `control::handle` is called from `tick` with `now` passed in (a signature change, not a wrapper):
  replace its `ctx.input(|i| i.time)` at `control.rs:428` with `now`, and `inspect`'s
  `windowFocused` at `control.rs:562` with `ctx.input(|i| i.viewport().focused)`. `ui.inspect` and
  `ui.elements` keep last-painted semantics.

### 4.4 Multi-pass and idempotence

A discarded pass re-runs `tick` and `draw` with the same raw input (events are taken by the first
pass). Rules:

- `tick` effects are idempotent for a replayed pass: polls drain queues; auto-save checks a
  timestamp; `advance_playback` computes the target frame from the plan and the monotonic `now`
  rather than incrementing, so a replay with a slightly later `now` does not double-count
  `Playback::frame_shown` (`lib.rs:230-246`); screenshot scheduling advances only when `now`
  passes a settle deadline, never once per pass (the old frame counter must not be replayed);
  job-waiter completion is keyed by the pending job, not by pass count.
- One-shot side effects in `draw` must rely on egui's per-pass input (`Response::clicked`,
  `drag_stopped`) and command merge keys, not on layout-pass counts. High-risk paths are viewer
  drags (`viewer.rs:1674-1874`), timeline drags (`timeline.rs:1500-1700`), painting strokes
  (`layer_panel.rs:350-360`) and form toggles inside `Grid`s.
- Add a forced-discard regression test (§8.3) that proves one undo step and one command effect
  across a real two-pass frame.

## 5. Concrete refactors with anchors

### 5.1 Manifests, features, pins and lock checks

Root `Cargo.toml` (final publication form; placeholders are filled only after the reviewed forks
exist). P1 waits for the complete reviewed/published storytold egui P8 (E1–E21, P2b, P7, P7b/E21b,
P7c and any justified perf fix), the completed winit (W1–W27, W15P, W27 lifecycle) and craft-ui
CL-1 + CL-2F. It includes the custom-editor IME payload adapter atomically. Stock 0.37.0 and
aiongg candidates remain comparison controls, never intermediate production stages.

```toml
craft-ui = { version = "0.1.0", git = "https://github.com/storytold/craft-libs", rev = "<reviewed main SHA including CL-1 and CL-2F>", features = ["serde"] }
egui = { version = "0.37", default-features = false, features = ["default_fonts", "monochrome_emoji_fonts"] }
eframe = { version = "0.37", default-features = false }
egui_kittest = { version = "0.37" }

[patch.crates-io]
winit = { git = "https://github.com/storytold/winit", rev = "<final reviewed winit SHA incl W15P>" }
egui = { git = "https://github.com/storytold/egui", rev = "<final reviewed egui P8 SHA incl E21b + P7c>" }
eframe = { git = "https://github.com/storytold/egui", rev = "<same egui SHA>" }
egui-winit = { git = "https://github.com/storytold/egui", rev = "<same egui SHA>" }
egui-wgpu = { git = "https://github.com/storytold/egui", rev = "<same egui SHA>" }
egui_kittest = { git = "https://github.com/storytold/egui", rev = "<same egui SHA>" }
hayro-syntax = { git = "https://github.com/storytold/craft-libs", rev = "ca50d63b773c99f96e75c77ce5a1d21e7c9cda0d", version = "=0.7.2" } # keep
```

`epaint`, `epaint_default_fonts`, `ecolor` and `emath` are path dependencies of the git egui crates
and must resolve from that same git revision. `aiongg/*` SHAs are comparison inputs only and must
not appear in a publication lock.

Notes:

- `craft-ui` is a git dependency from `craft-libs`; redirecting it with
  `[patch."https://github.com/storytold/craft-libs"]` is only acceptable for development, never in a
  publication branch. The publication form is the real `rev`.
- Do not patch `egui_plot`, `egui_dock`, `egui_extras` (unused) or `egui_system_font_provider`
  (unused while `system_fonts` is off).
- The fork's own `[patch.crates-io] winit = { branch = … }` belongs to the fork workspace and does
  **not** propagate; the consumer patch above is mandatory.
- Feature additions in the app manifests:
  - `crates/ui-egui/Cargo.toml:25`: `eframe` features += `clipboard`.
  - `apps/effectcraft/Cargo.toml:18`: `eframe` features += `clipboard`.
  - `apps/effectcraft-web/Cargo.toml:30`: keep `default_fonts`, `wgpu` (web has no OS clipboard
    feature; browser clipboard events are separate).
  - `egui_kittest` dev features: `eframe`, `wgpu`, plus `snapshot` only if §8.4's pixel goldens are
    adopted. `egui_kittest` depends on `egui` with `default-features = false`; only kittest's own
    `[dev-dependencies]` enable `default_fonts`/`monochrome_emoji_fonts`, and a dependency's
    dev-dependencies do **not** propagate. The workspace `egui` features above are therefore what
    EffectCraft's library and integration-test builds receive; do not rely on kittest to enable
    fonts. If a test root resolves `egui` independently, it must enable the font features itself
    or install controlled fonts, and the actual graph is checked per production/test root.
  - `monochrome_emoji_fonts` is enabled at the workspace `egui` dependency so `✔`/`▣` survive
    (§2.3.2). It propagates into `effectcraft-web`; record the raw and compressed WASM size change
    and either accept it with a budget or introduce a native-only `ui-egui` emoji-fonts feature
    after measuring (§9).
  - `system_fonts` stays **off** initially: the app already installs language-driven system
    fallbacks (`theme.rs:388-430`) and scans fonts for its menus; enabling eframe's fontique
    provider would add startup cost (`#8512`) and make rendering machine-dependent. Revisit as a
    separate product change for colour emoji.
- `winit` direct dependency in `apps/effectcraft/Cargo.toml:23` must match the patched fork (it
  does: `0.30`).
- Lock/duplicate checks after every pin change:
  ```sh
  cargo tree -d
  cargo tree -i egui@0.37.0
  cargo tree -i winit@0.30.13
  cargo tree -e features --target x86_64-unknown-linux-gnu
  cargo tree -i wasm-bindgen --target wasm32-unknown-unknown
  cargo check --locked
  cargo test --locked
  cargo metadata --locked --format-version 1 --no-deps
  ```
  Acceptance: exactly one egui-family source (the final storytold revision) covering `egui`,
  `eframe`, `egui-winit`, `egui-wgpu`, `egui_kittest`, `epaint`, `epaint_default_fonts`, `ecolor`
  and `emath`; exactly one winit (the final storytold revision); no registry `egui`/`epaint`
  beside it; no `path = "…"` patch and no branch dependency; a clean checkout fetches every
  approved revision and passes the checks with `--locked`.
- `wgpu` stays `30.0.1`; egui 0.37 accepts `30.0` and already resolves 30.0.1 here. The pinned
  `wasm-bindgen = 0.2.129` must stay consistent with `wasm-bindgen-futures`; check the wasm-target
  inverse tree above.

### 5.2 IDs and scopes

- Mechanical `Id::new(` → `Id::unique(`: **626 occurrences / 616 lines** (`601`/`591` in source,
  `25` in tests; 59 source and 12 test files). Count by occurrence and file, not only by line.
  `GlyphId::new` in `crates/text`/`crates/pdf` (7 lines) is unrelated and must stay. Values are
  identical because `Id::new` delegates to `Id::unique`
  (`egui-0.37.0/crates/egui/src/id.rs:101-104`); persisted workspace state
  (`workspace_state.rs`, **version 1**) and project JSON store no egui IDs, so no migration is
  needed.
- `color_picker.rs:335,337`: `ui.id()` → `ui.scope_id()`.
- `widgets.rs:24`: `s.id` → `s.scope_id` (the panel's stable scope; do not use `unique_id`, which
  changes with layout).
- `widgets.rs:492-497`: make `label_swatch_id() -> egui::IdSalt` = `IdSalt::new("label-swatch")`;
  `Atom::custom(salt, …)`; `WidgetAtomResponse::rect(salt)`.
- `menu_keys.rs:84`: `find_menu_root(ui).scope_id`.
- No new ID scheme; add a test that the same source yields the same `Id` across frames and that two
  different payloads differ. Do not derive IDs from translated text (already true).

### 5.3 Roles, names and accessibility

Migrate the 19 source `WidgetInfo` sites with the table in §2.2, plus `ui_agent_audit.rs:73` in the
test (`.role`). Specific choices:

- `color_picker.rs:19`: `Role::ColorWell`, label = hex (existing screen-reader contract).
- `color_picker.rs:336`: `Role::Unknown` (a painted swatch, not an input).
- `color_picker.rs:338`: `Role::Button` ("Original color").
- `color_picker.rs:271,300`: `Role::Slider` (labels stay).
- `effect_controls.rs:1274`, `text_panels.rs:700`: `WidgetInfo::selected(Role::Button, enabled,
  selected, label)`.
- `panels/project.rs:613`: use `WidgetInfo::labeled(Role::TreeItem, …)` directly, but **keep** the
  `accesskit_node_builder` at `project.rs:614-620` for `selected`, `level` and `expanded`
  (`WidgetInfo` cannot express level/expanded); remove only its `set_role(TreeItem)` line.
- `project.rs:781`: `Role::Unknown` (comment).
- `dock_ui.rs:367`: `Role::Splitter` ("Resize floating panel") instead of `Button`.
- `widgets.rs:497`: `WidgetAtomResponse`.

Naming work for kittest's accessibility check:

- **Strict checks stay on from the first green PR.** There is no shared test harness to flip:
  `Harness::builder()` appears at 180 call sites in 78 test files, and
  `HarnessBuilder::default()` sets `check_accessibility: true`
  (`egui_kittest/src/builder.rs:51`). A new helper would not reach existing direct builders, and a
  per-builder `with_accessibility_check(false)` would only hide failures. P1 therefore adds the
  actual names until every test passes with strict checks; a repository grep gate forbids
  `with_accessibility_check(false)` (§8.2). No global or blanket opt-out exists at any stage.
- Stock fields: give each `TextEdit` a `hint_text` or `accessible_name`, each `DragValue`/`Slider`
  an `accessible_name`. Known call sites: `TextEdit` (39) in `precomp.rs:117`, `rq_templates.rs:137`,
  `scriptui_view.rs:145`, `timeline.rs:1507,2066,2458,3516`, `project.rs:704`,
  `shortcut_editor.rs:276,440,532`, `markers_ui.rs:119`, `text_panels.rs:607`,
  `settings.rs:174,607`, `effect_controls.rs:1214`, `forms.rs:786,794,806`, `dialogs.rs:471`, plus
  the rest from `grep -rn TextEdit:: crates/ui-egui/src`; `DragValue`/`Slider` (51) in
  `layer_styles_dialog.rs`, `gradient_editor.rs`, `settings.rs`, `key_dialogs.rs`, `forms.rs`,
  `dialogs.rs`, `dialogs_3d.rs`, `viewer_tools.rs`, `scriptui_view.rs`.
- For rows whose label is a separate widget, use `Ui::label_inputs_by(label_id, |ui| editor)` —
  e.g. the Grid rows at `forms.rs:771`, `settings.rs:596`, `key_dialogs.rs:142,207,251`,
  `markers_ui.rs:133`, `rq_templates.rs:56,100`, `layer_styles_dialog.rs:214`.
- For rows whose label is painted (Timeline property rows, Effect Controls rows), use
  `Ui::name_inputs(label, |ui| editor)` or `Response::accessible_name(label)` where the label is
  already known. This covers `timeline.rs:2458,3516` and `effect_controls.rs:1214`.
- Custom widgets have no `WidgetInfo` today; add a small local helper, not a compatibility layer:
  ```rust
  fn named(resp: egui::Response, role: egui::Role, name: impl Into<String>) -> egui::Response {
      resp.widget_info(|| egui::WidgetInfo::labeled(role, resp.enabled(), name.into()));
      resp
  }
  ```
  Use it first for the highest-value controls: dock tabs (`Role::Tab`, selected state,
  `dock.rs:1149-1280`), active/close buttons, the dock split handle, numeric scrub fields
  (`widgets.rs:53-160`), the color field/slider (`color_picker.rs`), the error bar prev/next
  buttons (`expr_bar.rs:59-70`), and the floating-panel resize grip (`dock_ui.rs:367`). Do not
  blanket-wrap `ui.interact`: a node without a real name/role is worse than no node.
  The helper stays private to `effectcraft-ui-egui`; there is one consumer (this app), so it is not
  proposed for `craft-ui` and is not a generic compatibility layer.

Accessibility assertions (§8.4) must include roles, `selected`, enabled state, names, and the
AccessKit language once §5.9 lands. The app's `ui_agent_audit.rs` (automation coverage) stays
separate: an automation id is not an accessible name. `docs/gaps.md:227` records that the project
has done no accessibility work; this stage is a first, tested step (visible stock controls and the
highest-value custom controls), not a claim of screen-reader parity.

### 5.4 Fonts, glyphs and text

- Enable `egui/default_fonts` and `egui/monochrome_emoji_fonts` in the workspace dependency (see
  §5.1) so `FontDefinitions::default()` at `theme.rs:334` keeps the 0.36.2 font set. Do not rely on
  eframe feature unification for correctness.
- Keep the app's two-pass font warm-up (`styled`/`fonts_ready`, `lib.rs:1876-1890`,
  `lib.rs:1905-1911`): 0.37 may apply `set_fonts` immediately when no text has been laid out
  (`#8602`, `#8609`), but relying on that would change when the first frame draws and would make
  headless tests depend on pass timing. Do not use `run_logic` to install fonts in a UI test.
- `theme.rs:732`: `d.font.as_ref() == bytes` → `d.bytes() == bytes` (and keep `d.index`).
- `widgets.rs:329`: `TextEdit::vertical_align(Align::Center)` → `TextEdit::align(Align2::LEFT_CENTER)`
  (the old call only changed the vertical component; the horizontal default was LEFT).
- Update the comment at `theme.rs:354-360` ("egui … has no bidirectional layout"): the suite fork
  shapes RTL runs, but text layers still use `effectcraft-text` and are unaffected. Add a UI-label
  RTL test (Arabic/Hebrew `Label`/menu text has visual-right-to-left run order) and keep the
  text-layer RTL test in `crates/text/src/layout.rs:1419` as the text-engine contract.
- Keep `system_fonts` off. If colour emoji is wanted later, it is a separate change with a startup
  measurement and cross-platform screenshots, not part of this migration.
- Fallback registration stays as is (`theme.rs:388-430`), including `FontData::from_owned` and
  `data.index = face.info.index`; `FontData` still accepts that construction in 0.37. Optional
  follow-up: E14 `FontTweak::fallback_align` for the installed system faces, with CJK/Indic
  screenshots before and after.
- Extend the existing glyph-coverage test (`theme.rs:700-716`) to the full chrome symbol set used by
  the app, including `▣`, `✔`, `✓`, `⚠`, `▶`, `◀`, `▸`, `●`, `○`, `…`, `×`, `›`, `•`, `·`, `−`,
  `←/→/↑/↓`, `⌘`, `⇧`, `⌥`, `⌃`, `⏎`, `↵`, `′`, `″`, in Proportional, Monospace, `medium` and
  `semibold`. This is the regression test for finding 2.3.2.
- Tests that deliberately render scripts with no installed face opt out explicitly **at that test
  builder**: `ui_effect_names.rs:35` (the builder inside `harness()` at line 20, used by the
  `zh-hans` tests at 68, 81, 107 and 127) and `ui_presets_folder.rs:40` (`ja`; the builder is inside
  the test) use `Harness::builder().allow_missing_glyphs()` with a comment naming the script. No
  global opt-out. Test fonts come from the workspace `egui/default_fonts` +
  `monochrome_emoji_fonts` features (§5.1), not from `egui_kittest`'s non-propagating
  dev-dependencies.
- Headless text rendering is not proof of native font fallback/IME; native platform checks remain
  (§8.7).

### 5.5 Input, IME, clipboard and PRIMARY

- `control.rs:371`: `Event::MouseWheel { …, source: egui::MouseWheelSource::Wheel }`; the same in
  the 11 test constructions (§2.1). Use `Wheel`, not `Unknown`, so synthetic scrolls are not
  treated like momentum/rubber-band input.
- `viewer_text.rs:397-508` (custom text editor): handle stock
  `egui::ImeEvent::DeleteSurrounding { before_chars, after_chars }` by converting the caret
  position to a char range and issuing `text.delete { range, merge: <same key> }` before the
  following `ImeEvent::Commit`, so replacement is one undo step. Native winit emits the fork's
  `DeleteSurroundingBytes { before_bytes, after_bytes }` (E21/W26/W27); add that arm in the
  language/IME stage, converting UTF-8 byte offsets at char boundaries via `TextDoc::text`,
  `TextDoc::char_of_byte` and `TextDoc::byte_of` (`crates/keyframe/src/text_doc.rs:527-545`).
  Regression: preedit + delete-surrounding + commit replaces exactly the intended characters and
  adds one undo step; press-and-hold/dictation on macOS is verified manually (`W26`).
- **Incoming composition lifecycle (viewer_text).** Each viewport stores
  `ImeComposition::Active(owner)`, `TerminalPending(owner)` or `Stale`, where the owner is
  `(layer, edit session)` and excludes caret, selection, text, revision and geometry. A target
  switch or `text.endEdit` invalidates both live states; the `Stale` tombstone rejects every
  pre-edit/delete/commit from the replaced owner. An empty pre-edit clears the visible composition
  but records `TerminalPending(owner)`: the visible pre-edit ended, while its terminal commit or
  delete-surrounding is still pending. A switch before that terminal event therefore still rejects
  it rather than applying it to the replacement. A non-empty old pre-edit is treated as a continuing
  update of that old composition and keeps the tombstone across arbitrary idle frames. A rejected
  terminal commit is the composition boundary and consumes the tombstone immediately, so a new
  pre-edit later in the same raw input batch is accepted for the current target. A rejected empty
  pre-edit or delete-surrounding is only tentatively terminal: it keeps the tombstone through the
  rest of the batch so a paired old commit is rejected too, and is consumed at batch end when no
  commit follows. Tests must separate the late pre-edit from its commit/delete by at least three idle
  frames, exercise an observed empty pre-edit followed by a target switch and a delayed old commit,
  verify a new pre-edit after a rejected terminal commit in the same batch commits to the new
  target, verify a genuinely new composition after the consumed terminal batch, and verify that no
  permanent lock remains. Do not replace this with an elapsed-frame guard: time is not causal
  ownership.
- **E21b/P7b outgoing surrounding text (new prerequisite, not existing behaviour).**
  Source inspection at the candidate revision found no call to winit's
  `WindowExtWayland::set_ime_surrounding_text` anywhere under `egui-winit`, and `IMEOutput`
  (`egui/src/data/output.rs:100-115`) has only purpose/rect/cursor rect/interrupt. Deletion and
  commit handling alone therefore cannot pass the suite's surrounding-text contract.
  - The fork adds a bounded transient `IMESurroundingText` payload to `IMEOutput` (logical text,
    cursor/anchor as UTF-8 byte offsets, **owner/context generation identity**). It is built from
    committed text only, excludes preedit, is skipped from debug/persistence output, and is per
    IME-owning widget/viewport. The owner generation is present even when support is unavailable;
    **available-empty is distinct from unavailable** and is sent as an explicit empty payload,
    never as an absent one. Changing owner/support (normal, password, unavailable) clears the
    backend cache and uses the protocol's committed disable/enable boundary; an empty text payload
    is not a general clearing API.
  - The **final completed pass** owns the outgoing value, including `None`: the adapter must not
    use `ime.or(self.ime)`-style carry-over that can retain an earlier pass's text. `egui-winit`
    calls W27 on text/selection/focus changes; per-window caching cannot bleed between widgets or
    windows. The excerpt is at most 4,000 UTF-8 bytes cut at valid char boundaries. A selection
    that cannot fit returns an explicit **unavailable** context (or an explicit empty one
    when the document really is empty), never a truncated "complete" selection.
  - A current-epoch `Done` whose protocol serial does not match still applies its incoming edits
    **exactly once** and only defers outgoing surrounding state until a matching serial. A stale
    (different owner/focus generation) batch is invalidated and discarded. Track focus epochs and
    protocol serials separately.
  - **No new document-sized allocation.** Building the new bounded payload must not clone the full
    document; the baseline `TextEdit` already clones whole buffers for change detection/undo, and
    that pre-existing cost is measured separately.
  - **EffectCraft custom adapter (viewer_text).** The producer at `viewer_text.rs:568` must supply
    the logical model text from the edited layer's `TextDoc` (committed text), not display-bidi
    text, with preedit excluded (`viewer-text-preedit` stays a display-only temp value), plus the
    editor's owner generation unconditionally. Convert `TextEdit.anchor/caret` (character
    indices, `crates/engine/src/commands/text_edit.rs:18-40`) to UTF-8 bytes with
    `TextDoc::byte_of`, clamp at char boundaries, excerpt to 4,000 bytes, and remap cursor/anchor
    to the excerpt. Emit an explicit **empty** payload for an available empty document; emit
    **unavailable** (and trigger the committed disable boundary) when there is no edit session,
    when focus leaves the viewer, or after `text.endEdit`/Escape/click-away, so stale text cannot
    survive. The editor has no password mode; the stock-field password/unavailable clears are
    tested there. The payload is scoped to the viewport that owns the focused editor (floating
    panel windows and the main window cannot share state).
  - **Atomic compatibility.** `IMEOutput` loses `Copy` when the payload is owned. The P8 fork SHA,
    all egui-family crates, the `craft-ui` revision and EffectCraft's single `IMEOutput` literal
    (`viewer_text.rs:568`; there are no destructures in this repo) must change in one dependency
    unit. Do not land a mixed stock/fork lock, and do not claim a stock-0.37 app build after this
    stage: the literal would not compile against stock `IMEOutput` (§11.13).
  - Tests: outgoing text immediately after delete+commit and deletion-only batches; scalar/
    grapheme vs byte indices; supplementary emoji, combining marks and RTL logical order; preedit
    exclusion and caret movement; long selection/excerpt limit and allocation bound; same-window
    field switch; two windows; focus loss and late serial; current-epoch mismatched serial applies
    incoming edits once and defers outgoing; stale-epoch discard; available-empty vs unavailable
    and normal/password/unavailable transitions; final-pass `None` authoritative; multi-pass
    final output; native Wayland Vietnamese/predictive IME protocol trace. macOS `NSRange`
    conversion remains a separate W26 path; this does not claim new macOS surrounding-text APIs.
- Clipboard: `eframe/clipboard` feature (§5.1). `ctx.copy_text`/`output.commands` paths at
  `lib.rs:1600-1612`, `lib.rs:1946-1953`, `viewer_text.rs:413-430`, `menus.rs:1867-1906` and the macOS
  `Hooks::clipboard_text` (`lib.rs:159-160`, `apps/effectcraft/src/main.rs:326`) stay unchanged.
- **PRIMARY (E4 + W15P) is a native gate, not one manual row.** Upstream 0.37 fills PRIMARY from
  `Label`/`TextEdit` selections and pastes on middle click (`#8428`); the reviewed fork replaces
  the arboard fallback with a winit-owned PRIMARY path on the existing Wayland
  connection/event queue, using SCTK 0.19.2's `PrimarySelectionManagerState` and per-seat
  device/source/offer objects. Never create a second regular `wl_data_device`. `set_primary` /
  `get_primary` route through it; password selections are never offered; an unchanged selection
  does not reclaim ownership. Serial handling accepts keyboard **and** pointer press serials for
  the focused seat with origin, freshness and epoch (do not invent a keyboard-only rule — the
  protocol describes pointer-driven selection). **Claim the selection synchronously while the
  triggering serial is fresh**; only serving the selection is asynchronous. Reads are bounded:
  a 2 s total monotonic deadline, 16 MiB total bytes, at most 4 read workers and 2 writers with
  a bounded queue of 8. A live owner-specific completion waker wakes the owning viewport after the
  slot lock is released and never blocks the UI; canceled or stale results are discarded, and a
  stale paste is canceled after focus/target/generation changes.
  The native capability matrix in §8.7 covers wlroots/Hyprland, KWin and Mutter; ordinary paste is
  not PRIMARY acceptance. An isolated PRIMARY-only fallback requires measured necessity and review.
- Non-Latin layout shortcuts (E7/E19/W25): headless tests cannot exercise winit's mapping. Add a
  manual row (Russian, Arabic, Hebrew) that checks Menu shortcuts and the timeline/Project Ctrl+C/V/X
  paths, and keep `parse_shortcut` tests (`menus.rs:2229-2265`) as the app-level contract. Use a
  non-clipboard held-key case (e.g. Ctrl+Z) as well, because clipboard shortcuts return before
  emitting Key-down.

### 5.6 Menus, popups and docking

- `menus.rs:2025-2050`: keep `MenuBar::new().config(MenuConfig::new().style(menu_style)).ui(...)`;
  0.37 keeps `MenuConfig` and adds `submenu_gap`. Regression tests: hover-switch between top-level
  menus, arrow navigation (`menu_keys::Nav`), opening a submenu, Escape/outside-click closing, and
  a menu command that acts on the focused `TextEdit`'s selection (`#8763`). That last case is P7c's
  selected fix in the P8 revision, not an app workaround.
- `menu_keys.rs:84`: scope id (see §5.2). `menu_keys` also uses `ui.is_sizing_pass()` for popup
  sizing (already); keep and test that a sizing pass does not invoke a command twice.
- Dock: the layout and drawing are app-owned (`dock.rs`, `dock_ui.rs`); only `craft_ui::docking`
  actions are shared. Preserve panel order/nesting, clip rects, split sizes and IDs. 0.37's
  "Panel/Scene clip rect respects parent" (`#8562`) needs a clip test for a maximized panel and an
  embedded (web) floating panel.
- Root Ui order (`lib.rs:1614-1723`): menu bar strip, header, dock body, native drop overlay,
  mini flowchart, dialogs, ScriptUI windows, coach, import card, toast. There are no side
  `Panel::left/top/...` calls; the dock is an explicit child Ui with computed rects. Keep that order
  and nesting (it decides hover/layer order and clip rects). `CentralPanel` is used only inside
  floating-panel and video-preview viewports (`dock_ui.rs:475`, `prefs_live.rs:207`).
- Do not enable `Style::right_to_left` (E9); the dock and viewer are custom LTR geometry.

### 5.7 Screenshots (control channel, kittest, hidden windows)

Replace token-via-input-event correlation with the shared CL-2 completion slot (§3.3). Do not add an
unbounded or app-local result channel: the slot is owned by the app request record, and the
callback holds only a weak reference to it.

```rust
struct PendingShot {
    token: u64,
    path: Option<String>,
    crop: Option<[f32; 4]>,          // logical points, as the control protocol already takes
    reply: Sender<Value>,
    deadline: f64,                   // app clock
    admitted: (egui::Rect, f32, u64), // screen rect + ppp + geometry epoch at acceptance
    issued: Option<(egui::Rect, f32, u64)>, // target-UI rect + ppp + geometry epoch at issuance
    slot: Arc<capture::CompletionSlot>, // CL-2F helper type (API frozen)
    reserved_bytes: usize,
}
```

- **Admission.** Use the shared default policy: at most **4 outstanding requests**, **128 MiB per
  image** and **256 MiB aggregate** reserved in-flight/completed bytes. Stricter app caps are
  allowed, but never looser. Estimate the target surface with checked `width * height * 4` from the
  known UI geometry, reject over-budget requests before issuing a capture with an actionable
  reply, and revalidate the reservation at issuance. A post-callback size check alone does not
  bound peak capture memory; the reservation does.
- **Issue.** `ctx.request_screenshot(callback)` sends a viewport command that itself forces a
  repaint, and a pending screenshot makes a hidden window take a full UI+paint pass. A screenshot
  that is merely **queued and not yet issued**, and settling deadlines, must have `tick`/logic send
  `RequestPaintWhileHidden` so the UI reaches issuance; do not add a second wants-paint request
  once the screenshot command has been issued. At issuance record the target-UI logical
  content/screen rect, ppp and a geometry epoch. That issued geometry + epoch, not callback
  metadata, is bound to the image: the callback carries **only pixels**.
- **Completion.** The app owns the strong slot and the owner-specific waker; the callback holds
  only a weak reference, stores the image unless canceled/completed, takes the wake handle, drops
  the slot lock, then requests the owning viewport's repaint. The callback does not wake itself and
  must not strongly capture a `Context` or the app; that would create a cycle with the viewport's
  queued command. `collect_screenshots` drains completed slots in `tick` and encodes/writes/replies
  outside the callback.
- **Geometry/scale binding.** With an **unchanged geometry epoch**, derive the pixel scale from
  the returned image dimensions against the issued rect and verify aspect/rounding; the returned
  image may be smaller when E2 surface fitting applies. **Any** intervening resize/DPI/zoom/surface
  epoch change between admission, issuance and save cancels and retries the request — even if the
  geometry later changes back before the save. Never crop with the admission-time, current or any
  other ppp that disagrees with the issued epoch. Store admitted/issued/captured values and record
  mismatches; an explicit error is correct when a retry cannot complete inside the deadline.
- **Cancel/timeout/drop.** Removing the request record (timeout, close, app drop, caller drop)
  marks its slot canceled under the lock; a late callback finds no live slot or a canceled one and
  drops the image without a second reply or file write. Release the reserved bytes on every exit
  path (completion, timeout, cancellation) and assert retained bytes return to zero after drain.
- `save_screenshot` takes the actual scale/geometry instead of `&egui::Context`
  (`control.rs:570-606`), so it can run in `tick`.
- `issue_screenshots` (`lib.rs:1530-1558`) uses the tick's `now` and a settle deadline, not a
  per-pass frame counter; `collect_screenshots` (`lib.rs:1560-1580`) becomes the slot drain.
- Remove the "occluded/minimized" refusal at `lib.rs:1490-1498` for the native wgpu path only after
  the platform run shows the screenshot request really produces a frame. The refusal exists because
  0.36 never painted a hidden window; 0.37 makes a pending screenshot a wants-paint action
  (`wgpu_integration.rs:680-690`, `egui-winit/src/lib.rs:2116-2128`). Keep the timeout error and
  add `ui.focus` as the fallback. Record Windows/macOS/Linux results separately. Do not claim
  hidden-window screenshots on a platform that was not tested. On web, eframe does not paint a
  hidden tab (logic-only passes have no clipped primitives), so `ui.screenshot` on web requires a
  visible tab; record that limitation.
- Correlation tests: two concurrent screenshots with different crops; callback-once behavior; a
  timed-out request followed by a late callback (no second reply, no file/PNG); shutdown/close with
  a pending capture; aggregate byte-budget rejection; a resize/DPI change between admission,
  issuance and completion (retry or explicit error, never a stale crop); a hidden-window capture on
  native wgpu is a positive row (not a "missing RequestPaint" negative); retained bytes return to
  zero; the app/Context can be dropped without the callback keeping it alive.
- `docs/control-protocol.md:125-129` and the module docs at `control.rs:1-27` need the new wording
  (no user-visible protocol change: `ui.screenshot` keeps its method and reply shape).

### 5.8 Viewports and GPU

- Floating panels (`dock_ui.rs:466-512`) use `show_viewport_immediate`; the video preview uses the
  same (`prefs_live.rs:166-236`). Keep them inside `draw`. Persisted window rects
  (`PanelWindow::sanitized`, `ui.panel_windows`) are unchanged.
- **Hidden playback and child windows.** `show_viewport_immediate` children only update when the
  parent runs a pass. If playback is playing while the root window is minimized/occluded, or a
  floating panel/video preview is open, `tick` must keep `want_paint` set (and request a repaint at
  the needed cadence) so the parent runs the forced hidden pass. Do not rely on the one-shot
  open/close command mitigation alone. Test a minimized-window playback with the Video Preview open
  and with a floated panel, asserting the child content advances and the parent does not thrash.
- 0.37's `#8748` (a hidden parent does not update deferred children) is carried as P7c's selected
  fix in the final P8 revision; verify it in the hidden-playback test. Independently, a command that
  opens/closes/floats a panel or video preview also sets `want_paint`, and playback keeps the pass
  cadence, so the app does not depend on one mechanism alone.
- GPU: `egui_wgpu::Renderer`'s
  `register_native_texture_with_sampler_options`, `update_egui_texture_from_wgpu_texture_with_sampler_options`
  and `free_texture` are unchanged in 0.37 (`renderer.rs:790-930`), so `viewers.rs:150-210` needs no
  API change. `RenderState`/`Frame::wgpu_render_state` are unchanged. Keep E2+E3. The existing
  `ui_gpu.rs` GPU-vs-CPU tolerance (at most 0.3% of pixels differ by more than 1/255, worst channel
  difference ≤4) stays.
- **A skipped GPU test is not evidence.** `ui_gpu.rs` returns early when
  `effectcraft_gpu::Gpu::headless()` finds no adapter (`tests/ui_gpu.rs:44-47`) or when egui-wgpu's
  GL device has WebGL2 limits (`tests/ui_gpu.rs:62-65`). For the GPU acceptance row run with a
  required adapter, assert `gpu_adapter().is_some()`, and record adapter/backend/driver and the
  exact test invocation. The planning host exposes an i915 render node (`/dev/dri/renderD128`) with
  no display session; that can qualify a headless wgpu adapter run, not presentation.
- Device loss must be handled from `tick` (see §4.2); the `ui()` early return currently means a
  device lost while hidden is not acted on (`lib.rs:1905-1910`). Add a logic-only test that a
  reported failure closes the window and keeps the auto-save.
- 0.37 `Frame::set_wgpu_surface_config` is fixed (`#8442`); EffectCraft does not use it. Do not add it
  as part of this migration.

### 5.9 Language and screen readers (fork-only stage)

When `craft-libs#198`'s fork APIs are accepted:

- In the app tick, after `i18n::set_current(language)`, call
  `ctx.set_language(Some(language.to_string()))` (fork E17) and, where an `eframe::Frame` is
  available, `frame.set_ui_language(Some(language))` (fork E18). This sets the AccessKit root
  language and widget text layout language. It does not change the app's catalogs.
- Assert `ctx.language()` follows the interface language, the AccessKit root node carries the
  language, and a `TextEdit`'s text is shaped with the UI language (CJK glyph choice where fonts
  are installed). Tests that need the fork API are marked fork-required in their module docs.
- Keep the app LTR (`Style::right_to_left` unset). Text-layer RTL remains `effectcraft-text`'s
  contract; UI-label bidi is epaint's.
- The E17/E18 policy sub-stage is optional and separately revertable. The production app already
  requires the final fork and E21b from P1; a stock-0.37 control uses a separate minimal adapter
  and is not obtained merely by reverting P4 (§11.13).

## 6. Dependency topology, publication reachability and rollback

Intended graph (final reviewed storytold revisions; names shown for structure):

```text
effectcraft / effectcraft-web / effectcraft-cli
  -> effectcraft-ui-egui -> craft-ui (craft-libs rev with CL-1 + CL-2F)
                          -> egui 0.37.0 (git <final storytold egui P8 SHA, incl E21b/P7c>)
                             -> epaint, epaint_default_fonts, ecolor, emath (same git rev)
                             -> eframe, egui-wgpu, egui-winit (same git rev)
                             -> accesskit 0.25.1, kittest 0.5.0
                             -> winit 0.30.13 (git <final storytold winit SHA, incl W15P>)
  -> winit (direct, same git rev)
```

- Every egui-family crate EffectCraft resolves must come from the same git revision. `cargo tree -d`
  must show no second egui family, and `cargo tree -i winit@0.30.13` exactly one source.
- Publication: EffectCraft is an application, so a git egui/winit dependency is acceptable in its
  lock, but no branch names, no `path` patches and no machine-local checkout may appear in a
  publication branch. The `aiongg` candidate SHAs are comparison inputs and must not appear in a
  publication lock. `craft-libs` consumers keep their own patches; the app's `[patch.crates-io]`
  is the supported place for the suite fork.
- A clean checkout must fetch every final approved revision and pass `cargo check --locked` and
  `cargo test --locked` without local overrides; record that proof with the lock.
- Rollback: keep the exact baseline lock (egui family 0.36.2, craft-ui `5c874d51`, winit registry
  0.30.13) in the task evidence. The project file format, command ids, undo model, workspace-state
  **version 1** and the GPU compositor are unchanged by this migration, so rollback is a revert of
  the pin/refactor commits, with no data migration. Do not persist any 0.37-only preference until
  the final stage, so an older build ignores nothing it cannot read.
- `examples/plugins/posterize-bands` is a separate workspace with its own lock and no egui; do not
  edit it. `craft-pdf`/`hayro-syntax` and the filmcraft pins are unrelated and stay.
- There are no vendored crates in this repository (`vendor/` absent); the only patch is the
  craft-libs `hayro-syntax` entry. Do not delete it, and do not "clean up" the commented filmcraft
  path patch by enabling it.

## 7. Command, undo, save/reopen contracts

Nothing in the egui bump may change these:

- Every gesture still routes through `effectcraft_engine::Session::execute` or `menus::invoke`
  (`lib.rs:1-6`, `control.rs:1-19`); UI-local state must not become the source of truth.
- One gesture = one undo step: existing tests such as `ui_viewer.rs:2263` (fit/zoom),
  `ui_viewer.rs:2306` (rotate), `ui_project_delete.rs`, `ui_timeline_trim.rs` and `ui_keyframes.rs`
  stay green.
  `Session::history.merge_key` is reset when a numeric scrub ends (`lib.rs:1866-1867`,
  `pointer_wrap.rs:31-36`).
- Save/reopen: `.ecproj` JSON contains no egui types; save, reopen and pixel-compare a comp after a
  UI edit. Workspace state (`workspaces.json`, **version 1**, `workspace_state.rs:37-40,85-88`)
  contains `PanelKind`, rects and floats, not egui ids; save, restart and restore the dock/floating/
  window layout. If a new field is ever required, specify an explicit version bump, a migration and
  a backward-compatible reader test; do not silently reuse version 1.
- Multi-pass must not duplicate a command, an undo step, an autosave write or a close prompt
  (§4.4, §8.3).
- **Auto-save while hidden has two owners today.** Project auto-save/recovery already runs from
  `tick`; the workspace-layout autosave is still in `draw` (`lib.rs:1715-1722`, gated on
  `!ptr.any_down()` and `workspace_store.due(now)`). P2 moves its due check and write to `tick`
  using the app clock and the last pointer-down state observed in `draw`. Only then does the
  hidden claim cover both kinds; the close guard and crash recovery remain tick-owned. §8.3's
  test asserts both writes while hidden.

## 8. Tests and acceptance

### 8.1 Repository gates

Every stage runs the repository gates (through the server job workflow, never directly from the
planning task): `cargo fmt --check`, `cargo clippy --workspace --all-targets --release -- -D warnings`,
`cargo test --workspace --release`, `cargo xtask layers`, `cargo xtask assets`, `cargo xtask wasm`,
plus `node apps/effectcraft-web/tests/{workers,page,storage}.mjs` (no build needed).
On every dependency stage also run `cargo check --locked`, `cargo test --locked`,
`cargo tree -d`, the family/winit inverse trees and the wasm-target `wasm-bindgen` tree on a clean
checkout. `cargo xtask ci` is the final gate.

### 8.2 Compile and API contract

- The 15 errors and 602 warnings above are gone, with no `#[allow(deprecated)]` added to production
  code.
- Grep gates must use precise patterns:
  ```sh
  git grep -nP '\bId::new\(' -- '*.rs'                          # only comments/none
  git grep -n 'WidgetType\|AtomLayoutResponse\|\.vertical_align(' -- crates apps
  git grep -n 'with_accessibility_check(false)' -- crates apps     # must be empty at every stage
  ```
  The old `grep -rn "Id::new"` also matches `GlyphId::new` in `crates/text`/`crates/pdf` (7 lines)
  and can never return "none"; `\bId::new\(` does not match inside `GlyphId`.
- `ui_agent_audit.rs:73` uses `WidgetInfo.role` (`typ` is gone); all tests construct `MouseWheel`
  with `source`.
- Tests for `Id::unique` stability and for the role mapping table.

### 8.3 Lifecycle, hidden windows and multi-pass

New tests (headless, `Context::run_logic` + `run_ui`, no GPU). The driver must mirror the real
eframe order: `raw_input_hook`-equivalent (`feed_synthetic`) → `run_logic` → retain the mutated raw
input → next `run_ui`, and it must not replay input events from `tick`. Use `AppClock::manual()`:

1. `logic_only_drains_control_and_jobs`: a control request that only needs `tick` (e.g.
   `engine.execute` of a non-UI command) gets its reply with zero `run_ui` calls; `poll_render`
   completes a render job and the Toast/event is drained. Advance the manual clock between ticks.
2. `logic_only_feeds_synthetic_and_forces_one_paint`: with the root viewport marked
   `minimized: Some(true)` (so `visible() == Some(false)`), a queued `ui.click` is fed by the hook
   during a logic-only tick; the tick emits `RequestPaintWhileHidden` and sends **no** reply. A
   second logic-only tick does **not** feed another event while the first is still unapplied. The
   following forced `run_ui` sequence (still minimized) consumes one pointer event per pass, applies
   the buffered gesture exactly once and replies exactly once after the final pass. The same test
   asserts that the forced passes are not gated on `visible()` and that no event was dropped.
3. `after_input_replies_only_after_a_paint`: same, and assert the command's effect exists before
   the reply is sent; force a discard on that pass and show the reply still waits for the final
   non-discarded pass.
4. `forced_hidden_pass_applies_buffered_gesture`: feed during a logic-only tick, run forced full
   passes until `synthetic` is drained, and assert exactly one command and one undo step (no
   duplicate from the logic-only call).
5. `playback_advances_without_a_paint`: with a viewer scale recorded and the manual clock, hidden
   ticks advance `session.time` within the preview step and do not duplicate `Playback.frame_shown`;
   audio is started once (stub `audio_device`); a stale `ctx.input(|i| i.time)` cannot satisfy a
   deadline.
6. `autosave_and_close_guard_run_while_hidden`: project auto-save and the moved workspace-layout
   autosave both write in logic-only ticks; a hidden close request opens the unsaved dialog and
   sends `CancelClose`.
7. `discarded_pass_is_idempotent`: force `ctx.request_discard` (or show a first-time `Grid`) while
   performing one command, one drag and one auto-save; assert one command execution, one undo step,
   one file write.
8. `device_loss_while_hidden_closes_and_saves`: bridge a failure during a logic-only tick; assert
   the close command and the auto-save, as in `lib.rs:2040-2075`.
9. `hidden_playback_keeps_child_viewports`: with playback playing and a Video Preview or floating
   panel open, hidden ticks keep requesting a pass and the child content advances (§5.8).
10. Screenshot-slot tests from §5.7 (concurrency, cancellation, late callback, resize/DPI including
    a change back, byte budget, shutdown).
11. `repeated_logic_only_ticks_do_not_collapse_a_gesture`: queue a `ui.click` and a `ui.drag`, run
    several logic-only ticks (mirroring eframe's overdue `RedrawRequested` and 100 ms invisible
    throttle) while the root viewport stays minimized, and assert the hook fed at most the one
    unapplied event; then run forced full passes and assert press/move/release each reached a
    separate pass, exactly one click/drag command and one undo step resulted, and no event was
    dropped. Repeat for a click followed immediately by a drag.

### 8.4 Screen geometry, pixels and accessibility

- Fixtures: the built-in demo project (`file.openDemoProject`) for visual states, the generated
  `engine::perf::large_project` for scale/perf, and the bundled Inter/JetBrains Mono fonts. No
  external media is needed for UI geometry/accessibility/pixel tests; media-dependent tests keep
  their existing generated fixtures.
- Geometry/hit tests (existing helpers, automation rects): viewer comp rect and fit at 1.0/1.25/1.5/
  2.0 ppp; dock split resize; tab drag and overflow; floating panel window (native only; embedded
  web path separate); menu item click at a label; timeline row heights (`ui_parameter_spacing.rs`
  keeps 20/19 px); panel order/nesting after maximize/restore.
- Double-click paths: 24 `double_clicked()` call sites (27 textual `double_click` occurrences
  including the Timeline helper, e.g. project rename `project.rs:783,832,1036`, Timeline
  `timeline.rs:1502,2116,2325,2516,2876,3529`, Flowchart `flowchart.rs:360`): 0.37 changed
  double/triple-click classification (`#7915`). Assert one double-click performs exactly one action
  and does not also fire a triple-click path.
- **Visual review harness — correct the claim and extend it.** Today
  `crates/ui-egui/tests/ui_visual_review.rs` (`UI_REVIEW_DIR`, `#[ignore]`) captures the workspace
  in light and dark, but every `PanelKind::ALL` panel and the About/New Comp/Settings/Shortcuts/
  Command Palette dialogs only in **dark**, at the harness default ppp 1.0, at wide 1600×1000 and
  compact 1024×768 (`ui_visual_review.rs:13-48`). Extend it to capture every panel and dialog in
  **both themes at ppp 1.0 and 2.0** (four combinations per state, both sizes), then use it on the
  baseline and every stage. This is the primary before/after visual record; the golden set below is
  the automated subset. Record that the ignored test actually ran; an ignored/absent capture is not
  evidence.
- Pixel comparisons: choose one of
  - `egui_kittest` `snapshot` goldens (enable the `snapshot` feature, add `kittest.toml` with
    `threshold` per OS, commit a small fixed set of PNGs under `crates/ui-egui/tests/snapshots/`
    **with `.attribution` sidecars and `ATTRIBUTION.md` rows**, because the repository's asset rule
    applies even to screenshots); or
  - an app-owned tolerant compare against RGBA hashes stored in a Rust constant, with the same
    tolerance policy and no image assets.
  Exact selected set: five states — Standard workspace (demo, time 2 s), Timeline maximized with
  rows open, Project panel with folders open, a menu open, and the Settings dialog — each in
  **light and dark at ppp 1.0 and 2.0** (20 captures), not a single dark/default-scale set.
  Threshold: max channel delta 2/255, ≤0.1% failed pixels on Linux/Windows, 3/255 on macOS (if any
  platform-specific rasterization is observed); no tolerance for geometry rects. Never accept a new
  golden automatically; a changed golden needs a reasoned explanation in the PR.
- Mutation check (required, recorded as evidence, not committed as a test): nudge `Tokens::focus` or
  a panel width by a visible amount behind an env flag and show that the pixel/geometry test fails
  with the expected diff; restore. This proves the assertions detect change.
- Accessibility: **strict checks throughout**; `Harness::unnamed_widgets()` empty for the workspace,
  for each panel maximized, for every dialog in `Dialog`, and for the Home/Media Browser/Effect
  Controls/Properties/Timeline panels. Role contract tests for color well, tab + selected, splitter,
  tree item, text input, spin button, slider and a checked menu item. `Role::Button` + `selected` for
  selectable labels. Read-only fields are exempt by kittest. The repo-wide grep gate in §8.2 proves
  no global/per-builder accessibility opt-out remains.
- Glyph coverage: the extended chrome symbol test (§5.4); `ui_effect_names.rs:35` and
  `ui_presets_folder.rs:40` use `allow_missing_glyphs()` with a comment. No global glyph opt-out.
- `ui_gpu.rs` and `ui_visual_review.rs` are skip/ignore-capable: the acceptance record must name the
  exact invocation and show they actually ran. A skipped GPU test is not GPU evidence (§5.8).
- Screenshot tests (§5.7) and IME/adapter tests (§5.5).

### 8.5 Save/reopen and command/undo workflows

- UI edit → save `.ecproj` → reopen in a fresh `EffectcraftApp` → project JSON equal and viewer
  pixels equal; undo/redo across the save still works.
- Dock/floating/panel-window layout → save workspace → restart → restored; maximize/restore and
  split sizes unchanged.
- One command from the menu, one from the control channel and one from a synthetic click each
  produce one undo step; a discarded pass does not add another.

### 8.6 Web workflow

- `cargo xtask wasm` (all L0–L4 + ui-egui + web) and `cargo xtask web --serve 8765`.
- `node apps/effectcraft-web/tests/smoke.mjs --url http://127.0.0.1:8765/ --out <evidence>`:
  WebGPU/GPU-compositor path, viewer, `render.frame`, Render Queue downloads, background worker
  event-loop gaps (<400 ms), AudioContext, save/reopen, persistence across reload, offline service
  worker. Also `workers.mjs`, `page.mjs`, `storage.mjs`.
- Browser screenshots come from CDP (`smoke.mjs:113`), independent of the egui screenshot API; add
  a page-side check only for the in-app `window.effectcraft.request("ui.screenshot", …)` reply shape
  when the tab is visible. A hidden web tab cannot paint (logic-only passes carry no clipped
  primitives), so `ui.screenshot` on web is a visible-tab feature; record it.
- Web is a separate feature set: no OS clipboard/PRIMARY, no system fonts, embedded viewports.
- **Hidden-tab behaviour, realistically.** Upstream 0.37's web runner already falls back from
  `requestAnimationFrame` to `setTimeout(10 ms)` when `document.hidden()`
  (`eframe/src/web/web_runner.rs:238-279`), so `AppRunner::logic` keeps running with logic-only
  passes and the browser clamps background timers to roughly one second (long-backgrounded Chrome
  may throttle further). Assert that the session snapshot's timestamp advances across the hidden
  period at the browser's actual cadence, that frame workers keep completing within that allowance,
  and that the viewport is fully refreshed when the tab returns. Record the browser, visibility
  transition and measured cadence; do not require exactly 1 s and do not add a new worker/timer
  scheduler or page-independent persistence feature in this migration. Fix `WebApp::logic` to pass
  the app clock (`self.app.now()`) to `persist::snapshot`, not `ctx.input(|i| i.time)` (which is
  stale in a logic-only pass).

### 8.7 Native platform matrix

| Row | Required evidence |
| --- | --- |
| Linux headless wgpu (this server) | `ui_gpu.rs` with a **required** adapter on the i915 render node (`/dev/dri/renderD128`, no display session): assert `gpu_adapter().is_some()`, record adapter/backend/driver and the exact command. This qualifies a headless GPU-compositor run only, not native presentation. |
| Linux X11 | Native window under **Xvfb + a real window manager** (e.g. openbox) **+ an X11 input method** (e.g. ibus/fcitx): screenshot, save/reopen, floating panel window, focus/menu/close/auto-save, clipboard and PRIMARY between windows, X11 IME (Vietnamese/Japanese) with editor/undo checks. Record Xvfb/WM/IME versions and that the session is virtual and not composited. This can qualify the X11 parts; it cannot qualify a physical GPU or Wayland. |
| Linux Wayland | Requires a real compositor; unresolved unless a managed environment is provided. Run the E4/W15P PRIMARY **native capability matrix** on wlroots/Hyprland, KWin and Mutter: protocol availability, keyboard focus, keyboard and pointer-selection serial freshness, owner/seat change, empty/absent offers, self-paste, slow/canceled transfer, password exclusion, unchanged-selection reclaim, and no second regular data device (protocol trace). Also trace text-input-v3 current-epoch mismatched `Done` (incoming applied once, outgoing deferred), stale-epoch discard, and disable/enable on owner/support changes. Record unsupported capabilities honestly; do not substitute X11/arboard results. |
| Windows (`craft-windows` clone, separate task) | real native build/test, GUI launch, screenshots, menu/close/auto-save, clipboard, IME, DX12 pipeline startup timing, mixed-DPI window placement, and a held Ctrl+Z release after a layout switch; Basic Render Driver is software and must be reported as such. |
| macOS | **unresolved gate** unless real hardware evidence is provided: press-and-hold/dictation and surrounding-text NSRange (`W26`), Command-key shortcuts, traffic-light/titlebar placement, screenshots while occluded. |
| FreeBSD / Nix / Windows ARM64 | CI workflows are reusable `craft-common-actions`; run after the pin lands and report only what the runners provide (FreeBSD nightly, ARM64 MSI on schedule). |

Headless kittest is not evidence for native focus, IME, file pickers, clipboard, PRIMARY or GPU
presentation. Physical-GPU and Wayland rows remain explicit unresolved gates until measured.

### 8.8 Known upstream regressions as acceptance gates

- `#8731` (text tessellation, reported +61% instructions for plain text): measure with the paired
  text-heavy operations (workspace, timeline, project) and the raw `tessellate` instruction count
  where possible. This measurement belongs to the pre-P1 gate. If the median regression is above
  threshold, the row-wise fix (provenance, test, removal condition) must be in the reviewed P8
  revision before P1 is frozen, or the migration is blocked. P5 may not add it afterwards.
- `#8736` (DX12 pipeline creation, reported ~40 ms): measure Windows startup and first-frame cost
  separately for the DX12 backend. If it pushes startup over threshold, the fix must be in P8 before
  P1 or accepted with an explicit owner decision and the measurement; P5 only re-reports it.
- `#8512` (system font startup cost): keep `system_fonts` off and record font-install/discovery
  time in the startup row so a later color-emoji decision has a baseline.
- `#8489`/`#8583`/`#8601`/`#8589` are handled by this plan's refactors.

## 9. Performance plan

### 9.1 Baseline before production changes

Add instrumentation first (stage B0, no behaviour change): raw samples per operation, p50/p95/p99,
the number of **final** paint passes and discarded passes (`Context::will_discard()` at the end of
`draw`), texture-upload bytes per frame from `FullOutput.textures_delta`, logic-only tick time,
startup-to-first-frame and font-install/load time, and an optional counting global allocator in
`effectcraft-cli` for `bench --ops` (byte counts per operation). Record raw and compressed
`effectcraft-web` `.wasm` sizes. `Measure` gains `samples`, `median`, `p95`, `p99`, `cpu_ms`;
existing names and JSON fields stay. Then run the **B0-instrumented** baseline: commit `9581b53c`
+ the instrumentation patch, with its patch SHA and the 0.36.2 lock recorded. The baseline identity
is B0-instrumented `9581b53c`, not the bare commit. The same instrumentation is applied to the
candidate so both sides are measured identically.

### 9.2 Matched operations

| Operation | Command / driver | Sample rule |
| --- | --- | --- |
| Startup to first frame | `bench --ops --json` `startupToFirstFrame`; native log "first frame drawn" | ≥20 fresh processes per source/lock; same large project, fonts, window 1680×1020 |
| Steady workspace frame | `workspaceFrame` | ≥10 warmups, ≥30 timed iterations per session, 3 sessions |
| Timeline scroll / open rows | `timelineScrollFrame`, `timelineOpenFrame` | 5,000 layers, every 10th open; ≥30 iterations |
| Project panel scroll | `projectPanelFrame` | all folders open; ≥30 iterations |
| Menu open / dock resize / typing | new `menuOpenFrame`, `dockResizeFrame`, `textTypingFrame` | fixed input scripts; ≥30 iterations |
| Frame distributions | scripted workspace/timeline/typing loop or native playback | **≥1,000 actual final paint passes per row**; record discarded passes and dropped frames; p50/p95/p99 only with ≥1,000 |
| Logic-only tick | new `logicOnlyTick` | 1,000 `run_logic` ticks, idle queues; asserts no paint work |
| Idle repaint rate | new `idleRepaints60s` using `Context::set_repaint_observer` | virtual 60 s; count causes |
| Playback | native preview long enough for ≥1,000 presented frames (e.g. 20 s at 60 fps, or repeated 10 s segments with a fresh start) | actual pass count, dropped frames, audio clock; `ui.inspect.fps` is a smoothed readout, not a distribution |
| Texture uploads | bytes/frame from `textures_delta` | viewer frame and thumbnails |
| Allocations | counting allocator totals per op | optional but recorded |
| Font startup (`#8512`) | cold start with system-font provider off; record font discovery/install/load | ≥20 fresh processes; part of startup evidence |
| WASM size | `cargo xtask wasm` artifact | raw and gzip/brotli sizes, baseline vs candidate; accept a budget or make emoji fonts native-only |
| Engine control | `cargo xtask bench --quick --repeat 5` | guards accidental engine/render drift |
| GPU vs software | same UI ops with llvmpipe/software GL and a required hardware adapter where present; Windows Basic Render Driver separately | separate tables, never merged |

### 9.3 Environment and protocol

Record: CPU/GPU model, RAM, OS, driver, display scale, window size, renderer backend, release
profile, exact commit/SHA/lock/craft-ui/egui/winit revisions, font set, fixture project SHA, and
load average. Every row uses **≥10 warmups** and **≥30 timed iterations** for steady operations,
**≥20 fresh processes** for startup, and **≥1,000 actual final paint passes** for a frame
distribution. Report the exact frame counts, discarded passes, dropped frames, audio underruns, raw
samples, median/p50, p95 and p99; do not report a p99 from 30/90 samples. Measure peak RSS
(`/usr/bin/time -v` or `-l`) and idle CPU (60 s); use `perf stat -e instructions:u,cycles:u` where
available. Use `cpu_time`/instructions and wall time; on a shared host, wall time is secondary.
Software renderers and physical GPUs are separate tables.

Predefined thresholds (owner sign-off required to widen):

- steady/scroll/project/typing median ≤ +5%, p95 ≤ +10%;
- startup median ≤ +5%, p95 ≤ +10%, using the shared small-operation noise floor;
- logic-only tick median ≤ 0.5 ms and p99 ≤ 2 ms on the large project (no paint work);
- idle repaint count ≤ baseline per minute; no new recurring repaint while idle;
- texture upload bytes per steady frame ≤ +10%;
- peak RSS ≤ +5%; record absolute MiB as well; no automatic absolute-size exemption;
- engine render benchmark (`cargo xtask bench`) within its own noise band; no regression;
- text tessellation (`#8731`) and DX12 startup (`#8736`) are reported as their own rows and must
  meet the same median/p95 or be fixed/accepted explicitly.

Noise policy: run the unchanged 0.36.2 build twice as a control. The first report must also run
**stock egui 0.37.0** as the required upstream-equivalence measurement control (not a production
pin), and report regressions only against these controls. If control spread prevents resolving the
shared thresholds, report inconclusive and rerun under quieter conditions; never widen a threshold
automatically. If a regression exceeds the gate, re-run paired 3× and profile;
never update a golden or baseline to hide it. A changed pixel golden is a reported visual change
with a reason, not a performance result. No performance number is claimed by this document; the
thresholds are proposals to be validated by paired runs on recorded hardware.

## 10. Staged implementation PRs

Dependency DAG: `B0 → P1 → P2 → P3 → P4 → P5`. P1 waits for the complete reviewed/published
storytold egui **P8** (E1–E21, P2b async W15P routing, P7 held-key pairing, P7b/E21b, P7c's
selected `#8763`/`#8748` fixes and any justified `#8731`/`#8736` perf fix), the completed winit
(W1–W27, W15P, W27 lifecycle), and craft-libs CL-1 and CL-2F after its DesignCraft/GridCraft
pilots. P1 includes the compatible `IMEOutput` producer and all required fork API adaptations.
There is no stock or intermediate-fork production stage. If a consumer already has an
incompatible craft-ui/egui bump, it must be reconciled in this same atomic stage. P4 adds native
language/platform evidence and optional language UI policy; reverting it never removes the P1
compatibility adapter. P5 is measurement/docs only and requires final platform and performance
acceptance.

| Stage | Scope | Validation | Rollback |
| --- | --- | --- | --- |
| **B0** | Bench instrumentation only: samples/percentiles, final/discarded pass counters, logic-only tick, repaint observer, texture-byte counter, font-startup and WASM sizes, optional allocator; baseline run on B0-instrumented `9581b53c` (record patch SHA and lock) | `bench --ops --json` raw evidence; no behaviour change; gates | revert commit; baseline evidence retained |
| **P1** | Final reviewed storytold dependency pins (complete egui P8: E1–E21 + P2b + P7 + P7b/E21b + P7c `#8763`/`#8748` + justified perf fixes; completed winit W1–W27 + W15P and W27 lifecycle fixes; craft-ui with CL-1 + CL-2F), including the viewer_text surrounding-text adapter and compatible `IMEOutput` literal, feature `clipboard`, `default_fonts` + `monochrome_emoji_fonts`; mechanical API migration (IDs, roles, `Ui::scope_id`, `Atom`/`IdSalt`, font bytes, `TextEdit::align`, `MouseWheelSource`, screenshot callback via the CL-2F slot, test `Event`s); **actual strict kittest accessibility names and glyph fixes** until all tests pass with `check_accessibility: true` and missing-glyph `Panic`, with `allow_missing_glyphs` only on the two named script tests. No `with_accessibility_check(false)` anywhere. | `cargo xtask ci`; `cargo check/test --locked`; `cargo tree -d`; one-family/one-winit and wasm-target checks; all tests compile and pass | revert P1: restores the 0.36.2 lock; no data changes |
| **P2** | Lifecycle: `tick`/`draw` split, injected app clock, focus, polls/playback/audio/project and workspace-layout autosave/events in tick, hidden `RequestPaintWhileHidden` (including queued screenshots, settling and playback/child viewports), synthetic-input generation guard, input-waiter ordering after draw, close guard, screenshot scheduling ownership | §8.3 tests; native hidden/minimized manual check; benchmark logic-only/idle | revert P2: returns work to `ui`; behaviour-only, no data |
| **P3** | Accessibility completion (custom roles/names), glyph coverage, extended light/dark × 1.0/2.0 visual harness, geometry/pixel tests, style/visual adjustments if the tests show unintended movement | §8.4 tests, `cargo xtask assets` (only if PNG goldens), a required-adapter `ui_gpu.rs` run | revert P3: tests and names only (except any visual fix, which is isolated) |
| **P4** | Extended language/platform qualification: `set_language`/`set_ui_language` policy where applicable, RTL UI-label tests, PRIMARY native capability matrix, clipboard/non-Latin shortcut and native IME rows. E21b/`IMEOutput` compatibility is already part of P1. | fork-required tests and native matrix using the same coherent dependency unit | revert P4 policy/test additions; retain P1 final pins and required adapters |
| **P5** | Performance acceptance and re-measurement of the unchanged P1 dependency unit (no new fork commit), docs (`docs/architecture.md:359` still says 0.36.2; `docs/control-protocol.md` screenshots; `docs/testing.md` UI perf), final evidence. Any `#8731`/`#8736` fix must already be in the reviewed P8 pin from P1. | paired benchmark report with raw samples, frame counts, thresholds, stock-0.37 control, platform rows, unresolved gates | revert P5: docs/report only; no dependency or state change |

Each stage is a separate reviewable PR with its own exact source/lock identity, validation log and
screenshots. No stage pushes, opens a PR or publishes without fresh human approval of the exact
content and validation.

Compatibility checks after P1 and each later stage:

```sh
cargo tree -d
cargo tree -i egui@0.37.0
cargo tree -i winit@0.30.13
cargo tree -i wasm-bindgen --target wasm32-unknown-unknown
cargo check --locked
cargo test --locked
git grep -n 'path *= *"/' -- Cargo.toml crates apps   # must be empty
git grep -n 'branch *=' -- Cargo.toml                 # must be empty in publication
git grep -n 'aiongg' -- Cargo.toml Cargo.lock         # must be empty in publication
git grep -n 'with_accessibility_check(false)' -- crates apps # must be empty
```

## 11. Blocking unknowns and decisions

1. **Final storytold egui/winit SHAs.** The candidate `aiongg` revisions are comparison inputs only.
   Production waits for the approved, published implementation: a storytold egui **P8** SHA with
   E1–E21 + P2b + P7 + P7b/E21b + P7c's selected `#8763`/`#8748` fixes + any justified
   `#8731`/`#8736` fix, and a completed storytold winit SHA with W1–W27 + W15P and the W27
   lifecycle fixes. Both must be reachable, immutable and fetchable with `--locked`.
2. **craft-libs PR 199 base.** The implementation starts from merged PR 199 at
   `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`; historical `8614e4a`/`1764bca` on old `main`
   `4919088` and the stale `b68de8c`/`ec8840a` trees must not be merged. The final immutable
   craft-ui revision must keep registry `egui 0.37` and include CL-1 plus CL-2F.
3. **CL-2F.** EffectCraft requires the approved craft-ui slot helper at the CL-2F freeze in the
   same atomic stage as P1; only the Design/Grid pilots use the provisional CL-2 revision. If
   CL-2F slips, P1 cannot claim screenshot completion done; no app-local channel or unbounded
   queue is an acceptable substitute.
4. **E21b/P7b coordination.** The egui `IMEOutput` payload and the egui-winit W27 adapter must land
   atomically with the custom-editor adapters; the stock/fork `IMEOutput` shape cannot be mixed.
5. **`#8731` fix availability.** If text tessellation regresses over threshold, the fix belongs in
   the reviewed P8 revision before P1 (with provenance/test) or the migration is blocked; P5 cannot
   change the pin.
6. **`#8736` Windows startup cost.** Measure before P1; include the fix in P8 or record an explicit
   owner acceptance. P5 re-reports it.
7. **`monochrome_emoji_fonts` / `system_fonts` / WASM size.** The plan requires
   `monochrome_emoji_fonts` to preserve `✔`/`▣` and 0.36 parity. Measure the web raw/compressed
   size; accept a budget or add a native-only emoji-fonts feature. Enabling colour emoji via
   `system_fonts` is a separate decision with startup (`#8512`) and determinism costs.
8. **Pixel golden storage.** Committed PNGs require `.attribution` sidecars and `ATTRIBUTION.md`
   rows under the repository asset rule; if maintainers reject committed screenshots, use the
   hash-based tolerant comparison instead. Both need the same tolerance policy and the exact
   light/dark × 1.0/2.0 set.
9. **macOS evidence.** No macOS hardware is available to this task; the macOS gate (IME/NSRange,
   Command keys, title bar, hidden screenshots) remains unresolved until real evidence exists.
10. **Windows evidence.** Requires a `craft-windows` clone and the task workflow; the Basic Render
    Driver is software and cannot stand in for a real GPU result.
11. **Linux Wayland/PRIMARY and physical GPU.** The planning host has no display session (only an
    i915 render node), so a headless wgpu adapter run is possible but Wayland/PRIMARY/real
    presentation need a managed environment. Xvfb + WM + IME qualifies only the X11 parts.
12. **Accessibility workload.** The exact number of unnamed inputs is unknown until the suite runs;
    the known stock set is 39 `TextEdit` and 51 `DragValue`/`Slider` sites plus custom widgets that
    gain roles. P1 fixes what strict checks report; there is no global or per-builder opt-out, and
    P3 completes the custom-control coverage.
13. **Atomic compatibility boundary.** P1 uses the complete reviewed dependency unit and its
    required IME/screenshot adapters. No intermediate base pin is needed. Independent stock-0.37
    controls use a bounded test adapter and are never a production handoff. Extended language
    policy/qualification in P4 cannot undo required P1 API compatibility.

## 12. Limitations of this planning task

- No build, test, clippy, GUI, browser or benchmark was run. Compiler inventories are read from the
  2026-10-22 audit (a historical fork revision, machine-local path patches, `ui-egui` lib/lib-test
  only) and from source inspection; integration tests and the app/web crates will reveal additional
  errors.
- The environment record reports no `DISPLAY`, no `WAYLAND_DISPLAY` and `XDG_SESSION_TYPE=tty`, but
  an i915 render node is present (`/dev/dri/renderD128` readable/writable). This supports a
  headless wgpu adapter run only; it does not qualify native presentation, Wayland, PRIMARY or
  physical-GPU performance.
- The cmap/glyph findings are a read-only inspection of the bundled font files, not a rendered
  proof; the implementation must run the coverage test.
- Upstream issue claims (`#8731`, `#8736`, `#8763`, `#8748`, `#8631`, `#8512`, `#8428`, `#8657`,
  `#8577`) are reported by upstream authors, not measured here; the linked evidence file
  (`/home/codex/work/ssd2/task-evidence/egui037-migration-20261022/upstream-discussions.json`)
  records the snapshots. `#8764` is open/unmerged (`upstream-8764-fresh.json`), a proposal only.
- No performance number is claimed. The thresholds above are proposals to be validated by paired
  runs on recorded hardware.
- Windows and macOS behaviour, IME, native focus, file pickers and real-GPU presentation are not
  qualified by headless kittest and remain explicit gates.
- Reviewer dispositions: the review's findings on workspace version 1, the visual-harness
  coverage, the absent shared `with_accessibility_check` switch, the incomplete PRIMARY matrix,
  child-viewport freezing while hidden, the performance sample counts, the `ppm` capture point,
  dependency/lock gates, skipping `ui_gpu.rs`, and the `Id::new` grep/occurrence counts were adopted
  and are fixed above. Two review suggestions were corrected rather than adopted: `RawInput::time`
  cannot supply a logic-only clock (`run_logic` discards it; an app-owned injected clock is
  required), and freezing the admission-time DPI is wrong when the frame is captured later (the
  issued geometry + geometry epoch are bound to the image, and the returned dimensions supply the
  pixel scale). The v2 review's hidden-input accumulation, P7c/CL-2F dependency, baseline-identity,
  E21b state/serial, W27/W25 gate, accessibility-builder and anchor findings are fixed in place
  above. The review's blanket
  claim that hidden browser logic cannot run is also corrected: eframe 0.37 already falls back to
  `setTimeout` for hidden tabs; the realistic gate is the browser's clamped cadence, not a new
  worker feature. The review's Linux "no display means no X11 evidence" is narrowed to the parts
  that genuinely need a real compositor/physical GPU; Xvfb + WM + IME can qualify X11 focus,
  clipboard/PRIMARY and X11 IME behaviour.
- The pinned fork, winit, craft-libs and PR 199 can move before implementation; every plan stage
  records the exact SHAs it was reviewed against.


## Local implementation update — 2026-10-23

The user clarified that reviewed plans remain local and implementation proceeds before publication. The earlier requirement to publish/land this plan first is superseded; pushes and PR publication still require explicit approval.

Canonical `storytold/effectcraft` `main` now resolves to `f6ef1a790625657259f449f3f4f0773d7df449bb`. The owned checkout was fast-forwarded to this revision. Earlier baseline SHAs and line anchors above are retained as historical review evidence; use this new revision and its exact lockfile for current-main integration and paired measurements. The original refs remain reachable.

Preserve queue4096/click1–512/drag<=2048 validation and refusal-without-mutation test; these limits differ from FilmCraft. Preserve chrono feature split.

The complete static delta review and exact old/new object identities are retained in the task evidence: `canonical-diffs-refresh-20261023.json` and `overnight-consumer-review-20261023.md`. This amendment claims no new build, native GUI or performance acceptance.

## Local implementation stage 1 — 2026-10-23

Implemented in the owned worktree only: no commit, push, PR, release or publication; no build,
test, fmt, clippy or `cargo metadata` was run (the parent runs managed `task.py` validation and
generates the lockfile). Local dependency redirects are the parent's (unpublished) worktrees.

**Compatibility surface**

- `egui`/`eframe`/`egui_kittest` moved to `0.37` with explicit
  `egui` features `default_fonts` + `monochrome_emoji_fonts` and `eframe/clipboard` on the UI
  crate and desktop app. The `craft-ui` revision is unchanged (the 0.36-era pin) with a TODO:
  the reviewed CL-1 + CL-2F revision is the parent's to supply; no remote revision was invented.
- `Id::new`→`Id::unique` everywhere (603 occurrences); `Ui::id`→`Ui::scope_id`;
  `UiStack.id`→`UiStack.scope_id`; `WidgetType`→`egui::Role` (ColorButton→ColorWell,
  SelectableLabel→Button with `selected`, Other→Unknown/TreeItem); `AtomLayoutResponse`→
  `WidgetAtomResponse` with an `IdSalt` swatch id; `FontData` comparison through `bytes()`;
  `TextEdit::vertical_align`→`align(Align2::LEFT_CENTER)`; `Event::MouseWheel` gained
  `source: MouseWheelSource::Wheel` in the control channel and every test.
- Screenshots replaced `UserData`/`Event::Screenshot` token scanning with
  `craft_ui::capture::CaptureSlot`: 4 outstanding requests, 128 MiB per image and 256 MiB
  aggregate reservations from checked geometry, issuance only after 3 settled non-discarded
  passes plus 0.35 s, geometry-epoch binding (window position excluded), stale-epoch retry and
  rejection, deadlines, cancellation and late-callback release. The callback holds only the weak
  slot; files and replies are written in the tick.
- The covered/minimized refusal in `ui.screenshot` is intentionally still present: removing it
  needs a native platform run showing a pending screenshot forces a hidden paint.

**Lifecycle**

- `EffectcraftApp::tick` (all eframe logic callbacks, including hidden logic-only ones): engine and
  background queues, script tasks, engine events, native command inbox, close guard, window title,
  project auto-save, workspace-layout auto-save, memory watch/home-on-open, playback + audio,
  control channel, screenshot completion/issuance, and `RequestPaintWhileHidden` when a UI pass is
  needed. `EffectcraftApp::draw` (the root `Ui`): language/theme/font setup (the existing two-pass
  font warm-up), input-driven shortcuts, layout, panels, dialogs, dropped files, Video Preview,
  workspace-state observation, plus the pass geometry/epoch and the `AfterInput` acknowledgement on
  the final non-discarded pass.
- `AppClock` ( `web_time::Instant`, injectable manual clock) replaces every wall-clock or
  `ctx.input(|i| i.time)` use in the tick (auto-save, watch folder, footage reload, playback,
  control deadlines, screenshot deadlines, toast timestamps). `panels/viewer` now records its
  drawn scale and playback advances in the tick.
- One pending synthetic batch: `feed_synthetic` refuses a second batch until a UI pass applied the
  first (`mark_synthetic_applied`), a font-gated pass requeues the batch instead of dropping it,
  and `raw_input_hook` keeps the control provenance across hidden iterations.
- Web: dropped files are read in `raw_input_hook`; the persist snapshot uses the app clock.
  `RequestPaintWhileHidden` is sent on native only — the shared web fork does not carry it, so
  hidden web input/screenshots remain unqualified.
- IME: the custom viewer editor handles `ImeEvent::DeleteSurrounding` (characters) and
  `DeleteSurroundingBytes` (UTF-8 bytes, converted at char boundaries through `TextDoc`), grouped
  with the following `Commit` under one typing/undo key. Outgoing surrounding text (E21b/W27) is
  recorded as the next step at the single `IMEOutput` producer; no empty placeholder was invented.

**Tests added (not run here)**

- `lib.rs::lifecycle_tests`: logic-only tick records no fonts/geometry; a control command completes
  without a pass; one pending synthetic batch across repeated hidden hooks with a font-gated
  requeue and exactly one reply; geometry epoch ignores window movement and tracks resize/back;
  hidden playback advances and stops at the plan's end; workspace-layout auto-save writes on a
  hidden tick; a hidden close request opens the guard and sends `CancelClose`; screenshot
  issuance/settlement, resize retry, admission bounds + timeout release, and late-callback
  no-write/no-second-reply.
- `panels/viewer_text.rs` tests: byte-count IME deletion around an emoji, character-count deletion
  over a selection with clamping, and no-op cases.
- `theme.rs` glyph test extended to the full drawn chrome set; `ui_effect_names`/`ui_presets_folder`
  opt out of the missing-glyph panic only for their CJK/Japanese fixtures.

**Known remaining gates**

- Parent must generate `Cargo.lock`, supply the local patch config and the reviewed craft-ui
  revision, then run the repository gates. Strict kittest accessibility is still on: stock
  `TextEdit`/`DragValue`/`Slider` fields without a label or hint will be reported (no global or
  per-builder opt-out was added).
- `ui_gpu`/native GUI, hidden-window screenshots (the refusal above), Wayland/X11 IME and PRIMARY,
  Windows/macOS rows, and every performance row remain unmeasured here.
