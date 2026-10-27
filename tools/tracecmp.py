#!/usr/bin/env -S python3 -I
"""Compare two movement traces (retail `acb-trace record` vs ours `ACBMP_TRACE=csv:`).

Columns: findings/trace-format.md (tick,t,x,y,z,yaw_deg,vx,vy,vz,hspeed,state,retail_state,anim,anim_t).
Both traces are aligned on movement onset (first sample 5 cm from the start position) and
compared relative to their own start, so a retail start a few cm off the spot doesn't count.

  tools/tracecmp.py A.csv B.csv [--pos-tol 0.10] [--tick-tol 2]
Exit status 1 when the state sequence differs or a tolerance is exceeded.
"""
import argparse, csv, math, sys

ONSET_M = 0.05


def load(path):
    rows = []
    with open(path, newline='') as f:
        for r in csv.DictReader(f):
            rows.append({
                't': float(r['t']), 'x': float(r['x']), 'y': float(r['y']), 'z': float(r['z']),
                'yaw': float(r['yaw_deg']), 'h': float(r['hspeed'] or 0),
                'st': r.get('retail_state') or r.get('state') or '?',
            })
    if not rows:
        sys.exit(f'{path}: no samples')
    s = rows[0]
    onset = next((r['t'] for r in rows
                  if math.dist((r['x'], r['y'], r['z']), (s['x'], s['y'], s['z'])) > ONSET_M), s['t'])
    for r in rows:
        r['t'] -= onset
        r['dx'], r['dy'], r['dz'] = r['x'] - s['x'], r['y'] - s['y'], r['z'] - s['z']
    return rows


def segments(rows):
    """Collapse to (state, t_start, t_end, first_row, last_row)."""
    out = []
    for r in rows:
        if out and out[-1][0] == r['st']:
            out[-1][2], out[-1][4] = r['t'], r
        else:
            out.append([r['st'], r['t'], r['t'], r, r])
    return out


def flights(segs):
    res = []
    for st, t0, t1, a, b in segs:
        if st == 'HumanInAir':
            res.append({'t0': t0, 'dur': t1 - t0,
                        'dist': math.hypot(b['x'] - a['x'], b['y'] - a['y']), 'drop': a['z'] - b['z']})
    return res


def turn_rate(rows):
    best = 0.0
    for a, b in zip(rows, rows[3:]):
        dt = b['t'] - a['t']
        if dt > 0:
            d = (b['yaw'] - a['yaw'] + 180) % 360 - 180
            best = max(best, abs(d) / dt)
    return best


def accel_time(rows):
    """Seconds from onset to 90 % of the top horizontal speed."""
    top = max(r['h'] for r in rows)
    return next((r['t'] for r in rows if r['t'] >= 0 and r['h'] >= 0.9 * top), 0.0) if top > 0 else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('a'); ap.add_argument('b')
    ap.add_argument('--pos-tol', type=float, default=0.10, help='metres')
    ap.add_argument('--tick-tol', type=int, default=2, help='60 Hz ticks')
    o = ap.parse_args()
    A, B = load(o.a), load(o.b)
    sa, sb = segments(A), segments(B)
    tick = 1 / 60
    fails = []

    def line(name, va, vb, tol=None, unit=''):
        d = vb - va
        bad = tol is not None and abs(d) > tol
        if bad:
            fails.append(name)
        print(f'  {name:<22}{va:>9.3f}{vb:>9.3f}{d:>+9.3f} {unit:<4}{"FAIL" if bad else ""}')

    seq_a, seq_b = [s[0] for s in sa], [s[0] for s in sb]
    print(f'A {o.a}\nB {o.b}')
    print('states A:', ' > '.join(seq_a))
    print('states B:', ' > '.join(seq_b))
    if seq_a != seq_b:
        fails.append('state sequence')
        print('  state sequence differs')
    print(f'  {"metric":<22}{"A":>9}{"B":>9}{"B-A":>9}')
    for i, (x, y) in enumerate(zip(sa[1:], sb[1:])):
        if x[0] == y[0]:
            line(f'enter {x[0][5:] or x[0]}#{i + 1}', x[1], y[1], o.tick_tol * tick, 's')
    for i, (fa, fb) in enumerate(zip(flights(sa), flights(sb))):
        line(f'flight{i} time', fa['dur'], fb['dur'], o.tick_tol * tick, 's')
        line(f'flight{i} distance', fa['dist'], fb['dist'], o.pos_tol, 'm')
        line(f'flight{i} drop', fa['drop'], fb['drop'], o.pos_tol, 'm')
    ea, eb = A[-1], B[-1]
    for k in ('dx', 'dy', 'dz'):
        line(f'end {k}', ea[k], eb[k], o.pos_tol, 'm')
    line('top speed', max(r['h'] for r in A), max(r['h'] for r in B), unit='m/s')
    line('time to 90% speed', accel_time(A), accel_time(B), o.tick_tol * tick * 3, 's')
    line('peak turn rate', turn_rate(A), turn_rate(B), unit='°/s')
    print('RESULT', 'PASS' if not fails else 'FAIL: ' + ', '.join(fails))
    return 1 if fails else 0


if __name__ == '__main__':
    sys.exit(main())
