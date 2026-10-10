# Effects

<!-- Generated from the effect registry (crates/effects/src/catalog.rs); do not edit by hand.
     Regenerate: UPDATE_DOCS=1 cargo test -p effectcraft-effects --lib effects_doc_is_current -->

EffectCraft ships 307 effects with 3021 parameters in total, grouped into the same categories as the Effects & Presets panel. Every effect is our own implementation, written from public behaviour descriptions and standard image-processing literature. Parameter names, order, popup options, units, defaults and ranges follow the reference application so that projects, expressions and muscle memory carry over.

- **GPU**: 280 effects also run on the GPU compositor with identical results.
- **32**: 307 effects process 32-bit float (HDR, overbright) pixels without clamping.
- **Status**: 307 implemented in full, 0 partial (what is missing is listed).

Effects are addressed by id (`ec.<category>.<name>`) or display name in commands, scripts and the control channel, and every parameter by its id (`effects/#1/blurriness`).

## 3D Channel

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| 3D Channel Extract | `ec.3d.channelextract` | 6 | GPU | 32 | Implemented |
| Cryptomatte | `ec.3d.cryptomatte` | 4 | GPU | 32 | Implemented |
| Depth Matte | `ec.3d.depthmatte` | 3 | GPU | 32 | Implemented |
| Depth of Field | `ec.3d.depthoffield` | 4 | GPU | 32 | Implemented |
| EXtractoR | `ec.3d.extractor` | 8 | GPU | 32 | Implemented |
| Fog 3D | `ec.3d.fog3d` | 8 | GPU | 32 | Implemented |
| ID Matte | `ec.3d.idmatte` | 5 | GPU | 32 | Implemented |
| IDentifier | `ec.3d.identifier` | 3 | GPU | 32 | Implemented |

## Audio

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Backwards | `ec.audio.backwards` | 1 |  | 32 | Implemented |
| Bass & Treble | `ec.audio.basstreble` | 2 |  | 32 | Implemented |
| Compressor | `ec.audio.compressor` | 8 |  | 32 | Implemented |
| Delay | `ec.audio.delay` | 5 |  | 32 | Implemented |
| Distortion | `ec.audio.distortion` | 7 |  | 32 | Implemented |
| Flange & Chorus | `ec.audio.flangechorus` | 9 |  | 32 | Implemented |
| Gate | `ec.audio.gate` | 4 |  | 32 | Implemented |
| High-Low Pass | `ec.audio.highlowpass` | 4 |  | 32 | Implemented |
| Modulator | `ec.audio.modulator` | 4 |  | 32 | Implemented |
| Parametric EQ | `ec.audio.parametriceq` | 12 |  | 32 | Implemented |
| Reverb | `ec.audio.reverb` | 6 |  | 32 | Implemented |
| Stereo Mixer | `ec.audio.stereomixer` | 5 |  | 32 | Implemented |
| Tone | `ec.audio.tone` | 7 |  | 32 | Implemented |

## Blur & Sharpen

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Bilateral Blur | `ec.blur.bilateral` | 3 | GPU | 32 | Implemented |
| CC Cross Blur | `ec.blur.cccross` | 3 | GPU | 32 | Implemented |
| CC Radial Blur | `ec.blur.ccradial` | 4 | GPU | 32 | Implemented |
| CC Radial Fast Blur | `ec.blur.ccradialfast` | 3 | GPU | 32 | Implemented |
| CC Vector Blur | `ec.blur.ccvector` | 4 | GPU | 32 | Implemented |
| Camera Lens Blur | `ec.blur.cameralens` | 16 | GPU | 32 | Implemented |
| Camera-Shake Deblur | `ec.blur.camerashakedeblur` | 8 |  | 32 | Implemented |
| Channel Blur | `ec.blur.channel` | 6 | GPU | 32 | Implemented |
| Compound Blur | `ec.blur.compound` | 4 | GPU | 32 | Implemented |
| Directional Blur | `ec.blur.directional` | 2 | GPU | 32 | Implemented |
| Fast Box Blur | `ec.blur.fastbox` | 4 | GPU | 32 | Implemented |
| Gaussian Blur | `ec.blur.gaussian` | 3 | GPU | 32 | Implemented |
| Radial Blur | `ec.blur.radial` | 4 | GPU | 32 | Implemented |
| Sharpen | `ec.blur.sharpen` | 1 | GPU | 32 | Implemented |
| Smart Blur | `ec.blur.smart` | 3 | GPU | 32 | Implemented |
| Unsharp Mask | `ec.blur.unsharp` | 3 | GPU | 32 | Implemented |

