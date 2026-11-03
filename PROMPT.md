# Next agent: port ACB MP physics and movement (Phase 3, parkour fidelity)

You continue the acbmp-rs rewrite of Assassin's Creed Brotherhood multiplayer. Your task is to reverse
engineer retail's physics and parkour (the `Human*` state machine) and port it into `crates/acb-game`.
Work one mechanic at a time: find it in the binary, port it, test it, run the regression, commit.

## Rules (from the user; they override defaults)
- Gameplay and physics fidelity come first; visuals are parked.
- Behaviour must be identical at 60 and 144 fps: a fixed 60 Hz sim tick, so no frame time in the sim.
- Retail values come from the binary or the game data, never from guesses. When something is ours (not
  retail), say so in the code doc comment and in NOTES ("Ours: ...").
- No manual jump: a button press alone never jumps. Jumps only come out of freerun (high profile + Legs)
  at an edge, gap or ledge. Don't port retail states that turn a lone press into a jump (JumpImpulsion as a
  standing jump was rejected).
- San Donato is the reference map; Whiteroom is unplayable. There is no retail game on this machine:
  captures come later from the user's Wine machine (`docs/capture-protocol.md`, Wave C tuning).
- Never commit game data or screenshots. Never screenshot the desktop.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Work in a git worktree on a branch named like `phase3-<topic>`. Commit there; don't push (origin/master is
  far behind local master) and don't merge unless the user asks.
- After a merge, keep the README "Playtesting" section current. Update the Claude Docs plan doc: project
  492dcab4-5bde-447e-8363-9a304bff2d8d, node 5ae9cbb6-c580 (status chip, a new log row, the bug list, the byline date).

## Read first (only what you need)
- `NOTES.md` sections: "Retail Human* state graph (Phase 3)", "Human* port, B2a" through "Human state
  machine part 5", "Phase 3 leftovers", "Playability pass", "Falling off an edge, targeted-jump ballistics"
  (the newest, and the template for how a port is written up).
- `~/.cache/acbmp-re/findings/human-states.md`: retail transition table with values, sources, our equivalent
  and status (matched / differs / missing). Grep it for `missing` and `partial`; don't read it whole.
  Other findings: `jump.md`, `freerun.md`, `loco.md`, `stepoff.md`, `states.md`.
- `HANDOFF.md` in the main checkout (untracked), section "Paths": data locations and how to start Ghidra.

## Environment
- Data: `ACBMP_SCHEMAS=~/Music/anvilforge-py/src/anvilforge/schemas ACBMP_MULTI=~/Music/ACB/multi
  ACBMP_EXE=~/Music/ACB/ACBMP.exe`. The map tests (`crates/acb-game/tests/*_map.rs`) skip silently without
  them, so set them for `cargo test`.
- RE: the Mac build `~/Music/acbmp_sf.exe` has full symbols, and its addresses are the Ghidra addresses.
  Start the headless GhidraMCP on :8090 (HANDOFF.md "Paths"), then load `/acbmp_sf.exe`. The user can't run
  Ghidra; you manage it, and stop it when done.
- Use `tools/ghid.py` for everything RE (`python3 tools/ghid.py`; elsewhere in tools/ run Python with `-P`, since
  `tools/dis.py` shadows the stdlib `dis` on Python 3.14):
  - `sym REGEX` finds a symbol;
  - `dec ADDR|SYMBOL` decompiles into the cache `~/.cache/acbmp-re/dec/<prog>/<addr>.c` and prints only the
    path;
  - `cached` lists what's already decompiled (check it first);
  - `note TOPIC TEXT` records a finding.
  Read decompiles with grep and `sed -n` ranges, never whole. Disassembly helpers: `tools/dis.py`,
  `tools/rd.py`. To find who writes a struct field, scan objdump output for the offset.
- Token hygiene: redirect big output to files under your job tmp dir and print at most about 150 lines; filter
  cargo output. Subagents only when asked, at most 2 at a time.

