# Expression result-memo working sets

The local integration’s pre-change result memo cleared at 8,192 entries; upstream main did not
yet have this shared result memo. A speed-graph-style sweep of 20 properties,
720 points each and two evaluations per point requests 28,800 values. Repeating that sweep ran
every script again because the working set could not remain cached.

The memo now allows up to 65,536 small entries within a 32 MiB **estimated per-thread retained
storage budget**. Accounting includes text, paths, current values, request strings, vector
capacities, responses, detached results, inline entries and reserved hash-table storage. The
1 MiB per-entry and 4,096-dependency limits remain. Budget clears retain and account for reusable
table storage, releasing it only when necessary. A post-insertion check handles unexpected growth.
Validated memo hits skip unused metadata preparation; misses keep the same prefetch behavior.
Recorded dependency answers, recursion/cycle guards and native impurity tracking remain intact.

## Measurement provenance

These measurements were recorded on the local development integration before adapting the change
to this PR's current upstream base. They have not been rerun on the upstream-adapted branch and
are not a whole-PR performance claim. The standalone `graph_expression_profile` probe included in
`crates/expr/src/tests_perf.rs` makes the evaluator workload reproducible without a GUI.

Hardware: M1 Max, 10 cores, 64 GiB. Test-profile build, warmup excluded, three alternating
baseline/candidate process pairs, seven timed sweeps per process. Ordinary applications, including
After Effects, remained active; no compiler overlapped a trial. Table values are the median of
three run p50/p95 measurements. Each sweep hashes all numeric results and counts script executions.
Synthetic expressions use `Math.sin(time * 3) * 90`.

| Repeated sweep | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
|---|---:|---:|
| 1 property / 1,440 evaluations | 2.125 / 2.152 | 0.841 / 0.938 |
| 4 properties / 5,760 evaluations | 8.748 / 11.758 | 3.776 / 3.846 |
| 8 properties / 11,520 evaluations | 119.670 / 127.875 | 7.828 / 7.997 |
| 20 properties / 28,800 evaluations | 300.970 / 321.803 | 21.020 / 26.662 |
| 50 properties / 72,000 evaluations | 793.051 / 846.005 | 754.185 / 795.556 |

All five workload sizes had identical numeric checksums. Repeated 20-property sweeps execute zero
scripts after warmup instead of 28,800. The 50-property working set still exceeds the cache and
executes all 72,000 scripts; entry clears reuse the table without further growth during measured
sweeps. First visits to new times and more complex expressions still require evaluation.

Whole diagnostic-process peak RSS rose from 27.36 to 57.58 MiB (median of three runs, Darwin
`time -l`). The extra memory is a tradeoff; per-thread caches can add up across workers. The budget
is an estimate of retained memo storage, not a limit on allocator RSS, JS heap or application RAM.
The measurements do not establish input-to-visible-frame latency, GPU performance or arbitrary
expression fidelity. Four regressions cover working-set retention, oversized payloads, repeated
budget clears and replacement accounting. Current upstream adaptation checks are recorded in the
PR description; do not infer a full regression-suite pass from these timing probes.
