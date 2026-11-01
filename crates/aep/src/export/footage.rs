//! Footage items (files), built in code per kind ([`FtProtos::build`]) with the layout of the items
//! After Effects writes when it imports a file of that kind: png still 607, jpeg 605, OpenEXR 604,
//! TIFF 613, H.264 mp4 606, QuickTime mov 612, WAV 351, png sequence 353, OpenEXR sequence 603,
//! jpeg sequence 611, Targa, Photoshop, Illustrator and EPS 650-653. The colour management names
//! our own sRGB profile (`icc.rs`). The exporter then writes the decoded fields
//! (`docs/aep-format.md` 3.3, `docs/aep-export.md` R28):
//!
//! * the alias JSON (`Als2/alas`): the full path, for a sequence its folder with
//!   `target_is_folder`;
//! * `sspc`: width, height, duration (`num / den`), native frame rate, for sequences the first and
//!   last frame number and the digits, and the folder's modification time when the caller gives
//!   it ([`crate::export::export_with`]); the two `Utf8` around the number;
//! * `opti` of png: width, height and the first file's name;
//! * the item header: id, label, name; a fresh `pgui`.
//!
//! After Effects reads the format options of the other kinds from the file (their `opti` beyond
//! the header can be zero: read-backs v-fN-zoptiA).

use effectcraft_project::{Footage, FootageKind};

use crate::export::report::{Collector, LossCategory};

/// The prototype a footage item is cloned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FtKind {
    Png,
    Jpeg,
    Exr,
    Mp4,
    Wav,
    PngSeq,
    ExrSeq,
    JpegSeq,
    Mov,
    Tiff,
    Tga,
    Psd,
    Ai,
    Eps,
}

impl FtKind {
    /// The key a kind's prototype is found under.
    pub(crate) fn key(self) -> [u8; 4] {
        match self {
            FtKind::Png => *b"png ",
            FtKind::Jpeg => *b"jpg ",
            FtKind::Exr => *b"exr ",
            FtKind::Mp4 => *b"mp4 ",
            FtKind::Wav => *b"wav ",
            FtKind::PngSeq => *b"pngS",
            FtKind::ExrSeq => *b"exrS",
            FtKind::JpegSeq => *b"jpgS",
            FtKind::Mov => *b"mov ",
            FtKind::Tiff => *b"tif ",
            FtKind::Tga => *b"tga ",
            FtKind::Psd => *b"psd ",
            FtKind::Ai => *b"ai  ",
            FtKind::Eps => *b"eps ",
        }
    }

    /// The denominator of a still's duration (`sspc@42`: png 1, jpeg 600, OpenEXR 100).
    fn still_den(self) -> u32 {
        match self {
            FtKind::Jpeg | FtKind::Tiff | FtKind::Tga | FtKind::Psd | FtKind::Ai | FtKind::Eps => 600,
            FtKind::Exr => 100,
            _ => 1,
        }
    }

    pub(crate) fn is_sequence(self) -> bool {
        matches!(self, FtKind::PngSeq | FtKind::ExrSeq | FtKind::JpegSeq)
    }
}

/// The numbering of a sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Numbering {
    pub prefix: String,
    pub suffix: String,
    pub start: u32,
    pub end: u32,
    pub digits: u32,
}

/// A footage item in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FootagePlan {
    pub kind: FtKind,
    /// The alias target: the file, or a sequence's folder.
    pub alias: String,
    /// The (first) file's name.
    pub file_name: String,
    pub width: u32,
    pub height: u32,
    /// Duration as stored, `num / den` seconds.
    pub duration: (u32, u32),
    /// Native frame rate (sequences and clips), 0 otherwise.
    pub fps: u32,
    /// The rate a clip is conformed to (`sspc@146`, also the effective rate at @150; 645), 0 when
    /// it plays at its native rate.
    pub conform: u32,
    pub numbering: Option<Numbering>,
    /// The duration in seconds (the Time Remapping maximum of a layer showing it).
    pub seconds: f64,
    /// A sequence's folder modification time as stored (`sspc@118`, seconds since 1904, UTC; 860),
    /// when the caller supplied it ([`crate::export::export_with`]).
    pub folder_time: Option<u32>,
}

