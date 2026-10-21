// The PPO update of crates/godcore/src/ppo.rs (cpu_ppo_update), as compute
// kernels. One dispatch does one step of one minibatch; the CPU lists every
// step of the whole update up front and the GPU runs them in order.

struct Desc {
    n_in: u32, n_out: u32, rows: u32, w_off: u32,
    b_off: u32, in_off: u32, out_off: u32, flag: u32,
    d_off: u32, d2_off: u32, mb_start: u32, n_total: u32,
    logit_off: u32, val_off: u32, dl_off: u32, dv_off: u32,
    part_off: u32, split: u32, obs_dim: u32, n_act: u32,
    mask_off: u32, lp_off: u32, adv_off: u32, ret_off: u32,
    act_off: u32, stat_stride: u32, pad0: u32, pad1: u32,
    adv_mean: f32, adv_std: f32, clip: f32, ent_coef: f32,
    vf_coef: f32, lr: f32, c1a: f32, c2a: f32,
    c1c: f32, c2c: f32, max_norm: f32, inv_b: f32,
};

@group(0) @binding(0) var<storage, read_write> P: array<f32>;
@group(0) @binding(1) var<storage, read_write> G: array<f32>;
@group(0) @binding(2) var<storage, read_write> M: array<f32>;
@group(0) @binding(3) var<storage, read_write> V: array<f32>;
@group(0) @binding(4) var<storage, read_write> A: array<f32>;
@group(0) @binding(5) var<storage, read> D: array<f32>;
@group(0) @binding(6) var<storage, read> IDX: array<u32>;
@group(0) @binding(7) var<storage, read_write> S: array<f32>;
@group(0) @binding(8) var<uniform> d: Desc;

const WG: u32 = 64u;
const N_PARTS: u32 = 64u;
const MAX_ACT: u32 = 32u;

fn tid(g: vec3<u32>, n: vec3<u32>) -> u32 {
    return g.x + g.y * n.x * WG;
}

fn tanh_(x: f32) -> f32 {
    if (x > 9.0) { return 1.0; }
    if (x < -9.0) { return -1.0; }
    return 1.0 - 2.0 / (exp(2.0 * x) + 1.0);
}

// x0[b, k] = obs[idx[b], k]
@compute @workgroup_size(64)
fn gather(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let t = tid(g, n);
    if (t >= d.rows * d.obs_dim) { return; }
    let b = t / d.obs_dim;
    let k = t % d.obs_dim;
    let i = IDX[d.mb_start + b];
    A[d.out_off + t] = D[i * d.obs_dim + k];
}

// out[b, o] = act(bias[o] + sum_k W[o, k] in[b, k])
@compute @workgroup_size(64)
fn dense_fwd(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let t = tid(g, n);
    if (t >= d.rows * d.n_out) { return; }
    let b = t / d.n_out;
    let o = t % d.n_out;
    var s = P[d.b_off + o];
    let w = d.w_off + o * d.n_in;
    let x = d.in_off + b * d.n_in;
    for (var k = 0u; k < d.n_in; k++) {
        s += P[w + k] * A[x + k];
    }
    if (d.flag == 1u) { s = tanh_(s); }
    A[d.out_off + t] = s;
}

