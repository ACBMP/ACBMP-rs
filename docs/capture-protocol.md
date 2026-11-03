# Retail capture protocol (Phase 3, parkour fidelity)

These captures record retail ACB MP movement so we can compare it against acbmp-rs with `tools/tracecmp.py`. They run on the Wine machine with the capture kit (`dist/capture-kit.tar.gz`, built by `tools/make-capture-kit.sh`). The kit README covers installation and ptrace permissions.

## Prepare once
1. Retail ACB MP runs under Wine and you can get into a match on **San Donato**, alone if possible; a second account is fine. Whiteroom is not usable.
2. Pick a persona and use it for every capture. Note its name. Ours is `AC2MP_ID07` (see `--character`).
3. Copy the kit to the Wine machine and unpack it. Run `./acb-trace check` with the game in a match, then send `diag.txt` back **before recording anything**. If an offset is wrong, it gets fixed first.
4. Optional: a 60 fps screen recording of each move, as a fallback and for a visual check.

## Per move
- Walk to the start spot with `./acb-trace where --to X,Y,Z,YAW`. Stand within 0.3 m of it, face the given yaw (within about 5°), and stand still for one second.
- Run `./capture.sh <move>`, do the actions, and stop when the move has settled (about 1 s after the end).
- Do each move **3 times**. The files land as `<move>-<n>.csv`.
- If a take went wrong (missed a key, fell off somewhere else), delete it and redo it.

Actions use retail's names. Bind them however you like:
- **HP**: high profile (run);
- **HP+Legs**: sprint/free run;
- **Hands**: grab;
- **Empty hand / F**: our `f` (eject, leap, drop);
- **Back**: S.

Timings count from the first key press; ±0.1 s is fine.

In our viewer: HP = Shift, Legs = Space (HP+Legs = sprint), Empty hand = F. Freerun jumps need Legs held or pressed within the last 0.5 s (retail), so the replays run on Shift and tap Space just before the jump (`railing`, `parapet`, `beamend`); a held Space would sprint. The exact replays are in `tools/trace-ours.sh`.

| Move | Start X,Y,Z,yaw | Actions | Ends with |
|---|---|---|---|
| `walk` | -18, 44.6, 2.22, -97 | W for 2 s, release | standing still |
| `run` | -18, 44.6, 2.22, -97 | W+HP for 2 s, release | stop animation finished |
| `sprint` | -18, 44.6, 2.22, -97 | W+HP+Legs for 2 s, release | stop finished |
| `turn180` | -18, 44.6, 2.22, -97 | W+HP 1 s, then S+HP 1 s (180° run turn), release | standing |
| `climb` | -18, 44.6, 2.22, -97 | W+HP+Legs 1.5 s; at 2.5 s D (shimmy right) until 8 s; at 8.5 s S for 0.3 s | standing below the wall |
| `railing` | 34.5, -54.38, 0.1, -90 | W+HP+Legs 1.1 s, release | standing on the railing |
| `parapet` | -13, 0, 6, -97 | W+HP+Legs 0.9 s, release | landed |
| `eject` | -18, 44.6, 2.22, -97 | as `climb` to the hang (1.5 s), at 3 s Back+Empty hand for 0.3 s | landed behind |
| `sidejump` | -24.0, 43.3, 3.8, 180 | W+HP+Legs 1.5 s to the hang; at 2.5 s A+HP+Legs for 0.2 s | hanging on the next hold |
| `leap` | 5.21, 14.8, 3, 180 | W+HP+Legs 1.0 s; at 3 s W+Empty hand for 0.2 s | hanging 2.4 m higher |
| `pulldown` | -12.0, 43.99, 6.5, 90 | W (walk) 3 s to the roof edge; then W+Empty hand 0.2 s | hanging on the edge |
| `beamend` | 34.5, -54.38, 0.1, -90 | W+HP+Legs 1.1 s onto the railing; at 2 s D until 5 s | stopped at the railing's end |
| `roll` | 31, 20.4, 9.5, -90 | W (walk) off the 9.5 m edge; keep W held through the landing | damage roll finished |

Where the start Z is above the ground (`roll` at 9.5 m, `parapet` at 6 m), you start on a roof or wall top at that height. Use `where` to find it.

**Fall heights.** If you can, also record `fall5`, `fall8` and `fall12`: walk off ledges about 5, 8 and 12 m high, anywhere on the map. These check the landing type and damage thresholds. Note the start spot that `where` prints.

## Escort (teamvip) observations (optional)
These values only exist in game-mode XML that didn't ship, so we need them from a real match. If you can get an Escort match going on the Wine machine (it needs players on both teams), note:
- the round length, and whether a round also ends when a VIP finishes its path;
- the respawn delay after you die, as hunter and as protector;
- the protector score popups each time a VIP passes a checkpoint (the numbers shown and roughly where on the path);
- a 60 fps video of one round, if possible.
A short `./acb-trace record` while walking next to a VIP from its spawn to its exit gives us its path on that map. The ways aren't in the PC data, so for now we author them ourselves.

## Manhunt observations (optional)
Manhunt's brick list didn't ship either. We play it with the binary's defaults and some guesses (NOTES "Manhunt"). From one Manhunt match, note:
- the respawn delay after a hunter kills you (ours: 5 s);
- whether the prey score for escaping a hunter's chase, and how much (ours: no escape score);
- the score popups while hiding: the first one about 1 s after you hide, then every 5 s, 10 points alone and 20 / 30 / 50 with 1 / 2 / 3 hidden teammates nearby (the defaults, unconfirmed). Note whether blending in a crowd pays like a hiding spot;
- the kill bonus names a hunter gets (Focus, Silent, Incognito, ...), especially whether Incognito appears (ours: never in Manhunt);
- whether a hunter's compass shows every prey or only one best target;
- the banners at a round's end.

## Send back
- All `*.csv` files plus `diag.txt`.
- The persona name and the Wine/Proton version.
- Any take that felt unusual, with a note on what happened.

## On our side
`tools/trace-ours.sh ours` replays each move with the same `--at`/`--input` and `ACBMP_TRACE=csv:<path>`; `tools/tracecmp.py retail/<move>-1.csv ours/<move>.csv` then reports state sequence, flight time, distance, landing position, speed and turn rate, and tick deltas.
