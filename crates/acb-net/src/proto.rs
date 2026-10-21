//! Messages and their byte layout (little endian, hand packed).
//!
//! Every datagram starts with `MAGIC` (2 bytes), the protocol version and a
//! message type byte. See NOTES.md "Netcode (ours)".

use acb_game::PlayerInput;
use glam::Vec3;

pub const MAGIC: [u8; 2] = *b"AB";
pub const VERSION: u8 = 1;
/// Largest datagram we send or accept.
pub const MAX_PACKET: usize = 1400;
/// Inputs one client packet carries at most (redundancy window).
pub const MAX_INPUTS: usize = 32;
/// Players in a match at most (snapshot masks are 16 bits).
pub const MAX_PLAYERS: usize = 16;

// ---------------------------------------------------------------- inputs

/// A [`PlayerInput`] as sent: 9 buttons and the camera yaw in 1/65536
/// turns, 4 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireInput {
    pub buttons: u16,
    pub yaw: u16,
}

const B_FORWARD: u16 = 1;
const B_BACK: u16 = 2;
const B_LEFT: u16 = 4;
const B_RIGHT: u16 = 8;
const B_RUN: u16 = 16;
const B_SPRINT: u16 = 32;
const B_JUMP: u16 = 64;
const B_KILL: u16 = 128;
const B_STUN: u16 = 256;

impl WireInput {
    pub fn from_input(i: &PlayerInput) -> WireInput {
        let mut b = 0;
        for (on, bit) in [
            (i.forward, B_FORWARD),
            (i.back, B_BACK),
            (i.left, B_LEFT),
            (i.right, B_RIGHT),
            (i.run, B_RUN),
            (i.sprint, B_SPRINT),
            (i.jump, B_JUMP),
            (i.kill, B_KILL),
            (i.stun, B_STUN),
        ] {
            if on {
                b |= bit;
            }
        }
        let turns = (i.cam_yaw / std::f32::consts::TAU).rem_euclid(1.0);
        WireInput { buttons: b, yaw: ((turns * 65536.0).round() as u32 & 0xffff) as u16 }
    }

    pub fn to_input(self) -> PlayerInput {
        let b = |bit| self.buttons & bit != 0;
        PlayerInput {
            forward: b(B_FORWARD),
            back: b(B_BACK),
            left: b(B_LEFT),
            right: b(B_RIGHT),
            cam_yaw: self.yaw as f32 / 65536.0 * std::f32::consts::TAU,
            run: b(B_RUN),
            sprint: b(B_SPRINT),
            jump: b(B_JUMP),
            kill: b(B_KILL),
            stun: b(B_STUN),
        }
    }

    pub fn attacks(self) -> bool {
        self.buttons & (B_KILL | B_STUN) != 0
    }

    pub fn stuns(self) -> bool {
        self.buttons & B_STUN != 0
    }
}

/// What the simulation sees of `input` after the trip over the wire (the
/// client simulates this too, so both sides step the same values).
pub fn quantize_input(input: &PlayerInput) -> PlayerInput {
    WireInput::from_input(input).to_input()
}

// ------------------------------------------------------------- positions

/// A player's state in a snapshot, after retail `HumanPlayerNetPos`
/// (`SerializePlayerNetPos`): x, y as int16 cm (trunc, clamped), z as
/// trunc(z * 100 + 1) in the low 15 bits (signed, clamped to +-16383) with
/// bit 15 = moving, a yaw byte floor((wrap(yaw / pi) + 1) * 128); when
/// moving, int16 cm/s x, y velocity. Plus our action flags byte.
/// 8 bytes, 12 when moving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetPos {
    pub x: i16,
    pub y: i16,
    /// Low 15 bits z (two's complement), bit 15 moving.
    pub z: u16,
    pub yaw: u8,
    pub flags: u8,
    pub vx: i16,
    pub vy: i16,
}

pub const F_AIRBORNE: u8 = 1;
/// Playing a kill or stun as the assassin.
pub const F_ATTACKING: u8 = 2;
pub const F_CLIMBING: u8 = 4;
/// The body plays a scripted clip (a kill or stun victim).
pub const F_VICTIM: u8 = 8;

