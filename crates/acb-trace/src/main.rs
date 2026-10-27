//! acb-trace: record retail ACBMP.exe (Wine) movement as CSV. See the kit README.

use acb_trace::{csv_row, Chain, Mem, Module, Offsets, ProcMem, Probe, Sample, CSV_HEADER};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const HELP: &str = "acb-trace: record retail ACBMP.exe (Wine) player movement as CSV

USAGE:
  acb-trace check [--diag FILE]                 attach, validate the pointer chain, write diag.txt
  acb-trace record --out FILE [--duration S]    sample at 60 Hz until Ctrl-C / SIGUSR1 / S seconds
  acb-trace where [--to X,Y,Z[,YAW]]            print position/yaw at 4 Hz (+ distance/heading error)

OPTIONS (all commands):
  --pid PID          attach to this process instead of searching /proc for ACBMP.exe
  --offsets FILE     offsets table (default: offsets.toml next to the binary, else built in)
  -h, --help         this help

Positions are in the acbmp-rs world convention (Z up, metres; yaw in degrees, 0 = facing +Y,
positive = counter-clockwise seen from above). Needs ptrace access to the game: run as the same
user with kernel.yama.ptrace_scope=0, or `sudo setcap cap_sys_ptrace+ep acb-trace`.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    STOP.store(true, Ordering::SeqCst);
}

extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
}

fn trap_signals() {
    // SIGINT = 2, SIGUSR1 = 10, SIGTERM = 15 on x86_64 Linux
    for s in [2, 10, 15] {
        unsafe {
            signal(s, on_signal);
        }
    }
}

struct Args {
    cmd: String,
    out: Option<PathBuf>,
    diag: PathBuf,
    duration: Option<f64>,
    to: Option<Vec<f32>>,
    pid: Option<u32>,
    offsets: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args { cmd: String::new(), out: None, diag: "diag.txt".into(), duration: None, to: None, pid: None, offsets: None };
    while let Some(x) = it.next() {
        let mut val = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match x.as_str() {
            "-h" | "--help" | "help" => a.cmd = "help".into(),
            "--out" => a.out = Some(val("--out")?.into()),
            "--diag" => a.diag = val("--diag")?.into(),
            "--duration" => a.duration = Some(val("--duration")?.parse().map_err(|_| "bad --duration")?),
            "--pid" => a.pid = Some(val("--pid")?.parse().map_err(|_| "bad --pid")?),
            "--offsets" => a.offsets = Some(val("--offsets")?.into()),
            "--to" => {
                let v: Result<Vec<f32>, _> = val("--to")?.split(',').map(|s| s.trim().parse()).collect();
                let v = v.map_err(|_| "bad --to, expected X,Y,Z[,YAW]")?;
                if !(3..=4).contains(&v.len()) {
                    return Err("--to takes X,Y,Z[,YAW]".into());
                }
                a.to = Some(v);
            }
            c if a.cmd.is_empty() && !c.starts_with('-') => a.cmd = c.to_string(),
            o => return Err(format!("unknown argument {o:?} (see --help)")),
        }
    }
    Ok(a)
}

fn load_offsets(a: &Args) -> Result<(Offsets, String), String> {
    let next_to_exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("offsets.toml")));
    let path = a.offsets.clone().or(next_to_exe.filter(|p| p.exists()));
    match path {
        Some(p) => {
            let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            Ok((Offsets::parse(&t).map_err(|e| format!("{}: {e}", p.display()))?, p.display().to_string()))
        }
        None => Ok((Offsets::embedded(), "built in".into())),
    }
}

/// (pid, cmdline) of the game: a process whose command line names ACBMP.exe.
fn find_game() -> Option<(u32, String)> {
    let me = std::process::id();
    let mut hits = Vec::new();
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        let Ok(raw) = std::fs::read(e.path().join("cmdline")) else { continue };
        let cmd = String::from_utf8_lossy(&raw).replace('\0', " ");
        let low = cmd.to_ascii_lowercase();
        if low.contains("acbmp.exe") && !low.contains("acb-trace") {
            // prefer the process that has the image mapped (Wine's preloader/exe process)
            let mapped = std::fs::read_to_string(format!("/proc/{pid}/maps")).is_ok_and(|m| m.to_ascii_lowercase().contains("acbmp.exe"));
            hits.push((!mapped, pid, cmd.trim().to_string()));
        }
    }
    hits.sort();
    hits.into_iter().next().map(|(_, p, c)| (p, c))
}

