//! Trains troop brains on the graphics card.
//!
//! Only the slow part moves: the PPO update (forward, backward and Adam over
//! every minibatch). Playing the training games stays on the CPU, and battles
//! never touch this crate, so the server can still replay every raid exactly.
//! The update matches `godcore::ppo::cpu_ppo_update` step for step (same
//! shuffles, same losses, same clipping) up to float rounding.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use godcore::nn::{Adam, Mlp};
use godcore::ppo::{Accelerator, Batch, PpoConfig};
use godcore::rng::Rng;
use godcore::sim::{N_ACTIONS, OBS_DIM};

const N_PARTS: u32 = 64;
const MAX_ACT: usize = 32;
/// Descriptors sit this far apart in the uniform buffer (a safe dynamic
/// offset alignment on every GPU).
const DESC_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Desc {
    n_in: u32,
    n_out: u32,
    rows: u32,
    w_off: u32,
    b_off: u32,
    in_off: u32,
    out_off: u32,
    flag: u32,
    d_off: u32,
    d2_off: u32,
    mb_start: u32,
    n_total: u32,
    logit_off: u32,
    val_off: u32,
    dl_off: u32,
    dv_off: u32,
    part_off: u32,
    split: u32,
    obs_dim: u32,
    n_act: u32,
    mask_off: u32,
    lp_off: u32,
    adv_off: u32,
    ret_off: u32,
    act_off: u32,
    stat_stride: u32,
    pad0: u32,
    pad1: u32,
    adv_mean: f32,
    adv_std: f32,
    clip: f32,
    ent_coef: f32,
    vf_coef: f32,
    lr: f32,
    c1a: f32,
    c2a: f32,
    c1c: f32,
    c2c: f32,
    max_norm: f32,
    inv_b: f32,
}

#[derive(Clone, Copy)]
enum Kernel {
    Gather,
    Fwd,
    Head,
    Wgrad,
    Bprop,
    Norm,
    Adam,
}

struct Pipes {
    gather: wgpu::ComputePipeline,
    fwd: wgpu::ComputePipeline,
    head: wgpu::ComputePipeline,
    wgrad: wgpu::ComputePipeline,
    bprop: wgpu::ComputePipeline,
    norm: wgpu::ComputePipeline,
    adam: wgpu::ComputePipeline,
}

impl Pipes {
    fn get(&self, k: Kernel) -> &wgpu::ComputePipeline {
        match k {
            Kernel::Gather => &self.gather,
            Kernel::Fwd => &self.fwd,
            Kernel::Head => &self.head,
            Kernel::Wgrad => &self.wgrad,
            Kernel::Bprop => &self.bprop,
            Kernel::Norm => &self.norm,
            Kernel::Adam => &self.adam,
        }
    }
}

/// A graphics card ready to train brains.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipes: Pipes,
    name: String,
    /// One update at a time: they share the queue and it keeps memory low.
    busy: Mutex<()>,
}

/// Where one network's layers live in the shared buffers.
struct Net {
    dims: Vec<usize>,
    /// Weight offset of each layer in P (bias follows the weights).
    w_off: Vec<usize>,
    /// Output activations of each layer in A.
    act: Vec<usize>,
    /// d(loss)/d(output before tanh) of each layer in A.
    delta: Vec<usize>,
}

impl Net {
    fn new(dims: &[usize], p_base: usize, a_next: &mut usize, rows: usize) -> Net {
        let layers = dims.len() - 1;
        let mut w_off = Vec::new();
        let mut off = p_base;
        for l in 0..layers {
            w_off.push(off);
            off += dims[l] * dims[l + 1] + dims[l + 1];
        }
        let mut act = Vec::new();
        for l in 0..layers {
            act.push(*a_next);
            *a_next += rows * dims[l + 1];
        }
        let mut delta = Vec::new();
        for l in 0..layers {
            delta.push(*a_next);
            *a_next += rows * dims[l + 1];
        }
        Net { dims: dims.to_vec(), w_off, act, delta }
    }

    fn layers(&self) -> usize {
        self.dims.len() - 1
    }

    fn input(&self, l: usize, x0: usize) -> usize {
        if l == 0 {
            x0
        } else {
            self.act[l - 1]
        }
    }

