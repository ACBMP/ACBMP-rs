# acbmp-rs

Clean-room Rust reimplementation of Assassin's Creed Brotherhood multiplayer.
Reads the player's own retail install at runtime; ships no game data.

Crates:
- `anvil-forge`: `.forge` archive index, `.data` containers, LZO2A/LZO1X blocks
- `acbmp-cli` (`acbmp`): `ls`, `census`, `digest` over a forge

Payload decoding is verified byte-identical against anvilforge-py on
`DataPC.forge` and `DataPC_AC2MP_SanDonato.forge` (`acbmp digest`, with
`ACBMP_DUMP=<file>` to write the decoded stream for external comparison).