## Code map
- `crates/acb-game/src/human/` holds the retail decisions: pure functions and constants with their retail
  addresses (`ground.rs`, `in_air.rs`, `jump.rs`, `haystack.rs`). Motion lives elsewhere:
  - `player.rs`: the per-tick drive, which asks `human::` and applies the answer;
  - `jump.rs`: freerun jumps, `Flight`, `fall_off`;
  - `climb.rs`: hang, climb, ejects;
  - `locomotion.rs`: move cycle and anim speed (= retail HumanGround +0x904);
  - `ground.rs`: collision queries.
- Tests: unit tests next to the code; map scenarios in `crates/acb-game/tests/jump_map.rs`,
  `climb_map.rs`, `loco_map.rs` and the other `*_map.rs` files.
- Probes: `crates/acb-game/examples/parkour_sweep.rs` (493 San Donato walls), `probe.rs`, `tickbench.rs` (same dir).

## Regression (after every change, in this order)
1. `cargo build --release`: the scripts run `./target/release` binaries and don't build.
2. `cargo test --release` with the data env vars set.
3. `tools/gates.sh`: 11 runs at 60 and 144 fps. The expected final positions are in the script header (for
   example parapet -10.09 -0.36 6.59, sidejump speed 1.57). A changed gate needs a reason in NOTES, plus an
   updated header.
4. `tools/kills.sh`: 8 granted.
5. `tools/duel.sh`: a 100, b 0, 0 desync.
6. `tools/escort.sh`: 0 desync and 0 match desync on all clients, "match over". The team scores depend on
   netsim timing and vary run to run ([270, 130], [270, 110] and [290, 30] seen). It binds port 7951, so never run
   two at once.
   Sim cost matters: clients replay falling ticks, and a slow query breaks escort (see "Phase 3 leftovers",
   pilotis).

## Where things stand (master after phase3-fall-off-edge, 2026-11-03)
Ported:
- in-air check order, grasp gate and landing classes;
- edge fall, step down, edge stop and pull-down;
- fall-off clearance box;
- haystack contact and air entry;
- obstacle collision and turn on wall;
- StepOff on no-stand materials;
- pilotis and beam catch;
- ForwardFromIdle;
- retail FallOffEdge: a targeted jump to 1 + 2·anim speed m out (high profile) or 1 m (low profile), 3 m
  down, within a 60° cone, with a backward fall at 120° or more;
- retail targeted-jump ballistics (`human::in_air::home`: `GetDropTime` + `GetTargettingAcceleration`).
- retail hang rebound (back eject): leap first, stick cones, best target or the farthest foothold 8 m out
  (`human::ledge`).
Retail has two targeted-jump paths:
- clip warp onto the target: ground jumps, approximated by our `jump::Flight`;
- ballistic homing: only when InAirData +0x1fa is set, for ledge, ladder and leap-of-faith releases.

## Next targets (in suggested order)
1. ~~Hang eject~~: done in phase3-ledge-release (NOTES "Hang rebound"). The back eject is the retail
   rebound, a targeted jump (not ballistic). `ReleaseToTarget` 00d8ca6e is not on the player path (see
   NOTES "Phase 3 review"): a player release is a plain drop or a leap of faith.
2. ~~`Flight` warp~~: done (NOTES "Flight warp"). The end error is linear in time over the clips. Moving footholds
   (the per-tick re-read correction) are not modelled, since San Donato has none.
3. ~~Brain edge commands~~: done (NOTES "Low-profile edge brake"). Players never get UnbalanceOnEdge, which is NPC-only;
   low profile brakes hard (SetBreakHard) at drops over 2.5 m. Open: the LookDown pose, and the MoveStop clip on
   the brake.
4. ~~NarrowObject FreeRun~~ and 5. ~~smaller items~~: reviewed (NOTES "Phase 3 review"). Jumps off beams already
   work; the release never targets for players; the pilotis gap gate, slopes (rare on San Donato) and ladders
   are documented, not ported. Left: retail clips (beam entry/landing, bump, edge stop/look-down), visuals, parked.
6. **Wave C**: tune against retail traces once the user's captures arrive (`tools/tracecmp.py`,
   `crates/acb-trace`).

For each port:
- add a NOTES section `## <Topic> (<date>, <branch>)` covering retail functions and addresses, the values,
  what's ours, tests, gate changes and the next step;
- add a `ghid.py note`;
- add a unit test, plus a map test when it shows on San Donato;
- update the README Playtesting text if controls or feel change.