    fn layer_desc(&self, l: usize, base: &Desc, x0: usize) -> Desc {
        let (n_in, n_out) = (self.dims[l], self.dims[l + 1]);
        Desc {
            n_in: n_in as u32,
            n_out: n_out as u32,
            w_off: self.w_off[l] as u32,
            b_off: (self.w_off[l] + n_in * n_out) as u32,
            in_off: self.input(l, x0) as u32,
            out_off: self.act[l] as u32,
            d_off: self.delta[l] as u32,
            d2_off: if l > 0 { self.delta[l - 1] as u32 } else { 0 },
            flag: (l + 1 < self.layers()) as u32,
            ..*base
        }
    }

    fn forward(&self, base: &Desc, x0: usize, steps: &mut Vec<(Kernel, Desc, u32)>) {
        for l in 0..self.layers() {
            let d = self.layer_desc(l, base, x0);
            steps.push((Kernel::Fwd, d, d.rows * d.n_out));
        }
    }

    fn backward(&self, base: &Desc, x0: usize, steps: &mut Vec<(Kernel, Desc, u32)>) {
        for l in (0..self.layers()).rev() {
            let d = self.layer_desc(l, base, x0);
            steps.push((Kernel::Wgrad, d, (d.n_in + 1) * d.n_out));
            if l > 0 {
                steps.push((Kernel::Bprop, d, d.rows * d.n_in));
            }
        }
    }
}

