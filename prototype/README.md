# Fighter prototype

One RL-trained fighter troop attacks a defended tower. This tests whether a tiny policy, trained on a laptop CPU in minutes, learns a real tactic.

## The arena

- 20x20 continuous arena, simulated in numpy (`arena.py`), Gymnasium interface, fully seeded. The same seed and model always replay the same battle (checked).
- **Fighter** (the trained troop): 100 HP, melee 10 damage, 4-tick cooldown.
- **Tower**: 200 HP, shoots the fighter for 4 damage every 10 ticks inside a 6-unit radius.
- **3 guards**: 40 HP each, chase the fighter when it comes close, 3 damage every 8 ticks.
- **Win** = tower destroyed. **Lose** = fighter dies, or 400 ticks pass.
- **Observation**: 24 numbers (own position/HP/cooldown, tower relative position/HP, the 3 guards sorted by distance).
- **Actions**: 11 (idle, 8 move directions, hit nearest guard, hit tower).
- **Reward**: weighted sum of named signals: damage to tower, damage to guards, +1 win, -1 death, -0.001 per tick. Hybrid classes will later blend these weights.

The arena is tuned so that rushing the tower always loses and killing the guards first always wins. Learning has to discover that order.

## Results (500 unseen seeds)

| Policy | Win | Died | Timed out | HP left on win |
|---|---|---|---|---|
| Untrained (random actions) | 0% | 31% | 69% | n/a |
| Scripted: rush the tower | 0% | 100% | 0% | n/a |
| Scripted: guards first, then tower | 100% | 0% | 0% | 18 |
| **Trained, v3 (final)** | **95.6%** | 4.4% | 0% | 18 |
| Trained, v2 (guard damage weighted 0.3) | 71% | 28% | 1% | 22 |
| Trained, v1 (first try, short-horizon PPO) | 11% | 87% | 2% | 18 |

All three trained variants use the same 12,300-parameter MLP (two layers of 64) and 1M training steps. Each run took about 3 minutes on one CPU core (~5,500 steps/s, 8 parallel envs). The saved model is 181 KB including optimizer state.

`runs/v3_g0995_guardw1/learning_curve.png` and `replay_trained.gif` / `replay_untrained.gif` show it.

## What training looked like

v3, by phase:

| Steps | Win | Died | Timed out |
|---|---|---|---|
| 0 to 100k | 0% | 19% | 81% |
| 100k to 500k | 0% | ~2% | ~98% |
| 500k to 700k | 44% | 25% | 31% |
| 700k to 1M | 94% | 3% | 3% |

1. **It learned to kill guards early** (by about 150k steps).
2. **Then came a long plateau, which was a reward exploit:** after killing the guards it **stayed out of the tower's range until time ran out**. Dying costs -1, timing out only costs about -0.4 in tick penalties, so hiding was safer than fighting the tower. Every variant went through this phase.
3. Around 500k steps it found that the tower is beatable once the guards are dead, and the win rate jumped to ~95%.

What fixed it:

- **Longer horizon** (gamma 0.99 → 0.995, longer rollouts, more epochs per update): v1 stayed stuck in the hiding phase almost all the way to 1M steps. With the change, the win reward is visible from far enough back.
- **Weighting guard damage equal to tower damage** (v3 vs v2): this made the "clear guards" stage pay off more clearly, and the policy committed to it.

## Other things noticed

- **It does not kite.** It fights the guards inside tower range and soaks tower fire, the same as the scripted bot. It ends with similar HP (~18). Pulling guards out of range would be better, but nothing in the reward pushes for it, so it doesn't happen within 1M steps.
- **No collision with buildings.** The fighter can stand on top of the tower while hitting it. This is a sim gap to fix before the Rust version.
- It spams attack actions while on cooldown (harmless no-ops). A real action mask would make learning faster.

## Takeaways for the game design

- "Longer training = better troop" holds, but **progress comes in sudden jumps after plateaus, not smoothly**. A player watching a stuck troop for 400k steps would think nothing is happening. The UI should show sub-skills learned (e.g. "kills guards ✓, attacks tower ✗").
- **Reward design is where the work is.** The reward weights decided whether the troop learns at all. That's what the class and hybrid system will tune, so a hybrid is a blend of tested weights, not something players set raw.
- **Exploits like "hide until time runs out" will show up in every class.** War rules should make timeouts count as losses for the attacker (like CoC does).
- **Compute is a non-issue at this size:** 3 minutes per 1M steps on one core.

## Reproduce

```
python3 -m pip install gymnasium stable-baselines3 matplotlib pillow
python3 train.py --steps 1000000 --out runs/fighter        # v3 settings are the defaults
python3 train.py --guard-weight 0.3 --out runs/v2          # v2
python3 train.py --gamma 0.99 --n-steps 256 --epochs 4 --guard-weight 0.3 --out runs/v1   # v1
python3 evaluate.py --run runs/fighter
```
