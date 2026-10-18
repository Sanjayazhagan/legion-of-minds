//! Become a God: core game logic with no graphics.
//!
//! - `sim`: the deterministic battle simulation
//! - `classes`: troop classes and hybrids
//! - `scenario`: training drills and enemy bases
//! - `env`, `nn`, `ppo`: on-device reinforcement learning
//! - `troop`, `battle`: owned troops, battles and tournaments

pub mod battle;
pub mod classes;
pub mod env;
pub mod nn;
pub mod ppo;
pub mod rng;
pub mod scenario;
pub mod sim;
pub mod troop;
pub mod weapons;

pub use classes::{BaseClass, ClassSpec};
pub use troop::Troop;
