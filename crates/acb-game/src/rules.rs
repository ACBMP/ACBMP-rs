//! Server-side MP gameplay rules as pure, deterministic functions: who may
//! kill or stun whom from where, the walk-in before a paired kill, who
//! hunts whom (contracts) and what the compass shows. The server decides
//! with these; client prediction calls the same functions. Nothing here
//! reads a clock or a random source: time comes in as fixed ticks of
//! [`crate::DT`], ties break by player index.
//!
//! Retail tuning comes from the game data where it exists
//! (`DataPC.forge`, `GameParameters` uid 0xbc9: `GlobalParameters` and the
//! `Wanted2 GamemodeParameters` uid 0x3723493d) and from the defaults of
//! the game-mode bricks' XML readers in the Mac binary (the mode XML files
//! themselves are not shipped). Each constant names its source.

use glam::{Vec2, Vec3};

// ---------------------------------------------------------------------------
// retail constants

/// Targeting range: candidates for the highlighted target must be within
/// this horizontal distance. `GlobalParameters.Dist_visibility` = 9, read
/// by `AssassinTask::GetBestNearbyTarget` (Mac 0x00b4f648) as d^2 <= 9^2.
pub const DIST_VISIBILITY: f32 = 9.0;
/// Targeting height window, |dz| (Mac 0x00b4f648, literal 1.6).
pub const TARGET_HEIGHT: f32 = 1.6;
/// Targeting cone: cos of the half angle between aim and target direction
/// (Mac 0x00b4f648, literal 0.5 = 60 degrees).
pub const TARGET_CONE_COS: f32 = 0.5;
/// Gap a walking kill closes before the paired clips start (killer to the
/// kill's start point). `GlobalParameters.Dist_gap` = 3.5. Its use in code
/// was not located; this is our reading of it.
pub const DIST_GAP: f32 = 3.5;
/// Height difference a ground kill bridges. `GlobalParameters.Dist_height_gap` = 1.8.
pub const DIST_HEIGHT_GAP: f32 = 1.8;
/// Reach of a kill (and a stun) out of a run: no walk-in, the paired clips
/// start at once. `Wanted2 GamemodeParameters.Dist_kill_run` = 2.5.
pub const DIST_KILL_RUN: f32 = 2.5;
/// Reach of an air kill from above. `Wanted2 GamemodeParameters.Dist_kill_droptech` = 8.
pub const DIST_KILL_DROPTECH: f32 = 8.0;
/// Kill-clip distance bands (`AC2AnimationGround2Ground::GetAnimationGeneric`,
/// Mac 0x00e7a5d4): tag 47 below 0.7 m, 48 up to 2.0 m, 49 beyond.
pub const BAND_CLOSE: f32 = 0.7;
pub const BAND_MID: f32 = 2.0;
/// Pelvis spacing of the paired ground kills at t = 0 (the `G2G_*_Walk2All`
/// clips, measured: ~1 m). Fallback when no clip is at hand.
pub const PAIR_SPACING: f32 = 1.0;
/// Speed above which the killer counts as running (locomotion's run
/// cycle; the kill tree's AssassinSpeed context splits walk/run).
pub const RUN_SPEED: f32 = 3.0;

/// The walk-in ends when the killer is this close to the kill's start
/// point; the kill clip's 0.25 s blend absorbs the rest. Ours.
pub const APPROACH_ARRIVE: f32 = 0.3;
/// A walk-in gives up after this long (victim got away). Ours.
pub const APPROACH_TIMEOUT: f32 = 2.5;

/// Contract levels (`GBrick_LessFrustratingTarget` XML default
/// `NumberOfLevels` = 3, Mac 0x011c778e).
pub const NUMBER_OF_LEVELS: u8 = 3;
/// While a hunter has waited less than this, a new target must be at least
/// [`MIN_DISTANCE`] away and not the previous one (`MaxWaitTime` = 20 s,
/// used in `IsValidCandidateFor`, Mac 0x012138d0).
pub const MAX_WAIT_TIME: f32 = 20.0;
/// No new contract for this long after a kill or a lost contract
/// (`MinWaitTime` = 5 s; our reading of it).
pub const MIN_WAIT_TIME: f32 = 5.0;
/// Minimum distance of a fresh target (`MinDistance` = 15 m).
pub const MIN_DISTANCE: f32 = 15.0;