## Channel

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Arithmetic | `ec.channel.arithmetic` | 5 | GPU | 32 | Implemented |
| Blend | `ec.channel.blend` | 4 | GPU | 32 | Implemented |
| CC Composite | `ec.channel.cccomposite` | 3 | GPU | 32 | Implemented |
| Calculations | `ec.channel.calculations` | 9 | GPU | 32 | Implemented |
| Channel Combiner | `ec.channel.combiner` | 6 | GPU | 32 | Implemented |
| Compound Arithmetic | `ec.channel.compoundarithmetic` | 6 | GPU | 32 | Implemented |
| Invert | `ec.channel.invert` | 2 | GPU | 32 | Implemented |
| Minimax | `ec.channel.minimax` | 5 | GPU | 32 | Implemented |
| Remove Color Matting | `ec.channel.removecolormatting` | 2 | GPU | 32 | Implemented |
| Set Channels | `ec.channel.setchannels` | 9 | GPU | 32 | Implemented |
| Set Matte | `ec.channel.setmatte` | 6 | GPU | 32 | Implemented |
| Shift Channels | `ec.channel.shiftchannels` | 4 | GPU | 32 | Implemented |
| Solid Composite | `ec.channel.solidcomposite` | 4 | GPU | 32 | Implemented |

## Color Correction

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Auto Color | `ec.color.autocolor` | 6 | GPU | 32 | Implemented |
| Auto Contrast | `ec.color.autocontrast` | 5 | GPU | 32 | Implemented |
| Auto Levels | `ec.color.autolevels` | 5 | GPU | 32 | Implemented |
| Black & White | `ec.color.blackwhite` | 8 | GPU | 32 | Implemented |
| Brightness & Contrast | `ec.color.brightnesscontrast` | 3 | GPU | 32 | Implemented |
| Broadcast Colors | `ec.color.broadcast` | 3 | GPU | 32 | Implemented |
| CC Color Neutralizer | `ec.color.cccolorneutralizer` | 11 | GPU | 32 | Implemented |
| CC Color Offset | `ec.color.cccoloroffset` | 4 | GPU | 32 | Implemented |
| CC Kernel | `ec.color.cckernel` | 12 | GPU | 32 | Implemented |
| CC Toner | `ec.color.cctoner` | 7 | GPU | 32 | Implemented |
| Change Color | `ec.color.changecolor` | 9 | GPU | 32 | Implemented |
| Change to Color | `ec.color.changetocolor` | 9 | GPU | 32 | Implemented |
| Channel Mixer | `ec.color.channelmixer` | 13 | GPU | 32 | Implemented |
| Color Balance | `ec.color.colorbalance` | 10 | GPU | 32 | Implemented |
| Color Balance (HLS) | `ec.color.colorbalancehls` | 3 | GPU | 32 | Implemented |
| Color Link | `ec.color.colorlink` | 6 | GPU | 32 | Implemented |
| Color Stabilizer | `ec.color.colorstabilizer` | 6 | GPU | 32 | Implemented |
| Colorama | `ec.color.colorama` | 20 | GPU | 32 | Implemented |
| Curves | `ec.color.curves` | 0 | GPU | 32 | Implemented |
| Equalize | `ec.color.equalize` | 2 | GPU | 32 | Implemented |
| Exposure | `ec.color.exposure` | 14 | GPU | 32 | Implemented |
| Gamma/Pedestal/Gain | `ec.color.gammapedestalgain` | 10 | GPU | 32 | Implemented |
| Hue/Saturation | `ec.color.huesaturation` | 50 | GPU | 32 | Implemented |
| Leave Color | `ec.color.leavecolor` | 5 | GPU | 32 | Implemented |
| Levels | `ec.color.levels` | 36 | GPU | 32 | Implemented |
| Levels (Individual Controls) | `ec.color.levelsic` | 27 | GPU | 32 | Implemented |
| Lumetri Color | `ec.color.lumetri` | 66 | GPU | 32 | Implemented |
| OCIO CDL Transform | `ec.color.ociocdl` | 12 | GPU | 32 | Implemented |
| OCIO Color Space Transform | `ec.color.ociocolorspace` | 7 | GPU | 32 | Implemented |
| OCIO Display Transform | `ec.color.ociodisplay` | 9 | GPU | 32 | Implemented |
| OCIO File Transform | `ec.color.ociofile` | 4 | GPU | 32 | Implemented |
| OCIO Look Transform | `ec.color.ociolook` | 6 | GPU | 32 | Implemented |
| PS Arbitrary Map | `ec.color.psarbitrarymap` | 2 | GPU | 32 | Implemented |
| Photo Filter | `ec.color.photofilter` | 4 | GPU | 32 | Implemented |
| Selective Color | `ec.color.selectivecolor` | 38 | GPU | 32 | Implemented |
| Shadow/Highlight | `ec.color.shadowhighlight` | 14 | GPU | 32 | Implemented |
| Tint | `ec.color.tint` | 3 | GPU | 32 | Implemented |
| Tritone | `ec.color.tritone` | 4 | GPU | 32 | Implemented |
| Vibrance | `ec.color.vibrance` | 2 | GPU | 32 | Implemented |
| Video Limiter | `ec.color.videolimiter` | 5 | GPU | 32 | Implemented |

