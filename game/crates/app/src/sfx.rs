//! Sound effects, made by the game itself: every sound is synthesised from
//! noise, sine waves and filters when the game starts (no audio files, no
//! licences), then mixed on our own little mixer so many hits can overlap.
//!
//! While recording a video (`BAG_RECORD`) every sound is also logged against
//! the frame clock and mixed into the video's soundtrack at the end.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

const SR: usize = 44_100;
/// Versions of each sound, picked at random so repeats don't sound copied.
const VARIANTS: usize = 4;
const MAX_VOICES: usize = 40;
const MASTER: f32 = 0.55;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sfx {
    Swing,
    BladeHit,
    BluntHit,
    StoneHit,
    Collapse,
    BowRelease,
    ArrowImpact,
    Cannon,
    Explosion,
    MagicBolt,
    Heal,
    Dodge,
    Block,
    Stun,
    Blink,
    Charge,
    Fall,
    Shimmer,
    Trap,
    Victory,
    Defeat,
    Click,
}

const ALL: [Sfx; 22] = [
    Sfx::Swing,
    Sfx::BladeHit,
    Sfx::BluntHit,
    Sfx::StoneHit,
    Sfx::Collapse,
    Sfx::BowRelease,
    Sfx::ArrowImpact,
    Sfx::Cannon,
    Sfx::Explosion,
    Sfx::MagicBolt,
    Sfx::Heal,
    Sfx::Dodge,
    Sfx::Block,
    Sfx::Stun,
    Sfx::Blink,
    Sfx::Charge,
    Sfx::Fall,
    Sfx::Shimmer,
    Sfx::Trap,
    Sfx::Victory,
    Sfx::Defeat,
    Sfx::Click,
];

impl Sfx {
    /// Plays of the same sound closer together than this are dropped: the
    /// sim can land a dozen hits in one tick.
    fn min_gap(self) -> f64 {
        match self {
            Sfx::Collapse | Sfx::Victory | Sfx::Defeat => 0.3,
            Sfx::Heal | Sfx::Shimmer | Sfx::Stun => 0.12,
            _ => 0.045,
        }
    }

    fn gain(self) -> f32 {
        match self {
            Sfx::Swing | Sfx::Dodge => 0.45,
            Sfx::Click => 0.35,
            Sfx::Heal | Sfx::Shimmer => 0.5,
            Sfx::Victory | Sfx::Defeat => 0.8,
            _ => 0.75,
        }
    }
}

// ------------------------------------------------------------- mixing

struct Voice {
    buf: Arc<Vec<f32>>,
    pos: usize,
    delay: usize,
    gl: f32,
    gr: f32,
}

#[derive(Default)]
struct Mixer {
    voices: Vec<Voice>,
}

impl Mixer {
    fn add(&mut self, v: Voice) {
        if self.voices.len() >= MAX_VOICES {
            // Drop the voice closest to its end.
            if let Some(k) = (0..self.voices.len()).max_by_key(|&k| self.voices[k].pos * 1000 / self.voices[k].buf.len().max(1)) {
                self.voices.swap_remove(k);
            }
        }
        self.voices.push(v);
    }

    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        for v in &mut self.voices {
            for frame in out.chunks_mut(2) {
                if v.delay > 0 {
                    v.delay -= 1;
                    continue;
                }
                let Some(&s) = v.buf.get(v.pos) else { break };
                frame[0] += s * v.gl;
                frame[1] += s * v.gr;
                v.pos += 1;
            }
        }
        self.voices.retain(|v| v.pos < v.buf.len());
        for s in out.iter_mut() {
            *s = (*s * MASTER).tanh();
        }
    }
}

struct Bank {
    sounds: Vec<Vec<Arc<Vec<f32>>>>,
}

struct State {
    bank: OnceLock<Bank>,
    mixer: Mutex<Mixer>,
    device: Mutex<Option<tinyaudio::OutputDevice>>,
    last: Mutex<Vec<f64>>,
    log: Mutex<Vec<(f64, Sfx, usize, f32, f32, f32)>>,
    muted: std::sync::atomic::AtomicBool,
    seed: std::sync::atomic::AtomicU32,
}

