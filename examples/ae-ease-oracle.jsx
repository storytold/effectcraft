// Original behavior probe. Run this function body through AEsync's `run -f` command.
// Creates only a temporary text layer and removes its composition before returning.
// No Adobe assets, application internals, presets or implementation code are read.
var comp = null, layer, p, cases, results = [], times = [], i, j, c, values, output;
cases = [
    ["easy", 0, 100, 0, 100 / 3, 0, 100 / 3],
    ["high_influence", 0, 100, 0, 80, 0, 80],
    ["asymmetric", 0, 100, 20, 80, 80, 60],
    ["overshoot", 0, 100, 400, 40, -100, 40],
    ["descending", 100, 0, -20, 60, -80, 80],
    ["equal_endpoints", 50, 50, 100, 40, -100, 40],
    ["maximum_influence", 0, 100, 0, 100, 0, 100],
    ["minimum_maximum", 0, 100, 0, 0.1, 0, 100]
];
for (i = 0; i <= 20; i++) times.push(i / 20);
try {
    comp = app.project.items.addComp("Visualab Effects original ease oracle", 64, 64, 1, 2, 20);
    layer = comp.layers.addText("Original behavioral probe");
    p = layer.property("ADBE Transform Group").property("ADBE Rotate Z");
    for (i = 0; i < cases.length; i++) {
        c = cases[i];
        while (p.numKeys > 0) p.removeKey(p.numKeys);
        p.setValueAtTime(0, c[1]);
        p.setValueAtTime(1, c[2]);
        p.setInterpolationTypeAtKey(1, KeyframeInterpolationType.BEZIER, KeyframeInterpolationType.BEZIER);
        p.setInterpolationTypeAtKey(2, KeyframeInterpolationType.BEZIER, KeyframeInterpolationType.BEZIER);
        p.setTemporalAutoBezierAtKey(1, false);
        p.setTemporalAutoBezierAtKey(2, false);
        p.setTemporalContinuousAtKey(1, false);
        p.setTemporalContinuousAtKey(2, false);
        p.setTemporalEaseAtKey(1, [new KeyframeEase(0, 100 / 3)], [new KeyframeEase(c[3], c[4])]);
        p.setTemporalEaseAtKey(2, [new KeyframeEase(c[5], c[6])], [new KeyframeEase(0, 100 / 3)]);
        values = [];
        for (j = 0; j < times.length; j++) values.push(p.valueAtTime(times[j], true));
        results.push({name:c[0], from:c[1], to:c[2], outSpeed:c[3], outInfluence:c[4], inSpeed:c[5], inInfluence:c[6], times:times, values:values, applied:{outSpeed:p.keyOutTemporalEase(1)[0].speed, outInfluence:p.keyOutTemporalEase(1)[0].influence, inSpeed:p.keyInTemporalEase(2)[0].speed, inInfluence:p.keyInTemporalEase(2)[0].influence, outAuto:p.keyTemporalAutoBezier(1), inAuto:p.keyTemporalAutoBezier(2), outContinuous:p.keyTemporalContinuous(1), inContinuous:p.keyTemporalContinuous(2)}});
    }
    output = {aeVersion:app.version, cases:results};
    comp.remove();
    comp = null;
    return JSON.stringify(output);
} catch (e) {
    if (comp !== null) comp.remove();
    throw e;
}