## Distort

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Bezier Warp | `ec.distort.bezierwarp` | 13 | GPU | 32 | Implemented |
| Bulge | `ec.distort.bulge` | 6 | GPU | 32 | Implemented |
| CC Bend It | `ec.distort.ccbendit` | 4 | GPU | 32 | Implemented |
| CC Bender | `ec.distort.ccbender` | 5 | GPU | 32 | Implemented |
| CC Blobbylize | `ec.distort.ccblobbylize` | 15 | GPU | 32 | Implemented |
| CC Flo Motion | `ec.distort.ccflomotion` | 7 | GPU | 32 | Implemented |
| CC Griddler | `ec.distort.ccgriddler` | 5 | GPU | 32 | Implemented |
| CC Lens | `ec.distort.cclens` | 3 | GPU | 32 | Implemented |
| CC Page Turn | `ec.distort.ccpageturn` | 7 | GPU | 32 | Implemented |
| CC Power Pin | `ec.distort.ccpowerpin` | 10 | GPU | 32 | Implemented |
| CC Ripple Pulse | `ec.distort.ccripplepulse` | 5 | GPU | 32 | Implemented |
| CC Slant | `ec.distort.ccslant` | 4 | GPU | 32 | Implemented |
| CC Smear | `ec.distort.ccsmear` | 4 | GPU | 32 | Implemented |
| CC Split | `ec.distort.ccsplit` | 3 | GPU | 32 | Implemented |
| CC Split 2 | `ec.distort.ccsplit2` | 4 | GPU | 32 | Implemented |
| CC Tiler | `ec.distort.cctiler` | 3 | GPU | 32 | Implemented |
| Corner Pin | `ec.distort.cornerpin` | 4 | GPU | 32 | Implemented |
| Detail-preserving Upscale | `ec.distort.upscale` | 4 |  | 32 | Implemented |
| Displacement Map | `ec.distort.displacementmap` | 8 | GPU | 32 | Implemented |
| Liquify | `ec.distort.liquify` | 12 | GPU | 32 | Implemented |
| Magnify | `ec.distort.magnify` | 10 | GPU | 32 | Implemented |
| Mesh Warp | `ec.distort.meshwarp` | 3 | GPU | 32 | Implemented |
| Mirror | `ec.distort.mirror` | 2 | GPU | 32 | Implemented |
| Motion Transform | `ec.distort.motiontransformblur` | 7 | GPU | 32 | Implemented |
| Offset | `ec.distort.offset` | 2 | GPU | 32 | Implemented |
| Optics Compensation | `ec.distort.opticscompensation` | 6 | GPU | 32 | Implemented |
| Polar Coordinates | `ec.distort.polar` | 2 | GPU | 32 | Implemented |
| Puppet | `ec.distort.puppet` | 1 |  | 32 | Implemented |
| Reshape | `ec.distort.reshape` | 6 | GPU | 32 | Implemented |
| Ripple | `ec.distort.ripple` | 7 | GPU | 32 | Implemented |
| Rolling Shutter Repair | `ec.distort.rollingshutterrepair` | 5 |  | 32 | Implemented |
| Smear | `ec.distort.smear` | 8 | GPU | 32 | Implemented |
| Spherize | `ec.distort.spherize` | 2 | GPU | 32 | Implemented |
| Transform | `ec.distort.transform` | 12 | GPU | 32 | Implemented |
| Turbulent Displace | `ec.distort.turbulentdisplace` | 11 | GPU | 32 | Implemented |
| Twirl | `ec.distort.twirl` | 3 | GPU | 32 | Implemented |
| Twirl (Legacy) | `ec.distort.twirllegacy` | 3 | GPU | 32 | Implemented |
| Warp | `ec.distort.warp` | 5 | GPU | 32 | Implemented |
| Warp Stabilizer | `ec.distort.warpstabilizer` | 20 |  | 32 | Implemented |
| Wave Warp | `ec.distort.wavewarp` | 7 | GPU | 32 | Implemented |

