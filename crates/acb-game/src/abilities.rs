//! Abilities, perks and streaks: the six slots of a persona's loadout
//! (`AbilitySlotActive1/2`, `Passive1/2`, `WinBonus`, `LoseBonus`).
//!
//! Values are the shipped objects of `AbilityManagerMulti` (`DataPC.forge`,
//! uid 0x639b4f79; regenerate with `acbmp dump DataPC.forge 0x<uid>`). The
//! rules are `AbilityManager` and `SlotAbility` (addresses at each item):
//! confidence as in the rest of the crate, C from code or data, R our
//! reading.
//!
//! Pure and deterministic: one [`State`] per player lives in
//! [`crate::mode::Match`], stepped by the match clock. An empty [`Loadout`]
//! changes nothing in a match.

use crate::mode::ticks;

/// What an active ability does beyond its timers. Field names follow the
/// retail properties.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Params {
    /// `GrantsHidden`: counts as hidden for the hide bonus (see
    /// `AbilityDisguise::GrantsHidden` in `manhunt`).
    Disguise { grants_hidden: bool },
    /// Own speed × `factor` for the effect.
    Speed { factor: f32 },
    /// Stuns everyone in `radius` for `stun` s, `delay` s after the drop.
    Smoke { radius: f32, stun: f32, delay: f32 },
    /// `focus` s of aiming; hit chance is at least `min_hit`.
    Gun { focus: f32, min_hit: f32 },
    /// Blinds in `radius`, fully inside `inner`.
    Fire { radius: f32, inner: f32 },
    /// Up to `count` civilians within `radius` become duplicates.
    Morph { count: u32, radius: f32 },
    /// The hit opponent's speed × `factor` for the effect.
    Knives { factor: f32 },
    /// Shows personas within `distance`.
    Vision { distance: f32 },
    /// A rush of `range`, speed boosted by `over_boost`.
    Charge { range: f32, over_boost: f32 },
    /// Civilians within `radius` turn into decoys; `disguised`: they use
    /// Disguise too (`DisguiseDelayMin/Max` 3..6 s).
    Decoy { radius: f32, disguised: bool },
    /// Personas within `radius` can't attack or use abilities (`AbilitySilent`).
    Mute { radius: f32 },
    /// The target within `range` dies when the effect ends; the killer gets
    /// `points` on top of the kill (`AbilityPoison.Score`).
    Poison { range: f32, points: u32 },
}

/// One shipped active ability (a base or one of its upgrades).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Active {
    pub name: &'static str,
    /// `CooldownDuration`, s.
    pub cooldown: f32,
    /// Effect length, s (`GetAbilityEffectDuration`): `Duration`, Poison's
    /// `Delay`; 0 for Smoke, Morph and Gun, which inherit `Ability`'s 0
    /// (0x0118c25e) and go straight to their cooldown.
    pub duration: f32,
    pub params: Params,
}

const fn a(name: &'static str, cooldown: f32, duration: f32, params: Params) -> Active {
    Active { name, cooldown, duration, params }
}

const fn disguise(h: bool) -> Params {
    Params::Disguise { grants_hidden: h }
}
const fn speed(factor: f32) -> Params {
    Params::Speed { factor }
}
const fn smoke(radius: f32, stun: f32) -> Params {
    Params::Smoke { radius, stun, delay: 0.75 }
}
const fn gun(focus: f32, min_hit: f32) -> Params {
    Params::Gun { focus, min_hit }
}
const fn fire(inner: f32) -> Params {
    Params::Fire { radius: 12.0, inner }
}
const fn morph(count: u32, radius: f32) -> Params {
    Params::Morph { count, radius }
}
const fn knives(factor: f32) -> Params {
    Params::Knives { factor }
}
const fn vision(distance: f32) -> Params {
    Params::Vision { distance }
}
const fn charge(range: f32) -> Params {
    Params::Charge { range, over_boost: 1.3 }
}
const fn decoy(disguised: bool) -> Params {
    Params::Decoy { radius: 15.0, disguised }
}
const fn mute(radius: f32) -> Params {
    Params::Mute { radius }
}
const fn poison(points: u32) -> Params {
    Params::Poison { range: 1.5, points }
}

