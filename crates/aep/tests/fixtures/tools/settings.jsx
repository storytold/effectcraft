// settings.jsx - combined fixtures of the settings family. Project settings hold one value per
// project, so the 11 cases go into three small projects; each combines as many non-default
// settings as do not conflict (bpc 8/16, the three time display styles), everything else stays at
// the 000-base defaults. One comp per case (the base comp, 902 with its layers); the equivalence
// check compares every setting a case changes against the old fixture and that the file changes
// no setting none of its cases changes. Needs fam-common.jsx.

var PROJECTS = [
    {
        out: "settings-a.aep",
        settings: { bitsPerChannel: 8, workingSpace: "", workingGamma: 2.4, compensateForSceneReferredProfiles: false, timeDisplayType: "frames", displayStartFrame: 1 },
        setup: baseTemplate(),
        cases: [
            { id: "621-settings-bpc-8", steps: [] },
            { id: "623-settings-workingspace-none", steps: [] },
            { id: "625-settings-gamma-2.4", steps: [] },
            { id: "628-settings-scene-compensation-off", steps: [] },
            { id: "629-settings-time-display-frames", steps: [] },
            { id: "630-settings-frame-start-1", steps: [] }
        ]
    },
    {
        out: "settings-b.aep",
        settings: { linearizeWorkingSpace: true, linearBlending: true, timeDisplayType: "frames", framesUseFeetFrames: true, feetFramesFilmType: "mm35" },
        setup: baseTemplate(),
        cases: [
            { id: "632-settings-linearize-and-blend-linear", steps: [] },
            { id: "633-settings-feet-frames-35", steps: [] }
        ]
    },
    {
        out: "settings-c.aep",
        settings: { bitsPerChannel: 16, workingSpace: "Rec.709 Gamma 2.4", timeDisplayType: "frames", framesUseFeetFrames: true, feetFramesFilmType: "mm16" },
        setup: baseTemplate().concat([imp("steps", "test-media/steps.png")]),
        cases: [
            { id: "622-settings-bpc-16", steps: [] },
            { id: "634-settings-feet-frames-16", steps: [] },
            { id: "902-settings-rec709-16bpc-steps-png-grey-solid", steps: [
                footage("steps", "S"),
                op("layer.create_solid", { comp: "$", name: "G", color: [0.5, 0.5, 0.5], width: 400, height: 200 }),
                setp("G", ["Transform", "Position"], [1600, 900])] }
        ]
    }
];
