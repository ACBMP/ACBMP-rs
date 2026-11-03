# acbmp-rs

Clean-room Rust reimplementation of Assassin's Creed Brotherhood multiplayer.
Reads the player's own retail install at runtime; ships no game data.

## Playtesting

Kept current with master: if a control or flag changes, this section changes
in the same commit.

Setup (once per shell):

    cargo build --release
    export ACBMP_SCHEMAS=~/Music/anvilforge-py/src/anvilforge/schemas
    export ACBMP_EXE=~/Music/ACB/ACBMP.exe
    M=~/Music/ACB/multi

Single player on San Donato (free play, crowd of 20 lookalike NPCs to kill):

    ./target/release/acb-viewer $M/DataPC_AC2MP_SanDonato.forge \
        --character AC2MP_ID07_MainTemplate_Player --play --crowd 20

Two-player duel (Wanted rules: contracts, kills score, respawns, 5 min):

    ./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --players 2
    # then on each player's machine (or twice on one), host = the server's IP:
    ./target/release/acb-viewer $M/DataPC_AC2MP_SanDonato.forge \
        --character AC2MP_ID07_MainTemplate_Player --play \
        --connect HOST:7777 --name alice

The server waits for `--players` clients, then starts. `--time-limit S`
changes the match length, `--mode free` drops the match rules, `--crowd N`
(and `--crowd-seed S`) adds N wandering NPCs that every client simulates
too (blending works online; clients need no flag, the server sends it).

Escort (teamvip, 2v2 up to 4v4, two rounds with the attacking team swapped):

    ./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --players 4 --mode escort
    # clients exactly as for the duel (four of them)

Slots alternate teams (0, 1, 0, 1; `--teams 0,0,1,1` to choose), team 0 hunts
first (`--attacking 1`). Every round starts with all players on their team's
spawn group, then two VIPs walk their ways: hunters kill them (100 each),
the protecting team scores checkpoints and way ends. A VIP that reaches its
way's end escapes: the round's play ends at once and its protectors win it
(`VIP escaped: team T wins round R`; our rule, retail unconfirmed). Killed
players respawn by the retail VIP rule: protectors on the free spawn nearest
their VIPs and teammates, hunters at least 35 m from VIPs and protectors (near
their teammates). A client whose VIP state drifted gets the server's VIPs
re-created (`net: VIP re-created` in its log). `--time-limit S` is each
round's play (default 300 s, a placeholder), `--intro S` / `--recap S` the 8 s
phases around it. The server prints the team totals at the end. To play
locally, start the server with `--players 2` (or 4) and run that many
viewers with `--connect 127.0.0.1:7777` (`--port` sets another) on the same
machine. The Escort HUD is drawn as text across the top of the screen (also
the window title and the `hud tick` lines of `ACBMP_TRACE=1` clients), e.g. `team 0 hunter | VIP 12.3 m | [team 0 100]
team 1 10 | round 1/2 play 0:35 | +100 points`: your team and role (hunter or
protector), the distance to the nearest VIP (targetable ones when hunting,
living ones when protecting; `VIPs in N s` while they respawn), team totals
(yours bracketed), round, phase (intro/play/recap/over) with its clock, and
the last message. Living VIPs are drawn gold-tinted and get a gold
diamond with their distance (`12M`) over their head, drawn over the scene so
it shows through walls: hunters see the targetable VIPs, protectors every
living one. Off screen or behind the camera the marker sticks to the screen
edge with a wedge pointing towards the VIP (behind: lower half). It shrinks
with distance (retail scale 1.0 at 5 m to 0.6 at 30 m). Markers at the same
edge spot overlap. `tools/escort.sh` runs a 2v2 on one machine with 5 % loss and
150 ms round trip; its viewers use the test input `--input "0=vip"` (while
your team hunts, run at the nearest VIP pressing kill).

Assassinate (free-for-all, 10 min, any player count):

    ./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --players 4 --mode assassinate
    # clients exactly as for the duel