/// Every active ability, in `AbilityReferenceList` order. Base Disguise's
/// 1 s cooldown and duration, and Rapid Reload Disguise's 10 s cooldown, are
/// what ships (every other Rapid Reload saves 10 s).
pub const ACTIVES: [Active; 48] = [
    a("DISGUISE", 1.0, 1.0, disguise(false)),
    a("LONG LASTING DISGUISE", 60.0, 20.0, disguise(false)),
    a("STRONG DISGUISE", 60.0, 15.0, disguise(true)),
    a("RAPID RELOAD DISGUISE", 10.0, 15.0, disguise(true)),
    a("SPRINT BOOST", 60.0, 5.0, speed(1.2)),
    a("LONG LASTING SPRINT BOOST", 60.0, 6.5, speed(1.2)),
    a("STRONG SPRINT BOOST", 60.0, 5.0, speed(1.3)),
    a("RAPID RELOAD SPRINT BOOST", 50.0, 5.0, speed(1.2)),
    a("SMOKE BOMB", 60.0, 0.0, smoke(3.2, 3.0)),
    a("STRONG SMOKE BOMB", 60.0, 0.0, smoke(4.0, 3.0)),
    a("LONG LASTING SMOKE BOMB", 60.0, 0.0, smoke(3.2, 4.0)),
    a("RAPID RELOAD SMOKE BOMB", 50.0, 0.0, smoke(3.2, 3.0)),
    a("HIDDEN GUN", 90.0, 0.0, gun(1.5, 0.1)),
    a("DEADLY GUN", 90.0, 0.0, gun(1.5, 0.3)),
    a("QUICK FIRING GUN", 90.0, 0.0, gun(1.0, 0.1)),
    a("RAPID RELOAD GUN", 80.0, 0.0, gun(1.5, 0.1)),
    a("FIRECRACKERS", 45.0, 6.0, fire(6.0)),
    a("STRONG FIRECRACKERS", 45.0, 6.0, fire(7.5)),
    a("LONG LASTING FIRECRACKERS", 45.0, 7.0, fire(6.0)),
    a("RAPID RELOAD FIRECRACKERS", 35.0, 6.0, fire(6.0)),
    a("MORPH", 60.0, 0.0, morph(4, 3.2)),
    a("POWER MORPH", 60.0, 0.0, morph(6, 3.2)),
    a("STRONG MORPH", 60.0, 0.0, morph(4, 4.5)),
    a("RAPID RELOAD MORPH", 50.0, 0.0, morph(4, 3.2)),
    a("THROWING KNIVES", 60.0, 3.5, knives(0.9)),
    a("LONG LASTING KNIVES", 60.0, 5.0, knives(0.9)),
    a("SHARP KNIVES", 60.0, 3.5, knives(0.8)),
    a("RAPID RELOAD KNIVES", 50.0, 3.5, knives(0.9)),
    a("TEMPLAR VISION", 60.0, 3.5, vision(30.0)),
    a("STRONG TEMPLAR VISION", 60.0, 3.5, vision(40.0)),
    a("LONG LASTING TEMPLAR VISION", 60.0, 5.0, vision(30.0)),
    a("RAPID RELOAD TEMPLAR VISION", 50.0, 3.5, vision(30.0)),
    a("CHARGE", 90.0, 2.5, charge(2.5)),
    a("STRONG CHARGE", 90.0, 2.5, charge(2.7)),
    a("LONG LASTING CHARGE", 90.0, 4.0, charge(2.5)),
    a("RAPID RELOAD CHARGE", 80.0, 2.5, charge(2.5)),
    a("DECOY", 45.0, 10.0, decoy(false)),
    a("LONG LASTING DECOY", 45.0, 15.0, decoy(false)),
    a("DISGUISED DECOY", 45.0, 10.0, decoy(true)),
    a("RAPID RELOAD DECOY", 35.0, 10.0, decoy(false)),
    a("MUTE", 90.0, 1.5, mute(6.0)),
    a("STRONG MUTE", 90.0, 1.5, mute(7.0)),
    a("LONG LASTING MUTE", 90.0, 2.0, mute(6.0)),
    a("RAPID RELOAD MUTE", 80.0, 1.5, mute(6.0)),
    a("POISON", 90.0, 5.0, poison(200)),
    a("FAST ACTING POISON", 90.0, 3.5, poison(150)),
    a("SLOW ACTING POISON", 90.0, 6.5, poison(300)),
    a("RAPID RELOAD POISON", 80.0, 5.0, poison(200)),
];

