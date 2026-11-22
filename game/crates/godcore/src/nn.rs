//! A tiny tanh MLP with hand-written backprop and Adam. No ML framework:
//! a troop's whole brain is a few thousand floats.

use crate::rng::{det_tanh, Rng};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mlp {
    pub dims: Vec<usize>,
    #[serde(with = "packed")]
    pub params: Vec<f32>,
}

/// Brains up to 256 neurons are saved as a plain list of numbers, as they
/// always were. Bigger ones (100k+ numbers) are saved as the raw bits in
/// base64, which is about 2.5x smaller and quicker to read. Both forms load,
/// and both give back exactly the same numbers.
mod packed {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    /// Lists longer than this are packed. A 256-neuron actor has about
    /// 113k numbers (154 inputs), its critic about 106k; a 384 one has 218k.
    pub const PACK_ABOVE: usize = 150_000;
    const PREFIX: &str = "f32le:";
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(v: &Vec<f32>, s: S) -> Result<S::Ok, S::Error> {
        if v.len() <= PACK_ABOVE {
            return serde::Serialize::serialize(v, s);
        }
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        let mut out = String::with_capacity(PREFIX.len() + bytes.len() / 3 * 4 + 4);
        out.push_str(PREFIX);
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for k in 0..4 {
                out.push(if k <= c.len() { ABC[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
            }
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        d.deserialize_any(V)
    }

    struct V;

    impl<'de> Visitor<'de> for V {
        type Value = Vec<f32>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list of numbers or packed f32le base64")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Vec<f32>, A::Error> {
            let mut v = Vec::with_capacity(a.size_hint().unwrap_or(0).min(1 << 20));
            while let Some(x) = a.next_element::<f32>()? {
                v.push(x);
            }
            Ok(v)
        }

        fn visit_str<E: Error>(self, s: &str) -> Result<Vec<f32>, E> {
            let b64 = s.strip_prefix(PREFIX).ok_or_else(|| E::custom("unknown brain packing"))?.as_bytes();
            if b64.len() % 4 != 0 {
                return Err(E::custom("bad base64 length"));
            }
            let val = |c: u8| -> Result<u32, E> {
                Ok(match c {
                    b'A'..=b'Z' => c - b'A',
                    b'a'..=b'z' => c - b'a' + 26,
                    b'0'..=b'9' => c - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return Err(E::custom("bad base64")),
                } as u32)
            };
            let mut bytes = Vec::with_capacity(b64.len() / 4 * 3);
            for c in b64.chunks(4) {
                let pad = c.iter().rev().take_while(|&&x| x == b'=').count();
                if pad > 2 {
                    return Err(E::custom("bad base64 padding"));
                }
                let mut n = 0u32;
                for (k, &x) in c.iter().enumerate() {
                    n |= if k < 4 - pad { val(x)? } else { 0 } << (18 - 6 * k);
                }
                bytes.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
            }
            if bytes.len() % 4 != 0 {
                return Err(E::custom("packed brain is not whole f32s"));
            }
            Ok(bytes.chunks(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        }
    }
}

/// Activations of one forward pass, kept for backprop.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    pub acts: Vec<Vec<f32>>,
}

impl Mlp {
    /// `out_gain` scales the last layer's init (small for policy heads).
    pub fn new(dims: &[usize], out_gain: f32, rng: &mut Rng) -> Self {
        let mut params = Vec::new();
        let layers = dims.len() - 1;
        for l in 0..layers {
            let (n_in, n_out) = (dims[l], dims[l + 1]);
            let gain = if l + 1 == layers { out_gain } else { 1.0 };
            // Uniform with the variance of an orthogonal-ish init.
            let bound = gain * (3.0 / n_in as f32).sqrt();
            for _ in 0..n_in * n_out {
                params.push(rng.range(-bound, bound));
            }
            params.extend(std::iter::repeat(0.0).take(n_out));
        }
        Mlp {
            dims: dims.to_vec(),
            params,
        }
    }

    /// A bigger copy of this network that computes exactly the same outputs
    /// for the inputs and outputs it already had. New inputs start ignored,
    /// new hidden neurons get random inputs but no say in the output yet, and
    /// new outputs start at `new_out_bias`. `None` if `dims` would shrink it.
    pub fn grow(&self, dims: &[usize], new_out_bias: f32, rng: &mut Rng) -> Option<Mlp> {
        if dims.len() != self.dims.len() || dims.iter().zip(&self.dims).any(|(n, o)| n < o) {
            return None;
        }
        let layers = dims.len() - 1;
        let mut params = Vec::new();
        let mut off = 0;
        for l in 0..layers {
            let (oi, oo) = (self.dims[l], self.dims[l + 1]);
            let (ni, no) = (dims[l], dims[l + 1]);
            let w = &self.params[off..off + oi * oo];
            let b = &self.params[off + oi * oo..off + oi * oo + oo];
            let last = l + 1 == layers;
            let bound = (3.0 / ni as f32).sqrt();
            for o in 0..no {
                for i in 0..ni {
                    params.push(if o < oo && i < oi {
                        w[o * oi + i]
                    } else if o >= oo && !last {
                        rng.range(-bound, bound)
                    } else {
                        0.0
                    });
                }
            }
            for o in 0..no {
                params.push(if o < oo {
                    b[o]
                } else if last {
                    new_out_bias
                } else {
                    0.0
                });
            }
            off += oi * oo + oo;
        }
        Some(Mlp { dims: dims.to_vec(), params })
    }

    pub fn n_params(&self) -> usize {
        self.params.len()
    }

    pub fn out_dim(&self) -> usize {
        *self.dims.last().unwrap()
    }

    /// Forward pass. Hidden layers use tanh, the output is linear.
    pub fn forward(&self, x: &[f32], cache: &mut Cache) {
        let layers = self.dims.len() - 1;
        cache.acts.resize(layers + 1, Vec::new());
        cache.acts[0].clear();
        cache.acts[0].extend_from_slice(x);
        let mut off = 0;
        for l in 0..layers {
            let (n_in, n_out) = (self.dims[l], self.dims[l + 1]);
            let w = &self.params[off..off + n_in * n_out];
            let b = &self.params[off + n_in * n_out..off + n_in * n_out + n_out];
            let (prev, next) = cache.acts.split_at_mut(l + 1);
            let input = &prev[l];
            let out = &mut next[0];
            out.clear();
            for o in 0..n_out {
                let row = &w[o * n_in..(o + 1) * n_in];
                let mut s = b[o];
                for k in 0..n_in {
                    s += row[k] * input[k];
                }
                out.push(if l + 1 < layers { det_tanh(s) } else { s });
            }
            off += n_in * n_out + n_out;
        }
    }

    pub fn output<'a>(&self, cache: &'a Cache) -> &'a [f32] {
        cache.acts.last().unwrap()
    }

    /// Add d(loss)/d(params) to `grad`, given d(loss)/d(output).
    pub fn backward(&self, cache: &Cache, d_out: &[f32], grad: &mut [f32], scratch: &mut Vec<f32>) {
        let layers = self.dims.len() - 1;
        let mut offs = Vec::with_capacity(layers);
        let mut off = 0;
        for l in 0..layers {
            offs.push(off);
            off += self.dims[l] * self.dims[l + 1] + self.dims[l + 1];
        }
        let mut delta: Vec<f32> = d_out.to_vec();
        for l in (0..layers).rev() {
            let (n_in, n_out) = (self.dims[l], self.dims[l + 1]);
            let off = offs[l];
            let input = &cache.acts[l];
            // Bias and weight grads.
            for o in 0..n_out {
                let d = delta[o];
                grad[off + n_in * n_out + o] += d;
                let g = &mut grad[off + o * n_in..off + (o + 1) * n_in];
                for k in 0..n_in {
                    g[k] += d * input[k];
                }
            }
            if l == 0 {
                break;
            }
            // Delta for the previous layer, through tanh.
            scratch.clear();
            scratch.resize(n_in, 0.0);
            let w = &self.params[off..off + n_in * n_out];
            for o in 0..n_out {
                let d = delta[o];
                let row = &w[o * n_in..(o + 1) * n_in];
                for k in 0..n_in {
                    scratch[k] += d * row[k];
                }
            }
            for k in 0..n_in {
                let a = input[k];
                scratch[k] *= 1.0 - a * a;
            }
            std::mem::swap(&mut delta, scratch);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Adam {
    pub m: Vec<f32>,
    pub v: Vec<f32>,
    pub t: u32,
}

impl Adam {
    pub fn new(n: usize) -> Self {
        Adam {
            m: vec![0.0; n],
            v: vec![0.0; n],
            t: 0,
        }
    }

    pub fn step(&mut self, params: &mut [f32], grad: &[f32], lr: f32) {
        const B1: f32 = 0.9;
        const B2: f32 = 0.999;
        const EPS: f32 = 1e-5;
        self.t += 1;
        let c1 = 1.0 - B1.powi(self.t as i32);
        let c2 = 1.0 - B2.powi(self.t as i32);
        for i in 0..params.len() {
            self.m[i] = B1 * self.m[i] + (1.0 - B1) * grad[i];
            self.v[i] = B2 * self.v[i] + (1.0 - B2) * grad[i] * grad[i];
            params[i] -= lr * (self.m[i] / c1) / ((self.v[i] / c2).sqrt() + EPS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_brains_pack_and_load_bit_exact() {
        let mut rng = Rng::new(3);
        for h in [64usize, 256, 384] {
            let mut m = Mlp::new(&crate::ppo::actor_dims_h(h), 0.01, &mut rng);
            m.params[0] = f32::MIN_POSITIVE;
            m.params[1] = -0.0;
            m.params[2] = 1.0e-42; // subnormal
            let s = serde_json::to_string(&m).unwrap();
            assert_eq!(s.contains("f32le:"), m.params.len() > packed::PACK_ABOVE, "{h}");
            let back: Mlp = serde_json::from_str(&s).unwrap();
            assert_eq!(back.dims, m.dims);
            let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(&back.params), bits(&m.params), "{h}");
        }
        // Every padding length round-trips.
        for n in 1..7 {
            let v: Vec<f32> = (0..n).map(|k| k as f32 * 0.37 - 1.0).collect();
            let mut s = Vec::new();
            packed::serialize(&{ let mut big = v.clone(); big.resize(packed::PACK_ABOVE + 1 + n, 0.5); big }, &mut serde_json::Serializer::new(&mut s)).unwrap();
            let back: Vec<f32> = packed::deserialize(&mut serde_json::Deserializer::from_slice(&s)).unwrap();
            assert_eq!(&back[..n], &v[..]);
        }
        assert!(serde_json::from_str::<Mlp>(r#"{"dims":[1,1],"params":"nope"}"#).is_err());
    }

    #[test]
    fn growing_keeps_the_old_outputs() {
        let mut rng = Rng::new(9);
        let net = Mlp::new(&[5, 7, 7, 3], 1.0, &mut rng);
        let big = net.grow(&[8, 12, 12, 4], -2.0, &mut rng).unwrap();
        let x: Vec<f32> = (0..5).map(|i| (i as f32 * 0.61).cos()).collect();
        let mut xb = x.clone();
        xb.extend([0.3, -0.9, 0.5]);
        let (mut c, mut cb) = (Cache::default(), Cache::default());
        net.forward(&x, &mut c);
        big.forward(&xb, &mut cb);
        for k in 0..3 {
            assert!((net.output(&c)[k] - big.output(&cb)[k]).abs() < 1e-6);
        }
        assert_eq!(big.output(&cb)[3], -2.0);
        assert!(net.grow(&[4, 7, 7, 3], 0.0, &mut rng).is_none());
    }

    #[test]
    fn gradient_matches_finite_difference() {
        let mut rng = Rng::new(3);
        let mut net = Mlp::new(&[5, 7, 6, 3], 1.0, &mut rng);
        let x: Vec<f32> = (0..5).map(|i| (i as f32 * 0.37).sin()).collect();
        let w = [0.3f32, -1.2, 0.8];
        let loss = |net: &Mlp| {
            let mut c = Cache::default();
            net.forward(&x, &mut c);
            net.output(&c)
                .iter()
                .zip(w)
                .map(|(o, w)| o * w)
                .sum::<f32>()
        };
        let mut c = Cache::default();
        net.forward(&x, &mut c);
        let mut grad = vec![0.0; net.n_params()];
        let mut scratch = Vec::new();
        net.backward(&c, &w, &mut grad, &mut scratch);
        for i in (0..net.n_params()).step_by(7) {
            let orig = net.params[i];
            net.params[i] = orig + 1e-2;
            let up = loss(&net);
            net.params[i] = orig - 1e-2;
            let down = loss(&net);
            net.params[i] = orig;
            let fd = (up - down) / 2e-2;
            assert!(
                (fd - grad[i]).abs() < 2e-3,
                "param {i}: fd {fd} vs {}",
                grad[i]
            );
        }
    }
}