/// The ACBMP.exe image range from /proc/<pid>/maps (default 0x400000 + 36 MB).
fn module_of(pid: u32) -> (Module, String) {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default();
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    let mut first = String::new();
    for l in maps.lines().filter(|l| l.to_ascii_lowercase().ends_with("acbmp.exe")) {
        let Some((a, b)) = l.split_whitespace().next().and_then(|r| r.split_once('-')) else { continue };
        let (Ok(a), Ok(b)) = (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) else { continue };
        if a < lo {
            lo = a;
            first = l.to_string();
        }
        hi = hi.max(b);
    }
    if lo == u64::MAX || lo > u32::MAX as u64 {
        return (Module { base: 0x40_0000, end: 0x40_0000 + 0x240_0000 }, "ACBMP.exe not in maps; assuming base 0x400000".into());
    }
    // Wine may map only the headers from the file; the image spans at least 36 MB
    let end = hi.max(lo + 0x240_0000).min(u32::MAX as u64);
    (Module { base: lo as u32, end: end as u32 }, first)
}

struct Game {
    pid: u32,
    cmd: String,
    mem: ProcMem,
    module: Module,
    maps_line: String,
}

fn attach(a: &Args) -> Result<Game, (u8, String)> {
    let (pid, cmd) = match a.pid {
        Some(p) => (p, std::fs::read(format!("/proc/{p}/cmdline")).map(|r| String::from_utf8_lossy(&r).replace('\0', " ")).unwrap_or_default()),
        None => find_game().ok_or((
            1,
            "no ACBMP.exe process found in /proc (start the game under Wine first, or pass --pid)".to_string(),
        ))?,
    };
    let f = std::fs::File::open(format!("/proc/{pid}/mem")).map_err(|e| {
        (
            3,
            format!(
                "cannot open /proc/{pid}/mem: {e}\n{}",
                ptrace_hint()
            ),
        )
    })?;
    let (module, maps_line) = module_of(pid);
    let mem = ProcMem(f);
    if mem.u32(module.base).is_none() {
        return Err((3, format!("cannot read game memory at {:#x} (pid {pid})\n{}", module.base, ptrace_hint())));
    }
    Ok(Game { pid, cmd, mem, module, maps_line })
}

fn ptrace_hint() -> String {
    let scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into());
    format!(
        "kernel.yama.ptrace_scope = {scope}. Fix: `sudo sysctl kernel.yama.ptrace_scope=0` (until reboot) \
         or `sudo setcap cap_sys_ptrace+ep ./acb-trace`, and run as the same user as the game."
    )
}

fn hexdump(mem: &ProcMem, at: u32, len: u32, out: &mut String) {
    use std::fmt::Write as _;
    for row in (0..len).step_by(16) {
        let mut b = [0u8; 16];
        if mem.read(at + row, &mut b) {
            let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
            let _ = writeln!(out, "  +{row:04x}: {}", h.join(" "));
        } else {
            let _ = writeln!(out, "  +{row:04x}: unreadable");
        }
    }
}

