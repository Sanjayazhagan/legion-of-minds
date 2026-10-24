# How the RL works

Every troop has its own small brain that learns by trial and error on your computer. Nothing is downloaded or pre-trained: a new recruit starts out acting at random and gets better only from its own practice.

## 1. What a troop sees

Battles run in ticks (20 per battle second, shown at 6 a second when you watch). Every tick, each trained troop gets a list of 109 numbers describing its surroundings (the last 9, added in sim version 7, are calls from teammates: section 13). Since sim version 6 it only knows what its senses tell it (section 12), so "nearest" below means nearest of the ones it has seen:

- **Itself:** where it is on the map, its health, whether its weapon is ready, how much time is left, and whether it can hit a unit, hit a building or heal right now.
- **The 2 nearest buildings:** where they are, how damaged they are, and whether each one is a tower. Attackers see the buildings they are raiding; defenders see the ones they protect.
- **The 3 nearest enemies:** where they are, how hurt they are, and whether they are about to strike.
- **The 2 nearest allies, plus the most injured ally nearby.** Healers use this. For each of the 2 nearest allies it also sees their role (tank, striker, ranged or healer), whether something is attacking them, and whether they are fighting or healing right now (section 10). It also sees which of the two it could heal right now and how far its talents reach (section 11).
- **The overall picture:** the share of enemy buildings and enemy troops still standing.
- **Its senses (section 12):** which way it is looking, how fresh its knowledge of each of those buildings and enemies is, and the direction and loudness of the nearest fight it can hear.
- **Its moves and weapon:** which class moves it has and whether each is ready, whether its weapon skill is ready, whether its guard is up, cloaked or shielded by a ward, how many enemies are close enough for a spin or slam, whether the nearest enemies are stunned or blocking, and which weapon it holds.

Positions are relative ("a guard 3 steps to my left"), so the same brain works on any base layout.

## 2. What a troop can do

It has 24 actions, borrowed from action RPGs and MOBAs. Seven are class moves, and a troop can only use the ones its classes bring (section 9):

| Action | What it does |
|---|---|
| Stand still, or move in one of 8 directions | |
| Hit unit / hit building | Attack the nearest enemy unit or building in range |
| Heal ally 1 / Heal ally 2 | Heal the nearest or the second nearest ally (healing classes). The troop picks whom; the game does not pick for it (section 11) |
| **Weapon skill** | The weapon's special move, on its own cooldown (section 8) |
| **Dodge** | Roll away from the nearest threat, untouchable for a moment |
| **Block** | Raise the guard for 0.4 s: take much less damage, but no attacks and half speed |
| **Charge** | Rush up to 3.5 steps at an enemy just out of reach, hit it and stun it briefly |
| **Cloak** | Vanish: towers, guards and troops can't target you until you strike |
| **Blink** | Teleport away from the nearest threat |
| **Snare** | Drop a hidden trap that hurts and stuns the first enemy to step on it |
| **Ward** | Shield every ally close by: they take less damage for 1.5 s |
| Call: focus / fall back / help | Shout to nearby teammates while carrying on; only with a teammate in range (section 13) |

Actions that make no sense right now are blocked, such as attacking out of range, dodging with no enemy near, or using a skill that would hit nothing. The brain only ever chooses between moves that are possible.

Class stats and the weapon decide what those actions achieve: speed, health, range, damage, cooldown, heal power and splash. The brain decides **when, where and at what**, for example to block when a tower is about to fire, or to save Whirlwind until three guards are close.

## 3. The brain

The brain is a small neural network: the 90 numbers go in, pass through two layers of "neurons", and come out as a score for each of the 21 actions. Those weights are what training changes. **The player picks the brain size** when recruiting (64, 96, 128, 160, 192 or 256 neurons per layer) and can grow it later in the barracks:

| Neurons per layer | Numbers in the brain | Training speed (4-core test machine) |
|---|---|---|
| 64 | 11,156 | about 7,500 steps/s |
| 96 | 19,796 | about 4,600 steps/s |
| 128 | 30,484 | about 2,900 steps/s |
| 160 | 43,220 | about 2,300 steps/s |
| 192 | 58,004 | not measured |
| 256 | 93,716 | about 1,050 steps/s |

