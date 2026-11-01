# The test cases and the files that hold them

The tests of `effectcraft-aep` (and the After Effects import tests of `effectcraft-engine`) address
a case by the name of the single-case file it was first written against. The cases live in the
combined projects of the corpus (in the folder named by `EFFECTCRAFT_AEP_FIXTURES`): each case is the composition named like the case without its
extension, built like the single-case file's composition `C` (1920x1080, 25 fps, 5 s, black, one
red 1920x1080 solid `L`) plus the case's change (and the compositions that change uses). A test
gets the case as a project of its own, cut from the combined file (`tests/it/common/mod.rs`).
`empty.aep` and `fx-switch.aep` (the fx switch of an effect instance) are files of their own.

Project settings have one value per project, so the settings cases share the files `settings-*`
(each file holds the changes of all its cases). The generator and how to run it:
[`tools/README.md`](tools/README.md). Every file has an `.attribution` sidecar in the corpus.

| case | file | composition | change |
|---|---|---|---|
| `000-base.aep` | `layers-comps.aep` | `000-base` | base state |
| `000-base.aepx` | `aepx-pair.aepx` | `000-base` | base state |
| `001-empty.aep` | `empty.aep` | (whole file) | an empty project saved by After Effects (no items) |
| `010-layer-name.aep` | `layers-comps.aep` | `010-layer-name` | layer name `L` -> `Renamed` (layer.name; source footage name stays `L`) |
| `012-layer-shy.aep` | `layers-comps.aep` | `012-layer-shy` | layer.shy false -> true |
| `013-layer-locked.aep` | `layers-comps.aep` | `013-layer-locked` | layer.locked false -> true |
| `014-layer-solo.aep` | `layers-comps.aep` | `014-layer-solo` | layer.solo false -> true |
| `015-layer-3d.aep` | `3d.aep` | `015-layer-3d` | layer.threeDLayer false -> true (3D layer switch; adds Orientation/X/Y/Z Rotation, Material Options groups) |
| `016-layer-blend-multiply.aep` | `layers-comps.aep` | `016-layer-blend-multiply` | blending mode Normal -> Multiply |
| `017-layer-motionblur.aep` | `layers-comps.aep` | `017-layer-motionblur` | layer.motionBlur false -> true |
| `018-layer-stretch200.aep` | `keyframes.aep` | `018-layer-stretch200` | time stretch 100 -> 200 (out point becomes 10 s; comp is 5 s) |
| `019-layer-inpoint1.aep` | `layers-comps.aep` | `019-layer-inpoint1` | inPoint 0 -> 1 (AE moved outPoint to 6, startTime stays 0; solid source is unbounded) |
| `021-layer-starttime0.5.aep` | `layers-comps.aep` | `021-layer-starttime0.5` | startTime 0 -> 0.5 (in 0.5, out 5.5) |
| `022-layer-parent-null.aep` | `layers-comps.aep` | `022-layer-parent-null` | NEW null layer "Null" (source "Null 1", 100x100) created on top; L (now index 2) parent = layer 1 (Null). Adds 1 layer + 1 footage item |
| `023-layer-quality-draft.aep` | `layers-comps.aep` | `023-layer-quality-draft` | layer.quality Best -> Draft |
| `024-layer-collapse.aep` | `layers-comps.aep` | `024-layer-collapse` | layer.collapseTransformation false -> true (API call accepted; AE may ignore it for a solid, compare with base) |
| `025-layer-adjustment.aep` | `layers-comps.aep` | `025-layer-adjustment` | layer.adjustmentLayer false -> true |
| `026-layer-guide.aep` | `layers-comps.aep` | `026-layer-guide` | layer.guideLayer false -> true |
| `030-comp-size-1280x720.aep` | `layers-comps.aep` | `030-comp-size-1280x720` | comp width/height 1920x1080 -> 1280x720 (layer L stays 1920x1080, not repositioned) |
| `031-comp-fps30.aep` | `layers-comps.aep` | `031-comp-fps30` | frameRate 25 -> 30 |
| `032-comp-duration10.aep` | `layers-comps.aep` | `032-comp-duration10` | duration 5 -> 10 s (work area duration also becomes 10) |
| `033-comp-bg-green.aep` | `layers-comps.aep` | `033-comp-bg-green` | bgColor [0,0,0] -> [0,1,0] |
| `034-comp-shutter90.aep` | `layers-comps.aep` | `034-comp-shutter90` | shutterAngle 180 -> 90 |
| `035-comp-workarea-1-2.aep` | `layers-comps.aep` | `035-comp-workarea-1-2` | workAreaStart 0 -> 1, workAreaDuration 5 -> 2 |
| `037-comp-pixelaspect2.aep` | `layers-comps.aep` | `037-comp-pixelaspect2` | pixelAspect 1 -> 2 |
| `038-comp-motionblur.aep` | `layers-comps.aep` | `038-comp-motionblur` | comp.motionBlur false -> true |
| `039-comp-renderer-advanced3d.aep` | `3d.aep` | `039-comp-renderer-advanced3d` | renderer `ADBE Advanced 3d` (classic) -> `ADBE Calder` (= "Advanced 3D" friendly name `advanced3d`) |
| `040-static-position-100-200.aep` | `layers-comps.aep` | `040-static-position-100-200` | Position [960,540,0] -> [100,200,0] |
| `041-static-scale-50.aep` | `layers-comps.aep` | `041-static-scale-50` | Scale [100,100,100] -> [50,50,100] |
| `042-static-rotation-45.aep` | `layers-comps.aep` | `042-static-rotation-45` | Rotation 0 -> 45 |
| `043-static-opacity-25.aep` | `layers-comps.aep` | `043-static-opacity-25` | Opacity 100 -> 25 |
| `044-static-anchor-10-20.aep` | `layers-comps.aep` | `044-static-anchor-10-20` | Anchor Point [960,540,0] -> [10,20,0] |
| `052-key-position-2key-ease.aep` | `keyframes.aep` | `052-key-position-2key-ease` | from 051: both keys set_easing preset 'ease' (bezier in/out, speed 0, influence 33) |
| `053-key-position-2key-hold-key1.aep` | `keyframes.aep` | `053-key-position-2key-hold-key1` | from 051: key1 inType/outType = HOLD (key2 stays linear) |
| `054-key-position-2key-spatial-bezier.aep` | `keyframes.aep` | `054-key-position-2key-spatial-bezier` | from 051: key1 spatial outTangent [30,60,0], key2 spatial inTangent [-30,-60,0], autoBezier=false, continuous=false (S-shaped path; verified: value at t=.25 = [23.7,16.95]) |
| `055-key-position-3key-inserted-t2.5.aep` | `keyframes.aep` | `055-key-position-3key-inserted-t2.5` | from 051: 3rd key inserted into the already animated property at t=2.5 value [200,100,0] (= keys at t=0,1,2.5, all linear) |
| `056-key-position-3key-roving-key2.aep` | `keyframes.aep` | `056-key-position-3key-roving-key2` | from 055: key2 roving=true (AE moved key2 to t=1.0355078125, value kept [100,0,0]) |
| `057-key-position-3key-label9-key2.aep` | `keyframes.aep` | `057-key-position-3key-label9-key2` | from 055: key2 keyframe label = 9 (Green). (056 was undone first) |
| `058-key-opacity-2key.aep` | `keyframes.aep` | `058-key-opacity-2key` | Opacity keys t=0 -> 0, t=2 -> 100 (linear) |
| `058-key-opacity-2key.aepx` | `aepx-pair.aepx` | `058-key-opacity-2key` | Opacity keys t=0 -> 0, t=2 -> 100 (linear) |
| `060-expr-opacity-wiggle.aep` | `keyframes.aep` | `060-expr-opacity-wiggle` | Opacity expression `wiggle(1,10)` (enabled, JavaScript engine = project default) |
| `060-expr-opacity-wiggle.aepx` | `aepx-pair.aepx` | `060-expr-opacity-wiggle` | Opacity expression `wiggle(1,10)` (enabled, JavaScript engine = project default) |
| `071-fx-slider-value77.aep` | `effects.aep` | `071-fx-slider-value77` | from 070: Slider 0 -> 77 |
| `073-fx-color-0.2-0.6-1.aep` | `effects.aep` | `073-fx-color-0.2-0.6-1` | add `ADBE Color Control`, Color -> [0.2,0.6,1] (no default-only Color file) |
| `077-fx-dropdown-default.aep` | `effects.aep` | `077-fx-dropdown-default` | add Dropdown Menu Control (matchName `Pseudo/@@S1iPXtJFTHK4QaDbgJvLhw`, default items) |
| `080-fx-dropshadow-default.aep` | `effects.aep` | `080-fx-dropshadow-default` | add `ADBE Drop Shadow` (default params) |
| `090-mask-rect.aep` | `masks.aep` | `090-mask-rect` | mask.add rect size [800,400] centre [960,540]: "Mask 1", vertices [560,340],[1360,340],[1360,740],[560,740], mode Add |
| `091-mask-rect-props.aep` | `masks.aep` | `091-mask-rect-props` | from 090: feather [0,0]->[20,20], opacity 100->50, mode Add->Subtract, inverted false->true, expansion 0->5 |
| `093-mask-path-2keys.aep` | `masks.aep` | `093-mask-path-2keys` | from 090: mask path keyed: t=0 rect 400x200 (vertices [760,440],[1160,440],[1160,640],[760,640]), t=1 rect 800x400; linear interp. NOTE: mask.set_path 'time' makes the first write at t=0 the first key (stopwatch on) |
| `110-text-A.aep` | `text.aep` | `110-text-A` | new point text layer T, text "A" (defaults: Times New Roman TimesNewRomanPSMT, size 100, fill white) |
| `110-text-A.aepx` | `aepx-pair.aepx` | `110-text-A` | new point text layer T, text "A" (defaults: Times New Roman TimesNewRomanPSMT, size 100, fill white) |
| `113-text-Hello-size60-fillorange.aep` | `text.aep` | `113-text-Hello-size60-fillorange` | from 112: fillColor [1,1,1] -> [1,0.5,0] |
| `114-text-animator-opacity.aep` | `text.aep` | `114-text-animator-opacity` | from 113: + text animator "Animator 1" with Opacity=0 and a Range Selector 1 |
| `122-marker-comp-1s.aep` | `markers.aep` | `122-marker-comp-1s` | comp marker at t=1 s, comment "CompMarker" |
| `123-marker-layer-1s.aep` | `markers.aep` | `123-marker-layer-1s` | layer marker on L at t=1 s, comment "LayerMarker" |
| `124-precomp-Pre.aep` | `footage-precomp.aep` | `124-precomp-Pre` | L precomposed (moveAll) into new comp "Pre" (1920x1080, 25 fps, 5 s); C now holds one layer "Pre" (label 15). Adds comp item "Pre" |
| `124-precomp-Pre.aepx` | `aepx-pair.aepx` | `124-precomp-Pre` | L precomposed (moveAll) into new comp "Pre" (1920x1080, 25 fps, 5 s); C now holds one layer "Pre" (label 15). Adds comp item "Pre" |
| `126-footage-png-layer.aep` | `footage-precomp.aep` | `126-footage-png-layer` | from 125: layer "png" created from that footage in C (index 1 above L) |
| `202-key-out-hold-in-linear.aep` | `keyframes.aep` | `202-key-out-hold-in-linear` | Position keys t=0 [0,0,0], t=1 [100,0,0]; key1 inType=LINEAR, outType=HOLD (key2 linear/linear). Confirmed by layer.info: key1 in=linear out=hold |
| `203-key-in-hold-out-linear.aep` | `keyframes.aep` | `203-key-in-hold-out-linear` | same keys; key1 inType=HOLD, outType=LINEAR. Confirmed: key1 in=hold out=linear |
| `204-key-ease-asymmetric.aep` | `keyframes.aep` | `204-key-ease-asymmetric` | Position keys t=0 [0,0,0], t=1 [100,0,0]; set_easing key1 inSpeed 5/inInfluence 20, outSpeed 50/outInfluence 80; key2 inSpeed 7/inInfl 60, outSpeed 9/outInfl 40. AE read-back (spatial property): key1 in speed forced to 0 (infl 20), key1 out 50/80, key2 in 7/60, key2 out speed forced to 0 (infl 40); both bezier/bezier |
| `205-key-opacity-ease-asymmetric.aep` | `keyframes.aep` | `205-key-opacity-ease-asymmetric` | (extra) Opacity keys t=0 -> 0, t=2 -> 100; set_easing key1 inSpeed 5/inInfl 20, outSpeed 20/outInfl 80; key2 inSpeed 30/inInfl 60, outSpeed 9/outInfl 40. AE read-back: key1 in speed 0 (infl 20), key1 out 20/80, key2 in 30/60, key2 out speed 0 (infl 40); bezier/bezier (scalar 48-byte record ease slots) |
| `206-key-scale-2key-linear.aep` | `keyframes.aep` | `206-key-scale-2key-linear` | Scale keys t=0 [100,100,100], t=1 [200,150,100] (linear; 3-component non-spatial record) |
| `208-key-mask-feather-2key-linear.aep` | `masks.aep` | `208-key-mask-feather-2key-linear` | NEW mask "Mask 1" (rect 800x400 centre [960,540], mode Add, = 090) + Mask Feather keys t=0 [0,0], t=1 [40,20] (linear, 2-D). Diff against 090 |
| `210-key-fill-color-2key-linear.aep` | `keyframes.aep` | `210-key-fill-color-2key-linear` | effect `ADBE Fill` (index 1) added (all other params default); Color keys t=0 [1,0,0], t=1 [0,0.5,1] (linear, colour record) |
| `211-anchor-100x100-solid-10-20.aep` | `layers-comps.aep` | `211-anchor-100x100-solid-10-20` | NEW solid layer `S100` 100x100 white [1,1,1] (new footage item `S100`) in the 1920x1080 comp, Anchor Point set [50,50,0] (default) -> [10,20,0]; Position left at default (comp centre [960,540]) |
| `212-anchor-400x300-solid-100-50.aep` | `layers-comps.aep` | `212-anchor-400x300-solid-100-50` | NEW solid layer `S400` 400x300 white [1,1,1] (new footage item `S400`), Anchor Point -> [100,50,0]; Position default |
| `213-expr-opacity-wiggle-disabled.aep` | `keyframes.aep` | `213-expr-opacity-wiggle-disabled` | Opacity expression `wiggle(1,10)` set (as in 060) and then `expression.disable_all` on comp C (sets expressionEnabled=false; the expression text stays in the file, grep confirms 1 occurrence). Diff against 060 to find the enable flag |
| `214-fx-gaussianblur-default.aep` | `effects.aep` | `214-fx-gaussianblur-default` | add `ADBE Gaussian Blur 2`, all params default (Blurriness 0, Dimensions Horizontal and Vertical, Repeat Edge Pixels off) |
| `215-fx-fill-default.aep` | `effects.aep` | `215-fx-fill-default` | add `ADBE Fill`, all params default (colour red, opacity 100 %) |
| `216-fx-gaussianblur-40.aep` | `effects.aep` | `216-fx-gaussianblur-40` | add `ADBE Gaussian Blur 2`, Blurriness 0 -> 40 (compare 214 and 078 = 25) |
| `217-fx-checkbox-on.aep` | `effects.aep` | `217-fx-checkbox-on` | add `ADBE Checkbox Control`, Checkbox 0 -> 1 (on) |
| `218-fx-point-111-222.aep` | `effects.aep` | `218-fx-point-111-222` | add `ADBE Point Control`, Point -> [111,222] |
| `219-fx-angle-90.aep` | `effects.aep` | `219-fx-angle-90` | add `ADBE Angle Control`, Angle 0 -> 90 |
| `220-fx-layercontrol-to-layer1.aep` | `effects.aep` | `220-fx-layercontrol-to-layer1` | NEW null layer `Null` created on top (index 1, new footage item `Null 1`); L (now index 2) gets `ADBE Layer Control` with Layer = 1 (the Null; layer index 1, read back as value 1). Compare 076 (Layer = None) and 022 (same null) |
| `221-fx-slider-2key.aep` | `effects.aep` | `221-fx-slider-2key` | add `ADBE Slider Control`, Slider keyed t=0 -> 10, t=2 -> 90 (linear) |
| `222-layer-video-off.aep` | `layers-comps.aep` | `222-layer-video-off` | layer.enabled true -> false (Video switch). aepx `ldta` byte 39: 07 -> 06 |
| `223-layer-audio-off.aep` | `layers-comps.aep` | `223-layer-audio-off` | `audioEnabled` = false on L (a solid has no audio; AE accepted it without warning and the flag did change: byte 39: 07 -> 05) |
| `224-layer-effects-off.aep` | `layers-comps.aep` | `224-layer-effects-off` | `effectsActive` = false on L (no effects present; byte 39: 07 -> 03) |
| `225-layer-blend-screen.aep` | `layers-comps.aep` | `225-layer-blend-screen` | blending mode Normal -> Screen (aepx `ldta` byte 99 = 6) |
| `226-layer-blend-add.aep` | `layers-comps.aep` | `226-layer-blend-add` | blending mode Normal -> Add (byte 99 = 4) |
| `227-layer-blend-overlay.aep` | `layers-comps.aep` | `227-layer-blend-overlay` | blending mode Normal -> Overlay (byte 99 = 7) |
| `228-layer-blend-difference.aep` | `layers-comps.aep` | `228-layer-blend-difference` | blending mode Normal -> Difference (byte 99 = 26) |
| `229-comp-fps-29.97.aep` | `layers-comps.aep` | `229-comp-fps-29.97` | comp frameRate 25 -> 29.97 (AE read-back 29.9700012207031; duration snapped 5 s -> 5.00500500500501 s = 150 frames; work area duration follows) |
| `230-comp-fps-23.976.aep` | `layers-comps.aep` | `230-comp-fps-23.976` | comp frameRate 25 -> 23.976 (read-back 23.9759979248047; duration snapped to 5.00500500500501 s = 120 frames) |
| `231-comp-bg-grey-0.5.aep` | `layers-comps.aep` | `231-comp-bg-grey-0.5` | comp bgColor [0,0,0] -> [0.5,0.5,0.5] (read-back 0.50196078431373 = 128/255 on all channels) |
| `232-mask-ellipse.aep` | `masks.aep` | `232-mask-ellipse` | mask.add shape ellipse size [800,400] centre [960,540], mode Add. AE read-back of the path: vertices [960,340],[1360,540],[960,740],[560,540]; inTangents [-220.913879394531,0],[0,-110.456970214844],[220.913940429688,0],[0,110.456970214844]; outTangents [220.913940429688,0],[0,110.456970214844],[-220.913879394531,0],[0,-110.456970214844]; closed |
| `233-mask-open-path.aep` | `masks.aep` | `233-mask-open-path` | mask.add (no shape, mode Add) + mask.set_path vertices [500,300],[960,700],[1400,300], closed=false, no tangents. Read-back closed=false, vertices stored as float32 (499.999969482422, 699.999938964844) |
| `234-mask-mode-intersect.aep` | `masks.aep` | `234-mask-mode-intersect` | rect mask 800x400 centre [960,540] (as 090) with mode Intersect (aepx `mkif` @4 = 3; Add = 1 in 090) |
| `235-mask-mode-lighten.aep` | `masks.aep` | `235-mask-mode-lighten` | same rect, mode Lighten |
| `236-mask-mode-darken.aep` | `masks.aep` | `236-mask-mode-darken` | same rect, mode Darken |
| `237-mask-mode-difference.aep` | `masks.aep` | `237-mask-mode-difference` | same rect, mode Difference |
| `238-shape-rect-rounded-corners.aep` | `shapes.aep` | `238-shape-rect-rounded-corners` | Group 1 = Rectangle Path 400x300 + Fill [0,1,0] + `shape.add_rounded_corners` radius 20 ("Round Corners 1", `ADBE Vector Filter - RC`, `ADBE Vector RoundCorner Radius` 20) |
| `239-shape-stroke-green-w8-round.aep` | `shapes.aep` | `239-shape-stroke-green-w8-round` | Group 1 = Rectangle Path 400x300 + Stroke colour [0,1,0,1] width 8, Line Cap = 2 (Round), Line Join = 2 (Round); NO fill; read back: cap 2, join 2 |
| `240-shape-custom-path-tangents.aep` | `shapes.aep` | `240-shape-custom-path-tangents` | Group 1 = Path (closed) vertices [0,-100],[100,100],[-100,100], inTangents [-10,-20],[30,20],[-60,10], outTangents [40,0],[-20,30],[0,-50] (read back out2 = -19.9999847412109) + Fill [0,1,0] |
| `241-shape-gradient-fill-linear.aep` | `shapes.aep` | `241-shape-gradient-fill-linear` | Group 1 = Rectangle Path 400x300 + Gradient Fill 1 (`ADBE Vector Graphic - G-Fill`), linear, start [-200,0], end [200,0]; colour stops left at AE's default (not scriptable) |
| `242-shape-group-position-50-50.aep` | `shapes.aep` | `242-shape-group-position-50-50` | Group 1 = Rectangle Path 400x300 + Fill [0,1,0]; Group 1 / Transform / Position [0,0] -> [50,50] |
| `243-text-multiline-A-CR-B.aep` | `text.aep` | `243-text-multiline-A-CR-B` | point text layer T, text `A\rB` (CR between, 2 paragraphs / 2 lines) |
| `245-text-box-400x200-Hello.aep` | `text.aep` | `245-text-box-400x200-Hello` | paragraph (box) text layer T, `Hello`, boxSize [400,200] (AE: box position [-200,-100] in layer space), left aligned (default) |
| `246-text-box-center-align.aep` | `text.aep` | `246-text-box-center-align` | FROM 245 (not from base): justification left -> center (one variable vs 245) |
| `247-text-tracking-100.aep` | `text.aep` | `247-text-tracking-100` | point text `Hello` with tracking 0 -> 100 (whole document, text.set_style tracking 100) |
| `248-text-range-fill-red-1-3.aep` | `text.aep` | `248-text-range-fill-red-1-3` | point text `Hello`; per-range style: characters [1,3) ("el") fill colour [1,0,0]; rest white |
| `249-marker-layer-full.aep` | `markers.aep` | `249-marker-layer-full` | layer marker on L at t=1 s: comment `LM`, duration 1 s, chapter `Chap`, url `http://a.example/u`, frameTarget `_blank`, cuePointName `CueX` (AE read-back: eventCuePoint=true, label 0, params {}). All four link strings non-empty and distinct so that the string order in `Nmrd` can be decided |
| `251-camera-one-node.aep` | `3d.aep` | `251-camera-one-node` | camera layer `Camera 1` (default 50 mm, label 4) with autoOrient = off (one-node camera). aepx `ldta` byte 38 of the camera: 0x44 (two-node, 120/256) -> 0x04, so bit 0x40 = "orient towards point of interest". AE's summary reports Position [0,0,-2666.67] for this layer (a two-node camera reports [960,540,-2666.67] as in 120) |
| `252-light-point.aep` | `3d.aep` | `252-light-point` | light layer `Light 1`, lightType point (default intensity 100 %, label 6) |
| `253-camera-position-100-200-m1000.aep` | `3d.aep` | `253-camera-position-100-200-m1000` | two-node camera `Camera 1`, Position [960,540,-2666.67] -> [100,200,-1000] (read back [100,200,-1000], Point of Interest stays [960,540,0]) |
| `254-light-spot-intensity-50.aep` | `3d.aep` | `254-light-spot-intensity-50` | spot light `Light 1`, Light Options / Intensity 100 -> 50 |
| `256-camera-two-node-default.aep` | `3d.aep` | `256-camera-two-node-default` | (extra, noise-matched reference) default two-node camera `Camera 1`, compare 251, 253 and 120 |
| `259-solid-blue-0-0-1.aep` | `layers-comps.aep` | `259-solid-blue-0-0-1` | NEW solid layer `Blue` [0,0,1], comp size 1920x1080, on top of L (new footage item `Blue`); the colour sits in the `Soli` footage `opti` (compare L red = 1.0,1.0,0,0 as float32 ARGB) |
| `299-save-noise.aep` | `layers-comps.aep` | `299-save-noise` | plain save of the freshly loaded base copy (no change at all) to measure save noise of THIS session; compare with 000-base and 200/201 |
| `300-fx-gaussianblur-0-explicit.aep` | `effects.aep` | `300-fx-gaussianblur-0-explicit` | layer 1 effect `ADBE Gaussian Blur 2` added; AE read-back of Blurriness right after add = **25** (property.get value 25, numKeys 0); then Blurriness set explicitly 25 -> 0 (read back 0) |
| `301-fx-gaussianblur-40-then-25.aep` | `effects.aep` | `301-fx-gaussianblur-40-then-25` | `ADBE Gaussian Blur 2` added, Blurriness 25 -> 40 -> 25 (set twice in one call; read back 25) |
| `302-fx-fill-opacity-100-explicit.aep` | `effects.aep` | `302-fx-fill-opacity-100-explicit` | `ADBE Fill` added (default Opacity read back 1 = 100 %; scale of effect.set_property is 0..1, 100 is rejected "out of range 0 to 1"); Opacity set explicitly to 1 |
| `303-fx-fill-opacity-50-then-100.aep` | `effects.aep` | `303-fx-fill-opacity-50-then-100` | `ADBE Fill` added, Opacity 1 -> 0.5 -> 1 (read back 1) |
| `304-fx-slider-0-explicit.aep` | `effects.aep` | `304-fx-slider-0-explicit` | `ADBE Slider Control` added, Slider set explicitly 0 -> 0 (default is 0; read back 0) |
| `305-key-scale-bezier-ease.aep` | `keyframes.aep` | `305-key-scale-bezier-ease` | layer 1 Transform/Scale keys t=0 [100,100,100] and t=2 [200,150,100]; set_easing key1 inSpeed 0/inInfl 33, outSpeed 20/outInfl 66; key2 inSpeed 10/inInfl 66, outSpeed 0/outInfl 33. AE read-back: bezier/bezier on both, ease arrays 3 identical entries per side with exactly these values |
| `306-key-maskfeather-bezier-ease.aep` | `masks.aep` | `306-key-maskfeather-bezier-ease` | NEW mask `Mask 1` (rect 800x400 centre [960,540], vertices [560,340],[1360,340],[1360,740],[560,740], mode Add) + Mask Feather keys t=0 [0,0], t=2 [50,30]; key1 inSpeed 0/infl 33, outSpeed 15/infl 66; key2 inSpeed 8/infl 66, outSpeed 0/infl 33; read back as set (2 ease entries per side) |
| `307-key-fillcolor-bezier.aep` | `keyframes.aep` | `307-key-fillcolor-bezier` | `ADBE Fill` added; Color keys t=0 [1,0,0,1], t=2 [0,0,1,1] (value at t=1 read back [0.5,0,0.5,1]); both keys then set_interpolation inType/outType bezier (temporalContinuous/AutoBezier false). AE read-back ease: speed 0 / influence 0 on all four sides (colour has 1 ease entry, AE reports influence 0, not the 16.67 default) |
| `309-solid-100x100-mask-rect.aep` | `masks.aep` | `309-solid-100x100-mask-rect` | 308 + `mask.add` shape rect size [60,40] centre [50,50], mode Add; AE read-back path vertices [20,30],[80,30],[80,70],[20,70], all tangents 0 |
| `310-solid-100x100-pointcontrol.aep` | `layers-comps.aep` | `310-solid-100x100-pointcontrol` | 308 (NOT 309) + effect `ADBE Point Control`; AE read-back of the default Point = **[50,50]** = centre of the 100x100 layer (a 1920x1080 layer would give [960,540]; compare 074) |
| `311-position-default-explicit.aep` | `layers-comps.aep` | `311-position-default-explicit` | property.set Transform/Position [960,540,0] (was [960,540,0]) |
| `316-layer-stretch50.aep` | `keyframes.aep` | `316-layer-stretch50` | layer.set_timing stretch 100 -> 50 (AE: out point 5 -> 2.5, in 0, start 0) |
| `317-layer-stretch150.aep` | `keyframes.aep` | `317-layer-stretch150` | stretch 100 -> 150 (out point 7.5) |
| `318-layer-inpoint-0.3333.aep` | `layers-comps.aep` | `318-layer-inpoint-0.3333` | inPoint 0 -> 0.3333 (AE read-back: inPoint 0.33328125, outPoint 5.33328125, start 0, stretch 100; the requested 0.3333 was quantised by AE) |
| `319-precomp-timeremap.aep` | `footage-precomp.aep` | `319-precomp-timeremap` | SKIPPED on the solid (AE: "Can not set timeRemapEnabled ... canSetTimeRemapEnabled" for solids). Instead: L precomposed (moveAll) into `Pre` as in 124 (layer `Pre`, label 15) AND timeRemapEnabled = true on layer `Pre`; two variables vs base, one vs 124. AE read-back: Time Remap keys t=0 value 0 and t=5 value 5, linear/linear |
| `320-fx-layercontrol-self-explicit.aep` | `effects.aep` | `320-fx-layercontrol-self-explicit` | `ADBE Layer Control` added (default Layer read back 1) + Layer set explicitly to 1 (no-op) |
| `321-fx-layercontrol-none.aep` | `effects.aep` | `321-fx-layercontrol-none` | FROM 320: Layer 1 -> 0 (None; read back 0) |
| `323-fx-dropdown-5items.aep` | `effects.aep` | `323-fx-dropdown-5items` | `ADBE Dropdown Control` added (AE reports the effect matchName as `Pseudo/@@UNgGeIbLTy24Y7LB8Oar1Q`, display name `Dropdown Menu Control`), effect.set_dropdown_items ["Alpha","Beta","Gamma","Delta","Epsilon"]; read back value 1 / "Alpha" |
| `324-fx-dropdown-5items-select3.aep` | `effects.aep` | `324-fx-dropdown-5items-select3` | FROM 323: Menu set to 3 (read back value 3, valueText "Gamma"); NOTE the property matchName after the re-creation is `Pseudo/@@KbKT0rCPQi2A0ytuyGLWSw-0001` |
| `325-comp-pixelaspect-0.9091.aep` | `layers-comps.aep` | `325-comp-pixelaspect-0.9091` | pixelAspect 1 -> 0.9091 (AE read-back 0.90909090909091 = 10/11) |
| `326-comp-shutterphase-m90.aep` | `layers-comps.aep` | `326-comp-shutterphase-m90` | shutterPhase 0 -> -90 (read back -90; shutterAngle stays 180) |
| `327-comp-displaystartframe-100.aep` | `layers-comps.aep` | `327-comp-displaystartframe-100` | displayStartFrame 0 -> 100 (accepted without warning; comp.info does not report it, but the cdta bytes in the .aepx change vs 326: a (value,scale) pair 00000190 / 00000064 (= 400/100 = 4 s = 100 frames at 25 fps) appears in the tail, and the shutter phase `ffffffa6` of 326 is back to 0) |
| `328-mask-two-ellipse-rect.aep` | `masks.aep` | `328-mask-two-ellipse-rect` | mask.add ellipse 800x400 centre [960,540] (Mask 1, vertices as 232) + mask.add rect 400x300 centre [700,500] (Mask 2, vertices [500,350],[900,350],[900,650],[500,650] with float noise 499.999969482422 etc.); both mode Add (default) |
| `329-mask-ellipse-mode-none.aep` | `masks.aep` | `329-mask-ellipse-mode-none` | mask.add ellipse 800x400 centre [960,540] with mode None (compare 232 = same path, mode Add); AE offers no read-back of the mode via property.get (Mask Mode is not a listed property) |
| `330-shape-polystar-star-5-50-100.aep` | `shapes.aep` | `330-shape-polystar-star-5-50-100` | Polystar Path 1 starType star, Points 5, Outer Radius 100, Inner Radius 50 (Points/Inner/Outer read back as set; Type not read), roundness/rotation/position left default + Fill [0,1,0,1] |
| `331-shape-polystar-polygon-5-100.aep` | `shapes.aep` | `331-shape-polystar-polygon-5-100` | Polystar Path 1 starType polygon (Type read back = 2), Points 5, Outer Radius 100 + Fill [0,1,0,1] |
| `332-shape-gradient-fill-radial.aep` | `shapes.aep` | `332-shape-gradient-fill-radial` | Rectangle Path 400x300 + Gradient Fill 1 `ADBE Vector Graphic - G-Fill`, gradientType radial (Type read back 2), Start Point [0,0], End Point [200,0], Highlight Length/Angle 0, Scale [100,100], Rotation 0; colour stops default (not scriptable); compare 241 (linear, type 1) |
| `333-shape-group-two-rects.aep` | `shapes.aep` | `333-shape-group-two-rects` | Group 1 contents: Rectangle Path 1 size [400,300] + Rectangle Path 2 size [100,100] position [300,0] + Fill 1 [0,1,0,1] (order rect, rect, fill) |
| `334-text-box-right-align.aep` | `text.aep` | `334-text-box-right-align` | paragraph (box) text `T` "Hello", boxSize [400,200] (boxTextPos [-200,-100]), justification right (AE justification enum 7414; compare 245: left, 246: center) |
| `335-text-point-fauxbold.aep` | `text.aep` | `335-text-point-fauxbold` | point text "Hello" with fauxBold = true (fontStyle read back still "Regular"; font TimesNewRomanPSMT) |
| `336-text-point-fauxitalic.aep` | `text.aep` | `336-text-point-fauxitalic` | point text "Hello" (vs 244) with fauxItalic = true |
| `337-text-point-allcaps.aep` | `text.aep` | `337-text-point-allcaps` | point text "Hello" with allCaps = true (fontCapsOption) |
| `338-text-point-smallcaps.aep` | `text.aep` | `338-text-point-smallcaps` | point text "Hello" with smallCaps = true |
| `339-text-point-leading60.aep` | `text.aep` | `339-text-point-leading60` | point text "Hello" with leading 60 (autoLeading default read back leading 98 -> 60; read back 60; justification 7413 = left) |
| `340-text-range-two-styles.aep` | `styles.aep` | `340-text-range-two-styles` | point text "Hello" with two range styles in one call: characters [1,3) ("el") fill [1,0,0]; characters [3,5) ("lo") fill [0,0,1] and fontSize 60 (compare 248 = one range) |
| `341-camera-zoom-2000.aep` | `3d.aep` | `341-camera-zoom-2000` | camera layer `Camera 1` (default 50 mm; default Zoom read back 2666.6666666), Camera Options/Zoom set to 2000 (read back 2000) |
| `342-camera-dof-800-30.aep` | `3d.aep` | `342-camera-dof-800-30` | camera layer `Camera 1`, Camera Options/Depth of Field 0 -> 1, Focus Distance -> 800, Aperture -> 30 (all read back as set; Zoom stays default) |
| `343-light-spot-color-red.aep` | `3d.aep` | `343-light-spot-color-red` | spot light `Light 1` (defaults read back: Color [1,1,1,1], Cone Angle 90, Cone Feather 50) with Light Options/Color [1,0,0,1] |
| `344-light-spot-coneangle-60.aep` | `3d.aep` | `344-light-spot-coneangle-60` | spot light `Light 1`, Cone Angle 90 -> 60 (Color stays white; the spot light was created once as step 0 and kept for 343-345, each change undone before the next) |
| `345-light-spot-conefeather-30.aep` | `3d.aep` | `345-light-spot-conefeather-30` | spot light `Light 1`, Cone Feather 50 -> 30 |
| `347-marker-layer-label3.aep` | `markers.aep` | `347-marker-layer-label3` | marker.add_layer label 3; read back: label 3, eventCuePoint true, duration 0, strings empty, params {} |
| `348-marker-layer-navigation-cue.aep` | `markers.aep` | `348-marker-layer-navigation-cue` | cuePointName `CueN`, eventCuePoint = false (navigation cue); read back label 0, eventCuePoint false |
| `349-marker-layer-event-cue-params.aep` | `markers.aep` | `349-marker-layer-event-cue-params` | cuePointName `CueE`, eventCuePoint = true, params {"speaker":"A","n":"2"}; read back identical |
| `350-footage-wav-imported.aep` | `footage-precomp.aep` | `350-footage-wav-imported` | project.import_file test-tone.wav (name `test-tone.wav`, new footage item id 32, AE: width 0, height 0, duration 1, numItems 4); no layer |
| `351-footage-wav-as-layer.aep` | `footage-precomp.aep` | `351-footage-wav-as-layer` | FROM 350: layer.create_footage -> audio layer `test-tone.wav` on top of L in C (hasAudio true, hasVideo false, in 0, out 1, label 7); audio group not reported by the summary |
| `352-footage-pngseq-imported.aep` | `footage-precomp.aep` | `352-footage-pngseq-imported` | project.import_file seq_001.png with sequence=true -> footage item `seq_[001-003].png` (id 34, 64x64, duration 0.1 s = 3 frames at AE's default 30 fps); no layer |
| `354-footage-pngseq-timeremap.aep` | `footage-precomp.aep` | `354-footage-pngseq-timeremap` | FROM 353: timeRemapEnabled = true on the sequence layer; read back Time Remap keys t=0 value 0, t=0.1 value 0.1, linear/linear (this is the time-remap case that works on real footage; the solid in 319 cannot, so 319 uses a precomp) |
| `356-fx-tint-default.aep` | `effects.aep` | `356-fx-tint-default` | add `ADBE Tint` (default name "Tint", all parameters default) |
| `357-fx-tint-black-0.2-0.4-0.6.aep` | `effects.aep` | `357-fx-tint-black-0.2-0.4-0.6` | FROM 356: Map Black To [0,0,0] -> [0.2,0.4,0.6] (read back [0.20000000298023,0.40000000596046,0.60000002384186,1]) |
| `361-fx-tint-amount-0.aep` | `effects.aep` | `361-fx-tint-amount-0` | FROM 356: Amount to Tint 100 -> 0 |
| `363-fx-tint-amount-2key.aep` | `effects.aep` | `363-fx-tint-amount-2key` | FROM 356: Amount to Tint keyed t=0 -> 20, t=2 -> 80 (linear; value at t=1 read back 50) |
| `364-fx-tint-white-2key.aep` | `effects.aep` | `364-fx-tint-white-2key` | FROM 356: Map White To keyed t=0 -> [1,1,0], t=2 -> [0,0,1] (linear; value at t=1 read back [0.5,0.5,0.5,1]) |
| `366-fx-blur-then-tint.aep` | `effects.aep` | `366-fx-blur-then-tint` | FROM 365: effect.move Gaussian Blur 2 -> 1 (stack Gaussian Blur, Tint; read back order ["Gaussian Blur","Tint"]) |
| `368-fx-ramp-default.aep` | `effects.aep` | `368-fx-ramp-default` | add `ADBE Ramp` (default name "Gradient Ramp", all parameters default) |
| `369-fx-ramp-start-100-200.aep` | `effects.aep` | `369-fx-ramp-start-100-200` | FROM 368: Start of Ramp [960,0] -> [100,200] |
| `371-fx-ramp-startcolor-red.aep` | `effects.aep` | `371-fx-ramp-startcolor-red` | FROM 368: Start Color [0,0,0] -> [1,0,0]. |
| `374-fx-ramp-end-2000-m100.aep` | `effects.aep` | `374-fx-ramp-end-2000-m100` | FROM 368: End of Ramp [960,1080] -> [2000,-100] (outside the layer) |
| `377-fx-ramp-shape-radial.aep` | `effects.aep` | `377-fx-ramp-shape-radial` | FROM 368: Ramp Shape 1 -> 2 (Radial Ramp; read back 2) |
| `378-fx-ramp-scatter-12.aep` | `effects.aep` | `378-fx-ramp-scatter-12` | FROM 368: Ramp Scatter 0 -> 12 |
| `380-fx-ramp-blend-25pct.aep` | `effects.aep` | `380-fx-ramp-blend-25pct` | FROM 368: Blend With Original 0 -> 0.25 (= 25 %; a first attempt with 25 was rejected by AE and changed nothing) |
| `382-fx-ramp-start-2key.aep` | `effects.aep` | `382-fx-ramp-start-2key` | FROM 368: Start of Ramp keyed t=0 -> [100,100], t=2 -> [1800,900] (linear; value at t=1 read back [950,500]) |
| `383-fx-ramp-on-100x100-start-10-20.aep` | `effects.aep` | `383-fx-ramp-on-100x100-start-10-20` | FROM 368 (L keeps its DEFAULT Gradient Ramp): TWO changes in one call: NEW solid layer `S100` 100x100 white [1,1,1] on top (new footage item `S100`, layer index 1) AND a Gradient Ramp on S100 with Start of Ramp -> [10,20]; AE read-back of the S100 ramp's default End of Ramp = [50,100] (relative to the 100x100 layer). Two Ramp instances of the same type on two layers |
| `385-text-anchor-10-20.aep` | `text.aep` | `385-text-anchor-10-20` | FROM 384: text layer T Anchor Point -> [10,20]. Stored as cdat f64 pixels [10, 20] (not a fraction of a source size): confirms the text-anchor rule (was H) |
| `386-text-etx-A-B.aep` | `text.aep` | `386-text-etx-A-B` | FROM 384 (anchor undone): T text -> "A" U+0003 "B". Stored as UTF-16 U+0003. AE text.measure: 2 composed lines, 2 paragraphs, baselines at y 0 and 98 – exactly the same layout as 387 (CR): AE treats U+0003 as a line break |
| `389-fx-exposure-default.aep` | `effects.aep` | `389-fx-exposure-default` | add `ADBE Exposure2` (default name "Exposure", all parameters default) |
| `390-fx-exposure-channels-individual.aep` | `effects.aep` | `390-fx-exposure-channels-individual` | FROM 389: Channels 1 -> 2 (Individual Channels; read back 2) |
| `391-fx-exposure-master-1.5.aep` | `effects.aep` | `391-fx-exposure-master-1.5` | FROM 389: Exposure 0 -> 1.5 (Master Exposure `-0003`) |
| `393-fx-exposure-offset-0.1.aep` | `effects.aep` | `393-fx-exposure-offset-0.1` | FROM 389: Offset 0 -> 0.1 (Master Offset `-0004`; read back 0.10000000149012) |
| `394-fx-exposure-gamma-2.2.aep` | `effects.aep` | `394-fx-exposure-gamma-2.2` | FROM 389: Gamma Correction 1 -> 2.2 (Master `-0005`; read back 2.20000004768372) |
| `395-fx-exposure-individual-red-1.aep` | `effects.aep` | `395-fx-exposure-individual-red-1` | FROM 389: TWO changes in one call: Channels 1 -> 2 AND Red Exposure 0 -> 1 (`-0008`) |
| `396-fx-exposure-bypass-on.aep` | `effects.aep` | `396-fx-exposure-bypass-on` | FROM 389: Bypass Linear Light Conversion 0 -> 1 (`-0022`) |
| `397-fx-exposure-master-2key.aep` | `effects.aep` | `397-fx-exposure-master-2key` | FROM 389: Exposure keyed t=0 -> -1, t=2 -> 2 (linear; value at t=1 read back 0.5) |
| `400-text-autoleading-on.aep` | `text.aep` | `400-text-autoleading-on` | FROM 399: T (text "A" CR "B", size 100, explicit leading 98) Auto Leading -> ON. AE text.measure: second baseline 120 px below the first (= 1.2 x size 100), was 98 in 387/399 |
| `403-matte-L-alpha-M.aep` | `layers-comps.aep` | `403-matte-L-alpha-M` | FROM 402: L (index 2) track matte = layer M, type alpha (`AVLayer.setTrackMatte`); AE switched M's video off by itself (comp.info: M enabled false) |
| `404-matte-L-alphainv-M.aep` | `layers-comps.aep` | `404-matte-L-alphainv-M` | FROM 402: L track matte = M, alpha inverted (M video off by AE) |
| `405-matte-L-luma-M.aep` | `layers-comps.aep` | `405-matte-L-luma-M` | FROM 402: L track matte = M, luma (M video off by AE) |
| `406-matte-L-lumainv-M.aep` | `layers-comps.aep` | `406-matte-L-lumainv-M` | FROM 402: L track matte = M, luma inverted (M video off by AE) |
| `407-matte-M-alpha-L-below.aep` | `layers-comps.aep` | `407-matte-M-alpha-L-below` | FROM 402: M (index 1) track matte = L (index 2, BELOW the target), alpha (L video off by AE) |
| `408-matte-L-alpha-M-visible.aep` | `layers-comps.aep` | `408-matte-L-alpha-M-visible` | FROM 402: TWO changes in one call: L track matte = M alpha AND M video switch back on (comp.info: M enabled true) |
| `410-fx-invert-channel-2.aep` | `effects.aep` | `410-fx-invert-channel-2` | FROM 409: Channel 1 -> 2 |
| `411-fx-invert-blend-50.aep` | `effects.aep` | `411-fx-invert-blend-50` | FROM 409: Blend With Original 0 -> 50 (percent; accepted as 50) |
| `413-fx-fastbox-radius-20.aep` | `effects.aep` | `413-fx-fastbox-radius-20` | FROM 412: Blur Radius 0 -> 20 |
| `414-fx-fastbox-iterations-1.aep` | `effects.aep` | `414-fx-fastbox-iterations-1` | FROM 412: Iterations 3 -> 1 |
| `416-fx-fastbox-repeatedge-off.aep` | `effects.aep` | `416-fx-fastbox-repeatedge-off` | FROM 412: Repeat Edge Pixels 1 -> 0 |
| `418-fx-gaussian-dimensions-3.aep` | `effects.aep` | `418-fx-gaussian-dimensions-3` | FROM 417: Blur Dimensions 1 -> 3 |
| `419-fx-gaussian-repeatedge-off.aep` | `effects.aep` | `419-fx-gaussian-repeatedge-off` | FROM 417: Repeat Edge Pixels 1 -> 0 |
| `422-fx-dropshadow-opacity-204.aep` | `effects.aep` | `422-fx-dropshadow-opacity-204` | FROM 420: Opacity 127.5 -> 204 (scripting scale 0..255; 204 = 80 %) |
| `426-fx-dropshadow-shadowonly-on.aep` | `effects.aep` | `426-fx-dropshadow-shadowonly-on` | FROM 420: Shadow Only 0 -> 1 |
| `427-fx-glow-default.aep` | `effects.aep` | `427-fx-glow-default` | (Drop Shadow removed) add `ADBE Glo2` "Glow" (defaults read back: Glow Based On 2, Glow Threshold 153, Glow Radius 10, Glow Intensity 1, Composite Original 2, Glow Operation 3, Glow Colors 1, Color Looping 3, Color Loops 1, Color Phase 0, A & B Midpoint 0.5, Color A [1,1,1,1], Color B [0,0,0,1], Glow Dimensions 1) |
| `429-fx-glow-threshold-76.5.aep` | `effects.aep` | `429-fx-glow-threshold-76.5` | FROM 427: Glow Threshold 153 -> 76.5 (scripting scale 0..255) |
| `438-fx-glow-midpoint-0.25.aep` | `effects.aep` | `438-fx-glow-midpoint-0.25` | FROM 427: A & B Midpoint 0.5 -> 0.25 (scripting scale 0..1) |
| `444-fx-huesat-hue-90.aep` | `effects.aep` | `444-fx-huesat-hue-90` | FROM 443: Master Hue 0 -> 90 (the undo afterwards did NOT revert it; 445-450 were each made on a fresh reopen of a copy of 443, verified by property.get) |
| `446-fx-huesat-light-m30.aep` | `effects.aep` | `446-fx-huesat-light-m30` | FROM 443 (reopened): Master Lightness 0 -> -30 |
| `448-fx-huesat-colorizehue-120.aep` | `effects.aep` | `448-fx-huesat-colorizehue-120` | FROM 443 (reopened): Colorize Hue 0 -> 120 |
| `451-shape-ellipse-400x300-fill.aep` | `shapes.aep` | `451-shape-ellipse-400x300-fill` | NEW shape layer `S` on top: Group 1 = Ellipse Path 1 (`ADBE Vector Shape - Ellipse`, Size [400,300], Position default [0,0]) + Fill 1 colour [0,1,0,1]; NOT undone: base of 452-484 |
| `457-shape-group-scale-50-80.aep` | `shapes.aep` | `457-shape-group-scale-50-80` | FROM 451: Group 1 Transform Scale [100,100] -> [50,80] |
| `462-shape-layer-anchor-10-20.aep` | `shapes.aep` | `462-shape-layer-anchor-10-20` | FROM 451: layer S Transform Anchor Point [0,0,0] -> [10,20] (read back [10,20,0]) |
| `464-shape-stroke-opacity-50.aep` | `shapes.aep` | `464-shape-stroke-opacity-50` | FROM 463: Stroke Opacity 100 -> 50 |
| `467-shape-trim-end-70.aep` | `shapes.aep` | `467-shape-trim-end-70` | FROM 465: Trim End 100 -> 70 |
| `469-shape-trim-type-2.aep` | `shapes.aep` | `469-shape-trim-type-2` | FROM 465: Trim Multiple Shapes 1 -> 2 |
| `471-shape-rect-roundness-30.aep` | `shapes.aep` | `471-shape-rect-roundness-30` | FROM 470: Rect Roundness 0 -> 30 |
| `473-shape-star-points-6.aep` | `shapes.aep` | `473-shape-star-points-6` | FROM 472: Points 5 -> 6 |
| `482-shape-two-groups.aep` | `shapes.aep` | `482-shape-two-groups` | FROM 451: NEW Group 2 (top level) = Rectangle Path 1 [200,100] + Fill 1 [0,0,1,1] (one call) |
| `483-shape-nested-group-rect.aep` | `shapes.aep` | `483-shape-nested-group-rect` | FROM 451: TWO calls: a Group added INSIDE Group 1 (after Fill 1, named "Group 1") and a Rectangle Path 1 [100,50] inside it |
| `485-layer-3d-on.aep` | `3d.aep` | `485-layer-3d-on` | FROM base: L threeDLayer on (compare 015); NOT undone: base of 486-488 |
| `486-layer-3d-rotx-30.aep` | `3d.aep` | `486-layer-3d-rotx-30` | FROM 485: X Rotation 0 -> 30 |
| `487-layer-3d-roty-45.aep` | `3d.aep` | `487-layer-3d-roty-45` | FROM 485: Y Rotation 0 -> 45 |
| `488-layer-3d-orientation-10-20-30.aep` | `3d.aep` | `488-layer-3d-orientation-10-20-30` | FROM 485: Orientation [0,0,0] -> [10,20,30] |
| `509-marker-comp-two.aep` | `markers.aep` | `509-marker-comp-two` | FROM base: TWO comp markers in one call: t=1 s comment `M1` label 2; t=2.5 s comment `M2` duration 0.5 (read back as set) |
| `510-marker-layer-0.52-dur0.3.aep` | `markers.aep` | `510-marker-layer-0.52-dur0.3` | FROM base (509 undone, comp markers 0): layer marker on L at t=0.52 s, comment `LM`, duration 0.3 s (read back 0.52 / 0.3) |
| `512-fx-levels-inblack-0.1.aep` | `effects.aep` | `512-fx-levels-inblack-0.1` | FROM 511: Input Black 0 -> 0.1 (read back 0.10000000149012); the undo did NOT revert it; 513-516 were each made on a fresh reopen of a copy of 511 |
| `514-fx-levels-gamma-2.aep` | `effects.aep` | `514-fx-levels-gamma-2` | FROM 511 (reopened): Gamma 1 -> 2 |
| `516-fx-levels-outwhite-0.9.aep` | `effects.aep` | `516-fx-levels-outwhite-0.9` | FROM 511 (reopened): Output White 1 -> 0.9 (read back 0.89999997615814) |
| `517-text-box-600x400-wrap.aep` | `text.aep` | `517-text-box-600x400-wrap` | FROM base: NEW box text layer `T`, box [600,400] at [-300,-200], text `The quick brown fox jumps over the lazy dog and keeps running far away` (Times New Roman, default style). AE text.measure: 4 composed lines `The quick brown fox ` / `jumps over the lazy dog ` / `and keeps running far ` / `away`, baselines y -163.80, -65.80, 32.20, 130.20 (left x -300), no overflow |
| `519-shape-all-items-default.aep` | `shapes.aep` | `519-shape-all-items-default` | NEW shape layer `S`: Group 1 with, in this order, `property.add` of Rectangle Path, Ellipse Path, Polystar Path, Fill, Stroke, Gradient Fill, Gradient Stroke, Trim Paths, Round Corners, nothing set. AE read-back of the defaults: Rect Size [100,100], Position [0,0], Roundness 0, Shape Direction 1; Ellipse Size [100,100], Position [0,0]; Star Type 1, Points 5, Position [0,0], Rotation 0, Inner Radius 50, Outer Radius 100, Inner/Outer Roundness 0; Fill Blend Mode 1, Composite 1, Fill Rule 1, Color [1,0,0,1], Opacity 100; Stroke Color [1,1,1,1], Opacity 100, Width 2, Line Cap 1, Line Join 1, Miter Limit 4, Dashes Dash/Gap 1-3 = 10, Offset 0, Taper Length Units 1, lengths/widths/eases 0, Wave Amount 0, Units 1, Wavelength 100, Cycles 10, Phase 0; Gradient Fill/Stroke Type 1, Start Point [0,0], End Point [100,0], Highlight Length/Angle 0, Scale [100,100], Rotation 0, Colors NO_VALUE, Opacity 100 (stroke width 2, cap/join 1, miter 4); Trim Start 0, End 100, Offset 0, Trim Multiple Shapes 1; Round Corners Radius 10; Group Blend Mode 1, Transform Anchor/Position [0,0], Scale [100,100], Skew/Skew Axis/Rotation 0, Opacity 100; layer Anchor [0,0,0], Position [960,540,0] |
| `526-text-point-HHHH-default.aep` | `text.aep` | `526-text-point-HHHH-default` | NEW point text layer `T` `HHHH` (layer.create_text, nothing else set: 100 px, the default character state). AE: style slot /13 = 1 (superscript); text.measure width 168.4 px, baseline -33.3 (58.3 % size, 33.3 % shift) |
| `527-text-point-HHHH-baseline-normal.aep` | `text.aep` | `527-text-point-HHHH-baseline-normal` | T fontBaselineOption = normal (text.set_style): slot /13 = 0 (the only changed slot); width 288.9 px, baseline 0 |
| `528-text-point-HHHH-baseline-subscript.aep` | `text.aep` | `528-text-point-HHHH-baseline-subscript` | T fontBaselineOption = subscript (text.set_style): slot /13 = 2; width 168.4 px, baseline +33.3 |
| `529-fx-timewarp-default.aep` | `effects.aep` | `529-fx-timewarp-default` | Timewarp (`ADBE Timewarp`) on L at its defaults; AE read back Method 3 (Pixel Motion), Adjust Time By 1 (Speed), Speed 50, Source Frame 0; NOT undone: base of 530-533 |
| `530-fx-timewarp-speed-25.aep` | `effects.aep` | `530-fx-timewarp-speed-25` | Speed 50 -> 25 (read back 25); undone |
| `531-fx-timewarp-speed-keyed-50-200.aep` | `effects.aep` | `531-fx-timewarp-speed-keyed-50-200` | Speed keys 50 at 0 s and 200 at 2 s (linear; read back 125 at 1 s); undone |
| `532-fx-timewarp-method-1.aep` | `effects.aep` | `532-fx-timewarp-method-1` | Method 3 -> 1 (Whole Frames); undone |
| `533-fx-timewarp-sourceframe-12.aep` | `effects.aep` | `533-fx-timewarp-sourceframe-12` | Adjust Time By 2 (Source Frame) and Source Frame 12 (one batch); undone, and the Timewarp undone |
| `534-fx-slider-expression-time10.aep` | `effects.aep` | `534-fx-slider-expression-time10` | Slider Control on L, expression `time*10` on Slider (read back 20 at 2 s); undone |
| `535-camera-position-on-3d-rotx.aep` | `3d.aep` | `535-camera-position-on-3d-rotx` | NEW camera `K` (layer.create_camera, two-node), Position [100,200,-1000] (Point of Interest read back [960,540,0], Zoom 2666.67) |
| `536-camera-one-node-position-on-3d-rotx.aep` | `3d.aep` | `536-camera-one-node-position-on-3d-rotx` | K autoOrient off (one-node) |
| `537-camera-default-on-3d-rotx.aep` | `3d.aep` | `537-camera-default-on-3d-rotx` | NEW camera `K` at its defaults (Position read back [0,0,-2666.67]); 255 (POI [960,540,0], Position [0,0,-1333.33]) |
| `538-text-point-HHHH-normal-ref.aep` | `text.aep` | `538-text-point-HHHH-normal-ref` | NEW point text `T` `HHHH` (layer.create_text) + fontBaselineOption normal in one batch (= the 527 state); noise reference of S17, NOT undone: base of 539-578 |
| `539-text-hscale-0.9.aep` | `text.aep` | `539-text-hscale-0.9` | horizontalScale 0.9: style /6 1.0 -> 0.9 |
| `540-text-vscale-1.5.aep` | `text.aep` | `540-text-vscale-1.5` | verticalScale 1.5: style /7 -> 1.5 |
| `541-text-baselineshift-41.aep` | `text.aep` | `541-text-baselineshift-41` | baselineShift 41: style /9 0.0 -> 41.0 |
| `542-text-autokern-none.aep` | `text.aep` | `542-text-autokern-none` | autoKernType none: style /11 1 -> 0 |
| `543-text-autokern-optical.aep` | `text.aep` | `543-text-autokern-optical` | autoKernType optical: style /11 1 -> 2 (1 = metrics, the default) |
| `544-text-applystroke-on.aep` | `text.aep` | `544-text-applystroke-on` | applyStroke true: style /57 false -> true |
| `545-text-strokewidth-5.aep` | `text.aep` | `545-text-strokewidth-5` | strokeWidth 5: style /63 1.0 -> 5.0 |
| `546-text-applyfill-off.aep` | `text.aep` | `546-text-applyfill-off` | applyFill false: style /56 true -> false |
| `548-text-linejoin-round.aep` | `text.aep` | `548-text-linejoin-round` | lineJoinType round: style /62 0 -> 1 |
| `550-text-strokeoverfill-off.aep` | `text.aep` | `550-text-strokeoverfill-off` | strokeOverFill false: style /58 true -> false |
| `552-text-nobreak-on.aep` | `text.aep` | `552-text-nobreak-on` | noBreak true: style /52 -> true |
| `553-text-tsume-0.5.aep` | `text.aep` | `553-text-tsume-0.5` | tsume 0.5: style /36 0.0 -> 0.5 |
| `554-text-kerning-50.aep` | `text.aep` | `554-text-kerning-50` | kerning 50: style /11 1 -> 0 and the run gets a kern list `RUN/0/8/0[i]` = << /0 << /0 50 >> /1 1 >> per character (`RUN/0/7` = 50) |
| `555-text-ligature-on.aep` | `text.aep` | `555-text-ligature-on` | ligature true: style /18 -> true |
| `556-text-autohyphenate-off.aep` | `text.aep` | `556-text-autohyphenate-off` | autoHyphenate false: paragraph /9 true -> false |
| `558-text-hangingroman-on.aep` | `text.aep` | `558-text-hangingroman-on` | hangingRoman true: paragraph /21 -> true |
| `559-text-everyline-on.aep` | `text.aep` | `559-text-everyline-on` | everyLineComposer true: paragraph /29 false -> true |
| `560-text-firstlineindent-20.aep` | `text.aep` | `560-text-firstlineindent-20` | firstLineIndent 20: paragraph /1 -> 20.0 |
| `561-text-startindent-30.aep` | `text.aep` | `561-text-startindent-30` | startIndent 30: paragraph /2 -> 30.0 |
| `562-text-endindent-40.aep` | `text.aep` | `562-text-endindent-40` | endIndent 40: paragraph /3 -> 40.0 |
| `563-text-spacebefore-10.aep` | `text.aep` | `563-text-spacebefore-10` | spaceBefore 10: paragraph /4 -> 10.0 |
| `564-text-spaceafter-12.aep` | `text.aep` | `564-text-spaceafter-12` | spaceAfter 12: paragraph /5 -> 12.0 |
| `565-text-justify-fullleft.aep` | `text.aep` | `565-text-justify-fullleft` | justification fullLeft: paragraph /0 0 -> 3 |
| `566-text-justify-fullcenter.aep` | `text.aep` | `566-text-justify-fullcenter` | fullCenter: /0 -> 5 |
| `567-text-justify-fullright.aep` | `text.aep` | `567-text-justify-fullright` | fullRight: /0 -> 4 |
| `568-text-justify-fullall.aep` | `text.aep` | `568-text-justify-fullall` | fullLastLineFull: /0 -> 6 |
| `569-text-direction-rtl.aep` | `text.aep` | `569-text-direction-rtl` | direction rightToLeft: paragraph /33 0 -> 1 |
| `571-text-digitset-arabic.aep` | `text.aep` | `571-text-digitset-arabic` | digitSet arabic: style /70 0 -> 1. Same session, not saved: text `1234` has the same layer.bounds with digitSet default and arabic (left 11.72, width 184.81, top -67.58, height 68.75); undone |
| `572-text-leadingtype-japanese.aep` | `text.aep` | `572-text-leadingtype-japanese` | leadingType japanese: paragraph /8 0 -> 1 |
| `573-text-baselinedir-rotated.aep` | `text.aep` | `573-text-baselinedir-rotated` | baselineDirection verticalRotated: style /35 1 -> 2 |
| `575-text-vertical-rtl.aep` | `text.aep` | `575-text-vertical-rtl` | lineOrientation verticalRtl: document `/0/8/0[0]/0/2/1` -> 2 (absent before) |
| `576-text-baselinedir-crossstream.aep` | `text.aep` | `576-text-baselinedir-crossstream` | baselineDirection verticalCrossStream: style /35 -> 3 |
| `578-text-digitset-hindi.aep` | `text.aep` | `578-text-digitset-hindi` | digitSet hindi: style /70 -> 2 |
| `579-textanim-position-0-m50.aep` | `text.aep` | `579-textanim-position-0-m50` | (538) text.add_animator `Animator 1` with Position 3D [0,-50,0], range selector at its defaults; NOT undone: base of 580-591 |
| `580-textanim-start-25.aep` | `text.aep` | `580-textanim-start-25` | Range Selector 1 Start (ADBE Text Percent Start) 25 |
| `581-textanim-end-75.aep` | `text.aep` | `581-textanim-end-75` | End 75 |
| `582-textanim-offset-10.aep` | `text.aep` | `582-textanim-offset-10` | Offset 10 |
| `583-textanim-units-index.aep` | `text.aep` | `583-textanim-units-index` | Advanced Units 2 (Index); read back Index Start 0, Index End 4 (the character count) |
| `584-textanim-basedon-3-words.aep` | `text.aep` | `584-textanim-basedon-3-words` | Based On (ADBE Text Range Type2) 3 |
| `585-textanim-mode-2-subtract.aep` | `text.aep` | `585-textanim-mode-2-subtract` | Mode 2 |
| `586-textanim-amount-50.aep` | `text.aep` | `586-textanim-amount-50` | Amount (ADBE Text Selector Max Amount) 50 |
| `587-textanim-shape-2-rampup.aep` | `text.aep` | `587-textanim-shape-2-rampup` | Shape 2 |
| `588-textanim-smoothness-50.aep` | `text.aep` | `588-textanim-smoothness-50` | Smoothness 50 |
| `589-textanim-easehigh-30.aep` | `text.aep` | `589-textanim-easehigh-30` | Ease High (ADBE Text Levels Max Ease) 30 |
| `590-textanim-easelow-40.aep` | `text.aep` | `590-textanim-easelow-40` | Ease Low (ADBE Text Levels Min Ease) 40 |
| `591-textanim-randomize-seed-7.aep` | `text.aep` | `591-textanim-randomize-seed-7` | Randomize Order 1 + Random Seed 7 (one batch) |
| `592-textanim-many-props.aep` | `text.aep` | `592-textanim-many-props` | FROM 579: second animator `Many`: Anchor Point 3D [3,4,0], Scale 3D [150,120,100], Skew 10, Skew Axis 20, Rotation 45, Opacity 37, Fill Color [0,1,0], Stroke Color [0,0,1], Stroke Width 3, Tracking Amount 20 (AE added Track Type itself; setting it failed), Line Spacing [10,20], Blur [5,6] (read back) |
| `593-textanim-start-keyed-ease-0-100.aep` | `text.aep` | `593-textanim-start-keyed-ease-0-100` | FROM 592: Animator 1 Start keys 0 at 0 s, 100 at 2 s, interp ease (keyframe.apply) |
| `594-textanim-position-start-keyed-ease.aep` | `text.aep` | `594-textanim-position-start-keyed-ease` | 593 with animator `Many` removed (Animator 1 only: Position [0,-50,0], Start 0 -> 100 eased over 0..2 s) |
| `595-text-stroke-scale-shift-combined.aep` | `text.aep` | `595-text-stroke-scale-shift-combined` | 538 state (Animator 1 removed) + applyStroke, strokeWidth 5, strokeColor blue, horizontalScale 0.9, verticalScale 1.5, baselineShift 41 (one call) |
| `596-text-combined-fill-over-stroke.aep` | `text.aep` | `596-text-combined-fill-over-stroke` | 595 + strokeOverFill false |
| `597-key-anchor-ease-out50-40-in30-60.aep` | `keyframes.aep` | `597-key-anchor-ease-out50-40-in30-60` | L Anchor Point keys [960,540] at 0 s (out speed 50, influence 40) and [1160,640] at 2 s (in speed 30, influence 60), keyframe.apply ease; read back (re-applied in the same session): 0.5 s [1004.4121, 562.2061], 1 s [1084.8353, 602.4177], 1.5 s [1137.1339, 628.5670] |
| `598-key-ramp-point-ease-color-bezier.aep` | `keyframes.aep` | `598-key-ramp-point-ease-color-bezier` | Gradient Ramp on L: Start of Ramp keys [100,200] at 0 s (out 50 / 40) and [500,400] at 2 s (in 30 / 60): read back 0.5 s [178.4012, 239.2006], 1 s [353.0563, 326.5281], 1.5 s [462.9131, 381.4566]; Start Color keys red at 0 s and blue at 2 s switched to bezier by keyframe.set_interpolation (a custom colour ease is refused by AE: "Value array does not have 1 elements"): read back 0.5 s [0.75,0,0.25], 1 s [0.5,0,0.5], 1.5 s [0.25,0,0.75] |
| `599-key-mask-path-ease-infl75.aep` | `masks.aep` | `599-key-mask-path-ease-infl75` | L rect mask, path keys 200x200 at 0 s and 600x400 at 2 s (both centred), ease speed 0, out influence 75 on key 1, in influence 75 on key 2; first vertex read back 0.5 s [849.1606, 434.5803], 1 s [760.0, 390.0], 1.5 s [670.8394, 345.4197] |
| `601-precomp-two-comps-in-C.aep` | `footage-precomp.aep` | `601-precomp-two-comps-in-C` | FROM 600: new comp `Q` 320x240, 25 fps, 2 s with a blue 320x240 solid `B320`, Q added as a layer on top in C (C now shows P30 and Q) |
| `603-footage-exrseq-0101-as-layer.aep` | `footage-precomp.aep` | `603-footage-exrseq-0101-as-layer` | project.import_file `test-media\exrseq\shot_0101.exr` sequence=true -> item `shot_[0101-0103].exr` (64x48, 0.1 s = 3 frames at 30 fps) + layer on top of L in C |
| `610-fx-tint-named-My-Tint.aep` | `effects.aep` | `610-fx-tint-named-My-Tint` | effect.add `ADBE Tint` on L with name `My Tint` (effect.list_on_layer: name `My Tint`) |
| `621-settings-bpc-8.aep` | `settings-a.aep` | `621-settings-bpc-8` | project.set_settings bitsPerChannel 8 |
| `622-settings-bpc-16.aep` | `settings-c.aep` | `622-settings-bpc-16` | bitsPerChannel 16 |
| `623-settings-workingspace-none.aep` | `settings-a.aep` | `623-settings-workingspace-none` | workingSpace '' (read back None) |
| `625-settings-gamma-2.4.aep` | `settings-a.aep` | `625-settings-gamma-2.4` | workingGamma 2.4 |
| `628-settings-scene-compensation-off.aep` | `settings-a.aep` | `628-settings-scene-compensation-off` | compensateForSceneReferredProfiles false |
| `629-settings-time-display-frames.aep` | `settings-a.aep` | `629-settings-time-display-frames` | timeDisplayType frames |
| `630-settings-frame-start-1.aep` | `settings-a.aep` | `630-settings-frame-start-1` | displayStartFrame 1 |
| `632-settings-linearize-and-blend-linear.aep` | `settings-b.aep` | `632-settings-linearize-and-blend-linear` | linearizeWorkingSpace + linearBlending true |
| `633-settings-feet-frames-35.aep` | `settings-b.aep` | `633-settings-feet-frames-35` | timeDisplayType frames + framesUseFeetFrames true (film type mm35, read back) |
| `634-settings-feet-frames-16.aep` | `settings-c.aep` | `634-settings-feet-frames-16` | FROM 633 (same session, not reopened): feetFramesFilmType mm16 |
| `641-stretch-200-keys-position-1s-2s.aep` | `keyframes.aep` | `641-stretch-200-keys-position-1s-2s` | layer.set_timing stretch 200 on L, then the same two keys at comp 1 s and 2 s (read back: out 10, keys at 1 and 2) |
| `642-stretch-50-keys-position-1s-2s.aep` | `keyframes.aep` | `642-stretch-50-keys-position-1s-2s` | stretch 50 (out 2.5), keys at comp 1 s and 2 s |
| `643-stretch-200-start-1-keys-position-2s-3s.aep` | `keyframes.aep` | `643-stretch-200-start-1-keys-position-2s-3s` | startTime 1 + stretch 200 (in 1, out 11), keys at comp 2 s and 3 s |
| `645-footage-mp4-conform-30.aep` | `footage-precomp.aep` | `645-footage-mp4-conform-30` | FROM 644: footage.interpret conformFrameRate 30 on clip.mp4 (25 fps native; duration 1 -> 0.8333 s) |
| `650-footage-tga-still-as-layer.aep` | `footage-precomp.aep` | `650-footage-tga-still-as-layer` | (fresh base) import `test-media\still.tga` (64x48, PIL from still.png) + layer in C |
| `651-footage-psd-flat-as-layer.aep` | `footage-precomp.aep` | `651-footage-psd-flat-as-layer` | import `test-media\still.psd` (flat RGB PSD written by hand) importAs footage + layer in C |
| `652-footage-ai-as-layer.aep` | `footage-precomp.aep` | `652-footage-ai-as-layer` | import `test-media\still.ai` (hand-written one-page PDF named .ai, 64x48 pt) importAs footage + layer in C |
| `653-footage-eps-as-layer.aep` | `footage-precomp.aep` | `653-footage-eps-as-layer` | import `test-media\still.eps` (hand-written EPS, 64x48 pt) + layer in C |
| `700-key-mask-path-ease-out20-in50.aep` | `masks.aep` | `700-key-mask-path-ease-out20-in50` | from 599: out influence 20 on key 1, in influence 50 on key 2 (speed 0); first vertex 0.5 s [803.9844, 411.9922], 1 s [727.6194, 373.8097], 1.5 s [676.9510, 348.4755] |
| `701-light-parallel-int80-color.aep` | `3d.aep` | `701-light-parallel-int80-color` | NEW parallel light `P`, Intensity 80, Color [1,0.5,0.25] |
| `702-light-ambient-int50-color.aep` | `3d.aep` | `702-light-ambient-int50-color` | NEW ambient light `A`, Intensity 50, Color [0.25,0.5,1] |
| `703-key-orientation-0-to-10-20-30-linear.aep` | `3d.aep` | `703-key-orientation-0-to-10-20-30-linear` | L 3D, Orientation keys [0,0,0] at 0 s and [10,20,30] at 2 s (linear): read back 0.5 s [3.4341, 4.4691, 7.7129], 1 s [6.2643, 9.3531, 15.2190], 1.5 s [8.4650, 14.5598, 22.6095] (not per-component linear) |
| `704-light-environment-default.aep` | `3d.aep` | `704-light-environment-default` | NEW environment light `E` at its defaults |
| `705-mask-on-text-rect-40-m70-160-10.aep` | `text.aep` | `705-mask-on-text-rect-40-m70-160-10` | NEW point text `T` `HHHH` (normal baseline) with a rect mask (mask.add centre [100,-30], size [120,80]): vertices read back [40,-70] [160,-70] [160,10] [40,10] (layer space) |
| `706-mask-on-shape-rect-0-m150-100-150.aep` | `masks.aep` | `706-mask-on-shape-rect-0-m150-100-150` | NEW shape layer `S` (Group 1: Rectangle 400x200, Fill green) with a rect mask centre [50,0] size [100,300]: vertices [0,-150] [100,-150] [100,150] [0,150] |
| `707-shape-merge-subtract-rect-ellipse.aep` | `shapes.aep` | `707-shape-merge-subtract-rect-ellipse` | S without the mask: Rectangle, Ellipse 200x200, Merge Paths mode subtract (read back Merge Type 3), Fill (moved last) |
| `708-shape-merge-intersect-rect-ellipse.aep` | `shapes.aep` | `708-shape-merge-intersect-rect-ellipse` | 707 with Merge Type 4 |
| `709-shape-repeater-3-pos150-60-rot10-scale90-80-op40.aep` | `shapes.aep` | `709-shape-repeater-3-pos150-60-rot10-scale90-80-op40` | S: Rectangle, Fill, Repeater (Copies 3, Position [150,60], Rotation 10, Scale [90,80], End Opacity 40) |
| `710-shape-offset-paths-20-round.aep` | `shapes.aep` | `710-shape-offset-paths-20-round` | S: Rectangle, Offset Paths (Amount 20, Line Join round), Fill |
| `711-fx-directional-blur-30-40-on-M.aep` | `effects.aep` | `711-fx-directional-blur-30-40-on-M` | Directional Blur (ADBE Motion Blur) on M: Direction 30, Blur Length 40 |
| `712-fx-eleven-effects-defaults-on-M.aep` | `effects.aep` | `712-fx-eleven-effects-defaults-on-M` | M with, in this order, Radial Blur, CC Radial Blur, Venetian Blinds, Channel Blur, Black & White, Tritone, Luma Key, Simple Choker, Matte Choker, Transform (ADBE Geometry2), Set Matte (ADBE Set Matte3), all at their defaults |
| `713-fx-radial-blur-zoom-20-on-M.aep` | `effects.aep` | `713-fx-radial-blur-zoom-20-on-M` | Radial Blur: Amount 20, Center [600,300], Type 2 (Zoom), Antialiasing 2 (High) |
| `714-fx-ccradial-type2-30-60-on-M.aep` | `effects.aep` | `714-fx-ccradial-type2-30-60-on-M` | CC Radial Blur: Type 2, Amount 30, Quality 60, Center [400,200] |
| `715-fx-venetian-40-30-50-5-on-M.aep` | `effects.aep` | `715-fx-venetian-40-30-50-5-on-M` | Venetian Blinds: Completion 40, Direction 30, Width 50, Feather 5 |
| `716-fx-channel-blur-5-10-15-20-edge-horiz-on-M.aep` | `effects.aep` | `716-fx-channel-blur-5-10-15-20-edge-horiz-on-M` | Channel Blur: R 5, G 10, B 15, A 20, Edge Behavior (Repeat Edge Pixels) 1, Blur Dimensions 2 |
| `717-fx-blackwhite-set-tint-on-L.aep` | `effects.aep` | `717-fx-blackwhite-set-tint-on-L` | Black & White on L: Reds 10, Yellows 70, Greens 30, Cyans 50, Blues 90, Magentas 20, Tint 1, Tint Color [1,0.5,0] |
| `718-fx-tritone-set-blend25-on-L.aep` | `effects.aep` | `718-fx-tritone-set-blend25-on-L` | Tritone on L: Highlights [1,1,0.8], Midtones [0.6,0.2,0.2], Shadows [0,0,0.3], Blend With Original 25 |
| `719-fx-lumakey-darker-140-20-1-2.5-on-M.aep` | `effects.aep` | `719-fx-lumakey-darker-140-20-1-2.5-on-M` | Luma Key on M: Key Type 2, Threshold 140, Tolerance 20, Edge Thin 1, Edge Feather 2.5 |
| `720-fx-simplechoker-3.5-on-M.aep` | `effects.aep` | `720-fx-simplechoker-3.5-on-M` | Simple Choker: Choke Matte 3.5 |
| `721-fx-simplechoker-3.5-view-matte-on-M.aep` | `effects.aep` | `721-fx-simplechoker-3.5-view-matte-on-M` | 720 + View 2 |
| `722-fx-mattechoker-set-on-M.aep` | `effects.aep` | `722-fx-mattechoker-set-on-M` | Matte Choker: Geometric Softness 1 6, Choke 1 50, Geometric Softness 2 2, Choke 2 -10, Iterations 3 (Gray Level Softness 1 / 2 refused by AE: "Unable to call setValue", left at their defaults) |
| `723-fx-transform-set-on-M.aep` | `effects.aep` | `723-fx-transform-set-on-M` | Transform: Anchor [400,200], Position [500,300], Uniform Scale 0, Scale Height 80, Scale Width 120, Skew 10, Skew Axis 20, Rotation 30, Opacity 70, Use Composition's Shutter Angle 0, Shutter Angle 90, Sampling 2 |
| `725-text-point-HHHH-normal-ref-S18.aep` | `text.aep` | `725-text-point-HHHH-normal-ref-S18` | (fresh base) point text `T` `HHHH`, baseline normal: reference of S18 (= 538) |
| `727-text-reset-paragraph.aep` | `text.aep` | `727-text-reset-paragraph` | (726 undone) text.reset_style paragraph: ONLY paragraph /34 0 -> 7. Same session, not saved: a 600x400 box text of 88 characters, justified (fullLeft), lays out identically before and after a paragraph reset (text.measure: 4 lines, the same breaks and baselines -130.50 / -32.50 / 65.50 / 163.50; layer.bounds identical) |
| `728-textanim-character-offset-3.aep` | `text.aep` | `728-textanim-character-offset-3` | 727 + text.add_animator `Off` with Character Offset 3; AE adds Character Alignment (ADBE Text Character Change Type) 1 and Character Range 1 with it (read back); setting Character Range to 2 is refused by AE ("Can not set value") |
| `730-key-anchor-ease-on-400x800-solid.aep` | `keyframes.aep` | `730-key-anchor-ease-on-400x800-solid` | NEW blue solid `P` 400x800, Anchor Point keys [200,400] at 0 s (out speed 50, influence 40) and [300,500] at 2 s (in speed 30, influence 60); stored speeds 0.0625 / 0.0375 = 50 / 800 and 30 / 800 (597: 50 / 1080 on the 1920x1080 L), so a stored spatial speed is pixels per second divided by the layer height; read back 0.5 s [225.2343, 425.2343], 1 s [261.4341, 461.4341], 1.5 s [286.0550, 486.0550] |
| `731-shape-merge-offset-repeater-defaults.aep` | `shapes.aep` | `731-shape-merge-offset-repeater-defaults` | NEW shape layer `S`: Group 1 with Rectangle 400x200, then property.add of Merge Paths, Offset Paths, Repeater, nothing set; read back: Merge Type 1; Offset Amount 10, Line Join 1, Miter Limit 4, Copies 1, Copy Offset 1; Repeater Copies 1, Offset 0, Composite 1, Anchor [0,0], Position [0,0], Scale [100,100], Rotation 0, Start / End Opacity 100 / 100 (the values of a property without a record; the UI's Add menu and shape.add_repeater store their own) |
| `732-text-box-byteedit-para24-0.aep` | `text.aep` | `732-text-box-byteedit-para24-0` | 517 with paragraph `/23 true /24 1` -> `/24 0` (the run's paragraph); layout identical to 517 (4 lines, baselines -163.80 / -65.80 / 32.20 / 130.20, bounds 563.61 x 347.06); 732r-resaved keeps `/24 0` |
| `733-text-box-byteedit-para35-0.aep` | `text.aep` | `733-text-box-byteedit-para35-0` | 517 with paragraph `/34 0 /35 1` -> `/35 0`; layout identical (line ends within 1e-4 px); 733r-resaved keeps `/35 0` |
| `734-text-box-byteedit-style68-1.aep` | `styles.aep` | `734-text-box-byteedit-style68-1` | 517 with style `/68 0 /69 0` -> `/68 1` (3 occurrences: style sheet and runs); layout identical; 734r-resaved keeps `/68 1` |
| `828-shape-offset-paths-20-copies3-copyoffset1.5-stroke.aep` | `shapes.aep` | `828-shape-offset-paths-20-copies3-copyoffset1.5-stroke` | shape `S`: Rectangle 400x200, Offset Paths Amount 20, Stroke 2 px green: Copies 1 / Copy Offset 0 -> ring at the path, Copies 3 / Copy Offset 0 -> offsets 0, 20, 40, Copies 3 / Copy Offset 1.5 -> 30, 50, 70: offset of copy i = Amount x (i + Copy Offset) |
| `851-shape-key-trim-end-0-100-linear.aep` | `shapes.aep` | `851-shape-key-trim-end-0-100-linear` | Trim End keys 0 at 0 s, 100 at 2 s, linear (keyframe.apply); undone |
| `852-shape-key-trim-end-bezier-out0-60-in20-30.aep` | `shapes.aep` | `852-shape-key-trim-end-bezier-out0-60-in20-30` | Trim End keys 0 at 0 s (bezier, out speed 0 / influence 60) and 100 at 2 s (bezier, in speed 20 / influence 30); read back in ease of key 1 0 / 33, out of key 2 0 / 33; undone |
| `853-shape-key-group-position-0-200-100-linear.aep` | `shapes.aep` | `853-shape-key-group-position-0-200-100-linear` | Group 1 Transform Position keys [0,0] at 0 s, [200,100] at 2 s, linear; undone |
| `854-shape-key-fill-color-red-blue-linear.aep` | `shapes.aep` | `854-shape-key-fill-color-red-blue-linear` | Fill Color keys red at 0 s, blue at 2 s, linear; undone |
| `855-shape-key-trim-offset-0-90-hold.aep` | `shapes.aep` | `855-shape-key-trim-offset-0-90-hold` | Trim Offset keys 0 at 0 s (linear), 90 at 2 s (hold); undone |
| `856-shape-trim-end-expression-time50.aep` | `shapes.aep` | `856-shape-trim-end-expression-time50` | Trim End expression `time*50` (no keys); undone |
| `857-shape-key-group-scale-100-50-150-linear.aep` | `shapes.aep` | `857-shape-key-group-scale-100-50-150-linear` | Group 1 Transform Scale keys [100,100] at 0 s, [50,150] at 2 s, linear; undone (base state verified by layer.info) |
| `900-mask-added-no-path.aep` | `masks.aep` | `900-mask-added-no-path` | property.add `ADBE Mask Atom` on L, nothing set: the file stores the atom WITHOUT an `ADBE Mask Shape` entry (only `ADBE Group End`); read back (also after reopening) Mask Path closed [[0,0],[0,1080],[1920,1080],[1920,0]], tangents 0 = the layer's source rectangle |
| `901-mask-no-path-expansion-m100.aep` | `masks.aep` | `901-mask-no-path-expansion-m100` | from 900: Mask Expansion -100 (only `ADBE Mask Offset` stored) |
| `902-settings-rec709-16bpc-steps-png-grey-solid.aep` | `settings-c.aep` | `902-settings-rec709-16bpc-steps-png-grey-solid` | from 901: mask removed, project 16 bpc + working space Rec.709 Gamma 2.4; footage `test-media\steps.png` (sRGB PNG, no profile; top bands 0,16,32,64,96,128,160,192,224,255 grey, bottom (v, v/2, 255-v)) as layer `S`, grey 0.5 solid `G` 400x200 at [1600,900] on top |
| `905-fx-24-effects-set-one-comp.aep` | `effects.aep` | `905-fx-24-effects-set-one-comp` | the 24 effects and values of 904 in ONE comp `C` (L deleted): `R` = grey 0.5 solid with Gradient Ramp, video off, at the bottom (id 30); 24 layers E01..E24 = steps.png, each with the one effect of the 904 comp of the same name (E24 on top); the layer parameters of E22-E24 name R. Made as a smaller fixture than 904 (0.6 MB against 2 MB); no |
| `906-blendmodes-13-solids-on-steps.aep` | `layers-comps.aep` | `906-blendmodes-13-solids-on-steps` | L deleted; `S` = steps.png; 13 solids 300x300 [0.9,0.5,0.2] (named by mode) at a 320 x 360 px grid, each with layer.set_blend_mode normal, add, multiply, screen, overlay, softLight, hardLight, difference, exclusion, colorDodge, colorBurn, darken, lighten: `ldta`@99 = 2, 4, 5, 6, 7, 8, 9, 26, 25, 27, 28, 10, 11 |
| `951-styles-all-nine-defaults-on-B.aep` | `styles.aep` | `951-styles-all-nine-defaults-on-B` | from 950: on B `command.execute` 9000, 9001, 9002, 9003, 9004, 9005, 9006, 9007, 9008 (one call each), nothing set. AE read-back of the defaults (property.get): Drop Shadow mode2 5, color [0,0,0,1], opacity 75, useGlobalAngle 0, localLightingAngle 120, distance 5, chokeMatte 0, blur 5, noise 0, layerConceals 1; Inner Shadow mode2 5, color [0,0,0,1], opacity 75, useGlobalAngle 0, angle 120, distance 5, chokeMatte 0, blur 5, noise 0; Outer Glow mode2 11, opacity 75, noise 0, AEColorChoice 1, color [1,1,0.74509803921569,1], gradientSmoothness 100, glowTechnique 1, chokeMatte 0, blur 5, inputRange 50, shadingNoise 0; Inner Glow as Outer Glow plus innerGlowSource 1; Bevel and Emboss bevelStyle 2, bevelTechnique 1, strengthRatio 100, bevelDirection 1, blur 5, softness 0, useGlobalAngle 0, angle 120, altitude 30, highlightMode 11, highlightColor [1,1,1,1], highlightOpacity 75, shadowMode 5, shadowColor [0,0,0,1], shadowOpacity 75; Satin (chromeFX) mode2 5, color [0,0,0,1], opacity 50, angle 19, distance 11, blur 14, invert 1; Color Overlay (solidFill) mode2 1, color [1,0,0,1], opacity 100; Gradient Overlay mode2 1, opacity 100, gradientSmoothness 100, angle 90, type 1, reverse 0, align 1, scale 100, offset [0,0] (gradient not read); Stroke (frameFX) mode2 1, color [1,0,0,1], size 3, opacity 100, style 1; Blending Options Global Light Angle 120, Altitude 30, Fill Opacity 100, R/G/B Channel Blend 1, Blend Interior 0, Blend Ranges 1 |
| `963-curves-red-mid-down-on-S.aep` | `styles.aep` | `963-curves-red-mid-down-on-S` | from 984: apply `curves-red-mid-down.ffx` (Red curve: middle point dragged down). AE read-back: Channel: 1 (RGB, default; the editor channel Red is not the Channel parameter), Curves not readable. File: Red curve (0,0) (128,44) (255,255) |
| `964-curves-rgb-s-curve-on-S.aep` | `styles.aep` | `964-curves-rgb-s-curve-on-S` | from 984: apply `curves-rgb-two-points.ffx` (RGB S-curve: a point at about 1/4 dragged down, one at 3/4 up). AE read-back: Channel: 1, Curves not readable. File: RGB curve (0,0) (62,24) (194,230) (255,255) |
| `966-style-drop-shadow-opacity50-distance20.aep` | `styles.aep` | `966-style-drop-shadow-opacity50-distance20` | from 952 (opened): dropShadow/opacity 75 -> 50, dropShadow/distance 5 -> 20 (read back 50, 20) |
| `970-style-stroke-size10-color-green.aep` | `styles.aep` | `970-style-stroke-size10-color-green` | from 960: frameFX/size 3 -> 10, frameFX/color [1,0,0,1] -> [0,1,0,1] (read back 10, [0,1,0,1]) |
| `974-style-gradient-overlay-angle45-scale150.aep` | `styles.aep` | `974-style-gradient-overlay-angle45-scale150` | from 959: gradientFill/angle 90 -> 45, gradientFill/scale 100 -> 150 (read back 45, 150); the gradient colours were not touched |
| `975-style-fill-opacity0-with-drop-shadow.aep` | `styles.aep` | `975-style-fill-opacity0-with-drop-shadow` | from 952: ADBE Layer Fill Opacity2 100 -> 0 (Layer Styles > Blending Options > Advanced Blending > Fill Opacity; read back 0) |
| `976-style-drop-shadow-opacity-keyed-100-20-linear.aep` | `styles.aep` | `976-style-drop-shadow-opacity-keyed-100-20-linear` | from 952: dropShadow/opacity keys t=0 -> 100, t=2 -> 20, linear (keyframe.apply); read back 100 / 60 / 20 at 0 / 1 / 2 s |
| `977-styles-all-params-changed.aep` | `styles.aep` | `977-styles-all-params-changed` | on B every settable parameter of the nine styles and Blending Options set by `property.set` (one batch): Global Angle2 45, Global Altitude2 40, Fill Opacity2 80, R Channel Blend 0, Blend Interior 1, Blend Ranges 0; dropShadow mode2 1, color [0,0,1], opacity 60, useGlobalAngle 1, localLightingAngle 30, distance 12, chokeMatte 10, blur 8, noise 5, layerConceals 0; innerShadow mode2 1, color [0,1,0], opacity 55, useGlobalAngle 1, localLightingAngle 60, distance 7, chokeMatte 15, blur 9, noise 3; outerGlow mode2 1, opacity 65, noise 2, AEColorChoice 2, color [1,0,0], gradientSmoothness 80, glowTechnique 2, chokeMatte 12, blur 20, inputRange 40, shadingNoise 10; innerGlow mode2 1, opacity 45, noise 4, AEColorChoice 2, color [0,1,0], gradientSmoothness 70, glowTechnique 2, innerGlowSource 2, chokeMatte 8, blur 15, inputRange 30, shadingNoise 20; bevelEmboss bevelStyle 1, bevelTechnique 2, strengthRatio 150, bevelDirection 2, blur 10, softness 3, useGlobalAngle 1, localLightingAngle 70, localLightingAltitude 50, highlightMode 1, highlightColor [1,1,0], highlightOpacity 60, shadowMode 1, shadowColor [0,0,1], shadowOpacity 40; chromeFX mode2 1, color [1,0,1], opacity 70, localLightingAngle 45, distance 20, blur 10, invert 0; solidFill mode2 5, color [0,1,1], opacity 50; gradientFill mode2 5, opacity 70, gradientSmoothness 50, angle 30, type 2, reverse 1, align 0, scale 120, offset [10,20]; frameFX mode2 5, color [0,0,1], size 6, opacity 80, style 2. (Values equal to the default, e.g. dropShadow mode2 1 is not: default 5.) Purpose: one record of every parameter type. property.list read the exact match names (inner shadow / satin angle = `localLightingAngle`; Blending Options = `ADBE Global Angle2`, `ADBE Global Altitude2`, Adv Blend `ADBE Layer Fill Opacity2`, `ADBE R/G/B Channel Blend`, `ADBE Blend Interior`, `ADBE Blend Ranges`; no Knockout) |
| `979-style-inner-glow-source-2.aep` | `styles.aep` | `979-style-inner-glow-source-2` | innerGlow/innerGlowSource 1 -> 2; 955 centre unchanged 51,102,204) -> 1 = Edge, 2 = Center |
| `981-layer-stretch-minus100-start5.aep` | `keyframes.aep` | `981-layer-stretch-minus100-start5` | L: layer.set_timing stretch -100, then startTime 5; AE reads startTime 5, inPoint 4.99967, outPoint -0.00033, stretch -100 |
| `982-layer-stretch-minus100-opacity-keys.aep` | `keyframes.aep` | `982-layer-stretch-minus100-opacity-keys` | from 981 (not undone): Opacity keys added at comp time 0 -> 100 and comp 4 -> 20 (keyframe.add); AE lists the keys at 4.00002 (20) and 0.00002 (100), reads 80 at comp 1 s, 40 at 3 s |
| `983-layer-stretch-minus50-opacity-keys.aep` | `keyframes.aep` | `983-layer-stretch-minus50-opacity-keys` | from 982: stretch -100 -> -50 (keys kept); AE: inPoint 4.99983, outPoint 2.49983, keys at comp 4.5 (20) and 2.5 (100) |
| `985-meshwarp-3x3-vertex-moved-on-S.aep` | `styles.aep` | `985-meshwarp-3x3-vertex-moved-on-S` | from 984: apply `meshwarp-3x3-center-right.ffx` (Rows 3, Columns 3 = 3x3 cells, 4x4 vertices; the contributor dragged the LOWER-RIGHT INNER vertex (row 2, column 2, 0-based) to the right, its tangents moved with it). AE read-back: Rows 3, Columns 3, Quality 8, Distortion Mesh not readable. File: that vertex at (0.73542, 0.66481) of the layer = +132 px, -2 px; the instance mesh is byte-identical to the one in the user's `presets\presets-reference.aep` |
| `986-shape-fill-switched-off-green.aep` | `shapes.aep` | `986-shape-fill-switched-off-green` | from 850: the Fill group's `tdsb` 1 -> 0 patched in a copy (the item eye switch is not scriptable), opened with project.open, Fill Color set to green by property.set, saved by After Effects (it keeps `tdsb` 0); centre 255,0,0 = the solid L) |
| `988-style-drop-shadow-switched-off.aep` | `styles.aep` | `988-style-drop-shadow-switched-off` | from 952: the `dropShadow/enabled` group `tdsb` 1 -> 0 patched in a copy (the style eye is not scriptable), opened with project.open (AE lists the style, Opacity 75),; Layer Styles and Blending Options stay 1) |