fn check(a: &Args) -> Result<(), (u8, String)> {
    use std::fmt::Write as _;
    let (off, src) = load_offsets(a).map_err(|e| (2, e))?;
    let g = attach(a)?;
    let p = Probe { mem: &g.mem, off: &off, module: g.module };
    let mut d = String::new();
    let _ = writeln!(d, "acb-trace {} check", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(d, "pid {} cmdline {}", g.pid, g.cmd);
    let _ = writeln!(d, "module {:#x}..{:#x}  maps: {}", g.module.base, g.module.end, g.maps_line);
    let _ = writeln!(d, "offsets: {src}");
    let _ = writeln!(d, "module head: {:?}", g.mem.u16(g.module.base).map(|m| format!("{m:#06x} (MZ = 0x5a4d)")));
    let t0 = Instant::now();
    let mut log = Vec::new();
    let chain = p.resolve(&mut log);
    let _ = writeln!(d, "resolve ({:.2}s):", t0.elapsed().as_secs_f32());
    for l in &log {
        let _ = writeln!(d, "  {l}");
    }
    let mut ok = true;
    if let Some(c) = &chain {
        let _ = writeln!(d, "chain: {c:#x?}");
        let _ = writeln!(
            d,
            "pin these in offsets.toml once confirmed:\n  pad_mgr_off = {:#x}\n  components_off = {:#x}\n  matrix_off = {:#x}\n  transpose = {}\n  context_data_off = {:#x}",
            c.pad_mgr_off, c.components_off, c.matrix_off, c.transpose, c.context_data_off
        );
        if let Some((_, comps)) = p.components(c.player, c.components_off) {
            let _ = writeln!(d, "components:");
            for x in comps {
                let vt = g.mem.u32(x).unwrap_or(0);
                let _ = writeln!(d, "  {x:#x} vtable rva {:#x} {}", vt.wrapping_sub(g.module.base), p.class_name_of(x).unwrap_or("?"));
            }
        }
        let _ = writeln!(d, "entity {:#x}:", c.player);
        hexdump(&g.mem, c.player, 0x200, &mut d);
        let _ = writeln!(d, "actor {:#x}:", c.actor);
        hexdump(&g.mem, c.actor, 0x60, &mut d);
        // samples over a second: finite, named, advancing
        let mut rows = Vec::new();
        for _ in 0..30 {
            let s = p.sample(c);
            rows.push((t0.elapsed().as_secs_f64(), s));
            std::thread::sleep(Duration::from_millis(33));
        }
        let good: Vec<&(f64, Option<Sample>)> = rows.iter().filter(|r| r.1.as_ref().is_some_and(|s| s.pos.iter().all(|f| f.is_finite()))).collect();
        let named = good.iter().filter(|r| r.1.as_ref().is_some_and(|s| !s.state.is_empty())).count();
        let clocks: Vec<u32> = rows.iter().filter_map(|r| r.1.as_ref().and_then(|s| s.clock)).collect();
        let _ = writeln!(d, "samples: {} readable+finite of {}, {named} with a known state", good.len(), rows.len());
        for (t, s) in rows.iter().step_by(5) {
            let _ = writeln!(d, "  t {t:.3} {s:?}");
        }
        if off.clock_off != 0 {
            let adv = clocks.windows(2).any(|w| w[1] != w[0]);
            let _ = writeln!(d, "clock advances: {adv}");
            ok &= adv;
        } else {
            let _ = writeln!(d, "no game clock configured: host time is used");
        }
        ok &= good.len() == rows.len() && named == rows.len();
    } else {
        ok = false;
        if let Some(ctx) = g.mem.u32(g.module.base.wrapping_add(off.engine_ctx_rva)).filter(|&c| c != 0) {
            let _ = writeln!(d, "engine ctx {ctx:#x}:");
            hexdump(&g.mem, ctx, 0x100, &mut d);
        }
    }
    let _ = writeln!(d, "RESULT: {}", if ok { "OK" } else { "FAILED" });
    std::fs::write(&a.diag, &d).map_err(|e| (2, format!("{}: {e}", a.diag.display())))?;
    println!("{}", d.lines().filter(|l| !l.starts_with("  +")).take(40).collect::<Vec<_>>().join("\n"));
    println!("wrote {} - send it back", a.diag.display());
    if ok {
        Ok(())
    } else {
        Err((4, "chain check failed (details in diag.txt)".into()))
    }
}

fn resolve_or_fail<'a>(g: &'a Game, off: &'a Offsets) -> Result<(Probe<'a, ProcMem>, Chain), (u8, String)> {
    let p = Probe { mem: &g.mem, off, module: g.module };
    let mut log = Vec::new();
    match p.resolve(&mut log) {
        Some(c) => Ok((p, c)),
        None => Err((4, format!("could not resolve the local player (run `acb-trace check`):\n  {}", log.join("\n  ")))),
    }
}

