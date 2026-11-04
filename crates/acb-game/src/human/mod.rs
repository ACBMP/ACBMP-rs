//! Retail `Human*` movement state graph: the decisions (which state comes
//! next and the thresholds that pick it), ported from the retail FSM tests
//! (spec: findings/human-states.md). The motion itself stays in
//! `jump.rs`, `climb.rs`, `locomotion.rs` and the `Ground` queries; the
//! player drive (`player.rs`) asks this module what to do.
//!
//! Each constant names its retail source (Mac `acbmp_sf.exe` address or
//! symbol). Everything here is pure and deterministic: no clocks, no hash
//! iteration, only the fixed tick's `dt`.
//!
//! - [`in_air`]: `HumanInAir` (check order, grasp gate, landing classes and
//!   the landing-test delay).
//! - [`ground`]: `HumanGround` edges (fall off vs step down, edge stop,
//!   pull-down) and the obstacle collision (bumping into a wall).
//! - [`haystack`]: the haystack contact entry (facing / push cones).
//! - [`ledge`]: wall jumps from a hang or climb (leap first, rebound
//!   direction, the farthest foothold).
//! - [`jump`]: the Legs window and stick cone that let a freerun jump out
//!   at an edge (there is no manual jump).

pub mod ground;
pub mod haystack;
pub mod in_air;
pub mod jump;
pub mod ledge;

pub use ground::{Edge, EdgeStop, Obstacle};
pub use in_air::{Air, AirCheck, Contact};
pub use jump::Legs;