fn cm(v: f32) -> i16 {
    (v * 100.0).trunc().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Yaw byte as retail packs it: 256 steps per turn, 0 = -pi.
pub fn yaw_byte(yaw: f32) -> u8 {
    let h = yaw / std::f32::consts::PI;
    let wrapped = (h + 1.0).rem_euclid(2.0) - 1.0; // [-1, 1)
    (((wrapped + 1.0) * 128.0).floor() as i32).clamp(0, 255) as u8
}

pub fn yaw_from_byte(b: u8) -> f32 {
    (b as f32 / 128.0 - 1.0) * std::f32::consts::PI
}

impl NetPos {
    pub fn new(pos: Vec3, yaw: f32, vel: Vec3, flags: u8) -> NetPos {
        let zq = ((pos.z * 100.0 + 1.0).trunc() as i32).clamp(-16383, 16383);
        let moving = vel.truncate().length() > 0.05;
        let mut z = (zq as u16) & 0x7fff;
        if moving {
            z |= 0x8000;
        }
        let (vx, vy) = if moving { (cm(vel.x), cm(vel.y)) } else { (0, 0) };
        NetPos { x: cm(pos.x), y: cm(pos.y), z, yaw: yaw_byte(yaw), flags, vx, vy }
    }

    pub fn moving(&self) -> bool {
        self.z & 0x8000 != 0
    }

    pub fn pos(&self) -> Vec3 {
        // sign-extend the 15-bit z
        let zq = ((self.z << 1) as i16) >> 1;
        Vec3::new(self.x as f32 / 100.0, self.y as f32 / 100.0, (zq as f32 - 1.0) / 100.0)
    }

    pub fn yaw(&self) -> f32 {
        yaw_from_byte(self.yaw)
    }

    pub fn vel(&self) -> Vec3 {
        Vec3::new(self.vx as f32 / 100.0, self.vy as f32 / 100.0, 0.0)
    }

    pub fn write(&self, w: &mut Writer) {
        w.i16(self.x);
        w.i16(self.y);
        w.u16(self.z);
        w.u8(self.yaw);
        w.u8(self.flags);
        if self.moving() {
            w.i16(self.vx);
            w.i16(self.vy);
        }
    }

    pub fn read(r: &mut Reader) -> Option<NetPos> {
        let mut p = NetPos { x: r.i16()?, y: r.i16()?, z: r.u16()?, yaw: r.u8()?, flags: r.u8()?, vx: 0, vy: 0 };
        if p.moving() {
            p.vx = r.i16()?;
            p.vy = r.i16()?;
        }
        Some(p)
    }

    /// Same place to within one quantization step per axis (the two sides
    /// truncate floats that may differ in the last bits).
    pub fn close_to(&self, o: &NetPos) -> bool {
        let zq = |z: u16| (((z << 1) as i16) >> 1) as i32;
        (self.x as i32 - o.x as i32).abs() <= 1
            && (self.y as i32 - o.y as i32).abs() <= 1
            && (zq(self.z) - zq(o.z)).abs() <= 1
            && (self.yaw as i32 - o.yaw as i32).rem_euclid(256).min((o.yaw as i32 - self.yaw as i32).rem_euclid(256)) <= 1
    }
}

// ---------------------------------------------------------------- events

/// Outcomes the server decided, so clients show its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// a = assassin player, b = victim body.
    KillStarted = 1,
    StunStarted = 2,
    /// a = player, b = [`Reject`] reason.
    KillRejected = 3,
    /// a = victim body, b = assassin player.
    Death = 4,
    Stunned = 5,
    /// a = player (placed somewhere else or back in control).
    Respawn = 6,
}

impl EventKind {
    fn from_u8(v: u8) -> Option<EventKind> {
        use EventKind::*;
        Some(match v {
            1 => KillStarted,
            2 => StunStarted,
            3 => KillRejected,
            4 => Death,
            5 => Stunned,
            6 => Respawn,
            _ => return None,
        })
    }
}

/// Why the server did not start a kill or stun a player pressed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reject {
    /// Nobody in reach ahead (or the rules refused).
    NoTarget = 1,
    Airborne = 2,
    Climbing = 3,
    /// Already in a kill.
    Busy = 4,
}

impl Reject {
    pub fn name(b: u8) -> &'static str {
        match b {
            1 => "no target in reach",
            2 => "airborne",
            3 => "climbing",
            4 => "busy",
            _ => "?",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub tick: u32,
    pub kind: EventKind,
    pub a: u8,
    pub b: u8,
}

impl Event {
    fn write(&self, w: &mut Writer) {
        w.u32(self.tick);
        w.u8(self.kind as u8);
        w.u8(self.a);
        w.u8(self.b);
    }