fn record(a: &Args) -> Result<(), (u8, String)> {
    let out = a.out.as_deref().ok_or((2, "record needs --out FILE".to_string()))?;
    let (off, _) = load_offsets(a).map_err(|e| (2, e))?;
    let g = attach(a)?;
    let (p, mut chain) = resolve_or_fail(&g, &off)?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| (2, format!("{}: {e}", out.display())))?);
    writeln!(f, "{CSV_HEADER}").map_err(|e| (2, e.to_string()))?;
    trap_signals();
    eprintln!("recording {} (Ctrl-C or SIGUSR1 to stop{})", out.display(), a.duration.map_or(String::new(), |d| format!(", {d} s max")));
    let t0 = Instant::now();
    let step = Duration::from_secs_f64(1.0 / 60.0);
    let mut next = t0;
    let (mut tick, mut rows) = (0u64, 0u64);
    let mut prev: Option<(f64, [f32; 3])> = None;
    let mut last_clock = None;
    let mut lost_since: Option<Instant> = None;
    while !STOP.load(Ordering::SeqCst) {
        let t = t0.elapsed().as_secs_f64();
        if a.duration.is_some_and(|d| t >= d) {
            break;
        }
        match p.sample(&chain) {
            Some(s) if s.pos.iter().all(|f| f.is_finite()) => {
                lost_since = None;
                if s.clock.is_none() || s.clock != last_clock {
                    last_clock = s.clock;
                    let vel = match prev {
                        Some((pt, pp)) if t > pt => [0, 1, 2].map(|i| ((s.pos[i] - pp[i]) as f64 / (t - pt)) as f32),
                        _ => [0.0; 3],
                    };
                    let tk = s.clock.map_or(tick, u64::from);
                    writeln!(f, "{}", csv_row(tk, t, s.pos, s.yaw, vel, &s.state, &s.state, "", None)).map_err(|e| (2, e.to_string()))?;
                    prev = Some((t, s.pos));
                    rows += 1;
                }
            }
            _ => {
                // level change / respawn: re-resolve once a second
                let since = *lost_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(1) {
                    let mut log = Vec::new();
                    if let Some(c) = p.resolve(&mut log) {
                        chain = c;
                    }
                    lost_since = Some(Instant::now());
                }
            }
        }
        tick += 1;
        next += step;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    f.flush().map_err(|e| (2, e.to_string()))?;
    eprintln!("wrote {rows} rows in {:.1} s to {}", t0.elapsed().as_secs_f64(), out.display());
    Ok(())
}

fn wrap_deg(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

fn where_cmd(a: &Args) -> Result<(), (u8, String)> {
    let (off, _) = load_offsets(a).map_err(|e| (2, e))?;
    let g = attach(a)?;
    let (p, chain) = resolve_or_fail(&g, &off)?;
    trap_signals();
    while !STOP.load(Ordering::SeqCst) {
        if let Some(s) = p.sample(&chain) {
            let yaw = s.yaw.to_degrees();
            let mut line = format!("pos {:9.3} {:9.3} {:8.3}  yaw {yaw:7.1}  {:<18}", s.pos[0], s.pos[1], s.pos[2], s.state);
            if let Some(t) = &a.to {
                let d = [t[0] - s.pos[0], t[1] - s.pos[1], t[2] - s.pos[2]];
                let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
                // yaw that would face the target, in our convention
                let bearing = (-d[0]).atan2(d[1]).to_degrees();
                line += &format!("| dist {dist:6.2} m  dz {:+5.2}  turn {:+6.1}", d[2], wrap_deg(bearing - yaw));
                if let Some(ty) = t.get(3) {
                    line += &format!("  yaw err {:+6.1}", wrap_deg(ty - yaw));
                }
            }
            println!("{line}");
        } else {
            println!("(no sample: player not readable)");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn main() -> ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("acb-trace: {e}");
            return ExitCode::from(2);
        }
    };
    let r = match a.cmd.as_str() {
        "check" => check(&a),
        "record" => record(&a),
        "where" => where_cmd(&a),
        "" | "help" => {
            println!("{HELP}");
            return if a.cmd.is_empty() { ExitCode::from(2) } else { ExitCode::SUCCESS };
        }
        c => Err((2, format!("unknown command {c:?} (see --help)"))),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, msg)) => {
            eprintln!("acb-trace: {msg}");
            ExitCode::from(code)
        }
    }
}

