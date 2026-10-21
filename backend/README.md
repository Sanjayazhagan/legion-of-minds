# Become a God: online backend

The server for online play: accounts and sign-in, published home bases, matchmaking, raids that the server re-runs to check, a battle log with replays, and a leaderboard. Rust (Axum + Postgres via sqlx). It uses the game's own `godcore` crate (read-only, by path) to re-run every raid, so a raid on the server plays out exactly as it did on the attacker's machine.

## How online raids work

1. **Publish.** After training, the game sends your home base (Town Hall level, buildings) and your placed defenders' brains. The server checks it and stores it as an immutable snapshot. That snapshot is what defends while you're offline.
2. **Find a match.** An attacker asks for an opponent. The server picks a published base within 200 trophies (then anyone), skipping shielded bases and bases already under attack, and reserves it for 5 minutes.
3. **Raid.** The attacker's game runs the raid locally (for the battle screen), then sends the army, placement and its own result.
4. **Check.** The server rebuilds every troop's stats from its class blend (whole percentages adding up to 100, one side only) and training plan, using the game's own blend code, so edited HP, damage or splash in a troop file does nothing, checks levels, training caps, brain size, weapon mastery against training time (mastery is recomputed from it), talent reach from the troop's level (a level-1 Healer heals one ally at a time) and placement, re-runs the raid with the same code, and records its own result. If the client's result differs, the raid is flagged (`client_matched: false`) and the server's result counts.
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
docker build -f backend/Dockerfile -t become-a-god-server .
```

Set these environment variables on the host (as secrets where the host has them). Never put real values in files or chat.

| Variable | Required | What to set |
|---|---|---|
| `DATABASE_URL` | yes | The host's Postgres connection string. Migrations run automatically on start. |
| `PUBLIC_URL` | yes for OAuth | The server's public https address, e.g. `https://bag-server.fly.dev`. No trailing slash. |
| `DEV_LOGIN` | — | Leave unset (off) on a public server. |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | for Google sign-in | From Google Cloud Console → APIs & Services → Credentials → OAuth client ID (type "Web application"). Authorized redirect URI: `<PUBLIC_URL>/v1/auth/google/callback`. |
| `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET` | for Discord sign-in | From the Discord Developer Portal → your app → OAuth2. Redirect: `<PUBLIC_URL>/v1/auth/discord/callback`. |
| `PORT` | — | Most hosts set it for you. Defaults to 8080. `BIND_ADDR` overrides both. |
| `SESSION_DAYS`, `ATTACK_WINDOW_SECS`, `SHIELD_MINUTES`, `START_GOLD`, `MATCH_TROPHY_RANGE` | — | Tuning; defaults 30, 300, 60, 500, 200. |
| `RUST_LOG` | — | Log level, default `info,sqlx=warn`. |

Health check: `GET /health`. Serve it behind https (the host's TLS); tokens are bearer tokens.

## API

All bodies are JSON. Publishing, matchmaking and raids need the header `x-sim-version: <godcore::sim::SIM_VERSION>` (bag-client sends it); a game on different battle rules gets 409 `update_required`. Bases published under older rules are skipped by matchmaking until their owner publishes again, and replays carry the `sim_version` they were fought under. Errors are `{"error": code, "message": text}`, e.g. `unauthorized`, `invalid` (422, failed a check), `no_opponent`, `raid_closed`.

| Method and path | Auth | What it does |
|---|---|---|
| `GET /health` | | Liveness |
| `GET /v1/auth/providers` | | Which sign-ins are on |
| `POST /v1/auth/dev` `{name}` | | Test login → `{token, user}` |
| `POST /v1/auth/login` `{provider}` | | Start a browser sign-in → `{login_id, poll_secret, url}` |
| `POST /v1/auth/logins/{id}/poll` `{poll_secret}` | | `{status: "pending"}` or `{status: "done", token, user}` |
| `GET /v1/auth/{provider}/callback` | | Where the provider sends the browser back |
| `POST /v1/auth/logout` | ✓ | End this session |
| `GET /v1/me`, `PATCH /v1/me` `{display_name}` | ✓ | Profile: trophies, gold, Town Hall, shield |
| `PUT /v1/me/base`, `GET /v1/me/base` | ✓ | Publish / read your base snapshot |
| `POST /v1/matchmaking` | ✓ | Find and reserve an opponent → snapshot to fight |
| `POST /v1/attacks/{id}` `{army, client_result}` | ✓ | Submit the raid; the server re-runs and scores it |
| `GET /v1/me/attacks?role=defense\|attack\|all` | ✓ | Battle log |
| `GET /v1/attacks/{id}/replay` | ✓ | Snapshot + army, for either player |
| `GET /v1/players/{id}` | ✓ | Public profile |
| `GET /v1/leaderboard?limit=` | | Top players by trophies |

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
BAG_SERVER=https://your-server.example cargo build --release -p become-a-god
```

**Try it on one machine** (Postgres running):

```
# terminal 1, in backend/
DATABASE_URL=postgres://localhost/bag DEV_LOGIN=true cargo run --release -p bag-server
# terminal 2, in game/
cargo run --release -p become-a-god
```

Type a name under "Test login" and sign in. To have someone to raid, sign in a second name with bagctl and publish a base (see above), or run a second copy of the game with its own save folder (`BAG_SAVE_DIR=/tmp/player2`).

Recordings and screenshot tests (`BAG_SCREEN`, `BAG_RECORD`, `BAG_SCREENSHOT`) skip the sign-in screen; `BAG_SCREEN=signin` opens it directly.

## Not done yet

- **Online raids use your local troops as they are.** There is no army picker yet: the Online screen sends your best attackers and first four defenders.
- **The game's economy is still local.** Gold, Town Hall level and troop levels live in the player's save file. The server keeps its own trophies and online gold, and trusts the Town Hall level in the published base (it checks buildings and troop levels against it). Moving progression onto the server is the next step against cheating.
- **Training can't be verified.** The server checks a troop's level and training cap, but a brain trained longer elsewhere would still pass. Signing training runs, or capping by rank, is a later problem.
- **Weapon ownership isn't checked.** Weapons are bought with gold in the game's save file, so the server accepts any weapon a troop holds (its mastery is still checked). This goes with moving the economy onto the server.
- Clans, wars, friend challenges, Steam login, rate limiting and an admin view.
- Replays let the defender download the attacker's army brains (that's what makes "train against this raid" work). Decide later whether that's fair to attackers.