fn state() -> &'static State {
    static S: OnceLock<State> = OnceLock::new();
    S.get_or_init(|| State {
        bank: OnceLock::new(),
        mixer: Mutex::new(Mixer::default()),
        device: Mutex::new(None),
        last: Mutex::new(vec![-1.0; ALL.len()]),
        log: Mutex::new(Vec::new()),
        muted: std::sync::atomic::AtomicBool::new(std::env::var_os("BAG_MUTE").is_some()),
        seed: std::sync::atomic::AtomicU32::new(0x9e37_79b9),
    })
}

/// Makes the sounds and opens the speakers, in the background. With no
/// sound card the game simply stays silent.
pub fn init() {
    let recording = crate::record::active();
    std::thread::spawn(move || {
        let _ = state().bank.set(build_bank());
        if recording {
            return; // the video gets its soundtrack mixed offline
        }
        let params = tinyaudio::OutputDeviceParameters { sample_rate: SR, channels_count: 2, channel_sample_count: SR / 50 };
        match tinyaudio::run_output_device(params, |data| {
            if let Ok(mut m) = state().mixer.lock() {
                m.render(data);
            }
        }) {
            Ok(dev) => *state().device.lock().unwrap() = Some(dev),
            Err(e) => eprintln!("sound: no audio output ({e})"),
        }
    });
}

/// Sound on or off (the M key).
pub fn toggle_mute() -> bool {
    let m = !muted();
    set_muted(m);
    m
}

pub fn muted() -> bool {
    state().muted.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_muted(m: bool) {
    state().muted.store(m, std::sync::atomic::Ordering::Relaxed);
}

static VOLUME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000); // 1.0

