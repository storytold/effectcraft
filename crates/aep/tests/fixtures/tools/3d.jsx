// 3d.jsx - combined fixture of the 3D family (cameras, lights, orientation, 3D layers):
// one comp per case of partition.txt, recipes from synthetic/MANIFEST.md. Needs fam-common.jsx.

function cam(name) { return op("layer.create_camera", { comp: "$", name: name }); }
function light(name, type) { return op("layer.create_light", { comp: "$", name: name, lightType: type }); }
var CAM = cam("Camera 1");
var SPOT = light("Light 1", "spot");
var ON3D = props("L", { threeDLayer: true });
var ROTX30 = setp("L", ["Transform", "X Rotation"], 30);

var PROJECTS = [{
    out: "3d.aep",
    settings: null,
    setup: baseTemplate(),
    cases: [
        { id: "015-layer-3d", steps: [ON3D] },
        { id: "039-comp-renderer-advanced3d", steps: [op("comp.set_renderer", { comp: "$", renderer: "advanced3d" })] },
        { id: "251-camera-one-node", steps: [CAM, props("Camera 1", { autoOrient: "off" })] },
        { id: "252-light-point", steps: [light("Light 1", "point")] },
        { id: "253-camera-position-100-200-m1000", steps: [CAM, setp("Camera 1", ["Transform", "Position"], [100, 200, -1000])] },
        { id: "254-light-spot-intensity-50", steps: [SPOT, setp("Light 1", ["Light Options", "Intensity"], 50)] },
        { id: "256-camera-two-node-default", steps: [CAM] },
        { id: "341-camera-zoom-2000", steps: [CAM, setp("Camera 1", ["Camera Options", "Zoom"], 2000)] },
        { id: "342-camera-dof-800-30", steps: [CAM, setp("Camera 1", ["Camera Options", "Depth of Field"], 1), setp("Camera 1", ["Camera Options", "Focus Distance"], 800), setp("Camera 1", ["Camera Options", "Aperture"], 30)] },
        { id: "343-light-spot-color-red", steps: [SPOT, setp("Light 1", ["Light Options", "Color"], [1, 0, 0, 1])] },
        { id: "344-light-spot-coneangle-60", steps: [SPOT, setp("Light 1", ["Light Options", "Cone Angle"], 60)] },
        { id: "345-light-spot-conefeather-30", steps: [SPOT, setp("Light 1", ["Light Options", "Cone Feather"], 30)] },
        { id: "485-layer-3d-on", steps: [ON3D] },
        { id: "486-layer-3d-rotx-30", steps: [ON3D, ROTX30] },
        { id: "487-layer-3d-roty-45", steps: [ON3D, setp("L", ["Transform", "Y Rotation"], 45)] },
        { id: "488-layer-3d-orientation-10-20-30", steps: [ON3D, setp("L", ["Transform", "Orientation"], [10, 20, 30])] },
        { id: "535-camera-position-on-3d-rotx", steps: [ON3D, ROTX30, cam("K"), setp("K", ["Transform", "Position"], [100, 200, -1000])] },
        { id: "536-camera-one-node-position-on-3d-rotx", steps: [ON3D, ROTX30, cam("K"), setp("K", ["Transform", "Position"], [100, 200, -1000]), props("K", { autoOrient: "off" })] },
        { id: "537-camera-default-on-3d-rotx", steps: [ON3D, ROTX30, cam("K")] },
        { id: "701-light-parallel-int80-color", steps: [light("P", "parallel"), setp("P", ["Light Options", "Intensity"], 80), setp("P", ["Light Options", "Color"], [1, 0.5, 0.25, 1])] },
        { id: "702-light-ambient-int50-color", steps: [light("A", "ambient"), setp("A", ["Light Options", "Intensity"], 50), setp("A", ["Light Options", "Color"], [0.25, 0.5, 1, 1])] },
        // Keyed 3-D property: read the value once before the save (pilot lesson 3).
        { id: "703-key-orientation-0-to-10-20-30-linear", steps: [ON3D,
            op("keyframe.add", { comp: "$", layer: "L", property: ["Transform", "Orientation"], time: 0, value: [0, 0, 0] }),
            op("keyframe.add", { comp: "$", layer: "L", property: ["Transform", "Orientation"], time: 2, value: [10, 20, 30] }),
            getp("L", ["Transform", "Orientation"], 1)] },
        { id: "704-light-environment-default", steps: [light("E", "environment")] }
    ]
}];
