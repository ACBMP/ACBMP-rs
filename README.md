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