/// Overall loudness, 0 to 1 (Settings).
pub fn volume() -> f32 {
    f32::from_bits(VOLUME.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn set_volume(v: f32) {
    VOLUME.store(v.clamp(0.0, 1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

fn next_rand() -> u32 {
    let s = &state().seed;
    let mut x = s.load(std::sync::atomic::Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    s.store(x, std::sync::atomic::Ordering::Relaxed);
    x
}

/// Play `s`. `pan` runs -1 (left) to 1 (right), `delay` in seconds.
pub fn play(s: Sfx, pan: f32, vol: f32, delay: f32) {
    let st = state();
    let Some(bank) = st.bank.get() else { return };
    let now = crate::record::now() + delay as f64;
    let id = s as usize;
    {
        let mut last = st.last.lock().unwrap();
        if now - last[id] < s.min_gap() && now >= last[id] {
            return;
        }
        last[id] = now;
    }
    let variant = next_rand() as usize % VARIANTS;
    let pan = pan.clamp(-1.0, 1.0);
    let g = vol * s.gain();
    // Equal-power pan.
    let a = (pan + 1.0) * PI / 4.0;
    let (gl, gr) = (a.cos() * g * 1.41, a.sin() * g * 1.41);
    if crate::record::active() {
        st.log.lock().unwrap().push((now, s, variant, gl, gr, 0.0));
        return;
    }
    if st.muted.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let v = volume();
    let (gl, gr) = (gl * v, gr * v);
    let buf = bank.sounds[id][variant].clone();
    st.mixer.lock().unwrap().add(Voice { buf, pos: 0, delay: (delay.max(0.0) * SR as f32) as usize, gl, gr });
}

/// A sound with no place on the map (buttons, fanfares).
pub fn play_ui(s: Sfx) {
    play(s, 0.0, 1.0, 0.0);
}

/// After a recording: mix every logged sound into a soundtrack and add it to
/// the video at `out`.
pub fn finish_recording(out: &str, seconds: f64) {
    let st = state();
    // The bank is built in the background; wait for it if a tour was short.
    for _ in 0..200 {
        if st.bank.get().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let Some(bank) = st.bank.get() else { return };
    let log = st.log.lock().unwrap();
    if log.is_empty() {
        return;
    }
    let n = (seconds * SR as f64) as usize;
    let mut m = Mixer::default();
    let mut data = vec![0.0f32; n * 2];
    let mut sorted: Vec<_> = log.iter().copied().collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Render in blocks, starting each sound in its block.
    let block = SR / 100;
    let mut next = 0;
    for start in (0..n).step_by(block) {
        while next < sorted.len() && (sorted[next].0 * SR as f64) as usize <= start + block {
            let (t, s, var, gl, gr, _) = sorted[next];
            let at = ((t * SR as f64) as usize).saturating_sub(start).min(block);
            m.add(Voice { buf: bank.sounds[s as usize][var].clone(), pos: 0, delay: at, gl, gr });
            next += 1;
        }
        let end = (start + block).min(n);
        m.render(&mut data[start * 2..end * 2]);
    }
    let wav = std::path::PathBuf::from(format!("{out}.sound.wav"));
    if write_wav(&wav, &data).is_err() {
        return;
    }
    let tmp = format!("{out}.tmp.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i", out, "-i"])
        .arg(&wav)
        .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest", "-movflags", "+faststart", &tmp])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        let _ = std::fs::rename(&tmp, out);
    }
    let _ = std::fs::remove_file(&wav);
}

fn write_wav(path: &std::path::Path, data: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let bytes = (data.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + bytes).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(SR as u32).to_le_bytes())?;
    f.write_all(&(SR as u32 * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&bytes.to_le_bytes())?;
    for s in data {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32000.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

// ------------------------------------------------------------- synthesis

struct Rng(u32);

impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    /// White noise, -1 to 1.
    fn n(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
}

/// State-variable filter: low, band and high pass from one input.
#[derive(Default)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    fn tick(&mut self, x: f32, fc: f32, q: f32) -> (f32, f32, f32) {
        let f = 2.0 * (PI * (fc / SR as f32).min(0.45)).sin();
        self.low += f * self.band;
        let high = x - self.low - q * self.band;
        self.band += f * high;
        (self.low, self.band, high)
    }
}

fn secs(s: f32) -> usize {
    (s * SR as f32) as usize
}

/// Attack-decay envelope: rises over `a` seconds, then falls at `d` per second.
fn ad(t: f32, a: f32, d: f32) -> f32 {
    if t < a {
        t / a
    } else {
        (-(t - a) * d).exp()
    }
}

/// Damped sine partials: (frequency, amplitude, decay per second).
fn partials(out: &mut [f32], parts: &[(f32, f32, f32)], start: usize) {
    for &(f, a, d) in parts {
        for (i, s) in out[start..].iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            let e = (-t * d).exp();
            if e < 1e-4 {
                break;
            }
            *s += (TAU * f * t).sin() * a * e;
        }
    }
}

/// A sine whose pitch falls from `f0` to `f1` (a thump or boom).
fn drop_tone(out: &mut [f32], f0: f32, f1: f32, fall: f32, amp: f32, decay: f32, start: usize) {
    let mut ph = 0.0f32;
    for (i, s) in out[start..].iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        let f = f1 + (f0 - f1) * (-t * fall).exp();
        ph += TAU * f / SR as f32;
        *s += ph.sin() * amp * (-t * decay).exp();
    }
}

/// Filtered noise burst with a centre that moves from `f0` to `f1`.
#[allow(clippy::too_many_arguments)]
fn noise_sweep(out: &mut [f32], r: &mut Rng, f0: f32, f1: f32, q: f32, amp: f32, a: f32, d: f32, start: usize, kind: u8) {
    let mut svf = Svf::default();
    let len = (out.len() - start) as f32;
    for (i, s) in out[start..].iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        let k = i as f32 / len;
        let fc = f0 * (f1 / f0).powf(k);
        let (lo, bp, hi) = svf.tick(r.n(), fc, q);
        let x = match kind {
            0 => lo,
            1 => bp,
            _ => hi,
        };
        *s += x * amp * ad(t, a, d);
    }
}

/// Karplus-Strong plucked string (bow twang).
fn pluck(out: &mut [f32], r: &mut Rng, f: f32, amp: f32, damp: f32) {
    let n = (SR as f32 / f) as usize;
    let mut line: Vec<f32> = (0..n).map(|_| r.n()).collect();
    let mut k = 0;
    for s in out.iter_mut() {
        let a = line[k];
        let b = line[(k + 1) % n];
        line[k] = (a + b) * 0.5 * damp;
        *s += a * amp;
        k = (k + 1) % n;
    }
}

/// A bell or chime note.
fn bell(out: &mut [f32], f: f32, amp: f32, start: usize) {
    partials(out, &[(f, amp, 5.0), (f * 2.76, amp * 0.4, 9.0), (f * 5.4, amp * 0.15, 16.0)], start);
}

/// A brassy note: bright harmonics swelling in.
fn brass(out: &mut [f32], f: f32, amp: f32, start: usize, len: usize) {
    let end = (start + len).min(out.len());
    for (i, s) in out[start..end].iter_mut().enumerate() {
        let t = i as f32 / SR as f32;
        let lt = len as f32 / SR as f32;
        let env = (t / 0.04).min(1.0) * ((lt - t) / 0.12).clamp(0.0, 1.0);
        let bright = 0.35 + 0.65 * (t / 0.1).min(1.0);
        let mut v = 0.0;
        for h in 1..8 {
            let hf = h as f32;
            v += (TAU * f * hf * t + 0.2 * (TAU * 5.0 * t).sin()).sin() / hf * bright.powf(hf * 0.5);
        }
        *s += v * amp * env;
    }
}

fn normalize(mut v: Vec<f32>, peak: f32) -> Vec<f32> {
    let m = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if m > 1e-6 {
        for x in &mut v {
            *x *= peak / m;
        }
    }
    // A few milliseconds of fade at both ends: no clicks.
    let f = secs(0.003).min(v.len() / 2);
    let n = v.len();
    for i in 0..f {
        let g = i as f32 / f as f32;
        v[i] *= g;
        v[n - 1 - i] *= g;
    }
    v
}

fn make(s: Sfx, r: &mut Rng) -> Vec<f32> {
    let p = r.range(0.9, 1.1); // pitch variation
    match s {
        Sfx::Swing => {
            let mut o = vec![0.0; secs(0.26)];
            noise_sweep(&mut o, r, 500.0 * p, 2600.0 * p, 0.3, 1.0, 0.11, 18.0, 0, 1);
            normalize(o, 0.8)
        }
        Sfx::Dodge => {
            let mut o = vec![0.0; secs(0.22)];
            noise_sweep(&mut o, r, 1800.0 * p, 400.0 * p, 0.35, 1.0, 0.05, 16.0, 0, 1);
            normalize(o, 0.7)
        }
        Sfx::BladeHit => {
            let mut o = vec![0.0; secs(0.4)];
            noise_sweep(&mut o, r, 5000.0, 2500.0, 0.6, 0.9, 0.001, 70.0, 0, 2);
            let f = 1250.0 * p;
            partials(&mut o, &[(f, 0.35, 22.0), (f * 2.19, 0.25, 28.0), (f * 3.13, 0.18, 34.0), (f * 4.6, 0.1, 45.0)], 0);
            drop_tone(&mut o, 180.0, 90.0, 30.0, 0.6, 30.0, 0);
            normalize(o, 0.9)
        }
        Sfx::BluntHit => {
            let mut o = vec![0.0; secs(0.25)];
            drop_tone(&mut o, 150.0 * p, 55.0, 25.0, 1.0, 18.0, 0);
            noise_sweep(&mut o, r, 1500.0, 400.0, 0.9, 0.7, 0.001, 45.0, 0, 0);
            normalize(o, 0.9)
        }
        Sfx::StoneHit => {
            let mut o = vec![0.0; secs(0.35)];
            drop_tone(&mut o, 110.0 * p, 50.0, 20.0, 0.8, 16.0, 0);
            // Crumbling: a few short gritty bursts.
            for k in 0..5 {
                let at = secs(0.012 * k as f32 + r.range(0.0, 0.03));
                noise_sweep(&mut o, r, 2200.0 * p, 900.0, 0.9, 0.6 / (1.0 + k as f32 * 0.5), 0.001, 60.0, at, 1);
            }
            normalize(o, 0.9)
        }
        Sfx::Collapse => {
            let mut o = vec![0.0; secs(2.2)];
            drop_tone(&mut o, 90.0, 35.0, 3.0, 1.0, 2.2, 0);
            noise_sweep(&mut o, r, 900.0, 120.0, 1.0, 1.6, 0.02, 1.8, 0, 0);
            for _ in 0..28 {
                let at = secs(r.range(0.05, 1.6));
                let (f, a) = (r.range(1500.0, 4000.0), r.range(0.1, 0.35));
                noise_sweep(&mut o, r, f, 800.0, 0.8, a, 0.001, 70.0, at, 1);
            }
            normalize(o, 1.0)
        }
        Sfx::BowRelease => {
            let mut o = vec![0.0; secs(0.3)];
            pluck(&mut o, r, 190.0 * p, 0.7, 0.985);
            noise_sweep(&mut o, r, 3000.0, 1200.0, 0.8, 0.3, 0.002, 25.0, 0, 1);
            for (i, s) in o.iter_mut().enumerate() {
                *s *= (-(i as f32 / SR as f32) * 9.0).exp();
            }
            normalize(o, 0.75)
        }
        Sfx::ArrowImpact => {
            let mut o = vec![0.0; secs(0.15)];
            drop_tone(&mut o, 420.0 * p, 160.0, 40.0, 0.9, 30.0, 0);
            noise_sweep(&mut o, r, 3500.0, 1500.0, 0.7, 0.6, 0.0005, 90.0, 0, 2);
            normalize(o, 0.8)
        }
        Sfx::Cannon => {
            let mut o = vec![0.0; secs(1.0)];
            noise_sweep(&mut o, r, 3000.0, 150.0, 1.0, 1.4, 0.002, 4.5, 0, 0);
            drop_tone(&mut o, 120.0 * p, 40.0, 8.0, 1.2, 4.0, 0);
            noise_sweep(&mut o, r, 6000.0, 3000.0, 0.7, 0.8, 0.0005, 80.0, 0, 2);
            normalize(o, 1.0)
        }
        Sfx::Explosion => {
            let mut o = vec![0.0; secs(0.9)];
            noise_sweep(&mut o, r, 2500.0 * p, 200.0, 1.0, 1.4, 0.004, 5.0, 0, 0);
            drop_tone(&mut o, 90.0, 40.0, 6.0, 0.9, 5.0, 0);
            normalize(o, 0.95)
        }
        Sfx::MagicBolt => {
            let mut o = vec![0.0; secs(0.45)];
            let mut ph = [0.0f32; 3];
            for (i, s) in o.iter_mut().enumerate() {
                let t = i as f32 / SR as f32;
                let f = (380.0 + 1400.0 * (t / 0.3).min(1.0)) * p;
                let env = ad(t, 0.02, 7.0);
                for (k, d) in [1.0, 1.007, 1.5].iter().enumerate() {
                    ph[k] += TAU * f * d * (1.0 + 0.02 * (TAU * 22.0 * t).sin()) / SR as f32;
                    *s += ph[k].sin() * env * if k == 2 { 0.3 } else { 0.5 };
                }
            }
            noise_sweep(&mut o, r, 4000.0, 6000.0, 0.6, 0.25, 0.01, 8.0, 0, 1);
            normalize(o, 0.7)
        }
        Sfx::Heal => {
            let mut o = vec![0.0; secs(0.9)];
            let base = 1046.5 * p;
            for (k, m) in [1.0, 1.26, 1.5].iter().enumerate() {
                bell(&mut o, base * m, 0.5, secs(0.06 * k as f32));
            }
            normalize(o, 0.6)
        }
        Sfx::Shimmer => {
            let mut o = vec![0.0; secs(0.8)];
            for (k, m) in [1.0, 1.5, 2.0, 2.52].iter().enumerate() {
                bell(&mut o, 660.0 * p * m, 0.35, secs(0.03 * k as f32));
            }
            noise_sweep(&mut o, r, 5000.0, 8000.0, 0.5, 0.2, 0.2, 4.0, 0, 1);
            normalize(o, 0.55)
        }
        Sfx::Block => {
            let mut o = vec![0.0; secs(0.35)];
            let f = 620.0 * p;
            partials(&mut o, &[(f, 0.4, 18.0), (f * 2.33, 0.3, 24.0), (f * 3.7, 0.15, 30.0)], 0);
            drop_tone(&mut o, 200.0, 100.0, 30.0, 0.7, 25.0, 0);
            noise_sweep(&mut o, r, 4000.0, 2000.0, 0.7, 0.6, 0.0005, 80.0, 0, 2);
            normalize(o, 0.9)
        }
        Sfx::Stun => {
            let mut o = vec![0.0; secs(0.6)];
            for k in 0..4 {
                bell(&mut o, 2093.0 * p / (1.0 + k as f32 * 0.12), 0.35, secs(0.08 * k as f32));
            }
            normalize(o, 0.5)
        }
        Sfx::Blink => {
            let mut o = vec![0.0; secs(0.3)];
            let mut ph = 0.0f32;
            for (i, s) in o.iter_mut().enumerate() {
                let t = i as f32 / SR as f32;
                let f = 200.0 + 2200.0 * (-t * 14.0).exp();
                ph += TAU * f * p / SR as f32;
                *s += (ph.sin() + 0.4 * (ph * 2.0).sin().signum() * 0.3) * ad(t, 0.005, 10.0);
            }
            noise_sweep(&mut o, r, 6000.0, 1000.0, 0.7, 0.4, 0.002, 14.0, 0, 1);
            normalize(o, 0.65)
        }
        Sfx::Charge => {
            let mut o = vec![0.0; secs(0.4)];
            noise_sweep(&mut o, r, 300.0, 1800.0, 0.8, 0.8, 0.08, 10.0, 0, 1);
            drop_tone(&mut o, 120.0 * p, 45.0, 18.0, 1.0, 12.0, secs(0.1));
            normalize(o, 0.85)
        }
        Sfx::Fall => {
            let mut o = vec![0.0; secs(0.4)];
            drop_tone(&mut o, 95.0 * p, 45.0, 14.0, 1.0, 11.0, secs(0.02));
            noise_sweep(&mut o, r, 900.0, 250.0, 0.9, 0.6, 0.01, 12.0, 0, 0);
            normalize(o, 0.75)
        }
        Sfx::Trap => {
            let mut o = vec![0.0; secs(0.25)];
            partials(&mut o, &[(1800.0 * p, 0.4, 40.0), (2900.0 * p, 0.3, 50.0)], 0);
            drop_tone(&mut o, 300.0, 120.0, 30.0, 0.8, 25.0, 0);
            normalize(o, 0.8)
        }
        Sfx::Victory => {
            let mut o = vec![0.0; secs(1.6)];
            for (k, (f, l)) in [(523.3, 0.16), (659.3, 0.16), (784.0, 0.16), (1046.5, 0.9)].iter().enumerate() {
                brass(&mut o, *f, 0.5, secs(0.17 * k as f32), secs(*l));
                brass(&mut o, *f * 0.5, 0.25, secs(0.17 * k as f32), secs(*l));
            }
            normalize(o, 0.8)
        }
        Sfx::Defeat => {
            let mut o = vec![0.0; secs(1.6)];
            for (k, (f, l)) in [(392.0, 0.3), (370.0, 0.3), (349.2, 0.3), (293.7, 0.9)].iter().enumerate() {
                brass(&mut o, *f, 0.45, secs(0.3 * k as f32), secs(*l));
            }
            normalize(o, 0.7)
        }
        Sfx::Click => {
            let mut o = vec![0.0; secs(0.06)];
            partials(&mut o, &[(1700.0 * p, 0.5, 90.0), (3400.0, 0.2, 120.0)], 0);
            normalize(o, 0.6)
        }
    }
}

fn build_bank() -> Bank {
    let mut r = Rng(0x1234_5678);
    Bank { sounds: ALL.iter().map(|&s| (0..VARIANTS).map(|_| Arc::new(make(s, &mut r))).collect()).collect() }
}

/// Writes every sound to `dir` as WAV files, for listening to them.
pub fn export_all(dir: &std::path::Path) {
    let bank = build_bank();
    let _ = std::fs::create_dir_all(dir);
    for (k, s) in ALL.iter().enumerate() {
        let mono = &bank.sounds[k][0];
        let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x * 0.8, x * 0.8]).collect();
        let _ = write_wav(&dir.join(format!("{s:?}.wav")), &stereo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_finite_and_audible() {
        let b = build_bank();
        for (k, s) in ALL.iter().enumerate() {
            assert_eq!(ALL[k], *s);
            assert_eq!(*s as usize, k, "ALL must list sounds in enum order");
            for v in &b.sounds[k] {
                assert!(v.iter().all(|x| x.is_finite()), "{s:?} has NaN");
                let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(peak > 0.3 && peak <= 1.0, "{s:?} peak {peak}");
            }
        }
    }
}