Growing keeps everything the troop learned: the new neurons start with no say in its choices and are trained in from there. A brain never shrinks. Troops from before the choice existed keep growing with their level as they used to (64 at levels 1-2, then 96, 128, 160).

What we measured comparing 160 and 256 at level 5 (Fighter, Mage and a Fighter/Archer/Assassin mix, 900k steps, half the drills against trained defenders, 2 seeds each):

- The bigger brain trains about 2.2 times slower and starts behind (Mage at 300k steps: 30% drill wins vs 44%), then catches up by 900k steps.
- After that, neither size is reliably better. The first seed made 256 look stronger against trained defenders (10 to 13 points more of the base destroyed), but the second seed reversed it for the Mage and the mix. The gap between seeds was bigger than the gap between brain sizes.

So brain size is the player's bet: bigger is slower to train and slower to learn, and the game doesn't yet have fights hard enough to show a clear payoff.

A second network of the same size, the "critic", guesses how well things are going. It is only used during training, to judge whether a move turned out better or worse than expected.

The game uses no machine-learning library. The network, its learning and the battle simulation are all plain Rust in the game itself, and run on the CPU.

## 4. How training runs on your machine

Training uses **PPO** (Proximal Policy Optimization), a standard and stable way to train game-playing agents. One training round goes like this:

1. **Play.** 16 practice drills run side by side, each 512 ticks long, which makes 8,192 decisions per round. The brain picks actions with some randomness, so it keeps trying new things.
2. **Score.** Every decision gets a reward (section 5). The critic estimates how good each moment was, so the game can tell which choices led to good results later, not just straight away.
3. **Learn.** The brain is nudged, a little at a time, towards the choices that worked better than expected and away from the ones that worked worse. The nudges are kept small on purpose (that is the "proximal" part), so one lucky or unlucky drill cannot wreck what it already knows.
4. Repeat until the training budget runs out.

On a normal desktop this is about 5,000 to 8,000 decisions a second, so +200k steps takes under a minute. Training uses every CPU core but one, leaving one free so the window stays smooth. While it runs, the live drill on the left always plays the **newest** brain, so you can watch it improve.

A troop's level caps its total training (300k, 600k, 1.2M, 2M, 3.5M steps). More training gives a better troop, but each extra step helps a little less than the one before.

## 5. Where the rewards come from

The reward is how the troop is told what "good" means. It is built in three layers.

**a) Each class has its own reward recipe.** For example:

| Class | Mostly rewarded for |
|---|---|
| Fighter, Archer, Mage | damage to buildings (counted double) and to guards |
| Assassin | damage to guards and buildings, both weighted high |
| Healer | healing allies, and losing points when an ally dies |
| Paladin | some building and guard damage, plus healing |
| Defenders | hurting raiders, and losing points for every bit of building health lost |

Every attacker also gets a bonus for winning, a penalty for dying, a bigger penalty for running out of time (so hiding until the clock runs out never pays), and a small cost per tick so it doesn't dawdle. A gentle "move towards the action" bonus helps new recruits find the enemy sooner, without changing what the best play is.

**b) Your class mix blends the recipes.** Mage 50 + Healer 30 + Fighter 20 gets 50% of the Mage's reward weights, 30% of the Healer's and 20% of the Fighter's. The same goes for stats and practice drills.

**c) Your training plan reshapes them.**
- **Focus percentages** scale the four goals: break buildings, fight troops, heal allies and stay alive. An even split leaves the class recipe as it is. Setting buildings to 60% makes building damage count about 2.4 times as much, so the troop learns to rush buildings and ignore guards. Setting healing to 45% turns the same mix into a support troop that survives 94% of solo drills.
- **Solo vs squad.** Solo drills are one troop against a tower and its guards. Squad drills put it with two scripted fighters against a bigger base, which is where healers and tanks learn their job.
- **Vs your own troops.** The share of drills against your own trained troops instead of scripted enemies (next section).

Changing the plan later does not reset the brain. It keeps everything it learned, and the following sessions push it towards the new goals.

## 6. Defence and self-play

Defenders practise in their own drills: three copies of the troop, all driven by the same brain, guard a Town Hall and its towers against a raid party arriving from a random side. They learn to hold their posts under the towers instead of charging out.

