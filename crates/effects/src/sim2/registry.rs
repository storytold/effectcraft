use super::foam::{FOAM_TEXTURES, foam};
use super::pixels::{ball_action, pixel_polly, scatterize};
use super::shatter::{CD_PROPS, CD_SOURCES, SHATTER_PATTERNS, SHATTER_TEXTURE_MODES, card_dance, shatter};
use super::surfaces::{caustics, wave_world};
use super::*;

// ------------------------------------------------------------------ specs

fn producer_params(k: u32, amp: f64, pos: (f64, f64)) -> Vec<crate::ParamSpec> {
    let ids: [&'static str; 8] = match k {
        1 => [
            "producer1/producer1Type",
            "producer1/producer1Position",
            "producer1/producer1Length",
            "producer1/producer1Width",
            "producer1/producer1Angle",
            "producer1/producer1Amplitude",
            "producer1/producer1Frequency",
            "producer1/producer1Phase",
        ],
        _ => [
            "producer2/producer2Type",
            "producer2/producer2Position",
            "producer2/producer2Length",
            "producer2/producer2Width",
            "producer2/producer2Angle",
            "producer2/producer2Amplitude",
            "producer2/producer2Frequency",
            "producer2/producer2Phase",
        ],
    };
    vec![
        p(ids[0], "Type", Value::Enum(0), popup(&["Ring", "Line"])),
        p(ids[1], "Position", pt(pos.0, pos.1), ParamUi::Point),
        p(ids[2], "Height/Length", num(0.1), slider(0.0, 2.0, 0.0, 1.0, 3)),
        p(ids[3], "Width", num(0.1), slider(0.0, 2.0, 0.0, 1.0, 3)),
        p(ids[4], "Angle", num(0.0), ParamUi::Angle),
        p(ids[5], "Amplitude", num(amp), slider(-5.0, 5.0, -1.0, 1.0, 3)),
        p(ids[6], "Frequency", num(1.0), slider(0.0, 30.0, 0.0, 5.0, 3)),
        p(ids[7], "Phase", num(0.0), ParamUi::Angle),
    ]
}

