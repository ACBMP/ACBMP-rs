//! Server-side validation of action requests (kills, stuns) before a
//! tick: the hook an authoritative server calls with each player's input.
//! It asks the same rules the sim does ([`crate::rules`]), so a request it
//! grants starts in [`World::step`]; a refused one the server drops from
//! the input it applies (and relays), so clients replaying that input
//! roll their predicted kill back.

use crate::player::body_actor;
use crate::rules::{self, Act, Relation, Reject, Rules};
use crate::{PlayerInput, World};

/// What a player's input asks for this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Nothing new (no kill/stun button, or one already under way).
    None,
    /// Granted: kill or stun body `victim`.
    Granted { act: Act, victim: usize },
}

impl World {
    /// Checks player `i`'s kill/stun request in `input` against the
    /// current state with `rules`; `relation(of, body)` says how body
    /// `body` stands to player `of` (contracts; [`Relation::Npc`] when
    /// no match rules apply). Mirrors the victim choice of the player
    /// driver, so a grant here is the kill the next step starts.
    pub fn validate_action(&self, i: usize, input: &PlayerInput, rules: &Rules, relation: &dyn Fn(usize, usize) -> Relation) -> Result<Request, Reject> {
        let Some(p) = self.players.get(i) else { return Ok(Request::None) };
        if !(input.kill || input.stun) || p.kill.is_some() || p.approach.is_some() {
            return Ok(Request::None);
        }
        let Some(kills) = &self.clips.kills else { return Ok(Request::None) };
        let act = if input.stun { Act::Stun } else { Act::Kill };
        let me = p.actor();
        let candidates = || self.bodies.iter().enumerate().filter(|(j, _)| *j != p.body).map(|(j, b)| (j, body_actor(b), relation(i, j)));
        if let Some((victim, _)) = rules::best_victim(act, &me, candidates(), rules, kills.spacing()) {
            return Ok(Request::Granted { act, victim });
        }
        // the nearest one's reason
        let why = candidates()
            .min_by(|a, b| a.1.pos.distance(me.pos).total_cmp(&b.1.pos.distance(me.pos)))
            .map(|(_, v, r)| rules::check(act, &me, &v, r, rules, kills.spacing()));
        Err(match why {
            Some(Err(r)) => r,
            _ => Reject::Range,
        })
    }
}
