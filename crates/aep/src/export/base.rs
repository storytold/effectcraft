//! The project skeleton built in code (aep-format.md 3.1-3.3, 3.12a): the top-level chunks, the
//! composition shell with its view layers, the `Solids` folder, a solid item and the records that
//! follow a composition. Fields the exporter sets per item are left zero here; the bytes that are
//! not decoded are named constants, each one seen with the same value in every sample of its kind
//! (000, 022, 211, 259, 620-634) and kept because After Effects refuses the file without them
//! (read-backs t-no<id>, t-zcdta, u-zsspc, u-noclrs, u-noftgi, u-nofrec, u-nocdrp).

use crate::Chunk;
use crate::export::synth::{tdmn, tdsn};

/// A zero buffer of `len` bytes with `fields` (offset, bytes) written into it.
pub(crate) fn bytes(len: usize, fields: &[(usize, &[u8])]) -> Vec<u8> {
    let mut b = vec![0u8; len];
    for (at, v) in fields {
        if let Some(dst) = b.get_mut(*at..at.saturating_add(v.len())) {
            dst.copy_from_slice(v);
        }
    }
    b
}

pub(crate) fn utf8(text: &str) -> Chunk<'static> {
    Chunk::data(*b"Utf8", text.as_bytes().to_vec())
}

/// The After Effects version the file claims (`svap`, `head` 4..7): 26.x, the version every
/// sample was saved with.
const VERSION: [u8; 4] = [0x0f, 0x11, 0x0e, 0x02];
/// `head` 0..3 and byte 8: the same in every sample (not decoded; a zero `head` is refused as
/// "created with an older version", read-back u-zhead).
const HEAD_FORMAT: [u8; 4] = [0x00, 0x61, 0x00, 0x05];
/// The code page of the project's strings (`nhed` 18, `nnhd` 26): 65001, UTF-8.
const UTF8_CODE_PAGE: [u8; 2] = 65001u16.to_be_bytes();
/// The size of the render-queue settings blocks (`AFsi`, `ARsi`); zero bodies of this size are
/// read (read-back e1-zafsi), a missing one is refused (e1-noafsi).
const RQ_SETTINGS_LEN: usize = 1872;
/// The record size of the render-queue item list (`LRdr/list/lhd3` word 4).
const RQ_ITEM_LEN: u32 = 2246;
/// The id of the `Solids` folder, which `sfnm/sfid` names.
pub(crate) const SOLIDS_FOLDER_ID: u32 = 0x19;
/// The default end time of a view layer (5 s in 25 600 units): view layers keep it whatever the
/// composition's duration (000, 031, 032).
const VIEW_OUT: u32 = 128_000;