**Self-play** connects the two sides. Once you own trained troops of the other side, part of each troop's drills (your "vs your own troops" percentage) swap the scripted enemies for them:
- your attackers practise against your own defenders, in place of the scripted guards;
- your defenders practise against raids made of your own attackers.

So improving one side gives the other side a harder sparring partner. The Home Base "your own army" test is the same idea, played out in full.

## 7. How a trained brain fights in battle

In real battles, tournaments and home defence, the troop plays **without** the training randomness: every tick it takes the action its brain scores highest. The simulation uses only exact, repeatable maths, so the same troops placed in the same spots always fight the same battle, on any computer. That is what makes replays and the "replay check" on the result screen work, and it is what will make it possible to trust a shared troop file in future wars between players.

The whole troop, meaning its class mix, plan, weapon, mastery, both networks and training history, is one file of a few hundred KB (more for big level 5 brains). Troop files from before weapons and class moves still load: their brains are grown to take the new inputs and moves, and their past training counts as mastery of their class weapon and as practice of their class moves. That file is what gets exported, and later shared for wars.

## 8. Weapons and mastery

Every troop holds a weapon. Its class weapon is free; others are unlocked once with gold in the Armoury and can be given to any troop.

| Weapon | Feel | Skill |
|---|---|---|
| Sword and shield | Steady hits, best block (takes only 15%) | Shield Bash: 1.5x hit that stuns |
| Greataxe | Bigger, slower swings | Whirlwind: hits everything close by |
| Greatsword | The heaviest hits | Ground Slam: hits and stuns everyone close by |
| Twin daggers | Fast light stabs, quicker feet | Shadowstep: blink to a target up to 6 away and strike for 3x |
| Crossbow | Shoots from range | Volley: hits the 3 nearest enemies |
| Fire staff | Spells from range | Fireball: 2x on the target plus a blast around it |
| Holy wand | Weak hits, stronger heals (or a small heal for anyone) | Healing Nova: heals the most hurt allies nearby, as many as its level allows |

A class with its own weapon keeps its exact stats; another weapon changes them by the difference between the two weapons. A fighter with a crossbow really shoots, and an archer with daggers really stabs.

**Mastery** grows with training steps spent holding that weapon (50k, 150k, 400k, 900k and 1.8M steps for levels 1 to 5, Trained to Grandmaster). Each level adds 5% damage and makes the skill recharge 6% faster, and from Expert (level 3) the skill is 30% stronger. Each weapon keeps its own mastery, so switching weapons means learning again. The brain also sees which weapon it holds, so it can learn different habits for each.

In a 300k-step check of all 9 classes, every one learned to use its skill (1 to 2.5 times per drill), mages and assassins dodged 2 to 4 times per drill, and guards blocked about 37 damage per drill.

## 9. Class moves: jack of all trades, master of none

Every class has its own small kit of moves, and a mix gets the moves of every class it has at least 20% of:

| Class | Moves |
|---|---|
| Fighter | Block, Charge |
| Archer | Dodge, Snare |
| Healer | Dodge, Ward |
| Mage | Blink, Ward |
| Assassin | Dodge, Cloak |
| Paladin | Block, Ward |
| Guard (defender) | Block, Ward |
| Sentry (defender) | Dodge, Snare |
| Keeper (defender) | Ward |
| Warlock (defender) | Blink, Snare |
| Hunter (defender) | Charge, Snare |

A mix gets these moves at full strength, not weaker copies. What it costs is **practice**. Every move starts at 40% strength (longer cooldowns, shorter and weaker effects) and reaches 100% after 300k steps of practice. Each training step is shared evenly between all of a troop's moves, so:

- a pure Fighter (2 moves) masters Block and Charge in 600k steps;
- a Fighter 40 / Archer 30 / Assassin 30 mix (5 moves) needs 1.5M steps to master the same Block, and after 300k steps its Block is still at 52% strength where the Fighter's is at 70%.

The brain also has more choices to learn when to use. A mix can do more things, but a specialist does its few things better for the same training time. The recruit screen shows the kit and how long it takes to master, and the barracks shows how practised each move is.

## 10. Team training