/// Seconds between 1904-01-01 (the stored time's epoch) and 1970-01-01.
const SECONDS_1904_TO_1970: u64 = 2_082_844_800;

/// Fills in the folder time of every image sequence in `root` (folders included) from
/// `folder_time` (seconds since 1970); a time that does not fit the stored field is left out.
pub(crate) fn stamp_sequences(root: &mut [crate::export::plan::Entry], folder_time: &dyn Fn(&str) -> Option<u64>) {
    use crate::export::plan::ItemPlan;
    // Folders were planned with a bounded depth; walk them with an explicit stack anyway.
    let mut stack: Vec<&mut [crate::export::plan::Entry]> = vec![root];
    while let Some(entries) = stack.pop() {
        for e in entries.iter_mut() {
            match &mut e.plan {
                ItemPlan::Folder(kids) => stack.push(kids),
                ItemPlan::Footage(fp) if fp.kind.is_sequence() => {
                    fp.folder_time = folder_time(&fp.alias).and_then(|t| u32::try_from(t.checked_add(SECONDS_1904_TO_1970)?).ok());
                }
                _ => {}
            }
        }
    }
}

fn ext(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => String::new(),
    }
}

fn split_dir(path: &str) -> (&str, &str) {
    match path.rfind(['\\', '/']) {
        Some(i) => (path.get(..i).unwrap_or_default(), path.get(i + 1..).unwrap_or_default()),
        None => ("", path),
    }
}

/// `shot_0101.exr` -> (`shot_`, 101, 4, `.exr`): the last run of digits before the extension.
fn parse_numbered(name: &str) -> Option<(String, u32, u32, String)> {
    let dot = name.rfind('.').unwrap_or(name.len());
    let stem = name.get(..dot)?;
    let digits = stem.chars().rev().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let split = stem.len().checked_sub(digits)?;
    let number: u32 = stem.get(split..)?.parse().ok()?;
    Some((stem.get(..split)?.to_string(), number, u32::try_from(digits).ok()?, name.get(dot..)?.to_string()))
}

/// Whole frames at an integer rate, or `None` (a rate After Effects' footage header stores as an
/// integer: 30 in 353 and 603, 25 in 606).
fn integer_rate(f: &Footage) -> Option<u32> {
    let r = f.frame_rate;
    (r.den == 1 && r.num > 0).then(|| u32::try_from(r.num).ok()).flatten()
}

