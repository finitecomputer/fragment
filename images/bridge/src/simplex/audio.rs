//! How long a voice message plays: SimpleX's voice content carries it
//! (`duration`, whole seconds), and its apps show it on the message. Read
//! from the file itself: an MP3's frames (Hermes speaks MP3: its OpenAI and
//! Edge voices) or an MPEG-4 file's movie header (SimpleX's apps record a
//! voice message as MPEG-4 AAC). Anything else, or a file these do not
//! read, is no voice message: it goes as a file (run.rs, `outgoing`).
//!
//! Ours, not a crate's: the two headers are a few dozen lines, and a
//! decoder crate (symphonia) would carry every codec into the bridge to
//! read one number. Pure, and bounded by the bytes given (every step moves
//! forward by at least a header).

/// MP3 frames read, at most: past 30 minutes at any rate (a frame plays 24
/// ms at the shortest, MPEG-2's at 24 kHz), past which the file is no voice
/// message.
const MP3_FRAMES_MAX: u32 = 80_000;
/// Where an MP3's first frame may start after its ID3 tag, at most: past
/// it the bytes are no MP3 this reads.
const MP3_SYNC_WITHIN: usize = 8 * 1024;
/// MPEG-4 boxes read, at most, at the top level and inside `moov`.
const MP4_BOXES_MAX: usize = 64;

/// Whole seconds `bytes` play, to the nearest and at least 1 (SimpleX
/// shows a voice message of none as 0:00), as `media_type` says what they
/// are; `None` for a type that is no voice message here, or bytes it does
/// not read.
pub fn secs(media_type: &str, bytes: &[u8]) -> Option<u32> {
    let ms = match media_type {
        "audio/mpeg" | "audio/mp3" => mp3_ms(bytes)?,
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => mp4_ms(bytes)?,
        _ => return None,
    };
    (ms > 0).then(|| u32::try_from((ms + 500) / 1000).unwrap_or(u32::MAX).max(1))
}

/// An MP3's length, in ms: its frames' samples over its rate (an MPEG-1,
/// 2 or 2.5 Layer III stream, after any ID3v2 tag; an ID3v1 tag or other
/// bytes after its last frame end it). Two frames in a row, or one that
/// ends the file, before any is counted: one sync word alone is no MP3.
fn mp3_ms(b: &[u8]) -> Option<u64> {
    let mut at = 0;
    if b.len() >= 10 && &b[..3] == b"ID3" {
        let size = b[6..10].iter().try_fold(0usize, |n, &x| (x < 0x80).then_some((n << 7) | usize::from(x)))?;
        let footer = if b[5] & 0x10 != 0 { 10 } else { 0 };
        at = 10 + size + footer;
    }
    let start = (at..b.len().min(at + MP3_SYNC_WITHIN)).find(|&i| frame(&b[i..]).is_some_and(|(len, _, _)| b.len() == i + len || frame(&b[i + len..]).is_some()))?;
    let (mut at, mut samples, mut frames, mut rate) = (start, 0u64, 0u32, 0u32);
    // bounded: each pass moves past a frame, at most MP3_FRAMES_MAX
    while let Some((len, n, r)) = frame(&b[at..]) {
        if rate != 0 && r != rate {
            break;
        }
        rate = r;
        samples += u64::from(n);
        frames += 1;
        if frames > MP3_FRAMES_MAX {
            return None;
        }
        at += len;
    }
    (rate > 0).then(|| samples * 1000 / u64::from(rate))
}

/// One MP3 frame's header at the start of `b`: its length in bytes, its
/// samples and its sample rate; `None` when it is none (or the frame runs
/// past `b`).
fn frame(b: &[u8]) -> Option<(usize, u32, u32)> {
    let h = b.get(..4)?;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    // 11: MPEG-1, 10: MPEG-2, 00: MPEG-2.5 (01 is reserved); Layer III (01)
    let version = (h[1] >> 3) & 0b11;
    if version == 0b01 || (h[1] >> 1) & 0b11 != 0b01 {
        return None;
    }
    let mpeg1 = version == 0b11;
    const KBPS_V1: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const KBPS_V2: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    let kbps = if mpeg1 { KBPS_V1 } else { KBPS_V2 }[usize::from(h[2] >> 4)];
    let base = [44_100, 48_000, 32_000, 0][usize::from((h[2] >> 2) & 0b11)];
    if kbps == 0 || base == 0 {
        return None;
    }
    let rate = match version {
        0b11 => base,
        0b10 => base / 2,
        _ => base / 4,
    };
    let samples = if mpeg1 { 1152 } else { 576 };
    let padding = usize::from((h[2] >> 1) & 1);
    let len = (samples / 8 * kbps * 1000 / rate) as usize + padding;
    (len > 4 && len <= b.len()).then_some((len, samples, rate))
}

/// An MPEG-4 file's length, in ms: its movie header's (`moov`'s `mvhd`)
/// duration over its timescale.
fn mp4_ms(b: &[u8]) -> Option<u64> {
    let moov = boxed(b, b"moov")?;
    let mvhd = boxed(moov, b"mvhd")?;
    let (scale, duration) = match *mvhd.first()? {
        0 => (be(mvhd.get(12..16)?), be(mvhd.get(16..20)?)),
        1 => (be(mvhd.get(20..24)?), be(mvhd.get(24..32)?)),
        _ => return None,
    };
    (scale > 0 && duration > 0).then(|| duration.saturating_mul(1000) / scale)
}