/// The top-level chunks of a project: the headers (`settings::emit` writes the settings into
/// them, `emit` the next free id), the project lists (`effects::put_project_lists` fills `Pefl`),
/// an empty `Fold` with its `fdta`, the workspace name and the render queue.
pub(crate) fn project_chunks() -> Vec<Chunk<'static>> {
    let head = bytes(20, &[(0, &HEAD_FORMAT), (4, &VERSION), (8, &[0x80])]);
    // nhed / nnhd: bytes 7 and 9 the same in every sample, byte 12 / 15 the timecode base (30),
    // 13 / 19 the frames per foot (16), 14 / 20 the frame count start (2: from 0), 15 / 24 the
    // colour depth (2: 32 bits); the save noise after the code page is left zero.
    let nhed = bytes(32, &[(7, &[5]), (9, &[1]), (12, &[0x1e, 16, 2, 2]), (18, &UTF8_CODE_PAGE)]);
    let nnhd = bytes(40, &[(7, &[5]), (9, &[1]), (15, &[0x1e]), (19, &[16, 2]), (24, &[2]), (26, &UTF8_CODE_PAGE)]);
    let mut lhd3 = bytes(52, &[(0, &0x00d0_0bee_u32.to_be_bytes()), (12, &1u32.to_be_bytes()), (16, &RQ_ITEM_LEN.to_be_bytes())]);
    for at in [20, 24, 28] {
        if let Some(dst) = lhd3.get_mut(at..at + 4) {
            dst.copy_from_slice(&1u32.to_be_bytes());
        }
    }
    let wsnm: Vec<u8> = "Default".encode_utf16().flat_map(u16::to_be_bytes).collect();
    vec![
        Chunk::data(*b"svap", VERSION.to_vec()),
        Chunk::data(*b"head", head),
        Chunk::data(*b"nhed", nhed),
        Chunk::data(*b"nnhd", nnhd),
        // The project's audio sample rate.
        Chunk::data(*b"adfr", 48_000f64.to_be_bytes().to_vec()),
        Chunk::list(*b"Pefl", Vec::new()),
        Chunk::data(*b"qtlg", vec![0]),
        Chunk::list(*b"sfnm", vec![utf8("Solids"), Chunk::data(*b"sfid", SOLIDS_FOLDER_ID.to_le_bytes().to_vec())]),
        Chunk::data(*b"mrid", vec![0; 4]),
        // Compensate for Scene-referred Profiles: on.
        Chunk::data(*b"acer", vec![1]),
        Chunk::list(*b"CPPl", Vec::new()),
        Chunk::data(*b"cpid", vec![0xff; 16]),
        Chunk::data(*b"dwga", vec![0; 4]),
        Chunk::data(*b"pcms", vec![1]),
        utf8("{\"ocioConfigurationFile\":\"ACES 1.2\"}"),
        Chunk::data(*b"PwCs", vec![1]),
        utf8("{}"),
        Chunk::data(*b"pdvc", vec![1]),
        utf8("{}"),
        Chunk::list(*b"ExEn", vec![utf8("javascript-1.0")]),
        Chunk::list(*b"Fold", vec![Chunk::data(*b"fdta", vec![0; 14])]),
        Chunk::data(*b"wsns", vec![0, 0x0e]),
        Chunk::data(*b"wsnm", wsnm),
        utf8("Default"),
        Chunk::data(*b"fcid", vec![0; 4]),
        Chunk::data(*b"oacc", vec![0; 2]),
        Chunk::list(*b"LSIf", vec![Chunk::data(*b"AFsi", vec![0; RQ_SETTINGS_LEN])]),
        Chunk::list(
            *b"LRdr",
            vec![
                Chunk::data(*b"Rhed", vec![0; 20]),
                Chunk::data(*b"Rout", vec![0; 4]),
                Chunk::list(*b"list", vec![Chunk::data(*b"lhd3", lhd3)]),
                Chunk::list(*b"LItm", Vec::new()),
                Chunk::list(*b"LSIf", vec![Chunk::data(*b"ARsi", vec![0; RQ_SETTINGS_LEN])]),
            ],
        ),
    ]
}

/// `idta` of an item: the kind at 0, byte 23 per kind (0x20 composition, 0x30 footage, 0 folder;
/// After Effects renders differently without it, read-backs t-zidta and u-zidta2); `emit` writes
/// the id and the label.
pub(crate) fn idta(kind: u16, byte23: u8) -> Chunk<'static> {
    Chunk::data(*b"idta", bytes(84, &[(0, &kind.to_be_bytes()), (23, &[byte23])]))
}

/// The guides of a layer or item (`Gide`: `gdta` zero and an empty list header).
pub(crate) fn guides() -> Chunk<'static> {
    let header = crate::export::synth::key_list(16, 0, Vec::new()).children().iter().filter(|c| c.is(b"lhd3")).cloned().collect();
    Chunk::list(*b"Gide", vec![Chunk::data(*b"gdta", vec![0; 8]), Chunk::list(*b"list", header)])
}

/// `cdta` with the bytes `emit::build_comp` does not write: 0..3, the shutter angle and phase
/// limits (360 at 176 and 184), the motion blur samples per frame (16 at 200) and the adaptive
/// sample limit (128 at 196); the same in every sample, and a `cdta` without them crashes After
/// Effects (read-back t-zcdta).
fn cdta() -> Chunk<'static> {
    Chunk::data(
        *b"cdta",
        bytes(
            204,
            &[(0, &[0, 1, 0, 1]), (176, &360u32.to_be_bytes()), (184, &360u32.to_be_bytes()), (196, &128u32.to_be_bytes()), (200, &16u32.to_be_bytes())],
        ),
    )
}

/// The renderer (`PRin`): the Classic 3D match name and display name (aep-format.md 3.2).
fn renderer() -> Chunk<'static> {
    let prin = bytes(104, &[(4, b"ADBE Escher"), (52, b"Classic 3D"), (103, &[1])]);
    Chunk::list(*b"PRin", vec![Chunk::data(*b"prin", prin), Chunk::data(*b"prda", vec![0; 12])])
}

/// The built-in view layers of a composition: list type, name, `ldta` byte 61 (label) and byte
/// 128 (layer type: 2 camera, 4 for the markers layer).
const VIEWS: [(&[u8; 4], &str, u8, u8); 11] = [
    (b"DLay", "Default", 4, 2),
    (b"SLay", "Front", 4, 2),
    (b"SLay", "Left", 4, 2),
    (b"SLay", "Top", 4, 2),
    (b"SLay", "Back", 4, 2),
    (b"SLay", "Right", 4, 2),
    (b"SLay", "Bottom", 4, 2),
    (b"CLay", "Custom View 1", 4, 2),
    (b"CLay", "Custom View 2", 4, 2),
    (b"CLay", "Custom View 3", 4, 2),
    (b"SecL", "Markers", 8, 4),
];

