#!/usr/bin/env bash
# usage: tools/assassinate4.sh [NETSIM]   (default loss=5%,delay=150ms,jitter=5ms)
# Assassinate A5 gate: acbmp serve + four headless viewers on port 7952 (A 60 fps, B 144,
# C and D 60 on standard spawns, idle). A presses kill at 1.5 s (tick 90), before any
# acquisition: the server refuses it (KillRejected "not your target") and no client shows a
# kill. B stands in A's view; both acquire each other, then A kills B ~tick 400-430: the kill
# scores once, with the same points on the server and every client, and no client has a
# desync. Prints PASS or FAIL lines. Run from the repo root after cargo build --release;
# never at the same time as tools/assassinate.sh (same port).
export ACBMP_SCHEMAS=~/Music/anvilforge-py/src/anvilforge/schemas ACBMP_MULTI=~/Music/ACB/multi ACBMP_EXE=~/Music/ACB/ACBMP.exe
M=~/Music/ACB/multi; O=${O:-$(mktemp -d)}; mkdir -p "$O"; echo "logs: $O"
NET=${1:-loss=5%,delay=150ms,jitter=5ms}
./target/release/acbmp serve $M/DataPC_AC2MP_SanDonato.forge --port 7952 --mode assassinate --players 4 --at=31,20.4,0.02,-90 --at=41,20.4,0.02,90 --time-limit 20 > "$O/serve.txt" 2>&1 &
sleep 0.5
v() { WAYLAND_DISPLAY=wayland-1 XDG_RUNTIME_DIR=/run/user/1000 ACBMP_TRACE=1 timeout 240 ./target/release/acb-viewer $M/DataPC_AC2MP_SanDonato.forge --connect 127.0.0.1:7952 --name $1 --netsim $NET --character AC2MP_ID07_MainTemplate_Player --play --exe $ACBMP_EXE --input "$2" --fps $3 --capture "$O/$1.png" --capture-after 60 > "$O/$1.txt" 2>&1; }
# slots follow join order (a=0, b=1, c=2, d=3): start each client after the previous one joined
joined() { for _ in $(seq 600); do grep -q "joined as $1 " "$O/serve.txt" && return; sleep 0.1; done; echo "FAIL: $1 never joined"; }
v a "1.5=e 1.8= 2=w 3= 4=w,shift 6.2=w 6.8=e 7.2=" 60 & joined a
v b "" 144 & joined b
v c "" 60 & joined c
v d "" 60 &
wait
grep -E 'serve: (result|sent|[abcd]:)' "$O/serve.txt"
res=$(grep -oE 'result after [0-9.]+ s: .*' "$O/serve.txt" | head -1)
# result line "a N, b 0, c 0, d 0" in any order
result_ok() { echo "$1" | grep -qE "[:,] a $2[,( ]" || return 1; for q in b c d; do echo "$1" | grep -qE "[:,] $q 0[,( ]" || return 1; done; }
ok=1
pts=$(grep -oE 'player 0 scores [0-9]+' "$O/a.txt" | head -1)
tick_of() { grep -oE "event tick [0-9]+ $1" "$O/a.txt" | head -1 | awk '{print $3}'; }
for n in a b c d; do
  f=$O/$n.txt
  echo "== $n: $(grep -E '^net: tick .*snapshots' "$f" | grep -oE '[0-9]+ desync, [0-9]+ of [0-9]+ match desync.*rejected' | head -1)"
  grep -E '^net: event' "$f" | grep -E 'rejected|kills body|scores|bonus|BLOODBATH' | sort -u | head -12
  [ "$(grep -E '^net: event.*kills body' "$f" | sort -u | wc -l)" = 1 ] || { echo "FAIL $n: not exactly one kill"; ok=0; }
  [ "$(grep -E '^net: event.*player 0 scores' "$f" | sort -u | wc -l)" = 1 ] || { echo "FAIL $n: player 0 did not score exactly once"; ok=0; }
  [ "$(grep -oE 'player 0 scores [0-9]+' "$f" | head -1)" = "$pts" ] || { echo "FAIL $n: points differ from a ($pts)"; ok=0; }
  grep -qE ' 0 desync, 0 of [0-9]+ match desync' "$f" || { echo "FAIL $n: desync"; ok=0; }
  rl=$(grep -E '^net: match over: result after ' "$f" | head -1)
  result_ok "$rl" "[0-9]+" || { echo "FAIL $n: no result line"; ok=0; }
done
# the rejection is resent until acked, so a client that pressed always logs it
grep -qE 'event tick [0-9]+ player 0 kill rejected: not your target' "$O/a.txt" || { echo "FAIL a: no pre-acquisition rejection"; ok=0; }
k=$(tick_of 'player 0 kills body 1'); r=$(tick_of 'player 0 now hunts player 1')
{ [ -n "$k" ] && [ -n "$r" ] && [ "$k" -gt "$r" ]; } || { echo "FAIL: kill (tick $k) not after the acquisition (tick $r)"; ok=0; }
[ "${res#*: }" != "${res}" ] && echo "server $res; a: $pts"
result_ok "$res" "${pts##* }" || { echo "FAIL: server result differs from a's score"; ok=0; }
[ $ok = 1 ] && echo PASS || echo FAIL