## Expression Controls

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| 3D Point Control | `ec.control.point3d` | 1 | GPU | 32 | Implemented |
| Angle Control | `ec.control.angle` | 1 | GPU | 32 | Implemented |
| Checkbox Control | `ec.control.checkbox` | 1 | GPU | 32 | Implemented |
| Color Control | `ec.control.color` | 1 | GPU | 32 | Implemented |
| Dropdown Menu Control | `ec.control.dropdown` | 1 | GPU | 32 | Implemented |
| Layer Control | `ec.control.layer` | 1 | GPU | 32 | Implemented |
| Point Control | `ec.control.point` | 1 | GPU | 32 | Implemented |
| Slider Control | `ec.control.slider` | 1 | GPU | 32 | Implemented |
| Stereo 3D Controls | `ec.control.stereo3d` | 5 | GPU | 32 | Implemented |

## Generate

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| 4-Color Gradient | `ec.generate.fourcolor` | 12 | GPU | 32 | Implemented |
| Advanced Lightning | `ec.generate.advancedlightning` | 24 | GPU | 32 | Implemented |
| Audio Spectrum | `ec.generate.audiospectrum` | 23 | GPU | 32 | Implemented |
| Audio Waveform | `ec.generate.audiowaveform` | 17 | GPU | 32 | Implemented |
| Beam | `ec.generate.beam` | 11 | GPU | 32 | Implemented |
| CC Glue Gun | `ec.generate.ccgluegun` | 7 | GPU | 32 | Implemented |
| CC Light Burst 2.5 | `ec.generate.cclightburst` | 6 | GPU | 32 | Implemented |
| CC Light Rays | `ec.generate.cclightrays` | 8 | GPU | 32 | Implemented |
| CC Light Sweep | `ec.generate.cclightsweep` | 9 | GPU | 32 | Implemented |
| CC Threads | `ec.generate.ccthreads` | 8 | GPU | 32 | Implemented |
| Cell Pattern | `ec.generate.cellpattern` | 14 | GPU | 32 | Implemented |
| Checkerboard | `ec.generate.checkerboard` | 10 | GPU | 32 | Implemented |
| Circle | `ec.generate.circle` | 11 | GPU | 32 | Implemented |
| Ellipse | `ec.generate.ellipse` | 8 | GPU | 32 | Implemented |
| Eyedropper Fill | `ec.generate.eyedropperfill` | 5 | GPU | 32 | Implemented |
| Fill | `ec.generate.fill` | 7 | GPU | 32 | Implemented |
| Fractal | `ec.generate.fractal` | 21 |  | 32 | Implemented |
| Gradient Ramp | `ec.generate.gradientramp` | 7 | GPU | 32 | Implemented |
| Grid | `ec.generate.grid` | 12 | GPU | 32 | Implemented |
| Lens Flare | `ec.generate.lensflare` | 4 | GPU | 32 | Implemented |
| Paint Bucket | `ec.generate.paintbucket` | 11 | GPU | 32 | Implemented |
| Radio Waves | `ec.generate.radiowaves` | 31 | GPU | 32 | Implemented |
| Scribble | `ec.generate.scribble` | 25 | GPU | 32 | Implemented |
| Stroke | `ec.generate.stroke` | 11 | GPU | 32 | Implemented |
| Vegas | `ec.generate.vegas` | 26 | GPU | 32 | Implemented |
| Write-on | `ec.generate.writeon` | 10 | GPU | 32 | Implemented |