// Clipped policy loss, entropy bonus and value loss for one decision:
// d(loss)/d(logits) and d(loss)/d(value), plus the running loss sums.
@compute @workgroup_size(64)
fn head(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let b = tid(g, n);
    if (b >= d.rows) { return; }
    let i = IDX[d.mb_start + b];
    let na = d.n_act;
    let L = d.logit_off + b * na;
    var mx = -3.0e38;
    for (var k = 0u; k < na; k++) {
        if (D[d.mask_off + i * na + k] > 0.5) { mx = max(mx, A[L + k]); }
    }
    var z = 0.0;
    for (var k = 0u; k < na; k++) {
        if (D[d.mask_off + i * na + k] > 0.5) { z += exp(A[L + k] - mx); }
    }
    let lse = mx + log(z);
    var lp: array<f32, MAX_ACT>;
    var h = 0.0;
    for (var k = 0u; k < na; k++) {
        lp[k] = A[L + k] - lse;
        if (D[d.mask_off + i * na + k] > 0.5) { h -= exp(lp[k]) * lp[k]; }
    }
    let a = u32(D[d.act_off + i]);
    let ad = (D[d.adv_off + i] - d.adv_mean) / d.adv_std;
    let ratio = exp(lp[a] - D[d.lp_off + i]);
    let s1 = ratio * ad;
    let clipped = clamp(ratio, 1.0 - d.clip, 1.0 + d.clip);
    let s2 = clipped * ad;
    var g_logp = 0.0;
    if (s1 <= s2 || abs(ratio - clipped) < 1e-12) { g_logp = -ad * ratio; }
    for (var k = 0u; k < na; k++) {
        var dl = 0.0;
        if (D[d.mask_off + i * na + k] > 0.5) {
            let p = exp(lp[k]);
            var ind = 0.0;
            if (k == a) { ind = 1.0; }
            dl = (g_logp * (ind - p) + d.ent_coef * p * (lp[k] + h)) * d.inv_b;
        }
        A[d.dl_off + b * na + k] = dl;
    }
    let diff = A[d.val_off + b] - D[d.ret_off + i];
    A[d.dv_off + b] = d.vf_coef * 2.0 * diff * d.inv_b;
    S[b] += -min(s1, s2) * d.inv_b;
    S[d.stat_stride + b] += diff * diff * d.inv_b;
    S[2u * d.stat_stride + b] += h * d.inv_b;
}

// Weight grads (k < n_in) and bias grads (k == n_in) of one layer.
@compute @workgroup_size(64)
fn dense_wgrad(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let t = tid(g, n);
    let cols = d.n_in + 1u;
    if (t >= cols * d.n_out) { return; }
    let o = t / cols;
    let k = t % cols;
    var s = 0.0;
    if (k == d.n_in) {
        for (var b = 0u; b < d.rows; b++) { s += A[d.d_off + b * d.n_out + o]; }
        G[d.b_off + o] = s;
    } else {
        for (var b = 0u; b < d.rows; b++) {
            s += A[d.d_off + b * d.n_out + o] * A[d.in_off + b * d.n_in + k];
        }
        G[d.w_off + o * d.n_in + k] = s;
    }
}

// Delta for the layer below, through its tanh.
@compute @workgroup_size(64)
fn dense_bprop(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let t = tid(g, n);
    if (t >= d.rows * d.n_in) { return; }
    let b = t / d.n_in;
    let k = t % d.n_in;
    var s = 0.0;
    for (var o = 0u; o < d.n_out; o++) {
        s += A[d.d_off + b * d.n_out + o] * P[d.w_off + o * d.n_in + k];
    }
    let a = A[d.in_off + b * d.n_in + k];
    A[d.d2_off + b * d.n_in + k] = s * (1.0 - a * a);
}

var<workgroup> part: array<f32, 256>;

// Sum of squared grads, in N_PARTS pieces, for gradient clipping.
@compute @workgroup_size(256)
fn grad_norm(@builtin(local_invocation_id) l: vec3<u32>, @builtin(workgroup_id) w: vec3<u32>) {
    var s = 0.0;
    for (var i = w.x * 256u + l.x; i < d.n_total; i += N_PARTS * 256u) {
        s += G[i] * G[i];
    }
    part[l.x] = s;
    workgroupBarrier();
    for (var h = 128u; h > 0u; h >>= 1u) {
        if (l.x < h) { part[l.x] += part[l.x + h]; }
        workgroupBarrier();
    }
    if (l.x == 0u) { S[d.part_off + w.x] = part[0]; }
}

// Clip the grads to max_norm and take one Adam step (actor below `split`,
// critic above).
@compute @workgroup_size(64)
fn adam(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    let i = tid(g, n);
    if (i >= d.n_total) { return; }
    var sq = 0.0;
    for (var p = 0u; p < N_PARTS; p++) { sq += S[d.part_off + p]; }
    let norm = sqrt(sq);
    var scale = 1.0;
    if (norm > d.max_norm) { scale = d.max_norm / norm; }
    let gr = G[i] * scale;
    var c1 = d.c1a;
    var c2 = d.c2a;
    if (i >= d.split) { c1 = d.c1c; c2 = d.c2c; }
    let m = 0.9 * M[i] + 0.1 * gr;
    let v = 0.999 * V[i] + 0.001 * gr * gr;
    M[i] = m;
    V[i] = v;
    P[i] -= d.lr * (m / c1) / (sqrt(v / c2) + 1e-5);
}