/// The body of the first box of `kind` among `b`'s boxes.
fn boxed<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut at = 0;
    for _ in 0..MP4_BOXES_MAX {
        let head = b.get(at..at + 8)?;
        let (size, header) = match be(&head[..4]) {
            0 => ((b.len() - at) as u64, 8),
            1 => (be(b.get(at + 8..at + 16)?), 16),
            n => (n, 8),
        };
        let end = at.checked_add(usize::try_from(size).ok()?).filter(|e| *e <= b.len() && size >= header as u64)?;
        if &head[4..8] == kind {
            return Some(&b[at + header..end]);
        }
        at = end;
    }
    None
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0, |n, &x| (n << 8) | u64::from(x))
}

/// `secs` seconds of silence as an MP3 (MPEG-1 Layer III, 44.1 kHz, mono,
/// 128 kbps; each frame's side information all zero, which every decoder
/// plays as silence): a voice reply for the tests and the scripted agent.
pub fn silence_mp3(secs: u32) -> Vec<u8> {
    // the whole frames within `secs`: 1,152 samples each, at 44.1 kHz
    let frames = (u64::from(secs) * 44_100 / 1152) as usize;
    // a frame is 144 * 128,000 / 44,100 bytes: 417, or 418 padded as an
    // encoder pads, to keep 128 kbps
    let at = |i: usize| i * 144 * 128_000 / 44_100;
    let mut out = Vec::with_capacity(at(frames));
    for i in 0..frames {
        let len = at(i + 1) - at(i);
        out.extend([0xFF, 0xFB, 0x90 | if len == 418 { 0b10 } else { 0 }, 0xC0]);
        out.resize(out.len() + len - 4, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An MPEG-4 file with nothing but a movie header of `scale` and
    /// `duration` (version `v`), after a box of another kind.
    fn mp4(v: u8, scale: u32, duration: u64) -> Vec<u8> {
        let mut mvhd = vec![v, 0, 0, 0];
        if v == 0 {
            mvhd.extend([0; 8]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend((duration as u32).to_be_bytes());
        } else {
            mvhd.extend([0; 16]);
            mvhd.extend(scale.to_be_bytes());
            mvhd.extend(duration.to_be_bytes());
        }
        mvhd.extend([0; 80]);
        let boxed = |kind: &[u8], body: &[u8]| [((body.len() + 8) as u32).to_be_bytes().as_slice(), kind, body].concat();
        [boxed(b"ftyp", b"M4A isomiso2"), boxed(b"moov", &boxed(b"mvhd", &mvhd))].concat()
    }

    /// Goal (valid): a voice message's length is its file's, as SimpleX's
    /// apps show it: an MP3's by its frames (after an ID3 tag, before one at
    /// its end), an MPEG-4 file's by its movie header, rounded up to whole
    /// seconds and never 0.
    #[test]
    fn a_voice_messages_length_is_its_files() {
        for s in [1, 3, 30, 301] {
            assert_eq!(secs("audio/mpeg", &silence_mp3(s)), Some(s), "{s} s of MP3");
        }
        let tagged = [b"ID3\x04\x00\x00\x00\x00\x00\x0a".as_slice(), &[0; 10], &silence_mp3(4), b"TAG", &[0; 125]].concat();
        assert_eq!(secs("audio/mpeg", &tagged), Some(4), "its tags are no frames");
        assert_eq!(secs("audio/mp4", &mp4(0, 44_100, 44_100 * 7)), Some(7));
        assert_eq!(secs("audio/x-m4a", &mp4(1, 1000, 2_500)), Some(3), "to the nearest");
        assert_eq!(secs("audio/x-m4a", &mp4(1, 1000, 2_499)), Some(2));
        assert_eq!(secs("audio/mp4", &mp4(0, 1000, 20)), Some(1), "never 0");
    }

    /// Goal (invalid): what is no voice message here is not read as one: a
    /// type other than MP3 or MPEG-4 audio, bytes that do not read, a sync
    /// word alone, a header that lies about its size, a file too long.
    #[test]
    fn what_is_no_voice_message_is_not_read_as_one() {
        let mp3 = silence_mp3(2);
        for t in ["audio/ogg", "audio/wav", "image/png", "application/octet-stream", ""] {
            assert_eq!(secs(t, &mp3), None, "{t}");
        }
        for bad in [vec![], vec![0xFF, 0xFB, 0x90, 0xC0], b"not an mp3 at all".to_vec(), vec![0xAA; 20_000], mp3[..300].to_vec()] {
            assert_eq!(secs("audio/mpeg", &bad), None, "{} bytes", bad.len());
        }
        let past = [vec![0; MP3_SYNC_WITHIN + 1], mp3.clone()].concat();
        assert_eq!(secs("audio/mpeg", &past), None, "a first frame further in than it looks");
        assert_eq!(secs("audio/mpeg", &silence_mp3(MP3_FRAMES_MAX / 38 + 60)), None, "bounded: past MP3_FRAMES_MAX");
        let mut liar = mp4(0, 1000, 5000);
        liar[3] = 0xFF;
        assert_eq!(secs("audio/mp4", &liar), None, "a box past the end");
        assert_eq!(secs("audio/mp4", &mp4(0, 0, 5000)), None, "no timescale");
        assert_eq!(secs("audio/mp4", &mp4(2, 1000, 5000)), None, "a header of another version");
        assert_eq!(secs("audio/mp4", &mp3), None);
    }
}