/// Plans one footage item, or reports why it cannot be written.
pub(crate) fn plan_footage(name: &str, f: &Footage, col: &mut Collector) -> Option<FootagePlan> {
    let what = |why: &str| format!("footage '{name}': {why}");
    if f.layer.is_some() {
        col.lose(LossCategory::FootageItems, what("one layer of a layered file is not exported"));
        return None;
    }
    let e = ext(&f.path);
    let kind = match (f.kind, e.as_str()) {
        (FootageKind::Still, "png") => FtKind::Png,
        (FootageKind::Still, "jpg" | "jpeg") => FtKind::Jpeg,
        (FootageKind::Still, "exr") => FtKind::Exr,
        (FootageKind::Still, "tif" | "tiff") => FtKind::Tiff,
        (FootageKind::Still, "tga") => FtKind::Tga,
        (FootageKind::Still, "psd") => FtKind::Psd,
        (FootageKind::Still, "ai") => FtKind::Ai,
        (FootageKind::Still, "eps") => FtKind::Eps,
        (FootageKind::Sequence, "jpg" | "jpeg") => FtKind::JpegSeq,
        (FootageKind::Video, "mov") => FtKind::Mov,
        (FootageKind::Sequence, "png") => FtKind::PngSeq,
        (FootageKind::Sequence, "exr") => FtKind::ExrSeq,
        (FootageKind::Video, "mp4" | "m4v") => FtKind::Mp4,
        (FootageKind::Audio, "wav") => FtKind::Wav,
        _ => {
            col.lose(LossCategory::FootageItems, what(&format!("{:?} footage of type '.{e}' is not exported (no prototype)", f.kind)));
            return None;
        }
    };
    if f.path.is_empty() || f.path.chars().any(char::is_control) {
        col.lose(LossCategory::FootageItems, what("no usable path"));
        return None;
    }
    if f.width > crate::export::plan::MAX_SIDE || f.height > crate::export::plan::MAX_SIDE {
        col.lose(
            LossCategory::FootageItems,
            what(&format!("size {} x {} is larger than {}, written smaller", f.width, f.height, crate::export::plan::MAX_SIDE)),
        );
    }
    let width = f.width.min(crate::export::plan::MAX_SIDE);
    let height = f.height.min(crate::export::plan::MAX_SIDE);
    let (dir, file) = split_dir(&f.path);
    let mut plan = FootagePlan {
        kind,
        alias: f.path.clone(),
        file_name: file.to_string(),
        width,
        height,
        duration: (0, kind.still_den()),
        fps: 0,
        conform: 0,
        numbering: None,
        seconds: 0.0,
        folder_time: None,
    };
    match kind {
        FtKind::PngSeq | FtKind::ExrSeq | FtKind::JpegSeq => {
            let Some(fps) = integer_rate(f) else {
                col.lose(
                    LossCategory::FootageItems,
                    what(&format!("a sequence at {} fps is not exported (only whole rates are sampled)", f.frame_rate.label())),
                );
                return None;
            };
            let Some((prefix, start, digits, suffix)) = parse_numbered(file) else {
                col.lose(LossCategory::FootageItems, what("a sequence whose first file has no frame number"));
                return None;
            };
            // The files must be the run start, start + 1, ... with the same name around the number.
            let files: Vec<&str> = if f.sequence.is_empty() { vec![file] } else { f.sequence.iter().map(|p| split_dir(p).1).collect() };
            let mut expect = start;
            for name in &files {
                let ok = parse_numbered(name).is_some_and(|(p, n, _, s)| p == prefix && s == suffix && n == expect);
                if !ok {
                    col.lose(LossCategory::FootageItems, what("a sequence with gaps or mixed names is not exported"));
                    return None;
                }
                expect = expect.saturating_add(1);
            }
            let count = u32::try_from(files.len()).unwrap_or(u32::MAX);
            plan.alias = dir.to_string();
            plan.fps = fps;
            plan.duration = (count, fps);
            plan.seconds = f64::from(count) / f64::from(fps.max(1));
            plan.numbering = Some(Numbering { prefix, suffix, start, end: start.saturating_add(count.saturating_sub(1)), digits });
            if dir.is_empty() {
                col.lose(LossCategory::FootageItems, what("a sequence without a folder"));
                return None;
            }
        }
        FtKind::Mp4 | FtKind::Mov => {
            let Some(fps) = integer_rate(f) else {
                col.lose(LossCategory::FootageItems, what(&format!("a clip at {} fps is not exported (only whole rates are sampled)", f.frame_rate.label())));
                return None;
            };
            // A conformed clip (645): the frames play at `fps`, the stored duration stays in the
            // native rate's frames, and the conformed rate goes to @146 and @150.
            let native = match f.native_rate {
                Some(n) if n != f.frame_rate => {
                    if n.den == 1 && n.num > 0 && n.num <= i64::from(u32::MAX) {
                        u32::try_from(n.num).ok()
                    } else {
                        col.lose(
                            LossCategory::FootageItems,
                            what(&format!("a clip with the native rate {} fps is written at its conformed rate only", n.label())),
                        );
                        None
                    }
                }
                _ => None,
            };
            let frames = (f.duration.seconds() * f64::from(fps)).round();
            let frames = if frames.is_finite() && frames >= 1.0 && frames < f64::from(u32::MAX) { frames as u32 } else { 1 };
            plan.seconds = f64::from(frames) / f64::from(fps.max(1));
            match native {
                Some(n) => {
                    plan.fps = n;
                    plan.conform = fps;
                    plan.duration = (frames, n);
                }
                None => {
                    plan.fps = fps;
                    plan.duration = (frames, fps);
                }
            }
        }
        FtKind::Wav => {
            // The sample rate is the denominator (351: 48 000 / 48 000 for 1 s).
            let secs = f.duration.seconds();
            let samples = (secs * 48_000.0).round();
            let samples = if samples.is_finite() && samples >= 1.0 && samples < f64::from(u32::MAX) { samples as u32 } else { 48_000 };
            plan.duration = (samples, 48_000);
            plan.seconds = f64::from(samples) / 48_000.0;
        }
        FtKind::Png | FtKind::Jpeg | FtKind::Exr | FtKind::Tiff | FtKind::Tga | FtKind::Psd | FtKind::Ai | FtKind::Eps => {}
    }
    col.exported.footage = col.exported.footage.saturating_add(1);
    Some(plan)
}

