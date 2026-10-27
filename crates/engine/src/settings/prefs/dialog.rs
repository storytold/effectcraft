use super::choices::{APPEARANCE_MODES, DARK_THEMES, LANGUAGES, LIGHT_THEMES, WINDOW_GRAPHICS};
use super::*;

// ---------------------------------------------------------------- schema (Settings dialog)

/// What a setting row edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// Integer with range and unit.
    Int(i64, i64, &'static str),
    /// Number with range and unit.
    Float(f64, f64, &'static str),
    /// One of (label, value).
    Choice(&'static [(&'static str, &'static str)]),
    Text,
    /// A folder or file path (text + Choose…).
    Path,
    /// `#rrggbb`.
    Color,
    /// UI brightness slider.
    Slider(f64, f64),
}

/// One row of a settings page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Section(&'static str),
    Setting {
        key: &'static str,
        label: &'static str,
        kind: Kind,
        /// Changes behaviour today (false = shown for parity, listed in docs/preferences.md).
        live: bool,
    },
    /// A button running a command (`params` is JSON).
    Button {
        label: &'static str,
        command: &'static str,
        params: &'static str,
    },
    /// The 16 label names and colours.
    Labels,
    /// Output device picker (device list from the host).
    AudioDevices {
        key: &'static str,
    },
    /// Read-only information line.
    Note(&'static str),
    /// The browser's storage manager (web app only, `storage.*`): usage and quota, persistent
    /// storage, Clear buttons. Not shown where there is no browser storage.
    BrowserStorage,
    /// A task's trained models (`roto.models`, `face.models`): authors, licence, size, install,
    /// choose.
    Models(effectcraft_segment::Task),
}

/// One page of the Settings dialog.
#[derive(Clone, Debug)]
pub struct Page {
    pub id: &'static str,
    pub title: &'static str,
    pub items: Vec<Item>,
}

const fn s(key: &'static str, label: &'static str, kind: Kind, live: bool) -> Item {
    Item::Setting { key, label, kind, live }
}

const B: Kind = Kind::Bool;

const ON_OFF_RES: &[(&str, &str)] = &[("1/2", "1/2"), ("1/4", "1/4"), ("1/8", "1/8")];

const STYLES: &[(&str, &str)] = &[("Lines", "lines"), ("Dashed Lines", "dashed"), ("Dots", "dots")];

/// The Settings pages in After Effects 2026 order.
pub fn pages() -> Vec<Page> {
    use Item::*;
    vec![
        Page {
            id: "general",
            title: "General",
            items: vec![
                s("general.language", "Language", Kind::Choice(LANGUAGES), true),
                s("general.undoLevels", "Levels of Undo", Kind::Int(1, 99, ""), true),
                s("general.pathPointSize", "Path Point and Handle Size", Kind::Int(3, 20, "px"), true),
                s("general.recentItems", "Recent Projects Shown", Kind::Int(1, 30, ""), true),
                Section("Options"),
                s("general.showToolTips", "Show Tool Tips", B, true),
                s("general.createLayersAtCompStart", "Create Layers at Composition Start Time", B, true),
                s("general.switchesAffectNestedComps", "Switches Affect Nested Comps", B, true),
                s("general.defaultSpatialLinear", "Default Spatial Interpolation to Linear", B, true),
                s("general.preserveConstantVertexCount", "Preserve Constant Vertex and Feather Point Count when Editing Masks", B, true),
                s("general.syncTimeRelatedItems", "Synchronize Time of All Related Items", B, true),
                s("general.expressionPickWhipCompact", "Expression Pick Whip Writes Compact English", B, true),
                s("general.createSplitLayersAbove", "Create Split Layers Above Original Layer", B, true),
                s("general.useSystemColorPicker", "Use System Color Picker", B, false),
                Note("EffectCraft always uses its own colour picker (the same on every platform and the web)."),
            ],
        },
        Page {
            id: "startup",
            title: "Startup & Repair",
            items: vec![
                s("startup.showHomeOnLaunch", "Show Home Screen When Launching", B, true),
                s("startup.showHomeOnOpenProject", "Show Home Screen When Opening a Project", B, true),
                s("startup.offerCrashRecovery", "Offer to Open the Latest Auto-Save After a Crash", B, true),
                s("startup.windowGraphics", "Window Graphics", Kind::Choice(WINDOW_GRAPHICS), true),
                Note(
                    "Window Graphics applies from the next launch. When the graphics driver stops EffectCraft as its window opens, the next launch switches to OpenGL.",
                ),
                Section("Repair"),
                Button { label: "Reset Settings", command: "prefs.reset", params: "{}" },
                Button { label: "Reset Keyboard Shortcuts", command: "shortcuts.reset", params: "{}" },
                Button { label: "Empty Disk Cache", command: "edit.purge", params: r#"{"what":"disk"}"# },
            ],
        },
        Page {
            id: "project",
            title: "Project",
            items: vec![
                Section("New Project"),
                s("project.useTemplate", "New Project Loads Template", B, true),
                s("project.templatePath", "Template Project", Kind::Path, true),
                Section("Auto-Save"),
                s("autoSave.enabled", "Automatically Save Projects", B, true),
                s("autoSave.intervalMinutes", "Save Every", Kind::Int(1, 240, "Minutes"), true),
                s("autoSave.maxVersions", "Maximum Project Versions", Kind::Int(1, 99, ""), true),
                s("autoSave.location", "Auto-Save Location", Kind::Choice(&[("Next to Project", "nextToProject"), ("Custom Location", "custom")]), true),
                s("autoSave.folder", "Custom Location", Kind::Path, true),
                s("autoSave.saveOnRenderStart", "Save When Starting Render Queue", B, true),
                Button { label: "Save Now", command: "file.autoSave", params: "{}" },
            ],
        },
        Page {
            id: "composition",
            title: "Composition",
            items: vec![
                Section("Motion Path"),
                s(
                    "composition.motionPath",
                    "Motion Path",
                    Kind::Choice(&[
                        ("All Keyframes", "all"),
                        ("No Motion Path", "none"),
                        ("No More Than (Seconds)", "seconds"),
                        ("No More Than (Keyframes)", "keyframes"),
                    ]),
                    true,
                ),
                s("composition.motionPathSeconds", "Seconds", Kind::Float(0.0, 3600.0, "s"), true),
                s("composition.motionPathKeyframes", "Keyframes", Kind::Int(1, 10_000, ""), true),
                Section("Display"),
                s("composition.showRenderingProgress", "Show Rendering Progress in Info Panel and Flowchart", B, true),
                s("composition.hardwareAcceleratePanels", "Hardware Accelerate Composition, Layer and Footage Panels", B, false),
                Note("Panels are always drawn by the GPU (wgpu)."),
            ],
        },
        Page {
            id: "previews",
            title: "Previews",
            items: vec![
                s("previews.adaptiveResolutionLimit", "Adaptive Resolution Limit", Kind::Choice(ON_OFF_RES), true),
                s("previews.showInternalWireframes", "Show Internal Wireframes", B, true),
                s("previews.cacheFramesWhenIdle", "Cache Frames When Idle", B, true),
                s("previews.fastPreviews", "Fast Previews (Draft 3D, Faster Effects)", B, true),
                s("previews.zoomQuality", "Viewer Zoom Quality", Kind::Choice(&[("Faster", "faster"), ("More Accurate", "moreAccurate")]), true),
                s("previews.displayProfile", "Display Color Space", Kind::Choice(&[("sRGB", "srgb"), ("Display P3", "p3")]), true),
                Button { label: "GPU Information...", command: "app.gpuInfo", params: "{}" },
            ],
        },
        Page {
            id: "appearance",
            title: "Appearance",
            items: vec![
                s("appearance.appearanceMode", "Appearance Mode", Kind::Choice(APPEARANCE_MODES), true),
                s("appearance.lightTheme", "Light Theme", Kind::Choice(LIGHT_THEMES), true),
                s("appearance.darkTheme", "Dark Theme", Kind::Choice(DARK_THEMES), true),
                s("appearance.brightness", "Brightness", Kind::Slider(-1.0, 1.0), true),
                s("appearance.uiScale", "UI Scale", Kind::Int(75, 200, "%"), true),
                Section("Labels and Colors"),
                s("appearance.useLabelColorForHandles", "Use Label Color for Layer Handles and Paths", B, true),
                s("appearance.useLabelColorForTabs", "Use Label Color for Related Tabs", B, true),
                s("appearance.cycleMaskColors", "Cycle Mask Colors", B, true),
                s("appearance.useGradients", "Use Gradients", B, true),
                Section("Menu Bar"),
                s("appearance.inWindowMenuBarMac", "Use In-Window Menu Bar on macOS", B, true),
            ],
        },
        Page {
            id: "grids",
            title: "Grids & Guides",
            items: vec![
                Section("Grid"),
                s("grids.gridColor", "Color", Kind::Color, true),
                s("grids.gridSpacing", "Gridline Every", Kind::Float(1.0, 10_000.0, "px"), true),
                s("grids.gridSubdivisions", "Subdivisions", Kind::Int(1, 100, ""), true),
                s("grids.gridStyle", "Style", Kind::Choice(STYLES), true),
                Section("Proportional Grid"),
                s("grids.proportionalHorizontal", "Horizontal", Kind::Int(1, 100, ""), true),
                s("grids.proportionalVertical", "Vertical", Kind::Int(1, 100, ""), true),
                Section("Guides"),
                s("grids.guideColor", "Color", Kind::Color, true),
                s("grids.guideStyle", "Style", Kind::Choice(STYLES), true),
                Section("Safe Margins"),
                s("grids.actionSafe", "Action-safe", Kind::Float(0.0, 50.0, "%"), true),
                s("grids.titleSafe", "Title-safe", Kind::Float(0.0, 50.0, "%"), true),
            ],
        },
        Page { id: "labels", title: "Labels", items: vec![Labels] },
        Page {
            id: "type",
            title: "Type",
            items: vec![
                s("type.textEngine", "Text Engine", Kind::Choice(&[("Latin", "latin"), ("South Asian and Middle Eastern", "southAsian")]), true),
                s("type.fontPreview", "Show Font Preview", B, true),
                s("type.recentFonts", "Number of Recent Fonts to Display", Kind::Int(0, 30, ""), true),
                s("type.fontNamesInEnglish", "Show Font Names in English", B, true),
            ],
        },
        Page {
            id: "import",
            title: "Import",
            items: vec![
                s("import.stillFootage", "Still Footage", Kind::Choice(&[("Length of Composition", "compLength"), ("Duration (Seconds)", "seconds")]), true),
                s("import.stillSeconds", "Still Duration", Kind::Float(0.04, 86_400.0, "s"), true),
                s("import.sequenceFps", "Sequence Footage", Kind::Float(1.0, 999.0, "frames per second"), true),
                s("import.reportMissingFrames", "Report Missing Frames", B, true),
                s(
                    "import.autoReloadFootage",
                    "Automatically Reload Footage",
                    Kind::Choice(&[("Off", "off"), ("Non-Sequence Footage", "nonSequence"), ("All Footage", "all")]),
                    true,
                ),
                s(
                    "import.unlabeledAlpha",
                    "Interpret Unlabeled Alpha As",
                    Kind::Choice(&[
                        ("Ask User", "ask"),
                        ("Guess", "guess"),
                        ("Ignore", "ignore"),
                        ("Straight", "straight"),
                        ("Premultiplied", "premultiplied"),
                    ]),
                    true,
                ),
                s(
                    "import.dragImportAs",
                    "Default Drag Import As",
                    Kind::Choice(&[("Footage", "footage"), ("Composition", "comp"), ("Composition - Retain Layer Sizes", "compLayerSizes")]),
                    true,
                ),
            ],
        },
        Page {
            id: "export",
            title: "Export",
            items: vec![
                s("export.defaultOutputFolder", "Default Output Folder", Kind::Path, true),
                s("export.segmentSequences", "Segment Sequences", B, true),
                s("export.segmentSequenceFiles", "Files per Segment", Kind::Int(1, 100_000, "files"), true),
                s("export.segmentMovies", "Segment Movie Files", B, true),
                s("export.segmentMovieMb", "Segment Size", Kind::Int(1, 1_000_000, "MB"), true),
                s("export.appendBitsToName", "Append Bit Depth to File Name", B, true),
            ],
        },
        Page {
            id: "audio",
            title: "Audio",
            items: vec![
                Section("Audio Hardware"),
                AudioDevices { key: "audio.outputDevice" },
                s("audio.previewSampleRate", "Preview Sample Rate", Kind::Int(8_000, 192_000, "Hz"), true),
                Section("Audio Output Mapping"),
                s("audio.outputLeft", "Left", Kind::Int(1, 64, "channel"), true),
                s("audio.outputRight", "Right", Kind::Int(1, 64, "channel"), true),
            ],
        },
        Page {
            id: "disk",
            title: "Disk",
            items: vec![
                Section("Disk Cache"),
                s("disk.diskCacheEnabled", "Enable Disk Cache", B, true),
                s("disk.diskCacheMaxGb", "Maximum Disk Cache Size", Kind::Int(1, 100_000, "GB"), true),
                s("disk.diskCacheFolder", "Disk Cache Folder", Kind::Path, true),
                Button { label: "Empty Disk Cache", command: "edit.purge", params: r#"{"what":"disk"}"# },
                BrowserStorage,
                Section("Media Cache"),
                s("disk.mediaCacheFolder", "Database and Cache Folder", Kind::Path, true),
                s("disk.conformedMediaFolder", "Conformed Audio Folder", Kind::Path, true),
            ],
        },
        Page {
            id: "memory",
            title: "Memory & CPU",
            items: vec![
                s("memory.ramReservedGb", "RAM Reserved for Other Applications", Kind::Int(0, 1024, "GB"), true),
                Section("Cache Budgets"),
                s("memory.layerCacheMb", "Layer Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.mediaCacheMb", "Footage Frame Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.previewCacheMb", "Preview (RAM) Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.reduceCacheWhenLow", "Reduce Cache Size When System Is Low on Memory", B, true),
            ],
        },
        Page {
            id: "video",
            title: "Video",
            items: vec![
                s("video.enableOutput", "Enable Video Preview Output", B, true),
                s("video.device", "Video Device", Kind::Choice(&[("Floating Window", ""), ("Full Screen", "fullscreen")]), true),
                s("video.outputDuringPlayback", "Video Output During Playback", B, true),
                s("video.mirrorOnMonitor", "Mirror on Computer Monitor", B, true),
                s("video.disableWhenBackground", "Disable Video Output When in Background", B, true),
            ],
        },
        Page {
            id: "3d",
            title: "3D",
            items: vec![
                s("threeD.defaultRenderer", "Default 3D Renderer", Kind::Choice(&[("Classic 3D", "classic"), ("Advanced 3D", "advanced")]), true),
                s("threeD.showReferenceAxes", "Show 3D Reference Axes", B, true),
                s("threeD.extendedViewer", "Extended Viewer", B, true),
                s("threeD.realtimeShadows", "Realtime Shadows in Draft", B, true),
            ],
        },
        Page {
            id: "scripting",
            title: "Scripting & Expressions",
            items: vec![
                Section("Application Scripting"),
                s("scripting.allowScriptsWriteFiles", "Allow Scripts to Write Files and Access Network", B, true),
                s("scripting.warnExecutingFiles", "Warn User When Executing Files", B, true),
                s("scripting.enableJsDebugger", "Enable JavaScript Debugger", B, false),
                Section("Expressions Editor"),
                s("scripting.editorFontSize", "Font Size", Kind::Int(8, 40, "pt"), true),
                s("scripting.syntaxHighlighting", "Syntax Highlighting", B, true),
                s("scripting.lineNumbers", "Line Numbers", B, true),
                s("scripting.autoComplete", "Auto-complete", B, true),
                s("scripting.bracketMatching", "Bracket Matching", B, true),
                s("scripting.wordWrap", "Word Wrap", B, true),
                s("scripting.errorBanner", "Show Expression Error Banner", B, true),
                Note("Expressions use the JavaScript engine. Script errors report their file and line in the Script Console; there is no step debugger."),
            ],
        },
        Page {
            id: "roto",
            title: "Roto Brush",
            items: vec![
                Section("Segmentation Model"),
                Note(
                    "Roto Brush 2.0 and 3.0 use the model chosen here (Version 1.0 always uses the classic engine). Models are open source, downloaded only when you ask, and checked against their published SHA-256.",
                ),
                Models(effectcraft_segment::Task::Mask),
            ],
        },
        Page {
            id: "face",
            title: "Face Tracking",
            items: vec![
                Section("Face Model"),
                Note(
                    "Face tracking in the Tracker panel (Outline Only and Detailed Features) uses the model chosen here. Models are open source, downloaded only when you ask, and checked against their published SHA-256.",
                ),
                Models(effectcraft_segment::Task::Face),
            ],
        },
    ]
}

/// Keys of every setting that doesn't change behaviour yet (the TODO list in
/// `docs/preferences.md`).
pub fn todo_keys() -> Vec<&'static str> {
    pages()
        .into_iter()
        .flat_map(|p| p.items)
        .filter_map(|i| match i {
            Item::Setting { key, live: false, .. } => Some(key),
            _ => None,
        })
        .collect()
}

/// The schema as JSON (for `prefs.pages`).
pub fn pages_json() -> Value {
    let kind = |k: &Kind| -> Value {
        match k {
            Kind::Bool => json!({"type": "bool"}),
            Kind::Int(a, b, u) => json!({"type": "int", "min": a, "max": b, "unit": u}),
            Kind::Float(a, b, u) => json!({"type": "number", "min": a, "max": b, "unit": u}),
            Kind::Choice(o) => json!({"type": "choice", "options": o.iter().map(|(l, v)| json!({"label": l, "value": v})).collect::<Vec<_>>()}),
            Kind::Text => json!({"type": "text"}),
            Kind::Path => json!({"type": "path"}),
            Kind::Color => json!({"type": "color"}),
            Kind::Slider(a, b) => json!({"type": "slider", "min": a, "max": b}),
        }
    };
    let pages: Vec<Value> = pages()
        .iter()
        .map(|p| {
            let items: Vec<Value> = p
                .items
                .iter()
                .map(|i| match i {
                    Item::Section(t) => json!({"section": t}),
                    Item::Setting { key, label, kind: k, live } => {
                        let mut o = kind(k);
                        o["key"] = json!(key);
                        o["label"] = json!(label);
                        o["live"] = json!(live);
                        o
                    }
                    Item::Button { label, command, params } => {
                        json!({"button": label, "command": command, "params": serde_json::from_str::<Value>(params).unwrap_or_default()})
                    }
                    Item::Labels => json!({"labels": "labels.N.name / labels.N.color (N = 0..15)"}),
                    Item::AudioDevices { key } => json!({"type": "device", "key": key, "label": "Default Output"}),
                    Item::Note(t) => json!({"note": t}),
                    Item::BrowserStorage => json!({"browserStorage": "storage.info / storage.persist / storage.clear (web app)"}),
                    Item::Models(task) => {
                        let p = if *task == effectcraft_segment::Task::Face { "face" } else { "roto" };
                        json!({"models": format!("{p}.models / {p}.model.select / {p}.model.download / {p}.model.install / {p}.model.remove"), "key": format!("{p}.model")})
                    }
                })
                .collect();
            json!({"id": p.id, "title": p.title, "items": items})
        })
        .collect();
    json!(pages)
}
