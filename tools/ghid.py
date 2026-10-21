#!/usr/bin/env -S python3 -I
"""Shared, cached reverse-engineering helper over the Mac symbols and the headless Ghidra.

    tools/ghid.py sym REGEX [-n N]          demangled Mac symbols matching REGEX (first N, default 30, + total)
    tools/ghid.py dec ADDR|SYMBOL [-p PROG] decompile (creates the function if Ghidra lacks it); prints only
                                          the cache path, line count and signature, never the body
    tools/ghid.py cached [REGEX]            functions already decompiled (check before decompiling)
    tools/ghid.py note TOPIC TEXT           append a finding to ~/.cache/acbmp-re/findings/TOPIC.md

Decompiles are cached in ~/.cache/acbmp-re/dec/<program>/<addr>.c with every `func_0x...` call annotated
by its symbol name, so nobody decompiles the same function twice. Read them with grep / sed -n ranges.
Ghidra: headless GhidraMCP on 127.0.0.1:8090 (see HANDOFF.md). Mac addresses = Ghidra addresses.
"""
import argparse, bisect, json, os, re, subprocess, sys, urllib.parse, urllib.request

CACHE = os.path.expanduser('~/.cache/acbmp-re')
SYMS = os.path.join(CACHE, 'syms.txt')
MAC = os.path.expanduser('~/Music/acbmp_sf.exe')
GHIDRA = 'http://127.0.0.1:8090'


def symbols():
    if not os.path.exists(SYMS):
        os.makedirs(CACHE, exist_ok=True)
        nm = subprocess.run(['llvm-nm', '-n', MAC], capture_output=True, text=True, check=True).stdout
        out = subprocess.run(['c++filt', '-_'], input=nm, capture_output=True, text=True, check=True).stdout
        open(SYMS, 'w').write(out)
    addrs, names = [], []
    for line in open(SYMS):
        parts = line.rstrip('\n').split(' ', 2)
        if len(parts) == 3 and parts[0].strip():
            try:
                addrs.append(int(parts[0], 16)); names.append(parts[2])
            except ValueError:
                pass
    return addrs, names


def get(path, **q):
    with urllib.request.urlopen(f'{GHIDRA}/{path}?{urllib.parse.urlencode(q)}', timeout=300) as r:
        return r.read().decode()


def post(path, prog, **body):
    req = urllib.request.Request(f'{GHIDRA}/{path}?program={prog}', json.dumps(body).encode(),
                                 {'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.read().decode()


def short(n):
    n = re.sub(r'scimitar::ac2::|scimitar::|Gear::', '', n)
    return re.sub(r'\(.*\)', '()', n) if len(n) > 90 else n


def cmd_sym(a):
    addrs, names = symbols()
    rx = re.compile(a.regex)
    hits = [(x, n) for x, n in zip(addrs, names) if rx.search(n)]
    for x, n in hits[:a.n]:
        print(f'{x:08x} {n}')
    print(f'-- {len(hits)} matches' + (f' (showing {a.n})' if len(hits) > a.n else ''))


def cmd_dec(a):
    addrs, names = symbols()
    if re.fullmatch(r'(0x)?[0-9a-fA-F]{6,8}', a.target):
        addr = int(a.target, 16)
    else:
        rx = re.compile(a.target)
        hits = [x for x, n in zip(addrs, names) if rx.search(n)]
        if len(hits) != 1:
            sys.exit(f'{len(hits)} symbols match {a.target!r}; use `sym` and pass an address')
        addr = hits[0]
    d = os.path.join(CACHE, 'dec', a.p); os.makedirs(d, exist_ok=True)
    path = os.path.join(d, f'{addr:08x}.c')
    if not os.path.exists(path):
        h = f'0x{addr:08x}'
        r = get('decompile_function', address=h, program=a.p)
        if 'No function' in r[:200]:
            post('create_function', a.p, address=h, program=a.p)
            r = get('decompile_function', address=h, program=a.p)
        if r.startswith('{'):
            r = json.loads(r).get('decompiled', r)

        def rep(m):
            x = int(m.group(1), 16)
            i = bisect.bisect_right(addrs, x) - 1
            return f'{m.group(0)}/*{short(names[i])}*/' if i >= 0 and addrs[i] == x else m.group(0)
        r = re.sub(r'func_0x([0-9a-f]{8})', rep, r)
        open(path, 'w').write(r)
        i = bisect.bisect_right(addrs, addr) - 1
        sym = names[i] if i >= 0 and addrs[i] == addr else '?'
        with open(os.path.join(CACHE, 'dec', 'INDEX.tsv'), 'a') as f:
            f.write(f'{a.p}\t{addr:08x}\t{r.count(chr(10))}\t{sym}\n')
    body = open(path).read()
    sig = next((l for l in body.splitlines() if l.strip() and not l.startswith('//')), '')
    print(f'{path}  {body.count(chr(10))} lines  {sig[:160]}')


def cmd_cached(a):
    idx = os.path.join(CACHE, 'dec', 'INDEX.tsv')
    if os.path.exists(idx):
        rx = re.compile(a.regex or '')
        for line in open(idx):
            if rx.search(line):
                print(line.rstrip())


def cmd_note(a):
    d = os.path.join(CACHE, 'findings'); os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, f'{a.topic}.md'), 'a') as f:
        f.write(f'- {a.text}\n')


p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
s = p.add_subparsers(required=True)
x = s.add_parser('sym'); x.add_argument('regex'); x.add_argument('-n', type=int, default=30); x.set_defaults(f=cmd_sym)
x = s.add_parser('dec'); x.add_argument('target'); x.add_argument('-p', default='acbmp_sf.exe'); x.set_defaults(f=cmd_dec)
x = s.add_parser('cached'); x.add_argument('regex', nargs='?'); x.set_defaults(f=cmd_cached)
x = s.add_parser('note'); x.add_argument('topic'); x.add_argument('text'); x.set_defaults(f=cmd_note)
a = p.parse_args(); a.f(a)
