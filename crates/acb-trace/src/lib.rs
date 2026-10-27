//! Retail ACBMP.exe movement tracing: reads the local player of a running retail
//! client (32-bit, under Wine) through `/proc/<pid>/mem` and writes the same CSV
//! columns as our engine's `ACBMP_TRACE=csv:<path>`.
//!
//! Every lookup goes through [`Mem`], so the unit tests run against a synthetic
//! memory image ([`VecMem`]). Offsets come from `offsets.toml` (shipped next to the
//! binary, embedded as the default). Objects are identified by their scimitar class
//! id, read from the `mov eax, imm32; ret` GetClassID stub in their vtable, so the
//! chain is validated at every step and unknown offsets can be found by scanning.

use std::fmt::Write as _;

/// CSV columns, shared with `acb-game` (`ACBMP_TRACE=csv:<path>`). See
/// findings/trace-format.md.
pub const CSV_HEADER: &str = "tick,t,x,y,z,yaw_deg,vx,vy,vz,hspeed,state,retail_state,anim,anim_t";

/// The default offsets table, compiled in.
pub const DEFAULT_OFFSETS: &str = include_str!("../offsets.toml");

/// Process memory, addressed with 32-bit virtual addresses.
pub trait Mem {
    /// Fill `buf` from `addr`; false when any byte is unreadable.
    fn read(&self, addr: u32, buf: &mut [u8]) -> bool;

    fn u32(&self, addr: u32) -> Option<u32> {
        let mut b = [0; 4];
        self.read(addr, &mut b).then(|| u32::from_le_bytes(b))
    }
    fn u16(&self, addr: u32) -> Option<u16> {
        let mut b = [0; 2];
        self.read(addr, &mut b).then(|| u16::from_le_bytes(b))
    }
    fn f32s<const N: usize>(&self, addr: u32) -> Option<[f32; N]> {
        let mut b = vec![0; N * 4];
        if !self.read(addr, &mut b) {
            return None;
        }
        let mut out = [0.0; N];
        for (i, c) in b.chunks_exact(4).enumerate() {
            out[i] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        Some(out)
    }
}

/// A live process through `/proc/<pid>/mem` (pread).
pub struct ProcMem(pub std::fs::File);

impl Mem for ProcMem {
    fn read(&self, addr: u32, buf: &mut [u8]) -> bool {
        use std::os::unix::fs::FileExt;
        addr != 0 && self.0.read_exact_at(buf, addr as u64).is_ok()
    }
}

/// A synthetic image: `data` mapped at `base` (tests).
#[derive(Default)]
pub struct VecMem {
    pub base: u32,
    pub data: Vec<u8>,
}

impl VecMem {
    pub fn new(base: u32, len: usize) -> Self {
        Self { base, data: vec![0; len] }
    }
    pub fn put(&mut self, addr: u32, bytes: &[u8]) {
        let o = (addr - self.base) as usize;
        self.data[o..o + bytes.len()].copy_from_slice(bytes);
    }
    pub fn put_u32(&mut self, addr: u32, v: u32) {
        self.put(addr, &v.to_le_bytes());
    }
    pub fn put_u16(&mut self, addr: u32, v: u16) {
        self.put(addr, &v.to_le_bytes());
    }
    pub fn put_f32s(&mut self, addr: u32, v: &[f32]) {
        for (i, f) in v.iter().enumerate() {
            self.put(addr + 4 * i as u32, &f.to_le_bytes());
        }
    }
}

impl Mem for VecMem {
    fn read(&self, addr: u32, buf: &mut [u8]) -> bool {
        let Some(o) = addr.checked_sub(self.base) else { return false };
        let o = o as usize;
        match self.data.get(o..o + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        }
    }
}

/// The offsets table (`offsets.toml`): `key = value` lines, `#` comments, hex or
/// decimal numbers, `class.<Name> = id` and `vtable.<Name> = rva` entries.
#[derive(Clone, Debug, Default)]
pub struct Offsets {
    pub engine_ctx_rva: u32,
    pub world_off: u32,
    pub pad_mgr_off: u32,
    pub pad_slot: u32,
    pub components_off: u32,
    pub matrix_off: u32,
    pub transpose: bool,
    pub actor_context_off: u32,
    pub context_data_off: u32,
    pub clock_off: u32,
    pub classes: Vec<(String, u32)>,
    pub vtables: Vec<(String, u32)>,
}

fn num(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(&h.replace('_', ""), 16),
        None => s.replace('_', "").parse(),
    };
    r.map_err(|_| format!("bad number {s:?}"))
}