/// Compass: a target further than this is not detected (direction only).
/// `GBrick_PlayerDetector` XML default `DetectionRange` = 30 (Mac 0x012d140e).
pub const DETECTION_RANGE: f32 = 30.0;
/// Height difference the compass ignores (`AltitudeDifference` = 2).
pub const ALTITUDE_DIFFERENCE: f32 = 2.0;
/// Near-target state, entered below / left above (hysteresis).
/// `GlobalParameters.PlayerNearTarget` = 10, `PlayerFarFromTarget` = 15.
pub const NEAR_TARGET: f32 = 10.0;
pub const FAR_FROM_TARGET: f32 = 15.0;
/// Near-assassin (threat) state, entered below / left above.
/// `GlobalParameters.PlayerNearAssassin` = 15, `PlayerFarFromAssassin` = 70.
pub const NEAR_ASSASSIN: f32 = 15.0;
pub const FAR_FROM_ASSASSIN: f32 = 70.0;

// ---------------------------------------------------------------------------
// kill and stun validity

/// What a character is doing, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Ground,
    Air,
    Climb,
    /// In a hiding spot (haystack, bench, crowd blend).
    Hidden,
    /// Being killed, stunned or dead: nobody can act on it.
    Down,
}

/// A character's state for the rules.
#[derive(Clone, Copy, Debug)]
pub struct Actor {
    pub pos: Vec3,
    /// Facing, entity +Y = (-sin yaw, cos yaw) (as `Player::yaw`).
    pub yaw: f32,
    pub stance: Stance,
    /// Horizontal speed, m/s.
    pub speed: f32,
}

impl Actor {
    pub fn facing(&self) -> Vec2 {
        Vec2::new(-self.yaw.sin(), self.yaw.cos())
    }
}

/// The victim from the actor's side (`GameInfo::IsAssassinFor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// The actor holds a contract on it.
    Target,
    /// It holds a contract on the actor.
    Pursuer,
    /// Both at once (two-player games).
    Mutual,
    /// A crowd NPC.
    Npc,
    /// Another player with no contract either way.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Kill,
    Stun,
}

/// Which retail kill situation applies (`KillAnimTree.Situations`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Situation {
    /// Walking (or standing) ground kill, after a walk-in; also stuns.
    Ground2Ground,
    /// Running ground kill (`G2G_*_Run2All`), immediate.
    Run2Ground,
    /// Falling or jumping onto the victim (`Air2G`).
    AirDrop2Ground,
    /// Letting go of a hold above the victim (`Drop2G`).
    Drop2Ground,
    /// Killing someone in a hiding spot (`Walk2Hayground`).
    Ground2Haystack,
    /// Out of a hiding spot onto a passer-by (`Hayground2Walk_grab`).
    Haystack2Ground,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The killer is ahead of the victim.
    Front,
    Back,
}

/// Kill-clip distance band (tags 47/48/49).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Close,
    Mid,
    Far,
}

/// Why an action is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Not a valid victim for this act (a crowd NPC the rules protect).
    Relation,
    /// A player, but not the one the contract is on.
    NotTarget,
    /// A stun on a player who is not hunting the stunner.
    NotPursuer,
    /// Killer or victim state does not allow it.
    Stance,
    Range,
    Height,
    /// Victim outside the killer's cone.
    Angle,
}

/// A granted kill or stun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grant {
    pub situation: Situation,
    pub side: Side,
    pub band: Band,
    /// Horizontal killer-victim distance.
    pub dist: f32,
    /// The killer walks in to the kill's start point first (walking ground
    /// kills); otherwise the paired clips start at once.
    pub approach: bool,
}

/// Game-wide switches.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// No match running (viewer free play): crowd NPCs count as targets
    /// for kills and as pursuers for stuns.
    pub free_play: bool,
    /// `GBrick_ScoreKill` XML default `AllowKillingNPCs` = 1.
    pub allow_killing_npcs: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { free_play: true, allow_killing_npcs: true }
    }
}

/// Front when `from` is ahead of a victim at `victim` facing `victim_yaw`
/// (Mac 0x00e7a5d4 splits at a right angle).
pub fn side(from: Vec3, victim: Vec3, victim_yaw: f32) -> Side {
    let to = (from - victim).truncate();
    let facing = Vec2::new(-victim_yaw.sin(), victim_yaw.cos());
    if to.dot(facing) < 0.0 {
        Side::Back
    } else {
        Side::Front
    }
}

pub fn band(dist: f32) -> Band {
    if dist < BAND_CLOSE {
        Band::Close
    } else if dist <= BAND_MID {
        Band::Mid
    } else {
        Band::Far
    }
}

