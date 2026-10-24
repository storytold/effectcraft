# Plug-in availability and bounty list

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13 · **Change:** new public status list for plug-in/effect compatibility and bounty candidates · **Target:** Adobe After Effects 2026

This page answers a narrow question for users and contributors: which extension and effect
compatibility targets are already usable in EffectCraft, which are planned, and which are good
candidates for community bounty or sponsorship?

For the technical API, see [plugins.md](plugins.md). For overall product gaps, see
[gaps.md](gaps.md), especially gap 19, **Plug-in ecosystem**.

## Status legend

- **Shipped**: implemented and documented in this repository.
- **Partial**: usable, but known compatibility gaps remain.
- **Planned**: accepted direction or tracked gap, but not implemented enough for users to rely on.
- **Bounty candidate**: small enough to scope as a sponsored issue once acceptance criteria are written.
- **Needs owner decision**: likely large or legally/architecturally sensitive; do not start without maintainer guidance.

## Compatibility targets

| Target | Current status | Canonical tracking | Notes for contributors |
|---|---|---|---|
| WebAssembly effect plug-ins | **Shipped** | [Technical docs](plugins.md#effect-plug-ins) | API v1 loads deterministic `.wasm` / `.wat` effects on desktop and CLI. Good place for sample plug-ins and API hardening. |
| Rust effect plug-ins | **Shipped** | [Technical docs](plugins.md#effect-plug-ins) | Apps embedding EffectCraft crates can register Rust plug-ins directly. This is not a binary third-party ABI. |
| Script folder extensions | **Shipped** | [Technical docs](plugins.md#scripts) | `.jsx` / `.js` scripts installed in the settings folder appear in File ▸ Scripts. |
| ScriptUI panels | **Partial** | [Technical docs](plugins.md#scriptui-panels), [gap 19](gaps.md#every-known-gap-ranked) | Panels load and dock, but some workspace fidelity and shortcut integration gaps remain. |
| Bundled Ease Presets extension | **Shipped** | [Ease Presets docs](plugins.md#ease-presets-extensionsscriptui-panelsease-presetsjsx) | A maintained example of a bundled extension built on the public scripting API. |
| After Effects SDK binary plug-ins (`.aex` / `.plugin`) | **Needs owner decision** | [gap 19](gaps.md#every-known-gap-ranked), [#600](https://github.com/storytold/effectcraft/issues/600) | Native SDK hosting is a large compatibility and safety project. It should not be treated as a starter bounty. |
| OpenFX plug-ins | **Planned / needs owner decision** | [#327](https://github.com/storytold/effectcraft/issues/327) | Potentially cleaner than AE SDK hosting, but still a host API project. Needs maintainer direction first. |
| Third-party plug-in manager UI | **Bounty candidate** | [#376](https://github.com/storytold/effectcraft/issues/376) | A scoped first step could list loaded WASM plug-ins, source paths, API version, and load errors. |
| Community plug-in catalog / bounty list | **Bounty candidate** | [#1198](https://github.com/storytold/effectcraft/issues/1198) | Keep this page updated and link new planned or missing targets to canonical issues. |
| Animation preset expansion | **Bounty candidate** | [gap 7](gaps.md#every-known-gap-ranked) | Original presets are welcome; importing Adobe `.ffx` is a separate clean-room decision. |
| Effect behaviour parity fixes | **Bounty candidate** | [gap 9](gaps.md#every-known-gap-ranked), [effects-parity.md](effects-parity.md) | Prefer one effect bug per issue/PR with a regression test or comparison evidence. |
| Script startup/shutdown folders | **Bounty candidate** | [plugins.md](plugins.md#extension-points-compared-with-after-effects) | Small compatibility feature if scoped to documented startup/shutdown ordering and tests. |
| Keyboard shortcuts for scripts | **Bounty candidate** | [plugins.md](plugins.md#extension-points-compared-with-after-effects) | Useful integration task: expose installed scripts in the shortcut editor. |
| `.jsxbin` scripts | **Planned / likely low priority** | [plugins.md](plugins.md#extension-points-compared-with-after-effects) | Undocumented binary format; avoid unless maintainers explicitly bless an approach. |

## How to propose or sponsor a bounty

1. Open or find the canonical GitHub issue for the target.
2. Keep the scope narrow: one plug-in/effect compatibility target, one user-visible outcome, and one testable acceptance criterion.
3. Include evidence: current behaviour, expected behaviour, platform, and any public documentation used.
4. If sponsoring, comment on the issue with the reward amount, payout conditions, and deadline. Keep payments and identity checks outside the repository unless maintainers say otherwise.
5. Link back to this page when the status changes so users can find the maintained list.

## Good first bounty shapes

- Add one small sample WASM plug-in with docs and a smoke test.
- Add load-error reporting to the plug-in list or plug-in manager UI.
- Add startup/shutdown script-folder support with deterministic ordering tests.
- Add one original animation preset pack with preview coverage.
- Fix one named effect parity bug and add a regression test.