## Immersive Video

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| VR Blur | `ec.vr.blur` | 2 | GPU | 32 | Implemented |
| VR Chromatic Aberrations | `ec.vr.chromaticaberrations` | 8 | GPU | 32 | Implemented |
| VR Color Gradients | `ec.vr.colorgradients` | 19 | GPU | 32 | Implemented |
| VR Converter | `ec.vr.converter` | 8 | GPU | 32 | Implemented |
| VR De-Noise | `ec.vr.denoise` | 4 | GPU | 32 | Implemented |
| VR Digital Glitch | `ec.vr.digitalglitch` | 21 | GPU | 32 | Implemented |
| VR Fractal Noise | `ec.vr.fractalnoise` | 16 | GPU | 32 | Implemented |
| VR Glow | `ec.vr.glow` | 7 | GPU | 32 | Implemented |
| VR Plane to Sphere | `ec.vr.planetosphere` | 6 | GPU | 32 | Implemented |
| VR Rotate Sphere | `ec.vr.rotatesphere` | 5 | GPU | 32 | Implemented |
| VR Sharpen | `ec.vr.sharpen` | 2 | GPU | 32 | Implemented |
| VR Sphere to Plane | `ec.vr.spheretoplane` | 5 | GPU | 32 | Implemented |

## Keying

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Advanced Spill Suppressor | `ec.key.advancedspill` | 8 | GPU | 32 | Implemented |
| CC Simple Wire Removal | `ec.key.ccsimplewireremoval` | 7 | GPU | 32 | Implemented |
| Color Difference Key | `ec.key.colordifference` | 16 | GPU | 32 | Implemented |
| Color Range | `ec.key.colorrange` | 8 | GPU | 32 | Implemented |
| Difference Matte | `ec.key.differencematte` | 6 | GPU | 32 | Implemented |
| Extract | `ec.key.extract` | 6 | GPU | 32 | Implemented |
| Inner/Outer Key | `ec.key.innerouter` | 88 | GPU | 32 | Implemented |
| Key Cleaner | `ec.key.keycleaner` | 4 | GPU | 32 | Implemented |
| Key Light | `ec.keying.keylight` | 56 | GPU | 32 | Implemented |
| Linear Color Key | `ec.key.linearcolor` | 6 | GPU | 32 | Implemented |
| Screen Key | `ec.key.screen` | 11 | GPU | 32 | Implemented |
| Unmult | `ec.key.unmult` | 6 | GPU | 32 | Implemented |

## Matte

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Matte Choker | `ec.matte.mattechoker` | 7 | GPU | 32 | Implemented |
| Mocha shape | `ec.obsolete.mochashape` | 6 |  | 32 | Implemented |
| Refine Hard Matte | `ec.matte.refinehard` | 16 | GPU | 32 | Implemented |
| Refine Soft Matte | `ec.matte.refinesoft` | 18 | GPU | 32 | Implemented |
| Roto Brush & Refine Edge | `ec.matte.rotobrush` | 24 |  | 32 | Implemented |
| Simple Choker | `ec.matte.simplechoker` | 2 | GPU | 32 | Implemented |

