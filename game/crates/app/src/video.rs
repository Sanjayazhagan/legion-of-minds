//! MP4 video writing with no outside programs: frames are compressed to
//! H.264 by OpenH264 (built into the game) and packed into an .mp4 file
//! that YouTube, Reddit, Instagram and TikTok all take.
//!
//! Frames go to a background thread, so the game keeps drawing while the
//! CPU compresses.

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, VuiConfig};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use openh264::OpenH264API;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::JoinHandle;

pub const FPS: u32 = 30;

/// One frame as read back from the GPU: RGBA rows, bottom row first.
pub struct Frame(pub Vec<u8>);

/// Compresses frames on a background thread and writes the file at `finish`.
pub struct VideoWriter {
    tx: Option<SyncSender<Frame>>,
    worker: Option<JoinHandle<Result<(), String>>>,
    pub frames: u32,
}

impl VideoWriter {
    /// A `w` x `h` video (both even) written to `out` when finished.
    pub fn start(out: PathBuf, w: u32, h: u32) -> Result<VideoWriter, String> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate_for(w, h)))
            .max_frame_rate(FrameRate::from_hz(FPS as f32))
            .rate_control_mode(RateControlMode::Quality)
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(FPS * 2))
            .vui(VuiConfig::bt601());
        let enc = Encoder::with_api_config(OpenH264API::from_source(), config).map_err(|e| format!("video encoder: {e}"))?;
        // A few frames of slack: drawing waits when compressing falls behind.
        let (tx, rx) = sync_channel::<Frame>(4);
        let worker = std::thread::spawn(move || encode_all(enc, rx, &out, w, h));
        Ok(VideoWriter { tx: Some(tx), worker: Some(worker), frames: 0 })
    }

    pub fn push(&mut self, f: Frame) {
        if let Some(tx) = &self.tx {
            if tx.send(f).is_ok() {
                self.frames += 1;
            }
        }
    }

    /// Ends the stream; the file is ready once `wait` returns.
    pub fn finish(mut self) -> JoinHandle<Result<(), String>> {
        drop(self.tx.take());
        self.worker.take().unwrap()
    }
}

/// Plenty for crisp text and little fuzz on fast moving fights.
fn bitrate_for(w: u32, h: u32) -> u32 {
    ((w * h) as f32 * FPS as f32 * 0.16) as u32
}

fn encode_all(mut enc: Encoder, rx: Receiver<Frame>, out: &Path, w: u32, h: u32) -> Result<(), String> {
    let (w, h) = (w as usize, h as usize);
    let mut mp4 = Mp4::new(w as u16, h as u16);
    let mut flipped = vec![0u8; w * h * 4];
    let mut annexb = Vec::new();
    for Frame(rgba) in rx {
        if rgba.len() != w * h * 4 {
            return Err("video: frame has the wrong size".into());
        }
        // GL hands rows over bottom first.
        for y in 0..h {
            let src = &rgba[(h - 1 - y) * w * 4..(h - y) * w * 4];
            flipped[y * w * 4..(y + 1) * w * 4].copy_from_slice(src);
        }
        let yuv = YUVBuffer::from_rgb8_source(RgbaSliceU8::new(&flipped, (w, h)));
        let bits = enc.encode(&yuv).map_err(|e| format!("video encoder: {e}"))?;
        annexb.clear();
        bits.write_vec(&mut annexb);
        mp4.add_annexb(&annexb);
    }
    if mp4.samples.is_empty() {
        return Err("video: no frames".into());
    }
    let tmp = out.with_extension("part");
    std::fs::write(&tmp, mp4.finish()).map_err(|e| format!("could not write {}: {e}", out.display()))?;
    std::fs::rename(&tmp, out).map_err(|e| format!("could not write {}: {e}", out.display()))
}

/// The NAL units in an Annex B stream (split at 00 00 01 start codes).
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
        // A four byte start code leaves one extra zero behind.
        while e > s && data[e - 1] == 0 && k + 1 < starts.len() {
            e -= 1;
        }
        if e > s {
            out.push(&data[s..e]);
        }
    }
    out
}

