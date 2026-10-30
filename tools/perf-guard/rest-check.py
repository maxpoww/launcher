#!/usr/bin/env python3
"""THE DOCK AT REST, against a budget. Run it on any machine with the dock up:

    tools/perf-guard/rest-check.py [--secs 60] [--no-move] [--max-wakeups 30]
                                   [--max-cpu 1.0] [--max-spawned 2]

It moves to an empty workspace (a terminal animating its title elsewhere is not
rest), measures the dock's threads for --secs and comes back. Exit status 1 when
something is over budget, so a change that adds a timer or a poll is found by a
number rather than by a warm laptop months later.

Budgets are counts because a count is the same on a slow machine and a fast one.
The idea is Omarchy's (test/shell.d/shell-idle-spawn-test.sh: a shell at rest must
start no process), read 2026-10-30. Measured here that day: 100.7 wake-ups a
second and 0.60 % of a CPU before, 12.6 and 0.13 % after.

A "wake-up" is a context switch of a thread: a thread that went to sleep and was
woken. What each one is: strace the dock (ptrace_scope=1 means as its parent).
"""
import argparse, collections, json, os, subprocess, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("--secs", type=float, default=60)
ap.add_argument("--no-move", action="store_true", help="stay on this workspace")
ap.add_argument("--max-wakeups", type=float, default=30, help="per second, all threads")
ap.add_argument("--max-cpu", type=float, default=1.0, help="percent of one CPU")
ap.add_argument("--max-spawned", type=int, default=2, help="processes started during the window")
ap.add_argument("--pid", type=int, help="the dock's pid (default: found by name)")
a = ap.parse_args()


def find_dock():
    uid = os.getuid()
    found = []
    for p in os.listdir("/proc"):
        if not p.isdigit():
            continue
        try:
            if os.stat(f"/proc/{p}").st_uid != uid:
                continue
            if open(f"/proc/{p}/comm").read().strip() == "waverunner":
                found.append(int(p))
        except OSError:
            pass
    if not found:
        sys.exit("no dock is running")
    return min(found)


pid = a.pid or find_dock()
hz = os.sysconf("SC_CLK_TCK")


def tasks():
    return os.listdir(f"/proc/{pid}/task")


def threads():
    out = {}
    for t in tasks():
        try:
            d = open(f"/proc/{pid}/task/{t}/status").read()
            name = d.split("\n", 1)[0].split(":", 1)[1].strip()
            v = int(d.split("voluntary_ctxt_switches:")[1].split()[0])
            st = open(f"/proc/{pid}/task/{t}/stat").read().rsplit(")", 1)[1].split()
            out[t] = (name, v, int(st[11]) + int(st[12]))
        except (OSError, IndexError, ValueError):
            pass
    return out


def children():
    out = set()
    for t in tasks():
        try:
            out.update(open(f"/proc/{pid}/task/{t}/children").read().split())
        except OSError:
            pass
    return out


def hypr(*args):
    return subprocess.run(["hyprctl", *args], capture_output=True, text=True, timeout=5).stdout


was = None
if not a.no_move:
    try:
        was = json.loads(hypr("activeworkspace", "-j"))["id"]
        taken = {w["id"] for w in json.loads(hypr("workspaces", "-j")) if w.get("windows", 0) > 0}
        free = next(n for n in range(9, 1, -1) if n not in taken)
        hypr("dispatch", f'hl.dsp.focus({{ workspace = "{free}" }})')
        time.sleep(4)
    except (OSError, ValueError, KeyError, StopIteration, subprocess.SubprocessError) as e:
        print(f"(not moved to an empty workspace: {e})")
        was = None

before, residents = threads(), children()
spawned = collections.Counter()
t0 = time.time()
seen = set(residents)
try:
    while time.time() - t0 < a.secs:
        for k in children() - seen:
            seen.add(k)
            try:
                cmd = open(f"/proc/{k}/cmdline").read().replace("\0", " ").strip()
            except OSError:
                cmd = ""
            spawned[" ".join(cmd.split()[:3])[:70] or "(gone before it was read)"] += 1
        time.sleep(0.004)
    after = threads()
    dt = time.time() - t0
finally:
    if was is not None:
        hypr("dispatch", f'hl.dsp.focus({{ workspace = "{was}" }})')

rows = []
for t, (name, v, c) in after.items():
    b = before.get(t, (name, v, c))
    rows.append(((v - b[1]) / dt, (c - b[2]) / hz / dt * 100, name))
rows.sort(reverse=True)
wakeups = sum(r[0] for r in rows)
cpu = sum(r[1] for r in rows)
n = sum(spawned.values())
print(f"dock {pid}, {dt:.0f} s at rest, {len(after)} threads")
for w, c, name in rows[:6]:
    if w >= 0.05:
        print(f"  {w:7.1f}/s {c:5.2f}%  {name}")
print(f"spawned: {n}" + "".join(f"\n  {v:4d}  {k}" for k, v in spawned.most_common(10)))
over = []
if wakeups > a.max_wakeups:
    over.append(f"wake-ups {wakeups:.1f}/s > {a.max_wakeups}")
if cpu > a.max_cpu:
    over.append(f"cpu {cpu:.2f}% > {a.max_cpu}")
if n > a.max_spawned:
    over.append(f"spawned {n} > {a.max_spawned}")
print(f"wake-ups {wakeups:.1f}/s, cpu {cpu:.2f}%, spawned {n}: " + ("OVER BUDGET: " + "; ".join(over) if over else "within budget"))
sys.exit(1 if over else 0)
