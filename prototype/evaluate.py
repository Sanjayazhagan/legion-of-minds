"""Compare the trained fighter with baselines, plot learning, render a replay GIF.

    python3 evaluate.py --run runs/v3_g0995_guardw1
"""
import argparse, json, os
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from PIL import Image, ImageDraw
from stable_baselines3 import PPO

from arena import (FighterArena, ARENA, F_HP, G_HP, T_HP, T_RANGE, N_ACTIONS,
                   random_policy, rush_policy, guards_first_policy)

EVAL_SEEDS = range(10_000, 10_500)  # never seen in training


def run_episode(env, policy, seed, rng, record=False):
    obs, _ = env.reset(seed=seed)
    frames, actions, ret, done = [env.snapshot()] if record else [], [], 0.0, False
    while not done:
        a = policy(env, obs, rng)
        actions.append(a)
        obs, r, term, trunc, info = env.step(a)
        ret += r
        done = term or trunc
        if record:
            frames.append(env.snapshot())
    return dict(ret=ret, won=info["won"], died=info["died"], ticks=env.tick,
                hp_left=max(env.f_hp, 0), actions=actions, frames=frames)


def evaluate(policy, n=None):
    env, rng = FighterArena(), np.random.default_rng(0)
    eps = [run_episode(env, policy, s, rng) for s in list(EVAL_SEEDS)[:n]]
    wins = [e for e in eps if e["won"]]
    acts = np.bincount(np.concatenate([e["actions"] for e in eps]), minlength=N_ACTIONS)
    return dict(win_rate=len(wins) / len(eps), mean_return=float(np.mean([e["ret"] for e in eps])),
                died=float(np.mean([e["died"] for e in eps])),
                timeouts=float(np.mean([not e["won"] and not e["died"] for e in eps])),
                ticks_to_win=float(np.mean([e["ticks"] for e in wins])) if wins else None,
                hp_left_on_win=float(np.mean([e["hp_left"] for e in wins])) if wins else None,
                action_share=(acts / acts.sum()).round(3).tolist())


def draw(frame, title, scale=24):
    W = int(ARENA * scale)
    img = Image.new("RGB", (W, W + 30), (32, 36, 44))
    d = ImageDraw.Draw(img)
    P = lambda p: (p[0] * scale, W - p[1] * scale)
    tx, ty = P(frame["tower_pos"])
    rr = T_RANGE * scale
    d.ellipse([tx - rr, ty - rr, tx + rr, ty + rr], outline=(90, 60, 60))
    if frame["tower_hp"] > 0:
        d.rectangle([tx - 14, ty - 14, tx + 14, ty + 14], fill=(170, 120, 60))
        d.rectangle([tx - 14, ty - 20, tx - 14 + 28 * frame["tower_hp"] / T_HP, ty - 17], fill=(240, 200, 80))
    else:
        d.rectangle([tx - 14, ty - 14, tx + 14, ty + 14], outline=(110, 90, 60))
    for p, hp in zip(frame["g_pos"], frame["g_hp"]):
        if hp > 0:
            x, y = P(p)
            d.ellipse([x - 7, y - 7, x + 7, y + 7], fill=(210, 70, 70))
            d.rectangle([x - 7, y - 12, x - 7 + 14 * hp / G_HP, y - 10], fill=(240, 120, 120))
    x, y = P(frame["f_pos"])
    d.ellipse([x - 8, y - 8, x + 8, y + 8], fill=(80, 170, 240))
    d.rectangle([x - 8, y - 14, x - 8 + 16 * max(frame["f_hp"], 0) / F_HP, y - 12], fill=(120, 220, 255))
    for kind, p in frame["events"]:
        ex, ey = P(p)
        col = {"f_hit": (255, 255, 255), "g_hit": (255, 90, 90), "t_shot": (255, 200, 60)}[kind]
        if kind == "t_shot":
            d.line([tx, ty, ex, ey], fill=col, width=2)
        else:
            d.ellipse([ex - 11, ey - 11, ex + 11, ey + 11], outline=col, width=2)
    d.text((8, W + 8), f"{title}   tick {frame['tick']}   fighter hp {max(frame['f_hp'], 0):.0f}",
           fill=(230, 230, 230))
    return img


def save_gif(frames, title, path, every=2):
    imgs = [draw(f, title) for f in frames[::every]] + [draw(frames[-1], title)] * 15
    imgs[0].save(path, save_all=True, append_images=imgs[1:], duration=50, loop=0)


def plot_curve(log, baselines, path):
    rows = log["episodes"]
    steps = np.array([r["step"] for r in rows])
    won = np.array([r["won"] for r in rows], float)
    ret = np.array([r["ret"] for r in rows])
    k = 100
    sm = lambda x: np.convolve(x, np.ones(k) / k, mode="valid")
    fig, ax = plt.subplots(1, 2, figsize=(11, 4))
    ax[0].plot(steps[k - 1:], sm(ret), color="#3b82f6")
    ax[0].set(title="Episode return (training, 100-ep avg)", xlabel="training steps")
    ax[1].plot(steps[k - 1:], sm(won) * 100, color="#16a34a", label="trained fighter")
    for name, c in [("guards_first", "#9ca3af"), ("rush", "#ef4444")]:
        ax[1].axhline(baselines[name]["win_rate"] * 100, ls="--", color=c, label=f"scripted: {name}")
    ax[1].set(title="Win rate (training, 100-ep avg)", xlabel="training steps", ylabel="%", ylim=(-3, 103))
    ax[1].legend(loc="center right")
    for a in ax:
        a.grid(alpha=0.3)
    fig.tight_layout()
    fig.savefig(path, dpi=110)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", default="runs/v3_g0995_guardw1")
    a = ap.parse_args()
    model = PPO.load(os.path.join(a.run, "fighter"), device="cpu")
    trained = lambda env, obs, rng: int(model.predict(obs, deterministic=True)[0])

    results = {"untrained (random)": evaluate(random_policy),
               "scripted: rush tower": evaluate(rush_policy),
               "scripted: guards first": evaluate(guards_first_policy),
               "trained fighter": evaluate(trained)}
    with open(os.path.join(a.run, "eval.json"), "w") as f:
        json.dump(results, f, indent=1)
    for k, v in results.items():
        print(f"{k:24s} win {v['win_rate']:.1%}  died {v['died']:.1%}  timeout {v['timeouts']:.1%}  "
              f"ret {v['mean_return']:+.2f}  ticks-to-win {v['ticks_to_win']}  hp-left {v['hp_left_on_win']}")
        print(f"{'':24s} action share {v['action_share']}")

    log = json.load(open(os.path.join(a.run, "train_log.json")))
    plot_curve(log, {"guards_first": results["scripted: guards first"],
                     "rush": results["scripted: rush tower"]}, os.path.join(a.run, "learning_curve.png"))

    env, rng = FighterArena(), np.random.default_rng(0)
    seed = EVAL_SEEDS[0]
    save_gif(run_episode(env, trained, seed, rng, record=True)["frames"], "trained",
             os.path.join(a.run, "replay_trained.gif"))
    save_gif(run_episode(env, random_policy, seed, rng, record=True)["frames"], "untrained",
             os.path.join(a.run, "replay_untrained.gif"))


if __name__ == "__main__":
    main()