/// An in-memory MP4 with one H.264 video track. Clips are short (well under
/// 100 MB), so everything is kept until the end and written with the index
/// first, which lets players start a video before it has fully downloaded.
pub struct Mp4 {
    w: u16,
    h: u16,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    data: Vec<u8>,
    /// (size, key frame?) per frame.
    samples: Vec<(u32, bool)>,
}

impl Mp4 {
    pub fn new(w: u16, h: u16) -> Mp4 {
        Mp4 { w, h, sps: None, pps: None, data: Vec::new(), samples: Vec::new() }
    }

    /// One encoded frame, as an Annex B byte stream.
    pub fn add_annexb(&mut self, frame: &[u8]) {
        let mut size = 0u32;
        let mut key = false;
        for nal in nal_units(frame) {
            match nal[0] & 0x1f {
                7 => self.sps.get_or_insert_with(|| nal.to_vec()),
                8 => self.pps.get_or_insert_with(|| nal.to_vec()),
                // Access unit delimiters are not used in MP4.
                9 => continue,
                t => {
                    key |= t == 5;
                    self.data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    self.data.extend_from_slice(nal);
                    size += 4 + nal.len() as u32;
                    continue;
                }
            };
        }
        self.samples.push((size, key));
    }

    pub fn finish(self) -> Vec<u8> {
        let n = self.samples.len() as u32;
        let ms = (n as u64 * 1000 / FPS as u64) as u32;
        let sps = self.sps.clone().unwrap_or_default();
        let pps = self.pps.clone().unwrap_or_default();

        let ftyp = boxed(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", b"avc1", b"mp41"].concat());

        // Builds moov for a given mdat payload offset (its size does not
        // depend on the offset, so build twice).
        let moov_at = |offset: u32| -> Vec<u8> {
            let mut avcc = vec![1, sps.get(1).copied().unwrap_or(66), sps.get(2).copied().unwrap_or(0), sps.get(3).copied().unwrap_or(40), 0xff, 0xe1];
            avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(&sps);
            avcc.push(1);
            avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(&pps);

            let mut avc1 = vec![0u8; 6];
            avc1.extend_from_slice(&1u16.to_be_bytes()); // data reference
            avc1.extend_from_slice(&[0u8; 16]);
            avc1.extend_from_slice(&self.w.to_be_bytes());
            avc1.extend_from_slice(&self.h.to_be_bytes());
            avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            avc1.extend_from_slice(&0u32.to_be_bytes());
            avc1.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
            avc1.extend_from_slice(&[0u8; 32]); // compressor name
            avc1.extend_from_slice(&0x0018u16.to_be_bytes());
            avc1.extend_from_slice(&0xffffu16.to_be_bytes());
            avc1.extend_from_slice(&boxed(b"avcC", &avcc));
            let stsd = full(b"stsd", 0, &[&1u32.to_be_bytes()[..], &boxed(b"avc1", &avc1)].concat());

            let stts = full(b"stts", 0, &[1u32.to_be_bytes(), n.to_be_bytes(), 1u32.to_be_bytes()].concat());
            let keys: Vec<u32> = self.samples.iter().enumerate().filter(|(_, s)| s.1).map(|(i, _)| i as u32 + 1).collect();
            let mut stss = (keys.len() as u32).to_be_bytes().to_vec();
            for k in &keys {
                stss.extend_from_slice(&k.to_be_bytes());
            }
            let stss = full(b"stss", 0, &stss);
            let stsc = full(b"stsc", 0, &[1u32.to_be_bytes(), 1u32.to_be_bytes(), n.to_be_bytes(), 1u32.to_be_bytes()].concat());
            let mut stsz = [0u32.to_be_bytes(), n.to_be_bytes()].concat();
            for s in &self.samples {
                stsz.extend_from_slice(&s.0.to_be_bytes());
            }
            let stsz = full(b"stsz", 0, &stsz);
            let stco = full(b"stco", 0, &[1u32.to_be_bytes(), offset.to_be_bytes()].concat());
            let stbl = boxed(b"stbl", &[stsd, stts, stss, stsc, stsz, stco].concat());

            let vmhd = full(b"vmhd", 1, &[0u8; 8]);
            let dref = full(b"dref", 0, &[&1u32.to_be_bytes()[..], &full(b"url ", 1, &[])].concat());
            let dinf = boxed(b"dinf", &dref);
            let minf = boxed(b"minf", &[vmhd, dinf, stbl].concat());

            let mut mdhd = vec![0u8; 8]; // created, modified
            mdhd.extend_from_slice(&FPS.to_be_bytes());
            mdhd.extend_from_slice(&n.to_be_bytes());
            mdhd.extend_from_slice(&0x55c4u16.to_be_bytes()); // language "und"
            mdhd.extend_from_slice(&0u16.to_be_bytes());
            let mdhd = full(b"mdhd", 0, &mdhd);
            let mut hdlr = vec![0u8; 4];
            hdlr.extend_from_slice(b"vide");
            hdlr.extend_from_slice(&[0u8; 12]);
            hdlr.extend_from_slice(b"Legion of Minds video\0");
            let hdlr = full(b"hdlr", 0, &hdlr);
            let mdia = boxed(b"mdia", &[mdhd, hdlr, minf].concat());

            let mut tkhd = vec![0u8; 8];
            tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
            tkhd.extend_from_slice(&[0u8; 4]);
            tkhd.extend_from_slice(&ms.to_be_bytes());
            tkhd.extend_from_slice(&[0u8; 8]);
            tkhd.extend_from_slice(&[0u8; 8]); // layer, group, volume, reserved
            tkhd.extend_from_slice(&MATRIX);
            tkhd.extend_from_slice(&((self.w as u32) << 16).to_be_bytes());
            tkhd.extend_from_slice(&((self.h as u32) << 16).to_be_bytes());
            let tkhd = full(b"tkhd", 3, &tkhd);
            let trak = boxed(b"trak", &[tkhd, mdia].concat());

            let mut mvhd = vec![0u8; 8];
            mvhd.extend_from_slice(&1000u32.to_be_bytes());
            mvhd.extend_from_slice(&ms.to_be_bytes());
            mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
            mvhd.extend_from_slice(&[0u8; 10]);
            mvhd.extend_from_slice(&MATRIX);
            mvhd.extend_from_slice(&[0u8; 24]);
            mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id
            let mvhd = full(b"mvhd", 0, &mvhd);
            boxed(b"moov", &[mvhd, trak].concat())
        };
        let moov_len = moov_at(0).len() as u32;
        let offset = ftyp.len() as u32 + moov_len + 8;
        let moov = moov_at(offset);
        let mut out = Vec::with_capacity(offset as usize + self.data.len());
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        out.extend_from_slice(&(self.data.len() as u32 + 8).to_be_bytes());
        out.extend_from_slice(b"mdat");
        out.extend_from_slice(&self.data);
        out
    }
}

const MATRIX: [u8; 36] = {
    let mut m = [0u8; 36];
    m[1] = 1; // 0x00010000
    m[17] = 1; // 0x00010000
    m[32] = 0x40; // 0x40000000
    m
};

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full(kind: &[u8; 4], flags: u32, body: &[u8]) -> Vec<u8> {
    boxed(kind, &[&(flags & 0x00ff_ffff).to_be_bytes()[..], body].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annexb_into_nal_units() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = nal_units(&s);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
    }

    #[test]
    fn writes_a_playable_video() {
        let (w, h) = (320u32, 240u32);
        let dir = std::env::temp_dir().join(format!("lom-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("test.mp4");
        let mut vw = VideoWriter::start(out.clone(), w, h).unwrap();
        for f in 0..45u32 {
            let mut px = vec![0u8; (w * h * 4) as usize];
            for (i, p) in px.chunks_mut(4).enumerate() {
                p.copy_from_slice(&[(i as u32 + f * 5) as u8, 128, (f * 4) as u8, 255]);
            }
            vw.push(Frame(px));
        }
        vw.finish().join().unwrap().unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert!(bytes.windows(4).any(|x| x == b"avcC"));
        if std::env::var_os("BAG_KEEP_TEST_VIDEO").is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        } else {
            println!("video at {}", out.display());
        }
    }
}
