"""Small deterministic arena: one fighter troop attacks a defended building.

The fighter must destroy the tower. Guards chase and hit it; the tower shoots
when it is in range. Everything is numpy, seeded, and Gymnasium-compatible so
the same code runs for training and for replays.
"""
import numpy as np
import gymnasium as gym
from gymnasium import spaces

ARENA = 20.0
MAX_TICKS = 400
N_GUARDS = 3

# Fighter
F_HP, F_SPEED, F_RANGE, F_DMG, F_CD = 100.0, 0.35, 1.5, 10.0, 4
# Guards
G_HP, G_SPEED, G_RANGE, G_DMG, G_CD, G_AGGRO, G_LEASH = 40.0, 0.25, 1.2, 3.0, 8, 6.0, 9.0
# Tower
T_HP, T_RANGE, T_DMG, T_CD = 200.0, 6.0, 4.0, 10

DIRS = np.array([[np.cos(a), np.sin(a)] for a in np.arange(8) * np.pi / 4])
# 0 idle, 1-8 move, 9 attack nearest guard in range, 10 attack tower
N_ACTIONS = 11
OBS_DIM = 5 + 4 + 5 * N_GUARDS


class FighterArena(gym.Env):
    metadata = {"render_modes": ["rgb_array"]}

    def __init__(self, reward_weights=None):
        super().__init__()
        self.action_space = spaces.Discrete(N_ACTIONS)
        self.observation_space = spaces.Box(-1.0, 1.0, shape=(OBS_DIM,), dtype=np.float32)
        # Reward is a weighted sum of named signals; hybrids will blend these later.
        self.w = dict(tower_dmg=1.0, guard_dmg=0.3, win=1.0, death=-1.0, tick=-0.001)
        if reward_weights:
            self.w.update(reward_weights)

    # ---------------------------------------------------------------- reset
    def reset(self, seed=None, options=None):
        super().reset(seed=seed)
        rng = self.np_random
        self.tick = 0
        self.tower_pos = np.array([ARENA / 2, ARENA / 2]) + rng.uniform(-2, 2, 2)
        self.tower_hp, self.tower_cd = T_HP, 0
        ang = rng.uniform(0, 2 * np.pi)
        self.f_pos = self.tower_pos + 8.5 * np.array([np.cos(ang), np.sin(ang)])
        self.f_pos = np.clip(self.f_pos, 0.5, ARENA - 0.5)
        self.f_hp, self.f_cd = F_HP, 0
        g_ang = rng.uniform(0, 2 * np.pi, N_GUARDS)
        self.g_post = self.tower_pos + 2.0 * np.stack([np.cos(g_ang), np.sin(g_ang)], 1)
        self.g_pos = self.g_post.copy()
        self.g_hp = np.full(N_GUARDS, G_HP)
        self.g_cd = np.zeros(N_GUARDS, dtype=int)
        self.events = []  # per-tick visual events for replays
        return self._obs(), {}

    # ----------------------------------------------------------------- step
    def step(self, action):
        action = int(action)
        self.tick += 1
        self.events = []
        tower_dmg = guard_dmg = 0.0
        self.f_cd = max(0, self.f_cd - 1)

        # Fighter acts
        if 1 <= action <= 8:
            self.f_pos = np.clip(self.f_pos + F_SPEED * DIRS[action - 1], 0, ARENA)
        elif action == 9 and self.f_cd == 0:
            d = self._guard_dists()
            d[self.g_hp <= 0] = np.inf
            i = int(np.argmin(d))
            if d[i] <= F_RANGE:
                hit = min(F_DMG, self.g_hp[i])
                self.g_hp[i] -= hit
                guard_dmg += hit
                self.f_cd = F_CD
                self.events.append(("f_hit", self.g_pos[i].copy()))
        elif action == 10 and self.f_cd == 0 and self.tower_hp > 0:
            if np.linalg.norm(self.tower_pos - self.f_pos) <= F_RANGE + 0.5:
                hit = min(F_DMG, self.tower_hp)
                self.tower_hp -= hit
                tower_dmg += hit
                self.f_cd = F_CD
                self.events.append(("f_hit", self.tower_pos.copy()))

        # Guards act: chase the fighter if it's close or attacking, else go home
        for i in range(N_GUARDS):
            if self.g_hp[i] <= 0:
                continue
            self.g_cd[i] = max(0, self.g_cd[i] - 1)
            to_f = self.f_pos - self.g_pos[i]
            dist = np.linalg.norm(to_f)
            home = np.linalg.norm(self.g_pos[i] - self.g_post[i])
            if dist <= G_RANGE:
                if self.g_cd[i] == 0:
                    self.f_hp -= G_DMG
                    self.g_cd[i] = G_CD
                    self.events.append(("g_hit", self.f_pos.copy()))
            elif dist <= G_AGGRO and home <= G_LEASH:
                self.g_pos[i] += G_SPEED * to_f / dist
            elif home > 0.1:
                v = self.g_post[i] - self.g_pos[i]
                self.g_pos[i] += min(G_SPEED, home) * v / home

        # Tower shoots
        self.tower_cd = max(0, self.tower_cd - 1)
        if self.tower_hp > 0 and self.tower_cd == 0 and np.linalg.norm(self.f_pos - self.tower_pos) <= T_RANGE:
            self.f_hp -= T_DMG
            self.tower_cd = T_CD
            self.events.append(("t_shot", self.f_pos.copy()))

        won = self.tower_hp <= 0
        died = self.f_hp <= 0
        timeout = self.tick >= MAX_TICKS
        r = (self.w["tower_dmg"] * tower_dmg / T_HP
             + self.w["guard_dmg"] * guard_dmg / (G_HP * N_GUARDS)
             + self.w["tick"]
             + (self.w["win"] if won else 0.0)
             + (self.w["death"] if died and not won else 0.0))
        info = {"won": won, "died": died, "tower_hp": self.tower_hp,
                "guards_alive": int((self.g_hp > 0).sum())}
        return self._obs(), float(r), bool(won or died), bool(timeout and not (won or died)), info

    # ------------------------------------------------------------ helpers
    def _guard_dists(self):
        return np.linalg.norm(self.g_pos - self.f_pos, axis=1)

    def _obs(self):
        o = [self.f_pos[0] / ARENA * 2 - 1, self.f_pos[1] / ARENA * 2 - 1,
             self.f_hp / F_HP, self.f_cd / F_CD, self.tick / MAX_TICKS]
        rel = (self.tower_pos - self.f_pos) / ARENA
        o += [rel[0], rel[1], self.tower_hp / T_HP, self.tower_cd / T_CD]
        # Guards sorted by distance, dead ones last and zeroed
        d = self._guard_dists()
        d = np.where(self.g_hp > 0, d, np.inf)
        for i in np.argsort(d, kind="stable"):
            if self.g_hp[i] > 0:
                rel = (self.g_pos[i] - self.f_pos) / ARENA
                o += [rel[0], rel[1], self.g_hp[i] / G_HP, self.g_cd[i] / G_CD, 1.0]
            else:
                o += [0.0, 0.0, 0.0, 0.0, 0.0]
        return np.clip(np.array(o, dtype=np.float32), -1, 1)

    def snapshot(self):
        """State needed to draw one frame of a replay."""
        return dict(tick=self.tick, f_pos=self.f_pos.copy(), f_hp=self.f_hp,
                    tower_pos=self.tower_pos.copy(), tower_hp=self.tower_hp,
                    g_pos=self.g_pos.copy(), g_hp=self.g_hp.copy(), events=list(self.events))


# ------------------------------------------------------------------ baselines
def random_policy(env, obs, rng):
    return int(rng.integers(N_ACTIONS))


def _move_toward(vec):
    ang = np.arctan2(vec[1], vec[0]) % (2 * np.pi)
    return 1 + int(np.round(ang / (np.pi / 4))) % 8


def rush_policy(env, obs, rng):
    """Walk to the tower and hit it, ignoring guards."""
    to_t = env.tower_pos - env.f_pos
    if np.linalg.norm(to_t) <= F_RANGE + 0.5:
        return 10
    return _move_toward(to_t)


def guards_first_policy(env, obs, rng):
    """Hand-written tactic: kill the nearest guard, then the tower."""
    alive = env.g_hp > 0
    if alive.any():
        d = np.where(alive, env._guard_dists(), np.inf)
        i = int(np.argmin(d))
        if d[i] <= F_RANGE:
            return 9
        return _move_toward(env.g_pos[i] - env.f_pos)
    return rush_policy(env, obs, rng)