/// One view layer: `ldta` with the decoded fields only (read-back t-zldta), an empty property
/// tree (t-vempty), the guides, `Ewst` and the two f-record sets. `emit` writes the id.
fn view_layer(ty: &[u8; 4], name: &str, label: u8, kind: u8) -> Vec<Chunk<'static>> {
    let markers = kind == 4;
    let mut name_copy = name.as_bytes().to_vec();
    name_copy.truncate(31);
    let mut ldta = bytes(
        164,
        &[
            (8, &1u32.to_be_bytes()),
            (16, &600u32.to_be_bytes()),
            (24, &25_600u32.to_be_bytes()),
            (28, &VIEW_OUT.to_be_bytes()),
            (32, &25_600u32.to_be_bytes()),
            (61, &[label]),
            (64, &name_copy),
            (108, &1u32.to_be_bytes()),
            (128, &u32::from(kind).to_be_bytes()),
        ],
    );
    // Byte 38: 3D and auto-orient on the cameras; byte 39: the video switch (and on the markers
    // layer also effects, locked and collapse); quality Draft and blend mode Normal on the
    // markers layer.
    let flags: &[(usize, u8)] = if markers { &[(5, 2), (39, 0x87), (99, 2)] } else { &[(38, 0x44), (39, 1)] };
    for (at, v) in flags {
        if let Some(b) = ldta.get_mut(*at) {
            *b = *v;
        }
    }
    let root = vec![Chunk::data(*b"tdsb", 1u32.to_be_bytes().to_vec()), tdsn(""), tdmn("ADBE Group End")];
    let layer = Chunk::list(*ty, vec![Chunk::data(*b"ldta", ldta), utf8(name), Chunk::list(*b"tdgp", root), guides()]);
    let mut out = vec![layer, Chunk::list(*b"Ewst", Vec::new())];
    out.extend(crate::export::skeleton::f_records());
    out.extend(crate::export::skeleton::f_records());
    out
}

/// The composition item without layers, and the index of its children where the layer blocks go.
pub(crate) fn comp_shell() -> (Chunk<'static>, usize) {
    let mut kids = vec![
        Chunk::data(*b"iide", vec![0; 4]),
        Chunk::data(*b"idpc", vec![0; 8]),
        idta(4, 0x20),
        utf8(""),
        Chunk::list(*b"dats", vec![Chunk::data(*b"numS", vec![0; 4])]),
        cdta(),
        Chunk::data(*b"cdrp", vec![0]),
        Chunk::data(*b"comr", vec![0]),
        renderer(),
    ];
    let layer_at = kids.len();
    for (ty, name, label, kind) in VIEWS {
        kids.extend(view_layer(ty, name, label, kind));
    }
    kids.push(guides());
    (Chunk::list(*b"Item", kids), layer_at)
}

/// The list that follows a composition item (`FEE `, `ppSn` zero: read-back u-zppsn).
pub(crate) fn fee() -> Chunk<'static> {
    Chunk::list(*b"FEE ", vec![Chunk::data(*b"ppSn", vec![0; 8])])
}

/// An empty folder item (`sfdt` 1 in every sample).
pub(crate) fn folder_item() -> Chunk<'static> {
    Chunk::list(
        *b"Item",
        vec![
            Chunk::data(*b"iide", vec![0; 4]),
            Chunk::data(*b"idpc", vec![0; 8]),
            idta(1, 0),
            utf8(""),
            Chunk::data(*b"sfdt", 1u32.to_be_bytes().to_vec()),
            Chunk::list(*b"Sfdr", Vec::new()),
        ],
    )
}

/// `ftgi` of a footage item, the same in every sample (required, read-back u-noftgi).
pub(crate) fn ftgi() -> Chunk<'static> {
    Chunk::data(*b"ftgi", bytes(16, &[(7, &[1]), (8, &[0xff; 4]), (12, &600u32.to_be_bytes())]))
}

/// The `sspc` bytes of a solid that are not decoded, the same in every solid (000, 022, 211, 259);
/// After Effects refuses a solid with only the decoded fields (read-back u-zsspc).
const SOLID_SSPC: [(usize, &[u8]); 11] = [
    (50, &[0, 0, 0x02, 0x58]),
    (73, &[3]),
    (105, &[1]),
    (109, &[1]),
    (125, &[0x0c]),
    (129, &[1]),
    (134, &[1]),
    (139, &[1]),
    (143, &[1]),
    (188, &[0xff; 8]),
    (212, &[1]),
];