/// An index into [`ACTIVES`].
pub type ActiveId = u8;

/// The [`ACTIVES`] entry called `name`.
pub fn active(name: &str) -> Option<ActiveId> {
    ACTIVES.iter().position(|x| x.name == name).map(|i| i as ActiveId)
}

/// The passive slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perk {
    /// [`BASH_COUNT`]. Nothing to act on yet: we have no bash.
    AutoBash,
    /// "Run slightly higher up a wall when starting a climb".
    WallRunner,
    /// [`STUN_CUT`].
    Resistance,
    /// A blended player morphs one group member into a duplicate.
    Blender,
    /// [`ESCAPE_TIME_FACTOR`].
    Escapist,
    /// [`CHASE_SPEED_FACTOR`].
    ChaseBoost,
    /// [`COOLDOWN_ABATEMENT`].
    OverallCooldown,
    /// "Slow the rate at which your detection meter decreases"; no effect
    /// in retail: `AbilitySilentHunt::SpeedFactor` (10) and `Activated` have
    /// no reader (xrefs of 0x02e63e80, 0x02e63e7d). Neither have we.
    SilentHunt,
}

/// `AbilityAutoBash.CountOfBash`.
pub const BASH_COUNT: u32 = 2;
/// `AbilityResistance.DecreaseTime`, s off a stun.
pub const STUN_CUT: f32 = 3.0;
/// `AbilityEscapist.TimeFactor`: the escape meter's time ×0.75
/// (`GBrick_Escape::Update` 0x00c5f4fc divides the rate by it).
pub const ESCAPE_TIME_FACTOR: f32 = 0.75;
/// `AbilityChaseBoost.SpeedFactor` ("while chasing your target").
pub const CHASE_SPEED_FACTOR: f32 = 1.15;
/// `AbilityOverallCooldown.Abatement`, s off every cooldown.
pub const COOLDOWN_ABATEMENT: f32 = 10.0;

/// The `WinBonus` slot (`AbilityKillScoreBonus`, `AbilitySilentKillScoreBonus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillStreak {
    Kills3,
    Kills5,
    Silent3,
    Silent5,
}

impl KillStreak {
    /// `StreakValue`.
    pub fn every(self) -> u16 {
        match self {
            KillStreak::Kills3 | KillStreak::Silent3 => 3,
            KillStreak::Kills5 | KillStreak::Silent5 => 5,
        }
    }

    /// `ExtraPoints`.
    pub fn points(self) -> u32 {
        match self {
            KillStreak::Kills3 => 100,
            KillStreak::Kills5 => 250,
            KillStreak::Silent3 => 300,
            KillStreak::Silent5 => 750,
        }
    }

    /// Counts and pays silent kills only.
    pub fn silent(self) -> bool {
        matches!(self, KillStreak::Silent3 | KillStreak::Silent5)
    }
}

