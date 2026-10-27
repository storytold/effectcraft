use super::cdl::sl;
use super::effects::{
    CONFIGS, DISPLAYS, PROFILES, VIEWS, color_stabilizer, direction, looks, ocio_cdl, ocio_cst, ocio_display, ocio_file, ocio_look, profile_converter,
    space_param, spec,
};
use super::space::SPACES;
use super::*;

pub fn specs() -> Vec<EffectSpec> {
    let cc = "Color Correction";
    let srgb = SPACES.iter().position(|s| s.name == "sRGB").unwrap_or(0) as u32;
    let lin709 = SPACES.iter().position(|s| s.name == "Linear Rec.709 (sRGB)").unwrap_or(0) as u32;
    let acescg = SPACES.iter().position(|s| s.name == "ACEScg").unwrap_or(0) as u32;
    let mut look_opts = vec!["None"];
    look_opts.extend(looks().iter().map(|(n, _)| *n));
    vec![
        spec(
            "ec.color.ociocdl",
            "OCIO CDL Transform",
            cc,
            vec![
                p("slopeRed", "Slope Red", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("slopeGreen", "Slope Green", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("slopeBlue", "Slope Blue", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("offsetRed", "Offset Red", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("offsetGreen", "Offset Green", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("offsetBlue", "Offset Blue", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("powerRed", "Power Red", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("powerGreen", "Power Green", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("powerBlue", "Power Blue", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("saturation", "Saturation", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("style", "Style", Value::Enum(0), popup(&["ASC CDL v1.2", "No Clamp"])),
                direction(),
            ],
            ocio_cdl,
        ),
        spec(
            "ec.color.ociocolorspace",
            "OCIO Color Space Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(CONFIGS)),
                p("configFile", "Config File (.ocio)", Value::Str(String::new()), crate::file_ui(&["ocio"])),
                space_param("source", "Source", lin709),
                p("sourceName", "Source (config name)", Value::Str(String::new()), ParamUi::Text),
                space_param("destination", "Destination", srgb),
                p("destinationName", "Destination (config name)", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_cst,
        ),
        spec(
            "ec.color.ociodisplay",
            "OCIO Display Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(CONFIGS)),
                p("configFile", "Config File (.ocio)", Value::Str(String::new()), crate::file_ui(&["ocio"])),
                space_param("source", "Input Color Space", lin709),
                p("sourceName", "Source (config name)", Value::Str(String::new()), ParamUi::Text),
                p("display", "Display Device", Value::Enum(0), popup(&DISPLAYS.iter().map(|d| d.0).collect::<Vec<_>>())),
                p("displayName", "Display (config name)", Value::Str(String::new()), ParamUi::Text),
                p("view", "View Transform", Value::Enum(0), popup(VIEWS)),
                p("viewName", "View (config name)", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_display,
        ),
        // `file`: a path to a .cube/.3dl/.csp/.cc/.ccc/.cdl file, or the file's text itself.
        spec(
            "ec.color.ociofile",
            "OCIO File Transform",
            cc,
            vec![
                p("file", "File", Value::Str(String::new()), crate::file_ui(&["cube", "3dl", "csp", "cc", "ccc", "cdl", "spi1d", "spi3d", "spimtx"])),
                p("cccId", "CCC ID", Value::Str(String::new()), ParamUi::Text),
                p("interpolation", "Interpolation", Value::Enum(3), popup(&["Nearest", "Linear", "Tetrahedral", "Best"])),
                direction(),
            ],
            ocio_file,
        ),
        spec(
            "ec.color.ociolook",
            "OCIO Look Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(&["Built-in (EffectCraft minimal)"])),
                space_param("source", "Source", acescg),
                space_param("destination", "Destination", acescg),
                p("look", "Look", Value::Enum(0), popup(&look_opts)),
                p("looks", "Looks", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_look,
        ),
        spec(
            "ec.color.colorstabilizer",
            "Color Stabilizer",
            cc,
            vec![
                // The pivot frame (After Effects' Set Frame button stores it).
                p("referenceFrame", "Reference Frame (s)", num(0.0), slider(-100000.0, 100000.0, 0.0, 60.0, 2)),
                p("stabilize", "Stabilize", Value::Enum(0), popup(&["Brightness", "Levels", "Curves"])),
                p("blackPoint", "Black Point", Value::Vec2([0.25, 0.75]), ParamUi::Point),
                p("midPoint", "Mid Point", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("whitePoint", "White Point", Value::Vec2([0.75, 0.25]), ParamUi::Point),
                p("sampleSize", "Sample Size", num(5.0), slider(1.0, 100.0, 1.0, 50.0, 0)),
            ],
            color_stabilizer,
        ),
        spec(
            "ec.utility.colorprofileconverter",
            "Color Profile Converter",
            "Utility",
            vec![
                p("inputProfile", "Input Profile", Value::Enum(0), popup(PROFILES)),
                p("linearizeInputProfile", "Linearize Input Profile", Value::Bool(false), ParamUi::Checkbox),
                p("outputProfile", "Output Profile", Value::Enum(1), popup(PROFILES)),
                p("linearizeOutputProfile", "Linearize Output Profile", Value::Bool(false), ParamUi::Checkbox),
                p("intent", "Intent", Value::Enum(1), popup(&["Perceptual", "Relative Colorimetric", "Saturation", "Absolute Colorimetric"])),
                // Our built-in profiles share a zero black point and none is scene-referred, so
                // these two are kept for compatibility and do not change the result.
                p("useBlackPointCompensation", "Use Black Point Compensation", Value::Bool(true), ParamUi::Checkbox),
                p("sceneRefCompensation", "Scene-ref. Profile Compensation", Value::Enum(2), popup(&["On", "Off", "Use Project Setting"])),
            ],
            profile_converter,
        ),
    ]
}