/// Whether `killer` may `act` on `victim` now, and how. `spacing` is the
/// pelvis spacing of the paired clips at their start ([`PAIR_SPACING`]
/// when unknown): a walking kill may start from `spacing + DIST_GAP`.
pub fn check(act: Act, killer: &Actor, victim: &Actor, rel: Relation, rules: &Rules, spacing: f32) -> Result<Grant, Reject> {
    let allowed = match (act, rel) {
        (_, Relation::Mutual) => true,
        (Act::Kill, Relation::Target) => true,
        (Act::Kill, Relation::Npc) => rules.allow_killing_npcs || rules.free_play,
        (Act::Stun, Relation::Pursuer) => true,
        (Act::Stun, Relation::Npc) => rules.free_play,
        _ => false,
    };
    if !allowed {
        return Err(match (act, rel) {
            (_, Relation::Npc) => Reject::Relation,
            (Act::Kill, _) => Reject::NotTarget,
            (Act::Stun, _) => Reject::NotPursuer,
        });
    }
    if victim.stance == Stance::Down || killer.stance == Stance::Down || (killer.stance == Stance::Hidden && act == Act::Stun) {
        return Err(Reject::Stance);
    }
    let d = (victim.pos - killer.pos).truncate();
    let dist = d.length();
    let dz = victim.pos.z - killer.pos.z;
    let dir = d.normalize_or_zero();
    // very close victims are always "ahead"
    let in_cone = dist < BAND_CLOSE || dir.dot(killer.facing()) >= TARGET_CONE_COS;
    let side = side(killer.pos, victim.pos, victim.yaw);
    let running = killer.speed >= RUN_SPEED;
    let (situation, reach, approach) = match (killer.stance, victim.stance) {
        (Stance::Ground, Stance::Ground) if act == Act::Stun => (Situation::Ground2Ground, DIST_KILL_RUN, false),
        (Stance::Ground, Stance::Ground) if running => (Situation::Run2Ground, DIST_KILL_RUN, false),
        (Stance::Ground, Stance::Ground) => (Situation::Ground2Ground, spacing + DIST_GAP, true),
        (Stance::Ground, Stance::Hidden) if act == Act::Kill => (Situation::Ground2Haystack, DIST_KILL_RUN, false),
        (Stance::Hidden, Stance::Ground) => (Situation::Haystack2Ground, DIST_KILL_RUN, false),
        (Stance::Air | Stance::Climb, Stance::Ground) if act == Act::Kill => {
            // from above only, within droptech reach (3D)
            if dz > -0.5 {
                return Err(Reject::Height);
            }
            if (victim.pos - killer.pos).length() > DIST_KILL_DROPTECH {
                return Err(Reject::Range);
            }
            let situation = if killer.stance == Stance::Air { Situation::AirDrop2Ground } else { Situation::Drop2Ground };
            return Ok(Grant { situation, side, band: band(dist), dist, approach: false });
        }
        _ => return Err(Reject::Stance),
    };
    if dist > reach {
        return Err(Reject::Range);
    }
    if dz.abs() > DIST_HEIGHT_GAP {
        return Err(Reject::Height);
    }
    if !in_cone {
        return Err(Reject::Angle);
    }
    Ok(Grant { situation, side, band: band(dist), dist, approach })
}

/// Index and grant of the best victim among `candidates` (index, actor,
/// relation): the nearest valid one, ties by index.
pub fn best_victim(
    act: Act,
    killer: &Actor,
    candidates: impl IntoIterator<Item = (usize, Actor, Relation)>,
    rules: &Rules,
    spacing: f32,
) -> Option<(usize, Grant)> {
    candidates
        .into_iter()
        .filter_map(|(i, a, r)| check(act, killer, &a, r, rules, spacing).ok().map(|g| (i, g)))
        .min_by(|a, b| a.1.dist.total_cmp(&b.1.dist).then(a.0.cmp(&b.0)))
}

/// Where a stunned victim's clip frame goes: `spacing` ahead of the
/// stunner (the `IDXX_G2G_Humil_*` pair is authored around each one's own
/// origin, assassin facing +Y). Returns position and yaw of that frame.
pub fn stun_placement(stunner: &Actor, spacing: f32) -> (Vec3, f32) {
    (stunner.pos + (stunner.facing() * spacing).extend(0.0), stunner.yaw)
}

// ---------------------------------------------------------------------------
// the walk-in

/// One tick of the walk-in towards `goal` (the killer's entity position
/// at the kill's start): the movement wanted, or `None` once there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Approach {
    /// Keep moving: heading as a camera yaw (0 = +X) and whether to run.
    Move { heading: f32, run: bool, left: f32 },
    /// At the start point: play the paired kill.
    Arrived,
    /// Timed out or the victim got out of reach: drop it.
    Abort,
}

