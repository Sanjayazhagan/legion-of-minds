# Legion of Minds: online backend

The server for online play: accounts and sign-in, published home bases, matchmaking, raids that the server re-runs to check, a battle log with replays, and a leaderboard. Rust (Axum + Postgres via sqlx). It uses the game's own `godcore` crate (read-only, by path) to re-run every raid, so a raid on the server plays out exactly as it did on the attacker's machine.

## How online raids work

0. **Pay online.** The server keeps what each player has bought with online gold: Town Hall level, weapons and each troop's level. Gold, Town Hall and troop levels in the save file only count offline. A base above your online Town Hall, a troop above its online level, or a weapon you haven't unlocked online (each class's own weapon is free) is refused, so editing a save file gets you nothing online.
1. **Publish.** After training, the game sends your home base (Town Hall level, buildings) and your placed defenders' brains. The server checks it and stores it as an immutable snapshot. That snapshot is what defends while you're offline.
2. **Find a match.** An attacker asks for an opponent. The server picks a published base within 200 trophies (then anyone), skipping shielded bases and bases already under attack, and reserves it for 5 minutes.
3. **Raid.** The attacker's game runs the raid locally (for the battle screen), then sends the army, placement and its own result.
4. **Check.** The server rebuilds every troop's stats from its class blend (whole percentages adding up to 100, one side only) and training plan, using the game's own blend code, so edited HP, damage or splash in a troop file does nothing, checks levels, training caps, brain size (any listed size up to 256; 384 to 1024 only at level 5, with that brain's training cap; bigger brains stay offline), weapon mastery against training time (mastery is recomputed from it), talent reach from the troop's level (a level-1 Healer heals one ally at a time) and placement, re-runs the raid with the same code, and records its own result. If the client's result differs, the raid is flagged (`client_matched: false`) and the server's result counts.
5. **Score.** Trophies: 0 stars −8, 1 star +10, 2 stars +20, 3 stars +30 (the defender gets the opposite). Gold: up to 20% of the defender's gold, scaled by destruction. A defender raided for 1+ star gets a 1-hour shield.
6. **Learn from it.** Both players can fetch the replay: the defender's snapshot and the attacker's army. Re-running it gives the same result, and the raiders' brains can go straight into defence training as self-play opponents (`godctl train --vs`).

Raids have no hidden randomness: the result depends only on the snapshot, the army and where it was placed.

## Run it locally

**With Docker** (from this folder):

```
docker compose up --build
```

This starts Postgres and the server on http://localhost:8080 with the test login on.

**Without Docker:** you need Rust and a Postgres you can reach (tested with Postgres 16).

```
createdb bag            # or any database; the server runs its migrations on start
DATABASE_URL=postgres://localhost/bag DEV_LOGIN=true cargo run --release -p bag-server
```

**Try it with bagctl** (in another terminal; troop files come from the game's exports folder or `godctl train`):

```
cargo run --release -p bag-client --bin bagctl -- --token-file alice.token login-dev Alice
cargo run --release -p bag-client --bin bagctl -- --token-file alice.token publish --th 3 guard.troop.json sentry.troop.json
cargo run --release -p bag-client --bin bagctl -- --token-file bob.token login-dev Bob
cargo run --release -p bag-client --bin bagctl -- --token-file bob.token raid fighter.troop.json fighter.troop.json healer.troop.json
cargo run --release -p bag-client --bin bagctl -- --token-file alice.token log defense
cargo run --release -p bag-client --bin bagctl -- --token-file alice.token replay <ATTACK_ID> --out raiders
```

Sample run with the test fixtures:

```
raiding Alice ...
local:  1 stars, 30% destroyed, fingerprint f1f8576b2ccb3918
server: 1 stars, 30% destroyed, fingerprint f1f8576b2ccb3918 (match: Some(true))
trophies +10, gold looted 29
1 stars, 30% destroyed; replay check: identical
```

**Tests** need a Postgres user that can create databases (each test makes its own):

```
TEST_DATABASE_URL=postgres://bag:bag@localhost/bag cargo test --release
```

They cover the full raid flow, tampered armies and fake results, invalid bases, sessions and logout, the test-login switch, and the whole Google-style OAuth flow against a fake provider (PKCE, state, one-time token handout, same account on second sign-in).

## Sign-in

- **Test login** (`DEV_LOGIN=true`): sign in with just a name; the same name gives the same account. For local play and tests only. It is **off** unless you turn it on.
- **Google and Discord OAuth.** Each turns on when its client ID and secret are set. The desktop game doesn't need a local web server: it calls `POST /v1/auth/login`, opens the returned URL in the browser, and polls `POST /v1/auth/logins/{id}/poll` with the secret it got. When the player finishes in the browser, the poll returns a session token. Uses PKCE and a random `state`; the token is handed out once.
- **Sessions** are random bearer tokens (`Authorization: Bearer bag_…`), 30 days. The database stores only their SHA-256. Logout deletes the session.
- Steam can be added later the same way (it uses OpenID 2.0 and a Web API key instead of OAuth).

## Hosting: what to set

The server is one container plus a Postgres database. Any host that runs a Dockerfile works (Fly.io, Railway, Render, AWS). Build from the **project root**, because the image needs `game/crates/godcore`:

```
docker build -f backend/Dockerfile -t legion-of-minds-server .
```

### Deploy to the Azure VM with deploy.ps1

`deploy.ps1` works like the portfolio site's: from PowerShell on your laptop it packs the server source into a tarball, uploads it over `scp` (with the same keepalive options) and runs `deploy/server.sh` on the VM over `ssh`, sent as one base64 token with Windows line endings removed. That script does the rest:

- **The server comes ready-made from GitHub.** The "Server build" workflow (`.github/workflows/server.yml`) cross-builds it for ARM (aarch64, what the VM is) and x86_64 on every push to `dev` or `main` that touches the server, checks that the ARM build starts and answers `/health`, and keeps it as the artifact `legion-server-linux` for 30 days. `deploy.ps1` downloads the build made from the same server code as your checkout with the GitHub CLI (`gh`), checks its checksums and uploads it, so the VM never compiles. If your branch is pushed but has no build yet, it asks GitHub for one and waits.
- **Fallback: building on the VM.** Without `gh`, with uncommitted server changes, or with `-BuildOnVm`, the VM builds from the uploaded source as before: it installs build tools and Rust and adds a swap file on a small VM so the build fits in memory. The VM does the same if the ready-made server doesn't run there.
- **First deploy only:** it installs Postgres and creates a database with a random password. The password stays in the VM's env file and never leaves the VM.
- **Every deploy:** it puts the new server in place, merges `deploy.env` into `<VmPath>/server.env`, restarts the `legion-server` systemd service and checks `/health`.
- **Before changing anything:** it backs up the Postgres database to `<VmPath>/db-backups/legion-<time>.dump` (newest 10 kept) and stops if the backup fails. The previous server binary is kept as `<VmPath>/bag-server.prev`.
- **Leaves the portfolio alone:** it never touches pm2 or the portfolio's folder. It stops if the port is taken by another program, or if another nginx site already uses the domain. It checks nginx with `nginx -t` before reloading and removes its own site again if the check fails.

```
winget install --id GitHub.cli           # once; then open a new PowerShell
gh auth login                            # once
cd backend
copy deploy.env.example deploy.env      # then fill in the OAuth keys
.\deploy.ps1                             # http://40.81.20.114:8080
.\deploy.ps1 -Domain game.example.com    # https, via nginx+certbot or Caddy
.\deploy.ps1 -Binary $HOME\Downloads\legion-server-linux.zip   # a build downloaded by hand from Actions
.\deploy.ps1 -BuildOnVm                  # build on the VM instead
```

To roll back to the previous server on the VM: `cp ~/legion-server/bag-server.prev ~/legion-server/bag-server && sudo systemctl restart legion-server` (and restore the database backup if the new server changed it).

The defaults are the portfolio's VM (`azureuser@40.81.20.114`, key `~\.ssh\id_ed25519`). Override them with `-VmHost`, `-VmUser`, `-SshKey`, `-VmPath` or `-Port`.

**Google sign-in needs `-Domain`.** Google only accepts https redirect addresses with a domain name, not a bare IP. Point the domain's DNS at the VM first. If nginx is already on the VM (for example for the portfolio), the script adds a site and gets a certificate with certbot. Otherwise it installs Caddy.

You also need to open the server's ports in the Azure portal (the VM's Networking settings): TCP 8080 inbound without a domain, or 80 and 443 with one.

Set these environment variables on the host (as secrets where the host has them). Never put real values in files or chat.

| Variable | Required | What to set |
|---|---|---|
| `DATABASE_URL` | yes | The host's Postgres connection string. Migrations run automatically on start. |
| `PUBLIC_URL` | yes for OAuth | The server's public https address, e.g. `https://bag-server.fly.dev`. No trailing slash. |
| `DEV_LOGIN` | — | Leave unset (off) on a public server. |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | for Google sign-in | From Google Cloud Console → APIs & Services → Credentials → OAuth client ID (type "Web application"). Authorized redirect URI: `<PUBLIC_URL>/v1/auth/google/callback`. |
| `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET` | for Discord sign-in | From the Discord Developer Portal → your app → OAuth2. Redirect: `<PUBLIC_URL>/v1/auth/discord/callback`. |
| `PORT` | — | Most hosts set it for you. Defaults to 8080. `BIND_ADDR` overrides both. |
| `SESSION_DAYS`, `ATTACK_WINDOW_SECS`, `SHIELD_MINUTES`, `START_GOLD`, `MATCH_TROPHY_RANGE`, `KEEP_RAIDS_DAYS` | — | Tuning; defaults 30, 300, 60, 500, 200, 14. Raids older than `KEEP_RAIDS_DAYS` leave the battle log (each keeps the whole army for its replay: about 0.4 MB with 64-neuron brains, 4 MB with 256, 30 MB with 1024). |
| `DEFENCE_REWARD`, `DEFENCE_REWARDS_PER_DAY`, `DAILY_WIN_BONUS`, `PRACTICE_GOLD`, `PRACTICE_PAID_PER_DAY` | — | Gold the server hands out; defaults 30, 5, 100, 20, 5. A defender who holds a raid to no stars gets `DEFENCE_REWARD` (at most `DEFENCE_REWARDS_PER_DAY` times in 24 hours); the first raid of the UTC day that wins a star pays `DAILY_WIN_BONUS`; a practice raid pays `PRACTICE_GOLD` x Town Hall x destruction, for the first `PRACTICE_PAID_PER_DAY` in 24 hours. 0 turns one off. |
| `REQUESTS_PER_MINUTE`, `RAIDS_PER_MINUTE` | — | Per-address limits; defaults 240 for everything and 20 for raids, publishing and sign-ins. Over the limit the server answers 429 `slow_down` with `Retry-After`. |
| `TRUST_PROXY` | — | `true` behind our own HTTPS proxy (deploy.ps1 sets it with `-Domain`): the client address comes from the last `X-Forwarded-For` entry. Leave off when the server is reached directly, or clients could fake their address. |
| `MAX_ONLINE_BRAIN` | — | Biggest brain (neurons per layer) taken online, default 1024. Set 256 on a small server (see "How many players"). |
| `DB_POOL` | — | Database connections, default 8. Each keeps buffers as big as the biggest base or army it has moved, so fewer use less memory: 6 on a 1 GB server. |
| `BATTLE_THREADS`, `HEAVY_AT_ONCE` | — | Battles re-run at once (default: all cores but one, at least 1) and raids, bases and cup entries worked on at once (default one more). The rest wait their turn, so a burst of raids can't take every core or run the server out of memory. |
| `RUST_LOG` | — | Log level, default `info,sqlx=warn`. |

Health check: `GET /health`. Serve it behind https (the host's TLS); tokens are bearer tokens.

## API

All bodies are JSON. Publishing, matchmaking and raids need the header `x-sim-version: <godcore::sim::SIM_VERSION>` (bag-client sends it); a game on different battle rules gets 409 `update_required`. Bases published under older rules are skipped by matchmaking until their owner publishes again, and replays carry the `sim_version` they were fought under. Errors are `{"error": code, "message": text}`, e.g. `unauthorized`, `invalid` (422, failed a check), `no_opponent`, `raid_closed`, `not_enough_gold`.

| Method and path | Auth | What it does |
|---|---|---|
| `GET /health` | | Liveness |
| `GET /v1/auth/providers` | | Which sign-ins are on |
| `POST /v1/auth/dev` `{name}` | | Test login → `{token, user}` |
| `POST /v1/auth/login` `{provider}` | | Start a browser sign-in → `{login_id, poll_secret, url}` |
| `POST /v1/auth/logins/{id}/poll` `{poll_secret}` | | `{status: "pending"}` or `{status: "done", token, user}` |
| `GET /v1/auth/{provider}/callback` | | Where the provider sends the browser back |
| `POST /v1/auth/logout` | ✓ | End this session |
| `GET /v1/me`, `PATCH /v1/me` `{display_name}` | ✓ | Profile: trophies, online gold, published Town Hall, shield, and `upgrades` (online Town Hall, weapons, troop levels) |
| `POST /v1/me/town-hall` | ✓ | Buy the next Town Hall level with online gold → profile |
| `POST /v1/me/weapons` `{weapon}` | ✓ | Unlock a weapon online → profile |
| `POST /v1/me/troops/{troop_id}/level` `{from}` | ✓ | Level a troop up online from `from` (its online level now, so a repeat doesn't pay twice) → profile |
| `PUT /v1/me/base`, `GET /v1/me/base` | ✓ | Publish / read your base snapshot |
| `POST /v1/matchmaking` | ✓ | Find and reserve an opponent → snapshot to fight. Needs a published base (409 `no_base`). Never comes back empty: when nobody can be raided it returns `kind: "practice"`, a copy of another player's base or the Training Camp's computer-made one (no trophies, the defender loses nothing) |
| `POST /v1/attacks/{id}/revenge` | ✓ | Raid back the attacker of a raid on you (`can_revenge` in the log), once; their shield doesn't count → snapshot to fight, `kind: "revenge"` |
| `GET /v1/raid-of-the-day` | ✓ | Replay of the best raid of the last 24 hours (stars, then destruction, then speed) |
| `GET /v1/cup` | ✓ | This week's cup: top 50, your entry, last week's podium |
| `POST /v1/cup/entry` `{troop}` | ✓ | Enter an attacker (or replace your entry). Scored with `godcore::battle::tournament`. Weeks run Monday to Monday UTC; when one closes the cleanup pays 500/300/200, 100 for the rest of the top ten and 25 to everyone else |
| `GET /v1/cup/{week}/{player}` | ✓ | An entry with its troop, to replay its cup raids |
| `POST /v1/attacks/{id}` `{army, client_result}` | ✓ | Submit the raid; the server re-runs and scores it. Shields last 25%, 50% or 100% of `SHIELD_MINUTES` for 1, 2 or 3 stars. `bonus_gold` is what the server added (first win of the day, practice) |
| `GET /v1/me/attacks?role=defense\|attack\|all` | ✓ | Battle log |
| `GET /v1/attacks/{id}/replay` | ✓ | Snapshot + army, for either player |
| `GET /v1/players/{id}` | ✓ | Public profile |
| `GET /v1/leaderboard?limit=` | | Top players by trophies |
| `GET /v1/live` | ✓ | Running and upcoming events, the season, and gifts waiting to be claimed |
| `POST /v1/me/gifts/{id}/claim` | ✓ | Claim a gift → `{gold, user}` |
| `/v1/admin/...` | admin key | The operator's controls, see "Running events, rewards and seasons" |

## Running events, rewards and seasons

The server has an admin page for the game's operator at `<PUBLIC_URL>/admin`. It is off until `ADMIN_KEY` is set (at least 20 characters; put it in `deploy.env`, never in the repo or chat). Open the page, paste the key, and from there:

- **Events**: start or schedule a time-limited boost every player sees on the Online screen, for example a double gold weekend (`gold_pct` 200) or "+50 gold per win". Running events add up. They only change gold, never battle rules, so raids still check out on the server. Extra gold is new gold: the defender only loses normal loot.
- **Give gold**: a gift to every player or one player (name or id), claimed in the game with one click; close it any time.
- **Season**: rename the running season, move its end, or end it now (pays every player by league, halves trophies above 800, starts the next one).
- **Rewards and timings**: season length, end-of-season gold per league, league win bonus, shield length, war rewards and starting gold. Changes apply at once; set a value back to its default to undo.

Every change is listed under Recent changes (table `admin_log`). The API behind it is under `/v1/admin` with the header `x-admin-key` (`overview`, `events`, `events/{id}/stop`, `gifts`, `gifts/{id}/cancel`, `season`, `season/end`, `settings`); `bag_client::Client::admin(key)` wraps it for scripts. Changing how battles play still needs a game release with a new SIM_VERSION.

## Layout

```
crates/protocol   wire types, all validation, run_raid (shared by server and client)
crates/server     Axum app: auth.rs, players.rs, attacks.rs, config.rs; tests/api.rs
crates/client     bag-client: blocking client for the desktop game, and the bagctl CLI
migrations        Postgres schema (users, identities, sessions, oauth_logins, snapshots, attacks)
```

## The game's sign-in and Online screens

The game opens on a sign-in screen: **Sign in with Google**, **Sign in with Discord** (each shown only when the server has it turned on), the test login when `DEV_LOGIN` is on, and **Play offline**. Google and Discord open the player's browser; the game polls until the sign-in finishes there, then carries on by itself. The session is kept in `account.json` in the save folder, so the next start signs in by itself. A saved session the server no longer knows sends the player back to the sign-in screen.

Signed in, the **Online** button in the barracks opens the Online screen:

- **Publish my base**: your home base (buildings and Town Hall) with your first four defenders that the Town Hall allows, placed as on the Home Base screen.
- **Find an opponent** and **Attack!**: your best attackers (up to five) that your published Town Hall allows. The game fights the raid, the server checks it, and the replay plays with the server's result.
- **Battle log** with **Watch** for every raid you made or got, and the top 10 players.

Code: `game/crates/app/src/online.rs`, using `bag-client` from here. Every server call runs on a background thread.

**Which server the game talks to:** `BAG_SERVER` when the game starts, else `BAG_SERVER` when the game was built, else `http://localhost:8080`. Build the release game with the hosted address:

```
BAG_SERVER=https://your-server.example cargo build --release -p legion-of-minds
```

**Try it on one machine** (Postgres running):

```
# terminal 1, in backend/
DATABASE_URL=postgres://localhost/bag DEV_LOGIN=true cargo run --release -p bag-server
# terminal 2, in game/
cargo run --release -p legion-of-minds
```

Type a name under "Test login" and sign in. To have someone to raid, sign in a second name with bagctl and publish a base (see above), or run a second copy of the game with its own save folder (`BAG_SAVE_DIR=/tmp/player2`).

Recordings and screenshot tests (`BAG_SCREEN`, `BAG_RECORD`, `BAG_SCREENSHOT`) skip the sign-in screen; `BAG_SCREEN=signin` opens it directly.

## How many players

Measured 2026-11-25 with `examples/loadtest.rs` against Postgres 16, the server pinned to 2 cores of a cloud x86 machine (an ARM VM core is in the same range; expect up to ~30% slower). Brains were untrained, so every raid ran the full 3 minutes: the worst case. Each raid is 5 attackers against 4 defenders.

| Brain size | One raid check (CPU) | Raids a minute, 2 cores | Stored per raid | Server memory, 10 raids at once |
|---|---|---|---|---|
| 64 | 0.2 s | ~400 | 0.3 MB | 70 MB |
| 256 | 1-2 s | ~50 (~40 on 1 core) | 4 MB | 250 MB (650 MB before the limits below) |
| 512 | ~3-4 s | ~24 | ~10 MB | 350 MB |
| 1024 | ~10 s | ~7 | ~33 MB | over 1 GB |

Postgres used another 250-370 MB. Everything else (log, leaderboard, cup, season, events) answered in milliseconds while raids ran, once battles were queued.

- **100 players a day** (~1,000 raids a day) fit easily on any small VM. Disk is the limit: with 256 brains, 14 days of battle log is ~50 GB, so keep `KEEP_RAIDS_DAYS=3` on a small disk.
- **100 players online at once** (a raid every ~3 minutes each, ~33 a minute) fit on 2 cores up to 256 brains. 1024 brains do not (7 a minute), so a small server sets `MAX_ONLINE_BRAIN=256`. Burstable VMs (Azure B-series) only give full CPU while their credits last.
- Battles are queued (`BATTLE_THREADS`), and only a few raids, bases and cup entries are read in at once (`HEAVY_AT_ONCE`), so a burst makes raids wait instead of slowing every screen or running out of memory. The game waits up to 2 minutes for an answer.

Settings for a 1 GB VM (in `deploy.env.example`): `MAX_ONLINE_BRAIN=256`, `KEEP_RAIDS_DAYS=3`, `DB_POOL=6`; the systemd unit sets `MALLOC_ARENA_MAX=2`.

```sh
# Server: DEV_LOGIN=true REQUESTS_PER_MINUTE=1000000 RAIDS_PER_MINUTE=1000000 (all players share one address)
cargo run --release -p bag-server --example raidbench -- 256 5 4          # one raid: brain size, attackers, defenders
cargo run --release -p bag-server --example loadtest -- http://127.0.0.1:8080 postgres://bag:bag@localhost/bag 256 100 10 60 90 cup
```

## Pretend players (bagsim)

`bagsim` plays the game with a crowd of pretend players against a local server, to find what breaks before real players do. Train one troop per class first, for example `godctl train --class mage --steps 290000 --level 1 --out troops/mage.troop.json` (attackers and defenders both), then:

```sh
# Server with DEV_LOGIN=true on 127.0.0.1:8080
cargo run --release -p bag-client --bin bagsim -- crowd --troops troops --players 150 --secs 150 --chaos --out crowd.json
cargo run --release -p bag-client --bin bagsim -- matrix --troops troops       # every army vs every base, Town Hall 1-5
cargo run --release -p bag-client --bin bagsim -- economy --troops troops --players 200 --days 30
```

- `crowd`: each player picks an army and a defence, publishes a base, raids, buys Town Halls, reads the log and the leaderboard. `--chaos` adds players who misbehave (old game versions, broken and huge uploads, the same raid or purchase sent 4 times at once, tampered troops, cut-off connections) and lists every answer that wasn't what a healthy server should say. Prints speed per call and stars per army.
- `matrix` and `economy` need no server: they run the same battle code to show which armies beat which bases, and where online gold goes over many days.

Every pretend player signs in from the same address, so with the rate limits of PR #21 on they share one allowance.

## Not done yet

- **Online raids use your local troops as they are.** There is no army picker yet: the Online screen sends your best attackers and first four defenders.
- **Online gold comes from raids, the daily first win, held defences, practice raids and the weekly cup.** Campaign battles and defence waves are fought on the player's machine, so their gold stays offline. The server could re-run them (it already re-runs raids) to pay online gold too.
- **The raid of the day and cup entries hand out brains.** Anyone can download the featured army and any cup troop (that is how they are replayed). Decide later whether to send recordings instead.
- **The cup is troop against the same seeded drills, not troop against troop.** Scores don't depend on who else entered.
- **Recruiting and buildings are still local.** They don't need paying for online: the server checks a base's buildings against its Town Hall, and a fresh recruit is level 1.
- **Training can't be verified.** The server checks a troop's level and training cap, but a brain trained longer elsewhere would still pass. Signing training runs, or capping by rank, is a later problem.
- Clans, wars, friend challenges, Steam login and an admin view. Rate limits are per address, not per account.
- Replays let the defender download the attacker's army brains (that's what makes "train against this raid" work). Decide later whether that's fair to attackers.