impl Offsets {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut o = Offsets::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            let e = |m: String| format!("line {}: {m}", n + 1);
            if let Some(c) = k.strip_prefix("class.") {
                o.classes.push((c.to_string(), num(v).map_err(e)?));
                continue;
            }
            if let Some(c) = k.strip_prefix("vtable.") {
                o.vtables.push((c.to_string(), num(v).map_err(e)?));
                continue;
            }
            if k == "transpose" {
                o.transpose = matches!(v, "true" | "1");
                continue;
            }
            let x = num(v).map_err(e)?;
            match k {
                "engine_ctx_rva" => o.engine_ctx_rva = x,
                "world_off" => o.world_off = x,
                "pad_mgr_off" => o.pad_mgr_off = x,
                "pad_slot" => o.pad_slot = x,
                "components_off" => o.components_off = x,
                "matrix_off" => o.matrix_off = x,
                "actor_context_off" => o.actor_context_off = x,
                "context_data_off" => o.context_data_off = x,
                "clock_off" => o.clock_off = x,
                _ => return Err(format!("line {}: unknown key {k:?}", n + 1)),
            }
        }
        Ok(o)
    }

    pub fn embedded() -> Self {
        Self::parse(DEFAULT_OFFSETS).expect("embedded offsets.toml parses")
    }

    pub fn class_id(&self, name: &str) -> Option<u32> {
        self.classes.iter().find(|(n, _)| n == name).map(|&(_, id)| id)
    }

    pub fn class_name(&self, id: u32) -> Option<&str> {
        self.classes.iter().find(|&&(_, i)| i == id).map(|(n, _)| n.as_str())
    }
}

/// Retail state name of a class: `HumanInAirData` -> `HumanInAir`.
pub fn state_of_class(name: &str) -> Option<&str> {
    name.strip_suffix("Data").filter(|n| n.starts_with("Human") && *n != "Human")
}

/// The game module's address range.
#[derive(Clone, Copy, Debug)]
pub struct Module {
    pub base: u32,
    pub end: u32,
}

impl Module {
    pub fn contains(&self, a: u32) -> bool {
        a >= self.base && a < self.end
    }
}

/// A resolved pointer chain to the local player.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    pub engine_ctx: u32,
    pub world: u32,
    pub pad_mgr_off: u32,
    pub player: u32,
    pub components_off: u32,
    pub actor: u32,
    pub matrix_off: u32,
    pub transpose: bool,
    pub context_data_off: u32,
}

/// One reading of the local player, in our world convention.
#[derive(Clone, Debug)]
pub struct Sample {
    pub pos: [f32; 3],
    /// Our `Player::yaw` (radians): entity +Y = (-sin yaw, cos yaw).
    pub yaw: f32,
    /// Retail state (`HumanGround`, ...) or empty.
    pub state: String,
    pub clock: Option<u32>,
}

pub struct Probe<'a, M: Mem> {
    pub mem: &'a M,
    pub off: &'a Offsets,
    pub module: Module,
}

