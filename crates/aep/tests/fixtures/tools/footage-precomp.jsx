// footage-precomp.jsx - combined fixture of the footage-precomp family (stills, sequences, clips,
// PSD/AI/EPS, precomps, time remap): one comp per case of partition.txt, recipes from
// synthetic/MANIFEST.md. Footage items are imported once and shared by the cases (pilot lesson
// 4: layer sources are compared by what they are). Needs fam-common.jsx.

function precompose() { return op("comp.precompose", { comp: "$", layerIndices: [1], newName: "Pre", moveAll: true }); }

var PROJECTS = [{
    out: "footage-precomp.aep",
    settings: null,
    setup: baseTemplate().concat([
        imp("png8", "test8x8.png"),
        imp("wav", "test-tone.wav"),
        imp("seq", "test-seq/seq_001.png", { sequence: true }),
        imp("exrseq", "test-media/exrseq/shot_0101.exr", { sequence: true }),
        imp("mp4", "test-media/clip.mp4"),
        imp("tga", "test-media/still.tga"),
        imp("psd", "test-media/still.psd", { importAs: "footage" }),
        imp("ai", "test-media/still.ai", { importAs: "footage" }),
        imp("eps", "test-media/still.eps"),
        // 600/601: P30 (640x360, 30 fps, 3 s, green solid G640) and Q (320x240, 25 fps, 2 s, blue B320)
        tag("P30", op("comp.create", { name: "P30", width: 640, height: 360, fps: 30, duration: 3, bgColor: [0, 0, 0] })),
        op("layer.create_solid", { comp: "P30", name: "G640", color: [0, 1, 0], width: 640, height: 360 }),
        tag("Q", op("comp.create", { name: "Q", width: 320, height: 240, fps: 25, duration: 2, bgColor: [0, 0, 0] })),
        op("layer.create_solid", { comp: "Q", name: "B320", color: [0, 0, 1], width: 320, height: 240 })
    ]),
    cases: [
        { id: "124-precomp-Pre", steps: [precompose()] },
        { id: "126-footage-png-layer", steps: [footage("png8", "png")] },
        { id: "319-precomp-timeremap", steps: [precompose(), props("Pre", { timeRemapEnabled: true })] },
        // item only: the comp is the base, the item is checked at project level
        { id: "350-footage-wav-imported", steps: [] },
        { id: "351-footage-wav-as-layer", steps: [footage("wav")] },
        { id: "352-footage-pngseq-imported", steps: [] },
        { id: "354-footage-pngseq-timeremap", steps: [footage("seq"), props(1, { timeRemapEnabled: true })] },
        { id: "601-precomp-two-comps-in-C", steps: [footage("P30"), footage("Q")] },
        { id: "603-footage-exrseq-0101-as-layer", steps: [footage("exrseq")] },
        { id: "645-footage-mp4-conform-30", steps: [footage("mp4"), op("footage.interpret", { item: "@mp4", conformFrameRate: 30 })] },
        { id: "650-footage-tga-still-as-layer", steps: [footage("tga")] },
        { id: "651-footage-psd-flat-as-layer", steps: [footage("psd")] },
        { id: "652-footage-ai-as-layer", steps: [footage("ai")] },
        { id: "653-footage-eps-as-layer", steps: [footage("eps")] }
    ]
}];
