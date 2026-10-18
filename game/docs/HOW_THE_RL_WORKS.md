# How the RL works

Every troop has its own small brain that learns by trial and error on your computer. Nothing is downloaded or pre-trained: a new recruit starts out acting at random and gets better only from its own practice.

## 1. What a troop sees

Battles run in ticks (30 a second when you watch). Every tick, each trained troop gets a list of 67 numbers describing its surroundings:

- **Itself:** where it is on the map, its health, whether its weapon is ready, how much time is left, and whether it can hit a unit, hit a building or heal right now.
- **The 2 nearest buildings:** where they are, how damaged they are, and whether each one is a tower. Attackers see the buildings they are raiding; defenders see the ones they protect.
- **The 3 nearest enemies:** where they are, how hurt they are, and whether they are about to strike.
- **The 2 nearest allies, plus the most injured ally nearby.** Healers use this.
- **The overall picture:** the share of enemy buildings and enemy troops still standing.
- **Its moves and weapon:** whether dodge, block and its weapon skill are ready, whether its guard is up, how many enemies are close enough for a spin or slam, whether the nearest enemies are stunned or blocking, and which weapon it holds.

Positions are relative ("a guard 3 steps to my left"), so the same brain works on any base layout.

## 2. What a troop can do

It has 15 actions, borrowed from action RPGs and MOBAs:

| Action | What it does |
|---|---|
| Stand still, or move in one of 8 directions | |
| Hit unit / hit building | Attack the nearest enemy unit or building in range |
| Heal | Heal the most injured ally in range (healing classes) |
| **Dodge** | Roll away from the nearest threat, untouchable for a moment (1.5 s cooldown) |
| **Block** | Raise the guard for 0.4 s: take much less damage, but no attacks and half speed |
| **Weapon skill** | The weapon's special move, on its own cooldown (section 8) |

Actions that make no sense right now are blocked, such as attacking out of range, dodging with no enemy near, or using a skill that would hit nothing. The brain only ever chooses between moves that are possible.

Class stats and the weapon decide what those actions achieve: speed, health, range, damage, cooldown, heal power and splash. The brain decides **when, where and at what**, for example to block when a tower is about to fire, or to save Whirlwind until three guards are close.

## 3. The brain

The brain is a small neural network: the 67 numbers go in, pass through two layers of "neurons", and come out as a score for each of the 15 actions. Those weights are what training changes. The brain grows as the troop levels up:

| Troop level | Neurons per layer | Numbers in the brain | Training speed (4-core test machine) |
|---|---|---|---|
| 1-2 | 64 | 9,487 | about 7,500 steps/s |
| 3 | 96 | 17,295 | about 4,600 steps/s |
| 4 | 128 | 27,151 | about 2,900 steps/s |
| 5 | 160 | 39,055 | about 2,200 steps/s |

Growing keeps everything the troop learned: the new neurons start with no say in its choices and are trained in from there. A bigger brain can learn more subtle play, but trains slower, which is why it comes with level and not for free. A second network of the same size, the "critic", guesses how well things are going. It is only used during training, to judge whether a move turned out better or worse than expected.

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

The whole troop, meaning its class mix, plan, weapon, mastery, both networks and training history, is one file of a few hundred KB (more for big level 5 brains). Troop files from before weapons still load: their brains are grown to take the new inputs and moves, and their past training counts as mastery of their class weapon. That file is what gets exported, and later shared for wars.

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

## In one line

Your choices (class mix and training plan) decide **what the troop is rewarded for**, the training decides **how it actually behaves**, and the battle simply replays that behaviour exactly.