/// Decide the walk-in step after `elapsed` seconds of it. `reach` is the
/// distance beyond which the victim is lost (the grant's range check).
pub fn approach(killer: Vec3, goal: Vec3, victim: Vec3, elapsed: f32, reach: f32) -> Approach {
    let to = (goal - killer).truncate();
    let left = to.length();
    if left <= APPROACH_ARRIVE {
        return Approach::Arrived;
    }
    if elapsed >= APPROACH_TIMEOUT || (victim - killer).truncate().length() > reach + 0.5 {
        return Approach::Abort;
    }
    Approach::Move { heading: to.y.atan2(to.x), run: left > BAND_MID, left }
}

// ---------------------------------------------------------------------------
// contracts: who hunts whom

/// Pursuers allowed on a target of `level` (1..=levels) among `players`
/// (`GBrick_LessFrustratingTarget::GetMaxTargeted`, Mac 0x011c099e):
/// leaders draw more hunters, never more than players - 2 (at least one).
pub fn max_pursuers(level: u8, players: usize, levels: u8) -> usize {
    let cap = if players > 2 { players - 2 } else { 1 };
    let l = (level as f32 + 0.6) - 1.0;
    let by_level = ((l + l) / (levels.max(2) - 1) as f32 + 1.0) as usize;
    by_level.min(cap)
}

/// One player's contract state.
#[derive(Clone, Debug, Default)]
pub struct Hunter {
    pub target: Option<usize>,
    /// Target before the current one (not re-drawn while waiting is short).
    pub previous: Option<usize>,
    /// Seconds without a target.
    pub wait: f32,
    /// No new contract until this reaches 0 (after a kill / loss).
    pub cooldown: f32,
    /// 1..=levels; higher (score leaders) draw more pursuers.
    pub level: u8,
    /// In the game and alive (not dead, respawning or gone).
    pub active: bool,
}

/// What happened to contracts during an update or event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractEvent {
    Assigned { hunter: usize, target: usize },
    /// `hunter` lost its contract on `target` (target killed by someone
    /// else, left, or died).
    Lost { hunter: usize, target: usize },
    Fulfilled { hunter: usize, target: usize },
}

/// The killer loop of a Wanted match.
#[derive(Clone, Debug)]
pub struct Contracts {
    pub hunters: Vec<Hunter>,
    pub levels: u8,
}

impl Contracts {
    pub fn new(players: usize) -> Self {
        let h = Hunter { level: 1, active: true, ..Default::default() };
        Contracts { hunters: vec![h; players], levels: NUMBER_OF_LEVELS }
    }

    /// How many hunters hold a contract on `target`.
    pub fn pursuers(&self, target: usize) -> usize {
        self.hunters.iter().filter(|h| h.active && h.target == Some(target)).count()
    }

    /// `of`'s relation to `other` (kill/stun validity input).
    pub fn relation(&self, of: usize, other: usize) -> Relation {
        let mine = self.hunters[of].target == Some(other);
        let theirs = self.hunters[other].target == Some(of);
        match (mine, theirs) {
            (true, true) => Relation::Mutual,
            (true, false) => Relation::Target,
            (false, true) => Relation::Pursuer,
            _ => Relation::Other,
        }
    }

    /// Whether `cand` may become `hunter`'s target (`IsValidCandidateFor`,
    /// Mac 0x012138d0). Hard rules: active, not self, not hunting the
    /// hunter unless two players, fewer than players - 2 pursuers. Soft
    /// rules (`strict`; retail reports them as reasons 2..5 and drops them
    /// when nothing else is left): below the level's pursuer cap and, while
    /// the hunter's wait is short, far enough away and not the previous
    /// target.
    pub fn valid_candidate(&self, hunter: usize, cand: usize, pos: &[Vec3], strict: bool) -> bool {
        let n = self.hunters.iter().filter(|h| h.active).count();
        let (h, c) = (&self.hunters[hunter], &self.hunters[cand]);
        if hunter == cand || !c.active {
            return false;
        }
        if c.target == Some(hunter) && n > 2 {
            return false;
        }
        let pursuers = self.pursuers(cand);
        if pursuers >= if n > 2 { n - 2 } else { 1 } {
            return false;
        }
        if !strict {
            return true;
        }
        if pursuers >= max_pursuers(c.level, n, self.levels) {
            return false;
        }
        if h.wait < MAX_WAIT_TIME {
            if let (Some(a), Some(b)) = (pos.get(hunter), pos.get(cand)) {
                if a.distance(*b) < MIN_DISTANCE {
                    return false;
                }
            }
            if h.previous == Some(cand) {
                return false;
            }
        }
        true
    }

