//! How long the server takes to check one raid, by brain size (untrained
//! brains, so the raid runs the full 3 minutes: the worst case).
//! cargo run --release -p bag-server --example raidbench -- <hidden> <attackers> <defenders> [reps]
use bag_client::bag_protocol::{run_raid, BaseSnapshot};
use bag_client::{army_from, army_spots, defender_spots, snapshot_from};
use godcore::nn::Mlp;
use godcore::ppo::actor_dims_h;
use godcore::rng::Rng;
use godcore::scenario::HomeBase;
use godcore::troop::{Troop, MAX_LEVEL};
use std::time::Instant;

fn troop(name: &str, id: u64, h: usize) -> Troop {
    let p = format!(
        "{}/tests/fixtures/{name}.troop.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut t = Troop::load_json(&std::fs::read_to_string(p).unwrap()).unwrap();
    t.id = id;
    if h != t.actor.dims[1] {
        t.level = MAX_LEVEL;
        t.actor = Mlp::new(&actor_dims_h(h), 0.01, &mut Rng::new(id * 7 + h as u64));
        t.brain = Some(h);
    }
    t
}

fn main() {
    let a: Vec<usize> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let (h, na, nd, reps) = (a[0], a[1], a[2], *a.get(3).unwrap_or(&3));
    let names = ["fighter", "healer", "guard"];
    let defs: Vec<_> = (0..nd)
        .map(|k| troop("guard", 100 + k as u64, h))
        .zip(defender_spots(nd))
        .collect();
    let snap: BaseSnapshot = snapshot_from(&HomeBase::reference(), &defs);
    let army = army_from(
        &(0..na)
            .map(|k| troop(names[k % 2], 1 + k as u64, h))
            .zip(army_spots(na))
            .collect::<Vec<_>>(),
    );
    let snap_json = serde_json::to_string(&snap).unwrap();
    let army_json = serde_json::to_string(&army).unwrap();
    let mut best = f64::MAX;
    let mut ticks = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        let s: BaseSnapshot = serde_json::from_str(&snap_json).unwrap();
        let r = run_raid(&s, &army);
        best = best.min(t0.elapsed().as_secs_f64());
        ticks = r.ticks;
    }
    println!(
        "h={h} attackers={na} defenders={nd} ticks={ticks} best={best:.3}s base_json={:.1}MB army_json={:.1}MB",
        snap_json.len() as f64 / 1e6,
        army_json.len() as f64 / 1e6
    );
}
