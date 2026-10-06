#!/usr/bin/env python3
"""Throwaway profiling harness for docs/research/dash-performance.md.

Runs N `cstan dash` processes under a pty against a scratch project, samples
CPU ticks and RSS of each dash and of the daemon from /proc once a second, and
counts the bytes each dash writes to its terminal.

usage: dash-bench.py --project DIR --cli dist/src/cli.js --dashes N --seconds S
                     [--interval SECONDS] [--keys '+++'] [--cpu-prof DIR]
Run it with CAPSTAN_SOCKET/CAPSTAN_TOKEN unset (it also strips them itself).
"""
import argparse, fcntl, json, os, pty, select, signal, struct, subprocess, sys, termios, time

CLK = os.sysconf("SC_CLK_TCK")

def ticks(pid):
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return int(f[11]) + int(f[12])
    except OSError:
        return None

def rss_kb(pid):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except OSError:
        return None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--project", required=True)
    ap.add_argument("--cli", required=True)
    ap.add_argument("--dashes", type=int, default=1)
    ap.add_argument("--seconds", type=int, default=60)
    ap.add_argument("--interval", type=int)
    ap.add_argument("--keys", default="")
    ap.add_argument("--cpu-prof")
    ap.add_argument("--extra", default="")
    ap.add_argument("--daemon-pid", type=int)
    a = ap.parse_args()
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CAPSTAN_", "HERDR_"))}
    env["TERM"] = "xterm-256color"
    env["PATH"] = "/tmp/cstprof/nobin:/usr/bin:/bin"
    procs = []
    for _ in range(a.dashes):
        m, s = pty.openpty()
        fcntl.ioctl(s, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        node = ["node"]
        if a.cpu_prof:
            node += ["--cpu-prof", "--cpu-prof-dir", a.cpu_prof]
        cmd = node + [a.cli, "dash"] + (["--interval", str(a.interval)] if a.interval else []) + a.extra.split()
        p = subprocess.Popen(cmd, cwd=a.project, env=env, stdin=s, stdout=s, stderr=s, close_fds=True, start_new_session=True)
        os.close(s)
        procs.append({"p": p, "m": m, "bytes": 0})
    time.sleep(3)  # let the first frame draw
    if a.keys:
        for pr in procs:
            os.write(pr["m"], a.keys.encode())
    pids = [pr["p"].pid for pr in procs]
    node_pids = []
    # the dash is the node process itself (no shell between), so Popen pid is it
    d_pid = a.daemon_pid
    base_d = ticks(d_pid) if d_pid else None
    base = [ticks(x) for x in pids]
    for pr in procs: pr["bytes"] = 0
    t0 = time.time(); samples = []
    while time.time() - t0 < a.seconds:
        r, _, _ = select.select([pr["m"] for pr in procs], [], [], 1.0)
        for pr in procs:
            if pr["m"] in r:
                try: pr["bytes"] += len(os.read(pr["m"], 65536))
                except OSError: pass
        samples.append([rss_kb(x) for x in pids] + [rss_kb(d_pid) if d_pid else None])
    el = time.time() - t0
    out = {"seconds": round(el, 1), "dashes": []}
    for i, pr in enumerate(procs):
        t = ticks(pids[i])
        out["dashes"].append({"cpu_pct": round((t - base[i]) / CLK / el * 100, 1) if t is not None else None,
                              "rss_mb_end": round((rss_kb(pids[i]) or 0) / 1024, 1),
                              "tty_bytes_per_s": round(pr["bytes"] / el),
                              "rss_mb_every_60s": [round((x[i] or 0) / 1024) for x in samples[::60]]})
    if d_pid:
        out["daemon"] = {"rss_mb_every_60s": [round((x[-1] or 0) / 1024) for x in samples[::60]], "cpu_pct": round((ticks(d_pid) - base_d) / CLK / el * 100, 1), "rss_mb_end": round(rss_kb(d_pid) / 1024, 1)}
    for pr in procs:
        try: os.write(pr["m"], b"q")
        except OSError: pass
    time.sleep(1.5)
    for pr in procs:
        if pr["p"].poll() is None:
            pr["p"].send_signal(signal.SIGTERM)
    print(json.dumps(out))
main()