/// The `LoseBonus` slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossStreak {
    /// `AbilityExtraSensitivity`: compass precision, `Abatement` 3.
    ExtraPrecision,
    /// `AbilityResetCooldowns`: `factor` 1, `forceResetToZero`.
    ResetCooldowns,
    /// `AbilityResetCooldowns` "AbilityBoostCooldowns": `factor` 0.3.
    BoostCooldowns,
    /// `ExtraFactor` 2 on the next kill.
    ScoreX2,
}

impl LossStreak {
    /// `StreakValue`.
    pub fn after(self) -> u16 {
        match self {
            LossStreak::ExtraPrecision | LossStreak::ResetCooldowns => 3,
            LossStreak::BoostCooldowns | LossStreak::ScoreX2 => 5,
        }
    }

    /// `ms_CooldownFactor` while it runs.
    fn cooldown_factor(self) -> Option<f32> {
        match self {
            LossStreak::ResetCooldowns => Some(1.0),
            LossStreak::BoostCooldowns => Some(0.3),
            _ => None,
        }
    }
}

/// `AbilityExtraSensitivity.Abatement`.
pub const COMPASS_ABATEMENT: f32 = 3.0;

/// What a player brought into the match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loadout {
    pub actives: [Option<ActiveId>; 2],
    pub perks: [Option<Perk>; 2],
    pub kill_streak: Option<KillStreak>,
    pub loss_streak: Option<LossStreak>,
}

/// `SlotAbility`: effect, then cooldown, then ready.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    /// Ticks of effect left.
    pub effect: Option<u32>,
    /// Ticks of cooldown left.
    pub cooldown: Option<u32>,
    /// `+0x30`: the coming cooldown is 0.
    zero_next: bool,
}

/// A scored kill's points, ability parts included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KillScore {
    pub total: u32,
    /// The kill streak's `ExtraPoints` in `total` (0: none).
    pub streak: u32,
}

/// One player's abilities in a match.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub loadout: Loadout,
    pub slots: [Slot; 2],
    /// `AbilityManager+0x8a`: kills since the last death.
    pub kills: u16,
    /// `+0x8c`: silent kills since the last death.
    pub silent_kills: u16,
    /// `+0x88`: contracts lost since the last kill.
    pub losses: u16,
    /// The kill streak's bonus is due on the next kill.
    armed: bool,
    /// The loss streak runs.
    loss_running: bool,
    /// Ticks this player stays muted.
    pub muted: u32,
    /// Knives: ticks left and speed factor.
    pub slowed: Option<(u32, f32)>,
}

impl State {
    pub fn new(loadout: Loadout) -> State {
        State { loadout, ..State::default() }
    }

    pub fn has_perk(&self, perk: Perk) -> bool {
        self.loadout.perks.contains(&Some(perk))
    }

    /// The loss streak, if it runs.
    pub fn loss_running(&self) -> Option<LossStreak> {
        self.loadout.loss_streak.filter(|_| self.loss_running)
    }

    /// `Ability::GetAbilityCooldownDuration` 0x015edf26: a running cooldown
    /// streak's factor wins over the perk, which then does not apply.
    pub fn cooldown_secs(&self, base: f32) -> f32 {
        match self.loss_running().and_then(LossStreak::cooldown_factor) {
            Some(f) => base * f,
            None if self.has_perk(Perk::OverallCooldown) => (base - COOLDOWN_ABATEMENT).max(0.0),
            None => base,
        }
    }

    /// `SlotAbility::CanActivate` 0x00f390d4.
    pub fn can_activate(&self, slot: usize) -> bool {
        let s = &self.slots[slot];
        self.loadout.actives[slot].is_some() && s.effect.is_none() && s.cooldown.is_none()
    }

    /// `SlotAbility::Activate` 0x00f95b6e: the effect runs. Check
    /// [`State::can_activate`] first.
    pub fn activate(&mut self, slot: usize) {
        if let Some(id) = self.loadout.actives[slot] {
            self.slots[slot].effect = Some(ticks(ACTIVES[id as usize].duration));
        }
    }