    fn read(r: &mut Reader) -> Option<Event> {
        Some(Event { tick: r.u32()?, kind: EventKind::from_u8(r.u8()?)?, a: r.u8()?, b: r.u8()? })
    }

    pub fn describe(&self) -> String {
        match self.kind {
            EventKind::KillStarted => format!("tick {} player {} kills body {}", self.tick, self.a, self.b),
            EventKind::StunStarted => format!("tick {} player {} stuns body {}", self.tick, self.a, self.b),
            EventKind::KillRejected => format!("tick {} player {} kill rejected: {}", self.tick, self.a, Reject::name(self.b)),
            EventKind::Death => format!("tick {} body {} killed by player {}", self.tick, self.a, self.b),
            EventKind::Stunned => format!("tick {} body {} stunned by player {}", self.tick, self.a, self.b),
            EventKind::Respawn => format!("tick {} player {} respawns", self.tick, self.a),
        }
    }
}

// -------------------------------------------------------------- messages

/// A player slot of a match.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub name: String,
    /// EntityBuilder (persona) name.
    pub character: String,
    /// Start position and yaw instead of the slot's spawn.
    pub at: Option<[f32; 4]>,
}

/// One player's inputs as the server applied them, ticks `from..`,
/// run-length coded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputRuns {
    pub from: u32,
    pub runs: Vec<(u8, WireInput)>,
}

impl InputRuns {
    pub fn encode(from: u32, inputs: &[WireInput]) -> InputRuns {
        let mut runs: Vec<(u8, WireInput)> = Vec::new();
        for &i in inputs {
            match runs.last_mut() {
                Some((n, last)) if *last == i && *n < u8::MAX => *n += 1,
                _ => runs.push((1, i)),
            }
        }
        InputRuns { from, runs }
    }