impl Gpu {
    /// The best graphics card on this computer, or `None` if there isn't a
    /// usable one. Software renderers (no real GPU) are skipped unless
    /// `GODGPU_ALLOW_CPU=1`, since the CPU trainer is faster than them.
    pub fn new() -> Option<Gpu> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))?;
        let info = adapter.get_info();
        let allow_cpu = std::env::var("GODGPU_ALLOW_CPU").is_ok_and(|v| v == "1");
        if info.device_type == wgpu::DeviceType::Cpu && !allow_cpu {
            return None;
        }
        let limits = wgpu::Limits::default();
        let have = adapter.limits();
        if have.max_storage_buffers_per_shader_stage < 8 || have.max_compute_invocations_per_workgroup < 256 {
            return None;
        }
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: have.max_storage_buffer_binding_size,
            max_buffer_size: have.max_buffer_size,
            ..limits
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("godgpu"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ppo"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("ppo.wgsl"))),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ppo"),
            entries: &[
                storage(0, false),
                storage(1, false),
                storage(2, false),
                storage(3, false),
                storage(4, false),
                storage(5, true),
                storage(6, true),
                storage(7, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Desc>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ppo"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipes = Pipes {
            gather: pipe("gather"),
            fwd: pipe("dense_fwd"),
            head: pipe("head"),
            wgrad: pipe("dense_wgrad"),
            bprop: pipe("dense_bprop"),
            norm: pipe("grad_norm"),
            adam: pipe("adam"),
        };
        let kind = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => "graphics card",
            wgpu::DeviceType::IntegratedGpu => "built-in graphics",
            wgpu::DeviceType::Cpu => "software, no GPU",
            _ => "GPU",
        };
        Some(Gpu {
            device,
            queue,
            layout,
            pipes,
            name: format!("{} ({kind})", info.name.trim()),
            busy: Mutex::new(()),
        })
    }

    fn storage(&self, label: &str, data: &[u8], extra: wgpu::BufferUsages) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: data,
            usage: wgpu::BufferUsages::STORAGE | extra,
        })
    }

    fn zeroed(&self, label: &str, floats: usize, extra: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (floats.max(1) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | extra,
            mapped_at_creation: false,
        })
    }

    fn run(
        &self,
        cfg: &PpoConfig,
        actor: &mut Mlp,
        critic: &mut Mlp,
        opt_a: &mut Adam,
        opt_c: &mut Adam,
        rng: &mut Rng,
        batch: &Batch,
    ) -> Option<(f32, f32, f32, usize)> {
        let n = batch.acts.len();
        let n_act = actor.out_dim();
        if n == 0 || n_act != N_ACTIONS || n_act > MAX_ACT || critic.out_dim() != 1 || cfg.minibatch == 0 {
            return None;
        }
        if actor.dims[0] != OBS_DIM || critic.dims[0] != OBS_DIM {
            return None;
        }
        let rows = cfg.minibatch.min(n);
        let (na, nc) = (actor.n_params(), critic.n_params());
        let total = na + nc;

        // A: activations and deltas of one minibatch.
        let x0 = 0;
        let mut a_len = rows * OBS_DIM;
        let anet = Net::new(&actor.dims, 0, &mut a_len, rows);
        let cnet = Net::new(&critic.dims, na, &mut a_len, rows);

        // D: the whole rollout.
        let obs_off = 0;
        let mask_off = obs_off + n * OBS_DIM;
        let lp_off = mask_off + n * n_act;
        let adv_off = lp_off + n;
        let ret_off = adv_off + n;
        let act_off = ret_off + n;
        let mut data = Vec::with_capacity(act_off + n);
        data.extend_from_slice(&batch.obs);
        for m in &batch.masks {
            data.extend(m.iter().map(|&b| if b { 1.0f32 } else { 0.0 }));
        }
        data.extend_from_slice(&batch.old_logp);
        data.extend_from_slice(&batch.adv);
        data.extend_from_slice(&batch.rets);
        data.extend(batch.acts.iter().map(|&a| a as f32));
        if data.len() != act_off + n || batch.obs.len() != n * OBS_DIM {
            return None;
        }

        // The same shuffles as the CPU, so `rng` ends in the same state.
        let mut idx: Vec<u32> = (0..n as u32).collect();
        let mut all_idx = Vec::with_capacity(cfg.epochs * n);
        let mut steps: Vec<(Kernel, Desc, u32)> = Vec::new();
        let stride = rows;
        let part_off = 3 * stride;
        let (mut ta, mut tc) = (opt_a.t, opt_c.t);
        let mut batches = 0;
        for e in 0..cfg.epochs {
            for i in (1..n).rev() {
                let j = rng.below(i + 1);
                idx.swap(i, j);
            }
            all_idx.extend_from_slice(&idx);
            for (k, mb) in idx.chunks(cfg.minibatch).enumerate() {
                let b = mb.len() as f32;
                let mean = mb.iter().map(|&i| batch.adv[i as usize]).sum::<f32>() / b;
                let var = mb.iter().map(|&i| (batch.adv[i as usize] - mean).powi(2)).sum::<f32>() / b;
                ta += 1;
                tc += 1;
                let base = Desc {
                    rows: mb.len() as u32,
                    mb_start: (e * n + k * cfg.minibatch) as u32,
                    n_total: total as u32,
                    logit_off: anet.act[anet.layers() - 1] as u32,
                    val_off: cnet.act[cnet.layers() - 1] as u32,
                    dl_off: anet.delta[anet.layers() - 1] as u32,
                    dv_off: cnet.delta[cnet.layers() - 1] as u32,
                    part_off: part_off as u32,
                    split: na as u32,
                    obs_dim: OBS_DIM as u32,
                    n_act: n_act as u32,
                    mask_off: mask_off as u32,
                    lp_off: lp_off as u32,
                    adv_off: adv_off as u32,
                    ret_off: ret_off as u32,
                    act_off: act_off as u32,
                    stat_stride: stride as u32,
                    adv_mean: mean,
                    adv_std: var.sqrt() + 1e-8,
                    clip: cfg.clip,
                    ent_coef: cfg.ent_coef,
                    vf_coef: cfg.vf_coef,
                    lr: cfg.lr,
                    c1a: 1.0 - 0.9f32.powi(ta as i32),
                    c2a: 1.0 - 0.999f32.powi(ta as i32),
                    c1c: 1.0 - 0.9f32.powi(tc as i32),
                    c2c: 1.0 - 0.999f32.powi(tc as i32),
                    max_norm: cfg.max_grad_norm,
                    inv_b: 1.0 / b,
                    out_off: x0 as u32,
                    ..Desc::default()
                };
                steps.push((Kernel::Gather, base, base.rows * OBS_DIM as u32));
                anet.forward(&base, x0, &mut steps);
                cnet.forward(&base, x0, &mut steps);
                steps.push((Kernel::Head, base, base.rows));
                anet.backward(&base, x0, &mut steps);
                cnet.backward(&base, x0, &mut steps);
                steps.push((Kernel::Norm, base, 0));
                steps.push((Kernel::Adam, base, total as u32));
                batches += 1;
            }
        }

        let _guard = self.busy.lock().ok()?;
        let copy = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let mut params = actor.params.clone();
        params.extend_from_slice(&critic.params);
        let mut m = opt_a.m.clone();
        m.extend_from_slice(&opt_c.m);
        let mut v = opt_a.v.clone();
        v.extend_from_slice(&opt_c.v);
        if m.len() != total || v.len() != total {
            return None;
        }
        let bp = self.storage("P", bytemuck::cast_slice(&params), copy);
        let bg = self.zeroed("G", total, wgpu::BufferUsages::empty());
        let bm = self.storage("M", bytemuck::cast_slice(&m), copy);
        let bv = self.storage("V", bytemuck::cast_slice(&v), copy);
        let ba = self.zeroed("A", a_len, wgpu::BufferUsages::empty());
        let bd = self.storage("D", bytemuck::cast_slice(&data), wgpu::BufferUsages::empty());
        let bi = self.storage("IDX", bytemuck::cast_slice(&all_idx), wgpu::BufferUsages::empty());
        let bs = self.storage("S", bytemuck::cast_slice(&vec![0.0f32; part_off + N_PARTS as usize]), copy);
        let mut descs = vec![0u8; steps.len() * DESC_STRIDE as usize];
        for (i, (_, d, _)) in steps.iter().enumerate() {
            let at = i * DESC_STRIDE as usize;
            descs[at..at + std::mem::size_of::<Desc>()].copy_from_slice(bytemuck::bytes_of(d));
        }
        let bu = {
            use wgpu::util::DeviceExt;
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("descs"),
                contents: &descs,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        fn whole(b: &wgpu::Buffer, i: u32) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding: i, resource: b.as_entire_binding() }
        }
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ppo"),
            layout: &self.layout,
            entries: &[
                whole(&bp, 0),
                whole(&bg, 1),
                whole(&bm, 2),
                whole(&bv, 3),
                whole(&ba, 4),
                whole(&bd, 5),
                whole(&bi, 6),
                whole(&bs, 7),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &bu,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Desc>() as u64),
                    }),
                },
            ],
        });

        let max_x = self.device.limits().max_compute_workgroups_per_dimension;
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            for (i, (k, _, threads)) in steps.iter().enumerate() {
                pass.set_pipeline(self.pipes.get(*k));
                pass.set_bind_group(0, &group, &[(i as u64 * DESC_STRIDE) as u32]);
                let groups = match k {
                    Kernel::Norm => N_PARTS,
                    _ => threads.div_ceil(64),
                };
                let gx = groups.min(max_x).max(1);
                let gy = groups.div_ceil(gx).max(1);
                pass.dispatch_workgroups(gx, gy, 1);
            }
        }
        let s_len = part_off + N_PARTS as usize;
        let read_len = (3 * total + s_len) * 4;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: read_len as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let t4 = (total * 4) as u64;
        enc.copy_buffer_to_buffer(&bp, 0, &staging, 0, t4);
        enc.copy_buffer_to_buffer(&bm, 0, &staging, t4, t4);
        enc.copy_buffer_to_buffer(&bv, 0, &staging, 2 * t4, t4);
        enc.copy_buffer_to_buffer(&bs, 0, &staging, 3 * t4, (s_len * 4) as u64);
        self.queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;
        let out: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        staging.unmap();

        let (p, rest) = out.split_at(total);
        let (mo, rest) = rest.split_at(total);
        let (vo, s) = rest.split_at(total);
        if p.iter().any(|x| !x.is_finite()) {
            return None;
        }
        actor.params.copy_from_slice(&p[..na]);
        critic.params.copy_from_slice(&p[na..]);
        opt_a.m.copy_from_slice(&mo[..na]);
        opt_c.m.copy_from_slice(&mo[na..]);
        opt_a.v.copy_from_slice(&vo[..na]);
        opt_c.v.copy_from_slice(&vo[na..]);
        opt_a.t = ta;
        opt_c.t = tc;
        let sum = |r: std::ops::Range<usize>| s[r].iter().sum::<f32>();
        Some((sum(0..stride), sum(stride..2 * stride), sum(2 * stride..3 * stride), batches))
    }
}

impl Accelerator for Gpu {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn update(
        &self,
        cfg: &PpoConfig,
        actor: &mut Mlp,
        critic: &mut Mlp,
        opt_a: &mut Adam,
        opt_c: &mut Adam,
        rng: &mut Rng,
        batch: &Batch,
    ) -> Option<(f32, f32, f32, usize)> {
        self.run(cfg, actor, critic, opt_a, opt_c, rng, batch)
    }
}

/// Finds a graphics card and makes training use it for big brains. Returns
/// its name, or `None` if training stays on the CPU.
pub fn enable() -> Option<String> {
    let gpu = Arc::new(Gpu::new()?);
    let name = gpu.name.clone();
    godcore::ppo::set_accelerator(Some(gpu));
    Some(name)
}