    /// The target `hunter` draws now, if any: the candidate with the most
    /// free pursuer slots (leaders first), then the furthest, then the
    /// lowest index; the soft rules are dropped when no candidate passes
    /// them (retail's relaxed passes in `GetNewContractFor`, Mac 0x0121408c).
    pub fn draw(&self, hunter: usize, pos: &[Vec3]) -> Option<usize> {
        let n = self.hunters.iter().filter(|h| h.active).count();
        let slots = |c: usize| max_pursuers(self.hunters[c].level, n, self.levels) as i32 - self.pursuers(c) as i32;
        let d = |c: usize| match (pos.get(hunter), pos.get(c)) {
            (Some(p), Some(q)) => p.distance(*q),
            _ => 0.0,
        };
        let pick = |strict: bool| {
            (0..self.hunters.len())
                .filter(|&c| self.valid_candidate(hunter, c, pos, strict))
                .min_by(|&a, &b| slots(b).cmp(&slots(a)).then(d(b).total_cmp(&d(a))).then(a.cmp(&b)))
        };
        pick(true).or_else(|| pick(false))
    }

    /// One tick: advance waits and hand out contracts to the hunters
    /// without one, longest wait first, then lower level, then index
    /// (`SortByWaitTimeAndLevel`, Mac 0x01233a7b). `pos[i]` is player i.
    pub fn update(&mut self, dt: f32, pos: &[Vec3]) -> Vec<ContractEvent> {
        let mut out = Vec::new();
        for h in self.hunters.iter_mut().filter(|h| h.active) {
            h.cooldown = (h.cooldown - dt).max(0.0);
            if h.target.is_none() {
                h.wait += dt;
            }
        }
        let mut order: Vec<usize> = (0..self.hunters.len())
            .filter(|&i| {
                let h = &self.hunters[i];
                h.active && h.target.is_none() && h.cooldown <= 0.0
            })
            .collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&self.hunters[a], &self.hunters[b]);
            y.wait.total_cmp(&x.wait).then(x.level.cmp(&y.level)).then(a.cmp(&b))
        });
        for i in order {
            if let Some(t) = self.draw(i, pos) {
                let h = &mut self.hunters[i];
                h.target = Some(t);
                h.wait = 0.0;
                out.push(ContractEvent::Assigned { hunter: i, target: t });
            }
        }
        out
    }

    /// `victim` died (killed by `killer`, or by anything when `None`): the
    /// killer's contract on it is fulfilled, everybody else's lost; the
    /// victim drops its own contract and leaves until [`Contracts::respawn`].
    pub fn on_death(&mut self, victim: usize, killer: Option<usize>) -> Vec<ContractEvent> {
        let mut out = Vec::new();
        for (i, h) in self.hunters.iter_mut().enumerate() {
            if h.target != Some(victim) {
                continue;
            }
            h.previous = Some(victim);
            h.target = None;
            h.wait = 0.0;
            h.cooldown = MIN_WAIT_TIME;
            out.push(if Some(i) == killer {
                ContractEvent::Fulfilled { hunter: i, target: victim }
            } else {
                ContractEvent::Lost { hunter: i, target: victim }
            });
        }
        let v = &mut self.hunters[victim];
        if let Some(t) = v.target.take() {
            v.previous = Some(t);
            out.push(ContractEvent::Lost { hunter: victim, target: t });
        }
        v.active = false;
        v.wait = 0.0;
        out
    }

    /// `p` is back in the game (respawned): it draws a new contract after
    /// the cooldown and is a candidate again.
    pub fn respawn(&mut self, p: usize) {
        let h = &mut self.hunters[p];
        h.active = true;
        h.wait = 0.0;
        h.cooldown = MIN_WAIT_TIME;
    }

    /// `p` was stunned by its target: the contract is lost (Wanted).
    pub fn on_stunned(&mut self, p: usize) -> Vec<ContractEvent> {
        let h = &mut self.hunters[p];
        match h.target.take() {
            Some(t) => {
                h.previous = Some(t);
                h.wait = 0.0;
                h.cooldown = MIN_WAIT_TIME;
                vec![ContractEvent::Lost { hunter: p, target: t }]
            }
            None => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// compass / locate

/// What a hunter's compass shows about its target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Locate {
    /// Horizontal direction to the target (unit, or zero on top of it).
    pub dir: Vec2,
    pub dist: f32,
    /// 0 beyond [`DETECTION_RANGE`], 1 at [`NEAR_TARGET`] and closer.
    pub fill: f32,
    /// In the near-target state (hysteresis 10 / 15 m).
    pub near: bool,
    /// Within targeting range, height and line of sight: the target can be
    /// picked out (portrait lit, highlight possible).
    pub in_view: bool,
}

/// The compass for a hunter at `hunter` locating `target`; `was_near` is
/// the previous tick's `near`, `los` whether the target is in line of sight.
pub fn locate(hunter: Vec3, target: Vec3, was_near: bool, los: bool) -> Locate {
    let d = (target - hunter).truncate();
    let dist = d.length();
    let dz = (target.z - hunter.z).abs();
    let fill = ((DETECTION_RANGE - dist) / (DETECTION_RANGE - NEAR_TARGET)).clamp(0.0, 1.0);
    let near = if was_near { dist <= FAR_FROM_TARGET } else { dist < NEAR_TARGET };
    let in_view = los && dist <= DIST_VISIBILITY && dz <= ALTITUDE_DIFFERENCE;
    Locate { dir: d.normalize_or_zero(), dist, fill, near, in_view }
}

/// Threat state of a target with a pursuer at `pursuer` (hysteresis 15 / 70 m).
pub fn threatened(target: Vec3, pursuer: Vec3, was: bool) -> bool {
    let dist = (pursuer - target).truncate().length();
    if was {
        dist <= FAR_FROM_ASSASSIN
    } else {
        dist < NEAR_ASSASSIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, yaw: f32) -> Actor {
        Actor { pos: Vec3::new(x, y, 0.0), yaw, stance: Stance::Ground, speed: 0.0 }
    }
    const MP: Rules = Rules { free_play: false, allow_killing_npcs: true };

    #[test]
    fn kill_range_and_walk_in() {
        // killer at origin facing +Y (yaw 0), victim ahead
        let k = at(0.0, 0.0, 0.0);
        let g = check(Act::Kill, &k, &at(0.0, 4.0, 0.0), Relation::Target, &MP, PAIR_SPACING).unwrap();
        assert!(g.approach && g.band == Band::Far && g.side == Side::Back);
        assert_eq!(check(Act::Kill, &k, &at(0.0, 4.6, 0.0), Relation::Target, &MP, PAIR_SPACING), Err(Reject::Range));
        // running: no walk-in, shorter reach
        let r = Actor { speed: 4.0, ..k };
        assert!(!check(Act::Kill, &r, &at(0.0, 2.4, 0.0), Relation::Target, &MP, PAIR_SPACING).unwrap().approach);
        assert_eq!(check(Act::Kill, &r, &at(0.0, 3.0, 0.0), Relation::Target, &MP, PAIR_SPACING), Err(Reject::Range));
        // victim facing the killer: front
        let g = check(Act::Kill, &k, &at(0.0, 1.5, std::f32::consts::PI), Relation::Target, &MP, PAIR_SPACING).unwrap();
        assert_eq!((g.side, g.band), (Side::Front, Band::Mid));
    }

    #[test]
    fn kill_behind_height_and_relation() {
        let k = at(0.0, 0.0, 0.0);
        // victim behind the killer
        assert_eq!(check(Act::Kill, &k, &at(0.0, -2.0, 0.0), Relation::Target, &MP, 1.0), Err(Reject::Angle));
        // 70 degrees off: outside the 60 degree cone
        let off = Vec2::new(70f32.to_radians().sin(), 70f32.to_radians().cos()) * 2.0;
        assert_eq!(check(Act::Kill, &k, &at(off.x, off.y, 0.0), Relation::Target, &MP, 1.0), Err(Reject::Angle));
        // height
        let mut v = at(0.0, 2.0, 0.0);
        v.pos.z = 1.7;
        assert!(check(Act::Kill, &k, &v, Relation::Target, &MP, 1.0).is_ok());
        v.pos.z = 1.9;
        assert_eq!(check(Act::Kill, &k, &v, Relation::Target, &MP, 1.0), Err(Reject::Height));
        // only targets die; only pursuers get stunned
        let v = at(0.0, 2.0, 0.0);
        assert_eq!(check(Act::Kill, &k, &v, Relation::Other, &MP, 1.0), Err(Reject::NotTarget));
        assert_eq!(check(Act::Kill, &k, &v, Relation::Pursuer, &MP, 1.0), Err(Reject::NotTarget));
        assert!(check(Act::Stun, &k, &v, Relation::Pursuer, &MP, 1.0).is_ok());
        assert_eq!(check(Act::Stun, &k, &v, Relation::Target, &MP, 1.0), Err(Reject::NotPursuer));
        assert_eq!(check(Act::Stun, &k, &v, Relation::Npc, &MP, 1.0), Err(Reject::Relation));
        assert!(check(Act::Stun, &k, &v, Relation::Npc, &Rules::default(), 1.0).is_ok());
    }

    #[test]
    fn kill_stances() {
        let k = at(0.0, 0.0, 0.0);
        let v = at(0.0, 2.0, 0.0);
        for (ks, vs) in [(Stance::Climb, Stance::Climb), (Stance::Ground, Stance::Air), (Stance::Ground, Stance::Climb), (Stance::Hidden, Stance::Hidden), (Stance::Ground, Stance::Down)] {
            let r = check(Act::Kill, &Actor { stance: ks, ..k }, &Actor { stance: vs, ..v }, Relation::Target, &MP, 1.0);
            assert_eq!(r, Err(Reject::Stance), "{ks:?} -> {vs:?}");
        }
        // hidden victim: haystack kill within run reach
        let g = check(Act::Kill, &k, &Actor { stance: Stance::Hidden, ..v }, Relation::Target, &MP, 1.0).unwrap();
        assert_eq!(g.situation, Situation::Ground2Haystack);
        // air kill from above
        let mut a = Actor { stance: Stance::Air, ..k };
        a.pos.z = 4.0;
        assert_eq!(check(Act::Kill, &a, &v, Relation::Target, &MP, 1.0).unwrap().situation, Situation::AirDrop2Ground);
        a.pos.z = 0.0;
        assert_eq!(check(Act::Kill, &a, &v, Relation::Target, &MP, 1.0), Err(Reject::Height));
        // droptech reach: 8 m (3D) from above
        a.pos = Vec3::new(0.0, 6.0, 5.0);
        assert!(check(Act::Kill, &a, &v, Relation::Target, &MP, 1.0).is_ok());
        a.pos = Vec3::new(0.0, 8.0, 6.0);
        assert_eq!(check(Act::Kill, &a, &v, Relation::Target, &MP, 1.0), Err(Reject::Range));
        // air: no stuns
        a.pos = Vec3::new(0.0, 0.0, 3.0);
        assert_eq!(check(Act::Stun, &a, &v, Relation::Pursuer, &MP, 1.0), Err(Reject::Stance));
        // letting go of a hold above the victim: drop kill
        let c = Actor { stance: Stance::Climb, pos: Vec3::new(0.0, 0.5, 3.0), ..k };
        assert_eq!(check(Act::Kill, &c, &v, Relation::Target, &MP, 1.0).unwrap().situation, Situation::Drop2Ground);
        // out of a hiding spot onto a passer-by, within run reach; no stun from there
        let h = Actor { stance: Stance::Hidden, ..k };
        assert_eq!(check(Act::Kill, &h, &v, Relation::Target, &MP, 1.0).unwrap().situation, Situation::Haystack2Ground);
        assert_eq!(check(Act::Kill, &h, &at(0.0, 2.6, 0.0), Relation::Target, &MP, 1.0), Err(Reject::Range));
        assert_eq!(check(Act::Stun, &h, &v, Relation::Pursuer, &MP, 1.0), Err(Reject::Stance));
        // running kill: its own situation
        let r = Actor { speed: RUN_SPEED, ..k };
        assert_eq!(check(Act::Kill, &r, &v, Relation::Target, &MP, 1.0).unwrap().situation, Situation::Run2Ground);
    }

    #[test]
    fn walk_in_steps() {
        let goal = Vec3::new(0.0, 3.0, 0.0);
        let victim = Vec3::new(0.0, 4.0, 0.0);
        match approach(Vec3::ZERO, goal, victim, 0.0, 4.5) {
            Approach::Move { heading, run, .. } => assert!((heading - std::f32::consts::FRAC_PI_2).abs() < 1e-5 && run),
            a => panic!("{a:?}"),
        }
        assert_eq!(approach(Vec3::new(0.0, 2.8, 0.0), goal, victim, 1.0, 4.5), Approach::Arrived);
        assert_eq!(approach(Vec3::ZERO, goal, victim, APPROACH_TIMEOUT, 4.5), Approach::Abort);
    }

    #[test]
    fn pursuer_caps() {
        // GetMaxTargeted: level 1 -> 1, 2 -> 2, 3 -> 3, capped at players - 2
        assert_eq!((1..=3).map(|l| max_pursuers(l, 8, 3)).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(max_pursuers(3, 4, 3), 2);
        assert_eq!(max_pursuers(3, 2, 3), 1);
    }

    /// Players spread 20 m apart on a line (beyond MIN_DISTANCE).
    fn line(n: usize) -> Vec<Vec3> {
        (0..n).map(|i| Vec3::new(i as f32 * 20.0, 0.0, 0.0)).collect()
    }

    #[test]
    fn assignment_everyone_hunted_once() {
        for n in [2, 3, 4, 6, 8] {
            let pos = line(n);
            let mut c = Contracts::new(n);
            let ev = c.update(crate::DT, &pos);
            assert_eq!(ev.len(), n, "n {n}");
            for i in 0..n {
                let t = c.hunters[i].target.unwrap();
                assert_ne!(t, i);
                // level 1 everywhere: exactly one pursuer each
                assert_eq!(c.pursuers(i), 1, "n {n} player {i}");
                if n > 2 {
                    assert_ne!(c.hunters[t].target, Some(i), "no mutual hunting with {n}");
                }
            }
            // deterministic
            let mut d = Contracts::new(n);
            d.update(crate::DT, &pos);
            assert_eq!(c.hunters.iter().map(|h| h.target).collect::<Vec<_>>(), d.hunters.iter().map(|h| h.target).collect::<Vec<_>>());
        }
    }

    #[test]
    fn assignment_respects_levels_and_distance() {
        let n = 6;
        let pos = line(n);
        let mut c = Contracts::new(n);
        c.hunters[5].level = 3;
        c.update(crate::DT, &pos);
        // the leader may draw up to 3 hunters; nobody exceeds its cap
        for i in 0..n {
            assert!(c.pursuers(i) <= max_pursuers(c.hunters[i].level, n, 3));
        }
        assert!(c.pursuers(5) >= 2, "leader drew {}", c.pursuers(5));
        // a fresh hunter does not draw a target within 15 m when others exist
        let mut pos2 = line(4);
        pos2[1] = pos2[0] + Vec3::X * 5.0;
        let mut c = Contracts::new(4);
        c.update(crate::DT, &pos2);
        assert_ne!(c.hunters[0].target, Some(1));
        assert_ne!(c.hunters[1].target, Some(0));
    }

    #[test]
    fn reassignment_after_kill_and_respawn() {
        let n = 4;
        let pos = line(n);
        let mut c = Contracts::new(n);
        c.update(crate::DT, &pos);
        let killer = 0;
        let victim = c.hunters[killer].target.unwrap();
        let ev = c.on_death(victim, Some(killer));
        assert!(ev.contains(&ContractEvent::Fulfilled { hunter: killer, target: victim }));
        assert!(!c.hunters[victim].active && c.hunters[victim].target.is_none());
        assert_eq!(c.pursuers(victim), 0);
        // nothing during the cooldown, then a new target that is not the victim
        let mut ticks = 0;
        while c.hunters[killer].target.is_none() {
            c.update(crate::DT, &pos);
            ticks += 1;
            assert!(ticks < 60 * 30);
        }
        assert!(ticks as f32 * crate::DT >= MIN_WAIT_TIME - 1e-3);
        assert_ne!(c.hunters[killer].target, Some(victim));
        // the victim respawns and hunts again after its cooldown
        c.respawn(victim);
        for _ in 0..(MIN_WAIT_TIME / crate::DT) as usize + 2 {
            c.update(crate::DT, &pos);
        }
        assert!(c.hunters[victim].target.is_some());
        // the soft level cap gave way (everyone else was full), the hard one holds
        for i in 0..n {
            assert!(c.pursuers(i) <= n - 2);
        }
        // a stunned hunter loses its contract
        let h = (0..n).find(|&i| c.hunters[i].target.is_some()).unwrap();
        let t = c.hunters[h].target.unwrap();
        assert_eq!(c.relation(t, h), Relation::Pursuer);
        assert_eq!(c.on_stunned(h), vec![ContractEvent::Lost { hunter: h, target: t }]);
    }

    #[test]
    fn compass_bands() {
        let h = Vec3::ZERO;
        assert_eq!(locate(h, Vec3::new(40.0, 0.0, 0.0), false, true).fill, 0.0);
        assert!((locate(h, Vec3::new(20.0, 0.0, 0.0), false, true).fill - 0.5).abs() < 1e-5);
        let l = locate(h, Vec3::new(8.0, 0.0, 0.0), false, true);
        assert!(l.fill == 1.0 && l.near && l.in_view);
        assert!(!locate(h, Vec3::new(8.0, 0.0, 0.0), false, false).in_view);
        assert!(!locate(h, Vec3::new(8.0, 0.0, 3.0), false, true).in_view);
        // hysteresis
        assert!(locate(h, Vec3::new(12.0, 0.0, 0.0), true, true).near);
        assert!(!locate(h, Vec3::new(12.0, 0.0, 0.0), false, true).near);
        assert!(threatened(h, Vec3::new(50.0, 0.0, 0.0), true));
        assert!(!threatened(h, Vec3::new(50.0, 0.0, 0.0), false));
    }
}
