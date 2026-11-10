//! Display naming must not change file, command or script identities.
use crate::{find, instantiate, lookup, name_matches};
use effectcraft_project::build::Ids;

const RENAMED: &[(&str, &str, &str)] = &[
    ("ec.sim.ccballaction", "CC Ball Action", "Ball Grid"),
    ("ec.distort.ccbendit", "CC Bend It", "Two-Point Bend"),
    ("ec.distort.ccbender", "CC Bender", "Flexible Bend"),
    ("ec.distort.ccblobbylize", "CC Blobbylize", "Organic Displacement"),
    ("ec.stylize.ccblockload", "CC Block Load", "Block Reveal"),
    ("ec.sim.ccbubbles", "CC Bubbles", "Bubbles"),
    ("ec.stylize.ccburnfilm", "CC Burn Film", "Film Burn"),
    ("ec.color.cccolorneutralizer", "CC Color Neutralizer", "Color Neutralizer"),
    ("ec.color.cccoloroffset", "CC Color Offset", "Channel Offset"),
    ("ec.channel.cccomposite", "CC Composite", "Source Composite"),
    ("ec.blur.cccross", "CC Cross Blur", "Cross Blur"),
    ("ec.perspective.cccylinder", "CC Cylinder", "Cylinder Wrap"),
    ("ec.sim.ccdrizzle", "CC Drizzle", "Water Ripples"),
    ("ec.perspective.ccenvironment", "CC Environment", "Environment Map"),
    ("ec.distort.ccflomotion", "CC Flo Motion", "Dual Lens Warp"),
    ("ec.time.ccforcemotionblur", "CC Force Motion Blur", "Temporal Motion Blur"),
    ("ec.stylize.ccglass", "CC Glass", "Glass Relief"),
    ("ec.transition.ccglasswipe", "CC Glass Wipe", "Glass Wipe"),
    ("ec.generate.ccgluegun", "CC Glue Gun", "Paint Beads"),
    ("ec.transition.ccgridwipe", "CC Grid Wipe", "Grid Wipe"),
    ("ec.distort.ccgriddler", "CC Griddler", "Tile Grid"),
    ("ec.sim.cchair", "CC Hair", "Surface Fibers"),
    ("ec.stylize.cchextile", "CC HexTile", "Hexagonal Tiles"),
    ("ec.transition.ccimagewipe", "CC Image Wipe", "Luminance Image Wipe"),
    ("ec.transition.ccjaws", "CC Jaws", "Serrated Wipe"),
    ("ec.stylize.cckaleida", "CC Kaleida", "Kaleidoscope"),
    ("ec.color.cckernel", "CC Kernel", "Convolution Kernel"),
    ("ec.distort.cclens", "CC Lens", "Lens Distortion"),
    ("ec.generate.cclightburst", "CC Light Burst 2.5", "Radial Light Burst"),
    ("ec.generate.cclightrays", "CC Light Rays", "Light Rays"),
    ("ec.generate.cclightsweep", "CC Light Sweep", "Light Sweep"),
    ("ec.transition.cclightwipe", "CC Light Wipe", "Light Wipe"),
    ("ec.transition.cclinesweep", "CC Line Sweep", "Line Sweep"),
    ("ec.sim.ccmrmercury", "CC Mr. Mercury", "Liquid Particles"),
    ("ec.stylize.ccmrsmoothie", "CC Mr. Smoothie", "Palette Map"),
    ("ec.utility.ccoverbrights", "CC Overbrights", "Out-of-Range Display"),
    ("ec.distort.ccpageturn", "CC Page Turn", "Page Curl"),
    ("ec.sim.ccparticlesystems2", "CC Particle Systems II", "Particle Emitter 2D"),
    ("ec.sim.ccparticleworld", "CC Particle World", "Particle Emitter 3D"),
    ("ec.sim.ccpixelpolly", "CC Pixel Polly", "Pixel Shatter"),
    ("ec.stylize.ccplastic", "CC Plastic", "Plastic Shading"),
    ("ec.distort.ccpowerpin", "CC Power Pin", "Perspective Pin"),
    ("ec.blur.ccradial", "CC Radial Blur", "Variable Radial Blur"),
    ("ec.blur.ccradialfast", "CC Radial Fast Blur", "Fast Radial Blur"),
    ("ec.transition.ccradialscalewipe", "CC Radial ScaleWipe", "Radial Scale Wipe"),
    ("ec.sim.ccrainfall", "CC Rainfall", "Rainfall"),
    ("ec.stylize.ccrepetile", "CC RepeTile", "Edge Tile Repeat"),
    ("ec.distort.ccripplepulse", "CC Ripple Pulse", "Pulsed Ripple"),
    ("ec.transition.ccscalewipe", "CC Scale Wipe", "Scale Wipe"),
    ("ec.sim.ccscatterize", "CC Scatterize", "Pixel Scatter"),
    ("ec.key.ccsimplewireremoval", "CC Simple Wire Removal", "Line Removal"),
    ("ec.distort.ccslant", "CC Slant", "Slant"),
    ("ec.distort.ccsmear", "CC Smear", "Directional Smear"),
    ("ec.sim.ccsnowfall", "CC Snowfall", "Snowfall"),
    ("ec.perspective.ccsphere", "CC Sphere", "Sphere Wrap"),
    ("ec.distort.ccsplit", "CC Split", "Split Warp"),
    ("ec.distort.ccsplit2", "CC Split 2", "Dual Split Warp"),
    ("ec.perspective.ccspotlight", "CC Spotlight", "Surface Spotlight"),
    ("ec.sim.ccstarburst", "CC Star Burst", "Starfield"),
    ("ec.generate.ccthreads", "CC Threads", "Woven Threads"),
    ("ec.stylize.ccthreshold", "CC Threshold", "Channel Threshold"),
    ("ec.stylize.ccthresholdrgb", "CC Threshold RGB", "RGB Threshold"),
    ("ec.distort.cctiler", "CC Tiler", "Scaled Tiles"),
    ("ec.color.cctoner", "CC Toner", "Multi-Tone Tint"),
    ("ec.transition.cctwister", "CC Twister", "Twist Wipe"),
    ("ec.blur.ccvector", "CC Vector Blur", "Vector Blur"),
    ("ec.stylize.ccvignette", "CC Vignette", "Vignette"),
    ("ec.transition.ccwarpomatic", "CC WarpoMatic", "Warp Dissolve"),
    ("ec.time.ccwidetime", "CC Wide Time", "Temporal Blend"),
    ("ec.color.lumetri", "Lumetri Color", "Color Grading"),
    ("ec.obsolete.mochashape", "Mocha shape", "Tracked Shape Matte"),
    ("ec.keying.keylight", "Key Light", "Screen Color Key"),
];