## Noise & Grain

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Add Grain | `ec.noise.addgrain` | 32 | GPU | 32 | Implemented |
| Curl Noise | `ec.noise.curlnoise` | 10 | GPU | 32 | Implemented |
| Dust & Scratches | `ec.noise.dustscratches` | 3 | GPU | 32 | Implemented |
| Fractal Noise | `ec.noise.fractal` | 26 | GPU | 32 | Implemented |
| Match Grain | `ec.noise.matchgrain` | 37 |  | 32 | Implemented |
| Median | `ec.noise.median` | 2 | GPU | 32 | Implemented |
| Median (Legacy) | `ec.noise.medianlegacy` | 2 | GPU | 32 | Implemented |
| Noise | `ec.noise.noise` | 3 | GPU | 32 | Implemented |
| Noise Alpha | `ec.noise.noisealpha` | 8 | GPU | 32 | Implemented |
| Noise HLS | `ec.noise.noisehls` | 6 | GPU | 32 | Implemented |
| Noise HLS Auto | `ec.noise.noisehlsauto` | 6 | GPU | 32 | Implemented |
| Remove Grain | `ec.noise.removegrain` | 20 | GPU | 32 | Implemented |
| Turbulent Noise | `ec.noise.turbulent` | 22 | GPU | 32 | Implemented |

## Obsolete

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Basic 3D | `ec.obsolete.basic3d` | 5 | GPU | 32 | Implemented |
| Basic Text | `ec.obsolete.basictext` | 11 | GPU | 32 | Implemented |
| Color Key | `ec.key.colorkey` | 4 | GPU | 32 | Implemented |
| Gaussian Blur (Legacy) | `ec.obsolete.gaussianlegacy` | 2 | GPU | 32 | Implemented |
| Lightning | `ec.obsolete.lightning` | 25 | GPU | 32 | Implemented |
| Luma Key | `ec.key.luma` | 5 | GPU | 32 | Implemented |
| Path Text | `ec.obsolete.pathtext` | 32 | GPU | 32 | Implemented |
| Reduce Interlace Flicker | `ec.blur.reduceflicker` | 1 | GPU | 32 | Implemented |
| Spill Suppressor | `ec.key.spill` | 3 | GPU | 32 | Implemented |

## Paint

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Paint | `ec.paint.paint` | 1 |  | 32 | Implemented |

## Perspective

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| 3D Camera Tracker | `ec.perspective.cameratracker` | 12 |  | 32 | Implemented |
| 3D Glasses | `ec.perspective.3dglasses` | 8 | GPU | 32 | Implemented |
| Bevel Alpha | `ec.perspective.bevelalpha` | 4 | GPU | 32 | Implemented |
| Bevel Edges | `ec.perspective.beveledges` | 4 | GPU | 32 | Implemented |
| CC Cylinder | `ec.perspective.cccylinder` | 12 | GPU | 32 | Implemented |
| CC Environment | `ec.perspective.ccenvironment` | 4 | GPU | 32 | Implemented |
| CC Sphere | `ec.perspective.ccsphere` | 14 | GPU | 32 | Implemented |
| CC Spotlight | `ec.perspective.ccspotlight` | 8 | GPU | 32 | Implemented |
| Drop Shadow | `ec.perspective.dropshadow` | 6 | GPU | 32 | Implemented |
| Radial Shadow | `ec.perspective.radialshadow` | 9 | GPU | 32 | Implemented |