(Measured under sim version 4, before section 11's rules. Section 11 re-measures team training under the new rules.)

Normal squad drills put a troop next to scripted fighters, so it learns to stick with allies but never learns a real teammate. **Team training** puts 2 to 5 of your own troops of one side (attackers or defenders) into the same drills:

- **Each troop keeps its own brain.** A Paladin learns to be the Paladin of *this* team, and the Healer learns to keep *that* Paladin alive.
- **Teammates are visible.** Each troop sees its 2 nearest teammates' roles, whether they are under attack and whether they are fighting or healing.
- **Shared reward.** Each troop's reward is 50% its own (from its own class and plan) and 50% the team's average, so saving a teammate or finishing the building a teammate is hitting pays off.
- **Fair training time.** Every troop's own decisions count as its training: towards its level cap, weapon mastery and move practice. A team of 3 takes about 3 times as long as one troop.

Team raids face a bigger base than solo raids (3 towers, a core and 5 guards, plus 2 guards for each troop above 3). Team defences fight a raid that grows with the team, and the built-in guards only appear when the team has no frontline troop of its own. Half the drills can use your own trained troops of the other side, as in normal training.

### Does it work? (measured)

Two teams, each trained 2 ways with the same total budget per troop (600k steps): **apart** (600k solo) and **together** (300k solo then 300k team training). 2 seeds each, 300 team drills per check.

| Team | Seed | Team drill wins, apart → together | What changed |
|---|---|---|---|
| Paladin + Archer + Healer (attack) | 1 | 45% → 65% | Healer heals 220 → 324 per drill, Healer alive at the end 51% → 74% |
| | 2 | 91% → 90% | Healer heals 205 → 339, Healer alive 67% → 89%, Paladin alive 80% → 94% |
| Guard + Sentry + Keeper (defence) | 1 | 89% → 99.7% | Base damage 14% → 1%, Guard alive 63% → 97%, Keeper alive 62% → 94% |
| | 2 | 99% → 99.7% | Guard alive 83% → 98%, Keeper alive 85% → 99.7% |

The clearest change is behaviour, and it shows on every run: troops trained together keep each other alive. The Healer heals far more and the frontline survives far longer. Win rates went up a lot on one seed and stayed level on the other, which was already near the top.

Those drills are the kind the "together" troops trained in, so the attack teams were also sent against the 4 real enemy bases (one battle each):

| Base | Seed 1 apart | Seed 1 together | Seed 2 apart | Seed 2 together |
|---|---|---|---|---|
| Outpost | win, 282 ticks | win, 217 ticks | win, 235 ticks | win, 221 ticks |
| Fort | win, 401 ticks | win, 306 ticks | win, 2 troops left | win, all 3 left |
| Twin Gate | lost, 38% | lost, 26% | lost, 65% | **win, 100%** |
| Citadel | lost, 7% | 9%, 1 troop still alive at time out | lost, 5% | lost, 15% |

Together-trained teams won the easy bases faster with nobody lost, and seed 2 took Twin Gate, which no apart team managed. Seed 1 did worse at Twin Gate. Nobody can beat Citadel yet. Short version: team training makes troops play as a team, and it usually helps, but it is not a guaranteed win, and run-to-run luck still matters.

### Drill hardness

Team drills can be made harder on the Team training screen. Hardness 1 is the standard drill. Each step up adds 10% health and damage to the defence, plus one more guard. From hardness 4 the base also gets a fourth tower. In defence drills, each step adds one more raider, with the same 10% boost. **Auto** starts at 1. After every 60 drills it goes up one step if the team won at least 80% of them, and down one step if it won under 35%. That keeps the drills just hard enough for the team to keep learning.

A team trained for 1.3M steps at hardness 1 won 97% of hardness-1 drills, 51% at 2, 3% at 3 and 0% at 4: easy drills had taught it nothing about hard fights.

## 11. Whom to heal, and talents that grow with level

**Heals pick a target.** Earlier, the game chose the heal target for the troop: the most hurt ally in reach. Now a healer has two heal actions, "heal my nearest ally" and "heal my second nearest ally". It sees both allies' health, role and whether they are under attack, so it has to learn whom to save first. A heal is always one ally at a time.

**Talents that reach several units are limited by level.** Healing Nova, Ward, Mage and Warlock splash, Whirlwind, Ground Slam, Volley and the extra Fireball blast reach at most this many units:

| Troop level | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| Units reached | 1 | 2 | 2 | 3 | 4 |

When there are more units in range than that, healing talents go to the most hurt first, Ward goes to allies under attack first, and damage talents hit the closest first. Built-in enemy bases keep the full reach of 4. The barracks shows each troop's reach, and the level-up message says when it grows.

### Measured

Paladin + Archer + Healer, 4 seeds at level 1 and 4 at level 5, 300 team drills per check. "Apart" is 600k solo steps per troop; "together" is 300k solo plus 300k team steps, the same total.

| Level | Seed | Team wins, apart → together | Healer alive at end | Healer HP healed per drill | Heals that went to the Paladin |
|---|---|---|---|---|---|
| 1 | 1 | 56% → 96% | 46% → 97% | 221 → 424 | 76% → 72% |
| 1 | 2 | 82% → 96% | 89% → 95% | 322 → 339 | 73% → 77% |
| 1 | 3 | 92% → 91% | 62% → 95% | 247 → 387 | 84% → 86% |
| 1 | 4 | 34% → 88% | 48% → 91% | 259 → 347 | 73% → 78% |
| 5 | 1 | 54% → 99% | 20% → 98% | 109 → 357 | 80% → 84% |
| 5 | 2 | 67% → 92% | 74% → 93% | 229 → 445 | 82% → 75% |
| 5 | 3 | 74% → 88% | 47% → 91% | 202 → 354 | 74% → 73% |
| 5 | 4 | 79% → 90% | 90% → 89% | 179 → 331 | 48% → 81% |

What the troops learned to prioritise: healers send about three quarters of their heals to the Paladin, the tank in front. They learn this even in solo training, and team training keeps it. The "most hurt" ally is not always the right pick: the Archer often has less health left but is out of danger behind the Paladin.

Under the new rules, team training helped on 7 of 8 runs and drew on the 8th. Teams trained together won 88 to 99% of team drills, against 34 to 92% for the same troops trained apart. The Healer survived far more often and healed more on every run. Level 1 and level 5 teams ended up about the same, because this team's basic heals are single target anyway. Reach matters most for Mages, Warlocks and Holy Wand users. Their strength at each level has not been re-measured yet.

## 12. Senses: troops that act like people

Sanjay asked for troops with senses that imitate humans (2026-10-21). Before this, a trained troop always knew exactly where its nearest enemies and buildings were, even behind it or behind a wall. Now (sim version 6) trained troops know only what they notice:

| Sense or limit | Rule |
|---|---|
| Sight | A cone 120 degrees wide and 14 steps long in the direction the troop faces. |
| Blocked view | A standing building between the troop and something hides it. |
| Touch | Anything within 2.5 steps is noticed, even from behind. |
| Hearing | The direction and loudness of the nearest fight within 12 steps, even out of sight. |
| Memory | An enemy that goes out of sight is remembered where it was last seen for 3 seconds, fading. Buildings, once seen, are remembered for the whole battle. |
| Reaction time | Something newly seen can't be attacked for 4 ticks (0.2 s). |
| Turning | Troops turn at about a half turn per 6 ticks. They turn towards where they walk, and a troop has to face its target within 45 degrees to strike; otherwise that tick is spent turning. Standing still, a troop turns towards the nearest fight it hears. |
| Aim | Ranged shots can miss: 4% up close, up to 24% at full range, plus 12% if the target is moving. Buildings are missed 2% of the time. A miss still uses up the attack. |

Towers, guards and scripted troops are unchanged: they still know where everyone is. Randomness for misses comes from a fixed dice roll on the tick and the two units, so a battle still replays exactly and the server can check it.

The battle screen draws each trained troop's sight cone as a faint wedge on the ground.

### Measured

Same four classes, same seed, 64-neuron brains, 200 drills per check.

| Troop | Training | Old rules: solo / squad wins | New rules: solo / squad wins |
|---|---|---|---|
| Fighter | 150k steps | 92% / 47% | 0% / 17% |
| Archer | 150k steps | 46% / 25% | 7% / 27% |
| Healer | 150k steps | 0% / 54% | 0% / 53% |
| Mage | 150k steps | 6% / 22% | 1% / 22% |
| Fighter | 600k steps | 89% / 74% | 79% / 82% |
| Archer | 600k steps | 90% / 32% | 57% / 22% |

Troops need more training to learn to look around, keep enemies in view and turn before striking. With 4 times the training the Fighter almost caught up and did better in squad drills. The Archer is still behind because of missed shots. The scripted fighter, which knows where everything is, barely changed (55% → 55% solo), so the drop comes from what a trained troop has to learn, not from the fights being harder.

Full squad (600k Fighter and Archer, 150k Healer and Mage) against the built-in bases: 3 stars on Outpost, Fort and Twin Gate under both rules, but each raid took longer (Outpost 139 → 202 ticks, Twin Gate 324 → 404). Neither rule set beat the Citadel. Training runs about as fast as before (1,680 steps a second here, against 1,580).

Old saves still load: the brain grows to the 100 new numbers, but it learned under the old rules, so retrain before relying on it.

## 13. Calls: troops that talk to each other

Since sim version 7 a trained troop can shout one of three calls to its teammates:

| Call | Bubble | Points at |
|---|---|---|
| Focus | red ! | the nearest enemy the caller can see (a ring marks it) |
| Fall back | blue < | the caller |
| Help | green + | the caller |

The calls have no meaning built in. A troop only learns what "focus" means because, during team training, teammates that react to it well win more. Two teams can end up using the same call for different things.

- **Who hears it:** teammates with senses within 12 steps of the caller. Walls don't block calls. A troop doesn't hear its own calls.
- **When:** a call is taken in after the same 0.2 s reaction delay as sight, and fades after 1.5 s.
- **What the listener gets:** 9 more inputs (109 in total): for each call, which way it points and how fresh it is.
- **Cost:** none. A shout doesn't use up the turn; the troop keeps doing what it did the tick before. A troop can call at most every 0.5 s.
- **Solo troops:** a troop alone has nobody to call, so calls are switched off for it. Solo training gives exactly the same troop as before; we checked byte for byte.

Enemies can hear calls like any other noise, so shouting can give a troop away.

### Measured

Fighter, Archer and Healer, each trained solo for 150k steps, then trained as a team for 300k steps. 200 team drills, plus one raid on each built-in base.

| | Team drills won | Calls per drill (Fighter / Archer / Healer) | Twin Gate |
|---|---|---|---|
| No calls, seed 5 | 100% | none | 2 stars |
| No calls, seed 6 | 100% | none | 2 stars |
| Calls, seed 5 | 90% | 18 / 22 / 9 | 0 stars |
| Calls, seed 6 | 100% | 18 / 22 / 15 | 0 stars |

All teams took 3 stars on Outpost and Fort, and none could beat the Citadel. Team training took the same time with and without calls (about 745 s each).

At this amount of training, troops learn to call, but they haven't yet learned to gain anything from it. The extra inputs are one more thing to learn, the same way senses needed more training at first (section 12).

After another 1M steps of team training (seed 5 teams), the two were level. Team drills: 96.5% won with calls, 97.5% without. Both teams lost Twin Gate with about the same damage done (30% and 32% destroyed). Calling teams still called 16 to 20 times a drill. Their raids on Outpost were quicker (191 ticks against 218). Calls cost nothing to have, but on these drills they haven't shown a clear win yet. Their best use is likely harder fights and bigger teams, where no single troop can see everything.

### On hard drills, calls pay off

Next, both of those teams trained another 500k steps on **Auto** hardness (section 10). The team with calls climbed to hardness 4. The team without calls stayed mostly at 3.

| | Hardness 3 drills | Hardness 4 drills | Twin Gate | Citadel |
|---|---|---|---|---|
| No calls | 91% won | 62.5% won | 2 stars (72%) | 9% destroyed |
| Calls | 99.5% won | 85.5% won | 3 stars, all troops alive | 41% destroyed |

Easy drills never needed teamwork, so calls didn't help there. Once drills got hard, the calling team clearly pulled ahead. Training took the same time for both (about 1,500 s).

The first version made a call use up the turn. Teams called 45 times a drill and did worse. That is why calls are now free.

## In one line

Your choices (class mix and training plan) decide **what the troop is rewarded for**, the training decides **how it actually behaves**, and the battle simply replays that behaviour exactly.
