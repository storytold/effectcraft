//! The camera, light, shape and text animator skeletons as tables of [`Node`]s: match names, states, default
//! names and the decoded fields of each record (`tree_spec`). The rows were listed from our own
//! cases (the case numbers in each doc comment) through the decoded fields only.

#![cfg_attr(rustfmt, rustfmt::skip)]

use crate::export::tree_spec::{Node, Node::{G, O, P, R}, Rec};

/// The property tree of a two-node camera layer (256): Transform with its records and the Camera Options group.
pub(crate) static CAMERA_ROOT: &[Node] = &[
    G("ADBE Transform Group", 1, "-_0_/-", &[
        R("ADBE Anchor Point", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 72, cdat: &[960.0, 540.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[(76, 1), (77, 1)], cdat_len: 72, cdat: &[0.0, 0.0, -2666.6666666], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position_0", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_1", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_2", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Scale", Rec { state: 3, label: "-_0_/-", dims: 3, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 120, cdat: &[1.0, 1.0, 1.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Rotate Z", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Opacity", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Envir Appear in Reflect", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    ]),
    G("ADBE Camera Options Group", 1, "-_0_/-", &[
    ]),
];

/// The property tree of a spot light layer (254).
pub(crate) static SPOT_ROOT: &[Node] = &[
    G("ADBE Transform Group", 1, "-_0_/-", &[
        R("ADBE Anchor Point", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 72, cdat: &[960.0, 540.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[(76, 1), (77, 1)], cdat_len: 72, cdat: &[0.0, 0.0, -1333.3333333], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position_0", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_1", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_2", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Scale", Rec { state: 3, label: "-_0_/-", dims: 3, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 120, cdat: &[1.0, 1.0, 1.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Rotate Z", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Opacity", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Envir Appear in Reflect", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    ]),
    G("ADBE Light Options Group", 1, "-_0_/-", &[
        G("ADBE Light Env Atom", 3, "-_0_/-", &[
        ]),
        R("ADBE Light Backgd Visible", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Light Backgd Opacity", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[100.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Light Backgd Blur", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Light Intensity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Light Falloff Start", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[500.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Light Falloff Distance", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[500.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Casts Shadows", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    ]),
];

/// The property tree of a point light layer (252).
pub(crate) static POINT_ROOT: &[Node] = &[
    G("ADBE Transform Group", 1, "-_0_/-", &[
        R("ADBE Anchor Point", Rec { state: 3, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 72, cdat: &[960.0, 540.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[(76, 1), (77, 1)], cdat_len: 72, cdat: &[0.0, 0.0, -1333.3333333], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Position_0", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_1", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_2", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Scale", Rec { state: 3, label: "-_0_/-", dims: 3, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 120, cdat: &[1.0, 1.0, 1.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        O("ADBE Orientation", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x00060007, byte5: 0x07, header: &[(79, 1)], cdat_len: 24, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Rotate X", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Rotate Y", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Rotate Z", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Opacity", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Envir Appear in Reflect", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    ]),
    G("ADBE Light Options Group", 1, "-_0_/-", &[
        G("ADBE Light Env Atom", 3, "-_0_/-", &[
        ]),
        R("ADBE Light Backgd Visible", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Light Backgd Opacity", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[100.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Light Backgd Blur", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Light Cone Angle", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[90.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Light Cone Feather 2", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Light Falloff Start", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[500.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Light Falloff Distance", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[500.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Casts Shadows", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    ]),
];

/// The property tree of a shape layer (519) apart from Time Remapping and the contents.
pub(crate) static SHAPE_ROOT: &[Node] = &[
    G("ADBE Root Vectors Group", 1025, "-_0_/-", &[
    ]),
    G("ADBE Transform Group", 1, "-_0_/-", &[
        R("ADBE Position_0", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        R("ADBE Position_1", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
        O("ADBE Orientation", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x00060007, byte5: 0x07, header: &[(79, 1)], cdat_len: 24, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Rotate X", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Rotate Y", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Envir Appear in Reflect", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    ]),
    G("ADBE Layer Styles", 3, "-_0_/-", &[
        G("ADBE Blend Options Group", 3, "-_0_/-", &[
            G("ADBE Adv Blend Group", 1, "-_0_/-", &[
            ]),
        ]),
        G("dropShadow/enabled", 2, "-_0_/-", &[
        ]),
        G("innerShadow/enabled", 2, "-_0_/-", &[
        ]),
        G("outerGlow/enabled", 2, "-_0_/-", &[
        ]),
        G("innerGlow/enabled", 2, "-_0_/-", &[
        ]),
        G("bevelEmboss/enabled", 2, "-_0_/-", &[
        ]),
        G("chromeFX/enabled", 2, "-_0_/-", &[
        ]),
        G("solidFill/enabled", 2, "-_0_/-", &[
        ]),
        G("gradientFill/enabled", 2, "-_0_/-", &[
        ]),
        G("patternFill/enabled", 2, "-_0_/-", &[
        ]),
        G("frameFX/enabled", 2, "-_0_/-", &[
        ]),
    ]),
    G("ADBE Extrsn Options Group", 3, "-_0_/-", &[
        R("ADBE Bevel Direction", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    ]),
    G("ADBE Material Options Group", 3, "-_0_/-", &[
        R("ADBE Casts Shadows", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Light Transmission", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Accepts Shadows", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Accepts Lights", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Shadow Color", Rec { state: 3, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Appears in Reflections", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
        R("ADBE Ambient Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[100.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Diffuse Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Specular Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Shininess Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[5.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Metal Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[100.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Reflection Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Glossiness Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[100.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Fresnel Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Transparency Coefficient", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Transp Rolloff", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
        R("ADBE Index of Refraction", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: Some((1.0, 2.0)), tdpi: None }),
    ]),
    G("ADBE Audio Group", 3, "-_0_/-", &[
    ]),
    G("ADBE Layer Sets", 3, "-_0_/-", &[
    ]),
];

/// The camera and light option records the exporter writes (342-345, 254).
pub(crate) static OPTIONS: &[Node] = &[
    R("ADBE Camera Depth of Field", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Camera Focus Distance", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[800.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
    R("ADBE Camera Aperture", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[30.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
    R("ADBE Light Intensity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
    R("ADBE Light Color", Rec { state: 1, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0, 255.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Light Cone Angle", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[60.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Light Cone Feather 2", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[30.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
];

/// The shape items the exporter writes, as empty groups with their default names (240, 451-471).
pub(crate) static SHAPE_ITEMS: &[Node] = &[
    G("ADBE Vector Group", 1, "Group 1", &[
        G("ADBE Vectors Group", 1025, "-_0_/-", &[
        ]),
        G("ADBE Vector Transform Group", 1, "-_0_/-", &[
        ]),
        G("ADBE Vector Materials Group", 1, "-_0_/-", &[
        ]),
    ]),
    G("ADBE Vector Shape - Rect", 1, "Rectangle Path 1", &[
    ]),
    G("ADBE Vector Shape - Ellipse", 1, "Ellipse Path 1", &[
    ]),
    G("ADBE Vector Shape - Star", 1, "Polystar Path 1", &[
    ]),
    G("ADBE Vector Graphic - Fill", 1, "Fill 1", &[
    ]),
    G("ADBE Vector Graphic - Stroke", 1, "Stroke 1", &[
        G("ADBE Vector Stroke Dashes", 1, "-_0_/-", &[
        ]),
        G("ADBE Vector Stroke Taper", 1, "-_0_/-", &[
        ]),
        G("ADBE Vector Stroke Wave", 1, "-_0_/-", &[
        ]),
    ]),
    G("ADBE Vector Graphic - G-Fill", 1, "Gradient Fill 1", &[
    ]),
    G("ADBE Vector Graphic - G-Stroke", 1, "Gradient Stroke 1", &[
        G("ADBE Vector Stroke Dashes", 1, "-_0_/-", &[
        ]),
        G("ADBE Vector Stroke Taper", 1, "-_0_/-", &[
        ]),
        G("ADBE Vector Stroke Wave", 1, "-_0_/-", &[
        ]),
    ]),
    G("ADBE Vector Filter - Trim", 1, "Trim Paths 1", &[
    ]),
    G("ADBE Vector Filter - RC", 1, "Round Corners 1", &[
    ]),
    G("ADBE Vector Filter - Merge", 1, "Merge Paths 1", &[
    ]),
    G("ADBE Vector Filter - Offset", 1, "Offset Paths 1", &[
    ]),
    G("ADBE Vector Filter - Repeater", 1, "Repeater 1", &[
        G("ADBE Vector Repeater Transform", 1, "-_0_/-", &[
        ]),
    ]),
    G("ADBE Vector Shape - Group", 1, "Path 1", &[
        P("ADBE Vector Shape", Rec { state: 1, label: "-_0_/-", dims: 1, template: 1, class: 0x00020007, byte5: 0x07, header: &[], cdat_len: 4, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    ]),
];

/// The shape value records (239, 242).
pub(crate) static SHAPE_RECORDS: &[Node] = &[
    R("ADBE Vector Ellipse Size", Rec { state: 1, label: "-_0_/-", dims: 2, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 80, cdat: &[400.0, 300.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Vector Fill Color", Rec { state: 1, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0, 0.0, 255.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Ellipse Position", Rec { state: 1, label: "-_0_/-", dims: 2, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 48, cdat: &[100.0, 50.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Fill Opacity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Fill Rule", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Anchor", Rec { state: 1, label: "-_0_/-", dims: 2, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 48, cdat: &[10.0, 20.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Scale", Rec { state: 1, label: "-_0_/-", dims: 2, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 80, cdat: &[50.0, 80.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Vector Rotation", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[30.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Group Opacity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[40.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Skew", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[20.0], key_size: 0, limits: Some((-85.0, 85.0)), tdpi: None }),
    R("ADBE Vector Skew Axis", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[45.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Stroke Color", Rec { state: 1, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0, 255.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Stroke Width", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[6.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Stroke Opacity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Rect Size", Rec { state: 1, label: "-_0_/-", dims: 2, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 80, cdat: &[200.0, 100.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Vector Rect Roundness", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[30.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Stroke Line Cap", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Stroke Line Join", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Vector Stroke Miter Limit", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[4.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Vector Position", Rec { state: 1, label: "-_0_/-", dims: 2, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 48, cdat: &[50.0, 50.0], key_size: 0, limits: None, tdpi: None }),
];

/// The empty text animator group of a text layer (579).
pub(crate) static TEXT_ANIMATORS: &[Node] = &[
    G("ADBE Text Animators", 1, "-_0_/-", &[
    ]),
];

/// One text animator with a range selector and an empty property group (579, 580).
pub(crate) static TEXT_ANIMATOR: &[Node] = &[
    G("ADBE Text Animator", 1, "Animator 1", &[
        G("ADBE Text Selectors", 1, "-_0_/-", &[
            G("ADBE Text Selector", 1, "Range Selector 1", &[
                G("ADBE Text Range Advanced", 1, "-_0_/-", &[
                ]),
            ]),
        ]),
        G("ADBE Text Animator Properties", 1, "-_0_/-", &[
        ]),
    ]),
];

/// The text animator property records (580-592, 728).
pub(crate) static TEXT_ANIMATOR_RECORDS: &[Node] = &[
    R("ADBE Text Position 3D", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 72, cdat: &[0.0, -50.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Anchor Point 3D", Rec { state: 1, label: "-_0_/-", dims: 3, template: 3, class: 0xffffffff, byte5: 0x0f, header: &[], cdat_len: 72, cdat: &[3.0, 4.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Scale 3D", Rec { state: 1, label: "-_0_/-", dims: 3, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 120, cdat: &[150.0, 120.0, 100.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Text Skew", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[10.0], key_size: 0, limits: Some((-70.0, 70.0)), tdpi: None }),
    R("ADBE Text Skew Axis", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[20.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Rotation", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[45.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Opacity", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[37.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Text Fill Color", Rec { state: 1, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0, 0.0, 255.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Stroke Color", Rec { state: 1, label: "-_0_/-", dims: 4, template: 0, class: 0x0002ffff, byte5: 0x07, header: &[(79, 1)], cdat_len: 96, cdat: &[255.0, 0.0, 0.0, 255.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Stroke Width", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[3.0], key_size: 0, limits: Some((-1000.0, 1000.0)), tdpi: None }),
    R("ADBE Text Tracking Amount", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[20.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Text Line Spacing", Rec { state: 1, label: "-_0_/-", dims: 2, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 80, cdat: &[10.0, 20.0], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Text Blur", Rec { state: 1, label: "-_0_/-", dims: 2, template: 0, class: 0x0002ffff, byte5: 0x01, header: &[], cdat_len: 80, cdat: &[5.0, 6.0], key_size: 0, limits: Some((0.0, 10000.0)), tdpi: None }),
    R("ADBE Text Percent Start", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[25.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Text Percent End", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[75.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Text Percent Offset", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[10.0], key_size: 0, limits: Some((-100.0, 100.0)), tdpi: None }),
    R("ADBE Text Range Units", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Index Start", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 32000.0)), tdpi: None }),
    R("ADBE Text Index End", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[4.0], key_size: 0, limits: Some((0.0, 32000.0)), tdpi: None }),
    R("ADBE Text Index Offset", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((-32000.0, 32000.0)), tdpi: None }),
    R("ADBE Text Range Type2", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[3.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Selector Mode", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Selector Max Amount", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((-100.0, 100.0)), tdpi: None }),
    R("ADBE Text Range Shape", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0x00020000, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[2.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Selector Smoothness", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[50.0], key_size: 0, limits: Some((0.0, 100.0)), tdpi: None }),
    R("ADBE Text Levels Max Ease", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[30.0], key_size: 0, limits: Some((-100.0, 100.0)), tdpi: None }),
    R("ADBE Text Levels Min Ease", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[40.0], key_size: 0, limits: Some((-100.0, 100.0)), tdpi: None }),
    R("ADBE Text Randomize Order", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Text Random Seed", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[7.0], key_size: 0, limits: Some((0.0, 10000.0)), tdpi: None }),
    R("ADBE Text Character Offset", Rec { state: 1, label: "-_0_/-", dims: 1, template: 0, class: 0xffffffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[3.0], key_size: 0, limits: Some((-65535.0, 65535.0)), tdpi: None }),
];

/// The inert Transform records of an AV layer (000): split Position, Orientation, X and Y
/// Rotation, Appears in Reflections, all at their defaults (state 3).
pub(crate) static AV_TRANSFORM: &[Node] = &[
    R("ADBE Position_0", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
    R("ADBE Position_1", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: Some((0.0, 0.0)), tdpi: None }),
    O("ADBE Orientation", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x00060007, byte5: 0x07, header: &[(79, 1)], cdat_len: 24, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Rotate X", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Rotate Y", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0x0001ffff, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[], key_size: 0, limits: None, tdpi: None }),
    R("ADBE Envir Appear in Reflect", Rec { state: 3, label: "-_0_/-", dims: 1, template: 0, class: 0xffff0004, byte5: 0x01, header: &[], cdat_len: 40, cdat: &[1.0], key_size: 0, limits: None, tdpi: None }),
];
