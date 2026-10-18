# Legion of Minds

A desktop strategy game in the spirit of Clash of Clans, where every troop is a small reinforcement-learning agent that you train yourself on your own CPU. You blend classes into your own unique troops, train them with your own plan, then send them to raid bases or hold your own. Defenders are trained the same way. (The game's working title in the code is "Become a God".)

## What's in this repo

| Folder | What it is | Start here |
| --- | --- | --- |
| [`game/`](game) | The desktop game (Rust). `crates/godcore` is the simulation and RL training, `crates/app` is the 3D game client. | [game/README.md](game/README.md) |
| [`backend/`](backend) | The online server (Rust, Axum, Postgres): accounts, published bases, matchmaking, server-side raid verification, replays, leaderboard. | [backend/README.md](backend/README.md) |
| [`prototype/`](prototype) | The first Python RL prototype: one trained fighter against a defended tower. Kept for reference. | [prototype/README.md](prototype/README.md) |

The game and backend live together because the backend uses the game's own `godcore` crate (by path, `../game/crates/godcore`) to re-run every raid, so a raid plays out on the server exactly as it did on the player's machine. Any change to `godcore` that changes the simulation needs the backend re-checked.

Screen recordings of each game mode are not kept in git (`game/videos/` is ignored); they live in the project's shared files and can be re-recorded with `game/tools/record.sh`.
