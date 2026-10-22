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

Controls (on foot):

| Key | Action |
|---|---|
| W A S D | move, relative to the camera |
| (nothing) | low profile: walk; stops at ledges deeper than 2 m and at beam ends |
| Shift | high profile: run; runs into wall runs, freerun jumps and climbs |
| Space | sprint (high profile) |
| F | jump: at a ledge stop, lower onto the wall below |
| E | kill (your target online; any NPC in free play); walks in from ~4 m |
| Q | stun (your pursuer online) |
| right mouse (hold) | orbit the camera |
| mouse wheel | camera distance |
| Esc | quit |

On a wall: W/S/A/D climb up/down/left/right (fixed directions, not camera
relative); F + S or F alone jumps back off the wall, F + A/D jumps sideways
across a gap, F + W leaps to a hold above; Shift + A/D at the end of an edge
also side-jumps. Walk into a haystack in high profile (or drop onto it) to
hide; any move key leaves. Walking slowly among the crowd blends you. Stand
or walk (low profile) next to a bench seat and press F to sit (blended);
E kills a passer-by from the bench, a fresh move key stands up.

HUD: there is no on-screen text yet. Target, distance, compass fill, who is
hunting you, score, time and the last event are in the window title.

Useful extras: `--at x,y,z,yaw` starts the player somewhere else (see
`tools/gates.sh` for spots next to the climbs and jumps), `ACBMP_TRACE=1`
prints every tick's player state to stderr.

### Giving feedback

What helps most: what you did (keys), what you expected (retail behaviour),
what happened, and where. For "where", run with `ACBMP_TRACE=1` and quote the
last `player at x y z` line, or the `--at` you started from. Scripted repros
are best: `--input "0=w,shift 1.5= 2=d"` (keys from each time in seconds) plus
`--at` replays exactly, at any frame rate.

Known gaps (Phase 2): no on-screen HUD; respawn (5 s) and start (3 s) delays
are placeholders; blending uses retail's group polygons but our crowd forms
its groups by distance (retail groups not traced); bench kill placement is
our stand-in (clips lack a Reference track), ledge kills and beam-end edge animations are missing; chase start is
our reading; walkable slope and step height are still ours (retail values not
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