/// A solid's footage item: `sspc` (type `Soli`, duration 0 / 1), `opti` (`Soli`; `emit` writes
/// the colour and the name), `pgui`, the colour-management list (no profile, the defaults of
/// every sample; required, read-back u-noclrs), `ftgi` (required, u-noftgi) and the guides. The
/// solid of a null layer is the same item.
pub(crate) fn solid_item() -> Chunk<'static> {
    let mut sspc_fields: Vec<(usize, &[u8])> = vec![(22, b"Soli"), (45, &[1])];
    sspc_fields.extend(SOLID_SSPC);
    let clrs = Chunk::list(
        *b"CLRS",
        vec![
            Chunk::data(*b"epid", vec![0xff; 16]),
            Chunk::data(*b"apid", vec![0xff; 16]),
            Chunk::data(*b"linl", vec![2, 0, 0, 0]),
            Chunk::data(*b"embp", vec![1]),
            Chunk::data(*b"ipws", vec![1]),
            Chunk::data(*b"dcui", vec![1]),
            Chunk::data(*b"prgb", vec![1]),
            Chunk::data(*b"Mcsp", vec![1]),
            utf8(""),
            Chunk::data(*b"ocsp", vec![1]),
            utf8(""),
            Chunk::data(*b"hdrm", vec![1]),
            utf8("{}"),
        ],
    );
    let pin = Chunk::list(
        *b"Pin ",
        vec![
            Chunk::data(*b"sspc", bytes(222, &sspc_fields)),
            utf8(""),
            Chunk::data(*b"opti", bytes(282, &[(0, b"Soli")])),
            Chunk::data(*b"pgui", vec![0; 16]),
            clrs,
            utf8(""),
        ],
    );
    Chunk::list(*b"Item", vec![Chunk::data(*b"iide", vec![0; 4]), Chunk::data(*b"idpc", vec![0; 8]), idta(7, 0x30), utf8(""), pin, ftgi(), utf8(""), guides()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunks After Effects refuses a file without, or crashes on (aep-export.md, "What After
    /// Effects needs").
    #[test]
    fn the_skeleton_has_what_after_effects_needs() {
        let top = project_chunks();
        for id in [b"adfr", b"qtlg", b"mrid", b"acer", b"fcid", b"oacc", b"head", b"nhed"] {
            assert!(top.iter().any(|c| c.is(id)), "{}", String::from_utf8_lossy(id));
        }
        assert!(top.iter().any(|c| c.is_list(b"CPPl")));
        let head = top.iter().find(|c| c.is(b"head")).and_then(|c| c.bytes()).unwrap();
        assert_eq!(&head[..8], &[0x00, 0x61, 0x00, 0x05, 0x0f, 0x11, 0x0e, 0x02]);
        let rq = top.iter().find(|c| c.is_list(b"LRdr")).unwrap();
        let arsi = rq.child_list(b"LSIf").and_then(|l| l.child(b"ARsi")).and_then(|c| c.bytes()).unwrap();
        assert_eq!(arsi.len(), RQ_SETTINGS_LEN);
        let afsi = top.iter().find(|c| c.is_list(b"LSIf")).and_then(|l| l.child(b"AFsi")).and_then(|c| c.bytes()).unwrap();
        assert_eq!(afsi.len(), RQ_SETTINGS_LEN);

        let (comp, at) = comp_shell();
        let kids = comp.children();
        assert!(comp.child_list(b"dats").is_some(), "no dats: After Effects crashes");
        assert!(kids.iter().any(|c| c.is(b"cdrp")) && kids.iter().any(|c| c.is(b"comr")));
        let cdta = comp.child(b"cdta").and_then(|c| c.bytes()).unwrap();
        for at in [1usize, 3, 179, 187, 199, 203] {
            assert_ne!(cdta[at], 0, "cdta byte {at}: a zero cdta crashes After Effects");
        }
        let views = kids.iter().filter(|c| [*b"DLay", *b"SLay", *b"CLay", *b"SecL"].iter().any(|t| c.is_list(t))).count();
        assert_eq!(views, 11);
        assert!(kids.get(at).is_some_and(|c| c.is_list(b"DLay")), "the layers go before the view layers");

        let solid = solid_item();
        let pin = solid.child_list(b"Pin ").unwrap();
        assert!(pin.child_list(b"CLRS").is_some() && solid.child(b"ftgi").is_some());
        let sspc = pin.child(b"sspc").and_then(|c| c.bytes()).unwrap();
        assert!(SOLID_SSPC.iter().all(|(at, v)| &sspc[*at..*at + v.len()] == *v));
        assert_eq!(solid.child(b"idta").and_then(|c| c.bytes()).unwrap()[23], 0x30);
    }
}
