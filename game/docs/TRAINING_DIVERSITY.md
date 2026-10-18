# Does training make troops differ? (test, 2026-10-18)

One troop mix (Fighter 60 / Assassin 40) was trained 5 ways: the default plan, a building rush, a troop hunter, a survivor, and sparring against trained defenders. Each way was trained twice with different random seeds. Every troop then raided all 4 drill bases from 2 sides and a trained defence (guard, warlock, guard) from 3 sides, and its moves were counted.

The "gap" compares how different two troops trained with *different plans* are, against two troops trained with the *same plan* but a different seed. A gap of 1.0 means the plan made no more difference than luck; 2.0 means it made twice as much.

| Setup | Training | Behaviour gap | Result gap | Avg raid destruction | Vs trained defence |
|---|---|---|---|---|---|
| Old 12 moves | 300k | 1.39 | 1.40 | 56% | 26% |
| Current 15 moves (dodge, block, skill) | 300k | 1.15 | 1.07 | 64% | 25% |
| 17 moves (+ focus weakest, hit towers first) | 300k | 1.08 | 1.08 | 64% | 16% |
| Current 15 moves | 1.2M | 1.42 | 1.00 | 74% | 32% |

What it shows:
- The new moves made troops stronger (raids 56% to 64% destroyed) but not more different from each other. At a normal budget, luck shapes a troop about as much as its plan.
- Two more targeting moves made things slightly worse: more choices to learn with the same budget, so troops were weaker against a trained defence and no more varied.
- Longer training makes troops behave more differently (survivors kite and dodge, rushers trade hits) and much stronger, but they still score about the same: every plan converges on the one best way to raid these bases.

Why: the game currently has one best strategy. Troops only end up heavily different when different styles win in different situations, so that no single style beats everything.

What would create real differences:
1. **Counter-play between moves.** For example, a slow heavy attack that breaks a block and can be dodged, while block stops normal hits and dodge leaves you open to fast follow-ups. Which habits pay off then depends on the sparring partners you trained against.
2. **Bases and defences that need different styles.** Splash defences punish clumping, tower-heavy bases reward rushers, guard-heavy bases reward hunters.
3. **Matchups decided by training,** for example in tournaments between players' own troops.

Raw data: exp runs in the Rust game thread's scratch space (not kept); re-run with `godctl train` plus the plans above.
