# Descriptive feature names

Related to #1365. This is a naming cleanup, not a trademark clearance or a product rename.

## Naming policy

Use short, descriptive labels for EffectCraft implementations. Do not reuse a vendor prefix or
coined feature name as our branding. Qualify similar effects by what distinguishes them; each
English display name remains unique across the registry.

- Renderer: **GPU Acceleration** / **Software Rendering**.
- Panel: **Video Scopes**.
- Tracking command: **Motion Stabilizer**; tool and settings: **Rotoscope Brush**.
- The table below covers the 69 former CC effects plus color grading, the shape matte, keying,
  rotoscoping and stabilization.

## Compatibility contract

Only presentation names change. Effect IDs, command IDs, parameter IDs, enum values, panel IDs,
serialized types and AEP import/export match names stay unchanged. Old effect display names
remain case-insensitive lookup and search aliases. Existing project instance names, custom names,
expressions, animation and values are not rewritten. The UI displays a current label for an exact
legacy default name, optionally followed by its numeric instance suffix; all other names stay
verbatim. A user-entered name identical to a legacy default cannot be distinguished from it, but
its saved value is still retained.

External format specifications, comparison documents, attribution records and compatibility
fixtures may still name the external product or effect they actually describe.

## Display mapping

| Stable ID | Previous display name | Current display name |
|---|---|---|
| `ec.sim.ccballaction` | CC Ball Action | Ball Grid |
| `ec.distort.ccbendit` | CC Bend It | Two-Point Bend |
| `ec.distort.ccbender` | CC Bender | Flexible Bend |
| `ec.distort.ccblobbylize` | CC Blobbylize | Organic Displacement |
| `ec.stylize.ccblockload` | CC Block Load | Block Reveal |
| `ec.sim.ccbubbles` | CC Bubbles | Bubbles |
| `ec.stylize.ccburnfilm` | CC Burn Film | Film Burn |
| `ec.color.cccolorneutralizer` | CC Color Neutralizer | Color Neutralizer |
| `ec.color.cccoloroffset` | CC Color Offset | Channel Offset |
| `ec.channel.cccomposite` | CC Composite | Source Composite |
| `ec.blur.cccross` | CC Cross Blur | Cross Blur |
| `ec.perspective.cccylinder` | CC Cylinder | Cylinder Wrap |
| `ec.sim.ccdrizzle` | CC Drizzle | Water Ripples |
| `ec.perspective.ccenvironment` | CC Environment | Environment Map |
| `ec.distort.ccflomotion` | CC Flo Motion | Dual Lens Warp |
| `ec.time.ccforcemotionblur` | CC Force Motion Blur | Temporal Motion Blur |
| `ec.stylize.ccglass` | CC Glass | Glass Relief |
| `ec.transition.ccglasswipe` | CC Glass Wipe | Glass Wipe |
| `ec.generate.ccgluegun` | CC Glue Gun | Paint Beads |
| `ec.transition.ccgridwipe` | CC Grid Wipe | Grid Wipe |
| `ec.distort.ccgriddler` | CC Griddler | Tile Grid |
| `ec.sim.cchair` | CC Hair | Surface Fibers |
| `ec.stylize.cchextile` | CC HexTile | Hexagonal Tiles |
| `ec.transition.ccimagewipe` | CC Image Wipe | Luminance Image Wipe |
| `ec.transition.ccjaws` | CC Jaws | Serrated Wipe |
| `ec.stylize.cckaleida` | CC Kaleida | Kaleidoscope |
| `ec.color.cckernel` | CC Kernel | Convolution Kernel |
| `ec.distort.cclens` | CC Lens | Lens Distortion |
| `ec.generate.cclightburst` | CC Light Burst 2.5 | Radial Light Burst |
| `ec.generate.cclightrays` | CC Light Rays | Light Rays |
| `ec.generate.cclightsweep` | CC Light Sweep | Light Sweep |
| `ec.transition.cclightwipe` | CC Light Wipe | Light Wipe |
| `ec.transition.cclinesweep` | CC Line Sweep | Line Sweep |
| `ec.sim.ccmrmercury` | CC Mr. Mercury | Liquid Particles |
| `ec.stylize.ccmrsmoothie` | CC Mr. Smoothie | Color Flow |
| `ec.utility.ccoverbrights` | CC Overbrights | Highlight Recovery |
| `ec.distort.ccpageturn` | CC Page Turn | Page Curl |
| `ec.sim.ccparticlesystems2` | CC Particle Systems II | Particle Emitter 2D |
| `ec.sim.ccparticleworld` | CC Particle World | Particle Emitter 3D |
| `ec.sim.ccpixelpolly` | CC Pixel Polly | Pixel Shatter |
| `ec.stylize.ccplastic` | CC Plastic | Plastic Shading |
| `ec.distort.ccpowerpin` | CC Power Pin | Perspective Pin |
| `ec.blur.ccradial` | CC Radial Blur | Variable Radial Blur |
| `ec.blur.ccradialfast` | CC Radial Fast Blur | Fast Radial Blur |
| `ec.transition.ccradialscalewipe` | CC Radial ScaleWipe | Radial Scale Wipe |
| `ec.sim.ccrainfall` | CC Rainfall | Rainfall |
| `ec.stylize.ccrepetile` | CC RepeTile | Edge Tile Repeat |
| `ec.distort.ccripplepulse` | CC Ripple Pulse | Pulsed Ripple |
| `ec.transition.ccscalewipe` | CC Scale Wipe | Scale Wipe |
| `ec.sim.ccscatterize` | CC Scatterize | Pixel Scatter |
| `ec.key.ccsimplewireremoval` | CC Simple Wire Removal | Line Removal |
| `ec.distort.ccslant` | CC Slant | Slant |
| `ec.distort.ccsmear` | CC Smear | Directional Smear |
| `ec.sim.ccsnowfall` | CC Snowfall | Snowfall |
| `ec.perspective.ccsphere` | CC Sphere | Sphere Wrap |
| `ec.distort.ccsplit` | CC Split | Split Warp |
| `ec.distort.ccsplit2` | CC Split 2 | Dual Split Warp |
| `ec.perspective.ccspotlight` | CC Spotlight | Surface Spotlight |
| `ec.sim.ccstarburst` | CC Star Burst | Starfield |
| `ec.generate.ccthreads` | CC Threads | Woven Threads |
| `ec.stylize.ccthreshold` | CC Threshold | Channel Threshold |
| `ec.stylize.ccthresholdrgb` | CC Threshold RGB | RGB Threshold |
| `ec.distort.cctiler` | CC Tiler | Offset Tiles |
| `ec.color.cctoner` | CC Toner | Multi-Tone Tint |
| `ec.transition.cctwister` | CC Twister | Twist Wipe |
| `ec.blur.ccvector` | CC Vector Blur | Vector Blur |
| `ec.stylize.ccvignette` | CC Vignette | Vignette |
| `ec.transition.ccwarpomatic` | CC WarpoMatic | Warp Dissolve |
| `ec.time.ccwidetime` | CC Wide Time | Temporal Blend |
| `ec.color.lumetri` | Lumetri Color | Color Grading |
| `ec.obsolete.mochashape` | Mocha shape | Tracked Shape Matte |
| `ec.keying.keylight` | Key Light | Screen Color Key |
| `ec.matte.rotobrush` | Roto Brush & Refine Edge | Rotoscope & Refine Edge |
| `ec.distort.warpstabilizer` | Warp Stabilizer | Motion Stabilizer |

## Review and release plan

1. Review the names and the six existing effect-language catalogs (Japanese, Simplified and
   Traditional Chinese, French, Swedish and Georgian). Other effect-language catalogs do not
   exist yet and continue their established English fallback. Menu labels are updated in all
   13 existing menu languages; renderer and tool labels follow existing panel-language coverage.
2. Run the effects naming/alias tests, the UI catalog tests, AEP and scripting round-trip tests,
   then the unchanged shared `cargo xtask ci` gate. Verify both old projects and new instances.
3. Inspect the Effects & Presets search, Effect Controls, repeated and custom instance names,
   renderer dropdown, Video Scopes, tracking command and rotoscope tool in the running app.
4. Recapture the effects screenshot from the actual updated application with licensed demo
   media. The README no longer embeds the stale `docs/images/effectcraft-effects.png`; the
   historical image and its attribution remain intact until an authentic replacement is ready.
5. Mark ready only after CI and runtime review. This change does not authorize a merge or release.

No effect algorithms or formats are changed. Remaining partial localization and the screenshot
recapture are explicit review items, not claims of completed runtime or legal validation.