## Simulation

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| CC Ball Action | `ec.sim.ccballaction` | 8 | GPU | 32 | Implemented |
| CC Bubbles | `ec.sim.ccbubbles` | 7 | GPU | 32 | Implemented |
| CC Drizzle | `ec.sim.ccdrizzle` | 14 | GPU | 32 | Implemented |
| CC Hair | `ec.sim.cchair` | 20 | GPU | 32 | Implemented |
| CC Mr. Mercury | `ec.sim.ccmrmercury` | 23 | GPU | 32 | Implemented |
| CC Particle Systems II | `ec.sim.ccparticlesystems2` | 21 | GPU | 32 | Implemented |
| CC Particle World | `ec.sim.ccparticleworld` | 35 | GPU | 32 | Implemented |
| CC Pixel Polly | `ec.sim.ccpixelpolly` | 10 | GPU | 32 | Implemented |
| CC Rainfall | `ec.sim.ccrainfall` | 14 | GPU | 32 | Implemented |
| CC Scatterize | `ec.sim.ccscatterize` | 4 | GPU | 32 | Implemented |
| CC Snowfall | `ec.sim.ccsnowfall` | 19 | GPU | 32 | Implemented |
| CC Star Burst | `ec.sim.ccstarburst` | 6 | GPU | 32 | Implemented |
| Card Dance | `ec.sim.carddance` | 53 | GPU | 32 | Implemented |
| Caustics | `ec.sim.caustics` | 28 | GPU | 32 | Implemented |
| Foam | `ec.sim.foam` | 36 | GPU | 32 | Implemented |
| Particle Playground | `ec.sim.particleplayground` | 92 | GPU | 32 | Implemented |
| Shatter | `ec.sim.shatter` | 60 | GPU | 32 | Implemented |
| Wave World | `ec.sim.waveworld` | 35 | GPU | 32 | Implemented |

## Stylize

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Brush Strokes | `ec.stylize.brushstrokes` | 7 | GPU | 32 | Implemented |
| CC Block Load | `ec.stylize.ccblockload` | 5 | GPU | 32 | Implemented |
| CC Burn Film | `ec.stylize.ccburnfilm` | 3 | GPU | 32 | Implemented |
| CC Glass | `ec.stylize.ccglass` | 17 | GPU | 32 | Implemented |
| CC HexTile | `ec.stylize.cchextile` | 5 | GPU | 32 | Implemented |
| CC Kaleida | `ec.stylize.cckaleida` | 4 | GPU | 32 | Implemented |
| CC Mr. Smoothie | `ec.stylize.ccmrsmoothie` | 7 | GPU | 32 | Implemented |
| CC Plastic | `ec.stylize.ccplastic` | 18 | GPU | 32 | Implemented |
| CC RepeTile | `ec.stylize.ccrepetile` | 5 | GPU | 32 | Implemented |
| CC Threshold | `ec.stylize.ccthreshold` | 4 | GPU | 32 | Implemented |
| CC Threshold RGB | `ec.stylize.ccthresholdrgb` | 7 | GPU | 32 | Implemented |
| CC Vignette | `ec.stylize.ccvignette` | 4 | GPU | 32 | Implemented |
| Cartoon | `ec.stylize.cartoon` | 12 | GPU | 32 | Implemented |
| Color Emboss | `ec.stylize.coloremboss` | 4 | GPU | 32 | Implemented |
| Emboss | `ec.stylize.emboss` | 4 | GPU | 32 | Implemented |
| Find Edges | `ec.stylize.findedges` | 2 | GPU | 32 | Implemented |
| Glow | `ec.stylize.glow` | 14 | GPU | 32 | Implemented |
| Mosaic | `ec.stylize.mosaic` | 3 | GPU | 32 | Implemented |
| Motion Tile | `ec.stylize.motiontile` | 8 | GPU | 32 | Implemented |
| Posterize | `ec.stylize.posterize` | 1 | GPU | 32 | Implemented |
| Roughen Edges | `ec.stylize.roughenedges` | 13 | GPU | 32 | Implemented |
| Scatter | `ec.stylize.scatter` | 3 | GPU | 32 | Implemented |
| Strobe Light | `ec.stylize.strobe` | 7 | GPU | 32 | Implemented |
| Texturize | `ec.stylize.texturize` | 4 | GPU | 32 | Implemented |
| Threshold | `ec.stylize.threshold` | 1 | GPU | 32 | Implemented |

