//! The AudioSpecificConfig of the AAC track FilmCraft plays from a movie, from the containers that carry
//! it outside the audio frames: MP4 / MOV (`esds`), Matroska (`CodecPrivate`) and LATM in MPEG
//! TS / PS (its StreamMuxConfig). FilmCraft's other AAC is ADTS (in MPEG TS / PS or a raw `.aac`),
//! whose channel configurations FilmCraft decodes (1 to 7) its channel count tells apart
//! ([`crate::downmix::Order::Aac`]).

use std::io;

use filmcraft_aac::AudioSpecificConfig;
use filmcraft_media::ByteReader;

/// How far into an MPEG TS / PS file the audio streams are looked for: the stream is indexed as
/// far as this, not through the whole file.
const MPEG_HEAD: u64 = 8 << 20;

/// The configuration of the audio track FilmCraft plays, if it is AAC: from an MP4 / MOV `header`
/// (its ftyp and moov, or the whole file) and `file`, or else from `file`, read as Matroska or
/// MPEG TS / PS.
pub(crate) fn config(header: Option<&[u8]>, file: Option<&dyn ByteReader>) -> Option<AudioSpecificConfig> {
    let asc = match header.and_then(|h| mp4(h, file)) {
        Some(asc) => asc,
        None => {
            let file = file?;
            let mut magic = [0u8; 4];
            file.read_at(0, &mut magic).ok()?;
            if magic == [0x1A, 0x45, 0xDF, 0xA3] { matroska(file)? } else { latm(file)? }
        }
    };
    AudioSpecificConfig::parse(&asc).ok()
}

/// The track FilmCraft's MP4 source plays: the first audio track with samples, a sample
/// description and a timescale, read from the whole file as it is (a fragmented movie's samples
/// are in its fragments, not the `moov`).
fn mp4(header: &[u8], file: Option<&dyn ByteReader>) -> Option<Vec<u8>> {
    let mp4 = match file {
        Some(f) => filmcraft_isobmff::open(Head(f, f.len())),
        None => filmcraft_isobmff::open(header),
    };
    let mp4 = mp4.ok()?;
    let playable =
        |t: &&filmcraft_isobmff::Track| t.kind == filmcraft_isobmff::TrackKind::Audio && !t.samples.is_empty() && !t.entries.is_empty() && t.timescale > 0;
    let track = mp4.tracks.iter().find(playable)?;
    match &track.entries.first()?.codec {
        filmcraft_isobmff::CodecConfig::Aac(a) => Some(a.asc.clone()),
        _ => None,
    }
}

/// The track FilmCraft's Matroska source plays: the first audio track with frames, found by
/// indexing the clusters as that source does.
fn matroska(file: &dyn ByteReader) -> Option<Vec<u8>> {
    let mkv = filmcraft_matroska::open(Head(file, file.len())).ok()?;
    match &mkv.tracks.iter().find(|t| t.kind == filmcraft_matroska::TrackKind::Audio && !t.samples.is_empty())?.codec {
        filmcraft_matroska::Codec::Aac { asc } => Some(asc.clone()),
        _ => None,
    }
}

/// The configuration of the stream FilmCraft's MPEG source plays: the first audio stream whose
/// first unit it can read. Only AAC is asked about, so the streams before that one were refused
/// whatever their codec, and an ADTS stream played has no configuration outside its frames.
fn latm(file: &dyn ByteReader) -> Option<Vec<u8>> {
    use filmcraft_mpegts::Codec;
    let head = Head(file, file.len().min(MPEG_HEAD));
    let mpeg = filmcraft_mpegts::open(&head).ok()?;
    let audio = mpeg.streams.iter().enumerate().filter(|(_, s)| s.kind() == filmcraft_mpegts::Kind::Audio && !s.units.is_empty());
    let played = audio.take(filmcraft_media::MAX_AUDIO_STREAMS).find_map(|(i, s)| {
        let first = mpeg.read_unit(&head, i, 0).ok()?;
        match s.codec {
            Codec::AacAdts => filmcraft_mpegts::frame_info(&s.codec, &first).map(|_| None),
            Codec::AacLatm => filmcraft_codecs::audio::latm_config(&first).map(|c| Some(c.asc)),
            _ => None,
        }
    });
    played?
}

/// The first `.1` bytes of a file, as the byte source FilmCraft's demuxers read.
struct Head<'a>(&'a dyn ByteReader, u64);

impl Head<'_> {
    fn read(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        match offset.checked_add(buf.len() as u64) {
            Some(end) if end <= self.1 => self.0.read_at(offset, buf),
            _ => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end")),
        }
    }
}

impl filmcraft_isobmff::ByteSource for Head<'_> {
    fn len(&self) -> u64 {
        self.1
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf)
    }
}

impl filmcraft_matroska::ByteSource for Head<'_> {
    fn len(&self) -> u64 {
        self.1
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf)
    }
}

impl filmcraft_mpegts::ByteSource for Head<'_> {
    fn len(&self) -> u64 {
        self.1
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf)
    }
}
