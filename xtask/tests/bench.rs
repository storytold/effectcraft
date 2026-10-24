//! Regression gates must fail when benchmark evidence is corrupt or incomplete.
use serde_json::{Value, json};
use std::process::{Command, Output};

struct ResultsDir(tempfile::TempDir);
impl ResultsDir {
    fn new() -> Self {
        Self(tempfile::Builder::new().prefix("effectcraft-bench-").tempdir().unwrap())
    }
    fn compare(&self, old: &str, new: &str, gate: bool) -> Output {
        let baseline = self.0.path().join("baseline.jsonl");
        let candidate = self.0.path().join("candidate.jsonl");
        std::fs::write(&baseline, old).unwrap();
        std::fs::write(&candidate, new).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command.args(["bench", "--compare"]).arg(baseline).arg("--with").arg(candidate);
        if gate {
            command.args(["--fail-over", "5"]);
        }
        command.output().unwrap()
    }
}

fn row(scenario: &str, ok: bool) -> Value {
    json!({"scenario": scenario, "run": "full", "ok": ok,
        "settings": {"resolution": "full", "frames": 60, "format": "png"},
        "metrics": {"wall_s": 1.0, "cpu_s": 2.0, "peak_footprint_bytes": 1000, "max_rss_bytes": null, "instructions": null}})
}
fn diagnostic(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn corrupt_records_report_the_input_file_and_line() {
    let files = ResultsDir::new();
    let valid = row("demo", true);
    let mut missing = valid.clone();
    missing["settings"].as_object_mut().unwrap().remove("frames");
    let mut missing_wall = valid.clone();
    missing_wall["metrics"].as_object_mut().unwrap().remove("wall_s");
    let mut bad_metric = valid.clone();
    bad_metric["metrics"]["cpu_s"] = json!("not a measurement");
    let mut negative_metric = valid.clone();
    negative_metric["metrics"]["cpu_s"] = json!(-1);
    for bad in [
        "{".into(),
        "null".into(),
        "[]".into(),
        "{}".into(),
        missing.to_string(),
        missing_wall.to_string(),
        bad_metric.to_string(),
        negative_metric.to_string(),
    ] {
        let output = files.compare(&format!("{valid}\n"), &format!("{valid}\n\n \n{bad}\n"), true);
        let message = diagnostic(&output);
        assert!(!output.status.success(), "corrupt input passed: {bad}\n{message}");
        assert!(message.contains("candidate.jsonl:4:"), "{message}");
    }
    let output = files.compare("{\n", &format!("{valid}\n"), false);
    assert!(!output.status.success());
    assert!(diagnostic(&output).contains("baseline.jsonl:1:"));
}

#[test]
fn gates_reject_failed_latest_runs_and_missing_coverage() {
    let files = ResultsDir::new();
    let good = row("demo", true);
    let failed = row("demo", false);
    let other = row("layers", true);
    for (old, new) in [
        (good.to_string(), String::new()),
        (String::new(), String::new()),
        (good.to_string(), failed.to_string()),
        (good.to_string(), format!("{good}\n{failed}")),
        (format!("{good}\n{failed}"), good.to_string()),
        (good.to_string(), other.to_string()),
        (format!("{good}\n{other}"), good.to_string()),
        (good.to_string(), format!("{good}\n{other}")),
    ] {
        let output = files.compare(&old, &new, true);
        assert!(!output.status.success(), "incomplete gate passed: {old} -> {new}\n{}", diagnostic(&output));
    }
}

#[test]
fn gates_require_a_comparable_resource_metric_for_every_run() {
    let files = ResultsDir::new();
    let good = row("demo", true);
    let mut wall_only = good.clone();
    wall_only["metrics"]["cpu_s"] = Value::Null;
    wall_only["metrics"]["peak_footprint_bytes"] = Value::Null;
    let output = files.compare(&good.to_string(), &wall_only.to_string(), true);
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("comparable"));
    let mut zero = good.clone();
    zero["metrics"]["cpu_s"] = json!(0);
    zero["metrics"]["peak_footprint_bytes"] = json!(0);
    assert!(!files.compare(&zero.to_string(), &good.to_string(), true).status.success());
    let mut mixed = row("layers", true);
    mixed["metrics"] = wall_only["metrics"].clone();
    assert!(!files.compare(&format!("{good}\n{mixed}"), &format!("{good}\n{mixed}"), true).status.success());
}

#[test]
fn valid_results_keep_comparison_and_exploratory_mode() {
    let files = ResultsDir::new();
    let good = row("demo", true);
    let failed = row("demo", false);
    let output = files.compare(&format!("\n{failed}\n{good}\n"), &format!("{good}\n \n"), true);
    assert!(output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("+0.0%"));
    let mut open = good.clone();
    open["run"] = json!("open");
    open["settings"]["resolution"] = json!("-");
    open["settings"]["frames"] = json!(0);
    assert!(files.compare(&open.to_string(), &open.to_string(), true).status.success(), "project-open runs have zero rendered frames");
    let mut regression = good.clone();
    regression["metrics"]["cpu_s"] = json!(3);
    assert!(!files.compare(&good.to_string(), &regression.to_string(), true).status.success());
    let output = files.compare(&good.to_string(), &format!("{good}\n{failed}"), false);
    assert!(output.status.success(), "exploratory comparison stays available");
    assert!(diagnostic(&output).contains("failed"), "latest failure is visible: {}", diagnostic(&output));
}

/// With `--repeat`, a summary row (metrics = medians) follows the runs; comparison uses it.
/// The last raw run (cpu 2.0, +0 %) differs from the median (2.2, +10 %), so only the summary fails
/// the 5 % gate. Both sides are summaries, so the mixed-sample check stays out of the way.
#[test]
fn repeat_summary_rows_are_compared_by_median() {
    let dir = ResultsDir::new();
    let summary = |cpu: f64| {
        let mut r = row("demo", true);
        r["metrics"]["cpu_s"] = json!(cpu);
        r["kind"] = json!("summary");
        r["summary"] = json!({"runs": 3, "failed": 0, "stats": {}});
        format!("{r}\n")
    };
    let baseline = summary(2.0);
    let mut candidate = String::new();
    for wall in [1.1, 9.0, 1.0] {
        let mut r = row("demo", true);
        r["metrics"]["cpu_s"] = json!(wall * 2.0);
        candidate.push_str(&format!("{r}\n"));
    }
    let raw_only = dir.compare(&baseline, &candidate, false);
    assert!(diagnostic(&raw_only).contains("median of 3→1 run"), "{}", diagnostic(&raw_only));
    candidate.push_str(&summary(2.2));
    let output = dir.compare(&baseline, &candidate, true);
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(diagnostic(&output).contains("cpu_s +10.0%"), "{}", diagnostic(&output));
}