/// Rows of a 4x4 matrix as stored: x axis, y axis (forward), z axis, translation;
/// `transpose` = translation in floats 3/7/11.
pub fn axes(m: &[f32; 16], transpose: bool) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
    if transpose {
        ([m[0], m[4], m[8]], [m[1], m[5], m[9]], [m[2], m[6], m[10]], [m[3], m[7], m[11]])
    } else {
        ([m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]], [m[12], m[13], m[14]])
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Does `m` look like a world transform (orthonormal 3x3, homogeneous column,
/// finite translation within 100 km)? Returns the layout (`transpose`).
pub fn matrix_layout(m: &[f32; 16]) -> Option<bool> {
    if m.iter().any(|f| !f.is_finite()) {
        return None;
    }
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
    for t in [false, true] {
        let w = if t { [m[12], m[13], m[14], m[15]] } else { [m[3], m[7], m[11], m[15]] };
        if !(near(w[0], 0.0) && near(w[1], 0.0) && near(w[2], 0.0) && near(w[3], 1.0)) {
            continue;
        }
        let (x, y, z, p) = axes(m, t);
        let unit = [x, y, z].iter().all(|a| near(dot(*a, *a), 1.0));
        let ortho = near(dot(x, y), 0.0) && near(dot(y, z), 0.0) && near(dot(x, z), 0.0);
        if unit && ortho && p.iter().all(|c| c.abs() < 1e5) && dot(p, p) > 1e-6 {
            return Some(t);
        }
    }
    None
}

/// Our yaw (radians) from a forward (+Y) axis.
pub fn yaw_of(fwd: [f32; 3]) -> f32 {
    (-fwd[0]).atan2(fwd[1])
}

impl<'a, M: Mem> Probe<'a, M> {
    /// Class id of the object at `obj` (vtable fast path, then a GetClassID scan
    /// over the first 16 vtable slots); only ids in the table count.
    pub fn class_of(&self, obj: u32) -> Option<u32> {
        let vt = self.mem.u32(obj)?;
        if !self.module.contains(vt) {
            return None;
        }
        if let Some((n, _)) = self.off.vtables.iter().find(|&&(_, rva)| self.module.base.wrapping_add(rva) == vt) {
            return self.off.class_id(n);
        }
        for i in 0..16 {
            let f = self.mem.u32(vt + 4 * i)?;
            if !self.module.contains(f) {
                continue;
            }
            let mut b = [0u8; 6];
            if self.mem.read(f, &mut b) && b[0] == 0xb8 && b[5] == 0xc3 {
                let id = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
                if self.off.class_name(id).is_some() {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn class_name_of(&self, obj: u32) -> Option<&'a str> {
        let id = self.class_of(obj)?;
        self.off.class_name(id)
    }

    fn is(&self, obj: u32, name: &str) -> bool {
        obj != 0 && self.class_of(obj).is_some() && self.class_of(obj) == self.off.class_id(name)
    }

    /// Entity component array: (offset in the entity, component pointers).
    pub fn components(&self, ent: u32, fixed: u32) -> Option<(u32, Vec<u32>)> {
        let read = |off: u32| -> Option<Vec<u32>> {
            let arr = self.mem.u32(ent + off)?;
            let n = (self.mem.u16(ent + off + 6)? & 0x3fff) as u32;
            if arr == 0 || n == 0 || n > 64 {
                return None;
            }
            (0..n).map(|i| self.mem.u32(arr + 4 * i)).collect()
        };
        let has_actor = |c: &Vec<u32>| c.iter().any(|&p| self.is(p, "AIActor"));
        if fixed != 0 {
            return read(fixed).map(|c| (fixed, c));
        }
        (0..0x200).step_by(4).find_map(|off| read(off).filter(has_actor).map(|c| (off, c)))
    }

    /// Retail state of an AIActor: (state name, data offset used).
    pub fn state(&self, actor: u32, data_off: u32) -> Option<(String, u32)> {
        let ctx = self.mem.u32(actor + self.off.actor_context_off)?;
        if ctx == 0 {
            return None;
        }
        let try_off = |o: u32| {
            let d = self.mem.u32(ctx + o)?;
            let name = self.class_name_of(d)?;
            state_of_class(name).map(|s| (s.to_string(), o))
        };
        if data_off != 0 {
            if let Some(r) = try_off(data_off) {
                return Some(r);
            }
        }
        (0..0x40).step_by(4).find_map(try_off)
    }

    /// World matrix of an entity: (offset, transpose).
    pub fn matrix(&self, ent: u32, fixed: u32) -> Option<(u32, bool)> {
        let at = |off: u32| self.mem.f32s::<16>(ent + off).and_then(|m| matrix_layout(&m).map(|t| (off, t)));
        if fixed != 0 {
            return self.mem.f32s::<16>(ent + fixed).map(|_| (fixed, self.off.transpose));
        }
        (0..0x200).step_by(4).find_map(at)
    }

    /// The local player entity behind a world, with its components and actor.
    fn player_in(&self, world: u32, log: &mut Vec<String>) -> Option<(u32, u32, u32, u32)> {
        let slot = self.off.pad_slot;
        let at = |pm_off: u32, i: u32| -> Option<(u32, u32, u32)> {
            let pm = self.mem.u32(world + pm_off)?;
            let arr = self.mem.u32(pm + 4)?;
            let e = self.mem.u32(arr + 4 * i)?;
            if e == 0 {
                return None;
            }
            let (coff, comps) = self.components(e, self.off.components_off)?;
            let actor = comps.into_iter().find(|&p| self.is(p, "AIActor"))?;
            self.state(actor, self.off.context_data_off)?;
            Some((e, coff, actor))
        };
        if self.off.pad_mgr_off != 0 {
            let r = at(self.off.pad_mgr_off, slot);
            if r.is_none() {
                log.push(format!("pinned pad_mgr_off {:#x} slot {slot}: no player with a Human* state", self.off.pad_mgr_off));
            }
            return r.map(|(e, c, a)| (self.off.pad_mgr_off, e, c, a));
        }
        for off in (0..0x2000).step_by(4) {
            for i in std::iter::once(slot).chain((0..4).filter(|&i| i != slot)) {
                if let Some((e, c, a)) = at(off, i) {
                    log.push(format!("player found by scan: world+{off:#x} -> +4 -> [{i}]"));
                    return Some((off, e, c, a));
                }
            }
        }
        log.push("no player array found in world+0..0x2000".into());
        None
    }

    /// Resolve the whole chain; `log` collects what was tried (diag.txt).
    pub fn resolve(&self, log: &mut Vec<String>) -> Option<Chain> {
        let g = self.module.base.wrapping_add(self.off.engine_ctx_rva);
        let ctx = self.mem.u32(g).filter(|&c| c != 0);
        log.push(format!("engine ctx global {g:#x} -> {}", ctx.map_or("unreadable/null".into(), |c| format!("{c:#x}"))));
        let ctx = ctx?;
        let mut world = self.mem.u32(ctx + self.off.world_off).unwrap_or(0);
        if self.is(world, "World") {
            log.push(format!("world = [ctx+{:#x}] = {world:#x} (class World)", self.off.world_off));
        } else {
            log.push(format!("[ctx+{:#x}] = {world:#x} is not a World; scanning ctx+0..0x100", self.off.world_off));
            let found = (0..0x100).step_by(4).find_map(|o| self.mem.u32(ctx + o).filter(|&w| self.is(w, "World")).map(|w| (o, w)));
            match found {
                Some((o, w)) => {
                    log.push(format!("world found at ctx+{o:#x} = {w:#x}"));
                    world = w;
                }
                None => {
                    log.push("no World pointer in the engine context".into());
                    return None;
                }
            }
        }
        let (pad_mgr_off, player, components_off, actor) = self.player_in(world, log)?;
        log.push(format!(
            "player entity {player:#x} (class {}), components at +{components_off:#x}, AIActor {actor:#x}",
            self.class_name_of(player).unwrap_or("?")
        ));
        let (_, context_data_off) = self.state(actor, self.off.context_data_off)?;
        let Some((matrix_off, transpose)) = self.matrix(player, self.off.matrix_off) else {
            log.push("no world matrix found in entity+0..0x200".into());
            return None;
        };
        log.push(format!("matrix at entity+{matrix_off:#x} transpose {transpose}; context data at +{context_data_off:#x}"));
        Some(Chain { engine_ctx: ctx, world, pad_mgr_off, player, components_off, actor, matrix_off, transpose, context_data_off })
    }

    pub fn sample(&self, c: &Chain) -> Option<Sample> {
        let m = self.mem.f32s::<16>(c.player + c.matrix_off)?;
        let (_, fwd, _, pos) = axes(&m, c.transpose);
        let state = self.state(c.actor, c.context_data_off).map(|s| s.0).unwrap_or_default();
        let clock = (self.off.clock_off != 0).then(|| self.mem.u32(c.world + self.off.clock_off)).flatten();
        Some(Sample { pos, yaw: yaw_of(fwd), state, clock })
    }
}

/// One CSV row (same formatting as acb-game's writer).
#[allow(clippy::too_many_arguments)]
pub fn csv_row(tick: u64, t: f64, pos: [f32; 3], yaw: f32, vel: [f32; 3], state: &str, retail: &str, anim: &str, anim_t: Option<f32>) -> String {
    let mut s = String::new();
    let hs = (vel[0] * vel[0] + vel[1] * vel[1]).sqrt();
    let _ = write!(
        s,
        "{tick},{t:.4},{:.4},{:.4},{:.4},{:.2},{:.4},{:.4},{:.4},{hs:.4},{state},{retail},{anim},",
        pos[0],
        pos[1],
        pos[2],
        yaw.to_degrees(),
        vel[0],
        vel[1],
        vel[2]
    );
    if let Some(a) = anim_t {
        let _ = write!(s, "{a:.4}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x40_0000;
    const HEAP: u32 = 0x1000_0000;

    /// A fake module (vtables + GetClassID stubs) and heap with a full chain.
    fn image(off: &Offsets, state: &str, pos: [f32; 3], yaw: f32) -> (VecMem, VecMem) {
        let mut m = VecMem::new(BASE, 0x10_0000);
        let mut h = VecMem::new(HEAP, 0x10_0000);
        // one vtable per class at BASE+0x1000*k, GetClassID in slot 3, stub at +0x800
        let mut vt = std::collections::HashMap::new();
        for (k, (n, id)) in off.classes.iter().enumerate() {
            let v = BASE + 0x1000 * (k as u32 + 1);
            let f = v + 0x800;
            m.put_u32(v + 12, f);
            let mut stub = vec![0xb8];
            stub.extend_from_slice(&id.to_le_bytes());
            stub.push(0xc3);
            m.put(f, &stub);
            vt.insert(n.clone(), v);
        }
        // engine ctx global -> ctx; ctx+world_off -> world
        let (ctx, world, pm, arr, ent, comps, actor, context, data) =
            (HEAP + 0x100, HEAP + 0x1000, HEAP + 0x8000, HEAP + 0x8100, HEAP + 0x9000, HEAP + 0x9800, HEAP + 0xa000, HEAP + 0xb000, HEAP + 0xc000);
        let mut mm = m;
        mm.put_u32(BASE + off.engine_ctx_rva, ctx);
        h.put_u32(ctx + off.world_off, world);
        h.put_u32(world, vt["World"]);
        h.put_u32(world + 0x9c0, pm);
        h.put_u32(pm + 4, arr);
        h.put_u32(arr, ent);
        h.put_u32(ent, vt["Entity"]);
        h.put_u32(ent + 0x6c, comps);
        h.put_u16(ent + 0x72, 3);
        h.put_u32(comps, HEAP + 0xd000); // unknown component
        h.put_u32(comps + 4, actor);
        h.put_u32(comps + 8, HEAP + 0xd100);
        h.put_u32(actor, vt["AIActor"]);
        h.put_u32(actor + 0x18, context);
        h.put_u32(context + 0x10, data);
        h.put_u32(data, vt[&format!("{state}Data")]);
        let (s, c) = yaw.sin_cos();
        h.put_f32s(ent + 0x100, &[c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, pos[0], pos[1], pos[2], 1.0]);
        (mm, h)
    }

    struct Two(VecMem, VecMem);
    impl Mem for Two {
        fn read(&self, a: u32, b: &mut [u8]) -> bool {
            self.0.read(a, b) || self.1.read(a, b)
        }
    }

    #[test]
    fn embedded_table_parses() {
        let o = Offsets::embedded();
        assert_eq!(o.engine_ctx_rva, 0x0226_23f0);
        assert_eq!(o.class_id("HumanInAirData"), Some(0x9edc_74ac));
        assert_eq!(state_of_class("HumanClimbData"), Some("HumanClimb"));
        assert_eq!(state_of_class("HumanData"), None);
        assert!(Offsets::parse("bogus = 1").is_err());
    }

    #[test]
    fn resolves_by_scanning() {
        let mut off = Offsets::embedded();
        off.vtables.clear(); // exercise the GetClassID scan
        off.engine_ctx_rva = 0x8_0000;
        let (m, h) = image(&off, "HumanInAir", [12.5, -3.0, 7.25], 0.5);
        let mem = Two(m, h);
        let p = Probe { mem: &mem, off: &off, module: Module { base: BASE, end: BASE + 0x10_0000 } };
        let mut log = Vec::new();
        let c = p.resolve(&mut log).unwrap_or_else(|| panic!("{log:#?}"));
        assert_eq!((c.pad_mgr_off, c.components_off, c.matrix_off, c.transpose), (0x9c0, 0x6c, 0x100, false));
        let s = p.sample(&c).unwrap();
        assert_eq!(s.state, "HumanInAir");
        assert_eq!(s.pos, [12.5, -3.0, 7.25]);
        assert!((s.yaw - 0.5).abs() < 1e-5, "{}", s.yaw);
    }

    #[test]
    fn pinned_offsets_and_bad_world() {
        let mut off = Offsets::embedded();
        off.vtables.clear();
        off.engine_ctx_rva = 0x8_0000;
        off.pad_mgr_off = 0x9c0;
        off.components_off = 0x6c;
        off.matrix_off = 0x100;
        let (m, mut h) = image(&off, "HumanClimb", [1.0, 2.0, 3.0], 0.0);
        // move the world pointer: the probe has to find it by class
        h.put_u32(HEAP + 0x100 + off.world_off, 0);
        h.put_u32(HEAP + 0x100 + 0x20, HEAP + 0x1000);
        let mem = Two(m, h);
        let p = Probe { mem: &mem, off: &off, module: Module { base: BASE, end: BASE + 0x10_0000 } };
        let mut log = Vec::new();
        let c = p.resolve(&mut log).unwrap();
        assert_eq!(p.sample(&c).unwrap().state, "HumanClimb");
        assert!(log.iter().any(|l| l.contains("world found at ctx+0x20")));
    }

    #[test]
    fn no_chain_in_empty_memory() {
        let off = Offsets::embedded();
        let mem = VecMem::new(BASE, 0x1000);
        let p = Probe { mem: &mem, off: &off, module: Module { base: BASE, end: BASE + 0x1000 } };
        let mut log = Vec::new();
        assert!(p.resolve(&mut log).is_none());
        assert!(log[0].contains("unreadable"));
    }

    #[test]
    fn matrix_layouts() {
        let id = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, 6.0, 7.0, 1.0];
        assert_eq!(matrix_layout(&id), Some(false));
        let tr = [1.0, 0.0, 0.0, 5.0, 0.0, 1.0, 0.0, 6.0, 0.0, 0.0, 1.0, 7.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(matrix_layout(&tr), Some(true));
        assert_eq!(axes(&tr, true).3, [5.0, 6.0, 7.0]);
        assert_eq!(matrix_layout(&[0.0; 16]), None);
    }

    #[test]
    fn row_format() {
        let r = csv_row(3, 0.05, [1.0, 2.0, 3.0], 0.0, [3.0, 4.0, 0.0], "ground", "HumanGround", "", None);
        assert_eq!(r, "3,0.0500,1.0000,2.0000,3.0000,0.00,3.0000,4.0000,0.0000,5.0000,ground,HumanGround,,");
        assert_eq!(r.split(',').count(), CSV_HEADER.split(',').count());
    }
}
