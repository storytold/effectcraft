# `.aep` / `.aepx` decoder — stage tracker

**Tracking issue:** #251 · **Stages:** [CHECKLIST.md](CHECKLIST.md) · **Format:** [format-notes.md](format-notes.md)

Update this file **at the end of every stage**: flip the status, record the commit, restate what the
next stage is. Nothing here is aspirational — if a "Done when" was not met, the stage is not done.

> **Last updated:** 2026-10-11 · **Current stage:** S0 (provenance & scope) · **Blocked on:** maintainer
> answer on clean-room scope

---

## Status

| Stage | Name | Status | Evidence |
|---|---|---|---|
| **S0** | Provenance & scope | **in progress** | `format-notes.md` written; module header not yet rewritten; scope question not yet posted |
| **S1** | Synthetic corpus + differential fixtures | not started | — |
| **S2** | Two-pass project table (**precomp fix**) | not started | root cause identified, see CHECKLIST S2 |
| **S3** | Layer taxonomy | not started | — |
| **S4** | Hardening pass | not started | — |
| **S5** | Test suite | not started | — |
| **S6** | Fidelity harness | not started | — |
| **S7** | CI green | not started | — |
| **S8** | PR split & ship | not started | — |

Legend: **not started** · **in progress** · **blocked** · **done**

---

## Decisions

Record maintainer answers here verbatim, with a date and a link to the comment. These are the
things that must not be re-litigated.

| Date | Question | Answer | Source |
|---|---|---|---|
| — | Is a reverse-engineered `.aep` reader in scope for this repo, given `AGENTS.md` §2 and `docs/gaps.md` G4? | *pending* | #251 |

---

## Known defects

Live list of what is currently wrong, so a later stage can point at a named item rather than a
vague "it looks broken".

| # | Defect | Stage that fixes it |
|---|---|---|
| 1 | **Precomp layers do not resolve.** `ldta` source id has no single item namespace to resolve against; `is_shape` is tested before the comp lookup, so a vector-containing precomp is classified as a shape layer. | S2 |
| 2 | `comp_ids` keyed by **name**: two comps sharing a name collapse into one binding. | S2 |
| 3 | Self-referencing or unresolved precomp falls through to `LayerSource::Null`, which renders nothing — empty viewer with no diagnostic. | S2 |
| 4 | Layer kind guessed from `tdgp` match-name strings and a `parent_folder == Some("Solids")` name comparison. | S3 |
| 5 | Solids are assumed `[0.5, 0.5, 0.5]`, `1920x1080`, `30 fps`, `10 s` regardless of the file's `sspc`. | S3 |
| 6 | Footage items are pushed with hardcoded `1920x1080 / 30 fps / 300 frames`. | S3 |
| 7 | Non-test `unwrap()`s in `crates/ui-egui/src/panels/viewer.rs`. | S4 |
| 8 | `std::fs` use inside the engine crate for footage existence. | S4 |
| 9 | Tests reference local `C:\Users\...` template-pack paths and pass silently in CI. | S1 |
| 10 | Import report does not surface *unlinked layers*, only missing footage. | S2 |

---

## Stage log

Append one entry per completed stage. Keep the "Done when" line and the commit hash.

```
### S0 — Provenance & scope — <date>
Commit: <hash>
Done when: <restate the exit condition> -> PASS/FAIL
Notes: <what changed, what was learned, what it unblocked>
Next: S1 — synthetic corpus
```