## Text

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Numbers | `ec.text.numbers` | 14 | GPU | 32 | Implemented |
| Timecode | `ec.text.timecode` | 12 | GPU | 32 | Implemented |

## Time

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| CC Force Motion Blur | `ec.time.ccforcemotionblur` | 4 | GPU | 32 | Implemented |
| CC Wide Time | `ec.time.ccwidetime` | 3 | GPU | 32 | Implemented |
| Echo | `ec.time.echo` | 5 | GPU | 32 | Implemented |
| Pixel Motion Blur | `ec.time.pixelmotionblur` | 4 | GPU | 32 | Implemented |
| Posterize Time | `ec.time.posterizetime` | 1 | GPU | 32 | Implemented |
| Time Difference | `ec.time.timedifference` | 5 | GPU | 32 | Implemented |
| Time Displacement | `ec.time.timedisplacement` | 4 | GPU | 32 | Implemented |
| Timewarp | `ec.time.timewarp` | 28 | GPU | 32 | Implemented |

## Transition

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Block Dissolve | `ec.transition.blockdissolve` | 5 | GPU | 32 | Implemented |
| CC Glass Wipe | `ec.transition.ccglasswipe` | 5 | GPU | 32 | Implemented |
| CC Grid Wipe | `ec.transition.ccgridwipe` | 7 | GPU | 32 | Implemented |
| CC Image Wipe | `ec.transition.ccimagewipe` | 7 | GPU | 32 | Implemented |
| CC Jaws | `ec.transition.ccjaws` | 6 | GPU | 32 | Implemented |
| CC Light Wipe | `ec.transition.cclightwipe` | 8 | GPU | 32 | Implemented |
| CC Line Sweep | `ec.transition.cclinesweep` | 5 | GPU | 32 | Implemented |
| CC Radial ScaleWipe | `ec.transition.ccradialscalewipe` | 3 | GPU | 32 | Implemented |
| CC Scale Wipe | `ec.transition.ccscalewipe` | 3 | GPU | 32 | Implemented |
| CC Twister | `ec.transition.cctwister` | 5 | GPU | 32 | Implemented |
| CC WarpoMatic | `ec.transition.ccwarpomatic` | 8 | GPU | 32 | Implemented |
| Card Wipe | `ec.transition.cardwipe` | 44 | GPU | 32 | Implemented |
| Gradient Wipe | `ec.transition.gradientwipe` | 5 | GPU | 32 | Implemented |
| Iris Wipe | `ec.transition.iriswipe` | 7 | GPU | 32 | Implemented |
| Linear Wipe | `ec.transition.linearwipe` | 3 | GPU | 32 | Implemented |
| Radial Wipe | `ec.transition.radialwipe` | 5 | GPU | 32 | Implemented |
| Venetian Blinds | `ec.transition.venetian` | 4 | GPU | 32 | Implemented |

## Utility

| Effect | Id | Params | GPU | 32 | Status |
|---|---|---:|:-:|:-:|---|
| Apply Color LUT | `ec.utility.applylut` | 0 | GPU | 32 | Implemented |
| CC Overbrights | `ec.utility.ccoverbrights` | 2 | GPU | 32 | Implemented |
| Cineon Converter | `ec.utility.cineon` | 7 | GPU | 32 | Implemented |
| Color Profile Converter | `ec.utility.colorprofileconverter` | 7 | GPU | 32 | Implemented |
| Face Measurements | `ec.utility.facemeasurements` | 14 |  | 32 | Implemented |
| Face Track Points | `ec.utility.facetrackpoints` | 26 |  | 32 | Implemented |
| Grow Bounds | `ec.utility.growbounds` | 1 | GPU | 32 | Implemented |
| HDR Compander | `ec.utility.hdrcompander` | 3 | GPU | 32 | Implemented |
| HDR Highlight Compression | `ec.utility.hdrcompression` | 1 | GPU | 32 | Implemented |