    /// (tick, input) pairs.
    pub fn ticks(&self) -> impl Iterator<Item = (u32, WireInput)> + '_ {
        let mut t = self.from;
        self.runs.iter().flat_map(move |&(n, i)| {
            let start = t;
            t += n as u32;
            (start..start + n as u32).map(move |k| (k, i))
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Server tick this is the state after (0 = before the first step).
    pub tick: u32,
    /// Snapshot tick the positions are a delta against (0xffffffff: full).
    pub baseline: u32,
    /// Highest tick through which the server has all of this client's
    /// inputs (it resends from there).
    pub input_ack: u32,
    /// How many ticks ahead of the server this client's newest input
    /// arrived (smallest since the last snapshot).
    pub lead: i8,
    /// The client's clock of its newest input, and how long the server
    /// held it before this snapshot, for the round-trip time.
    pub echo_ms: u32,
    pub hold_ms: u16,
    /// Players whose [`NetPos`] is in `pos` (bit per player); the rest
    /// are unchanged from the baseline.
    pub changed: u16,
    pub players: u8,
    pub pos: Vec<NetPos>,
    /// Per player.
    pub inputs: Vec<InputRuns>,
    pub events: Vec<Event>,
}

pub const FULL: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    // client -> server
    Hello { name: String, character: String, map: String },
    Ping { client_ms: u32 },
    Ready,
    /// Inputs for ticks `last - inputs.len() + 1 ..= last`.
    Input { snapshot_ack: u32, client_ms: u32, last: u32, inputs: Vec<WireInput> },
    Bye,
    // server -> client
    Welcome { slot: u8, players: u8 },
    Reject { reason: String },
    Lobby { joined: u8, players: u8, echo_ms: u32 },
    Start { map: String, snapshot_every: u8, slots: Vec<Slot> },
    Snapshot(Snapshot),
    Over { reason: String },
}

const T_HELLO: u8 = 1;
const T_PING: u8 = 2;
const T_READY: u8 = 3;
const T_INPUT: u8 = 4;
const T_BYE: u8 = 5;
const T_WELCOME: u8 = 16;
const T_REJECT: u8 = 17;
const T_LOBBY: u8 = 18;
const T_START: u8 = 19;
const T_SNAPSHOT: u8 = 20;
const T_OVER: u8 = 21;

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(&MAGIC);
        w.u8(VERSION);
        match self {
            Msg::Hello { name, character, map } => {
                w.u8(T_HELLO);
                w.str(name);
                w.str(character);
                w.str(map);
            }
            Msg::Ping { client_ms } => {
                w.u8(T_PING);
                w.u32(*client_ms);
            }
            Msg::Ready => w.u8(T_READY),
            Msg::Input { snapshot_ack, client_ms, last, inputs } => {
                w.u8(T_INPUT);
                w.u32(*snapshot_ack);
                w.u32(*client_ms);
                w.u32(*last);
                w.u8(inputs.len().min(MAX_INPUTS) as u8);
                for i in inputs.iter().skip(inputs.len().saturating_sub(MAX_INPUTS)) {
                    w.u16(i.buttons);
                    w.u16(i.yaw);
                }
            }
            Msg::Bye => w.u8(T_BYE),
            Msg::Welcome { slot, players } => {
                w.u8(T_WELCOME);
                w.u8(*slot);
                w.u8(*players);
            }
            Msg::Reject { reason } => {
                w.u8(T_REJECT);
                w.str(reason);
            }
            Msg::Lobby { joined, players, echo_ms } => {
                w.u8(T_LOBBY);
                w.u8(*joined);
                w.u8(*players);
                w.u32(*echo_ms);
            }
            Msg::Start { map, snapshot_every, slots } => {
                w.u8(T_START);
                w.str(map);
                w.u8(*snapshot_every);
                w.u8(slots.len() as u8);
                for s in slots {
                    w.str(&s.name);
                    w.str(&s.character);
                    match s.at {
                        Some(a) => {
                            w.u8(1);
                            a.iter().for_each(|v| w.f32(*v));
                        }
                        None => w.u8(0),
                    }
                }
            }
            Msg::Snapshot(s) => {
                w.u8(T_SNAPSHOT);
                w.u32(s.tick);
                w.u32(s.baseline);
                w.u32(s.input_ack);
                w.u8(s.lead as u8);
                w.u32(s.echo_ms);
                w.u16(s.hold_ms);
                w.u8(s.players);
                w.u16(s.changed);
                for p in &s.pos {
                    p.write(&mut w);
                }
                for r in &s.inputs {
                    w.u32(r.from);
                    w.u8(r.runs.len() as u8);
                    for (n, i) in &r.runs {
                        w.u8(*n);
                        w.u16(i.buttons);
                        w.u16(i.yaw);
                    }
                }
                w.u8(s.events.len() as u8);
                for e in &s.events {
                    e.write(&mut w);
                }
            }
            Msg::Over { reason } => {
                w.u8(T_OVER);
                w.str(reason);
            }
        }
        w.0
    }

    pub fn decode(data: &[u8]) -> Option<Msg> {
        let mut r = Reader(data);
        if r.take(2)? != MAGIC || r.u8()? != VERSION {
            return None;
        }
        let msg = match r.u8()? {
            T_HELLO => Msg::Hello { name: r.str()?, character: r.str()?, map: r.str()? },
            T_PING => Msg::Ping { client_ms: r.u32()? },
            T_READY => Msg::Ready,
            T_INPUT => {
                let (snapshot_ack, client_ms, last, n) = (r.u32()?, r.u32()?, r.u32()?, r.u8()?);
                let inputs = (0..n).map(|_| Some(WireInput { buttons: r.u16()?, yaw: r.u16()? })).collect::<Option<_>>()?;
                Msg::Input { snapshot_ack, client_ms, last, inputs }
            }
            T_BYE => Msg::Bye,
            T_WELCOME => Msg::Welcome { slot: r.u8()?, players: r.u8()? },
            T_REJECT => Msg::Reject { reason: r.str()? },
            T_LOBBY => Msg::Lobby { joined: r.u8()?, players: r.u8()?, echo_ms: r.u32()? },
            T_START => {
                let map = r.str()?;
                let snapshot_every = r.u8()?;
                let n = r.u8()?;
                let mut slots = Vec::new();
                for _ in 0..n {
                    let (name, character) = (r.str()?, r.str()?);
                    let at = if r.u8()? != 0 { Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?]) } else { None };
                    slots.push(Slot { name, character, at });
                }
                Msg::Start { map, snapshot_every, slots }
            }
            T_SNAPSHOT => {
                let tick = r.u32()?;
                let baseline = r.u32()?;
                let input_ack = r.u32()?;
                let lead = r.u8()? as i8;
                let echo_ms = r.u32()?;
                let hold_ms = r.u16()?;
                let players = r.u8()?;
                let changed = r.u16()?;
                if players as usize > MAX_PLAYERS {
                    return None;
                }
                let pos = (0..changed.count_ones()).map(|_| NetPos::read(&mut r)).collect::<Option<_>>()?;
                let mut inputs = Vec::new();
                for _ in 0..players {
                    let from = r.u32()?;
                    let n = r.u8()?;
                    let runs = (0..n).map(|_| Some((r.u8()?, WireInput { buttons: r.u16()?, yaw: r.u16()? }))).collect::<Option<_>>()?;
                    inputs.push(InputRuns { from, runs });
                }
                let n = r.u8()?;
                let events = (0..n).map(|_| Event::read(&mut r)).collect::<Option<_>>()?;
                Msg::Snapshot(Snapshot { tick, baseline, input_ack, lead, echo_ms, hold_ms, changed, players, pos, inputs, events })
            }
            T_OVER => Msg::Over { reason: r.str()? },
            _ => return None,
        };
        r.0.is_empty().then_some(msg)
    }
}

