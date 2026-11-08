# Releasing

## The battle rules are locked

Since launch, the battle rules are frozen at **SIM_VERSION 8**. Players' trained troops, every published base and the server's raid checks all depend on them: a battle has to replay bit for bit on the player's machine and on the server.

A change counts as a rules change if it changes any battle result (the golden fingerprint in `crates/godcore/tests/determinism.rs`), what a troop sees (`OBS_DIM`) or what it can do (`N_ACTIONS`). The `rules_are_locked` test fails on any of these.

Things that are **not** rules changes, and can ship any time: art, animation, sound, UI, training speed, new screens, the server's economy, anything in `crates/app`.

If a rules change is really needed:

1. Bump `SIM_VERSION` in `crates/godcore/src/sim.rs` and update the numbers in `rules_are_locked` and the backend's protocol test (`team_training_is_kept_and_raids_match`).
2. Old troop files must keep loading: add the migration to `Troop::load_json` (it already grows brains for new inputs). Say in the release notes how much retraining is needed.
3. Release the server and the game **together**. The server only accepts the current version, so old games get "please update", and bases published under the old rules stop being matched until their owners publish again.

## A release, step by step

1. All tests green (CI runs them on every pull request).
2. Bump `version` in `game/Cargo.toml` (`[workspace.package]`).
3. Tag it: `git tag v0.2.0 && git push origin v0.2.0`. The Release workflow builds the Windows installer and zip and attaches them to a GitHub Release, which is what the launch page's Download button points at.
4. If the server changed: on the VM, `cd backend; .\deploy.ps1` (see `backend/README.md`).

To build the Windows installer by hand, see `installer/README.md`.