    /// `SlotAbility::Update` 0x00f95a3c, one tick per slot: an elapsed
    /// effect starts the cooldown (0 after a reset during the effect); plus
    /// the effects received from others.
    pub fn tick(&mut self) {
        for i in 0..self.slots.len() {
            let Some(id) = self.loadout.actives[i] else { continue };
            let cooldown = ticks(self.cooldown_secs(ACTIVES[id as usize].cooldown));
            let s = &mut self.slots[i];
            if let Some(e) = &mut s.effect {
                *e = e.saturating_sub(1);
                if *e == 0 {
                    s.effect = None;
                    let t = if s.zero_next { 0 } else { cooldown };
                    s.zero_next = false;
                    s.cooldown = (t > 0).then_some(t);
                }
            } else if let Some(c) = &mut s.cooldown {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    s.cooldown = None;
                }
            }
        }
        self.muted = self.muted.saturating_sub(1);
        if let Some((t, _)) = &mut self.slowed {
            *t = t.saturating_sub(1);
            if *t == 0 {
                self.slowed = None;
            }
        }
    }

    /// `SlotAbility::ResetCooldowns` 0x00f94eb8: a slot in its effect will
    /// get no cooldown; with `force`, a cooling slot is ready now.
    pub fn reset_cooldowns(&mut self, force: bool) {
        for i in 0..self.slots.len() {
            if self.loadout.actives[i].is_none() {
                continue;
            }
            let s = &mut self.slots[i];
            if s.effect.is_some() {
                s.zero_next = true;
            } else if force {
                s.cooldown = None;
            }
        }
    }

    /// Dying ends running effects; their cooldowns start at the next tick
    /// (ours: `SlotAbility::RequestAbilityStop` 0x00f95426 exists, its death
    /// caller was not read).
    pub fn stop_effects(&mut self) {
        for s in &mut self.slots {
            if s.effect.is_some() {
                s.effect = Some(1);
            }
        }
    }

    /// The points of a scoring kill worth `base`, before the streak counts
    /// move. `(base + extra) × multiplier` as in `GameInfo` 0x01180946: extra
    /// from `GetAbilityExtraKillScoreForPlayer` 0x00f51b10 (a silent streak's
    /// bonus only on a silent kill), multiplier
    /// `GetMultiplicatorScoreFromAbilitiesForPlayer` 0x0111d15c.
    pub fn kill_score(&self, base: u32, silent: bool) -> KillScore {
        let streak = match self.loadout.kill_streak {
            Some(k) if self.armed && (silent || !k.silent()) => k.points(),
            _ => 0,
        };
        let factor = if self.loss_running() == Some(LossStreak::ScoreX2) { 2 } else { 1 };
        KillScore { total: (base + streak) * factor, streak }
    }

    /// `AbilityManager::UpdateKillCounts` 0x00f94d36, after the kill scored:
    /// a kill ends the loss streak. `silent`: the kill earned Silent or
    /// Incognito (`ScoringManager::PlayScoreFeedbacks` 0x015b1c04).
    ///
    /// `AbilityKillScoreBonus::StartResquest` 0x010db168 makes the streak
    /// periodic: the bonus arms at `count % every == every - 1` and disarms
    /// at `count % every == 0`, so every `every`-th kill pays.
    pub fn on_kill(&mut self, silent: bool) {
        self.kills = self.kills.saturating_add(1);
        if silent {
            self.silent_kills = self.silent_kills.saturating_add(1);
        }
        self.losses = 0;
        if let Some(k) = self.loadout.kill_streak {
            let n = k.every();
            let count = if k.silent() { self.silent_kills } else { self.kills };
            if count % n == n - 1 {
                self.armed = true;
            } else if count % n == 0 {
                self.armed = false;
            }
        }
        if self.loss_running {
            self.loss_running = false;
            // `AbilityResetCooldowns::OnStreakAbilityStop` 0x01119fa2
            if self.loadout.loss_streak.and_then(LossStreak::cooldown_factor).is_some() {
                self.reset_cooldowns(false);
            }
        }
    }

    /// `AbilityManager::HandleEndContractEvent` 0x00f94b86 for a lost
    /// contract. `died`: reasons 7 (suicide) and 8 (killed) also end the kill
    /// streak; 2 (target killed by another), 4 (escaped), 5, 6 (humiliated)
    /// don't. Returns whether the loss streak has just started.
    ///
    /// `LoseBonusAbility::StartResquest` 0x0163c210 runs at every loss from
    /// `after` on, and `AbilityResetCooldowns::Start` 0x01119f64 resets
    /// again each time ("after every loss or death").
    pub fn on_loss(&mut self, died: bool) -> bool {
        if died {
            self.kills = 0;
            self.silent_kills = 0;
            self.armed = false;
        }
        self.losses = self.losses.saturating_add(1);
        let Some(l) = self.loadout.loss_streak else { return false };
        if self.losses < l.after() {
            return false;
        }
        let started = !self.loss_running;
        self.loss_running = true;
        if l == LossStreak::ResetCooldowns {
            self.reset_cooldowns(true);
        }
        started
    }

    /// Own running speed effect × received knives.
    pub fn speed_factor(&self) -> f32 {
        let mut f = self.slowed.map_or(1.0, |(_, f)| f);
        for (i, s) in self.slots.iter().enumerate() {
            if let (Some(_), Some(id)) = (s.effect, self.loadout.actives[i]) {
                if let Params::Speed { factor } = ACTIVES[id as usize].params {
                    f *= factor;
                }
            }
        }
        f
    }

    /// A slot's running effect grants hiding (Disguise's `GrantsHidden`).
    pub fn effect_hides(&self) -> bool {
        self.slots.iter().enumerate().any(|(i, s)| {
            s.effect.is_some() && self.loadout.actives[i].is_some_and(|id| matches!(ACTIVES[id as usize].params, Params::Disguise { grants_hidden: true }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(actives: [&str; 2]) -> State {
        let id = |n: &str| if n.is_empty() { None } else { Some(active(n).unwrap()) };
        State::new(Loadout { actives: [id(actives[0]), id(actives[1])], ..Loadout::default() })
    }

    fn run(s: &mut State, secs: f32) {
        for _ in 0..ticks(secs) {
            s.tick();
        }
    }

    #[test]
    fn slot_cycle_effect_cooldown_ready() {
        let mut s = with(["SPRINT BOOST", ""]);
        assert!(!s.can_activate(1), "an empty slot never activates");
        assert!(s.can_activate(0));
        s.activate(0);
        assert!(!s.can_activate(0));
        assert_eq!(s.speed_factor(), 1.2);
        run(&mut s, 5.0);
        assert_eq!(s.speed_factor(), 1.0);
        assert_eq!(s.slots[0].cooldown, Some(ticks(60.0)));
        assert!(!s.can_activate(0));
        run(&mut s, 60.0);
        assert!(s.can_activate(0));
    }

    #[test]
    fn no_effect_abilities_go_straight_to_cooldown() {
        let mut s = with(["SMOKE BOMB", ""]);
        s.activate(0);
        s.tick();
        assert_eq!(s.slots[0].effect, None);
        assert_eq!(s.slots[0].cooldown, Some(ticks(60.0)));
    }

    #[test]
    fn kill_streak_pays_every_nth_kill() {
        let mut s = State::new(Loadout { kill_streak: Some(KillStreak::Kills3), ..Loadout::default() });
        let mut paid = Vec::new();
        for _ in 0..7 {
            paid.push(s.kill_score(100, false).streak);
            s.on_kill(false);
        }
        assert_eq!(paid, [0, 0, 100, 0, 0, 100, 0]);
    }

    #[test]
    fn silent_streak_counts_and_pays_only_silent_kills() {
        let mut s = State::new(Loadout { kill_streak: Some(KillStreak::Silent3), ..Loadout::default() });
        s.on_kill(true);
        s.on_kill(false);
        s.on_kill(true);
        assert_eq!((s.kills, s.silent_kills), (3, 2));
        assert_eq!(s.kill_score(100, false).streak, 0, "armed, but this kill is not silent");
        assert_eq!(s.kill_score(100, true).streak, 300);
    }

    #[test]
    fn only_death_ends_a_kill_streak() {
        let mut s = State::new(Loadout { kill_streak: Some(KillStreak::Kills3), ..Loadout::default() });
        s.on_kill(false);
        s.on_kill(false);
        s.on_loss(false);
        assert_eq!(s.kills, 2, "an escape or a humiliation keeps it");
        assert_eq!(s.kill_score(100, false).streak, 100);
        s.on_loss(true);
        assert_eq!(s.kills, 0);
        assert_eq!(s.kill_score(100, false).streak, 0);
    }

    #[test]
    fn score_x2_after_five_losses_doubles_one_kill() {
        let mut s = State::new(Loadout { kill_streak: Some(KillStreak::Kills3), loss_streak: Some(LossStreak::ScoreX2), ..Loadout::default() });
        s.on_kill(false);
        s.on_kill(false);
        let started: Vec<bool> = (0..6).map(|_| s.on_loss(false)).collect();
        assert_eq!(started, [false, false, false, false, true, false]);
        // (base + extra) × factor
        assert_eq!(s.kill_score(100, false), KillScore { total: 400, streak: 100 });
        s.on_kill(false);
        assert_eq!(s.losses, 0);
        assert_eq!(s.kill_score(100, false).total, 100);
    }

    #[test]
    fn reset_cooldowns_every_loss_from_three() {
        let mut s = with(["SPRINT BOOST", "MUTE"]);
        s.loadout.loss_streak = Some(LossStreak::ResetCooldowns);
        s.activate(0);
        run(&mut s, 6.0);
        assert!(s.slots[0].cooldown.is_some());
        s.on_loss(false);
        s.on_loss(false);
        assert!(s.slots[0].cooldown.is_some(), "not before the third loss");
        s.on_loss(false);
        assert!(s.can_activate(0));
        // during an effect: no cooldown afterwards
        s.activate(1);
        s.on_loss(false);
        run(&mut s, 2.0);
        assert!(s.can_activate(1));
    }

    #[test]
    fn cooldown_modifiers() {
        let mut s = with(["SPRINT BOOST", ""]);
        assert_eq!(s.cooldown_secs(60.0), 60.0);
        s.loadout.perks[0] = Some(Perk::OverallCooldown);
        assert_eq!(s.cooldown_secs(60.0), 50.0);
        assert_eq!(s.cooldown_secs(1.0), 0.0, "never negative");
        s.loadout.loss_streak = Some(LossStreak::BoostCooldowns);
        for _ in 0..5 {
            s.on_loss(false);
        }
        assert!((s.cooldown_secs(60.0) - 18.0).abs() < 1e-3, "the streak's factor replaces the perk");
        s.on_kill(false);
        assert_eq!(s.cooldown_secs(60.0), 50.0);
    }

    #[test]
    fn dying_ends_effects() {
        let mut s = with(["SPRINT BOOST", ""]);
        s.activate(0);
        s.stop_effects();
        s.tick();
        assert_eq!(s.speed_factor(), 1.0);
        assert_eq!(s.slots[0].cooldown, Some(ticks(60.0)));
    }

    #[test]
    fn table_lookup() {
        assert_eq!(ACTIVES[active("SLOW ACTING POISON").unwrap() as usize].params, Params::Poison { range: 1.5, points: 300 });
        assert_eq!(active("NOPE"), None);
    }
}