/// Positions of every player from a delta snapshot and its baseline's.
pub fn apply_delta(baseline: Option<&[NetPos]>, s: &Snapshot) -> Option<Vec<NetPos>> {
    let n = s.players as usize;
    let mut out = match baseline {
        Some(b) if b.len() == n => b.to_vec(),
        _ if s.changed.count_ones() as usize == n => vec![NetPos::default(); n],
        _ => return None,
    };
    let mut it = s.pos.iter();
    for (i, p) in out.iter_mut().enumerate() {
        if s.changed & (1 << i) != 0 {
            *p = *it.next()?;
        }
    }
    Some(out)
}

/// The changed mask and the [`NetPos`] list of `now` against `baseline`.
pub fn make_delta(baseline: Option<&[NetPos]>, now: &[NetPos]) -> (u16, Vec<NetPos>) {
    let mut mask = 0;
    let mut pos = Vec::new();
    for (i, p) in now.iter().enumerate() {
        if baseline.and_then(|b| b.get(i)) != Some(p) {
            mask |= 1 << i;
            pos.push(*p);
        }
    }
    (mask, pos)
}

// ----------------------------------------------------------------- bytes

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.bytes(&v.to_le_bytes());
    }
    /// Up to 255 bytes of UTF-8 with a length byte.
    pub fn str(&mut self, s: &str) {
        let mut end = s.len().min(255);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u8(end as u8);
        self.bytes(&s.as_bytes()[..end]);
    }
}