/// The alias JSON After Effects reads (`Als2/alas`, 126, 351, 353, 603), with the path escaped.
pub(crate) fn alias_json(p: &FootagePlan) -> Vec<u8> {
    let mut path = String::with_capacity(p.alias.len() + 8);
    for c in p.alias.chars() {
        match c {
            '"' => path.push_str("\\\""),
            '\\' => path.push_str("\\\\"),
            c if (c as u32) < 0x20 => path.push_str(&format!("\\u{:04x}", c as u32)),
            c => path.push(c),
        }
    }
    let folder = p.kind.is_sequence();
    format!(
        "{{\"ascendcount_base\":1,\"ascendcount_target\":{},\"fullpath\":\"{path}\",\"platform\":1,\"server_name\":\"\",\"server_volume_name\":\"\",\"target_is_folder\":{folder}}}",
        if folder { 2 } else { 1 }
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------------------------
// Emit.

use crate::Chunk;

use crate::export::tree::{R, child_data, kids_mut, list_mut, put, put_cstr, put_u8, put_u16, put_u32, tpl};

/// The footage prototypes: per kind the item and the f-records that follow it.
pub(crate) struct FtProtos {
    items: Vec<([u8; 4], Vec<Chunk<'static>>)>,
}

/// Every kind the exporter writes.
const KINDS: [FtKind; 14] = [
    FtKind::Png,
    FtKind::Jpeg,
    FtKind::Exr,
    FtKind::Mp4,
    FtKind::Wav,
    FtKind::PngSeq,
    FtKind::ExrSeq,
    FtKind::JpegSeq,
    FtKind::Mov,
    FtKind::Tiff,
    FtKind::Tga,
    FtKind::Psd,
    FtKind::Ai,
    FtKind::Eps,
];

/// The colour management a kind's item carries (`Pin /CLRS`, 351, 353, 603-607, 611-613, 650-653).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clrs {
    /// Stills, sequences and audio: interpreted in the sRGB profile (`apid` its id).
    Assigned,
    /// A file with an embedded profile (TIFF, Photoshop): `epid` its id, the profile twice.
    Embedded,
    /// Video and JPEG: Rec. 709 with the codec's colour description.
    Video(&'static str),
}

/// The colour description After Effects gives a JPEG, a jpeg sequence and a ProRes clip.
const BT709_FULL: &str = r#"{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}"#;
/// The colour description of an H.264 clip.
const BT709_DISPLAY: &str =
    r#"{"baseColorProfile":{"colorProfileData":"AQAAAP////8=","colorProfileName":"BT.709,8-bit,Display-Referred"},"baseProfileType":1}"#;

/// What a kind's item stores beyond the decoded fields: the file type tag (`sspc` 22, `opti` 0),
/// `idta` byte 23, the bits per pixel of the sampled file (`sspc` 63), `sspc` byte 73 (3, 1 for
/// OpenEXR, 0 for Illustrator and EPS), the `opti` size and its header bytes 4, 5 and 10 (the rest of
/// `opti` can be zero: read-back v-fN-zoptiA), and the colour management.
struct KindSpec {
    tag: [u8; 4],
    idta23: u8,
    bpp: u8,
    b73: u8,
    opti: (u16, u8, u8, u8),
    clrs: Clrs,
}

fn spec(kind: FtKind) -> KindSpec {
    let (tag, idta23, bpp, b73, opti, clrs) = match kind {
        FtKind::Png => (*b"png!", 0x30, 24, 3, (322, 0, 1, 1), Clrs::Assigned),
        FtKind::Jpeg => (*b"ZPEG", 0x30, 24, 3, (58, 0, 5, 0), Clrs::Video(BT709_FULL)),
        FtKind::Exr => (*b"oEXR", 0x30, 128, 1, (9750, 0, 1, 1), Clrs::Assigned),
        FtKind::Mp4 => (*b"XCEX", 0x20, 24, 3, (58, 0, 5, 0), Clrs::Video(BT709_DISPLAY)),
        FtKind::Wav => (*b"WAVE", 0x04, 0, 3, (58, 0, 5, 0), Clrs::Assigned),
        FtKind::PngSeq => (*b"png!", 0x20, 24, 3, (322, 0, 1, 1), Clrs::Assigned),
        FtKind::ExrSeq => (*b"oEXR", 0x20, 128, 1, (9750, 0, 1, 1), Clrs::Assigned),
        FtKind::JpegSeq => (*b"ZPEG", 0x20, 24, 3, (58, 0, 5, 0), Clrs::Video(BT709_FULL)),
        FtKind::Mov => (*b"MOoV", 0x20, 48, 3, (58, 0, 5, 0), Clrs::Video(BT709_FULL)),
        FtKind::Tiff => (*b"TIF ", 0x30, 24, 3, (602, 1, 9, 0), Clrs::Embedded),
        FtKind::Tga => (*b"TPIC", 0x30, 24, 3, (30, 0, 46, 5), Clrs::Assigned),
        FtKind::Psd => (*b"8BPS", 0x30, 24, 3, (602, 1, 9, 0), Clrs::Embedded),
        FtKind::Ai | FtKind::Eps => (*b"TEXT", 0x30, 32, 0, (596, 0, 8, 0), Clrs::Assigned),
    };
    KindSpec { tag, idta23, bpp, b73, opti, clrs }
}

/// The colour-management list of a kind (the order After Effects writes).
fn clrs(kind: Clrs) -> Chunk<'static> {
    use crate::export::base::utf8;
    let icc = crate::export::icc::srgb();
    let id = crate::export::icc::profile_id(&icc).to_vec();
    let json = crate::export::icc::profile_json(&icc, crate::export::icc::SRGB_NAME);
    let none = vec![0xffu8; 16];
    let one = |id: &[u8; 4]| Chunk::data(*id, vec![1u8]);
    let tail = |kids: &mut Vec<Chunk<'static>>| {
        kids.push(one(b"hdrm"));
        kids.push(utf8("{}"));
    };
    let mut kids = match kind {
        Clrs::Assigned => vec![
            Chunk::data(*b"epid", none),
            Chunk::data(*b"apid", id),
            Chunk::data(*b"linl", vec![2, 0, 0, 0]),
            one(b"embp"),
            Chunk::data(*b"ipws", vec![0]),
            one(b"Mcsp"),
            utf8(""),
            one(b"ocsp"),
            utf8(&json),
        ],
        Clrs::Embedded => vec![
            Chunk::data(*b"epid", id),
            Chunk::data(*b"apid", none),
            Chunk::data(*b"linl", vec![2, 0, 0, 0]),
            one(b"embp"),
            Chunk::data(*b"ipws", vec![0]),
            one(b"mcsp"),
            utf8(&json),
            one(b"Mcsp"),
            utf8(&json),
            one(b"ocsp"),
            utf8(""),
        ],
        Clrs::Video(desc) => vec![
            Chunk::data(*b"epid", none.clone()),
            Chunk::data(*b"apid", none),
            one(b"empd"),
            utf8("Rec. 709"),
            Chunk::data(*b"linl", vec![0; 4]),
            one(b"embp"),
            Chunk::data(*b"ipws", vec![0]),
            one(b"mcsp"),
            utf8(desc),
            one(b"Mcsp"),
            utf8(desc),
            one(b"ocsp"),
            utf8(""),
        ],
    };
    tail(&mut kids);
    Chunk::list(*b"CLRS", kids)
}

/// The item of a kind with zero ids and an empty alias (`build_footage` fills them), and the
/// f-records after it. `sspc` holds the tag, the bits per pixel, the time base of a still (600:
/// a clip's is its rate, written per item) and the bytes every sample of its kind has (125, 129,
/// 139, 143, 188..195; a sequence also 168, 184, 186, 187; a WAV its sample rate at 160); the bytes
/// left zero were zero in read-backs v-fN-zsspc1..3. `mnfo` is left out (v-fN-nomnfo).
fn kind_item(kind: FtKind) -> Vec<Chunk<'static>> {
    use crate::export::base::{bytes, utf8};
    let s = spec(kind);
    let mut fields: Vec<(usize, Vec<u8>)> = vec![
        (22, s.tag.to_vec()),
        (50, 600u32.to_be_bytes().to_vec()),
        (63, vec![s.bpp]),
        (73, vec![s.b73]),
        (125, vec![0x0c]),
        (129, vec![1]),
        (139, vec![1]),
        (143, vec![1]),
        (188, vec![0xff; 8]),
    ];
    if kind.is_sequence() {
        fields.extend([(168, 2u32.to_be_bytes().to_vec()), (184, vec![1]), (186, vec![1, 1])]);
    }
    if kind == FtKind::Wav {
        fields.push((160, 48_000f64.to_be_bytes().to_vec()));
    }
    let refs: Vec<(usize, &[u8])> = fields.iter().map(|(at, v)| (*at, v.as_slice())).collect();
    let (len, b4, b5, b10) = s.opti;
    let opti = bytes(usize::from(len), &[(0, &s.tag), (4, &[b4, b5]), (8, &len.to_be_bytes()), (10, &[b10])]);
    let mut pin = vec![Chunk::data(*b"sspc", bytes(222, &refs)), utf8(""), Chunk::list(*b"Als2", vec![Chunk::data(*b"alas", b"{}".to_vec())])];
    if kind.is_sequence() {
        pin.extend([utf8(""), utf8("")]);
    }
    pin.extend([Chunk::data(*b"opti", opti), Chunk::data(*b"pgui", vec![0; 16]), clrs(s.clrs), utf8("")]);
    let item = Chunk::list(
        *b"Item",
        vec![
            Chunk::data(*b"iide", vec![0; 4]),
            Chunk::data(*b"idpc", vec![0; 8]),
            crate::export::base::idta(7, s.idta23),
            utf8(""),
            Chunk::list(*b"Pin ", pin),
            crate::export::base::ftgi(),
            utf8(""),
            crate::export::base::guides(),
        ],
    );
    let mut out = vec![item];
    out.extend(crate::export::skeleton::f_records());
    out
}

