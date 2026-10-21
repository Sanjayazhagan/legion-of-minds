//! The GPU update must give the same brain as the CPU one (up to rounding).
//! Skipped when there is no GPU; `GODGPU_ALLOW_CPU=1` runs it on a software
//! Vulkan driver such as lavapipe.

use godcore::nn::{Adam, Mlp};
use godcore::ppo::{cpu_ppo_update, Accelerator, Batch, PpoConfig};
use godcore::rng::Rng;
use godcore::sim::{N_ACTIONS, OBS_DIM};

fn batch(n: usize, rng: &mut Rng) -> Batch {
    let mut b = Batch { obs: vec![], masks: vec![], acts: vec![], old_logp: vec![], adv: vec![], rets: vec![] };
    for _ in 0..n {
        for _ in 0..OBS_DIM {
            b.obs.push(rng.range(-1.0, 1.0));
        }
        let mut m = [false; N_ACTIONS];
        for k in 0..N_ACTIONS {
            m[k] = rng.range(0.0, 1.0) < 0.7;
        }
        let a = rng.below(N_ACTIONS);
        m[a] = true;
        b.masks.push(m);
        b.acts.push(a as u8);
        b.old_logp.push(-rng.range(1.0, 4.0));
        b.adv.push(rng.range(-2.0, 2.0));
        b.rets.push(rng.range(-1.0, 1.0));
    }
    b
}

#[test]
fn gpu_update_matches_cpu() {
    let Some(gpu) = godgpu::Gpu::new() else {
        eprintln!("no GPU here, skipped");
        return;
    };
    eprintln!("running on {}", gpu.name());
    let mut rng = Rng::new(7);
    let h = 128;
    let actor = Mlp::new(&godcore::ppo::actor_dims_h(h), 0.01, &mut rng);
    let critic = Mlp::new(&godcore::ppo::critic_dims_h(h), 1.0, &mut rng);
    let cfg = PpoConfig { epochs: 2, minibatch: 256, ..PpoConfig::default() };
    // 600 is not a multiple of 256, so the last minibatch is short.
    let data = batch(600, &mut rng);
    let (mut a1, mut c1, mut oa1, mut oc1) = (actor.clone(), critic.clone(), Adam::new(actor.n_params()), Adam::new(critic.n_params()));
    let (mut a2, mut c2, mut oa2, mut oc2) = (actor.clone(), critic.clone(), Adam::new(actor.n_params()), Adam::new(critic.n_params()));
    let (mut r1, mut r2) = (Rng::new(99), Rng::new(99));
    let cpu = cpu_ppo_update(&cfg, &mut a1, &mut c1, &mut oa1, &mut oc1, &mut r1, &data);
    let gpu_r = gpu.update(&cfg, &mut a2, &mut c2, &mut oa2, &mut oc2, &mut r2, &data).expect("gpu update ran");
    assert_eq!(r1.below(1 << 30), r2.below(1 << 30), "rng used the same way");
    assert_eq!(cpu.3, gpu_r.3);
    assert_eq!(oa1.t, oa2.t);
    let close = |x: f32, y: f32, tol: f32| (x - y).abs() <= tol * (1.0 + x.abs().max(y.abs()));
    assert!(close(cpu.0, gpu_r.0, 1e-3), "policy loss {} vs {}", cpu.0, gpu_r.0);
    assert!(close(cpu.1, gpu_r.1, 1e-3), "value loss {} vs {}", cpu.1, gpu_r.1);
    assert!(close(cpu.2, gpu_r.2, 1e-3), "entropy {} vs {}", cpu.2, gpu_r.2);
    // How far did the update move each weight, and do both agree on it?
    let mut worst = 0.0f32;
    for (k, (x, y)) in a1.params.iter().zip(&a2.params).chain(c1.params.iter().zip(&c2.params)).enumerate() {
        let start = if k < actor.n_params() { actor.params[k] } else { critic.params[k - actor.n_params()] };
        let moved = (x - start).abs().max(1e-6);
        worst = worst.max((x - y).abs() / moved.max(cfg.lr));
    }
    eprintln!("worst difference, relative to the step size: {worst}");
    assert!(worst < 0.05, "GPU and CPU brains drifted apart: {worst}");
}
