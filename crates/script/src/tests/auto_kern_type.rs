//! `TextDocument.autoKernType` and the `AutoKernType` enum (After Effects' numbers: 11412 none,
//! 11413 metrics, 11414 optical), over the document's `kerning`.

use std::sync::Arc;

use effectcraft_engine::Session;
use serde_json::json;

use crate::{Outcome, run_code};

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), expr_check: Some(effectcraft_expr::check_syntax), ..Default::default() };
    crate::install(&mut s);
    s
}

fn ok(s: &mut Session, code: &str) -> Outcome {
    let o = run_code(s, code, "test.jsx");
    assert!(o.error.is_none(), "script failed: {:?}\noutput: {:?}", o.error, o.output);
    o
}

#[test]
fn auto_kern_type_reads_and_sets_the_kerning() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var t = c.layers.addText("AVATAR");
        var p = t.property("Source Text"), out = [AutoKernType.NO_AUTO_KERN, AutoKernType.METRIC_KERN, AutoKernType.OPTICAL_KERN, p.value.autoKernType];
        var kinds = [AutoKernType.OPTICAL_KERN, AutoKernType.NO_AUTO_KERN, AutoKernType.METRIC_KERN];
        for (var i = 0; i < kinds.length; i++) { var d = p.value; d.autoKernType = kinds[i]; p.setValue(d); out.push(p.value.autoKernType); }
        var bad = "accepted"; try { var d = p.value; d.autoKernType = 5; } catch (e) { bad = "refused"; }
        out.push(bad);
        out
        "#,
    );
    assert_eq!(o.result, json!([11412, 11413, 11414, 11413, 11414, 11412, 11413, "refused"]));
}
