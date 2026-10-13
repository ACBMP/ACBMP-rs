# acbmp-rs

Clean-room Rust reimplementation of Assassin's Creed Brotherhood multiplayer.
Reads the player's own retail install at runtime; ships no game data.

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
  static meshes (vertex formats 3/4), textures (BGRA8/BC1-3 mip chains)

`acbmp gltf <forge> <out.glb>` exports a map's static geometry. San Donato:
1,971 instances of 280 meshes, 317k triangles, in 0.6 s; layout checked by
rendering the GLB in Blender.
- `acb-render`: wgpu renderer (instanced static meshes, native BC textures,
  opaque / alpha-test / blended passes, placeholder sun + fog shading)

`acbmp shot <map.forge> out.png --eye x,y,z --target x,y,z` renders a frame
offscreen (the map is mounted over the core `DataPC.forge` next to it).
`acbmp meshes` and `acbmp dump <forge> <name>` help inspect content.
- `acb-viewer`: interactive fly-through (WASD, Space/C, right mouse to look,
  Shift faster, wheel sets speed). `acb-viewer <map.forge> --schemas <dir>`.
  San Donato: 1.2M triangles at a vsynced 60 fps on an RX 580.
- `d3d9-shader`: SM3 bytecode parser (incl. CTAB constant tables),
  disassembler and WGSL translator. `acbmp shaderlab <forge>` translates every
  MaterialTemplate permutation and validates it with naga: 100% of the unique
  shaders in DataPC.forge (5,796) and every MP map forge pass.
