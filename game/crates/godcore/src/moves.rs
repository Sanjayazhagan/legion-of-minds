//! Class moves: every class has its own small kit of special moves. A mix
//! inherits the moves of every part it has enough of, but each move has to
//! be practised: training time is shared out across all of a troop's moves.
//! A specialist with 2 moves masters them quickly; a mix with 8 moves needs
//! four times the training to get each one as good (jack of all trades).

use crate::classes::{BaseClass, ClassSpec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Move {
    /// Roll away from the nearest threat, untouchable for a moment.
    Dodge,
    /// Raise the guard: much less damage taken, no attacks, half speed.
    Block,
    /// Rush at an enemy, hit it and knock it off balance (short stun).
    Charge,
    /// Vanish: towers, guards and enemy troops can't target you until you
    /// strike (area attacks still hit).
    Cloak,
    /// Teleport far away from the nearest threat.
    Blink,
    /// Drop a hidden trap that stuns and hurts the first enemy to step on it.
    Snare,
    /// Shield every ally nearby: they take less damage for a while.
    Ward,
}

pub const N_MOVES: usize = 7;

impl Move {
    pub const ALL: [Move; N_MOVES] = [Move::Dodge, Move::Block, Move::Charge, Move::Cloak, Move::Blink, Move::Snare, Move::Ward];

    pub fn index(self) -> usize {
        Move::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Move::Dodge => "Dodge",
            Move::Block => "Block",
            Move::Charge => "Charge",
            Move::Cloak => "Cloak",
            Move::Blink => "Blink",
            Move::Snare => "Snare",
            Move::Ward => "Ward",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Move::Dodge => "roll away, untouchable for a moment",
            Move::Block => "guard up, take far less damage",
            Move::Charge => "rush in, hit and stagger",
            Move::Cloak => "vanish until the next strike",
            Move::Blink => "teleport out of danger",
            Move::Snare => "hidden trap that stuns",
            Move::Ward => "shield nearby allies",
        }
    }
}

impl BaseClass {
    /// This class's own moves.
    pub fn moves(self) -> &'static [Move] {
        use BaseClass::*;
        match self {
            Fighter => &[Move::Block, Move::Charge],
            Archer => &[Move::Dodge, Move::Snare],
            Healer => &[Move::Dodge, Move::Ward],
            Mage => &[Move::Blink, Move::Ward],
            Assassin => &[Move::Dodge, Move::Cloak],
            Paladin => &[Move::Block, Move::Ward],
            Guard => &[Move::Block, Move::Ward],
            Sentry => &[Move::Dodge, Move::Snare],
            Keeper => &[Move::Ward],
            Warlock => &[Move::Blink, Move::Snare],
            Hunter => &[Move::Charge, Move::Snare],
        }
    }
}

/// A mix needs at least this share of the classes with a move to have it.
pub const MIN_MOVE_SHARE: f32 = 0.2;
/// Training steps of practice that fully master one move. Each training
/// step counts towards every move in the kit, split evenly between them.
pub const MOVE_MASTERY_STEPS: u64 = 300_000;
/// A move starts at this strength and reaches full strength with practice.
pub const MOVE_START_POWER: f32 = 0.4;

/// Practice (0 to 1) from training steps spent on a move.
pub fn practice_of(steps: u64) -> f32 {
    (steps as f32 / MOVE_MASTERY_STEPS as f32).min(1.0)
}

impl ClassSpec {
    /// Whether this troop's classes bring the move (20% share or more).
    pub fn has_move(&self, m: Move) -> bool {
        let share: f32 = self.parents.iter().filter(|(c, _)| c.moves().contains(&m)).map(|p| p.1).sum();
        share >= MIN_MOVE_SHARE - 1e-4
    }

    /// The moves this troop has.
    pub fn moves(&self) -> Vec<Move> {
        Move::ALL.into_iter().filter(|&m| self.has_move(m)).collect()
    }

    /// How strong the move is: 0 if not in the kit, else 0.4 untrained up to
    /// 1.0 fully practised (shorter cooldowns, longer and stronger effects).
    pub fn move_power(&self, m: Move) -> f32 {
        if !self.has_move(m) {
            return 0.0;
        }
        MOVE_START_POWER + (1.0 - MOVE_START_POWER) * self.move_practice[m as usize].clamp(0.0, 1.0)
    }

    /// The moves this troop has, with their power.
    pub fn move_kit(&self) -> Vec<(Move, f32)> {
        self.moves().into_iter().map(|m| (m, self.move_power(m))).collect()
    }

    /// "Block 40%, Charge 40%" (practice) for the UI.
    pub fn kit_label(&self) -> String {
        self.moves().iter().map(|m| format!("{} {:.0}%", m.name(), self.move_practice[*m as usize] * 100.0)).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_get_more_moves_to_practise() {
        let f = BaseClass::Fighter.spec();
        assert_eq!(f.moves(), vec![Move::Block, Move::Charge]);
        assert_eq!(f.move_power(Move::Block), MOVE_START_POWER);
        assert_eq!(f.move_power(Move::Dodge), 0.0);
        let mix = ClassSpec::blend(&[(BaseClass::Fighter, 60.0), (BaseClass::Assassin, 40.0)]);
        assert_eq!(mix.moves().len(), 4);
        let three = ClassSpec::blend(&[(BaseClass::Fighter, 40.0), (BaseClass::Archer, 30.0), (BaseClass::Assassin, 30.0)]);
        assert_eq!(three.moves().len(), 5);
        // A sliver of a class doesn't bring its moves.
        let sliver = ClassSpec::blend(&[(BaseClass::Fighter, 90.0), (BaseClass::Mage, 10.0)]);
        assert!(!sliver.has_move(Move::Blink));
    }
}