impl FtProtos {
    /// The items of every kind, built in code.
    pub(crate) fn build() -> FtProtos {
        FtProtos { items: KINDS.iter().map(|k| (k.key(), kind_item(*k))).collect() }
    }
}

/// The item (and its f-records) for `p`, with the id, label and name, and a fresh `pgui`.
pub(crate) fn build_footage(protos: &FtProtos, id: u32, label: u8, name: &str, p: &FootagePlan, guid: [u8; 16]) -> R<Vec<Chunk<'static>>> {
    let key = p.kind.key();
    let mut chunks = protos.items.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).ok_or_else(|| tpl("footage prototype missing"))?;
    let item = chunks.get_mut(0).ok_or_else(|| tpl("empty footage prototype"))?;
    put(child_data(item, b"iide")?, 0, &id.to_le_bytes())?;
    {
        let idta = child_data(item, b"idta")?;
        put_u32(idta, 16, id)?;
        put_u8(idta, 58, label)?;
    }
    // After Effects names a footage item after its file and then stores no name of its own (126,
    // 603-607); a name of the user's is the item's `Utf8`. The WAV prototype keeps the name (351).
    let default = match &p.numbering {
        Some(n) => format!("{}[{:0w$}-{:0w$}]{}", n.prefix, n.start, n.end, n.suffix, w = usize::try_from(n.digits.min(16)).unwrap_or(0)),
        None => p.file_name.clone(),
    };
    let stored = if name == default && p.kind != FtKind::Wav { "" } else { name };
    crate::export::tree::set_utf8(item, stored)?;
    let pin = list_mut(item, b"Pin ")?;
    {
        let sspc = child_data(pin, b"sspc")?;
        put_u32(sspc, 30, p.width)?;
        put_u32(sspc, 34, p.height)?;
        put_u32(sspc, 38, p.duration.0)?;
        put_u32(sspc, 42, p.duration.1)?;
        if p.fps > 0 {
            // A clip counts its time in its native rate (606, 612: 25); stills, sequences and
            // audio in 600ths.
            if matches!(p.kind, FtKind::Mp4 | FtKind::Mov) {
                put_u32(sspc, 50, p.fps)?;
            }
            put_u32(sspc, 56, p.fps)?;
            put_u32(sspc, 150, if p.conform > 0 { p.conform } else { p.fps })?;
        }
        if p.conform > 0 {
            put_u32(sspc, 146, p.conform)?;
        }
        if let Some(n) = &p.numbering {
            put_u32(sspc, 172, n.start)?;
            put_u32(sspc, 176, n.end)?;
            put_u32(sspc, 180, n.digits)?;
        }
        if let (true, Some(t)) = (p.kind.is_sequence(), p.folder_time) {
            put_u32(sspc, 118, t)?;
        }
    }
    {
        let als = list_mut(pin, b"Als2")?;
        let alas = kids_mut(als)?.iter_mut().find(|c| c.is(b"alas")).ok_or_else(|| tpl("footage prototype without alias"))?;
        *alas = Chunk::data(*b"alas", alias_json(p));
    }
    if let Some(n) = &p.numbering {
        // The two Utf8 between the alias and opti: the name around the number (353, 603).
        let kids = kids_mut(pin)?;
        let at = kids.iter().position(|c| c.is_list(b"Als2")).ok_or_else(|| tpl("footage prototype without alias"))?;
        let mut names = [n.prefix.as_str(), n.suffix.as_str()].into_iter();
        for c in kids.iter_mut().skip(at + 1).take_while(|c| !c.is(b"opti")) {
            if c.is(b"Utf8")
                && let Some(text) = names.next()
            {
                *c = Chunk::data(*b"Utf8", text.as_bytes().to_vec());
            }
        }
    }
    if matches!(p.kind, FtKind::Png | FtKind::PngSeq) {
        // png options: width and height (u16 at 20 and 24) and the (first) file's name at 58
        // (126, 353, 607).
        let opti = child_data(pin, b"opti")?;
        put_u16(opti, 20, u16::try_from(p.width).unwrap_or(u16::MAX))?;
        put_u16(opti, 24, u16::try_from(p.height).unwrap_or(u16::MAX))?;
        put_cstr(opti, 58, 256, &p.file_name)?;
    }
    put(child_data(pin, b"pgui")?, 0, &guid)?;
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind's item has the chunks After Effects refuses it without (`CLRS`, `ftgi`: read-backs
    /// u-noclrs, u-noftgi) and the f-records after it; `opti` has its header and its size.
    #[test]
    fn every_kind_has_what_after_effects_needs() {
        let protos = FtProtos::build();
        assert_eq!(protos.items.len(), KINDS.len());
        for (key, chunks) in &protos.items {
            let what = String::from_utf8_lossy(key).into_owned();
            let item = &chunks[0];
            let pin = item.child_list(b"Pin ").unwrap();
            assert!(pin.child_list(b"CLRS").is_some() && item.child(b"ftgi").is_some(), "{what}");
            assert_eq!(chunks.len(), 8, "{what}: the item and seven f-records");
            let opti = pin.child(b"opti").and_then(|c| c.bytes()).unwrap();
            assert_eq!(usize::from(u16::from_be_bytes([opti[8], opti[9]])), opti.len(), "{what}");
            let sspc = pin.child(b"sspc").and_then(|c| c.bytes()).unwrap();
            assert_eq!(&sspc[22..26], &opti[0..4], "{what}");
        }
    }
}
