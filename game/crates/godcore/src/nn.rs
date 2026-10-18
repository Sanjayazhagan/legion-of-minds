//! A tiny tanh MLP with hand-written backprop and Adam. No ML framework:
//! a troop's whole brain is a few thousand floats.

use crate::rng::{det_tanh, Rng};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mlp {
    pub dims: Vec<usize>,
    pub params: Vec<f32>,
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