#[test]
fn descriptive_names_keep_every_legacy_lookup_and_search() {
    for &(id, old, new) in RENAMED {
        let spec = find(id).unwrap();
        assert_eq!(spec.name, new, "{id}");
        for query in [id.to_string(), old.to_string(), old.to_lowercase(), new.to_string()] {
            assert_eq!(lookup(&query).map(|s| s.id), Some(id), "{query}");
        }
        assert!(name_matches(spec, &old.to_lowercase()), "{old}");
        assert!(name_matches(spec, &new.to_lowercase()), "{new}");
    }
    for alias in ["Keylight", "Keylight (1.2)", "Key Light"] {
        assert_eq!(lookup(alias).map(|s| s.id), Some("ec.keying.keylight"));
    }
}

#[test]
fn descriptive_display_names_do_not_collide() {
    let mut names = std::collections::HashSet::new();
    for spec in crate::registry() {
        assert!(names.insert(spec.name.to_lowercase()), "duplicate name {}", spec.name);
        let label = spec.name.to_lowercase();
        assert!(!label.starts_with("cc "), "{}", spec.name);
        for mark in ["lumetri", "mercury", "mocha", "keylight"] {
            assert!(!label.contains(mark), "{}", spec.name);
        }
    }
}

#[test]
fn legacy_instance_names_and_parameter_data_survive_refresh() {
    for &(id, old, _) in RENAMED {
        let spec = find(id).unwrap();
        for stored in [old.to_string(), format!("{old} 2"), "My custom instance".to_string()] {
            let mut next = 1;
            let mut ids = Ids(&mut next);
            let mut instance = instantiate(spec, &mut ids, &stored, [128.0, 72.0]);
            let before = serde_json::to_value(&instance).unwrap();
            crate::migrate::upgrade_instance(spec, &mut instance, &mut ids, [128.0, 72.0]);
            assert_eq!(instance.name, stored, "{id}: never rewrite a stored instance name");
            assert_eq!(serde_json::to_value(&instance).unwrap(), before, "{id}");
        }
    }
}
