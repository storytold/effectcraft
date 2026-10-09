//! Frontend-only commands: viewer zoom/resolution/overlays, panels, workspaces, dialogs and app
//! chrome. They live in the registry so every menu entry is a command (menus, shortcuts, the
//! control channel and MCP all see them); running one emits [`crate::Event::Frontend`] and the UI
//! performs it. Headless sessions accept and ignore them.

use super::{CommandSpec, always, frontend, has_comp, has_layers};
use crate::cmd;

macro_rules! fe {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr) => {
        cmd!($id, $label, [$($m),*], $sc, $params, $en, |s, p| frontend(s, $id, p))
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // Application.
        fe!("app.about", "About EffectCraft...", [], None, "{}", always),
        fe!("layer.style.options", "Layer Style Options...", ["Layer", "Layer Styles"], None, "{layer?, style?: blendingOptions|dropShadow|…}", has_layers),
        fe!(
            "app.settings",
            "Settings...",
            [],
            None,
            "{page?: general|startup|project|composition|previews|appearance|grids|labels|type|import|export|audio|disk|memory|video|3d|scripting}",
            always
        ),
        fe!("app.gpuInfo", "GPU Information...", [], None, "{}", always),
        fe!("app.hide", "Hide EffectCraft", [], None, "{}", always),
        // macOS: the native menu's Hide Others / Show All (the desktop app hides the other apps).
        fe!("app.hideOthers", "Hide Others", [], None, "{}", always),
        fe!("app.showAll", "Show All", [], None, "{}", always),
        fe!("app.quit", "Quit EffectCraft", [], Some("Cmd+Q"), "{}", always),
        fe!("app.commandPalette", "Quick Apply...", ["Edit"], Some("Cmd+Shift+Space"), "{query?}", always),
        fe!("app.keyboardShortcuts", "Keyboard Shortcuts", ["Edit"], Some("Cmd+Alt+'"), "{}", always),
        fe!("app.templates", "Templates", [], None, "{kind: renderSettings|outputModule}", always),
        fe!("app.find", "Find", ["File"], Some("Cmd+F"), "{query?}", always),
        // Preview.
        fe!(
            "playback.toggle",
            "Play Current Preview",
            ["Composition", "Preview"],
            Some("Space"),
            "{shortcut?: spacebar|shiftSpacebar|numpad0|shiftNumpad0|altNumpad0 (whose Preview panel options to use; default spacebar)}",
            has_comp
        ),
        fe!("playback.cacheWhenIdle", "Cache Frames When Idle", ["Composition", "Preview"], None, "{value?}", always),
        fe!("playback.audio", "Audio", ["Composition", "Preview"], None, "{value?: include audio in previews}", always),
        fe!(
            "playback.scrubAudio",
            "Scrub Audio",
            [],
            None,
            "{time? (s, default the current time)} — plays one frame of the comp's audio there (Ctrl/Cmd-drag the current time)",
            has_comp
        ),
        // Viewer.
        fe!("view.zoomIn", "Zoom In", ["View"], Some("."), "{}", has_comp),
        fe!("view.zoomOut", "Zoom Out", ["View"], Some(","), "{}", has_comp),
        fe!("view.res.full", "Full", ["View", "Resolution"], Some("Cmd+J"), "{}", has_comp),
        fe!("view.res.half", "Half", ["View", "Resolution"], Some("Cmd+Shift+J"), "{}", has_comp),
        fe!("view.res.third", "Third", ["View", "Resolution"], None, "{}", has_comp),
        fe!("view.res.quarter", "Quarter", ["View", "Resolution"], Some("Cmd+Alt+Shift+J"), "{}", has_comp),
        fe!("view.res.custom", "Custom...", ["View", "Resolution"], None, "{factor?: 1..40 (render every n-th pixel)}", has_comp),
        fe!("view.rulers", "Show Rulers", ["View"], Some("Cmd+R"), "{value?}", always),
        fe!("view.panelBackground", "Panel Background Color", [], None, "{color?: black|darkGray|mediumGray|lightGray|white|custom|#hex, pick?: true}", always),
        fe!("view.guides", "Show Guides", ["View"], Some("Cmd+;"), "{value?}", always),
        fe!("view.snapToGuides", "Snap to Guides", ["View"], Some("Cmd+Shift+;"), "{value?}", always),
        fe!("view.lockGuides", "Lock Guides", ["View"], Some("Cmd+Alt+Shift+;"), "{value?}", always),
        fe!("view.grid", "Show Grid", ["View"], Some("Cmd+'"), "{value?}", always),
        fe!("view.snapToGrid", "Snap to Grid", ["View"], Some("Cmd+Shift+'"), "{value?}", always),
        fe!("view.options", "View Options...", ["View"], Some("Cmd+Alt+U"), "{}", has_comp),
        fe!("view.layerControls", "Show Layer Controls", ["View"], Some("Cmd+Shift+H"), "{value?}", always),
        fe!("view.fullScreen", "Enter Full Screen", ["View"], Some("Ctrl+Cmd+F"), "{}", always),
        fe!(
            "view.newViewer",
            "New Viewer",
            ["View"],
            Some("Alt+Shift+N"),
            "{} — another Composition viewer of the active comp; the one in use is locked",
            has_comp
        ),
        // Panels and workspaces.
        fe!(
            "window.panel",
            "Show Panel",
            [],
            None,
            "{panel: project|effectControls|composition|layer|timeline|info|audio|preview|effectsPresets|properties|character|paragraph|align|tracker|wiggler|smoother|motionSketch|paint|brushes|renderQueue|flowchart|history|markers|tools|lumetriScopes|footage|mediaBrowser|metadata|progress|contentAwareFill|createNullsFromPaths|vrCompEditor}",
            always
        ),
        fe!(
            "window.workspace",
            "Workspace",
            [],
            None,
            "{name: Default|Standard|Small Screen|Animation|Effects|Motion Tracking|Paint|Text|Minimal|All Panels}",
            always
        ),
        fe!("window.saveWorkspace", "Save Changes to this Workspace", ["Window", "Workspace"], None, "{}", always),
        fe!("window.saveWorkspaceAs", "Save as New Workspace...", ["Window", "Workspace"], None, "{name}", always),
        fe!("window.editWorkspaces", "Edit Workspaces...", ["Window", "Workspace"], None, "{name, rename?: new name, delete?: bool}", always),
        fe!("window.resetWorkspace", "Reset to Saved Layout", ["Window", "Workspace"], None, "{}", always),
        fe!("comp.flowchart", "Composition Flowchart", ["Composition"], Some("Cmd+Shift+F11"), "{}", always),
        fe!("comp.miniFlowchart", "Composition Mini-Flowchart", ["Composition"], Some("Tab"), "{}", has_comp),
        fe!("layer.openLayer", "Open Layer", ["Layer"], None, "{layer?}", has_layers),
        fe!("effect.manage", "Manage Effects...", ["Effect"], None, "{}", always),
        fe!("anim.browsePresets", "Browse Presets...", ["Animation"], None, "{}", always),
        // Project panel: delete the selected items without the prompt that Delete shows when
        // compositions use them. The UI runs `project.delete` (agents can run it directly).
        fe!(
            "project.deleteWithoutConfirmation",
            "Delete Project Items Without Confirmation",
            [],
            Some("Shift+Delete"),
            "{} — the Project panel's selection, with the layers that use it; one undo step",
            always
        ),
    ]
}
