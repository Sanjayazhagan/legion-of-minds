# How the RL works

Every troop has its own small brain that learns by trial and error on your computer. Nothing is downloaded or pre-trained: a new recruit starts out acting at random and gets better only from its own practice.

## 1. What a troop sees

Battles run in ticks (30 a second when you watch). Every tick, each trained troop gets a list of 74 numbers describing its surroundings:

- **Itself:** where it is on the map, its health, whether its weapon is ready, how much time is left, and whether it can hit a unit, hit a building or heal right now.
- **The 2 nearest buildings:** where they are, how damaged they are, and whether each one is a tower. Attackers see the buildings they are raiding; defenders see the ones they protect.
- **The 3 nearest enemies:** where they are, how hurt they are, and whether they are about to strike.
- **The 2 nearest allies, plus the most injured ally nearby.** Healers use this.
- **The overall picture:** the share of enemy buildings and enemy troops still standing.
- **Its moves and weapon:** which class moves it has and whether each is ready, whether its weapon skill is ready, whether its guard is up, cloaked or shielded by a ward, how many enemies are close enough for a spin or slam, whether the nearest enemies are stunned or blocking, and which weapon it holds.

Positions are relative ("a guard 3 steps to my left"), so the same brain works on any base layout.

## 2. What a troop can do

It has 20 actions, borrowed from action RPGs and MOBAs. The last seven are class moves, and a troop can only use the ones its classes bring (section 9):

| Action | What it does |
|---|---|
| Stand still, or move in one of 8 directions | |
| Hit unit / hit building | Attack the nearest enemy unit or building in range |
| Heal | Heal the most injured ally in range (healing classes) |
| **Weapon skill** | The weapon's special move, on its own cooldown (section 8) |
| **Dodge** | Roll away from the nearest threat, untouchable for a moment |
| **Block** | Raise the guard for 0.4 s: take much less damage, but no attacks and half speed |
| **Charge** | Rush up to 3.5 steps at an enemy just out of reach, hit it and stun it briefly |
| **Cloak** | Vanish: towers, guards and troops can't target you until you strike |
| **Blink** | Teleport away from the nearest threat |
| **Snare** | Drop a hidden trap that hurts and stuns the first enemy to step on it |
| **Ward** | Shield every ally close by: they take less damage for 1.5 s |

Actions that make no sense right now are blocked, such as attacking out of range, dodging with no enemy near, or using a skill that would hit nothing. The brain only ever chooses between moves that are possible.

Class stats and the weapon decide what those actions achieve: speed, health, range, damage, cooldown, heal power and splash. The brain decides **when, where and at what**, for example to block when a tower is about to fire, or to save Whirlwind until three guards are close.

## 3. The brain

The brain is a small neural network: the 74 numbers go in, pass through two layers of "neurons", and come out as a score for each of the 20 actions. Those weights are what training changes. **The player picks the brain size** when recruiting (64, 96, 128, 160, 192 or 256 neurons per layer) and can grow it later in the barracks:

| Neurons per layer | Numbers in the brain | Training speed (4-core test machine) |
|---|---|---|
| 64 | 10,260 | about 7,500 steps/s |
| 96 | 18,452 | about 4,600 steps/s |
| 128 | 28,692 | about 2,900 steps/s |
| 160 | 40,980 | about 2,300 steps/s |
| 192 | 55,316 | not measured |
| 256 | 90,132 | about 1,050 steps/s |

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
| Holy wand | Weak hits, stronger heals (or a small heal for anyone) | Healing Nova: heals every ally nearby |

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

## In one line

Your choices (class mix and training plan) decide **what the troop is rewarded for**, the training decides **how it actually behaves**, and the battle simply replays that behaviour exactly.
