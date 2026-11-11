# Credits

All third-party art is CC0 (public domain). No attribution is required, but credit is given gladly.

| Pack | Author | License | Source | Used for |
|---|---|---|---|---|
| KayKit Adventurers 1.0 | Kay Lousberg (www.kaylousberg.com) | CC0 1.0 | https://github.com/KayKit-Game-Assets/KayKit-Character-Pack-Adventures-1.0 | Barbarian, Knight, Mage, Rogue, hooded Rogue, weapons, quiver, shield, spellbook, arrow |
| KayKit Skeletons 1.0 | Kay Lousberg | CC0 1.0 | https://github.com/KayKit-Game-Assets/KayKit-Character-Pack-Skeletons-1.0 | Raiders and AI base guards, skeleton weapons |
| KayKit Medieval Hexagon 1.0 | Kay Lousberg | CC0 1.0 | https://github.com/KayKit-Game-Assets/KayKit-Medieval-Hexagon-Pack-1.0 | Castle (Town Hall), towers, rubble, cannonball, trees, rocks, hills |

Each pack's license file is kept next to its models in `assets/`. Unused animations were removed from the character files to keep them small; no geometry was changed. Extra clips (swing variants, Death_B, Hit_B, and the skeletons' Death_C_Skeletons, Spawn_Ground_Skeletons and Taunt) were copied back in from the same packs.

Built in code for this game: the ground and arena grid, cannon barrels, the healer halo, team rings, HP bars and effects, and the 2D fallback art in `crates/app/src/art.rs`.

## Code

Battle clips are compressed with [OpenH264](https://github.com/cisco/openh264) (BSD 2-Clause, Copyright (c) 2013, Cisco Systems), built from source into the game through the `openh264` Rust crate (BSD 2-Clause, Ralf Biedert). The installer should ship this notice with the game.