pub struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netpos_round_trip() {
        let mut worst = Vec3::ZERO;
        let mut worst_yaw: f32 = 0.0;
        for i in 0..20000 {
            let f = i as f32;
            let pos = Vec3::new((f * 0.731).sin() * 300.0, (f * 0.377).cos() * 300.0, (f * 0.193).sin() * 150.0);
            let yaw = (f * 0.0123) % 20.0 - 10.0;
            let vel = Vec3::new((f * 0.11).sin() * 7.0, (f * 0.07).cos() * 7.0, 0.0);
            let q = NetPos::new(pos, yaw, vel, F_CLIMBING);
            let mut w = Writer::default();
            q.write(&mut w);
            assert_eq!(w.0.len(), if q.moving() { 12 } else { 8 });
            let back = NetPos::read(&mut Reader(&w.0)).unwrap();
            assert_eq!(back, q);
            let d = (back.pos() - pos).abs();
            worst = worst.max(d);
            // yaw: one step of 2 pi / 256 (floor), modulo a turn
            let dy = (back.yaw() - yaw).rem_euclid(std::f32::consts::TAU);
            let dy = dy.min(std::f32::consts::TAU - dy);
            worst_yaw = worst_yaw.max(dy);
            assert_eq!(back.flags, F_CLIMBING);
            if q.moving() {
                assert!((back.vel() - vel).abs().max_element() < 0.0101);
            }
        }
        // trunc: under 1 cm per axis (z carries the +1 cm offset back)
        assert!(worst.max_element() < 0.0101, "{worst:?}");
        assert!(worst_yaw <= std::f32::consts::TAU / 256.0 + 1e-5, "{worst_yaw}");
    }

    #[test]
    fn netpos_clamps_and_flags() {
        let q = NetPos::new(Vec3::new(1000.0, -1000.0, 500.0), 0.0, Vec3::ZERO, 0);
        assert_eq!((q.x, q.y), (i16::MAX, i16::MIN));
        assert!(!q.moving());
        assert!((q.pos().z - 163.82).abs() < 1e-3);
        let q = NetPos::new(Vec3::new(0.0, 0.0, -500.0), 0.0, Vec3::X, 0);
        assert!(q.moving());
        assert!((q.pos().z + 163.84).abs() < 1e-3, "{}", q.pos().z);
        // the retail yaw byte: -pi -> 0, 0 -> 128, just under pi -> 255
        assert_eq!(yaw_byte(-std::f32::consts::PI), 0);
        assert_eq!(yaw_byte(0.0), 128);
        assert_eq!(yaw_byte(std::f32::consts::PI - 1e-3), 255);
        assert_eq!(yaw_byte(std::f32::consts::PI), 0);
    }

    #[test]
    fn input_round_trip() {
        let i = PlayerInput { forward: true, right: true, cam_yaw: -2.5, run: true, kill: true, stun: true, ..Default::default() };
        let q = quantize_input(&i);
        assert!(q.forward && q.right && q.run && q.kill && q.stun && !q.back && !q.left && !q.sprint && !q.jump);
        let d = (q.cam_yaw - i.cam_yaw).rem_euclid(std::f32::consts::TAU);
        assert!(d.min(std::f32::consts::TAU - d) < 1e-4);
        // quantizing twice changes nothing: both sides step the same input
        assert_eq!(quantize_input(&q), q);
    }

    fn snapshot() -> Snapshot {
        let p = |x: f32| NetPos::new(Vec3::new(x, 2.0, 3.0), 1.0, Vec3::new(x, 0.0, 0.0), 0);
        let base = vec![p(1.0), p(0.0), p(5.0)];
        let now = vec![p(1.0), p(4.0), p(5.0)];
        let (changed, pos) = make_delta(Some(&base), &now);
        assert_eq!(changed, 0b010);
        let a = WireInput { buttons: 3, yaw: 7 };
        let b = WireInput { buttons: 0, yaw: 7 };
        Snapshot {
            tick: 120,
            baseline: 118,
            input_ack: 125,
            lead: -3,
            echo_ms: 12345,
            hold_ms: 4,
            changed,
            players: 3,
            pos,
            inputs: vec![InputRuns::encode(119, &[a, a, b]), InputRuns::encode(119, &[]), InputRuns::encode(110, &[b; 11])],
            events: vec![Event { tick: 119, kind: EventKind::KillRejected, a: 1, b: Reject::NoTarget as u8 }],
        }
    }

    #[test]
    fn messages_round_trip() {
        let s = snapshot();
        let msgs = vec![
            Msg::Hello { name: "a".into(), character: "AC2MP_ID07_MainTemplate_Player".into(), map: "x.forge".into() },
            Msg::Ping { client_ms: 9 },
            Msg::Ready,
            Msg::Input { snapshot_ack: 4, client_ms: 5, last: 99, inputs: vec![WireInput { buttons: 1, yaw: 2 }; 5] },
            Msg::Bye,
            Msg::Welcome { slot: 1, players: 2 },
            Msg::Reject { reason: "full".into() },
            Msg::Lobby { joined: 1, players: 2, echo_ms: 77 },
            Msg::Start {
                map: "m".into(),
                snapshot_every: 2,
                slots: vec![Slot { name: "a".into(), character: "c".into(), at: Some([1.0, 2.0, 3.0, 4.0]) }, Slot { name: "b".into(), character: "d".into(), at: None }],
            },
            Msg::Snapshot(s),
            Msg::Over { reason: "done".into() },
        ];
        for m in msgs {
            let bytes = m.encode();
            assert!(bytes.len() <= MAX_PACKET);
            assert_eq!(Msg::decode(&bytes), Some(m.clone()), "{m:?}");
            // truncated packets are refused, not misread
            assert_eq!(Msg::decode(&bytes[..bytes.len() - 1]), None);
        }
    }

    #[test]
    fn delta_snapshot_applies_on_baseline() {
        let s = snapshot();
        let p = |x: f32| NetPos::new(Vec3::new(x, 2.0, 3.0), 1.0, Vec3::new(x, 0.0, 0.0), 0);
        let base = vec![p(1.0), p(0.0), p(5.0)];
        assert_eq!(apply_delta(Some(&base), &s).unwrap(), vec![p(1.0), p(4.0), p(5.0)]);
        // a delta needs its baseline
        assert_eq!(apply_delta(None, &s), None);
        let ticks: Vec<_> = s.inputs[0].ticks().collect();
        assert_eq!(ticks.iter().map(|t| t.0).collect::<Vec<_>>(), vec![119, 120, 121]);
        assert_eq!(s.inputs[0].runs.len(), 2);
        assert_eq!(s.inputs[2].runs, vec![(11, WireInput { buttons: 0, yaw: 7 })]);
    }
}