Nobody hands out contracts: you acquire one by keeping a player in the
view (within 60 degrees of the camera, 9 m, 1.6 m up or down, in line of
sight) for 3 s, or at once if that player is running (High Profile). The player you are
acquiring is warned and may stun you meanwhile (200 points, you start over). R locks the highlighted
player (it stays your candidate behind walls until it dies or is 30 m away;
R again unlocks); Y releases your contract (not during a kill or stun) and
that player cannot be acquired again for 5 s.
Your contract lasts while you keep looking; look away for 2 s and it is lost
(look back within those 2 s and it is still yours, also after switching to
someone else). A kill is only valid on your contract (100 points plus
bonuses); killing a crowd NPC loses your contract. Kill bonuses (Assassinate
only; the duel and Escort keep 100 + acrobatic/hidden): FOCUS +150 (you
stayed within 10 m of the victim, your nearest player, for 3 s), then one of
INCOGNITO +300 (the victim's threat meter never picked you up), SILENT +200
(no running in the last 3 s) or UNSPOTTED +50 (not in a chase), FIRST BLOOD
+50 (first kill of the match), REVENGE +50 (you kill the player who killed
you last, before anyone else does). The message line shows `bonus: FOCUS,
INCOGNITO, FIRST BLOOD`. A kill that closes a chain of 3 kills within 10 s
(you killed B, who had killed C, who had killed D) announces BLOODBATH to
everyone (no points). HUD text: `acquiring pN 45%`, `being acquired by pN`,
and `compass pA 12 m, pB 30 m` (every other living player, nearest first).
On screen under the text: a compass strip (straight ahead in the middle,
left/right at the quarter ticks, behind at the ends, turning with the camera)
with a white bar per potential target, a gold triangle on the player you are
acquiring and a red diamond on your acquired target; a gold progress bar
while you acquire; and a blinking red `BEING ACQUIRED!` box in the lower
third while anyone acquires you. `tools/assassinate.sh` runs a 1v1 on one
machine, `tools/assassinate4.sh` four clients at 5% loss and 150 ms (both
port 7952, never both at once). Known gaps: the look cone and range are our
retail's targeting values, but that the highlight is that pick is our
reading; the lock break range (30 m) and the 5 s re-acquire cooldown after a
release are placeholders; the focus range, the Silent window and what
Incognito means are our readings; the informer NPC is missing; the HUD widgets are our
own art, not retail's compass; the start delay (3 s) and respawn delay (5 s)
are the duel's placeholders.

Controls (on foot):

| Key | Action |
|---|---|
| W A S D | move, relative to the camera |
| (nothing) | low profile: walk; stops at ledges deeper than 2 m and at beam ends |
| Shift | high profile: run; runs into wall runs and climbs |
| Space | Legs: sprint (high profile). Freerun jumps (up onto a ledge, across a gap, to a hang across a gap) need Space held or pressed in the last 0.5 s and the move direction within 80° of facing (retail); there is no jump button: Space alone does nothing, but Shift + Space + a direction toward a ledge jumps even from standing (retail ForwardFromIdle). Running off an edge with nothing in reach to land on jumps out into the open the retail way: 1 m out in low profile, up to 3 m out in high profile (more the faster you go), reaching that 3 m below the edge, then straight down. Starting off in any direction from standing turns and moves at once (0.2 s turn in high profile, 0.4 s walking) |
| F | jump: at a ledge stop (walking into it or standing there), lower onto the wall below; with no wall to lower onto (a beam end, a railing, a ledge without a hold) step off and drop |
| E | kill (your target online; any NPC in free play); walks in from ~4 m |
| Q | stun (your pursuer online) |
| R | Assassinate: lock / unlock the highlighted player |
| Y | Assassinate: release your contract |
| right mouse (hold) | orbit the camera |
| mouse wheel | camera distance |
| Esc | quit |

On a wall: W/S/A/D climb up/down/left/right (fixed directions, not camera
relative); F + S or F alone jumps back off the wall, F + A/D jumps sideways
across a gap, F + W leaps to a hold above; Shift + A/D at the end of an edge
also side-jumps. W at the top edge pulls up over it onto the floor behind (also onto narrow tops: wall tops, posts, centred) when there is room to stand there. A fall grabs a hold up to 1.2 m ahead only in its first
2 m (retail); after that you go down. Drops under 0.53 m are stepped down,
deeper ones fallen off, unless something blocks the space just past the
edge (a railing, a wall, a low overhang: you stay on the edge). A short
fall (its first 2 m) next to a railing or beam lands on it, in the beam
stance; next to a post top (pilotis) you land on its centre (try a drop at
`--at=-41.81,10.30,7.5,-90`). Walk or run into a haystack (touch it, facing it and pushing
towards it) to hide; in the air you go in when you touch it or when the
1.8 m from your feet up crosses it (dropping or jumping onto or past
it). Any move key leaves. Falling onto a surface whose collision material
forbids standing pushes you off it (retail StepOff; on San Donato only one
small cap, near 14.9 22.1 3.7). Running
into a wall you can't climb stops you (retail obstacle collision); leaping
face first into a wall turns your back to it and you slide down it. Bushes count as haystacks too (one is right
behind the first San Donato spawn): sunk into one with the legs out of sight
means hidden, not stuck; any move key hops out. Walking slowly among the crowd blends you. Stand
or walk (low profile) next to a bench seat and press F to sit (blended);
E kills a passer-by from the bench, a fresh move key stands up.

HUD: drawn as text at the top of the screen (and in the window title):
target, distance, compass fill, who is hunting you, score, time and the last
event. In Escort, hunters also get a gold arrow in a ring under the text
pointing to the nearest targetable VIP relative to the camera (up = ahead).

Useful extras: `--at x,y,z,yaw` starts the player somewhere else (see
`tools/gates.sh` for spots next to the climbs and jumps), `ACBMP_TRACE=1`
prints every tick's player state to stderr, and `ACBMP_TRACE=csv:run.csv`
writes it as CSV (one row per 60 Hz tick, same columns as the retail captures).
`acbmp ways <map.forge> [--hunters 1..4] [--points]` lists the generated
Escort VIP ways (length, checkpoints, distance of the start from the hunter
team spawns). The VIP cycle (two crowd walkers on those ways, respawning 5 s
after both die and targetable 10 s later) also runs offline in the game
library's tests
(`ACBMP_TRACE=1 cargo test --release -p acb-game --test escort_map -- --nocapture`
prints the `vip:` walk/kill/respawn trace and `escort:` team scores).

### Retail captures (kit)

`tools/make-capture-kit.sh` builds `dist/capture-kit.tar.gz`: a static
`acb-trace`, its `offsets.toml`, `capture.sh` and a README. On the machine that
runs retail ACBMP.exe under Wine: allow ptrace (`sudo sysctl
kernel.yama.ptrace_scope=0` or `sudo setcap cap_sys_ptrace+ep acb-trace`), get
into a level, run `./acb-trace check` and send back `diag.txt` (the memory
offsets are not confirmed on a live game yet). Then walk to a start spot with
`./acb-trace where --to x,y,z,yaw` and record with `./capture.sh MOVE...` (or
`./acb-trace record --out FILE`). The animation columns are still empty.

The moves, start spots and timings to record are in `docs/capture-protocol.md`.
To compare: `tools/trace-ours.sh OUT [move...] [--fps N]` replays those moves here
and writes `OUT/<move>.csv`, then `tools/tracecmp.py retail/<move>-1.csv OUT/<move>.csv`
prints the state sequence, flights, end offset, speed and turn rate side by side
(exit 1 past 10 cm or 2 ticks).

### Giving feedback

What helps most: what you did (keys), what you expected (retail behaviour),
what happened, and where. For "where", run with `ACBMP_TRACE=1` and quote the
last `player at x y z` line, or the `--at` you started from. Scripted repros
are best: `--input "0=w,shift 1.5= 2=d"` (keys from each time in seconds) plus
`--at` replays exactly, at any frame rate.

Known gaps (Phase 2): the HUD is plain text (no retail widgets or compass art); respawn (5 s) and start (3 s) delays
are placeholders; blending uses retail's group polygons but our crowd forms
its groups by distance (retail groups not traced); bench kill placement is
our stand-in (clips lack a Reference track), ledge kills and beam-end edge animations are missing; chase start uses
the retail threat meter (rates, in-air rates, awareness with distance ramp, facing cones and speed ramp; HUD
"threat NN%"), but which vectors the facing cones compare is our reading and there is no target lock-on, so
the retail lock-on 0.99 (KillerDiscoveredEvent) is missing; walkable slope and step height are still ours (retail values not
found).

Crates:
- `anvil-forge`: `.forge` archive index, `.data` containers, LZO2A/LZO1X blocks
- `acbmp-cli` (`acbmp`): `ls`, `census`, `digest` over a forge

Payload decoding is verified byte-identical against anvilforge-py on
`DataPC.forge` and `DataPC_AC2MP_SanDonato.forge` (`acbmp digest`, with
`ACBMP_DUMP=<file>` to write the decoded stream for external comparison).
- `anvil-schema`: SCHM reflection schema loader (ACB with placeholders filled from ACR)
- `anvil-fastload`: schema-driven FastLoad object decoder

`acbmp fastload <forge> --schemas <dir>` (or `ACBMP_SCHEMAS`) decodes every
sub-object. Per-type decoded/failed counts match anvilforge-py's `fastload.py`
exactly on DataPC.forge (19,526 decoded, 489,403 incl. nested) and San Donato
(6,319 / 120,861), about 35-50x faster. Animation, FX and MaterialTemplate use
hand-written serializers and stay opaque for now.

The `.schema` files come from anvilforge-py and are not vendored here.
- `acb-world`: uid index over a forge, entity instances (Visual -> LOD0 mesh),
  static meshes (vertex formats 3/4), textures (BGRA8/BC1-3 mip chains),
  material parameters (CRC32 property ids, TextureSelector resolution) and
  compiled template permutations

`acbmp gltf <forge> <out.glb>` exports a map's static geometry. San Donato:
1,971 instances of 280 meshes, 317k triangles, in 0.6 s; layout checked by
rendering the GLB in Blender.
- `acb-render`: wgpu renderer (instanced static meshes, native BC textures,
  opaque / alpha-test / blended passes). Materials are drawn with the game's
  own translated vertex + lighting pixel shaders, fed the material's
  parameters and textures from its template; anything that doesn't
  translate falls back to a placeholder sun + fog shader (`--placeholder`
  forces it). San Donato: all 202 material/format pairs on 29 programs.
  `ACBMP_DEBUG_MATERIALS=1` prints each material's resolved parameters.

`acbmp shot <map.forge> out.png --eye x,y,z --target x,y,z` renders a frame
offscreen (the map is mounted over the core `DataPC.forge` next to it).
`acbmp meshes` and `acbmp dump <forge> <name>` help inspect content.
`acbmp loc DataPC.forge [0x<id> | text]` prints the English game text as
`id<TAB>text` (`UIString.OasisLineID` is the id; `--package` picks another language).
- `acb-viewer`: interactive fly-through (WASD, Space/C, right mouse to look,
  Shift faster, wheel sets speed). `acb-viewer <map.forge> --schemas <dir>`.
  San Donato: 1.2M triangles at a vsynced 60 fps on an RX 580.
  With `--character`/`--play` it is a client of `acb-game`: it runs the
  due 60 Hz ticks each frame and draws characters interpolated between
  the last two (`--fps N` simulates a render rate in `--capture` runs).
- `acb-game`: the gameplay simulation, no renderer: map collision and
  freerun guidance (`ground.rs`), player control (`player.rs`), locomotion,
  freerun jumps (`jump.rs`), climbing, kills, crowd. `World::step(&[PlayerInput])`
  advances one fixed 60 Hz tick; bodies expose placement and pose for the
  last two ticks.
- `d3d9-shader`: SM3 bytecode parser (incl. CTAB constant tables),
  disassembler and WGSL translator. `acbmp shaderlab <forge>` translates every
  MaterialTemplate permutation and validates it with naga: 100% of the unique
  shaders in DataPC.forge (5,796) and every MP map forge pass.
- `acb-anim`: animation track stream (54 track types: 18 compressions x 3
  key-time formats), all quaternion/vector/float decompressors.
  `acbmp animstats <forge>`: every one of the core forge's 15,254 animations
  decodes to its exact length; keys are at 60 frames per second of `Length`;
  91% of TrackIDs are skeleton BoneIDs; decoded keys are continuous
  (median step 0.8-7.7 degrees for all but Quat16).
  `acbmp animplay <forge> <name> out.png` samples a clip on its best-matching
  skeleton and draws stick figures (verified: a 2.2 s air assassination
  plays correctly on UCMA_Altair).
