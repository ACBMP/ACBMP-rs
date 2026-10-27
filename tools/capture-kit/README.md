# acbmp-rs retail capture kit

Records how the retail game moves (position, facing, Human* state at 60 Hz) so the
acbmp-rs rewrite can be compared against it. Runs on the Linux machine where retail
ACBMP.exe (PC) runs under Wine. Nothing is sent anywhere; you send the files back by hand.

Contents: `acb-trace` (static Linux x86_64 binary), `offsets.toml` (memory offsets, editable,
read from next to the binary), `capture.sh`, this README.

## 1. Setup (once)
acb-trace reads the game's memory through /proc/<pid>/mem, which needs ptrace permission.
Either (until the next reboot):

    sudo sysctl kernel.yama.ptrace_scope=0

or give only this binary the capability (keeps the system setting):

    sudo setcap cap_sys_ptrace+ep ./acb-trace

Run acb-trace as the same user as the game.

## 2. Check first, and send diag.txt back
Start the game, get into a level (in control of your character, not in a menu), then:

    ./acb-trace check

It finds the ACBMP.exe process, follows the pointer chain to your player, samples for one
second and writes `diag.txt` (ends with `RESULT: OK` or `RESULT: FAILED`). Several offsets are
derived statically and found by scanning at attach time, so **send diag.txt back even when it
says OK**: the offsets it found get pinned in offsets.toml. If it fails, a new offsets.toml may
be all that's needed (no rebuild). Exit codes: 1 no game process, 3 no permission (see 1),
4 chain not found.

## 3. Walk to a start spot
Retail can't teleport, so walk to each move's start spot with:

    ./acb-trace where --to X,Y,Z,YAW

It prints position/yaw (in the acbmp-rs world frame: metres, Z up, yaw in degrees) four times a
second, plus the horizontal distance, height difference, the turn needed to face the spot and
the yaw error to YAW. Ctrl-C to quit.

## 4. Record
Either by hand:

    ./acb-trace record --out wallrun-1.csv [--duration 10]

(stops on Ctrl-C, `kill -USR1 <pid>`, or after --duration seconds), or with the helper, which
asks for each move in turn and saves `captures/<move>-<n>.csv`:

    ./capture.sh climb-up ledge-jump haystack-dive

Move names come from the capture protocol you were given; repeat a name to take it again.
Send back the `captures/` folder.

CSV columns: tick,t,x,y,z,yaw_deg,vx,vy,vz,hspeed,state,retail_state,anim,anim_t
(anim/anim_t are empty for now).