fn card_dance_params() -> Vec<crate::ParamSpec> {
    let mut v = vec![
        p("rowsColumns", "Rows & Columns", Value::Enum(0), popup(&["Independent", "Columns Follows Rows"])),
        p("rows", "Rows", num(10.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
        p("columns", "Columns", num(10.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
        p("backLayer", "Back Layer", Value::Layer(None), ParamUi::Layer),
        p("gradientLayer1", "Gradient Layer 1", Value::Layer(None), ParamUi::Layer),
        p("gradientLayer2", "Gradient Layer 2", Value::Layer(None), ParamUi::Layer),
    ];
    let names: [[&'static str; 4]; 8] = [
        ["xPosition/xPosSource", "xPosition/xPosMultiplier", "xPosition/xPosOffset", "X Position"],
        ["yPosition/yPosSource", "yPosition/yPosMultiplier", "yPosition/yPosOffset", "Y Position"],
        ["zPosition/zPosSource", "zPosition/zPosMultiplier", "zPosition/zPosOffset", "Z Position"],
        ["xRotation/xRotSource", "xRotation/xRotMultiplier", "xRotation/xRotOffset", "X Rotation"],
        ["yRotation/yRotSource", "yRotation/yRotMultiplier", "yRotation/yRotOffset", "Y Rotation"],
        ["zRotation/zRotSource", "zRotation/zRotMultiplier", "zRotation/zRotOffset", "Z Rotation"],
        ["xScale/xScaleSource", "xScale/xScaleMultiplier", "xScale/xScaleOffset", "X Scale"],
        ["yScale/yScaleSource", "yScale/yScaleMultiplier", "yScale/yScaleOffset", "Y Scale"],
    ];
    debug_assert_eq!(names.len(), CD_PROPS.len());
    for (i, n) in names.iter().enumerate() {
        let rot = (3..6).contains(&i);
        let scale = i >= 6;
        let (lo, hi) = if rot { (-3600.0, 3600.0) } else { (-100.0, 100.0) };
        v.push(p(n[0], "Source", Value::Enum(0), popup(CD_SOURCES)));
        v.push(p(n[1], "Multiplier", num(0.0), slider(lo, hi, if rot { -360.0 } else { -10.0 }, if rot { 360.0 } else { 10.0 }, 2)));
        v.push(p(n[2], "Offset", num(if scale { 1.0 } else { 0.0 }), slider(lo, hi, if rot { -360.0 } else { -10.0 }, if rot { 360.0 } else { 10.0 }, 2)));
    }
    v.extend(crate::card3d::camera_params(2.0));
    v.extend(crate::card3d::lighting_params(1.0, 0.25));
    v.extend(crate::card3d::material_params(0.75));
    v
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.sim.ccballaction",
            "CC Ball Action",
            vec![
                p("scatter", "Scatter", num(0.0), slider(0.0, 2000.0, 0.0, 300.0, 1)),
                p("rotationAxis", "Rotation Axis", Value::Enum(0), popup(&["X Axis", "Y Axis", "Z Axis", "XY Axis", "XZ Axis", "YZ Axis", "XYZ Axis"])),
                p("rotation", "Rotation", num(0.0), ParamUi::Angle),
                p("twistProperty", "Twist Property", Value::Enum(0), popup(&["X Axis", "Y Axis", "Radius", "Brightness"])),
                p("twistAngle", "Twist Angle", num(0.0), ParamUi::Angle),
                p("gridSpacing", "Grid Spacing", num(4.0), slider(1.0, 200.0, 1.0, 50.0, 0)),
                p("ballSize", "Ball Size", num(40.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("instabilityState", "Instability State", num(0.0), ParamUi::Angle),
            ],
            ball_action,
        ),
        spec(
            "ec.sim.ccpixelpolly",
            "CC Pixel Polly",
            vec![
                p("force", "Force", num(100.0), slider(-1000.0, 1000.0, -300.0, 300.0, 1)),
                p("gravity", "Gravity", num(1.0), slider(-20.0, 20.0, -5.0, 5.0, 2)),
                p("spinning", "Spinning", num(0.0), ParamUi::Angle),
                p("forceCenter", "Force Center", pt(0.5, 0.5), ParamUi::Point),
                p("directionRandomness", "Direction Randomness", num(10.0), pct()),
                p("speedRandomness", "Speed Randomness", num(10.0), pct()),
                p("gridSpacing", "Grid Spacing", num(10.0), slider(1.0, 500.0, 2.0, 100.0, 0)),
                p("object", "Object", Value::Enum(0), popup(&["Polygon", "Textured Polygon", "Square", "Textured Square"])),
                p("enableDepthSort", "Enable Depth Sort", Value::Bool(true), ParamUi::Checkbox),
                p("startTime", "Start Time (sec)", num(0.0), slider(-1000.0, 1000.0, 0.0, 10.0, 2)),
            ],
            pixel_polly,
        ),
        spec(
            "ec.sim.ccscatterize",
            "CC Scatterize",
            vec![
                p("scatter", "Scatter", num(0.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("rightTwist", "Right Twist", num(0.0), ParamUi::Angle),
                p("leftTwist", "Left Twist", num(0.0), ParamUi::Angle),
                p("transferMode", "Transfer Mode", Value::Enum(0), popup(&["Composite", "Add"])),
            ],
            scatterize,
        ),
        spec("ec.sim.carddance", "Card Dance", card_dance_params(), card_dance),
        spec(
            "ec.sim.caustics",
            "Caustics",
            vec![
                p("bottom/bottom", "Bottom", Value::Layer(None), ParamUi::Layer),
                p("bottom/scaling", "Scaling", num(1.0), slider(0.01, 10.0, 0.1, 3.0, 3)),
                p("bottom/repeatMode", "Repeat Mode", Value::Enum(2), popup(&["Once", "Tiled", "Reflected"])),
                p("bottom/bottomSizeDiffers", "If Layer Size Differs", Value::Enum(1), popup(&["Center", "Stretch to Fit"])),
                p("bottom/blur", "Blur", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("water/waterSurface", "Water Surface", Value::Layer(None), ParamUi::Layer),
                p("water/waveHeight", "Wave Height", num(0.2), slider(-1.0, 1.0, -1.0, 1.0, 3)),
                p("water/smoothing", "Smoothing", num(5.0), slider(0.0, 100.0, 0.0, 50.0, 1)),
                p("water/waterDepth", "Water Depth", num(0.1), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("water/refractiveIndex", "Refractive Index", num(1.2), slider(1.0, 3.0, 1.0, 2.0, 3)),
                p("water/surfaceColor", "Surface Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("water/surfaceOpacity", "Surface Opacity", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("water/causticsStrength", "Caustics Strength", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("sky/sky", "Sky", Value::Layer(None), ParamUi::Layer),
                p("sky/scaling", "Scaling", num(1.0), slider(0.01, 10.0, 0.1, 3.0, 3)),
                p("sky/repeatMode", "Repeat Mode", Value::Enum(2), popup(&["Once", "Tiled", "Reflected"])),
                p("sky/skySizeDiffers", "If Layer Size Differs", Value::Enum(1), popup(&["Center", "Stretch to Fit"])),
                p("sky/intensity", "Intensity", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("sky/convergence", "Convergence", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("lighting/lightType", "Light Type", Value::Enum(0), popup(&crate::card3d::LIGHT_TYPES)),
                p("lighting/lightIntensity", "Light Intensity", num(1.0), slider(0.0, 4.0, 0.0, 2.0, 2)),
                p("lighting/lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("lighting/lightPosition", "Light Position", pt(0.0, 0.0), ParamUi::Point),
                p("lighting/lightHeight", "Light Height", num(1.0), slider(0.0, 10.0, 0.0, 4.0, 2)),
                p("lighting/ambientLight", "Ambient Light", num(0.35), slider(0.0, 2.0, 0.0, 1.0, 2)),
                p("material/diffuse", "Diffuse Reflection", num(0.75), slider(0.0, 2.0, 0.0, 1.0, 2)),
                p("material/specular", "Specular Reflection", num(0.2), slider(0.0, 2.0, 0.0, 1.0, 2)),
                p("material/highlightSharpness", "Highlight Sharpness", num(15.0), slider(1.0, 100.0, 1.0, 100.0, 1)),
            ],
            caustics,
        ),
        spec(
            "ec.sim.waveworld",
            "Wave World",
            {
                let mut v = vec![
                    p("view", "View", Value::Enum(0), popup(&["Wireframe Preview", "Height Map"])),
                    p("wireframeControls/horizontalRotation", "Horizontal Rotation", num(0.0), ParamUi::Angle),
                    p("wireframeControls/verticalRotation", "Vertical Rotation", num(53.0), slider(0.0, 90.0, 0.0, 90.0, 1)),
                    p("wireframeControls/verticalScale", "Vertical Scale", num(0.5), slider(0.0, 10.0, 0.0, 2.0, 3)),
                    p("heightMapControls/brightness", "Brightness", num(0.5), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                    p("heightMapControls/contrast", "Contrast", num(0.75), slider(0.0, 4.0, 0.0, 2.0, 3)),
                    p("heightMapControls/gamma", "Gamma Adjustment", num(1.0), slider(0.01, 10.0, 0.2, 5.0, 3)),
                    p("heightMapControls/renderDryAreasAs", "Render Dry Areas As", Value::Enum(0), popup(&["Solid", "Transparent"])),
                    p("heightMapControls/transparency", "Transparency", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                    p("simulation/gridResolution", "Grid Resolution", num(60.0), slider(1.0, 400.0, 1.0, 200.0, 0)),
                    p("simulation/gridResDownsamples", "Grid Res Downsamples", Value::Bool(false), ParamUi::Checkbox),
                    p("simulation/waveSpeed", "Wave Speed", num(0.2), slider(0.0, 5.0, 0.0, 1.0, 3)),
                    p("simulation/damping", "Damping", num(0.05), slider(0.0, 5.0, 0.0, 1.0, 3)),
                    p("simulation/reflectEdges", "Reflect Edges", Value::Enum(0), popup(&["None", "Left", "Top", "Right", "Bottom", "All"])),
                    p("simulation/preRoll", "Pre-roll (seconds)", num(0.0), slider(0.0, 30.0, 0.0, 10.0, 2)),
                    p("ground/ground", "Ground", Value::Layer(None), ParamUi::Layer),
                    p("ground/steepness", "Steepness", num(0.25), slider(0.0, 2.0, 0.0, 1.0, 3)),
                    p("ground/height", "Height", num(0.25), slider(0.0, 2.0, 0.0, 1.0, 3)),
                    p("ground/waveStrength", "Wave Strength", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                ];
                v.extend(producer_params(1, 0.5, (0.5, 0.5)));
                v.extend(producer_params(2, 0.0, (0.25, 0.25)));
                v
            },
            wave_world,
        ),
        spec(
            "ec.sim.shatter",
            "Shatter",
            vec![
                p(
                    "view",
                    "View",
                    Value::Enum(1),
                    popup(&["Rendered", "Wireframe Front View", "Wireframe", "Wireframe Front View + Forces", "Wireframe + Forces"]),
                ),
                p("render", "Render", Value::Enum(0), popup(&["All", "Layer", "Pieces"])),
                p("shape/pattern", "Pattern", Value::Enum(0), popup(SHATTER_PATTERNS)),
                p("shape/repetitions", "Repetitions", num(10.0), slider(1.0, 500.0, 1.0, 100.0, 2)),
                p("shape/direction", "Direction", num(0.0), ParamUi::Angle),
                p("shape/origin", "Origin", pt(0.5, 0.5), ParamUi::Point),
                p("shape/customShatterMap", "Custom Shatter Map", Value::Layer(None), ParamUi::Layer),
                p("shape/whiteTilesFixed", "White Tiles Fixed", Value::Bool(false), ParamUi::Checkbox),
                p("shape/extrusionDepth", "Extrusion Depth", num(0.05), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("force1/force1Position", "Position", pt(0.5, 0.5), ParamUi::Point),
                p("force1/force1Depth", "Depth", num(0.1), slider(-1.0, 1.0, -1.0, 1.0, 3)),
                p("force1/force1Radius", "Radius", num(0.4), slider(0.0, 4.0, 0.0, 2.0, 3)),
                p("force1/force1Strength", "Strength", num(5.0), slider(-20.0, 20.0, -10.0, 10.0, 2)),
                p("force2/force2Position", "Position", pt(0.25, 0.25), ParamUi::Point),
                p("force2/force2Depth", "Depth", num(0.1), slider(-1.0, 1.0, -1.0, 1.0, 3)),
                p("force2/force2Radius", "Radius", num(0.0), slider(0.0, 4.0, 0.0, 2.0, 3)),
                p("force2/force2Strength", "Strength", num(5.0), slider(-20.0, 20.0, -10.0, 10.0, 2)),
                p("physics/rotationSpeed", "Rotation Speed", num(0.2), slider(0.0, 5.0, 0.0, 1.0, 3)),
                p("physics/tumbleAxis", "Tumble Axis", Value::Enum(0), popup(&["Free", "None", "X", "Y", "Z", "XY", "XZ", "YZ"])),
                p("physics/randomness", "Randomness", num(0.1), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("physics/viscosity", "Viscosity", num(0.1), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("physics/massVariance", "Mass Variance", num(30.0), pct()),
                p("physics/gravity", "Gravity", num(3.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("physics/gravityDirection", "Gravity Direction", num(180.0), ParamUi::Angle),
                p("physics/gravityInclination", "Gravity Inclination", num(0.0), slider(-90.0, 90.0, -90.0, 90.0, 1)),
                p("gradient/shatterThreshold", "Shatter Threshold", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("gradient/gradientLayer", "Gradient Layer", Value::Layer(None), ParamUi::Layer),
                p("gradient/invertGradient", "Invert Gradient", Value::Bool(false), ParamUi::Checkbox),
                p("textures/color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("textures/opacity", "Opacity", num(1.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("textures/frontMode", "Front Mode", Value::Enum(1), popup(&SHATTER_TEXTURE_MODES)),
                p("textures/sideMode", "Side Mode", Value::Enum(0), popup(&SHATTER_TEXTURE_MODES)),
                p("textures/backMode", "Back Mode", Value::Enum(1), popup(&SHATTER_TEXTURE_MODES)),
                p("textures/frontLayer", "Front Layer", Value::Layer(None), ParamUi::Layer),
                p("textures/sideLayer", "Side Layer", Value::Layer(None), ParamUi::Layer),
                p("textures/backLayer", "Back Layer", Value::Layer(None), ParamUi::Layer),
            ]
            .into_iter()
            .chain(crate::card3d::camera_params(4.0))
            .chain(crate::card3d::lighting_params(1.0, 0.25))
            .chain(crate::card3d::material_params(0.75))
            .chain([p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0))])
            .collect(),
            shatter,
        ),
        spec(
            "ec.sim.foam",
            "Foam",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Draft", "Draft + Flow Map", "Rendered"])),
                p("producer/producerPoint", "Producer Point", pt(0.5, 0.5), ParamUi::Point),
                p("producer/producerXSize", "Producer X Size", num(0.05), slider(0.0, 2.0, 0.0, 1.0, 3)),
                p("producer/producerYSize", "Producer Y Size", num(0.05), slider(0.0, 2.0, 0.0, 1.0, 3)),
                p("producer/producerOrientation", "Producer Orientation", num(0.0), ParamUi::Angle),
                p("producer/zoomProducerPoint", "Zoom Producer Point", Value::Bool(false), ParamUi::Checkbox),
                p("producer/productionRate", "Production Rate", num(1.0), slider(0.0, 50.0, 0.0, 10.0, 3)),
                p("bubbles/size", "Size", num(0.5), slider(0.0, 10.0, 0.0, 2.0, 3)),
                p("bubbles/sizeVariance", "Size Variance", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("bubbles/lifespan", "Lifespan", num(300.0), slider(1.0, 30_000.0, 1.0, 1000.0, 1)),
                p("bubbles/growthSpeed", "Bubble Growth Speed", num(0.1), slider(0.0001, 10.0, 0.0001, 1.0, 3)),
                p("bubbles/strength", "Strength", num(10.0), slider(0.0, 100.0, 0.0, 20.0, 2)),
                p("physics/initialSpeed", "Initial Speed", num(0.0), slider(-100.0, 100.0, -10.0, 10.0, 3)),
                p("physics/initialDirection", "Initial Direction", num(0.0), ParamUi::Angle),
                p("physics/windSpeed", "Wind Speed", num(0.5), slider(-100.0, 100.0, -10.0, 10.0, 3)),
                p("physics/windDirection", "Wind Direction", num(90.0), ParamUi::Angle),
                p("physics/turbulence", "Turbulence", num(0.5), slider(0.0, 10.0, 0.0, 2.0, 3)),
                p("physics/wobbleAmount", "Wobble Amount", num(0.05), slider(0.0, 10.0, 0.0, 2.0, 3)),
                p("physics/repulsion", "Repulsion", num(1.0), slider(0.0, 10.0, 0.0, 2.0, 3)),
                p("physics/popVelocity", "Pop Velocity", num(1.0), slider(-100.0, 100.0, -10.0, 10.0, 3)),
                p("physics/viscosity", "Viscosity", num(0.1), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("physics/stickiness", "Stickiness", num(0.75), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("zoom", "Zoom", num(1.0), slider(0.01, 50.0, 0.1, 5.0, 3)),
                p("universeSize", "Universe Size", num(1.0), slider(0.01, 10.0, 0.5, 3.0, 3)),
                p("rendering/blendMode", "Blend Mode", Value::Enum(0), popup(&["Transparent", "Solid Old on Top", "Solid New on Top"])),
                p("rendering/bubbleTexture", "Bubble Texture", Value::Enum(0), popup(&FOAM_TEXTURES)),
                p("rendering/bubbleTextureLayer", "Bubble Texture Layer", Value::Layer(None), ParamUi::Layer),
                p("rendering/bubbleOrientation", "Bubble Orientation", Value::Enum(0), popup(&["Fixed", "Physical Orientation", "Bubble Velocity"])),
                p("rendering/environmentMap", "Environment Map", Value::Layer(None), ParamUi::Layer),
                p("rendering/reflectionStrength", "Reflection Strength", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("rendering/reflectionConvergence", "Reflection Convergence", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("flowMap/flowMap", "Flow Map", Value::Layer(None), ParamUi::Layer),
                p("flowMap/flowMapSteepness", "Flow Map Steepness", num(0.05), slider(-100.0, 100.0, -1.0, 1.0, 3)),
                p("flowMap/flowMapFits", "Flow Map Fits", Value::Enum(0), popup(&["Background", "Universe"])),
                p("flowMap/simulationQuality", "Simulation Quality", Value::Enum(0), popup(&["Normal", "High", "Intense"])),
                p("randomSeed", "Random Seed", num(1.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
            ],
            foam,
        ),
    ]
}
