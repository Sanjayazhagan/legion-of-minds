"""Train one fighter with PPO on CPU.

    python3 train.py --steps 1000000 --out runs/fighter
"""
import argparse, json, os, time
import numpy as np
import torch
from stable_baselines3 import PPO
from stable_baselines3.common.callbacks import BaseCallback
from stable_baselines3.common.env_util import make_vec_env

from arena import FighterArena


class EpisodeLog(BaseCallback):
    """Record every finished episode so we can plot the learning curve."""

    def __init__(self):
        super().__init__()
        self.rows = []
        self.ret = None

    def _on_step(self):
        if self.ret is None:
            self.ret = np.zeros(len(self.locals["dones"]))
        self.ret += self.locals["rewards"]
        for i, done in enumerate(self.locals["dones"]):
            if done:
                info = self.locals["infos"][i]
                self.rows.append(dict(step=int(self.num_timesteps), ret=float(self.ret[i]),
                                      won=bool(info["won"]), died=bool(info["died"]),
                                      length=int(info["episode"]["l"])))
                self.ret[i] = 0.0
        return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--steps", type=int, default=1_000_000, help="training step budget")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--envs", type=int, default=8)
    ap.add_argument("--out", default="runs/fighter")
    ap.add_argument("--gamma", type=float, default=0.995)
    ap.add_argument("--n-steps", type=int, default=1024, help="rollout length per env")
    ap.add_argument("--epochs", type=int, default=10)
    ap.add_argument("--guard-weight", type=float, default=1.0, help="reward weight for guard damage")
    a = ap.parse_args()

    torch.set_num_threads(1)
    os.makedirs(a.out, exist_ok=True)
    env = make_vec_env(FighterArena, n_envs=a.envs, seed=a.seed,
                       env_kwargs=dict(reward_weights=dict(guard_dmg=a.guard_weight)))
    model = PPO("MlpPolicy", env, seed=a.seed, verbose=0, device="cpu",
                n_steps=a.n_steps, batch_size=1024, n_epochs=a.epochs, learning_rate=3e-4,
                gamma=a.gamma, ent_coef=0.01,
                policy_kwargs=dict(net_arch=[64, 64]))
    n_params = sum(p.numel() for p in model.policy.parameters())
    print(f"policy params: {n_params}")

    log = EpisodeLog()
    t0 = time.time()
    model.learn(total_timesteps=a.steps, callback=log)
    secs = time.time() - t0
    model.save(os.path.join(a.out, "fighter"))
    with open(os.path.join(a.out, "train_log.json"), "w") as f:
        json.dump(dict(args=vars(a), steps=a.steps, seed=a.seed, seconds=secs, params=n_params,
                       episodes=log.rows), f)
    print(f"trained {a.steps} steps in {secs:.0f}s ({a.steps / secs:.0f} steps/s), "
          f"{len(log.rows)} episodes")


if __name__ == "__main__":
    main()
