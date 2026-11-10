// markers.jsx - combined fixture of the markers family: one comp per case of partition.txt,
// recipes from synthetic/MANIFEST.md (groups J, 12, 249, 509/510). Needs fam-common.jsx.

function cm(a) { a.comp = "$"; return op("marker.add_comp", a); }
function lm(a) { a.comp = "$"; a.layer = "L"; return op("marker.add_layer", a); }

var PROJECTS = [{
    out: "markers.aep",
    settings: null,
    setup: baseTemplate(),
    cases: [
        { id: "122-marker-comp-1s", steps: [cm({ time: 1, comment: "CompMarker" })] },
        { id: "123-marker-layer-1s", steps: [lm({ time: 1, comment: "LayerMarker" })] },
        { id: "249-marker-layer-full", steps: [lm({ time: 1, comment: "LM", duration: 1, chapter: "Chap", url: "http://a.example/u", frameTarget: "_blank", cuePointName: "CueX" })] },
        { id: "347-marker-layer-label3", steps: [lm({ time: 1, comment: "M", label: 3 })] },
        { id: "348-marker-layer-navigation-cue", steps: [lm({ time: 1, comment: "M", cuePointName: "CueN", eventCuePoint: false })] },
        { id: "349-marker-layer-event-cue-params", steps: [lm({ time: 1, comment: "M", cuePointName: "CueE", eventCuePoint: true, params: { speaker: "A", n: "2" } })] },
        { id: "509-marker-comp-two", steps: [cm({ time: 1, comment: "M1", label: 2 }), cm({ time: 2.5, comment: "M2", duration: 0.5 })] },
        { id: "510-marker-layer-0.52-dur0.3", steps: [lm({ time: 0.52, comment: "LM", duration: 0.3 })] }
    ]
}];
